//! macOS system bindings: CGEvent injection, Accessibility (AX) window focus,
//! TCC permission checks and the keyboard-layout cache.
//!
//! Threading rules (plan D4, binding on every implementer):
//! * TIS (`TISCopyCurrentKeyboardLayoutInputSource` & co.) is main-thread-only
//!   and runs ONLY inside [`refresh_layout_cache`] and the main-thread branch
//!   of [`keycode_for_char`].
//! * Off-main callers read the layout cache only; they never dispatch to, or
//!   wait on, the main thread.
//! * `LAYOUT_CACHE` is a leaf lock: never held while acquiring another lock and
//!   never held across a TIS call.
//! * AX calls block up to the messaging timeout (0.25 s) per unresponsive app.
//!
//! U1 scaffold: signatures are final, bodies are `todo!("U2")` placeholders
//! that unit U2 replaces.

#![allow(unused_variables)]

use crate::keymap::{DictationMenuItem, KeyStroke};

/// `AXIsProcessTrusted()` — the single grant covering AX focus and posting.
pub fn input_permission_granted() -> bool {
    todo!("U2")
}

/// `CGPreflightPostEventAccess()` — diagnostic only.
pub fn post_event_access_preflight() -> bool {
    todo!("U2")
}

/// Show the one system Accessibility prompt; returns the current grant.
pub fn request_input_permission() -> bool {
    todo!("U2")
}

/// Main thread only (no-op returning false elsewhere). Never takes an app lock.
pub fn refresh_layout_cache() -> bool {
    todo!("U2")
}

/// Main thread: TIS direct + cache update. Off main: cache read only; `None` =
/// fail closed.
pub fn keycode_for_char(c: char, with_cmd: bool) -> Option<u16> {
    todo!("U2")
}

pub fn post_strokes(strokes: &[KeyStroke]) -> Result<(), String> {
    todo!("U2")
}

pub fn post_key(keycode: u16) -> Result<(), String> {
    todo!("U2")
}

pub fn type_text(text: &str) -> Result<(), String> {
    todo!("U2")
}

pub fn double_tap_modifier(keycode: u16, flags: u64) -> Result<(), String> {
    todo!("U2")
}

pub fn find_window_pid_by_title(
    needle: &str,
    exclude_pid: i32,
    hint_pid: Option<i32>,
) -> Option<i32> {
    todo!("U2")
}

pub fn focus_window_by_title(
    needle: &str,
    exclude_pid: i32,
    hint_pid: Option<i32>,
) -> Result<i32, String> {
    todo!("U2")
}

pub fn frontmost_pid() -> Option<i32> {
    todo!("U2")
}

pub fn press_menu_item(
    pid: i32,
    matcher: fn(&str) -> Option<DictationMenuItem>,
    prefer: DictationMenuItem,
) -> Result<DictationMenuItem, String> {
    todo!("U2")
}
