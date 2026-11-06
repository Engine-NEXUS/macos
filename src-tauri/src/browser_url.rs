//! Cross-platform active browser URL extraction.
//!
//! Used by the Architecture Mapper to auto-detect the GitHub repo URL
//! from the user's active browser tab.
//!
//! Platform approaches:
//! - Windows: UI Automation API (reads address bar text directly)
//! - macOS: AppleScript (queries active tab URL)
//! - Linux: xdotool + xclip (keyboard shortcut to copy URL)

/// Extract the URL from the currently focused browser tab.
///
/// Returns `Some(url)` if a browser is foreground and the URL was extracted,
/// or `None` if no browser is foreground or extraction failed.
pub fn get_active_browser_url() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        get_browser_url_windows()
    }
    #[cfg(target_os = "macos")]
    {
        get_browser_url_macos()
    }
    #[cfg(target_os = "linux")]
    {
        get_browser_url_linux()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// URL evidence for screen capture and foreground review. Unlike the legacy
/// accessor, this never invents a URL from a window title. A missing URL is
/// valid context; callers can still analyse the identified foreground window.
pub(crate) fn get_verified_active_browser_url() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        get_browser_url_windows_verified()
    }
    #[cfg(not(target_os = "windows"))]
    {
        get_active_browser_url()
    }
}

fn normalized_verified_web_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_whitespace) {
        return None;
    }
    let candidate = if raw.starts_with("https://") || raw.starts_with("http://") {
        raw.to_string()
    } else if raw.contains('.') && !raw.contains("://") && !raw.contains('@') {
        // Chromium can omit the scheme in the address bar. The URL is used as
        // optional context; if it cannot be parsed, capture remains possible.
        format!("https://{raw}")
    } else {
        return None;
    };
    let url = reqwest::Url::parse(&candidate).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    Some(url.to_string())
}

/// Extract the host (domain) from a URL string for logging.
/// Returns None if the URL cannot be parsed.
pub(crate) fn extract_host(url: &str) -> Option<String> {
    reqwest::Url::parse(url).ok()?.host_str().map(|s| s.to_string())
}

fn is_address_bar_identity(name: &str, automation_id: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let id = automation_id.to_ascii_lowercase();
    name.contains("address and search bar")
        || name.contains("search or enter address")
        || name.contains("enter web address")
        || name.trim() == "address bar"
        || id.contains("omnibox")
        || id.contains("locationbar")
}

/// Read only the address bar belonging to the exact foreground browser HWND.
/// Searching from the focused element can start inside the webpage, where an
/// arbitrary Edit field is neither the address bar nor a safe page identity.
#[cfg(target_os = "windows")]
fn get_browser_url_windows_verified() -> Option<String> {
    get_browser_url_windows_inner(true)
}

/// Same logic as `get_browser_url_windows_verified` but WITHOUT the
/// `has_keyboard_focus()` rejection. Used as a relaxed fallback when the
/// verified reader returns None (e.g., Chromium hasn't built its accessibility
/// tree yet). Accepts the value only if `normalized_verified_web_url` parses
/// it to an http(s) URL with a host.
#[cfg(target_os = "windows")]
fn get_browser_url_windows_relaxed() -> Option<String> {
    get_browser_url_windows_inner(false)
}

