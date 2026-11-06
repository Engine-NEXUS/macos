//! Hand mode — a sub command center that turns the webcam into a pointer.
//!
//! Pipeline: the `hand` window (camera + MediaPipe Hand Landmarker, see `frontend/src/hand/`) sends
//! 21 landmarks per frame to `hand_frame`; the pure `engine` decides what the hand means; this
//! module owns the session, the safety gates and the mouse output.
//!
//! LIVE BY DEFAULT: arming (palm, fist, palm, fist) is the confirmation, so the cursor follows the hand at
//! once. `handMousePreview` (or "hand mouse preview") makes it read-only: decisions are only drawn on the
//! overlay ("would click here") and never reach the real mouse.
//!
//! The camera exists only while the mode is on (window created on demand, destroyed on exit), no
//! frame ever leaves the window (only 21 numeric landmarks cross IPC) and nothing is stored unless
//! `handTrace` is enabled (landmark numbers only, for tuning).

pub mod engine;
pub mod filter;
pub mod gesture;
pub mod pose;
pub mod snap;

/// Live mouse control (move/click/scroll) is implemented and tested for Windows only. Elsewhere hand
/// mode tracks and shows its overlay but stays in preview: injecting input also needs per-OS
/// permissions (macOS Accessibility, Wayland portals) and a protected-window guard we do not have yet.
pub fn live_supported() -> bool {
    cfg!(target_os = "windows")
}

use engine::{Action, Button, Config, Engine, Mon, Snapshot};
use parking_lot::Mutex;
use pose::{Hand, Pt};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static LIVE: AtomicBool = AtomicBool::new(false);
/// Last position WE moved the cursor to (packed x:y), and when. A real cursor far from it = the user's own mouse.
static LAST_SET: AtomicI64 = AtomicI64::new(i64::MIN);
static LAST_SET_MS: AtomicU64 = AtomicU64::new(0);
/// Hand mode was started by a Ghost session (so ending Ghost ends it too).
static STARTED_BY_GHOST: AtomicBool = AtomicBool::new(false);
/// Epoch lease for Focus Mode while Hand Mode runs.
static FOCUS_EPOCH: AtomicU64 = AtomicU64::new(0);

const MOUSE_TAKEOVER_PX: i32 = 40;
const SNAP_RADIUS_PX: i32 = 36;
const HUD_EMIT_EVERY: Duration = Duration::from_millis(50);
const METRICS_EVERY: Duration = Duration::from_secs(5);
const NO_FRAMES_WARN: Duration = Duration::from_secs(12);
/// Armed and no frame at all for this long: the camera froze. Disarm (which releases a held drag).
/// Frames arrive at least every few hundred ms even with no hand in view (every 3rd frame at 6 fps).
const FRAME_STALL_DISARM: Duration = Duration::from_secs(2);

/// True when an armed session has gone silent long enough that the pointer must be released.
fn frames_stalled(armed: bool, silent_for: Duration) -> bool {
    armed && silent_for > FRAME_STALL_DISARM
}
const TRACE_MAX_BYTES: u64 = 20_000_000;

pub fn active() -> bool {
    SESSION.lock().is_some()
}

/// Hand Mode alone does not listen: while it is the only thing active, the wake word stays
/// suppressed (same choke point as a meeting or a manual pause) so ambient speech while gesturing
/// at the screen never turns into a cloud question-and-answer turn — field report: "it felt like
/// Ghost Mode was listening". The instant Ghost Mode is ALSO active (`handWithGhost`, or Ghost
/// started while Hand was already on) this returns false again: Ghost is supposed to listen, and
/// Hand just rides along as its pointer. A deliberate hotkey press still starts a capture directly
/// (`start_stt_capture`), bypassing this ambient gate entirely, so the hotkey + "exit hand mode" (or
/// any other command) keeps working even while this is suppressing the wake word.
pub fn suppresses_voice() -> bool {
    active() && !crate::ghost::session_active()
}

// ── settings (loose reads, like directed.rs: a missing key must never break a session) ───────

