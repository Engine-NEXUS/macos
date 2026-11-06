//! IPC commands for setup window management and configuration.

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(feature = "wakeword-sherpa")]
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};
#[cfg(not(target_os = "windows"))]
use tauri_plugin_autostart::ManagerExt;

// Phase D: voice_profile is now available for both wakeword-oww and wakeword-sherpa
use crate::voice_profile;
use crate::app_registry;

// ─── Pending sidebar content ───────────────────────────────────────
//
// When the sidebar window is created on-demand, the WebView2 needs time
// to load sidebar.html and mount the React app before it can receive
// Tauri events. If we emit `sidebar:show` / `sidebar:backdrop` immediately
// after creating the window, those events are lost because no listener
// exists yet.
//
// Instead, we store the content + backdrop here. The frontend calls
// `get_pending_sidebar_content` on mount, which returns and clears the
// pending data. This is race-free regardless of how long the WebView
// takes to load.

#[derive(Clone)]
struct PendingSidebar {
    query: String,
    text: String,
    backdrop: Option<String>, // data:image/png;base64,... URI
    analysis: Option<serde_json::Value>, // structured repo analysis data
    confirmation: Option<serde_json::Value>, // action confirmation payload
}

static PENDING_SIDEBAR: Mutex<Option<PendingSidebar>> = Mutex::new(None);

// ─── Pending PR list (race-free sidebar creation) ──────────────────────
// Same pattern as PENDING_SIDEBAR: the orchestrator stores the PR list
// result here before creating the sidebar window. The frontend calls
// `get_pending_pr_list` on mount to fetch (and clear) the pending data.
// This is race-free regardless of how long the WebView takes to load.
static PENDING_PR_LIST: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Store a pending PR list result (called from the orchestrator before
/// creating the sidebar window). The frontend fetches this on mount.
pub fn set_pending_pr_list(pr_list_json: serde_json::Value) {
    let mut pending = PENDING_PR_LIST.lock().unwrap();
    *pending = Some(pr_list_json);
}

/// IPC: Fetch pending PR list data (called by the PR list sidebar on mount).
/// Returns the PR list JSON that was stored by the orchestrator, or null
/// if no data is pending. Clears the pending data after returning.
#[tauri::command]
pub fn get_pending_pr_list() -> Result<Option<serde_json::Value>, String> {
    let mut pending = PENDING_PR_LIST.lock().unwrap();
    let data = pending.take();
    if data.is_some() {
        tracing::info!("pr-list: pending data fetched by frontend");
    }
    Ok(data)
}

/// IPC: open the setup window (called from tray menu "Settings…" or first launch).
/// Creates the window on-demand if it doesn't exist (saves ~250 MB RAM at idle).
#[tauri::command]
pub fn open_setup_window<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    let win = crate::dyn_windows::get_or_create_window(&app, crate::dyn_windows::WindowConfig::setup())?;
    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// IPC: close/destroy the setup window and activate the main assistant orb.
/// Destroys the window (not just hide) to free ~250 MB of WebView2 processes.
/// If `first_run` is true, speaks the first-run greeting instead of waking.
#[tauri::command]
pub fn close_setup_window<R: Runtime>(
    app: tauri::AppHandle<R>,
    first_run: Option<bool>,
) -> Result<(), String> {
    let _ = crate::dyn_windows::destroy_window(&app, "setup");
    crate::window_manager::show_orb_interactive(&app);
    if first_run.unwrap_or(false) {
        let _ = tauri::Emitter::emit(&app, "orb:first_run_greeting", ());
    } else {
        let _ = tauri::Emitter::emit(&app, "orb:wake", ());
    }
    Ok(())
}

/// IPC: save the server URL config (marks setup as complete).
/// Writes a JSON file to the app data dir so the app knows setup is done.
#[tauri::command]
pub fn save_server_config<R: Runtime>(
    app: tauri::AppHandle<R>,
    server_url: String,
    user_id: String,
    device_id: String,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let config_path = dir.join("nexus-config.json");
    let config = serde_json::json!({
        "serverUrl": server_url,
        "userId": user_id,
        "deviceId": device_id,
    });
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(&config_path, config.to_string()).map_err(|e| e.to_string())?;
    tracing::info!("server config saved to {:?}", config_path);
    Ok(())
}

/// Serialized server config returned by `get_server_config`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub server_url: String,
    pub user_id: String,
    pub device_id: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            server_url: option_env!("NEXUS_SERVER_URL")
                .unwrap_or("https://nexus-worker.chitkullakshya.workers.dev")
                .to_string(),
            user_id: String::new(),
            device_id: String::new(),
        }
    }
}

/// IPC: Get the saved server config (or defaults if not yet configured).
/// The frontend calls this at startup to get the Worker URL, user ID,
/// and device ID — instead of relying on build-time env vars.
#[tauri::command]
pub fn get_server_config<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<ServerConfig, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let config_path = dir.join("nexus-config.json");
    if !config_path.exists() {
        return Ok(ServerConfig::default());
    }
    let content = std::fs::read_to_string(&config_path).map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let default_url = option_env!("NEXUS_SERVER_URL")
        .unwrap_or("https://nexus-worker.chitkullakshya.workers.dev");
    // Use saved URL, but fall back to default if the saved URL is empty
    // (a previous save_settings call may have written an empty string).
    let saved_url = json["serverUrl"].as_str().unwrap_or(default_url);
    let server_url = if saved_url.is_empty() { default_url } else { saved_url };
    Ok(ServerConfig {
        server_url: server_url.to_string(),
        user_id: json["userId"].as_str().unwrap_or("").to_string(),
        device_id: json["deviceId"].as_str().unwrap_or("").to_string(),
    })
}

// ─── Feature 88: canonical laptop identity commands ──────────────

/// IPC: Claim the canonical profile from the Worker (setup Accounts step).
/// The Worker issues profile_id/device_id/device_token; the token goes to
/// the OS keyring. Idempotent — safe to call again on retry.
#[tauri::command]
pub async fn claim_profile<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<crate::identity_state::IdentityStatus, String> {
    let status = crate::identity_state::claim(&app).await?;
    // Kick off the pending-approval poll loop (bounded).
    crate::identity_state::spawn_pending_poll(app);
    Ok(status)
}

/// IPC: Current identity state (provisional/pending/approved/suspended/revoked).
#[tauri::command]
pub fn get_identity_status<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<crate::identity_state::IdentityStatus, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::identity_state::identity_status(&dir))
}

/// IPC: Force-refresh identity state from the Worker (/v1/profiles/me).
#[tauri::command]
pub async fn refresh_identity_status<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<crate::identity_state::IdentityStatus, String> {
    crate::identity_state::refresh_status(&app).await
}

/// IPC: Self-revoke this device (settings Identity card "Disconnect").
#[tauri::command]
pub async fn disconnect_device<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    crate::identity_state::disconnect(&app).await
}

// ─── Voice profile commands ──────────────────────────────────────
// Phase D: Speaker verification is now available for both engines.
// - wakeword-sherpa: uses sherpa-onnx for embedding extraction (C++ deps)
// - wakeword-oww: uses the existing embedding_model.onnx (pure Rust)
//
// The OWW commands use the voice_profile module's SpeakerVerifier which
// extracts embeddings from the OWW embedding model via tract-onnx.
#[cfg(feature = "wakeword-sherpa")]
pub use voice_profile_commands::*;
#[cfg(feature = "wakeword-sherpa")]
mod voice_profile_commands {
    use super::*;

    /// IPC: Get the current voice profile status (enrolled or not, number of clips, threshold).
    #[tauri::command]
    pub fn get_voice_profile_status<R: Runtime>(
        app: tauri::AppHandle<R>,
    ) -> Result<voice_profile::VoiceProfileStatus, String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let profile_path = voice_profile::resolve_profile_path(&dir);

        let sound_alikes: Vec<String> = voice_profile::SOUND_ALIKES
            .iter()
            .map(|s| s.to_string())
            .collect();

        if !profile_path.exists() {
            return Ok(voice_profile::VoiceProfileStatus {
                enrolled: false,
                num_clips: 0,
                threshold: voice_profile::DEFAULT_THRESHOLD,
                created_at: 0,
                updated_at: 0,
                wake_variants: vec!["nexus".to_string()],
                sound_alikes,
            });
        }

        let profile = voice_profile::VoiceProfile::load(&profile_path)
            .map_err(|e| e.to_string())?;
        Ok(voice_profile::VoiceProfileStatus {
            enrolled: profile.is_valid(),
            num_clips: profile.num_clips,
            threshold: profile.threshold,
            created_at: profile.created_at,
            updated_at: profile.updated_at,
            wake_variants: profile.wake_variants,
            sound_alikes,
        })
    }

    /// Resolve the sherpa resource directory (handles dev + production paths).
    pub fn resolve_sherpa_dir(resource_dir: &Path) -> Option<PathBuf> {
        // Production: resource_dir/resources/sherpa (Tauri v2 on Windows: resource_dir() = exe_dir)
        let sherpa = resource_dir.join("resources").join("sherpa");
        if sherpa.exists() {
            return Some(sherpa);
        }
        // Fallback: resource_dir/sherpa (some Tauri versions may return resources/ directly)
        let sherpa_alt = resource_dir.join("sherpa");
        if sherpa_alt.exists() {
            return Some(sherpa_alt);
        }
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            let dev = PathBuf::from(manifest).join("resources").join("sherpa");
            if dev.exists() {
                return Some(dev);
            }
        }
        // Fallback: exe_dir/../resources/sherpa
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let p = parent.join("../resources/sherpa");
                if p.exists() {
                    return Some(p);
                }
            }
        }
        None
    }

    /// Run ASR on enrollment clips to capture wake-word variants.
    /// Returns the list of ASR transcripts (one per clip, may be empty/garbage).
    pub fn transcribe_enrollment_clips(
        sherpa_dir: &Path,
        clips: &[Vec<f32>],
    ) -> Result<Vec<String>, String> {
        use sherpa_onnx::{
            OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineTransducerModelConfig,
        };

        let kws_dir = sherpa_dir.join("kws");

        // Prefer int8 (quantized) models, fall back to fp32
        let encoder = kws_dir.join("encoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx");
        let encoder = if encoder.exists() { encoder } else { kws_dir.join("encoder-epoch-12-avg-2-chunk-16-left-64.onnx") };
        let decoder = kws_dir.join("decoder-epoch-12-avg-2-chunk-16-left-64.int8.onnx");
        let decoder = if decoder.exists() { decoder } else { kws_dir.join("decoder-epoch-12-avg-2-chunk-16-left-64.onnx") };
        let joiner = kws_dir.join("joiner-epoch-12-avg-2-chunk-16-left-64.int8.onnx");
        let joiner = if joiner.exists() { joiner } else { kws_dir.join("joiner-epoch-12-avg-2-chunk-16-left-64.onnx") };
        let tokens = kws_dir.join("tokens.txt");

        for (name, path) in [
            ("encoder", &encoder),
            ("decoder", &decoder),
            ("joiner", &joiner),
            ("tokens", &tokens),
        ] {
            if !path.exists() {
                return Err(format!("ASR model file '{}' not found at: {}", name, path.display()));
            }
        }

        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: Some(encoder.to_string_lossy().to_string()),
                    decoder: Some(decoder.to_string_lossy().to_string()),
                    joiner: Some(joiner.to_string_lossy().to_string()),
                },
                tokens: Some(tokens.to_string_lossy().to_string()),
                num_threads: 1,
                provider: Some("cpu".to_string()),
                ..Default::default()
            },
            decoding_method: Some("greedy_search".to_string()),
            enable_endpoint: false,
            ..Default::default()
        };

        let recognizer = OnlineRecognizer::create(&config)
            .ok_or_else(|| "Failed to create OnlineRecognizer for enrollment".to_string())?;

        let mut variants = Vec::with_capacity(clips.len());

        for (i, clip) in clips.iter().enumerate() {
            if clip.is_empty() {
                variants.push(String::new());
                continue;
            }

            let stream = recognizer.create_stream();

            // Feed the clip + 0.5s tail padding
            stream.accept_waveform(16000, clip);
            let tail = vec![0.0f32; 8000];
            stream.accept_waveform(16000, &tail);
            stream.input_finished();

            while recognizer.is_ready(&stream) {
                recognizer.decode(&stream);
            }

            let text = if let Some(result) = recognizer.get_result(&stream) {
                result.text.trim().to_lowercase()
            } else {
                String::new()
            };

            tracing::info!("Enrollment clip {} ASR transcript: \"{}\"", i + 1, text);
            variants.push(text);

            recognizer.reset(&stream);
        }

        Ok(variants)
    }

    /// IPC: Enroll a voice profile from multiple audio clips.
    /// Each clip is a Vec<f32> of 16kHz mono audio samples.
    /// Also runs ASR on each clip to capture wake-word variants.
    /// Re-enrollment APPENDS new variants to existing ones (does not wipe).
    #[tauri::command]
    pub fn enroll_voice<R: Runtime>(
        app: tauri::AppHandle<R>,
        clips: Vec<Vec<f32>>,
        threshold: Option<f32>,
    ) -> Result<Vec<String>, String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let profile_path = voice_profile::resolve_profile_path(&dir);

        let resource_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
        let sherpa_dir = resolve_sherpa_dir(&resource_dir)
            .ok_or_else(|| "Sherpa resource directory not found".to_string())?;

        let speaker_model = sherpa_dir.join("speaker_model.onnx");

        let mut verifier = voice_profile::SpeakerVerifier::new(speaker_model, profile_path)
            .map_err(|e| e.to_string())?;

        // Run ASR on each clip to capture wake-word variants
        let asr_variants = transcribe_enrollment_clips(&sherpa_dir, &clips)
            .map_err(|e| {
                tracing::warn!("Enrollment ASR failed (continuing without variants): {e}");
                // Don't fail enrollment if ASR fails — just use empty variants
                vec![String::new(); clips.len()]
            })
            .unwrap_or_else(|_| vec![String::new(); clips.len()]);

        let threshold = threshold.unwrap_or(voice_profile::DEFAULT_THRESHOLD);
        verifier
            .enroll(&clips, threshold, asr_variants.clone())
            .map_err(|e| e.to_string())?;

        // Return the captured variants so the UI can show them
        let captured: Vec<String> = verifier
            .profile()
            .map(|p| p.wake_variants.clone())
            .unwrap_or_default();

        Ok(captured)
    }

    /// IPC: Delete the profile; an enabled Voice Lock continues to fail closed.
    #[tauri::command]
    pub fn delete_voice_profile<R: Runtime>(
        app: tauri::AppHandle<R>,
    ) -> Result<(), String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let profile_path = voice_profile::resolve_profile_path(&dir);

        voice_profile::delete_profile_file(&profile_path).map_err(|e| e.to_string())?;
        tracing::info!("Voice profile deleted");
        Ok(())
    }
}

// ─── OWW Voice Profile Commands (Phase D) ──────────────────────────
// These commands work with the default wakeword-oww engine using the
// voice_profile module's SpeakerVerifier (pure Rust, no C++ deps).
// Enrollment extracts embeddings from the OWW embedding_model.onnx.
#[cfg(feature = "wakeword-oww")]
pub use oww_voice_profile_commands::*;
#[cfg(feature = "wakeword-oww")]
mod oww_voice_profile_commands {
    use super::*;

    /// IPC: Get the current voice profile status (enrolled or not, number of clips, threshold).
    #[tauri::command]
    pub fn get_voice_profile_status<R: Runtime>(
        app: tauri::AppHandle<R>,
    ) -> Result<voice_profile::VoiceProfileStatus, String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let profile_path = voice_profile::resolve_profile_path(&dir);

        let sound_alikes: Vec<String> = voice_profile::SOUND_ALIKES
            .iter()
            .map(|s| s.to_string())
            .collect();

        if !profile_path.exists() {
            return Ok(voice_profile::VoiceProfileStatus {
                enrolled: false,
                num_clips: 0,
                threshold: voice_profile::DEFAULT_THRESHOLD,
                created_at: 0,
                updated_at: 0,
                wake_variants: vec!["nexus".to_string()],
                sound_alikes,
            });
        }

        let profile = voice_profile::VoiceProfile::load(&profile_path)
            .map_err(|e| e.to_string())?;
        Ok(voice_profile::VoiceProfileStatus {
            enrolled: profile.is_valid(),
            num_clips: profile.num_clips,
            threshold: profile.threshold,
            created_at: profile.created_at,
            updated_at: profile.updated_at,
            wake_variants: profile.wake_variants,
            sound_alikes,
        })
    }

    /// IPC: Enroll the user's voice for speaker verification.
    /// Accepts ten audio clips (16kHz mono f32): five enrollment recordings
    /// and five held-out owner recordings for provisional calibration.
    #[tauri::command]
    pub async fn enroll_voice<R: Runtime>(
        app: tauri::AppHandle<R>,
        clips: Vec<Vec<f32>>,
        threshold: Option<f32>,
    ) -> Result<Vec<String>, String> {
        if threshold.is_some() {
            return Err("Manual speaker thresholds are no longer supported".to_string());
        }
        if clips.len() != 10 || clips.iter().any(|clip| {
            !(16_000..=80_000).contains(&clip.len())
                || clip.iter().any(|sample| !sample.is_finite() || sample.abs() > 1.0)
        }) {
            return Err("Record ten valid 1–5 second clips: five for enrollment and five held out".to_string());
        }
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let profile_path = voice_profile::resolve_profile_path(&dir);

        let resource_dir = app.path().resource_dir().map_err(|e| e.to_string())?;
        let embeddings = tauri::async_runtime::spawn_blocking(move || {
            crate::wakeword_oww::enrollment_embeddings(&resource_dir, &clips)
        }).await.map_err(|error| format!("Enrollment worker failed: {error}"))?
            .map_err(|error| error.to_string())?;

        if embeddings.len() != 10 {
            return Err("Could not extract all ten voice embeddings".to_string());
        }

        let calibrated_threshold = voice_profile::calibrate_owner_threshold(&embeddings[..5], &embeddings[5..])
            .map_err(|e| e.to_string())?;
        // Held-out recordings never enter the stored profile.
        let mut verifier = voice_profile::SpeakerVerifier::for_enrollment(profile_path);
        verifier
            .enroll(embeddings[..5].to_vec(), calibrated_threshold, vec!["nexus".to_string(), "hey nexus".to_string(), "ok nexus".to_string()])
            .map_err(|e| e.to_string())?;

        let captured: Vec<String> = verifier
            .profile()
            .map(|p| p.wake_variants.clone())
            .unwrap_or_default();

        tracing::info!("Voice profile enrolled (5 clips + 5 held out, threshold {:.2})", calibrated_threshold);
        Ok(captured)
    }

    /// IPC: Delete the profile; an enabled Voice Lock continues to fail closed.
    #[tauri::command]
    pub fn delete_voice_profile<R: Runtime>(
        app: tauri::AppHandle<R>,
    ) -> Result<(), String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let profile_path = voice_profile::resolve_profile_path(&dir);

        voice_profile::delete_profile_file(&profile_path).map_err(|e| e.to_string())?;
        tracing::info!("Voice profile deleted");
        Ok(())
    }
}

// ─── Meeting / privacy mode commands ─────────────────────────────────

/// IPC: Check whether TTS should be suppressed right now.
///
/// The frontend calls this before speaking to decide whether to
/// produce audible TTS or show a silent visual response instead.
///
/// Uses `should_suppress_tts()` (not `is_meeting_active()`) so that
/// disabling auto-detection in settings takes effect immediately.
/// `is_meeting_active()` only reports the raw detection flag, which the
/// polling loop clears up to 2s later — long enough for the user to
/// disable detection and still have their next response muted.
#[tauri::command]
pub fn meeting_active<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<bool, String> {
    let state = app
        .try_state::<std::sync::Arc<crate::meeting_detect::MeetingState>>()
        .ok_or_else(|| "meeting state not managed".to_string())?;
    Ok(state.should_suppress_tts())
}

/// IPC: Check if NEXUS is paused (manual pause via tray).
#[tauri::command]
pub fn is_nexus_paused<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<bool, String> {
    let state = app
        .try_state::<std::sync::Arc<crate::meeting_detect::MeetingState>>()
        .ok_or_else(|| "meeting state not managed".to_string())?;
    Ok(state.is_paused())
}

/// IPC: Get the full meeting/privacy mode status.
#[tauri::command]
pub fn meeting_status<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<MeetingStatus, String> {
    let state = app
        .try_state::<std::sync::Arc<crate::meeting_detect::MeetingState>>()
        .ok_or_else(|| "meeting state not managed".to_string())?;
    Ok(MeetingStatus {
        meeting_active: state.is_meeting_active(),
        paused: state.is_paused(),
        tts_playing: state.tts_playing.load(std::sync::atomic::Ordering::Relaxed),
        detection_enabled: state.detection_enabled.load(std::sync::atomic::Ordering::Relaxed),
    })
}

/// IPC: Enable or disable automatic meeting detection.
#[tauri::command]
pub fn set_meeting_detection<R: Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    let state = app
        .try_state::<std::sync::Arc<crate::meeting_detect::MeetingState>>()
        .ok_or_else(|| "meeting state not managed".to_string())?;
    state.detection_enabled.store(enabled, std::sync::atomic::Ordering::Relaxed);
    tracing::info!("meeting detection: {}", if enabled { "enabled" } else { "disabled" });
    Ok(())
}

