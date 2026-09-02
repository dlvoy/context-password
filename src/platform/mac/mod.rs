//! macOS platform backend. Phase 1 has only `singleton` wired up for real
//! (needed by `main.rs` on both platforms); the rest are stubs with the
//! Windows counterpart's function signatures, to be filled in during
//! Phase 3 once `src/mac_ui/` exists to actually call them. No
//! `window_style` module here — unlike Windows, macOS has nothing to poke
//! at a raw window handle for; the equivalent panel properties are set once
//! at creation in `mac_ui::panel` (Phase 5).

pub mod autostart;
pub mod focus;
pub mod indicator;
pub mod monitor;
pub mod singleton;
pub mod typing;
