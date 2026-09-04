//! Data types and pure logic pulled out of `app.rs`, shared where the
//! underlying concept has no OS/UI-toolkit coupling, and `#[cfg(windows)]`
//! where it still does.
//!
//! **Scope note:** `VaultState`, `ExitState`, `DeliveryPhase`/`Delivery`,
//! and `IndicatorPhase`/`next_indicator_phase` are genuinely OS-agnostic —
//! none of them reference `egui` or a Windows type, so they're built
//! unconditionally. `VaultState`/`DeliveryPhase`/`Delivery`/`IndicatorPhase`
//! are used directly by `src/mac_ui/`, not a parallel reimplementation;
//! `ExitState` is the one exception — `mac_ui`'s lock-on-exit is a
//! deliberately simpler fire-and-forget send with no waiting state (see
//! `mac_ui::app_delegate`'s `Msg::Tray(TrayCmd::Quit)` handler), so this
//! type is Windows-`app.rs`-only in practice, kept unconditional here only
//! because nothing about its definition is actually Windows-specific.
//! `Content`, `DeliveryKind`, and the digit-selection functions are still
//! `egui`/`ui::config_window`-shaped, so they stay `#[cfg(windows)]`. macOS
//! doesn't need an
//! equivalent of `Content::Settings`/`Content::About`: those exist only
//! because Windows folds every screen into one `eframe` viewport (working
//! around an eframe/glow bug — see `Content::Settings`'s old doc in
//! `app.rs`'s history), whereas `mac_ui` gives Settings and About their own
//! plain `NSWindow`s, entirely outside the popup panel's own content enum
//! (`mac_ui::PopupContent`, Prompting/Busy/List states only).

use std::time::{Duration, Instant};

use crate::platform::focus::Target;
use crate::secret::Secret;

/// How long the autotype indicator holds the typing glyph even after
/// `send_unicode` returns, and how long its checkmark stays up afterward —
/// see `next_indicator_phase`'s doc for why both are needed.
pub const INDICATOR_MIN_TYPING: Duration = Duration::from_millis(400);
pub const INDICATOR_CHECK_DURATION: Duration = Duration::from_secs(2);

/// Whether the vault currently has anything unlocked — drives the tray's
/// `Show`/`Unlock` label and whether `Lock` is present at all.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VaultState {
    Locked,
    Unlocked,
    /// `bw lock` has been sent but hasn't finished yet.
    Locking,
}

/// Quitting normally just closes the root viewport; `lock_on_exit` needs to
/// briefly *not* do that so a `bw lock` call has a chance to finish first
/// (§7 of the plan) — tracked here rather than a bare bool so the "already
/// asked once" state survives across frames without re-deriving it from
/// `vault_state`, which also changes for unrelated reasons (the tray's own
/// Lock item).
// Only `app.rs` (Windows) actually uses this — see the module doc's scope
// note for why it's `mac_ui`'s one exception to reusing these shared types.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExitState {
    NotExiting,
    WaitingForLock { deadline: Instant },
}

/// The phases of plan §4's typing delivery sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryPhase {
    HidePopup,
    DrainModifiers,
    Activate,
    VerifyForeground,
    Settle,
}

pub struct Delivery {
    pub target: Target,
    pub secret: Secret,
    pub phase: DeliveryPhase,
    /// Meaning depends on the phase: a drop-dead time for `DrainModifiers`
    /// and `VerifyForeground` (past which they give up), or the time to
    /// wait *until* for `Settle`.
    pub deadline: Instant,
}

/// The autotype indicator's state, independent of `DeliveryPhase` — it
/// covers the whole delivery sequence (`HidePopup` through `Settle`), not
/// just the instantaneous `SendInput`/`CGEventPost` call. `typed` flips to
/// `true` the moment text actually lands; an abort (target gone, foreground
/// restore failed, blocked target) never sets it and hides the indicator
/// directly instead of going through this transition at all — a checkmark
/// must never claim a delivery that didn't happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorPhase {
    Typing { since: Instant, typed: bool },
    Done { until: Instant },
}

