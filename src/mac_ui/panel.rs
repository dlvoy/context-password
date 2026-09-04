//! The popup panel — a visually chrome-free, non-activating `NSPanel`
//! hosting plain AppKit controls (no custom drawing here, unlike
//! `platform::mac::indicator`), toggled visible/hidden per `PopupContent`
//! state rather than rebuilt each time.
//!
//! `.nonactivatingPanel` is what lets this take keyboard input (the secure
//! text field, arrow-key list navigation) without the panel *itself*
//! needing to be the active application's key window in the usual sense.
//! It does **not**, however, mean the show sequence can skip activation
//! altogether — found in manual testing, `app_delegate::show_popup` briefly
//! activates the app before raising this panel (see that function's doc),
//! because `makeKeyAndOrderFront:` alone won't reliably raise a window
//! above other applications' windows while this process stays inactive.
//! `show_at` (placement) and `raise` (ordering) are deliberately two
//! separate calls so the caller can activate in between.
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

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSBezierPath, NSButton, NSColor, NSEvent, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSLineCapStyle, NSPanel,
    NSProgressIndicator, NSProgressIndicatorStyle, NSSecureTextField, NSStringDrawing,
    NSTextAlignment, NSTextContent, NSTextContentTypeOneTimeCode, NSTextField, NSTextFieldDelegate,
    NSTextView, NSView, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use super::state::{DeliveryKind, PopupContent};

// The blue rounded band drawn behind whichever row is currently selected —
// replaces the old `‣` prefix character, which was the list's only
// selection affordance and forced every row's title to be centred to keep
// the marker's width from shifting the text. Hand-painted the same way
// `platform::mac::indicator::IndicatorView` is: a custom `NSView` subclass
// overriding `drawRect:`, not a `CALayer` background, so this needs no new
// dependency (`objc2-quartz-core`). A plain `//` comment, not `///` —
// `define_class!`'s expansion doesn't carry a doc comment through to the
// generated struct, so `///` here just warns as unused.
define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    struct SelectionBand;

    unsafe impl NSObjectProtocol for SelectionBand {}

    impl SelectionBand {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            NSColor::selectedContentBackgroundColor().setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), 6.0, 6.0).fill();
        }
    }
);

impl SelectionBand {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

// The white card behind the whole row area — without it, rows sat directly
// on the panel's own `windowBackgroundColor` (a light warm gray close
// enough to the row text's tone that unselected rows didn't read as a
// distinct list at all, found in manual testing). Deliberately a hardcoded
// true white (`NSColor::whiteColor`), not the semantic
// `textBackgroundColor` tried first — that resolved to a faint cool
// gray-blue rather than actual white in manual testing (likely tinted by
// the system accent color), which is exactly the low-contrast look this
// card exists to fix. Paired with `NSColor::blackColor` on row text below,
// not `labelColor`/`textColor` (see the `layout` `ShowingList` arm) for
// the same reason: those render as a slightly-transparent near-black that
// still read as "dark gray" against white. This trades away automatic
// dark-mode adaptation for an unambiguous match to what was asked for; if
// a dark popup theme is wanted later this card and its text will need
// their own explicit dark-mode pair, not a semantic color that turned out
// not to mean what its name suggests here. Same `drawRect:`-over-
// `NSBezierPath` pattern as `SelectionBand`, just a second, larger
// instance of the same idea — kept as its own type rather than a generic
// "rounded rect view" the two share, since each currently has exactly one
// call site and a shared abstraction would be pure indirection for two
// lines of real logic.
define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    struct ListCard;

    unsafe impl NSObjectProtocol for ListCard {}

    impl ListCard {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            NSColor::whiteColor().setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), 8.0, 8.0).fill();
        }
    }
);

impl ListCard {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

// The per-row mode glyph — keyhole (password), person (username), or clock
// (OTP) — reflecting whichever modifier is currently held, ported 1:1 from
// the Windows front-end's hand-painted `egui::Painter` version
// (`ui/popup.rs`'s `paint_icon`) to `NSBezierPath`. `isFlipped` makes this
// view's own coordinate space top-left-origin/y-down, matching egui's
// convention exactly, so the geometry below is the same numbers as the
// Windows source, not re-derived.
struct RowIconIvars {
    mode: Cell<DeliveryKind>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = RowIconIvars]
    struct RowIcon;