fn read_setting<R: Runtime>(app: &AppHandle<R>, key: &str) -> Option<serde_json::Value> {
    let dir = app.path().app_data_dir().ok()?;
    let text = std::fs::read_to_string(dir.join("settings.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()?.get(key).cloned()
}

fn read_bool<R: Runtime>(app: &AppHandle<R>, key: &str, default: bool) -> bool {
    read_setting(app, key).and_then(|v| v.as_bool()).unwrap_or(default)
}

// ── wire types ────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct FrameIn {
    /// 21 landmarks (x, y, z), normalised to the camera image; None = no hand in this frame.
    pub lm: Option<Vec<[f64; 3]>>,
    /// `Date.now()` in the hand window just before inference.
    pub t_epoch_ms: f64,
    /// When the camera actually captured the frame (epoch ms), if the browser tells us. The latency the
    /// pointer is led by is measured from here, not from when processing started.
    #[serde(default)]
    pub t_capture_epoch_ms: Option<f64>,
    pub infer_ms: f64,
    /// Per-window frame counter (1, 2, 3 ...). Absent from older builds of the page.
    #[serde(default)]
    pub seq: Option<u64>,
}

/// A frame older than this is never allowed to move the pointer (camera capture to arrival).
const MAX_FRAME_AGE_MS: f64 = 3000.0;

/// Whether a frame may change pointer or gesture state: its sequence must advance (a restarted
/// window begins again at 1) and its capture age must be sane. Frames without a sequence pass.
fn accept_frame(last_seq: u64, seq: Option<u64>, age_ms: f64) -> bool {
    if age_ms > MAX_FRAME_AGE_MS {
        return false;
    }
    match seq {
        None => true,
        Some(1) => true,
        Some(n) => n > last_seq,
    }
}

#[derive(Debug, Serialize, Clone)]
struct HudState {
    #[serde(flatten)]
    snap: Snapshot,
    live: bool,
    fps: f64,
    /// Last decision, e.g. "click", "drag", "scroll", with a counter so the HUD can flash on repeats.
    last_action: Option<&'static str>,
    action_seq: u64,
    note: Option<String>,
    /// Camera frames per second as last measured by the hand window (0 = unknown). Under 15 the overlay
    /// says the room is too dark: webcams cut their frame rate in dim light.
    cam_fps: f64,
    /// Sustained GPU contention suspected (infer time well above the calibrated baseline). Informational
    /// only — tracking keeps running either way.
    gpu_busy: bool,
    /// The hand window's own judgement that the camera looks covered (dark + textureless frames).
    covered: bool,
}

fn to_hand(lm: &[[f64; 3]]) -> Option<Hand> {
    if lm.len() != 21 {
        return None;
    }
    let mut h = [Pt::default(); 21];
    for (i, p) in lm.iter().enumerate() {
        h[i] = Pt { x: p[0], y: p[1], z: p[2] };
    }
    Some(h)
}

// ── session ───────────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
enum Out {
    Move(i32, i32),
    Click(i32, i32, Button),
    Press(i32, i32),
    Release(i32, i32),
    Scroll(i32),
    Gesture(gesture::Cmd),
    Stop,
}

/// Most messages the output thread takes from its queue in one go.
const OUT_BATCH_MAX: usize = 256;
/// A click's nearest-control lookup may take this long; past it the click lands where the hand pointed.
const SNAP_DEADLINE: Duration = Duration::from_millis(150);

/// Bounds what the output thread replays after a stall: runs of moves collapse to the newest one,
/// discrete actions keep their order, and a stop drops everything except releases (a held button is
/// always let go) before the stop itself.
fn coalesce(batch: Vec<Out>) -> Vec<Out> {
    if batch.iter().any(|m| matches!(m, Out::Stop)) {
        let mut out: Vec<Out> = batch.into_iter().filter(|m| matches!(m, Out::Release(..))).collect();
        out.push(Out::Stop);
        return out;
    }
    let mut out: Vec<Out> = Vec::with_capacity(batch.len());
    for m in batch {
        if matches!(m, Out::Move(..)) && matches!(out.last(), Some(Out::Move(..))) {
            out.pop();
        }
        out.push(m);
    }
    out
}

#[derive(Default)]
struct Metrics {
    frames: u64,
    with_hand: u64,
    lat: Vec<u32>,
    infer: Vec<u32>,
    clicks: u64,
    drags: u64,
    scrolls: u64,
    last_log: Option<Instant>,
}

impl Metrics {
    fn p50(v: &mut Vec<u32>) -> u32 {
        if v.is_empty() {
            return 0;
        }
        v.sort_unstable();
        v[v.len() / 2]
    }
}

struct Session {
    engine: Engine,
    started: Instant,
    last_frame: Instant,
    last_seq: u64,
    stale_rejected: u64,
    last_emit: Instant,
    fps_window: (Instant, u32, f64),
    out: mpsc::Sender<Out>,
    metrics: Metrics,
    trace: Option<std::io::BufWriter<std::fs::File>>,
    trace_bytes: u64,
    esc_armed: bool,
    last_action: Option<&'static str>,
    action_seq: u64,
    note: Option<String>,
    /// Smoothed capture-to-Rust delay (ms): what the pointer prediction leads by.
    lat_ema: f64,
    cam_fps: f64,
    /// One-time calibrated infer p50 (ms) for whichever delegate the hand window picked; 0.0 = not
    /// yet known (calibration hasn't reported in via `hand_info` yet, or GPU/CPU trial never ran).
    calibrated_infer_p50_ms: f64,
    /// Consecutive 5 s metrics windows where live infer p50 stayed well above the calibrated
    /// baseline — e.g. a third app (a browser) competing for the same shared GPU process.
    gpu_busy_high_windows: u32,
    /// True once sustained contention is confirmed; cleared once it recovers. Drives the HUD chip
    /// suffix; never stops tracking, only reports it.
    gpu_busy: bool,
    /// Whether the GPU-busy notice has already been spoken since the last time it cleared.
    gpu_busy_notified: bool,
    /// Latest "camera looks covered" signal reported by the hand window (brightness + variance on
    /// its own frame data — no pixel ever crosses IPC, only this one boolean).
    covered: bool,
    /// Whether the camera-covered notice has already been spoken since the last time it cleared.
    covered_notified: bool,
}

fn now_epoch_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn pack(x: i32, y: i32) -> i64 {
    ((x as i64) << 32) | (y as u32 as i64)
}

fn unpack(v: i64) -> (i32, i32) {
    ((v >> 32) as i32, v as i32)
}

// ── safety gate for clicks, drags and scrolls ─────────────────────────────────────────────────

/// Why a click must NOT be sent to the foreground window (None = fine). Pure so it can be tested.
pub fn guard_decision(title: &str, process: &str, target_elevated: bool, self_elevated: bool) -> Option<String> {
    if crate::live::safety::is_target_blocked(title) || crate::live::safety::is_target_blocked(process) {
        return Some("protected window".to_string());
    }
    crate::live::commands::window::elevated_block_reason(target_elevated, self_elevated)
        .map(|_| "admin window".to_string())
}

#[cfg(target_os = "windows")]
fn guard_now() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let title = crate::browser_url::foreground_window_title().unwrap_or_default();
    let process = crate::browser_url::foreground_process_name().unwrap_or_default();
    let (target_elevated, self_elevated) = unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            (false, false)
        } else {
            (
                crate::live::commands::window::window_process_elevated(hwnd),
                crate::live::commands::window::our_process_elevated(),
            )
        }
    };
    guard_decision(&title, &process, target_elevated, self_elevated)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn guard_now() -> Option<String> {
    Some("hand mouse output is not implemented on this platform".to_string())
}

#[cfg(target_os = "windows")]
fn cursor_pos() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt).as_bool().then_some((pt.x, pt.y)) }
}

/// macOS: enigo posts CGEvents, which the OS drops unless the binary holds the
/// Accessibility permission. `AXIsProcessTrusted` asks Genetics-free C — no
/// prompt is raised, the settings-UI flow stays the user's job (the guard
/// message points there). Then the same protection gate as Windows applies:
/// protected/elevated foreground windows refuse the same way.
#[cfg(target_os = "macos")]
fn ax_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }
    unsafe { AXIsProcessTrusted() != 0 }
}

