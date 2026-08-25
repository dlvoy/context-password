//! The settings window (plan M7): a hotkey recorder and the autostart
//! toggle. Drawn as a `Content::Settings` screen inside the app's one
//! popup window rather than a separate viewport — see that variant's doc
//! in `app.rs` for why (`show_viewport_immediate` panics when invoked
//! while the root viewport is hidden, which is exactly when the tray's
//! Settings item needs to open it).
//!
//! Everything below `draw` that isn't pure UI drawing (`capture_hotkey`,
//! `key_name`) takes plain `egui` input types and returns plain values, so
//! it can be unit tested without a live `Ui`.

use eframe::egui;

use crate::config::{MAX_VISIBLE_ITEMS, MIN_VISIBLE_ITEMS};

const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xcc, 0x33, 0x33);
const OK_COLOR: egui::Color32 = egui::Color32::from_rgb(0x33, 0x99, 0x33);
const LISTENING_COLOR: egui::Color32 = egui::Color32::from_rgb(0xcc, 0x88, 0x00);
/// Scales every text style up for this dialog only — the popup itself stays
/// at egui's defaults, this is specifically "bigger fonts" for Settings.
const FONT_SCALE: f32 = 1.25;
const BODY_MARGIN: f32 = 16.0;
/// Extra room inside every button in this dialog — egui's default padding
/// reads as cramped once the text itself is scaled up.
const BUTTON_PADDING: egui::Vec2 = egui::Vec2::new(12.0, 8.0);
const STEP_BUTTON_WIDTH: f32 = 32.0;

/// Working copy of the settings being edited — separate from the live
/// `Config` until Save is clicked, so a cancelled dialog changes nothing.
#[derive(Clone)]
pub struct ConfigWindowState {
    pub hotkey_spec: String,
    pub recording: bool,
    pub autostart: bool,
    pub lock_on_exit: bool,
    /// Whether the vault should prompt for the master password
    /// automatically a while after startup (`UnlockMode::Delayed`) or only
    /// once the popup is first opened (`UnlockMode::Lazy`).
    pub auto_unlock: bool,
    pub max_visible_items: u32,
    pub message: Option<(String, bool)>,
    /// Physical modifier keys currently held while recording, tracked by
    /// hand rather than trusting egui's aggregated `Modifiers` for this —
    /// two real Windows/winit quirks make that unreliable here: (1) winit
    /// zeroes *both* the Ctrl and Alt flags system-wide while AltGr (right
    /// Alt) is held, on any layout that has one, since AltGr is
    /// conventionally its own thing rather than literal Ctrl+Alt; and (2)
    /// once Alt is held, Windows delivers the *other* key of the chord as
    /// `WM_SYSKEYDOWN`, and that down-edge sometimes never reaches egui as
    /// a normal `pressed: true` event at all — only the matching key-up
    /// does (confirmed by logging the raw events: Ctrl+(left)Alt+V arrived
    /// as only a key-*up* for V, correctly still carrying `ctrl`/`alt` in
    /// its own per-event modifiers, but never a key-down). Tracking the
    /// physical modifier keys' own down/up transitions sidesteps both,
    /// since neither quirk touches those events themselves.
    pub held: HeldMods,
}

/// See `ConfigWindowState::held`. AltGr counts as *both* Ctrl and Alt when
/// building the combo string, matching the real Windows convention that
/// `RegisterHotKey(MOD_CONTROL | MOD_ALT, vk)` fires on either genuine
/// Ctrl+Alt or on AltGr.
#[derive(Clone, Copy, Default)]
pub struct HeldMods {
    ctrl: bool,
    alt: bool,
    alt_gr: bool,
    shift: bool,
}

pub enum Action {
    None,
    Save,
    Cancel,
}

