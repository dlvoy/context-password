//! The `eframe::App` implementation. The root viewport is the popup window
//! itself (see plan §F5 — it's the one that needs Win32 surgery, so it has
//! to be the root, with the config window as a child viewport once M7 adds
//! it).
//!
//! M2 scope: the hotkey summons a placeholder popup at the cursor via the
//! three-frame show sequence (plan §2), self-activates it (§3), and Esc or
//! focus loss dismisses it with a best-effort restore of the previous
//! foreground window. There is no real item list, unlock flow, or typing
//! yet — those are M3 onward.

use std::sync::mpsc::{self, Receiver};

use eframe::egui;

use crate::hotkey::Hotkey;
use crate::msg::{Msg, TrayCmd};
use crate::win::focus::{self, ActivationResult, Target};
use crate::{tray, win};

/// The popup's show/hide state. `Placing`/`Showing`/`Activating` are the
/// three frames of the plan's show sequence (§2) — each one advances on the
/// next `logic()` call, chained via `request_repaint()`.
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

    fn total(&self) -> u32 {
        self.direct + self.attached + self.failed
    }
}

pub struct App {
    // Order matters for Drop: dropping the tray icon removes it from the
    // shell; nothing here depends on drop order otherwise, but keep it
    // explicit rather than relying on field order being incidental.
    _tray: tray_icon::TrayIcon,
    _hotkey: Hotkey,
    hwnd: windows_sys::Win32::Foundation::HWND,
    rx: Receiver<Msg>,
    hidden_after_startup: bool,
    popup: PopupState,
    stats: ActivationStats,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        hotkey: Hotkey,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let hwnd = win::window_style::hwnd_of(cc)
            .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                "failed to obtain the native window handle".into()
            })?;
        win::window_style::make_tool_window(hwnd);

        let (tx, rx) = mpsc::channel();

        // Both handlers below run synchronously on the UI thread, inside
        // DispatchMessage — for the hotkey that's exactly what plan F1
        // relies on: WM_HOTKEY fires here before any window of ours has
        // shown, so this is the only correct place to capture the
        // foreground window and cursor position.
        let menu_tx = tx.clone();
        let menu_ctx = cc.egui_ctx.clone();
        tray_icon::menu::MenuEvent::set_event_handler(Some(
            move |event: tray_icon::menu::MenuEvent| {
                if event.id() == tray::QUIT_ID {
                    let _ = menu_tx.send(Msg::Tray(TrayCmd::Quit));
                    menu_ctx.request_repaint();
                }
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
                if let Some(target) = focus::capture_target() {
                    let _ = hotkey_tx.send(Msg::Hotkey(target));
                    hotkey_ctx.request_repaint();
                }
            },
        ));

        let _tray = tray::build()?;

        Ok(Self {
            _tray,
            _hotkey: hotkey,
            hwnd,
            rx,
            hidden_after_startup: false,
            popup: PopupState {
                phase: ShowPhase::Hidden,
                target: None,
                seen_focus: false,
            },
            stats: ActivationStats::default(),
        })
    }

    fn hide_popup(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        if let Some(target) = &self.popup.target {
            focus::restore_target(target);
        }
        self.popup.phase = ShowPhase::Hidden;
        self.popup.seen_focus = false;
    }
}

impl eframe::App for App {
    /// Runs every frame regardless of whether the root window is visible —
    /// unlike `ui`, which eframe skips entirely while hidden. This is where
    /// all inbound events (tray, hotkey; the `bw` worker joins from M4) get
    /// drained, and where the show/hide phase machine advances.
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
                Msg::Hotkey(target) => {
                    self.popup.target = Some(target);
                    self.popup.phase = ShowPhase::Placing;
                }
            }
        }

        match self.popup.phase {
            ShowPhase::Hidden => {}
            ShowPhase::Placing => {
                if let Some(target) = self.popup.target {
                    let (cx, cy) = target.cursor;
                    // Reapply defensively — M1 found this doesn't reliably
                    // survive a visibility transition (see window_style.rs).
                    win::window_style::make_tool_window(self.hwnd);
                    win::window_style::place(self.hwnd, cx + 8, cy + 8, 340, 220);
                }
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
                eprintln!(
                    "activate_self -> {result:?}  (direct={} attached={} failed={}, total={})",
                    self.stats.direct,
                    self.stats.attached,
                    self.stats.failed,
                    self.stats.total()
                );
                self.popup.seen_focus = false;
                self.popup.phase = ShowPhase::Shown;
                ctx.request_repaint();
            }
            ShowPhase::Shown => {
                let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
                if focused {
                    self.popup.seen_focus = true;
                }
                let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                let blurred = self.popup.seen_focus && !focused;
                if escape || blurred {
                    self.hide_popup(ctx);
                }
            }
        }
    }

    /// Only ever runs while `Shown` — eframe skips `ui` entirely for a
    /// hidden window, and the earlier phases have nothing to draw yet.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.popup.phase != ShowPhase::Shown {
            return;
        }
        ui.heading("context-password — M2 spike");
        ui.separator();
        ui.label(format!(
            "activations: direct={} attached={} failed={} (total {})",
            self.stats.direct,
            self.stats.attached,
            self.stats.failed,
            self.stats.total()
        ));
        ui.label("Esc to dismiss");
    }
}
