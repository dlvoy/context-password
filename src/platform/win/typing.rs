//! `SendInput`-based Unicode typing (plan §4 "Typing") and the modifier
//! check that gates it.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VK_CONTROL, VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT,
};

/// Written to `dwExtraInfo` so our own injected input is recognizable (by
/// us, or by other tools inspecting the input stream) — 'CPWD' in ASCII.
const INJECTION_SIGNATURE: usize = 0x4350_5744;

/// True once none of Ctrl/Alt/Shift/either-Win/Enter are physically held.
/// The user is still holding the hotkey's modifiers (and just pressed
/// Enter) at the moment delivery starts; typing while any of them are down
/// would have the target interpret `WM_CHAR` as a shortcut or a control
/// character instead of a literal character.
pub fn modifiers_up() -> bool {
    const KEYS: [u16; 6] = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_RETURN];
    KEYS.iter().all(|&vk| {
        let state = unsafe { GetAsyncKeyState(vk as i32) } as u16;
        state & 0x8000 == 0
    })
}

/// Types `text` into whatever window currently has keyboard focus, via
/// `KEYEVENTF_UNICODE` — layout-independent, so it works regardless of the
/// user's keyboard layout (a `ł` types correctly on a US layout).
///
/// Returns the number of control characters that were skipped. Never logs
/// or returns *which* characters — this may run on a password.
pub fn send_unicode(text: &str) -> usize {
    let mut skipped = 0usize;
    let mut inputs: Vec<INPUT> = Vec::with_capacity(text.len() * 2);

    // `encode_utf16` yields a surrogate pair as two adjacent code units for
    // any non-BMP character (e.g. most emoji) — exactly the two down/up
    // pairs Windows expects to recombine them, as long as they stay in the
    // same SendInput batch, which they do here.
    for unit in text.encode_utf16() {
        if unit < 0x20 || unit == 0x7f {
            skipped += 1;
            continue;
        }
        push_key_event(&mut inputs, unit, 0);
        push_key_event(&mut inputs, unit, KEYEVENTF_KEYUP);
    }

    if !inputs.is_empty() {
        // One call so the whole payload is inserted into the input queue
        // atomically — nothing else can interleave with it.
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
    }

    // The wScan field of every event above holds one UTF-16 code unit of
    // the text that was just typed — scrub the buffer before it's dropped.
    // (Writing to a union field, unlike reading one, is not `unsafe`.)
    for input in inputs.iter_mut() {
        input.Anonymous.ki.wScan = 0;
    }

    skipped
}

fn push_key_event(inputs: &mut Vec<INPUT>, code_unit: u16, extra_flags: u32) {
    inputs.push(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0, // must be 0 with KEYEVENTF_UNICODE
                wScan: code_unit,
                dwFlags: KEYEVENTF_UNICODE | extra_flags,
                time: 0,
                dwExtraInfo: INJECTION_SIGNATURE,
            },
        },
    });
}
