use super::*;
use listenbox_sync_engine::publicapi::{EpisodePage, EpisodeStatus};

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
        if append && (self.episode_loading || self.episode_cursor.is_none()) {
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

    pub(super) fn episode_list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.import_open || self.show().is_none() {
            return div().into_any_element();
        }
        let t = Tokens::current(cx);
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
                    .text_size(px(tokens::TITLE))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Episodes"),
            );
        for episode in &self.episodes {
            let mut metadata = Vec::new();
            if let Some(seconds) = episode.duration_seconds.filter(|v| *v > 0) {
                metadata.push(duration(seconds as u64));
            }
            let state = match episode.status {
                EpisodeStatus::Published => "Published",
                EpisodeStatus::Draft => "Draft",
            };
            metadata.push(state.into());
            pane = pane.child(
                div()
                    .py_3()
                    .border_b_1()
                    .border_color(t.divider)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(episode.title.clone())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child(metadata.join(" · ")),
                    ),
            );
        }
        if self.episode_loading {
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
        } else if self.episodes.is_empty() {
            pane = pane.child(
                div()
                    .py_3()
                    .text_color(t.muted)
                    .child("No episodes synced yet."),
            );
        }
        if self.episode_cursor.is_some() {
            pane = pane.child(
                Button::new("more-episodes")
                    .ghost()
                    .label("Load more episodes")
                    .disabled(self.episode_loading)
                    .on_click(cx.listener(|view, _, _, cx| view.load_episodes(true, cx))),
            );
        }
        pane.into_any_element()
    }
}
