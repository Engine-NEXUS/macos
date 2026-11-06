//! macOS screen capture — Phase 2 v0.
//!
//! `screencapture -x -R x,y,w,h out.png` fills the same seam
//! `sidebar_backdrop::capture_region_bgra_public` exposes on Windows; a
//! ScreenCaptureKit fast path can replace the exec call without touching
//! callers. Capture is skipped (None) while the Screen Recording permission
//! is missing, so it never re-triggers the macOS prompt or hands callers a
//! wallpaper-only image.
//!
//! Coordinates on this path are macOS POINTS and the returned image is
//! resized back to the requested (points) size, so the vision stack's
//! geometry stays consistent on Retina — physical pixels are folded into
//! points here instead of multiplying the callers' coordinate math.

use crate::ax_mac::mac::{CGPoint, CGSize};
use image::GenericImageView;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Size used when the display query fails: the historical default.
const FALLBACK_POINTS: (i32, i32) = (1440, 900);

/// Core Graphics `CGRect`: origin and size in points, four `f64` (CGFloat is a double on 64-bit macOS).
#[repr(C)]
#[derive(Default, Debug, Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

// Quartz Display Services. Neither call needs a privacy permission.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(display: u32) -> CGRect;
}

/// Primary (main) display size in points, the space `screencapture -R` and the vision stack use. Read on every call
/// (a cheap local query) so a resolution or main-display change is picked up; 1440x900 only if the query fails.
pub(crate) fn primary_points() -> (i32, i32) {
    // SAFETY: plain queries; CGMainDisplayID takes no arguments and CGDisplayBounds accepts any display id (an
    // invalid one yields an empty rect, which `points_from_bounds` rejects).
    let bounds = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    points_from_bounds(bounds).unwrap_or_else(|| {
        warn_once(&FALLBACK_WARNED, "screen_capture_mac: main display bounds unavailable, using 1440x900");
        FALLBACK_POINTS
    })
}

/// Whole-point (width, height) of a display rectangle, or None when it is empty, negative, or not finite.
fn points_from_bounds(r: CGRect) -> Option<(i32, i32)> {
    let w = dimension_to_i32(r.size.w)?;
    let h = dimension_to_i32(r.size.h)?;
    (w > 0 && h > 0).then_some((w, h))
}

/// A point dimension as a whole number. `i32::try_from(f64)` does not exist, so range-check and round explicitly;
/// NaN / infinity / out-of-range values are rejected.
fn dimension_to_i32(v: f64) -> Option<i32> {
    (v.is_finite() && v >= f64::from(i32::MIN) && v <= f64::from(i32::MAX)).then(|| v.round() as i32)
}

static FALLBACK_WARNED: AtomicBool = AtomicBool::new(false);
static PERMISSION_WARNED: AtomicBool = AtomicBool::new(false);

/// Log `msg` the first time only: these conditions repeat on every screen command and last the whole run.
fn warn_once(flag: &AtomicBool, msg: &str) {
    if !flag.swap(true, Ordering::Relaxed) {
        tracing::warn!("{msg}");
    }
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
/// small text far better at native pixels; map results back to points with the width ratio. None (logged once per
/// run) while Screen Recording is not granted.
pub(crate) fn capture_native(x: i32, y: i32, w: i32, h: i32) -> Option<image::DynamicImage> {
    if w <= 0 || h <= 0 {
        return None;
    }
    if !crate::mac_permissions::screen_recording_allowed() {
        warn_once(&PERMISSION_WARNED, "screen_capture_mac: Screen Recording permission missing, capture skipped");
        return None;
    }
    let png = TempFile(unique_png_path(&std::env::temp_dir()));
    let out = std::process::Command::new("screencapture")
        .args(["-x", "-t", "png", "-R"])
        .arg(format!("{x},{y},{w},{h}"))
        .arg(&png.0)
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
    image::ImageReader::open(&png.0).ok()?.decode().ok()
}

/// Per-process capture counter: OCR and cloud vision can capture at the same time, so each call gets its own file.
static CAPTURE_SEQ: AtomicU64 = AtomicU64::new(0);

/// `<dir>/nexus-cap-<pid>-<n>.png`, unique per call within this process and across processes.
fn unique_png_path(dir: &Path) -> PathBuf {
    let n = CAPTURE_SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("nexus-cap-{}-{n}.png", std::process::id()))
}

/// A capture file removed when dropped, so every exit path (failed spawn, failed capture, bad PNG) cleans up.
struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
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
    use super::*;

    fn rect(w: f64, h: f64) -> CGRect {
        CGRect { origin: CGPoint::default(), size: CGSize { w, h } }
    }

    #[test]
    fn bounds_convert_to_whole_points() {
        assert_eq!(points_from_bounds(rect(1512.0, 982.0)), Some((1512, 982)));
        assert_eq!(points_from_bounds(rect(1440.6, 899.4)), Some((1441, 899)));
        assert_eq!(points_from_bounds(rect(0.0, 0.0)), None);
        assert_eq!(points_from_bounds(rect(-5.0, 900.0)), None);
        assert_eq!(points_from_bounds(rect(f64::NAN, 900.0)), None);
        assert_eq!(points_from_bounds(rect(f64::INFINITY, 900.0)), None);
        assert_eq!(points_from_bounds(rect(1e12, 900.0)), None);
    }

    #[test]
    fn cgrect_is_four_doubles() {
        assert_eq!(std::mem::size_of::<CGRect>(), 4 * std::mem::size_of::<f64>());
    }

    /// Real Core Graphics call; needs no permission. Prints the main display size in points.
    #[test]
    fn primary_points_reads_the_main_display() {
        let (w, h) = primary_points();
        println!("main display: {w}x{h} points");
        assert!(w > 0 && h > 0);
    }

    #[test]
    fn capture_paths_are_unique_per_call() {
        let dir = Path::new("/x");
        let a = unique_png_path(dir);
        let b = unique_png_path(dir);
        assert_ne!(a, b);
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(&format!("nexus-cap-{}-", std::process::id())) && name.ends_with(".png"));
    }

    #[test]
    fn temp_file_is_removed_on_drop() {
        let path = unique_png_path(&std::env::temp_dir());
        std::fs::write(&path, b"png").unwrap();
        drop(TempFile(path.clone()));
        assert!(!path.exists());
    }

    #[test]
    fn rgba_swaps_to_bgra() {
        let img = image::RgbaImage::from_raw(1, 1, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(rgba_to_bgra(&img), vec![3, 2, 1, 4]);
    }
}
