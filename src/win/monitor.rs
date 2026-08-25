//! Monitor-aware popup placement: which monitor, its DPI, its work area
//! (excludes the taskbar), and the flip-then-clamp algorithm that keeps the
//! popup fully on screen regardless of where the cursor is (plan §9,
//! resolves spike S3).
//!
//! `GetDpiForMonitor` is queried on the *target* monitor — never derived
//! from the popup window's own current scale factor, which may belong to a
//! different monitor than the one it's about to move to (plan F4). The
//! returned size is scaled to that DPI, so the popup is a consistent
//! *physical* size across monitors rather than a consistent pixel count
//! that visibly shrinks on a high-DPI screen relative to a 100% one.

use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

/// The DPI Windows treats as "100%" — every scale factor is relative to it.
const BASE_DPI: f32 = 96.0;
/// Default offset from the cursor, in 96-DPI-equivalent points, before any
/// per-monitor scaling is applied.
const CURSOR_OFFSET_PT: i32 = 8;

pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Computes a fully-on-screen placement for a `w_pt`×`h_pt` popup (sized in
/// 96-DPI-equivalent points — the same numbers used everywhere else in the
/// app, e.g. `POPUP_SIZE` in `app.rs`).
///
/// - `Some(cursor)`: offset down-right from the cursor; flips to the
///   opposite side (left/above) of the cursor if the naive offset would run
///   past the monitor's work area, then clamps as a last resort — the
///   standard context-menu behavior.
/// - `None`: centered on the primary monitor's work area — used when there
///   is no cursor context (the tray's Show item, the delayed-unlock
///   prompt).
pub fn placement_for(cursor: Option<(i32, i32)>, w_pt: i32, h_pt: i32) -> Placement {
    let (work, scale) = monitor_metrics(cursor);

    let w = (w_pt as f32 * scale).round() as i32;
    let h = (h_pt as f32 * scale).round() as i32;
    let offset = (CURSOR_OFFSET_PT as f32 * scale).round() as i32;

    let (x, y) = place_within(work, cursor, w, h, offset);
    Placement { x, y, w, h }
}

/// The live Win32 half: which monitor is under `cursor` (or the primary
/// monitor if `None`), its work area, and its DPI scale factor relative to
/// 96 DPI. Not unit-tested — it's a thin wrapper over three system calls
/// with an obvious fallback if any of them fail; `place_within` below is
/// where the actual placement logic lives and is tested.
fn monitor_metrics(cursor: Option<(i32, i32)>) -> (RECT, f32) {
    let (monitor_point, monitor_flag) = match cursor {
        Some((x, y)) => (POINT { x, y }, MONITOR_DEFAULTTONEAREST),
        // The point is ignored by Windows for this flag; a fixed origin is
        // just as valid as any other.
        None => (POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
    };

    unsafe {
        let monitor = MonitorFromPoint(monitor_point, monitor_flag);

        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(monitor, &mut info) != 0 {
            info.rcWork
        } else {
            // Should not happen for a handle Windows just handed us, but a
            // window a few thousand pixels off-screen is a poor failure
            // mode — fall back to a generous, always-safe guess.
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        };

        let mut dpi_x = BASE_DPI as u32;
        let mut dpi_y = BASE_DPI as u32;
        // Ignore the HRESULT: on failure the out-params are left at the
        // 96 (100%) default we initialized them to, which is a safe
        // fallback scale.
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        (work, dpi_x as f32 / BASE_DPI)
    }
}

/// The flip-then-clamp math, in physical pixels, already-scaled sizes, and
/// with no Win32 calls — pure enough to unit test directly.
fn place_within(
    work: RECT,
    cursor: Option<(i32, i32)>,
    w: i32,
    h: i32,
    offset: i32,
) -> (i32, i32) {
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
    /// bottom — `rcWork` excludes it, which is what `place_within` always
    /// receives.
    const WORK: RECT = RECT {
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
        let tiny_work = RECT {
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
