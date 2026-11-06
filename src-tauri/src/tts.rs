//! TTS — 2-tier fallback: edge-tts (cloud) → Kokoro (local).
//!
//! Phase 2 architecture:
//!   Primary:   edge-tts (Microsoft Neural, cloud, $0, ~200ms, 0 MB RAM)
//!   Fallback:  Kokoro-82M (local ONNX, $0, ~real-time on CPU, ~260-370 MB RAM while loaded,
//!              lazy-loaded + unloaded after 10 min of stable network). Piper was removed (GPL espeak-ng).
//!
//! Cached acknowledgment phrases ("On it sir", etc.) are pre-synthesized at
//! boot using edge-tts and stored as f32 PCM in RAM. Playback is <5ms
//! regardless of which engine generated them.

use crate::meeting_detect::MeetingState;
use rodio::{buffer::SamplesBuffer, OutputStream, Sink};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tauri::{Emitter, State};

/// Global Kokoro engine reference — set at TtsState creation. Lets the network monitor unload the
/// engine and the streaming producer task synthesize locally without borrowing TtsState.
static GLOBAL_KOKORO_ENGINE: std::sync::OnceLock<crate::tts_kokoro::KokoroEngine> =
    std::sync::OnceLock::new();

/// Where short voice lines are persisted (set once at start-up).
static DISK_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Enable the on-disk voice cache under the app data folder.
pub fn init_disk_cache(app_data: &std::path::Path) {
    let _ = DISK_DIR.set(crate::tts_cache_disk::dir_for(app_data));
}

/// Speech calls in flight (speak_text / speak_cached). Background cache rendering waits for 0 so it
/// never competes with a line the user is waiting for.
static LIVE_SPEECH: AtomicUsize = AtomicUsize::new(0);

/// True while any speech call is in flight. The voice-swap worker waits for false so a voice change
/// is applied between utterances, never mid-reply (Feature 83).
pub fn speech_active() -> bool {
    LIVE_SPEECH.load(Ordering::SeqCst) > 0
}

struct LiveSpeech;

impl LiveSpeech {
    fn begin() -> Self {
        LIVE_SPEECH.fetch_add(1, Ordering::SeqCst);
        LiveSpeech
    }
}

impl Drop for LiveSpeech {
    fn drop(&mut self) {
        LIVE_SPEECH.fetch_sub(1, Ordering::SeqCst);
    }
}

/// First audio of a line voiced by the local neural voice must be ready within this long, or the
/// Windows voice speaks it instead (measured: 10-45 ms; the local voice took 6-20 s per line in the
/// 2026-10-15 logs). A cold local engine never gets the chance: it loads in the background.
const LOCAL_FIRST_AUDIO_BUDGET_MS: u64 = 2_000;
/// Pause before the single retry of a failed cloud sentence in a streamed reply.
const EDGE_RETRY_PAUSE_MS: u64 = 450;

/// A healthy cloud voice answers in 0.2-0.6 s. Waiting 8 s for one that hangs (then another 2 s for the
/// local voice) outlasted the orb's 10 s "stuck speaking" watchdog; 4 s keeps the worst case under it.
const EDGE_FIRST_AUDIO_TIMEOUT_SECS: u64 = 4;

fn words_to_tuples(c: &CaptionTrack) -> Vec<(String, u64, u64)> {
    c.words.iter().map(|w| (w.text.clone(), w.start_ms, w.duration_ms)).collect()
}

fn cached_from_loaded(l: crate::tts_cache_disk::Loaded) -> CachedAudio {
    let words = l
        .words
        .iter()
        .map(|(t, s, d)| CaptionWord { text: t.clone(), start_ms: *s, duration_ms: *d })
        .collect();
    let envelope = compute_envelope(&l.samples, l.sample_rate, ENVELOPE_FRAME_MS);
    CachedAudio {
        sample_rate: l.sample_rate,
        caption: CaptionTrack {
            words,
            total_ms: l.total_ms,
            estimated: l.origin == crate::tts_cache_disk::Origin::Local,
            envelope,
            frame_ms: ENVELOPE_FRAME_MS,
            envelope_start_ms: 0,
            system: false,
        },
        samples: l.samples,
    }
}

/// Remember a freshly rendered short line on disk (background, best effort). The Windows voice is
/// never stored: it is a stop-gap, not the persona.
fn persist_line_bg(text: &str, voice: &str, samples: &[f32], sample_rate: u32, caption: &CaptionTrack) {
    let Some(dir) = DISK_DIR.get().cloned() else { return };
    if caption.system || !crate::tts_cache_disk::cacheable(text) {
        return;
    }
    let origin = if caption.estimated {
        crate::tts_cache_disk::Origin::Local
    } else {
        crate::tts_cache_disk::Origin::Edge
    };
    let (text, voice) = (text.to_string(), voice.to_string());
    let samples = samples.to_vec();
    let words = words_to_tuples(caption);
    let total_ms = caption.total_ms;
    tokio::task::spawn_blocking(move || {
        if let Err(e) = crate::tts_cache_disk::save(&dir, &voice, &text, origin, &samples, sample_rate, total_ms, &words) {
            tracing::debug!("tts: could not persist '{}': {e}", text);
            return;
        }
        if origin == crate::tts_cache_disk::Origin::Edge {
            crate::tts_cache_disk::forget(&dir, &voice, &text, crate::tts_cache_disk::Origin::Local);
        }
    });
}

/// A line from the on-disk cache (best stored version), if any.
async fn disk_lookup(text: &str, voice: &str) -> Option<CachedAudio> {
    let dir = DISK_DIR.get().cloned()?;
    if !crate::tts_cache_disk::cacheable(text) {
        return None;
    }
    let (text, voice) = (text.to_string(), voice.to_string());
    tokio::task::spawn_blocking(move || crate::tts_cache_disk::load(&dir, &voice, &text))
        .await
        .ok()
        .flatten()
        .map(cached_from_loaded)
}

/// The Windows built-in voice as a full synthesis result (estimated word timings).
async fn system_voice_line(text: &str, reason: &str) -> Result<(Vec<f32>, u32, CaptionTrack), String> {
    let t0 = std::time::Instant::now();
    let (samples, sr) = crate::tts_system::synthesize(text).await?;
    let total_ms = samples.len() as u64 * 1000 / (sr as u64).max(1);
    let words = estimate_words(text, total_ms);
    let envelope = compute_envelope(&samples, sr, ENVELOPE_FRAME_MS);
    tracing::info!("[TTS] Windows voice used ({reason}) - ready in {} ms", t0.elapsed().as_millis());
    Ok((
        samples,
        sr,
        CaptionTrack { words, total_ms, estimated: true, envelope, frame_ms: ENVELOPE_FRAME_MS, envelope_start_ms: 0, system: true },
    ))
}

/// Local-neural voice with a hard first-audio deadline. Cold engine, or one that is slower than
/// `LOCAL_FIRST_AUDIO_BUDGET_MS`: the line is spoken NOW in the Windows voice while the neural
/// engine keeps loading/rendering in the background (so the next line can use it).
async fn synthesize_local_guarded(text: &str) -> Result<(Vec<f32>, u32, CaptionTrack), String> {
    let owned = text.to_string();
    let mut job = tokio::spawn(async move { synthesize_local(&owned).await });
    if !crate::tts_network::is_local_loaded() {
        return match system_voice_line(text, "local voice still loading").await {
            Ok(v) => Ok(v),
            Err(why) => {
                tracing::warn!("tts: system voice unavailable ({why}); waiting for the local voice");
                job.await.map_err(|e| format!("local voice join: {e}"))?
            }
        };
    }
    match tokio::time::timeout(std::time::Duration::from_millis(LOCAL_FIRST_AUDIO_BUDGET_MS), &mut job).await {
        Ok(joined) => joined.map_err(|e| format!("local voice join: {e}"))?,
        Err(_late) => match system_voice_line(text, "local voice too slow").await {
            Ok(v) => Ok(v),
            Err(why) => {
                tracing::warn!("tts: system voice unavailable ({why}); waiting for the local voice");
                job.await.map_err(|e| format!("local voice join: {e}"))?
            }
        },
    }
}

pub struct TtsState {
    /// Local (offline) engine: Kokoro, lazy-loaded only when the cloud is unavailable.
    pub kokoro_engine: crate::tts_kokoro::KokoroEngine,
    /// Pre-synthesized short phrases for instant acknowledgment playback.
    /// Keyed by the exact phrase text. Stores f32 PCM samples + sample rate.
    pub cache: Arc<Mutex<HashMap<String, CachedAudio>>>,
}

/// Cached audio: PCM samples + sample rate for rodio playback, plus the
/// response caption to emit alongside it (so cache-hit playback — the
/// instant <5ms path — gets a caption exactly like every other path).
#[derive(Clone)]
pub struct CachedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub caption: CaptionTrack,
}

static CACHE_REFRESHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A cached ack that was voiced by the LOCAL engine (its caption is `estimated`; the cloud path
/// has real word boundaries) must not be played once the cloud voice is available again - the ack
/// would sound like a different speaker than the reply that follows. Stale entries are skipped and
/// the whole cache is re-synthesized once in the background.
fn cache_entry_usable(
    hit: &CachedAudio,
    cache: &Arc<Mutex<HashMap<String, CachedAudio>>>,
    cloud_voice: &str,
) -> bool {
    // A local-voice line is stale when the cloud voice is available.
    let cloud_available = crate::tts_health::edge_recently_ok()
        || (crate::tts_network::is_network_up() && !crate::tts_health::edge_degraded());
    if !(hit.caption.estimated && !hit.caption.system && cloud_available) {
        return true;
    }
    if !CACHE_REFRESHING.swap(true, Ordering::SeqCst) {
        let cache = cache.clone();
        let voice = cloud_voice.to_string();
        tracing::info!("[TTS] cached ack is the local voice but the cloud voice works again; refreshing cache ({voice})");
        tokio::spawn(async move {
            crate::focus_mode::wait_until_clear("tts_cache_refresh").await;
            pregenerate_cache(&cache, &voice).await;
            CACHE_REFRESHING.store(false, Ordering::SeqCst);
        });
    }
    false
}

/// One word's reveal timing for the response caption (plan Phase 3).
/// `start_ms`/`duration_ms` are milliseconds from the start of the overall
/// utterance (for a streamed multi-chunk reply, chunk N's words already
/// have chunk 0..N-1's cumulative audio duration baked in — see
/// `play_audio_streamed`'s `cumulative_ms` — so the frontend never needs
/// to know chunking happened).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct CaptionWord {
    pub text: String,
    pub start_ms: u64,
    pub duration_ms: u64,
}

