//! The `eframe::App` implementation. The root viewport is the popup window
//! itself (see plan §F5 — it's the one that needs Win32 surgery, so it has
//! to be the root, with the config window as a child viewport once M7 adds
//! it).
//!
//! M2 added the hotkey-summoned placeholder popup: the three-frame show
//! sequence (plan §2), self-activation (§3), and Esc/blur dismissal with a
//! best-effort focus restore.
//!
//! M3 added delivery: the full `Delivering` phase machine (plan §4) — hide,
//! drain the hotkey's own modifiers, restore the target's foreground status
//! with verification (no blind sleep), then type via `SendInput`.
//!
//! M4 added the `bw` worker (headless): unlock → sync → list on a
//! background thread.
//!
//! M5 wired it all together into the real popup (plan §1/§9): `Prompting`
//! (a password field that never touches `egui::TextEdit`, per F6),
//! `Unlocking`, and `ShowingList` with keyboard/mouse selection, feeding
//! real passwords into the M3 delivery machine instead of a hardcoded
//! payload — the first fully working flow end to end. M6 added
//! monitor-aware flip-and-clamp placement.
//!
//! M7 adds the settings screen (`ui::config_window`) — `Content::Settings`,
//! a screen in the same popup window rather than a separate viewport (see
//! that variant's doc for why) — with a hotkey recorder and the autostart
//! toggle, both applied live (re-registering the hotkey without a
//! restart). It also adds the tray's Lock item: forgets the cached items
//! and the worker's session key, and tells `bw` to lock too.
//!
//! A later round adds: tray text/visibility reflecting vault state
//! (`VaultState`), visible progress while locking (`Content::Locking`),
//! username/OTP autotype alongside the password (`DeliveryKind`,
//! `Content::FetchingOtp`), a digit quick-select, a configurable
//! `max_visible_items` with a dynamically sized list popup, and
//! lock-on-exit with a timeout (`ExitState`).

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use zeroize::Zeroizing;

use crate::bw::model::Entry;
use crate::config::{Config, UnlockMode};
use crate::hotkey::Hotkey;
use crate::msg::{BwCmd, BwResult, Msg, TrayCmd};
use crate::secret::Secret;
use crate::ui::config_window;
use crate::ui::config_window::ConfigWindowState;
use crate::ui::popup::IconMode;
use crate::win::focus::{self, ActivationResult, Target};
use crate::{bw, tray, win};

const MODIFIER_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);
const FOREGROUND_VERIFY_TIMEOUT: Duration = Duration::from_millis(500);
/// How soon to ask for the next `logic()` call while a phase is still
/// waiting on something (a modifier release, foreground confirmation, the
/// settle delay). eframe throttles invisible-window repaints to no faster
/// than ~100ms regardless (see `INVISIBLE_WINDOW_REPAINT_INTERVAL` in
/// eframe's `native/run.rs`), so this is a lower bound, not the real
/// cadence — total delivery latency is a few hundred ms, not a few ms.
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const POPUP_SIZE: (i32, i32) = (340, 220);
// Hand-tuned to comfortably fit every row (hotkey recorder, three
// checkboxes, the max-visible-items stepper, and the footer) at the
// dialog's scaled-up font size (`config_window::FONT_SCALE`) with no
// clipping or crowding.
const SETTINGS_SIZE: (i32, i32) = (460, 420);
/// Bounds for the `ShowingList` popup's content-fitted width (see
/// `measure_list_width`) — never narrower than the other screens, and never
/// so wide that one absurdly long item name blows the popup up.
const LIST_MIN_WIDTH: f32 = POPUP_SIZE.0 as f32;
const LIST_MAX_WIDTH: f32 = 520.0;
/// Non-list chrome above/below the item rows in `ShowingList` (title,
/// separators, hint footer, spacing) — hand-tuned the same way `POPUP_SIZE`
/// itself was, revisit if the list ever looks cramped or has dead space at
/// the top/bottom.
const LIST_CHROME_HEIGHT: f32 = 100.0;
/// How long Quit waits for `bw lock` to finish when `lock_on_exit` is on,
/// before giving up and closing anyway — a hung or very slow `bw` must
/// never make quitting feel stuck.
const LOCK_ON_EXIT_TIMEOUT: Duration = Duration::from_secs(3);

/// The height of a `ShowingList` popup that shows exactly `max_visible`
/// rows, no more and no less — no dead space, no overflow.
fn list_popup_height(max_visible: usize) -> i32 {
    (LIST_CHROME_HEIGHT + max_visible as f32 * crate::ui::popup::ITEM_ROW_HEIGHT).round() as i32
}

/// The visible-row index a quick-select digit key was just pressed for
/// (`1`..`9` → 0..8, `0` → 9), if any — the keyboard counterpart of
/// `ui::popup::badge_for`'s numbering.
fn digit_pressed(ctx: &egui::Context) -> Option<usize> {
    use egui::Key;
    const DIGITS: [(Key, usize); 10] = [
        (Key::Num1, 0),
        (Key::Num2, 1),
        (Key::Num3, 2),
        (Key::Num4, 3),
        (Key::Num5, 4),
        (Key::Num6, 5),
        (Key::Num7, 6),
        (Key::Num8, 7),
        (Key::Num9, 8),
        (Key::Num0, 9),
    ];
    ctx.input(|i| {
        DIGITS
            .iter()
            .find(|(key, _)| i.key_pressed(*key))
            .map(|(_, idx)| *idx)
    })
}

