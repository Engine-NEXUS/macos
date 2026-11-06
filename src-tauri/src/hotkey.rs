//! Global hotkeys and action dispatch.
//!
//! Hotkeys (Windows chord first, macOS chord second; the macOS ones use the physical Control key, see below):
//!   - `CommandOrControl+Space` / `Control+Space`: Symmetric agent call / cancel.
//!     If the agent is active (capturing audio, speaking TTS, processing a
//!     request, or running a ghost session), cancels the active agent and
//!     hides the orb. If the agent is idle, calls the agent (plays the wake
//!     cue, starts STT capture, and wakes the orb). Windows (settings, sidebar,
//!     setup) are closed by Escape, never by this hotkey.
//!   - `CommandOrControl+Shift+S` / `Control+Shift+S`: Opens the Command Hub settings sidebar directly.
//!   - `CommandOrControl+Alt+X` / `Control+Alt+X`: Stage kill switch: destroys and disables the
//!     fullscreen overlay for the session as an escape hatch even if the renderer
//!     is wedged.
//!
//! macOS: `CommandOrControl` means Cmd there, and Cmd+Space is Spotlight (Cmd+Shift+S is Save As in many apps), so the
//! macOS chords use the literal Control key, the same physical key as on Windows. Control+Space is also macOS's default
//! "select the previous input source" shortcut, which only matters if more than one keyboard layout is enabled.
//!
//! NOTE: The global-shortcut plugin is not available on Linux.
//! This entire module is compiled only on Windows and macOS.

#![cfg(not(target_os = "linux"))]

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// What a hotkey does. The chord text differs per OS; the role does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Call the agent when idle, cancel it when busy.
    Call,
    /// Open the Command Hub settings sidebar.
    Settings,
    /// Destroy and disable the fullscreen stage (escape hatch for a wedged renderer).
    KillStage,
}

/// Windows chords. `CommandOrControl` resolves to Ctrl on Windows. Unchanged from before the macOS port.
const HOTKEYS_WINDOWS: &[(&str, Role)] = &[
    ("CommandOrControl+Space", Role::Call),
    ("CommandOrControl+Shift+S", Role::Settings),
    ("CommandOrControl+Alt+X", Role::KillStage),
];

/// macOS chords: the literal Control key, never Cmd (Cmd+Space is Spotlight).
const HOTKEYS_MACOS: &[(&str, Role)] = &[
    ("Control+Space", Role::Call),
    ("Control+Shift+S", Role::Settings),
    ("Control+Alt+X", Role::KillStage),
];

/// The chords registered on this OS. Both tables are referenced on every OS so neither is dead code.
fn active_hotkeys() -> &'static [(&'static str, Role)] {
    if cfg!(target_os = "macos") {
        HOTKEYS_MACOS
    } else {
        HOTKEYS_WINDOWS
    }
}