#[cfg(target_os = "macos")]
fn guard_now() -> Option<String> {
    if !ax_trusted() {
        return Some("Accessibility permission missing (System Settings → Privacy & Security → Accessibility)".to_string());
    }
    let title = crate::browser_url::foreground_window_title().unwrap_or_default();
    let process = crate::browser_url::foreground_process_name().unwrap_or_default();
    // Fail closed: until macOS foreground-app detection exists (doc 155, M6/M7) both reads are empty, and an empty
    // title/process would sail past the protected-window denylist. Hand Mode stays in preview instead.
    if title.is_empty() && process.is_empty() {
        return Some("cannot identify the foreground app on this Mac yet".to_string());
    }
    guard_decision(&title, &process, false, false)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn guard_now() -> Option<String> {
    Some("hand mouse output is not implemented on this platform".to_string())
}

/// Where is the real cursor? On macOS this is a plain CoreGraphics call — the
/// same points coordinate space enigo's `Coordinate::Abs` posts in, so engine
/// seeding and the takeover check see consistent values.
#[cfg(target_os = "macos")]
fn cursor_pos() -> Option<(i32, i32)> {
    #[repr(C)]
    struct MacPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct MacSize {
        w: f64,
        h: f64,
    }
    #[repr(C)]
    struct MacRect {
        origin: MacPoint,
        size: MacSize,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventCreate(unknown: *const core::ffi::c_void) -> *mut core::ffi::c_void;
        fn CGEventGetLocation(ev: *mut core::ffi::c_void) -> MacRect;
        fn CFRelease(cf: *mut core::ffi::c_void);
    }
    unsafe {
        let ev = CGEventCreate(std::ptr::null());
        if ev.is_null() {
            return None;
        }
        let r = CGEventGetLocation(ev);
        CFRelease(ev);
        Some((r.origin.x.round() as i32, r.origin.y.round() as i32))
    }
}

// ── keyboard output for gestures ──────────────────────────────────────────────────────────────

/// (virtual key, extended flag, key-up) events in one batch.
fn send_keys(events: &[(u16, bool, bool)]) -> Result<(), String> {
    let ev: Vec<crate::shortcuts::keys::KeyEvent> = events
        .iter()
        .map(|&(vk, extended, up)| crate::shortcuts::keys::KeyEvent { vk, extended, up })
        .collect();
    crate::shortcuts::exec::send_events(&ev)
}

const VK_ALT: u16 = 0x12;
const VK_TAB: u16 = 0x09;
const VK_SHIFT: u16 = 0x10;
const VK_CTRL: u16 = 0x11;
const VK_LWIN: u16 = 0x5B;
const VK_LEFT: u16 = 0x25;
const VK_RIGHT: u16 = 0x27;
const VK_D: u16 = 0x44;

/// Carry out one gesture command. Protected / elevated foreground windows refuse (same gate as clicks),
/// except that letting go of Alt is always done.
fn run_gesture(
    cmd: gesture::Cmd,
    alt_since: &mut Option<Instant>,
    release_alt: &dyn Fn(&mut Option<Instant>, bool),
    guard: &mut dyn FnMut() -> Option<String>,
) {
    use gesture::Cmd;
    use crate::shortcuts::map::Action as Sc;
    let tap = |vk: u16, ext: bool| [(vk, ext, false), (vk, ext, true)];
    match cmd {
        Cmd::SwitcherCommit => {
            release_alt(alt_since, false);
            return;
        }
        Cmd::SwitcherCancel => {
            release_alt(alt_since, true);
            return;
        }
        _ => {}
    }
    if let Some(why) = guard() {
        tracing::info!("hand: gesture {} blocked ({why})", cmd.id());
        return;
    }
    let sent = match cmd {
        Cmd::NextTab => {
            crate::shortcuts::exec::do_action(Sc::NextTab, None);
            Ok(())
        }
        Cmd::PrevTab => {
            crate::shortcuts::exec::do_action(Sc::PrevTab, None);
            Ok(())
        }
        Cmd::SwitcherOpen => {
            let r = send_keys(&[(VK_ALT, false, false), (VK_TAB, false, false), (VK_TAB, false, true)]);
            if r.is_ok() {
                *alt_since = Some(Instant::now());
            }
            r
        }
        Cmd::SwitcherNext if alt_since.is_some() => send_keys(&tap(VK_TAB, false)),
        Cmd::SwitcherPrev if alt_since.is_some() => {
            send_keys(&[(VK_SHIFT, false, false), (VK_TAB, false, false), (VK_TAB, false, true), (VK_SHIFT, false, true)])
        }
        Cmd::DesktopPrev => send_keys(&[(VK_LWIN, true, false), (VK_CTRL, false, false), (VK_LEFT, true, false), (VK_LEFT, true, true), (VK_CTRL, false, true), (VK_LWIN, true, true)]),
        Cmd::DesktopNext => send_keys(&[(VK_LWIN, true, false), (VK_CTRL, false, false), (VK_RIGHT, true, false), (VK_RIGHT, true, true), (VK_CTRL, false, true), (VK_LWIN, true, true)]),
        Cmd::TaskView => send_keys(&[(VK_LWIN, true, false), (VK_TAB, false, false), (VK_TAB, false, true), (VK_LWIN, true, true)]),
        Cmd::ShowDesktop => send_keys(&[(VK_LWIN, true, false), (VK_D, false, false), (VK_D, false, true), (VK_LWIN, true, true)]),
        _ => Ok(()),
    };
    if let Err(e) = sent {
        tracing::warn!("hand: gesture {} not sent: {e}", cmd.id());
    }
}

// ── the user's own pointer settings ───────────────────────────────────────────────────────────

/// Windows pointer speed (1..=20, default 10) as the multiplier Windows itself applies.
fn speed_multiplier(level: i32) -> f64 {
    const T: [f64; 20] = [
        0.03125, 0.0625, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75,
        3.0, 3.25, 3.5,
    ];
    T[(level.clamp(1, 20) - 1) as usize]
}

/// (sensitivity multiplier, "enhance pointer precision" on). Defaults to (1.0, true) when unreadable.
#[cfg(target_os = "windows")]
fn pointer_prefs() -> (f64, bool) {
    #[link(name = "user32")]
    extern "system" {
        fn SystemParametersInfoW(action: u32, param: u32, pv: *mut core::ffi::c_void, win_ini: u32) -> i32;
    }
    const SPI_GETMOUSE: u32 = 0x0003;
    const SPI_GETMOUSESPEED: u32 = 0x0070;
    let mut speed: u32 = 10;
    let mut acc = [0i32; 3];
    let (ok_speed, ok_acc) = unsafe {
        (
            SystemParametersInfoW(SPI_GETMOUSESPEED, 0, &mut speed as *mut u32 as *mut _, 0) != 0,
            SystemParametersInfoW(SPI_GETMOUSE, 0, acc.as_mut_ptr() as *mut _, 0) != 0,
        )
    };
    (
        if ok_speed { speed_multiplier(speed as i32) } else { 1.0 },
        if ok_acc { acc[2] != 0 } else { true },
    )
}

#[cfg(not(target_os = "windows"))]
fn pointer_prefs() -> (f64, bool) {
    (1.0, true)
}

/// `handSensitivity` > 0 overrides the OS value (a plain multiplier; 1.0 = Windows speed 10).
fn effective_pointer<R: Runtime>(app: &AppHandle<R>) -> (f64, bool) {
    let (os_mult, precision) = pointer_prefs();
    let custom = read_setting(app, "handSensitivity").and_then(|v| v.as_f64()).unwrap_or(0.0);
    (if custom > 0.0 { custom } else { os_mult }, precision)
}

// ── mouse output thread ───────────────────────────────────────────────────────────────────────

fn spawn_output_thread(snap: bool) -> mpsc::Sender<Out> {
    let (tx, rx) = mpsc::channel::<Out>();
    std::thread::Builder::new()
        .name("hand-out".into())
        .spawn(move || {
            use enigo::{Axis, Coordinate, Direction, Enigo, Mouse, Settings};
            let Ok(mut e) = Enigo::new(&Settings::default()) else {
                tracing::warn!("hand: enigo init failed, mouse output disabled");
                return;
            };
            let mut guard_cache: (Instant, Option<String>) = (Instant::now() - Duration::from_secs(5), None);
            let mut guard = || {
                if guard_cache.0.elapsed() > Duration::from_millis(150) {
                    guard_cache = (Instant::now(), guard_now());
                }
                guard_cache.1.clone()
            };
            let to_btn = |b: Button| match b {
                Button::Left => enigo::Button::Left,
                Button::Right => enigo::Button::Right,
            };
            let mv = |e: &mut Enigo, x: i32, y: i32| {
                if e.move_mouse(x, y, Coordinate::Abs).is_ok() {
                    LAST_SET.store(pack(x, y), Ordering::Relaxed);
                    LAST_SET_MS.store(now_epoch_ms() as u64, Ordering::Relaxed);
                }
            };
            // The window switcher holds Alt. It is always released: on commit/cancel, on Stop, when the
            // channel closes, and by this timer if every one of those were somehow missed.
            let mut alt_since: Option<Instant> = None;
            let release_alt = |since: &mut Option<Instant>, cancel: bool| {
                if since.take().is_some() {
                    if cancel {
                        let _ = send_keys(&[(0x1B, false, false), (0x1B, false, true)]);
                    }
                    let _ = send_keys(&[(0x12, false, true)]);
                }
            };
            loop {
                let msg = match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(m) => m,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if alt_since.is_some_and(|t| t.elapsed() > Duration::from_secs(12)) {
                            release_alt(&mut alt_since, true);
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        release_alt(&mut alt_since, true);
                        return;
                    }
                };
                let mut batch = vec![msg];
                while batch.len() < OUT_BATCH_MAX {
                    match rx.try_recv() {
                        Ok(m) => batch.push(m),
                        Err(_) => break,
                    }
                }
                for msg in coalesce(batch) {
                match msg {
                    Out::Stop => {
                        release_alt(&mut alt_since, true);
                        return;
                    }
                    Out::Gesture(cmd) => run_gesture(cmd, &mut alt_since, &release_alt, &mut guard),
                    Out::Move(x, y) => mv(&mut e, x, y),
                    Out::Click(x, y, b) => {
                        if let Some(why) = guard() {
                            tracing::info!("hand: click blocked ({why})");
                            continue;
                        }
                        let (x, y) = if snap { snapped_within(x, y, SNAP_DEADLINE) } else { (x, y) };
                        mv(&mut e, x, y);
                        let _ = e.button(to_btn(b), Direction::Click);
                    }
                    Out::Press(x, y) => {
                        if let Some(why) = guard() {
                            tracing::info!("hand: drag blocked ({why})");
                            continue;
                        }
                        mv(&mut e, x, y);
                        let _ = e.button(enigo::Button::Left, Direction::Press);
                    }
                    // A release is always sent: a stuck-down button is worse than a stray release.
                    Out::Release(x, y) => {
                        mv(&mut e, x, y);
                        let _ = e.button(enigo::Button::Left, Direction::Release);
                    }
                    Out::Scroll(lines) => {
                        if guard().is_none() {
                            let _ = e.scroll(lines, Axis::Vertical);
                        }
                    }
                }
                }
            }
        })
        .ok();
    tx
}