pub fn draw(ui: &mut egui::Ui, state: &mut ConfigWindowState) -> Action {
    // Checked before anything else so it takes effect even if a widget
    // below happens to consume the same frame's input first.
    if !state.recording && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return Action::Cancel;
    }

    // Scale this Ui's text styles up for the whole dialog — done once here
    // rather than per-widget, and as a ratio (not a fixed size) so headings
    // stay bigger than body text instead of flattening the hierarchy. Also
    // bump button padding — the default reads as cramped once button text
    // itself is bigger too.
    let style = ui.style_mut();
    for font_id in style.text_styles.values_mut() {
        font_id.size *= FONT_SCALE;
    }
    style.spacing.button_padding = BUTTON_PADDING;

    let mut action = Action::None;

    egui::Panel::bottom("settings_footer")
        .show_separator_line(true)
        .show(ui, |ui| {
            // Uniform on all four sides — left of Cancel, right of Save,
            // and above the row all match the gap already below it (the
            // panel's own bottom edge), instead of the wider left/right
            // margin used elsewhere in the dialog.
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    let (cancel_clicked, save_clicked) = egui::Sides::new().show(
                        ui,
                        |ui| ui.button("Cancel").clicked(),
                        |ui| ui.button("Save").clicked(),
                    );
                    if cancel_clicked {
                        action = Action::Cancel;
                    }
                    if save_clicked {
                        action = Action::Save;
                    }
                });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(BODY_MARGIN as i8)))
        .show(ui, |ui| {
            ui.heading("Context Password | Settings");
            ui.separator();
            ui.add_space(8.0);

            ui.label("Global hotkey:");
            egui::Sides::new().show(
                ui,
                |ui| {
                    ui.monospace(&state.hotkey_spec);
                },
                |ui| {
                    let label = if state.recording {
                        "Enter hotkeys…"
                    } else {
                        "Record…"
                    };
                    if ui.button(label).clicked() && !state.recording {
                        state.recording = true;
                        state.message = None;
                        state.held = HeldMods::default();
                    }
                },
            );

            if state.recording {
                let events = ui.input(|i| i.events.clone());
                match capture_hotkey(&events, &mut state.held) {
                    Capture::Waiting => {
                        ui.colored_label(
                            LISTENING_COLOR,
                            "Listening — press your desired combination, or Esc to cancel.",
                        );
                    }
                    Capture::Cancelled => {
                        state.recording = false;
                    }
                    Capture::Rejected(msg) => {
                        state.message = Some((msg, true));
                    }
                    Capture::Captured(spec) => {
                        state.hotkey_spec = spec;
                        state.recording = false;
                        state.message = None;
                    }
                }
            }

            ui.add_space(12.0);
            ui.checkbox(&mut state.autostart, "Start with Windows");
            ui.add_space(6.0);
            ui.checkbox(&mut state.auto_unlock, "Auto-unlock at start");
            ui.add_space(6.0);
            ui.checkbox(&mut state.lock_on_exit, "Lock vault on exit");

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label("Max visible items:");
                let button = |text| egui::Button::new(text).min_size(egui::vec2(STEP_BUTTON_WIDTH, 0.0));
                if ui.add(button("-")).clicked() {
                    state.max_visible_items =
                        state.max_visible_items.saturating_sub(1).max(MIN_VISIBLE_ITEMS);
                }
                ui.monospace(state.max_visible_items.to_string());
                if ui.add(button("+")).clicked() {
                    state.max_visible_items =
                        (state.max_visible_items + 1).min(MAX_VISIBLE_ITEMS);
                }
            });

            ui.add_space(12.0);
            if let Some((msg, is_error)) = &state.message {
                ui.colored_label(if *is_error { ERROR_COLOR } else { OK_COLOR }, msg.as_str());
            }
        });

    action
}

enum Capture {
    /// No relevant key event yet this frame — keep waiting.
    Waiting,
    /// A full, valid combination was captured.
    Captured(String),
    /// A key event arrived but couldn't become a hotkey (unsupported key,
    /// or no modifier held) — stays in recording mode.
    Rejected(String),
    /// Esc pressed with no modifier held — the user wants out.
    Cancelled,
}