/// Serialized meeting status returned by `meeting_status`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MeetingStatus {
    pub meeting_active: bool,
    pub paused: bool,
    pub tts_playing: bool,
    pub detection_enabled: bool,
}

// ─── Response Sidebar window ─────────────────────────────────────────

/// IPC: Show the response sidebar window (positioned at bottom-right).
/// Called when a server response is incoming (n8n/Ollama/Hermes).
/// Creates the sidebar window on-demand if it doesn't exist (saves ~250 MB RAM at idle).
///
/// MUST be async — WebviewWindowBuilder::build() dispatches to the main thread,
/// and a synchronous command runs on a blocking thread that can't yield, causing
/// a deadlock. Async commands run on the tokio runtime which can properly yield.
#[tauri::command]
pub async fn show_sidebar<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    // Unified window delegate — Assistant view, default right dock.
    let _ = unified_show_sidebar(&app, "assistant", None).await?;
    Ok(())
}

/// IPC: Show the sidebar AND set its content.
///
/// The sidebar window is created on-demand. Since the React app needs time
/// to load before it can receive Tauri events, we store the content + backdrop
/// in a static. The frontend calls `get_pending_sidebar_content` on mount,
/// which returns and clears the pending data. This is race-free.
///
/// If the window already exists (React already loaded), we also emit the
/// `sidebar:show` event as a fast path — the event listener will handle it
/// immediately without needing to poll the pending content.
#[tauri::command]
pub async fn show_sidebar_with_content<R: Runtime>(
    app: tauri::AppHandle<R>,
    query: String,
    text: String,
) -> Result<(), String> {
    let window_existed = app.get_webview_window("sidebar").is_some();

    // Unified delegate — Assistant view, right dock. prepare_sidebar
    // applies geometry WITHOUT showing, so the backdrop capture below
    // photographs the desktop, not the sidebar itself.
    let prep = prepare_sidebar(&app, "assistant", None).await?;
    let backdrop = capture_and_emit_backdrop(&app, &prep);

    // Store the pending content so the frontend can fetch it on mount.
    // This handles the fresh-window case where events would be missed.
    {
        let mut pending = PENDING_SIDEBAR.lock().unwrap();
        *pending = Some(PendingSidebar {
            query: query.clone(),
            text: text.clone(),
            backdrop: backdrop.clone(),
            analysis: None,
            confirmation: None,
        });
    }

    // Show + focus + emit sidebar:set_view (React switches view instantly).
    finish_sidebar(&app, &prep.win, "assistant", None)?;
    spawn_sidebar_live_blur(&app, &prep.win);

    // If the window already existed (React already loaded), also emit the
    // event as a fast path. The frontend listener will handle it immediately.
    if window_existed {
        let payload = serde_json::json!({
            "query": query,
            "text": text,
        });
        let _ = app.emit("sidebar:show", payload);
        // Also emit the backdrop if we captured one.
        if let Some(uri) = backdrop {
            let _ = app.emit("sidebar:backdrop", uri);
        }
    }

    tracing::info!("sidebar: shown with content (query={} chars, text={} chars, window_existed={})", query.len(), text.len(), window_existed);
    Ok(())
}

/// IPC: Show the sidebar with structured analysis data (rich dashboard).
/// Like `show_sidebar_with_content` but also stores the analysis JSON so
/// the frontend can render the AnalysisDashboard with pie charts.
#[tauri::command]
pub async fn show_sidebar_with_analysis<R: Runtime>(
    app: tauri::AppHandle<R>,
    query: String,
    text: String,
    analysis: serde_json::Value,
) -> Result<(), String> {
    let window_existed = app.get_webview_window("sidebar").is_some();

    let is_coderabbit = analysis.get("findings").is_some()
        || analysis.get("activePrs").is_some()
        || query.to_lowercase().contains("coderabbit")
        || query.to_lowercase().contains("code rabbit");
    let view_name = if is_coderabbit { "coderabbit" } else { "assistant" };
    let dock_opt = if is_coderabbit { Some("center") } else { None };

    // Unified delegate — Assistant or CodeRabbit view, geometry before capture.
    let prep = prepare_sidebar(&app, view_name, dock_opt).await?;
    let backdrop = capture_and_emit_backdrop(&app, &prep);

    {
        let mut pending = PENDING_SIDEBAR.lock().unwrap();
        *pending = Some(PendingSidebar {
            query: query.clone(),
            text: text.clone(),
            backdrop: backdrop.clone(),
            analysis: Some(analysis.clone()),
            confirmation: None,
        });
    }

    finish_sidebar(&app, &prep.win, view_name, dock_opt)?;
    spawn_sidebar_live_blur(&app, &prep.win);

    // Fast path: if the window already exists, also emit events
    if window_existed {
        let _ = app.emit("sidebar:show", serde_json::json!({
            "query": query,
            "text": text,
        }));
        let _ = app.emit("sidebar:analysis", serde_json::json!({
            "query": query,
            "text": text,
            "analysis": analysis,
        }));
        if let Some(uri) = backdrop {
            let _ = app.emit("sidebar:backdrop", uri);
        }
    }

    tracing::info!("sidebar: shown with analysis (query={} chars, text={} chars, window_existed={})", query.len(), text.len(), window_existed);
    Ok(())
}

/// IPC: Show the sidebar with an action confirmation dialog.
/// Stores the confirmation JSON so the frontend can render the ConfirmationPanel
/// with Confirm and Cancel buttons.
#[tauri::command]
pub async fn show_sidebar_with_confirmation<R: Runtime>(
    app: tauri::AppHandle<R>,
    query: String,
    prompt: String,
    confirmation: serde_json::Value,
) -> Result<(), String> {
    let window_existed = app.get_webview_window("sidebar").is_some();

    // Unified delegate — Assistant view, right dock, geometry before capture.
    let prep = prepare_sidebar(&app, "assistant", None).await?;
    let backdrop = capture_and_emit_backdrop(&app, &prep);

    {
        let mut pending = PENDING_SIDEBAR.lock().unwrap();
        *pending = Some(PendingSidebar {
            query: query.clone(),
            text: prompt.clone(),
            backdrop: backdrop.clone(),
            analysis: None,
            confirmation: Some(confirmation.clone()),
        });
    }

    finish_sidebar(&app, &prep.win, "assistant", None)?;
    spawn_sidebar_live_blur(&app, &prep.win);

    if window_existed {
        let _ = app.emit("sidebar:show", serde_json::json!({
            "query": query,
            "text": prompt,
        }));
        let _ = app.emit("sidebar:confirmation", serde_json::json!({
            "query": query,
            "prompt": prompt,
            "confirmation": confirmation,
        }));
        if let Some(uri) = backdrop {
            let _ = app.emit("sidebar:backdrop", uri);
        }
    }

    tracing::info!("sidebar: shown with confirmation (query={}, prompt={}, window_existed={})", query, prompt, window_existed);
    Ok(())
}

/// Store pending sidebar content from non-commands modules (the OCR
/// fallback in the orchestrator). Same race-free pattern; no backdrop.
pub fn set_pending_sidebar_text(query: String, text: String) {
    let mut pending = PENDING_SIDEBAR.lock().unwrap();
    *pending = Some(PendingSidebar {
        query,
        text,
        backdrop: None,
        analysis: None,
        confirmation: None,
    });
}

/// IPC: Fetch pending sidebar content (called by the frontend on mount).
/// Returns the content + backdrop + analysis + confirmation that was stored by
/// `show_sidebar_with_content`, `show_sidebar_with_analysis`, or
/// `show_sidebar_with_confirmation`, or null if no content is pending.
/// Clears the pending data after returning.
#[tauri::command]
pub fn get_pending_sidebar_content() -> Result<Option<serde_json::Value>, String> {    let mut pending = PENDING_SIDEBAR.lock().unwrap();
    let data = pending.take();
    match data {
        Some(p) => {
            tracing::info!("sidebar: pending content fetched (query={} chars, text={} chars, has_backdrop={}, has_analysis={}, has_confirmation={})", p.query.len(), p.text.len(), p.backdrop.is_some(), p.analysis.is_some(), p.confirmation.is_some());
            Ok(Some(serde_json::json!({
                "query": p.query,
                "text": p.text,
                "backdrop": p.backdrop,
                "analysis": p.analysis,
                "confirmation": p.confirmation,
            })))
        }
        None => Ok(None),
    }
}

/// Capture the desktop region behind the sidebar window (Windows only).
/// Must be called BEFORE `win.show()` so we don't capture the sidebar itself.
/// If `geom` is provided, uses those physical coords directly (avoids a race
/// where `win.outer_position()`/`inner_size()` haven't propagated yet).
/// Returns the blurred backdrop as a `data:image/jpeg;base64,...` URI, or None.
pub(crate) fn capture_backdrop<R: Runtime>(
    _app: &tauri::AppHandle<R>,
    win: &tauri::WebviewWindow<R>,
    geom: Option<(i32, i32, i32, i32)>,
) -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        let (x, y, phys_w, phys_h) = match geom {
            Some(g) => g,
            None => {
                let pos = win.outer_position().ok()?;
                let scale = win.scale_factor().ok()?;
                let logical = win.inner_size().ok()?.to_logical::<f64>(scale);
                (pos.x, pos.y, (logical.width * scale) as i32, (logical.height * scale) as i32)
            }
        };

        match crate::sidebar_backdrop::capture_and_blur_jpeg(x, y, phys_w, phys_h, 10.0) {
            Some(data_uri) => {
                tracing::info!("sidebar: backdrop captured ({} bytes)", data_uri.len());
                Some(data_uri)
            }
            None => {
                tracing::warn!("sidebar: backdrop capture failed (x={}, y={}, w={}, h={})", x, y, phys_w, phys_h);
                None
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (win, geom);
        None
    }
}

// ─── Unified dynamic sidebar (one window, four views) ────────────────
//
// ONE Tauri window ("sidebar") hosts every panel view — Assistant,
// Command Hub (settings), Architect, PR List — switched in React via
// `sidebar:set_view` without creating/destroying OS HWNDs. This kills
// the 4×WebView2 process cost (~250 MB each), the z-order fighting, and
// the spawn flicker, while the single compact HWND keeps native DWM
// hardware glass + rounded corners + capture exclusion fully active.
//
// Current view geometry (logical px):
//   assistant / pr-list : 520×min(1040, mh-40), right dock (x=mw-530, y=20)
//   settings            : 400×min(780, mh-40), right dock (x=mw-410, y=20)
//   architect           : 960×min(1040, mh-40), centered (y=40)
// Explicit left/right/center docks override the default anchor per view.

/// The currently active sidebar view — kept so `set_sidebar_dock` can
/// re-anchor the window without the frontend re-sending the view name.
static ACTIVE_SIDEBAR_VIEW: Mutex<String> = Mutex::new(String::new());

/// Requested-but-possibly-lost view — the same race as pending sidebar
/// content: Rust emits `sidebar:set_view` the moment the window shows,
/// but the React router may not have mounted yet. Persist here; the
/// frontend fetches (and clears) it on mount so the FIRST show lands on
/// the right view.
static PENDING_SIDEBAR_VIEW: Mutex<Option<String>> = Mutex::new(None);

fn set_active_sidebar_view(view: &str) {
    *ACTIVE_SIDEBAR_VIEW.lock().unwrap() = view.to_string();
}

/// IPC: Fetch the pending sidebar view (router calls this on mount).
/// Returns the view name that was requested before the router mounted,
/// or null. Clears the pending value.
#[tauri::command]
pub fn get_pending_sidebar_view() -> Result<Option<String>, String> {
    let mut pending = PENDING_SIDEBAR_VIEW.lock().unwrap();
    Ok(pending.take())
}

// ─── Pending spatial analysis (Feature 86 race-free pattern) ─────────
// The orchestrator stores the full spatial payload BEFORE showing the
// unified sidebar; the Spatial view fetches it on mount (fresh-window
// race path) — same shape as PENDING_SIDEBAR.
static PENDING_SPATIAL: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Store the spatial payload (called by the orchestrator).
pub fn set_pending_spatial(payload: &serde_json::Value) {
    *PENDING_SPATIAL.lock().unwrap() = Some(payload.clone());
}

/// IPC: Fetch pending spatial data (Spatial view calls this on mount).
/// Clears after returning.
#[tauri::command]
pub fn get_pending_spatial() -> Result<Option<serde_json::Value>, String> {
    let mut pending = PENDING_SPATIAL.lock().unwrap();
    Ok(pending.take())
}

// ─── Pending annotation seed (Feature 87 race-free pattern) ─────────
// Same shape as PENDING_SPATIAL: the orchestrator stores the seed canvas
// BEFORE showing the unified sidebar; the Annotate view fetches it on
// mount (fresh-window race path) and also listens for show_annotation
// (warm-window path).
static PENDING_ANNOTATION: Mutex<Option<serde_json::Value>> = Mutex::new(None);

/// Store the annotation seed (called by the orchestrator).
pub fn set_pending_annotation(payload: &serde_json::Value) {
    *PENDING_ANNOTATION.lock().unwrap() = Some(payload.clone());
}

/// IPC: Fetch pending annotation seed (Annotate view calls this on mount).
/// Clears after returning.
#[tauri::command]
pub fn get_pending_annotation() -> Result<Option<serde_json::Value>, String> {
    let mut pending = PENDING_ANNOTATION.lock().unwrap();
    Ok(pending.take())
}

/// Pure geometry resolver for the unified sidebar window.
/// Returns (x, y, width, height) in LOGICAL pixels for `view`/`dock` on a
/// monitor of logical size (monitor_w, monitor_h). Unit-tested — keep it
/// side-effect free.
pub fn sidebar_geometry(
    view: &str,
    dock: Option<&str>,
    monitor_w: f64,
    monitor_h: f64,
) -> (f64, f64, f64, f64) {
    let left = dock == Some("left");
    let right = dock == Some("right");
    let docked = left || right;
// Right dock: window sits inside the right screen edge with a 10px margin.
// x = monitor_w - w - 10. Left dock: 10px from left edge. y = 20px top
// margin for both. The right edge must never leave the screen.
    let dock_x = |w: f64, h: f64| -> (f64, f64, f64, f64) {
        let x = if left {
            10.0
        } else {
            (monitor_w - w - 10.0).max(0.0)
        };
        (x, 20.0, w, h)
    };
    let center_xy = |w: f64, h: f64| -> (f64, f64, f64, f64) {
        (
            ((monitor_w - w) / 2.0).max(0.0),
            ((monitor_h - h) / 2.0).max(0.0),
            w,
            h,
        )
    };
    match view {
        "settings" => {
            // Command Hub: full vertical span card — 400px wide, covering
            // from top margin down to screen edge / taskbar, right-docked (or left).
            let h = (monitor_h - 40.0).min(1040.0).max(400.0);
            dock_x(400.0, h)
        }
        "architect" | "maps" => {
            let h = (monitor_h - 40.0).min(1040.0).max(400.0);
            if docked {
                dock_x(960.0, h)
            } else {
                // Centered horizontally, pinned near the top (y=40, spec).
                let x = ((monitor_w - 960.0) / 2.0).max(0.0);
                (x, 40.0, 960.0, h)
            }
        }
        "coderabbit" => {
            // CodeRabbit-panel width (595) with the same full-height rule as every other sidebar view.
            let w = 595.0_f64.min((monitor_w - 20.0).max(320.0));
            let h = (monitor_h - 40.0).min(1040.0).max(400.0);
            if docked { dock_x(w, h) } else { center_xy(w, h) }
        }
        "pr-list" | "github-dashboard" => {
            // Exact same size as Settings — 400px wide, covering from top margin down to screen edge.
            let h = (monitor_h - 40.0).min(1040.0).max(400.0);
            if dock == Some("center") { center_xy(400.0, h) } else { dock_x(400.0, h) }
        }
        // The Architect map as its own window, directly LEFT of the right-docked response sidebar (520 px wide).
        // 10 px from the left edge, 10 px gap to the sidebar, at most 960 wide.
        "architect-beside" => {
            let h = (monitor_h - 40.0).min(1040.0).max(400.0);
            let sidebar_x = (monitor_w - 520.0 - 10.0).max(0.0);
            let w = (sidebar_x - 20.0).min(960.0).max(320.0);
            let x = (sidebar_x - 10.0 - w).max(0.0);
            (x, 20.0, w, h)
        }
        // The annotation palette is a small tool strip, not a panel: a compact window so it does not cover the screen
        // being annotated.
        "annotate" => {
            let h = 380.0_f64.min((monitor_h - 40.0).max(300.0));
            dock_x(320.0, h)
        }
        // "assistant" | fallback → default right dock, full height.
        _ => {
            let h = (monitor_h - 40.0).min(1040.0).max(400.0);
            if dock == Some("center") { center_xy(520.0, h) } else { dock_x(520.0, h) }
        }
    }
}

/// One shared 1 FPS live-blur loop for the unified sidebar (replaces the
/// three per-view loops). Reads the window's ACTUAL position + size every
/// tick, so view switches and dock moves are tracked automatically with
/// zero extra wiring.
///
/// TEMPORARY: disabled. The post-show capture photographs the sidebar's own
/// transparent pixels — GDI BitBlt does not reliably honor
/// WDA_EXCLUDEFROMCAPTURE — producing black frames that paint the panel
/// pitch black (2026-10-01 live regression). Blur comes from the pre-show
/// `capture_backdrop` (ADR-05). A correct live loop needs DXGI desktop
/// duplication, which is separate future work.
pub(crate) fn spawn_sidebar_live_blur<R: Runtime>(app: &tauri::AppHandle<R>, win: &tauri::WebviewWindow<R>) {
    const DEV_LIVE_BLUR_DISABLED: bool = true; // Disabled for solid opaque styling and 0% CPU overhead
    if DEV_LIVE_BLUR_DISABLED {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        static LIVE_BLUR_ACTIVE: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        static LAST_FRAME_HASH: Mutex<Option<u64>> = Mutex::new(None);
        if LIVE_BLUR_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        LIVE_BLUR_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
        *LAST_FRAME_HASH.lock().unwrap() = None;

        let win_clone = win.clone();
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            // Wait for window to fully appear
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            while win_clone.is_visible().unwrap_or(false) {
                // 4 FPS live blur — shared interval, see sidebar_backdrop.
                tokio::time::sleep(std::time::Duration::from_millis(
                    crate::sidebar_backdrop::LIVE_BLUR_INTERVAL_MS,
                ))
                .await;

                if !win_clone.is_visible().unwrap_or(false) {
                    break;
                }

                let scale = win_clone
                    .scale_factor()
                    .unwrap_or(1.0);
                let Ok(pos) = win_clone.outer_position() else { continue };
                let Ok(logical) = win_clone
                    .inner_size()
                    .map(|s| s.to_logical::<f64>(scale))
                else { continue };
                let phys_w = (logical.width * scale) as i32;
                let phys_h = (logical.height * scale) as i32;

                // Step 1: cheap raw capture for hashing (~1ms)
                let raw_bgra = match crate::sidebar_backdrop::capture_region_bgra_public(pos.x, pos.y, phys_w, phys_h) {
                    Some(bgra) => bgra,
                    None => continue,
                };

                // Step 2: hash and compare to previous frame
                let current_hash = crate::sidebar_backdrop::frame_hash(&raw_bgra);
                let mut prev_hash_guard = LAST_FRAME_HASH.lock().unwrap();
                let should_emit = match *prev_hash_guard {
                    Some(prev) => prev != current_hash,
                    None => true, // First frame after show — always emit
                };
                *prev_hash_guard = Some(current_hash);
                drop(prev_hash_guard);

                // Step 3: only run the pipeline if changed —
                // half-res fast blur keeps 4 FPS affordable.
                if should_emit {
                    if let Some(data_uri) = crate::sidebar_backdrop::blur_bgra_to_jpeg_fast(&raw_bgra, phys_w, phys_h, 32.0) {
                        let _ = app_clone.emit("sidebar:backdrop", data_uri);
                    }
                }
            }

            // Clean up: reset hash so next show captures fresh
            *LAST_FRAME_HASH.lock().unwrap() = None;
            LIVE_BLUR_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, win);
    }
}

/// Prepared unified-sidebar state: window + whether it was already on
/// screen (a visible window must NOT have its backdrop re-captured —
/// that would photograph the sidebar itself) + physical capture geometry.
pub(crate) struct SidebarPrepared<R: Runtime> {
    pub(crate) win: tauri::WebviewWindow<R>,
    pub(crate) already_visible: bool,
    pub(crate) capture_geom: Option<(i32, i32, i32, i32)>,
}

/// Get/create the unified sidebar window and apply the view's geometry
/// (logical size + position from `sidebar_geometry`). Does NOT show it.
/// MUST be awaited from an async context — `get_or_create_window` may
/// build a WebView, and `WebviewWindowBuilder::build()` dispatches to the
/// main thread (a sync command would deadlock there).
/// The whole monitor (position included). Sidebar panels are sized from it, exactly as before the multi-monitor
/// fix: top margin 20, bottom margin 20, full height. Only the ORIGIN is new (monitors left of or above the primary).
fn work_rect(monitor: &tauri::window::Monitor) -> crate::work_area::PRect {
    let (p, s) = (monitor.position(), monitor.size());
    crate::work_area::PRect { x: p.x, y: p.y, w: s.width as i32, h: s.height as i32 }
}

