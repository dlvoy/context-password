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
//! M5 wires it all together into the real popup (plan §1/§9): `Prompting`
//! (a password field that never touches `egui::TextEdit`, per F6),
//! `Unlocking`, and `ShowingList` with keyboard/mouse selection, feeding
//! real passwords into the M3 delivery machine instead of a hardcoded
//! payload. This is the first fully working flow end to end.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui;
use zeroize::Zeroizing;

use crate::bw::model::Entry;
use crate::config::{Config, UnlockMode};
use crate::hotkey::Hotkey;
use crate::msg::{BwCmd, BwResult, Msg, TrayCmd};
use crate::secret::Secret;
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
    },
    Unlocking,
    ShowingList {
        selected: usize,
    },
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
    _tray: tray_icon::TrayIcon,
    _hotkey: Hotkey,
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
        let hotkey_id = hotkey.id();
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |event: global_hotkey::GlobalHotKeyEvent| {
                if event.id() != hotkey_id || event.state() != global_hotkey::HotKeyState::Pressed
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

        let _tray = tray::build()?;

        let bw_cmd_tx = bw::spawn(
            cfg.bw_path.clone(),
            cfg.uri_prefix.clone(),
            tx.clone(),
            cc.egui_ctx.clone(),
        );

        Ok(Self {
            _tray,
            _hotkey: hotkey,
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
        })
    }

    /// Begins showing the popup for `target` (`None` for the tray/delayed
    /// path, with no cursor context). Content is decided once, here, from
    /// whichever cache state currently holds — not re-decided every frame.
    fn open_popup(&mut self, target: Option<Target>) {
        if self.delivery.is_some() {
            // Never interrupt a delivery already in flight.
            return;
        }
        self.popup.target = target;
        self.popup.content = match (&self.cached_entries, &self.popup.content) {
            (Some(_), _) => Content::ShowingList { selected: 0 },
            // An unlock submitted before the popup was last dismissed may
            // still be in flight on the worker thread — resume showing the
            // spinner instead of discarding that state and asking the user
            // to type their password again (`handle_bw_result` still
            // updates this content field even while the popup is hidden).
            (None, Content::Unlocking) => Content::Unlocking,
            (None, _) => PopupState::fresh_prompt(),
        };
        self.popup.phase = ShowPhase::Placing;
    }

    fn hide_popup(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        if let Some(target) = &self.popup.target {
            focus::activate_target(target);
        }
        self.popup.phase = ShowPhase::Hidden;
        self.popup.seen_focus = false;
    }

    /// The only place `bw` worker results reach stderr — names and ORDs
    /// only, per `Entry::log_line`, never a password — and where they feed
    /// back into the popup if it's waiting on them.
    fn handle_bw_result(&mut self, result: BwResult) {
        match result {
            BwResult::Items { entries, dropped } => {
                eprintln!(
                    "bw: unlocked, {} item(s) matched, {dropped} dropped",
                    entries.len()
                );
                for entry in &entries {
                    eprintln!("  {}", entry.log_line());
                }
                self.cached_entries = Some((entries, dropped));
                if matches!(self.popup.content, Content::Unlocking) {
                    self.popup.content = Content::ShowingList { selected: 0 };
                }
            }
            BwResult::Failed { stage, message } => {
                eprintln!("bw: failed at {stage}: {message}");
                if matches!(self.popup.content, Content::Unlocking) {
                    self.popup.content = Content::Prompting {
                        password: Zeroizing::new(String::with_capacity(256)),
                        error: Some(format!("{stage}: {message}")),
                    };
                }
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
            Deliver,
        }
        let mut action = Action::None;

        match &mut self.popup.content {
            Content::Prompting { password, error } => {
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
            Content::Unlocking => {
                // Dismiss-on-blur is suspended here (plan §1): an 8-15s
                // cold unlock that vanishes on a stray click would be
                // maddening. Escape still works — explicit intent.
                if escape {
                    action = Action::Hide;
                }
            }
            Content::ShowingList { selected } => {
                let count = self.cached_entries.as_ref().map_or(0, |(e, _)| e.len());
                if count > 0 {
                    if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                        *selected = (*selected + 1) % count;
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                        *selected = if *selected == 0 {
                            count - 1
                        } else {
                            *selected - 1
                        };
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::Home)) {
                        *selected = 0;
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::End)) {
                        *selected = count - 1;
                    }
                }
                if enter && count > 0 {
                    action = Action::Deliver;
                } else if escape || blurred {
                    action = Action::Hide;
                }
            }
        }

        match action {
            Action::None => {}
            Action::Hide => self.hide_popup(ctx),
            Action::Unlock(secret) => {
                let _ = self.bw_cmd_tx.send(BwCmd::Unlock(secret));
                self.popup.content = Content::Unlocking;
            }
            Action::Deliver => self.start_delivery(),
        }
    }

    fn start_delivery(&mut self) {
        let Content::ShowingList { selected } = &self.popup.content else {
            return;
        };
        let Some((entries, _)) = &self.cached_entries else {
            return;
        };
        let Some(entry) = entries.get(*selected) else {
            return;
        };
        let Some(target) = self.popup.target else {
            return;
        };

        eprintln!(
            "delivery: starting for target hwnd={} (elevated_beyond_us={}), item={}",
            target.hwnd,
            target.elevated_beyond_us,
            entry.log_line()
        );
        self.delivery = Some(Delivery {
            target,
            secret: entry.password.clone_secret(),
            phase: DeliveryPhase::HidePopup,
            deadline: Instant::now(),
        });
        self.popup.phase = ShowPhase::Hidden;
        self.popup.seen_focus = false;
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
                Msg::Hotkey(target) => self.open_popup(Some(target)),
                Msg::Bw(result) => self.handle_bw_result(result),
            }
        }

        match self.popup.phase {
            ShowPhase::Hidden => {}
            ShowPhase::Placing => {
                let (w_pt, h_pt) = POPUP_SIZE;
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.popup.phase != ShowPhase::Shown {
            return;
        }
        match &self.popup.content {
            Content::Prompting { password, error } => {
                crate::ui::popup::prompting(ui, password.chars().count(), error.as_deref());
            }
            Content::Unlocking => crate::ui::popup::unlocking(ui),
            Content::ShowingList { selected } => {
                let (entries, dropped) = self
                    .cached_entries
                    .as_ref()
                    .map_or((&[][..], 0), |(e, d)| (e.as_slice(), *d));
                let elevated = self
                    .popup
                    .target
                    .is_some_and(|t| t.elevated_beyond_us);
                if let Some(clicked) =
                    crate::ui::popup::showing_list(ui, entries, *selected, dropped, elevated)
                {
                    self.popup.content = Content::ShowingList { selected: clicked };
                }
            }
        }
    }
}
