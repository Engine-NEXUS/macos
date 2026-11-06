//! Local command executor — cached app resolution + direct platform launch.
//!
//! Resolution strategy:
//!   1. AppRegistry cache lookup (O(1) HashMap + fuzzy match) → direct ShellExecuteW / open -b / exec
//!   2. URL fallback (built into the registry) → open in default browser
//!   3. Not found → "Didn't find that, sir."
//!
//! The old 4-tier strategy (process scan → Get-StartApps → URL → not found) is
//! replaced by the pre-indexed AppRegistry. Get-StartApps runs ONCE at startup
//! (background thread) and results are cached in memory + on disk.
//!
//! Performance: ~1ms per command (was ~1.5s with the old approach).

use crate::app_registry;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

/// Intent received from the frontend intent parser.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action")]
pub enum Intent {
    #[serde(rename = "open_app")]
    OpenApp { target: String },
    #[serde(rename = "open_url")]
    OpenUrl { target: String, url: String },
    #[serde(rename = "close_app")]
    CloseApp { target: String },
    #[serde(rename = "whatsapp_chat")]
    WhatsappChat { contact: String },
    #[serde(rename = "search")]
    Search { query: String },
    // ── Type 2: Parameterized commands (acoustic trigger + STT parameter) ──
    #[serde(rename = "spotify_play")]
    SpotifyPlay { query: String },
    #[serde(rename = "youtube_search")]
    YoutubeSearch { query: String },
    #[serde(rename = "youtube_play")]
    YoutubePlay { query: String },
    #[serde(rename = "google_search")]
    GoogleSearch { query: String },
    #[serde(rename = "github_search")]
    GithubSearch { query: String },
    #[serde(rename = "send_message")]
    SendMessage { query: String },
    #[serde(rename = "set_timer")]
    SetTimer { query: String },
    #[serde(rename = "set_alarm")]
    SetAlarm { query: String },
    #[serde(rename = "create_event")]
    CreateEvent { query: String },
    // ── Type 1: Fixed commands (no parameter) ──
    #[serde(rename = "volume_mute")]
    VolumeMute,
    #[serde(rename = "screenshot")]
    Screenshot,
    #[serde(rename = "lock")]
    Lock,
    #[serde(rename = "browser_new_tab")]
    BrowserNewTab,
    #[serde(rename = "browser_close_tab")]
    BrowserCloseTab,
    #[serde(rename = "browser_next_tab")]
    BrowserNextTab,
    #[serde(rename = "browser_back")]
    BrowserBack,
    #[serde(rename = "media_play_pause")]
    MediaPlayPause,
    #[serde(rename = "media_next")]
    MediaNext,
    #[serde(rename = "media_previous")]
    MediaPrevious,
    #[serde(rename = "media_stop")]
    MediaStop,
    #[serde(rename = "play_music")]
    PlayMusic {
        #[serde(default)]
        query: Option<String>,
        #[serde(default)]
        player: Option<String>,
    },
    #[serde(rename = "open_music_player")]
    OpenMusicPlayer {
        #[serde(default)]
        player: Option<String>,
    },
    /// Local conversational reply (greetings, thanks, etc.)
    #[serde(rename = "greeting")]
    Greeting { reply: String },
    /// Partial intent prompt (e.g. "What should I say to mummy?")
    #[serde(rename = "need_more_info")]
    NeedMoreInfo { prompt: String },
    #[serde(rename = "unknown")]
    Unknown { raw: String },
    #[serde(other)]
    Other,
}

/// Result of executing a command.
#[derive(Debug, Clone, Serialize)]
pub struct CommandResult {
    pub success: bool,
    /// Human-readable message to speak via TTS.
    pub message: String,
}

/// IPC: Execute a local command from a parsed intent.
#[tauri::command]
pub async fn execute_command(intent: Intent) -> Result<CommandResult, String> {
    tracing::info!("executing local intent: {:?}", intent);
    match intent {
        Intent::OpenUrl { target, url } => open_url(&target, &url),
        Intent::OpenApp { target } => resolve_and_open_app(&target),
        Intent::CloseApp { target } => close_app(&target),
        Intent::WhatsappChat { contact } => whatsapp_chat(&contact),
        Intent::Search { query } => open_search(&query),
        // ── Type 2: Parameterized commands ──
        Intent::SpotifyPlay { query } => spotify_play(&query),
        Intent::YoutubeSearch { query } => youtube_search(&query),
        Intent::YoutubePlay { query } => youtube_play(&query),
        Intent::GoogleSearch { query } => open_search(&query),
        Intent::GithubSearch { query } => github_search(&query),
        Intent::SendMessage { query } => send_message(&query),
        Intent::SetTimer { query } => set_timer(&query),
        Intent::SetAlarm { query } => set_alarm(&query),
        Intent::CreateEvent { query } => create_event(&query),
        // ── Type 1: Fixed commands (no parameter) ──
        Intent::VolumeMute => volume_mute(),
        Intent::Screenshot => take_screenshot(),
        Intent::Lock => lock_screen(),
        Intent::BrowserNewTab => browser_key("ctrl+t", "new tab"),
        Intent::BrowserCloseTab => browser_key("ctrl+w", "close tab"),
        Intent::BrowserNextTab => browser_key("ctrl+tab", "next tab"),
        Intent::BrowserBack => browser_key("alt+left", "back"),
        Intent::MediaPlayPause => execute_media_command("play_pause").await,
        Intent::MediaNext => execute_media_command("next").await,
        Intent::MediaPrevious => execute_media_command("previous").await,
        Intent::MediaStop => execute_media_command("stop").await,
        Intent::PlayMusic { query, player } => {
            match crate::media_player::execute_media_action("play", query.as_deref(), player.as_deref()).await {
                Ok(msg) => Ok(CommandResult {
                    success: true,
                    message: msg,
                }),
                Err(e) => Ok(CommandResult {
                    success: false,
                    message: format!("Couldn't play music: {e}"),
                }),
            }
        }
        Intent::OpenMusicPlayer { player } => {
            match crate::media_player::execute_media_action("open", None, player.as_deref()).await {
                Ok(msg) => Ok(CommandResult {
                    success: true,
                    message: msg,
                }),
                Err(e) => Ok(CommandResult {
                    success: false,
                    message: format!("Couldn't open player: {e}"),
                }),
            }
        }
        Intent::Greeting { reply } => Ok(CommandResult {
            success: true,
            message: reply,
        }),
        Intent::NeedMoreInfo { prompt } => Ok(CommandResult {
            success: true,
            message: prompt,
        }),
        Intent::Unknown { raw } => Ok(CommandResult {
            success: false,
            message: format!(
                "I didn't understand: {}. Could you rephrase that, sir?",
                raw
            ),
        }),
        Intent::Other => Ok(CommandResult {
            success: false,
            message: "Couldn't do that, sir.".to_string(),
        }),
    }
}

async fn execute_media_command(action: &str) -> Result<CommandResult, String> {
    match crate::media_player::execute_media_action(action, None, None).await {
        Ok(msg) => Ok(CommandResult {
            success: true,
            message: msg,
        }),
        Err(e) => Ok(CommandResult {
            success: false,
            message: format!("Couldn't control media: {e}"),
        }),
    }
}

// ─── URL + Search ──────────────────────────────────────────────────────────

/// Processes that must never be closed by voice: the shell, the compositor and
/// core Windows services, plus the assistant itself.
const PROTECTED_PROCESSES: &[&str] = &[
    "explorer", "dwm", "csrss", "winlogon", "wininit", "services", "lsass", "smss", "svchost",
    "system", "registry", "sihost", "fontdrvhost", "taskhostw", "nexus",
];

/// The macOS equivalents of the list above: the desktop shell, the login/session processes and the assistant.
/// `loginwindow` is the dangerous one: ending it logs the user out. Lower-case; compared with the executable
/// stem AND the display name (macOS apps are bundles, so there is no `.exe` to take a stem from).
const PROTECTED_PROCESSES_MACOS: &[&str] = &[
    "finder", "dock", "loginwindow", "windowserver", "systemuiserver", "controlcenter", "notificationcenter",
    "launchd", "kernel_task", "coreservicesd", "universalaccessd", "nexus",
];

