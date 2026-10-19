import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create as createQrCode } from "qrcode";
import "./styles.css";
import { MAC_DICTATION_TRIGGERS, parsePlatform, uiLabels } from "./platform";

// ─── Types mirroring the Rust backend (src-tauri/src/lib.rs) ────────────────

type Transport = "none" | "usb" | "ble";

type Status = {
  device_connected: boolean;
  transport: Transport;
  endpoint: string;
  firmware_version: string;
  last_error: string;
  control_enabled?: boolean;
  device_volume: number;
  device_brightness: number;
  device_settings_synced: boolean;
  crew_api_connected?: boolean;
  crew_api_base_url?: string;
  crew_api_last_error?: string;
  crew_api_slots?: number;
  /** Host OS from the backend (`std::env::consts::OS`): "windows", "macos", … */
  platform?: string;
  /** macOS Accessibility grant: "granted" | "missing" | "not-required". */
  input_permission?: string;
  /** Canonical macOS dictation trigger wire name. */
  mac_dictation_trigger?: string;
};

type CrewSlot = {
  key: string;
  title: string;
  agent: string;
  running: boolean;
  stopping: boolean;
  pending_approval: boolean;
  waiting_for_input: boolean;
  needs_input: boolean;
  subagents_running: boolean;
  last_message: string;
};

type CrewConnection = {
  connected: boolean;
  base_url: string;
  error: string;
  retry_secs: number;
};

type DeviceSettings = {
  volume: number;
  brightness: number;
};

type LicenseInfo = {
  source: string;
  uuid: string;
  authkey_tail: string;
};

type ActivationStatus = {
  state: string;
  qr_url: string;
};

const ACTIVATION_STEPS = ["Router", "Scan", "Confirm", "Ready"] as const;

function activationStepIndex(state: string) {
  switch (state) {
    case "connecting_router": return 0;
    case "router_failed": return 0;
    case "wifi_connected": return 1;
    case "scan_qr": return 1;
    case "token_received": return 2;
    case "activated": return 2;
    case "cloud_ready": return 3;
    case "cloud_disconnected": return 2;
    default: return -1;
  }
}

function activationLabel(state: string) {
  switch (state) {
    case "connecting_router": return "Connecting router";
    case "router_failed": return "WiFi connection failed";
    case "wifi_connected": return "WiFi connected · preparing QR";
    case "pairing": return "Configure WiFi to start";
    case "scan_qr": return "Scan with SmartLife";
    case "token_received": return "SmartLife confirmed";
    case "activated": return "Registration successful";
    case "cloud_ready": return "Setup complete · reboot ready";
    case "cloud_disconnected": return "Setup saved · Cloud offline";
    case "resetting": return "Resetting";
    case "disabled": return "Cloud setup unavailable";
    default: return "Configure WiFi to start";
  }
}

type QrDrawing = { size: number; path: string };

function buildQrDrawing(url: string): QrDrawing | null {
  if (!url.startsWith("https://smartapp.tuya.com/s/p?")) return null;
  const qr = createQrCode(url, { errorCorrectionLevel: "M" });
  const size = qr.modules.size;
  let path = "";
  for (let row = 0; row < size; row += 1) {
    for (let col = 0; col < size; col += 1) {
      if (qr.modules.get(row, col)) path += `M${col} ${row}h1v1h-1z`;
    }
  }
  return { size, path };
}

type SerialPortInfo = {
  name: string;
  kind: string;
};

type BleDeviceInfo = {
  id: string;
  name: string;
};

// Flat board-event object emitted by `inbound_to_json` in the backend.
type BoardEvent = {
  kind: "key" | "nav" | "voice" | "macro" | "activation_status" | "license_info" | "device_settings" | "system_info" | "hello_ack" | "ping" | "pong" | "unknown";
  seq?: number | null;
  id?: string;
  dir?: string;
  action?: string;
  held_ms?: number;
  state?: string;
  name?: string;
  volume?: number;
  brightness?: number;
  fw?: string;
  transport?: string;
  wifi_name?: string;
  wifi_state?: string;
  ble_name?: string;
  ble_state?: string;
  cloud?: string;
  license?: string;
  source?: string;
  uuid?: string;
  authkey_tail?: string;
  qr_url?: string;
  type?: string;
};

// FP-4b STATUS: a mapped agent status pushed from the hook sink via `kc-status`.
type KcStatus = {
  session_id: string;
  agent_name: string;
  state: string;
  text: string;
  event?: string;
  detail_text?: string;
  // Set by the sink: true when this session is the one currently driving the
  // board LCD (either nothing is pinned, or this is the pinned session).
  forwarded?: boolean;
};

// FP-4b: one-click user-level global Crew hook install status.
type HookStatus = {
  command: string;
  hooks_dir: string;
  hook_file: string;
  installed_events: number;
  fully_installed: boolean;
  any_installed: boolean;
};

// FP-4b M4 (§3.2): each A/B/C slot has a chat shortcut number and an optional
// AK display alias. Empty aliases fall back to A, B, and C on the board.
type ChatNumbers = number[];
type ChatAliases = string[];
type ChatSessions = string[];
type WatchedView = "compact" | "detail";
type QuickBindDraft = { sessionId: string; slot: number; number: number };

const SLOT_DEFAULT_NAMES = ["A", "B", "C"] as const;
const ALIAS_MAX_CHARS = 7;

function sessionDisplayName(sessionId: string) {
  const firstPart = sessionId.split(":", 1)[0]?.trim();
  return firstPart || "Kiro Crew";
}

const defaultStatus: Status = {
  device_connected: false,
  transport: "none",
  endpoint: "",
  firmware_version: "",
  last_error: "",
  control_enabled: true,
  device_volume: 70,
  device_brightness: 100,
  device_settings_synced: false,
  crew_api_connected: false,
  crew_api_base_url: "http://127.0.0.1:5476",
  crew_api_last_error: "",
  crew_api_slots: 0,
};

// Our 5 mechanical keys (k1..k5) with their phase-1 default intents. This is a
// reference mapping placeholder — remapping UI lands in a later pass. `wide`
// marks K4 as the double-width push-to-talk key in the INPUT layout.
const KEY_MAP: { id: string; label: string; intent: string; wide?: boolean }[] = [
  { id: "k1", label: "K1", intent: "Chat A" },
  { id: "k2", label: "K2", intent: "Chat B" },
  { id: "k3", label: "K3", intent: "Chat C" },
  { id: "k4", label: "K4", intent: "Push-to-Talk (voice)", wide: true },
  { id: "k5", label: "K5", intent: "Submit / Continue" },
];

// 3×3 grid positions for the compact 5-way navigation cluster.
const NAV_LAYOUT: (string | null)[] = [
  null, "up", null,
  "left", "center", "right",
  null, "down", null,
];

