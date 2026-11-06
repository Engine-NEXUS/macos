//! Keyboard simulation — type text, press keys, press hotkey combos.
//!
//! Uses `enigo` (cross-platform) for input simulation and `arboard` for
//! clipboard-paste (the OpenDex pattern that avoids autocomplete corruption).
//!
//! Key mapping inspired by OpenDex's `keyFromToken()` in computer/skill.ts.
//! Clipboard-paste pattern inspired by OpenDex's `pasteText()`.
//!
//! Why clipboard paste > character-by-character typing:
//!   Typing "hi what are u doing" into WhatsApp character by character can
//!   trigger autocomplete suggestions that corrupt the text (e.g., "u" →
//!   "you" or "doing" → "doing?"). Clipboard paste is instant and bypasses
//!   autocomplete entirely. We restore the previous clipboard contents
//!   after pasting.

use enigo::{
    Direction::{Click, Press, Release},
    Enigo, Key, Keyboard, Settings,
};
use std::thread;
use std::time::Duration;

/// Type text into the currently focused input field.
///
/// For short text (< 50 chars), uses enigo's `text()` which simulates
/// individual key presses. For longer text, uses clipboard paste
/// (Ctrl+V) to avoid autocomplete corruption and improve speed.
pub fn type_text(text: &str) -> Result<(), String> {
    tracing::info!("live: typing {} chars", text.len());
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("enigo init: {e}"))?;

    if text.len() > 50 {
        // Use clipboard paste for longer text (OpenDex pattern)
        paste_text(text)?;
    } else {
        enigo
            .text(text)
            .map_err(|e| format!("type_text failed: {e}"))?;
    }

    Ok(())
}

/// Paste text via clipboard (Ctrl+V / Cmd+V on macOS).
/// Restores the previous clipboard contents after pasting.
fn paste_text(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("clipboard init: {e}"))?;

    // Save previous clipboard contents
    let prev = clipboard.get_text().unwrap_or_default();

    // Set new text
    clipboard
        .set_text(text.to_string())
        .map_err(|e| format!("clipboard set: {e}"))?;

    // Small delay to ensure clipboard is ready
    thread::sleep(Duration::from_millis(50));

    // Press Ctrl+V (or Cmd+V on macOS)
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("enigo init: {e}"))?;

    let modifier = if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    };

    enigo
        .key(modifier, Press)
        .map_err(|e| format!("press modifier: {e}"))?;
    enigo
        .key(Key::Unicode('v'), Click)
        .map_err(|e| format!("press V: {e}"))?;
    enigo
        .key(modifier, Release)
        .map_err(|e| format!("release modifier: {e}"))?;

    // Wait for paste to complete
    thread::sleep(Duration::from_millis(120));

    // Restore previous clipboard
    let _ = clipboard.set_text(prev);

    tracing::info!("live: pasted {} chars via clipboard", text.len());
    Ok(())
}

/// Press a single key (e.g., "enter", "escape", "tab").
pub fn press_key(key: &str) -> Result<(), String> {
    tracing::info!("live: pressing key: {key}");
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("enigo init: {e}"))?;

    let k = parse_key(key).ok_or_else(|| format!("unknown key: {key}"))?;
    enigo
        .key(k, Click)
        .map_err(|e| format!("press_key failed: {e}"))?;

    Ok(())
}

/// Press a key combination (e.g., Ctrl+A, Ctrl+Shift+Tab).
/// The last key in the slice is the "main" key; all others are modifiers
/// that are held down during the press.
pub fn press_hotkey(keys: &[&str]) -> Result<(), String> {
    if keys.is_empty() {
        return Err("press_hotkey: no keys provided".to_string());
    }

    tracing::info!("live: pressing hotkey: {:?}", keys);

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("enigo init: {e}"))?;

    // Parse all keys
    let parsed: Vec<Key> = keys
        .iter()
        .map(|k| parse_key(k))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("unknown key in hotkey: {:?}", keys))?;

    // Press all modifier keys (all except the last)
    for k in &parsed[..parsed.len() - 1] {
        enigo
            .key(k.clone(), Press)
            .map_err(|e| format!("press modifier: {e}"))?;
    }

    // Click the last key
    enigo
        .key(parsed[parsed.len() - 1].clone(), Click)
        .map_err(|e| format!("click main key: {e}"))?;

    // Release all modifiers in reverse order
    for k in parsed[..parsed.len() - 1].iter().rev() {
        enigo
            .key(k.clone(), Release)
            .map_err(|e| format!("release modifier: {e}"))?;
    }

    Ok(())
}

/// The "primary modifier" spelled Windows-style (`ctrl`) becomes Command on a Mac: Ctrl+C / Ctrl+T / Ctrl+L are
/// Cmd+C / Cmd+T / Cmd+L there. Pure so it is testable on any OS; with `macos == false` the keys come back unchanged.
/// Only for NEXUS's own shortcuts; a user who says "press control c" gets exactly what they said (`press_hotkey`).
fn platform_keys(keys: &[&str], macos: bool) -> Vec<String> {
    keys.iter()
        .map(|k| match k.to_ascii_lowercase().as_str() {
            "ctrl" | "control" if macos => "cmd".to_string(),
            _ => (*k).to_string(),
        })
        .collect()
}

