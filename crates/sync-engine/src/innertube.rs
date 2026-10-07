//! Application policy over the shared typed youtubei bindings.
use crate::api::{Api, read_bounded};
use crate::cookies::{CookieJar, SignInRequired};
use anyhow::{Context, Result, bail, ensure};
use sha1::{Digest, Sha1};
use std::{collections::HashSet, future::Future};
use tokio_util::task::TaskTracker;
use youtubei::{
    BrowseOptions, Client, Engine, EngineOptions, FetchRequest, FetchResponse, Format,
    GetVideoInfoOptions, Innertube, Player, Playlist, SessionOptions, UniversalCache, VideoInfo,
    models::{
        ContentImage, LockupContentType, Microformat, PlayabilityStatus, PlayerErrorScreen,
        PlaylistAlert, PlaylistItem, ThumbnailOverlay,
    },
};

pub struct YouTube {
    client: Innertube,
    user_agent: String,
    playback_client: Client,
    cookie_writes: TaskTracker,
}

pub struct PlaylistSnapshot {
    pub title: String,
    pub artwork_url: Option<String>,
    pub present: Vec<Video>,
    /// Sum of known listing durations, including unplayable entries, rounded up per video.
    pub estimated_seconds: i64,
    /// Missing IDs prove removal only when the full scan has no warnings.
    pub can_remove: bool,
}

/// Observed unique entries from the pages read so far, never a guessed total.
#[derive(Clone, Debug)]
pub struct PlaylistScan {
    pub title: String,
    pub videos: usize,
    pub estimated_seconds: i64,
    pub unknown_durations: usize,
}

#[derive(Clone)]
pub struct Video {
    pub id: String,
    pub title: String,
    /// Only real source titles may replace published metadata.
    pub source_title: Option<String>,
    pub duration_seconds: Option<u64>,
}

enum PlayerResponse {
    Available(VideoInfo),
    Unavailable(String),
}

pub enum Playback {
    Available(Box<Media>),
    Unavailable(String),
}

