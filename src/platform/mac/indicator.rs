//! A small always-on-top, click-through, never-activating indicator shown
//! near the cursor during autotype delivery — the macOS counterpart of
//! `win::indicator`. A borderless, non-activating `NSPanel` with a custom
//! `NSView` subclass overriding `drawRect:`, porting that module's GDI
//! primitives to `NSBezierPath`/`NSColor` 1:1 (see the table in the port
//! plan). No `SetWindowRgn`-style circular clip is needed: a
//! clear-background, non-opaque `NSWindow` has no backing plate to clip in
//! the first place.
//!
//! `.nonactivatingPanel` plus never calling `makeKeyAndOrderFront:` (only
//! `orderFrontRegardless`) keeps it from ever taking foreground — delivery
//! polls `focus::is_foreground` and would abort if it did.
//! `setIgnoresMouseEvents(true)` is the click-through mechanism, the direct
//! counterpart of `HTTRANSPARENT` + `WS_EX_TRANSPARENT`.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSColor, NSLineCapStyle, NSLineJoinStyle, NSPanel, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize};

/// What the indicator currently displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Typing,
    Done,
}

struct ViewIvars {
    kind: Cell<Kind>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewIvars]
    struct IndicatorView;

    unsafe impl NSObjectProtocol for IndicatorView {}

    impl IndicatorView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty_rect: NSRect) {
            draw(dirty_rect.size, self.ivars().kind.get());
        }

        // Keeps the coordinate math identical to `win::indicator::draw`'s
        // top-left-origin GDI primitives — without this, AppKit's default
        // bottom-left-origin view space would mirror everything vertically.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl IndicatorView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            kind: Cell::new(Kind::Typing),
        });
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    static PANEL: RefCell<Option<Retained<NSPanel>>> = const { RefCell::new(None) };
    static VIEW: RefCell<Option<Retained<IndicatorView>>> = const { RefCell::new(None) };
}

/// Creates the (initially hidden, zero-sized) indicator panel the first
/// time it's needed, and reuses the same panel on every later call — same
/// lazy-once-then-reuse lifecycle as `win::indicator::ensure_window`.
fn ensure_panel(mtm: MainThreadMarker) -> (Retained<NSPanel>, Retained<IndicatorView>) {
    if let (Some(panel), Some(view)) = (
        PANEL.with(|p| p.borrow().clone()),
        VIEW.with(|v| v.borrow().clone()),
    ) {
        return (panel, view);
    }

    let view = IndicatorView::new(mtm);
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        NSPanel::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    // SAFETY: this panel is never used inside a window controller, so
    // disabling auto-release on close is required — same requirement
    // `objc2`'s own `hello_world_app` example notes for a bare `NSWindow`.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setLevel(objc2_app_kit::NSStatusWindowLevel);
    panel.setOpaque(false);
    panel.setHasShadow(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setIgnoresMouseEvents(true);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    // Without this, `focus::activate_target` deactivating our app (see
    // that module's doc) would hide the indicator at exactly the moment
    // delivery starts — the one time it must stay visible.
    panel.setHidesOnDeactivate(false);
    panel.setContentView(Some(&view));

    PANEL.with(|p| *p.borrow_mut() = Some(panel.clone()));
    VIEW.with(|v| *v.borrow_mut() = Some(view.clone()));
    (panel, view)
}

/// Shows the indicator as a `size`×`size` circle at `(x, y)` (top-left
/// origin, points — same coordinate space `platform::monitor::placement_for`
/// returns), displaying `kind`.
pub fn show(kind: Kind, x: i32, y: i32, size: i32) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let (panel, view) = ensure_panel(mtm);
    view.ivars().kind.set(kind);

    let primary_height = super::monitor::primary_height_pt();
    let origin = NSPoint {
        x: x as f64,
        y: primary_height - (y + size) as f64,
    };
    panel.setFrame_display(NSRect::new(origin, NSSize::new(size as f64, size as f64)), false);
    panel.orderFrontRegardless();
    view.setNeedsDisplay(true);
}

