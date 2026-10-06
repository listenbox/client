use parking_lot::RwLock;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

mod adaptive;
pub use adaptive::{INITIAL_DOWNLOADS, INITIAL_UPLOADS, MAX_DOWNLOADS, MAX_UPLOADS};
use adaptive::{NetworkLimits, Observation};

const MAX_PREPARATIONS: usize = 4;
pub const MAX_IN_FLIGHT: usize = MAX_DOWNLOADS + MAX_UPLOADS + MAX_PREPARATIONS;

pub const RANGES_PER_TRANSFER: usize = 4;
pub const RANGE_BYTES: usize = 1024 * 1024;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_RESET_AFTER: Duration = Duration::from_secs(5);

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
    WaitingToPrepare,
    Preparing,
    WaitingToUpload,
    Uploading,
    Publishing,
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
            Self::WaitingToPrepare => "Waiting to prepare",
            Self::Preparing => "Preparing media",
            Self::WaitingToUpload => "Waiting to upload",
            Self::Uploading => "Uploading",
            Self::Publishing => "Publishing",
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
    pub source_url: String,
    pub position: Option<i64>,
    pub source_title: String,
    pub title: String,
    pub duration_seconds: Option<u64>,
    pub reason: Option<String>,
    pub phase: Phase,
    pub attempt: usize,
    pub total: u64,
    pub upload_total: u64,
    pub uploaded: u64,
    pub ranges: Vec<ByteRange>,
    pub error: Option<String>,
    rate: Option<Rate>,
}

impl Download {
    pub(crate) fn queued(source_id: &str, source_title: &str, video_id: &str, title: &str) -> Self {
        Self {
            id: format!("{source_id}/{video_id}"),
            source_id: source_id.into(),
            source_title: source_title.into(),
            source_url: format!("https://www.youtube.com/watch?v={video_id}"),
            position: None,
            title: title.into(),
            duration_seconds: None,
            reason: None,
            phase: Phase::Queued,
            attempt: 0,
            total: 0,
            upload_total: 0,
            uploaded: 0,
            ranges: Vec::new(),
            error: None,
            rate: None,
        }
    }

    pub fn received(&self) -> u64 {
        self.ranges.iter().map(|range| range.received).sum()
    }

    pub fn bytes_per_second(&self) -> u64 {
        if !matches!(self.phase, Phase::Downloading | Phase::Uploading) {
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
    pub download_slots: usize,
    pub upload_slots: usize,
    pub preparation_slots: usize,
}

/// Whole-inventory progress, including episodes published before this scan.
/// A terminal failure is handled work, but never an imported episode.
#[derive(Default)]
pub struct Progress {
    pub total: usize,
    pub imported: usize,
    pub not_imported: usize,
    pub queued: usize,
    pub resolving: usize,
    pub downloading: usize,
    pub preparing: usize,
    pub uploading: usize,
    pub publishing: usize,
    pub retrying: usize,
    pub download_rate: u64,
    pub upload_rate: u64,
}

impl Snapshot {
    pub fn progress(&self) -> Progress {
        let mut progress = Progress::default();
        for item in &self.items {
            progress.total += 1;
            match item.phase {
                Phase::Complete => progress.imported += 1,
                Phase::Failed | Phase::Skipped => progress.not_imported += 1,
                Phase::Queued | Phase::WaitingToPrepare | Phase::WaitingToUpload => {
                    progress.queued += 1
                }
                Phase::Resolving => progress.resolving += 1,
                Phase::Downloading => {
                    progress.downloading += 1;
                    progress.download_rate += item.bytes_per_second();
                }
                Phase::Preparing => progress.preparing += 1,
                Phase::Uploading => {
                    progress.uploading += 1;
                    progress.upload_rate += item.bytes_per_second();
                }
                Phase::Publishing => progress.publishing += 1,
                Phase::Retrying => progress.retrying += 1,
            }
        }
        progress
    }
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            download_slots: INITIAL_DOWNLOADS,
            upload_slots: INITIAL_UPLOADS,
            preparation_slots: std::thread::available_parallelism()
                .map_or(1, usize::from)
                .saturating_sub(1)
                .clamp(1, MAX_PREPARATIONS),
        }
    }
}

struct ManagerState {
    snapshot: Snapshot,
    positions: HashMap<String, usize>,
    active: usize,
    stage_active: [usize; 3],
    stage_waiting: [usize; 3],
    adaptive: NetworkLimits,
    sampled_at: tokio::time::Instant,
    bytes: [u64; 2],
    failed: [bool; 2],
    population_changed: [bool; 2],
    idle_since: Option<tokio::time::Instant>,
}

impl Default for ManagerState {
    fn default() -> Self {
        Self {
            snapshot: Snapshot::default(),
            positions: HashMap::new(),
            active: 0,
            stage_active: [0; 3],
            stage_waiting: [0; 3],
            adaptive: NetworkLimits::default(),
            sampled_at: tokio::time::Instant::now(),
            bytes: [0; 2],
            failed: [false; 2],
            population_changed: [false; 2],
            idle_since: Some(tokio::time::Instant::now()),
        }
    }
}

