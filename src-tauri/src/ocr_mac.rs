//! macOS local OCR through Apple's Vision framework: the same functions as the Windows `ocr.rs` (Windows.Media.Ocr),
//! mounted as `crate::ocr` on macOS (lib.rs), so callers stay the same.
//!
//! STUB: returns None until the Vision implementation lands; callers already degrade to their next tier on None.

/// True when text recognition can run on this machine.
#[allow(dead_code)]
pub fn is_available() -> bool {
    false
}

/// OCR one screen region (points). Rects are relative to the region's top-left corner, one per returned line.
pub fn capture_region_text_rects(x: i32, y: i32, w: i32, h: i32) -> Option<(String, Vec<(i32, i32, i32, i32)>)> {
    let _ = (x, y, w, h);
    None
}

/// OCR text of one screen region, no rects.
pub fn capture_region_text(x: i32, y: i32, w: i32, h: i32) -> Option<String> {
    capture_region_text_rects(x, y, w, h).map(|(t, _)| t)
}

/// OCR text of the whole primary display.
#[allow(dead_code)]
pub fn capture_screen_text_only() -> Option<String> {
    let (mw, mh) = crate::screen::primary_monitor_size()?;
    capture_region_text_rects(0, 0, mw, mh).map(|(t, _)| t)
}
