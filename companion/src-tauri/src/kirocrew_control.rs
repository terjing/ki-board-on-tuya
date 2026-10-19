//! FP-4b Route B — the *hands*: drive a **local** Kiro Crew window by focusing
//! it and injecting keystrokes (no HTTP, no session-control API). This is the
//! control plane that supersedes route A for the actual control path.
//!
//! Two layers live here:
//!
//! * [`KiroCrewController`] — Windows-gated `user32` FFI backend that finds the
//!   Kiro Crew window by a configurable title substring, brings it to the
//!   foreground, and injects text / the Ctrl+K command bar / named chords. It
//!   reuses the raw `SendInput` pattern from [`crate::inject`] (no new crate).
//!   On macOS the backend focuses the window through the Accessibility API and
//!   injects CGEvents (`mac_sys`); the command bar is ⌘K. On other hosts a
//!   stub compiles and returns `Err` / no-ops so the Linux CI build and the
//!   pure router tests below still work.
//!
//! * [`ControlRouter`] / [`route_input`] — a **pure**, IO-free mapping from a
//!   decoded board [`Inbound`] event to a [`ControlAction`]. Table-driven and
//!   unit-tested on Linux (the tests at the bottom run in `cargo test`).
//!
//! Design ref: `.kiro/specs/feature-probes-kirocrew-local-control/design.md`.

use crate::protocol::{Inbound, KeyAction, NavDir};

// ───────────────────────────── Chord ─────────────────────────────

/// A named key / key-combination the controller can inject. Kept small and
/// `Copy` so the pure router can hand one to the (Windows) backend by value.
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
    /// Ctrl+Backspace — delete the word before the composer caret.
    DeleteWord,
    /// Ctrl+A, Backspace — clear the current composer sentence.
    ClearInput,
    /// Ctrl+K (⌘K on macOS) — open the Kiro Crew command bar.
    CtrlK,
    /// Alt+N — jump to chat N (N = 1..=9). Deterministic, unlike a command-bar
    /// text search. (Kiro Crew default shortcut "Jump to chat N".)
    JumpChat(u8),
    /// Alt+Left — previous chat.
    PrevChat,
    /// Alt+Right — next chat.
    NextChat,
    /// Alt+Enter — focus the chat text input (composer).
    FocusInput,
    /// Alt+Shift+N — new chat.
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
/// (Windows) [`KiroCrewController`]; pure and IO-free so it is fully testable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlAction {
    /// Open the command bar (Ctrl+K / ⌘K), type the string, press Enter.
    CommandBar(String),
    /// Inject a single named chord.
    Chord(Chord),
    /// Type free text into the focused input (no command bar, no Enter).
    SendText(String),
    /// Focus the chat composer (Alt+Enter), then type the text — no Enter, so
    /// the user reviews and submits with K5. This is the K4 voice path.
    Compose(String),
    /// Start repeating Ctrl+Backspace while joystick-left remains held.
    StartDeleteWords,
    /// Stop a previously started continuous word deletion.
    StopDeleteWords,
    /// Nothing to do for this event (e.g. a key *up*, a ping, an ack).
    None,
}

/// Router context — the small amount of state the pure mapping needs but does
/// not own. `jump_chats[i]` is the chat NUMBER (1..=9) that K(i+1) (K1..K3)
/// jumps to via the deterministic `Alt+N` shortcut. Defaults to `[1, 2, 3]`.
/// The number for each key is user-configurable (persisted in the bind store)
/// and kept in sync here by `lib.rs`.
#[derive(Clone, Debug, Default)]
pub struct ControlCtx {
    /// Chat numbers for K1, K2, K3 (index 0..=2). Configurable by the UI.
    pub jump_chats: Vec<u8>,
}

impl ControlCtx {
    /// Sensible defaults: K1->chat 1, K2->chat 2, K3->chat 3 (Alt+1/2/3).
    pub fn with_default_jumps() -> Self {
        Self { jump_chats: vec![1, 2, 3] }
    }

