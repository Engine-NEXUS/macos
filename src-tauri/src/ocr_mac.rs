//! macOS local OCR through Apple's Vision framework: the same functions as the Windows `ocr.rs` (Windows.Media.Ocr),
//! mounted as `crate::ocr` on macOS (lib.rs), so callers stay the same.
//!
//! OS-native like the Windows engine: no model download, no cloud, works offline. Vision needs no privacy permission;
//! only the screen capture in front of it needs Screen Recording.
//!
//! Pipeline: `screencapture` at native (Retina) pixels → RGBA → CGImage → `VNImageRequestHandler` +
//! `VNRecognizeTextRequest` (accurate level, language correction) → one observation per line → its best candidate and
//! normalized bounding box → box mapped back to the region's points → lines in reading order.
//!
//! Recognition runs on the native-pixel image because small UI text reads far better at 2x; callers work in points,
//! so boxes are divided by the capture scale on the way out. Languages are Vision's default (`en_US`); the Windows
//! engine follows the user's profile languages first.
//!
//! Cost (measured on an M5, macOS 27.2): ~30-40 ms for a 1200x400 image and ~100-300 ms for a full Retina screen. The
//! first request of a process whose model cache is cold takes ~24 s: the system compiles Vision's models into
//! `~/Library/Caches/<process name>/com.apple.e5rt.e5bundlecache`, keyed by OS build, so expect it once after install
//! and again after each macOS update.
//!
//! Fail-open everywhere: no Vision class, a failed capture, a Vision error or an Objective-C exception → None (callers
//! fall through to their next tier, as on Windows without an OCR language pack).

/// A line box in points, relative to the region's top-left corner: (x, y, w, h).
type Rect = (i32, i32, i32, i32);

/// A Vision bounding box: normalized to the image (0..1), origin at the image's BOTTOM-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
struct NormBox {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// True when Vision text recognition exists on this machine (macOS 10.15+). Cheap: two class lookups, no model load.
#[allow(dead_code)]
pub fn is_available() -> bool {
    vision::classes().is_some()
}

/// OCR one screen region (points). Rects are in points, relative to the region's top-left corner, one per line.
pub fn capture_region_text_rects(x: i32, y: i32, w: i32, h: i32) -> Option<(String, Vec<Rect>)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let img = crate::screen_capture_mac::capture_native(x, y, w, h)?;
    let raw = recognize_image(&img)?;
    join_lines(lines_in_region(raw, img.width(), img.height(), w, h))
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

/// Run one recognition on a small blank image so Vision builds its model now, off the voice path. The first
/// recognition for a new process name (after install, and again after each macOS update) compiles the model into
/// `~/Library/Caches/<process>/com.apple.e5rt.e5bundlecache` and takes about 24 s; a later "what's on my screen" then
/// answers in tens of milliseconds. Returns how long it took.
pub fn warm_up() -> std::time::Duration {
    let t0 = std::time::Instant::now();
    let blank = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(256, 64, image::Rgba([255, 255, 255, 255])));
    let _ = recognize_image(&blank);
    t0.elapsed()
}

/// Vision text recognition on one image: every recognized line's best candidate with its normalized box, in Vision's
/// order. None when Vision is missing or the request fails (logged); Some(empty) when the image holds no text.
fn recognize_image(img: &image::DynamicImage) -> Option<Vec<(String, NormBox)>> {
    let converted;
    let rgba = match img.as_rgba8() {
        Some(rgba) => rgba,
        None => {
            converted = img.to_rgba8();
            &converted
        }
    };
    let cg = vision::cg_image(rgba)?;
    vision::recognize(&cg)
}

/// Vision lines → (text, rect in region points), blank lines dropped, in reading order. `img_w`/`img_h` are the pixel
/// size of the image Vision read; `region_w`/`region_h` the points it covers.
fn lines_in_region(
    raw: Vec<(String, NormBox)>,
    img_w: u32,
    img_h: u32,
    region_w: i32,
    region_h: i32,
) -> Vec<(String, Rect)> {
    let lines = raw
        .into_iter()
        .filter(|(text, _)| !text.trim().is_empty())
        // Callers count lines with `str::lines`, so a line must never carry its own break.
        .map(|(text, b)| (text.replace(['\r', '\n'], " "), norm_box_to_points(b, img_w, img_h, region_w, region_h)))
        .collect();
    reading_order(lines)
}