function App() {
  const [status, setStatus] = useState<Status>(defaultStatus);
  // Shortcut labels follow the host OS reported by the backend; Windows (and
  // the pre-first-poll default) keep the original literals.
  const platform = parsePlatform(status.platform);
  const labels = uiLabels(platform);
  const isMac = platform === "macos";
  const [ports, setPorts] = useState<SerialPortInfo[]>([]);
  const [selectedPort, setSelectedPort] = useState("");
  const [transportMode, setTransportMode] = useState<"usb" | "ble">("usb");
  const [bleDevices, setBleDevices] = useState<BleDeviceInfo[]>([]);
  const [selectedBle, setSelectedBle] = useState("");
  const [bleScanning, setBleScanning] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  // Raw board-frame JSON stream, shown when the Logs "raw" toggle is on. The
  // `logs` array is the human-readable/detailed view ([RX] key k4 …); `logsRaw`
  // is the untouched JSON object per board-event.
  const [logsRaw, setLogsRaw] = useState<string[]>([]);
  const [logsShowRaw, setLogsShowRaw] = useState(false);
  // LOGS pause/resume: while paused the visible list is FROZEN (so the user can
  // observe and copy without it scrolling); new lines are buffered in refs and
  // flushed back on resume. A ref mirrors the flag so the once-registered event
  // listeners read the live value instead of a stale closure.
  const [logsPaused, setLogsPaused] = useState(false);
  const logsPausedRef = useRef(false);
  const logBufRef = useRef<string[]>([]);
  const rawBufRef = useRef<string[]>([]);
  const [pausedCount, setPausedCount] = useState(0);
  const [voiceSource, setVoiceSource] = useState<"cloud" | "system">("cloud");
  // WiFi provisioning (DEVICE panel): send SSID/pass to the board over the link.
  const [wifiSsid, setWifiSsid] = useState("");
  const [wifiPass, setWifiPass] = useState("");
  const [showWifiPass, setShowWifiPass] = useState(false);
  const [wifiStatus, setWifiStatus] = useState("");
  const [wifiBusy, setWifiBusy] = useState(false);
  const [activationStatus, setActivationStatus] = useState<ActivationStatus>({
    state: "unknown",
    qr_url: "",
  });
  const [activationExpanded, setActivationExpanded] = useState(false);
  // Device license (UUID/AuthKey) provisioning.
  const [licUuid, setLicUuid] = useState("");
  const [licAuthkey, setLicAuthkey] = useState("");
  const [licStatus, setLicStatus] = useState("");
  const [licBusy, setLicBusy] = useState(false);
  const [licInfo, setLicInfo] = useState<LicenseInfo | null>(null);
  const [pairingBusy, setPairingBusy] = useState(false);
  const [pairingStatus, setPairingStatus] = useState("");
  const [deviceSettings, setDeviceSettings] = useState<DeviceSettings>({
    volume: defaultStatus.device_volume,
    brightness: defaultStatus.device_brightness,
  });
  const [deviceSettingsBusy, setDeviceSettingsBusy] = useState(false);
  const [lastKey, setLastKey] = useState("");
  const [lastNav, setLastNav] = useState("");
  const [voiceActive, setVoiceActive] = useState(false);
  const [activeView, setActiveView] = useState<"device" | "input" | "kirocrew" | "logs">("device");
  const [uiError, setUiError] = useState("");
  const statusPollInFlightRef = useRef(false);
  const usbProvisioningAvailable = status.device_connected && status.transport === "usb";
  const activationQrDrawing = useMemo(
    () => activationStatus.state === "scan_qr"
      ? buildQrDrawing(activationStatus.qr_url)
      : null,
    [activationStatus.state, activationStatus.qr_url],
  );
  const activationQrError = activationStatus.state === "scan_qr" &&
    activationStatus.qr_url !== "" && activationQrDrawing === null
    ? "The keyboard returned an unexpected activation URL."
    : "";

  // FP-4b route B ("Local Control", the hands) state: focus a Kiro Crew window
  // and inject keystrokes.
  const [lcTitle, setLcTitle] = useState("");
  const [lcBound, setLcBound] = useState(false);
  const [lcText, setLcText] = useState("");
  const [lcResult, setLcResult] = useState("");
  const [lcError, setLcError] = useState("");
  const [lcBusy, setLcBusy] = useState(false);

  // Primary local API control + WebSocket status path. The legacy Hook listener
  // and SendInput controls below remain available as fallbacks.
  const [crewSlots, setCrewSlots] = useState<CrewSlot[]>([]);
  const [crewSelectedSlot, setCrewSelectedSlot] = useState("");
  const [crewMessage, setCrewMessage] = useState("");
  const [crewApiBusy, setCrewApiBusy] = useState(false);
  const [crewApiError, setCrewApiError] = useState("");

  // FP-4b M4 (§3.2): A/B/C chat numbers and optional AK display aliases.
  // Aliases save on blur/Enter rather than on every keystroke.
  const [binds, setBinds] = useState<ChatNumbers>([]);
  const [bindAliases, setBindAliases] = useState<ChatAliases>(["", "", ""]);
  const [bindSessions, setBindSessions] = useState<ChatSessions>(["", "", ""]);
  const [bindError, setBindError] = useState("");
  const [bindBusy, setBindBusy] = useState(false);

  // FP-4b STATUS sink ("the eyes"): a localhost HTTP ingress for Kiro agent
  // hooks (delivered by `companion --hook`). Shows the token + hook command to
  // paste, and a live per-session status list from the `kc-status` UI event.
  const [ssRunning, setSsRunning] = useState(false);
  const [ssToken, setSsToken] = useState("");
  const [ssError, setSsError] = useState("");
  const [ssBusy, setSsBusy] = useState(false);
  const [kcStatuses, setKcStatuses] = useState<Record<string, KcStatus>>({});
  const [watchedView, setWatchedView] = useState<WatchedView>("compact");
  const [quickBindDraft, setQuickBindDraft] = useState<QuickBindDraft | null>(null);
  // Mode 2 (long-press bind): the session the user picked in the Watched Chat
  // list as the target for the next physical K1/K2/K3 long-press. Long-pressing
  // a key binds this session to that key's slot (K1->A, K2->B, K3->C), writing
  // through the SAME path as the dropdown bind so the settings UI stays in sync.
  const [selectedSession, setSelectedSession] = useState("");
  // Transient banner shown after a long-press bind completes (also mirrored to
  // the keyboard screen via a display frame).
  const [longBindNotice, setLongBindNotice] = useState("");
  // FP-4b Route STATUS (option A): the pinned "watched chat". "" = follow all
  // sessions. Non-empty = only that session's status drives the board LCD.
  const [pinnedSession, setPinnedSession] = useState("");
  // FP-4b: Crew ScriptHookStore hooks apply globally to dashboard agent turns.
  const [hookStatus, setHookStatus] = useState<HookStatus | null>(null);

  // The `board-event` listener is registered once, so it reads live values
  // through refs instead of its (stale) initial closure. These mirror the state
  // the long-press auto-bind path needs: the picked session, the API chat list,
  // and the current bind numbers/aliases.
  const selectedSessionRef = useRef("");
  const crewSlotsRef = useRef<CrewSlot[]>([]);
  const bindsRef = useRef<ChatNumbers>([]);
  const bindAliasesRef = useRef<ChatAliases>(["", "", ""]);
  useEffect(() => {
    selectedSessionRef.current = selectedSession;
  }, [selectedSession]);
  useEffect(() => {
    crewSlotsRef.current = crewSlots;
  }, [crewSlots]);
  useEffect(() => {
    bindsRef.current = binds;
  }, [binds]);
  useEffect(() => {
    bindAliasesRef.current = bindAliases;
  }, [bindAliases]);

  const boardPorts = useMemo(
    () => ports.filter((port) => port.kind !== "flash"),
    [ports],
  );
  const sortedKcStatuses = useMemo(
    () =>
      Object.values(kcStatuses).sort((a, b) =>
        a.session_id === pinnedSession
          ? -1
          : b.session_id === pinnedSession
            ? 1
            : a.session_id.localeCompare(b.session_id),
      ),
    [kcStatuses, pinnedSession],
  );

  async function refreshRuntime() {
    const runtime = await invoke<Status>("get_status");
    setStatus(runtime);
    setDeviceSettings({
      volume: runtime.device_volume,
      brightness: runtime.device_brightness,
    });
    setPorts(await invoke<SerialPortInfo[]>("list_serial_ports"));
    // Don't clobber the frozen list while the LOGS view is paused.
    if (!logsPausedRef.current) {
      setLogs(await invoke<string[]>("get_logs"));
    }
  }

  async function runAction(action: () => Promise<void>) {
    try {
      setUiError("");
      await action();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setUiError(message);
    }
  }

  async function connect() {
    setLicInfo(null);
    if (transportMode === "ble") {
      if (!selectedBle) {
        throw new Error("Scan and select an AK-KB device first.");
      }
      await invoke("connect_device", { transport: "ble", portName: selectedBle });
    } else {
      await invoke("connect_device", { transport: "usb", portName: selectedPort || null });
    }
    await refreshRuntime();
  }

  async function scanBle() {
    setBleScanning(true);
    try {
      const found = await invoke<BleDeviceInfo[]>("scan_ble");
      setBleDevices(found);
      // Auto-select the first match so Connect is one click after a scan.
      if (found.length > 0 && !found.some((d) => d.id === selectedBle)) {
        setSelectedBle(found[0].id);
      }
    } finally {
      setBleScanning(false);
    }
  }

  async function disconnect() {
    await invoke("disconnect_device");
    setLicInfo(null);
    await refreshRuntime();
  }

  async function adjustDeviceSetting(key: keyof DeviceSettings, delta: number) {
    const minimum = key === "brightness" ? 10 : 0;
    const current = deviceSettings[key];
    const nextValue = Math.max(minimum, Math.min(100, current + delta));
    if (nextValue === current) return;

    const next = { ...deviceSettings, [key]: nextValue };
    setDeviceSettingsBusy(true);
    try {
      await invoke("set_device_settings", next);
    } finally {
      setDeviceSettingsBusy(false);
    }
  }

  async function clearLogs() {
    await invoke("clear_logs");
    setLogs([]);
    setLogsRaw([]);
    logBufRef.current = [];
    rawBufRef.current = [];
    setPausedCount(0);
  }

  // Toggle LOGS pause. On resume, flush the buffered lines (newest-first) back
  // onto the head of each list so nothing observed while paused is lost.
  function toggleLogsPause() {
    if (logsPausedRef.current) {
      logsPausedRef.current = false;
      setLogsPaused(false);
      if (logBufRef.current.length) {
        const buf = logBufRef.current;
        setLogs((items) => [...buf, ...items].slice(0, 200));
        logBufRef.current = [];
      }
      if (rawBufRef.current.length) {
        const buf = rawBufRef.current;
        setLogsRaw((items) => [...buf, ...items].slice(0, 200));
        rawBufRef.current = [];
      }
      setPausedCount(0);
    } else {
      logsPausedRef.current = true;
      setLogsPaused(true);
    }
  }

  // Enable/disable routing board keys to Kiro Crew control. Default ON so the
  // Companion is ready to drive Kiro Crew immediately; disabling it still
  // gates every board-triggered PC action, including K4 / Win+H.
  async function toggleControl(enabled: boolean) {
    await invoke("set_control_enabled", { enabled });
    setStatus((s) => ({ ...s, control_enabled: enabled }));
  }

  // macOS: show the system Accessibility prompt + open the Privacy pane.
  async function grantAccessibility() {
    const input_permission = await invoke<string>("request_input_permission");
    setStatus((s) => ({ ...s, input_permission }));
  }

  // macOS: choose how K4 starts system dictation (persisted by the backend).
  async function selectDictationTrigger(trigger: string) {
    const mac_dictation_trigger = await invoke<string>("set_mac_dictation_trigger", { trigger });
    setStatus((s) => ({ ...s, mac_dictation_trigger }));
  }

  async function selectVoiceSource(source: "cloud" | "system") {
    setVoiceSource(source);
    // set_voice_source updates companion state AND mirrors the choice to the
    // board via a config frame. Ignore the "not connected" error when offline.
    await invoke("set_voice_source", { source }).catch(() => undefined);
  }

  async function sendWifi() {
    if (!usbProvisioningAvailable) {
      setUiError("Connect the board over USB before configuring WiFi.");
      return;
    }
    if (!wifiSsid.trim()) {
      setUiError("Enter a WiFi SSID first.");
      return;
    }
    setWifiBusy(true);
    setActivationStatus({ state: "connecting_router", qr_url: "" });
    setActivationExpanded(false);
    try {
      setUiError("");
      setWifiStatus("sending…");
      await invoke("send_wifi", { ssid: wifiSsid.trim(), pass: wifiPass });
      setWifiStatus("sent, waiting for board…");
    } catch (error) {
      setWifiStatus("");
      setUiError(error instanceof Error ? error.message : String(error));
    } finally {
      setWifiBusy(false);
    }
  }

  async function sendLicense() {
    if (!usbProvisioningAvailable) {
      setUiError("Connect the board over USB before writing UUID/AuthKey.");
      return;
    }
    if (!licUuid.trim() || !licAuthkey.trim()) {
      setUiError("UUID and AuthKey are both required.");
      return;
    }
    setLicBusy(true);
    try {
      setUiError("");
      setLicStatus("writing…");
      await invoke("send_license", { uuid: licUuid.trim(), authkey: licAuthkey.trim() });
      setLicStatus("sent, waiting for board…");
    } catch (error) {
      setLicStatus("");
      setUiError(error instanceof Error ? error.message : String(error));
    } finally {
      setLicBusy(false);
    }
  }

  async function readLicense() {
    if (!usbProvisioningAvailable) {
      setUiError("Connect the board over USB before reading License diagnostics.");
      return;
    }
    setLicBusy(true);
    try {
      setUiError("");
      setLicStatus("reading…");
      setLicInfo(null);
      await invoke("read_license");
      setLicStatus("read request sent…");
    } catch (error) {
      setLicStatus("");
      setUiError(error instanceof Error ? error.message : String(error));
    } finally {
      setLicBusy(false);
    }
  }

  // One-click "enter pairing": clears the board's Wi-Fi + cloud binding and
  // reboots it into provisioning. The License (UUID/AuthKey) is preserved.
  // Destructive + reboots the board, so require an explicit confirmation and
  // gate on USB (matches wifi/license).
  async function enterPairing() {
    if (!usbProvisioningAvailable) {
      setUiError("Connect the board over USB before entering pairing mode.");
      return;
    }
    const ok = window.confirm(
      "Enter pairing mode?\n\nThis clears the keyboard's saved Wi-Fi and cloud " +
        "binding and reboots it into provisioning. The device License " +
        "(UUID/AuthKey) is kept. You will need to re-provision Wi-Fi afterwards.",
    );
    if (!ok) {
      return;
    }
    setPairingBusy(true);
    try {
      setUiError("");
      setPairingStatus("sending…");
      await invoke("enter_pairing");
      setPairingStatus("sent — keyboard will reboot into pairing mode");
    } catch (error) {
      setPairingStatus("");
      setUiError(error instanceof Error ? error.message : String(error));
    } finally {
      setPairingBusy(false);
    }
  }

  // ── Primary Kiro Crew local API + WebSocket path ──────────────────────────
  async function runCrewApi<T>(action: () => Promise<T>): Promise<T | undefined> {
    setCrewApiBusy(true);
    try {
      setCrewApiError("");
      return await action();
    } catch (error) {
      setCrewApiError(error instanceof Error ? error.message : String(error));
      return undefined;
    } finally {
      setCrewApiBusy(false);
    }
  }

  async function refreshCrewSlots() {
    const slots = await invoke<CrewSlot[]>("crew_api_list_slots");
    setCrewSlots(slots);
    if (!crewSelectedSlot || !slots.some((slot) => slot.key === crewSelectedSlot)) {
      setCrewSelectedSlot(slots[0]?.key ?? "");
    }
  }

  async function sendCrewApiMessage() {
    const message = crewMessage.trim();
    if (!crewSelectedSlot) throw new Error("Select a Kiro Crew chat first.");
    if (!message) throw new Error("Type a message first.");
    await invoke("crew_api_send_message", { slot: crewSelectedSlot, message });
    setCrewMessage("");
  }

  async function startCrewApi() {
    await invoke("crew_api_start");
  }

  async function stopCrewApi() {
    await invoke("crew_api_stop");
  }

  async function crewSlotAction(action: "continue" | "interrupt" | "stop") {
    if (!crewSelectedSlot) throw new Error("Select a Kiro Crew chat first.");
    await invoke("crew_api_slot_action", { slot: crewSelectedSlot, action });
  }

  // ── FP-4b route B: Local Control (window focus + keystroke injection) ──────
  // Arg names match the #[tauri::command] signatures in src-tauri/src/lib.rs:
  //   kc_bind_window(title)   kc_send_text(text)
  //   kc_command_bar(cmd)     kc_chord(name)
  async function runLc(action: () => Promise<void>) {
    setLcBusy(true);
    try {
      setLcError("");
      await action();
    } catch (error) {
      setLcError(error instanceof Error ? error.message : String(error));
      setLcBound(false);
      setLcResult("bind/inject failed");
    } finally {
      setLcBusy(false);
    }
  }

  async function lcBind() {
    // Empty title => backend tries the default candidates ("Kiro Crew"/"Agentic").
    await invoke("kc_bind_window", { title: lcTitle.trim() });
    setLcBound(true);
    setLcResult(`focused ${lcTitle.trim() || "Kiro Crew / Agentic"}`);
  }

  async function lcSendText() {
    const text = lcText.trim();
    if (!text) {
      setLcError("Type some text to send.");
      return;
    }
    await invoke("kc_send_text", { text });
    setLcResult(`sent text (${text.length} chars)`);
  }

  async function lcCommandBar() {
    const cmd = lcText.trim();
    if (!cmd) {
      setLcError("Type a command for the command bar.");
      return;
    }
    await invoke("kc_command_bar", { cmd });
    setLcResult(`command bar: ${cmd}`);
    setLcText("");
  }

  async function lcChord(name: string) {
    await invoke("kc_chord", { name });
    setLcResult(`chord: ${name}`);
  }

  // ── FP-4b M4: per-key chat numbers and A/B/C aliases ──────────────────────
  // Arg names match the #[tauri::command] signatures in src-tauri/src/lib.rs:
  //   kc_bind_get() -> number[]      kc_bind_set(slot, number)
  //   kc_bind_aliases_get() -> string[]
  //   kc_bind_alias_set(slot, alias) -> string[]
  //   kc_bind_clear(slot)
  async function runBind(action: () => Promise<void>) {
    setBindBusy(true);
    try {
      setBindError("");
      await action();
    } catch (error) {
      setBindError(error instanceof Error ? error.message : String(error));
    } finally {
      setBindBusy(false);
    }
  }

  async function refreshBinds() {
    const [numbers, aliases, sessions] = await Promise.all([
      invoke<ChatNumbers>("kc_bind_get"),
      invoke<ChatAliases>("kc_bind_aliases_get"),
      invoke<ChatSessions>("kc_bind_sessions_get"),
    ]);
    setBinds(numbers);
    setBindAliases(aliases);
    setBindSessions(sessions);
  }

  // Set A/B/C slot `slot` (1-based API index) to chat `number` (1..9).
  async function bindSet(slot: number, number: number) {
    await invoke("kc_bind_set", { slot, number });
    await refreshBinds();
  }

  async function bindSession(slot: number, number: number, sessionId: string) {
    await invoke("kc_bind_session_set", { slot, number, sessionId });
    await refreshBinds();
  }

  async function clearBoundSession(slot: number) {
    await invoke("kc_bind_session_clear", { slot });
    await refreshBinds();
    setQuickBindDraft(null);
  }

  function startQuickBind(sessionId: string, slot: number) {
    setQuickBindDraft({ sessionId, slot, number: binds[slot - 1] ?? slot });
  }

  async function confirmQuickBind() {
    if (!quickBindDraft) return;
    const draft = quickBindDraft;
    await runBind(async () => {
      await bindSession(draft.slot, draft.number, draft.sessionId);
      setQuickBindDraft(null);
    });
  }

  // Mode 2: a physical K1/K2/K3 long-press binds the currently selected
  // Watched Chat session to that key's slot. Reads live values from refs (the
  // board-event listener closure is stale). Writes through bindSession — the
  // SAME path as the dropdown — so the settings UI updates automatically, then
  // mirrors a confirmation to the keyboard screen via a display frame.
  async function longPressBind(slot: number) {
    const sessionId = selectedSessionRef.current.trim();
    if (!sessionId) {
      setLongBindNotice(
        `长按 K${slot}:先在 Watched Chat 列表里选择一个对话,再长按按键绑定。`,
      );
      return;
    }
    const slots = crewSlotsRef.current;
    const match = slots.find((s) => s.key === sessionId);
    if (!match) {
      setLongBindNotice(`长按 K${slot}:选中的对话已不在 API 列表中,无法绑定。`);
      return;
    }
    // Preserve the slot's existing Alt+N fallback number (session binding is
    // what drives the jump while the API is connected; the number is only the
    // offline fallback and must stay a valid 1..=9 for the backend).
    const defaultName = SLOT_DEFAULT_NAMES[slot - 1];
    // Preserve the slot's existing Alt+N jump setting (0 = not set).
    const number = bindsRef.current[slot - 1] ?? 0;
    const alias = bindAliasesRef.current[slot - 1] || defaultName;
    await runBind(async () => {
      await bindSession(slot, number, sessionId);
      setSelectedSession("");
      const label = match.title || sessionDisplayName(sessionId);
      setLongBindNotice(`已绑定 Chat ${defaultName} (K${slot}) → ${label}`);
      // Mirror to the keyboard screen (best-effort; ignored when board offline).
      try {
        await invoke("send_display", {
          chatSlot: slot,
          agentName: alias,
          displayState: "idle",
          text: `Bound K${slot}->Chat ${defaultName}`,
        });
      } catch {
        // Board may be disconnected; the on-screen UI notice still confirms it.
      }
    });
  }

  function bindAliasDraft(slot: number, alias: string) {
    setBindAliases((current) => {
      const next = [...current];
      next[slot - 1] = alias;
      return next;
    });
  }

  async function bindAliasSave(slot: number, alias: string) {
    const aliases = await invoke<ChatAliases>("kc_bind_alias_set", { slot, alias });
    setBindAliases(aliases);
  }

  // ── FP-4b STATUS sink (the eyes) ──────────────────────────────────────────
  // Arg names match the #[tauri::command] signatures in src-tauri/src/lib.rs:
  //   status_sink_start(port?) -> String (token)   status_sink_stop()
  //   status_token() -> String
  async function runSs(action: () => Promise<void>) {
    setSsBusy(true);
    try {
      setSsError("");
      await action();
    } catch (error) {
      setSsError(error instanceof Error ? error.message : String(error));
    } finally {
      setSsBusy(false);
    }
  }

  async function ssStart() {
    const token = await invoke<string>("status_sink_start", { port: null });
    setSsToken(token);
    setSsRunning(true);
  }

  async function ssStop() {
    await invoke("status_sink_stop");
    setSsRunning(false);
    setSsToken("");
  }

  // Pin one session as the watched chat (only its status drives the board), or
  // pass "" to unpin and follow every session again.
  async function pinSession(sessionId: string) {
    await invoke("kc_pin_session", { sessionId });
    setPinnedSession(sessionId);
  }

  async function unpinSession() {
    await invoke("kc_unpin_session");
    setPinnedSession("");
  }

  // FP-4b: global Crew ScriptHookStore install/remove/status.
  async function hookRefresh() {
    setHookStatus(await invoke<HookStatus>("kc_hook_status", { exe: null }));
  }

  async function hookOpenFolder() {
    await invoke<string>("kc_open_hooks_folder");
  }

  async function hookInstall() {
    setHookStatus(await invoke<HookStatus>("kc_hook_install", { exe: null }));
  }

  async function hookRemove() {
    setHookStatus(await invoke<HookStatus>("kc_hook_remove", { exe: null }));
  }

  useEffect(() => {
    refreshRuntime().catch(() => undefined);
    const timer = window.setInterval(() => {
      // Never pile up status requests behind a blocked backend mutex. One
      // outstanding poll is enough; the persistent backend watchdog records
      // lock stalls for post-mortem diagnosis.
      if (statusPollInFlightRef.current) return;
      statusPollInFlightRef.current = true;
      invoke<Status>("get_status")
        .then(setStatus)
        .catch((error) => console.warn("get_status failed", error))
        .finally(() => {
          statusPollInFlightRef.current = false;
        });
    }, 1500);

    const unlistenLog = listen<string>("companion-log", (event) => {
      if (logsPausedRef.current) {
        logBufRef.current = [event.payload, ...logBufRef.current].slice(0, 200);
        setPausedCount((n) => n + 1);
        return;
      }
      setLogs((items) => [event.payload, ...items].slice(0, 200));
    });

    const unlistenBoard = listen<BoardEvent>("board-event", (event) => {
      const payload = event.payload;
      if (logsPausedRef.current) {
        rawBufRef.current = [JSON.stringify(payload), ...rawBufRef.current].slice(0, 200);
      } else {
        setLogsRaw((items) => [JSON.stringify(payload), ...items].slice(0, 200));
      }
      if (payload.kind === "key" && payload.id) {
        if (payload.action === "down" || payload.action === "long") {
          setLastKey(payload.id);
          // Mode 2: long-pressing K1/K2/K3 auto-binds the selected chat to the
          // matching A/B/C slot. Fire-and-forget; longPressBind reads live refs.
          if (
            payload.action === "long" &&
            (payload.id === "k1" || payload.id === "k2" || payload.id === "k3")
          ) {
            const slot = payload.id === "k1" ? 1 : payload.id === "k2" ? 2 : 3;
            void longPressBind(slot);
          }
        } else if (payload.action === "up") {
          // Release clears the highlight so the INPUT view reflects only the
          // current physical trigger state, not the last-pressed key.
          setLastKey((cur) => (cur === payload.id ? "" : cur));
        }
      } else if (payload.kind === "nav" && payload.dir) {
        if (payload.action === "down" || payload.action === "long") {
          setLastNav(payload.dir);
        } else if (payload.action === "up") {
          setLastNav((cur) => (cur === payload.dir ? "" : cur));
        }
      } else if (payload.kind === "voice") {
        setVoiceActive(payload.state === "start");
      }
    });

    // Hook and API statuses both enter the same backend `kc-status` event,
    // preserving Watched Chat and Chat A/B/C LCD routing.
    const unlistenStatus = listen<KcStatus>("kc-status", (event) => {
      const s = event.payload;
      setKcStatuses((prev) => ({ ...prev, [s.session_id]: s }));
    });

    const unlistenCrewConnection = listen<CrewConnection>("crew-api-connection", (event) => {
      const connection = event.payload;
      setStatus((current) => ({
        ...current,
        crew_api_connected: connection.connected,
        crew_api_base_url: connection.base_url,
        crew_api_last_error: connection.error,
      }));
    });

    const unlistenCrewSlots = listen<CrewSlot[]>("crew-api-slots", (event) => {
      const slots = event.payload;
      setCrewSlots(slots);
      setStatus((current) => ({ ...current, crew_api_slots: slots.length }));
      setCrewSelectedSlot((current) =>
        current && slots.some((slot) => slot.key === current) ? current : (slots[0]?.key ?? ""),
      );
    });

    const unlistenCrewActiveSlot = listen<string>("crew-api-active-slot", (event) => {
      setCrewSelectedSlot(event.payload);
    });

    const unlistenCrewDraft = listen<{ slot: string; text: string }>(
      "crew-api-draft",
      (event) => {
        setCrewSelectedSlot(event.payload.slot);
        setCrewMessage(event.payload.text);
      },
    );

    // The board reports its current voice routing (cloud/system) on change and
    // on hello — reflect a board-side switch in the selector.
    const unlistenVoiceSource = listen<string>("voice-source", (event) => {
      const src = event.payload === "cloud" ? "cloud" : "system";
      setVoiceSource(src);
    });

    // The board reports WiFi provisioning progress.
    const unlistenWifi = listen<{ state: string; ssid: string }>("wifi-status", (event) => {
      const { state, ssid } = event.payload;
      setWifiStatus(ssid ? `${state} · ${ssid}` : state);
      setActivationStatus((current) => {
        const preActivation = ["unknown", "pairing", "connecting_router", "wifi_connected", "router_failed"]
          .includes(current.state);
        if (!preActivation) return current;
        if (state === "connected") return { state: "wifi_connected", qr_url: "" };
        if (state === "failed" || state === "disconnected") {
          return { state: "router_failed", qr_url: "" };
        }
        if (state === "connecting") return { state: "connecting_router", qr_url: "" };
        return current;
      });
    });

    const unlistenActivation = listen<ActivationStatus>("activation-status", (event) => {
      const next = event.payload;
      setActivationStatus(next);
      if (next.state === "scan_qr" || next.state === "token_received" || next.state === "activated") {
        setActivationExpanded(true);
      } else if (next.state === "cloud_ready" || next.state === "resetting") {
        setActivationExpanded(false);
      }
    });

    // The board reports device-license provisioning result.
    const unlistenLicense = listen<string>("license-status", (event) => {
      setLicStatus(event.payload);
    });

    const unlistenLicenseInfo = listen<LicenseInfo>("license-info", (event) => {
      setLicInfo(event.payload);
      setLicStatus(`read · ${event.payload.source}`);
    });

    // The AK reports authoritative values after hello and every local/remote change.
    const unlistenDeviceSettings = listen<DeviceSettings>("device-settings", (event) => {
      setDeviceSettings(event.payload);
      setStatus((current) => ({ ...current, device_settings_synced: true }));
    });

    // Recover the sink token/running state on mount (survives a UI reload).
    invoke<string>("status_token")
      .then((token) => {
        if (token) {
          setSsToken(token);
          setSsRunning(true);
        }
      })
      .catch(() => undefined);

    // Load persisted A/B/C chat numbers and optional AK display aliases.
    invoke<ChatNumbers>("kc_bind_get")
      .then(setBinds)
      .catch(() => undefined);
    invoke<ChatAliases>("kc_bind_aliases_get")
      .then(setBindAliases)
      .catch(() => undefined);
    invoke<ChatSessions>("kc_bind_sessions_get")
      .then(setBindSessions)
      .catch(() => undefined);

    // Recover the pinned "watched chat" on mount (survives a UI reload).
    invoke<string>("kc_get_pinned")
      .then((sid) => setPinnedSession(sid || ""))
      .catch(() => undefined);

    // Prime the REST-backed list as a cold-start/reconnect fallback. The
    // WebSocket worker will keep it live after the initial snapshot.
    invoke<CrewSlot[]>("crew_api_list_slots")
      .then((slots) => {
        setCrewSlots(slots);
        setCrewSelectedSlot((current) => current || slots[0]?.key || "");
      })
      .catch(() => undefined);

    // Load the user-level global Crew hook status on first mount.
    invoke<HookStatus>("kc_hook_status", { exe: null })
      .then(setHookStatus)
      .catch(() => undefined);

    return () => {
      window.clearInterval(timer);
      unlistenLog.then((fn) => fn());
      unlistenBoard.then((fn) => fn());
      unlistenStatus.then((fn) => fn());
      unlistenCrewConnection.then((fn) => fn());
      unlistenCrewSlots.then((fn) => fn());
      unlistenCrewActiveSlot.then((fn) => fn());
      unlistenCrewDraft.then((fn) => fn());
      unlistenVoiceSource.then((fn) => fn());
      unlistenWifi.then((fn) => fn());
      unlistenActivation.then((fn) => fn());
      unlistenLicense.then((fn) => fn());
      unlistenLicenseInfo.then((fn) => fn());
      unlistenDeviceSettings.then((fn) => fn());
    };
  }, []);

  return (
    <main className="shell">
      <aside className="sidebar">
        <div className="brand">
          <h1>Agentic Keyboard</h1>
          <p>Companion · T5AI-Core</p>
        </div>
        {(["device", "input", "kirocrew", "logs"] as const).map((view) => (
          <button
            key={view}
            className={activeView === view ? "nav active" : "nav"}
            onClick={() => setActiveView(view)}
          >
            {view === "kirocrew" ? "LOCAL CTRL" : view.toUpperCase()}
          </button>
        ))}
        <div className="sidebar-footer">
          <p>Protocol v1 · UART 115200</p>
          <p>v0.1.0</p>
        </div>
      </aside>

      <section className="content">
        <header className="topbar">
          <div>
            <div className="eyebrow">UART / USB-CDC · 5 keys + 5-way nav · voice</div>
            <h2>{viewTitle(activeView)}</h2>
          </div>
          <div className={status.device_connected ? "status online" : "status"}>
            {status.device_connected ? "BOARD ONLINE" : "BOARD OFFLINE"}
          </div>
        </header>

        <section className="lcd-grid">
          <div className="lcd">
            <span>LINK</span>
            <strong>{status.device_connected ? status.transport.toUpperCase() : "OFFLINE"}</strong>
          </div>
          <div className="lcd">
            <span>ENDPOINT</span>
            <strong>{status.endpoint || "--"}</strong>
          </div>
          <div className="lcd">
            <span>FIRMWARE</span>
            <strong>{status.firmware_version || "--"}</strong>
          </div>
          <div className="lcd">
            <span>VOICE</span>
            <strong>{voiceActive ? "LISTENING" : "IDLE"}</strong>
          </div>
        </section>

        {(uiError || status.last_error) && (
          <div className="error-banner">{uiError || status.last_error}</div>
        )}

        {isMac && status.input_permission === "missing" && (
          <div className="warning-banner" role="alert">
            <span>
              Accessibility permission is missing — the Companion cannot focus the Kiro Crew
              window or inject keystrokes until it is granted.
            </span>
            <button onClick={() => runAction(grantAccessibility)}>Grant Accessibility</button>
          </div>
        )}

        {activeView === "device" && (
          <Panel title="Device Bridge">
            <label>Transport</label>
            <div className="actions" style={{ marginTop: 6 }}>
              <button
                className={transportMode === "usb" ? "" : "secondary"}
                onClick={() => setTransportMode("usb")}
              >
                USB
              </button>
              <button
                className={transportMode === "ble" ? "" : "secondary"}
                onClick={() => setTransportMode("ble")}
              >
                BLE
              </button>
            </div>

            {transportMode === "usb" ? (
              <>
                <label>Board USB port</label>
                <select value={selectedPort} onChange={(event) => setSelectedPort(event.target.value)}>
                  <option value="">Auto-detect (CH340/CH343)</option>
                  {boardPorts.map((port) => (
                    <option key={port.name} value={port.name}>
                      {port.name} · {port.kind}
                    </option>
                  ))}
                </select>
                <p className="hint good-text">
                  USB/UART is physically trusted — no pairing. The companion greets the board on connect.
                </p>
              </>
            ) : (
              <>
                <label>Agentic Keyboard device</label>
                <select value={selectedBle} onChange={(event) => setSelectedBle(event.target.value)}>
                  {bleDevices.length === 0 ? (
                    <option value="">No devices — press Scan</option>
                  ) : (
                    bleDevices.map((dev) => (
                      <option key={dev.id} value={dev.id}>
                        {dev.name || "Agentic Keyboard"} · {dev.id}
                      </option>
                    ))
                  )}
                </select>
                <div className="actions" style={{ marginTop: 6 }}>
                  <button
                    className="secondary"
                    disabled={bleScanning}
                    onClick={() => runAction(scanBle)}
                  >
                    {bleScanning ? "Scanning…" : "Scan"}
                  </button>
                </div>
                <p className="hint warn-text">
                  BLE GATT · current firmware exposes Tuya command service{" "}
                  <code>0xFD50</code> (notify <code>0x0002</code> board→PC, write
                  <code> 0x0001</code> PC→board); older builds use{" "}
                  <code>0x1910</code>/<code>0x2B10</code>/<code>0x2B11</code> and the
                  companion accepts either. Scan (~4s) finds current boards advertising{" "}
                  <code>Agentic Keyboard-XXXXXX</code> and legacy boards advertising{" "}
                  <code>AK-KB</code>; the companion greets only after notify subscription is live.
                </p>
              </>
            )}

            <div className="actions">
              <button className="secondary" onClick={() => runAction(connect)}>Connect</button>
              <button className="secondary" onClick={() => runAction(disconnect)}>Disconnect</button>
              <button className="secondary" onClick={() => runAction(refreshRuntime)}>Refresh</button>
            </div>

            <div className="device-settings-compact" aria-label="AK device settings">
              <span className="device-settings-title">DEVICE SETTINGS</span>
              {([
                { key: "volume" as const, label: "VOL", minimum: 0 },
                { key: "brightness" as const, label: "SCREEN", minimum: 10 },
              ]).map(({ key, label, minimum }) => (
                <div className="device-setting-control" key={key}>
                  <span>{label}</span>
                  <button
                    className="secondary"
                    aria-label={`Decrease ${label.toLowerCase()}`}
                    disabled={
                      !status.device_connected ||
                      !status.device_settings_synced ||
                      deviceSettingsBusy ||
                      deviceSettings[key] <= minimum
                    }
                    onClick={() => runAction(() => adjustDeviceSetting(key, -10))}
                  >
                    −
                  </button>
                  <strong>{deviceSettings[key]}%</strong>
                  <button
                    className="secondary"
                    aria-label={`Increase ${label.toLowerCase()}`}
                    disabled={
                      !status.device_connected ||
                      !status.device_settings_synced ||
                      deviceSettingsBusy ||
                      deviceSettings[key] >= 100
                    }
                    onClick={() => runAction(() => adjustDeviceSetting(key, 10))}
                  >
                    +
                  </button>
                </div>
              ))}
              <span className={`device-settings-sync ${status.device_settings_synced ? "good-text" : ""}`}>
                {!status.device_connected
                  ? "AK OFFLINE"
                  : status.device_settings_synced
                    ? "AK SYNCED"
                    : "WAITING FOR AK"}
              </span>
            </div>

            <label>Voice recognition path</label>
            <div className="actions" style={{ marginTop: 6 }}>
              <button
                className={voiceSource === "cloud" ? "" : "secondary"}
                onClick={() => runAction(() => selectVoiceSource("cloud"))}
              >
                Board cloud ASR
              </button>
              <button
                className={voiceSource === "system" ? "" : "secondary"}
                onClick={() => runAction(() => selectVoiceSource("system"))}
              >
                PC system ASR
              </button>
            </div>
            <p className="hint">
              <strong>Board cloud ASR</strong>: the board mic feeds Tuya cloud ASR; recognized text
              returns as an <code>asr</code> frame. <strong>PC system ASR</strong>: a K4 press sends
              <code> voice.start</code> and the companion activates {labels.dictation}. The
              choice is mirrored to the board (and the board can switch it too — this reflects the
              board's reported path).
            </p>
            {isMac && (
              <div className="dictation-trigger">
                <label htmlFor="mac-dictation-trigger">macOS dictation trigger</label>
                <select
                  id="mac-dictation-trigger"
                  value={status.mac_dictation_trigger ?? "edit-menu"}
                  onChange={(event) =>
                    runAction(() => selectDictationTrigger(event.target.value))
                  }
                >
                  {MAC_DICTATION_TRIGGERS.map((t) => (
                    <option key={t.value} value={t.value}>
                      {t.label}
                    </option>
                  ))}
                </select>
                <p className="hint">
                  Edit-menu works with any shortcut; double-key options must match System
                  Settings → Keyboard → Dictation. Fn/🌐 cannot be simulated.
                </p>
              </div>
            )}

            <label style={{ marginTop: 14 }}>WiFi provisioning (for board cloud ASR)</label>
            <input
              value={wifiSsid}
              disabled={!usbProvisioningAvailable || wifiBusy}
              onChange={(event) => setWifiSsid(event.target.value)}
              placeholder="WiFi SSID (2.4GHz)"
            />
            <div className="input-with-toggle" style={{ display: "flex", gap: 6, marginTop: 6 }}>
              <input
                type={showWifiPass ? "text" : "password"}
                value={wifiPass}
                disabled={!usbProvisioningAvailable || wifiBusy}
                onChange={(event) => setWifiPass(event.target.value)}
                placeholder="WiFi password"
                style={{ flex: 1 }}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && usbProvisioningAvailable && !wifiBusy) {
                    runAction(sendWifi);
                  }
                }}
              />
              <button
                type="button"
                className="secondary"
                onClick={() => setShowWifiPass((v) => !v)}
                aria-label={showWifiPass ? "Hide password" : "Show password"}
                title={showWifiPass ? "Hide password" : "Show password"}
              >
                {showWifiPass ? "Hide" : "Show"}
              </button>
            </div>
            <div className="actions" style={{ marginTop: 6 }}>
              <button
                disabled={!usbProvisioningAvailable || wifiBusy}
                onClick={() => runAction(sendWifi)}
              >
                Configure WiFi
              </button>
            </div>
            <div className="lcd" style={{ marginTop: 6 }}>
              <span>WIFI</span>
              <strong className={wifiStatus.startsWith("connected") ? "good-text" : ""}>
                {wifiStatus || "-- (not configured)"}
              </strong>
            </div>
            <p className={`hint ${usbProvisioningAvailable ? "good-text" : "warn-text"}`}>
              USB connection required. WiFi credentials cannot be sent over BLE. The board joins
              its 2.4GHz AP using the cloud-ASR firmware profile. If first-time setup is needed,
              the SmartLife QR card below opens automatically.
            </p>

            <section className={`activation-card ${activationExpanded ? "expanded" : ""}`}>
              <button
                type="button"
                className="activation-toggle"
                aria-expanded={activationExpanded}
                onClick={() => setActivationExpanded((value) => !value)}
              >
                <span>
                  <strong>SmartLife setup</strong>
                  <small>First setup or after network reset</small>
                </span>
                <span className={`activation-badge state-${activationStatus.state}`}>
                  {activationLabel(activationStatus.state)}
                </span>
                <span className="activation-chevron" aria-hidden="true" />
              </button>

              {activationExpanded && (
                <div className="activation-body">
                  <div className="activation-steps" aria-label="Activation progress">
                    {ACTIVATION_STEPS.map((step, index) => {
                      const current = activationStepIndex(activationStatus.state);
                      const stepClass = index < current ? "done" : index === current ? "current" : "";
                      return (
                        <div className={`activation-step ${stepClass}`} key={step}>
                          <span>{index + 1}</span>
                          <small>{step}</small>
                        </div>
                      );
                    })}
                  </div>

                  {activationStatus.state === "scan_qr" && (
                    <div className="activation-qr-row">
                      <div className="activation-qr">
                        {activationQrDrawing ? (
                          <div
                            className="activation-qr-svg"
                            aria-label="SmartLife device activation QR code"
                            role="img"
                          >
                            <svg
                              viewBox={`-1 -1 ${activationQrDrawing.size + 2} ${activationQrDrawing.size + 2}`}
                              shapeRendering="crispEdges"
                              xmlns="http://www.w3.org/2000/svg"
                            >
                              <rect x="-1" y="-1" width={activationQrDrawing.size + 2} height={activationQrDrawing.size + 2} fill="#fff" />
                              <path d={activationQrDrawing.path} fill="#000" />
                            </svg>
                          </div>
                        ) : (
                          <span>{activationQrError || "Generating QR code…"}</span>
                        )}
                      </div>
                      <div>
                        <strong>Scan with SmartLife</strong>
                        <p>
                          Open SmartLife, tap the scan icon, and scan this code. Keep the keyboard
                          powered and connected to WiFi until Setup complete appears.
                        </p>
                        {activationQrError && <p className="warn-text">{activationQrError}</p>}
                      </div>
                    </div>
                  )}

                  {activationStatus.state !== "scan_qr" && (
                    <p className="activation-message">
                      {activationStatus.state === "wifi_connected"
                        ? "WiFi connected successfully. Preparing the SmartLife setup code."
                        : activationStatus.state === "router_failed"
                          ? "Could not connect to WiFi. Check the 2.4GHz network name and password, then try again."
                          : activationStatus.state === "token_received"
                            ? "SmartLife confirmed the keyboard. Completing device setup now."
                            : activationStatus.state === "activated"
                              ? "Registration successful. Finishing the online connection."
                              : activationStatus.state === "cloud_ready"
                                ? "Setup complete. The keyboard will reconnect after reboot without scanning or pairing again."
                                : activationStatus.state === "cloud_disconnected"
                                  ? "Setup is saved, but the online service is temporarily unavailable. PC ASR remains available."
                                  : "Configure WiFi to begin. This card opens automatically if SmartLife scanning is required."}
                    </p>
                  )}
                </div>
              )}
            </section>

            <label style={{ marginTop: 14 }}>Re-enter pairing mode</label>
            <div className="actions" style={{ marginTop: 6 }}>
              <button
                className="secondary"
                disabled={!usbProvisioningAvailable || pairingBusy}
                onClick={() => enterPairing()}
              >
                Enter pairing mode
              </button>
            </div>
            <div className="lcd" style={{ marginTop: 6 }}>
              <span>PAIRING</span>
              <strong>{pairingStatus || "-- (idle)"}</strong>
            </div>
            <p className={`hint ${usbProvisioningAvailable ? "good-text" : "warn-text"}`}>
              USB connection required. Clears the board's saved WiFi and cloud binding and reboots
              it into provisioning so you can re-provision WiFi. The device license (UUID/AuthKey)
              is preserved. Same effect as three fast power cycles.
            </p>

            <label style={{ marginTop: 14 }}>Device license (UUID / AuthKey)</label>
            <input
              value={licUuid}
              disabled={!usbProvisioningAvailable || licBusy}
              onChange={(event) => setLicUuid(event.target.value)}
              placeholder="UUID (20 chars)"
            />
            <input
              type="password"
              value={licAuthkey}
              disabled={!usbProvisioningAvailable || licBusy}
              onChange={(event) => setLicAuthkey(event.target.value)}
              placeholder="AuthKey (32 chars)"
              style={{ marginTop: 6 }}
            />
            <div className="actions" style={{ marginTop: 6 }}>
              <button
                disabled={!usbProvisioningAvailable || licBusy}
                onClick={() => runAction(sendLicense)}
              >
                Write license
              </button>
              <button
                className="secondary"
                disabled={!usbProvisioningAvailable || licBusy}
                onClick={() => runAction(readLicense)}
              >
                Read license info
              </button>
            </div>
            <div className="lcd" style={{ marginTop: 6 }}>
              <span>LICENSE</span>
              <strong className={licStatus.startsWith("written") ? "good-text" : ""}>
                {licStatus || "-- (not written)"}
              </strong>
            </div>
            {licInfo && (
              <p className="hint">
                Source: <code>{licInfo.source}</code> · UUID: <code>{licInfo.uuid || "--"}</code> ·
                AuthKey: <code>{licInfo.authkey_tail ? `****${licInfo.authkey_tail}` : "--"}</code>
              </p>
            )}
            <p className={`hint ${usbProvisioningAvailable ? "good-text" : "warn-text"}`}>
              USB connection required. UUID/AuthKey cannot be written over BLE. The license is
              stored on the board; the PID remains compile-time. Debug read shows the full UUID
              and only the last four AuthKey characters; Companion never persists the secret.
            </p>
          </Panel>
        )}

        {activeView === "input" && (
          <Panel title="Live Input">
            <p className="hint">
              Highlights the last key / direction reported by the board. Default intents shown are a
              placeholder for the remapping UI.
            </p>
            <div className="lcd" style={{ marginTop: 6 }}>
              <span>KIRO CREW CONTROL</span>
              <strong className={status.control_enabled ? "good-text" : ""}>
                {status.control_enabled ? "ENABLED — keys drive Kiro Crew" : "DISABLED — trigger check only"}
              </strong>
            </div>
            <div className="actions" style={{ marginTop: 6 }}>
              <button
                className={status.control_enabled ? "secondary" : ""}
                onClick={() => runAction(() => toggleControl(false))}
              >
                Disable control
              </button>
              <button
                className={status.control_enabled ? "" : "secondary"}
                onClick={() => runAction(() => toggleControl(true))}
              >
                Enable control
              </button>
            </div>
            <p className="hint">
              Default is <strong>enabled</strong>: board keys/nav drive the local Kiro Crew window,
              and K4 may activate system dictation. Disable control to keep input visualization
              while blocking every board-triggered PC action.
            </p>
            <div className="input-row">
              <div className="nav-cluster">
                <label className="cluster-label">5-WAY NAV</label>
                <div className="nav-pad">
                  {NAV_LAYOUT.map((dir, index) => (
                    <div
                      key={index}
                      className={
                        dir === null
                          ? "nav-cell"
                          : lastNav === dir
                            ? "nav-cell dir hit"
                            : "nav-cell dir"
                      }
                    >
                      {dir ? dir.toUpperCase() : ""}
                    </div>
                  ))}
                </div>
              </div>

              <div className="key-row">
                {KEY_MAP.map((key) => (
                  <div
                    className={
                      (lastKey === key.id ? "key-card hit" : "key-card") +
                      (key.wide ? " wide" : "")
                    }
                    key={key.id}
                  >
                    <div className="key-screen">{key.label}</div>
                    <div className="key-map">{key.intent}</div>
                  </div>
                ))}
              </div>
            </div>
          </Panel>
        )}

        {activeView === "kirocrew" && (
          <>
            <Panel title="Kiro Crew API (Primary)">
              <p className="hint">
                Controls the local Crew gateway through REST and receives live chat state through
                WebSocket. Authentication is bootstrapped from
                <code> ~/.kiro/crew/.local_secret</code>; no secret or message content is logged.
                The Hook listener and window injection panels below remain available as fallback.
              </p>

              <div className="lcd-grid" style={{ marginTop: 10 }}>
                <div className="lcd">
                  <span>CREW API</span>
                  <strong className={status.crew_api_connected ? "good-text" : ""}>
                    {status.crew_api_connected ? "CONNECTED" : "RECONNECTING"}
                  </strong>
                </div>
                <div className="lcd">
                  <span>ENDPOINT</span>
                  <strong>{status.crew_api_base_url || "http://127.0.0.1:5476"}</strong>
                </div>
                <div className="lcd">
                  <span>CHATS</span>
                  <strong>{crewSlots.length || status.crew_api_slots || 0}</strong>
                </div>
              </div>

              <div className="actions" style={{ marginTop: 10 }}>
                <button
                  disabled={crewApiBusy}
                  onClick={() => void runCrewApi(refreshCrewSlots)}
                >
                  Refresh chats
                </button>
                <button
                  className="secondary"
                  disabled={crewApiBusy || Boolean(status.crew_api_connected)}
                  onClick={() => void runCrewApi(startCrewApi)}
                >
                  Start WebSocket
                </button>
                <button
                  className="secondary"
                  disabled={crewApiBusy || !status.crew_api_connected}
                  onClick={() => void runCrewApi(stopCrewApi)}
                >
                  Stop WebSocket
                </button>
              </div>

              <label style={{ marginTop: 14 }}>Target chat (stable slot key)</label>
              <select
                value={crewSelectedSlot}
                disabled={crewApiBusy || crewSlots.length === 0}
                onChange={(event) => setCrewSelectedSlot(event.target.value)}
              >
                {crewSlots.length === 0 && <option value="">No chats discovered</option>}
                {crewSlots.map((slot) => (
                  <option key={slot.key} value={slot.key}>
                    {slot.title || slot.key} · {slot.running ? "RUNNING" : "IDLE"} · {slot.key}
                  </option>
                ))}
              </select>

              <label style={{ marginTop: 12 }}>API message</label>
              <input
                value={crewMessage}
                disabled={crewApiBusy || !crewSelectedSlot}
                placeholder="Send directly to the selected Crew chat…"
                onChange={(event) => setCrewMessage(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && !crewApiBusy) {
                    void runCrewApi(sendCrewApiMessage);
                  }
                }}
              />
              <div className="actions" style={{ marginTop: 6, flexWrap: "wrap" }}>
                <button
                  disabled={crewApiBusy || !crewSelectedSlot || !crewMessage.trim()}
                  onClick={() => void runCrewApi(sendCrewApiMessage)}
                >
                  Send through API
                </button>
                <button
                  className="secondary"
                  disabled={crewApiBusy || !crewSelectedSlot}
                  onClick={() => void runCrewApi(() => crewSlotAction("continue"))}
                >
                  Continue
                </button>
                <button
                  className="secondary"
                  disabled={crewApiBusy || !crewSelectedSlot}
                  onClick={() => void runCrewApi(() => crewSlotAction("interrupt"))}
                >
                  Interrupt
                </button>
                <button
                  className="secondary"
                  disabled={crewApiBusy || !crewSelectedSlot}
                  onClick={() => void runCrewApi(() => crewSlotAction("stop"))}
                >
                  Stop
                </button>
              </div>

              {(crewApiError || status.crew_api_last_error) && (
                <div className="error-banner" style={{ marginTop: 12 }}>
                  {crewApiError || status.crew_api_last_error} · Hook fallback remains available.
                </div>
              )}
            </Panel>

            <Panel title="Bind Chats with Keys (Control)">
              <p className="hint">
                为 Chat A / B / C 各绑定一个稳定的 Kiro Crew API 对话,并可选命名。K1–K3 直接
                选中对应的 API 对话(不抢窗口焦点),ASR 填入草稿、K5 提交。也可以在下方
                Watched Chat 列表里<strong>选中一个对话后长按 K1/K2/K3 自动绑定</strong>,结果会同步到这里。
                别名在回车或失焦时保存,最多 {ALIAS_MAX_CHARS} 个英文字母、数字、空格或符号。
              </p>
              <p className="hint">
                末尾的 <strong>切换对话窗格</strong> 是可选加成:设置了 <code>{labels.jumpChat("N")}</code> 号后,
                按 K1–K3 在选中对话的同时,还会让 Kiro Crew 界面<strong>跳转到第 N 个对话窗格</strong>
                (注入 {labels.jumpChat("N")},需要 Kiro Crew 窗口可被激活)。选 <strong>"未设置"</strong> 则只选中、不跳转。
                N 对应 Kiro Crew 侧栏里该对话的位置,侧栏顺序变化时可能需要重设。
              </p>

              {[1, 2, 3].map((slot) => {
                const n = binds[slot - 1];
                const alias = bindAliases[slot - 1] ?? "";
                const defaultName = SLOT_DEFAULT_NAMES[slot - 1];
                // Alt+N number: 0 (or undefined) means "not set" (no jump).
                const jumpN = n ?? 0;
                return (
                  <div key={slot} className="binding-row">
                    <strong className="binding-slot-label">
                      {`Chat ${defaultName} (K${slot}):`}
                    </strong>
                    <input
                      className="binding-alias-input"
                      value={alias}
                      maxLength={ALIAS_MAX_CHARS}
                      disabled={bindBusy}
                      aria-label={`Chat ${defaultName} name; blank uses ${defaultName}`}
                      placeholder={`别名 · 默认 ${defaultName}`}
                      onChange={(event) => bindAliasDraft(slot, event.target.value)}
                      onBlur={(event) =>
                        runBind(() => bindAliasSave(slot, event.currentTarget.value))
                      }
                      onKeyDown={(event) => {
                        if (event.key === "Enter") event.currentTarget.blur();
                      }}
                    />
                    <div className="binding-field">
                      <span className="hint">API 对话</span>
                      <select
                        value={bindSessions[slot - 1] ?? ""}
                        disabled={bindBusy || crewSlots.length === 0}
                        aria-label={`Chat ${defaultName} stable Crew API chat`}
                        onChange={(event) => {
                          const sessionId = event.target.value;
                          runBind(() =>
                            sessionId
                              ? bindSession(slot, jumpN, sessionId)
                              : clearBoundSession(slot),
                          );
                        }}
                      >
                        <option value="">未绑定</option>
                        {crewSlots.map((crewSlot) => (
                          <option key={crewSlot.key} value={crewSlot.key}>
                            {crewSlot.title || crewSlot.key} · {crewSlot.key}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div className="binding-field">
                      <span className="hint">切换对话窗格</span>
                      <select
                        className="binding-chat-select"
                        // 0 = 未设置 (press selects the slot but does not jump the
                        // Kiro Crew view). 1..=9 = also inject Alt+N on click.
                        value={jumpN}
                        disabled={bindBusy}
                        aria-label={`Chat ${defaultName} 切换对话窗格 ${labels.jumpChat("N")}`}
                        onChange={(event) =>
                          runBind(() => bindSet(slot, Number(event.target.value)))
                        }
                      >
                        <option value={0}>未设置 · 不跳转</option>
                        {[1, 2, 3, 4, 5, 6, 7, 8, 9].map((num) => (
                          <option key={num} value={num}>
                            跳到 {labels.jumpChat(num)}
                          </option>
                        ))}
                      </select>
                    </div>
                  </div>
                );
              })}

              {bindError && <div className="error-banner" style={{ marginTop: 14 }}>{bindError}</div>}
            </Panel>

            <Panel title="Bind the Window">
              <p className="hint">
                Focus a <strong>local</strong> Kiro Crew window and inject keystrokes directly
                (no HTTP). Board keys route to command-bar / chords through the same backend.
              </p>

              <label>Window title match</label>
              <input
                value={lcTitle}
                onChange={(event) => setLcTitle(event.target.value)}
                placeholder="Kiro Crew (blank = try Kiro Crew / Agentic)"
              />
              <div className="actions" style={{ marginTop: 6 }}>
                <button disabled={lcBusy} onClick={() => runLc(lcBind)}>
                  Bind window
                </button>
              </div>

              <div className="lcd" style={{ marginTop: 14 }}>
                <span>FOCUS</span>
                <strong>
                  {lcBound ? "BOUND" : "-- (bind first)"}
                  {lcResult ? ` · ${lcResult}` : ""}
                </strong>
              </div>

              <label style={{ marginTop: 18 }}>Text / command</label>
              <input
                value={lcText}
                onChange={(event) => setLcText(event.target.value)}
                placeholder="Type text to inject, or a command-bar command…"
                onKeyDown={(event) => {
                  if (event.key === "Enter" && !lcBusy) runLc(lcCommandBar);
                }}
              />
              <div className="actions" style={{ marginTop: 6 }}>
                <button disabled={lcBusy} onClick={() => runLc(lcSendText)}>
                  Send text
                </button>
                <button disabled={lcBusy} onClick={() => runLc(lcCommandBar)}>
                  {labels.commandBar}
                </button>
              </div>

              <label style={{ marginTop: 18 }}>Quick chords</label>
              <div className="actions" style={{ marginTop: 6, flexWrap: "wrap" }}>
                {["enter", "esc", "up", "down", "left", "right"].map((name) => (
                  <button
                    key={name}
                    className="secondary"
                    disabled={lcBusy}
                    onClick={() => runLc(() => lcChord(name))}
                  >
                    {name.toUpperCase()}
                  </button>
                ))}
              </div>

              {lcError && <div className="error-banner" style={{ marginTop: 14 }}>{lcError}</div>}
            </Panel>

            <Panel title="Agent Status Hook (Fallback)" collapsible defaultOpen={false}>
              <p className="hint">
                Fallback path when the primary WebSocket is unavailable. It receives Kiro Crew
                <strong> agent hooks</strong> on
                <code> 127.0.0.1:5477/status</code> and mirrors each agent's state to the board
                LCD as a <code>display</code> frame. The hook is a mode of this very binary
                (<code>companion --hook</code>), gated by a per-run token.
              </p>

              <label style={{ marginTop: 8 }}>Global Crew hook installation</label>
              <p className="hint" style={{ marginTop: 4 }}>
                Adds four global entries to Crew's shared ScriptHookStore at
                <code>{hookStatus?.hooks_dir ?? "~/.kiro/crew"}</code>. It covers
                <em> AgentSpawn / UserPromptSubmit / PostToolUse / Stop</em> and never
                modifies the separate ACP Agent Hooks shown below it in Crew.
              </p>
              <div className="lcd" style={{ marginTop: 6 }}>
                <span>HOOK</span>
                <strong className={hookStatus?.fully_installed ? "good-text" : ""}>
                  {!hookStatus
                    ? "-- (checking)"
                    : hookStatus.fully_installed
                    ? "CONFIGURED · 4/4"
                    : hookStatus.any_installed
                    ? `PARTIAL · ${hookStatus.installed_events}/4`
                    : "NOT INSTALLED"}
                </strong>
              </div>
              <p className="hint" style={{ marginTop: 4 }}>
                <code>{hookStatus?.hook_file ?? "~/.kiro/crew/hooks.json"}</code>
              </p>
              <div className="actions" style={{ marginTop: 6 }}>
                <button className="secondary" disabled={ssBusy} onClick={() => runSs(hookRefresh)}>
                  Refresh
                </button>
                <button className="secondary" disabled={ssBusy} onClick={() => runSs(hookOpenFolder)}>
                  Open Crew folder
                </button>
                <button disabled={ssBusy} onClick={() => runSs(hookInstall)}>
                  Install globally
                </button>
                <button
                  className="secondary"
                  disabled={ssBusy || !hookStatus?.any_installed}
                  onClick={() => runSs(hookRemove)}
                >
                  Remove global hook
                </button>
              </div>
              <p className="hint" style={{ marginTop: 4, opacity: 0.7 }}>
                After install, reinstall, or removal, click Apply & Restart in Kiro Crew (or
                restart Crew) so its in-memory HookStore reloads hooks.json. Reinstalling is
                idempotent and refreshes the Companion executable path.
              </p>

              <div className="listener-control-row">
                <button
                  type="button"
                  role="switch"
                  aria-checked={ssRunning}
                  aria-label={ssRunning ? "Stop status listener" : "Start status listener"}
                  className="listener-switch"
                  disabled={ssBusy}
                  onClick={() => runSs(ssRunning ? ssStop : ssStart)}
                >
                  <span className="listener-switch-action">{ssRunning ? "Stop" : "Start"}</span>
                  <span className="listener-switch-track" aria-hidden="true">
                    <span className="listener-switch-thumb" />
                  </span>
                </button>
                <div className="listener-inline-status" aria-live="polite">
                  <span>LISTENER</span>
                  <strong className={ssRunning ? "good-text" : ""}>
                    {ssRunning ? "LISTENING · 127.0.0.1:5477" : "STOPPED"}
                  </strong>
                </div>
              </div>

              {ssRunning && (
                <details className="listener-details">
                  <summary>
                    Listener details <span>Token &amp; global hook command</span>
                  </summary>
                  <div className="listener-details-body">
                    <label>Token (X-AK-Status-Token)</label>
                    <input readOnly value={ssToken} onFocus={(e) => e.currentTarget.select()} />

                    <label>Global hook command</label>
                    <div className="terminal" style={{ whiteSpace: "pre-wrap" }}>
                      {hookStatus?.command ?? labels.hookPlaceholder}
                    </div>
                    <p className="hint">
                      Each Crew ScriptHook invokes this command for dashboard agent turns. The hook
                      reads the sink port and token from
                      <code> ~/.ak-companion/hook.json</code> (all platforms). Starting the sink writes
                      that handoff file; no token is stored in the Crew hook configuration.
                    </p>
                  </div>
                </details>
              )}

              <div className="watched-toolbar">
                <div>
                  <label>Watched chat (shown on the keyboard screen)</label>
                  <p className="hint" style={{ marginTop: 4 }}>
                    {pinnedSession
                      ? `Watching ${pinnedSession} — only this chat appears on the keyboard screen; other chats are hidden.`
                      : "Following all sessions — the latest active chat updates the keyboard screen."}
                  </p>
                </div>
                <button
                  className="secondary"
                  disabled={!pinnedSession || ssBusy}
                  onClick={() => runSs(unpinSession)}
                >
                  Follow all
                </button>
              </div>

              <p className="hint watched-model-hint">
                Chat A–C use stable API chats when bound; {labels.jumpChatRange} remains the offline fallback.
                Watch chooses which live chat can update the keyboard screen.
              </p>

              <p className="hint">
                长按绑定:在下方列表里<strong>点选一个对话</strong>,再<strong>长按键盘上的
                K1 / K2 / K3</strong>,即可把它绑定到 Chat A / B / C。绑定结果会同步到上方
                "Bind Chats with Keys" 设置,并在键盘屏幕上显示提示。
              </p>

              {selectedSession && (
                <div className="watched-selected-banner">
                  已选中:<code>{selectedSession}</code> · 现在长按 K1/K2/K3 完成绑定
                  <button
                    className="secondary"
                    onClick={() => setSelectedSession("")}
                    style={{ marginLeft: 8 }}
                  >
                    取消选择
                  </button>
                </div>
              )}

              {longBindNotice && (
                <div className="watched-bind-notice">
                  {longBindNotice}
                  <button
                    className="secondary"
                    onClick={() => setLongBindNotice("")}
                    style={{ marginLeft: 8 }}
                  >
                    知道了
                  </button>
                </div>
              )}

              <div className="watched-tabs" role="tablist" aria-label="Watched chat view">
                <button
                  role="tab"
                  aria-selected={watchedView === "compact"}
                  className={watchedView === "compact" ? "watched-tab active" : "watched-tab"}
                  onClick={() => setWatchedView("compact")}
                >
                  Compact
                </button>
                <button
                  role="tab"
                  aria-selected={watchedView === "detail"}
                  className={watchedView === "detail" ? "watched-tab active" : "watched-tab"}
                  onClick={() => setWatchedView("detail")}
                >
                  Detail
                </button>
              </div>

              <div className="watched-list">
                {sortedKcStatuses.length === 0 ? (
                  <div className="watched-empty">
                    No active chats yet. Start the status listener above, then run a Kiro Crew turn.
                  </div>
                ) : watchedView === "compact" ? (
                  sortedKcStatuses.map((s) => {
                    const isPinned = s.session_id === pinnedSession;
                    const isSelected = s.session_id === selectedSession;
                    const boundSlot = bindSessions.findIndex((id) => id === s.session_id) + 1;
                    const draftForSession = quickBindDraft?.sessionId === s.session_id;
                    return (
                      <div
                        className={`watched-compact-card${isPinned ? " pinned" : ""}${isSelected ? " selected" : ""}`}
                        key={s.session_id}
                        role="button"
                        tabIndex={0}
                        aria-pressed={isSelected}
                        title="点击选中此对话,然后长按 K1/K2/K3 绑定到 Chat A/B/C"
                        onClick={(event) => {
                          // Ignore clicks that land on an inner control (the
                          // quick-bind buttons, Watch, the chat # select) so
                          // selecting the card never fights those actions.
                          if (
                            (event.target as HTMLElement).closest(
                              "button, select, input, label",
                            )
                          ) {
                            return;
                          }
                          setSelectedSession((cur) =>
                            cur === s.session_id ? "" : s.session_id,
                          );
                        }}
                        onKeyDown={(event) => {
                          if (
                            (event.key === "Enter" || event.key === " ") &&
                            event.target === event.currentTarget
                          ) {
                            event.preventDefault();
                            setSelectedSession((cur) =>
                              cur === s.session_id ? "" : s.session_id,
                            );
                          }
                        }}
                      >
                        <div className="watched-compact-row">
                          <div className="watched-session-summary">
                            <strong>{sessionDisplayName(s.session_id)}</strong>
                            <code>{s.session_id}</code>
                          </div>
                          <span className={`watched-state state-${s.state.toLowerCase()}`}>
                            {s.state.toUpperCase()}
                          </span>
                          <div className="watched-slot-binding">
                            <span>Bind chat</span>
                            <div className="watched-slot-actions" aria-label="Quick bind keys">
                              {[1, 2, 3].map((slot) => (
                              <button
                                key={slot}
                                className={bindSessions[slot - 1] === s.session_id ? "bound" : "secondary"}
                                disabled={bindBusy}
                                aria-label={`Bind ${s.session_id} to Chat ${SLOT_DEFAULT_NAMES[slot - 1]}`}
                                onClick={() => startQuickBind(s.session_id, slot)}
                              >
                                {SLOT_DEFAULT_NAMES[slot - 1]}
                              </button>
                              ))}
                            </div>
                          </div>
                          <button
                            className={isPinned ? "watched-watch active" : "watched-watch secondary"}
                            disabled={ssBusy}
                            onClick={() =>
                              runSs(() => isPinned ? unpinSession() : pinSession(s.session_id))
                            }
                          >
                            {isPinned ? "Watch all" : "Watch"}
                          </button>
                        </div>
                        <div className="watched-compact-meta">
                          {boundSlot
                            ? `Chat ${SLOT_DEFAULT_NAMES[boundSlot - 1]} → Kiro Crew Chat #${binds[boundSlot - 1]} (${labels.jumpChat(binds[boundSlot - 1])}) · screen label ${bindAliases[boundSlot - 1] || SLOT_DEFAULT_NAMES[boundSlot - 1]}`
                            : s.text}
                        </div>
                        {draftForSession && quickBindDraft && (
                          <div className="quick-bind-editor">
                            <strong>Bind Chat {SLOT_DEFAULT_NAMES[quickBindDraft.slot - 1]}</strong>
                            <label htmlFor={`quick-chat-${quickBindDraft.slot}`}>Kiro Crew Chat #</label>
                            <select
                              id={`quick-chat-${quickBindDraft.slot}`}
                              value={quickBindDraft.number}
                              disabled={bindBusy}
                              onChange={(event) =>
                                setQuickBindDraft({
                                  ...quickBindDraft,
                                  number: Number(event.target.value),
                                })
                              }
                            >
                              {[1, 2, 3, 4, 5, 6, 7, 8, 9].map((number) => (
                                <option key={number} value={number}>{number}</option>
                              ))}
                            </select>
                            <button disabled={bindBusy} onClick={confirmQuickBind}>Save binding</button>
                            {bindSessions[quickBindDraft.slot - 1] && (
                              <button
                                className="secondary"
                                disabled={bindBusy}
                                onClick={() => runBind(() => clearBoundSession(quickBindDraft.slot))}
                              >
                                Remove session link
                              </button>
                            )}
                            <button
                              className="secondary"
                              disabled={bindBusy}
                              onClick={() => setQuickBindDraft(null)}
                            >
                              Cancel
                            </button>
                          </div>
                        )}
                      </div>
                    );
                  })
                ) : (
                  sortedKcStatuses.map((s) => {
                    const isPinned = s.session_id === pinnedSession;
                    const driving = pinnedSession ? isPinned : true;
                    return (
                      <article
                        className={`watched-detail-card${isPinned ? " pinned" : ""}${driving ? "" : " muted"}`}
                        key={s.session_id}
                      >
                        <div className="watched-detail-head">
                          <div>
                            <strong>{sessionDisplayName(s.session_id)}</strong>
                            <span>{s.agent_name}</span>
                          </div>
                          <span className={`watched-state state-${s.state.toLowerCase()}`}>
                            {s.state.toUpperCase()}
                          </span>
                          <button
                            className={isPinned ? "watched-watch active" : "watched-watch secondary"}
                            disabled={ssBusy}
                            onClick={() =>
                              runSs(() => isPinned ? unpinSession() : pinSession(s.session_id))
                            }
                          >
                            {isPinned ? "Watch all" : "Watch"}
                          </button>
                        </div>
                        <dl className="watched-detail-fields">
                          <div><dt>Session ID</dt><dd><code>{s.session_id}</code></dd></div>
                          <div><dt>Event</dt><dd>{s.event || "Status update"}</dd></div>
                          <div><dt>Keyboard</dt><dd>{driving ? "Shown on keyboard screen" : "Hidden while another chat is watched"}</dd></div>
                        </dl>
                        <div className="watched-detail-text">{s.detail_text ?? s.text}</div>
                      </article>
                    );
                  })
                )}
              </div>

              {bindError && <div className="error-banner" style={{ marginTop: 14 }}>{bindError}</div>}
              {ssError && <div className="error-banner" style={{ marginTop: 14 }}>{ssError}</div>}
            </Panel>
          </>
        )}

        {activeView === "logs" && (
          <Panel title="Event Stream">
            <div className="actions" style={{ marginTop: 0 }}>
              <button
                className={logsShowRaw ? "secondary" : ""}
                onClick={() => setLogsShowRaw(false)}
              >
                Detailed
              </button>
              <button
                className={logsShowRaw ? "" : "secondary"}
                onClick={() => setLogsShowRaw(true)}
              >
                Raw JSON
              </button>
              <button
                className={logsPaused ? "" : "secondary"}
                onClick={toggleLogsPause}
                title={logsPaused ? "继续滚动并补回暂停期间的日志" : "冻结日志便于观察/复制"}
              >
                {logsPaused ? `▶ 继续${pausedCount ? ` (+${pausedCount})` : ""}` : "⏸ 暂停"}
              </button>
              <button className="secondary" onClick={() => runAction(clearLogs)}>Clear</button>
            </div>
            <p className="hint">
              {logsPaused
                ? `已暂停 — 显示已冻结，可安心选择/复制${
                    pausedCount ? `；暂停期间缓冲了 ${pausedCount} 条新日志，点"继续"补回` : ""
                  }。`
                : logsShowRaw
                ? "Raw board-frame JSON, one object per line (as received over the transport)."
                : "Human-readable event log ([RX] key/nav/voice, [KC-CTL], [INFO]/[ERROR])."}
            </p>
            <div className="log">
              {logsShowRaw ? (
                logsRaw.length === 0
                  ? "No frames yet. Connect the board to see raw JSON frames."
                  : logsRaw.map((line, index) => <div key={`raw-${index}`}>{line}</div>)
              ) : logs.length === 0 ? (
                "No events yet. Connect the board to see key / nav / voice frames."
              ) : (
                logs.map((line, index) => <div key={`${index}-${line}`}>{line}</div>)
              )}
            </div>
          </Panel>
        )}
      </section>
    </main>
  );
}

function Panel({
  title,
  children,
  collapsible = false,
  defaultOpen = true,
}: {
  title: string;
  children: ReactNode;
  // When collapsible, the panel renders as a <details> whose summary is the
  // title. `defaultOpen` controls the initial state (Hook fallback starts
  // collapsed to keep the primary API/WS controls above the fold).
  collapsible?: boolean;
  defaultOpen?: boolean;
}) {
  if (collapsible) {
    return (
      <details className="panel panel-collapsible" open={defaultOpen}>
        <summary>
          <h3>{title}</h3>
        </summary>
        <div className="panel-collapsible-body">{children}</div>
      </details>
    );
  }
  return (
    <section className="panel">
      <h3>{title}</h3>
      {children}
    </section>
  );
}

function viewTitle(view: string): string {
  switch (view) {
    case "input":
      return "Live Input";
    case "kirocrew":
      return "Local Control";
    case "logs":
      return "Event Stream";
    default:
      return "Device Control";
  }
}

createRoot(document.getElementById("root")!).render(<App />);
