use parking_lot::RwLock;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

mod adaptive;
use adaptive::AdaptiveLimit;
pub use adaptive::{INITIAL_TRANSFERS, MAX_TRANSFERS};

pub const RANGES_PER_TRANSFER: usize = 4;
pub const RANGE_BYTES: usize = 1024 * 1024;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rate {
    since: Instant,
    last_progress: Instant,
    bytes: u64,
    per_second: u64,
}

impl Rate {
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            last_progress: now,
            bytes: 0,
            per_second: 0,
        }
    }

    fn record(&mut self, bytes: u64, now: Instant) {
        self.bytes += bytes;
        if bytes > 0 {
            self.last_progress = now;
        }
        let elapsed = now.duration_since(self.since);
        if elapsed >= Duration::from_secs(1) {
            self.per_second = (self.bytes as f64 / elapsed.as_secs_f64()) as u64;
            self.bytes = 0;
            self.since = now;
        }
    }

    fn current(&self, now: Instant) -> u64 {
        if now.duration_since(self.last_progress) < Duration::from_secs(3) {
            self.per_second
        } else {
            0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Resolving,
    Downloading,
    Preparing,
    Uploading,
    Retrying,
    Complete,
    Skipped,
    Failed,
}

impl Phase {
    pub fn active(self) -> bool {
        !matches!(
            self,
            Self::Queued | Self::Complete | Self::Skipped | Self::Failed
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Resolving => "Resolving media",
            Self::Downloading => "Downloading",
            Self::Preparing => "Preparing media",
            Self::Uploading => "Uploading",
            Self::Retrying => "Retrying",
            Self::Complete => "Complete",
            Self::Skipped => "Skipped",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangePhase {
    Waiting,
    Active,
    Retrying,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
    pub received: u64,
    pub phase: RangePhase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Download {
    pub id: String,
    pub source_id: String,
    pub source_title: String,
    pub title: String,
    pub duration_seconds: Option<u64>,
    pub reason: Option<String>,
    pub phase: Phase,
    pub attempt: usize,
    pub total: u64,
    pub ranges: Vec<ByteRange>,
    pub error: Option<String>,
    rate: Option<Rate>,
}

impl Download {
    pub fn received(&self) -> u64 {
        self.ranges.iter().map(|range| range.received).sum()
    }

    pub fn bytes_per_second(&self) -> u64 {
        if self.phase != Phase::Downloading {
            return 0;
        }
        self.rate
            .as_ref()
            .map(|rate| rate.current(Instant::now()))
            .unwrap_or(0)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub items: Vec<Download>,
    pub slot_limit: usize,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            slot_limit: INITIAL_TRANSFERS,
        }
    }
}

struct ManagerState {
    snapshot: Snapshot,
    active: usize,
    adaptive: AdaptiveLimit,
    sampled_at: Instant,
    received: u64,
    retried: bool,
    idle_since: Option<Instant>,
}

impl Default for ManagerState {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            active: 0,
            adaptive: AdaptiveLimit::default(),
            sampled_at: Instant::now(),
            received: 0,
            retried: false,
            idle_since: Some(Instant::now()),
        }
    }
}

impl ManagerState {
    fn sample(&mut self, now: Instant) -> bool {
        let elapsed = now.duration_since(self.sampled_at);
        if elapsed < SAMPLE_INTERVAL {
            return false;
        }
        let downloading = self
            .snapshot
            .items
            .iter()
            .filter(|item| item.phase == Phase::Downloading)
            .count();
        let saturated = self.active >= self.adaptive.limit
            && self
                .snapshot
                .items
                .iter()
                .any(|item| item.phase == Phase::Queued);
        self.adaptive.sample(
            (self.received as f64 / elapsed.as_secs_f64()) as u64,
            saturated,
            downloading,
            self.retried,
        );
        self.received = 0;
        self.retried = false;
        self.sampled_at = now;
        let changed = self.snapshot.slot_limit != self.adaptive.limit;
        self.snapshot.slot_limit = self.adaptive.limit;
        changed
    }
}

#[derive(Clone)]
pub struct DownloadManager {
    state: Arc<RwLock<ManagerState>>,
    changed: watch::Sender<()>,
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            changed: watch::channel(()).0,
        }
    }
}

