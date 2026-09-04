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
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use eframe::egui;
use zeroize::Zeroizing;

use crate::config::{Config, UnlockMode};
use crate::controller::{
    self, Content, Delivery, DeliveryKind, DeliveryPhase, ExitState, IndicatorPhase, VaultState,
};
use crate::hotkey::Hotkey;
use crate::msg::{Msg, StaleNotice, TrayCmd, VaultCmd, VaultResult};
use crate::secret::Secret;
use crate::ui::config_window;
use crate::ui::config_window::ConfigWindowState;
use crate::platform;
use crate::platform::focus::{self, ActivationResult, Target};
use crate::vault::{Entry, Provider, VaultHandle};
use crate::tray;

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
// Hand-tuned to comfortably fit every row (the vault dropdown, the
// conditional KeePass database row, the hotkey recorder, three checkboxes,
// the max-visible-items stepper, and the footer) at the dialog's scaled-up
// font size (`config_window::FONT_SCALE`) with no clipping or crowding.
// Sized for the taller KeePass-selected state so the window doesn't
// visibly resize under the user when they flip the dropdown — unlike
// macOS's fixed-frame `settings.rs`, egui's immediate-mode layout would
// otherwise just reflow, but a resizing *window* mid-interaction still
// reads as a glitch.
const SETTINGS_SIZE: (i32, i32) = (460, 480);
/// Wider than Settings — the license text needs room to stay readable
/// without wrapping every line down to a couple of words.
const ABOUT_SIZE: (i32, i32) = (830, 480);
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
/// Square size (96-DPI-equivalent points) of the autotype indicator —
/// `platform::monitor::placement_for` scales it per-monitor the same way it does
/// `POPUP_SIZE`.
const INDICATOR_SIZE_PT: i32 = 32;

