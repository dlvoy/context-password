//! The shared tag grammar: what one `context-password` tag means for *this*
//! build, independent of how it was stored.
//!
//! Two storage shapes carry the identical grammar: a Bitwarden login uri
//! (`<prefix>/<ORD>[?os=...]`, matched via [`from_uri`]) and a KeePass
//! `context-password` custom field, whose *value* is exactly the tag body —
//! the substring that would follow `<prefix>/` in the uri form (matched via
//! [`from_body`] directly). Splitting the seam here, at "how do I get from
//! raw storage to a `(ord, query)` pair", is what lets both providers share
//! every downstream rule (ord parsing, `?os=` tokenization, the
//! Show/BadOs/OtherOs verdict, and the priority fold) with zero duplication.

use crate::platform::OS_TAGS;

/// What one tag means for *this* build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Ours and meant for this build (no `os=` filter, or one that lists
    /// this build's tag). Inner `None` = the ORD segment is missing or
    /// unparseable — kept and sorted last, exactly as before `?os=`
    /// existed.
    Show(Option<i64>),
    /// Ours, but its `os=` list names only other systems. Deliberate, so
    /// the item is hidden *without* counting toward `dropped` — otherwise
    /// a cross-platform vault would show a permanent "N item(s) hidden"
    /// for entries that are working exactly as intended.
    OtherOs,
    /// Ours, but `os=` held no token this build recognises at all (e.g. a
    /// typo like `?os=beos`, or an empty value). Hidden and counted, like
    /// any other malformed item.
    BadOs,
}

/// One resolved tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub verdict: Verdict,
    /// Set when the tag's `os=` value carried at least one token outside
    /// [`OS_TAGS`] — even when another token in the same list still
    /// matched (`?os=win,xx`) or the whole thing is [`Verdict::BadOs`].
    /// Logged by each provider's `build_entries` so a typo is visible,
    /// independent of whether it changed whether the item shows.
    pub unknown_os: Option<String>,
}

pub fn verdict_priority(v: &Verdict) -> u8 {
    match v {
        Verdict::Show(_) => 2,
        Verdict::BadOs => 1,
        Verdict::OtherOs => 0,
    }
}

/// Parses a tag *body* — the substring that follows `<prefix>/` in the uri
/// form, and the entire value in the KeePass custom-field form. Accepts
/// `"1"`, `"1?os=mac"`, `"?os=mac"` (no ord), and `""` (bare tag).
pub fn from_body(body: &str, os: &str) -> Tag {
    let (ord_part, query) = body.split_once('?').unwrap_or((body, ""));
    let ord_str = ord_part.trim();
    let ord = if ord_str.is_empty() { None } else { ord_str.parse::<i64>().ok() };
    classify(ord, query, os)
}

/// Strips `prefix` from a full uri, enforces the `/`-boundary rule (so
/// `app://context-password-other/1` does not match `app://context-password`),
/// and delegates the remainder to [`from_body`]. `None` when the uri isn't
/// ours at all.
pub fn from_uri(uri: &str, prefix: &str, os: &str) -> Option<Tag> {
    let trimmed = uri.trim();
    let rest = trimmed.strip_prefix(prefix)?;
    // Reject a uri that merely starts with the prefix as a substring —
    // e.g. "app://context-password-other/1" — by requiring the next
    // character to be the path separator, or nothing at all.
    if !rest.is_empty() && !rest.starts_with('/') {
        return None;
    }
    let rest = rest.trim_start_matches('/');
    Some(from_body(rest, os))
}

/// Folds `candidate` into `current` by [`verdict_priority`] (`Show` >
/// `BadOs` > `OtherOs`), first-wins on ties. Used when an item can carry
/// more than one tag — e.g. a Bitwarden item with both
/// `app://context-password/1?os=win` and `app://context-password/5?os=mac`
/// uris, giving it a different ORD per platform: a tag meant for this build
/// always wins over one that isn't, and a malformed filter is surfaced over
/// one that's merely for another OS.
pub fn best(current: Option<Tag>, candidate: Tag) -> Option<Tag> {
    match current {
        Some(c) if verdict_priority(&c.verdict) >= verdict_priority(&candidate.verdict) => Some(c),
        _ => Some(candidate),
    }
}

