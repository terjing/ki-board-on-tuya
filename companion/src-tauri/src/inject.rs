//! Keyboard injection into the foreground application (Windows / macOS).
//!
//! On macOS the backend is a thin wrapper over `mac_sys` (CGEvent at the HID
//! tap); the Accessibility grant is checked before every call.
//!
//! Phase 1 uses Win32 `SendInput` (Unicode key events) to type transcribed text
//! and to press editing keys (Enter / Backspace). The real implementation is
//! gated behind `#[cfg(windows)]` and talks to `user32.dll` through raw FFI, so
//! no extra crate is pulled into the (Linux) CI build. On every other host a
//! stub implements the same trait, compiles cleanly, and logs that injection is
//! unsupported.

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
    #[cfg(windows)]
    {
        Box::new(windows_impl::Win32Injector)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(mac_impl::MacInjector)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Box::new(stub::StubInjector)
    }
}

// ───────────────────────────── Windows ─────────────────────────────

#[cfg(windows)]
mod windows_impl {
    use super::KeyInjector;
    use std::mem::size_of;

    // Minimal Win32 SendInput bindings (raw FFI, no `windows` crate needed).
    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const KEYEVENTF_UNICODE: u32 = 0x0004;
    const VK_RETURN: u16 = 0x0D;
    const VK_BACK: u16 = 0x08;

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
        // Pad the union to the size of the largest member (MOUSEINPUT is the
        // widest at 32 bytes on x64); KEYBDINPUT is 24 bytes, so add 8.
        _pad: [u8; 8],
    }

    #[link(name = "user32")]
    extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const INPUT, cb_size: i32) -> u32;
    }

    fn send(inputs: &[INPUT]) -> Result<(), String> {
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            )
        };
        if sent as usize == inputs.len() {
            Ok(())
        } else {
            Err(format!(
                "SendInput injected {sent}/{} events",
                inputs.len()
            ))
        }
    }

    fn unicode_event(unit: u16, key_up: bool) -> INPUT {
        let mut flags = KEYEVENTF_UNICODE;
        if key_up {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            ki: KEYBDINPUT {
                w_vk: 0,
                w_scan: unit,
                dw_flags: flags,
                time: 0,
                dw_extra_info: 0,
            },
            _pad: [0; 8],
        }
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

    pub struct Win32Injector;

    impl KeyInjector for Win32Injector {
        fn type_text(&self, text: &str) -> Result<(), String> {
            // Emit each UTF-16 code unit as a down/up Unicode key pair.
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

        fn press_enter(&self) -> Result<(), String> {
            send(&[vkey_event(VK_RETURN, false), vkey_event(VK_RETURN, true)])
        }

        fn press_backspace(&self) -> Result<(), String> {
            send(&[vkey_event(VK_BACK, false), vkey_event(VK_BACK, true)])
        }
    }
}

// ───────────────────────────── macOS ─────────────────────────────

#[cfg(target_os = "macos")]
mod mac_impl {
    use super::KeyInjector;
    use crate::{keymap::mac_keycode, mac_sys};

    pub struct MacInjector;

    fn ensure_granted() -> Result<(), String> {
        if mac_sys::input_permission_granted() {
            Ok(())
        } else {
            Err("Accessibility permission not granted — nothing injected".to_string())
        }
    }

    impl KeyInjector for MacInjector {
        fn type_text(&self, text: &str) -> Result<(), String> {
            ensure_granted()?;
            mac_sys::type_text(text)
        }

        fn press_enter(&self) -> Result<(), String> {
            ensure_granted()?;
            mac_sys::post_key(mac_keycode::RETURN)
        }

        fn press_backspace(&self) -> Result<(), String> {
            ensure_granted()?;
            mac_sys::post_key(mac_keycode::DELETE)
        }
    }
}

// ─────────────────────────── Other hosts ───────────────────────────

#[cfg(not(any(windows, target_os = "macos")))]
mod stub {
    use super::KeyInjector;

    pub struct StubInjector;

    impl KeyInjector for StubInjector {
        fn type_text(&self, _text: &str) -> Result<(), String> {
            eprintln!("inject: type_text unsupported on this host (Windows-only)");
            Ok(())
        }

        fn press_enter(&self) -> Result<(), String> {
            eprintln!("inject: press_enter unsupported on this host (Windows-only)");
            Ok(())
        }

        fn press_backspace(&self) -> Result<(), String> {
            eprintln!("inject: press_backspace unsupported on this host (Windows-only)");
            Ok(())
        }
    }
}

#[cfg(all(test, not(any(windows, target_os = "macos"))))]
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
