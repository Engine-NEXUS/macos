//! macOS Accessibility (AXUIElement) reading: the macOS side of the UI Automation lookup in `screen.rs`.
//!
//! "Click the third option", "read the second link" and "click Save" need the actionable controls of the window in
//! front, with names and screen boxes. On a Mac that is the Accessibility API. It only answers when the app that started
//! NEXUS holds the Accessibility permission (System Settings → Privacy & Security → Accessibility); without it the list is
//! empty and callers fall through to their next tier, as on Windows when UI Automation is unavailable.
//!
//! Boxes are global top-left points: the space CGEvent and enigo's `Coordinate::Abs` use, so a box centre is clicked as
//! is. The pure parts (role mapping, naming, visibility) are tested on every OS; the walk is macOS-only and has an ignored
//! live test (it needs the permission and a desktop).

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

/// Windows control-type name for an accessibility role, or None when the role is not an actionable control. Keeping the
/// Windows names makes spoken lines ("third Button says OK") the same on both systems. Secure text fields never resolve.
pub(crate) fn kind_for_role(role: &str, subrole: &str) -> Option<&'static str> {
    if subrole == "AXSecureTextField" {
        return None;
    }
    Some(match role {
        "AXButton" | "AXMenuButton" | "AXDisclosureTriangle" => "Button",
        "AXLink" => "Hyperlink",
        "AXRadioButton" if subrole == "AXTabButton" => "TabItem",
        "AXRadioButton" => "RadioButton",
        "AXCheckBox" => "CheckBox",
        "AXMenuItem" => "MenuItem",
        "AXPopUpButton" | "AXComboBox" => "ComboBox",
        "AXTextField" | "AXTextArea" => "Edit",
        _ => return None,
    })
}

/// The name a person reads on the control: the first non-empty of (in the caller's order) title, description, an empty
/// field's placeholder, the text inside it (a link's words), help tag. Whitespace runs collapse to one space (link text
/// often spans lines) and very long names are cut.
pub(crate) fn pick_name(candidates: &[&str]) -> String {
    let first = candidates.iter().map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or("");
    first.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect()
}

/// Chromium browsers build the web page's accessibility tree only for an assistive app, which announces itself with
/// `AXEnhancedUserInterface`. Not set on other apps: it is known to make some of them animate window moves.
pub(crate) fn wants_enhanced_ui(bundle_id: &str) -> bool {
    matches!(
        bundle_id.to_ascii_lowercase().as_str(),
        "com.google.chrome"
            | "com.google.chrome.beta"
            | "com.google.chrome.canary"
            | "org.chromium.chromium"
            | "com.brave.browser"
            | "com.microsoft.edgemac"
            | "company.thebrowser.browser"
            | "com.vivaldi.vivaldi"
            | "com.operasoftware.opera"
    )
}

