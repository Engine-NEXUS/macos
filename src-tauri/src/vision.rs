//! Vision grounding fallback for Ghost Mode (C1).
//!
//! Grounding order: UIA bounds (exact, free) → vision model (Groq,
//! screenshot + 0-1000 coords). Vision runs ONLY when UIA misses, so
//! canvas/custom UI clicks work without paying VLM latency on every
//! click. Fail-open everywhere: no key / capture fail / parse fail → None.

use tauri::{Manager, Runtime};

/// Groq vision-capable models, fastest first. IDs churn — fallbacks matter.
pub const GROQ_VISION_MODELS: &[&str] = &[
    "meta-llama/llama-4-scout-17b-16e-instruct",
    "meta-llama/llama-4-maverick-17b-128e-instruct",
    "qwen-qwq-32b",
];

/// Max screenshot width sent to the VLM (token cost control).
/// 1024 default; 768 trims encode+upload ~40% when latency matters.
pub const VISION_MAX_W: u32 = 1024;
pub const VISION_FAST_W: u32 = 768;

// ─── Feature 98 P5: pooled HTTP client (kills cold-TLS tax) ─────────────
//
// Every turn used to build a fresh reqwest::Client, paying a 150–400ms
// cold TLS handshake. One keep-alive client is shared by the tour, the
// spatial pass, and locate/grounding vision calls.
static SHARED_VISION_CLIENT: once_cell::sync::Lazy<reqwest::Client> =
    once_cell::sync::Lazy::new(|| {
        reqwest::Client::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(4))
            .build()
            .unwrap_or_default()
    });

pub fn shared_vision_client() -> reqwest::Client {
    SHARED_VISION_CLIENT.clone()
}

/// Per-request timeout used only by the narrated screen tour: a tour answer
/// needs 6-20 s, so the shared 4 s client made the tour impossible. Click
/// grounding keeps `shared_vision_client` (fast 4 s).
pub const TOUR_REQUEST_TIMEOUT_SECS: u64 = 20;

// Tour-only pooled client — built exactly like `SHARED_VISION_CLIENT`
// (same keep-alive/connect settings) but with the long per-request timeout
// a whole model ladder needs.
static TOUR_VISION_CLIENT: once_cell::sync::Lazy<reqwest::Client> =
    once_cell::sync::Lazy::new(|| {
        reqwest::Client::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(TOUR_REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_default()
    });

pub fn tour_vision_client() -> reqwest::Client {
    TOUR_VISION_CLIENT.clone()
}

// ─── Feature 98 P6: perceptual frame-hash result cache ──────────────────
//
// Repeating a visual query on an unchanged screen (identical capture hash
// within the TTL) replays the last answer in <30ms with 0 quota burn.

struct FrameCacheEntry {
    hash: u64,
    at: std::time::Instant,
    payload: SpatialAnalysisPayload,
}

static FRAME_CACHE: once_cell::sync::Lazy<std::sync::Mutex<Vec<FrameCacheEntry>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(Vec::new()));
const FRAME_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const FRAME_CACHE_MAX: usize = 8;

/// FNV-1a hash of the capture bytes (no allocation, pure).
pub fn frame_hash(b64: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in b64.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Look up a cached answer for `hash` (60s TTL). Prunes expired entries.
pub fn frame_cache_get(hash: u64) -> Option<SpatialAnalysisPayload> {
    let mut cache = FRAME_CACHE.lock().unwrap();
    cache.retain(|e| e.at.elapsed() < FRAME_CACHE_TTL);
    let hit = cache
        .iter()
        .rev()
        .find(|e| e.hash == hash)
        .map(|e| e.payload.clone());
    hit
}

/// Store an answer for `hash` (bounded, newest-wins).
pub fn frame_cache_put(hash: u64, payload: SpatialAnalysisPayload) {
    let mut cache = FRAME_CACHE.lock().unwrap();
    cache.retain(|e| e.hash != hash && e.at.elapsed() < FRAME_CACHE_TTL);
    cache.push(FrameCacheEntry {
        hash,
        at: std::time::Instant::now(),
        payload,
    });
    while cache.len() > FRAME_CACHE_MAX {
        cache.remove(0);
    }
}

/// Screenshot width for the current race setting (fast = smaller image).
pub fn capture_width_for(race: &str) -> u32 {
    if race == "speed" {
        VISION_FAST_W
    } else {
        VISION_MAX_W
    }
}

/// Prompt: force a single JSON coordinate pair, 0-1000 space. Pure.
/// Mentions the axis grid overlay (ticks every 100 units) so the model
/// reads positions off it instead of regressing blind coordinates.
pub fn build_locate_prompt(description: &str) -> String {
    format!(
        "Locate the UI element described as: \"{}\". A faint red grid is overlaid on the screenshot every 100 units, with the edges running 0 (top/left) to 1000 (bottom/right) — read the position off the grid. Reply with ONLY a JSON object like {{\"x\": 500, \"y\": 300}} where x and y are coordinates in 0-1000 space (0,0 = top-left). If the element is not visible, reply with {{\"x\": -1, \"y\": -1}}. No other text.",
        description.trim()
    )
}

/// Parse a 0-1000 coordinate pair from VLM text, scale to pixels.
/// Accepts {"x":N,"y":M}, [N,M], or bare N,M. Pure + unit-tested.
pub fn parse_coords(text: &str, w: i32, h: i32) -> Option<(i32, i32)> {
    // Numbers may be decimals ("512.5, 300.2"): the old integer-only tokenizer
    // split them into four numbers and returned (512, 5).
    let nums: Vec<f64> = {
        let mut out = vec![];
        let mut cur = String::new();
        let mut neg = false;
        let chars: Vec<char> = text.chars().chain(std::iter::once(' ')).collect();
        for (i, &ch) in chars.iter().enumerate() {
            if ch == '-' && cur.is_empty() {
                neg = true;
            } else if ch.is_ascii_digit() {
                cur.push(ch);
            } else if ch == '.'
                && !cur.is_empty()
                && !cur.contains('.')
                && chars.get(i + 1).map_or(false, |c| c.is_ascii_digit())
            {
                cur.push('.');
            } else if !cur.is_empty() {
                let mut n: f64 = cur.parse().ok()?;
                if neg {
                    n = -n;
                }
                out.push(n);
                cur.clear();
                neg = false;
            } else {
                neg = false;
            }
        }
        out
    };
    if nums.len() < 2 {
        return None;
    }
    let (x, y) = (nums[0], nums[1]);
    if x < 0.0 || y < 0.0 || x > 1000.0 || y > 1000.0 {
        return None;
    }
    let (x, y) = (x.round() as i64, y.round() as i64);
    Some(((x * w as i64 / 1000) as i32, (y * h as i64 / 1000) as i32))
}

/// Clicky-style aspect-ratio matching Computer Use resolution.
/// Picks the standard Anthropic resolution closest to the actual monitor aspect ratio
/// to avoid image stretching and X/Y coordinate distortion.
#[allow(dead_code)]
pub fn best_computer_use_resolution(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (1024, 768);
    }
    let target_ratio = width as f64 / height as f64;
    let standard_resolutions: [(u32, u32, f64); 3] = [
        (1024, 768, 1024.0 / 768.0),  // 4:3 = 1.333
        (1280, 800, 1280.0 / 800.0),  // 16:10 = 1.600
        (1366, 768, 1366.0 / 768.0),  // ~16:9 = 1.779
    ];

    let mut best = (1024, 768);
    let mut min_diff = f64::MAX;

    for (w, h, ratio) in standard_resolutions {
        let diff = (ratio - target_ratio).abs();
        if diff < min_diff {
            min_diff = diff;
            best = (w, h);
        }
    }
    best
}

/// Transform normalized computer use coordinates back into screen points
/// with strict screen boundary clamping.
#[allow(dead_code)]
pub fn denormalize_computer_use_coord(
    coord_x: i32,
    coord_y: i32,
    declared_w: u32,
    declared_h: u32,
    screen_w: i32,
    screen_h: i32,
) -> (i32, i32) {
    if declared_w == 0 || declared_h == 0 {
        return (0, 0);
    }
    let px = (coord_x as i64 * screen_w as i64 / declared_w as i64) as i32;
    let py = (coord_y as i64 * screen_h as i64 / declared_h as i64) as i32;

    (px.clamp(0, screen_w - 1), py.clamp(0, screen_h - 1))
}

/// Capture the full primary monitor, downscale to VISION_MAX_W, JPEG q60,
/// base64. Returns (base64_jpeg, width, height). None on any failure.
pub fn capture_primary_jpeg_base64() -> Option<(String, i32, i32)> {
    capture_plain_jpeg_base64_w(VISION_MAX_W)
}

/// Un-gridded capture at an explicit width (the narrated screen tour sends
/// a clean screenshot — the red axis grid is only needed for click
/// grounding, and it can occlude small text). Returns
/// (base64_jpeg, screen_w, screen_h). None on any failure.
pub fn capture_plain_jpeg_base64_w(max_w: u32) -> Option<(String, i32, i32)> {
    let (mw, mh) = crate::screen::primary_monitor_size()?;
    if mw <= 0 || mh <= 0 {
        return None;
    }
    let b64 = capture_region_jpeg_base64(0, 0, mw, mh, max_w)?;
    Some((b64, mw, mh))
}

/// Capture one screen rectangle (physical px) as a native-resolution RGBA image.
/// No resizing, no JPEG encoding. Returns the raw pixels for sharp crops.
pub fn capture_region_rgba(x: i32, y: i32, w: i32, h: i32) -> Option<image::RgbaImage> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let bgra = capture_bgra(x, y, w, h)?;
    let img = image::RgbaImage::from_raw(w as u32, h as u32, bgra_to_rgba(bgra))?;
    Some(img)
}

/// Windows: the GDI capture in `sidebar_backdrop`. macOS: the Phase 2 v0
/// `screencapture` path in `screen_capture_mac`. Elsewhere: None — every
/// caller degrades to None safely.
fn capture_bgra(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    #[cfg(target_os = "windows")]
    {
        crate::sidebar_backdrop::capture_region_bgra_public(x, y, w, h)
    }
    #[cfg(target_os = "macos")]
    {
        crate::screen_capture_mac::capture_bgra(x, y, w, h)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (x, y, w, h);
        None
    }
}

/// Capture one screen rectangle (physical px), downscale to `max_w`, JPEG,
/// base64. The narrated tour sends only the page-content region (no browser
/// tabs / URL bar / taskbar) at higher effective resolution. None on failure.
pub fn capture_region_jpeg_base64(x: i32, y: i32, w: i32, h: i32, max_w: u32) -> Option<String> {
    let img = capture_region_rgba(x, y, w, h)?;
    jpeg_base64_from_rgba(&img, max_w)
}

/// One capture, two products: the downscaled JPEG that goes to the vision
/// model and the native-resolution pixels used for sharp sidebar crops.
pub fn capture_region_pair(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    max_w: u32,
) -> Option<(String, image::RgbaImage)> {
    let img = capture_region_rgba(x, y, w, h)?;
    let b64 = jpeg_base64_from_rgba(&img, max_w)?;
    Some((b64, img))
}

fn jpeg_base64_from_rgba(img: &image::RgbaImage, max_w: u32) -> Option<String> {
    let (fw, fh) = img.dimensions();
    let scale = (max_w as f32 / fw as f32).min(1.0);
    let (tw, th) = (
        ((fw as f32 * scale) as u32).max(1),
        ((fh as f32 * scale) as u32).max(1),
    );
    let small = image::imageops::resize(img, tw, th, image::imageops::FilterType::Triangle);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 70)
        .encode_image(&image::DynamicImage::ImageRgba8(small))
        .ok()?;
    use base64::Engine;
    Some(base64::engine::general_purpose::STANDARD.encode(&jpeg))
}

fn bgra_to_rgba(mut bgra: Vec<u8>) -> Vec<u8> {
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    bgra
}

/// Axis-grid overlay (Axis-Grid Scaffold style): red grid lines every
/// 1/10 of width/height (= 100 units in 0-1000 space) plus a brighter
/// border, so the VLM reads positions off visible anchors instead of
/// regressing blind coordinates. No text labels (no font deps) — the
/// prompt describes the scale. Pure over pixels + unit-tested.
pub fn overlay_axis_grid(mut img: image::RgbaImage) -> image::RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w < 20 || h < 20 {
        return img;
    }
    let grid = image::Rgba([255u8, 60u8, 60u8, 255u8]);
    let edge = image::Rgba([255u8, 30u8, 30u8, 255u8]);
    let put = |img: &mut image::RgbaImage, x: u32, y: u32, c: image::Rgba<u8>| {
        if x < w && y < h {
            img.put_pixel(x, y, c);
        }
    };
    // Interior grid: 2px lines at each 100-unit mark.
    for i in 1..10u32 {
        let gx = (w * i / 10).min(w.saturating_sub(1));
        let gy = (h * i / 10).min(h.saturating_sub(1));
        for y in 0..h {
            put(&mut img, gx, y, grid);
            if gx + 1 < w {
                put(&mut img, gx + 1, y, grid);
            }
        }
        for x in 0..w {
            put(&mut img, x, gy, grid);
            put(&mut img, x, gy + 1, grid);
        }
    }
    // Brighter 3px border = the 0/1000 edges.
    for x in 0..w {
        for dy in 0..3u32 {
            put(&mut img, x, dy, edge);
            if h > dy {
                put(&mut img, x, h - 1 - dy, edge);
            }
        }
    }
    for y in 0..h {
        for dx in 0..3u32 {
            put(&mut img, dx, y, edge);
            if w > dx {
                put(&mut img, w - 1 - dx, y, edge);
            }
        }
    }
    img
}

/// Capture + downscale + grid overlay + JPEG base64.
/// `max_w` controls the downscale target (1024 standard, 768 fast).
/// Returns (base64_jpeg, screen_w, screen_h). None on any failure.
pub fn capture_gridded_jpeg_base64_w(max_w: u32) -> Option<(String, i32, i32)> {
    let (mw, mh) = crate::screen::primary_monitor_size()?;
    if mw <= 0 || mh <= 0 {
        return None;
    }
    let bgra = capture_bgra(0, 0, mw, mh)?;
    let img = image::RgbaImage::from_raw(mw as u32, mh as u32, bgra_to_rgba(bgra))?;
    let scale = (max_w as f32 / mw as f32).min(1.0);
    let (tw, th) = (
        ((mw as f32 * scale) as u32).max(1),
        ((mh as f32 * scale) as u32).max(1),
    );
    let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    let gridded = overlay_axis_grid(small);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 60)
        .encode_image(&image::DynamicImage::ImageRgba8(gridded))
        .ok()?;
    use base64::Engine;
    Some((
        base64::engine::general_purpose::STANDARD.encode(&jpeg),
        mw,
        mh,
    ))
}