/// One utterance's (or one streamed chunk's) full caption track, emitted
/// as the `tts:caption` event right as its audio is appended to the sink.
/// `estimated` is true for the local (Kokoro) path (no real word-boundary
/// data — timings are evenly distributed across the known PCM duration).
///
/// `envelope`/`frame_ms`/`envelope_start_ms` (sub-phase C2) carry NEXUS's
/// own voice amplitude to the frontend so the speaking-state orb's "beat"
/// deformation reacts to the actual TTS audio instead of the microphone's
/// volume reused as a stand-in. `envelope_start_ms` mirrors the same
/// cumulative-offset bookkeeping `CaptionWord.start_ms` already uses for a
/// streamed multi-chunk reply (see `play_audio_streamed`) — both ride the
/// SAME absolute utterance timeline, so the frontend never special-cases
/// chunking for either one.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CaptionTrack {
    pub words: Vec<CaptionWord>,
    pub total_ms: u64,
    pub estimated: bool,
    pub envelope: Vec<f32>,
    pub frame_ms: u64,
    pub envelope_start_ms: u64,
    /// Voiced by the Windows built-in voice (the safety net). Internal: never sent to the frontend.
    #[serde(skip)]
    pub system: bool,
}

/// Evenly distribute `text`'s whitespace-split words across `total_ms` of
/// known audio duration. Used for the local Kokoro fallback (it has
/// no word-boundary events) so the frontend caption scheduler never needs
/// to special-case which engine spoke. Pure + unit-tested.
pub fn estimate_words(text: &str, total_ms: u64) -> Vec<CaptionWord> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let per = total_ms / words.len() as u64;
    words
        .iter()
        .enumerate()
        .map(|(i, w)| CaptionWord {
            text: (*w).to_string(),
            start_ms: i as u64 * per,
            duration_ms: per,
        })
        .collect()
}

/// Unescape common SSML/XML entities emitted by Microsoft Edge-TTS (e.g. "didn&apos;t").
pub fn unescape_ssml_entities(text: &str) -> String {
    text.replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// Convert edge-tts's real word-boundary events (100ns ticks) into
/// `CaptionWord`s, offset by `cumulative_ms` (non-zero for chunk 2+ of a
/// streamed reply — see `play_audio_streamed`). Pure + unit-tested.
pub fn boundaries_to_words(
    boundaries: &[edge_tts_rust::BoundaryEvent],
    cumulative_ms: u64,
) -> Vec<CaptionWord> {
    boundaries
        .iter()
        .map(|b| CaptionWord {
            text: unescape_ssml_entities(&b.text),
            start_ms: cumulative_ms + b.offset_ticks / 10_000,
            duration_ms: b.duration_ticks / 10_000,
        })
        .collect()
}

/// Frame size for the TTS amplitude envelope (sub-phase C2): short enough
/// to track syllable-level energy changes, long enough to stay cheap to
/// compute and transmit over IPC.
pub const ENVELOPE_FRAME_MS: u64 = 20;

/// Per-frame RMS envelope of `samples`, normalized so the loudest frame in
/// the track reads as 1.0 — absolute amplitude varies a lot between
/// engines/voices, so a fixed reference level would make quiet voices
/// barely move the orb and loud ones clip at 1.0 with no headroom. Silent
/// input returns an all-zero envelope rather than dividing by zero.
/// Pure + unit-tested.
pub fn compute_envelope(samples: &[f32], sample_rate: u32, frame_ms: u64) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 || frame_ms == 0 {
        return Vec::new();
    }
    let frame_len = ((sample_rate as u64 * frame_ms) / 1000).max(1) as usize;
    let mut raw: Vec<f32> = Vec::with_capacity(samples.len() / frame_len + 1);
    let mut peak = 0.0f32;
    for chunk in samples.chunks(frame_len) {
        let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
        let rms = (sum_sq / chunk.len() as f32).sqrt();
        peak = peak.max(rms);
        raw.push(rms);
    }
    if peak <= 1e-6 {
        return raw.iter().map(|_| 0.0).collect();
    }
    raw.into_iter().map(|v| (v / peak).clamp(0.0, 1.0)).collect()
}

