//! Screen vision + control — see the screen, resolve ordinals, click.
//!
//! Grounding order (see docs/mcp/09-screen-vision-control.md):
//!   1. Browser tabs → hotkeys first (Ctrl+1..8, 100% reliable, instant).
//!   2. Everything else → Windows UIA tree (exact names + boxes, free).
//!   3. Canvas/custom UI → future (vision model / OCR), not this module.
//!
//! Ordinal rule: visible + enabled actionables in the foreground window,
//! sorted top-to-bottom then left-to-right. "3rd option" = 3rd actionable.
//! Password/secure fields never resolve; destructive targets need confirm
//! (enforced by callers, not here).

/// One actionable UI element.
#[derive(Debug, Clone, PartialEq)]
pub struct UiElement {
    pub name: String,
    pub kind: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Sort elements in reading order: top-to-bottom, then left-to-right.
/// Pure function — unit-tested below.
pub fn sort_reading_order(mut els: Vec<UiElement>) -> Vec<UiElement> {
    // Row tolerance: items within 12px vertically share a row.
    els.sort_by(|a, b| {
        let row_a = a.y / 12;
        let row_b = b.y / 12;
        row_a.cmp(&row_b).then(a.x.cmp(&b.x))
    });
    els
}

/// 1-based ordinal pick. Returns None when out of range.
pub fn pick_ordinal(sorted: &[UiElement], n: u32) -> Option<&UiElement> {
    if n == 0 {
        return None;
    }
    sorted.get((n - 1) as usize)
}

/// Parse "3rd", "2nd", "1st", "4th", "12th", bare "3", and word ordinals ("first", "second", "one", "two") into a number.
pub fn parse_ordinal(text: &str) -> Option<u32> {
    let t = text.trim().to_lowercase();
    match t.as_str() {
        "first" | "one" | "won" => return Some(1),
        "second" | "two" | "to" | "too" => return Some(2),
        "third" | "three" => return Some(3),
        "fourth" | "four" | "for" => return Some(4),
        "fifth" | "five" => return Some(5),
        "sixth" | "six" => return Some(6),
        "seventh" | "seven" => return Some(7),
        "eighth" | "eight" | "ate" => return Some(8),
        "ninth" | "nine" => return Some(9),
        "tenth" | "ten" => return Some(10),
        _ => {}
    }
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    // Accept "3", "3rd", "3th"-ish; reject trailing junk like "3x".
    let rest = t[digits.len()..].trim_start();
    if !rest.is_empty()
        && !["st", "nd", "rd", "th"].iter().any(|suf| rest.starts_with(suf))
    {
        return None;
    }
    digits.parse().ok()
}

/// Shared acceptance rule for a candidate control (pure, unit-tested). Same as the legacy filter
/// (enabled, named, not a password field, has bounds) plus **on-screen**: an off-screen control
/// cannot be clicked, so offering it as "the 3rd option" sends the cursor into the void.
pub fn accept_actionable(enabled: bool, offscreen: bool, name: &str, w: i32, h: i32) -> bool {
    if !enabled || offscreen || w <= 0 || h <= 0 {
        return false;
    }
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    // Skip password/secure fields by name.
    !name.to_lowercase().contains("password")
}

/// Wall-clock budget for the fast lookup. UIA calls block on the target app, so a hung or
/// enormous tree must not stall the voice pipeline: on expiry the caller gets an empty list
/// (and falls through to its next tier) instead of waiting seconds.
pub const FAST_LOOKUP_BUDGET_MS: u64 = 2500;

#[cfg(target_os = "windows")]
mod win {
    use super::UiElement;
    use uiautomation::controls::ControlType;
    use uiautomation::UIAutomation;

    const CLICKABLE: &[ControlType] = &[
        ControlType::Button,
        ControlType::Hyperlink,
        ControlType::TabItem,
        ControlType::MenuItem,
        ControlType::ListItem,
        ControlType::CheckBox,
        ControlType::RadioButton,
        ControlType::SplitButton,
        ControlType::Edit,
        ControlType::ComboBox,
    ];

