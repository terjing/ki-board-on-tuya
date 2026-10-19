//! Install AK Companion status hooks in Kiro Crew's global ScriptHookStore.
//!
//! Kiro Crew 0.6.0 persists the hooks shown in Agent Capabilities > Hooks in
//! `~/.kiro/crew/hooks.json` (or `$KIRO_HOME/crew/hooks.json`). This is separate
//! from the CLI 2.x ACP Agent Hooks embedded in Agent JSON files and from the
//! CLI 3.0 standalone v1 `.kiro/hooks/*.json` format.
//!
//! The store is shared with webhook continuation records, so every mutation:
//! * takes Crew's `hooks.json.lock` sidecar lock;
//! * re-reads while holding the lock;
//! * changes only Companion-owned entries in the top-level `hooks` array;
//! * preserves every other top-level key and every non-AK hook.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

pub const HOOK_FILE_NAME: &str = "hooks.json";
pub const HOOK_NAME_PREFIX: &str = "ak-companion-status-";
pub const HOOK_MARKER: &str = "--hook";
pub const HOOK_EVENTS: [(&str, &str, &str); 4] = [
    ("AgentSpawn", "agent-spawn", "akspawn"),
    ("UserPromptSubmit", "prompt-submit", "akprompt"),
    ("PostToolUse", "post-tool-use", "akpost"),
    ("Stop", "stop", "akstop"),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HookStatus {
    /// Command written into every AK Companion ScriptHook entry.
    pub command: String,
    /// Kiro Crew configuration directory containing `hooks.json`.
    pub hooks_dir: String,
    /// Shared ScriptHookStore file managed cooperatively by Crew and Companion.
    pub hook_file: String,
    /// Number of required lifecycle events correctly configured (0..=4).
    pub installed_events: u8,
    pub fully_installed: bool,
    pub any_installed: bool,
}

/// Resolve Kiro Crew's configuration directory. `KIRO_HOME` points at the Kiro
/// root containing `agents` and `crew`.
pub fn hooks_dir() -> Option<PathBuf> {
    if let Ok(kiro_home) = std::env::var("KIRO_HOME") {
        let value = kiro_home.trim();
        if !value.is_empty() {
            return Some(PathBuf::from(value).join("crew"));
        }
    }
    let home = dirs::home_dir().or_else(|| std::env::var("HOME").ok().map(PathBuf::from))?;
    Some(home.join(".kiro").join("crew"))
}

pub fn hook_file() -> Option<PathBuf> {
    hooks_dir().map(|dir| dir.join(HOOK_FILE_NAME))
}

/// Which shell family Kiro Crew uses to run hook commands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookShell {
    Windows,
    Posix,
}

impl HookShell {
    pub fn current() -> HookShell {
        if cfg!(windows) {
            HookShell::Windows
        } else {
            HookShell::Posix
        }
    }
}

/// Quote the executable so paths with spaces survive Crew's platform shell.
/// Windows: `"<exe>" --hook`. POSIX (`/bin/sh -c`): `'<exe>' --hook` with each
/// embedded `'` written as `'\''`.
pub fn hook_command_for(shell: HookShell, companion_exe: &str) -> String {
    match shell {
        HookShell::Windows => format!("\"{companion_exe}\" {HOOK_MARKER}"),
        HookShell::Posix => {
            let quoted = companion_exe.replace('\'', "'\\''");
            format!("'{quoted}' {HOOK_MARKER}")
        }
    }
}

pub fn hook_command(companion_exe: &str) -> String {
    hook_command_for(HookShell::current(), companion_exe)
}

pub fn current_exe_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.to_str().map(str::to_string))
        .unwrap_or_else(|| "companion".to_string())
}

fn canonical_hook(event: &str, slug: &str, id: &str, command: &str, old: Option<&Value>) -> Value {
    let last_run = old
        .and_then(|h| h.get("last_run"))
        .cloned()
        .unwrap_or(json!(0.0));
    let last_status = old
        .and_then(|h| h.get("last_status"))
        .cloned()
        .unwrap_or(json!(""));
    let last_error = old
        .and_then(|h| h.get("last_error"))
        .cloned()
        .unwrap_or(json!(""));
    let run_count = old
        .and_then(|h| h.get("run_count"))
        .cloned()
        .unwrap_or(json!(0));
    json!({
        "id": id,
        "name": format!("{HOOK_NAME_PREFIX}{slug}"),
        "event": event,
        "matcher": "",
        "matcher_mode": "glob",
        "command": command,
        "skills": [],
        "timeout": 5,
        "enabled": true,
        "last_run": last_run,
        "last_status": last_status,
        "last_error": last_error,
        "run_count": run_count,
    })
}