static SNAP_BUSY: AtomicBool = AtomicBool::new(false);

/// Runs the lookup off the output thread. A slow or hung UI Automation provider costs one deadline,
/// never a stalled pointer or a delayed release; a late answer is dropped, and at most one lookup is
/// ever outstanding (while one is still running, further clicks land where the hand pointed).
fn snapped_within(x: i32, y: i32, deadline: Duration) -> (i32, i32) {
    if SNAP_BUSY.swap(true, Ordering::AcqRel) {
        return (x, y);
    }
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().name("hand-snap".into()).spawn(move || {
        let r = snapped(x, y);
        SNAP_BUSY.store(false, Ordering::Release);
        let _ = tx.send(r);
    });
    if spawned.is_err() {
        SNAP_BUSY.store(false, Ordering::Release);
        return (x, y);
    }
    rx.recv_timeout(deadline).unwrap_or((x, y))
}

/// Nearest-control correction for one click (UI Automation, once per click, never per frame).
#[cfg(not(target_os = "windows"))]
fn snapped(x: i32, y: i32) -> (i32, i32) {
    (x, y) // UI Automation is Windows-only; other systems click where the hand points
}

#[cfg(target_os = "windows")]
fn snapped(x: i32, y: i32) -> (i32, i32) {
    let rects: Vec<snap::Rect> = crate::screen::list_actionables()
        .iter()
        .map(|e| snap::Rect { x: e.x, y: e.y, w: e.w, h: e.h })
        .collect();
    snap::snap_point((x, y), &rects, SNAP_RADIUS_PX)
}

/// Arm = two rising tones, disarm = two falling tones, so the state is audible without looking.
/// `Beep` blocks for its duration: run it off the frame path.
fn beep(armed: bool) {
    let tones: [(u32, u32); 2] = if armed { [(660, 90), (990, 120)] } else { [(520, 110), (330, 160)] };
    std::thread::spawn(move || {
        #[cfg(target_os = "windows")]
        for (hz, ms) in tones {
            unsafe {
                use windows::Win32::System::Diagnostics::Debug::Beep;
                let _ = Beep(hz, ms);
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = tones;
    });
}

// ── applying engine decisions ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn register_esc<R: Runtime>(_app: &AppHandle<R>) -> bool {
    false // the global-shortcut plugin is not available on Linux
}

#[cfg(not(target_os = "linux"))]
fn register_esc<R: Runtime>(app: &AppHandle<R>) -> bool {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
    let handle = app.clone();
    app.global_shortcut()
        .on_shortcut("Escape", move |_a, _s, ev| {
            if ev.state() == ShortcutState::Pressed {
                disarm(&handle, "esc");
            }
        })
        .is_ok()
}

#[cfg(target_os = "linux")]
fn unregister_esc<R: Runtime>(_app: &AppHandle<R>) {}

#[cfg(not(target_os = "linux"))]
fn unregister_esc<R: Runtime>(app: &AppHandle<R>) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister("Escape");
}

