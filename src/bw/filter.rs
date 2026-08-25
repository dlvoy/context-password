//! Client-side re-filtering, ORD parsing, and sorting of `bw list items`
//! output.
//!
//! `--search` is a fuzzy match across many fields (plan §6), so it is only
//! a prefilter; the real filter — matching `login.uris[].uri` against the
//! app's own prefix — happens here.

use super::model::{Entry, RawItem};
use crate::secret::Secret;

/// Returns `Some(ord)` if `item` carries a `<prefix>/<ORD>` uri (at any
/// index in `login.uris`, regardless of `match`), or `None` if it doesn't
/// belong to this app at all. The inner `ord` is `None` when the uri is
/// present but its trailing segment is missing or unparseable.
fn ord_of(item: &RawItem, prefix: &str) -> Option<Option<i64>> {
    let login = item.login.as_ref()?;
    for uri in &login.uris {
        let trimmed = uri.uri.trim();
        let Some(rest) = trimmed.strip_prefix(prefix) else {
            continue;
        };
        // Reject a uri that merely starts with the prefix as a substring —
        // e.g. "app://context-password-other/1" — by requiring the next
        // character to be the path separator, or nothing at all.
        if !rest.is_empty() && !rest.starts_with('/') {
            continue;
        }
        let ord_str = rest.trim_start_matches('/').trim();
        return Some(if ord_str.is_empty() {
            None
        } else {
            ord_str.parse::<i64>().ok()
        });
    }
    None
}

/// Filters and sorts raw `bw list items` output into the popup's entries.
/// Returns the entries plus a count of items that were dropped (wrong item
/// type, no password, or no matching `prefix` uri at all) — surfaced so a
/// mistagged item doesn't silently vanish without a trace.
pub fn build_entries(items: Vec<RawItem>, prefix: &str) -> (Vec<Entry>, usize) {
    let mut entries = Vec::new();
    let mut dropped = 0usize;

    for item in items {
        if item.ty != 1 {
            dropped += 1;
            continue;
        }
        let Some(ord) = ord_of(&item, prefix) else {
            dropped += 1;
            continue;
        };
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

    // Malformed/missing ORDs sort last, not as 0 — sorting them as 0 would
    // silently promote a typo to the top and shuffle every deliberately
    // ordered entry (plan §6).
    entries.sort_by(|a, b| {
        (a.ord.is_none(), a.ord.unwrap_or(0), a.name.to_lowercase()).cmp(&(
            b.ord.is_none(),
            b.ord.unwrap_or(0),
            b.name.to_lowercase(),
        ))
    });

    (entries, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bw::model::{RawLogin, RawUri};

    const PREFIX: &str = "app://context-password";

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
        let (entries, dropped) = build_entries(items, PREFIX);
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
        let (entries, _) = build_entries(items, PREFIX);
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
        let (entries, _) = build_entries(items, PREFIX);
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
        let (entries, dropped) = build_entries(items, PREFIX);
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
        let (entries, dropped) = build_entries(items, PREFIX);
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
        let (entries, dropped) = build_entries(items, PREFIX);
        assert_eq!(entries.len(), 0);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn drops_non_login_items() {
        let mut secure_note = item("1", "note", 2, vec![], None);
        secure_note.login = None;
        let (entries, dropped) = build_entries(vec![secure_note], PREFIX);
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
        let (entries, dropped) = build_entries(items, PREFIX);
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
        let (entries, dropped) = build_entries(vec![it], PREFIX);
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
        let (entries, dropped) = build_entries(items, PREFIX);
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
          }
        ]"#;
        let items: Vec<RawItem> = serde_json::from_str(json).unwrap();
        let (entries, dropped) = build_entries(items, PREFIX);
        assert_eq!(dropped, 0);
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
        let (entries, _) = build_entries(vec![with_empty], PREFIX);
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
        let (entries, _) = build_entries(vec![with_totp], PREFIX);
        assert!(entries[0].has_totp);
    }
}
