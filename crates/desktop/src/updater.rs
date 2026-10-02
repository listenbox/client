//! Native frameworks own scheduling, preferences, prompts and installation.
//! The workspace owns cancellation and draining before permitting termination.
#[cfg(listenbox_updater)]
use crate::workspace::Shutdown;
use crate::workspace::Workspace;
use gpui_kit::{App, Entity};

#[cfg(listenbox_updater)]
mod native {
    use std::sync::OnceLock;
    use tokio::sync::mpsc::UnboundedSender;
    static EVENTS: OnceLock<UnboundedSender<i32>> = OnceLock::new();

    pub extern "C" fn event(kind: i32) {
        // No Rust unwind may cross a native callback boundary.
        let _ = std::panic::catch_unwind(|| {
            if let Some(sender) = EVENTS.get() {
                let _ = sender.send(kind);
            }
        });
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn listenbox_updater_init(
            key: *const i8,
            feed: *const i8,
            event: extern "C" fn(i32),
        ) -> i32;
        fn listenbox_updater_can_check() -> i32;
        fn listenbox_updater_automatic() -> i32;
        fn listenbox_updater_set_automatic(enabled: i32);
        fn listenbox_updater_check();
        fn listenbox_updater_drained();
        fn listenbox_updater_permit_termination();
        fn listenbox_updater_cleanup();
    }

    #[cfg(target_os = "windows")]
    #[link(name = "WinSparkle")]
    unsafe extern "C" {
        fn win_sparkle_set_appcast_url(url: *const i8);
        fn win_sparkle_set_eddsa_public_key(key: *const i8) -> i32;
        fn win_sparkle_set_app_details(company: *const u16, name: *const u16, version: *const u16);
        fn win_sparkle_set_app_build_version(version: *const u16);
        fn win_sparkle_set_registry_path(path: *const i8);
        fn win_sparkle_set_shutdown_request_callback(callback: extern "C" fn());
        fn win_sparkle_set_update_check_interval(seconds: i32);
        fn win_sparkle_init();
        fn win_sparkle_cleanup();
        fn win_sparkle_check_update_with_ui();
        fn win_sparkle_get_automatic_check_for_updates() -> i32;
        fn win_sparkle_set_automatic_check_for_updates(enabled: i32);
    }

    #[cfg(target_os = "windows")]
    extern "C" fn shutdown() {
        event(2);
    }

    pub fn init(sender: UnboundedSender<i32>) -> anyhow::Result<()> {
        EVENTS
            .set(sender)
            .map_err(|_| anyhow::anyhow!("Updater already initialized"))?;
        let key = std::ffi::CString::new(env!("LISTENBOX_UPDATE_PUBLIC_KEY"))?;
        let feed = std::ffi::CString::new(concat!(
            "https://github.com/listenbox/client/releases/latest/download/appcast-",
            env!("LISTENBOX_UPDATE_PLATFORM"),
            ".xml"
        ))?;
        #[cfg(target_os = "macos")]
        anyhow::ensure!(
            unsafe { listenbox_updater_init(key.as_ptr(), feed.as_ptr(), event) } == 1,
            "Invalid bundled updater configuration"
        );
        #[cfg(target_os = "windows")]
        {
            fn wide(value: &str) -> Vec<u16> {
                value.encode_utf16().chain([0]).collect()
            }
            // WinSparkle otherwise permits unsigned updates. Initialization must
            // never happen after a missing, invalid, or rejected key.
            anyhow::ensure!(
                unsafe { win_sparkle_set_eddsa_public_key(key.as_ptr()) } == 1,
                "WinSparkle rejected the update public key"
            );
            unsafe {
                win_sparkle_set_app_details(
                    wide("Listenbox").as_ptr(),
                    wide("Listenbox").as_ptr(),
                    wide(env!("CARGO_PKG_VERSION")).as_ptr(),
                );
                win_sparkle_set_app_build_version(
                    wide(concat!(env!("CARGO_PKG_VERSION"), ".0")).as_ptr(),
                );
                win_sparkle_set_registry_path(c"Software\\Listenbox\\Updates".as_ptr());
                win_sparkle_set_appcast_url(feed.as_ptr());
                win_sparkle_set_shutdown_request_callback(shutdown);
                win_sparkle_set_update_check_interval(86400);
                win_sparkle_init();
            }
        }
        Ok(())
    }
    pub fn can_check() -> bool {
        #[cfg(target_os = "macos")]
        return unsafe { listenbox_updater_can_check() == 1 };
        #[cfg(target_os = "windows")]
        true
    }
    pub fn automatic() -> bool {
        #[cfg(target_os = "macos")]
        return unsafe { listenbox_updater_automatic() == 1 };
        #[cfg(target_os = "windows")]
        unsafe {
            win_sparkle_get_automatic_check_for_updates() == 1
        }
    }
    pub fn toggle_automatic() {
        let enabled = i32::from(!automatic());
        #[cfg(target_os = "macos")]
        unsafe {
            listenbox_updater_set_automatic(enabled);
        }
        #[cfg(target_os = "windows")]
        unsafe {
            win_sparkle_set_automatic_check_for_updates(enabled);
        }
    }
    pub fn check() {
        #[cfg(target_os = "macos")]
        unsafe {
            listenbox_updater_check();
        }
        #[cfg(target_os = "windows")]
        unsafe {
            win_sparkle_check_update_with_ui();
        }
    }
    pub fn drained() {
        #[cfg(target_os = "macos")]
        unsafe {
            listenbox_updater_drained();
        }
    }
    pub fn permit_termination() {
        #[cfg(target_os = "macos")]
        unsafe {
            listenbox_updater_permit_termination();
        }
    }
    pub fn cleanup() {
        #[cfg(target_os = "macos")]
        unsafe {
            listenbox_updater_cleanup();
        }
        #[cfg(target_os = "windows")]
        unsafe {
            win_sparkle_cleanup();
        }
    }
}

pub fn install(view: &Entity<Workspace>, cx: &mut App) {
    #[cfg(listenbox_updater)]
    {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        native::init(sender).expect("Native updater configuration");
        let view = view.clone();
        cx.spawn(async move |cx| {
            while let Some(event) = receiver.recv().await {
                cx.update(|cx| {
                    if event == 2 {
                        crate::platform::show_window(cx);
                        view.update(cx, |view, cx| view.shutdown(Shutdown::Update, cx));
                    } else {
                        crate::platform::refresh_menus(cx);
                    }
                });
            }
        })
        .detach();
    }
    let _ = (view, cx);
}

pub fn enabled() -> bool {
    cfg!(listenbox_updater)
}
pub fn can_check() -> bool {
    #[cfg(listenbox_updater)]
    return native::can_check();
    #[cfg(not(listenbox_updater))]
    false
}
pub fn automatic() -> bool {
    #[cfg(listenbox_updater)]
    return native::automatic();
    #[cfg(not(listenbox_updater))]
    false
}
pub fn check() {
    #[cfg(listenbox_updater)]
    if native::can_check() {
        native::check();
    }
}
pub fn toggle_automatic() {
    #[cfg(listenbox_updater)]
    native::toggle_automatic();
}
pub fn drained(cx: &mut App) {
    #[cfg(listenbox_updater)]
    native::drained();
    #[cfg(not(target_os = "macos"))]
    cx.quit();
    let _ = cx;
}
pub fn cleanup() {
    #[cfg(listenbox_updater)]
    native::cleanup();
}

pub fn quit(cx: &mut App) {
    #[cfg(listenbox_updater)]
    native::permit_termination();
    cx.quit();
}