/// The popup's show/hide state — window mechanics, independent of what's
/// displayed once shown (`Content`, below). `Placing`/`Showing`/`Activating`
/// are the three frames of the plan's show sequence (§2) — each one
/// advances on the next `logic()` call, chained via `request_repaint()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShowPhase {
    Hidden,
    Placing,
    Showing,
    Activating,
    Shown,
}

/// What the popup displays once `Shown` (plan §1/§9).
enum Content {
    Prompting {
        password: Zeroizing<String>,
        error: Option<String>,
        /// Whether the field is showing plaintext instead of bullets —
        /// still never an `egui::TextEdit` (see the module doc's F6 note),
        /// so revealing it doesn't reopen the leak that avoiding `TextEdit`
        /// was for; it's a plain painted label either way.
        revealed: bool,
    },
    Unlocking,
    /// Waiting on `bw lock` to finish — shown instead of leaving the popup
    /// blank or jumping straight to the prompt if it's opened (or already
    /// open) while a lock triggered from the tray is still in flight.
    Locking,
    ShowingList {
        selected: usize,
        /// An inline reason a requested field couldn't be delivered (no
        /// username, no TOTP configured) — cleared on the next selection
        /// change or delivery attempt, not a lingering banner.
        message: Option<String>,
    },
    /// Waiting on `bw get totp` for the item at `selected` (in the same
    /// cached list `ShowingList` reads from). Unlike `Unlocking`/`Locking`
    /// there's a natural "back" — Escape returns to `ShowingList { selected,
    /// .. }` instead of hiding the whole popup.
    FetchingOtp {
        selected: usize,
    },
    /// The settings screen (M7). A screen within the *same* popup window
    /// rather than a separate child viewport: `show_viewport_immediate`
    /// panics when called while the root viewport is hidden (which is
    /// exactly when the tray's Settings item opens it) — eframe's glow
    /// backend can't upgrade the `Weak` GL-context references it needs on
    /// that code path, so the render callback silently never runs. Folding
    /// Settings into the existing show/hide machinery sidesteps the bug
    /// entirely instead of working around eframe internals.
    Settings(config_window::ConfigWindowState),
}

/// Which field Enter/a digit delivers, driven by which modifier is held.
/// The render side (`ui::popup::IconMode`) mirrors this one-to-one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DeliveryKind {
    Password,
    Username,
    Otp,
}

impl DeliveryKind {
    /// Shift wins over Alt if somehow both are held — an arbitrary but
    /// documented tie-break, not a meaningful combination either way.
    fn from_modifiers(modifiers: egui::Modifiers) -> Self {
        if modifiers.shift {
            Self::Username
        } else if modifiers.alt {
            Self::Otp
        } else {
            Self::Password
        }
    }

    fn icon_mode(self) -> IconMode {
        match self {
            Self::Password => IconMode::Password,
            Self::Username => IconMode::Username,
            Self::Otp => IconMode::Otp,
        }
    }
}

/// Whether the vault currently has anything unlocked — drives the tray's
/// `Show`/`Unlock` label and whether `Lock` is present at all.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VaultState {
    Locked,
    Unlocked,
    /// `bw lock` has been sent but hasn't finished yet.
    Locking,
}

/// Quitting normally just closes the root viewport; `lock_on_exit` needs to
/// briefly *not* do that so a `bw lock` call has a chance to finish first
/// (§7 of the plan) — tracked here rather than a bare bool so the "already
/// asked once" state survives across frames without re-deriving it from
/// `vault_state`, which also changes for unrelated reasons (the tray's own
/// Lock item).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ExitState {
    NotExiting,
    WaitingForLock { deadline: Instant },
}

struct PopupState {
    phase: ShowPhase,
    content: Content,
    /// `None` when the popup was opened with no cursor context (tray
    /// "Show", or the delayed-unlock prompt) — placement falls back to
    /// centering on the primary monitor, and there's nothing to restore
    /// focus to on dismiss.
    target: Option<Target>,
    /// Dismiss-on-blur latch (plan §2): the window is visible but not yet
    /// foreground during `Showing`/`Activating`, so blur can't be trusted
    /// as "the user clicked away" until we've seen it focused at least once.
    seen_focus: bool,
}

impl PopupState {
    fn fresh_prompt() -> Content {
        Content::Prompting {
            password: Zeroizing::new(String::with_capacity(256)),
            error: None,
            revealed: false,
        }
    }
}

/// The phases of plan §4's typing delivery sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryPhase {
    HidePopup,
    DrainModifiers,
    Activate,
    VerifyForeground,
    Settle,
}

struct Delivery {
    target: Target,
    secret: Secret,
    phase: DeliveryPhase,
    /// Meaning depends on the phase: a drop-dead time for `DrainModifiers`
    /// and `VerifyForeground` (past which they give up), or the time to
    /// wait *until* for `Settle`.
    deadline: Instant,
}

/// Counts which `activate_self` branch won, across repeated presses — the
/// instrumentation M2 asks for to resolve spike S1 (does the plain
/// `SetForegroundWindow` call reliably work, or is `AttachThreadInput`
/// needed often in practice).
#[derive(Debug, Default, Clone, Copy)]
struct ActivationStats {
    direct: u32,
    attached: u32,
    failed: u32,
}

impl ActivationStats {
    fn record(&mut self, result: ActivationResult) {
        match result {
            ActivationResult::Direct => self.direct += 1,
            ActivationResult::Attached => self.attached += 1,
            ActivationResult::Failed => self.failed += 1,
        }
    }
}

