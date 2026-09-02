//! Frontmost-app capture, activation, and restoration — stub. Real
//! implementation is Phase 3 of the port plan (`NSWorkspace`,
//! `NSRunningApplication`). Mirrors `win::focus`'s public surface
//! (`Target`'s accessors, `capture_target`, `activate_target`,
//! `cursor_pos`) so `src/mac_ui/` can be written against the same shape.

#![allow(unused)]

use super::super::BlockReason;

#[derive(Debug, Clone, Copy)]
pub struct Target {
    pid: i32,
    cursor: (i32, i32),
    blocked: Option<BlockReason>,
}

impl Target {
    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    pub fn blocked(&self) -> Option<BlockReason> {
        self.blocked
    }

    pub fn still_valid(&self) -> bool {
        unimplemented!("macOS focus tracking lands in Phase 3 of the port plan")
    }

    pub fn is_foreground(&self) -> bool {
        unimplemented!("macOS focus tracking lands in Phase 3 of the port plan")
    }
}

pub fn capture_target(_ctx: ()) -> Option<Target> {
    unimplemented!("macOS focus tracking lands in Phase 3 of the port plan")
}

pub fn cursor_pos() -> (i32, i32) {
    unimplemented!("macOS focus tracking lands in Phase 3 of the port plan")
}

pub fn activate_target(_target: &Target) {
    unimplemented!("macOS focus tracking lands in Phase 3 of the port plan")
}
