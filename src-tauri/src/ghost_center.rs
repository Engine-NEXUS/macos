//! Ghost Mode command center. The main command center owns the turn and its
//! ownership/validity gates; this center owns only foreground desktop actions.
//! Each sub-center returns an honest receipt. No vision/OCR path exists here.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::shortcuts::{map::Action, router};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GhostStep {
    Shortcut { action: String, arg: Option<u32> },
    AccessibleClick { name: String },
    Learn { alias: String },
    Forget { alias: String },
    LearnedClick { alias: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostTask {
    pub steps: Vec<GhostStep>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    KeyboardCenter,
    AccessibilityCenter,
    /// Only Windows falls back to a pointer click; macOS presses through the Accessibility API.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    CursorCenter,
    LearningCenter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Verified,
    Unconfirmed,
    Blocked,
    Stopped,
}

#[derive(Debug, Clone, Serialize)]
pub struct Receipt {
    pub step: usize,
    pub route: Route,
    pub outcome: Outcome,
    pub detail: String,
    pub elapsed_ms: u64,
}

impl Receipt {
    pub fn spoken(&self) -> String {
        match self.outcome {
            Outcome::Verified => format!("{}, sir.", self.detail),
            Outcome::Unconfirmed => format!(
                "I tried that, sir, but couldn't verify the result. {}",
                self.detail
            ),
            Outcome::Blocked => format!("I couldn't do that safely, sir. {}", self.detail),
            Outcome::Stopped => "Stopped, sir. Ghost mode is still on.".into(),
        }
    }
}

/// Strict, local grammar for low-risk desktop actions. A compound command is
/// accepted only if EVERY step parses; unknown text never becomes a click.
pub fn parse(transcript: &str) -> Option<GhostTask> {
    let mut text = transcript
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .to_lowercase();
    loop {
        let next = ["nexus, ", "nexus ", "please ", "can you ", "could you "]
            .iter()
            .find_map(|prefix| text.strip_prefix(prefix).map(|rest| rest.to_string()));
        match next {
            Some(rest) => text = rest,
            None => break,
        }
    }
    let parts: Vec<&str> = text.split(" then ").collect();
    if parts.is_empty() || parts.len() > 5 {
        return None;
    }
    let steps = parts
        .into_iter()
        .map(parse_step)
        .collect::<Option<Vec<_>>>()?;
    Some(GhostTask { steps })
}

fn parse_step(raw: &str) -> Option<GhostStep> {
    let t = raw.trim().trim_start_matches("please ").trim();
    let t = t.trim_start_matches("the ");
    for prefix in [
        "learn this as ",
        "remember this button as ",
        "remember this control as ",
    ] {
        if let Some(alias) = t.strip_prefix(prefix) {
            return valid_alias(alias).then(|| GhostStep::Learn {
                alias: alias.into(),
            });
        }
    }
    if let Some(alias) = t.strip_prefix("forget learned ") {
        return valid_alias(alias).then(|| GhostStep::Forget {
            alias: alias.into(),
        });
    }
    if let Some(alias) = t.strip_prefix("click learned ") {
        return valid_alias(alias).then(|| GhostStep::LearnedClick {
            alias: alias.into(),
        });
    }
    let action = match t {
        "save" | "save this" | "save file" | "save this file" | "save the file" => {
            Some(Action::Save)
        }
        "copy" | "copy this" | "copy selection" => Some(Action::Copy),
        "paste" | "paste this" => Some(Action::Paste),
        "cut" | "cut this" => Some(Action::Cut),
        "undo" | "undo that" => Some(Action::Undo),
        "redo" | "redo that" => Some(Action::Redo),
        "select all" | "select everything" => Some(Action::SelectAll),
        "refresh" | "reload" | "refresh page" | "reload page" => Some(Action::Refresh),
        "go back" | "back" => Some(Action::GoBack),
        "go forward" | "forward" => Some(Action::GoForward),
        "new tab" | "open new tab" | "open a new tab" => Some(Action::NewTab),
        "close tab" | "close this tab" => Some(Action::CloseTab),
        "reopen tab" | "reopen closed tab" => Some(Action::ReopenTab),
        "next tab" | "switch to next tab" => Some(Action::NextTab),
        "previous tab" | "prev tab" | "switch to previous tab" => Some(Action::PrevTab),
        "address bar" | "focus address bar" => Some(Action::AddressBar),
        "find" | "find in page" | "search in page" => Some(Action::FindInPage),
        "scroll down" => Some(Action::ScrollDown),
        "scroll up" => Some(Action::ScrollUp),
        "zoom in" => Some(Action::ZoomIn),
        "zoom out" => Some(Action::ZoomOut),
        "switch window" => Some(Action::SwitchWindow),
        _ => None,
    };
    if let Some(action) = action {
        return Some(GhostStep::Shortcut {
            action: action.id().into(),
            arg: None,
        });
    }
    for prefix in ["switch to tab ", "go to tab ", "tab "] {
        if let Some(n) = t.strip_prefix(prefix).and_then(|v| v.parse::<u32>().ok()) {
            if (1..=9).contains(&n) {
                return Some(GhostStep::Shortcut {
                    action: Action::GotoTab.id().into(),
                    arg: Some(n),
                });
            }
        }
    }
    let name = t
        .strip_prefix("click ")
        .or_else(|| t.strip_prefix("press "))?
        .trim_start_matches("the ")
        .trim();
    let name = name
        .strip_suffix(" button")
        .or_else(|| name.strip_suffix(" tab"))
        .or_else(|| name.strip_suffix(" option"))
        .unwrap_or(name)
        .trim();
    if name.len() < 2 || name.len() > 80 || is_risky_target(name) || is_ambiguous_target(name) {
        return None;
    }
    Some(GhostStep::AccessibleClick { name: name.into() })
}

pub(crate) fn is_risky_target(name: &str) -> bool {
    let lower = name.to_lowercase();
    if ["sign in", "log in", "close account", "place order"]
        .iter()
        .any(|phrase| lower.contains(phrase))
    {
        return true;
    }
    [
        "send",
        "submit",
        "delete",
        "remove",
        "buy",
        "purchase",
        "pay",
        "transfer",
        "confirm",
        "allow",
        "authorize",
        "install",
        "password",
        "erase",
        "discard",
        "publish",
        "post",
        "withdraw",
        "approve",
        "reject",
        "unsubscribe",
        "disconnect",
        "archive",
        "reset",
        "clear",
        "revoke",
        "grant",
        "share",
    ]
    .iter()
    .any(|word| {
        lower
            .split(|c: char| !c.is_alphanumeric())
            .any(|part| part == *word)
    })
}

fn valid_alias(alias: &str) -> bool {
    let alias = alias.trim();
    (2..=40).contains(&alias.len())
        && alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
        && !is_risky_target(alias)
        && !is_ambiguous_target(alias)
}

pub(crate) fn is_ambiguous_target(name: &str) -> bool {
    let name = name.to_lowercase();
    let base = name
        .strip_suffix(" button")
        .or_else(|| name.strip_suffix(" option"))
        .or_else(|| name.strip_suffix(" tab"))
        .unwrap_or(&name);
    matches!(
        base,
        "here"
            | "there"
            | "this"
            | "that"
            | "it"
            | "one"
            | "button"
            | "option"
            | "ok"
            | "yes"
            | "continue"
            | "next"
            | "accept"
            | "agree"
            | "red"
            | "blue"
            | "green"
            | "yellow"
            | "black"
            | "white"
    ) || name.ends_with(" one")
        || name.ends_with(" thing")
}

pub fn validate(task: &GhostTask) -> bool {
    !task.steps.is_empty()
        && task.steps.len() <= 5
        && task.steps.iter().all(|step| match step {
            GhostStep::Shortcut { action, arg } => Action::from_id(action).is_some_and(|a| {
                a.takes_number() == arg.is_some() && arg.is_none_or(|n| (1..=9).contains(&n))
            }),
            GhostStep::AccessibleClick { name } => {
                name.len() >= 2
                    && name.len() <= 80
                    && !is_risky_target(name)
                    && !is_ambiguous_target(name)
            }
            GhostStep::Learn { alias }
            | GhostStep::Forget { alias }
            | GhostStep::LearnedClick { alias } => valid_alias(alias),
        })
}

/// Sequential execution is intentional: after an unverified step, the next
/// command may target a different UI state and must not be sent blindly.
pub fn execute_with_store(
    task: &GhostTask,
    cancel: &AtomicBool,
    store_dir: Option<&Path>,
) -> Vec<Receipt> {
    if !validate(task) {
        return vec![Receipt {
            step: 0,
            route: Route::KeyboardCenter,
            outcome: Outcome::Blocked,
            detail: "The Ghost command is incomplete or unsafe.".into(),
            elapsed_ms: 0,
        }];
    }
    let mut receipts = Vec::with_capacity(task.steps.len());
    for (index, step) in task.steps.iter().enumerate() {
        if cancel.load(Ordering::Relaxed)
            || crate::ghost::stop_requested()
            || !crate::ghost::session_active()
        {
            receipts.push(Receipt {
                step: index,
                route: route_for(step),
                outcome: Outcome::Stopped,
                detail: "The task was stopped.".into(),
                elapsed_ms: 0,
            });
            break;
        }
        let start = Instant::now();
        let (route, outcome, detail) = match step {
            GhostStep::Shortcut { action, arg } => keyboard_subcenter(action, *arg, cancel),
            GhostStep::AccessibleClick { name } => accessibility_subcenter(name, cancel),
            GhostStep::Learn { alias } => learning_subcenter(alias, store_dir, true),
            GhostStep::Forget { alias } => learning_subcenter(alias, store_dir, false),
            GhostStep::LearnedClick { alias } => learned_click_subcenter(alias, store_dir, cancel),
        };
        let receipt = Receipt {
            step: index,
            route,
            outcome,
            detail,
            elapsed_ms: start.elapsed().as_millis() as u64,
        };
        tracing::info!(
            "[GHOST-CENTER] step={} route={route:?} outcome={outcome:?} elapsed_ms={}",
            index,
            receipt.elapsed_ms
        );
        receipts.push(receipt);
        if outcome != Outcome::Verified {
            break;
        }
    }
    receipts
}

fn route_for(step: &GhostStep) -> Route {
    match step {
        GhostStep::Shortcut { .. } => Route::KeyboardCenter,
        GhostStep::AccessibleClick { .. } => Route::AccessibilityCenter,
        GhostStep::Learn { .. } | GhostStep::Forget { .. } | GhostStep::LearnedClick { .. } => {
            Route::LearningCenter
        }
    }
}

fn learning_subcenter(
    alias: &str,
    store_dir: Option<&Path>,
    teach: bool,
) -> (Route, Outcome, String) {
    let Some(dir) = store_dir else {
        return (
            Route::LearningCenter,
            Outcome::Blocked,
            "I can't access the Ghost learning store.".into(),
        );
    };
    let result = if teach {
        crate::ghost_learning::teach(dir, alias)
    } else {
        crate::ghost_learning::forget(dir, alias)
    };
    match result {
        Ok(detail) => (Route::LearningCenter, Outcome::Verified, detail),
        Err(detail) => (Route::LearningCenter, Outcome::Blocked, detail),
    }
}

fn learned_click_subcenter(
    alias: &str,
    store_dir: Option<&Path>,
    cancel: &AtomicBool,
) -> (Route, Outcome, String) {
    let Some(dir) = store_dir else {
        return (
            Route::LearningCenter,
            Outcome::Blocked,
            "I can't access the Ghost learning store.".into(),
        );
    };
    match crate::ghost_learning::resolve(dir, alias) {
        Ok((name, id)) => accessibility_subcenter_with_id(&name, Some(&id), cancel),
        Err(reason) => (Route::LearningCenter, Outcome::Blocked, reason),
    }
}

fn keyboard_subcenter(
    action_id: &str,
    arg: Option<u32>,
    cancel: &AtomicBool,
) -> (Route, Outcome, String) {
    if cancel.load(Ordering::Relaxed) {
        return (
            Route::KeyboardCenter,
            Outcome::Stopped,
            "The task was stopped.".into(),
        );
    }
    let Some(action) = Action::from_id(action_id) else {
        return (
            Route::KeyboardCenter,
            Outcome::Blocked,
            "Unknown keyboard action.".into(),
        );
    };
    if matches!(
        action,
        Action::Copy
            | Action::Paste
            | Action::Cut
            | Action::Undo
            | Action::Redo
            | Action::SelectAll
    ) && focused_field_is_secure()
    {
        return (
            Route::KeyboardCenter,
            Outcome::Blocked,
            "I can't safely edit a protected or unreadable field.".into(),
        );
    }
    if !crate::shortcuts::enabled() {
        return (
            Route::KeyboardCenter,
            Outcome::Blocked,
            "Keyboard routing is disabled.".into(),
        );
    }
    let out = crate::shortcuts::exec::do_action(action, arg);
    match &out {
        router::Outcome::Sent {
            verified: router::Verified::Yes,
            ..
        } => (
            Route::KeyboardCenter,
            Outcome::Verified,
            format!("{} completed", action.label()),
        ),
        router::Outcome::Sent { .. } => (
            Route::KeyboardCenter,
            Outcome::Unconfirmed,
            format!("The {} shortcut was sent once.", action.label()),
        ),
        router::Outcome::Aborted(router::Abort::Stopped) => (
            Route::KeyboardCenter,
            Outcome::Stopped,
            "The task was stopped.".into(),
        ),
        other => (
            Route::KeyboardCenter,
            Outcome::Blocked,
            router::reason(action, other).unwrap_or_else(|| "The shortcut was refused.".into()),
        ),
    }
}

#[cfg(target_os = "windows")]
fn focused_field_is_secure() -> bool {
    let Ok(automation) = uiautomation::UIAutomation::new() else {
        return true;
    };
    let Ok(focused) = automation.get_focused_element() else {
        return true;
    };
    focused.is_password().unwrap_or(true)
}

#[cfg(target_os = "macos")]
fn focused_field_is_secure() -> bool {
    mac_accessibility::focused_field_is_secure()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn focused_field_is_secure() -> bool {
    true
}

fn accessibility_subcenter(name: &str, cancel: &AtomicBool) -> (Route, Outcome, String) {
    accessibility_subcenter_with_id(name, None, cancel)
}

fn accessibility_subcenter_with_id(
    name: &str,
    expected_id: Option<&str>,
    cancel: &AtomicBool,
) -> (Route, Outcome, String) {
    #[cfg(target_os = "windows")]
    {
        match windows_accessibility::act(name, expected_id, cancel) {
            Ok((route, outcome, detail)) => (route, outcome, detail),
            Err(reason) => (Route::AccessibilityCenter, Outcome::Blocked, reason),
        }
    }
    #[cfg(target_os = "macos")]
    {
        match mac_accessibility::act(name, expected_id, cancel) {
            Ok((route, outcome, detail)) => (route, outcome, detail),
            Err(reason) => (Route::AccessibilityCenter, Outcome::Blocked, reason),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = (name, expected_id, cancel);
        (
            Route::AccessibilityCenter,
            Outcome::Blocked,
            "Accessible Ghost control currently requires Windows or macOS.".into(),
        )
    }
}

// ── macOS Accessibility Center: pure decisions (tested on every OS) ─────────────────────────────────────────────────

/// AXError codes (HIServices `AXError.h`) the press can return.
#[cfg(any(target_os = "macos", test))]
const AX_ERR_INVALID_ELEMENT: i32 = -25202;
#[cfg(any(target_os = "macos", test))]
const AX_ERR_CANNOT_COMPLETE: i32 = -25204;
#[cfg(any(target_os = "macos", test))]
const AX_ERR_ACTION_UNSUPPORTED: i32 = -25206;
#[cfg(any(target_os = "macos", test))]
const AX_ERR_API_DISABLED: i32 = -25211;

/// Spoken (after "I couldn't do that safely, sir.") when NEXUS lacks the Accessibility permission.
#[cfg(any(target_os = "macos", test))]
const MAC_PERMISSION_DETAIL: &str =
    "I need the Accessibility permission for that. It is in System Settings, Privacy and Security, Accessibility.";

/// Names compare the way a person reads them: case and runs of whitespace do not matter (link text often wraps).
#[cfg(any(target_os = "macos", test))]
fn same_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    norm(a) == norm(b)
}

/// The one control a spoken name means, as an index into `controls` (each eligible control's name and `AXIdentifier`).
/// Same rule as Windows: the whole name must match, and a learned control's identifier must match exactly too. No
/// match or several matches is a refusal with the Windows wording: two controls with one name are never guessed between.
#[cfg(any(target_os = "macos", test))]
fn choose_target(
    controls: &[(String, Option<String>)],
    name: &str,
    expected_id: Option<&str>,
) -> Result<usize, String> {
    let hits: Vec<usize> = controls
        .iter()
        .enumerate()
        .filter(|(_, (label, id))| {
            same_name(label, name) && expected_id.is_none_or(|want| id.as_deref() == Some(want))
        })
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("I couldn't find an accessible control named {name}.")),
        _ => Err(format!(
            "More than one control is named {name}; please be more specific."
        )),
    }
}

