//! Host-OS helpers shared by the UI-facing code (labels, file manager, URLs).
//!
//! Everything here is pure except [`open_url`] and [`input_permission_status`],
//! so the Windows strings and commands are pinned by tests on every host.

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
/// Windows: `explorer.exe <dir>`; macOS: `open <dir>`; elsewhere `None`.
pub fn reveal_dir_command(os: HostOs, dir: &Path) -> Option<(&'static str, Vec<OsString>)> {
    match os {
        HostOs::Windows => Some(("explorer.exe", vec![dir.as_os_str().to_os_string()])),
        HostOs::Macos => Some(("open", vec![dir.as_os_str().to_os_string()])),
        HostOs::Other => None,
    }
}

/// System Settings deep link for the Accessibility privacy pane (macOS only).
pub fn accessibility_settings_url(os: HostOs) -> Option<&'static str> {
    match os {
        HostOs::Macos => Some(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        ),
        HostOs::Windows | HostOs::Other => None,
    }
}

/// Open a URL with the host handler (`open` on macOS; unsupported elsewhere).
pub fn open_url(url: &str) -> Result<(), String> {
    if HostOs::current() != HostOs::Macos {
        return Err("open_url is only supported on macOS".to_string());
    }
    let status = std::process::Command::new("open")
        .arg(url)
        .status()
        .map_err(|e| format!("open {url}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("open {url}: exited with {status}"))
    }
}

/// `"granted"`, `"missing"` or `"not-required"`.
pub fn input_permission_status() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        if crate::mac_sys::input_permission_granted() {
            "granted"
        } else {
            "missing"
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        "not-required"
    }
}

pub fn file_manager_name(os: HostOs) -> &'static str {
    match os {
        HostOs::Windows => "Explorer",
        HostOs::Macos => "Finder",
        HostOs::Other => "file manager",
    }
}

/// Label of the "jump to chat N" shortcut: "Alt+3" on Windows/other, "⌃3" on macOS.
pub fn jump_chat_label(os: HostOs, n: u8) -> String {
    match os {
        HostOs::Macos => format!("\u{2303}{n}"),
        HostOs::Windows | HostOs::Other => format!("Alt+{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn reveal_dir_windows_is_explorer_with_one_arg() {
        let dir = PathBuf::from("C:\\Users\\a b\\.kiro\\crew");
        let (prog, args) = reveal_dir_command(HostOs::Windows, &dir).unwrap();
        assert_eq!(prog, "explorer.exe");
        assert_eq!(args, vec![OsString::from(dir.as_os_str())]);
    }

    #[test]
    fn reveal_dir_macos_keeps_spaces_in_one_arg() {
        let dir = PathBuf::from("/Users/a b/.kiro/crew");
        let (prog, args) = reveal_dir_command(HostOs::Macos, &dir).unwrap();
        assert_eq!(prog, "open");
        assert_eq!(args, vec![OsString::from("/Users/a b/.kiro/crew")]);
    }

    #[test]
    fn reveal_dir_other_is_none() {
        assert!(reveal_dir_command(HostOs::Other, Path::new("/x")).is_none());
    }

    #[test]
    fn accessibility_url_is_macos_only() {
        assert_eq!(
            accessibility_settings_url(HostOs::Macos),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        );
        assert_eq!(accessibility_settings_url(HostOs::Windows), None);
        assert_eq!(accessibility_settings_url(HostOs::Other), None);
    }

    #[test]
    fn input_permission_status_not_required_off_macos() {
        if !cfg!(target_os = "macos") {
            assert_eq!(input_permission_status(), "not-required");
        }
    }

    #[test]
    fn open_url_errors_off_macos() {
        if !cfg!(target_os = "macos") {
            assert!(open_url("https://example.com").is_err());
        }
    }

    #[test]
    fn file_manager_names() {
        assert_eq!(file_manager_name(HostOs::Windows), "Explorer");
        assert_eq!(file_manager_name(HostOs::Macos), "Finder");
        assert_eq!(file_manager_name(HostOs::Other), "file manager");
    }

    #[test]
    fn jump_chat_labels() {
        assert_eq!(jump_chat_label(HostOs::Windows, 3), "Alt+3");
        assert_eq!(jump_chat_label(HostOs::Other, 9), "Alt+9");
        assert_eq!(jump_chat_label(HostOs::Macos, 3), "\u{2303}3");
        assert_eq!(jump_chat_label(HostOs::Macos, 3), "⌃3");
    }
}
