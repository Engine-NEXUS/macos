//! What the user is actually looking at — context for the narrated screen
//! tour ("Nexus, analyse my screen").
//!
//! Pixels alone cannot say what a page or video IS, so the tour also gets:
//! the foreground app, the window/tab title, the page URL (privacy-trimmed)
//! and, for YouTube, the video title + channel from YouTube's public oEmbed
//! endpoint (no key; only the clean video URL is sent). It also decides the
//! screen REGION to capture: the page content only (UI Automation `Document`
//! for browsers, else the window), so tabs / URL bar / taskbar never reach
//! the model. The page URL is OPTIONAL: it enriches the answer, it never
//! decides whether a capture may happen. Everything pure here is unit-tested;
//! the Windows probes and the oEmbed call are thin glue around it. On macOS
//! the same target comes from `lsappinfo` and the Accessibility API (front
//! window title and frame, in points) and the browser URL from `AXDocument`
//! or AppleScript; there is no page-content rectangle, so browsers use the
//! toolbar-inset window.

use std::collections::HashMap;
use sha2::{Digest, Sha256};

/// Physical-px rectangle on the primary monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Region {
    pub fn full(w: i32, h: i32) -> Self {
        Self { x: 0, y: 0, w, h }
    }

    pub fn is_full(&self, mw: i32, mh: i32) -> bool {
        self.x <= 0 && self.y <= 0 && self.w >= mw && self.h >= mh
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YoutubeMeta {
    pub title: String,
    pub author: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Video,
    BrowserPage,
    App,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct ScreenContext {
    pub app: String,
    pub window_title: String,
    /// UTC time when the live foreground OS probe completed.
    pub captured_at_utc: Option<String>,
    /// Privacy-trimmed URL (scheme + host + path; YouTube keeps only the id).
    pub url: Option<String>,
    /// Hash of the complete foreground URL, including query/fragment. This
    /// detects a changed page without putting private URL parts in prompts.
    pub url_fingerprint: Option<[u8; 32]>,
    pub youtube: Option<YoutubeMeta>,
    pub kind: ContentKind,
    pub region: Region,
    /// The region came from a valid OS window/document rectangle rather than
    /// the unknown-target full-monitor fallback.
    pub region_verified: bool,
    /// Region is the full monitor (no crop happened).
    pub full_screen: bool,
    /// Title/URL/app matched the sensitive-window denylist (bank, password
    /// manager, wallet…): the tour refuses instead of capturing.
    pub sensitive: bool,
    /// OCR-visible text lines (compact), injected into the vision prompt as
    /// ground-truth clues so celebrity/creator recognition can cross-reference
    /// on-screen text (Feature 88 v2 / P3).
    pub ocr_lines: Vec<String>,
}

/// What the OS probes returned, before any interpretation.
#[derive(Debug, Clone, Default)]
pub struct RawContext {
    pub app: String,
    pub title: String,
    pub url: Option<String>,
    pub document_rect: Option<(i32, i32, i32, i32)>,
    pub window_rect: Option<(i32, i32, i32, i32)>,
}

pub const MIN_REGION_W: i32 = 200;
pub const MIN_REGION_H: i32 = 150;

// ─── Pure helpers ──────────────────────────────────────────────────────

/// 11-char YouTube video id from a watch / youtu.be / shorts / live URL. Pure.
pub fn youtube_video_id(url: &str) -> Option<String> {
    let lower = url.trim().to_lowercase();
    let no_scheme = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let host_end = no_scheme.find(['/', '?', '#']).unwrap_or(no_scheme.len());
    let host = no_scheme[..host_end].trim_start_matches("www.").trim_start_matches("m.");
    let is_yt = host == "youtube.com" || host == "music.youtube.com";
    let is_short = host == "youtu.be";
    if !is_yt && !is_short {
        return None;
    }
    // Work on the ORIGINAL casing for the id (ids are case-sensitive).
    let orig = url.trim();
    let orig_no_scheme = orig
        .strip_prefix("https://")
        .or_else(|| orig.strip_prefix("http://"))
        .or_else(|| orig.strip_prefix("HTTPS://"))
        .or_else(|| orig.strip_prefix("HTTP://"))
        .unwrap_or(orig);
    let rest = &orig_no_scheme[host_end.min(orig_no_scheme.len())..];
    let valid = |s: &str| -> Option<String> {
        let id: String = s
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if id.len() == 11 {
            Some(id)
        } else {
            None
        }
    };
    if is_short {
        return valid(rest.trim_start_matches('/'));
    }
    for prefix in ["/shorts/", "/live/", "/embed/"] {
        if let Some(p) = rest.to_lowercase().find(prefix) {
            return valid(&rest[p + prefix.len()..]);
        }
    }
    if rest.to_lowercase().starts_with("/watch") {
        let query = rest.split_once('?').map(|x| x.1).unwrap_or("");
        let query = query.split('#').next().unwrap_or("");
        for pair in query.split('&') {
            if let Some(v) = pair.strip_prefix("v=") {
                return valid(v);
            }
        }
    }
    None
}

/// Privacy-trim a page URL before it goes to a cloud model: scheme + host +
/// path only (no query string, fragment or credentials); YouTube collapses
/// to the canonical watch URL with just the video id. Pure.
pub fn sanitize_url(url: &str) -> Option<String> {
    let u = url.trim();
    if u.is_empty() {
        return None;
    }
    if let Some(id) = youtube_video_id(u) {
        return Some(format!("https://www.youtube.com/watch?v={id}"));
    }
    let no_scheme = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .unwrap_or(u);
    let cut = no_scheme.find(['?', '#']).unwrap_or(no_scheme.len());
    let mut clean = no_scheme[..cut].to_string();
    // Drop `user:pass@`.
    if let Some(at) = clean.find('@') {
        if clean[..at].find('/').is_none() {
            clean = clean[at + 1..].to_string();
        }
    }
    if !clean.contains('.') {
        return None; // not a web address (e.g. "about:blank", "chrome://…")
    }
    let out = format!("https://{}", clean.trim_end_matches('/'));
    Some(out.chars().take(200).collect())
}

/// Parse YouTube oEmbed JSON (`{"title": …, "author_name": …}`). Pure.
pub fn parse_oembed(json: &str) -> Option<YoutubeMeta> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let title = v.get("title")?.as_str()?.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let author = v
        .get("author_name")
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    Some(YoutubeMeta { title, author })
}

/// Browser families the macOS probe tells apart: the Chromium family and
/// Safari can report their page URL through AppleScript, Firefox cannot.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacBrowser {
    Chromium,
    Safari,
    Firefox,
}

