//! Resume points (plan P3, tier T5): where the user was working, so NEXUS can
//! say "you left off in …" after a reboot.
//!
//! A recorder samples the foreground window every 30 s while the user is at
//! the PC and stores one record per *change* of window (same window = just
//! refresh its `last_seen`). Sampling beats a shutdown hook on purpose:
//! Windows gives no reliable exit event on logoff/power loss, so at most 30 s
//! are lost.
//!
//! Privacy: local only (sealed with the rest of the Memory Core), 7-day ring,
//! URLs reduced to scheme+host+path, NEXUS's own windows, lock screen and
//! sensitive windows (banks, password managers, wallets) are never recorded,
//! and the whole recorder is one setting (`memcoreActivity`).

use serde::{Deserialize, Serialize};

use super::store::{Observation, Store, Tier, Trust};

/// Sampling cadence.
pub const SAMPLE_SECS: u64 = 30;
/// The user counts as away after this long without keyboard/mouse input.
pub const AWAY_AFTER_SECS: u64 = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// Process executable, lower-case ("brave.exe").
    pub app: String,
    pub title: String,
    /// Privacy-trimmed page address for browsers.
    #[serde(default)]
    pub url: Option<String>,
    /// Timetable activity running when this was recorded ("dsa"). Lets
    /// "start DSA" reopen what was open DURING DSA, not just the latest page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Browser,
    App,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Inserted,
    Touched,
    Skipped,
}

/// Processes that are never "where the user was": NEXUS itself and shell chrome.
const IGNORED_APPS: &[&str] = &[
    "nexus.exe",
    "lockapp.exe",
    "searchhost.exe",
    "searchapp.exe",
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "textinputhost.exe",
    "logonui.exe",
    "consent.exe",
];

pub fn friendly_app(process: &str) -> String {
    match process.to_lowercase().as_str() {
        "code.exe" => "VS Code".into(),
        "brave.exe" => "Brave".into(),
        "chrome.exe" => "Chrome".into(),
        "msedge.exe" => "Edge".into(),
        "firefox.exe" => "Firefox".into(),
        "explorer.exe" => "File Explorer".into(),
        "winword.exe" => "Word".into(),
        "excel.exe" => "Excel".into(),
        "powerpnt.exe" => "PowerPoint".into(),
        "whatsapp.exe" | "whatsapp.root.exe" => "WhatsApp".into(),
        "spotify.exe" => "Spotify".into(),
        "windowsterminal.exe" => "Terminal".into(),
        // Proper-cased names the "capitalise the stem" fallback would get wrong.
        "opencode.exe" => "OpenCode".into(),
        "claude.exe" => "Claude".into(),
        "notepad.exe" => "Notepad".into(),
        "devenv.exe" => "Visual Studio".into(),
        "cursor.exe" => "Cursor".into(),
        "slack.exe" => "Slack".into(),
        "discord.exe" => "Discord".into(),
        "teams.exe" | "ms-teams.exe" => "Teams".into(),
        "zoom.exe" => "Zoom".into(),
        "vlc.exe" => "VLC".into(),
        "postman.exe" => "Postman".into(),
        other => {
            let stem = other.strip_suffix(".exe").unwrap_or(other);
            let mut c = stem.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

/// Strip the browser/app suffix window titles carry ("… - Brave").
pub fn clean_title(app: &str, title: &str) -> String {
    let t = title.trim();
    let friendly = friendly_app(app).to_lowercase();
    if let Some((head, tail)) = t.rsplit_once(" - ") {
        let tail_l = tail.trim().to_lowercase();
        if tail_l == friendly
            || tail_l.contains("google chrome")
            || tail_l.contains("microsoft edge")
            || tail_l.contains("brave")
            || tail_l.contains("visual studio code")
            || tail_l.contains("mozilla firefox")
        {
            return head.trim().to_string();
        }
    }
    t.to_string()
}

pub fn kind(s: &Sample) -> Kind {
    if s.url.as_deref().map(|u| u.contains("youtube.com/watch")).unwrap_or(false) {
        Kind::Video
    } else if crate::browser_url::is_browser_process(&s.app) {
        Kind::Browser
    } else {
        Kind::App
    }
}

/// Whether this window is worth remembering. Pure.
pub fn should_record(s: &Sample) -> bool {
    let app = s.app.to_lowercase();
    if app.is_empty() || IGNORED_APPS.contains(&app.as_str()) {
        return false;
    }
    if s.title.trim().is_empty() {
        return false;
    }
    if crate::screen_context::is_sensitive(&app, &s.title, s.url.as_deref()) {
        return false;
    }
    true
}

pub fn encode(s: &Sample) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

pub fn decode(v: &str) -> Option<Sample> {
    serde_json::from_str(v).ok()
}

/// Store one sample: a changed window becomes a new record, the same window
/// only refreshes `last_seen` (so `last_seen` ≈ when the user last used it).
pub fn record_sample(store: &Store, sample: &Sample, now: i64) -> Recorded {
    if !should_record(sample) {
        return Recorded::Skipped;
    }
    let clean = Sample {
        app: sample.app.to_lowercase(),
        title: clean_title(&sample.app, &sample.title).chars().take(160).collect(),
        url: sample.url.clone(),
        activity: sample.activity.clone(),
    };
    if let Some(latest) = store.recent(Tier::Resume, 1).into_iter().next() {
        if decode(&latest.value).as_ref() == Some(&clean) {
            store.touch(latest.id, now);
            return Recorded::Touched;
        }
    }
    let ok = store.observe(
        &Observation {
            tier: Tier::Resume,
            key: format!("r:{now}"),
            value: encode(&clean),
            source: "activity:foreground".into(),
            trust: Trust::Derived,
            pinned: false,
        },
        now,
    );
    if ok.is_ok() {
        Recorded::Inserted
    } else {
        Recorded::Skipped
    }
}

// ─── Windows glue ────────────────────────────────────────────────────

/// Seconds since the last keyboard/mouse input (None if it cannot be read).
#[cfg(target_os = "windows")]
pub fn user_idle_secs() -> Option<u64> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: `info` is a valid, correctly sized LASTINPUTINFO.
    let ok = unsafe { GetLastInputInfo(&mut info) };
    if !ok.as_bool() {
        return None;
    }
    let now = unsafe { GetTickCount() };
    Some(now.wrapping_sub(info.dwTime) as u64 / 1000)
}

/// Seconds since the last keyboard/mouse input (None if it cannot be read).
#[cfg(target_os = "macos")]
pub fn user_idle_secs() -> Option<u64> {
    mac_idle_seconds().map(|s| s as u64)
}

/// macOS: seconds since the last hardware input event. The HID system state counts the person's keyboard, mouse and
/// trackpad only, not events NEXUS itself posts (Ghost clicks), so its own actions never look like the user being back.
/// No permission is needed.
#[cfg(target_os = "macos")]
pub(crate) fn mac_idle_seconds() -> Option<f64> {
    const HID_SYSTEM_STATE: i32 = 1; // kCGEventSourceStateHIDSystemState
    const ANY_INPUT_EVENT: u32 = u32::MAX; // kCGAnyInputEventType
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }
    // SAFETY: plain query with constant arguments.
    let s = unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) };
    (s.is_finite() && s >= 0.0).then_some(s)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn user_idle_secs() -> Option<u64> {
    None
}