/// Joined text plus one rect per line, or None when nothing was read (the Windows engine's contract).
fn join_lines(lines: Vec<(String, Rect)>) -> Option<(String, Vec<Rect>)> {
    if lines.is_empty() {
        return None;
    }
    let (texts, rects): (Vec<String>, Vec<Rect>) = lines.into_iter().unzip();
    Some((texts.join("\n"), rects))
}

/// Vision's normalized, bottom-left-origin box → (x, y, w, h) in the region's points, top-left origin.
///
/// The image is the region at native pixels, so `scale = img_w / region_w` (2 on Retina). Edges are rounded rather
/// than the size, so neighbouring boxes do not drift apart, then clamped to the region: Vision boxes can poke slightly
/// past the image edge. A degenerate size or a non-finite box yields an empty rect instead of a panic.
fn norm_box_to_points(b: NormBox, img_w: u32, img_h: u32, region_w: i32, region_h: i32) -> Rect {
    let (img_w, img_h) = (f64::from(img_w), f64::from(img_h));
    let scale = img_w / f64::from(region_w);
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    // Pixels from the image's top-left corner.
    let left = b.x * img_w;
    let right = (b.x + b.w) * img_w;
    let top = (1.0 - (b.y + b.h)) * img_h;
    let bottom = (1.0 - b.y) * img_h;
    // `as i32` maps NaN to 0 and saturates, so the clamp below always sees a real number.
    let to_pt = |px: f64, max: i32| ((px / scale).round() as i32).clamp(0, max.max(0));
    let (x0, x1) = (to_pt(left, region_w), to_pt(right, region_w));
    let (y0, y1) = (to_pt(top, region_h), to_pt(bottom, region_h));
    (x0, y0, (x1 - x0).max(0), (y1 - y0).max(0))
}

/// Rows top to bottom, each row left to right. Vision returns observations in its own order, while the Windows engine
/// returns reading order and callers number lines from it ("the third line"). A line joins the current row when its
/// vertical centre falls inside the row's first (topmost) line. Bucketing first keeps every sort a total order; a
/// pairwise "overlaps vertically" comparator is not transitive, and `sort_by` may panic on one.
fn reading_order(mut lines: Vec<(String, Rect)>) -> Vec<(String, Rect)> {
    lines.sort_by_key(|(_, r)| (r.1, r.0));
    let mut out = Vec::with_capacity(lines.len());
    let mut row: Vec<(String, Rect)> = Vec::new();
    let mut row_bottom = i32::MIN;
    for line in lines {
        let r = line.1;
        let centre = r.1.saturating_add(r.3 / 2);
        if !row.is_empty() && centre >= row_bottom {
            row.sort_by_key(|(_, r)| r.0);
            out.append(&mut row);
        }
        if row.is_empty() {
            row_bottom = r.1.saturating_add(r.3);
        }
        row.push(line);
    }
    row.sort_by_key(|(_, r)| r.0);
    out.append(&mut row);
    out
}