/// Core URL extraction logic shared by verified and relaxed readers.
/// If `require_no_focus` is true, rejects when the address bar has keyboard focus.
#[cfg(target_os = "windows")]
fn get_browser_url_windows_inner(require_no_focus: bool) -> Option<String> {
    use uiautomation::controls::ControlType;
    use uiautomation::types::Handle;
    use uiautomation::{UIAutomation, UIElement};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0 == 0 {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid); }
    if pid == 0 || !is_browser_process(&get_process_name(pid)?) {
        return None;
    }
    let automation = UIAutomation::new().ok()?;
    let window = automation.element_from_handle(Handle::from(hwnd.0 as isize)).ok()?;

    let location = automation
        .create_matcher()
        .from(window.clone())
        .timeout(400)
        .classname("LocationBarView")
        .find_first()
        .ok();
    let edit = location.and_then(|bar| {
        automation
            .create_matcher()
            .from(bar)
            .timeout(300)
            .control_type(ControlType::Edit)
            .find_first()
            .ok()
    }).or_else(|| {
        automation
            .create_matcher()
            .from(window)
            .timeout(500)
            .control_type(ControlType::Edit)
            .filter_fn(Box::new(|element: &UIElement| {
                Ok(is_address_bar_identity(
                    &element.get_name().unwrap_or_default(),
                    &element.get_automation_id().unwrap_or_default(),
                ))
            }))
            .find_first()
            .ok()
    })?;

    if require_no_focus && edit.has_keyboard_focus().unwrap_or(false) {
        return None;
    }
    let url = normalized_verified_web_url(&read_url_from_edit(&edit)?)?;
    if unsafe { GetForegroundWindow() }.0 != hwnd.0 {
        return None;
    }
    Some(url)
}

/// Resolve the foreground browser URL using a multi-stage ladder.
/// Each stage runs only if the previous gave None. All blocking UIA work
/// runs inside `tokio::task::spawn_blocking`.
/// Logs each stage with `println!("[REVERSE] url stage={} result={}", ...)`.
/// Never prints more than scheme+host of the URL.
#[cfg(target_os = "windows")]
pub(crate) async fn resolve_foreground_url_for_command() -> Option<String> {
    use crate::live::commands::keyboard::press_hotkey;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let initial_hwnd = unsafe { GetForegroundWindow() };
    if initial_hwnd.0 == 0 {
        return None;
    }

    // Helper to log result (scheme+host only)
    let log_result = |stage: &str, result: &Option<String>| {
        let display = result.as_ref().map(|u| {
            if let Ok(parsed) = reqwest::Url::parse(u) {
                format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""))
            } else {
                "invalid".to_string()
            }
        }).unwrap_or_else(|| "none".to_string());
        println!("[REVERSE] url stage={} result={}", stage, display);
    };

    // Stage 0: Verified (once)
    let result = tokio::task::spawn_blocking(|| get_browser_url_windows_verified()).await.ok().flatten();
    log_result("verified", &result);
    if let Some(url) = result {
        return Some(url);
    }

    // Stages 1-3: Retry verified up to 3 times with 350ms delay
    for _retry in 1..=3 {
        tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        // Check HWND hasn't changed
        if unsafe { GetForegroundWindow() }.0 != initial_hwnd.0 {
            println!("[REVERSE] url stage=verified_retry hwnd_changed");
            return None;
        }
        let result = tokio::task::spawn_blocking(|| get_browser_url_windows_verified()).await.ok().flatten();
        log_result("verified_retry", &result);
        if let Some(url) = result {
            return Some(url);
        }
    }

    // Stage 4: Relaxed reader (no keyboard focus check)
    if unsafe { GetForegroundWindow() }.0 != initial_hwnd.0 {
        println!("[REVERSE] url stage=relaxed hwnd_changed");
        return None;
    }
    let result = tokio::task::spawn_blocking(|| get_browser_url_windows_relaxed()).await.ok().flatten();
    log_result("relaxed", &result);
    if let Some(url) = result {
        return Some(url);
    }

    // Stage 5: Ctrl+L to focus omnibox, read via relaxed reader, then Escape
    if unsafe { GetForegroundWindow() }.0 != initial_hwnd.0 {
        println!("[REVERSE] url stage=ctrl_l hwnd_changed");
        return None;
    }
    // Send Ctrl+L
    if press_hotkey(&["ctrl", "l"]).is_err() {
        println!("[REVERSE] url stage=ctrl_l send_failed");
        return None;
    }
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;

    if unsafe { GetForegroundWindow() }.0 != initial_hwnd.0 {
        println!("[REVERSE] url stage=ctrl_l hwnd_changed_after_ctrl_l");
        // Still send Escape to restore focus
        let _ = press_hotkey(&["escape"]);
        return None;
    }

    let result = tokio::task::spawn_blocking(|| get_browser_url_windows_relaxed()).await.ok().flatten();
    // ALWAYS send Escape to give focus back to the page
    let _ = press_hotkey(&["escape"]);

    log_result("ctrl_l", &result);
    result
}