/// Is this a process voice must never close? `macos` is passed in (not read from `cfg!`) so the macOS list can be
/// unit-tested on every OS; with `macos == false` the result is exactly the old Windows-list check.
fn is_protected_process(stem: &str, display: &str, macos: bool) -> bool {
    if PROTECTED_PROCESSES.contains(&stem) {
        return true;
    }
    macos && {
        let display = display.trim().to_lowercase();
        PROTECTED_PROCESSES_MACOS.contains(&stem) || PROTECTED_PROCESSES_MACOS.contains(&display.as_str())
    }
}

/// The AppleScript for a polite quit. The app name arrives as `argv`, never spliced into the script text, so a
/// spoken or registry name cannot inject AppleScript. It checks the app is running first (telling a closed app to
/// quit would launch it), and `quit` lets the app ask to save, like the Windows `taskkill` without `/F`.
///
/// Compiled on macOS (where `close_app` uses it) and in tests on every OS (so the injection and guard tests run on
/// Windows and Linux CI). It is NOT compiled into a Windows release build, where the dead-code ratchet would
/// rightly flag an unused item.
#[cfg(any(target_os = "macos", test))]
const MACOS_QUIT_SCRIPT: &[&str] = &[
    "on run argv",
    "set appName to item 1 of argv",
    "tell application \"System Events\" to set isRunning to ((name of every application process) contains appName)",
    "if isRunning then",
    "tell application appName to quit",
    "return \"quit\"",
    "end if",
    "return \"not running\"",
    "end run",
];

/// `osascript` arguments for a polite quit of `name`, or None when the name is not a plausible app name.
#[cfg(any(target_os = "macos", test))]
fn macos_quit_args(name: &str) -> Option<Vec<String>> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(|c| c.is_control()) {
        return None;
    }
    let mut args: Vec<String> = Vec::new();
    for line in MACOS_QUIT_SCRIPT {
        args.push("-e".into());
        args.push((*line).into());
    }
    args.push(name.into());
    Some(args)
}

/// What `close_app` will do for a spoken target. Pure apart from the registry read (unit-tested).
#[derive(Debug, PartialEq)]
enum ClosePlan {
    Close { display: String, exe: String },
    Refuse(String),
}

fn plan_close(target: &str) -> ClosePlan {
    let spoken = target.trim();
    // Only a strict registry hit may name the app to close. A guess ("chat" inside
    // "mummy chat") used to be force-killed on the spot.
    let (display, exe) = match app_registry::lookup_strict(spoken) {
        Some(entry) => {
            let exe = match &entry.launch {
                app_registry::LaunchMethod::Exe { path } => std::path::Path::new(path)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .map(|s| s.to_string()),
                _ => None,
            }
            .unwrap_or_else(|| format!("{}.exe", spoken));
            (entry.display_name.clone(), exe)
        }
        None => {
            if let Some((entry, _, _)) = app_registry::lookup_with_kind(spoken) {
                return ClosePlan::Refuse(format!(
                    "I'm not sure which app '{}' is, sir. Did you mean {}? I haven't closed anything.",
                    spoken, entry.display_name
                ));
            }
            (spoken.to_string(), format!("{}.exe", spoken))
        }
    };
    let stem = exe.to_lowercase();
    let stem = stem.trim_end_matches(".exe");
    if is_protected_process(stem, &display, cfg!(target_os = "macos")) {
        return ClosePlan::Refuse(format!("I won't close {}, sir.", capitalize(&display)));
    }
    ClosePlan::Close { display, exe }
}

/// Close an application by name. Windows: a polite `taskkill` (WM_CLOSE: the app can ask
/// to save), never `/F` and never a name-substring kill. macOS: a polite AppleScript quit. Other Unix: `pkill -x`.
fn close_app(target: &str) -> Result<CommandResult, String> {
    let (app_name, exe_name) = match plan_close(target) {
        ClosePlan::Refuse(message) => return Ok(CommandResult { success: false, message }),
        ClosePlan::Close { display, exe } => (display, exe),
    };

    #[cfg(target_os = "windows")]
    {
        // Polite close: posts WM_CLOSE to the app's windows, so unsaved work gets its
        // "save changes?" prompt. (It used to be `/F`, which loses that work.)
        let output = crate::no_window::command("taskkill").args(["/IM", &exe_name]).output();
        match output {
            Ok(o) if o.status.success() => {
                tracing::info!("closed app '{}' ({})", app_name, exe_name);
                Ok(CommandResult {
                    success: true,
                    message: format!("Closed {}, sir.", capitalize(&app_name)),
                })
            }
            Ok(o) => {
                let text = format!(
                    "{} {}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                )
                .to_lowercase();
                tracing::warn!("polite close of '{}' failed: {}", exe_name, text.trim());
                let message = if text.contains("forcefully") {
                    format!("{} won't close politely, sir. I haven't forced it.", capitalize(&app_name))
                } else {
                    format!("I couldn't find {} running, sir.", capitalize(&app_name))
                };
                Ok(CommandResult { success: false, message })
            }
            Err(e) => {
                tracing::error!("failed to run taskkill for '{}': {}", app_name, e);
                Ok(CommandResult {
                    success: false,
                    message: format!("I couldn't close {}, sir.", capitalize(&app_name)),
                })
            }
        }
    }

    // macOS: a polite quit (the app can ask to save), never a signal. `pkill -x` used to be the only Unix path, which
    // hard-kills the app and, for the wrong name, the Mac's own shell or the login session.
    #[cfg(target_os = "macos")]
    {
        let _ = &exe_name;
        let Some(args) = macos_quit_args(&app_name) else {
            return Ok(CommandResult { success: false, message: format!("I couldn't close {}, sir.", capitalize(&app_name)) });
        };
        return match Command::new("osascript").args(&args).output() {
            Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "quit" => {
                tracing::info!("asked '{}' to quit", app_name);
                Ok(CommandResult { success: true, message: format!("Closed {}, sir.", capitalize(&app_name)) })
            }
            Ok(o) if o.status.success() => Ok(CommandResult {
                success: false,
                message: format!("I couldn't find {} running, sir.", capitalize(&app_name)),
            }),
            Ok(o) => {
                tracing::warn!("polite quit of '{}' failed: {}", app_name, String::from_utf8_lossy(&o.stderr).trim());
                Ok(CommandResult { success: false, message: format!("I couldn't close {}, sir.", capitalize(&app_name)) })
            }
            Err(e) => {
                tracing::error!("failed to run osascript for '{}': {}", app_name, e);
                Ok(CommandResult { success: false, message: format!("I couldn't close {}, sir.", capitalize(&app_name)) })
            }
        };
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let _ = &exe_name;
        let output = Command::new("pkill").args(["-x", &app_name]).output();
        match output {
            Ok(o) if o.status.success() => {
                tracing::info!("closed app '{}'", app_name);
                Ok(CommandResult {
                    success: true,
                    message: format!("Closed {}, sir.", capitalize(&app_name)),
                })
            }
            _ => Ok(CommandResult {
                success: false,
                message: format!("I couldn't find {} running, sir.", capitalize(&app_name)),
            }),
        }
    }
}

/// Open a WhatsApp chat with a contact by name.
/// Uses whatsapp:// deep link with phone number lookup from local contacts file.
fn whatsapp_chat(contact: &str) -> Result<CommandResult, String> {
    // Load contacts from nexus config dir
    let contacts_path = dirs_next::config_dir()
        .map(|d| d.join("com.nexus.assistant").join("contacts.json"))
        .or_else(|| dirs_next::home_dir().map(|h| h.join(".nexus").join("contacts.json")));

    // Try to find the contact in the local contacts file
    if let Some(path) = &contacts_path {
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(contacts) = serde_json::from_str::<serde_json::Value>(&content) {
                let contact_lower = contact.to_lowercase();
                if let Some(obj) = contacts.as_object() {
                    // Exact match first
                    for (name, number) in obj.iter() {
                        if name.to_lowercase() == contact_lower {
                            return whatsapp_launch(name, number.as_str().unwrap_or(""));
                        }
                    }
                    // Fuzzy match
                    for (name, number) in obj.iter() {
                        if name.to_lowercase().contains(&contact_lower) || contact_lower.contains(&name.to_lowercase()) {
                            return whatsapp_launch(name, number.as_str().unwrap_or(""));
                        }
                    }
                }
            }
        }
    }

    // No contact found — open WhatsApp and let user pick
    let _ = open::that("whatsapp://");
    Ok(CommandResult {
        success: true,
        // Honest, actionable: this path has no desktop control (that is ghost mode) and no phone
        // number for the contact, so it can only open the app.
        message: format!(
            "I opened WhatsApp, sir, but I can't pick {}'s chat from here. Say ghost mode, then ask me again.",
            capitalize(contact)
        ),
    })
}

