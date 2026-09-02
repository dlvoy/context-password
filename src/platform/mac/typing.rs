//! `CGEventPost`-based Unicode typing. Layout-independent, same as
//! `win::typing`'s `KEYEVENTF_UNICODE` approach: the virtual keycode is
//! always 0 and the character comes from `CGEventKeyboardSetUnicodeString`
//! instead, so a `ł` types correctly regardless of the active keyboard
//! layout.
//!
//! Proven working on this machine (macOS 26.6.2 / Tahoe, the version the
//! port plan flagged as the autotype risk) by the Phase 0 spike: plain-text
//! injection via this exact approach lands in TextEdit, VS Code, and
//! Chrome.

use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
};

/// Written to the event's `EventSourceUserData` field so our own injected
/// input is recognizable — 'CPWD' in ASCII, the same signature
/// `win::typing::INJECTION_SIGNATURE` uses.
const INJECTION_SIGNATURE: i64 = 0x4350_5744;

/// `CGEventKeyboardSetUnicodeString`'s `UniChar` buffer is interpreted as
/// UTF-16 code units; keep each event's chunk comfortably under any
/// internal limit and small enough that a surrogate pair is never split
/// across a chunk boundary (`push_chunk` below guarantees that directly by
/// building chunks from `char`s, not raw units).
const MAX_CHUNK_UNITS: usize = 20;

/// The virtual keycodes for the two physical Return keys — checked
/// alongside the modifier keys since holding either while typing would have
/// the target interpret a synthetic character as "confirm" rather than
/// literal text, exactly as `win::typing::modifiers_up` treats `VK_RETURN`.
const KC_RETURN: u16 = 0x24;
const KC_KEYPAD_ENTER: u16 = 0x4c;

/// True once none of Cmd/Shift/Option/Control and neither Return key are
/// physically held. The user is still holding the hotkey's modifiers (and
/// just pressed Enter) at the moment delivery starts; typing while any of
/// them are down would have the target interpret the keystroke as a
/// shortcut instead of literal text.
pub fn modifiers_up() -> bool {
    let held = CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState);
    let blocking = CGEventFlags::MaskCommand
        | CGEventFlags::MaskShift
        | CGEventFlags::MaskAlternate
        | CGEventFlags::MaskControl;
    if held.intersects(blocking) {
        return false;
    }
    let key_down = |code: u16| CGEventSource::key_state(CGEventSourceStateID::CombinedSessionState, code);
    !key_down(KC_RETURN) && !key_down(KC_KEYPAD_ENTER)
}

