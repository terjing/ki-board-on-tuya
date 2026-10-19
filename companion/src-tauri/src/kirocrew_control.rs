//! FP-4b Route B — the *hands*: drive a **local** Kiro Crew window by focusing
//! it and injecting keystrokes (no HTTP, no session-control API). This is the
//! control plane that supersedes route A for the actual control path.
//!
//! Two layers live here:
//!
//! * [`KiroCrewController`] — macOS-gated backend that finds the Kiro Crew
//!   window by a configurable title substring, brings it to the foreground
//!   (via AppleScript / System Events through `osascript`), and injects text /
//!   the command bar (Cmd+K) / named chords. Key injection reuses the raw
//!   CoreGraphics / ApplicationServices FFI pattern from [`crate::inject`]
//!   (`CGEventCreateKeyboardEvent` + `CGEventPost`), so no new crate is pulled
//!   in. On non-macOS hosts a stub compiles and returns `Err` / no-ops so the
//!   Linux CI build and the pure router tests below still work.
//!
//!   Runtime note: the macOS backend requires both **Accessibility** (to post
//!   synthetic key events) and **Automation** permission (so `osascript` may
//!   drive System Events) in System Settings > Privacy & Security.
//!
//! * [`ControlRouter`] / [`route_input`] — a **pure**, IO-free mapping from a
//!   decoded board [`Inbound`] event to a [`ControlAction`]. Table-driven and
//!   unit-tested on Linux (the tests at the bottom run in `cargo test`).
//!
//! Design ref: `.kiro/specs/feature-probes-kirocrew-local-control/design.md`.

use crate::protocol::{Inbound, KeyAction, NavDir};

// ───────────────────────────── Chord ─────────────────────────────

/// A named key / key-combination the controller can inject. Kept small and
/// `Copy` so the pure router can hand one to the (macOS) backend by value.
///
/// The variant names are platform-neutral; the actual macOS modifier used for
/// each is defined in the `macos_impl` chord table (Cmd / Option / Shift). The
/// `CtrlK` variant name is historical (it was Ctrl+K on Windows); on macOS it
/// maps to **Cmd+K**.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Chord {
    Enter,
    Esc,
    Up,
    Down,
    Left,
    Right,
    /// Backspace — delete the character before the composer caret.
    Backspace,
    /// Delete the word before the composer caret. macOS: Option+Backspace.
    DeleteWord,
    /// Clear the current composer sentence. macOS: Cmd+A, then Backspace.
    ClearInput,
    /// Open the Kiro Crew command bar. macOS: Cmd+K (name is historical).
    CtrlK,
    /// Jump to chat N (N = 1..=9). Deterministic, unlike a command-bar text
    /// search. macOS: Cmd+N digit ("Jump to chat N").
    JumpChat(u8),
    /// Previous chat. macOS: Cmd+Shift+[ (the macOS "previous tab" convention).
    PrevChat,
    /// Next chat. macOS: Cmd+Shift+] (the macOS "next tab" convention).
    NextChat,
    /// Focus the chat text input (composer). macOS: Cmd+Return.
    FocusInput,
    /// New chat. macOS: Cmd+N.
    NewChat,
}

impl Chord {
    /// Parse the wire / UI name (case-insensitive) into a chord. Returns `None`
    /// for an unrecognized name so callers can reject rather than guess.
    pub fn parse(name: &str) -> Option<Chord> {
        match name.trim().to_ascii_lowercase().as_str() {
            "enter" | "return" => Some(Chord::Enter),
            "esc" | "escape" => Some(Chord::Esc),
            "up" => Some(Chord::Up),
            "down" => Some(Chord::Down),
            "left" => Some(Chord::Left),
            "right" => Some(Chord::Right),
            "backspace" | "delete_char" | "delete-char" => Some(Chord::Backspace),
            "deleteword" | "delete_word" | "delete-word" => Some(Chord::DeleteWord),
            "clearinput" | "clear_input" | "clear-input" => Some(Chord::ClearInput),
            "ctrlk" | "ctrl+k" | "command_bar" | "commandbar" => Some(Chord::CtrlK),
            "prevchat" | "prev_chat" | "prev-chat" => Some(Chord::PrevChat),
            "nextchat" | "next_chat" | "next-chat" => Some(Chord::NextChat),
            "focusinput" | "focus_input" | "focus-input" => Some(Chord::FocusInput),
            "newchat" | "new_chat" | "new-chat" => Some(Chord::NewChat),
            other => {
                // "chat1".."chat9" -> JumpChat(1..=9)
                if let Some(d) = other.strip_prefix("chat") {
                    if let Ok(n) = d.parse::<u8>() {
                        if (1..=9).contains(&n) {
                            return Some(Chord::JumpChat(n));
                        }
                    }
                }
                None
            }
        }
    }
}

// ─────────────────────── Pure ControlRouter (T2) ───────────────────────

/// The action the router decided a board event should produce. Consumed by the
/// (macOS) [`KiroCrewController`]; pure and IO-free so it is fully testable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlAction {
    /// Open the command bar (Cmd+K on macOS), type the string, press Enter.
    CommandBar(String),
    /// Inject a single named chord.
    Chord(Chord),
    /// Type free text into the focused input (no command bar, no Enter).
    SendText(String),
    /// Focus the chat composer, then type the text — no Enter, so the user
    /// reviews and submits with K5. This is the K4 voice path.
    Compose(String),
    /// Start repeating the DeleteWord chord (Option+Backspace on macOS) while
    /// joystick-left remains held.
    StartDeleteWords,
    /// Stop a previously started continuous word deletion.
    StopDeleteWords,
    /// Nothing to do for this event (e.g. a key *up*, a ping, an ack).
    None,
}

/// Router context — the small amount of state the pure mapping needs but does
/// not own. `jump_chats[i]` is the chat NUMBER (1..=9) that K(i+1) (K1..K3)
/// jumps to via the deterministic jump-to-chat shortcut (Cmd+N on macOS).
/// Defaults to `[1, 2, 3]`.
/// The number for each key is user-configurable (persisted in the bind store)
/// and kept in sync here by `lib.rs`.
#[derive(Clone, Debug, Default)]
pub struct ControlCtx {
    /// Chat numbers for K1, K2, K3 (index 0..=2). Configurable by the UI.
    pub jump_chats: Vec<u8>,
}

