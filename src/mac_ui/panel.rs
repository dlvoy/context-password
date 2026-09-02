//! The popup panel — a visually chrome-free, non-activating `NSPanel`
//! hosting plain AppKit controls (no custom drawing here, unlike
//! `platform::mac::indicator`), toggled visible/hidden per `PopupContent`
//! state rather than rebuilt each time.
//!
//! `.nonactivatingPanel` is what lets this take keyboard input (the secure
//! text field, arrow-key list navigation) without ever making our app the
//! active one — replacing Windows' three-frame `Placing → Showing →
//! Activating` dance entirely: `show_at` is one `setFrame_display` +
//! `makeKeyAndOrderFront:`.
//!
//! **Not actually borderless — deliberately.** Confirmed by manual
//! testing plus a temporary diagnostic (`panel.isKeyWindow()` logged
//! `false` even with the app active, `setBecomesKeyOnlyIfNeeded(false)`,
//! and `makeKeyAndOrderFront:` called): Apple's default
//! `canBecomeKeyWindow` returns `NO` for any *borderless* window
//! regardless of `NSPanel` vs `NSWindow` — the well-known "`NSPanel`
//! defaults to `YES`" behavior only applies to windows that carry
//! `.titled`. A custom subclass overriding `canBecomeKeyWindow` is the
//! textbook fix, but calling `NSPanel`'s multi-argument designated
//! initializer through `super(...)` hits an `objc2` 0.6.4 `msg_send!`
//! limitation with no working precedent found anywhere in the crate
//! ecosystem (searched the full local registry). The workaround used
//! here instead: keep `.titled` (which is what actually grants the
//! `YES` default) but make it *look* borderless —
//! `titlebarAppearsTransparent` + `titleVisibility(.hidden)` hide the
//! title bar visually, `.fullSizeContentView` lets our own content view
//! fill the space it would have occupied, and omitting
//! `.closable`/`.miniaturizable`/`.resizable` leaves no title-bar buttons
//! to hide in the first place.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{sel, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSButton, NSColor, NSFont, NSPanel, NSProgressIndicator,
    NSProgressIndicatorStyle, NSSecureTextField, NSTextContent, NSTextContentTypeOneTimeCode,
    NSTextField, NSTextFieldDelegate, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
    NSWindowTitleVisibility,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

use super::state::PopupContent;

pub const PANEL_WIDTH: f64 = 340.0;
const ROW_HEIGHT: f64 = 28.0;
const LINE_HEIGHT: f64 = 20.0;
const CHROME_TOP: f64 = 34.0;
const CHROME_BOTTOM: f64 = 26.0;
/// Rows beyond this are simply not shown — matches
/// `config::MAX_VISIBLE_ITEMS`, kept as a separate literal here since
/// `mac_ui` doesn't otherwise depend on `config`'s internals.
pub const MAX_ROWS: usize = 10;

fn new_panel(mtm: MainThreadMarker) -> Retained<NSPanel> {
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
        NSWindowStyleMask::Titled
            | NSWindowStyleMask::NonactivatingPanel
            | NSWindowStyleMask::FullSizeContentView,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setTitlebarAppearsTransparent(true);
    panel.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    panel
}

/// Owns every control the popup can possibly show, created once and
/// reused — visibility and content are updated per `PopupContent`, not
/// rebuilt. Mirrors `platform::mac::indicator`'s lazy-once-then-reuse
/// lifecycle, one level up.
pub struct Panel {
    pub panel: Retained<NSPanel>,
    title_label: Retained<NSTextField>,
    hint_label: Retained<NSTextField>,
    prompt_field: Retained<NSSecureTextField>,
    list_key_catcher: Retained<NSTextField>,
    prompt_error: Retained<NSTextField>,
    busy_spinner: Retained<NSProgressIndicator>,
    busy_label: Retained<NSTextField>,
    row_buttons: Vec<Retained<NSButton>>,
    stale_label: Retained<NSTextField>,
    blocked_label: Retained<NSTextField>,
    message_label: Retained<NSTextField>,
}

/// One control to place, deferred until the content view's final height is
/// known — see `Panel::layout`'s doc for why this can't be a single pass.
struct Placement<'a> {
    view: &'a NSView,
    x: f64,
    top: f64,
    w: f64,
    h: f64,
}