/// Truncate text to at most `max_chars` CHARACTERS for log lines.
/// Byte-slicing (`&text[..n]`) panics on multibyte UTF-8 (measured
/// 2026-09-19: byte 50 inside 'の' → panic → `panic=abort` → whole app
/// dead). This can never panic by construction.
pub fn truncate_for_log(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

impl TtsState {
    pub fn new() -> Self {
        let kokoro_engine = crate::tts_kokoro::new_engine();
        // Store global reference for the network monitor / streaming producer to access
        let _ = GLOBAL_KOKORO_ENGINE.set(kokoro_engine.clone());
        Self {
            kokoro_engine,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

/// Unload the local engine globally (called by the network monitor after 10 min of stable network).
pub async fn unload_local_global() {
    if let Some(engine) = GLOBAL_KOKORO_ENGINE.get() {
        crate::tts_kokoro::unload(engine).await;
    }
}

/// Handle to the shared local engine (swap worker / startup sync).
pub fn kokoro_engine_handle() -> Option<crate::tts_kokoro::KokoroEngine> {
    GLOBAL_KOKORO_ENGINE.get().cloned()
}

/// Synthesize with the local Kokoro engine (loads it lazily). Used by every offline path.
async fn synthesize_local(text: &str) -> Result<(Vec<f32>, u32, CaptionTrack), String> {
    let engine = GLOBAL_KOKORO_ENGINE
        .get()
        .ok_or_else(|| "local TTS engine unavailable".to_string())?;
    crate::tts_kokoro::ensure_loaded(engine).await?;
    crate::tts_network::mark_local_loaded();
    let (samples, sr) = crate::tts_kokoro::synthesize(engine, text, 1.0).await?;
    let total_ms = samples.len() as u64 * 1000 / (sr as u64).max(1);
    let words = estimate_words(text, total_ms);
    let envelope = compute_envelope(&samples, sr, ENVELOPE_FRAME_MS);
    Ok((
        samples,
        sr,
        CaptionTrack { words, total_ms, estimated: true, envelope, frame_ms: ENVELOPE_FRAME_MS, envelope_start_ms: 0, system: false },
    ))
}

/// Stock lines that must play with ZERO synthesis delay. Rendered once, kept on disk
/// (`tts_cache_disk`), loaded into memory at start-up. Any other short line is persisted
/// automatically the first time it is synthesised, so frequent replies become instant too.
const CACHED_PHRASES: &[&str] = &[
    "On it sir",
    "Right away sir",
    "Working on it sir",
    "Give me a minute sir",
    "Let me check that sir",
    "Didn't understand that sir",
    "Didn't catch that sir",
    "Here is the analysis, sir",
    "Ok sir",
    "Ready to search, sir",
    "Stopped typing, sir",
    "Opening Command Hub, sir",
    "Annotation ready, sir",
    "One moment sir",
    "New tab sir",
    "Hello, sir",
    "Stopped, sir",
    "Queued, sir",
    "Hand mode on, sir",
    "Hand mode off, sir. The camera is off",
    "Ghost mode off, sir",
    "Ghost mode initialized, sir. Tell me what to do",
    "Muted, sir",
    "Minimized, sir",
    "Maximized, sir",
    "Screenshot taken, sir",
    "Didn't find that, sir",
    "Nothing running, sir",
    "Good morning, sir",
    "Good afternoon, sir",
    "Good evening, sir",
    "Good night, sir",
    // From 18:00 the hello is "Welcome back" (early_hello + briefing); rendered once per voice, then read from disk at logon.
    "Welcome back, sir",
    "Good to have you back, sir",
    "Shutting down, sir. See you soon",
];

fn insert_cached(map: &mut HashMap<String, CachedAudio>, phrase: &str, audio: CachedAudio) {
    map.insert(format!("{phrase}."), audio.clone());
    map.insert(phrase.to_string(), audio);
}

/// Wait (bounded) until no spoken line is being prepared, so background rendering never delays one.
async fn wait_for_speech_idle(max: std::time::Duration) {
    let start = std::time::Instant::now();
    while LIVE_SPEECH.load(Ordering::SeqCst) > 0 && start.elapsed() < max {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// Fill the in-memory cache with the stock lines.
///
/// 1. from DISK (instant, nothing is synthesised);
/// 2. missing lines from the cloud voice, only while the breaker says it is healthy;
/// 3. still missing: the local persona voice, one line at a time, only while nobody is waiting for speech.
/// Everything rendered is persisted, so the next start does none of this.
pub async fn pregenerate_cache(
    cache_arc: &Arc<Mutex<HashMap<String, CachedAudio>>>,
    voice: &str,
) {
    let cache_start = std::time::Instant::now();
    crate::focus_mode::wait_until_clear("tts_pregenerate_cache").await;
    let (mut from_disk, mut from_edge, mut from_local) = (0usize, 0usize, 0usize);
    // 2026-10-15 log: 29 back-to-back cloud requests drew "429 Too Many Requests", and the throttle then
    // also hit the user's first real reply. Pace the cloud requests and stop using it for this pass at
    // the first 429 (the rest are rendered locally and upgraded on a later boot).
    let mut cloud_throttled = false;

    for phrase in CACHED_PHRASES {
        // 1. disk
        if let Some(audio) = disk_lookup(phrase, voice).await {
            let cloud_available = crate::tts_health::edge_recently_ok()
                || (crate::tts_network::is_network_up() && !crate::tts_health::edge_degraded());
            let stale = audio.caption.estimated && cloud_available;
            if !stale {
                insert_cached(&mut *cache_arc.lock().await, phrase, audio);
                from_disk += 1;
                continue;
            }
        }
        // 2. cloud voice (skipped while the breaker is open: no 403 storm)
        if !crate::tts_health::edge_degraded() && !cloud_throttled {
            // Real user speech outranks stock lines: never compete with it for the cloud voice.
            wait_for_speech_idle(std::time::Duration::from_secs(30)).await;
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            match tokio::time::timeout(
                std::time::Duration::from_secs(8),
                crate::tts_edge::synthesize_to_pcm(phrase, voice),
            )
            .await
            {
                Ok(Ok((samples, sr, boundaries))) => {
                    crate::tts_health::edge_note_ok();
                    let total_ms = samples.len() as u64 * 1000 / (sr as u64).max(1);
                    let words = boundaries_to_words(&boundaries, 0);
                    let envelope = compute_envelope(&samples, sr, ENVELOPE_FRAME_MS);
                    let caption = CaptionTrack { words, total_ms, estimated: false, envelope, frame_ms: ENVELOPE_FRAME_MS, envelope_start_ms: 0, system: false };
                    persist_line_bg(phrase, voice, &samples, sr, &caption);
                    insert_cached(&mut *cache_arc.lock().await, phrase, CachedAudio { samples, sample_rate: sr, caption });
                    from_edge += 1;
                    continue;
                }
                Ok(Err(e)) => {
                    if e.contains("429") {
                        cloud_throttled = true;
                    } else {
                        crate::tts_health::edge_note_fail(&e);
                    }
                    tracing::warn!("tts: edge-tts cache failed for '{}': {}", phrase, e);
                }
                Err(_) => {
                    crate::tts_health::edge_note_fail("timeout");
                    tracing::warn!("tts: edge-tts cache timed out for '{}'", phrase);
                }
            }
        }
        // 3. local persona voice, politely
        wait_for_speech_idle(std::time::Duration::from_secs(30)).await;
        match synthesize_local(phrase).await {
            Ok((samples, sr, caption)) => {
                persist_line_bg(phrase, voice, &samples, sr, &caption);
                insert_cached(&mut *cache_arc.lock().await, phrase, CachedAudio { samples, sample_rate: sr, caption });
                from_local += 1;
            }
            Err(e) => tracing::warn!("tts: local cache failed for '{}': {}", phrase, e),
        }
    }

    tracing::info!(
        "tts: stock lines ready in {:.2}s ({} from disk, {} from the cloud voice, {} rendered locally)",
        cache_start.elapsed().as_secs_f32(),
        from_disk,
        from_edge,
        from_local
    );
}

/// Global generation counter: incremented by `stop_tts` to signal the playback thread
/// to stop the current audio immediately.
static TTS_GENERATION: AtomicUsize = AtomicUsize::new(0);

/// The generation the last stop produced. A hotkey, a barge-in, the frontend and the stage all ask for a stop
/// within about 20 ms of each other; only the first has anything to cancel, and a later one must not cancel a
/// line that began after it (the acknowledgement of the turn that the press just started).
static LAST_STOP_GENERATION: std::sync::Mutex<usize> = std::sync::Mutex::new(usize::MAX);

/// Does a stop request still have a playing or pending utterance to cancel? Pure.
fn stop_has_work(current: usize, last_stop: usize) -> bool {
    current != last_stop
}

/// IPC: Stop any currently-playing TTS audio and immediately restore system volume.
#[tauri::command]
pub fn stop_tts() -> Result<(), String> {
    {
        let mut last = LAST_STOP_GENERATION.lock().unwrap_or_else(|e| e.into_inner());
        if stop_has_work(TTS_GENERATION.load(Ordering::SeqCst), *last) {
            *last = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        }
    }
    crate::speech_sync::cancel_current();
    tracing::info!("tts: stop requested (generation {})", TTS_GENERATION.load(Ordering::SeqCst));
    // Deferred: the frontend calls stopTts() right before every speak(), so restoring instantly would
    // dip the volume between the old utterance and the new one. Stale leases end now; the original
    // volume returns after a short grace unless the next utterance claims it.
    crate::volume::defer_restore_volume();
    Ok(())
}

/// IPC: Explicitly restore system volume to baseline.
#[tauri::command]
pub fn restore_tts_volume() -> Result<(), String> {
    crate::volume::force_restore_volume();
    Ok(())
}

/// IPC: Speak text using the 3-tier fallback chain.
///
/// Tries edge-tts (cloud) first, then the local Kokoro voice (offline only).
/// For cached phrases, plays instantly from memory (<5ms).
#[tauri::command]
pub async fn speak_text(
    text: String,
    voice: Option<String>,
    _speed: Option<f32>,
    request_id: Option<String>,
    source_center: Option<String>,
    kind: Option<crate::speech_sync::SpeechKind>,
    state: State<'_, TtsState>,
    meeting: State<'_, Arc<MeetingState>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    tracing::info!("tts: speaking '{}'", text);
    crate::directed::note_spoken(&text); // echo rejection for open-mic turns (Phase 8)
    let _live = LiveSpeech::begin();

    meeting.set_tts_playing(true);
    let my_generation = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    crate::speech_sync::register_intent(my_generation as u64, request_id.clone(), source_center, kind);

    // Read settings
    let edge_voice = crate::commands::read_edge_tts_voice(&app);
    let voice_id = voice.unwrap_or_else(|| edge_voice.clone());
    let tts_volume_pct = read_tts_volume(&app);
    // B3: emotional prosody (settings override or auto heuristics).
    let emotion = resolve_emotion(&app, &text);

    // Fast path: exact-match cache (<5ms, no network, no synthesis).
    // Acks like "Ok sir." were pre-generated at boot — replay them instead
    // of re-synthesizing (which previously paid a probe + cold-engine cost).
    // Falls through to synthesis on miss.
    let trimmed = text.trim().trim_end_matches(['.', '!', '?', ',', ';']).trim();
    let mut memory_hit = {
        let guard = state.cache.lock().await;
        guard
            .get(&text)
            .or_else(|| guard.get(trimmed))
            .filter(|h| cache_entry_usable(h, &state.cache, &edge_voice))
            .cloned()
    };
    if memory_hit.is_none() {
        memory_hit = disk_lookup(&text, &edge_voice).await.filter(|h| cache_entry_usable(h, &state.cache, &edge_voice));
    }
    let (audio, sample_rate, caption) = if let Some(hit) = memory_hit {
        tracing::info!("tts: cache hit for '{}'", text);
        (hit.samples, hit.sample_rate, hit.caption)
    } else {
        // B2 streaming: long multi-sentence replies play chunk 0
        // immediately while the rest synthesizes. Short texts keep the
        // legacy single-shot path (zero behavior change). While OFFLINE the
        // local engine runs ~real-time, so ANY multi-sentence reply streams
        // (first audio = one sentence, not the whole reply).
        let sentences = plan_chunks(&text);
        let use_streaming = sentences.len() > 1
            && (text.chars().count() >= STREAM_MIN_CHARS || !crate::tts_network::is_network_up());
        if use_streaming {
            // Set volume BEFORE playback (same as legacy path).
            let stream_volume_lease = if tts_volume_pct > 0 {
                let target = tts_volume_pct as f32 / 100.0;
                crate::volume::acquire_volume_lease(target)
            } else {
                None
            };
            // Kept awake regardless of the volume setting above; released by normal scope-end
            // drop (slightly later than the manual volume-lease drops below is harmless here —
            // unlike volume there is no audible defect from releasing a beat late).
            let _stream_keep_awake_lease = crate::keep_awake::acquire_lease();
            match speak_streaming(sentences, &voice_id, &state, my_generation, emotion, app.clone()).await {
                Ok(()) => {
                    drop(stream_volume_lease);
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    meeting.set_tts_playing(false);
                    return Ok(());
                }
                Err(e) => {
                    if e.starts_with("Speech stage") || e.starts_with("Speech superseded") {
                        drop(stream_volume_lease);
                        meeting.set_tts_playing(false);
                        return Err(e);
                    }
                    tracing::warn!("tts: streaming failed ({}), falling back to single-shot", e);
                    drop(stream_volume_lease);
                    // Fall through to legacy single-shot synthesis below.
                    match synthesize_with_fallback(&text, &voice_id, &state, emotion).await {
                        Ok(result) => result,
                        Err(e) => {
                            tracing::error!("tts: all TTS engines failed: {}", e);
                            meeting.set_tts_playing(false);
                            return Err(e);
                        }
                    }
                }
            }
        } else {
            // Try to synthesize using the 2-tier fallback chain
            match synthesize_with_fallback(&text, &voice_id, &state, emotion).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("tts: all TTS engines failed: {}", e);
                    meeting.set_tts_playing(false);
                    return Err(e);
                }
            }
        }
    };

    // Check if stop was requested during synthesis
    if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
        tracing::info!("tts: stop requested during synthesis, skipping playback");
        // Do NOT set tts_playing=false here. A newer TTS call (higher
        // generation) is already active and has set tts_playing=true.
        // If we set it false, the wake word detector becomes un-suppressed
        // during the newer TTS playback, causing false wakes from TTS echo.
        // Only the current (newest) generation should clear the flag.
        return Ok(());
    }

    tracing::info!(
        "[TTS] engine={} voice={} volume={}% req={}",
        if caption.estimated { "local" } else { "edge" },
        voice_id,
        tts_volume_pct,
        my_generation
    );
    // Playback-start signal (ack deadline guard, live log 2026-10-19): the
    // frontend guarantees an ack within 2 s (net/orchestrator.ts) because a
    // dynamic repeat-back ack must synthesize live — once the Edge synth
    // stalled ~27 s, leaving the turn silent with a pinned orb and no
    // loader. Emit here, AFTER synthesis/cache-hit, right before rodio
    // starts: the only truthful "audio is actually playing now" moment the
    // frontend can observe (its own tts-started fires BEFORE synthesis).
    crate::commands::emit_logged(&app, "tts:playback_started", serde_json::json!({
        "requestId": request_id.clone(),
        "voiceId": voice_id,
    }));
    // Save and set system volume with RAII lease
    let volume_lease = if tts_volume_pct > 0 {
        let target = tts_volume_pct as f32 / 100.0;
        crate::volume::acquire_volume_lease(target)
    } else {
        None
    };
    let _keep_awake_lease = crate::keep_awake::acquire_lease();

    // Play audio
    let play_result = play_audio(audio, sample_rate, my_generation, caption, app.clone()).await;

    // Release lease
    drop(volume_lease);

    // Grace period
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    meeting.set_tts_playing(false);

    play_result
}

/// IPC: Preview a specific TTS voice with a short demo phrase.
/// Used by the settings sidebar voice picker — plays a demo without
/// saving the voice as the default. Does NOT change system volume.
#[tauri::command]
pub async fn preview_voice(
    voice_id: String,
    text: Option<String>,
    state: State<'_, TtsState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let demo_text = text.unwrap_or_else(|| {
        if voice_id.starts_with("en-US-") {
            let name = voice_id
                .strip_prefix("en-US-")
                .unwrap_or("")
                .trim_end_matches("Neural");
            format!("Hello, I'm {}. This is how I sound.", name)
        } else {
            "Hello, this is a voice preview.".to_string()
        }
    });

    tracing::info!("tts: previewing voice '{}' with text '{}'", voice_id, demo_text);

    let my_generation = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    let (audio, sample_rate, caption) = match synthesize_with_fallback(&demo_text, &voice_id, &state, crate::tts_edge::TtsEmotion::Neutral).await {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("tts: voice preview failed for '{}': {}", voice_id, e);
            return Err(e);
        }
    };

    if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
        return Ok(());
    }

    play_audio(audio, sample_rate, my_generation, caption, app).await
}

/// Feature 83 — equip result returned synchronously by set_voice_preference
/// so the hub can update the card without waiting for the status event.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceEquipResult {
    pub voice_key: String,
    pub cloud_id: String,
    pub sync_state: crate::voice_catalog::VoiceSyncState,
}

