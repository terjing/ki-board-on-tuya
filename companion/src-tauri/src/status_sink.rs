//! FP-4b Route STATUS — the *eyes*: ingest local Kiro Crew agent-hook events and
//! turn them into board `display` frames.
//!
//! Two layers live here, both **cross-platform** so `cargo check` / `cargo test`
//! build them on the Linux CI host (only the callback wiring that actually emits
//! a `display` frame — which needs the Tauri transport — is gated on
//! `target_os = "macos"` in `lib.rs`):
//!
//! * **Pure mapping (T5).** [`HookEvent`] is the slimmed hook payload the
//!   companion's `--hook` mode POSTs, and [`map_hook_event`] maps it to a
//!   [`DisplayState`] (agent_name / state / text). No IO — unit-tested below.
//!
//! * **Server (T6).** [`StatusSink`] binds `127.0.0.1:<port>` on a background
//!   thread (using `tiny_http`, sync — mirrors the transport reader-thread +
//!   stop-flag pattern in `transport::usb_serial`; the app stays sync). It
//!   validates the `X-AK-Status-Token` header, parses the body into a
//!   [`HookEvent`], maps it, and hands the resulting [`DisplayState`] to a
//!   caller-supplied callback. A random token is generated at start and written,
//!   together with the bound port, to a stable **handoff file** so the separate
//!   `companion --hook` process can find the sink (see [`handoff_path`] /
//!   [`write_handoff`] / [`read_handoff`]).
//!
//! Design ref: `.kiro/specs/feature-probes-kirocrew-local-control/design.md`
//! (Decision 2: the hook is a mode of the companion binary, `companion --hook`,
//! not a separate shell script — see `Handoff` below).

use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Header carrying the shared token the `--hook` mode must present. A localhost
/// listener with no auth would let any local process spoof board status
/// (design "Security notes" / AC3.4), so the token gates every request.
pub const STATUS_TOKEN_HEADER: &str = "X-AK-Status-Token";

/// Default port the status sink binds when the UI does not pin one.
pub const DEFAULT_STATUS_PORT: u16 = 5477;

// ───────────────────────────── T5 — pure mapping ─────────────────────────────

/// The slimmed hook event the companion's `--hook` mode reads from STDIN and
/// POSTs to the sink. Tolerant on field names: Kiro hooks spell the event key
/// `hook_event_name`; we also accept a plain `event` alias so the same struct
/// works whether the raw hook JSON or a pre-slimmed body is posted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookEvent {
    /// The hook name: `agentSpawn` | `postToolUse` | `stop` | `userPromptSubmit`
    /// | …. Accepts either `hook_event_name` (Kiro's own key) or `event`.
    #[serde(default, alias = "hook_event_name")]
    pub event: String,
    /// Stable session identity. CLI hooks provide `session_id`/`sessionId` or
    /// `$KIRO_SESSION_ID`; Crew ScriptHookStore provides `parent_session_key`.
    #[serde(default, alias = "sessionId", alias = "parent_session_key", alias = "session_key")]
    pub session_id: String,
    /// Present on `postToolUse`: the tool that just ran.
    #[serde(default, alias = "toolName")]
    pub tool_name: Option<String>,
    /// Present on Stop: CLI uses `assistant_response`; Crew uses `assistant_text`.
    #[serde(default, alias = "assistantResponse", alias = "assistant_text")]
    pub assistant_response: Option<String>,
}

/// A mapped status ready to render on the board LCD. `state` is a free string
/// matching the `display` frame's state enum. The board colors its slot icons
/// by it: `running`=busy(red), `idle`=green, `waiting`=orange, `info`=blue,
/// `unbound`/empty=purple.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayState {
    pub agent_name: String,
    pub state: String,
    /// Compact, single-line text forwarded to the board LCD.
    pub text: String,
    /// Original Hook event name shown in the Companion detail view.
    pub event: String,
    /// Full event/assistant text shown in the Companion without truncation.
    pub detail_text: String,
    /// Session this status belongs to (echoed through for per-session keying).
    pub session_id: String,
}

/// Max characters of an assistant response we forward to the tiny LCD.
const MAX_RESPONSE_CHARS: usize = 40;