impl ControlCtx {
    /// Sensible defaults: K1->chat 1, K2->chat 2, K3->chat 3 (Cmd+1/2/3).
    pub fn with_default_jumps() -> Self {
        Self { jump_chats: vec![1, 2, 3] }
    }

    /// Deterministic jump-to-chat for slot `idx` (0-based). Returns
    /// `Chord(JumpChat(n))` when `jump_chats[idx]` is a valid chat number
    /// (1..=9); `None` when the index is missing or the number is out of range.
    fn jump(&self, idx: usize) -> ControlAction {
        match self.jump_chats.get(idx) {
            Some(&n) if (1..=9).contains(&n) => ControlAction::Chord(Chord::JumpChat(n)),
            _ => ControlAction::None,
        }
    }
}

/// Is this a *triggering* key edge? We act on `down` / `long`, never on `up`
/// (so a single physical press produces exactly one action).
fn is_trigger(action: KeyAction) -> bool {
    matches!(action, KeyAction::Down | KeyAction::Long)
}

/// Must match firmware/app/common/include/ak_input_fsm.h.
const BOARD_LONG_PRESS_MS: u32 = 800;

/// Pure mapping: board event + context → control action. No IO.
///
/// Mapping table (design R2.3), using Kiro Crew's real default shortcuts:
///
/// | event                          | action                              |
/// |--------------------------------|-------------------------------------|
/// | `macro { name }` (recognized text) | `Compose(name)` — K4 voice → focus composer + type (no send) |
/// | `key k1..k3` `Click`           | `Chord(JumpChat(n))` — n = configured `jump_chats[idx]` (Cmd+N on macOS, client-side jump to chat n); `None` if unset / out of 1..=9 |
/// | `key k1..k3` `Down`/`Long`/`Up`/`Dclick` | `None` — jump fires only on `Click`; `Long` is reserved for long-press auto-bind |
/// | `key k4` (down/long)           | `None` — K4 is push-to-talk; text arrives later as `macro` |
/// | `key k5` (down/long)           | `Chord(Enter)` — submit / continue  |
/// | `nav left` `Up` (< 800 ms)  | `Chord(Backspace)` — delete one character |
/// | `nav left` `Long`           | `StartDeleteWords` — repeat Option+Backspace |
/// | `nav left` `Up` (>= 800 ms) | `StopDeleteWords` — stop repeat            |
/// | `nav up` `Up` (< 800 ms)    | `Chord(Up)` — move to previous line      |
/// | `nav up` `Long`             | `Chord(ClearInput)` — Cmd+A, Backspace   |
/// | `nav right` `Down`          | `Chord(NextChat)` — next chat (Cmd+Shift+]) |
/// | `nav down` `Down`           | `Chord(Down)` — arrow key                |
/// | `nav center` `Down`         | `Chord(Enter)`                            |
/// | anything else / key `up`    | `None`                                    |
pub fn route_input(ev: &Inbound, ctx: &ControlCtx) -> ControlAction {
    match ev {
        // Recognized speech text -> type into the chat composer (NO send). Both
        // the board-side cloud ASR (`asr` frame) and the PC dictation/macro path
        // (`macro` frame) land here, so BOTH voice routes behave identically:
        // the text appears in the Kiro Crew composer and the user submits it
        // with K5 (Enter). No auto-send, so a misrecognition can be reviewed.
        Inbound::Asr { payload, .. } => {
            let text = payload.text.trim();
            if text.is_empty() {
                ControlAction::None
            } else {
                ControlAction::Compose(text.to_string())
            }
        }
        // K4's recognized dictation → type into the chat composer (no send).
        Inbound::Macro { payload, .. } => {
            let text = payload.name.trim();
            if text.is_empty() {
                ControlAction::None
            } else {
                ControlAction::Compose(text.to_string())
            }
        }

        // K1-K3 = jump keys. They fire on `Click` ONLY — a confirmed single
        // click (short press). `Down`/`Long`/`Up`/`Dclick` are no-ops, so a
        // long-press never jumps (it drives the companion long-press auto-bind).
        // Matched BEFORE the generic `is_trigger` arm so Down/Long cannot leak.
        Inbound::Key { payload, .. }
            if matches!(payload.id.as_str(), "k1" | "k2" | "k3") =>
        {
            let idx = match payload.id.as_str() {
                "k1" => 0usize,
                "k2" => 1usize,
                _ => 2usize,
            };
            match payload.action {
                // Click -> jump to the configured chat number for this key.
                // The jump fires on CLICK (short-press release confirmed as a
                // single click), NOT on Down, so a long-press (which produces
                // `long` and no `click`) never jumps — it is reserved for the
                // long-press auto-bind flow in the companion UI.
                KeyAction::Click => ctx.jump(idx),
                // Down / Long / Up / Dclick (and anything else) -> nothing.
                _ => ControlAction::None,
            }
        }

        Inbound::Key { payload, .. } if is_trigger(payload.action) => {
            match payload.id.as_str() {
                // K4 = push-to-talk: pressing it starts voice; the recognized
                // text comes back later as a `macro` frame (handled above).
                "k4" => ControlAction::None,
                // K5 = submit / continue.
                "k5" => ControlAction::Chord(Chord::Enter),
                _ => ControlAction::None,
            }
        }

        // Composer editing after ASR insertion. A short Left action is delayed
        // until Up so a future Long event can replace it rather than deleting
        // one character first. The long-tail Up is ignored via held_ms.
        Inbound::Nav { payload, .. } => match (payload.dir, payload.action) {
            (NavDir::Left, KeyAction::Up) if payload.held_ms < BOARD_LONG_PRESS_MS => {
                ControlAction::Chord(Chord::Backspace)
            }
            (NavDir::Left, KeyAction::Long) => ControlAction::StartDeleteWords,
            (NavDir::Left, KeyAction::Up) => ControlAction::StopDeleteWords,
            (NavDir::Up, KeyAction::Up) if payload.held_ms < BOARD_LONG_PRESS_MS => {
                ControlAction::Chord(Chord::Up)
            }
            (NavDir::Up, KeyAction::Long) => ControlAction::Chord(Chord::ClearInput),
            (NavDir::Right, KeyAction::Down) => ControlAction::Chord(Chord::NextChat),
            (NavDir::Down, KeyAction::Down) => ControlAction::Chord(Chord::Down),
            (NavDir::Center, KeyAction::Down) => ControlAction::Chord(Chord::Enter),
            _ => ControlAction::None,
        },

        // Key/nav `up` edges, voice edges, ping/pong/ack, hello_ack, unknown.
        _ => ControlAction::None,
    }
}

