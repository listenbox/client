use crate::workspace::{Shutdown, Workspace};
use gpui_kit::{App, Entity, Menu, MenuItem, Window, actions};

#[cfg(target_os = "windows")]
#[path = "platform/windows.rs"]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::InstallationLock;

actions!(
    listenbox,
    [Logout, Quit, OpenSettings, CheckUpdates, AutomaticUpdates]
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
    status_item::install(view, cx);
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
    cx.on_action(|_: &CheckUpdates, cx| {
        show_window(cx);
        crate::updater::check();
    });
    cx.on_action(|_: &AutomaticUpdates, cx| {
        crate::updater::toggle_automatic();
        refresh_menus(cx);
    });
    refresh_menus(cx);
}

pub fn refresh_menus(cx: &mut App) {
    let mut items = vec![
        MenuItem::action("Settings…", OpenSettings),
        MenuItem::action("Check for Updates…", CheckUpdates).disabled(!crate::updater::can_check()),
    ];
    if crate::updater::enabled() {
        items.push(
            MenuItem::action("Automatically Check for Updates", AutomaticUpdates)
                .checked(crate::updater::automatic()),
        );
    }
    items.extend([
        MenuItem::separator(),
        MenuItem::action("Log out", Logout),
        MenuItem::separator(),
        MenuItem::action("Quit Listenbox", Quit),
    ]);
    cx.set_menus(vec![Menu::new("Listenbox").items(items)]);
    #[cfg(target_os = "macos")]
    macos::configure_app_menu();
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
        menu::{Menu as TrayMenu, MenuEvent, MenuItem as TrayMenuItem},
    };

    struct StatusItem {
        _icon: TrayIcon,
    }
    impl Global for StatusItem {}

    pub(super) fn install(view: &Entity<Workspace>, cx: &mut App) {
        let menu = TrayMenu::new();
        let open = TrayMenuItem::new("Open Listenbox", true, None);
        let quit = TrayMenuItem::new("Quit Listenbox", true, None);
        menu.append_items(&[&open, &quit]).expect("status menu");
        let (open, quit) = (open.id().clone(), quit.id().clone());
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        #[cfg(target_os = "windows")]
        {
            use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
            let sender = sender.clone();
            let open = open.clone();
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
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = sender.send(event.id);
        }));
        let view = view.clone();
        cx.spawn(async move |cx| {
            while let Some(id) = receiver.recv().await {
                cx.update(|cx| {
                    if id == open {
                        super::show_window(cx);
                    } else if id == quit {
                        view.update(cx, |view, cx| view.shutdown(Shutdown::Quit, cx));
                    }
                });
            }
        })
        .detach();
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
            .with_tooltip("Listenbox — YouTube to podcast sync")
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
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    pub(super) fn configure_app_menu() {
        let Some(main_thread) = objc2::MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(main_thread);
        if let Some(menu) = app
            .mainMenu()
            .and_then(|menu| menu.itemAtIndex(0))
            .and_then(|item| item.submenu())
        {
            // GPUI's native validation only checks whether an action has a
            // handler. Preserve our explicit enabled states, including Sparkle's
            // canCheckForUpdates and development builds without an updater.
            menu.setAutoenablesItems(false);
        }
    }

    pub(super) fn hide_window() {
        let app = NSApplication::sharedApplication(
            objc2::MainThreadMarker::new().expect("macOS main thread"),
        );
        for window in app.windows() {
            if window.title().to_string() == "Listenbox" {
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
            if window.title().to_string() == "Listenbox" {
                window.deminiaturize(None);
                window.makeKeyAndOrderFront(None);
            }
        }
    }
}
