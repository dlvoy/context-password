//! Client-side re-filtering, ORD parsing, and sorting of `bw list items`
//! output.
//!
//! `--search` is a fuzzy match across many fields (plan §6), so it is only
//! a prefilter; the real filter — matching `login.uris[].uri` against the
//! app's own prefix, then its optional `?os=` list against this build's
//! [`platform::OS_TAG`](crate::platform::OS_TAG) — happens here.

use super::model::RawItem;
use crate::secret::Secret;
use crate::vault::tag::{self, Tag, Verdict};
use crate::vault::{sort_entries, Entry};

/// Returns the resolved tag for `item`, or `None` if none of its uris
/// belong to this app at all (wrong prefix, or a mere substring collision
/// like `app://context-password-other/1`).
///
/// An item can carry more than one tag uri — e.g.
/// `app://context-password/1?os=win` alongside
/// `app://context-password/5?os=mac`, giving it a different ORD per
/// platform. When more than one tag uri is present, the first
/// [`Verdict::Show`] wins; failing that, the first [`Verdict::BadOs`];
/// failing that, [`Verdict::OtherOs`]. This is deliberate priority (see
/// `vault::tag::best`), not an arbitrary pick: a uri meant for this build
/// always wins over one that isn't, and a malformed filter is surfaced over
/// one that's merely for another OS.
fn tag_of(item: &RawItem, prefix: &str, os: &str) -> Option<Tag> {
    let login = item.login.as_ref()?;
    let mut best: Option<Tag> = None;
    for uri in &login.uris {
        if let Some(t) = tag::from_uri(&uri.uri, prefix, os) {
            best = tag::best(best, t);
        }
    }
    best
}