pub(crate) async fn prepare_sidebar<R: Runtime>(
    app: &tauri::AppHandle<R>,
    view: &str,
    dock: Option<&str>,
) -> Result<SidebarPrepared<R>, String> {
    let win = crate::dyn_windows::get_or_create_window(app, crate::dyn_windows::WindowConfig::sidebar())?;
    let already_visible = win.is_visible().unwrap_or(false);

    // Placement comes from the monitor's WORK AREA (origin included, so monitors left of / above the primary and
    // taskbars on any edge are respected) and is set in physical pixels, converted once in `work_area`.
    if let Ok(Some(monitor)) = win.current_monitor().or_else(|_| win.primary_monitor()) {
        let rect = crate::work_area::sidebar_rect(view, dock, work_rect(&monitor), monitor.scale_factor());
        let _ = win.set_size(tauri::PhysicalSize::new(rect.w as u32, rect.h as u32));
        let _ = win.set_position(tauri::PhysicalPosition::new(rect.x, rect.y));
    }

    set_active_sidebar_view(view);
    // Persist for the router's fresh-window race path only after the window
    // has been prepared successfully.
    *PENDING_SIDEBAR_VIEW.lock().unwrap() = Some(view.to_string());

    // Compute physical capture geometry from the logical values we just set.
    // This avoids the race where win.outer_position()/inner_size() haven't
    // propagated yet when capture_backdrop runs immediately after.
    let capture_geom = if let Ok(Some(monitor)) = win.current_monitor().or_else(|_| win.primary_monitor()) {
        let r = crate::work_area::sidebar_rect(view, dock, work_rect(&monitor), monitor.scale_factor());
        Some((r.x, r.y, r.w, r.h))
    } else {
        None
    };

    Ok(SidebarPrepared { win, already_visible, capture_geom })
}

/// Show + focus + re-assert DWM corners (+ macOS vibrancy re-apply) +
/// emit `sidebar:set_view` so the React router switches instantly.
pub(crate) fn finish_sidebar<R: Runtime>(
    app: &tauri::AppHandle<R>,
    win: &tauri::WebviewWindow<R>,
    view: &str,
    dock: Option<&str>,
) -> Result<(), String> {
    win.show().map_err(|e| format!("sidebar show: {e}"))?;
    // Panels beside it (Architecture map, Maps, place card) were arranged around where the sidebar WAS: move them out of
    // its way now that it is placed and visible, so they sit side by side instead of underneath it.
    crate::panel_layout::apply(app);
    let _ = win.set_focus();
    #[cfg(target_os = "windows")]
    {
        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                use windows::Win32::Foundation::HWND;
                let _ = SetForegroundWindow(HWND(hwnd.0 as _));
            }
        }
    }
    #[cfg(target_os = "windows")]
    crate::dwm_corners::round_corners(win);
    #[cfg(target_os = "macos")]
    {
        use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
        let _ = apply_vibrancy(
            win,
            NSVisualEffectMaterial::Sidebar,
            Some(NSVisualEffectState::Active),
            Some(20.0),
        );
    }
    let _ = app.emit(
        "sidebar:set_view",
        serde_json::json!({ "view": view, "dock": dock }),
    );
    Ok(())
}

/// Capture the backdrop (if safe) and emit it as the fast-path
/// `sidebar:backdrop` event. Returns the captured URI so legacy commands
/// can also store it in their pending content (fresh-window race path).
/// When the window is already visible, returns None — capturing then
/// would photograph the sidebar itself.
pub(crate) fn capture_and_emit_backdrop<R: Runtime>(
    app: &tauri::AppHandle<R>,
    prep: &SidebarPrepared<R>,
) -> Option<String> {
    if prep.already_visible {
        return None;
    }
    let uri = capture_backdrop(app, &prep.win, prep.capture_geom);
    if let Some(u) = &uri {
        let _ = app.emit("sidebar:backdrop", u.clone());
    }
    uri
}

/// One-stop unified show: prepare → backdrop (currently disabled in
/// solid-black mode) → show/focus → set_view → live-blur loop (currently
/// disabled). Returns the captured backdrop for legacy pending
/// storage. Used by the new `show_sidebar_view` / `set_sidebar_dock`
/// commands and by every legacy show command below.
pub(crate) async fn unified_show_sidebar<R: Runtime>(
    app: &tauri::AppHandle<R>,
    view: &str,
    dock: Option<&str>,
) -> Result<Option<String>, String> {
    // `prepare_sidebar` persists the requested view for the router's
    // fresh-window race path and tracks the active view.
    let prep = prepare_sidebar(app, view, dock).await?;
    let backdrop = capture_and_emit_backdrop(app, &prep);
    finish_sidebar(app, &prep.win, view, dock)?;
    spawn_sidebar_live_blur(app, &prep.win);
    Ok(backdrop)
}

/// IPC: Show the unified sidebar in a specific view with an optional
/// dock anchor. `view`: "assistant" | "settings" | "architect" | "pr-list".
/// `dock`: Some("left"|"right"|"center") or None (view default).
#[tauri::command]
pub async fn show_sidebar_view<R: Runtime>(
    app: tauri::AppHandle<R>,
    view: String,
    dock: Option<String>,
) -> Result<(), String> {
    unified_show_sidebar(&app, &view, dock.as_deref())
        .await
        .map(|_| ())
}

/// IPC: Re-anchor the unified sidebar to a dock position ("left",
/// "right", or "center"/"float") keeping the currently active view.
#[tauri::command]
pub async fn set_sidebar_dock<R: Runtime>(
    app: tauri::AppHandle<R>,
    dock: String,
) -> Result<(), String> {
    let view = {
        let current = ACTIVE_SIDEBAR_VIEW.lock().unwrap();
        if current.is_empty() { "assistant".to_string() } else { current.clone() }
    };
    let dock_opt = match dock.as_str() {
        "center" | "float" | "" => None,
        other => Some(other.to_string()),
    };
    unified_show_sidebar(&app, &view, dock_opt.as_deref())
        .await
        .map(|_| ())
}

/// IPC: Hide the response sidebar window.
/// Called after the server response has been spoken.
#[tauri::command]
pub fn hide_sidebar<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    // Destroy the unified sidebar window to free its ~250 MB WebView2 tree.
    let _ = crate::dyn_windows::destroy_window(&app, "sidebar");
    crate::panel_layout::apply(&app);
    Ok(())
}

// ─── PR List view (unified sidebar) ──────────────────────────────────

/// IPC: Show the PR list in the unified sidebar window.
/// Right-dock geometry (520×980) comes from `prepare_sidebar`; the
/// backdrop capture + shared live-blur loop give the same liquid-glass
/// appearance as every other view.
#[tauri::command]
pub async fn show_pr_list_sidebar<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    let _ = unified_show_sidebar(&app, "pr-list", None).await?;
    Ok(())
}

/// IPC: Hide (destroy) the PR list sidebar window.
/// With the unified window this closes the whole sidebar panel.
#[tauri::command]
pub fn hide_pr_list_sidebar<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    let _ = crate::dyn_windows::destroy_window(&app, "sidebar");
    crate::panel_layout::apply(&app);
    Ok(())
}

/// IPC: Show the Command Hub (settings view) in the unified sidebar.
/// Default geometry: center floating modal (740×min(900, mh-100)); users
/// can re-dock via the drag bar.
#[tauri::command]
pub async fn show_settings_sidebar<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    // Unified delegate — Command Hub view, default center modal.
    let prep = prepare_sidebar(&app, "settings", None).await?;

    finish_sidebar(&app, &prep.win, "settings", None)?;
    spawn_sidebar_live_blur(&app, &prep.win);

    Ok(())
}

/// IPC: Hide (destroy) the settings sidebar window.
/// With the unified window this closes the whole sidebar panel.
#[tauri::command]
pub fn hide_settings_sidebar<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    let _ = crate::dyn_windows::destroy_window(&app, "sidebar");
    crate::panel_layout::apply(&app);
    Ok(())
}

/// Views an alert may replace without a way back: the plain assistant view and the CodeRabbit view. Any other view
/// (settings, architect, PR list...) is work the owner opened and is restored by Back.
pub fn is_pinned_view(view: &str) -> bool {
    !matches!(view, "assistant" | "coderabbit")
}

/// The sidebar view showing now and whether a notification may treat it as pinned work to restore.
pub fn active_sidebar_view_for_notify<R: Runtime>(app: &tauri::AppHandle<R>) -> (Option<String>, bool) {
    let view = ACTIVE_SIDEBAR_VIEW.lock().map(|v| v.clone()).unwrap_or_default();
    let open = app.get_webview_window("sidebar").is_some();
    if !open || view.is_empty() {
        return (None, false);
    }
    // The plain assistant view and the CodeRabbit view are replaceable by an alert; any other view is work the
    // owner opened and is restored by Back.
    let pinned = is_pinned_view(&view);
    (Some(view), pinned)
}

/// Shows a notification's detail in the existing right-docked sidebar (cold mount reads PENDING_SIDEBAR, a live
/// window receives `sidebar:analysis`). The payload is built in Rust from validated ledger state.
pub async fn show_event_detail<R: Runtime>(app: tauri::AppHandle<R>, payload: serde_json::Value) -> Result<(), String> {
    let window_existed = app.get_webview_window("sidebar").is_some();
    use crate::notify_hub::{DETAIL_DOCK, DETAIL_VIEW};
    let prep = prepare_sidebar(&app, DETAIL_VIEW, Some(DETAIL_DOCK)).await?;
    let backdrop = capture_and_emit_backdrop(&app, &prep);
    {
        let mut pending = PENDING_SIDEBAR.lock().unwrap();
        *pending = Some(PendingSidebar {
            // Must not contain the word "coderabbit": that routes the payload to the PR-list panel instead.
            query: "GitHub alert".to_string(),
            text: String::new(),
            backdrop,
            analysis: Some(payload.clone()),
            confirmation: None,
        });
    }
    finish_sidebar(&app, &prep.win, DETAIL_VIEW, Some(DETAIL_DOCK))?;
    spawn_sidebar_live_blur(&app, &prep.win);
    if window_existed {
        let _ = app.emit("sidebar:analysis", serde_json::json!({ "query": "GitHub alert", "text": "", "analysis": payload }));
    }
    Ok(())
}

/// Opens the CodeRabbit panel straight on one pull request's review (used when its CI fails).
pub async fn show_coderabbit_pr<R: Runtime>(
    app: tauri::AppHandle<R>,
    repo: String,
    pr_number: u32,
) -> Result<(), String> {
    let review = crate::coderabbit::coderabbit_review_pr_data(pr_number, Some(repo.clone())).await?;
    let payload = serde_json::to_value(&review).map_err(|e| e.to_string())?;
    let window_existed = app.get_webview_window("sidebar").is_some();
    let prep = prepare_sidebar(&app, "coderabbit", Some("center")).await?;
    let backdrop = capture_and_emit_backdrop(&app, &prep);
    {
        let mut pending = PENDING_SIDEBAR.lock().unwrap();
        *pending = Some(PendingSidebar {
            query: "CodeRabbit review".to_string(),
            text: format!("CodeRabbit review for PR {pr_number} in {repo}."),
            backdrop,
            analysis: Some(payload.clone()),
            confirmation: None,
        });
    }
    finish_sidebar(&app, &prep.win, "coderabbit", Some("center"))?;
    spawn_sidebar_live_blur(&app, &prep.win);
    if window_existed {
        let _ = app.emit("sidebar:analysis", payload);
    }
    Ok(())
}

/// Show the CodeRabbit active pull requests hub (595 x 975 panel, centred).
pub async fn show_coderabbit_hub<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    let window_existed = app.get_webview_window("sidebar").is_some();
    let prep = prepare_sidebar(&app, "coderabbit", Some("center")).await?;
    let backdrop = capture_and_emit_backdrop(&app, &prep);

    // Real open PRs of the repo with the most open work; an unreachable GitHub shows an empty list.
    let repo = crate::github_repo_index::entries()
        .await
        .into_iter()
        .max_by_key(|e| e.open_issues)
        .map(|e| e.full_name);
    let active_prs = match &repo {
        Some(r) => crate::coderabbit::list_active_prs(r).await.unwrap_or_default(),
        None => Vec::new(),
    };
    let payload = serde_json::json!({
        "mode": "pr_list",
        "repo": repo,
        "activePrs": active_prs
    });

    {
        let mut pending = PENDING_SIDEBAR.lock().unwrap();
        *pending = Some(PendingSidebar {
            query: "CodeRabbit AI — Active Pull Requests".to_string(),
            text: "Here are the open pull requests, sir.".to_string(),
            backdrop: backdrop.clone(),
            analysis: Some(payload.clone()),
            confirmation: None,
        });
    }

    finish_sidebar(&app, &prep.win, "coderabbit", Some("center"))?;
    spawn_sidebar_live_blur(&app, &prep.win);

    if window_existed {
        let _ = app.emit("sidebar:analysis", payload);
    }

    Ok(())
}

// ─── Loading indicator (stage-hosted, see orchestrator::show_loading/
// hide_loading and window_manager::emit_loading_rect) ──────────────────

// ─── Settings window + persistence ───────────────────────────────────

/// IPC: Open the settings window.
/// Creates the window on-demand if it doesn't exist (saves ~250 MB RAM at idle).
#[tauri::command]
// WebView2 creation must run off the Windows synchronous IPC/UI thread.
pub async fn open_settings_window<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    let win = crate::dyn_windows::get_or_create_window(&app, crate::dyn_windows::WindowConfig::settings())?;
    let _ = win.set_always_on_top(true);
    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// IPC: Close/hide the settings window.
#[tauri::command]
pub fn close_settings_window<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    // Destroy to free ~250 MB of WebView2 processes.
    let _ = crate::dyn_windows::destroy_window(&app, "settings");
    Ok(())
}

/// IPC: Open the Orb Studio customization window.
/// Spawns directly to the left of the Settings sidebar without closing Settings.
/// Width is 800px (twice Settings' 400px width).
#[tauri::command]
pub async fn open_orb_studio_window<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    let win = crate::dyn_windows::get_or_create_window(&app, crate::dyn_windows::WindowConfig::orb_studio())?;

    // Position directly to the left of the settings window
    if let Ok(monitors) = win.available_monitors() {
        if let Some(primary) = monitors.first() {
            let size = primary.size();
            let scale = primary.scale_factor();
            let mw = size.width as f64 / scale;
            let mh = size.height as f64 / scale;

            let h = (mh - 40.0).min(1040.0).max(400.0);
            let studio_w = 800.0;
            // Settings right dock x = mw - 400 - 10 = mw - 410.
            // Studio sits 12px to the left of settings: x = mw - 410 - 800 - 12 = mw - 1222.
            let studio_x = (mw - 1222.0).max(10.0);
            let studio_y = 20.0;

            let _ = win.set_size(tauri::LogicalSize::new(studio_w, h));
            let _ = win.set_position(tauri::LogicalPosition::new(studio_x, studio_y));
        }
    }

    win.show().map_err(|e| e.to_string())?;
    win.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// IPC: Close/destroy the Orb Studio window.
#[tauri::command]
pub fn close_orb_studio_window<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<(), String> {
    let _ = crate::dyn_windows::destroy_window(&app, "orb-studio");
    Ok(())
}

/// IPC: Save custom orb morphology selection and broadcast to all windows immediately.
#[tauri::command]
pub fn set_orb_morphology<R: Runtime>(
    app: tauri::AppHandle<R>,
    think: u32,
    speak: u32,
    listen: u32,
) -> Result<(), String> {
    let mut settings = get_settings(app.clone())?;
    settings.orb_think_mode = think;
    settings.orb_speak_mode = speak;
    settings.orb_listen_mode = listen;
    save_settings(app.clone(), settings.clone())?;
    let _ = app.emit("orb:morphology_changed", serde_json::json!({
        "think": think,
        "speak": speak,
        "listen": listen,
    }));
    Ok(())
}

/// IPC: Save custom orb capsule style ("classic" | "pebble") and broadcast immediately.
#[tauri::command]
pub fn set_orb_capsule_style<R: Runtime>(
    app: tauri::AppHandle<R>,
    style: String,
    mode: Option<u32>,
    genesis: Option<u32>,
) -> Result<(), String> {
    let mut settings = get_settings(app.clone())?;
    settings.orb_capsule_style = style.clone();
    if let Some(m) = mode { settings.orb_wakeup_mode = m; }
    if let Some(g) = genesis { settings.orb_interior_genesis = g; }
    save_settings(app.clone(), settings.clone())?;
    let _ = app.emit("orb:capsule_style", serde_json::json!({
        "style": style,
        "mode": settings.orb_wakeup_mode,
        "genesis": settings.orb_interior_genesis,
    }));
    Ok(())
}

/// IPC: Play one wake-sound candidate (Command Hub preview / audition). `style` is rise | glass | pluck |
/// triplet | sweep; `volume` 0-100.
#[tauri::command]
pub fn preview_wake_sound(style: String, volume: u8) -> Result<(), String> {
    crate::wake_cue::play_preview(&style, volume);
    Ok(())
}

/// IPC: Trigger test wakeup animation on desktop.
#[tauri::command]
pub fn test_wake_orb<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    crate::window_manager::wake_orb_src(&app, "preview-wake");
    Ok(())
}