    unsafe impl NSObjectProtocol for RowIcon {}

    impl RowIcon {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            draw_icon(self.bounds().size, self.ivars().mode.get());
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl RowIcon {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RowIconIvars { mode: Cell::new(DeliveryKind::Password) });
        unsafe { msg_send![super(this), init] }
    }

    /// No-op (and no redraw) when `mode` hasn't actually changed — called
    /// on every visible row on every `layout()`, so this keeps a steady
    /// list (no held modifier) from repainting rows that look identical.
    fn set_mode(&self, mode: DeliveryKind) {
        if self.ivars().mode.get() != mode {
            self.ivars().mode.set(mode);
            self.setNeedsDisplay(true);
        }
    }
}

/// Ported from `ui/popup.rs::paint_icon` — same proportions (`ICON_SIZE`
/// fractions), same three glyphs. One deliberate simplification: the
/// Windows version rounds only the person icon's top two corners
/// (`egui::CornerRadius` with `sw`/`se` at 0); `NSBezierPath` has no single
/// convenience for per-corner rounding, and building one by hand isn't
/// worth it for a ~6×3pt shape where the difference is imperceptible — a
/// uniformly rounded rect is used instead.
fn draw_icon(size: NSSize, mode: DeliveryKind) {
    let s = size.width.min(size.height);
    let center = NSPoint::new(size.width / 2.0, size.height / 2.0);
    let color = NSColor::labelColor();
    color.setFill();
    color.setStroke();
    match mode {
        DeliveryKind::Password => {
            let hole_r = s * 0.2;
            let hole_center = NSPoint::new(center.x, center.y - s * 0.12);
            circle(hole_center, hole_r).fill();
            let top_half = hole_r * 0.65;
            let bottom_half = hole_r * 0.32;
            let bottom_y = hole_center.y + s * 0.38;
            let wedge = NSBezierPath::bezierPath();
            wedge.moveToPoint(NSPoint::new(hole_center.x - top_half, hole_center.y));
            wedge.lineToPoint(NSPoint::new(hole_center.x + top_half, hole_center.y));
            wedge.lineToPoint(NSPoint::new(hole_center.x + bottom_half, bottom_y));
            wedge.lineToPoint(NSPoint::new(hole_center.x - bottom_half, bottom_y));
            wedge.closePath();
            wedge.fill();
        }
        DeliveryKind::Username => {
            let head_r = s * 0.2;
            let head_center = NSPoint::new(center.x, center.y - s * 0.18);
            circle(head_center, head_r).fill();
            let sw = s * 0.62;
            let sh = s * 0.34;
            let shoulders_center = NSPoint::new(center.x, center.y + s * 0.22);
            let corner = (sw.min(sh) * 0.3).min(sh / 2.0);
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                NSRect::new(
                    NSPoint::new(shoulders_center.x - sw / 2.0, shoulders_center.y - sh / 2.0),
                    NSSize::new(sw, sh),
                ),
                corner,
                corner,
            )
            .fill();
        }
        DeliveryKind::Otp => {
            let r = s * 0.34;
            let ring = circle(center, r);
            ring.setLineWidth(1.5);
            ring.stroke();
            let hands = NSBezierPath::bezierPath();
            hands.setLineWidth(1.5);
            hands.setLineCapStyle(NSLineCapStyle::Round);
            hands.moveToPoint(center);
            hands.lineToPoint(NSPoint::new(center.x, center.y - r * 0.6));
            hands.moveToPoint(center);
            hands.lineToPoint(NSPoint::new(center.x + r * 0.45, center.y + r * 0.15));
            hands.stroke();
        }
    }
}

fn circle(center: NSPoint, radius: f64) -> Retained<NSBezierPath> {
    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(center.x - radius, center.y - radius),
        NSSize::new(radius * 2.0, radius * 2.0),
    ))
}

// The blue quick-select digit badge — ported from `ui/popup.rs::paint_badge`.
// Unlike `RowIcon`, this never changes after construction: row slot *i*
// always shows the same digit regardless of which entry currently occupies
// it, so the digit lives in `set_ivars` directly rather than a `Cell`.
struct RowBadgeIvars {
    digit: char,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = RowBadgeIvars]
    struct RowBadge;

    unsafe impl NSObjectProtocol for RowBadge {}

    impl RowBadge {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            draw_badge(self.bounds(), self.ivars().digit);
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl RowBadge {
    fn new(mtm: MainThreadMarker, digit: char) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RowBadgeIvars { digit });
        unsafe { msg_send![super(this), init] }
    }
}

