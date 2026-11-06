//! Agent Stop hooks (doc 142, phase W4): the only fully reliable way to know that a coding agent stopped
//! is for the agent to say so. Claude Code runs a hook command when it stops or needs the user; Codex
//! runs its `notify` program after each turn. NEXUS installs a tiny script that posts "claude stopped" to
//! its own localhost endpoint, and records the event.
//!
//! Safety rules (all tested):
//!   * installing is ALWAYS approved by the user first (voice offer), never silent;
//!   * the user's file is backed up before it is touched, written atomically, and left alone when it
//!     cannot be understood (invalid JSON, an unexpected shape, a `notify` command of their own);
//!   * only NEXUS's own entries (recognised by `MARKER` in their command) are ever replaced or removed;
//!   * the endpoint has its OWN token. The `/trigger` token can run any spoken command, so it is never
//!     written anywhere an agent config can see it; this one can only record "agent X stopped";
//!   * an event carries no text: just the agent name and the kind. Output is judged from the screen or
//!     the sidecar, never from what a hook posts.

use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Appears in the script's file name, so it is in every command line NEXUS writes into a config.
pub const MARKER: &str = "nexus-agent-event";
const TOKEN_SERVICE: &str = "agent_event";
/// Events kept in all (a safety net); each agent keeps its own latest `MAX_EVENTS_PER_AGENT`, so a chatty agent can
/// never push another agent's Stop out of the list.
const MAX_EVENTS: usize = 256;
const MAX_EVENTS_PER_AGENT: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Antigravity,
    OpenCode,
    Gemini,
    Cursor,
    Aider,
    /// Cognition's Devin CLI. It reads Claude Code's hook settings, so Claude's hook already reports it; the event
    /// is relabelled from its payload (`parse_event`) and it has no hook of its own.
    Devin,
    /// Windsurf / Devin Desktop: `~/.codeium/windsurf/hooks.json`.
    Windsurf,
    /// Kilo Code (CLI and VS Code extension): a plugin file of its own, like OpenCode.
    Kilo,
    /// GitHub Copilot CLI: a hook file of its own in `~/.copilot/hooks/`.
    Copilot,
    /// ByteDance's Trae IDE: no hook mechanism exists. It can only be observed (processes, window title).
    Trae,
}

pub const ALL_AGENTS: [Agent; 12] = [
    Agent::Claude,
    Agent::Codex,
    Agent::Antigravity,
    Agent::OpenCode,
    Agent::Gemini,
    Agent::Cursor,
    Agent::Aider,
    Agent::Devin,
    Agent::Windsurf,
    Agent::Kilo,
    Agent::Copilot,
    Agent::Trae,
];

impl Agent {
    /// An IDE: its process tree (renderers, language server, file watchers) is never quiet, so process activity
    /// cannot say whether it is working.
    pub fn is_ide(self) -> bool {
        matches!(self, Agent::Antigravity | Agent::Cursor | Agent::Windsurf | Agent::Trae)
    }

    /// No hook mechanism exists for it: only its processes can be watched, so it can never count as "finished
    /// and perfect".
    pub fn observe_only(self) -> bool {
        matches!(self, Agent::Trae)
    }

    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Antigravity => "antigravity",
            Agent::OpenCode => "opencode",
            Agent::Gemini => "gemini",
            Agent::Cursor => "cursor",
            Agent::Aider => "aider",
            Agent::Devin => "devin",
            Agent::Windsurf => "windsurf",
            Agent::Kilo => "kilo",
            Agent::Copilot => "copilot",
            Agent::Trae => "trae",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Antigravity => "Antigravity",
            Agent::OpenCode => "OpenCode",
            Agent::Gemini => "Gemini CLI",
            Agent::Cursor => "Cursor",
            Agent::Aider => "Aider",
            Agent::Devin => "Devin",
            Agent::Windsurf => "Windsurf",
            Agent::Kilo => "Kilo Code",
            Agent::Copilot => "GitHub Copilot CLI",
            Agent::Trae => "Trae",
        }
    }

    /// The machine name ("claude") or a name the speech engine and `watch_power` use ("agy").
    pub fn from_name(s: &str) -> Option<Agent> {
        match s {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "antigravity" | "agy" => Some(Agent::Antigravity),
            "opencode" | "open code" | "open-code" => Some(Agent::OpenCode),
            "gemini" => Some(Agent::Gemini),
            "cursor" => Some(Agent::Cursor),
            "aider" => Some(Agent::Aider),
            "devin" | "devin cli" => Some(Agent::Devin),
            "windsurf" | "devin desktop" => Some(Agent::Windsurf),
            "kilo" | "kilo code" | "kilocode" => Some(Agent::Kilo),
            "copilot" | "github copilot" | "copilot cli" => Some(Agent::Copilot),
            "trae" => Some(Agent::Trae),
            _ => None,
        }
    }

    /// "Claude Code, Codex, Antigravity, OpenCode, Gemini CLI, Cursor or Aider", for a spoken line.
    pub fn spoken_list() -> String {
        let labels: Vec<&str> = ALL_AGENTS.iter().map(|a| a.label()).collect();
        match labels.split_last() {
            Some((last, rest)) => format!("{} or {}", rest.join(", "), last),
            None => String::new(),
        }
    }

    /// What the user has to do for a new hook to be read.
    pub fn restart_note(self) -> &'static str {
        match self {
            Agent::Claude => "Start a new Claude Code session for it to take effect.",
            Agent::Codex => "Start a new Codex session for it to take effect.",
            Agent::Antigravity => "Start a new Antigravity session for it to take effect.",
            Agent::OpenCode => "Restart OpenCode so it loads the plugin.",
            Agent::Gemini => "Start a new Gemini CLI session. If its hooks are switched off in its settings, I did not change that.",
            Agent::Cursor => "Restart Cursor so it reads the hook.",
            Agent::Aider => "Start a new Aider session for it to take effect.",
            Agent::Devin => "Devin reads Claude Code's hook, so start a new Devin session. I have not seen a Devin stop yet, so tell me if it does not report.",
            Agent::Windsurf => "Restart Windsurf so it reads the hook. Hooks do not run while a workspace is in Restricted Mode.",
            Agent::Kilo => "Restart Kilo so it loads the plugin.",
            Agent::Copilot => "Start a new Copilot CLI session for it to take effect.",
            Agent::Trae => "Trae has no hook to install.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Stop,
    /// The agent needs the user (a permission prompt, an elicitation).
    Notification,
    /// The user sent a new prompt: a new turn began, so an older stop no longer counts.
    Start,
    /// A reply of the agent's, sent on its own before the stop (Cursor's `afterAgentResponse`).
    Message,
}

/// What a Stop event says about HOW the agent stopped, when the agent says. Only ever makes the verdict stricter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StopInfo {
    /// `Some(false)` = the agent reported an error / an abort. `Some(true)` = it reported a normal finish.
    pub ok: Option<bool>,
    /// `Some(false)` = the agent says background work is still running (Antigravity's `fullyIdle`).
    pub fully_idle: Option<bool>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HookError {
    /// The config file exists but is not something we can edit safely.
    Unreadable(String),
    /// Codex already has a `notify` command that is not ours.
    HasOwnNotify,
    /// The agent has no hook mechanism at all (Trae).
    NoHookMechanism,
    Io(String),
}

impl HookError {
    pub fn spoken(&self, agent: Agent) -> String {
        match self {
            HookError::HasOwnNotify => format!(
                "{} already has a notify command of its own, so I left its settings alone, sir.",
                agent.label()
            ),
            HookError::NoHookMechanism => format!(
                "{} has no hook I can use, sir. I can only watch whether it goes quiet, so I will never call its work perfect.",
                agent.label()
            ),
            HookError::Unreadable(_) => format!(
                "I couldn't read the {} settings file safely, so I didn't touch it, sir.",
                agent.label()
            ),
            HookError::Io(_) => format!("I couldn't write the {} settings file, sir; nothing was changed.", agent.label()),
        }
    }
}

// ─── The token ───────────────────────────────────────────────────────

/// Get-or-create the endpoint token (keychain). Different from the `/trigger` token on purpose.
pub fn agent_event_token() -> String {
    if let Some(tok) = crate::auth_vault::get_api_key(TOKEN_SERVICE) {
        if !tok.is_empty() {
            return tok;
        }
    }
    let tok = crate::webhook::generate_token();
    crate::auth_vault::set_api_key(TOKEN_SERVICE, &tok);
    tok
}

// ─── The script the agents run ───────────────────────────────────────

#[cfg(windows)]
pub const SCRIPT_NAME: &str = "nexus-agent-event.cmd";
#[cfg(not(windows))]
pub const SCRIPT_NAME: &str = "nexus-agent-event.sh";

/// `script <agent> <stop|notification|start|message> [ignored extra arguments]`. The agent and event travel in the
/// URL; the BODY is whatever the agent's hook gives on stdin (Claude Code sends JSON with its last message). Codex
/// appends its own JSON as an extra argument, which is ignored. Aider's notification command has no stdin, so for
/// Aider the body is the working directory (its chat history is read from there). The script prints a neutral JSON
/// line and always exits 0, so it can never block an agent: `{}`, or `{"continue":true}` for Cursor, whose prompt
/// hook blocks the prompt without it. The token lives only in this file (user's app data), not in any agent config.
#[cfg(windows)]
const SCRIPT_TEMPLATE: &str = "@echo off\r\n\
rem NEXUS agent hook: tells NEXUS that an agent stopped. Remove with \"disconnect <agent>\".\r\n\
set AG=%~1\r\n\
set EV=%~2\r\n\
if \"%EV%\"==\"\" set EV=stop\r\n\
if /i \"%AG%\"==\"aider\" goto aider\r\n\
curl.exe -s -m 3 -X POST \"http://127.0.0.1:@PORT@/agent-event?agent=%AG%&event=%EV%\" -H \"Authorization: Bearer @TOKEN@\" -H \"Content-Type: application/json\" --data-binary @- >nul 2>&1\r\n\
goto out\r\n\
:aider\r\n\
curl.exe -s -m 3 -X POST \"http://127.0.0.1:@PORT@/agent-event?agent=%AG%&event=%EV%\" -H \"Authorization: Bearer @TOKEN@\" --data-urlencode \"cwd=%CD%\" >nul 2>&1\r\n\
:out\r\n\
if /i \"%AG%\"==\"cursor\" goto cursor\r\n\
echo {}\r\n\
exit /b 0\r\n\
:cursor\r\n\
echo {\"continue\":true}\r\n\
exit /b 0\r\n";

#[cfg(not(windows))]
const SCRIPT_TEMPLATE: &str = "#!/bin/sh\n\
# NEXUS agent hook: tells NEXUS that an agent stopped.\n\
AG=\"$1\"\n\
EV=\"${2:-stop}\"\n\
URL=\"http://127.0.0.1:@PORT@/agent-event?agent=$AG&event=$EV\"\n\
if [ \"$AG\" = \"aider\" ]; then\n\
  curl -s -m 3 -X POST \"$URL\" -H \"Authorization: Bearer @TOKEN@\" --data-urlencode \"cwd=$PWD\" >/dev/null 2>&1\n\
else\n\
  curl -s -m 3 -X POST \"$URL\" -H \"Authorization: Bearer @TOKEN@\" -H \"Content-Type: application/json\" --data-binary @- >/dev/null 2>&1\n\
fi\n\
if [ \"$AG\" = \"cursor\" ]; then echo '{\"continue\":true}'; else echo '{}'; fi\n\
exit 0\n";

pub fn script_text(token: &str, port: u16) -> String {
    SCRIPT_TEMPLATE.replace("@PORT@", &port.to_string()).replace("@TOKEN@", token)
}

pub fn script_path(app_data: &Path) -> PathBuf {
    app_data.join("agent_hooks").join(SCRIPT_NAME)
}

/// Write (or refresh) the script. Returns its path.
pub fn write_script(app_data: &Path, token: &str, port: u16) -> Result<PathBuf, HookError> {
    let path = script_path(app_data);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| HookError::Io(e.to_string()))?;
    }
    std::fs::write(&path, script_text(token, port)).map_err(|e| HookError::Io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(path)
}

// ─── Editing settings files that hold a map of hook lists (Claude Code, Gemini CLI, Cursor) ──────

/// (agent event, our kind, matcher). `Notification` for Claude Code is limited to prompts that really wait for the
/// user: "idle_prompt" also fires after a finished turn and would look like "needs you" forever.
type HookEvents = &'static [(&'static str, &'static str, Option<&'static str>)];

const CLAUDE_EVENTS: HookEvents = &[
    ("Stop", "stop", None),
    ("Notification", "notification", Some("permission_prompt|elicitation_dialog")),
    ("UserPromptSubmit", "start", None),
];

/// Gemini CLI: `AfterAgent` fires when the agent loop completes and carries `prompt_response`.
const GEMINI_EVENTS: HookEvents = &[
    ("AfterAgent", "stop", None),
    ("BeforeAgent", "start", None),
    ("Notification", "notification", None),
];

/// Cursor: `stop` has a status but no text; the text comes from `afterAgentResponse`.
const CURSOR_EVENTS: HookEvents = &[
    ("stop", "stop", None),
    ("afterAgentResponse", "message", None),
    ("beforeSubmitPrompt", "start", None),
];

/// Windsurf / Devin Desktop: `post_cascade_response` runs after each reply (its `tool_info.response` is the text),
/// `pre_user_prompt` before a new prompt. Entries are flat `{ "command": ... }` objects.
const WINDSURF_EVENTS: HookEvents = &[("post_cascade_response", "stop", None), ("pre_user_prompt", "start", None)];

/// How one hook entry looks in that agent's file.
#[derive(Clone, Copy)]
enum Shape {
    /// `{ "matcher": ..., "hooks": [ { "type": "command", "command": ..., "timeout": N } ] }` (Claude Code, Gemini CLI)
    Nested { timeout: u64 },
    /// `{ "command": ... }` (Cursor)
    Flat,
}

/// The 8.3 short form of an existing path (no spaces), when Windows has one.
#[cfg(windows)]
fn short_path(p: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetShortPathNameW(long: *const u16, short: *mut u16, cch: u32) -> u32;
    }
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n]))
}

#[cfg(not(windows))]
fn short_path(_: &Path) -> Option<String> {
    None
}

/// Antigravity runs a hook command through `cmd /C` and escapes quotes the way Go does, which breaks any quoted
/// path ("\"C:\Users\me\x.cmd\"' is not recognized as an internal or external command", seen live with `agy`
/// 1.2.9). A path with no spaces needs no quotes, so a script path with a space is handed over in its short form.
fn unquoted_script(script: &Path) -> Result<String, HookError> {
    let long = script.display().to_string();
    let candidate = if long.contains(' ') { short_path(script).unwrap_or(long) } else { long };
    if candidate.contains(' ') || candidate.contains('"') {
        return Err(HookError::Io("the path to my hook script has a space in it and no short form".into()));
    }
    Ok(candidate)
}

/// The command written into an agent's hook file: `<script> <agent> <kind>`.
fn command_line(script: &Path, agent: &str, kind: &str) -> Result<String, HookError> {
    if agent == "antigravity" || agent == "windsurf" {
        // The 8.3 short path has no "nexus-agent-event" in it, so the marker rides along as a trailing argument
        // (the script reads only its first two): without it NEXUS could not recognise its own hook afterwards.
        return Ok(format!("{} {agent} {kind} {MARKER}", unquoted_script(script)?));
    }
    Ok(format!("\"{}\" {agent} {kind}", script.display()))
}

fn entry_is_ours(entry: &Value) -> bool {
    let nested = entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .map_or(false, |hs| hs.iter().any(|h| h.get("command").and_then(|c| c.as_str()).map_or(false, |c| c.contains(MARKER))));
    nested || entry.get("command").and_then(|c| c.as_str()).map_or(false, |c| c.contains(MARKER))
}

fn parse_settings(existing: &str) -> Result<Value, HookError> {
    if existing.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(existing).map_err(|e| HookError::Unreadable(e.to_string()))?;
    if !v.is_object() {
        return Err(HookError::Unreadable("settings is not a JSON object".into()));
    }
    Ok(v)
}

fn pretty(v: &Value) -> Result<String, HookError> {
    serde_json::to_string_pretty(v).map(|s| s + "\n").map_err(|e| HookError::Io(e.to_string()))
}

/// Add (or refresh) NEXUS's hooks, leaving every other setting and hook as it is.
fn hooks_map_add(existing: &str, script: &Path, agent: &str, events: HookEvents, shape: Shape, version: bool) -> Result<String, HookError> {
    let mut root = parse_settings(existing)?;
    let obj = root.as_object_mut().expect("checked object");
    if version {
        obj.entry("version").or_insert_with(|| json!(1));
    }
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().ok_or_else(|| HookError::Unreadable("\"hooks\" is not an object".into()))?;
    for (event, kind, matcher) in events {
        let list = hooks.entry(*event).or_insert_with(|| json!([]));
        let list = list.as_array_mut().ok_or_else(|| HookError::Unreadable(format!("\"{event}\" is not a list")))?;
        list.retain(|e| !entry_is_ours(e));
        let cmd = command_line(script, agent, kind)?;
        let mut entry = match shape {
            Shape::Nested { timeout } => json!({ "hooks": [ { "type": "command", "command": cmd, "timeout": timeout } ] }),
            Shape::Flat => json!({ "command": cmd }),
        };
        if let Some(m) = matcher {
            entry["matcher"] = json!(m);
        }
        list.push(entry);
    }
    pretty(&root)
}

