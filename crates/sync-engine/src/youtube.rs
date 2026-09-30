use crate::{
    api::{Api, PaymentRequired},
    download::download,
    innertube::{Playback, PlaylistSnapshot, Video, YouTube},
    publicapi as p,
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use url::Url;

enum ImportListing {
    Pending(youtubei::Playlist),
    Complete(PlaylistSnapshot),
}

pub fn is_source(source: &str) -> bool {
    Url::parse(source).ok().is_some_and(|url| {
        matches!(
            url.host_str(),
            Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be")
        )
    })
}

/// Validate the playlist boundary before starting network or creating a podcast.
pub fn playlist_source(source: &str) -> Result<String> {
    let url = Url::parse(source.trim())?;
    let ids: Vec<_> = url.query_pairs().filter(|(key, _)| key == "list").collect();
    ensure!(
        source.len() <= 2048
            && url.scheme() == "https"
            && matches!(url.host_str(), Some("youtube.com" | "www.youtube.com"))
            && url.path() == "/playlist"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && ids.len() == 1
            && (2..=200).contains(&ids[0].1.len())
            && valid_youtube_id(&ids[0].1),
        "Enter a public YouTube playlist URL, such as https://www.youtube.com/playlist?list=PL…"
    );
    Ok(format!(
        "https://www.youtube.com/playlist?list={}",
        ids[0].1
    ))
}

/// Both interfaces use this creation boundary and the same durable first sync.
/// Creation is never an upsert: collisions must be resumed with sync explicitly.
pub async fn import(
    api: &Api,
    engine: &crate::sync::Engine,
    source: &str,
    requested_slug: Option<&str>,
    kind: p::ShowSourceKind,
    mut created: impl FnMut(&p::Show),
) -> Result<(p::Show, crate::sync::Report)> {
    let url = Url::parse(source)?;
    ensure!(
        is_source(source)
            && url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none(),
        "invalid YouTube source URL"
    );
    let collection = if url.query_pairs().any(|(key, _)| key == "list") {
        Some(playlist_source(source)?)
    } else {
        None
    };
    let slug = requested_slug.map(str::to_owned).unwrap_or_else(|| {
        format!(
            "youtube-{}",
            &hex::encode(Sha256::digest(collection.as_deref().unwrap_or(source)))[..16]
        )
    });
    let slug = crate::slug(&slug).map_err(anyhow::Error::msg)?;
    let shows: Vec<p::Show> = match api.request(api.client().list_shows()).await? {
        p::ListShowsResponse::Status200(value) => value,
        response => return Err(api.response_error(response).await),
    };
    ensure!(
        !shows.iter().any(|show| show.slug == slug),
        "Podcast {slug:?} already exists. Use sync to resume an imported podcast, or choose a new slug."
    );
    let capacity: p::ImportCapacity = match api.request(api.client().get_import_capacity()).await? {
        p::GetImportCapacityResponse::Status200(value) => value,
        response => return Err(api.response_error(response).await),
    };
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
    let youtube = YouTube::new(api).await?;
    let (title, canonical, listing) = if let Some(collection) = &collection {
        let id = Url::parse(collection)?
            .query_pairs()
            .find(|(key, _)| key == "list")
            .unwrap()
            .1
            .into_owned();
        let (title, page) = youtube.playlist_head(api, &id).await?;
        let listing = if kind == p::ShowSourceKind::Video {
            ImportListing::Complete(youtube.complete_playlist(api, page).await?)
        } else {
            ImportListing::Pending(page)
        };
        (title, collection.clone(), listing)
    } else {
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
        let media = match youtube.media(api, &id).await? {
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
                present: vec![Video {
                    id: id.clone(),
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
    let show = match api.request(api.client().create_show(p::CreateShowParams {
        body: p::CreateShow {
            id: format!("shw_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]),
            title, slug: slug.clone(), source_kind: kind.clone(), language: "en".into(),
            image_asset_id: None, youtube_source_url: Some(canonical.clone()),
        },
    })).await.with_context(|| format!("Create podcast {slug:?}. If creation completed but its reply was lost, reload your podcasts and resume with sync."))? {
        p::CreateShowResponse::Status201(show) => show,
        response => return Err(api.response_error(response).await).with_context(|| format!("Create podcast {slug:?}. If creation completed but its reply was lost, reload your podcasts and resume with sync.")),
    };
    created(&show);
    let report = async {
        let snapshot = match listing {
            ImportListing::Pending(page) => youtube.complete_playlist(api, page).await?,
            ImportListing::Complete(snapshot) => snapshot,
        };
        engine
            .import_snapshot(api, &slug, &canonical, snapshot)
            .await
    }
    .await
    .with_context(|| format!("Podcast {slug:?} was created. Resume it with sync."))?;
    Ok((show, report))
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
    pub transfer: Option<&'a crate::downloads::Transfer>,
    pub journal: &'a crate::database::Database,
    pub audio: bool,
}

pub(crate) enum ImportOutcome {
    Published,
    Skipped(String),
}

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
    let directory = journal.directory(&operation_id)?;
    let saved = journal.prepared(&operation_id)?;
    let manifest = match saved {
        Some(manifest) => {
            for object in &manifest.objects {
                let (length, hash) =
                    crate::episodes::file_hash(&directory.join(&object.name)).await?;
                ensure!(
                    length as i64 == object.byte_length && hash == object.sha256,
                    "Prepared media changed on disk; cannot resume this upload"
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
            let media = match youtube.media(api, id).await? {
                Playback::Available(media) => media,
                Playback::Unavailable(reason) => return Ok(ImportOutcome::Skipped(reason)),
            };
            if let Some(transfer) = transfer {
                transfer.title(&media.title);
                transfer.duration(media.duration_seconds);
            }
            let downloaded: Result<()> = async {
                if audio {
                    download(
                        api,
                        media.audio.as_ref().unwrap_or(&media.video),
                        &directory.join("source-audio"),
                        transfer,
                        Some((journal, operation_id.as_str())),
                    )
                    .await?;
                } else {
                    download(
                        api,
                        &media.video,
                        &directory.join("source-video"),
                        transfer,
                        Some((journal, operation_id.as_str())),
                    )
                    .await?;
                    if let Some(stream) = &media.audio {
                        download(
                            api,
                            stream,
                            &directory.join("source-audio"),
                            transfer,
                            Some((journal, operation_id.as_str())),
                        )
                        .await?;
                    }
                }
                Ok(())
            }
            .await;
            if let Err(error) = downloaded {
                if error.is::<crate::download::MediaUnavailable>() {
                    return Ok(ImportOutcome::Skipped(error.to_string()));
                }
                return Err(error);
            }
            if let Some(transfer) = transfer {
                transfer.phase(crate::downloads::Phase::Preparing);
            }
            let root = directory.clone();
            let separate_audio = media.audio.is_some();
            let cancel = api.cancel.clone();
            // Join the FFmpeg owner before releasing files, including after cancellation.
            let duration = tokio::task::spawn_blocking(move || {
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
            })
            .await??;
            let manifest = p::CreateEpisodePackage {
                show_slug: slug.into(),
                source_url: source_url.clone(),
                operation_id,
                title: media.title,
                description: Some(media.description),
                duration_seconds: duration,
                published_at: media.published_at,
                objects: inventory(&directory, audio).await?,
            };
            for object in &manifest.objects {
                std::fs::File::open(directory.join(&object.name))?.sync_all()?;
            }
            journal.save_prepared(&manifest)?;
            manifest
        }
    };
    let objects = &manifest.objects;
    if let Some(transfer) = transfer {
        transfer.phase(crate::downloads::Phase::Uploading);
    }
    let response = api
        .request(
            api.client()
                .create_episode_package(p::CreateEpisodePackageParams {
                    body: manifest.clone(),
                }),
        )
        .await?;
    let session = match response {
        p::CreateEpisodePackageResponse::Status201(session) => session,
        response => return Err(api.response_error(response).await),
    };
    if session.status == p::EpisodePackageStatus::Completed {
        journal.forget(&api.config.api_origin, slug, &source_url)?;
        return Ok(ImportOutcome::Published);
    }
    journal.session(&manifest.operation_id, &session.upload_session_id)?;
    let result: Result<()> = async {
        ensure!(session.part_size >= 5 << 20, "invalid package part size");
        for (ordinal, object) in objects.iter().enumerate() {
            let initial = session
                .uploads
                .iter()
                .find(|upload| upload.object_index == ordinal as i64);
            let mut offset = 0;
            let mut number = 1;
            while offset < object.byte_length {
                let length = session.part_size.min(object.byte_length - offset);
                if journal.has_part(&manifest.operation_id, ordinal, number)? {
                    offset += length;
                    number += 1;
                    continue;
                }
                let parts = if number == 1
                    && let Some(initial) = initial
                {
                    initial.parts.clone()
                } else {
                    let signed: p::PresignedEpisodeUploadParts = match api
                        .request(api.client().presign_episode_package_parts(
                            p::PresignEpisodePackagePartsParams {
                                upload_session_id: session.upload_session_id.clone(),
                                object_index: ordinal as i64,
                                body: p::PresignEpisodeUploadSessionParts {
                                    part_numbers: vec![number],
                                },
                            },
                        ))
                        .await?
                    {
                        p::PresignEpisodePackagePartsResponse::Status200(value) => value,
                        response => return Err(api.response_error(response).await),
                    };
                    signed.parts
                };
                if parts.is_empty() {
                    break;
                }
                ensure!(
                    parts.len() == 1 && parts[0].part_number == number,
                    "invalid signed package part"
                );
                let length = session.part_size.min(object.byte_length - offset);
                crate::episodes::upload_part(
                    api,
                    &directory.join(&object.name),
                    offset as u64,
                    length as u64,
                    "PUT",
                    &parts[0].upload_url,
                )
                .await?;
                journal.save_part(&manifest.operation_id, ordinal, number)?;
                offset += length;
                number += 1;
            }
        }
        let completed = match api
            .request(
                api.client()
                    .complete_episode_package(p::CompleteEpisodePackageParams {
                        upload_session_id: session.upload_session_id.clone(),
                    }),
            )
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
    .await;
    // Failure and cancellation preserve this operation for the next explicit sync.
    result?;
    journal.forget(&api.config.api_origin, slug, &source_url)?;
    Ok(ImportOutcome::Published)
}

async fn inventory(root: &Path, audio: bool) -> Result<Vec<p::PreparedMediaObject>> {
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
        let (length, sha256) = crate::episodes::file_hash(&root.join(&name)).await?;
        objects.push(p::PreparedMediaObject {
            name,
            content_type: serde_json::from_value(json!(content_type))?,
            byte_length: length as i64,
            sha256,
        });
    }
    Ok(objects)
}