impl ManagerState {
    // A source can be replaced while another source is transferring. Rebuild
    // under the same lock as inventory edits so every live handle still finds
    // its item without scanning thousands of episodes on each network chunk.
    fn reindex(&mut self) {
        self.positions = self
            .snapshot
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| (item.id.clone(), index))
            .collect();
    }

    fn item(&self, id: &str) -> Option<&Download> {
        self.positions
            .get(id)
            .and_then(|&index| self.snapshot.items.get(index))
    }

    fn item_mut(&mut self, id: &str) -> Option<&mut Download> {
        let index = *self.positions.get(id)?;
        self.snapshot.items.get_mut(index)
    }

    fn sample(&mut self, now: tokio::time::Instant) -> bool {
        let elapsed = now.duration_since(self.sampled_at);
        if elapsed < SAMPLE_INTERVAL {
            return false;
        }
        let limits = self.adaptive.limits();
        self.adaptive
            .sample(std::array::from_fn(|index| Observation {
                rate: self.bytes[index] as f64 / elapsed.as_secs_f64(),
                active: self.stage_active[index],
                saturated: self.stage_active[index] >= limits[index]
                    && self.stage_waiting[index] > 0,
                stable: !self.population_changed[index],
                failed: self.failed[index],
            }));
        self.bytes = [0; 2];
        self.failed = [false; 2];
        self.population_changed = [false; 2];
        self.sampled_at = now;
        let next = self.adaptive.limits();
        let changed = limits != next;
        [self.snapshot.download_slots, self.snapshot.upload_slots] = next;
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
        let mut state = self.state.write();
        state
            .snapshot
            .items
            .retain(|item| item.source_id != source_id);
        state.reindex();
        drop(state);
        self.changed.send_replace(());
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.read().snapshot.clone()
    }

    pub fn progress(&self) -> Progress {
        self.state.read().snapshot.progress()
    }

    pub fn not_imported(&self) -> Vec<Download> {
        self.state
            .read()
            .snapshot
            .items
            .iter()
            .filter(|item| matches!(item.phase, Phase::Skipped | Phase::Failed))
            .cloned()
            .collect()
    }

    fn sample(&self, now: tokio::time::Instant) {
        if self.state.write().sample(now) {
            self.changed.send_replace(());
        }
    }

    /// Publish the complete source inventory, retaining other podcasts. Episodes
    /// already present on the server are complete; only missing episodes transfer.
    pub(crate) fn enqueue(
        &self,
        source_id: &str,
        source_title: &str,
        episodes: &[crate::innertube::Video],
        published: &HashSet<&str>,
    ) -> Vec<Transfer> {
        let transfers = {
            let mut state = self.state.write();
            state
                .snapshot
                .items
                .retain(|item| item.source_id != source_id);
            let mut transfers = Vec::new();
            for (position, video) in episodes.iter().enumerate() {
                let mut item = Download::queued(source_id, source_title, &video.id, &video.title);
                item.position = Some(position as i64);
                item.duration_seconds = video.duration_seconds;
                if published.contains(item.source_url.as_str()) {
                    item.phase = Phase::Complete;
                } else {
                    transfers.push(Transfer {
                        manager: self.clone(),
                        id: item.id.clone(),
                    });
                }
                state.snapshot.items.push(item);
            }
            state.reindex();
            transfers
        };
        self.changed.send_replace(());
        transfers
    }
}

#[derive(Clone)]
pub struct Transfer {
    manager: DownloadManager,
    id: String,
}

impl Transfer {
    pub(crate) fn item(&self) -> Download {
        self.manager
            .state
            .read()
            .item(&self.id)
            .expect("Transfer belongs to its manager")
            .clone()
    }

    pub(crate) fn queued(&self) {
        self.update(|item| {
            item.phase = Phase::Queued;
            item.error = None;
            item.reason = None;
        });
    }

    fn update(&self, update: impl FnOnce(&mut Download)) {
        if let Some(item) = self.manager.state.write().item_mut(&self.id) {
            update(item);
        }
        self.manager.changed.send_replace(());
    }

