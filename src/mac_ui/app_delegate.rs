//! The `NSApplicationDelegate` — owns everything: the hotkey manager, tray,
//! `bw` worker, popup panel, and all the state `app.rs`'s `App` struct
//! holds on Windows. Driven by one repeating `NSTimer` (see `on_tick`'s
//! doc for why a simple always-on poll was chosen over a dynamically
//! armed/disarmed one) rather than eframe's per-frame `logic()`/`ui()`
//! split — there's no render loop to piggyback on here, so polling is the
//! direct replacement for `request_repaint_after`.

use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSControl,
    NSControlTextEditingDelegate, NSEventModifierFlags, NSTextFieldDelegate, NSTextView,
};
use objc2_foundation::{MainThreadMarker, NSNotification, NSObjectProtocol, NSTimer};

use crate::bw;
use crate::bw::model::Entry;
use crate::config::{Config, UnlockMode};
use crate::controller::{Delivery, DeliveryPhase, IndicatorPhase, VaultState};
use crate::hotkey::Hotkey;
use crate::msg::{BwCmd, BwResult, Msg, StaleNotice, TrayCmd};
use crate::platform::focus::{self, Target};
use crate::platform::{self, BlockReason};
use crate::secret::Secret;
use crate::tray;

use super::about;
use super::panel::Panel;
use super::settings::{self, SettingsWindow};
use super::state::{DeliveryKind, PopupContent};

/// How often `tick:` fires — the single polling interval standing in for
/// every `request_repaint_after(POLL_INTERVAL)` call on Windows
/// (`MODIFIER_DRAIN`/`FOREGROUND_VERIFY`/delivery `Settle`/indicator
/// animation/lock-on-exit wait). Deliberately always-on rather than
/// dynamically armed/disarmed: a `CFRunLoopSource`-based waker plus a
/// re-armed one-shot timer is the "correct" design (see the port plan),
/// but it's meaningfully more code for a cost — an idle wakeup every 16ms —
/// that's negligible in practice for a tray app. Revisit if profiling ever
/// says otherwise.
const TICK_INTERVAL_SECS: f64 = 0.016;

const MODIFIER_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);
const FOREGROUND_VERIFY_TIMEOUT: Duration = Duration::from_millis(500);
const INDICATOR_SIZE_PT: i32 = 32;

struct AppState {
    cfg: Config,
    rx: Receiver<Msg>,
    bw_cmd_tx: Sender<BwCmd>,
    hotkey: Hotkey,
    hotkey_suspended: bool,
    tray: tray::TrayHandles,
    panel: Panel,
    popup_visible: bool,
    popup_content: PopupContent,
    popup_target: Option<Target>,
    /// Dismiss-on-blur latch, same reasoning as Windows'
    /// `PopupState::seen_focus`: the panel is visible but not yet key for
    /// one tick after `show_at`, so a blur can't be trusted as "the user
    /// clicked away" until it's been key at least once.
    seen_key: bool,
    cached_entries: Option<(Vec<Entry>, usize)>,
    sync_stale: Option<StaleNotice>,
    vault_state: VaultState,
    delivery: Option<Delivery>,
    indicator: Option<IndicatorPhase>,
    settings_window: Option<SettingsWindow>,
    about_window: Option<Retained<objc2_app_kit::NSWindow>>,
}

pub struct AppIvars {
    state: RefCell<Option<AppState>>,
}

