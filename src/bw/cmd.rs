//! Spawning `bw`.
//!
//! Every call here shares three properties: no console flash
//! (`CREATE_NO_WINDOW`), stderr captured (not discarded — see `bw::error`;
//! `bw` writes its own diagnostics there, which is exactly what makes a
//! network failure distinguishable from a wrong password), and secrets
//! passed only via the child's environment, never argv (plan §6 — argv is
//! readable by any same-user process, e.g. Task Manager's command-line
//! column, Process Explorer, or WMI `Win32_Process.CommandLine`, and is
//! commonly logged by EDR; the child's environment block needs
//! `PROCESS_VM_READ` plus a PEB walk to read from outside). Every call also
//! runs under a timeout (`bw::run`) — the CLI's own network calls can hang
//! indefinitely against a firewalled server otherwise, wedging the
//! strictly-sequential worker thread forever.

use std::io;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::exe::BwExe;
use super::run::{self, Run};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Local calls (`lock`, `status`) never touch the network — an argon2 KDF
/// unlock and a `sync`/`list`/`get totp` round trip against a real server
/// might still be slow on a bad link, so they get more room. Chosen to fail
/// fast enough that a firewalled Vaultwarden domain (the motivating case)
/// doesn't wedge the worker thread for an unreasonable stretch, while
/// staying comfortably above a normal cold start.
const UNLOCK_TIMEOUT: Duration = Duration::from_secs(45);
const SYNC_TIMEOUT: Duration = Duration::from_secs(20);
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
const TOTP_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

fn base(exe: &BwExe) -> Command {
    let mut cmd = match exe {
        BwExe::Direct(path) => Command::new(path),
        // `ViaCmd` only exists on Windows — see `bw::exe::BwExe`.
        #[cfg(windows)]
        BwExe::ViaCmd(path) => {
            // The .cmd is passed as an *argument* to cmd.exe rather than as
            // the program — this sidesteps Rust's post-CVE-2024-24576
            // batch-file argument-escaping rules, which otherwise reject
            // arguments they can't prove are safe. Our own arguments never
            // contain shell metacharacters (& | ^ < >); that stops being
            // true the day the search term becomes user-configurable, so
            // don't add one without revisiting this.
            let mut c = Command::new("cmd.exe");
            c.arg("/C").arg(path);
            c
        }
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .env_remove("BW_SESSION");
    // No console flash on Windows — irrelevant on macOS, which has no
    // console window to flash in the first place.
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// `bw unlock --raw`, with the master password passed via `--passwordenv`
/// — **never** as a positional argument. `bw unlock myPassword321` is the
/// form shown in the CLI's own help text, and is the obvious thing to
/// "simplify" this to. Don't: see the module doc for why.
pub fn unlock(exe: &BwExe, master_password: &str) -> io::Result<Run> {
    let mut cmd = base(exe);
    cmd.args([
        "unlock",
        "--raw",
        "--nointeraction",
        "--passwordenv",
        "CP_MASTERPW",
    ])
    .env("CP_MASTERPW", master_password);
    run::run(cmd, UNLOCK_TIMEOUT)
}

/// `bw sync`, session passed via `BW_SESSION` (which `--session` reads from
/// by default) — never `--session KEY` on argv (plan F9): the session key
/// decrypts the whole vault and deserves the same argv hygiene as the
/// master password.
pub fn sync(exe: &BwExe, session_key: &str) -> io::Result<Run> {
    let mut cmd = base(exe);
    cmd.args(["sync", "--nointeraction"])
        .env("BW_SESSION", session_key);
    run::run(cmd, SYNC_TIMEOUT)
}

/// `bw list items`, prefiltered server-side by `search_term`. The caller
/// must still re-filter client-side on `login.uris[].uri` — `--search` is a
/// fuzzy match across many fields (see `bw::filter`), not an exact prefix
/// match.
pub fn list_items(exe: &BwExe, session_key: &str, search_term: &str) -> io::Result<Run> {
    let mut cmd = base(exe);
    cmd.args(["list", "items", "--nointeraction", "--search", search_term])
        .env("BW_SESSION", session_key);
    run::run(cmd, LIST_TIMEOUT)
}

/// `bw lock` — destroys the CLI's own active session keys. Doesn't need
/// `BW_SESSION` itself (locking isn't scoped to a particular session). Runs
/// from the tray's Lock item, and optionally on exit if `lock_on_exit` is
/// enabled — off by default, since it would invalidate session keys the
/// user may be relying on in other terminals, which is fine when they asked
/// for it and surprising when they didn't (plan §8).
pub fn lock(exe: &BwExe) -> io::Result<Run> {
    let mut cmd = base(exe);
    cmd.args(["lock", "--nointeraction"]);
    run::run(cmd, LOCK_TIMEOUT)
}

/// `bw get totp <item_id> --raw`, session via `BW_SESSION` like every other
/// authenticated call. Fetched fresh at the moment of use rather than
/// computed from the seed locally — see the plan's rationale (reuses
/// Bitwarden's own algorithm exactly, no local crypto dependency).
pub fn get_totp(exe: &BwExe, session_key: &str, item_id: &str) -> io::Result<Run> {
    let mut cmd = base(exe);
    cmd.args(["get", "totp", item_id, "--raw", "--nointeraction"])
        .env("BW_SESSION", session_key);
    run::run(cmd, TOTP_TIMEOUT)
}

/// `bw status --nointeraction` — local only, never touches the network.
/// Used purely for diagnostics: turning a vague sync/unlock failure into a
/// precise message (is there even an account logged in on this machine?
/// when did the vault last sync successfully?). Its JSON lands on stdout
/// with exit 0 even when the vault is locked or unauthenticated; only a
/// genuinely broken `bw` install should make this fail.
pub fn status(exe: &BwExe) -> io::Result<Run> {
    let mut cmd = base(exe);
    cmd.args(["status", "--nointeraction"]);
    run::run(cmd, STATUS_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bw::exe;

    /// Exercises the real `cmd.exe /C <bw.cmd>` spawn path end to end (no
    /// secrets involved, just `--version`) — this is what actually caught,
    /// and confirmed the fix for, F8 on this machine (`bw` here resolves to
    /// `bw.cmd`, which `Command::new("bw")` alone cannot spawn).
    #[test]
    #[ignore = "depends on bw being installed on this machine's PATH"]
    fn can_spawn_bw_version() {
        let found = exe::resolve("").expect("bw should be discoverable on PATH");
        let mut cmd = base(&found);
        cmd.arg("--version");
        let run = run::run(cmd, Duration::from_secs(10)).expect("bw --version should spawn");
        assert!(run.success());
        let version = String::from_utf8_lossy(&run.stdout);
        eprintln!("bw --version -> {}", version.trim());
        assert!(!version.trim().is_empty());
    }
}
