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
            BwCmd::Sync => {
                let result = match &session {
                    None => BwResult::Failed {
                        stage: "sync",
                        message: "vault is locked".to_string(),
                    },
                    Some(key) => match exe::resolve(bw_path) {
                        Err(message) => BwResult::Failed {
                            stage: "resolve",
                            message,
                        },
                        Ok(exe) => sync_then_list(&exe, uri_prefix, key),
                    },
                };
                let _ = results_tx.send(Msg::Bw(result));
                ctx.request_repaint();
            }
            BwCmd::Lock => {
                // Best-effort and silent either way: the worker's own
                // session field is dropped (and zeroized) regardless of
                // whether the CLI call itself succeeds — the user's intent
                // is "we don't have a vault open anymore," not "only if the
                // CLI agrees." The UI still needs to know it's *done*
                // though (§2/§7 of the plan both wait on this), hence
                // `BwResult::Locked` regardless of which branch ran.
                match exe::resolve(bw_path).map(|exe| cmd::lock(&exe)) {
                    Ok(Ok(output)) if output.status.success() => {
                        eprintln!("bw: locked");
                    }
                    Ok(Ok(_)) => eprintln!("bw: lock command returned a non-success exit code"),
                    Ok(Err(e)) => eprintln!("bw: failed to spawn lock: {e}"),
                    Err(e) => eprintln!("bw: failed to resolve executable for lock: {e}"),
                }
                session = None;
                let _ = results_tx.send(Msg::Bw(BwResult::Locked));
                ctx.request_repaint();
            }
            BwCmd::GetTotp(item_id) => {
                let result = get_totp(bw_path, &item_id, &session);
                let _ = results_tx.send(Msg::Bw(result));
                ctx.request_repaint();
            }
        }
    }
}

fn get_totp(bw_path: &str, item_id: &str, session: &Option<Secret>) -> BwResult {
    let Some(session) = session else {
        // Shouldn't normally be reachable — the item list this id came from
        // only exists after a successful unlock — but handled rather than
        // unwrapped in case a lock races a still-open popup.
        return BwResult::Failed {
            stage: "totp",
            message: "vault is locked".to_string(),
        };
    };
    let exe = match exe::resolve(bw_path) {
        Ok(exe) => exe,
        Err(message) => {
            return BwResult::Failed {
                stage: "totp",
                message,
            };
        }
    };
    let mut output = match cmd::get_totp(&exe, session.expose(), item_id) {
        Ok(o) => o,
        Err(e) => {
            return BwResult::Failed {
                stage: "totp",
                message: e.to_string(),
            };
        }
    };
    if !output.status.success() {
        return BwResult::Failed {
            stage: "totp",
            message: "bw get totp failed (does this item have a TOTP configured?)".to_string(),
        };
    }
    let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // The buffer held the cleartext code — scrub it before it drops, same
    // as the unlock/list stdout buffers.
    output.stdout.fill(0);
    if code.is_empty() {
        return BwResult::Failed {
            stage: "totp",
            message: "bw get totp returned an empty code".to_string(),
        };
    }
    BwResult::Totp(Secret::new(code))
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

    // Stored as soon as it's known good, before sync/list/parse have a
    // chance to fail — a failed sync or list must not throw away a
    // perfectly valid session, or the tray's Sync item would have nothing
    // left to reuse and the user would be forced back to re-entering their
    // master password just to retry.
    *session = Some(key);
    let key = session.as_ref().expect("just assigned above");

    sync_then_list(&exe, uri_prefix, key)
}

/// `bw sync` + `bw list items` + parse + filter — the shared tail of both a
/// fresh unlock and the tray's Sync item (`BwCmd::Sync`), which reuses the
/// session the worker already holds instead of unlocking again.
fn sync_then_list(exe: &exe::BwExe, uri_prefix: &str, key: &Secret) -> BwResult {
    // A non-zero exit is only logged, not treated as failure: an offline or
    // otherwise failing `bw sync` shouldn't cost the user their (still
    // perfectly usable) local list. A spawn error, by contrast, likely means
    // `bw` itself is broken and the list call would fail too — reported as
    // `Failed` rather than silently skipped.
    match cmd::sync(exe, key.expose()) {
        Ok(output) if !output.status.success() => {
            eprintln!("bw: sync command returned a non-success exit code");
        }
        Ok(_) => {}
        Err(e) => {
            return BwResult::Failed {
                stage: "sync",
                message: e.to_string(),
            };
        }
    }

    let list_output = match cmd::list_items(exe, key.expose(), uri_prefix) {
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

    let (entries, dropped) =
        filter::build_entries(raw_items, uri_prefix, crate::platform::OS_TAG);

    BwResult::Items { entries, dropped }
}
