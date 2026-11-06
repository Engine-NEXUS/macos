//! macOS privacy permissions NEXUS's screen features depend on (Accessibility, Screen Recording), and the plain spoken
//! line for each one that is missing.
//!
//! STUB: being filled in (screen recording preflight/request, spoken guidance).

/// True when this process may read other apps' accessibility trees and post input events.
#[allow(dead_code)]
pub(crate) fn accessibility_trusted() -> bool {
    crate::ax_mac::trusted()
}
