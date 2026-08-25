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
