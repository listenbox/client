use gpui_kit::{App, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        IsIconic, SW_HIDE, SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindowAsync,
    },
};

fn hwnd(window: &Window) -> isize {
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