pub struct Media {
    fallback_clients: std::vec::IntoIter<Client>,
    pub estimated_seconds: i64,
    pub duration_seconds: Option<u64>,
    pub title: String,
    pub artwork_url: Option<String>,
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
    #[tracing::instrument(name = "youtube.initialize", skip_all)]
    pub async fn new(api: &Api) -> Result<Self> {
        let cookie_writes = TaskTracker::new();
        // Closed trackers still admit tasks. Closing makes wait() resolve when
        // the admitted writes reach zero, including after a JS promise is dropped.
        cookie_writes.close();
        let result = api
            .wait(async {
                let jar = CookieJar::new(&api.config.directory);
                let snapshot_jar = jar.clone();
                let initial =
                    tokio::task::spawn_blocking(move || snapshot_jar.snapshot()).await??;
                let cookie = initial.header(&url::Url::parse("https://www.youtube.com/")?);
                let playback_client = if cookie.split("; ").any(|part| {
                    part.split_once('=').is_some_and(|(name, value)| {
                        !value.is_empty()
                            && matches!(name, "SAPISID" | "__Secure-1PAPISID" | "__Secure-3PAPISID")
                    })
                }) {
                    Client::WebEmbedded
                } else {
                    Client::VisionOs
                };
                let cancel = api.cancel.clone();
                let engine = Engine::with_options(EngineOptions {
                    interrupt_handler: Some(Box::new(move || cancel.is_cancelled())),
                    ..Default::default()
                })
                .await?;
                // UI diagnostics (including dynamically generated parsers) do not
                // establish playlist loss. The listing's is_complete flag does.
                let callback = engine
                    .value_with(|ctx| {
                        Ok(youtubei::rquickjs::Function::new(
                            ctx,
                            |_: youtubei::rquickjs::Object<'_>| {},
                        )?
                        .into_value())
                    })
                    .await?;
                engine
                    .export(&["Parser"])
                    .await?
                    .call("setParserErrorHandler", &[callback.into()])
                    .await?;
                let mut api = api.clone();
                // Never forward browser credentials through a redirect.
                api.http = Api::http_client(true)?;
                let continuations = std::sync::Arc::new(parking_lot::Mutex::new(HashSet::new()));
                let writes = cookie_writes.clone();
                let fetch = engine
                    .fetch_with(move |request| {
                        let api = api.clone();
                        let jar = jar.clone();
                        let continuations = continuations.clone();
                        let writes = writes.clone();
                        async move {
                            if request.url.contains("/youtubei/v1/browse")
                                && let Some(token) = request
                                    .body
                                    .as_deref()
                                    .and_then(|body| {
                                        serde_json::from_slice::<serde_json::Value>(body).ok()
                                    })
                                    .and_then(|body| {
                                        body["continuation"].as_str().map(str::to_owned)
                                    })
                                && !continuations.lock().insert(token)
                            {
                                return Err(youtubei::Error::new("Repeated YouTube continuation"));
                            }
                            fetch(&api, &jar, &writes, request)
                                .await
                                .map_err(crate::errors::youtube_error)
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
                        cookie: (!cookie.is_empty()).then_some(cookie),
                        generate_session_locally: Some(false),
                        fail_fast: Some(true),
                        // Web and embedded playback need the player signature timestamp.
                        retrieve_player: Some(true),
                        retrieve_innertube_config: Some(false),
                        ..Default::default()
                    },
                )
                .await
                .context("initialize YouTube")?;
                let user_agent = match playback_client {
                    Client::VisionOs => {
                        engine
                            .export(&["Constants", "CLIENTS", "VISIONOS", "USER_AGENT"])
                            .await?
                            .deserialize::<String>()
                            .await?
                    }
                    _ => client.session().await?.user_agent().await?,
                };
                Ok(Self {
                    client,
                    user_agent,
                    playback_client,
                    cookie_writes: cookie_writes.clone(),
                })
            })
            .await;
        cookie_writes.wait().await;
        result
    }

    async fn wait<T>(&self, api: &Api, work: impl Future<Output = Result<T>>) -> Result<T> {
        let result = api.wait(work).await;
        // Cancelling JS/network waits must not detach an admitted atomic cookie
        // replacement. Its lock wait is cancellable; an acquired commit finishes.
        self.cookie_writes.wait().await;
        result
    }

    pub async fn snapshot(&self, api: &Api, id: &str) -> Result<PlaylistSnapshot> {
        let (_, page) = self.playlist_head(api, id).await?;
        self.complete_playlist(api, page, |_| {}).await
    }

    /// Read only the first page before creating an audio podcast. Video
    /// admission completes the same scan before checking total storage.
    #[tracing::instrument(name = "youtube.playlist", skip_all, fields(playlist_id = id))]
    pub(crate) async fn playlist_head(&self, api: &Api, id: &str) -> Result<(String, Playlist)> {
        self.wait(api, async {
            let actions = self.client.actions().await?;
            let response = actions
                .browse(BrowseOptions {
                    browse_id: format!("VL{id}"),
                    // Ask YouTube to include entries hidden by its default view.
                    params: Some("wgYCCAA=".into()),
                })
                .await?;
            let page = Playlist::new(&actions, &response, false).await?;
            let data = page.data().await?;
            ensure!(
                data.is_complete,
                "YouTube playlist could not be parsed completely; no changes were made"
            );
            for alert in data.alerts {
                match alert {
                    PlaylistAlert::Alert(alert) | PlaylistAlert::AlertWithButton(alert)
                        if matches!(alert.alert_type.as_str(), "INFO" | "WARNING") => {}
                    _ => bail!("YouTube could not list this playlist"),
                }
            }
            let title = data
                .info
                .title
                .filter(|title| !title.trim().is_empty())
                .context("YouTube playlist missing title")?;
            Ok((title, page))
        })
        .await
    }

    #[tracing::instrument(name = "youtube.scan", skip_all)]
    pub(crate) async fn complete_playlist(
        &self,
        api: &Api,
        mut page: Playlist,
        mut progress: impl FnMut(PlaylistScan),
    ) -> Result<PlaylistSnapshot> {
        self.wait(api, async {
            let mut title = None;
            let mut artwork_url = None;
            let mut present = Vec::new();
            let mut estimated_seconds = 0_i64;
            let mut seen = HashSet::new();
            let mut pages = 0;
            let mut can_remove = true;
            let mut unknown_durations = 0;
            loop {
                let data = page.data().await?;
                ensure!(
                    data.is_complete,
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
                if artwork_url.is_none() {
                    artwork_url = thumbnail_url(data.info.thumbnails);
                }
                for item in data.items {
                    let (video, seconds) = match item {
                        PlaylistItem::PlaylistVideo(video) => {
                            ensure!(
                                valid_video_id(&video.id),
                                "Playlist contains an unidentified unavailable item"
                            );
                            let source_title = video
                                .title
                                .source_text()
                                .filter(|title| video.is_playable && !title.trim().is_empty())
                                .map(str::to_owned);
                            let title = video.title.into_string();
                            (
                                Video {
                                    id: video.id,
                                    title,
                                    source_title,
                                    duration_seconds: duration_seconds(video.duration.seconds),
                                },
                                known_seconds(video.duration.seconds),
                            )
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
                            let duration = match video.content_image {
                                Some(ContentImage::ThumbnailView { overlays }) => {
                                    overlays.into_iter().find_map(|overlay| {
                                        let badges = match overlay {
                                            ThumbnailOverlay::ThumbnailOverlayBadgeView {
                                                badges,
                                            }
                                            | ThumbnailOverlay::ThumbnailBottomOverlayView {
                                                badges,
                                            } => badges,
                                            _ => return None,
                                        };
                                        badges
                                            .into_iter()
                                            .filter_map(|badge| badge.text)
                                            .find_map(|text| duration_badge_seconds(&text))
                                    })
                                }
                                _ => None,
                            };
                            let source_title = video
                                .metadata
                                .and_then(|metadata| metadata.title)
                                .and_then(|title| title.source_text().map(str::to_owned))
                                .filter(|title| !title.trim().is_empty());
                            (
                                Video {
                                    title: source_title.clone().unwrap_or_else(|| {
                                        format!("YouTube video {}", video.content_id)
                                    }),
                                    source_title,
                                    id: video.content_id,
                                    duration_seconds: duration
                                        .and_then(|seconds| duration_seconds(seconds as f64)),
                                },
                                duration.unwrap_or(0),
                            )
                        }
                        _ => bail!("Unsupported playlist item; listing is incomplete"),
                    };
                    if seen.insert(video.id.clone()) {
                        unknown_durations += usize::from(seconds == 0);
                        estimated_seconds = estimated_seconds
                            .checked_add(seconds)
                            .context("Playlist duration exceeds the supported range")?;
                        present.push(video);
                    }
                }
                pages += 1;
                ensure!(pages <= 10000, "YouTube playlist exceeds the scan limit");
                progress(PlaylistScan {
                    title: title.clone().context("YouTube playlist missing title")?,
                    videos: present.len(),
                    estimated_seconds,
                    unknown_durations,
                });
                if !data.has_continuation {
                    break;
                }
                page = page.get_continuation().await?;
            }

            Ok(PlaylistSnapshot {
                title: title
                    .filter(|title| !title.is_empty())
                    .context("YouTube playlist missing title")?,
                artwork_url,
                present,
                estimated_seconds,
                can_remove,
            })
        })
        .await
    }

    /// A single-video show still reconciles source metadata after publication,
    /// without resolving streams or downloading its media again.
    pub(crate) async fn video_snapshot(&self, api: &Api, id: &str) -> Result<PlaylistSnapshot> {
        self.wait(api, async {
            let (source_title, artwork_url, duration_seconds) =
                match self.player_info(id, Client::Web).await? {
                    PlayerResponse::Available(info) => {
                        let data = info.data().await?;
                        (
                            data.basic_info
                                .title
                                .filter(|title| !title.trim().is_empty()),
                            thumbnail_url(data.basic_info.thumbnail),
                            data.basic_info.duration.and_then(duration_seconds),
                        )
                    }
                    PlayerResponse::Unavailable(_) => (None, None, None),
                };
            let title = source_title
                .clone()
                .unwrap_or_else(|| format!("YouTube video {id}"));
            Ok(PlaylistSnapshot {
                title: title.clone(),
                artwork_url,
                present: vec![Video {
                    id: id.into(),
                    title,
                    source_title,
                    duration_seconds,
                }],
                estimated_seconds: 0,
                can_remove: true,
            })
        })
        .await
    }

    async fn player_info(&self, id: &str, client: Client) -> Result<PlayerResponse> {
        let info = match self
            .client
            .get_basic_info(
                id,
                GetVideoInfoOptions {
                    client: Some(client),
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
                    .playability_status
                    .as_ref()
                    .is_some_and(|status| status.status == "ERROR") =>
            {
                return Ok(PlayerResponse::Unavailable(unavailable_reason(
                    error.playability_status.as_ref().unwrap(),
                )));
            }
            Err(error) => return Err(error.into()),
        };
        let data = info.data().await?;
        let status = data
            .playability_status
            .as_ref()
            .context("YouTube player response missing playability status")?;
        match status.status.as_str() {
            "OK" => {}
            "UNPLAYABLE" => {
                return Ok(PlayerResponse::Unavailable(unavailable_reason(status)));
            }
            "LOGIN_REQUIRED" => {
                return Err(anyhow::Error::new(SignInRequired).context(format!(
                    "{} ({id})",
                    status.reason.as_deref().unwrap_or("Sign in required")
                )));
            }
            _ => bail!(
                "YouTube could not resolve {id} ({}): {}",
                status.status,
                status.reason.as_deref().unwrap_or("No reason supplied")
            ),
        }
        ensure!(
            data.basic_info.id.as_deref() == Some(id),
            "YouTube metadata video ID differs from the requested video"
        );
        Ok(PlayerResponse::Available(info))
    }

    #[tracing::instrument(name = "youtube.media", skip_all, fields(video_id = id))]
    pub async fn media(&self, api: &Api, id: &str, collection: &str) -> Result<Playback> {
        self.wait(api, async {
            let metadata_client = if matches!(self.playback_client, Client::WebEmbedded) {
                Client::Mweb
            } else {
                Client::Web
            };
            let info = match self.player_info(id, metadata_client).await? {
                PlayerResponse::Available(info) => info,
                PlayerResponse::Unavailable(reason) => return Ok(Playback::Unavailable(reason)),
            };
            let data = info.data().await?;
            if data.basic_info.is_live.unwrap_or(false)
                || data.basic_info.is_upcoming.unwrap_or(false)
            {
                return Ok(Playback::Unavailable(
                    "Live or upcoming video; sync after it has finished".into(),
                ));
            }
            // Mobile web supplies publication metadata, but its direct streams
            // require proof of origin. Ask the session's preferred playback
            // client immediately rather than learning that through failed ranges.
            let mut selected_client = self.playback_client;
            let playback = match self.player_info(id, selected_client).await? {
                PlayerResponse::Available(info) => info,
                // Embedding refusal does not establish video unavailability.
                PlayerResponse::Unavailable(_)
                    if matches!(selected_client, Client::WebEmbedded) =>
                {
                    selected_client = Client::WebCreator;
                    match self.player_info(id, selected_client).await? {
                        PlayerResponse::Available(info) => info,
                        PlayerResponse::Unavailable(reason) => {
                            return Ok(Playback::Unavailable(reason));
                        }
                    }
                }
                PlayerResponse::Unavailable(reason) => {
                    return Ok(Playback::Unavailable(reason));
                }
            };
            let mut formats = playback.formats().await?;
            formats.extend(playback.adaptive_formats().await?);
            let cpn = playback.cpn().await?;
            let (video, audio) = self
                .select_streams(id, formats, &cpn, data.basic_info.duration, selected_client)
                .await?;
            ensure!(
                data.basic_info.id.as_deref() == Some(id),
                "YouTube metadata video ID differs from the requested video"
            );
            let published = match data.microformat {
                Some(Microformat::PlayerMicroformat(metadata)) => {
                    let (primary, secondary) =
                        if collection.starts_with("https://www.youtube.com/channel/") {
                            (metadata.upload_date, metadata.publish_date)
                        } else {
                            (metadata.publish_date, metadata.upload_date)
                        };
                    primary
                        .filter(|date| !date.is_empty())
                        .or(secondary.filter(|date| !date.is_empty()))
                }
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
            Ok(Playback::Available(Box::new(Media {
                // A media denial gets one native alternative. Creator playback
                // is reserved for an embedding refusal, not another media probe.
                fallback_clients: match selected_client {
                    Client::WebEmbedded | Client::WebCreator => vec![Client::VisionOs],
                    _ => Vec::new(),
                }
                .into_iter(),
                estimated_seconds: known_seconds(data.basic_info.duration.unwrap_or(0.0)),
                duration_seconds: data.basic_info.duration.and_then(duration_seconds),
                title: data.basic_info.title.unwrap_or_else(|| id.into()),
                artwork_url: thumbnail_url(data.basic_info.thumbnail),
                description: data.basic_info.short_description.unwrap_or_default(),
                published_at,
                video,
                audio,
            })))
        })
        .await
    }

    // Player permission is not proof that its media URLs allow a full download.
    // A client's refusal does not establish source unavailability. Only streams
    // change here; the initial response's publication metadata stays authoritative.
    pub(crate) async fn fallback_media(
        &self,
        api: &Api,
        id: &str,
        mut media: Box<Media>,
    ) -> Result<Option<Box<Media>>> {
        self.wait(api, async {
            for client in media.fallback_clients.by_ref() {
                let playback = match self.player_info(id, client).await {
                    Ok(PlayerResponse::Available(info)) => info,
                    Ok(PlayerResponse::Unavailable(_)) => continue,
                    // Anonymous native playback cannot access restricted videos.
                    Err(error)
                        if matches!(client, Client::VisionOs) && error.is::<SignInRequired>() =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let mut formats = playback.formats().await?;
                formats.extend(playback.adaptive_formats().await?);
                let cpn = playback.cpn().await?;
                let (video, audio) = self
                    .select_streams(
                        id,
                        formats,
                        &cpn,
                        media.duration_seconds.map(|seconds| seconds as f64),
                        client,
                    )
                    .await?;
                media.video = video;
                media.audio = audio;
                return Ok(Some(media));
            }
            Ok(None)
        })
        .await
    }

    async fn select_streams(
        &self,
        id: &str,
        mut formats: Vec<Format>,
        cpn: &str,
        duration: Option<f64>,
        client: Client,
    ) -> Result<(Stream, Option<Stream>)> {
        ensure!(
            formats.iter().any(|format| {
                let f = format.info();
                f.url.is_some() || f.cipher.is_some() || f.signature_cipher.is_some()
            }),
            "YouTube returned no downloadable stream URLs; video resolution is not the problem"
        );
        formats.retain(|format| {
            let f = format.info();
            f.drm_families.as_ref().is_none_or(Vec::is_empty)
                && !f.is_type_otf
                && (f.url.is_some() || f.cipher.is_some() || f.signature_cipher.is_some())
        });
        // AAC can be copied into M4A, so prefer it before comparing bitrates.
        let audio = formats
            .iter()
            .filter(|format| {
                let f = format.info();
                f.has_audio
                    && !f.has_video
                    && f.audio_track
                        .as_ref()
                        .is_none_or(|track| track.audio_is_default)
            })
            .max_by(|a, b| {
                let (a, b) = (a.info(), b.info());
                a.mime_type
                    .contains("mp4a.40.")
                    .cmp(&b.mime_type.contains("mp4a.40."))
                    .then_with(|| a.bitrate.total_cmp(&b.bitrate))
            });
        let embedded_aac = |format: &Format| {
            let f = format.info();
            f.has_audio && f.mime_type.contains("mp4a.40.")
        };
        let separate_audio = |video: &Format| {
            // Keep standalone AAC for audio-only imports and its default track.
            // Embedded AAC avoids using a standalone stream that needs encoding.
            if embedded_aac(video)
                && !audio.is_some_and(|audio| audio.info().mime_type.contains("mp4a.40."))
            {
                None
            } else {
                audio
            }
        };
        let copyable_audio = |video: &Format| {
            embedded_aac(video)
                || audio.is_some_and(|audio| audio.info().mime_type.contains("mp4a.40."))
        };
        let seconds = duration.filter(|seconds| seconds.is_finite() && *seconds > 0.0);
        let download_bitrate = |video: &Format| {
            let valid_rate = |format: &Format| {
                if let Some(seconds) = seconds
                    && let Some(length) = format
                        .info()
                        .content_length
                        .filter(|length| length.is_finite() && *length > 0.0)
                {
                    return length * 8.0 / seconds;
                }
                let rate = format.info().bitrate;
                if rate.is_finite() && rate > 0.0 {
                    rate
                } else {
                    f64::INFINITY
                }
            };
            valid_rate(video) + separate_audio(video).map_or(0.0, valid_rate)
        };
        let frame_rate = |format: &Format| {
            format
                .info()
                .fps
                .filter(|fps| fps.is_finite() && *fps > 0.0)
                .unwrap_or(0.0)
        };
        // Select actual source quality; packaging applies the delivery ceiling.
        // Within that resolution/frame rate, avoid AAC encoding, then minimize bytes.
        let video = formats
            .iter()
            .filter(|format| {
                let f = format.info();
                f.has_video
                    && (f.mime_type.contains("avc1")
                        || f.mime_type.contains("vp09")
                        || f.mime_type.contains("hev1")
                        || f.mime_type.contains("hvc1"))
                    && f.height
                        .is_some_and(|height| height.is_finite() && height > 0.0)
                    && f.audio_track
                        .as_ref()
                        .is_none_or(|track| track.audio_is_default)
                    && (f.has_audio || audio.is_some())
            })
            .max_by(|a, b| {
                a.info()
                    .height
                    .unwrap()
                    .total_cmp(&b.info().height.unwrap())
                    .then_with(|| frame_rate(a).total_cmp(&frame_rate(b)))
                    .then_with(|| copyable_audio(a).cmp(&copyable_audio(b)))
                    .then_with(|| download_bitrate(b).total_cmp(&download_bitrate(a)))
                    .then_with(|| b.info().itag.cmp(&a.info().itag))
            })
            .context("YouTube video has no supported video rendition with audio")?;
        let audio = separate_audio(video);
        let selected_audio = audio.unwrap_or(video).info();
        if cfg!(debug_assertions) {
            eprintln!(
                "YouTube media selected video_id={id} video_itag={} video_mime={:?} audio_itag={} audio_mime={:?} audio_bitrate_bps={} audio_has_video={}",
                video.info().itag,
                video.info().mime_type,
                selected_audio.itag,
                selected_audio.mime_type,
                selected_audio.bitrate,
                selected_audio.has_video,
            );
        }
        let user_agent = if matches!(client, Client::VisionOs) {
            self.client
                .engine()
                .export(&["Constants", "CLIENTS", "VISIONOS", "USER_AGENT"])
                .await?
                .deserialize::<String>()
                .await?
        } else {
            self.user_agent.clone()
        };
        Ok((
            self.stream(video, cpn, &user_agent).await?,
            match audio {
                Some(format) => Some(self.stream(format, cpn, &user_agent).await?),
                None => None,
            },
        ))
    }

    async fn stream(&self, format: &Format, cpn: &str, user_agent: &str) -> Result<Stream> {
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
            .await
            .context("Load YouTube player for stream deciphering")?;
            session.set_player(&player).await?;
        }
        let mut url = url::Url::parse(&format.decipher(session.player().await?.as_ref()).await?)?;
        url.query_pairs_mut().append_pair("cpn", cpn);
        Ok(Stream {
            identity: format!("{}:{}:{:?}", info.itag, info.mime_type, info.content_length),
            url: url.into(),
            user_agent: user_agent.into(),
        })
    }
}

async fn fetch(
    api: &Api,
    jar: &CookieJar,
    writes: &TaskTracker,
    input: FetchRequest,
) -> Result<FetchResponse> {
    let url = url::Url::parse(&input.url)?;
    let host = url.host_str().unwrap_or("").to_owned();
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
    let snapshot_jar = jar.clone();
    let snapshot = tokio::task::spawn_blocking(move || snapshot_jar.snapshot()).await??;
    let native_playback = input
        .body
        .as_deref()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
        .is_some_and(|body| body["context"]["client"]["clientName"] == "VISIONOS");
    let cookie = if native_playback {
        String::new()
    } else {
        snapshot.header(&url)
    };
    let mut request = api
        .http
        .request(input.method.parse()?, url.clone())
        .timeout(std::time::Duration::from_secs(30));
    for (name, value) in input.headers {
        let name_lower = name.to_ascii_lowercase();
        if matches!(
            name_lower.as_str(),
            "cookie"
                | "authorization"
                | "x-goog-authuser"
                | "x-goog-pageid"
                | "x-origin"
                | "x-youtube-bootstrap-logged-in"
        ) {
            continue;
        }
        if !["host", "content-length"].contains(&name_lower.as_str()) {
            request = request.header(name, value);
        }
    }
    if !cookie.is_empty() {
        request = request.header("cookie", &cookie);
        if host == "www.youtube.com" && url.path().starts_with("/youtubei/") {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            let authorization = cookie_authorization(&cookie, timestamp);
            if !authorization.is_empty() {
                request = request
                    .header("authorization", authorization)
                    .header("x-origin", "https://www.youtube.com")
                    .header("x-goog-authuser", "0");
            }
        }
    }
    if let Some(body) = input.body {
        request = request.body(body);
    }
    let response = api.send(request).await?;
    let updates = response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok().map(str::to_owned))
        .collect::<Vec<_>>();
    let update_jar = jar.clone();
    let cancel = api.cancel.clone();
    // Commit response cookies before reading the body. Once the writer acquires
    // its lock, cancellation or body failure cannot discard an admitted rotation.
    let span = tracing::Span::current();
    writes
        .spawn_blocking(move || {
            let _entered = span.enter();
            update_jar.update(&snapshot, &url, &updates, &cancel)
        })
        .await??;
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

fn duration_seconds(seconds: f64) -> Option<u64> {
    (seconds.is_finite() && seconds > 0.0 && seconds <= 7.0 * 24.0 * 3600.0)
        .then_some(seconds as u64)
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

fn thumbnail_url(thumbnails: Option<Vec<youtubei::models::Thumbnail>>) -> Option<String> {
    thumbnails?
        .into_iter()
        .filter(|thumbnail| {
            url::Url::parse(&thumbnail.url).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
            })
        })
        .max_by(|a, b| {
            let area = |thumbnail: &youtubei::models::Thumbnail| {
                thumbnail.width.unwrap_or(0.0) * thumbnail.height.unwrap_or(0.0)
            };
            area(a).total_cmp(&area(b))
        })
        .map(|thumbnail| thumbnail.url)
}

fn unavailable_reason(status: &PlayabilityStatus) -> String {
    let (screen_reason, detail) = match status.error_screen.as_ref() {
        Some(PlayerErrorScreen::PlayerInterstitial {
            content: Some(content),
        }) => (
            Some(content.title.as_str()),
            Some(content.description.as_str()),
        ),
        Some(PlayerErrorScreen::PlayerErrorMessage(message)) => (
            Some(message.reason.as_str()),
            Some(message.subreason.as_str()),
        ),
        _ => (None, None),
    };
    // Upstream Text.toString() uses N/A for an empty text node.
    let present = |text: &&str| !text.trim().is_empty() && text.trim() != "N/A";
    let mut reason = status
        .reason
        .as_deref()
        .filter(present)
        .or(screen_reason.filter(present))
        .unwrap_or("Video unavailable")
        .trim()
        .to_owned();
    if let Some(detail) = detail.filter(present).map(str::trim)
        && !reason.contains(detail)
    {
        if !reason.ends_with(['.', '!', '?']) {
            reason.push('.');
        }
        reason.push(' ');
        reason.push_str(detail);
    }
    reason
}

fn cookie_authorization(header: &str, timestamp: u64) -> String {
    let value = |name: &str| {
        header.split("; ").find_map(|part| {
            part.split_once('=')
                .filter(|(key, _)| *key == name)
                .map(|(_, value)| value)
        })
    };
    [
        (
            "SAPISIDHASH",
            value("SAPISID").or_else(|| value("__Secure-3PAPISID")),
        ),
        ("SAPISID1PHASH", value("__Secure-1PAPISID")),
        ("SAPISID3PHASH", value("__Secure-3PAPISID")),
    ]
    .into_iter()
    .filter_map(|(scheme, sid)| {
        sid.map(|sid| {
            let digest = hex::encode(Sha1::digest(format!(
                "{timestamp} {sid} https://www.youtube.com"
            )));
            format!("{scheme} {timestamp}_{digest}")
        })
    })
    .collect::<Vec<_>>()
    .join(" ")
}