pub struct App {
    // Order matters for Drop: dropping the tray icon removes it from the
    // shell; nothing here depends on drop order otherwise, but keep it
    // explicit rather than relying on field order being incidental.
    tray: tray::TrayHandles,
    hotkey: Hotkey,
    hwnd: windows_sys::Win32::Foundation::HWND,
    cfg: Config,
    rx: Receiver<Msg>,
    bw_cmd_tx: Sender<BwCmd>,
    hidden_after_startup: bool,
    popup: PopupState,
    delivery: Option<Delivery>,
    stats: ActivationStats,
    /// The cache: populated once by a successful unlock, kept for the
    /// process's lifetime (the settled decision — see the plan's Context
    /// section). `None` means "never unlocked yet" and is what routes the
    /// popup to `Prompting` instead of `ShowingList`.
    cached_entries: Option<(Vec<Entry>, usize)>,
    /// Set while the Settings screen has temporarily unregistered the live
    /// hotkey so it can be re-captured (see `Hotkey::suspend`) — tracked
    /// here rather than derived from `ConfigWindowState.recording` so the
    /// registration is guaranteed to be restored even if the screen closes
    /// mid-recording.
    hotkey_suspended: bool,
    vault_state: VaultState,
    exit_state: ExitState,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        hotkey: Hotkey,
        cfg: Config,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let hwnd = win::window_style::hwnd_of(cc)
            .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                "failed to obtain the native window handle".into()
            })?;
        win::window_style::make_tool_window(hwnd);

        // `ThemePreference::System` is egui's default already, so this is
        // just making the intent explicit — egui reads the OS theme from
        // winit (`RawInput::system_theme`) and switches `Visuals::dark()`/
        // `light()` automatically. The part that *isn't* automatic is
        // `clear_color` below.
        cc.egui_ctx.set_theme(egui::ThemePreference::System);

        // Cached once: never changes for the process's lifetime, and every
        // hotkey press needs it to evaluate the target's elevation (plan §5).
        let our_integrity_rid = win::integrity::our_integrity_rid();

        let (tx, rx) = mpsc::channel();

        // All three handlers below run synchronously on the UI thread —
        // for the hotkey that's exactly what plan F1 relies on: WM_HOTKEY
        // fires here, inside DispatchMessage, before any window of ours has
        // shown, so this is the only correct place to capture the
        // foreground window and cursor position.
        let menu_tx = tx.clone();
        let menu_ctx = cc.egui_ctx.clone();
        tray_icon::menu::MenuEvent::set_event_handler(Some(
            move |event: tray_icon::menu::MenuEvent| {
                let cmd = if event.id() == tray::SHOW_ID {
                    TrayCmd::Show
                } else if event.id() == tray::LOCK_ID {
                    TrayCmd::Lock
                } else if event.id() == tray::SETTINGS_ID {
                    TrayCmd::Settings
                } else if event.id() == tray::QUIT_ID {
                    TrayCmd::Quit
                } else {
                    return;
                };
                let _ = menu_tx.send(Msg::Tray(cmd));
                menu_ctx.request_repaint();
            },
        ));

        let hotkey_tx = tx.clone();
        let hotkey_ctx = cc.egui_ctx.clone();
        // A handle, not a copied `u32`: `Hotkey::set` (M7's live
        // re-registration) changes the id underneath this 'static closure,
        // which has no other way to learn about it.
        let hotkey_id = hotkey.id_handle();
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |event: global_hotkey::GlobalHotKeyEvent| {
                if event.id() != hotkey_id.load(Ordering::Relaxed)
                    || event.state() != global_hotkey::HotKeyState::Pressed
                {
                    return;
                }
                if let Some(target) = focus::capture_target(our_integrity_rid) {
                    let _ = hotkey_tx.send(Msg::Hotkey(target));
                    hotkey_ctx.request_repaint();
                }
            },
        ));

        if cfg.unlock_mode == UnlockMode::Delayed {
            let delay_tx = tx.clone();
            let delay_ctx = cc.egui_ctx.clone();
            let delay = Duration::from_secs(u64::from(cfg.unlock_delay_secs));
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let _ = delay_tx.send(Msg::ShowPopup);
                delay_ctx.request_repaint();
            });
        }

        let mut tray = tray::build()?;
        // Locked at startup, so the tray should say "Unlock" and hide
        // "Lock" from the very first frame, not just after the first state
        // change.
        tray.set_show_label(false);
        tray.set_lock_visible(false);

        let bw_cmd_tx = bw::spawn(
            cfg.bw_path.clone(),
            cfg.uri_prefix.clone(),
            tx.clone(),
            cc.egui_ctx.clone(),
        );

        Ok(Self {
            tray,
            hotkey,
            hwnd,
            cfg,
            rx,
            bw_cmd_tx,
            hidden_after_startup: false,
            popup: PopupState {
                phase: ShowPhase::Hidden,
                content: PopupState::fresh_prompt(),
                target: None,
                seen_focus: false,
            },
            delivery: None,
            stats: ActivationStats::default(),
            cached_entries: None,
            hotkey_suspended: false,
            vault_state: VaultState::Locked,
            exit_state: ExitState::NotExiting,
        })
    }

    /// Begins showing the popup for `target` (`None` for the tray/delayed
    /// path, with no cursor context). Content is decided once, here, from
    /// whichever cache state currently holds — not re-decided every frame.
    fn open_popup(&mut self, target: Option<Target>) {
        if self.popup.phase != ShowPhase::Hidden || self.delivery.is_some() {
            // Never interrupt a popup that's already showing something —
            // Settings, an in-flight OTP fetch, the master-password prompt
            // mid-type, all of it — regardless of what asked for a new one
            // (hotkey, tray Show, or the delayed-unlock timer firing while
            // the user is already looking at the popup it summoned). Nor a
            // delivery already in flight, which hides the popup itself
            // (phase is `Hidden` there) but is just as much "already doing
            // something."
            return;
        }
        self.popup.target = target;
        self.popup.content = match (&self.cached_entries, &self.popup.content) {
            (Some(_), _) => Content::ShowingList { selected: 0, message: None },
            // An unlock (or lock) submitted before the popup was last
            // dismissed may still be in flight on the worker thread —
            // resume showing the spinner instead of discarding that state
            // (`handle_bw_result` still updates this content field even
            // while the popup is hidden).
            (None, Content::Unlocking) => Content::Unlocking,
            (None, Content::Locking) => Content::Locking,
            (None, _) => PopupState::fresh_prompt(),
        };
        self.popup.phase = ShowPhase::Placing;
    }

    /// Keeps the tray's `Show`/`Unlock` label and `Lock` item's presence in
    /// sync with `vault_state` — call this instead of assigning
    /// `self.vault_state` directly, so the two can never drift apart.
    fn set_vault_state(&mut self, state: VaultState) {
        self.vault_state = state;
        self.tray.set_show_label(state == VaultState::Unlocked);
        self.tray.set_lock_visible(state == VaultState::Unlocked);
    }

    fn hide_popup(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        if let Some(target) = &self.popup.target {
            focus::activate_target(target);
        }
        self.popup.phase = ShowPhase::Hidden;
        self.popup.seen_focus = false;
        // Zeroize a half-typed password immediately on dismiss rather than
        // waiting for the next `open_popup` to overwrite (and so drop) it —
        // no reason for it to sit in memory for however long the popup
        // happens to stay closed (plan §8's zeroize audit, M8). Settings
        // must be reset here too: `open_popup` refuses to run at all while
        // `self.popup.content` is `Content::Settings` (so a stray hotkey
        // press can't clobber an in-progress edit) — leaving it as
        // `Settings` after hiding would permanently lock the popup out of
        // ever reopening as the prompt/list again.
        if matches!(self.popup.content, Content::Prompting { .. } | Content::Settings(_)) {
            self.popup.content = PopupState::fresh_prompt();
        }
    }

    /// Forgets everything the app currently has unlocked: the cached items
    /// (and their passwords, zeroized on drop), and — via the worker — its
    /// own session key and the CLI's, best-effort. Shows visible progress
    /// (`Content::Locking`) unconditionally rather than only when the popup
    /// happens to already be showing the list — pressing the hotkey while a
    /// lock is in flight should reveal that progress, not the empty prompt
    /// or nothing at all.
    fn handle_lock(&mut self) {
        eprintln!("lock: clearing cached items and locking the vault");
        self.cached_entries = None;
        if self.hotkey_suspended {
            // Settings might be mid-recording when a global Lock action
            // interrupts it — don't leave the hotkey unregistered with
            // nothing left to resume it.
            self.hotkey.resume();
            self.hotkey_suspended = false;
        }
        self.popup.content = Content::Locking;
        self.set_vault_state(VaultState::Locking);
        let _ = self.bw_cmd_tx.send(BwCmd::Lock);
    }

    /// The only place `bw` worker results reach stderr — names and ORDs
    /// only, per `Entry::log_line`, never a password (and only at all when
    /// `debug_log` is on — plan §8's log review, M8) — and where they feed
    /// back into the popup if it's waiting on them.
    fn handle_bw_result(&mut self, result: BwResult) {
        match result {
            BwResult::Items { entries, dropped } => {
                if self.cfg.debug_log {
                    eprintln!(
                        "bw: unlocked, {} item(s) matched, {dropped} dropped",
                        entries.len()
                    );
                    for entry in &entries {
                        eprintln!("  {}", entry.log_line());
                    }
                }
                self.cached_entries = Some((entries, dropped));
                self.set_vault_state(VaultState::Unlocked);
                if matches!(self.popup.content, Content::Unlocking) {
                    self.popup.content = Content::ShowingList { selected: 0, message: None };
                    if self.popup.phase == ShowPhase::Shown {
                        // The window is already placed/sized for the
                        // smaller Prompting/Unlocking screen — re-enter
                        // Placing so it picks up the list's own (dynamic)
                        // width and height instead of keeping whatever size
                        // it had before this switch. When the popup was
                        // hidden instead, no fix-up is needed: `open_popup`
                        // always re-places from scratch next time.
                        self.popup.phase = ShowPhase::Placing;
                    }
                }
            }
            BwResult::Failed { stage, message } => {
                eprintln!("bw: failed at {stage}: {message}");
                match &self.popup.content {
                    Content::Unlocking => {
                        self.popup.content = Content::Prompting {
                            password: Zeroizing::new(String::with_capacity(256)),
                            error: Some(format!("{stage}: {message}")),
                            revealed: false,
                        };
                    }
                    Content::FetchingOtp { selected } => {
                        let selected = *selected;
                        self.popup.content = Content::ShowingList {
                            selected,
                            message: Some(format!("{stage}: {message}")),
                        };
                    }
                    _ => {}
                }
            }
            BwResult::Locked => {
                if self.cfg.debug_log {
                    eprintln!("bw: lock finished");
                }
                self.set_vault_state(VaultState::Locked);
                if matches!(self.popup.content, Content::Locking) {
                    self.popup.content = PopupState::fresh_prompt();
                }
            }
            BwResult::Totp(code) => {
                let Content::FetchingOtp { selected } = self.popup.content else {
                    return; // Stale — content already moved on.
                };
                let Some(target) = self.popup.target else {
                    self.popup.content = Content::ShowingList {
                        selected,
                        message: Some("no delivery target".to_string()),
                    };
                    return;
                };
                let log_line = self
                    .cached_entries
                    .as_ref()
                    .and_then(|(entries, _)| entries.get(selected))
                    .map(Entry::log_line)
                    .unwrap_or_default();
                // `begin_delivery` itself never touches `popup.content` (the
                // Password/Username callers already leave it as
                // `ShowingList`) — reset it here first, or it would stay
                // `FetchingOtp` forever, which `open_popup` treats the same
                // as `Settings`: refuses to ever reopen the popup again.
                self.popup.content = Content::ShowingList { selected, message: None };
                self.begin_delivery(target, code, &log_line);
            }
        }
    }

    /// Advances the typing delivery sequence by one step, if one is in
    /// flight. Never blocks: each phase either finishes immediately or asks
    /// for another `logic()` call via `request_repaint_after` and returns.
    fn advance_delivery(&mut self, ctx: &egui::Context) {
        let Some(delivery) = self.delivery.as_mut() else {
            return;
        };
        let mut done = false;

        match delivery.phase {
            DeliveryPhase::HidePopup => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                delivery.phase = DeliveryPhase::DrainModifiers;
                delivery.deadline = Instant::now() + MODIFIER_DRAIN_TIMEOUT;
            }
            DeliveryPhase::DrainModifiers => {
                // The user is still physically holding the hotkey's
                // modifiers (and just pressed Enter) when delivery starts;
                // typing before they're up would have the target see
                // WM_CHAR with a modifier held and misinterpret it.
                if win::typing::modifiers_up() || Instant::now() >= delivery.deadline {
                    delivery.phase = DeliveryPhase::Activate;
                }
            }
            DeliveryPhase::Activate => {
                if !focus::is_window(delivery.target.hwnd) {
                    eprintln!("delivery aborted: target window no longer exists");
                    done = true;
                } else {
                    focus::activate_target(&delivery.target);
                    delivery.phase = DeliveryPhase::VerifyForeground;
                    delivery.deadline = Instant::now() + FOREGROUND_VERIFY_TIMEOUT;
                }
            }
            DeliveryPhase::VerifyForeground => {
                // Polled, not a blind sleep: typing into whatever happens
                // to be focused if activation silently failed would be the
                // worst possible outcome here.
                if focus::is_foreground(delivery.target.hwnd) {
                    delivery.phase = DeliveryPhase::Settle;
                    delivery.deadline =
                        Instant::now() + Duration::from_millis(u64::from(self.cfg.type_settle_ms));
                } else if Instant::now() >= delivery.deadline {
                    eprintln!("delivery aborted: could not restore foreground to target");
                    done = true;
                }
            }
            DeliveryPhase::Settle => {
                if Instant::now() >= delivery.deadline {
                    if delivery.target.elevated_beyond_us {
                        eprintln!(
                            "delivery aborted: target runs elevated; SendInput would be \
                             silently blocked by UIPI"
                        );
                    } else {
                        let skipped = win::typing::send_unicode(delivery.secret.expose());
                        eprintln!(
                            "delivery: typed the selected password ({skipped} code unit(s) skipped)"
                        );
                    }
                    done = true;
                }
            }
        }

        if done {
            self.delivery = None;
        } else {
            ctx.request_repaint_after(POLL_INTERVAL);
        }
    }

    /// Handles input and state transitions for the `Shown` phase — dispatch
    /// only; actual mutation happens after the match to avoid borrowing
    /// `self.popup.content` and `self` simultaneously.
    fn update_shown(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        if focused {
            self.popup.seen_focus = true;
        }
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let blurred = self.popup.seen_focus && !focused;

        enum Action {
            None,
            Hide,
            Unlock(Secret),
            Deliver(DeliveryKind),
            BackToList(usize),
        }
        let mut action = Action::None;

        match &mut self.popup.content {
            Content::Prompting { password, error, .. } => {
                ctx.input(|i| {
                    for event in &i.events {
                        match event {
                            egui::Event::Text(s) => {
                                for ch in s.chars() {
                                    if !ch.is_control() {
                                        password.push(ch);
                                    }
                                }
                            }
                            egui::Event::Key {
                                key: egui::Key::Backspace,
                                pressed: true,
                                ..
                            } => {
                                password.pop();
                            }
                            _ => {}
                        }
                    }
                });
                if enter && !password.is_empty() {
                    *error = None;
                    action = Action::Unlock(Secret::new(password.to_string()));
                } else if escape || blurred {
                    action = Action::Hide;
                }
            }
            Content::Unlocking | Content::Locking => {
                // Dismiss-on-blur is suspended here (plan §1): an 8-15s
                // cold unlock (or a lock) that vanishes on a stray click
                // would be maddening. Escape still works — explicit intent.
                if escape {
                    action = Action::Hide;
                }
            }
            Content::ShowingList { selected, message } => {
                let count = self
                    .cached_entries
                    .as_ref()
                    .map_or(0, |(e, _)| e.len().min(self.cfg.effective_max_visible()));
                let mut moved = false;
                // A digit is a quick-select *and* deliver in one press (like
                // clicking the row then pressing Enter) — distinct from
                // Arrow/Home/End, which only move the selection.
                let mut digit_selected = false;
                if count > 0 {
                    if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                        *selected = (*selected + 1) % count;
                        moved = true;
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        *selected = if *selected == 0 {
                            count - 1
                        } else {
                            *selected - 1
                        };
                        moved = true;
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::Home)) {
                        *selected = 0;
                        moved = true;
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::End)) {
                        *selected = count - 1;
                        moved = true;
                    }
                    if let Some(i) = digit_pressed(ctx).filter(|&i| i < count) {
                        *selected = i;
                        moved = true;
                        digit_selected = true;
                    }
                }
                if moved {
                    *message = None;
                }
                if (enter || digit_selected) && count > 0 {
                    let kind = DeliveryKind::from_modifiers(ctx.input(|i| i.modifiers));
                    action = Action::Deliver(kind);
                } else if escape || blurred {
                    action = Action::Hide;
                }
            }
            Content::FetchingOtp { selected } => {
                // There's a natural "back" here, unlike Unlocking/Locking —
                // return to the list rather than hiding the whole popup.
                if escape {
                    action = Action::BackToList(*selected);
                }
            }
            // Handled entirely in `ui()` instead: the settings screen needs
            // a `Ui` to draw and to read its own input (button clicks, the
            // hotkey recorder's key capture), which `update_shown` — called
            // from `logic()` — doesn't have.
            Content::Settings(_) => {}
        }

        match action {
            Action::None => {}
            Action::Hide => self.hide_popup(ctx),
            Action::Unlock(secret) => {
                let _ = self.bw_cmd_tx.send(BwCmd::Unlock(secret));
                self.popup.content = Content::Unlocking;
            }
            Action::Deliver(kind) => self.start_delivery(kind),
            Action::BackToList(selected) => {
                self.popup.content = Content::ShowingList { selected, message: None };
            }
        }
    }

    /// Dispatches Enter/a digit for the currently selected item.
    /// `Password`/`Username` already have their secret on hand (in the
    /// cache) and go straight to `begin_delivery`; `Otp` doesn't, and goes
    /// through `Content::FetchingOtp` and a worker round trip instead.
    fn start_delivery(&mut self, kind: DeliveryKind) {
        let Content::ShowingList { selected, .. } = &self.popup.content else {
            return;
        };
        let selected = *selected;
        let Some((entries, _)) = &self.cached_entries else {
            return;
        };
        let Some(entry) = entries.get(selected) else {
            return;
        };
        let Some(target) = self.popup.target else {
            return;
        };

        match kind {
            DeliveryKind::Password => {
                let secret = entry.password.clone_secret();
                let log_line = entry.log_line();
                self.begin_delivery(target, secret, &log_line);
            }
            DeliveryKind::Username => {
                let Some(username) = entry.username.clone() else {
                    self.set_list_message(selected, "This item has no username.".to_string());
                    return;
                };
                let log_line = entry.log_line();
                self.begin_delivery(target, Secret::new(username), &log_line);
            }
            DeliveryKind::Otp => {
                if !entry.has_totp {
                    self.set_list_message(selected, "This item has no TOTP configured.".to_string());
                    return;
                }
                let item_id = entry.id.clone();
                self.popup.content = Content::FetchingOtp { selected };
                let _ = self.bw_cmd_tx.send(BwCmd::GetTotp(item_id));
            }
        }
    }

    /// Replaces the popup's content with `ShowingList` at `selected`,
    /// carrying an inline error — used when a requested field (username,
    /// OTP) can't be delivered, per the settled decision to say so rather
    /// than silently falling back to the password.
    fn set_list_message(&mut self, selected: usize, message: String) {
        self.popup.content = Content::ShowingList { selected, message: Some(message) };
    }

    /// The common tail of every delivery kind once its secret is in hand:
    /// hides the popup and hands off to the frame-driven `Delivering`
    /// machine (plan §4). `item_log_line` is only ever printed when
    /// `debug_log` is on (plan §8).
    fn begin_delivery(&mut self, target: Target, secret: Secret, item_log_line: &str) {
        eprintln!(
            "delivery: starting for target hwnd={} (elevated_beyond_us={})",
            target.hwnd, target.elevated_beyond_us
        );
        if self.cfg.debug_log {
            eprintln!("delivery: item {item_log_line}");
        }
        self.delivery = Some(Delivery {
            target,
            secret,
            phase: DeliveryPhase::HidePopup,
            deadline: Instant::now(),
        });
        self.popup.phase = ShowPhase::Hidden;
        self.popup.seen_focus = false;
    }

    /// Opens the settings screen in the same popup window (see
    /// `Content::Settings`'s doc for why not a separate viewport), seeded
    /// from the live config and the autostart registry's actual current
    /// state — not the config file's belief about it, since the user may
    /// have removed it via Task Manager's Startup tab since we last wrote
    /// it. No cursor context, so it centers on the primary monitor, same
    /// as the tray's Show item.
    fn open_settings(&mut self) {
        if self.delivery.is_some() {
            return;
        }
        self.popup.target = None;
        self.popup.content = Content::Settings(ConfigWindowState {
            hotkey_spec: self.hotkey.spec(),
            recording: false,
            autostart: win::autostart::is_enabled(),
            lock_on_exit: self.cfg.lock_on_exit,
            auto_unlock: self.cfg.unlock_mode == UnlockMode::Delayed,
            // Clamped, not the raw field: a config saved before this
            // setting existed (or hand-edited) could carry a value outside
            // the widget's 3..=10 range, which must never be what the
            // widget starts at.
            max_visible_items: self.cfg.effective_max_visible() as u32,
            message: None,
            held: config_window::HeldMods::default(),
        });
        self.popup.phase = ShowPhase::Placing;
    }

    /// Applies whichever of the hotkey/autostart actually changed, saves
    /// the config, and reports the first failure (if any) rather than
    /// silently discarding it — but still applies whatever *did* succeed
    /// rather than requiring all-or-nothing, since a failed autostart
    /// toggle is no reason to also refuse a valid hotkey change.
    fn try_apply_config_window(&mut self, state: &ConfigWindowState) -> Result<(), String> {
        let mut error = None;

        if state.hotkey_spec != self.cfg.hotkey {
            match self.hotkey.set(&state.hotkey_spec) {
                Ok(()) => {
                    eprintln!("settings: hotkey changed to {}", state.hotkey_spec);
                    self.cfg.hotkey.clone_from(&state.hotkey_spec);
                }
                Err(e) => {
                    error = Some(format!("hotkey: {e}"));
                }
            }
        }

        if state.autostart != self.cfg.autostart {
            match win::autostart::set_enabled(state.autostart) {
                Ok(()) => {
                    eprintln!(
                        "settings: autostart {}",
                        if state.autostart { "enabled" } else { "disabled" }
                    );
                    self.cfg.autostart = state.autostart;
                }
                Err(e) => {
                    error.get_or_insert(format!("autostart: {e}"));
                }
            }
        }

        // Neither of these can fail — just record and save. Takes effect
        // next launch only: the delayed-unlock timer (if any) was already
        // spawned in `App::new` for this session and has no way to learn
        // the setting changed underneath it.
        let new_unlock_mode =
            if state.auto_unlock { UnlockMode::Delayed } else { UnlockMode::Lazy };
        if new_unlock_mode != self.cfg.unlock_mode {
            eprintln!("settings: auto-unlock at start {}", state.auto_unlock);
            self.cfg.unlock_mode = new_unlock_mode;
        }
        if state.lock_on_exit != self.cfg.lock_on_exit {
            eprintln!(
                "settings: lock on exit {}",
                if state.lock_on_exit { "enabled" } else { "disabled" }
            );
            self.cfg.lock_on_exit = state.lock_on_exit;
        }
        let clamped_max_visible = state
            .max_visible_items
            .clamp(crate::config::MIN_VISIBLE_ITEMS, crate::config::MAX_VISIBLE_ITEMS);
        if clamped_max_visible != self.cfg.max_visible_items {
            eprintln!("settings: max visible items changed to {clamped_max_visible}");
            self.cfg.max_visible_items = clamped_max_visible;
        }

        if let Err(e) = self.cfg.save() {
            error.get_or_insert(format!("saving config: {e}"));
        }

        error.map_or(Ok(()), Err)
    }
}

