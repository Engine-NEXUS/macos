//! Keep the screen/system awake while NEXUS is speaking or working.
//!
//! Structurally a trimmed copy of [`crate::volume`]'s `Controller`/RAII-lease/heartbeat-watchdog
//! pattern — there is no baseline to restore here (the OS releases an execution-state assertion
//! when the process dies, unlike system volume, which is a persistent OS setting), and no
//! deferred-restore grace period (a flicker in sleep-prevention has no audible defect, and the
//! user explicitly wants it cleared the instant it's idle, not after a grace window).
//!
//! Two independent reasons can each hold the system awake — "NEXUS is speaking" (a lease per
//! utterance, acquired next to the existing `volume::acquire_volume_lease` calls in `tts.rs`) and
//! "NEXUS is working" (one lease, held/dropped by [`set_working`] from a 3s poll of
//! `ghost::drill_running()`/`orchestrator::has_active_request()`). Both share one refcounted
//! [`Controller`]: the screen stays awake while either reason holds a lease, and clears the
//! instant both let go.
//!
//! The Win32 contract for `SetThreadExecutionState` is inconsistently documented (some sources
//! say `ES_CONTINUOUS` holds indefinitely from one call, others imply it resets an idle timer and
//! needs periodic re-assertion). Rather than trust either reading, the watchdog re-asserts on
//! every tick while any lease is held — a free Win32 call either way, correct under both
//! interpretations.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WATCHDOG_MS: u64 = 10_000;
const WATCHDOG_POLL_MS: u64 = 500;
/// The longest a watch on a coding agent may hold the machine awake (a forgotten watch must not do it forever).
pub const MAX_WATCH_HOLD: Duration = Duration::from_secs(8 * 3600);

fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// OS access, injectable for tests.
pub trait KeepAwakeBackend: Send + Sync {
    fn assert(&self) -> Result<(), String>;
    fn clear(&self) -> Result<(), String>;
}

struct SystemBackend;

impl KeepAwakeBackend for SystemBackend {
    fn assert(&self) -> Result<(), String> {
        assert_awake()
    }
    fn clear(&self) -> Result<(), String> {
        clear_awake()
    }
}

#[derive(Default)]
struct State {
    /// Short leases (a spoken line, a request): these need a heartbeat, or the watchdog drops them.
    loose: usize,
    /// Bumped whenever every loose lease is dropped at once (watchdog, force clear), so a LATE drop of one of the
    /// dropped leases cannot release a newer, live one.
    loose_epoch: u64,
    /// Long holds (watching a coding agent for hours): id -> deadline in ms. No heartbeat needed; they end when their
    /// owner drops them or at the deadline, whichever comes first.
    pins: BTreeMap<u64, u64>,
    next_pin: u64,
    watchdog_running: bool,
}

impl State {
    fn count(&self) -> usize {
        self.loose + self.pins.len()
    }
}

pub struct Controller {
    st: Mutex<State>,
    backend: Arc<dyn KeepAwakeBackend>,
    last_beat: AtomicU64,
    watchdog: Duration,
    watchdog_poll: Duration,
}

enum Kind {
    Loose(u64),
    Pin(u64),
}

/// RAII lease: clears its slot when dropped.
pub struct KeepAwakeLease {
    ctrl: Arc<Controller>,
    kind: Kind,
    active: bool,
}

impl Drop for KeepAwakeLease {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            self.ctrl.release(&self.kind);
        }
    }
}

impl Controller {
    pub fn new(backend: Arc<dyn KeepAwakeBackend>) -> Arc<Self> {
        Self::with_timing(backend, Duration::from_millis(WATCHDOG_MS), Duration::from_millis(WATCHDOG_POLL_MS))
    }

    pub fn with_timing(backend: Arc<dyn KeepAwakeBackend>, watchdog: Duration, watchdog_poll: Duration) -> Arc<Self> {
        Arc::new(Self {
            st: Mutex::new(State::default()),
            backend,
            last_beat: AtomicU64::new(now_millis()),
            watchdog,
            watchdog_poll,
        })
    }

    pub fn heartbeat(&self) {
        self.last_beat.store(now_millis(), Ordering::Relaxed);
    }

