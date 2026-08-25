//! The single inbound event type the UI thread drains every frame (see
//! `App::logic`). Tray, hotkey, and `bw` worker events all funnel into this
//! so there is exactly one place that decides what an event means.

use crate::bw::model::Entry;
use crate::secret::Secret;
use crate::win::focus::Target;

// No `derive(Debug)` here: `Bw(BwResult)` carries item data and, per plan
// §8, nothing in that path gets a Debug impl that could print it.
pub enum Msg {
    Tray(TrayCmd),
    Hotkey(Target),
    /// Open the popup with no cursor/target context — used by the tray's
    /// Show item and by the delayed-unlock timer. Falls back to centering
    /// on the primary monitor (see `win::window_style::primary_monitor_center`).
    ShowPopup,
    Bw(BwResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    Quit,
}

/// Commands the UI thread sends to the `bw` worker.
pub enum BwCmd {
    Unlock(Secret),
}

/// Results the `bw` worker sends back. Never carries the session key —
/// that stays inside the worker for the process's lifetime (plan §7).
pub enum BwResult {
    Items { entries: Vec<Entry>, dropped: usize },
    Failed { stage: &'static str, message: String },
}
