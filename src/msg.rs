//! The single inbound event type the UI thread drains every frame (see
//! `App::logic`). Tray, hotkey, and `bw` worker events all funnel into this
//! so there is exactly one place that decides what an event means.

use crate::bw::error::BwErrorKind;
use crate::bw::model::Entry;
use crate::platform::focus::Target;
use crate::secret::Secret;

// No `derive(Debug)` here: `Bw(BwResult)` carries item data and, per plan
// §8, nothing in that path gets a Debug impl that could print it.
pub enum Msg {
    Tray(TrayCmd),
    Hotkey(Target),
    /// Open the popup with no cursor/target context — used by the tray's
    /// Show item and by the delayed-unlock timer. Falls back to centering
    /// on the primary monitor (see `platform::monitor::placement_for`).
    ShowPopup,
    Bw(BwResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    /// Re-run `bw sync` + `bw list items` against the retained session and
    /// replace the cached item list — no re-unlock needed.
    Sync,
    /// Forget the unlocked session: clears the cached items (and their
    /// passwords) from memory, tells the worker to run `bw lock` and drop
    /// its own session key, and — if the popup is open — snaps it back to
    /// the password prompt immediately.
    Lock,
    Settings,
    About,
    Quit,
}

/// Commands the UI thread sends to the `bw` worker.
pub enum BwCmd {
    Unlock(Secret),
    /// Re-runs `bw sync` + `bw list items` using the session the worker
    /// already holds, instead of unlocking again. Fails with `stage: "sync"`
    /// if there's no session (the vault isn't actually unlocked).
    Sync,
    /// Best-effort: runs `bw lock` and drops the worker's own session key
    /// regardless of whether that call succeeds — the user's intent is "we
    /// don't have a vault open anymore," not "only if the CLI agrees."
    Lock,
    /// Fetches the current TOTP code for one item, by id. Requires the
    /// worker to already hold a session (it always should — the item only
    /// ever appears in a list fetched after a successful unlock).
    GetTotp(String),
}

/// Carried on `BwResult::Items` when the list came from the local cache
/// because the `bw sync` that preceded it failed — the graceful-degradation
/// case the self-hosted-vault-behind-an-intermittently-blocked-domain
/// scenario needs: the vault keeps working, but the user is told the data
/// might be stale rather than being left to assume it's current.
pub struct StaleNotice {
    pub reason: BwErrorKind,
    /// From a `bw status` probe's `lastSync` field (RFC3339), when that
    /// probe itself succeeded. `None` if the probe also failed or the vault
    /// has never synced on this machine.
    pub last_sync: Option<String>,
    /// The real `bw` message, for the log and the tooltip — never shown
    /// verbatim in the banner itself, which uses `reason.summary()`.
    pub detail: String,
}

/// Results the `bw` worker sends back. Never carries the session key —
/// that stays inside the worker for the process's lifetime (plan §7).
pub enum BwResult {
    /// `stale: Some(_)` when this list is the local cache from before a
    /// failed sync, rather than confirmed fresh — see `StaleNotice`.
    Items {
        entries: Vec<Entry>,
        dropped: usize,
        stale: Option<StaleNotice>,
    },
    Failed {
        stage: &'static str,
        kind: BwErrorKind,
        message: String,
    },
    /// `bw lock` finished (successfully or not — see `BwCmd::Lock`'s doc).
    Locked,
    Totp(Secret),
}