/// Truncate `s` to at most `MAX_RESPONSE_CHARS` characters (char-safe, appends
/// an ellipsis when cut). Collapses inner whitespace/newlines to single spaces
/// so a multi-line reply renders on one LCD line.
fn truncate_response(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(MAX_RESPONSE_CHARS).collect();
    if flat.chars().count() > MAX_RESPONSE_CHARS {
        out.push('…');
    }
    out
}

/// Pure map (T5): a hook event -> the display status to show on the board.
///
/// | hook event        | state     | text                                  |
/// |-------------------|-----------|---------------------------------------|
/// | `agentSpawn`      | `running` | `agent started`                       |
/// | `postToolUse`     | `running` | `using <tool_name>`                   |
/// | `stop` (w/ reply) | `info`    | `<assistant_response truncated>`      |
/// | `stop` (no reply) | `idle`    | `done`                                |
/// | `userPromptSubmit`| `running` | `prompt received`                     |
/// | (anything else)   | `running` | `<event>`                             |
///
/// Board colors: `running`=red(busy), `idle`=green, `waiting`=orange,
/// `info`=blue, `unbound`/empty=purple.
///
/// `agent_name` is not carried by the hook payload (a hook fires for one agent
/// but the name is not in the JSON), so it defaults to `"kirocrew"`; the app
/// layer may override it from the bound session.
pub fn map_hook_event(ev: &HookEvent) -> DisplayState {
    let normalized = ev.event.to_ascii_lowercase();
    let (state, text, detail_text) = match normalized.as_str() {
        "agentspawn" | "sessionstart" => {
            let text = "agent started".to_string();
            ("running", text.clone(), text)
        }
        "posttooluse" => {
            // Still working — maps to the "busy" color (red) on the board.
            let tool = ev.tool_name.as_deref().unwrap_or("tool");
            let text = format!("using {tool}");
            ("running", text.clone(), text)
        }
        "stop" => {
            // Turn finished. With a reply to read but nothing to act on, this is
            // "info" (blue). With no reply, it is plain "idle" (green).
            let resp = ev.assistant_response.as_deref().unwrap_or("");
            if resp.trim().is_empty() {
                let text = "done".to_string();
                ("idle", text.clone(), text)
            } else {
                ("info", truncate_response(resp), resp.to_string())
            }
        }
        "userpromptsubmit" => {
            // Prompt accepted; the agent is now working -> busy (red).
            let text = "prompt received".to_string();
            ("running", text.clone(), text)
        }
        // Unknown / finer states retain the original spelling for diagnostics.
        _ => ("running", ev.event.clone(), ev.event.clone()),
    };
    DisplayState {
        agent_name: "kirocrew".to_string(),
        state: state.to_string(),
        text,
        event: ev.event.clone(),
        detail_text,
        session_id: ev.session_id.clone(),
    }
}

/// Pure "watched chat" filter (option A). Decide whether a status for
/// `session_id` should be forwarded to the board LCD, given the currently
/// pinned session:
///
/// * `pinned == None`  → follow every session (forward all).
/// * `pinned == Some(p)` → forward ONLY the session whose id equals `p`, so a
///   background chat's activity does not clobber the LCD of the watched chat.
///
/// Kept here (cross-platform) rather than in the Windows-gated `app` module so
/// it builds and unit-tests on the Linux CI host.
pub fn should_forward(pinned: Option<&str>, session_id: &str) -> bool {
    match pinned {
        None => true,
        Some(p) => p == session_id,
    }
}

// ───────────────────────────── Handoff file ─────────────────────────────
/// The `{port, token}` handoff the GUI writes when the sink starts and the
/// `--hook` process reads to find the sink. Small and non-secret-at-rest beyond
/// the ephemeral token (the sink is loopback-only).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub port: u16,
    pub token: String,
}

/// Path of the handoff file: `~/.ak-companion/hook.json` (see `app_dirs`).
/// The same dot-directory holds the bind store and the diagnostic logs. The
/// `--hook` subprocess resolves this same path to discover the running sink.
pub fn handoff_path() -> Option<PathBuf> {
    Some(crate::app_dirs::companion_file("hook.json"))
}

