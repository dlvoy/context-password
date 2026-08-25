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
    /// on the primary monitor (see `win::monitor::placement_for`).
    ShowPopup,
    Bw(BwResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    /// Forget the unlocked session: clears the cached items (and their
    /// passwords) from memory, tells the worker to run `bw lock` and drop
    /// its own session key, and — if the popup is open — snaps it back to
    /// the password prompt immediately.
    Lock,
    Settings,
    Quit,
}

/// Commands the UI thread sends to the `bw` worker.
pub enum BwCmd {
    Unlock(Secret),
    /// Best-effort: runs `bw lock` and drops the worker's own session key
    /// regardless of whether that call succeeds — the user's intent is "we
    /// don't have a vault open anymore," not "only if the CLI agrees."
    Lock,
    /// Fetches the current TOTP code for one item, by id. Requires the
    /// worker to already hold a session (it always should — the item only
    /// ever appears in a list fetched after a successful unlock).
    GetTotp(String),
}

/// Results the `bw` worker sends back. Never carries the session key —
/// that stays inside the worker for the process's lifetime (plan §7).
pub enum BwResult {
    Items { entries: Vec<Entry>, dropped: usize },
    Failed { stage: &'static str, message: String },
    /// `bw lock` finished (successfully or not — see `BwCmd::Lock`'s doc).
    Locked,
    Totp(Secret),
}