impl Default for AppIvars {
    fn default() -> Self {
        Self { state: RefCell::new(None) }
    }
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = AppIvars]
    #[name = "CPAppDelegate"]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            self.setup();
        }
    }

    unsafe impl NSControlTextEditingDelegate for AppDelegate {
        #[unsafe(method(control:textView:doCommandBySelector:))]
        unsafe fn control_text_view_do_command_by_selector(
            &self,
            _control: &NSControl,
            _text_view: &NSTextView,
            command_selector: Sel,
        ) -> bool {
            self.handle_command(command_selector)
        }
    }

    unsafe impl NSTextFieldDelegate for AppDelegate {}

    impl AppDelegate {
        #[unsafe(method(rowClicked:))]
        fn row_clicked(&self, sender: &NSObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            self.handle_row_clicked(tag as usize);
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            self.on_tick();
        }

        #[unsafe(method(settingsSave:))]
        fn settings_save(&self, _sender: &NSObject) {
            self.handle_settings_save();
        }

        #[unsafe(method(settingsCancel:))]
        fn settings_cancel(&self, _sender: &NSObject) {
            let mut guard = self.ivars().state.borrow_mut();
            if let Some(state) = guard.as_mut()
                && let Some(window) = &state.settings_window
            {
                window.hide();
            }
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppIvars::default());
        unsafe { msg_send![super(this), init] }
    }

    fn setup(&self) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let cfg = match Config::load() {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("failed to load config, using defaults: {e}");
                Config::default()
            }
        };
        eprintln!("context-password starting (hotkey={})", cfg.hotkey);

        let (tx, rx) = mpsc::channel::<Msg>();

        let menu_tx = tx.clone();
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
            },
        ));

        // Registered here, not before `setActivationPolicy` — Carbon's
        // `RegisterEventHotKey` only routes to a process the WindowServer
        // treats as an app (see the port plan's Phase 5 notes on this
        // ordering). Confirmed synchronous main-thread delivery via the
        // Phase 0 spike, which is what makes capturing the frontmost app
        // inline here (rather than a moment later) correct — by the time
        // any window of ours shows, the real frontmost app is gone.
        let hotkey = match Hotkey::register(&cfg.hotkey) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("failed to register global hotkey '{}': {e}", cfg.hotkey);
                return;
            }
        };
        let hotkey_tx = tx.clone();
        let hotkey_id = hotkey.id_handle();
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |event: global_hotkey::GlobalHotKeyEvent| {
                if event.id() != hotkey_id.load(Ordering::Relaxed)
                    || event.state() != global_hotkey::HotKeyState::Pressed
                {
                    return;
                }
                if let Some(target) = focus::capture_target() {
                    let _ = hotkey_tx.send(Msg::Hotkey(target));
                }
            },
        ));

        if cfg.unlock_mode == UnlockMode::Delayed {
            let delay_tx = tx.clone();
            let delay = Duration::from_secs(u64::from(cfg.unlock_delay_secs));
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let _ = delay_tx.send(Msg::ShowPopup);
            });
        }

        let mut tray = match tray::build() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("failed to create tray icon: {e}");
                return;
            }
        };
        tray.set_show_label(false);
        tray.set_unlocked_items_visible(false);

        // The timer alone drains `rx` every tick, so `bw::spawn`'s waker
        // has nothing to do — unlike Windows' `ctx.request_repaint()`,
        // which is the *only* thing that wakes an otherwise-idle egui loop.
        let bw_cmd_tx = bw::spawn(
            cfg.bw_path.clone(),
            cfg.uri_prefix.clone(),
            tx.clone(),
            Arc::new(|| {}),
        );

        let delegate_protocol = ProtocolObject::from_ref(self);
        let panel = Panel::new(mtm, delegate_protocol);

        *self.ivars().state.borrow_mut() = Some(AppState {
            cfg,
            rx,
            bw_cmd_tx,
            hotkey,
            hotkey_suspended: false,
            tray,
            panel,
            popup_visible: false,
            popup_content: PopupContent::fresh_prompt(),
            popup_target: None,
            seen_key: false,
            cached_entries: None,
            sync_stale: None,
            vault_state: VaultState::Locked,
            delivery: None,
            indicator: None,
            settings_window: None,
            about_window: None,
        });

        unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                TICK_INTERVAL_SECS,
                self,
                sel!(tick:),
                None,
                true,
            );
        }
    }

    fn mtm(&self) -> MainThreadMarker {
        MainThreadMarker::from(self)
    }

    /// The one place everything converges — drains `rx`, advances the
    /// delivery and indicator machines, checks blur/exit, and refreshes
    /// whatever the panel is currently showing. The macOS counterpart of
    /// `App::logic` + `App::ui` combined into one poll instead of a
    /// frame callback pair.
    fn on_tick(&self) {
        let mut guard = self.ivars().state.borrow_mut();
        let Some(state) = guard.as_mut() else { return };

        while let Ok(msg) = state.rx.try_recv() {
            match msg {
                Msg::Tray(TrayCmd::Quit) => {
                    platform::indicator::hide();
                    if state.cfg.lock_on_exit && state.vault_state == VaultState::Unlocked {
                        // Best-effort, fire-and-forget: unlike Windows,
                        // this doesn't wait for `bw lock` to finish before
                        // quitting — a deliberate v1 simplification (the
                        // wait needs `applicationShouldTerminate:` plus
                        // `NSApplicationTerminateReply::TerminateLater`,
                        // deferred along with the rest of Settings'
                        // polish). The lock request is still sent.
                        let _ = state.bw_cmd_tx.send(BwCmd::Lock);
                    }
                    drop(guard);
                    let mtm = self.mtm();
                    NSApplication::sharedApplication(mtm).terminate(None);
                    return;
                }
                Msg::Tray(TrayCmd::Show) | Msg::ShowPopup => open_popup(state, None),
                Msg::Tray(TrayCmd::Sync) => handle_sync(state),
                Msg::Tray(TrayCmd::Lock) => handle_lock(state),
                Msg::Tray(TrayCmd::Settings) => self.open_settings(state),
                Msg::Tray(TrayCmd::About) => self.open_about(state),
                Msg::Hotkey(target) => open_popup(state, Some(target)),
                Msg::Bw(result) => handle_bw_result(state, result),
            }
        }

        advance_delivery(state);
        advance_indicator(state);
        check_blur(state);
        // Deliberately *not* an unconditional `refresh_panel(state)` here
        // — that was a real bug found in manual testing. `Panel::layout`
        // starts by hiding every control (`setHidden(true)`) before
        // conditionally re-showing whichever ones the current content
        // needs, and AppKit resigns first-responder status the instant a
        // view holding it is hidden — it does not restore automatically
        // when the view is un-hidden a moment later in the same call.
        // Calling `layout()` every ~16ms tick was silently kicking the
        // master-password field's focus back to the panel itself on the
        // very next tick after `show_popup` had just (successfully)
        // focused it — confirmed with a temporary diagnostic:
        // `makeFirstResponder` returned `true` right after showing the
        // prompt, but the panel's `firstResponder` had already reverted
        // to itself one tick later. `list_key_catcher` isn't part of that
        // reset list, which is why list keyboard navigation was never
        // affected — only the password prompt was. The fix: call
        // `layout()` only where something actually needs to visually
        // change (`show_popup` on content transitions, `refresh_list`
        // after a selection move), never as a blind per-tick poll.
    }

    fn handle_row_clicked(&self, index: usize) {
        let mut guard = self.ivars().state.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        let PopupContent::ShowingList { .. } = &state.popup_content else { return };
        start_delivery(state, index, DeliveryKind::Password);
    }

    /// Shared by the password field and the list's invisible key-catcher
    /// (see `Panel::list_key_catcher`'s doc) — which one fired is
    /// irrelevant; what matters is the *current* `PopupContent`.
    fn handle_command(&self, command: Sel) -> bool {
        let mut guard = self.ivars().state.borrow_mut();
        let Some(state) = guard.as_mut() else { return false };

        let is_enter = command == sel!(insertNewline:);
        let is_escape = command == sel!(cancelOperation:);
        let is_up = command == sel!(moveUp:);
        let is_down = command == sel!(moveDown:);
        // Set inside the match below, applied after it — `layout()` must
        // never run while `state.popup_content` is still borrowed by the
        // match, and (unlike `show_popup`) must not touch focus at all:
        // `list_key_catcher` stays first responder throughout, only the
        // row highlight/`message` text need to change.
        let mut needs_list_refresh = false;

        let handled = match &mut state.popup_content {
            PopupContent::Prompting { error } => {
                if is_enter {
                    let password = state.panel.take_password();
                    if !password.is_empty() {
                        *error = None;
                        let _ = state.bw_cmd_tx.send(BwCmd::Unlock(Secret::new(password)));
                        state.popup_content = PopupContent::Unlocking;
                    }
                    true
                } else if is_escape {
                    hide_popup(state);
                    true
                } else {
                    false
                }
            }
            PopupContent::Unlocking | PopupContent::Locking | PopupContent::Syncing => {
                if is_escape {
                    hide_popup(state);
                    true
                } else {
                    false
                }
            }
            PopupContent::ShowingList { selected, message } => {
                let count = state
                    .cached_entries
                    .as_ref()
                    .map_or(0, |(e, _)| e.len().min(super::panel::MAX_ROWS));
                if count == 0 {
                    return is_escape && {
                        drop(guard);
                        self.hide_popup();
                        true
                    };
                }
                if is_up {
                    *selected = if *selected == 0 { count - 1 } else { *selected - 1 };
                    *message = None;
                    needs_list_refresh = true;
                    true
                } else if is_down {
                    *selected = (*selected + 1) % count;
                    *message = None;
                    needs_list_refresh = true;
                    true
                } else if is_enter {
                    let index = *selected;
                    let mtm = self.mtm();
                    let flags = NSApplication::sharedApplication(mtm)
                        .currentEvent()
                        .map(|e| e.modifierFlags())
                        .unwrap_or(NSEventModifierFlags::empty());
                    let kind = DeliveryKind::from_flags(
                        flags.contains(NSEventModifierFlags::Shift),
                        flags.contains(NSEventModifierFlags::Option),
                    );
                    start_delivery(state, index, kind);
                    true
                } else if is_escape {
                    hide_popup(state);
                    true
                } else {
                    false
                }
            }
            PopupContent::FetchingOtp { selected } => {
                if is_escape {
                    let selected = *selected;
                    state.popup_content = PopupContent::ShowingList { selected, message: None };
                    true
                } else {
                    false
                }
            }
        };

        if needs_list_refresh {
            refresh_list(state);
        }
        handled
    }

    fn hide_popup(&self) {
        let mut guard = self.ivars().state.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        hide_popup(state);
    }

    fn open_settings(&self, state: &mut AppState) {
        if state.delivery.is_some() {
            return;
        }
        hide_popup(state);
        let mtm = self.mtm();
        let window = state
            .settings_window
            .get_or_insert_with(|| SettingsWindow::new(mtm, ProtocolObject::from_ref(self)));
        window.show(&state.cfg, &state.hotkey, state.vault_state);
    }

    fn open_about(&self, state: &mut AppState) {
        if state.delivery.is_some() {
            return;
        }
        hide_popup(state);
        let mtm = self.mtm();
        let window = state.about_window.get_or_insert_with(|| about::build(mtm));
        window.makeKeyAndOrderFront(None);
    }

    /// The Save button's handler — mirrors `App::try_apply_config_window`.
    fn handle_settings_save(&self) {
        let mut guard = self.ivars().state.borrow_mut();
        let Some(state) = guard.as_mut() else { return };
        let Some(window) = state.settings_window.take() else { return };
        let draft = window.draft();
        match apply_settings(state, &draft) {
            Ok(()) => window.hide(),
            Err(e) => window.show_error(&e),
        }
        state.settings_window = Some(window);
    }

}