/// Spatial analysis of an explicit, verified foreground region. The returned
/// coordinates are relative to that region, never the full monitor.
pub fn capture_gridded_region_jpeg_base64_w(
    region: crate::screen_context::Region,
    max_w: u32,
) -> Option<String> {
    if region.w <= 0 || region.h <= 0 {
        return None;
    }
    let bgra = capture_bgra(
        region.x, region.y, region.w, region.h,
    )?;
    let img = image::RgbaImage::from_raw(
        region.w as u32, region.h as u32, bgra_to_rgba(bgra),
    )?;
    let scale = (max_w as f32 / region.w as f32).min(1.0);
    let tw = ((region.w as f32 * scale) as u32).max(1);
    let th = ((region.h as f32 * scale) as u32).max(1);
    let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    let gridded = overlay_axis_grid(small);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 60)
        .encode_image(&image::DynamicImage::ImageRgba8(gridded))
        .ok()?;
    use base64::Engine;
    Some(base64::engine::general_purpose::STANDARD.encode(jpeg))
}

/// Ask Groq vision for element coordinates. Tries each vision model in
/// order; first parseable in-bounds answer wins. None = fall through.
/// Uses the gridded capture so the model reads off visible anchors.
#[allow(dead_code)]
pub async fn locate_via_vision(
    description: &str,
    api_key: &str,
    client: &reqwest::Client,
) -> Option<crate::screen::UiElement> {
    if description.trim().is_empty() || api_key.is_empty() {
        return None;
    }
    let (b64, w, h) = capture_gridded_jpeg_base64_w(VISION_MAX_W)
        .or_else(capture_primary_jpeg_base64)?;
    let prompt = build_locate_prompt(description);
    let data_uri = format!("data:image/jpeg;base64,{b64}");
    for model in GROQ_VISION_MODELS {
        let body = serde_json::json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url", "image_url": {"url": data_uri}},
                ],
            }],
            "max_tokens": 64,
            "temperature": 0.0,
        });
        let resp = client
            .post("https://api.groq.com/openai/v1/chat/completions")
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            continue;
        }
        let json: serde_json::Value = resp.json().await.ok()?;
        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("");
        if let Some((x, y)) = parse_coords(text, w, h) {
            tracing::info!("vision: '{}' → ({}, {}) via {}", description, x, y, model);
            return Some(crate::screen::UiElement {
                name: description.trim().to_string(),
                kind: "vision".to_string(),
                x,
                y,
                w: 0,
                h: 0,
            });
        }
    }
    None
}

/// UIA first (exact, free), vision on miss. api_key empty → UIA only.
#[allow(dead_code)]
pub async fn resolve_named_target(
    name: &str,
    api_key: &str,
    client: &reqwest::Client,
) -> Option<crate::screen::UiElement> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if let Some(el) = crate::live::commands::mouse::resolve_element(name) {
        return Some(el);
    }
    if api_key.is_empty() {
        return None;
    }
    locate_via_vision(name, api_key, client).await
}

// ─── Dual providers + daily quotas ──────────────────────────────

/// Gemini vision model (live-verified 2026-10-07: HTTP 200 in 1.27s).
pub const GEMINI_VISION_MODEL: &str = "gemini-3.5-flash-lite";
/// Gemini vision ladder — live-verified 2026-10-07. The older
/// gemini-2.5-flash / gemini-2.0-flash ids are 404 (deprecated) and
/// gemini-3.8-flash was 503 under demand; they are gone from the ladder.
pub const GEMINI_VISION_FALLBACKS: &[&str] = &["gemini-3.5-flash", "gemini-flash-lite-latest"];
/// Free-tier daily caps (per project). Conservative vs docs.
pub const GROQ_VISION_RPD: u32 = 14400;
pub const GEMINI_VISION_RPD: u32 = 500;
pub const USAGE_FILE: &str = "vision_usage.json";

/// Read the `visionProvider` setting: "groq" | "gemini" | "auto".
pub fn read_vision_provider(app_data_dir: &std::path::Path) -> String {
    let path = app_data_dir.join("settings.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return "auto".to_string();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return "auto".to_string();
    };
    json.get("visionProvider")
        .or_else(|| json.get("vision_provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("auto")
        .to_string()
}

/// Provider attempt order for a setting value. Pure.
///
/// 2026-10-01 live finding: Groq decommissioned ALL vision models
/// (current catalog: text/TTS/ASR only — scout/maverick 404). "auto"
/// therefore routes to Gemini ONLY; a wasted 404 round-trip per call
/// helps nobody. GROQ_VISION_MODELS stays for the day Groq re-adds
/// vision (explicit "groq" setting still attempts it).
pub fn provider_order(setting: &str) -> Vec<&'static str> {
    match setting.trim().to_lowercase().as_str() {
        "groq" => vec!["groq"],
        "gemini" => vec!["gemini"],
        _ => vec!["gemini"],
    }
}

/// Monday=0 weekday for days-since-epoch (1970-01-01 was Thursday).
fn weekday_mon0(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

/// Civil (y, m, d) from days since epoch (Hinnant). Pure.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (mp + 3 - 12 * (mp / 10)) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Day-of-month of the nth Sunday (n=1 first) of month m. Pure.
fn nth_sunday(y: i64, m: u32, n: u32) -> u32 {
    let dow_mon0 = weekday_mon0(days_from_civil(y, m, 1));
    1 + ((6 + 7 - dow_mon0) % 7) + 7 * (n - 1)
}

/// Pacific UTC offset seconds at ts (PDT -7h in DST, else PST -8h).
/// DST: second Sunday March 02:00 local → first Sunday November 02:00.
fn pacific_offset_secs(ts: i64) -> i64 {
    let (y, _, _) = civil_from_days(ts.div_euclid(86400));
    let start = days_from_civil(y, 3, nth_sunday(y, 3, 2)) * 86400 + 10 * 3600;
    let end = days_from_civil(y, 11, nth_sunday(y, 11, 1)) * 86400 + 9 * 3600;
    if ts >= start && ts < end {
        -7 * 3600
    } else {
        -8 * 3600
    }
}

/// Pacific calendar date YYYY-MM-DD at ts. Pure + unit-tested.
pub fn pacific_date(ts: i64) -> String {
    let local_days = (ts + pacific_offset_secs(ts)).div_euclid(86400);
    let (y, m, d) = civil_from_days(local_days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct UsageFile {
    #[serde(default)]
    date: String,
    #[serde(default)]
    groq: u32,
    #[serde(default)]
    gemini: u32,
}

fn load_usage(app_data_dir: &std::path::Path) -> UsageFile {
    let today = pacific_date(chrono::Utc::now().timestamp());
    let path = app_data_dir.join(USAGE_FILE);
    let stored: UsageFile = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or(UsageFile {
            date: today.clone(),
            groq: 0,
            gemini: 0,
        });
    if stored.date != today {
        return UsageFile {
            date: today,
            groq: 0,
            gemini: 0,
        };
    }
    stored
}

fn save_usage(app_data_dir: &std::path::Path, u: &UsageFile) {
    if let Ok(s) = serde_json::to_string_pretty(u) {
        let _ = std::fs::write(app_data_dir.join(USAGE_FILE), s);
    }
}

/// Record one vision call. Best-effort.
pub fn record_use(app_data_dir: &std::path::Path, provider: &str) {
    crate::usage_counter::bump("vision");
    let mut u = load_usage(app_data_dir);
    match provider {
        "groq" => u.groq = u.groq.saturating_add(1),
        "gemini" => u.gemini = u.gemini.saturating_add(1),
        _ => return,
    }
    save_usage(app_data_dir, &u);
}

/// Daily Gemini vision cap NEXUS enforces: the `geminiVisionDailyLimit`
/// setting (the figure shown in AI Studio's rate-limit page — Google does
/// not publish per-model free numbers in its docs) or the built-in default.
pub fn gemini_daily_limit(app_data_dir: &std::path::Path) -> u32 {
    let text = std::fs::read_to_string(app_data_dir.join("settings.json")).ok();
    parse_daily_limit(text.as_deref())
}

/// Pure: `geminiVisionDailyLimit` from settings JSON text; invalid/zero →
/// default. Clamped to a sane range.
pub fn parse_daily_limit(settings_json: Option<&str>) -> u32 {
    settings_json
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| v.get("geminiVisionDailyLimit").and_then(|n| n.as_u64()))
        .filter(|n| *n > 0)
        .map(|n| n.min(1_000_000) as u32)
        .unwrap_or(GEMINI_VISION_RPD)
}

/// Mark a provider exhausted NOW (e.g. HTTP 429 mid-day).
pub fn mark_exhausted(app_data_dir: &std::path::Path, provider: &str) {
    let mut u = load_usage(app_data_dir);
    match provider {
        "groq" => u.groq = GROQ_VISION_RPD,
        "gemini" => u.gemini = u.gemini.max(gemini_daily_limit(app_data_dir)),
        _ => return,
    }
    save_usage(app_data_dir, &u);
}

/// What an HTTP 429 from a vision provider actually means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKind {
    /// The free-tier DAILY quota is gone — locked until the Pacific day rolls.
    Daily,
    /// A per-minute (or other short-window) limit — retry shortly.
    Rate,
}

/// Classify a 429 response body. Gemini's error carries a `quotaId` such as
/// `GenerateRequestsPerDayPerProjectPerModel-FreeTier` (daily) versus
/// `...PerMinute...` (rate). Anything that doesn't name a daily window is a
/// short rate limit: treating every 429 as "spent for the day" (the old
/// behavior) locked vision — shared with ghost click grounding — until the
/// next Pacific midnight after a single RPM blip. Pure.
pub fn classify_429(body: &str) -> RateLimitKind {
    let b = body.to_lowercase();
    if b.contains("perday") || b.contains("per day") || b.contains("daily") {
        RateLimitKind::Daily
    } else {
        RateLimitKind::Rate
    }
}

/// How long a rate-limited provider is skipped.
pub const RATE_COOLDOWN_SECS: u64 = 60;

static COOLDOWNS: once_cell::sync::Lazy<
    std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
> = once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Skip `provider` for `secs` (in-memory only — a restart clears it, which is
/// right for a short-window limit).
pub fn set_cooldown(provider: &str, secs: u64) {
    if let Ok(mut m) = COOLDOWNS.lock() {
        m.insert(
            provider.to_string(),
            std::time::Instant::now() + std::time::Duration::from_secs(secs),
        );
    }
}

/// True while a short rate-limit cooldown is running for `provider`.
pub fn on_cooldown(provider: &str) -> bool {
    COOLDOWNS
        .lock()
        .ok()
        .and_then(|m| m.get(provider).copied())
        .map(|until| std::time::Instant::now() < until)
        .unwrap_or(false)
}

/// Record the consequence of a 429: daily → locked for the day, otherwise a
/// short cooldown.
pub fn apply_rate_limit(app_data_dir: &std::path::Path, provider: &str, kind: RateLimitKind) {
    match kind {
        RateLimitKind::Daily => mark_exhausted(app_data_dir, provider),
        RateLimitKind::Rate => set_cooldown(provider, RATE_COOLDOWN_SECS),
    }
}

/// Provider can't be used right now: daily cap hit OR short cooldown running.
pub fn unavailable(app_data_dir: &std::path::Path, provider: &str) -> bool {
    exhausted(app_data_dir, provider) || on_cooldown(provider)
}

/// Pure: does this failed response mean the API key itself is bad? 401/403,
/// or Gemini's 400 `API_KEY_INVALID` ("API key not valid").
pub fn is_auth_failure(status: u16, body: &str) -> bool {
    match status {
        401 | 403 => true,
        400 => {
            let b = body.to_lowercase();
            b.contains("api key") || b.contains("api_key_invalid")
        }
        _ => false,
    }
}

/// Read a failed response's status and a short body snippet (for the log and
/// for 429/auth classification). Previously non-2xx replies were swallowed
/// with no trace, so a rejected key looked identical to "no result".
async fn failure_detail(resp: reqwest::Response) -> (u16, String) {
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    (status, body.chars().take(400).collect())
}

/// First `n` chars of a response body for one-line logging.
fn snip(body: &str) -> String {
    body.chars().take(200).collect::<String>().replace('\n', " ")
}

/// True when today's count hit the free-tier cap.
pub fn exhausted(app_data_dir: &std::path::Path, provider: &str) -> bool {
    let u = load_usage(app_data_dir);
    match provider {
        "groq" => u.groq >= GROQ_VISION_RPD,
        "gemini" => u.gemini >= gemini_daily_limit(app_data_dir),
        _ => true,
    }
}

/// Quota snapshot for the settings UI.
pub fn quota_status(app_data_dir: &std::path::Path) -> serde_json::Value {
    let u = load_usage(app_data_dir);
    serde_json::json!({
        "date": u.date,
        "groq": {"used": u.groq, "limit": GROQ_VISION_RPD},
        "gemini": {"used": u.gemini, "limit": gemini_daily_limit(app_data_dir)},
    })
}

/// Per-attempt outcome: found, clean miss, or quota-hit (skip provider).
enum LocateStep {
    Found(crate::screen::UiElement),
    Miss,
    /// 429 — daily cap vs short rate limit decides the lockout (see `classify_429`).
    Quota(RateLimitKind),
}

fn found_el(description: &str, x: i32, y: i32, via: &str) -> crate::screen::UiElement {
    tracing::info!("vision: '{}' → ({}, {}) via {}", description, x, y, via);
    crate::screen::UiElement {
        name: description.trim().to_string(),
        kind: "vision".to_string(),
        x,
        y,
        w: 0,
        h: 0,
    }
}

async fn locate_groq_with_image(
    description: &str,
    b64: &str,
    w: i32,
    h: i32,
    api_key: &str,
    client: &reqwest::Client,
) -> LocateStep {
    let prompt = build_locate_prompt(description);
    let data_uri = format!("data:image/jpeg;base64,{b64}");
    for model in GROQ_VISION_MODELS {
        let body = serde_json::json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url", "image_url": {"url": data_uri}},
                ],
            }],
            "max_tokens": 64,
            "temperature": 0.0,
        });
        let resp = match client
            .post("https://api.groq.com/openai/v1/chat/completions")
            .bearer_auth(api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !resp.status().is_success() {
            let (status, body) = failure_detail(resp).await;
            if status == 429 {
                return LocateStep::Quota(classify_429(&body));
            }
            tracing::warn!("vision: groq locate {model} HTTP {status}: {}", snip(&body));
            if is_auth_failure(status, &body) {
                return LocateStep::Miss;
            }
            continue;
        }
        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(_) => continue,
        };
        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("");
        if let Some((x, y)) = parse_coords(text, w, h) {
            return LocateStep::Found(found_el(description, x, y, model));
        }
    }
    LocateStep::Miss
}

