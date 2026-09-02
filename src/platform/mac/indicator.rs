//! The autotype progress indicator — stub. Real implementation is Phase 3
//! of the port plan: a borderless, non-activating `NSPanel` with a custom
//! `NSView` porting `win::indicator`'s GDI drawing to `NSBezierPath`/
//! CoreGraphics 1:1 (see the port plan's table for the exact mapping).

#![allow(unused)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Typing,
    Done,
}

pub fn show(_kind: Kind, _x: i32, _y: i32, _size: i32) {
    unimplemented!("macOS indicator lands in Phase 3 of the port plan")
}

pub fn set_kind(_kind: Kind) {
    unimplemented!("macOS indicator lands in Phase 3 of the port plan")
}

pub fn hide() {
    unimplemented!("macOS indicator lands in Phase 3 of the port plan")
}
