use crate::{
    api::{Api, string},
    database::Database,
    downloads::DownloadManager,
    errors::{Category, classify},
    events::Events,
    innertube::{PlaylistSnapshot, YouTube},
    publicapi as p,
    youtube::ImportOutcome,
};
use anyhow::{Context, Result, bail, ensure};
use futures_util::{StreamExt, stream};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::OpenOptions,
    time::Duration,
};

pub const WATCH_INTERVAL: Duration = Duration::from_secs(3600);
const TRANSFER_ATTEMPTS: usize = 5;
const METADATA_REQUEST_BYTES: usize = 4 << 20;

#[derive(Default, Clone, Debug)]
pub struct Report {
    pub added: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub reordered: bool,
    pub stopped: bool,
}

#[derive(Clone)]
pub struct Engine {
    pub downloads: DownloadManager,
    journal: std::sync::Arc<parking_lot::Mutex<Option<Database>>>,
    scan_anchor: tokio::time::Instant,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            downloads: DownloadManager::default(),
            journal: Default::default(),
            scan_anchor: tokio::time::Instant::now(),
        }
    }
}

pub async fn inventory(api: &Api, slug: &str) -> Result<p::SyncInventory> {
    let mut cursor = None;
    let mut cursors = HashSet::new();
    let mut complete: Option<p::SyncInventory> = None;
    loop {
        let result = match api
            .client()
            .get_sync_inventory(p::GetSyncInventoryParams {
                show_slug: slug.into(),
                cursor,
                limit: Some(500),
            })
            .await?
        {
            p::GetSyncInventoryResponse::Status200(value) => value,
            response => return Err(api.response_error(response).await),
        };
        let mut page = result;
        ensure!(
            page.show.slug == slug,
            "Sync inventory returned a different show"
        );
        cursor = page.next_cursor.take();
        match &mut complete {
            Some(result) => {
                ensure!(
                    result.show.id == page.show.id
                        && result.show.youtube.url == page.show.youtube.url,
                    "Show source changed while reading inventory"
                );
                result.episodes.extend(page.episodes);
            }
            None => complete = Some(page),
        }
        match &cursor {
            None => return complete.context("Missing sync inventory"),
            Some(cursor) => ensure!(
                cursors.insert(cursor.clone()),
                "Sync inventory repeated a cursor"
            ),
        }
    }
}

impl Engine {
    pub(crate) fn database(&self, config: &crate::config::Config) -> Result<Database> {
        let mut saved = self.journal.lock();
        if saved.is_none() {
            *saved = Some(Database::open(&config.directory)?);
        }
        Ok(saved.as_ref().context("Missing sync journal")?.clone())
    }

    fn next_scan_at(&self) -> tokio::time::Instant {
        let periods = self.scan_anchor.elapsed().as_secs() / WATCH_INTERVAL.as_secs();
        self.scan_anchor + Duration::from_secs((periods + 1) * WATCH_INTERVAL.as_secs())
    }