// Non-Windows stub: just call the existing accessor.
#[cfg(not(target_os = "windows"))]
pub(crate) async fn resolve_foreground_url_for_command() -> Option<String> {
    get_active_browser_url()
}

/// Known browser executables (lowercase). Pure.
pub fn is_browser_process(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "chrome.exe" | "brave.exe" | "msedge.exe" | "firefox.exe" | "opera.exe" | "vivaldi.exe"
    )
}

/// True when a known browser holds the foreground window.
///
/// Unlike `get_active_browser_url` this needs no readable address bar: a fresh
/// new tab has an empty omnibox, so the URL read returned `None` and queued
/// ghost steps ("search almonds" right after "new tab") were skipped with
/// "Target window lost" although the browser was plainly in front.
pub fn foreground_is_browser() -> bool {
    #[cfg(target_os = "windows")]
    {
        get_foreground_process_id()
            .and_then(get_process_name)
            .map(|name| is_browser_process(&name))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        get_active_browser_url().is_some()
    }
}

// ─── Windows: UI Automation ──────────────────────────────────────────

#[cfg(target_os = "windows")]
fn get_browser_url_windows() -> Option<String> {
    use uiautomation::UIAutomation;
    use uiautomation::controls::ControlType;

    // 1. Get the foreground window and its process ID
    let pid = get_foreground_process_id()?;
    let proc_name = get_process_name(pid)?;
    tracing::debug!("[browser_url] foreground process: {} (pid={})", proc_name, pid);

    // 2. Check if it's a known browser
    let is_chrome = proc_name == "chrome.exe" || proc_name == "brave.exe";
    let is_edge = proc_name == "msedge.exe";
    let is_firefox = proc_name == "firefox.exe";

    if !is_chrome && !is_edge && !is_firefox {
        return None;
    }

    // 3. Use UI Automation to find the address bar.
    //    Use a short timeout (500ms) — if the UI tree structure doesn't match,
    //    we fall back to window title parsing quickly.
    let automation = UIAutomation::new().ok()?;
    let root = automation.get_root_element().ok()?;

    // Find the browser element by process ID.
    // uiautomation v0.25 doesn't have .process_id() on UIMatcher, so we
    // use get_focused_element() which returns the focused element in the
    // foreground window (the browser). We then walk up to find the root
    // browser element, or just search for Edit controls from there.
    // Root at the foreground WINDOW: with focus inside the page the focused
    // element is a page node whose subtree has no address bar.
    let browser = crate::screen::foreground_window_element(&automation)
        .or_else(|| automation.get_focused_element().ok())
        .or_else(|| {
            // Fallback: find the first window element from root
            automation
                .create_matcher()
                .from(root.clone())
                .timeout(500)
                .control_type(ControlType::Window)
                .find_first()
                .ok()
        })?;

    // 4. Navigate the UI tree to find the address bar Edit control
    //    Chrome/Brave: ToolbarView → LocationBarView → Edit
    //    Edge: EdgeToolbarView → LocationBarView → Edit
    //    Firefox: Edit (directly under browser)
    let url = if is_firefox {
        // Firefox: find Edit control directly
        let edit = automation
            .create_matcher()
            .from(browser)
            .timeout(500)
            .control_type(ControlType::Edit)
            .find_first()
            .ok()?;
        read_url_from_edit(&edit)
    } else {
        // Chrome/Edge/Brave: try direct Edit search first (fastest),
        // then fall back to toolbar → address bar → edit path.
        let edit_direct = automation
            .create_matcher()
            .from(browser.clone())
            .timeout(500)
            .control_type(ControlType::Edit)
            .find_first();

        if let Ok(edit) = edit_direct {
            if let Some(url) = read_url_from_edit(&edit) {
                return Some(url);
            }
        }

        // Slower path: toolbar → address bar → edit
        let toolbar_class = if is_edge { "EdgeToolbarView" } else { "ToolbarView" };

        let toolbar = automation
            .create_matcher()
            .from(browser.clone())
            .timeout(500)
            .classname(toolbar_class)
            .find_first();

        if let Ok(toolbar) = toolbar {
            let address_bar = automation
                .create_matcher()
                .from(toolbar)
                .timeout(500)
                .classname("LocationBarView")
                .find_first();

            if let Ok(address_bar) = address_bar {
                let edit = automation
                    .create_matcher()
                    .from(address_bar)
                    .timeout(500)
                    .control_type(ControlType::Edit)
                    .find_first()
                    .ok()?;
                read_url_from_edit(&edit)
            } else {
                None
            }
        } else {
            None
        }
    };

    let resolved_url = url.map(|u| {
        let trimmed = u.trim().to_string();
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed
        } else if trimmed.contains("mail.google.com") || trimmed.contains("github.com") || trimmed.contains('.') {
            format!("https://{}", trimmed)
        } else {
            trimmed
        }
    });

    if resolved_url.is_some() {
        return resolved_url;
    }

    // Fallback: inspect foreground window title for Gmail
    if let Some(title) = get_foreground_window_title() {
        let lower = title.to_lowercase();
        if lower.contains("gmail") {
            tracing::info!("[browser_url] detected Gmail in window title: {}", title);
            return Some(format!(
                "https://mail.google.com/mail/u/0/#inbox?title={}",
                url_encode(&title)
            ));
        }
    }

    None
}

