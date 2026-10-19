//! Companion app library entry.
//!
//! Layering:
//! * [`protocol`] — pure v1 wire codec (no IO, unit-tested on any host).
//! * [`transport`] — `serialport`-based UART backend + a stable transport trait.
//! * [`inject`] / [`asr`] — Windows-only key injection and system dictation,
//!   each a trait with a real `#[cfg(windows)]` impl and a non-Windows stub.
//! * [`status_sink`] — FP-4b STATUS ("the eyes"): a pure `map_hook_event` (T5)
//!   plus a cross-platform `tiny_http` localhost sink (T6) that ingests Kiro
//!   agent-hook events (delivered by this same binary's `--hook` mode) and a
//!   `{port, token}` handoff file the `--hook` mode reads. Only the callback
//!   that emits a `display` frame is `#[cfg(windows)]` (in the app module).
//! * [`describe_inbound`] / [`inbound_to_json`] — pure frame-routing helpers,
//!   shared by the Tauri reader thread and testable on Linux.
//! * The Tauri application itself (`app` module + [`run`]) is `#[cfg(windows)]`
//!   because Tauri needs WebKitGTK, which is absent on the Linux CI host.

pub mod app_dirs;
pub mod asr;
pub mod diagnostics;
pub mod inject;
pub mod keymap;
pub mod kirocrew_control;
pub mod bind_store;
pub mod crew_api;
pub mod hook_install;
#[cfg(target_os = "macos")]
pub mod mac_sys;
pub mod pc_asr;
pub mod platform;
pub mod protocol;
pub mod status_sink;
pub mod transport;

use protocol::Inbound;

/// Render a decoded frame as a compact human-readable log line.
pub fn describe_inbound(frame: &Inbound) -> String {
    match frame {
        Inbound::Key { seq, payload } => format!(
            "key {} {:?} ({}ms){}",
            payload.id,
            payload.action,
            payload.held_ms,
            seq_suffix(*seq)
        ),
        Inbound::Nav { seq, payload } => format!(
            "nav {:?} {:?} ({}ms){}",
            payload.dir,
            payload.action,
            payload.held_ms,
            seq_suffix(*seq)
        ),
        Inbound::Voice { seq, payload } => {
            format!("voice {:?}{}", payload.state, seq_suffix(*seq))
        }
        Inbound::Macro { seq, payload } => {
            format!("macro {}{}", payload.name, seq_suffix(*seq))
        }
        Inbound::Asr { seq, payload } => {
            format!("asr {:?}{}", payload.text, seq_suffix(*seq))
        }
        Inbound::VoiceSource { seq, payload } => {
            format!("voice_source {}{}", payload.source, seq_suffix(*seq))
        }
        Inbound::WifiStatus { seq, payload } => {
            format!("wifi_status {} {}{}", payload.state, payload.ssid, seq_suffix(*seq))
        }
        Inbound::ActivationStatus { seq, payload } => format!(
            "activation_status {} qr={}{}",
            payload.state,
            if payload.qr_url.is_empty() { "no" } else { "yes" },
            seq_suffix(*seq)
        ),
        Inbound::LicenseStatus { seq, payload } => {
            format!("license_status {}{}", payload.state, seq_suffix(*seq))
        }
        Inbound::LicenseInfo { seq, payload } => format!(
            "license_info source={} uuid={} authkey=****{}{}",
            payload.source,
            payload.uuid,
            payload.authkey_tail,
            seq_suffix(*seq)
        ),
        Inbound::SystemInfo { seq, payload } => format!(
            "system_info fw={} transport={} wifi={}({}) ble={}({}) cloud={} license={}{}",
            payload.fw,
            payload.transport,
            payload.wifi_name,
            payload.wifi_state,
            payload.ble_name,
            payload.ble_state,
            payload.cloud,
            payload.license,
            seq_suffix(*seq)
        ),
        Inbound::DeviceSettings { seq, payload } => format!(
            "device_settings volume={} brightness={}{}",
            payload.volume,
            payload.brightness,
            seq_suffix(*seq)
        ),
        Inbound::HelloAck(ack) => format!(
            "hello_ack fw={} transport={} caps=[{}]",
            ack.fw,
            ack.transport,
            ack.capabilities.join(",")
        ),
        Inbound::Ping => "ping".to_string(),
        Inbound::Pong => "pong".to_string(),
        Inbound::Unknown { msg_type } => format!("unknown type={msg_type}"),
    }
}

fn seq_suffix(seq: Option<u64>) -> String {
    match seq {
        Some(n) => format!(" seq={n}"),
        None => String::new(),
    }
}

#[cfg(any(test, target_os = "windows", target_os = "macos"))]
fn ensure_usb_provisioning_transport(
    connected: bool,
    kind: transport::TransportKind,
) -> Result<(), String> {
    if connected && kind == transport::TransportKind::Usb {
        Ok(())
    } else {
        Err("WiFi and UUID/AuthKey settings require an active USB connection".to_string())
    }
}

/// Convert a decoded frame into a flat JSON object for the UI event stream.
pub fn inbound_to_json(frame: &Inbound) -> serde_json::Value {
    use serde_json::json;
    match frame {
        Inbound::Key { seq, payload } => json!({
            "kind": "key", "seq": seq, "id": payload.id,
            "action": payload.action, "held_ms": payload.held_ms
        }),
        Inbound::Nav { seq, payload } => json!({
            "kind": "nav", "seq": seq, "dir": payload.dir,
            "action": payload.action, "held_ms": payload.held_ms
        }),
        Inbound::Voice { seq, payload } => json!({
            "kind": "voice", "seq": seq, "state": payload.state
        }),
        Inbound::Macro { seq, payload } => json!({
            "kind": "macro", "seq": seq, "name": payload.name
        }),
        Inbound::Asr { seq, payload } => json!({
            "kind": "asr", "seq": seq, "text": payload.text
        }),
        Inbound::VoiceSource { seq, payload } => json!({
            "kind": "voice_source", "seq": seq, "source": payload.source
        }),
        Inbound::WifiStatus { seq, payload } => json!({
            "kind": "wifi_status", "seq": seq, "state": payload.state, "ssid": payload.ssid
        }),
        Inbound::ActivationStatus { seq, payload } => json!({
            "kind": "activation_status", "seq": seq,
            "state": payload.state, "qr_url": payload.qr_url
        }),
        Inbound::LicenseStatus { seq, payload } => json!({
            "kind": "license_status", "seq": seq, "state": payload.state
        }),
        Inbound::LicenseInfo { seq, payload } => json!({
            "kind": "license_info", "seq": seq,
            "source": payload.source, "uuid": payload.uuid,
            "authkey_tail": payload.authkey_tail
        }),
        Inbound::SystemInfo { seq, payload } => json!({
            "kind": "system_info", "seq": seq,
            "fw": payload.fw, "transport": payload.transport,
            "wifi_name": payload.wifi_name, "wifi_state": payload.wifi_state,
            "ble_name": payload.ble_name, "ble_state": payload.ble_state,
            "cloud": payload.cloud, "license": payload.license
        }),
        Inbound::DeviceSettings { seq, payload } => json!({
            "kind": "device_settings", "seq": seq,
            "volume": payload.volume, "brightness": payload.brightness
        }),
        Inbound::HelloAck(ack) => json!({
            "kind": "hello_ack", "fw": ack.fw,
            "transport": ack.transport, "capabilities": ack.capabilities
        }),
        Inbound::Ping => json!({ "kind": "ping" }),
        Inbound::Pong => json!({ "kind": "pong" }),
        Inbound::Unknown { msg_type } => json!({ "kind": "unknown", "type": msg_type }),
    }
}

/// Launch the desktop application (Windows and macOS).
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn run() {
    app::run();
}

/// Keep the pure Rust core testable on the Linux build host without GTK/WebKit.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn run() {
    eprintln!("the Companion desktop application is supported on Windows and macOS");
}

