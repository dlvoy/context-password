# Building Context Password for Bitwarden

## What a build produces

| Command | Artifact |
| --- | --- |
| `cargo build` | `target\debug\context-password.exe` |
| `cargo build --release` | `target\release\context-password.exe` |
| `cargo wix` | `target\wix\context-password-<version>-x86_64.msi` |
| `makensis packaging\installer.nsi` | `target\release\ContextPassword-<version>-x64-setup.exe` |
| `cargo bundle` | all three of the above, one command |

## Prerequisites

- **Rust**, MSVC toolchain — `rustup default stable-x86_64-pc-windows-msvc`. The crate is on
  edition 2024 and was built and tested against rustc 1.97.1; anything close to that works.
- **Windows 10 or 11**. The app is Windows-only throughout (`windows-sys`, `RegisterHotKey`,
  `SendInput`) — it will not build for any other target.
- Building the installers additionally needs:
  - **`cargo-wix`** (`cargo install cargo-wix`) and the **WiX Toolset v3** (`choco install
    wixtoolset`, or download from the WiX Toolset website) for the MSI.
  - **NSIS** (`choco install nsis`, or download from the NSIS website) for the `.exe` installer.
  - Neither is needed for a plain `cargo build`.

## Commands

```
cargo build --release
cargo test
cargo clippy -- -D warnings
```

Regenerating the icon (only needed if `development/logo.png` changes — not run by a normal build):

```
powershell -ExecutionPolicy Bypass -File resources\generate-icons.ps1
```

This overwrites `resources\app.ico` and `resources\tray_icon.rgba` from the source PNG.

Building everything — the release binary, the MSI, and the NSIS installer, in that order, stopping
at the first failure:

```
cargo bundle
```

This is a small `xtask` crate (`xtask/`, aliased in `.cargo/config.toml`) — a separate crate, not
part of the main project, so it can never affect `context-password`'s own build or dependencies.
It shells out to exactly the commands below; run them individually instead if you only need one.

Building the MSI alone (needs `cargo build --release` first, and `cargo-wix` installed):

```
cargo wix --nocapture -L -sice:ICE69
```

No extension flag needed — `cargo-wix` already loads `WixUtilExtension` by default, which is what
the "launch after install" checkbox's `WixShellExec` needs. `-sice:ICE69` suppresses a false-
positive validation error: it flags the desktop shortcut for pointing at a file owned by a
different feature, but that feature can never be excluded (`Absent='disallow'`), so the shortcut
can never actually dangle.

Building the NSIS installer alone (needs `cargo build --release` first, and NSIS's `makensis` on
PATH):

```
makensis packaging\installer.nsi
```

Pass `/DPRODUCT_VERSION=x.y.z` to stamp a specific version into the installer's metadata and
filename; without it, the installer is named with a `0.0.0` placeholder. `cargo bundle` does this
automatically, reading the version straight from `Cargo.toml`.

## Project layout

- `src/app.rs` — the `eframe::App` implementation: the popup's show/hide and content-state
  machine, and the typing-delivery phase machine.
- `src/bw/` — everything that talks to the `bw` CLI: the worker thread, executable resolution,
  command building, and client-side filtering of vault items.
- `src/ui/` — pure rendering functions for each popup screen (prompt, item list, Settings, About).
- `src/platform/win/` — Win32 specifics: focus capture/restore, window placement, autostart,
  typing. A future macOS port lands alongside it as `src/platform/mac/`, both exposing the same
  module API via `src/platform/mod.rs`'s `#[cfg]` switch.
- `src/hotkey.rs`, `src/config.rs`, `src/secret.rs`, `src/msg.rs`, `src/tray.rs` — the global
  hotkey registration, on-disk config, the password-holding type, the inter-thread message enum,
  and the tray icon/menu.
- `xtask/` — the `cargo bundle` helper (see above). A separate crate, not part of the main build.

## The release pipeline

`.github/workflows/release.yml` runs on a pushed `vX.Y.Z` tag (or manually, for a dry run). It
checks that `Cargo.toml`'s version matches the tag, runs the test suite, builds the release binary,
builds both installers, computes a `checksums.txt`, and uploads everything as a build artifact. On
an actual tag push, it also publishes a GitHub Release with the exe, the MSI, the NSIS installer,
and the checksums attached.

## Cutting a release

1. Bump `version` in `Cargo.toml`.
2. `cargo update --workspace --offline` to refresh `Cargo.lock`.
3. Add a dated section to `CHANGELOG.md` for the new version.
4. Commit: `git commit -am "chore(release): vX.Y.Z"`.
5. Tag: `git tag vX.Y.Z`.
6. Push: `git push --follow-tags`.

Pushing the tag triggers the release workflow.

## Code signing

Release builds are unsigned. There is no code-signing certificate for this project, so Windows
SmartScreen will warn on first run of a freshly downloaded build — expected, not a bug. Real
signing would need an Authenticode certificate (from a CA or an EV token) and a `signtool sign`
step added to the release workflow after each installer is built.

## Troubleshooting

| Symptom | Cause |
| --- | --- |
| `cargo wix` fails to find `candle`/`light` | WiX Toolset isn't installed, or isn't on PATH. |
| `makensis` isn't recognized | NSIS isn't installed, or isn't on PATH. |
| A second launch exits immediately with no window | Expected — the single-instance guard is working; check the system tray for the existing instance. |
| The hotkey doesn't fire | Another application may already be using the same combination; try changing it in Settings. |