    pub async fn next_scan(&self, cancel: &tokio_util::sync::CancellationToken) -> bool {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => false,
            _ = tokio::time::sleep_until(self.next_scan_at()) => true,
        }
    }

    pub async fn once(&self, api: &Api, slug: &str) -> Result<Report> {
        let mut api = api.clone();
        api.cancel = api.cancel.child_token();
        match self.sync(&api, slug, None).await {
            Err(error) if error.is::<crate::api::SyncStopped>() => Ok(Report {
                stopped: true,
                ..Report::default()
            }),
            result => result,
        }
    }

    /// The initial sync consumes the exact listing used for creation admission.
    pub(crate) async fn import_snapshot(
        &self,
        api: &Api,
        slug: &str,
        source: &str,
        snapshot: PlaylistSnapshot,
        youtube: &YouTube,
    ) -> Result<Report> {
        match self
            .sync(api, slug, Some((source, snapshot, youtube)))
            .await
        {
            Err(error) if error.is::<crate::api::SyncStopped>() => Ok(Report {
                stopped: true,
                ..Report::default()
            }),
            result => result,
        }
    }

    async fn sync(
        &self,
        api: &Api,
        slug: &str,
        scanned: Option<(&str, PlaylistSnapshot, &YouTube)>,
    ) -> Result<Report> {
        std::fs::create_dir_all(&api.config.directory)?;
        let lock_key = hex::encode(Sha256::digest(format!("{}\0{slug}", api.config.api_origin)));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(api.config.directory.join(format!("sync-{lock_key}.lock")))?;
        lock.try_lock()
            .context("Another client is already syncing this show on this computer")?;
        let journal = self.database(&api.config)?;
        let before = inventory(api, slug).await?;
        let collection = before
            .show
            .youtube_linkage()
            .context("This podcast has no linked YouTube playlist, channel or video.")?;
        let source = url::Url::parse(collection)?;
        let initialized;
        let youtube = if let Some((_, _, youtube)) = &scanned {
            *youtube
        } else {
            initialized = YouTube::new(api).await?;
            &initialized
        };
        let snapshot = if let Some((scanned_source, snapshot, _)) = scanned {
            ensure!(
                collection == scanned_source,
                "Show source differs from the admitted playlist"
            );
            snapshot
        } else if let Some(playlist) = crate::youtube::collection_playlist_id(&source) {
            youtube.snapshot(api, &playlist).await?
        } else {
            let id = source
                .query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, id)| id.into_owned())
                .context("Show source has no video ID")?;
            youtube.video_snapshot(api, &id).await?
        };
        if before
            .show
            .image_url
            .as_deref()
            .is_none_or(|url| url.trim().is_empty())
            && let Some(url) = &snapshot.artwork_url
        {
            let image = crate::artwork::upload_source(api, &before.show.id, url).await?;
            update_metadata(api, slug, collection, Some(image), Vec::new()).await?;
        }
        let ordered_urls: Vec<String> = snapshot
            .present
            .iter()
            .map(|video| format!("https://www.youtube.com/watch?v={}", video.id))
            .collect();
        journal.snapshot(
            &api.config.api_origin,
            slug,
            collection,
            &snapshot.present,
            snapshot.can_remove,
        )?;
        let remote: HashSet<String> = snapshot
            .present
            .iter()
            .map(|video| format!("https://www.youtube.com/watch?v={}", video.id))
            .collect();
        journal.published(
            &api.config.api_origin,
            slug,
            before
                .episodes
                .iter()
                .map(|episode| episode.source_url.as_str()),
        )?;
        for episode in &before.episodes {
            journal.forget(&api.config.api_origin, slug, &episode.source_url)?;
        }
        let existing: HashMap<&str, &p::SyncEpisode> = before
            .episodes
            .iter()
            .map(|episode| (episode.source_url.as_str(), episode))
            .collect();
        let additions: Vec<_> = snapshot
            .present
            .iter()
            .filter(|video| {
                !existing
                    .contains_key(format!("https://www.youtube.com/watch?v={}", video.id).as_str())
            })
            .cloned()
            .collect();
        // Persist the whole admitted queue before its first external effect.
        for video in &additions {
            journal.operation(
                &api.config.api_origin,
                slug,
                &format!("https://www.youtube.com/watch?v={}", video.id),
                collection,
            )?;
        }
        let transfers = self.downloads.enqueue(
            slug,
            &before.show.title,
            &snapshot.present,
            &existing.keys().copied().collect(),
        );
        for transfer in &transfers {
            journal.outcome(&api.config.api_origin, slug, &transfer.item())?;
        }
        let audio = before.show.source_kind == p::ShowSourceKind::Audio;
        let mut work = stream::iter(additions.iter().zip(transfers).map(|(video, transfer)| {
            let journal = &journal;
            async move {
                let active = match api.wait(async { Ok(transfer.acquire().await) }).await {
                    Ok(active) => active,
                    Err(error) => {
                        transfer.queued();
                        return Err(error);
                    }
                };
                let mut attempt = 1;
                let result = loop {
                    transfer.attempt(attempt);
                    let result = crate::youtube::import_video(
                        api,
                        youtube,
                        crate::youtube::VideoImport {
                            slug,
                            id: &video.id,
                            collection,
                            transfer: &transfer,
                            journal,
                            audio,
                        },
                    )
                    .await;
                    let Err(error) = &result else {
                        break result;
                    };
                    if error.is::<crate::api::SyncStopped>() {
                        api.cancel.cancel();
                    }
                    if api.cancel.is_cancelled()
                        || attempt == TRANSFER_ATTEMPTS
                        || classify(error) != Category::Retryable
                    {
                        break result;
                    }
                    // import_video has joined its writes and retains its durable
                    // operation, ranges, manifest and parts. It is the sole resume
                    // path, including when publication succeeded but its reply was lost.
                    transfer.error(error);
                    transfer.phase(crate::downloads::Phase::Retrying);
                    journal.outcome(&api.config.api_origin, slug, &transfer.item())?;
                    if let Err(error) = api
                        .wait(async {
                            tokio::time::sleep(Duration::from_secs(1 << (attempt - 1))).await;
                            Ok(())
                        })
                        .await
                    {
                        break Err(error);
                    }
                    attempt += 1;
                };
                // Only non-retryable episode errors are skips. Exhausted
                // transient failures remain failed, with their resumable journal.
                let result = match result {
                    Err(error)
                        if !api.cancel.is_cancelled() && classify(&error) == Category::Skip =>
                    {
                        Ok(ImportOutcome::Skipped(crate::redact(&format!("{error:#}"))))
                    }
                    result => result,
                };
                if let Ok(ImportOutcome::Skipped(reason)) = &result {
                    transfer.skipped(reason);
                }
                if api.cancel.is_cancelled() && result.is_err() {
                    active.cancelled();
                } else {
                    let outcome = result
                        .as_ref()
                        .map(|_| ())
                        .map_err(|error| anyhow::anyhow!("{error:#}"));
                    active.finish(&outcome);
                }
                journal.outcome(&api.config.api_origin, slug, &transfer.item())?;
                result
            }
        }))
        .buffer_unordered(crate::downloads::MAX_IN_FLIGHT);
        let mut failures = Vec::new();
        let mut stopped = false;
        let mut report = Report {
            unchanged: existing
                .keys()
                .filter(|url| !snapshot.can_remove || remote.contains(**url))
                .count(),
            ..Report::default()
        };
        while let Some(result) = work.next().await {
            match result {
                Ok(ImportOutcome::Published) => report.added += 1,
                Ok(ImportOutcome::Skipped(_)) => report.skipped += 1,
                Err(error) => {
                    stopped |= error.is::<crate::api::SyncStopped>();
                    failures.push(error);
                }
            }
        }
        if stopped {
            return Err(crate::api::SyncStopped.into());
        }
        if !failures.is_empty() {
            let message = format!(
                "{} transfer(s) failed: {}",
                failures.len(),
                failures
                    .iter()
                    .map(|error| format!("{error:#}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            bail!(message);
        }

        let mut after = inventory(api, slug).await?;
        ensure!(
            after.show.youtube_linkage() == Some(collection),
            "Show linkage changed during sync"
        );
        for episode in &before.episodes {
            if episode.imported && snapshot.can_remove && !remote.contains(&episode.source_url) {
                delete(api, slug, &episode.id, collection).await?;
                journal.forget(&api.config.api_origin, slug, &episode.source_url)?;
                report.removed += 1;
            }
        }
        if report.removed > 0 {
            after = inventory(api, slug).await?;
        }
        ensure!(
            after.show.youtube_linkage() == Some(collection),
            "Show source changed during sync"
        );
        let episodes: HashMap<_, _> = after
            .episodes
            .iter()
            .map(|episode| (episode.source_url.as_str(), episode))
            .collect();
        let titles: Vec<_> = snapshot
            .present
            .iter()
            .filter_map(|video| {
                let title = video.source_title.as_ref()?;
                let source_url = format!("https://www.youtube.com/watch?v={}", video.id);
                let episode = episodes.get(source_url.as_str())?;
                (episode.imported && episode.title != *title).then(|| p::SyncEpisodeTitle {
                    episode_id: episode.id.clone(),
                    source_url,
                    title: title.clone(),
                })
            })
            .collect();
        update_titles(api, slug, collection, titles).await?;
        let ordered: Vec<_> = ordered_urls
            .iter()
            .filter_map(|url| episodes.get(url.as_str()))
            .collect();
        if ordered
            .iter()
            .enumerate()
            .any(|(position, episode)| episode.position != Some(position as i64))
        {
            set_order_for_linkage(
                api,
                slug,
                Some(collection.into()),
                ordered.iter().map(|episode| episode.id.clone()).collect(),
            )
            .await?;
            report.reordered = true;
        }
        // Completion belongs to the engine, after every transfer and metadata
        // effect has been acknowledged. Failed and cancelled passes leave the
        // previous successful completion intact, including across restarts.
        api.wait(async { journal.complete_sync(&api.config.api_origin, slug) })
            .await?;
        Ok(report)
    }

    pub async fn watch(
        &self,
        api: &Api,
        slug: &str,
        mut report: impl FnMut(&Report),
    ) -> Result<()> {
        loop {
            if api.cancel.is_cancelled() {
                return Ok(());
            }
            let result = self.once(api, slug).await;
            if api.cancel.is_cancelled() {
                return Ok(());
            }
            let result = result?;
            report(&result);
            if result.stopped {
                return Ok(());
            }
            if !self.next_scan(&api.cancel).await {
                return Ok(());
            }
        }
    }
}

async fn update_titles(
    api: &Api,
    slug: &str,
    collection: &str,
    titles: Vec<p::SyncEpisodeTitle>,
) -> Result<()> {
    let overhead = serde_json::to_vec(&p::UpdateSyncMetadata {
        youtube_url: collection.into(),
        image_asset_id: None,
        episode_titles: Vec::new(),
    })?
    .len();
    let mut bytes = overhead;
    let mut batch = Vec::new();
    for title in titles {
        let size = serde_json::to_vec(&title)?.len();
        ensure!(
            overhead + size <= METADATA_REQUEST_BYTES,
            "Episode title exceeds the metadata request budget"
        );
        let separator = usize::from(!batch.is_empty());
        if bytes + size + separator > METADATA_REQUEST_BYTES {
            update_metadata(api, slug, collection, None, std::mem::take(&mut batch)).await?;
            bytes = overhead;
        }
        bytes += size + usize::from(!batch.is_empty());
        batch.push(title);
    }
    if !batch.is_empty() {
        update_metadata(api, slug, collection, None, batch).await?;
    }
    Ok(())
}

async fn update_metadata(
    api: &Api,
    slug: &str,
    collection: &str,
    image_asset_id: Option<String>,
    episode_titles: Vec<p::SyncEpisodeTitle>,
) -> Result<()> {
    match api
        .client()
        .update_sync_metadata(p::UpdateSyncMetadataParams {
            show_slug: slug.into(),
            body: p::UpdateSyncMetadata {
                youtube_url: collection.into(),
                image_asset_id,
                episode_titles,
            },
        })
        .await?
    {
        p::UpdateSyncMetadataResponse::Status204(()) => Ok(()),
        response => Err(api.response_error(response).await),
    }
}

pub async fn set_order(api: &Api, slug: &str, episode_ids: Vec<String>) -> Result<p::EpisodeOrder> {
    set_order_for_linkage(api, slug, None, episode_ids).await
}

async fn set_order_for_linkage(
    api: &Api,
    slug: &str,
    youtube_url: Option<String>,
    episode_ids: Vec<String>,
) -> Result<p::EpisodeOrder> {
    Ok(
        match api
            .client()
            .set_episode_order(p::SetEpisodeOrderParams {
                show_slug: slug.into(),
                body: p::SetEpisodeOrder {
                    episode_ids,
                    youtube_url,
                },
            })
            .await?
        {
            p::SetEpisodeOrderResponse::Status200(value) => value,
            response => return Err(api.response_error(response).await),
        },
    )
}

async fn delete(api: &Api, slug: &str, episode: &str, collection: &str) -> Result<()> {
    let result = match api
        .client()
        .create_sync_episode_deletion(p::CreateSyncEpisodeDeletionParams {
            show_slug: slug.into(),
            episode_id: episode.into(),
            youtube_url: collection.into(),
        })
        .await?
    {
        p::CreateSyncEpisodeDeletionResponse::Status202(value) => value,
        response => return Err(api.response_error(response).await),
    };
    let mut events = Events::open(
        api,
        api.client()
            .episode_deletion_events(p::EpisodeDeletionEventsParams {
                episode_deletion_run_id: result.episode_deletion_run_id,
            })
            .await?,
    )
    .await?;
    loop {
        let event = events.next(api).await?;
        if string(&event, "type")? != "terminal" {
            continue;
        }
        ensure!(
            string(&event, "episode_id")? == episode,
            "Deletion completed for a different episode"
        );
        ensure!(
            string(&event, "status")? == "completed",
            "Episode deletion failed"
        );
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn automatic_scan_wait_uses_shared_clock_and_cancels_promptly() {
        let engine = Engine::default();
        let cancel = tokio_util::sync::CancellationToken::new();
        let waiting = engine.next_scan(&cancel);
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        tokio::time::advance(WATCH_INTERVAL).await;
        assert!(waiting.await);
        let next = engine.next_scan(&cancel);
        tokio::pin!(next);
        assert!(futures_util::poll!(&mut next).is_pending());
        cancel.cancel();
        assert!(!next.await);
    }

    #[tokio::test(start_paused = true)]
    async fn all_shows_use_one_hourly_clock_and_skip_missed_ticks() {
        let engine = Engine::default();
        let other_show = engine.clone();
        let first = engine.next_scan_at();
        assert_eq!(
            first - tokio::time::Instant::now(),
            Duration::from_secs(3600)
        );
        tokio::time::advance(Duration::from_secs(25)).await;
        assert_eq!(other_show.next_scan_at(), first);
        tokio::time::advance(Duration::from_secs(7200)).await;
        assert_eq!(engine.next_scan_at(), first + Duration::from_secs(7200));
    }
}
