//! Deserialization of `bw list items` output — the raw JSON shape only.
//! The provider-agnostic `Entry` type these raw items become lives in
//! `vault::entry`, shared with the `keepass` backend.
//!
//! No `Debug` derive anywhere in this file (plan §8) — a stray `dbg!` on an
//! item is the realistic way a vault leaks into a log.

use serde::Deserialize;

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