async fn locate_gemini_with_image(
    description: &str,
    b64: &str,
    w: i32,
    h: i32,
    api_key: &str,
    client: &reqwest::Client,
) -> LocateStep {
    let prompt = build_locate_prompt(description);
    let body = serde_json::json!({
        "contents": [{
            "parts": [
                {"text": prompt},
                {"inline_data": {"mime_type": "image/jpeg", "data": b64}},
            ],
        }],
        "generationConfig": {"maxOutputTokens": 64, "temperature": 0.0},
    });
    let mut quota_seen: Option<RateLimitKind> = None;
    for model in [GEMINI_VISION_MODEL].into_iter().chain(GEMINI_VISION_FALLBACKS.iter().copied()) {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
        );
        let resp = match client
            .post(&url)
            .header("x-goog-api-key", api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !resp.status().is_success() {
            let (status, body) = failure_detail(resp).await;
            if status == 429 {
                // Each model has its own free-tier quota — try the next
                // before declaring the provider out.
                quota_seen = Some(match (quota_seen, classify_429(&body)) {
                    (Some(RateLimitKind::Daily), _) | (_, RateLimitKind::Daily) => RateLimitKind::Daily,
                    _ => RateLimitKind::Rate,
                });
                continue;
            }
            tracing::warn!("vision: gemini locate {model} HTTP {status}: {}", snip(&body));
            if is_auth_failure(status, &body) {
                return LocateStep::Miss;
            }
            continue;
        }
        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(_) => continue,
        };
        let text: String = json["candidates"][0]["content"]["parts"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text")?.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        if let Some((x, y)) = parse_coords(&text, w, h) {
            return LocateStep::Found(found_el(description, x, y, model));
        }
    }
    match quota_seen {
        Some(kind) => LocateStep::Quota(kind),
        None => LocateStep::Miss,
    }
}

/// Is there a key for a provider that the given order will ACTUALLY try?
/// The gates used to check "any key at all" — but Groq vision is gone, so
/// `provider_order("auto")` is Gemini-only and a Groq-only user passed the gate,
/// had the screenshot captured and then heard "my daily vision limits are
/// spent" (a lie: no call was ever made). Pure.
pub fn has_usable_vision_key(order_setting: &str, groq_key: &str, gemini_key: &str) -> bool {
    provider_order(order_setting).iter().any(|p| match *p {
        "groq" => !groq_key.is_empty(),
        _ => !gemini_key.is_empty(),
    })
}

/// Located target + how it was found for speech. `raced` = parallel
/// race fired both providers (2 quota units — announce for honesty).
#[allow(dead_code)]
pub struct LocatedTarget {
    pub el: crate::screen::UiElement,
    pub fell_back: bool,
    pub raced: bool,
}

/// Read the `visionRace` setting: "speed" | "sequential" (default).
pub fn read_vision_race(app_data_dir: &std::path::Path) -> String {
    let path = app_data_dir.join("settings.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return "sequential".to_string();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return "sequential".to_string();
    };
    json.get("visionRace")
        .or_else(|| json.get("vision_race"))
        .and_then(|v| v.as_str())
        .unwrap_or("sequential")
        .to_string()
}

