//! The two macOS-only preconditions for synthetic keystroke delivery,
//! neither of which has a Windows analogue: the Accessibility permission
//! (TCC-gated, required for `CGEventPost` to affect any process but our
//! own) and Secure Event Input (blocks synthetic keystrokes into whichever
//! field currently holds it, regardless of Accessibility). Feeds
//! `platform::BlockReason::{NoAccessibility, SecureInput}` — the direct
//! counterpart of Windows' UIPI elevation check (`win::integrity`), same
//! "silent failure, so pre-check and warn" shape, different predicate.

use objc2_application_services::AXIsProcessTrusted;

use super::super::BlockReason;

// `IsSecureEventInputEnabled` is Carbon API with no objc2 binding — one
// manual declaration, confirmed present in this machine's SDK
// (`HIToolbox.tbd`, part of the Carbon umbrella framework).
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8; // Boolean
}

/// Whether this process has been granted Accessibility. Cheap (no IPC to
/// the TCC daemon on the fast path) — safe to call on every hotkey press,
/// not just once at startup, since the user can revoke it at any time in
/// System Settings.
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Whether the currently focused field anywhere on the system holds Secure
/// Event Input — e.g. a password field in a *different* app than the one
/// we're about to type into wouldn't block us, but the one we're actually
/// targeting would. `CGEventPost` into it is silently dropped, so this
/// must be checked ahead of time, not discovered after the fact.
pub fn secure_input_active() -> bool {
    unsafe { IsSecureEventInputEnabled() != 0 }
}

/// The `BlockReason` for a delivery attempted right now, if any.
/// Accessibility is checked first since it blocks *everything*; Secure
/// Input only blocks the specific focused field.
pub fn block_reason() -> Option<BlockReason> {
    if !accessibility_trusted() {
        return Some(BlockReason::NoAccessibility);
    }
    if secure_input_active() {
        return Some(BlockReason::SecureInput);
    }
    None
}