/// Serialized settings returned by `get_settings` and accepted by `save_settings`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NexusSettings {
    pub autostart: bool,
    pub hotkey: String,
    pub auto_hide_delay: u32,
    pub wake_word_enabled: bool,
    pub wake_phrase: String,
    pub wake_sensitivity: String,
    pub speaker_verification: bool,
    pub meeting_mode_auto: bool,
    pub suppress_tts_in_meetings: bool,
    pub local_stt_only: bool,
    pub server_url: String,
    pub user_id: String,
    pub device_id: String,
    pub tts_voice: String,
    pub speech_rate: f64,
    #[serde(default = "default_tts_provider")]
    pub tts_provider: String,
    /// TTS auto-volume: NEXUS sets system volume to this level (0-100)
    /// before speaking, then restores the original volume after.
    /// Default 75. Set to 0 to disable auto-volume.
    #[serde(default = "default_tts_volume")]
    pub tts_volume: u8,
    /// Groq API key for cloud STT (Whisper Large v3 Turbo).
    /// Free at console.groq.com — 2,000 requests/day per user.
    /// When empty, NEXUS uses local faster-whisper STT.
    #[serde(default)]
    pub groq_api_key: String,
    /// edge-tts voice name (Microsoft Neural, free, no API key).
    /// Default: en-US-AvaNeural. See full list at Microsoft Speech docs.
    #[serde(default = "default_edge_tts_voice")]
    pub edge_tts_voice: String,
    /// Iconic voice persona key (Feature 83 catalog, e.g. "jarvis").
    /// Default: "nexus". Written by set_voice_preference.
    #[serde(default = "default_selected_voice")]
    pub selected_voice: String,
    /// Local Kokoro voice name for the selected persona (e.g. "bm_george").
    /// Default: af_heart. Written by set_voice_preference; resolved by the swap worker.
    #[serde(default = "default_offline_voice_model")]
    pub offline_voice_model: String,
    /// Orb horizontal position as percentage (0.0 = left, 0.5 = center, 1.0 = right).
    /// Default 0.5 (center). Saved to settings.json, persists across restarts.
    #[serde(default = "default_orb_horizontal_pct")]
    pub orb_horizontal_pct: f64,
    /// Orb vertical position as percentage (0.0 = top, 1.0 = bottom).
    /// Default 1.0 (bottom). Saved to settings.json, persists across restarts.
    #[serde(default = "default_orb_vertical_pct")]
    pub orb_vertical_pct: f64,
    /// Orb window size in pixels (100-400).
    /// Default 200. Saved to settings.json, persists across restarts.
    #[serde(default = "default_orb_size")]
    pub orb_size: u32,
    /// Orb color theme (hex string, e.g. "#f2b859").
    /// Default: "#f2b859" (golden amber).
    #[serde(default = "default_orb_color")]
    pub orb_color: String,
    /// Orb fixed position: "top" or "bottom".
    /// Default: "top".
    #[serde(default = "default_orb_position")]
    pub orb_position: String,
    /// Orb background capsule style: "classic" or "pebble".
    /// Default: "classic".
    #[serde(default = "default_orb_capsule_style")]
    pub orb_capsule_style: String,
    /// Orb wakeup animation mode (1..5): 1=elastic thread, 2=bowstring, 3=liquid meniscus, 4=horizon, 5=acoustic.
    #[serde(default = "default_orb_wakeup_mode")]
    pub orb_wakeup_mode: u32,
    /// Orb interior genesis mode (0..3): 0=bloom, 1=droplet roll, 2=spark burst, 3=helix.
    #[serde(default = "default_orb_interior_genesis")]
    pub orb_interior_genesis: u32,
    /// Scoop width in px.
    #[serde(default = "default_orb_scoop_width")]
    pub orb_scoop_width: u32,
    /// Scoop depth in px.
    #[serde(default = "default_orb_scoop_height")]
    pub orb_scoop_height: u32,
    /// Wing contact highlight opacity (0..100%).
    #[serde(default = "default_orb_wing_rim_opacity")]
    pub orb_wing_rim_opacity: u32,
    /// Perimeter rim base opacity (0..100%).
    #[serde(default = "default_orb_base_rim_opacity")]
    pub orb_base_rim_opacity: u32,
    /// Wing contact glow width in px.
    #[serde(default = "default_orb_wing_glow_spread")]
    pub orb_wing_glow_spread: u32,
    /// Elastic spring stiffness factor * 100.
    #[serde(default = "default_orb_stiffness")]
    pub orb_stiffness: u32,
    /// Orb thinking morphology mode (1..12).
    /// Default: 6 (Trefoil Celtic Helix Cage).
    #[serde(default = "default_orb_think_mode")]
    pub orb_think_mode: u32,
    /// Orb speaking morphology mode (1..6).
    /// Default: 2 (64-strand Fibonacci Blossom).
    #[serde(default = "default_orb_speak_mode")]
    pub orb_speak_mode: u32,
    /// Orb listening morphology mode (1..4).
    /// Default: 1 (Acoustic Sand Cymatics & Inward Vortex).
    #[serde(default = "default_orb_listen_mode")]
    pub orb_listen_mode: u32,
    /// Gemini API key for Google Gemini models.
    /// When empty, Gemini features are unavailable.
    #[serde(default)]
    pub gemini_api_key: String,
    /// Cerebras API key for Cerebras-hosted Llama models.
    /// Free at cloud.cerebras.ai — 1M tokens/day. Fastest inference (~80ms).
    /// When empty, Cerebras is skipped in the 9Router cascade.
    #[serde(default)]
    pub cerebras_api_key: String,
    /// Deepgram API key for Nova-2 cloud streaming STT.
    /// When empty, NEXUS uses Groq Whisper Large v3 Turbo ($0 free tier).
    #[serde(default)]
    pub deepgram_api_key: String,
    /// Google Routes API key (spoken ETA for "directions to ..."). Optional.
    #[serde(default)]
    pub routes_api_key: String,
    /// Google Places API (New) key ("nearest pharmacy"). Optional.
    #[serde(default)]
    pub places_api_key: String,
    /// Logo.dev publishable key (`pk_...`) for brand logos (img.logo.dev).
    /// Optional; without it brand-logo lookups return nothing (no network).
    #[serde(default)]
    pub logo_dev_publishable_key: String,
    /// Moonshine STT model architecture.
    /// Options: "small_streaming" (123M, 7.84% WER, default for family),
    ///          "medium_streaming" (245M, 6.65% WER, ~400 MB — opt-in),
    ///          "tiny_streaming" (34M, 12% WER, ultra-low RAM).
    /// Default: "small_streaming" — fits the 600–700 MB local-usage budget.
    #[serde(default = "default_moonshine_model")]
    pub moonshine_model: String,
    /// Stage-2 wake-word verifier: on a stage-1 (acoustic) candidate, run the
    /// buffered audio through STT and wake only if the transcript contains
    /// "nexus". Kills TV/conversation false wakes at +~250ms latency.
    /// Default: true. Set false in settings.json to restore fire-on-detect.
    #[serde(default = "default_verify_wake")]
    pub verify_wake: bool,
    /// Speech-check policy: "always" (every candidate is checked, the default), "tiered" (candidates the
    /// classifier is very sure about, `wake_verify_high`, fire without the check) or "off". Empty = follow
    /// the legacy `verify_wake` boolean. Choose with `wake_calibrate.py --verify` on your clips.
    #[serde(default)]
    pub wake_verify_mode: String,
    /// Tiered mode: stage-1 probability at or above which the speech check is skipped (0.80-0.9999).
    #[serde(default = "default_wake_verify_high")]
    pub wake_verify_high: f32,
    /// Mic keep-alive render (Meet parity): inaudible output stream beside
    /// capture holds the Intel DSP awake so it never power-gates the mic
    /// path mid-session. Default: true. Silent zeros, ~0 CPU.
    #[serde(default = "default_mic_keep_alive")]
    pub mic_keep_alive: bool,
    /// TTS voice emotion: "auto" (default, heuristics per reply),
    /// "neutral", "cheerful", "calm", "sad", "urgent", "whisper".
    /// Prosody mapped to edge-tts rate/pitch/volume.
    #[serde(default = "default_tts_emotion")]
    pub tts_emotion: String,
    /// Ghost vision provider order: "auto" (default, Gemini — sole vision
    /// engine since Groq decommissioned vision 2026-10-01), "groq"
    /// (kept for forward-compat; currently always fails), or "gemini".
    #[serde(default = "default_vision_provider")]
    pub vision_provider: String,
    /// Ghost vision strategy: "sequential" (default, quota-frugal) or
    /// "speed" (race both providers in parallel, first valid wins —
    /// halves worst-case latency, spends 2 quota units per miss).
    #[serde(default = "default_vision_race")]
    pub vision_race: String,
    /// Telegram owner chat id for the owner-only remote bridge (see
    /// telegram.rs). Empty = bridge off. The bot token itself lives in the
    /// vault ("telegram" service), never here. Set from the Connections tab.
    #[serde(default)]
    pub telegram_chat_id: String,
    /// Ghost FIFO queue: speak "Queued, sir." once per session when queue
    /// depth reaches 2+. Default: true. Plans D5/D6.
    #[serde(default = "default_ghost_depth_ack")]
    pub ghost_depth_ack: bool,
    /// Drop Groq transcripts whose own segment statistics (avg_logprob /
    /// compression_ratio) mark them as unreliable. Off by default: enable it
    /// only after `voice_scorecard.py` shows a threshold that catches
    /// hallucinations without rejecting your real commands.
    #[serde(default)]
    pub stt_confidence_gate: bool,
    /// Send generic NEXUS command hints (`COMMAND_VOCABULARY`) to Groq on turns
    /// that are not owner-verified. Off by default; A/B it with the scorecard.
    #[serde(default)]
    pub stt_command_hints: bool,
    /// Where the command capture starts, in ms relative to the end of the
    /// chunk that detected the wake word. 0 = right after the detection point
    /// (audio spoken while verifying/starting up is kept; the wake word is not).
    /// Negative values reach back into the wake word's tail — experimental;
    /// choose with `stt_eval --capture-offset-ms`. Clamped to ±400.
    #[serde(default)]
    pub wake_seed_offset_ms: i32,
    /// When Voice Lock already confirmed the owner's voice on a wake candidate, skip the cloud speech check:
    /// the local wake model plus the local voice match decide. Default OFF: the owner's own speech that merely sounds like
    /// the name passed that gate and woke NEXUS (2026-10-31 log), so the cloud speech check stays the default.
    #[serde(default)]
    pub wake_owner_fast_path: bool,
    /// Show maps inside a NEXUS panel (Google's embed). Off = always use the old Maps window.
    #[serde(default = "default_true")]
    pub maps_embed: bool,
    /// Where the place card gets distance and travel time: "auto" (your routing server if one is set, else your Google
    /// key, else the open service), "server", "google" or "open".
    #[serde(default = "default_routing_mode")]
    pub routing_mode: String,
    /// Address of your own routing server (see server/routing). Empty = none.
    #[serde(default)]
    pub routing_server_url: String,
    /// Wake-word front-end chain: "legacy" (the original runtime chain, default)
    /// or "aligned" (identical to the chain the classifier was trained with).
    /// Compare with `wake_eval --dsp` on your recordings before switching.
    #[serde(default = "default_wake_dsp_profile")]
    pub wake_dsp_profile: String,
    /// Ghost FIFO queue: per-step watchdog timeout in ms. Default: 15000. D7.
    #[serde(default = "default_ghost_step_timeout_ms")]
    pub ghost_step_timeout_ms: u64,
    /// Ghost FIFO queue: inter-command gap τ in ms. Default: 1000. D4.
    #[serde(default = "default_ghost_turn_gap_ms")]
    pub ghost_turn_gap_ms: u64,
    /// OPT-IN open-mic engagement window in seconds: outside it a Ghost hot-mic turn needs the word
    /// "Nexus". 0 = off (the default: Ghost mode must not make the user say "Nexus" every time).
    #[serde(default = "default_ghost_engagement_secs")]
    pub ghost_engagement_secs: u64,
    /// Directed-speech gate for the Ghost open mic (default on).
    #[serde(default = "default_true")]
    pub directed_gate: bool,
    /// Hand mode (camera pointer) may be started by voice. The camera is only ever on while the mode is.
    #[serde(default = "default_true")]
    pub hand_mode: bool,
    /// Hand mode is read-only: decisions are only drawn on the overlay and never reach the real mouse.
    /// Off (default): arming with palm-fist-palm-fist is the confirmation and the cursor follows the hand.
    /// Voice: "hand mouse preview" / "hand mouse live" (this session only).
    #[serde(default)]
    pub hand_mouse_preview: bool,
    /// Debug: show the camera picture and skeleton in a small window. Off (default): nothing visible.
    #[serde(default)]
    pub hand_show_camera: bool,
    /// Where the hand window gets camera frames: "auto" (track reader if available), "track", "rvfc".
    #[serde(default = "default_hand_frame_source")]
    pub hand_frame_source: String,
    /// Record landmark numbers (no video) to hand_trace.jsonl for tuning.
    #[serde(default)]
    pub hand_trace: bool,
    /// Start hand mode together with Ghost mode (ends with it). Default off.
    #[serde(default)]
    pub hand_with_ghost: bool,
    /// A hand click just outside a control snaps to the nearest control. Default on.
    #[serde(default = "default_true")]
    pub hand_snap: bool,
    /// Touchpad-style gestures with 3 fingers: flick = next/previous tab, flick up = window switcher. Default on.
    #[serde(default = "default_true")]
    pub hand_gestures: bool,
    /// Pause unneeded background work while Hand Mode runs. Default on.
    #[serde(default = "default_true")]
    pub hand_focus_mode: bool,
    /// Lower other heavy apps while Hand Mode runs. Default on.
    #[serde(default = "default_true")]
    pub hand_focus_lower_apps: bool,
    /// 4 fingers together: virtual desktops, Task View, show desktop. Default off until the finger-spread
    /// threshold has been checked on the user's own hand (see the `[HAND] pose ... spread=` log).
    #[serde(default)]
    pub hand_gestures_desktop: bool,
    /// Pointer speed multiplier for hand mode. 0 (default) = follow the Windows pointer speed setting.
    #[serde(default)]
    pub hand_sensitivity: f64,
    /// Ghost waves placement (ghost sessions reposition the orb window to
    /// this rect — the waves own the visual then). Defaults = orb defaults.
    #[serde(default)]
    pub waves_horizontal_pct: f64,
    #[serde(default = "default_waves_vertical_pct")]
    pub waves_vertical_pct: f64,
    #[serde(default = "default_waves_size")]
    pub waves_size: u32,
    /// Loading indicator placement (center-anchored fractions + logical px).
    /// Defaults ≈ the historical top-right corner.
    #[serde(default = "default_loading_horizontal_pct")]
    pub loading_horizontal_pct: f64,
    #[serde(default = "default_loading_vertical_pct")]
    pub loading_vertical_pct: f64,
    #[serde(default = "default_loading_size")]
    pub loading_size: u32,
    /// Narrated screen tour ("analyse my screen"): pointers + callouts synced
    /// to TTS. Default true; false = legacy spatial/OCR chain only. Must live
    /// in this struct — `save_settings` rewrites settings.json from it, so a
    /// hand-added key outside the struct would be erased on the next save.
    #[serde(default = "default_screen_tour")]
    pub screen_tour: bool,
    /// Log Whisper's per-segment confidence (`avg_logprob`, `no_speech_prob`) for every
    /// command turn as `[STT-CONF]` lines (P0 measurement for docs/features/103). Default off:
    /// it switches the Groq request to `verbose_json` (same model, larger response).
    /// Must live in this struct or `save_settings` erases it.
    #[serde(default)]
    pub stt_log_confidence: bool,
    /// Opt-in encrypted native STT clips and turn trace, retained for 24 h.
    #[serde(default)]
    pub stt_diagnostics: bool,
    /// Vision model for the screen tour (empty = built-in ladder:
    /// gemini-3.8-flash → gemini-3.5-flash → gemini-3.5-flash-lite → 2.5).
    #[serde(default)]
    pub tour_model: String,
    /// Daily Gemini vision request cap NEXUS enforces for itself (shared by
    /// ghost clicks, spatial analysis and the tour). Google's real per-model
    /// free limits are only visible in AI Studio (aistudio.google.com/rate-limit);
    /// enter that figure here. Default 500.
    #[serde(default = "default_gemini_vision_daily_limit")]
    pub gemini_vision_daily_limit: u32,
    /// Personality tone: "butler" (default — formal, "sir") or "friend"
    /// (first-name, contractions, situational lightness, honest counsel).
    /// Voice-switchable ("talk like a friend" / "be formal"). Must live
    /// in this struct — `save_settings` rewrites settings.json from it.
    #[serde(default = "default_persona_mode")]
    pub persona_mode: String,
    /// Command Hub theme: "dark" (default, opaque) or "light" (opaque).
    /// Transparency is out (user directive) — both themes are solid.
    /// Must live in this struct — `save_settings` rewrites settings.json.
    #[serde(default = "default_theme_mode")]
    pub theme_mode: String,
    /// Memory Core (SQLite, ranked recall) on/off. Off = legacy JSON files
    /// only. Must live in this struct — `save_settings` rewrites settings.json.
    #[serde(default = "default_memcore")]
    pub memcore: bool,
    /// Swap locally known people's names for "Person A/B…" in text sent to
    /// cloud models (and back in replies). Off by default.
    #[serde(default)]
    pub memcore_redact_names: bool,
    /// Record which window the user is working in (30 s samples, local only,
    /// 7-day ring) so NEXUS can say where they left off. Voice/Memory-page switch.
    #[serde(default = "default_memcore")]
    pub memcore_activity: bool,
    /// Once-a-day spoken "where you left off" shortly after startup.
    #[serde(default = "default_memcore")]
    pub memcore_briefing: bool,
    /// Send the always-on profile (facts the user told NEXUS, minus sensitive topics) with every
    /// cloud request. Memory page switch; the egress log shows exactly what was sent.
    #[serde(default = "default_memcore")]
    pub memcore_profile: bool,
    /// Learn the owner's name from the connected Google account (fills a gap only; never overwrites).
    #[serde(default = "default_memcore")]
    pub memcore_account_seed: bool,
    /// Play a short "I heard you" sound the instant NEXUS wakes (wake word or Ctrl+Space).
    #[serde(default = "default_memcore")]
    pub wake_sound: bool,
    /// Which sound: rise | glass | pluck | triplet | sweep.
    #[serde(default = "default_wake_sound_style")]
    pub wake_sound_style: String,
    /// Wake sound level, 0-100 (on top of the system volume).
    #[serde(default = "default_wake_sound_volume")]
    pub wake_sound_volume: u8,
    /// Notice command sequences the user repeats and offer to save them as a one-phrase routine.
    #[serde(default = "default_memcore")]
    pub memcore_routines: bool,
    /// Unattended shutdown after a watched coding session ends perfectly: "off" (default; nothing is ever powered off
    /// by itself), "dry" (practice: says what it would do, never does it) or "on" (a 30 second cancellable countdown).
    /// Anything else is read as "off". Changed by voice ("allow unattended shutdown").
    #[serde(default = "default_watch_power")]
    pub watch_power: String,
    /// Let a free cloud model read a redacted copy of recent cloud conversations and SUGGEST profile facts.
    /// Off by default; nothing is stored until the user approves it on the Memory page.
    #[serde(default)]
    pub memcore_review: bool,
    /// Timetable slot reminders ("It's time for DSA. Shall I start?").
    #[serde(default = "default_memcore")]
    pub memcore_timetable: bool,
    /// Watch Gmail for important mail (exams, hackathons, GitHub, database
    /// notices, deadlines) and read today's calendar. Runs only while a
    /// Google account with the right access is connected.
    #[serde(default = "default_memcore")]
    pub memcore_mail: bool,
    /// Watch GitHub Actions runs for recently pushed repositories.
    #[serde(default = "default_memcore")]
    pub memcore_github_watch: bool,
    /// Watch latest production deploys (Vercel, Render, Netlify) and announce failure/recovery.
    #[serde(default = "default_memcore")]
    pub memcore_deploy_watch: bool,
    /// Open the CodeRabbit panel on a PR whose CI just failed.
    #[serde(default = "default_memcore")]
    pub coderabbit_auto_open: bool,
    /// Tell the owner about GitHub failures/recoveries with a Windows notification-center toast (default on).
    #[serde(default = "default_memcore")]
    pub notify_native: bool,
    /// Failure toasts ask Windows to show them even while Do Not Disturb is on (Windows asks the owner once).
    #[serde(default = "default_memcore")]
    pub notify_urgent: bool,
    /// Start a FREE CodeRabbit review automatically when an alert opens (default OFF: NEXUS asks first and shows the count).
    #[serde(default)]
    pub coderabbit_auto_review: bool,
    /// Speak important emails in plain words (GitHub on device; unusual emails
    /// via a free cloud model using sender + subject only). Default OFF: the cloud
    /// step is opt-in; on-device wording (GitHub + categories) always runs.
    #[serde(default)]
    pub memcore_mail_plain: bool,
    /// Read-only WhatsApp watcher (priority people). OFF until the
    /// read-receipt check in docs/testing/whatsapp-read-receipt-test.md is done.
    #[serde(default)]
    pub memcore_whatsapp: bool,
    /// Speak WhatsApp message text aloud. OFF by default: the voice service is
    /// a cloud service and would receive the text (the sidebar card is local).
    #[serde(default)]
    pub memcore_whatsapp_speak: bool,
    /// "Time to leave for ..." reminders from calendar + traffic + remembered home.
    #[serde(default = "default_memcore")]
    pub memcore_leave_by: bool,
    /// Remember which apps/folders/browser windows are open so NEXUS can offer
    /// to reopen them after a restart. Must live in this struct (save_settings rewrites the file).
    #[serde(default = "default_memcore")]
    pub memcore_session: bool,
    /// Play the cached greeting as soon as the desktop is shown at logon, before the app has finished starting.
    /// Must live in this struct (save_settings rewrites the file). Read early from settings.json by `early_hello`.
    #[serde(default = "default_early_hello")]
    pub early_hello: bool,
    /// Check the connections NEXUS relies on at startup and mention problems in the greeting.
    #[serde(default = "default_memcore")]
    pub memcore_health: bool,
    /// Voice power commands ("shut down my laptop"). Kill switch.
    #[serde(default = "default_memcore")]
    pub power_commands: bool,
    /// Default music player: "spotify" (default) or "apple_music".
    #[serde(default = "default_music_player")]
    pub default_music_player: String,
    /// Extra unknown settings preserved across saves so frontend/plugins never get wiped.
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

fn default_music_player() -> String {
    "spotify".to_string()
}

fn default_watch_power() -> String {
    "off".to_string()
}

fn default_memcore() -> bool {
    true
}

fn default_early_hello() -> bool {
    true
}

fn default_persona_mode() -> String {
    "butler".to_string()
}

fn default_theme_mode() -> String {
    "dark".to_string()
}

fn default_screen_tour() -> bool {
    true
}

fn default_gemini_vision_daily_limit() -> u32 {
    crate::vision::GEMINI_VISION_RPD
}

fn default_tts_provider() -> String {
    "kokoro".to_string()
}

fn default_tts_volume() -> u8 {
    75
}

fn default_wake_sound_style() -> String {
    "rise".to_string()
}

fn default_wake_sound_volume() -> u8 {
    crate::wake_cue::DEFAULT_VOLUME
}

fn default_edge_tts_voice() -> String {
    "en-GB-RyanNeural".to_string()
}

fn default_orb_horizontal_pct() -> f64 {
    0.5
}

fn default_orb_vertical_pct() -> f64 {
    0.0
}

fn default_orb_size() -> u32 {
    200
}

fn default_moonshine_model() -> String {
    crate::lazy_stt::DEFAULT_MOONSHINE_MODEL.to_string()
}

fn default_verify_wake() -> bool {
    true
}

fn default_wake_verify_high() -> f32 {
    crate::wakeword_oww::DEFAULT_VERIFY_HIGH
}

fn default_mic_keep_alive() -> bool {
    true
}

fn default_tts_emotion() -> String {
    "auto".to_string()
}

fn default_selected_voice() -> String {
    crate::voice_catalog::DEFAULT_VOICE_KEY.to_string()
}

fn default_offline_voice_model() -> String {
    crate::voice_catalog::FALLBACK_KOKORO_VOICE.to_string()
}

fn default_vision_provider() -> String {
    "auto".to_string()
}

fn default_vision_race() -> String {
    "sequential".to_string()
}

fn default_wake_dsp_profile() -> String {
    "legacy".to_string()
}

fn default_ghost_depth_ack() -> bool {
    true
}

fn default_ghost_step_timeout_ms() -> u64 {
    15000
}

fn default_ghost_turn_gap_ms() -> u64 {
    1000
}

fn default_true() -> bool {
    true
}

fn default_hand_frame_source() -> String {
    "auto".to_string()
}

fn default_ghost_engagement_secs() -> u64 {
    0
}

