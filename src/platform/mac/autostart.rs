//! "Open at Login" via `SMAppService` — the macOS counterpart of
//! `win::autostart`'s `HKCU\...\Run` registry key. macOS 13+, and — unlike
//! the registry key, which works for any executable — `SMAppService.mainApp`
//! only works from *inside* a proper `.app` bundle with a stable
//! `CFBundleIdentifier`; calling it from the unbundled dev binary fails
//! with a signature/bundle error, surfaced here as a clear message rather
//! than a confusing OS error code.
//!
//! `is_available()` is `true` once running from a real `cargo-packager`
//! `.app` bundle, `false` for the raw dev binary — in which case the
//! Settings checkbox stays disabled with a note explaining why, see
//! `mac_ui::settings`.

use objc2_foundation::NSBundle;
use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// True only when running from inside a real `.app` bundle: `NSBundle`
/// reports a `CFBundleIdentifier` *and* the running executable actually
/// sits under a `Contents/MacOS/` directory (a bare binary launched next to
/// a stray `Info.plist` would satisfy the first check alone). Both
/// `is_enabled`/`set_enabled` behave correctly regardless, but calling them
/// outside a bundle either fails outright or reports a status that can
/// never become `Enabled` — so the UI checks this first and disables the
/// control instead of offering a setting that can't work.
pub fn is_available() -> bool {
    let has_bundle_id = NSBundle::mainBundle().bundleIdentifier().is_some();
    let in_app_bundle = std::env::current_exe()
        .map(|p| p.components().any(|c| c.as_os_str() == "Contents"))
        .unwrap_or(false);
    has_bundle_id && in_app_bundle
}

/// Reads the live status from `SMAppService` rather than trusting a config
/// flag, since the user may have removed it via System Settings' Login
/// Items list directly — same "reflect reality, not our last write"
/// posture as `win::autostart::is_enabled`. `RequiresApproval` counts as
/// enabled: the service *is* registered, the user just hasn't clicked
/// through System Settings' approval prompt yet — reporting that as "off"
/// is what previously made the checkbox flip back unchecked right after a
/// successful registration.
pub fn is_enabled() -> bool {
    let service = unsafe { SMAppService::mainAppService() };
    matches!(
        unsafe { service.status() },
        SMAppServiceStatus::Enabled | SMAppServiceStatus::RequiresApproval
    )
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let service = unsafe { SMAppService::mainAppService() };
    if !enabled && matches!(unsafe { service.status() }, SMAppServiceStatus::NotRegistered) {
        // Unregistering something that's already unregistered isn't a
        // failure from the caller's point of view — the end state (not
        // present) matches. Mirrors win::autostart's ERROR_FILE_NOT_FOUND
        // swallow on delete, and is what closes the loop where reading
        // back a not-quite-`Enabled` status as "off" then made Save try to
        // unregister an already-unregistered service.
        return Ok(());
    }
    let result = if enabled {
        unsafe { service.registerAndReturnError() }
    } else {
        unsafe { service.unregisterAndReturnError() }
    };
    result.map_err(|e| describe_error(&e, enabled))
}

fn describe_error(error: &objc2_foundation::NSError, enabled: bool) -> String {
    let action = if enabled { "register" } else { "unregister" };
    // `SMAppService` fails with a signature/bundle error when called from
    // the unbundled dev binary — the far more likely cause during
    // development than an actual "requires approval" state, so name it
    // explicitly rather than just echoing NSError's own description.
    format!(
        "could not {action} for Open at Login ({}) — this only works from inside the packaged \
         .app bundle, not the raw binary",
        error.localizedDescription()
    )
}
