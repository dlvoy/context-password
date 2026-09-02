//! Ensures only one instance of context-password runs at a time — the
//! macOS counterpart of `win::singleton`, same guarantee (a second launch
//! must not register the global hotkey twice or leave two tray icons
//! behind), different mechanism: an advisory `flock` on a lock file under
//! the app's own config directory rather than a named OS mutex. This also
//! works for the unbundled dev binary, unlike checking for a running
//! bundle-id, which only a packaged `.app` has.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

/// Holds the lock file open (and locked) for the process lifetime. Dropping
/// it — including on normal exit — closes the fd, which releases the
/// `flock` for the next launch.
pub struct SingleInstance {
    _file: File,
}

impl SingleInstance {
    /// Returns `Some(guard)` if this is the only running instance, or `None`
    /// if another instance already holds the lock.
    pub fn acquire() -> Option<Self> {
        let path = match lock_path() {
            Ok(p) => p,
            // Fail open rather than block startup on an OS-level anomaly we
            // can't diagnose here — same posture as `win::singleton` when
            // `CreateMutexW` itself fails.
            Err(_) => return Some(Self {
                _file: File::create("/dev/null").expect("/dev/null must exist"),
            }),
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = match File::create(&path) {
            Ok(f) => f,
            Err(_) => {
                return Some(Self {
                    _file: File::create("/dev/null").expect("/dev/null must exist"),
                });
            }
        };
        let fd = file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            Some(Self { _file: file })
        } else if io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) {
            None
        } else {
            // Some other, unexpected `flock` failure — fail open, same
            // reasoning as above.
            Some(Self { _file: file })
        }
    }
}

fn lock_path() -> io::Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "$HOME is not set"))?;
    Ok(std::path::PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("context-password")
        .join(".lock"))
}