/// IPC: Equip an iconic voice persona (Feature 83).
/// Validates the key against the voice catalog, writes selected_voice +
/// edge_tts_voice + offline_voice_model to settings.json (instant cloud
/// switch — next utterance speaks in the new voice), kicks a background
/// Fast-ACK cache re-synthesis, and emits `voice:status` (ready when the
/// local twin is already in the single slot, cloud-only otherwise — the
/// P3 swap worker fills the download in).
#[tauri::command]
pub async fn set_voice_preference(
    voice_key: String,
    state: State<'_, TtsState>,
    app: tauri::AppHandle,
) -> Result<VoiceEquipResult, String> {
    use tauri::{Emitter, Manager};
    let persona = crate::voice_catalog::find_by_key(&voice_key)
        .ok_or_else(|| format!("Unknown voice '{voice_key}'"))?;

    // Persist the selection (same write path as save_settings, identity-safe:
    // only the three voice fields change).
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("settings.json");
    let mut settings: crate::commands::NexusSettings = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();
    settings.selected_voice = persona.key.to_string();
    settings.edge_tts_voice = persona.cloud_id.to_string();
    settings.tts_voice = persona.cloud_id.to_string();
    settings.tts_provider = "edge".to_string();
    settings.offline_voice_model = persona.kokoro_voice.to_string();
    crate::tts_kokoro::set_preferred_voice(persona.kokoro_voice);
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    tracing::info!(
        "tts: voice equipped '{}' (cloud={}, twin={})",
        persona.key,
        persona.cloud_id,
        persona.kokoro_voice
    );

    // Fast-ACK re-synthesis in the background (new voice, ~350ms).
    // Clear old cache immediately so no stale voice audio can leak!
    {
        let cache = state.cache.clone();
        cache.lock().await.clear();
        let cloud_id = persona.cloud_id.to_string();
        tokio::spawn(async move {
            crate::focus_mode::wait_until_clear("tts_cache_refresh").await;
            pregenerate_cache(&cache, &cloud_id).await;
        });
    }

    // Sync state of the local twin. Twin present → ready now; missing →
    // emit downloading(0) and spawn the P3 swap worker (progress + final
    // ready/error events land as the download proceeds).
    let sync_state = crate::voice_catalog::sync_state_for(persona.key);
    if sync_state == crate::voice_catalog::VoiceSyncState::Ready {
        let _ = app.emit(
            "voice:status",
            serde_json::json!({ "status": sync_state, "voice_key": persona.key }),
        );
    } else {
        // Latest wins: a rapid A -> B -> C ends with C's offline voice installed.
        crate::tts_swap::request_swap(app.clone(), persona.key.to_string(), state.kokoro_engine.clone());
    }

    Ok(VoiceEquipResult {
        voice_key: persona.key.to_string(),
        cloud_id: persona.cloud_id.to_string(),
        sync_state,
    })
}

#[cfg(test)]
mod voice_preference_tests {
    use super::*;

    #[test]
    fn test_equip_result_serializes_camel_case() {
        let r = VoiceEquipResult {
            voice_key: "jarvis".into(),
            cloud_id: "en-GB-RyanNeural".into(),
            sync_state: crate::voice_catalog::VoiceSyncState::Ready,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["voiceKey"], "jarvis");
        assert_eq!(v["cloudId"], "en-GB-RyanNeural");
        assert_eq!(v["syncState"], "ready");
    }

    #[test]
    fn test_sync_state_serializes_cloud_only() {
        let v = serde_json::to_value(crate::voice_catalog::VoiceSyncState::CloudOnly).unwrap();
        assert_eq!(v, "cloud_only");
    }
}

/// IPC: List the iconic voice catalog for the hub grid (single source of
/// truth lives in voice_catalog.rs — the frontend never hardcodes it).
#[tauri::command]
pub fn list_voice_personas() -> Result<Vec<crate::voice_catalog::VoicePersona>, String> {
    Ok(crate::voice_catalog::VOICE_CATALOG.to_vec())
}

/// IPC: Current synthesis transport for the hub header dot
/// ("cloud" = next utterance goes Edge, "local" = next goes Kokoro).
/// Reads the live network flag — no probe, no cost.
#[tauri::command]
pub fn get_voice_transport() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "transport": if crate::tts_network::is_network_up() { "cloud" } else { "local" },
    }))
}

/// IPC: Sync state of one persona's offline twin ("ready" / "downloading" / "cloud_only"),
/// used by the Settings voice cards.
#[tauri::command]
pub fn get_voice_sync_state(voice_key: String) -> Result<String, String> {
    if crate::tts_swap::swap_target().as_deref() == Some(voice_key.as_str()) {
        return Ok("downloading".to_string());
    }
    Ok(match crate::voice_catalog::sync_state_for(&voice_key) {
        crate::voice_catalog::VoiceSyncState::Ready => "ready",
        crate::voice_catalog::VoiceSyncState::CloudOnly => "cloud_only",
    }
    .to_string())
}

/// IPC: Sync state of the currently selected persona's offline twin
/// (hub reads this on mount for the card pill: ready vs cloud-only).
/// Reports "downloading" truthfully while the swap worker runs.
#[tauri::command]
pub fn get_voice_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<serde_json::Value, String> {
    let key = crate::commands::read_selected_voice(&app);
    let persona = crate::voice_catalog::find_by_key(&key);
    let downloading = crate::tts_swap::swap_target().as_deref() == Some(key.as_str());
    let sync_state = if downloading {
        "downloading"
    } else {
        match crate::voice_catalog::sync_state_for(&key) {
            crate::voice_catalog::VoiceSyncState::Ready => "ready",
            crate::voice_catalog::VoiceSyncState::CloudOnly => "cloud_only",
        }
    };
    Ok(serde_json::json!({
        "voice_key": key,
        "cloud_id": persona.map(|p| p.cloud_id).unwrap_or(""),
        "twin": persona.map(|p| p.kokoro_voice).unwrap_or(""),
        "sync_state": sync_state,
    }))
}
///
/// Falls back to `speak_text` if the phrase is not in the cache.
#[tauri::command]
pub async fn speak_cached(
    text: String,
    request_id: Option<String>,
    source_center: Option<String>,
    kind: Option<crate::speech_sync::SpeechKind>,
    state: State<'_, TtsState>,
    meeting: State<'_, Arc<MeetingState>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    tracing::info!("tts: speaking cached '{}'", text);
    crate::directed::note_spoken(&text);
    let _live = LiveSpeech::begin();

    meeting.set_tts_playing(true);
    let my_generation = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    crate::speech_sync::register_intent(my_generation as u64, request_id, source_center, kind);

    // Try to get cached audio
    let cache_arc = state.cache.clone();
    let cloud_voice = crate::commands::read_edge_tts_voice(&app);
    let mut cached_audio = {
        let cache = cache_arc.lock().await;
        cache
            .get(&text)
            .filter(|h| cache_entry_usable(h, &cache_arc, &cloud_voice))
            .cloned()
    };
    if cached_audio.is_none() {
        cached_audio = disk_lookup(&text, &cloud_voice).await.filter(|h| cache_entry_usable(h, &cache_arc, &cloud_voice));
    }

    let (audio, sample_rate, caption) = match cached_audio {
        Some(ca) => {
            tracing::info!("tts: cache hit for '{}'", text);
            (ca.samples, ca.sample_rate, ca.caption)
        }
        None => {
            // Cache miss — synthesize on demand
            tracing::info!("tts: cache miss for '{}', synthesizing on demand", text);
            let edge_voice = crate::commands::read_edge_tts_voice(&app);
            match synthesize_with_fallback(&text, &edge_voice, &state, crate::tts_edge::TtsEmotion::Neutral).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("tts: synthesis failed for cached phrase: {}", e);
                    meeting.set_tts_playing(false);
                    return Err(e);
                }
            }
        }
    };

    // Check if stop was requested
    if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
        tracing::info!("tts: stop requested before cached playback, skipping");
        // Do NOT set tts_playing=false — a newer generation is active.
        return Ok(());
    }

    // Read TTS volume and acquire lease
    let tts_volume_pct = read_tts_volume(&app);
    let volume_lease = if tts_volume_pct > 0 {
        let target = tts_volume_pct as f32 / 100.0;
        crate::volume::acquire_volume_lease(target)
    } else {
        None
    };
    let _keep_awake_lease = crate::keep_awake::acquire_lease();

    // Play
    let play_result = play_audio(audio, sample_rate, my_generation, caption, app.clone()).await;

    // Release lease
    drop(volume_lease);

    // Grace period
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    meeting.set_tts_playing(false);

    play_result
}

/// 2-tier synthesis fallback: edge-tts (cloud, primary) → Kokoro (local, only when the cloud is
/// unavailable). Online is ALWAYS tried first; the network watchdog flips back to the cloud as soon
/// as it returns (`tts_network`).
///
/// Returns (f32 PCM samples, sample_rate, response caption) on success.
/// The caption is `estimated: true` for the local path (no real word-boundary
/// data), `false` for edge-tts (real boundaries).
async fn synthesize_with_fallback(
    text: &str,
    voice: &str,
    state: &TtsState,
    emotion: crate::tts_edge::TtsEmotion,
) -> Result<(Vec<f32>, u32, CaptionTrack), String> {
    let _ = state; // the local engine is reached through the global handle (shared with the streaming producer)
    // Upfront engine policy (Main Center TTS director, doc 74 P3):
    // network-down skips Edge entirely (saves
    // ~1-2s of waiting for Edge to fail). Identical branches to before —
    // the policy is now pinned + tested in `center::tts_engine_for`.
    //
    // NOTE: this is the CACHED flag only (maintained by the background
    // monitor + synthesis outcomes below). We used to run a live HTTPS
    // probe here on every call — it lied (reported down while Groq worked)
    // and added up to 5s before synthesis even started. The Edge attempt
    // itself, raced with a timeout, is the only honest connectivity check.
    // A legacy `piper-*` voice id (selected in an older build's picker) means "the offline voice".
    if crate::center::tts_engine_for(voice.starts_with("piper-"), crate::tts_network::is_network_up())
        == crate::center::TtsEngine::Local
    {
        tracing::info!("tts: network down (cached) — using local Kokoro directly (skipping Edge TTS)");
    } else if crate::tts_health::edge_degraded() {
        tracing::info!("tts: cloud voice marked broken - skipping Edge TTS for this line");
    } else {
        // Tier 1: edge-tts (cloud, ~200ms, best quality, 0 MB RAM),
        // raced with a timeout so a hanging endpoint can't stall speech.
        match tokio::time::timeout(
            std::time::Duration::from_secs(EDGE_FIRST_AUDIO_TIMEOUT_SECS),
            crate::tts_edge::synthesize_to_pcm_with_emotion(text, voice, emotion),
        )
        .await
        {
            Ok(Ok((samples, sr, boundaries))) => {
                tracing::info!("tts: edge-tts synthesis OK (cloud)");
                crate::tts_network::set_network_up();
                crate::tts_health::edge_note_ok();
                let total_ms = samples.len() as u64 * 1000 / (sr as u64).max(1);
                let words = boundaries_to_words(&boundaries, 0);
                let envelope = compute_envelope(&samples, sr, ENVELOPE_FRAME_MS);
                let caption = CaptionTrack { words, total_ms, estimated: false, envelope, frame_ms: ENVELOPE_FRAME_MS, envelope_start_ms: 0, system: false };
                persist_line_bg(text, voice, &samples, sr, &caption);
                return Ok((samples, sr, caption));
            }
            Err(_elapsed) => {
                // Timeout = genuine transport failure → mark down so the
                // next call skips Edge (saves 8s per call while offline).
                tracing::warn!("tts: edge-tts timed out, trying local fallback");
                crate::tts_network::set_network_down();
                crate::tts_health::edge_note_fail("timeout");
            }
            Ok(Err(e)) => {
                // Content/API rejection (bad voice, unsupported script, 4xx)
                // is NOT a network outage — a single bad sentence must not
                // poison the global flag (measured 2026-09-19: Japanese text
                // → English voice rejection → "network down" → forced local
                // voice → panic). Fall through to the local voice for THIS call only.
                tracing::warn!("tts: edge-tts rejected ({e}), trying local fallback (network flag untouched)");
                crate::tts_health::edge_note_fail(&e);
            }
        }
    }

    // Tier 2: Kokoro (local, ~real-time on CPU, ~260-370 MB RAM while loaded)
    match synthesize_local_guarded(text).await {
        Ok(result) => {
            tracing::info!("tts: local synthesis OK (offline voice, system={})", result.2.system);
            persist_line_bg(text, voice, &result.0, result.1, &result.2);
            Ok(result)
        }
        Err(e) => {
            tracing::error!("tts: local fallback also failed: {}", e);
            Err(format!("All TTS engines failed. Last error: {}", e))
        }
    }
}

