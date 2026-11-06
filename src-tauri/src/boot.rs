//! Boot-path helpers: who is the primary instance, when the WebView2 profile must be reset,
//! and a persisted boot timeline.
//!
//! Why this exists (docs/features/123, plus the 2026-10-18 start-menu analysis):
//!
//! * Launching NEXUS while it is already running (the Start menu shortcut, after the logon
//!   task has started it) used to run the WebView2 profile cleanup in the *second* process,
//!   before the single-instance check. That cleanup kills every `msedgewebview2.exe` that
//!   uses NEXUS's profile and deletes the profile, so it took the running orb down and the
//!   stage watchdog needed >10 s to rebuild it. A secondary launch must not touch anything.
//! * The cleanup also ran on every start, forcing WebView2 to build a brand-new profile
//!   (cold start) each time. It is only needed when the build type changed (dev ↔ release),
//!   because a stale dev URL survives in the profile's Preferences.
//! * Autostart boots wrote no log at all, so nothing could be measured. `mark()` appends to
//!   `boot_timeline.jsonl` with time measured from the real process start.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Once;

static FIRST_AUDIO: Once = Once::new();

/// Mark the first moment any sound starts playing (the hello at logon), once per run.
pub fn first_audio() {
    FIRST_AUDIO.call_once(|| mark("first_audio"));
}

const TIMELINE_FILE: &str = "boot_timeline.jsonl";
const TIMELINE_MAX_LINES: usize = 300;

// ---- Primary / secondary instance ----

/// Claim the right to be the running instance. `true` = we are first (or the platform has no
/// cheap check); `false` = another NEXUS process already holds it, so this launch is only a
/// "bring the existing one forward" request and must not touch shared state.
///
/// Separate from the single-instance plugin's own mutex on purpose: the plugin only checks
/// inside `Builder::run`, which is after the work this guard has to precede.
pub fn claim_primary_instance() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;

        let name: Vec<u16> = "Local\\com.nexus.assistant-boot\0".encode_utf16().collect();
        // The handle is intentionally never closed: the mutex must live as long as the process.
        let _handle = unsafe { CreateMutexW(std::ptr::null(), true, PCWSTR(name.as_ptr())) };
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        !already
    }
    #[cfg(not(target_os = "windows"))]
    {
        true
    }
}

// ---- WebView2 profile reset decision ----

/// "release" for builds that embed the frontend, "dev" for builds that load the Vite server.
pub fn webview_mode_now() -> &'static str {
    if cfg!(feature = "custom-protocol") {
        "release"
    } else {
        "dev"
    }
}

/// The profile must be wiped only when the build type changed (or was never recorded):
/// a stale dev URL in the profile's Preferences makes a release build show
/// "localhost refused to connect". With an unchanged type the existing profile is reused,
/// so WebView2 starts warm instead of building a new profile.
pub fn needs_profile_reset(recorded_mode: Option<&str>, current_mode: &str) -> bool {
    recorded_mode.map(str::trim) != Some(current_mode)
}

/// True when the executable is the one inside an installed `.app` bundle (`NEXUS.app/Contents/MacOS/nexus`).
/// A macOS login item must only point there: a dev build in `src-tauri/target/` registered as a LaunchAgent starts a
/// stale binary at every login and asks for the Keychain password each time it is rebuilt.
pub fn runs_from_app_bundle(exe: &str) -> bool {
    exe.contains(".app/Contents/MacOS/")
}

// ---- Persisted boot timeline ----

fn data_dir() -> Option<PathBuf> {
    crate::worker_auth::app_data_dir_envless()
}

/// Milliseconds since the process was created (kernel creation time, so it includes the
/// loader and runtime start-up that happen before `main`). `None` if unavailable.
fn since_process_start_ms() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let ok = unsafe {
            GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user)
        };
        if !ok.as_bool() {
            return None;
        }
        let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        // FILETIME counts 100 ns from 1601-01-01; convert to Unix milliseconds.
        let created_unix_ms = ticks.checked_sub(116_444_736_000_000_000)? / 10_000;
        Some(unix_ms_now().saturating_sub(created_unix_ms))
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// One JSON line for the timeline. Pure.
pub fn format_mark(unix_ms: u64, pid: u32, name: &str, since_start_ms: Option<u64>, note: &str) -> String {
    serde_json::json!({
        "at": unix_ms,
        "pid": pid,
        "mark": name,
        "t_ms": since_start_ms, // null when the OS cannot say
        "note": note,
    })
    .to_string()
}