    /// Outcome of the fast lookup (the dispatcher decides what each case means).
    enum Fast {
        Done(Vec<UiElement>),
        /// Budget exhausted - the app is slow/hung or the tree is huge. Do NOT retry with the
        /// legacy path (it is slower still); the caller falls through to its next tier.
        TimedOut,
        /// UIA could not be used at all (no foreground window, COM failure...) - legacy may still work.
        Unavailable(String),
    }

    /// UIA element for the foreground WINDOW (not the focused leaf).
    ///
    /// `UIMatcher::from(el)` only searches `el`'s DESCENDANTS. Rooting at
    /// `get_focused_element()` meant a focused leaf — the omnibox after Ctrl+T,
    /// a text box, a button — has no children, so "click X" / "read the 2nd
    /// result" found nothing (and fell to paid vision) whenever focus sat on
    /// anything but a container.
    pub fn foreground_window_element(automation: &UIAutomation) -> Option<uiautomation::UIElement> {
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0 == 0 {
            return None;
        }
        automation
            .element_from_handle(uiautomation::types::Handle::from(hwnd.0))
            .ok()
    }

    /// List actionable elements in the foreground window via UIA.
    ///
    /// ONE cached query rooted at the **foreground window** (not the focused element): the old
    /// path searched under the focused element, so a leaf focus (text box, document) returned
    /// nothing after ten 500 ms timeouts - measured 5.1 s and 0 controls in Notepad/Calculator,
    /// versus 18-79 ms and full recall with this query (docs/research/ghost-mode/11-...).
    /// Set `NEXUS_UIA_LEGACY=1` to force the old behaviour (kill switch).
    pub fn list_actionables() -> Vec<UiElement> {
        if std::env::var("NEXUS_UIA_LEGACY").map(|v| v == "1").unwrap_or(false) {
            return list_actionables_legacy();
        }
        let t0 = std::time::Instant::now();
        match fast_with_budget(super::FAST_LOOKUP_BUDGET_MS) {
            Fast::Done(v) => {
                tracing::info!(
                    "[UIA] list_actionables: {} controls in {}ms (window-rooted, cached)",
                    v.len(),
                    t0.elapsed().as_millis()
                );
                v
            }
            Fast::TimedOut => {
                tracing::warn!(
                    "[UIA] list_actionables: exceeded {}ms budget - returning empty (caller falls through)",
                    super::FAST_LOOKUP_BUDGET_MS
                );
                Vec::new()
            }
            Fast::Unavailable(why) => {
                tracing::warn!("[UIA] fast lookup unavailable ({why}) - using legacy focused-element query");
                list_actionables_legacy()
            }
        }
    }

    /// Run the cached query on a worker thread so a blocked UIA call cannot hold the caller past
    /// the budget (the thread finishes on its own; one stuck call leaks one thread at worst).
    fn fast_with_budget(budget_ms: u64) -> Fast {
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<UiElement>, String>>();
        let spawned = std::thread::Builder::new()
            .name("uia-fast-lookup".into())
            .spawn(move || {
                let _ = tx.send(fast_query());
            });
        if let Err(e) = spawned {
            return Fast::Unavailable(format!("thread spawn failed: {e}"));
        }
        match rx.recv_timeout(std::time::Duration::from_millis(budget_ms)) {
            Ok(Ok(v)) => Fast::Done(v),
            Ok(Err(why)) => Fast::Unavailable(why),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Fast::TimedOut,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Fast::Unavailable("lookup thread ended without a result".into())
            }
        }
    }

