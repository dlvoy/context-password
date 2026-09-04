//! `cargo bundle` — one invocation that builds the release binary(ies) and
//! the platform's installer(s), in the order each one depends on. Branches
//! on the *host* OS (`cfg!(target_os = ...)`, not a `--target` flag — this
//! always packages for whatever machine it's running on) since the two
//! chains share nothing beyond "run a sequence of external commands,
//! stop at the first failure": Windows builds one binary then feeds it to
//! `cargo wix`/`makensis`; macOS builds two architectures, `lipo`s them
//! into one universal binary, then hands that to `cargo packager`.
//!
//! A separate crate (not a workspace member) rather than a shell/PowerShell/
//! bash script, so it's one thing to run regardless of host — see the
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

    if cfg!(target_os = "macos") {
        run_macos(&root, &version)
    } else {
        run_windows(&root, &version)
    }
}

fn run_windows(root: &Path, version: &str) -> ExitCode {
    let steps: Vec<(&str, &str, Vec<String>)> = vec![
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

    for (label, program, args) in &steps {
        if let Err(code) = run_step(label, program, args, root) {
            return code;
        }
    }

    println!(
        "\nDone. target\\release\\context-password.exe, target\\wix\\*.msi, \
         target\\release\\ContextPassword-{version}-x64-setup.exe"
    );
    ExitCode::SUCCESS
}

fn run_macos(root: &Path, version: &str) -> ExitCode {
    let steps: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "Building the release binary (Apple Silicon)",
            "cargo",
            vec![
                "build".into(),
                "--release".into(),
                "--target".into(),
                "aarch64-apple-darwin".into(),
            ],
        ),
        (
            "Building the release binary (Intel)",
            "cargo",
            vec![
                "build".into(),
                "--release".into(),
                "--target".into(),
                "x86_64-apple-darwin".into(),
            ],
        ),
    ];
    for (label, program, args) in &steps {
        if let Err(code) = run_step(label, program, args, root) {
            return code;
        }
    }

    // `lipo -create`'s output directory has to exist first — not a build
    // step in its own right, so this isn't one of the printed steps above.
    // Its path matches `binaries-dir` in `Cargo.toml`'s
    // `[package.metadata.packager]`, which is where `cargo packager` (next
    // step) expects to find the binary it's told to bundle.
    let universal_dir = root.join("target/universal/release");
    if let Err(e) = std::fs::create_dir_all(&universal_dir) {
        eprintln!("could not create {}: {e}", universal_dir.display());
        return ExitCode::FAILURE;
    }

    let lipo_args = vec![
        "-create".to_string(),
        "-output".to_string(),
        universal_dir.join("context-password").to_string_lossy().into_owned(),
        root.join("target/aarch64-apple-darwin/release/context-password")
            .to_string_lossy()
            .into_owned(),
        root.join("target/x86_64-apple-darwin/release/context-password")
            .to_string_lossy()
            .into_owned(),
    ];
    if let Err(code) = run_step("Combining into a universal binary", "lipo", &lipo_args, root) {
        return code;
    }

    let mut packager_args = vec!["packager".to_string(), "--release".to_string()];
    if let Ok(identity) = std::env::var("APPLE_SIGNING_IDENTITY") {
        // Real Developer ID signing overrides the ad-hoc default in
        // Cargo.toml. `signing-identity` is a plain config value, not
        // itself env-driven, so `--config <json>` (documented as accepting
        // a raw JSON string merged over the base config) is the supported
        // way to override it per-invocation. Everything else — importing
        // the certificate from `APPLE_CERTIFICATE`/`APPLE_CERTIFICATE_PASSWORD`,
        // attempting notarization from whichever of `APPLE_KEYCHAIN_PROFILE`
        // / `APPLE_ID`+`APPLE_PASSWORD`+`APPLE_TEAM_ID` /
        // `APPLE_API_KEY`+`APPLE_API_ISSUER`+`APPLE_API_KEY_PATH` is
        // present — `cargo packager` already reads directly from the
        // environment on its own; nothing else to wire here. See
        // `context-password-project/development/apple-signing-setup.md`
        // for how a real account configures all of these.
        packager_args.push("--config".to_string());
        packager_args.push(format!(r#"{{"macos":{{"signingIdentity":"{identity}"}}}}"#));
    }
    if let Err(code) = run_step("Packaging the .app and .dmg", "cargo", &packager_args, root) {
        return code;
    }

    println!(
        "\nDone. target/packager/Context Password.app, \
         target/packager/Context Password_{version}_universal.dmg"
    );
    ExitCode::SUCCESS
}

fn run_step(label: &str, program: &str, args: &[String], root: &Path) -> Result<(), ExitCode> {
    println!("\n=== {label} ===");
    println!("> {program} {}", args.join(" "));
    match Command::new(program).args(args).current_dir(root).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            eprintln!("{program} exited with {status}");
            Err(ExitCode::FAILURE)
        }
        Err(e) => {
            eprintln!("failed to run {program}: {e}");
            Err(ExitCode::FAILURE)
        }
    }
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