impl Panel {
    pub fn new(mtm: MainThreadMarker, delegate: &ProtocolObject<dyn NSTextFieldDelegate>) -> Self {
        let panel = new_panel(mtm);
        // SAFETY: this panel is never used inside a window controller, so
        // disabling auto-release on close is required (same reasoning as
        // `platform::mac::indicator`'s panel).
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(objc2_app_kit::NSFloatingWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setHidesOnDeactivate(false);
        // Without this, `makeKeyAndOrderFront:` only makes the panel key
        // if some view already "needs" it at that exact moment — and
        // since the first responder is set *after* that call (`show_at`
        // runs before `focus_prompt`/`focus_list_catcher`), the panel
        // would silently stay non-key and no keyboard event would ever
        // reach it. This is the actual cause of a real bug found in
        // manual testing: the popup rendered but nothing typed reached
        // it, and Escape/Enter did nothing either — both symptoms of the
        // panel never truly becoming key at the OS level.
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setOpaque(true);
        panel.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
        panel.setAnimationBehavior(objc2_app_kit::NSWindowAnimationBehavior::None);

        let content_view = panel.contentView().expect("panel must have a content view");

        let title_label = label(mtm, "context-password", 13.0, true);
        let hint_label = label(mtm, "", 11.0, false);
        hint_label.setTextColor(Some(&NSColor::secondaryLabelColor()));

        let prompt_field = NSSecureTextField::new(mtm);
        unsafe { prompt_field.setDelegate(Some(delegate)) };
        // Found in manual testing: any `NSSecureTextField` triggers
        // macOS's system Password AutoFill suggestion popover on focus,
        // independent of `contentType` (which defaults to `nil` and
        // doesn't stop it) — it steals keyboard focus out from under the
        // field entirely. `OneTimeCode` is the documented content type
        // that opts a secure field out of password-manager suggestions
        // (it signals "not a persistent credential"), which is the actual
        // intent here: this field holds the vault's own master password,
        // not a site login `bw` (let alone the OS) should be suggesting
        // saved credentials for.
        prompt_field.setContentType(Some(unsafe { NSTextContentTypeOneTimeCode }));

        // Invisible (zero-size, unbezeled, no background) but *not*
        // `setHidden` — a hidden view can't become first responder. Made
        // key whenever `ShowingList`/`FetchingOtp` is current, so arrow
        // keys / Enter / Escape reach `control:textView:doCommandBySelector:`
        // the same way the password field's do — one delegate method
        // handles both, branching on the live `PopupContent`.
        let list_key_catcher = NSTextField::new(mtm);
        list_key_catcher.setBezeled(false);
        list_key_catcher.setDrawsBackground(false);
        list_key_catcher.setEditable(true);
        unsafe { list_key_catcher.setDelegate(Some(delegate)) };

        let prompt_error = label(mtm, "", 11.0, false);
        prompt_error.setTextColor(Some(&NSColor::systemRedColor()));

        let busy_spinner = NSProgressIndicator::new(mtm);
        busy_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        busy_spinner.setIndeterminate(true);

        let busy_label = label(mtm, "", 12.0, false);

        let mut row_buttons = Vec::with_capacity(MAX_ROWS);
        for i in 0..MAX_ROWS {
            let button = NSButton::new(mtm);
            button.setBezelStyle(NSBezelStyle::Automatic);
            button.setBordered(false);
            unsafe {
                button.setTarget(Some(delegate.as_ref()));
                button.setAction(Some(sel!(rowClicked:)));
            }
            button.setTag(i as isize);
            button.setHidden(true);
            row_buttons.push(button);
        }

        let stale_label = label(mtm, "", 11.0, false);
        stale_label.setTextColor(Some(&NSColor::systemOrangeColor()));
        let blocked_label = label(mtm, "", 11.0, false);
        blocked_label.setTextColor(Some(&NSColor::systemOrangeColor()));
        let message_label = label(mtm, "", 11.0, false);
        message_label.setTextColor(Some(&NSColor::systemRedColor()));

        list_key_catcher.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)));
        content_view.addSubview(&list_key_catcher);

        for view in [&*title_label as &NSView, &hint_label, &prompt_field, &prompt_error]
            .into_iter()
            .chain(std::iter::once(&*busy_spinner as &NSView))
            .chain([&*busy_label as &NSView, &stale_label, &blocked_label, &message_label])
        {
            content_view.addSubview(view);
        }
        for button in &row_buttons {
            content_view.addSubview(button);
        }

        Self {
            panel,
            title_label,
            hint_label,
            prompt_field,
            list_key_catcher,
            prompt_error,
            busy_spinner,
            busy_label,
            row_buttons,
            stale_label,
            blocked_label,
            message_label,
        }
    }

    /// Reads and clears the master-password field in one step — the field
    /// is never left holding what was just submitted. `NSString` itself
    /// can't be zeroed (immutable, possibly interned) — this is the
    /// accepted, documented tradeoff (port plan, `prompt.rs`'s design
    /// notes) for the Secure Event Input protection a real
    /// `NSSecureTextField` gives while the user is typing, which a
    /// hand-painted field can't.
    pub fn take_password(&self) -> String {
        let value = self.prompt_field.stringValue().to_string();
        self.prompt_field.setStringValue(&NSString::from_str(""));
        value
    }

    pub fn focus_prompt(&self) {
        self.panel.makeFirstResponder(Some(&self.prompt_field));
    }

    /// Makes the invisible key-catcher first responder, so arrow keys,
    /// Enter, and Escape reach the delegate while the list (or a busy/OTP
    /// screen) is showing — see the field's own doc for why.
    pub fn focus_list_catcher(&self) {
        self.panel.makeFirstResponder(Some(&self.list_key_catcher));
    }

    /// Lays out and shows only the controls relevant to `content`, and
    /// returns the total size the panel should be (the caller places it —
    /// see `platform::monitor::placement_for`).
    ///
    /// Two passes: `plan` walks the same state-driven branches as the
    /// original design but only *measures* (accumulating `Placement`s and
    /// a running `y`, touching no AppKit object), so the content view's
    /// final height is known before any `setFrame` call converts a
    /// top-down `y` into AppKit's bottom-left frame coordinates — doing
    /// that conversion against a still-unknown height would place
    /// everything but the last-measured control wrong.
    pub fn layout(
        &self,
        content: &PopupContent,
        rows: &[String],
        dropped: usize,
        hidden_by_cap: usize,
        stale: Option<&str>,
        blocked: Option<&str>,
    ) -> (f64, f64) {
        for button in &self.row_buttons {
            button.setHidden(true);
        }
        self.prompt_field.setHidden(true);
        self.prompt_error.setHidden(true);
        self.busy_spinner.setHidden(true);
        unsafe { self.busy_spinner.stopAnimation(None) };
        self.busy_label.setHidden(true);
        self.stale_label.setHidden(true);
        self.blocked_label.setHidden(true);
        self.message_label.setHidden(true);

        let width = PANEL_WIDTH;
        let mut placements: Vec<Placement<'_>> = Vec::new();
        let mut y = CHROME_TOP;
        

        let hint = match content {
            PopupContent::Prompting { error } => {
                self.prompt_field.setHidden(false);
                placements.push(Placement { view: &self.prompt_field, x: 12.0, top: y, w: width - 24.0, h: 22.0 });
                y += 30.0;
                if let Some(e) = error {
                    self.prompt_error.setHidden(false);
                    self.prompt_error.setStringValue(&NSString::from_str(e));
                    placements.push(Placement { view: &self.prompt_error, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                    y += LINE_HEIGHT;
                }
                "Enter unlocks · Esc dismisses".to_string()
            }
            PopupContent::Unlocking | PopupContent::Locking | PopupContent::Syncing => {
                let text = match content {
                    PopupContent::Unlocking => "Unlocking vault…",
                    PopupContent::Locking => "Locking vault…",
                    _ => "Syncing…",
                };
                self.busy_spinner.setHidden(false);
                unsafe { self.busy_spinner.startAnimation(None) };
                placements.push(Placement { view: &self.busy_spinner, x: (width - 24.0) / 2.0, top: y, w: 24.0, h: 24.0 });
                y += 30.0;
                self.busy_label.setHidden(false);
                self.busy_label.setStringValue(&NSString::from_str(text));
                placements.push(Placement { view: &self.busy_label, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                y += LINE_HEIGHT;
                "Esc dismisses".to_string()
            }
            PopupContent::FetchingOtp { .. } => {
                self.busy_spinner.setHidden(false);
                unsafe { self.busy_spinner.startAnimation(None) };
                placements.push(Placement { view: &self.busy_spinner, x: (width - 24.0) / 2.0, top: y, w: 24.0, h: 24.0 });
                y += 30.0;
                self.busy_label.setHidden(false);
                self.busy_label.setStringValue(&NSString::from_str("Fetching code…"));
                placements.push(Placement { view: &self.busy_label, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                y += LINE_HEIGHT;
                "Esc returns to the list".to_string()
            }
            PopupContent::ShowingList { selected, message } => {
                if rows.is_empty() {
                    let button = &self.row_buttons[0];
                    button.setHidden(false);
                    button.setEnabled(false);
                    button.setTitle(&NSString::from_str("No items tagged for this app yet."));
                    placements.push(Placement { view: button, x: 4.0, top: y, w: width - 8.0, h: ROW_HEIGHT });
                    y += ROW_HEIGHT;
                } else {
                    for (i, (row_text, button)) in rows.iter().zip(&self.row_buttons).enumerate() {
                        button.setHidden(false);
                        button.setEnabled(true);
                        let title = if i == *selected {
                            format!("\u{2023} {row_text}")
                        } else {
                            format!("   {row_text}")
                        };
                        button.setTitle(&NSString::from_str(&title));
                        placements.push(Placement { view: button, x: 4.0, top: y, w: width - 8.0, h: ROW_HEIGHT });
                        y += ROW_HEIGHT;
                    }
                }
                if dropped > 0 {
                    self.message_label.setHidden(false);
                    self.message_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                    self.message_label.setStringValue(&NSString::from_str(&format!(
                        "({dropped} item(s) hidden — see log)"
                    )));
                    placements.push(Placement { view: &self.message_label, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                    y += LINE_HEIGHT;
                } else if hidden_by_cap > 0 {
                    self.message_label.setHidden(false);
                    self.message_label.setTextColor(Some(&NSColor::secondaryLabelColor()));
                    self.message_label.setStringValue(&NSString::from_str(&format!(
                        "({hidden_by_cap} more not shown — raise Max visible items in Settings)"
                    )));
                    placements.push(Placement { view: &self.message_label, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                    y += LINE_HEIGHT;
                } else if let Some(text) = message {
                    self.message_label.setHidden(false);
                    self.message_label.setTextColor(Some(&NSColor::systemRedColor()));
                    self.message_label.setStringValue(&NSString::from_str(text));
                    placements.push(Placement { view: &self.message_label, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                    y += LINE_HEIGHT;
                }
                if let Some(text) = stale {
                    self.stale_label.setHidden(false);
                    self.stale_label.setStringValue(&NSString::from_str(text));
                    placements.push(Placement { view: &self.stale_label, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                    y += LINE_HEIGHT;
                }
                if let Some(text) = blocked {
                    self.blocked_label.setHidden(false);
                    self.blocked_label.setStringValue(&NSString::from_str(text));
                    placements.push(Placement { view: &self.blocked_label, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                    y += LINE_HEIGHT;
                }
                "Click/Enter password · Shift+Enter user · Opt+Enter code".to_string()
            }
        };

        self.hint_label.setStringValue(&NSString::from_str(&hint));
        let height = y + CHROME_BOTTOM;

        placements.push(Placement { view: &self.title_label, x: 12.0, top: 10.0, w: width - 24.0, h: 16.0 });
        placements.push(Placement { view: &self.hint_label, x: 12.0, top: height - 20.0, w: width - 24.0, h: 14.0 });

        for p in placements {
            p.view.setFrame(NSRect::new(
                NSPoint::new(p.x, height - p.top - p.h),
                NSSize::new(p.w, p.h),
            ));
        }

        (width, height)
    }

    /// `x`/`y` are top-left-origin points — the same convention
    /// `platform::monitor::placement_for` returns and `win::window_style::
    /// place` consumes directly on Windows. AppKit's `setFrame:` instead
    /// wants the *bottom-left* corner, so the real bug this fixes (found
    /// in manual testing: the popup appearing in a seemingly unrelated
    /// spot relative to the cursor) was a missing flip right here —
    /// `platform::mac::monitor` converts *into* top-left space at its own
    /// boundary (screen geometry in, cursor position in) but nothing had
    /// converted back *out* before this, the one place a coordinate
    /// actually reaches a raw AppKit placement call.
    pub fn show_at(&self, x: i32, y: i32, w: i32, h: i32) {
        let primary_height = crate::platform::monitor::primary_height_pt();
        let appkit_y = primary_height - (y + h) as f64;
        self.panel.setFrame_display(
            NSRect::new(NSPoint::new(x as f64, appkit_y), NSSize::new(w as f64, h as f64)),
            false,
        );
        self.panel.makeKeyAndOrderFront(None);
    }

    pub fn hide(&self) {
        self.panel.makeFirstResponder(None);
        self.panel.orderOut(None);
    }

    pub fn is_key(&self) -> bool {
        self.panel.isKeyWindow()
    }
}

fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let font: Retained<NSFont> =
        if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
    field.setFont(Some(&font));
    field
}