/// `None` means "hide the indicator". Pure function of its inputs, in the
/// same spirit as `ui::config_window::capture_hotkey` and
/// `platform::place::place_within` — so it's unit tested directly rather
/// than through the whole delivery machine.
pub fn next_indicator_phase(phase: IndicatorPhase, now: Instant) -> Option<IndicatorPhase> {
    match phase {
        IndicatorPhase::Typing { since, typed } => {
            if typed && now >= since + INDICATOR_MIN_TYPING {
                Some(IndicatorPhase::Done {
                    until: now + INDICATOR_CHECK_DURATION,
                })
            } else {
                Some(phase)
            }
        }
        IndicatorPhase::Done { until } => {
            if now >= until {
                None
            } else {
                Some(phase)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_holds_the_glyph_even_after_typed_until_the_minimum_elapses() {
        let now = Instant::now();
        let phase = IndicatorPhase::Typing {
            since: now,
            typed: true,
        };
        // Just short of the floor: the burst finished almost instantly,
        // but the glyph must still be showing.
        let next =
            next_indicator_phase(phase, now + INDICATOR_MIN_TYPING - Duration::from_millis(1));
        assert_eq!(next, Some(phase));
    }

    #[test]
    fn typing_not_yet_typed_never_transitions_on_its_own() {
        let now = Instant::now();
        let phase = IndicatorPhase::Typing {
            since: now,
            typed: false,
        };
        // Delivery can legitimately take longer than the floor (modifier
        // drain, foreground verify) before it ever calls `send_unicode` —
        // that must not be mistaken for "done".
        let next = next_indicator_phase(phase, now + INDICATOR_MIN_TYPING * 10);
        assert_eq!(next, Some(phase));
    }

    #[test]
    fn typed_and_past_the_floor_becomes_done() {
        let now = Instant::now();
        let phase = IndicatorPhase::Typing {
            since: now,
            typed: true,
        };
        let after = now + INDICATOR_MIN_TYPING;
        match next_indicator_phase(phase, after) {
            Some(IndicatorPhase::Done { until }) => {
                assert_eq!(until, after + INDICATOR_CHECK_DURATION);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn done_stays_up_until_its_deadline() {
        let until = Instant::now() + INDICATOR_CHECK_DURATION;
        let phase = IndicatorPhase::Done { until };
        let next = next_indicator_phase(phase, until - Duration::from_millis(1));
        assert_eq!(next, Some(phase));
    }

    #[test]
    fn done_hides_once_its_deadline_passes() {
        let until = Instant::now();
        let phase = IndicatorPhase::Done { until };
        assert_eq!(next_indicator_phase(phase, until), None);
    }
}

/// Windows/eframe-shaped types — see the module doc for why these stay
/// `#[cfg(windows)]` while everything above is shared.
#[cfg(windows)]
mod win {
    use eframe::egui;
    use zeroize::Zeroizing;

    use crate::ui::config_window;
    use crate::ui::popup::IconMode;

    /// `Key::Num1`..`Key::Num9` → 0..8, `Key::Num0` → 9 — the keyboard
    /// counterpart of `ui::popup::badge_for`'s numbering. `None` for any
    /// other key.
    pub fn digit_index(key: egui::Key) -> Option<usize> {
        use egui::Key;
        match key {
            Key::Num1 => Some(0),
            Key::Num2 => Some(1),
            Key::Num3 => Some(2),
            Key::Num4 => Some(3),
            Key::Num5 => Some(4),
            Key::Num6 => Some(5),
            Key::Num7 => Some(6),
            Key::Num8 => Some(7),
            Key::Num9 => Some(8),
            Key::Num0 => Some(9),
            _ => None,
        }
    }

    /// Keys the popup already binds by logical name, distinct from a digit
    /// quick-select — see `digit_pressed` for why this matters.
    fn binds_by_name(key: egui::Key) -> bool {
        use egui::Key;
        matches!(
            key,
            Key::ArrowUp
                | Key::ArrowDown
                | Key::ArrowLeft
                | Key::ArrowRight
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::Insert
                | Key::Delete
                | Key::Enter
        )
    }

    /// The visible-row index a quick-select digit key was just pressed
    /// for, if any (see `digit_index`).
    ///
    /// Matches on `physical_key` — the key's *position* — rather than
    /// egui's aggregated logical `key`, for the same reason
    /// `ui::config_window::capture_hotkey` does: winit folds Shift into
    /// the logical key, and `egui::Key::from_name` happens to name some
    /// shifted top-row glyphs (`!` → `Exclamationmark`) but not others
    /// (`@#$%^&*()` have no egui `Key`, so they fall back to the physical
    /// key already). Matching the logical key alone made Shift+1 a dead
    /// shortcut — on a layout where Shift+1 types `!`, no `Key::Num1`
    /// event was ever produced — while Shift+2..Shift+0 worked purely by
    /// accident of `from_name`'s gaps.
    ///
    /// The `binds_by_name` guard exists because egui-winit maps both
    /// `KeyCode::Digit1` and `KeyCode::Numpad1` to the same `Key::Num1`:
    /// with NumLock off, a numpad press already has a meaning
    /// (Home/End/arrows) via its *logical* key, and a bare physical
    /// fallback would hijack it into selecting-and-delivering a row
    /// instead.
    pub fn digit_pressed(events: &[egui::Event]) -> Option<usize> {
        events.iter().find_map(|event| {
            let egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                ..
            } = event
            else {
                return None;
            };
            if let Some(idx) = digit_index(*key) {
                return Some(idx);
            }
            if binds_by_name(*key) {
                return None;
            }
            physical_key.and_then(digit_index)
        })
    }

    /// What the popup displays once `Shown` (plan §1/§9).
    pub enum Content {
        Prompting {
            password: Zeroizing<String>,
            error: Option<String>,
            /// Whether the field is showing plaintext instead of bullets —
            /// still never an `egui::TextEdit` (see the module doc's F6
            /// note), so revealing it doesn't reopen the leak that
            /// avoiding `TextEdit` was for; it's a plain painted label
            /// either way.
            revealed: bool,
        },
        Unlocking,
        /// Waiting on `bw lock` to finish — shown instead of leaving the
        /// popup blank or jumping straight to the prompt if it's opened
        /// (or already open) while a lock triggered from the tray is
        /// still in flight.
        Locking,
        /// Waiting on the tray's Sync item: `bw sync` + `bw list items`
        /// against the retained session, no re-unlock needed. Unlike
        /// `Locking`, `cached_entries` is deliberately left untouched
        /// while this is in flight — a failed sync must not destroy an
        /// otherwise working list.
        Syncing,
        ShowingList {
            selected: usize,
            /// An inline reason a requested field couldn't be delivered
            /// (no username, no TOTP configured) — cleared on the next
            /// selection change or delivery attempt, not a lingering
            /// banner.
            message: Option<String>,
        },
        /// Waiting on `bw get totp` for the item at `selected` (in the
        /// same cached list `ShowingList` reads from). Unlike
        /// `Unlocking`/`Locking` there's a natural "back" — Escape
        /// returns to `ShowingList { selected, .. }` instead of hiding
        /// the whole popup.
        FetchingOtp {
            selected: usize,
        },
        /// The settings screen (M7). A screen within the *same* popup
        /// window rather than a separate child viewport:
        /// `show_viewport_immediate` panics when called while the root
        /// viewport is hidden (which is exactly when the tray's Settings
        /// item opens it) — eframe's glow backend can't upgrade the
        /// `Weak` GL-context references it needs on that code path, so
        /// the render callback silently never runs. Folding Settings into
        /// the existing show/hide machinery sidesteps the bug entirely
        /// instead of working around eframe internals.
        Settings(config_window::ConfigWindowState),
        /// The About screen: version, license, copyright — read-only, no
        /// state to carry, so a unit variant is enough. Same
        /// same-window-not-a-separate-viewport reasoning as `Settings`
        /// above.
        About,
    }

    impl Content {
        pub fn fresh_prompt() -> Self {
            Self::Prompting {
                password: Zeroizing::new(String::with_capacity(256)),
                error: None,
                revealed: false,
            }
        }
    }

    /// Which field Enter/a digit delivers, driven by which modifier is
    /// held. The render side (`ui::popup::IconMode`) mirrors this
    /// one-to-one.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum DeliveryKind {
        Password,
        Username,
        Otp,
    }

    impl DeliveryKind {
        /// Shift wins over Alt if somehow both are held — an arbitrary
        /// but documented tie-break, not a meaningful combination either
        /// way.
        pub fn from_modifiers(modifiers: egui::Modifiers) -> Self {
            if modifiers.shift {
                Self::Username
            } else if modifiers.alt {
                Self::Otp
            } else {
                Self::Password
            }
        }

        pub fn icon_mode(self) -> IconMode {
            match self {
                Self::Password => IconMode::Password,
                Self::Username => IconMode::Username,
                Self::Otp => IconMode::Otp,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn key_event(key: egui::Key, physical_key: Option<egui::Key>) -> egui::Event {
            egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }
        }

        #[test]
        fn plain_digit_selects_by_index() {
            let events = [key_event(egui::Key::Num1, Some(egui::Key::Num1))];
            assert_eq!(digit_pressed(&events), Some(0));
        }

        #[test]
        fn shift_1_falls_back_to_physical_key() {
            // Regression: on layouts where Shift+1 types "!", winit's
            // logical key is `Exclamationmark`, not `Num1` — only the
            // physical key still says "1".
            let events = [key_event(egui::Key::Exclamationmark, Some(egui::Key::Num1))];
            assert_eq!(digit_pressed(&events), Some(0));
        }

        #[test]
        fn shift_2_still_works_when_logical_key_already_matches() {
            // The accidental-success case: `egui::Key::from_name` has no
            // entry for "@", so this already arrived as a physical
            // fallback before the fix — must keep working after it.
            let events = [key_event(egui::Key::Num2, Some(egui::Key::Num2))];
            assert_eq!(digit_pressed(&events), Some(1));
        }

        #[test]
        fn other_shifted_glyphs_fall_back_to_physical_key() {
            // German-layout shapes: Shift+7 types "/", Shift+0 types "=".
            let events = [key_event(egui::Key::Slash, Some(egui::Key::Num7))];
            assert_eq!(digit_pressed(&events), Some(6));
            let events = [key_event(egui::Key::Equals, Some(egui::Key::Num0))];
            assert_eq!(digit_pressed(&events), Some(9));
        }

        #[test]
        fn numlock_off_numpad_keeps_its_navigation_meaning() {
            // Numpad 1 with NumLock off arrives logically as `End`,
            // physically as `Num1` — must not be hijacked into a
            // quick-select.
            let events = [key_event(egui::Key::End, Some(egui::Key::Num1))];
            assert_eq!(digit_pressed(&events), None);
        }

        #[test]
        fn ignores_key_up_events() {
            let mut event = key_event(egui::Key::Num1, Some(egui::Key::Num1));
            if let egui::Event::Key { pressed, .. } = &mut event {
                *pressed = false;
            }
            assert_eq!(digit_pressed(&[event]), None);
        }

        #[test]
        fn ignores_non_digit_keys() {
            let events = [key_event(egui::Key::A, Some(egui::Key::A))];
            assert_eq!(digit_pressed(&events), None);
        }
    }
}

#[cfg(windows)]
pub use win::{digit_pressed, Content, DeliveryKind};