/// How a control is operated, and so which state proves the press: the macOS counterparts of the UI Automation
/// Toggle, SelectionItem and Invoke patterns, plus focus for text fields (Windows clicks into them at their clickable
/// point; macOS gives them focus through the API instead, so the pointer never moves).
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PressKind {
    /// AXPress flips `AXValue` (checkbox, switch, disclosure triangle).
    Toggle,
    /// AXPress makes `AXValue` 1 (radio button, tab).
    Select,
    /// Setting `AXFocused` makes it read back true (text field, text area, combo box).
    Focus,
    /// AXPress with no state to read back (button, link, menu item, pop-up).
    Activate,
}

#[cfg(any(target_os = "macos", test))]
fn press_kind(role: &str) -> PressKind {
    match role {
        "AXCheckBox" | "AXDisclosureTriangle" => PressKind::Toggle,
        "AXRadioButton" => PressKind::Select,
        "AXTextField" | "AXTextArea" | "AXComboBox" => PressKind::Focus,
        _ => PressKind::Activate,
    }
}

/// Verified only when the control's own state shows the press took effect AND the same window is still in front, as
/// the Windows toggle/selection checks. A plain press proves dispatch, not the app's effect (like UIA Invoke), so it is
/// never Verified.
#[cfg(any(target_os = "macos", test))]
fn outcome_after(kind: PressKind, before: Option<i64>, after: Option<i64>, still_in_front: bool) -> Outcome {
    let effect = match kind {
        PressKind::Toggle => before.is_some() && after.is_some() && before != after,
        PressKind::Select | PressKind::Focus => after == Some(1),
        PressKind::Activate => false,
    };
    if effect && still_in_front {
        Outcome::Verified
    } else {
        Outcome::Unconfirmed
    }
}