/// Play f32 PCM audio through rodio with barge-in support. Emits
/// `tts:caption` right as the audio is appended to the sink (plan Phase 3)
/// — skipped if a barge-in already superseded this generation, so a
/// stale reply's caption never flashes on screen.
async fn play_audio(
    audio: Vec<f32>,
    sample_rate: u32,
    my_generation: usize,
    caption: CaptionTrack,
    app: tauri::AppHandle,
) -> Result<(), String> {
    crate::boot::first_audio();
    // A lease taken before synthesis may have been ended (sleep, barge-in): re-assert the speech volume now.
    let _playback_volume = crate::volume::ensure_live_lease(read_tts_volume(&app));
    tokio::task::spawn_blocking(move || {
        match OutputStream::try_default() {
            Ok((_stream, handle)) => {
                match Sink::try_new(&handle) {
                    Ok(sink) => {
                        if TTS_GENERATION.load(Ordering::SeqCst) > my_generation { return Ok(()); }
                        crate::speech_sync::prepare(my_generation as u64, caption.clone())?;
                        if TTS_GENERATION.load(Ordering::SeqCst) > my_generation { return Ok(()); }
                        crate::latency::first_audio();
                        let _ = app.emit("tts:caption", &caption);
                        let source = SamplesBuffer::new(1, sample_rate, audio);
                        sink.append(source);

                        // Poll for stop request (barge-in)
                        while !sink.empty() {
                            crate::speech_sync::playing(my_generation as u64, sink.get_pos().as_millis() as u64);
                            crate::volume::heartbeat();
                            crate::keep_awake::heartbeat();
                            if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
                                sink.stop();
                                crate::speech_sync::finish(my_generation as u64, "cancelled", 0);
                                tracing::info!("tts: playback stopped by user (barge-in)");
                                return Ok(());
                            }
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                        crate::speech_sync::finish(my_generation as u64, "finished", caption.total_ms);
                        tracing::info!("tts: audio playback completed");
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!("tts: failed to create audio sink: {}", e);
                        Err(format!("Failed to create audio sink: {}", e))
                    }
                }
            }
            Err(e) => {
                tracing::error!("tts: failed to open audio output: {}", e);
                Err(format!("Failed to get audio output stream: {}", e))
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        tracing::error!("tts: audio thread panicked");
        Err("Audio thread panicked".to_string())
    })
}

/// Resolve the effective emotion: zero-emotion directive (Doc 141) locks all utterances to Neutral.
fn resolve_emotion<R: tauri::Runtime>(_app: &tauri::AppHandle<R>, _text: &str) -> crate::tts_edge::TtsEmotion {
    crate::tts_edge::TtsEmotion::Neutral
}

/// Read the TTS volume setting from settings.json.
/// Returns 0-100. 0 means "disabled" (don't adjust system volume).
fn read_tts_volume<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> u8 {

    use tauri::Manager;
    let dir = match app.path().app_data_dir() {
        Ok(d) => d,
        Err(_) => return 75,
    };
    let path = dir.join("settings.json");
    if !path.exists() {
        return 75;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return 75,
    };
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
        if let Some(vol) = json.get("ttsVolume").and_then(|v| v.as_u64()) {
            return vol as u8;
        }
        if let Some(vol) = json.get("tts_volume").and_then(|v| v.as_u64()) {
            return vol as u8;
        }
    }
    75
}

/// Sentence-chunked streaming TTS (B2): split long replies into sentences,
/// synthesize + play the first chunk immediately, synthesize the rest while
/// it plays. First-audio latency ≈ one sentence instead of the full reply.
///
/// Short/single-sentence texts use the legacy single-shot path (zero
/// behavior change for the common case).
///
/// Speed first (owner, 2026-10-17): streaming starts from 60 characters instead of 150, so a normal
/// two-sentence answer speaks its first piece while the second is still being synthesized.
const STREAM_MIN_CHARS: usize = 60;

/// A first sentence longer than this many words is split at its first clause break so the first
/// audio arrives sooner (synthesis time grows with text length).
const FIRST_CLAUSE_MIN_WORDS: usize = 14;
/// The first piece keeps at least this many words, and the rest of the sentence at least `TAIL_MIN_WORDS`
/// (a 2-word first piece sounds clipped; a 1-word tail is a wasted request).
const FIRST_CLAUSE_HEAD_MIN_WORDS: usize = 4;
const FIRST_CLAUSE_TAIL_MIN_WORDS: usize = 3;

/// Split a long sentence at its first comma/semicolon/colon that leaves a sensible head and tail.
/// Never splits mid-clause (no comma = no split). Pure + unit-tested.
pub fn split_first_clause(sentence: &str) -> Option<(String, String)> {
    let words: Vec<&str> = sentence.split_whitespace().collect();
    if words.len() < FIRST_CLAUSE_MIN_WORDS {
        return None;
    }
    let last_head_end = words.len().checked_sub(FIRST_CLAUSE_TAIL_MIN_WORDS)?;
    for i in FIRST_CLAUSE_HEAD_MIN_WORDS..=last_head_end {
        let w = words[i - 1];
        if w.ends_with(',') || w.ends_with(';') || w.ends_with(':') {
            let head = words[..i].join(" ");
            let tail = words[i..].join(" ");
            return Some((head, tail));
        }
    }
    None
}

/// Sentence chunks for streaming, with the first sentence shortened at a clause break when it is long.
pub fn plan_chunks(text: &str) -> Vec<String> {
    let mut chunks = split_sentences(text);
    if let Some(first) = chunks.first().cloned() {
        if let Some((head, tail)) = split_first_clause(&first) {
            chunks[0] = head;
            chunks.insert(1, tail);
        }
    }
    chunks
}

/// Split text into sentence chunks (keeps delimiters). Pure + unit-tested.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            let s = cur.trim().to_string();
            if !s.is_empty() {
                out.push(s);
            }
            cur.clear();
        }
    }
    let tail = cur.trim().to_string();
    if !tail.is_empty() {
        out.push(tail);
    }
    if out.is_empty() {
        out.push(text.to_string());
    }
    out
}

/// Synthesize one chunk: per-sentence cache check, then the 2-tier fallback.
async fn synthesize_chunk(
    sentence: &str,
    voice_id: &str,
    state: &TtsState,
    emotion: crate::tts_edge::TtsEmotion,
) -> Result<(Vec<f32>, u32, CaptionTrack), String> {
    let trimmed = sentence.trim().trim_end_matches(['.', '!', '?', ',', ';']).trim();
    if let Some(hit) = {
        let guard = state.cache.lock().await;
        guard
            .get(sentence)
            .or_else(|| guard.get(trimmed))
            .filter(|h| cache_entry_usable(h, &state.cache, voice_id))
            .cloned()
    } {
        return Ok((hit.samples, hit.sample_rate, hit.caption));
    }
    synthesize_with_fallback(sentence, voice_id, state, emotion).await
}

/// Streaming playback: chunk 0 is already synthesized (plays immediately);
/// remaining chunks are synthesized while earlier audio plays. Barge-in
/// checked before each synthesis + append, and during the drain poll.
async fn speak_streaming(
    sentences: Vec<String>,
    voice_id: &str,
    state: &TtsState,
    my_generation: usize,
    emotion: crate::tts_edge::TtsEmotion,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Chunk 0 first — this defines first-audio latency.
    if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
        return Ok(());
    }
    let (first_audio, first_sr, first_caption) = synthesize_chunk(&sentences[0], voice_id, state, emotion).await?;
    // A cached phrase that was voiced locally should NOT force the rest of the reply to use Kokoro
    // if the network is up and Edge TTS is healthy.
    let utterance_local = first_caption.estimated && (!crate::tts_network::is_network_up() || crate::tts_health::edge_degraded());
    let utterance_system = first_caption.system;
    tracing::info!(
        "[TTS] streaming reply: engine={} voice={} chunks={}",
        if utterance_local { "local" } else { "edge" },
        voice_id,
        sentences.len()
    );
    let (stream_tx, stream_rx) =
        std::sync::mpsc::channel::<Option<(Vec<f32>, u32, CaptionTrack)>>();
    // Producer: synthesize remaining chunks while chunk 0 plays.
    let rest: Vec<String> = sentences.into_iter().skip(1).collect();
    let voice_owned = voice_id.to_string();
    let state_ref = TtsStateRef {
        cache: state.cache.clone(),
    };
    let gen = my_generation;
    tokio::spawn(async move {
        // If the cloud voice dies MID-reply (2026-10-15 log: chunk 1 failed twice, the rest of the answer
        // was never spoken and the orb sat silent for 10 s), the remaining sentences are finished in the
        // owner's local twin voice, and only if that also fails in the Windows voice (2026-10-28 log: the
        // briefing's tail was read by the Windows voice, a third voice nobody chose). A voice change is
        // better than a half-spoken answer; it happens at most once per reply, and only on failure.
        let mut engine = ChunkEngine::start(utterance_local, utterance_system);
        for s in rest {
            if TTS_GENERATION.load(Ordering::SeqCst) > gen {
                break;
            }
            let mut res = synthesize_chunk_with(engine, &s, &voice_owned, &state_ref, emotion).await;
            while let Err(e) = &res {
                let Some(next) = engine.after_failure() else { break };
                tracing::warn!("tts: streaming chunk failed ({e}); finishing the reply with {}", next.label());
                engine = next;
                res = synthesize_chunk_with(engine, &s, &voice_owned, &state_ref, emotion).await;
            }
            match res {
                Ok(triple) => {
                    if stream_tx.send(Some(triple)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("tts: streaming chunk failed ({}), playing what we have", e);
                    break;
                }
            }
        }
        let _ = stream_tx.send(None);
    });

    // Consumer: single rodio Sink, append chunks as they arrive.
    let first = Some((first_audio, first_sr, first_caption));
    play_audio_streamed(first, stream_rx, my_generation, app).await
}

