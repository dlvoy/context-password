//! Small adapter helpers between `keepass_core`'s model types and this
//! app's own — id round-tripping and case-insensitive custom-field lookup,
//! shared by `filter.rs` (tag resolution) and `totp.rs` (`otp` field
//! recognition).
//!
//! No `Debug` derive on anything added here (plan §8, guarded by
//! `secret::tests::no_debug_derive_on_types_holding_passwords`) — should
//! this module ever grow a type that carries revealed field material, it
//! must follow the same redaction discipline as `vault::entry::Entry` and
//! `bw::model`.

use keepass_core::model::{CustomField, Entry as KpEntry, EntryId};
use uuid::Uuid;

/// `vault::Entry.id` is an opaque `String` shared across providers — this
/// is the KeePass side of that contract: a `keepass_core::model::EntryId`
/// round-tripped through its UUID's string form.
pub fn entry_id_to_string(id: EntryId) -> String {
    id.0.to_string()
}

/// The inverse of [`entry_id_to_string`] — `None` if the string isn't a
/// valid UUID, which should only happen if something outside this provider
/// (a stale id from a different provider, a corrupted round-trip) hands
/// back a value that was never one of ours.
pub fn parse_entry_id(s: &str) -> Option<EntryId> {
    Uuid::parse_str(s).ok().map(EntryId)
}

/// Finds a custom field by key, case-insensitively — used both for the
/// `context-password` tag fallback (`filter.rs`) and `otp` field
/// recognition (`totp.rs`). Returns the **first** match in `custom_fields`
/// order; duplicate keys aren't legal KDBX to begin with, so there's
/// nothing more meaningful to tie-break on.
pub fn custom_field_ci<'a>(entry: &'a KpEntry, key: &str) -> Option<&'a CustomField> {
    entry.custom_fields.iter().find(|f| f.key.eq_ignore_ascii_case(key))
}