    pub async fn acquire(&self) -> ActiveTransfer {
        let mut changed = self.manager.changed.subscribe();
        loop {
            self.manager.sample(tokio::time::Instant::now());
            {
                let mut state = self.manager.state.write();
                if state.active < MAX_IN_FLIGHT {
                    // Bound downloaded/prepared work if a later stage is slower.
                    // Resource permits are acquired separately, never nested.
                    if state
                        .idle_since
                        .take()
                        .is_some_and(|since| since.elapsed() >= IDLE_RESET_AFTER)
                    {
                        state.adaptive = NetworkLimits::default();
                        state.snapshot.download_slots = INITIAL_DOWNLOADS;
                        state.snapshot.upload_slots = INITIAL_UPLOADS;
                        state.sampled_at = tokio::time::Instant::now();
                        state.bytes = [0; 2];
                        state.failed = [false; 2];
                        state.population_changed = [false; 2];
                    }
                    state.active += 1;
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

    pub(crate) async fn acquire_stage(&self, stage: Stage) -> StageTransfer {
        let mut changed = self.manager.changed.subscribe();
        let index = stage as usize;
        self.manager.state.write().stage_waiting[index] += 1;
        let mut waiting = WaitingStage {
            manager: self.manager.clone(),
            stage,
            waiting: true,
        };
        self.phase(stage.waiting_phase());
        loop {
            self.manager.sample(tokio::time::Instant::now());
            {
                let mut state = self.manager.state.write();
                let limit = match stage {
                    Stage::Download => state.snapshot.download_slots,
                    Stage::Upload => state.snapshot.upload_slots,
                    Stage::Prepare => state.snapshot.preparation_slots,
                };
                if state.stage_active[index] < limit {
                    state.stage_active[index] += 1;
                    state.stage_waiting[index] -= 1;
                    waiting.waiting = false;
                    if index < 2 {
                        state.population_changed[index] = true;
                    }
                    drop(state);
                    self.phase(stage.active_phase());
                    return StageTransfer {
                        manager: self.manager.clone(),
                        stage,
                    };
                }
            }
            tokio::select! {
                result = changed.changed() => result.expect("Manager owns admission channel"),
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    }

    pub(crate) fn uploaded(&self, bytes: u64) {
        let mut state = self.manager.state.write();
        if let Some(item) = state.item_mut(&self.id) {
            item.uploaded = (item.uploaded + bytes).min(item.upload_total);
            if let Some(rate) = &mut item.rate {
                rate.record(bytes, Instant::now());
            }
        }
        state.bytes[Stage::Upload as usize] += bytes;
        state.sample(tokio::time::Instant::now());
        drop(state);
        self.manager.changed.send_replace(());
    }

    pub fn attempt(&self, attempt: usize) {
        self.update(|item| {
            item.phase = Phase::Resolving;
            item.attempt = attempt;
            item.error = None;
            item.total = 0;
            item.upload_total = 0;
            item.uploaded = 0;
            item.ranges.clear();
            item.rate = None;
        });
    }

    pub fn phase(&self, phase: Phase) {
        self.update(|item| item.phase = phase);
    }

    pub(crate) fn start_upload(&self, total: u64, saved: u64) {
        self.update(|item| {
            item.upload_total = total;
            item.uploaded = saved.min(total);
            item.rate = Some(Rate::new(Instant::now()));
        });
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
        if let Some(item) = state.item_mut(&self.id)
            && let Ok(index) = item
                .ranges
                .binary_search_by_key(&start, |range| range.start)
        {
            let range = &mut item.ranges[index];
            let received = received.min(range.end - range.start + 1);
            // Verified journal ranges update the display, but are not network
            // traffic. A completion callback must not count the same bytes twice.
            if phase == RangePhase::Active {
                bytes = received.saturating_sub(range.received);
            }
            range.received = received;
            range.phase = phase;
            if let Some(rate) = &mut item.rate {
                rate.record(bytes, now);
            }
        }
        state.bytes[Stage::Download as usize] += bytes;
        state.sample(tokio::time::Instant::now());
        self.manager.changed.send_replace(());
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    Download = 0,
    Upload = 1,
    Prepare = 2,
}

impl Stage {
    fn waiting_phase(self) -> Phase {
        match self {
            Self::Download => Phase::Queued,
            Self::Upload => Phase::WaitingToUpload,
            Self::Prepare => Phase::WaitingToPrepare,
        }
    }

    fn active_phase(self) -> Phase {
        match self {
            Self::Download => Phase::Resolving,
            Self::Upload => Phase::Uploading,
            Self::Prepare => Phase::Preparing,
        }
    }
}

struct WaitingStage {
    manager: DownloadManager,
    stage: Stage,
    waiting: bool,
}

impl Drop for WaitingStage {
    fn drop(&mut self) {
        if self.waiting {
            self.manager.state.write().stage_waiting[self.stage as usize] -= 1;
            self.manager.changed.send_replace(());
        }
    }
}

pub(crate) struct StageTransfer {
    manager: DownloadManager,
    stage: Stage,
}

impl StageTransfer {
    pub(crate) fn finish<T>(self, result: &anyhow::Result<T>, cancelled: bool) {
        let index = self.stage as usize;
        if index < 2
            && !cancelled
            && result.as_ref().is_err_and(|error| {
                crate::errors::classify(error) == crate::errors::Category::Retryable
            })
        {
            self.manager.state.write().failed[index] = true;
        }
    }
}

impl Drop for StageTransfer {
    fn drop(&mut self) {
        let index = self.stage as usize;
        let mut state = self.manager.state.write();
        state.stage_active[index] -= 1;
        if index < 2 {
            state.population_changed[index] = true;
        }
        drop(state);
        self.manager.changed.send_replace(());
    }
}

pub struct ActiveTransfer {
    transfer: Transfer,
    finished: bool,
}

impl ActiveTransfer {
    pub(crate) fn cancelled(mut self) {
        self.transfer.queued();
        self.finished = true;
    }

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
            state.idle_since = Some(tokio::time::Instant::now());
        }
        drop(state);
        self.transfer.manager.changed.send_replace(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
