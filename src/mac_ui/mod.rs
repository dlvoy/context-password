//! The native AppKit front-end (port plan Phase 5). No winit, no eframe —
//! we own `NSApplication` directly.

mod about;
mod app_delegate;
mod panel;
mod settings;
mod state;

/// Hands off to `NSApp.run()`. Config is loaded inside
/// `applicationDidFinishLaunching:`, not here — unlike Windows'
/// `main.rs`, which needs it before `eframe::run_native` to register the
/// hotkey up front; on macOS the hotkey must be registered *after*
/// `setActivationPolicy` (see `app_delegate::setup`'s doc), which only
/// happens once the app has actually launched.
pub fn run() {
    app_delegate::run();
}
