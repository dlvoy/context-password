//! "Open at Login" via `SMAppService` — stub. Real implementation is
//! Phase 3 of the port plan (`objc2-service-management`); needs an actual
//! `.app` bundle to test against `SMAppService.mainApp`, since it only
//! works from inside one.

#![allow(unused)]

pub fn is_enabled() -> bool {
    unimplemented!("macOS autostart lands in Phase 3 of the port plan")
}

pub fn set_enabled(_enabled: bool) -> Result<(), String> {
    unimplemented!("macOS autostart lands in Phase 3 of the port plan")
}