/// The height of a `ShowingList` popup that shows exactly `max_visible`
/// rows, no more and no less — no dead space, no overflow.
fn list_popup_height(max_visible: usize) -> i32 {
    (LIST_CHROME_HEIGHT + max_visible as f32 * crate::ui::popup::ITEM_ROW_HEIGHT).round() as i32
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
    vault: VaultHandle,
    hidden_after_startup: bool,
    popup: PopupState,
    delivery: Option<Delivery>,
    /// The autotype progress indicator's state — `None` when hidden.
    /// Independent of `delivery`: it must outlive `delivery` being set to
    /// `None` the moment typing finishes, since the checkmark still has to
    /// stay up for `INDICATOR_CHECK_DURATION` afterward (see
    /// `advance_indicator`).
    indicator: Option<IndicatorPhase>,
    stats: ActivationStats,
    /// The cache: populated once by a successful unlock, kept for the
    /// process's lifetime (the settled decision — see the plan's Context
    /// section). `None` means "never unlocked yet" and is what routes the
    /// popup to `Prompting` instead of `ShowingList`.
    cached_entries: Option<(Vec<Entry>, usize)>,
    /// Set whenever the most recent sync attempt (from a fresh unlock or
    /// the tray's Sync item) failed and `cached_entries` is therefore
    /// possibly stale — cleared the moment a sync actually succeeds.
    /// Deliberately not part of `Content::ShowingList`: unlike `message`,
    /// this must survive a selection change, since it describes the *data*
    /// rather than the last action. Also what makes a sync failure visible
    /// even when it happened in the background (popup on Settings/About/a
    /// fresh prompt) rather than being dropped entirely — see
    /// `handle_bw_result`'s `Failed` arm.
    sync_stale: Option<StaleNotice>,
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
        let hwnd = platform::window_style::hwnd_of(cc).ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                "failed to obtain the native window handle".into()
            },
        )?;
        platform::window_style::make_tool_window(hwnd);
        // Before the first paint — see `park_offscreen`'s doc for why.
        platform::window_style::park_offscreen(hwnd);

        // `ThemePreference::System` is egui's default already, so this is
        // just making the intent explicit — egui reads the OS theme from
        // winit (`RawInput::system_theme`) and switches `Visuals::dark()`/
        // `light()` automatically. The part that *isn't* automatic is
        // `clear_color` below.
        cc.egui_ctx.set_theme(egui::ThemePreference::System);

        // Cached once: never changes for the process's lifetime, and every
        // hotkey press needs it to evaluate the target's elevation (plan §5).
        let our_integrity_rid = platform::integrity::our_integrity_rid();

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
                } else if event.id() == tray::SYNC_ID {
                    TrayCmd::Sync
                } else if event.id() == tray::LOCK_ID {
                    TrayCmd::Lock
                } else if event.id() == tray::SETTINGS_ID {
                    TrayCmd::Settings
                } else if event.id() == tray::ABOUT_ID {
                    TrayCmd::About
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
        // "Sync"/"Lock" from the very first frame, not just after the first
        // state change.
        tray.set_show_label(false);
        tray.set_unlocked_items_visible(false);

        let bw_waker_ctx = cc.egui_ctx.clone();
        let vault = VaultHandle::spawn(
            &cfg,
            tx.clone(),
            std::sync::Arc::new(move || bw_waker_ctx.request_repaint()),
        );

        Ok(Self {
            tray,
            hotkey,
            hwnd,
            cfg,
            rx,
            vault,
            hidden_after_startup: false,
            popup: PopupState {
                phase: ShowPhase::Hidden,
                content: Content::fresh_prompt(),
                target: None,
                seen_focus: false,
            },
            delivery: None,
            indicator: None,
            stats: ActivationStats::default(),
            cached_entries: None,
            sync_stale: None,
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
            // A sync submitted before the popup was last dismissed may
            // still be in flight — resume showing its spinner regardless of
            // whether a cache already exists (unlike `Unlocking`/`Locking`
            // below, `cached_entries` is deliberately left populated during
            // a sync), or this arm would lose to `(Some(_), _)` and show the
            // stale list instead.
            (_, Content::Syncing) => Content::Syncing,
            (Some(_), _) => Content::ShowingList {
                selected: 0,
                message: None,
            },
            // An unlock (or lock) submitted before the popup was last
            // dismissed may still be in flight on the worker thread —
            // resume showing the spinner instead of discarding that state
            // (`handle_bw_result` still updates this content field even
            // while the popup is hidden).
            (None, Content::Unlocking) => Content::Unlocking,
            (None, Content::Locking) => Content::Locking,
            (None, _) => Content::fresh_prompt(),
        };
        self.popup.phase = ShowPhase::Placing;
        // `cached_entries` is a display buffer now, not the source of truth
        // (plan requirement #6) — re-enumerate on every open so a KeePass
        // database edited externally (or a Bitwarden item changed via
        // another client) doesn't show stale data just because this
        // session already had a list cached. For KeePass this is free (the
        // resident unlocked vault, no I/O); for Bitwarden it's `bw list
        // items` against the retained session, no `bw sync`. At worst one
        // tick stale — the already-cached list shows immediately, `List`'s
        // reply updates it a moment later.
        if self.vault_state == VaultState::Unlocked {
            self.vault.send(VaultCmd::List);
        }
    }

    /// Keeps the tray's `Show`/`Unlock` label and `Lock` item's presence in
    /// sync with `vault_state` — call this instead of assigning
    /// `self.vault_state` directly, so the two can never drift apart.
    fn set_vault_state(&mut self, state: VaultState) {
        self.vault_state = state;
        self.tray.set_show_label(state == VaultState::Unlocked);
        self.tray
            .set_unlocked_items_visible(state == VaultState::Unlocked);
    }

    /// Called right after `self.vault.respawn(..)` — the old worker's
    /// secrets are already gone the moment its `Sender` is replaced (see
    /// `VaultHandle::respawn`'s doc), so unlike `handle_lock` this snaps
    /// straight to `Locked`/a fresh prompt rather than sending a command
    /// and waiting for a reply. Deliberately does **not** send
    /// `VaultCmd::Lock` to the old worker first — that would be redundant
    /// (dropping the sender already destroys its secrets) and, per
    /// `Config::lock_on_exit`'s own existing rationale, a provider switch
    /// isn't an implicit "log out of Bitwarden everywhere" request. No
    /// explicit popup redraw needed here (unlike macOS's retained-mode
    /// `show_popup`) — `ui()` redraws from `self.popup.content` every
    /// frame regardless of whether the popup is currently visible.
    fn reset_for_provider_switch(&mut self) {
        eprintln!("settings: vault provider switched — clearing cached items and locking");
        self.cached_entries = None;
        self.sync_stale = None;
        self.delivery = None;
        self.hide_indicator();
        if self.hotkey_suspended {
            self.hotkey.resume();
            self.hotkey_suspended = false;
        }
        self.set_vault_state(VaultState::Locked);
        self.popup.content = Content::fresh_prompt();
    }

    fn hide_popup(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        if let Some(target) = &self.popup.target {
            focus::activate_target(target);
        }
        self.popup.phase = ShowPhase::Hidden;
        self.popup.seen_focus = false;
        // A leftover checkmark from a delivery the user has since moved on
        // from (by dismissing and reopening the popup) shouldn't linger.
        self.hide_indicator();
        // Zeroize a half-typed password immediately on dismiss rather than
        // waiting for the next `open_popup` to overwrite (and so drop) it —
        // no reason for it to sit in memory for however long the popup
        // happens to stay closed (plan §8's zeroize audit, M8). Settings and
        // About get the same eager reset for consistency, even though
        // `open_popup`'s own content-decision match would overwrite either
        // anyway on the next open (it's keyed on `phase`, not `content`).
        if matches!(
            self.popup.content,
            Content::Prompting { .. } | Content::Settings(_) | Content::About
        ) {
            self.popup.content = Content::fresh_prompt();
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
        self.sync_stale = None;
        if self.hotkey_suspended {
            // Settings might be mid-recording when a global Lock action
            // interrupts it — don't leave the hotkey unregistered with
            // nothing left to resume it.
            self.hotkey.resume();
            self.hotkey_suspended = false;
        }
        self.popup.content = Content::Locking;
        self.set_vault_state(VaultState::Locking);
        self.vault.send(VaultCmd::Lock);
    }

    /// Re-runs `bw sync` + `bw list items` against the retained session,
    /// without disturbing the popup — silent, like the tray's Lock item
    /// isn't but this deliberately is (see the plan): a click on the tray
    /// icon shouldn't yank focus. If the popup happens to already be open on
    /// the list (or is closed), it switches to `Content::Syncing` so the
    /// hotkey/Show reveals progress instead of a stale list; any other
    /// screen (Settings, About, mid-prompt, fetching a TOTP) is left alone
    /// — the refresh still happens in the background.
    fn handle_sync(&mut self) {
        if self.vault_state != VaultState::Unlocked
            || matches!(self.popup.content, Content::Syncing)
        {
            return;
        }
        eprintln!("sync: refreshing cached items");
        if self.popup.phase == ShowPhase::Hidden
            || matches!(self.popup.content, Content::ShowingList { .. })
        {
            self.popup.content = Content::Syncing;
        }
        self.vault.send(VaultCmd::Sync);
    }

    /// The only place `bw` worker results reach stderr — names and ORDs
    /// only, per `Entry::log_line`, never a password (and only at all when
    /// `debug_log` is on — plan §8's log review, M8) — and where they feed
    /// back into the popup if it's waiting on them.
    fn handle_vault_result(&mut self, result: VaultResult) {
        match result {
            VaultResult::Items { entries, dropped, stale } => {
                // A Sync can be in flight when the tray's Lock item — still
                // visible throughout a sync, since `vault_state` stays
                // `Unlocked` — fires and clears `cached_entries` right out
                // from under it. The worker processes commands strictly in
                // send order (one `mpsc` queue, `BwCmd::Lock` queued after
                // `BwCmd::Sync`), so `handle_lock`'s `Locking` state is still
                // current when this stale result lands: dropping it here,
                // rather than resurrecting `cached_entries` and stomping
                // `vault_state` back to `Unlocked`, is what keeps "locked"
                // meaning "nothing cached in memory." An in-flight *unlock*
                // never hits this arm at `Locking` — the tray has no way to
                // request a lock before an unlock's own `Items`/`Failed`
                // result has already moved `vault_state` off `Locked`.
                if self.vault_state == VaultState::Locking {
                    if self.cfg.debug_log {
                        eprintln!("bw: dropping stale sync result — a lock is in flight");
                    }
                    return;
                }
                if self.cfg.debug_log {
                    eprintln!(
                        "bw: unlocked, {} item(s) matched, {dropped} dropped",
                        entries.len()
                    );
                    for entry in &entries {
                        eprintln!("  {}", entry.log_line());
                    }
                }
                if self.cfg.debug_log
                    && let Some(notice) = &stale
                {
                    eprintln!(
                        "bw: sync did not succeed ({}): {} — showing the cached list anyway",
                        notice.reason.summary(self.vault.provider()),
                        notice.detail
                    );
                }
                self.cached_entries = Some((entries, dropped));
                self.sync_stale = stale;
                self.set_vault_state(VaultState::Unlocked);
                // Also refreshes an *already-shown* list — not just the
                // Unlocking/Syncing transitions — since `VaultCmd::List`
                // (sent on every popup open, see `open_popup`) can now
                // deliver an `Items` result while the popup is already
                // sitting on `ShowingList` from a still-fresh previous
                // fetch. Preserves the current selection rather than
                // resetting it to 0, since this is a background refresh,
                // not a new list appearing — and deliberately does *not*
                // re-enter `Placing` in that case (unlike the Unlocking/
                // Syncing transition below): the window is already
                // correctly sized for a list, and re-placing on every
                // background refresh would visibly jank the window under
                // the user.
                if let Content::Unlocking | Content::Syncing | Content::ShowingList { .. } =
                    self.popup.content
                {
                    let was_already_showing_list = matches!(self.popup.content, Content::ShowingList { .. });
                    let selected = if let Content::ShowingList { selected, .. } = self.popup.content {
                        selected
                    } else {
                        0
                    };
                    self.popup.content = Content::ShowingList { selected, message: None };
                    if !was_already_showing_list && self.popup.phase == ShowPhase::Shown {
                        // The window is already placed/sized for the
                        // smaller Prompting/Unlocking/Syncing screen —
                        // re-enter Placing so it picks up the list's own
                        // (dynamic) width and height instead of keeping
                        // whatever size it had before this switch. When the
                        // popup was hidden instead, no fix-up is needed:
                        // `open_popup` always re-places from scratch next
                        // time.
                        self.popup.phase = ShowPhase::Placing;
                    }
                }
            }
            VaultResult::Failed { stage, kind, message } => {
                eprintln!("bw: failed at {stage}: {message}");
                if stage == "sync" {
                    // A sync only ever reaches `Failed` (rather than
                    // `Items { stale: Some(_), .. }`) via a hard failure —
                    // no session to sync with, or `bw` itself couldn't be
                    // resolved/spawned — never a plain non-zero exit, which
                    // `sync_then_list` already turns into a `StaleNotice`
                    // alongside a still-usable list. Record it the same way
                    // regardless of what the popup happens to be showing
                    // right now, so the banner still appears the next time
                    // the list is shown rather than being silently dropped
                    // (the `_ => {}` arm below, when the popup is on
                    // Settings/About/a fresh prompt during a background
                    // sync).
                    self.sync_stale = Some(StaleNotice {
                        reason: kind,
                        last_sync: None,
                        detail: message.clone(),
                    });
                }
                match &self.popup.content {
                    Content::Unlocking => {
                        // `message` is already a complete, user-facing
                        // sentence by this point for every stage that can
                        // reach `Unlocking` ("resolve"'s own error text, or
                        // `precise_unlock_message`'s classification-driven
                        // text for "unlock") — no "{stage}: " prefix needed.
                        self.popup.content = Content::Prompting {
                            password: Zeroizing::new(String::with_capacity(256)),
                            error: Some(message.clone()),
                            revealed: false,
                        };
                    }
                    Content::Syncing => {
                        // Unlike a failed unlock, there's an existing list
                        // to fall back to (a sync never clears
                        // `cached_entries`) — report the failure inline on
                        // it rather than dropping all the way back to the
                        // master-password prompt. The `None` case shouldn't
                        // be reachable (Sync only ever runs while unlocked,
                        // which implies a cache), but degrades to the
                        // prompt rather than panicking if it somehow is.
                        self.popup.content = match &self.cached_entries {
                            Some(_) => Content::ShowingList {
                                selected: 0,
                                message: Some(format!("{stage}: {message}")),
                            },
                            None => Content::fresh_prompt(),
                        };
                        if self.popup.phase == ShowPhase::Shown {
                            // Same re-measure as the success path above —
                            // this can also switch a spinner-sized popup to
                            // the list (or the prompt).
                            self.popup.phase = ShowPhase::Placing;
                        }
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
            VaultResult::Locked => {
                if self.cfg.debug_log {
                    eprintln!("bw: lock finished");
                }
                self.set_vault_state(VaultState::Locked);
                if matches!(self.popup.content, Content::Locking) {
                    self.popup.content = Content::fresh_prompt();
                }
            }
            VaultResult::Totp(code) => {
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
                self.popup.content = Content::ShowingList {
                    selected,
                    message: None,
                };
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
        // Only the successful `Settle` branch sets this — every abort path
        // leaves it `false` so the indicator is hidden outright below
        // rather than fed into the Typing->Done transition.
        let mut typed = false;

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
                if platform::typing::modifiers_up() || Instant::now() >= delivery.deadline {
                    delivery.phase = DeliveryPhase::Activate;
                }
            }
            DeliveryPhase::Activate => {
                if !delivery.target.still_valid() {
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
                if delivery.target.is_foreground() {
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
                    if let Some(reason) = delivery.target.blocked() {
                        eprintln!(
                            "delivery aborted: target can't receive synthetic keystrokes \
                             ({reason:?})"
                        );
                    } else {
                        let skipped = platform::typing::send_unicode(delivery.secret.expose());
                        eprintln!(
                            "delivery: typed the selected password ({skipped} code unit(s) skipped)"
                        );
                        typed = true;
                    }
                    done = true;
                }
            }
        }

        if done {
            self.delivery = None;
            if typed {
                if let Some(IndicatorPhase::Typing { typed, .. }) = self.indicator.as_mut() {
                    *typed = true;
                }
            } else {
                self.hide_indicator();
            }
        } else {
            ctx.request_repaint_after(POLL_INTERVAL);
        }
    }

    /// Shows the autotype indicator at the *live* cursor position (not
    /// `target.cursor()`, which is frozen at hotkey-press time — the user
    /// may have moved the mouse since) as the keyboard glyph.
    fn show_indicator(&mut self) {
        let cursor = focus::cursor_pos();
        let placement =
            platform::monitor::placement_for(Some(cursor), INDICATOR_SIZE_PT, INDICATOR_SIZE_PT);
        platform::indicator::show(
            platform::indicator::Kind::Typing,
            placement.x,
            placement.y,
            placement.w,
        );
        self.indicator = Some(IndicatorPhase::Typing {
            since: Instant::now(),
            typed: false,
        });
    }

    fn hide_indicator(&mut self) {
        self.indicator = None;
        platform::indicator::hide();
    }

    /// Advances the indicator's own timer, independent of `delivery` (which
    /// is long gone by the time the checkmark's 2 seconds run out). Mirrors
    /// `advance_delivery`'s never-blocks, request-a-repaint-and-return
    /// shape.
    fn advance_indicator(&mut self, ctx: &egui::Context) {
        let Some(phase) = self.indicator else {
            return;
        };
        match controller::next_indicator_phase(phase, Instant::now()) {
            Some(next) => {
                if matches!(phase, IndicatorPhase::Typing { .. })
                    && matches!(next, IndicatorPhase::Done { .. })
                {
                    platform::indicator::set_kind(platform::indicator::Kind::Done);
                }
                self.indicator = Some(next);
                ctx.request_repaint_after(POLL_INTERVAL);
            }
            None => self.hide_indicator(),
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
            Content::Prompting {
                password, error, ..
            } => {
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
            Content::Unlocking | Content::Locking | Content::Syncing => {
                // Dismiss-on-blur is suspended here (plan §1): an 8-15s
                // cold unlock (or a lock/sync) that vanishes on a stray
                // click would be maddening. Escape still works — explicit
                // intent.
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
                    if let Some(i) = ctx
                        .input(|i| controller::digit_pressed(&i.events))
                        .filter(|&i| i < count)
                    {
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
            // Same reasoning — Close/Escape are read directly in
            // `ui::about::draw`.
            Content::About => {}
        }

        match action {
            Action::None => {}
            Action::Hide => self.hide_popup(ctx),
            Action::Unlock(secret) => {
                self.vault.send(VaultCmd::Unlock(secret));
                self.popup.content = Content::Unlocking;
            }
            Action::Deliver(kind) => self.start_delivery(kind),
            Action::BackToList(selected) => {
                self.popup.content = Content::ShowingList {
                    selected,
                    message: None,
                };
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
                    self.set_list_message(
                        selected,
                        "This item has no TOTP configured.".to_string(),
                    );
                    return;
                }
                let item_id = entry.id.clone();
                self.popup.content = Content::FetchingOtp { selected };
                self.vault.send(VaultCmd::GetTotp(item_id));
            }
        }
    }

    /// Replaces the popup's content with `ShowingList` at `selected`,
    /// carrying an inline error — used when a requested field (username,
    /// OTP) can't be delivered, per the settled decision to say so rather
    /// than silently falling back to the password.
    fn set_list_message(&mut self, selected: usize, message: String) {
        self.popup.content = Content::ShowingList {
            selected,
            message: Some(message),
        };
    }

    /// The common tail of every delivery kind once its secret is in hand:
    /// hides the popup and hands off to the frame-driven `Delivering`
    /// machine (plan §4). `item_log_line` is only ever printed when
    /// `debug_log` is on (plan §8).
    fn begin_delivery(&mut self, target: Target, secret: Secret, item_log_line: &str) {
        eprintln!("delivery: starting for target {target:?}");
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
        self.show_indicator();
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
            provider: self.cfg.provider,
            keepass_path: self.cfg.keepass_path.clone(),
            hotkey_spec: self.hotkey.spec(),
            recording: false,
            autostart: platform::autostart::is_enabled(),
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

    /// Opens the About screen — same reasoning and same no-cursor-context
    /// centering as `open_settings`, minus any state to seed since there's
    /// nothing to edit.
    fn open_about(&mut self) {
        if self.delivery.is_some() {
            return;
        }
        self.popup.target = None;
        self.popup.content = Content::About;
        self.popup.phase = ShowPhase::Placing;
    }

    /// Applies whichever of the hotkey/autostart actually changed, saves
    /// the config, and reports the first failure (if any) rather than
    /// silently discarding it — but still applies whatever *did* succeed
    /// rather than requiring all-or-nothing, since a failed autostart
    /// toggle is no reason to also refuse a valid hotkey change.
    fn try_apply_config_window(&mut self, state: &ConfigWindowState) -> Result<(), String> {
        // Validated first, and refuses the whole save on failure (rather
        // than applying the other fields and only complaining about this
        // one) — there's no such thing as a partially-valid provider
        // selection.
        Config::provider_ready(state.provider, &state.keepass_path)?;
        if state.provider == Provider::KeePass {
            let path = state.keepass_path.trim();
            if !std::path::Path::new(path).is_file() {
                return Err(format!("KeePass database not found: {path}"));
            }
        }

        let mut error = None;

        // Deliberately inert as of this phase: changing the provider or the
        // KeePass path here persists to `self.cfg` but does not yet respawn
        // the vault worker (that's `vault::VaultHandle`, Phase 4) — the app
        // still unconditionally runs the `bw` backend until then.
        if state.provider != self.cfg.provider {
            eprintln!("settings: vault provider changed to {:?}", state.provider);
            self.cfg.provider = state.provider;
        }
        let trimmed_keepass_path = state.keepass_path.trim();
        if trimmed_keepass_path != self.cfg.keepass_path {
            eprintln!("settings: KeePass database path changed");
            self.cfg.keepass_path = trimmed_keepass_path.to_string();
        }

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
            match platform::autostart::set_enabled(state.autostart) {
                Ok(()) => {
                    eprintln!(
                        "settings: autostart {}",
                        if state.autostart {
                            "enabled"
                        } else {
                            "disabled"
                        }
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
        let new_unlock_mode = if state.auto_unlock {
            UnlockMode::Delayed
        } else {
            UnlockMode::Lazy
        };
        if new_unlock_mode != self.cfg.unlock_mode {
            eprintln!("settings: auto-unlock at start {}", state.auto_unlock);
            self.cfg.unlock_mode = new_unlock_mode;
        }
        if state.lock_on_exit != self.cfg.lock_on_exit {
            eprintln!(
                "settings: lock on exit {}",
                if state.lock_on_exit {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            self.cfg.lock_on_exit = state.lock_on_exit;
        }
        let clamped_max_visible = state.max_visible_items.clamp(
            crate::config::MIN_VISIBLE_ITEMS,
            crate::config::MAX_VISIBLE_ITEMS,
        );
        if clamped_max_visible != self.cfg.max_visible_items {
            eprintln!("settings: max visible items changed to {clamped_max_visible}");
            self.cfg.max_visible_items = clamped_max_visible;
        }

        match self.cfg.save() {
            Ok(()) => {
                if self.vault.needs_respawn(&self.cfg) {
                    self.vault.respawn(&self.cfg);
                    self.reset_for_provider_switch();
                }
            }
            Err(e) => {
                error.get_or_insert(format!("saving config: {e}"));
            }
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
    /// unlike `ui`, which draws only while `Shown` (see its own doc for why
    /// that's an early return here, not an eframe guarantee). This is where
    /// all inbound events (tray, hotkey, `bw` worker) get drained, and
    /// where the show/hide and delivery phase machines advance.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.hidden_after_startup {
            self.hidden_after_startup = true;
            // eframe's glow backend always reveals the root window after its
            // first rendered frame, regardless of the ViewportBuilder's
            // `with_visible(false)` — see `window_style::park_offscreen`,
            // which keeps that one unavoidable frame off-screen.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Tray(TrayCmd::Quit) => {
                    self.hide_indicator();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Msg::Tray(TrayCmd::Show) | Msg::ShowPopup => {
                    self.open_popup(None);
                }
                Msg::Tray(TrayCmd::Sync) => self.handle_sync(),
                Msg::Tray(TrayCmd::Lock) => self.handle_lock(),
                Msg::Tray(TrayCmd::Settings) => self.open_settings(),
                Msg::Tray(TrayCmd::About) => self.open_about(),
                Msg::Hotkey(target) => self.open_popup(Some(target)),
                Msg::Vault { generation, result } => {
                    // A stale generation means this result is from a
                    // worker already torn down by a provider switch
                    // (`respawn`) — drop it rather than resurrecting a
                    // dead provider's entries into the live one's UI.
                    if self.vault.accepts(generation) {
                        self.handle_vault_result(result);
                    } else if self.cfg.debug_log {
                        eprintln!("vault: dropping a result from a torn-down worker (generation {generation})");
                    }
                }
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
                if close_requested
                    && self.cfg.lock_on_exit
                    && self.vault_state == VaultState::Unlocked
                {
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
                    Content::About => ABOUT_SIZE,
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
                let cursor = self.popup.target.map(|t| t.cursor());
                let placement = platform::monitor::placement_for(cursor, w_pt, h_pt);
                // Reapply defensively — M1 found this doesn't reliably
                // survive a visibility transition (see window_style.rs).
                platform::window_style::make_tool_window(self.hwnd);
                platform::window_style::place(
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
        self.advance_indicator(ctx);
    }

    /// Only ever draws while `Shown` (the earlier phases have nothing to
    /// draw yet) — note this is enforced by the early return below, not by
    /// eframe: contrary to `logic`'s doc, eframe's glow backend derives
    /// visibility from `ViewportInfo::visible()`, which `egui-winit` never
    /// populates (it doesn't set `occluded`), so eframe actually runs a
    /// full paint every tick even while the window is hidden.
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
            /// Run the (blocking, modal) `rfd` picker — deliberately
            /// deferred to *after* this frame's drawing, not run from
            /// inside `config_window::draw`: see `config_window::Action::
            /// BrowseKeepass`'s doc for why.
            BrowseKeepass,
        }
        let mut post_action = None;

        match &mut self.popup.content {
            Content::Prompting {
                password,
                error,
                revealed,
            } => {
                if crate::ui::popup::prompting(ui, password, *revealed, error.as_deref()) {
                    *revealed = !*revealed;
                }
            }
            Content::Unlocking => crate::ui::popup::busy(ui, "Unlocking vault…"),
            Content::Locking => crate::ui::popup::busy(ui, "Locking vault…"),
            Content::Syncing => crate::ui::popup::busy(ui, "Syncing…"),
            Content::FetchingOtp { .. } => crate::ui::popup::busy(ui, "Fetching code…"),
            Content::ShowingList { selected, message } => {
                let (all_entries, dropped) = self
                    .cached_entries
                    .as_ref()
                    .map_or((&[][..], 0), |(e, d)| (e.as_slice(), *d));
                let max_visible = self.cfg.effective_max_visible();
                let entries = &all_entries[..all_entries.len().min(max_visible)];
                let hidden_by_cap = all_entries.len().saturating_sub(max_visible);
                let blocked = self.popup.target.and_then(|t| t.blocked());
                let kind = DeliveryKind::from_modifiers(ui.input(|i| i.modifiers));
                if let Some(clicked) = crate::ui::popup::showing_list(
                    ui,
                    entries,
                    *selected,
                    crate::ui::popup::ListInfo {
                        dropped,
                        hidden_by_cap,
                        message: message.as_deref(),
                        target_blocked: blocked,
                        stale: self.sync_stale.as_ref(),
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
                    config_window::Action::BrowseKeepass => {
                        post_action = Some(PostAction::BrowseKeepass);
                    }
                }
            }
            Content::About => {
                if crate::ui::about::draw(ui) {
                    post_action = Some(PostAction::Hide);
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
            Some(PostAction::BrowseKeepass) => {
                let picked = rfd::FileDialog::new()
                    .add_filter("KeePass database", &["kdbx"])
                    .set_title("Choose a KeePass database")
                    .pick_file();
                if let Some(path) = picked
                    && let Content::Settings(s) = &mut self.popup.content
                {
                    s.keepass_path = path.to_string_lossy().into_owned();
                }
            }
        }
    }
}

// The digit-selection and indicator-phase unit tests that used to live
// here moved to `controller.rs` along with the pure functions/types they
// test (`digit_pressed`, `next_indicator_phase`, `IndicatorPhase`).
