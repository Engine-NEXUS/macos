//! macOS privacy permissions NEXUS's screen features depend on (Accessibility, Screen Recording), and the plain spoken
//! line for each one that is missing.
//!
//! STUB contract (being filled in): signatures are fixed, bodies are placeholders.

/// Spoken when Accessibility is missing (click / read controls, Ghost).
#[allow(dead_code)]
pub(crate) const ACCESSIBILITY_LINE: &str =
    "I need the Accessibility permission for that, sir. It is in System Settings, Privacy and Security, Accessibility.";

/// Spoken when Screen Recording is missing (screen analysis, OCR, screen tour).
#[allow(dead_code)]
pub(crate) const SCREEN_RECORDING_LINE: &str =
    "I need the Screen Recording permission to see your screen, sir. It is in System Settings, Privacy and Security, Screen and System Audio Recording.";

/// True when this process may read other apps' accessibility trees and post input events.
#[allow(dead_code)]
pub(crate) fn accessibility_trusted() -> bool {
    crate::ax_mac::trusted()
}

/// True when this process may capture other apps' windows.
#[allow(dead_code)]
pub(crate) fn screen_recording_allowed() -> bool {
    true
}

/// Ask macOS to show its Screen Recording prompt (at most once per run). Returns `screen_recording_allowed()`.
#[allow(dead_code)]
pub(crate) fn request_screen_recording() -> bool {
    screen_recording_allowed()
}
