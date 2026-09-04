//! The shared failure classification both providers report through.
//!
//! Every classification stays paired with the original message (never
//! discarded, but never shown to the user either — see [`VaultErrorKind::
//! summary`]) so a misclassification degrades to "shows a generic sentence"
//! rather than "shows a confidently wrong category". `bw::error::classify`
//! (substring matching over `bw`'s Node.js stderr text) and
//! `keepass::error`'s mapping (a typed match over `keepass_core::Error`)
//! both produce this same enum — see each module for how.

use super::Provider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultErrorKind {
    /// "bw executable not found" (from `bw::exe::resolve`) or "KeePass
    /// database file not found" (from a `keepass::Kdbx::open` `io::Error`).
    NotFound,
    /// Measured: `bw sync`/`list items`/`unlock` all say exactly
    /// "You are not logged in." when no account is logged in on this
    /// machine at all (`bw login` was never run). Bitwarden-only — KeePass
    /// has no login step.
    NotLoggedIn,
    /// Expected, unverified: "Vault is locked."-shaped messages when a
    /// session exists but isn't the one just presented. Bitwarden-only.
    Locked,
    /// `bw unlock`'s rejection of a wrong master password, or
    /// `keepass_core`'s `CryptoError::Decrypt`/`HmacMismatch` — deliberately
    /// also covers "this KDBX database needs a key file, which isn't
    /// supported", since `keepass-core` cannot distinguish the two (an
    /// anti-oracle design choice on its part, not a gap on ours).
    BadPassword,
    /// The subprocess didn't exit within its budget (`bw::run`). Bitwarden
    /// backend only — the KeePass backend never shells out or hits the
    /// network, so nothing on that path can time out this way.
    Timeout,
    /// Measured (a real, if synthetic, unreachable-host case): Node's
    /// `FetchError`/`getaddrinfo ENOTFOUND` shape, plus the connection-level
    /// variants a corporate proxy (Zscaler, etc.) is expected but unverified
    /// to produce when it blackholes the connection outright. Bitwarden-only.
    Network,
    /// Measured: a corporate TLS-intercepting proxy (this machine's Zscaler)
    /// presents its own certificate for the vault's domain instead of
    /// blackholing it — Node reports this as a certificate mismatch
    /// (`ERR_TLS_CERT_ALTNAME_INVALID` and siblings), not a connection
    /// failure. Kept distinct from `Network` because it alone is worth an
    /// automatic retry with certificate verification disabled (see
    /// `bw::cmd::run_with_tls_fallback`) — retrying a genuine DNS/connection
    /// failure that way would just waste a second timeout for no benefit.
    /// Bitwarden-only.
    Tls,
    /// Measured: the local vault cache (`data.json`) holds a record with an
    /// invalid cached date (observed: a `Policy.revisionDate` of `null`/
    /// unparseable), which crashes `bw` — an uncaught `RangeError: Invalid
    /// time value` — the moment it needs that record, including during a
    /// plain `unlock`. `bw`'s own repair path (`bw logout` + `bw login`
    /// again, from a network that can actually reach the server) is the
    /// only fix; nothing this app does can repair `bw`'s on-disk cache.
    /// Bitwarden-only.
    CorruptedCache,
    /// The file isn't a readable KeePass database at all — `keepass_core::
    /// Error::{Format, Xml}`. KeePass backend only; kept distinct from
    /// `BadPassword` because it means something different ("wrong kind of
    /// file", not "wrong password/needs a keyfile").
    BadFormat,
    /// Recognised as *a* failure, but not classified beyond that. Still
    /// carries the real message everywhere `VaultErrorKind` does.
    Other,
}

impl VaultErrorKind {
    /// A short, user-facing sentence — never echoes the underlying error's
    /// raw text, which for Bitwarden can be a full Node stack trace
    /// (leaking the vault's server URL) and for KeePass a library-internal
    /// `Display` string. `provider` only changes the wording for the couple
    /// of variants that mean something provider-specific.
    pub fn summary(&self, provider: Provider) -> &'static str {
        match self {
            Self::NotFound => match provider {
                Provider::Bitwarden => "Bitwarden CLI not found",
                Provider::KeePass => "KeePass database file not found",
            },
            Self::NotLoggedIn => "not logged in",
            Self::Locked => "vault is locked",
            Self::BadPassword => "wrong master password",
            Self::Timeout => "timed out",
            Self::Network => "can't reach the server",
            Self::Tls => "server certificate doesn't match (proxy?)",
            Self::CorruptedCache => "local vault cache is corrupted — try `bw logout` + `bw login`",
            Self::BadFormat => "not a readable KeePass database",
            Self::Other => "failed",
        }
    }
}
