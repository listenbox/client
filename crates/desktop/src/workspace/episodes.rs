use super::*;
use gpui_kit::base::Selectable;
use listenbox_sync_engine::{
    downloads::Download,
    publicapi::{EpisodeListItem, EpisodePage, EpisodeStatus},
};

pub(super) struct EpisodeRow {
    pub episode: Option<EpisodeListItem>,
    pub item: Option<Download>,
}

impl EpisodeRow {
    pub fn issue(&self) -> bool {
        self.episode.is_none()
            && self
                .item
                .as_ref()
                .is_some_and(|item| matches!(item.phase, Phase::Failed | Phase::Skipped))
    }
}

pub(super) fn duration(seconds: u64) -> String {
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

impl Workspace {
    pub(super) fn load_episodes(&mut self, append: bool, cx: &mut Context<Self>) {
        if append && (self.episode_loading || self.source_loading || self.episode_cursor.is_none())
        {
            return;
        }
        if let Some(cancel) = self.episode_cancel.take() {
            cancel.cancel();
        }
        self.episode_request += 1;
        self.episode_error = None;
        if !append {
            self.episodes.clear();
            self.episode_cursor = None;
            self.source_items.clear();
            self.source_error = None;
            self.source_loading = false;
        }
        let Some(slug) = self
            .selected
            .clone()
            .filter(|_| self.loaded && self.stopping.is_none())
        else {
            self.episode_loading = false;
            return;
        };
        let cursor = if append {
            self.episode_cursor.clone()
        } else {
            None
        };
        let request = self.episode_request;
        if !append {
            self.source_loading = true;
            let (client, sender, slug) = (self.client.clone(), self.sender.clone(), slug.clone());
            self.tasks.spawn_on(
                async move {
                    let _ =
                        sender.send(Message::SourceItems(request, client.sync_items(slug).await));
                },
                self.runtime.handle(),
            );
        }
        let cancel = self.cancel.child_token();
        self.episode_cancel = Some(cancel.clone());
        self.episode_loading = true;
        let (client, sender) = (self.client.clone(), self.sender.clone());
        self.tasks.spawn_on(
            async move {
                let _ = sender.send(Message::Episodes(
                    request,
                    append,
                    client.episodes(slug, cursor, cancel).await,
                ));
            },
            self.runtime.handle(),
        );
        cx.notify();
    }

    pub(super) fn episodes_received(
        &mut self,
        request: u64,
        append: bool,
        result: anyhow::Result<EpisodePage>,
    ) {
        if request != self.episode_request {
            return;
        }
        self.episode_loading = false;
        self.episode_cancel = None;
        match result {
            Ok(page) => {
                if !append {
                    self.episodes.clear();
                }
                for episode in page.episodes {
                    if !self.episodes.iter().any(|item| item.id == episode.id) {
                        self.episodes.push(episode);
                    }
                }
                if append && page.next_cursor.is_some() && page.next_cursor == self.episode_cursor {
                    self.episode_error = Some(
                        "Could not load the next page. Reload the podcast to try again.".into(),
                    );
                    self.episode_cursor = None;
                } else {
                    self.episode_cursor = page.next_cursor;
                }
            }
            Err(error) => self.episode_error = Some(format!("Could not load episodes. {error}")),
        }
    }

    pub(super) fn episode_rows(&self) -> Vec<EpisodeRow> {
        let mut sources: HashMap<String, Download> = self
            .source_items
            .iter()
            .map(|item| (item.source_url.clone(), item.clone()))
            .collect();
        let mut live = std::collections::HashSet::new();
        for item in self
            .progress
            .items
            .iter()
            .filter(|item| Some(&item.source_id) == self.selected.as_ref())
        {
            live.insert(item.source_url.clone());
            sources.insert(item.source_url.clone(), item.clone());
        }
        let mut published = Vec::new();
        for episode in &self.episodes {
            let item = episode
                .source_url
                .as_ref()
                .and_then(|url| sources.remove(url));
            published.push(EpisodeRow {
                episode: Some(episode.clone()),
                item,
            });
        }
        let mut remaining: Vec<_> = sources.into_values().collect();
        remaining.retain(|item| item.phase != Phase::Complete || live.contains(&item.source_url));
        remaining.sort_by(|a, b| {
            (a.position.unwrap_or(i64::MAX), &a.source_url)
                .cmp(&(b.position.unwrap_or(i64::MAX), &b.source_url))
        });
        let mut remaining = remaining.into_iter().peekable();
        let mut rows = Vec::new();
        // The API array owns published order. Local positions only place pending
        // sources before their next published neighbour; a stale scan cannot
        // reorder episodes received from the server.
        for row in published {
            if let Some(position) = row.item.as_ref().and_then(|item| item.position) {
                while remaining
                    .peek()
                    .is_some_and(|item| item.position.is_some_and(|pending| pending < position))
                {
                    rows.push(EpisodeRow {
                        episode: None,
                        item: remaining.next(),
                    });
                }
            }
            rows.push(row);
        }
        rows.extend(remaining.map(|item| EpisodeRow {
            episode: None,
            item: Some(item),
        }));
        rows
    }

    fn episode_row(&self, row: &EpisodeRow, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let item = row.item.as_ref();
        let episode = row.episode.as_ref();
        let title = episode
            .map(|episode| episode.title.clone())
            .unwrap_or_else(|| item.unwrap().title.clone());
        let status = match episode {
            Some(episode) => match episode.status {
                EpisodeStatus::Published => "Published",
                EpisodeStatus::Draft => "Draft",
            },
            None => match item.unwrap().phase {
                Phase::Queued
                    if !self
                        .jobs
                        .contains_key(self.selected.as_deref().unwrap_or("")) =>
                {
                    "Waiting for sync"
                }
                Phase::Complete => "Publishing…",
                phase => phase.label(),
            },
        };
        let id = episode
            .map(|episode| episode.id.clone())
            .unwrap_or_else(|| item.unwrap().id.clone());
        let mut element = div()
            .id(SharedString::from(format!("episode-row-{id}")))
            .py_3()
            .border_b_1()
            .border_color(t.divider)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap_4()
                    .child(div().flex_1().min_w_0().child(title))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(if row.issue() { t.danger } else { t.muted })
                            .child(status),
                    ),
            );
        if let Some(seconds) = episode
            .and_then(|episode| episode.duration_seconds)
            .filter(|seconds| *seconds > 0)
            .map(|seconds| seconds as u64)
            .or_else(|| item.and_then(|item| item.duration_seconds))
        {
            element = element.child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(duration(seconds)),
            );
        }
        if episode.is_none()
            && let Some(item) = item
        {
            if item.phase == Phase::Downloading {
                let value = if item.total == 0 {
                    0.
                } else {
                    100. * item.received() as f32 / item.total as f32
                };
                element = element
                    .child(
                        Progress::new(SharedString::from(format!("progress-{}", item.id)))
                            .value(value)
                            .accessibility_label(format!("Downloading {}", item.title)),
                    )
                    .child(div().text_size(px(12.)).text_color(t.muted).child(format!(
                        "{:.1} / {:.1} MB · {:.1} MB/s",
                        item.received() as f64 / 1_000_000.,
                        item.total as f64 / 1_000_000.,
                        item.bytes_per_second() as f64 / 1_000_000.
                    )));
            }
            if let Some(reason) = &item.reason {
                element = element.child(div().text_color(t.muted).child(reason.clone()));
            }
            if let Some(error) = &item.error {
                element = element.child(div().text_color(t.danger).child(error.clone()));
            }
            if row.issue() {
                let source = item.source_url.clone();
                element = element.child(
                    div().child(
                        Button::new(SharedString::from(format!("open-source-{}", item.id)))
                            .ghost()
                            .icon(gpui_kit::component::IconName::ExternalLink)
                            .label("Open in YouTube")
                            .on_click(move |_, _, cx| cx.open_url(&source)),
                    ),
                );
            }
        }
        element.into_any_element()
    }

    pub(super) fn episode_list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.import_open || self.show().is_none() {
            return div().into_any_element();
        }
        let t = Tokens::current(cx);
        let rows = self.episode_rows();
        let missing = rows.iter().filter(|row| row.issue()).count();
        let showing_issues = self.show_issues && missing > 0;
        let published = self
            .episodes
            .iter()
            .filter(|episode| episode.status == EpisodeStatus::Published)
            .count();
        let mut pane = div()
            .mt(px(tokens::SPACE))
            .border_t_1()
            .border_color(t.divider)
            .pt(px(tokens::SPACE))
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .id("episode-summary")
                    .text_color(t.muted)
                    .child(format!(
                        "{} · {missing} not imported",
                        if self.episode_cursor.is_some() {
                            format!("{} episodes loaded", self.episodes.len())
                        } else {
                            format!("{published} in RSS")
                        }
                    )),
            );
        pane = pane.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("filter-episodes")
                        .ghost()
                        .label("Episodes")
                        .selected(!showing_issues)
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.show_issues = false;
                            cx.notify();
                        })),
                )
                .when(missing > 0, |tabs| {
                    tabs.child(
                        Button::new("filter-not-imported")
                            .ghost()
                            .label(format!("Not imported ({missing})"))
                            .selected(showing_issues)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.show_issues = true;
                                cx.notify();
                            })),
                    )
                }),
        );
        let visible: Vec<_> = rows
            .iter()
            .filter(|row| row.issue() == showing_issues)
            .collect();
        for row in &visible {
            pane = pane.child(self.episode_row(row, cx));
        }
        if self.episode_loading || self.source_loading {
            pane = pane.child(div().py_3().text_color(t.muted).child("Loading episodes…"));
        } else if let Some(error) = &self.episode_error {
            pane = pane
                .child(div().text_color(t.danger).child(error.clone()))
                .child(
                    Button::new("retry-episodes")
                        .ghost()
                        .label("Reload episodes")
                        .on_click(cx.listener(|view, _, _, cx| view.load_episodes(false, cx))),
                );
        } else if visible.is_empty() && self.source_error.is_none() {
            pane = pane.child(div().py_3().text_color(t.muted).child(if showing_issues {
                "Every listed video has imported or is waiting for sync."
            } else {
                "No episodes synced yet. New episodes will appear here as they sync."
            }));
        }
        if let Some(error) = &self.source_error {
            pane = pane
                .child(div().text_color(t.danger).child(error.clone()))
                .child(
                    Button::new("retry-imports")
                        .ghost()
                        .label("Reload imports")
                        .on_click(cx.listener(|view, _, _, cx| view.load_episodes(false, cx))),
                );
        }
        if self.episode_cursor.is_some() {
            pane = pane.child(
                Button::new("more-episodes")
                    .ghost()
                    .label("Load more episodes")
                    .disabled(self.episode_loading || self.source_loading)
                    .on_click(cx.listener(|view, _, _, cx| view.load_episodes(true, cx))),
            );
        }
        pane.into_any_element()
    }
}