/// The browser family of a macOS app key (its lower-case bundle id, as the
/// mac probe reports it). The Chromium list is the one `ax_mac` already keeps
/// for asking those browsers for their web tree. Pure.
#[cfg(any(target_os = "macos", test))]
fn mac_browser(key: &str) -> Option<MacBrowser> {
    let key = key.trim().to_ascii_lowercase();
    if crate::ax_mac::wants_enhanced_ui(&key) {
        return Some(MacBrowser::Chromium);
    }
    match key.as_str() {
        "com.apple.safari" | "com.apple.safaritechnologypreview" => Some(MacBrowser::Safari),
        "org.mozilla.firefox" | "org.mozilla.firefoxdeveloperedition" | "org.mozilla.nightly" => {
            Some(MacBrowser::Firefox)
        }
        _ => None,
    }
}

/// A browser holds the window: a Windows browser executable, or on macOS a
/// browser bundle id. Pure.
fn is_browser_app(app: &str) -> bool {
    #[cfg(target_os = "macos")]
    if mac_browser(app).is_some() {
        return true;
    }
    crate::browser_url::is_browser_process(app)
}

/// Pure classification of the foreground content.
pub fn classify_content(app: &str, url: Option<&str>) -> ContentKind {
    if url.map(|u| youtube_video_id(u).is_some()).unwrap_or(false) {
        return ContentKind::Video;
    }
    if is_browser_app(app) {
        return ContentKind::BrowserPage;
    }
    if app.is_empty() {
        ContentKind::Unknown
    } else {
        ContentKind::App
    }
}

/// Intersect (x,y,w,h) with the monitor; None when the result is too small
/// to be a useful crop. Pure.
pub fn clamp_region(x: i32, y: i32, w: i32, h: i32, mw: i32, mh: i32) -> Option<Region> {
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(mw), (y + h).min(mh));
    let (cw, ch) = (x1 - x0, y1 - y0);
    if cw < MIN_REGION_W || ch < MIN_REGION_H {
        None
    } else {
        Some(Region { x: x0, y: y0, w: cw, h: ch })
    }
}

/// Toolbar height guess for a browser whose Document element is not exposed
/// (tab strip + address bar + a little): ~8.5 % of the window height,
/// clamped. Fallback only. Pure.
pub fn toolbar_inset(window_h: i32) -> i32 {
    (((window_h as f32) * 0.085) as i32).clamp(80, 140)
}

/// Pick the capture region. Order: browser page `Document` rect → (browser)
/// window rect minus a toolbar inset / (other app) window rect → full
/// monitor. Anything too small or off-monitor falls through. Pure.
pub fn choose_region(raw: &RawContext, mw: i32, mh: i32) -> Region {
    let is_browser = is_browser_app(&raw.app);
    if let Some((x, y, w, h)) = raw.document_rect {
        if let Some(r) = clamp_region(x, y, w, h, mw, mh) {
            return r;
        }
    }
    if let Some((x, y, w, h)) = raw.window_rect {
        let (y2, h2) = if is_browser {
            let inset = toolbar_inset(h);
            (y + inset, h - inset)
        } else {
            (y, h)
        };
        if let Some(r) = clamp_region(x, y2, w, h2, mw, mh) {
            return r;
        }
    }
    Region::full(mw, mh)
}

/// Sensitive-window check against the live-mode denylist (banks, password
/// managers, wallets). Pure over strings.
pub fn is_sensitive(app: &str, title: &str, url: Option<&str>) -> bool {
    let t = crate::live::safety::is_target_blocked;
    t(app) || t(title) || url.map(t).unwrap_or(false)
}

/// Assemble the context from raw probes (+ optional YouTube metadata). Pure.
pub fn build_context(
    raw: &RawContext,
    youtube: Option<YoutubeMeta>,
    mw: i32,
    mh: i32,
) -> ScreenContext {
    let url = raw.url.as_deref().and_then(sanitize_url);
    let region = choose_region(raw, mw, mh);
    let region_verified = raw.document_rect
        .and_then(|(x, y, w, h)| clamp_region(x, y, w, h, mw, mh))
        .or_else(|| raw.window_rect.and_then(|(x, y, w, h)| {
            clamp_region(x, y, w, h, mw, mh)
        }))
        .is_some();
    ScreenContext {
        app: raw.app.clone(),
        window_title: raw.title.trim().to_string(),
        captured_at_utc: None,
        kind: classify_content(&raw.app, url.as_deref()),
        full_screen: region.is_full(mw, mh),
        region_verified,
        sensitive: is_sensitive(&raw.app, &raw.title, raw.url.as_deref()),
        youtube,
        url,
        url_fingerprint: raw.url.as_deref().map(|u| Sha256::digest(u.as_bytes()).into()),
        region,
        ocr_lines: Vec::new(),
    }
}

/// The macOS probe result as a `RawContext`. The Accessibility frame
/// (fractional global top-left points) becomes the window rectangle in whole
/// points; a non-finite or empty frame is dropped, so the target stays
/// unverified and capture is refused. macOS gives no page-content rectangle
/// here, so a browser falls back to the window minus a toolbar inset. Pure.
#[cfg(any(target_os = "macos", test))]
fn mac_raw_context(
    app: String,
    title: String,
    url: Option<String>,
    frame: Option<(f64, f64, f64, f64)>,
) -> RawContext {
    let window_rect = frame.and_then(|(x, y, w, h)| {
        let sane = [x, y, w, h].iter().all(|v| v.is_finite() && v.abs() < 1.0e6);
        (sane && w >= 1.0 && h >= 1.0)
            .then(|| (x.round() as i32, y.round() as i32, w.round() as i32, h.round() as i32))
    });
    RawContext { app, title, url, document_rect: None, window_rect }
}

/// A page URL as a macOS browser reported it, kept only when it is a plain
/// http(s) address: not `favorites://`, a blank tab or AppleScript error text.
/// Pure.
#[cfg(any(target_os = "macos", test))]
fn mac_page_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    let lower = url.to_ascii_lowercase();
    let web = lower.starts_with("https://") || lower.starts_with("http://");
    (web && url.len() <= 4096 && !url.chars().any(char::is_whitespace)).then(|| url.to_string())
}