impl DownloadManager {
    /// Clear account-specific display state after the caller has joined all work.
    pub(crate) fn clear(&self) {
        *self.state.write() = ManagerState::default();
        self.changed.send_replace(());
    }
    pub fn changes(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    pub fn remove_source(&self, source_id: &str) {
        self.state
            .write()
            .snapshot
            .items
            .retain(|item| item.source_id != source_id);
        self.changed.send_replace(());
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.read().snapshot.clone()
    }

    fn sample(&self, now: Instant) {
        if self.state.write().sample(now) {
            self.changed.send_replace(());
        }
    }

    /// Replace the previous sync's history for this source, retaining other podcasts.
    pub fn enqueue(
        &self,
        source_id: &str,
        source_title: &str,
        episodes: &[(String, String)],
    ) -> Vec<Transfer> {
        let mut state = self.state.write();
        let state = &mut state.snapshot;
        state.items.retain(|item| item.source_id != source_id);
        episodes
            .iter()
            .map(|(video_id, title)| {
                let id = format!("{source_id}/{video_id}");
                state.items.push(Download {
                    id: id.clone(),
                    source_id: source_id.into(),
                    source_title: source_title.into(),
                    title: title.clone(),
                    duration_seconds: None,
                    reason: None,
                    phase: Phase::Queued,
                    attempt: 0,
                    total: 0,
                    ranges: Vec::new(),
                    error: None,
                    rate: None,
                });
                Transfer {
                    manager: self.clone(),
                    id,
                }
            })
            .collect()
    }
}

#[derive(Clone)]
pub struct Transfer {
    manager: DownloadManager,
    id: String,
}

impl Transfer {
    fn update(&self, update: impl FnOnce(&mut Download)) {
        if let Some(item) = self
            .manager
            .state
            .write()
            .snapshot
            .items
            .iter_mut()
            .find(|item| item.id == self.id)
        {
            update(item);
        }
        self.manager.changed.send_replace(());
    }