/// Apply what the engine decided. `live` = really move/click; otherwise only report it.
fn apply<R: Runtime>(app: &AppHandle<R>, s: &mut Session, actions: Vec<Action>) {
    let live = LIVE.load(Ordering::Relaxed);
    for a in actions {
        match a {
            Action::ArmChanged { armed, reason } => {
                if armed {
                    // Forget where an earlier session put the cursor, or the first check would read
                    // the user's own mouse position as a takeover.
                    LAST_SET.store(i64::MIN, Ordering::Relaxed);
                    // The layout may have changed since the mode started (monitor plugged, scale changed).
                    s.engine.set_monitors(desktop(app).0);
                    let (sens, precision) = effective_pointer(app);
                    s.engine.set_pointer(sens, precision);
                    println!("[HAND] pointer sensitivity x{sens:.2} (OS pointer speed{}, precision {})", if read_setting(app, "handSensitivity").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0 { " overridden" } else { "" }, if precision { "on" } else { "off" });
                    if let Some((x, y)) = cursor_pos() {
                        s.engine.seed_cursor(x, y);
                    }
                    if !s.esc_armed && !crate::ghost::session_active() {
                        s.esc_armed = register_esc(app);
                    }
                } else if s.esc_armed {
                    // A Ghost session that started after us owns Esc now: leave it alone.
                    if !crate::ghost::session_active() {
                        unregister_esc(app);
                    }
                    s.esc_armed = false;
                }
                beep(armed);
                s.note = Some(if armed { "armed".into() } else { format!("disarmed ({reason})") });
                println!("[HAND] {} ({reason}) live={live}", if armed { "ARMED" } else { "DISARMED" });
                s.last_action = Some(if armed { "armed" } else { "disarmed" });
                s.action_seq += 1;
            }
            Action::PoseChanged { from, to, ratios, spread } => {
                println!(
                    "[HAND] pose {from} -> {to}  (finger ratios idx={:.2} mid={:.2} ring={:.2} pinky={:.2}; >1.12 = straight; spread={spread:.2}, <{:.2} = fingers together)",
                    ratios[0], ratios[1], ratios[2], ratios[3], pose::FOUR_SPREAD_MAX
                );
            }
            Action::BladeChanged { to, spread } => {
                println!("[HAND] gesture pose {to} (spread={spread:.2})");
            }
            Action::Gesture(cmd) => {
                println!("[HAND] gesture {} live={live}", cmd.id());
                s.last_action = Some(cmd.id());
                s.action_seq += 1;
                if live {
                    let _ = s.out.send(Out::Gesture(cmd));
                }
            }
            Action::ExitMode { reason } => {
                println!("[HAND] exit requested ({reason})");
                beep(false);
                let app = app.clone();
                // `stop` takes the session lock this call already holds: leave through another thread.
                std::thread::spawn(move || stop(&app, reason));
            }
            Action::Move { x, y } => {
                if live {
                    let _ = s.out.send(Out::Move(x, y));
                }
            }
            Action::Click { x, y, button } => {
                s.metrics.clicks += 1;
                s.last_action = Some(if button == Button::Left { "click" } else { "right_click" });
                s.action_seq += 1;
                if live {
                    let _ = s.out.send(Out::Click(x, y, button));
                }
            }
            Action::DragStart { x, y } => {
                s.metrics.drags += 1;
                s.last_action = Some("drag");
                s.action_seq += 1;
                if live {
                    let _ = s.out.send(Out::Press(x, y));
                }
            }
            Action::DragEnd { x, y } => {
                s.last_action = Some("drop");
                s.action_seq += 1;
                if live {
                    let _ = s.out.send(Out::Release(x, y));
                }
            }
            Action::Scroll { lines } => {
                s.metrics.scrolls += 1;
                s.last_action = Some("scroll");
                s.action_seq += 1;
                if live {
                    let _ = s.out.send(Out::Scroll(lines));
                }
            }
        }
    }
}

fn hud<R: Runtime>(app: &AppHandle<R>, s: &Session, snap: Snapshot) {
    let st = HudState {
        snap,
        live: LIVE.load(Ordering::Relaxed),
        fps: s.fps_window.2,
        last_action: s.last_action,
        action_seq: s.action_seq,
        note: s.note.clone(),
        cam_fps: s.cam_fps,
        gpu_busy: s.gpu_busy,
        covered: s.covered,
    };
    let _ = app.emit("hand:state", st);
}

/// Stop clicks reaching the OS right now: used by Esc, the mouse-takeover check and exits.
pub fn disarm<R: Runtime>(app: &AppHandle<R>, reason: &'static str) {
    let mut g = SESSION.lock();
    if let Some(s) = g.as_mut() {
        let actions = s.engine.set_armed(false, reason);
        apply(app, s, actions);
    }
}

// ── IPC from the hand window ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn hand_frame<R: Runtime>(app: AppHandle<R>, frame: FrameIn) -> Result<(), String> {
    // Speaking must never happen while SESSION is locked (TTS can take a while; every subsequent
    // frame needs this same lock, so holding it across a speak call would freeze tracking for the
    // duration). Decide inside the lock, speak after it's dropped.
    let mut notify_gpu_busy = false;
    let mut g = SESSION.lock();
    let Some(s) = g.as_mut() else { return Ok(()) };
    let now = Instant::now();
    let age = now_epoch_ms() - frame.t_capture_epoch_ms.unwrap_or(frame.t_epoch_ms);
    if !accept_frame(s.last_seq, frame.seq, age) {
        s.stale_rejected += 1;
        return Ok(());
    }
    if let Some(n) = frame.seq {
        s.last_seq = n;
    }
    s.last_frame = now;

    let hand = frame.lm.as_deref().and_then(to_hand);
    let t = s.started.elapsed().as_millis() as u64;
    // True delay from camera capture to now. The pointer is led by (most of) it.
    let capture = frame.t_capture_epoch_ms.unwrap_or(frame.t_epoch_ms);
    let lat = (now_epoch_ms() - capture).clamp(0.0, 1000.0);
    s.lat_ema = if s.lat_ema == 0.0 { lat } else { 0.8 * s.lat_ema + 0.2 * lat };
    s.engine.set_predict_ms(s.lat_ema * 0.7);
    let (actions, snap) = s.engine.step(t, hand.as_ref());

    // metrics
    s.metrics.frames += 1;
    if hand.is_some() {
        s.metrics.with_hand += 1;
    }
    if s.metrics.lat.len() < 400 {
        s.metrics.lat.push(lat as u32);
        s.metrics.infer.push(frame.infer_ms.clamp(0.0, 5000.0) as u32);
    }
    s.fps_window.1 += 1;
    let span = now.duration_since(s.fps_window.0);
    if span >= Duration::from_secs(1) {
        s.fps_window = (now, 0, s.fps_window.1 as f64 / span.as_secs_f64());
    }

    // optional landmark trace (numbers only) for offline tuning
    if let (Some(w), Some(lm)) = (s.trace.as_mut(), frame.lm.as_ref()) {
        if s.trace_bytes < TRACE_MAX_BYTES {
            let pts: Vec<[f64; 3]> = lm.iter().map(|p| [(p[0] * 1e4).round() / 1e4, (p[1] * 1e4).round() / 1e4, (p[2] * 1e4).round() / 1e4]).collect();
            if let Ok(line) = serde_json::to_string(&serde_json::json!({ "t": t, "lm": pts })) {
                s.trace_bytes += line.len() as u64 + 1;
                let _ = writeln!(w, "{line}");
            }
        }
    }

    let had_actions = !actions.is_empty();
    apply(&app, s, actions);

    if had_actions || now.duration_since(s.last_emit) >= HUD_EMIT_EVERY {
        s.last_emit = now;
        hud(&app, s, snap);
        if had_actions {
            s.note = None;
        }
    }

    if s.metrics.last_log.map_or(true, |t0| now.duration_since(t0) >= METRICS_EVERY) {
        if s.metrics.last_log.is_some() {
            let m = &mut s.metrics;
            let pct = if m.frames > 0 { m.with_hand * 100 / m.frames } else { 0 };
            let (lat, inf) = (Metrics::p50(&mut m.lat), Metrics::p50(&mut m.infer));

            // Contention detection: compare this window's infer p50 against the one-time
            // calibrated baseline for whichever delegate was picked. A third app (e.g. a browser)
            // competing for the same shared WebView2 GPU process shows up here as infer_p50
            // roughly doubling/tripling while the delegate itself never changes (it's calibrated
            // once at startup and never re-measured — see `frontend/src/hand/main.ts`).
            if s.calibrated_infer_p50_ms > 0.0 {
                let ratio = inf as f64 / s.calibrated_infer_p50_ms;
                if ratio >= 2.0 {
                    s.gpu_busy_high_windows = s.gpu_busy_high_windows.saturating_add(1);
                } else if ratio < 1.5 {
                    s.gpu_busy_high_windows = 0;
                    s.gpu_busy = false;
                    s.gpu_busy_notified = false;
                }
                if s.gpu_busy_high_windows >= 2 {
                    s.gpu_busy = true;
                }
                if s.gpu_busy && !s.gpu_busy_notified {
                    s.gpu_busy_notified = true;
                    notify_gpu_busy = true;
                }
            }

            println!(
                "[HAND] fps={:.1} hand={pct}% capture_to_rust_p50={lat}ms infer_p50={inf}ms lead={:.0}ms armed={} live={} clicks={} drags={} scrolls={} gpu_busy={} stale={}",
                s.fps_window.2,
                s.lat_ema * 0.7,
                s.engine.is_armed(),
                LIVE.load(Ordering::Relaxed),
                m.clicks,
                m.drags,
                m.scrolls,
                s.gpu_busy,
                s.stale_rejected
            );
            m.frames = 0;
            m.with_hand = 0;
            m.lat.clear();
            m.infer.clear();
        }
        s.metrics.last_log = Some(now);
    }
    drop(g);
    if notify_gpu_busy {
        let (request_id, _) = crate::orchestrator::install_new_request(crate::orchestrator::Subsystem::LocalCommand);
        crate::orchestrator::speak_line(
            &app,
            "Something on screen is using a lot of graphics power, sir — the pointer might lag.".to_string(),
            &request_id,
        );
        crate::orchestrator::clear_active_request(&request_id);
    }
    Ok(())
}

