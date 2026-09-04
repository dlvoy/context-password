//! The KeePass worker thread — the KeePass (KDBX) `VaultCmd`/`VaultResult`
//! backend, structurally parallel to `bw::mod` but fully local: no
//! subprocess, no network, no session key. `unlock` derives a
//! `CompositeKey` (an Argon2id/AES-KDF round, up to ~1-2s on a real
//! KeePassXC-written database) and keeps the resulting `Kdbx<Unlocked>`
//! resident in this thread for the rest of the session — `List` then
//! re-enumerates that already-decrypted structure with no further I/O or
//! KDF, and `Sync` (the tray's Sync item, repurposed here to mean "reload
//! from disk") re-derives against the file's current bytes using the
//! retained key, which is the only way to pick up an edit made by another
//! KeePass client.
//!
//! The `CompositeKey` — like `bw`'s session key — lives only in this
//! thread's locals and never crosses back to the UI thread.

pub mod error;
pub mod filter;
pub mod model;
pub mod totp;

use std::sync::mpsc::{Receiver, Sender};

use keepass_core::kdbx::{Kdbx, Unlocked};
use keepass_core::secret::CompositeKey;

use crate::msg::{Msg, VaultCmd, VaultResult};
use crate::secret::Secret;
use crate::vault::{Provider, VaultErrorKind, Waker};

const THIS_PROVIDER: Provider = Provider::KeePass;

/// Spawns the worker thread and returns a channel to send it commands —
/// same contract as `bw::spawn`. `generation` is stamped onto every
/// `Msg::Vault` this worker ever sends; see `Msg::Vault`'s doc.
pub fn spawn(
    db_path: String,
    uri_prefix: String,
    generation: u64,
    results_tx: Sender<Msg>,
    wake: Waker,
) -> Sender<VaultCmd> {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<VaultCmd>();
    std::thread::spawn(move || {
        worker_loop(&db_path, &uri_prefix, generation, &cmd_rx, &results_tx, &wake);
    });
    cmd_tx
}

fn worker_loop(
    db_path: &str,
    uri_prefix: &str,
    generation: u64,
    cmd_rx: &Receiver<VaultCmd>,
    results_tx: &Sender<Msg>,
    wake: &Waker,
) {
    // Both dropped (and zeroized, for `key`) the moment the worker exits —
    // teardown for a provider switch is just letting the last `Sender` drop
    // (see `vault::handle::VaultHandle`, Phase 4).
    let mut vault: Option<Kdbx<Unlocked>> = None;
    let mut key: Option<CompositeKey> = None;
    let send = |result: VaultResult| {
        let _ = results_tx.send(Msg::Vault { generation, result });
    };

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            VaultCmd::Unlock(master_password) => {
                let result = unlock_and_list(db_path, uri_prefix, &master_password, &mut vault, &mut key);
                send(result);
                wake();
            }
            VaultCmd::Sync => {
                // "Reload from disk" — see the module doc. Requires the
                // retained key from a prior unlock; there is no re-prompt
                // path for this action.
                let result = match &key {
                    None => VaultResult::Failed {
                        stage: "sync",
                        kind: VaultErrorKind::Locked,
                        message: "vault is locked".to_string(),
                    },
                    Some(k) => match open_and_unlock(db_path, k) {
                        Ok(unlocked) => {
                            let (entries, dropped) =
                                filter::build_entries(&unlocked, uri_prefix, crate::platform::OS_TAG);
                            vault = Some(unlocked);
                            VaultResult::Items { entries, dropped, stale: None }
                        }
                        Err(e) => {
                            let kind = error::classify(&e);
                            eprintln!("keepass: reload failed (kind={kind:?}): {e}");
                            VaultResult::Failed {
                                stage: "sync",
                                message: format!("reload failed: {}", kind.summary(THIS_PROVIDER)),
                                kind,
                            }
                        }
                    },
                };
                send(result);
                wake();
            }
            VaultCmd::List => {
                // No I/O, no KDF — just re-enumerate the resident vault.
                // The popup-open/refresh path; `Sync` above is the explicit
                // tray action that actually touches disk again.
                let result = match &vault {
                    None => VaultResult::Failed {
                        stage: "list",
                        kind: VaultErrorKind::Locked,
                        message: "vault is locked".to_string(),
                    },
                    Some(v) => {
                        let (entries, dropped) =
                            filter::build_entries(v, uri_prefix, crate::platform::OS_TAG);
                        VaultResult::Items { entries, dropped, stale: None }
                    }
                };
                send(result);
                wake();
            }
            VaultCmd::Lock => {
                vault = None;
                key = None;
                send(VaultResult::Locked);
                wake();
            }
            VaultCmd::GetTotp(item_id) => {
                let result = match (&vault, model::parse_entry_id(&item_id)) {
                    (None, _) => VaultResult::Failed {
                        stage: "totp",
                        kind: VaultErrorKind::Locked,
                        message: "vault is locked".to_string(),
                    },
                    (Some(_), None) => VaultResult::Failed {
                        stage: "totp",
                        kind: VaultErrorKind::Other,
                        message: "invalid item id".to_string(),
                    },
                    (Some(v), Some(id)) => match totp::compute(v, id) {
                        Ok(secret) => VaultResult::Totp(secret),
                        Err((kind, message)) => VaultResult::Failed { stage: "totp", kind, message },
                    },
                };
                send(result);
                wake();
            }
        }
    }
}

