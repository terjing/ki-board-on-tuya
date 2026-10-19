//! System speech recognition (dictation) seam.
//!
//! Phase 1's "system" voice engine drives macOS' built-in speech recognition
//! via Apple's **Speech** framework (`SFSpeechRecognizer`). That binding needs
//! Objective-C/Swift bridging, which only resolves for the macOS target and
//! cannot compile on the Linux CI host; to keep the Linux build (and
//! `Cargo.lock`) untouched, the macOS implementation here is a compiling seam
//! that returns a clear "not yet wired" error, and the full `SFSpeechRecognizer`
//! session is a documented follow-up. On non-macOS hosts a stub implements the
//! same trait and logs that recognition is unsupported.
//!
//! The trait is what the rest of the app depends on, so swapping in the real
//! Apple Speech recognizer later is a drop-in change with no call-site churn.

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
    #[cfg(target_os = "macos")]
    {
        Box::new(macos_impl::AppleSpeechRecognizer)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(stub::StubRecognizer)
    }
}

// ────────────────────────────── macOS ──────────────────────────────

#[cfg(target_os = "macos")]
mod macos_impl {
    use super::{RecognitionResult, SpeechRecognizer};

    /// Seam for Apple's Speech framework (`SFSpeechRecognizer`). The real
    /// session (SFSpeechRecognizer + SFSpeechAudioBufferRecognitionRequest +
    /// an AVAudioEngine tap) is wired in a follow-up that adds the required
    /// Objective-C/Swift bridging for the macOS target; it cannot compile on
    /// the Linux CI host, so until then this returns a descriptive error
    /// instead of silently succeeding.
    pub struct AppleSpeechRecognizer;

    impl SpeechRecognizer for AppleSpeechRecognizer {
        fn recognize_once(&self) -> Result<RecognitionResult, String> {
            Err("Apple Speech (SFSpeechRecognizer) not yet wired (phase-1 seam)".into())
        }

        fn is_available(&self) -> bool {
            false
        }
    }
}

// ─────────────────────────── Non-macOS ───────────────────────────

#[cfg(not(target_os = "macos"))]
mod stub {
    use super::{RecognitionResult, SpeechRecognizer};

    pub struct StubRecognizer;

    impl SpeechRecognizer for StubRecognizer {
        fn recognize_once(&self) -> Result<RecognitionResult, String> {
            eprintln!("asr: system speech recognition unsupported on this host (macOS-only)");
            Ok(RecognitionResult::Empty)
        }

        fn is_available(&self) -> bool {
            false
        }
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn stub_recognizer_reports_unavailable_and_empty() {
        let rec = recognizer();
        assert!(!rec.is_available());
        assert_eq!(rec.recognize_once().unwrap(), RecognitionResult::Empty);
    }
}
