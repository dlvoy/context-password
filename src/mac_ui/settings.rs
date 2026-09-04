//! The Settings window — a plain `NSWindow` (not folded into the popup
//! panel the way Windows does `Content::Settings`, since AppKit has no
//! equivalent of the eframe/glow bug that forced that design there; see
//! `crate::controller`'s module doc).
//!
//! **v1 scope note:** the hotkey is shown read-only with no in-app
//! recorder. A real recorder needs a local `NSEvent` monitor, which is
//! block-based (`addLocalMonitorForEventsMatchingMask:handler:`) — left
//! for a follow-up pass rather than adding `block2` closures to the first
//! working cut of this window. Every other setting (provider, KeePass
//! database, autostart, auto-unlock, lock-on-exit, max visible items) is
//! fully live here.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{sel, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSButtonType, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSFont, NSPopUpButton, NSTextField, NSTextFieldDelegate, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use crate::config::Config;
use crate::controller::VaultState;
use crate::hotkey::Hotkey;
use crate::vault::Provider;

/// What Save reads back out of the window's live controls — the macOS
/// counterpart of (Windows-only) `ui::config_window::ConfigWindowState`,
/// minus the hotkey-recording machinery this version doesn't have yet.
pub struct Draft {
    pub provider: Provider,
    pub keepass_path: String,
    pub hotkey_spec: String,
    pub autostart: bool,
    pub auto_unlock: bool,
    pub lock_on_exit: bool,
    pub max_visible_items: u32,
}

pub struct SettingsWindow {
    window: Retained<NSWindow>,
    provider_popup: Retained<NSPopUpButton>,
    db_title: Retained<NSTextField>,
    db_field: Retained<NSTextField>,
    db_browse: Retained<NSButton>,
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

// +30 for the Vault row, +30 for the Database row, +16 breathing room —
// the Database row's content is the tallest thing added (a path field can
// run long), so it gets the same width growth as the height growth.
const WIDTH: f64 = 440.0;
const HEIGHT: f64 = 336.0;

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

        // `y` is top-measured and grows downward — `place`'s `top` parameter
        // already does the AppKit bottom-left flip internally, so treating
        // `y` as anything but "distance from the top of the window" here
        // double-flips every row (this was the exact bug that rendered the
        // whole dialog upside down: see the previous plan's Fix 1).
        let mut y = 16.0;

        let vault_title = text(mtm, "Vault", false);
        place(&vault_title, 16.0, y, 100.0, 18.0, HEIGHT);
        let provider_popup = NSPopUpButton::new(mtm);
        provider_popup.addItemWithTitle(&NSString::from_str("Bitwarden (bw CLI)"));
        provider_popup.addItemWithTitle(&NSString::from_str("KeePass (.kdbx)"));
        unsafe {
            provider_popup.setTarget(Some(delegate.as_ref()));
            provider_popup.setAction(Some(sel!(settingsProviderChanged:)));
        }
        place(&provider_popup, 120.0, y + 2.0, WIDTH - 140.0, 24.0, HEIGHT);
        y += 30.0;

        let db_title = text(mtm, "Database", false);
        place(&db_title, 16.0, y, 100.0, 18.0, HEIGHT);
        let db_field = text(mtm, "", true);
        place(&db_field, 120.0, y, WIDTH - 140.0 - 84.0, 18.0, HEIGHT);
        let db_browse = NSButton::new(mtm);
        db_browse.setTitle(&NSString::from_str("Browse…"));
        db_browse.setBezelStyle(objc2_app_kit::NSBezelStyle::Automatic);
        unsafe {
            db_browse.setTarget(Some(delegate.as_ref()));
            db_browse.setAction(Some(sel!(settingsBrowse:)));
        }
        place(&db_browse, WIDTH - 16.0 - 74.0, y - 3.0, 74.0, 24.0, HEIGHT);
        y += 30.0;

        let hotkey_title = text(mtm, "Hotkey", false);
        place(&hotkey_title, 16.0, y, 100.0, 18.0, HEIGHT);
        let hotkey_label = text(mtm, "", true);
        place(&hotkey_label, 120.0, y, WIDTH - 140.0, 18.0, HEIGHT);
        y += 26.0;

        let note = text(mtm, "(hotkey recording isn't available yet — edit config.toml)", false);
        note.setFont(Some(&NSFont::systemFontOfSize(10.0)));
        note.setTextColor(Some(&NSColor::secondaryLabelColor()));
        place(&note, 16.0, y, WIDTH - 32.0, 14.0, HEIGHT);
        y += 30.0;

        let autostart_check = checkbox(mtm, "Open at Login");
        place(&autostart_check, 16.0, y, WIDTH - 32.0, 20.0, HEIGHT);
        y += 26.0;

        let auto_unlock_check = checkbox(mtm, "Auto-unlock prompt at startup");
        place(&auto_unlock_check, 16.0, y, WIDTH - 32.0, 20.0, HEIGHT);
        y += 26.0;

        let lock_on_exit_check = checkbox(mtm, "Lock vault on exit");
        place(&lock_on_exit_check, 16.0, y, WIDTH - 32.0, 20.0, HEIGHT);
        y += 30.0;

        let max_visible_title = text(mtm, "Max visible items (3-10)", false);
        place(&max_visible_title, 16.0, y, 200.0, 18.0, HEIGHT);
        let max_visible_field = NSTextField::new(mtm);
        place(&max_visible_field, 220.0, y + 2.0, 60.0, 22.0, HEIGHT);
        y += 34.0;

