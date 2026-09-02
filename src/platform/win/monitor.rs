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

use crate::platform::place::{self, Rect};

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

    let work_rect = Rect {
        left: work.left,
        top: work.top,
        right: work.right,
        bottom: work.bottom,
    };
    let (x, y) = place::place_within(work_rect, cursor, w, h, offset);
    Placement { x, y, w, h }
}

/// The live Win32 half: which monitor is under `cursor` (or the primary
/// monitor if `None`), its work area, and its DPI scale factor relative to
/// 96 DPI. Not unit-tested — it's a thin wrapper over three system calls
/// with an obvious fallback if any of them fail; `platform::place::place_within`
/// is where the actual placement logic lives and is tested (shared with
/// macOS's `monitor_metrics`, which does the equivalent bottom-left-to-
/// top-left Y flip before calling into the same function).
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

