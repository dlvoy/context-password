//! Mapping `keepass_core::Error` to the shared `vault::VaultErrorKind`.
//!
//! Unlike `bw::error` (substring matching over Node.js stderr text),
//! `keepass_core::Error` is a typed Rust enum — this is a plain match, no
//! guessing. Every arm still needs a wildcard: `keepass_core`'s error enums
//! are all `#[non_exhaustive]`, so a future rev bump of the pinned
//! dependency can add a variant without breaking this match (it just falls
//! into `Other` until this file is updated, rather than failing to build).

use keepass_core::crypto::CryptoError;
use keepass_core::error::Error;

use crate::vault::VaultErrorKind;

pub fn classify(err: &Error) -> VaultErrorKind {
    match err {
        Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => VaultErrorKind::NotFound,
        Error::Io(_) => VaultErrorKind::Other,
        // `keepass_core` deliberately does not distinguish "wrong password"
        // from "corrupt file" (an anti-oracle design choice on its part —
        // see `VaultErrorKind::BadPassword`'s doc) — and it also can't tell
        // us a KDBX database needs a key file, since a missing key-file
        // component and a wrong password both fail the same HMAC check.
        // `BadPassword` therefore also covers "needs a key file, which
        // isn't supported" — see `unlock_message` below for the combined
        // wording this produces.
        Error::Crypto(CryptoError::Decrypt | CryptoError::HmacMismatch { .. }) => {
            VaultErrorKind::BadPassword
        }
        Error::Crypto(_) => VaultErrorKind::Other,
        Error::Format(_) | Error::Xml(_) => VaultErrorKind::BadFormat,
        _ => VaultErrorKind::Other,
    }
}

/// A user-facing message for an unlock failure — never the underlying
/// error's raw `Display` text, which is a library-internal message never
/// meant for an end user. Combined wording for `BadPassword` (see
/// `classify`'s doc for why "wrong password" and "needs a key file" can't
/// be told apart), and a short, honest fallback everywhere else.
pub fn unlock_message(kind: VaultErrorKind, err: &Error) -> String {
    match kind {
        VaultErrorKind::NotFound => "KeePass database file not found.".to_string(),
        VaultErrorKind::BadPassword => {
            "Wrong password — or this database also needs a key file, which isn't supported."
                .to_string()
        }
        VaultErrorKind::BadFormat => {
            "That file isn't a readable KeePass (KDBX) database.".to_string()
        }
        _ => format!("Couldn't open the KeePass database: {err}"),
    }
}
