//! Windows adapter for the Action Router (shortcut brain, phase E1): real foreground probe, batched
//! `SendInput`, ghost stop flag. Everything decision-like lives in `router.rs`; this file is deliberately
//! thin so the tested logic is the logic that ships.

use super::keys::{key_events, Chord, KeyEvent};
use super::map::{active_seed, Action};
use super::router::{self, Env, Foreground, Outcome};

/// Credential managers and wallets: protected even when only the *window title* says so
/// (e.g. a browser tab "Bitwarden Web Vault"). The money terms in `live::safety::DENIED_TARGETS`
/// ("bank", "ledger", …) are far too common in ordinary page titles, so they are matched against the
/// executable name only.
const TITLE_PROTECTED: &[&str] = &[
    "1password", "bitwarden", "keychain", "lastpass", "keepass", "dashlane", "metamask", "trezor",
];

/// Pure (unit-tested): should keystrokes be refused for this window?
pub fn is_protected_window(exe: &str, title: &str) -> bool {
    if crate::live::safety::is_target_blocked(exe) {
        return true;
    }
    let t = title.to_lowercase();
    TITLE_PROTECTED.iter().any(|p| t.contains(p))
}

/// Kill switch: `NEXUS_SHORTCUTS_LEGACY=1` makes every call site use its previous implementation.
pub fn enabled() -> bool {
    !std::env::var("NEXUS_SHORTCUTS_LEGACY").map(|v| v == "1").unwrap_or(false)
}

pub struct WinEnv;

impl Env for WinEnv {
    fn foreground(&self) -> Option<Foreground> {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            let hwnd = unsafe { GetForegroundWindow() };
            if hwnd.0 == 0 {
                return None;
            }
            let exe = crate::browser_url::foreground_process_name()?;
            let title = crate::browser_url::foreground_window_title().unwrap_or_default();
            let elevated = crate::live::commands::window::window_process_elevated(hwnd);
            Some(Foreground { exe, title, hwnd: hwnd.0, elevated })
        }
        #[cfg(target_os = "macos")]
        {
            // Bundle id is the map key (what seed_macos.json lists); the pid doubles as the window identity
            // so the router's "same window before sending" check still works. A Mac app cannot be "elevated"
            // in the Windows sense.
            let app = crate::macos_foreground::front_app()?;
            let title = crate::macos_foreground::window_title(app.pid).unwrap_or_default();
            Some(Foreground { exe: app.key(), title, hwnd: app.pid as isize, elevated: false })
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            None
        }
    }

    fn send(&self, chord: &Chord) -> Result<(), String> {
        send_chord(chord)
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn now_ms(&self) -> u64 {
        static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
    }

    fn stop_requested(&self) -> bool {
        // Only meaningful inside a ghost session; outside one the flag is irrelevant.
        crate::ghost::session_active() && crate::ghost::stop_requested()
    }

    fn self_elevated(&self) -> bool {
        crate::live::commands::window::our_process_elevated()
    }

    fn is_protected(&self, exe: &str, title: &str) -> bool {
        is_protected_window(exe, title)
    }
}

/// One chord = ONE `SendInput` call (modifiers down, key down/up, modifiers up), by virtual key, so it is
/// atomic with respect to other input and independent of the keyboard layout / IME.
#[cfg(target_os = "windows")]
fn send_chord(chord: &Chord) -> Result<(), String> {
    send_events(&key_events(chord))
}

/// Raw key events in one `SendInput` batch (used by hand gestures to hold Alt for the window switcher).
#[cfg(target_os = "windows")]
pub fn send_events(events: &[KeyEvent]) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    let inputs: Vec<INPUT> = events
        .iter()
        .map(|e| {
            let mut flags = 0u32;
            if e.extended {
                flags |= KEYEVENTF_EXTENDEDKEY.0;
            }
            if e.up {
                flags |= KEYEVENTF_KEYUP.0;
            }
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(e.vk),
                        wScan: 0,
                        dwFlags: KEYBD_EVENT_FLAGS(flags),
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        })
        .collect();
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(format!(
            "SendInput delivered {sent}/{} events (blocked by UIPI or another desktop?)",
            inputs.len()
        ))
    }
}