fn default_waves_vertical_pct() -> f64 {
    0.0
}

fn default_waves_size() -> u32 {
    200
}

fn default_loading_horizontal_pct() -> f64 {
    0.95
}

fn default_loading_vertical_pct() -> f64 {
    0.05
}

fn default_loading_size() -> u32 {
    80
}

fn default_orb_color() -> String {
    "#f2b859".to_string()
}

fn default_orb_position() -> String {
    "top".to_string()
}

fn default_orb_capsule_style() -> String {
    "pebble".to_string()
}

fn default_orb_wakeup_mode() -> u32 {
    1
}

fn default_orb_interior_genesis() -> u32 {
    2
}

fn default_orb_scoop_width() -> u32 {
    301
}

fn default_orb_scoop_height() -> u32 {
    105
}

fn default_orb_wing_rim_opacity() -> u32 {
    0
}

fn default_orb_base_rim_opacity() -> u32 {
    25
}

fn default_orb_wing_glow_spread() -> u32 {
    120
}

fn default_orb_stiffness() -> u32 {
    103
}

fn default_orb_think_mode() -> u32 {
    6
}

fn default_orb_speak_mode() -> u32 {
    2
}

fn default_orb_listen_mode() -> u32 {
    1
}

/// `localSttOnly` when the setting is absent. ONE value for the struct default
/// and every raw-JSON reader (they used to disagree: the Settings UI showed
/// `true` while the runtime read `false`, so a user who only pasted a Groq key
/// saw "Local STT only" ON — and, once any save persisted it, had the key
/// silently ignored). `false`: a key the user entered is used; with no key the
/// local engine is used regardless.
pub const DEFAULT_LOCAL_STT_ONLY: bool = false;

/// Read `localSttOnly` from parsed settings JSON (camelCase or snake_case).
pub fn local_stt_only_from_json(json: &serde_json::Value) -> bool {
    json.get("localSttOnly")
        .or_else(|| json.get("local_stt_only"))
        .and_then(|v| v.as_bool())
        .unwrap_or(DEFAULT_LOCAL_STT_ONLY)
}

impl Default for NexusSettings {
    fn default() -> Self {
        Self {
            autostart: true,
            hotkey: "Ctrl+Space".to_string(),
            auto_hide_delay: 8,
            wake_word_enabled: true,
            wake_phrase: "NEXUS".to_string(),
            wake_sensitivity: "medium".to_string(),
            speaker_verification: false,
            meeting_mode_auto: true,
            suppress_tts_in_meetings: true,
            local_stt_only: DEFAULT_LOCAL_STT_ONLY,
            server_url: option_env!("NEXUS_SERVER_URL")
                .unwrap_or("https://nexus-worker.chitkullakshya.workers.dev")
                .to_string(),
            user_id: String::new(),
            device_id: String::new(),
            tts_voice: "af_sky".to_string(),
            speech_rate: 1.15,
            tts_provider: "kokoro".to_string(),
            tts_volume: 75,
            groq_api_key: String::new(),
            edge_tts_voice: "en-US-AvaNeural".to_string(),
            selected_voice: default_selected_voice(),
            offline_voice_model: default_offline_voice_model(),
            orb_horizontal_pct: 0.5,
            orb_vertical_pct: 0.0,
            orb_size: 200,
            orb_color: default_orb_color(),
            orb_position: default_orb_position(),
            orb_capsule_style: default_orb_capsule_style(),
            orb_wakeup_mode: default_orb_wakeup_mode(),
            orb_interior_genesis: default_orb_interior_genesis(),
            orb_scoop_width: default_orb_scoop_width(),
            orb_scoop_height: default_orb_scoop_height(),
            orb_wing_rim_opacity: default_orb_wing_rim_opacity(),
            orb_base_rim_opacity: default_orb_base_rim_opacity(),
            orb_wing_glow_spread: default_orb_wing_glow_spread(),
            orb_stiffness: default_orb_stiffness(),
            orb_think_mode: default_orb_think_mode(),
            orb_speak_mode: default_orb_speak_mode(),
            orb_listen_mode: default_orb_listen_mode(),
            gemini_api_key: String::new(),
            cerebras_api_key: String::new(),
            deepgram_api_key: String::new(),
            routes_api_key: String::new(),
            places_api_key: String::new(),
            logo_dev_publishable_key: String::new(),
            moonshine_model: crate::lazy_stt::DEFAULT_MOONSHINE_MODEL.to_string(),
            tts_emotion: "auto".to_string(),
            vision_provider: "auto".to_string(),
            vision_race: "sequential".to_string(),
            verify_wake: true,
            wake_verify_mode: String::new(),
            wake_verify_high: default_wake_verify_high(),
            mic_keep_alive: true,
            telegram_chat_id: String::new(),
            ghost_depth_ack: true,
            stt_confidence_gate: false,
            stt_command_hints: false,
            wake_seed_offset_ms: 0,
            wake_owner_fast_path: false,
            maps_embed: true,
            routing_mode: default_routing_mode(),
            routing_server_url: String::new(),
            wake_dsp_profile: default_wake_dsp_profile(),
            ghost_step_timeout_ms: 15000,
            ghost_turn_gap_ms: 1000,
            ghost_engagement_secs: 0,
            directed_gate: true,
            hand_mode: true,
            hand_mouse_preview: false,
            hand_frame_source: default_hand_frame_source(),
            hand_show_camera: false,
            hand_trace: false,
            hand_with_ghost: false,
            hand_snap: true,
            hand_gestures: true,
            hand_focus_mode: true,
            hand_focus_lower_apps: true,
            hand_gestures_desktop: false,
            hand_sensitivity: 0.0,
            waves_horizontal_pct: 0.5,
            waves_vertical_pct: 1.0,
            waves_size: 200,
            loading_horizontal_pct: 0.95,
            loading_vertical_pct: 0.05,
            loading_size: 80,
            screen_tour: true,
            stt_log_confidence: false,
            stt_diagnostics: false,
            tour_model: String::new(),
            gemini_vision_daily_limit: default_gemini_vision_daily_limit(),
            persona_mode: default_persona_mode(),
            theme_mode: default_theme_mode(),
            memcore: default_memcore(),
            memcore_redact_names: false,
            memcore_activity: default_memcore(),
            memcore_briefing: default_memcore(),
            memcore_profile: default_memcore(),
            memcore_account_seed: default_memcore(),
            memcore_routines: default_memcore(),
            memcore_review: false,
            watch_power: default_watch_power(),
            wake_sound: true,
            wake_sound_style: default_wake_sound_style(),
            wake_sound_volume: default_wake_sound_volume(),
            memcore_timetable: default_memcore(),
            memcore_mail: default_memcore(),
            memcore_github_watch: default_memcore(),
            memcore_deploy_watch: default_memcore(),
            coderabbit_auto_open: default_memcore(),
            notify_native: true,
            notify_urgent: true,
            coderabbit_auto_review: false,
            memcore_mail_plain: false,
            memcore_whatsapp: false,
            memcore_whatsapp_speak: false,
            memcore_leave_by: default_memcore(),
            memcore_session: default_memcore(),
            early_hello: default_early_hello(),
            memcore_health: default_memcore(),
            power_commands: default_memcore(),
            default_music_player: default_music_player(),
            extra: std::collections::HashMap::new(),
        }
    }
}

/// TTS voice metadata returned by `list_tts_voices`.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtsVoiceInfo {
    pub id: String,
    pub name: String,
    pub gender: String,
    pub provider: String,
    pub language: String,
}

/// IPC: List the cloud TTS voices (Edge TTS). The offline Kokoro voice follows the equipped persona
/// (Feature 83), so it is not a separate picker entry.
/// Returns a curated list of en-US voices. The full Edge TTS catalog has
/// 400+ voices across 140+ locales — we surface the most useful en-US ones
/// to keep the settings UI manageable.
#[tauri::command]
pub fn list_tts_voices() -> Result<Vec<TtsVoiceInfo>, String> {
    let edge_voices = [
        ("en-US-AvaNeural",        "Ava",       "Female"),
        ("en-US-AndrewNeural",     "Andrew",    "Male"),
        ("en-US-AndrewMultilingualNeural", "Andrew ML", "Male"),
        ("en-US-EmmaNeural",       "Emma",      "Female"),
        ("en-US-BrianNeural",      "Brian",     "Male"),
        ("en-US-ChristopherNeural","Christopher","Male"),
        ("en-US-EricNeural",       "Eric",      "Male"),
        ("en-US-GuyNeural",        "Guy",       "Male"),
        ("en-US-JennyNeural",      "Jenny",     "Female"),
        ("en-US-MichelleNeural",   "Michelle",  "Female"),
        ("en-US-RogerNeural",      "Roger",     "Male"),
        ("en-US-SteffanNeural",    "Steffan",   "Male"),
        ("en-US-AriaNeural",       "Aria",      "Female"),
        ("en-US-DavisNeural",      "Davis",     "Male"),
        ("en-US-NancyNeural",      "Nancy",     "Female"),
        ("en-US-SaraNeural",       "Sara",      "Female"),
        ("en-US-NoraNeural",       "Nora",      "Female"),
        ("en-US-AmberNeural",      "Amber",     "Female"),
        ("en-US-AshleyNeural",     "Ashley",    "Female"),
        ("en-US-BrandonNeural",    "Brandon",   "Male"),
        ("enUS-CoraNeural",        "Cora",      "Female"),
        ("en-US-ElizabethNeural",  "Elizabeth", "Female"),
        ("en-US-MonicaNeural",     "Monica",    "Female"),
        ("en-US-SoniaNeural",      "Sonia",     "Female"),
    ];

    let voices: Vec<TtsVoiceInfo> = edge_voices
        .iter()
        .map(|(id, name, gender)| TtsVoiceInfo {
            id: id.to_string(),
            name: name.to_string(),
            gender: gender.to_string(),
            provider: "edge-tts".to_string(),
            language: "en-US".to_string(),
        })
        .collect();

    Ok(voices)
}

#[cfg(test)]
fn parse_settings_content(content: &str) -> NexusSettings {
    let mut settings: NexusSettings = serde_json::from_str(content).unwrap_or_default();
    settings.speaker_verification = voice_profile::voice_lock_from_settings(content);
    settings
}

/// IPC: Get the current settings (merged with defaults for missing fields).
#[tauri::command]
pub fn get_settings<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<NexusSettings, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let path = dir.join("settings.json");
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(NexusSettings::default());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut settings: NexusSettings = match parse_settings_lenient(&content) {
        Some((s, dropped)) => {
            if !dropped.is_empty() {
                tracing::warn!(
                    "settings.json: ignored invalid field(s) {:?}; other settings kept",
                    dropped
                );
            }
            s
        }
        None => {
            // Not JSON at all. Keep the broken file next to the new one so the
            // next Save cannot silently destroy it.
            let backup = dir.join("settings.json.corrupt");
            if !backup.exists() {
                let _ = std::fs::copy(&path, &backup);
            }
            tracing::error!("settings.json is unreadable; using defaults (copy kept at {:?})", backup);
            NexusSettings::default()
        }
    };
    // Merge server config if present
    let config_path = dir.join("nexus-config.json");
    if config_path.exists() {
        if let Ok(config) = std::fs::read_to_string(&config_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&config) {
                let default_url = option_env!("NEXUS_SERVER_URL")
                    .unwrap_or("https://nexus-worker.chitkullakshya.workers.dev");
                if let Some(url) = json.get("serverUrl").and_then(|v| v.as_str()) {
                    // Don't overwrite with empty string — keep default
                    settings.server_url = if url.is_empty() { default_url.to_string() } else { url.to_string() };
                }
                if let Some(uid) = json.get("userId").and_then(|v| v.as_str()) {
                    settings.user_id = uid.to_string();
                }
                if let Some(did) = json.get("deviceId").and_then(|v| v.as_str()) {
                    settings.device_id = did.to_string();
                }
            }
        }
    }
    normalize_orb_fields(&mut settings);
    Ok(settings)
}

/// IPC: Save settings to disk.
#[tauri::command]
pub fn save_settings<R: Runtime>(
    app: tauri::AppHandle<R>,
    settings: NexusSettings,
) -> Result<(), String> {
    save_settings_with_voice_intent(app, settings, None)
}

fn save_settings_with_voice_intent<R: Runtime>(
    app: tauri::AppHandle<R>,
    mut settings: NexusSettings,
    voice_lock_change: Option<bool>,
) -> Result<(), String> {
    // Keys are pasted by hand: a trailing space/newline makes every provider
    // answer 401 while the Settings card still looks configured.
    settings.groq_api_key = settings.groq_api_key.trim().to_string();
    settings.gemini_api_key = settings.gemini_api_key.trim().to_string();
    settings.cerebras_api_key = settings.cerebras_api_key.trim().to_string();
    // The keychain wins on read (`read_api_key`), but Settings only ever wrote
    // settings.json — so a stale keychain copy (from a heal-forward / import)
    // silently overrode a corrected key and made "Test key" report the OLD
    // key's result. Every save now updates both stores (an emptied field
    // clears the credential).
    crate::auth_vault::set_api_key("groq", &settings.groq_api_key);
    crate::auth_vault::set_api_key("gemini", &settings.gemini_api_key);
    crate::auth_vault::set_api_key("cerebras", &settings.cerebras_api_key);
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    settings.speaker_verification = voice_profile::voice_lock_after_settings_save(
        read_speaker_verification(&app), voice_lock_change,
    );
    if voice_lock_change == Some(true) {
        let profile_path = voice_profile::resolve_profile_path(&dir);
        let profile = voice_profile::VoiceProfile::load(&profile_path)
            .map_err(|_| "Enroll a valid voice profile before enabling Voice Lock".to_string())?;
        if !profile.is_valid() {
            return Err("Enroll a valid voice profile before enabling Voice Lock".to_string());
        }
    }
    let path = dir.join("settings.json");
    // API keys are managed by `api_keys::*`: an EMPTY key in the payload never
    // wipes what is on disk (and a deleted key cannot be resurrected by a
    // stale form state — delete clears the disk copy first).
    let existing = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    settings.groq_api_key = crate::api_keys::handle_save_key(&crate::api_keys::KeychainKeyStore, "groq", "groqApiKey", &existing, &settings.groq_api_key);
    settings.gemini_api_key = crate::api_keys::handle_save_key(&crate::api_keys::KeychainKeyStore, "gemini", "geminiApiKey", &existing, &settings.gemini_api_key);
    settings.cerebras_api_key = crate::api_keys::handle_save_key(&crate::api_keys::KeychainKeyStore, "cerebras", "cerebrasApiKey", &existing, &settings.cerebras_api_key);
    settings.deepgram_api_key = crate::api_keys::handle_save_key(&crate::api_keys::KeychainKeyStore, "deepgram", "deepgramApiKey", &existing, &settings.deepgram_api_key);
    settings.routes_api_key = crate::api_keys::handle_save_key(&crate::api_keys::KeychainKeyStore, "google_maps", "routesApiKey", &existing, &settings.routes_api_key);
    settings.places_api_key = crate::api_keys::handle_save_key(&crate::api_keys::KeychainKeyStore, "google_places", "placesApiKey", &existing, &settings.places_api_key);
    settings.logo_dev_publishable_key = crate::api_keys::handle_save_key(&crate::api_keys::KeychainKeyStore, "logo_dev", "logoDevPublishableKey", &existing, &settings.logo_dev_publishable_key);
    // The Worker URL receives every transcript and the device token: never accept
    // plain http (except localhost) or a malformed value from the UI.
    if !settings.server_url.is_empty() {
        settings.server_url = crate::network::normalize_worker_url(&settings.server_url)?;
    }
    normalize_voice_fields(&mut settings);
    normalize_orb_fields(&mut settings);
    if !settings.edge_tts_voice.is_empty() {
        settings.tts_voice = settings.edge_tts_voice.clone();
        if settings.tts_provider.is_empty() || settings.tts_provider == "kokoro" {
            settings.tts_provider = "edge".to_string();
        }
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    // Sensitivity takes effect immediately (no restart).
    crate::wakeword_oww::apply_wake_sensitivity(&settings.wake_sensitivity);
    crate::wakeword_oww::apply_wake_dsp(&settings.wake_dsp_profile);

    // Also save server config separately — but NEVER overwrite an existing
    // identity (user_id / device_id) with empty values. This prevents
    // identity loss if settings.json is stale or nexus-config.json was
    // temporarily missing.
    let config_path = dir.join("nexus-config.json");
    let default_url = option_env!("NEXUS_SERVER_URL")
        .unwrap_or("https://nexus-worker.chitkullakshya.workers.dev");

    // Read existing config to preserve identity if needed
    let existing = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok());

    let (server_url, user_id, device_id) = if let Some(ref existing) = existing {
        let preserved_url = if settings.server_url.is_empty() {
            existing["serverUrl"].as_str().unwrap_or(default_url).to_string()
        } else {
            settings.server_url.clone()
        };
        let preserved_uid = if settings.user_id.is_empty() {
            existing["userId"].as_str().unwrap_or("").to_string()
        } else {
            settings.user_id.clone()
        };
        let preserved_did = if settings.device_id.is_empty() {
            existing["deviceId"].as_str().unwrap_or("").to_string()
        } else {
            settings.device_id.clone()
        };
        (preserved_url, preserved_uid, preserved_did)
    } else {
        // No existing config — use what we have, with URL fallback
        let url = if settings.server_url.is_empty() { default_url.to_string() } else { settings.server_url.clone() };
        (url, settings.user_id.clone(), settings.device_id.clone())
    };

    // Update ONLY the three fields this command owns. The file also carries
    // the identity record (identity / profileId / lastKnownStatus / …) written
    // by identity_state; rebuilding it from scratch here dropped those on
    // every Settings save and silently reverted a claimed profile.
    let mut config = existing
        .as_ref()
        .and_then(|e| e.as_object().cloned())
        .unwrap_or_default();
    config.insert("serverUrl".into(), serde_json::Value::String(server_url));
    config.insert("userId".into(), serde_json::Value::String(user_id));
    config.insert("deviceId".into(), serde_json::Value::String(device_id));
    std::fs::write(&config_path, serde_json::Value::Object(config).to_string())
        .map_err(|e| e.to_string())?;
    let _ = app.emit("settings:updated", &settings);
    let _ = app.emit("orb:morphology_changed", serde_json::json!({
        "think": settings.orb_think_mode,
        "speak": settings.orb_speak_mode,
        "listen": settings.orb_listen_mode,
    }));
    let _ = app.emit("orb:capsule_style", serde_json::json!({
        "style": settings.orb_capsule_style,
        "mode": settings.orb_wakeup_mode,
        "genesis": settings.orb_interior_genesis,
    }));
    tracing::info!("settings saved to {:?}", path);
    Ok(())
}

/// One fact, many fields: the online voice IS the persona. When `edgeTtsVoice` is one of the catalog voices,
/// `selectedVoice`, `ttsVoice` and `offlineVoiceModel` are derived from it, so a window holding a stale copy of
/// the other fields can no longer write a mixed record (2026-10-15 23:02:44: the hub saved `bm_george` back over
/// the freshly equipped `af_heart`). A custom (non-catalog) voice is left exactly as the user set it.
pub fn normalize_voice_fields(s: &mut NexusSettings) {
    if let Some(p) = crate::voice_catalog::find_by_cloud_id(&s.edge_tts_voice) {
        s.selected_voice = p.key.to_string();
        s.tts_voice = p.cloud_id.to_string();
        s.offline_voice_model = p.kokoro_voice.to_string();
    }
}

/// The voice orb is always fixed at the top (top-center, 200px); only its design can be chosen.
pub fn normalize_orb_fields(s: &mut NexusSettings) {
    s.orb_position = "top".to_string();
    s.orb_horizontal_pct = 0.5;
    s.orb_vertical_pct = 0.0;
    s.orb_size = 200;
}

/// Merge `patch` (camelCase keys) into the current settings and validate the result as a whole.
pub fn apply_settings_patch(
    current: &NexusSettings,
    patch: &serde_json::Map<String, serde_json::Value>,
) -> Result<NexusSettings, String> {
    let mut v = serde_json::to_value(current).map_err(|e| e.to_string())?;
    let obj = v.as_object_mut().ok_or_else(|| "settings are not an object".to_string())?;
    for (k, val) in patch {
        obj.insert(k.clone(), val.clone());
    }
    serde_json::from_value(v).map_err(|e| format!("invalid settings patch: {e}"))
}

/// IPC: change only the given settings (e.g. `{ "themeMode": "light" }`). Unlike `save_settings`, which replaces
/// the whole file from the caller's possibly stale snapshot, this starts from the CURRENT settings, so a toggle
/// in one window can never revert something another window or an IPC changed meanwhile.
#[tauri::command]
pub fn settings_patch<R: Runtime>(
    app: tauri::AppHandle<R>,
    patch: serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let current = get_settings(app.clone())?;
    let merged = apply_settings_patch(&current, &patch)?;
    let voice_lock_change = patch.get("speakerVerification").and_then(serde_json::Value::as_bool);
    save_settings_with_voice_intent(app, merged, voice_lock_change)
}

#[cfg(test)]
mod settings_voice_tests {
    use super::*;