fn open_popup(state: &mut AppState, target: Option<Target>) {
    if state.popup_visible || state.delivery.is_some() {
        return;
    }
    state.popup_target = target;
    state.popup_content = match (&state.cached_entries, &state.popup_content) {
        (_, PopupContent::Syncing) => PopupContent::Syncing,
        (Some(_), _) => PopupContent::ShowingList { selected: 0, message: None },
        (None, PopupContent::Unlocking) => PopupContent::Unlocking,
        (None, PopupContent::Locking) => PopupContent::Locking,
        (None, _) => PopupContent::fresh_prompt(),
    };
    show_popup(state);
}

fn show_popup(state: &mut AppState) {
    let (w, h) = state.panel.layout(
        &state.popup_content,
        &row_labels(state),
        dropped_count(state),
        hidden_by_cap_count(state),
        stale_text(state).as_deref(),
        blocked_text(state).as_deref(),
    );
    let cursor = state.popup_target.map(|t| t.cursor());
    let placement = platform::monitor::placement_for(cursor, w.round() as i32, h.round() as i32);
    state.panel.show_at(placement.x, placement.y, placement.w, placement.h);
    // Found in manual testing: `.nonactivatingPanel` alone does not
    // actually route hardware keyboard events to the panel for a regular
    // (non-privileged) app — only specially-entitled system UI (Spotlight
    // and the like) gets that for free. A normal app has to briefly
    // become the active app while its panel is up, exactly like
    // `platform::focus::activate_target`'s Windows counterpart
    // (`SetForegroundWindow`) already does for restoring the *previous*
    // app afterward — this is that same idea applied to *ourselves* first.
    // `.accessory` activation policy means this never adds a Dock icon or
    // shows up as a persistent app switch, matching how Alfred/Raycast-
    // style launchers behave.
    //
    // `activateIgnoringOtherApps(true)`, not the modern no-arg
    // `activate()` — confirmed by manual testing (a real bug: the panel
    // correctly became key a moment later, but `makeFirstResponder` right
    // after showing it still silently failed) plus `objc2`'s own
    // `hello_world_app` example, which uses this exact deprecated call
    // with the comment "Required when launching unbundled (as is done
    // with Cargo)" — this app is exactly that case today. `activate()` is
    // documented to be able to defer taking effect; the deprecated call
    // is synchronous, which `makeFirstResponder` immediately afterward
    // depends on.
    let mtm = MainThreadMarker::new().expect("show_popup must run on the main thread");
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    match &state.popup_content {
        PopupContent::Prompting { .. } => state.panel.focus_prompt(),
        _ => state.panel.focus_list_catcher(),
    }
    state.popup_visible = true;
    state.seen_key = false;
}

