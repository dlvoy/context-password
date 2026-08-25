//! `cargo bundle` — one invocation that builds the release binary and both
//! Windows installers, in the order each one depends on.
//!
//! A separate crate (not a workspace member) rather than a shell/PowerShell
//! script, so it's one thing to run regardless of shell — see the
//! `cargo-xtask` convention (<https://github.com/matklad/cargo-xtask>).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let root = repo_root();
    let version = match cargo_toml_version(&root.join("Cargo.toml")) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("could not read the version from Cargo.toml: {e}");
            return ExitCode::FAILURE;
        }
    };

    let steps: [(&str, &str, Vec<String>); 3] = [
        ("Building the release binary", "cargo", vec!["build".into(), "--release".into()]),
        (
            // `cargo-wix` already loads WixUtilExtension (needed for the
            // "launch after install" checkbox's `WixShellExec`) by default
            // — passing it again with `-C -ext ...` collides with its own
            // copy (candle error CNDL0125) rather than being a no-op.
            //
            // -sice:ICE69 suppresses a validation error that's a false
            // positive here: it flags the desktop shortcut's component for
            // referencing a file (the exe) owned by a different feature,
            // but that feature (Binaries) has Absent='disallow' — it can
            // never be excluded, so the shortcut can never dangle.
            "Building the MSI",
            "cargo",
            vec!["wix".into(), "--nocapture".into(), "-L".into(), "-sice:ICE69".into()],
        ),
        (
            "Building the NSIS installer",
            "makensis",
            vec![format!("/DPRODUCT_VERSION={version}"), "packaging/installer.nsi".into()],
        ),
    ];

    for (label, program, args) in steps {
        println!("\n=== {label} ===");
        println!("> {program} {}", args.join(" "));
        match Command::new(program).args(&args).current_dir(&root).status() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("{program} exited with {status}");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("failed to run {program}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    println!(
        "\nDone. target\\release\\context-password.exe, target\\wix\\*.msi, \
         target\\release\\ContextPassword-{version}-x64-setup.exe"
    );
    ExitCode::SUCCESS
}

/// This crate's own `Cargo.toml` lives at `<repo>\xtask\Cargo.toml`, so the
/// repo root is always its grandparent — independent of whatever directory
/// `cargo bundle` was actually invoked from.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/Cargo.toml always has a parent directory")
        .to_path_buf()
}

/// A full TOML parser is more than one line needs — this is the same
/// approach the release workflow already uses in its own version check.
fn cargo_toml_version(path: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    text.lines()
        .find_map(|line| line.strip_prefix("version = "))
        .map(|v| v.trim_matches('"').to_string())
        .ok_or_else(|| "no `version = \"...\"` line found".to_string())
}