/// Thin stateful wrapper so callers can hold configuration (the `ControlCtx`)
/// alongside the routing entry point. Pure — no IO.
#[derive(Clone, Debug, Default)]
pub struct ControlRouter {
    pub ctx: ControlCtx,
}

impl ControlRouter {
    pub fn new(ctx: ControlCtx) -> Self {
        Self { ctx }
    }

    pub fn route(&self, ev: &Inbound) -> ControlAction {
        route_input(ev, &self.ctx)
    }
}

// ─────────────────────── KiroCrewController (T1) ───────────────────────

/// Default window-title substrings tried in order when none is configured.
pub const DEFAULT_TITLE_CANDIDATES: &[&str] = &["Kiro Crew", "Agentic"];

// =============================== macOS ===============================

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{Chord, DEFAULT_TITLE_CANDIDATES};
    use std::os::raw::c_void;
    use std::process::Command;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    // ── macOS virtual keycodes (ANSI, from <HIToolbox/Events.h>) ──
    //
    // These are the hardware keycodes CoreGraphics expects, NOT ASCII. The
    // digit keycodes are intentionally out of numeric order (that is the real
    // ANSI layout order).
    const VK_RETURN: u16 = 0x24; // kVK_Return
    const VK_ESCAPE: u16 = 0x35; // kVK_Escape
    const VK_UP: u16 = 0x7E; // kVK_UpArrow
    const VK_DOWN: u16 = 0x7D; // kVK_DownArrow
    const VK_LEFT: u16 = 0x7B; // kVK_LeftArrow
    const VK_RIGHT: u16 = 0x7C; // kVK_RightArrow
    const VK_DELETE: u16 = 0x33; // kVK_Delete (Backspace)
    const VK_A: u16 = 0x00; // kVK_ANSI_A
    const VK_K: u16 = 0x28; // kVK_ANSI_K
    const VK_N: u16 = 0x2D; // kVK_ANSI_N
    const VK_LEFT_BRACKET: u16 = 0x21; // kVK_ANSI_LeftBracket  ('[')
    const VK_RIGHT_BRACKET: u16 = 0x1E; // kVK_ANSI_RightBracket (']')
    /// Keycodes for the digit keys '1'..'9', in digit order (index 0 == '1').
    /// From <HIToolbox/Events.h>: kVK_ANSI_1..9 are NOT contiguous.
    const VK_DIGIT: [u16; 9] = [
        0x12, // '1'
        0x13, // '2'
        0x14, // '3'
        0x15, // '4'
        0x17, // '5'
        0x16, // '6'
        0x1A, // '7'
        0x1C, // '8'
        0x19, // '9'
    ];

    // ── CGEventFlags modifier masks (from CoreGraphics CGEventTypes.h) ──
    const FLAG_SHIFT: u64 = 1 << 17; // kCGEventFlagMaskShift
    #[allow(dead_code)]
    const FLAG_CONTROL: u64 = 1 << 18; // kCGEventFlagMaskControl
    const FLAG_OPTION: u64 = 1 << 19; // kCGEventFlagMaskAlternate (Option/Alt)
    const FLAG_COMMAND: u64 = 1 << 20; // kCGEventFlagMaskCommand (⌘)

    // `kCGHIDEventTap == 0`: post into the HID stream so the frontmost app sees
    // the synthetic events as real hardware input.
    const KCG_HID_EVENT_TAP: u32 = 0;

    const DELETE_REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(280);
    const DELETE_REPEAT_INTERVAL: Duration = Duration::from_millis(120);
    // Hard cap on the continuous word-delete repeat. The Windows impl re-checked
    // GetForegroundWindow every iteration and stopped the moment focus left the
    // Kiro Crew window; the macOS impl has no cheap crate-free foreground check
    // (adding one would mean a new crate / more FFI, which the offline build
    // forbids), so this bound is the backstop that limits how long misdirected
    // deletes could land if focus moves away while joystick-left is still held.
    // Tightened from 10s to 4s to shrink that worst-case window: at the 120ms
    // repeat interval 4s is ~33 word-deletes, far more than any realistic held
    // sentence-clear, while the normal joystick-left release still stops the
    // repeat immediately via the stop flag. See `start_delete_words`.
    const DELETE_REPEAT_MAX_DURATION: Duration = Duration::from_secs(4);

    // Raw FFI into the ApplicationServices umbrella framework (CoreGraphics +
    // CoreFoundation live under it), mirroring the raw FFI pattern in
    // `crate::inject` — no `core-graphics`/`core-foundation` crate is added.
    // The source/event pointers are opaque CF types, modeled as `*const c_void`.
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventCreateKeyboardEvent(
            source: *const c_void,
            keycode: u16,
            keydown: bool,
        ) -> *const c_void;
        fn CGEventKeyboardSetUnicodeString(
            event: *const c_void,
            length: usize,
            unicode_string: *const u16,
        );
        fn CGEventSetFlags(event: *const c_void, flags: u64);
        fn CGEventPost(tap: u32, event: *const c_void);
        fn CFRelease(cf: *const c_void);
    }

    // ── Key injection helpers (CoreGraphics event posting). ──

    /// Create, post, and release one keyboard event (down or up) carrying the
    /// given modifier `flags`. Returns Err on a null event pointer.
    fn post_key(vk: u16, keydown: bool, flags: u64) -> Result<(), String> {
        unsafe {
            let event = CGEventCreateKeyboardEvent(std::ptr::null(), vk, keydown);
            if event.is_null() {
                return Err("CGEventCreateKeyboardEvent returned null".to_string());
            }
            if flags != 0 {
                CGEventSetFlags(event, flags);
            }
            CGEventPost(KCG_HID_EVENT_TAP, event);
            CFRelease(event);
        }
        Ok(())
    }

    /// Tap `vk` with the given modifier flags: keydown then keyup, both carrying
    /// the modifier mask (CoreGraphics attaches modifiers via the event flags,
    /// so there is no separate modifier down/up event to synthesize).
    fn chord_tap(vk: u16, flags: u64) -> Result<(), String> {
        post_key(vk, true, flags)?;
        post_key(vk, false, flags)
    }

    /// Plain tap (no modifiers).
    fn tap(vk: u16) -> Result<(), String> {
        chord_tap(vk, 0)
    }

    /// Type a UTF-8 string as Unicode via a keycode-0 event, mirroring
    /// `inject.rs`'s `type_text`.
    fn type_text_raw(text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            for &keydown in &[true, false] {
                let event = CGEventCreateKeyboardEvent(std::ptr::null(), 0, keydown);
                if event.is_null() {
                    return Err("CGEventCreateKeyboardEvent returned null".to_string());
                }
                CGEventKeyboardSetUnicodeString(event, units.len(), units.as_ptr());
                CGEventPost(KCG_HID_EVENT_TAP, event);
                CFRelease(event);
            }
        }
        Ok(())
    }

    /// Cmd+A then Backspace clears the focused composer.
    fn clear_input() -> Result<(), String> {
        chord_tap(VK_A, FLAG_COMMAND)?;
        tap(VK_DELETE)
    }

    /// Cmd+K — open the command bar.
    fn command_bar_hotkey() -> Result<(), String> {
        chord_tap(VK_K, FLAG_COMMAND)
    }

    // ── macOS chord → (keycode, modifier flags) mapping table ──
    //
    // IMPORTANT: these macOS modifiers were chosen to follow the idiomatic
    // Cmd/Option conventions of a macOS/Electron app, but research of Kiro
    // Crew's ACTUAL macOS keybindings was NOT possible offline. If the real
    // app binds different shortcuts on macOS, adjust the right-hand column of
    // this table — it is the single place the backend decides modifiers.
    //
    //   Chord           macOS shortcut             (was on Windows)
    //   ─────────────   ────────────────────────   ────────────────────
    //   Enter           Return                     Return
    //   Esc             Escape                     Escape
    //   Up/Down/        arrow keys (no modifier)   arrow keys
    //   Left/Right
    //   Backspace       Delete (no modifier)       Backspace
    //   DeleteWord      Option+Backspace           Ctrl+Backspace
    //   ClearInput      Cmd+A then Backspace       Ctrl+A then Backspace
    //   CtrlK           Cmd+K  (command bar)       Ctrl+K
    //   JumpChat(n)     Cmd+<digit n>              Alt+<digit n>
    //   PrevChat        Cmd+Shift+[  ("prev tab")  Alt+Left
    //   NextChat        Cmd+Shift+]  ("next tab")  Alt+Right
    //   FocusInput      Cmd+Return                 Alt+Enter
    //   NewChat         Cmd+N                      Alt+Shift+N
    //
    // (PrevChat/NextChat use the macOS "previous/next tab" convention
    // Cmd+Shift+[ / Cmd+Shift+]; change these if Kiro Crew uses arrows.)
    fn inject_chord(c: Chord) -> Result<(), String> {
        match c {
            Chord::Enter => tap(VK_RETURN),
            Chord::Esc => tap(VK_ESCAPE),
            Chord::Up => tap(VK_UP),
            Chord::Down => tap(VK_DOWN),
            Chord::Left => tap(VK_LEFT),
            Chord::Right => tap(VK_RIGHT),
            Chord::Backspace => tap(VK_DELETE),
            Chord::DeleteWord => chord_tap(VK_DELETE, FLAG_OPTION), // Option+Backspace
            Chord::ClearInput => clear_input(),                     // Cmd+A, Backspace
            Chord::CtrlK => command_bar_hotkey(),                   // Cmd+K
            Chord::JumpChat(n) if (1..=9).contains(&n) => {
                chord_tap(VK_DIGIT[(n - 1) as usize], FLAG_COMMAND) // Cmd+<digit>
            }
            Chord::JumpChat(n) => Err(format!("JumpChat({n}) out of range 1..=9")),
            Chord::PrevChat => chord_tap(VK_LEFT_BRACKET, FLAG_COMMAND | FLAG_SHIFT), // Cmd+Shift+[
            Chord::NextChat => chord_tap(VK_RIGHT_BRACKET, FLAG_COMMAND | FLAG_SHIFT), // Cmd+Shift+]
            Chord::FocusInput => chord_tap(VK_RETURN, FLAG_COMMAND),                   // Cmd+Return
            Chord::NewChat => chord_tap(VK_N, FLAG_COMMAND),                           // Cmd+N
        }
    }

    // ── Window focus by title (AppleScript via /usr/bin/osascript). ──
    //
    // There is no CoreGraphics API to raise a window by title without a crate,
    // so we shell out to `osascript` running a System Events script. The script
    // finds the first application process that owns a window whose title (or the
    // process name) contains the configured substring and `activate`s it,
    // printing "AK_FOCUS_OK" on success. This requires macOS **Automation**
    // permission (to drive System Events) and, together with key injection,
    // **Accessibility** permission.

    /// True if `needle` contains any ASCII control character (newline, carriage
    /// return, tab, NUL, etc.). An AppleScript double-quoted literal cannot span
    /// a raw newline, so such a value would either break the script or let an
    /// attacker-ish `title_match` append AppleScript lines. We reject these
    /// up-front in [`try_focus`] (fail closed → treated as "no match") rather
    /// than try to escape every control char; normal window titles never
    /// contain control characters, so this does not change behavior for them.
    fn has_control_chars(needle: &str) -> bool {
        needle.chars().any(|c| c.is_control())
    }

    /// Build the AppleScript that activates the first app whose window title
    /// (or process name) contains `needle` (case-insensitive — AppleScript's
    /// `contains` is case-insensitive by default). `needle` is embedded as an
    /// AppleScript string literal with quotes/backslashes escaped so an odd
    /// title substring cannot break out of the script. Callers MUST reject
    /// needles containing control chars first (see [`has_control_chars`]); a raw
    /// newline cannot be represented in a double-quoted AppleScript literal.
    fn focus_script(needle: &str) -> String {
        let escaped = needle.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            "set needle to \"{escaped}\"\n\
             tell application \"System Events\"\n\
             \tset procs to (every application process whose background only is false)\n\
             \trepeat with p in procs\n\
             \t\ttry\n\
             \t\t\tif (name of p) contains needle then\n\
             \t\t\t\tset frontmost of p to true\n\
             \t\t\t\treturn \"AK_FOCUS_OK\"\n\
             \t\t\tend if\n\
             \t\t\trepeat with w in (windows of p)\n\
             \t\t\t\tif (name of w) contains needle then\n\
             \t\t\t\t\tset frontmost of p to true\n\
             \t\t\t\t\treturn \"AK_FOCUS_OK\"\n\
             \t\t\t\tend if\n\
             \t\t\tend repeat\n\
             \t\tend try\n\
             \tend repeat\n\
             end tell\n\
             return \"AK_FOCUS_NONE\""
        )
    }

    /// Run the focus AppleScript for one `needle`; `Ok(true)` when a matching
    /// app was activated, `Ok(false)` when none matched, `Err` on osascript
    /// failure (e.g. missing Automation permission).
    fn try_focus(needle: &str) -> Result<bool, String> {
        // Fail closed on control characters (newlines/CR/tab/NUL/…): they
        // cannot be embedded in a double-quoted AppleScript literal and could
        // otherwise inject script lines. Treat such a needle as "no match" so
        // focus() moves on to the next candidate / returns the normal "no
        // window matches" Err — injecting nothing.
        if has_control_chars(needle) {
            return Ok(false);
        }
        let out = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(focus_script(needle))
            .output()
            .map_err(|e| format!("control: osascript spawn failed: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "control: osascript failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).contains("AK_FOCUS_OK"))
    }

    struct DeleteRepeat {
        stop: Arc<AtomicBool>,
        worker: JoinHandle<()>,
    }

    /// macOS controller: focuses a Kiro Crew window (via AppleScript) then
    /// injects keystrokes (via CoreGraphics).
    pub struct MacController {
        title_match: String,
        delete_repeat: Option<DeleteRepeat>,
    }

    impl MacController {
        pub fn new(title_match: Option<String>) -> Self {
            Self {
                title_match: title_match.unwrap_or_default(),
                delete_repeat: None,
            }
        }

        pub fn set_title_match(&mut self, title: String) {
            self.title_match = title;
        }

        /// Resolve + foreground the Kiro Crew window. Verifies a match BEFORE
        /// any injection (AC1.5, mirroring the Windows contract); returns `Err`
        /// and injects nothing if no matching window/app is found.
        pub fn focus(&mut self) -> Result<(), String> {
            // Candidate list: the configured substring first, else the defaults.
            let candidates: Vec<String> = if self.title_match.trim().is_empty() {
                DEFAULT_TITLE_CANDIDATES.iter().map(|s| s.to_string()).collect()
            } else {
                vec![self.title_match.clone()]
            };

            let mut matched = false;
            for needle in &candidates {
                if try_focus(needle)? {
                    matched = true;
                    break;
                }
            }
            if !matched {
                return Err(format!(
                    "control: no window matches title {:?} (nothing injected)",
                    if self.title_match.trim().is_empty() {
                        DEFAULT_TITLE_CANDIDATES.join(" / ")
                    } else {
                        self.title_match.clone()
                    }
                ));
            }

            // Let the target actually receive focus before keys are injected —
            // injection otherwise races the foreground switch and keys are
            // dropped or land in the previous window (same 40ms settle as the
            // former Windows impl).
            std::thread::sleep(std::time::Duration::from_millis(40));
            Ok(())
        }

        pub fn send_text(&mut self, text: &str) -> Result<(), String> {
            self.focus()?;
            type_text_raw(text)
        }

        pub fn command_bar(&mut self, cmd: &str) -> Result<(), String> {
            self.focus()?;
            command_bar_hotkey()?;
            type_text_raw(cmd)?;
            tap(VK_RETURN)
        }

        /// Focus the window, then type — no Enter (K5 submits).
        pub fn compose(&mut self, text: &str) -> Result<(), String> {
            // Focus the window only, then type. We deliberately do NOT inject a
            // "focus the composer" chord: on Kiro Crew that chord SUBMITS, which
            // would auto-send whatever was already in the composer. Rely on the
            // composer being the default focused control; typing never submits,
            // and the user still submits explicitly with K5 (Enter).
            self.focus()?;
            type_text_raw(text)
        }

        pub fn chord(&mut self, keys: Chord) -> Result<(), String> {
            self.focus()?;
            inject_chord(keys)
        }

        pub fn start_delete_words(&mut self) -> Result<(), String> {
            self.stop_delete_words()?;
            self.focus()?;

            // Delete one word immediately when LONG arrives, then repeat after
            // a short typematic delay. BEHAVIOR DELTA vs Windows: the Windows
            // impl re-checked GetForegroundWindow each iteration and stopped if
            // focus moved away. There is no cheap crate-free macOS equivalent
            // (a per-iteration osascript/CGWindow foreground check would add a
            // crate or heavy FFI and shell out ~every 120ms), so we rely on the
            // stop flag (set the instant joystick-left is released — the normal
            // case) plus the tightened DELETE_REPEAT_MAX_DURATION backstop. The
            // only residual gap is: focus moves elsewhere WHILE the key stays
            // held, where deletes keep landing in whatever is focused until the
            // 4s cap. Documented as an accepted delta (see the const above).
            inject_chord(Chord::DeleteWord)?; // Option+Backspace
            let stop = Arc::new(AtomicBool::new(false));
            let worker_stop = Arc::clone(&stop);
            let worker = thread::Builder::new()
                .name("ak-delete-words".to_string())
                .spawn(move || {
                    let started = Instant::now();
                    thread::sleep(DELETE_REPEAT_INITIAL_DELAY);
                    while !worker_stop.load(Ordering::Acquire)
                        && started.elapsed() < DELETE_REPEAT_MAX_DURATION
                    {
                        if inject_chord(Chord::DeleteWord).is_err() {
                            break;
                        }
                        thread::sleep(DELETE_REPEAT_INTERVAL);
                    }
                })
                .map_err(|error| format!("control: start delete repeat: {error}"))?;
            self.delete_repeat = Some(DeleteRepeat { stop, worker });
            Ok(())
        }

        pub fn stop_delete_words(&mut self) -> Result<(), String> {
            if let Some(repeat) = self.delete_repeat.take() {
                repeat.stop.store(true, Ordering::Release);
                repeat
                    .worker
                    .join()
                    .map_err(|_| "control: delete repeat thread panicked".to_string())?;
            }
            Ok(())
        }
    }

    impl Drop for MacController {
        fn drop(&mut self) {
            let _ = self.stop_delete_words();
        }
    }
}

