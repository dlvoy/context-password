//! The single inbound event type the UI thread drains every frame (see
//! `App::logic`). Tray and hotkey events funnel into this now; the `bw`
//! worker (M4) joins the same channel later so there is exactly one place
//! that decides what an event means.

use crate::win::focus::Target;

#[derive(Debug, Clone, Copy)]
pub enum Msg {
    Tray(TrayCmd),
    Hotkey(Target),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Quit,
}
