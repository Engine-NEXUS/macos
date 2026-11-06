//! Window focus management — bring an app to the foreground.
//!
//! On Windows, `SetForegroundWindow` silently fails when called from a
//! background process (Windows' foreground lock). The workaround is the
//! `AttachThreadInput` trick (from ghost-hands window.rs):
//!
//!   1. Get the foreground window's thread ID
//!   2. Attach our input queue to that thread
//!   3. Call SetForegroundWindow (now succeeds because we share input state)
//!   4. Detach the input queue
//!
//! This is the same pattern used by ghost-hands, nuphus-mcp, and many
//! other Windows automation tools.

/// Pure UIPI gate: Some(reason) when a click must not be attempted —
/// target elevated, sender not. Returned verbatim as the spoken error
/// (reroute, never a blind retry into the void). Shared by all platforms.
pub fn elevated_block_reason(target_elevated: bool, self_elevated: bool) -> Option<String> {
    if target_elevated && !self_elevated {
        Some(
            "that's an admin window, sir — Windows blocks my clicks there. \
             Click it yourself, or restart NEXUS as admin."
                .to_string(),
        )
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
pub use windows_impl::*;

#[cfg(not(target_os = "windows"))]
pub use unix_impl::*;

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::sync::Mutex;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowTextW,
        GetWindowThreadProcessId, IsIconic, SetForegroundWindow, ShowWindow, SW_MAXIMIZE,
        SW_MINIMIZE, SW_RESTORE,
    };

    // Thread-local storage for the search target and result.
    // EnumWindows requires a C callback, so we can't capture closures.
    static SEARCH_TARGET: Mutex<Option<String>> = Mutex::new(None);
    static FOUND_HWND: Mutex<Option<HWND>> = Mutex::new(None);

    unsafe extern "system" fn enum_proc(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        let mut title = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title);
        if len <= 0 {
            return BOOL(1); // continue enumeration
        }
        let title_str = String::from_utf16_lossy(&title[..len as usize]);
        let lower = title_str.to_lowercase();

        let target = SEARCH_TARGET.lock().unwrap();
        if let Some(search) = target.as_ref() {
            if lower.contains(&search.to_lowercase()) {
                drop(target); // release lock before acquiring FOUND_HWND
                *FOUND_HWND.lock().unwrap() = Some(hwnd);
                return BOOL(0); // stop enumeration
            }
        }
        BOOL(1) // continue
    }

    /// Find a window whose title contains `partial_title` (case-insensitive).
    pub fn find_window(partial_title: &str) -> Option<HWND> {
        *SEARCH_TARGET.lock().unwrap() = Some(partial_title.to_string());
        *FOUND_HWND.lock().unwrap() = None;

        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(0));
        }

        FOUND_HWND.lock().unwrap().take()
    }

    /// Bring a window to the foreground using the AttachThreadInput trick and Alt-key tap.
    /// Returns true if the window is now in the foreground.
    pub fn focus_window(hwnd: HWND) -> bool {
        unsafe {
            // Restore if minimized
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }

            let _ = BringWindowToTop(hwnd);

            // Synthesize an Alt key tap to unlock Windows OS foreground lock
            // (classic Win32 OS input state unlock technique)
            let _ = super::super::keyboard::press_key("alt");

            // AttachThreadInput trick: attach our input queue to the
            // foreground thread's so SetForegroundWindow succeeds.
            let fg = GetForegroundWindow();
            let mut fg_pid = 0u32;
            let fg_thread = GetWindowThreadProcessId(fg, &mut fg_pid as *mut u32);
            let this_thread = GetCurrentThreadId();

            let attached = fg_thread != 0
                && fg_thread != this_thread
                && AttachThreadInput(this_thread, fg_thread, true).as_bool();

            let _ = SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);

            if attached {
                let _ = AttachThreadInput(this_thread, fg_thread, false);
            }

            // Verify success
            GetForegroundWindow() == hwnd
        }
    }

    /// Find a window by partial title and bring it to the foreground.
    /// Returns true if the window was found and focused.
    pub fn focus_app_by_title(partial_title: &str) -> bool {
        if let Some(hwnd) = find_window(partial_title) {
            focus_window(hwnd)
        } else {
            false
        }
    }

    /// Poll for an app window by partial title up to `timeout_ms`,
    /// bringing it to the foreground once found (essential for slow-starting apps like WhatsApp).
    pub fn wait_and_focus_app(partial_title: &str, timeout_ms: u64) -> bool {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);
        let interval = std::time::Duration::from_millis(150);

        while start.elapsed() < timeout {
            if focus_app_by_title(partial_title) {
                return true;
            }
            std::thread::sleep(interval);
        }
        false
    }

    /// Get the title of the current foreground window.
    pub fn get_foreground_window_title() -> Option<String> {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.0 == 0 {
                return None;
            }
            let mut title = [0u16; 512];
            let len = GetWindowTextW(fg, &mut title);
            if len <= 0 {
                return None;
            }
            Some(String::from_utf16_lossy(&title[..len as usize]))
        }
    }

    /// Check if the current foreground window belongs to the given app name (case-insensitive).
    pub fn is_foreground_app(partial: &str) -> bool {
        let want = partial.to_lowercase();
        if let Some(title) = get_foreground_window_title() {
            if title.to_lowercase().contains(&want) {
                return true;
            }
        }
        false
    }

    /// Minimize the current foreground window. `ShowWindow`'s return value
    /// reports the window's PRIOR visibility, not call success, so it's
    /// discarded (same convention as the `SW_RESTORE` call above) — this
    /// returns whether a foreground window existed to act on at all.
    pub fn minimize_foreground_window() -> bool {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0 == 0 {
                return false;
            }
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
            true
        }
    }

    /// Maximize the current foreground window. Same fire-and-forget
    /// convention as `minimize_foreground_window`.
    pub fn maximize_foreground_window() -> bool {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.0 == 0 {
                return false;
            }
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            true
        }
    }

    /// True if OUR process runs elevated (admin). Clicks from a
    /// non-elevated sender into an elevated window are silently eaten by
    /// UIPI — detect first, reroute with speech instead.
    pub fn our_process_elevated() -> bool {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        unsafe {
            let mut token: HANDLE = HANDLE(0);
            if !OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).as_bool() {
                return false;
            }
            let mut elev: TOKEN_ELEVATION = std::mem::zeroed();
            let mut ret_len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                &mut elev as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_len,
            )
            .as_bool();
            ok && elev.TokenIsElevated != 0
        }
    }

    fn process_token_elevated(proc: windows::Win32::Foundation::HANDLE) -> bool {
        use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
        use windows::Win32::System::Threading::OpenProcessToken;
        unsafe {
            let mut token: windows::Win32::Foundation::HANDLE =
                windows::Win32::Foundation::HANDLE(0);
            if !OpenProcessToken(proc, TOKEN_QUERY, &mut token).as_bool() {
                return false;
            }
            let mut elev: TOKEN_ELEVATION = std::mem::zeroed();
            let mut ret_len = 0u32;
            GetTokenInformation(
                token,
                TokenElevation,
                &mut elev as *mut _ as *mut std::ffi::c_void,
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut ret_len,
            )
            .as_bool()
                && elev.TokenIsElevated != 0
        }
    }

    /// True if the process owning `hwnd` runs elevated (admin).
    pub fn window_process_elevated(hwnd: HWND) -> bool {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid as *mut u32);
            if pid == 0 {
                return false;
            }
            let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let elevated = process_token_elevated(proc);
            let _ = CloseHandle(proc);
            elevated
        }
    }
}