fn url_encode(s: &str) -> String {
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

#[cfg(target_os = "windows")]
fn read_url_from_edit(edit: &uiautomation::UIElement) -> Option<String> {
    use uiautomation::types::UIProperty;
    if let Ok(url_variant) = edit.get_property_value(UIProperty::ValueValue) {
        if let Ok(url) = url_variant.get_string() {
            let trimmed = url.trim();
            if !trimmed.is_empty() && (trimmed.contains('.') || trimmed.starts_with("http")) {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Ok(name) = edit.get_name() {
        let trimmed = name.trim();
        if !trimmed.is_empty()
            && (trimmed.contains('.') || trimmed.starts_with("http"))
            && !trimmed.contains("Address and search bar")
        {
            return Some(trimmed.to_string());
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn get_foreground_window_title() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return None;
        }
        let mut title_buf = vec![0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title_buf);
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&title_buf[..len as usize]))
    }
}

// ─── Screen-tour context accessors (pub(crate)) ──────────────────────

/// Foreground window title (screen-tour context).
#[cfg(target_os = "windows")]
pub(crate) fn foreground_window_title() -> Option<String> {
    get_foreground_window_title()
}

/// macOS: the front window's title via System Events (needs the user's Automation permission; None without it, so
/// callers fall back to the bundle id). Linux: not implemented.
#[cfg(target_os = "macos")]
pub(crate) fn foreground_window_title() -> Option<String> {
    let app = crate::macos_foreground::front_app()?;
    crate::macos_foreground::window_title(app.pid)
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub(crate) fn foreground_window_title() -> Option<String> {
    None
}

/// macOS: the frontmost app's lower-case bundle id (e.g. `com.google.chrome`), the counterpart of `brave.exe`. `None`
/// means "unknown", and every safety caller must treat unknown as "refuse" (see `hand::guard_now`).
#[cfg(target_os = "macos")]
pub(crate) fn foreground_process_name() -> Option<String> {
    crate::macos_foreground::front_app().map(|a| a.key())
}

/// Foreground process executable name, lower-case (e.g. `brave.exe`).
#[cfg(target_os = "windows")]
pub(crate) fn foreground_process_name() -> Option<String> {
    get_process_name(get_foreground_process_id()?)
}

/// Full path of the foreground process executable (to open a URL in the SAME browser the user is
/// looking at instead of whatever the system default is).
#[cfg(target_os = "windows")]
pub(crate) fn foreground_process_path() -> Option<std::path::PathBuf> {
    get_process_path(get_foreground_process_id()?)
}



/// Foreground window rect in physical px: (x, y, w, h).
#[cfg(target_os = "windows")]
pub(crate) fn foreground_window_rect() -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return None;
        }
        let mut r = RECT::default();
        if !GetWindowRect(hwnd, &mut r).as_bool() {
            return None;
        }
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }
}

