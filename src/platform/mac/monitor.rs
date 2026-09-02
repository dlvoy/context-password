//! Monitor-aware popup placement — stub. Real implementation is Phase 3 of
//! the port plan (`NSScreen`), including the bottom-left-to-top-left Y flip
//! at the boundary so `platform::place::place_within` (shared with
//! Windows) stays coordinate-system-agnostic.

#![allow(unused)]

use crate::platform::place;

pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub fn placement_for(_cursor: Option<(i32, i32)>, _w_pt: i32, _h_pt: i32) -> Placement {
    unimplemented!("macOS monitor placement lands in Phase 3 of the port plan")
}

#[allow(dead_code)]
fn monitor_metrics(_cursor: Option<(i32, i32)>) -> (place::Rect, f32) {
    unimplemented!("macOS monitor placement lands in Phase 3 of the port plan")
}