// ============================ Non-macOS ============================

#[cfg(not(target_os = "macos"))]
mod stub_impl {
    use super::Chord;

    /// Non-macOS stub: compiles and unit-tests on Linux. `focus` is a no-op
    /// Ok (so router logic can be exercised end to end without a real window);
    /// the injecting methods return `Err` — there is no window system to drive.
    pub struct StubController {
        #[allow(dead_code)]
        title_match: String,
    }

    impl StubController {
        pub fn new(title_match: Option<String>) -> Self {
            Self { title_match: title_match.unwrap_or_default() }
        }

        pub fn set_title_match(&mut self, title: String) {
            self.title_match = title;
        }

        pub fn focus(&mut self) -> Result<(), String> {
            Ok(())
        }

        pub fn send_text(&mut self, _text: &str) -> Result<(), String> {
            Err("control: macOS only".to_string())
        }

        pub fn command_bar(&mut self, _cmd: &str) -> Result<(), String> {
            Err("control: macOS only".to_string())
        }

        pub fn compose(&mut self, _text: &str) -> Result<(), String> {
            Err("control: macOS only".to_string())
        }

        pub fn chord(&mut self, _keys: Chord) -> Result<(), String> {
            Err("control: macOS only".to_string())
        }

        pub fn start_delete_words(&mut self) -> Result<(), String> {
            Err("control: macOS only".to_string())
        }

