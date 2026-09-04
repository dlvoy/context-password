//! The provider-agnostic popup-cache entry, and the sort order both
//! providers apply to it.
//!
//! No `Debug` derive anywhere in this file (plan §8, guarded by
//! `secret::tests::no_debug_derive_on_types_holding_passwords`) — a stray
//! `dbg!` on an item is the realistic way a vault leaks into a log.
//! `Entry::log_line` below is the one sanctioned way to describe an entry in
//! a log message.

use crate::secret::Secret;

/// One entry in the in-memory popup cache — built by either `bw::filter` or
/// `keepass::filter` from that provider's own raw shape, and read only by
/// the two front-ends and `ui::popup` from here on. `id` is an opaque,
/// provider-owned string: a Bitwarden item UUID for the `bw` backend, a
/// KeePass `EntryId` in string form for the `keepass` backend. Nothing
/// outside the owning provider module ever parses it — front-ends only ever
/// hand it back verbatim via `VaultCmd::GetTotp`.
pub struct Entry {
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    /// `None` when the item's tag (a `<prefix>/ORD` uri, or the
    /// `context-password` custom field) is present but the ORD segment is
    /// missing or unparseable — see `vault::tag`. Such entries still show up
    /// in the popup (sorted last), rather than vanishing, so a typo is
    /// visible and self-correcting.
    pub ord: Option<i64>,
    pub password: Secret,
    /// Whether the item has a TOTP source configured — checked before ever
    /// attempting to fetch/compute a code, so asking for one on an item that
    /// doesn't have it is an immediate inline error, not a doomed round
    /// trip.
    pub has_totp: bool,
}

impl Entry {
    /// A redacted line for logging — never the password, and this is the
    /// only place item data should reach stderr.
    pub fn log_line(&self) -> String {
        match self.ord {
            Some(ord) => format!("{ord}: {}", self.name),
            None => format!("?: {} (missing/invalid ORD)", self.name),
        }
    }
}

/// Sorts entries by ord (missing/unparseable ords last, not as `0` — that
/// would silently promote a typo to the top and shuffle every deliberately
/// ordered entry), then by name, case-insensitively. Shared by both
/// providers' `build_entries` so the comparator can't drift between them.
pub fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        (a.ord.is_none(), a.ord.unwrap_or(0), a.name.to_lowercase()).cmp(&(
            b.ord.is_none(),
            b.ord.unwrap_or(0),
            b.name.to_lowercase(),
        ))
    });
}