    pub fn acquire(self: &Arc<Self>) -> Option<KeepAwakeLease> {
        let mut st = self.st.lock().unwrap_or_else(|e| e.into_inner());
        self.heartbeat();
        if !self.assert_if_first(&st) {
            return None;
        }
        st.loose += 1;
        tracing::debug!("[KEEPAWAKE] acquire count={}", st.count());
        let epoch = st.loose_epoch;
        self.ensure_watchdog(&mut st);
        drop(st);
        Some(KeepAwakeLease { ctrl: Arc::clone(self), kind: Kind::Loose(epoch), active: true })
    }

    /// A long hold that needs no heartbeat: a watch on a coding agent can sit inside a slow cloud check for longer
    /// than the watchdog allows, and must not lose the screen because of that. It ends when dropped or after `max`.
    pub fn acquire_pinned(self: &Arc<Self>, max: Duration) -> Option<KeepAwakeLease> {
        let mut st = self.st.lock().unwrap_or_else(|e| e.into_inner());
        if !self.assert_if_first(&st) {
            return None;
        }
        let id = st.next_pin;
        st.next_pin += 1;
        st.pins.insert(id, now_millis().saturating_add(max.as_millis() as u64));
        tracing::debug!("[KEEPAWAKE] pinned count={}", st.count());
        self.ensure_watchdog(&mut st);
        drop(st);
        Some(KeepAwakeLease { ctrl: Arc::clone(self), kind: Kind::Pin(id), active: true })
    }

    fn assert_if_first(&self, st: &State) -> bool {
        if st.count() == 0 {
            if let Err(e) = self.backend.assert() {
                tracing::warn!("keep_awake: failed to assert ({e}); not holding a lease");
                return false;
            }
            tracing::info!("[KEEPAWAKE] asserted");
        }
        true
    }

    fn ensure_watchdog(self: &Arc<Self>, st: &mut State) {
        if !st.watchdog_running {
            st.watchdog_running = true;
            let me = Arc::clone(self);
            std::thread::spawn(move || me.watchdog_loop());
        }
    }

    fn release(self: &Arc<Self>, kind: &Kind) {
        let mut st = self.st.lock().unwrap_or_else(|e| e.into_inner());
        let released = match kind {
            // a lease from before the last "drop everything" no longer counts for anything
            Kind::Loose(epoch) => {
                if *epoch == st.loose_epoch && st.loose > 0 {
                    st.loose -= 1;
                    true
                } else {
                    false
                }
            }
            Kind::Pin(id) => st.pins.remove(id).is_some(),
        };
        tracing::debug!("[KEEPAWAKE] release count={}", st.count());
        if released && st.count() == 0 {
            // Cleared instantly, no grace period: unlike volume, a flicker here has no
            // audible defect, and the user explicitly wants this instant.
            if let Err(e) = self.backend.clear() {
                tracing::warn!("keep_awake: failed to clear: {e}");
            } else {
                tracing::info!("[KEEPAWAKE] cleared");
            }
        }
    }

    /// Immediate clear regardless of refcount (explicit stop, app exit).
    pub fn force_clear(&self) {
        let mut st = self.st.lock().unwrap_or_else(|e| e.into_inner());
        st.loose_epoch += 1;
        if st.count() > 0 {
            st.loose = 0;
            st.pins.clear();
            let _ = self.backend.clear();
            tracing::info!("[KEEPAWAKE] force-cleared");
        }
    }

    fn watchdog_loop(self: Arc<Self>) {
        loop {
            std::thread::sleep(self.watchdog_poll);
            let mut st = self.st.lock().unwrap_or_else(|e| e.into_inner());
            let now = now_millis();
            let before = st.pins.len();
            st.pins.retain(|_, deadline| *deadline > now);
            let expired = before - st.pins.len();
            if expired > 0 {
                tracing::warn!("[KEEPAWAKE] {expired} pinned hold(s) reached their time limit and were released");
            }
            let silent = now.saturating_sub(self.last_beat.load(Ordering::Relaxed));
            let mut dropped_loose = false;
            if st.loose > 0 && silent as u128 > self.watchdog.as_millis() {
                tracing::warn!("[KEEPAWAKE] watchdog: no heartbeat for {silent} ms, dropping {} short lease(s)", st.loose);
                st.loose = 0;
                st.loose_epoch += 1;
                dropped_loose = true;
            }
            if st.count() == 0 {
                // released normally (already cleared), or everything that was left was dropped just now
                if expired > 0 || dropped_loose {
                    let _ = self.backend.clear();
                }
                st.watchdog_running = false;
                return;
            }
            // Re-assert on every tick while held: the documented Win32 contract for
            // ES_CONTINUOUS is inconsistent across sources, so don't rely on a single
            // call holding forever. A no-op-if-redundant Win32 call is cheap either way.
            let _ = self.backend.assert();
        }
    }

