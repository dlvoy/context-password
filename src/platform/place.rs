//! The flip-then-clamp popup-placement math (plan §9, resolves spike S3),
//! shared by every OS's `monitor` module. Pure — no OS calls — so it's
//! unit-tested directly here rather than through a real monitor query.
//!
//! Each platform's `monitor::monitor_metrics` (or macOS equivalent) is
//! responsible for producing a top-left-origin [`Rect`] and a DPI/backing
//! scale factor; `place_within` never touches a native API itself, which is
//! exactly what let these tests move here unchanged from the original
//! Windows-only `win/monitor.rs`.

/// A monitor's work area, in top-left-origin physical pixels. macOS reports
/// screen geometry bottom-left-origin (`NSScreen::visibleFrame`) — the Y
/// flip into this space happens in `platform::mac::monitor`, at the call
/// site, so this type and `place_within` stay OS-neutral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// The flip-then-clamp math, in physical pixels, already-scaled sizes, and
/// with no OS calls — pure enough to unit test directly.
///
/// - `Some(cursor)`: offset down-right from the cursor; flips to the
///   opposite side (left/above) of the cursor if the naive offset would run
///   past the monitor's work area, then clamps as a last resort — the
///   standard context-menu behavior.
/// - `None`: centered in the work area — used when there is no cursor
///   context (the tray's Show item, the delayed-unlock prompt).
pub fn place_within(work: Rect, cursor: Option<(i32, i32)>, w: i32, h: i32, offset: i32) -> (i32, i32) {
    let (x, y) = match cursor {
        Some((cursor_x, cursor_y)) => {
            let mut x = cursor_x + offset;
            let mut y = cursor_y + offset;
            if x + w > work.right {
                x = cursor_x - w - offset; // flip to the left of the cursor
            }
            if y + h > work.bottom {
                y = cursor_y - h - offset; // flip above the cursor
            }
            (x, y)
        }
        None => (
            work.left + (work.right - work.left - w) / 2,
            work.top + (work.bottom - work.top - h) / 2,
        ),
    };

    // Last resort: even after flipping, a monitor smaller than the popup
    // (or a cursor pinned right in a corner) could still leave it hanging
    // off an edge — clamp fully inside the work area.
    let x = x.clamp(work.left, (work.right - w).max(work.left));
    let y = y.clamp(work.top, (work.bottom - h).max(work.top));
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1920x1080 monitor at (0,0) with a 40px taskbar docked at the
    /// bottom — the work area excludes it, which is what `place_within`
    /// always receives.
    const WORK: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };
    const W: i32 = 340;
    const H: i32 = 220;
    const OFFSET: i32 = 8;

    #[test]
    fn offsets_down_right_when_it_fits() {
        let (x, y) = place_within(WORK, Some((500, 500)), W, H, OFFSET);
        assert_eq!((x, y), (508, 508));
    }

    #[test]
    fn flips_left_near_the_right_edge() {
        // cursor 100px from the right edge — offset+width would overrun it.
        let cursor = (WORK.right - 100, 500);
        let (x, _y) = place_within(WORK, Some(cursor), W, H, OFFSET);
        assert_eq!(x, cursor.0 - W - OFFSET, "should flip to the left of the cursor");
        assert!(x + W <= WORK.right, "must not run past the right edge");
    }

    #[test]
    fn flips_up_near_the_bottom_edge() {
        // cursor 100px from the bottom of the *work area* (above the
        // taskbar) — offset+height would overrun it.
        let cursor = (500, WORK.bottom - 100);
        let (_x, y) = place_within(WORK, Some(cursor), W, H, OFFSET);
        assert_eq!(y, cursor.1 - H - OFFSET, "should flip above the cursor");
        assert!(y + H <= WORK.bottom, "must not run under the taskbar");
    }

    #[test]
    fn flips_both_axes_in_the_bottom_right_corner() {
        let cursor = (WORK.right - 5, WORK.bottom - 5);
        let (x, y) = place_within(WORK, Some(cursor), W, H, OFFSET);
        assert!(x + W <= WORK.right);
        assert!(y + H <= WORK.bottom);
        assert!(x >= WORK.left);
        assert!(y >= WORK.top);
    }

    #[test]
    fn clamps_when_pinned_exactly_in_the_top_left_corner() {
        // Flipping left/up from (0,0) would go negative; the clamp must
        // catch what the flip alone can't.
        let (x, y) = place_within(WORK, Some((0, 0)), W, H, OFFSET);
        assert!(x >= WORK.left);
        assert!(y >= WORK.top);
    }

    #[test]
    fn clamps_without_panicking_when_popup_is_bigger_than_the_monitor() {
        let tiny_work = Rect {
            left: 0,
            top: 0,
            right: 200,
            bottom: 150,
        };
        let (x, y) = place_within(tiny_work, Some((100, 100)), W, H, OFFSET);
        // Can't satisfy "fully on screen" here — just must not panic, and
        // must not drift left/above the monitor entirely.
        assert!(x >= tiny_work.left);
        assert!(y >= tiny_work.top);
    }

    #[test]
    fn centers_on_the_work_area_when_there_is_no_cursor() {
        let (x, y) = place_within(WORK, None, W, H, OFFSET);
        assert_eq!(x, (WORK.right - W) / 2);
        assert_eq!(y, (WORK.bottom - H) / 2);
    }
}
