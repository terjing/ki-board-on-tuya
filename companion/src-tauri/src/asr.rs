//! System speech recognition (dictation) seam.
//!
//! Phase 1's "system" voice engine drives Windows' built-in dictation via
//! `Windows.Media.SpeechRecognition` (WinRT). That binding requires the
//! `windows` crate, which only resolves for the Windows target; to keep the
//! Linux CI build (and `Cargo.lock`) untouched, the Windows implementation here
//! is a compiling seam that returns a clear "not yet wired" error, and the full
//! WinRT session is a documented follow-up. On non-Windows hosts a stub
//! implements the same trait and logs that recognition is unsupported.
//!
//! macOS has no WinRT recognizer: it keeps using the stub below (the gate is
//! unchanged). macOS dictation is driven by `pc_asr::activator`, which presses
//! the Edit ▸ Start Dictation menu item or posts a double-tap modifier, and lets
//! the OS type the recognized text into the focused window.
//!
//! The trait is what the rest of the app depends on, so swapping in the real
//! WinRT recognizer later is a drop-in change with no call-site churn.

/// Outcome of a dictation request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecognitionResult {
    /// Final recognized text.
    Text(String),
    /// The user cancelled or nothing was recognized.
    Empty,
}

/// Drives a one-shot system dictation session.
pub trait SpeechRecognizer: Send {
    /// Begin listening and block until a final result (or error).
    fn recognize_once(&self) -> Result<RecognitionResult, String>;
    /// True when this backend can actually recognize on the current host.
    fn is_available(&self) -> bool;
}

/// Construct the platform speech recognizer.
pub fn recognizer() -> Box<dyn SpeechRecognizer> {
    #[cfg(windows)]
    {
        Box::new(windows_impl::WinRtRecognizer)
    }
    #[cfg(not(windows))]
    {
        Box::new(stub::StubRecognizer)
    }
}

// ───────────────────────────── Windows ─────────────────────────────

#[cfg(windows)]
mod windows_impl {
    use super::{RecognitionResult, SpeechRecognizer};

    /// Seam for `Windows.Media.SpeechRecognition`. The WinRT session
    /// (SpeechRecognizer + CompileConstraintsAsync + RecognizeAsync) is wired in
    /// a follow-up that adds the `windows` crate for the Windows target; until
    /// then this returns a descriptive error instead of silently succeeding.
    pub struct WinRtRecognizer;

    impl SpeechRecognizer for WinRtRecognizer {
        fn recognize_once(&self) -> Result<RecognitionResult, String> {
            Err("WinRT SpeechRecognition not yet wired (phase-1 seam)".into())
        }

        fn is_available(&self) -> bool {
            false
        }
    }
}

// ─────────────────────────── Non-Windows ───────────────────────────

#[cfg(not(windows))]
mod stub {
    use super::{RecognitionResult, SpeechRecognizer};

    pub struct StubRecognizer;

    impl SpeechRecognizer for StubRecognizer {
        fn recognize_once(&self) -> Result<RecognitionResult, String> {
            eprintln!("asr: system speech recognition unsupported on this host (Windows-only)");
            Ok(RecognitionResult::Empty)
        }

        fn is_available(&self) -> bool {
            false
        }
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn stub_recognizer_reports_unavailable_and_empty() {
        let rec = recognizer();
        assert!(!rec.is_available());
        assert_eq!(rec.recognize_once().unwrap(), RecognitionResult::Empty);
    }
}
