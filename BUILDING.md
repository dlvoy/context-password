# Building Context Password for Bitwarden

## What a build produces

| Command | Artifact |
| --- | --- |
| `cargo build` | `target\debug\context-password.exe` (Windows) / `target/debug/context-password` (macOS) |
| `cargo build --release` | `target\release\context-password.exe` / `target/release/context-password` |
| `cargo wix` (Windows) | `target\wix\context-password-<version>-x86_64.msi` |
| `makensis packaging\installer.nsi` (Windows) | `target\release\ContextPassword-<version>-x64-setup.exe` |
| `cargo packager --release` (macOS) | `target/packager/Context Password.app`, `target/packager/Context Password_<version>_universal.dmg` |
| `cargo bundle` | everything above for the host platform, one command |

## Prerequisites

- **Rust**, edition 2024, tested against rustc 1.97.1; anything close to that works.
  - Windows: the MSVC toolchain — `rustup default stable-x86_64-pc-windows-msvc`.
  - macOS: `rustup default stable` is enough for the host architecture; the universal-binary build
    additionally needs both Apple targets — `rustup target add aarch64-apple-darwin
    x86_64-apple-darwin`.
- Building the Windows installers additionally needs:
  - **`cargo-wix`** (`cargo install cargo-wix`) and the **WiX Toolset v3** (`choco install
    wixtoolset`, or download from the WiX Toolset website) for the MSI.
  - **NSIS** (`choco install nsis`, or download from the NSIS website) for the `.exe` installer.
  - Neither is needed for a plain `cargo build`.