/// A shortcut NEXUS itself needs (new tab, address bar, find, undo…), written with `ctrl` and sent as the
/// platform's primary modifier. On Windows and Linux this is exactly `press_hotkey`.
pub fn press_shortcut(keys: &[&str]) -> Result<(), String> {
    let mapped = platform_keys(keys, cfg!(target_os = "macos"));
    let refs: Vec<&str> = mapped.iter().map(String::as_str).collect();
    press_hotkey(&refs)
}

/// Map a string key name to an enigo Key.
/// Inspired by OpenDex's `keyFromToken()` in computer/skill.ts.
fn parse_key(token: &str) -> Option<Key> {
    let t = token.trim().to_lowercase();

    // Special keys
    let special: &[(&str, Key)] = &[
        ("enter", Key::Return),
        ("return", Key::Return),
        ("tab", Key::Tab),
        ("escape", Key::Escape),
        ("esc", Key::Escape),
        ("space", Key::Space),
        ("spacebar", Key::Space),
        ("backspace", Key::Backspace),
        ("delete", Key::Delete),
        ("del", Key::Delete),
        ("up", Key::UpArrow),
        ("arrowup", Key::UpArrow),
        ("down", Key::DownArrow),
        ("arrowdown", Key::DownArrow),
        ("left", Key::LeftArrow),
        ("arrowleft", Key::LeftArrow),
        ("right", Key::RightArrow),
        ("arrowright", Key::RightArrow),
        ("home", Key::Home),
        ("end", Key::End),
        ("pageup", Key::PageUp),
        ("pgup", Key::PageUp),
        ("pagedown", Key::PageDown),
        ("pgdn", Key::PageDown),
        ("ctrl", Key::Control),
        ("control", Key::Control),
        ("alt", Key::Alt),
        ("option", Key::Alt),
        ("shift", Key::Shift),
        ("cmd", Key::Meta),
        ("command", Key::Meta),
        ("meta", Key::Meta),
        ("win", Key::Meta),
        ("windows", Key::Meta),
        ("super", Key::Meta),
    ];

    for (name, key) in special {
        if t == *name {
            return Some(key.clone());
        }
    }

    // Single character (letters, digits, symbols)
    if t.len() == 1 {
        if let Some(c) = t.chars().next() {
            return Some(Key::Unicode(c));
        }
    }

    // Function keys F1-F12
    if let Some(num) = t.strip_prefix('f') {
        if let Ok(n) = num.parse::<u8>() {
            if (1..=12).contains(&n) {
                return Some(match n {
                    1 => Key::F1,
                    2 => Key::F2,
                    3 => Key::F3,
                    4 => Key::F4,
                    5 => Key::F5,
                    6 => Key::F6,
                    7 => Key::F7,
                    8 => Key::F8,
                    9 => Key::F9,
                    10 => Key::F10,
                    11 => Key::F11,
                    12 => Key::F12,
                    _ => return None,
                });
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_key_special() {
        assert_eq!(parse_key("enter"), Some(Key::Return));
        assert_eq!(parse_key("Enter"), Some(Key::Return));
        assert_eq!(parse_key("RETURN"), Some(Key::Return));
        assert_eq!(parse_key("esc"), Some(Key::Escape));
        assert_eq!(parse_key("escape"), Some(Key::Escape));
        assert_eq!(parse_key("tab"), Some(Key::Tab));
        assert_eq!(parse_key("space"), Some(Key::Space));
        assert_eq!(parse_key("ctrl"), Some(Key::Control));
        assert_eq!(parse_key("shift"), Some(Key::Shift));
        assert_eq!(parse_key("alt"), Some(Key::Alt));
        assert_eq!(parse_key("cmd"), Some(Key::Meta));
        assert_eq!(parse_key("win"), Some(Key::Meta));
    }

    #[test]
    fn test_parse_key_single_char() {
        assert_eq!(parse_key("a"), Some(Key::Unicode('a')));
        assert_eq!(parse_key("A"), Some(Key::Unicode('a')));
        assert_eq!(parse_key("v"), Some(Key::Unicode('v')));
        assert_eq!(parse_key("1"), Some(Key::Unicode('1')));
    }

    #[test]
    fn test_parse_key_function_keys() {
        assert_eq!(parse_key("f1"), Some(Key::F1));
        assert_eq!(parse_key("f12"), Some(Key::F12));
        // F13-F24 exist in enigo 0.5 but we only map F1-F12
    }

    #[test]
    fn test_parse_key_unknown() {
        assert_eq!(parse_key("unknown"), None);
        assert_eq!(parse_key("xyz"), None);
        assert_eq!(parse_key(""), None);
    }

    #[test]
    fn platform_keys_maps_ctrl_to_cmd_only_on_macos() {
        assert_eq!(platform_keys(&["ctrl", "t"], true), vec!["cmd", "t"]);
        assert_eq!(platform_keys(&["Control", "shift", "t"], true), vec!["cmd", "shift", "t"]);
        // Windows / Linux: untouched, byte for byte.
        assert_eq!(platform_keys(&["ctrl", "t"], false), vec!["ctrl", "t"]);
        assert_eq!(platform_keys(&["ctrl", "shift", "tab"], false), vec!["ctrl", "shift", "tab"]);
        // Everything that is not the primary modifier is left alone on a Mac too.
        assert_eq!(platform_keys(&["alt", "left"], true), vec!["alt", "left"]);
        assert_eq!(platform_keys(&["escape"], true), vec!["escape"]);
    }
}
