//! Pure macOS keystroke planning (chord → virtual keycodes + modifier flags).
//!
//! Everything here is host-independent so it is unit-tested on Linux. The
//! macOS-only injection backend lives in `mac_sys.rs`.
//!
//! Design (plan D4): chords are planned as [`KeyStroke`]s. A stroke's key is
//! either *physical* ([`MacKey::Code`], an ANSI virtual keycode, used where the
//! target matches `KeyboardEvent.code`) or a *character* ([`MacKey::Char`],
//! resolved from the current keyboard layout by `mac_sys`, used where the
//! target matches by character, e.g. ⌘K / ⌘A). A hard-coded ANSI keycode for a
//! character would be wrong on non-US layouts (on AZERTY ANSI 0x00 is `Q`, so
//! ⌘+0x00 would quit the browser), hence the split.

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

// `CGEventFlags` device-independent modifier bits (CGEventTypes.h /
// IOLLEvent.h `NX_*MASK`).
const CG_FLAG_SHIFT: u64 = 0x0002_0000;
const CG_FLAG_CONTROL: u64 = 0x0004_0000;
const CG_FLAG_ALTERNATE: u64 = 0x0008_0000;
const CG_FLAG_COMMAND: u64 = 0x0010_0000;

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
        let mut f = 0u64;
        if self.cmd {
            f |= CG_FLAG_COMMAND;
        }
        if self.ctrl {
            f |= CG_FLAG_CONTROL;
        }
        if self.opt {
            f |= CG_FLAG_ALTERNATE;
        }
        if self.shift {
            f |= CG_FLAG_SHIFT;
        }
        f
    }

    /// `(modifier keycode, accumulated flags after pressing it)` in press
    /// order: cmd, ctrl, opt, shift.
    pub fn modifier_keycodes(self) -> Vec<(u16, u64)> {
        let mut out = Vec::with_capacity(4);
        let mut acc = 0u64;
        for (on, keycode, flag) in [
            (self.cmd, mac_keycode::COMMAND, CG_FLAG_COMMAND),
            (self.ctrl, mac_keycode::CONTROL, CG_FLAG_CONTROL),
            (self.opt, mac_keycode::OPTION, CG_FLAG_ALTERNATE),
            (self.shift, mac_keycode::SHIFT, CG_FLAG_SHIFT),
        ] {
            if on {
                acc |= flag;
                out.push((keycode, acc));
            }
        }
        out
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
    // kVK_ANSI_1 … kVK_ANSI_9 (HIToolbox/Events.h) — not contiguous.
    match n {
        1 => Some(0x12),
        2 => Some(0x13),
        3 => Some(0x14),
        4 => Some(0x15),
        5 => Some(0x17),
        6 => Some(0x16),
        7 => Some(0x1A),
        8 => Some(0x1C),
        9 => Some(0x19),
        _ => None,
    }
}

/// ANSI keycode for a character. Test/reference only — never a runtime
/// fallback (see plan D4).
pub fn ansi_keycode_for_char(c: char) -> Option<u16> {
    // kVK_ANSI_* on a US (ANSI) layout; letters are case-insensitive.
    let code = match c.to_ascii_lowercase() {
        'a' => 0x00,
        's' => 0x01,
        'd' => 0x02,
        'f' => 0x03,
        'h' => 0x04,
        'g' => 0x05,
        'z' => 0x06,
        'x' => 0x07,
        'c' => 0x08,
        'v' => 0x09,
        'b' => 0x0B,
        'q' => 0x0C,
        'w' => 0x0D,
        'e' => 0x0E,
        'r' => 0x0F,
        'y' => 0x10,
        't' => 0x11,
        'o' => 0x1F,
        'u' => 0x20,
        'i' => 0x22,
        'p' => 0x23,
        'l' => 0x25,
        'j' => 0x26,
        'k' => 0x28,
        'n' => 0x2D,
        'm' => 0x2E,
        '0' => 0x1D,
        d @ '1'..='9' => return mac_digit_keycode(d as u8 - b'0'),
        _ => return None,
    };
    Some(code)
}