/// The Objective-C side: Vision through the runtime (`msg_send!` on classes looked up by name) and the CoreGraphics
/// calls that wrap the pixels in a CGImage. No binding crates beyond `objc2`.
mod vision {
    use super::NormBox;
    use crate::ax_mac::mac::Owned;
    use core_foundation_sys::base::{kCFAllocatorDefault, CFIndex, CFTypeRef};
    use core_foundation_sys::data::CFDataCreate;
    use objc2::encode::{Encode, Encoding, RefEncode};
    use objc2::msg_send;
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject};
    use std::ffi::{c_char, CStr};
    use std::panic::AssertUnwindSafe;

    // Loads the frameworks so their classes are registered with the runtime; the classes are then found by name.
    #[link(name = "Vision", kind = "framework")]
    extern "C" {}
    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub(super) fn CGColorSpaceCreateDeviceRGB() -> CFTypeRef;
        fn CGDataProviderCreateWithCFData(data: CFTypeRef) -> CFTypeRef;
        fn CGImageCreate(
            width: usize,
            height: usize,
            bits_per_component: usize,
            bits_per_pixel: usize,
            bytes_per_row: usize,
            space: CFTypeRef,
            bitmap_info: u32,
            provider: CFTypeRef,
            decode: *const f64,
            should_interpolate: bool,
            intent: i32,
        ) -> CFTypeRef;
    }

    /// RGBX: the 4th byte is ignored. Screen captures are opaque, and an alpha channel would only add a blend step.
    const K_CG_IMAGE_ALPHA_NONE_SKIP_LAST: u32 = 5;
    const K_CG_RENDERING_INTENT_DEFAULT: i32 = 0;
    /// `VNRequestTextRecognitionLevelAccurate`.
    const RECOGNITION_LEVEL_ACCURATE: isize = 0;

    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub(super) struct CGPoint {
        pub x: f64,
        pub y: f64,
    }
    // SAFETY: same layout and type encoding as CoreGraphics' `CGPoint` (two CGFloat = f64 on 64-bit macOS).
    unsafe impl Encode for CGPoint {
        const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub(super) struct CGSize {
        pub width: f64,
        pub height: f64,
    }
    // SAFETY: same layout and type encoding as CoreGraphics' `CGSize`.
    unsafe impl Encode for CGSize {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub(super) struct CGRect {
        pub origin: CGPoint,
        pub size: CGSize,
    }
    // SAFETY: same layout and type encoding as CoreGraphics' `CGRect`.
    unsafe impl Encode for CGRect {
        const ENCODING: Encoding = Encoding::Struct("CGRect", &[CGPoint::ENCODING, CGSize::ENCODING]);
    }

    /// Opaque `CGImage`, only ever used behind a pointer (`CGImageRef`).
    #[repr(C)]
    struct CGImage {
        _opaque: [u8; 0],
    }
    // SAFETY: a pointer to it encodes as `^{CGImage=}`, the Objective-C encoding of `CGImageRef`.
    unsafe impl RefEncode for CGImage {
        const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
    }

    /// The request and handler classes, or None before macOS 10.15 (or if Vision failed to load).
    pub(super) fn classes() -> Option<(&'static AnyClass, &'static AnyClass)> {
        Some((AnyClass::get(c"VNRecognizeTextRequest")?, AnyClass::get(c"VNImageRequestHandler")?))
    }

    /// The pixels as a CGImage (copied, so the CGImage owns its bytes for as long as Vision keeps it).
    pub(super) fn cg_image(rgba: &image::RgbaImage) -> Option<Owned> {
        let (w, h) = rgba.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        let bytes = rgba.as_raw();
        let len = CFIndex::try_from(bytes.len()).ok()?;
        // SAFETY: CFDataCreate copies `len` bytes out of a live slice; the result is +1 (create rule).
        let data = Owned::new(unsafe { CFDataCreate(kCFAllocatorDefault, bytes.as_ptr(), len) } as CFTypeRef)?;
        // SAFETY: plain constructors over a live CFData; each result is +1 and released by `Owned`.
        let provider = Owned::new(unsafe { CGDataProviderCreateWithCFData(data.0) })?;
        // SAFETY: plain constructor, no arguments.
        let space = Owned::new(unsafe { CGColorSpaceCreateDeviceRGB() })?;
        // SAFETY: 8-bit RGBX, rows of exactly `w * 4` bytes, so the provider holds `h` full rows; the image retains the
        // provider and colour space, so dropping ours afterwards is fine.
        Owned::new(unsafe {
            CGImageCreate(
                w as usize,
                h as usize,
                8,
                32,
                w as usize * 4,
                space.0,
                K_CG_IMAGE_ALPHA_NONE_SKIP_LAST,
                provider.0,
                std::ptr::null(),
                false,
                K_CG_RENDERING_INTENT_DEFAULT,
            )
        })
    }

    /// Runs the text request on a CGImage. Objective-C exceptions are caught here: unwinding one into Rust aborts the
    /// process (release builds use panic = "abort"), and OCR must fail open.
    pub(super) fn recognize(image: &Owned) -> Option<Vec<(String, NormBox)>> {
        let cg = image.0 as *const CGImage;
        // SAFETY: `cg` is a live CGImage, held by `image` for the whole call.
        let caught = objc2::exception::catch(AssertUnwindSafe(|| autoreleasepool(|_| unsafe { perform(cg) })));
        match caught {
            Ok(lines) => lines,
            Err(exception) => {
                tracing::warn!("ocr_mac: Vision raised an Objective-C exception: {exception:?}");
                None
            }
        }
    }

    /// # Safety
    /// `cg` must be a live CGImage for the duration of the call; call inside an autorelease pool.
    unsafe fn perform(cg: *const CGImage) -> Option<Vec<(String, NormBox)>> {
        let (request_cls, handler_cls) = classes()?;
        let dictionary_cls = AnyClass::get(c"NSDictionary")?;
        let array_cls = AnyClass::get(c"NSArray")?;
        // SAFETY: every receiver below is a live object or class (nil results are caught by `?` before use), and every
        // selector exists on macOS 10.15+ with the argument and return types used here (Vision, Foundation headers).
        unsafe {
            let request: Option<Retained<AnyObject>> = msg_send![request_cls, new];
            let request = request?;
            let _: () = msg_send![&*request, setRecognitionLevel: RECOGNITION_LEVEL_ACCURATE];
            let _: () = msg_send![&*request, setUsesLanguageCorrection: Bool::YES];

            let options: Option<Retained<AnyObject>> = msg_send![dictionary_cls, dictionary];
            let options = options?;
            let handler: Allocated<AnyObject> = msg_send![handler_cls, alloc];
            let handler: Option<Retained<AnyObject>> = msg_send![handler, initWithCGImage: cg, options: &*options];
            let handler = handler?;
            let requests: Option<Retained<AnyObject>> = msg_send![array_cls, arrayWithObject: &*request];
            let requests = requests?;

            // Synchronous: returns once the request has finished.
            let done: Result<(), Retained<NSObject>> = msg_send![&*handler, performRequests: &*requests, error: _];
            if let Err(err) = done {
                let reason: Option<Retained<AnyObject>> = msg_send![&*err, localizedDescription];
                let reason = reason.as_deref().map(|s| ns_string(s)).unwrap_or_default();
                tracing::warn!("ocr_mac: Vision text request failed: {reason}");
                return None;
            }

            let results: Option<Retained<AnyObject>> = msg_send![&*request, results];
            let Some(results) = results else {
                return Some(Vec::new());
            };
            let count: usize = msg_send![&*results, count];
            let mut lines = Vec::with_capacity(count);
            for i in 0..count {
                let observation: Option<Retained<AnyObject>> = msg_send![&*results, objectAtIndex: i];
                let Some(observation) = observation else { continue };
                let candidates: Option<Retained<AnyObject>> = msg_send![&*observation, topCandidates: 1usize];
                let Some(candidates) = candidates else { continue };
                let best: Option<Retained<AnyObject>> = msg_send![&*candidates, firstObject];
                let Some(best) = best else { continue };
                let text: Option<Retained<AnyObject>> = msg_send![&*best, string];
                let Some(text) = text else { continue };
                let b: CGRect = msg_send![&*observation, boundingBox];
                lines.push((
                    ns_string(&text),
                    NormBox { x: b.origin.x, y: b.origin.y, w: b.size.width, h: b.size.height },
                ));
            }
            Some(lines)
        }
    }

    /// Copies an NSString into Rust. `UTF8String` points into storage owned by the string or the current autorelease
    /// pool, so it is copied before either goes away.
    ///
    /// # Safety
    /// `s` must be an NSString.
    unsafe fn ns_string(s: &AnyObject) -> String {
        // SAFETY: `s` is an NSString (caller); UTF8String gives null or a NUL-terminated buffer that outlives the copy.
        unsafe {
            let p: *const c_char = msg_send![s, UTF8String];
            if p.is_null() {
                String::new()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::vision::{CGPoint, CGRect, CGSize};
    use super::*;
    use crate::ax_mac::mac::{cf_string, Owned};
    use core_foundation_sys::attributed_string::CFAttributedStringCreate;
    use core_foundation_sys::base::{kCFAllocatorDefault, CFTypeRef};
    use core_foundation_sys::dictionary::{
        kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks, CFDictionaryCreate,
    };
    use core_foundation_sys::string::CFStringRef;
    use std::ffi::c_void;

    fn nb(x: f64, y: f64, w: f64, h: f64) -> NormBox {
        NormBox { x, y, w, h }
    }

    fn line(text: &str, r: Rect) -> (String, Rect) {
        (text.to_string(), r)
    }

    #[test]
    fn box_flips_bottom_left_origin_to_top_left() {
        // Bottom-left quarter strip of a 200x100 image, region the same size.
        assert_eq!(norm_box_to_points(nb(0.0, 0.0, 0.5, 0.25), 200, 100, 200, 100), (0, 75, 100, 25));
        // A box touching the top edge starts at y = 0.
        assert_eq!(norm_box_to_points(nb(0.25, 0.75, 0.5, 0.25), 200, 100, 200, 100), (50, 0, 100, 25));
    }

    #[test]
    fn box_maps_retina_pixels_back_to_points() {
        // A 2x capture of a 200x100-point region: same normalized box, same points.
        assert_eq!(norm_box_to_points(nb(0.0, 0.0, 0.5, 0.25), 400, 200, 200, 100), (0, 75, 100, 25));
        // x 151.2..453.6 → 151..454, y (1 - 0.6) * 982 = 392.8 → 393 down to 491.
        assert_eq!(norm_box_to_points(nb(0.1, 0.5, 0.2, 0.1), 3024, 1964, 1512, 982), (151, 393, 303, 98));
    }

    #[test]
    fn box_rounds_edges_not_sizes() {
        // Edges 10.4 → 10 and 30.6 → 31: width is 21, not round(20.2) = 20.
        assert_eq!(norm_box_to_points(nb(0.104, 0.0, 0.202, 1.0), 100, 100, 100, 100), (10, 0, 21, 100));
        // Top 39.6 → 40 and bottom 60.4 → 60 (after the flip): height 20, not round(20.8) = 21.
        assert_eq!(norm_box_to_points(nb(0.0, 0.396, 1.0, 0.208), 100, 100, 100, 100), (0, 40, 100, 20));
    }

    #[test]
    fn box_is_clamped_to_the_region() {
        assert_eq!(norm_box_to_points(nb(-0.1, -0.1, 1.3, 1.3), 400, 200, 200, 100), (0, 0, 200, 100));
        // Entirely past the right edge: zero width on that edge, the vertical extent kept.
        assert_eq!(norm_box_to_points(nb(1.2, 0.0, 0.1, 0.5), 200, 100, 200, 100), (200, 50, 0, 50));
    }

    #[test]
    fn box_survives_degenerate_input() {
        assert_eq!(norm_box_to_points(nb(f64::NAN, f64::NAN, f64::NAN, f64::NAN), 200, 100, 200, 100), (0, 0, 0, 0));
        assert_eq!(norm_box_to_points(nb(0.0, 0.0, f64::INFINITY, 1.0), 200, 100, 200, 100), (0, 0, 200, 100));
        // Zero-size region or image: no division by zero, an empty rect.
        assert_eq!(norm_box_to_points(nb(0.1, 0.1, 0.5, 0.5), 200, 100, 0, 0), (0, 0, 0, 0));
        assert_eq!(norm_box_to_points(nb(0.1, 0.1, 0.5, 0.5), 0, 0, 200, 100), (0, 0, 0, 0));
    }

    #[test]
    fn reading_order_is_rows_then_columns() {
        let ordered = reading_order(vec![
            line("bottom", (10, 200, 80, 20)),
            line("top right", (300, 10, 80, 20)),
            line("top left", (10, 14, 80, 20)), // 4 pt lower baseline, still the same row
            line("middle", (10, 100, 80, 20)),
        ]);
        let texts: Vec<&str> = ordered.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(texts, ["top left", "top right", "middle", "bottom"]);
        assert!(reading_order(Vec::new()).is_empty());
    }

    #[test]
    fn reading_order_starts_a_new_row_when_the_centre_is_below_the_row() {
        // The second line's centre (y 25) is below the first line's bottom (y 20): a new row, even though it is left.
        let ordered = reading_order(vec![line("first", (100, 0, 50, 20)), line("second", (0, 15, 50, 20))]);
        assert_eq!(ordered[0].0, "first");
        assert_eq!(ordered[1].0, "second");
    }

    #[test]
    fn lines_drop_blanks_and_inner_breaks_and_keep_one_rect_each() {
        let raw = vec![
            ("Second".to_string(), nb(0.0, 0.0, 0.5, 0.25)),
            ("   ".to_string(), nb(0.0, 0.5, 0.5, 0.25)),
            ("First\nline".to_string(), nb(0.0, 0.75, 0.5, 0.25)),
        ];
        let (text, rects) = join_lines(lines_in_region(raw, 200, 100, 200, 100)).unwrap();
        assert_eq!(text, "First line\nSecond");
        assert_eq!(rects, vec![(0, 0, 100, 25), (0, 75, 100, 25)]);
        assert_eq!(rects.len(), text.lines().count());
        assert_eq!(join_lines(Vec::new()), None);
    }

    #[test]
    fn empty_regions_and_images_fail_open_without_capturing() {
        assert_eq!(capture_region_text_rects(0, 0, 0, 100), None);
        assert_eq!(capture_region_text_rects(0, 0, 100, -1), None);
        assert_eq!(recognize_image(&image::DynamicImage::new_rgba8(0, 0)), None);
    }

    #[test]
    fn vision_text_recognition_is_available() {
        assert!(is_available(), "VNRecognizeTextRequest missing: Vision framework not linked or loaded");
    }

    // ── a real Vision run on an image drawn here (no screen capture) ─────────────────────────────

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits_per_component: usize,
            bytes_per_row: usize,
            space: CFTypeRef,
            bitmap_info: u32,
        ) -> CFTypeRef;
        fn CGContextSetRGBFillColor(c: CFTypeRef, r: f64, g: f64, b: f64, a: f64);
        fn CGContextFillRect(c: CFTypeRef, rect: CGRect);
        fn CGContextSetTextPosition(c: CFTypeRef, x: f64, y: f64);
    }
    #[link(name = "CoreText", kind = "framework")]
    extern "C" {
        static kCTFontAttributeName: CFStringRef;
        fn CTFontCreateWithName(name: CFStringRef, size: f64, matrix: *const c_void) -> CFTypeRef;
        fn CTLineCreateWithAttributedString(s: CFTypeRef) -> CFTypeRef;
        fn CTLineDraw(line: CFTypeRef, c: CFTypeRef);
    }
    const K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST: u32 = 1;

    /// Draws `lines` black on white in Helvetica with CoreText. Each line is (text, baseline in pixels from the top).
    fn render_text(width: u32, height: u32, font_px: f64, left: f64, lines: &[(&str, f64)]) -> image::DynamicImage {
        let mut buf = vec![0u8; width as usize * height as usize * 4];
        {
            // SAFETY: plain constructors; `buf` holds exactly `height` rows of `width * 4` bytes and outlives `ctx`
            // (dropped at the end of this block). Every CF/CG result is +1 and released by `Owned`.
            unsafe {
                let space = Owned::new(super::vision::CGColorSpaceCreateDeviceRGB()).unwrap();
                let ctx = Owned::new(CGBitmapContextCreate(
                    buf.as_mut_ptr().cast(),
                    width as usize,
                    height as usize,
                    8,
                    width as usize * 4,
                    space.0,
                    K_CG_IMAGE_ALPHA_PREMULTIPLIED_LAST,
                ))
                .unwrap();
                CGContextSetRGBFillColor(ctx.0, 1.0, 1.0, 1.0, 1.0);
                let full = CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize { width: f64::from(width), height: f64::from(height) },
                };
                CGContextFillRect(ctx.0, full);
                let name = cf_string("Helvetica");
                let font = Owned::new(CTFontCreateWithName(name.0 as CFStringRef, font_px, std::ptr::null())).unwrap();
                let keys = [kCTFontAttributeName as *const c_void];
                let values = [font.0];
                let attrs = Owned::new(CFDictionaryCreate(
                    kCFAllocatorDefault,
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks,
                    &kCFTypeDictionaryValueCallBacks,
                ) as CFTypeRef)
                .unwrap();
                for (text, baseline_from_top) in lines {
                    let s = cf_string(text);
                    let astr =
                        Owned::new(CFAttributedStringCreate(kCFAllocatorDefault, s.0 as CFStringRef, attrs.0 as _)
                            as CFTypeRef)
                        .unwrap();
                    let ct_line = Owned::new(CTLineCreateWithAttributedString(astr.0)).unwrap();
                    // CoreGraphics y grows upwards from the bottom edge.
                    CGContextSetTextPosition(ctx.0, left, f64::from(height) - baseline_from_top);
                    CTLineDraw(ct_line.0, ctx.0);
                }
            }
        }
        // A bitmap context stores its top row first; opaque premultiplied RGBA equals straight RGBA.
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width, height, buf).unwrap())
    }

    /// Runs Vision for real on CoreText-drawn lines and checks text, order and boxes at 1x and at a simulated 2x
    /// Retina capture. Ignored by default: CI runs on macOS VMs, where Vision's text models are unverified, and the
    /// first run of each new test binary spends ~24 s compiling them. Run with
    /// `cargo test --lib ocr -- --ignored --nocapture`.
    /// The blank-image warm-up must be what pays the model build, so the first real read is fast. Meaningful only with a
    /// cold cache: copy the test binary to a new name first (the cache is keyed by process name).
    #[test]
    #[ignore = "real Vision run; meaningful only from a freshly named test binary (cold model cache)"]
    fn warm_up_pays_the_model_build_so_the_first_read_is_fast() {
        let warm = super::warm_up();
        let img = render_text(1200, 400, 56.0, 60.0, &[("NEXUS reads the screen", 100.0)]);
        let started = std::time::Instant::now();
        let raw = recognize_image(&img).expect("Vision request failed");
        let read = started.elapsed();
        println!("warm-up {} ms, then first real read {} ms: {raw:?}", warm.as_millis(), read.as_millis());
        assert!(raw.iter().any(|(t, _)| t.contains("NEXUS")));
        assert!(read < std::time::Duration::from_secs(2), "the real read still paid the model build: {read:?}");
    }

    #[test]
    #[ignore = "real Vision run: unverified on CI macOS VMs; first run per binary compiles the models (~24 s)"]
    fn vision_reads_rendered_text_with_boxes_in_points() {
        // 1200x400 px, 56 px Helvetica, left margin 60 px; baselines 100 and 300 px from the top.
        let img = render_text(1200, 400, 56.0, 60.0, &[("NEXUS reads the screen", 100.0), ("Hello World 2026", 300.0)]);

        let started = std::time::Instant::now();
        let raw = recognize_image(&img).expect("Vision request failed");
        let first_ms = started.elapsed().as_millis();
        let started = std::time::Instant::now();
        let raw_again = recognize_image(&img).expect("Vision request failed (second run)");
        let warm_ms = started.elapsed().as_millis();
        println!("Vision OCR 1200x400: first {first_ms} ms, warm {warm_ms} ms; raw = {raw:?}");
        assert_eq!(raw.len(), raw_again.len());

        // 1x: region == image.
        let (text, rects) = join_lines(lines_in_region(raw.clone(), 1200, 400, 1200, 400)).expect("no text read");
        println!("text = {text:?}\nrects = {rects:?}");
        assert_eq!(rects.len(), text.lines().count());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "expected two lines, got {lines:?}");
        assert!(lines[0].contains("NEXUS"), "first line: {:?}", lines[0]);
        assert!(lines[1].contains("Hello World"), "second line: {:?}", lines[1]);
        let (top, bottom) = (rects[0], rects[1]);
        // Line 1: cap height of 56 px Helvetica is ~40 px, so the box sits around y 60..100 and starts near x 60.
        assert!((45..=75).contains(&top.0), "line 1 x: {top:?}");
        assert!((40..=75).contains(&top.1) && top.1 + top.3 >= 95 && top.1 + top.3 <= 125, "line 1 y: {top:?}");
        assert!(top.2 > 400 && top.0 + top.2 <= 1200, "line 1 width: {top:?}");
        assert!(bottom.1 > top.1 + top.3, "line 2 must sit below line 1: {rects:?}");
        assert!(bottom.1 + bottom.3 <= 400);

        // Simulated Retina: the same pixels covering a 600x200-point region → every box halves (±1 for rounding).
        let (_, half) = join_lines(lines_in_region(raw, 1200, 400, 600, 200)).unwrap();
        for (full, half) in rects.iter().zip(&half) {
            let near = |a: i32, b: i32| (a - b * 2).abs() <= 2;
            assert!(
                near(full.0, half.0) && near(full.1, half.1) && near(full.2, half.2) && near(full.3, half.3),
                "1x {full:?} vs 2x {half:?}"
            );
        }

        // A blank image: Vision succeeds with no lines, and the public contract turns that into None.
        let blank = render_text(400, 200, 40.0, 20.0, &[]);
        let raw = recognize_image(&blank).expect("Vision request failed on a blank image");
        assert!(raw.is_empty(), "blank image read as {raw:?}");
        assert_eq!(join_lines(lines_in_region(raw, 400, 200, 400, 200)), None);
    }
}
