//! Explicit manual live probe, never invoked by tests or CI. Uses an isolated
//! profile and never publishes episodes or prints cookies or signed URLs.
use anyhow::{Context, Result, ensure};
use listenbox_sync_engine::{
    api::Api,
    config::Config,
    cookies::{CookieJar, SignInRequired},
    innertube::{Playback, Stream, YouTube},
};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let url = url::Url::parse(
        &args
            .next()
            .context("Usage: verify_youtube <playlist URL> [cookies.txt]")?,
    )?;
    let playlist = url
        .query_pairs()
        .find(|(name, _)| name == "list")
        .context("Playlist URL required")?
        .1
        .into_owned();
    let profile = tempfile::tempdir()?;
    if let Some(file) = args.next() {
        CookieJar::new(profile.path()).import_file(std::path::Path::new(&file))?;
    }
    ensure!(args.next().is_none(), "Too many arguments");
    let api = Api::new(
        Config::load_in(None, profile.path().into())?,
        CancellationToken::new(),
    )?;
    let youtube = YouTube::new(&api)
        .await
        .context("Initialize live YouTube session")?;
    let listing = youtube.snapshot(&api, &playlist).await?;
    println!("Listed {} videos", listing.present.len());
    let (mut failed, mut available, mut skipped) = (0, 0, 0);
    for video in listing.present {
        match youtube.media(&api, &video.id).await {
            Ok(Playback::Available(media)) => {
                let mut allowed = true;
                for stream in std::iter::once(media.video).chain(media.audio) {
                    if !check_stream(&api, &stream).await? {
                        allowed = false;
                        break;
                    }
                }
                if allowed {
                    available += 1;
                    println!("{}: media accessible", video.id);
                } else {
                    skipped += 1;
                    println!("{}: skipped (YouTube refused media playback)", video.id);
                }
            }
            Ok(Playback::Unavailable(_)) => {
                skipped += 1;
                println!("{}: skipped (YouTube marked unavailable)", video.id);
            }
            Err(error) => {
                failed += 1;
                println!(
                    "{}: {}",
                    video.id,
                    if error.is::<SignInRequired>() {
                        "sign-in required"
                    } else {
                        "extraction failed"
                    }
                );
            }
        }
    }
    println!("{available} accessible, {skipped} skipped, {failed} failed");
    ensure!(available > 0, "No playable videos reached the media check");
    ensure!(failed == 0, "{failed} videos failed extraction");
    Ok(())
}

async fn check_stream(api: &Api, stream: &Stream) -> Result<bool> {
    // Check both ends, including data beyond a temporary token's initial allowance.
    let mut range = "bytes=0-65535".to_owned();
    for _ in 0..2 {
        let response = api
            .send(
                api.http
                    .get(&stream.url)
                    .header("user-agent", &stream.user_agent)
                    .header("range", &range),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Media request failed"))?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Ok(false);
        }
        ensure!(
            response.status() == reqwest::StatusCode::PARTIAL_CONTENT,
            "Media range rejected: {}",
            response.status()
        );
        let total = response
            .headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit('/').next())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|total| *total > 0)
            .context("Media response has no valid total length")?;
        range = format!("bytes={}-{}", total.saturating_sub(65536), total - 1);
        let bytes = api
            .bytes(response, 128 << 10)
            .await
            .map_err(|_| anyhow::anyhow!("Media request failed"))?;
        ensure!(!bytes.is_empty(), "Empty media response");
    }
    Ok(true)
}