    #[test]
    fn settings_window_command_returns_future() {
        fn requires_future<F, Fut>(_: F)
        where
            F: FnOnce(tauri::AppHandle<tauri::Wry>) -> Fut,
            Fut: std::future::Future<Output = Result<(), String>>,
        {}
        requires_future(open_settings_window::<tauri::Wry>);
    }


    #[test]
    fn corrupt_settings_do_not_offer_an_unlocked_default_to_the_ui() {
        let current = parse_settings_content("corrupt settings");
        assert!(current.speaker_verification);
        let unrelated = serde_json::json!({"themeMode": "light"}).as_object().unwrap().clone();
        assert!(apply_settings_patch(&current, &unrelated).unwrap().speaker_verification);
        let recovery = serde_json::json!({"speakerVerification": false}).as_object().unwrap().clone();
        assert!(!apply_settings_patch(&current, &recovery).unwrap().speaker_verification);
    }

    fn with_voice(edge: &str, selected: &str, offline: &str) -> NexusSettings {
        let mut s = NexusSettings::default();
        s.edge_tts_voice = edge.to_string();
        s.selected_voice = selected.to_string();
        s.offline_voice_model = offline.to_string();
        s
    }

    #[test]
    fn the_2026_10_15_stale_offline_voice_is_repaired_from_the_online_voice() {
        // Hub equipped NEXUS (Ava), then saved a snapshot that still held JARVIS's twin.
        let mut s = with_voice("en-US-AvaNeural", "nexus", "bm_george");
        normalize_voice_fields(&mut s);
        assert_eq!(s.offline_voice_model, "af_heart");
        assert_eq!(s.selected_voice, "nexus");
        assert_eq!(s.tts_voice, "en-US-AvaNeural");
    }

    #[test]
    fn a_stale_persona_key_follows_the_online_voice_not_the_other_way_round() {
        let mut s = with_voice("en-GB-RyanNeural", "nexus", "af_heart");
        normalize_voice_fields(&mut s);
        assert_eq!((s.selected_voice.as_str(), s.offline_voice_model.as_str()), ("jarvis", "bm_george"));
    }

    #[test]
    fn a_custom_voice_is_never_overwritten() {
        let mut s = with_voice("en-IN-NeerjaNeural", "nexus", "af_heart");
        s.tts_voice = "en-IN-NeerjaNeural".to_string();
        normalize_voice_fields(&mut s);
        assert_eq!(s.selected_voice, "nexus");
        assert_eq!(s.tts_voice, "en-IN-NeerjaNeural");
        assert_eq!(s.edge_tts_voice, "en-IN-NeerjaNeural");
    }

    #[test]
    fn normalize_orb_fields_forces_fixed_top_center_and_size() {
        let mut s = NexusSettings::default();
        s.orb_position = "bottom".to_string();
        s.orb_horizontal_pct = 0.85;
        s.orb_vertical_pct = 0.95;
        s.orb_size = 360;

        normalize_orb_fields(&mut s);

        assert_eq!(s.orb_position, "top");
        assert_eq!(s.orb_horizontal_pct, 0.5);
        assert_eq!(s.orb_vertical_pct, 0.0);
        assert_eq!(s.orb_size, 200);
    }

    #[test]
    fn a_patch_changes_only_the_named_fields() {
        let mut cur = NexusSettings::default();
        cur.theme_mode = "dark".to_string();
        cur.edge_tts_voice = "en-US-AvaNeural".to_string();
        cur.selected_voice = "nexus".to_string();
        let patch: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"themeMode":"light"}"#).unwrap();
        let out = apply_settings_patch(&cur, &patch).unwrap();
        assert_eq!(out.theme_mode, "light");
        assert_eq!(out.edge_tts_voice, "en-US-AvaNeural", "a theme toggle must not touch the voice");
        assert_eq!(out.selected_voice, "nexus");
    }

    #[test]
    fn a_patch_with_the_wrong_type_is_rejected_whole() {
        let cur = NexusSettings::default();
        let patch: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"themeMode":42}"#).unwrap();
        assert!(apply_settings_patch(&cur, &patch).is_err());
    }
}

/// Read the Groq API key (non-IPC helper for stt.rs and the vision gates).
/// Delegates to `read_api_key` (keychain first, settings.json fallback) so a
/// keychain-only key — e.g. after a settings import, which strips keys from
/// the file — is honored by cloud STT too. Returns empty string if no key.
pub fn read_groq_api_key<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    read_api_key(app, "groq")
}

/// Read the localSttOnly flag from settings.json (non-IPC helper for stt.rs).
/// Returns true if the user has enabled "Local STT only" (privacy mode —
/// audio never leaves the device). Returns false if not set or file missing.
pub fn read_local_stt_only<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return DEFAULT_LOCAL_STT_ONLY; };
    let path = dir.join("settings.json");
    let Ok(content) = std::fs::read_to_string(&path) else { return DEFAULT_LOCAL_STT_ONLY; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return DEFAULT_LOCAL_STT_ONLY; };
    local_stt_only_from_json(&json)
}

/// Read `wakeSensitivity` ("low" | "medium" | "high") from settings.json.
pub fn read_wake_sensitivity<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let Ok(dir) = app.path().app_data_dir() else { return "medium".to_string(); };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("wakeSensitivity").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "medium".to_string())
}

/// Parse settings.json field by field. One bad or missing field (an older file
/// without a newer required key, a hand-edit with the wrong type) used to make
/// the whole parse fail → `unwrap_or_default()` → **every** setting (hotkey,
/// voices, orb position, …) reset to defaults in the UI, and the next Save
/// wrote those defaults over the user's file. Now only the offending fields
/// fall back; the rest is kept. Returns the settings and the dropped keys.
pub fn parse_settings_lenient(content: &str) -> Option<(NexusSettings, Vec<String>)> {
    let file: serde_json::Value = serde_json::from_str(content).ok()?;
    let file = file.as_object()?;
    // Fast path: a fully valid file.
    if let Ok(s) = serde_json::from_value::<NexusSettings>(serde_json::Value::Object(file.clone())) {
        return Some((s, Vec::new()));
    }
    let mut merged = serde_json::to_value(NexusSettings::default()).ok()?;
    let mut dropped = Vec::new();
    for (key, value) in file {
        let mut candidate = merged.clone();
        candidate.as_object_mut()?.insert(key.clone(), value.clone());
        if serde_json::from_value::<NexusSettings>(candidate.clone()).is_ok() {
            merged = candidate;
        } else {
            dropped.push(key.clone());
        }
    }
    serde_json::from_value::<NexusSettings>(merged).ok().map(|s| (s, dropped))
}

/// Read the sttLogConfidence flag (P0 measurement): log Whisper confidence per command turn.
pub fn read_stt_log_confidence<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let Ok(dir) = app.path().app_data_dir() else { return false; };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else { return false; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return false; };
    json.get("sttLogConfidence").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Read an API key by service name: keychain first, settings.json fallback.
/// Used by ghost.rs (vision keys) and the health panel. `service` is the
/// vault service id ("groq" | "gemini" | "cerebras").
pub fn read_api_key<R: tauri::Runtime>(app: &tauri::AppHandle<R>, service: &str) -> String {
    let vault_svc = match service {
        "routes" => "google_maps",
        "places" => "google_places",
        other => other,
    };
    if let Some(k) = crate::auth_vault::get_api_key(vault_svc) {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return k;
        }
    }
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return String::new(); };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else {
        return String::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return String::new();
    };
    let camel = match service {
        "gemini" => "geminiApiKey",
        "cerebras" => "cerebrasApiKey",
        "deepgram" => "deepgramApiKey",
        "google_maps" | "routes" => "routesApiKey",
        "google_places" | "places" => "placesApiKey",
        "logo_dev" => "logoDevPublishableKey",
        _ => "groqApiKey",
    };
    let snake = match service {
        "gemini" => "gemini_api_key",
        "cerebras" => "cerebras_api_key",
        "deepgram" => "deepgram_api_key",
        "google_maps" | "routes" => "routes_api_key",
        "google_places" | "places" => "places_api_key",
        "logo_dev" => "logo_dev_publishable_key",
        _ => "groq_api_key",
    };
    let val = json.get(camel)
        .or_else(|| json.get(snake))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if !val.is_empty() {
        return val;
    }
    if service == "google_maps" || service == "routes" {
        if let Some(v) = json
            .get("googleMapsApiKey")
            .or_else(|| json.get("google_maps_api_key"))
            .and_then(|v| v.as_str())
        {
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    String::new()
}

/// Read the verifyWake flag from settings.json (non-IPC helper for wakeword_oww.rs).
/// Stage-2 verifier: cross-check stage-1 acoustic candidates with STT before
/// firing the wake. Defaults to TRUE (fail-open default: verification on).
/// Set `"verifyWake": false` in settings.json to restore fire-on-detect.
pub fn read_verify_wake<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return true; };
    let path = dir.join("settings.json");
    let Ok(content) = std::fs::read_to_string(&path) else { return true; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return true; };
    json.get("verifyWake")
        .or_else(|| json.get("verify_wake"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// Read the vadSilero flag from settings.json (non-IPC helper for wakeword_oww.rs).
/// Neural (Silero v4) voice gate for the STT capture loop. Defaults to FALSE — the RMS gate is
/// the proven path; flip to true only after the evaluation in docs/research/jarvis-landscape/07.
pub fn read_vad_silero<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let Ok(dir) = app.path().app_data_dir() else { return false; };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else { return false; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return false; };
    json.get("vadSilero")
        .or_else(|| json.get("vad_silero"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Read the smartTurn flag from settings.json (non-IPC helper for wakeword_oww.rs).
/// Smart Turn v3.2 end-of-turn model for the STT capture loop. Defaults to FALSE (unproven).
pub fn read_smart_turn<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let Ok(dir) = app.path().app_data_dir() else { return false; };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else { return false; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return false; };
    json.get("smartTurn")
        .or_else(|| json.get("smart_turn"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Read turnEagerness ("low" | "medium" | "high") from settings.json. Unknown -> "medium".
pub fn read_turn_eagerness<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let Ok(dir) = app.path().app_data_dir() else { return "medium".into(); };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else { return "medium".into(); };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return "medium".into(); };
    json.get("turnEagerness")
        .or_else(|| json.get("turn_eagerness"))
        .and_then(|v| v.as_str())
        .unwrap_or("medium")
        .to_string()
}

/// Read the micKeepAlive flag from settings.json (non-IPC helper for wakeword_oww.rs).
/// Keep-alive render: inaudible output stream holds the Intel DSP awake.
/// Defaults to TRUE. Set `"micKeepAlive": false` to disable.
pub fn read_mic_keep_alive<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return true; };
    let path = dir.join("settings.json");
    let Ok(content) = std::fs::read_to_string(&path) else { return true; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return true; };
    json.get("micKeepAlive")
        .or_else(|| json.get("mic_keep_alive"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// Read the selected iconic voice persona key from settings.json
/// (Feature 83 catalog key, e.g. "jarvis"). Defaults to "nexus".
pub fn read_selected_voice<R: Runtime>(app: &tauri::AppHandle<R>) -> String {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return default_selected_voice() };
    let path = dir.join("settings.json");
    let Ok(content) = std::fs::read_to_string(&path) else { return default_selected_voice() };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return default_selected_voice() };
    let key = json.get("selectedVoice")
        .or_else(|| json.get("selected_voice"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if crate::voice_catalog::find_by_key(key).is_some() {
        key.to_string()
    } else {
        default_selected_voice()
    }
}

/// Read the edge-tts voice from settings.json (non-IPC helper for tts.rs).
/// Returns default voice if not set.
pub fn read_edge_tts_voice<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return "en-US-AvaNeural".to_string(); };
    let path = dir.join("settings.json");
    let Ok(content) = std::fs::read_to_string(&path) else { return "en-US-AvaNeural".to_string(); };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return "en-US-AvaNeural".to_string(); };
    json.get("edgeTtsVoice")
        .or_else(|| json.get("edge_tts_voice"))
        .and_then(|v| v.as_str())
        .unwrap_or("en-US-AvaNeural")
        .to_string()
}

// ─── Autostart management ───────────────────────────────────────────
//
// Windows: uses a Scheduled Task named "NEXUS" with AtLogOn trigger.
// macOS/Linux: uses tauri-plugin-autostart (LaunchAgent / systemd).

/// IPC: Enable or disable auto-start at login.
/// On Windows, creates/removes the "NEXUS" Scheduled Task.
/// On macOS/Linux, calls the autostart plugin enable/disable.
#[tauri::command]
pub fn set_autostart<R: Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = &app; // unused on Windows — autostart uses PowerShell directly
        let exe_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .map_err(|e| e.to_string())?;

        if enabled {
            // Create/update scheduled task with --background flag for silent start
            let ps_script = format!(
                r#"$exe = '{}';
                $user = [Security.Principal.WindowsIdentity]::GetCurrent().Name;
                $action = New-ScheduledTaskAction -Execute $exe -Argument '--background';
                $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user;
                $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable -Priority 4 -ExecutionTimeLimit (New-TimeSpan -Seconds 0);
                $result = Register-ScheduledTask -TaskName 'NEXUS' -Action $action -Trigger $trigger -Settings $settings -User $user -Force;
                if ($result) {{ Write-Output 'NEXUS_TASK_OK' }} else {{ Write-Output 'NEXUS_TASK_FAIL' }}"#,
                exe_path
            );
            let result = std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
                .creation_flags(0x08000000) // CREATE_NO_WINDOW
                .output();
            match result {
                Ok(out) if out.status.success()
                    && String::from_utf8_lossy(&out.stdout).contains("NEXUS_TASK_OK") =>
                {
                    tracing::info!("autostart: scheduled task 'NEXUS' created (AtLogOn, --background)");
                    Ok(true)
                }
                Ok(out) => {
                    tracing::warn!(
                        "autostart: Register-ScheduledTask failed: stdout={} stderr={}",
                        String::from_utf8_lossy(&out.stdout).trim(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                    Err("Failed to create scheduled task".to_string())
                }
                Err(e) => Err(format!("PowerShell error: {e}")),
            }
        } else {
            // Remove the scheduled task
            let result = std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command",
                    "Unregister-ScheduledTask -TaskName 'NEXUS' -Confirm:$false"])
                .creation_flags(0x08000000)
                .output();
            match result {
                Ok(out) if out.status.success() => {
                    tracing::info!("autostart: scheduled task 'NEXUS' removed");
                    Ok(false)
                }
                Ok(out) => {
                    // Task may not exist — that's fine, it's already disabled
                    tracing::info!(
                        "autostart: Unregister-ScheduledTask completed (stderr={})",
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                    Ok(false)
                }
                Err(e) => Err(format!("PowerShell error: {e}")),
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let autostart = app.autolaunch();
        #[cfg(target_os = "macos")]
        if enabled {
            let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
            if !crate::boot::runs_from_app_bundle(&exe) {
                return Err("Start at login works only from the installed NEXUS app, not a development build.".into());
            }
        }
        if enabled {
            autostart.enable().map_err(|e| e.to_string())?;
            tracing::info!("autostart: enabled (LaunchAgent)");
        } else {
            autostart.disable().map_err(|e| e.to_string())?;
            tracing::info!("autostart: disabled");
        }
        Ok(enabled)
    }
}

/// IPC: Check if auto-start is currently enabled.
#[tauri::command]
pub fn is_autostart_enabled<R: Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<bool, String> {
    #[cfg(target_os = "windows")]
    {
        let _ = app; // unused on Windows
        let result = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command",
                "Get-ScheduledTask -TaskName 'NEXUS' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty State"])
            .creation_flags(0x08000000)
            .output();
        match result {
            Ok(out) => {
                let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let enabled = state == "Ready" || state == "Running";
                Ok(enabled)
            }
            Err(e) => {
                tracing::warn!("is_autostart_enabled: {e}");
                Ok(false)
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let autostart = app.autolaunch();
        Ok(autostart.is_enabled().unwrap_or(false))
    }
}

// ─── Microphone permission ──────────────────────────────────────────
//
// On Windows, desktop (Win32) apps don't need per-app mic permission like UWP
// apps do. However, the GLOBAL mic privacy toggle (Settings → Privacy →
// Microphone → "Allow apps to access your microphone") can block all apps.
// When that's off, cpal returns an empty device list or fails to build a
// stream. We probe the default input device to detect this state.

/// IPC: Check if the microphone is accessible.
///
/// Probes cpal's default input device. Returns:
///   "granted"   — device found and stream can be built
///   "denied"    — no input devices or stream build failed (likely mic privacy off)
///   "no_device" — no input devices at all (no mic connected)
#[tauri::command]
pub fn check_mic_permission() -> String {
    use cpal::traits::{DeviceTrait, HostTrait};

    let host = cpal::default_host();

    // Check if there are any input devices at all
    let devices = match host.input_devices() {
        Ok(d) => d.collect::<Vec<_>>(),
        Err(e) => {
            tracing::warn!("check_mic_permission: input_devices() failed: {e}");
            return "denied".to_string();
        }
    };

    if devices.is_empty() {
        tracing::warn!("check_mic_permission: no input devices found");
        return "no_device".to_string();
    }

    // Try the default device
    match host.default_input_device() {
        Some(device) => {
            let dev_name = device.name().unwrap_or_else(|_| "unknown".into());
            tracing::info!("check_mic_permission: probing device '{}'", dev_name);

            // Try to build a minimal stream config to verify access
            let default_config = device.default_input_config();
            match default_config {
                Ok(_config) => {
                    // If we can get a default config, the device is accessible.
                    // Actually building a stream would require a callback and
                    // play() call, which is heavy for a permission check.
                    // Getting the default config is sufficient — if mic privacy
                    // is off, this returns an error on Windows.
                    tracing::info!("check_mic_permission: granted (device '{}')", dev_name);
                    "granted".to_string()
                }
                Err(e) => {
                    tracing::warn!("check_mic_permission: default_input_config failed: {e}");
                    "denied".to_string()
                }
            }
        }
        None => {
            tracing::warn!("check_mic_permission: no default input device");
            "no_device".to_string()
        }
    }
}

/// IPC: Open the OS microphone privacy settings.
///
/// On Windows, opens `ms-settings:privacy-microphone`.
/// On macOS, opens System Settings → Microphone.
/// On Linux, this is a no-op (PipeWire/PulseAudio handle permissions differently).
#[tauri::command]
pub fn open_mic_settings() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "ms-settings:privacy-microphone"])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| e.to_string())?;
        tracing::info!("opened Windows mic privacy settings");
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"])
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "linux")]
    {
        // No standard mic privacy settings on most Linux distros
        tracing::info!("open_mic_settings: no-op on Linux");
    }

    Ok(())
}

/// IPC: Clear the stored conversation. This used to be a no-op ("the frontend
/// handles it") while the Settings label promised "Remove all stored
/// transcripts" — `conversation.jsonl`, the conversation brief and the episodic
/// log stayed on disk. It now deletes them (profile/facts are separate:
/// `memory_clear`).
#[tauri::command]
pub fn clear_transcript<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    // Raw conversation turns + brief, and the legacy episodic log.
    let turns_cleared = crate::conversation::clear_all(&dir);
    let episodes = crate::memory::resolve_memory_dir(&dir).join("episodes.jsonl");
    let episodes_cleared = !episodes.exists() || std::fs::remove_file(episodes).is_ok();
    if turns_cleared && episodes_cleared {
        tracing::info!("stored conversation cleared");
        Ok(())
    } else {
        Err("couldn't remove every stored transcript file".to_string())
    }
}

/// Force a manual app registry refresh (e.g. after installing a new app).
/// Scans the OS for installed apps and updates the cache immediately.
#[tauri::command]
pub fn refresh_app_registry() -> Result<String, String> {
    tracing::info!("manual app registry refresh requested");
    app_registry::force_refresh();
    Ok("App registry refreshed".to_string())
}

/// IPC: Pause the wake-word engine's cpal stream.
/// Called by the frontend before acquiring the microphone via getUserMedia()
/// to avoid OS-level mic lock contention (Intel Smart Sound Technology).
#[tauri::command]
pub async fn pause_wakeword() -> Result<(), String> {
    // Pause can wait for a wake dispatch; never block the UI event loop it uses.
    tauri::async_runtime::spawn_blocking(crate::wakeword_oww::pause_stream)
        .await.map_err(|error| format!("Wake pause worker failed: {error}"))?
}

/// IPC: Resume the wake-word engine's cpal stream.
/// Called by the frontend after releasing the microphone so the wake-word
/// engine can resume listening for "NEXUS".
#[tauri::command]
pub async fn resume_wakeword() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(crate::wakeword_oww::resume_stream)
        .await.map_err(|error| format!("Wake resume worker failed: {error}"))?
}

/// IPC: Mic self-test — is the mic healthy, quiet, or dead?
/// Analyzes the live 2.5s verify ring (no new stream opened, SST-safe).
/// Returns peak level, exact-zero ratio, and a verdict string:
/// "healthy" | "quiet-room" | "dead-silence".
#[tauri::command]
pub fn mic_self_test() -> Result<crate::wakeword_oww::MicSelfTestReport, String> {
    Ok(crate::wakeword_oww::mic_self_test_data())
}