fn hide_popup(state: &mut AppState) {
    state.panel.hide();
    if let Some(target) = &state.popup_target {
        focus::activate_target(target);
    }
    state.popup_visible = false;
    state.seen_key = false;
    hide_indicator(state);
    if matches!(state.popup_content, PopupContent::Prompting { .. }) {
        state.popup_content = PopupContent::fresh_prompt();
    }
}

fn check_blur(state: &mut AppState) {
    if !state.popup_visible {
        return;
    }
    let key = state.panel.is_key();
    if key {
        state.seen_key = true;
        return;
    }
    if !state.seen_key {
        return;
    }
    // Dismiss-on-blur is suspended for the busy states, same reasoning as
    // Windows: an unlock/lock/sync that vanishes on a stray click would be
    // maddening.
    if matches!(
        state.popup_content,
        PopupContent::Unlocking | PopupContent::Locking | PopupContent::Syncing
    ) {
        return;
    }
    hide_popup(state);
}

fn handle_lock(state: &mut AppState) {
    eprintln!("lock: clearing cached items and locking the vault");
    state.cached_entries = None;
    state.sync_stale = None;
    if state.hotkey_suspended {
        state.hotkey.resume();
        state.hotkey_suspended = false;
    }
    state.popup_content = PopupContent::Locking;
    set_vault_state(state, VaultState::Locking);
    let _ = state.bw_cmd_tx.send(BwCmd::Lock);
}