    fn fast_query() -> Result<Vec<UiElement>, String> {
        use uiautomation::types::{Handle, TreeScope, UIProperty};
        use uiautomation::variants::Variant;
        let err = |e: uiautomation::errors::Error| e.to_string();

        let automation = UIAutomation::new().map_err(err)?;
        let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        if hwnd.0 == 0 {
            return Err("no foreground window".into());
        }
        let window = automation.element_from_handle(Handle::from(hwnd.0)).map_err(err)?;

        // Server-side filter: only the clickable control types cross the process boundary
        // (a Chromium page has thousands of Text/Group nodes we never use).
        let mut cond = None;
        for ct in CLICKABLE {
            let c = automation
                .create_property_condition(UIProperty::ControlType, Variant::from(*ct as i32), None)
                .map_err(err)?;
            cond = Some(match cond {
                None => c,
                Some(acc) => automation.create_or_condition(acc, c).map_err(err)?,
            });
        }
        let cond = cond.ok_or_else(|| "no clickable control types configured".to_string())?;

        let cache = automation.create_cache_request().map_err(err)?;
        for p in [
            UIProperty::Name,
            UIProperty::ControlType,
            UIProperty::BoundingRectangle,
            UIProperty::IsEnabled,
            UIProperty::IsOffscreen,
        ] {
            cache.add_property(p).map_err(err)?;
        }

        let found = window
            .find_all_build_cache(TreeScope::Descendants, &cond, &cache)
            .map_err(err)?;

        let mut out = Vec::with_capacity(found.len());
        for el in &found {
            // Cached reads only - no further cross-process calls. Any failing element is skipped.
            let Ok(ct) = el.get_cached_control_type() else { continue };
            let name = el.get_cached_name().unwrap_or_default();
            let enabled = el.is_cached_enabled().unwrap_or(false);
            let offscreen = el.is_cached_offscreen().unwrap_or(true);
            let Ok(r) = el.get_cached_bounding_rectangle() else { continue };
            let (x, y, w, h) = (r.get_left(), r.get_top(), r.get_width(), r.get_height());
            if !super::accept_actionable(enabled, offscreen, &name, w, h) {
                continue;
            }
            out.push(UiElement { name, kind: format!("{ct:?}"), x, y, w, h });
        }
        Ok(super::sort_reading_order(out))
    }

    /// The original focused-element-rooted lookup (10 sequential queries). Kept as the explicit
    /// fallback when the fast path cannot run, and as the kill-switch target. Slow (2.5-5 s) and
    /// blind when focus is on a leaf element - do not call it directly from new code.
    pub fn list_actionables_legacy() -> Vec<UiElement> {
        let automation = match UIAutomation::new() {
            Ok(a) => a,
            Err(_) => return Vec::new(),
        };
        // Foreground window first; the focused element is only a fallback.
        let focused = match foreground_window_element(&automation)
            .or_else(|| automation.get_focused_element().ok())
        {
            Some(e) => e,
            None => return Vec::new(),
        };
        let mut out = Vec::new();
        for ct in CLICKABLE {
            let found = automation
                .create_matcher()
                .from(focused.clone())
                .timeout(500)
                .control_type(*ct)
                .find_all();
            let elements = match found {
                Ok(e) => e,
                Err(_) => continue,
            };
            for el in elements.iter() {
                let enabled = el.is_enabled().unwrap_or(false);
                if !enabled {
                    continue;
                }
                let name = el.get_name().unwrap_or_default();
                if name.trim().is_empty() {
                    continue;
                }
                let kind = format!("{:?}", ct);
                // Skip password/secure fields and address bars by name.
                let lower = name.to_lowercase();
                if lower.contains("password") {
                    continue;
                }
                let (x, y, w, h) = match el.get_bounding_rectangle() {
                    Ok(r) => (r.get_left(), r.get_top(), r.get_width(), r.get_height()),
                    Err(_) => continue,
                };
                if w <= 0 || h <= 0 {
                    continue;
                }
                out.push(UiElement {
                    name,
                    kind,
                    x,
                    y,
                    w,
                    h,
                });
            }
        }
        super::sort_reading_order(out)
    }

