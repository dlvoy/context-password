//! Frontmost-app capture, activation, and restoration — the macOS
//! counterpart of `win::focus`. See that module's doc for why capture must
//! happen synchronously, before any window of ours has shown: the Phase 0
//! spike confirmed `global-hotkey`'s macOS backend (Carbon's
//! `RegisterEventHotKey`) delivers its callback synchronously on the main
//! thread, which is exactly what makes that possible here too.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationOptions, NSRunningApplication, NSWorkspace};

use super::super::BlockReason;
use super::{monitor, permissions};

/// `Send` by construction — no Objective-C object pointers, just a pid and
/// plain numbers — so it crosses the app's `mpsc` channel exactly like
/// Windows' `isize`-encoded HWNDs do.
#[derive(Debug, Clone, Copy)]
pub struct Target {
    pid: i32,
    cursor: (i32, i32),
    blocked: Option<BlockReason>,
}

impl Target {
    /// The cursor position at hotkey-press time.
    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    /// Why synthetic keystrokes into this target would be blocked, if they
    /// would be.
    pub fn blocked(&self) -> Option<BlockReason> {
        self.blocked
    }

    /// Whether the target app is still running — it may have quit in the
    /// time it took to pick an item from the popup.
    pub fn still_valid(&self) -> bool {
        NSRunningApplication::runningApplicationWithProcessIdentifier(self.pid)
            .is_some_and(|app| !app.isTerminated())
    }

    /// Whether the target is currently the frontmost app — the poll
    /// `VerifyForeground` (plan §4) uses instead of a blind sleep.
    pub fn is_foreground(&self) -> bool {
        is_foreground(self.pid)
    }
}

/// Captures the current frontmost app and cursor position. Must be called
/// synchronously from the hotkey handler — by the time any window of ours
/// shows, the real frontmost app is gone.
///
/// Returns `None` if the frontmost app is this process itself — a repeat
/// press while the popup already has focus, mirroring
/// `win::focus::capture_target`'s same early return.
pub fn capture_target() -> Option<Target> {
    let ws = NSWorkspace::sharedWorkspace();
    let app = ws.frontmostApplication()?;
    let pid = app.processIdentifier();
    if pid == std::process::id() as i32 {
        return None;
    }

    Some(Target {
        pid,
        cursor: cursor_pos(),
        blocked: permissions::block_reason(),
    })
}

/// The cursor's current position, in the same top-left-origin space every
/// other coordinate in this app uses. `Target::cursor` is a snapshot from
/// hotkey-press time; this is for callers that want the live position
/// instead (e.g. placing the autotype indicator).
pub fn cursor_pos() -> (i32, i32) {
    let primary_height = monitor::primary_height_pt();
    let p = objc2_app_kit::NSEvent::mouseLocation();
    (p.x.round() as i32, (primary_height - p.y).round() as i32)
}

/// Makes `target`'s app frontmost again. Unlike Windows' `activate_self`/
/// `activate_target` split, there's no separate "activate our own window"
/// path here: an `NSPanel` with `.nonactivatingPanel` (see the future
/// `mac_ui::panel`) becomes key without ever making our app active in the
/// first place, so there's nothing to hand back *from* except the app we
/// deactivate here before asking the target to activate.
pub fn activate_target(target: &Target) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // macOS 14+ requires the currently-active app to yield activation
    // before another app can reliably take it.
    NSApplication::sharedApplication(mtm).deactivate();
    if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(target.pid) {
        app.activateWithOptions(NSApplicationActivationOptions::empty());
    }
}

fn is_foreground(pid: i32) -> bool {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .is_some_and(|app| app.processIdentifier() == pid)
}
