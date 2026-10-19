//! FP-5 pc-asr-activate — activate the PC's own dictation on a board voice event.
//!
//! MVP requirement (4): besides the board mic → Tuya cloud ASR (FP-3), the user
//! can one-key activate the PC's own ASR. When the active voice source is
//! `System`, a board `voice.start` (K4 PTT) toggles Windows Voice Typing via the
//! **Win+H** chord, so the OS types recognized speech into the focused window
//! (e.g. the Kiro composer). No API keys, no companion-side recognizer needed.
//!
//! Platform strategy mirrors `inject.rs` / `asr.rs`: a trait with a real
//! `#[cfg(windows)]` `SendInput` impl and a non-Windows stub, so the Linux build
//! stays crate-clean and the pure router below is unit-tested on any host.

use crate::protocol::VoiceState;

/// Which ASR the system uses for a board voice event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceSource {
    /// Board records on its own mic → Tuya cloud ASR (FP-3). Companion ignores.
    Cloud,
    /// PC's own ASR: companion activates OS dictation (this probe).
    System,
}

impl Default for VoiceSource {
    fn default() -> Self {
        VoiceSource::Cloud
    }
}

impl VoiceSource {
    /// Parse the UI/board `voice_engine` string; unknown values fall back to Cloud.
    pub fn parse(s: &str) -> VoiceSource {
        match s.trim().to_ascii_lowercase().as_str() {
            "system" | "pc" | "windows" => VoiceSource::System,
            _ => VoiceSource::Cloud,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            VoiceSource::Cloud => "cloud",
            VoiceSource::System => "system",
        }
    }
}

/// What the companion should do for a given (source, voice-state) pair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VoiceAction {
    /// Activate the PC's own dictation.
    Activate,
    /// Do nothing (board owns the audio, or not a start edge).
    Ignore,
}

/// Pure routing decision — no IO, unit-tested on the Linux host.
pub fn route_voice(source: VoiceSource, state: VoiceState) -> VoiceAction {
    match (source, state) {
        (VoiceSource::System, VoiceState::Start) => VoiceAction::Activate,
        _ => VoiceAction::Ignore,
    }
}

/// Activates the OS's built-in dictation (types into the focused window).
pub trait PcAsrActivator: Send {
    /// Toggle the OS dictation bar (route A: Win+H on Windows).
    fn activate(&self) -> Result<(), String>;
    /// True when this backend can actually activate on the current host.
    fn is_available(&self) -> bool;
}

/// macOS dictation trigger (stored in `binds.json` as `mac_dictation_trigger`).
/// U1 scaffold: `parse`/`as_str` are `todo!("U3")`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DictationTrigger {
    #[default]
    EditMenu,
    DoubleControl,
    DoubleRightCommand,
    DoubleLeftCommand,
}

impl DictationTrigger {
    pub fn parse(s: &str) -> Self {
        let _ = s;
        todo!("U3")
    }

    pub fn as_str(self) -> &'static str {
        todo!("U3")
    }
}

/// Classify an Edit-menu title as a dictation Start/Stop entry.
pub fn dictation_menu_match(title: &str) -> Option<crate::keymap::DictationMenuItem> {
    let _ = title;
    todo!("U3")
}

/// `(modifier keycode, device flags)` for a double-tap trigger.
pub fn double_tap_spec(t: DictationTrigger) -> Option<(u16, u64)> {
    let _ = t;
    todo!("U3")
}

/// Human label of the activation chord/trigger for the given host OS.
pub fn activation_label_for(os: crate::platform::HostOs, t: DictationTrigger) -> String {
    let _ = (os, t);
    todo!("U3")
}

/// [`activation_label_for`] for the current host.
pub fn activation_label(t: DictationTrigger) -> String {
    activation_label_for(crate::platform::HostOs::current(), t)
}

