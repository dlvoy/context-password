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

/// This build's token for the `?os=` filter on tag uris (`bw::filter`) —
/// the first runtime OS identifier in the codebase. Everything else here is
/// compile-time `#[cfg]`; this is a `const` rather than
/// `std::env::consts::OS` so `bw::filter::build_entries` stays a pure
/// function a test can drive with any of `OS_TAGS`, not just the one this
/// binary happens to be compiled for.
#[cfg(windows)]
pub const OS_TAG: &str = "win";
#[cfg(target_os = "macos")]
pub const OS_TAG: &str = "mac";
// Neither Windows nor macOS. Nothing in this repo ships for Linux today —
// this arm exists only so `cargo check`/`clippy` on a non-Windows,
// non-macOS host (e.g. CI) compiles this file at all.
#[cfg(not(any(windows, target_os = "macos")))]
pub const OS_TAG: &str = "linux";

/// The full `?os=` vocabulary, identical on every build regardless of
/// `OS_TAG` — a `?os=ios` tag is a valid filter on Windows (it hides the
/// item), not a typo, so every build must recognise every token. **These
/// strings must stay stable forever**: they live in users' vault data, not
/// just in this binary. A future Android or iOS port adds an `OS_TAG` arm
/// above and needs nothing else here.
pub const OS_TAGS: [&str; 5] = ["win", "mac", "linux", "android", "ios"];

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
