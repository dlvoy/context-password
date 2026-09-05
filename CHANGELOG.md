# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.6.0] - 2026-09-05

### Fixed

- macOS release builds are published again — `release-macos` had been disabled since 1.5.0 after
  `cargo packager`'s `--config` override turned out to replace its whole config instead of
  merging it, and to unconditionally pass empty-string Apple secrets that made notarization fail
  outright instead of falling back to ad-hoc. The DMG remains ad-hoc signed and unnotarized (no
  Apple Developer Program account); the one-time Gatekeeper unlock is now documented in the
  README's "Opening it the first time" section and linked from every release's notes. A failed or
  skipped macOS build no longer blocks the Windows release.
- On macOS, the popup's secure-input warning now names the app currently holding it instead of
  vaguely blaming "the target field" — Secure Input is a session-wide flag, not scoped to any one
  field — and is re-checked right before typing instead of relying on a stale check from when the
  popup opened, so a warning that already cleared no longer blocks Enter for no visible reason.
- Selected rows in the macOS popup no longer wash out their delivery icon and number badge against
  the blue selection highlight.

## [1.5.0] - 2026-09-04

### Added

- macOS builds can now be packaged into a proper `.app`/`.dmg`, with an app icon and ad-hoc code
  signing by default — this is what "Open at Login" needs, since `SMAppService` requires a real
  app bundle rather than a bare dev binary. See `BUILDING.md` for how to build one, and the
  Developer ID signing doc referenced there for notarized builds.

## [1.4.0] - 2026-09-04

### Added

- macOS support: a native AppKit front-end (global hotkey, popup, tray, Settings, About), running
  alongside the existing Windows build.
- A KeePass (KDBX) vault provider, selectable alongside Bitwarden in Settings on both platforms —
  unlocks a local `.kdbx` file directly, no server or CLI required. Entries are tagged the same
  way as Bitwarden (a URL or a `context-password` custom field), with optional TOTP support via
  an `otp` custom field.

## [1.3.0] - 2026-09-02

### Added

- Tag URIs accept an optional `?os=` filter (`app://context-password/1?os=win`, or a comma list
  like `?os=win,mac`) to restrict an item to one or more systems, for vaults shared across
  platforms. Omitting it shows an item everywhere, as before. An unrecognized value hides the item
  and is counted in the existing hidden-item log; an item hidden for a different, well-formed OS
  filter is not.

## [1.2.0] - 2026-08-30

### Added

- A tray Sync item, shown below Show once the vault is unlocked, that refreshes the cached item list against Bitwarden without requiring the master password again. The popup shows the same progress spinner as unlocking while it runs.

## [1.1.0] - 2026-08-30

### Added

- A small progress indicator near the cursor while autotype is running: a keyboard icon while typing is in progress, then a green checkmark for a couple of seconds once it's done.

### Fixed

- Pressing Shift+1 in the item list now types the selected item's username, matching every other Shift+digit combination.
- A brief empty window no longer flashes near the top-left of the screen when the app starts.

## [1.0.1] - 2026-08-25

### Fixed

- The release workflow's NSIS installer build, which failed to find `makensis` right after installing it.

## [1.0.0] - 2026-08-25

### Added

- A proper application and tray icon, based on the project's logo.
- An About screen, from the tray, showing the app's version, license, and copyright.
- A LICENSE file (MIT), with the full text also shown in the About screen.
- Prebuilt Windows installers (MSI and NSIS), each with a desktop shortcut and a launch-after-install option, both on by default.
- README and BUILDING documentation, including how to set up the Bitwarden CLI and tag vault items so they show up in the popup.
- A GitHub Actions workflow that builds and publishes releases from a version tag.
- A system tray icon with a Show and a Quit menu item.
- A configurable global hotkey (default Ctrl+Alt+V) that pops up a small window at the mouse cursor and takes keyboard focus.
- Pressing Escape dismisses the popup and returns focus to whatever was focused before it appeared.
- Unlock your Bitwarden vault from the popup and select a tagged item to have its password typed automatically into whatever was focused.
- The vault stays unlocked for the rest of the session, so later hotkey presses skip straight to the item list.
- A warning when the previously focused window is running elevated, since typing into it would otherwise silently do nothing.
- The popup follows your Windows light or dark theme.
- The popup stays fully on screen and at a consistent size regardless of which monitor or display scaling it's summoned on.
- A Settings window, opened from the tray, for changing the global hotkey and toggling "Start with Windows" — both take effect immediately, no restart needed.
- A tray Lock action that clears the cached vault items and locks the vault, so the master password is required again next time.
- Shift+Enter types the selected item's username, and Alt+Enter fetches and types its current one-time code — each shown with its own icon on the list.
- Number keys 1–9 and 0 jump straight to an item in the list and type it immediately, matching a numbered badge on each row.
- A "Max visible items" control and a "Lock vault on exit" option in Settings.
- Visible progress in the popup while the vault is being locked.
- A reveal-toggle eye icon on the password field, to check what you've typed before pressing Enter.
- "Auto-unlock at start" is now a Settings checkbox (on by default) instead of a config file setting.

### Changed

- The tray's Show item is labeled "Unlock" until the vault has been unlocked, and Lock only appears once it has.
- The item list is capped to a configurable number of rows (4 by default) instead of showing everything at once.
- The Settings window has larger text, more padding, and a Save/Cancel row pinned to the bottom.
- The Settings window is bigger and its padding more consistent around the Save/Cancel row.
- Clicking an item in the list now delivers it immediately, the same as pressing Enter or its number key.

### Fixed

- The popup no longer resets to an empty prompt if the delayed auto-unlock timer fires while you're already typing the master password.
- The item list is now sized correctly right after unlocking, instead of only from the next time the popup opens.
