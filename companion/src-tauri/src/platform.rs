//! Host-OS helpers shared by the UI-facing code (labels, file manager, URLs).
//!
//! U1 scaffold: signatures are final; `todo!("U4")` bodies are replaced by
//! unit U4. `HostOs::current` is real.

#![allow(unused_variables)]

use std::ffi::OsString;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostOs {
    Windows,
    Macos,
    Other,
}

impl HostOs {
    pub fn current() -> HostOs {
        if cfg!(target_os = "windows") {
            HostOs::Windows
        } else if cfg!(target_os = "macos") {
            HostOs::Macos
        } else {
            HostOs::Other
        }
    }
}

/// Program + args that reveal `dir` in the host file manager.
pub fn reveal_dir_command(os: HostOs, dir: &Path) -> Option<(&'static str, Vec<OsString>)> {
    todo!("U4")
}

/// System Settings deep link for the Accessibility privacy pane.
pub fn accessibility_settings_url(os: HostOs) -> Option<&'static str> {
    todo!("U4")
}

/// Open a URL with the host handler.
pub fn open_url(url: &str) -> Result<(), String> {
    todo!("U4")
}

/// `"granted"`, `"missing"` or `"not-required"`.
pub fn input_permission_status() -> &'static str {
    todo!("U4")
}

pub fn file_manager_name(os: HostOs) -> &'static str {
    todo!("U4")
}

/// "Alt+3" / "⌃3".
pub fn jump_chat_label(os: HostOs, n: u8) -> String {
    todo!("U4")
}