/// Start Rust-side STT capture from the cpal stream.
/// Called by the frontend when the user wakes NEXUS or when retrying
/// after an empty transcript. The cpal stream captures audio directly —
/// no getUserMedia, no baton pass. This fixes the Intel SST driver issue
/// where getUserMedia returns silence but cpal is still working.
#[tauri::command]
pub fn start_stt_capture(origin: Option<String>) -> Result<(), String> {
    // `origin: "hot_mic"` = the ghost hot-mic loop re-opened the mic (Phase 8 gate applies);
    // anything else (confirm window, clarification, follow-up question) is an explicit turn.
    let origin = crate::directed::Origin::parse(origin.as_deref().unwrap_or("direct"));
    crate::wakeword_oww::start_stt_capture_with_origin(origin);
    Ok(())
}

/// Abort an in-flight Rust-side STT capture (hotkey second-press cancel).
/// Returns whether voice was already underway + capture length so the
/// frontend can decide: hide the orb (no speech) vs let the turn finish.
#[tauri::command]
pub fn stop_stt_capture() -> Result<crate::wakeword_oww::SttAbortResult, String> {
    Ok(crate::wakeword_oww::abort_stt_capture())
}

/// Non-destructive voice check (no-input timeout): true once real voice
/// chunks landed. No state touched — slow-starter turns stay intact.
#[tauri::command]
pub fn stt_capture_had_speech() -> Result<bool, String> {
    Ok(crate::wakeword_oww::stt_capture_had_speech())
}

// ─── Ghost FIFO queue readers (loose settings.json parse) ───────────────

/// Read `wakeDspProfile` ("legacy" when unset).
pub fn read_wake_dsp_profile<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let Ok(dir) = app.path().app_data_dir() else { return "legacy".to_string(); };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("wakeDspProfile").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "legacy".to_string())
}

/// Read `wakeVerifyMode` ("always" | "tiered" | "off"); None when unset.
pub fn read_wake_verify_mode<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<String> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("wakeVerifyMode").and_then(|v| v.as_str()).map(str::to_string))
        .filter(|s| !s.trim().is_empty())
}

/// Read `wakeVerifyHigh` (the provisional default when unset or invalid).
pub fn read_wake_verify_high<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> f32 {
    let Ok(dir) = app.path().app_data_dir() else { return crate::wakeword_oww::DEFAULT_VERIFY_HIGH; };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("wakeVerifyHigh").and_then(|v| v.as_f64()))
        .map(|v| v as f32)
        .unwrap_or(crate::wakeword_oww::DEFAULT_VERIFY_HIGH)
}

fn default_routing_mode() -> String {
    "auto".to_string()
}

/// `(routingMode, routingServerUrl)` from settings.json, with the defaults when unset. The server token is a key in
/// the vault (`routing_server`), never in this file.
pub fn read_routing_settings<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> (String, String) {
    let j = app
        .path()
        .app_data_dir()
        .ok()
        .and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok())
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok());
    let get = |k: &str| j.as_ref().and_then(|j| j.get(k)).and_then(|v| v.as_str()).map(|s| s.trim().to_string());
    (get("routingMode").filter(|m| matches!(m.as_str(), "auto" | "server" | "google" | "open")).unwrap_or_else(default_routing_mode), get("routingServerUrl").unwrap_or_default())
}

/// Read `wakeOwnerFastPath` (false when unset: the cloud speech check stays on).
pub fn read_wake_owner_fast_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let Ok(dir) = app.path().app_data_dir() else { return false; };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("wakeOwnerFastPath").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Read `wakeSeedOffsetMs` (0 when unset or invalid).
pub fn read_wake_seed_offset_ms<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> i32 {
    let Ok(dir) = app.path().app_data_dir() else { return 0; };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("wakeSeedOffsetMs").and_then(|v| v.as_i64()))
        .map(|v| v.clamp(-400, 400) as i32)
        .unwrap_or(0)
}

/// Read the sttCommandHints flag (off unless the user turned it on).
pub fn read_stt_command_hints<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let Ok(dir) = app.path().app_data_dir() else { return false; };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("sttCommandHints").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Read the sttConfidenceGate flag (off unless the user turned it on).
pub fn read_stt_confidence_gate<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let Ok(dir) = app.path().app_data_dir() else { return false; };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("sttConfidenceGate").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// Read the ghostDepthAck flag (ghost FIFO queue depth-ACK, D5/D6).
pub fn read_ghost_depth_ack<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return true; };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else { return true; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return true; };
    json.get("ghostDepthAck")
        .or_else(|| json.get("ghost_depth_ack"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

/// Read the persona tone mode ("butler" default, "friend" opt-in).
/// Unknown values fall back to butler — tone must never break on a typo.
pub fn read_persona_mode<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return default_persona_mode() };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else {
        return default_persona_mode();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return default_persona_mode();
    };
    match json
        .get("personaMode")
        .or_else(|| json.get("persona_mode"))
        .and_then(|v| v.as_str())
    {
        Some("friend") => "friend".to_string(),
        _ => default_persona_mode(),
    }
}

/// Voice-switch the persona tone (read-modify-write — unknown fields survive).
/// Returns the stored mode ("friend" or "butler").
pub fn set_persona_mode<R: tauri::Runtime>(app: &tauri::AppHandle<R>, mode: &str) -> String {
    let stored = if mode == "friend" { "friend" } else { "butler" }.to_string();
    if let Ok(dir) = app.path().app_data_dir() {
        let path = dir.join("settings.json");
        let mut json: serde_json::Value =
            std::fs::read_to_string(&path).ok().and_then(|c| serde_json::from_str(&c).ok()).unwrap_or(
                serde_json::json!({}),
            );
        if let Some(obj) = json.as_object_mut() {
            obj.insert("personaMode".to_string(), serde_json::Value::String(stored.clone()));
            let _ = std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap_or_default());
        }
    }
    stored
}

/// Read the ghostStepTimeoutMs per-step watchdog value (D7). Floor 1000ms.
pub fn read_ghost_step_timeout_ms<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> u64 {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return 15000; };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else { return 15000; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return 15000; };
    json.get("ghostStepTimeoutMs")
        .or_else(|| json.get("ghost_step_timeout_ms"))
        .and_then(|v| v.as_u64())
        .filter(|v| *v >= 1000)
        .unwrap_or(15000)
}

/// Read the ghostTurnGapMs inter-step gap τ (D4). Ceiling 5000ms.
pub fn read_ghost_turn_gap_ms<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> u64 {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return 1000; };
    let Ok(content) = std::fs::read_to_string(dir.join("settings.json")) else { return 1000; };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else { return 1000; };
    json.get("ghostTurnGapMs")
        .or_else(|| json.get("ghost_turn_gap_ms"))
        .and_then(|v| v.as_u64())
        .filter(|v| *v <= 5000)
        .unwrap_or(1000)
}

/// Read the speakerVerification flag from settings.json (wakeword_oww.rs).
/// Defaults to FALSE. Set `"speakerVerification": true` to enable.
pub fn read_speaker_verification<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    let dir = app.path().app_data_dir();
    let Ok(dir) = dir else { return true; };
    let path = dir.join("settings.json");
    match path.try_exists() {
        Ok(false) => return false,
        Err(_) => return true,
        Ok(true) => {},
    }
    let Ok(content) = std::fs::read_to_string(path) else { return true; };
    voice_profile::voice_lock_from_settings(&content)
}

/// A single source of truth for frontend voice routing, including corrupt
/// settings recovery where the lock remains closed.
#[tauri::command]
pub fn voice_lock_enabled<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> bool {
    read_speaker_verification(&app)
}

// ─── Restored IPC commands (clobbered by a stale commands.rs rewrite) ───

/// IPC: Temporary ghost-pipeline tracer (p0–p4 stamps from recorder.ts).
/// Prints to the unified console so a silent death downstream is provable.
#[tauri::command]
pub fn debug_trace(msg: String) -> Result<(), String> {
    println!("[TRACE] {msg}");
    tracing::info!("debug_trace: {msg}");
    Ok(())
}

/// Checked emit (log-completeness P3): turn-critical events log their
/// emission at debug level (file-complete for cross-check grep,
/// console-quiet by default) so a missing frontend reaction is
/// distinguishable from a never-sent event. Backend emit failures
/// surface as warn. Use for turn-critical events only — never for
/// per-frame hot paths (audio:level etc).
pub fn emit_logged<R: Runtime, S: serde::Serialize + Clone>(app: &AppHandle<R>, event: &str, payload: S) {
    tracing::debug!("emit: {event}");
    if let Err(e) = app.emit(event, payload) {
        tracing::warn!("emit: '{event}' backend failure ({e})");
    }
}

// ─── Google multi-account commands ──────────────────────────────────────

/// IPC: Retrieve all connected Google account profiles.
#[tauri::command]
pub fn google_get_accounts() -> Vec<crate::google::types::GoogleAccountProfile> {
    crate::google::accounts::list_accounts()
}

/// IPC: Trigger direct RFC 8252 loopback OAuth sign-in to connect a Google account.
#[tauri::command]
pub async fn google_connect_account() -> Result<crate::google::types::GoogleAccountProfile, String> {
    crate::google::accounts::connect_account().await
}

/// IPC: Progressive re-consent for phone + address (settings "Add phone &
/// address"). Opens the same loopback flow with 2 extra sensitive scopes
/// and merges the result into the existing profile. Absence is normal.
#[tauri::command]
pub async fn google_connect_extended() -> Result<crate::google::types::GoogleAccountProfile, String> {
    crate::google::accounts::connect_extended().await
}

/// IPC: Promote an account to primary (avatar badge + default token).
#[tauri::command]
pub fn google_set_primary_account(email: String) -> Result<(), String> {
    crate::google::accounts::set_primary_account(&email)
}

/// IPC: Disconnect a Google account (registry + keyring cleanup).
#[tauri::command]
pub fn google_disconnect_account(email: String) -> Result<(), String> {
    crate::google::accounts::disconnect_account(&email)
}

/// IPC: Seed identity from the setup installer (Worker-OAuth connect).
/// Worker-held tokens must NEVER enter the engine registry — this fills
/// user.json gaps only (name/email/photo), never overwrites confirmed data.
#[tauri::command]
pub fn memory_seed_setup_identity<R: Runtime>(
    app: tauri::AppHandle<R>,
    name: Option<String>,
    email: Option<String>,
    picture: Option<String>,
) -> Result<bool, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    Ok(crate::memory::seed_setup_identity(
        &dir,
        name.as_deref(),
        email.as_deref(),
        picture.as_deref(),
    ))
}

/// IPC: Save custom developer Google OAuth credentials (override built-ins).
#[tauri::command]
pub fn google_save_custom_credentials(
    client_id: String,
    client_secret: Option<String>,
) -> Result<(), String> {
    crate::google::accounts::save_custom_credentials(&client_id, client_secret.as_deref())
}

/// IPC: Can Google sign-in start? (Client ID + Secret present and well-formed;
/// never returns the secret.)
#[tauri::command]
pub fn google_credentials_status() -> crate::google::oauth::CredentialsStatus {
    crate::google::accounts::credentials_status()
}

// ─── Memory / diary / webhook / improvement / health / settings IO ──────

/// IPC: Recall stored personal memories ("what is my X" support).
#[tauri::command]
pub fn memory_recall<R: Runtime>(app: AppHandle<R>, query: String) -> Vec<(String, String)> {
    let Ok(dir) = app.path().app_data_dir() else { return vec![] };
    crate::memory::recall(&dir, &query)
}

/// IPC: Memory Core status (enabled, encrypted at rest, counts, egress).
#[tauri::command]
pub fn memcore_status<R: Runtime>(app: AppHandle<R>) -> Result<crate::memcore::Status, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::status(&dir))
}

/// IPC: How long to keep the mic open for the answer to a question NEXUS
/// just asked ("Shall I start?"). `None` = no question is waiting.
#[tauri::command]
pub fn offer_pending() -> Option<u64> {
    crate::memcore::offer::listen_window_ms()
}

/// One filed important email for the Memory page.
#[derive(serde::Serialize)]
pub struct MailRowOut {
    pub key: String,
    #[serde(flatten)]
    pub mail: crate::memcore::mailtriage::StoredMail,
}

/// IPC: Important emails the inbox watcher filed (7 days, newest first).
#[tauri::command]
pub fn memcore_mail_list<R: Runtime>(app: AppHandle<R>) -> Result<Vec<MailRowOut>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let rows = crate::memcore::with_store(&dir, |s| {
        s.recent(crate::memcore::store::Tier::Mail, 60)
            .into_iter()
            .filter_map(|h| {
                serde_json::from_str::<crate::memcore::mailtriage::StoredMail>(&h.value)
                    .ok()
                    .map(|mail| MailRowOut { key: h.key, mail })
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    Ok(rows)
}

/// IPC: Stop alerts from one sender (stored as a visible, forgettable fact).
#[tauri::command]
pub fn memcore_mail_mute<R: Runtime>(app: AppHandle<R>, email: String) -> Result<bool, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    Ok(crate::memcore::with_store(&dir, |s| crate::memcore::mailwatch::mute_sender(s, &email, now)).unwrap_or(false))
}


/// One learned WhatsApp person for the Memory page.
#[derive(serde::Serialize)]
pub struct PersonOut {
    pub jid: String,
    pub name: String,
    pub score: u8,
    pub why: String,
    pub vip: bool,
    pub muted: bool,
}

/// IPC: People NEXUS has learned from WhatsApp patterns (best first). Counts
/// and timestamps only — no message text is kept for this list.
#[tauri::command]
pub fn memcore_people<R: Runtime>(app: AppHandle<R>) -> Result<Vec<PersonOut>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    Ok(crate::memcore::with_store(&dir, |s| {
        crate::memcore::people::ranked(&crate::memcore::people::load_all(s), now)
            .into_iter()
            .take(40)
            .map(|(p, sc)| PersonOut { jid: p.jid, name: p.name, score: sc.value, why: sc.why, vip: p.vip, muted: p.muted })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default())
}

/// IPC: Pin (VIP) or mute a learned person. `kind` = "vip" | "mute".
#[tauri::command]
pub fn memcore_person_flag<R: Runtime>(app: AppHandle<R>, jid: String, kind: String, on: bool) -> Result<bool, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    let flag = match kind.as_str() {
        "vip" => crate::memcore::wa::PersonFlag::Vip,
        "mute" => crate::memcore::wa::PersonFlag::Mute,
        _ => return Err("kind must be vip or mute".into()),
    };
    Ok(crate::memcore::with_store(&dir, |s| match crate::memcore::people::get(s, &jid) {
        Some(p) => matches!(crate::memcore::wa::set_flag(s, &p.name, flag, on, now), crate::memcore::wa::FlagOutcome::Done(_)) || {
            // Two people can share a name: fall back to the exact id.
            let mut p = p;
            match flag {
                crate::memcore::wa::PersonFlag::Vip => {
                    p.vip = on;
                    if on { p.muted = false; }
                }
                crate::memcore::wa::PersonFlag::Mute => {
                    p.muted = on;
                    if on { p.vip = false; }
                }
            }
            crate::memcore::people::save_user_choice(s, &p, now);
            true
        },
        None => false,
    })
    .unwrap_or(false))
}

/// IPC: Check the WhatsApp bridge (running? paired? list format understood?
/// which receipt/presence tools does it offer — NEXUS refuses to call them).
#[tauri::command]
pub async fn whatsapp_selftest() -> crate::memcore::wa::SelfTest {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default();
    crate::memcore::wa::selftest(&client).await
}

/// Parse a spoken/IPC travel mode ("driving" | "walking" | "bicycling" | "transit"); default driving.
pub fn parse_travel_mode(mode: Option<&str>) -> crate::google::types::TravelMode {
    use crate::google::types::TravelMode;
    match mode.map(|m| m.trim().to_lowercase()).as_deref() {
        Some("walking") | Some("walk") => TravelMode::Walking,
        Some("bicycling") | Some("cycling") | Some("bike") => TravelMode::Bicycling,
        Some("transit") | Some("bus") | Some("train") => TravelMode::Transit,
        _ => TravelMode::Driving,
    }
}

/// Open the real Google Maps app (directions) in its own window, sized and
/// placed exactly like the Architecture Mapper (960 wide, centered, y = 40).
pub fn open_maps<R: Runtime>(
    app: &tauri::AppHandle<R>,
    destination: &str,
    origin: Option<&str>,
    mode: crate::google::types::TravelMode,
) -> Result<(), String> {
    if destination.trim().is_empty() {
        return Err("no destination".into());
    }
    let url = crate::google::maps::MapsService::build_directions_url(origin, destination, mode);
    open_maps_url(app, &url)
}

/// Open any Google Maps https URL in the Architecture-Mapper-sized window.
pub fn open_maps_url<R: Runtime>(app: &tauri::AppHandle<R>, url: &str) -> Result<(), String> {
    let (mw, mh) = match app.primary_monitor() {
        Ok(Some(m)) => (m.size().width as f64 / m.scale_factor(), m.size().height as f64 / m.scale_factor()),
        _ => (1920.0, 1080.0),
    };
    let geom = sidebar_geometry("maps", None, mw, mh);
    crate::dyn_windows::open_external_window(app, "maps", url, "Google Maps", geom)
}

/// IPC: open Google Maps directions to `destination` (optionally from `origin`).
#[tauri::command]
pub async fn open_maps_window<R: Runtime>(
    app: tauri::AppHandle<R>,
    destination: String,
    origin: Option<String>,
    mode: Option<String>,
) -> Result<(), String> {
    open_maps(&app, &destination, origin.as_deref(), parse_travel_mode(mode.as_deref()))
}

/// IPC: The saved timetable.
#[tauri::command]
pub fn memcore_timetable<R: Runtime>(app: AppHandle<R>) -> Result<Vec<crate::memcore::timetable::Slot>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::with_store(&dir, crate::memcore::timetable::load_slots).unwrap_or_default())
}

/// IPC: Remove one timetable slot.
#[tauri::command]
pub fn memcore_timetable_delete<R: Runtime>(app: AppHandle<R>, id: String) -> Result<bool, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().timestamp();
    Ok(crate::memcore::with_store(&dir, |s| crate::memcore::timetable::delete_slot(s, &id, now)).unwrap_or(false))
}

/// IPC: Erase the recorded activity history (resume points) only.
#[tauri::command]
pub fn memcore_clear_activity<R: Runtime>(app: AppHandle<R>) -> Result<usize, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::clear_activity(&dir))
}

/// IPC: Everything NEXUS remembers, with provenance ("what do you remember").
#[tauri::command]
pub fn memcore_list<R: Runtime>(app: AppHandle<R>) -> Result<Vec<crate::memcore::store::Row>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::list_rows(&dir, 500))
}

/// IPC: Mishearings NEXUS has noticed ("heard brief, you meant brave"), with whether each is applied.
#[tauri::command]
pub fn memcore_corrections<R: Runtime>(app: AppHandle<R>) -> Result<Vec<crate::memcore::store::CorrectionRow>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::list_corrections(&dir))
}

/// IPC: Saved routines plus the suggestion waiting for a name, if any.
#[tauri::command]
pub fn memcore_routines<R: Runtime>(app: AppHandle<R>) -> Result<crate::memcore::routines::View, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::routines::view(&dir))
}

/// IPC: Save the waiting suggestion under a name the user typed.
#[tauri::command]
pub fn memcore_routine_save<R: Runtime>(app: AppHandle<R>, name: String) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    crate::memcore::routines::save_pending(&dir, &name).map(|r| r.name)
}

/// IPC: Delete a saved routine.
#[tauri::command]
pub fn memcore_routine_delete<R: Runtime>(app: AppHandle<R>, name: String) -> Result<bool, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::routines::delete(&dir, name.trim()))
}

/// IPC: Memory changes suggested by the background review, waiting for approval.
#[tauri::command]
pub fn memcore_pending<R: Runtime>(app: AppHandle<R>) -> Result<Vec<crate::memcore::store::PendingRow>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::review::list(&dir))
}

/// IPC: Approve one suggestion; returns a sentence describing what was stored.
#[tauri::command]
pub fn memcore_pending_approve<R: Runtime>(app: AppHandle<R>, id: i64) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    crate::memcore::review::approve(&dir, id)
}

/// IPC: Refuse one suggestion for good.
#[tauri::command]
pub fn memcore_pending_reject<R: Runtime>(app: AppHandle<R>, id: i64) -> Result<bool, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::review::reject(&dir, id))
}

/// IPC: Stop applying (and forget) what was learned about one mis-heard phrase.
#[tauri::command]
pub fn memcore_correction_forget<R: Runtime>(app: AppHandle<R>, heard: String) -> Result<usize, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::forget_correction(&dir, &heard))
}

/// IPC: The last 7 days of what was sent to cloud models.
#[tauri::command]
pub fn memcore_egress_log<R: Runtime>(
    app: AppHandle<R>,
    limit: Option<usize>,
) -> Result<Vec<crate::memcore::store::EgressEntry>, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::memcore::egress_log(&dir, limit.unwrap_or(50).min(200)))
}

/// IPC: Forget a stored memory key.
#[tauri::command]
pub fn memory_forget<R: Runtime>(app: AppHandle<R>, key: String) -> bool {
    let Ok(dir) = app.path().app_data_dir() else { return false };
    crate::memory::forget(&dir, &key)
}