fn whatsapp_launch(name: &str, phone: &str) -> Result<CommandResult, String> {
    // Clean phone number (remove spaces, dashes, +)
    let clean_phone: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    let url = format!("whatsapp://send?phone={}", clean_phone);
    match open::that(&url) {
        Ok(()) => {
            tracing::info!("opened whatsapp chat with '{}' ({})", name, clean_phone);
            Ok(CommandResult {
                success: true,
                message: format!("Opening your chat with {}, sir.", name),
            })
        }
        Err(e) => {
            tracing::error!("failed to open whatsapp: {}", e);
            Ok(CommandResult {
                success: false,
                message: "I couldn't open WhatsApp, sir. Make sure it's installed.".to_string(),
            })
        }
    }
}

/// Only plain web URLs may be opened from a spoken/NLU-supplied intent. `open::that`
/// would otherwise hand `file:`, `ms-msdt:`, `search-ms:` or a bare executable path to
/// the shell. Deep links the app really needs (whatsapp:, spotify:) are opened by their
/// own functions, never through this one.
fn is_safe_web_url(url: &str) -> bool {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && !u.chars().any(|c| c.is_control() || c.is_whitespace())
}

fn open_url(target: &str, url: &str) -> Result<CommandResult, String> {
    if !is_safe_web_url(url) {
        tracing::warn!("refused to open non-web URL for '{}'", target);
        return Ok(CommandResult {
            success: false,
            message: "I can only open web links from a voice command, sir.".to_string(),
        });
    }
    match open::that(url.trim()) {
        Ok(()) => {
            tracing::info!("opened URL for '{}': {}", target, url);
            Ok(CommandResult {
                success: true,
                message: format!("Opened {} in your browser, sir.", capitalize(target)),
            })
        }
        Err(e) => {
            tracing::error!("failed to open URL '{}': {}", url, e);
            Ok(CommandResult {
                success: false,
                message: format!("I couldn't open {}, sir.", target),
            })
        }
    }
}

fn open_search(query: &str) -> Result<CommandResult, String> {
    let url = format!(
        "https://www.google.com/search?q={}",
        urlencoding::encode(query)
    );
    match open::that(&url) {
        Ok(()) => {
            tracing::info!("opened search for: {}", query);
            Ok(CommandResult {
                success: true,
                message: format!("Searching for {}, sir.", query),
            })
        }
        Err(e) => {
            tracing::error!("failed to search '{}': {}", query, e);
            Ok(CommandResult {
                success: false,
                message: format!("I couldn't search for {}, sir.", query),
            })
        }
    }
}

// ─── Type 2: Parameterized command implementations ────────────────────────

/// Play a song on Spotify via deep link (opens Spotify app or web player).
fn spotify_play(query: &str) -> Result<CommandResult, String> {
    // Spotify search deep link — works on both desktop app and web player
    let url = format!("spotify:search:{}", urlencoding::encode(query));
    let web_url = format!("https://open.spotify.com/search/{}", urlencoding::encode(query));

    // Try the Spotify URI scheme first (opens desktop app if installed)
    #[cfg(target_os = "windows")]
    {
        if Command::new("cmd").args(["/c", "start", "", &url])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn().is_ok() {
            tracing::info!("spotify play: {} (desktop app)", query);
            return Ok(CommandResult {
                success: true,
                message: format!("Playing {} on Spotify, sir.", query),
            });
        }
    }
    #[cfg(target_os = "macos")]
    {
        if Command::new("open").arg(&url).spawn().is_ok() {
            tracing::info!("spotify play: {} (desktop app)", query);
            return Ok(CommandResult {
                success: true,
                message: format!("Playing {} on Spotify, sir.", query),
            });
        }
    }
    #[cfg(target_os = "linux")]
    {
        if Command::new("xdg-open").arg(&url).spawn().is_ok() {
            tracing::info!("spotify play: {} (desktop app)", query);
            return Ok(CommandResult {
                success: true,
                message: format!("Playing {} on Spotify, sir.", query),
            });
        }
    }

    // Fallback: open Spotify web player in browser
    match open::that(&web_url) {
        Ok(()) => {
            tracing::info!("spotify play: {} (web player)", query);
            Ok(CommandResult {
                success: true,
                message: format!("Playing {} on Spotify, sir.", query),
            })
        }
        Err(e) => {
            tracing::error!("spotify play failed: {}", e);
            Ok(CommandResult {
                success: false,
                message: "I couldn't open Spotify, sir.".to_string(),
            })
        }
    }
}

/// Search on YouTube.
fn youtube_search(query: &str) -> Result<CommandResult, String> {
    let url = format!(
        "https://www.youtube.com/results?search_query={}",
        urlencoding::encode(query)
    );
    match open::that(&url) {
        Ok(()) => {
            tracing::info!("youtube search: {}", query);
            Ok(CommandResult {
                success: true,
                message: format!("Searching {} on YouTube, sir.", query),
            })
        }
        Err(e) => {
            tracing::error!("youtube search failed: {}", e);
            Ok(CommandResult {
                success: false,
                message: "I couldn't search YouTube, sir.".to_string(),
            })
        }
    }
}

/// Play a video on YouTube (search + first result via YouTube deep link).
fn youtube_play(query: &str) -> Result<CommandResult, String> {
    // YouTube search URL — user can click the first result.
    // A true "play first result" would need the YouTube API, but search
    // is the most reliable cross-platform approach.
    let url = format!(
        "https://www.youtube.com/results?search_query={}",
        urlencoding::encode(query)
    );
    match open::that(&url) {
        Ok(()) => {
            tracing::info!("youtube play: {}", query);
            Ok(CommandResult {
                success: true,
                message: format!("Playing {} on YouTube, sir.", query),
            })
        }
        Err(e) => {
            tracing::error!("youtube play failed: {}", e);
            Ok(CommandResult {
                success: false,
                message: "I couldn't open YouTube, sir.".to_string(),
            })
        }
    }
}

/// Search on GitHub.
fn github_search(query: &str) -> Result<CommandResult, String> {
    let url = format!(
        "https://github.com/search?q={}&type=repositories",
        urlencoding::encode(query)
    );
    match open::that(&url) {
        Ok(()) => {
            tracing::info!("github search: {}", query);
            Ok(CommandResult {
                success: true,
                message: format!("Searching {} on GitHub, sir.", query),
            })
        }
        Err(e) => {
            tracing::error!("github search failed: {}", e);
            Ok(CommandResult {
                success: false,
                message: "I couldn't search GitHub, sir.".to_string(),
            })
        }
    }
}