/// Every `(char, with_cmd)` a `MacKey::Char` stroke can need.
pub const LAYOUT_CACHE_KEYS: &[(char, bool)] = &[('a', true), ('k', true)];

/// Plan the keystrokes for a chord on macOS.
pub fn mac_chord_strokes(c: crate::kirocrew_control::Chord) -> Result<Vec<KeyStroke>, String> {
    use crate::kirocrew_control::Chord;

    let none = Mods::default();
    let cmd = Mods { cmd: true, ..Mods::default() };
    let opt = Mods { opt: true, ..Mods::default() };
    let code = |mods: Mods, k: u16| KeyStroke { mods, key: MacKey::Code(k) };

    Ok(match c {
        Chord::Enter => vec![code(none, mac_keycode::RETURN)],
        Chord::Esc => vec![code(none, mac_keycode::ESCAPE)],
        Chord::Up => vec![code(none, mac_keycode::UP)],
        Chord::Down => vec![code(none, mac_keycode::DOWN)],
        Chord::Left => vec![code(none, mac_keycode::LEFT)],
        Chord::Right => vec![code(none, mac_keycode::RIGHT)],
        Chord::Backspace => vec![code(none, mac_keycode::DELETE)],
        // macOS word delete is Option+Delete. All-physical on purpose: the
        // delete-repeat worker runs off the main thread and must need no
        // layout (TIS) lookup.
        Chord::DeleteWord => vec![code(opt, mac_keycode::DELETE)],
        // Ctrl+A is "line start" on macOS; select-all is ⌘A (by character).
        Chord::ClearInput => vec![
            KeyStroke { mods: cmd, key: MacKey::Char('a') },
            code(none, mac_keycode::DELETE),
        ],
        // Kiro Crew's command bar accepts ⌘K (by character); Ctrl+K is
        // kill-line on macOS.
        Chord::CtrlK => vec![KeyStroke { mods: cmd, key: MacKey::Char('k') }],
        // Kiro Crew's mac binding for `chat-N` is Control+Digit (by `e.code`).
        Chord::JumpChat(n) => match mac_digit_keycode(n) {
            Some(k) => vec![code(Mods { ctrl: true, ..Mods::default() }, k)],
            None => return Err(format!("JumpChat({n}) out of range 1..=9")),
        },
        Chord::PrevChat => vec![code(opt, mac_keycode::LEFT)],
        Chord::NextChat => vec![code(opt, mac_keycode::RIGHT)],
        Chord::FocusInput => vec![code(opt, mac_keycode::RETURN)],
        Chord::NewChat => vec![code(
            Mods { opt: true, shift: true, ..Mods::default() },
            mac_keycode::ANSI_N,
        )],
    })
}

/// Resolve each stroke's keycode. `MacKey::Code(k)` → k (`lookup` NOT called);
/// `MacKey::Char(c)` → `lookup(c, stroke.mods.cmd)`. The first miss returns
/// `Err("keyboard layout key '{c}' not resolved yet — nothing injected (open the Companion window once)")`
/// and no codes. Never falls back to [`ansi_keycode_for_char`] (fail closed: a
/// wrong physical code on a non-US layout could turn ⌘A into ⌘Q).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn resolve_stroke_codes(
    strokes: &[KeyStroke],
    mut lookup: impl FnMut(char, bool) -> Option<u16>,
) -> Result<Vec<u16>, String> {
    let mut codes = Vec::with_capacity(strokes.len());
    for s in strokes {
        let code = match s.key {
            MacKey::Code(k) => k,
            MacKey::Char(c) => lookup(c, s.mods.cmd).ok_or_else(|| {
                format!(
                    "keyboard layout key '{c}' not resolved yet — nothing injected (open the Companion window once)"
                )
            })?,
        };
        codes.push(code);
    }
    Ok(codes)
}

