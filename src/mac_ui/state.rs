//! What the popup panel displays. The macOS counterpart of (Windows-only)
//! `controller::Content`, deliberately smaller: Settings and About are
//! separate `NSWindow`s here (see `crate::controller`'s module doc for
//! why Windows can't do that), and the master password never needs to be
//! accumulated char-by-char — a real `NSSecureTextField` owns its own
//! buffer natively, so `Prompting` only needs to carry the error text.

pub enum PopupContent {
    Prompting {
        error: Option<String>,
    },
    Unlocking,
    Locking,
    Syncing,
    ShowingList {
        selected: usize,
        /// An inline reason a requested field couldn't be delivered (no
        /// username, no TOTP configured) — cleared on the next selection
        /// change or delivery attempt, not a lingering banner.
        message: Option<String>,
    },
    /// Waiting on `bw get totp` for the item at `selected`. Escape returns
    /// to `ShowingList { selected, .. }` instead of hiding the whole popup.
    FetchingOtp {
        selected: usize,
    },
}

impl PopupContent {
    pub fn fresh_prompt() -> Self {
        Self::Prompting { error: None }
    }
}

/// Which field Enter/a click delivers, driven by which modifier is held —
/// the macOS counterpart of (Windows-only) `controller::DeliveryKind`,
/// without the egui `IconMode` coupling.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    Password,
    Username,
    Otp,
}

impl DeliveryKind {
    /// Shift wins over Option(Alt) if somehow both are held — same
    /// arbitrary-but-documented tie-break as the Windows version.
    pub fn from_flags(shift: bool, option: bool) -> Self {
        if shift {
            Self::Username
        } else if option {
            Self::Otp
        } else {
            Self::Password
        }
    }
}
