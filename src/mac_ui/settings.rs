//! The Settings window — a plain `NSWindow` (not folded into the popup
//! panel the way Windows does `Content::Settings`, since AppKit has no
//! equivalent of the eframe/glow bug that forced that design there; see
//! `crate::controller`'s module doc).
//!
//! **v1 scope note:** the hotkey is shown read-only with no in-app
//! recorder. A real recorder needs a local `NSEvent` monitor, which is
//! block-based (`addLocalMonitorForEventsMatchingMask:handler:`) — left
//! for a follow-up pass rather than adding `block2` closures to the first
//! working cut of this window. Every other setting (autostart, auto-unlock,
//! lock-on-exit, max visible items) is fully live here.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{sel, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSButtonType, NSColor, NSControlStateValueOff, NSControlStateValueOn,
    NSFont, NSTextField, NSTextFieldDelegate, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use crate::config::Config;
use crate::controller::VaultState;
use crate::hotkey::Hotkey;

/// What Save reads back out of the window's live controls — the macOS
/// counterpart of (Windows-only) `ui::config_window::ConfigWindowState`,
/// minus the hotkey-recording machinery this version doesn't have yet.
pub struct Draft {
    pub hotkey_spec: String,
    pub autostart: bool,
    pub auto_unlock: bool,
    pub lock_on_exit: bool,
    pub max_visible_items: u32,
}

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    hotkey_label: Retained<NSTextField>,
    autostart_check: Retained<NSButton>,
    auto_unlock_check: Retained<NSButton>,
    lock_on_exit_check: Retained<NSButton>,
    max_visible_field: Retained<NSTextField>,
    message_label: Retained<NSTextField>,
    /// Carried through `show`/Save rather than re-read from `Hotkey`,
    /// since this build has no recorder to change it.
    hotkey_spec: std::cell::RefCell<String>,
}

const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 260.0;

impl SettingsWindow {
    pub fn new(mtm: MainThreadMarker, delegate: &ProtocolObject<dyn NSTextFieldDelegate>) -> Self {
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Settings"));
        window.center();

        let content = window.contentView().expect("window must have a content view");

        let mut y = HEIGHT - 34.0;
        let hotkey_title = text(mtm, "Hotkey", false);
        place(&hotkey_title, 16.0, y, 100.0, 18.0, HEIGHT);
        let hotkey_label = text(mtm, "", true);
        place(&hotkey_label, 120.0, y, WIDTH - 140.0, 18.0, HEIGHT);
        y -= 26.0;

        let note = text(mtm, "(hotkey recording isn't available yet — edit config.toml)", false);
        note.setFont(Some(&NSFont::systemFontOfSize(10.0)));
        note.setTextColor(Some(&NSColor::secondaryLabelColor()));
        place(&note, 16.0, y, WIDTH - 32.0, 14.0, HEIGHT);
        y -= 30.0;

        let autostart_check = checkbox(mtm, "Open at Login");
        place(&autostart_check, 16.0, y, WIDTH - 32.0, 20.0, HEIGHT);
        y -= 26.0;

        let auto_unlock_check = checkbox(mtm, "Auto-unlock prompt at startup");
        place(&auto_unlock_check, 16.0, y, WIDTH - 32.0, 20.0, HEIGHT);
        y -= 26.0;

        let lock_on_exit_check = checkbox(mtm, "Lock vault on exit");
        place(&lock_on_exit_check, 16.0, y, WIDTH - 32.0, 20.0, HEIGHT);
        y -= 30.0;

        let max_visible_title = text(mtm, "Max visible items (3-10)", false);
        place(&max_visible_title, 16.0, y, 200.0, 18.0, HEIGHT);
        let max_visible_field = NSTextField::new(mtm);
        place(&max_visible_field, 220.0, y - 2.0, 60.0, 22.0, HEIGHT);
        y -= 34.0;

        let message_label = text(mtm, "", false);
        message_label.setTextColor(Some(&NSColor::systemRedColor()));
        place(&message_label, 16.0, y, WIDTH - 32.0, 18.0, HEIGHT);

        let save = NSButton::new(mtm);
        save.setTitle(&NSString::from_str("Save"));
        save.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
        unsafe {
            save.setTarget(Some(delegate.as_ref()));
            save.setAction(Some(sel!(settingsSave:)));
        }
        place(&save, WIDTH - 90.0, 14.0, 74.0, 26.0, HEIGHT);

        let cancel = NSButton::new(mtm);
        cancel.setTitle(&NSString::from_str("Cancel"));
        cancel.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
        unsafe {
            cancel.setTarget(Some(delegate.as_ref()));
            cancel.setAction(Some(sel!(settingsCancel:)));
        }
        place(&cancel, WIDTH - 170.0, 14.0, 74.0, 26.0, HEIGHT);

        for view in [
            &*hotkey_title as &objc2_app_kit::NSView,
            &hotkey_label,
            &note,
            &autostart_check,
            &auto_unlock_check,
            &lock_on_exit_check,
            &max_visible_title,
            &max_visible_field,
            &message_label,
            &save,
            &cancel,
        ] {
            content.addSubview(view);
        }

        Self {
            window,
            hotkey_label,
            autostart_check,
            auto_unlock_check,
            lock_on_exit_check,
            max_visible_field,
            message_label,
            hotkey_spec: std::cell::RefCell::new(String::new()),
        }
    }