/// Page-content rect of the foreground browser (UI Automation `Document`
/// control = the web view, i.e. without tab strip / URL bar / bookmarks).
/// Physical px (x, y, w, h). None when the tree does not expose it (the
/// caller then falls back to the window rect minus a toolbar inset).
#[cfg(target_os = "windows")]
pub(crate) fn browser_document_rect() -> Option<(i32, i32, i32, i32)> {
    use uiautomation::controls::ControlType;
    use uiautomation::types::Handle;
    use uiautomation::UIAutomation;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0 == 0 {
        return None;
    }
    let automation = UIAutomation::new().ok()?;
    let window = automation.element_from_handle(Handle::from(hwnd.0 as isize)).ok()?;
    let doc = automation
        .create_matcher()
        .from(window)
        .timeout(700)
        .control_type(ControlType::Document)
        .find_first()
        .ok()?;
    let r = doc.get_bounding_rectangle().ok()?;
    let (x, y) = (r.get_left(), r.get_top());
    Some((x, y, r.get_right() - x, r.get_bottom() - y))
}

/// Process id of the foreground window (None when it cannot be read; always None off Windows).
#[cfg(target_os = "windows")]
pub(crate) fn foreground_pid() -> Option<u32> {
    get_foreground_process_id()
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn foreground_pid() -> Option<u32> {
    None
}

#[cfg(target_os = "windows")]
fn get_foreground_process_id() -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid as *mut u32);
        if pid == 0 {
            None
        } else {
            Some(pid)
        }
    }
}

#[cfg(target_os = "windows")]
fn get_process_name(pid: u32) -> Option<String> {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};
    use windows::Win32::System::ProcessStatus::K32GetModuleBaseNameW;
    unsafe {
        let process_handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
        let mut name_buf = vec![0u16; 256];
        let len = K32GetModuleBaseNameW(process_handle, None, &mut name_buf);
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&name_buf[..len as usize]).to_lowercase())
    }
}

#[cfg(target_os = "windows")]
fn get_process_path(pid: u32) -> Option<std::path::PathBuf> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::K32GetModuleFileNameExW;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let len = K32GetModuleFileNameExW(handle, None, &mut buf);
        let _ = CloseHandle(handle);
        if len == 0 {
            return None;
        }
        Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

// ─── macOS: AppleScript ──────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn get_browser_url_macos() -> Option<String> {
    // Only the browser in front, asked by its own bundle id with a bounded wait. The old scripts told "Google Chrome"
    // then "Safari" by name: that launched Chrome when it was not running, read Chrome's tab while Safari was in front,
    // and waited without limit while macOS showed its Automation prompt.
    let front = crate::macos_foreground::front_app()?;
    crate::screen_context::mac_front_browser_url(front.bundle_id.as_deref()?)
}

// ─── Linux: xdotool + xclip ──────────────────────────────────────────

