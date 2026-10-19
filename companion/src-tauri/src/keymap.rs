//! Pure macOS keystroke planning (chord → virtual keycodes + modifier flags).
//!
//! Everything here is host-independent so it is unit-tested on Linux. The
//! macOS-only injection backend lives in `mac_sys.rs`.
//!
//! U1 scaffold: the signatures below are final; the `todo!("U2")` bodies are
//! replaced by unit U2. No placeholder is reachable from existing tests or
//! from Windows code paths.

/// Maximum UTF-16 code units per synthesized Unicode keyboard event (the
/// system truncates longer strings).
pub const MAC_UNICODE_CHUNK: usize = 20;

/// macOS virtual keycodes (`kVK_*`, HIToolbox/Events.h).
pub mod mac_keycode {
    pub const RETURN: u16 = 0x24;
    pub const DELETE: u16 = 0x33;
    pub const ESCAPE: u16 = 0x35;
    pub const LEFT: u16 = 0x7B;
    pub const RIGHT: u16 = 0x7C;
    pub const DOWN: u16 = 0x7D;
    pub const UP: u16 = 0x7E;
    pub const ANSI_A: u16 = 0x00;
    pub const ANSI_K: u16 = 0x28;
    pub const ANSI_N: u16 = 0x2D;
    pub const COMMAND: u16 = 0x37;
    pub const RIGHT_COMMAND: u16 = 0x36;
    pub const SHIFT: u16 = 0x38;
    pub const OPTION: u16 = 0x3A;
    pub const CONTROL: u16 = 0x3B;
}

/// Modifier set for one keystroke.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mods {
    pub cmd: bool,
    pub ctrl: bool,
    pub opt: bool,
    pub shift: bool,
}

impl Mods {
    /// Combined `CGEventFlags` bits for this modifier set.
    pub fn cg_flags(self) -> u64 {
        todo!("U2")
    }

    /// `(modifier keycode, accumulated flags after pressing it)` in press
    /// order: cmd, ctrl, opt, shift.
    pub fn modifier_keycodes(self) -> Vec<(u16, u64)> {
        todo!("U2")
    }
}

/// How a stroke's key is identified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacKey {
    /// A fixed (physical, ANSI) virtual keycode.
    Code(u16),
    /// A key resolved from the current keyboard layout by character.
    Char(char),
}

/// One key press with its modifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyStroke {
    pub mods: Mods,
    pub key: MacKey,
}

/// Dictation menu entry kind. Lives here (not `pc_asr`) so `mac_sys` has no
/// `pc_asr` dependency.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DictationMenuItem {
    Start,
    Stop,
}

/// Physical keycode of the digit key `n` (1..=9), else `None`.
pub fn mac_digit_keycode(n: u8) -> Option<u16> {
    let _ = n;
    todo!("U2")
}

/// ANSI keycode for a character. Test/reference only — never a runtime
/// fallback (see plan D4).
pub fn ansi_keycode_for_char(c: char) -> Option<u16> {
    let _ = c;
    todo!("U2")
}

/// Every `(char, with_cmd)` a `MacKey::Char` stroke can need.
pub const LAYOUT_CACHE_KEYS: &[(char, bool)] = &[('a', true), ('k', true)];

/// Plan the keystrokes for a chord on macOS.
pub fn mac_chord_strokes(c: crate::kirocrew_control::Chord) -> Result<Vec<KeyStroke>, String> {
    let _ = c;
    todo!("U2")
}

/// Split `text` into UTF-16 chunks of at most `max_units` (clamped to ≥ 2)
/// without splitting a surrogate pair.
pub fn utf16_chunks(text: &str, max_units: usize) -> Vec<Vec<u16>> {
    let _ = (text, max_units);
    todo!("U2")
}
