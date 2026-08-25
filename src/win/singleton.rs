//! Ensures only one instance of context-password runs at a time. Without
//! this, a second launch fails to register the global hotkey with a
//! confusing error and leaves two tray icons behind.

use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

/// Holds the OS mutex for the process lifetime. Dropping it (including on
/// normal exit) releases the instance slot for the next launch.
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// Returns `Some(guard)` if this is the only running instance, or `None`
    /// if another instance already holds the mutex.
    pub fn acquire() -> Option<Self> {
        // "Local\" scopes the mutex to this login session rather than the
        // whole machine, which is the right scope for a per-user tray app.
        let name: Vec<u16> = "Local\\context-password-9f3f2b7e-singleton\0"
            .encode_utf16()
            .collect();
        let handle = unsafe { CreateMutexW(ptr::null(), 1, name.as_ptr()) };
        if handle.is_null() {
            // Could not even create the mutex (unexpected). Fail open rather
            // than block startup on an OS-level anomaly we can't diagnose here.
            return Some(Self { handle });
        }
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_running {
            unsafe { CloseHandle(handle) };
            None
        } else {
            Some(Self { handle })
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
        }
    }
}
