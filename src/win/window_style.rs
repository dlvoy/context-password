//! Win32 styling applied directly to the root viewport's HWND — things
//! eframe's cross-platform viewport API cannot express (see the plan's F3
//! finding).

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOREDRAW,
    SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

/// Extracts the raw HWND from anything exposing `raw-window-handle` (eframe's
/// `CreationContext` and `Frame` both do). Returns `None` on a platform where
/// the handle isn't a Win32 window — should never happen in this
/// Windows-only app, but a panic here would be a poor way to find out.
pub fn hwnd_of(source: &impl HasWindowHandle) -> Option<HWND> {
    let handle = source.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
        _ => None,
    }
}

/// Removes the window from Alt+Tab and the taskbar. eframe's
/// `with_taskbar(false)` only calls `ITaskbarList::DeleteTab`, which hides
/// the taskbar button but leaves the window in Alt+Tab — this is the actual
/// fix. Call while the window is still hidden so no `SWP_FRAMECHANGED`
/// repaint is needed.
///
/// Observed in M1: applying this once at startup doesn't reliably survive a
/// later `ViewportCommand::Visible` transition — winit appears to recompute
/// the ex-style on some show/hide paths. Not a problem while the window
/// never becomes visible, but the popup's show sequence (M2) should reapply
/// this immediately before each `Visible(true)` rather than trust the
/// startup call alone.
pub fn make_tool_window(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new_style = (ex_style | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_style);
    }
}

/// Positions and sizes the window in physical pixels while leaving it
/// hidden and unactivated (`SWP_NOACTIVATE`) and without a redraw of the
/// old position (`SWP_NOREDRAW`) — this is frame N of the plan's
/// three-frame show sequence (§2). Pass the output of
/// `win::monitor::placement_for`, which handles which monitor, DPI
/// scaling, and keeping the window on-screen.
pub fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOREDRAW,
        );
    }
}
