//! FP-5 pc-asr-activate — activate the PC's own dictation on a board voice event.
//!
//! MVP requirement (4): besides the board mic → Tuya cloud ASR (FP-3), the user
//! can one-key activate the PC's own ASR. When the active voice source is
//! `System`, a board `voice.start` (K4 PTT) toggles Windows Voice Typing via the
//! **Win+H** chord, so the OS types recognized speech into the focused window
//! (e.g. the Kiro composer). No API keys, no companion-side recognizer needed.
//!
//! Platform strategy mirrors `inject.rs` / `asr.rs`: a trait with a real
//! `#[cfg(windows)]` `SendInput` impl, a `#[cfg(target_os = "macos")]` impl
//! (Edit-menu "Start/Stop Dictation" or a double-tap modifier, see
//! [`DictationTrigger`]) and a stub for every other host, so the Linux build
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
            "system" | "pc" | "windows" | "mac" | "macos" => VoiceSource::System,
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
///
/// `EditMenu` presses the target app's Edit ▸ Start/Stop Dictation menu item;
/// the others post a double tap of a modifier key, which must match the shortcut
/// chosen in System Settings ▸ Keyboard ▸ Dictation. Fn/🌐 is not offered:
/// it is handled below the CGEvent layer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DictationTrigger {
    #[default]
    EditMenu,
    DoubleControl,
    DoubleRightCommand,
    DoubleLeftCommand,
}

impl DictationTrigger {
    /// Parse a wire name (trimmed, case-insensitive). Unknown or empty values
    /// fall back to [`DictationTrigger::EditMenu`].
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "double-control" => DictationTrigger::DoubleControl,
            "double-right-command" => DictationTrigger::DoubleRightCommand,
            "double-left-command" => DictationTrigger::DoubleLeftCommand,
            _ => DictationTrigger::EditMenu,
        }
    }

    /// The canonical wire name; `parse(t.as_str()) == t`.
    pub fn as_str(self) -> &'static str {
        match self {
            DictationTrigger::EditMenu => "edit-menu",
            DictationTrigger::DoubleControl => "double-control",
            DictationTrigger::DoubleRightCommand => "double-right-command",
            DictationTrigger::DoubleLeftCommand => "double-left-command",
        }
    }
}

/// Classify an Edit-menu title as a dictation Start/Stop entry.
///
/// The title is trimmed, lowercased and stripped of one trailing `…` / `...`.
/// "Start Dictation" / "开始听写" is `Start`, "Stop Dictation" / "停止听写" is
/// `Stop`; any other title containing "dictation" / "听写" counts as `Start`
/// (localized variants). Everything else is `None`.
pub fn dictation_menu_match(title: &str) -> Option<crate::keymap::DictationMenuItem> {
    use crate::keymap::DictationMenuItem;
    let lower = title.trim().to_lowercase();
    let t = lower
        .strip_suffix('\u{2026}')
        .or_else(|| lower.strip_suffix("..."))
        .unwrap_or(&lower)
        .trim();
    if t.starts_with("stop dictation") || t.starts_with("停止听写") {
        Some(DictationMenuItem::Stop)
    } else if t.contains("dictation") || t.contains("听写") {
        Some(DictationMenuItem::Start)
    } else {
        None
    }
}

/// True for the primary Start/Stop Dictation entries of the Edit menu
/// ("Start Dictation…", "Stop Dictation", "开始听写…", "停止听写…"), as opposed
/// to a localized or unrelated title that merely contains "dictation" / "听写".
/// The macOS backend uses it to pick the Edit menu among the menu-bar menus.
pub fn dictation_menu_is_primary(title: &str) -> bool {
    let lower = title.trim().to_lowercase();
    let t = lower
        .strip_suffix('\u{2026}')
        .or_else(|| lower.strip_suffix("..."))
        .unwrap_or(&lower)
        .trim();
    ["start dictation", "stop dictation", "开始听写", "停止听写"]
        .iter()
        .any(|p| t.starts_with(p))
}

/// `(modifier keycode, device-dependent flags)` for a double-tap trigger; the
/// flags are the CGEvent modifier mask OR'd with the left/right device bit.
/// `EditMenu` has no double-tap form.
pub fn double_tap_spec(t: DictationTrigger) -> Option<(u16, u64)> {
    match t {
        DictationTrigger::EditMenu => None,
        // kVK_Control, kCGEventFlagMaskControl | NX_DEVICELCTLKEYMASK
        DictationTrigger::DoubleControl => Some((0x3B, 0x40000 | 0x1)),
        // kVK_RightCommand, kCGEventFlagMaskCommand | NX_DEVICERCMDKEYMASK
        DictationTrigger::DoubleRightCommand => Some((0x36, 0x100000 | 0x10)),
        // kVK_Command, kCGEventFlagMaskCommand | NX_DEVICELCMDKEYMASK
        DictationTrigger::DoubleLeftCommand => Some((0x37, 0x100000 | 0x08)),
    }
}

