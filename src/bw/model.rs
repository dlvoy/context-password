//! Deserialization of `bw list items` output, and the in-memory cache entry
//! it becomes.
//!
//! No `Debug` derive anywhere in this file (plan §8) — a stray `dbg!` on an
//! item is the realistic way a vault leaks into a log. `Entry::log_line`
//! below is the one sanctioned way to describe an entry in a log message.

use serde::Deserialize;

use crate::secret::Secret;

#[derive(Deserialize)]
pub struct RawItem {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: u8,
    #[serde(default)]
    pub login: Option<RawLogin>,
}

#[derive(Deserialize)]
pub struct RawLogin {
    #[serde(default)]
    pub uris: Vec<RawUri>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// The raw TOTP seed, if the item has one configured. Never read
    /// directly — only its presence matters (`Entry::has_totp`); the actual
    /// current code is fetched from `bw get totp` at time of use, never
    /// computed from this locally (see `bw::cmd::get_totp`'s doc).
    #[serde(default)]
    pub totp: Option<String>,
}

#[derive(Deserialize)]
pub struct RawUri {
    pub uri: String,
    /// Unused for now — kept because it's part of the real JSON shape
    /// (`bw` sometimes omits it entirely; see `bw::filter::ord_of`'s
    /// handling), not because anything reads it yet.
    #[serde(default, rename = "match")]
    #[allow(dead_code)]
    pub match_type: Option<u8>,
}

/// One entry in the in-memory popup cache.
pub struct Entry {
    pub id: String,
    pub name: String,
    pub username: Option<String>,
    /// `None` when the item's `app://.../ORD` uri is present but the
    /// trailing segment is missing or unparseable — see
    /// `bw::filter::ord_of`. Such entries still show up in the popup
    /// (sorted last), rather than vanishing, so a typo is visible and
    /// self-correcting.
    pub ord: Option<i64>,
    pub password: Secret,
    /// Whether the item has a TOTP seed configured — checked before ever
    /// attempting `bw get totp`, so asking for a code on an item that
    /// doesn't have one is an immediate inline error, not a doomed
    /// subprocess call.
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
