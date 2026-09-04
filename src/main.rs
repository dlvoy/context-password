// No console window in release builds — this is a tray-resident app. Debug
// builds keep the console so `eprintln!` output is visible while developing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
mod bw;
mod config;
mod controller;
mod hotkey;
mod keepass;
#[cfg(target_os = "macos")]
mod mac_ui;
mod msg;
mod platform;
mod secret;
mod tray;
#[cfg(windows)]
mod ui;
mod vault;

#[cfg(windows)]
use config::Config;
use platform::singleton::SingleInstance;

#[cfg(windows)]
fn main() {
    use eframe::egui;

    let Some(_instance_guard) = SingleInstance::acquire() else {
        eprintln!("context-password is already running; exiting");
        return;
    };

    let cfg = load_config();
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
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, hotkey, cfg)?))),
    ) {
        eprintln!("eframe exited with an error: {e}");
    }
}

/// macOS entry point (port plan Phase 5). Config is loaded again inside
/// `mac_ui::app_delegate::setup` — see that module's doc for why the
/// hotkey can't be registered this early the way Windows' arm does it.
#[cfg(target_os = "macos")]
fn main() {
    let Some(_instance_guard) = SingleInstance::acquire() else {
        eprintln!("context-password is already running; exiting");
        return;
    };

    mac_ui::run();
}

#[cfg(windows)]
fn load_config() -> Config {
    match Config::load() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("failed to load config, using defaults: {e}");
            Config::default()
        }
    }
}
