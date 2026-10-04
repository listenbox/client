use super::*;
use gpui_kit::component::input::Textarea;
use listenbox_sync_engine::cookies;

pub(super) fn cookie_input(
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<TextareaState> {
    cx.new(|cx| {
        TextareaState::new(window, cx)
            .rows(6)
            .placeholder("Paste the complete Netscape cookies.txt export here")
    })
}
impl Workspace {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open || self.stopping.is_some() {
            return;
        }
        self.settings_open = true;
        self.cookie_message = None;
        self.cookie_error = None;
        self.cookie_input = cookie_input(window, cx);
        if !self.cookie_busy {
            self.cookie_busy = true;
            let (jar, sender) = (self.client.cookie_jar(), self.sender.clone());
            self.tasks.spawn_on(
                async move {
                    let result = tokio::task::spawn_blocking(move || jar.is_enabled())
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|result| result);
                    let _ = sender.send(Message::CookieStatus(result));
                },
                self.runtime.handle(),
            );
        }
        cx.notify();
    }
    fn save_cookies(&mut self, remove: bool, cx: &mut Context<Self>) {
        if self.cookie_busy || self.stopping.is_some() {
            return;
        }
        let text = self.cookie_input.read(cx).value().to_string();
        self.cookie_busy = true;
        self.cookie_error = None;
        self.cookie_message = None;
        self.cookie_input
            .update(cx, |input, cx| input.set_disabled(true, cx));
        let (jar, sender) = (self.client.cookie_jar(), self.sender.clone());
        self.tasks.spawn_on(
            async move {
                let result = tokio::task::spawn_blocking(move || {
                    if remove {
                        jar.remove()
                    } else {
                        jar.import(&text)
                    }
                })
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
                let _ = sender.send(Message::CookiesSaved(!remove, result));
            },
            self.runtime.handle(),
        );
        cx.notify();
    }
    pub(super) fn cookies_received(
        &mut self,
        saved: bool,
        result: anyhow::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cookie_busy = false;
        self.cookie_input
            .update(cx, |input, cx| input.set_disabled(false, cx));
        match result {
            Ok(()) => {
                self.cookie_saved = saved;
                // Drop the editing history too: Undo must not reveal saved secrets.
                self.cookie_input = cookie_input(window, cx);
                self.cookie_message = Some(
                    if saved {
                        "Cookies saved. Return to your podcast and choose Sync now."
                    } else {
                        "Cookies removed. Future requests will use anonymous access."
                    }
                    .into(),
                );
            }
            Err(error) => self.cookie_error = Some(error.to_string()),
        }
    }
    pub(super) fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let busy = self.cookie_busy || self.stopping.is_some();
        div().flex().flex_col().gap(px(tokens::SPACE)).max_w(px(720.))
            .child(div().flex().justify_between().items_center()
                .child(div().text_size(px(tokens::PAGE_TITLE)).font_weight(FontWeight::BOLD).child("Settings"))
                .child(Button::new("close-settings").ghost().label("Done").disabled(busy).on_click(cx.listener(|view, _, window, cx| {
                    view.settings_open = false;
                    view.cookie_input = cookie_input(window, cx);
                    view.focus.focus(window, cx);
                    cx.notify();
                }))))
            .child(div().flex().flex_col().gap_2()
                .child(div().text_size(px(tokens::TITLE)).font_weight(FontWeight::SEMIBOLD).child("YouTube"))
                .child("If YouTube asks you to sign in, add cookies from a private browser session to continue importing.")
                .child(div().text_color(t.muted).child("Cookies stay on this computer and are shared with the Listenbox CLI. Keep them private, like a password."))
                .child(div().flex().items_start().child(Button::new("cookie-guide").ghost().icon(assets::IconName::ExternalLink).label("How to export YouTube cookies").on_click(|_, _, cx| cx.open_url(cookies::GUIDE_URL)))))
            .child(div().flex().flex_col().gap_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("YouTube cookies"))
                .child(div().text_color(t.muted).child(if self.cookie_saved { "A session is saved. Pasting a new export replaces it." } else { "Paste a Netscape cookies.txt export, including the header line." }))
                .child(Textarea::new(&self.cookie_input).h(px(132.)).accessibility_id("cookie-export").aria_label("YouTube cookies").disabled(busy))
                .when_some(self.cookie_error.clone(), |pane, error| pane.child(div().text_color(t.danger).child(error)))
                .when_some(self.cookie_message.clone(), |pane, message| pane.child(div().child(message)))
                .child(div().flex().items_center().gap_3()
                    .child(Button::new("save-cookies").primary().label(if busy { "Please wait…" } else { "Save cookies" }).disabled(busy).on_click(cx.listener(|view, _, _, cx| view.save_cookies(false, cx))))
                    .child(Button::new("remove-cookies").ghost().label("Remove cookies").disabled(busy || (!self.cookie_saved && self.cookie_error.is_none())).on_click(cx.listener(|view, _, _, cx| view.save_cookies(true, cx))))))
            .when(cfg!(target_os = "windows"), |pane| pane
                .child(div().flex().flex_col().gap_2()
                    .child(div().text_size(px(tokens::TITLE)).font_weight(FontWeight::SEMIBOLD).child("Updates"))
                    .child(div().text_color(t.muted).child(format!("{} {}", crate::APP_NAME, env!("CARGO_PKG_VERSION"))))
                    .when(!crate::updater::enabled(), |section| section.child(div().text_color(t.muted).child("Updates are unavailable in this development build.")))
                    .child(div().flex().items_start().child(Button::new("check-updates")
                        .label("Check for Updates…")
                        .disabled(self.stopping.is_some() || !crate::updater::can_check())
                        .on_click(|_, _, cx| cx.dispatch_action(&crate::platform::CheckUpdates))))))
            .into_any_element()
    }
}
