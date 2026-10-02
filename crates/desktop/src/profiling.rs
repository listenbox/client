use gpui_kit::{
    App,
    profiler::{
        self,
        journal::{ForegroundEvent, ForegroundJournalEntry, IntervalBoundary},
    },
};
use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

pub struct Recorder {
    file: std::fs::File,
    clock: Clock,
}

struct Clock {
    anchor: Instant,
    epoch_ms: f64,
}

impl Recorder {
    pub fn open() -> anyhow::Result<Option<Self>> {
        let Some(path) = std::env::var_os("LISTENBOX_RENDER_PROFILE") else {
            return Ok(None);
        };
        let anchor = Instant::now();
        let epoch_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64() * 1000.;
        let mut file = std::fs::File::create_new(path)?;
        use std::io::Write;
        writeln!(
            file,
            "{}",
            json!({
                "type": "session", "pid": std::process::id(), "unix_ms": epoch_ms,
                "version": env!("CARGO_PKG_VERSION"),
                "source_commit": env!("LISTENBOX_SOURCE_COMMIT"),
                "units": "milliseconds", "gpu_completion_measured": false,
            })
        )?;
        Ok(Some(Self {
            file,
            clock: Clock { anchor, epoch_ms },
        }))
    }

    pub fn start(
        self,
        cx: &App,
        runtime: &tokio::runtime::Runtime,
        cancel: CancellationToken,
        tasks: &TaskTracker,
    ) {
        profiler::set_trace_enabled(true);
        let mut collector = cx.foreground_journal().collector();
        let Self { file, clock } = self;
        tasks.spawn_on(
            async move {
                let mut file = tokio::fs::File::from_std(file);
                let mut interval = tokio::time::interval(Duration::from_millis(100));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    let stopping = tokio::select! {
                        _ = cancel.cancelled() => true,
                        _ = interval.tick() => false,
                    };
                    let mut batch = String::new();
                    for entry in collector.collect_unseen().entries {
                        if let Some(value) = clock.record(entry) {
                            batch.push_str(&value.to_string());
                            batch.push('\n');
                        }
                    }
                    batch.push_str(
                        &json!({
                            "type": "checkpoint", "unix_ms": clock.timestamp(Instant::now()),
                        })
                        .to_string(),
                    );
                    batch.push('\n');
                    if let Err(error) = async {
                        file.write_all(batch.as_bytes()).await?;
                        file.flush().await
                    }
                    .await
                    {
                        eprintln!("GPUI profile recording failed: {error}");
                        break;
                    }
                    if stopping {
                        break;
                    }
                }
            },
            runtime.handle(),
        );
    }
}

impl Clock {
    fn timestamp(&self, at: Instant) -> f64 {
        self.epoch_ms + at.duration_since(self.anchor).as_secs_f64() * 1000.
    }

    fn record(&self, entry: ForegroundJournalEntry) -> Option<Value> {
        match entry {
            ForegroundJournalEntry::Boundary(IntervalBoundary::Presented(frame)) => Some(json!({
                "type": "frame", "unix_ms": self.timestamp(frame.presentation.present_end),
                "window": format!("{:?}", frame.frame.window_id),
                "draw_ms": frame.frame.draw_duration().as_secs_f64() * 1000.,
                "present_ms": frame.presentation.present_duration().as_secs_f64() * 1000.,
                "dirty_to_present_ms": frame.dirty_to_present_duration().map(|d| d.as_secs_f64() * 1000.),
                "invalidations": frame.frame.invalidations,
            })),
            ForegroundJournalEntry::Event(ForegroundEvent::Input(input)) => Some(json!({
                "type": "input", "unix_ms": self.timestamp(input.start), "kind": input.kind,
                "dispatch_ms": input.end.duration_since(input.start).as_secs_f64() * 1000.,
                "invalidated": input.caused_invalidation,
            })),
            ForegroundJournalEntry::Event(ForegroundEvent::TaskPoll(task)) => Some(json!({
                "type": "task", "unix_ms": self.timestamp(task.start),
                "duration_ms": task.poll_duration().as_secs_f64() * 1000.,
                "location": task.location.to_string(),
            })),
            ForegroundJournalEntry::Discontinuity { lost } => Some(json!({
                "type": "lost", "unix_ms": self.timestamp(Instant::now()), "entries": lost,
            })),
            _ => None,
        }
    }
}
