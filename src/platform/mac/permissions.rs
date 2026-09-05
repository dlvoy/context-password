//! The two macOS-only preconditions for synthetic keystroke delivery,
//! neither of which has a Windows analogue: the Accessibility permission
//! (TCC-gated, required for `CGEventPost` to affect any process but our
//! own) and Secure Event Input (a session-wide flag some process has
//! turned on, which blocks synthetic keystrokes into *every* field on the
//! system while it's set, regardless of Accessibility). Feeds
//! `platform::BlockReason::{NoAccessibility, SecureInput}` — the direct
//! counterpart of Windows' UIPI elevation check (`win::integrity`), same
//! "silent failure, so pre-check and warn" shape, different predicate.

use core::ffi::{c_char, c_void};
use std::ptr::NonNull;

use objc2_application_services::AXIsProcessTrusted;
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};

use super::super::BlockReason;

// `IsSecureEventInputEnabled` is Carbon API with no objc2 binding — one
// manual declaration, confirmed present in this machine's SDK
// (`HIToolbox.tbd`, part of the Carbon umbrella framework).
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8; // Boolean
}

/// Whether this process has been granted Accessibility. Cheap (no IPC to
/// the TCC daemon on the fast path) — safe to call on every hotkey press,
/// not just once at startup, since the user can revoke it at any time in
/// System Settings.
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Whether *any* process in this login session currently holds Secure
/// Event Input. `IsSecureEventInputEnabled` is not scoped to a window, an
/// app, or "the field we're about to type into" — it's one flag per
/// session (confirmed against `ioreg -l | grep kCGSSessionSecureInputPID`,
/// which tracks the same underlying value `secure_input_holder` below
/// reads). A password field in a completely unrelated, background app can
/// set it and block `CGEventPost` everywhere else on the system until that
/// app clears it (or exits). This must be checked ahead of time, since a
/// blocked delivery is silently dropped rather than reported.
pub fn secure_input_active() -> bool {
    unsafe { IsSecureEventInputEnabled() != 0 }
}

/// The `BlockReason` for a delivery attempted right now, if any.
/// Accessibility is checked first since it blocks *everything*; Secure
/// Input is the session-wide flag described above.
pub fn block_reason() -> Option<BlockReason> {
    if !accessibility_trusted() {
        return Some(BlockReason::NoAccessibility);
    }
    if secure_input_active() {
        return Some(BlockReason::SecureInput);
    }
    None
}

// mach_port_t / io_object_t / io_service_t / io_registry_entry_t are all
// the same `unsigned int` typedef (IOKit/IOTypes.h); kern_return_t is
// `int`; IOOptionBits is `UInt32`. No objc2 binding exists for IOKit, so
// these are manual declarations, same pattern (and same justification) as
// `IsSecureEventInputEnabled` above.
type IoMachPort = u32;
type IoKernReturn = i32;
type IoOptionBits = u32;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    static kIOMainPortDefault: IoMachPort;
    fn IOServiceMatching(name: *const c_char) -> *mut c_void; // +1 CFMutableDictionaryRef
    fn IOServiceGetMatchingService(main_port: IoMachPort, matching: *mut c_void) -> IoMachPort;
    fn IORegistryEntryCreateCFProperty(
        entry: IoMachPort,
        key: *const c_void, // CFStringRef
        allocator: *const c_void, // CFAllocatorRef; null means "use the default allocator"
        options: IoOptionBits,
    ) -> *mut c_void; // +1 CFTypeRef, or null
    fn IOObjectRelease(object: IoMachPort) -> IoKernReturn;
}

/// Best-effort name of the app currently holding Secure Event Input, so the
/// warning built from `BlockReason::SecureInput` can say *who*, not just
/// *that*. `None` on any failure along the way (no matching IOKit service,
/// no property, nothing currently holds it, or the holding pid doesn't map
/// to a `NSRunningApplication` — e.g. `loginwindow`) — this only feeds a
/// label and must never panic or hold up the hotkey path.
pub fn secure_input_holder() -> Option<String> {
    let pid = secure_input_pid()?;
    let app = objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    app.localizedName().map(|name| name.to_string())
}

/// Reads `IOHIDSystem`'s `IOConsoleUsers` property — an array with one
/// dictionary per login session — off the IOKit registry, and returns
/// `kCGSSessionSecureInputPID` from the on-console session's dictionary.
/// This is the exact value `ioreg -l | grep kCGSSessionSecureInputPID`
/// prints, just read programmatically instead of shelling out.
fn secure_input_pid() -> Option<i32> {
    let name = c"IOHIDSystem";
    // SAFETY: `name` is a valid, NUL-terminated C string. Returns either a
    // valid +1 `CFMutableDictionaryRef` or null.
    let matching = unsafe { IOServiceMatching(name.as_ptr()) };
    if matching.is_null() {
        return None;
    }
    // SAFETY: `matching` is released by this call either way (its
    // documented "consumes one reference" contract) — no separate release
    // needed on any path below.
    let service = unsafe { IOServiceGetMatchingService(kIOMainPortDefault, matching) };
    if service == 0 {
        return None;
    }
    let result = read_secure_input_pid(service);
    // SAFETY: `service` was just returned by `IOServiceGetMatchingService`
    // and hasn't been released anywhere else.
    unsafe { IOObjectRelease(service) };
    result
}

fn read_secure_input_pid(service: IoMachPort) -> Option<i32> {
    let key = CFString::from_str("IOConsoleUsers");
    // SAFETY: `service` is a live `io_registry_entry_t` (the IOHIDSystem
    // service, still held by the caller); `key` is a valid `CFStringRef`
    // for the duration of the call; a null allocator falls back to the
    // default allocator; no options are defined for this call. Returns a
    // +1 `CFTypeRef`, or null if the property doesn't exist.
    let property = unsafe {
        IORegistryEntryCreateCFProperty(service, (&*key as *const CFString).cast(), core::ptr::null(), 0)
    };
    let property = NonNull::new(property.cast::<CFType>())?;
    // SAFETY: `property` is a live +1 reference; handing it to `CFRetained`
    // makes it responsible for the matching release.
    let property = unsafe { CFRetained::from_raw(property) };
    // SAFETY: `IOConsoleUsers` is documented (IOKitLib.h) to be a CFArray
    // of CFDictionary, and confirmed on this machine via `ioreg -l` to
    // contain string/number/boolean-valued session dictionaries — i.e.
    // `CFArray<CFDictionary<CFString, CFType>>`.
    let sessions: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
        unsafe { CFRetained::cast_unchecked(property) };

    let on_console_key = CFString::from_str("kCGSSessionOnConsoleKey");
    let secure_input_key = CFString::from_str("kCGSSessionSecureInputPID");

    for i in 0..sessions.len() {
        let session = sessions.get(i)?;
        // SAFETY: `kCGSSessionOnConsoleKey`'s documented value type is a
        // CFBoolean.
        let on_console = session
            .get(&on_console_key)
            .map(|v| unsafe { CFRetained::cast_unchecked::<CFBoolean>(v) })
            .is_some_and(|b| b.as_bool());
        if !on_console {
            continue;
        }
        // SAFETY: `kCGSSessionSecureInputPID`'s documented value type is a
        // CFNumber.
        let pid = session
            .get(&secure_input_key)
            .map(|v| unsafe { CFRetained::cast_unchecked::<CFNumber>(v) })
            .and_then(|n| n.as_i32())
            .unwrap_or(0);
        return if pid != 0 { Some(pid) } else { None };
    }
    None
}
