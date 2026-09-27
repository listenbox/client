use super::*;
use gpui_kit::component::{
    input::Textarea,
    select::Select,
    setting::{SettingGroup, SettingItem, SettingPage, Settings},
};
use listenbox_sync_engine::cookies;

impl Workspace {
    pub(super) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            match self.client.youtube_cookies() {
                Ok(status) => self.cookie_status = status,
                Err(error) => self.cookie_error = Some(error.to_string()),
            }
        } else {
            self.cookie_json
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    fn import_cookies(&mut self, cx: &mut Context<Self>) {
        if self.cookie_busy || self.stopping.is_some() {
            return;
        }
        let Some(browser) = cookies::browsers().into_iter().find(|browser| {
            self.cookie_browser
                .read(cx)
                .selected_value()
                .is_some_and(|name| name.as_ref() == browser.name)
        }) else {
            return;
        };
        let profile = self.cookie_profile.read(cx).value().trim().to_owned();
        let cancel = self.cancel.child_token();
        self.cookie_cancel = Some(cancel.clone());
        self.cookie_busy = true;
        self.cookie_error = None;
        let (client, sender) = (self.client.clone(), self.sender.clone());
        self.tasks.spawn_on(
            async move {
                let result = client
                    .import_browser_cookies(
                        &browser.id,
                        (!profile.is_empty()).then_some(profile.as_str()),
                        cancel,
                    )
                    .await;
                let _ = sender.send(Message::Cookies(result));
            },
            self.runtime.handle(),
        );
        cx.notify();
    }

    pub(super) fn cookies_received(
        &mut self,
        result: anyhow::Result<cookies::Status>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cookie_busy = false;
        self.cookie_cancel = None;
        match result {
            Ok(status) => {
                self.cookie_status = Some(status);
                self.cookie_error = None;
                self.cookie_json
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
            Err(error) => self.cookie_error = Some(error.to_string()),
        }
    }

    pub(super) fn settings(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let t = Tokens::current(cx);
        div().flex().flex_col().gap_4().h_full()
            .child(div().flex().justify_between().items_center()
                .child(div().text_size(px(tokens::PAGE_TITLE)).font_weight(FontWeight::BOLD).child("Settings"))
                .child(Button::new("close-settings").ghost().label("Done").on_click(cx.listener(|view, _, window, cx| {
                    view.settings_open = false;
                    view.cookie_json.update(cx, |input, cx| input.set_value("", window, cx));
                    cx.notify();
                }))))
            .when_some(self.cookie_error.clone(), |pane, error| pane.child(div().flex_shrink_0().text_color(t.danger).child(error)))
            .child(div().flex_1().min_h_0().child(Settings::new("youtube-settings").page(
                SettingPage::new("YouTube").default_open(true).description("Use your YouTube session for playlist imports. Cookies stay on this device and are sent only to YouTube.")
                    .group(SettingGroup::new().title("YouTube session").item(SettingItem::render(move |_, window, cx| {
                        entity.update(cx, |view, cx| view.cookie_controls(window, cx))
                    })))
            ))).text_color(t.ink).into_any_element()
    }

    fn cookie_controls(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::current(cx);
        let disabled = self.cookie_busy || self.stopping.is_some();
        let status = self
            .cookie_status
            .as_ref()
            .map(|status| {
                format!(
                    "{} YouTube cookies saved · {}. Sync again to use this session.",
                    status.count,
                    if status.browser == "json" {
                        "JSON input"
                    } else {
                        status.browser.as_str()
                    }
                )
            })
            .unwrap_or_else(|| "No YouTube session saved.".into());
        div().flex().flex_col().gap_4().w_full()
            .child(div().text_color(t.muted).child(status))
            .when_some(self.cookie_status.clone(), |pane, status| pane.children(status.warnings.into_iter().map(|warning| div().text_color(t.muted).child(warning))))
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Import from a browser"))
            .child(Select::new(&self.cookie_browser).accessibility_label("Browser for YouTube cookies").disabled(disabled).w_full())
            .child(div().flex().flex_col().gap_2().child("Profile (optional)")
                .child(Input::new(&self.cookie_profile).accessibility_id("browser-profile").disabled(disabled))
                .child(div().text_size(px(12.)).text_color(t.muted).child("Profile name, profile directory, or cookie database path. Your browser may ask for keychain access.")))
            .child(div().flex().gap_2()
                .child(Button::new("import-browser-cookies").primary().label(if self.cookie_busy {"Importing…"} else {"Import YouTube cookies"}).disabled(disabled).on_click(cx.listener(|view, _, _, cx| view.import_cookies(cx))))
                .when(self.cookie_busy, |row| row.child(Button::new("cancel-cookie-import").outline().label("Cancel").on_click(cx.listener(|view, _, _, cx| {if let Some(cancel) = &view.cookie_cancel {cancel.cancel();} cx.notify();})))))
            .child(div().border_t_1().border_color(t.divider).pt_4().font_weight(FontWeight::SEMIBOLD).child("Paste cookie JSON"))
            .child(div().text_color(t.muted).child("Choose exactly which cookies to import. This option reads only the JSON below and does not access browser storage."))
            .child(Textarea::new(&self.cookie_json).h(px(120.)).accessibility_id("youtube-cookie-json").disabled(disabled))
            .child(div().text_size(px(12.)).text_color(t.muted).child(r#"[{"name":"SAPISID","value":"…","domain":".youtube.com","path":"/"}]"#))
            .child(div().flex().child(Button::new("import-json-cookies").primary().label("Import JSON").disabled(disabled).on_click(cx.listener(|view, _, window, cx| {
                let json = view.cookie_json.read(cx).value().to_string();
                let result = view.client.import_cookie_json(&json);
                view.cookies_received(result, window, cx);
                cx.notify();
            }))))
            .child(div().flex().child(Button::new("clear-youtube-cookies").ghost().label("Remove saved cookies").disabled(disabled || self.cookie_status.is_none()).on_click(cx.listener(|view, _, _, cx| {
                match view.client.clear_youtube_cookies() {Ok(()) => {view.cookie_status = None; view.cookie_error = None;}, Err(error) => view.cookie_error = Some(error.to_string())}
                cx.notify();
            })))).into_any_element()
    }
}