/// UIA already tried by the caller. Two strategies:
/// - "speed": race both providers in parallel, first valid answer wins
///   (both quota units consumed — worst case halves to the fastest call).
/// - sequential (default): try in provider order, skip exhausted ones.
///
/// Returns None only when every provider missed or was unavailable.
pub async fn locate_with_fallback(
    description: &str,
    groq_key: &str,
    gemini_key: &str,
    order_setting: &str,
    usage_dir: &std::path::Path,
    client: &reqwest::Client,
) -> Option<LocatedTarget> {
    if description.trim().is_empty() {
        return None;
    }
    let race = read_vision_race(usage_dir);
    // Capture once — all providers share the gridded screenshot.
    // Race mode uses the smaller fast capture (faster encode/upload).
    let (b64, w, h) = capture_gridded_jpeg_base64_w(capture_width_for(&race))
        .or_else(capture_primary_jpeg_base64)?;

    // ── Race mode: both keys, both under quota, auto order ──────
    if read_vision_race(usage_dir) == "speed"
        && provider_order(order_setting).len() > 1
        && !groq_key.is_empty()
        && !gemini_key.is_empty()
        && !unavailable(usage_dir, "groq")
        && !unavailable(usage_dir, "gemini")
    {
        let g1 = locate_groq_with_image(description, &b64, w, h, groq_key, client);
        let g2 = locate_gemini_with_image(description, &b64, w, h, gemini_key, client);
        let (r1, r2) = tokio::join!(g1, g2);
        // Usage recorded for BOTH calls (both consumed quota regardless
        // of who won).
        record_use(usage_dir, "groq");
        record_use(usage_dir, "gemini");
        let pick = |s: &LocateStep| match s {
            LocateStep::Found(el) => Some(el.clone()),
            _ => None,
        };
        if let LocateStep::Quota(kind) = r1 {
            apply_rate_limit(usage_dir, "groq", kind);
        }
        if let LocateStep::Quota(kind) = r2 {
            apply_rate_limit(usage_dir, "gemini", kind);
        }
        if let Some(el) = pick(&r1).or_else(|| pick(&r2)) {
            return Some(LocatedTarget {
                el,
                fell_back: false,
                raced: true,
            });
        }
        return None;
    }

    // ── Sequential path (default) ────────────────────────────────
    let order = provider_order(order_setting);
    let mut tried = 0;
    for provider in order {
        let (key, label): (&str, &'static str) = match provider {
            "groq" => (groq_key, "groq"),
            _ => (gemini_key, "gemini"),
        };
        if key.is_empty() {
            continue;
        }
        if unavailable(usage_dir, label) {
            continue;
        }
        tried += 1;
        let step = match provider {
            "groq" => locate_groq_with_image(description, &b64, w, h, key, client).await,
            _ => locate_gemini_with_image(description, &b64, w, h, key, client).await,
        };
        match step {
            LocateStep::Found(el) => {
                record_use(usage_dir, label);
                return Some(LocatedTarget {
                    el,
                    fell_back: tried > 1,
                    raced: false,
                });
            }
            LocateStep::Quota(kind) => {
                apply_rate_limit(usage_dir, label, kind);
                continue;
            }
            LocateStep::Miss => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tour_client_timeout_is_the_locked_20s_cap() {
        // The tour runs its own pooled client with the long cap;
        // the shared client stays at the fast 4 s for click grounding.
        assert_eq!(TOUR_REQUEST_TIMEOUT_SECS, 20);
        assert!(
            std::time::Duration::from_secs(TOUR_REQUEST_TIMEOUT_SECS)
                > std::time::Duration::from_secs(4)
        );
    }

    #[test]
    fn test_best_computer_use_resolution() {
        // 16:9 monitor (1920x1080) -> (1366, 768)
        assert_eq!(best_computer_use_resolution(1920, 1080), (1366, 768));
        // 16:10 monitor (2560x1600) -> (1280, 800)
        assert_eq!(best_computer_use_resolution(2560, 1600), (1280, 800));
        // 4:3 display (1024x768) -> (1024, 768)
        assert_eq!(best_computer_use_resolution(1024, 768), (1024, 768));
    }

    #[test]
    fn test_denormalize_computer_use_coord() {
        let (px, py) = denormalize_computer_use_coord(683, 384, 1366, 768, 1920, 1080);
        assert_eq!((px, py), (960, 540));
    }

    #[test]
    fn test_build_locate_prompt_contains_desc() {
        let p = build_locate_prompt("Send button");
        assert!(p.contains("Send button"));
        assert!(p.contains("0-1000"));
    }

    #[test]
    fn test_parse_coords_json() {
        assert_eq!(parse_coords(r#"{"x": 500, "y": 250}"#, 2000, 1000), Some((1000, 250)));
    }

    #[test]
    fn test_parse_coords_decimals_are_one_number_each() {
        assert_eq!(parse_coords("512.5, 300.2", 1000, 1000), Some((513, 300)));
        assert_eq!(parse_coords("[250.0, 750.4]", 2000, 1000), Some((500, 750)));
        assert_eq!(parse_coords("{\"x\": 100.5, \"y\": 200.5}", 1000, 1000), Some((101, 201)));
        // A trailing sentence period is not a decimal point.
        assert_eq!(parse_coords("It is at 400, 600.", 1000, 1000), Some((400, 600)));
        assert_eq!(parse_coords("1000.7, 5", 1000, 1000), None);
    }

    #[test]
    fn test_parse_coords_array() {
        assert_eq!(parse_coords("[100, 900]", 1000, 1000), Some((100, 900)));
    }

    #[test]
    fn test_parse_coords_bare() {
        assert_eq!(parse_coords("750, 250", 800, 600), Some((600, 150)));
    }

    #[test]
    fn test_parse_coords_missing_rejected() {
        assert_eq!(parse_coords(r#"{"x": -1, "y": -1}"#, 1000, 1000), None);
    }

    #[test]
    fn test_parse_coords_out_of_range_rejected() {
        assert_eq!(parse_coords("1500, 2000", 1000, 1000), None);
    }

    #[test]
    fn test_parse_coords_garbage_rejected() {
        assert_eq!(parse_coords("no coordinates here", 1000, 1000), None);
        assert_eq!(parse_coords("", 1000, 1000), None);
    }

    #[test]
    fn test_bgra_to_rgba_swaps() {
        assert_eq!(bgra_to_rgba(vec![1, 2, 3, 4]), vec![3, 2, 1, 4]);
    }

    #[test]
    fn test_grid_preserves_dims_and_marks() {
        let img = image::RgbaImage::from_pixel(200, 100, image::Rgba([0, 0, 0, 255]));
        let out = overlay_axis_grid(img);
        assert_eq!((out.width(), out.height()), (200, 100));
        // Border pixel is grid-red.
        assert_eq!(*out.get_pixel(0, 0), image::Rgba([255, 30, 30, 255]));
        // Interior grid line at x = w/10.
        assert_eq!(*out.get_pixel(20, 50), image::Rgba([255, 60, 60, 255]));
        // Off-grid pixel untouched.
        assert_eq!(*out.get_pixel(5, 5), image::Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn test_grid_tiny_passthrough() {
        let img = image::RgbaImage::from_pixel(10, 10, image::Rgba([1, 2, 3, 255]));
        let out = overlay_axis_grid(img);
        assert_eq!(*out.get_pixel(0, 0), image::Rgba([1, 2, 3, 255]));
    }

    #[test]
    fn test_provider_order() {
        // Groq decommissioned all vision models (2026-10-01): auto = Gemini only.
        assert_eq!(provider_order("groq"), vec!["groq"]);
        assert_eq!(provider_order("gemini"), vec!["gemini"]);
        assert_eq!(provider_order("auto"), vec!["gemini"]);
        assert_eq!(provider_order(""), vec!["gemini"]);
        assert_eq!(provider_order("GEMINI"), vec!["gemini"]);
    }

    #[test]
    fn test_capture_width_for_race() {
        assert_eq!(capture_width_for("speed"), VISION_FAST_W);
        assert_eq!(capture_width_for("sequential"), VISION_MAX_W);
        assert_eq!(capture_width_for(""), VISION_MAX_W);
    }

    fn racedir(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("nexus_race_test_{}_{}", name, std::process::id()));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    #[test]
    fn test_race_setting_default_sequential() {
        let d = racedir("default");
        assert_eq!(read_vision_race(&d), "sequential");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_race_setting_speed() {
        let d = racedir("speed");
        std::fs::write(
            d.join("settings.json"),
            r#"{"visionRace": "speed"}"#,
        )
        .unwrap();
        assert_eq!(read_vision_race(&d), "speed");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_race_skipped_when_one_quota_exhausted() {
        // With gemini exhausted, race must NOT fire (single-provider
        // sequential path) — the race fires only when both are fresh.
        let d = racedir("skipped");
        std::fs::write(
            d.join("settings.json"),
            r#"{"visionRace": "speed", "visionProvider": "auto"}"#,
        )
        .unwrap();
        mark_exhausted(&d, "gemini");
        assert!(exhausted(&d, "gemini"));
        assert!(!exhausted(&d, "groq"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn ts(y: i32, mo: u32, d: u32, h: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    #[test]
    fn test_pacific_dst_spring_forward_2026() {
        // 2nd Sunday March 2026 = Mar 8; switch 10:00 UTC.
        assert_eq!(pacific_offset_secs(ts(2026, 3, 8, 9)), -8 * 3600);
        assert_eq!(pacific_offset_secs(ts(2026, 3, 8, 11)), -7 * 3600);
    }

    #[test]
    fn test_pacific_dst_fall_back_2026() {
        // 1st Sunday Nov 2026 = Nov 1; switch 09:00 UTC.
        assert_eq!(pacific_offset_secs(ts(2026, 11, 1, 8)), -7 * 3600);
        assert_eq!(pacific_offset_secs(ts(2026, 11, 1, 10)), -8 * 3600);
    }

    #[test]
    fn test_pacific_date_rolls_day() {
        // 06:00 UTC Sep 27 = 23:00 PDT Sep 26.
        assert_eq!(pacific_date(ts(2026, 9, 27, 6)), "2026-09-26");
        assert_eq!(pacific_date(ts(2026, 9, 27, 12)), "2026-09-27");
    }

    fn qdir(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("nexus_vision_test_{}_{}", name, std::process::id()));
        let _ = std::fs::create_dir_all(&p);
        p
    }

    #[test]
    fn test_quota_record_and_exhaust() {
        let d = qdir("record");
        assert!(!exhausted(&d, "gemini"));
        record_use(&d, "gemini");
        assert_eq!(load_usage(&d).gemini, 1);
        mark_exhausted(&d, "gemini");
        assert!(exhausted(&d, "gemini"));
        assert!(!exhausted(&d, "groq"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_quota_status_shape() {
        let d = qdir("status");
        let s = quota_status(&d);
        assert_eq!(s["groq"]["limit"], 14400);
        assert_eq!(s["gemini"]["limit"], 500);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_quota_unknown_provider_conservative() {
        let d = qdir("unknown");
        assert!(exhausted(&d, "nope"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn test_parse_screen_email_json_with_fences() {
        let raw = "```json\n{\n  \"is_email_visible\": true,\n  \"sender_name\": \"Prof. Smith\",\n  \"sender_email\": \"smith@mit.edu\",\n  \"subject\": \"CS50 Milestone 2\",\n  \"current_deadline\": \"Friday 5 PM\",\n  \"summary\": \"Milestone instructions and deadline\"\n}\n```";
        let parsed = parse_screen_email_json(raw).unwrap();
        assert!(parsed.is_email_visible);
        assert_eq!(parsed.sender_name, "Prof. Smith");
        assert_eq!(parsed.sender_email, "smith@mit.edu");
        assert_eq!(parsed.subject, "CS50 Milestone 2");
        assert_eq!(parsed.current_deadline, Some("Friday 5 PM".to_string()));
    }

    #[test]
    fn test_parse_screen_email_json_not_visible() {
        let raw = "{\n  \"is_email_visible\": false,\n  \"sender_name\": \"\",\n  \"sender_email\": \"\",\n  \"subject\": \"\",\n  \"current_deadline\": null,\n  \"summary\": \"Desktop wallpaper\"\n}";
        let parsed = parse_screen_email_json(raw).unwrap();
        assert!(!parsed.is_email_visible);
        assert_eq!(parsed.current_deadline, None);
    }

    /// Check whether a predicted point lies inside a rectangle expanded by `margin` pixels on each side.
    fn point_in_expanded_rect(px: i32, py: i32, rx: i32, ry: i32, rw: i32, rh: i32, margin: i32) -> bool {
        let min_x = rx - margin;
        let max_x = rx + rw + margin;
        let min_y = ry - margin;
        let max_y = ry + rh + margin;
        px >= min_x && px <= max_x && py >= min_y && py <= max_y
    }

    /// Compute the median of a slice of floats. Returns None if the slice is empty.
    fn median(values: &[f64]) -> Option<f64> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = sorted.len() / 2;
        if sorted.len() % 2 == 1 {
            Some(sorted[mid])
        } else {
            Some((sorted[mid - 1] + sorted[mid]) / 2.0)
        }
    }

    #[test]
    fn test_point_in_expanded_rect_inside() {
        // Control rect at (100, 100) with size 50x30, expanded by 8px -> [92, 158] x [92, 138]
        assert!(point_in_expanded_rect(125, 115, 100, 100, 50, 30, 8));
        // Corners of unexpanded rect
        assert!(point_in_expanded_rect(100, 100, 100, 100, 50, 30, 8));
        assert!(point_in_expanded_rect(150, 130, 100, 100, 50, 30, 8));
        // Point in the expanded margin (between 0 and 8px away)
        assert!(point_in_expanded_rect(95, 100, 100, 100, 50, 30, 8));
    }

    #[test]
    fn test_point_in_expanded_rect_outside() {
        // [92, 158] x [92, 138]
        assert!(!point_in_expanded_rect(91, 115, 100, 100, 50, 30, 8)); // 1px left of expanded border
        assert!(!point_in_expanded_rect(159, 115, 100, 100, 50, 30, 8)); // 1px right of expanded border
        assert!(!point_in_expanded_rect(125, 91, 100, 100, 50, 30, 8)); // 1px above expanded border
        assert!(!point_in_expanded_rect(125, 139, 100, 100, 50, 30, 8)); // 1px below expanded border
        assert!(!point_in_expanded_rect(0, 0, 100, 100, 50, 30, 8));
    }

    #[test]
    fn test_point_in_expanded_rect_border() {
        // [92, 158] x [92, 138]
        assert!(point_in_expanded_rect(92, 115, 100, 100, 50, 30, 8)); // left border
        assert!(point_in_expanded_rect(158, 115, 100, 100, 50, 30, 8)); // right border
        assert!(point_in_expanded_rect(125, 92, 100, 100, 50, 30, 8)); // top border
        assert!(point_in_expanded_rect(125, 138, 100, 100, 50, 30, 8)); // bottom border
        assert!(point_in_expanded_rect(92, 92, 100, 100, 50, 30, 8)); // top-left corner
        assert!(point_in_expanded_rect(158, 138, 100, 100, 50, 30, 8)); // bottom-right corner
    }

    #[test]
    fn test_point_in_expanded_rect_negative() {
        // Rect at (-100, -80) size 40x30, margin 8 -> [-108, -52] x [-88, -42]
        assert!(point_in_expanded_rect(-80, -65, -100, -80, 40, 30, 8)); // inside
        assert!(point_in_expanded_rect(-108, -65, -100, -80, 40, 30, 8)); // left border
        assert!(point_in_expanded_rect(-52, -42, -100, -80, 40, 30, 8)); // bottom-right corner
        assert!(!point_in_expanded_rect(-109, -65, -100, -80, 40, 30, 8)); // outside left
        assert!(!point_in_expanded_rect(-51, -65, -100, -80, 40, 30, 8)); // outside right
    }

    #[test]
    fn test_median_empty_list() {
        let empty: [f64; 0] = [];
        assert_eq!(median(&empty), None);
    }

    #[test]
    fn test_median_odd_even_and_negative() {
        assert_eq!(median(&[42.0]), Some(42.0));
        assert_eq!(median(&[10.0, 50.0, 20.0]), Some(20.0));
        assert_eq!(median(&[10.0, 20.0, 30.0, 40.0]), Some(25.0));
        assert_eq!(median(&[-30.0, -10.0, -20.0]), Some(-20.0));
    }

    #[test]
    #[ignore = "calls the real vision providers (uses a little free quota) and opens Notepad; run by hand"]
    #[cfg(target_os = "windows")]
    fn live_vision_accuracy_against_uia_ground_truth() {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

        struct WindowGuard {
            hwnd: isize,
            child: Option<std::process::Child>,
        }

        impl Drop for WindowGuard {
            fn drop(&mut self) {
                if self.hwnd != 0 {
                    // SAFETY: posting WM_CLOSE to a window opened by this test.
                    unsafe {
                        let _ = PostMessageW(HWND(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                }
                if let Some(mut child) = self.child.take() {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }

        // 1. Open Notepad and bring it to the foreground
        let known_hwnds: std::collections::HashSet<isize> = crate::window_tile::list_top_windows()
            .into_iter()
            .filter(|w| w.exe == "notepad.exe" || w.title.to_lowercase().contains("notepad"))
            .map(|w| w.hwnd)
            .collect();

        let child = match std::process::Command::new("notepad.exe").spawn() {
            Ok(c) => c,
            Err(e) => {
                println!("[PROOF] SKIPPED: failed to spawn notepad.exe: {e}");
                return;
            }
        };

        let start = std::time::Instant::now();
        let mut notepad_hwnd: Option<isize> = None;
        while start.elapsed() < std::time::Duration::from_secs(5) {
            let wins = crate::window_tile::list_top_windows();
            if let Some(w) = wins.iter().find(|w| {
                (w.exe == "notepad.exe" || w.title.to_lowercase().contains("notepad"))
                    && !known_hwnds.contains(&w.hwnd)
            }) {
                notepad_hwnd = Some(w.hwnd);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        if notepad_hwnd.is_none() {
            let wins = crate::window_tile::list_top_windows();
            if let Some(w) = wins.iter().find(|w| w.exe == "notepad.exe" || w.title.to_lowercase().contains("notepad")) {
                notepad_hwnd = Some(w.hwnd);
            }
        }

        let Some(hwnd) = notepad_hwnd else {
            println!("[PROOF] SKIPPED: notepad window not found within 5 s");
            return;
        };

        let _guard = WindowGuard {
            hwnd,
            child: Some(child),
        };

        crate::live::commands::window::focus_window(HWND(hwnd));
        let _ = crate::live::commands::window::wait_and_focus_app("Notepad", 2000);
        std::thread::sleep(std::time::Duration::from_millis(700));

        // 2. Enumerate actionable controls from UI Automation
        let actionables = crate::screen::list_actionables();
        let (mw, mh) = crate::screen::primary_monitor_size().unwrap_or((1920, 1080));

        let mut seen_names = std::collections::HashSet::new();
        let mut primary_controls: Vec<crate::screen::UiElement> = Vec::new();
        let mut caption_controls: Vec<crate::screen::UiElement> = Vec::new();

        for el in actionables {
            let name_trimmed = el.name.trim();
            let char_count = name_trimmed.chars().count();
            if char_count < 3 || char_count > 40 {
                continue;
            }
            if el.w < 12 || el.h < 12 {
                continue;
            }
            if el.x < 0 || el.y < 0 || (el.x + el.w) > mw || (el.y + el.h) > mh {
                continue;
            }
            let lower_name = name_trimmed.to_lowercase();
            if !seen_names.insert(lower_name.clone()) {
                continue;
            }

            // Skip document / text-edit area
            let is_doc_or_edit = lower_name.contains("text editor")
                || lower_name.contains("document")
                || lower_name == "rich edit control"
                || (el.kind.eq_ignore_ascii_case("edit") && (el.h > 80 || el.w > 300));
            if is_doc_or_edit {
                continue;
            }

            let is_caption = matches!(lower_name.as_str(), "minimize" | "maximize" | "restore" | "close");
            if is_caption {
                caption_controls.push(el);
            } else {
                primary_controls.push(el);
            }
        }

        let mut selected_controls: Vec<crate::screen::UiElement> = Vec::new();
        for el in primary_controls {
            if selected_controls.len() >= 8 {
                break;
            }
            selected_controls.push(el);
        }
        if selected_controls.len() < 8 {
            for el in caption_controls {
                if selected_controls.len() >= 8 {
                    break;
                }
                selected_controls.push(el);
            }
        }

        if selected_controls.len() < 3 {
            println!("[PROOF] SKIPPED: only {} usable controls", selected_controls.len());
            return;
        }

        // 3. API keys and configuration
        let groq_key = crate::auth_vault::get_api_key("groq").unwrap_or_default();
        let gemini_key = crate::auth_vault::get_api_key("gemini").unwrap_or_default();
        if groq_key.trim().is_empty() && gemini_key.trim().is_empty() {
            println!("[PROOF] SKIPPED: no vision key");
            return;
        }

        let temp_usage_dir = std::env::temp_dir().join(format!("nexus_vision_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_usage_dir);
        struct TempUsageGuard(std::path::PathBuf);
        impl Drop for TempUsageGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _temp_guard = TempUsageGuard(temp_usage_dir.clone());

        let client = shared_vision_client();
        let order_setting = if gemini_key.trim().is_empty() && !groq_key.trim().is_empty() {
            "groq"
        } else {
            "auto"
        };

        // 4. Measure accuracy for each selected control
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                println!("[PROOF] SKIPPED: failed to create tokio runtime: {e}");
                return;
            }
        };

        struct Meas {
            dist: i64,
            hit: bool,
            ms: u64,
        }

        let (measurements, answered_any) = rt.block_on(async {
            let mut records: Vec<Meas> = Vec::new();
            let mut answered = false;

            for (idx, el) in selected_controls.iter().enumerate() {
                if idx > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                }

                let description = format!("the {} {}", el.name, el.kind);
                let t0 = std::time::Instant::now();
                let target = locate_with_fallback(
                    &description,
                    &groq_key,
                    &gemini_key,
                    order_setting,
                    &temp_usage_dir,
                    &client,
                )
                .await;
                let elapsed_ms = t0.elapsed().as_millis() as u64;

                let cx = el.x + el.w / 2;
                let cy = el.y + el.h / 2;

                let (pred_x, pred_y, dist, hit, provider) = if let Some(t) = target {
                    answered = true;
                    let px = t.el.x;
                    let py = t.el.y;
                    let dx = (px - cx) as f64;
                    let dy = (py - cy) as f64;
                    let d = (dx * dx + dy * dy).sqrt().round() as i64;
                    let is_hit = point_in_expanded_rect(px, py, el.x, el.y, el.w, el.h, 8);
                    (px, py, d, is_hit, if t.fell_back { "fallback" } else { "primary" }.to_string())
                } else {
                    (-1, -1, -1, false, "none".to_string())
                };

                println!(
                    "[PROOF] vision name=\"{}\" kind={} uia=({},{},{},{}) pred=({},{}) dist={} hit={} ms={} provider={}",
                    el.name, el.kind, el.x, el.y, el.w, el.h, pred_x, pred_y, dist, hit, elapsed_ms, provider
                );

                records.push(Meas {
                    dist,
                    hit,
                    ms: elapsed_ms,
                });
            }

            (records, answered)
        });

        // 5. Final summary
        let hits = measurements.iter().filter(|m| m.hit).count();
        let total = measurements.len();
        let dist_vals: Vec<f64> = measurements.iter().filter(|m| m.dist >= 0).map(|m| m.dist as f64).collect();
        let med_dist = median(&dist_vals).map(|d| d.round() as i64).unwrap_or(0);
        let ms_vals: Vec<f64> = measurements.iter().map(|m| m.ms as f64).collect();
        let med_ms = median(&ms_vals).map(|m| m.round() as u64).unwrap_or(0);

        println!(
            "[PROOF] vision hit_rate={}/{} median_dist_px={} median_ms={}",
            hits, total, med_dist, med_ms
        );

        assert!(
            answered_any,
            "no vision provider answered any control request (all returned None)"
        );
    }
}

// ─── Semantic Document & Email Extraction ────────────────────────────

#[allow(dead_code)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ScreenEmailContext {
    pub is_email_visible: bool,
    pub sender_name: String,
    pub sender_email: String,
    pub subject: String,
    pub current_deadline: Option<String>,
    pub summary: String,
}

/// Prompt instructing the VLM to extract email metadata and deadlines from the screen.
#[allow(dead_code)]
pub fn build_email_extraction_prompt() -> &'static str {
    "You are an expert OCR and document understanding engine. Inspect the screenshot and find the email client or open email message.\n\
    Extract the following fields into a single JSON object:\n\
    - is_email_visible: boolean (true if an email message or inbox thread is visible)\n\
    - sender_name: string (name of the email sender or organizer, empty if not found)\n\
    - sender_email: string (email address of the sender, empty if not found)\n\
    - subject: string (subject line of the email thread, empty if not found)\n\
    - current_deadline: string or null (any deadline, due date, submission date, or scheduled time mentioned in the email body)\n\
    - summary: string (one-sentence summary of the email content)\n\
    Reply with ONLY the raw JSON object and no other text."
}

/// Parse VLM response text into a typed `ScreenEmailContext`.
#[allow(dead_code)]
pub fn parse_screen_email_json(text: &str) -> Option<ScreenEmailContext> {
    let clean = text.trim();
    let json_str = if let Some(stripped) = clean.strip_prefix("```json") {
        stripped.strip_suffix("```").unwrap_or(stripped).trim()
    } else if let Some(stripped) = clean.strip_prefix("```") {
        stripped.strip_suffix("```").unwrap_or(stripped).trim()
    } else {
        clean
    };

    let val: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let is_email_visible = val.get("is_email_visible").and_then(|v| v.as_bool()).unwrap_or(false);
    let sender_name = val.get("sender_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let sender_email = val.get("sender_email").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let subject = val.get("subject").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let current_deadline = val.get("current_deadline").and_then(|v| v.as_str()).map(|s| s.to_string());
    let summary = val.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();

    Some(ScreenEmailContext {
        is_email_visible,
        sender_name,
        sender_email,
        subject,
        current_deadline,
        summary,
    })
}

// ─── Feature 86: Spatial Vision & Screen OCR Command Center ──────────

/// Normalized 2D bounding box in the Gemini spatial-grounding grid:
/// `[ymin, xmin, ymax, xmax]` on a 0–1000 integer space (0,0 = top-left).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpatialBoundingBox {
    pub ymin: u32,
    pub xmin: u32,
    pub ymax: u32,
    pub xmax: u32,
}

impl SpatialBoundingBox {
    /// Width in 0-1000 grid units (never negative after normalization).
    pub fn w_units(&self) -> u32 {
        self.xmax.saturating_sub(self.xmin)
    }
    /// Height in 0-1000 grid units.
    pub fn h_units(&self) -> u32 {
        self.ymax.saturating_sub(self.ymin)
    }
}

/// One detail row in the sidebar card (e.g. "Protein" → "6g").
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpatialDetailRow {
    pub title: String,
    pub value: String,
}

/// One spatially-located item: pin id + label + category + box + deep dive.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpatialAnalysisItem {
    pub id: u32,
    pub label: String,
    #[serde(default)]
    pub category: String,
    pub box_2d: SpatialBoundingBox,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub details: Vec<SpatialDetailRow>,
    /// Small crop from the same captured frame, kept in memory for the sidebar.
    /// Never persisted or fetched from a remote image URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_data_url: Option<String>,
    /// Brand logo (small PNG data URL) for a recognisable product/app item,
    /// resolved by `brand_logo`. Never the screenshot itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_data_url: Option<String>,
}

fn default_confidence() -> f32 {
    0.9
}

/// Full analysis result: title + overview + item cards + provider used.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpatialAnalysisPayload {
    pub title: String,
    #[serde(default)]
    pub overview: String,
    pub items: Vec<SpatialAnalysisItem>,
    pub provider_used: String, // "gemini" | "groq"
    /// The analysed frame, displayed above the cards so users can verify the
    /// review belongs to the page they were looking at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_image_data_url: Option<String>,
}

/// Attach bounded visual crops to the cards using the exact frame that was
/// analysed. A second screen capture could show a different window and attach
/// the wrong person's image to a card. Crops live only in the current payload.
pub fn attach_spatial_thumbnails(payload: &mut SpatialAnalysisPayload, jpeg_b64: &str) -> usize {
    use base64::Engine;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(jpeg_b64) else {
        return 0;
    };
    if bytes.len() > 4_000_000 {
        return 0;
    }
    let Ok(frame) = image::load_from_memory(&bytes) else {
        return 0;
    };
    let frame = frame.to_rgb8();
    let (fw, fh) = frame.dimensions();
    if fw == 0 || fh == 0 {
        return 0;
    }
    payload.screen_image_data_url = Some(format!("data:image/jpeg;base64,{jpeg_b64}"));
    let mut attached = 0;
    for item in &mut payload.items {
        let b = &item.box_2d;
        let x0 = (fw as u64 * b.xmin as u64 / 1000).min(fw as u64) as u32;
        let y0 = (fh as u64 * b.ymin as u64 / 1000).min(fh as u64) as u32;
        let x1 = (fw as u64 * b.xmax as u64 / 1000).min(fw as u64) as u32;
        let y1 = (fh as u64 * b.ymax as u64 / 1000).min(fh as u64) as u32;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let crop = image::imageops::crop_imm(&frame, x0, y0, x1 - x0, y1 - y0).to_image();
        let scale = (360.0 / crop.width() as f32)
            .min(240.0 / crop.height() as f32)
            .min(1.0);
        let tw = ((crop.width() as f32 * scale) as u32).max(1);
        let th = ((crop.height() as f32 * scale) as u32).max(1);
        let thumb = image::imageops::resize(&crop, tw, th, image::imageops::FilterType::Triangle);
        let mut jpeg = Vec::new();
        if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 70)
            .encode_image(&image::DynamicImage::ImageRgb8(thumb))
            .is_ok()
            && jpeg.len() <= 100_000
        {
            item.image_data_url = Some(format!(
                "data:image/jpeg;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(jpeg)
            ));
            attached += 1;
        }
    }
    attached
}

/// Compute the padded crop rectangle in physical pixels from a normalized box.
/// `pad_frac` is applied on each side (e.g. 0.12 = 12%). Returns None for
/// degenerate or zero-area boxes, or if the frame has zero dimensions. Pure.
pub fn padded_crop_rect(
    fw: u32,
    fh: u32,
    b: &SpatialBoundingBox,
    pad_frac: f32,
) -> Option<(u32, u32, u32, u32)> {
    if fw == 0 || fh == 0 {
        return None;
    }
    let x0 = (fw as f32 * b.xmin as f32 / 1000.0).round() as i32;
    let y0 = (fh as f32 * b.ymin as f32 / 1000.0).round() as i32;
    let x1 = (fw as f32 * b.xmax as f32 / 1000.0).round() as i32;
    let y1 = (fh as f32 * b.ymax as f32 / 1000.0).round() as i32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let w = x1 - x0;
    let h = y1 - y0;
    let pad_w = (w as f32 * pad_frac).round() as i32;
    let pad_h = (h as f32 * pad_frac).round() as i32;
    let x0 = (x0 - pad_w).clamp(0, fw as i32 - 1) as u32;
    let y0 = (y0 - pad_h).clamp(0, fh as i32 - 1) as u32;
    let x1 = (x1 + pad_w).clamp(1, fw as i32) as u32;
    let y1 = (y1 + pad_h).clamp(1, fh as i32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((x0, y0, x1 - x0, y1 - y0))
}

/// Compute final dimensions for a crop:
/// - If crop width < `min_upscale_below`, upscale by 2x with Lanczos3.
/// - Then scale down (never up) to fit within `max_w` x `max_h`, preserving aspect.
/// Returns (width, height). Never returns 0 dimensions. Pure.
pub fn fit_dims(
    w: u32,
    h: u32,
    min_upscale_below: u32,
    max_w: u32,
    max_h: u32,
) -> (u32, u32) {
    let (mut tw, mut th) = (w, h);
    if w < min_upscale_below {
        tw = w.saturating_mul(2);
        th = h.saturating_mul(2);
    }
    let scale_w = max_w as f32 / tw as f32;
    let scale_h = max_h as f32 / th as f32;
    let scale = scale_w.min(scale_h).min(1.0);
    let final_w = ((tw as f32 * scale) as u32).max(1);
    let final_h = ((th as f32 * scale) as u32).max(1);
    (final_w, final_h)
}

/// Attach sharp crops from a native-resolution capture, plus a downscaled
/// overview screenshot. The old `attach_spatial_thumbnails` is kept for
/// other callers.
pub fn attach_spatial_thumbnails_native(
    payload: &mut SpatialAnalysisPayload,
    native: &image::RgbaImage,
    overview_b64_jpeg: &str,
) -> usize {
    use base64::Engine;
    let (fw, fh) = native.dimensions();
    if fw == 0 || fh == 0 {
        return 0;
    }

    // Screenshot at the top: downscale native to width <= 1400, JPEG q85.
    // If encoding fails or exceeds 900_000 bytes, fall back to overview_b64_jpeg.
    let scale_overview = (1400.0 / fw as f32).min(1.0);
    let (ow, oh) = (
        ((fw as f32 * scale_overview) as u32).max(1),
        ((fh as f32 * scale_overview) as u32).max(1),
    );
    let overview_img = image::imageops::resize(native, ow, oh, image::imageops::FilterType::Triangle);
    let mut overview_jpeg = Vec::new();
    let overview_data_url = if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut overview_jpeg, 85)
        .encode_image(&image::DynamicImage::ImageRgba8(overview_img))
        .is_ok()
        && overview_jpeg.len() <= 900_000
    {
        format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&overview_jpeg)
        )
    } else {
        format!("data:image/jpeg;base64,{overview_b64_jpeg}")
    };
    payload.screen_image_data_url = Some(overview_data_url);

    let mut attached = 0;
    for item in &mut payload.items {
        let b = &item.box_2d;
        let Some((cx, cy, cw, ch)) = padded_crop_rect(fw, fh, b, 0.12) else {
            continue;
        };
        let crop = image::imageops::crop_imm(native, cx, cy, cw, ch).to_image();
        let (mut tw, mut th) = fit_dims(crop.width(), crop.height(), 320, 640, 480);
        let mut needs_upscale = crop.width() < 320;
        let mut quality = 88;
        for attempt in 0..3 {
            let thumb = if needs_upscale {
                image::imageops::resize(&crop, tw, th, image::imageops::FilterType::Lanczos3)
            } else {
                image::imageops::resize(&crop, tw, th, image::imageops::FilterType::Triangle)
            };
            let mut jpeg = Vec::new();
            if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality)
                .encode_image(&image::DynamicImage::ImageRgba8(thumb))
                .is_ok()
                && jpeg.len() <= 120_000
            {
                item.image_data_url = Some(format!(
                    "data:image/jpeg;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&jpeg)
                ));
                attached += 1;
                break;
            }
            if attempt < 2 {
                // Next retry: halve dimensions, lower quality
                tw = (tw / 2).max(1);
                th = (th / 2).max(1);
                needs_upscale = false; // after halving, use Triangle
                quality = 65;
            }
        }
        // If still not encoded after retries, skip this item (leave image_data_url as None)
    }
    attached
}

/// Max items rendered on the stage/sidebar (clutter + token budget).
pub const SPATIAL_MAX_ITEMS: usize = 8;
/// Minimum box size in grid units — smaller is noise/degenerate.
pub const SPATIAL_MIN_UNITS: u32 = 8;

/// Spatial analysis prompt: force the Gemini 2D-grounding JSON contract.
/// Mentions the axis-grid overlay (ticks every 100 units) so the model
/// reads positions off visible anchors. Pure.
pub fn build_spatial_prompt(user_prompt: &str) -> String {
    format!(
        "You are NEXUS, a desktop spatial-vision engine. A faint red grid is overlaid on the screenshot \
every 100 units, edges running 0 (top/left) to 1000 (bottom/right) — read positions off the grid.\n\
User request: \"{}\"\n\n\
Decompose the visible content into up to {} distinct visual elements worth analysing. \
For EACH element reply with one item containing:\n\
- id: sequential 1-based number\n\
- label: a screen-grounded name (e.g. \"Revenue Chart\", \"Submit Button\", \"Person 1\")\n\
- category: one of [object, ui_element, text_block, chart, diagram, code, product, food, person, other]\n\
- box_2d: [ymin, xmin, ymax, xmax] integers 0-1000 framing the element tightly\n\
- confidence: 0.0-1.0\n\
- summary: one rich sentence of what it is / its value\n\
- details: 2-5 rows of deep specifics ({{\"title\": ..., \"value\": ...}}) — nutrition/specs/text snippets/meaning, whatever fits the content\n\n\
For a person, describe only visible attributes and actions. Never infer identity from a face. \
Use a name only when visible text explicitly associates it with that person; attribute any \
research claim to its source and say when the association is uncertain.\n\n\
Also reply with a top-level title (one line describing the screen) and overview (1-2 sentences).\n\
Reply with ONLY a raw JSON object:\n\
{{\"title\": ..., \"overview\": ..., \"items\": [ ... ]}}",
        user_prompt.trim(),
        SPATIAL_MAX_ITEMS
    )
}

/// Strip markdown fences / prose around a JSON payload. Pure.
pub(crate) fn strip_json_fences(text: &str) -> &str {
    let clean = text.trim();
    if let Some(stripped) = clean.strip_prefix("```json") {
        stripped.strip_suffix("```").unwrap_or(stripped).trim()
    } else if let Some(stripped) = clean.strip_prefix("```") {
        stripped.strip_suffix("```").unwrap_or(stripped).trim()
    } else {
        // Some models prepend prose; grab the first { ... } block.
        match (clean.find('{'), clean.rfind('}')) {
            (Some(s), Some(e)) if e > s => &clean[s..=e],
            _ => clean,
        }
    }
}

/// Validate/normalize one box: clamp into 0-1000, fix inverted edges,
/// reject degenerate boxes. Returns None when degenerate. Pure.
pub(crate) fn normalize_box(v: &serde_json::Value) -> Option<SpatialBoundingBox> {
    // Accept [ymin, xmin, ymax, xmax] array or named object.
    let (ymin, xmin, ymax, xmax) = if let Some(arr) = v.as_array() {
        if arr.len() < 4 {
            return None;
        }
        (
            arr[0].as_i64()?,
            arr[1].as_i64()?,
            arr[2].as_i64()?,
            arr[3].as_i64()?,
        )
    } else {
        (
            v.get("ymin")?.as_i64()?,
            v.get("xmin")?.as_i64()?,
            v.get("ymax")?.as_i64()?,
            v.get("xmax")?.as_i64()?,
        )
    };
    let clamp = |n: i64| -> u32 { n.clamp(0, 1000) as u32 };
    let (ymin, xmin, ymax, xmax) = (clamp(ymin), clamp(xmin), clamp(ymax), clamp(xmax));
    let (ymin, ymax) = (ymin.min(ymax), ymin.max(ymax));
    let (xmin, xmax) = (xmin.min(xmax), xmin.max(xmax));
    let b = SpatialBoundingBox { ymin, xmin, ymax, xmax };
    if b.w_units() < SPATIAL_MIN_UNITS || b.h_units() < SPATIAL_MIN_UNITS {
        return None;
    }
    Some(b)
}

/// Parse a spatial analysis response into a validated payload.
/// Renumber ids sequentially (model duplicates happen), drop degenerate
/// boxes, cap at SPATIAL_MAX_ITEMS. Pure + unit-tested.
/// Repair JSON truncated mid-array (token limit hit while listing items):
/// find the longest prefix ending in `}` that parses once `]}` is appended
/// (closing the `items` array and the root object). Pure; bounded work.
pub fn repair_truncated_json(raw: &str) -> Option<String> {
    let t = raw.trim_end();
    let mut tries = 0;
    for (idx, ch) in t.char_indices().rev() {
        if ch != '}' {
            continue;
        }
        tries += 1;
        if tries > 400 {
            break;
        }
        let candidate = format!("{}]}}", &t[..=idx]);
        if serde_json::from_str::<serde_json::Value>(&candidate).is_ok() {
            return Some(candidate);
        }
    }
    None
}

pub fn parse_spatial_json(text: &str) -> Option<SpatialAnalysisPayload> {
    let json_str = strip_json_fences(text);
    let val: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        // A reply cut off at the token limit is still mostly good: keep the
        // complete items instead of discarding the whole (billed) call.
        Err(_) => serde_json::from_str(&repair_truncated_json(json_str)?).ok()?,
    };
    let items_val = val.get("items")?.as_array()?;
    let mut items: Vec<SpatialAnalysisItem> = Vec::new();
    for raw in items_val.iter() {
        if items.len() >= SPATIAL_MAX_ITEMS {
            break;
        }
        let Some(box_2d) = normalize_box(raw.get("box_2d").unwrap_or(&serde_json::Value::Null)) else {
            continue;
        };
        let label = raw
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("Element")
            .trim()
            .to_string();
        if label.is_empty() {
            continue;
        }
        let details = raw
            .get("details")
            .and_then(|d| d.as_array())
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        Some(SpatialDetailRow {
                            title: r.get("title")?.as_str()?.trim().to_string(),
                            value: r.get("value")?.as_str()?.trim().to_string(),
                        })
                        .filter(|row| !row.title.is_empty() && !row.value.is_empty())
                    })
                    .collect()
            })
            .unwrap_or_default();
        items.push(SpatialAnalysisItem {
            id: (items.len() + 1) as u32, // renumber by pushed count
            label,
            category: raw
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("other")
                .trim()
                .to_string(),
            box_2d,
            confidence: raw.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.9) as f32,
            summary: raw
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            details,
            image_data_url: None,
            logo_data_url: None,
        });
    }
    if items.is_empty() {
        return None;
    }
    Some(SpatialAnalysisPayload {
        title: val
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Screen Analysis")
            .trim()
            .to_string(),
        overview: val
            .get("overview")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        items,
        provider_used: String::new(), // set by the engine layer
        screen_image_data_url: None,
    })
}

/// Denormalize a 0-1000 box into PHYSICAL screen pixels (clamped to the
/// monitor). Callers emit physical px; the stage frontend divides by
/// devicePixelRatio for CSS logical coords (same contract as ghost:ring).
/// Pure + unit-tested.
pub fn denormalize_spatial_box(
    b: &SpatialBoundingBox,
    screen_w: i32,
    screen_h: i32,
) -> (i32, i32, i32, i32) {
    let px = (b.xmin as i64 * screen_w.max(1) as i64 / 1000) as i32;
    let py = (b.ymin as i64 * screen_h.max(1) as i64 / 1000) as i32;
    let pw = (b.w_units() as i64 * screen_w.max(1) as i64 / 1000) as i32;
    let ph = (b.h_units() as i64 * screen_h.max(1) as i64 / 1000) as i32;
    (
        px.clamp(0, screen_w.max(1) - 1),
        py.clamp(0, screen_h.max(1) - 1),
        pw.clamp(1, screen_w.max(1)),
        ph.clamp(1, screen_h.max(1)),
    )
}

/// Pin anchor: floats just above the box's top-right corner (badge reads
/// better there and leaves the element visible), clamped on-screen.
/// Pure + unit-tested.
pub fn pin_anchor_for(
    b: &SpatialBoundingBox,
    screen_w: i32,
    screen_h: i32,
) -> (i32, i32) {
    let (px, py, pw, _) = denormalize_spatial_box(b, screen_w, screen_h);
    // Badge is ~84px wide; anchor its center near the box's top-right.
    let pin_x = (px + pw - 42).clamp(8, (screen_w - 92).max(8));
    let pin_y = (py - 30).max(8);
    let _ = screen_h;
    (pin_x, pin_y)
}

/// Per-attempt outcome for a spatial provider call.
enum SpatialStep {
    Found(SpatialAnalysisPayload),
    Miss,
    /// 429 — daily cap vs short rate limit decides the lockout.
    Quota(RateLimitKind),
    /// The provider rejected the API key (401/403/"API key not valid").
    Rejected,
}

/// Why spatial screen analysis produced nothing — drives what NEXUS says,
/// and whether the plain-text VLM retry is worth its extra seconds (it is not
/// when the cause is the key, the quota, the rate limit or a timeout: the
/// second call would fail identically and just add dead air).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialError {
    NoKeys,
    CaptureFailed,
    KeyRejected,
    RateLimited,
    QuotaSpent,
    TimedOut,
    /// Providers answered but nothing usable came back (parse miss, 404s).
    Unavailable,
}

impl std::fmt::Display for SpatialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SpatialError::NoKeys => "No vision API keys configured",
            SpatialError::CaptureFailed => "Screen capture failed",
            SpatialError::KeyRejected => "Vision API key rejected",
            SpatialError::RateLimited => "Vision provider rate limited",
            SpatialError::QuotaSpent => "Vision quota spent",
            SpatialError::TimedOut => "Vision request timed out",
            SpatialError::Unavailable => "All vision providers unavailable",
        })
    }
}

impl SpatialError {
    /// Worth a second (plain-text) VLM attempt? Only when the failure was a
    /// parse/availability miss, not a key/quota/rate/time problem.
    pub fn plain_text_retry_worthwhile(&self) -> bool {
        matches!(self, SpatialError::Unavailable)
    }

    /// One spoken clause explaining the downgrade (None = say nothing extra:
    /// the keyless/quota onboarding guidance already covers those).
    pub fn spoken_reason(&self) -> Option<&'static str> {
        match self {
            SpatialError::KeyRejected => Some(
                "Gemini rejected the key, sir — check it in Command Hub, Accounts.",
            ),
            SpatialError::RateLimited => {
                Some("Gemini is rate limiting me, sir — try again in a minute.")
            }
            SpatialError::TimedOut => Some("Gemini took too long to answer, sir."),
            _ => None,
        }
    }
}

/// Total wall-clock budget for one spatial provider attempt. Each request
/// already times out at 20s and the Gemini model ladder tries up to 3 models,
/// so a bad network used to mean a full minute of silence.
const SPATIAL_TOTAL_TIMEOUT_SECS: u64 = 25;

/// Provider attempt order for SPATIAL analysis. Gemini-only for auto:
/// Groq decommissioned every vision model (2026-10-01 live check —
/// scout/maverick 404; catalog is text/TTS/ASR only). Explicit "groq"
/// setting still attempts it for the day Groq re-adds vision. Pure.
pub fn spatial_provider_order(setting: &str) -> Vec<&'static str> {
    match setting.trim().to_lowercase().as_str() {
        "groq" => vec!["groq"],
        "gemini" => vec!["gemini"],
        _ => vec!["gemini"],
    }
}

/// Groq spatial pass: OpenAI-style chat with the gridded screenshot.
async fn spatial_groq_with_image(
    prompt: &str,
    b64: &str,
    api_key: &str,
    client: &reqwest::Client,
) -> SpatialStep {
    let data_uri = format!("data:image/jpeg;base64,{b64}");
    let body = serde_json::json!({
        "model": GROQ_VISION_MODELS[0],
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": prompt},
                {"type": "image_url", "image_url": {"url": data_uri}},
            ],
        }],
        "max_tokens": 2048,
        "temperature": 0.2,
    });
    let resp = match client
        .post("https://api.groq.com/openai/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return SpatialStep::Miss,
    };
    if !resp.status().is_success() {
        let (status, body) = failure_detail(resp).await;
        if status == 429 {
            return SpatialStep::Quota(classify_429(&body));
        }
        tracing::warn!("vision: groq spatial HTTP {status}: {}", snip(&body));
        return if is_auth_failure(status, &body) {
            SpatialStep::Rejected
        } else {
            SpatialStep::Miss
        };
    }
    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return SpatialStep::Miss,
    };
    let text = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    match parse_spatial_json(text) {
        Some(mut p) => {
            p.provider_used = "groq".to_string();
            SpatialStep::Found(p)
        }
        None => SpatialStep::Miss,
    }
}