/// Human label of the activation chord/trigger for the given host OS.
pub fn activation_label_for(os: crate::platform::HostOs, t: DictationTrigger) -> String {
    use crate::platform::HostOs;
    match os {
        HostOs::Windows => "Win+H".to_string(),
        HostOs::Macos => format!("macOS Dictation ({})", t.as_str()),
        HostOs::Other => "unsupported".to_string(),
    }
}

/// [`activation_label_for`] for the current host.
pub fn activation_label(t: DictationTrigger) -> String {
    activation_label_for(crate::platform::HostOs::current(), t)
}

/// Construct the platform activator. `trigger` and `target_pid` (the PID the
/// last successful Kiro Crew `focus()` resolved) are consumed by the macOS
/// backend; Windows and the stub ignore them.
pub fn activator(trigger: DictationTrigger, target_pid: Option<i32>) -> Box<dyn PcAsrActivator> {
    let _ = (trigger, target_pid);
    #[cfg(windows)]
    {
        Box::new(windows_impl::WinHActivator)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos_impl::MacDictationActivator {
            trigger,
            pid: target_pid,
        })
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Box::new(stub::StubActivator)
    }
}

/// What the macOS activator will do for a (trigger, target PID) pair. Pure, so
/// the refusal rules are unit-tested on any host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum MacPlan {
    /// Press the target app's Edit-menu Start/Stop Dictation item.
    EditMenu(i32),
    /// Post a double tap of `(modifier keycode, flags)`.
    DoubleTap(u16, u64),
}

/// Decide the macOS activation. Dictation must never start without a resolved
/// Kiro Crew PID, and never against the Companion itself.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn plan_mac_activation(
    trigger: DictationTrigger,
    pid: Option<i32>,
    own_pid: i32,
) -> Result<MacPlan, String> {
    let pid = pid.ok_or_else(|| "Kiro Crew window not found — dictation not started".to_string())?;
    if pid == own_pid {
        return Err("refusing to start dictation in the Companion's own window".to_string());
    }
    Ok(match double_tap_spec(trigger) {
        None => MacPlan::EditMenu(pid),
        Some((keycode, flags)) => MacPlan::DoubleTap(keycode, flags),
    })
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

// ───────────────────────────── macOS ─────────────────────────────

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{dictation_menu_match, plan_mac_activation, DictationTrigger, MacPlan, PcAsrActivator};
    use crate::keymap::DictationMenuItem;

    /// Starts/stops macOS Dictation in the Kiro Crew window the controller
    /// last focused (`pid`). It never targets the frontmost app by guess.
    pub struct MacDictationActivator {
        pub trigger: DictationTrigger,
        pub pid: Option<i32>,
    }

    impl PcAsrActivator for MacDictationActivator {
        fn activate(&self) -> Result<(), String> {
            let plan = plan_mac_activation(self.trigger, self.pid, std::process::id() as i32)?;
            if !crate::mac_sys::input_permission_granted() {
                return Err("Accessibility permission not granted — dictation not started".to_string());
            }
            match plan {
                MacPlan::EditMenu(pid) => {
                    // Prefer "Stop" when present: toggle behaviour like Win+H.
                    crate::mac_sys::press_menu_item(pid, dictation_menu_match, DictationMenuItem::Stop)
                        .map(|_| ())
                        .map_err(|e| {
                            format!(
                                "{e} — if the app has no Dictation menu item, \
                                 switch the dictation trigger to a double-tap key"
                            )
                        })
                }
                MacPlan::DoubleTap(keycode, flags) => {
                    crate::mac_sys::double_tap_modifier(keycode, flags)
                }
            }
        }

        fn is_available(&self) -> bool {
            crate::mac_sys::input_permission_granted()
        }
    }
}

// ─────────────────────────── Other hosts ───────────────────────────

#[cfg(not(any(windows, target_os = "macos")))]
mod stub {
    use super::PcAsrActivator;

    pub struct StubActivator;