#[cfg(any(target_os = "macos", test))]
fn detail_for(kind: PressKind, name: &str) -> String {
    match kind {
        PressKind::Toggle => format!("Toggled {name}."),
        PressKind::Select => format!("Selected {name}."),
        PressKind::Focus => format!("Focused {name}."),
        PressKind::Activate => {
            format!("Activated {name}, but the app did not expose a verifiable result.")
        }
    }
}

/// What the AXError of a press says about delivery.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    /// The app performed the press and answered.
    Done,
    /// The request went out but the app did not answer within the messaging timeout. This is what a button that opens
    /// a modal dialog returns (the app answers only after the dialog closes), so the press may well have happened: the
    /// receipt is Unconfirmed, never Blocked.
    NoReply,
}

#[cfg(any(target_os = "macos", test))]
fn press_delivery(err: i32, name: &str) -> Result<Delivery, String> {
    match err {
        0 => Ok(Delivery::Done),
        AX_ERR_CANNOT_COMPLETE => Ok(Delivery::NoReply),
        AX_ERR_ACTION_UNSUPPORTED => Err(format!("{name} has no accessible press action.")),
        AX_ERR_INVALID_ELEMENT => Err(format!("{name} disappeared before I could press it.")),
        AX_ERR_API_DISABLED => Err(MAC_PERMISSION_DETAIL.into()),
        other => Err(format!("The app refused the press (accessibility error {other}).")),
    }
}

