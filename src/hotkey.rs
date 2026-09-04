//! Global hotkey registration.
//!
//! Must be created on the thread that will run the winit event loop (plan
//! F1) — `global-hotkey` registers via `RegisterHotKey` on a hidden window
//! it creates on the calling thread, and `WM_HOTKEY` only reaches it if
//! that thread is the one pumping messages. In practice: construct this in
//! `main`, before `eframe::run_native`, and move it into `App` so the
//! manager (and therefore the registration) stays alive for the process.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use global_hotkey::GlobalHotKeyManager;
use global_hotkey::hotkey::HotKey;

pub struct Hotkey {
    // Never read directly again after registration, but must stay alive:
    // dropping the manager unregisters everything and tears down its
    // hidden window.
    manager: GlobalHotKeyManager,
    current: HotKey,
    /// Shared with the `GlobalHotKeyEvent` handler closure registered in
    /// `App::new`, which is `'static` and so can't hold a fresh `u32` after
    /// `set` changes the registration — it reads this atomically instead
    /// (plan M7: "changing the hotkey takes effect without restart").
    current_id: Arc<AtomicU32>,
}

impl Hotkey {
    /// Parses `spec` (e.g. "Ctrl+Alt+KeyV") and registers it.
    pub fn register(spec: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let manager = GlobalHotKeyManager::new()?;
        let current: HotKey = spec.parse()?;
        manager.register(current)?;
        Ok(Self {
            manager,
            current,
            current_id: Arc::new(AtomicU32::new(current.id())),
        })
    }

    /// A handle the hotkey event handler closure reads from on every press,
    /// instead of a `u32` copied in at registration time.
    pub fn id_handle(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.current_id)
    }

    pub fn spec(&self) -> String {
        self.current.into_string()
    }

    /// Temporarily frees the OS-level registration, so the combination
    /// becomes a normal, capturable key combination again instead of being
    /// intercepted system-wide.
    ///
    /// `RegisterHotKey` diverts a matching keystroke straight to
    /// `WM_HOTKEY` *before* it ever reaches the focused window as an
    /// ordinary key event — which means the hotkey recorder can never see
    /// (and so can never let you re-confirm) whatever combination is
    /// *currently* registered, unless that registration is out of the way
    /// while recording. Call `resume` when recording ends without saving a
    /// change, so the app is never left with no hotkey active.
    ///
    /// Only `app.rs` (Windows) calls this — `mac_ui`'s Settings has no
    /// in-app hotkey recorder yet (`mac_ui::settings`'s own module doc
    /// covers why: it needs a `block2`-based local `NSEvent` monitor,
    /// deliberately deferred). `resume` below has no such gap: `mac_ui`
    /// already calls it defensively in a couple of places, ready for
    /// whenever a recorder lands.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn suspend(&self) {
        let _ = self.manager.unregister(self.current);
    }

    /// Restores the registration `suspend` temporarily removed.
    pub fn resume(&self) {
        let _ = self.manager.register(self.current);
    }

    /// Unregisters the current hotkey and registers `spec` in its place.
    /// On failure the old registration is left untouched — the new one is
    /// registered *first*, so the app is never left with no hotkey at all
    /// (plan §9: "surface `AlreadyRegistered` ... and keep the old
    /// binding").
    pub fn set(&mut self, spec: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let new_hotkey: HotKey = spec.parse()?;
        if new_hotkey.id() == self.current.id() {
            return Ok(()); // Same combination — nothing to do.
        }
        self.manager.register(new_hotkey)?;
        // Best-effort: if this fails the old OS-level registration becomes
        // an orphan until the manager (and its hidden window) is dropped,
        // which still cleans it up — not a leak, just delayed.
        let _ = self.manager.unregister(self.current);
        self.current = new_hotkey;
        self.current_id.store(new_hotkey.id(), Ordering::Relaxed);
        Ok(())
    }
}