/// One-time report from the hand window: which delegate actually runs, camera format, and what the
/// calibration measured. This is what turns "it feels slow" into a number.
#[derive(Debug, Deserialize)]
pub struct HandInfo {
    pub delegate: String,
    pub camera: String,
    pub load_ms: f64,
    pub calibration: String,
    /// The chosen delegate's own calibrated infer p50 (ms); 0.0 when no numeric trial ran (e.g.
    /// GPU was unavailable at boot, so there was nothing to time before falling back to CPU).
    #[serde(default)]
    pub calibrated_ms: f64,
}

#[tauri::command]
pub async fn hand_info(info: HandInfo) -> Result<(), String> {
    println!(
        "[HAND] camera={} delegate={} model_load={:.0}ms calibration: {}",
        info.camera, info.delegate, info.load_ms, info.calibration
    );
    if info.calibrated_ms > 0.0 {
        if let Some(s) = SESSION.lock().as_mut() {
            s.calibrated_infer_p50_ms = info.calibrated_ms;
        }
    }
    Ok(())
}

/// Periodic camera/pipeline report from the hand window (every 5 s). This is the evidence for "why is it
/// slow": the real camera rate on two independent paths, processed rate, driver-reported rate, true latency.
#[derive(Debug, Deserialize)]
pub struct HandStats {
    pub source: String,
    pub cam_fps_track: f64,
    pub cam_fps_rvfc: f64,
    pub proc_fps: f64,
    pub driver_fps: f64,
    pub busy_skips: u64,
    pub dropped: u64,
    pub e2e_p50: f64,
    pub e2e_p95: f64,
    pub latency_kind: String,
    pub page: String,
    pub exposure: String,
    pub power: String,
    /// The hand window's own brightness+variance judgement that the camera looks covered.
    #[serde(default)]
    pub covered: bool,
}

#[tauri::command]
pub async fn hand_stats<R: Runtime>(app: AppHandle<R>, stats: HandStats) -> Result<(), String> {
    // Same rule as `hand_frame`: decide while locked, speak after the lock is dropped.
    let mut notify_covered = false;
    {
        let mut g = SESSION.lock();
        if let Some(s) = g.as_mut() {
            s.cam_fps = stats.cam_fps_track.max(stats.cam_fps_rvfc);
            if stats.covered && !s.covered {
                s.covered = true;
                if !s.covered_notified {
                    s.covered_notified = true;
                    notify_covered = true;
                }
            } else if !stats.covered && s.covered {
                s.covered = false;
                s.covered_notified = false;
            }
        }
    }
    println!(
        "[HAND] cam: source={} cam_fps(track)={:.1} cam_fps(rvfc)={:.1} proc_fps={:.1} driver_fps={:.0} busy_skips={} dropped={} capture_to_landmarks p50={:.0}ms p95={:.0}ms ({}) page={} exposure={} power={} covered={}",
        stats.source, stats.cam_fps_track, stats.cam_fps_rvfc, stats.proc_fps, stats.driver_fps,
        stats.busy_skips, stats.dropped, stats.e2e_p50, stats.e2e_p95, stats.latency_kind,
        stats.page, stats.exposure, stats.power, stats.covered
    );
    if notify_covered {
        let (request_id, _) = crate::orchestrator::install_new_request(crate::orchestrator::Subsystem::LocalCommand);
        crate::orchestrator::speak_line(
            &app,
            "I can't see anything, sir — your camera may be covered.".to_string(),
            &request_id,
        );
        crate::orchestrator::clear_active_request(&request_id);
    }
    Ok(())
}

#[derive(Serialize)]
pub struct HandConfig {
    frame_source: String,
    /// Draw the camera picture and skeleton in the hand window (debug). Off = nothing visible, less work.
    show_camera: bool,
}

/// Settings the hand window needs at start.
#[tauri::command]
pub async fn hand_config<R: Runtime>(app: AppHandle<R>) -> HandConfig {
    let src = read_setting(&app, "handFrameSource")
        .and_then(|v| v.as_str().map(str::to_string))
        .filter(|s| matches!(s.as_str(), "auto" | "track" | "rvfc"))
        .unwrap_or_else(|| "auto".to_string());
    HandConfig { frame_source: src, show_camera: read_bool(&app, "handShowCamera", false) }
}

/// The hand window could not start the camera or the model: tell the user and end the mode.
#[tauri::command]
pub async fn hand_error<R: Runtime>(app: AppHandle<R>, message: String) -> Result<(), String> {
    println!("[HAND] ERR {message}");
    tracing::warn!("hand: window reported: {message}");
    if active() {
        stop(&app, "error");
        let (request_id, _) = crate::orchestrator::install_new_request(crate::orchestrator::Subsystem::LocalCommand);
        crate::orchestrator::speak_line(
            &app,
            "Hand mode could not start, sir. The camera or the hand model is not available.".to_string(),
            &request_id,
        );
        crate::orchestrator::clear_active_request(&request_id);
    }
    Ok(())
}

