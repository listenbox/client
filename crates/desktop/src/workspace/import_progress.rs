use super::*;

#[derive(Default)]
pub(super) struct ImportProgress {
    pub total: usize,
    pub imported: usize,
    queued: usize,
    resolving: usize,
    downloading: usize,
    preparing: usize,
    uploading: usize,
    publishing: usize,
    upload_bytes: u64,
    upload_total: u64,
    upload_rate: u64,
    retrying: usize,
    not_imported: usize,
}

impl ImportProgress {
    pub fn accessibility_label(&self) -> String {
        if self.upload_total > 0 {
            "Prepared media upload progress".into()
        } else if self.total == 0 {
            "Checking the YouTube source".into()
        } else {
            format!("{} of {} episodes imported", self.imported, self.total)
        }
    }

    pub fn percent(&self) -> f32 {
        if self.upload_total > 0 {
            100. * self.upload_bytes as f32 / self.upload_total as f32
        } else if self.total == 0 {
            0.
        } else {
            100. * self.imported as f32 / self.total as f32
        }
    }

    fn activity(&self) -> String {
        let stages = [
            (self.resolving, "reading media"),
            (self.downloading, "downloading"),
            (self.preparing, "preparing"),
            (self.uploading, "uploading"),
            (self.publishing, "publishing"),
            (self.retrying, "retrying"),
            (self.queued, "queued"),
        ];
        let active: Vec<_> = stages
            .into_iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, label)| format!("{count} {label}"))
            .collect();
        if active.is_empty() {
            "Finishing the sync…".into()
        } else {
            let stages = active.join(" · ");
            if self.upload_total > 0 {
                format!(
                    "{stages} · {:.1} / {:.1} MB sent · {:.1} MB/s",
                    self.upload_bytes as f64 / 1_000_000.,
                    self.upload_total as f64 / 1_000_000.,
                    self.upload_rate as f64 / 1_000_000.
                )
            } else {
                stages
            }
        }
    }
}

