//! Platform switch. Every OS-specific module lives under here, one
//! subdirectory per target — `win/` today, `mac/` once the macOS port
//! lands — each exposing the same function-level API (`focus`, `typing`,
//! `monitor`, `singleton`, `autostart`, `indicator`, `window_style`,
//! `integrity`) so call sites elsewhere in the app go through
//! `platform::<module>::<fn>` without caring which OS they're on.

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

/// Why a target window can't receive synthetic keystrokes right now. The
/// underlying cause is platform-specific and the failure is always
/// silent — `SendInput`/`CGEventPost` return success either way — so it has
/// to be detected ahead of time (at hotkey-capture time, into
/// `focus::Target::blocked`) and surfaced as a warning, rather than
/// discovered after a delivery attempt that quietly did nothing.
// `NoAccessibility`/`SecureInput` are only ever constructed by the macOS
// backend (not yet written) — `cfg_attr` rather than a blanket allow so the
// warning comes back on its own once that backend exists.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Windows: the target's process runs at a higher UIPI integrity level
    /// than ours, so `SendInput` targeting it is silently dropped. See
    /// `win::integrity`.
    Elevated,
    /// macOS: the Accessibility permission hasn't been granted, so
    /// `CGEventPost` has no effect outside our own process.
    NoAccessibility,
    /// macOS: the focused field holds Secure Event Input
    /// (`IsSecureEventInputEnabled`), which blocks synthetic keystrokes
    /// into it regardless of Accessibility.
    SecureInput,
}
