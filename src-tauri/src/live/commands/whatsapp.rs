//! WhatsApp full flow — open app, search contact, type message, send.
//!
//! Uses keyboard shortcuts (Ctrl+F → type contact → Enter → type message →
//! Enter) instead of UI Automation element finding. This is more reliable
//! because:
//!   1. Keyboard shortcuts are stable across WhatsApp versions
//!   2. No dependency on specific UI element names/classes
//!   3. Works even if WhatsApp changes their UI layout
//!
//! The send action ALWAYS requires user confirmation (safety.rs).
//!
//! Flow:
//!   1. Open WhatsApp (reuse existing deep link from command_executor)
//!   2. Wait 2s for app to load
//!   3. Ctrl+F → search bar opens
//!   4. Type contact name → Enter → chat opens
//!   5. Type message text
//!   6. [CONFIRMATION REQUIRED] Press Enter to send

use std::thread;
use std::time::Duration;

use super::keyboard;

/// Open WhatsApp and prepare for a chat.
/// Reuses the existing whatsapp_chat() deep link from command_executor.
pub fn open_whatsapp() -> Result<(), String> {
    tracing::info!("live: opening whatsapp");
    // Use the existing deep link — opens WhatsApp Desktop or web
    open::that("whatsapp://").map_err(|e| format!("open whatsapp: {e}"))?;
    thread::sleep(Duration::from_millis(2000));
    Ok(())
}

/// Search for a contact in WhatsApp and open their chat.
/// Assumes WhatsApp is already open and focused.
pub fn search_contact(contact: &str) -> Result<(), String> {
    tracing::info!("live: searching whatsapp contact: {contact}");

    // Ctrl+F opens the search bar in WhatsApp Desktop
    keyboard::press_shortcut(&["ctrl", "f"])?;
    thread::sleep(Duration::from_millis(500));

    // Type the contact name
    keyboard::type_text(contact)?;
    thread::sleep(Duration::from_millis(500));

    // Press Enter to open the first matching chat
    keyboard::press_key("enter")?;
    thread::sleep(Duration::from_millis(500));

    Ok(())
}

/// Type a message into the currently open WhatsApp chat.
/// Assumes a chat is already open (search_contact was called first).
pub fn type_message(message: &str) -> Result<(), String> {
    tracing::info!("live: typing whatsapp message: {} chars", message.len());
    keyboard::type_text(message)?;
    thread::sleep(Duration::from_millis(300));
    Ok(())
}

/// Send the typed message by pressing Enter.
/// This action is IRREVERSIBLE — always requires user confirmation.
pub fn send_message() -> Result<(), String> {
    tracing::info!("live: sending whatsapp message (pressing enter)");
    keyboard::press_key("enter")?;
    Ok(())
}

/// Full WhatsApp send flow: open → search contact → type → send.
/// The `confirm_send` callback is called before the final Enter press.
/// If it returns false, the message is NOT sent.
#[allow(dead_code)]
pub fn full_send_flow(
    contact: &str,
    message: &str,
    confirm_send: impl FnOnce(&str, &str) -> bool,
) -> Result<bool, String> {
    // 1. Open WhatsApp
    open_whatsapp()?;

    // 2. Search for the contact
    search_contact(contact)?;

    // 3. Type the message
    type_message(message)?;

    // 4. Ask for confirmation before sending
    if !confirm_send(contact, message) {
        tracing::info!("live: whatsapp send cancelled by user");
        return Ok(false); // not sent
    }

    // 5. Send
    send_message()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    // These are integration tests that require WhatsApp to be installed.
    // They're not run in CI — only manually for verification.

    // #[test]
    // fn test_open_whatsapp() {
    //     assert!(open_whatsapp().is_ok());
    // }
}