#[cfg(target_os = "linux")]
fn get_browser_url_linux() -> Option<String> {
    // 1. Get the active window ID
    let wid_output = std::process::Command::new("xdotool")
        .args(["getactivewindow"])
        .output()
        .ok()?;
    if !wid_output.status.success() {
        return None;
    }
    let wid = String::from_utf8_lossy(&wid_output.stdout).trim().to_string();
    if wid.is_empty() {
        return None;
    }

    // 2. Send Ctrl+L (focus address bar) + Ctrl+C (copy) + Escape (close)
    let _ = std::process::Command::new("xdotool")
        .args([
            "key", "--window", &wid, "--delay", "20",
            "--clearmodifiers", "ctrl+l", "ctrl+c", "Escape",
        ])
        .output();

    // 3. Read the URL from clipboard
    let clip_output = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&clip_output.stdout).trim().to_string();
    if url.starts_with("http") {
        Some(url)
    } else {
        None
    }
}

// ─── Gmail URL Thread Extraction ─────────────────────────────────────

/// Extracts a Gmail thread ID or message identifier from an active Gmail URL.
/// Matches URLs containing `mail.google.com` with a fragment ending in a thread/message ID:
/// e.g. `https://mail.google.com/mail/u/0/#inbox/FMfcgzQVzQ...` -> `Some("FMfcgzQVzQ...")`
/// e.g. `https://mail.google.com/mail/u/0/#all/18ac5d7e3` -> `Some("18ac5d7e3")`
pub fn extract_gmail_thread_id_from_url(url: &str) -> Option<String> {
    if !url.contains("mail.google.com") {
        return None;
    }

    // Split on '#' to get the hash fragment
    let hash = url.split('#').nth(1)?;

    // Common paths: inbox/<id>, all/<id>, search/<query>/<id>, label/<name>/<id>
    let parts: Vec<&str> = hash.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }

    let last_segment = parts.last()?.trim();
    // Valid Gmail thread IDs are alphanumeric strings usually between 8 and 40 characters
    if last_segment.len() >= 8 && last_segment.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        Some(last_segment.to_string())
    } else {
        None
    }
}