/// AppleScript returning the page URL of the front window of the browser with
/// `bundle_id`. The browser is addressed by its own id, so only the one in
/// front is asked and nothing is launched. The id is part of the script text
/// (AppleScript needs the literal to load that browser's terms), so only a
/// plain bundle id is accepted. Firefox has no scriptable URL. Pure.
#[cfg(any(target_os = "macos", test))]
fn mac_url_script(browser: MacBrowser, bundle_id: &str) -> Option<String> {
    let plain = !bundle_id.is_empty()
        && bundle_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    let tab = match browser {
        MacBrowser::Chromium => "active tab",
        MacBrowser::Safari => "current tab",
        MacBrowser::Firefox => return None,
    };
    plain.then(|| format!("tell application id \"{bundle_id}\" to return URL of {tab} of front window"))
}

/// A screen may be captured only when the foreground probe identified a
/// concrete, non-sensitive target. Probe failure must not become permission to
/// upload an unidentified full-monitor image.
///
/// The page URL is NOT part of that bar. A browser address bar is
/// occasionally unreadable (an internal page such as `chrome://settings`, a
/// busy renderer, a UI Automation tree that does not expose the field), and
/// the URL probe alone must never refuse the analysis of a window whose app,
/// title and region are known. A desktop application has no URL at all.
pub fn capture_block_reason(ctx: &ScreenContext) -> Option<&'static str> {
    if ctx.sensitive {
        Some("This window is sensitive, so I won't analyse it.")
    } else if ctx.app.trim().is_empty() || ctx.window_title.trim().is_empty()
        || !ctx.region_verified
    {
        Some("I couldn't identify the foreground window. Please try again.")
    } else {
        None
    }
}

fn same_capture_target(expected: &ScreenContext, current: &ScreenContext) -> bool {
    capture_block_reason(current).is_none()
        && expected.app == current.app
        && expected.window_title == current.window_title
        // A missing URL in both probes is valid. If it appears or disappears
        // between probes, the target is uncertain and capture must retry.
        && expected.url == current.url
        && expected.url_fingerprint == current.url_fingerprint
        && expected.region == current.region
}

/// Recheck focus immediately before a capture. This has no OCR or network
/// side effects; a timed-out probe fails closed.
pub async fn foreground_still_matches(expected: &ScreenContext, mw: i32, mh: i32) -> bool {
    let Ok(Ok(raw)) = tokio::time::timeout(
        std::time::Duration::from_millis(3500),
        tokio::task::spawn_blocking(probe_blocking),
    ).await else {
        return false;
    };
    same_capture_target(expected, &build_context(&raw, None, mw, mh))
}

/// Compact the WinRT OCR text into distinct non-empty lines for the prompt
/// (bounded — the prompt must stay small). Pure + unit-tested.
pub fn compact_ocr_lines(text: &str, max_lines: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || out.iter().any(|s| s == line) {
            continue;
        }
        out.push(line.chars().take(90).collect());
        if out.len() >= max_lines {
            break;
        }
    }
    out
}

impl ScreenContext {
    /// Empty context (probes failed): full screen, nothing known.
    #[allow(dead_code)]
    pub fn unknown(mw: i32, mh: i32) -> Self {
        build_context(&RawContext::default(), None, mw, mh)
    }

    /// The block inserted into the model prompt. Never contains the raw
    /// (un-trimmed) URL.
    pub fn prompt_block(&self) -> String {
        let mut lines = vec![
            "Context about what the user is looking at (from the operating system — trust it over guesses):"
                .to_string(),
        ];
        if let Some(captured) = &self.captured_at_utc {
            lines.push(format!("- Evidence source: live foreground OS probe, captured at {captured}."));
        }
        if !self.app.is_empty() {
            lines.push(format!("- App: {}", self.app));
        }
        if !self.window_title.is_empty() {
            lines.push(format!("- Window / tab title: {}", self.window_title));
        }
        if let Some(u) = &self.url {
            lines.push(format!("- Page URL: {u}"));
        } else if matches!(self.kind, ContentKind::BrowserPage | ContentKind::Video) {
            // Be honest instead of letting the model invent a URL.
            lines.push(
                "- Page URL: not exposed by the browser — identify the page from what is visible only."
                    .to_string(),
            );
        }
        if let Some(y) = &self.youtube {
            lines.push(format!("- YouTube video title: {}", y.title));
            if !y.author.is_empty() {
                lines.push(format!("- YouTube channel: {}", y.author));
            }
        }
        lines.push(match self.kind {
            ContentKind::Video => "- Kind: a video page (use the title/channel to say what it is about).".to_string(),
            ContentKind::BrowserPage => "- Kind: a web page.".to_string(),
            ContentKind::App => "- Kind: a desktop application.".to_string(),
            ContentKind::Unknown => "- Kind: unknown.".to_string(),
        });
        lines.push(if self.full_screen {
            "- The image is the full screen.".to_string()
        } else {
            "- The image shows ONLY the content area (browser tabs, address bar and the taskbar are cropped out).".to_string()
        });
        // Feature 98: ground-truth text from local OCR — the model can
        // cross-reference visible names/handles when naming public figures.
        if !self.ocr_lines.is_empty() {
            lines.push("[On-Screen Context Clues from OS & OCR]:".to_string());
            for line in &self.ocr_lines {
                lines.push(format!("- \"{line}\""));
            }
        }
        lines.join("\n")
    }
}

// ─── Probes + oEmbed (thin glue) ───────────────────────────────────────

static OEMBED_CACHE: once_cell::sync::Lazy<parking_lot::Mutex<HashMap<String, YoutubeMeta>>> =
    once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));

/// Blocking OS probes (UI Automation can take a moment — call from
/// `spawn_blocking`).
#[cfg(target_os = "windows")]
fn probe_blocking() -> RawContext {
    let app = crate::browser_url::foreground_process_name().unwrap_or_default();
    let title = crate::browser_url::foreground_window_title().unwrap_or_default();
    let is_browser = crate::browser_url::is_browser_process(&app);
    let url = if is_browser {
        // Verified probe: anchored to the EXACT foreground window and read
        // from that browser's own address bar. A missing URL is fine — the
        // capture policy no longer depends on it.
        crate::browser_url::get_verified_active_browser_url()
    } else {
        None
    };
    let document_rect = if is_browser {
        crate::browser_url::browser_document_rect()
    } else {
        None
    };
    RawContext {
        app,
        title,
        url,
        document_rect,
        window_rect: crate::browser_url::foreground_window_rect(),
    }
}

