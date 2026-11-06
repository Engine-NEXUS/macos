//! Safety layer for live mode — whitelist, denylist, and confirmation gates.
//!
//! Inspired by:
//!   - AnovaX two-stage safety filter (prompt rules + code whitelist/denylist)
//!   - OpenDex permission model (sensitive/optIn flags per skill)
//!   - SynapseKit SafetyPolicy (confirm_before, forbidden_apps)
//!   - Anthropic Computer Use guidelines (confirm destructive actions)
//!
//! Every live-mode action passes through `safety_check()` before execution.
//! Actions that match the denylist are refused. Actions in the
//! `confirm_before` list require explicit user confirmation.

/// Tools that are allowed in live mode.
pub const ALLOWED_TOOLS: &[&str] = &[
    "type_text",
    "press_key",
    "press_hotkey",
    "open_app",
    "close_app",
    "focus_app",
    "open_url",
    "open_search",
    "whatsapp_open",
    "whatsapp_search",
    "whatsapp_send",
    "browser_new_tab",
    "browser_navigate",
    "browser_search",
    "confirm_send",
    "cancel_action",
    "mouse_move",
    "mouse_click",
    "ghost_calibrate",
];

/// Targets that are never allowed (banking, password managers, etc.).
/// Matched case-insensitively against the target string.
pub const DENIED_TARGETS: &[&str] = &[
    "1password",
    "bitwarden",
    "keychain",
    "lastpass",
    "keepass",
    "dashlane",
    "bank",
    "paypal",
    "venmo",
    "cashapp",
    "stripe",
    "coinbase",
    "metamask",
    "trezor",
    "ledger",
    // macOS bundle ids that do not contain one of the names above (targets are matched as substrings).
    "onepassword",
    "agilebits",
    "com.apple.passwords",
];

/// Actions that always require user confirmation before execution.
pub const CONFIRM_ACTIONS: &[&str] = &[
    "whatsapp_send",   // sending a message is irreversible
    "confirm_send",     // explicit send confirmation
    "close_app",       // closing an app may lose work
];

/// Check if a tool is allowed.
pub fn is_tool_allowed(tool: &str) -> bool {
    let t = tool.trim().to_lowercase();
    ALLOWED_TOOLS.iter().any(|&a| a == t.as_str())
}

/// Check if a target (app name, URL, etc.) is blocked.
pub fn is_target_blocked(target: &str) -> bool {
    let lower = target.to_lowercase();
    DENIED_TARGETS.iter().any(|d| lower.contains(d))
}

/// Check if an action requires user confirmation.
pub fn needs_confirmation(tool: &str) -> bool {
    let t = tool.trim().to_lowercase();
    CONFIRM_ACTIONS.iter().any(|&c| c == t.as_str())
}

/// Full safety check for a (tool, target) pair.
/// Returns Ok(()) if the action is safe to execute,
/// Err(message) if it should be blocked.
/// Returns Ok(ConfirmRequired) if the action needs user confirmation.
pub fn safety_check(tool: &str, target: Option<&str>) -> SafetyVerdict {
    if !is_tool_allowed(tool) {
        return SafetyVerdict::Blocked(format!("Tool '{tool}' is not in the allowed list"));
    }

    if let Some(t) = target {
        if is_target_blocked(t) {
            return SafetyVerdict::Blocked(format!(
                "Target '{t}' is blocked for your safety, sir"
            ));
        }
    }

    if needs_confirmation(tool) {
        return SafetyVerdict::NeedsConfirmation;
    }

    SafetyVerdict::Allowed
}