impl Workspace {
    pub(super) fn creation_status(
        &self,
        preparation: &ImportPreparation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Tokens::current(cx);
        let video = self.import_kind == ShowSourceKind::Video;
        let (heading, explanation) = match preparation.stage {
            ImportStage::CheckingPlan => (
                "Checking your plan…",
                "Checking your team’s plan before reading the playlist.",
            ),
            ImportStage::ReadingPlaylist => (
                "Opening the playlist…",
                "Connecting to YouTube to read the playlist title and videos.",
            ),
            ImportStage::ScanningPlaylist => (
                "Reading all playlist videos…",
                "Reading every playlist page to check that it fits your video storage. Large playlists take longer.",
            ),
            ImportStage::CreatingPodcast => (
                "Creating your podcast…",
                if video {
                    "Playlist checked. Creating the podcast before importing episodes."
                } else {
                    "Creating the podcast. Episodes will start importing next."
                },
            ),
        };
        let mut status = div()
            .id("creation-status")
            .role(Role::Status)
            .aria_label(heading)
            .aria_description(explanation)
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(tokens::TITLE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(heading),
            )
            .child(div().text_color(t.muted).child(explanation))
            .child(
                Progress::new("creation-progress")
                    .small()
                    .color(t.action)
                    .loading(true)
                    .accessibility_label(heading),
            );
        if let Some(scan) = &preparation.scan {
            let count = format!(
                "{} {} found{}",
                scan.videos,
                if scan.videos == 1 { "video" } else { "videos" },
                if preparation.stage == ImportStage::ScanningPlaylist {
                    " so far"
                } else {
                    ""
                }
            );
            status = status
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(scan.title.clone()),
                )
                .child(
                    div()
                        .id("creation-videos")
                        .role(Role::Label)
                        .aria_label(count.clone())
                        .child(count)
                        .test_support(),
                )
                .child(
                    div()
                        .id("creation-duration")
                        .role(Role::Label)
                        .aria_label(format!(
                            "Known video duration: {}",
                            storage_duration(scan.estimated_seconds)
                        ))
                        .flex()
                        .justify_between()
                        .gap_3()
                        .child(div().text_color(t.muted).child("Known video duration"))
                        .child(storage_duration(scan.estimated_seconds))
                        .test_support(),
                );
            if scan.unknown_durations > 0 {
                status = status.child(div().text_size(px(12.)).text_color(t.muted).child(format!(
                    "{} {} no listed duration. Actual storage is checked as videos are imported.",
                    scan.unknown_durations,
                    if scan.unknown_durations == 1 {
                        "video has"
                    } else {
                        "videos have"
                    },
                )));
            }
        }
        if let Some(remaining) = preparation.video_remaining_seconds {
            status = status.child(
                div()
                    .id("creation-storage")
                    .role(Role::Label)
                    .aria_label(format!(
                        "Video storage available: {}",
                        storage_duration(remaining)
                    ))
                    .border_t_1()
                    .border_color(t.divider)
                    .pt_3()
                    .flex()
                    .justify_between()
                    .gap_3()
                    .child(div().text_color(t.muted).child("Video storage available"))
                    .child(storage_duration(remaining))
                    .test_support(),
            );
        }
        status.test_support().into_any_element()
    }

    pub(super) fn import_progress(&self, slug: &str) -> ImportProgress {
        // The engine publishes every playlist entry, including episodes already
        // imported before this sync, independently of the selected podcast.
        let mut progress = ImportProgress::default();
        for item in self
            .progress
            .items
            .iter()
            .filter(|item| item.source_id == slug)
        {
            progress.total += 1;
            progress.upload_bytes += item.uploaded;
            progress.upload_total += item.upload_total;
            progress.upload_rate +=
                item.bytes_per_second() * u64::from(item.phase == Phase::Uploading);
            match item.phase {
                Phase::Complete => progress.imported += 1,
                Phase::Queued | Phase::WaitingToPrepare | Phase::WaitingToUpload => {
                    progress.queued += 1
                }
                Phase::Resolving => progress.resolving += 1,
                Phase::Downloading => progress.downloading += 1,
                Phase::Preparing => progress.preparing += 1,
                Phase::Uploading => progress.uploading += 1,
                Phase::Publishing => progress.publishing += 1,
                Phase::Retrying => progress.retrying += 1,
                Phase::Failed | Phase::Skipped => progress.not_imported += 1,
            }
        }
        progress
    }

    pub(super) fn import_status(&self, show: &Show, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let running = self.jobs.get(&show.slug);
        let pausing = running.is_some_and(|cancel| cancel.is_cancelled());
        let paused = self.paused.contains(&show.slug);
        let progress = self.import_progress(&show.slug);
        let mut status = div()
            .id("import-status")
            .border_t_1()
            .border_color(t.divider)
            .pt(px(tokens::SPACE))
            .flex()
            .flex_col()
            .gap_3();
        let mut heading = div().flex().items_center().gap_3();
        if running.is_some() || paused {
            heading = heading.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if pausing {
                        "Pausing sync…".into()
                    } else if paused {
                        "Sync paused".into()
                    } else if progress.total == 0 {
                        "Checking for new episodes…".into()
                    } else {
                        format!(
                            "{} of {} episodes imported",
                            progress.imported, progress.total
                        )
                    }),
            );
        }
        heading = heading
            .child(
                Button::new("sync-now")
                    .primary()
                    .icon(assets::IconName::RefreshCw)
                    .label(if paused { "Resume" } else { "Sync now" })
                    .disabled(
                        running.is_some()
                            || self.stopping.is_some()
                            || !show.has_active_subscription,
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.sync(cx))),
            )
            .when(!paused || running.is_some(), |row| {
                row.child(
                    Button::new("pause-sync")
                        .outline()
                        .label(if pausing { "Pausing…" } else { "Pause" })
                        .disabled(pausing || self.stopping.is_some())
                        .on_click(cx.listener(|view, _, _, cx| view.pause(cx))),
                )
            });
        status = status.child(heading);
        if running.is_some() {
            status = status
                .child(
                    Progress::new("import-progress")
                        .small()
                        .color(t.action)
                        .loading(progress.total == 0 && !pausing)
                        .value(progress.percent())
                        .accessibility_label(progress.accessibility_label()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child(if pausing {
                            "Finishing current writes and saving progress.".into()
                        } else if progress.total == 0 {
                            "Reading YouTube and comparing it with this podcast.".into()
                        } else {
                            progress.activity()
                        }),
                )
                .when(progress.not_imported > 0, |status| {
                    status.child(div().text_size(px(12.)).text_color(t.danger).child(format!(
                        "{} not imported — see Not imported below for details.",
                        progress.not_imported
                    )))
                });
            if let Some(SyncReport::Summary(report)) = self.reports.get(&show.slug) {
                let summary = format!(
                    "{} added · {} removed · {} unchanged · {} skipped{}",
                    report.added,
                    report.removed,
                    report.unchanged,
                    report.skipped,
                    if report.reordered {
                        " · Order updated"
                    } else {
                        ""
                    }
                );
                status = status.child(
                    div()
                        .id("sync-summary")
                        .role(Role::Label)
                        .aria_label(summary.clone())
                        .text_color(t.muted)
                        .child(summary)
                        .test_support(),
                );
            }
        } else if paused {
            status = status.child(
                div()
                    .text_color(t.muted)
                    .child("Paused for this session. Resume to continue syncing."),
            );
        } else if let Some(SyncReport::Notice(notice)) = self.reports.get(&show.slug) {
            status = status.child(div().text_color(t.muted).child(notice.clone()));
        }
        status.into_any_element()
    }
}

fn storage_duration(seconds: i64) -> String {
    if seconds == 0 {
        "0 min".into()
    } else if seconds < 60 {
        "Less than 1 min".into()
    } else {
        let hours = seconds / 3600;
        let minutes = seconds / 60 % 60;
        match (hours, minutes) {
            (0, minutes) => format!("{minutes} min"),
            (hours, 0) => format!("{hours} hr"),
            (hours, minutes) => format!("{hours} hr {minutes} min"),
        }
    }
}
