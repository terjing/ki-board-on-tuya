# Companion on macOS

The Companion (`companion/`, Tauri: Rust + React/TS) has a native macOS target.
It does the same job as on Windows: read the keyboard over USB serial or BLE,
route board events into the Kiro Crew window, and reflect Agent status back to
the board. Only the OS-specific layer differs:

| Concern | Windows | macOS |
|---------|---------|-------|
| Key injection | Win32 `SendInput` | CGEvent, posted at the HID tap |
| Focusing the Kiro Crew window | Win32 window search by title | Accessibility (AX) API, window level |
| Voice input | `Win+H` | macOS Dictation (Edit menu item or a double-key trigger) |
| Hook command | `"<exe>" --hook` | `'<exe>' --hook` (POSIX single quotes) |
| Open a folder | `explorer.exe` | `open` (Finder) |

Windows behaviour is unchanged by the macOS work. Linux is only a compile and
test host (stubs, no injection).

> Status: the macOS code is cross-checked (`cargo check` for
> `aarch64-apple-darwin` and `x86_64-apple-darwin`) but has **not been run on a
> Mac yet**. See [Known limitations](#known-limitations) and the
> [smoke checklist](#manual-smoke-checklist).

## Prerequisites

- macOS 12 (Monterey) or later. The bundle sets `minimumSystemVersion` to 12.0.
- Xcode Command Line Tools: `xcode-select --install`.
- Rust 1.98 (pinned by `rust-toolchain.toml`) with both Apple targets, which a
  universal build needs:

  ```bash
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  ```

- Node 22.

## Run in development

```bash
cd companion
npm ci
npm run dev        # tauri dev
```

Run the Rust unit tests with `cd src-tauri && cargo test`.

## Build a distributable app

```bash
cd companion
npm ci
npm run build:mac  # tauri build --target universal-apple-darwin --bundles app,dmg
```

The `.app` and `.dmg` are written to:

```text
companion/src-tauri/target/universal-apple-darwin/release/bundle/macos/
companion/src-tauri/target/universal-apple-darwin/release/bundle/dmg/
```

macOS-only bundle settings live in `companion/src-tauri/tauri.macos.conf.json`
(targets `app` and `dmg`, minimum system version 12.0, `.icns` icon) and are
merged over `tauri.conf.json`, which is shared with Windows. The Bluetooth usage
string is in `companion/src-tauri/Info.plist`.

### Gatekeeper

The bundle is signed **ad hoc** (`signingIdentity "-"`), not with a Developer ID,
and is not notarised. A copy downloaded or moved from another machine is
quarantined, and macOS refuses to open it by default. Either:

- right-click the app and choose **Open**, then confirm; or
- clear the quarantine flag:

  ```bash
  xattr -dr com.apple.quarantine "/Applications/Agentic Keyboard Companion.app"
  ```

## Permissions

### Accessibility (required)

One Accessibility grant covers both finding/raising the Kiro Crew window and
posting keystrokes. The Companion checks for it, and does not prompt at launch:

- If it is missing, the Companion window shows a warning banner ("Accessibility
  permission is missing — the Companion cannot focus the Kiro Crew window or
  inject keystrokes until it is granted.") with a **Grant Accessibility**
  button, and the log gets a `[MAC] Accessibility permission missing — …` line.
- The button shows the system prompt once and opens
  **System Settings → Privacy & Security → Accessibility**. Enable the
  Companion there.
- Every focus, inject or dictation action checks the grant first. Without it you
  get an explicit "Accessibility permission not granted — nothing injected"
  (or "— cannot focus window" / "— dictation not started") error, not a
  misleading "no window matches".

Things that bite:

- **Dev runs from Terminal:** the process that needs the grant is the one that
  launched the app. Grant **Terminal** (or iTerm, VS Code, …), not only the
  Companion.
- **Rebuilding resets the grant.** Ad-hoc signatures change on every build, and
  macOS then treats it as a new app. Remove the old entry in the Accessibility
  list, then add and enable it again.
- **Secure Event Input blocks injection.** While a password field is focused
  (any app, including a terminal with "Secure Keyboard Entry" on), the system
  discards synthetic key events. Click out of the field.

### Bluetooth

Using BLE makes macOS ask for Bluetooth access on first scan. The prompt text is
"Agentic Keyboard Companion uses Bluetooth to connect to the AK keyboard." If
you declined it, re-enable it under **System Settings → Privacy & Security →
Bluetooth**. For a dev run from Terminal the grant may need to be on Terminal.

Before scanning, the Companion waits up to 3 s for the Bluetooth adapter to be
powered on.

### Dictation

Voice input uses macOS Dictation instead of `Win+H`, so it must be enabled
first: **System Settings → Keyboard → Dictation**. No microphone usage string
is added by the Companion; the system handles that.

Choose how the Companion starts it with the **macOS dictation trigger**
dropdown (shown only on macOS, in the voice-input section; saved in
`~/.ak-companion/binds.json` as `mac_dictation_trigger`). It is used when the
voice source is **PC system ASR** and you press K4. The log line names the
trigger, for example `macOS Dictation (edit-menu)`:

| Trigger | Wire name | How it works |
|---------|-----------|--------------|
| Edit menu (default) | `edit-menu` | Presses the **Start Dictation…** item (or **Stop Dictation…** when it is present, so K4 toggles) in the menu bar of the app that owns the Kiro Crew window. Works with any dictation shortcut. Items are matched in English and Simplified Chinese (开始听写 / 停止听写), falling back to any item whose title contains "dictation" or 听写. |
| Double Control | `double-control` | Two quick left-Control taps. |
| Double Right Command | `double-right-command` | Two quick right-⌘ taps. |
| Double Left Command | `double-left-command` | Two quick left-⌘ taps. |

An unknown or empty saved value falls back to Edit menu.

- The double-key options are best effort and must match the shortcut chosen in
  System Settings → Keyboard → Dictation. The UI hint reads: "Edit-menu works
  with any shortcut; double-key options must match System Settings → Keyboard →
  Dictation. Fn/🌐 cannot be simulated."
- **Fn / 🌐 (twice) is not offered.** The Globe key is handled below the level
  that synthetic events can reach.
- Dictation only starts in the Kiro Crew window. If that window cannot be found,
  the Companion refuses and logs `Kiro Crew window not found — dictation not
  started`; it never starts dictation in itself (`refusing to start dictation
  in the Companion's own window`). If the Edit-menu item cannot be found, the
  error suggests switching to a double-key trigger.

## Chord mapping

Kiro Crew has separate macOS and non-macOS shortcut bindings (taken from Kiro
Crew 0.9.0). The Companion sends the macOS ones, and its UI shows the macOS
labels (`⌃1–9` for chat jumps, **Command bar (⌘K)** for the test button).
"Physical" keys are matched by key position, "Char" keys by the character your
current layout produces.

| Board action | Windows | macOS | Kind |
|--------------|---------|-------|------|
| Enter / Esc / arrows / Backspace | same keys | Return / Esc / arrows / Delete | physical |
| Delete word | Ctrl+Backspace | **⌥ Delete** | physical |
| Clear input | Ctrl+A, Backspace | **⌘A**, Delete | Char `a` |
| Command bar | Ctrl+K | **⌘K** | Char `k` |
| Jump to chat N (1–9) | Alt+N | **⌃N** | physical |
| Previous / next chat | Alt+← / Alt+→ | ⌥← / ⌥→ | physical |
| Focus input | Alt+Enter | ⌥ Return | physical |
| New chat | Alt+Shift+N | ⌥⇧N | physical |

Why Cmd instead of Ctrl for the Char keys: on macOS Ctrl+A moves the caret to
the line start and Ctrl+K kills to the end of the line, so they would not do
what the board asks.

Why the layout lookup matters: ⌘A and ⌘K are resolved through your *current*
keyboard layout (an input method such as Pinyin falls back to an ASCII-capable
layout). A hard-coded US keycode would send ⌘Q on AZERTY, which quits the
browser. The Companion keeps a small cache of those two keycodes, refreshes it
when the system reports a layout change, and also on every status poll. If a
keycode is not in the cache, it **fails closed**: nothing is posted and the log
shows `keyboard layout key 'k' not resolved yet — nothing injected (open the
Companion window once)`.

## Serial (USB)

macOS lists each serial device twice, as `/dev/tty.*` (dial-in) and
`/dev/cu.*` (callout). Auto-discovery prefers `/dev/cu.*`, and the port dropdown
hides a `tty.*` entry when its `cu.*` twin is listed. Use the `cu` one if you
pick a port by hand. Windows `COM` names are unaffected.

Drivers for the USB-to-UART bridge on the board:

- **CH340:** built into macOS 11 and later, no install.
- **CH342F / CH343:** install WCH's `CH34xVCPDriver` from the vendor, then
  re-plug the board. These chips expose two UARTs; if the first port is silent,
  try the other.

## Bluetooth (BLE)

CoreBluetooth does not expose MAC addresses, so BLE device ids in the UI and in
`binds.json` are CoreBluetooth UUIDs, not `AA:BB:CC:…` addresses. They are stable
per Mac, but differ from the ids on Windows and on another Mac.

If a connection reports an empty GATT table, the log says "none — macOS
returned an empty GATT table (toggle Bluetooth or forget the AK-KB device and
re-scan)". Toggle Bluetooth off and on, or forget the AK-KB device in System
Settings → Bluetooth, and scan again.

## Kiro Crew hook

The status hook is the Companion binary itself, run with `--hook`. On macOS Kiro
Crew runs it through `/bin/sh -c`, so the stored command is the executable path
in single quotes:

```text
'/Applications/Agentic Keyboard Companion.app/Contents/MacOS/<executable>' --hook
```

`<executable>` is the main binary inside the bundle's `Contents/MacOS/`. Use
the Companion's **Install globally** button (it installs the Global Crew hook) rather than typing this by hand; it
records the path of the running executable in `~/.kiro/crew/hooks.json`, and
**Open Crew folder** shows that folder in Finder.

If you move or rename the `.app`, install the hook again, or Kiro Crew keeps
calling the old path. A dev run (`npm run dev`) records the dev binary under
`src-tauri/target/`, so reinstall the hook when you switch to the built app.

## Known limitations

- **Not run on a real Mac yet.** Linking, TCC prompts, real keystrokes, focus,
  dictation, CoreBluetooth and serial enumeration have not been verified.
- **A layout switch has a millisecond-scale stale-keycode gap.** The Companion
  clears and refreshes its keycode cache as soon as macOS reports a layout
  change. In the few milliseconds between the switch and that notification, a
  board shortcut that depends on the layout (ClearInput = ⌘A, and the ⌘K command
  bar) still uses the previous layout's keycode. If the notification were ever
  missed, the next board action would also use the old keycode before its queued
  refresh runs. This is not fail-closed. Shortcuts that use fixed physical keys
  (Enter, arrows, delete word, chat jumps, new chat) are not affected.
- **Layouts without Latin keys fail closed for ⌘A / ⌘K.** If the current layout
  has no `a` / `k` under ⌘ (for example plain Russian), the keycode cannot be
  resolved, so ClearInput and the ⌘K command bar inject **nothing** and log
  `keyboard layout key … not resolved yet`. This is deliberate: a guessed
  keycode could send ⌘Q. Switch to a Latin layout to use those two shortcuts.
- **UI test buttons may stutter.** The window-bind, send-text, command-bar and
  chord test buttons run focus and a short settle wait on the UI thread, so the
  window can hitch briefly per click. Board-driven actions do not freeze the UI,
  but they hold the Companion's state lock, so a status refresh can wait for one
  action to finish (also true on Windows).
- **Focus may be refused.** macOS 14+ cooperative activation can stop one app
  raising another. The Companion polls for the frontmost app and reports the
  failure ("found window … but macOS did not bring it to the front — nothing
  injected") instead of typing into the wrong window.
- **IME input.** Text is typed as Unicode key events; with an input method active
  (for example Pinyin) the result can be altered. There is no clipboard-paste
  fallback.
- **Shortcut collisions.** Ctrl+digit can clash with Mission Control's "Switch to
  Desktop N", and ⌥← / ⌥→ with word movement in the composer. Kiro Crew's
  bindings may also change between versions.
- **Dictation.** The Edit-menu item depends on the target app having one, in a
  language the matcher knows. Double-key triggers depend on System Settings.
  Fn / 🌐 is not supported.
- **Icon.** The `.icns` is scaled up from a 128 px source and looks soft.
- **Window title.** The Kiro Crew window is found by a case-insensitive title
  substring (default `Kiro Crew`, then `Agentic`). The Companion skips its own
  window.

## Manual smoke checklist

1. Launch the app.
2. Grant Accessibility with the banner button.
3. Bind the window "Kiro Crew".
4. Send text from the UI.
5. Layout-dependent chords, for each of two paths: the UI **Command bar (⌘K)**
   button, and a board **nav-up long** press (ClearInput, ⌘A then Delete). For
   each path, test on a US layout; then switch to AZERTY, wait 2 s with the
   Companion window visible and repeat (it must not send ⌘Q or quit the
   browser); with Pinyin active; and with a layout that has no Latin keys under
   ⌘ (for example plain Russian), where the chords must inject nothing and log
   `not resolved`. Then press nav-up long 20 times with the UI
   open: none may log `layout key … not resolved`.
6. K1 jumps to chat 1 (⌃1).
7. Hold nav-left: it deletes words.
8. Nav-up long clears the input.
9. K4 with the System source toggles dictation.
10. With the Companion frontmost and Kiro Crew closed, K4 must **not** start
    dictation in the Companion; the error is logged.
11. USB connects on `/dev/cu.*`.
12. BLE scans and connects.
13. Install the hook (**Install globally**), then **Open Crew folder** opens Finder.