/// A screen rectangle in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// The overlapping part, or None when the two do not overlap or either is degenerate.
    pub(crate) fn intersect(&self, o: &Rect) -> Option<Rect> {
        let ok = |r: &Rect| r.w > 0.0 && r.h > 0.0 && [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite());
        if !ok(self) || !ok(o) {
            return None;
        }
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        (x1 > x0 && y1 > y0).then(|| Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    /// Whole-point box for `UiElement`.
    pub(crate) fn rounded(&self) -> (i32, i32, i32, i32) {
        (self.x.round() as i32, self.y.round() as i32, self.w.round() as i32, self.h.round() as i32)
    }
}

/// Actionable controls of the front window, in reading order. Empty (with the reason logged) when the permission is
/// missing, NEXUS itself is in front, there is no window, or the walk ran past its budget.
#[cfg(target_os = "macos")]
pub(crate) fn list_actionables() -> Vec<crate::screen::UiElement> {
    let t0 = std::time::Instant::now();
    let budget = std::time::Duration::from_millis(crate::screen::FAST_LOOKUP_BUDGET_MS);
    match mac::list(budget, MAX_NODES) {
        Ok(v) => {
            tracing::info!("[AX] list_actionables: {} controls in {}ms", v.len(), t0.elapsed().as_millis());
            v
        }
        Err(why) => {
            tracing::warn!("[AX] list_actionables: {why} ({}ms)", t0.elapsed().as_millis());
            Vec::new()
        }
    }
}

/// True when this process may read other apps' accessibility trees and post input events.
#[cfg(target_os = "macos")]
pub(crate) fn trusted() -> bool {
    mac::trusted()
}

/// Upper bound on elements read per lookup: a long web page can hold tens of thousands.
#[cfg(target_os = "macos")]
const MAX_NODES: usize = 6000;

/// Accessibility FFI and helpers, shared with the other macOS screen modules (Ghost press, window tiling, screen
/// context). Keep raw `extern` declarations here rather than repeating them per module.
#[cfg(target_os = "macos")]
pub(crate) mod mac {
    use super::{kind_for_role, pick_name, Rect};
    use crate::screen::UiElement;
    use core_foundation_sys::array::{
        kCFTypeArrayCallBacks, CFArrayCreate, CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFArrayRef,
    };
    use core_foundation_sys::base::{kCFAllocatorDefault, CFGetTypeID, CFIndex, CFRelease, CFRetain, CFTypeID, CFTypeRef};
    use core_foundation_sys::number::{kCFBooleanFalse, kCFBooleanTrue, CFBooleanGetTypeID, CFBooleanGetValue, CFBooleanRef};
    use core_foundation_sys::string::{
        kCFStringEncodingUTF8, CFStringCreateWithBytes, CFStringGetCString, CFStringGetLength,
        CFStringGetMaximumSizeForEncoding, CFStringGetTypeID, CFStringRef,
    };
    use std::ffi::{c_char, c_void};
    use std::time::{Duration, Instant};

    pub(crate) type AXError = i32;
    pub(crate) const AX_VALUE_CGPOINT: u32 = 1;
    pub(crate) const AX_VALUE_CGSIZE: u32 = 2;
    /// Per-call limit for a cross-process accessibility request (the system default is 6 s, so one hung app would stall
    /// the voice pipeline).
    const MESSAGING_TIMEOUT_S: f32 = 0.5;

    #[repr(C)]
    #[derive(Default, Debug, Clone, Copy)]
    pub(crate) struct CGPoint {
        pub x: f64,
        pub y: f64,
    }
    #[repr(C)]
    #[derive(Default, Debug, Clone, Copy)]
    pub(crate) struct CGSize {
        pub w: f64,
        pub h: f64,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        pub(crate) fn AXUIElementCreateSystemWide() -> CFTypeRef;
        pub(crate) fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
        pub(crate) fn AXUIElementCopyAttributeValue(el: CFTypeRef, attr: CFStringRef, value: *mut CFTypeRef) -> AXError;
        pub(crate) fn AXUIElementCopyMultipleAttributeValues(
            el: CFTypeRef,
            attrs: CFArrayRef,
            options: u32,
            values: *mut CFArrayRef,
        ) -> AXError;
        pub(crate) fn AXUIElementSetAttributeValue(el: CFTypeRef, attr: CFStringRef, value: CFTypeRef) -> AXError;
        pub(crate) fn AXUIElementSetMessagingTimeout(el: CFTypeRef, seconds: f32) -> AXError;
        pub(crate) fn AXUIElementPerformAction(el: CFTypeRef, action: CFStringRef) -> AXError;
        pub(crate) fn AXValueCreate(kind: u32, value: *const c_void) -> CFTypeRef;
        pub(crate) fn AXValueGetTypeID() -> CFTypeID;
        pub(crate) fn AXValueGetType(v: CFTypeRef) -> u32;
        pub(crate) fn AXValueGetValue(v: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
    }

    /// An owned Core Foundation reference, released on drop.
    pub(crate) struct Owned(pub(crate) CFTypeRef);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: `Owned` only ever holds a +1 reference (create rule, or retained in `retain`).
            unsafe { CFRelease(self.0) }
        }
    }

    impl Owned {
        /// Takes a +1 reference (create/copy rule). None for null.
        pub(crate) fn new(r: CFTypeRef) -> Option<Self> {
            (!r.is_null()).then_some(Self(r))
        }

        /// Retains a borrowed reference (get rule) so it outlives its container.
        pub(crate) fn retain(r: CFTypeRef) -> Option<Self> {
            if r.is_null() {
                return None;
            }
            // SAFETY: `r` is a live CF object borrowed from an array we still hold.
            unsafe { CFRetain(r) };
            Some(Self(r))
        }
    }

    pub(crate) fn cf_string(s: &str) -> Owned {
        // SAFETY: plain CF constructor over a valid UTF-8 buffer; never null for valid UTF-8.
        let r = unsafe {
            CFStringCreateWithBytes(kCFAllocatorDefault, s.as_ptr(), s.len() as CFIndex, kCFStringEncodingUTF8, 0)
        };
        Owned(r as CFTypeRef)
    }

    /// A CFArray of attribute names, plus the strings it was built from.
    pub(crate) struct Names {
        pub(crate) array: Owned,
        _strings: Vec<Owned>,
    }

    pub(crate) fn names(list: &[&str]) -> Names {
        let strings: Vec<Owned> = list.iter().map(|s| cf_string(s)).collect();
        let ptrs: Vec<*const c_void> = strings.iter().map(|s| s.0).collect();
        // SAFETY: `ptrs` holds live CFStrings; the array retains each through kCFTypeArrayCallBacks.
        let array = unsafe {
            CFArrayCreate(kCFAllocatorDefault, ptrs.as_ptr(), ptrs.len() as CFIndex, &kCFTypeArrayCallBacks)
        };
        Names { array: Owned(array as CFTypeRef), _strings: strings }
    }

    // SAFETY (the readers below): `v` is null or a live CF object; each checks the type before reading it.
    pub(crate) unsafe fn string_of(v: CFTypeRef) -> String {
        if v.is_null() || CFGetTypeID(v) != CFStringGetTypeID() {
            return String::new();
        }
        let s = v as CFStringRef;
        let max = CFStringGetMaximumSizeForEncoding(CFStringGetLength(s), kCFStringEncodingUTF8) + 1;
        let mut buf = vec![0u8; max.max(1) as usize];
        if CFStringGetCString(s, buf.as_mut_ptr() as *mut c_char, max, kCFStringEncodingUTF8) == 0 {
            return String::new();
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }

    pub(crate) unsafe fn bool_of(v: CFTypeRef) -> Option<bool> {
        if v.is_null() || CFGetTypeID(v) != CFBooleanGetTypeID() {
            return None;
        }
        Some(CFBooleanGetValue(v as CFBooleanRef))
    }

    pub(crate) unsafe fn ax_value<T: Default>(v: CFTypeRef, kind: u32) -> Option<T> {
        if v.is_null() || CFGetTypeID(v) != AXValueGetTypeID() || AXValueGetType(v) != kind {
            return None;
        }
        let mut out = T::default();
        (AXValueGetValue(v, kind, &mut out as *mut T as *mut c_void) != 0).then_some(out)
    }

    pub(crate) unsafe fn children_of(v: CFTypeRef) -> Vec<Owned> {
        if v.is_null() || CFGetTypeID(v) != CFArrayGetTypeID() {
            return Vec::new();
        }
        let a = v as CFArrayRef;
        (0..CFArrayGetCount(a)).filter_map(|i| Owned::retain(CFArrayGetValueAtIndex(a, i))).collect()
    }

    pub(crate) fn copy_attr(el: &Owned, attr: &str) -> Option<Owned> {
        let name = cf_string(attr);
        let mut out: CFTypeRef = std::ptr::null();
        // SAFETY: `el` is a live AXUIElement; `out` receives a +1 reference on success.
        let err = unsafe { AXUIElementCopyAttributeValue(el.0, name.0 as CFStringRef, &mut out) };
        if err == 0 {
            Owned::new(out)
        } else {
            None
        }
    }

    pub(crate) fn set_bool(el: &Owned, attr: &str, value: bool) -> bool {
        let name = cf_string(attr);
        // SAFETY: the CF boolean constants are immortal; `el` is a live AXUIElement.
        let v = unsafe { if value { kCFBooleanTrue } else { kCFBooleanFalse } };
        unsafe { AXUIElementSetAttributeValue(el.0, name.0 as CFStringRef, v as CFTypeRef) == 0 }
    }

    pub(crate) fn trusted() -> bool {
        // SAFETY: plain query, no arguments.
        unsafe { AXIsProcessTrusted() != 0 }
    }

    /// What one element says about itself, read in a single cross-process call.
    pub(crate) struct Node {
        pub(crate) role: String,
        pub(crate) subrole: String,
        pub(crate) title: String,
        pub(crate) description: String,
        pub(crate) placeholder: String,
        pub(crate) help: String,
        pub(crate) frame: Option<Rect>,
        pub(crate) enabled: bool,
        pub(crate) children: Vec<Owned>,
    }

    pub(crate) const NODE_ATTRS: &[&str] = &[
        "AXRole",
        "AXSubrole",
        "AXTitle",
        "AXDescription",
        "AXPlaceholderValue",
        "AXHelp",
        "AXPosition",
        "AXSize",
        "AXEnabled",
        "AXChildren",
    ];

    pub(crate) fn read_node(el: &Owned, attrs: &Names) -> Option<Node> {
        let mut values: CFArrayRef = std::ptr::null();
        // SAFETY: `el` is a live AXUIElement and `attrs.array` a CFArray of CFStrings; `values` receives a +1 array
        // with one entry per attribute (an AXValue error placeholder where an attribute is missing).
        let err = unsafe { AXUIElementCopyMultipleAttributeValues(el.0, attrs.array.0 as CFArrayRef, 0, &mut values) };
        if err != 0 {
            return None;
        }
        let values = Owned::new(values as CFTypeRef)?;
        let arr = values.0 as CFArrayRef;
        // SAFETY: index is bounds-checked against the array count; entries stay alive while `values` is held.
        let at = |i: CFIndex| unsafe {
            if i < CFArrayGetCount(arr) {
                CFArrayGetValueAtIndex(arr, i)
            } else {
                std::ptr::null()
            }
        };
        unsafe {
            let pos: Option<CGPoint> = ax_value(at(6), AX_VALUE_CGPOINT);
            let size: Option<CGSize> = ax_value(at(7), AX_VALUE_CGSIZE);
            let frame = match (pos, size) {
                (Some(p), Some(s)) => Some(Rect { x: p.x, y: p.y, w: s.w, h: s.h }),
                _ => None,
            };
            Some(Node {
                role: string_of(at(0)),
                subrole: string_of(at(1)),
                title: string_of(at(2)),
                description: string_of(at(3)),
                placeholder: string_of(at(4)),
                help: string_of(at(5)),
                frame,
                // A control that does not report AXEnabled is treated as enabled (static content never is actionable).
                enabled: bool_of(at(8)).unwrap_or(true),
                children: children_of(at(9)),
            })
        }
    }

    /// The words inside a control with no title of its own (a web link is usually a text child): its first few text
    /// children's values, joined.
    pub(crate) fn inner_text(children: &[Owned], attrs: &Names) -> String {
        let mut parts: Vec<String> = Vec::new();
        for child in children.iter().take(4) {
            let mut values: CFArrayRef = std::ptr::null();
            // SAFETY: as in `read_node`.
            let err =
                unsafe { AXUIElementCopyMultipleAttributeValues(child.0, attrs.array.0 as CFArrayRef, 0, &mut values) };
            let Some(values) = (err == 0).then(|| Owned::new(values as CFTypeRef)).flatten() else { continue };
            let arr = values.0 as CFArrayRef;
            // SAFETY: two attributes were requested; entries live while `values` is held.
            let (value, title) = unsafe {
                if CFArrayGetCount(arr) < 2 {
                    continue;
                }
                (string_of(CFArrayGetValueAtIndex(arr, 0)), string_of(CFArrayGetValueAtIndex(arr, 1)))
            };
            let text = if value.trim().is_empty() { title } else { value };
            if !text.trim().is_empty() {
                parts.push(text);
            }
        }
        parts.join(" ")
    }

    /// Pids for which the web tree was already requested (the request is per process; the tree builds asynchronously).
    static WEB_TREE_ASKED: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

    /// Ask Electron (`AXManualAccessibility`, refused harmlessly by other apps) and Chromium browsers
    /// (`AXEnhancedUserInterface`) to build their web content tree. True the first time it is asked for this process,
    /// so the caller can give the tree a moment to appear.
    pub(crate) fn ask_for_web_tree(app: &Owned, pid: i32, bundle_id: &str) -> bool {
        let Ok(mut asked) = WEB_TREE_ASKED.lock() else { return false };
        if asked.contains(&pid) {
            return false;
        }
        asked.push(pid);
        let electron = set_bool(app, "AXManualAccessibility", true);
        let chromium = super::wants_enhanced_ui(bundle_id) && set_bool(app, "AXEnhancedUserInterface", true);
        electron || chromium
    }

    /// The app in front (never NEXUS itself), its application element and its focused (else main) window. Sets the
    /// process-wide per-call timeout first. Err carries a plain reason for the log.
    pub(crate) fn front_window() -> Result<(crate::macos_foreground::FrontApp, Owned, Owned), String> {
        if !trusted() {
            return Err("Accessibility permission missing (System Settings → Privacy & Security → Accessibility)".into());
        }
        let front = crate::macos_foreground::front_app().ok_or("cannot tell which app is in front")?;
        if front.pid == std::process::id() {
            return Err("NEXUS itself is in front".into());
        }
        // SAFETY: plain constructors; both return +1 references.
        let system = Owned::new(unsafe { AXUIElementCreateSystemWide() }).ok_or("no system-wide element")?;
        unsafe { AXUIElementSetMessagingTimeout(system.0, MESSAGING_TIMEOUT_S) };
        let app = Owned::new(unsafe { AXUIElementCreateApplication(front.pid as i32) }).ok_or("no application element")?;
        let window = copy_attr(&app, "AXFocusedWindow")
            .or_else(|| copy_attr(&app, "AXMainWindow"))
            .ok_or_else(|| format!("{} has no window in front", front.name))?;
        Ok((front, app, window))
    }

    /// The front app and its window's frame in global top-left points.
    pub(crate) fn front_window_frame() -> Option<(crate::macos_foreground::FrontApp, Rect)> {
        let (front, _app, window) = front_window().ok()?;
        let attrs = names(&["AXPosition", "AXSize"]);
        let mut values: CFArrayRef = std::ptr::null();
        // SAFETY: as in `read_node`.
        let err = unsafe { AXUIElementCopyMultipleAttributeValues(window.0, attrs.array.0 as CFArrayRef, 0, &mut values) };
        let values = (err == 0).then(|| Owned::new(values as CFTypeRef)).flatten()?;
        let arr = values.0 as CFArrayRef;
        // SAFETY: two attributes requested; entries live while `values` is held.
        let (p, s) = unsafe {
            if CFArrayGetCount(arr) < 2 {
                return None;
            }
            let p: CGPoint = ax_value(CFArrayGetValueAtIndex(arr, 0), AX_VALUE_CGPOINT)?;
            let s: CGSize = ax_value(CFArrayGetValueAtIndex(arr, 1), AX_VALUE_CGSIZE)?;
            (p, s)
        };
        Some((front, Rect { x: p.x, y: p.y, w: s.w, h: s.h }))
    }

    pub(super) fn list(budget: Duration, max_nodes: usize) -> Result<Vec<UiElement>, String> {
        let deadline = Instant::now() + budget;
        let (front, app, mut window) = front_window()?;
        if ask_for_web_tree(&app, front.pid as i32, front.bundle_id.as_deref().unwrap_or("")) {
            std::thread::sleep(Duration::from_millis(300));
            if let Some(w) = copy_attr(&app, "AXFocusedWindow") {
                window = w;
            }
        }

        let attrs = names(NODE_ATTRS);
        let text_attrs = names(&["AXValue", "AXTitle"]);
        let win = read_node(&window, &attrs).ok_or("the front window did not answer")?;
        let view = win.frame.ok_or("the front window has no frame")?;

        let mut out: Vec<UiElement> = Vec::new();
        let mut stack: Vec<(Owned, Rect, u16)> = win.children.into_iter().rev().map(|c| (c, view, 1)).collect();
        let mut visited = 0usize;
        while let Some((el, clip, depth)) = stack.pop() {
            if Instant::now() >= deadline {
                return Err(format!("walk exceeded {}ms after {visited} elements", budget.as_millis()));
            }
            visited += 1;
            if visited > max_nodes {
                tracing::warn!("[AX] stopped at {max_nodes} elements; later controls are not listed");
                break;
            }
            let Some(node) = read_node(&el, &attrs) else { continue };
            // An element with a real frame that lies entirely outside the visible area is scrolled away, and so is
            // everything inside it: skip the subtree. Elements without a frame (some groups) are only passed through.
            let mut inner_clip = clip;
            let visible = match node.frame {
                Some(f) if f.w > 0.0 && f.h > 0.0 => match f.intersect(&clip) {
                    Some(v) => Some(v),
                    None => continue,
                },
                _ => None,
            };
            if node.role == "AXScrollArea" {
                if let Some(v) = visible {
                    inner_clip = v;
                }
            }
            if let (Some(kind), Some(v)) = (kind_for_role(&node.role, &node.subrole), visible) {
                if node.enabled {
                    let mut name = pick_name(&[&node.title, &node.description, &node.placeholder]);
                    if name.is_empty() {
                        name = pick_name(&[&inner_text(&node.children, &text_attrs), &node.help]);
                    }
                    let (x, y, w, h) = v.rounded();
                    if crate::screen::accept_actionable(true, false, &name, w, h) {
                        out.push(UiElement { name, kind: kind.to_string(), x, y, w, h });
                    }
                }
                // A control's own children (its label text, icon) are not separate targets.
                continue;
            }
            if depth < 64 {
                stack.extend(node.children.into_iter().rev().map(|c| (c, inner_clip, depth + 1)));
            }
        }
        tracing::debug!("[AX] walked {visited} elements of {} ({})", front.name, front.key());
        Ok(crate::screen::sort_reading_order(out))
    }

    /// Test helper: the values of the static texts in the front window (Calculator's display, a dialog's labels).
    #[cfg(test)]
    pub(super) fn front_window_texts() -> Vec<String> {
        let Some(front) = crate::macos_foreground::front_app() else { return Vec::new() };
        let Some(app) = Owned::new(unsafe { AXUIElementCreateApplication(front.pid as i32) }) else { return Vec::new() };
        let Some(window) = copy_attr(&app, "AXFocusedWindow") else { return Vec::new() };
        let attrs = names(NODE_ATTRS);
        let mut out = Vec::new();
        let mut stack = vec![window];
        while let Some(el) = stack.pop() {
            let Some(node) = read_node(&el, &attrs) else { continue };
            if node.role == "AXStaticText" {
                if let Some(v) = copy_attr(&el, "AXValue") {
                    out.push(unsafe { string_of(v.0) });
                }
            }
            stack.extend(node.children);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actionable_roles_map_to_the_windows_kind_names() {
        assert_eq!(kind_for_role("AXButton", ""), Some("Button"));
        assert_eq!(kind_for_role("AXLink", ""), Some("Hyperlink"));
        assert_eq!(kind_for_role("AXRadioButton", "AXTabButton"), Some("TabItem"));
        assert_eq!(kind_for_role("AXRadioButton", ""), Some("RadioButton"));
        assert_eq!(kind_for_role("AXTextField", "AXSearchField"), Some("Edit"));
        assert_eq!(kind_for_role("AXPopUpButton", ""), Some("ComboBox"));
    }

    #[test]
    fn secure_fields_and_static_content_never_resolve() {
        assert_eq!(kind_for_role("AXTextField", "AXSecureTextField"), None);
        assert_eq!(kind_for_role("AXStaticText", ""), None);
        assert_eq!(kind_for_role("AXGroup", ""), None);
        assert_eq!(kind_for_role("AXImage", ""), None);
        assert_eq!(kind_for_role("", ""), None);
    }

    #[test]
    fn name_is_the_first_non_empty_candidate_with_whitespace_collapsed() {
        assert_eq!(pick_name(&["", "  ", "Save"]), "Save");
        assert_eq!(pick_name(&["Open", "Description"]), "Open");
        assert_eq!(pick_name(&["  Learn\n   more  "]), "Learn more");
        assert_eq!(pick_name(&["", ""]), "");
        assert_eq!(pick_name(&[&"x".repeat(500)]).len(), 200);
    }

    #[test]
    fn only_chromium_browsers_get_the_enhanced_ui_request() {
        assert!(wants_enhanced_ui("com.google.Chrome"));
        assert!(wants_enhanced_ui("com.brave.Browser"));
        assert!(!wants_enhanced_ui("com.apple.Safari"));
        assert!(!wants_enhanced_ui("com.apple.finder"));
        assert!(!wants_enhanced_ui(""));
    }

    #[test]
    fn intersection_clips_to_the_visible_part() {
        let view = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let half = Rect { x: 50.0, y: 80.0, w: 100.0, h: 40.0 };
        assert_eq!(half.intersect(&view), Some(Rect { x: 50.0, y: 80.0, w: 50.0, h: 20.0 }));
        let below = Rect { x: 10.0, y: 150.0, w: 10.0, h: 10.0 };
        assert_eq!(below.intersect(&view), None, "scrolled out of view");
        let touching = Rect { x: 100.0, y: 0.0, w: 10.0, h: 10.0 };
        assert_eq!(touching.intersect(&view), None, "sharing an edge is not overlap");
        let empty = Rect { x: 10.0, y: 10.0, w: 0.0, h: 10.0 };
        assert_eq!(empty.intersect(&view), None);
        let nan = Rect { x: f64::NAN, y: 0.0, w: 10.0, h: 10.0 };
        assert_eq!(nan.intersect(&view), None);
    }

    #[test]
    fn rounded_box_uses_whole_points() {
        let r = Rect { x: 10.4, y: 20.6, w: 30.5, h: 0.4 };
        assert_eq!(r.rounded(), (10, 21, 31, 0));
    }

    /// Live: lists the controls of the window in front (run from a terminal with the Accessibility permission, with a
    /// normal app window in front, e.g. `cargo test --lib ax_mac -- --ignored --nocapture`).
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs the Accessibility permission and a desktop session"]
    fn lists_the_front_window_controls() {
        assert!(trusted(), "grant Accessibility to the terminal first");
        let t0 = std::time::Instant::now();
        let els = list_actionables();
        eprintln!("{} controls in {} ms", els.len(), t0.elapsed().as_millis());
        for e in els.iter().take(40) {
            eprintln!("  {:<12} {:<40} @ {},{} {}x{}", e.kind, e.name, e.x, e.y, e.w, e.h);
        }
        assert!(!els.is_empty(), "a normal window has at least one control");
    }

    /// Live end to end through the calls the voice commands make: "click the seventh" (list_actionables → pick_ordinal →
    /// click_element) and "click Add" (resolve_element → click_at with its glide and pointer check), then reads the
    /// display back. Opens Calculator (quits it again if it was not running) and moves the pointer, then puts it back.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "drives Calculator: needs the Accessibility permission and a desktop; moves the pointer"]
    fn drives_calculator_by_number_and_by_name() {
        use crate::live::commands::mouse;
        use std::process::Command;
        use std::time::{Duration, Instant};
        assert!(trusted(), "grant Accessibility to the terminal first");
        let was_running = Command::new("pgrep").args(["-x", "Calculator"]).status().is_ok_and(|s| s.success());
        let home = mouse::current_pos();
        Command::new("open").args(["-a", "Calculator"]).status().expect("open Calculator");
        let t0 = Instant::now();
        while crate::macos_foreground::front_app().is_none_or(|f| f.name != "Calculator") {
            assert!(t0.elapsed() < Duration::from_secs(10), "Calculator never came to the front");
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(400));
        let by_name = |name: &str| {
            let el = mouse::resolve_element(name).unwrap_or_else(|| panic!("no '{name}' control"));
            let (x, y) = mouse::element_center(&el);
            mouse::click_at(x, y, || false).unwrap_or_else(|e| panic!("click {name}: {e}"));
            std::thread::sleep(Duration::from_millis(150));
        };
        // "Clear" matches both C and All Clear; twice resets an entry and any pending operation.
        by_name("Clear");
        by_name("Clear");
        let els = crate::screen::list_actionables();
        let seventh = crate::screen::pick_ordinal(&els, 7).expect("at least seven controls").clone();
        assert_eq!(seventh.name, "7", "toolbar (2), Delete, All Clear, Per cent, Divide, then 7");
        crate::screen::click_element(&seventh).expect("ordinal click");
        std::thread::sleep(Duration::from_millis(150));
        by_name("Add");
        by_name("3");
        by_name("Equals");
        std::thread::sleep(Duration::from_millis(300));
        let texts = mac::front_window_texts();
        eprintln!("display texts: {texts:?}");
        if !was_running {
            let _ = Command::new("pkill").args(["-x", "Calculator"]).status();
        }
        if let Some(p) = home {
            let _ = mouse::restore(p);
        }
        assert!(texts.iter().any(|t| t.trim() == "10"), "7 + 3 should show 10, got {texts:?}");
    }
}
