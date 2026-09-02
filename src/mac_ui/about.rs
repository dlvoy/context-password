//! The About window: version, license, copyright — read-only, no state to
//! carry, so unlike `settings.rs` this is just one `build` function
//! returning a ready-to-show `NSWindow`. Sized generously rather than
//! wrapped in an `NSScrollView`, so the license text fits without needing
//! to scroll — simpler for a window nobody interacts with beyond reading
//! it and closing it.

use objc2::rc::Retained;
use objc2::MainThreadOnly;
use objc2_app_kit::{NSBackingStoreType, NSFont, NSTextView, NSWindow, NSWindowStyleMask};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

const LICENSE_TEXT: &str = include_str!("../../LICENSE.txt");
const WIDTH: f64 = 640.0;
const HEIGHT: f64 = 480.0;

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
    window.setTitle(&NSString::from_str("About context-password"));
    window.center();

    let text_view = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(NSPoint::new(12.0, 12.0), NSSize::new(WIDTH - 24.0, HEIGHT - 24.0)),
    );
    text_view.setEditable(false);
    let font = NSFont::userFixedPitchFontOfSize(11.0).unwrap_or_else(|| NSFont::systemFontOfSize(11.0));
    text_view.setFont(Some(&font));
    text_view.setString(&NSString::from_str(LICENSE_TEXT));

    let content = window.contentView().expect("window must have a content view");
    content.addSubview(&text_view);

    window
}