/// Remove only NEXUS's entries. Returns the new text and whether anything was removed.
fn hooks_map_remove(existing: &str, events: HookEvents) -> Result<(String, bool), HookError> {
    let mut root = parse_settings(existing)?;
    let mut removed = false;
    if let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) {
        for (event, _, _) in events {
            if let Some(list) = hooks.get_mut(*event).and_then(|l| l.as_array_mut()) {
                let before = list.len();
                list.retain(|e| !entry_is_ours(e));
                removed |= list.len() != before;
            }
            // an event list we emptied is dropped, so the file goes back to how it was
            if hooks.get(*event).and_then(|l| l.as_array()).map_or(false, |l| l.is_empty()) {
                hooks.remove(*event);
            }
        }
    }
    if root.get("hooks").and_then(|h| h.as_object()).map_or(false, |h| h.is_empty()) {
        root.as_object_mut().expect("object").remove("hooks");
    }
    if !removed {
        return Ok((existing.to_string(), false));
    }
    Ok((pretty(&root)?, true))
}

fn hooks_map_has(existing: &str, events: HookEvents) -> bool {
    parse_settings(existing)
        .ok()
        .and_then(|r| r.get("hooks").and_then(|h| h.as_object()).cloned())
        .map_or(false, |hooks| {
            events.iter().all(|(event, _, _)| hooks.get(*event).and_then(|l| l.as_array()).map_or(false, |l| l.iter().any(entry_is_ours)))
        })
}

pub fn claude_add_hook(existing: &str, script: &Path) -> Result<String, HookError> {
    hooks_map_add(existing, script, "claude", CLAUDE_EVENTS, Shape::Nested { timeout: 10 }, false)
}

pub fn claude_remove_hook(existing: &str) -> Result<(String, bool), HookError> {
    hooks_map_remove(existing, CLAUDE_EVENTS)
}

pub fn claude_has_hook(existing: &str) -> bool {
    hooks_map_has(existing, CLAUDE_EVENTS)
}

/// Gemini CLI timeouts are in milliseconds.
pub fn gemini_add_hook(existing: &str, script: &Path) -> Result<String, HookError> {
    hooks_map_add(existing, script, "gemini", GEMINI_EVENTS, Shape::Nested { timeout: 10_000 }, false)
}

pub fn gemini_remove_hook(existing: &str) -> Result<(String, bool), HookError> {
    hooks_map_remove(existing, GEMINI_EVENTS)
}

pub fn gemini_has_hook(existing: &str) -> bool {
    hooks_map_has(existing, GEMINI_EVENTS)
}

pub fn cursor_add_hook(existing: &str, script: &Path) -> Result<String, HookError> {
    hooks_map_add(existing, script, "cursor", CURSOR_EVENTS, Shape::Flat, true)
}

pub fn cursor_remove_hook(existing: &str) -> Result<(String, bool), HookError> {
    hooks_map_remove(existing, CURSOR_EVENTS)
}

pub fn cursor_has_hook(existing: &str) -> bool {
    hooks_map_has(existing, CURSOR_EVENTS)
}

pub fn windsurf_add_hook(existing: &str, script: &Path) -> Result<String, HookError> {
    hooks_map_add(existing, script, "windsurf", WINDSURF_EVENTS, Shape::Flat, false)
}

pub fn windsurf_remove_hook(existing: &str) -> Result<(String, bool), HookError> {
    hooks_map_remove(existing, WINDSURF_EVENTS)
}

pub fn windsurf_has_hook(existing: &str) -> bool {
    hooks_map_has(existing, WINDSURF_EVENTS)
}

// ─── Antigravity: ~/.gemini/config/hooks.json (App, CLI and IDE share it) ───────────────────────────

/// The file is a map of NAMED hooks; ours is one entry of it. For events other than tool use the handlers sit directly
/// under the event key (Antigravity's hooks documentation). `Stop` fires when the execution loop terminates;
/// `PreInvocation` fires before each model call, which is how a new turn is noticed.
const ANTIGRAVITY_HOOK_NAME: &str = "nexus-agent-event";
const ANTIGRAVITY_EVENTS: &[(&str, &str)] = &[("Stop", "stop"), ("PreInvocation", "start")];

/// Hooks NEXUS wrote before the marker travelled as an argument: `<...\NEXUS-~1.CMD | ...\nexus-agent-event.cmd>
/// antigravity <stop|start>`, nothing else. Live 2026-10-19: the user's own file looked like this and NEXUS refused to
/// touch, find or remove it.
fn legacy_antigravity_command_is_ours(cmd: &str) -> bool {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    let [path, agent, kind] = parts[..] else { return false };
    let file = path.rsplit(|c| c == '\\' || c == '/').next().unwrap_or("").to_lowercase();
    let our_file = file == SCRIPT_NAME || (file.starts_with("nexus-~") && file.ends_with(".cmd"));
    our_file && agent == "antigravity" && matches!(kind, "stop" | "start")
}

fn antigravity_group_is_ours(v: &Value) -> bool {
    if v.to_string().contains(MARKER) {
        return true;
    }
    // legacy: every command in the group is one of ours, and there is at least one
    let Some(events) = v.as_object() else { return false };
    let commands: Vec<&str> = events
        .values()
        .filter_map(|l| l.as_array())
        .flatten()
        .filter_map(|e| e.get("command").and_then(|c| c.as_str()))
        .collect();
    !commands.is_empty() && commands.iter().all(|c| legacy_antigravity_command_is_ours(c))
}

pub fn antigravity_add_hook(existing: &str, script: &Path) -> Result<String, HookError> {
    let mut root = parse_settings(existing)?;
    let obj = root.as_object_mut().expect("checked object");
    if obj.get(ANTIGRAVITY_HOOK_NAME).map_or(false, |g| !antigravity_group_is_ours(g)) {
        return Err(HookError::Unreadable("a hook named nexus-agent-event that is not ours".into()));
    }
    let mut group = serde_json::Map::new();
    for (event, kind) in ANTIGRAVITY_EVENTS {
        group.insert((*event).to_string(), json!([ { "type": "command", "command": command_line(script, "antigravity", kind)?, "timeout": 10 } ]));
    }
    obj.insert(ANTIGRAVITY_HOOK_NAME.to_string(), Value::Object(group));
    pretty(&root)
}

pub fn antigravity_remove_hook(existing: &str) -> Result<(String, bool), HookError> {
    let mut root = parse_settings(existing)?;
    let ours = root.get(ANTIGRAVITY_HOOK_NAME).map_or(false, antigravity_group_is_ours);
    if !ours {
        return Ok((existing.to_string(), false));
    }
    root.as_object_mut().expect("object").remove(ANTIGRAVITY_HOOK_NAME);
    Ok((pretty(&root)?, true))
}

pub fn antigravity_has_hook(existing: &str) -> bool {
    parse_settings(existing).ok().map_or(false, |r| {
        r.get(ANTIGRAVITY_HOOK_NAME)
            .map_or(false, |g| antigravity_group_is_ours(g) && ANTIGRAVITY_EVENTS.iter().all(|(e, _)| g.get(*e).and_then(|l| l.as_array()).map_or(false, |l| !l.is_empty())))
    })
}

// ─── Editing Codex's config.toml (`notify`) ──────────────────────────

const CODEX_COMMENT: &str = "# NEXUS: tells NEXUS when Codex finishes a turn (remove with \"disconnect codex\")";

fn notify_line(script: &Path) -> Result<String, HookError> {
    let p = script.display().to_string();
    if p.contains('\'') || p.contains('\n') {
        return Err(HookError::Io("the script path contains a quote".into()));
    }
    // TOML literal strings (single quotes) need no escaping of backslashes
    Ok(format!("notify = ['{p}', 'codex', 'stop']"))
}

/// Index of the first `[table]` header line: top-level keys must stay before it.
fn first_table_line(lines: &[&str]) -> usize {
    lines.iter().position(|l| l.trim_start().starts_with('[')).unwrap_or(lines.len())
}

fn is_notify_key(line: &str) -> bool {
    let t = line.trim_start();
    t.strip_prefix("notify").map_or(false, |r| r.trim_start().starts_with('='))
}

pub fn codex_add_notify(existing: &str, script: &Path) -> Result<String, HookError> {
    let ours = notify_line(script)?;
    let lines: Vec<&str> = existing.lines().collect();
    let top = first_table_line(&lines);
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    for (i, l) in lines.iter().enumerate() {
        if i < top && is_notify_key(l) {
            if !l.contains(MARKER) {
                return Err(HookError::HasOwnNotify);
            }
            out.push(ours.clone());
            replaced = true;
        } else {
            out.push((*l).to_string());
        }
    }
    if !replaced {
        let mut head = vec![CODEX_COMMENT.to_string(), ours];
        head.extend(out);
        out = head;
    }
    Ok(out.join("\n") + "\n")
}

pub fn codex_remove_notify(existing: &str) -> (String, bool) {
    let lines: Vec<&str> = existing.lines().collect();
    let top = first_table_line(&lines);
    let mut out: Vec<&str> = Vec::new();
    let mut removed = false;
    for (i, l) in lines.iter().enumerate() {
        if i < top && is_notify_key(l) && l.contains(MARKER) {
            removed = true;
            if out.last() == Some(&CODEX_COMMENT) {
                out.pop();
            }
            continue;
        }
        out.push(l);
    }
    if !removed {
        return (existing.to_string(), false);
    }
    (out.join("\n") + "\n", true)
}

pub fn codex_has_notify(existing: &str) -> bool {
    let lines: Vec<&str> = existing.lines().collect();
    let top = first_table_line(&lines);
    lines.iter().take(top).any(|l| is_notify_key(l) && l.contains(MARKER))
}

// ─── Editing Aider's .aider.conf.yml (`notifications-command`) ───────────────────────────────────

const AIDER_COMMENT: &str = "# NEXUS: tells NEXUS when Aider finishes a reply (remove with \"disconnect aider\")";

/// The top-level YAML key of a line (no indentation, not a comment), if it has one.
fn yaml_key(line: &str) -> Option<&str> {
    if line.starts_with(' ') || line.starts_with('\t') || line.starts_with('#') {
        return None;
    }
    line.split_once(':').map(|(k, _)| k.trim())
}

fn is_aider_command_key(line: &str) -> bool {
    matches!(yaml_key(line), Some("notifications-command") | Some("notifications_command"))
}

fn is_aider_notifications_key(line: &str) -> bool {
    yaml_key(line) == Some("notifications")
}

/// Refuses when Aider already has notifications of its own (`notifications` or a command that is not ours):
/// it is the user's choice, not ours to change.
pub fn aider_add_notify(existing: &str, script: &Path) -> Result<String, HookError> {
    let cmd = format!("\"{}\" aider stop", script.display());
    if cmd.contains('\'') || cmd.contains('\n') {
        return Err(HookError::Io("the script path contains a quote".into()));
    }
    let ours_line = format!("notifications-command: '{cmd}'");
    let lines: Vec<&str> = existing.lines().collect();
    let has_ours = lines.iter().any(|l| is_aider_command_key(l) && l.contains(MARKER));
    if !has_ours && lines.iter().any(|l| is_aider_command_key(l) || is_aider_notifications_key(l)) {
        return Err(HookError::HasOwnNotify);
    }
    let mut out: Vec<String> = Vec::new();
    for l in &lines {
        if is_aider_command_key(l) && l.contains(MARKER) {
            out.push(ours_line.clone());
        } else {
            out.push((*l).to_string());
        }
    }
    if !has_ours {
        let mut head = vec![AIDER_COMMENT.to_string(), "notifications: true".to_string(), ours_line];
        head.extend(out);
        out = head;
    }
    Ok(out.join("\n") + "\n")
}

pub fn aider_remove_notify(existing: &str) -> (String, bool) {
    let lines: Vec<&str> = existing.lines().collect();
    if !lines.iter().any(|l| is_aider_command_key(l) && l.contains(MARKER)) {
        return (existing.to_string(), false);
    }
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l == AIDER_COMMENT {
            i += 1;
            if lines.get(i).map_or(false, |n| n.trim() == "notifications: true") {
                i += 1;
            }
            continue;
        }
        if is_aider_command_key(l) && l.contains(MARKER) {
            i += 1;
            continue;
        }
        out.push(l);
        i += 1;
    }
    (out.join("\n") + "\n", true)
}

pub fn aider_has_notify(existing: &str) -> bool {
    let lines: Vec<&str> = existing.lines().collect();
    lines.iter().any(|l| is_aider_command_key(l) && l.contains(MARKER)) && lines.iter().any(|l| l.trim() == "notifications: true")
}

// ─── OpenCode: a plugin file of its own ──────────────────────────────────────────────────────────

/// OpenCode loads every file in `~/.config/opencode/plugins/` at startup, so NEXUS owns one file there and never
/// edits the user's config. The plugin posts `session.idle` (with the last assistant text, read through the SDK client
/// it is given), permission prompts and new user messages. Sub-agent sessions (they have a `parentID`) are ignored,
/// so a helper finishing is not mistaken for the whole session finishing.
const OPENCODE_PLUGIN_TEMPLATE: &str = r###"// NEXUS agent hook (nexus-agent-event): tells NEXUS when OpenCode stops. Remove with "disconnect opencode".
const ENDPOINT = "http://127.0.0.1:@PORT@/agent-event";
const TOKEN = "@TOKEN@";

const post = async (event, body) => {
  try {
    await fetch(ENDPOINT + "?agent=opencode&event=" + event, {
      method: "POST",
      headers: { "Authorization": "Bearer " + TOKEN, "Content-Type": "application/json" },
      body: JSON.stringify(body || {}),
      signal: AbortSignal.timeout(3000),
    });
  } catch (_) {}
};

export const NexusAgentEvent = async ({ client }) => {
  const isSubSession = async (id) => {
    try {
      const res = await client.session.get({ path: { id } });
      return !!(res && res.data && res.data.parentID);
    } catch (_) {
      return false;
    }
  };
  const lastAssistantText = async (id) => {
    try {
      const res = await client.session.messages({ path: { id } });
      const msgs = (res && res.data) || [];
      for (let i = msgs.length - 1; i >= 0; i--) {
        const m = msgs[i];
        if (!m.info || m.info.role !== "assistant") continue;
        const text = (m.parts || [])
          .filter((p) => p.type === "text" && !p.synthetic && !p.ignored)
          .map((p) => p.text)
          .join("\n")
          .trim();
        if (text) return text;
      }
    } catch (_) {}
    return "";
  };
  return {
    event: async ({ event }) => {
      if (!event || !event.type) return;
      const props = event.properties || {};
      if (event.type === "session.idle") {
        const id = props.sessionID;
        if (id && (await isSubSession(id))) return;
        await post("stop", { last_assistant_message: id ? await lastAssistantText(id) : "" });
      } else if (event.type === "permission.updated" || event.type === "permission.asked") {
        await post("notification", {});
      } else if (event.type === "message.updated" && props.info && props.info.role === "user") {
        await post("start", {});
      }
    },
  };
};
"###;

pub fn opencode_plugin_text(token: &str, port: u16) -> String {
    OPENCODE_PLUGIN_TEMPLATE.replace("@PORT@", &port.to_string()).replace("@TOKEN@", token)
}

/// Kilo Code is built on OpenCode and loads every file in `~/.config/kilo/plugin/`. Its documentation shows
/// `export default { id, server }`; the events are the same (`session.idle`, `permission.asked`, user messages).
/// Not run against a real Kilo yet.
const KILO_PLUGIN_TEMPLATE: &str = r###"// NEXUS agent hook (nexus-agent-event): tells NEXUS when Kilo stops. Remove with "disconnect kilo".
const ENDPOINT = "http://127.0.0.1:@PORT@/agent-event";
const TOKEN = "@TOKEN@";

const post = async (event, body) => {
  try {
    await fetch(ENDPOINT + "?agent=kilo&event=" + event, {
      method: "POST",
      headers: { "Authorization": "Bearer " + TOKEN, "Content-Type": "application/json" },
      body: JSON.stringify(body || {}),
      signal: AbortSignal.timeout(3000),
    });
  } catch (_) {}
};

const server = async ({ client }) => {
  const isSubSession = async (id) => {
    try {
      const res = await client.session.get({ path: { id } });
      return !!(res && res.data && res.data.parentID);
    } catch (_) {
      return false;
    }
  };
  const lastAssistantText = async (id) => {
    try {
      const res = await client.session.messages({ path: { id } });
      const msgs = (res && res.data) || [];
      for (let i = msgs.length - 1; i >= 0; i--) {
        const m = msgs[i];
        if (!m.info || m.info.role !== "assistant") continue;
        const text = (m.parts || [])
          .filter((p) => p.type === "text" && !p.synthetic && !p.ignored)
          .map((p) => p.text)
          .join("\n")
          .trim();
        if (text) return text;
      }
    } catch (_) {}
    return "";
  };
  return {
    event: async ({ event }) => {
      if (!event || !event.type) return;
      const props = event.properties || {};
      if (event.type === "session.idle") {
        const id = props.sessionID;
        if (id && (await isSubSession(id))) return;
        await post("stop", { last_assistant_message: id ? await lastAssistantText(id) : "" });
      } else if (event.type === "permission.updated" || event.type === "permission.asked") {
        await post("notification", {});
      } else if (event.type === "message.updated" && props.info && props.info.role === "user") {
        await post("start", {});
      }
    },
  };
};