    /// Deterministic `Alt+N` jump for slot `idx` (0-based). Returns
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
/// | `key k1..k3` `Click`           | `Chord(JumpChat(n))` — n = configured `jump_chats[idx]` (Alt+N, client-side jump to chat n); `None` if unset / out of 1..=9 |
/// | `key k1..k3` `Down`/`Long`/`Up`/`Dclick` | `None` — jump fires only on `Click`; `Long` is reserved for long-press auto-bind |
/// | `key k4` (down/long)           | `None` — K4 is push-to-talk; text arrives later as `macro` |
/// | `key k5` (down/long)           | `Chord(Enter)` — submit / continue  |
/// | `nav left` `Up` (< 800 ms)  | `Chord(Backspace)` — delete one character |
/// | `nav left` `Long`           | `StartDeleteWords` — repeat Ctrl+Backspace |
/// | `nav left` `Up` (>= 800 ms) | `StopDeleteWords` — stop repeat            |
/// | `nav up` `Up` (< 800 ms)    | `Chord(Up)` — move to previous line      |
/// | `nav up` `Long`             | `Chord(ClearInput)` — Ctrl+A, Backspace  |
/// | `nav right` `Down`          | `Chord(NextChat)` — Alt+Right            |
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

// ============================== Windows ==============================

#[cfg(windows)]
mod windows_impl {
    use super::{Chord, DEFAULT_TITLE_CANDIDATES};
    use std::mem::size_of;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    // --- Minimal Win32 bindings (raw FFI, mirrors inject.rs — no `windows` crate). ---
    type HWND = isize;
    type LPARAM = isize;
    type BOOL = i32;
    type WPARAM = usize;

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const KEYEVENTF_UNICODE: u32 = 0x0004;
    const KEYEVENTF_SCANCODE: u32 = 0x0008;
    const MAPVK_VK_TO_VSC: u32 = 0;

    // Virtual-key codes we use for chords / the command bar.
    const VK_RETURN: u16 = 0x0D;
    const VK_ESCAPE: u16 = 0x1B;
    const VK_BACK: u16 = 0x08;
    const VK_LEFT: u16 = 0x25;
    const VK_UP: u16 = 0x26;
    const VK_RIGHT: u16 = 0x27;
    const VK_DOWN: u16 = 0x28;
    const VK_CONTROL: u16 = 0x11;
    const VK_MENU: u16 = 0x12; // Alt
    const VK_SHIFT: u16 = 0x10;
    const VK_A: u16 = 0x41; // 'A'
    const VK_K: u16 = 0x4B; // 'K'
    const VK_N: u16 = 0x4E; // 'N'

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KEYBDINPUT {
        w_vk: u16,
        w_scan: u16,
        dw_flags: u32,
        time: u32,
        dw_extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct INPUT {
        r#type: u32,
        ki: KEYBDINPUT,
        _pad: [u8; 8],
    }

    // EnumWindows callback signature.
    type WNDENUMPROC = unsafe extern "system" fn(HWND, LPARAM) -> BOOL;

    #[link(name = "user32")]
    extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const INPUT, cb_size: i32) -> u32;
        fn MapVirtualKeyW(u_code: u32, u_map_type: u32) -> u32;
        fn EnumWindows(lp_enum_func: WNDENUMPROC, l_param: LPARAM) -> BOOL;
        fn GetWindowTextW(hwnd: HWND, lp_string: *mut u16, n_max_count: i32) -> i32;
        fn GetWindowTextLengthW(hwnd: HWND) -> i32;
        fn IsWindowVisible(hwnd: HWND) -> BOOL;
        fn SetForegroundWindow(hwnd: HWND) -> BOOL;
        fn ShowWindow(hwnd: HWND, n_cmd_show: i32) -> BOOL;
        fn IsIconic(hwnd: HWND) -> BOOL;
        fn SendMessageW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize;
        fn GetForegroundWindow() -> HWND;
        fn GetWindowThreadProcessId(hwnd: HWND, lp_process_id: *mut u32) -> u32;
        fn AttachThreadInput(id_attach: u32, id_attach_to: u32, f_attach: BOOL) -> BOOL;
        fn AllowSetForegroundWindow(dw_process_id: u32) -> BOOL;
        fn BringWindowToTop(hwnd: HWND) -> BOOL;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }

    const SW_RESTORE: i32 = 9;
    const SW_SHOW: i32 = 5;
    // ASFW_ANY — allow any process to set the foreground window.
    const ASFW_ANY: u32 = 0xFFFF_FFFF;
    const DELETE_REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(280);
    const DELETE_REPEAT_INTERVAL: Duration = Duration::from_millis(120);
    const DELETE_REPEAT_MAX_DURATION: Duration = Duration::from_secs(10);