    #[cfg(test)]
    fn snapshot(&self) -> usize {
        self.st.lock().unwrap().count()
    }
}

// ─── Global instance + stable free-function API ────────────────────────

static CONTROLLER: OnceLock<Arc<Controller>> = OnceLock::new();
/// Holds the "NEXUS is working" lease independently of per-utterance speaking leases.
static WORKING_LEASE: Mutex<Option<KeepAwakeLease>> = Mutex::new(None);

fn ctrl() -> &'static Arc<Controller> {
    CONTROLLER.get_or_init(|| Controller::new(Arc::new(SystemBackend)))
}

/// Acquire an RAII lease (e.g. held for the duration of one spoken utterance).
pub fn acquire_lease() -> Option<KeepAwakeLease> {
    ctrl().acquire()
}

/// A long hold for watching a coding agent: needs no heartbeat, ends when dropped or after `max`.
pub fn acquire_pinned(max: Duration) -> Option<KeepAwakeLease> {
    ctrl().acquire_pinned(max)
}

/// Playback/work loops call this periodically so a long-running lease is never force-cleared.
pub fn heartbeat() {
    ctrl().heartbeat();
}

/// Immediate clear: explicit stop, app exit.
pub fn force_clear() {
    ctrl().force_clear();
    *WORKING_LEASE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Set whether the "NEXUS is working on a task" reason currently holds the screen awake.
/// Idempotent: calling with the same value twice is a no-op (no acquire/release churn).
pub fn set_working(active: bool) {
    let mut slot = WORKING_LEASE.lock().unwrap_or_else(|e| e.into_inner());
    match (active, slot.is_some()) {
        (true, false) => *slot = acquire_lease(),
        (false, true) => *slot = None,
        _ => {}
    }
}

/// Background poller: holds the screen awake while a Ghost drill or an orchestrator request is
/// running, independent of whether NEXUS is also speaking. No app handle needed — both signals
/// it reads are free functions.
pub fn start_working_poller() {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let busy = crate::ghost::drill_running() || crate::orchestrator::has_active_request();
            set_working(busy);
        }
    });
}

// ─── Windows: SetThreadExecutionState ──────────────────────────────────

#[cfg(target_os = "windows")]
fn assert_awake() -> Result<(), String> {
    use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
    // SAFETY: plain Win32 call, no pointers.
    let prev = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED) };
    if prev.0 != 0 { Ok(()) } else { Err("SetThreadExecutionState (assert) failed".into()) }
}

#[cfg(target_os = "windows")]
fn clear_awake() -> Result<(), String> {
    use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
    // SAFETY: plain Win32 call, no pointers.
    let prev = unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    if prev.0 != 0 { Ok(()) } else { Err("SetThreadExecutionState (clear) failed".into()) }
}

/// macOS: a `caffeinate -dis` child holds the display + idle assertions while
/// it lives. The watchdog's periodic re-assert doubles as a respawn check in
/// case the child died; `clear` kills it, so the screen sleeps immediately.
#[cfg(target_os = "macos")]
static CAFFEINATE: Mutex<Option<std::process::Child>> = Mutex::new(None);

#[cfg(target_os = "macos")]
fn assert_awake() -> Result<(), String> {
    let mut slot = CAFFEINATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ch) = slot.as_mut() {
        match ch.try_wait() {
            Ok(None) => return Ok(()),
            _ => {
                *slot = None;
            }
        }
    }
    let child = std::process::Command::new("caffeinate")
        .args(["-dis"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("caffeinate: {e}"))?;
    *slot = Some(child);
    Ok(())
}

