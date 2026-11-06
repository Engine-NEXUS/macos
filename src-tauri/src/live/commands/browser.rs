//! Browser navigation commands — new tab, navigate to URL, search.
//!
//! Uses keyboard shortcuts (Ctrl+T, Ctrl+L) to control the browser without
//! needing UI Automation or Playwright. This works with any browser
//! (Brave, Chrome, Edge, Firefox) because these shortcuts are universal.
//!
//! For searches, reuses the existing open_search() from command_executor
//! which opens Google in the default browser.

use std::thread;
use std::time::Duration;

use super::keyboard;

/// Open a new browser tab (Ctrl+T).
/// Assumes a browser is currently focused.
pub fn new_tab() -> Result<(), String> {
    tracing::info!("live: browser new tab");
    // Shortcut brain (E1): gated + verified. When the title changed the tab demonstrably exists, so the
    // fixed settle wait is only kept for the unverified / legacy cases.
    match crate::shortcuts::run_or_legacy(crate::shortcuts::Action::NewTab, None, || {
        keyboard::press_shortcut(&["ctrl", "t"])
    })? {
        crate::shortcuts::Sent::Router(crate::shortcuts::router::Verified::Yes) => {}
        _ => thread::sleep(Duration::from_millis(300)),
    }
    Ok(())
}

/// Focus the address bar (Ctrl+L) through the shortcut brain, or the legacy hotkey for unknown apps.
fn address_bar_hotkey() -> Result<(), String> {
    crate::shortcuts::run_or_legacy(crate::shortcuts::Action::AddressBar, None, || {
        keyboard::press_shortcut(&["ctrl", "l"])
    })
    .map(|_| ())
}

/// Navigate to a URL in the current browser tab (Ctrl+L → type URL → Enter).
/// Assumes a browser is currently focused.
pub fn navigate(url: &str) -> Result<(), String> {
    tracing::info!("live: browser navigate to: {url}");

    // Ctrl+L focuses the address bar
    address_bar_hotkey()?;
    thread::sleep(Duration::from_millis(200));

    // Type the URL
    keyboard::type_text(url)?;
    thread::sleep(Duration::from_millis(200));

    // Press Enter to navigate
    keyboard::press_key("enter")?;
    Ok(())
}

/// Search in the current browser tab (Ctrl+L → type query → Enter).
/// This uses the browser's default search engine.
pub fn search(query: &str) -> Result<(), String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("empty search".to_string());
    }
    let url = search_url(q);
    tracing::info!("live: browser search: {q}");

    // Open the results page itself. Typing into the address bar (Ctrl+L, text, Enter with fixed
    // 200 ms gaps) raced with focus and was never verified; a URL cannot land in the wrong box.
    // Prefer the browser the user is looking at, so the results open in a new tab there.
    #[cfg(target_os = "windows")]
    {
        if let (Some(name), Some(path)) = (
            crate::browser_url::foreground_process_name(),
            crate::browser_url::foreground_process_path(),
        ) {
            if crate::browser_url::is_browser_process(&name)
                && crate::no_window::command(&path).arg(&url).spawn().is_ok()
            {
                return Ok(());
            }
        }
    }
    open::that(&url).map_err(|e| format!("open search: {e}"))
}

/// Google results URL for a spoken query (percent-encoded). Pure.
pub fn search_url(query: &str) -> String {
    format!(
        "https://www.google.com/search?q={}",
        crate::memcore::mailtriage::pct_encode(query.trim())
    )
}

/// Focus the address bar / search bar in the current browser tab (Ctrl+L).
pub fn focus_search_bar() -> Result<(), String> {
    tracing::info!("live: browser focus search bar");
    address_bar_hotkey()?;
    thread::sleep(Duration::from_millis(200));
    Ok(())
}

/// Open a specific website by name (e.g., "wikipedia", "github").
/// Maps common site names to their URLs.
pub fn open_site(site: &str) -> Result<(), String> {
    let url = resolve_site_url(site);
    tracing::info!("live: opening site '{}' → {url}", site);

    // Use the existing open::that to open in default browser
    open::that(&url).map_err(|e| format!("open site: {e}"))?;
    thread::sleep(Duration::from_millis(1000));
    Ok(())
}

/// Map a site name to its URL.
fn resolve_site_url(site: &str) -> String {
    let lower = site.to_lowercase();
    match lower.as_str() {
        "wikipedia" | "wiki" => "https://www.wikipedia.org".to_string(),
        "github" => "https://github.com".to_string(),
        "google" => "https://www.google.com".to_string(),
        "youtube" => "https://www.youtube.com".to_string(),
        "gmail" => "https://mail.google.com".to_string(),
        "twitter" | "x" => "https://twitter.com".to_string(),
        "reddit" => "https://www.reddit.com".to_string(),
        "stackoverflow" => "https://stackoverflow.com".to_string(),
        "linkedin" => "https://www.linkedin.com".to_string(),
        "amazon" => "https://www.amazon.com".to_string(),
        "netflix" => "https://www.netflix.com".to_string(),
        "spotify" => "https://open.spotify.com".to_string(),
        _ => {
            // If it looks like a URL, use it directly
            if lower.starts_with("http://") || lower.starts_with("https://") {
                lower
            } else if lower.contains('.') {
                format!("https://{lower}")
            } else {
                format!("https://www.{lower}.com")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_site_url() {
        assert_eq!(resolve_site_url("wikipedia"), "https://www.wikipedia.org");
        assert_eq!(resolve_site_url("wiki"), "https://www.wikipedia.org");
        assert_eq!(resolve_site_url("github"), "https://github.com");
        assert_eq!(resolve_site_url("google"), "https://www.google.com");
        assert_eq!(
            resolve_site_url("youtube"),
            "https://www.youtube.com"
        );
    }

    #[test]
    fn test_resolve_site_url_direct() {
        assert_eq!(
            resolve_site_url("https://example.com"),
            "https://example.com"
        );
        assert_eq!(
            resolve_site_url("example.com"),
            "https://example.com"
        );
    }

    #[test]
    fn test_resolve_site_url_unknown() {
        assert_eq!(
            resolve_site_url("mycustomsite"),
            "https://www.mycustomsite.com"
        );
    }

    #[test]
    fn search_url_encodes_the_query() {
        assert_eq!(search_url("fitness psychology"), "https://www.google.com/search?q=fitness%20psychology");
        assert_eq!(search_url("  rust & go  "), "https://www.google.com/search?q=rust%20%26%20go");
        assert!(search_url("a=b&c").contains("a%3Db%26c"));
    }
}
