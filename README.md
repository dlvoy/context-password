# Context Password for Bitwarden

Press a hotkey anywhere on Windows, pick a Bitwarden item from a small popup, and its password
(or username, or one-time code) is typed straight into whatever you were doing. Nothing touches
the clipboard.

## What it does

- Global hotkey (default `Ctrl+Alt+V`) opens a small popup at your mouse cursor.
- Unlock your Bitwarden vault once; it stays unlocked for the rest of the session.
- Only items you've explicitly tagged show up — see [Setting up Bitwarden](#setting-up-bitwarden).
- Enter types the password. Shift+Enter types the username. Alt+Enter fetches and types the
  current one-time code, if the item has one.
- Number keys 1-9 and 0 jump straight to an item and type it, without touching the mouse.
- Escape dismisses the popup and gives focus back to whatever you were in.
- Lives in the system tray. Lock the vault or open Settings from there.

## Install

Download the latest release from the
[releases page](https://github.com/dlvoy/context-password/releases/latest).

| File | Use |
| --- | --- |
| `ContextPassword-<version>-x64-setup.exe` | NSIS installer. Adds a Start Menu entry and an uninstaller. |
| `ContextPassword-<version>-x64.msi` | MSI installer, for deployment through Group Policy or Intune. |
| `ContextPassword-<version>-x64-portable.exe` | The application on its own, nothing to install. |

Requires 64-bit Windows 10 or 11.

These builds are unsigned, so Windows SmartScreen warns on first run. Choose **More info**, then
**Run anyway**. To verify a download against the release's `checksums.txt`:

```
certutil -hashfile <file> SHA256
```

## Setting up Bitwarden

The app talks to your vault through the official [Bitwarden CLI](https://bitwarden.com/help/cli/)
(`bw`). Install it, then:

```
bw login
bw unlock
```

You only need to unlock through the app itself afterward — `bw login` is a one-time step.

### Tagging items

The app only shows vault items you've explicitly opted in. Add a URI to a login item's URIs:

```
app://context-password/1
```

The number is the item's position in the popup list — `1` shows first, `2` second, and so on. Any
login with a URI matching this pattern shows up regardless of what other URIs it already has.

Example: a login item named "Example Server" with these URIs:

```
https://example.com
app://context-password/1
```

shows up first in the popup, labeled "Example Server". A missing or unparseable number (e.g.
`app://context-password/` with nothing after the slash) still shows the item — just sorted after
every properly numbered one, so a typo is visible rather than silently hiding the item.

## Using it

- **Hotkey** (default `Ctrl+Alt+V`, configurable in Settings): opens the popup at your cursor.
- **Locked vault**: the popup becomes a master password prompt. Type it and press Enter.
- **Unlocked vault**: the popup shows your tagged items.
  - `Enter` or a left click: type the password.
  - `Shift+Enter`: type the username.
  - `Alt+Enter`: fetch and type the current one-time code.
  - `1`-`9`, `0`: jump to that item (matching the number badge on the row) and act on it
    immediately, respecting Shift/Alt the same way Enter does.
  - `Up`/`Down`/`Home`/`End`: move the selection without acting on it.
  - `Escape`: dismiss the popup.
- **Tray icon**: Show (or Unlock, before the vault has been unlocked), Lock, Settings, About, Quit.
- **Settings**: change the hotkey, toggle "Start with Windows", auto-unlock at startup, lock on
  exit, and how many items the popup shows at once.

## Licence

MIT. See [LICENSE.txt](LICENSE.txt). The full text also ships inside the application — see it from
the tray's **About** item, alongside the version and copyright.

## Building from source

See [BUILDING.md](BUILDING.md).