/// Split `text` into UTF-16 chunks of at most `max_units` (clamped to ≥ 2)
/// without splitting a surrogate pair.
pub fn utf16_chunks(text: &str, max_units: usize) -> Vec<Vec<u16>> {
    let max = max_units.max(2);
    let mut chunks: Vec<Vec<u16>> = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    let mut buf = [0u16; 2];
    for ch in text.chars() {
        let units = ch.encode_utf16(&mut buf);
        if !cur.is_empty() && cur.len() + units.len() > max {
            chunks.push(std::mem::take(&mut cur));
        }
        cur.extend_from_slice(units);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kirocrew_control::Chord;

    fn all_chords() -> Vec<Chord> {
        let mut v = vec![
            Chord::Enter,
            Chord::Esc,
            Chord::Up,
            Chord::Down,
            Chord::Left,
            Chord::Right,
            Chord::Backspace,
            Chord::DeleteWord,
            Chord::ClearInput,
            Chord::CtrlK,
            Chord::PrevChat,
            Chord::NextChat,
            Chord::FocusInput,
            Chord::NewChat,
        ];
        v.extend((1..=9).map(Chord::JumpChat));
        v
    }

    #[test]
    fn cg_flags_values_and_combinations() {
        assert_eq!(Mods::default().cg_flags(), 0);
        assert_eq!(Mods { cmd: true, ..Mods::default() }.cg_flags(), 0x100000);
        assert_eq!(Mods { ctrl: true, ..Mods::default() }.cg_flags(), 0x40000);
        assert_eq!(Mods { opt: true, ..Mods::default() }.cg_flags(), 0x80000);
        assert_eq!(Mods { shift: true, ..Mods::default() }.cg_flags(), 0x20000);
        assert_eq!(
            Mods { cmd: true, ctrl: true, opt: true, shift: true }.cg_flags(),
            0x100000 | 0x40000 | 0x80000 | 0x20000
        );
    }

    #[test]
    fn modifier_keycodes_order_and_accumulation() {
        assert!(Mods::default().modifier_keycodes().is_empty());
        let all = Mods { cmd: true, ctrl: true, opt: true, shift: true };
        assert_eq!(
            all.modifier_keycodes(),
            vec![
                (mac_keycode::COMMAND, 0x100000),
                (mac_keycode::CONTROL, 0x100000 | 0x40000),
                (mac_keycode::OPTION, 0x100000 | 0x40000 | 0x80000),
                (mac_keycode::SHIFT, all.cg_flags()),
            ]
        );
        let os = Mods { opt: true, shift: true, ..Mods::default() };
        assert_eq!(
            os.modifier_keycodes(),
            vec![(mac_keycode::OPTION, 0x80000), (mac_keycode::SHIFT, 0x80000 | 0x20000)]
        );
    }

    #[test]
    fn digit_keycodes_match_kvk_ansi_table() {
        let want = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
        for (i, w) in want.iter().enumerate() {
            assert_eq!(mac_digit_keycode(i as u8 + 1), Some(*w));
        }
        assert_eq!(mac_digit_keycode(0), None);
        assert_eq!(mac_digit_keycode(10), None);
    }

    #[test]
    fn ansi_keycode_reference_values() {
        assert_eq!(ansi_keycode_for_char('a'), Some(mac_keycode::ANSI_A));
        assert_eq!(ansi_keycode_for_char('K'), Some(mac_keycode::ANSI_K));
        assert_eq!(ansi_keycode_for_char('n'), Some(mac_keycode::ANSI_N));
        assert_eq!(ansi_keycode_for_char('3'), Some(0x14));
        assert_eq!(ansi_keycode_for_char('0'), Some(0x1D));
        assert_eq!(ansi_keycode_for_char('é'), None);
    }

    fn stroke(mods: Mods, key: MacKey) -> KeyStroke {
        KeyStroke { mods, key }
    }

    #[test]
    fn d4_chord_table() {
        let none = Mods::default();
        let cmd = Mods { cmd: true, ..none };
        let opt = Mods { opt: true, ..none };
        let ctrl = Mods { ctrl: true, ..none };
        let cases: Vec<(Chord, Vec<KeyStroke>)> = vec![
            (Chord::Enter, vec![stroke(none, MacKey::Code(0x24))]),
            (Chord::Esc, vec![stroke(none, MacKey::Code(0x35))]),
            (Chord::Left, vec![stroke(none, MacKey::Code(0x7B))]),
            (Chord::Right, vec![stroke(none, MacKey::Code(0x7C))]),
            (Chord::Down, vec![stroke(none, MacKey::Code(0x7D))]),
            (Chord::Up, vec![stroke(none, MacKey::Code(0x7E))]),
            (Chord::Backspace, vec![stroke(none, MacKey::Code(0x33))]),
            (Chord::DeleteWord, vec![stroke(opt, MacKey::Code(0x33))]),
            (
                Chord::ClearInput,
                vec![stroke(cmd, MacKey::Char('a')), stroke(none, MacKey::Code(0x33))],
            ),
            (Chord::CtrlK, vec![stroke(cmd, MacKey::Char('k'))]),
            (Chord::PrevChat, vec![stroke(opt, MacKey::Code(0x7B))]),
            (Chord::NextChat, vec![stroke(opt, MacKey::Code(0x7C))]),
            (Chord::FocusInput, vec![stroke(opt, MacKey::Code(0x24))]),
            (
                Chord::NewChat,
                vec![stroke(Mods { opt: true, shift: true, ..none }, MacKey::Code(0x2D))],
            ),
        ];
        for (chord, want) in cases {
            assert_eq!(mac_chord_strokes(chord).unwrap(), want, "{chord:?}");
        }
        let digits = [0x12u16, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
        for (i, k) in digits.iter().enumerate() {
            assert_eq!(
                mac_chord_strokes(Chord::JumpChat(i as u8 + 1)).unwrap(),
                vec![stroke(ctrl, MacKey::Code(*k))]
            );
        }
        assert!(mac_chord_strokes(Chord::JumpChat(0)).is_err());
        assert!(mac_chord_strokes(Chord::JumpChat(10)).is_err());
    }

    #[test]
    fn only_jump_chat_uses_control() {
        for c in all_chords() {
            let uses_ctrl = mac_chord_strokes(c).unwrap().iter().any(|s| s.mods.ctrl);
            assert_eq!(uses_ctrl, matches!(c, Chord::JumpChat(_)), "{c:?}");
        }
    }

    #[test]
    fn delete_word_is_all_physical() {
        // The delete-repeat worker runs off the main thread: no TIS lookup.
        assert!(mac_chord_strokes(Chord::DeleteWord)
            .unwrap()
            .iter()
            .all(|s| matches!(s.key, MacKey::Code(_))));
    }

    #[test]
    fn layout_cache_keys_cover_every_char_stroke() {
        for c in all_chords() {
            for s in mac_chord_strokes(c).unwrap() {
                if let MacKey::Char(ch) = s.key {
                    assert!(
                        LAYOUT_CACHE_KEYS.contains(&(ch, s.mods.cmd)),
                        "{c:?} needs ({ch:?}, {}) in LAYOUT_CACHE_KEYS",
                        s.mods.cmd
                    );
                }
            }
        }
    }

    #[test]
    fn cmd_strokes_use_char_keys_only() {
        // No ⌘ ever pairs with a hard-coded physical code: on AZERTY, ANSI 0x00
        // is Q, so ⌘+0x00 would be ⌘Q (quit).
        for c in all_chords() {
            for s in mac_chord_strokes(c).unwrap() {
                assert_eq!(
                    s.mods.cmd,
                    matches!(s.key, MacKey::Char(_)),
                    "{c:?}: cmd and Char keys must go together ({s:?})"
                );
            }
        }
    }

    #[test]
    fn no_chord_plans_cmd_q() {
        // Readable guard; implied by cmd_strokes_use_char_keys_only + d4_chord_table.
        let cmd = Mods { cmd: true, ..Mods::default() };
        let bad = [
            stroke(cmd, MacKey::Char('q')),
            stroke(cmd, MacKey::Code(0x0C)), // ANSI Q
            stroke(cmd, MacKey::Code(0x00)), // ANSI A = AZERTY Q
        ];
        for c in all_chords() {
            for s in mac_chord_strokes(c).unwrap() {
                assert!(!bad.contains(&s), "{c:?} plans a quit-capable stroke {s:?}");
            }
        }
    }

    #[test]
    fn layout_cache_keys_have_no_dead_or_duplicate_entries() {
        use std::collections::HashSet;
        let mut needed: HashSet<(char, bool)> = HashSet::new();
        for c in all_chords() {
            for s in mac_chord_strokes(c).unwrap() {
                if let MacKey::Char(ch) = s.key {
                    needed.insert((ch, s.mods.cmd));
                }
            }
        }
        let cached: HashSet<(char, bool)> = LAYOUT_CACHE_KEYS.iter().copied().collect();
        assert_eq!(cached.len(), LAYOUT_CACHE_KEYS.len(), "duplicate LAYOUT_CACHE_KEYS entry");
        assert_eq!(cached, needed, "LAYOUT_CACHE_KEYS must be exactly the Char strokes in use");
    }

    #[test]
    fn resolve_stroke_codes_uses_lookup_for_char_keys() {
        // AZERTY-like: 'a' is on physical 0x0C, 'k' stays on 0x28.
        let table = |c: char, cmd: bool| match (c, cmd) {
            ('a', true) => Some(0x0Cu16),
            ('k', true) => Some(0x28),
            _ => None,
        };
        let mut seen: Vec<(char, bool)> = Vec::new();
        let clear = mac_chord_strokes(Chord::ClearInput).unwrap();
        let codes = resolve_stroke_codes(&clear, |c, cmd| {
            seen.push((c, cmd));
            table(c, cmd)
        })
        .unwrap();
        assert_eq!(codes, vec![0x0C, 0x33]);
        assert_eq!(seen, vec![('a', true)]);

        let k = mac_chord_strokes(Chord::CtrlK).unwrap();
        assert_eq!(resolve_stroke_codes(&k, table).unwrap(), vec![0x28]);
    }

    #[test]
    fn resolve_stroke_codes_physical_keys_never_call_lookup() {
        let mut chords = vec![Chord::DeleteWord, Chord::Enter, Chord::NewChat];
        chords.extend((1..=9).map(Chord::JumpChat));
        for c in chords {
            let strokes = mac_chord_strokes(c).unwrap();
            let want: Vec<u16> = strokes
                .iter()
                .map(|s| match s.key {
                    MacKey::Code(k) => k,
                    MacKey::Char(_) => unreachable!("{c:?} must be all-physical"),
                })
                .collect();
            let got = resolve_stroke_codes(&strokes, |ch, _| {
                panic!("lookup called for {ch:?} in {c:?}")
            })
            .unwrap();
            assert_eq!(got, want, "{c:?}");
        }
        // Spot-check against the D4 keycodes.
        let d = mac_chord_strokes(Chord::DeleteWord).unwrap();
        assert_eq!(resolve_stroke_codes(&d, |_, _| None).unwrap(), vec![0x33]);
        let n = mac_chord_strokes(Chord::NewChat).unwrap();
        assert_eq!(resolve_stroke_codes(&n, |_, _| None).unwrap(), vec![0x2D]);
        let j = mac_chord_strokes(Chord::JumpChat(5)).unwrap();
        assert_eq!(resolve_stroke_codes(&j, |_, _| None).unwrap(), vec![0x17]);
    }

    #[test]
    fn resolve_stroke_codes_fails_closed_on_miss() {
        let k = mac_chord_strokes(Chord::CtrlK).unwrap();
        assert_eq!(
            resolve_stroke_codes(&k, |_, _| None).unwrap_err(),
            "keyboard layout key 'k' not resolved yet — nothing injected (open the Companion window once)"
        );
        let clear = mac_chord_strokes(Chord::ClearInput).unwrap();
        let err = resolve_stroke_codes(&clear, |_, _| None).unwrap_err();
        assert!(err.contains("'a'"), "{err}");
        // The ANSI table has 'a', yet the result is Err: there is no fallback.
        assert!(ansi_keycode_for_char('a').is_some());
    }

    #[test]
    fn resolve_stroke_codes_partial_miss_names_missing_key() {
        let mut strokes = mac_chord_strokes(Chord::ClearInput).unwrap();
        strokes.extend(mac_chord_strokes(Chord::CtrlK).unwrap());
        let err = resolve_stroke_codes(&strokes, |c, _| (c == 'a').then_some(0x0C)).unwrap_err();
        assert!(err.contains("'k'"), "{err}");
        assert!(!err.contains("'a'"), "{err}");
    }

    /// Deliberate source-grep gate (mac_sys is not compiled on Linux, so this is
    /// the only Linux-side guard). It also fails on a *comment* that mentions
    /// `dispatch_` or the ANSI char-to-keycode table, and that is intentional:
    /// reword the comment rather than this test.
    #[test]
    fn runtime_sources_never_use_ansi_table_or_dispatch() {
        let ansi = concat!("ansi_keycode", "_for_char");
        let dispatch = concat!("dispatch", "_");
        let mac_sys = include_str!("mac_sys.rs");
        for (name, src) in [
            ("mac_sys.rs", mac_sys),
            ("kirocrew_control.rs", include_str!("kirocrew_control.rs")),
            ("lib.rs", include_str!("lib.rs")),
            ("inject.rs", include_str!("inject.rs")),
        ] {
            assert!(!src.contains(ansi), "{name} must not use the ANSI table at runtime");
        }
        assert!(!mac_sys.contains(dispatch), "mac_sys.rs must not use dispatch queues");
        // Pins the pure-core wiring on a Linux host.
        for call in ["resolve_stroke_codes(", "scan_window_pids(", "focused_window_matches("] {
            assert!(mac_sys.contains(call), "mac_sys.rs must call {call}");
        }
    }

    fn flat(chunks: &[Vec<u16>]) -> Vec<u16> {
        chunks.iter().flatten().copied().collect()
    }

    #[test]
    fn utf16_chunks_basic() {
        assert!(utf16_chunks("", 20).is_empty());
        let text = "a".repeat(45);
        let chunks = utf16_chunks(&text, MAC_UNICODE_CHUNK);
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), vec![20, 20, 5]);
        assert_eq!(flat(&chunks), text.encode_utf16().collect::<Vec<_>>());
    }

    #[test]
    fn utf16_chunks_never_split_surrogate_pairs() {
        // 19 ASCII units then an astral char (2 units) must start a new chunk.
        let text = format!("{}😀tail", "a".repeat(19));
        let chunks = utf16_chunks(&text, 20);
        assert_eq!(chunks[0].len(), 19);
        assert_eq!(chunks[1][0], 0xD83D);
        assert_eq!(flat(&chunks), text.encode_utf16().collect::<Vec<_>>());
        for ch in &chunks {
            assert!(ch.len() <= 20);
            // A chunk never ends with a lone high surrogate or starts with a low one.
            assert!(!(0xD800..0xDC00).contains(ch.last().unwrap()));
            assert!(!(0xDC00..0xE000).contains(&ch[0]));
        }
    }

    #[test]
    fn utf16_chunks_cjk_and_clamp() {
        let text = "你好世界".repeat(10); // 40 BMP units
        let chunks = utf16_chunks(&text, 20);
        assert_eq!(chunks.len(), 2);
        assert_eq!(flat(&chunks), text.encode_utf16().collect::<Vec<_>>());
        // max_units of 1 clamps to 2, so an astral char still fits.
        let chunks = utf16_chunks("😀😀", 1);
        assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), vec![2, 2]);
    }
}
