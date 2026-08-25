//! System tray icon and its context menu.
//!
//! Built once, inside `App::new`, on the winit event-loop thread: tray-icon
//! creates a message-only window on Windows that must be pumped by the same
//! `GetMessage` loop winit already owns on that thread.

use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Opens the popup centered on the primary monitor, the same path the
/// delayed-unlock prompt uses, since a tray click has no cursor/target
/// context to place relative to. Labeled "Show" or "Unlock" depending on
/// vault state — see `TrayHandles::set_show_label`.
pub const SHOW_ID: &str = "show";
/// Forgets the unlocked session and clears cached items from memory.
pub const LOCK_ID: &str = "lock";
/// Opens the settings window.
pub const SETTINGS_ID: &str = "settings";
pub const QUIT_ID: &str = "quit";

/// The tray icon plus handles to its menu items, kept alive and mutable for
/// the process's lifetime — `App` uses these to reflect vault state
/// (`Show`/`Unlock` label, `Lock` item presence) without rebuilding the
/// menu from scratch on every change.
pub struct TrayHandles {
    // Never read again, but must stay alive: dropping it removes the icon.
    _icon: TrayIcon,
    menu: Menu,
    show: MenuItem,
    lock: MenuItem,
    lock_item_present: bool,
}

impl TrayHandles {
    /// "Show" once the vault is unlocked (there's a session to reopen the
    /// list from); "Unlock" otherwise, since that's what the item will
    /// actually prompt for.
    pub fn set_show_label(&self, unlocked: bool) {
        self.show.set_text(if unlocked { "Show" } else { "Unlock" });
    }

    /// The Lock item only makes sense while there's something to lock.
    /// `Menu`/`MenuItem` have no `set_visible` (checked against
    /// `muda-0.19.3`) — hiding means structurally removing the item and
    /// reinserting it later, right after Show, rather than disabling it.
    pub fn set_lock_visible(&mut self, visible: bool) {
        if visible == self.lock_item_present {
            return;
        }
        if visible {
            let _ = self.menu.insert(&self.lock, 1);
        } else {
            let _ = self.menu.remove(&self.lock);
        }
        self.lock_item_present = visible;
    }
}

pub fn build() -> tray_icon::Result<TrayHandles> {
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

    // `Menu` is a cheap `Rc`-backed handle (confirmed in muda's source): the
    // clone kept here and the one handed to the builder refer to the same
    // native menu, so mutating `menu` later (via `set_lock_visible`) is
    // reflected immediately without calling `TrayIcon::set_menu` again.
    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        .with_icon(placeholder_icon())
        .with_tooltip("context-password")
        .build()?;

    Ok(TrayHandles {
        _icon: icon,
        menu,
        show,
        lock,
        lock_item_present: true,
    })
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