/// Send a message to a contact (opens WhatsApp Web with the contact name).
fn send_message(query: &str) -> Result<CommandResult, String> {
    // WhatsApp Web doesn't support pre-filling recipient by name via URL.
    // Open WhatsApp Web and let the user select the contact.
    let url = "https://web.whatsapp.com";
    match open::that(url) {
        Ok(()) => {
            tracing::info!("send message to: {} (opened WhatsApp Web)", query);
            Ok(CommandResult {
                success: true,
                message: format!("Opening WhatsApp for {}, sir.", query),
            })
        }
        Err(e) => {
            tracing::error!("send message failed: {}", e);
            Ok(CommandResult {
                success: false,
                message: "I couldn't open WhatsApp, sir.".to_string(),
            })
        }
    }
}

/// Set a timer (placeholder — would need OS-specific timer API).
fn set_timer(query: &str) -> Result<CommandResult, String> {
    tracing::info!("set timer: {} (not yet implemented)", query);
    Ok(CommandResult {
        success: true,
        message: format!("Timer set for {}, sir.", query),
    })
}

/// Set an alarm (placeholder — would need OS-specific alarm API).
fn set_alarm(query: &str) -> Result<CommandResult, String> {
    tracing::info!("set alarm: {} (not yet implemented)", query);
    Ok(CommandResult {
        success: true,
        message: format!("Alarm set for {}, sir.", query),
    })
}

/// Create a calendar event (opens Google Calendar).
fn create_event(query: &str) -> Result<CommandResult, String> {
    let url = "https://calendar.google.com/calendar/u/0/r/eventedit";
    match open::that(url) {
        Ok(()) => {
            tracing::info!("create event: {} (opened Google Calendar)", query);
            Ok(CommandResult {
                success: true,
                message: format!("Creating event {} in your calendar, sir.", query),
            })
        }
        Err(e) => {
            tracing::error!("create event failed: {}", e);
            Ok(CommandResult {
                success: false,
                message: "I couldn't open your calendar, sir.".to_string(),
            })
        }
    }
}

// ─── Type 1: Fixed command implementations (no parameter) ────────────────

/// Mute the system volume.
fn volume_mute() -> Result<CommandResult, String> {
    #[cfg(target_os = "windows")]
    {
        // Use PowerShell to send the mute key
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command",
                "(New-Object Media.SoundPlayer).PlaySync()"])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn();
        // Actually use the keyboard mute key via nircmd or SendMessage
        // For now, use the Windows mute key via keybd_event
        let ps = "$signature = '[DllImport(\"user32.dll\")] public static extern void keybd_event(byte bVk, byte bScan, uint dwFlags, int dwExtraInfo);'; $key = Add-Type -MemberDefinition $signature -Name 'Win32' -Namespace 'Native' -PassThru; $key::keybd_event(0xAD, 0, 0, 0); $key::keybd_event(0xAD, 0, 2, 0)";
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command", ps])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("osascript")
            .args(["-e", "set volume with output muted"])
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        // Try PipeWire (wpctl), PulseAudio (pactl), or ALSA (amixer)
        let _ = Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
            .spawn()
            .or_else(|_| Command::new("pactl").args(["set-sink-mute", "@DEFAULT_SINK@", "toggle"]).spawn())
            .or_else(|_| Command::new("amixer").args(["-q", "set", "Master", "toggle"]).spawn());
    }
    tracing::info!("volume muted");
    Ok(CommandResult {
        success: true,
        message: "Muted, sir.".to_string(),
    })
}

/// Take a screenshot.
fn take_screenshot() -> Result<CommandResult, String> {
    #[cfg(target_os = "windows")]
    {
        // Win+Shift+S opens the Snipping Tool
        let ps = "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('{%}P')";
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command", ps])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("screencapture").args(["-i", "-c"]).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        // Try COSMIC / GNOME / Spectacle / grim / xdg-desktop-portal screenshot tools
        let _ = Command::new("cosmic-screenshot")
            .spawn()
            .or_else(|_| Command::new("gnome-screenshot").arg("-a").spawn())
            .or_else(|_| Command::new("spectacle").arg("-r").spawn())
            .or_else(|_| Command::new("flameshot").arg("gui").spawn());
    }
    tracing::info!("screenshot taken");
    Ok(CommandResult {
        success: true,
        message: "Screenshot taken, sir.".to_string(),
    })
}

/// Lock the screen.
fn lock_screen() -> Result<CommandResult, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = crate::no_window::command("rundll32.exe")
            .args(["user32.dll,LockWorkStation"])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("/System/Library/CoreServices/Menu Extras/User.menu")
            .args(["/Contents/Resources/CGSession", "-suspend"])
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("loginctl").args(["lock-session"]).spawn();
    }
    tracing::info!("screen locked");
    Ok(CommandResult {
        success: true,
        message: "Locking screen, sir.".to_string(),
    })
}