/// One chord on macOS = the same down/up sequence sent through enigo's `Keyboard::raw`,
/// which posts CGEvents with the platform's virtual key codes (Accessibility
/// permission required; without it the OS silently drops the keys, so callers'
/// verification steps still catch a failed press).
#[cfg(target_os = "macos")]
fn send_chord(chord: &Chord) -> Result<(), String> {
    send_events(&key_events(chord))
}

/// CG virtual key codes, per `Events.h` (`kVK_ANSI_*`, `kVK_Tab`, …). Windows
/// sends VK codes, so `send_events` translates: the accent keys and F-keys sit
/// in different ranges, and Up/Down swap when read as a Windows-style arrow
/// block, which is exactly the asymmetry the table bakes in.
#[cfg(target_os = "macos")]
fn vk_to_mac(vk: u16) -> Option<u16> {
    Some(match vk {
        // Whitespace/control keys
        0x08 => 51,   // Backspace      kVK_Delete
        0x09 => 48,   // Tab            kVK_Tab
        0x0D => 36,   // Enter          kVK_Return
        0x1B => 53,   // Esc            kVK_Escape
        0x20 => 49,   // Space          kVK_Space
        // Editing keys
        0x2D => 114,  // Insert         kVK_Help
        0x2E => 117,  // Delete         kVK_ForwardDelete
        0x21 => 116,  // PageUp         kVK_PageUp
        0x22 => 121,  // PageDown       kVK_PageDown
        0x23 => 119,  // End            kVK_End
        0x24 => 115,  // Home           kVK_Home
        // Arrow block
        0x25 => 123,  // Left           kVK_LeftArrow
        0x26 => 126,  // Up             kVK_UpArrow
        0x27 => 124,  // Right          kVK_RightArrow
        0x28 => 125,  // Down           kVK_DownArrow
        // Modifiers (Windows sends the generic VK; mac needs the concrete key)
        0x10 => 56,   // Shift          kVK_Shift
        0x11 => 59,   // Control        kVK_Control
        0x12 => 58,   // Alt/Option     kVK_Option
        0x5B => 55,   // Win/Command    kVK_Command
        // OEM punctuation (matched to `keys.rs::parse_chord`)
        0xBA => 41,   // ;              kVK_ANSI_Semicolon
        0xBB => 24,   // =/+            kVK_ANSI_Equal
        0xBD => 27,   // -/_            kVK_ANSI_Minus
        0xBC => 43,   // ,              kVK_ANSI_Comma
        0xBE => 47,   // .              kVK_ANSI_Period
        0xBF => 44,   // /              kVK_ANSI_Slash
        0xC0 => 50,   // `              kVK_ANSI_Grave
        0xDB => 33,   // [              kVK_ANSI_LeftBracket
        0xDC => 42,   // \              kVK_ANSI_Backslash
        0xDD => 30,   // ]              kVK_ANSI_RightBracket
        0xDE => 39,   // '              kVK_ANSI_Quote
        0x6B => 69,   // NumAdd         kVK_ANSI_KeypadPlus
        0x6D => 78,   // NumSubtract    kVK_ANSI_KeypadMinus
        // Function keys F1..F12 (Windows 0x70.. — mac has no contiguous range)
        0x70 => 122, 0x71 => 120, 0x72 => 99, 0x73 => 118,
        0x74 => 96,   // F5
        0x75 => 97, 0x76 => 98, 0x77 => 100, 0x78 => 101, 0x79 => 109,
        0x7A => 103,  // F11
        0x7B => 111,  // F12
        // Letters: each maps independently (no linear offset)
        0x41 => 0,    // A
        0x42 => 11,   // B
        0x43 => 8,    // C
        0x44 => 2,    // D
        0x45 => 14,   // E
        0x46 => 3,    // F
        0x47 => 5,    // G
        0x48 => 4,    // H
        0x49 => 34,   // I
        0x4A => 38,   // J
        0x4B => 40,   // K
        0x4C => 37,   // L
        0x4D => 46,   // M
        0x4E => 45,   // N
        0x4F => 31,   // O
        0x50 => 35,   // P
        0x51 => 12,   // Q
        0x52 => 15,   // R
        0x53 => 1,    // S
        0x54 => 17,   // T
        0x55 => 32,   // U
        0x56 => 9,    // V
        0x57 => 13,   // W
        0x58 => 7,    // X
        0x59 => 16,   // Y
        0x5A => 6,    // Z
        // Digits row
        0x30 => 29, 0x31 => 18, 0x32 => 19, 0x33 => 20, 0x34 => 21,
        0x35 => 23,   // 5
        0x36 => 22, 0x37 => 26, 0x38 => 28,
        0x39 => 25,   // 9
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
pub fn send_events(events: &[KeyEvent]) -> Result<(), String> {
    use enigo::{Direction, Enigo, Keyboard, Settings};
    if events.is_empty() {
        return Ok(());
    }
    let mut e = Enigo::new(&Settings::default()).map_err(|err| err.to_string())?;
    for ev in events {
        let code = vk_to_mac(ev.vk)
            .ok_or_else(|| format!("virtual key 0x{:02X} has no macOS mapping yet", ev.vk))?;
        let dir = if ev.up { Direction::Release } else { Direction::Press };
        // enigo 0.5 removed `Key::Raw`; `Keyboard::raw` posts the CGKeyCode and tracks modifier flags itself.
        e.raw(code, dir).map_err(|err| err.to_string())?;
    }
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn send_chord(_chord: &Chord) -> Result<(), String> {
    Err("keyboard shortcuts are only implemented on Windows and macOS".into())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn send_events(_events: &[KeyEvent]) -> Result<(), String> {
    Err("keyboard shortcuts are only implemented on Windows and macOS".into())
}

/// Perform `action` in the application that is in front right now. Blocking (≤ ~0.8 s when it verifies a
/// title change, a few ms otherwise): call from a blocking context, not from a latency-critical async task.
pub fn do_action(action: Action, arg: Option<u32>) -> Outcome {
    let out = router::run(&WinEnv, active_seed(), action, arg);
    log_outcome(action, arg, &out);
    out
}

fn log_outcome(action: Action, arg: Option<u32>, out: &Outcome) {
    let a = match arg {
        Some(n) => format!("{}({n})", action.id()),
        None => action.id().to_string(),
    };
    match out {
        Outcome::Sent { chord, scope, verified, ms } => tracing::info!(
            "[SHORTCUT] {a} -> {chord} ({}) verified={verified:?} in {ms}ms",
            scope.describe()
        ),
        Outcome::NoBinding { known_app, exe } => {
            tracing::info!("[SHORTCUT] {a}: no binding for {exe} (known_app={known_app})")
        }
        Outcome::BadArg(m) => tracing::info!("[SHORTCUT] {a}: bad argument - {m}"),
        Outcome::Aborted(why) => tracing::warn!("[SHORTCUT] {a}: ABORTED, nothing sent - {why:?}"),
        Outcome::Failed(e) => tracing::warn!("[SHORTCUT] {a}: send failed - {e}"),
    }
}

/// Run an action and turn the outcome into the legacy `Result<spoken line, spoken reason>` shape.
///
/// `legacy_ok_for_unknown_app` preserves the old behaviour for browser hotkeys: if the foreground app is
/// **not in any seeded family** (an unknown browser such as Arc/Zen) and has no binding, the caller should
/// run its previous implementation instead of refusing — returned as `Err(None)`-like via `Fallback`.
pub enum Spoken {
    /// Success line (legacy wording).
    Ok(String),
    /// Plain reason (no honorific) — embed it: `format!("Couldn't …, sir: {reason}")`.
    Refused(String),
    /// Unknown app, no binding: let the caller's legacy path decide.
    Fallback,
}

pub fn do_action_spoken(action: Action, arg: Option<u32>, legacy_ok_for_unknown_app: bool) -> Spoken {
    let out = do_action(action, arg);
    if legacy_ok_for_unknown_app {
        if let Outcome::NoBinding { known_app: false, .. } = out {
            return Spoken::Fallback;
        }
    }
    match router::reason(action, &out) {
        None => Spoken::Ok(router::spoken(action, arg, &out).unwrap_or_default()),
        Some(reason) => Spoken::Refused(reason),
    }
}

/// How a low-level hotkey call was carried out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// Through the router (gated, verified when the action allows).
    Router(router::Verified),
    /// The caller's previous implementation ran (kill switch, or an app the map knows nothing about).
    Legacy,
}

/// For low-level callers that only need "make this happen": run the action through the router, or fall back
/// to `legacy` when the shortcut brain is off or the foreground app is not in any seeded family (an unknown
/// browser, say) — exactly what those callers did before. A refusal (protected window, window changed,
/// known app without that shortcut…) is an `Err` with a plain reason and sends nothing.
pub fn run_or_legacy(
    action: Action,
    arg: Option<u32>,
    legacy: impl FnOnce() -> Result<(), String>,
) -> Result<Sent, String> {
    if !enabled() {
        return legacy().map(|_| Sent::Legacy);
    }
    let out = do_action(action, arg);
    match &out {
        Outcome::Sent { verified, .. } => Ok(Sent::Router(*verified)),
        Outcome::NoBinding { known_app: false, .. } => {
            tracing::info!("[SHORTCUT] {}: unknown app, using the legacy hotkey", action.id());
            legacy().map(|_| Sent::Legacy)
        }
        other => Err(router::reason(action, other).unwrap_or_else(|| "refused".to_string())),
    }
}

/// IPC (dev + the upcoming grammar track): `invoke("shortcut_do", { action: "refresh" })` or
/// `{ action: "goto_tab_n", arg: 3 }`. Returns the spoken line on success, a spoken reason on refusal.
/// Runs on a blocking thread (verification may wait up to ~0.8 s).
#[tauri::command]
pub async fn shortcut_do(action: String, arg: Option<u32>) -> Result<String, String> {
    let act = Action::from_id(&action).ok_or_else(|| format!("unknown shortcut action '{action}'"))?;
    if !enabled() {
        return Err("shortcut brain is disabled (NEXUS_SHORTCUTS_LEGACY=1)".into());
    }
    let out = tokio::task::spawn_blocking(move || do_action(act, arg))
        .await
        .map_err(|e| e.to_string())?;
    router::spoken(act, arg, &out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_windows_by_exe_and_by_credential_title() {
        // Executable names: the full live::safety denylist applies.
        assert!(is_protected_window("bitwarden.exe", "Vault"));
        assert!(is_protected_window("1password.exe", ""));
        assert!(is_protected_window("coinbase.exe", "Coinbase"));
        // Titles: only credential managers / wallets.
        assert!(is_protected_window("brave.exe", "Bitwarden Web Vault"));
        assert!(is_protected_window("chrome.exe", "MetaMask Notification"));
        // Ordinary pages that merely contain a money word are NOT blocked (Ctrl+W on them is harmless).
        assert!(!is_protected_window("brave.exe", "Memory Bank notes - Brave"));
        assert!(!is_protected_window("chrome.exe", "Ledger of expenses - Google Sheets"));
        assert!(!is_protected_window("notepad.exe", "Untitled - Notepad"));
        // macOS: the exe slot carries the bundle id.
        assert!(is_protected_window("com.1password.1password", ""));
        assert!(is_protected_window("com.agilebits.onepassword7", ""));
        assert!(is_protected_window("com.bitwarden.desktop", ""));
        assert!(is_protected_window("com.apple.keychainaccess", ""));
        assert!(is_protected_window("com.apple.passwords", ""));
        assert!(!is_protected_window("com.apple.textedit", "Untitled"));
        assert!(!is_protected_window("com.google.chrome", "News - Google Chrome"));
    }

    #[test]
    fn kill_switch_reads_the_environment() {
        // Not mutating the process env here (tests run in parallel); just assert the default.
        if std::env::var("NEXUS_SHORTCUTS_LEGACY").is_err() {
            assert!(enabled());
        }
    }

    // macOS key translation: CI-verified on the aarch64 runner (the table is
    // cfg'd out on Windows builds, where CG codes are meaningless).
    /// Every chord the macOS seed can send must be translatable, or the router would pass validation and then
    /// fail at send time with "unmapped key". Runs only on macOS (where `vk_to_mac` exists).
    #[cfg(target_os = "macos")]
    #[test]
    fn every_macos_seed_chord_translates_to_mac_key_codes() {
        use super::super::keys::parse_chord;
        let v: serde_json::Value =
            serde_json::from_str(include_str!("seed_macos.json")).expect("valid json");
        let mut checked = 0;
        for (_scope, table) in v["bindings"].as_object().expect("bindings") {
            for (_action, val) in table.as_object().expect("table") {
                let templates: Vec<&str> = match val {
                    serde_json::Value::String(s) => vec![s.as_str()],
                    serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str()).collect(),
                    _ => vec![],
                };
                for t in templates {
                    let chord = parse_chord(&t.replace("{n}", "1")).expect("chord parses");
                    for ev in key_events(&chord) {
                        assert!(vk_to_mac(ev.vk).is_some(), "'{t}': key 0x{:02X} has no mac code", ev.vk);
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 40, "suspiciously few chords checked: {checked}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn vk_to_mac_is_unique_and_exact_per_keys_rs_output() {
        let used: &[u16] = &[
            0x08, 0x09, 0x0D, 0x1B, 0x20, 0x2D, 0x2E, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
            0x28, 0x10, 0x11, 0x12, 0x5B, 0xBA, 0xBB, 0xBD, 0xBC, 0xBE, 0xBF, 0xC0, 0xDB, 0xDC,
            0xDD, 0xDE, 0x6B, 0x6D, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
            0x7A, 0x7B, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C,
            0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A,
            0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
        ];
        // Every key `keys.rs` can emit must translate.
        for &vk in used {
            assert!(vk_to_mac(vk).is_some(), "unmapped vk 0x{vk:02X}");
        }
        // …to distinct mac codes (two VKs landing on one code would type the wrong key).
        let mut codes: Vec<u16> = used.iter().filter_map(|&vk| vk_to_mac(vk)).collect();
        codes.sort_unstable();
        let n = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), n, "two VKs map to the same CG code");
        // Spot checks against Events.h.
        assert_eq!(vk_to_mac(0x09), Some(48)); // Tab -> kVK_Tab
        assert_eq!(vk_to_mac(0x12), Some(58)); // Alt -> kVK_Option
        assert_eq!(vk_to_mac(0x11), Some(59)); // Ctrl -> kVK_Control
        assert_eq!(vk_to_mac(0x5B), Some(55)); // Win -> kVK_Command
        assert_eq!(vk_to_mac(0x25), Some(123)); // Left
        assert_eq!(vk_to_mac(0x26), Some(126)); // Up (arrow block is NOT linear on mac)
        assert_eq!(vk_to_mac(0x28), Some(125)); // Down
        assert_eq!(vk_to_mac(0x41), Some(0)); // A
        assert_eq!(vk_to_mac(0x5A), Some(6)); // Z
        assert_eq!(vk_to_mac(0x34), Some(21)); // 4
        assert_eq!(vk_to_mac(0x74), Some(96)); // F5
        assert_eq!(vk_to_mac(0xBB), Some(24)); // '='
        assert_eq!(vk_to_mac(0x6B), Some(69)); // NumAdd
    }
}
