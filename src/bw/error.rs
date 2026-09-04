//! Classifying a failed `bw` call from its stderr text into the shared
//! `vault::VaultErrorKind`.
//!
//! The CLI has no stable error codes — this is substring matching against
//! text a future `bw` release could rephrase. Every classification stays
//! paired with the original message (never discarded) specifically so a
//! misclassification degrades to "shows the real text" rather than "shows
//! a confidently wrong category."
//!
//! Strings marked "measured" were captured directly from this machine's
//! `bw` 2026.8.0 against an isolated, unauthenticated data dir
//! (`BITWARDENCLI_APPDATA_DIR`) and a deliberately unreachable server —
//! see the port plan's notes. Strings marked "expected, unverified" could
//! not be produced without a real logged-in vault and/or an actual
//! Zscaler-blocked link; they're included so classification degrades
//! sensibly if they occur, not because they were observed.
//!
//! This is Bitwarden-specific (Node.js stderr text) and has no business
//! being shared with the `keepass` backend, which maps a typed
//! `keepass_core::Error` instead — see `keepass::error`.

use crate::vault::VaultErrorKind;

/// `timed_out` takes priority over the stderr text, which may be empty or
/// stale (from a partially-drained pipe) when the process was killed.
pub fn classify(stderr: &str, timed_out: bool) -> VaultErrorKind {
    if timed_out {
        return VaultErrorKind::Timeout;
    }

    let lower = stderr.to_ascii_lowercase();

    // Checked before the generic network markers below: a `FetchError`
    // wrapper line is common to both shapes ("...FetchError: request to
    // .../api/config failed, reason: <specific reason>"), so the specific
    // TLS reason must win when both are present in the same message.
    const TLS_MARKERS: &[&str] = &[
        "unable to verify the first certificate",
        "unable to get local issuer certificate",
        "self_signed_cert_in_chain",
        "unable_to_get_issuer_cert_locally",
        "certificate has expired",
        "cert_has_expired",
        // Measured: this machine's Zscaler MITM proxy presents its own
        // certificate (for its block-notice domain) instead of the vault's
        // — Node reports that as a hostname/altname mismatch, not a plain
        // connection failure.
        "err_tls_cert_altname_invalid",
        "does not match certificate's altnames",
        "depth zero self signed certificate",
    ];
    if TLS_MARKERS.iter().any(|m| lower.contains(m)) {
        return VaultErrorKind::Tls;
    }

    // Checked before the network markers below: this crash can print its
    // own "Unable to fetch ServerConfig"/`FetchError`-shaped noise as a
    // side effect (the corrupted record surfaces while `bw` is also trying
    // and failing to refresh other things), but the real cause is the local
    // cache, not the network — see `VaultErrorKind::CorruptedCache`'s doc.
    if lower.contains("invalid time value") {
        return VaultErrorKind::CorruptedCache;
    }

    const NETWORK_MARKERS: &[&str] = &[
        "enotfound",
        "econnrefused",
        "econnreset",
        "etimedout",
        "eai_again",
        "getaddrinfo",
        "fetcherror",
        "fetch failed",
        "socket hang up",
        "network request failed",
        "proxy",
    ];
    if NETWORK_MARKERS.iter().any(|m| lower.contains(m)) {
        return VaultErrorKind::Network;
    }

    if lower.contains("you are not logged in") {
        return VaultErrorKind::NotLoggedIn;
    }
    if lower.contains("vault is locked") {
        return VaultErrorKind::Locked;
    }
    if lower.contains("invalid master password")
        || lower.contains("username or password is incorrect")
    {
        return VaultErrorKind::BadPassword;
    }

    VaultErrorKind::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_wins_regardless_of_stderr_content() {
        assert_eq!(classify("You are not logged in.", true), VaultErrorKind::Timeout);
        assert_eq!(classify("", true), VaultErrorKind::Timeout);
    }

    #[test]
    fn measured_corrupted_local_cache_classifies_distinctly_from_network() {
        // Captured verbatim (trimmed) from this machine: a `bw sync` that
        // crashed mid-write (via the TLS fallback that has since been
        // removed from `cmd::sync`) left a cached `Policy` record with an
        // invalid date, which then crashed a *plain* `bw unlock` too, even
        // though its own stderr also mentions ServerConfig/fetch noise as a
        // side effect — the cache corruption must still win.
        let stderr = "Error getting vault timeout: RangeError: Invalid time value\n\
                       Unable to fetch ServerConfig from https://bitwarden.dzienia.pl/api Error\n  \
                       message: 'no elements in sequence'";
        assert_eq!(classify(stderr, false), VaultErrorKind::CorruptedCache);
    }

    #[test]
    fn measured_not_logged_in() {
        assert_eq!(classify("You are not logged in.", false), VaultErrorKind::NotLoggedIn);
    }

    #[test]
    fn measured_unreachable_host() {
        // Captured verbatim (trimmed) from this machine against a
        // deliberately unresolvable hostname.
        let stderr = "Unable to fetch ServerConfig from https://vault.invalid.example.test/api \
                       FetchError: request to https://vault.invalid.example.test/api/config \
                       failed, reason: getaddrinfo ENOTFOUND vault.invalid.example.test";
        assert_eq!(classify(stderr, false), VaultErrorKind::Network);
    }

    #[test]
    fn tls_interception_shapes_classify_as_tls() {
        for marker in [
            "unable to verify the first certificate",
            "SELF_SIGNED_CERT_IN_CHAIN",
            "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
        ] {
            assert_eq!(classify(marker, false), VaultErrorKind::Tls, "marker: {marker}");
        }
    }

    #[test]
    fn measured_zscaler_cert_altname_mismatch_classifies_as_tls() {
        // Captured verbatim from this machine: Zscaler intercepts the
        // vault's domain and presents its own block-notice certificate
        // instead, which Node reports as an altname mismatch rather than a
        // connection failure.
        let stderr = "Unable to fetch ServerConfig from https://bitwarden.dzienia.pl/api \
                       FetchError: request to https://bitwarden.dzienia.pl/api/config failed, \
                       reason: Hostname/IP does not match certificate's altnames: Host: \
                       bitwarden.dzienia.pl. is not in the cert's altnames: DNS:dnsblocknotice.\
                       capgemini.com, DNS:www.dnsblocknotice.capgemini.com\n  type: 'system',\n  \
                       errno: 'ERR_TLS_CERT_ALTNAME_INVALID',\n  code: 'ERR_TLS_CERT_ALTNAME_INVALID'";
        assert_eq!(classify(stderr, false), VaultErrorKind::Tls);
    }

    #[test]
    fn connection_level_shapes_classify_as_network() {
        for marker in ["ECONNREFUSED", "ETIMEDOUT", "ECONNRESET", "EAI_AGAIN"] {
            assert_eq!(classify(marker, false), VaultErrorKind::Network, "marker: {marker}");
        }
    }

    #[test]
    fn bad_password_shapes() {
        assert_eq!(classify("Invalid master password.", false), VaultErrorKind::BadPassword);
        assert_eq!(
            classify("Username or password is incorrect. Try again.", false),
            VaultErrorKind::BadPassword
        );
    }

    #[test]
    fn locked_shape() {
        assert_eq!(classify("Vault is locked.", false), VaultErrorKind::Locked);
    }

    #[test]
    fn unrecognised_text_falls_back_to_other_rather_than_guessing() {
        assert_eq!(classify("Something bw has never said before.", false), VaultErrorKind::Other);
    }

    #[test]
    fn empty_stderr_is_other_not_a_panic() {
        assert_eq!(classify("", false), VaultErrorKind::Other);
    }
}