// ── lifecycle ─────────────────────────────────────────────────────────────────────────────────

/// Every monitor (physical px, virtual-desktop coordinates) plus the primary one's size and scale.
fn desktop<R: Runtime>(app: &AppHandle<R>) -> (Vec<Mon>, (f64, f64), f64) {
    let (primary, scale) = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| ((m.size().width as f64, m.size().height as f64), m.scale_factor()))
        .unwrap_or(((1920.0, 1080.0), 1.0));
    let mons: Vec<Mon> = app
        .available_monitors()
        .map(|v| v.iter().map(|m| Mon::new(m.position().x as f64, m.position().y as f64, m.size().width as f64, m.size().height as f64)).collect())
        .unwrap_or_default();
    (if mons.is_empty() { vec![Mon::new(0.0, 0.0, primary.0, primary.1)] } else { mons }, primary, scale)
}

/// Re-read the monitor layout (plug/unplug, resolution, scale) into a running session.
fn refresh_monitors<R: Runtime>(app: &AppHandle<R>) {
    let (mons, _, _) = desktop(app);
    if let Some(s) = SESSION.lock().as_mut() {
        s.engine.set_monitors(mons);
    }
}

/// Turn hand mode on (camera window + engine). Idempotent. Returns whether the mouse is live.
pub fn start<R: Runtime>(app: &AppHandle<R>) -> Result<bool, String> {
    if !read_bool(app, "handMode", true) {
        return Err("hand mode is switched off in settings".into());
    }
    if active() {
        return Ok(LIVE.load(Ordering::Relaxed));
    }
    // Arming is the confirmation: live unless the user asked for preview.
    let live = live_supported() && !read_bool(app, "handMousePreview", false);
    LIVE.store(live, Ordering::Relaxed);

    let (mons, screen, scale) = desktop(app);
    let preview = read_bool(app, "handShowCamera", false);
    let cfg = if preview { crate::dyn_windows::WindowConfig::hand() } else { crate::dyn_windows::WindowConfig::hand_hidden() };
    let win = crate::dyn_windows::get_or_create_window(app, cfg)?;
    crate::mic_permissions::allow_media_for_window(&win);
    if preview {
        let (hw, hh) = (300.0, 250.0);
        let _ = win.set_position(tauri::LogicalPosition::new(16.0, (screen.1 / scale - hh - 72.0).max(0.0)));
        let _ = win.set_size(tauri::LogicalSize::new(hw, hh));
    } else {
        let _ = win.set_position(tauri::LogicalPosition::new((screen.0 / scale - 8.0).max(0.0), 0.0));
        let _ = win.set_size(tauri::LogicalSize::new(8.0, 8.0));
    }
    let _ = win.set_ignore_cursor_events(true);
    let _ = win.show();

    let trace = if read_bool(app, "handTrace", false) {
        app.path()
            .app_data_dir()
            .ok()
            .and_then(|d| std::fs::OpenOptions::new().create(true).append(true).open(d.join("hand_trace.jsonl")).ok())
            .map(std::io::BufWriter::new)
    } else {
        None
    };

    let now = Instant::now();
    let mut cfg = Config::new(screen);
    cfg.gestures = read_bool(app, "handGestures", true);
    cfg.gestures_desktop = read_bool(app, "handGesturesDesktop", false);
    let (sens, precision) = effective_pointer(app);
    cfg.sensitivity = sens;
    cfg.precision = precision;
    // Auto-exit after this long with no hand at all (counted from the same moment the hand first
    // disappeared that the 5 s disarm uses — not restarted at disarm). The engine's own generic
    // default (10 min, in `Config::new`) is a fallback for callers that build a `Config` directly;
    // product behaviour is 1 minute unless overridden by `handIdleExitMs`.
    cfg.idle_exit_ms = read_setting(app, "handIdleExitMs").and_then(|v| v.as_u64()).unwrap_or(60_000);
    let mut engine = Engine::new(cfg);
    engine.set_monitors(mons.clone());
    if let Some((x, y)) = cursor_pos() {
        engine.seed_cursor(x, y);
    }
    *SESSION.lock() = Some(Session {
        engine,
        started: now,
        last_frame: now,
        last_seq: 0,
        stale_rejected: 0,
        last_emit: now,
        fps_window: (now, 0, 0.0),
        out: spawn_output_thread(read_bool(app, "handSnap", true)),
        metrics: Metrics::default(),
        trace,
        trace_bytes: 0,
        esc_armed: false,
        last_action: None,
        action_seq: 0,
        note: Some("starting camera…".into()),
        lat_ema: 0.0,
        cam_fps: 0.0,
        calibrated_infer_p50_ms: 0.0,
        gpu_busy_high_windows: 0,
        gpu_busy: false,
        gpu_busy_notified: false,
        covered: false,
        covered_notified: false,
    });
    if read_bool(app, "handFocusMode", true) {
        let ep = crate::focus_mode::acquire("hand");
        FOCUS_EPOCH.store(ep, Ordering::SeqCst);
        println!("[FOCUS] on");
    }
    // Show the chip at once; the camera and the model take a few seconds.
    {
        let mut g = SESSION.lock();
        if let Some(s) = g.as_mut() {
            let snap = s.engine.idle_snapshot();
            hud(app, s, snap);
        }
    }
    println!("[HAND] mode ON live={live} primary={}x{} monitors={}", screen.0, screen.1, mons.len());
    spawn_watchdog(app.clone());
    Ok(live)
}

/// Ghost session start: begin hand mode too when `handWithGhost` is on (default off).
pub fn start_with_ghost<R: Runtime>(app: &AppHandle<R>) {
    if read_bool(app, "handWithGhost", false) && !active() && start(app).is_ok() {
        STARTED_BY_GHOST.store(true, Ordering::Relaxed);
    }
}

/// Ghost session end: end hand mode only if Ghost started it (a mode you asked for stays).
pub fn stop_if_ghost_started<R: Runtime>(app: &AppHandle<R>) {
    if STARTED_BY_GHOST.swap(false, Ordering::Relaxed) {
        stop(app, "ghost-ended");
    }
}

/// Turn hand mode off: release anything held, drop the camera window.
pub fn stop<R: Runtime>(app: &AppHandle<R>, reason: &str) {
    let taken = SESSION.lock().take();
    let Some(mut s) = taken else { return };
    let actions = s.engine.set_armed(false, "stop");
    apply(app, &mut s, actions);
    if s.esc_armed && !crate::ghost::session_active() {
        unregister_esc(app);
    }
    let _ = s.out.send(Out::Stop);
    STARTED_BY_GHOST.store(false, Ordering::Relaxed);
    let epoch = FOCUS_EPOCH.swap(0, Ordering::SeqCst);
    if epoch != 0 {
        crate::focus_mode::release(epoch);
        println!("[FOCUS] off");
    }
    let _ = crate::dyn_windows::destroy_window(app, "hand");
    let _ = app.emit("hand:stopped", ());
    println!("[HAND] mode OFF ({reason})");
}

