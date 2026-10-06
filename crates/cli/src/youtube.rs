use anyhow::Result;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use listenbox_sync_engine::{
    api::Api,
    client::{Client, SyncEvent},
    downloads::{Download, DownloadManager, Phase, Progress},
    publicapi::Show,
};
use std::{collections::HashSet, time::Duration};

pub async fn sync(api: &Api, show: Option<&str>, watch: bool) -> Result<()> {
    let client = Client::desktop(api.config.clone())?;
    let manager = client.downloads();
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let work = client.sync_youtube(show, watch, api.cancel.clone(), move |event| {
        let _ = sender.send(event);
    });
    tokio::pin!(work);
    let mut output = Output::new(watch)?;
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cancelling = false;
    let result = loop {
        tokio::select! {
            result = &mut work => break result,
            Some(event) = events.recv() => output.event(event, &manager, api.cancel.is_cancelled()),
            _ = api.cancel.cancelled(), if !cancelling => {
                cancelling = true;
                output.print("Stopping… finishing current writes and saving progress.");
            },
            _ = tick.tick() => output.refresh(&manager, cancelling),
        }
    };
    // Completion and the final failure can arrive together. Preserve all queued
    // terminal events before clearing the footer, even for very fast syncs.
    while let Ok(event) = events.try_recv() {
        output.event(event, &manager, api.cancel.is_cancelled());
    }
    output.refresh(&manager, api.cancel.is_cancelled());
    output.bar.finish_and_clear();
    if api.cancel.is_cancelled() {
        eprintln!("Stopped. Progress saved; run `listenbox youtube sync` to resume.");
        return Ok(());
    }
    result
}

struct Output {
    bar: ProgressBar,
    shown: HashSet<(String, String)>,
    shows: Vec<Show>,
    finished: usize,
    watch: bool,
    waiting: bool,
}

