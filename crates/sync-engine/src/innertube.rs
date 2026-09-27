//! Application policy over the shared typed youtubei bindings.
use crate::api::{Api, read_bounded};
use anyhow::{Context, Result, bail, ensure};
use std::collections::HashSet;
use youtubei::{
    BrowseOptions, Client, Engine, EngineOptions, FetchRequest, FetchResponse, Format,
    GetVideoInfoOptions, Innertube, Player, Playlist, SessionOptions, UniversalCache,
    models::{
        ContentImage, LockupContentType, Microformat, PlaylistAlert, PlaylistItem, ThumbnailOverlay,
    },
};

pub struct YouTube {
    client: Innertube,
    parse_failed: std::rc::Rc<std::cell::Cell<bool>>,
}

pub struct PlaylistSnapshot {
    pub title: String,
    pub present: Vec<String>,
    /// Sum of known listing durations, including unplayable entries, rounded up per video.
    pub estimated_seconds: i64,
    /// Missing IDs prove removal only when the full scan has no warnings.
    pub can_remove: bool,
}

pub struct Media {
    pub estimated_seconds: i64,
    pub title: String,
    pub description: String,
    pub published_at: i64,
    pub video: Stream,
    pub audio: Option<Stream>,
}

pub struct Stream {
    pub url: String,
    pub user_agent: String,
    pub identity: String,
}

