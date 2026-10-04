use crate::{
    APP_NAME,
    workspace::{Shutdown, Workspace},
};
use gpui_kit::{App, Entity, Menu, MenuItem, OwnedMenu, Window, actions};

#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[path = "platform/native_menu.rs"]
pub(crate) mod native_menu;

#[cfg(target_os = "windows")]
#[path = "platform/windows.rs"]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::InstallationLock;

actions!(
    listenbox,
    [
        Logout,
        Quit,
        OpenSettings,
        Reload,
        CheckUpdates,
        AutomaticUpdates,
        AutomaticDownloads
    ]
);

/// Query the actual shortcut key while its quit attempt is active. macOS can
/// consume Command-key releases before they reach GPUI's focused view.
pub fn quit_key_state() -> Option<Box<dyn Fn() -> bool>> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplication, NSEventType};
        use objc2_core_graphics::{CGEventFlags, CGEventSource, CGEventSourceStateID};
        let app = NSApplication::sharedApplication(objc2::MainThreadMarker::new()?);
        let event = app.currentEvent()?;
        if event.r#type() != NSEventType::KeyDown {
            return None;
        }
        // Use the event's hardware code, so this follows the active layout.
        let key = event.keyCode();
        Some(Box::new(move || {
            let state = CGEventSourceStateID::CombinedSessionState;
            CGEventSource::key_state(state, key)
                && CGEventSource::flags_state(state).contains(CGEventFlags::MaskCommand)
        }))
    }
    #[cfg(not(target_os = "macos"))]
    None
}

pub fn install(view: &Entity<Workspace>, window: &mut Window, cx: &mut App) {
    crate::updater::install(view, cx);
    install_actions(view, cx);
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let sender = native_menu::install(window, cx);
        status_item::install(cx, sender);
    }
    window.on_window_should_close(cx, |window, cx| {
        hide_window(window, cx);
        false
    });
}

pub fn install_actions(view: &Entity<Workspace>, cx: &mut App) {
    let logout = view.clone();
    cx.on_action(move |_: &Logout, cx| {
        logout.update(cx, |view, cx| view.shutdown(Shutdown::Logout, cx))
    });
    let quit = view.clone();
    cx.on_action(move |_: &Quit, cx| quit.update(cx, |view, cx| view.shutdown(Shutdown::Quit, cx)));
    cx.bind_keys([gpui_kit::KeyBinding::new(
        if cfg!(target_os = "macos") {
            "cmd-,"
        } else {
            "ctrl-,"
        },
        OpenSettings,
        None,
    )]);
    cx.bind_keys([gpui_kit::KeyBinding::new(
        if cfg!(target_os = "macos") {
            "cmd-r"
        } else {
            "ctrl-r"
        },
        Reload,
        None,
    )]);
    cx.on_action(|_: &CheckUpdates, cx| {
        show_window(cx);
        crate::updater::check();
    });
    let automatic_updates = view.clone();
    cx.on_action(move |_: &AutomaticUpdates, cx| {
        crate::updater::toggle_automatic();
        automatic_updates.update(cx, |_, cx| cx.notify());
    });
    let automatic_downloads = view.clone();
    cx.on_action(move |_: &AutomaticDownloads, cx| {
        crate::updater::toggle_automatic_downloads();
        automatic_downloads.update(cx, |_, cx| cx.notify());
    });
    refresh_menus(cx);
}

fn application_menus() -> Vec<OwnedMenu> {
    let items = vec![
        MenuItem::action("Settings…", OpenSettings),
        MenuItem::action("Check for Updates…", CheckUpdates).disabled(!crate::updater::can_check()),
        MenuItem::separator(),
        MenuItem::action("Log out", Logout),
        MenuItem::separator(),
        MenuItem::action(format!("Quit {APP_NAME}"), Quit),
    ];
    vec![
        Menu::new(APP_NAME).items(items),
        Menu::new("View").items(vec![MenuItem::action("Reload", Reload)]),
    ]
    .into_iter()
    .map(|menu| menu.owned())
    .collect()
}

