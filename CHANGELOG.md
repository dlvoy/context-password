# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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

### Changed

- The tray's Show item is labeled "Unlock" until the vault has been unlocked, and Lock only appears once it has.
- The item list is capped to a configurable number of rows (4 by default) instead of showing everything at once.
- The Settings window has larger text, more padding, and a Save/Cancel row pinned to the bottom.
