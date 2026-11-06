//! Live mode state machine — tracks context across sequential voice commands.
//!
//! This is NEXUS's unique contribution. No open-source project we studied
//! tracks context across sequential voice commands. They either do single
//! commands (Ari, OpenDex) or full LLM plans (AnovaX). NEXUS's state machine
//! approach is simpler and more predictable.
//!
//! Example flow:
//!   User: "open whatsapp"     → state: AppOpen { app: "whatsapp" }
//!   User: "chat with mummy"   → state: ChatActive { app, contact: "mummy" }
//!   User: "type hi"           → state: TextTyped { app, contact, text: "hi" }
//!   User: "send"              → executes send, state: Idle
//!
//! Auto-resets to Idle after 30s of silence (configurable).

use serde::Serialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The current state of the live-mode conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum LiveState {
    /// No context — ready for a new command sequence.
    Idle,
    /// An app was just opened/focused.
    AppOpen { app: String },
    /// A chat is active (e.g., WhatsApp chat with a specific contact).
    ChatActive { app: String, contact: String },
    /// Text has been typed into a chat but not yet sent.
    TextTyped { app: String, contact: String, text: String },
    /// A browser search was performed.
    BrowserSearch { query: String },
    /// A browser tab is open and ready for navigation.
    BrowserOpen { url: Option<String> },
}

impl LiveState {
    /// Returns a human-readable description of the current state.
    pub fn describe(&self) -> String {
        match self {
            LiveState::Idle => "idle".to_string(),
            LiveState::AppOpen { app } => format!("{app} open"),
            LiveState::ChatActive { app, contact } => {
                format!("{app} chat with {contact}")
            }
            LiveState::TextTyped { app, contact, text } => {
                format!("{app} chat with {contact}, typed: {text}")
            }
            LiveState::BrowserSearch { query } => format!("searched: {query}"),
            LiveState::BrowserOpen { url } => {
                format!("browser open{}", url.as_ref().map(|u| format!(" at {u}")).unwrap_or_default())
            }
        }
    }

    /// Whether the current state implies an app is focused.
    #[allow(dead_code)]
    pub fn app_context(&self) -> Option<&str> {
        match self {
            LiveState::AppOpen { app }
            | LiveState::ChatActive { app, .. }
            | LiveState::TextTyped { app, .. } => Some(app),
            _ => None,
        }
    }

    /// Whether there's unsent text that a "send" command would dispatch.
    #[allow(dead_code)]
    pub fn has_pending_text(&self) -> bool {
        matches!(self, LiveState::TextTyped { .. })
    }
}

/// Thread-safe live-mode context shared across command invocations.
pub struct LiveContext {
    state: Mutex<LiveState>,
    last_action: Mutex<Instant>,
    timeout: Duration,
}

impl LiveContext {
    /// Create a new context with the default 30s timeout.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LiveState::Idle),
            last_action: Mutex::new(Instant::now()),
            timeout: Duration::from_secs(30),
        }
    }

    /// Create a new context with a custom timeout.
    #[cfg(test)]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            state: Mutex::new(LiveState::Idle),
            last_action: Mutex::new(Instant::now()),
            timeout,
        }
    }

    /// Get the current state. Resets to Idle if expired.
    pub fn state(&self) -> LiveState {
        let mut guard = self.state.lock().unwrap();
        if self.is_expired() {
            *guard = LiveState::Idle;
        }
        guard.clone()
    }

    /// Transition to a new state and update the last-action timestamp.
    pub fn transition(&self, new_state: LiveState) {
        let mut guard = self.state.lock().unwrap();
        tracing::info!("live: {} → {}", guard.describe(), new_state.describe());
        *guard = new_state;
        *self.last_action.lock().unwrap() = Instant::now();
    }

    /// Reset to Idle immediately.
    pub fn reset(&self) {
        let mut guard = self.state.lock().unwrap();
        tracing::info!("live: {} → idle (reset)", guard.describe());
        *guard = LiveState::Idle;
        *self.last_action.lock().unwrap() = Instant::now();
    }

    /// Check if the context has expired (no action for `timeout` duration).
    pub fn is_expired(&self) -> bool {
        let last = *self.last_action.lock().unwrap();
        is_expired_at(last, Instant::now(), self.timeout)
    }

    /// Time since the last action.
    pub fn time_since_last_action(&self) -> Duration {
        Instant::now().saturating_duration_since(*self.last_action.lock().unwrap())
    }
}

