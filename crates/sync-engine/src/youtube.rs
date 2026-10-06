use crate::{
    api::{Api, PaymentRequired},
    download::download,
    downloads::Stage,
    innertube::{Playback, PlaylistScan, PlaylistSnapshot, Video, YouTube},
    publicapi as p,
};
use anyhow::{Context, Result, ensure};
use futures_util::{StreamExt, TryStreamExt};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tracing::Instrument;
use url::Url;

enum ImportListing {
    Pending(youtubei::Playlist),
    Complete(PlaylistSnapshot),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ImportStage {
    #[default]
    CheckingPlan,
    ReadingPlaylist,
    ScanningPlaylist,
    CreatingPodcast,
}

#[derive(Clone, Debug, Default)]
pub struct ImportPreparation {
    pub stage: ImportStage,
    pub scan: Option<PlaylistScan>,
    pub video_remaining_seconds: Option<i64>,
}

pub enum ImportEvent {
    Preparing(ImportPreparation),
    Created(p::Show),
    Existing(p::Show),
}

pub fn is_source(source: &str) -> bool {
    Url::parse(source).ok().is_some_and(|url| {
        matches!(
            url.host_str(),
            Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be")
        )
    })
}

/// Validate collection URLs before starting network or creating a podcast.
pub fn collection_source(source: &str) -> Result<String> {
    let url = Url::parse(source.trim())?;
    ensure!(
        source.len() <= 2048
            && url.scheme() == "https"
            && matches!(url.host_str(), Some("youtube.com" | "www.youtube.com"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none(),
        "Enter an HTTPS YouTube playlist or channel URL"
    );
    if let Some(path) = channel_path(&url)? {
        ensure!(
            !url.query_pairs()
                .any(|(key, _)| key == "list" || key == "v"),
            "Channel URL cannot contain a playlist or video ID"
        );
        return Ok(format!("https://www.youtube.com{path}"));
    }
    let ids: Vec<_> = url.query_pairs().filter(|(key, _)| key == "list").collect();
    ensure!(
        url.path() == "/playlist"
            && ids.len() == 1
            && (2..=200).contains(&ids[0].1.len())
            && valid_youtube_id(&ids[0].1),
        "Enter a public YouTube playlist or channel URL"
    );
    Ok(format!(
        "https://www.youtube.com/playlist?list={}",
        ids[0].1
    ))
}

fn channel_path(url: &Url) -> Result<Option<String>> {
    let path = url.path().trim_end_matches('/');
    let path = path.strip_suffix("/videos").unwrap_or(path);
    if let Some(id) = path.strip_prefix("/channel/") {
        ensure!(valid_channel_id(id), "invalid YouTube channel ID");
        return Ok(Some(format!("/channel/{id}")));
    }
    if let Some(handle) = path.strip_prefix("/@") {
        let decoded = percent_encoding::percent_decode_str(handle).decode_utf8()?;
        ensure!(
            !decoded.is_empty()
                && decoded.chars().count() <= 100
                && decoded
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-')),
            "invalid YouTube channel handle"
        );
        return Ok(Some(format!("/@{handle}")));
    }
    Ok(None)
}

fn valid_channel_id(id: &str) -> bool {
    id.len() == 24 && id.starts_with("UC") && valid_youtube_id(id)
}

pub(crate) fn collection_playlist_id(url: &Url) -> Option<String> {
    if let Some(id) = url.path().strip_prefix("/channel/") {
        return valid_channel_id(id).then(|| format!("UU{}", &id[2..]));
    }
    url.query_pairs()
        .find(|(key, _)| key == "list")
        .map(|(_, id)| id.into_owned())
}

async fn resolve_channel(api: &Api, collection: String) -> Result<String> {
    if !Url::parse(&collection)?.path().starts_with("/@") {
        return Ok(collection);
    }
    let http = Api::http_client(true)?;
    let response = api
        .send(http.get(&collection).timeout(Duration::from_secs(10)))
        .await
        .context("read YouTube channel page")?;
    ensure!(
        response.status().as_u16() == 200,
        "YouTube channel page returned HTTP {}",
        response.status()
    );
    let body = api
        .wait(crate::api::read_bounded(response, 8 << 20))
        .await
        .context("read YouTube channel HTML")?;
    let html = std::str::from_utf8(&body).context("invalid YouTube channel HTML")?;
    let assignment = regex::Regex::new(r"<script\b[^>]*>\s*(?:var\s+)?ytInitialData\s*=\s*")?;
    let start = assignment
        .find(html)
        .context("YouTube channel page has no metadata")?
        .end();
    let data: serde_json::Value = serde_json::Deserializer::from_str(&html[start..])
        .into_iter()
        .next()
        .context("YouTube channel metadata is missing")?
        .context("invalid YouTube channel metadata")?;
    let id = data["metadata"]["channelMetadataRenderer"]["externalId"]
        .as_str()
        .filter(|id| valid_channel_id(id))
        .context("YouTube channel metadata has no valid channel ID")?;
    Ok(format!("https://www.youtube.com/channel/{id}"))
}

/// Both interfaces use this creation boundary and the same durable first sync.
/// Creation is never an upsert: explicit slug collisions must be resumed with sync.
pub async fn import(
    api: &Api,
    engine: &crate::sync::Engine,
    source: &str,
    team: &str,
    requested_slug: Option<&str>,
    kind: p::ShowSourceKind,
    mut event: impl FnMut(ImportEvent),
) -> Result<(p::Show, crate::sync::Report)> {
    // One import owns both the YouTube session and first sync's cancellation.
    // A server stop drains this operation without cancelling sibling imports.
    let mut import_api = api.clone();
    import_api.cancel = api.cancel.child_token();
    let api = &import_api;
    let source = source.trim();
    let url = Url::parse(source)?;
    ensure!(
        source.len() <= 2048
            && is_source(source)
            && url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none(),
        "invalid YouTube source URL"
    );
    let collection = if url.query_pairs().any(|(key, _)| key == "list")
        || url.path() == "/playlist"
        || channel_path(&url)?.is_some()
    {
        Some(collection_source(source)?)
    } else {
        None
    };
    let requested_slug = requested_slug
        .map(crate::slug)
        .transpose()
        .map_err(anyhow::Error::msg)?;
    let mut preparation = ImportPreparation::default();
    event(ImportEvent::Preparing(preparation.clone()));
    let client = api.client();
    let video_id = if collection.is_none() {
        let id = if url.host_str() == Some("youtu.be") {
            url.path().trim_matches('/').to_owned()
        } else if let Some((_, id)) = url.query_pairs().find(|(key, _)| key == "v") {
            id.into_owned()
        } else {
            url.path()
                .strip_prefix("/shorts/")
                .or_else(|| url.path().strip_prefix("/embed/"))
                .unwrap_or("")
                .into()
        };
        ensure!(
            id.len() == 11 && valid_youtube_id(&id),
            "invalid YouTube video ID"
        );
        Some(id)
    } else {
        None
    };
    let (shows, capacity) = tokio::try_join!(
        client.list_shows(p::ListShowsParams {
            youtube_imports_only: None
        }),
        client.get_import_capacity(p::GetImportCapacityParams {
            team_id: team.to_owned()
        })
    )?;
    let shows = match shows {
        p::ListShowsResponse::Status200(shows) => shows,
        response => return Err(api.response_error(response).await),
    };
    let capacity = match capacity {
        p::GetImportCapacityResponse::Status200(value) => value,
        response => return Err(api.response_error(response).await),
    };
    let collection = match collection {
        Some(collection) => Some(resolve_channel(api, collection).await?),
        None => None,
    };
    let canonical = collection
        .clone()
        .or_else(|| {
            video_id
                .as_ref()
                .map(|id| format!("https://www.youtube.com/watch?v={id}"))
        })
        .context("YouTube source has no canonical identity")?;
    if let Some(show) = shows.iter().find(|show| {
        show.team_id == team && show.youtube.url.as_deref() == Some(canonical.as_str())
    }) {
        event(ImportEvent::Existing(show.clone()));
        return Ok((show.clone(), Default::default()));
    }
    if let Some(slug) = &requested_slug {
        ensure!(
            !shows.iter().any(|show| show.slug == *slug),
            "Podcast {slug:?} already exists. Use sync to resume an imported podcast, or choose a new slug."
        );
    }
    if !capacity.has_active_subscription {
        return Err(PaymentRequired(
            "A paid podcast plan is required for YouTube imports. Choose a plan, then try again."
                .into(),
        )
        .into());
    }
    if kind == p::ShowSourceKind::Video && !capacity.video_allowed {
        return Err(PaymentRequired("A video podcast plan is required to import this playlist as video. Upgrade your plan, then try again.".into()).into());
    }
    preparation.stage = ImportStage::ReadingPlaylist;
    preparation.video_remaining_seconds =
        (kind == p::ShowSourceKind::Video).then_some(capacity.video_remaining_seconds);
    event(ImportEvent::Preparing(preparation.clone()));
    let youtube = YouTube::new(api).await?;
    let (title, canonical, listing) = if let Some(collection) = &collection {
        let id = collection_playlist_id(&Url::parse(collection)?)
            .context("YouTube collection has no playlist ID")?;
        let (title, page) = youtube.playlist_head(api, &id).await?;
        let listing = if kind == p::ShowSourceKind::Video {
            ImportListing::Complete(
                youtube
                    .complete_playlist(api, page, |scan| {
                        preparation.stage = ImportStage::ScanningPlaylist;
                        preparation.scan = Some(scan);
                        event(ImportEvent::Preparing(preparation.clone()));
                    })
                    .await?,
            )
        } else {
            ImportListing::Pending(page)
        };
        (title, collection.clone(), listing)
    } else {
        let id = video_id
            .as_ref()
            .context("YouTube source has no video ID")?;
        let media = match youtube.media(api, id, source).await? {
            Playback::Available(media) => media,
            Playback::Unavailable(reason) => {
                anyhow::bail!("YouTube playback unavailable: {reason}")
            }
        };
        (
            media.title.clone(),
            format!("https://www.youtube.com/watch?v={id}"),
            ImportListing::Complete(PlaylistSnapshot {
                title: media.title.clone(),
                artwork_url: media.artwork_url,
                present: vec![Video {
                    id: id.clone(),
                    source_title: Some(media.title.clone()),
                    title: media.title,
                    duration_seconds: media.duration_seconds,
                }],
                estimated_seconds: media.estimated_seconds,
                can_remove: true,
            }),
        )
    };
    if kind == p::ShowSourceKind::Video
        && let ImportListing::Complete(snapshot) = &listing
        && snapshot.estimated_seconds > capacity.video_remaining_seconds
    {
        return Err(PaymentRequired(format!(
                "This playlist needs about {:.2} hours of video storage. Your team has {:.2} hours available. Upgrade your plan, then try again.",
                snapshot.estimated_seconds as f64 / 3600.0,
                capacity.video_remaining_seconds as f64 / 3600.0,
            )).into());
    }
    preparation.stage = ImportStage::CreatingPodcast;
    event(ImportEvent::Preparing(preparation));
    let mut slug = match &requested_slug {
        Some(slug) => slug.clone(),
        None if collection.is_some() => {
            let mut slug = slug::slugify(&title);
            slug.truncate(63);
            slug.trim_end_matches('-').to_owned()
        }
        None => format!("youtube-{}", &hex::encode(Sha256::digest(source))[..16]),
    };
    let mut title_slug = requested_slug.is_none() && collection.is_some() && !slug.is_empty();
    if slug.is_empty() {
        slug = random_playlist_slug();
    }
    let mut body = p::CreateShow {
        id: format!("shw_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
        team_id: team.to_owned(),
        title,
        slug,
        source_kind: kind,
        language: "en".into(),
        image_asset_id: None,
        youtube_url: Some(canonical.clone()),
    };
    let show = loop {
        let context = format!(
            "Create podcast {:?}. If creation completed but its reply was lost, reload your podcasts and resume with sync.",
            body.slug
        );
        let response = api
            .client()
            .create_show(p::CreateShowParams { body: body.clone() })
            .await
            .with_context(|| context.clone())?;
        match response {
            p::CreateShowResponse::Status201(show) => break show,
            p::CreateShowResponse::Status409(p::PodcastCreationConflict::ShowAlreadyImported {
                show_id,
                team_id,
                ..
            }) => {
                ensure!(
                    team_id == team,
                    "import conflict returned another team's podcast"
                );
                let existing = show_from_import_conflict(api, team, &show_id).await?;
                event(ImportEvent::Existing(existing.clone()));
                return Ok((existing, Default::default()));
            }
            // A conflict confirms no show was created; an ambiguous failure cannot.
            p::CreateShowResponse::Status409(_) if title_slug => {
                title_slug = false;
                body.slug = random_playlist_slug();
            }
            response => return Err(api.response_error(response).await).context(context),
        }
    };
    let slug = &show.slug;
    event(ImportEvent::Created(show.clone()));
    let report = async {
        let snapshot = match listing {
            ImportListing::Pending(page) => youtube.complete_playlist(api, page, |_| {}).await?,
            ImportListing::Complete(snapshot) => snapshot,
        };
        engine
            .import_snapshot(api, slug, &canonical, snapshot, &youtube)
            .await
    }
    .await
    .with_context(|| format!("Podcast {slug:?} was created. Resume it with sync."))?;
    Ok((show, report))
}

async fn show_from_import_conflict(api: &Api, team: &str, show_id: &str) -> Result<p::Show> {
    let shows = match api
        .client()
        .list_shows(p::ListShowsParams {
            youtube_imports_only: None,
        })
        .await?
    {
        p::ListShowsResponse::Status200(shows) => shows,
        response => return Err(api.response_error(response).await),
    };
    shows
        .into_iter()
        .find(|show| show.id == show_id && show.team_id == team)
        .context("Existing imported podcast is no longer available; reload your podcasts")
}

fn random_playlist_slug() -> String {
    format!(
        "youtube-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..16]
    )
}

fn valid_youtube_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

pub(crate) struct VideoImport<'a> {
    pub slug: &'a str,
    pub id: &'a str,
    pub collection: &'a str,
    pub transfer: &'a crate::downloads::Transfer,
    pub journal: &'a crate::database::Database,
    pub audio: bool,
}

pub(crate) enum ImportOutcome {
    Published,
    Skipped(String),
}

#[tracing::instrument(name = "media.transfer", skip_all,
    fields(show_slug = work.slug, video_id = work.id, operation_id = tracing::field::Empty))]
pub(crate) async fn import_video(
    api: &Api,
    youtube: &YouTube,
    work: VideoImport<'_>,
) -> Result<ImportOutcome> {
    let VideoImport {
        slug,
        id,
        collection,
        transfer,
        journal,
        audio,
    } = work;
    let source_url = format!("https://www.youtube.com/watch?v={id}");
    let operation_id = journal.operation(&api.config.api_origin, slug, &source_url, collection)?;
    tracing::Span::current().record("operation_id", operation_id.as_str());
    let directory = journal.directory(&operation_id)?;
    let saved = journal.prepared(&operation_id)?;
    let manifest = match saved {
        Some(manifest) => {
            tracing::info!("reusing prepared media");
            if cfg!(debug_assertions) {
                eprintln!(
                    "FFmpeg preparation reused show_slug={slug} video_id={id} operation_id={operation_id}"
                );
            }
            manifest
        }
        None => {
            for name in ["audio.m4a", "video.mp4", "hls"] {
                let path = directory.join(name);
                if path.is_dir() {
                    std::fs::remove_dir_all(path)?;
                } else if path.exists() {
                    std::fs::remove_file(path)?;
                }
            }
            tracing::info!(stage = "download", "waiting for transfer capacity");
            let downloading = api
                .wait(async { Ok(transfer.acquire_stage(Stage::Download).await) })
                .await?;
            let downloaded: Result<Playback> = async {
                let playback = youtube.media(api, id, collection).await?;
                let Playback::Available(mut media) = playback else {
                    return Ok(playback);
                };
                transfer.title(&media.title);
                transfer.duration(media.duration_seconds);
                loop {
                    let result: Result<()> = async {
                        if audio {
                            download(
                                api,
                                media.audio.as_ref().unwrap_or(&media.video),
                                &directory.join("source-audio"),
                                Some(transfer),
                                Some((journal, operation_id.as_str())),
                            )
                            .await?;
                        } else {
                            download(
                                api,
                                &media.video,
                                &directory.join("source-video"),
                                Some(transfer),
                                Some((journal, operation_id.as_str())),
                            )
                            .await?;
                            if let Some(stream) = &media.audio {
                                download(
                                    api,
                                    stream,
                                    &directory.join("source-audio"),
                                    Some(transfer),
                                    Some((journal, operation_id.as_str())),
                                )
                                .await?;
                            }
                        }
                        Ok(())
                    }
                    .await;
                    match result {
                        Ok(()) => return Ok(Playback::Available(media)),
                        Err(error)
                            if error.is::<crate::download::MediaUnavailable>()
                                && !api.cancel.is_cancelled() =>
                        {
                            // download() drains all admitted writes first. The same
                            // journal validates which ranges survive a client change.
                            match youtube.fallback_media(api, id, media).await? {
                                Some(fallback) => media = fallback,
                                None => return Err(error),
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            .instrument(tracing::info_span!("media.download"))
            .await;
            // Range writes are joined before handing these files to FFmpeg.
            downloading.finish(&downloaded, api.cancel.is_cancelled());
            let media = match downloaded? {
                Playback::Available(media) => media,
                Playback::Unavailable(reason) => return Ok(ImportOutcome::Skipped(reason)),
            };
            tracing::info!(stage = "prepare", "waiting for transfer capacity");
            let preparing = api
                .wait(async { Ok(transfer.acquire_stage(Stage::Prepare).await) })
                .await?;
            let root = directory.clone();
            let separate_audio = media.audio.is_some();
            let cancel = api.cancel.clone();
            let preparation = format!(
                "show_slug={slug} video_id={id} operation_id={operation_id} kind={} title={:?}",
                if audio { "audio" } else { "video" },
                media.title,
            );
            // Join the FFmpeg owner before releasing files, including after cancellation.
            let span = tracing::info_span!("media.prepare");
            let duration = tokio::task::spawn_blocking(move || {
                let _entered = span.enter();
                // The actual blocking worker owns the CPU permit, including if
                // its async caller is dropped. Normal cancellation still joins it.
                let _preparing = preparing;
                // Measure execution, excluding download and blocking-pool wait time.
                let started = Instant::now();
                if cfg!(debug_assertions) {
                    eprintln!("FFmpeg preparation started {preparation}");
                }
                let result = (|| {
                    if audio {
                        let output = root.join("audio.m4a");
                        if output.exists() {
                            std::fs::remove_file(&output)?;
                        }
                        crate::audio::prepare_m4a(&root.join("source-audio"), &output, &cancel)?;
                        crate::audio::duration(&output)
                    } else {
                        crate::media::prepare(&root, separate_audio, &cancel)
                    }
                })();
                tracing::info!(success = result.is_ok(), elapsed_ms = started.elapsed().as_millis() as u64,
                    cancelled = cancel.is_cancelled(), error = ?result.as_ref().err().map(ToString::to_string),
                    "media preparation finished");
                if cfg!(debug_assertions) {
                    eprintln!(
                        "FFmpeg preparation finished {preparation} elapsed_ms={} result={} media_duration_seconds={:?} error={:?}",
                        started.elapsed().as_millis(),
                        if result.is_ok() { "success" } else { "failed" },
                        result.as_ref().ok(),
                        result.as_ref().err().map(ToString::to_string),
                    );
                }
                result
            })
            .await??;
            let manifest = p::CreateEpisodePackage {
                show_slug: slug.into(),
                source_url: source_url.clone(),
                youtube_url: collection.into(),
                operation_id,
                title: media.title,
                description: Some(media.description),
                duration_seconds: duration,
                published_at: media.published_at,
                objects: inventory(&directory, audio)?,
            };
            for object in &manifest.objects {
                std::fs::File::open(directory.join(&object.name))?.sync_all()?;
            }
            journal.save_prepared(&manifest)?;
            manifest
        }
    };
    let objects = &manifest.objects;
    let total = objects.iter().map(|object| object.byte_length as u64).sum();
    transfer.start_upload(total, 0);
    tracing::info!(stage = "upload", "waiting for transfer capacity");
    let uploading = api
        .wait(async { Ok(transfer.acquire_stage(Stage::Upload).await) })
        .await?;
    let result: Result<()> = async {
        let response = api
            .client()
            .create_episode_package(p::CreateEpisodePackageParams {
                body: manifest.clone(),
            })
            .await?;
        let session = match response {
            p::CreateEpisodePackageResponse::Status201(session) => session,
            response => return Err(api.response_error(response).await),
        };
        if session.status == p::EpisodePackageStatus::Completed {
            transfer.start_upload(total, total);
            return Ok(());
        }
        journal.session(&manifest.operation_id, &session.upload_session_id)?;
        ensure!(session.part_size >= 5 << 20, "invalid package part size");
        let completed_object = |ordinal| {
            session
                .uploads
                .iter()
                .any(|upload| upload.object_index == ordinal as i64 && upload.parts.is_empty())
        };
        let mut saved = 0;
        for (ordinal, object) in objects.iter().enumerate() {
            if completed_object(ordinal) {
                saved += object.byte_length as u64;
                continue;
            }
            for number in 1..=1 + (object.byte_length - 1) / session.part_size {
                if journal.has_part(&manifest.operation_id, ordinal, number)? {
                    saved += session
                        .part_size
                        .min(object.byte_length - (number - 1) * session.part_size)
                        as u64;
                }
            }
        }
        transfer.start_upload(total, saved);
        let session = &session;
        let manifest = &manifest;
        let directory = &directory;
        let jobs = objects
            .iter()
            .enumerate()
            .filter(|(ordinal, _)| !completed_object(*ordinal))
            .flat_map(|(ordinal, object)| {
                (1..=1 + (object.byte_length - 1) / session.part_size)
                    .map(move |number| (ordinal, object, number))
            });
        futures_util::stream::iter(jobs)
            .map(|(ordinal, object, number)| async move {
                if journal.has_part(&manifest.operation_id, ordinal, number)? {
                    return Ok::<_, anyhow::Error>(());
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis() as i64;
                // Admission URLs can expire while earlier objects upload. Refresh at
                // the point of use, retaining this operation's original multipart ID.
                let initial = session
                    .uploads
                    .iter()
                    .find(|upload| upload.object_index == ordinal as i64)
                    .and_then(|upload| {
                        upload.parts.iter().find(|part| {
                            part.part_number == number && part.expires_at > now + 30_000
                        })
                    });
                let part = if let Some(initial) = initial {
                    initial.clone()
                } else {
                    let signed = match api
                        .client()
                        .presign_episode_package_parts(p::PresignEpisodePackagePartsParams {
                            upload_session_id: session.upload_session_id.clone(),
                            object_index: ordinal as i64,
                            body: p::PresignEpisodeUploadSessionParts {
                                part_numbers: vec![number],
                            },
                        })
                        .await?
                    {
                        p::PresignEpisodePackagePartsResponse::Status200(value) => value,
                        response => return Err(api.response_error(response).await),
                    };
                    ensure!(
                        signed.parts.len() == 1 && signed.parts[0].part_number == number,
                        "invalid signed package part"
                    );
                    signed.parts.into_iter().next().unwrap()
                };
                let offset = (number - 1) * session.part_size;
                let length = session.part_size.min(object.byte_length - offset);
                crate::episodes::upload_part(
                    api,
                    &directory.join(&object.name),
                    offset as u64,
                    length as u64,
                    "PUT",
                    &part.upload_url,
                    Some(transfer),
                )
                .await?;
                // Each acknowledged part becomes durable before scheduling more work.
                // Dropping siblings on error preserves both saved and unknown writes;
                // an explicit resume safely overwrites an unacknowledged part number.
                journal.save_part(&manifest.operation_id, ordinal, number)?;
                Ok(())
            })
            .buffer_unordered(4)
            .try_collect::<Vec<_>>()
            .await?;
        transfer.phase(crate::downloads::Phase::Publishing);
        let completed = match api
            .client()
            .complete_episode_package(p::CompleteEpisodePackageParams {
                upload_session_id: session.upload_session_id.clone(),
            })
            .await?
        {
            p::CompleteEpisodePackageResponse::Status200(value) => value,
            response => return Err(api.response_error(response).await),
        };
        ensure!(
            completed.status == p::EpisodePackageStatus::Completed,
            "prepared media did not complete"
        );
        Ok(())
    }
    .instrument(tracing::info_span!("media.upload"))
    .await;
    uploading.finish(&result, api.cancel.is_cancelled());
    // Failure and cancellation preserve this operation for the next explicit sync.
    result?;
    journal.forget(&api.config.api_origin, slug, &source_url)?;
    Ok(ImportOutcome::Published)
}

fn inventory(root: &Path, audio: bool) -> Result<Vec<p::PreparedMediaObject>> {
    let mut directories = vec![root.to_owned()];
    let mut names = Vec::new();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                directories.push(entry.path());
                continue;
            }
            ensure!(
                entry.file_type()?.is_file(),
                "media package contains a non-regular object"
            );
            let path = entry.path();
            let name = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            if name == "audio.m4a" || (!audio && (name == "video.mp4" || name.starts_with("hls/")))
            {
                names.push(name);
            }
        }
    }
    names.sort();
    let mut objects = Vec::new();
    for name in names {
        let content_type = if name.ends_with(".m3u8") {
            "application/vnd.apple.mpegurl"
        } else if name.starts_with("hls/audio/") || name == "audio.m4a" {
            "audio/mp4"
        } else {
            "video/mp4"
        };
        let length = std::fs::metadata(root.join(&name))?.len();
        objects.push(p::PreparedMediaObject {
            name,
            content_type: serde_json::from_value(json!(content_type))?,
            byte_length: i64::try_from(length)?,
        });
    }
    Ok(objects)
}