impl YouTube {
    pub async fn new(api: &Api) -> Result<Self> {
        let engine = Engine::with_options(EngineOptions {
            memory_limit: 256 * 1024 * 1024,
            stack_size: 4 * 1024 * 1024,
        })
        .await?;
        let cancel = api.cancel.clone();
        engine
            .set_interrupt_handler(move || cancel.is_cancelled())
            .await;
        let parse_failed = std::rc::Rc::new(std::cell::Cell::new(false));
        let failed = parse_failed.clone();
        let callback = engine
            .value_with(|ctx| {
                Ok(youtubei::rquickjs::Function::new(
                    ctx,
                    move |_error: youtubei::rquickjs::Value<'_>| {
                        failed.set(true);
                    },
                )?
                .into_value())
            })
            .await?;
        engine
            .export(&["Parser"])
            .await?
            .call("setParserErrorHandler", &[callback.into()])
            .await?;
        let api = api.clone();
        let continuations = std::sync::Arc::new(parking_lot::Mutex::new(HashSet::new()));
        let fetch = engine
            .fetch_with(move |request| {
                let api = api.clone();
                let continuations = continuations.clone();
                async move {
                    if request.url.contains("/youtubei/v1/browse")
                        && let Some(token) = request
                            .body
                            .as_deref()
                            .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
                            .and_then(|body| body["continuation"].as_str().map(str::to_owned))
                        && !continuations.lock().insert(token)
                    {
                        return Err(youtubei::Error::new("Repeated YouTube continuation"));
                    }
                    fetch(&api, request)
                        .await
                        .map_err(|error| youtubei::Error::new(error.to_string()))
                }
            })
            .await?;
        let cache = UniversalCache::new(&engine, false, None).await?;
        let client = Innertube::create_in(
            &engine,
            SessionOptions {
                lang: Some("en".into()),
                location: Some("US".into()),
                cache: Some(cache.as_cache()),
                fetch: Some(fetch),
                ..SessionOptions::local()
            },
        )
        .await
        .context("initialize YouTube")?;
        Ok(Self {
            client,
            parse_failed,
        })
    }

    pub async fn snapshot(&self, api: &Api, id: &str) -> Result<PlaylistSnapshot> {
        api.wait(async {
            self.parse_failed.set(false);
            let actions = self.client.actions().await?;
            let response = actions
                .browse(BrowseOptions {
                    browse_id: format!("VL{id}"),
                    // Ask YouTube to include entries hidden by its default view.
                    params: Some("wgYCCAA=".into()),
                })
                .await?;
            let mut page = Playlist::new(&actions, &response, false).await?;
            let mut title = None;
            let mut present = Vec::new();
            let mut estimated_seconds = 0_i64;
            let mut seen = HashSet::new();
            let mut pages = 0;
            let mut can_remove = true;
            loop {
                let data = page.data().await?;
                ensure!(
                    !self.parse_failed.get(),
                    "YouTube playlist could not be parsed completely; no changes were made"
                );
                for alert in data.alerts {
                    match alert {
                        PlaylistAlert::Alert(alert) | PlaylistAlert::AlertWithButton(alert)
                            if matches!(alert.alert_type.as_str(), "INFO" | "WARNING") =>
                        {
                            // Returned IDs can be checked, but an alert may mean
                            // omitted IDs are hidden rather than removed.
                            can_remove = false;
                        }
                        _ => bail!("YouTube could not list this playlist"),
                    }
                }
                if title.is_none() {
                    title = data.info.title;
                }
                for item in data.items {
                    let (id, seconds) = match item {
                        PlaylistItem::PlaylistVideo(video) => {
                            ensure!(
                                valid_video_id(&video.id),
                                "Playlist contains an unidentified unavailable item"
                            );
                            (video.id, known_seconds(video.duration.seconds))
                        }
                        PlaylistItem::LockupView(video)
                            if matches!(
                                video.content_type,
                                LockupContentType::Video | LockupContentType::Short
                            ) =>
                        {
                            ensure!(
                                valid_video_id(&video.content_id),
                                "Playlist contains an invalid video ID"
                            );
                            let seconds =
                                match video.content_image {
                                    Some(ContentImage::ThumbnailView { overlays }) => overlays
                                        .iter()
                                        .find_map(|overlay| match overlay {
                                            ThumbnailOverlay::ThumbnailOverlayBadgeView {
                                                badges,
                                            }
                                            | ThumbnailOverlay::ThumbnailBottomOverlayView {
                                                badges,
                                            } => badges
                                                .iter()
                                                .filter_map(|badge| badge.text.as_deref())
                                                .find_map(duration_badge_seconds),
                                            _ => None,
                                        })
                                        .unwrap_or(0),
                                    _ => 0,
                                };
                            (video.content_id, seconds)
                        }
                        _ => bail!("Unsupported playlist item; listing is incomplete"),
                    };
                    if seen.insert(id.clone()) {
                        estimated_seconds = estimated_seconds
                            .checked_add(seconds)
                            .context("Playlist duration exceeds the supported range")?;
                        present.push(id);
                    }
                }
                pages += 1;
                ensure!(pages <= 10000, "YouTube playlist exceeds the scan limit");
                if !data.has_continuation {
                    break;
                }
                page = page.get_continuation().await?;
            }

            Ok(PlaylistSnapshot {
                title: title
                    .filter(|title| !title.is_empty())
                    .context("YouTube playlist missing title")?,
                present,
                estimated_seconds,
                can_remove,
            })
        })
        .await
    }

    pub async fn media(&self, api: &Api, id: &str) -> Result<Option<Media>> {
        api.wait(async {
            let info = match self
                .client
                .get_basic_info(
                    id,
                    GetVideoInfoOptions {
                        client: Some(Client::VisionOs),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(info) => info,
                // YouTube.js throws for ERROR player responses before exposing
                // VideoInfo. Inspect its structured status, never error wording.
                Err(error)
                    if error
                        .info
                        .as_ref()
                        .is_some_and(|info| info["status"] == "ERROR") =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
            let data = info.data().await?;
            let status = data
                .playability_status
                .as_ref()
                .context("YouTube player response missing playability status")?;
            if status.status != "OK"
                || data.basic_info.is_live.unwrap_or(false)
                || data.basic_info.is_upcoming.unwrap_or(false)
            {
                return Ok(None);
            }
            let mut formats = info.formats().await?;
            formats.extend(info.adaptive_formats().await?);
            formats.retain(|format| {
                let f = format.info();
                f.drm_families.as_ref().is_none_or(Vec::is_empty)
                    && !f.is_type_otf
                    && (f.url.is_some() || f.cipher.is_some() || f.signature_cipher.is_some())
            });
            let video = formats
                .iter()
                .filter(|format| {
                    let f = format.info();
                    f.has_video
                        && f.mime_type.contains("avc1")
                        && f.height
                            .is_some_and(|height| height > 0.0 && height <= 1080.0)
                })
                .max_by(|a, b| {
                    a.info()
                        .height
                        .unwrap()
                        .total_cmp(&b.info().height.unwrap())
                })
                .context("YouTube video has no AVC rendition at or below 1080p")?;
            let audio = if video.info().has_audio {
                None
            } else {
                Some(
                    formats
                        .iter()
                        .filter(|format| format.info().has_audio && !format.info().has_video)
                        .max_by(|a, b| a.info().bitrate.total_cmp(&b.info().bitrate))
                        .context("YouTube video has no audio stream")?,
                )
            };
            let published = match data.microformat {
                Some(Microformat::PlayerMicroformat(metadata)) => metadata
                    .publish_date
                    .filter(|date| !date.is_empty())
                    .or(metadata.upload_date),
                _ => None,
            }
            .context("YouTube video has no publication date")?;
            let published_at = chrono::DateTime::parse_from_rfc3339(&published)
                .ok()
                .map(|date| date.to_utc())
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(&published, "%Y-%m-%d")
                        .ok()?
                        .and_hms_opt(0, 0, 0)
                        .map(|date| date.and_utc())
                })
                .context("YouTube publication date is invalid")?
                .timestamp_millis();
            Ok(Some(Media {
                estimated_seconds: known_seconds(data.basic_info.duration.unwrap_or(0.0)),
                title: data.basic_info.title.unwrap_or_else(|| id.into()),
                description: data.basic_info.short_description.unwrap_or_default(),
                published_at,
                video: self.stream(video, &data.cpn).await?,
                audio: match audio {
                    Some(format) => Some(self.stream(format, &data.cpn).await?),
                    None => None,
                },
            }))
        })
        .await
    }

    async fn stream(&self, format: &Format, cpn: &str) -> Result<Stream> {
        let session = self.client.session().await?;
        let info = format.info();
        let needs_player = info.cipher.is_some()
            || info.signature_cipher.is_some()
            || info.url.as_ref().is_some_and(|url| {
                url::Url::parse(url).is_ok_and(|url| url.query_pairs().any(|(key, _)| key == "n"))
            });
        if needs_player && session.player().await?.is_none() {
            let player = Player::create(
                self.client.engine(),
                session.cache().await?.as_ref(),
                Some(&session.fetch().await?),
                None,
                None,
            )
            .await?;
            session.set_player(&player).await?;
        }
        let mut url = url::Url::parse(&format.decipher(session.player().await?.as_ref()).await?)?;
        url.query_pairs_mut().append_pair("cpn", cpn);
        Ok(Stream {
            identity: format!("{}:{}:{:?}", info.itag, info.mime_type, info.content_length),
            url: url.into(),
            user_agent: Client::VisionOs
                .user_agent(self.client.engine())
                .await?
                .context("YouTube VISIONOS user agent missing")?,
        })
    }
}