/// Gemini spatial pass: generateContent with JSON response mime type.
/// Model ladder: GEMINI_VISION_MODEL → GEMINI_VISION_FALLBACKS (IDs
/// churn; a 404/miss advances, 429 marks quota). Keeps the SAME parsed
/// prompt across attempts.
async fn spatial_gemini_with_image(
    prompt: &str,
    b64: &str,
    api_key: &str,
    client: &reqwest::Client,
) -> SpatialStep {
    let models = default_vision_models();
    match gemini_json_with_image(prompt, b64, api_key, client, 2048, &models, parse_spatial_json).await {
        JsonStep::Found(mut p) => {
            p.provider_used = "gemini".to_string();
            SpatialStep::Found(p)
        }
        JsonStep::Quota => SpatialStep::Quota(RateLimitKind::Daily),
        JsonStep::Miss => SpatialStep::Miss,
    }
}

/// Outcome of a generic Gemini JSON-vision call.
pub(crate) enum JsonStep<T> {
    Found(T),
    Miss,
    Quota,
}

/// The long-standing spatial/grounding ladder: lite model, then fallbacks.
pub(crate) fn default_vision_models() -> Vec<String> {
    [GEMINI_VISION_MODEL]
        .into_iter()
        .chain(GEMINI_VISION_FALLBACKS.iter().copied())
        .map(String::from)
        .collect()
}