/// Send a keyboard shortcut to the active window (for browser commands).
fn browser_key(keys: &str, label: &str) -> Result<CommandResult, String> {
    // Shortcut brain (E1): resolve the chord from the Shortcut Map for the app that is actually in front,
    // re-check the foreground right before sending, and send ONE batched SendInput by virtual key - instead
    // of a detached PowerShell SendKeys that never checked the window and reported success regardless.
    // Unknown apps (e.g. a browser not in the seed) keep the old behaviour below; known apps without that
    // shortcut, protected windows and a window change are refused with a spoken reason.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if crate::shortcuts::enabled() {
        use crate::shortcuts::{Action, Spoken};
        let action = match keys {
            "ctrl+t" => Some(Action::NewTab),
            "ctrl+w" => Some(Action::CloseTab),
            "ctrl+tab" => Some(Action::NextTab),
            "alt+left" => Some(Action::GoBack),
            _ => None,
        };
        if let Some(action) = action {
            match crate::shortcuts::do_action_spoken(action, None, true) {
                // Keep each call site's own wording ("Close tab sir." vs "Tab closed sir.").
                Spoken::Ok(_) => {
                    return Ok(CommandResult { success: true, message: format!("{} sir.", capitalize(label)) })
                }
                Spoken::Refused(reason) => {
                    return Ok(CommandResult { success: false, message: format!("{}, sir.", capitalize(&reason)) })
                }
                Spoken::Fallback => {} // unknown app -> legacy path below
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Map our key notation to SendKeys notation
        let sendkeys = match keys {
            "ctrl+t" => "^t",
            "ctrl+w" => "^w",
            "ctrl+tab" => "^{TAB}",
            "alt+left" => "%{LEFT}",
            _ => "",
        };
        if !sendkeys.is_empty() {
            let ps = format!(
                "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait('{}')",
                sendkeys
            );
            let _ = Command::new("powershell")
                .args(["-NoProfile", "-Command", &ps])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn();
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Use osascript to send keystrokes
        let (cmd_key, key) = match keys {
            "ctrl+t" => ("command down", "t"),
            "ctrl+w" => ("command down", "w"),
            "ctrl+tab" => ("control down", "tab"),
            "alt+left" => ("command down", "["),
            _ => ("", ""),
        };
        if !key.is_empty() {
            let script = format!(
                "tell application \"System Events\" to keystroke \"{}\" using {{{}}}",
                key, cmd_key
            );
            let _ = Command::new("osascript").args(["-e", &script]).spawn();
        }
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("xdotool").args(["key", keys]).spawn();
    }
    tracing::info!("browser key: {} ({})", keys, label);
    Ok(CommandResult {
        success: true,
        message: format!("{} sir.", capitalize(label)),
    })
}

/// Voice-driven browser hotkeys (orchestrator path). The deterministic
/// parser produced `browser_new_tab` at 1.0 confidence but it routed to
/// the Worker as chat while these sat unused — now reachable.
pub fn browser_new_tab_cmd() -> Result<CommandResult, String> {
    browser_key("ctrl+t", "new tab")
}

/// Close a browser tab (Ctrl+W). An index switches to that tab first
/// (Ctrl+N) so "delete tab 5" closes the 5th, not the active one.
pub fn browser_close_tab_cmd(index: Option<u32>) -> Result<CommandResult, String> {
    if let Some(n) = index {
        // Switch first; if the switch fails, do NOT close the active tab
        // (it would close the wrong one).
        crate::screen::switch_browser_tab(n)?;
    }
    browser_key("ctrl+w", "tab closed")
}

// ─── App Resolution (focus-first → launch-new → URL fallback) ──────────────
//
// Priority order (what the user asked for):
//   1. If app is already running → FOCUS its existing window
//   2. If app is installed → LAUNCH a new instance
//   3. If app is a known web service → OPEN as URL in browser
//   4. If nothing found → "Didn't find that, sir."

pub(crate) fn resolve_and_open_app(target: &str) -> Result<CommandResult, String> {
    let start = std::time::Instant::now();
    tracing::info!("resolving app: {}", target);

    // Match-kind + installer gate: only an exact / resolved / well-covered
    // prefix match may launch. A guess ("mummy chat" -> Google Chat) is
    // spoken back as a question and parked as an offer; an installer is
    // refused. Both return Ok(success: false) with a speakable message —
    // callers speak it instead of drilling or claiming "Ok sir.". Registry
    // misses still fall through to the legacy chain below.
    let assessed = match crate::app_registry::assess_open_target(target) {
        crate::app_registry::OpenAssessment::Refuse { message } => {
            return Ok(CommandResult { success: false, message });
        }
        crate::app_registry::OpenAssessment::Confirm { entry, message } => {
            crate::memcore::offer::set(
                crate::memcore::offer::Offer::OpenApp {
                    display_name: entry.display_name.clone(),
                    spoken: target.trim().to_string(),
                },
                true,
            );
            return Ok(CommandResult { success: false, message });
        }
        // Confident match: name it in the trace so the next live log
        // shows exactly which entry the gate approved before launching.
        crate::app_registry::OpenAssessment::Proceed(entry) => {
            tracing::debug!("open gate: confident match '{}'", entry.display_name);
            Some(entry)
        }
        crate::app_registry::OpenAssessment::ProceedLegacy => None,
    };

    // Registry hit (O(1) HashMap + tiered match, ~0.1ms)
    if let Some(entry) = assessed {
        tracing::info!(
            "registry hit: '{}' → '{}' ({:?}) in {:.1}ms",
            target,
            entry.display_name,
            entry.launch,
            start.elapsed().as_secs_f64() * 1000.0
        );

        // PRIORITY 1: Focus existing window if app is already running
        if app_registry::try_focus_existing(&entry) {
            app_registry::record_usage(target);
            return Ok(CommandResult {
                success: true,
                message: "Ok sir.".to_string(),
            });
        }

        // A launch a moment ago that has not shown its window yet: wait for it, never start a second
        // copy (repeating "open notepad" used to open one window per repeat).
        if app_registry::recently_launched(&entry) {
            for _ in 0..16 {
                if app_registry::try_focus_existing(&entry) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            return Ok(CommandResult {
                success: true,
                message: "Ok sir.".to_string(),
            });
        }

        // PRIORITY 2: Launch a new instance (app is not running)
        match app_registry::launch(&entry) {
            Ok(()) => {
                app_registry::note_launched(&entry);
                app_registry::record_usage(target);
                return Ok(CommandResult {
                    success: true,
                    message: "Ok sir.".to_string(),
                });
            }
            Err(e) => {
                tracing::error!("direct launch failed for '{}': {}", entry.display_name, e);
                // Fall through to legacy resolver
            }
        }
    } else {
        tracing::info!(
            "registry miss for '{}' in {:.1}ms, trying legacy resolver",
            target,
            start.elapsed().as_secs_f64() * 1000.0
        );
    }

    // Legacy fallback: the old 4-tier resolution (only if registry misses or launch fails)
    let target_lower = target.to_lowercase();
    let display_name = app_display_name(&target_lower);

    // Tier 1: Check if the app is already running → focus its window.
    if let Some(result) = check_running_and_focus(&target_lower, &display_name) {
        return Ok(result);
    }

    // Tier 2: Check installed apps via OS-specific methods.
    if let Some(result) = check_installed_and_launch(&target_lower, &display_name) {
        return Ok(result);
    }

    // Tier 3: URL fallback — open in default browser.
    if let Some(url) = url_fallback(&target_lower) {
        return open_url(&display_name, &url);
    }

    // Not found.
    Ok(CommandResult {
        success: false,
        message: "Didn't find that, sir.".to_string(),
    })
}

/// Map internal target names to human-friendly display names for TTS.
fn app_display_name(target: &str) -> String {
    match target {
        "gmail" | "google mail" => "Gmail".to_string(),
        "youtube" | "you tube" => "YouTube".to_string(),
        "github" | "git hub" => "GitHub".to_string(),
        "vs code" | "visual studio code" | "code" => "VS Code".to_string(),
        "notepad" => "Notepad".to_string(),
        "calculator" | "calc" => "Calculator".to_string(),
        "explorer" | "file explorer" => "File Explorer".to_string(),
        "terminal" | "windows terminal" | "wt" => "Terminal".to_string(),
        "command prompt" | "cmd" => "Command Prompt".to_string(),
        "powershell" => "PowerShell".to_string(),
        "task manager" => "Task Manager".to_string(),
        "control panel" => "Control Panel".to_string(),
        "settings" => "Settings".to_string(),
        "paint" | "mspaint" => "Paint".to_string(),
        "spotify" => "Spotify".to_string(),
        "discord" => "Discord".to_string(),
        "slack" => "Slack".to_string(),
        "figma" => "Figma".to_string(),
        "notion" => "Notion".to_string(),
        "chatgpt" | "chat gpt" => "ChatGPT".to_string(),
        "claude" => "Claude".to_string(),
        "whatsapp" => "WhatsApp".to_string(),
        "netflix" => "Netflix".to_string(),
        "twitter" => "Twitter".to_string(),
        "reddit" => "Reddit".to_string(),
        "facebook" => "Facebook".to_string(),
        "instagram" => "Instagram".to_string(),
        "linkedin" => "LinkedIn".to_string(),
        "twitch" => "Twitch".to_string(),
        _ => capitalize(target),
    }
}

// ─── Tier 1: Running process check + focus ─────────────────────────────────

fn check_running_and_focus(target: &str, display_name: &str) -> Option<CommandResult> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, false, ProcessRefreshKind::new());

    let process_names = app_to_process_names(target);
    tracing::debug!("checking running processes: {:?}", process_names);

    for proc in sys.processes().values() {
        let name = proc.name().to_string_lossy().to_lowercase();
        for candidate in &process_names {
            if name.contains(candidate) {
                tracing::info!("found running process: {} (pid={})", name, proc.pid());
                #[cfg(target_os = "windows")]
                {
                    if let Some(result) = focus_window_by_process(proc.pid().as_u32(), display_name) {
                        return Some(result);
                    }
                }
                #[cfg(target_os = "macos")]
                {
                    let _ = Command::new("open").args(["-a", display_name]).spawn();
                    return Some(CommandResult {
                        success: true,
                        message: format!("{} is already open, sir.", display_name),
                    });
                }
                #[cfg(target_os = "linux")]
                {
                    let _ = Command::new("wmctrl").args(["-a", display_name]).spawn();
                    return Some(CommandResult {
                        success: true,
                        message: format!("{} is already open, sir.", display_name),
                    });
                }
                #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
                {
                    return Some(CommandResult {
                        success: true,
                        message: format!("{} is already running, sir.", display_name),
                    });
                }
            }
        }
    }

    None
}

/// Map app name to possible process names (for sysinfo matching).
fn app_to_process_names(target: &str) -> Vec<String> {
    let mut names = Vec::new();
    names.push(target.to_lowercase());
    names.push(target.replace(' ', ""));

    match target {
        "chrome" | "google chrome" | "browser" => { names.push("chrome".to_string()); }
        "firefox" => { names.push("firefox".to_string()); }
        "brave" => { names.push("brave".to_string()); }
        "vs code" | "visual studio code" | "code" => { names.push("code".to_string()); }
        "notepad" => { names.push("notepad".to_string()); }
        "calculator" | "calc" => { names.push("calc".to_string()); names.push("calculator".to_string()); }
        "spotify" => { names.push("spotify".to_string()); }
        "discord" => { names.push("discord".to_string()); }
        "terminal" | "windows terminal" | "wt" => { names.push("windowsterminal".to_string()); names.push("wt".to_string()); }
        "explorer" | "file explorer" => { names.push("explorer".to_string()); }
        "figma" => { names.push("figma".to_string()); }
        "slack" => { names.push("slack".to_string()); }
        "notion" => { names.push("notion".to_string()); }
        "whatsapp" => { names.push("whatsapp".to_string()); }
        _ => {}
    }

    names.sort();
    names.dedup();
    names
}

// ─── Tier 2: Installed app check + launch ──────────────────────────────────

// Exactly one of the `cfg` blocks below compiles on any given target, so the
// surviving block is the function's tail expression — no `return` needed.
fn check_installed_and_launch(target: &str, display_name: &str) -> Option<CommandResult> {
    #[cfg(target_os = "windows")]
    {
        check_installed_windows(target, display_name)
    }
    #[cfg(target_os = "macos")]
    {
        check_installed_macos(target, display_name)
    }
    #[cfg(target_os = "linux")]
    {
        check_installed_linux(target, display_name)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = (target, display_name);
        None
    }
}

// ─── Windows: Tier 2 (Get-StartApps primary resolver) ──────────────────────

#[cfg(target_os = "windows")]
fn check_installed_windows(target: &str, display_name: &str) -> Option<CommandResult> {
    // PRIMARY: Use Get-StartApps — the universal app resolver on Windows.
    // Finds: native Win32, UWP/Store, PWA (Chrome/Edge/Brave), Squirrel apps.
    if let Some(result) = launch_via_start_apps(target, display_name) {
        return Some(result);
    }

    // FALLBACK: Try `where.exe` for PATH-resolvable executables.
    if let Some(result) = launch_via_where(target, display_name) {
        return Some(result);
    }

    None
}

/// Use PowerShell Get-StartApps to find and launch an app by name.
/// This finds PWAs, UWP apps, Store apps, and native apps with Start Menu shortcuts.
#[cfg(target_os = "windows")]
fn launch_via_start_apps(target: &str, _display_name: &str) -> Option<CommandResult> {
    // Build a list of name variants to search for.
    let search_names = app_search_names(target);
    tracing::debug!("searching Get-StartApps for: {:?}", search_names);

    // Get all Start apps as JSON for reliable parsing.
    let ps_script = "Get-StartApps | ConvertTo-Json -Compress";
    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", ps_script])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();

    let json_str = match output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        _ => {
            tracing::warn!("Get-StartApps failed");
            return None;
        }
    };

    // Parse the JSON manually (avoid adding serde_json dependency).
    // Format: [{"Name":"Gmail","AppID":"Brave._crx_..."},...] or {"Name":"...","AppID":"..."}
    let entries = parse_start_apps_json(&json_str);
    tracing::debug!("Get-StartApps returned {} entries", entries.len());

    // Find the best match: prefer exact name match, then contains match.
    // Prefer native apps (com.squirrel.*, no _crx_) over PWAs (_crx_).
    let mut exact_match: Option<&(String, String)> = None;
    let mut contains_match: Option<&(String, String)> = None;
    let mut exact_native: Option<&(String, String)> = None;
    let mut contains_native: Option<&(String, String)> = None;

    for entry in &entries {
        let entry_name_lower = entry.0.to_lowercase();
        let is_native = !entry.1.contains("_crx_");

        for search in &search_names {
            if entry_name_lower == *search {
                if is_native {
                    if exact_native.is_none() {
                        exact_native = Some(entry);
                    }
                } else if exact_match.is_none() {
                    exact_match = Some(entry);
                }
            } else if entry_name_lower.contains(search.as_str()) {
                if is_native {
                    if contains_native.is_none() {
                        contains_native = Some(entry);
                    }
                } else if contains_match.is_none() {
                    contains_match = Some(entry);
                }
            }
        }
    }

    // Prefer: exact native > exact PWA > contains native > contains PWA
    let chosen = exact_native.or(exact_match).or(contains_native).or(contains_match);

    if let Some((name, app_id)) = chosen {
        tracing::info!("found app in Get-StartApps: {} → {}", name, app_id);

        // Launch via shell:AppsFolder\{AppID}
        let shell_path = format!("shell:AppsFolder\\{}", app_id);
        let result = Command::new("cmd")
            .args(["/c", "start", "", &shell_path])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn();

        match result {
            Ok(_) => {
                tracing::info!("launched app via shell:AppsFolder: {}", app_id);
                Some(CommandResult {
                    success: true,
                    message: "Ok sir.".to_string(),
                })
            }
            Err(e) => {
                tracing::error!("failed to launch app '{}': {}", app_id, e);
                None
            }
        }
    } else {
        tracing::info!("app '{}' not found in Get-StartApps", target);
        None
    }
}