        pub fn stop_delete_words(&mut self) -> Result<(), String> {
            Ok(())
        }
    }
}

// ─────────────── Public controller facade (platform-neutral) ───────────────

#[cfg(target_os = "macos")]
type Backend = macos_impl::MacController;
#[cfg(not(target_os = "macos"))]
type Backend = stub_impl::StubController;

/// The FP-4b controller: focus a local Kiro Crew window and inject text / the
/// command bar / chords. macOS-backed for real (CoreGraphics key injection +
/// AppleScript window focus); a compiling no-op/Err stub on every other host.
/// Construct with [`KiroCrewController::new`].
pub struct KiroCrewController {
    backend: Backend,
}

impl KiroCrewController {
    /// `title_match` — a window-title substring; `None`/empty uses
    /// [`DEFAULT_TITLE_CANDIDATES`] ("Kiro Crew" / "Agentic").
    pub fn new(title_match: Option<String>) -> Self {
        Self { backend: Backend::new(title_match) }
    }

    /// Re-configure the title substring to match (clears any cached handle).
    pub fn set_title_match(&mut self, title: String) {
        self.backend.set_title_match(title);
    }

    /// Resolve + foreground the window. Verifies a match before injecting
    /// (AC1.5); on non-macOS this is a no-op `Ok`.
    pub fn focus(&mut self) -> Result<(), String> {
        self.backend.focus()
    }

