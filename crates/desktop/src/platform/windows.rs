use gpui_kit::{App, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindowAsync,
    },
};

/// Held until runtime draining and updater cleanup complete. Setup never kills
/// the application; it refuses replacement while this mutex exists.
pub struct InstallationLock(windows_sys::Win32::Foundation::HANDLE);

impl InstallationLock {
    pub fn new() -> anyhow::Result<Self> {
        let name: Vec<u16> = "Local\\ListenboxDesktop"
            .encode_utf16()
            .chain([0])
            .collect();
        let handle = unsafe {
            windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, name.as_ptr())
        };
        anyhow::ensure!(!handle.is_null(), "Cannot create installation mutex");
        Ok(Self(handle))
    }
}

impl Drop for InstallationLock {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub(super) fn hwnd(window: &Window) -> isize {
    let handle = HasWindowHandle::window_handle(window).expect("Listenbox window handle");
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        unreachable!("Listenbox must have a Win32 window on Windows");
    };
    handle.hwnd.get()
}

pub(super) fn hide_window(window: &Window, cx: &App) {
    let hwnd = hwnd(window);
    // Native visibility changes can re-enter GPUI through WM_SHOWWINDOW. Run
    // after its current App borrow ends, without destroying the workspace.
    cx.foreground_executor()
        .spawn(async move {
            // SAFETY: the window is retained by GPUI; closing only hides it.
            unsafe { ShowWindowAsync(hwnd as HWND, SW_HIDE) };
        })
        .detach();
}

pub(super) fn show_window(cx: &mut App) {
    for handle in cx.windows() {
        let _ = handle.update(cx, |_, window, cx| {
            let hwnd = hwnd(window);
            cx.foreground_executor()
                .spawn(async move {
                    // SAFETY: this handle belongs to the retained GPUI window.
                    unsafe {
                        let hwnd = hwnd as HWND;
                        let state = if IsIconic(hwnd) != 0 {
                            SW_RESTORE
                        } else {
                            SW_SHOW
                        };
                        ShowWindowAsync(hwnd, state);
                        SetForegroundWindow(hwnd);
                    }
                })
                .detach();
        });
    }
}
