//! The About window: version, license, copyright — read-only, no state to
//! carry, so unlike `settings.rs` this is just one `build` function
//! returning a ready-to-show `NSWindow`. Sized generously rather than
//! wrapped in an `NSScrollView`, so the license text fits without needing
//! to scroll — simpler for a window nobody interacts with beyond reading
//! it and closing it.

use objc2::rc::Retained;
use objc2::MainThreadOnly;
use objc2_app_kit::{NSBackingStoreType, NSColor, NSFont, NSTextField, NSTextView, NSView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

const LICENSE_TEXT: &str = include_str!("../../LICENSE.txt");
const WIDTH: f64 = 640.0;
// +132 over the license-only original: a heading, version, license name,
// copyright, and repository line above the text view — see `ui/about.rs`
// (Windows), which has always shown these; this window never did.
const HEIGHT: f64 = 612.0;

pub fn build(mtm: MainThreadMarker) -> Retained<NSWindow> {
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
    window.setTitle(&NSString::from_str("About Context Password"));
    window.center();

    let content = window.contentView().expect("window must have a content view");

    // Top-measured, grows downward — `place` converts to AppKit's
    // bottom-left frame origin internally (same convention `settings.rs`
    // uses; see that file for why getting this backwards is an easy and
    // very visible mistake).
    let mut y = 16.0;

    let heading = label(mtm, "Context Password", 15.0, true);
    place(&heading, &content, 12.0, y, WIDTH - 24.0, 20.0);
    y += 28.0;

    let version = label(mtm, &format!("Version {}", env!("CARGO_PKG_VERSION")), 12.0, false);
    place(&version, &content, 12.0, y, WIDTH - 24.0, 16.0);
    y += 20.0;

    let license = label(mtm, "MIT License", 12.0, false);
    place(&license, &content, 12.0, y, WIDTH - 24.0, 16.0);
    y += 20.0;

    let copyright = label(mtm, "Copyright \u{a9} 2026 Dominik Dzienia", 12.0, false);
    place(&copyright, &content, 12.0, y, WIDTH - 24.0, 16.0);
    y += 20.0;

    let repository = label(mtm, env!("CARGO_PKG_REPOSITORY"), 12.0, false);
    repository.setTextColor(Some(&NSColor::secondaryLabelColor()));
    place(&repository, &content, 12.0, y, WIDTH - 24.0, 16.0);
    y += 28.0;

    let text_view = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(NSPoint::new(12.0, 12.0), NSSize::new(WIDTH - 24.0, HEIGHT - y - 12.0)),
    );
    text_view.setEditable(false);
    let font = NSFont::userFixedPitchFontOfSize(11.0).unwrap_or_else(|| NSFont::systemFontOfSize(11.0));
    text_view.setFont(Some(&font));
    text_view.setString(&NSString::from_str(LICENSE_TEXT));
    content.addSubview(&text_view);

    window
}

fn label(mtm: MainThreadMarker, text: &str, size: f64, bold: bool) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let font: Retained<NSFont> =
        if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
    field.setFont(Some(&font));
    field
}

fn place(view: &NSTextField, content: &NSView, x: f64, top: f64, w: f64, h: f64) {
    view.setFrame(NSRect::new(NSPoint::new(x, HEIGHT - top - h), NSSize::new(w, h)));
    content.addSubview(view);
}