    /// Focus, then type `text` (no command bar, no Enter).
    pub fn send_text(&mut self, text: &str) -> Result<(), String> {
        self.backend.send_text(text)
    }

    /// Focus, open the command bar (Cmd+K on macOS), type `cmd`, press Enter.
    pub fn command_bar(&mut self, cmd: &str) -> Result<(), String> {
        self.backend.command_bar(cmd)
    }

    /// Focus the window, then type `text` (no Enter — K5 submits).
    pub fn compose(&mut self, text: &str) -> Result<(), String> {
        self.backend.compose(text)
    }

    /// Focus, then inject a single named [`Chord`].
    pub fn chord(&mut self, keys: Chord) -> Result<(), String> {
        self.backend.chord(keys)
    }

    /// Delete one word immediately, then keep deleting while Left remains held.
    pub fn start_delete_words(&mut self) -> Result<(), String> {
        self.backend.start_delete_words()
    }

    /// Stop continuous word deletion. Safe when no repeat is active.
    pub fn stop_delete_words(&mut self) -> Result<(), String> {
        self.backend.stop_delete_words()
    }

    /// Execute a routed [`ControlAction`] against the window. `None` is a no-op.
    pub fn apply(&mut self, action: &ControlAction) -> Result<(), String> {
        match action {
            ControlAction::CommandBar(cmd) => self.command_bar(cmd),
            ControlAction::Chord(c) => self.chord(*c),
            ControlAction::SendText(t) => self.send_text(t),
            ControlAction::Compose(t) => self.compose(t),
            ControlAction::StartDeleteWords => self.start_delete_words(),
            ControlAction::StopDeleteWords => self.stop_delete_words(),
            ControlAction::None => Ok(()),
        }
    }
}