/// Which voice speaks the rest of a streamed reply. Falls one step at a time, never back up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkEngine {
    Cloud,
    LocalTwin,
    WindowsVoice,
}

impl ChunkEngine {
    /// The engine chunk 0 was voiced by.
    fn start(local: bool, system: bool) -> Self {
        if system {
            Self::WindowsVoice
        } else if local {
            Self::LocalTwin
        } else {
            Self::Cloud
        }
    }

    /// The next voice to try after a failure: cloud, then the local twin, then the Windows voice last.
    fn after_failure(self) -> Option<Self> {
        match self {
            Self::Cloud => Some(Self::LocalTwin),
            Self::LocalTwin => Some(Self::WindowsVoice),
            Self::WindowsVoice => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cloud => "the cloud voice",
            Self::LocalTwin => "the local twin voice",
            Self::WindowsVoice => "the Windows voice",
        }
    }
}

async fn synthesize_chunk_with(
    engine: ChunkEngine,
    sentence: &str,
    voice_id: &str,
    state: &TtsStateRef,
    emotion: crate::tts_edge::TtsEmotion,
) -> Result<(Vec<f32>, u32, CaptionTrack), String> {
    match engine {
        ChunkEngine::Cloud => synthesize_chunk_owned(sentence, voice_id, state, emotion, false, false).await,
        ChunkEngine::LocalTwin => synthesize_chunk_owned(sentence, voice_id, state, emotion, true, false).await,
        ChunkEngine::WindowsVoice => synthesize_chunk_owned(sentence, voice_id, state, emotion, true, true).await,
    }
}

/// Minimal shared refs for the streaming producer task.
struct TtsStateRef {
    cache: Arc<Mutex<HashMap<String, CachedAudio>>>,
}

async fn synthesize_chunk_owned(
    sentence: &str,
    voice_id: &str,
    state: &TtsStateRef,
    emotion: crate::tts_edge::TtsEmotion,
    force_local: bool,
    force_system: bool,
) -> Result<(Vec<f32>, u32, CaptionTrack), String> {
    if let Some(hit) = state
        .cache
        .lock()
        .await
        .get(sentence)
        .filter(|h| force_local || cache_entry_usable(h, &state.cache, voice_id))
        .cloned()
    {
        return Ok((hit.samples, hit.sample_rate, hit.caption));
    }
    // ONE VOICE PER UTTERANCE. The engine is decided when the utterance starts (`force_local`
    // carries chunk 0's engine); a later sentence never silently switches to the other voice. The
    // cloud is retried once; if it still fails the caller plays timed silence / what it has, and
    // a transport failure flips the cached network flag so the NEXT utterance starts on the local
    // voice from its first word.
    if force_system {
        return system_voice_line(sentence, "last resort: cloud and local voices both failed").await;
    }
    if force_local || !crate::tts_network::is_network_up() || crate::tts_health::edge_degraded() {
        return synthesize_local(sentence).await;
    }
    let mut last_err = String::new();
    let mut edge_ok = None;
    for attempt in 1..=2 {
        match tokio::time::timeout(
            std::time::Duration::from_secs(8),
            crate::tts_edge::synthesize_to_pcm_with_emotion(sentence, voice_id, emotion),
        )
        .await
        {
            Ok(Ok(v)) => {
                crate::tts_health::edge_note_ok();
                edge_ok = Some(v);
                break;
            }
            Ok(Err(e)) => {
                crate::tts_health::edge_note_fail(&e);
                last_err = e;
            }
            Err(_) => {
                last_err = "edge-tts timed out".to_string();
                crate::tts_network::set_network_down();
                crate::tts_health::edge_note_fail("timeout");
            }
        }
        tracing::warn!("tts: edge chunk attempt {attempt}/2 failed ({last_err})");
        // The 2026-10-28 failure was "no audio was received" in the burst of connections right after boot; an
        // instant second try meets the same congestion, a short pause usually does not.
        if attempt == 1 {
            tokio::time::sleep(std::time::Duration::from_millis(EDGE_RETRY_PAUSE_MS)).await;
        }
    }
    let Some((samples, sr, boundaries)) = edge_ok else {
        return Err(format!("edge-tts failed after retry: {last_err}"));
    };
    // cumulative_ms is applied by the consumer (play_audio_streamed), which
    // is the only place that knows how much audio has already been queued —
    // the producer races ahead of playback and has no visibility into it.
    let total_ms = samples.len() as u64 * 1000 / (sr as u64).max(1);
    let words = boundaries_to_words(&boundaries, 0);
    let envelope = compute_envelope(&samples, sr, ENVELOPE_FRAME_MS);
    Ok((samples, sr, CaptionTrack { words, total_ms, estimated: false, envelope, frame_ms: ENVELOPE_FRAME_MS, envelope_start_ms: 0, system: false }))
}

/// Play chunk 0 immediately, then append streamed chunks as they arrive.
/// Each chunk's `tts:caption` is emitted right as it's appended to the
/// sink, with its word `start_ms` offset by `cumulative_ms` — the *actual
/// decoded PCM duration* of every previously-queued chunk (not any
/// engine-reported duration), so the frontend never needs to know
/// streaming happened at all: it just sees one utterance's words arrive
/// in a few batches, all already on one absolute timeline.
async fn play_audio_streamed(
    first: Option<(Vec<f32>, u32, CaptionTrack)>,
    rx: std::sync::mpsc::Receiver<Option<(Vec<f32>, u32, CaptionTrack)>>,
    my_generation: usize,
    app: tauri::AppHandle,
) -> Result<(), String> {
    crate::boot::first_audio();
    let _playback_volume = crate::volume::ensure_live_lease(read_tts_volume(&app));
    tokio::task::spawn_blocking(move || {
        match OutputStream::try_default() {
            Ok((_stream, handle)) => match Sink::try_new(&handle) {
                Ok(sink) => {
                    let mut cumulative_ms: u64 = 0;
                    let mut completed_ms: u64 = 0;
                    let mut queued_ms = std::collections::VecDeque::<u64>::new();
                    if let Some((audio, sr, mut caption)) = first {
                        caption.envelope_start_ms = cumulative_ms;
                        if TTS_GENERATION.load(Ordering::SeqCst) > my_generation { return Ok(()); }
                        crate::speech_sync::prepare(my_generation as u64, caption.clone())?;
                        if TTS_GENERATION.load(Ordering::SeqCst) > my_generation { return Ok(()); }
                        crate::latency::first_audio();
                        let _ = app.emit("tts:caption", &caption);
                        cumulative_ms += caption.total_ms;
                        queued_ms.push_back(caption.total_ms);
                        sink.append(SamplesBuffer::new(1, sr, audio));
                    }
                    // Drain the producer channel (blocking recv is fine here —
                    // we're on a blocking thread, audio plays async).
                    loop {
                        // Short timeout so the volume heartbeat keeps beating (and a barge-in is
                        // noticed) while the producer is still synthesizing the next sentence.
                        let msg = match rx.recv_timeout(std::time::Duration::from_millis(20)) {
                            Ok(m) => m,
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                while queued_ms.len() > sink.len() {
                                    completed_ms += queued_ms.pop_front().unwrap_or(0);
                                }
                                if !sink.empty() {
                                    crate::speech_sync::playing(my_generation as u64, completed_ms + sink.get_pos().as_millis() as u64);
                                }
                                crate::volume::heartbeat();
                                crate::keep_awake::heartbeat();
                                if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
                                    sink.stop();
                                    crate::speech_sync::finish(my_generation as u64, "cancelled", completed_ms);
                                    tracing::info!("tts: streaming stopped by user (barge-in)");
                                    return Ok(());
                                }
                                continue;
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        };
                        crate::volume::heartbeat();
                        crate::keep_awake::heartbeat();
                        if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
                            sink.stop();
                            crate::speech_sync::finish(my_generation as u64, "cancelled", completed_ms);
                            tracing::info!("tts: streaming stopped by user (barge-in)");
                            return Ok(());
                        }
                        match msg {
                            Some((audio, sr, mut caption)) => {
                                for w in caption.words.iter_mut() {
                                    w.start_ms += cumulative_ms;
                                }
                                caption.envelope_start_ms = cumulative_ms;
                                crate::latency::first_audio();
                                let _ = app.emit("tts:caption", &caption);
                                crate::speech_sync::chunk(my_generation as u64, caption.clone());
                                cumulative_ms += caption.total_ms;
                                queued_ms.push_back(caption.total_ms);
                                sink.append(SamplesBuffer::new(1, sr, audio));
                            }
                            None => break,
                        }
                    }
                    while !sink.empty() {
                        while queued_ms.len() > sink.len() {
                            completed_ms += queued_ms.pop_front().unwrap_or(0);
                        }
                        crate::speech_sync::playing(my_generation as u64, completed_ms + sink.get_pos().as_millis() as u64);
                        crate::volume::heartbeat();
                        crate::keep_awake::heartbeat();
                        if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
                            sink.stop();
                            crate::speech_sync::finish(my_generation as u64, "cancelled", completed_ms);
                            tracing::info!("tts: playback stopped by user (barge-in)");
                            return Ok(());
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    crate::speech_sync::finish(my_generation as u64, "finished", cumulative_ms);
                    tracing::info!("tts: streaming playback completed");
                    Ok(())
                }
                Err(e) => Err(format!("Failed to create audio sink: {}", e)),
            },
            Err(e) => Err(format!("Failed to get audio output stream: {}", e)),
        }
    })
    .await
    .unwrap_or_else(|_| {
        tracing::error!("tts: streaming audio thread panicked");
        Err("Audio thread panicked".to_string())
    })
}

// ─── Narration player (screen tour) ────────────────────────────────────
//
// Plays N lines back-to-back as ONE rodio queue (one source per line, so
// queue position == line index) and reports exactly which line is audible,
// so the screen-tour overlay can show callout i only while line i plays.
// The frontend never holds future lines — "nothing drawn ahead of speech"
// is structural, not timing luck. Reuses the cache/synthesis helpers of the
// streaming path; barge-in rides the same `TTS_GENERATION` counter.

/// One progress event from `narrate`. The index is into the `lines` slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NarrationEvent {
    Started(usize),
    Ended(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NarrateOutcome {
    Completed,
    /// Barge-in / stop arrived (generation bumped).
    Cancelled,
    /// No audio could be produced for line 0 or no output device — nothing
    /// was heard; the caller may fall back to timed (silent) dwell.
    Unavailable(String),
}

/// Silence appended after every line so callouts breathe.
pub const NARRATION_GAP_MS: u64 = 250;

/// Turns (items appended, items still queued) into the events that became
/// due. Pure + unit-tested — this is the sync core: a line is "started"
/// when it is the head of the queue and "ended" once it left the queue.
#[derive(Debug, Default)]
pub struct NarrationTracker {
    started: usize,
    ended: usize,
}

impl NarrationTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, appended: usize, sink_len: usize) -> Vec<NarrationEvent> {
        let completed = appended.saturating_sub(sink_len);
        let mut out = Vec::new();
        while self.ended < completed {
            if self.started <= self.ended {
                out.push(NarrationEvent::Started(self.ended));
                self.started = self.ended + 1;
            }
            out.push(NarrationEvent::Ended(self.ended));
            self.ended += 1;
        }
        if completed < appended && self.started <= completed {
            out.push(NarrationEvent::Started(completed));
            self.started = completed + 1;
        }
        out
    }
}