fn handle_sync(state: &mut AppState) {
    if state.vault_state != VaultState::Unlocked || matches!(state.popup_content, PopupContent::Syncing) {
        return;
    }
    eprintln!("sync: refreshing cached items");
    if !state.popup_visible || matches!(state.popup_content, PopupContent::ShowingList { .. }) {
        state.popup_content = PopupContent::Syncing;
    }
    let _ = state.bw_cmd_tx.send(BwCmd::Sync);
}

fn set_vault_state(state: &mut AppState, new: VaultState) {
    state.vault_state = new;
    state.tray.set_show_label(new == VaultState::Unlocked);
    state.tray.set_unlocked_items_visible(new == VaultState::Unlocked);
}

fn handle_bw_result(state: &mut AppState, result: BwResult) {
    match result {
        BwResult::Items { entries, dropped, stale } => {
            if state.vault_state == VaultState::Locking {
                if state.cfg.debug_log {
                    eprintln!("bw: dropping stale sync result — a lock is in flight");
                }
                return;
            }
            if state.cfg.debug_log {
                eprintln!("bw: unlocked, {} item(s) matched, {dropped} dropped", entries.len());
                for entry in &entries {
                    eprintln!("  {}", entry.log_line());
                }
                if let Some(notice) = &stale {
                    eprintln!(
                        "bw: sync did not succeed ({}): {} — showing the cached list anyway",
                        notice.reason.summary(),
                        notice.detail
                    );
                }
            }
            state.cached_entries = Some((entries, dropped));
            state.sync_stale = stale;
            set_vault_state(state, VaultState::Unlocked);
            if matches!(state.popup_content, PopupContent::Unlocking | PopupContent::Syncing) {
                state.popup_content = PopupContent::ShowingList { selected: 0, message: None };
                if state.popup_visible {
                    show_popup(state);
                }
            }
        }
        BwResult::Failed { stage, kind, message } => {
            eprintln!("bw: failed at {stage}: {message}");
            if stage == "sync" {
                state.sync_stale = Some(StaleNotice { reason: kind, last_sync: None, detail: message.clone() });
            }
            match &state.popup_content {
                PopupContent::Unlocking => {
                    state.popup_content = PopupContent::Prompting { error: Some(message) };
                }
                PopupContent::Syncing => {
                    state.popup_content = if state.cached_entries.is_some() {
                        PopupContent::ShowingList { selected: 0, message: Some(format!("{stage}: {message}")) }
                    } else {
                        PopupContent::fresh_prompt()
                    };
                    if state.popup_visible {
                        show_popup(state);
                    }
                }
                PopupContent::FetchingOtp { selected } => {
                    let selected = *selected;
                    state.popup_content =
                        PopupContent::ShowingList { selected, message: Some(format!("{stage}: {message}")) };
                }
                _ => {}
            }
        }
        BwResult::Locked => {
            if state.cfg.debug_log {
                eprintln!("bw: lock finished");
            }
            set_vault_state(state, VaultState::Locked);
            if matches!(state.popup_content, PopupContent::Locking) {
                state.popup_content = PopupContent::fresh_prompt();
            }
        }
        BwResult::Totp(code) => {
            let PopupContent::FetchingOtp { selected } = state.popup_content else { return };
            let Some(target) = state.popup_target else {
                state.popup_content = PopupContent::ShowingList { selected, message: Some("no delivery target".to_string()) };
                return;
            };
            let log_line = state
                .cached_entries
                .as_ref()
                .and_then(|(entries, _)| entries.get(selected))
                .map(Entry::log_line)
                .unwrap_or_default();
            state.popup_content = PopupContent::ShowingList { selected, message: None };
            begin_delivery(state, target, code, &log_line);
        }
    }
}