        // Two lines tall and wrapped: the longest message that lands here
        // (the combined autostart-bundle error) runs well past one line at
        // this width, and used to just clip mid-sentence.
        let message_label = text(mtm, "", false);
        message_label.setTextColor(Some(&NSColor::systemRedColor()));
        message_label.setUsesSingleLineMode(false);
        message_label.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByWordWrapping);
        if let Some(cell) = message_label.cell() {
            cell.setWraps(true);
        }
        place(&message_label, 16.0, y, WIDTH - 32.0, 32.0, HEIGHT);

        let save = NSButton::new(mtm);
        save.setTitle(&NSString::from_str("Save"));
        save.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
        unsafe {
            save.setTarget(Some(delegate.as_ref()));
            save.setAction(Some(sel!(settingsSave:)));
        }
        place(&save, WIDTH - 90.0, HEIGHT - 40.0, 74.0, 26.0, HEIGHT);

        let cancel = NSButton::new(mtm);
        cancel.setTitle(&NSString::from_str("Cancel"));
        cancel.setBezelStyle(objc2_app_kit::NSBezelStyle::Push);
        unsafe {
            cancel.setTarget(Some(delegate.as_ref()));
            cancel.setAction(Some(sel!(settingsCancel:)));
        }
        place(&cancel, WIDTH - 170.0, HEIGHT - 40.0, 74.0, 26.0, HEIGHT);

        for view in [
            &*vault_title as &objc2_app_kit::NSView,
            &provider_popup,
            &db_title,
            &db_field,
            &db_browse,
            &hotkey_title,
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

        let this = Self {
            window,
            provider_popup,
            db_title,
            db_field,
            db_browse,
            hotkey_label,
            autostart_check,
            auto_unlock_check,
            lock_on_exit_check,
            max_visible_field,
            message_label,
            hotkey_spec: std::cell::RefCell::new(String::new()),
        };
        this.sync_provider_rows();
        this
    }

    pub fn show(&self, cfg: &Config, hotkey: &Hotkey, _vault_state: VaultState) {
        self.provider_popup.selectItemAtIndex(provider_index(cfg.provider));
        self.db_field.setStringValue(&NSString::from_str(&cfg.keepass_path));
        self.sync_provider_rows();
        *self.hotkey_spec.borrow_mut() = hotkey.spec();
        self.hotkey_label.setStringValue(&NSString::from_str(&hotkey.spec()));
        // Greyed out with the reason baked into its own title when running
        // unbundled (the raw dev binary, not a `cargo-packager` `.app`):
        // `SMAppService` can never report `Enabled` from a bare binary, so
        // offering a live checkbox here just relearns that the hard way on
        // every Save. See `platform::mac::autostart::is_available`.
        let autostart_available = platform_autostart_available();
        self.autostart_check.setEnabled(autostart_available);
        self.autostart_check.setTitle(&NSString::from_str(if autostart_available {
            "Open at Login"
        } else {
            "Open at Login (needs the packaged .app)"
        }));
        set_checkbox(&self.autostart_check, autostart_available && platform_autostart_enabled());
        set_checkbox(&self.auto_unlock_check, cfg.unlock_mode == crate::config::UnlockMode::Delayed);
        set_checkbox(&self.lock_on_exit_check, cfg.lock_on_exit);
        self.max_visible_field
            .setStringValue(&NSString::from_str(&cfg.effective_max_visible().to_string()));
        self.message_label.setStringValue(&NSString::from_str(""));
        // Same activate-before-raise fix as the popup panel's `raise()`
        // (`mac_ui::panel`): this process runs `.accessory`, so a bare
        // `makeKeyAndOrderFront:` alone doesn't reliably raise the window
        // above another app's — found in manual testing to affect this
        // window (and About) exactly like it used to affect the popup.
        let mtm = MainThreadMarker::new().expect("SettingsWindow::show must run on the main thread");
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
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
            provider: provider_from_index(self.provider_popup.indexOfSelectedItem()),
            keepass_path: self.db_field.stringValue().to_string(),
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

    /// Sets the KeePass database path field — the `Browse…` button's
    /// handler calls this after a successful `rfd` pick; the field is
    /// otherwise read-only from the user's perspective (no direct typing
    /// affordance, matching the read-only hotkey label above it).
    pub fn set_db_path(&self, path: &str) {
        self.db_field.setStringValue(&NSString::from_str(path));
    }

    /// Shows/hides the Database row based on the popup's *current*
    /// selection — called on init and from the popup's own action handler
    /// only, never from a periodic refresh (see the macOS port plan's bug
    /// #4: hiding a view resigns its first-responder status, so toggling
    /// this must stay tied to an explicit user action, not a timer).
    pub fn sync_provider_rows(&self) {
        let is_keepass = provider_from_index(self.provider_popup.indexOfSelectedItem()) == Provider::KeePass;
        self.db_title.setHidden(!is_keepass);
        self.db_field.setHidden(!is_keepass);
        self.db_browse.setHidden(!is_keepass);
    }
}

fn provider_index(provider: Provider) -> isize {
    match provider {
        Provider::Bitwarden => 0,
        Provider::KeePass => 1,
    }
}

fn provider_from_index(index: isize) -> Provider {
    if index == 1 { Provider::KeePass } else { Provider::Bitwarden }
}

fn platform_autostart_enabled() -> bool {
    crate::platform::autostart::is_enabled()
}

fn platform_autostart_available() -> bool {
    crate::platform::autostart::is_available()
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