/// The role of a chord from either table. An unknown chord counts as `Call`, as it always has.
fn role_of(hotkey: &str) -> Role {
    HOTKEYS_WINDOWS
        .iter()
        .chain(HOTKEYS_MACOS.iter())
        .find(|(chord, _)| *chord == hotkey)
        .map(|(_, role)| *role)
        .unwrap_or(Role::Call)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AgentState {
    pub capturing: bool,
    pub speaking: bool,
    pub has_request: bool,
    pub ghost: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Press {
    OpenSettings,
    KillStage,
    Cancel { abort_ghost: bool },
    Call,
}

/// Pure hotkey action decision based on hotkey chord and live agent activity.
pub(crate) fn decide(hotkey: &str, state: AgentState) -> Press {
    match role_of(hotkey) {
        Role::Settings => Press::OpenSettings,
        Role::KillStage => Press::KillStage,
        Role::Call => {
            if state.capturing || state.speaking || state.has_request || state.ghost {
                Press::Cancel {
                    abort_ghost: state.ghost,
                }
            } else {
                Press::Call
            }
        }
    }
}

pub fn init<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    for &(hk, _) in active_hotkeys() {
        let sc: Shortcut = match hk.parse() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to parse hotkey '{hk}': {e}");
                continue;
            }
        };

        let handle = app.clone();
        if let Err(e) = app.global_shortcut().on_shortcut(sc, move |_app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                let state = AgentState {
                    capturing: crate::wakeword_oww::stt_capturing(),
                    speaking: handle
                        .try_state::<std::sync::Arc<crate::meeting_detect::MeetingState>>()
                        .map(|ms| ms.tts_playing.load(std::sync::atomic::Ordering::Relaxed))
                        .unwrap_or(false),
                    has_request: crate::orchestrator::has_active_request(),
                    ghost: crate::ghost::session_active(),
                };

                match decide(hk, state) {
                    Press::OpenSettings => {
                        // settings chord (Ctrl+Shift+S on Windows, Control+Shift+S on macOS) → open settings sidebar directly
                        tracing::info!("hotkey ({}) → opening settings sidebar", hk);
                        let app_clone = handle.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = crate::commands::show_settings_sidebar(app_clone).await;
                        });
                    }
                    Press::KillStage => {
                        // kill-switch chord (Ctrl+Alt+X / Control+Alt+X) → stage kill-switch: destroy + disable the
                        // fullscreen overlay for the session (blackout escape hatch
                        // even if the renderer is wedged and can't hear IPC).
                        tracing::warn!("hotkey ({}) → stage kill-switch", hk);
                        let app_clone = handle.clone();
                        tauri::async_runtime::spawn(async move {
                            crate::orchestrator::request_barge_in("hotkey-stage-kill");
                            let _ = crate::stage::stage_hide_kill(app_clone).await;
                        });
                    }
                    Press::Cancel { abort_ghost } => {
                        // call chord (Ctrl+Space / Control+Space) → agent is active: cancel calling agent.
                        tracing::info!(
                            "hotkey ({}) → agent is active (capturing={}, speaking={}, thinking={}, ghost={}), CANCELING calling agent",
                            hk, state.capturing, state.speaking, state.has_request, state.ghost
                        );

                        // 1. Stop TTS audio immediately
                        let _ = crate::tts::stop_tts();
                        let _ = tauri::Emitter::emit(&handle, "tts:stop", ());
                        if let Some(ms) = handle.try_state::<std::sync::Arc<crate::meeting_detect::MeetingState>>() {
                            ms.set_tts_playing(false);
                        }

                        // 2. Cancel in-flight orchestrator requests
                        crate::orchestrator::cancel_active();

                        // 3. Abort STT capture
                        crate::wakeword_oww::abort_stt_capture();

                        // 4. If ghost session was active, abort it
                        if abort_ghost {
                            let app_clone = handle.clone();
                            tauri::async_runtime::spawn(async move {
                                let _ = crate::ghost::ghost_abort(app_clone).await;
                            });
                        }

                        // 5. Emit cancellation event to stage overlay to immediately hide orb & reset to idle
                        let _ = tauri::Emitter::emit(&handle, "orb:cancel", ());
                    }
                    Press::Call => {
                        // Agent is idle → CALL the agent
                        tracing::info!("hotkey ({}) → agent is idle, CALLING agent", hk);
                        println!("[ORB-HOTKEY] Hotkey ({hk}) → agent idle, calling agent & waking orb to listening");

                        // Cancel any stale request slot just in case
                        crate::orchestrator::request_barge_in("hotkey-wake");
                        let _ = tauri::Emitter::emit(&handle, "tts:stop", ());
                        let _ = crate::tts::stop_tts();

                        // Only pre-start local STT sidecar if cloud STT won't be used
                        // (saves ~340 MB RAM when Groq cloud STT is active).
                        let groq_key = crate::commands::read_groq_api_key(&handle);
                        let local_only = crate::commands::read_local_stt_only(&handle);
                        if groq_key.is_empty() || local_only {
                            std::thread::spawn(|| {
                                crate::lazy_stt::ensure_stt_running();
                            });
                        } else {
                            tracing::info!("hotkey: Groq cloud STT configured, skipping local sidecar pre-start (saves RAM)");
                        }

                        // "I heard you" sound (skipped in meetings / when switched off), then capture.
                        crate::wake_cue::play_for_wake(&handle);

                        // Start Rust-side STT capture (same as wake word path).
                        crate::wakeword_oww::start_stt_capture();
                        crate::window_manager::wake_orb_src(&handle, "hotkey");
                    }
                }
            }
        }) {
            tracing::warn!("Failed to register handler for hotkey '{hk}': {e}");
        } else {
            tracing::info!("Registered global hotkey handler: {hk}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_space_decision_matrix_all_16_states() {
        for capturing in [false, true] {
            for speaking in [false, true] {
                for has_request in [false, true] {
                    for ghost in [false, true] {
                        let state = AgentState {
                            capturing,
                            speaking,
                            has_request,
                            ghost,
                        };
                        let press = decide("CommandOrControl+Space", state);
                        if !capturing && !speaking && !has_request && !ghost {
                            assert_eq!(press, Press::Call);
                        } else {
                            assert_eq!(press, Press::Cancel { abort_ghost: ghost });
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn settings_and_kill_stage_hotkeys_for_all_16_states() {
        for capturing in [false, true] {
            for speaking in [false, true] {
                for has_request in [false, true] {
                    for ghost in [false, true] {
                        let state = AgentState {
                            capturing,
                            speaking,
                            has_request,
                            ghost,
                        };
                        assert_eq!(
                            decide("CommandOrControl+Shift+S", state),
                            Press::OpenSettings
                        );
                        assert_eq!(
                            decide("CommandOrControl+Alt+X", state),
                            Press::KillStage
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn unknown_hotkey_behaves_like_ctrl_space() {
        for capturing in [false, true] {
            for speaking in [false, true] {
                for has_request in [false, true] {
                    for ghost in [false, true] {
                        let state = AgentState {
                            capturing,
                            speaking,
                            has_request,
                            ghost,
                        };
                        let unknown_decision = decide("CommandOrControl+Shift+Alt+Z", state);
                        let ctrl_space_decision = decide("CommandOrControl+Space", state);
                        assert_eq!(unknown_decision, ctrl_space_decision);
                    }
                }
            }
        }
    }

    #[test]
    fn every_hotkey_chord_on_every_os_parses_and_is_distinct() {
        // A chord that fails to parse is skipped at start-up with only a log line, so the key would silently not
        // exist. Parse BOTH tables on every OS so a typo is caught by Windows and Linux CI too.
        for table in [HOTKEYS_WINDOWS, HOTKEYS_MACOS] {
            let mut parsed = Vec::new();
            for &(hk, _) in table {
                let sc = str::parse::<Shortcut>(hk)
                    .unwrap_or_else(|e| panic!("failed to parse hotkey '{hk}': {e}"));
                parsed.push(sc);
            }
            assert_eq!(parsed.len(), 3);
            assert_ne!(parsed[0], parsed[1]);
            assert_ne!(parsed[0], parsed[2]);
            assert_ne!(parsed[1], parsed[2]);
        }
    }

    #[test]
    fn each_os_table_has_each_role_exactly_once() {
        for table in [HOTKEYS_WINDOWS, HOTKEYS_MACOS] {
            assert_eq!(table.len(), 3);
            for role in [Role::Call, Role::Settings, Role::KillStage] {
                assert_eq!(table.iter().filter(|(_, r)| *r == role).count(), 1, "{role:?}");
            }
        }
    }

    #[test]
    fn windows_chords_are_exactly_the_documented_ones() {
        assert_eq!(
            HOTKEYS_WINDOWS,
            &[
                ("CommandOrControl+Space", Role::Call),
                ("CommandOrControl+Shift+S", Role::Settings),
                ("CommandOrControl+Alt+X", Role::KillStage),
            ]
        );
    }

    #[test]
    fn macos_call_key_is_control_space_and_no_macos_chord_uses_cmd() {
        assert_eq!(HOTKEYS_MACOS[0], ("Control+Space", Role::Call));
        assert_eq!(
            HOTKEYS_MACOS,
            &[
                ("Control+Space", Role::Call),
                ("Control+Shift+S", Role::Settings),
                ("Control+Alt+X", Role::KillStage),
            ]
        );
        // Cmd+Space is Spotlight and Cmd+Shift+S is Save As: no macOS chord may resolve to Cmd.
        for &(hk, _) in HOTKEYS_MACOS {
            let lower = hk.to_lowercase();
            for banned in ["commandorcontrol", "command", "cmd", "super", "meta"] {
                assert!(!lower.contains(banned), "{hk} must use the Control key, not {banned}");
            }
        }
    }

    #[test]
    fn the_same_agent_logic_runs_whichever_chord_is_pressed() {
        for (win, mac) in HOTKEYS_WINDOWS.iter().zip(HOTKEYS_MACOS.iter()) {
            assert_eq!(win.1, mac.1, "the two tables list the roles in the same order");
            for capturing in [false, true] {
                for speaking in [false, true] {
                    for has_request in [false, true] {
                        for ghost in [false, true] {
                            let state = AgentState { capturing, speaking, has_request, ghost };
                            assert_eq!(decide(win.0, state), decide(mac.0, state), "{} vs {}", win.0, mac.0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_active_table_is_the_one_for_this_os() {
        if cfg!(target_os = "macos") {
            assert_eq!(active_hotkeys(), HOTKEYS_MACOS);
        } else {
            assert_eq!(active_hotkeys(), HOTKEYS_WINDOWS);
        }
    }
}