fn start_delivery(state: &mut AppState, selected: usize, kind: DeliveryKind) {
    let Some((entries, _)) = &state.cached_entries else { return };
    let Some(entry) = entries.get(selected) else { return };
    let Some(target) = state.popup_target else { return };

    match kind {
        DeliveryKind::Password => {
            let secret = entry.password.clone_secret();
            let log_line = entry.log_line();
            begin_delivery(state, target, secret, &log_line);
        }
        DeliveryKind::Username => {
            let Some(username) = entry.username.clone() else {
                set_list_message(state, selected, "This item has no username.".to_string());
                return;
            };
            let log_line = entry.log_line();
            begin_delivery(state, target, Secret::new(username), &log_line);
        }
        DeliveryKind::Otp => {
            if !entry.has_totp {
                set_list_message(state, selected, "This item has no TOTP configured.".to_string());
                return;
            }
            let item_id = entry.id.clone();
            state.popup_content = PopupContent::FetchingOtp { selected };
            let _ = state.bw_cmd_tx.send(BwCmd::GetTotp(item_id));
        }
    }
}

fn set_list_message(state: &mut AppState, selected: usize, message: String) {
    state.popup_content = PopupContent::ShowingList { selected, message: Some(message) };
    refresh_list(state);
}

fn begin_delivery(state: &mut AppState, target: Target, secret: Secret, item_log_line: &str) {
    eprintln!("delivery: starting for target {target:?}");
    if state.cfg.debug_log {
        eprintln!("delivery: item {item_log_line}");
    }
    state.delivery = Some(Delivery { target, secret, phase: DeliveryPhase::HidePopup, deadline: Instant::now() });
    state.panel.hide();
    state.popup_visible = false;
    state.seen_key = false;
    show_indicator(state);
}

