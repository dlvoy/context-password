//! Classifying a failed `bw` call from its stderr text.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BwErrorKind {
    /// "bw executable not found" — from `exe::resolve`, never from `bw`
    /// itself.
    NotFound,
    /// Measured: `bw sync`/`list items`/`unlock` all say exactly
    /// "You are not logged in." when no account is logged in on this
    /// machine at all (`bw login` was never run).
    NotLoggedIn,
    /// Expected, unverified: "Vault is locked."-shaped messages when a
    /// session exists but isn't the one just presented.
    Locked,
    /// Expected, unverified: `bw unlock`'s rejection of a wrong master
    /// password.
    BadPassword,
    /// The subprocess didn't exit within its budget — see `bw::run`.
    Timeout,
    /// Measured (a real, if synthetic, unreachable-host case): Node's
    /// `FetchError`/`getaddrinfo ENOTFOUND` shape, plus the connection-level
    /// and TLS-interception variants a corporate proxy (Zscaler, etc.) is
    /// expected but unverified to produce — it MITMs TLS rather than
    /// blackholing the connection, so a certificate error is at least as
    /// likely as a DNS one.
    Network,
    /// Recognised as *a* failure, but not classified beyond that. Still
    /// carries the real message everywhere `BwErrorKind` does.
    Other,
}

impl BwErrorKind {
    /// A short, user-facing sentence — never echoes raw `bw` stderr, which
    /// can contain a full Node stack trace and the vault's server URL.
    pub fn summary(&self) -> &'static str {
        match self {
            Self::NotFound => "Bitwarden CLI not found",
            Self::NotLoggedIn => "not logged in",
            Self::Locked => "vault is locked",
            Self::BadPassword => "wrong master password",
            Self::Timeout => "timed out",
            Self::Network => "can't reach the server",
            Self::Other => "failed",
        }
    }
}

/// `timed_out` takes priority over the stderr text, which may be empty or
/// stale (from a partially-drained pipe) when the process was killed.
pub fn classify(stderr: &str, timed_out: bool) -> BwErrorKind {
    if timed_out {
        return BwErrorKind::Timeout;
    }

    let lower = stderr.to_ascii_lowercase();

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
        "unable to verify the first certificate",
        "self_signed_cert_in_chain",
        "unable_to_get_issuer_cert_locally",
        "certificate has expired",
        "cert_has_expired",
        "proxy",
    ];
    if NETWORK_MARKERS.iter().any(|m| lower.contains(m)) {
        return BwErrorKind::Network;
    }

    if lower.contains("you are not logged in") {
        return BwErrorKind::NotLoggedIn;
    }
    if lower.contains("vault is locked") {
        return BwErrorKind::Locked;
    }
    if lower.contains("invalid master password")
        || lower.contains("username or password is incorrect")
    {
        return BwErrorKind::BadPassword;
    }

    BwErrorKind::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_wins_regardless_of_stderr_content() {
        assert_eq!(classify("You are not logged in.", true), BwErrorKind::Timeout);
        assert_eq!(classify("", true), BwErrorKind::Timeout);
    }

    #[test]
    fn measured_not_logged_in() {
        assert_eq!(classify("You are not logged in.", false), BwErrorKind::NotLoggedIn);
    }

    #[test]
    fn measured_unreachable_host() {
        // Captured verbatim (trimmed) from this machine against a
        // deliberately unresolvable hostname.
        let stderr = "Unable to fetch ServerConfig from https://vault.invalid.example.test/api \
                       FetchError: request to https://vault.invalid.example.test/api/config \
                       failed, reason: getaddrinfo ENOTFOUND vault.invalid.example.test";
        assert_eq!(classify(stderr, false), BwErrorKind::Network);
    }

    #[test]
    fn tls_interception_shapes_classify_as_network() {
        for marker in [
            "unable to verify the first certificate",
            "SELF_SIGNED_CERT_IN_CHAIN",
            "UNABLE_TO_GET_ISSUER_CERT_LOCALLY",
        ] {
            assert_eq!(classify(marker, false), BwErrorKind::Network, "marker: {marker}");
        }
    }

    #[test]
    fn connection_level_shapes_classify_as_network() {
        for marker in ["ECONNREFUSED", "ETIMEDOUT", "ECONNRESET", "EAI_AGAIN"] {
            assert_eq!(classify(marker, false), BwErrorKind::Network, "marker: {marker}");
        }
    }

    #[test]
    fn bad_password_shapes() {
        assert_eq!(classify("Invalid master password.", false), BwErrorKind::BadPassword);
        assert_eq!(
            classify("Username or password is incorrect. Try again.", false),
            BwErrorKind::BadPassword
        );
    }

    #[test]
    fn locked_shape() {
        assert_eq!(classify("Vault is locked.", false), BwErrorKind::Locked);
    }

    #[test]
    fn unrecognised_text_falls_back_to_other_rather_than_guessing() {
        assert_eq!(classify("Something bw has never said before.", false), BwErrorKind::Other);
    }

    #[test]
    fn empty_stderr_is_other_not_a_panic() {
        assert_eq!(classify("", false), BwErrorKind::Other);
    }
}