/// Filters and sorts raw `bw list items` output into the popup's entries.
/// `os` is this build's `?os=` token (`platform::OS_TAG` at the call site;
/// threaded through as a parameter, not read directly, so tests can drive
/// every platform from one build). Returns the entries plus a count of
/// items that were dropped (wrong item type, no password, no matching
/// `prefix` uri at all, or a malformed `os=` filter) — surfaced so a
/// mistagged item doesn't silently vanish without a trace. Items filtered
/// out by a well-formed `os=` for another platform are *not* counted here —
/// see [`Verdict::OtherOs`].
pub fn build_entries(items: Vec<RawItem>, prefix: &str, os: &str) -> (Vec<Entry>, usize) {
    let mut entries = Vec::new();
    let mut dropped = 0usize;

    for item in items {
        if item.ty != 1 {
            dropped += 1;
            continue;
        }
        let Some(tag) = tag_of(&item, prefix, os) else {
            dropped += 1;
            continue;
        };

        let ord = match tag.verdict {
            Verdict::Show(ord) => ord,
            Verdict::BadOs => {
                eprintln!(
                    "bw: {:?} — app:// tag's os= filter has no token this build recognises \
                     ({:?}); hiding it",
                    item.name,
                    tag.unknown_os.as_deref().unwrap_or("")
                );
                dropped += 1;
                continue;
            }
            Verdict::OtherOs => continue,
        };
        if let Some(unknown) = &tag.unknown_os {
            eprintln!(
                "bw: {:?} — app:// tag's os= filter includes an unrecognised token: {unknown}",
                item.name
            );
        }

        let Some(login) = item.login else {
            dropped += 1;
            continue;
        };
        let Some(password) = login.password.filter(|p| !p.is_empty()) else {
            dropped += 1;
            continue;
        };
        let has_totp = login.totp.is_some_and(|t| !t.is_empty());

        entries.push(Entry {
            id: item.id,
            name: item.name,
            username: login.username,
            ord,
            password: Secret::new(password),
            has_totp,
        });
    }

    sort_entries(&mut entries);

    (entries, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bw::model::{RawLogin, RawUri};
    use crate::platform::OS_TAGS;

    const PREFIX: &str = "app://context-password";
    /// The OS token these tests drive `build_entries` with by default —
    /// arbitrary; the whole point of threading `os` through as a parameter
    /// is that every token in `platform::OS_TAGS` is exercisable from a
    /// single (Windows) test binary.
    const OS: &str = "win";

    fn item(
        id: &str,
        name: &str,
        ty: u8,
        uris: Vec<(&str, Option<u8>)>,
        password: Option<&str>,
    ) -> RawItem {
        RawItem {
            id: id.to_string(),
            name: name.to_string(),
            ty,
            login: Some(RawLogin {
                uris: uris
                    .into_iter()
                    .map(|(u, m)| RawUri {
                        uri: u.to_string(),
                        match_type: m,
                    })
                    .collect(),
                username: Some("user".to_string()),
                password: password.map(str::to_string),
                totp: None,
            }),
        }
    }

    #[test]
    fn keeps_items_with_prefixed_uri_and_sorts_by_ord() {
        let items = vec![
            item(
                "1",
                "b-item",
                1,
                vec![("app://context-password/2", Some(1))],
                Some("pw1"),
            ),
            item(
                "2",
                "a-item",
                1,
                vec![("app://context-password/1", None)],
                Some("pw2"),
            ),
        ];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(dropped, 0);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a-item");
        assert_eq!(entries[1].name, "b-item");
    }

    #[test]
    fn ties_break_by_name_case_insensitively() {
        let items = vec![
            item(
                "1",
                "Zebra",
                1,
                vec![("app://context-password/5", None)],
                Some("pw"),
            ),
            item(
                "2",
                "apple",
                1,
                vec![("app://context-password/5", None)],
                Some("pw"),
            ),
        ];
        let (entries, _) = build_entries(items, PREFIX, OS);
        assert_eq!(entries[0].name, "apple");
        assert_eq!(entries[1].name, "Zebra");
    }

    #[test]
    fn missing_ord_sorts_last_not_as_zero() {
        let items = vec![
            item(
                "1",
                "no-ord",
                1,
                vec![("app://context-password/", None)],
                Some("pw"),
            ),
            item(
                "2",
                "has-ord",
                1,
                vec![("app://context-password/0", None)],
                Some("pw"),
            ),
        ];
        let (entries, _) = build_entries(items, PREFIX, OS);
        assert_eq!(entries[0].name, "has-ord"); // real ord (0) sorts before missing
        assert_eq!(entries[1].name, "no-ord");
    }

    #[test]
    fn unparseable_ord_sorts_last_and_is_kept() {
        let items = vec![item(
            "1",
            "typo",
            1,
            vec![("app://context-password/abc", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(dropped, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ord, None);
    }

    #[test]
    fn tagged_uri_can_be_at_any_index() {
        let items = vec![item(
            "1",
            "mixed",
            1,
            vec![
                ("http://192.168.5.30", None),
                ("app://context-password/2", Some(1)),
            ],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(dropped, 0);
        assert_eq!(entries[0].ord, Some(2));
    }

    #[test]
    fn rejects_prefix_collision() {
        // Starts with the prefix as a plain substring but is not one of
        // ours — must not match.
        let items = vec![item(
            "1",
            "impostor",
            1,
            vec![("app://context-password-other/1", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn rejects_prefix_collision_even_with_a_query_string() {
        let items = vec![item(
            "1",
            "impostor",
            1,
            vec![("app://context-password-other/1?os=win", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn drops_non_login_items() {
        let mut secure_note = item("1", "note", 2, vec![], None);
        secure_note.login = None;
        let (entries, dropped) = build_entries(vec![secure_note], PREFIX, OS);
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn drops_items_with_no_matching_uri() {
        let items = vec![item(
            "1",
            "unrelated",
            1,
            vec![("https://example.com", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn drops_items_with_missing_login() {
        let mut it = item(
            "1",
            "weird",
            1,
            vec![("app://context-password/1", None)],
            Some("pw"),
        );
        it.login = None;
        let (entries, dropped) = build_entries(vec![it], PREFIX, OS);
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn drops_items_with_empty_password() {
        let items = vec![item(
            "1",
            "no-pass",
            1,
            vec![("app://context-password/1", None)],
            Some(""),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 1);
    }

    /// Shaped like a real `bw list items --search app://context-password`
    /// response — three items, ORDs 2/3/1, tagged uri at varying indices,
    /// `match` sometimes absent, extra fields like `totp` present on one.
    /// Values are synthetic, not anyone's real vault data.
    #[test]
    fn realistic_shaped_response_sorts_by_ord() {
        let json = r#"[
          {
            "type": 1, "id": "11111111-1111-1111-1111-111111111111",
            "name": "example@home-server",
            "login": {
              "uris": [
                {"uri": "http://10.0.0.10"},
                {"uri": "app://context-password/2", "match": 1}
              ],
              "username": "admin", "password": "REDACTED"
            }
          },
          {
            "type": 1, "id": "22222222-2222-2222-2222-222222222222",
            "name": "git-mirror - token",
            "login": {
              "uris": [
                {"uri": "https://git.example.com", "match": 1},
                {"uri": "app://context-password/3", "match": 1}
              ],
              "username": "ci-bot", "password": "REDACTED"
            }
          },
          {
            "type": 1, "id": "33333333-3333-3333-3333-333333333333",
            "name": "nas - admin",
            "login": {
              "uris": [
                {"uri": "http://nas.example.com:5000/"},
                {"uri": "http://10.0.0.20:5000"},
                {"uri": "androidapp://com.example.viewer"},
                {"uri": "androidapp://com.example.files"},
                {"uri": "app://context-password/1"}
              ],
              "username": "admin", "password": "REDACTED", "totp": "REDACTED"
            }
          },
          {
            "type": 1, "id": "44444444-4444-4444-4444-444444444444",
            "name": "mac-only-vpn",
            "login": {
              "uris": [
                {"uri": "app://context-password/4?os=mac"}
              ],
              "username": "admin", "password": "REDACTED"
            }
          }
        ]"#;
        let items: Vec<RawItem> = serde_json::from_str(json).unwrap();
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(dropped, 0, "the os=mac item is filtered out, not dropped");
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, "nas - admin"); // ord 1
        assert_eq!(entries[1].name, "example@home-server"); // ord 2
        assert_eq!(entries[2].name, "git-mirror - token"); // ord 3
        assert!(entries[0].has_totp, "nas - admin has a totp field in the fixture");
        assert!(!entries[1].has_totp, "example@home-server has no totp field");
    }

    #[test]
    fn has_totp_is_false_when_the_field_is_absent_or_empty() {
        let mut with_empty = item(
            "1",
            "empty-totp",
            1,
            vec![("app://context-password/1", None)],
            Some("pw"),
        );
        with_empty.login.as_mut().unwrap().totp = Some(String::new());
        let (entries, _) = build_entries(vec![with_empty], PREFIX, OS);
        assert!(!entries[0].has_totp);
    }

    #[test]
    fn has_totp_is_true_when_the_field_is_present() {
        let mut with_totp = item(
            "1",
            "has-totp",
            1,
            vec![("app://context-password/1", None)],
            Some("pw"),
        );
        with_totp.login.as_mut().unwrap().totp = Some("SEED".to_string());
        let (entries, _) = build_entries(vec![with_totp], PREFIX, OS);
        assert!(entries[0].has_totp);
    }

    // --- ?os= filtering ---------------------------------------------------

    #[test]
    fn ord_survives_a_query_string() {
        let items = vec![item(
            "1",
            "tagged",
            1,
            vec![("app://context-password/1?os=win", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(dropped, 0);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ord, Some(1));
    }

    #[test]
    fn shows_on_matching_os() {
        let items = vec![item(
            "1",
            "win-only",
            1,
            vec![("app://context-password/1?os=win", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, "win");
        assert_eq!(entries.len(), 1);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn hides_on_non_matching_os_without_counting_as_dropped() {
        let items = vec![item(
            "1",
            "mac-only",
            1,
            vec![("app://context-password/1?os=mac", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, "win");
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 0, "filtered for another OS is not a drop");
    }

    #[test]
    fn comma_list_matches_any_listed_os() {
        let uri = "app://context-password/1?os=win,mac";
        let win = build_entries(
            vec![item("1", "cross", 1, vec![(uri, None)], Some("pw"))],
            PREFIX,
            "win",
        );
        assert_eq!(win.0.len(), 1);
        let mac = build_entries(
            vec![item("1", "cross", 1, vec![(uri, None)], Some("pw"))],
            PREFIX,
            "mac",
        );
        assert_eq!(mac.0.len(), 1);
        let linux = build_entries(
            vec![item("1", "cross", 1, vec![(uri, None)], Some("pw"))],
            PREFIX,
            "linux",
        );
        assert_eq!(linux.0.len(), 0);
        assert_eq!(linux.1, 0);
    }

    #[test]
    fn repeated_os_param_unions_like_a_comma_list() {
        let uri = "app://context-password/1?os=win&os=mac";
        let win = build_entries(
            vec![item("1", "cross", 1, vec![(uri, None)], Some("pw"))],
            PREFIX,
            "win",
        );
        assert_eq!(win.0.len(), 1);
        let mac = build_entries(
            vec![item("1", "cross", 1, vec![(uri, None)], Some("pw"))],
            PREFIX,
            "mac",
        );
        assert_eq!(mac.0.len(), 1);
    }

    #[test]
    fn os_matching_is_case_and_whitespace_insensitive() {
        for uri in [
            "app://context-password/1?os=Win",
            "app://context-password/1?os=win, mac",
            "app://context-password/1?OS=win",
        ] {
            let items = vec![item("1", "cross", 1, vec![(uri, None)], Some("pw"))];
            let (entries, dropped) = build_entries(items, PREFIX, "win");
            assert_eq!(entries.len(), 1, "uri {uri:?} should match win");
            assert_eq!(dropped, 0);
        }
    }

    #[test]
    fn every_supported_os_token_round_trips() {
        for &os in OS_TAGS.iter() {
            let uri = format!("app://context-password/1?os={os}");
            let items = vec![item("1", "cross", 1, vec![(&uri, None)], Some("pw"))];
            let (entries, dropped) = build_entries(items, PREFIX, os);
            assert_eq!(entries.len(), 1, "{os} should match its own tag");
            assert_eq!(dropped, 0);
        }
        // ?os=ios hides on win without inflating `dropped`.
        let items = vec![item(
            "1",
            "ios-only",
            1,
            vec![("app://context-password/1?os=ios", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, "win");
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn unrecognised_os_value_hides_and_counts_as_dropped() {
        for uri in [
            "app://context-password/1?os=beos",
            "app://context-password/1?os=",
            "app://context-password/1?os",
        ] {
            let items = vec![item("1", "typo", 1, vec![(uri, None)], Some("pw"))];
            let (entries, dropped) = build_entries(items, PREFIX, OS);
            assert_eq!(entries.len(), 0, "uri {uri:?} should be hidden");
            assert_eq!(dropped, 1, "uri {uri:?} should count as dropped");
        }
    }

    #[test]
    fn partially_bad_os_value_still_shows_on_the_recognised_token() {
        let items = vec![item(
            "1",
            "partial",
            1,
            vec![("app://context-password/1?os=win,xx", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, "win");
        assert_eq!(entries.len(), 1);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn unknown_query_param_is_ignored() {
        let items = vec![item(
            "1",
            "unrelated-param",
            1,
            vec![("app://context-password/1?foo=bar", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, OS);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ord, Some(1));
        assert_eq!(dropped, 0);
    }

    #[test]
    fn missing_ord_with_an_os_filter_is_still_shown_sorted_last() {
        let items = vec![item(
            "1",
            "no-ord-filtered",
            1,
            vec![("app://context-password/?os=win", None)],
            Some("pw"),
        )];
        let (entries, dropped) = build_entries(items, PREFIX, "win");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].ord, None);
        assert_eq!(dropped, 0);
    }

    #[test]
    fn an_item_can_have_a_different_ord_per_os() {
        let uris = vec![
            ("app://context-password/1?os=win", None),
            ("app://context-password/5?os=mac", None),
        ];
        let (win_entries, _) = build_entries(
            vec![item("1", "per-os", 1, uris.clone(), Some("pw"))],
            PREFIX,
            "win",
        );
        assert_eq!(win_entries[0].ord, Some(1));
        let (mac_entries, _) = build_entries(
            vec![item("1", "per-os", 1, uris, Some("pw"))],
            PREFIX,
            "mac",
        );
        assert_eq!(mac_entries[0].ord, Some(5));
    }
}