/// Append `ms` of silence to `samples` (mono).
fn pad_silence(samples: &mut Vec<f32>, sample_rate: u32, ms: u64) {
    let n = (sample_rate as u64 * ms / 1000) as usize;
    samples.extend(std::iter::repeat(0.0f32).take(n));
}

/// Speak `lines` as a single gap-separated queue and report which line is
/// audible via `on_event`. Items are synthesized ahead while earlier ones
/// play; a line that fails to synthesize (after line 0) becomes timed
/// silence so indices stay 1:1 with the queue.
pub async fn narrate<R, F>(
    app: tauri::AppHandle<R>,
    lines: Vec<String>,
    on_event: F,
) -> NarrateOutcome
where
    R: tauri::Runtime,
    F: FnMut(NarrationEvent) + Send + 'static,
{
    use tauri::Manager;
    if lines.is_empty() {
        return NarrateOutcome::Completed;
    }
    let tts_state = app.state::<TtsState>();
    let meeting = app.state::<Arc<MeetingState>>();
    let voice_id = crate::commands::read_edge_tts_voice(&app);
    let volume_pct = read_tts_volume(&app);
    let emotions: Vec<crate::tts_edge::TtsEmotion> =
        lines.iter().map(|l| resolve_emotion(&app, l)).collect();

    meeting.set_tts_playing(true);
    let my_generation = TTS_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    crate::speech_sync::register_intent(my_generation as u64, None, Some("screen_tour".into()), Some(crate::speech_sync::SpeechKind::Narration));
    for l in &lines {
        crate::directed::note_spoken(l);
    }

    let state_ref = TtsStateRef { cache: tts_state.cache.clone() };
    // Line 0 first: its success decides Unavailable vs playing.
    let first = match tokio::time::timeout(
        std::time::Duration::from_secs(25),
        async {
            // Line 0 may fall back to the local voice (nothing has been heard yet, so no voice
            // switch is audible); the engine that voices it then voices every later line.
            match synthesize_chunk_owned(&lines[0], &voice_id, &state_ref, emotions[0], false, false).await {
                Ok(v) => Ok(v),
                Err(_) => synthesize_chunk_owned(&lines[0], &voice_id, &state_ref, emotions[0], true, false).await,
            }
        },
    )
    .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            meeting.set_tts_playing(false);
            return NarrateOutcome::Unavailable(e);
        }
        Err(_) => {
            meeting.set_tts_playing(false);
            return NarrateOutcome::Unavailable("synthesis timed out".to_string());
        }
    };
    if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
        return NarrateOutcome::Cancelled;
    }

    type Item = (Vec<f32>, u32, CaptionTrack);
    let (tx, rx) = std::sync::mpsc::channel::<Option<Item>>();
    let rest: Vec<(String, crate::tts_edge::TtsEmotion)> = lines
        .iter()
        .cloned()
        .zip(emotions.iter().copied())
        .skip(1)
        .collect();
    let voice_owned = voice_id.clone();
    let rate_hint = first.1;
    let utterance_local = first.2.estimated;
    tokio::spawn(async move {
        for (text, emotion) in rest {
            if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
                break;
            }
            let item = match tokio::time::timeout(
                std::time::Duration::from_secs(12),
                synthesize_chunk_owned(&text, &voice_owned, &state_ref, emotion, utterance_local, false),
            )
            .await
            {
                Ok(Ok(v)) => v,
                _ => {
                    // Timed silence keeps queue index == line index.
                    let words = text.split_whitespace().count() as u64;
                    let ms = (words * 330 + 1200).clamp(2500, 9000);
                    let mut s = Vec::new();
                    pad_silence(&mut s, rate_hint, ms);
                    (s, rate_hint, CaptionTrack::default())
                }
            };
            if tx.send(Some(item)).is_err() {
                return;
            }
        }
        let _ = tx.send(None);
    });

    // Held for the whole narration (the lease restores the volume when it drops).
    let narrate_volume_lease = if volume_pct > 0 {
        crate::volume::acquire_volume_lease(volume_pct as f32 / 100.0)
    } else {
        None
    };
    let _narrate_keep_awake_lease = crate::keep_awake::acquire_lease();

    let app_for_audio = app.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let mut on_event = on_event;
        let (_stream, handle) = match OutputStream::try_default() {
            Ok(v) => v,
            Err(e) => return NarrateOutcome::Unavailable(format!("audio output: {e}")),
        };
        let sink = match Sink::try_new(&handle) {
            Ok(s) => s,
            Err(e) => return NarrateOutcome::Unavailable(format!("audio sink: {e}")),
        };
        let mut tracker = NarrationTracker::new();
        let mut appended = 0usize;
        let mut cumulative_ms: u64 = 0;
        let mut durations = Vec::<u64>::new();
        let mut channel_done = false;

        let append = |sink: &Sink, item: Item, appended: &mut usize, cumulative: &mut u64, durations: &mut Vec<u64>| {
            let (mut audio, sr, mut caption) = item;
            let own_ms = audio.len() as u64 * 1000 / (sr as u64).max(1);
            pad_silence(&mut audio, sr, NARRATION_GAP_MS);
            for w in caption.words.iter_mut() {
                w.start_ms += *cumulative;
            }
            caption.envelope_start_ms = *cumulative;
            if caption.total_ms == 0 {
                caption.total_ms = own_ms;
            }
            let _ = app_for_audio.emit("tts:caption", &caption);
            if *appended > 0 {
                crate::speech_sync::chunk(my_generation as u64, caption.clone());
            }
            *cumulative += own_ms + NARRATION_GAP_MS;
            durations.push(own_ms + NARRATION_GAP_MS);
            sink.append(SamplesBuffer::new(1, sr, audio));
            *appended += 1;
        };

        if let Err(e) = crate::speech_sync::prepare(my_generation as u64, first.2.clone()) {
            return NarrateOutcome::Unavailable(e);
        }
        append(&sink, first, &mut appended, &mut cumulative_ms, &mut durations);
        for ev in tracker.update(appended, sink.len()) {
            on_event(ev);
        }
        loop {
            if TTS_GENERATION.load(Ordering::SeqCst) > my_generation {
                sink.stop();
                crate::speech_sync::finish(my_generation as u64, "cancelled", 0);
                return NarrateOutcome::Cancelled;
            }
            if !channel_done {
                loop {
                    match rx.try_recv() {
                        Ok(Some(item)) => {
                            append(&sink, item, &mut appended, &mut cumulative_ms, &mut durations);
                        }
                        Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            channel_done = true;
                            break;
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    }
                }
            }
            crate::volume::heartbeat();
            crate::keep_awake::heartbeat();
            for ev in tracker.update(appended, sink.len()) {
                on_event(ev);
            }
            let completed = appended.saturating_sub(sink.len());
            let prior_ms: u64 = durations.iter().take(completed).sum();
            if !sink.empty() {
                crate::speech_sync::playing(my_generation as u64, prior_ms + sink.get_pos().as_millis() as u64);
            }
            if channel_done && sink.empty() {
                crate::speech_sync::finish(my_generation as u64, "finished", cumulative_ms);
                return NarrateOutcome::Completed;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    })
    .await
    .unwrap_or_else(|_| NarrateOutcome::Unavailable("audio thread panicked".to_string()));

    drop(narrate_volume_lease);
    if outcome != NarrateOutcome::Cancelled {
        // Same grace as speak_text; a newer generation owns the flag when cancelled.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        meeting.set_tts_playing(false);
    }
    outcome
}

#[cfg(test)]
mod narration_tests {
    use super::*;
    use NarrationEvent::*;

    #[test]
    fn tracker_first_item_starts_immediately() {
        let mut t = NarrationTracker::new();
        assert_eq!(t.update(1, 1), vec![Started(0)]);
        assert!(t.update(1, 1).is_empty()); // idempotent
    }

    #[test]
    fn tracker_gapless_transition() {
        let mut t = NarrationTracker::new();
        t.update(1, 1);
        assert!(t.update(2, 2).is_empty());
        assert_eq!(t.update(2, 1), vec![Ended(0), Started(1)]);
        assert_eq!(t.update(2, 0), vec![Ended(1)]);
    }

    #[test]
    fn tracker_underrun_gap_clears_before_next() {
        // Item 0 finishes before item 1 is synthesized.
        let mut t = NarrationTracker::new();
        t.update(1, 1);
        assert_eq!(t.update(1, 0), vec![Ended(0)]);
        // Item 1 arrives later: starts only now.
        assert_eq!(t.update(2, 1), vec![Started(1)]);
    }

    #[test]
    fn tracker_missed_polls_still_ordered() {
        // A long stall: two items finished between polls.
        let mut t = NarrationTracker::new();
        assert_eq!(
            t.update(3, 1),
            vec![Started(0), Ended(0), Started(1), Ended(1), Started(2)]
        );
    }

    #[test]
    fn tracker_never_emits_start_before_prior_end() {
        let mut t = NarrationTracker::new();
        let mut open: Option<usize> = None;
        let mut appended = 0usize;
        let mut len = 0usize;
        // deterministic pseudo-random walk
        let mut seed = 12345u64;
        for _ in 0..500 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let r = (seed >> 33) % 3;
            if r == 0 && appended < 8 {
                appended += 1;
                len += 1;
            } else if len > 0 {
                len -= 1;
            }
            for ev in t.update(appended, len) {
                match ev {
                    Started(i) => {
                        assert!(open.is_none(), "start({i}) while {open:?} open");
                        open = Some(i);
                    }
                    Ended(i) => {
                        assert_eq!(open, Some(i));
                        open = None;
                    }
                }
            }
        }
    }

    #[test]
    fn silence_padding_length() {
        let mut s = Vec::new();
        pad_silence(&mut s, 24_000, 250);
        assert_eq!(s.len(), 6_000);
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    #[test]
    fn test_split_single_sentence() {
        assert_eq!(split_sentences("Hello sir."), vec!["Hello sir."]);
    }

    #[test]
    fn test_split_multi_sentence() {
        let out = split_sentences("First. Second! Third?");
        assert_eq!(out, vec!["First.", "Second!", "Third?"]);
    }

    #[test]
    fn test_split_newlines() {
        let out = split_sentences("Line one\nLine two");
        assert_eq!(out, vec!["Line one", "Line two"]);
    }

    #[test]
    fn test_split_no_delimiter() {
        assert_eq!(split_sentences("hello"), vec!["hello"]);
    }

    #[test]
    fn test_split_empty_tail_ignored() {
        let out = split_sentences("Done. ");
        assert_eq!(out, vec!["Done."]);
    }

    #[test]
    fn test_split_unicode_safe() {
        let out = split_sentences("の test. Second.");
        assert_eq!(out.len(), 2);
    }
}

