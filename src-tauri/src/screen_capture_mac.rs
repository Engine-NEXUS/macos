//! macOS screen capture — Phase 2 v0.
//!
//! `screencapture -x -R x,y,w,h out.png` fills the same seam
//! `sidebar_backdrop::capture_region_bgra_public` exposes on Windows; a
//! ScreenCaptureKit fast path can replace the exec call without touching
//! callers. The Screen Recording permission (TCC) gates the first run.
//!
//! Coordinates on this path are macOS POINTS and the returned image is
//! resized back to the requested (points) size, so the vision stack's
//! geometry stays consistent on Retina — physical pixels are folded into
//! points here instead of multiplying the callers' coordinate math.

use image::GenericImageView;
use std::path::PathBuf;
use std::sync::OnceLock;

static PRIMARY_POINTS: OnceLock<(i32, i32)> = OnceLock::new();

/// Primary display size in points, probed once. The Finder desktop window
/// covers the main display on stock setups; when the probe is refused
/// (Automation permission), fall back to the historical default 1440x900.
pub(crate) fn primary_points() -> (i32, i32) {
    *PRIMARY_POINTS.get_or_init(|| {
        let size = std::process::Command::new("osascript")
            .arg("-e")
            .arg("tell application \"System Events\" to get size of front window of process \"Finder\"")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .and_then(|s| parse_size_query(&s));
        size.unwrap_or((1440, 900))
    })
}

/// `"1920, 1080"` (osascript size output) → `(1920, 1080)`.
fn parse_size_query(s: &str) -> Option<(i32, i32)> {
    let mut parts = s.split(',').map(|x| x.trim().parse::<f64>().ok());
    let w = parts.next()?.and_then(dimension_to_i32)?;
    let h = parts.next()?.and_then(dimension_to_i32)?;
    if w <= 0 || h <= 0 {
        return None;
    }
    Some((w, h))
}

/// A size read from osascript (`1440` or `1440.0`) as whole points. `i32::try_from(f64)` does not exist, so
/// range-check and round explicitly; NaN / infinity / out-of-range values are rejected.
fn dimension_to_i32(v: f64) -> Option<i32> {
    (v.is_finite() && v >= f64::from(i32::MIN) && v <= f64::from(i32::MAX)).then(|| v.round() as i32)
}

/// BGRA of an on-screen rectangle in points. None when the permission is
/// missing or the capture fails — callers degrade to None, never crash.
pub(crate) fn capture_bgra(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    let img = capture_native(x, y, w, h)?;
    let rgba = if img.dimensions() == (w as u32, h as u32) {
        img.to_rgba8()
    } else {
        image::imageops::resize(&img, w as u32, h as u32, image::imageops::FilterType::Triangle)
    };
    Some(rgba_to_bgra(&rgba))
}

/// An on-screen rectangle given in points, at the display's native resolution (2x on Retina, not resized). OCR reads
/// small text far better at native pixels; map results back to points with the width ratio.
pub(crate) fn capture_native(x: i32, y: i32, w: i32, h: i32) -> Option<image::DynamicImage> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let png: PathBuf = std::env::temp_dir().join(format!("nexus-cap-{}.png", std::process::id()));
    let _ = std::fs::remove_file(&png);
    let out = std::process::Command::new("screencapture")
        .args(["-x", "-t", "png", "-R"])
        .arg(format!("{x},{y},{w},{h}"))
        .arg(&png)
        .output()
        .ok()?;
    if !out.status.success() {
        tracing::warn!(
            "screen_capture_mac: screencapture failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let img = image::ImageReader::open(&png).ok()?.decode().ok()?;
    let _ = std::fs::remove_file(&png);
    Some(img)
}

/// The Windows callers' contract is BGRA (B first); PNG decodes RGBA, so
/// swap R/B while collecting.
fn rgba_to_bgra(rgba: &image::RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.as_raw().len());
    for px in rgba.as_raw().chunks_exact(4) {
        out.push(px[2]);
        out.push(px[1]);
        out.push(px[0]);
        out.push(px[3]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{dimension_to_i32, parse_size_query};

    #[test]
    fn size_query_parses_osascript_output() {
        assert_eq!(parse_size_query("1512, 982"), Some((1512, 982)));
        assert_eq!(parse_size_query("0, 0"), None);
        assert_eq!(parse_size_query(""), None);
        assert_eq!(parse_size_query("abc"), None);
    }

    #[test]
    fn size_query_accepts_decimal_output_and_rejects_bad_numbers() {
        assert_eq!(parse_size_query("1440.0, 900.0"), Some((1440, 900)));
        assert_eq!(parse_size_query("1440.6, 899.4"), Some((1441, 899)));
        assert_eq!(parse_size_query("-5, 900"), None);
        assert_eq!(dimension_to_i32(f64::NAN), None);
        assert_eq!(dimension_to_i32(f64::INFINITY), None);
        assert_eq!(dimension_to_i32(1e12), None);
        assert_eq!(dimension_to_i32(1920.0), Some(1920));
    }
}
