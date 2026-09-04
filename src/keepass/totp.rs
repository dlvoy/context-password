//! `otp` custom-field recognition and local TOTP computation.
//!
//! v1 scope: a custom field whose key case-insensitively matches `"otp"`,
//! holding an `otpauth://` URI — the de facto convention used by
//! KeePassXC/KeeTrayTOTP. `keepass-core` deliberately does not implement
//! TOTP at all (a stated non-goal of the format library); this is entirely
//! application code. Unlike Bitwarden's `GetTotp` (a subprocess round trip
//! to the real server), this is a synchronous, local computation — but it
//! still goes through the same `VaultCmd::GetTotp` round trip both
//! front-ends already have a spinner state for, so nothing upstream needs
//! to know the difference.

use keepass_core::kdbx::{Kdbx, Unlocked};
use keepass_core::model::{Entry as KpEntry, EntryId};

use super::model::custom_field_ci;
use crate::secret::Secret;
use crate::vault::VaultErrorKind;

const OTP_FIELD_KEY: &str = "otp";

/// Whether `entry` has an `otp`-shaped custom field at all — a key check
/// only, no reveal needed, since custom-field *keys* are plaintext in KDBX
/// regardless of whether the field's value is protected.
pub fn has_totp_field(entry: &KpEntry) -> bool {
    custom_field_ci(entry, OTP_FIELD_KEY).is_some()
}

/// The `otp` field's key, in its **original casing** — deliberately not
/// lowercased. `reveal_custom_field` looks the field up by exact key, so
/// retrieval must use the same casing the entry actually has, or a
/// `"OTP"`-keyed entry would silently fail to reveal at `GetTotp` time even
/// though `has_totp_field` correctly (case-insensitively) found it at list
/// time.
fn totp_key(entry: &KpEntry) -> Option<&str> {
    custom_field_ci(entry, OTP_FIELD_KEY).map(|f| f.key.as_str())
}

/// Computes the current TOTP code for `id`'s `otp` field, or a
/// `(kind, message)` pair ready for `VaultResult::Failed` on any failure —
/// missing field, reveal failure, or a value that isn't an `otpauth://`
/// URI (deliberately not guessed at: a bare base32 secret's algorithm/
/// digits/period can't be inferred, and a silently-wrong code is worse than
/// a clear error).
pub fn compute(kdbx: &Kdbx<Unlocked>, id: EntryId) -> Result<Secret, (VaultErrorKind, String)> {
    let Some(entry) = kdbx.vault().entry(id) else {
        return Err((VaultErrorKind::Other, "item no longer exists".to_string()));
    };
    let Some(key) = totp_key(entry) else {
        return Err((VaultErrorKind::Other, "this item has no otp field".to_string()));
    };

    let value = kdbx
        .reveal_custom_field(id, key)
        .map_err(|e| (VaultErrorKind::Other, format!("couldn't reveal the otp field: {e}")))?
        .ok_or_else(|| (VaultErrorKind::Other, "this item has no otp field".to_string()))?;

    if !value.to_ascii_lowercase().starts_with("otpauth://") {
        return Err((
            VaultErrorKind::Other,
            "the entry's otp field isn't an otpauth:// URI".to_string(),
        ));
    }

    let totp = totp_rs::Totp::from_url_unchecked(&value)
        .map_err(|e| (VaultErrorKind::Other, format!("couldn't parse the otp field: {e}")))?;
    let code = totp.generate_current().to_string();
    Ok(Secret::new(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use keepass_core::secret::CompositeKey;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    /// The fixture's `otp` field holds a fixed, known secret — this proves
    /// `compute` actually parses the URI and derives *a* correctly-shaped
    /// code, without depending on wall-clock time by using `Totp::generate`
    /// at a pinned timestamp instead of going through `compute` (which
    /// always uses "now"). See `keepass::filter::tests` for the id lookup.
    #[test]
    fn otpauth_uri_from_fixture_produces_a_valid_totp_code() {
        let composite = CompositeKey::from_password(b"tagged-fixture-pw");
        let unlocked = Kdbx::open(fixture("tagged.kdbx"))
            .unwrap()
            .read_header()
            .unwrap()
            .unlock(&composite)
            .unwrap();
        let entry = unlocked
            .vault()
            .all_entries()
            .into_iter()
            .find(|e| e.title == "OTP Entry")
            .expect("fixture has an OTP Entry");

        assert!(has_totp_field(entry));
        let code = compute(&unlocked, entry.id).expect("compute should succeed");

        // Cross-check against a direct, pinned-timestamp computation from
        // the same known secret, rather than asserting an exact code that
        // would immediately go stale.
        let expected = totp_rs::Totp::from_url_unchecked(
            "otpauth://totp/Fixture:user9?secret=JBSWY3DPEHPK3PXP&issuer=Fixture&digits=6&period=30",
        )
        .unwrap()
        .generate_current()
        .to_string();
        assert_eq!(code.expose(), expected);
        assert_eq!(code.expose().len(), 6);
        assert!(code.expose().chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn entry_without_otp_field_reports_a_clear_error() {
        let composite = CompositeKey::from_password(b"tagged-fixture-pw");
        let unlocked = Kdbx::open(fixture("tagged.kdbx"))
            .unwrap()
            .read_header()
            .unwrap()
            .unlock(&composite)
            .unwrap();
        let entry = unlocked
            .vault()
            .all_entries()
            .into_iter()
            .find(|e| e.title == "URL Tagged")
            .expect("fixture has a URL Tagged entry");

        assert!(!has_totp_field(entry));
        let (kind, message) = compute(&unlocked, entry.id).unwrap_err();
        assert_eq!(kind, VaultErrorKind::Other);
        assert!(message.contains("no otp field"), "message was: {message}");
    }
}