async fn fetch(api: &Api, input: FetchRequest) -> Result<FetchResponse> {
    let url = url::Url::parse(&input.url)?;
    let host = url.host_str().unwrap_or("");
    ensure!(
        url.scheme() == "https"
            && [
                "youtube.com",
                "google.com",
                "googleapis.com",
                "googlevideo.com",
                "ytimg.com"
            ]
            .iter()
            .any(|base| host == *base || host.ends_with(&format!(".{base}"))),
        "unexpected YouTube API host"
    );
    let mut request = api.http.request(input.method.parse()?, url);
    for (name, value) in input.headers {
        if !["host", "content-length", "cookie"].contains(&name.to_ascii_lowercase().as_str()) {
            request = request.header(name, value);
        }
    }
    if let Some(body) = input.body {
        request = request.body(body);
    }
    let response = api.send(request).await?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(key, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (key.to_string(), value.to_owned()))
        })
        .collect();
    let body = api.wait(read_bounded(response, 32 << 20)).await?;
    Ok(FetchResponse {
        status,
        headers,
        body,
    })
}

fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

// Listing metadata can omit durations or use non-duration badges (LIVE, etc.).
// Unknown lengths contribute zero; admission never requests individual players.
fn known_seconds(seconds: f64) -> i64 {
    if seconds.is_finite() && seconds > 0.0 {
        seconds.ceil() as i64
    } else {
        0
    }
}

fn duration_badge_seconds(text: &str) -> Option<i64> {
    let parts: Vec<_> = text.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut total = 0_i64;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let value = part.parse::<i64>().ok()?;
        if index > 0 && value >= 60 {
            return None;
        }
        total = total.checked_mul(60)?.checked_add(value)?;
    }
    Some(total)
}
