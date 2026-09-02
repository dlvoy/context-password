//! "Open at Login" via `SMAppService` — the macOS counterpart of
//! `win::autostart`'s `HKCU\...\Run` registry key. macOS 13+, and — unlike
//! the registry key, which works for any executable — `SMAppService.mainApp`
//! only works from *inside* a proper `.app` bundle with a stable
//! `CFBundleIdentifier`; calling it from the unbundled dev binary fails
//! with a signature/bundle error, surfaced here as a clear message rather
//! than a confusing OS error code.

use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// Reads the live status from `SMAppService` rather than trusting a config
/// flag, since the user may have removed it via System Settings' Login
/// Items list directly — same "reflect reality, not our last write"
/// posture as `win::autostart::is_enabled`.
pub fn is_enabled() -> bool {
    let service = unsafe { SMAppService::mainAppService() };
    matches!(unsafe { service.status() }, SMAppServiceStatus::Enabled)
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let service = unsafe { SMAppService::mainAppService() };
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