#[cfg(all(not(target_os = "windows"), target_os = "macos"))]
mod unix_impl {
    //! macOS: the Accessibility API (move, minimise, raise) and `NSRunningApplication` (activate), through
    //! `ax_mac::mac` and `window_tile::mac`. No AppleScript, so no Automation prompt; minimise, maximise and title
    //! matching need the Accessibility permission and report `false` without it.

    use crate::ax_mac::mac::{self as ax, Owned};
    use crate::window_tile::mac as wm;
    use std::time::{Duration, Instant};

    /// "AppName WindowTitle" of the app in front, or the app name alone when its window has no title or Accessibility
    /// is off. NEXUS's own window is named by the app only (asking our own process through Accessibility from here
    /// could wait on our own main thread).
    fn foreground_identity() -> Option<String> {
        let front = crate::macos_foreground::front_app()?;
        let title = if front.pid != std::process::id() && ax::trusted() {
            wm::app_element(front.pid)
                .and_then(|app| ax::copy_attr(&app, "AXFocusedWindow").or_else(|| ax::copy_attr(&app, "AXMainWindow")))
                .map(|w| wm::string_attr(&w, "AXTitle"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let line = super::identity_line(&front.name, &title);
        (!line.is_empty()).then_some(line)
    }

    pub fn get_foreground_window_title() -> Option<String> {
        foreground_identity()
    }

    pub fn is_foreground_app(partial: &str) -> bool {
        let want = partial.to_lowercase();
        foreground_identity()
            .map(|t| t.to_lowercase())
            .is_some_and(|t| t.contains(&want))
    }

    /// Bring an app to the front by its name ("chrome" finds Google Chrome), else the window whose title contains
    /// `partial_title`. Un-hides and un-minimises it. True once that app is really in front.
    pub fn focus_app_by_title(partial_title: &str) -> bool {
        let want = partial_title.trim();
        if want.is_empty() {
            return false;
        }
        let own = std::process::id();
        let apps: Vec<wm::RunningApp> = wm::running_apps().into_iter().filter(|a| a.regular && a.pid != own).collect();
        let names: Vec<(u32, &str)> = apps.iter().map(|a| (a.pid, a.name.as_str())).collect();
        if let Some(pid) = super::pick_app(&names, want) {
            return bring_to_front(pid, None);
        }
        // No app by that name: a window that says it in its title (a document, a browser tab).
        if !ax::trusted() {
            tracing::warn!("[WINDOW] no app called {want:?}; window titles need the Accessibility permission");
            return false;
        }
        for a in &apps {
            let Some(el) = wm::app_element(a.pid) else { continue };
            for w in wm::windows_of(&el) {
                if super::title_has(&wm::string_attr(&w, "AXTitle"), want) {
                    return bring_to_front(a.pid, Some(w));
                }
            }
        }
        false
    }

    /// Bring the app with `pid` (and its main window) to the front; true once it is. Used when NEXUS's own window holds
    /// focus and a screen command must act on the app behind it.
    pub fn bring_app_to_front(pid: u32) -> bool {
        bring_to_front(pid, None)
    }

    /// Activate the app and raise `window` (else its main window). macOS 14+ may decline a background app's activation
    /// request; the Accessibility `AXFrontmost` attribute is the fallback.
    fn bring_to_front(pid: u32, window: Option<Owned>) -> bool {
        let app = if ax::trusted() { wm::app_element(pid) } else { None };
        let window = window.or_else(|| {
            let a = app.as_ref()?;
            ax::copy_attr(a, "AXMainWindow")
                .or_else(|| ax::copy_attr(a, "AXFocusedWindow"))
                .or_else(|| wm::windows_of(a).into_iter().next())
        });
        if let Some(w) = &window {
            if wm::bool_attr(w, "AXMinimized") == Some(true) {
                ax::set_bool(w, "AXMinimized", false);
            }
        }
        let asked = wm::activate_app(pid);
        let raise = || {
            if let Some(w) = &window {
                wm::perform(w, "AXRaise");
                ax::set_bool(w, "AXMain", true);
            }
        };
        raise();
        if wait_front(pid, Duration::from_millis(400)) {
            return true;
        }
        if let Some(a) = &app {
            ax::set_bool(a, "AXFrontmost", true);
            raise();
        }
        let ok = wait_front(pid, Duration::from_millis(600));
        if !ok {
            tracing::warn!("[WINDOW] pid {pid} did not come to the front (activation asked: {asked})");
        }
        ok
    }

    fn wait_front(pid: u32, limit: Duration) -> bool {
        let t0 = Instant::now();
        loop {
            if crate::macos_foreground::front_app().is_some_and(|f| f.pid == pid) {
                return true;
            }
            if t0.elapsed() >= limit {
                return false;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
    }

    pub fn wait_and_focus_app(partial_title: &str, timeout_ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        // The app may be launching without a window yet; keep trying until it
        // can take focus or the budget runs out.
        loop {
            if focus_app_by_title(partial_title) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    /// The focused window of the app in front (never NEXUS's own), or None with the reason logged.
    fn front_window() -> Option<Owned> {
        match ax::front_window() {
            Ok((_front, _app, window)) => Some(window),
            Err(why) => {
                tracing::warn!("[WINDOW] no front window to act on: {why}");
                None
            }
        }
    }

    /// Minimise the front window into the Dock. True when macOS took it (or it already was).
    pub fn minimize_foreground_window() -> bool {
        let Some(w) = front_window() else { return false };
        if wm::bool_attr(&w, "AXMinimized") == Some(true) || ax::set_bool(&w, "AXMinimized", true) {
            return true;
        }
        // A window that refuses the attribute may still have the yellow button.
        ax::copy_attr(&w, "AXMinimizeButton").is_some_and(|b| wm::perform(&b, "AXPress"))
    }

    /// "Maximise" on a Mac: fill the visible frame (menu bar and Dock excluded) of the screen the window is on. Not the
    /// green button's full screen, which moves the window to a Space of its own. True when macOS accepted the new frame
    /// (same "a window was acted on" meaning as the Windows `SW_MAXIMIZE`); a window with a maximum size stays smaller.
    pub fn maximize_foreground_window() -> bool {
        let Some(w) = front_window() else { return false };
        if wm::bool_attr(&w, "AXFullScreen") == Some(true) {
            return true;
        }
        if wm::bool_attr(&w, "AXMinimized") == Some(true) {
            ax::set_bool(&w, "AXMinimized", false);
            std::thread::sleep(Duration::from_millis(400));
        }
        let Some(screens) = wm::screens() else {
            tracing::warn!("[WINDOW] maximise: the screen frames could not be read");
            return false;
        };
        let current = wm::frame_of(&w);
        let idx = current
            .and_then(|f| crate::window_tile::screen_at(&screens.list, f.x + f.w / 2.0, f.y + f.h / 2.0))
            .unwrap_or(screens.main);
        let Some(target) = screens.list.get(idx).and_then(|s| crate::window_tile::inner_rect(s.visible)) else {
            return false;
        };
        if !wm::set_frame(&w, crate::window_tile::pt_rect(target)) {
            tracing::warn!("[WINDOW] maximise: macOS refused to move or resize the window");
            return false;
        }
        if let Some(got) = wm::frame_of(&w).and_then(crate::window_tile::rounded_rect) {
            if !crate::window_tile::near(target, got, crate::window_tile::MAC_FIT_TOL) {
                tracing::info!("[WINDOW] maximise: wanted {target:?}, the app kept {got:?} (size limits?)");
            }
        }
        true
    }

    /// No UAC/UIPI concept — never elevated-blocked.
    pub fn our_process_elevated() -> bool {
        false
    }

    /// No UAC/UIPI concept — never elevated-blocked.
    pub fn window_process_elevated(_hwnd: ()) -> bool {
        false
    }
}

/// Lower-case, trimmed, without a trailing ".app" (people and NLU slots sometimes say "Notes.app").
#[cfg(any(target_os = "macos", test))]
fn norm_app(s: &str) -> String {
    let s = s.trim().to_lowercase();
    s.strip_suffix(".app").map(str::trim_end).unwrap_or(&s).to_string()
}

/// The running app a spoken name means, by display name: an exact name first, then a name starting with it, then a
/// name containing it ("chrome" finds Google Chrome). Ties go to the earliest in `apps`. None for an empty name.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn pick_app(apps: &[(u32, &str)], spoken: &str) -> Option<u32> {
    let want = norm_app(spoken);
    if want.is_empty() {
        return None;
    }
    let rank = |name: &str| {
        let n = norm_app(name);
        if n.is_empty() {
            None
        } else if n == want {
            Some(0)
        } else if n.starts_with(&want) {
            Some(1)
        } else if n.contains(&want) {
            Some(2)
        } else {
            None
        }
    };
    apps.iter().filter_map(|(pid, name)| rank(name).map(|r| (r, *pid))).min_by_key(|(r, _)| *r).map(|(_, pid)| pid)
}

/// Case-insensitive "the title contains it", never for an empty search.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn title_has(title: &str, spoken: &str) -> bool {
    let want = spoken.trim().to_lowercase();
    !want.is_empty() && title.to_lowercase().contains(&want)
}

/// "AppName WindowTitle", the shape callers match agent and app names against; the app name alone when there is no
/// title, and no doubled name when the title is just the app's name.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn identity_line(app: &str, title: &str) -> String {
    let (a, t) = (app.trim(), title.trim());
    if t.is_empty() || t.eq_ignore_ascii_case(a) {
        a.to_string()
    } else if a.is_empty() {
        t.to_string()
    } else {
        format!("{a} {t}")
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
mod unix_impl {
    /// On Unix, focus is managed by the window manager.
    /// For now, we just return true (no-op).
    /// A future implementation could use xdotool or wmctrl.
    pub fn focus_app_by_title(_partial_title: &str) -> bool {
        tracing::warn!("live: window focus not implemented on this platform");
        true
    }

    /// Not implemented on this platform — always reports "nothing to act on".
    pub fn minimize_foreground_window() -> bool {
        false
    }

    /// Not implemented on this platform — always reports "nothing to act on".
    pub fn maximize_foreground_window() -> bool {
        false
    }

    /// No UAC/UIPI concept — never elevated-blocked.
    pub fn our_process_elevated() -> bool {
        false
    }

    /// No UAC/UIPI concept — never elevated-blocked.
    pub fn window_process_elevated(_hwnd: ()) -> bool {
        false
    }

    /// Unix stub: always succeeds.
    pub fn wait_and_focus_app(_partial_title: &str, _timeout_ms: u64) -> bool {
        true
    }

    /// Unix stub: returns None.
    pub fn get_foreground_window_title() -> Option<String> {
        None
    }

    /// Unix stub: returns false.
    pub fn is_foreground_app(_partial: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::elevated_block_reason;

    #[test]
    fn test_elevated_gate_truth_table() {
        // Unelevated sender vs elevated target: blocked with guidance.
        let reason = elevated_block_reason(true, false);
        assert!(reason.is_some());
        let text = reason.unwrap().to_lowercase();
        assert!(text.contains("admin"));
        // All other combinations: no block.
        assert!(elevated_block_reason(false, false).is_none());
        assert!(elevated_block_reason(true, true).is_none());
        assert!(elevated_block_reason(false, true).is_none());
    }

    #[test]
    fn test_own_elevation_check_runs() {
        // Must never panic; value depends on how the test runner launched.
        let _ = super::our_process_elevated();
    }

    // ── macOS name matching (pure; run on every OS) ──

    use super::{identity_line, pick_app, title_has};

    const APPS: &[(u32, &str)] = &[
        (10, "Finder"),
        (20, "Google Chrome"),
        (30, "Safari Technology Preview"),
        (40, "Safari"),
        (50, "Code"),
        (60, "WhatsApp"),
        (70, "Notes"),
    ];

    #[test]
    fn window_spoken_app_name_finds_the_running_app() {
        assert_eq!(pick_app(APPS, "WhatsApp"), Some(60));
        assert_eq!(pick_app(APPS, "  whatsapp "), Some(60), "case and spaces do not matter");
        assert_eq!(pick_app(APPS, "chrome"), Some(20), "part of the name");
        assert_eq!(pick_app(APPS, "google"), Some(20), "start of the name");
        assert_eq!(pick_app(APPS, "Notes.app"), Some(70), "a trailing .app is ignored");
    }

    #[test]
    fn window_exact_name_beats_a_longer_name_listed_earlier() {
        assert_eq!(pick_app(APPS, "safari"), Some(40));
        assert_eq!(pick_app(APPS, "safari tech"), Some(30));
        // equal rank: the first listed wins
        assert_eq!(pick_app(&[(1, "Code Helper Tool"), (2, "Code Runner")], "code"), Some(1));
    }

    #[test]
    fn window_unknown_or_empty_names_find_nothing() {
        assert_eq!(pick_app(APPS, "spotify"), None);
        assert_eq!(pick_app(APPS, ""), None);
        assert_eq!(pick_app(APPS, "   "), None);
        assert_eq!(pick_app(APPS, ".app"), None);
        assert_eq!(pick_app(&[], "finder"), None);
        assert_eq!(pick_app(&[(1, "")], "x"), None, "an app without a name never matches");
    }

    #[test]
    fn window_title_search_is_case_insensitive_and_never_empty() {
        assert!(title_has("Inbox (3) - lsai@example.com - Gmail", "gmail"));
        assert!(title_has("README.md — NEXUS", "readme"));
        assert!(!title_has("README.md — NEXUS", "notes"));
        assert!(!title_has("anything", ""));
        assert!(!title_has("anything", "  "));
    }

    #[test]
    fn window_identity_line_names_the_app_then_the_window() {
        assert_eq!(identity_line("Terminal", "claude — 80×24"), "Terminal claude — 80×24");
        assert_eq!(identity_line("Finder", ""), "Finder");
        assert_eq!(identity_line("Finder", "finder"), "Finder", "no doubled name");
        assert_eq!(identity_line("", "Untitled"), "Untitled");
        assert_eq!(identity_line(" ", " "), "");
    }

    /// Live: maximize, minimize and focus on a scratch TextEdit document (never the user's windows). Refuses to run
    /// while TextEdit is already open, because it quits TextEdit at the end.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "opens and quits TextEdit, moves focus; needs the Accessibility permission"]
    fn live_mac_maximize_minimize_and_focus_a_scratch_textedit_window() {
        use super::{focus_app_by_title, maximize_foreground_window, minimize_foreground_window};
        use std::process::Command;
        use std::time::{Duration, Instant};
        let running = |name: &str| Command::new("pgrep").args(["-x", name]).status().is_ok_and(|s| s.success());
        assert!(!running("TextEdit"), "close TextEdit first: this test quits it at the end");
        let front_is = |name: &str| crate::macos_foreground::front_app().is_some_and(|f| f.name == name);
        let was_front = crate::macos_foreground::front_app();
        let doc = std::env::temp_dir().join(format!("nexus-window-test-{}.txt", std::process::id()));
        std::fs::write(&doc, "NEXUS window test\n").unwrap();
        Command::new("open").args(["-a", "TextEdit"]).arg(&doc).status().unwrap();
        let t0 = Instant::now();
        while !(front_is("TextEdit") && crate::ax_mac::mac::front_window_frame().is_some()) {
            assert!(t0.elapsed() < Duration::from_secs(10), "TextEdit never showed its window");
            std::thread::sleep(Duration::from_millis(100));
        }
        let result = std::panic::catch_unwind(|| {
            let (screen_w, _) = crate::screen::primary_monitor_size().unwrap();
            assert!(maximize_foreground_window(), "maximize");
            std::thread::sleep(Duration::from_millis(300));
            let (_, f) = crate::ax_mac::mac::front_window_frame().unwrap();
            eprintln!("maximized frame: {f:?} (screen width {screen_w})");
            assert!(f.w >= screen_w as f64 * 0.95, "fills the screen width");
            assert!(minimize_foreground_window(), "minimize");
            std::thread::sleep(Duration::from_millis(600));
            assert!(focus_app_by_title("Finder"), "focus another app");
            assert!(!front_is("TextEdit"));
            assert!(focus_app_by_title("TextEdit"), "focus TextEdit back");
            std::thread::sleep(Duration::from_millis(600));
            let back = crate::ax_mac::mac::front_window_frame();
            eprintln!("after refocus: {back:?}");
            assert!(back.is_some(), "its window came back from the Dock");
        });
        let _ = Command::new("pkill").args(["-x", "TextEdit"]).status();
        let _ = std::fs::remove_file(&doc);
        // Hand the screen back to whatever the user had in front.
        if let Some(b) = was_front.and_then(|f| f.bundle_id) {
            let _ = Command::new("open").args(["-b", &b]).status();
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }
}