impl Output {
    fn new(watch: bool) -> Result<Self> {
        let bar = ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr_with_hz(5));
        bar.set_style(
            ProgressStyle::with_template("{spinner} {msg}\n{prefix}")?
                .tick_strings(&["|", "/", "-", "\\", " "]),
        );
        bar.set_message("Finding YouTube shows across your teams…");
        bar.set_prefix("Ctrl+C to stop and save progress");
        Ok(Self {
            bar,
            shown: HashSet::new(),
            shows: Vec::new(),
            finished: 0,
            watch,
            waiting: false,
        })
    }

    fn print(&self, message: &str) {
        if self.bar.is_hidden() {
            eprintln!("{message}");
        } else {
            self.bar.println(message);
        }
    }

    fn event(&mut self, event: SyncEvent, manager: &DownloadManager, cancelled: bool) {
        match event {
            SyncEvent::ScanStarted(shows) => {
                self.shown.clear();
                self.finished = 0;
                self.waiting = false;
                self.shows = shows;
                self.bar.reset_elapsed();
                let teams = self
                    .shows
                    .iter()
                    .map(|show| &show.team_id)
                    .collect::<HashSet<_>>()
                    .len();
                self.print(&format!(
                    "YouTube sync · {} shows across {teams} teams",
                    self.shows.len()
                ));
                if self.shows.is_empty() {
                    self.print("No eligible YouTube shows. Import a source with `listenbox import <url>`; an active paid plan is required.");
                }
            }
            SyncEvent::ShowFinished {
                slug,
                result,
                items,
            } => {
                self.failures(items);
                self.finished += 1;
                match result {
                    Ok(report) if report.stopped => self.show_failure(
                        &slug,
                        "Listenbox stopped YouTube importing for this show. Progress is saved.",
                    ),
                    Ok(report) => self.bar.suspend(|| {
                        println!(
                            "{}: {} added, {} removed, {} unchanged, {} skipped{}",
                            clean(&slug),
                            report.added,
                            report.removed,
                            report.unchanged,
                            report.skipped,
                            if report.reordered {
                                "; feed order updated"
                            } else {
                                ""
                            }
                        )
                    }),
                    Err(error) if !cancelled => {
                        // Cancellation is a saved queue, not a failed import.
                        if !manager
                            .not_imported()
                            .iter()
                            .any(|item| item.source_id == slug && item.phase == Phase::Failed)
                        {
                            self.show_failure(&slug, &format!("{error:#}"));
                        }
                    }
                    Err(_) => {}
                }
            }
            SyncEvent::ScanFinished { failed } => {
                let progress = manager.progress();
                self.print(&format!(
                    "{} · {}/{} episodes imported · {} not imported · {failed} shows failed",
                    if cancelled {
                        "Sync stopped"
                    } else {
                        "Sync finished"
                    },
                    progress.imported,
                    progress.total,
                    progress.not_imported
                ));
                self.waiting = self.watch && !cancelled;
                if self.waiting && self.bar.is_hidden() {
                    self.print(
                        "Watching for new episodes; the next scan is hourly. Ctrl+C to stop.",
                    );
                }
            }
        }
    }

    fn show_failure(&self, slug: &str, reason: &str) {
        let show = self.shows.iter().find(|show| show.slug == slug);
        self.print(&format!(
            "\nNot synced · {} (--show {})\n  {}\n  {}\n",
            clean(show.map_or(slug, |show| show.title.as_str())),
            clean(slug),
            clean(show.and_then(|show| show.youtube_linkage()).unwrap_or("")),
            clean(reason)
        ));
    }

    fn failures(&mut self, items: Vec<Download>) {
        for item in items {
            let reason = item
                .reason
                .as_deref()
                .or(item.error.as_deref())
                .unwrap_or("No failure reason was reported. Sync again to check this video.")
                .replace(
                    "Open Settings → YouTube to add fresh cookies",
                    "Run `listenbox youtube cookies import <netscape.txt>` with fresh cookies",
                );
            if !self.shown.insert((item.id, reason.clone())) {
                continue;
            }
            let heading = if self.bar.is_hidden() {
                "Not imported".into()
            } else {
                console::style("Not imported")
                    .for_stderr()
                    .red()
                    .bold()
                    .to_string()
            };
            self.print(&format!(
                "\n{heading} · {}\n  {} (--show {})\n  {}\n  {}\n",
                clean(&item.title),
                clean(&item.source_title),
                clean(&item.source_id),
                clean(&item.source_url),
                clean(&reason)
            ));
        }
    }

    fn refresh(&mut self, manager: &DownloadManager, cancelling: bool) {
        self.failures(manager.not_imported());
        if self.bar.is_hidden() {
            return;
        }
        let progress = manager.progress();
        let width = usize::from(console::Term::stderr().size().1).saturating_sub(1);
        let clip = |line: String| console::truncate_str(&line, width, "…").into_owned();
        let heading = if cancelling {
            "Stopping · saving progress"
        } else if self.waiting {
            "Watching · next scan hourly"
        } else {
            "YouTube sync"
        };
        let counts = format!(
            "{}/{} imported · {} not imported · {} queued",
            progress.imported, progress.total, progress.not_imported, progress.queued
        );
        self.bar.set_message(format!(
            "{}\n{}",
            clip(format!(
                "{heading} · {}/{} shows finished",
                self.finished,
                self.shows.len()
            )),
            clip(counts)
        ));
        if progress.total == 0 || self.waiting || cancelling {
            self.bar.set_style(
                ProgressStyle::with_template(
                    "{msg}\n{spinner} {prefix}\n{elapsed_precise} elapsed · Ctrl+C to stop",
                )
                .expect("valid sync spinner template")
                .tick_strings(&["|", "/", "-", "\\", " "]),
            );
        } else {
            self.bar.set_style(
                ProgressStyle::with_template("{msg}\n{wide_bar} {percent:>3}% imported\n{prefix}")
                    .expect("valid sync progress template")
                    .progress_chars("━━─"),
            );
            self.bar.set_length(progress.total as u64);
            self.bar.set_position(progress.imported as u64);
        }
        let activity = if cancelling {
            "Finishing owned writes; the queue will resume next time.".into()
        } else if self.waiting {
            "Ctrl+C to stop. New shows are discovered on each scan.".into()
        } else if progress.total == 0 {
            "Reading YouTube sources and comparing saved episodes…".into()
        } else {
            activity(&progress)
        };
        self.bar.set_prefix(clip(activity));
        self.bar.tick();
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.bar.finish_and_clear();
    }
}

fn activity(progress: &Progress) -> String {
    let stages = [
        (progress.resolving, "reading"),
        (progress.downloading, "downloading"),
        (progress.preparing, "preparing"),
        (progress.uploading, "uploading"),
        (progress.publishing, "publishing"),
        (progress.retrying, "retrying"),
    ];
    let mut parts: Vec<_> = stages
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, name)| format!("{count} {name}"))
        .collect();
    if progress.download_rate > 0 {
        parts.push(format!(
            "↓ {:.1} MB/s",
            progress.download_rate as f64 / 1_000_000.
        ));
    }
    if progress.upload_rate > 0 {
        parts.push(format!(
            "↑ {:.1} MB/s",
            progress.upload_rate as f64 / 1_000_000.
        ));
    }
    if parts.is_empty() {
        "Finishing source scans and feed updates…".into()
    } else {
        parts.join(" · ")
    }
}

fn clean(value: &str) -> String {
    console::strip_ansi_codes(value)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}