// ───────────────────── Tauri application (Windows, macOS) ─────────────────────

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod app {
    use crate::diagnostics;
    use crate::crew_api::{
        slot_display_status, CrewApiClient, CrewConnection, CrewEvent, CrewRealtime, CrewSlot,
        SendReceipt,
    };
    use crate::pc_asr::{self, DictationTrigger, VoiceAction, VoiceSource};
    use crate::platform::{self, HostOs};
    use crate::protocol::{self, Inbound, KeyAction};
    use crate::hook_install;
    use crate::kirocrew_control::{Chord, ControlAction, ControlRouter, KiroCrewController};
    use crate::bind_store::BindStore;
    use crate::status_sink::{DisplayState as StatusDisplay, StatusSink, DEFAULT_STATUS_PORT};
    use crate::transport::ble_gatt::{self, BleDeviceInfo, BleGattTransport};
    use crate::transport::usb_serial::{self, SerialPortInfo, UsbSerialTransport};
    use crate::transport::{BoardTransport, TransportKind};
    use crate::{describe_inbound, inbound_to_json};
    use serde::Serialize;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex, TryLockError};
    use std::thread;
    use std::time::{Duration, Instant};
    use tauri::{AppHandle, Emitter, Manager, State};

    struct Inner {
        // NOTE: the board transport is NOT here — it lives in
        // `AppState.transport` behind its own lock so blocking serial writes
        // never hold the global `inner` mutex. `kind`/`endpoint` stay here as
        // cheap metadata describing the current connection.
        kind: TransportKind,
        endpoint: String,
        firmware: String,
        connection_id: u64,
        logs: Vec<String>,
        last_error: String,
        voice_source: VoiceSource,
        /// macOS dictation trigger (persisted in `binds.json` as
        /// `mac_dictation_trigger`); ignored on Windows.
        mac_dictation_trigger: DictationTrigger,
        /// Last settings state reported by the AK. Defaults match firmware and
        /// is replaced by the board's authoritative values after each hello.
        device_volume: u8,
        device_brightness: u8,
        device_settings_synced: bool,
        /// FP-4b (route B): the window-focus + SendInput controller ("the
        /// hands") and the pure router that maps board events to control
        /// actions. This is the control path that supersedes route A.
        kc_controller: KiroCrewController,
        kc_router: ControlRouter,
        /// Enabled by default so board events immediately drive the local Kiro
        /// Crew window. When false, board key/nav frames only reflect trigger
        /// state in the INPUT view; every board-triggered PC action, including
        /// K4 / Win+H, is blocked until control is enabled again.
        control_enabled: bool,
        /// FP-4b STATUS ("the eyes"): the running localhost hook sink, if
        /// started. `status_sink_start` sets it; `status_sink_stop` / drop
        /// tears it down (releasing the port + removing the handoff file).
        status_sink: Option<StatusSink>,
        /// The sink's token, cached so `status_token()` can return it without
        /// re-reading the handoff file. Empty when no sink is running.
        status_token: String,
        /// Last mapped status per Kiro `session_id` (design: keep last-status
        /// per session, emit a `display` frame for it, push to the UI).
        kc_status: HashMap<String, StatusDisplay>,
        /// FP-4b Route STATUS — persisted "watched chat" filter. `None` = follow
        /// every session (only the UI live-list keys them apart). `Some(id)` = pin ONE
        /// Kiro `session_id`: only that session's status is forwarded as a
        /// `display` frame to the board, so a background chat's activity no
        /// longer clobbers the LCD. The `kc-status` UI event is ALWAYS pushed
        /// (pinned or not) so the picker can list every active session and the
        /// user can choose which one to pin.
        kc_pinned_session: Option<String>,
        /// FP-4b M4 (§3.2): per-key chat-number store for K1-K3, loaded from
        /// `~/.ak-companion/binds.json` at startup. Each key binds to a
        /// chat NUMBER (1..=9); pressing it fires `Alt+N` (jump to chat N). The
        /// numbers here are mirrored into `kc_router.ctx.jump_chats`.
        kc_binds: BindStore,
        /// Local Kiro Crew REST client. Authentication bootstraps from the
        /// gateway's rotating `.local_secret` before every operation.
        crew_api: Option<CrewApiClient>,
        /// Background `/api/ws` connection. Kept separate from the Hook sink:
        /// API status is primary, Hook remains an independent fallback.
        crew_realtime: Option<CrewRealtime>,
        crew_api_connected: bool,
        crew_api_base_url: String,
        crew_api_last_error: String,
        crew_api_slots: Vec<CrewSlot>,
        /// Stable Crew slot selected by K1/K2/K3 while API is connected.
        crew_api_active_slot: Option<String>,
        /// Review-before-send drafts keyed by stable Crew slot. ASR populates a
        /// draft; K5 submits it through REST, preserving the existing UX.
        crew_api_drafts: HashMap<String, String>,
    }

    struct AppState {
        inner: Mutex<Inner>,
        /// The board transport lives behind its OWN lock, separate from the
        /// `inner` state mutex. Serial `write_json` is a blocking IO call that
        /// can stall for seconds when the link is congested; keeping it out of
        /// the global `inner` lock means a slow/stuck write no longer freezes
        /// the UI, the watchdog, or any other command. The two locks are never
        /// held at the same time (see `write_board_guarded`).
        transport: Mutex<Option<Box<dyn BoardTransport>>>,
    }

    type Shared = Arc<AppState>;

    #[derive(Serialize)]
    struct Status {
        device_connected: bool,
        transport: TransportKind,
        endpoint: String,
        firmware_version: String,
        last_error: String,
        voice_source: String,
        device_volume: u8,
        device_brightness: u8,
        device_settings_synced: bool,
        control_enabled: bool,
        crew_api_connected: bool,
        crew_api_base_url: String,
        crew_api_last_error: String,
        crew_api_slots: usize,
        /// `std::env::consts::OS` of the host ("windows", "macos", …); lets the
        /// UI swap shortcut labels.
        platform: &'static str,
        /// `"granted"`, `"missing"` or `"not-required"` (Windows).
        input_permission: &'static str,
        /// Canonical wire name of the macOS dictation trigger.
        mac_dictation_trigger: String,
    }

    fn log_line(app: &AppHandle, message: impl Into<String>) {
        let message = message.into();
        if let Some(state) = app.try_state::<Shared>() {
            if let Ok(mut inner) = state.inner.lock() {
                inner.logs.insert(0, message.clone());
                inner.logs.truncate(200);
            }
        }
        let _ = app.emit("companion-log", message);
    }

    fn payload_kind(payload: &Value) -> &str {
        payload.get("type").and_then(Value::as_str).unwrap_or("unknown")
    }

    fn write_board_guarded(
        state: &Shared,
        payload: &Value,
        require_usb: bool,
    ) -> Result<(), String> {
        let kind = payload_kind(payload);
        let routine = matches!(kind, "ack" | "pong");
        let started = Instant::now();
        if !routine {
            diagnostics::record("DEBUG", "board-tx", &format!("begin type={kind}"));
        }
        // Read the connection kind under a SHORT hold of the inner lock, then
        // release it immediately — the blocking serial write below must never
        // run while holding the global state mutex.
        let kind_enum = {
            let inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            inner.kind
        };
        // Now take the dedicated transport lock for the (possibly slow) write.
        // A congested serial link stalls only this lock, not `inner`.
        let lock_started = Instant::now();
        let mut transport_guard = state
            .transport
            .lock()
            .map_err(|_| "transport lock poisoned")?;
        let lock_wait = lock_started.elapsed();
        if lock_wait >= Duration::from_millis(250) {
            diagnostics::record(
                "WARN",
                "board-tx",
                &format!("board write waited {}ms for transport lock type={kind}", lock_wait.as_millis()),
            );
        }
        if require_usb {
            crate::ensure_usb_provisioning_transport(transport_guard.is_some(), kind_enum)?;
        }
        let transport = transport_guard.as_mut().ok_or("device is not connected")?;
        let result = transport.write_json(payload);
        let elapsed = started.elapsed();
        let slow_or_failed = result.is_err() || elapsed >= Duration::from_millis(250);
        if !routine || slow_or_failed {
            diagnostics::record(
                if slow_or_failed { "WARN" } else { "DEBUG" },
                "board-tx",
                &format!(
                    "end type={kind} elapsed_ms={} result={}",
                    elapsed.as_millis(),
                    if result.is_ok() { "ok" } else { "error" }
                ),
            );
        }
        result
    }

    fn write_board(state: &Shared, payload: &Value) -> Result<(), String> {
        write_board_guarded(state, payload, false)
    }

    fn write_board_usb_only(state: &Shared, payload: &Value) -> Result<(), String> {
        write_board_guarded(state, payload, true)
    }

    fn spawn_state_watchdog(state: Shared) {
        let result = thread::Builder::new()
            .name("companion-watchdog".to_string())
            .spawn(move || {
                let mut busy_since: Option<Instant> = None;
                let mut last_report = Instant::now();
                loop {
                    thread::sleep(Duration::from_secs(1));
                    match state.inner.try_lock() {
                        Ok(_) => {
                            if let Some(started) = busy_since.take() {
                                diagnostics::record(
                                    "INFO",
                                    "state-lock",
                                    &format!("mutex recovered after {}ms", started.elapsed().as_millis()),
                                );
                            }
                        }
                        Err(TryLockError::WouldBlock) => {
                            let started = busy_since.get_or_insert_with(Instant::now);
                            if started.elapsed() >= Duration::from_secs(2)
                                && last_report.elapsed() >= Duration::from_secs(5)
                            {
                                diagnostics::record(
                                    "WARN",
                                    "state-lock",
                                    &format!("mutex busy for {}ms", started.elapsed().as_millis()),
                                );
                                last_report = Instant::now();
                            }
                        }
                        Err(TryLockError::Poisoned(_)) => {
                            diagnostics::record("ERROR", "state-lock", "mutex poisoned");
                            return;
                        }
                    }
                }
            });
        if let Err(error) = result {
            diagnostics::record(
                "ERROR",
                "watchdog",
                &format!("failed to start state watchdog: {error}"),
            );
        }
    }

    #[tauri::command]
    fn list_serial_ports() -> Result<Vec<SerialPortInfo>, String> {
        usb_serial::list_serial_ports()
    }

    /// Scan ~4s for BLE peripherals matching the board's advertisement
    /// (local name `AK-KB` or service `0x1910`) and return their id/name.
    #[tauri::command]
    fn scan_ble() -> Result<Vec<BleDeviceInfo>, String> {
        ble_gatt::scan()
    }

    #[tauri::command]
    fn get_logs(state: State<Shared>) -> Result<Vec<String>, String> {
        Ok(state.inner.lock().map_err(|_| "state lock poisoned")?.logs.clone())
    }

    #[tauri::command]
    fn clear_logs(state: State<Shared>) -> Result<(), String> {
        state.inner.lock().map_err(|_| "state lock poisoned")?.logs.clear();
        Ok(())
    }

    /// Enable/disable routing board key/nav frames to Kiro Crew control. Default
    /// is ON; while disabled the board only reports trigger state to the UI and
    /// never drives the local Kiro Crew window.
    #[tauri::command]
    fn set_control_enabled(app: AppHandle, state: State<Shared>, enabled: bool) -> Result<(), String> {
        state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .control_enabled = enabled;
        log_line(
            &app,
            format!("[KC-CTL] board control {}", if enabled { "ENABLED" } else { "disabled" }),
        );
        Ok(())
    }

    #[tauri::command]
    fn get_status(state: State<Shared>) -> Result<Status, String> {
        // Sync command => main thread. Refresh the keyboard-layout cache BEFORE
        // taking `transport` or `inner` (it only takes its own leaf lock).
        #[cfg(target_os = "macos")]
        {
            let _ = crate::mac_sys::refresh_layout_cache();
        }
        // Read connectedness from the transport lock first, then release it
        // before taking the inner lock (never hold both at once).
        let device_connected = state
            .transport
            .lock()
            .map(|t| t.is_some())
            .unwrap_or(false);
        let inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        Ok(Status {
            device_connected,
            transport: inner.kind,
            endpoint: inner.endpoint.clone(),
            firmware_version: inner.firmware.clone(),
            last_error: inner.last_error.clone(),
            voice_source: inner.voice_source.as_str().to_string(),
            device_volume: inner.device_volume,
            device_brightness: inner.device_brightness,
            device_settings_synced: inner.device_settings_synced,
            control_enabled: inner.control_enabled,
            crew_api_connected: inner.crew_api_connected,
            crew_api_base_url: inner.crew_api_base_url.clone(),
            crew_api_last_error: inner.crew_api_last_error.clone(),
            crew_api_slots: inner.crew_api_slots.len(),
            platform: std::env::consts::OS,
            input_permission: platform::input_permission_status(),
            mac_dictation_trigger: inner.mac_dictation_trigger.as_str().to_string(),
        })
    }

    /// macOS: show the system Accessibility prompt and open the Privacy pane,
    /// then report the resulting status (`"granted"` / `"missing"`). Windows
    /// needs no grant and returns `"not-required"`.
    #[tauri::command]
    fn request_input_permission() -> Result<String, String> {
        #[cfg(target_os = "macos")]
        {
            let _ = crate::mac_sys::request_input_permission();
            if let Some(url) = platform::accessibility_settings_url(HostOs::current()) {
                platform::open_url(url)?;
            }
            Ok(platform::input_permission_status().to_string())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok("not-required".to_string())
        }
    }

    /// Choose the macOS dictation trigger by wire name; persists it and returns
    /// the canonical value (unknown names become `edit-menu`).
    #[tauri::command]
    fn set_mac_dictation_trigger(state: State<Shared>, trigger: String) -> Result<String, String> {
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        inner.kc_binds.set_mac_dictation_trigger(&trigger);
        let parsed = DictationTrigger::parse(&inner.kc_binds.mac_dictation_trigger);
        inner.mac_dictation_trigger = parsed;
        inner.kc_binds.save()?;
        Ok(parsed.as_str().to_string())
    }

    /// Explicitly (re)start the Crew WebSocket worker. Startup invokes this
    /// automatically; the command is exposed for UI diagnostics/recovery.
    #[tauri::command]
    fn crew_api_start(app: AppHandle, state: State<Shared>) -> Result<(), String> {
        start_crew_realtime(&app, state.inner())
    }

    #[tauri::command]
    fn crew_api_stop(app: AppHandle, state: State<Shared>) -> Result<(), String> {
        let (worker, base_url) = {
            let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            inner.crew_api_connected = false;
            (inner.crew_realtime.take(), inner.crew_api_base_url.clone())
        };
        if let Some(mut worker) = worker {
            worker.stop();
        }
        let connection = CrewConnection {
            connected: false,
            base_url,
            error: "stopped by user".to_string(),
            retry_secs: 0,
        };
        let _ = app.emit("crew-api-connection", &connection);
        log_line(&app, "[CREW-API] stopped · Hook fallback remains active");
        Ok(())
    }

    #[tauri::command]
    fn crew_api_list_slots(app: AppHandle, state: State<Shared>) -> Result<Vec<CrewSlot>, String> {
        let client = state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .crew_api
            .clone()
            .ok_or("Kiro Crew API client is unavailable")?;
        let slots = client.list_slots()?;
        if let Ok(mut inner) = state.inner.lock() {
            inner.crew_api_slots = slots.clone();
            inner.crew_api_last_error.clear();
        }
        let _ = app.emit("crew-api-slots", &slots);
        Ok(slots)
    }

    #[tauri::command]
    fn crew_api_send_message(
        app: AppHandle,
        state: State<Shared>,
        slot: String,
        message: String,
    ) -> Result<SendReceipt, String> {
        let client = state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .crew_api
            .clone()
            .ok_or("Kiro Crew API client is unavailable")?;
        let receipt = client.send_message(&slot, &message)?;
        if let Ok(mut inner) = state.inner.lock() {
            inner.crew_api_drafts.remove(&slot);
            inner.crew_api_active_slot = Some(slot.clone());
        }
        emit_crew_draft(&app, &slot, "");
        log_line(
            &app,
            format!("[CREW-API] message accepted for slot {slot} (content withheld from logs)"),
        );
        Ok(receipt)
    }

    #[tauri::command]
    fn crew_api_slot_action(
        app: AppHandle,
        state: State<Shared>,
        slot: String,
        action: String,
    ) -> Result<(), String> {
        let client = state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .crew_api
            .clone()
            .ok_or("Kiro Crew API client is unavailable")?;
        match action.as_str() {
            "continue" => client.continue_slot(&slot)?,
            "interrupt" => client.interrupt_slot(&slot)?,
            "stop" => client.stop_slot(&slot)?,
            _ => return Err(format!("unsupported Kiro Crew slot action {action:?}")),
        }
        log_line(&app, format!("[CREW-API] {action} accepted for slot {slot}"));
        Ok(())
    }

    #[tauri::command]
    fn connect_device(
        app: AppHandle,
        state: State<Shared>,
        transport: Option<String>,
        port_name: Option<String>,
    ) -> Result<(), String> {
        let transport_kind = transport.as_deref().unwrap_or("usb");

        diagnostics::record(
            "INFO",
            "connect",
            &format!("begin transport={transport_kind}"),
        );
        let shared = state.inner();
        // Only one transport at a time. If something is already connected, do
        // NOT silently tear it down — tell the user to disconnect first. This
        // makes switching USB<->BLE an explicit two-step action and avoids
        // yanking a live link out from under the reader thread.
        {
            let already = shared
                .transport
                .lock()
                .map(|t| t.is_some())
                .unwrap_or(false);
            if already {
                let inner = shared.inner.lock().map_err(|_| "state lock poisoned")?;
                let current = match inner.kind {
                    TransportKind::Usb => "USB",
                    TransportKind::Ble => "BLE",
                    TransportKind::None => "a transport",
                };
                let want = if transport_kind == "ble" { "BLE" } else { "USB" };
                return Err(format!(
                    "Already connected over {current} ({}). Disconnect first before connecting over {want}.",
                    if inner.endpoint.is_empty() { "—" } else { inner.endpoint.as_str() }
                ));
            }
        }

        let (line_tx, line_rx) = mpsc::channel::<String>();
        // Build the selected backend. Both hand complete JSONL lines to the
        // same LineSender, so the reader thread below is transport-agnostic.
        let boxed: Box<dyn BoardTransport> = match transport_kind {
            "ble" => {
                let device_id = port_name
                    .clone()
                    .filter(|s| !s.is_empty())
                    .ok_or("BLE connect requires a device id (scan and pick one)")?;
                let transport =
                    BleGattTransport::connect(device_id, line_tx).map_err(|err| {
                        diagnostics::record("ERROR", "connect", &format!("BLE failed: {err}"));
                        log_line(&app, format!("[ERROR] ble transport: {err}"));
                        err
                    })?;
                Box::new(transport)
            }
            _ => {
                let transport =
                    UsbSerialTransport::connect(port_name.clone(), line_tx).map_err(|err| {
                        diagnostics::record("ERROR", "connect", &format!("USB failed: {err}"));
                        log_line(&app, format!("[ERROR] transport: {err}"));
                        err
                    })?;
                Box::new(transport)
            }
        };
        let kind = boxed.kind();
        let endpoint = boxed.endpoint();

        // Install the transport under its own lock first, then update the inner
        // metadata. The two locks are taken sequentially, never nested.
        {
            let mut transport_guard =
                shared.transport.lock().map_err(|_| "transport lock poisoned")?;
            *transport_guard = Some(boxed);
        }
        let connection_id = {
            let mut inner = shared.inner.lock().map_err(|_| "state lock poisoned")?;
            inner.kind = kind;
            inner.endpoint = endpoint.clone();
            inner.last_error.clear();
            inner.device_settings_synced = false;
            inner.connection_id = inner.connection_id.wrapping_add(1);
            inner.connection_id
        };

        // Start the reader BEFORE greeting so the board's hello_ack is not
        // missed. USB/UART is physically trusted and greets immediately; for
        // BLE the subscription is already confirmed live inside
        // `BleGattTransport::connect`, so it is likewise safe to greet now
        // (BLE, unlike USB, is not greet-immediately at the wire level — the
        // greeting deliberately waits until after notify-subscription).
        spawn_board_reader(app.clone(), shared.clone(), line_rx, connection_id);
        write_board(shared, &protocol::hello_payload())?;
        let (aliases, status_payloads) = {
            let inner = shared.inner.lock().map_err(|_| "state lock poisoned")?;
            let aliases = inner.kc_binds.aliases.clone();
            let status_payloads = (0..3)
                .map(|index| {
                    let session_id = inner
                        .kc_binds
                        .sessions
                        .get(index)
                        .map(|value| value.trim())
                        .unwrap_or("");
                    if let Some(status) = inner
                        .kc_status
                        .get(session_id)
                        .filter(|_| !session_id.is_empty())
                    {
                        protocol::display_payload(
                            index as u8 + 1,
                            &status.agent_name,
                            &status.state,
                            &status.text,
                        )
                    } else {
                        let alias = aliases.get(index).map(String::as_str).unwrap_or("");
                        protocol::display_payload(index as u8 + 1, alias, "unbound", "")
                    }
                })
                .collect::<Vec<_>>();
            (aliases, status_payloads)
        };
        if let Err(err) = write_board(shared, &protocol::binding_aliases_payload(&aliases)) {
            log_line(&app, format!("[WARN] binding alias sync failed: {err}"));
        }
        // Replay all three slots on every board link. Explicit unbound frames
        // clear stale colors for slots that have no current cached status.
        for payload in status_payloads {
            if let Err(err) = write_board(shared, &payload) {
                log_line(&app, format!("[WARN] status replay failed: {err}"));
            }
        }
        log_line(
            &app,
            format!("[INFO] connected transport={kind:?} endpoint={endpoint}"),
        );
        diagnostics::record("INFO", "connect", &format!("ready transport={kind:?}"));
        Ok(())
    }

    #[tauri::command]
    fn disconnect_device(state: State<Shared>) -> Result<(), String> {
        diagnostics::record("INFO", "disconnect", "begin");
        // Tear the transport down under its OWN lock (bye write + close are
        // blocking IO and must not hold the inner lock).
        {
            let mut transport_guard =
                state.transport.lock().map_err(|_| "transport lock poisoned")?;
            // Best-effort graceful goodbye BEFORE we drop the transport: plain
            // UART has no hardware disconnect signal, so without this the
            // board's link header would stay "USB Connected" forever.
            if let Some(transport) = transport_guard.as_mut() {
                let _ = transport.write_json(&protocol::bye_payload());
            }
            if let Some(mut transport) = transport_guard.take() {
                // Sets the transport stop flag so the low-level serial reader
                // thread exits and releases the OS port; dropping `transport`
                // closes the port handle. The app-level board reader then sees
                // the transport gone on its next liveness check and also exits.
                transport.disconnect();
            }
        }
        // Clear the connection metadata under the inner lock.
        {
            let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            inner.kind = TransportKind::None;
            inner.endpoint.clear();
            inner.device_settings_synced = false;
            inner.connection_id = inner.connection_id.wrapping_add(1);
        }
        diagnostics::record("INFO", "disconnect", "complete");
        Ok(())
    }

    #[tauri::command]
    fn send_display(
        state: State<Shared>,
        chat_slot: u8,
        agent_name: String,
        display_state: String,
        text: String,
    ) -> Result<(), String> {
        if !(1..=3).contains(&chat_slot) {
            return Err(format!("chat slot must be 1..=3 (got {chat_slot})"));
        }
        write_board(
            state.inner(),
            &protocol::display_payload(chat_slot, &agent_name, &display_state, &text),
        )
    }

    #[tauri::command]
    fn send_led(state: State<Shared>, led_state: String, color: String) -> Result<(), String> {
        write_board(state.inner(), &protocol::led_payload(&led_state, &color))
    }

    #[tauri::command]
    fn send_config(state: State<Shared>, voice_engine: String) -> Result<(), String> {
        write_board(state.inner(), &protocol::config_payload(&voice_engine))
    }

    /// Set both runtime device values. The AK applies the command and reports a
    /// `device_settings_state` frame; the UI updates from that authoritative echo.
    #[tauri::command]
    fn set_device_settings(
        state: State<Shared>,
        volume: u8,
        brightness: u8,
    ) -> Result<(), String> {
        if volume > 100 {
            return Err(format!("volume must be 0..=100 (got {volume})"));
        }
        if !(10..=100).contains(&brightness) {
            return Err(format!("brightness must be 10..=100 (got {brightness})"));
        }
        write_board(
            state.inner(),
            &protocol::device_settings_payload(volume, brightness),
        )
    }

    /// Provision Wi-Fi on the board: send SSID + password over the JSONL
    /// channel. The board applies it via netmgr and reports back a wifi_status
    /// frame (surfaced to the UI as a 'wifi-status' event).
    #[tauri::command]
    fn send_wifi(state: State<Shared>, ssid: String, pass: String) -> Result<(), String> {
        if ssid.trim().is_empty() {
            return Err("SSID is required".to_string());
        }
        write_board_usb_only(state.inner(), &protocol::wifi_payload(ssid.trim(), &pass))
    }

    /// Provision the device license (UUID + AuthKey) on the board. The board
    /// persists it via tuya_authorize_write and reports back a license_status
    /// frame (surfaced as a 'license-status' event). The secret only travels
    /// board-ward; it is never stored by the companion or echoed back.
    #[tauri::command]
    fn send_license(state: State<Shared>, uuid: String, authkey: String) -> Result<(), String> {
        if uuid.trim().is_empty() || authkey.trim().is_empty() {
            return Err("UUID and AuthKey are both required".to_string());
        }
        write_board_usb_only(
            state.inner(),
            &protocol::license_payload(uuid.trim(), authkey.trim()),
        )
    }

    /// Request development-safe License diagnostics. The board replies only on
    /// UART0 with the full UUID, AuthKey last four characters, and value source.
    #[tauri::command]
    fn read_license(state: State<Shared>) -> Result<(), String> {
        write_board_usb_only(state.inner(), &protocol::license_read_payload())
    }

    /// One-click "enter pairing": clear the board's Wi-Fi/cloud binding and
    /// reboot it into provisioning. The board's License (UUID/AuthKey) is
    /// preserved — only Wi-Fi credentials and cloud binding are cleared. This is
    /// destructive (the board reboots and the link drops), so it travels only
    /// over the physically trusted USB/UART transport; the UI must confirm first.
    #[tauri::command]
    fn enter_pairing(state: State<Shared>) -> Result<(), String> {
        write_board_usb_only(state.inner(), &protocol::reset_netcfg_payload())
    }

    /// FP-5: choose which ASR handles a board voice event. `cloud` = board mic +
    /// Tuya cloud ASR (FP-3); `system` = activate the PC's own dictation here.
    /// Mirrors the choice to the board via the config frame so the LCD can show it.
    #[tauri::command]
    fn set_voice_source(state: State<Shared>, source: String) -> Result<(), String> {
        let src = VoiceSource::parse(&source);
        {
            let shared = state.inner();
            let mut inner = shared.inner.lock().map_err(|_| "state lock poisoned")?;
            inner.voice_source = src;
        }
        // Best-effort mirror to the board (ignored if not connected).
        let _ = write_board(state.inner(), &protocol::config_payload(src.as_str()));
        Ok(())
    }

    // ───────────────────── FP-4b: Kiro Crew control (route B — "the hands") ─────────────────────

    /// route B: point the controller at a window whose title contains `title`
    /// (empty → default candidates "Kiro Crew" / "Agentic"). Verifies a match
    /// by focusing immediately, so the UI can show a bind result (AC1.5).
    #[tauri::command]
    fn kc_bind_window(state: State<Shared>, title: String) -> Result<(), String> {
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        inner.kc_controller.set_title_match(title);
        // Focus now to verify the window exists; bubbles up the "no match" Err.
        inner.kc_controller.focus()
    }

    /// route B: focus the bound window and type `text` (no command bar, no Enter).
    #[tauri::command]
    fn kc_send_text(state: State<Shared>, text: String) -> Result<(), String> {
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        inner.kc_controller.send_text(&text)
    }

    /// route B: focus, open the command bar (Ctrl+K), type `cmd`, press Enter.
    #[tauri::command]
    fn kc_command_bar(state: State<Shared>, cmd: String) -> Result<(), String> {
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        inner.kc_controller.command_bar(&cmd)
    }

    /// route B: focus and inject a single named chord
    /// (`enter`/`esc`/`up`/`down`/`left`/`right`/`ctrlk`).
    #[tauri::command]
    fn kc_chord(state: State<Shared>, name: String) -> Result<(), String> {
        let chord = Chord::parse(&name).ok_or_else(|| format!("unknown chord {name:?}"))?;
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        inner.kc_controller.chord(chord)
    }

    // ─────────────── FP-4b M4: per-key chat-number bind slots (§3.2) ───────────────

    /// M4: return the three per-key chat NUMBERS (index 0 = K1 … index 2 = K3)
    /// as a JSON array of `u8` for the UI.
    #[tauri::command]
    fn kc_bind_get(state: State<Shared>) -> Result<Vec<u8>, String> {
        Ok(state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .kc_binds
            .chats
            .clone())
    }

    /// Return optional display aliases for slots A, B, and C. Empty entries use
    /// the default letter on the AK.
    #[tauri::command]
    fn kc_bind_aliases_get(state: State<Shared>) -> Result<Vec<String>, String> {
        Ok(state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .kc_binds
            .aliases
            .clone())
    }

    /// Return the Crew session associated with each K1-K3 slot. Empty entries
    /// have not been quick-bound from Watched Chat yet.
    #[tauri::command]
    fn kc_bind_sessions_get(state: State<Shared>) -> Result<Vec<String>, String> {
        Ok(state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .kc_binds
            .sessions
            .clone())
    }

    /// Persist one slot alias and immediately synchronize all aliases to the AK
    /// when connected. Saving while offline still succeeds; connect_device sends
    /// the stored values on the next connection.
    #[tauri::command]
    fn kc_bind_alias_set(
        state: State<Shared>,
        slot: u8,
        alias: String,
    ) -> Result<Vec<String>, String> {
        let aliases = {
            let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            inner.kc_binds.set_alias(slot, &alias)?;
            inner.kc_binds.save()?;
            inner.kc_binds.aliases.clone()
        };
        let _ = write_board(state.inner(), &protocol::binding_aliases_payload(&aliases));
        Ok(aliases)
    }

    /// M4: bind slot `slot` (1-based) to chat `number` (`1..=9`). Validates the
    /// number, persists the store, and updates the LIVE router `ControlCtx` so
    /// the next board press jumps to the new number without a restart.
    #[tauri::command]
    fn kc_bind_set(state: State<Shared>, slot: u8, number: u8) -> Result<(), String> {
        // 0 = not set (no Alt+N jump); 1..=9 = jump to that chat. Reject >9.
        if number > 9 {
            return Err(format!("chat number must be 0..=9 (got {number})"));
        }
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        inner.kc_binds.set(slot, number);
        inner.kc_binds.save()?;
        // Keep the live router context in sync with the persisted numbers.
        inner.kc_router.ctx.jump_chats = inner.kc_binds.chats.clone();
        Ok(())
    }

    /// Quick-bind one observed Crew session and its real dashboard Chat # to a
    /// K1-K3 slot atomically. The session id is presentation metadata; the live
    /// router continues to inject Alt+`number` as before.
    #[tauri::command]
    fn kc_bind_session_set(
        state: State<Shared>,
        slot: u8,
        number: u8,
        session_id: String,
    ) -> Result<(), String> {
        // 0 = not set (no Alt+N jump); 1..=9 = jump to that chat. Reject >9.
        if number > 9 {
            return Err(format!("chat number must be 0..=9 (got {number})"));
        }
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        inner.kc_binds.set_session(slot, &session_id)?;
        inner.kc_binds.set(slot, number);
        inner.kc_binds.save()?;
        inner.kc_router.ctx.jump_chats = inner.kc_binds.chats.clone();
        Ok(())
    }

    /// Remove only a slot's Watched Chat association, preserving its Chat # and
    /// AK alias.
    #[tauri::command]
    fn kc_bind_session_clear(state: State<Shared>, slot: u8) -> Result<(), String> {
        let alias = {
            let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            inner.kc_binds.clear_session(slot)?;
            inner.kc_binds.save()?;
            // Alias for the now-unbound slot (empty falls back to A/B/C on AK).
            inner
                .kc_binds
                .aliases
                .get((slot as usize).saturating_sub(1))
                .cloned()
                .unwrap_or_default()
        };
        // Tell the board this slot is unbound so its sidebar icon goes purple.
        // Best-effort: ignored when the board is offline or slot is out of 1..=3.
        if (1..=3).contains(&slot) {
            let _ = write_board(
                state.inner(),
                &protocol::display_payload(slot, &alias, "unbound", ""),
            );
        }
        Ok(())
    }

    /// M4: reset slot `slot` (1-based) to its default chat number (slot index+1:
    /// K1->1, K2->2, K3->3), persist, and update the live router context.
    #[tauri::command]
    fn kc_bind_clear(state: State<Shared>, slot: u8) -> Result<(), String> {
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        inner.kc_binds.clear(slot);
        inner.kc_binds.save()?;
        inner.kc_router.ctx.jump_chats = inner.kc_binds.chats.clone();
        Ok(())
    }

    // ─────────────── FP-4b: STATUS sink ("the eyes") — T7 wiring ───────────────


    /// Apply one event from the Crew REST/WebSocket client. API status enters
    /// the same `apply_status` path as Hook events, so existing Watched Chat,
    /// Chat A/B/C bindings, UI events, and LCD forwarding continue to work.
    /// Hook remains live and can still update status whenever API is offline.
    fn apply_crew_event(app: &AppHandle, state: &Shared, event: CrewEvent) {
        match event {
            CrewEvent::Connection(connection) => {
                if let Ok(mut inner) = state.inner.lock() {
                    inner.crew_api_connected = connection.connected;
                    inner.crew_api_base_url = connection.base_url.clone();
                    inner.crew_api_last_error = connection.error.clone();
                    if connection.connected {
                        // Treat the first slots frame on every WebSocket
                        // connection as a full projection, even when its data
                        // matches the snapshot cached before reconnecting.
                        inner.crew_api_slots.clear();
                    }
                }
                let _ = app.emit("crew-api-connection", &connection);
                if connection.connected {
                    log_line(app, format!("[CREW-API] connected {}", connection.base_url));
                } else {
                    log_line(
                        app,
                        format!(
                            "[CREW-API] disconnected: {} · retry {}s · Hook fallback remains active",
                            connection.error, connection.retry_secs
                        ),
                    );
                }
            }
            CrewEvent::Slots(slots) => {
                // Only feed changed projections into apply_status; the gateway
                // periodically rebroadcasts the full list and replaying every
                // unchanged idle row would spam logs/LCD writes.
                let changed = if let Ok(mut inner) = state.inner.lock() {
                    let previous: HashMap<_, _> = inner
                        .crew_api_slots
                        .iter()
                        .map(|slot| (slot.key.as_str(), slot))
                        .collect();
                    let changed = slots
                        .iter()
                        .filter(|slot| previous.get(slot.key.as_str()).copied() != Some(slot))
                        .cloned()
                        .collect::<Vec<_>>();
                    inner.crew_api_slots = slots.clone();
                    changed
                } else {
                    Vec::new()
                };
                let _ = app.emit("crew-api-slots", &slots);
                for slot in changed {
                    apply_status(app, state, slot_display_status(&slot));
                }
            }
            CrewEvent::ChatMessage(data) => {
                let _ = app.emit("crew-api-message", data);
            }
            CrewEvent::Dashboard(data) => {
                let _ = app.emit("crew-api-dashboard", data);
            }
            CrewEvent::SlotTitle(data) => {
                let _ = app.emit("crew-api-slot-title", data);
            }
            CrewEvent::Other { event_type, data } => {
                if event_type == "decode_error" {
                    log_line(app, format!("[CREW-API] WebSocket decode warning: {data}"));
                }
            }
        }
    }

    fn start_crew_realtime(app: &AppHandle, state: &Shared) -> Result<(), String> {
        let client = {
            let inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            if inner.crew_realtime.is_some() {
                return Ok(());
            }
            inner
                .crew_api
                .clone()
                .ok_or_else(|| "Kiro Crew API client is unavailable".to_string())?
        };
        let app_cb = app.clone();
        let state_cb = state.clone();
        let worker = CrewRealtime::start(client, move |event| {
            apply_crew_event(&app_cb, &state_cb, event);
        })?;
        state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .crew_realtime = Some(worker);
        Ok(())
    }

    /// Apply one mapped status update coming from the hook sink (runs on the
    /// sink's listener thread, so it takes the shared state + app handle by
    /// value/clone). Keeps last-status per `session_id`, emits a `display` frame
    /// to the board (best-effort — ignored when the board is offline), and
    /// emits a `kc-status` UI event.
    fn apply_status(app: &AppHandle, state: &Shared, status: StatusDisplay) {
        // Record last-status per session, then resolve its persisted Chat A/B/C
        // destination under the same lock as the Watch filter.
        let (watch_allows, slots) = if let Ok(mut inner) = state.inner.lock() {
            inner
                .kc_status
                .insert(status.session_id.clone(), status.clone());
            let watch_allows = crate::status_sink::should_forward(
                inner.kc_pinned_session.as_deref(),
                &status.session_id,
            );
            let slots = inner.kc_binds.slots_for_session(&status.session_id);
            (watch_allows, slots)
        } else {
            // Without a trustworthy binding lookup, fail closed rather than
            // writing this status into whichever Chat is currently selected.
            (false, Vec::new())
        };
        // One Crew session may intentionally be assigned to multiple slots.
        // Unbound sessions remain visible in the Companion but never mutate AK.
        if watch_allows {
            for chat_slot in &slots {
                let _ = write_board(
                    state,
                    &protocol::display_payload(
                        *chat_slot,
                        &status.agent_name,
                        &status.state,
                        &status.text,
                    ),
                );
            }
        }
        let forwarded = watch_allows && !slots.is_empty();
        // Push to the UI live status list ALWAYS (pinned or not) so the picker
        // can list every active session; carry `forwarded` so the UI can mark
        // which session is currently driving the board.
        let _ = app.emit(
            "kc-status",
            serde_json::json!({
                "session_id": status.session_id,
                "agent_name": status.agent_name,
                "state": status.state,
                "text": status.text,
                "event": status.event,
                "detail_text": status.detail_text,
                "forwarded": forwarded,
            }),
        );
        log_line(
            app,
            format!(
                "[KC-STATUS] {} · {} · {}{}",
                status.session_id,
                status.state,
                status.text,
                if !watch_allows {
                    " (muted — not watched)"
                } else if slots.is_empty() {
                    " (not sent — no Chat A/B/C binding)"
                } else {
                    ""
                }
            ),
        );
    }

    /// Start the localhost status sink on `port` (default when omitted). Wires a
    /// callback that forwards each mapped status through [`apply_status`]. The
    /// sink generates a token and writes the `{port, token}` handoff file the
    /// `--hook` mode reads. Returns the token so the UI can show the exact hook
    /// command to paste.
    #[tauri::command]
    fn status_sink_start(
        app: AppHandle,
        state: State<Shared>,
        port: Option<u16>,
    ) -> Result<String, String> {
        let port = port.unwrap_or(DEFAULT_STATUS_PORT);
        // Tear down any existing sink first (start is idempotent-ish).
        {
            let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            if let Some(mut existing) = inner.status_sink.take() {
                existing.stop();
            }
        }
        let app_cb = app.clone();
        let shared_cb = state.inner().clone();
        let sink = StatusSink::start(port, move |status| {
            apply_status(&app_cb, &shared_cb, status);
        })?;
        let token = sink.token().to_string();
        let bound = sink.port();
        {
            let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            inner.status_sink = Some(sink);
            inner.status_token = token.clone();
        }
        log_line(&app, format!("[KC-STATUS] sink listening on 127.0.0.1:{bound}"));
        Ok(token)
    }

    /// Stop the status sink (releases the port, removes the handoff file).
    #[tauri::command]
    fn status_sink_stop(app: AppHandle, state: State<Shared>) -> Result<(), String> {
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        if let Some(mut sink) = inner.status_sink.take() {
            sink.stop();
        }
        inner.status_token.clear();
        drop(inner);
        log_line(&app, "[KC-STATUS] sink stopped");
        Ok(())
    }

    /// Return the current sink token (empty when no sink is running).
    #[tauri::command]
    fn status_token(state: State<Shared>) -> Result<String, String> {
        Ok(state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .status_token
            .clone())
    }

    // ─────────── FP-4b Route STATUS — "watched chat" pin (option A) ───────────

    /// One row for the UI's active-session picker: a session the sink has seen,
    /// with its last-known status and whether it is the one currently pinned.
    #[derive(Serialize)]
    struct SessionRow {
        session_id: String,
        agent_name: String,
        state: String,
        text: String,
        event: String,
        detail_text: String,
        pinned: bool,
    }

    /// List every session the sink has observed (with last status), so the UI
    /// can render a picker. The `pinned` flag marks which one is driving the
    /// board. When nothing is pinned, ALL sessions drive the board (and every
    /// row's `pinned` is false).
    #[tauri::command]
    fn kc_list_sessions(state: State<Shared>) -> Result<Vec<SessionRow>, String> {
        let inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        let pinned = inner.kc_pinned_session.clone();
        let mut rows: Vec<SessionRow> = inner
            .kc_status
            .values()
            .map(|s| SessionRow {
                session_id: s.session_id.clone(),
                agent_name: s.agent_name.clone(),
                state: s.state.clone(),
                text: s.text.clone(),
                event: s.event.clone(),
                detail_text: s.detail_text.clone(),
                pinned: pinned.as_deref() == Some(s.session_id.as_str()),
            })
            .collect();
        // Stable order: pinned first, then by session id.
        rows.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(a.session_id.cmp(&b.session_id)));
        Ok(rows)
    }

    /// Return the currently pinned session id, or empty string when following
    /// all sessions.
    #[tauri::command]
    fn kc_get_pinned(state: State<Shared>) -> Result<String, String> {
        Ok(state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned")?
            .kc_pinned_session
            .clone()
            .unwrap_or_default())
    }

    /// Pin ONE session as the watched chat: only its status is forwarded to the
    /// board LCD. Immediately pushes that session's last-known status (if any)
    /// to the board so the LCD reflects the choice without waiting for the next
    /// hook event. An empty / whitespace id is treated as unpin.
    #[tauri::command]
    fn kc_pin_session(app: AppHandle, state: State<Shared>, session_id: String) -> Result<(), String> {
        let sid = session_id.trim().to_string();
        if sid.is_empty() {
            return kc_unpin_session(app, state);
        }
        // Persist the pin and resolve the pinned session's bound AK slots under
        // one lock, together with its last known status.
        let (last, slots) = {
            let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
            let mut next_binds = inner.kc_binds.clone();
            next_binds.set_pinned_session(&sid)?;
            next_binds.save()?;
            let slots = next_binds.slots_for_session(&sid);
            inner.kc_binds = next_binds;
            inner.kc_pinned_session = Some(sid.clone());
            (inner.kc_status.get(&sid).cloned(), slots)
        };
        // Refresh only the pinned session's explicitly bound Chat slot(s).
        if let Some(s) = last {
            for chat_slot in slots {
                let _ = write_board(
                    &state,
                    &protocol::display_payload(chat_slot, &s.agent_name, &s.state, &s.text),
                );
            }
        }
        log_line(&app, format!("[KC-STATUS] pinned session {sid}"));
        Ok(())
    }

    /// Clear the pin: go back to following every session (all statuses drive the
    /// board again).
    #[tauri::command]
    fn kc_unpin_session(app: AppHandle, state: State<Shared>) -> Result<(), String> {
        let mut inner = state.inner.lock().map_err(|_| "state lock poisoned")?;
        let mut next_binds = inner.kc_binds.clone();
        next_binds.set_pinned_session("")?;
        next_binds.save()?;
        inner.kc_binds = next_binds;
        inner.kc_pinned_session = None;
        drop(inner);
        log_line(&app, "[KC-STATUS] unpinned — following all sessions");
        Ok(())
    }

    // ─────── FP-4b Route STATUS — one-click global hook install/remove ───────

    /// Resolve the hook command: the UI may pass an explicit companion exe path
    /// (e.g. the user browsed to `companion.exe`); empty falls back to THIS
    /// running executable.
    fn resolve_hook_command(exe: Option<String>) -> String {
        let exe = exe
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(hook_install::current_exe_path);
        hook_install::hook_command(&exe)
    }

    /// Report global Crew ScriptHookStore status without modifying anything.
    #[tauri::command]
    fn kc_hook_status(exe: Option<String>) -> Result<hook_install::HookStatus, String> {
        hook_install::status(&resolve_hook_command(exe))
    }

    /// Install four lifecycle commands into Crew's shared hooks.json store.
    /// Crew must be restarted after an external file update so its in-memory
    /// ScriptHookStore reloads the entries.
    #[tauri::command]
    fn kc_hook_install(
        app: AppHandle,
        exe: Option<String>,
    ) -> Result<hook_install::HookStatus, String> {
        let command = resolve_hook_command(exe);
        let status = hook_install::install(&command)?;
        log_line(
            &app,
            format!("[KC-HOOK] global hook installed · {}", status.hook_file),
        );
        Ok(status)
    }

    /// Remove only AK Companion entries from Crew's shared hooks.json store.
    #[tauri::command]
    fn kc_hook_remove(
        app: AppHandle,
        exe: Option<String>,
    ) -> Result<hook_install::HookStatus, String> {
        let status = hook_install::remove(&resolve_hook_command(exe))?;
        log_line(
            &app,
            format!("[KC-HOOK] global hook removed · {}", status.hook_file),
        );
        Ok(status)
    }

    /// Open the Crew configuration directory containing hooks.json in the
    /// platform file manager (Explorer / Finder).
    #[tauri::command]
    fn kc_open_hooks_folder() -> Result<String, String> {
        let dir = hook_install::hooks_dir().ok_or("cannot resolve Kiro Crew configuration directory")?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("create {}: {e}", dir.display()))?;
        let os = HostOs::current();
        let (program, args) = platform::reveal_dir_command(os, &dir)
            .ok_or("opening a folder is not supported on this platform")?;
        std::process::Command::new(program)
            .args(args)
            .spawn()
            .map_err(|e| {
                format!("open {} in {}: {e}", dir.display(), platform::file_manager_name(os))
            })?;
        Ok(dir.display().to_string())
    }

    /// Compact human-readable description of a routed control action for logs.
    fn describe_control_action(action: &ControlAction) -> String {
        match action {
            ControlAction::CommandBar(cmd) => format!("command-bar {cmd:?}"),
            ControlAction::Chord(c) => format!("chord {c:?}"),
            ControlAction::SendText(t) => format!("send-text {t:?}"),
            ControlAction::Compose(t) => format!("compose {t:?}"),
            ControlAction::StartDeleteWords => "start-delete-words".to_string(),
            ControlAction::StopDeleteWords => "stop-delete-words".to_string(),
            ControlAction::None => "none".to_string(),
        }
    }

    /// Execute a routed control action against the controller facade. All
    /// actions (including K1-K3 `Chord(JumpChat(n))`) go straight through
    /// `apply()`; the router already resolved the per-key chat number, so there
    /// is no stateful slot special-casing left here.
    fn exec_control_action(
        _app: &AppHandle,
        state: &Shared,
        action: &ControlAction,
    ) -> Result<(), String> {
        state
            .inner
            .lock()
            .map_err(|_| "state lock poisoned".to_string())
            .and_then(|mut inner| inner.kc_controller.apply(action))
    }

    fn spawn_board_reader(
        app: AppHandle,
        state: Shared,
        line_rx: Receiver<String>,
        connection_id: u64,
    ) {
        thread::spawn(move || {
            diagnostics::record(
                "INFO",
                "board-reader",
                &format!("started connection_id={connection_id}"),
            );
            while let Ok(line) = line_rx.recv() {
                // Stop if this connection has been superseded. Check the
                // transport presence and the connection id under their own
                // locks (taken one at a time, never nested).
                let transport_present = state
                    .transport
                    .lock()
                    .map(|t| t.is_some())
                    .unwrap_or(false);
                let id_matches = state
                    .inner
                    .lock()
                    .map(|inner| inner.connection_id == connection_id)
                    .unwrap_or(false);
                let live = transport_present && id_matches;
                if !live {
                    diagnostics::record("INFO", "board-reader", "stopped: connection superseded");
                    break;
                }
                if let Some(frame) = protocol::decode_line(&line) {
                    handle_frame(&app, &state, frame);
                } else {
                    diagnostics::record(
                        "WARN",
                        "board-reader",
                        &format!("discarded undecodable line bytes={}", line.len()),
                    );
                }
            }
            diagnostics::record("INFO", "board-reader", "channel closed");
        });
    }

    fn emit_crew_draft(app: &AppHandle, slot: &str, text: &str) {
        let _ = app.emit(
            "crew-api-draft",
            serde_json::json!({ "slot": slot, "text": text }),
        );
    }

    /// API-first board control. Returns true when the frame was consumed by the
    /// stable-slot REST path. False means the caller should use the legacy
    /// focus/SendInput route as fallback.
    fn route_board_via_crew_api(app: &AppHandle, state: &Shared, frame: &Inbound) -> bool {
        let api_ready = state
            .inner
            .lock()
            .map(|inner| inner.control_enabled && inner.crew_api_connected)
            .unwrap_or(false);
        if !api_ready {
            return false;
        }

        match frame {
            Inbound::Key { payload, .. }
                if matches!(payload.id.as_str(), "k1" | "k2" | "k3") =>
            {
                let index = match payload.id.as_str() {
                    "k1" => 0,
                    "k2" => 1,
                    _ => 2,
                };
                // Select the stable slot ONLY on Click (a confirmed single
                // click). Down/Up/Long/Dclick do not select or jump here, so a
                // long-press never jumps — it is reserved for the UI long-press
                // auto-bind (delivered independently via the `board-event`).
                if payload.action != KeyAction::Click {
                    return false;
                }
                // Resolve the stable slot and its optional Alt+N number under
                // one lock, then release before any window injection.
                let selected = state.inner.lock().ok().and_then(|mut inner| {
                    let key = inner.kc_binds.sessions.get(index)?.trim().to_string();
                    let is_api_slot = !key.is_empty()
                        && inner.crew_api_slots.iter().any(|slot| slot.key == key);
                    if !is_api_slot {
                        return None;
                    }
                    inner.crew_api_active_slot = Some(key.clone());
                    // Optional Alt+N jump number for this slot (0 = not set).
                    let jump = inner.kc_binds.chats.get(index).copied().unwrap_or(0);
                    Some((key, jump))
                });
                if let Some((slot, jump)) = selected {
                    let _ = app.emit("crew-api-active-slot", slot.clone());
                    log_line(app, format!("[CREW-API] selected stable slot {slot}"));
                    // Bonus: if the user set an Alt+N for this slot, ALSO jump the
                    // Kiro Crew view there (focus the window + inject Alt+N). When
                    // unset (0) nothing is injected — selection only.
                    if (1..=9).contains(&jump) {
                        let action = ControlAction::Chord(Chord::JumpChat(jump));
                        let label = platform::jump_chat_label(HostOs::current(), jump);
                        match exec_control_action(app, state, &action) {
                            Ok(()) => log_line(app, format!("[CREW-API] jumped Kiro Crew view to {label}")),
                            Err(e) => log_line(app, format!("[CREW-API] {label} jump failed: {e}")),
                        }
                        // exec_control_action has returned, so `inner` is no
                        // longer held. Fire-and-forget: queue a layout-cache
                        // refresh on the main thread; nothing waits on it.
                        #[cfg(target_os = "macos")]
                        let _ = app.run_on_main_thread(|| {
                            let _ = crate::mac_sys::refresh_layout_cache();
                        });
                    }
                    return true;
                }
                false
            }
            // ASR/macro text, K5, and composer edits use the same live
            // composer path. Fill-back types without sending; K5 injects Enter.
            Inbound::Asr { .. } | Inbound::Macro { .. } => false,
            Inbound::Key { payload, .. } if payload.id == "k5" => false,
            // Let the live composer router handle Backspace, word deletion,
            // and clear so board edits affect the text the user can see.
            Inbound::Nav { .. } => false,
            _ => false,
        }
    }

    // Kept for REST draft UI compatibility. Board fill-back and editing
    // intentionally bypass this store for the live composer.
    #[allow(dead_code)]
    fn append_crew_draft(app: &AppHandle, state: &Shared, text: &str) -> bool {
        if text.is_empty() {
            return true;
        }
        let updated = state.inner.lock().ok().and_then(|mut inner| {
            let slot = inner.crew_api_active_slot.clone()?;
            let draft = inner.crew_api_drafts.entry(slot.clone()).or_default();
            if !draft.is_empty() && !draft.ends_with(char::is_whitespace) {
                draft.push(' ');
            }
            draft.push_str(text);
            Some((slot, draft.clone()))
        });
        if let Some((slot, draft)) = updated {
            emit_crew_draft(app, &slot, &draft);
            log_line(
                app,
                format!("[CREW-API] draft updated for {slot} (content withheld from logs)"),
            );
            true
        } else {
            false
        }
    }

    #[allow(dead_code)]
    enum DraftEdit {
        Backspace,
        DeleteWord,
        Clear,
    }

    #[allow(dead_code)]
    fn edit_crew_draft(app: &AppHandle, state: &Shared, edit: DraftEdit) -> bool {
        let updated = state.inner.lock().ok().and_then(|mut inner| {
            let slot = inner.crew_api_active_slot.clone()?;
            let draft = inner.crew_api_drafts.entry(slot.clone()).or_default();
            match edit {
                DraftEdit::Backspace => {
                    draft.pop();
                }
                DraftEdit::DeleteWord => {
                    let trimmed = draft.trim_end().to_string();
                    let keep = trimmed
                        .char_indices()
                        .rev()
                        .find(|(_, ch)| ch.is_whitespace())
                        .map(|(index, _)| index)
                        .unwrap_or(0);
                    draft.truncate(keep);
                }
                DraftEdit::Clear => draft.clear(),
            }
            Some((slot, draft.clone()))
        });
        if let Some((slot, draft)) = updated {
            emit_crew_draft(app, &slot, &draft);
            true
        } else {
            false
        }
    }

    fn handle_frame(app: &AppHandle, state: &Shared, frame: Inbound) {
        // Housekeeping frames the reader answers itself.
        match &frame {
            Inbound::Ping => {
                let _ = write_board(state, &protocol::pong_payload());
            }
            Inbound::HelloAck(ack) => {
                if let Ok(mut inner) = state.inner.lock() {
                    inner.firmware = ack.fw.clone();
                }
            }
            Inbound::SystemInfo { payload, .. } => {
                if let Ok(mut inner) = state.inner.lock() {
                    inner.firmware = payload.fw.clone();
                }
            }
            _ => {}
        }
        // Acknowledge sequenced board events.
        if let Inbound::Key { seq: Some(seq), .. }
        | Inbound::Nav { seq: Some(seq), .. }
        | Inbound::Voice { seq: Some(seq), .. }
        | Inbound::Macro { seq: Some(seq), .. }
        | Inbound::Asr { seq: Some(seq), .. }
        | Inbound::ActivationStatus { seq: Some(seq), .. }
        | Inbound::LicenseInfo { seq: Some(seq), .. }
        | Inbound::SystemInfo { seq: Some(seq), .. }
        | Inbound::DeviceSettings { seq: Some(seq), .. } = &frame
        {
            let _ = write_board(state, &protocol::ack_payload(*seq));
        }

        // The board reports its current voice routing (cloud/system) on change
        // and on hello — mirror it into companion state and push to the UI so
        // the selector reflects a board-side switch.
        if let Inbound::VoiceSource { payload, .. } = &frame {
            let src = VoiceSource::parse(&payload.source);
            if let Ok(mut inner) = state.inner.lock() {
                inner.voice_source = src;
            }
            let _ = app.emit("voice-source", src.as_str());
            log_line(app, format!("[VOICE] board voice source = {}", src.as_str()));
        }

        // The board reports Wi-Fi provisioning progress -> push to the UI so the
        // WiFi config panel shows connecting/connected/failed.
        if let Inbound::WifiStatus { payload, .. } = &frame {
            let _ = app.emit("wifi-status", serde_json::json!({
                "state": payload.state, "ssid": payload.ssid
            }));
            log_line(app, format!("[WIFI] {} {}", payload.state, payload.ssid));
        }

        // Tuya activation lifecycle -> collapsible Device-panel activation card.
        // qr_url carries only PID+UUID and is empty outside the scan step.
        if let Inbound::ActivationStatus { payload, .. } = &frame {
            let _ = app.emit("activation-status", payload.clone());
            log_line(
                app,
                format!(
                    "[ACTIVATION] {} qr={}",
                    payload.state,
                    if payload.qr_url.is_empty() { "no" } else { "yes" }
                ),
            );
        }

        // Device-license provisioning result -> UI (no secret is carried).
        if let Inbound::LicenseStatus { payload, .. } = &frame {
            let _ = app.emit("license-status", payload.state.clone());
            log_line(app, format!("[LICENSE] {}", payload.state));
        }

        // Explicit USB-only diagnostic: UUID is visible, AuthKey remains masked.
        if let Inbound::LicenseInfo { payload, .. } = &frame {
            let _ = app.emit("license-info", payload.clone());
            log_line(
                app,
                format!(
                    "[LICENSE] source={} uuid={} authkey=****{}",
                    payload.source, payload.uuid, payload.authkey_tail
                ),
            );
        }

        // AK owns the applied runtime values. Mirror every hello/local/remote
        // settings report into backend state and the Device view.
        if let Inbound::DeviceSettings { payload, .. } = &frame {
            if let Ok(mut inner) = state.inner.lock() {
                inner.device_volume = payload.volume;
                inner.device_brightness = payload.brightness;
                inner.device_settings_synced = true;
            }
            let _ = app.emit("device-settings", payload.clone());
            log_line(
                app,
                format!(
                    "[DEVICE] volume={} brightness={}",
                    payload.volume, payload.brightness
                ),
            );
        }

        // FP-5: on a voice-start edge, if the active source is the PC's own ASR,
        // activate OS dictation (Win+H). Board-mic (cloud) source is a no-op here.
        // GATED on `control_enabled` (same gate as route B): while control is
        // disabled the K4 voice frame only reflects trigger state in the UI and
        // NEVER activates Win+H — no board frame drives the PC/Kiro Crew.
        if let Inbound::Voice { payload, .. } = &frame {
            let (enabled, source) = state
                .inner
                .lock()
                .map(|inner| (inner.control_enabled, inner.voice_source))
                .unwrap_or((false, VoiceSource::Cloud));
            if enabled && pc_asr::route_voice(source, payload.state) == VoiceAction::Activate {
                // Bring the Kiro Crew WINDOW to the foreground so OS dictation
                // types into it. Do NOT inject Alt+Enter here: that chord SUBMITS
                // the composer in Kiro Crew, so using it to "focus" auto-sent the
                // text left from the previous dictation (confirmed off-by-one on
                // hardware — each K4 press submitted the prior utterance). Window
                // focus alone is enough (the composer is the default focused
                // control) and can never submit. Best-effort; Win+H still fires if
                // focus fails (the user may already be on the window).
                let (trigger, pid) = match state.inner.lock() {
                    Ok(mut inner) => {
                        let _ = inner.kc_controller.focus();
                        (inner.mac_dictation_trigger, inner.kc_controller.target_pid())
                    }
                    Err(_) => (DictationTrigger::default(), None),
                };
                match pc_asr::activator(trigger, pid).activate() {
                    Ok(()) => log_line(
                        app,
                        format!(
                            "[PC-ASR] focused window + activated OS dictation ({})",
                            pc_asr::activation_label(trigger)
                        ),
                    ),
                    Err(e) => log_line(app, format!("[PC-ASR] activate failed: {e}")),
                }
            }
        }

        // Primary route: stable Crew slot REST control. When API is connected
        // and Chat A/B/C has an API slot binding, consume the frame without
        // focusing a window. Otherwise preserve the legacy SendInput behavior.
        let api_consumed = route_board_via_crew_api(app, state, &frame);
        if !api_consumed {
            let (enabled, action) = state
                .inner
                .lock()
                .ok()
                .map(|inner| (inner.control_enabled, inner.kc_router.route(&frame)))
                .unwrap_or((false, ControlAction::None));
            if enabled && action != ControlAction::None {
                let desc = describe_control_action(&action);
                let result = exec_control_action(app, state, &action);
                match result {
                    Ok(()) => log_line(app, format!("[KC-CTL] fallback {desc}")),
                    Err(e) => log_line(app, format!("[KC-CTL] fallback {desc} failed: {e}")),
                }
                // exec_control_action has returned, so `inner` is no longer
                // held. Fire-and-forget: queue a layout-cache refresh on the
                // main thread; nothing waits on it (the drain thread must
                // never wait for the main thread).
                #[cfg(target_os = "macos")]
                let _ = app.run_on_main_thread(|| {
                    let _ = crate::mac_sys::refresh_layout_cache();
                });
            }
        }

        log_line(app, format!("[RX] {}", describe_inbound(&frame)));
        let _ = app.emit("board-event", inbound_to_json(&frame));
    }

    pub fn run() {
        // Load the persisted per-key chat numbers once, and seed both the store
        // and the router's ControlCtx from them so the first board press already
        // jumps to the configured numbers.
        let kc_binds = BindStore::load();
        let mac_dictation_trigger = DictationTrigger::parse(&kc_binds.mac_dictation_trigger);
        let jump_chats = kc_binds.chats.clone();
        let kc_pinned_session = (!kc_binds.pinned_session.is_empty())
            .then(|| kc_binds.pinned_session.clone());
        let crew_api = match CrewApiClient::from_env() {
            Ok(client) => Some(client),
            Err(error) => {
                diagnostics::record("ERROR", "crew-api", &format!("client init failed: {error}"));
                None
            }
        };
        let crew_api_base_url = crew_api
            .as_ref()
            .map(|client| client.config().base_url())
            .unwrap_or_default();
        let state: Shared = Arc::new(AppState {
            transport: Mutex::new(None),
            inner: Mutex::new(Inner {
                kind: TransportKind::None,
                endpoint: String::new(),
                firmware: String::new(),
                connection_id: 0,
                logs: Vec::new(),
                last_error: String::new(),
                voice_source: VoiceSource::default(),
                mac_dictation_trigger,
                device_volume: 70,
                device_brightness: 100,
                device_settings_synced: false,
                kc_controller: KiroCrewController::new(None),
                kc_router: ControlRouter::new(crate::kirocrew_control::ControlCtx {
                    jump_chats,
                }),
                control_enabled: true,
                status_sink: None,
                status_token: String::new(),
                kc_status: HashMap::new(),
                kc_pinned_session,
                kc_binds,
                crew_api,
                crew_realtime: None,
                crew_api_connected: false,
                crew_api_base_url,
                crew_api_last_error: String::new(),
                crew_api_slots: Vec::new(),
                crew_api_active_slot: None,
                crew_api_drafts: HashMap::new(),
            }),
        });
        spawn_state_watchdog(state.clone());
        diagnostics::record("INFO", "lifecycle", "Tauri state initialized");

        tauri::Builder::default()
            .manage(state)
            .setup(|app| {
                let app_handle = app.handle().clone();
                let shared = app.state::<Shared>().inner().clone();
                // macOS: `setup` runs on the main thread, so warm the keyboard
                // layout cache before the first board event. No permission
                // prompt here; the UI banner offers a button instead.
                #[cfg(target_os = "macos")]
                {
                    let _ = crate::mac_sys::refresh_layout_cache();
                    if !crate::mac_sys::input_permission_granted() {
                        log_line(
                            &app_handle,
                            "[MAC] Accessibility permission missing — window focus and key injection are disabled until it is granted (use \"Grant Accessibility\" in the Companion window)",
                        );
                    }
                }
                if let Err(error) = start_crew_realtime(&app_handle, &shared) {
                    if let Ok(mut inner) = shared.inner.lock() {
                        inner.crew_api_last_error = error.clone();
                    }
                    log_line(
                        &app_handle,
                        format!("[CREW-API] startup unavailable: {error} · Hook fallback remains active"),
                    );
                }
                Ok(())
            })
            .invoke_handler(tauri::generate_handler![
                list_serial_ports,
                scan_ble,
                get_logs,
                clear_logs,
                set_control_enabled,
                get_status,
                request_input_permission,
                set_mac_dictation_trigger,
                crew_api_start,
                crew_api_stop,
                crew_api_list_slots,
                crew_api_send_message,
                crew_api_slot_action,
                connect_device,
                disconnect_device,
                send_display,
                send_led,
                send_config,
                set_device_settings,
                send_wifi,
                send_license,
                read_license,
                enter_pairing,
                set_voice_source,
                kc_bind_window,
                kc_send_text,
                kc_command_bar,
                kc_chord,
                kc_bind_get,
                kc_bind_aliases_get,
                kc_bind_sessions_get,
                kc_bind_alias_set,
                kc_bind_set,
                kc_bind_session_set,
                kc_bind_session_clear,
                kc_bind_clear,
                status_sink_start,
                status_sink_stop,
                status_token,
                kc_list_sessions,
                kc_get_pinned,
                kc_pin_session,
                kc_unpin_session,
                kc_hook_status,
                kc_hook_install,
                kc_hook_remove,
                kc_open_hooks_folder,
            ])
            .run(tauri::generate_context!())
            .expect("error while running tauri application");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{KeyAction, KeyPayload, NavDir, NavPayload, VoicePayload, VoiceState};

    #[test]
    fn describe_key_and_nav() {
        let key = Inbound::Key {
            seq: Some(7),
            payload: KeyPayload {
                id: "k2".into(),
                action: KeyAction::Long,
                held_ms: 820,
            },
        };
        assert_eq!(describe_inbound(&key), "key k2 Long (820ms) seq=7");

        let nav = Inbound::Nav {
            seq: None,
            payload: NavPayload {
                dir: NavDir::Center,
                action: KeyAction::Down,
                held_ms: 0,
            },
        };
        assert_eq!(describe_inbound(&nav), "nav Center Down (0ms)");
    }

    #[test]
    fn inbound_json_is_flat_and_typed() {
        let voice = Inbound::Voice {
            seq: Some(3),
            payload: VoicePayload {
                state: VoiceState::Start,
            },
        };
        let json = inbound_to_json(&voice);
        assert_eq!(json["kind"], "voice");
        assert_eq!(json["seq"], 3);
        assert_eq!(json["state"], "start");
    }

    #[test]
    fn provisioning_requires_connected_usb_transport() {
        assert!(ensure_usb_provisioning_transport(true, transport::TransportKind::Usb).is_ok());

        for (connected, kind) in [
            (false, transport::TransportKind::None),
            (false, transport::TransportKind::Usb),
            (true, transport::TransportKind::Ble),
        ] {
            assert_eq!(
                ensure_usb_provisioning_transport(connected, kind).unwrap_err(),
                "WiFi and UUID/AuthKey settings require an active USB connection"
            );
        }
    }

    #[test]
    fn unknown_frame_describes_and_serializes() {
        let frame = Inbound::Unknown {
            msg_type: "weird".into(),
        };
        assert_eq!(describe_inbound(&frame), "unknown type=weird");
        assert_eq!(inbound_to_json(&frame)["type"], "weird");
    }
}