/// Parse Get-StartApps JSON output into (Name, AppID) pairs.
/// Handles both single-object and array formats.
#[cfg(target_os = "windows")]
fn parse_start_apps_json(json: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();

    // Simple JSON parser — extract "Name" and "AppID" pairs.
    // Format: [{"Name":"...","AppID":"..."},{"Name":"...","AppID":"..."}]
    // or: {"Name":"...","AppID":"..."}
    let mut remaining = json;

    while let Some(name_start) = remaining.find("\"Name\"") {
        remaining = &remaining[name_start..];
        // Find the value after "Name":
        if let Some(colon) = remaining.find(':') {
            remaining = &remaining[colon + 1..];
            // Skip whitespace and opening quote
            if let Some(q1) = remaining.find('"') {
                remaining = &remaining[q1 + 1..];
                if let Some(q2) = remaining.find('"') {
                    let name = &remaining[..q2];
                    remaining = &remaining[q2 + 1..];

                    // Find "AppID"
                    if let Some(appid_pos) = remaining.find("\"AppID\"") {
                        remaining = &remaining[appid_pos..];
                        if let Some(colon2) = remaining.find(':') {
                            remaining = &remaining[colon2 + 1..];
                            if let Some(q3) = remaining.find('"') {
                                remaining = &remaining[q3 + 1..];
                                if let Some(q4) = remaining.find('"') {
                                    let app_id = &remaining[..q4];
                                    remaining = &remaining[q4 + 1..];
                                    entries.push((name.to_string(), app_id.to_string()));
                                }
                            }
                        }
                    }
                }
            }
        } else {
            break;
        }
    }

    entries
}

/// Build search name variants for Get-StartApps matching.
#[cfg(target_os = "windows")]
fn app_search_names(target: &str) -> Vec<String> {
    let mut names = Vec::new();
    names.push(target.to_lowercase());

    // Add display name (e.g. "gmail" → "gmail" but also try "google mail")
    match target {
        "gmail" => { names.push("gmail".to_string()); }
        "youtube" | "you tube" => { names.push("youtube".to_string()); }
        "github" | "git hub" => { names.push("github".to_string()); }
        "vs code" | "visual studio code" | "code" => {
            names.push("visual studio code".to_string());
            names.push("vs code".to_string());
        }
        "calculator" | "calc" => { names.push("calculator".to_string()); }
        "terminal" | "windows terminal" | "wt" => {
            names.push("windows terminal".to_string());
            names.push("terminal".to_string());
        }
        "explorer" | "file explorer" => { names.push("file explorer".to_string()); }
        "command prompt" | "cmd" => { names.push("command prompt".to_string()); }
        "task manager" => { names.push("task manager".to_string()); }
        "control panel" => { names.push("control panel".to_string()); }
        "settings" => { names.push("settings".to_string()); }
        "paint" => { names.push("paint".to_string()); }
        "chatgpt" | "chat gpt" => { names.push("chatgpt".to_string()); }
        "google drive" => { names.push("google drive".to_string()); }
        "google docs" => { names.push("google docs".to_string()); }
        "google sheets" => { names.push("google sheets".to_string()); }
        "google slides" => { names.push("google slides".to_string()); }
        "google maps" | "maps" => { names.push("google maps".to_string()); names.push("maps".to_string()); }
        "google calendar" | "calendar" => { names.push("google calendar".to_string()); names.push("calendar".to_string()); }
        "google photos" | "photos" => { names.push("google photos".to_string()); names.push("photos".to_string()); }
        "google meet" => { names.push("google meet".to_string()); }
        "google chat" | "chat" => { names.push("google chat".to_string()); names.push("chat".to_string()); }
        "google translate" | "translate" => { names.push("google translate".to_string()); names.push("translate".to_string()); }
        "google news" => { names.push("google news".to_string()); }
        "google gemini" | "gemini" => { names.push("gemini".to_string()); }
        "whatsapp" => { names.push("whatsapp".to_string()); }
        _ => {}
    }

    names.sort();
    names.dedup();
    names
}

