//! FP-5 pc-asr-activate — activate the PC's own dictation on a board voice event.
//!
//! MVP requirement (4): besides the board mic → Tuya cloud ASR (FP-3), the user
//! can one-key activate the PC's own ASR. When the active voice source is
//! `System`, a board `voice.start` (K4 PTT) triggers macOS Dictation so the OS
//! types recognized speech into the focused window (e.g. the Kiro composer). No
//! API keys, no companion-side recognizer needed.
//!
//! macOS caveat (why this is a not-yet-wired seam, like `asr.rs`): macOS
//! Dictation's trigger is NOT a bindable modifier+key chord. System Settings >
//! Keyboard > Dictation exposes only a FIXED dropdown of shortcuts — "Press
//! Left/Right/Either Command Key Twice", "Press Control Key Twice", "Press
//! Fn/Globe", or "Off" — none of which is an arbitrary chord a program can
//! assign, and all of which are modifier *double-presses* that a single
//! `CGEventCreateKeyboardEvent` cannot reproduce. There is therefore no
//! crate-free CoreGraphics chord that reliably starts Dictation on a stock
//! machine (and Control+Space, the obvious candidate, is the system default for
//! "Select the previous input source", so posting it toggles the input menu
//! instead). Rather than claim a configuration step the OS does not expose, the
//! macOS activator is an explicit seam: `activate()` returns a descriptive
//! `Err` and `is_available()` returns `false`. Wiring real one-key activation
//! is a documented follow-up (e.g. a user-created macOS *Keyboard Shortcut* or
//! a Shortcuts.app automation that runs Dictation, which the companion could
//! then invoke, or an NSApplication-level approach), tracked alongside the
//! Apple Speech seam in `asr.rs`.
//!
//! Platform strategy mirrors `inject.rs` / `asr.rs`: a trait with a real
//! `#[cfg(target_os = "macos")]` impl and a non-macOS stub, so the Linux build
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
    /// Trigger the OS dictation. On macOS this is currently a not-yet-wired
    /// seam that returns `Err` (see the module doc: Dictation's trigger is a
    /// fixed modifier double-press, not a bindable chord).
    fn activate(&self) -> Result<(), String>;
    /// True when this backend can actually activate on the current host.
    fn is_available(&self) -> bool;
}

/// Construct the platform activator.
pub fn activator() -> Box<dyn PcAsrActivator> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos_impl::DictationActivator)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(stub::StubActivator)
    }
}

// ────────────────────────────── macOS ──────────────────────────────

/// macOS Dictation activation — a not-yet-wired seam (mirrors `asr.rs`).
///
/// There is no crate-free, reliable way to start macOS Dictation from a
/// synthetic key event: the OS exposes Dictation's trigger only as a fixed
/// dropdown of modifier *double-presses* (see the module doc), none of which a
/// single `CGEventCreateKeyboardEvent` chord can reproduce, and the obvious
/// Control+Space candidate is the default input-source switch. Posting a chord
/// here would silently do the wrong thing while *claiming* success, so instead
/// `activate()` returns a descriptive `Err` and `is_available()` is `false`
/// — exactly like the Apple Speech seam in `asr.rs`. A follow-up can wire real
/// activation (a user-created macOS Keyboard Shortcut / Shortcuts.app
/// automation that runs Dictation, invoked here via `osascript`/`shortcuts
/// run`, or an NSApplication-level approach); that work needs a mac host to
/// verify and is out of scope for the Linux-only offline conversion.
#[cfg(target_os = "macos")]
mod macos_impl {
    use super::PcAsrActivator;

    pub struct DictationActivator;

    impl PcAsrActivator for DictationActivator {
        fn activate(&self) -> Result<(), String> {
            Err("macOS Dictation one-key activation not yet wired: its trigger \
                 is a fixed modifier double-press, not a bindable chord \
                 (phase-1 seam, see pc_asr.rs module doc)"
                .into())
        }

        fn is_available(&self) -> bool {
            false
        }
    }
}

// ─────────────────────────── Non-macOS ───────────────────────────

#[cfg(not(target_os = "macos"))]
mod stub {
    use super::PcAsrActivator;

    pub struct StubActivator;

    impl PcAsrActivator for StubActivator {
        fn activate(&self) -> Result<(), String> {
            eprintln!("pc_asr: OS dictation activation unsupported on this host (macOS-only)");
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

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stub_activator_unavailable_and_ok() {
        let a = activator();
        assert!(!a.is_available());
        assert!(a.activate().is_ok());
    }
}