/// The exact blue Windows uses (`ui/popup.rs::BADGE_COLOR`) — ported as a
/// literal, not a semantic system color, for the same reason `ListCard`
/// and its row text settled on hardcoded white/black: a specific requested
/// look, not one that should shift with system accent color.
fn draw_badge(bounds: NSRect, digit: char) {
    rgb(0x2e, 0x6b, 0xd6).setFill();
    NSBezierPath::bezierPathWithOvalInRect(bounds).fill();

    let text = NSString::from_str(&digit.to_string());
    // SAFETY: `NSFont`/`NSColor` are ordinary Objective-C objects; casting
    // to the untyped root type is exactly what building a heterogeneous
    // `{NSAttributedStringKey: AnyObject}` attributes dictionary requires
    // (the same thing Swift does implicitly for `[.font: font, ...]`).
    let font: Retained<AnyObject> =
        unsafe { Retained::cast_unchecked(NSFont::boldSystemFontOfSize(11.0)) };
    let color: Retained<AnyObject> = unsafe { Retained::cast_unchecked(NSColor::whiteColor()) };
    let keys: [&NSString; 2] = [unsafe { NSFontAttributeName }, unsafe { NSForegroundColorAttributeName }];
    let values: [&AnyObject; 2] = [&font, &color];
    let attrs = NSDictionary::from_slices(&keys, &values);

    let size = unsafe { text.sizeWithAttributes(Some(&attrs)) };
    let origin = NSPoint::new(
        bounds.origin.x + (bounds.size.width - size.width) / 2.0,
        bounds.origin.y + (bounds.size.height - size.height) / 2.0,
    );
    unsafe { text.drawAtPoint_withAttributes(origin, Some(&attrs)) };
}

fn rgb(r: u8, g: u8, b: u8) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0)
}

// A digit key (1-9, 0) isn't bound to any `NSResponder` command selector,
// so it never reaches `control:textView:doCommandBySelector:` below — it
// goes straight to `insertText:` via the normal `interpretKeyEvents:`
// pipeline. Two things were tried and failed before this one, both
// confirmed by manual testing (debug logging showed neither ever fired):
// `NSTextViewDelegate`'s `textView:shouldChangeTextInRange:
// replacementString:` — `NSTextField`'s delegate relay only forwards a
// curated subset of `NSTextViewDelegate` (the methods behind
// `NSControlTextEditingDelegate`), and that one isn't in it; and a
// `keyDown:` override on `list_key_catcher` itself — irrelevant, because
// `NSTextField.becomeFirstResponder` installs the *window's shared field
// editor* (a private `NSTextView`) as the actual first responder once
// editing begins, not the `NSTextField` instance, so `keyDown:` never
// reaches it at all.
//
// `NSWindowDelegate.windowWillReturnFieldEditor:toObject:` is Apple's
// documented mechanism for supplying a *custom* field editor for a
// specific client instead of the generic shared one, and is what actually
// works: `AppDelegate` implements it (see `app_delegate.rs`), asks
// `Panel::field_editor_for` whether the requesting client is
// `list_key_catcher`, and if so hands back this type instead. A digit key
// is swallowed here — delegated to `AppDelegate::handleDigitKeyPress` via
// a direct `msg_send!`, the same "call back into the delegate" shape
// `rowClicked:`'s target/action already uses, just invoked without
// `NSControl`'s send-action machinery — anything else falls through to
// `super.keyDown:`, so arrow/Enter/Escape handling
// (`control:textView:doCommandBySelector:`, still relayed via
// `list_key_catcher`'s own delegate exactly as before) is unaffected. This
// editor is only ever installed for `list_key_catcher`'s requests — never
// while the master password prompt (`prompt_field`, its own ordinary
// `NSSecureTextField`, untouched here) is focused — so there's no risk of
// ever intercepting a keystroke meant for it.
struct KeyCatcherEditorIvars {
    delegate: Retained<AnyObject>,
}

define_class!(
    #[unsafe(super = NSTextView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = KeyCatcherEditorIvars]
    struct KeyCatcherEditor;

    unsafe impl NSObjectProtocol for KeyCatcherEditor {}

    impl KeyCatcherEditor {
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let handled: bool = unsafe { msg_send![&*self.ivars().delegate, handleDigitKeyPress] };
            if !handled {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }
    }
);