/// Write the handoff file (creating the parent dir). Used by the GUI on sink
/// start so the `--hook` process can locate the sink.
pub fn write_handoff(h: &Handoff) -> Result<(), String> {
    let path = handoff_path().ok_or("cannot resolve config dir for hook handoff")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let body = serde_json::to_string(h).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Read the handoff file. `None` (not `Err`) when it is absent, so the `--hook`
/// mode can silently exit 0 when the GUI/sink is not running.
pub fn read_handoff() -> Option<Handoff> {
    let path = handoff_path()?;
    let body = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&body).ok()
}

/// Best-effort removal of the handoff file (on sink stop / shutdown).
pub fn remove_handoff() {
    if let Some(path) = handoff_path() {
        let _ = std::fs::remove_file(path);
    }
}

/// Generate a URL-safe random token (32 hex chars = 16 random bytes) using
/// `getrandom` (already resolved transitively). No `rand` crate needed.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Fall back to a time-seeded token; still non-guessable enough for a
        // loopback-only listener, and generate_token never fails the caller.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((nanos >> (i * 8)) & 0xff) as u8;
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ───────────────────────────── T6 — HTTP server ─────────────────────────────

/// A running localhost status sink. Holds the generated token, the bound port,
/// a stop flag, and the background listener thread's handle.
///
/// Drop / [`StatusSink::stop`] sets the stop flag and unblocks the accept loop
/// so the thread exits and the OS port is released — mirroring the
/// `transport::usb_serial` reader-thread + stop-flag lifecycle.
pub struct StatusSink {
    port: u16,
    token: String,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl StatusSink {
    /// Bind `127.0.0.1:port` (0 = OS-chosen) and start the listener thread.
    ///
    /// `on_status` is invoked (on the listener thread) with each mapped
    /// [`DisplayState`]. It is `Send + 'static` so the app can move a channel /
    /// `AppHandle` into it. On start, a token is generated and the `{port,
    /// token}` handoff file is written for the `--hook` process.
    pub fn start<F>(port: u16, on_status: F) -> Result<Self, String>
    where
        F: Fn(DisplayState) + Send + 'static,
    {
        let server = tiny_http::Server::http(("127.0.0.1", port))
            .map_err(|e| format!("bind 127.0.0.1:{port} failed: {e}"))?;
        // Resolve the actually-bound port (matters when `port == 0`).
        let bound_port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[allow(unreachable_patterns)]
            _ => port,
        };

        let token = generate_token();
        write_handoff(&Handoff {
            port: bound_port,
            token: token.clone(),
        })?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let token_thread = token.clone();
        // The callback runs only on this single listener thread, so it need not
        // be `Sync` — box it and MOVE it in rather than sharing an `Arc`.
        let cb: Box<dyn Fn(DisplayState) + Send> = Box::new(on_status);

        let handle = thread::spawn(move || {
            serve_loop(server, stop_thread, token_thread, cb);
        });

        Ok(Self {
            port: bound_port,
            token,
            stop,
            handle: Some(handle),
        })
    }

    /// The bound port (resolved even when started with `port == 0`).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The shared token the `--hook` process must present.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Stop the listener: set the stop flag, poke the accept loop with a local
    /// request so it wakes, join the thread, and remove the handoff file.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock a thread parked in `recv()` with a throwaway local request.
        // Best-effort: if it fails the recv_timeout below still bounds the wait.
        let _ = poke(self.port);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        remove_handoff();
    }
}

impl Drop for StatusSink {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.stop();
        }
    }
}

/// Send a tiny throwaway TCP connection to unblock a parked `recv()`.
fn poke(port: u16) -> std::io::Result<()> {
    use std::io::Write;
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port))?;
    // A minimal request line is enough to make tiny_http return a Request.
    let _ = s.write_all(b"GET /_poke HTTP/1.0\r\n\r\n");
    Ok(())
}

