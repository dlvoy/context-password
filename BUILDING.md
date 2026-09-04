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
- **Windows 10 or 11**, to build the full app today. A macOS port is in progress but not yet
  buildable end to end — see [macOS](#macos-work-in-progress) below.
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

## macOS (work in progress)

The macOS port isn't finished — full status, the phased plan, and the design decisions behind it
live in `../context-password-project/plan/macos-port-plan.md`, a sibling repo to this one (kept
outside `context-password` itself so the plan can be pulled onto another machine independently of
this repo's history). This section only covers what needs to be installed to pick the work back up.

### Prerequisites

- **A real Mac.** Apple licenses the macOS SDK for use on Apple hardware only, and the actually
  risky parts of this port — the Accessibility/TCC permission, Secure Input, and WindowServer's
  synthetic-event filtering — can only be observed on real hardware. See the plan's Phase 0.
- **Xcode Command Line Tools**: `xcode-select --install`. Provides the macOS SDK, `clang`,
  `codesign`, `lipo`, `iconutil`, `sips`, and `hdiutil` — everything Phase 4's packaging needs, and
  what linking the `objc2`/AppKit bindings from Phase 2 onward requires.
- **Rust**, same edition and version as Windows (edition 2024, tested against rustc 1.97.1) —
  `rustup default stable` is enough for the host architecture. For the universal binary Phase 4
  calls for: `rustup target add aarch64-apple-darwin x86_64-apple-darwin`.
- **The Bitwarden CLI** (`bw`): `brew install bitwarden-cli` (or `npm install -g @bitwarden/cli`),
  then `bw login` / `bw unlock` as in the main [README](README.md#setting-up-bitwarden). Homebrew
  installs to `/opt/homebrew/bin` on Apple Silicon or `/usr/local/bin` on Intel — neither is on the
  minimal `PATH` a GUI-launched `.app` inherits, which is why the macOS arm of `bw::exe`'s discovery
  (Phase 2, not yet written) has to check both locations plus `~/.npm-global/bin` explicitly, the
  same way the `bw_path` Settings override is the escape hatch on Windows.
- For packaging, once Phase 4 starts: `cargo install cargo-packager`, for `.app`/`.dmg` bundling.
  Signing and notarization use `codesign`/`notarytool` from Xcode Command Line Tools, already
  above — ad-hoc signing needs nothing further; a paid Developer ID identity is only needed to turn
  on real notarization later.

### Where things stand

`cargo build`/`cargo test` on macOS itself won't fully succeed yet: `src/app.rs`, `src/ui/`, and
`main.rs` are still unconditionally Windows/`eframe`-specific and haven't been split out from the
future `mac_ui` front-end (tracked in the plan as the deferred step 1d). What already compiles
cleanly for a macOS target today is the shared core underneath that UI layer — `src/bw/`,
`src/config.rs`, `src/secret.rs`, `src/msg.rs`, `src/hotkey.rs`, `src/tray.rs`, and the
`src/platform/` `#[cfg]` switch itself (`src/platform/win/` behind `#[cfg(windows)]`, with
`src/platform/mac/` still to come). That was verified without a Mac at all, from Windows, since
`rustup target add aarch64-apple-darwin` downloads a prebuilt std even without the SDK:

```
rustup target add aarch64-apple-darwin
cargo check --target aarch64-apple-darwin
```

This type-checks everything but can't link (no SDK), so it stops at the still-Windows-only files
rather than succeeding outright — useful as a quick way to catch a shared file accidentally picking
up Windows-only code, from either machine. On the Mac itself, once there's a macOS entry point to
check against, plain `cargo check` (no `--target`) is the equivalent sanity check against the host.

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

## macOS: keeping the Accessibility grant across rebuilds

`context-password` needs the Accessibility permission (`AXIsProcessTrusted`, checked in
`src/platform/mac/permissions.rs`) to post synthetic keystrokes into another app. macOS's TCC ties
that grant to the binary's *code identity*, not its path. `cargo build`'s debug output is
unsigned, and an unsigned/ad-hoc binary's identity is derived from a hash of its own bytes — so
**every rebuild is a new app as far as TCC is concerned**, and a grant given to yesterday's build
silently stops applying even though the checkbox in System Settings still looks checked.
`AXIsProcessTrusted()` then returns false and delivery aborts with `BlockReason::NoAccessibility`
— macOS dev-loop friction, not a bug in this app.

Fix once per machine:

1. Keychain Access → menu **Keychain Access → Certificate Assistant → Create a Certificate…**
   - Name: anything memorable, e.g. `context-password-dev`
   - Identity Type: **Self Signed Root**
   - Certificate Type: **Code Signing**
   - Accept the defaults for everything else.
2. Sign the debug binary with it after every build:
   ```
   codesign --force --sign "context-password-dev" target/debug/context-password
   ```
3. Launch the app once, then grant Accessibility: trigger the hotkey (macOS prompts
   automatically), or add it manually via System Settings → Privacy & Security → Accessibility →
   **+** → navigate to `target/debug/context-password` → enable the checkbox.

Because the identity comes from the certificate rather than the binary's contents, it stays the
same across rebuilds as long as step 2 is re-run each time — the existing grant keeps applying
without re-prompting. If Accessibility was previously granted to an unsigned build, remove that
stale row first (select it, click **-**) before re-adding the signed one, since a duplicate stale
entry can otherwise mask whether the fix took effect.

## Troubleshooting

| Symptom | Cause |
| --- | --- |
| `cargo wix` fails to find `candle`/`light` | WiX Toolset isn't installed, or isn't on PATH. |
| `makensis` isn't recognized | NSIS isn't installed, or isn't on PATH. |
| A second launch exits immediately with no window | Expected — the single-instance guard is working; check the system tray for the existing instance. |
| The hotkey doesn't fire | Another application may already be using the same combination; try changing it in Settings. |
| (macOS) Accessibility looks granted but delivery still fails with `NoAccessibility` | Debug builds are unsigned, so each rebuild is a new identity to TCC — see [macOS: keeping the Accessibility grant across rebuilds](#macos-keeping-the-accessibility-grant-across-rebuilds). |