export default { id: "nexus-agent-event", server };
"###;

pub fn kilo_plugin_text(token: &str, port: u16) -> String {
    KILO_PLUGIN_TEMPLATE.replace("@PORT@", &port.to_string()).replace("@TOKEN@", token)
}

/// GitHub Copilot CLI reads every `*.json` in `~/.copilot/hooks/`: NEXUS owns one file there. `agentStop` is the end
/// of a turn, `userPromptSubmitted` a new one, `notification` a prompt that waits. The payload names a
/// `transcriptPath` but its format is not read yet, so a Copilot stop carries no text. Not run against a real
/// Copilot CLI yet.
pub fn copilot_hook_text(script: &Path) -> Result<String, HookError> {
    let p = script.display().to_string();
    if p.contains('\'') || p.contains('\n') {
        return Err(HookError::Io("the script path contains a quote".into()));
    }
    let entry = |kind: &str| {
        json!({
            "type": "command",
            "powershell": format!("& '{p}' copilot {kind} {MARKER}"),
            "bash": format!("'{p}' copilot {kind} {MARKER}"),
            "timeoutSec": 10
        })
    };
    pretty(&json!({
        "version": 1,
        "hooks": {
            "agentStop": [entry("stop")],
            "userPromptSubmitted": [entry("start")],
            "notification": [entry("notification")]
        }
    }))
}

fn opencode_install(plugin: &Path, t: &HookTarget) -> Result<Installed, HookError> {
    owned_file_install(plugin, &opencode_plugin_text(t.token, t.port))
}

/// A file NEXUS owns outright (a plugin, a hook file in a drop-in folder). Refuses to overwrite a file of that name
/// that is not ours.
fn owned_file_install(plugin: &Path, text: &str) -> Result<Installed, HookError> {
    let existing = match std::fs::read_to_string(plugin) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(HookError::Unreadable(e.to_string())),
    };
    if existing.as_deref().map_or(false, |s| !s.contains(MARKER)) {
        return Err(HookError::Unreadable("a plugin file with that name is not ours".into()));
    }
    if existing.as_deref() == Some(text) {
        return Ok(Installed { backup: None });
    }
    if let Some(dir) = plugin.parent() {
        std::fs::create_dir_all(dir).map_err(|e| HookError::Io(e.to_string()))?;
    }
    std::fs::write(plugin, text).map_err(|e| HookError::Io(e.to_string()))?;
    Ok(Installed { backup: None })
}

// ─── Files ───────────────────────────────────────────────────────────

fn home() -> Option<PathBuf> {
    dirs_next::home_dir()
}

/// Agents that look installed on this machine: the folder their tool keeps its settings in exists (Aider: its
/// config file, Gemini CLI: its settings file). Pure over `home`, so it can be tested.
pub fn agents_present_in(home: &Path) -> Vec<Agent> {
    let dir = |rel: &str| home.join(rel).is_dir();
    let file = |rel: &str| home.join(rel).is_file();
    ALL_AGENTS
        .iter()
        .copied()
        .filter(|a| match a {
            Agent::Claude => dir(".claude"),
            Agent::Codex => dir(".codex"),
            Agent::Antigravity => dir(".gemini/config") || dir(".gemini/antigravity") || dir(".gemini/antigravity-cli"),
            Agent::Gemini => file(".gemini/settings.json"),
            Agent::Cursor => dir(".cursor"),
            Agent::Aider => file(".aider.conf.yml") || dir(".aider"),
            Agent::OpenCode => dir(".config/opencode"),
            Agent::Kilo => dir(".config/kilo") || dir(".kilocode"),
            Agent::Windsurf => dir(".codeium/windsurf"),
            Agent::Copilot => dir(".copilot"),
            Agent::Devin => dir(".config/devin") || dir("AppData/Roaming/devin"),
            Agent::Trae => dir(".trae") || dir("AppData/Roaming/Trae"),
        })
        .collect()
}

/// `agents_present_in` for the real home folder.
pub fn agents_present() -> Vec<Agent> {
    home().map(|h| agents_present_in(&h)).unwrap_or_default()
}

/// The agents "hook up all agents" would touch: present ones that have a hook of their own to install (not Devin,
/// which uses Claude's, and not Trae, which has none).
pub fn hookable_present() -> Vec<Agent> {
    agents_present().into_iter().filter(|a| !a.observe_only() && *a != Agent::Devin).collect()
}

/// The file NEXUS edits (or, for OpenCode, owns) for this agent.
pub fn config_path(agent: Agent) -> Option<PathBuf> {
    match agent {
        Agent::Claude => {
            let base = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| home().map(|h| h.join(".claude")))?;
            Some(base.join("settings.json"))
        }
        Agent::Codex => {
            let base = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".codex")))?;
            Some(base.join("config.toml"))
        }
        Agent::Antigravity => Some(home()?.join(".gemini").join("config").join("hooks.json")),
        Agent::Gemini => Some(home()?.join(".gemini").join("settings.json")),
        Agent::Cursor => Some(home()?.join(".cursor").join("hooks.json")),
        Agent::Aider => Some(home()?.join(".aider.conf.yml")),
        Agent::OpenCode => {
            let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")))?;
            Some(base.join("opencode").join("plugins").join(OPENCODE_PLUGIN_FILE))
        }
        // Devin reads Claude Code's settings, so the file is Claude's
        Agent::Devin => config_path(Agent::Claude),
        Agent::Windsurf => Some(home()?.join(".codeium").join("windsurf").join("hooks.json")),
        Agent::Kilo => {
            let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".config")))?;
            Some(base.join("kilo").join("plugin").join(OPENCODE_PLUGIN_FILE))
        }
        Agent::Copilot => {
            let base = std::env::var_os("COPILOT_HOME").map(PathBuf::from).or_else(|| home().map(|h| h.join(".copilot")))?;
            Some(base.join("hooks").join("nexus-agent-event.json"))
        }
        Agent::Trae => None,
    }
}

const OPENCODE_PLUGIN_FILE: &str = "nexus-agent-event.js";

fn read_or_empty(path: &Path) -> Result<String, HookError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(HookError::Unreadable(e.to_string())),
    }
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Backup (if the file exists), then write atomically. Returns the backup path.
fn write_with_backup(path: &Path, new_text: &str) -> Result<Option<PathBuf>, HookError> {
    let mut backup = None;
    if path.exists() {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let b = path.with_file_name(format!("{name}.nexus-backup-{}", unix_secs()));
        std::fs::copy(path, &b).map_err(|e| HookError::Io(format!("backup failed: {e}")))?;
        backup = Some(b);
    } else if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| HookError::Io(e.to_string()))?;
    }
    let tmp = path.with_extension("nexus-tmp");
    std::fs::write(&tmp, new_text).map_err(|e| HookError::Io(e.to_string()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        HookError::Io(e.to_string())
    })?;
    Ok(backup)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Installed {
    pub backup: Option<PathBuf>,
}

/// What an installer needs besides the file: the script path (most agents run it) and, for OpenCode's plugin
/// (which posts by itself), the endpoint token and port.
pub struct HookTarget<'a> {
    pub script: &'a Path,
    pub token: &'a str,
    pub port: u16,
}

/// Install the hook for `agent` into `cfg` (the script must already exist at `target.script`).
pub fn install(agent: Agent, cfg: &Path, target: &HookTarget) -> Result<Installed, HookError> {
    match agent {
        Agent::OpenCode => return opencode_install(cfg, target),
        Agent::Kilo => return owned_file_install(cfg, &kilo_plugin_text(target.token, target.port)),
        Agent::Copilot => return owned_file_install(cfg, &copilot_hook_text(target.script)?),
        Agent::Trae => return Err(HookError::NoHookMechanism),
        _ => {}
    }
    let existing = read_or_empty(cfg)?;
    let script = target.script;
    let updated = match agent {
        Agent::Claude | Agent::Devin => claude_add_hook(&existing, script)?,
        Agent::Windsurf => windsurf_add_hook(&existing, script)?,
        Agent::Codex => codex_add_notify(&existing, script)?,
        Agent::Antigravity => antigravity_add_hook(&existing, script)?,
        Agent::Gemini => gemini_add_hook(&existing, script)?,
        Agent::Cursor => cursor_add_hook(&existing, script)?,
        Agent::Aider => aider_add_notify(&existing, script)?,
        Agent::OpenCode | Agent::Kilo | Agent::Copilot | Agent::Trae => unreachable!("handled above"),
    };
    if updated == existing {
        return Ok(Installed { backup: None });
    }
    Ok(Installed { backup: write_with_backup(cfg, &updated)? })
}

/// Remove NEXUS's hook. `Ok(false)` = there was none.
pub fn uninstall(agent: Agent, cfg: &Path) -> Result<bool, HookError> {
    if !cfg.exists() {
        return Ok(false);
    }
    // Devin has no hook of its own (it shares Claude's), and Trae has none at all: nothing to remove
    if matches!(agent, Agent::Devin | Agent::Trae) {
        return Ok(false);
    }
    if matches!(agent, Agent::OpenCode | Agent::Kilo | Agent::Copilot) {
        let text = read_or_empty(cfg)?;
        if !text.contains(MARKER) {
            return Ok(false);
        }
        std::fs::remove_file(cfg).map_err(|e| HookError::Io(e.to_string()))?;
        return Ok(true);
    }
    let existing = read_or_empty(cfg)?;
    let (updated, removed) = match agent {
        Agent::Claude => claude_remove_hook(&existing)?,
        Agent::Windsurf => windsurf_remove_hook(&existing)?,
        Agent::Codex => codex_remove_notify(&existing),
        Agent::Antigravity => antigravity_remove_hook(&existing)?,
        Agent::Gemini => gemini_remove_hook(&existing)?,
        Agent::Cursor => cursor_remove_hook(&existing)?,
        Agent::Aider => aider_remove_notify(&existing),
        Agent::OpenCode | Agent::Kilo | Agent::Copilot | Agent::Devin | Agent::Trae => unreachable!("handled above"),
    };
    if removed {
        write_with_backup(cfg, &updated)?;
    }
    Ok(removed)
}

pub fn is_installed(agent: Agent, cfg: &Path) -> bool {
    let Ok(text) = read_or_empty(cfg) else { return false };
    match agent {
        Agent::Claude | Agent::Devin => claude_has_hook(&text),
        Agent::Windsurf => windsurf_has_hook(&text),
        Agent::Trae => false,
        Agent::Codex => codex_has_notify(&text),
        Agent::Antigravity => antigravity_has_hook(&text),
        Agent::Gemini => gemini_has_hook(&text),
        Agent::Cursor => cursor_has_hook(&text),
        Agent::Aider => aider_has_notify(&text),
        Agent::OpenCode | Agent::Kilo | Agent::Copilot => text.contains(MARKER),
    }
}

// ─── Events ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub agent: String,
    pub kind: EventKind,
    pub at_ms: u64,
    /// The agent's last message, for a Stop or a Message (already cut to the last 4 KB and masked).
    pub tail: Option<String>,
    pub info: StopInfo,
}

static EVENTS: Mutex<VecDeque<AgentEvent>> = Mutex::new(VecDeque::new());

/// Where a Stop event's text has to be read from when the agent did not send it in the event itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailSource {
    /// Antigravity's `transcriptPath` (a jsonl log).
    Transcript(String),
    /// Aider's working directory (its `.aider.chat.history.md` is read from there).
    AiderCwd(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEvent {
    pub agent: Agent,
    pub kind: EventKind,
    pub tail: Option<String>,
    pub info: StopInfo,
    pub source: Option<TailSource>,
    /// The folder the agent works in, when its hook says (`cwd`, or the first of Antigravity's `workspacePaths`).
    pub cwd: Option<String>,
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
}

fn kind_from(s: &str) -> Option<EventKind> {
    Some(match s {
        "stop" => EventKind::Stop,
        "notification" => EventKind::Notification,
        "start" => EventKind::Start,
        "message" => EventKind::Message,
        _ => return None,
    })
}

/// The agent's final text in the JSON its hook receives: Claude Code `last_assistant_message`, Codex's notify
/// payload `last-assistant-message`, Gemini CLI's AfterAgent `prompt_response`; Cursor's `afterAgentResponse`
/// (a Message event) carries `text`.
fn final_message(v: &Value, kind: EventKind) -> Option<String> {
    // Windsurf / Devin Desktop `post_cascade_response`: the reply is `tool_info.response`
    if let Some(r) = v.get("tool_info").and_then(|t| t.get("response")).and_then(|x| x.as_str()) {
        return Some(r.to_string());
    }
    let mut keys = vec!["last_assistant_message", "last-assistant-message", "prompt_response"];
    if kind == EventKind::Message {
        keys.push("text");
    }
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(String::from))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// `cwd=C%3A%5CUsers%5Cme%5Cproj` -> the decoded value (Aider's notification script posts a form body).
fn form_value(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| percent_decode(v)))
}

/// What the agent itself says about how it stopped. Only ever makes the verdict stricter.
fn stop_info(agent: Agent, v: &Value) -> StopInfo {
    let mut info = StopInfo::default();
    match agent {
        Agent::Antigravity => {
            let error = v.get("error").map_or(false, |e| match e {
                Value::String(s) => !s.trim().is_empty(),
                Value::Null => false,
                Value::Bool(b) => *b,
                _ => true,
            });
            let reason = v.get("terminationReason").and_then(|r| r.as_str()).unwrap_or("").to_lowercase();
            let bad_reason = ["error", "fail", "abort", "cancel", "limit", "exceed", "crash"].iter().any(|w| reason.contains(w));
            if error || bad_reason {
                info.ok = Some(false);
            }
            info.fully_idle = v.get("fullyIdle").and_then(|b| b.as_bool());
        }
        Agent::Cursor => {
            info.ok = match v.get("status").and_then(|s| s.as_str()) {
                Some("completed") => Some(true),
                Some(_) => Some(false),
                None => None,
            };
        }
        _ => {}
    }
    info
}

/// Devin CLI reads Claude Code's hook settings, so Claude's hook reports Devin's events as "claude". Devin's payload
/// carries a `prompt_id` and no `transcript_path` (Claude Code's carries a transcript path). Taken from Devin's
/// documentation, not yet seen on a real Devin: if Claude's payload ever carried `prompt_id` this would mislabel it.
fn relabel(agent: Agent, json: Option<&Value>) -> Agent {
    match (agent, json) {
        (Agent::Claude, Some(v)) if v.get("prompt_id").is_some() && v.get("transcript_path").is_none() => Agent::Devin,
        _ => agent,
    }
}

/// `?agent=claude&event=stop` (new script) or a body `{"agent":"claude","event":"stop"}` (old script) ->
/// the event. Only known agents and four known events are accepted. For a stop, the agent's final message is kept
/// as DATA for the verdict (cut to the last 4 KB and masked); nothing else in the body is read or stored beyond
/// the stop status and (Antigravity, Aider) the place to read the final text from; no text ever becomes a command.
pub fn parse_event(query: &str, body: &str) -> Option<ParsedEvent> {
    let json: Option<Value> = serde_json::from_str(body).ok();
    let (agent, kind) = match (query_value(query, "agent"), query_value(query, "event")) {
        (Some(a), Some(e)) => (Agent::from_name(a)?, kind_from(e)?),
        (None, None) => {
            let v = json.as_ref()?;
            (Agent::from_name(v.get("agent")?.as_str()?)?, kind_from(v.get("event")?.as_str()?)?)
        }
        _ => return None,
    };
    let agent = relabel(agent, json.as_ref());
    let mut tail = None;
    let mut info = StopInfo::default();
    let mut source = None;
    if matches!(kind, EventKind::Stop | EventKind::Message) {
        tail = json
            .as_ref()
            .and_then(|v| final_message(v, kind))
            .map(|m| crate::session_verdict::prepare_tail(&m))
            .filter(|t| !t.trim().is_empty());
        if kind == EventKind::Stop {
            if let Some(v) = json.as_ref() {
                info = stop_info(agent, v);
            }
            if tail.is_none() {
                match agent {
                    Agent::Antigravity => {
                        source = json.as_ref().and_then(|v| v.get("transcriptPath")).and_then(|p| p.as_str()).map(|p| TailSource::Transcript(p.to_string()));
                    }
                    Agent::Aider => {
                        source = form_value(body, "cwd").filter(|c| !c.trim().is_empty()).map(TailSource::AiderCwd);
                    }
                    _ => {}
                }
            }
        }
    }
    let cwd = json.as_ref().and_then(|v| {
        v.get("cwd")
            .and_then(|c| c.as_str())
            .or_else(|| v.get("workspacePaths").and_then(|w| w.as_array()).and_then(|a| a.first()).and_then(|p| p.as_str()))
            .map(String::from)
    });
    let cwd = cwd.or_else(|| form_value(body, "cwd")).filter(|c| !c.trim().is_empty());
    Some(ParsedEvent { agent, kind, tail, info, source, cwd })
}

static CWDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Remember the folder an agent reported it works in (the latest one wins).
pub fn note_cwd(agent: Agent, cwd: &str) {
    if let Ok(mut g) = CWDS.lock() {
        g.retain(|(a, _)| a != agent.name());
        g.push((agent.name().to_string(), cwd.to_string()));
    }
}

/// The folder the agent last reported working in.
pub fn last_cwd(agent: Agent) -> Option<String> {
    CWDS.lock().ok().and_then(|g| g.iter().find(|(a, _)| a == agent.name()).map(|(_, c)| c.clone()))
}

// ─── Reading the final text from a file the agent names ──────────────

/// At most this much is read from the END of a transcript / history file (they can be tens of megabytes).
const TAIL_READ_BYTES: u64 = 256 * 1024;

fn read_tail_bytes(path: &Path, max: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.take(max).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf).to_string();
    // a read that starts mid-file begins in the middle of a line: drop that line
    Some(if start > 0 { text.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default() } else { text })
}

/// The file `raw` names, only if it is a regular file with extension `ext` that lies under `root`
/// (`~/` is expanded). A path that escapes `root` (`..`, a symlink, another drive) gives `None`.
fn safe_file_under(raw: &str, root: &Path, home: &Path, ext: &str) -> Option<PathBuf> {
    let expanded = match raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(raw),
    };
    let file = expanded.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    let ok = file.starts_with(&root) && file.is_file() && file.extension().map_or(false, |e| e.eq_ignore_ascii_case(ext));
    ok.then_some(file)
}

/// Antigravity's `transcript.jsonl`: the final reply is the last `PLANNER_RESPONSE` that has text and no tool calls.
/// The second value is true when an `ERROR_MESSAGE` came after it (the turn ended in an error).
fn transcript_final(text: &str) -> (Option<String>, bool) {
    let mut error_after = false;
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("ERROR_MESSAGE") => error_after = true,
            Some("USER_INPUT") => return (None, error_after),
            Some("PLANNER_RESPONSE") => {
                let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("").trim();
                let has_tools = v.get("tool_calls").and_then(|t| t.as_array()).map_or(false, |a| !a.is_empty());
                if !content.is_empty() && !has_tools {
                    return (Some(content.to_string()), error_after);
                }
            }
            _ => {}
        }
    }
    (None, error_after)
}