/// Keep only the last `max` lines. Pure.
pub fn trim_lines(content: &str, max: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= max {
        return content.to_string();
    }
    let mut out = lines[lines.len() - max..].join("\n");
    out.push('\n');
    out
}

/// Record a boot milestone. Cheap (one small append) and never fails the caller.
/// `t_ms` is measured from the real process start, so pre-`main` time counts.
pub fn mark(name: &str) {
    mark_with(name, "");
}

pub fn mark_with(name: &str, note: &str) {
    static TRIM_ONCE: Once = Once::new();
    let Some(dir) = data_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(TIMELINE_FILE);
    TRIM_ONCE.call_once(|| {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if content.lines().count() > TIMELINE_MAX_LINES {
                let _ = std::fs::write(&path, trim_lines(&content, TIMELINE_MAX_LINES / 2));
            }
        }
    });
    let line = format_mark(unix_ms_now(), std::process::id(), name, since_process_start_ms(), note);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_is_reused_when_the_build_type_is_unchanged() {
        assert!(!needs_profile_reset(Some("release"), "release"));
        assert!(!needs_profile_reset(Some("release\n"), "release"));
        assert!(!needs_profile_reset(Some("dev"), "dev"));
    }

    #[test]
    fn profile_is_reset_when_build_type_changes_or_is_unknown() {
        assert!(needs_profile_reset(Some("dev"), "release"));
        assert!(needs_profile_reset(Some("release"), "dev"));
        assert!(needs_profile_reset(None, "release"));
        assert!(needs_profile_reset(Some(""), "release"));
    }

    #[test]
    fn only_an_installed_app_bundle_counts_as_installed() {
        assert!(runs_from_app_bundle("/Applications/NEXUS.app/Contents/MacOS/nexus"));
        assert!(runs_from_app_bundle("/Users/me/Applications/NEXUS.app/Contents/MacOS/nexus"));
        assert!(!runs_from_app_bundle("/Users/me/NEXUS/macos/src-tauri/target/debug/nexus"));
        assert!(!runs_from_app_bundle("/Users/me/NEXUS/macos/src-tauri/target/release/nexus"));
        assert!(!runs_from_app_bundle(""));
    }

    #[test]
    fn mark_line_is_valid_json_with_all_fields() {
        let line = format_mark(1_800_000_000_000, 4242, "stage_alive", Some(2900), "primary");
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["mark"], "stage_alive");
        assert_eq!(v["pid"], 4242);
        assert_eq!(v["t_ms"], 2900);
        assert_eq!(v["note"], "primary");
        let unknown = format_mark(1, 1, "main", None, "");
        let v: serde_json::Value = serde_json::from_str(&unknown).unwrap();
        assert!(v["t_ms"].is_null());
    }

    #[test]
    fn trim_keeps_only_the_newest_lines() {
        let content = (1..=10).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n") + "\n";
        assert_eq!(trim_lines(&content, 3), "l8\nl9\nl10\n");
        assert_eq!(trim_lines(&content, 50), content);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn process_start_offset_is_available_and_sane() {
        let ms = since_process_start_ms().expect("GetProcessTimes should work");
        // The test process started moments ago, certainly less than an hour.
        assert!(ms < 3_600_000, "{ms}");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn second_claim_in_the_same_process_sees_the_mutex() {
        // The first claim in this process wins; a repeat claim must observe that the name is
        // taken. (Within one process CreateMutexW on an existing name also returns
        // ERROR_ALREADY_EXISTS, which is exactly the signal a second process would get.)
        let _first = claim_primary_instance(); // true unless a real NEXUS is running right now
        assert!(!claim_primary_instance());
    }
}
