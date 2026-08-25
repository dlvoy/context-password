//! System tray icon and its context menu.
//!
//! Built once, inside `App::new`, on the winit event-loop thread: tray-icon
//! creates a message-only window on Windows that must be pumped by the same
//! `GetMessage` loop winit already owns on that thread.

use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Opens the popup centered on the primary monitor, the same path the
/// delayed-unlock prompt uses, since a tray click has no cursor/target
/// context to place relative to.
pub const SHOW_ID: &str = "show";
/// Forgets the unlocked session and clears cached items from memory.
pub const LOCK_ID: &str = "lock";
/// Opens the settings window.
pub const SETTINGS_ID: &str = "settings";
pub const QUIT_ID: &str = "quit";

pub fn build() -> tray_icon::Result<TrayIcon> {
    let menu = Menu::new();
    let show = MenuItem::with_id(SHOW_ID, "Show", true, None);
    let lock = MenuItem::with_id(LOCK_ID, "Lock", true, None);
    let settings = MenuItem::with_id(SETTINGS_ID, "Settings…", true, None);
    let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);

    let append = |item: &dyn tray_icon::menu::IsMenuItem| {
        menu.append(item)
            .expect("appending a single item to a fresh menu cannot fail")
    };
    append(&show);
    append(&lock);
    append(&PredefinedMenuItem::separator());
    append(&settings);
    append(&PredefinedMenuItem::separator());
    append(&quit);

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(placeholder_icon())
        .with_tooltip("context-password")
        .build()
}

/// A solid placeholder glyph until a real icon is designed. 16x16 keeps the
/// tray crisp at 100% scaling; Windows scales it up itself at higher DPI.
fn placeholder_icon() -> Icon {
    const SIZE: u32 = 16;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..(SIZE * SIZE) {
        rgba.extend_from_slice(&[0x6a, 0x3d, 0xd1, 0xff]); // solid violet
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("a solid 16x16 RGBA buffer is always a valid icon")
}