#[cfg(test)]
mod caption_tests {
    use super::*;
    use edge_tts_rust::{Boundary, BoundaryEvent};

    #[test]
    fn test_estimate_words_empty_text() {
        assert_eq!(estimate_words("   ", 1000), Vec::new());
        assert_eq!(estimate_words("", 1000), Vec::new());
    }

    #[test]
    fn test_estimate_words_single_word_gets_full_duration() {
        let words = estimate_words("Hello", 500);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "Hello");
        assert_eq!(words[0].start_ms, 0);
        assert_eq!(words[0].duration_ms, 500);
    }

    #[test]
    fn test_estimate_words_evenly_distributed() {
        let words = estimate_words("one two three four", 800);
        assert_eq!(words.len(), 4);
        assert_eq!(words[0].start_ms, 0);
        assert_eq!(words[1].start_ms, 200);
        assert_eq!(words[2].start_ms, 400);
        assert_eq!(words[3].start_ms, 600);
        assert!(words.iter().all(|w| w.duration_ms == 200));
    }

    #[test]
    fn test_boundaries_to_words_converts_ticks_to_ms() {
        // 10_000_000 ticks/sec -> 10_000 ticks/ms.
        let boundaries = vec![
            BoundaryEvent { kind: Boundary::Word, offset_ticks: 0, duration_ticks: 30_000, text: "Hi".into() },
            BoundaryEvent { kind: Boundary::Word, offset_ticks: 50_000, duration_ticks: 40_000, text: "there".into() },
        ];
        let words = boundaries_to_words(&boundaries, 0);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0], CaptionWord { text: "Hi".into(), start_ms: 0, duration_ms: 3 });
        assert_eq!(words[1], CaptionWord { text: "there".into(), start_ms: 5, duration_ms: 4 });
    }

    #[test]
    fn test_boundaries_to_words_applies_cumulative_offset() {
        // Streaming chunk 2+: its own boundaries start at 0, but the
        // consumer must offset them by everything already queued.
        let boundaries = vec![BoundaryEvent {
            kind: Boundary::Word,
            offset_ticks: 0,
            duration_ticks: 10_000,
            text: "second".into(),
        }];
        let words = boundaries_to_words(&boundaries, 1500);
        assert_eq!(words[0].start_ms, 1500);
    }

    #[test]
    fn test_boundaries_to_words_unescapes_xml_entities() {
        let boundaries = vec![
            BoundaryEvent {
                kind: Boundary::Word,
                offset_ticks: 0,
                duration_ticks: 10_000,
                text: "didn&apos;t".into(),
            },
            BoundaryEvent {
                kind: Boundary::Word,
                offset_ticks: 10_000,
                duration_ticks: 10_000,
                text: "&quot;hello&quot;".into(),
            },
        ];
        let words = boundaries_to_words(&boundaries, 0);
        assert_eq!(words[0].text, "didn't");
        assert_eq!(words[1].text, "\"hello\"");
    }

    #[test]
    fn test_compute_envelope_empty_input() {
        assert!(compute_envelope(&[], 24000, ENVELOPE_FRAME_MS).is_empty());
        assert!(compute_envelope(&[0.1, 0.2], 0, ENVELOPE_FRAME_MS).is_empty());
    }

    #[test]
    fn test_compute_envelope_silence_is_all_zero() {
        let samples = vec![0.0f32; 24000]; // 1s of silence at 24kHz
        let env = compute_envelope(&samples, 24000, ENVELOPE_FRAME_MS);
        assert!(!env.is_empty());
        assert!(env.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_compute_envelope_normalizes_to_loudest_frame() {
        // Two 20ms frames at 1000Hz sample rate: frame 0 quiet, frame 1 loud.
        let sr = 1000u32;
        let frame_len = (sr as u64 * ENVELOPE_FRAME_MS / 1000) as usize; // 20 samples
        let mut samples = vec![0.1f32; frame_len];
        samples.extend(vec![0.5f32; frame_len]);
        let env = compute_envelope(&samples, sr, ENVELOPE_FRAME_MS);
        assert_eq!(env.len(), 2);
        // Loudest frame normalizes to exactly 1.0; the quiet frame is
        // proportionally smaller, never negative, never above 1.0.
        assert!((env[1] - 1.0).abs() < 1e-5);
        assert!(env[0] > 0.0 && env[0] < env[1]);
    }

    #[test]
    fn test_compute_envelope_frame_count_matches_duration() {
        let sr = 24000u32;
        let samples = vec![0.3f32; sr as usize]; // exactly 1000ms
        let env = compute_envelope(&samples, sr, ENVELOPE_FRAME_MS);
        // 1000ms / 20ms per frame = 50 frames.
        assert_eq!(env.len(), 50);
    }
}

#[cfg(test)]
mod voice_stack_tests {
    use super::*;

    fn local_line() -> CachedAudio {
        CachedAudio {
            samples: vec![0.0; 16],
            sample_rate: 24_000,
            caption: CaptionTrack { estimated: true, ..Default::default() },
        }
    }

    /// The 2026-10-15 bug: Edge answered 403 to every request while bing.com was reachable, so the
    /// instant local-voice acknowledgements were declared "stale" and re-rendered on the live path.
    #[tokio::test]
    async fn local_voice_acks_stay_usable_while_the_cloud_voice_is_broken() {
        crate::tts_health::reset_for_test();
        CACHE_REFRESHING.store(true, Ordering::SeqCst); // a stale verdict must not start a real re-render here
        let cache = Arc::new(Mutex::new(HashMap::new()));
        for _ in 0..12 {
            crate::tts_health::edge_note_fail("HTTP 403");
        }
        assert!(cache_entry_usable(&local_line(), &cache, "en-US-AvaNeural"), "403 forever must never make acks stale");
        crate::tts_health::edge_note_ok();
        assert!(!cache_entry_usable(&local_line(), &cache, "en-US-AvaNeural"), "stale only once Edge really worked");
        CACHE_REFRESHING.store(false, Ordering::SeqCst);
        crate::tts_health::reset_for_test();
    }

    #[tokio::test]
    async fn the_windows_voice_line_is_never_persisted_or_called_the_persona() {
        // A system-voice caption must not be written to the voice cache (it would later be served as "the persona").
        let dir = std::env::temp_dir().join(format!("nexus_voice_stack_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = DISK_DIR.set(dir.clone());
        let Some(effective) = DISK_DIR.get().cloned() else { return };
        let system = CaptionTrack { estimated: true, system: true, ..Default::default() };
        persist_line_bg("Ok sir", "v", &[0.1; 64], 24_000, &system);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(crate::tts_cache_disk::load(&effective, "v", "Ok sir").is_none());
        let local = CaptionTrack { estimated: true, system: false, total_ms: 3, ..Default::default() };
        persist_line_bg("Ok sir", "v", &[0.1; 64], 24_000, &local);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let got = crate::tts_cache_disk::load(&effective, "v", "Ok sir").expect("local persona line is persisted");
        assert_eq!(got.origin, crate::tts_cache_disk::Origin::Local);
        let _ = std::fs::remove_dir_all(&effective);
    }

    /// Live: with the local neural voice not loaded, a line must be audible-ready fast via the Windows voice.
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "needs the Windows voices and the local engine handle; run with --ignored"]
    async fn a_cold_local_voice_never_delays_a_line() {
        let _state = TtsState::new(); // installs the global engine handle (not loaded yet)
        let t0 = std::time::Instant::now();
        let (samples, _sr, caption) = synthesize_local_guarded("New tab sir.").await.expect("speaks");
        let ms = t0.elapsed().as_millis();
        eprintln!("cold local voice -> first audio ready in {ms} ms (system voice: {})", caption.system);
        assert!(caption.system, "a cold local engine hands over to the Windows voice");
        assert!(!samples.is_empty());
        assert!(ms < 1_500, "first audio within 1.5 s, got {ms} ms");
    }
}

#[cfg(test)]
mod first_clause_tests {
    use super::*;

    #[test]
    fn short_sentence_is_never_split() {
        assert_eq!(split_first_clause("Done, sir, the file is saved."), None);
    }

    #[test]
    fn long_sentence_splits_at_first_clause_break() {
        let s = "I checked the three repositories you mentioned yesterday, and the second one has an open pull request waiting for review.";
        let (head, tail) = split_first_clause(s).expect("should split");
        assert_eq!(head, "I checked the three repositories you mentioned yesterday,");
        assert!(tail.starts_with("and the second one"));
        assert_eq!(format!("{head} {tail}"), s, "no words lost or reordered");
    }

    #[test]
    fn no_comma_means_no_split() {
        let s = "I checked the three repositories you mentioned yesterday and the second one has an open pull request waiting";
        assert_eq!(split_first_clause(s), None);
    }

    #[test]
    fn head_and_tail_keep_a_minimum_size() {
        // comma after word 2 (head too short) and after the 13th of 14 (tail too short)
        let s = "Well, I looked through every single folder on the disk, twice.";
        assert_eq!(split_first_clause(s), None);
    }

    #[test]
    fn plan_chunks_keeps_later_sentences() {
        let text = "I checked the three repositories you mentioned yesterday, and the second one has an open pull request waiting for review. Shall I open it?";
        let c = plan_chunks(text);
        assert_eq!(c.len(), 3);
        assert_eq!(c[2], "Shall I open it?");
        assert_eq!(plan_chunks("Done, sir."), vec!["Done, sir."]);
    }
}

#[cfg(test)]
mod voice_continuity_tests {
    use super::*;

    #[test]
    fn a_failed_cloud_sentence_falls_to_the_local_twin_before_the_windows_voice() {
        assert_eq!(ChunkEngine::Cloud.after_failure(), Some(ChunkEngine::LocalTwin));
        assert_eq!(ChunkEngine::LocalTwin.after_failure(), Some(ChunkEngine::WindowsVoice));
        assert_eq!(ChunkEngine::WindowsVoice.after_failure(), None);
    }

    #[test]
    fn the_reply_starts_on_the_voice_chunk_zero_used() {
        assert_eq!(ChunkEngine::start(false, false), ChunkEngine::Cloud);
        assert_eq!(ChunkEngine::start(true, false), ChunkEngine::LocalTwin);
        assert_eq!(ChunkEngine::start(true, true), ChunkEngine::WindowsVoice);
        assert_eq!(ChunkEngine::start(false, true), ChunkEngine::WindowsVoice);
    }

    #[test]
    fn a_second_stop_with_nothing_new_playing_cancels_nothing() {
        assert!(stop_has_work(7, usize::MAX), "first stop ever");
        assert!(!stop_has_work(8, 8), "the burst after a stop: the generation it made is still current");
        assert!(stop_has_work(9, 8), "a line began after the last stop, so a stop must cancel it");
    }
}
