//! The `bw` worker thread.
//!
//! One long-lived thread plus one `mpsc` channel in, one out (plan §7) — no
//! async runtime. Every `bw` call is a blocking subprocess call (now with a
//! timeout — see `run`) on a Node process; the calls are strictly
//! sequential (unlock → sync → list) with exactly one ever in flight, so an
//! async runtime would buy nothing but a bigger binary.
//!
//! The session key lives only in this thread's local — it never crosses
//! back to the UI thread. The UI only ever learns `Items` or `Failed`.

pub mod cmd;
pub mod error;
pub mod exe;
pub mod filter;
pub mod model;
pub mod run;

use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use error::{classify, BwErrorKind};

use crate::msg::{BwCmd, BwResult, Msg, StaleNotice};
use crate::secret::Secret;

/// Wakes whatever event loop owns the UI thread after a result has been
/// pushed onto `results_tx` — `ctx.request_repaint()` on Windows/eframe, a
/// run-loop signal on macOS/AppKit. Every other event source in this app
/// (tray, hotkey, the delayed-unlock timer) follows the same "send, then
/// wake" pattern; this is what lets `bw::spawn` stay UI-toolkit-agnostic.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Spawns the worker thread and returns a channel to send it commands.
/// Results are pushed back through `results_tx` — the app's shared `Msg`
/// channel — followed by a call to `wake`.
pub fn spawn(
    bw_path: String,
    uri_prefix: String,
    results_tx: Sender<Msg>,
    wake: Waker,
) -> Sender<BwCmd> {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<BwCmd>();
    std::thread::spawn(move || worker_loop(&bw_path, &uri_prefix, &cmd_rx, &results_tx, &wake));
    cmd_tx
}

fn worker_loop(
    bw_path: &str,
    uri_prefix: &str,
    cmd_rx: &Receiver<BwCmd>,
    results_tx: &Sender<Msg>,
    wake: &Waker,
) {
    // Blocks at zero CPU between commands; never crosses back to the UI.
    let mut session: Option<Secret> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            BwCmd::Unlock(master_password) => {
                let result =
                    unlock_sync_list(bw_path, uri_prefix, &master_password, &mut session);
                let _ = results_tx.send(Msg::Bw(result));
                wake();
            }
            BwCmd::Sync => {
                let result = match &session {
                    None => BwResult::Failed {
                        stage: "sync",
                        kind: BwErrorKind::Locked,
                        message: "vault is locked".to_string(),
                    },
                    Some(key) => match exe::resolve(bw_path) {
                        Err(message) => BwResult::Failed {
                            stage: "resolve",
                            kind: BwErrorKind::NotFound,
                            message,
                        },
                        Ok(exe) => sync_then_list(&exe, uri_prefix, key),
                    },
                };
                let _ = results_tx.send(Msg::Bw(result));
                wake();
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
                    Ok(Ok(run)) if run.success() => {
                        eprintln!("bw: locked");
                    }
                    Ok(Ok(run)) => eprintln!(
                        "bw: lock command did not succeed (timed_out={}): {}",
                        run.timed_out, run.stderr
                    ),
                    Ok(Err(e)) => eprintln!("bw: failed to spawn lock: {e}"),
                    Err(e) => eprintln!("bw: failed to resolve executable for lock: {e}"),
                }
                session = None;
                let _ = results_tx.send(Msg::Bw(BwResult::Locked));
                wake();
            }
            BwCmd::GetTotp(item_id) => {
                let result = get_totp(bw_path, &item_id, &session);
                let _ = results_tx.send(Msg::Bw(result));
                wake();
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
            kind: BwErrorKind::Locked,
            message: "vault is locked".to_string(),
        };
    };
    let exe = match exe::resolve(bw_path) {
        Ok(exe) => exe,
        Err(message) => {
            return BwResult::Failed {
                stage: "totp",
                kind: BwErrorKind::NotFound,
                message,
            };
        }
    };
    let mut run = match cmd::get_totp(&exe, session.expose(), item_id) {
        Ok(r) => r,
        Err(e) => {
            return BwResult::Failed {
                stage: "totp",
                kind: BwErrorKind::Other,
                message: e.to_string(),
            };
        }
    };
    if !run.success() {
        let kind = classify(&run.stderr, run.timed_out);
        let message = if run.stderr.is_empty() {
            "bw get totp failed (does this item have a TOTP configured?)".to_string()
        } else {
            run.stderr.clone()
        };
        return BwResult::Failed {
            stage: "totp",
            kind,
            message,
        };
    }
    let code = String::from_utf8_lossy(&run.stdout).trim().to_string();
    // The buffer held the cleartext code — scrub it before it drops, same
    // as the unlock/list stdout buffers.
    run.stdout.fill(0);
    if code.is_empty() {
        return BwResult::Failed {
            stage: "totp",
            kind: BwErrorKind::Other,
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
                kind: BwErrorKind::NotFound,
                message,
            };
        }
    };

    let mut unlock_run = match cmd::unlock(&exe, master_password.expose()) {
        Ok(r) => r,
        Err(e) => {
            return BwResult::Failed {
                stage: "unlock",
                kind: BwErrorKind::Other,
                message: e.to_string(),
            };
        }
    };
    if !unlock_run.success() {
        let kind = classify(&unlock_run.stderr, unlock_run.timed_out);
        // `bw unlock` derives the key from the locally-cached vault and
        // never touches the network — so a `Network` classification here
        // means the stderr text was misread, not that unlocking genuinely
        // needs connectivity. Fall back to a status probe for a message
        // that actually matches what's wrong.
        let message = precise_unlock_message(&exe, kind, &unlock_run.stderr, unlock_run.timed_out);
        return BwResult::Failed {
            stage: "unlock",
            kind,
            message,
        };
    }
    let key = Secret::new(String::from_utf8_lossy(&unlock_run.stdout).trim().to_string());
    // The buffer held the cleartext session key — scrub it before it drops.
    unlock_run.stdout.fill(0);

    // Stored as soon as it's known good, before sync/list/parse have a
    // chance to fail — a failed sync or list must not throw away a
    // perfectly valid session, or the tray's Sync item would have nothing
    // left to reuse and the user would be forced back to re-entering their
    // master password just to retry.
    *session = Some(key);
    let key = session.as_ref().expect("just assigned above");

    sync_then_list(&exe, uri_prefix, key)
}

