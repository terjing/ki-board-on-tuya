// Prevents an extra console window on Windows in release builds. The companion
// is now macOS-native, but keep this attribute harmless on any Windows build by
// gating it to the Windows target only (it is a no-op elsewhere).
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use agentic_keyboard_companion_lib::{diagnostics, status_sink};

fn main() {
    // FP-4b Decision 2: the Kiro agent hook is a MODE of this same binary,
    // `companion --hook`, not a separate shell script. When invoked that way we
    // read the hook JSON from STDIN, forward it to the running status sink, and
    // exit(0) fast WITHOUT starting the GUI. Cross-platform (reqwest blocking),
    // so it works wherever Kiro Crew runs the hook. Absent sink → silent exit 0.
    if std::env::args().skip(1).any(|a| a == "--hook") {
        status_sink::run_hook_mode();
        std::process::exit(0);
    }

    let _diagnostic_log = diagnostics::init();
    agentic_keyboard_companion_lib::run();
}