/// The accept loop. Runs on the listener thread until `stop` is set. Validates
/// the token, maps the body, and fires the callback. Never panics on a bad
/// request — a malformed body is answered `400` and the loop continues.
fn serve_loop(
    server: tiny_http::Server,
    stop: Arc<AtomicBool>,
    token: String,
    cb: Box<dyn Fn(DisplayState) + Send>,
) {
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        // Bounded wait so we re-check `stop` even without traffic.
        let req = match server.recv_timeout(Duration::from_millis(500)) {
            Ok(Some(req)) => req,
            Ok(None) => continue,
            Err(_) => break,
        };
        if stop.load(Ordering::SeqCst) {
            let _ = req.respond(tiny_http::Response::empty(503));
            break;
        }
        handle_request(req, &token, cb.as_ref());
    }
}

/// Validate + parse one request and fire the callback. Factored out so it stays
/// small and the loop is only lifecycle.
fn handle_request(mut req: tiny_http::Request, token: &str, cb: &dyn Fn(DisplayState)) {
    // Token gate: any request without the exact header is 401 and dropped.
    let presented = req
        .headers()
        .iter()
        .find(|h| h.field.equiv(STATUS_TOKEN_HEADER))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default();
    if presented != token || token.is_empty() {
        let _ = req.respond(tiny_http::Response::from_string("unauthorized").with_status_code(401));
        return;
    }

    let mut body = String::new();
    if req.as_reader().read_to_string(&mut body).is_err() {
        let _ = req.respond(tiny_http::Response::from_string("bad body").with_status_code(400));
        return;
    }

    match serde_json::from_str::<HookEvent>(&body) {
        Ok(ev) => {
            let display = map_hook_event(&ev);
            cb(display);
            let _ = req.respond(tiny_http::Response::from_string("ok"));
        }
        Err(e) => {
            let _ = req.respond(
                tiny_http::Response::from_string(format!("bad json: {e}")).with_status_code(400),
            );
        }
    }
}

// ───────────────────────────── --hook client ─────────────────────────────

/// Deliver a hook event to a running sink. Delivery failures are returned for
/// Crew's Hook Test/details UI to expose; `run_hook_mode` still exits zero so a
/// missing Companion can never fail an agent turn.
pub fn post_hook_event(ev: &HookEvent) -> Result<(), String> {
    let handoff = read_handoff().ok_or_else(|| {
        let path = handoff_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unresolved>".to_string());
        format!("status sink handoff missing or invalid: {path}; start the Companion sink")
    })?;
    let url = format!("http://127.0.0.1:{}/status", handoff.port);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(800))
        .build()
        .map_err(|e| format!("build hook HTTP client: {e}"))?;
    let response = client
        .post(&url)
        .header(STATUS_TOKEN_HEADER, &handoff.token)
        .json(ev)
        .send()
        .map_err(|e| format!("POST {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("POST {url}: HTTP {}", response.status()));
    }
    Ok(())
}

/// Read one raw Crew/CLI hook event from STDIN and forward it. Diagnostics go
/// to stderr (visible from Crew's Hook Test and hook run details), while the
/// process remains non-fatal to the agent turn.
pub fn run_hook_mode() {
    let mut buf = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut buf) {
        eprintln!("[AK-HOOK] read stdin: {error}");
        return;
    }
    let mut ev: HookEvent = match serde_json::from_str(&buf) {
        Ok(event) => event,
        Err(error) => {
            eprintln!("[AK-HOOK] parse stdin JSON: {error}");
            return;
        }
    };
    if ev.session_id.trim().is_empty() {
        if let Ok(sid) = std::env::var("KIRO_SESSION_ID") {
            ev.session_id = sid;
        }
    }
    if let Err(error) = post_hook_event(&ev) {
        eprintln!("[AK-HOOK] {error}");
    }
}