/// A precise message for an unlock failure, using a `bw status` probe
/// (local-only, so safe to run even when the network is down) rather than
/// trusting `unlock`'s own stderr alone — see `unlock_sync_list`'s call
/// site for why that stderr can be misleading.
fn precise_unlock_message(exe: &exe::BwExe, kind: BwErrorKind, stderr: &str, timed_out: bool) -> String {
    if timed_out {
        return "bw unlock timed out — the Bitwarden CLI may be unresponsive.".to_string();
    }
    match probe_status(exe) {
        Some(VaultStatus::Unauthenticated) => {
            "No Bitwarden account is logged in on this machine. Run `bw login` in a terminal, \
             then try again."
                .to_string()
        }
        _ => match kind {
            BwErrorKind::NotLoggedIn => {
                "No Bitwarden account is logged in on this machine. Run `bw login` in a \
                 terminal, then try again."
                    .to_string()
            }
            BwErrorKind::Network => {
                "Can't reach the Bitwarden server (network blocked?). Unlocking isn't possible \
                 right now."
                    .to_string()
            }
            BwErrorKind::BadPassword => "wrong master password".to_string(),
            _ if stderr.is_empty() => {
                "bw unlock failed (wrong password, or the vault isn't logged in)".to_string()
            }
            _ => stderr.to_string(),
        },
    }
}

enum VaultStatus {
    Unauthenticated,
    Locked,
    Unlocked,
}

/// Runs `bw status` (local-only, never touches the network) and parses its
/// `status` field. Returns `None` on any failure — a broken probe is a
/// reason to fall back to the caller's own message, not to fail harder.
fn probe_status(exe: &exe::BwExe) -> Option<VaultStatus> {
    let run = cmd::status(exe).ok()?;
    if !run.success() {
        return None;
    }
    // `bw status`'s stdout can be preceded by chatter (e.g. "Could not find
    // data file...; creating it instead." on a brand-new data dir) — find
    // the JSON object rather than assuming stdout is exactly one value.
    let text = String::from_utf8_lossy(&run.stdout);
    let start = text.find('{')?;
    let value: serde_json::Value = serde_json::from_str(&text[start..]).ok()?;
    match value.get("status")?.as_str()? {
        "unauthenticated" => Some(VaultStatus::Unauthenticated),
        "locked" => Some(VaultStatus::Locked),
        "unlocked" => Some(VaultStatus::Unlocked),
        _ => None,
    }
}

/// The `lastSync` field from a `bw status` probe, if it succeeds — for the
/// stale-vault banner's "last synced …" detail. Best-effort: `None` on any
/// failure, including a vault that has never synced on this machine.
fn probe_last_sync(exe: &exe::BwExe) -> Option<String> {
    let run = cmd::status(exe).ok()?;
    if !run.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&run.stdout);
    let start = text.find('{')?;
    let value: serde_json::Value = serde_json::from_str(&text[start..]).ok()?;
    value.get("lastSync")?.as_str().map(str::to_string)
}