fn is_our_hook(hook: &Value) -> bool {
    hook.get("name")
        .and_then(Value::as_str)
        .map(|name| name.starts_with(HOOK_NAME_PREFIX))
        .unwrap_or(false)
}

/// Count required ScriptHook events with the expected command and enabled state.
pub fn count_installed(config: &Value, command: &str) -> u8 {
    let Some(hooks) = config.get("hooks").and_then(Value::as_array) else {
        return 0;
    };
    HOOK_EVENTS
        .iter()
        .filter(|(event, _, _)| {
            hooks.iter().any(|hook| {
                is_our_hook(hook)
                    && hook.get("event").and_then(Value::as_str) == Some(*event)
                    && hook.get("command").and_then(Value::as_str) == Some(command)
                    && hook.get("enabled").and_then(Value::as_bool) == Some(true)
            })
        })
        .count() as u8
}

/// Merge the four canonical entries into Crew's shared ScriptHookStore object.
/// Existing Companion entries are replaced as a group; all foreign entries and
/// top-level webhook registration keys are retained.
pub fn apply_install(config: &mut Value, command: &str) -> Result<bool, String> {
    let before = config.clone();
    let root = config
        .as_object_mut()
        .ok_or("Kiro Crew hooks.json must contain a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or("Kiro Crew hooks.json 'hooks' must be an array")?;

    let old_hooks = hooks.clone();
    hooks.retain(|hook| !is_our_hook(hook));
    hooks.extend(HOOK_EVENTS.iter().map(|(event, slug, id)| {
        let old = old_hooks.iter().find(|hook| {
            is_our_hook(hook) && hook.get("event").and_then(Value::as_str) == Some(*event)
        });
        canonical_hook(event, slug, id, command, old)
    }));
    Ok(*config != before)
}

/// Remove only Companion-owned entries. Other script hooks and webhook records
/// in the shared object remain untouched.
pub fn apply_remove(config: &mut Value) -> Result<bool, String> {
    let root = config
        .as_object_mut()
        .ok_or("Kiro Crew hooks.json must contain a JSON object")?;
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(false);
    };
    let hooks = hooks
        .as_array_mut()
        .ok_or("Kiro Crew hooks.json 'hooks' must be an array")?;
    let before = hooks.len();
    hooks.retain(|hook| !is_our_hook(hook));
    Ok(hooks.len() != before)
}

fn read_config(path: &Path) -> Result<Option<Value>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_str(&body)
        .map(Some)
        .map_err(|error| format!("parse {}: {error}", path.display()))
}