/// Narrated-tour ladder: an explicit `tourModel` first (if set), then the
/// strongest Flash models, ending on the lite model that always worked. IDs
/// churn — a 404 just advances the ladder. Pure.
pub(crate) fn tour_model_ladder(override_model: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |m: &str| {
        let m = m.trim();
        if !m.is_empty() && !out.iter().any(|x| x == m) {
            out.push(m.to_string());
        }
    };
    if let Some(m) = override_model {
        push(m);
    }
    // Live-verified 2026-10-07 ladder: the fast lite model first (1.27s),
    // then the stronger flash (2.37s, superior entity recognition), then
    // the official alias. Dead ids (3.8-flash 503, 2.5/2.0-flash 404)
    // removed — they burned ~16s of dead air per turn.
    for m in [
        GEMINI_VISION_MODEL, // gemini-3.5-flash-lite (1.27s)
        "gemini-3.5-flash",  // high entity reasoning (2.37s)
        "gemini-flash-lite-latest", // official alias (1.37s)
    ] {
        push(m);
    }
    out
}

/// How a walk down the model ladder ended. Quota only when at least one
/// model answered 429 and NO other model failed for another reason (a wrong
/// model id 404-ing must not mark the whole provider exhausted for the day).
/// Pure.
pub(crate) fn ladder_end(n_429: usize, n_other_fail: usize) -> bool {
    n_429 > 0 && n_other_fail == 0
}

/// Generic Gemini vision pass: generateContent with the JSON response mime
/// type, walking `models` in order. A 404 (unknown id) advances silently; a
/// 429 advances to the next model (Google limits per model per project, so
/// the next model often still has quota); transport/parse failures advance.
/// `parse` turns the model text into the caller's type — returning None makes
/// the ladder try the next model. Shared by spatial analysis and the
/// narrated screen tour.
pub(crate) async fn gemini_json_with_image<T>(
    prompt: &str,
    b64: &str,
    api_key: &str,
    client: &reqwest::Client,
    max_output_tokens: u32,
    models: &[String],
    parse: impl Fn(&str) -> Option<T>,
) -> JsonStep<T> {
    let body = serde_json::json!({
        "contents": [{
            "parts": [
                {"text": prompt},
                {"inline_data": {"mime_type": "image/jpeg", "data": b64}},
            ],
        }],
        "generationConfig": {
            "temperature": 0.2,
            "maxOutputTokens": max_output_tokens,
            "response_mime_type": "application/json",
        },
    });
    let (mut n_429, mut n_other) = (0usize, 0usize);
    for model in models {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
        );
        let resp = match client
            .post(&url)
            .header("x-goog-api-key", api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => {
                n_other += 1;
                continue;
            }
        };
        let status = resp.status().as_u16();
        if status == 429 {
            tracing::warn!("vision: {model} rate-limited (429) — trying the next model");
            n_429 += 1;
            continue;
        }
        if status == 404 {
            tracing::warn!("vision: model id {model} not found (404) — trying the next model");
            continue;
        }
        if !resp.status().is_success() {
            tracing::warn!("vision: {model} HTTP {status} — trying the next model");
            n_other += 1;
            continue;
        }
        let json: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(_) => {
                n_other += 1;
                continue;
            }
        };
        let text: String = json["candidates"][0]["content"]["parts"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text")?.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        if let Some(v) = parse(&text) {
            tracing::info!("vision: answered by {model}");
            return JsonStep::Found(v);
        }
        tracing::warn!("vision: {model} returned an unusable answer — trying the next model");
        n_other += 1;
    }
    if ladder_end(n_429, n_other) {
        JsonStep::Quota
    } else {
        JsonStep::Miss
    }
}

/// Spatial screen analysis (Feature 86): capture the gridded screenshot,
/// ask Gemini Flash-Lite (PRIMARY) for normalized 2D boxes + deep cards,
/// fall back to Groq Llama-4-Scout on 429/quota/miss. Zero local RAM —
/// pure cloud + the OS-native capture. Uses the same quota ledger as
/// ghost vision grounding (vision_usage.json).
pub async fn analyze_screen_spatial<R: Runtime>(
    app: &tauri::AppHandle<R>,
    user_prompt: &str,
    target: &crate::screen_context::ScreenContext,
    on_captured: impl FnOnce() + Send,
) -> Result<SpatialAnalysisPayload, SpatialError> {
    let (sw, sh) = crate::screen::primary_monitor_size().ok_or(SpatialError::CaptureFailed)?;
    if !crate::screen_context::foreground_still_matches(target, sw, sh).await {
        return Err(SpatialError::CaptureFailed);
    }
    let region = target.region;
    let Some(b64) = capture_gridded_region_jpeg_base64_w(region, VISION_MAX_W) else {
        return Err(SpatialError::CaptureFailed);
    };
    // The screenshot is taken: only now is it safe to speak ("Analyzing…" and
    // its word-by-word caption are stage overlays that BitBlt would photograph).
    on_captured();
    analyze_screen_spatial_image(app, user_prompt, &b64, true).await
}