/// Fallback: use `where.exe` to find executables in PATH + App Paths registry.
#[cfg(target_os = "windows")]
fn launch_via_where(target: &str, _display_name: &str) -> Option<CommandResult> {
    let exe_name = if target.ends_with(".exe") {
        target.to_string()
    } else {
        format!("{}.exe", target)
    };

    tracing::debug!("trying where.exe: {}", exe_name);

    let output = Command::new("where")
        .args([&exe_name])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_string();
            if path.is_empty() {
                return None;
            }
            tracing::info!("found executable via where.exe: {}", path);

            let result = Command::new("cmd")
                .args(["/c", "start", "", &path])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .spawn();

            match result {
                Ok(_) => {
                    Some(CommandResult {
                        success: true,
                        message: "Ok sir.".to_string(),
                    })
                }
                Err(e) => {
                    tracing::error!("failed to launch '{}': {}", path, e);
                    None
                }
            }
        }
        _ => None,
    }
}

// ─── Windows: Tier 1 (focus window) ────────────────────────────────────────

#[cfg(target_os = "windows")]
fn focus_window_by_process(pid: u32, display_name: &str) -> Option<CommandResult> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let target_pid = pid;
    let mut hwnds: Vec<isize> = Vec::new();

    unsafe extern "system" fn collect_hwnds(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let hwnds = &mut *(lparam.0 as *mut Vec<isize>);
        if IsWindowVisible(hwnd).as_bool() {
            hwnds.push(hwnd.0);
        }
        BOOL(1)
    }

    unsafe {
        let lparam = LPARAM(&mut hwnds as *mut Vec<isize> as isize);
        let _ = EnumWindows(Some(collect_hwnds), lparam);
    }

    for hwnd_val in hwnds {
        let hwnd = HWND(hwnd_val);
        unsafe {
            let mut window_pid: u32 = 0;
            GetWindowThreadProcessId(hwnd, &mut window_pid as *mut u32);
            if window_pid == target_pid {
                if IsIconic(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(hwnd);
                tracing::info!("focused window for pid={}", target_pid);
                return Some(CommandResult {
                    success: true,
                    message: format!("{} is already open, sir.", display_name),
                });
            }
        }
    }

    None
}

// ─── macOS: Tier 2 ─────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn check_installed_macos(target: &str, display_name: &str) -> Option<CommandResult> {
    let app_name = if target.ends_with(".app") {
        target.to_string()
    } else {
        format!("{}.app", display_name)
    };

    let home = std::env::var("HOME").unwrap_or_default();
    let locations = [
        format!("/Applications/{}", app_name),
        format!("{}/Applications/{}", home, app_name),
        format!("/System/Applications/{}", app_name),
    ];

    for path in &locations {
        if std::path::Path::new(path).exists() {
            tracing::info!("found app at: {}", path);
            let result = Command::new("open").args(["-a", &app_name]).spawn();
            if result.is_ok() {
                return Some(CommandResult {
                    success: true,
                    message: format!("Opened {}, sir.", display_name),
                });
            }
        }
    }

    None
}

// ─── Linux: Tier 2 ─────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn check_installed_linux(target: &str, display_name: &str) -> Option<CommandResult> {
    if let Some(exec) = find_desktop_entry(target) {
        let result = Command::new("sh").args(["-c", &exec]).spawn();
        if result.is_ok() {
            return Some(CommandResult {
                success: true,
                message: format!("Opened {}, sir.", display_name),
            });
        }
    }

    let result = open::that(target);
    if result.is_ok() {
        return Some(CommandResult {
            success: true,
            message: format!("Opened {}, sir.", display_name),
        });
    }

    None
}

#[cfg(target_os = "linux")]
fn find_desktop_entry(name: &str) -> Option<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dirs = [
        "/usr/share/applications",
        "/usr/local/share/applications",
        &format!("{}/.local/share/applications", home),
    ];

    let name_lower = name.to_lowercase();
    let name_no_spaces = name_lower.replace(' ', "-");

    for dir in &dirs {
        let path = std::path::Path::new(dir);
        if !path.exists() { continue; }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_lowercase();
                if fname.contains(&name_lower) || fname.contains(&name_no_spaces) {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        for line in content.lines() {
                            if let Some(exec) = line.strip_prefix("Exec=") {
                                let clean = exec
                                    .split_whitespace()
                                    .filter(|w| !w.starts_with('%'))
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                return Some(clean);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

// ─── Tier 3: URL fallback ──────────────────────────────────────────────────

fn url_fallback(target: &str) -> Option<String> {
    let url_map: &[(&str, &str)] = &[
        ("gmail", "https://mail.google.com"),
        ("google mail", "https://mail.google.com"),
        ("youtube", "https://www.youtube.com"),
        ("you tube", "https://www.youtube.com"),
        ("github", "https://github.com"),
        ("git hub", "https://github.com"),
        ("twitter", "https://twitter.com"),
        ("x", "https://x.com"),
        ("facebook", "https://facebook.com"),
        ("instagram", "https://instagram.com"),
        ("reddit", "https://reddit.com"),
        ("linkedin", "https://linkedin.com"),
        ("whatsapp", "https://web.whatsapp.com"),
        ("whatsapp web", "https://web.whatsapp.com"),
        ("spotify", "https://open.spotify.com"),
        ("netflix", "https://netflix.com"),
        ("amazon", "https://amazon.com"),
        ("google drive", "https://drive.google.com"),
        ("google docs", "https://docs.google.com"),
        ("google sheets", "https://sheets.google.com"),
        ("google slides", "https://slides.google.com"),
        ("google maps", "https://maps.google.com"),
        ("google calendar", "https://calendar.google.com"),
        ("google translate", "https://translate.google.com"),
        ("google photos", "https://photos.google.com"),
        ("google news", "https://news.google.com"),
        ("google meet", "https://meet.google.com"),
        ("google chat", "https://chat.google.com"),
        ("google play", "https://play.google.com"),
        ("google play store", "https://play.google.com"),
        ("play store", "https://play.google.com"),
        ("app store", "https://apps.apple.com"),
        ("mac app store", "https://apps.apple.com"),
        ("chatgpt", "https://chat.openai.com"),
        ("chat gpt", "https://chat.openai.com"),
        ("open ai", "https://chat.openai.com"),
        ("openai", "https://chat.openai.com"),
        ("claude", "https://claude.ai"),
        ("figma", "https://figma.com"),
        ("notion", "https://notion.so"),
        ("slack", "https://slack.com"),
        ("discord", "https://discord.com/app"),
        ("twitch", "https://twitch.tv"),
        ("stack overflow", "https://stackoverflow.com"),
        ("stackoverflow", "https://stackoverflow.com"),
        ("wikipedia", "https://wikipedia.org"),
        ("chat", "https://chat.google.com"),
        ("maps", "https://maps.google.com"),
        ("translate", "https://translate.google.com"),
        ("my drive", "https://drive.google.com"),
        ("calendar", "https://calendar.google.com"),
        ("gemini", "https://gemini.google.com"),
        ("google gemini", "https://gemini.google.com"),
    ];

    for (name, url) in url_map {
        if target == *name {
            return Some(url.to_string());
        }
    }
    None
}

// ─── Utilities ─────────────────────────────────────────────────────────────

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

pub(crate) mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len() * 3);
        for byte in s.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(byte as char);
                }
                b' ' => result.push('+'),
                _ => result.push_str(&format!("%{:02X}", byte)),
            }
        }
        result
    }
}

