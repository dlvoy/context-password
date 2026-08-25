//! Locating the `bw` executable.
//!
//! See plan F8: on a machine with the CLI installed via npm there is no
//! `bw.exe`, only `bw.cmd` — a batch shim — and Rust's `Command::new("bw")`
//! only appends `.exe` during PATH search, so it fails outright. This walks
//! `PATH` (honoring `PATHEXT`) and a couple of known install locations
//! itself, the same resolution `cmd.exe` would do.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum BwExe {
    /// A real `bw.exe` — spawn directly.
    Direct(PathBuf),
    /// A `.cmd`/`.bat` shim — must be spawned as an argument to `cmd.exe`,
    /// not as the program itself (see `bw::cmd`).
    ViaCmd(PathBuf),
}

impl BwExe {
    /// Unused until a later milestone surfaces the resolved path (Settings'
    /// autodetect display, or an error message naming which file failed).
    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        match self {
            BwExe::Direct(p) | BwExe::ViaCmd(p) => p,
        }
    }
}

/// `bw_path` is the config override; empty means autodetect.
pub fn resolve(bw_path: &str) -> Result<BwExe, String> {
    if !bw_path.is_empty() {
        let p = PathBuf::from(bw_path);
        return classify(&p)
            .ok_or_else(|| format!("configured bw path '{}' does not exist", p.display()));
    }

    if let Some(found) = search_path("bw") {
        return Ok(found);
    }

    for candidate in known_install_locations() {
        if let Some(found) = classify(&candidate) {
            return Ok(found);
        }
    }

    Err(
        "bw executable not found on PATH or in common install locations; \
         set the path explicitly in Settings"
            .to_string(),
    )
}

fn classify(path: &Path) -> Option<BwExe> {
    if !path.is_file() {
        return None;
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("exe") => Some(BwExe::Direct(path.to_path_buf())),
        Some(ext) if ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat") => {
            Some(BwExe::ViaCmd(path.to_path_buf()))
        }
        _ => None,
    }
}

/// Walks `PATH`, trying each directory with every extension in `PATHEXT` —
/// the same resolution order `cmd.exe` itself uses, since `Command::new`
/// only tries `.exe` on Windows.
fn search_path(name: &str) -> Option<BwExe> {
    let path_var = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let exts: Vec<&str> = pathext.split(';').filter(|e| !e.is_empty()).collect();

    for dir in std::env::split_paths(&path_var) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if let Some(found) = classify(&candidate) {
                return Some(found);
            }
        }
    }
    None
}

fn known_install_locations() -> Vec<PathBuf> {
    let mut locations = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        locations.push(PathBuf::from(&appdata).join("npm").join("bw.cmd"));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        locations.push(
            PathBuf::from(&program_files)
                .join("Bitwarden CLI")
                .join("bw.exe"),
        );
    }
    locations
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Depends on `bw` actually being installed on this machine's PATH —
    /// not something to assert on an arbitrary CI runner, but worth being
    /// able to check by hand (`cargo test -- --ignored`) on a dev machine,
    /// and it's what caught F8 in the first place (`bw` here resolves to
    /// `bw.cmd`, not `bw.exe`).
    #[test]
    #[ignore = "depends on bw being installed on this machine's PATH"]
    fn finds_bw_on_this_machine() {
        let found = resolve("").expect("bw should be discoverable on PATH");
        eprintln!("resolved: {found:?}");
    }
}