/// `Kdbx::open` → `read_header` → `unlock`, collapsed into one `Result` so
/// callers classify a single error once rather than three times with
/// identical handling at each stage.
fn open_and_unlock(
    db_path: &str,
    composite: &CompositeKey,
) -> Result<Kdbx<Unlocked>, keepass_core::error::Error> {
    Kdbx::open(db_path)?.read_header()?.unlock(composite)
}

fn unlock_and_list(
    db_path: &str,
    uri_prefix: &str,
    master_password: &Secret,
    vault: &mut Option<Kdbx<Unlocked>>,
    key: &mut Option<CompositeKey>,
) -> VaultResult {
    if db_path.is_empty() {
        return VaultResult::Failed {
            stage: "resolve",
            kind: VaultErrorKind::NotFound,
            message: "No KeePass database selected — pick one in Settings.".to_string(),
        };
    }

    let composite = CompositeKey::from_password(master_password.expose().as_bytes());
    let unlocked = match open_and_unlock(db_path, &composite) {
        Ok(u) => u,
        Err(e) => {
            let kind = error::classify(&e);
            let message = error::unlock_message(kind, &e);
            return VaultResult::Failed { stage: "unlock", kind, message };
        }
    };

    let (entries, dropped) = filter::build_entries(&unlocked, uri_prefix, crate::platform::OS_TAG);
    // Stored as soon as it's known good, before anything else — mirrors
    // `bw::unlock_sync_list`'s reasoning: a display quirk after unlock must
    // never throw away key material the user would then have to re-enter.
    *vault = Some(unlocked);
    *key = Some(composite);

    VaultResult::Items { entries, dropped, stale: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::sync::Arc;

    fn fixture(name: &str) -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
            .to_string_lossy()
            .into_owned()
    }

    /// A worker wired to a plain channel with a no-op waker — these tests
    /// run for real (unlike `bw`'s worker tests, this backend touches
    /// neither the network nor an external binary, so there's nothing to
    /// `#[ignore]`).
    fn spawn_test_worker(db_path: &str) -> (Sender<VaultCmd>, mpsc::Receiver<Msg>) {
        let (results_tx, results_rx) = mpsc::channel();
        let cmd_tx = spawn(
            db_path.to_string(),
            "app://context-password".to_string(),
            0,
            results_tx,
            Arc::new(|| {}),
        );
        (cmd_tx, results_rx)
    }

    fn recv_result(rx: &mpsc::Receiver<Msg>) -> VaultResult {
        // Generous enough to cover the `#[ignore]`d KDBX3 fixtures' real
        // ~6M-round AES-KDF (~10-30s in debug) when run explicitly via
        // `cargo test -- --ignored`; the fast (KDBX4/Argon2id) tests return
        // in well under a second either way.
        match rx.recv_timeout(std::time::Duration::from_secs(60)) {
            Ok(Msg::Vault { result, .. }) => result,
            Ok(_) => unreachable!("only the vault worker sends on this test channel"),
            Err(e) => panic!("worker did not reply in time: {e}"),
        }
    }

    #[test]
    fn unlock_with_correct_password_lists_the_tagged_entries() {
        let (tx, rx) = spawn_test_worker(&fixture("tagged.kdbx"));
        tx.send(VaultCmd::Unlock(Secret::new("tagged-fixture-pw".to_string()))).unwrap();
        match recv_result(&rx) {
            VaultResult::Items { entries, dropped, stale } => {
                assert_eq!(entries.len(), 6);
                assert_eq!(dropped, 2);
                assert!(stale.is_none());
            }
            other => panic!("expected Items, got a different result: {}", matches_desc(&other)),
        }
    }

    #[test]
    fn unlock_with_wrong_password_fails_with_bad_password() {
        let (tx, rx) = spawn_test_worker(&fixture("tagged.kdbx"));
        tx.send(VaultCmd::Unlock(Secret::new("definitely-wrong".to_string()))).unwrap();
        match recv_result(&rx) {
            VaultResult::Failed { stage, kind, message } => {
                assert_eq!(stage, "unlock");
                assert_eq!(kind, VaultErrorKind::BadPassword);
                assert!(message.contains("key file"), "message was: {message}");
            }
            other => panic!("expected Failed, got a different result: {}", matches_desc(&other)),
        }
    }

    #[test]
    #[ignore = "real KDBX3 AES-KDF at ~6M rounds — ~10-30s in a debug build, per keepass-core's \
                own test suite; run with `cargo test -- --ignored` when touching KDBX3 support"]
    fn keyfile_protected_database_fails_with_the_same_combined_message() {
        // We deliberately don't support keyfiles — unlocking with the
        // password alone must fail exactly like a wrong password would,
        // since `keepass-core` can't tell the two cases apart either.
        let (tx, rx) = spawn_test_worker(&fixture("keyfile.kdbx"));
        tx.send(VaultCmd::Unlock(Secret::new("tëst pässwörd 🔑/\\".to_string()))).unwrap();
        match recv_result(&rx) {
            VaultResult::Failed { kind, .. } => assert_eq!(kind, VaultErrorKind::BadPassword),
            other => panic!("expected Failed, got a different result: {}", matches_desc(&other)),
        }
    }

    #[test]
    #[ignore = "real KDBX3 AES-KDF at ~6M rounds — ~10-30s in a debug build, per keepass-core's \
                own test suite; run with `cargo test -- --ignored` when touching KDBX3 support"]
    fn legacy_kdbx3_database_unlocks_and_lists() {
        let (tx, rx) = spawn_test_worker(&fixture("legacy.kdbx"));
        tx.send(VaultCmd::Unlock(Secret::new("tëst pässwörd 🔑/\\".to_string()))).unwrap();
        // The legacy fixture's one entry isn't tagged for this app — it
        // should unlock cleanly and simply show nothing, not fail.
        match recv_result(&rx) {
            VaultResult::Items { entries, dropped, .. } => {
                assert!(entries.is_empty());
                assert_eq!(dropped, 0);
            }
            other => panic!("expected Items, got a different result: {}", matches_desc(&other)),
        }
    }

    #[test]
    fn no_database_selected_fails_clearly_not_with_an_io_error() {
        let (tx, rx) = spawn_test_worker("");
        tx.send(VaultCmd::Unlock(Secret::new("anything".to_string()))).unwrap();
        match recv_result(&rx) {
            VaultResult::Failed { stage, kind, message } => {
                assert_eq!(stage, "resolve");
                assert_eq!(kind, VaultErrorKind::NotFound);
                assert!(message.contains("Settings"), "message was: {message}");
            }
            other => panic!("expected Failed, got a different result: {}", matches_desc(&other)),
        }
    }

    #[test]
    fn list_after_unlock_is_instant_and_matches_the_unlock_result() {
        let (tx, rx) = spawn_test_worker(&fixture("tagged.kdbx"));
        tx.send(VaultCmd::Unlock(Secret::new("tagged-fixture-pw".to_string()))).unwrap();
        let VaultResult::Items { entries: first, .. } = recv_result(&rx) else {
            panic!("expected Items from Unlock")
        };

        tx.send(VaultCmd::List).unwrap();
        let VaultResult::Items { entries: second, .. } = recv_result(&rx) else {
            panic!("expected Items from List")
        };
        assert_eq!(first.len(), second.len());
    }

    #[test]
    fn list_before_any_unlock_fails_locked_not_a_panic() {
        let (tx, rx) = spawn_test_worker(&fixture("tagged.kdbx"));
        tx.send(VaultCmd::List).unwrap();
        match recv_result(&rx) {
            VaultResult::Failed { kind, .. } => assert_eq!(kind, VaultErrorKind::Locked),
            other => panic!("expected Failed, got a different result: {}", matches_desc(&other)),
        }
    }

    #[test]
    fn lock_clears_state_so_a_later_list_reports_locked_again() {
        let (tx, rx) = spawn_test_worker(&fixture("tagged.kdbx"));
        tx.send(VaultCmd::Unlock(Secret::new("tagged-fixture-pw".to_string()))).unwrap();
        recv_result(&rx);

        tx.send(VaultCmd::Lock).unwrap();
        assert!(matches!(recv_result(&rx), VaultResult::Locked));

        tx.send(VaultCmd::List).unwrap();
        match recv_result(&rx) {
            VaultResult::Failed { kind, .. } => assert_eq!(kind, VaultErrorKind::Locked),
            other => panic!("expected Failed, got a different result: {}", matches_desc(&other)),
        }
    }

    #[test]
    fn get_totp_round_trips_through_the_worker() {
        let (tx, rx) = spawn_test_worker(&fixture("tagged.kdbx"));
        tx.send(VaultCmd::Unlock(Secret::new("tagged-fixture-pw".to_string()))).unwrap();
        let VaultResult::Items { entries, .. } = recv_result(&rx) else {
            panic!("expected Items from Unlock")
        };
        let otp_entry = entries.iter().find(|e| e.name == "OTP Entry").expect("fixture has OTP Entry");
        assert!(otp_entry.has_totp);

        tx.send(VaultCmd::GetTotp(otp_entry.id.clone())).unwrap();
        match recv_result(&rx) {
            VaultResult::Totp(code) => {
                assert_eq!(code.expose().len(), 6);
                assert!(code.expose().chars().all(|c| c.is_ascii_digit()));
            }
            other => panic!("expected Totp, got a different result: {}", matches_desc(&other)),
        }
    }

    #[test]
    fn get_totp_on_an_entry_without_one_fails_clearly() {
        let (tx, rx) = spawn_test_worker(&fixture("tagged.kdbx"));
        tx.send(VaultCmd::Unlock(Secret::new("tagged-fixture-pw".to_string()))).unwrap();
        let VaultResult::Items { entries, .. } = recv_result(&rx) else {
            panic!("expected Items from Unlock")
        };
        let plain = entries.iter().find(|e| e.name == "URL Tagged").unwrap();

        tx.send(VaultCmd::GetTotp(plain.id.clone())).unwrap();
        match recv_result(&rx) {
            VaultResult::Failed { stage, message, .. } => {
                assert_eq!(stage, "totp");
                assert!(message.contains("no otp field"), "message was: {message}");
            }
            other => panic!("expected Failed, got a different result: {}", matches_desc(&other)),
        }
    }

    fn matches_desc(result: &VaultResult) -> &'static str {
        match result {
            VaultResult::Items { .. } => "Items",
            VaultResult::Failed { .. } => "Failed",
            VaultResult::Locked => "Locked",
            VaultResult::Totp(_) => "Totp",
        }
    }
}
