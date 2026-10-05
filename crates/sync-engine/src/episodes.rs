use crate::{
    api::{Api, number, string},
    config::write_private_json,
    events::Events,
    publicapi as p,
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::Path};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Serialize, Deserialize)]
struct Resume {
    version: u8,
    episode_id: String,
    upload_session_id: String,
    team_id: String,
    show_id: String,
    management_url: String,
    file_path: String,
    file_sha256: String,
    file_byte_length: u64,
    completed_parts: Vec<p::CompletedEpisodeUploadPart>,
    expires_at: i64,
    phase: String,
}

pub async fn file_hash(path: &Path) -> Result<(u64, String)> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("open --file {:?}", path))?;
    let metadata = file.metadata().await?;
    ensure!(
        metadata.is_file() && metadata.len() > 0,
        "--file must be a non-empty regular file"
    );
    let mut buffer = [0; 64 * 1024];
    let mut hash = Sha256::new();
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok((metadata.len(), hex::encode(hash.finalize())))
}

pub async fn create(api: &Api, args: EpisodeCreate) -> Result<()> {
    let path = std::path::absolute(&args.file)?;
    let (length, hash) = file_hash(&path).await?;
    let content_type = match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        _ => bail!(
            "unsupported media upload file; accepted extensions: .mp3, .m4a, .wav, .flac, .mp4, .m4v, .mov"
        ),
    };
    let key = hex::encode(Sha256::digest(format!("{}\0{hash}", args.show)));
    let resume_path = api
        .config
        .directory
        .join("episode-resume")
        .join(format!("{key}.json"));
    let mut record = match std::fs::read(&resume_path) {
        Ok(raw) => {
            let record: Resume =
                serde_json::from_slice(&raw).context("decode episode resume record")?;
            ensure!(
                record.version == 1
                    && record.file_sha256 == hash
                    && record.file_byte_length == length,
                "invalid episode resume record"
            );
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as i64;
            ensure!(
                record.expires_at == 0
                    || record.expires_at > now
                    || ["finalizing", "processing", "completed"].contains(&record.phase.as_str()),
                "resumable upload expired; remove {} to restart explicitly",
                resume_path.display()
            );
            record
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Resume {
            version: 1,
            episode_id: String::new(),
            upload_session_id: String::new(),
            team_id: String::new(),
            show_id: String::new(),
            management_url: String::new(),
            file_path: path.to_string_lossy().into(),
            file_sha256: hash.clone(),
            file_byte_length: length,
            completed_parts: Vec::new(),
            expires_at: 0,
            phase: "prepared".into(),
        },
        Err(error) => return Err(error.into()),
    };
    if !["finalizing", "processing", "completed"].contains(&record.phase.as_str()) {
        let response = api.client().create_episode_upload_session(p::CreateEpisodeUploadSessionParams { body: serde_json::from_value(json!({
            "show_slug": args.show, "title": args.title, "file_name": path.file_name().context("source filename")?.to_string_lossy(),
            "content_type": content_type, "byte_length": length, "source_sha256": hash,
            "description": args.description,
        }))? }).await?;
        let session = match response {
            p::CreateEpisodeUploadSessionResponse::Status200(session)
            | p::CreateEpisodeUploadSessionResponse::Status201(session) => session,
            response => return Err(api.response_error(response).await),
        };
        record.upload_session_id = session.upload_session_id.clone();
        record.team_id = session.team_id.clone();
        record.show_id = session.show_id.clone();
        record.episode_id = session.episode_id.clone().unwrap_or_default();
        record.expires_at = session.expires_at;
        record.phase = match session.phase {
            p::EpisodeUploadSessionPhase::Created => "created",
            p::EpisodeUploadSessionPhase::Uploading => "uploading",
            p::EpisodeUploadSessionPhase::Processing => "processing",
            p::EpisodeUploadSessionPhase::Completed => "completed",
            p::EpisodeUploadSessionPhase::Failed => "failed",
            p::EpisodeUploadSessionPhase::Expired => "expired",
        }
        .into();
        record.completed_parts = session.completed_parts.clone();
        write_private_json(&resume_path, &record)?;
        if record.phase != "finalizing" {
            let part_size = u64::try_from(session.part_size)?;
            ensure!(part_size >= 5 << 20, "invalid upload part size {part_size}");
            let count = length.div_ceil(part_size);
            ensure!(count <= 10000, "episode source requires too many parts");
            let accepted = record
                .completed_parts
                .iter()
                .map(|p| p.part_number)
                .collect::<HashSet<_>>();
            let missing: Vec<_> = (1..=count as i64)
                .filter(|n| !accepted.contains(n))
                .collect();
            saved_progress(&record);
            if !missing.is_empty() {
                let response = api
                    .client()
                    .presign_episode_upload_session_parts(
                        p::PresignEpisodeUploadSessionPartsParams {
                            upload_session_id: record.upload_session_id.clone(),
                            body: p::PresignEpisodeUploadSessionParts {
                                part_numbers: missing.clone(),
                            },
                        },
                    )
                    .await?;
                let parts = match response {
                    p::PresignEpisodeUploadSessionPartsResponse::Status201(signed) => signed.parts,
                    response => return Err(api.response_error(response).await),
                };
                let mut pending = missing.into_iter().collect::<HashSet<_>>();
                for part in parts {
                    let number = part.part_number;
                    ensure!(
                        pending.remove(&number),
                        "unexpected or duplicate signed part {number}"
                    );
                    let offset = (number as u64 - 1) * part_size;
                    let size = part_size.min(length - offset);
                    let etag = upload_part(api, &path, offset, size, "PUT", &part.upload_url, None)
                        .await?;
                    record.completed_parts.push(p::CompletedEpisodeUploadPart {
                        part_number: number,
                        etag,
                        size: size as i64,
                    });
                    record.completed_parts.sort_by_key(|part| part.part_number);
                    record.phase = "uploading".into();
                    write_private_json(&resume_path, &record)?;
                    saved_progress(&record);
                }
                ensure!(pending.is_empty(), "signed response omitted upload parts");
            }
        }
        record.phase = "finalizing".into();
        write_private_json(&resume_path, &record)?;
    }
    let response = api
        .client()
        .complete_episode_upload_session(p::CompleteEpisodeUploadSessionParams {
            upload_session_id: record.upload_session_id.clone(),
            body: serde_json::from_value(json!({"publication": args.publication}))?,
        })
        .await?;
    let episode = match response {
        p::CompleteEpisodeUploadSessionResponse::Status200(episode)
        | p::CompleteEpisodeUploadSessionResponse::Status201(episode) => episode,
        response => return Err(api.response_error(response).await),
    };
    record.episode_id = episode.id;
    record.show_id = episode.show_id;
    ensure!(
        crate::valid_id(&record.episode_id, "ep_"),
        "invalid episode management identifier"
    );
    record.management_url = format!(
        "{}/episodes/{}/edit",
        api.config.show_url(&record.team_id, &record.show_id)?,
        record.episode_id
    );
    record.phase = "processing".into();
    write_private_json(&resume_path, &record)?;
    let mut events = Events::open(
        api,
        api.client()
            .episode_upload_session_events(p::EpisodeUploadSessionEventsParams {
                upload_session_id: record.upload_session_id.clone(),
            })
            .await?,
    )
    .await?;
    loop {
        let event = events.next(api).await?;
        match string(&event, "type")? {
            "episode.processing.progress" => {
                let label = match string(&event, "current_step")? {
                    "queued" | "processing_audio" | "listenbox_processed" => "Listenbox processing",
                    "video_preparation" | "video_prepared" => "Video preparation",
                    "youtube_upload" => "YouTube upload",
                    _ => "Episode processing",
                };
                let percent = number(&event, "percent")?;
                ensure!((0..=100).contains(&percent), "invalid processing progress");
                eprintln!("{label}: {percent}%");
            }
            "episode.processing.safe_handoff" => {
                record.phase = "completed".into();
                write_private_json(&resume_path, &record)?;
                let verb = if args.publication == Publication::Publish {
                    "Published episode"
                } else {
                    "Created draft episode"
                };
                println!(
                    "{verb} {:?}\nOpen in Listenbox: {}",
                    args.title, record.management_url
                );
                if let Some(id) = event["youtube_video_id"].as_str().filter(|s| !s.is_empty()) {
                    println!("YouTube (processing): https://www.youtube.com/watch?v={id}");
                }
                return Ok(());
            }
            "episode.processing.failed" => bail!(
                "episode processing failed ({}): {}",
                string(&event, "error_code")?,
                string(&event, "error_message")?
            ),
            kind => bail!("unknown episode processing event {kind:?}"),
        }
    }
}