/// macOS probes, the counterpart of the UI Automation ones above. The front
/// app comes from `lsappinfo`, keyed by its lower-case bundle id (the key the
/// safety denylist and `browser_url::foreground_process_name` use). Its
/// focused window's title and frame come from the Accessibility API, which
/// never prompts: without that permission, or with NEXUS itself in front, the
/// probe is empty and capture is refused. The frame is in global top-left
/// points, the space `screen_capture_mac` captures in.
#[cfg(target_os = "macos")]
fn probe_blocking() -> RawContext {
    use crate::ax_mac::mac;
    let Ok((front, _app, window)) = mac::front_window() else {
        return RawContext::default();
    };
    let Some(node) = mac::read_node(&window, &mac::names(mac::NODE_ATTRS)) else {
        return RawContext::default();
    };
    let key = front.key();
    let url = mac_browser(&key)
        .and_then(|browser| mac_browser_url(browser, &window, front.bundle_id.as_deref()));
    mac_raw_context(key, node.title, url, node.frame.map(|f| (f.x, f.y, f.w, f.h)))
}

/// The front browser window's page URL on macOS. The window's `AXDocument`
/// (Safari reports the page there) needs no further permission. Otherwise
/// Safari and the Chromium family are asked through AppleScript, which shows
/// macOS's Automation prompt for that browser the first time. While the prompt
/// is open the read times out, and once it is refused the read fails at once;
/// either way the URL stays unknown, which capture does not depend on.
#[cfg(target_os = "macos")]
fn mac_browser_url(
    browser: MacBrowser,
    window: &crate::ax_mac::mac::Owned,
    bundle_id: Option<&str>,
) -> Option<String> {
    use crate::ax_mac::mac;
    // SAFETY: `string_of` checks the value's type; `v` is held while it is read.
    let document = mac::copy_attr(window, "AXDocument").map(|v| unsafe { mac::string_of(v.0) });
    if let Some(url) = document.as_deref().and_then(mac_page_url) {
        return Some(url);
    }
    scripted_url(browser, bundle_id?)
}

/// The page URL of the front window of the browser with `bundle_id`, asked through AppleScript by its own id (nothing
/// launches) with a bounded wait (an open Automation prompt must not hang the caller). None for a non-browser app.
#[cfg(target_os = "macos")]
pub(crate) fn mac_front_browser_url(bundle_id: &str) -> Option<String> {
    scripted_url(mac_browser(bundle_id)?, bundle_id)
}

