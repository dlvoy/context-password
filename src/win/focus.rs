//! Foreground-window capture and self-activation for the popup.
//!
//! See the plan's F1/F2 findings: `global-hotkey`'s handler runs
//! synchronously inside the wndproc during `DispatchMessage` on the UI
//! thread — exactly when `GetForegroundWindow`/`GetCursorPos` must be read,
//! since by the time any window of ours shows, the real foreground window
//! is gone. And self-activation must go through raw `SetForegroundWindow`,
//! never `ViewportCommand::Focus` — that injects a fake Left-Alt keystroke
//! (see F2), which is actively harmful while the user is physically holding
//! a modifier to trigger the hotkey.

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowThreadProcessId, GUITHREADINFO, SetForegroundWindow,
};

/// The window that had focus immediately before the popup was summoned,
/// plus enough context to restore it later and to place the popup now.
/// `hwnd`/`focus_child` are stored as `isize` rather than `HWND` so `Target`
/// is `Send` — it travels through the app's `mpsc` channel.
///
/// `thread_id` and `focus_child` are captured now but only consumed from M3
/// onward (the `AttachThreadInput` fallback and `SetFocus(focus_child)` in
/// the typing sequence's `Activate` phase) — captured here because they can
/// only be read at hotkey time, not retroactively when M3 needs them.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct Target {
    pub hwnd: isize,
    pub thread_id: u32,
    pub focus_child: isize,
    pub cursor: (i32, i32),
}

/// Captures the current foreground window and cursor position. Must be
/// called synchronously from the hotkey handler.
///
/// Returns `None` if the foreground window belongs to this process — a
/// repeat press while the popup already has focus. M2 treats that as a
/// no-op rather than falling back to a remembered target; that refinement
/// ("keep the last good target so a double press doesn't clobber it", per
/// the plan) can wait until it's actually needed.
pub fn capture_target() -> Option<Target> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }

        let mut pid = 0u32;
        let thread_id = GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == GetCurrentProcessId() {
            return None;
        }

        // The focused *child* control of the foreign thread — readable
        // directly via GetGUIThreadInfo, no AttachThreadInput needed.
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let focus_child = if GetGUIThreadInfo(thread_id, &mut info) != 0 {
            info.hwndFocus as isize
        } else {
            0
        };

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        Some(Target {
            hwnd: hwnd as isize,
            thread_id,
            focus_child,
            cursor: (pt.x, pt.y),
        })
    }
}

/// How self-activation succeeded — the instrumentation the plan's M2 calls
/// for: counting how often the plain call is enough versus needing the
/// `AttachThreadInput` fallback, across repeated presses against real apps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationResult {
    /// `SetForegroundWindow` alone succeeded — the expected path, since the
    /// hotkey handler runs with the "received the last input event"
    /// credential `SetForegroundWindow` checks for.
    Direct,
    /// The plain call didn't stick; borrowing the incumbent foreground
    /// thread's input state via `AttachThreadInput` did.
    Attached,
    /// Neither worked. There is no further fallback: never inject a
    /// synthetic key to force activation — that's what
    /// `ViewportCommand::Focus`/`winit::Window::focus_window` do (F2), and
    /// it desyncs the keyboard state the user is mid-chord on.
    Failed,
}

/// Makes `hwnd` the foreground window.
pub fn activate_self(hwnd: HWND) -> ActivationResult {
    unsafe {
        if SetForegroundWindow(hwnd) != 0 && GetForegroundWindow() == hwnd {
            return ActivationResult::Direct;
        }

        let fg = GetForegroundWindow();
        if !fg.is_null() {
            let mut fg_pid = 0u32;
            let fg_tid = GetWindowThreadProcessId(fg, &mut fg_pid);
            let my_tid = GetCurrentThreadId();
            if fg_tid != 0 && fg_tid != my_tid {
                AttachThreadInput(my_tid, fg_tid, 1);
                BringWindowToTop(hwnd);
                SetForegroundWindow(hwnd);
                SetFocus(hwnd);
                AttachThreadInput(my_tid, fg_tid, 0);
                if GetForegroundWindow() == hwnd {
                    return ActivationResult::Attached;
                }
            }
        }

        ActivationResult::Failed
    }
}

/// Best-effort restore of the target's foreground status. M2 scope only —
/// no verification/retry loop or `AttachThreadInput` dance; that lands in
/// M3 alongside the typing sequence (`DrainModifiers`/`VerifyForeground`).
pub fn restore_target(target: &Target) {
    unsafe {
        SetForegroundWindow(target.hwnd as HWND);
    }
}