fn saved_progress(record: &Resume) {
    let saved: i64 = record.completed_parts.iter().map(|part| part.size).sum();
    eprintln!(
        "Source upload: {}% saved",
        (saved as u64 * 100 / record.file_byte_length).min(100)
    );
}

#[derive(Debug)]
pub(crate) struct UploadStalled;

impl std::fmt::Display for UploadStalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("upload stalled for 60 seconds; resumable upload state preserved")
    }
}
impl std::error::Error for UploadStalled {}

pub async fn upload_part(
    api: &Api,
    path: &Path,
    offset: u64,
    length: u64,
    method: &str,
    url: &str,
    transfer: Option<&crate::downloads::Transfer>,
) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let transfer = transfer.cloned();
    // Measure bytes as reqwest consumes the streaming body, rather than only
    // observing a multipart acknowledgement after an arbitrarily large part.
    let (progress, mut changed) = tokio::sync::watch::channel(tokio::time::Instant::now());
    let stream = tokio_util::io::ReaderStream::with_capacity(file.take(length), 128 << 10)
        .inspect_ok(move |chunk| {
            progress.send_replace(tokio::time::Instant::now());
            if let Some(transfer) = &transfer {
                transfer.uploaded(chunk.len() as u64);
            }
        });
    let request = async {
        let response = api
            .send(
                api.upload_http
                    .request(method.parse()?, url)
                    .header("Content-Length", length)
                    .body(reqwest::Body::wrap_stream(stream)),
            )
            .await?;
        ensure!(
            response.status().is_success(),
            "upload source part returned HTTP {}",
            response.status()
        );
        let etag = response
            .headers()
            .get("ETag")
            .context("upload part returned no ETag")?
            .to_str()?
            .to_owned();
        api.bytes(response, 64 << 10).await?;
        Ok(etag)
    };
    tokio::pin!(request);
    let mut body_open = true;
    let idle_timeout = std::time::Duration::from_secs(60);
    // An active upload may take hours. Only stalled body/acknowledgement progress
    // expires; cancellation still drops the request and preserves the journal.
    loop {
        let deadline = *changed.borrow() + idle_timeout;
        tokio::select! {
            biased;
            result = &mut request => return result,
            _ = tokio::time::sleep_until(deadline) => {
                // Polling the request above may have just consumed another chunk.
                if changed.borrow().elapsed() >= idle_timeout {
                    return Err(UploadStalled.into());
                }
            },
            result = changed.changed(), if body_open => { body_open = result.is_ok(); }
        }
    }
}

#[derive(Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Publication {
    Draft,
    Publish,
}
pub struct EpisodeCreate {
    pub show: String,
    pub title: String,
    pub description: Option<String>,
    pub file: std::path::PathBuf,
    pub publication: Publication,
}
