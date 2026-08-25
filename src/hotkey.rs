//! Global hotkey registration.
//!
//! Must be created on the thread that will run the winit event loop (plan
//! F1) — `global-hotkey` registers via `RegisterHotKey` on a hidden window
//! it creates on the calling thread, and `WM_HOTKEY` only reaches it if
//! that thread is the one pumping messages. In practice: construct this in
//! `main`, before `eframe::run_native`, and move it into `App` so the
//! manager (and therefore the registration) stays alive for the process.

use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;

pub struct Hotkey {
    // Never read again after registration, but must stay alive: dropping
    // the manager unregisters everything and tears down its hidden window.
    _manager: GlobalHotKeyManager,
    current: HotKey,
}

impl Hotkey {
    /// Parses `spec` (e.g. "Ctrl+Alt+KeyV") and registers it.
    pub fn register(spec: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let manager = GlobalHotKeyManager::new()?;
        let current: HotKey = spec.parse()?;
        manager.register(current)?;
        Ok(Self {
            _manager: manager,
            current,
        })
    }

    /// The id `GlobalHotKeyEvent::id()` matches against to recognize a
    /// press of this hotkey (there's only ever one, for now).
    pub fn id(&self) -> u32 {
        self.current.id()
    }
}