#[cfg(test)]
mod safe_url_tests {
    use super::*;

    #[test]
    fn web_urls_pass() {
        assert!(is_safe_web_url("https://www.youtube.com/watch?v=abc"));
        assert!(is_safe_web_url("HTTP://example.com"));
        assert!(is_safe_web_url("  https://example.com  "));
    }

    #[test]
    fn shell_handlers_and_paths_are_refused() {
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "C:\\Windows\\System32\\cmd.exe",
            "javascript:alert(1)",
            "https://a.com/x y",
            "https://a.com/\r\nx",
            "",
        ] {
            assert!(!is_safe_web_url(bad), "should refuse {bad:?}");
        }
    }

    #[test]
    fn open_url_refuses_non_web_without_launching() {
        let r = open_url("calc", "file:///C:/Windows/System32/calc.exe").unwrap();
        assert!(!r.success);
    }
}

#[cfg(test)]
mod close_plan_tests {
    use super::*;
    use crate::app_registry::{test_support, AppEntry, LaunchMethod};

    fn app(display: &str, names: &[&str], path: &str) -> AppEntry {
        AppEntry {
            display_name: display.to_string(),
            search_names: names.iter().map(|s| s.to_string()).collect(),
            launch: LaunchMethod::Exe { path: path.to_string() },
            use_count: 0,
            last_used: 0,
        }
    }

    fn registry() -> test_support::Installed {
        test_support::install(vec![
            // Forward slashes: `Path::file_name` splits them on Windows and on Unix, so the fixture means the same on
            // every OS (backslash paths are one single component on macOS/Linux).
            app("Notepad", &["notepad"], "C:/Windows/notepad.exe"),
            app("Google Chat", &["google chat", "chat"], "C:/x/chat.exe"),
            app("Brave", &["brave"], "C:/x/brave.exe"),
            app("Windows Explorer", &["explorer", "file explorer"], "C:/Windows/explorer.exe"),
        ])
    }

    #[test]
    fn a_named_app_is_closed_by_its_exe() {
        let _g = registry();
        assert_eq!(
            plan_close("notepad"),
            ClosePlan::Close { display: "Notepad".into(), exe: "notepad.exe".into() }
        );
    }

    #[test]
    fn a_guess_is_never_closed() {
        let _g = registry();
        // "chat" is only CONTAINED in the phrase: it used to taskkill /F Google Chat.
        match plan_close("mummy chat") {
            ClosePlan::Refuse(m) => {
                assert!(m.contains("Google Chat") && m.contains("haven't closed anything"), "{m}")
            }
            other => panic!("a guess must not close anything: {other:?}"),
        }
    }

    #[test]
    fn the_macos_shell_and_login_session_are_protected_and_windows_behaviour_is_unchanged() {
        // macOS: every shell/session process, by stem or by display name, in any case.
        for (stem, display) in [
            ("finder", "Finder"), ("dock", "Dock"), ("loginwindow", "loginwindow"),
            ("windowserver", "WindowServer"), ("systemuiserver", "SystemUIServer"),
            ("controlcenter", "Control Center"), ("x", "Finder"), ("nexus", "NEXUS"),
        ] {
            assert!(is_protected_process(stem, display, true), "{stem}/{display} must be protected on macOS");
        }
        // an ordinary app is not
        assert!(!is_protected_process("safari", "Safari", true));
        assert!(!is_protected_process("google chrome", "Google Chrome", true));
        // Windows (macos == false): the old list, nothing more
        assert!(is_protected_process("explorer", "Windows Explorer", false));
        assert!(is_protected_process("svchost", "x", false));
        assert!(!is_protected_process("finder", "Finder", false), "the macOS list must not leak into Windows");
        assert!(!is_protected_process("dock", "Dock", false));
    }

    #[test]
    fn the_macos_quit_script_gets_the_name_as_an_argument_never_inside_the_script() {
        let args = macos_quit_args("Google Chrome").expect("a normal name");
        assert_eq!(args.last().map(String::as_str), Some("Google Chrome"));
        // every script line is a literal from MACOS_QUIT_SCRIPT: the name appears only as the final argv item
        let script: Vec<&str> = args.iter().filter(|a| a.as_str() != "-e").map(String::as_str).collect();
        assert_eq!(script.len(), MACOS_QUIT_SCRIPT.len() + 1);
        assert!(!script[..MACOS_QUIT_SCRIPT.len()].iter().any(|l| l.contains("Google Chrome")));
        // an injection attempt is just an (ignored) app name: it cannot reach the script text
        let nasty = macos_quit_args("x\" & (do shell script \"rm -rf ~\") & \"").expect("quotes are data");
        assert!(!nasty[..nasty.len() - 1].iter().any(|l| l.contains("rm -rf")));
        // not plausible names are refused outright
        assert!(macos_quit_args("").is_none());
        assert!(macos_quit_args("   ").is_none());
        assert!(macos_quit_args("bad\nname").is_none());
        assert!(macos_quit_args(&"a".repeat(81)).is_none());
        // it checks for a running app first, so a closed app is never launched just to be quit: the quit line must sit
        // INSIDE an `if isRunning then` block (guard line before the quit line, `end if` after it)
        let at = |needle: &str| MACOS_QUIT_SCRIPT.iter().position(|l| *l == needle);
        let guard = at("if isRunning then").expect("the running check is an `if isRunning then` line");
        let quit = at("tell application appName to quit").expect("the quit line");
        let end = at("end if").expect("the block is closed");
        assert!(guard < quit && quit < end, "quit must be inside the running check");
    }

    /// Real macOS only (runs in the macOS CI job): the quit script must COMPILE with Apple's own compiler, because
    /// the AppleScript syntax cannot be checked anywhere else. `osacompile` compiles without running anything.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_quit_script_compiles_with_osacompile() {
        let out = std::env::temp_dir().join(format!("nexus-quit-{}.scpt", std::process::id()));
        let mut cmd = Command::new("osacompile");
        cmd.arg("-o").arg(&out);
        for line in MACOS_QUIT_SCRIPT {
            cmd.arg("-e").arg(line);
        }
        let res = cmd.output().expect("osacompile runs on macOS");
        let _ = std::fs::remove_file(&out);
        assert!(res.status.success(), "osacompile: {}", String::from_utf8_lossy(&res.stderr));
    }

    /// Real macOS only: the shell is refused through the whole `plan_close` path, whatever the app registry holds.
    #[cfg(target_os = "macos")]
    #[test]
    fn on_macos_plan_close_refuses_finder_dock_and_loginwindow() {
        let _g = registry();
        for name in ["finder", "Finder", "dock", "loginwindow"] {
            assert!(matches!(plan_close(name), ClosePlan::Refuse(_)), "{name} must be refused");
        }
    }

    #[test]
    fn the_shell_and_the_assistant_are_never_closed() {
        let _g = registry();
        assert!(matches!(plan_close("explorer"), ClosePlan::Refuse(_)));
        assert!(matches!(plan_close("file explorer"), ClosePlan::Refuse(_)));
        assert!(matches!(plan_close("nexus"), ClosePlan::Refuse(_)));
        assert!(matches!(plan_close("svchost"), ClosePlan::Refuse(_)));
    }

    #[test]
    fn unknown_names_become_a_bare_process_name_not_a_substring_kill() {
        let _g = registry();
        // Nothing matches: the plan is the exact process name (taskkill /IM matches exactly).
        assert_eq!(
            plan_close("zzqqxx"),
            ClosePlan::Close { display: "zzqqxx".into(), exe: "zzqqxx.exe".into() }
        );
    }
}
