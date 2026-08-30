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
/// Re-runs `bw sync` + `bw list items` against the retained session and
/// replaces the cached item list — no re-unlock needed.
pub const SYNC_ID: &str = "sync";
/// Forgets the unlocked session and clears cached items from memory.
pub const LOCK_ID: &str = "lock";
/// Opens the settings window.
pub const SETTINGS_ID: &str = "settings";
/// Opens the About screen.
pub const ABOUT_ID: &str = "about";
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
    sync: MenuItem,
    lock: MenuItem,
    unlocked_items_present: bool,
}

impl TrayHandles {
    /// "Show" once the vault is unlocked (there's a session to reopen the
    /// list from); "Unlock" otherwise, since that's what the item will
    /// actually prompt for.
    pub fn set_show_label(&self, unlocked: bool) {
        self.show.set_text(if unlocked { "Show" } else { "Unlock" });
    }

    /// Sync and Lock only make sense while there's an unlocked session —
    /// nothing to refresh or forget otherwise. `Menu`/`MenuItem` have no
    /// `set_visible` (checked against `muda-0.19.3`) — hiding means
    /// structurally removing the items and reinserting them later, right
    /// after Show, rather than disabling them.
    pub fn set_unlocked_items_visible(&mut self, visible: bool) {
        if visible == self.unlocked_items_present {
            return;
        }
        if visible {
            // Reinsert in display order: Sync directly under Show, Lock
            // under Sync.
            let _ = self.menu.insert(&self.sync, 1);
            let _ = self.menu.insert(&self.lock, 2);
        } else {
            let _ = self.menu.remove(&self.sync);
            let _ = self.menu.remove(&self.lock);
        }
        self.unlocked_items_present = visible;
    }
}

pub fn build() -> tray_icon::Result<TrayHandles> {
    let menu = Menu::new();
    let show = MenuItem::with_id(SHOW_ID, "Show", true, None);
    let sync = MenuItem::with_id(SYNC_ID, "Sync", true, None);
    let lock = MenuItem::with_id(LOCK_ID, "Lock", true, None);
    let settings = MenuItem::with_id(SETTINGS_ID, "Settings…", true, None);
    let about = MenuItem::with_id(ABOUT_ID, "About…", true, None);
    let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);

    let append = |item: &dyn tray_icon::menu::IsMenuItem| {
        menu.append(item)
            .expect("appending a single item to a fresh menu cannot fail")
    };
    append(&show);
    append(&sync);
    append(&lock);
    append(&PredefinedMenuItem::separator());
    append(&settings);
    append(&about);
    append(&PredefinedMenuItem::separator());
    append(&quit);

    // `Menu` is a cheap `Rc`-backed handle (confirmed in muda's source): the
    // clone kept here and the one handed to the builder refer to the same
    // native menu, so mutating `menu` later (via `set_lock_visible`) is
    // reflected immediately without calling `TrayIcon::set_menu` again.
    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        .with_icon(tray_icon_image())
        .with_tooltip("Context Password for Bitwarden")
        .build()?;

    Ok(TrayHandles {
        _icon: icon,
        menu,
        show,
        sync,
        lock,
        unlocked_items_present: true,
    })
}

/// The app's logo (`development/logo.png`, regenerated via
/// `resources/generate-icons.ps1`), embedded as a raw 32x32 RGBA buffer —
/// no runtime PNG-decoding dependency needed for a single fixed-size image.
fn tray_icon_image() -> Icon {
    const SIZE: u32 = 32;
    let rgba = include_bytes!("../resources/tray_icon.rgba").to_vec();
    Icon::from_rgba(rgba, SIZE, SIZE).expect("resources/tray_icon.rgba is a 32x32 RGBA buffer")
}