/// IPC: Diary rollup for UI — recent events + a one-line summary label.
#[tauri::command]
pub fn diary_summary<R: Runtime>(app: AppHandle<R>) -> Result<serde_json::Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let events = crate::diary::recent_events(&dir, 50);
    let label = crate::diary::rollup_label(&events, std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64);
    Ok(serde_json::json!({ "label": label, "events": events }))
}

/// IPC: Current webhook bearer token (Connections tab display).
#[tauri::command]
pub fn webhook_token() -> String {
    crate::webhook::webhook_token()
}

/// IPC: Rotate the webhook bearer token; returns the new one.
#[tauri::command]
pub fn webhook_rotate_token() -> String {
    crate::webhook::rotate_webhook_token()
}

/// IPC: Edge-case miner report (miss/failure clusters → suggestions).
#[tauri::command]
pub fn improvement_report<R: Runtime>(app: AppHandle<R>) -> Result<serde_json::Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let clusters = crate::improve::run_miner(&dir);
    Ok(serde_json::json!({ "suggestions": crate::improve::top_suggestions(&clusters, 20) }))
}

/// IPC: Ghost vision provider daily-quota counters (Accounts tab UI).
#[tauri::command]
pub fn vision_quota_status<R: Runtime>(app: AppHandle<R>) -> Result<serde_json::Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(crate::vision::quota_status(&dir))
}

/// IPC: Vision key presence + Gemini quota in one call (Feature 83 P5 —
/// drives the Gemini card status pill; presence only, never key values).
#[tauri::command]
pub fn vision_key_status<R: Runtime>(app: AppHandle<R>) -> Result<serde_json::Value, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let quota = crate::vision::quota_status(&dir);
    Ok(serde_json::json!({
        "gemini_present": !read_api_key(&app, "gemini").is_empty(),
        "groq_present": !read_api_key(&app, "groq").is_empty(),
        "gemini_used": quota.get("gemini").and_then(|g| g.get("used")),
        "gemini_limit": quota.get("gemini").and_then(|g| g.get("limit")),
    }))
}

/// IPC: Validate the stored Gemini key with a quota-free models/list call
/// (Feature 83 P5 — the hub "Test" button; retrieval costs no quota).
#[tauri::command]
pub async fn vision_test_key<R: Runtime>(app: AppHandle<R>) -> Result<serde_json::Value, String> {
    let key = read_api_key(&app, "gemini");
    if key.is_empty() {
        return Ok(serde_json::json!({ "ok": false, "detail": "no-key" }));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    match client
        .get("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1")
        .header("x-goog-api-key", &key)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            Ok(serde_json::json!({ "ok": true, "detail": "valid" }))
        }
        Ok(resp) if resp.status().as_u16() == 429 => {
            Ok(serde_json::json!({ "ok": false, "detail": "quota-exhausted" }))
        }
        Ok(_) => Ok(serde_json::json!({ "ok": false, "detail": "rejected" })),
        Err(_) => Ok(serde_json::json!({ "ok": false, "detail": "network-error" })),
    }
}

/// IPC: Export settings (secrets stay in the OS keychain — never in the file).
#[tauri::command]
pub fn export_settings<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let path = dir.join("settings.json");
    let content = std::fs::read_to_string(&path).unwrap_or_else(|_| "{}".to_string());
    let mut json: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));
    // Secrets live in the keychain — strip any legacy disk copies.
    for key in ["groqApiKey", "geminiApiKey", "cerebrasApiKey", "deepgramApiKey", "routesApiKey", "placesApiKey", "logoDevPublishableKey"] {
        json[key] = serde_json::json!("");
    }
    serde_json::to_string_pretty(&json).map_err(|e| e.to_string())
}

/// Fields an imported file must never be able to set: identity (it comes from the
/// registered device), and the remote-control channel (a swapped Telegram chat id would
/// hand command access to whoever wrote the file).
const IMPORT_BLOCKED_KEYS: [&str; 3] = ["userId", "deviceId", "telegramChatId"];

/// Validate an imported settings document before it is written to disk.
fn sanitize_imported_settings(mut json: serde_json::Value) -> Result<serde_json::Value, String> {
    let obj = json
        .as_object_mut()
        .ok_or_else(|| "settings file must be a JSON object".to_string())?;
    for key in IMPORT_BLOCKED_KEYS {
        obj.remove(key);
    }
    if let Some(url) = obj.get("serverUrl").and_then(|v| v.as_str()).map(str::to_owned) {
        if url.is_empty() {
            obj.remove("serverUrl");
        } else {
            let ok = crate::network::normalize_worker_url(&url)?;
            obj.insert("serverUrl".into(), serde_json::Value::String(ok));
        }
    }
    Ok(json)
}

/// IPC: Import settings from a JSON string (writes settings.json + keychain).
#[tauri::command]
pub fn import_settings<R: Runtime>(app: AppHandle<R>, json_str: String) -> Result<(), String> {
    let json: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| format!("invalid settings JSON: {e}"))?;
    let json = sanitize_imported_settings(json)?;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut json = json;
    // An import means "use these": a key in the imported file replaces the one in the keychain (the plain scrub would
    // treat a different stored value as a conflict and leave the imported one unused). Then the file copy is blanked.
    for &(service, _, field) in crate::api_keys::KEY_SERVICES {
        if let Some(k) = json.get(field).and_then(|v| v.as_str()).filter(|k| !k.is_empty()) {
            crate::auth_vault::set_api_key(service, k);
        }
    }
    crate::api_keys::scrub_plaintext_keys(&mut json, &crate::api_keys::KeychainKeyStore);
    let path = dir.join("settings.json");
    let pretty = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    crate::memory::atomic_write(&path, pretty.as_bytes());
    Ok(())
}

// ─── Unified sidebar geometry tests ─────────────────────────────────

#[cfg(test)]
mod sidebar_geometry_tests {
    use super::sidebar_geometry;

    #[test]
    fn annotate_is_a_compact_window_on_screen() {
        for (mw, mh) in [(1366.0, 728.0), (1920.0, 1040.0), (2560.0, 1400.0), (1000.0, 600.0)] {
            let (x, y, w, h) = sidebar_geometry("annotate", None, mw, mh);
            assert!(w <= 320.0 && h <= 380.0 && x >= 0.0 && y >= 0.0 && x + w <= mw + 0.001 && y + h <= mh + 0.001, "{mw}x{mh}: {x},{y},{w},{h}");
        }
    }

    #[test]
    fn architect_beside_sits_left_of_the_sidebar_without_overlap() {
        for (mw, mh) in [(1366.0, 728.0), (1280.0, 720.0), (1536.0, 824.0), (1920.0, 1040.0), (2560.0, 1400.0), (1000.0, 700.0)] {
            let (sx, _, sw, _) = sidebar_geometry("assistant", None, mw, mh);
            let (x, y, w, h) = sidebar_geometry("architect-beside", None, mw, mh);
            assert!(x >= 0.0 && y >= 0.0 && w >= 320.0 && h >= 400.0, "{mw}x{mh}: {x},{y},{w},{h}");
            assert!(x + w <= sx - 10.0 + 0.001 || sx - 10.0 < 320.0, "{mw}x{mh}: map right edge {} vs sidebar left {}", x + w, sx);
            assert!(sx + sw <= mw + 0.001, "{mw}x{mh}: sidebar stays on screen");
            assert!(w <= 960.0);
        }
    }

    #[test]
    fn only_the_assistant_and_coderabbit_views_are_replaceable_by_an_alert() {
        use super::is_pinned_view;
        assert!(!is_pinned_view("assistant"));
        assert!(!is_pinned_view("coderabbit"));
        for v in ["settings", "architect", "pr-list", "github-dashboard", "maps", "stack"] {
            assert!(is_pinned_view(v), "{v}");
        }
    }

    #[test]
    fn assistant_defaults_to_right_dock() {
        // 1920x1080: x = 1920 - 520 - 10 = 1390, y = 20,
        // h = min(1040, 1080-40) = 1040
        let (x, y, w, h) = sidebar_geometry("assistant", None, 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (1390.0, 20.0, 520.0, 1040.0));
    }

    #[test]
    fn assistant_left_dock_mirrors() {
        let (x, y, w, h) = sidebar_geometry("assistant", Some("left"), 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (10.0, 20.0, 520.0, 1040.0));
    }

    #[test]
    fn assistant_center_float() {
        // center: x = (1920-520)/2 = 700, y = (1080-1040)/2 = 20
        let (x, y, w, h) = sidebar_geometry("assistant", Some("center"), 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (700.0, 20.0, 520.0, 1040.0));
    }

    #[test]
    fn assistant_height_clamps_on_short_monitor() {
        // mh - 40 < 1040 → clamp; h never below 400
        let (_, _, _, h) = sidebar_geometry("assistant", None, 1920.0, 600.0);
        assert_eq!(h, 560.0);
        let (_, _, _, h) = sidebar_geometry("assistant", None, 1920.0, 300.0);
        assert_eq!(h, 400.0);
    }

    #[test]
    fn pr_list_uses_right_dock() {
        // x = 1920 - 400 - 10 = 1510, y = 20, h = 1040 (400px same as settings)
        let (x, y, w, h) = sidebar_geometry("pr-list", None, 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (1510.0, 20.0, 400.0, 1040.0));
    }

    #[test]
    fn settings_right_docked() {
        // h = min(1040, 1080-40) = 1040; x = 1920 - 400 - 10 = 1510; y = 20
        let (x, y, w, h) = sidebar_geometry("settings", None, 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (1510.0, 20.0, 400.0, 1040.0));
    }

    #[test]
    fn settings_docked_goes_right() {
        let (x, y, w, h) = sidebar_geometry("settings", Some("right"), 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (1510.0, 20.0, 400.0, 1040.0));
    }

    #[test]
    fn settings_height_clamps_on_short_monitor() {
        let (_, _, _, h) = sidebar_geometry("settings", None, 1920.0, 800.0);
        assert_eq!(h, 760.0);
        let (_, _, _, h) = sidebar_geometry("settings", None, 1920.0, 300.0);
        assert_eq!(h, 400.0);
    }

    #[test]
    fn architect_centers_with_top_pin() {
        // h = min(1040, 1080-40) = 1040; x = (1920-960)/2 = 480; y = 40 (pinned)
        let (x, y, w, h) = sidebar_geometry("architect", None, 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (480.0, 40.0, 960.0, 1040.0));
    }

    #[test]
    fn maps_window_matches_architect_geometry() {
        for (mw, mh) in [(1920.0, 1080.0), (2560.0, 1440.0), (1366.0, 768.0), (500.0, 500.0)] {
            assert_eq!(sidebar_geometry("maps", None, mw, mh), sidebar_geometry("architect", None, mw, mh));
        }
        assert_eq!(sidebar_geometry("maps", None, 1920.0, 1080.0), (480.0, 40.0, 960.0, 1040.0));
    }

    #[test]
    fn travel_mode_parsing() {
        use crate::google::types::TravelMode as T;
        assert_eq!(super::parse_travel_mode(None), T::Driving);
        assert_eq!(super::parse_travel_mode(Some("Walking")), T::Walking);
        assert_eq!(super::parse_travel_mode(Some("bus")), T::Transit);
        assert_eq!(super::parse_travel_mode(Some("cycling")), T::Bicycling);
        assert_eq!(super::parse_travel_mode(Some("???")), T::Driving);
    }

    #[test]
    fn architect_docked_goes_right() {
        // x = 1920 - 960 - 10 = 950; y = 20; h = 1040
        let (x, y, w, h) = sidebar_geometry("architect", Some("right"), 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (950.0, 20.0, 960.0, 1040.0));
    }

    #[test]
    fn architect_height_clamps_on_short_monitor() {
        let (_, _, _, h) = sidebar_geometry("architect", None, 1920.0, 500.0);
        assert_eq!(h, 460.0);
    }

    #[test]
    fn unknown_view_falls_back_to_assistant() {
        let (x, y, w, h) = sidebar_geometry("nonsense", None, 1920.0, 1080.0);
        assert_eq!((x, y, w, h), (1390.0, 20.0, 520.0, 1040.0));
    }

    #[test]
    fn small_monitor_x_never_negative() {
        let (x, _, _, _) = sidebar_geometry("architect", None, 500.0, 1080.0);
        assert_eq!(x, 0.0);
        let (x, _, _, _) = sidebar_geometry("assistant", None, 100.0, 1080.0);
        // 100 - 520 - 10 = -430 → clamp to 0
        assert_eq!(x, 0.0);
    }
}

#[cfg(test)]
mod import_sanitize_tests {
    use super::sanitize_imported_settings;
    use serde_json::json;

    #[test]
    fn blocks_identity_and_remote_control_fields() {
        let out = sanitize_imported_settings(json!({
            "userId": "victim", "deviceId": "d", "telegramChatId": "999",
            "hotkey": "Ctrl+Space"
        }))
        .unwrap();
        assert!(out.get("userId").is_none());
        assert!(out.get("deviceId").is_none());
        assert!(out.get("telegramChatId").is_none());
        assert_eq!(out["hotkey"], "Ctrl+Space");
    }

    #[test]
    fn rejects_insecure_or_malformed_server_url() {
        assert!(sanitize_imported_settings(json!({"serverUrl": "http://evil.example.com"})).is_err());
        assert!(sanitize_imported_settings(json!({"serverUrl": "javascript:1"})).is_err());
    }

    #[test]
    fn keeps_and_normalizes_a_good_server_url() {
        let out = sanitize_imported_settings(json!({"serverUrl": "https://w.example.workers.dev/"})).unwrap();
        assert_eq!(out["serverUrl"], "https://w.example.workers.dev");
        let out = sanitize_imported_settings(json!({"serverUrl": ""})).unwrap();
        assert!(out.get("serverUrl").is_none());
    }

    #[test]
    fn non_object_documents_are_refused() {
        assert!(sanitize_imported_settings(json!([1, 2])).is_err());
        assert!(sanitize_imported_settings(json!("x")).is_err());
    }
}

#[cfg(test)]
mod nexus_settings_tests {
    use super::NexusSettings;

    #[test]
    fn settings_round_trip_preserves_unknown_fields() {
        let raw = serde_json::json!({
            "autostart": true,
            "hotkey": "Ctrl+Space",
            "autoHideDelay": 8,
            "wakeWordEnabled": true,
            "wakePhrase": "NEXUS",
            "wakeSensitivity": "medium",
            "speakerVerification": false,
            "meetingModeAuto": true,
            "suppressTtsInMeetings": true,
            "localSttOnly": true,
            "serverUrl": "https://example.com",
            "userId": "u123",
            "deviceId": "d456",
            "ttsVoice": "af_sky",
            "speechRate": 1.15,
            // Unknown keys that must NOT be wiped on save
            "vadSilero": true,
            "smartTurn": false,
            "turnEagerness": 0.85,
            "directedGate": true,
            "proactiveCriticalInMeeting": false,
            "calibrationDevPersist": "custom_val"
        });

        let parsed: NexusSettings = serde_json::from_value(raw).expect("deserialization should succeed");
        assert_eq!(parsed.extra.get("vadSilero"), Some(&serde_json::json!(true)));
        assert_eq!(parsed.extra.get("calibrationDevPersist"), Some(&serde_json::json!("custom_val")));

        let reserialized = serde_json::to_value(&parsed).expect("serialization should succeed");
        assert_eq!(reserialized.get("vadSilero"), Some(&serde_json::json!(true)));
        assert_eq!(reserialized.get("turnEagerness"), Some(&serde_json::json!(0.85)));
        assert_eq!(reserialized.get("calibrationDevPersist"), Some(&serde_json::json!("custom_val")));
    }

    #[test]
    fn memcore_mail_plain_defaults_to_off() {
        let mut raw = serde_json::to_value(NexusSettings::default()).unwrap();
        raw.as_object_mut().unwrap().remove("memcoreMailPlain");
        let parsed: NexusSettings = serde_json::from_value(raw).expect("deserialization should succeed");
        assert!(!parsed.memcore_mail_plain, "the cloud rewrite is opt-in");
        let reserialized = serde_json::to_value(&parsed).expect("serialization should succeed");
        assert_eq!(reserialized.get("memcoreMailPlain"), Some(&serde_json::json!(false)));
    }

    #[test]
    fn hand_focus_mode_defaults_to_true_and_round_trips() {
        let mut raw = serde_json::to_value(NexusSettings::default()).unwrap();
        raw.as_object_mut().unwrap().remove("handFocusMode");
        let parsed: NexusSettings = serde_json::from_value(raw).expect("deserialization should succeed");
        assert!(parsed.hand_focus_mode, "handFocusMode must default to true");
        let reserialized = serde_json::to_value(&parsed).expect("serialization should succeed");
        assert_eq!(reserialized.get("handFocusMode"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn hand_focus_lower_apps_defaults_to_true_and_round_trips() {
        let mut raw = serde_json::to_value(NexusSettings::default()).unwrap();
        raw.as_object_mut().unwrap().remove("handFocusLowerApps");
        let parsed: NexusSettings = serde_json::from_value(raw).expect("deserialization should succeed");
        assert!(parsed.hand_focus_lower_apps, "handFocusLowerApps must default to true");
        let reserialized = serde_json::to_value(&parsed).expect("serialization should succeed");
        assert_eq!(reserialized.get("handFocusLowerApps"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn memcore_github_watch_defaults_to_true_and_round_trips() {
        let mut raw = serde_json::to_value(NexusSettings::default()).unwrap();
        raw.as_object_mut().unwrap().remove("memcoreGithubWatch");
        let parsed: NexusSettings = serde_json::from_value(raw).expect("deserialization should succeed");
        assert!(parsed.memcore_github_watch, "memcoreGithubWatch must default to true");
        let reserialized = serde_json::to_value(&parsed).expect("serialization should succeed");
        assert_eq!(reserialized.get("memcoreGithubWatch"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn memcore_deploy_watch_defaults_to_true_and_round_trips() {
        let mut raw = serde_json::to_value(NexusSettings::default()).unwrap();
        raw.as_object_mut().unwrap().remove("memcoreDeployWatch");
        let parsed: NexusSettings = serde_json::from_value(raw).expect("deserialization should succeed");
        assert!(parsed.memcore_deploy_watch, "memcoreDeployWatch must default to true");
        let reserialized = serde_json::to_value(&parsed).expect("serialization should succeed");
        assert_eq!(reserialized.get("memcoreDeployWatch"), Some(&serde_json::json!(true)));
    }
}


#[cfg(test)]
mod settings_robustness_tests {
    use super::*;

    #[test]
    fn local_stt_only_has_one_default_everywhere() {
        assert!(!DEFAULT_LOCAL_STT_ONLY);
        assert_eq!(NexusSettings::default().local_stt_only, DEFAULT_LOCAL_STT_ONLY);
        // Absent key → the same default the struct uses.
        assert_eq!(local_stt_only_from_json(&serde_json::json!({})), DEFAULT_LOCAL_STT_ONLY);
        // Explicit values win, in either spelling.
        assert!(local_stt_only_from_json(&serde_json::json!({"localSttOnly": true})));
        assert!(local_stt_only_from_json(&serde_json::json!({"local_stt_only": true})));
        assert!(!local_stt_only_from_json(&serde_json::json!({"localSttOnly": false})));
        // Wrong type → default, not a panic.
        assert_eq!(
            local_stt_only_from_json(&serde_json::json!({"localSttOnly": "yes"})),
            DEFAULT_LOCAL_STT_ONLY
        );
    }

    #[test]
    fn lenient_parse_keeps_valid_fields_when_one_is_bad() {
        let mut v = serde_json::to_value(NexusSettings::default()).unwrap();
        v["hotkey"] = serde_json::json!("Ctrl+Alt+N");
        v["orbSize"] = serde_json::json!("huge"); // wrong type
        v["ttsVolume"] = serde_json::json!(33);
        v["somethingNew"] = serde_json::json!(1); // unknown keys are fine
        let (s, dropped) = parse_settings_lenient(&v.to_string()).expect("parses");
        assert_eq!(s.hotkey, "Ctrl+Alt+N", "valid custom field survives");
        assert_eq!(s.tts_volume, 33);
        assert_eq!(s.orb_size, NexusSettings::default().orb_size, "bad field falls back alone");
        assert_eq!(dropped, vec!["orbSize".to_string()]);
    }

    #[test]
    fn lenient_parse_handles_files_missing_required_fields() {
        // An older file that predates most fields used to reset EVERYTHING.
        let old = r#"{"hotkey":"Ctrl+Shift+Space","wakeSensitivity":"high","groqApiKey":"k"}"#;
        let (s, dropped) = parse_settings_lenient(old).expect("parses");
        assert_eq!(s.hotkey, "Ctrl+Shift+Space");
        assert_eq!(s.wake_sensitivity, "high");
        assert_eq!(s.groq_api_key, "k");
        assert!(dropped.is_empty());
    }

    #[test]
    fn lenient_parse_valid_file_is_untouched_and_garbage_is_none() {
        let full = serde_json::to_string(&NexusSettings::default()).unwrap();
        let (s, dropped) = parse_settings_lenient(&full).unwrap();
        assert!(dropped.is_empty());
        assert_eq!(s.hotkey, NexusSettings::default().hotkey);
        assert!(parse_settings_lenient("{not json").is_none());
        assert!(parse_settings_lenient("[1,2]").is_none());
        assert!(parse_settings_lenient("").is_none());
    }
}