fn advance_delivery(state: &mut AppState) {
    let Some(delivery) = state.delivery.as_mut() else { return };
    let mut done = false;
    let mut typed = false;

    match delivery.phase {
        DeliveryPhase::HidePopup => {
            delivery.phase = DeliveryPhase::DrainModifiers;
            delivery.deadline = Instant::now() + MODIFIER_DRAIN_TIMEOUT;
        }
        DeliveryPhase::DrainModifiers => {
            if platform::typing::modifiers_up() || Instant::now() >= delivery.deadline {
                delivery.phase = DeliveryPhase::Activate;
            }
        }
        DeliveryPhase::Activate => {
            if !delivery.target.still_valid() {
                eprintln!("delivery aborted: target app no longer exists");
                done = true;
            } else {
                focus::activate_target(&delivery.target);
                delivery.phase = DeliveryPhase::VerifyForeground;
                delivery.deadline = Instant::now() + FOREGROUND_VERIFY_TIMEOUT;
            }
        }
        DeliveryPhase::VerifyForeground => {
            if delivery.target.is_foreground() {
                delivery.phase = DeliveryPhase::Settle;
                delivery.deadline = Instant::now() + Duration::from_millis(u64::from(state.cfg.type_settle_ms));
            } else if Instant::now() >= delivery.deadline {
                eprintln!("delivery aborted: could not restore foreground to target");
                done = true;
            }
        }
        DeliveryPhase::Settle => {
            if Instant::now() >= delivery.deadline {
                if let Some(reason) = delivery.target.blocked() {
                    eprintln!("delivery aborted: target can't receive synthetic keystrokes ({reason:?})");
                } else {
                    let skipped = platform::typing::send_unicode(delivery.secret.expose());
                    eprintln!("delivery: typed the selected password ({skipped} code unit(s) skipped)");
                    typed = true;
                }
                done = true;
            }
        }
    }

    if done {
        state.delivery = None;
        if typed {
            if let Some(IndicatorPhase::Typing { typed, .. }) = state.indicator.as_mut() {
                *typed = true;
            }
        } else {
            hide_indicator(state);
        }
    }
}

fn show_indicator(state: &mut AppState) {
    let cursor = focus::cursor_pos();
    let placement = platform::monitor::placement_for(Some(cursor), INDICATOR_SIZE_PT, INDICATOR_SIZE_PT);
    platform::indicator::show(platform::indicator::Kind::Typing, placement.x, placement.y, placement.w);
    state.indicator = Some(IndicatorPhase::Typing { since: Instant::now(), typed: false });
}

fn hide_indicator(state: &mut AppState) {
    state.indicator = None;
    platform::indicator::hide();
}

fn advance_indicator(state: &mut AppState) {
    let Some(phase) = state.indicator else { return };
    match crate::controller::next_indicator_phase(phase, Instant::now()) {
        Some(next) => {
            if matches!(phase, IndicatorPhase::Typing { .. }) && matches!(next, IndicatorPhase::Done { .. }) {
                platform::indicator::set_kind(platform::indicator::Kind::Done);
            }
            state.indicator = Some(next);
        }
        None => hide_indicator(state),
    }
}

/// Re-renders the list's rows/message/banners in place — deliberately
/// **not** called on a timer (see `on_tick`'s doc for why that broke
/// keyboard focus). Safe to call while `list_key_catcher` is first
/// responder specifically because `Panel::layout` never touches that
/// field's hidden state; it would not be safe to call this on a path
/// where `prompt_field` is focused.
fn refresh_list(state: &mut AppState) {
    if !state.popup_visible {
        return;
    }
    let (w, h) = state.panel.layout(
        &state.popup_content,
        &row_labels(state),
        dropped_count(state),
        hidden_by_cap_count(state),
        stale_text(state).as_deref(),
        blocked_text(state).as_deref(),
    );
    let _ = (w, h); // size is fixed once shown; layout() still needs to run to update control content/visibility
}

