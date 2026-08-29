//! A small always-on-top, click-through, never-activating indicator shown
//! near the cursor during autotype delivery: a keyboard glyph while typing
//! is in flight, a green check once it has landed. A raw Win32 window
//! rather than a second egui viewport — the whole delivery sequence
//! (`app::DeliveryPhase`) runs with the root viewport hidden, and
//! `show_viewport_immediate` panics on eframe's glow backend while the root
//! is hidden (see `Content::Settings`'s doc in `app.rs`), so a child
//! viewport is a dead end here.
//!
//! `WS_EX_NOACTIVATE` keeps it from ever taking foreground (delivery polls
//! `focus::is_foreground` and would abort if it did), and `WM_NCHITTEST` ->
//! `HTTRANSPARENT` makes clicks pass through to whatever is underneath.
//! `SetWindowRgn` clips the window to a circle so there's no square backing
//! plate. The window is created once, lazily, and reused (shown/hidden)
//! for the process's lifetime rather than recreated per delivery.

use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateEllipticRgn, CreatePen, CreateSolidBrush, DeleteObject, Ellipse, EndPaint,
    GetStockObject, HDC, InvalidateRect, NULL_BRUSH, NULL_PEN, PAINTSTRUCT, PS_SOLID, Polyline,
    Rectangle, RoundRect, SelectObject, SetWindowRgn,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, GetClientRect, HTTRANSPARENT,
    HWND_TOPMOST, IDC_ARROW, LoadCursorW, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetWindowPos, ShowWindow, WM_NCHITTEST, WM_PAINT, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

/// What the indicator currently displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Typing,
    Done,
}

thread_local! {
    static WINDOW: RefCell<Option<HWND>> = const { RefCell::new(None) };
    static KIND: Cell<Kind> = const { Cell::new(Kind::Typing) };
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

/// Creates the (initially hidden, zero-sized) indicator window the first
/// time it's needed, and reuses the same `HWND` on every later call.
fn ensure_window() -> HWND {
    if let Some(hwnd) = WINDOW.with(|w| *w.borrow()) {
        return hwnd;
    }

    let class_name = wide("ContextPasswordIndicator");
    let hwnd = unsafe {
        let hinstance = GetModuleHandleW(ptr::null());
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        // A 0 return means the class is already registered (shouldn't
        // happen — this only runs once) or genuinely failed; either way
        // `CreateWindowExW` below is the real signal, so the atom itself
        // isn't checked.
        RegisterClassW(&class);

        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TRANSPARENT,
            class_name.as_ptr(),
            ptr::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        )
    };

    if hwnd.is_null() {
        eprintln!("indicator: failed to create the overlay window");
    }
    WINDOW.with(|w| *w.borrow_mut() = Some(hwnd));
    hwnd
}

/// Shows the indicator as a `size`×`size` circle at `(x, y)` (top-left,
/// physical pixels — same coordinate space `win::window_style::place`
/// takes), displaying `kind`.
pub fn show(kind: Kind, x: i32, y: i32, size: i32) {
    let hwnd = ensure_window();
    if hwnd.is_null() {
        return;
    }
    KIND.with(|k| k.set(kind));
    unsafe {
        // The system takes ownership of the region on a successful call —
        // it must not be freed here.
        let region = CreateEllipticRgn(0, 0, size, size);
        SetWindowRgn(hwnd, region, 0);
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, size, size, SWP_NOACTIVATE);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        InvalidateRect(hwnd, ptr::null(), 1);
    }
}

/// Switches what an already-shown indicator displays, in place — no
/// repositioning. Used for the typing-glyph-to-checkmark transition.
pub fn set_kind(kind: Kind) {
    KIND.with(|k| k.set(kind));
    if let Some(hwnd) = WINDOW.with(|w| *w.borrow())
        && !hwnd.is_null()
    {
        unsafe { InvalidateRect(hwnd, ptr::null(), 1) };
    }
}

pub fn hide() {
    if let Some(hwnd) = WINDOW.with(|w| *w.borrow())
        && !hwnd.is_null()
    {
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        // Routes every mouse message straight through to whatever real
        // window sits beneath the cursor, regardless of z-order or which
        // process owns it — the general-purpose click-through mechanism,
        // independent of `WS_EX_TRANSPARENT` (which only affects sibling
        // paint order, not hit-testing).
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rect: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rect);
        let kind = KIND.with(Cell::get);
        draw(hdc, rect, kind);
        EndPaint(hwnd, &ps);
    }
}

/// Hand-painted with GDI primitives rather than an icon asset, in the same
/// no-asset-pipeline spirit as `ui::popup::paint_icon`.
fn draw(hdc: HDC, rect: RECT, kind: Kind) {
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return;
    }

    let (bg, fg) = match kind {
        Kind::Typing => (rgb(51, 51, 55), rgb(235, 235, 235)),
        Kind::Done => (rgb(40, 167, 69), rgb(255, 255, 255)),
    };

    unsafe {
        let brush = CreateSolidBrush(bg);
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        let old_brush = SelectObject(hdc, brush);
        Ellipse(hdc, 0, 0, w, h);
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(brush);
    }

    let stroke = (w.min(h) as f32 * 0.09).max(1.5).round() as i32;
    unsafe {
        let pen = CreatePen(PS_SOLID, stroke, fg);
        let old_pen = SelectObject(hdc, pen);
        match kind {
            Kind::Typing => draw_keyboard(hdc, w, h, fg),
            Kind::Done => draw_check(hdc, w, h),
        }
        SelectObject(hdc, old_pen);
        DeleteObject(pen);
    }
}

/// A rounded-rectangle outline with three small filled "keys" inside —
/// simpler and clearer at 32px than a literal keyboard silhouette.
fn draw_keyboard(hdc: HDC, w: i32, h: i32, fg: COLORREF) {
    let margin_x = (w as f32 * 0.22).round() as i32;
    let margin_y = (h as f32 * 0.30).round() as i32;
    let left = margin_x;
    let top = margin_y;
    let right = w - margin_x;
    let bottom = h - margin_y;
    let corner = ((right - left).min(bottom - top) as f32 * 0.35).round() as i32;

    unsafe {
        let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        RoundRect(hdc, left, top, right, bottom, corner, corner);
        SelectObject(hdc, old_brush);
    }

    let inner_w = right - left;
    let inner_h = bottom - top;
    let key_w = (inner_w as f32 * 0.16).round() as i32;
    let key_h = (inner_h as f32 * 0.30).round() as i32;
    let gap = (inner_w as f32 * 0.10).round() as i32;
    let total = 3 * key_w + 2 * gap;
    let key_left = left + (inner_w - total) / 2;
    let key_top = top + (inner_h - key_h) / 2;

    unsafe {
        let brush = CreateSolidBrush(fg);
        let old_brush = SelectObject(hdc, brush);
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        for i in 0..3 {
            let x = key_left + i * (key_w + gap);
            Rectangle(hdc, x, key_top, x + key_w, key_top + key_h);
        }
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(brush);
    }
}

/// A two-segment checkmark, proportioned to the circle.
fn draw_check(hdc: HDC, w: i32, h: i32) {
    let points = [
        POINT {
            x: (w as f32 * 0.26) as i32,
            y: (h as f32 * 0.52) as i32,
        },
        POINT {
            x: (w as f32 * 0.43) as i32,
            y: (h as f32 * 0.68) as i32,
        },
        POINT {
            x: (w as f32 * 0.75) as i32,
            y: (h as f32 * 0.32) as i32,
        },
    ];
    unsafe {
        Polyline(hdc, points.as_ptr(), points.len() as i32);
    }
}