/// Extracts the numeric account index `/u/<N>/` from an active Gmail URL.
/// e.g. `https://mail.google.com/mail/u/1/#inbox/...` -> `Some(1)`
pub fn extract_gmail_account_index_from_url(url: &str) -> Option<usize> {
    if let Some(pos) = url.find("/mail/u/") {
        let rem = &url[pos + 8..];
        if let Some(slash) = rem.find('/') {
            return rem[..slash].parse::<usize>().ok();
        }
    }
    None
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_browser_process_known_browsers_only() {
        for n in ["chrome.exe", "Brave.exe", "MSEDGE.EXE", " firefox.exe ", "opera.exe", "vivaldi.exe"] {
            assert!(is_browser_process(n), "{n}");
        }
        for n in ["notepad.exe", "explorer.exe", "code.exe", "whatsapp.exe", "", "chrome"] {
            assert!(!is_browser_process(n), "{n}");
        }
    }

    #[test]
    fn verified_url_requires_a_web_address() {
        assert_eq!(
            normalized_verified_web_url("https://example.com/path?x=1"),
            Some("https://example.com/path?x=1".into())
        );
        assert_eq!(
            normalized_verified_web_url("example.com/path"),
            Some("https://example.com/path".into())
        );
        for value in ["brave://settings", "about:blank", "hello world", "search term", "user@example.com"] {
            assert!(normalized_verified_web_url(value).is_none(), "{value}");
        }
    }

    #[test]
    fn only_address_bar_labels_are_accepted() {
        assert!(is_address_bar_identity("Address and search bar", ""));
        assert!(is_address_bar_identity("Search or enter address", ""));
        assert!(is_address_bar_identity("", "OmniboxView"));
        assert!(!is_address_bar_identity("Email address", ""));
        assert!(!is_address_bar_identity("Shipping address", ""));
        assert!(!is_address_bar_identity("Search this page", ""));
    }

    #[test]
    fn test_get_active_browser_url_does_not_crash() {
        // This test just verifies the function doesn't panic.
        // It may return None if no browser is foreground.
        let _ = get_active_browser_url();
    }

    #[test]
    fn test_extract_gmail_thread_id_from_url() {
        // Standard inbox thread
        let u1 = "https://mail.google.com/mail/u/0/#inbox/FMfcgzQVzQZlPkrXFw";
        assert_eq!(
            extract_gmail_thread_id_from_url(u1),
            Some("FMfcgzQVzQZlPkrXFw".to_string())
        );

        // All mail thread with hex ID
        let u2 = "https://mail.google.com/mail/u/1/#all/18ac5d7e3901";
        assert_eq!(
            extract_gmail_thread_id_from_url(u2),
            Some("18ac5d7e3901".to_string())
        );

        // Search thread
        let u3 = "https://mail.google.com/mail/u/0/#search/project/FMfcgzQ1234567";
        assert_eq!(
            extract_gmail_thread_id_from_url(u3),
            Some("FMfcgzQ1234567".to_string())
        );

        // Main inbox list without opened email
        let u_list = "https://mail.google.com/mail/u/0/#inbox";
        assert_eq!(extract_gmail_thread_id_from_url(u_list), None);

        // Non-Gmail site
        let u_github = "https://github.com/facebook/react/pull/12345";
        assert_eq!(extract_gmail_thread_id_from_url(u_github), None);
    }

    #[test]
    fn test_extract_gmail_account_index_from_url() {
        assert_eq!(
            extract_gmail_account_index_from_url("https://mail.google.com/mail/u/0/#inbox/FMfcgzQ123"),
            Some(0)
        );
        assert_eq!(
            extract_gmail_account_index_from_url("https://mail.google.com/mail/u/1/#all/18ac5d7e3"),
            Some(1)
        );
        assert_eq!(
            extract_gmail_account_index_from_url("https://mail.google.com/mail/u/5/#search/test/FMfcgz"),
            Some(5)
        );
        assert_eq!(
            extract_gmail_account_index_from_url("https://mail.google.com/mail/#inbox"),
            None
        );
        assert_eq!(
            extract_gmail_account_index_from_url("https://news.ycombinator.com"),
            None
        );
    }

    #[test]
    fn normalized_verified_web_url_accepts_valid_http_urls() {
        assert_eq!(
            normalized_verified_web_url("https://example.com/path?x=1"),
            Some("https://example.com/path?x=1".into())
        );
        assert_eq!(
            normalized_verified_web_url("http://localhost:3000/foo"),
            Some("http://localhost:3000/foo".into())
        );
        // Schemeless with host -> prepends https
        assert_eq!(
            normalized_verified_web_url("nextjs.org/docs"),
            Some("https://nextjs.org/docs".into())
        );
        assert_eq!(
            normalized_verified_web_url("github.com/user/repo"),
            Some("https://github.com/user/repo".into())
        );
        assert_eq!(
            normalized_verified_web_url("a.b/c?d=e"),
            Some("https://a.b/c?d=e".into())
        );
    }

    #[test]
    fn normalized_verified_web_url_rejects_invalid() {
        // Browser internal URLs
        assert!(normalized_verified_web_url("chrome://settings").is_none());
        assert!(normalized_verified_web_url("brave://settings").is_none());
        assert!(normalized_verified_web_url("about:blank").is_none());
        assert!(normalized_verified_web_url("about:newtab").is_none());
        assert!(normalized_verified_web_url("edge://settings").is_none());
        assert!(normalized_verified_web_url("firefox://settings").is_none());

        // Internal whitespace -> reject (leading/trailing is trimmed)
        assert!(normalized_verified_web_url("https://example.com/path with space").is_none());
        assert!(normalized_verified_web_url("search term").is_none());
        assert!(normalized_verified_web_url("user@example.com").is_none()); // email, not URL

        // Empty / too long
        assert!(normalized_verified_web_url("").is_none());
        let long = "https://example.com/".repeat(300);
        assert!(normalized_verified_web_url(&long).is_none());
    }

}