/// The macOS authentication agents (admin password and Touch ID sheets, the login window). They are the counterpart
/// of the Windows secure desktop / elevated windows that Ghost refuses: nothing is pressed in them by voice.
#[cfg(any(target_os = "macos", test))]
fn is_system_prompt(app_key: &str) -> bool {
    let key = app_key.to_lowercase();
    ["securityagent", "coreauth", "localauthentication", "loginwindow"]
        .iter()
        .any(|p| key.contains(p))
}

/// Why the window in front must not be driven, if it must not: the same protected-window rule the keyboard center uses
/// (`shortcuts::exec::is_protected_window`, matched on the bundle id and the window title), then the system prompts.
#[cfg(any(target_os = "macos", test))]
fn refusal(app_key: &str, title: &str) -> Option<&'static str> {
    if crate::shortcuts::exec::is_protected_window(app_key, title) {
        Some("That window is protected.")
    } else if is_system_prompt(app_key) {
        Some("That is a macOS security prompt, so I won't press anything in it.")
    } else {
        None
    }
}

/// Typing guard: a focused element whose role cannot be read counts as secure (as Windows treats an unreadable
/// password flag), and so does a secure text field. A missing subrole is normal for ordinary fields.
#[cfg(any(target_os = "macos", test))]
fn is_secure_focus(role: Option<&str>, subrole: &str) -> bool {
    match role {
        None => true,
        Some(r) if r.trim().is_empty() => true,
        Some(_) => subrole == "AXSecureTextField",
    }
}

/// macOS side of the Accessibility Center: the Accessibility API (AXUIElement) in place of UI Automation, with the
/// same refusals and receipts as `windows_accessibility`. One deliberate difference: the control is operated through
/// the API (`AXPress`, or `AXFocused` for a text field), so the pointer never moves. There is no cursor fallback and
/// so no hit test: a press that is not a mouse event cannot land on a window that covers the control.
#[cfg(target_os = "macos")]
mod mac_accessibility {
    use super::{
        choose_target, detail_for, is_secure_focus, outcome_after, press_delivery, press_kind, refusal,
        same_name, Delivery, Outcome, PressKind, Route, MAC_PERMISSION_DETAIL,
    };
    use crate::ax_mac::mac::{self, Owned};
    use crate::ax_mac::{kind_for_role, pick_name, Rect};
    use crate::macos_foreground::FrontApp;
    use core_foundation_sys::base::{CFEqual, CFGetTypeID, CFTypeRef};
    use core_foundation_sys::number::{
        kCFBooleanTrue, kCFNumberSInt64Type, CFNumberGetTypeID, CFNumberGetValue, CFNumberRef,
    };
    use core_foundation_sys::string::CFStringRef;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    /// Upper bound on elements read per search (as `ax_mac`'s list): a long web page can hold tens of thousands.
    const MAX_NODES: usize = 6000;
    /// Per-call limit for the focus query (the system default of 6 s would stall a keyboard shortcut on a hung app).
    const FOCUS_TIMEOUT_S: f32 = 0.5;