/// The assistant's last reply in `.aider.chat.history.md`: what follows the last `#### ` (user) line, without the
/// `> ` lines Aider uses for its own status output.
fn aider_last_reply(history: &str) -> Option<String> {
    let lines: Vec<&str> = history.lines().collect();
    let start = lines.iter().rposition(|l| l.starts_with("#### "))?;
    let reply: Vec<&str> = lines[start + 1..]
        .iter()
        .filter(|l| !l.starts_with('>') && !l.starts_with("# aider chat started"))
        .copied()
        .collect();
    let text = reply.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Fill in the final text (and, for Antigravity, whether the turn ended in an error) from the file the agent
/// named. Any failure leaves the event without text, which can only make the verdict stricter.
pub fn resolve_tail(ev: &mut ParsedEvent) {
    resolve_tail_with(ev, home().as_deref());
}

fn resolve_tail_with(ev: &mut ParsedEvent, home: Option<&Path>) {
    let Some(src) = ev.source.take() else { return };
    let Some(home) = home else { return };
    let text = match src {
        TailSource::Transcript(raw) => {
            let Some(file) = safe_file_under(&raw, &home.join(".gemini"), home, "jsonl") else { return };
            let Some(raw_text) = read_tail_bytes(&file, TAIL_READ_BYTES) else { return };
            let (reply, error_after) = transcript_final(&raw_text);
            if error_after {
                ev.info.ok = Some(false);
            }
            reply
        }
        TailSource::AiderCwd(cwd) => {
            let dir = PathBuf::from(&cwd);
            if !dir.is_absolute() || !dir.is_dir() {
                return;
            }
            let file = dir.join(".aider.chat.history.md");
            if !file.is_file() {
                return;
            }
            read_tail_bytes(&file, TAIL_READ_BYTES).and_then(|t| aider_last_reply(&t))
        }
    };
    ev.tail = text.map(|t| crate::session_verdict::prepare_tail(&t)).filter(|t| !t.trim().is_empty());
}

pub fn record_event_at(agent: Agent, kind: EventKind, at_ms: u64, tail: Option<String>, info: StopInfo) {
    if let Ok(mut q) = EVENTS.lock() {
        // Antigravity posts a "start" before every model call: one per agent is enough, so they never crowd out a stop.
        if kind == EventKind::Start {
            if let Some(last) = q.iter_mut().rev().find(|e| e.agent == agent.name()) {
                if last.kind == EventKind::Start {
                    last.at_ms = at_ms;
                    return;
                }
            }
        }
        q.push_back(AgentEvent { agent: agent.name().to_string(), kind, at_ms, tail, info });
        while q.iter().filter(|e| e.agent == agent.name()).count() > MAX_EVENTS_PER_AGENT {
            if let Some(i) = q.iter().position(|e| e.agent == agent.name()) {
                q.remove(i);
            }
        }
        while q.len() > MAX_EVENTS {
            q.pop_front();
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn last_index(q: &VecDeque<AgentEvent>, agent: Agent, kind: EventKind) -> Option<usize> {
    q.iter().rposition(|e| e.agent == agent.name() && e.kind == kind)
}

/// When did this agent last report this kind of event?
pub fn last_event_ms(agent: Agent, kind: EventKind) -> Option<u64> {
    EVENTS.lock().ok().and_then(|q| last_index(&q, agent, kind).map(|i| q[i].at_ms))
}

/// Has this agent stopped (at or after `since_ms`) with no new prompt since? A new prompt means a new turn
/// is running, so the older stop no longer says "finished".
pub fn stop_is_latest(agent: Agent, since_ms: u64) -> bool {
    EVENTS.lock().map_or(false, |q| match last_index(&q, agent, EventKind::Stop) {
        Some(stop) => q[stop].at_ms >= since_ms && last_index(&q, agent, EventKind::Start).map_or(true, |start| stop > start),
        None => false,
    })
}

/// Has the agent reported anything (a prompt, a stop, a permission wait) at or after `since_ms`? That is how a plan
/// about "all my agents" learns which of them are doing something.
pub fn any_event_since(agent: Agent, since_ms: u64) -> bool {
    EVENTS.lock().map_or(false, |q| q.iter().any(|e| e.agent == agent.name() && e.at_ms >= since_ms))
}

/// How long the agent's Stop has been its latest event of any kind (no start, notice or message since), when it
/// arrived at or after `since_ms`. `None` = the agent did something after it, or never stopped.
pub fn hook_quiet_ms_at(agent: Agent, since_ms: u64, now_ms: u64) -> Option<u64> {
    let q = EVENTS.lock().ok()?;
    let last = q.iter().rposition(|e| e.agent == agent.name())?;
    (q[last].kind == EventKind::Stop && q[last].at_ms >= since_ms).then(|| now_ms.saturating_sub(q[last].at_ms))
}

/// How long ago the agent's latest stop arrived, while it is still the latest (no new prompt since).
pub fn stop_age_ms_at(agent: Agent, since_ms: u64, now_ms: u64) -> Option<u64> {
    if !stop_is_latest(agent, since_ms) {
        return None;
    }
    last_event_ms(agent, EventKind::Stop).map(|t| now_ms.saturating_sub(t))
}

/// The final message of the agent's latest stop: from the stop itself, or (Cursor) from the reply it sent just
/// before it, within the same turn.
pub fn last_stop_tail(agent: Agent) -> Option<String> {
    let q = EVENTS.lock().ok()?;
    let stop = last_index(&q, agent, EventKind::Stop)?;
    if let Some(t) = q[stop].tail.clone() {
        return Some(t);
    }
    let turn_start = (0..stop).rev().find(|&i| q[i].agent == agent.name() && q[i].kind == EventKind::Start).map_or(0, |s| s + 1);
    (turn_start..stop).rev().find(|&i| q[i].agent == agent.name() && q[i].kind == EventKind::Message).and_then(|i| q[i].tail.clone())
}

/// How the agent's latest stop says it stopped (all `None` when the agent does not say).
pub fn last_stop_info(agent: Agent) -> StopInfo {
    EVENTS.lock().ok().and_then(|q| last_index(&q, agent, EventKind::Stop).map(|i| q[i].info)).unwrap_or_default()
}

/// A permission prompt arrived (at or after `since_ms`) after the agent's latest stop and prompt: it is
/// waiting for the user.
pub fn needs_user(agent: Agent, since_ms: u64) -> bool {
    EVENTS.lock().map_or(false, |q| match last_index(&q, agent, EventKind::Notification) {
        Some(n) => {
            q[n].at_ms >= since_ms
                && last_index(&q, agent, EventKind::Stop).map_or(true, |i| n > i)
                && last_index(&q, agent, EventKind::Start).map_or(true, |i| n > i)
        }
        None => false,
    })
}

/// "5 minutes ago" for the status answer.
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..=59 => "moments ago".to_string(),
        60..=3599 => format!("{} minute{} ago", secs / 60, if secs / 60 == 1 { "" } else { "s" }),
        _ => format!("{} hour{} ago", secs / 3600, if secs / 3600 == 1 { "" } else { "s" }),
    }
}

// ─── Which agent is "this session"? ──────────────────────────────────

/// Does this (lower-case) window title name this agent? "Gemini" alone is a website, so Gemini CLI needs "cli".
fn title_names(agent: Agent, t: &str) -> bool {
    match agent {
        Agent::Antigravity => t.contains("antigravity") || t.split(|c: char| !c.is_alphanumeric()).any(|w| w == "agy"),
        Agent::OpenCode => t.contains("opencode") || t.contains("open code"),
        Agent::Gemini => t.contains("gemini cli") || t.contains("gemini-cli"),
        other => t.contains(other.name()),
    }
}

/// The one agent a window title names ("Antigravity", "OpenCode", "Cursor", "agy"). Two or none = `None`.
pub fn agent_in_title(title: &str) -> Option<Agent> {
    let t = title.to_lowercase();
    let found: Vec<Agent> = ALL_AGENTS.iter().copied().filter(|a| title_names(*a, &t)).collect();
    match found.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// A terminal's title rarely names the agent in it: find the one agent whose processes live below the foreground
/// window's process (or are it). Exactly one, else `None` (the caller asks the user which).
pub fn agent_under(foreground_pid: u32, all: &[crate::session_observer::ProcSnap], own_pid: u32) -> Option<Agent> {
    let below = crate::session_observer::descendants(&[foreground_pid], all);
    let found: Vec<Agent> = ALL_AGENTS
        .iter()
        .copied()
        .filter(|a| crate::session_observer::find_roots(Some(a.name()), all, None, own_pid).iter().any(|r| below.contains(r)))
        .collect();
    match found.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

// ─── What the endpoint answers (pure) ────────────────────────────────

/// The decision for `POST /agent-event`: (HTTP status, body, event to record).
pub fn route_event(authorized: bool, query: &str, body: &str) -> (u16, &'static str, Option<ParsedEvent>) {
    if !authorized {
        return (401, r#"{"error":"unauthorized"}"#, None);
    }
    match parse_event(query, body) {
        Some(ev) => (202, r#"{"ok":true}"#, Some(ev)),
        None => (400, r#"{"error":"bad event"}"#, None),
    }
}

// ─── Voice ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookAction {
    Connect,
    Disconnect,
    Status,
}

impl HookAction {
    pub fn as_str(self) -> &'static str {
        match self {
            HookAction::Connect => "connect",
            HookAction::Disconnect => "disconnect",
            HookAction::Status => "status",
        }
    }

    pub fn from_str(s: &str) -> Option<HookAction> {
        Some(match s {
            "connect" => HookAction::Connect,
            "disconnect" => HookAction::Disconnect,
            "status" => HookAction::Status,
            _ => return None,
        })
    }
}

fn norm(text: &str) -> String {
    let mut t = text
        .to_lowercase()
        .replace('\u{2019}', "'")
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    loop {
        let before = t.clone();
        for p in ["hey nexus ", "nexus ", "okay ", "ok ", "please ", "can you ", "could you ", "alright "] {
            if let Some(r) = t.strip_prefix(p) {
                t = r.to_string();
            }
        }
        for sfx in [" please"] {
            if let Some(r) = t.strip_suffix(sfx) {
                t = r.to_string();
            }
        }
        if t == before {
            return t;
        }
    }
}

const HOOK_VOCAB: &[&str] = &[
    "connect", "hook", "hooks", "up", "set", "setup", "install", "add", "enable", "the", "stop", "to", "nexus", "claude", "codex", "code",
    "antigravity", "agy", "opencode", "gemini", "cursor", "aider", "devin", "windsurf", "kilo", "copilot", "trae",
    "let", "know", "when", "stops", "finishes", "is", "done", "with", "notifications", "notification", "signal", "for", "my", "me", "a",
    "disconnect", "remove", "uninstall", "delete", "disable", "unhook", "take", "out", "of", "from", "connected", "installed", "status",
    "has", "reported", "yet", "been", "hooked", "on", "in", "set-up", "tell",
];

/// The one agent a sentence names ("agy" is Antigravity). Two different agents in one sentence = none.
fn find_agent(padded: &str) -> Option<Agent> {
    let found: Vec<Agent> = ALL_AGENTS
        .iter()
        .copied()
        .filter(|a| padded.contains(&format!(" {} ", a.name())) || (*a == Agent::Antigravity && padded.contains(" agy ")))
        .collect();
    match found.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// "connect claude's stop hook", "let nexus know when codex stops", "remove the claude hook", "is the
/// claude hook installed". Strict: every word must be in the hook vocabulary AND the sentence must name
/// the hook / stop / nexus, so "install claude" or "connect to claude" are never claimed.
pub fn parse_command(text: &str) -> Option<(HookAction, Agent)> {
    parse_command_sentence(text).or_else(|| last_sentence(text).and_then(parse_command_sentence))
}

/// When speech-to-text returns several sentences, the LAST one alone. Live 2026-10-19: Whisper continued the decoder
/// prompt and wrote `hook up anti-pilot. hook up anti-gravity.`: a junk sentence, then the real command. The strict
/// grammars still apply to that last sentence on its own.
pub(crate) fn last_sentence(text: &str) -> Option<&str> {
    let parts: Vec<&str> = text.split(|c| matches!(c, '.' | '!' | '?')).map(str::trim).filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        parts.last().copied()
    } else {
        None
    }
}

fn parse_command_sentence(text: &str) -> Option<(HookAction, Agent)> {
    // speech splits some names: "open code", "gemini cli"
    // Live (2026-10-19) Whisper wrote "hookup anti-gravity." and "hookup open code.": one word for "hook up", and
    // "anti gravity" for the agent's name.
    let t = format!(" {} ", norm(text))
        .replace(" hookup ", " hook up ")
        .replace(" anti gravity ", " antigravity ")
        .replace(" open code ", " opencode ")
        .replace(" gemini cli ", " gemini ")
        .replace(" kilo code ", " kilo ")
        .replace(" kilocode ", " kilo ")
        .replace(" github copilot ", " copilot ")
        .replace(" copilot cli ", " copilot ")
        .replace(" devin cli ", " devin ")
        .replace(" devin desktop ", " windsurf ")
        .trim()
        .to_string();
    let tokens: Vec<&str> = t.split(' ').collect();
    if tokens.is_empty() || !tokens.iter().all(|w| HOOK_VOCAB.contains(w)) {
        return None;
    }
    let p = format!(" {t} ");
    let agent = find_agent(&p)?;
    let context = [" hook ", " hooks ", " unhook ", " nexus ", " notification ", " notifications ", " signal ", " reported "].iter().any(|c| p.contains(c));
    if !context {
        return None;
    }
    let has = |ws: &[&str]| ws.iter().any(|w| p.contains(&format!(" {w} ")));
    if has(&["disconnect", "remove", "uninstall", "delete", "disable", "unhook"]) || p.contains(" take out ") {
        return Some((HookAction::Disconnect, agent));
    }
    if (has(&["is", "has", "status"]) && has(&["connected", "installed", "status", "reported", "hooked"])) || p.contains(" set up yet ") {
        return Some((HookAction::Status, agent));
    }
    if has(&["connect", "hook", "install", "add", "enable", "setup", "set-up", "let"]) || p.contains(" set up ") || p.contains(" hook up ") {
        return Some((HookAction::Connect, agent));
    }
    None
}

/// "hook up all agents", "connect every agent", "which agents are hooked up", "remove all the agent hooks". Strict like
/// `parse_command`: every word from the hook vocabulary plus the quantifier words, and it must say all/every/agents.
pub fn parse_all_command(text: &str) -> Option<HookAction> {
    parse_all_sentence(text).or_else(|| last_sentence(text).and_then(parse_all_sentence))
}

fn parse_all_sentence(text: &str) -> Option<HookAction> {
    const EXTRA: &[&str] = &["all", "every", "each", "agents", "agent", "ai", "coding", "assistants", "which", "are", "tools", "everything"];
    let t = format!(" {} ", norm(text)).replace(" hookup ", " hook up ").trim().to_string();
    let tokens: Vec<&str> = t.split(' ').collect();
    if tokens.is_empty() || !tokens.iter().all(|w| HOOK_VOCAB.contains(w) || EXTRA.contains(w)) {
        return None;
    }
    let p = format!(" {t} ");
    let has = |ws: &[&str]| ws.iter().any(|w| p.contains(&format!(" {w} ")));
    let quantified = has(&["all", "every", "each", "everything"]) && has(&["agents", "agent", "assistants", "tools"]);
    let which = has(&["which"]) && has(&["agents", "agent"]);
    if !(quantified || which) {
        return None;
    }
    let context = has(&["hook", "hooks", "unhook", "hooked", "connect", "connected", "disconnect", "remove", "uninstall", "delete", "disable", "install", "installed", "notification", "notifications", "nexus"])
        || p.contains(" set up ")
        || p.contains(" hook up ");
    if !context {
        return None;
    }
    if has(&["disconnect", "remove", "uninstall", "delete", "disable", "unhook"]) || p.contains(" take out ") {
        return Some(HookAction::Disconnect);
    }
    if which || has(&["status", "connected", "installed", "hooked"]) && has(&["are", "is", "which", "status"]) {
        return Some(HookAction::Status);
    }
    if has(&["connect", "hook", "install", "add", "enable", "setup", "set-up"]) || p.contains(" set up ") || p.contains(" hook up ") {
        return Some(HookAction::Connect);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script() -> PathBuf {
        PathBuf::from(r"C:\Users\me\AppData\Roaming\com.nexus.assistant\agent_hooks").join("nexus-agent-event.cmd")
    }

    fn tgt(script: &Path) -> HookTarget<'_> {
        HookTarget { script, token: "test-token", port: 39220 }
    }

    /// The event queue is process-global and capped (MAX_EVENTS): the cap test pushes enough events to evict other
    /// tests' events, so every test that records events takes this lock.
    fn events_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nexus_hooks_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_settings_get_both_hooks_and_keep_everything_else() {
        let before = r#"{"theme":"dark","permissions":{"allow":["Bash(ls)"]},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"echo hi"}]}],"Stop":[{"hooks":[{"type":"command","command":"my-own-stop.sh"}]}]}}"#;
        let after = claude_add_hook(before, &script()).unwrap();
        let v: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls)");
        assert_eq!(v["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "echo hi");
        // the user's own Stop hook is still there, ours is added beside it
        let stops = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0]["hooks"][0]["command"], "my-own-stop.sh");
        let ours = stops[1]["hooks"][0]["command"].as_str().unwrap();
        assert!(ours.contains(MARKER) && ours.ends_with("claude stop"), "{ours}");
        assert_eq!(v["hooks"]["Notification"][0]["hooks"][0]["command"].as_str().unwrap().ends_with("claude notification"), true);
        // only prompts that really wait for the user (idle_prompt would also fire after a finished turn)
        assert_eq!(v["hooks"]["Notification"][0]["matcher"], "permission_prompt|elicitation_dialog");
        // a new prompt is reported too, so an older stop cannot be mistaken for the end of the new turn
        assert!(v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"].as_str().unwrap().ends_with("claude start"));
        assert!(v["hooks"]["UserPromptSubmit"][0].get("matcher").is_none());
        assert!(claude_has_hook(&after));
        assert!(!claude_has_hook(before));
    }

    #[test]
    fn installing_twice_changes_nothing_and_a_moved_script_is_refreshed_not_duplicated() {
        let once = claude_add_hook("", &script()).unwrap();
        assert_eq!(claude_add_hook(&once, &script()).unwrap(), once);
        let moved = PathBuf::from(r"D:\elsewhere").join("nexus-agent-event.cmd");
        let twice = claude_add_hook(&once, &moved).unwrap();
        let v: Value = serde_json::from_str(&twice).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(v["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap().contains("elsewhere"));
    }

    #[test]
    fn removing_takes_out_only_ours_and_restores_the_file() {
        let own = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"my-own-stop.sh"}]}]}}"#;
        let with = claude_add_hook(own, &script()).unwrap();
        let (back, removed) = claude_remove_hook(&with).unwrap();
        assert!(removed);
        let v: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "my-own-stop.sh");
        assert!(v["hooks"].get("Notification").is_none());
        // a file that held only ours goes back to empty settings
        let only = claude_add_hook("{}", &script()).unwrap();
        let (empty, removed) = claude_remove_hook(&only).unwrap();
        assert!(removed);
        assert_eq!(serde_json::from_str::<Value>(&empty).unwrap(), json!({}));
        // nothing of ours: untouched, byte for byte
        let (same, removed) = claude_remove_hook(own).unwrap();
        assert!(!removed);
        assert_eq!(same, own);
    }

    #[test]
    fn a_settings_file_that_cannot_be_understood_is_never_overwritten() {
        for bad in ["{not json", "[1,2,3]", "\"text\"", r#"{"hooks":[]}"#, r#"{"hooks":{"Stop":"x"}}"#, r#"{"hooks":{"Notification":{}}}"#] {
            assert!(matches!(claude_add_hook(bad, &script()), Err(HookError::Unreadable(_))), "{bad}");
        }
        assert!(claude_remove_hook("{not json").is_err());
        assert!(!claude_has_hook("{not json"));
    }

    #[test]
    fn codex_gets_a_top_level_notify_line_before_any_table() {
        let before = "model = \"gpt-5\"\n\n[profiles.fast]\nmodel = \"x\"\n";
        let after = codex_add_notify(before, &script()).unwrap();
        let lines: Vec<&str> = after.lines().collect();
        let notify = lines.iter().position(|l| l.starts_with("notify")).unwrap();
        let table = lines.iter().position(|l| l.starts_with("[profiles")).unwrap();
        assert!(notify < table);
        assert!(after.contains("model = \"gpt-5\"") && after.contains("[profiles.fast]"));
        assert!(lines[notify].contains(MARKER) && lines[notify].contains("'codex', 'stop']"), "{}", lines[notify]);
        assert!(codex_has_notify(&after) && !codex_has_notify(before));
        // empty file
        assert!(codex_add_notify("", &script()).unwrap().contains("notify = ["));
    }

    #[test]
    fn codex_keeps_the_users_own_notify_and_edits_ours_in_place() {
        let own = "notify = [\"python\", \"notify.py\"]\n";
        assert_eq!(codex_add_notify(own, &script()), Err(HookError::HasOwnNotify));
        // a notify inside a [table] is a different key and does not count
        let in_table = "[tui]\nnotify = true\n";
        let after = codex_add_notify(in_table, &script()).unwrap();
        assert!(after.starts_with("# NEXUS") && after.contains("[tui]\nnotify = true"));
        // ours is rewritten, not duplicated
        let moved = PathBuf::from(r"D:\elsewhere").join("nexus-agent-event.cmd");
        let again = codex_add_notify(&after, &moved).unwrap();
        assert_eq!(again.matches("notify = [").count(), 1);
        assert!(again.contains("elsewhere"));
        // identical script: byte for byte the same
        assert_eq!(codex_add_notify(&after, &script()).unwrap(), after);
    }

    #[test]
    fn codex_removal_takes_only_our_line_and_its_comment() {
        let before = "model = \"x\"\n\n[tui]\nnotify = true\n";
        let with = codex_add_notify(before, &script()).unwrap();
        let (back, removed) = codex_remove_notify(&with);
        assert!(removed);
        assert_eq!(back, before);
        let own = "notify = [\"python\", \"notify.py\"]\n";
        let (same, removed) = codex_remove_notify(own);
        assert!(!removed && same == own);
        let bad = "notify = ['x']";
        assert!(codex_add_notify("", Path::new("it's/bad.cmd")).is_err());
        let _ = bad;
    }

    #[test]
    fn install_backs_up_writes_atomically_and_uninstall_restores() {
        let d = tmp("install");
        let cfg = d.join("settings.json");
        std::fs::write(&cfg, r#"{"theme":"dark"}"#).unwrap();
        let r = install(Agent::Claude, &cfg, &tgt(&script())).unwrap();
        let b = r.backup.expect("backup made");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), r#"{"theme":"dark"}"#);
        assert!(is_installed(Agent::Claude, &cfg));
        assert!(!cfg.with_extension("nexus-tmp").exists());
        // second install is a no-op (no new backup)
        assert_eq!(install(Agent::Claude, &cfg, &tgt(&script())).unwrap().backup, None);
        assert!(uninstall(Agent::Claude, &cfg).unwrap());
        assert!(!is_installed(Agent::Claude, &cfg));
        assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(&cfg).unwrap()).unwrap(), json!({"theme": "dark"}));
        assert!(!uninstall(Agent::Claude, &cfg).unwrap());
        // no file yet: created, no backup
        let fresh = d.join("sub").join("config.toml");
        assert_eq!(install(Agent::Codex, &fresh, &tgt(&script())).unwrap().backup, None);
        assert!(is_installed(Agent::Codex, &fresh));
        // unreadable file: error, file untouched
        let bad = d.join("bad.json");
        std::fs::write(&bad, "{nope").unwrap();
        assert!(install(Agent::Claude, &bad, &tgt(&script())).is_err());
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{nope");
        // uninstall with no file at all
        assert!(!uninstall(Agent::Codex, &d.join("missing.toml")).unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_script_carries_the_token_only_for_the_agent_event_endpoint() {
        let s = script_text("SECRETTOKEN", 39220);
        assert!(s.contains("SECRETTOKEN") && s.contains("/agent-event?agent=") && s.contains("127.0.0.1:39220"));
        assert!(s.contains("--data-binary @-"), "the agent's own stdin JSON is the body");
        assert!(!s.contains("/trigger"));
        let d = tmp("script");
        let p = write_script(&d, "T", 39220).unwrap();
        assert!(p.ends_with(SCRIPT_NAME) && p.exists());
        assert!(p.to_string_lossy().contains(MARKER));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_folder_an_agent_works_in_comes_from_its_hook_and_nothing_else() {
        let ev = |body: &str| route_event(true, "agent=claude&event=stop", body).2.unwrap();
        assert_eq!(ev(r#"{"cwd":"D:/proj"}"#).cwd.as_deref(), Some("D:/proj"));
        assert_eq!(ev(r#"{"cwd":"  "}"#).cwd, None);
        assert_eq!(ev("not json").cwd, None);
        let agy = route_event(true, "agent=antigravity&event=stop", r#"{"workspacePaths":["D:/a","D:/b"]}"#).2.unwrap();
        assert_eq!(agy.cwd.as_deref(), Some("D:/a"));
        note_cwd(Agent::Codex, "D:/one");
        note_cwd(Agent::Codex, "D:/two");
        assert_eq!(last_cwd(Agent::Codex).as_deref(), Some("D:/two"));
    }

    #[test]
    fn the_endpoint_accepts_only_an_authorised_known_agent_and_event() {
        let ev = |agent, kind| Some(ParsedEvent { agent, kind, tail: None, info: StopInfo::default(), source: None, cwd: None });
        // new script: agent and event in the URL
        assert_eq!(route_event(true, "agent=claude&event=stop", ""), (202, r#"{"ok":true}"#, ev(Agent::Claude, EventKind::Stop)));
        assert_eq!(route_event(true, "agent=codex&event=notification", "{}").2, ev(Agent::Codex, EventKind::Notification));
        assert_eq!(route_event(true, "agent=claude&event=start", "not json at all").2, ev(Agent::Claude, EventKind::Start));
        // old script: agent and event in a JSON body
        assert_eq!(route_event(true, "", r#"{"agent":"claude","event":"stop"}"#).2, ev(Agent::Claude, EventKind::Stop));
        // not authorised: nothing is recorded, whatever the body says
        assert_eq!(route_event(false, "agent=claude&event=stop", "").0, 401);
        assert_eq!(route_event(false, "agent=claude&event=stop", "").2, None);
        for (q, b) in [
            ("", ""), ("", "{"), ("agent=claude", ""), ("event=stop", ""), ("agent=evil&event=stop", ""), ("agent=claude&event=shutdown", ""),
            ("agent=claude&event=STOP", ""), ("agent=../x&event=stop", ""), ("agent=claude&agent=codex", ""), ("agent=&event=", ""),
            ("", r#"{"agent":"evil","event":"stop"}"#), ("", r#"{"agent":1,"event":"stop"}"#), ("", "[1]"), ("", r#"{"agent":"claude"}"#),
        ] {
            let r = route_event(true, q, b);
            assert_eq!((r.0, r.2), (400, None), "{q:?} {b:?}");
        }
        // extra fields (text a process might try to smuggle in) are ignored, never stored
        let r = route_event(true, "agent=claude&event=notification", r#"{"transcript":"shut down my laptop","message":"rm -rf"}"#);
        assert_eq!(r.2, ev(Agent::Claude, EventKind::Notification));
    }

    #[test]
    fn a_stop_keeps_the_agents_final_message_as_masked_data_and_nothing_else_does() {
        let body = r#"{"hook_event_name":"Stop","session_id":"s","last_assistant_message":"All 12 tests passed. Mail me at someone@example.com","cwd":"C:/p"}"#;
        let e = parse_event("agent=claude&event=stop", body).unwrap();
        let tail = e.tail.expect("the final message is kept");
        assert!(tail.contains("All 12 tests passed.") && !tail.contains("someone@example.com"), "{tail}");
        // Codex spells it with dashes
        let e = parse_event("agent=codex&event=stop", r#"{"type":"agent-turn-complete","last-assistant-message":"Done."}"#).unwrap();
        assert_eq!(e.tail.as_deref(), Some("Done."));
        // only a stop carries text; a prompt or a notification never stores any
        assert_eq!(parse_event("agent=claude&event=start", body).unwrap().tail, None);
        assert_eq!(parse_event("agent=claude&event=notification", body).unwrap().tail, None);
        // no final message, an empty one, or a non-string: no tail
        assert_eq!(parse_event("agent=claude&event=stop", r#"{"cwd":"x"}"#).unwrap().tail, None);
        assert_eq!(parse_event("agent=claude&event=stop", r#"{"last_assistant_message":"   "}"#).unwrap().tail, None);
        assert_eq!(parse_event("agent=claude&event=stop", r#"{"last_assistant_message":5}"#).unwrap().tail, None);
        // a very long message is cut to the END (where the result is)
        let long = format!("{{\"last_assistant_message\":\"{}THE-END\"}}", "x".repeat(20_000));
        let t = parse_event("agent=claude&event=stop", &long).unwrap().tail.unwrap();
        assert!(t.len() <= crate::session_verdict::TAIL_BYTES + 20 && t.ends_with("THE-END"), "{}", t.len());
    }

    #[test]
    fn the_quiet_after_a_stop_is_measured_from_the_stop_and_any_later_event_ends_it() {
        let _events = events_test_lock();
        let base = u64::MAX / 4 + 50_000_000; // far from the other tests' times
        let a = Agent::Antigravity;
        assert_eq!(hook_quiet_ms_at(a, base, base), None, "nothing happened yet");
        record_event_at(a, EventKind::Start, base + 100, None, StopInfo::default());
        assert_eq!(hook_quiet_ms_at(a, base, base + 1_000), None, "a running turn is not quiet");
        record_event_at(a, EventKind::Stop, base + 10_000, None, StopInfo::default());
        assert_eq!(hook_quiet_ms_at(a, base, base + 10_000), Some(0));
        assert_eq!(hook_quiet_ms_at(a, base, base + 55_000), Some(45_000));
        assert_eq!(stop_age_ms_at(a, base, base + 55_000), Some(45_000));
        assert_eq!(hook_quiet_ms_at(a, base + 10_001, base + 55_000), None, "that stop is older than `since`");
        // a permission notice, a message or a new prompt after the stop: no longer quiet
        for kind in [EventKind::Notification, EventKind::Message, EventKind::Start] {
            record_event_at(a, EventKind::Stop, base + 20_000, None, StopInfo::default());
            assert!(hook_quiet_ms_at(a, base, base + 90_000).is_some());
            record_event_at(a, kind, base + 30_000, None, StopInfo::default());
            assert_eq!(hook_quiet_ms_at(a, base, base + 90_000), None, "{kind:?}");
        }
        // another agent's activity does not matter
        record_event_at(Agent::Claude, EventKind::Start, base + 95_000, None, StopInfo::default());
        record_event_at(a, EventKind::Stop, base + 40_000, None, StopInfo::default());
        assert_eq!(hook_quiet_ms_at(a, base, base + 100_000), Some(60_000));
        assert!(Agent::Antigravity.is_ide() && Agent::Cursor.is_ide());
        assert!(!Agent::Claude.is_ide() && !Agent::Codex.is_ide() && !Agent::Aider.is_ide() && !Agent::OpenCode.is_ide() && !Agent::Gemini.is_ide());
    }

    #[test]
    fn stop_start_and_notification_events_are_ordered_per_agent() {
        let _events = events_test_lock();
        // other tests share the global list: use times far in the future
        let base = u64::MAX / 4;
        let a = Agent::Claude;
        let b = Agent::Codex;
        record_event_at(a, EventKind::Start, base + 100, None, StopInfo::default());
        assert!(!stop_is_latest(a, base), "a prompt without a stop is a running turn");
        record_event_at(a, EventKind::Stop, base + 1_000, Some("first tail".into()), StopInfo::default());
        assert!(stop_is_latest(a, base + 1_000));
        assert!(!stop_is_latest(a, base + 1_001), "the stop is older than `since`");
        assert_eq!(last_stop_tail(a).as_deref(), Some("first tail"));
        assert_eq!(last_event_ms(a, EventKind::Stop), Some(base + 1_000));
        // a new prompt: the old stop no longer says "finished"
        record_event_at(a, EventKind::Start, base + 2_000, None, StopInfo::default());
        assert!(!stop_is_latest(a, base));
        // and the next stop does, with its own message
        record_event_at(a, EventKind::Stop, base + 3_000, Some("second tail".into()), StopInfo::default());
        assert!(stop_is_latest(a, base));
        assert_eq!(last_stop_tail(a).as_deref(), Some("second tail"));
        // another agent's events do not count for this one
        assert!(!stop_is_latest(b, base));
        record_event_at(b, EventKind::Notification, base + 4_000, None, StopInfo::default());
        assert!(!stop_is_latest(b, base), "a notification is not a stop");
        // a permission prompt after the latest stop = waiting for the user, until the agent moves on
        assert!(needs_user(b, base));
        assert!(!needs_user(b, base + 4_001), "older than `since`");
        record_event_at(b, EventKind::Stop, base + 5_000, None, StopInfo::default());
        assert!(!needs_user(b, base), "a stop after the prompt answers it");
        record_event_at(a, EventKind::Notification, base + 6_000, None, StopInfo::default());
        assert!(needs_user(a, base));
        record_event_at(a, EventKind::Start, base + 7_000, None, StopInfo::default());
        assert!(!needs_user(a, base), "a new prompt answers it too");
        // the list stays capped, per agent: a chatty agent never pushes another agent's stop out
        let a_stop = last_event_ms(a, EventKind::Stop);
        assert!(a_stop.is_some());
        for i in 0..(MAX_EVENTS_PER_AGENT as u64 * 4) {
            record_event_at(b, EventKind::Stop, base + 10_000 + i, None, StopInfo::default());
        }
        assert!(EVENTS.lock().unwrap().iter().filter(|e| e.agent == b.name()).count() <= MAX_EVENTS_PER_AGENT);
        assert!(EVENTS.lock().unwrap().len() <= MAX_EVENTS);
        assert_eq!(last_event_ms(a, EventKind::Stop), a_stop, "the other agent's stop survived"); 
        assert_eq!(ago(120_000, 0), "2 minutes ago");
        assert_eq!(ago(10_000, 0), "moments ago");
        assert_eq!(ago(3_600_000, 0), "1 hour ago");
    }

    #[test]
    fn the_whole_parser_hands_hook_sentences_to_the_hooks_and_leaves_other_agent_talk_alone() {
        use crate::intent_parser::{parse_deterministic, ParsedIntent};
        for (t, action, agent) in [
            ("hook up claude", "connect", "claude"),
            ("remove the codex hook", "disconnect", "codex"),
            ("is the claude hook installed", "status", "claude"),
        ] {
            match parse_deterministic(t).map(|r| r.intent) {
                Some(ParsedIntent::AgentHookCommand { hook_action, agent: a }) => assert_eq!((hook_action.as_str(), a.as_str()), (action, agent), "{t}"),
                other => panic!("{t:?} -> {other:?}"),
            }
        }
        for t in ["open claude", "install claude", "watch claude until it's done and shut down my laptop", "cancel the shutdown plan"] {
            assert!(!matches!(parse_deterministic(t).map(|r| r.intent), Some(ParsedIntent::AgentHookCommand { .. })), "{t}");
        }
        // the question that asks for the setup carries the agent through the offer and a yes is a plain yes
        use crate::memcore::offer::{classify_for, Offer, Reply};
        let offer = Offer::HookSetup { agent: "claude".into() };
        assert_eq!(classify_for(&offer, "yes"), Reply::Yes);
        assert_eq!(classify_for(&offer, "no"), Reply::No);
        assert_eq!(classify_for(&offer, "shut down"), Reply::Other, "a power phrase does not approve a settings change");
    }

    #[test]
    fn voice_commands_for_the_hooks_are_strict() {
        for t in [
            "connect nexus to claude", "hook up claude", "set up the claude hook", "install the claude hook", "add the stop hook to claude",
            "let nexus know when claude stops", "set up claude notifications", "enable the claude hook", "nexus install the stop hook for claude",
            "connect the claude stop hook", "Nexus, hook up claude.", "please set up the codex hook", "let nexus know when codex finishes",
            "install the codex hook", "connect nexus to codex",
        ] {
            let r = parse_command(t);
            assert!(matches!(r, Some((HookAction::Connect, _))), "{t} -> {r:?}");
        }
        assert_eq!(parse_command("hook up claude"), Some((HookAction::Connect, Agent::Claude)));
        assert_eq!(parse_command("install the codex hook"), Some((HookAction::Connect, Agent::Codex)));
        for t in ["disconnect claude from nexus", "remove the claude hook", "uninstall the codex hook", "take out the claude hook", "disable the claude stop hook", "unhook claude"] {
            assert!(matches!(parse_command(t), Some((HookAction::Disconnect, _))), "{t}");
        }
        for t in ["is the claude hook installed", "is claude connected to nexus", "claude hook status", "has claude reported yet", "is the codex hook set up yet"] {
            assert!(matches!(parse_command(t), Some((HookAction::Status, _))), "{t}");
        }
        for t in [
            "", "install claude", "connect to claude", "open claude", "claude", "remove claude", "uninstall codex", "hook up", "set up the hook",
            "connect claude and codex hooks", "install the claude hook and open chrome", "tell me when claude stops", "what is a hook",
            "install the claude extension", "remove the hook from my fishing rod", "let me know when claude stops",
            "hook up the claude codex hook", "set up the claude hook for codex", "remove the codex claude hook",
        ] {
            assert_eq!(parse_command(t), None, "{t:?}");
        }
    }

    // ─── more agents (Antigravity, OpenCode, Gemini CLI, Cursor, Aider) ─────────────────────────

    /// Seen live with `agy` 1.2.9: a quoted script path made Antigravity's own hook runner fail with "is not
    /// recognized as an internal or external command". Its command must hold no quotes and no spaces.
    #[test]
    fn antigravity_commands_carry_no_quotes_and_no_spaces() {
        let after = antigravity_add_hook("", &script()).unwrap();
        let v: Value = serde_json::from_str(&after).unwrap();
        for event in ["Stop", "PreInvocation"] {
            let cmd = v["nexus-agent-event"][event][0]["command"].as_str().unwrap();
            assert!(!cmd.contains('"'), "{cmd}");
            assert_eq!(cmd.split(' ').count(), 4, "script, agent, kind, marker: {cmd}");
            assert!(cmd.ends_with(MARKER), "{cmd}");
        }
        // other agents keep the quoted form that already works for them
        assert!(claude_add_hook("", &script()).unwrap().contains(r#"\"C:\\Users"#), "claude keeps quotes");
        #[cfg(windows)]
        {
            // a real directory with a space gets its short form (skipped on a volume without 8.3 names)
            let d = tmp("space dir");
            let file = d.join("x y.cmd");
            std::fs::write(&file, "@echo off").unwrap();
            match unquoted_script(&file) {
                Ok(short) => assert!(!short.contains(' ') && !short.contains('"'), "{short}"),
                Err(_) => eprintln!("8.3 names are off on this volume: skipped"),
            }
            let _ = std::fs::remove_dir_all(&d);
        }
        // a path that has a space and no short form is refused, never written quoted
        #[cfg(windows)]
        assert!(unquoted_script(Path::new(r"C:\no such folder\x y.cmd")).is_err());
    }

    #[test]
    fn every_agent_has_a_name_that_round_trips_and_a_spoken_label() {
        for a in ALL_AGENTS {
            assert_eq!(Agent::from_name(a.name()), Some(a));
            assert!(!a.label().is_empty() && !a.restart_note().is_empty());
        }
        assert_eq!(Agent::from_name("agy"), Some(Agent::Antigravity));
        assert_eq!(Agent::from_name("open code"), Some(Agent::OpenCode));
        for bad in ["", "evil", "Claude", "vscode", "hermes", "kilo code cli", "trae agent"] {
            assert_eq!(Agent::from_name(bad), None, "{bad}");
        }
        for (said, want) in [
            ("devin", Agent::Devin),
            ("windsurf", Agent::Windsurf),
            ("devin desktop", Agent::Windsurf),
            ("kilo", Agent::Kilo),
            ("kilo code", Agent::Kilo),
            ("copilot", Agent::Copilot),
            ("github copilot", Agent::Copilot),
            ("trae", Agent::Trae),
        ] {
            assert_eq!(Agent::from_name(said), Some(want), "{said}");
        }
        let list = Agent::spoken_list();
        assert!(list.starts_with("Claude Code, Codex") && list.contains("Kilo Code") && list.ends_with(" or Trae"), "{list}");
    }

    #[test]
    fn windsurf_gets_its_two_hooks_with_a_marker_the_user_keeps_their_own_and_it_comes_out_clean() {
        let before = r#"{"hooks":{"post_cascade_response":[{"command":"python audit.py","show_output":false}],"pre_run_command":[{"command":"guard.sh"}]}}"#;
        let after = windsurf_add_hook(before, &script()).unwrap();
        let v: Value = serde_json::from_str(&after).unwrap();
        // the user's own hooks are untouched, ours sit beside them
        assert_eq!(v["hooks"]["pre_run_command"][0]["command"], "guard.sh");
        let stops = v["hooks"]["post_cascade_response"].as_array().unwrap();
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0]["command"], "python audit.py");
        let ours = stops[1]["command"].as_str().unwrap();
        assert!(ours.contains(" windsurf stop ") && ours.ends_with(MARKER), "{ours}");
        assert!(v["hooks"]["pre_user_prompt"][0]["command"].as_str().unwrap().contains(" windsurf start "));
        assert!(windsurf_has_hook(&after) && !windsurf_has_hook(before));
        assert_eq!(windsurf_add_hook(&after, &script()).unwrap(), after, "idempotent");
        let (back, removed) = windsurf_remove_hook(&after).unwrap();
        assert!(removed);
        let b: Value = serde_json::from_str(&back).unwrap();
        assert_eq!(b, serde_json::from_str::<Value>(before).unwrap(), "only ours was removed");
        // the reply text of a Windsurf stop is `tool_info.response`
        let ev = parse_event(
            "agent=windsurf&event=stop",
            r#"{"agent_action_name":"post_cascade_response","trajectory_id":"t1","tool_info":{"response":"Refactored the parser.\nAll tests pass."}}"#,
        )
        .unwrap();
        assert_eq!((ev.agent, ev.kind), (Agent::Windsurf, EventKind::Stop));
        assert_eq!(ev.tail.as_deref(), Some("Refactored the parser.\nAll tests pass."));
    }

    #[test]
    fn kilo_and_copilot_get_a_file_of_their_own_and_a_foreign_file_of_that_name_is_never_touched() {
        let d = tmp("owned_files");
        let t = HookTarget { script: &script(), token: "tok", port: 39220 };
        // Kilo: a plugin file
        let kilo = d.join("kilo").join("plugin").join("nexus-agent-event.js");
        let done = install(Agent::Kilo, &kilo, &t).unwrap();
        assert_eq!(done.backup, None);
        let text = std::fs::read_to_string(&kilo).unwrap();
        assert!(text.contains("agent=kilo") && text.contains("session.idle") && text.contains("export default") && text.contains("\"tok\""), "{text}");
        assert!(is_installed(Agent::Kilo, &kilo));
        assert!(install(Agent::Kilo, &kilo, &t).is_ok(), "idempotent");
        assert_eq!(uninstall(Agent::Kilo, &kilo), Ok(true));
        assert!(!kilo.exists());
        std::fs::create_dir_all(kilo.parent().unwrap()).unwrap();
        std::fs::write(&kilo, "// the user's own plugin").unwrap();
        assert!(matches!(install(Agent::Kilo, &kilo, &t), Err(HookError::Unreadable(_))));
        assert_eq!(std::fs::read_to_string(&kilo).unwrap(), "// the user's own plugin");
        assert_eq!(uninstall(Agent::Kilo, &kilo), Ok(false), "not ours: left alone");
        // Copilot: a hook file with the three events
        let cp = d.join(".copilot").join("hooks").join("nexus-agent-event.json");
        install(Agent::Copilot, &cp, &t).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cp).unwrap()).unwrap();
        assert_eq!(v["version"], 1);
        for (event, kind) in [("agentStop", "stop"), ("userPromptSubmitted", "start"), ("notification", "notification")] {
            let e = &v["hooks"][event][0];
            assert_eq!(e["type"], "command", "{event}");
            let ps = e["powershell"].as_str().unwrap();
            assert!(ps.starts_with("& '") && ps.contains(&format!(" copilot {kind} ")) && ps.ends_with(MARKER), "{ps}");
            assert!(e["bash"].as_str().unwrap().contains(MARKER));
        }
        assert!(is_installed(Agent::Copilot, &cp));
        assert_eq!(uninstall(Agent::Copilot, &cp), Ok(true));
        std::fs::write(&cp, "{}").unwrap();
        assert!(matches!(install(Agent::Copilot, &cp, &t), Err(HookError::Unreadable(_))), "a hook file of the user's with that name");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn devin_shares_claudes_hook_and_is_told_apart_by_its_payload_while_trae_has_nothing_to_install() {
        // Claude's own payload carries a transcript path
        let claude = parse_event("agent=claude&event=stop", r#"{"session_id":"s","transcript_path":"/t.jsonl","hook_event_name":"Stop","last_assistant_message":"Done."}"#).unwrap();
        assert_eq!(claude.agent, Agent::Claude);
        // Devin's has a prompt id and no transcript
        let devin = parse_event("agent=claude&event=stop", r#"{"session_id":"s","prompt_id":"p1","stop_hook_active":false}"#).unwrap();
        assert_eq!(devin.agent, Agent::Devin);
        assert_eq!(devin.kind, EventKind::Stop);
        let devin_start = parse_event("agent=claude&event=start", r#"{"session_id":"s","prompt_id":"p2"}"#).unwrap();
        assert_eq!(devin_start.agent, Agent::Devin);
        // no payload, or another agent's name: never relabelled
        assert_eq!(parse_event("agent=claude&event=stop", "").unwrap().agent, Agent::Claude);
        assert_eq!(parse_event("agent=claude&event=stop", r#"{"session_id":"s","last_assistant_message":"x"}"#).unwrap().agent, Agent::Claude, "no prompt id: still Claude");
        assert_eq!(parse_event("agent=codex&event=stop", r#"{"prompt_id":"p"}"#).unwrap().agent, Agent::Codex);
        assert_eq!(
            parse_event("agent=claude&event=stop", r#"{"session_id":"s","prompt_id":"p","transcript_path":"/t.jsonl"}"#).unwrap().agent,
            Agent::Claude,
            "a transcript path means Claude Code, whatever else the payload carries"
        );
        // installing "devin" installs Claude's hook in Claude's file; removing it never removes Claude's
        assert_eq!(config_path(Agent::Devin), config_path(Agent::Claude));
        let d = tmp("devin");
        let cfg = d.join("settings.json");
        let t = HookTarget { script: &script(), token: "t", port: 1 };
        install(Agent::Devin, &cfg, &t).unwrap();
        assert!(is_installed(Agent::Devin, &cfg) && is_installed(Agent::Claude, &cfg));
        assert_eq!(uninstall(Agent::Devin, &cfg), Ok(false));
        assert!(is_installed(Agent::Claude, &cfg), "Devin's disconnect must not remove Claude's hook");
        let _ = std::fs::remove_dir_all(&d);
        // Trae: no hook exists, nothing is written, and it is the only observe-only agent
        assert_eq!(config_path(Agent::Trae), None);
        assert_eq!(install(Agent::Trae, Path::new("x.json"), &t), Err(HookError::NoHookMechanism));
        assert!(Agent::Trae.observe_only() && ALL_AGENTS.iter().filter(|a| a.observe_only()).count() == 1);
        assert!(HookError::NoHookMechanism.spoken(Agent::Trae).contains("never call its work perfect"));
    }

    #[test]
    fn hook_up_all_agents_is_understood_and_ordinary_sentences_are_not_claimed() {
        for t in ["hook up all agents", "hook up all my agents", "hookup every agent", "connect all the coding agents", "hook up all the ai agents to nexus"] {
            assert_eq!(parse_all_command(t), Some(HookAction::Connect), "{t}");
        }
        for t in ["remove all the agent hooks", "unhook all agents", "disconnect every agent"] {
            assert_eq!(parse_all_command(t), Some(HookAction::Disconnect), "{t}");
        }
        for t in ["which agents are hooked up", "are all agents hooked up", "which agents are connected"] {
            assert_eq!(parse_all_command(t), Some(HookAction::Status), "{t}");
        }
        for t in ["hook up claude", "open all the windows", "all agents are coming", "track all the agents running", "remove all the files", "which agents should I use for coding"] {
            assert_eq!(parse_all_command(t), None, "{t}");
        }
    }

    #[test]
    fn the_agents_on_this_machine_are_the_ones_whose_settings_folder_exists() {
        let h = tmp("present");
        assert!(agents_present_in(&h).is_empty());
        for d in [".claude", ".codex", ".config/kilo", ".codeium/windsurf", ".copilot", ".config/devin", ".trae"] {
            std::fs::create_dir_all(h.join(d)).unwrap();
        }
        std::fs::create_dir_all(h.join(".gemini").join("config")).unwrap();
        let present = agents_present_in(&h);
        for a in [Agent::Claude, Agent::Codex, Agent::Kilo, Agent::Windsurf, Agent::Copilot, Agent::Devin, Agent::Trae, Agent::Antigravity] {
            assert!(present.contains(&a), "{a:?}");
        }
        for a in [Agent::Gemini, Agent::Cursor, Agent::Aider, Agent::OpenCode] {
            assert!(!present.contains(&a), "{a:?}");
        }
        std::fs::write(h.join(".gemini").join("settings.json"), "{}").unwrap();
        assert!(agents_present_in(&h).contains(&Agent::Gemini));
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn the_hook_voice_commands_know_the_new_agents() {
        for (t, agent) in [
            ("hook up kilo code", Agent::Kilo),
            ("hook up kilo", Agent::Kilo),
            ("hook up github copilot", Agent::Copilot),
            ("hook up copilot cli", Agent::Copilot),
            ("hook up devin", Agent::Devin),
            ("hook up devin desktop", Agent::Windsurf),
            ("hook up windsurf", Agent::Windsurf),
            ("hook up trae", Agent::Trae),
        ] {
            assert_eq!(parse_command(t), Some((HookAction::Connect, agent)), "{t}");
        }
        assert_eq!(parse_command("remove the kilo hook"), Some((HookAction::Disconnect, Agent::Kilo)));
        assert_eq!(parse_command("is the windsurf hook installed"), Some((HookAction::Status, Agent::Windsurf)));
        assert_eq!(parse_command("hook up kilo and devin"), None, "two agents in one sentence");
    }

    #[test]
    fn antigravity_gets_a_named_hook_with_stop_and_preinvocation_and_keeps_the_rest() {
        let before = r#"{"my-linter":{"PostToolUse":[{"matcher":"run_command","hooks":[{"type":"command","command":"./lint.sh"}]}]}}"#;
        let after = antigravity_add_hook(before, &script()).unwrap();
        let v: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["my-linter"], serde_json::from_str::<Value>(before).unwrap()["my-linter"]);
        let ours = &v["nexus-agent-event"];
        // handlers sit directly under the event key for non-tool events (Antigravity's hooks documentation)
        assert_eq!(ours["Stop"][0]["type"], "command");
        assert!(ours["Stop"][0]["command"].as_str().unwrap().ends_with(&format!(" antigravity stop {MARKER}")));
        assert!(ours["PreInvocation"][0]["command"].as_str().unwrap().ends_with(&format!(" antigravity start {MARKER}")));
        assert!(ours["Stop"][0].get("hooks").is_none());
        assert!(antigravity_has_hook(&after));
        assert_eq!(antigravity_add_hook(&after, &script()).unwrap(), after, "idempotent");
        let (back, removed) = antigravity_remove_hook(&after).unwrap();
        assert!(removed && !back.contains(MARKER) && back.contains("my-linter"));
        assert!(!antigravity_has_hook(&back));
        assert!(antigravity_has_hook(&antigravity_add_hook("", &script()).unwrap()));
    }

    /// The exact file NEXUS wrote on the user's machine (2026-10-19 19:18): short path, no marker in the value.
    const USERS_LEGACY_FILE: &str = r#"{
  "nexus-agent-event": {
    "PreInvocation": [
      { "command": "C:\\Users\\CHITKU~1\\AppData\\Roaming\\COMNEX~1.ASS\\AGENT_~1\\NEXUS-~1.CMD antigravity start", "timeout": 10, "type": "command" }
    ],
    "Stop": [
      { "command": "C:\\Users\\CHITKU~1\\AppData\\Roaming\\COMNEX~1.ASS\\AGENT_~1\\NEXUS-~1.CMD antigravity stop", "timeout": 10, "type": "command" }
    ]
  }
}"#;

    #[test]
    fn nexus_recognises_upgrades_and_removes_its_own_old_antigravity_hook() {
        assert!(antigravity_has_hook(USERS_LEGACY_FILE), "it must find the hook it wrote");
        // installing again no longer refuses; it upgrades the commands so they carry the marker
        let upgraded = antigravity_add_hook(USERS_LEGACY_FILE, &script()).expect("an old hook of ours is not foreign");
        assert!(upgraded.contains(MARKER) && antigravity_has_hook(&upgraded));
        assert_eq!(antigravity_add_hook(&upgraded, &script()).unwrap(), upgraded, "idempotent once upgraded");
        let (back, removed) = antigravity_remove_hook(USERS_LEGACY_FILE).unwrap();
        assert!(removed && !back.contains("NEXUS-~1") && !antigravity_has_hook(&back));
        // through the public install/is_installed/uninstall path on a real file
        let d = tmp("agy_legacy");
        let cfg = d.join("hooks.json");
        std::fs::write(&cfg, USERS_LEGACY_FILE).unwrap();
        assert!(is_installed(Agent::Antigravity, &cfg));
        assert!(uninstall(Agent::Antigravity, &cfg).unwrap() && !is_installed(Agent::Antigravity, &cfg));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_our_exact_old_commands_count_as_ours() {
        // the script's own name is `.cmd` on Windows and `.sh` elsewhere, so build that case from SCRIPT_NAME
        let ok_cases = [
            r"C:\x\NEXUS-~1.CMD antigravity stop".to_string(),
            format!(r"c:\x\{SCRIPT_NAME} antigravity start"),
            "C:/x/NEXUS-~2.cmd antigravity stop".to_string(),
        ];
        for ok in ok_cases {
            assert!(legacy_antigravity_command_is_ours(&ok), "{ok}");
        }
        for bad in [
            r"C:\x\my-hook.cmd antigravity stop",
            r"C:\x\NEXUS-~1.CMD antigravity stop && del /q *",
            r"C:\x\NEXUS-~1.CMD claude stop",
            r"C:\x\NEXUS-~1.CMD antigravity",
            r"C:\x\NEXUS-~1.CMD antigravity explode",
            "NEXUS-~1.CMD antigravity stop extra",
            "",
        ] {
            assert!(!legacy_antigravity_command_is_ours(bad), "{bad}");
        }
        // a group under our key that mixes ours and someone else's, or holds none, is not ours
        let mixed = r#"{"nexus-agent-event":{"Stop":[{"type":"command","command":"C:\x\NEXUS-~1.CMD antigravity stop"},{"type":"command","command":"evil.cmd"}]}}"#;
        assert!(!antigravity_has_hook(mixed) && antigravity_add_hook(mixed, &script()).is_err());
        let empty = r#"{"nexus-agent-event":{}}"#;
        assert!(antigravity_add_hook(empty, &script()).is_err());
    }

    #[test]
    fn a_foreign_antigravity_hook_with_our_name_is_never_touched() {
        let theirs = r#"{"nexus-agent-event":{"Stop":[{"type":"command","command":"my-own.sh"}]}}"#;
        assert!(antigravity_add_hook(theirs, &script()).is_err());
        let (text, removed) = antigravity_remove_hook(theirs).unwrap();
        assert!(!removed && text == theirs);
        assert!(!antigravity_has_hook(theirs));
        assert!(antigravity_add_hook("[1,2]", &script()).is_err() && antigravity_add_hook("{bad", &script()).is_err());
    }

    #[test]
    fn gemini_gets_afteragent_beforeagent_and_notification_with_millisecond_timeouts() {
        let before = r#"{"theme":"dark","hooks":{"AfterAgent":[{"hooks":[{"type":"command","command":"my.sh"}]}]}}"#;
        let after = gemini_add_hook(before, &script()).unwrap();
        let v: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["theme"], "dark");
        let after_agent = v["hooks"]["AfterAgent"].as_array().unwrap();
        assert_eq!(after_agent.len(), 2);
        assert!(after_agent.iter().any(|e| e["hooks"][0]["command"] == "my.sh"));
        let ours = after_agent.iter().find(|e| entry_is_ours(e)).unwrap();
        assert_eq!(ours["hooks"][0]["timeout"], 10_000);
        assert!(ours["hooks"][0]["command"].as_str().unwrap().ends_with(" gemini stop"));
        assert!(v["hooks"]["BeforeAgent"][0]["hooks"][0]["command"].as_str().unwrap().ends_with(" gemini start"));
        assert!(gemini_has_hook(&after));
        let (back, removed) = gemini_remove_hook(&after).unwrap();
        assert!(removed && !back.contains(MARKER) && back.contains("my.sh"));
    }

    #[test]
    fn cursor_gets_flat_hooks_and_a_version_and_keeps_the_rest() {
        let before = r#"{"version":1,"hooks":{"stop":[{"command":"./mine.sh"}],"afterFileEdit":[{"command":"./fmt.sh"}]}}"#;
        let after = cursor_add_hook(before, &script()).unwrap();
        let v: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["hooks"]["afterFileEdit"][0]["command"], "./fmt.sh");
        assert_eq!(v["hooks"]["stop"].as_array().unwrap().len(), 2);
        assert!(v["hooks"]["afterAgentResponse"][0]["command"].as_str().unwrap().ends_with(" cursor message"));
        assert!(v["hooks"]["beforeSubmitPrompt"][0]["command"].as_str().unwrap().ends_with(" cursor start"));
        assert!(cursor_has_hook(&after));
        let fresh = cursor_add_hook("", &script()).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&fresh).unwrap()["version"], 1);
        let (back, removed) = cursor_remove_hook(&after).unwrap();
        assert!(removed && !back.contains(MARKER) && back.contains("./mine.sh") && back.contains("./fmt.sh"));
        assert!(!cursor_remove_hook(before).unwrap().1);
    }

    #[test]
    fn aider_gets_notifications_only_when_it_has_none_of_its_own() {
        let before = "model: gpt-4o\ndark-mode: true\n";
        let after = aider_add_notify(before, &script()).unwrap();
        assert!(after.contains("model: gpt-4o") && after.contains("dark-mode: true"));
        assert!(after.contains("notifications: true") && after.contains("notifications-command: '"));
        assert!(after.contains(" aider stop'"));
        assert!(aider_has_notify(&after));
        assert_eq!(aider_add_notify(&after, &script()).unwrap(), after, "idempotent");
        let (back, removed) = aider_remove_notify(&after);
        assert!(removed && back == before, "{back:?}");
        // the user's own notification settings are theirs
        for theirs in ["notifications: true\n", "notifications-command: say done\n", "notifications_command: say done\n", "notifications: false\nmodel: x\n"] {
            assert_eq!(aider_add_notify(theirs, &script()), Err(HookError::HasOwnNotify), "{theirs}");
            assert_eq!(aider_remove_notify(theirs), (theirs.to_string(), false));
            assert!(!aider_has_notify(theirs));
        }
        // a moved script refreshes our line only
        let moved = aider_add_notify(&after, Path::new(r"D:\other\nexus-agent-event.cmd")).unwrap();
        assert!(moved.contains(r"D:\other") && !moved.contains("Roaming") && moved.matches("notifications-command").count() == 1);
        assert!(aider_add_notify("", Path::new("C:\\it's\\nexus-agent-event.cmd")).is_err());
    }

    #[test]
    fn the_opencode_plugin_is_its_own_file_and_never_replaces_someone_elses() {
        let d = tmp("opencode");
        let plugin = d.join("opencode").join("plugins").join(OPENCODE_PLUGIN_FILE);
        let sc = script();
        let t = HookTarget { script: &sc, token: "tok-123", port: 39220 };
        assert!(!is_installed(Agent::OpenCode, &plugin));
        assert_eq!(install(Agent::OpenCode, &plugin, &t).unwrap().backup, None);
        let text = std::fs::read_to_string(&plugin).unwrap();
        assert!(text.contains("tok-123") && text.contains("127.0.0.1:39220/agent-event") && text.contains(MARKER));
        assert!(text.contains("session.idle") && text.contains("permission.updated") && text.contains("parentID"));
        assert!(is_installed(Agent::OpenCode, &plugin));
        assert_eq!(install(Agent::OpenCode, &plugin, &t).unwrap(), Installed { backup: None }, "idempotent");
        assert!(uninstall(Agent::OpenCode, &plugin).unwrap() && !plugin.exists());
        assert!(!uninstall(Agent::OpenCode, &plugin).unwrap());
        // somebody else's file with that name is left alone
        std::fs::create_dir_all(plugin.parent().unwrap()).unwrap();
        std::fs::write(&plugin, "export const Mine = async () => ({})").unwrap();
        assert!(install(Agent::OpenCode, &plugin, &t).is_err());
        assert!(!uninstall(Agent::OpenCode, &plugin).unwrap() && plugin.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Runs the generated plugin under node with a fake OpenCode client and a local socket: proves the JavaScript is
    /// valid, ignores sub-sessions and synthetic text, and posts what the endpoint accepts. Skipped when node is absent.
    #[test]
    fn the_opencode_plugin_runs_and_posts_what_the_endpoint_accepts() {
        use std::io::{Read, Write};
        if std::process::Command::new("node").arg("--version").output().is_err() {
            eprintln!("node not available: skipped");
            return;
        }
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let mut seen: Vec<String> = Vec::new();
            let started = std::time::Instant::now();
            while seen.len() < 3 && started.elapsed() < std::time::Duration::from_secs(20) {
                let Ok((mut s, _)) = listener.accept() else {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                };
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let want: usize = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse().ok())).unwrap_or(0);
                        if body.len() >= want {
                            break;
                        }
                    }
                }
                let _ = s.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}");
                seen.push(String::from_utf8_lossy(&raw).to_string());
            }
            seen
        });
        let dir = tmp("opencode_js");
        std::fs::write(dir.join("p.mjs"), opencode_plugin_text("js-token", port)).unwrap();
        std::fs::write(
            dir.join("run.mjs"),
            r#"import { NexusAgentEvent } from "./p.mjs";
const client = {
  session: {
    get: async ({ path }) => ({ data: { id: path.id, parentID: path.id === "child" ? "root" : undefined } }),
    messages: async () => ({ data: [
      { info: { role: "user" }, parts: [{ type: "text", text: "do it" }] },
      { info: { role: "assistant" }, parts: [{ type: "text", text: "All 12 tests passed." }, { type: "text", text: "hidden", synthetic: true }] },
    ] }),
  },
};
const hooks = await NexusAgentEvent({ client });
await hooks.event({ event: { type: "message.updated", properties: { info: { role: "user" } } } });
await hooks.event({ event: { type: "message.updated", properties: { info: { role: "assistant" } } } });
await hooks.event({ event: { type: "permission.updated", properties: {} } });
await hooks.event({ event: { type: "session.idle", properties: { sessionID: "child" } } });
await hooks.event({ event: { type: "session.idle", properties: { sessionID: "root" } } });
"#,
        )
        .unwrap();
        let out = std::process::Command::new("node").arg("run.mjs").current_dir(&dir).output().unwrap();
        assert!(out.status.success(), "node failed: {}", String::from_utf8_lossy(&out.stderr));
        let seen = server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(seen.len(), 3, "start, notification and ONE stop (the sub-session and the assistant message are ignored): {seen:?}");
        let events: Vec<ParsedEvent> = seen
            .iter()
            .map(|r| {
                assert!(r.to_lowercase().contains("authorization: bearer js-token"), "{r}");
                let target = r.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("");
                let (_, q) = target.split_once('?').unwrap_or(("", ""));
                parse_event(q, r.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("")).expect("a valid event")
            })
            .collect();
        assert_eq!(events.iter().map(|e| (e.agent, e.kind)).collect::<Vec<_>>(), vec![
            (Agent::OpenCode, EventKind::Start),
            (Agent::OpenCode, EventKind::Notification),
            (Agent::OpenCode, EventKind::Stop),
        ]);
        assert_eq!(events[2].tail.as_deref(), Some("All 12 tests passed."), "the synthetic text part is not the reply");
    }

    #[test]
    fn each_agents_payload_becomes_the_right_event() {
        // Antigravity: the stop status is structured; the text is read from the transcript afterwards
        let ev = parse_event(
            "agent=antigravity&event=stop",
            r#"{"executionNum":1,"terminationReason":"model_stop","error":"","fullyIdle":true,"conversationId":"c","transcriptPath":"~/.gemini/antigravity/brain/c/transcript.jsonl"}"#,
        )
        .unwrap();
        assert_eq!((ev.agent, ev.kind), (Agent::Antigravity, EventKind::Stop));
        assert_eq!(ev.info, StopInfo { ok: None, fully_idle: Some(true) });
        assert_eq!(ev.source, Some(TailSource::Transcript("~/.gemini/antigravity/brain/c/transcript.jsonl".into())));
        assert_eq!(ev.tail, None);
        let bad = parse_event("agent=antigravity&event=stop", r#"{"terminationReason":"model_stop","error":"quota exceeded","fullyIdle":false}"#).unwrap();
        assert_eq!(bad.info, StopInfo { ok: Some(false), fully_idle: Some(false) });
        let cancelled = parse_event("agent=antigravity&event=stop", r#"{"terminationReason":"user_cancelled","error":""}"#).unwrap();
        assert_eq!(cancelled.info.ok, Some(false));
        // Gemini CLI
        let g = parse_event("agent=gemini&event=stop", r#"{"hook_event_name":"AfterAgent","prompt":"x","prompt_response":"Done. 5 tests pass.","stop_hook_active":false}"#).unwrap();
        assert_eq!(g.tail.as_deref(), Some("Done. 5 tests pass."));
        // Cursor
        assert_eq!(parse_event("agent=cursor&event=stop", r#"{"status":"aborted","loop_count":0}"#).unwrap().info.ok, Some(false));
        assert_eq!(parse_event("agent=cursor&event=stop", r#"{"status":"error"}"#).unwrap().info.ok, Some(false));
        assert_eq!(parse_event("agent=cursor&event=stop", r#"{"status":"completed"}"#).unwrap().info.ok, Some(true));
        let m = parse_event("agent=cursor&event=message", r#"{"text":"Fixed the bug."}"#).unwrap();
        assert_eq!((m.kind, m.tail.as_deref()), (EventKind::Message, Some("Fixed the bug.")));
        // "text" is only read for a message, never for a stop
        assert_eq!(parse_event("agent=cursor&event=stop", r#"{"status":"completed","text":"ignore me"}"#).unwrap().tail, None);
        // Aider posts a form body with its working directory
        let a = parse_event("agent=aider&event=stop", "cwd=C%3A%5CUsers%5Cme%5Cmy+proj").unwrap();
        assert_eq!(a.source, Some(TailSource::AiderCwd(r"C:\Users\me\my proj".into())));
        // OpenCode's plugin
        let o = parse_event("agent=opencode&event=stop", r#"{"last_assistant_message":"Merged."}"#).unwrap();
        assert_eq!(o.tail.as_deref(), Some("Merged."));
        // still strict
        for (q, b) in [("agent=evil&event=stop", "{}"), ("agent=cursor&event=explode", "{}"), ("agent=cursor", "{}"), ("event=stop", "{}")] {
            assert_eq!(parse_event(q, b), None, "{q}");
        }
    }

    #[test]
    fn percent_decoding_handles_the_edges() {
        assert_eq!(percent_decode("C%3A%5Cx+y"), r"C:\x y");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
        assert_eq!(percent_decode(""), "");
    }

    fn transcript_home(name: &str) -> (PathBuf, PathBuf) {
        let home = tmp(name);
        let dir = home.join(".gemini").join("antigravity").join("brain").join("c1").join(".system_generated").join("logs");
        std::fs::create_dir_all(&dir).unwrap();
        (home, dir.join("transcript.jsonl"))
    }

    fn transcript_event(raw: &str) -> ParsedEvent {
        ParsedEvent { agent: Agent::Antigravity, kind: EventKind::Stop, tail: None, info: StopInfo::default(), source: Some(TailSource::Transcript(raw.to_string())), cwd: None }
    }

    #[test]
    fn antigravity_text_is_read_from_the_transcript_but_only_from_inside_its_own_folder() {
        let (home, file) = transcript_home("agy_home");
        let lines = [
            r#"{"type":"USER_INPUT","content":"fix it","step_index":1}"#,
            r#"{"type":"PLANNER_RESPONSE","content":"","tool_calls":[{"name":"run_command","args":{}}],"step_index":2}"#,
            r#"{"type":"GENERIC","content":"output","step_index":3}"#,
            r#"{"type":"PLANNER_RESPONSE","content":"All 12 tests pass.","step_index":4}"#,
        ];
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();
        let mut ev = transcript_event(&file.display().to_string());
        resolve_tail_with(&mut ev, Some(&home));
        assert_eq!(ev.tail.as_deref(), Some("All 12 tests pass."));
        assert_eq!(ev.info.ok, None);
        assert!(ev.source.is_none(), "the source is consumed");
        // ~/ is expanded
        let mut ev = transcript_event("~/.gemini/antigravity/brain/c1/.system_generated/logs/transcript.jsonl");
        resolve_tail_with(&mut ev, Some(&home));
        assert_eq!(ev.tail.as_deref(), Some("All 12 tests pass."));

        // a file outside ~/.gemini, a ".." climb out of it, the wrong extension and a directory all give no text
        let outside = home.join("other");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("transcript.jsonl"), lines.join("\n")).unwrap();
        std::fs::write(file.with_extension("txt"), lines.join("\n")).unwrap();
        let climb = home.join(".gemini").join("..").join("other").join("transcript.jsonl");
        for raw in [
            outside.join("transcript.jsonl").display().to_string(),
            climb.display().to_string(),
            file.with_extension("txt").display().to_string(),
            file.parent().unwrap().display().to_string(),
            "~/other/transcript.jsonl".to_string(),
            "".to_string(),
        ] {
            let mut ev = transcript_event(&raw);
            resolve_tail_with(&mut ev, Some(&home));
            assert_eq!(ev.tail, None, "{raw}");
        }
        let mut ev = transcript_event(&file.display().to_string());
        resolve_tail_with(&mut ev, None);
        assert_eq!(ev.tail, None, "no home folder known: nothing is read");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The exact shape `agy` 1.2.9 posted in a live run (2026-10-19): forward-slash paths under
    /// `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript_full.jsonl`, `terminationReason`
    /// "NO_TOOL_CALL" for a normal finish, a reply that is a `PLANNER_RESPONSE` with `content`.
    #[test]
    fn a_real_agy_stop_resolves_to_its_reply_and_a_clean_status() {
        let home = tmp("agy_live_shape");
        let dir = home.join(".gemini").join("antigravity-cli").join("brain").join("e52a92c1").join(".system_generated").join("logs");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("transcript_full.jsonl");
        std::fs::write(
            &file,
            concat!(
                r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-10-19T11:09:36Z","content":"Reply with exactly the single word ok."}"#,
                "\n",
                r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-10-19T11:09:38Z","input_tokens":900,"cache_read_tokens":0,"output_tokens":2,"content":"ok"}"#,
                "\n"
            ),
        )
        .unwrap();
        let slashed = file.display().to_string().replace('\\', "/");
        let body = format!(
            r#"{{"artifactDirectoryPath":"x","conversationId":"e52a92c1","error":"","executionNum":0,"fullyIdle":true,"modelName":"gemini-3.8-flash-high","terminationReason":"NO_TOOL_CALL","transcriptPath":"{slashed}","workspacePaths":["C:/w"]}}"#
        );
        let mut ev = parse_event("agent=antigravity&event=stop", &body).unwrap();
        assert_eq!(ev.info, StopInfo { ok: None, fully_idle: Some(true) });
        resolve_tail_with(&mut ev, Some(&home));
        assert_eq!(ev.tail.as_deref(), Some("ok"));
        assert_eq!(ev.info.ok, None, "NO_TOOL_CALL is how a normal finish is reported");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_error_after_the_last_reply_or_a_turn_without_a_reply_is_not_a_clean_finish() {
        let (home, file) = transcript_home("agy_err");
        let reply = r#"{"type":"PLANNER_RESPONSE","content":"Done.","step_index":2}"#;
        let user = r#"{"type":"USER_INPUT","content":"go","step_index":1}"#;
        let err = r#"{"type":"ERROR_MESSAGE","error":"model overloaded","step_index":3}"#;
        std::fs::write(&file, [user, reply, err].join("\n") + "\n").unwrap();
        let mut ev = transcript_event(&file.display().to_string());
        resolve_tail_with(&mut ev, Some(&home));
        assert_eq!((ev.tail.as_deref(), ev.info.ok), (Some("Done."), Some(false)));
        // an error BEFORE the final reply (a retry that worked) is not held against it
        std::fs::write(&file, [user, err, reply].join("\n") + "\n").unwrap();
        let mut ev = transcript_event(&file.display().to_string());
        resolve_tail_with(&mut ev, Some(&home));
        assert_eq!((ev.tail.as_deref(), ev.info.ok), (Some("Done."), None));
        // the last thing is the user's own message: no reply to this turn
        std::fs::write(&file, [reply, user].join("\n") + "\n").unwrap();
        let mut ev = transcript_event(&file.display().to_string());
        resolve_tail_with(&mut ev, Some(&home));
        assert_eq!(ev.tail, None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_huge_transcript_is_read_from_its_end_only() {
        let (home, file) = transcript_home("agy_big");
        let filler = format!("{{\"type\":\"GENERIC\",\"content\":\"{}\",\"step_index\":1}}\n", "x".repeat(900));
        let mut text = String::new();
        while text.len() < 2 * 1024 * 1024 {
            text.push_str(&filler);
        }
        text.push_str(r#"{"type":"PLANNER_RESPONSE","content":"Final answer.","step_index":9}"#);
        text.push('\n');
        std::fs::write(&file, &text).unwrap();
        let mut ev = transcript_event(&file.display().to_string());
        let t0 = std::time::Instant::now();
        resolve_tail_with(&mut ev, Some(&home));
        assert_eq!(ev.tail.as_deref(), Some("Final answer."));
        assert!(t0.elapsed() < std::time::Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn aider_text_is_the_last_reply_in_its_history_file() {
        let dir = tmp("aider_cwd");
        let history = "# aider chat started at 2026-10-19 10:00:00\n\n> Model: x\n\n#### add a retry\n\nI added a retry loop.\n\n> Applied edit to a.py\n\n#### now run the tests\n\nAll 4 tests passed.\n\n> Tokens: 1k sent\n";
        std::fs::write(dir.join(".aider.chat.history.md"), history).unwrap();
        let mk = |cwd: String| ParsedEvent { agent: Agent::Aider, kind: EventKind::Stop, tail: None, info: StopInfo::default(), source: Some(TailSource::AiderCwd(cwd)), cwd: None };
        let mut ev = mk(dir.display().to_string());
        resolve_tail_with(&mut ev, Some(&dir));
        assert_eq!(ev.tail.as_deref(), Some("All 4 tests passed."));
        // a relative directory, a missing directory or a directory without the history file give no text
        for cwd in ["proj".to_string(), "".to_string(), dir.join("nope").display().to_string(), std::env::temp_dir().display().to_string()] {
            let mut ev = mk(cwd.clone());
            resolve_tail_with(&mut ev, Some(&dir));
            assert_eq!(ev.tail, None, "{cwd:?}");
        }
        assert_eq!(aider_last_reply("no user lines here"), None);
        assert_eq!(aider_last_reply("#### hi\n"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_events_are_merged_so_they_never_push_a_stop_out() {
        let _events = events_test_lock();
        let a = Agent::Gemini;
        let base = 9_100_000_000u64;
        record_event_at(a, EventKind::Stop, base, Some("done".into()), StopInfo::default());
        for i in 0..200 {
            record_event_at(a, EventKind::Start, base + 10 + i, None, StopInfo::default());
        }
        assert_eq!(last_event_ms(a, EventKind::Stop), Some(base), "the stop is still there");
        assert_eq!(last_event_ms(a, EventKind::Start), Some(base + 209), "one start, refreshed");
        assert!(!stop_is_latest(a, base), "a prompt after the stop means a new turn");
    }

    #[test]
    fn a_cursor_reply_attaches_to_its_own_turns_stop_only() {
        let _events = events_test_lock();
        let a = Agent::Cursor;
        let base = 9_200_000_000u64;
        record_event_at(a, EventKind::Start, base, None, StopInfo::default());
        record_event_at(a, EventKind::Message, base + 100, Some("Fixed the bug.".into()), StopInfo::default());
        record_event_at(a, EventKind::Stop, base + 200, None, StopInfo { ok: Some(true), fully_idle: None });
        assert_eq!(last_stop_tail(a).as_deref(), Some("Fixed the bug."));
        assert_eq!(last_stop_info(a).ok, Some(true));
        assert!(stop_is_latest(a, base));
        // the next turn has no reply of its own: the earlier turn's reply is not borrowed
        record_event_at(a, EventKind::Start, base + 300, None, StopInfo::default());
        record_event_at(a, EventKind::Stop, base + 400, None, StopInfo { ok: Some(false), fully_idle: None });
        assert_eq!(last_stop_tail(a), None);
        assert_eq!(last_stop_info(a).ok, Some(false));
    }

    #[test]
    fn the_hook_script_answers_with_neutral_json_and_never_blocks() {
        let s = script_text("tok", 39220);
        assert!(s.contains("aider") && s.contains("--data-urlencode") && s.contains("cwd="), "{s}");
        assert!(s.contains(r#"{"continue":true}"#) && s.contains("{}"), "{s}");
        assert!(s.contains("exit /b 0") || s.contains("exit 0"));
        assert!(!s.contains("@PORT@") && !s.contains("@TOKEN@") && s.contains("39220") && s.contains("tok"));
    }

    /// The exact transcripts of the first live try (2026-10-19 17:50).
    #[test]
    fn speech_to_text_spellings_of_hook_up_and_the_agents_are_understood() {
        for (t, agent) in [
            ("hookup anti-gravity.", Agent::Antigravity),
            ("Hookup open code.", Agent::OpenCode),
            ("hook up anti gravity", Agent::Antigravity),
            ("Nexus, hookup Open Code please", Agent::OpenCode),
            ("hookup gemini cli", Agent::Gemini),
        ] {
            assert_eq!(parse_command(t), Some((HookAction::Connect, agent)), "{t}");
        }
        for t in ["hookup", "hookup anti-gravity and cursor", "anti-gravity", "hookup the gravity", "hookup evil code"] {
            assert_eq!(parse_command(t), None, "{t}");
        }
    }

    /// The exact transcripts of the live try (2026-10-19 19:17 and 18:52): a junk sentence, then the command.
    #[test]
    fn a_junk_first_sentence_does_not_hide_the_hook_command() {
        for (t, agent) in [
            ("hook up anti-pilot. hook up anti-gravity.", Agent::Antigravity),
            ("hook up anti-cognitive shutdown. hook up anti-gravity.", Agent::Antigravity),
            ("Practice. Hook up open code.", Agent::OpenCode),
        ] {
            assert_eq!(parse_command(t), Some((HookAction::Connect, agent)), "{t}");
        }
        // only the last sentence is tried, and it must be a hook command by itself
        for t in ["hook up antigravity. open chrome.", "hook up anti-pilot.", "what is the weather. play music.", "hook up evil. hook up nothing."] {
            assert_eq!(parse_command(t), None, "{t}");
        }
        assert_eq!(last_sentence("one. two. three"), Some("three"));
        assert_eq!(last_sentence("just one."), None);
        assert_eq!(last_sentence(""), None);
    }

    #[test]
    fn the_hook_voice_commands_know_every_agent() {
        for (t, action, agent) in [
            ("hook up antigravity", "connect", Agent::Antigravity),
            ("hook up agy", "connect", Agent::Antigravity),
            ("hook up opencode", "connect", Agent::OpenCode),
            ("hook up open code", "connect", Agent::OpenCode),
            ("let nexus know when gemini cli stops", "connect", Agent::Gemini),
            ("set up the cursor hook", "connect", Agent::Cursor),
            ("is the aider hook installed", "status", Agent::Aider),
            ("remove the opencode hook", "disconnect", Agent::OpenCode),
            ("hook up claude", "connect", Agent::Claude),
        ] {
            let (a, ag) = parse_command(t).unwrap_or_else(|| panic!("{t}"));
            assert_eq!((a.as_str(), ag), (action, agent), "{t}");
        }
        for t in [
            "hook up antigravity and cursor", "hook up claude and codex", "install cursor", "open cursor", "connect to aider", "hook up", "open the opencode hook",
            "hook up evil", "run the aider hook",
        ] {
            assert_eq!(parse_command(t), None, "{t}");
        }
    }

    #[test]
    fn a_window_or_a_terminal_names_at_most_one_agent() {
        assert_eq!(agent_in_title("Antigravity - my-project"), Some(Agent::Antigravity));
        assert_eq!(agent_in_title("OpenCode: fix the tests"), Some(Agent::OpenCode));
        assert_eq!(agent_in_title("agy - ~/proj"), Some(Agent::Antigravity));
        assert_eq!(agent_in_title("main.rs - Cursor"), Some(Agent::Cursor));
        assert_eq!(agent_in_title("gemini cli"), Some(Agent::Gemini));
        for none in ["claude cursor", "Windows PowerShell", "magyar.txt", "Gemini - Google Chrome", ""] {
            assert_eq!(agent_in_title(none), None, "{none}");
        }
    }

    fn proc(pid: u32, ppid: u32, name: &str, cmd: &str) -> crate::session_observer::ProcSnap {
        crate::session_observer::ProcSnap { pid, ppid, name: name.into(), cmd: cmd.into(), cpu_pct: 0.0, io_total: 0 }
    }

    #[test]
    fn the_agent_in_a_terminal_is_found_below_the_foreground_process() {
        let all = vec![
            proc(10, 1, "windowsterminal.exe", "windowsterminal.exe"),
            proc(11, 10, "pwsh.exe", "pwsh.exe"),
            proc(12, 11, "node.exe", r"node c:\users\me\.config\opencode\bin\opencode"),
            proc(20, 1, "claude.exe", "claude"),
            proc(30, 1, "explorer.exe", "explorer.exe"),
        ];
        assert_eq!(agent_under(10, &all, 999), Some(Agent::OpenCode), "claude is not below this terminal");
        assert_eq!(agent_under(30, &all, 999), None);
        assert_eq!(agent_under(20, &all, 999), Some(Agent::Claude), "the foreground process can be the agent itself");
        // two agents under one terminal: ask, never guess
        let mut two = all.clone();
        two.push(proc(13, 11, "codex.exe", "codex"));
        assert_eq!(agent_under(10, &two, 999), None);
        // NEXUS's own processes never count
        assert_eq!(agent_under(10, &all, 12), None);
    }
}
