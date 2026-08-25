//! System tray icon and its context menu.
//!
//! Built once, inside `App::new`, on the winit event-loop thread: tray-icon
//! creates a message-only window on Windows that must be pumped by the same
//! `GetMessage` loop winit already owns on that thread.

use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// The id `MenuEvent::set_event_handler` compares against to know Show was
/// chosen — opens the popup centered on the primary monitor, the same path
/// the delayed-unlock prompt uses, since a tray click has no cursor/target
/// context to place relative to.
pub const SHOW_ID: &str = "show";
/// The id for Quit.
pub const QUIT_ID: &str = "quit";

pub fn build() -> tray_icon::Result<TrayIcon> {
    let menu = Menu::new();
    let show = MenuItem::with_id(SHOW_ID, "Show", true, None);
    let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
    menu.append(&show)
        .expect("appending a single item to a fresh menu cannot fail");
    menu.append(&PredefinedMenuItem::separator())
        .expect("appending a single item to a fresh menu cannot fail");
    menu.append(&quit)
        .expect("appending a single item to a fresh menu cannot fail");

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(placeholder_icon())
        .with_tooltip("context-password")
        .build()
}

/// A solid placeholder glyph until a real icon is designed (see the plan's
/// M8 hardening/polish milestone). 16x16 keeps the tray crisp at 100%
/// scaling; Windows scales it up itself at higher DPI.
fn placeholder_icon() -> Icon {
    const SIZE: u32 = 16;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..(SIZE * SIZE) {
        rgba.extend_from_slice(&[0x6a, 0x3d, 0xd1, 0xff]); // solid violet
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("a solid 16x16 RGBA buffer is always a valid icon")
}