    pub fn act(
        name: &str,
        expected_id: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<(Route, Outcome, String), String> {
        let keep_going = || {
            crate::ghost::session_active()
                && !crate::ghost::stop_requested()
                && !cancel.load(Ordering::Relaxed)
        };
        act_guarded(name, expected_id, &keep_going)
    }

    /// `act` with the session/stop/cancel guard passed in, so the live test can drive it without a Ghost session.
    pub(super) fn act_guarded(
        name: &str,
        expected_id: Option<&str>,
        keep_going: &dyn Fn() -> bool,
    ) -> Result<(Route, Outcome, String), String> {
        let (front, app, window) = checked_front()?;
        // Chromium and Electron build their web content tree only when asked; the first ask needs a moment.
        let (front, app, window) = if ask_for_web_tree(&app, &front) {
            std::thread::sleep(Duration::from_millis(300));
            checked_front()?
        } else {
            (front, app, window)
        };
        let deadline = Instant::now() + Duration::from_millis(crate::screen::FAST_LOOKUP_BUDGET_MS);
        let found = match find_named(&window, name, deadline, keep_going) {
            Ok(found) => found,
            Err(Walk::Stopped) => return Ok(stopped()),
            Err(Walk::Failed(reason)) => return Err(reason),
        };
        // The identifier is read only for the controls whose name matched: one extra call each, not one per control.
        let labels: Vec<(String, Option<String>)> = found
            .iter()
            .map(|f| (f.name.clone(), expected_id.and_then(|_| identifier(&f.el))))
            .collect();
        let target = &found[choose_target(&labels, name, expected_id)?];
        if !keep_going() {
            return Ok(stopped());
        }
        if !still_in_front(&front, &app, &window) {
            return Err("The window changed before I could act.".into());
        }
        let kind = press_kind(&target.role);
        let before = read_state(&target.el, kind);
        if press_delivery(perform(&target.el, kind), name)? == Delivery::NoReply {
            return Ok((
                Route::AccessibilityCenter,
                Outcome::Unconfirmed,
                format!("I sent the press to {name}, but the app did not answer in time."),
            ));
        }
        let mut after = read_state(&target.el, kind);
        if kind != PressKind::Activate {
            // An app may publish the new state a moment after it answers the press.
            for _ in 0..3 {
                if outcome_after(kind, before, after, true) == Outcome::Verified {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
                after = read_state(&target.el, kind);
            }
        }
        let in_front = kind != PressKind::Activate && still_in_front(&front, &app, &window);
        let outcome = outcome_after(kind, before, after, in_front);
        tracing::info!(
            "[GHOST-AX] {:?} '{}' in {} -> {outcome:?}",
            kind,
            target.name,
            front.key()
        );
        Ok((Route::AccessibilityCenter, outcome, detail_for(kind, name)))
    }

    fn stopped() -> (Route, Outcome, String) {
        (Route::AccessibilityCenter, Outcome::Stopped, "The task was stopped.".into())
    }

    /// The app in front, its element and its window, after the permission, protected-window and system-prompt
    /// refusals. NEXUS itself in front is refused by `front_window`.
    fn checked_front() -> Result<(FrontApp, Owned, Owned), String> {
        if !mac::trusted() {
            return Err(MAC_PERMISSION_DETAIL.into());
        }
        let (front, app, window) = mac::front_window().map_err(|why| {
            tracing::warn!("[GHOST-AX] no front window: {why}");
            "I can't identify the foreground window.".to_string()
        })?;
        let title = mac::copy_attr(&window, "AXTitle")
            // SAFETY: `t` holds the value for the call; `string_of` checks the type first.
            .map(|t| unsafe { mac::string_of(t.0) })
            .unwrap_or_default();
        if let Some(reason) = refusal(&front.key(), &title) {
            return Err(reason.into());
        }
        Ok((front, app, window))
    }

    /// Same app (pid) in front and the same window focused: the macOS form of "the foreground HWND is unchanged".
    fn still_in_front(front: &FrontApp, app: &Owned, window: &Owned) -> bool {
        if crate::macos_foreground::front_app().is_none_or(|now| now.pid != front.pid) {
            return false;
        }
        mac::copy_attr(app, "AXFocusedWindow")
            .or_else(|| mac::copy_attr(app, "AXMainWindow"))
            // SAFETY: both are live AXUIElements; CFEqual compares the UI elements they refer to.
            .is_some_and(|now| unsafe { CFEqual(now.0, window.0) != 0 })
    }

    /// Pids already asked for their web tree (the request is per process; the tree builds asynchronously). `ax_mac`
    /// keeps its own list for "click X"; at worst each side asks once.
    /// Shared with the "click X" list (`ax_mac`), so each app is asked for its web tree once per run.
    fn ask_for_web_tree(app: &Owned, front: &FrontApp) -> bool {
        mac::ask_for_web_tree(app, front.pid as i32, front.bundle_id.as_deref().unwrap_or(""))
    }

    /// One eligible control whose name matched.
    struct Found {
        el: Owned,
        role: String,
        name: String,
    }

    enum Walk {
        Stopped,
        Failed(String),
    }

    /// Every enabled, visible actionable control in `window` named `name`, read with the same rules as the "click X"
    /// list (`ax_mac`): role → kind, secure fields never resolve, scrolled-away subtrees are skipped, a control's own
    /// children are not separate targets. Unlike the list, a walk cut short by the budget or the element cap fails:
    /// a partial walk cannot prove that the match is the only one.
    fn find_named(
        window: &Owned,
        name: &str,
        deadline: Instant,
        keep_going: &dyn Fn() -> bool,
    ) -> Result<Vec<Found>, Walk> {
        let attrs = mac::names(mac::NODE_ATTRS);
        let text_attrs = mac::names(&["AXValue", "AXTitle"]);
        let win = mac::read_node(window, &attrs)
            .ok_or_else(|| Walk::Failed("The window in front did not answer.".into()))?;
        let view = win
            .frame
            .ok_or_else(|| Walk::Failed("The window in front has no frame.".into()))?;
        let mut stack: Vec<(Owned, Rect, u16)> =
            win.children.into_iter().rev().map(|c| (c, view, 1)).collect();
        let mut found = Vec::new();
        let mut visited = 0usize;
        while let Some((el, clip, depth)) = stack.pop() {
            if visited >= MAX_NODES || Instant::now() >= deadline {
                tracing::warn!("[GHOST-AX] search stopped after {visited} elements; refusing to guess");
                return Err(Walk::Failed(
                    "The window is too big to search completely, so I won't guess.".into(),
                ));
            }
            visited += 1;
            if visited % 64 == 0 && !keep_going() {
                return Err(Walk::Stopped);
            }
            let Some(node) = mac::read_node(&el, &attrs) else { continue };
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
            if let (Some(_), Some(_)) = (kind_for_role(&node.role, &node.subrole), visible) {
                if node.enabled {
                    let mut label = pick_name(&[&node.title, &node.description, &node.placeholder]);
                    if label.is_empty() {
                        label = pick_name(&[&mac::inner_text(&node.children, &text_attrs), &node.help]);
                    }
                    if same_name(&label, name) {
                        found.push(Found { el, role: node.role, name: label });
                    }
                }
                continue;
            }
            if depth < 64 {
                stack.extend(node.children.into_iter().rev().map(|c| (c, inner_clip, depth + 1)));
            }
        }
        tracing::debug!("[GHOST-AX] {} match(es) for '{name}' in {visited} elements", found.len());
        Ok(found)
    }

    fn identifier(el: &Owned) -> Option<String> {
        let v = mac::copy_attr(el, "AXIdentifier")?;
        // SAFETY: `v` holds the value for the call; `string_of` checks the type first.
        let id = unsafe { mac::string_of(v.0) };
        (!id.is_empty()).then_some(id)
    }

    /// The state that proves the press for `kind`: `AXValue` (0/1/2, number or boolean) or `AXFocused` as 0/1.
    fn read_state(el: &Owned, kind: PressKind) -> Option<i64> {
        let attr = match kind {
            PressKind::Toggle | PressKind::Select => "AXValue",
            PressKind::Focus => "AXFocused",
            PressKind::Activate => return None,
        };
        let v = mac::copy_attr(el, attr)?;
        // SAFETY: `v` holds a live CF object for the call; each reader checks the type before reading it.
        unsafe { number_of(v.0).or_else(|| mac::bool_of(v.0).map(i64::from)) }
    }

    /// SAFETY: `v` is null or a live CF object.
    unsafe fn number_of(v: CFTypeRef) -> Option<i64> {
        if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: i64 = 0;
        CFNumberGetValue(v as CFNumberRef, kCFNumberSInt64Type, &mut out as *mut i64 as *mut c_void).then_some(out)
    }

    /// Operate the control once. Returns the raw AXError (0 = done).
    fn perform(el: &Owned, kind: PressKind) -> i32 {
        if kind == PressKind::Focus {
            let attr = mac::cf_string("AXFocused");
            // SAFETY: `el` is a live AXUIElement, `attr` a live CFString, and kCFBooleanTrue is immortal.
            unsafe { mac::AXUIElementSetAttributeValue(el.0, attr.0 as CFStringRef, kCFBooleanTrue as CFTypeRef) }
        } else {
            let action = mac::cf_string("AXPress");
            // SAFETY: `el` is a live AXUIElement and `action` a live CFString; the call only sends a request.
            unsafe { mac::AXUIElementPerformAction(el.0, action.0 as CFStringRef) }
        }
    }

    /// True when typing must be refused: no permission, no readable focused element, or a secure text field in focus.
    pub(super) fn focused_field_is_secure() -> bool {
        if !mac::trusted() {
            return true;
        }
        // SAFETY: plain constructor; returns a +1 reference (or null, handled by `Owned::new`).
        let Some(system) = Owned::new(unsafe { mac::AXUIElementCreateSystemWide() }) else {
            return true;
        };
        // SAFETY: `system` is a live AXUIElement; on the system-wide element the timeout applies to every call.
        unsafe { mac::AXUIElementSetMessagingTimeout(system.0, FOCUS_TIMEOUT_S) };
        let Some(focused) = mac::copy_attr(&system, "AXFocusedUIElement") else {
            return true;
        };
        // SAFETY (both reads): the value is held for the call; `string_of` checks the type first.
        let role = mac::copy_attr(&focused, "AXRole").map(|r| unsafe { mac::string_of(r.0) });
        let subrole = mac::copy_attr(&focused, "AXSubrole")
            .map(|s| unsafe { mac::string_of(s.0) })
            .unwrap_or_default();
        is_secure_focus(role.as_deref(), &subrole)
    }
}

#[cfg(target_os = "windows")]
mod windows_accessibility {
    use super::{Outcome, Route};
    use std::sync::atomic::{AtomicBool, Ordering};
    use uiautomation::controls::ControlType;
    use uiautomation::patterns::{UIInvokePattern, UISelectionItemPattern, UITogglePattern};
    use uiautomation::types::{Handle, PropertyConditionFlags, TreeScope, UIProperty};
    use uiautomation::variants::Variant;
    use uiautomation::UIAutomation;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    pub fn act(
        name: &str,
        expected_id: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<(Route, Outcome, String), String> {
        let automation = UIAutomation::new().map_err(|e| e.to_string())?;
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0 == 0 {
            return Err("I can't identify the foreground window.".into());
        }
        let exe = crate::browser_url::foreground_process_name()
            .ok_or_else(|| "I can't identify the foreground app.".to_string())?;
        let title = crate::browser_url::foreground_window_title().unwrap_or_default();
        if crate::shortcuts::exec::is_protected_window(&exe, &title) {
            return Err("That window is protected.".into());
        }
        if crate::live::commands::window::window_process_elevated(hwnd)
            && !crate::live::commands::window::our_process_elevated()
        {
            return Err("That app runs as administrator.".into());
        }
        let root = automation
            .element_from_handle(Handle::from(hwnd.0))
            .map_err(|e| e.to_string())?;
        let cond = automation
            .create_property_condition(
                UIProperty::Name,
                Variant::from(name.to_string()),
                Some(PropertyConditionFlags::IgnoreCase),
            )
            .map_err(|e| e.to_string())?;
        let matches = root
            .find_all(TreeScope::Descendants, &cond)
            .map_err(|e| e.to_string())?;
        let eligible = matches
            .into_iter()
            .filter(|el| {
                el.get_name().is_ok_and(|n| n.eq_ignore_ascii_case(name))
                    && expected_id
                        .is_none_or(|id| el.get_automation_id().is_ok_and(|current| current == id))
                    && el.get_control_type().is_ok_and(|kind| {
                        matches!(
                            kind,
                            ControlType::Button
                                | ControlType::Hyperlink
                                | ControlType::TabItem
                                | ControlType::MenuItem
                                | ControlType::ListItem
                                | ControlType::CheckBox
                                | ControlType::RadioButton
                                | ControlType::SplitButton
                                | ControlType::ComboBox
                                | ControlType::Edit
                        )
                    })
                    && el.is_enabled().unwrap_or(false)
                    && !el.is_offscreen().unwrap_or(true)
                    && !el.is_password().unwrap_or(true)
            })
            .collect::<Vec<_>>();
        if eligible.len() != 1 {
            return Err(if eligible.is_empty() {
                format!("I couldn't find an accessible control named {name}.")
            } else {
                format!("More than one control is named {name}; please be more specific.")
            });
        }
        let el = &eligible[0];
        let same_window = || unsafe { GetForegroundWindow() }.0 == hwnd.0
            && crate::ghost::session_active() && !crate::ghost::stop_requested()
            && !cancel.load(Ordering::Relaxed);
        if !same_window() {
            return Err("The window changed before I could act.".into());
        }
        if let Ok(toggle) = el.get_pattern::<UITogglePattern>() {
            let before = toggle.get_toggle_state().map_err(|e| e.to_string())?;
            toggle.toggle().map_err(|e| e.to_string())?;
            let after = toggle.get_toggle_state().ok();
            return Ok((
                Route::AccessibilityCenter,
                if after.is_some_and(|state| state != before) {
                    Outcome::Verified
                } else {
                    Outcome::Unconfirmed
                },
                format!("Toggled {name}."),
            ));
        }
        if let Ok(selection) = el.get_pattern::<UISelectionItemPattern>() {
            selection.select().map_err(|e| e.to_string())?;
            return Ok((
                Route::AccessibilityCenter,
                if selection.is_selected().unwrap_or(false) {
                    Outcome::Verified
                } else {
                    Outcome::Unconfirmed
                },
                format!("Selected {name}."),
            ));
        }
        if let Ok(invoke) = el.get_pattern::<UIInvokePattern>() {
            invoke.invoke().map_err(|e| e.to_string())?;
            // Invoke returning success proves dispatch, not the app's effect.
            return Ok((
                Route::AccessibilityCenter,
                Outcome::Unconfirmed,
                format!("Activated {name}, but the app did not expose a verifiable result."),
            ));
        }
        let point = el
            .get_clickable_point()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{name} has no safe clickable point."))?;
        let x = point.get_x();
        let y = point.get_y();
        let intended_id = el.get_runtime_id().map_err(|e| e.to_string())?;
        let checked = || {
            if !same_window() {
                return Err("The foreground window changed.".into());
            }
            let hit = automation
                .element_from_point(point)
                .map_err(|e| e.to_string())?;
            if hit.get_runtime_id().map_err(|e| e.to_string())? != intended_id {
                return Err("Another control covers the target point.".into());
            }
            Ok(())
        };
        checked()?;
        crate::live::commands::mouse::click_at_checked(x, y, || !same_window(), checked)?;
        Ok((
            Route::CursorCenter,
            Outcome::Unconfirmed,
            format!("Clicked {name}, but the app did not expose a verifiable result."),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcuts_and_ordered_steps() {
        assert_eq!(
            parse("Nexus save then open a new tab"),
            Some(GhostTask {
                steps: vec![
                    GhostStep::Shortcut {
                        action: "save".into(),
                        arg: None
                    },
                    GhostStep::Shortcut {
                        action: "new_tab".into(),
                        arg: None
                    },
                ]
            })
        );
        assert_eq!(
            parse("switch to tab 3").unwrap().steps[0],
            GhostStep::Shortcut {
                action: "goto_tab_n".into(),
                arg: Some(3)
            }
        );
        assert_eq!(
            parse("Nexus, can you please click the Settings button")
                .unwrap()
                .steps[0],
            GhostStep::AccessibleClick {
                name: "settings".into()
            }
        );
    }

    #[test]
    fn parser_abstains_on_ambiguous_or_risky_text() {
        for text in [
            "I wish this was saved",
            "click the blue one",
            "click the blue button",
            "click Send",
            "click Delete",
            "save then maybe buy it",
            "tab 10",
        ] {
            assert!(parse(text).is_none(), "{text}");
        }
        assert!(!validate(&GhostTask {
            steps: vec![GhostStep::AccessibleClick {
                name: "Blue button".into()
            }],
        }));
    }

    #[test]
    fn learned_controls_use_explicit_alias_commands() {
        assert_eq!(
            parse("remember this button as settings").unwrap().steps[0],
            GhostStep::Learn {
                alias: "settings".into()
            }
        );
        assert_eq!(
            parse("click learned settings").unwrap().steps[0],
            GhostStep::LearnedClick {
                alias: "settings".into()
            }
        );
        assert_eq!(
            parse("forget learned settings").unwrap().steps[0],
            GhostStep::Forget {
                alias: "settings".into()
            }
        );
        assert!(parse("remember this button as send").is_none());
    }

    #[test]
    fn receipt_never_claims_unverified_success() {
        let r = Receipt {
            step: 0,
            route: Route::KeyboardCenter,
            outcome: Outcome::Unconfirmed,
            detail: "Save shortcut was sent.".into(),
            elapsed_ms: 1,
        };
        assert!(r.spoken().contains("couldn't verify"));
        assert!(!r.spoken().starts_with("Done"));
    }

    fn controls(list: &[(&str, Option<&str>)]) -> Vec<(String, Option<String>)> {
        list.iter()
            .map(|(n, id)| (n.to_string(), id.map(str::to_string)))
            .collect()
    }

    #[test]
    fn mac_target_is_the_one_control_with_that_whole_name() {
        let found = controls(&[("Settings", None), ("Settings menu", None), ("  Learn\n more ", None)]);
        assert_eq!(choose_target(&found, "settings", None), Ok(0), "case does not matter, whole name only");
        assert_eq!(choose_target(&found, "learn more", None), Ok(2), "whitespace runs collapse");
        assert_eq!(
            choose_target(&found, "help", None),
            Err("I couldn't find an accessible control named help.".into())
        );
        assert_eq!(
            choose_target(&controls(&[]), "help", None),
            Err("I couldn't find an accessible control named help.".into())
        );
    }

    #[test]
    fn mac_two_controls_with_one_name_are_refused_unless_the_identifier_tells_them_apart() {
        let found = controls(&[("Save", Some("saveDraft")), ("save", Some("saveFinal")), ("Save", None)]);
        assert_eq!(
            choose_target(&found, "Save", None),
            Err("More than one control is named Save; please be more specific.".into())
        );
        assert_eq!(choose_target(&found, "Save", Some("saveFinal")), Ok(1));
        assert!(
            choose_target(&found, "Save", Some("saveOld")).is_err(),
            "a learned identifier that is gone never falls back to the name alone"
        );
        assert!(
            choose_target(&controls(&[("Open", Some("x"))]), "Save", Some("x")).is_err(),
            "the identifier never overrides the name"
        );
    }

    #[test]
    fn mac_press_kind_follows_the_role() {
        assert_eq!(press_kind("AXCheckBox"), PressKind::Toggle);
        assert_eq!(press_kind("AXDisclosureTriangle"), PressKind::Toggle);
        assert_eq!(press_kind("AXRadioButton"), PressKind::Select, "tabs are radio buttons too");
        assert_eq!(press_kind("AXTextField"), PressKind::Focus);
        assert_eq!(press_kind("AXComboBox"), PressKind::Focus);
        for role in ["AXButton", "AXLink", "AXMenuItem", "AXPopUpButton", "AXMenuButton"] {
            assert_eq!(press_kind(role), PressKind::Activate, "{role}");
        }
    }

    #[test]
    fn mac_verified_needs_the_state_change_and_the_same_window_in_front() {
        use PressKind::*;
        assert_eq!(outcome_after(Toggle, Some(0), Some(1), true), Outcome::Verified);
        assert_eq!(outcome_after(Toggle, Some(2), Some(1), true), Outcome::Verified, "mixed state counts");
        assert_eq!(outcome_after(Toggle, Some(1), Some(1), true), Outcome::Unconfirmed, "no change");
        assert_eq!(outcome_after(Toggle, None, Some(1), true), Outcome::Unconfirmed, "unknown before");
        assert_eq!(outcome_after(Toggle, Some(0), None, true), Outcome::Unconfirmed, "unknown after");
        assert_eq!(outcome_after(Toggle, Some(0), Some(1), false), Outcome::Unconfirmed, "window changed");
        assert_eq!(outcome_after(Select, Some(0), Some(1), true), Outcome::Verified);
        assert_eq!(outcome_after(Select, Some(1), Some(1), true), Outcome::Verified, "already selected");
        assert_eq!(outcome_after(Select, Some(0), Some(0), true), Outcome::Unconfirmed);
        assert_eq!(outcome_after(Focus, Some(0), Some(1), true), Outcome::Verified);
        assert_eq!(outcome_after(Focus, Some(0), Some(1), false), Outcome::Unconfirmed);
        assert_eq!(
            outcome_after(Activate, None, None, true),
            Outcome::Unconfirmed,
            "a plain press proves dispatch only, like UIA Invoke"
        );
        assert!(detail_for(Activate, "Add").contains("did not expose a verifiable result"));
        assert_eq!(detail_for(Toggle, "Wi-Fi"), "Toggled Wi-Fi.");
    }

    #[test]
    fn mac_press_errors_map_to_honest_receipts() {
        assert_eq!(press_delivery(0, "Add"), Ok(Delivery::Done));
        assert_eq!(
            press_delivery(AX_ERR_CANNOT_COMPLETE, "Add"),
            Ok(Delivery::NoReply),
            "a press that opened a modal answers late: unconfirmed, not refused"
        );
        assert_eq!(
            press_delivery(AX_ERR_ACTION_UNSUPPORTED, "Add"),
            Err("Add has no accessible press action.".into())
        );
        assert!(press_delivery(AX_ERR_INVALID_ELEMENT, "Add").unwrap_err().contains("disappeared"));
        assert_eq!(press_delivery(AX_ERR_API_DISABLED, "Add"), Err(MAC_PERMISSION_DETAIL.into()));
        assert!(press_delivery(-25200, "Add").unwrap_err().contains("-25200"));
    }

    #[test]
    fn mac_protected_apps_titles_and_system_prompts_are_refused() {
        assert_eq!(refusal("com.1password.1password", ""), Some("That window is protected."));
        assert_eq!(refusal("com.apple.keychainaccess", "Keychain Access"), Some("That window is protected."));
        assert_eq!(refusal("com.apple.passwords", ""), Some("That window is protected."));
        assert_eq!(
            refusal("com.google.chrome", "Bitwarden Web Vault"),
            Some("That window is protected."),
            "a protected page in an ordinary browser is caught by its title"
        );
        assert!(refusal("com.apple.securityagent", "").is_some_and(|r| r.contains("security prompt")));
        assert!(refusal("com.apple.loginwindow", "").is_some());
        assert!(refusal("coreautha", "").is_some(), "Touch ID sheet without a bundle id");
        assert_eq!(refusal("com.apple.calculator", "Calculator"), None);
        assert_eq!(refusal("com.google.chrome", "Ledger of expenses - Google Sheets"), None);
    }

    #[test]
    fn mac_typing_is_refused_in_secure_or_unreadable_focus() {
        assert!(is_secure_focus(Some("AXTextField"), "AXSecureTextField"));
        assert!(is_secure_focus(None, ""), "unreadable focus counts as secure");
        assert!(is_secure_focus(Some(""), ""), "a role that reads empty is unreadable");
        assert!(!is_secure_focus(Some("AXTextField"), ""));
        assert!(!is_secure_focus(Some("AXTextArea"), ""), "no subrole is normal");
        assert!(!is_secure_focus(Some("AXTextField"), "AXSearchField"));
    }

    /// Live, macOS: presses Calculator's buttons by name through the Accessibility Center (no Ghost session: the guard
    /// is passed in), then reads the display back. Also checks the pointer never moved. Opens Calculator (quits it
    /// again if it was not running). Run: `cargo test --lib ghost_center -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "drives Calculator: needs the Accessibility permission and a desktop; moves focus to Calculator"]
    fn mac_presses_calculator_buttons_by_name_without_moving_the_pointer() {
        use crate::ax_mac::mac;
        use std::process::Command;
        use std::time::{Duration, Instant};
        assert!(crate::ax_mac::trusted(), "grant Accessibility to the terminal first");
        let was_running = Command::new("pgrep")
            .args(["-x", "Calculator"])
            .status()
            .is_ok_and(|s| s.success());
        let pointer = crate::live::commands::mouse::current_pos();
        Command::new("open").args(["-a", "Calculator"]).status().expect("open Calculator");
        let t0 = Instant::now();
        while crate::macos_foreground::front_app().is_none_or(|f| f.name != "Calculator") {
            assert!(t0.elapsed() < Duration::from_secs(10), "Calculator never came to the front");
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(400));
        let press = |n: &str| mac_accessibility::act_guarded(n, None, &|| true);
        // "Clear" exists only while there is an entry; after it the key reads "All Clear", which also drops a pending
        // operation. Either may be missing, so their refusals are ignored.
        for reset in ["Clear", "All Clear"] {
            let _ = press(reset);
            std::thread::sleep(Duration::from_millis(150));
        }
        let mut receipts = Vec::new();
        for key in ["7", "Add", "3", "Equals"] {
            receipts.push((key, press(key)));
            std::thread::sleep(Duration::from_millis(150));
        }
        std::thread::sleep(Duration::from_millis(300));
        let display = {
            let (_, _, window) = mac::front_window().expect("Calculator window");
            let attrs = mac::names(mac::NODE_ATTRS);
            let mut texts = Vec::new();
            let mut stack = vec![window];
            while let Some(el) = stack.pop() {
                let Some(node) = mac::read_node(&el, &attrs) else { continue };
                if node.role == "AXStaticText" {
                    if let Some(v) = mac::copy_attr(&el, "AXValue") {
                        // SAFETY: `v` holds the value for the call; `string_of` checks the type.
                        texts.push(unsafe { mac::string_of(v.0) });
                    }
                }
                stack.extend(node.children);
            }
            texts
        };
        let pointer_after = crate::live::commands::mouse::current_pos();
        if !was_running {
            let _ = Command::new("pkill").args(["-x", "Calculator"]).status();
        }
        eprintln!("receipts: {receipts:?}\ndisplay: {display:?}");
        for (key, receipt) in &receipts {
            let (route, outcome, detail) = receipt.as_ref().unwrap_or_else(|e| panic!("press {key}: {e}"));
            assert_eq!(*route, Route::AccessibilityCenter, "{key}");
            assert_eq!(*outcome, Outcome::Unconfirmed, "{key}: a plain button proves dispatch only ({detail})");
        }
        assert_eq!(pointer, pointer_after, "an accessibility press never moves the pointer");
        assert!(display.iter().any(|t| t.trim() == "10"), "7 + 3 should show 10, got {display:?}");
    }
}