- Building the macOS `.app`/`.dmg` additionally needs **Xcode Command Line Tools**
  (`xcode-select --install` — provides `clang`, `codesign`, `lipo`, `iconutil`, `sips`, `hdiutil`)
  and **`cargo-packager`** (`cargo install cargo-packager --locked`). See [macOS](#macos) below.

## Commands

```
cargo build --release
cargo test
cargo clippy -- -D warnings
```

Regenerating the app icon (only needed if `development/logo.png` changes — not run by a normal
build; `development/` is gitignored, so this is a local, as-needed step, not part of CI):

```
powershell -ExecutionPolicy Bypass -File resources\generate-icons.ps1   # Windows: app.ico, tray_icon.rgba
bash resources/generate-icons-macos.sh                                  # macOS: AppIcon.icns
```

Building everything for the host platform — on Windows, the release binary, the MSI, and the NSIS
installer, in that order; on macOS, both architectures, `lipo`'d into a universal binary, then the
`.app` and `.dmg` — stopping at the first failure:

```
cargo bundle
```

This is a small `xtask` crate (`xtask/`, aliased in `.cargo/config.toml`) — a separate crate, not
part of the main project, so it can never affect `context-password`'s own build or dependencies.
It shells out to exactly the commands below (or their macOS equivalents, see
[macOS](#macos)); run them individually instead if you only need one.

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

## macOS

The macOS port (native AppKit front-end, `src/mac_ui/`) and packaging are both done and released.
The phased plan and the design decisions behind the port live in
`../context-password-project/plan/macos-port-plan.md`, a sibling repo to this one (kept outside
`context-password` itself so the plan can be pulled onto another machine independently of this
repo's history) — useful background, not required reading to build the app.

### Prerequisites

- **A real Mac.** Apple licenses the macOS SDK for use on Apple hardware only.
- **Xcode Command Line Tools**: `xcode-select --install`. Provides the macOS SDK, `clang`,
  `codesign`, `lipo`, `iconutil`, `sips`, and `hdiutil`.
- **Rust**, same edition and version as Windows — `rustup default stable`, plus
  `rustup target add aarch64-apple-darwin x86_64-apple-darwin` for the universal binary `cargo
  bundle` produces.
- **`cargo-packager`** (`cargo install cargo-packager --locked`) for `.app`/`.dmg` bundling —
  invoked by `xtask`'s macOS branch, not a project dependency (same relationship `cargo-wix` has
  to the Windows side).
- **The Bitwarden CLI** (`bw`), if you plan to use the Bitwarden provider rather than KeePass:
  `brew install bitwarden-cli` (or `npm install -g @bitwarden/cli`), then `bw login` / `bw unlock`
  as in the main [README](README.md#setting-up-bitwarden). Homebrew installs to
  `/opt/homebrew/bin` on Apple Silicon or `/usr/local/bin` on Intel — neither is on the minimal
  `PATH` a GUI-launched `.app` inherits, which is why `bw::exe`'s discovery checks both locations
  plus `~/.npm-global/bin` explicitly, the same way the `bw_path` Settings override is the escape
  hatch on Windows.

### Building and packaging

`cargo build` / `cargo test` / `cargo clippy` work directly on macOS, same as Windows. `cargo
bundle` (via `xtask`) builds both architectures, combines them with `lipo`, and packages the
result:

```
cargo bundle
```

Signing is **ad-hoc by default** (`Cargo.toml`'s `[package.metadata.packager.macos]`,
`signing-identity = "-"`) — no secrets or paid account needed for a local or CI build; every build
signs, none are notarized. See [Code signing](#code-signing) below for what changes with a real
Apple Developer ID.

Cross-checking macOS-only code from Windows, without a Mac (type-checks but can't link, since
there's no SDK — still useful to catch a shared file accidentally picking up Windows-only code):

```
rustup target add aarch64-apple-darwin
cargo check --target aarch64-apple-darwin
```

## Project layout

- `src/app.rs`, `src/ui/` — the Windows front-end: the `eframe::App` implementation (popup
  show/hide and content-state machine, typing-delivery phase machine) and its pure rendering
  functions (prompt, item list, Settings, About).
- `src/mac_ui/` — the macOS front-end: native AppKit (`objc2`/`objc2-app-kit`), no egui. Owns the
  same state machines as `src/app.rs`/`src/ui/` but expressed as `NSPanel`/`NSWindow` controls
  instead of immediate-mode drawing.
- `src/controller.rs` — the platform-neutral popup state machine (`VaultState`, delivery/indicator
  phases) both front-ends drive.
- `src/vault/` — the provider-agnostic layer both credential backends build on: the shared `Entry`
  type, the tag-matching grammar that decides which vault items belong to this app's popup, and
  `VaultHandle` (spawns/respawns whichever backend is active, live, without a restart).
- `src/bw/` — the Bitwarden provider: the `bw` CLI worker thread, executable resolution, command
  building, and client-side filtering.
- `src/keepass/` — the KeePass (KDBX) provider: local unlock/list/TOTP via `keepass-core`, no
  server or CLI.
- `src/platform/win/`, `src/platform/mac/` — OS-specific: focus capture/restore, window placement,
  autostart, typing, both exposing the same module API via `src/platform/mod.rs`'s `#[cfg]` switch.
- `src/hotkey.rs`, `src/config.rs`, `src/secret.rs`, `src/msg.rs`, `src/tray.rs` — the global
  hotkey registration, on-disk config, the password-holding type, the inter-thread message enum,
  and the tray icon/menu (shared by both platforms).
- `xtask/` — the `cargo bundle` helper (see above). A separate crate, not part of the main build.
- `resources/` — icons and platform resource files (`app.ico`/`app.manifest`/`app.rc` for
  Windows, `AppIcon.icns`/`macos/entitlements.plist` for macOS), plus the two icon-regeneration
  scripts.
- `packaging/` — `installer.nsi` (Windows NSIS script). macOS's packaging config lives in
  `Cargo.toml`'s `[package.metadata.packager]` instead — `cargo-packager` needs no separate script.

## The release pipeline

`.github/workflows/release.yml` runs on a pushed `vX.Y.Z` tag (or manually, for a dry run) as four
jobs: `meta` (works out the version, checks it matches `Cargo.toml`), `release-windows` and
`release-macos` (test, lint, build, package — independently, in parallel), and `publish` (only on
an actual tag push — downloads whichever platforms' artifacts exist, writes the release notes, and
creates one GitHub Release). `release-macos` is allowed to fail or be skipped without blocking the
release — it depends on a third-party `create-dmg` script downloaded at build time and drives
Finder via AppleScript, either of which can flake on a hosted runner independent of whether the
app itself built fine — in which case `publish` still ships the Windows artifacts, and
`release-notes.sh` omits the macOS download row rather than advertising a file that isn't attached.
`.github/workflows/ci.yml` runs the test-and-lint half of that — matrixed over both platforms — on
every push and pull request, so a broken commit is caught long before anyone tags a release.

## Cutting a release

1. Bump `version` in `Cargo.toml`.
2. `cargo update --workspace --offline` to refresh `Cargo.lock`.
3. Add a dated section to `CHANGELOG.md` for the new version.
4. Commit: `git commit -am "chore(release): vX.Y.Z"`.
5. Tag: `git tag vX.Y.Z`.
6. Push: `git push --follow-tags`.

Pushing the tag triggers the release workflow, which builds and publishes both platforms.

## Code signing

**Windows** release builds are unsigned — there is no code-signing certificate for this project,
so SmartScreen warns on first run of a freshly downloaded build. Real signing would need an
Authenticode certificate (from a CA or an EV token) and a `signtool sign` step added to the
release workflow after each installer is built.

**macOS** release builds are signed ad-hoc (`signing-identity = "-"` in `Cargo.toml`) — enough to
run locally and to launch at all (an unsigned universal binary won't launch on Apple Silicon), but
a fresh download still needs the one-time unlock the README's
[Opening it the first time](README.md#opening-it-the-first-time) section describes, since ad-hoc
signing alone doesn't satisfy Gatekeeper's "identified developer" check. `release.yml` passes no
Apple secrets to `release-macos` at all, and `xtask` passes no `--config` override to
`cargo packager` either — real Developer ID signing needs both a `Cargo.toml` change (setting
`signing-identity` to the actual identity string; there's no env-var override for it, since
`cargo packager`'s `--config <json>` flag replaces the whole config rather than merging with
`[package.metadata.packager]`) and a matching `env:` block added back to `release-macos` naming
only the secrets actually configured — listing `APPLE_CERTIFICATE`/`APPLE_KEYCHAIN_PROFILE`/etc.
unconditionally is what broke this before (GitHub Actions expands an unset secret to an *empty
string*, and `cargo packager` can't tell that apart from a real one, so it fails instead of falling
back to ad-hoc). Configuring a real Apple Developer ID account for this is documented separately,
outside this repo: `context-password-project/development/apple-signing-setup.md`.

## macOS: keeping the Accessibility grant across rebuilds

`context-password` needs the Accessibility permission (`AXIsProcessTrusted`, checked in
`src/platform/mac/permissions.rs`) to post synthetic keystrokes into another app. macOS's TCC ties
that grant to the binary's *code identity*, not its path. `cargo build`'s debug output is
unsigned, and an unsigned/ad-hoc binary's identity is derived from a hash of its own bytes — so
**every rebuild is a new app as far as TCC is concerned**, and a grant given to yesterday's build
silently stops applying even though the checkbox in System Settings still looks checked.
`AXIsProcessTrusted()` then returns false and delivery aborts with `BlockReason::NoAccessibility`
— macOS dev-loop friction, not a bug in this app. (This is specific to the raw `target/debug`
binary — a `cargo bundle`-packaged `.app` is signed the same way on every build, ad-hoc or not, so
its Accessibility grant is stable across rebuilds without any of the below.)

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
| (macOS) Accessibility looks granted but delivery still fails with `NoAccessibility` | Debug builds are unsigned, so each rebuild is a new identity to TCC — see [macOS: keeping the Accessibility grant across rebuilds](#macos-keeping-the-accessibility-grant-across-rebuilds). Packaged `.app` builds don't have this problem. |
| (macOS) `cargo packager` fails immediately | It isn't installed — `cargo install cargo-packager --locked`. |
| (macOS) `cargo bundle` fails at the `lipo` step | One of the two `rustup target add` targets (`aarch64-apple-darwin`, `x86_64-apple-darwin`) is missing. |
| (macOS) The downloaded `.dmg`'s app won't open from a double-click (Gatekeeper warns or refuses) | Expected for an ad-hoc-signed, unnotarized build — confirmed with `spctl -a -vv`, which reports `rejected` for exactly this reason. See the README's [Opening it the first time](README.md#opening-it-the-first-time): System Settings → Privacy & Security → **Open Anyway** on macOS 15+, right-click → **Open** on macOS 13–14, or `xattr -dr com.apple.quarantine "Context Password.app"` on either. |