impl eframe::App for App {
    /// eframe's default `clear_color` is a hardcoded near-black regardless
    /// of theme (`Color32::from_rgba_unmultiplied(12, 12, 12, 180)`) — on a
    /// light OS theme, egui correctly switches `Visuals` to light (dark
    /// text) but the window background stayed dark, so text and the
    /// spinner (which paints with `strong_text_color()`) rendered
    /// dark-on-near-black and were effectively invisible. Deriving the
    /// clear color from the active visuals keeps background and foreground
    /// coordinated in both themes.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.window_fill().to_normalized_gamma_f32()
    }

    /// Runs every frame regardless of whether the root window is visible —
    /// unlike `ui`, which eframe skips entirely while hidden. This is where
    /// all inbound events (tray, hotkey, `bw` worker) get drained, and
    /// where the show/hide and delivery phase machines advance.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.hidden_after_startup {
            self.hidden_after_startup = true;
            // eframe's glow backend always reveals the root window after its
            // first rendered frame, regardless of the ViewportBuilder's
            // `with_visible(false)` — see M1's finding in window_style.rs.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Tray(TrayCmd::Quit) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Msg::Tray(TrayCmd::Show) | Msg::ShowPopup => {
                    self.open_popup(None);
                }
                Msg::Tray(TrayCmd::Lock) => self.handle_lock(),
                Msg::Tray(TrayCmd::Settings) => self.open_settings(),
                Msg::Hotkey(target) => self.open_popup(Some(target)),
                Msg::Bw(result) => self.handle_bw_result(result),
            }
        }

        // `close_requested()` reflects any `ViewportCommand::Close` sent
        // for this viewport, from us (the Quit handler above) or the OS
        // alike — both funnel into the same `ViewportEvent::Close` egui
        // tracks (confirmed in egui-0.36.1's `viewport_info.rs`). Checked
        // every frame regardless of visibility, since this must work even
        // while the popup is hidden, which is this app's normal state.
        match self.exit_state {
            ExitState::NotExiting => {
                let close_requested = ctx.input(|i| i.viewport().close_requested());
                if close_requested && self.cfg.lock_on_exit && self.vault_state == VaultState::Unlocked {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.handle_lock();
                    self.exit_state = ExitState::WaitingForLock {
                        deadline: Instant::now() + LOCK_ON_EXIT_TIMEOUT,
                    };
                    ctx.request_repaint_after(POLL_INTERVAL);
                }
                // Otherwise: nothing to do — either there's no close to
                // react to, or lock-on-exit doesn't apply, and the close
                // already in flight is left to complete on its own.
            }
            ExitState::WaitingForLock { deadline } => {
                if self.vault_state != VaultState::Locking || Instant::now() >= deadline {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    ctx.request_repaint_after(POLL_INTERVAL);
                }
            }
        }

        match self.popup.phase {
            ShowPhase::Hidden => {}
            ShowPhase::Placing => {
                let (w_pt, h_pt) = match &self.popup.content {
                    // Needs more room than the item list for the hotkey
                    // recorder, checkbox, and buttons.
                    Content::Settings(_) => SETTINGS_SIZE,
                    Content::ShowingList { .. } => {
                        let max_visible = self.cfg.effective_max_visible();
                        let width = self.cached_entries.as_ref().map_or(POPUP_SIZE.0, |(e, _)| {
                            let visible = &e[..e.len().min(max_visible)];
                            crate::ui::popup::measure_list_width(ctx, visible)
                                .clamp(LIST_MIN_WIDTH, LIST_MAX_WIDTH)
                                .round() as i32
                        });
                        (width, list_popup_height(max_visible))
                    }
                    _ => POPUP_SIZE,
                };
                let cursor = self.popup.target.map(|t| t.cursor);
                let placement = win::monitor::placement_for(cursor, w_pt, h_pt);
                // Reapply defensively — M1 found this doesn't reliably
                // survive a visibility transition (see window_style.rs).
                win::window_style::make_tool_window(self.hwnd);
                win::window_style::place(
                    self.hwnd,
                    placement.x,
                    placement.y,
                    placement.w,
                    placement.h,
                );
                self.popup.phase = ShowPhase::Showing;
                ctx.request_repaint();
            }
            ShowPhase::Showing => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                self.popup.phase = ShowPhase::Activating;
                ctx.request_repaint();
            }
            ShowPhase::Activating => {
                let result = focus::activate_self(self.hwnd);
                self.stats.record(result);
                eprintln!("activate_self -> {result:?}");
                self.popup.seen_focus = false;
                self.popup.phase = ShowPhase::Shown;
                ctx.request_repaint();
            }
            ShowPhase::Shown => self.update_shown(ctx),
        }

        self.advance_delivery(ctx);
    }

    /// Only ever runs while `Shown` — eframe skips `ui` entirely for a
    /// hidden window, and the earlier phases have nothing to draw yet.
    ///
    /// `Content::Settings` is the one variant here that both draws *and*
    /// decides on an action (Save/Cancel), unlike the others where
    /// `update_shown` (called from `logic()`) already decided everything
    /// and this function only draws — it needs a `Ui` to render itself and
    /// to read its own input (the hotkey recorder's key capture), and
    /// `update_shown` doesn't have one. Any resulting action is recorded
    /// and applied *after* the match, once the borrow of `self.popup.content`
    /// the match holds has ended, the same pattern `advance_delivery` and
    /// `update_shown` use.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.popup.phase != ShowPhase::Shown {
            return;
        }

        enum PostAction {
            Hide,
            Save(ConfigWindowState),
            Deliver(DeliveryKind),
        }
        let mut post_action = None;

        match &mut self.popup.content {
            Content::Prompting { password, error, revealed } => {
                if crate::ui::popup::prompting(ui, password, *revealed, error.as_deref()) {
                    *revealed = !*revealed;
                }
            }
            Content::Unlocking => crate::ui::popup::busy(ui, "Unlocking vault…"),
            Content::Locking => crate::ui::popup::busy(ui, "Locking vault…"),
            Content::FetchingOtp { .. } => crate::ui::popup::busy(ui, "Fetching code…"),
            Content::ShowingList { selected, message } => {
                let (all_entries, dropped) = self
                    .cached_entries
                    .as_ref()
                    .map_or((&[][..], 0), |(e, d)| (e.as_slice(), *d));
                let max_visible = self.cfg.effective_max_visible();
                let entries = &all_entries[..all_entries.len().min(max_visible)];
                let hidden_by_cap = all_entries.len().saturating_sub(max_visible);
                let elevated = self.popup.target.is_some_and(|t| t.elevated_beyond_us);
                let kind = DeliveryKind::from_modifiers(ui.input(|i| i.modifiers));
                if let Some(clicked) = crate::ui::popup::showing_list(
                    ui,
                    entries,
                    *selected,
                    crate::ui::popup::ListInfo {
                        dropped,
                        hidden_by_cap,
                        message: message.as_deref(),
                        target_elevated: elevated,
                        icon_mode: kind.icon_mode(),
                    },
                ) {
                    // A click is a select-and-deliver, same as Enter or a
                    // digit — not just a selection change.
                    *selected = clicked;
                    *message = None;
                    post_action = Some(PostAction::Deliver(kind));
                }
            }
            Content::Settings(state) => {
                // Recording needs the *current* hotkey combination to be a
                // normal, capturable key event rather than intercepted
                // system-wide by its own `RegisterHotKey` registration (see
                // `Hotkey::suspend`'s doc) — otherwise re-confirming (or
                // just noticing you're retyping) the live combo is
                // impossible: the keystroke never reaches this window.
                if state.recording && !self.hotkey_suspended {
                    self.hotkey.suspend();
                    self.hotkey_suspended = true;
                } else if !state.recording && self.hotkey_suspended {
                    self.hotkey.resume();
                    self.hotkey_suspended = false;
                }

                match config_window::draw(ui, state) {
                    config_window::Action::None => {}
                    config_window::Action::Cancel => {
                        // Belt-and-suspenders beyond the check above: the
                        // Cancel button is reachable while `recording` is
                        // still true, which would otherwise leave the
                        // hotkey suspended with nothing left to un-suspend
                        // it once Settings closes.
                        if self.hotkey_suspended {
                            self.hotkey.resume();
                            self.hotkey_suspended = false;
                        }
                        post_action = Some(PostAction::Hide);
                    }
                    config_window::Action::Save => {
                        if self.hotkey_suspended {
                            self.hotkey.resume();
                            self.hotkey_suspended = false;
                        }
                        post_action = Some(PostAction::Save(state.clone()));
                    }
                }
            }
        }

        match post_action {
            None => {}
            Some(PostAction::Hide) => self.hide_popup(ui.ctx()),
            Some(PostAction::Save(state)) => match self.try_apply_config_window(&state) {
                Ok(()) => self.hide_popup(ui.ctx()),
                Err(e) => {
                    if let Content::Settings(s) = &mut self.popup.content {
                        s.message = Some((e, true));
                    }
                }
            },
            Some(PostAction::Deliver(kind)) => self.start_delivery(kind),
        }
    }
}
