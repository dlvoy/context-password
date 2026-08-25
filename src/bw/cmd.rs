//! Spawning `bw`.
//!
//! Every call here shares three properties: no console flash
//! (`CREATE_NO_WINDOW`), stderr discarded (`bw` writes noise there, per the
//! research doc), and secrets passed only via the child's environment,
//! never argv (plan §6 — argv is readable by any same-user process, e.g.
//! Task Manager's command-line column, Process Explorer, or WMI
//! `Win32_Process.CommandLine`, and is commonly logged by EDR; the child's
//! environment block needs `PROCESS_VM_READ` plus a PEB walk to read from
//! outside).

use std::io;
use std::os::windows::process::CommandExt;
use std::process::{Command, Output, Stdio};

use super::exe::BwExe;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn base(exe: &BwExe) -> Command {
    let mut cmd = match exe {
        BwExe::Direct(path) => Command::new(path),
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
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .env("NO_COLOR", "1")
        .env_remove("BW_SESSION");
    cmd
}

/// `bw unlock --raw`, with the master password passed via `--passwordenv`
/// — **never** as a positional argument. `bw unlock myPassword321` is the
/// form shown in the CLI's own help text, and is the obvious thing to
/// "simplify" this to. Don't: see the module doc for why.
pub fn unlock(exe: &BwExe, master_password: &str) -> io::Result<Output> {
    let mut cmd = base(exe);
    cmd.args([
        "unlock",
        "--raw",
        "--nointeraction",
        "--passwordenv",
        "CP_MASTERPW",
    ])
    .env("CP_MASTERPW", master_password);
    cmd.output()
}

/// `bw sync`, session passed via `BW_SESSION` (which `--session` reads from
/// by default) — never `--session KEY` on argv (plan F9): the session key
/// decrypts the whole vault and deserves the same argv hygiene as the
/// master password.
pub fn sync(exe: &BwExe, session_key: &str) -> io::Result<Output> {
    let mut cmd = base(exe);
    cmd.args(["sync", "--nointeraction"])
        .env("BW_SESSION", session_key);
    cmd.output()
}

/// `bw list items`, prefiltered server-side by `search_term`. The caller
/// must still re-filter client-side on `login.uris[].uri` — `--search` is a
/// fuzzy match across many fields (see `bw::filter`), not an exact prefix
/// match.
pub fn list_items(exe: &BwExe, session_key: &str, search_term: &str) -> io::Result<Output> {
    let mut cmd = base(exe);
    cmd.args(["list", "items", "--nointeraction", "--search", search_term])
        .env("BW_SESSION", session_key);
    cmd.output()
}

/// `bw lock` — destroys the CLI's own active session keys. Doesn't need
/// `BW_SESSION` itself (locking isn't scoped to a particular session), but
/// this only ever runs from an explicit user action (the tray's Lock item),
/// never automatically on exit — see plan §8 on why that distinction
/// matters (it would invalidate session keys the user may be relying on in
/// other terminals, which is fine when they asked for it and surprising
/// when they didn't).
pub fn lock(exe: &BwExe) -> io::Result<Output> {
    let mut cmd = base(exe);
    cmd.args(["lock", "--nointeraction"]);
    cmd.output()
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
        let output = cmd.output().expect("bw --version should spawn");
        assert!(output.status.success());
        let version = String::from_utf8_lossy(&output.stdout);
        eprintln!("bw --version -> {}", version.trim());
        assert!(!version.trim().is_empty());
    }
}