    // --- SendInput helpers (same shape as inject.rs). ---

    fn send(inputs: &[INPUT]) -> Result<(), String> {
        let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            Err(format!("SendInput injected {sent}/{} events", inputs.len()))
        }
    }

    fn unicode_event(unit: u16, key_up: bool) -> INPUT {
        let mut flags = KEYEVENTF_UNICODE;
        if key_up {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            ki: KEYBDINPUT { w_vk: 0, w_scan: unit, dw_flags: flags, time: 0, dw_extra_info: 0 },
            _pad: [0; 8],
        }
    }

    /// Build a physical key event with a scan code whenever Windows can map the
    /// virtual key. Kiro Crew routes chat shortcuts by `KeyboardEvent.code`
    /// (`Digit1` … `Digit9`); Chromium/WebView cannot reliably derive that code
    /// from a SendInput packet whose scan code is zero. Fall back to the virtual
    /// key for the rare values MapVirtualKeyW cannot map.
    fn vkey_event(vk: u16, key_up: bool) -> INPUT {
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
        let mut flags = if key_up { KEYEVENTF_KEYUP } else { 0 };
        let extended = matches!(vk, VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN);
        let (w_vk, w_scan) = if scan != 0 {
            flags |= KEYEVENTF_SCANCODE;
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            (0, scan)
        } else {
            (vk, 0)
        };
        INPUT {
            r#type: INPUT_KEYBOARD,
            ki: KEYBDINPUT {
                w_vk,
                w_scan,
                dw_flags: flags,
                time: 0,
                dw_extra_info: 0,
            },
            _pad: [0; 8],
        }
    }

    fn type_text_raw(text: &str) -> Result<(), String> {
        let mut events = Vec::with_capacity(text.len() * 2);
        for unit in text.encode_utf16() {
            events.push(unicode_event(unit, false));
            events.push(unicode_event(unit, true));
        }
        if events.is_empty() {
            return Ok(());
        }
        send(&events)
    }

    fn tap(vk: u16) -> Result<(), String> {
        send(&[vkey_event(vk, false), vkey_event(vk, true)])
    }

    /// LCTRL + <vk> as one SendInput batch.
    fn ctrl_tap(vk: u16) -> Result<(), String> {
        send(&[
            vkey_event(VK_CONTROL, false),
            vkey_event(vk, false),
            vkey_event(vk, true),
            vkey_event(VK_CONTROL, true),
        ])
    }

    /// Ctrl+A followed by Backspace clears the focused composer.
    fn clear_input() -> Result<(), String> {
        send(&[
            vkey_event(VK_CONTROL, false),
            vkey_event(VK_A, false),
            vkey_event(VK_A, true),
            vkey_event(VK_CONTROL, true),
            vkey_event(VK_BACK, false),
            vkey_event(VK_BACK, true),
        ])
    }

    /// LCTRL↓ K↓ K↑ LCTRL↑ — the command-bar hotkey.
    fn ctrl_k() -> Result<(), String> {
        send(&[
            vkey_event(VK_CONTROL, false),
            vkey_event(VK_K, false),
            vkey_event(VK_K, true),
            vkey_event(VK_CONTROL, true),
        ])
    }

    /// Alt + <vk> (Alt↓ vk↓ vk↑ Alt↑).
    fn alt_tap(vk: u16) -> Result<(), String> {
        send(&[
            vkey_event(VK_MENU, false),
            vkey_event(vk, false),
            vkey_event(vk, true),
            vkey_event(VK_MENU, true),
        ])
    }

    /// Alt + Shift + <vk>.
    fn alt_shift_tap(vk: u16) -> Result<(), String> {
        send(&[
            vkey_event(VK_MENU, false),
            vkey_event(VK_SHIFT, false),
            vkey_event(vk, false),
            vkey_event(vk, true),
            vkey_event(VK_SHIFT, true),
            vkey_event(VK_MENU, true),
        ])
    }

    /// Inject one chord. Simple chords tap a single VK; the Kiro Crew shortcuts
    /// use Alt / Alt+Shift combos.
    fn inject_chord(c: Chord) -> Result<(), String> {
        match c {
            Chord::Enter => tap(VK_RETURN),
            Chord::Esc => tap(VK_ESCAPE),
            Chord::Up => tap(VK_UP),
            Chord::Down => tap(VK_DOWN),
            Chord::Left => tap(VK_LEFT),
            Chord::Right => tap(VK_RIGHT),
            Chord::Backspace => tap(VK_BACK),
            Chord::DeleteWord => ctrl_tap(VK_BACK),
            Chord::ClearInput => clear_input(),
            Chord::CtrlK => ctrl_k(),
            Chord::JumpChat(n) if (1..=9).contains(&n) => alt_tap(0x30 + n as u16), // Alt+'1'..'9'
            Chord::JumpChat(n) => Err(format!("JumpChat({n}) out of range 1..=9")),
            Chord::PrevChat => alt_tap(VK_LEFT),  // Alt+Left
            Chord::NextChat => alt_tap(VK_RIGHT), // Alt+Right
            Chord::FocusInput => alt_tap(VK_RETURN), // Alt+Enter
            Chord::NewChat => alt_shift_tap(VK_N),   // Alt+Shift+N
        }
    }

    // --- Window enumeration + focus. ---

    struct FindCtx {
        needle_lower: String,
        found: HWND,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam as *mut FindCtx);
        if IsWindowVisible(hwnd) == 0 {
            return 1; // keep enumerating
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return 1;
        }
        let mut buf = vec![0u16; (len as usize) + 1];
        let copied = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if copied <= 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&buf[..copied as usize]);
        if title.to_lowercase().contains(&ctx.needle_lower) {
            ctx.found = hwnd;
            return 0; // stop enumerating — first visible match wins
        }
        1
    }

    /// Find the first visible top-level window whose title contains `needle`
    /// (case-insensitive). Returns `None` if none match.
    fn find_window_by_title(needle: &str) -> Option<HWND> {
        let mut ctx = FindCtx { needle_lower: needle.to_lowercase(), found: 0 };
        unsafe {
            EnumWindows(enum_proc, &mut ctx as *mut FindCtx as LPARAM);
        }
        if ctx.found != 0 {
            Some(ctx.found)
        } else {
            None
        }
    }

    struct DeleteRepeat {
        stop: Arc<AtomicBool>,
        worker: JoinHandle<()>,
    }

    /// Windows controller: focuses a Kiro Crew window then injects.
    pub struct WinController {
        hwnd: Option<HWND>,
        title_match: String,
        delete_repeat: Option<DeleteRepeat>,
    }

    impl WinController {
        pub fn new(title_match: Option<String>) -> Self {
            Self {
                hwnd: None,
                title_match: title_match.unwrap_or_default(),
                delete_repeat: None,
            }
        }

        pub fn set_title_match(&mut self, title: String) {
            self.title_match = title;
            self.hwnd = None; // force re-resolve on next focus
        }

        /// Resolve + foreground the Kiro Crew window. Verifies a match BEFORE any
        /// injection (AC1.5); returns `Err` and injects nothing if none found.
        pub fn focus(&mut self) -> Result<(), String> {
            // Candidate list: the configured substring first, else the defaults.
            let candidates: Vec<String> = if self.title_match.trim().is_empty() {
                DEFAULT_TITLE_CANDIDATES.iter().map(|s| s.to_string()).collect()
            } else {
                vec![self.title_match.clone()]
            };

            let hwnd = candidates
                .iter()
                .find_map(|needle| find_window_by_title(needle))
                .ok_or_else(|| {
                    format!(
                        "control: no window matches title {:?} (nothing injected)",
                        if self.title_match.trim().is_empty() {
                            DEFAULT_TITLE_CANDIDATES.join(" / ")
                        } else {
                            self.title_match.clone()
                        }
                    )
                })?;

            // Force the target to the foreground. A bare SetForegroundWindow is
            // refused by Windows' focus-stealing rules when our process is not the
            // current foreground owner, so the injected keys would land in whatever
            // window is actually focused. The reliable path is to attach OUR input
            // queue AND the current foreground window's input queue to the target's
            // thread for the duration of the call — Windows then treats us as the
            // foreground thread and honours SetForegroundWindow. Detach in all cases.
            let ok = unsafe {
                let fg = GetForegroundWindow();
                let target_thread = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
                let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
                let cur_thread = GetCurrentThreadId();

                let attached_cur = target_thread != cur_thread
                    && AttachThreadInput(cur_thread, target_thread, 1) != 0;
                let attached_fg = fg_thread != 0
                    && fg_thread != target_thread
                    && fg_thread != cur_thread
                    && AttachThreadInput(fg_thread, target_thread, 1) != 0;

                AllowSetForegroundWindow(ASFW_ANY);
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                } else {
                    ShowWindow(hwnd, SW_SHOW);
                }
                BringWindowToTop(hwnd);
                let set = SetForegroundWindow(hwnd) != 0;

                if attached_fg {
                    AttachThreadInput(fg_thread, target_thread, 0);
                }
                if attached_cur {
                    AttachThreadInput(cur_thread, target_thread, 0);
                }
                set
            };
            if !ok {
                // Non-fatal: the handle is valid and the attach dance usually still
                // lands focus even when the return is 0. Record and continue.
                eprintln!("control: SetForegroundWindow returned 0 (window still targeted)");
            }
            // Let the target actually receive focus before keys are injected —
            // SendInput otherwise races the foreground switch and keys are dropped
            // or land in the previous window.
            std::thread::sleep(std::time::Duration::from_millis(40));
            self.hwnd = Some(hwnd);
            Ok(())
        }

        pub fn send_text(&mut self, text: &str) -> Result<(), String> {
            self.focus()?;
            type_text_raw(text)
        }

        pub fn command_bar(&mut self, cmd: &str) -> Result<(), String> {
            self.focus()?;
            ctrl_k()?;
            type_text_raw(cmd)?;
            tap(VK_RETURN)
        }

        /// Focus the composer (Alt+Enter), then type — no Enter (K5 submits).
        pub fn compose(&mut self, text: &str) -> Result<(), String> {
            // Focus the window only, then type. This previously injected Alt+Enter
            // to "focus the composer", but that chord SUBMITS in Kiro Crew — so it
            // auto-sent whatever was already in the composer. Rely on the composer
            // being the default focused control; typing never submits, and the user
            // still submits explicitly with K5 (Enter).
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
            let hwnd = self.hwnd.ok_or("control: target window unavailable")?;

            // Delete one word immediately when LONG arrives, then repeat after
            // a short typematic delay while the same target remains foreground.
            ctrl_tap(VK_BACK)?;
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
                        // Never continue injecting if focus moved to another app.
                        if unsafe { GetForegroundWindow() } != hwnd {
                            break;
                        }
                        if ctrl_tap(VK_BACK).is_err() {
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

    impl Drop for WinController {
        fn drop(&mut self) {
            let _ = self.stop_delete_words();
        }
    }

    // Silence dead-code warnings for the Win32 bindings kept for the window
    // enumeration path / future close-menu use but not otherwise referenced;
    // touched here so the linker keeps them honest.
    #[allow(dead_code)]
    fn _keep_win32_linked() -> (
        unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> isize,
        unsafe extern "system" fn(HWND, *mut u16, i32) -> i32,
        unsafe extern "system" fn(HWND) -> i32,
    ) {
        (SendMessageW, GetWindowTextW, GetWindowTextLengthW)
    }
}

// =============================== macOS ===============================

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{candidate_needles, Chord, DEFAULT_TITLE_CANDIDATES};
    use crate::keymap::{self, mac_keycode};
    use crate::mac_sys;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    const DELETE_REPEAT_INITIAL_DELAY: Duration = Duration::from_millis(280);
    const DELETE_REPEAT_INTERVAL: Duration = Duration::from_millis(120);
    const DELETE_REPEAT_MAX_DURATION: Duration = Duration::from_secs(10);

    struct DeleteRepeat {
        stop: Arc<AtomicBool>,
        worker: JoinHandle<()>,
    }

    /// macOS controller: focuses a Kiro Crew window (AX) then injects (CGEvent).
    /// Caches only the PID, never AX handles, so it stays `Send`.
    pub struct MacController {
        pid: Option<i32>,
        title_match: String,
        delete_repeat: Option<DeleteRepeat>,
    }

    fn require_permission() -> Result<(), String> {
        if mac_sys::input_permission_granted() {
            Ok(())
        } else {
            Err("Accessibility permission not granted — enable it in System Settings → \
                 Privacy & Security → Accessibility (nothing injected)"
                .to_string())
        }
    }

    fn post_chord(c: Chord) -> Result<(), String> {
        mac_sys::post_strokes(&keymap::mac_chord_strokes(c)?)
    }

    impl MacController {
        pub fn new(title_match: Option<String>) -> Self {
            Self {
                pid: None,
                title_match: title_match.unwrap_or_default(),
                delete_repeat: None,
            }
        }

        pub fn set_title_match(&mut self, title: String) {
            self.title_match = title;
            self.pid = None; // force re-resolve on next focus
        }

        /// PID resolved by the last successful `focus()`.
        pub fn target_pid(&self) -> Option<i32> {
            self.pid
        }

        /// Resolve + foreground the Kiro Crew window. Checks the Accessibility
        /// grant FIRST so the user never sees a misleading "no window matches".
        /// Verifies a match BEFORE any injection; on a miss the cached PID is
        /// cleared and `Err` returned.
        pub fn focus(&mut self) -> Result<(), String> {
            require_permission()?;
            let own_pid = std::process::id() as i32;
            let mut focus_err: Option<String> = None;
            for needle in candidate_needles(&self.title_match) {
                match mac_sys::focus_window_by_title(&needle, own_pid, self.pid) {
                    Ok(pid) => {
                        self.pid = Some(pid);
                        return Ok(());
                    }
                    Err(e) if e.starts_with(mac_sys::ERR_NO_MATCH) => {}
                    // Window found but could not be raised, etc.: report as-is.
                    Err(e) => focus_err = focus_err.or(Some(e)),
                }
            }
            self.pid = None;
            if let Some(e) = focus_err {
                return Err(format!("control: {e}"));
            }
            Err(format!(
                "control: no window matches title {:?} (nothing injected)",
                if self.title_match.trim().is_empty() {
                    DEFAULT_TITLE_CANDIDATES.join(" / ")
                } else {
                    self.title_match.clone()
                }
            ))
        }

        pub fn send_text(&mut self, text: &str) -> Result<(), String> {
            self.focus()?;
            mac_sys::type_text(text)
        }

        pub fn command_bar(&mut self, cmd: &str) -> Result<(), String> {
            self.focus()?;
            post_chord(Chord::CtrlK)?;
            mac_sys::type_text(cmd)?;
            mac_sys::post_key(mac_keycode::RETURN)
        }

        /// Focus the window, then type — no Enter (K5 submits).
        pub fn compose(&mut self, text: &str) -> Result<(), String> {
            self.focus()?;
            mac_sys::type_text(text)
        }

        pub fn chord(&mut self, keys: Chord) -> Result<(), String> {
            self.focus()?;
            post_chord(keys)
        }

        pub fn start_delete_words(&mut self) -> Result<(), String> {
            self.stop_delete_words()?;
            self.focus()?;
            let pid = self.pid.ok_or("control: target window unavailable")?;
            // DeleteWord is all-physical, so the off-main worker needs no layout lookup.
            let strokes = keymap::mac_chord_strokes(Chord::DeleteWord)?;

            // Delete one word immediately, then repeat after a typematic delay
            // while the same target stays frontmost.
            mac_sys::post_strokes(&strokes)?;
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
                        // Never continue injecting if focus moved to another app.
                        if mac_sys::frontmost_pid() != Some(pid) {
                            break;
                        }
                        if mac_sys::post_strokes(&strokes).is_err() {
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

// ============================ Other hosts ============================

#[cfg(not(any(windows, target_os = "macos")))]
mod stub_impl {
    use super::Chord;

    /// Non-Windows stub: compiles and unit-tests on Linux. `focus` is a no-op
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
            Err("control: windows only".to_string())
        }

        pub fn command_bar(&mut self, _cmd: &str) -> Result<(), String> {
            Err("control: windows only".to_string())
        }

        pub fn compose(&mut self, _text: &str) -> Result<(), String> {
            Err("control: windows only".to_string())
        }

        pub fn chord(&mut self, _keys: Chord) -> Result<(), String> {
            Err("control: windows only".to_string())
        }

        pub fn start_delete_words(&mut self) -> Result<(), String> {
            Err("control: windows only".to_string())
        }

        pub fn stop_delete_words(&mut self) -> Result<(), String> {
            Ok(())
        }
    }
}

// ─────────────── Public controller facade (platform-neutral) ───────────────

#[cfg(windows)]
type Backend = windows_impl::WinController;
#[cfg(target_os = "macos")]
type Backend = macos_impl::MacController;
#[cfg(not(any(windows, target_os = "macos")))]
type Backend = stub_impl::StubController;

/// Window-title needles to try, in order, for a configured title match: the
/// configured substring alone, else [`DEFAULT_TITLE_CANDIDATES`].
pub fn candidate_needles(title_match: &str) -> Vec<String> {
    if title_match.trim().is_empty() {
        DEFAULT_TITLE_CANDIDATES.iter().map(|s| s.to_string()).collect()
    } else {
        vec![title_match.to_string()]
    }
}

/// Find-window scan order and budget rule (pure core of `mac_sys::find_window`).
/// The hint is tried first (if != `exclude_pid`; NOT subject to the budget). Only
/// on a hint miss is `candidates()` called, once, and walked in order: before
/// each pid, `budget_spent()` → return `None`; skip `pid == hint_pid` and
/// `pid == exclude_pid`; the first `probe(pid)` that returns `Some(t)` wins.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn scan_window_pids<T>(
    hint_pid: Option<i32>,
    exclude_pid: i32,
    candidates: impl FnOnce() -> Vec<i32>,
    mut budget_spent: impl FnMut() -> bool,
    mut probe: impl FnMut(i32) -> Option<T>,
) -> Option<(i32, T)> {
    if let Some(pid) = hint_pid.filter(|p| *p != exclude_pid) {
        if let Some(t) = probe(pid) {
            return Some((pid, t));
        }
    }
    for pid in candidates() {
        if budget_spent() {
            return None; // budget spent: report no match rather than stall the caller
        }
        if Some(pid) == hint_pid || pid == exclude_pid {
            continue; // already tried / ours
        }
        if let Some(t) = probe(pid) {
            return Some((pid, t));
        }
    }
    None
}

/// Pure core of `mac_sys::is_focused_window`. No focused window → false (fail
/// closed). Same AX element → true. Otherwise true only when both titles are
/// present, equal, and the target title is non-empty. (Two same-titled windows
/// of one app count as focused: pre-existing, accepted in the build reviews.)
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn focused_window_matches(
    focused_present: bool,
    same_element: bool,
    focused_title: Option<&str>,
    target_title: Option<&str>,
) -> bool {
    if !focused_present {
        return false;
    }
    if same_element {
        return true;
    }
    match (focused_title, target_title) {
        (Some(a), Some(b)) => !b.is_empty() && a == b,
        _ => false,
    }
}

/// The FP-4b controller: focus a local Kiro Crew window and inject text / the
/// command bar / chords. Windows-backed for real; a compiling no-op/Err stub
/// elsewhere. Construct with [`KiroCrewController::new`].
pub struct KiroCrewController {
    backend: Backend,
}

impl KiroCrewController {
    /// `title_match` — a window-title substring; `None`/empty uses
    /// [`DEFAULT_TITLE_CANDIDATES`] ("Kiro Crew" / "Agentic").
    pub fn new(title_match: Option<String>) -> Self {
        Self { backend: Backend::new(title_match) }
    }

    /// PID of the window the last successful `focus()` resolved (macOS); `None`
    /// elsewhere.
    pub fn target_pid(&self) -> Option<i32> {
        #[cfg(target_os = "macos")]
        {
            self.backend.target_pid()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    /// Re-configure the title substring to match (clears any cached handle).
    pub fn set_title_match(&mut self, title: String) {
        self.backend.set_title_match(title);
    }

    /// Resolve + foreground the window. Verifies a match before injecting
    /// (AC1.5); on non-Windows this is a no-op `Ok`.
    pub fn focus(&mut self) -> Result<(), String> {
        self.backend.focus()
    }

    /// Focus, then type `text` (no command bar, no Enter).
    pub fn send_text(&mut self, text: &str) -> Result<(), String> {
        self.backend.send_text(text)
    }

    /// Focus, open the command bar (Ctrl+K / ⌘K), type `cmd`, press Enter.
    pub fn command_bar(&mut self, cmd: &str) -> Result<(), String> {
        self.backend.command_bar(cmd)
    }

    /// Focus the composer (Alt+Enter), then type `text` (no Enter).
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

    // ── Stub controller behaviour on non-Windows ───────────────

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn stub_controller_focus_ok_inject_err() {
        let mut c = KiroCrewController::new(None);
        assert!(c.focus().is_ok());
        assert!(c.send_text("x").is_err());
        assert!(c.command_bar("cmd").is_err());
        assert!(c.chord(Chord::Enter).is_err());
        // apply() delegates: no-op/stop are safe; injection is Windows-only.
        assert!(c.apply(&ControlAction::None).is_ok());
        assert!(c.apply(&ControlAction::StopDeleteWords).is_ok());
        assert!(c.apply(&ControlAction::StartDeleteWords).is_err());
        assert!(c.apply(&ControlAction::Chord(Chord::Up)).is_err());
    }

    // ── Window-title needles / target pid ──────────────────────

    #[test]
    fn candidate_needles_default_and_configured() {
        assert_eq!(candidate_needles(""), vec!["Kiro Crew".to_string(), "Agentic".to_string()]);
        assert_eq!(candidate_needles("   "), vec!["Kiro Crew".to_string(), "Agentic".to_string()]);
        assert_eq!(candidate_needles("My Crew"), vec!["My Crew".to_string()]);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn target_pid_is_none_off_macos() {
        let mut c = KiroCrewController::new(None);
        assert_eq!(c.target_pid(), None);
        let _ = c.focus();
        assert_eq!(c.target_pid(), None);
    }

    // ── scan_window_pids / focused_window_matches (pure cores of mac_sys) ──

    use std::cell::{Cell, RefCell};

    #[test]
    fn scan_hint_hit_skips_enumeration_and_budget() {
        let got = scan_window_pids(
            Some(7),
            1,
            || panic!("candidates must not be enumerated on a hint hit"),
            || true,
            |pid| (pid == 7).then_some("win"),
        );
        assert_eq!(got, Some((7, "win")));
    }

    #[test]
    fn scan_hint_equal_to_exclude_is_never_probed() {
        let log = RefCell::new(Vec::new());
        let got = scan_window_pids(
            Some(5),
            5,
            || vec![5, 6],
            || false,
            |pid| {
                log.borrow_mut().push(pid);
                Some(())
            },
        );
        assert_eq!(got, Some((6, ())));
        assert_eq!(*log.borrow(), vec![6]);
    }

    #[test]
    fn scan_does_not_reprobe_hint() {
        let log = RefCell::new(Vec::new());
        let got = scan_window_pids(
            Some(3),
            1,
            || vec![3, 4],
            || false,
            |pid| {
                log.borrow_mut().push(pid);
                (pid == 4).then_some(())
            },
        );
        assert_eq!(got, Some((4, ())));
        assert_eq!(*log.borrow(), vec![3, 4]);
    }

    #[test]
    fn scan_returns_first_match_in_candidate_order() {
        let got = scan_window_pids(
            None,
            0,
            || vec![1, 2, 3],
            || false,
            |pid| (pid >= 2).then_some(pid * 10),
        );
        assert_eq!(got, Some((2, 20)));
    }

    #[test]
    fn scan_stops_when_budget_spent() {
        let calls = Cell::new(0);
        let log = RefCell::new(Vec::new());
        let got = scan_window_pids(
            None,
            0,
            || vec![1, 2, 3],
            || {
                calls.set(calls.get() + 1);
                calls.get() >= 3
            },
            |pid| {
                log.borrow_mut().push(pid);
                (pid == 3).then_some(())
            },
        );
        assert_eq!(got, None);
        assert_eq!(*log.borrow(), vec![1, 2]);
    }

    #[test]
    fn scan_budget_spent_before_first_candidate() {
        let got = scan_window_pids::<()>(
            None,
            0,
            || vec![1, 2],
            || true,
            |pid| panic!("probe({pid}) called after budget spent"),
        );
        assert_eq!(got, None);
    }

    #[test]
    fn focused_window_matches_rules() {
        let k = Some("Kiro Crew");
        // No focused window fails closed even if the titles would match.
        assert!(!focused_window_matches(false, true, k, k));
        // Same AX element.
        assert!(focused_window_matches(true, true, None, None));
        // Different elements, same non-empty title: current accepted behaviour.
        assert!(focused_window_matches(true, false, k, k));
        // Empty titles never match.
        assert!(!focused_window_matches(true, false, Some(""), Some("")));
        // A missing title on either side never matches.
        assert!(!focused_window_matches(true, false, Some("x"), None));
        assert!(!focused_window_matches(true, false, None, Some("x")));
        // Different windows of the app.
        assert!(!focused_window_matches(true, false, k, Some("Gmail")));
    }
}