// ──────────────────────────── Tests (Linux) ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        HelloAckPayload, KeyAction, KeyPayload, MacroPayload, NavPayload, VoicePayload, VoiceState,
    };

    fn key(id: &str, action: KeyAction) -> Inbound {
        Inbound::Key {
            seq: Some(1),
            payload: KeyPayload { id: id.into(), action, held_ms: 0 },
        }
    }

    fn key_held(id: &str, action: KeyAction, held_ms: u32) -> Inbound {
        Inbound::Key {
            seq: Some(1),
            payload: KeyPayload { id: id.into(), action, held_ms },
        }
    }

    fn nav(dir: NavDir, action: KeyAction) -> Inbound {
        nav_held(dir, action, 0)
    }

    fn nav_held(dir: NavDir, action: KeyAction, held_ms: u32) -> Inbound {
        Inbound::Nav {
            seq: Some(1),
            payload: NavPayload {
                dir,
                action,
                held_ms,
            },
        }
    }

    fn mac(name: &str) -> Inbound {
        Inbound::Macro {
            seq: None,
            payload: MacroPayload { name: name.into() },
        }
    }

    // ── Chord parsing ──────────────────────────────────────────

    #[test]
    fn chord_parse_is_case_insensitive_and_rejects_unknown() {
        assert_eq!(Chord::parse("Enter"), Some(Chord::Enter));
        assert_eq!(Chord::parse("RETURN"), Some(Chord::Enter));
        assert_eq!(Chord::parse("esc"), Some(Chord::Esc));
        assert_eq!(Chord::parse("escape"), Some(Chord::Esc));
        assert_eq!(Chord::parse(" up "), Some(Chord::Up));
        assert_eq!(Chord::parse("backspace"), Some(Chord::Backspace));
        assert_eq!(Chord::parse("delete-word"), Some(Chord::DeleteWord));
        assert_eq!(Chord::parse("clear_input"), Some(Chord::ClearInput));
        assert_eq!(Chord::parse("ctrl+k"), Some(Chord::CtrlK));
        assert_eq!(Chord::parse("command_bar"), Some(Chord::CtrlK));
        assert_eq!(Chord::parse("prevchat"), Some(Chord::PrevChat));
        assert_eq!(Chord::parse("next-chat"), Some(Chord::NextChat));
        assert_eq!(Chord::parse("focus_input"), Some(Chord::FocusInput));
        assert_eq!(Chord::parse("newchat"), Some(Chord::NewChat));
        assert_eq!(Chord::parse("chat3"), Some(Chord::JumpChat(3)));
        assert_eq!(Chord::parse("chat9"), Some(Chord::JumpChat(9)));
        assert_eq!(Chord::parse("chat0"), None);
        assert_eq!(Chord::parse("nope"), None);
        assert_eq!(Chord::parse(""), None);
    }

    // ── Router: K4 recognized text → compose into the chat input ───

    #[test]
    fn macro_text_routes_to_compose() {
        let ctx = ControlCtx::with_default_jumps();
        assert_eq!(
            route_input(&mac("summarize the file"), &ctx),
            ControlAction::Compose("summarize the file".into())
        );
    }

    #[test]
    fn asr_text_routes_to_compose() {
        // Cloud ASR text is unified with the macro path: type into the composer
        // (no send), so both voice routes behave the same and K5 submits.
        let ctx = ControlCtx::with_default_jumps();
        let asr = Inbound::Asr {
            seq: Some(9),
            payload: crate::protocol::AsrPayload { text: "hello there".into() },
        };
        assert_eq!(route_input(&asr, &ctx), ControlAction::Compose("hello there".into()));
    }

    #[test]
    fn empty_macro_is_none() {
        let ctx = ControlCtx::default();
        assert_eq!(route_input(&mac("   "), &ctx), ControlAction::None);
    }

    // ── Router: K5 → Enter ─────────────────────────────────────

    #[test]
    fn k5_down_routes_to_enter_chord() {
        let ctx = ControlCtx::default();
        assert_eq!(
            route_input(&key("k5", KeyAction::Down), &ctx),
            ControlAction::Chord(Chord::Enter)
        );
        // Long press also triggers.
        assert_eq!(
            route_input(&key("k5", KeyAction::Long), &ctx),
            ControlAction::Chord(Chord::Enter)
        );
    }

    // ── Router: K1–K3 → Alt+<configured number> jump on Down only ──

    #[test]
    fn k1_k2_k3_click_jumps_to_configured_chat_number() {
        // Non-default numbers to prove the router reads ctx.jump_chats.
        let ctx = ControlCtx { jump_chats: vec![5, 2, 9] };
        assert_eq!(
            route_input(&key("k1", KeyAction::Click), &ctx),
            ControlAction::Chord(Chord::JumpChat(5))
        );
        assert_eq!(
            route_input(&key("k2", KeyAction::Click), &ctx),
            ControlAction::Chord(Chord::JumpChat(2))
        );
        assert_eq!(
            route_input(&key("k3", KeyAction::Click), &ctx),
            ControlAction::Chord(Chord::JumpChat(9))
        );
    }

    #[test]
    fn k1_k2_k3_click_uses_defaults() {
        let ctx = ControlCtx::with_default_jumps();
        assert_eq!(
            route_input(&key("k1", KeyAction::Click), &ctx),
            ControlAction::Chord(Chord::JumpChat(1))
        );
        assert_eq!(
            route_input(&key("k2", KeyAction::Click), &ctx),
            ControlAction::Chord(Chord::JumpChat(2))
        );
        assert_eq!(
            route_input(&key("k3", KeyAction::Click), &ctx),
            ControlAction::Chord(Chord::JumpChat(3))
        );
    }

    #[test]
    fn k1_k3_down_long_up_dclick_are_none() {
        let ctx = ControlCtx::with_default_jumps();
        // The jump fires ONLY on Click. Down (no longer a trigger), Long, Up,
        // and Dclick all do nothing — long-press is reserved for auto-bind.
        for id in ["k1", "k2", "k3"] {
            assert_eq!(
                route_input(&key(id, KeyAction::Down), &ctx),
                ControlAction::None,
                "{id} Down should be None (jump moved to Click)"
            );
            assert_eq!(
                route_input(&key(id, KeyAction::Long), &ctx),
                ControlAction::None,
                "{id} Long should be None"
            );
            assert_eq!(
                route_input(&key(id, KeyAction::Dclick), &ctx),
                ControlAction::None,
                "{id} Dclick should be None (no action bound yet)"
            );
            assert_eq!(
                route_input(&key_held(id, KeyAction::Up, 120), &ctx),
                ControlAction::None,
                "{id} short Up should be None"
            );
            assert_eq!(
                route_input(&key_held(id, KeyAction::Up, 1500), &ctx),
                ControlAction::None,
                "{id} long-tail Up should be None"
            );
        }
    }

    #[test]
    fn k1_k3_click_none_when_number_unset_or_out_of_range() {
        // Missing index (short jump_chats vector) -> None.
        let short = ControlCtx { jump_chats: vec![1] };
        assert_eq!(route_input(&key("k2", KeyAction::Click), &short), ControlAction::None);
        assert_eq!(route_input(&key("k3", KeyAction::Click), &short), ControlAction::None);
        // Out-of-range number (0 or >9) -> None.
        let bad = ControlCtx { jump_chats: vec![0, 10, 3] };
        assert_eq!(route_input(&key("k1", KeyAction::Click), &bad), ControlAction::None);
        assert_eq!(route_input(&key("k2", KeyAction::Click), &bad), ControlAction::None);
        assert_eq!(
            route_input(&key("k3", KeyAction::Click), &bad),
            ControlAction::Chord(Chord::JumpChat(3))
        );
        // Empty vector (default ControlCtx) -> None.
        let empty = ControlCtx::default();
        assert_eq!(route_input(&key("k1", KeyAction::Click), &empty), ControlAction::None);
    }

    #[test]
    fn k4_press_is_none_text_comes_via_macro() {
        let ctx = ControlCtx::with_default_jumps();
        assert_eq!(route_input(&key("k4", KeyAction::Down), &ctx), ControlAction::None);
    }

    // ── Router: nav → composer editing / navigation / submit ───

    #[test]
    fn nav_left_short_release_deletes_one_character() {
        let ctx = ControlCtx::default();
        assert_eq!(
            route_input(&nav(NavDir::Left, KeyAction::Down), &ctx),
            ControlAction::None
        );
        assert_eq!(
            route_input(&nav_held(NavDir::Left, KeyAction::Up, 120), &ctx),
            ControlAction::Chord(Chord::Backspace)
        );
    }

    #[test]
    fn nav_left_long_starts_repeat_and_release_stops_it() {
        let ctx = ControlCtx::default();
        assert_eq!(
            route_input(
                &nav_held(NavDir::Left, KeyAction::Long, BOARD_LONG_PRESS_MS),
                &ctx
            ),
            ControlAction::StartDeleteWords
        );
        assert_eq!(
            route_input(&nav_held(NavDir::Left, KeyAction::Up, 1200), &ctx),
            ControlAction::StopDeleteWords
        );
    }

    #[test]
    fn nav_up_short_moves_to_previous_line_and_long_clears_input() {
        let ctx = ControlCtx::default();
        assert_eq!(route_input(&nav(NavDir::Up, KeyAction::Down), &ctx), ControlAction::None);
        assert_eq!(
            route_input(&nav_held(NavDir::Up, KeyAction::Up, 120), &ctx),
            ControlAction::Chord(Chord::Up)
        );
        assert_eq!(
            route_input(&nav_held(NavDir::Up, KeyAction::Long, BOARD_LONG_PRESS_MS), &ctx),
            ControlAction::Chord(Chord::ClearInput)
        );
        assert_eq!(
            route_input(&nav_held(NavDir::Up, KeyAction::Up, 1200), &ctx),
            ControlAction::None
        );
    }

    #[test]
    fn nav_right_down_moves_to_next_chat_and_down_moves_caret() {
        let ctx = ControlCtx::default();
        assert_eq!(
            route_input(&nav(NavDir::Right, KeyAction::Down), &ctx),
            ControlAction::Chord(Chord::NextChat)
        );
        assert_eq!(
            route_input(&nav(NavDir::Down, KeyAction::Down), &ctx),
            ControlAction::Chord(Chord::Down)
        );
    }

    #[test]
    fn nav_center_routes_to_enter_on_down_only() {
        let ctx = ControlCtx::default();
        assert_eq!(
            route_input(&nav(NavDir::Center, KeyAction::Down), &ctx),
            ControlAction::Chord(Chord::Enter)
        );
        assert_eq!(
            route_input(&nav(NavDir::Center, KeyAction::Long), &ctx),
            ControlAction::None
        );
    }

    // ── Router: non-triggering / ignored events → None ─────────

    #[test]
    fn key_up_edges_do_not_trigger() {
        let ctx = ControlCtx::with_default_jumps();
        // K5 is not a jump key; its Up edge is a no-op.
        assert_eq!(route_input(&key("k5", KeyAction::Up), &ctx), ControlAction::None);
        // (K1-K3 Up edges are covered above — all None now.)
    }

    #[test]
    fn nav_right_up_edge_does_not_trigger() {
        let ctx = ControlCtx::default();
        assert_eq!(
            route_input(&nav(NavDir::Right, KeyAction::Up), &ctx),
            ControlAction::None
        );
    }

    #[test]
    fn unknown_key_and_housekeeping_frames_are_none() {
        let ctx = ControlCtx::with_default_jumps();
        assert_eq!(route_input(&key("k9", KeyAction::Down), &ctx), ControlAction::None);
        assert_eq!(route_input(&Inbound::Ping, &ctx), ControlAction::None);
        assert_eq!(route_input(&Inbound::Pong, &ctx), ControlAction::None);
        assert_eq!(
            route_input(
                &Inbound::Voice { seq: None, payload: VoicePayload { state: VoiceState::Start } },
                &ctx
            ),
            ControlAction::None
        );
        assert_eq!(
            route_input(&Inbound::HelloAck(HelloAckPayload::default()), &ctx),
            ControlAction::None
        );
    }

    // ── ControlRouter wrapper ──────────────────────────────────

    #[test]
    fn control_router_wraps_route_input() {
        let router = ControlRouter::new(ControlCtx::with_default_jumps());
        assert_eq!(
            router.route(&key("k5", KeyAction::Down)),
            ControlAction::Chord(Chord::Enter)
        );
        // And a K1 Click jumps to the configured number (Down no longer does).
        assert_eq!(
            router.route(&key("k1", KeyAction::Click)),
            ControlAction::Chord(Chord::JumpChat(1))
        );
        assert_eq!(router.route(&key("k1", KeyAction::Down)), ControlAction::None);
    }

    // ── Stub controller behaviour on non-macOS ─────────────────

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stub_controller_focus_ok_inject_err() {
        let mut c = KiroCrewController::new(None);
        assert!(c.focus().is_ok());
        assert!(c.send_text("x").is_err());
        assert!(c.command_bar("cmd").is_err());
        assert!(c.chord(Chord::Enter).is_err());
        // apply() delegates: no-op/stop are safe; injection is macOS-only.
        assert!(c.apply(&ControlAction::None).is_ok());
        assert!(c.apply(&ControlAction::StopDeleteWords).is_ok());
        assert!(c.apply(&ControlAction::StartDeleteWords).is_err());
        assert!(c.apply(&ControlAction::Chord(Chord::Up)).is_err());
    }
}