#[cfg(target_os = "macos")]
fn scripted_url(browser: MacBrowser, bundle_id: &str) -> Option<String> {
    let script = mac_url_script(browser, bundle_id)?;
    let out = crate::macos_foreground::run_capture(
        std::process::Command::new("osascript").args(["-e", &script]),
        std::time::Duration::from_millis(1200),
    )?;
    mac_page_url(&out)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn probe_blocking() -> RawContext {
    RawContext::default()
}

async fn fetch_oembed(video_id: &str) -> Option<YoutubeMeta> {
    if let Some(hit) = OEMBED_CACHE.lock().get(video_id).cloned() {
        return Some(hit);
    }
    let watch = format!("https://www.youtube.com/watch?v={video_id}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;
    let resp = client
        .get("https://www.youtube.com/oembed")
        .query(&[("url", watch.as_str()), ("format", "json")])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let meta = parse_oembed(&resp.text().await.ok()?)?;
    let mut cache = OEMBED_CACHE.lock();
    if cache.len() > 64 {
        cache.clear();
    }
    cache.insert(video_id.to_string(), meta.clone());
    Some(meta)
}

/// Freeze the foreground target before any acknowledgement or NEXUS window
/// can affect focus. No OCR, screenshot, or network work runs here.
pub async fn collect_target(mw: i32, mh: i32) -> ScreenContext {
    let raw = match tokio::time::timeout(
        std::time::Duration::from_millis(3500),
        tokio::task::spawn_blocking(probe_blocking),
    )
    .await
    {
        Ok(Ok(raw)) => raw,
        _ => {
            tracing::warn!("screen_context: probes timed out — refusing unidentified capture");
            RawContext::default()
        }
    };
    let mut context = build_context(&raw, None, mw, mh);
    context.captured_at_utc = Some(chrono::Utc::now().to_rfc3339());
    context
}

/// Feature 98 P3 text clues from local OCR: WinRT OCR on Windows, the Vision
/// framework on macOS (`ocr_mac`, mounted as `crate::ocr`). Elsewhere the
/// vision model proceeds without local text grounding.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn capture_ocr_clue_rects(
    x: i32, y: i32, w: i32, h: i32,
) -> Option<(String, Vec<(i32, i32, i32, i32)>)> {
    crate::ocr::capture_region_text_rects(x, y, w, h)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn capture_ocr_clue_rects(
    _x: i32, _y: i32, _w: i32, _h: i32,
) -> Option<(String, Vec<(i32, i32, i32, i32)>)> {
    None
}

/// Add public video metadata and local OCR clues after the short spoken
/// acknowledgement. Every capture remains scoped to the frozen target.
pub async fn enrich_target(mut ctx: ScreenContext, mw: i32, mh: i32) -> ScreenContext {
    if capture_block_reason(&ctx).is_some()
        || !foreground_still_matches(&ctx, mw, mh).await
    {
        return ctx;
    }
    if let Some(id) = ctx.url.as_deref().and_then(youtube_video_id) {
        ctx.youtube = fetch_oembed(&id).await;
    }
    // Feature 98 P3: ground-truth text clues from local WinRT OCR (fast,
    // free, ~25ms) so the vision model can cross-reference on-screen text
    // (names, handles, chyrons) for public-figure recognition.
    let ocr = if foreground_still_matches(&ctx, mw, mh).await {
        let region = ctx.region;
        tokio::time::timeout(
            std::time::Duration::from_millis(1200),
            tokio::task::spawn_blocking(move || {
                capture_ocr_clue_rects(
                    region.x, region.y, region.w, region.h,
                )
            }),
        )
        .await
        .ok()
        .and_then(|r| r.ok())
        .and_then(|r| r)
    } else {
        None
    };
    ctx.ocr_lines = match ocr {
        Some((text, _)) => compact_ocr_lines(&text, 10),
        None => Vec::new(),
    };
    tracing::info!(
        "screen_context: app='{}' kind={:?} region={:?} full={} youtube={} sensitive={}",
        ctx.app,
        ctx.kind,
        ctx.region,
        ctx.full_screen,
        ctx.youtube.is_some(),
        ctx.sensitive
    );
    ctx
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "dQw4w9WgXcQ";

    #[test]
    fn youtube_ids_from_every_url_shape() {
        for u in [
            format!("https://www.youtube.com/watch?v={ID}"),
            format!("https://youtube.com/watch?feature=share&v={ID}&t=42s"),
            format!("https://m.youtube.com/watch?v={ID}#frag"),
            format!("youtube.com/watch?v={ID}"),
            format!("https://youtu.be/{ID}?si=abc"),
            format!("https://www.youtube.com/shorts/{ID}"),
            format!("https://www.youtube.com/live/{ID}?feature=share"),
            format!("https://music.youtube.com/watch?v={ID}&list=PL1"),
        ] {
            assert_eq!(youtube_video_id(&u).as_deref(), Some(ID), "url: {u}");
        }
    }

    #[test]
    fn youtube_id_rejects_non_video_urls() {
        for u in [
            "https://www.youtube.com/",
            "https://www.youtube.com/feed/subscriptions",
            "https://www.youtube.com/watch?v=short",
            "https://example.com/watch?v=dQw4w9WgXcQ",
            "https://notyoutube.com/watch?v=dQw4w9WgXcQ",
            "",
        ] {
            assert_eq!(youtube_video_id(u), None, "url: {u}");
        }
    }

    #[test]
    fn sanitize_drops_query_fragment_and_credentials() {
        assert_eq!(
            sanitize_url("https://www.womenshealthmag.com/uk/food/healthy-eating/g63752621/healthiest-nuts/?utm_source=x&token=abc#top").as_deref(),
            Some("https://www.womenshealthmag.com/uk/food/healthy-eating/g63752621/healthiest-nuts")
        );
        assert_eq!(
            sanitize_url("womenshealthmag.com/uk/food/healthiest-nuts/").as_deref(),
            Some("https://womenshealthmag.com/uk/food/healthiest-nuts")
        );
        assert_eq!(
            sanitize_url("https://user:pw@example.com/a?b=1").as_deref(),
            Some("https://example.com/a")
        );
        // YouTube collapses to the canonical id-only URL.
        assert_eq!(
            sanitize_url(&format!("https://www.youtube.com/watch?v={ID}&list=PLsecret&t=9")).as_deref(),
            Some(format!("https://www.youtube.com/watch?v={ID}").as_str())
        );
        assert_eq!(sanitize_url(""), None);
        assert_eq!(sanitize_url("about:blank"), None);
        assert_eq!(sanitize_url("chrome://settings"), None);
    }

    #[test]
    fn oembed_parsing() {
        let m = parse_oembed(r#"{"title":"Never Gonna Give You Up","author_name":"Rick Astley","type":"video"}"#).unwrap();
        assert_eq!(m.title, "Never Gonna Give You Up");
        assert_eq!(m.author, "Rick Astley");
        assert_eq!(parse_oembed(r#"{"title":"T"}"#).unwrap().author, "");
        assert!(parse_oembed(r#"{"title":"  "}"#).is_none());
        assert!(parse_oembed("not json").is_none());
        assert!(parse_oembed(r#"{"author_name":"x"}"#).is_none());
    }

    #[test]
    fn classification() {
        assert_eq!(
            classify_content("brave.exe", Some(&format!("https://www.youtube.com/watch?v={ID}"))),
            ContentKind::Video
        );
        assert_eq!(classify_content("brave.exe", Some("https://example.com/a")), ContentKind::BrowserPage);
        assert_eq!(classify_content("brave.exe", None), ContentKind::BrowserPage);
        assert_eq!(classify_content("code.exe", None), ContentKind::App);
        assert_eq!(classify_content("", None), ContentKind::Unknown);
    }

    #[test]
    fn clamp_region_intersects_and_rejects_tiny() {
        // Maximised window overhanging the monitor by the 8-px frame.
        assert_eq!(
            clamp_region(-8, -8, 1936, 1096, 1920, 1080),
            Some(Region { x: 0, y: 0, w: 1920, h: 1080 })
        );
        assert_eq!(
            clamp_region(0, 82, 1920, 950, 1920, 1080),
            Some(Region { x: 0, y: 82, w: 1920, h: 950 })
        );
        assert_eq!(clamp_region(0, 0, 100, 100, 1920, 1080), None); // too small
        assert_eq!(clamp_region(2000, 0, 500, 500, 1920, 1080), None); // off monitor
        assert_eq!(clamp_region(1800, 900, 500, 500, 1920, 1080), None); // sliver
    }

    #[test]
    fn region_prefers_document_rect() {
        let raw = RawContext {
            app: "brave.exe".into(),
            document_rect: Some((0, 82, 1920, 950)),
            window_rect: Some((0, 0, 1920, 1032)),
            ..Default::default()
        };
        assert_eq!(choose_region(&raw, 1920, 1080), Region { x: 0, y: 82, w: 1920, h: 950 });
    }

    #[test]
    fn browser_without_document_rect_uses_inset_window() {
        let raw = RawContext {
            app: "chrome.exe".into(),
            window_rect: Some((0, 0, 1920, 1040)),
            ..Default::default()
        };
        let r = choose_region(&raw, 1920, 1080);
        assert_eq!(r.y, toolbar_inset(1040));
        assert_eq!(r.y + r.h, 1040);
        assert!(r.y >= 80 && r.y <= 140);
    }

    #[test]
    fn non_browser_uses_window_rect_and_unknown_uses_full() {
        let raw = RawContext {
            app: "code.exe".into(),
            window_rect: Some((100, 50, 1200, 800)),
            ..Default::default()
        };
        assert_eq!(choose_region(&raw, 1920, 1080), Region { x: 100, y: 50, w: 1200, h: 800 });
        let none = RawContext::default();
        assert_eq!(choose_region(&none, 1920, 1080), Region::full(1920, 1080));
        // Bad probe data (tiny rect) also falls back to full screen.
        let tiny = RawContext {
            app: "code.exe".into(),
            window_rect: Some((0, 0, 50, 50)),
            ..Default::default()
        };
        assert_eq!(choose_region(&tiny, 1920, 1080), Region::full(1920, 1080));
    }

    #[test]
    fn toolbar_inset_scales_and_clamps() {
        assert_eq!(toolbar_inset(400), 80);
        assert_eq!(toolbar_inset(1000), 85);
        assert_eq!(toolbar_inset(4000), 140);
    }

    #[test]
    fn sensitive_windows_are_flagged() {
        assert!(is_sensitive("brave.exe", "Chase Bank — Sign in", None));
        assert!(is_sensitive("1password.exe", "", None));
        assert!(is_sensitive("brave.exe", "x", Some("https://www.paypal.com/myaccount")));
        assert!(!is_sensitive("brave.exe", "Top 10 healthiest nuts | Women's Health", Some("https://www.womenshealthmag.com/uk/food")));
    }

    #[test]
    fn context_assembly_for_the_nuts_page() {
        let raw = RawContext {
            app: "brave.exe".into(),
            title: "Top 10 healthiest nuts | Women's Health".into(),
            url: Some("womenshealthmag.com/uk/food/healthy-eating/g63752621/healthiest-nuts/".into()),
            document_rect: Some((0, 82, 1920, 950)),
            window_rect: Some((0, 0, 1920, 1032)),
        };
        let ctx = build_context(&raw, None, 1920, 1080);
        assert_eq!(ctx.kind, ContentKind::BrowserPage);
        assert!(!ctx.full_screen && !ctx.sensitive);
        assert_eq!(ctx.region, Region { x: 0, y: 82, w: 1920, h: 950 });
        let block = ctx.prompt_block();
        assert!(block.contains("Top 10 healthiest nuts"));
        assert!(block.contains("https://womenshealthmag.com/uk/food/healthy-eating/g63752621/healthiest-nuts"));
        assert!(block.contains("cropped out"));
    }

    #[test]
    fn context_for_a_video_includes_title_and_channel_but_not_tracking() {
        let raw = RawContext {
            app: "brave.exe".into(),
            title: "Some Video - YouTube".into(),
            url: Some(format!("https://www.youtube.com/watch?v={ID}&list=PLsecret&si=trk")),
            ..Default::default()
        };
        let meta = YoutubeMeta { title: "Never Gonna Give You Up".into(), author: "Rick Astley".into() };
        let ctx = build_context(&raw, Some(meta), 1920, 1080);
        assert_eq!(ctx.kind, ContentKind::Video);
        let block = ctx.prompt_block();
        assert!(block.contains("YouTube video title: Never Gonna Give You Up"));
        assert!(block.contains("YouTube channel: Rick Astley"));
        assert!(!block.contains("PLsecret") && !block.contains("trk"));
        assert!(block.contains("full screen")); // no rects → full capture
    }

    #[test]
    fn unknown_context_is_full_screen_and_harmless() {
        let ctx = ScreenContext::unknown(1920, 1080);
        assert!(ctx.full_screen && !ctx.sensitive);
        assert!(!ctx.region_verified);
        assert!(capture_block_reason(&ctx).is_some());
        assert_eq!(ctx.kind, ContentKind::Unknown);
        assert!(ctx.prompt_block().contains("unknown"));
    }

    #[test]
    fn capture_requires_a_stable_bounded_nonsensitive_target() {
        let raw = RawContext {
            app: "brave.exe".into(),
            title: "Example".into(),
            url: Some("https://example.com/".into()),
            window_rect: Some((10, 20, 900, 700)),
            ..Default::default()
        };
        let expected = build_context(&raw, None, 1920, 1080);
        assert!(expected.region_verified);
        assert_eq!(capture_block_reason(&expected), None);
        assert!(same_capture_target(&expected, &expected));

        let mut changed = raw.clone();
        changed.title = "Another tab".into();
        assert!(!same_capture_target(&expected, &build_context(&changed, None, 1920, 1080)));
        changed = raw.clone();
        changed.url = Some("https://example.com/other".into());
        assert!(!same_capture_target(&expected, &build_context(&changed, None, 1920, 1080)));
        changed = raw.clone();
        changed.url = Some("https://example.com/?private=other".into());
        let query_changed = build_context(&changed, None, 1920, 1080);
        assert_eq!(expected.url, query_changed.url);
        assert!(!same_capture_target(&expected, &query_changed));
        changed = raw.clone();
        changed.window_rect = Some((25, 20, 900, 700));
        assert!(!same_capture_target(&expected, &build_context(&changed, None, 1920, 1080)));
        changed = raw.clone();
        changed.window_rect = None;
        assert!(capture_block_reason(&build_context(&changed, None, 1920, 1080)).is_some());
        changed = raw;
        changed.title = "Bank account sign in".into();
        assert!(capture_block_reason(&build_context(&changed, None, 1920, 1080)).is_some());
    }

    // ─── The URL is optional for capture ────────────────────────────────

    #[test]
    fn brave_without_a_url_is_still_capturable() {
        // The address bar can be unreadable (internal page, busy renderer,
        // a UI Automation tree that does not expose the field). The window
        // is still identified: app, title and a verified region.
        let raw = RawContext {
            app: "brave.exe".into(),
            title: "Brave — Settings".into(),
            url: None,
            document_rect: Some((0, 82, 1920, 950)),
            window_rect: Some((0, 0, 1920, 1032)),
        };
        let ctx = build_context(&raw, None, 1920, 1080);
        assert_eq!(ctx.kind, ContentKind::BrowserPage);
        assert!(ctx.url.is_none());
        assert!(ctx.region_verified);
        assert!(!ctx.full_screen);
        assert_eq!(capture_block_reason(&ctx), None);
        assert!(same_capture_target(&ctx, &ctx));
        // The model is told the URL is unknown instead of being left to guess.
        assert!(ctx.prompt_block().contains("not exposed by the browser"));
    }

    #[test]
    fn desktop_app_without_a_url_is_still_capturable() {
        let raw = RawContext {
            app: "code.exe".into(),
            title: "main.rs — nexus".into(),
            url: None,
            document_rect: None,
            window_rect: Some((100, 50, 1200, 800)),
        };
        let ctx = build_context(&raw, None, 1920, 1080);
        assert_eq!(ctx.kind, ContentKind::App);
        assert!(ctx.url.is_none());
        assert!(ctx.region_verified);
        assert_eq!(capture_block_reason(&ctx), None);
        assert!(same_capture_target(&ctx, &ctx));
    }

    #[test]
    fn missing_app_title_or_region_still_denies() {
        let mut raw = RawContext {
            app: "brave.exe".into(),
            title: "Example".into(),
            url: None,
            document_rect: None,
            window_rect: Some((10, 20, 900, 700)),
        };
        assert_eq!(capture_block_reason(&build_context(&raw, None, 1920, 1080)), None);

        let mut no_app = raw.clone();
        no_app.app = String::new();
        assert!(capture_block_reason(&build_context(&no_app, None, 1920, 1080)).is_some());

        let mut no_title = raw.clone();
        no_title.title = "   ".into();
        assert!(capture_block_reason(&build_context(&no_title, None, 1920, 1080)).is_some());

        let mut no_region = raw.clone();
        no_region.window_rect = None;
        assert!(capture_block_reason(&build_context(&no_region, None, 1920, 1080)).is_some());

        raw.window_rect = Some((0, 0, 50, 50)); // too small to be a real crop
        assert!(capture_block_reason(&build_context(&raw, None, 1920, 1080)).is_some());
    }

    #[test]
    fn sensitive_window_still_denies_without_a_url() {
        let raw = RawContext {
            app: "brave.exe".into(),
            title: "Chase Bank — Sign in".into(),
            url: None,
            document_rect: None,
            window_rect: Some((10, 20, 900, 700)),
        };
        let ctx = build_context(&raw, None, 1920, 1080);
        assert!(ctx.sensitive);
        assert!(capture_block_reason(&ctx).is_some());
        assert!(!same_capture_target(&ctx, &ctx));

        let wallet = RawContext {
            app: "1password.exe".into(),
            title: "Vault".into(),
            url: None,
            document_rect: None,
            window_rect: Some((10, 20, 900, 700)),
        };
        assert!(capture_block_reason(&build_context(&wallet, None, 1920, 1080)).is_some());
    }

    #[test]
    fn changed_title_or_region_still_fails_the_recheck() {
        let raw = RawContext {
            app: "brave.exe".into(),
            title: "Example".into(),
            url: None,
            document_rect: Some((0, 82, 1920, 950)),
            window_rect: Some((0, 0, 1920, 1032)),
        };
        let expected = build_context(&raw, None, 1920, 1080);

        let mut changed = raw.clone();
        changed.title = "Another tab".into();
        assert!(!same_capture_target(&expected, &build_context(&changed, None, 1920, 1080)));

        changed = raw.clone();
        changed.document_rect = Some((0, 82, 1900, 900));
        changed.window_rect = Some((0, 0, 1900, 1032));
        assert!(!same_capture_target(&expected, &build_context(&changed, None, 1920, 1080)));

        changed = raw.clone();
        changed.app = "msedge.exe".into();
        assert!(!same_capture_target(&expected, &build_context(&changed, None, 1920, 1080)));
    }

    #[test]
    fn an_unreadable_address_bar_requires_a_retry_if_probe_results_change() {
        // When only one probe resolves the URL, a tab change cannot be ruled
        // out. A consistently unreadable URL is still capturable.
        let raw = RawContext {
            app: "brave.exe".into(),
            title: "Example".into(),
            url: Some("https://example.com/".into()),
            document_rect: Some((0, 82, 1920, 950)),
            window_rect: Some((0, 0, 1920, 1032)),
        };
        let with_url = build_context(&raw, None, 1920, 1080);

        let mut without_url = raw.clone();
        without_url.url = None;
        let without = build_context(&without_url, None, 1920, 1080);
        assert!(!same_capture_target(&with_url, &without));
        assert!(!same_capture_target(&without, &with_url));
        assert!(same_capture_target(&without, &without));

        // Two DIFFERENT urls are still a change of target.
        let mut other = raw.clone();
        other.url = Some("https://example.com/other".into());
        assert!(!same_capture_target(&with_url, &build_context(&other, None, 1920, 1080)));
    }

    // ─── macOS probe → context ──────────────────────────────────────────
    // A 14-inch MacBook's main display in points; the menu bar is 37 pt tall.
    const MW: i32 = 1512;
    const MH: i32 = 982;

    #[test]
    fn mac_window_frame_becomes_a_verified_capture_region() {
        let raw = mac_raw_context(
            "com.apple.textedit".into(),
            "notes.txt".into(),
            None,
            Some((100.4, 37.6, 900.2, 700.0)),
        );
        let ctx = build_context(&raw, None, MW, MH);
        assert_eq!(ctx.region, Region { x: 100, y: 38, w: 900, h: 700 });
        assert!(ctx.region_verified && !ctx.full_screen && !ctx.sensitive);
        assert_eq!(ctx.kind, ContentKind::App);
        assert_eq!(capture_block_reason(&ctx), None);
        assert!(same_capture_target(&ctx, &ctx));
    }

    #[test]
    fn mac_missing_or_broken_frames_refuse_the_capture() {
        for frame in [
            None,
            Some((f64::NAN, 0.0, 800.0, 600.0)),
            Some((0.0, 0.0, f64::INFINITY, 600.0)),
            Some((10.0, 40.0, 0.0, 600.0)),
            Some((10.0, 40.0, 800.0, 0.4)),
            Some((0.0, 0.0, 1.0e9, 600.0)),
        ] {
            let raw = mac_raw_context("com.apple.textedit".into(), "a.txt".into(), None, frame);
            assert_eq!(raw.window_rect, None, "frame {frame:?}");
            assert!(capture_block_reason(&build_context(&raw, None, MW, MH)).is_some(), "frame {frame:?}");
        }
        // The probe found nothing at all (no Accessibility, NEXUS in front).
        assert!(capture_block_reason(&build_context(&RawContext::default(), None, MW, MH)).is_some());
    }

    #[test]
    fn mac_window_on_a_second_display_is_not_captured_from_the_main_one() {
        // Entirely right of the main display: nothing of it is on the captured display.
        let away = mac_raw_context("com.apple.textedit".into(), "a.txt".into(), None, Some((1600.0, 100.0, 800.0, 600.0)));
        assert!(capture_block_reason(&build_context(&away, None, MW, MH)).is_some());
        // Straddling the edge: only the part on the main display is captured.
        let edge = mac_raw_context("com.apple.textedit".into(), "a.txt".into(), None, Some((1100.0, 100.0, 800.0, 600.0)));
        assert_eq!(build_context(&edge, None, MW, MH).region, Region { x: 1100, y: 100, w: 412, h: 600 });
    }

    #[test]
    fn mac_sensitive_apps_are_refused_by_bundle_id() {
        for key in ["com.1password.1password", "com.agilebits.onepassword7", "com.apple.keychainaccess", "com.apple.passwords"] {
            let raw = mac_raw_context(key.into(), "Vault".into(), None, Some((10.0, 40.0, 900.0, 700.0)));
            let ctx = build_context(&raw, None, MW, MH);
            assert!(ctx.sensitive, "{key}");
            assert!(capture_block_reason(&ctx).is_some(), "{key}");
        }
        let bank = mac_raw_context(
            "com.apple.safari".into(),
            "Example".into(),
            Some("https://www.paypal.com/myaccount".into()),
            Some((10.0, 40.0, 900.0, 700.0)),
        );
        assert!(build_context(&bank, None, MW, MH).sensitive);
    }

    #[test]
    fn mac_browser_families() {
        for (key, want) in [
            ("com.google.chrome", Some(MacBrowser::Chromium)),
            ("com.brave.browser", Some(MacBrowser::Chromium)),
            ("com.microsoft.edgemac", Some(MacBrowser::Chromium)),
            ("company.thebrowser.browser", Some(MacBrowser::Chromium)),
            ("com.apple.Safari", Some(MacBrowser::Safari)),
            ("org.mozilla.firefox", Some(MacBrowser::Firefox)),
            ("com.apple.textedit", None),
            ("chrome.exe", None),
            ("", None),
        ] {
            assert_eq!(mac_browser(key), want, "{key}");
        }
    }

    #[test]
    fn mac_page_url_keeps_only_web_addresses() {
        assert_eq!(mac_page_url("https://example.com/a?b=1\n").as_deref(), Some("https://example.com/a?b=1"));
        assert_eq!(mac_page_url("HTTP://Example.com").as_deref(), Some("HTTP://Example.com"));
        for bad in ["", "favorites://", "about:blank", "file:///Users/me/notes.txt", "missing value",
                    "execution error: Not authorized to send Apple events (-1743)", "https://a.com/x y"] {
            assert_eq!(mac_page_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn mac_url_script_asks_only_the_front_browser_by_its_own_id() {
        assert_eq!(
            mac_url_script(MacBrowser::Chromium, "com.google.Chrome").as_deref(),
            Some("tell application id \"com.google.Chrome\" to return URL of active tab of front window")
        );
        assert_eq!(
            mac_url_script(MacBrowser::Safari, "com.apple.Safari").as_deref(),
            Some("tell application id \"com.apple.Safari\" to return URL of current tab of front window")
        );
        assert_eq!(mac_url_script(MacBrowser::Firefox, "org.mozilla.firefox"), None);
        // The id is script text: anything but a plain bundle id is refused.
        for bad in ["", "com.evil\" to do shell script \"x", "com.google.chrome\nend", "com google"] {
            assert_eq!(mac_url_script(MacBrowser::Chromium, bad), None, "{bad:?}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_browser_window_is_a_page_cropped_below_the_toolbar() {
        let raw = mac_raw_context(
            "com.google.chrome".into(),
            "Top 10 healthiest nuts | Women's Health".into(),
            Some("https://www.womenshealthmag.com/uk/food/?utm_source=x".into()),
            Some((0.0, 37.0, 1512.0, 945.0)),
        );
        let ctx = build_context(&raw, None, MW, MH);
        assert_eq!(ctx.kind, ContentKind::BrowserPage);
        let inset = toolbar_inset(945);
        assert_eq!(ctx.region, Region { x: 0, y: 37 + inset, w: 1512, h: 945 - inset });
        assert_eq!(capture_block_reason(&ctx), None);
        let block = ctx.prompt_block();
        assert!(block.contains("App: com.google.chrome"));
        assert!(block.contains("https://www.womenshealthmag.com/uk/food") && !block.contains("utm_source"));

        let video = mac_raw_context(
            "com.apple.safari".into(),
            "Some Video - YouTube".into(),
            Some(format!("https://www.youtube.com/watch?v={ID}&list=PLsecret")),
            Some((0.0, 37.0, 1512.0, 945.0)),
        );
        assert_eq!(build_context(&video, None, MW, MH).kind, ContentKind::Video);

        // A browser whose URL could not be read is still capturable.
        let no_url = mac_raw_context("org.mozilla.firefox".into(), "Mozilla Firefox".into(), None, Some((0.0, 37.0, 1512.0, 945.0)));
        let ctx = build_context(&no_url, None, MW, MH);
        assert_eq!(ctx.kind, ContentKind::BrowserPage);
        assert_eq!(capture_block_reason(&ctx), None);
        assert!(ctx.prompt_block().contains("not exposed by the browser"));
    }

    #[test]
    fn mac_recheck_catches_a_new_tab_a_moved_window_or_another_app() {
        let frame = Some((0.0, 37.0, 1512.0, 945.0));
        let url = Some("https://example.com/".to_string());
        let expected = build_context(
            &mac_raw_context("com.google.chrome".into(), "Example".into(), url.clone(), frame),
            None, MW, MH,
        );
        let again = build_context(
            &mac_raw_context("com.google.chrome".into(), "Example".into(), url.clone(), frame),
            None, MW, MH,
        );
        assert!(same_capture_target(&expected, &again));
        let changes = [
            mac_raw_context("com.google.chrome".into(), "Other tab".into(), Some("https://example.com/other".into()), frame),
            mac_raw_context("com.google.chrome".into(), "Example".into(), url.clone(), Some((40.0, 37.0, 1200.0, 900.0))),
            mac_raw_context("com.apple.safari".into(), "Example".into(), url, frame),
        ];
        for raw in changes {
            assert!(!same_capture_target(&expected, &build_context(&raw, None, MW, MH)), "{raw:?}");
        }
    }
}