/// Analyse a frame already captured by the orchestrator. This lets a saved
/// foreground review use one clean frame for its hero image and item crops.
pub async fn analyze_screen_spatial_image<R: Runtime>(
    app: &tauri::AppHandle<R>, user_prompt: &str, b64: &str, gridded: bool,
) -> Result<SpatialAnalysisPayload, SpatialError> {
    let groq_key = crate::commands::read_groq_api_key(app);
    let gemini_key = crate::commands::read_api_key(app, "gemini");
    if groq_key.is_empty() && gemini_key.is_empty() {
        return Err(SpatialError::NoKeys);
    }
    let usage_dir = app
        .path()
        .app_data_dir()
        .map_err(|_: tauri::Error| SpatialError::CaptureFailed)?;
    // Feature 98 P6: identical screen within 60s replays the cached answer
    // (<30ms, zero quota burn).
    let hash = frame_hash(&b64);
    if let Some(cached) = frame_cache_get(hash) {
        tracing::info!("vision: spatial cache hit (unchanged screen) — 0 quota");
        return Ok(cached);
    }
    let prompt = if gridded {
        build_spatial_prompt(user_prompt)
    } else {
        build_spatial_prompt(user_prompt).replace(
            "A faint red grid is overlaid on the screenshot every 100 units, edges running 0 (top/left) to 1000 (bottom/right) — read positions off the grid.",
            "The screenshot has no grid. Return positions normalized to 0-1000 from top-left to bottom-right.",
        )
    };
    // Feature 98: pooled shared client (Phase 5) + clamped timeout 20s → 4s.
    let client = shared_vision_client();

    let setting = read_vision_provider(&usage_dir);
    // Strongest reason seen across providers; reported if none succeeds.
    let mut why = SpatialError::Unavailable;
    for provider in spatial_provider_order(&setting) {
        let (key, label): (&str, &'static str) = match provider {
            "groq" => (groq_key.as_str(), "groq"),
            _ => (gemini_key.as_str(), "gemini"),
        };
        if key.is_empty() {
            continue;
        }
        if exhausted(&usage_dir, label) {
            why = SpatialError::QuotaSpent;
            continue;
        }
        if on_cooldown(label) {
            why = SpatialError::RateLimited;
            continue;
        }
        let attempt = async {
            match provider {
                "groq" => spatial_groq_with_image(&prompt, &b64, key, &client).await,
                _ => spatial_gemini_with_image(&prompt, &b64, key, &client).await,
            }
        };
        let step = match tokio::time::timeout(
            std::time::Duration::from_secs(SPATIAL_TOTAL_TIMEOUT_SECS),
            attempt,
        )
        .await
        {
            Ok(step) => step,
            Err(_) => {
                tracing::warn!("vision: {label} spatial attempt exceeded {SPATIAL_TOTAL_TIMEOUT_SECS}s");
                why = SpatialError::TimedOut;
                continue;
            }
        };
        match step {
            SpatialStep::Found(mut payload) => {
                record_use(&usage_dir, label);
                attach_spatial_thumbnails(&mut payload, &b64);
                frame_cache_put(hash, payload.clone());
                tracing::info!(
                    "vision: spatial analysis via {} ({} items)",
                    label,
                    payload.items.len()
                );
                return Ok(payload);
            }
            SpatialStep::Quota(kind) => {
                apply_rate_limit(&usage_dir, label, kind);
                why = match kind {
                    RateLimitKind::Daily => SpatialError::QuotaSpent,
                    RateLimitKind::Rate => SpatialError::RateLimited,
                };
                continue;
            }
            SpatialStep::Rejected => {
                why = SpatialError::KeyRejected;
                continue;
            }
            SpatialStep::Miss => continue,
        }
    }
    Err(why)
}

#[cfg(test)]
mod spatial_tests {
    use super::*;

    #[test]
    fn thumbnails_are_cropped_from_the_analysed_frame() {
        use base64::Engine;
        let mut frame = image::RgbImage::new(100, 50);
        for (x, _, pixel) in frame.enumerate_pixels_mut() {
            *pixel = if x < 50 { image::Rgb([240, 10, 10]) } else { image::Rgb([10, 10, 240]) };
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&image::DynamicImage::ImageRgb8(frame))
            .unwrap();
        let source = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let mut payload = parse_spatial_json(
            r#"{"items":[{"label":"Left","box_2d":[0,0,1000,400]},{"label":"Right","box_2d":[0,600,1000,1000]}]}"#,
        ).unwrap();
        assert_eq!(attach_spatial_thumbnails(&mut payload, &source), 2);
        let crop_bytes = |index: usize| {
            let uri = payload.items[index].image_data_url.as_ref().unwrap();
            assert!(uri.starts_with("data:image/jpeg;base64,"));
            let encoded = uri.strip_prefix("data:image/jpeg;base64,").unwrap();
            base64::engine::general_purpose::STANDARD.decode(encoded).unwrap()
        };
        let left = image::load_from_memory(&crop_bytes(0)).unwrap().to_rgb8();
        let right = image::load_from_memory(&crop_bytes(1)).unwrap().to_rgb8();
        assert!(left.get_pixel(10, 10)[0] > left.get_pixel(10, 10)[2]);
        assert!(right.get_pixel(10, 10)[2] > right.get_pixel(10, 10)[0]);
        assert_eq!(attach_spatial_thumbnails(&mut payload, "invalid"), 0);
    }

    #[test]
    fn test_has_usable_vision_key_follows_provider_order() {
        // auto / gemini: only a Gemini key counts (Groq vision is gone).
        assert!(has_usable_vision_key("auto", "", "g-key"));
        assert!(!has_usable_vision_key("auto", "groq-key", ""));
        assert!(!has_usable_vision_key("gemini", "groq-key", ""));
        assert!(!has_usable_vision_key("auto", "", ""));
        // Explicit groq setting still uses the Groq key.
        assert!(has_usable_vision_key("groq", "groq-key", ""));
        assert!(!has_usable_vision_key("groq", "", "g-key"));
    }

    #[test]
    fn test_classify_429_daily_vs_rate() {
        let daily = r#"{"error":{"code":429,"details":[{"violations":[{"quotaId":"GenerateRequestsPerDayPerProjectPerModel-FreeTier"}]}]}}"#;
        let rate = r#"{"error":{"code":429,"details":[{"violations":[{"quotaId":"GenerateContentRequestsPerMinutePerProjectPerModel-FreeTier"}]}]}}"#;
        assert_eq!(classify_429(daily), RateLimitKind::Daily);
        assert_eq!(classify_429(rate), RateLimitKind::Rate);
        assert_eq!(classify_429("Resource exhausted"), RateLimitKind::Rate);
        assert_eq!(classify_429(""), RateLimitKind::Rate);
    }

    #[test]
    fn test_is_auth_failure_matrix() {
        assert!(is_auth_failure(401, ""));
        assert!(is_auth_failure(403, "permission denied"));
        assert!(is_auth_failure(400, "API key not valid. Please pass a valid API key."));
        assert!(is_auth_failure(400, "API_KEY_INVALID"));
        assert!(!is_auth_failure(400, "Invalid JSON payload"));
        assert!(!is_auth_failure(404, "model not found"));
        assert!(!is_auth_failure(429, "quota"));
        assert!(!is_auth_failure(500, ""));
    }

    #[test]
    fn test_rate_cooldown_is_short_lived_and_per_provider() {
        set_cooldown("test-provider-a", 60);
        assert!(on_cooldown("test-provider-a"));
        assert!(!on_cooldown("test-provider-b"));
        set_cooldown("test-provider-c", 0);
        assert!(!on_cooldown("test-provider-c"));
    }

    #[test]
    fn test_rate_limit_does_not_lock_the_day() {
        let dir = std::env::temp_dir().join(format!("nexus-vision-rl-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        apply_rate_limit(&dir, "gemini", RateLimitKind::Rate);
        assert!(!exhausted(&dir, "gemini"), "an RPM blip must not spend the day");
        assert!(unavailable(&dir, "gemini"));
        apply_rate_limit(&dir, "gemini", RateLimitKind::Daily);
        assert!(exhausted(&dir, "gemini"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_spatial_error_policy() {
        assert!(SpatialError::Unavailable.plain_text_retry_worthwhile());
        for e in [
            SpatialError::NoKeys,
            SpatialError::KeyRejected,
            SpatialError::RateLimited,
            SpatialError::QuotaSpent,
            SpatialError::TimedOut,
        ] {
            assert!(!e.plain_text_retry_worthwhile(), "{e:?}");
        }
        assert!(SpatialError::KeyRejected.spoken_reason().is_some());
        assert!(SpatialError::RateLimited.spoken_reason().is_some());
        assert!(SpatialError::TimedOut.spoken_reason().is_some());
        // Keyless / quota-spent are covered by the onboarding guidance.
        assert!(SpatialError::NoKeys.spoken_reason().is_none());
        assert!(SpatialError::QuotaSpent.spoken_reason().is_none());
    }

    #[test]
    fn test_repair_truncated_json_keeps_complete_items() {
        let cut = r#"{"title":"T","overview":"O","items":[
            {"id":1,"label":"A","box_2d":[10,20,30,40],"details":[{"title":"x","value":"y"}]},
            {"id":2,"label":"B","box_2d":[50,60,70,80],"details":[{"title":"x","va"#;
        let p = parse_spatial_json(cut).expect("truncated reply should be repaired");
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].label, "A");
        // Nothing salvageable stays None.
        assert!(parse_spatial_json("{\"items\": [{\"label\": \"A\", \"box_2d").is_none());
        assert!(repair_truncated_json("not json").is_none());
    }

    #[test]
    fn test_spatial_prompt_contract() {
        let p = build_spatial_prompt("analyse the screen for me");
        assert!(p.contains("analyse the screen for me"));
        assert!(p.contains("ymin, xmin, ymax, xmax"));
        assert!(p.contains("0-1000"));
        assert!(p.contains("box_2d"));
    }

    #[test]
    fn test_parse_spatial_json_with_fences() {
        let raw = "```json\n{\"title\": \"Dry Fruits Catalog\", \"overview\": \"Three nuts\", \
            \"items\": [{\"id\": 1, \"label\": \"Almonds\", \"category\": \"food\", \
            \"box_2d\": [180, 240, 420, 510], \"confidence\": 0.95, \
            \"summary\": \"160 kcal, high vitamin E\", \
            \"details\": [{\"title\": \"Protein\", \"value\": \"6g\"}]}]}\n```";
        let p = parse_spatial_json(raw).unwrap();
        assert_eq!(p.title, "Dry Fruits Catalog");
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].label, "Almonds");
        assert_eq!(p.items[0].id, 1);
        assert_eq!(p.items[0].box_2d, SpatialBoundingBox { ymin: 180, xmin: 240, ymax: 420, xmax: 510 });
        assert_eq!(p.items[0].details[0].value, "6g");
    }

    #[test]
    fn test_parse_spatial_json_prose_wrapped() {
        let raw = "Here is the analysis: {\"title\": \"T\", \"items\": [{\"label\": \"Btn\", \
            \"box_2d\": [10, 20, 40, 80]}]} hope that helps";
        let p = parse_spatial_json(raw).unwrap();
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].label, "Btn");
        assert_eq!(p.items[0].id, 1); // renumbered sequentially
        assert_eq!(p.items[0].category, "other"); // default
        assert!((p.items[0].confidence - 0.9).abs() < 1e-6); // default
    }

    #[test]
    fn test_parse_spatial_box_order_fixed_and_clamped() {
        // Inverted + out-of-range edges get normalized.
        let raw = "{\"items\": [{\"label\": \"X\", \"box_2d\": [900, -50, 400, 1200]}]}";
        let p = parse_spatial_json(raw).unwrap();
        let b = &p.items[0].box_2d;
        assert_eq!((b.ymin, b.xmin, b.ymax, b.xmax), (400, 0, 900, 1000));
    }

    #[test]
    fn test_parse_spatial_degenerate_box_dropped() {
        // 5x5 units — below SPATIAL_MIN_UNITS → item dropped entirely.
        let raw = "{\"items\": [{\"label\": \"Dot\", \"box_2d\": [10, 10, 15, 15]}, \
            {\"label\": \"Real\", \"box_2d\": [100, 100, 300, 400]}]}";
        let p = parse_spatial_json(raw).unwrap();
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].label, "Real");
        assert_eq!(p.items[0].id, 1); // renumbered
    }

    #[test]
    fn test_parse_spatial_cap_and_renumber() {
        let mut items = String::new();
        for i in 0..12 {
            items.push_str(&format!(
                "{{\"label\": \"I{i}\", \"box_2d\": [{}, 100, {}, 300]}},",
                i * 40 + 10,
                i * 40 + 30
            ));
        }
        let raw = format!("{{\"items\": [{}]}}", items.trim_end_matches(','));
        let p = parse_spatial_json(&raw).unwrap();
        assert_eq!(p.items.len(), SPATIAL_MAX_ITEMS);
        for (n, item) in p.items.iter().enumerate() {
            assert_eq!(item.id, (n + 1) as u32);
        }
    }

    #[test]
    fn test_parse_spatial_garbage_rejected() {
        assert!(parse_spatial_json("no json here").is_none());
        assert!(parse_spatial_json("").is_none());
        assert!(parse_spatial_json("{\"items\": []}").is_none());
        assert!(parse_spatial_json("{\"items\": [{\"label\": \"NoBox\"}]}").is_none());
    }

    #[test]
    fn test_denormalize_spatial_box() {
        let b = SpatialBoundingBox { ymin: 100, xmin: 500, ymax: 500, xmax: 1000 };
        // 1920x1080: x = 500/1000*1920 = 960, y = 100/1000*1080 = 108,
        // w = 500/1000*1920 = 960, h = 400/1000*1080 = 432
        assert_eq!(denormalize_spatial_box(&b, 1920, 1080), (960, 108, 960, 432));
    }

    #[test]
    fn test_pin_anchor_above_top_right() {
        let b = SpatialBoundingBox { ymin: 100, xmin: 500, ymax: 500, xmax: 1000 };
        let (px, py, pw, _) = denormalize_spatial_box(&b, 1920, 1080);
        let (ax, ay) = pin_anchor_for(&b, 1920, 1080);
        assert_eq!(ax, (px + pw - 42).clamp(8, 1920 - 92));
        assert_eq!(ay, (py - 30).max(8));
    }

    #[test]
    fn test_pin_anchor_clamped_on_screen() {
        // Box near the top edge → pin clamps to 8, never negative.
        let b = SpatialBoundingBox { ymin: 2, xmin: 2, ymax: 100, xmax: 100 };
        let (ax, ay) = pin_anchor_for(&b, 1920, 1080);
        assert!(ax >= 8 && ay >= 8);
    }

