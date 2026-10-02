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
    retrying: usize,
    not_imported: usize,
}

impl ImportProgress {
    pub fn percent(&self) -> f32 {
        if self.total == 0 {
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
            (self.uploading, "uploading & publishing"),
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
            active.join(" · ")
        }
    }
}

impl Workspace {
    pub(super) fn import_progress(&self, slug: &str) -> ImportProgress {
        // The admitted queue covers this entire pass, independently of the
        // selected podcast and the paginated API episode list.
        let mut progress = ImportProgress::default();
        for item in self
            .progress
            .items
            .iter()
            .filter(|item| item.source_id == slug)
        {
            progress.total += 1;
            match item.phase {
                Phase::Complete => progress.imported += 1,
                Phase::Queued => progress.queued += 1,
                Phase::Resolving => progress.resolving += 1,
                Phase::Downloading => progress.downloading += 1,
                Phase::Preparing => progress.preparing += 1,
                Phase::Uploading => progress.uploading += 1,
                Phase::Retrying => progress.retrying += 1,
                Phase::Failed | Phase::Skipped => progress.not_imported += 1,
            }
        }
        progress
    }

    pub(super) fn import_status(&self, show: &Show, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let running = self.jobs.get(&show.slug);
        let stopping = running.is_some_and(|cancel| cancel.is_cancelled());
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
        if running.is_some() {
            heading = heading.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if stopping {
                        "Stopping import…".into()
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
                    .label("Sync now")
                    .disabled(
                        running.is_some()
                            || self.stopping.is_some()
                            || !show.has_active_subscription,
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.sync(cx))),
            )
            .when(running.is_some(), |row| {
                row.child(
                    Button::new("stop-sync")
                        .outline()
                        .label(if stopping { "Stopping…" } else { "Stop" })
                        .disabled(stopping)
                        .on_click(cx.listener(|view, _, _, cx| {
                            if let Some(cancel) =
                                view.selected.as_ref().and_then(|slug| view.jobs.get(slug))
                            {
                                cancel.cancel();
                            }
                            cx.notify();
                        })),
                )
            });
        status = status.child(heading);
        if running.is_some() {
            status = status
                .child(
                    Progress::new("import-progress")
                        .small()
                        .color(t.action)
                        .loading(progress.total == 0 && !stopping)
                        .value(progress.percent())
                        .accessibility_label(if progress.total == 0 {
                            "Checking the YouTube source".into()
                        } else {
                            format!(
                                "{} of {} episodes imported",
                                progress.imported, progress.total
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child(if stopping {
                            "Finishing current work and saving progress.".into()
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
        } else {
            status = status.child(div().text_color(t.muted).child(
                self.reports.get(&show.slug).cloned().unwrap_or_else(|| {
                    "Syncs automatically every hour while Listenbox is running.".into()
                }),
            ));
        }
        status.into_any_element()
    }
}