/// Construct the platform activator. `trigger` and `target_pid` are consumed by
/// the macOS backend (U3); Windows ignores them.
pub fn activator(trigger: DictationTrigger, target_pid: Option<i32>) -> Box<dyn PcAsrActivator> {
    let _ = (trigger, target_pid);
    #[cfg(windows)]
    {
        Box::new(windows_impl::WinHActivator)
    }
    #[cfg(not(windows))]
    {
        Box::new(stub::StubActivator)
    }
}

// ───────────────────────────── Windows ─────────────────────────────

#[cfg(windows)]
mod windows_impl {
    use super::PcAsrActivator;
    use std::mem::size_of;

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_LWIN: u16 = 0x5B;
    const VK_H: u16 = 0x48;

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

    #[link(name = "user32")]
    extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const INPUT, cb_size: i32) -> u32;
    }

    fn vkey_event(vk: u16, key_up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            ki: KEYBDINPUT {
                w_vk: vk,
                w_scan: 0,
                dw_flags: if key_up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dw_extra_info: 0,
            },
            _pad: [0; 8],
        }
    }

    pub struct WinHActivator;

    impl PcAsrActivator for WinHActivator {
        fn activate(&self) -> Result<(), String> {
            // Win+H toggles Windows Voice Typing. Press order: LWIN down, H down,
            // H up, LWIN up.
            let events = [
                vkey_event(VK_LWIN, false),
                vkey_event(VK_H, false),
                vkey_event(VK_H, true),
                vkey_event(VK_LWIN, true),
            ];
            let sent = unsafe {
                SendInput(
                    events.len() as u32,
                    events.as_ptr(),
                    size_of::<INPUT>() as i32,
                )
            };
            if sent as usize == events.len() {
                Ok(())
            } else {
                Err(format!("SendInput injected {sent}/{} events", events.len()))
            }
        }

        fn is_available(&self) -> bool {
            true
        }
    }
}

// ─────────────────────────── Non-Windows ───────────────────────────

#[cfg(not(windows))]
mod stub {
    use super::PcAsrActivator;

    pub struct StubActivator;

    impl PcAsrActivator for StubActivator {
        fn activate(&self) -> Result<(), String> {
            eprintln!("pc_asr: OS dictation activation unsupported on this host (Windows-only)");
            Ok(())
        }

        fn is_available(&self) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_is_default_source() {
        assert_eq!(VoiceSource::default(), VoiceSource::Cloud);
    }

    #[test]
    fn system_start_activates() {
        assert_eq!(
            route_voice(VoiceSource::System, VoiceState::Start),
            VoiceAction::Activate
        );
    }

    #[test]
    fn cloud_never_activates() {
        assert_eq!(
            route_voice(VoiceSource::Cloud, VoiceState::Start),
            VoiceAction::Ignore
        );
        assert_eq!(
            route_voice(VoiceSource::Cloud, VoiceState::Stop),
            VoiceAction::Ignore
        );
    }

    #[test]
    fn system_stop_does_not_activate() {
        assert_eq!(
            route_voice(VoiceSource::System, VoiceState::Stop),
            VoiceAction::Ignore
        );
    }

    #[test]
    fn source_parse_roundtrip() {
        assert_eq!(VoiceSource::parse("system"), VoiceSource::System);
        assert_eq!(VoiceSource::parse("SYSTEM"), VoiceSource::System);
        assert_eq!(VoiceSource::parse("cloud"), VoiceSource::Cloud);
        assert_eq!(VoiceSource::parse("bogus"), VoiceSource::Cloud);
        assert_eq!(VoiceSource::System.as_str(), "system");
        assert_eq!(VoiceSource::Cloud.as_str(), "cloud");
    }

    #[cfg(not(windows))]
    #[test]
    fn stub_activator_unavailable_and_ok() {
        let a = activator(DictationTrigger::default(), None);
        assert!(!a.is_available());
        assert!(a.activate().is_ok());
    }
}
