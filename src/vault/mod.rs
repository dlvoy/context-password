//! The provider-agnostic layer shared by `bw` (Bitwarden) and `keepass`
//! (KeePass 2 / KDBX) — the credential entry type, the tag grammar that
//! decides which vault items belong to this app's popup, and the shared
//! failure classification. See `controller::VaultState` for the
//! popup-visible unlock/lock state machine this layer feeds.

pub mod entry;
pub mod error;
pub mod handle;
pub mod tag;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

pub use entry::{sort_entries, Entry};
pub use error::VaultErrorKind;
pub use handle::VaultHandle;

/// Which credential backend the app talks to. Persisted on `Config`
/// (`config::Config::provider`); defaults to `Bitwarden` so a config file
/// written before this field existed keeps working unchanged.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    #[default]
    Bitwarden,
    // `rename_all = "snake_case"` alone would write this variant as
    // "kee_pass" — an ugly, permanent wart in every user's config.toml.
    // The explicit rename is required, not optional; see
    // `config::tests::provider_serializes_as_expected_toml_strings`.
    #[serde(rename = "keepass")]
    KeePass,
}

/// Wakes whatever event loop owns the UI thread after a result has been
/// pushed onto the shared `Msg` channel — `ctx.request_repaint()` on
/// Windows/eframe, a no-op on macOS/AppKit (a repeating `NSTimer` drains the
/// channel unconditionally there instead). Every other event source in this
/// app (tray, hotkey, the delayed-unlock timer) follows the same "send,
/// then wake" pattern; this is what lets both `bw::spawn` and
/// `keepass::spawn` stay UI-toolkit-agnostic.
pub type Waker = Arc<dyn Fn() + Send + Sync>;
