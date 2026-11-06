//! macOS privacy permissions NEXUS's screen features depend on (Accessibility, Screen Recording), and the plain spoken
//! line for each one that is missing.
//!
//! The checks never prompt: `screen_recording_allowed` and `accessibility_trusted` are plain queries, safe to call on
//! every request. Only `request_screen_recording` can show a macOS prompt, and it does so at most once per run. macOS
//! applies a Screen Recording grant made in System Settings only after NEXUS is restarted (Settings offers "Quit &
//! Reopen"), so a check that failed stays failed for the rest of the run.

use std::sync::atomic::{AtomicBool, Ordering};

/// Spoken when Accessibility is missing (click / read controls, Ghost).
#[allow(dead_code)]
pub(crate) const ACCESSIBILITY_LINE: &str =
    "I need the Accessibility permission for that, sir. It is in System Settings, Privacy and Security, Accessibility.";

/// Spoken when Screen Recording is missing (screen analysis, OCR, screen tour).
#[allow(dead_code)]
pub(crate) const SCREEN_RECORDING_LINE: &str =
    "I need the Screen Recording permission to see your screen, sir. It is in System Settings, Privacy and Security, Screen and System Audio Recording.";

// Screen Recording access checks, CoreGraphics (macOS 10.15+). Both return a C `bool`, which Rust's `bool` matches.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Set once `CGRequestScreenCaptureAccess` has run, so a missing permission raises the macOS prompt once per run, not
/// on every screen command.
static SCREEN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// True when this process may read other apps' accessibility trees and post input events.
#[allow(dead_code)]
pub(crate) fn accessibility_trusted() -> bool {
    crate::ax_mac::trusted()
}

/// True when this process may capture other apps' windows. Without it `screencapture` can raise the macOS prompt again
/// or return only the wallpaper and NEXUS's own windows, so capture checks this first. Never prompts.
pub(crate) fn screen_recording_allowed() -> bool {
    // SAFETY: plain query, no arguments, no side effects (does not prompt).
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Ask macOS to show its Screen Recording prompt (at most once per run). Returns `screen_recording_allowed()`.
///
/// macOS itself shows the prompt only the first time an app asks; after a denial it adds the app to the Screen
/// Recording list switched off, and the user has to turn it on there (see `open_privacy_pane`).
#[allow(dead_code)]
pub(crate) fn request_screen_recording() -> bool {
    if screen_recording_allowed() {
        return true;
    }
    if !SCREEN_REQUESTED.swap(true, Ordering::SeqCst) {
        // SAFETY: plain call, no arguments. It may show a system prompt; it does not wait for the user's answer.
        unsafe { CGRequestScreenCaptureAccess() };
    }
    screen_recording_allowed()
}

/// A System Settings pane a spoken permission line points the user to.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivacyPane {
    /// Privacy & Security → Screen & System Audio Recording.
    ScreenRecording,
    /// Privacy & Security → Accessibility.
    Accessibility,
}

/// The `x-apple.systempreferences:` URL that opens `pane`. System Settings maps the legacy `com.apple.preference.security`
/// id to its Privacy & Security extension (`legacyBundleIdentifier` in that extension's Info.plist, which also allows the
/// URL scheme); the `Privacy_*` anchors are the ones its binary and search terms list on macOS 27.2.
#[allow(dead_code)]
fn privacy_pane_url(pane: PrivacyPane) -> &'static str {
    match pane {
        PrivacyPane::ScreenRecording => "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
        PrivacyPane::Accessibility => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    }
}

/// Open System Settings at `pane`. Runs `open` on its own thread so the caller (a voice handler) never waits on it;
/// a failure is logged, not returned, because the spoken line already names the place.
#[allow(dead_code)]
pub(crate) fn open_privacy_pane(pane: PrivacyPane) {
    let url = privacy_pane_url(pane);
    std::thread::spawn(move || match std::process::Command::new("open").arg(url).status() {
        Ok(s) if s.success() => {}
        Ok(s) => tracing::warn!("mac_permissions: open {url} exited with {s}"),
        Err(e) => tracing::warn!("mac_permissions: open {url} failed: {e}"),
    });
}

#[cfg(test)]
mod tests {
    use super::{privacy_pane_url, screen_recording_allowed, PrivacyPane};

    #[test]
    fn pane_urls_point_at_the_privacy_anchors() {
        assert_eq!(
            privacy_pane_url(PrivacyPane::ScreenRecording),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        );
        assert_eq!(
            privacy_pane_url(PrivacyPane::Accessibility),
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        );
    }

    #[test]
    fn spoken_lines_name_the_settings_place() {
        assert!(super::SCREEN_RECORDING_LINE.contains("Screen and System Audio Recording"));
        assert!(super::ACCESSIBILITY_LINE.contains("Privacy and Security, Accessibility"));
    }

    /// The preflight never prompts, so it is safe in a test. Prints the answer for this process (the terminal's grant).
    #[test]
    fn screen_recording_preflight_returns() {
        let allowed = screen_recording_allowed();
        println!("screen recording allowed for this process: {allowed}");
    }
}