pub fn set_live<R: Runtime>(app: &AppHandle<R>, live: bool) -> bool {
    if !active() || (live && !live_supported()) {
        return false;
    }
    if !live {
        // Release anything held while the mouse is still live, then go read-only.
        disarm(app, "preview");
    }
    LIVE.store(live, Ordering::Relaxed);
    println!("[HAND] live={live}");
    true
}

/// Voice "click here" / "double click" / "right click": click where the cursor is now. Works with the
/// hand pointer or the real mouse, with the same protected/admin-window refusal as hand clicks.
pub fn pointer_click(kind: &str) -> Result<(), String> {
    if let Some(why) = guard_now() {
        return Err(format!("I won't click in a {why}, sir."));
    }
    use enigo::{Button as B, Direction, Enigo, Mouse, Settings};
    let mut e = Enigo::new(&Settings::default()).map_err(|e| format!("mouse unavailable: {e}"))?;
    let (button, times) = match kind {
        "double" => (B::Left, 2),
        "right" => (B::Right, 1),
        _ => (B::Left, 1),
    };
    for i in 0..times {
        if i > 0 {
            std::thread::sleep(Duration::from_millis(60));
        }
        e.button(button, Direction::Click).map_err(|e| format!("click failed: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn hand_start<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    start(&app)
}

#[tauri::command]
pub async fn hand_stop<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    stop(&app, "settings");
    Ok(())
}

#[derive(Serialize)]
pub struct HandStatus {
    active: bool,
    live: bool,
}

#[tauri::command]
pub fn hand_status() -> HandStatus {
    HandStatus { active: active(), live: LIVE.load(Ordering::Relaxed) }
}

/// Watches for: the window dying, no frames arriving, and the user's own mouse taking over.
fn spawn_watchdog<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut warned = false;
        let mut ticks = 0u32;
        loop {
            tokio::time::sleep(Duration::from_millis(80)).await;
            if !active() {
                return;
            }
            crate::focus_mode::heartbeat();
            ticks = ticks.wrapping_add(1);
            if ticks % 25 == 0 {
                refresh_monitors(&app); // about every 2 s: plugging a monitor mid-session just works
            }
            if app.get_webview_window("hand").is_none() {
                stop(&app, "window-gone");
                return;
            }
            let (armed, silent_for) = {
                let g = SESSION.lock();
                match g.as_ref() {
                    Some(s) => (s.engine.is_armed(), s.last_frame.elapsed()),
                    None => return,
                }
            };
            if silent_for > NO_FRAMES_WARN && !warned {
                warned = true;
                println!("[HAND] WARN no frames for {}s: camera busy or model missing?", silent_for.as_secs());
            }
            if frames_stalled(armed, silent_for) {
                println!("[HAND] camera stalled {}ms while armed: releasing and disarming", silent_for.as_millis());
                disarm(&app, "camera-stalled");
                continue;
            }
            if armed && LIVE.load(Ordering::Relaxed) {
                let last = LAST_SET.load(Ordering::Relaxed);
                if last != i64::MIN {
                    if let Some((cx, cy)) = cursor_pos() {
                        let (lx, ly) = unpack(last);
                        if (cx - lx).abs() > MOUSE_TAKEOVER_PX || (cy - ly).abs() > MOUSE_TAKEOVER_PX {
                            disarm(&app, "mouse");
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(v: &[Out]) -> Vec<&'static str> {
        v.iter()
            .map(|m| match m {
                Out::Move(..) => "move",
                Out::Click(..) => "click",
                Out::Press(..) => "press",
                Out::Release(..) => "release",
                Out::Scroll(..) => "scroll",
                Out::Gesture(..) => "gesture",
                Out::Stop => "stop",
            })
            .collect()
    }

    #[test]
    fn queued_moves_collapse_to_the_newest_and_actions_keep_order() {
        let b = vec![Out::Move(1, 1), Out::Move(2, 2), Out::Move(3, 3), Out::Click(3, 3, Button::Left), Out::Move(4, 4), Out::Move(5, 5), Out::Release(5, 5)];
        let c = coalesce(b);
        assert_eq!(kinds(&c), ["move", "click", "move", "release"]);
        assert!(matches!(c[0], Out::Move(3, 3)));
        assert!(matches!(c[2], Out::Move(5, 5)));
    }

    #[test]
    fn a_stop_drops_pending_motion_but_keeps_releases() {
        let b = vec![Out::Move(1, 1), Out::Press(1, 1), Out::Release(2, 2), Out::Click(2, 2, Button::Left), Out::Stop];
        assert_eq!(kinds(&coalesce(b)), ["release", "stop"]);
    }

    #[test]
    fn a_hung_snap_lookup_cannot_hold_the_caller() {
        // Simulate a busy lookup: the caller must return the original point at once.
        SNAP_BUSY.store(true, Ordering::SeqCst);
        let t = Instant::now();
        assert_eq!(snapped_within(10, 20, Duration::from_millis(150)), (10, 20));
        assert!(t.elapsed() < Duration::from_millis(100));
        SNAP_BUSY.store(false, Ordering::SeqCst);
    }

    #[test]
    fn stale_duplicate_and_out_of_order_frames_are_rejected() {
        assert!(accept_frame(5, Some(6), 40.0));
        assert!(!accept_frame(5, Some(5), 40.0), "duplicate");
        assert!(!accept_frame(5, Some(4), 40.0), "out of order");
        assert!(!accept_frame(5, Some(6), 3500.0), "too old");
        assert!(accept_frame(5, None, 40.0), "older page build without a sequence");
        assert!(accept_frame(900, Some(1), 40.0), "restarted window begins again at 1");
    }

    #[test]
    fn a_frozen_camera_disarms_only_an_armed_session() {
        assert!(!frames_stalled(true, Duration::from_millis(1500)));
        assert!(frames_stalled(true, Duration::from_millis(2100)));
        assert!(!frames_stalled(false, Duration::from_secs(60)));
    }

    #[test]
    fn landmarks_need_exactly_21_points() {
        assert!(to_hand(&[[0.0; 3]; 20]).is_none());
        assert!(to_hand(&[[0.5; 3]; 21]).is_some());
    }

    #[test]
    fn packed_positions_round_trip_including_negatives() {
        for p in [(0, 0), (1919, 1079), (-5, 7), (3000, -20)] {
            assert_eq!(unpack(pack(p.0, p.1)), p);
        }
    }

    #[test]
    fn clicks_into_protected_or_admin_windows_are_blocked() {
        assert!(guard_decision("Untitled - Notepad", "notepad.exe", false, false).is_none());
        assert!(guard_decision("1Password", "1password.exe", false, false).is_some());
        assert!(guard_decision("Online Banking - Brave", "brave.exe", false, false).is_some());
        assert!(guard_decision("Task Manager", "taskmgr.exe", true, false).is_some());
        // We are admin ourselves: elevated targets are reachable.
        assert!(guard_decision("Task Manager", "taskmgr.exe", true, true).is_none());
    }
}