pub fn refresh_menus(cx: &mut App) {
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    native_menu::refresh(cx);
    cx.refresh_windows();
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn application_menu_button() -> impl gpui_kit::IntoElement {
    use gpui_kit::component::{
        Sizable,
        button::{Button, ButtonVariants},
        menu::DropdownMenu,
    };
    Button::new("application-menu")
        .ghost()
        .small()
        .label("Menu")
        .accessibility_label("Application menu")
        .dropdown_menu_with_anchor(gpui_kit::Anchor::BottomLeft, |mut popup, window, cx| {
            for menu in application_menus() {
                popup = popup.submenu(menu.name, window, cx, move |popup, _, _| {
                    menu.items.iter().fold(popup, |popup, item| match item {
                        gpui_kit::OwnedMenuItem::Separator => popup.separator(),
                        gpui_kit::OwnedMenuItem::Action {
                            name,
                            action,
                            checked,
                            disabled,
                            ..
                        } => popup.menu_with_check_and_disabled(
                            name.clone(),
                            *checked,
                            action.boxed_clone(),
                            *disabled,
                        ),
                        _ => unreachable!(
                            "Listenbox application menus contain actions and separators"
                        ),
                    })
                });
            }
            popup
        })
}

pub fn show_window(cx: &mut App) {
    #[cfg(target_os = "macos")]
    macos::show_window();
    #[cfg(target_os = "windows")]
    windows::show_window(cx);
    cx.activate(true);
}

fn hide_window(window: &Window, cx: &mut App) {
    #[cfg(target_os = "macos")]
    macos::hide_window();
    #[cfg(target_os = "windows")]
    windows::hide_window(window, cx);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    cx.hide();
    let _ = (window, cx);
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod status_item {
    use super::*;
    use gpui_kit::Global;
    use tray_icon::{
        Icon, TrayIcon, TrayIconBuilder,
        menu::{Menu as TrayMenu, MenuItem as TrayMenuItem},
    };

    struct StatusItem {
        _icon: TrayIcon,
    }
    impl Global for StatusItem {}

    pub(super) fn install(cx: &mut App, sender: tokio::sync::mpsc::UnboundedSender<muda::MenuId>) {
        let menu = TrayMenu::new();
        let open = TrayMenuItem::with_id(
            native_menu::OPEN_WINDOW,
            format!("Show {APP_NAME}"),
            true,
            None,
        );
        let quit = TrayMenuItem::with_id(native_menu::QUIT, "Quit", true, None);
        menu.append_items(&[&open, &quit]).expect("status menu");
        #[cfg(target_os = "windows")]
        {
            use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
            let sender = sender.clone();
            let open = open.id().clone();
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                ) {
                    let _ = sender.send(open.clone());
                }
            }));
        }
        let _ = sender;
        #[cfg(target_os = "macos")]
        let (pixels, size) = (
            include_bytes!(concat!(env!("OUT_DIR"), "/tray-macos.rgba")).as_slice(),
            36,
        );
        #[cfg(target_os = "windows")]
        let (pixels, size) = (
            include_bytes!(concat!(env!("OUT_DIR"), "/tray-windows.rgba")).as_slice(),
            32,
        );
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(format!("{APP_NAME} — YouTube to podcast sync"))
            .with_icon(Icon::from_rgba(pixels.to_vec(), size, size).expect("status icon pixels"));
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_as_template(true);
        #[cfg(target_os = "windows")]
        let builder = builder.with_menu_on_left_click(false);
        let icon = builder.build().expect("Listenbox status icon");
        cx.set_global(StatusItem { _icon: icon });
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::APP_NAME;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    pub(super) fn hide_window() {
        let app = NSApplication::sharedApplication(
            objc2::MainThreadMarker::new().expect("macOS main thread"),
        );
        for window in app.windows() {
            if window.title().to_string() == APP_NAME {
                window.orderOut(None);
            }
        }
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }

    pub(super) fn show_window() {
        let app = NSApplication::sharedApplication(
            objc2::MainThreadMarker::new().expect("macOS main thread"),
        );
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        for window in app.windows() {
            if window.title().to_string() == APP_NAME {
                window.deminiaturize(None);
                window.makeKeyAndOrderFront(None);
            }
        }
    }
}