impl KeyCatcherEditor {
    fn new(mtm: MainThreadMarker, delegate: &ProtocolObject<dyn NSTextFieldDelegate>) -> Retained<Self> {
        // SAFETY: `delegate` is `AppDelegate`, which outlives every `Panel`
        // it owns — this just takes out our own strong reference to the
        // same live object `setDelegate` elsewhere also hands to Cocoa's
        // object graph (untracked by Rust either way).
        let delegate: Retained<AnyObject> = unsafe {
            Retained::retain(delegate.as_ref() as *const AnyObject as *mut AnyObject)
                .expect("delegate must not be null")
        };
        let this = Self::alloc(mtm).set_ivars(KeyCatcherEditorIvars { delegate });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        // Required by Apple's docs for any custom field editor instance —
        // marks it as a shared, reusable field editor rather than an
        // ordinary standalone text view.
        this.setFieldEditor(true);
        this
    }
}

/// Everything `AppDelegate`'s `windowWillReturnFieldEditor:toObject:` needs
/// to resolve a request, deliberately independent of `AppState`. Found in
/// manual testing to matter for a subtle reason: `Panel::layout` setting an
/// `NSControl`'s string value while `list_key_catcher`'s custom field
/// editor is active can make AppKit re-query the window for a field editor
/// *reentrantly*, from inside that very `setStringValue:` call — which
/// itself runs from inside `on_tick`'s `state.borrow_mut()`. A delegate
/// method that tried to `state.borrow()` at that point paniced
/// (`RefCell` already mutably borrowed) every time the popup opened.
/// `Panel::field_editor_routing` snapshots just the two cheap values this
/// needs (a raw pointer for identity comparison, and one more
/// `Retained` handle to the already-existing editor) once, so resolving a
/// request never has to touch the `RefCell` at all — safe to call however
/// deep in the call stack AppKit decides to invoke it from.
pub struct FieldEditorRouting {
    list_key_catcher: *const (),
    list_editor: Retained<AnyObject>,
}

impl FieldEditorRouting {
    /// `client` identifies which object is asking the window for a field
    /// editor to type into. Returns the custom `KeyCatcherEditor` only
    /// when it's specifically `list_key_catcher` asking (compared by
    /// identity, not any `PartialEq` on the AppKit objects themselves);
    /// `None` for every other client — in particular `prompt_field` — so
    /// AppKit supplies its own ordinary shared field editor there,
    /// completely unaffected.
    pub fn resolve(&self, client: Option<&AnyObject>) -> Option<Retained<AnyObject>> {
        let client = client?;
        let is_catcher = std::ptr::eq(client as *const AnyObject as *const (), self.list_key_catcher);
        is_catcher.then(|| self.list_editor.clone())
    }
}