/// Parses the bit after `?` on a single tag against this build's `os`
/// token. `query` is empty when the tag had no `?` at all.
fn classify(ord: Option<i64>, query: &str, os: &str) -> Tag {
    if query.is_empty() {
        return Tag { verdict: Verdict::Show(ord), unknown_os: None };
    }

    let mut saw_os_pair = false;
    let mut recognized_any = false;
    let mut matched = false;
    let mut unknown: Vec<String> = Vec::new();
    let mut raw_values: Vec<String> = Vec::new();

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        // No `=` (e.g. a bare `?os`) is treated as an empty value, not
        // skipped — `?os` alone is exactly as malformed as `?os=`.
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if !key.trim().eq_ignore_ascii_case("os") {
            // Unknown parameters are ignored, not errors.
            continue;
        }
        saw_os_pair = true;
        raw_values.push(value.trim().to_string());
        for token in value.split(',') {
            let token = token.trim().to_lowercase();
            if token.is_empty() {
                continue;
            }
            if OS_TAGS.contains(&token.as_str()) {
                recognized_any = true;
                if token == os {
                    matched = true;
                }
            } else {
                unknown.push(token);
            }
        }
    }

    if !saw_os_pair {
        // A query string with no `os` pair at all (e.g. `?foo=bar`).
        return Tag { verdict: Verdict::Show(ord), unknown_os: None };
    }

    let verdict = if !recognized_any {
        Verdict::BadOs
    } else if matched {
        Verdict::Show(ord)
    } else {
        Verdict::OtherOs
    };

    let unknown_os = if !unknown.is_empty() {
        Some(unknown.join(","))
    } else if !recognized_any {
        // Every `os=` pair was present but empty (`?os=` or `?os`) — still
        // worth logging so the typo shows the (blank) value it had.
        Some(raw_values.join(","))
    } else {
        None
    };

    Tag { verdict, unknown_os }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "app://context-password";

    /// The equivalence proof: the two storage shapes must parse to
    /// identical `Tag`s for the same logical body, or the extraction
    /// changed behavior. Cases lifted from `bw::filter`'s original fixtures.
    #[test]
    fn from_body_and_from_uri_agree_on_the_same_grammar() {
        let cases: &[(&str, &str)] = &[
            ("1", "mac"),
            ("", "mac"),
            ("1?os=win", "mac"),
            ("1?os=mac", "mac"),
            ("?os=win", "mac"),
            ("1?os=win,xx", "win"),
            ("1?os=beos", "mac"),
            ("1?os=", "mac"),
            ("1?os", "mac"),
            ("1?foo=bar", "mac"),
            ("notanumber", "mac"),
        ];
        for (body, os) in cases {
            let via_body = from_body(body, os);
            let uri = format!("{PREFIX}/{body}");
            let via_uri = from_uri(&uri, PREFIX, os).unwrap();
            assert_eq!(via_body, via_uri, "mismatch for body {body:?}");
        }
    }

    #[test]
    fn from_uri_rejects_a_prefix_collision() {
        assert!(from_uri("app://context-password-other/1", PREFIX, "mac").is_none());
        assert!(from_uri("app://context-password-other", PREFIX, "mac").is_none());
    }

    #[test]
    fn from_uri_rejects_the_wrong_prefix_entirely() {
        assert!(from_uri("https://example.com", PREFIX, "mac").is_none());
    }

    #[test]
    fn from_uri_tolerates_surrounding_whitespace() {
        let via_uri = from_uri("  app://context-password/1?os=mac  ", PREFIX, "mac").unwrap();
        assert_eq!(via_uri.verdict, Verdict::Show(Some(1)));
    }

    #[test]
    fn best_prefers_show_over_bad_os_over_other_os() {
        let show = Tag { verdict: Verdict::Show(Some(1)), unknown_os: None };
        let bad = Tag { verdict: Verdict::BadOs, unknown_os: None };
        let other = Tag { verdict: Verdict::OtherOs, unknown_os: None };

        assert_eq!(best(Some(other.clone()), show.clone()).unwrap().verdict, Verdict::Show(Some(1)));
        assert_eq!(best(Some(show.clone()), bad.clone()).unwrap().verdict, Verdict::Show(Some(1)));
        assert_eq!(best(Some(other.clone()), bad.clone()).unwrap().verdict, Verdict::BadOs);
        assert_eq!(best(None, other.clone()).unwrap().verdict, Verdict::OtherOs);
    }
}
