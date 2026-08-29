//! Foreground-window capture, activation, and restoration.
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
    BringWindowToTop, GUITHREADINFO, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowThreadProcessId, IsIconic, IsWindow, SW_RESTORE, SetForegroundWindow, ShowWindow,
};

use super::integrity;

/// The window that had focus immediately before the popup was summoned,
/// plus enough context to restore it later and to place the popup now.
/// `hwnd`/`focus_child` are stored as `isize` rather than `HWND` so `Target`
/// is `Send` — it travels through the app's `mpsc` channel.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pub hwnd: isize,
    pub thread_id: u32,
    pub focus_child: isize,
    pub cursor: (i32, i32),
    /// Whether this window's process runs at a higher integrity level than
    /// ours (plan §5/F11) — if so, `SendInput` targeting it will be
    /// silently dropped by UIPI, so delivery must refuse to type rather
    /// than fail invisibly.
    pub elevated_beyond_us: bool,
}

/// Captures the current foreground window and cursor position. Must be
/// called synchronously from the hotkey handler. `our_integrity_rid` is
/// read once at startup (`integrity::our_integrity_rid`) and passed in
/// rather than re-read here, since it never changes for the process's
/// lifetime.
///
/// Returns `None` if the foreground window belongs to this process — a
/// repeat press while the popup already has focus. This is treated as a
/// no-op rather than falling back to a remembered target; that refinement
/// ("keep the last good target so a double press doesn't clobber it", per
/// the plan) can wait until it's actually needed.
pub fn capture_target(our_integrity_rid: u32) -> Option<Target> {
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

        Some(Target {
            hwnd: hwnd as isize,
            thread_id,
            focus_child,
            cursor: cursor_pos(),
            elevated_beyond_us: integrity::target_is_higher(pid, our_integrity_rid),
        })
    }
}

/// The cursor's current position. `Target::cursor` is a snapshot from
/// hotkey-press time; this is for callers that want the live position
/// instead — e.g. placing the autotype indicator, since the user may have
/// moved the mouse between pressing the hotkey and picking an item.
pub fn cursor_pos() -> (i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt) };
    (pt.x, pt.y)
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

/// Makes `target.hwnd` the foreground window again and restores its
/// previously focused child control. Used both for the Esc/blur dismiss
/// path and as the `Activate` phase of the typing delivery sequence (plan
/// §4) — unlike `activate_self`, this also un-minimizes the window and
/// re-applies the child that had focus, since the goal is to put the user
/// back exactly where they were, not just to raise a window.
pub fn activate_target(target: &Target) {
    unsafe {
        let hwnd = target.hwnd as HWND;
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd) == 0 || GetForegroundWindow() != hwnd {
            let my_tid = GetCurrentThreadId();
            if target.thread_id != 0 && target.thread_id != my_tid {
                AttachThreadInput(my_tid, target.thread_id, 1);
                SetForegroundWindow(hwnd);
                AttachThreadInput(my_tid, target.thread_id, 0);
            }
        }
        if target.focus_child != 0 {
            SetFocus(target.focus_child as HWND);
        }
    }
}

/// Whether `hwnd` still refers to a live window — a target may have closed
/// in the time it took to pick an item from the popup.
pub fn is_window(hwnd: isize) -> bool {
    unsafe { IsWindow(hwnd as HWND) != 0 }
}

/// Whether `hwnd` is currently the foreground window — the poll
/// `VerifyForeground` (plan §4) uses instead of a blind sleep.
pub fn is_foreground(hwnd: isize) -> bool {
    unsafe { GetForegroundWindow() as isize == hwnd }
}