/// `bw sync` + `bw list items` + parse + filter — the shared tail of both a
/// fresh unlock and the tray's Sync item (`BwCmd::Sync`), which reuses the
/// session the worker already holds instead of unlocking again.
fn sync_then_list(exe: &exe::BwExe, uri_prefix: &str, key: &Secret) -> BwResult {
    // A non-zero exit (or a timeout) is only recorded as a `StaleNotice`,
    // not treated as failure: an offline or otherwise failing `bw sync`
    // shouldn't cost the user their (still perfectly usable) local list —
    // this is the graceful-degradation requirement. A *spawn* error, by
    // contrast, likely means `bw` itself is broken and the list call would
    // fail too — reported as `Failed` rather than silently skipped.
    let stale = match cmd::sync(exe, key.expose()) {
        Ok(run) if run.success() => None,
        Ok(run) => {
            let kind = classify(&run.stderr, run.timed_out);
            eprintln!(
                "bw: sync did not succeed (timed_out={}, kind={kind:?}): {}",
                run.timed_out, run.stderr
            );
            let detail = if run.timed_out {
                "bw sync timed out".to_string()
            } else if run.stderr.is_empty() {
                "bw sync failed".to_string()
            } else {
                run.stderr
            };
            Some(StaleNotice {
                reason: kind,
                last_sync: probe_last_sync(exe),
                detail,
            })
        }
        Err(e) => {
            return BwResult::Failed {
                stage: "sync",
                kind: BwErrorKind::Other,
                message: e.to_string(),
            };
        }
    };

    let list_run = match cmd::list_items(exe, key.expose(), uri_prefix) {
        Ok(r) => r,
        Err(e) => {
            return BwResult::Failed {
                stage: "list",
                kind: BwErrorKind::Other,
                message: e.to_string(),
            };
        }
    };
    if !list_run.success() {
        let kind = classify(&list_run.stderr, list_run.timed_out);
        let message = if list_run.stderr.is_empty() {
            "bw list items failed".to_string()
        } else {
            list_run.stderr
        };
        return BwResult::Failed {
            stage: "list",
            kind,
            message,
        };
    }

    let raw_items: Vec<model::RawItem> = match serde_json::from_slice(&list_run.stdout) {
        Ok(items) => items,
        Err(e) => {
            return BwResult::Failed {
                stage: "parse",
                kind: BwErrorKind::Other,
                message: e.to_string(),
            };
        }
    };
    let mut stdout_buf = list_run.stdout;
    // Held cleartext item passwords — scrub before it drops.
    stdout_buf.fill(0);

    let (entries, dropped) =
        filter::build_entries(raw_items, uri_prefix, crate::platform::OS_TAG);

    BwResult::Items {
        entries,
        dropped,
        stale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the real graceful-degradation path end to end against the
    /// actual `bw` binary on this machine — the "not unlocked, server
    /// unreachable" case the user's self-hosted-Vaultwarden-behind-an-
    /// intermittently-blocked-domain scenario needs to fail *clearly*
    /// rather than hang or crash. Runs against an isolated
    /// `BITWARDENCLI_APPDATA_DIR` (never touches a real vault) pointed at a
    /// server that can't resolve. `#[ignore]`d — it depends on `bw` being
    /// installed and on this machine's DNS actually failing to resolve the
    /// bogus hostname (fast, unlike a firewall blackhole — that path is
    /// covered instead, deterministically, by
    /// `run::tests::kills_and_reports_timeout_on_a_hung_process`).
    #[test]
    #[ignore = "depends on bw being installed on this machine's PATH and on real DNS resolution failing"]
    fn unlock_against_an_unreachable_server_fails_clearly_not_silently() {
        let scratch = std::env::temp_dir().join(format!(
            "context-password-bw-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&scratch).expect("create scratch dir");
        // SAFETY: this test is `#[ignore]`d and meant to be run in
        // isolation (`cargo test -- --ignored --test-threads=1`) precisely
        // because it mutates process-global environment that every `bw`
        // subprocess this test spawns inherits.
        unsafe {
            std::env::set_var("BITWARDENCLI_APPDATA_DIR", &scratch);
        }

        let exe = exe::resolve("").expect("bw should be discoverable on PATH");
        let config_ok = std::process::Command::new(exe.path())
            .args(["config", "server", "https://vault.invalid.example.test", "--nointeraction"])
            .env("BITWARDENCLI_APPDATA_DIR", &scratch)
            .output()
            .map(|o| o.status.success());
        assert_eq!(config_ok.ok(), Some(true), "bw config server should succeed locally");

        let mut session = None;
        let result = unlock_sync_list(
            "",
            "app://context-password",
            &Secret::new("irrelevant-password".to_string()),
            &mut session,
        );

        let _ = std::fs::remove_dir_all(&scratch);

        match result {
            BwResult::Failed { stage, kind, message } => {
                assert_eq!(stage, "unlock");
                // Unauthenticated (never logged in) beats "network" here —
                // `bw` refuses before ever touching the server, and that's
                // exactly the more-specific, more-useful message.
                assert_eq!(kind, BwErrorKind::NotLoggedIn, "message was: {message}");
                assert!(
                    message.contains("bw login"),
                    "message should point at the fix, got: {message}"
                );
            }
            BwResult::Items { .. } => panic!("expected Failed{{stage: \"unlock\", ..}}, got Items"),
            BwResult::Locked => panic!("expected Failed{{stage: \"unlock\", ..}}, got Locked"),
            BwResult::Totp(_) => panic!("expected Failed{{stage: \"unlock\", ..}}, got Totp"),
        }
    }
}