/// The verdict from a safety check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyVerdict {
    /// The action is safe to execute immediately.
    Allowed,
    /// The action is safe but requires user confirmation first.
    NeedsConfirmation,
    /// The action is blocked and must not execute.
    Blocked(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allowed_tools() {
        assert!(is_tool_allowed("type_text"));
        assert!(is_tool_allowed("whatsapp_send"));
        assert!(is_tool_allowed("open_app"));
        assert!(!is_tool_allowed("delete_file"));
        assert!(!is_tool_allowed("rm_rf"));
    }

    #[test]
    fn test_blocked_targets() {
        assert!(is_target_blocked("1Password"));
        assert!(is_target_blocked("Bitwarden"));
        assert!(is_target_blocked("my bank account"));
        assert!(is_target_blocked("PayPal"));
        assert!(!is_target_blocked("whatsapp"));
        assert!(!is_target_blocked("brave"));
        assert!(!is_target_blocked("notepad"));
    }

    #[test]
    fn test_confirmation_required() {
        assert!(needs_confirmation("whatsapp_send"));
        assert!(needs_confirmation("confirm_send"));
        assert!(!needs_confirmation("type_text"));
        assert!(!needs_confirmation("open_app"));
    }

    #[test]
    fn test_safety_check_allowed() {
        assert_eq!(safety_check("type_text", None), SafetyVerdict::Allowed);
        assert_eq!(
            safety_check("open_app", Some("notepad")),
            SafetyVerdict::Allowed
        );
    }

    #[test]
    fn test_safety_check_blocked() {
        assert_eq!(
            safety_check("delete_file", None),
            SafetyVerdict::Blocked("Tool 'delete_file' is not in the allowed list".to_string())
        );
        assert_eq!(
            safety_check("open_app", Some("1Password")),
            SafetyVerdict::Blocked(
                "Target '1Password' is blocked for your safety, sir".to_string()
            )
        );
    }

    #[test]
    fn test_safety_check_needs_confirmation() {
        assert_eq!(
            safety_check("whatsapp_send", None),
            SafetyVerdict::NeedsConfirmation
        );
        assert_eq!(
            safety_check("confirm_send", None),
            SafetyVerdict::NeedsConfirmation
        );
    }

    #[test]
    fn test_ghost_tools_allowed_but_gated() {
        // Mouse + calibrate tools exist in the allowlist (refusal comes
        // from targets/confirm, never from a missing tool entry).
        assert!(is_tool_allowed("mouse_move"));
        assert!(is_tool_allowed("mouse_click"));
        assert!(is_tool_allowed("ghost_calibrate"));
        assert_eq!(
            safety_check("mouse_click", Some("1Password")),
            SafetyVerdict::Blocked(
                "Target '1Password' is blocked for your safety, sir".to_string()
            )
        );
        assert_eq!(
            safety_check("ghost_calibrate", None),
            SafetyVerdict::Allowed
        );
    }

    #[test]
    fn test_all_allowed_tools_whitelisted() {
        for &tool in ALLOWED_TOOLS {
            assert!(
                is_tool_allowed(tool),
                "Allowed tool list must accept '{tool}'"
            );
            // Without a blocked target, each should either be Allowed or NeedsConfirmation, never Blocked
            let verdict = safety_check(tool, None);
            assert!(
                matches!(verdict, SafetyVerdict::Allowed | SafetyVerdict::NeedsConfirmation),
                "Tool '{tool}' had unexpected verdict: {verdict:?}"
            );
        }
    }

    #[test]
    fn test_unknown_and_dangerous_tools_blocked() {
        let dangerous = [
            "delete_file",
            "rm_rf",
            "format_c",
            "shell_exec",
            "powershell",
            "bash",
            "cmd",
            "eval",
            "write_file",
            "kill_process",
            "",
            "   ",
        ];
        for tool in dangerous {
            assert!(
                !is_tool_allowed(tool),
                "Tool '{tool}' must not be allowed"
            );
            assert_eq!(
                safety_check(tool, None),
                SafetyVerdict::Blocked(format!("Tool '{tool}' is not in the allowed list"))
            );
        }
    }

    #[test]
    fn test_tool_casing_and_whitespace_normalisation() {
        assert!(is_tool_allowed("  type_text  "));
        assert!(is_tool_allowed("TYPE_TEXT"));
        assert!(is_tool_allowed("WhatsApp_Open"));
        assert!(is_tool_allowed(" browser_search\t"));
        assert!(!is_tool_allowed("unknown_command"));
    }

    #[test]
    fn test_denied_targets_all_classes_blocked() {
        // Password managers
        let pwd_managers = [
            "1password.exe",
            "Bitwarden Desktop",
            "LastPass Vault",
            "keepassxc.exe",
            "Dashlane",
            "Apple Keychain",
        ];
        for target in pwd_managers {
            assert!(is_target_blocked(target), "Password manager target '{target}' must be blocked");
        }

        // Banking & payment sites
        let banking = [
            "Chase Bank Online",
            "https://bankofamerica.com",
            "PayPal Balance",
            "Venmo Payment",
            "CashApp Transfer",
            "Stripe Dashboard",
        ];
        for target in banking {
            assert!(is_target_blocked(target), "Banking target '{target}' must be blocked");
        }

        // Crypto & hardware wallets
        let crypto = [
            "Coinbase Pro",
            "MetaMask Extension",
            "Trezor Suite",
            "Ledger Live",
        ];
        for target in crypto {
            assert!(is_target_blocked(target), "Crypto target '{target}' must be blocked");
        }
    }

    #[test]
    fn test_denied_targets_casing_and_punctuation_variations() {
        assert!(is_target_blocked("1PASSWORD"));
        assert!(is_target_blocked("BiTwArDeN"));
        assert!(is_target_blocked("   paypal   "));
        assert!(is_target_blocked("MY_BANK_ACCOUNT"));
        assert!(is_target_blocked("metamask.io"));
        assert!(is_target_blocked("KeePass-2.54.exe"));
    }

    #[test]
    fn test_harmless_targets_not_blocked() {
        let safe = [
            "whatsapp",
            "brave",
            "notepad",
            "Visual Studio Code",
            "Spotify Free",
            "Calculator",
            "Google Chrome",
            "Slack",
            "Terminal",
            "River Park",
            "Central Park",
        ];
        for target in safe {
            assert!(!is_target_blocked(target), "Harmless target '{target}' should not be blocked");
            assert_eq!(
                safety_check("open_app", Some(target)),
                SafetyVerdict::Allowed
            );
        }
    }

    #[test]
    fn test_substring_matching_rules_and_ambiguity() {
        // As defined by the substring check in DENIED_TARGETS:
        // "bank" is a denylist entry, so "Bank of the River Park" contains "bank" and is blocked.
        assert!(
            is_target_blocked("Bank of the River Park"),
            "Substring matching blocks 'Bank of the River Park' due to 'bank'"
        );
        // By contrast, the park without the word 'bank' is not blocked.
        assert!(
            !is_target_blocked("River Park"),
            "'River Park' has no denylist keywords and must be allowed"
        );
    }

    #[test]
    fn test_google_accounts_not_blocked_by_default() {
        // The current DENIED_TARGETS list does not claim generic Google login pages.
        // Assert current behavior as defined in the rules.
        assert!(
            !is_target_blocked("Sign in - Google Accounts"),
            "Sign in - Google Accounts is not currently in DENIED_TARGETS"
        );
    }

    #[test]
    fn test_confirmation_required_exact_and_variations() {
        for &tool in CONFIRM_ACTIONS {
            assert!(
                needs_confirmation(tool),
                "Action '{tool}' must require confirmation"
            );
        }
        assert!(!needs_confirmation("type_text"));
        assert!(!needs_confirmation("open_app"));
        assert!(!needs_confirmation("open_url"));
    }

    #[test]
    fn test_confirmation_cannot_be_bypassed_by_spaces_or_casing() {
        // Extra spaces or case variations must still trigger confirmation, never Allowed
        let variations = [
            "whatsapp_send",
            " whatsapp_send ",
            "WhatsApp_Send",
            "WHATSAPP_SEND",
            "confirm_send",
            " confirm_send ",
            "Confirm_Send",
            "CONFIRM_SEND",
            "close_app",
            " Close_App ",
            "CLOSE_APP",
        ];
        for tool in variations {
            assert!(
                needs_confirmation(tool),
                "Confirmation check failed for variation '{tool}'"
            );
            assert_eq!(
                safety_check(tool, None),
                SafetyVerdict::NeedsConfirmation,
                "safety_check must demand confirmation for '{tool}', never skip it"
            );
        }
    }

    #[test]
    fn test_safety_check_matrix_combinations() {
        // 1. Allowed tool + Safe target -> Allowed
        assert_eq!(
            safety_check("open_app", Some("notepad")),
            SafetyVerdict::Allowed
        );

        // 2. Allowed tool + Blocked target -> Blocked
        assert_eq!(
            safety_check("open_app", Some("1Password")),
            SafetyVerdict::Blocked("Target '1Password' is blocked for your safety, sir".to_string())
        );

        // 3. Confirm tool + Safe target -> NeedsConfirmation
        assert_eq!(
            safety_check("whatsapp_send", Some("friend")),
            SafetyVerdict::NeedsConfirmation
        );

        // 4. Confirm tool + Blocked target -> Blocked wins over confirmation
        assert_eq!(
            safety_check("whatsapp_send", Some("my bank")),
            SafetyVerdict::Blocked("Target 'my bank' is blocked for your safety, sir".to_string())
        );

        // 5. Unknown tool + Blocked target -> Unknown tool blocked wins
        assert_eq!(
            safety_check("steal_passwords", Some("1Password")),
            SafetyVerdict::Blocked("Tool 'steal_passwords' is not in the allowed list".to_string())
        );
    }
}
