//! The single inbound event type the UI thread drains every frame (see
//! `App::logic`). Tray, hotkey, and vault-worker events all funnel into
//! this so there is exactly one place that decides what an event means.

use crate::platform::focus::Target;
use crate::secret::Secret;
use crate::vault::{Entry, VaultErrorKind};

// No `derive(Debug)` here: `Vault { result, .. }` carries item data and,
// per plan §8, nothing in that path gets a Debug impl that could print it.
pub enum Msg {
    Tray(TrayCmd),
    Hotkey(Target),
    /// Open the popup with no cursor/target context — used by the tray's
    /// Show item and by the delayed-unlock timer. Falls back to centering
    /// on the primary monitor (see `platform::monitor::placement_for`).
    ShowPopup,
    /// A result from the active vault worker (`bw::spawn` or
    /// `keepass::spawn`). `generation` is the worker's own generation
    /// number, stamped at spawn time — once a `VaultHandle` (Phase 4) can
    /// respawn workers on a provider switch, the front-end compares this
    /// against the *live* worker's generation and silently drops a result
    /// from one that's already been torn down, rather than resurrecting a
    /// dead provider's entries into the new provider's UI.
    Vault { generation: u64, result: VaultResult },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Show,
    /// Re-fetch against the retained session/unlocked vault and replace the
    /// cached item list — no re-unlock needed. For Bitwarden this re-runs
    /// `bw sync` + `bw list items`; for KeePass it reloads the `.kdbx` file
    /// from disk (the only way to pick up an edit made by another KeePass
    /// client) and re-derives the key it already holds.
    Sync,
    /// Forget the unlocked session: clears the cached items (and their
    /// passwords) from memory, tells the worker to lock and drop whatever
    /// key material it holds, and — if the popup is open — snaps it back to
    /// the password prompt immediately.
    Lock,
    Settings,
    About,
    Quit,
}

/// Commands the UI thread sends to the active vault worker (`bw::spawn` or
/// `keepass::spawn`).
pub enum VaultCmd {
    Unlock(Secret),
    /// Re-fetches using whatever the worker already holds — see
    /// `TrayCmd::Sync`'s doc for what this means per provider. Fails with
    /// `stage: "sync"` if there's nothing unlocked yet.
    Sync,
    /// Re-enumerates entries from whatever the worker already holds, with
    /// no network round-trip and no re-derivation — the popup-open/refresh
    /// path. For KeePass this is a walk of the resident unlocked vault; for
    /// Bitwarden it's `bw list items` against the retained session,
    /// skipping `bw sync` (unlike `Sync` above).
    List,
    /// Best-effort: drops whatever key material the worker holds and, for
    /// Bitwarden, best-effort-runs `bw lock` too — regardless of whether
    /// that call succeeds, the user's intent is "we don't have a vault open
    /// anymore," not "only if the CLI agrees."
    Lock,
    /// Fetches the current TOTP code for one item, by id. Requires the
    /// worker to already hold a session/unlocked vault (it always should —
    /// the item only ever appears in a list fetched after a successful
    /// unlock).
    GetTotp(String),
}

/// Carried on `VaultResult::Items` when the list came from the local cache
/// because the refresh that preceded it failed — the graceful-degradation
/// case a self-hosted-vault-behind-an-intermittently-blocked-domain
/// scenario needs: the vault keeps working, but the user is told the data
/// might be stale rather than being left to assume it's current.
/// Bitwarden-only in practice — the KeePass backend has no partial-failure
/// state (either the resident vault answers, or the whole operation fails),
/// so its results always carry `stale: None`.
pub struct StaleNotice {
    pub reason: VaultErrorKind,
    /// From a `bw status` probe's `lastSync` field (RFC3339), when that
    /// probe itself succeeded. `None` if the probe also failed or the vault
    /// has never synced on this machine.
    pub last_sync: Option<String>,
    /// The real underlying message, for the log and the tooltip — never
    /// shown verbatim in the banner itself, which uses `reason.summary()`.
    pub detail: String,
}

/// Results a vault worker sends back. Never carries key material — that
/// stays inside the worker for the process's lifetime (plan §7).
pub enum VaultResult {
    /// `stale: Some(_)` when this list is the local cache from before a
    /// failed refresh, rather than confirmed fresh — see `StaleNotice`.
    Items {
        entries: Vec<Entry>,
        dropped: usize,
        stale: Option<StaleNotice>,
    },
    Failed {
        stage: &'static str,
        kind: VaultErrorKind,
        message: String,
    },
    Locked,
    Totp(Secret),
}
