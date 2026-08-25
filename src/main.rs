// No console window in release builds — this is a tray-resident app. Debug
// builds keep the console so `eprintln!` output is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod hotkey;
mod msg;
mod tray;
mod win;

use eframe::egui;

use config::Config;
use win::singleton::SingleInstance;

fn main() {
    let Some(_instance_guard) = SingleInstance::acquire() else {
        eprintln!("context-password is already running; exiting");
        return;
    };

    let cfg = match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("failed to load config, using defaults: {e}");
            Config::default()
        }
    };
    eprintln!("context-password starting (hotkey={})", cfg.hotkey);

    // Must happen on this thread, before `eframe::run_native` hands it to
    // winit's event loop — see plan F1: `global-hotkey` registers via
    // `RegisterHotKey` on a hidden window created on the calling thread,
    // and `WM_HOTKEY` only reaches it if that thread pumps messages.
    let hotkey = match hotkey::Hotkey::register(&cfg.hotkey) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("failed to register global hotkey '{}': {e}", cfg.hotkey);
            return;
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("context-password")
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_visible(false)
            .with_active(false)
            .with_inner_size([340.0, 220.0]),
        persist_window: false,
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        "context-password",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, hotkey)?))),
    ) {
        eprintln!("eframe exited with an error: {e}");
    }
}
