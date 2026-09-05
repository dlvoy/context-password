# Context Password for Bitwarden

Press a hotkey anywhere on Windows or macOS, pick a Bitwarden item from a small popup, and its
password (or username, or one-time code) is typed straight into whatever you were doing. Nothing
touches the clipboard.

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

### Windows

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

### macOS

| File | Use |
| --- | --- |
| `ContextPassword-<version>-universal.dmg` | Disk image — open it, then drag Context Password into Applications. Runs on both Apple Silicon and Intel. |

Requires macOS 13 (Ventura) or later.

This build is signed ad-hoc, not notarized — there's no Apple Developer Program membership behind
this project, so Gatekeeper's "identified developer" check fails by design and it blocks the app on
first launch. See [Opening it the first time](#opening-it-the-first-time) below for the one-time
steps to allow it; this doesn't repeat on later launches of the same copy, though it does for each
new version you download.

To verify a download against the release's `checksums.txt`:

```
shasum -a 256 -c checksums.txt
```

#### Opening it the first time

Because the build is ad-hoc signed rather than notarized, Gatekeeper blocks it outright — a plain
double-click says the app "is damaged" or "cannot be opened" (a misleading message; nothing is
actually wrong with it). Which workaround applies depends on your macOS version:

**macOS 15 (Sequoia) and later:** double-click the app, dismiss the warning dialog, then go to
System Settings → **Privacy & Security**, scroll to the Security section, and click **Open
Anyway** next to the mention of Context Password. Confirm with your password or Touch ID, then
**Open Anyway** once more in the dialog that follows.

**macOS 13–14 (Ventura/Sonoma):** right-click (or Control-click) the app in Applications and choose
**Open**, then **Open** again in the dialog. Right-clicking still works on later versions too, but
no longer clears Gatekeeper's block on its own the way it used to.

**Either version, from the Terminal:** remove the quarantine flag directly, which skips both
dialogs:

```
xattr -dr com.apple.quarantine "/Applications/Context Password.app"
```

The first time the app runs, macOS will ask for the **Accessibility** permission (System Settings
→ Privacy & Security → Accessibility) — required to type into other applications; without it,
delivery is blocked and the popup says so. **Secure Input** is a separate, session-wide macOS flag
— when any app currently has a password field mid-edit (or otherwise turns it on), synthetic
keystrokes are blocked into *every* app on the system, not just that one, regardless of
Accessibility. The popup flags this too, naming the app holding it when it can, rather than
silently doing nothing.

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

### Restricting an item to one or more systems

If you share a vault across a Windows machine and another platform, append `?os=` to limit where an
item shows up:

```
app://context-password/2?os=win
app://context-password/3?os=win,mac
```

Supported tokens: `win`, `mac`, `linux`, `android`, `ios` — comma-separated for more than one, and
matching is case-insensitive. Leaving `?os=` off (as in the examples above) shows the item on every
system, unchanged from before. An item can even carry more than one tag uri to get a different
position per platform, e.g. `app://context-password/1?os=win` alongside
`app://context-password/5?os=mac`.

An unrecognized value (`?os=beos`, or `?os=` with nothing after the `=`) hides the item and counts
it in the "N item(s) hidden — see log" line, the same as any other malformed tag — so a typo is
visible instead of silently doing nothing. An item correctly filtered out for a *different* system
(e.g. `?os=mac` on this Windows build) is not counted there at all; it's working as intended, not an
error.

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
- **Settings**: switch vault provider (Bitwarden or KeePass), change the hotkey, toggle "Start
  with Windows" (or "Open at Login" on macOS — only available from the packaged `.app`, not a raw
  build), auto-unlock at startup, lock on exit, and how many items the popup shows at once.

## Licence

MIT. See [LICENSE.txt](LICENSE.txt). The full text also ships inside the application — see it from
the tray's **About** item, alongside the version and copyright.

## Building from source

See [BUILDING.md](BUILDING.md).