/// Switches what an already-shown indicator displays, in place — no
/// repositioning. Used for the typing-glyph-to-checkmark transition.
pub fn set_kind(kind: Kind) {
    if let Some(view) = VIEW.with(|v| v.borrow().clone()) {
        view.ivars().kind.set(kind);
        view.setNeedsDisplay(true);
    }
}

pub fn hide() {
    if let Some(panel) = PANEL.with(|p| p.borrow().clone()) {
        panel.orderOut(None);
    }
}

/// Hand-painted with `NSBezierPath` rather than an icon asset, porting
/// `win::indicator::draw`'s GDI primitives 1:1. `size` is the view's own
/// bounds (always square — `show` sets width == height), so `w == h`
/// throughout, unlike the Windows version which keeps them as independent
/// parameters for generality it never actually uses differently either.
fn draw(size: NSSize, kind: Kind) {
    let w = size.width;
    let h = size.height;
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let (bg, fg) = match kind {
        Kind::Typing => (rgb(51, 51, 55), rgb(235, 235, 235)),
        Kind::Done => (rgb(40, 167, 69), rgb(255, 255, 255)),
    };

    bg.setFill();
    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(NSPoint::new(0.0, 0.0), size)).fill();

    let stroke = (w.min(h) * 0.09).max(1.5).round();
    fg.setStroke();
    match kind {
        Kind::Typing => draw_keyboard(w, h, stroke, &fg),
        Kind::Done => draw_check(w, h, stroke),
    }
}

/// A rounded-rectangle outline with three small filled "keys" inside —
/// simpler and clearer at 32pt than a literal keyboard silhouette. Mirrors
/// `win::indicator::draw_keyboard`'s proportions exactly.
fn draw_keyboard(w: f64, h: f64, stroke: f64, fg: &Retained<NSColor>) {
    let margin_x = (w * 0.22).round();
    let margin_y = (h * 0.30).round();
    let left = margin_x;
    let top = margin_y;
    let right = w - margin_x;
    let bottom = h - margin_y;
    let corner = (((right - left).min(bottom - top)) * 0.35).round();

    let outline = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        NSRect::new(NSPoint::new(left, top), NSSize::new(right - left, bottom - top)),
        corner,
        corner,
    );
    outline.setLineWidth(stroke);
    outline.stroke();

    let inner_w = right - left;
    let inner_h = bottom - top;
    let key_w = (inner_w * 0.16).round();
    let key_h = (inner_h * 0.30).round();
    let gap = (inner_w * 0.10).round();
    let total = 3.0 * key_w + 2.0 * gap;
    let key_left = left + (inner_w - total) / 2.0;
    let key_top = top + (inner_h - key_h) / 2.0;

    fg.setFill();
    for i in 0..3 {
        let x = key_left + i as f64 * (key_w + gap);
        NSBezierPath::fillRect(NSRect::new(NSPoint::new(x, key_top), NSSize::new(key_w, key_h)));
    }
}

/// A two-segment checkmark, proportioned to the circle — mirrors
/// `win::indicator::draw_check`'s three points exactly.
fn draw_check(w: f64, h: f64, stroke: f64) {
    let points = [
        NSPoint::new(w * 0.26, h * 0.52),
        NSPoint::new(w * 0.43, h * 0.68),
        NSPoint::new(w * 0.75, h * 0.32),
    ];
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(stroke);
    path.setLineCapStyle(NSLineCapStyle::Round);
    path.setLineJoinStyle(NSLineJoinStyle::Round);
    path.moveToPoint(points[0]);
    path.lineToPoint(points[1]);
    path.lineToPoint(points[2]);
    path.stroke();
}

fn rgb(r: u8, g: u8, b: u8) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0,
        1.0,
    )
}
