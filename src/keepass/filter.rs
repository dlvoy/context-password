//! Building the popup's entry list from an unlocked KDBX vault: tag
//! resolution (URL field, with a `context-password` custom-field
//! fallback), the `dropped` count, and sorting.
//!
//! Unlike `bw::filter::build_entries`, this walks the **entire** vault —
//! there's no server-side `--search <prefix>` prefilter to lean on, since
//! the whole file is already local. The tag-resolution `continue` below
//! (an entry with neither a URL tag nor a `context-password` field) is
//! therefore doing exactly the job that prefilter does for Bitwarden: it
//! must never count toward `dropped`, or a vault with hundreds of unrelated
//! entries would show a permanent, meaningless "N item(s) hidden" banner.

use keepass_core::kdbx::{Kdbx, Unlocked};

use super::model::{custom_field_ci, entry_id_to_string};
use super::totp;
use crate::secret::Secret;
use crate::vault::tag::{self, Verdict};
use crate::vault::{sort_entries, Entry};

pub fn build_entries(kdbx: &Kdbx<Unlocked>, prefix: &str, os: &str) -> (Vec<Entry>, usize) {
    let mut entries = Vec::new();
    let mut dropped = 0usize;

    for kp in kdbx.vault().all_entries() {
        // Step 1: resolve the tag — URL field first, `context-password`
        // custom field only if the URL didn't match at all. When an entry
        // carries both, the URL wins outright; the custom field is never
        // even consulted in that case.
        let tag = match tag::from_uri(kp.url.trim(), prefix, os) {
            Some(t) => t,
            None => {
                let Some(field) = custom_field_ci(kp, "context-password") else {
                    // Neither mechanism present — not one of ours. This is
                    // the "prefilter" gate described in the module doc:
                    // silently skip, do NOT count toward `dropped`.
                    continue;
                };
                // The tag field is routing metadata, not a secret, but it
                // could still be marked `Protected="True"` by a user's
                // KeePass client — reveal it rather than trusting the
                // possibly-still-ciphertext raw `value`. A reveal failure
                // falls back to the raw value (worst case: an unparseable
                // ord, which still shows the entry rather than losing it).
                let value = kdbx
                    .reveal_custom_field(kp.id, &field.key)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| field.value.clone());
                tag::from_body(value.trim(), os)
            }
        };

        let ord = match tag.verdict {
            Verdict::Show(ord) => ord,
            Verdict::BadOs => {
                eprintln!(
                    "keepass: {:?} — context-password tag's os= filter has no token this build \
                     recognises ({:?}); hiding it",
                    kp.title,
                    tag.unknown_os.as_deref().unwrap_or("")
                );
                dropped += 1;
                continue;
            }
            Verdict::OtherOs => continue,
        };
        if let Some(unknown) = &tag.unknown_os {
            eprintln!(
                "keepass: {:?} — context-password tag's os= filter includes an unrecognised \
                 token: {unknown}",
                kp.title
            );
        }

        // Step 2: reveal the password. A reveal failure means this entry
        // is clearly tagged for us but broken somehow (corrupt protected
        // field) — counted as dropped, same as an empty password.
        let password = match kdbx.reveal_password(kp.id) {
            Ok(p) if !p.is_empty() => p,
            Ok(_) => {
                dropped += 1;
                continue;
            }
            Err(e) => {
                eprintln!("keepass: {:?} — couldn't reveal password: {e}", kp.title);
                dropped += 1;
                continue;
            }
        };

        entries.push(Entry {
            id: entry_id_to_string(kp.id),
            name: kp.title.clone(),
            username: (!kp.username.is_empty()).then(|| kp.username.clone()),
            ord,
            password: Secret::new(password),
            has_totp: totp::has_totp_field(kp),
        });
    }

    sort_entries(&mut entries);

    (entries, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use keepass_core::secret::CompositeKey;
    use std::path::PathBuf;

    const PREFIX: &str = "app://context-password";
    const OS: &str = "mac";

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    fn unlock(name: &str, password: &str) -> Kdbx<Unlocked> {
        let composite = CompositeKey::from_password(password.as_bytes());
        Kdbx::open(fixture(name))
            .unwrap()
            .read_header()
            .unwrap()
            .unlock(&composite)
            .unwrap()
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// The full picture: which of the fixture's 10 entries show, in what
    /// order, and how many count as `dropped`. See `tests/fixtures/README.md`
    /// for exactly what each entry is testing.
    #[test]
    fn tagged_fixture_shows_the_right_entries_in_order_with_the_right_dropped_count() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, dropped) = build_entries(&kdbx, PREFIX, OS);

        // Shown, in ord order: URL Tagged(1), Custom Field Tagged(2), Both
        // Tags(3, URL's ord wins not the custom field's 9), Nested(7),
        // OTP(8), History(10).
        assert_eq!(
            names(&entries),
            vec![
                "URL Tagged",
                "Custom Field Tagged",
                "Both Tags (URL Wins)",
                "Nested Entry",
                "OTP Entry",
                "History Entry",
            ]
        );

        // Dropped: Bad OS Entry + Empty Password Entry. NOT the Untagged
        // Entry (neither mechanism present — not a prefilter miss) and NOT
        // the Foreign OS Entry (a well-formed tag for a different, real OS).
        assert_eq!(dropped, 2);
    }

    #[test]
    fn url_tag_wins_over_custom_field_tag_when_an_entry_has_both() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, _) = build_entries(&kdbx, PREFIX, OS);
        let both = entries.iter().find(|e| e.name == "Both Tags (URL Wins)").unwrap();
        assert_eq!(both.ord, Some(3), "the URL's ord must win, not the custom field's 9");
    }

    #[test]
    fn custom_field_tag_alone_behaves_like_the_equivalent_url() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, _) = build_entries(&kdbx, PREFIX, OS);
        let cf = entries.iter().find(|e| e.name == "Custom Field Tagged").unwrap();
        assert_eq!(cf.ord, Some(2));
    }

    #[test]
    fn untagged_entry_is_invisible_and_not_counted() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, dropped) = build_entries(&kdbx, PREFIX, OS);
        assert!(!names(&entries).contains(&"Untagged Entry"));
        // If the untagged entry were (wrongly) counted, dropped would be 3
        // instead of 2 — this is asserted precisely in the fixture-wide test
        // above; this test names the specific regression it guards.
        assert_eq!(dropped, 2);
    }

    #[test]
    fn foreign_os_is_hidden_but_not_counted_as_dropped() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, _) = build_entries(&kdbx, PREFIX, OS);
        assert!(!names(&entries).contains(&"Foreign OS Entry"));
    }

    #[test]
    fn nested_group_entries_are_found() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, _) = build_entries(&kdbx, PREFIX, OS);
        assert!(names(&entries).contains(&"Nested Entry"));
    }

    #[test]
    fn history_snapshots_never_appear_as_their_own_entries() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, _) = build_entries(&kdbx, PREFIX, OS);
        assert_eq!(names(&entries).iter().filter(|n| n.starts_with("History")).count(), 1);
    }

    #[test]
    fn revealed_passwords_match_what_was_stored() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, _) = build_entries(&kdbx, PREFIX, OS);
        let e = entries.iter().find(|e| e.name == "URL Tagged").unwrap();
        assert_eq!(e.password.expose(), "urlpass1");
        assert_eq!(e.username.as_deref(), Some("user1"));
    }

    #[test]
    fn otp_entry_is_flagged_has_totp() {
        let kdbx = unlock("tagged.kdbx", "tagged-fixture-pw");
        let (entries, _) = build_entries(&kdbx, PREFIX, OS);
        let e = entries.iter().find(|e| e.name == "OTP Entry").unwrap();
        assert!(e.has_totp);
        let not_otp = entries.iter().find(|e| e.name == "URL Tagged").unwrap();
        assert!(!not_otp.has_totp);
    }

    #[test]
    fn empty_vault_yields_no_entries_and_no_drops() {
        let kdbx = unlock("empty.kdbx", "empty-fixture-pw");
        let (entries, dropped) = build_entries(&kdbx, PREFIX, OS);
        assert!(entries.is_empty());
        assert_eq!(dropped, 0);
    }
}