fn lock_file(path: &Path) -> Result<File, String> {
    let lock_path = path.with_file_name(format!("{HOOK_FILE_NAME}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| format!("open {}: {error}", lock_path.display()))?;
    file.lock()
        .map_err(|error| format!("lock {}: {error}", lock_path.display()))?;
    Ok(file)
}

fn write_config(path: &Path, config: &Value) -> Result<(), String> {
    let mut body = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    body.push('\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))?;
    use std::io::Write;
    file.write_all(body.as_bytes())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", path.display()))
}

fn with_locked_config<T>(
    path: &Path,
    mutate: impl FnOnce(&mut Value) -> Result<T, String>,
) -> Result<T, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let _lock = lock_file(path)?;
    let mut config = read_config(path)?.unwrap_or_else(|| json!({ "hooks": [] }));
    let result = mutate(&mut config)?;
    write_config(path, &config)?;
    Ok(result)
}

fn status_at(command: &str, dir: &Path, path: &Path) -> Result<HookStatus, String> {
    let installed_events = read_config(path)?
        .as_ref()
        .map(|config| count_installed(config, command))
        .unwrap_or(0);
    Ok(HookStatus {
        command: command.to_string(),
        hooks_dir: dir.display().to_string(),
        hook_file: path.display().to_string(),
        installed_events,
        fully_installed: installed_events == HOOK_EVENTS.len() as u8,
        any_installed: installed_events > 0,
    })
}

pub fn status(command: &str) -> Result<HookStatus, String> {
    let dir = hooks_dir().ok_or("cannot resolve Kiro Crew configuration directory")?;
    let path = dir.join(HOOK_FILE_NAME);
    status_at(command, &dir, &path)
}

pub fn install(command: &str) -> Result<HookStatus, String> {
    let dir = hooks_dir().ok_or("cannot resolve Kiro Crew configuration directory")?;
    let path = dir.join(HOOK_FILE_NAME);
    with_locked_config(&path, |config| apply_install(config, command))?;
    status_at(command, &dir, &path)
}

pub fn remove(command: &str) -> Result<HookStatus, String> {
    let dir = hooks_dir().ok_or("cannot resolve Kiro Crew configuration directory")?;
    let path = dir.join(HOOK_FILE_NAME);
    if path.exists() {
        with_locked_config(&path, apply_remove)?;
    }
    status_at(command, &dir, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> String {
        hook_command_for(
            HookShell::Windows,
            "C:\\Program Files\\AK Companion\\companion.exe",
        )
    }

    #[test]
    fn hook_command_quotes_paths_with_spaces() {
        assert_eq!(
            command(),
            "\"C:\\Program Files\\AK Companion\\companion.exe\" --hook"
        );
    }

    #[test]
    fn posix_hook_command_single_quotes_app_bundle_path_with_spaces() {
        assert_eq!(
            hook_command_for(
                HookShell::Posix,
                "/Applications/Agentic Keyboard Companion.app/Contents/MacOS/companion"
            ),
            "'/Applications/Agentic Keyboard Companion.app/Contents/MacOS/companion' --hook"
        );
    }

    #[test]
    fn posix_hook_command_escapes_embedded_single_quote() {
        assert_eq!(
            hook_command_for(HookShell::Posix, "/Users/it's/companion"),
            "'/Users/it'\\''s/companion' --hook"
        );
    }

    #[test]
    fn hook_command_uses_current_shell() {
        let exe = "/opt/companion";
        assert_eq!(
            hook_command(exe),
            hook_command_for(HookShell::current(), exe)
        );
    }

    #[test]
    fn posix_apply_install_counts_four_events() {
        let cmd = hook_command_for(HookShell::Posix, "/Applications/AK Companion.app/x");
        let mut config = json!({ "hooks": [] });
        assert!(apply_install(&mut config, &cmd).unwrap());
        assert_eq!(count_installed(&config, &cmd), 4);
    }

    #[test]
    fn install_creates_four_crew_script_hooks() {
        let mut config = json!({ "hooks": [] });
        assert!(apply_install(&mut config, &command()).unwrap());
        assert_eq!(count_installed(&config, &command()), 4);
        assert_eq!(config["hooks"].as_array().unwrap().len(), 4);
        assert_eq!(config["hooks"][0]["event"], "AgentSpawn");
        assert_eq!(config["hooks"][3]["event"], "Stop");
        assert!(config.get("version").is_none());
    }

    #[test]
    fn install_is_idempotent_and_updates_command() {
        let mut config = json!({ "hooks": [] });
        assert!(apply_install(&mut config, &command()).unwrap());
        assert!(!apply_install(&mut config, &command()).unwrap());
        let changed = "\"D:\\AK\\companion.exe\" --hook";
        assert!(apply_install(&mut config, changed).unwrap());
        assert_eq!(config["hooks"].as_array().unwrap().len(), 4);
        assert_eq!(count_installed(&config, changed), 4);
    }

    #[test]
    fn install_and_remove_preserve_foreign_hooks_and_webhook_keys() {
        let custom = json!({
            "id": "custom",
            "name": "test",
            "event": "UserPromptSubmit",
            "command": "echo hello",
            "enabled": true
        });
        let webhook = json!({ "context": "resume me" });
        let mut config = json!({
            "hooks": [custom.clone()],
            "registered-webhook": webhook.clone()
        });
        apply_install(&mut config, &command()).unwrap();
        assert_eq!(config["hooks"].as_array().unwrap().len(), 5);
        assert_eq!(config["registered-webhook"], webhook);
        assert!(apply_remove(&mut config).unwrap());
        assert_eq!(config["hooks"], json!([custom]));
        assert_eq!(config["registered-webhook"], webhook);
    }

    #[test]
    fn reinstall_preserves_runtime_status_fields() {
        let mut config = json!({ "hooks": [] });
        apply_install(&mut config, &command()).unwrap();
        config["hooks"][0]["run_count"] = json!(7);
        config["hooks"][0]["last_status"] = json!("ok");
        apply_install(&mut config, &command()).unwrap();
        assert_eq!(config["hooks"][0]["run_count"], 7);
        assert_eq!(config["hooks"][0]["last_status"], "ok");
    }

    #[test]
    fn partial_or_disabled_status_is_not_fully_installed() {
        let mut config = json!({ "hooks": [] });
        apply_install(&mut config, &command()).unwrap();
        config["hooks"][0]["enabled"] = json!(false);
        assert_eq!(count_installed(&config, &command()), 3);
    }

    #[test]
    fn malformed_store_is_refused() {
        let mut array = json!([]);
        assert!(apply_install(&mut array, &command()).is_err());
        let mut hooks = json!({ "hooks": {} });
        assert!(apply_install(&mut hooks, &command()).is_err());
    }
}