    /// Move the mouse to an element's center and left-click.
    pub fn click_element(el: &UiElement) -> Result<(), String> {
        use enigo::{Button, Coordinate, Direction, Enigo, Mouse, Settings};
        let mut enigo =
            Enigo::new(&Settings::default()).map_err(|e| format!("enigo init: {e}"))?;
        let cx = el.x + el.w / 2;
        let cy = el.y + el.h / 2;
        enigo
            .move_mouse(cx, cy, Coordinate::Abs)
            .map_err(|e| format!("mouse move: {e}"))?;
        enigo
            .button(Button::Left, Direction::Click)
            .map_err(|e| format!("mouse click: {e}"))?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
pub use win::{click_element, foreground_window_element, list_actionables, list_actionables_legacy};

#[cfg(not(target_os = "windows"))]
pub use unix_extra::{list_actionables, primary_monitor_size};
#[cfg(target_os = "macos")]
pub use unix_extra::click_element;

#[cfg(target_os = "windows")]
mod win_extra {
    /// Primary monitor size in physical px (matches GetWindowRect space,
    /// so fullscreen detection compares apples to apples).
    pub fn primary_monitor_size() -> Option<(i32, i32)> {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
        };
        unsafe {
            let w = GetSystemMetrics(SM_CXSCREEN);
            let h = GetSystemMetrics(SM_CYSCREEN);
            if w > 0 && h > 0 {
                Some((w, h))
            } else {
                None
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub use win_extra::primary_monitor_size;

#[cfg(not(target_os = "windows"))]
mod unix_extra {
    use super::UiElement;

    /// macOS: the front window's controls through the Accessibility API (`ax_mac`). Elsewhere: none, so grounding
    /// falls back to hotkeys/coarse clicks.
    pub fn list_actionables() -> Vec<UiElement> {
        #[cfg(target_os = "macos")]
        {
            crate::ax_mac::list_actionables()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Vec::new()
        }
    }

    /// Glide to an element's center and left-click (points, the space the Accessibility boxes use). Not a teleport
    /// plus instant click as on Windows: on macOS a click posted right after a jump lands at the old pointer position
    /// (measured with Calculator: the "7" never registered). The glide, arrival dwell and pointer check of
    /// `mouse::click_at` are what "click X" by name already uses.
    #[cfg(target_os = "macos")]
    pub fn click_element(el: &UiElement) -> Result<(), String> {
        let (x, y) = crate::live::commands::mouse::element_center(el);
        crate::live::commands::mouse::click_at(x, y, || false)
    }

    /// Primary display in macOS POINTS (probe via `screen_capture_mac`), so
    /// the capture path's coordinate space matches the vision stack's math.
    pub fn primary_monitor_size() -> Option<(i32, i32)> {
        Some(crate::screen_capture_mac::primary_points())
    }
}

/// Switch browser tab by index (1-based): Ctrl+1..8, Ctrl+9 = last.
/// Hotkeys beat grounding for tabs — 100% reliable, instant.
pub fn switch_browser_tab(index: u32) -> Result<String, String> {
    if index == 0 || index > 9 {
        return Err("tab number must be 1-9".to_string());
    }
    // Shortcut brain (E1): layout-independent virtual-key chord for the app in front, foreground re-checked
    // right before sending. Unknown apps keep the legacy Ctrl+digit below.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if crate::shortcuts::enabled() {
        use crate::shortcuts::{Action, Spoken};
        match crate::shortcuts::do_action_spoken(Action::GotoTab, Some(index), true) {
            Spoken::Ok(line) => return Ok(line),
            Spoken::Refused(reason) => return Err(reason),
            Spoken::Fallback => {} // unknown app -> legacy path below
        }
    }
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    // Windows/Linux browsers switch tabs with Ctrl+digit, Chromium-family browsers on a Mac with Cmd+digit.
    let primary = if cfg!(target_os = "macos") { Key::Meta } else { Key::Control };
    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| format!("enigo init: {e}"))?;
    enigo
        .key(primary, Direction::Press)
        .map_err(|e| format!("modifier press: {e}"))?;
    let digit = match index {
        1 => Key::Unicode('1'),
        2 => Key::Unicode('2'),
        3 => Key::Unicode('3'),
        4 => Key::Unicode('4'),
        5 => Key::Unicode('5'),
        6 => Key::Unicode('6'),
        7 => Key::Unicode('7'),
        8 => Key::Unicode('8'),
        _ => Key::Unicode('9'),
    };
    let r = enigo
        .key(digit, Direction::Click)
        .map_err(|e| format!("digit: {e}"));
    let _ = enigo.key(primary, Direction::Release);
    r?;
    Ok(format!("Switched to tab {index}, sir."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(name: &str, x: i32, y: i32) -> UiElement {
        UiElement {
            name: name.into(),
            kind: "Button".into(),
            x,
            y,
            w: 80,
            h: 30,
        }
    }

    #[test]
    fn reading_order_top_to_bottom_then_left_to_right() {
        let v = sort_reading_order(vec![el("b", 200, 10), el("a", 10, 10), el("c", 10, 100)]);
        assert_eq!(v[0].name, "a");
        assert_eq!(v[1].name, "b");
        assert_eq!(v[2].name, "c");
    }

    #[test]
    fn accept_actionable_rules() {
        // Happy path.
        assert!(accept_actionable(true, false, "Submit", 80, 30));
        assert!(accept_actionable(true, false, "  Save  ", 1, 1));
        // Each rejection reason on its own.
        assert!(!accept_actionable(false, false, "Submit", 80, 30), "disabled");
        assert!(!accept_actionable(true, true, "Submit", 80, 30), "off-screen can't be clicked");
        assert!(!accept_actionable(true, false, "", 80, 30), "unnamed");
        assert!(!accept_actionable(true, false, "   ", 80, 30), "blank name");
        assert!(!accept_actionable(true, false, "Submit", 0, 30), "no width");
        assert!(!accept_actionable(true, false, "Submit", 80, -1), "no height");
        // Password fields never resolve, any casing.
        assert!(!accept_actionable(true, false, "Enter Password", 80, 30));
        assert!(!accept_actionable(true, false, "PASSWORD", 80, 30));
    }

    #[test]
    fn fast_lookup_budget_stays_well_under_the_legacy_cost() {
        // Legacy burned 10 x 500 ms; the fast path's hard cap must stay below that.
        assert!(FAST_LOOKUP_BUDGET_MS < 5000);
        assert!(FAST_LOOKUP_BUDGET_MS >= 1000, "too tight for a cold Chromium tree");
    }

    #[test]
    fn ordinal_pick_one_based() {
        let v = vec![el("a", 0, 0), el("b", 0, 50)];
        assert_eq!(pick_ordinal(&v, 1).unwrap().name, "a");
        assert_eq!(pick_ordinal(&v, 2).unwrap().name, "b");
        assert!(pick_ordinal(&v, 0).is_none());
        assert!(pick_ordinal(&v, 3).is_none());
    }

    #[test]
    fn ordinal_parse_suffixes() {
        assert_eq!(parse_ordinal("3rd"), Some(3));
        assert_eq!(parse_ordinal("1st"), Some(1));
        assert_eq!(parse_ordinal("2nd"), Some(2));
        assert_eq!(parse_ordinal("12th"), Some(12));
        assert_eq!(parse_ordinal("4"), Some(4));
        assert_eq!(parse_ordinal("first"), Some(1));
        assert_eq!(parse_ordinal("second"), Some(2));
        assert_eq!(parse_ordinal("two"), Some(2));
        assert_eq!(parse_ordinal("option"), None);
        assert_eq!(parse_ordinal("3x"), None);
    }
}
