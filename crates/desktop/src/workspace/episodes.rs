use super::*;
use gpui_kit::base::{Link, ScrollbarHandle, Selectable};
use listenbox_sync_engine::{
    downloads::Download,
    publicapi::{EpisodeListItem, EpisodeStatus},
};

pub(super) struct EpisodeRow {
    pub episode: Option<EpisodeListItem>,
    pub item: Option<Download>,
}

impl EpisodeRow {
    fn key(&self) -> &str {
        self.episode
            .as_ref()
            .map(|episode| episode.id.as_str())
            .unwrap_or_else(|| self.item.as_ref().unwrap().id.as_str())
    }

    pub fn issue(&self) -> bool {
        self.episode.is_none()
            && self
                .item
                .as_ref()
                .is_some_and(|item| matches!(item.phase, Phase::Failed | Phase::Skipped))
    }
}

pub(super) struct EpisodeList {
    state: ListState,
    pub dirty: bool,
    rows: Vec<EpisodeRow>,
    missing: usize,
    issues: bool,
    focus: Vec<Option<FocusHandle>>,
}

#[derive(Clone)]
struct EpisodeScrollbar(ListState);

impl ScrollbarHandle for EpisodeScrollbar {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.0.viewport_bounds()
    }

    fn offset(&self) -> Point<Pixels> {
        self.0.offset()
    }

    fn content_size(&self) -> Size<Pixels> {
        self.0.content_size()
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        let max = self.0.max_offset_for_scrollbar().y;
        if max > px(0.) && offset.y <= -max {
            // The drag freezes an estimated height, while wrapped rows keep
            // being measured. The thumb's end means the actual last item.
            self.0.scroll_to(ListOffset {
                item_ix: self.0.item_count(),
                offset_in_item: px(0.),
            });
        } else {
            self.0.set_offset_from_scrollbar(offset);
        }
    }

    fn start_drag(&self) {
        self.0.scrollbar_drag_started();
    }

    fn end_drag(&self) {
        self.0.scrollbar_drag_ended();
    }
}