/// Probe the foreground window now. Blocking (UI Automation for browsers).
#[cfg(target_os = "windows")]
pub fn probe() -> Option<Sample> {
    let app = crate::browser_url::foreground_process_name()?;
    let title = crate::browser_url::foreground_window_title().unwrap_or_default();
    let url = if crate::browser_url::is_browser_process(&app) {
        crate::browser_url::get_active_browser_url().and_then(|u| crate::screen_context::sanitize_url(&u))
    } else {
        None
    };
    Some(Sample { app, title, url, activity: None })
}

#[cfg(not(target_os = "windows"))]
pub fn probe() -> Option<Sample> {
    None
}

/// Start the 30 s recorder. Cheap no-op while `memcoreActivity` is off or the
/// user is away from the PC.
pub fn start_recorder<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(SAMPLE_SECS)).await;
            if crate::focus_mode::paused() {
                crate::focus_mode::defer_log("resume_recorder");
                continue;
            }
            let Ok(dir) = app.path().app_data_dir() else { continue };
            if !super::flag(&dir, "memcoreActivity", true) {
                continue;
            }
            // Away from the PC: do not let an idle window keep looking "in use".
            if user_idle_secs().map(|s| s >= AWAY_AFTER_SECS).unwrap_or(false) {
                continue;
            }
            let probed = tokio::time::timeout(
                std::time::Duration::from_secs(4),
                tauri::async_runtime::spawn_blocking(probe),
            )
            .await;
            let Ok(Ok(Some(mut sample))) = probed else { continue };
            let now = chrono::Utc::now().timestamp();
            super::with_store(&dir, |s| {
                // Tag the sample with the timetable activity running right now.
                sample.activity = super::timetable::current_activity_now(s);
                record_sample(s, &sample, now)
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Quartz idle query needs no permission and answers in any session (a CI runner included).
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_idle_time_is_read() {
        let s = mac_idle_seconds().expect("idle seconds");
        assert!(s.is_finite() && s >= 0.0);
        assert!(user_idle_secs().is_some());
        eprintln!("idle for {s:.1} s");
    }

    fn s(app: &str, title: &str, url: Option<&str>) -> Sample {
        Sample { app: app.into(), title: title.into(), url: url.map(String::from), activity: None }
    }

    #[test]
    fn titles_lose_their_app_suffix() {
        assert_eq!(clean_title("brave.exe", "Top 10 healthiest nuts - Brave"), "Top 10 healthiest nuts");
        assert_eq!(clean_title("chrome.exe", "Docs - Google Chrome"), "Docs");
        assert_eq!(clean_title("code.exe", "main.rs - ULTRON - Visual Studio Code"), "main.rs - ULTRON");
        assert_eq!(clean_title("notepad.exe", "todo.txt - Notepad"), "todo.txt");
        assert_eq!(clean_title("notepad.exe", "plan - draft"), "plan - draft", "unrelated suffix stays");
    }

    #[test]
    fn friendly_names() {
        assert_eq!(friendly_app("code.exe"), "VS Code");
        assert_eq!(friendly_app("brave.exe"), "Brave");
        assert_eq!(friendly_app("obsidian.exe"), "Obsidian");
    }

    #[test]
    fn what_is_worth_recording() {
        assert!(should_record(&s("code.exe", "main.rs - Visual Studio Code", None)));
        assert!(!should_record(&s("nexus.exe", "NEXUS", None)), "NEXUS itself");
        assert!(!should_record(&s("lockapp.exe", "Windows Default Lock Screen", None)));
        assert!(!should_record(&s("brave.exe", "   ", None)), "empty title");
        assert!(!should_record(&s("", "x", None)));
    }

    #[test]
    fn sensitive_windows_are_never_recorded() {
        assert!(!should_record(&s("1password.exe", "1Password", None)));
        assert!(!should_record(&s("brave.exe", "Sign in", Some("https://paypal.com/login"))));
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(&s("brave.exe", "x", Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ"))), Kind::Video);
        assert_eq!(kind(&s("brave.exe", "x", Some("https://leetcode.com/problems/two-sum"))), Kind::Browser);
        assert_eq!(kind(&s("code.exe", "x", None)), Kind::App);
    }

    #[test]
    fn same_window_touches_a_changed_window_inserts() {
        let st = Store::open_in_memory().unwrap();
        let a = s("brave.exe", "Two Sum - LeetCode - Brave", Some("https://leetcode.com/problems/two-sum"));
        assert_eq!(record_sample(&st, &a, 100), Recorded::Inserted);
        assert_eq!(record_sample(&st, &a, 130), Recorded::Touched);
        assert_eq!(st.count(Tier::Resume), 1);
        assert_eq!(st.recent(Tier::Resume, 1)[0].last_seen, 130, "touch must refresh last_seen");
        let b = s("code.exe", "main.rs - Visual Studio Code", None);
        assert_eq!(record_sample(&st, &b, 160), Recorded::Inserted);
        assert_eq!(st.count(Tier::Resume), 2);
        let newest = decode(&st.recent(Tier::Resume, 1)[0].value).unwrap();
        assert_eq!((newest.app.as_str(), newest.title.as_str()), ("code.exe", "main.rs"));
    }

    #[test]
    fn skipped_samples_write_nothing() {
        let st = Store::open_in_memory().unwrap();
        assert_eq!(record_sample(&st, &s("nexus.exe", "NEXUS", None), 1), Recorded::Skipped);
        assert_eq!(st.count(Tier::Resume), 0);
    }

    #[test]
    fn resume_ring_keeps_seven_days_and_is_not_decayed_like_facts() {
        use super::super::store::RESUME_RETENTION_SECS;
        let st = Store::open_in_memory().unwrap();
        record_sample(&st, &s("code.exe", "old.rs - Visual Studio Code", None), 0);
        record_sample(&st, &s("code.exe", "new.rs - Visual Studio Code", None), RESUME_RETENTION_SECS - 10);
        st.prune(RESUME_RETENTION_SECS + 5);
        assert_eq!(st.count(Tier::Resume), 1);
        let kept = decode(&st.recent(Tier::Resume, 1)[0].value).unwrap();
        assert_eq!(kept.title, "new.rs");
    }

    #[test]
    fn clear_tier_removes_only_that_tier() {
        let st = Store::open_in_memory().unwrap();
        record_sample(&st, &s("code.exe", "a.rs - Visual Studio Code", None), 10);
        st.observe(
            &Observation {
                tier: Tier::Fact,
                key: "city".into(),
                value: "Pune".into(),
                source: "t".into(),
                trust: Trust::UserSaid,
                pinned: true,
            },
            11,
        )
        .unwrap();
        assert_eq!(st.clear_tier(Tier::Resume, "user", 12), 1);
        assert_eq!((st.count(Tier::Resume), st.count(Tier::Fact)), (0, 1));
    }

    /// Live probe (run by hand): cargo test --lib live_probe_foreground -- --ignored --nocapture
    #[test]
    #[ignore = "reads the real foreground window"]
    fn live_probe_foreground() {
        let idle = user_idle_secs();
        let sample = probe();
        println!("idle_secs={idle:?}");
        println!("sample={sample:?}");
        assert!(idle.is_some(), "GetLastInputInfo failed");
        let s = sample.expect("no foreground window");
        println!("would_record={} kind={:?}", should_record(&s), kind(&s));
    }
}
