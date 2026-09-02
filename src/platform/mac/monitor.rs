//! Monitor-aware popup placement — the macOS counterpart of
//! `win::monitor`. Delegates the actual flip-then-clamp math to
//! `platform::place::place_within`, which is shared verbatim with Windows
//! and stays coordinate-system-agnostic; this module's only job is
//! converting AppKit's bottom-left-origin screen geometry into the
//! top-left-origin space `place_within` expects, and converting its answer
//! back.
//!
//! Unlike Windows, AppKit's window/screen geometry is already
//! DPI-independent (in points, not physical pixels) — `backingScaleFactor`
//! is a pixel-buffer ratio for drawing, not a geometry multiplier, so
//! there's no `win::monitor`-style DPI scaling step here: `Placement`'s
//! `w`/`h` are simply `w_pt`/`h_pt` unchanged.

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_foundation::NSPoint;

use crate::platform::place::{self, Rect};

/// Matches `win::monitor::Placement`'s shape so call sites (`app.rs`,
/// eventually `mac_ui`) don't need to know which platform they're on.
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Default offset from the cursor, in points, before flip/clamp — same
/// value as `win::monitor::CURSOR_OFFSET_PT` (no DPI scaling to apply it
/// through here).
const CURSOR_OFFSET_PT: i32 = 8;

pub fn placement_for(cursor: Option<(i32, i32)>, w_pt: i32, h_pt: i32) -> Placement {
    let work = monitor_metrics(cursor);
    let (x, y) = place::place_within(work, cursor, w_pt, h_pt, CURSOR_OFFSET_PT);
    Placement {
        x,
        y,
        w: w_pt,
        h: h_pt,
    }
}

/// The live AppKit half: which screen is under `cursor` (or the primary/
/// menu-bar screen if `None`), and its visible work area — converted into
/// the top-left-origin space `place_within` expects. Not unit-tested, same
/// reasoning as `win::monitor::monitor_metrics`: a thin wrapper over
/// `NSScreen` queries with an obvious fallback if they come back empty.
fn monitor_metrics(cursor: Option<(i32, i32)>) -> Rect {
    let mtm = MainThreadMarker::new().expect("monitor queries must run on the main thread");
    let screens = NSScreen::screens(mtm);
    let primary = screens.firstObject();

    // `screens()[0]` is documented by Apple as the screen anchoring the
    // global coordinate system's origin — the menu-bar screen — which is
    // NOT necessarily `NSScreen::mainScreen()` (that follows the key
    // window instead). `primary_height` is the flip constant every other
    // screen's geometry is converted against.
    let primary_height = primary_height_pt();

    let target = match (cursor, &primary) {
        (Some((x, y_top)), _) => {
            let appkit_point = NSPoint {
                x: x as f64,
                y: primary_height - y_top as f64,
            };
            screens
                .iter()
                .find(|s| ns_rect_contains(s.frame(), appkit_point))
                .or_else(|| primary.clone())
        }
        (None, p) => p.clone(),
    };

    match target {
        Some(screen) => {
            let vf = screen.visibleFrame();
            Rect {
                left: vf.origin.x.round() as i32,
                right: (vf.origin.x + vf.size.width).round() as i32,
                top: (primary_height - (vf.origin.y + vf.size.height)).round() as i32,
                bottom: (primary_height - vf.origin.y).round() as i32,
            }
        }
        // Should not happen — even a headless CI runner reports a virtual
        // display — but a window a few thousand points off-screen is a
        // poor failure mode, same posture as `win::monitor`'s fallback.
        None => Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        },
    }
}

fn ns_rect_contains(rect: objc2_foundation::NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y < rect.origin.y + rect.size.height
}

/// The primary/menu-bar screen's full height — the Y-flip constant every
/// other screen's geometry (and any raw AppKit point, e.g.
/// `NSEvent::mouseLocation()`) is converted against. `pub(crate)` so
/// `focus::cursor_pos` can share it rather than re-deriving it.
pub(crate) fn primary_height_pt() -> f64 {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(1080.0, |s| s.frame().size.height)
}
