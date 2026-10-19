//! Keyboard injection into the foreground macOS application.
//!
//! Phase 1 synthesizes keystrokes with CoreGraphics event taps (the
//! `CGEvent*` family in the ApplicationServices umbrella framework) to type
//! transcribed text and press editing keys (Enter / Backspace). The real
//! implementation is gated behind `#[cfg(target_os = "macos")]` and talks to
//! the `ApplicationServices` framework through raw FFI, so no extra crate is
//! pulled into the (Linux) CI build. On every other host a stub implements the
//! same trait, compiles cleanly, and logs that injection is unsupported.
//!
//! Runtime note: posting synthetic events on macOS requires the app to be
//! granted **Accessibility** permission (System Settings > Privacy & Security
//! > Accessibility). Without it `CGEventPost` is silently dropped by the OS.

/// Injects keystrokes into the active application. Kept as a trait so a test
/// double or an alternate backend (e.g. clipboard-paste) can slot in later.
pub trait KeyInjector: Send {
    /// Type a UTF-8 string as Unicode key events.
    fn type_text(&self, text: &str) -> Result<(), String>;
    /// Press Enter (commit / send).
    fn press_enter(&self) -> Result<(), String>;
    /// Press Backspace (delete previous character).
    fn press_backspace(&self) -> Result<(), String>;
}

/// Construct the platform key injector.
pub fn injector() -> Box<dyn KeyInjector> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos_impl::CoreGraphicsInjector)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(stub::StubInjector)
    }
}

// ────────────────────────────── macOS ──────────────────────────────

/// macOS key injection via CoreGraphics event posting.
///
/// Each keystroke is a `CGEventCreateKeyboardEvent` (keydown then keyup) posted
/// to the HID event tap with `CGEventPost`. Text is typed by attaching the
/// UTF-16 units to a keycode-0 event with `CGEventKeyboardSetUnicodeString`.
/// Every FFI call is `unsafe`; the hand-rolled pointer types stay local to this
/// module (mirroring the raw `user32` FFI pattern used previously). Requires
/// macOS **Accessibility** permission at runtime.
#[cfg(target_os = "macos")]
mod macos_impl {
    use super::KeyInjector;
    use std::os::raw::c_void;

    // `kCGHIDEventTap == 0`: post into the HID stream so the frontmost app sees
    // the synthetic events as real hardware input.
    const KCG_HID_EVENT_TAP: u32 = 0;

    // Virtual keycodes (ANSI, from <HIToolbox/Events.h>).
    const VK_RETURN: u16 = 0x24; // kVK_Return (36)
    const VK_DELETE: u16 = 0x33; // kVK_Delete / Backspace (51)

    // Raw FFI into the ApplicationServices umbrella framework (CoreGraphics +
    // CoreFoundation live under it), mirroring the former raw `user32` FFI — no
    // `core-graphics`/`core-foundation` crate is added. The source/event
    // pointers are opaque CF types, so they are modeled as `*const c_void`.
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
        fn CGEventPost(tap: u32, event: *const c_void);
        fn CFRelease(cf: *const c_void);
    }

    pub struct CoreGraphicsInjector;

    impl CoreGraphicsInjector {
        /// Post a single keycode as a down/up pair. Returns Err on a null event
        /// pointer (event creation failed).
        fn tap_vkey(vk: u16) -> Result<(), String> {
            Self::post_event(vk, true)?;
            Self::post_event(vk, false)
        }

        /// Create, post, and release one keyboard event (down or up).
        fn post_event(vk: u16, keydown: bool) -> Result<(), String> {
            unsafe {
                let event =
                    CGEventCreateKeyboardEvent(std::ptr::null(), vk, keydown);
                if event.is_null() {
                    return Err("CGEventCreateKeyboardEvent returned null".to_string());
                }
                CGEventPost(KCG_HID_EVENT_TAP, event);
                CFRelease(event);
            }
            Ok(())
        }
    }

    impl KeyInjector for CoreGraphicsInjector {
        fn type_text(&self, text: &str) -> Result<(), String> {
            if text.is_empty() {
                return Ok(());
            }
            // Attach the whole string's UTF-16 units to a keycode-0 event. The
            // keyup event carries the same string so the sequence is balanced.
            let units: Vec<u16> = text.encode_utf16().collect();
            unsafe {
                for &keydown in &[true, false] {
                    let event =
                        CGEventCreateKeyboardEvent(std::ptr::null(), 0, keydown);
                    if event.is_null() {
                        return Err(
                            "CGEventCreateKeyboardEvent returned null".to_string()
                        );
                    }
                    CGEventKeyboardSetUnicodeString(
                        event,
                        units.len(),
                        units.as_ptr(),
                    );
                    CGEventPost(KCG_HID_EVENT_TAP, event);
                    CFRelease(event);
                }
            }
            Ok(())
        }

        fn press_enter(&self) -> Result<(), String> {
            Self::tap_vkey(VK_RETURN)
        }

        fn press_backspace(&self) -> Result<(), String> {
            Self::tap_vkey(VK_DELETE)
        }
    }
}

// ─────────────────────────── Non-macOS ───────────────────────────

#[cfg(not(target_os = "macos"))]
mod stub {
    use super::KeyInjector;

    pub struct StubInjector;

    impl KeyInjector for StubInjector {
        fn type_text(&self, _text: &str) -> Result<(), String> {
            eprintln!("inject: type_text unsupported on this host (macOS-only)");
            Ok(())
        }

        fn press_enter(&self) -> Result<(), String> {
            eprintln!("inject: press_enter unsupported on this host (macOS-only)");
            Ok(())
        }

        fn press_backspace(&self) -> Result<(), String> {
            eprintln!("inject: press_backspace unsupported on this host (macOS-only)");
            Ok(())
        }
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn stub_injector_is_constructible_and_noop_ok() {
        let inj = injector();
        assert!(inj.type_text("hello").is_ok());
        assert!(inj.press_enter().is_ok());
        assert!(inj.press_backspace().is_ok());
    }
}