impl EpisodeList {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        let focus = vec![
            Some(cx.focus_handle().tab_stop(false)),
            Some(cx.focus_handle().tab_stop(false)),
        ];
        let state = ListState::new(0, ListAlignment::Top, px(300.));
        state.splice_focusable(0..0, focus.iter().cloned());
        state.clone().with_uniform_item_height(px(80.));
        Self {
            state,
            dirty: true,
            rows: vec![],
            missing: 0,
            issues: false,
            focus,
        }
    }

    pub fn reset(&mut self) {
        let footer = self.focus.pop().unwrap();
        self.focus.truncate(1);
        self.focus.push(footer);
        self.state.reset_with_uniform_height(2, px(80.));
        self.state
            .splice_focusable(0..2, self.focus.iter().cloned());
        self.rows.clear();
        self.dirty = true;
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
    pub(super) fn load_episodes(&mut self, cx: &mut Context<Self>) {
        self.episode_view.dirty = true;
        if let Some(cancel) = self.episode_cancel.take() {
            cancel.cancel();
        }
        self.episode_request += 1;
        self.episode_error = None;
        self.episodes.clear();
        self.source_items.clear();
        self.last_synced = None;
        self.source_error = None;
        self.source_loading = false;
        let Some(slug) = self
            .selected
            .clone()
            .filter(|_| self.loaded && self.stopping.is_none())
        else {
            self.episode_loading = false;
            return;
        };
        let request = self.episode_request;
        self.source_loading = true;
        let (client, sender, slug_items) = (self.client.clone(), self.sender.clone(), slug.clone());
        self.tasks.spawn_on(
            async move {
                let _ = sender.send(Message::SyncState(
                    request,
                    client.sync_state(slug_items).await,
                ));
            },
            self.runtime.handle(),
        );
        let cancel = self.cancel.child_token();
        self.episode_cancel = Some(cancel.clone());
        self.episode_loading = true;
        let (client, sender) = (self.client.clone(), self.sender.clone());
        self.tasks.spawn_on(
            async move {
                let _ = sender.send(Message::Episodes(
                    request,
                    client.episodes(slug, cancel).await,
                ));
            },
            self.runtime.handle(),
        );
        cx.notify();
    }

    pub(super) fn episodes_received(
        &mut self,
        request: u64,
        result: anyhow::Result<Vec<EpisodeListItem>>,
    ) {
        if request != self.episode_request {
            return;
        }
        self.episode_loading = false;
        self.episode_cancel = None;
        match result {
            Ok(episodes) => self.episodes = episodes,
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
                Phase::Complete => "Imported",
                phase => phase.label(),
            },
        };
        let id = episode
            .map(|episode| episode.id.clone())
            .unwrap_or_else(|| item.unwrap().id.clone());
        let title = if row.issue() {
            Link::new(SharedString::from(format!("open-source-{id}")))
                .href(item.unwrap().source_url.clone())
                .open_with(|url, _, _, cx| cx.open_url(url))
                .accessibility_label(title.clone())
                .text_color(t.ink)
                .text_decoration_1()
                .text_decoration_color(t.muted)
                .cursor_pointer()
                .hover(|style| style.text_decoration_color(t.ink))
                .focus_visible(|style| style.bg(t.selected).text_decoration_2())
                .child(title)
                .into_any_element()
        } else {
            div().child(title).into_any_element()
        };
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
                            .text_color(
                                if row.issue()
                                    && item.is_some_and(|item| item.phase == Phase::Failed)
                                {
                                    t.danger
                                } else {
                                    t.muted
                                },
                            )
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
                element = element.child(
                    div()
                        .text_color(if row.issue() { t.danger } else { t.muted })
                        .child(reason.clone()),
                );
            }
            if let Some(error) = &item.error {
                element = element.child(div().text_color(t.danger).child(error.clone()));
            }
        }
        element.test_support().into_any_element()
    }

    fn prepare_episode_list(&mut self, cx: &mut Context<Self>) {
        if !self.episode_view.dirty {
            return;
        }
        let mut rows = self.episode_rows();
        let missing = rows.iter().filter(|row| row.issue()).count();
        let issues = self.show_issues && missing > 0;
        rows.retain(|row| row.issue() == issues);
        let cached = &mut self.episode_view;
        let prefix = cached
            .rows
            .iter()
            .zip(&rows)
            .take_while(|(a, b)| a.key() == b.key())
            .count();
        let suffix = cached.rows[prefix..]
            .iter()
            .rev()
            .zip(rows[prefix..].iter().rev())
            .take_while(|(a, b)| a.key() == b.key())
            .count();
        let mut focus = vec![cached.focus[0].clone()];
        focus.extend((0..rows.len()).map(|ix| {
            if ix < prefix {
                cached.focus[ix + 1].clone()
            } else if ix >= rows.len() - suffix {
                cached.focus[cached.rows.len() - (rows.len() - ix) + 1].clone()
            } else {
                rows[ix].issue().then(|| cx.focus_handle().tab_stop(false))
            }
        }));
        focus.push(cached.focus.last().unwrap().clone());
        if cached.issues != issues {
            cached
                .state
                .reset_with_uniform_height(rows.len() + 2, px(80.));
            cached
                .state
                .splice_focusable(0..rows.len() + 2, focus.iter().cloned());
        } else {
            // Retain measurements and the reading position for unchanged rows,
            // including when the complete API episode list is refreshed.
            cached.state.splice_focusable(
                prefix + 1..cached.rows.len() - suffix + 1,
                focus[prefix + 1..rows.len() - suffix + 1].iter().cloned(),
            );
            // A progress update or saved failure can change a row's height even
            // while it is off-screen. Keep its scroll anchor when remeasuring.
            cached.state.remeasure_items(0..rows.len() + 2);
        }
        // Wheel scrolling must be able to seek into unmeasured rows. Actual
        // layout replaces this estimate as each row reaches the viewport.
        cached.state.clone().with_uniform_item_height(px(80.));
        cached.rows = rows;
        cached.focus = focus;
        cached.missing = missing;
        cached.issues = issues;
        cached.dirty = false;
    }

    pub(super) fn episode_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.prepare_episode_list(cx);
        let state = self.episode_view.state.clone();
        // Header/footer controls can change without an episode-data update.
        state.remeasure_items(0..1);
        state.remeasure_items(state.item_count() - 1..state.item_count());
        let content = list(
            state.clone(),
            cx.processor(|view, ix: usize, window, cx| {
                // Keep keyboard focus alive when an interactive row (including
                // the header and recovery actions) scrolls outside the viewport.
                let pane = div()
                    .px(px(tokens::SPACE))
                    .when_some(view.episode_view.focus[ix].as_ref(), |pane, focus| {
                        pane.track_focus(focus)
                    });
                if ix == 0 {
                    pane.pt(px(tokens::SPACE))
                        .pb_2()
                        .child(view.content_header(window, cx))
                        .child(view.episode_heading(cx))
                        .into_any_element()
                } else if let Some(row) = view.episode_view.rows.get(ix - 1) {
                    pane.pb_2()
                        .child(view.episode_row(row, cx))
                        .into_any_element()
                } else {
                    pane.pb(px(tokens::SPACE))
                        .child(view.episode_footer(cx))
                        .into_any_element()
                }
            }),
        )
        .size_full();
        div()
            .relative()
            .size_full()
            .child(content)
            .child(Scrollbar::vertical(&EpisodeScrollbar(state)).mode(ScrollbarMode::Always))
            .into_any_element()
    }

    fn episode_heading(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let missing = self.episode_view.missing;
        let showing_issues = self.episode_view.issues;
        let pane = div()
            .mt(px(tokens::SPACE))
            .border_t_1()
            .border_color(t.divider)
            .pt(px(tokens::SPACE))
            .flex()
            .flex_col()
            .gap_2();
        pane.child(
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
                            view.episode_view.dirty = true;
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
                                view.episode_view.dirty = true;
                                cx.notify();
                            })),
                    )
                })
                .when(showing_issues && missing > 0, |tabs| {
                    tabs.child(
                        Button::new("retry-not-imported")
                            .outline()
                            .icon(assets::IconName::RefreshCw)
                            .label("Retry imports")
                            .disabled(
                                self.stopping.is_some()
                                    || self
                                        .selected
                                        .as_ref()
                                        .is_some_and(|slug| self.jobs.contains_key(slug))
                                    || self.show().is_none_or(|show| !show.has_active_subscription),
                            )
                            .on_click(cx.listener(|view, _, _, cx| view.sync(cx))),
                    )
                }),
        )
        .into_any_element()
    }

    fn episode_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let mut pane = div().flex().flex_col().gap_2();
        if self.episode_loading || self.source_loading {
            pane = pane.child(div().py_3().text_color(t.muted).child(
                match (self.episode_loading, self.source_loading) {
                    (true, true) => "Loading episodes and imports…",
                    (true, false) => "Loading episodes…",
                    _ => "Loading saved imports…",
                },
            ));
        } else if let Some(error) = &self.episode_error {
            pane = pane
                .child(div().text_color(t.danger).child(error.clone()))
                .child(
                    Button::new("retry-episodes")
                        .ghost()
                        .label("Reload episodes")
                        .on_click(cx.listener(|view, _, _, cx| view.load_episodes(cx))),
                );
        } else if self.episode_view.rows.is_empty() && self.source_error.is_none() {
            pane = pane.child(div().py_3().text_color(t.muted).child(
                if self.episode_view.issues {
                    "Every listed video has imported or is waiting for sync."
                } else {
                    "No episodes synced yet. New episodes will appear here as they sync."
                },
            ));
        }
        if let Some(error) = &self.source_error {
            pane = pane
                .child(div().text_color(t.danger).child(error.clone()))
                .child(
                    Button::new("retry-imports")
                        .ghost()
                        .label("Reload imports")
                        .on_click(cx.listener(|view, _, _, cx| view.load_episodes(cx))),
                );
        }
        pane.into_any_element()
    }
}
