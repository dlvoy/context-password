//! Target-process integrity-level check for the UIPI pre-check (plan §5).
//!
//! `SendInput`'s failure under UIPI is not reported via its return value or
//! `GetLastError` (see plan F11), so detection has to happen *before* the
//! user commits to typing — at hotkey-capture time — rather than after a
//! send that silently did nothing.

use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_MANDATORY_LABEL,
    TOKEN_QUERY, TokenIntegrityLevel,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Medium integrity (0x2000) is what a normal, non-elevated process runs
/// at. Used as the fallback when a token can't be read at all, which should
/// only happen for our own process in practice.
const INTEGRITY_MEDIUM: u32 = 0x2000;

/// `TOKEN_MANDATORY_LABEL` contains a pointer (`Sid: PSID`), which needs
/// 8-byte alignment on 64-bit — a plain `[u8; 64]` only guarantees
/// alignment 1, so casting its pointer straight to `*const
/// TOKEN_MANDATORY_LABEL` and dereferencing it is undefined behavior (this
/// was caught immediately by the aligned-pointer-dereference check).
#[repr(align(8))]
struct AlignedTokenBuf([u8; 64]);

/// A `TOKEN_MANDATORY_LABEL` plus its SID always fits comfortably in a
/// fixed-size buffer, avoiding the usual two-call `GetTokenInformation`
/// dance (query the required size, then fetch).
fn integrity_rid_of_token(token: HANDLE) -> Option<u32> {
    let mut buf = AlignedTokenBuf([0u8; 64]);
    let mut returned = 0u32;
    unsafe {
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buf.0.as_mut_ptr().cast(),
            buf.0.len() as u32,
            &mut returned,
        );
        if ok == 0 {
            return None;
        }
        let label = &*(buf.0.as_ptr().cast::<TOKEN_MANDATORY_LABEL>());
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(sid, (count - 1) as u32))
    }
}

/// Our own process's integrity RID, meant to be read once at startup and
/// cached — every target gets compared against this baseline.
pub fn our_integrity_rid() -> u32 {
    unsafe {
        let mut token: HANDLE = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return INTEGRITY_MEDIUM;
        }
        let rid = integrity_rid_of_token(token).unwrap_or(INTEGRITY_MEDIUM);
        CloseHandle(token);
        rid
    }
}

/// Whether the process owning `pid` runs at a higher integrity level than
/// `our_rid` — the condition under which `SendInput` targeting one of its
/// windows will be silently dropped by UIPI. Being unable to even open the
/// process or its token (`ACCESS_DENIED`) is itself treated as "higher": a
/// process we can't query for its token is almost always elevated relative
/// to us.
pub fn target_is_higher(pid: u32, our_rid: u32) -> bool {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return true;
        }
        let mut token: HANDLE = ptr::null_mut();
        let opened = OpenProcessToken(handle, TOKEN_QUERY, &mut token);
        CloseHandle(handle);
        if opened == 0 {
            return true;
        }
        let their_rid = integrity_rid_of_token(token);
        CloseHandle(token);
        their_rid.is_none_or(|rid| rid > our_rid)
    }
}