    #[test]
    fn test_spatial_provider_order_gemini_primary() {
        // Groq vision decommissioned (2026-10-01): auto = Gemini only.
        assert_eq!(spatial_provider_order("auto"), vec!["gemini"]);
        assert_eq!(spatial_provider_order(""), vec!["gemini"]);
        assert_eq!(spatial_provider_order("GROQ"), vec!["groq"]);
        assert_eq!(spatial_provider_order("gemini"), vec!["gemini"]);
    }

    #[test]
    fn test_padded_crop_rect_normal() {
        let b = SpatialBoundingBox { ymin: 200, xmin: 300, ymax: 600, xmax: 700 };
        // 1000x1000 frame, box at (300,200)-(700,600) = 400x400, 12% pad = 48 each side
        let rect = padded_crop_rect(1000, 1000, &b, 0.12).unwrap();
        assert_eq!(rect, (252, 152, 496, 496)); // clamped to frame
    }

    #[test]
    fn test_padded_crop_rect_edge_clamped() {
        let b = SpatialBoundingBox { ymin: 0, xmin: 0, ymax: 100, xmax: 100 };
        // At top-left edge, padding should clamp to 0
        let rect = padded_crop_rect(1000, 1000, &b, 0.12).unwrap();
        assert_eq!(rect.0, 0); // x0 clamped
        assert_eq!(rect.1, 0); // y0 clamped
        assert!(rect.2 > 100); // width expanded by padding on right/bottom
        assert!(rect.3 > 100);
    }

    #[test]
    fn test_padded_crop_rect_zero_area_none() {
        let b = SpatialBoundingBox { ymin: 100, xmin: 100, ymax: 100, xmax: 100 };
        assert!(padded_crop_rect(1000, 1000, &b, 0.12).is_none());
    }

    #[test]
    fn test_padded_crop_rect_inverted_none() {
        let b = SpatialBoundingBox { ymin: 500, xmin: 500, ymax: 100, xmax: 100 };
        assert!(padded_crop_rect(1000, 1000, &b, 0.12).is_none());
    }

    #[test]
    fn test_padded_crop_rect_zero_frame_none() {
        let b = SpatialBoundingBox { ymin: 100, xmin: 100, ymax: 200, xmax: 200 };
        assert!(padded_crop_rect(0, 1000, &b, 0.12).is_none());
        assert!(padded_crop_rect(1000, 0, &b, 0.12).is_none());
    }

    #[test]
    fn test_fit_dims_upscale() {
        // 100x60 crop -> upscaled x2 = 200x120
        assert_eq!(fit_dims(100, 60, 320, 640, 480), (200, 120));
    }

    #[test]
    fn test_fit_dims_no_upscale() {
        // 400x300 stays (above 320, fits in 640x480)
        assert_eq!(fit_dims(400, 300, 320, 640, 480), (400, 300));
    }

    #[test]
    fn test_fit_dims_downscale() {
        // 2000x1000 -> 640x320 (aspect preserved)
        assert_eq!(fit_dims(2000, 1000, 320, 640, 480), (640, 320));
    }

    #[test]
    fn test_fit_dims_aspect_preserved() {
        // 500x100: width >= 320 so no upscale, fits in 640x480 so stays 500x100
        let (w, h) = fit_dims(500, 100, 320, 640, 480);
        assert_eq!((w, h), (500, 100));
        // Aspect ratio preserved: 500/100 = 5
        assert!((w as f32 / h as f32 - 5.0).abs() < 0.01);
        // Test a case that actually downscales: 2000x1000 -> 640x320 (aspect 2:1)
        let (w2, h2) = fit_dims(2000, 1000, 320, 640, 480);
        assert_eq!((w2, h2), (640, 320));
        assert!((w2 as f32 / h2 as f32 - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_fit_dims_never_zero() {
        assert_eq!(fit_dims(1, 1, 320, 640, 480), (2, 2));
        assert_eq!(fit_dims(10, 1, 320, 640, 480), (20, 2));
    }

    #[test]
    fn test_attach_spatial_thumbnails_native_sharp_crops() {
        use base64::Engine;
        // Synthetic 2000x1200 gradient image
        let mut native = image::RgbaImage::new(2000, 1200);
        for (x, y, pixel) in native.enumerate_pixels_mut() {
            let r = ((x as f32 / 2000.0) * 255.0) as u8;
            let g = ((y as f32 / 1200.0) * 255.0) as u8;
            *pixel = image::Rgba([r, g, 128, 255]);
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&image::DynamicImage::ImageRgba8(native.clone()))
            .unwrap();
        let overview_b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);

        let mut payload = parse_spatial_json(
            r#"{"items":[{"label":"Left","box_2d":[0,0,1000,500]},{"label":"Right","box_2d":[0,500,1000,1000]}]}"#,
        ).unwrap();
        let attached = attach_spatial_thumbnails_native(&mut payload, &native, &overview_b64);
        assert_eq!(attached, 2);
        for item in &payload.items {
            let uri = item.image_data_url.as_ref().unwrap();
            assert!(uri.starts_with("data:image/jpeg;base64,"));
            let encoded = uri.strip_prefix("data:image/jpeg;base64,").unwrap();
            let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
            assert!(bytes.len() <= 120_000, "thumb too large: {} bytes", bytes.len());
            let img = image::load_from_memory(&bytes).unwrap();
            assert!(img.width() <= 640, "thumb width {} > 640", img.width());
        }
        // screen_image_data_url width == 1400
        let screen_uri = payload.screen_image_data_url.as_ref().unwrap();
        let screen_encoded = screen_uri.strip_prefix("data:image/jpeg;base64,").unwrap();
        let screen_bytes = base64::engine::general_purpose::STANDARD.decode(screen_encoded).unwrap();
        let screen_img = image::load_from_memory(&screen_bytes).unwrap();
        assert_eq!(screen_img.width(), 1400);
    }

    #[test]
    fn test_attach_spatial_thumbnails_native_degenerate_skipped() {
        use base64::Engine;
        let native = image::RgbaImage::new(2000, 1200);
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&image::DynamicImage::ImageRgba8(native.clone()))
            .unwrap();
        let overview_b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);

        // One valid, one degenerate (zero-area) box
        let mut payload = parse_spatial_json(
            r#"{"items":[{"label":"Valid","box_2d":[100,100,300,300]},{"label":"Degenerate","box_2d":[500,500,500,500]}]}"#,
        ).unwrap();
        let attached = attach_spatial_thumbnails_native(&mut payload, &native, &overview_b64);
        assert_eq!(attached, 1);
        assert!(payload.items[0].image_data_url.is_some());
        // Degenerate item should not have image_data_url
    }

    #[test]
    fn test_attach_spatial_thumbnails_native_sharpness_vs_old() {
        use base64::Engine;
        // Build a 2000x1200 image with a fine 2-px black/white checker
        // inside one item's box (center region ~400x400 px at native)
        let mut native = image::RgbaImage::new(2000, 1200);
        for (x, y, pixel) in native.enumerate_pixels_mut() {
            // Checker pattern in center region (600-1000, 300-700)
            if x >= 600 && x < 1000 && y >= 300 && y < 700 {
                let checker = ((x / 2) + (y / 2)) % 2;
                *pixel = if checker == 0 {
                    image::Rgba([0, 0, 0, 255])
                } else {
                    image::Rgba([255, 255, 255, 255])
                };
            } else {
                *pixel = image::Rgba([128, 128, 128, 255]);
            }
        }
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
            .encode_image(&image::DynamicImage::ImageRgba8(native.clone()))
            .unwrap();
        let overview_b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg);

        // Native function
        let mut payload_native = parse_spatial_json(
            r#"{"items":[{"label":"Checker","box_2d":[300,150,583,500]}]}"#,
        ).unwrap();
        attach_spatial_thumbnails_native(&mut payload_native, &native, &overview_b64);
        let native_uri = payload_native.items[0].image_data_url.as_ref().unwrap();
        let native_encoded = native_uri.strip_prefix("data:image/jpeg;base64,").unwrap();
        let native_bytes = base64::engine::general_purpose::STANDARD.decode(native_encoded).unwrap();
        let native_img = image::load_from_memory(&native_bytes).unwrap().to_luma8();

        // Old function: downscale to 768px first, then crop
        let scale_768 = 768.0 / 2000.0;
        let (tw, th) = ((2000.0 * scale_768) as u32, (1200.0 * scale_768) as u32);
        let small = image::imageops::resize(&native, tw, th, image::imageops::FilterType::Triangle);
        let mut small_jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut small_jpeg, 70)
            .encode_image(&image::DynamicImage::ImageRgba8(small))
            .unwrap();
        let small_b64 = base64::engine::general_purpose::STANDARD.encode(&small_jpeg);
        let mut payload_old = parse_spatial_json(
            r#"{"items":[{"label":"Checker","box_2d":[300,150,583,500]}]}"#,
        ).unwrap();
        attach_spatial_thumbnails(&mut payload_old, &small_b64);
        let old_uri = payload_old.items[0].image_data_url.as_ref().unwrap();
        let old_encoded = old_uri.strip_prefix("data:image/jpeg;base64,").unwrap();
        let old_bytes = base64::engine::general_purpose::STANDARD.decode(old_encoded).unwrap();
        let old_img = image::load_from_memory(&old_bytes).unwrap().to_luma8();

        // Compute mean absolute neighbour-pixel difference (local contrast)
        fn mean_neighbour_diff(img: &image::GrayImage) -> f32 {
            let (w, h) = img.dimensions();
            if w < 2 || h < 2 {
                return 0.0;
            }
            let mut sum = 0u64;
            let mut count = 0u64;
            for y in 0..h {
                for x in 0..w - 1 {
                    let diff = (img.get_pixel(x, y)[0] as i16 - img.get_pixel(x + 1, y)[0] as i16).abs() as u64;
                    sum += diff;
                    count += 1;
                }
            }
            for y in 0..h - 1 {
                for x in 0..w {
                    let diff = (img.get_pixel(x, y)[0] as i16 - img.get_pixel(x, y + 1)[0] as i16).abs() as u64;
                    sum += diff;
                    count += 1;
                }
            }
            if count == 0 {
                return 0.0;
            }
            (sum as f32 / count as f32) / 255.0
        }

        let native_contrast = mean_neighbour_diff(&native_img);
        let old_contrast = mean_neighbour_diff(&old_img);
        // Native must have clearly higher contrast (> 20/255 = ~0.078 more)
        assert!(native_contrast > old_contrast + 20.0 / 255.0,
            "native contrast {} not > old contrast {} + 0.078", native_contrast, old_contrast);
    }
}

#[cfg(test)]
mod tour_ladder_tests {
    use super::*;

    #[test]
    fn tour_ladder_leads_with_the_fast_lite_model_and_has_no_dead_ids() {
        let l = tour_model_ladder(None);
        // Live-verified 2026-10-07: the 1.27s lite model leads; the stronger
        // flash (2.37s) is second; the official alias last.
        assert_eq!(l[0], GEMINI_VISION_MODEL);
        assert_eq!(l[1], "gemini-3.5-flash");
        assert_eq!(l[2], "gemini-flash-lite-latest");
        // Dead models (503/404) must never re-enter the ladder.
        assert!(!l.iter().any(|m| m == "gemini-3.8-flash"));
        assert!(!l.iter().any(|m| m == "gemini-2.5-flash"));
        assert!(!l.iter().any(|m| m == "gemini-2.0-flash"));
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn tour_ladder_honours_an_override_without_duplicates() {
        let l = tour_model_ladder(Some("gemini-3.7-flash"));
        assert_eq!(l[0], "gemini-3.7-flash");
        assert_eq!(l.len(), 4);
        // Overriding with a model already in the ladder just moves it first.
        let l2 = tour_model_ladder(Some("gemini-3.5-flash"));
        assert_eq!(l2[0], "gemini-3.5-flash");
        assert_eq!(l2.iter().filter(|m| *m == "gemini-3.5-flash").count(), 1);
        // Blank override is ignored.
        assert_eq!(tour_model_ladder(Some("  ")), tour_model_ladder(None));
    }

    #[test]
    fn default_ladder_is_the_verified_fast_one() {
        let l = default_vision_models();
        assert_eq!(l[0], GEMINI_VISION_MODEL);
        assert_eq!(l[1], "gemini-3.5-flash");
        assert_eq!(l[2], "gemini-flash-lite-latest");
        assert_eq!(l.len(), 1 + GEMINI_VISION_FALLBACKS.len());
    }

    #[test]
    fn ladder_end_marks_quota_only_when_every_failure_was_a_429() {
        assert!(ladder_end(1, 0)); // one model rate-limited, the rest 404 → quota
        assert!(ladder_end(3, 0));
        assert!(!ladder_end(0, 0)); // nothing answered at all → plain miss
        assert!(!ladder_end(2, 1)); // a real failure mixed in → don't burn the day's quota
        assert!(!ladder_end(0, 3));
    }

    #[test]
    fn daily_limit_comes_from_settings_with_a_safe_default() {
        assert_eq!(parse_daily_limit(None), GEMINI_VISION_RPD);
        assert_eq!(parse_daily_limit(Some("{}")), GEMINI_VISION_RPD);
        assert_eq!(parse_daily_limit(Some("not json")), GEMINI_VISION_RPD);
        assert_eq!(parse_daily_limit(Some("{\"geminiVisionDailyLimit\": 0}")), GEMINI_VISION_RPD);
        assert_eq!(parse_daily_limit(Some("{\"geminiVisionDailyLimit\": 1500}")), 1500);
        assert_eq!(parse_daily_limit(Some("{\"geminiVisionDailyLimit\": 99999999999}")), 1_000_000);
        assert_eq!(parse_daily_limit(Some("{\"geminiVisionDailyLimit\": \"x\"}")), GEMINI_VISION_RPD);
    }

    #[test]
    fn exhausted_uses_the_configured_limit() {
        let dir = std::env::temp_dir().join(format!("nexus_vis_limit_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("settings.json"), "{\"geminiVisionDailyLimit\": 3}").unwrap();
        let _ = std::fs::remove_file(dir.join(USAGE_FILE));
        for _ in 0..2 {
            record_use(&dir, "gemini");
        }
        assert!(!exhausted(&dir, "gemini"));
        record_use(&dir, "gemini");
        assert!(exhausted(&dir, "gemini")); // 3 of 3
        assert_eq!(quota_status(&dir)["gemini"]["limit"], 3);
        // mark_exhausted lifts the counter to the configured limit, never below.
        let _ = std::fs::remove_file(dir.join(USAGE_FILE));
        mark_exhausted(&dir, "gemini");
        assert_eq!(quota_status(&dir)["gemini"]["used"], 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