fn row_labels(state: &AppState) -> Vec<String> {
    let Some((entries, _)) = &state.cached_entries else { return Vec::new() };
    let max = state.cfg.effective_max_visible().min(super::panel::MAX_ROWS);
    entries
        .iter()
        .take(max)
        .enumerate()
        .map(|(i, e)| {
            let badge = if i == 9 { '0' } else { char::from(b'1' + i as u8) };
            match &e.username {
                Some(u) => format!("{badge}. {} ({u})", e.name),
                None => format!("{badge}. {}", e.name),
            }
        })
        .collect()
}

fn dropped_count(state: &AppState) -> usize {
    state.cached_entries.as_ref().map_or(0, |(_, d)| *d)
}

fn hidden_by_cap_count(state: &AppState) -> usize {
    state.cached_entries.as_ref().map_or(0, |(entries, _)| {
        entries.len().saturating_sub(state.cfg.effective_max_visible())
    })
}

fn stale_text(state: &AppState) -> Option<String> {
    if !matches!(state.popup_content, PopupContent::ShowingList { .. }) {
        return None;
    }
    state.sync_stale.as_ref().map(|notice| match &notice.last_sync {
        Some(ts) => format!("Offline — showing the cached vault (last synced {ts})."),
        None => "Offline — showing the cached vault, which may be stale.".to_string(),
    })
}

fn blocked_text(state: &AppState) -> Option<String> {
    if !matches!(state.popup_content, PopupContent::ShowingList { .. }) {
        return None;
    }
    let reason = state.popup_target.and_then(|t| t.blocked())?;
    Some(
        match reason {
            BlockReason::Elevated => "Target runs elevated — typing would be blocked.",
            BlockReason::NoAccessibility => {
                "Accessibility permission not granted — typing would be blocked."
            }
            BlockReason::SecureInput => {
                "Target field has secure input active — typing would be blocked."
            }
        }
        .to_string(),
    )
}

fn apply_settings(state: &mut AppState, draft: &settings::Draft) -> Result<(), String> {
    let mut error = None;

    if draft.hotkey_spec != state.cfg.hotkey {
        match state.hotkey.set(&draft.hotkey_spec) {
            Ok(()) => {
                eprintln!("settings: hotkey changed to {}", draft.hotkey_spec);
                state.cfg.hotkey.clone_from(&draft.hotkey_spec);
            }
            Err(e) => error = Some(format!("hotkey: {e}")),
        }
    }

    if draft.autostart != state.cfg.autostart {
        match platform::autostart::set_enabled(draft.autostart) {
            Ok(()) => {
                eprintln!("settings: autostart {}", if draft.autostart { "enabled" } else { "disabled" });
                state.cfg.autostart = draft.autostart;
            }
            Err(e) => {
                error.get_or_insert(format!("autostart: {e}"));
            }
        }
    }

    let new_unlock_mode = if draft.auto_unlock { UnlockMode::Delayed } else { UnlockMode::Lazy };
    if new_unlock_mode != state.cfg.unlock_mode {
        eprintln!("settings: auto-unlock at start {}", draft.auto_unlock);
        state.cfg.unlock_mode = new_unlock_mode;
    }
    if draft.lock_on_exit != state.cfg.lock_on_exit {
        eprintln!("settings: lock on exit {}", if draft.lock_on_exit { "enabled" } else { "disabled" });
        state.cfg.lock_on_exit = draft.lock_on_exit;
    }
    let clamped = draft
        .max_visible_items
        .clamp(crate::config::MIN_VISIBLE_ITEMS, crate::config::MAX_VISIBLE_ITEMS);
    if clamped != state.cfg.max_visible_items {
        eprintln!("settings: max visible items changed to {clamped}");
        state.cfg.max_visible_items = clamped;
    }

    if let Err(e) = state.cfg.save() {
        error.get_or_insert(format!("saving config: {e}"));
    }

    error.map_or(Ok(()), Err)
}

/// Entry point called from `mac_ui::run`.
pub fn run() {
    let mtm = MainThreadMarker::new().expect("mac_ui::run must be called from the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