/// Splits `text` into UTF-16 chunks of at most `max_units` code units each,
/// dropping control characters (returned as `skipped`) and never splitting
/// a surrogate pair across a chunk boundary — building chunks from
/// `char`s rather than raw units is what guarantees that directly, no
/// separate boundary check needed. Pure and OS-call-free specifically so
/// it's unit-testable without a running `NSApplication` — none of the
/// `objc2` calls in this module can run outside one (`MainThreadMarker`
/// wouldn't be available from a plain `cargo test` worker thread either).
fn chunk_utf16(text: &str, max_units: usize) -> (Vec<Vec<u16>>, usize) {
    let mut chunks = Vec::new();
    let mut skipped = 0usize;
    let mut current: Vec<u16> = Vec::with_capacity(max_units);

    for ch in text.chars() {
        if ch.is_control() {
            skipped += 1;
            continue;
        }
        let mut buf = [0u16; 2];
        let units = ch.encode_utf16(&mut buf);
        if current.len() + units.len() > max_units && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        current.extend_from_slice(units);
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    (chunks, skipped)
}

/// Types `text` into whatever window currently has keyboard focus, via
/// `CGEventKeyboardSetUnicodeString` — layout-independent. Returns the
/// number of control characters that were skipped. Never logs or returns
/// *which* characters — this may run on a password.
pub fn send_unicode(text: &str) -> usize {
    let (mut chunks, skipped) = chunk_utf16(text, MAX_CHUNK_UNITS);

    let Some(source) = CGEventSource::new(CGEventSourceStateID::Private) else {
        return text.chars().count();
    };
    for chunk in &mut chunks {
        post_chunk(&source, chunk);
        // Held one chunk of the text that was just typed — scrub it before
        // it drops, mirroring `win::typing::send_unicode`'s wScan scrub.
        chunk.fill(0);
    }

    skipped
}

fn post_chunk(source: &CFRetained<CGEventSource>, chunk: &[u16]) {
    for key_down in [true, false] {
        let Some(event) = CGEvent::new_keyboard_event(Some(source), 0, key_down) else {
            continue;
        };
        CGEvent::set_flags(Some(&event), CGEventFlags::empty());
        unsafe {
            CGEvent::keyboard_set_unicode_string(Some(&event), chunk.len() as _, chunk.as_ptr());
        }
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUserData,
            INJECTION_SIGNATURE,
        );
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_text_fits_in_one_chunk() {
        let (chunks, skipped) = chunk_utf16("hello", 20);
        assert_eq!(skipped, 0);
        assert_eq!(chunks, vec![vec![
            'h' as u16, 'e' as u16, 'l' as u16, 'l' as u16, 'o' as u16
        ]]);
    }

    #[test]
    fn control_characters_are_skipped_and_counted() {
        let (chunks, skipped) = chunk_utf16("a\tb\nc", 20);
        assert_eq!(skipped, 2);
        let flat: Vec<u16> = chunks.into_iter().flatten().collect();
        assert_eq!(flat, vec!['a' as u16, 'b' as u16, 'c' as u16]);
    }

    #[test]
    fn splits_into_multiple_chunks_at_the_boundary() {
        // Greedy packing: 2 units per chunk fits "ab" exactly, then "c"
        // starts a new chunk rather than being dropped or corrupted.
        let (chunks, _) = chunk_utf16("abc", 2);
        assert_eq!(chunks, vec![vec!['a' as u16, 'b' as u16], vec!['c' as u16]]);
    }

    /// The whole reason `chunk_utf16` builds from `char`s instead of raw
    /// UTF-16 units: a naive `.encode_utf16().collect::<Vec<_>>().chunks(n)`
    /// can slice a surrogate pair in half, corrupting a non-BMP character
    /// (most emoji, some symbols) silently. `✓` is a real regression guard
    /// from the port plan's own verification list ("non-ASCII password,
    /// e.g. pässwörd–✓").
    #[test]
    fn never_splits_a_surrogate_pair_across_chunks() {
        // U+1F600 GRINNING FACE — a real 2-unit surrogate pair, not a
        // contrived example.
        let text = "a😀b";
        let (chunks, skipped) = chunk_utf16(text, 2);
        assert_eq!(skipped, 0);
        for chunk in &chunks {
            // Every chunk must decode back to whole chars — a split
            // surrogate pair would fail `char::decode_utf16` here.
            assert!(
                char::decode_utf16(chunk.iter().copied()).all(|r| r.is_ok()),
                "chunk {chunk:?} contains a split surrogate pair"
            );
        }
        let flat: Vec<u16> = chunks.into_iter().flatten().collect();
        assert_eq!(flat, text.encode_utf16().collect::<Vec<_>>());
    }

    #[test]
    fn non_ascii_password_round_trips_through_chunking() {
        let text = "pässwörd–✓";
        let (chunks, skipped) = chunk_utf16(text, MAX_CHUNK_UNITS);
        assert_eq!(skipped, 0);
        let flat: Vec<u16> = chunks.into_iter().flatten().collect();
        let decoded: String = char::decode_utf16(flat).map(|r| r.unwrap()).collect();
        assert_eq!(decoded, text);
    }

    #[test]
    fn empty_text_produces_no_chunks() {
        let (chunks, skipped) = chunk_utf16("", 20);
        assert!(chunks.is_empty());
        assert_eq!(skipped, 0);
    }
}