    pub async fn acquire(&self) -> ActiveTransfer {
        let mut changed = self.manager.changed.subscribe();
        loop {
            self.manager.sample(Instant::now());
            {
                let mut state = self.manager.state.write();
                if state.active < state.snapshot.slot_limit {
                    // One shared budget includes resolution, conversion and upload.
                    if state
                        .idle_since
                        .take()
                        .is_some_and(|since| since.elapsed() >= SAMPLE_INTERVAL)
                    {
                        state.adaptive = AdaptiveLimit::default();
                        state.snapshot.slot_limit = INITIAL_TRANSFERS;
                        state.sampled_at = Instant::now();
                        state.received = 0;
                        state.retried = false;
                    }
                    state.active += 1;
                    if let Some(item) = state
                        .snapshot
                        .items
                        .iter_mut()
                        .find(|item| item.id == self.id)
                    {
                        item.phase = Phase::Resolving;
                    }
                    return ActiveTransfer {
                        transfer: self.clone(),
                        finished: false,
                    };
                }
            }
            // The timer detects stalls even when no byte callbacks arrive. It runs
            // on the worker, including while the desktop window is hidden.
            tokio::select! {
                result = changed.changed() => result.expect("Manager owns admission channel"),
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }

    pub fn attempt(&self, attempt: usize) {
        self.update(|item| {
            item.phase = Phase::Resolving;
            item.attempt = attempt;
            item.error = None;
            item.total = 0;
            item.ranges.clear();
            item.rate = None;
        });
    }

    pub fn phase(&self, phase: Phase) {
        self.update(|item| item.phase = phase);
    }

    pub fn title(&self, title: &str) {
        self.update(|item| item.title = title.into());
    }

    pub fn duration(&self, duration: Option<u64>) {
        self.update(|item| {
            if duration.is_some() {
                item.duration_seconds = duration;
            }
        });
    }

    pub fn skipped(&self, reason: &str) {
        self.update(|item| {
            item.phase = Phase::Skipped;
            item.reason = Some(reason.into());
        });
    }

    pub fn error(&self, error: &anyhow::Error) {
        self.update(|item| item.error = Some(if error.is::<crate::cookies::SignInRequired>() {
            "YouTube needs a signed-in session. Open Settings → YouTube to add fresh cookies, then sync again.".into()
        } else { crate::redact(&format!("{error:#}")) }));
    }

    pub fn start_download(&self, total: u64, chunk_bytes: usize) {
        self.update(|item| {
            item.phase = Phase::Downloading;
            item.total = total;
            item.rate = Some(Rate::new(Instant::now()));
            item.ranges = (0..total)
                .step_by(chunk_bytes)
                .map(|start| ByteRange {
                    start,
                    end: (start + chunk_bytes as u64).min(total) - 1,
                    received: 0,
                    phase: RangePhase::Waiting,
                })
                .collect();
        });
    }

    pub fn range(&self, start: u64, received: u64, phase: RangePhase) {
        let now = Instant::now();
        let mut state = self.manager.state.write();
        let mut bytes = 0;
        let mut retried = false;
        if let Some(item) = state
            .snapshot
            .items
            .iter_mut()
            .find(|item| item.id == self.id)
            && let Ok(index) = item
                .ranges
                .binary_search_by_key(&start, |range| range.start)
        {
            let range = &mut item.ranges[index];
            let received = received.min(range.end - range.start + 1);
            bytes = received.saturating_sub(range.received);
            retried = phase == RangePhase::Retrying && range.phase != RangePhase::Retrying;
            range.received = received;
            range.phase = phase;
            if let Some(rate) = &mut item.rate {
                rate.record(bytes, now);
            }
        }
        state.received += bytes;
        state.retried |= retried;
        state.sample(now);
        self.manager.changed.send_replace(());
    }
}

pub struct ActiveTransfer {
    transfer: Transfer,
    finished: bool,
}

impl ActiveTransfer {
    pub fn finish(mut self, result: &anyhow::Result<()>) {
        if let Err(error) = result {
            self.transfer.error(error);
        }
        self.transfer.update(|item| {
            if result.is_err() {
                item.phase = Phase::Failed;
            } else if item.phase != Phase::Skipped {
                item.phase = Phase::Complete;
            }
        });
        self.finished = true;
    }
}

impl Drop for ActiveTransfer {
    fn drop(&mut self) {
        if !self.finished {
            self.transfer.update(|item| {
                item.phase = Phase::Failed;
                item.error = Some("Transfer interrupted; refresh the subscription to retry".into());
            });
        }
        let mut state = self.transfer.manager.state.write();
        state.active -= 1;
        if state.active == 0 {
            state.idle_since = Some(Instant::now());
        }
        drop(state);
        self.transfer.manager.changed.send_replace(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::poll;

    fn enqueue(manager: &DownloadManager, source: &str, count: usize) -> Vec<Transfer> {
        manager.enqueue(
            source,
            source,
            &(0..count)
                .map(|index| (index.to_string(), format!("Episode {index}")))
                .collect::<Vec<_>>(),
        )
    }

    #[tokio::test]
    async fn podcasts_share_one_budget_and_release_admits_waiting_work() {
        let manager = DownloadManager::default();
        let first = enqueue(&manager, "first", INITIAL_TRANSFERS);
        let second = enqueue(&manager, "second", 2);
        let mut active = Vec::new();
        for transfer in &first {
            active.push(transfer.acquire().await);
        }
        let mut waiting = Box::pin(second[0].acquire());
        assert!(poll!(&mut waiting).is_pending());
        assert_eq!(
            manager
                .snapshot()
                .items
                .iter()
                .filter(|item| item.phase.active())
                .count(),
            INITIAL_TRANSFERS
        );
        assert_eq!(
            manager.snapshot().items.last().unwrap().phase,
            Phase::Queued
        );
        active.pop().unwrap().finish(&Ok(()));
        let admitted = waiting.await;
        assert_eq!(
            manager
                .snapshot()
                .items
                .iter()
                .filter(|item| item.phase.active())
                .count(),
            INITIAL_TRANSFERS
        );
        // Cancellation releases its slot and reports an actionable failure.
        drop(admitted);
        let next = second[1].acquire().await;
        assert_eq!(
            manager.snapshot().items[INITIAL_TRANSFERS].phase,
            Phase::Failed
        );
        next.finish(&Ok(()));
        for item in active {
            item.finish(&Ok(()));
        }
        assert_eq!(manager.state.read().active, 0);
    }

    #[tokio::test]
    async fn retry_resets_ranges_and_failure_does_not_block_other_podcasts() {
        let manager = DownloadManager::default();
        let transfers = enqueue(&manager, "first", 1);
        let transfer = &transfers[0];
        let active = transfer.acquire().await;
        transfer.attempt(1);
        transfer.start_download(10, 4);
        transfer.range(4, 4, RangePhase::Complete);
        transfer.range(0, 2, RangePhase::Active);
        assert_eq!(manager.snapshot().items[0].received(), 6);
        transfer.range(0, 0, RangePhase::Retrying);
        assert_eq!(manager.snapshot().items[0].received(), 4);
        transfer.error(&anyhow::anyhow!(
            "Failed https://secret.example/audio?token=private"
        ));
        assert!(
            !manager.snapshot().items[0]
                .error
                .as_ref()
                .unwrap()
                .contains("token")
        );
        transfer.attempt(2);
        let item = &manager.snapshot().items[0];
        assert!(item.ranges.is_empty());
        assert!(item.error.is_none());
        assert_eq!(item.total, 0);
        active.finish(&Err(anyhow::anyhow!("Upload failed")));
        let second = enqueue(&manager, "second", 1);
        second[0].acquire().await.finish(&Ok(()));
        let snapshot = manager.snapshot();
        assert_eq!(snapshot.items[0].phase, Phase::Failed);
        assert_eq!(snapshot.items[1].phase, Phase::Complete);
    }

    #[tokio::test]
    async fn adaptive_growth_wakes_waiters_and_shrinking_drains_without_cancellation() {
        let manager = DownloadManager::default();
        let first = enqueue(&manager, "first", 2);
        let other = enqueue(&manager, "other", 3);
        let mut active = Vec::new();
        for transfer in &first {
            active.push(transfer.acquire().await);
            transfer.start_download(100_000_000, 10_000_000);
        }
        let mut waiting = Box::pin(other[0].acquire());
        assert!(poll!(&mut waiting).is_pending());
        // Advance the sampling window without a wall-clock sleep, then exercise
        // the real byte callback and admission notification.
        manager.state.write().sampled_at = Instant::now() - SAMPLE_INTERVAL;
        first[0].range(0, 5_000_000, RangePhase::Active);
        assert_eq!(manager.snapshot().slot_limit, 3);
        active.push(waiting.await);
        other[0].start_download(100_000_000, 10_000_000);

        let mut next = Box::pin(other[1].acquire());
        assert!(poll!(&mut next).is_pending());
        manager.state.write().sampled_at = Instant::now() - SAMPLE_INTERVAL;
        first[0].range(0, 10_000_000, RangePhase::Complete);
        assert_eq!(manager.snapshot().slot_limit, 2);
        assert_eq!(manager.state.read().active, 3);
        assert!(poll!(&mut next).is_pending());
        active.pop().unwrap().finish(&Ok(()));
        assert!(poll!(&mut next).is_pending());
        active.pop().unwrap().finish(&Ok(()));
        let admitted = next.await;
        assert_eq!(manager.state.read().active, 2);
        drop(admitted);
        other[2].acquire().await.finish(&Ok(()));
        active.pop().unwrap().finish(&Ok(()));
        assert_eq!(manager.state.read().active, 0);
    }

    #[tokio::test]
    async fn uploads_do_not_probe_and_retry_feedback_closes_slots() {
        let manager = DownloadManager::default();
        let transfers = enqueue(&manager, "first", 3);
        let first = transfers[0].acquire().await;
        let second = transfers[1].acquire().await;
        transfers[0].start_download(100_000_000, 10_000_000);
        transfers[1].phase(Phase::Uploading);
        let mut waiting = Box::pin(transfers[2].acquire());
        assert!(poll!(&mut waiting).is_pending());
        transfers[0].phase(Phase::Uploading);
        manager.state.write().sampled_at = Instant::now() - SAMPLE_INTERVAL;
        manager.sample(Instant::now());
        assert_eq!(manager.snapshot().slot_limit, INITIAL_TRANSFERS);
        transfers[0].phase(Phase::Downloading);
        manager.state.write().sampled_at = Instant::now() - SAMPLE_INTERVAL;
        transfers[0].range(0, 0, RangePhase::Retrying);
        assert_eq!(manager.snapshot().slot_limit, 1);
        first.finish(&Ok(()));
        assert!(poll!(&mut waiting).is_pending());
        second.finish(&Ok(()));
        let admitted = waiting.await;
        assert_eq!(manager.snapshot().slot_limit, 1);
        admitted.finish(&Ok(()));
    }

    #[test]
    fn telemetry_counts_received_deltas_and_retries_without_double_counting_completion() {
        let manager = DownloadManager::default();
        let transfers = enqueue(&manager, "first", 1);
        let transfer = &transfers[0];
        transfer.start_download(10, 10);
        transfer.range(0, 5, RangePhase::Active);
        transfer.range(0, 5, RangePhase::Active);
        assert_eq!(manager.state.read().received, 5);
        transfer.range(0, 0, RangePhase::Retrying);
        assert!(manager.state.read().retried);
        assert_eq!(manager.state.read().received, 5);
        transfer.range(0, 10, RangePhase::Active);
        transfer.range(0, 10, RangePhase::Complete);
        assert_eq!(manager.state.read().received, 15);
        assert_eq!(manager.snapshot().items[0].received(), 10);
    }

    #[test]
    fn displayed_speed_uses_recent_traffic_and_expires_when_stalled() {
        let now = Instant::now();
        let mut rate = Rate::new(now);
        rate.record(10_000, now + Duration::from_secs(1));
        assert_eq!(rate.current(now + Duration::from_secs(1)), 10_000);
        rate.record(1_000, now + Duration::from_secs(2));
        assert_eq!(rate.current(now + Duration::from_secs(2)), 1_000);
        assert_eq!(rate.current(now + Duration::from_secs(5)), 0);
    }
}