    pub fn show(&self, cfg: &Config, hotkey: &Hotkey, _vault_state: VaultState) {
        *self.hotkey_spec.borrow_mut() = hotkey.spec();
        self.hotkey_label.setStringValue(&NSString::from_str(&hotkey.spec()));
        set_checkbox(&self.autostart_check, platform_autostart_enabled());
        set_checkbox(&self.auto_unlock_check, cfg.unlock_mode == crate::config::UnlockMode::Delayed);
        set_checkbox(&self.lock_on_exit_check, cfg.lock_on_exit);
        self.max_visible_field
            .setStringValue(&NSString::from_str(&cfg.effective_max_visible().to_string()));
        self.message_label.setStringValue(&NSString::from_str(""));
        self.window.makeKeyAndOrderFront(None);
    }

    pub fn hide(&self) {
        self.window.orderOut(None);
    }

    pub fn draft(&self) -> Draft {
        let max_visible_items = self
            .max_visible_field
            .stringValue()
            .to_string()
            .trim()
            .parse::<u32>()
            .unwrap_or(4);
        Draft {
            hotkey_spec: self.hotkey_spec.borrow().clone(),
            autostart: checkbox_is_on(&self.autostart_check),
            auto_unlock: checkbox_is_on(&self.auto_unlock_check),
            lock_on_exit: checkbox_is_on(&self.lock_on_exit_check),
            max_visible_items,
        }
    }

    pub fn show_error(&self, message: &str) {
        self.message_label.setStringValue(&NSString::from_str(message));
    }
}

fn platform_autostart_enabled() -> bool {
    crate::platform::autostart::is_enabled()
}

fn text(mtm: MainThreadMarker, s: &str, bold: bool) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(s), mtm);
    let font: Retained<NSFont> =
        if bold { NSFont::boldSystemFontOfSize(12.0) } else { NSFont::systemFontOfSize(12.0) };
    field.setFont(Some(&font));
    field
}

fn checkbox(mtm: MainThreadMarker, title: &str) -> Retained<NSButton> {
    let button = NSButton::new(mtm);
    button.setButtonType(NSButtonType::Switch);
    button.setTitle(&NSString::from_str(title));
    button
}

fn set_checkbox(button: &NSButton, on: bool) {
    button.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
}

fn checkbox_is_on(button: &NSButton) -> bool {
    button.state() == NSControlStateValueOn
}

fn place(view: &objc2_app_kit::NSView, x: f64, top: f64, w: f64, h: f64, window_height: f64) {
    view.setFrame(NSRect::new(NSPoint::new(x, window_height - top - h), NSSize::new(w, h)));
}