/// Turns this frame's raw key events into a hotkey spec string in the form
/// `global_hotkey::hotkey::HotKey::from_str` accepts (e.g. "Ctrl+Alt+KeyV"),
/// updating `held` from each event's `physical_key` along the way (see
/// `ConfigWindowState::held` for why that, rather than egui's aggregated
/// `Modifiers`, is the source of truth here). Pure function of its inputs —
/// no `Ui` needed — so it's unit tested directly below.
fn capture_hotkey(events: &[egui::Event], held: &mut HeldMods) -> Capture {
    for event in events {
        let egui::Event::Key { key, physical_key, pressed, repeat, .. } = event else {
            continue;
        };
        if let Some(phys) = physical_key {
            match phys {
                egui::Key::ControlLeft | egui::Key::ControlRight => held.ctrl = *pressed,
                egui::Key::AltLeft => held.alt = *pressed,
                egui::Key::AltRight => held.alt_gr = *pressed,
                egui::Key::ShiftLeft | egui::Key::ShiftRight => held.shift = *pressed,
                _ => {}
            }
        }
        if is_modifier_key(*key) || *repeat {
            // A bare modifier press isn't a complete combination yet — keep
            // listening for the key that follows it. Auto-repeat of an
            // already-held key isn't a new press either.
            continue;
        }
        // AltGr counts as both: see `HeldMods`'s doc.
        let ctrl = held.ctrl || held.alt_gr;
        let alt = held.alt || held.alt_gr;
        let has_modifier = ctrl || alt || held.shift;
        if *key == egui::Key::Escape && !has_modifier {
            return Capture::Cancelled;
        }
        let Some(name) = key_name(*key) else {
            return Capture::Rejected(format!("{key:?} isn't supported in a hotkey"));
        };
        if !has_modifier {
            return Capture::Rejected("hold at least one of Ctrl / Alt / Shift".to_string());
        }
        let mut combo = String::new();
        if ctrl {
            combo.push_str("Ctrl+");
        }
        if alt {
            combo.push_str("Alt+");
        }
        if held.shift {
            combo.push_str("Shift+");
        }
        combo.push_str(name);
        return Capture::Captured(combo);
    }
    Capture::Waiting
}

fn is_modifier_key(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::ShiftLeft
            | egui::Key::ShiftRight
            | egui::Key::ControlLeft
            | egui::Key::ControlRight
            | egui::Key::AltLeft
            | egui::Key::AltRight
            | egui::Key::SuperLeft
            | egui::Key::SuperRight
    )
}

