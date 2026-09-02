//! Runs a `bw` subprocess with a timeout, draining both pipes on their own
//! threads while waiting.
//!
//! Every call in `bw::cmd` used to be a bare `Command::output()` with no
//! deadline at all — fine when the CLI is local and instant, disastrous
//! when it isn't: `bw`'s own network calls (`sync`, `unlock`, `list items`,
//! `get totp`) can hang indefinitely against a server whose domain is
//! firewalled outright (as opposed to merely DNS-failing, which is fast).
//! Since the worker thread (`bw::mod`) processes commands strictly one at a
//! time, a single hung call wedges every later Unlock/Lock/Sync/GetTotp
//! forever, with no way for the UI to cancel or recover.
//!
//! Draining stdout and stderr on separate threads (rather than a
//! `try_wait` polling loop that only reads after the child exits) avoids
//! the classic pipe deadlock: `bw list items` can write megabytes of JSON
//! to stdout while also filling the ~64 KiB stderr pipe with unrelated
//! noise, and a child blocks on a full pipe until *someone* reads it.

use std::io::Read;
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

pub struct Run {
    /// `None` only when `timed_out` is true — the process was killed before
    /// it could exit on its own.
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    /// Lossily decoded and trimmed — `bw`'s stderr is human-readable text
    /// and diagnostic Node stack traces, never binary.
    pub stderr: String,
    pub timed_out: bool,
}

impl Run {
    pub fn success(&self) -> bool {
        !self.timed_out && self.status.is_some_and(|s| s.success())
    }
}

/// Spawns `cmd` (which must have `.stdout(Stdio::piped()).stderr(Stdio::piped())`
/// already set) and waits up to `timeout`. On timeout, kills the child and
/// returns `Run { timed_out: true, status: None, .. }` with whatever output
/// had been produced so far.
pub fn run(mut cmd: Command, timeout: Duration) -> std::io::Result<Run> {
    let mut child: Child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout must be piped");
    let mut stderr = child.stderr.take().expect("stderr must be piped");

    let stdout_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_thread = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        match child.try_wait()? {
            Some(status) => break (Some(status), false),
            None if Instant::now() >= deadline => {
                // Best-effort: the child may have exited in the gap between
                // `try_wait` and `kill`, which just makes `kill` a no-op.
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr_bytes = stderr_thread.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

    Ok(Run {
        status,
        stdout,
        stderr,
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulates the actual failure mode this module exists for — `bw sync`
    /// blackholed by a firewalled domain, rather than failing fast on a bad
    /// DNS lookup — without waiting on a real network. `sleep` is a
    /// standard Unix utility; this test is `#[cfg(unix)]` since Windows has
    /// no equivalent single-purpose blocking command to spawn directly
    /// (Windows verification for this port goes through a cross-compile
    /// check, not test execution, until a Windows machine is available —
    /// see the port plan's status notes).
    #[cfg(unix)]
    #[test]
    fn kills_and_reports_timeout_on_a_hung_process() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let started = Instant::now();
        let result = run(cmd, Duration::from_millis(200)).expect("spawn should succeed");
        let elapsed = started.elapsed();

        assert!(result.timed_out, "expected the 5s sleep to be killed as a timeout");
        assert!(result.status.is_none());
        assert!(
            elapsed < Duration::from_secs(2),
            "should return promptly after killing, not wait out the full sleep: {elapsed:?}"
        );
    }

    #[test]
    fn reports_success_and_captures_stdout_for_a_fast_process() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let result = run(cmd, Duration::from_secs(5)).expect("spawn should succeed");
        assert!(result.success());
        assert!(!result.timed_out);
        assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), "hello");
    }
}
