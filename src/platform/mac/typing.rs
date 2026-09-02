//! `CGEventPost`-based Unicode typing — stub. Real implementation is
//! Phase 3 of the port plan; the approach is already proven by the Phase 0
//! spike (throwaway binary, not in this repo): `CGEventCreateKeyboardEvent`
//! plus `CGEventKeyboardSetUnicodeString`, chunked to <=20 UTF-16 units per
//! event, posted to `kCGSessionEventTapLocation`. Confirmed working on
//! this machine (macOS 26.6.2) into TextEdit, VS Code, and Chrome.

#![allow(unused)]

pub fn modifiers_up() -> bool {
    unimplemented!("macOS typing lands in Phase 3 of the port plan")
}

pub fn send_unicode(_text: &str) -> usize {
    unimplemented!("macOS typing lands in Phase 3 of the port plan")
}
