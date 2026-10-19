// Host-platform dependent UI labels (pure, no React / Tauri imports).
//
// The backend reports `std::env::consts::OS` in `get_status().platform`.
// Windows, Linux and any unknown value get the ORIGINAL literals verbatim, so
// the Windows UI is unchanged; only macOS swaps the shortcut tokens.

export type HostPlatform = "windows" | "macos" | "other";

/** Map the backend `platform` string to a [`HostPlatform`]. */
export function parsePlatform(s?: string): HostPlatform {
  switch ((s ?? "").trim().toLowerCase()) {
    case "windows":
      return "windows";
    case "macos":
      return "macos";
    default:
      return "other";
  }
}

export type UiLabels = {
  /** "Jump to chat N" shortcut, e.g. "Alt+3" / "⌃3". `"N"` gives the generic form. */
  jumpChat: (n: number | string) => string;
  /** The 1–9 range form, e.g. "Alt+1–9" / "⌃1–9". */
  jumpChatRange: string;
  /** Label of the Command-bar test button. */
  commandBar: string;
  /** Name of the OS dictation feature the K4 key activates. */
  dictation: string;
  /** Placeholder shown for the global hook command before the sink reports it. */
  hookPlaceholder: string;
  /** Name of the platform file manager. */
  fileManager: string;
};

/** UI labels for a platform. `windows`, `other` and unknown share the old literals. */
export function uiLabels(p: HostPlatform): UiLabels {
  if (p === "macos") {
    return {
      jumpChat: (n) => `\u2303${n}`,
      jumpChatRange: "\u23031\u20139",
      commandBar: "Command bar (\u2318K)",
      dictation: "macOS Dictation",
      hookPlaceholder:
        "'/Applications/Agentic Keyboard Companion.app/Contents/MacOS/<executable>' --hook",
      fileManager: "Finder",
    };
  }
  return {
    jumpChat: (n) => `Alt+${n}`,
    jumpChatRange: "Alt+1\u20139",
    commandBar: "Command bar (Ctrl+K)",
    dictation: "Windows dictation (Win+H)",
    hookPlaceholder: '"<path-to-companion.exe>" --hook',
    fileManager: "Explorer",
  };
}

/** Wire names accepted by the backend `set_mac_dictation_trigger` command. */
export const MAC_DICTATION_TRIGGERS: { value: string; label: string }[] = [
  { value: "edit-menu", label: "Edit menu (default)" },
  { value: "double-control", label: "Double Control" },
  { value: "double-right-command", label: "Double Right Command" },
  { value: "double-left-command", label: "Double Left Command" },
];