#[cfg(target_os = "macos")]
fn clear_awake() -> Result<(), String> {
    let mut slot = CAFFEINATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut ch) = slot.take() {
        let _ = ch.kill();
        let _ = ch.wait();
    }
    Ok(())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn assert_awake() -> Result<(), String> {
    Err("keep_awake: unsupported platform".to_string())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn clear_awake() -> Result<(), String> {
    Err("keep_awake: unsupported platform".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Fake {
        asserted: AtomicUsize,
        cleared: AtomicUsize,
        live: Mutex<bool>,
    }
    impl Fake {
        fn new() -> Arc<Self> {
            Arc::new(Self { asserted: AtomicUsize::new(0), cleared: AtomicUsize::new(0), live: Mutex::new(false) })
        }
    }
    impl KeepAwakeBackend for Fake {
        fn assert(&self) -> Result<(), String> {
            self.asserted.fetch_add(1, Ordering::SeqCst);
            *self.live.lock().unwrap() = true;
            Ok(())
        }
        fn clear(&self) -> Result<(), String> {
            self.cleared.fetch_add(1, Ordering::SeqCst);
            *self.live.lock().unwrap() = false;
            Ok(())
        }
    }

    struct FailingBackend;
    impl KeepAwakeBackend for FailingBackend {
        fn assert(&self) -> Result<(), String> {
            Err("hardware failure".into())
        }
        fn clear(&self) -> Result<(), String> {
            Ok(())
        }
    }

    fn rig() -> (Arc<Fake>, Arc<Controller>) {
        let fake = Fake::new();
        let c = Controller::with_timing(fake.clone(), Duration::from_millis(400), Duration::from_millis(40));
        (fake, c)
    }

    fn sleep(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    #[test]
    fn acquire_asserts_and_release_clears_immediately_no_grace() {
        let (fake, c) = rig();
        let lease = c.acquire().expect("lease");
        assert!(*fake.live.lock().unwrap());
        drop(lease);
        // Unlike volume.rs, clearing is instant — no deferred-restore window to wait out.
        assert!(!*fake.live.lock().unwrap(), "must clear the instant the last lease drops");
    }

    #[test]
    fn concurrent_leases_share_one_assert_and_clear_only_when_both_drop() {
        let (fake, c) = rig();
        let a = c.acquire().unwrap();
        let b = c.acquire().unwrap();
        assert_eq!(fake.asserted.load(Ordering::SeqCst), 1, "second acquire must not re-assert");
        drop(a);
        assert!(*fake.live.lock().unwrap(), "must stay awake while one lease remains");
        drop(b);
        assert!(!*fake.live.lock().unwrap());
    }

    #[test]
    fn two_overlapping_leases_keep_state_until_last_drops() {
        let (fake, c) = rig();
        let lease_a = c.acquire().expect("lease a");
        assert!(*fake.live.lock().unwrap());
        assert_eq!(fake.asserted.load(Ordering::SeqCst), 1);
        assert_eq!(fake.cleared.load(Ordering::SeqCst), 0);

        // Second lease overlaps with the first.
        let lease_b = c.acquire().expect("lease b");
        assert_eq!(c.snapshot(), 2);
        assert_eq!(fake.asserted.load(Ordering::SeqCst), 1);

        // First lease drops while second is still held: backend must remain asserted.
        drop(lease_a);
        assert!(*fake.live.lock().unwrap(), "state must remain set while lease_b is held");
        assert_eq!(fake.cleared.load(Ordering::SeqCst), 0);
        assert_eq!(c.snapshot(), 1);

        // Second lease drops: backend must now clear.
        drop(lease_b);
        assert!(!*fake.live.lock().unwrap(), "state must clear when last lease drops");
        assert_eq!(fake.cleared.load(Ordering::SeqCst), 1);
        assert_eq!(c.snapshot(), 0);
    }

    #[test]
    fn leaked_lease_is_released_by_the_watchdog() {
        let (fake, c) = rig();
        let lease = c.acquire().unwrap();
        std::mem::forget(lease); // a task that died without dropping
        sleep(900);
        assert!(!*fake.live.lock().unwrap(), "leaked lease kept the screen awake");
        assert_eq!(c.snapshot(), 0);
    }

    #[test]
    fn long_lease_with_heartbeat_survives_the_watchdog_and_reasserts() {
        let (fake, c) = rig();
        let lease = c.acquire().unwrap();
        for _ in 0..20 {
            sleep(60); // 1.2 s total, three times the 400 ms watchdog
            c.heartbeat();
        }
        assert!(*fake.live.lock().unwrap(), "long speech/work was force-cleared");
        assert!(fake.asserted.load(Ordering::SeqCst) > 1, "watchdog should have re-asserted at least once");
        drop(lease);
        assert!(!*fake.live.lock().unwrap());
    }

    #[test]
    fn watchdog_clears_lease_whose_heartbeat_stopped() {
        let fake = Fake::new();
        // 80 ms watchdog timeout, 15 ms poll interval
        let c = Controller::with_timing(fake.clone(), Duration::from_millis(80), Duration::from_millis(15));
        let lease = c.acquire().expect("lease");
        assert!(*fake.live.lock().unwrap());
        std::mem::forget(lease); // abandoned lease, heartbeat ceases

        // Wait up to 250 ms (under 300 ms) for watchdog to clear the un-heartbeated lease.
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        while *fake.live.lock().unwrap() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(15));
        }

        assert!(!*fake.live.lock().unwrap(), "watchdog must clear un-heartbeated lease within 250 ms");
        assert_eq!(c.snapshot(), 0);
    }

    #[test]
    fn watchdog_does_not_clear_heartbeating_lease() {
        let fake = Fake::new();
        // 400 ms watchdog timeout, 15 ms poll interval. These margins were 80 ms / 20 ms, which a loaded CI machine
        // (a sleep(20) that takes 100 ms) turned into a flaky failure on macOS.
        let c = Controller::with_timing(fake.clone(), Duration::from_millis(400), Duration::from_millis(15));
        let lease = c.acquire().expect("lease");
        assert!(*fake.live.lock().unwrap());

        // Heartbeat every 50 ms for 500 ms: longer than the watchdog timeout, but each gap is 8x shorter than it.
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(50));
            c.heartbeat();
        }

        assert!(*fake.live.lock().unwrap(), "heartbeating lease must not be cleared by watchdog");
        assert_eq!(c.snapshot(), 1);

        drop(lease);
        assert!(!*fake.live.lock().unwrap());
        assert_eq!(c.snapshot(), 0);
    }

    #[test]
    fn force_clear_drops_the_live_count_regardless_of_outstanding_leases() {
        let (fake, c) = rig();
        let lease = c.acquire().unwrap();
        c.force_clear();
        assert!(!*fake.live.lock().unwrap());
        assert_eq!(c.snapshot(), 0);
        drop(lease); // the stale drop must not underflow or re-clear incorrectly
        assert_eq!(c.snapshot(), 0);
    }

    #[test]
    fn drop_after_force_clear_does_not_underflow_or_reset_state() {
        let (fake, c) = rig();
        let lease = c.acquire().expect("lease");
        assert!(*fake.live.lock().unwrap());
        assert_eq!(c.snapshot(), 1);

        c.force_clear();
        assert!(!*fake.live.lock().unwrap());
        assert_eq!(c.snapshot(), 0);
        let cleared_before = fake.cleared.load(Ordering::SeqCst);

        // Stale drop: dropping a lease whose controller was already force-cleared.
        drop(lease);
        assert_eq!(c.snapshot(), 0, "snapshot count must not underflow below zero");
        assert!(!*fake.live.lock().unwrap(), "state must not be re-set to asserted");
        assert_eq!(fake.asserted.load(Ordering::SeqCst), 1, "backend must not re-assert");
        assert_eq!(fake.cleared.load(Ordering::SeqCst), cleared_before, "backend must not re-clear");
    }

    #[test]
    fn lease_acquired_after_force_clear_works_again() {
        let (fake, c) = rig();
        let lease1 = c.acquire().expect("initial lease");
        assert!(*fake.live.lock().unwrap());
        assert_eq!(c.snapshot(), 1);

        c.force_clear();
        assert!(!*fake.live.lock().unwrap());
        assert_eq!(c.snapshot(), 0);
        drop(lease1);

        // A new lease acquired after force_clear must assert the backend and function normally.
        let lease2 = c.acquire().expect("lease after force_clear");
        assert!(*fake.live.lock().unwrap(), "backend must be re-asserted for new lease");
        assert_eq!(c.snapshot(), 1);
        assert_eq!(fake.asserted.load(Ordering::SeqCst), 2);

        drop(lease2);
        assert!(!*fake.live.lock().unwrap(), "backend must clear when lease2 is released");
        assert_eq!(c.snapshot(), 0);
    }

    #[test]
    fn acquire_returns_none_when_backend_assert_fails() {
        let c = Controller::new(Arc::new(FailingBackend));
        assert!(c.acquire().is_none(), "acquire must fail if backend assert fails");
        assert_eq!(c.snapshot(), 0);
    }

    #[test]
    fn a_pinned_hold_survives_silence_far_longer_than_the_watchdog() {
        let (fake, c) = rig();
        let pin = c.acquire_pinned(Duration::from_secs(60)).unwrap();
        sleep(1_000); // 2.5x the 400 ms watchdog, no heartbeat at all
        assert!(*fake.live.lock().unwrap(), "a watch that waits on a slow check lost the screen");
        drop(pin);
        assert!(!*fake.live.lock().unwrap());
    }

    #[test]
    fn a_pinned_hold_ends_at_its_time_limit() {
        let (fake, c) = rig();
        let pin = c.acquire_pinned(Duration::from_millis(150)).unwrap();
        std::mem::forget(pin);
        sleep(600);
        assert!(!*fake.live.lock().unwrap(), "a forgotten pin held the screen forever");
        assert_eq!(c.snapshot(), 0);
    }

    #[test]
    fn a_silent_loose_lease_is_dropped_but_a_pinned_one_stays() {
        let (fake, c) = rig();
        let pin = c.acquire_pinned(Duration::from_secs(60)).unwrap();
        let loose = c.acquire().unwrap();
        std::mem::forget(loose);
        sleep(900);
        assert!(*fake.live.lock().unwrap(), "the pinned hold must keep the screen on");
        assert_eq!(c.snapshot(), 1, "only the pinned hold is left");
        drop(pin);
        assert!(!*fake.live.lock().unwrap());
    }

    #[test]
    fn a_late_drop_of_a_dropped_lease_cannot_release_a_live_one() {
        let (fake, c) = rig();
        let old = c.acquire().unwrap();
        c.force_clear();
        let live = c.acquire().unwrap();
        drop(old); // the stale drop: before the epoch guard this took the live lease's slot
        assert!(*fake.live.lock().unwrap(), "a stale lease switched off a live hold");
        assert_eq!(c.snapshot(), 1);
        drop(live);
        assert!(!*fake.live.lock().unwrap());
    }

    #[test]
    fn set_working_is_idempotent() {
        // Exercises the global set_working API directly against the real global controller.
        // Isolated by using force_clear() first so this test doesn't depend on ordering.
        force_clear();
        set_working(true);
        set_working(true); // no-op, must not double-acquire
        set_working(false);
        set_working(false); // no-op
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "changes the real power state for a second; run by hand"]
    fn live_lease_sets_and_clears_the_real_execution_state() {
        use windows::Win32::System::Power::{
            CallNtPowerInformation, SystemExecutionState, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
        };

        struct CleanupGuard;
        impl Drop for CleanupGuard {
            fn drop(&mut self) {
                force_clear();
            }
        }

        // Clean up even if an assertion or panic occurs.
        let _guard = CleanupGuard;

        // Ensure we start with no residual keep-awake state from prior operations.
        force_clear();

        let query_execution_state = || -> u32 {
            let mut state: u32 = 0;
            // SAFETY: CallNtPowerInformation with SystemExecutionState writes a ULONG bitmask
            // to outputbuffer. inputbuffer is NULL and inputbufferlength is 0.
            let status = unsafe {
                CallNtPowerInformation(
                    SystemExecutionState,
                    std::ptr::null(),
                    0,
                    &mut state as *mut u32 as *mut std::ffi::c_void,
                    std::mem::size_of::<u32>() as u32,
                )
            };
            assert_eq!(status, 0, "CallNtPowerInformation failed with NTSTATUS {status:#x}");
            state
        };

        let required_bits = ES_SYSTEM_REQUIRED.0 | ES_DISPLAY_REQUIRED.0;

        // Acquire a real lease using the production backend.
        let lease = acquire_lease().expect("real lease must be acquired");
        let while_held = query_execution_state();
        println!("[PROOF] execution_state_while_held=0x{while_held:08x}");
        assert_eq!(
            while_held & required_bits,
            required_bits,
            "system-required and display-required bits must be set while lease is held; got 0x{while_held:08x}"
        );

        // Release the lease and poll for up to 2 seconds until the bits are cleared.
        drop(lease);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut after_release = query_execution_state();
        while (after_release & required_bits) != 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            after_release = query_execution_state();
        }

        println!("[PROOF] execution_state_after_release=0x{after_release:08x}");
        assert_eq!(
            after_release & required_bits,
            0,
            "system-required and display-required bits must be cleared after lease release; got 0x{after_release:08x}"
        );
    }
}
