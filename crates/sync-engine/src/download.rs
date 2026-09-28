use crate::{
    api::Api,
    database::Database,
    downloads::{RANGE_BYTES, RANGES_PER_TRANSFER, RangePhase, Transfer},
    innertube::Stream,
};
use anyhow::{Context, Result, ensure};
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use std::{io::SeekFrom, path::Path};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

#[derive(Debug)]
pub(crate) struct MediaUnavailable;

impl std::fmt::Display for MediaUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("YouTube did not allow playback of this video. It will be checked again at the next sync.")
    }
}
impl std::error::Error for MediaUnavailable {}

pub(crate) async fn download(
    api: &Api,
    source: &Stream,
    path: &Path,
    transfer: Option<&Transfer>,
    journal: Option<(&Database, &str)>,
) -> Result<()> {
    let response = api
        .send(
            api.http
                .get(&source.url)
                .header("Range", "bytes=0-0")
                .header("Origin", "https://youtube.com")
                .header("User-Agent", &source.user_agent),
        )
        .await?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err(MediaUnavailable.into());
    }
    ensure!(
        response.status() == reqwest::StatusCode::PARTIAL_CONTENT,
        "Resolve download length: HTTP {}",
        response.status()
    );
    let total = response
        .headers()
        .get("content-range")
        .context("Media response missing Content-Range")?
        .to_str()?
        .strip_prefix("bytes 0-0/")
        .context("Invalid initial media range")?
        .parse::<u64>()?;
    ensure!(
        total > 0 && total <= 5_000_000_000_000,
        "Invalid media length"
    );
    let validator = response
        .headers()
        .get("etag")
        .filter(|v| !v.as_bytes().starts_with(b"W/"))
        .or_else(|| response.headers().get("last-modified"))
        .cloned();
    // Signed URLs expire. Identity uses rendition metadata and a content revision,
    // never the URL's credentials. Without a revision, ranges cannot be reused safely.
    let revision = validator
        .as_ref()
        .map(|v| v.to_str().map(str::to_owned))
        .transpose()?
        .or_else(|| {
            url::Url::parse(&source.url)
                .ok()?
                .query_pairs()
                .find(|(k, _)| k == "lmt")
                .map(|(_, v)| v.into_owned())
        });
    let identity = format!(
        "{}:{total}:{}",
        source.identity,
        revision.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    );
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid download file name")?;
    if let Some((db, operation)) = journal {
        db.download(operation, name, &identity)?;
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .await?;
    file.set_len(total).await?;
    drop(file);
    if let Some(transfer) = transfer {
        transfer.start_download(total, RANGE_BYTES);
    }
    let mut work = stream::iter((0..total).step_by(RANGE_BYTES).map(|start| {
        let validator = validator.as_ref();
        async move {
            let end = (start + RANGE_BYTES as u64).min(total) - 1;
            let length = (end - start + 1) as usize;
            let mut file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .await?;
            if let Some((db, operation)) = journal
                && let Some(hash) = db.range_hash(operation, name, start)?
            {
                let mut saved = vec![0; length];
                file.seek(SeekFrom::Start(start)).await?;
                if file.read_exact(&mut saved).await.is_ok()
                    && hex::encode(Sha256::digest(&saved)) == hash
                {
                    if let Some(transfer) = transfer {
                        transfer.range(start, length as u64, RangePhase::Complete);
                    }
                    return Ok::<_, anyhow::Error>(());
                }
            }
            let mut request = api
                .http
                .get(&source.url)
                .header("Origin", "https://youtube.com")
                .header("User-Agent", &source.user_agent)
                .header("Range", format!("bytes={start}-{end}"));
            if let Some(validator) = validator {
                request = request.header("If-Range", validator);
            }
            let mut response = api.send(request).await?;
            if response.status() == reqwest::StatusCode::FORBIDDEN {
                return Err(MediaUnavailable.into());
            }
            ensure!(
                response.status() == reqwest::StatusCode::PARTIAL_CONTENT,
                "Download range returned HTTP {}; source may have changed",
                response.status()
            );
            ensure!(
                response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    == Some(format!("bytes {start}-{end}/{total}").as_str()),
                "Media range differs from the requested bytes"
            );
            let mut bytes = Vec::with_capacity(length);
            while let Some(chunk) = api.wait(async { Ok(response.chunk().await?) }).await? {
                ensure!(
                    bytes.len() + chunk.len() <= length,
                    "Media range exceeded its declared length"
                );
                bytes.extend_from_slice(&chunk);
                if let Some(transfer) = transfer {
                    transfer.range(start, bytes.len() as u64, RangePhase::Active);
                }
            }
            ensure!(bytes.len() == length, "Media range was truncated");
            file.seek(SeekFrom::Start(start)).await?;
            file.write_all(&bytes).await?;
            file.sync_data().await?;
            if let Some((db, operation)) = journal {
                db.save_range(operation, name, start, &hex::encode(Sha256::digest(&bytes)))?;
            }
            if let Some(transfer) = transfer {
                transfer.range(start, length as u64, RangePhase::Complete);
            }
            Ok(())
        }
    }))
    .buffer_unordered(RANGES_PER_TRANSFER);
    // Join all admitted writes before the caller can prepare, retry or release files.
    let mut failure = None;
    while let Some(result) = work.next().await {
        if let Err(error) = result {
            failure.get_or_insert(error);
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}
