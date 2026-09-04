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

use super::error::classify;
use crate::vault::VaultErrorKind;
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

/// Runs `build()`'s command; if it fails with a `Tls`-classified error (this
/// machine's corporate proxy intercepting the vault's TLS connection and
/// presenting its own certificate — see `vault::VaultErrorKind::Tls`), runs it
/// a second time with certificate verification disabled for that one child
/// process, exactly as the user confirmed manually (`NODE_TLS_REJECT_
/// UNAUTHORIZED=0 bw unlock`) gets past it. `build` a closure rather than a
/// single `Command` because a `Command` can't be re-spawned after use, and
/// the two attempts otherwise need identical args/env.
///
/// Deliberately narrow: only a `Tls` classification retries. A `Network`
/// classification (DNS failure, connection refused, a genuinely blackholed
/// link) would just fail the same way again with verification off — that's
/// not what this environment variable fixes — so retrying there would only
/// double the wait before the graceful-degradation path (stale vault /
/// "can't reach the server") kicks in.
///
/// Returns whether the insecure retry actually ran, alongside the `Run` it
/// produced (or the first `Run`, if no retry was needed) — `unlock_sync_
/// list` uses that to skip the `bw sync` that would otherwise immediately
/// follow a successful unlock: `sync`/`list_items` are deliberately *not*
/// given this same fallback (see their own docs — going through to the
/// intercepting host crashes them), so if unlock only got through this way,
/// a sync attempt right after is known to just fail again, for no benefit.
fn run_with_tls_fallback(build: impl Fn() -> Command, timeout: Duration) -> io::Result<(Run, bool)> {
    let first = run::run(build(), timeout)?;
    if first.success() || classify(&first.stderr, first.timed_out) != VaultErrorKind::Tls {
        return Ok((first, false));
    }
    eprintln!(
        "bw: TLS certificate mismatch talking to the vault server (corporate proxy/MITM \
         suspected) — retrying this call with certificate verification disabled"
    );
    let mut insecure = build();
    insecure.env("NODE_TLS_REJECT_UNAUTHORIZED", "0");
    Ok((run::run(insecure, timeout)?, true))
}

/// `bw unlock --raw`, with the master password passed via `--passwordenv`
/// — **never** as a positional argument. `bw unlock myPassword321` is the
/// form shown in the CLI's own help text, and is the obvious thing to
/// "simplify" this to. Don't: see the module doc for why.
pub fn unlock(exe: &BwExe, master_password: &str) -> io::Result<(Run, bool)> {
    run_with_tls_fallback(
        || {
            let mut cmd = base(exe);
            cmd.args([
                "unlock",
                "--raw",
                "--nointeraction",
                "--passwordenv",
                "CP_MASTERPW",
            ])
            .env("CP_MASTERPW", master_password);
            cmd
        },
        UNLOCK_TIMEOUT,
    )
}

/// `bw sync`, session passed via `BW_SESSION` (which `--session` reads from
/// by default) — never `--session KEY` on argv (plan F9): the session key
/// decrypts the whole vault and deserves the same argv hygiene as the
/// master password.
///
/// Deliberately **not** run through `run_with_tls_fallback` — confirmed by
/// manual testing to be actively harmful here, unlike `unlock`. Behind this
/// machine's Zscaler, the vault's domain resolves to a corporate
/// block-notice host; with certificate verification on, `bw sync`'s TLS
/// handshake to it fails cleanly (a `Tls`-classified error, handled by
/// `sync_then_list`'s existing stale-vault fallback below). With
/// verification disabled, the handshake succeeds and `bw sync` receives that
/// host's HTML block page where it expected JSON — which it does not handle
/// gracefully: it throws an uncaught exception and the whole `bw` process
/// crashes mid-sync, observed to leave the local vault cache (`data.json`)
/// corrupted (a bad cached `Policy.revisionDate`), which then broke *every*
/// later `bw list items` call with `bitwarden_crypto` decryption errors —
/// nothing to do with the master password despite how that first presented.
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
///
/// Not run through `run_with_tls_fallback` either — same reasoning as
/// `sync` above: unverified, but presumed to risk the identical
/// crash-on-malformed-response failure mode if it ever needs the network,
/// which a clean `Tls`/`Network`-classified failure here does not.
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
/// Bitwarden's own algorithm exactly, no local crypto dependency). Not run
/// through `run_with_tls_fallback` — same reasoning as `sync`/`list_items`
/// above, and this one genuinely needs the real server (a TOTP code from the
/// wrong host is worthless anyway), so there is nothing for the fallback to
/// usefully buy here even setting the crash risk aside.
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