pub const PANEL_WIDTH: f64 = 340.0;
const ROW_HEIGHT: f64 = 28.0;
const LINE_HEIGHT: f64 = 20.0;
const CHROME_TOP: f64 = 34.0;
const CHROME_BOTTOM: f64 = 26.0;
const ICON_SIZE: f64 = 16.0;
const BADGE_SIZE: f64 = 18.0;
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
    list_editor: Retained<KeyCatcherEditor>,
    prompt_error: Retained<NSTextField>,
    busy_spinner: Retained<NSProgressIndicator>,
    busy_label: Retained<NSTextField>,
    list_card: Retained<ListCard>,
    selection_band: Retained<SelectionBand>,
    row_buttons: Vec<Retained<NSButton>>,
    row_icons: Vec<Retained<RowIcon>>,
    row_badges: Vec<Retained<RowBadge>>,
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
        // `NSStatusWindowLevel`, not `NSFloatingWindowLevel` — the same
        // level `platform::mac::indicator` already uses. At the floating
        // level, another app's own floating-level window (a conferencing
        // toolbar, an always-on-top utility) can tie or beat this panel and
        // cover it; found in manual testing to be one contributor to the
        // popup occasionally opening behind another window.
        panel.setLevel(objc2_app_kit::NSStatusWindowLevel);
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
        // The custom field editor `windowWillReturnFieldEditor:toObject:`
        // hands back specifically when `list_key_catcher` requests one —
        // see `KeyCatcherEditor`'s doc for why this, and not the field
        // itself, is where digit-key interception has to live.
        let list_editor = KeyCatcherEditor::new(mtm, delegate);

        let prompt_error = label(mtm, "", 11.0, false);
        prompt_error.setTextColor(Some(&NSColor::systemRedColor()));

        let busy_spinner = NSProgressIndicator::new(mtm);
        busy_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        busy_spinner.setIndeterminate(true);

        let busy_label = label(mtm, "", 12.0, false);

        let list_card = ListCard::new(mtm);
        list_card.setHidden(true);
        let selection_band = SelectionBand::new(mtm);
        selection_band.setHidden(true);

        let mut row_buttons = Vec::with_capacity(MAX_ROWS);
        let mut row_icons = Vec::with_capacity(MAX_ROWS);
        let mut row_badges = Vec::with_capacity(MAX_ROWS);
        for i in 0..MAX_ROWS {
            let button = NSButton::new(mtm);
            button.setBezelStyle(NSBezelStyle::Automatic);
            button.setBordered(false);
            button.setAlignment(NSTextAlignment::Left);
            unsafe {
                button.setTarget(Some(delegate.as_ref()));
                button.setAction(Some(sel!(rowClicked:)));
            }
            button.setTag(i as isize);
            button.setHidden(true);
            row_buttons.push(button);

            let icon = RowIcon::new(mtm);
            icon.setHidden(true);
            row_icons.push(icon);

            // Slot i always shows the same digit regardless of which entry
            // ends up occupying it — set once here, never touched again.
            let digit = if i == 9 { '0' } else { char::from(b'1' + i as u8) };
            let badge = RowBadge::new(mtm, digit);
            badge.setHidden(true);
            row_badges.push(badge);
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
        // Card, then band, then row buttons, then icons/badges — AppKit
        // stacks subviews in the order they're added, later on top: white
        // card behind everything, the selected row's blue band on top of
        // the card, the row text on top of both, and the icon/badge
        // topmost of all so they stay visible over the band regardless of
        // any incidental overlap at their shared row edges.
        content_view.addSubview(&list_card);
        content_view.addSubview(&selection_band);
        for button in &row_buttons {
            content_view.addSubview(button);
        }
        for icon in &row_icons {
            content_view.addSubview(icon);
        }
        for badge in &row_badges {
            content_view.addSubview(badge);
        }

        Self {
            panel,
            title_label,
            hint_label,
            prompt_field,
            list_key_catcher,
            list_editor,
            prompt_error,
            busy_spinner,
            busy_label,
            list_card,
            selection_band,
            row_buttons,
            row_icons,
            row_badges,
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
        let mut value = self.prompt_field.stringValue().to_string();
        self.prompt_field.setStringValue(&NSString::from_str(""));
        // Defensive: found in manual testing that the field's value read
        // from inside `insertNewline:`'s handler could carry a trailing
        // newline even though the field editor's default insertion is
        // suppressed (`handle_command` always returns `true` for it) — a
        // real password never legitimately ends in one, so stripping it
        // here is safe regardless of the exact AppKit timing at play, and
        // is what was silently turning a correct password into a wrong one
        // (a different byte string hashes to a different key — "crypto
        // error" despite the visible password being right).
        while matches!(value.chars().next_back(), Some('\n' | '\r')) {
            value.pop();
        }
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

    /// A cheap, `AppState`-independent snapshot of what
    /// `windowWillReturnFieldEditor:toObject:` needs — see
    /// `FieldEditorRouting`'s own doc for why this exists as a separate
    /// type rather than a `&Panel` method.
    pub fn field_editor_routing(&self) -> FieldEditorRouting {
        FieldEditorRouting {
            list_key_catcher: &*self.list_key_catcher as *const NSTextField as *const (),
            // SAFETY: `KeyCatcherEditor`'s superclass is `NSTextView`,
            // itself an ordinary `NSObject` subclass — reinterpreting as
            // the untyped root type is sound for any Objective-C object.
            list_editor: unsafe { Retained::cast_unchecked(self.list_editor.clone()) },
        }
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
        mode: DeliveryKind,
        hidden_by_cap: usize,
        stale: Option<&str>,
        blocked: Option<&str>,
    ) -> (f64, f64) {
        for button in &self.row_buttons {
            button.setHidden(true);
            button.setContentTintColor(None);
        }
        for icon in &self.row_icons {
            icon.setHidden(true);
        }
        for badge in &self.row_badges {
            badge.setHidden(true);
        }
        self.list_card.setHidden(true);
        self.selection_band.setHidden(true);
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

        let title = title_for(content);

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
                "⏎ unlock · ⎋ dismiss".to_string()
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
                "⎋ dismiss".to_string()
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
                "⎋ back to list".to_string()
            }
            PopupContent::ShowingList { selected, message } => {
                // The white card sits inset 12pt from the panel edges —
                // the same inset every text line in this dialog already
                // uses (title, hint, message/stale/blocked) — and the band
                // and row text nest further inside it, each with their own
                // small margin, so the selection highlight never touches
                // the card's rounded corners and the row text never sits
                // flush against the card's edge. See the previous plan's
                // Fix 7 for the reasoning behind these specific insets.
                let card_x = 12.0;
                let card_w = width - 24.0;
                let band_x = card_x + 4.0;
                let band_w = card_w - 8.0;
                // Icon at the band's left edge, badge at its right edge,
                // row text narrowed to the gap between them — replaces the
                // earlier round's simpler "just inset from the band" span
                // now that the icon/badge occupy real space there.
                let icon_x = band_x + 8.0;
                let badge_x = band_x + band_w - 8.0 - BADGE_SIZE;
                let text_x = icon_x + ICON_SIZE + 8.0;
                let text_w = badge_x - 8.0 - text_x;
                let card_top = y;
                self.list_card.setHidden(false);

                if rows.is_empty() {
                    let button = &self.row_buttons[0];
                    button.setHidden(false);
                    button.setEnabled(false);
                    button.setTitle(&NSString::from_str("No items tagged for this app yet."));
                    // No icon/badge for the placeholder row — it isn't a
                    // real entry, so it spans the full band width instead
                    // of leaving room for either.
                    placements.push(Placement { view: button, x: band_x + 10.0, top: y, w: band_w - 14.0, h: ROW_HEIGHT });
                    y += ROW_HEIGHT;
                } else {
                    let icon_top_offset = (ROW_HEIGHT - ICON_SIZE) / 2.0;
                    let badge_top_offset = (ROW_HEIGHT - BADGE_SIZE) / 2.0;
                    for (i, (row_text, button)) in rows.iter().zip(&self.row_buttons).enumerate() {
                        button.setHidden(false);
                        button.setEnabled(true);
                        button.setTitle(&NSString::from_str(row_text));
                        let row_placement =
                            Placement { view: button, x: text_x, top: y, w: text_w, h: ROW_HEIGHT };
                        if i == *selected {
                            button.setContentTintColor(Some(&NSColor::alternateSelectedControlTextColor()));
                            self.selection_band.setHidden(false);
                            // Deferred into `placements` like everything
                            // else here, not set directly, since the final
                            // AppKit frame depends on `height`, not known
                            // until every row has been measured.
                            placements.push(Placement { view: &self.selection_band, x: band_x, top: y, w: band_w, h: ROW_HEIGHT });
                        } else {
                            // Explicit true black, not left at the
                            // reset-loop's `None` (the button's own default
                            // title color) — that default renders as a
                            // muted dark gray against the card's white,
                            // which read as low-contrast in manual testing
                            // the same way the card's own color did.
                            button.setContentTintColor(Some(&NSColor::blackColor()));
                        }
                        placements.push(row_placement);

                        let icon = &self.row_icons[i];
                        icon.setHidden(false);
                        icon.set_mode(mode);
                        placements.push(Placement { view: icon, x: icon_x, top: y + icon_top_offset, w: ICON_SIZE, h: ICON_SIZE });

                        let badge = &self.row_badges[i];
                        badge.setHidden(false);
                        placements.push(Placement { view: badge, x: badge_x, top: y + badge_top_offset, w: BADGE_SIZE, h: BADGE_SIZE });

                        y += ROW_HEIGHT;
                    }
                }
                // Sized to exactly enclose the row area just measured —
                // pushed after the loop, once `y` reflects the last row's
                // bottom edge.
                placements.push(Placement { view: &self.list_card, x: card_x, top: card_top, w: card_w, h: y - card_top });

                // The hidden-by-tagging count is deliberately not shown here
                // (it's still logged) — KeePass in particular reads a whole
                // local vault with no server-side prefilter, so on a real
                // vault this line was permanently on screen and not
                // actionable from the popup itself.
                if hidden_by_cap > 0 {
                    // A lesser aside, not primary chrome: smaller and
                    // lighter than the footer hint, with its own breathing
                    // room below the card rather than sitting flush under
                    // the last row.
                    y += 6.0;
                    self.message_label.setHidden(false);
                    self.message_label.setFont(Some(&NSFont::systemFontOfSize(10.0)));
                    self.message_label.setTextColor(Some(&NSColor::tertiaryLabelColor()));
                    self.message_label.setStringValue(&NSString::from_str(&format!(
                        "({hidden_by_cap} more not shown — raise Max visible items in Settings)"
                    )));
                    placements.push(Placement { view: &self.message_label, x: 12.0, top: y, w: width - 24.0, h: 16.0 });
                    y += LINE_HEIGHT;
                } else if let Some(text) = message {
                    self.message_label.setHidden(false);
                    self.message_label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
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
                "⏎ password · ⇧⏎ user · ⌥⏎ OTP".to_string()
            }
        };

        self.title_label.setStringValue(&NSString::from_str(title));
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
    }

    /// Actually brings the panel to the front — split out from `show_at` so
    /// the caller can activate the app *first* (see `app_delegate::show_popup`'s
    /// doc for why that ordering matters) and only then raise. Two calls,
    /// not one: `makeKeyAndOrderFront:` is what makes the panel able to take
    /// keyboard input at all (see `setBecomesKeyOnlyIfNeeded` above), but
    /// Apple documents it as *not* raising a window above other
    /// applications' windows while this process is inactive —
    /// `orderFrontRegardless()` is the documented call for that (already
    /// used the same way by `platform::mac::indicator`), kept as a
    /// belt-and-braces raise for the case where `activateIgnoringOtherApps`
    /// gets deferred or refused (an unbundled binary, or macOS 14+'s
    /// activation-yield rules) rather than the popup silently staying
    /// behind whatever was frontmost.
    pub fn raise(&self) {
        self.panel.makeKeyAndOrderFront(None);
        self.panel.orderFrontRegardless();
    }

    /// Grows/shrinks the panel to `(w, h)` while keeping its current
    /// top-left corner fixed, and only touches the frame at all if the size
    /// actually changed. For a background refresh of an already-visible
    /// list (`app_delegate::refresh_list`) — `layout` re-measures and
    /// re-positions every control against whatever `height` it computes,
    /// so leaving the window's actual frame at its old size after a
    /// row-count change (an external KeePass edit picked up by Sync, an
    /// item appearing or disappearing) let content slide around inside a
    /// frame that no longer matched it. `show_at` deliberately does not
    /// share this: the initial open also needs to reposition to track the
    /// cursor, not just resize.
    pub fn resize_keeping_top_left(&self, w: f64, h: f64) {
        let frame = self.panel.frame();
        if (frame.size.width - w).abs() < 0.5 && (frame.size.height - h).abs() < 0.5 {
            return;
        }
        let top = frame.origin.y + frame.size.height;
        self.panel.setFrame_display(
            NSRect::new(NSPoint::new(frame.origin.x, top - h), NSSize::new(w, h)),
            false,
        );
    }

    pub fn hide(&self) {
        self.panel.makeFirstResponder(None);
        self.panel.orderOut(None);
    }

    pub fn is_key(&self) -> bool {
        self.panel.isKeyWindow()
    }
}

/// The popup's own heading (the `NSWindow` title is hidden — see the
/// module doc on why this panel is `.titled` but not visibly so — so this
/// label is the only title the user ever sees). Names the app and, past
/// the initial prompt, what it's currently doing, rather than staying a
/// static `"context-password"` regardless of state.
fn title_for(content: &PopupContent) -> &'static str {
    match content {
        PopupContent::Prompting { .. } => "Context Password — Unlock",
        PopupContent::Unlocking => "Context Password — Unlocking",
        PopupContent::Locking => "Context Password — Locking",
        PopupContent::Syncing => "Context Password — Syncing",
        PopupContent::FetchingOtp { .. } => "Context Password — Fetching OTP",
        PopupContent::ShowingList { .. } => "Context Password",
    }
}

fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let font: Retained<NSFont> =
        if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
    field.setFont(Some(&font));
    field
}
