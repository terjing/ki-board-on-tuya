fn main() {
    // NB: `cfg!`/`#[cfg]` in a build script describe the *host* the script is
    // compiled for, not the cross target. Linux cross-checks therefore skip
    // `tauri_build`; Windows and macOS hosts run it.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    tauri_build::build()
}