/// Helper: returns true if `now` has elapsed past `last` by more than the default 30s timeout.
#[cfg(test)]
pub fn expired(last: Instant, now: Instant) -> bool {
    is_expired_at(last, now, Duration::from_secs(30))
}

/// Helper: returns true if `now` has elapsed past `last` by more than `timeout`.
pub fn is_expired_at(last: Instant, now: Instant, timeout: Duration) -> bool {
    now.saturating_duration_since(last) > timeout
}

/// Global live-mode context (lazy-initialized, shared across Tauri commands).
static LIVE_CONTEXT: once_cell::sync::Lazy<Arc<LiveContext>> =
    once_cell::sync::Lazy::new(|| Arc::new(LiveContext::new()));

/// Get the global live-mode context.
pub fn context() -> Arc<LiveContext> {
    LIVE_CONTEXT.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_describe() {
        assert_eq!(LiveState::Idle.describe(), "idle");
        assert_eq!(
            LiveState::AppOpen { app: "whatsapp".into() }.describe(),
            "whatsapp open"
        );
        assert_eq!(
            LiveState::ChatActive { app: "whatsapp".into(), contact: "mummy".into() }.describe(),
            "whatsapp chat with mummy"
        );
    }

    #[test]
    fn test_app_context() {
        assert_eq!(LiveState::Idle.app_context(), None);
        assert_eq!(
            LiveState::AppOpen { app: "brave".into() }.app_context(),
            Some("brave")
        );
        assert_eq!(
            LiveState::ChatActive { app: "whatsapp".into(), contact: "mum".into() }.app_context(),
            Some("whatsapp")
        );
    }

    #[test]
    fn test_has_pending_text() {
        assert!(!LiveState::Idle.has_pending_text());
        assert!(
            LiveState::TextTyped {
                app: "whatsapp".into(),
                contact: "mummy".into(),
                text: "hi".into()
            }
            .has_pending_text()
        );
    }

    #[test]
    fn test_context_transition() {
        let ctx = LiveContext::new();
        assert_eq!(ctx.state(), LiveState::Idle);

        ctx.transition(LiveState::AppOpen { app: "whatsapp".into() });
        assert_eq!(
            ctx.state(),
            LiveState::AppOpen { app: "whatsapp".into() }
        );

        ctx.reset();
        assert_eq!(ctx.state(), LiveState::Idle);
    }

    #[test]
    fn test_state_describe_browser_and_typed_variants() {
        assert_eq!(
            LiveState::BrowserSearch { query: "weather forecast".into() }.describe(),
            "searched: weather forecast"
        );
        assert_eq!(
            LiveState::BrowserOpen { url: None }.describe(),
            "browser open"
        );
        assert_eq!(
            LiveState::BrowserOpen { url: Some("https://example.com".into()) }.describe(),
            "browser open at https://example.com"
        );
        assert_eq!(
            LiveState::TextTyped {
                app: "whatsapp".into(),
                contact: "raj".into(),
                text: "see you soon".into()
            }
            .describe(),
            "whatsapp chat with raj, typed: see you soon"
        );
    }

    #[test]
    fn test_app_context_coverage_all_states() {
        assert_eq!(LiveState::Idle.app_context(), None);
        assert_eq!(LiveState::AppOpen { app: "slack".into() }.app_context(), Some("slack"));
        assert_eq!(
            LiveState::ChatActive { app: "telegram".into(), contact: "bob".into() }.app_context(),
            Some("telegram")
        );
        assert_eq!(
            LiveState::TextTyped {
                app: "whatsapp".into(),
                contact: "alice".into(),
                text: "hey".into()
            }
            .app_context(),
            Some("whatsapp")
        );
        assert_eq!(LiveState::BrowserSearch { query: "rust".into() }.app_context(), None);
        assert_eq!(
            LiveState::BrowserOpen { url: Some("https://rust-lang.org".into()) }.app_context(),
            None
        );
        assert_eq!(LiveState::BrowserOpen { url: None }.app_context(), None);
    }

    #[test]
    fn test_has_pending_text_all_states() {
        assert!(!LiveState::Idle.has_pending_text());
        assert!(!LiveState::AppOpen { app: "notepad".into() }.has_pending_text());
        assert!(!LiveState::ChatActive { app: "whatsapp".into(), contact: "mom".into() }.has_pending_text());
        assert!(LiveState::TextTyped {
            app: "whatsapp".into(),
            contact: "mom".into(),
            text: "on my way".into()
        }.has_pending_text());
        assert!(!LiveState::BrowserSearch { query: "query".into() }.has_pending_text());
        assert!(!LiveState::BrowserOpen { url: None }.has_pending_text());
    }

    #[test]
    fn test_state_sequential_lifecycle_transitions() {
        let ctx = LiveContext::new();
        assert_eq!(ctx.state(), LiveState::Idle);
        assert_eq!(ctx.state().app_context(), None);
        assert!(!ctx.state().has_pending_text());

        // Step 1: User: "open whatsapp"
        ctx.transition(LiveState::AppOpen { app: "whatsapp".into() });
        assert_eq!(ctx.state(), LiveState::AppOpen { app: "whatsapp".into() });
        assert_eq!(ctx.state().app_context(), Some("whatsapp"));
        assert_eq!(ctx.state().describe(), "whatsapp open");
        assert!(!ctx.state().has_pending_text());

        // Step 2: User: "chat with mummy"
        ctx.transition(LiveState::ChatActive { app: "whatsapp".into(), contact: "mummy".into() });
        assert_eq!(
            ctx.state(),
            LiveState::ChatActive { app: "whatsapp".into(), contact: "mummy".into() }
        );
        assert_eq!(ctx.state().app_context(), Some("whatsapp"));
        assert_eq!(ctx.state().describe(), "whatsapp chat with mummy");
        assert!(!ctx.state().has_pending_text());

        // Step 3: User: "type hi"
        ctx.transition(LiveState::TextTyped {
            app: "whatsapp".into(),
            contact: "mummy".into(),
            text: "hi".into(),
        });
        assert_eq!(
            ctx.state(),
            LiveState::TextTyped {
                app: "whatsapp".into(),
                contact: "mummy".into(),
                text: "hi".into(),
            }
        );
        assert_eq!(ctx.state().app_context(), Some("whatsapp"));
        assert!(ctx.state().has_pending_text());

        // Step 4: User: "send" -> resets to Idle
        ctx.reset();
        assert_eq!(ctx.state(), LiveState::Idle);
        assert_eq!(ctx.state().app_context(), None);
        assert!(!ctx.state().has_pending_text());
    }

    #[test]
    fn test_reset_from_every_state_returns_to_idle() {
        let states = vec![
            LiveState::Idle,
            LiveState::AppOpen { app: "terminal".into() },
            LiveState::ChatActive { app: "whatsapp".into(), contact: "alice".into() },
            LiveState::TextTyped {
                app: "whatsapp".into(),
                contact: "alice".into(),
                text: "draft".into(),
            },
            LiveState::BrowserSearch { query: "search terms".into() },
            LiveState::BrowserOpen { url: Some("https://example.com".into()) },
            LiveState::BrowserOpen { url: None },
        ];

        for s in states {
            let ctx = LiveContext::new();
            ctx.transition(s);
            ctx.reset();
            assert_eq!(ctx.state(), LiveState::Idle, "reset must return to Idle from any state");
        }
    }

    #[test]
    fn test_pure_expired_helper_30s_threshold() {
        let now = Instant::now();
        // Exact same instant -> not expired
        assert!(!expired(now, now));
        // Up to 30s -> not expired
        assert!(!expired(now, now + Duration::from_secs(10)));
        assert!(!expired(now, now + Duration::from_secs(29)));
        assert!(!expired(now, now + Duration::from_secs(30)));
        // Strictly over 30s -> expired
        assert!(expired(now, now + Duration::from_secs(30) + Duration::from_millis(1)));
        assert!(expired(now, now + Duration::from_secs(31)));
        assert!(expired(now, now + Duration::from_secs(100)));
        // Clock jitter where now is before last -> saturates at 0, not expired
        assert!(!expired(now + Duration::from_secs(10), now));
    }

    #[test]
    fn test_pure_is_expired_at_custom_durations() {
        let base = Instant::now();
        let timeout_5s = Duration::from_secs(5);
        assert!(!is_expired_at(base, base + Duration::from_secs(4), timeout_5s));
        assert!(!is_expired_at(base, base + Duration::from_secs(5), timeout_5s));
        assert!(is_expired_at(base, base + Duration::from_secs(6), timeout_5s));

        let timeout_100ms = Duration::from_millis(100);
        assert!(!is_expired_at(base, base + Duration::from_millis(50), timeout_100ms));
        assert!(is_expired_at(base, base + Duration::from_millis(101), timeout_100ms));
    }

    #[test]
    fn test_context_with_timeout_auto_reset() {
        let ctx = LiveContext::with_timeout(Duration::from_millis(5));
        ctx.transition(LiveState::AppOpen { app: "whatsapp".into() });
        assert_eq!(ctx.state(), LiveState::AppOpen { app: "whatsapp".into() });

        // Deterministic short sleep (< 1000ms per hard rule 5) to allow 5ms timeout to expire
        std::thread::sleep(Duration::from_millis(20));

        assert!(ctx.is_expired());
        // state() call detects expiry and resets to Idle
        assert_eq!(ctx.state(), LiveState::Idle);
    }

    #[test]
    fn test_context_transition_refreshes_expiry_clock() {
        // No sleeping: a real-time wait is only as reliable as the machine's scheduler (a loaded macOS CI runner
        // overslept a 250 ms wait past a 400 ms timeout). Instead the "last action" is moved into the past directly.
        let ctx = LiveContext::with_timeout(Duration::from_secs(30));
        ctx.transition(LiveState::AppOpen { app: "whatsapp".into() });

        let backdate = |secs: u64| {
            let past = Instant::now().checked_sub(Duration::from_secs(secs));
            *ctx.last_action.lock().unwrap() = past.expect("monotonic clock has run for at least a minute");
        };

        // Control: with no refresh, 40 s idle is past the 30 s timeout.
        backdate(40);
        assert!(ctx.is_expired(), "40s idle must be expired");

        // 25 s idle is not expired yet; a transition then restarts the clock.
        backdate(25);
        assert!(!ctx.is_expired(), "25s < 30s timeout");
        ctx.transition(LiveState::ChatActive { app: "whatsapp".into(), contact: "dad".into() });
        assert!(
            ctx.time_since_last_action() < Duration::from_secs(5),
            "transition must refresh the action clock (was 25s)"
        );
        assert!(!ctx.is_expired(), "action clock was refreshed by transition");
        assert_eq!(
            ctx.state(),
            LiveState::ChatActive { app: "whatsapp".into(), contact: "dad".into() }
        );
    }

    #[test]
    fn test_unexpected_state_transitions_handled() {
        let ctx = LiveContext::new();
        // Jump directly from Idle to TextTyped without AppOpen/ChatActive
        ctx.transition(LiveState::TextTyped {
            app: "unknown".into(),
            contact: "nobody".into(),
            text: "lost text".into(),
        });
        assert_eq!(
            ctx.state(),
            LiveState::TextTyped {
                app: "unknown".into(),
                contact: "nobody".into(),
                text: "lost text".into(),
            }
        );
        // Resetting cleans up immediately
        ctx.reset();
        assert_eq!(ctx.state(), LiveState::Idle);
    }
}
