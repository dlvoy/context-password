//! Autostart via the `HKCU\...\Run` registry key (plan §9) — the standard,
//! no-elevation-needed mechanism for "start with Windows." Not the
//! `auto-launch` crate: this is a few dozen lines against `windows-sys`,
//! which is already in the dependency tree, for the same handful of
//! registry calls, without pulling in a cross-platform surface this app
//! (Windows-only) never uses.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "context-password";

/// Always available on Windows — any executable can add itself to the Run
/// key, unlike macOS's `SMAppService`, which needs a real `.app` bundle.
/// Exists so `mac_ui::settings` can gate the checkbox through one
/// `platform::autostart::is_available()` call on either target — Windows'
/// own Settings UI has no equivalent gate to call this from (the checkbox
/// there is unconditionally enabled), so this is legitimately unused on a
/// Windows build specifically.
#[cfg_attr(windows, allow(dead_code))]
pub fn is_available() -> bool {
    true
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

fn open_run_key(write: bool) -> Option<HKEY> {
    let subkey = wide(RUN_KEY);
    let mut hkey: HKEY = ptr::null_mut();
    let access = if write { KEY_WRITE } else { KEY_READ };
    let result =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, access, &mut hkey) };
    (result == ERROR_SUCCESS).then_some(hkey)
}

/// Reads the value back from the registry rather than trusting the config
/// file, since the user may have disabled it via Task Manager's Startup tab
/// — the Settings checkbox should reflect reality, not our last write.
pub fn is_enabled() -> bool {
    let Some(hkey) = open_run_key(false) else {
        return false;
    };
    let value_name = wide(VALUE_NAME);
    let mut kind = 0u32;
    let mut size = 0u32;
    let result = unsafe {
        RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut size,
        )
    };
    unsafe { RegCloseKey(hkey) };
    result == ERROR_SUCCESS && kind == REG_SZ
}

/// Adds or removes the Run value. When adding, points it at the current
/// process's own executable, quoted (install paths with spaces are common
/// under `%ProgramFiles%`).
pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let Some(hkey) = open_run_key(true) else {
        return Err("could not open the Run registry key".to_string());
    };
    let value_name = wide(VALUE_NAME);

    let result = if enabled {
        let exe = std::env::current_exe()
            .map_err(|e| format!("could not determine the executable path: {e}"))?;
        let command = format!("\"{}\"", exe.display());
        let data = wide(&command);
        let data_bytes: &[u8] =
            unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
        unsafe {
            RegSetValueExW(
                hkey,
                value_name.as_ptr(),
                0,
                REG_SZ,
                data_bytes.as_ptr(),
                data_bytes.len() as u32,
            )
        }
    } else {
        let deleted = unsafe { RegDeleteValueW(hkey, value_name.as_ptr()) };
        // Deleting a value that's already absent isn't a failure from the
        // caller's point of view — the end state (not present) matches.
        if deleted == ERROR_FILE_NOT_FOUND {
            ERROR_SUCCESS
        } else {
            deleted
        }
    };

    unsafe { RegCloseKey(hkey) };
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registry operation failed (error {result})"))
    }
}