/// Probe helper for tests / callers: is a port bindable on loopback right now?
#[allow(dead_code)]
fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── T5 pure mapping: one test per branch ─────────────────

    #[test]
    fn agent_spawn_maps_to_running() {
        let ev = HookEvent {
            event: "AgentSpawn".into(),
            session_id: "s1".into(),
            ..Default::default()
        };
        let d = map_hook_event(&ev);
        assert_eq!(d.state, "running");
        assert_eq!(d.text, "agent started");
        assert_eq!(d.session_id, "s1");
        assert_eq!(d.agent_name, "kirocrew");
        assert_eq!(d.event, "AgentSpawn");
        assert_eq!(d.detail_text, "agent started");
    }

    #[test]
    fn post_tool_use_maps_to_tool_with_name() {
        let ev = HookEvent {
            event: "PostToolUse".into(),
            session_id: "s2".into(),
            tool_name: Some("web_search".into()),
            ..Default::default()
        };
        let d = map_hook_event(&ev);
        // PostToolUse means the agent is still working -> busy (red).
        assert_eq!(d.state, "running");
        assert_eq!(d.text, "using web_search");
    }

    #[test]
    fn post_tool_use_without_name_falls_back() {
        let ev = HookEvent {
            event: "PostToolUse".into(),
            ..Default::default()
        };
        assert_eq!(map_hook_event(&ev).text, "using tool");
    }

    #[test]
    fn stop_with_response_maps_to_info_with_truncated_text() {
        let long = "a".repeat(100);
        let ev = HookEvent {
            event: "Stop".into(),
            session_id: "s3".into(),
            assistant_response: Some(long),
            ..Default::default()
        };
        let d = map_hook_event(&ev);
        // A finished turn with a reply to read, no action needed -> info (blue).
        assert_eq!(d.state, "info");
        // 40 chars + ellipsis.
        assert_eq!(d.text.chars().count(), MAX_RESPONSE_CHARS + 1);
        assert!(d.text.ends_with('…'));
        assert_eq!(d.detail_text, "a".repeat(100));
    }

    #[test]
    fn stop_without_response_maps_to_idle() {
        let ev = HookEvent {
            event: "Stop".into(),
            session_id: "s3".into(),
            assistant_response: Some("   ".into()),
            ..Default::default()
        };
        let d = map_hook_event(&ev);
        assert_eq!(d.state, "idle");
        assert_eq!(d.text, "done");
    }

    #[test]
    fn stop_with_empty_response_says_done() {
        let ev = HookEvent {
            event: "Stop".into(),
            assistant_response: Some("   ".into()),
            ..Default::default()
        };
        assert_eq!(map_hook_event(&ev).text, "done");

        let ev2 = HookEvent {
            event: "Stop".into(),
            ..Default::default()
        };
        assert_eq!(map_hook_event(&ev2).text, "done");
    }

    #[test]
    fn stop_flattens_multiline_response() {
        let ev = HookEvent {
            event: "stop".into(),
            assistant_response: Some("line one\n\nline two   with  spaces".into()),
            ..Default::default()
        };
        let d = map_hook_event(&ev);
        assert!(!d.text.contains('\n'));
        assert!(d.text.starts_with("line one line two"));
        assert_eq!(d.detail_text, "line one\n\nline two   with  spaces");
    }

    #[test]
    fn user_prompt_submit_maps_to_running() {
        let ev = HookEvent {
            event: "UserPromptSubmit".into(),
            ..Default::default()
        };
        let d = map_hook_event(&ev);
        // Prompt accepted; agent now working -> busy (red).
        assert_eq!(d.state, "running");
        assert_eq!(d.text, "prompt received");
    }

    #[test]
    fn unknown_event_falls_back_to_running_with_event_text() {
        let ev = HookEvent {
            event: "preToolUse".into(),
            ..Default::default()
        };
        let d = map_hook_event(&ev);
        assert_eq!(d.state, "running");
        assert_eq!(d.text, "preToolUse");
    }

    // ── should_forward "watched chat" filter ─────────────────

    #[test]
    fn no_pin_forwards_every_session() {
        assert!(should_forward(None, "sess-A"));
        assert!(should_forward(None, "sess-B"));
        assert!(should_forward(None, ""));
    }

    #[test]
    fn pin_forwards_only_matching_session() {
        assert!(should_forward(Some("sess-A"), "sess-A"));
        assert!(!should_forward(Some("sess-A"), "sess-B"));
        assert!(!should_forward(Some("sess-A"), ""));
    }

    // ── HookEvent deserialization tolerance ──────────────────
    #[test]
    fn hook_event_accepts_kiro_key_names() {
        // Raw Kiro hook JSON uses hook_event_name; a slimmed body uses `event`.
        let raw: HookEvent = serde_json::from_str(
            r#"{"hook_event_name":"postToolUse","sessionId":"abc","toolName":"grep"}"#,
        )
        .unwrap();
        assert_eq!(raw.event, "postToolUse");
        assert_eq!(raw.session_id, "abc");
        assert_eq!(raw.tool_name.as_deref(), Some("grep"));

        let crew: HookEvent = serde_json::from_str(
            r#"{"hook_event_name":"Stop","parent_session_key":"dashboard:chat-1","assistant_text":"done"}"#,
        )
        .unwrap();
        assert_eq!(crew.event, "Stop");
        assert_eq!(crew.session_id, "dashboard:chat-1");
        assert_eq!(crew.assistant_response.as_deref(), Some("done"));

        let slim: HookEvent =
            serde_json::from_str(r#"{"event":"stop","session_id":"z","assistant_response":"hi"}"#)
                .unwrap();
        assert_eq!(slim.event, "stop");
        assert_eq!(slim.session_id, "z");
        assert_eq!(slim.assistant_response.as_deref(), Some("hi"));
    }

    #[test]
    fn hook_event_empty_body_is_default() {
        let ev: HookEvent = serde_json::from_str("{}").unwrap();
        assert_eq!(ev, HookEvent::default());
    }

    // ── token generation ─────────────────────────────────────

    #[test]
    fn token_is_32_hex_chars_and_random() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two tokens must differ");
    }

    // ── Handoff round-trip (in an isolated HOME/XDG dir) ─────

    #[test]
    fn handoff_json_round_trips() {
        let h = Handoff {
            port: 5477,
            token: "deadbeef".into(),
        };
        let s = serde_json::to_string(&h).unwrap();
        let back: Handoff = serde_json::from_str(&s).unwrap();
        assert_eq!(h, back);
    }

    // ── Server end-to-end on an ephemeral port (loopback only) ─

    #[test]
    fn sink_round_trip_maps_and_fires_callback() {
        use std::sync::mpsc;

        // Point the companion home at a scratch dir so the test's handoff file
        // does not clobber a real one. app_dirs::companion_dir resolves from
        // HOME (then USERPROFILE), so overriding HOME redirects it to
        // <scratch>/.ak-companion/hook.json.
        let scratch = std::env::var("KIROCREW_SCRATCH")
            .or_else(|_| std::env::var("TMPDIR"))
            .unwrap_or_else(|_| "/tmp".into());
        let cfg = format!("{scratch}/ak-hook-test-{}", std::process::id());
        std::env::set_var("HOME", &cfg);

        let (tx, rx) = mpsc::channel::<DisplayState>();
        let mut sink = StatusSink::start(0, move |d| {
            let _ = tx.send(d);
        })
        .expect("sink starts");
        let port = sink.port();
        let token = sink.token().to_string();
        assert_ne!(port, 0);

        // Handoff file was written with the bound port + token.
        let h = read_handoff().expect("handoff written");
        assert_eq!(h.port, port);
        assert_eq!(h.token, token);

        // A valid POST fires the callback with the mapped state.
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{port}/status"))
            .header(STATUS_TOKEN_HEADER, &token)
            .json(&HookEvent {
                event: "agentSpawn".into(),
                session_id: "sess-A".into(),
                ..Default::default()
            })
            .send()
            .expect("post");
        assert!(resp.status().is_success());
        let got = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("callback fired");
        assert_eq!(got.state, "running");
        assert_eq!(got.session_id, "sess-A");

        // A wrong token is rejected 401 and does NOT fire the callback.
        let bad = client
            .post(format!("http://127.0.0.1:{port}/status"))
            .header(STATUS_TOKEN_HEADER, "wrong")
            .json(&HookEvent {
                event: "stop".into(),
                ..Default::default()
            })
            .send()
            .expect("post");
        assert_eq!(bad.status().as_u16(), 401);
        assert!(rx.recv_timeout(Duration::from_millis(400)).is_err());

        sink.stop();
        // Handoff removed on stop.
        assert!(read_handoff().is_none());
    }
}