/// Maps an egui key to the name `global_hotkey`'s parser accepts. Anything
/// not listed here (punctuation, Copy/Cut/Paste, F25+, …) falls through to
/// `None` and is reported to the user as unsupported rather than silently
/// producing a hotkey that won't parse.
fn key_name(key: egui::Key) -> Option<&'static str> {
    use egui::Key::*;
    Some(match key {
        A => "KeyA",
        B => "KeyB",
        C => "KeyC",
        D => "KeyD",
        E => "KeyE",
        F => "KeyF",
        G => "KeyG",
        H => "KeyH",
        I => "KeyI",
        J => "KeyJ",
        K => "KeyK",
        L => "KeyL",
        M => "KeyM",
        N => "KeyN",
        O => "KeyO",
        P => "KeyP",
        Q => "KeyQ",
        R => "KeyR",
        S => "KeyS",
        T => "KeyT",
        U => "KeyU",
        V => "KeyV",
        W => "KeyW",
        X => "KeyX",
        Y => "KeyY",
        Z => "KeyZ",
        Num0 => "Digit0",
        Num1 => "Digit1",
        Num2 => "Digit2",
        Num3 => "Digit3",
        Num4 => "Digit4",
        Num5 => "Digit5",
        Num6 => "Digit6",
        Num7 => "Digit7",
        Num8 => "Digit8",
        Num9 => "Digit9",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        F13 => "F13",
        F14 => "F14",
        F15 => "F15",
        F16 => "F16",
        F17 => "F17",
        F18 => "F18",
        F19 => "F19",
        F20 => "F20",
        F21 => "F21",
        F22 => "F22",
        F23 => "F23",
        F24 => "F24",
        ArrowUp => "ArrowUp",
        ArrowDown => "ArrowDown",
        ArrowLeft => "ArrowLeft",
        ArrowRight => "ArrowRight",
        Space => "SPACE",
        Tab => "TAB",
        Enter => "ENTER",
        Backspace => "BACKSPACE",
        Insert => "INSERT",
        Delete => "DELETE",
        Home => "HOME",
        End => "END",
        PageUp => "PAGEUP",
        PageDown => "PAGEDOWN",
        Escape => "ESCAPE",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn key_event(key: egui::Key) -> egui::Event {
        key_event_with(key, None, true, false)
    }

    fn key_event_with(
        key: egui::Key,
        physical_key: Option<egui::Key>,
        pressed: bool,
        repeat: bool,
    ) -> egui::Event {
        egui::Event::Key { key, physical_key, pressed, repeat, modifiers: egui::Modifiers::NONE }
    }

    fn held(ctrl: bool, alt: bool, alt_gr: bool, shift: bool) -> HeldMods {
        HeldMods { ctrl, alt, alt_gr, shift }
    }

    #[test]
    fn waits_when_no_key_event_present() {
        assert!(matches!(
            capture_hotkey(&[], &mut held(true, false, false, false)),
            Capture::Waiting
        ));
    }

    #[test]
    fn captures_a_plain_combo() {
        let events = [key_event(egui::Key::V)];
        match capture_hotkey(&events, &mut held(true, true, false, false)) {
            Capture::Captured(spec) => assert_eq!(spec, "Ctrl+Alt+KeyV"),
            _ => panic!("expected a capture"),
        }
    }

    #[test]
    fn ignores_a_bare_modifier_press() {
        let events = [key_event(egui::Key::ControlLeft)];
        assert!(matches!(
            capture_hotkey(&events, &mut held(true, false, false, false)),
            Capture::Waiting
        ));
    }

    #[test]
    fn rejects_a_key_with_no_modifier_held() {
        let events = [key_event(egui::Key::V)];
        assert!(matches!(
            capture_hotkey(&events, &mut held(false, false, false, false)),
            Capture::Rejected(_)
        ));
    }

    #[test]
    fn rejects_an_unsupported_key() {
        let events = [key_event(egui::Key::Colon)];
        match capture_hotkey(&events, &mut held(true, false, false, false)) {
            Capture::Rejected(_) => {}
            _ => panic!("Colon should be reported as unsupported"),
        }
    }

    #[test]
    fn escape_with_no_modifier_cancels() {
        let events = [key_event(egui::Key::Escape)];
        assert!(matches!(
            capture_hotkey(&events, &mut held(false, false, false, false)),
            Capture::Cancelled
        ));
    }

    #[test]
    fn escape_with_a_modifier_is_a_valid_combo() {
        let events = [key_event(egui::Key::Escape)];
        match capture_hotkey(&events, &mut held(true, false, false, false)) {
            Capture::Captured(spec) => assert_eq!(spec, "Ctrl+ESCAPE"),
            _ => panic!("expected a capture"),
        }
    }

    #[test]
    fn function_keys_map_correctly() {
        let events = [key_event(egui::Key::F5)];
        match capture_hotkey(&events, &mut held(false, true, false, false)) {
            Capture::Captured(spec) => assert_eq!(spec, "Alt+F5"),
            _ => panic!("expected a capture"),
        }
    }

    #[test]
    fn digit_keys_map_to_digit_names() {
        let events = [key_event(egui::Key::Num1)];
        match capture_hotkey(&events, &mut held(true, false, false, false)) {
            Capture::Captured(spec) => assert_eq!(spec, "Ctrl+Digit1"),
            _ => panic!("expected a capture"),
        }
    }

    /// Regression test for the bug this module was rewritten to fix: once
    /// Alt is held, Windows delivers the *other* key of the chord as
    /// `WM_SYSKEYDOWN`, and real-world logging showed that down-edge
    /// sometimes never reaches egui as a `pressed: true` event at all —
    /// only the matching key-up does, still correctly carrying physical
    /// Ctrl/Alt as held. The recorder must still capture from that alone.
    #[test]
    fn captures_on_key_up_when_the_key_down_never_arrives() {
        use egui::Key;
        let events = [
            key_event_with(Key::ControlLeft, Some(Key::ControlLeft), true, false),
            key_event_with(Key::AltLeft, Some(Key::AltLeft), true, false),
            key_event_with(Key::V, Some(Key::V), false, false),
        ];
        match capture_hotkey(&events, &mut HeldMods::default()) {
            Capture::Captured(spec) => assert_eq!(spec, "Ctrl+Alt+KeyV"),
            _ => panic!("expected a capture from the key-up alone"),
        }
    }

    /// Regression test for the other real quirk found alongside the one
    /// above: on a layout with AltGr, Windows/winit zeroes *both* the
    /// aggregated Ctrl and Alt modifier flags while right-Alt is held —
    /// egui's `Modifiers` can't be trusted for this combo at all, which is
    /// exactly why `held` is tracked from physical key events instead.
    #[test]
    fn altgr_counts_as_ctrl_and_alt() {
        use egui::Key;
        let events = [
            key_event_with(Key::AltRight, Some(Key::AltRight), true, false),
            key_event_with(Key::V, Some(Key::V), true, false),
        ];
        match capture_hotkey(&events, &mut HeldMods::default()) {
            Capture::Captured(spec) => assert_eq!(spec, "Ctrl+Alt+KeyV"),
            _ => panic!("expected AltGr+V to capture as Ctrl+Alt+KeyV"),
        }
    }

    /// In practice the modifiers are usually held for many frames (and
    /// therefore many separate `capture_hotkey` calls) before the target
    /// key arrives — `held` has to accumulate across calls, not just
    /// within one.
    #[test]
    fn held_state_persists_across_separate_frames() {
        use egui::Key;
        let mut h = HeldMods::default();
        let ctrl_down = [key_event_with(Key::ControlLeft, Some(Key::ControlLeft), true, false)];
        assert!(matches!(capture_hotkey(&ctrl_down, &mut h), Capture::Waiting));
        let alt_down = [key_event_with(Key::AltLeft, Some(Key::AltLeft), true, false)];
        assert!(matches!(capture_hotkey(&alt_down, &mut h), Capture::Waiting));
        let v_down = [key_event_with(Key::V, Some(Key::V), true, false)];
        match capture_hotkey(&v_down, &mut h) {
            Capture::Captured(spec) => assert_eq!(spec, "Ctrl+Alt+KeyV"),
            _ => panic!("expected capture using held state built up across frames"),
        }
    }

    /// Every string this module can produce must actually parse — a
    /// mismatch here would mean `Hotkey::set` fails on a combo the recorder
    /// just told the user was valid.
    #[test]
    fn every_capturable_combo_parses_as_a_real_hotkey() {
        use egui::Key;
        let keys = [
            Key::A, Key::Z, Key::Num0, Key::Num9, Key::F1, Key::F24, Key::ArrowUp,
            Key::ArrowDown, Key::ArrowLeft, Key::ArrowRight, Key::Space, Key::Tab, Key::Enter,
            Key::Backspace, Key::Insert, Key::Delete, Key::Home, Key::End, Key::PageUp,
            Key::PageDown, Key::Escape,
        ];
        for key in keys {
            let events = [key_event(key)];
            match capture_hotkey(&events, &mut held(true, true, false, false)) {
                Capture::Captured(spec) => {
                    global_hotkey::hotkey::HotKey::from_str(&spec)
                        .unwrap_or_else(|e| panic!("{spec:?} from {key:?} failed to parse: {e}"));
                }
                _ => panic!("{key:?} should have been captured"),
            }
        }
    }
}