    impl PcAsrActivator for StubActivator {
        fn activate(&self) -> Result<(), String> {
            eprintln!("pc_asr: OS dictation activation unsupported on this host (Windows/macOS only)");
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

    #[test]
    fn source_parse_accepts_mac_aliases() {
        assert_eq!(VoiceSource::parse("mac"), VoiceSource::System);
        assert_eq!(VoiceSource::parse(" macOS "), VoiceSource::System);
    }

    #[test]
    fn trigger_parse_as_str_roundtrip() {
        for t in [
            DictationTrigger::EditMenu,
            DictationTrigger::DoubleControl,
            DictationTrigger::DoubleRightCommand,
            DictationTrigger::DoubleLeftCommand,
        ] {
            assert_eq!(DictationTrigger::parse(t.as_str()), t);
        }
        assert_eq!(DictationTrigger::EditMenu.as_str(), "edit-menu");
        assert_eq!(DictationTrigger::DoubleControl.as_str(), "double-control");
        assert_eq!(DictationTrigger::DoubleRightCommand.as_str(), "double-right-command");
        assert_eq!(DictationTrigger::DoubleLeftCommand.as_str(), "double-left-command");
        assert_eq!(DictationTrigger::parse(" Double-Control "), DictationTrigger::DoubleControl);
        assert_eq!(DictationTrigger::parse(""), DictationTrigger::EditMenu);
        assert_eq!(DictationTrigger::parse("bogus"), DictationTrigger::EditMenu);
        assert_eq!(DictationTrigger::default(), DictationTrigger::EditMenu);
    }

    #[test]
    fn dictation_menu_match_classifies_titles() {
        use crate::keymap::DictationMenuItem::{Start, Stop};
        for t in ["Start Dictation\u{2026}", "Start Dictation...", "start dictation", "  Start Dictation  ", "开始听写…", "开始听写"] {
            assert_eq!(dictation_menu_match(t), Some(Start), "{t:?}");
        }
        for t in ["Stop Dictation", "Stop Dictation\u{2026}", "停止听写", "停止听写…"] {
            assert_eq!(dictation_menu_match(t), Some(Stop), "{t:?}");
        }
        // Localized/variant titles that merely mention dictation count as Start.
        assert_eq!(dictation_menu_match("Démarrer la dictation"), Some(Start));
        assert_eq!(dictation_menu_match("听写"), Some(Start));
        assert_eq!(dictation_menu_match("Emoji & Symbols"), None);
        assert_eq!(dictation_menu_match(""), None);
    }

    #[test]
    fn dictation_menu_is_primary_only_for_start_stop_titles() {
        for t in ["Start Dictation\u{2026}", "Stop Dictation", "  start dictation...  ", "开始听写…", "停止听写"] {
            assert!(dictation_menu_is_primary(t), "{t:?}");
        }
        for t in ["Dictation Settings", "Démarrer la dictation", "听写", "Emoji & Symbols", ""] {
            assert!(!dictation_menu_is_primary(t), "{t:?}");
        }
    }

    #[test]
    fn double_tap_spec_values() {
        assert_eq!(double_tap_spec(DictationTrigger::EditMenu), None);
        assert_eq!(double_tap_spec(DictationTrigger::DoubleControl), Some((0x3B, 0x40001)));
        assert_eq!(double_tap_spec(DictationTrigger::DoubleRightCommand), Some((0x36, 0x100010)));
        assert_eq!(double_tap_spec(DictationTrigger::DoubleLeftCommand), Some((0x37, 0x100008)));
    }

    #[test]
    fn activation_labels() {
        use crate::platform::HostOs;
        assert_eq!(activation_label_for(HostOs::Windows, DictationTrigger::default()), "Win+H");
        assert_eq!(activation_label_for(HostOs::Windows, DictationTrigger::DoubleControl), "Win+H");
        assert_eq!(
            activation_label_for(HostOs::Macos, DictationTrigger::EditMenu),
            "macOS Dictation (edit-menu)"
        );
        assert_eq!(
            activation_label_for(HostOs::Macos, DictationTrigger::DoubleRightCommand),
            "macOS Dictation (double-right-command)"
        );
        assert_eq!(activation_label_for(HostOs::Other, DictationTrigger::EditMenu), "unsupported");
    }

    #[test]
    fn mac_plan_requires_pid_and_refuses_own_pid() {
        let own = 4242;
        for t in [
            DictationTrigger::EditMenu,
            DictationTrigger::DoubleControl,
            DictationTrigger::DoubleRightCommand,
            DictationTrigger::DoubleLeftCommand,
        ] {
            assert_eq!(
                plan_mac_activation(t, None, own),
                Err("Kiro Crew window not found — dictation not started".to_string())
            );
            assert!(plan_mac_activation(t, Some(own), own).is_err());
        }
        assert_eq!(
            plan_mac_activation(DictationTrigger::EditMenu, Some(7), own),
            Ok(MacPlan::EditMenu(7))
        );
        assert_eq!(
            plan_mac_activation(DictationTrigger::DoubleLeftCommand, Some(7), own),
            Ok(MacPlan::DoubleTap(0x37, 0x100008))
        );
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn stub_activator_unavailable_and_ok() {
        let a = activator(DictationTrigger::default(), None);
        assert!(!a.is_available());
        assert!(a.activate().is_ok());
    }
}
