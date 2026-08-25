//! The `bw` worker thread.
//!
//! One long-lived thread plus one `mpsc` channel in, one out (plan §7) — no
//! async runtime. Every `bw` call is a blocking `Command::output()` on a
//! Node process; the calls are strictly sequential (unlock → sync → list)
//! with exactly one ever in flight, so an async runtime would buy nothing
//! but a bigger binary.
//!
//! The session key lives only in this thread's local — it never crosses
//! back to the UI thread. The UI only ever learns `Items` or `Failed`.

pub mod cmd;
pub mod exe;
pub mod filter;
pub mod model;

use std::sync::mpsc::{Receiver, Sender};

use eframe::egui;

use crate::msg::{BwCmd, BwResult, Msg};
use crate::secret::Secret;

/// Spawns the worker thread and returns a channel to send it commands.
/// Results are pushed back through `results_tx` — the app's shared `Msg`
/// channel — followed by `ctx.request_repaint()`, the same wake pattern
/// every other event source in this app uses.
pub fn spawn(
    bw_path: String,
    uri_prefix: String,
    results_tx: Sender<Msg>,
    ctx: egui::Context,
) -> Sender<BwCmd> {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<BwCmd>();
    std::thread::spawn(move || worker_loop(&bw_path, &uri_prefix, &cmd_rx, &results_tx, &ctx));
    cmd_tx
}

fn worker_loop(
    bw_path: &str,
    uri_prefix: &str,
    cmd_rx: &Receiver<BwCmd>,
    results_tx: &Sender<Msg>,
    ctx: &egui::Context,
) {
    // Blocks at zero CPU between commands; never crosses back to the UI.
    let mut session: Option<Secret> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            BwCmd::Unlock(master_password) => {
                let result =
                    unlock_sync_list(bw_path, uri_prefix, &master_password, &mut session);
                let _ = results_tx.send(Msg::Bw(result));
                ctx.request_repaint();
            }
        }
    }
}

fn unlock_sync_list(
    bw_path: &str,
    uri_prefix: &str,
    master_password: &Secret,
    session: &mut Option<Secret>,
) -> BwResult {
    let exe = match exe::resolve(bw_path) {
        Ok(exe) => exe,
        Err(message) => {
            return BwResult::Failed {
                stage: "resolve",
                message,
            };
        }
    };

    let unlock_output = match cmd::unlock(&exe, master_password.expose()) {
        Ok(o) => o,
        Err(e) => {
            return BwResult::Failed {
                stage: "unlock",
                message: e.to_string(),
            };
        }
    };
    if !unlock_output.status.success() {
        return BwResult::Failed {
            stage: "unlock",
            message: "bw unlock failed (wrong password, or the vault isn't logged in)"
                .to_string(),
        };
    }
    let mut key_bytes = unlock_output.stdout;
    let key = Secret::new(String::from_utf8_lossy(&key_bytes).trim().to_string());
    // The buffer held the cleartext session key — scrub it before it drops.
    key_bytes.fill(0);

    if let Err(e) = cmd::sync(&exe, key.expose()) {
        return BwResult::Failed {
            stage: "sync",
            message: e.to_string(),
        };
    }

    let list_output = match cmd::list_items(&exe, key.expose(), uri_prefix) {
        Ok(o) => o,
        Err(e) => {
            return BwResult::Failed {
                stage: "list",
                message: e.to_string(),
            };
        }
    };
    if !list_output.status.success() {
        return BwResult::Failed {
            stage: "list",
            message: "bw list items failed".to_string(),
        };
    }

    let raw_items: Vec<model::RawItem> = match serde_json::from_slice(&list_output.stdout) {
        Ok(items) => items,
        Err(e) => {
            return BwResult::Failed {
                stage: "parse",
                message: e.to_string(),
            };
        }
    };
    let mut stdout_buf = list_output.stdout;
    // Held cleartext item passwords — scrub before it drops.
    stdout_buf.fill(0);

    let (entries, dropped) = filter::build_entries(raw_items, uri_prefix);

    // Kept for the process's lifetime; not read again until a later
    // milestone adds a "refresh" command that can reuse it instead of
    // unlocking again (each `bw unlock` invalidates the previous session).
    *session = Some(key);

    BwResult::Items { entries, dropped }
}
