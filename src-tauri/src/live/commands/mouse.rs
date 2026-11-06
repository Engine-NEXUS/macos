//! Mouse ghost — eased motion, clicks, scroll, drag, restore.
//!
//! Windows-only for now (Ghost plan Phase 2). Every motion is eased
//! (teleports break hover menus and look possessed), abort-checked per
//! step, and wrapped in the ghost suppress window so the takeover
//! detector never mistakes our own glide for a human hand.
//!
//! Ghost clicks now use the Ghost command center's live accessibility
//! lookup and hit-test. Inaccessible controls stop without a vision call.

/// Cosine-eased interpolation from `from` to `to` in `n` steps.
/// Pure — unit-tested. Ease-in-out reads as intentional motion and gives
/// the abort poll time to fire mid-glide.
pub fn eased_steps(from: (i32, i32), to: (i32, i32), n: usize) -> Vec<(i32, i32)> {
    let n = n.max(2);
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            let e = 0.5 - 0.5 * (std::f64::consts::PI * t).cos();
            (
                (from.0 as f64 + (to.0 - from.0) as f64 * e).round() as i32,
                (from.1 as f64 + (to.1 - from.1) as f64 * e).round() as i32,
            )
        })
        .collect()
}

/// Pure formatter for `PhaseTimer::summary` (unit-tested): `name=12ms name2=30ms ... total=140ms`.
pub fn format_phase_summary(marks: &[(&str, u64)], total_ms: u64) -> String {
    let mut parts: Vec<String> = marks.iter().map(|(n, ms)| format!("{n}={ms}ms")).collect();
    parts.push(format!("total={total_ms}ms"));
    parts.join(" ")
}

/// Wall-clock phases of one ghost action (P0 baseline, docs/features/103): where do the
/// ~seconds of a click actually go? `mark` records the time since the previous mark.
pub struct PhaseTimer {
    start: std::time::Instant,
    last: std::time::Instant,
    marks: Vec<(&'static str, u64)>,
}

impl PhaseTimer {
    pub fn new() -> Self {
        let now = std::time::Instant::now();
        Self { start: now, last: now, marks: Vec::new() }
    }

    pub fn mark(&mut self, name: &'static str) {
        let now = std::time::Instant::now();
        self.marks.push((name, now.duration_since(self.last).as_millis() as u64));
        self.last = now;
    }

    pub fn summary(&self) -> String {
        format_phase_summary(&self.marks, self.start.elapsed().as_millis() as u64)
    }
}

impl Default for PhaseTimer {
    fn default() -> Self {
        Self::new()
    }
}

// Also built on macOS: enigo drives the pointer there too, and the boxes from `screen::list_actionables` (Accessibility)
// are in the same points space enigo posts in. Only the cursor read, the fullscreen check and parts of the Ghost session
// click below (elevation gate, glide home) are Windows-specific.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use windows_impl::*;

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod windows_impl {
    use super::eased_steps;
    use enigo::{Axis, Button, Coordinate, Direction, Enigo, Mouse, Settings};
    use std::thread;
    use std::time::Duration;
    use tauri::Manager;

    /// Current cursor position (points). None when unreadable.
    #[cfg(target_os = "macos")]
    pub fn current_pos() -> Option<(i32, i32)> {
        crate::hand::cursor_pos()
    }

    /// Current cursor position (physical px). None when unreadable.
    #[cfg(target_os = "windows")]
    pub fn current_pos() -> Option<(i32, i32)> {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt).as_bool() } {
            Some((pt.x, pt.y))
        } else {
            None
        }
    }

    fn enigo() -> Result<Enigo, String> {
        Enigo::new(&Settings::default()).map_err(|e| format!("enigo init: {e}"))
    }

    /// Calculate human-visible glide duration based on distance (Fitts's Law).
    /// Short moves (~200px): ~550ms. Screen-spanning moves (~1500px): ~725ms.
    pub fn calculate_glide_duration(from: (i32, i32), to: (i32, i32)) -> u64 {
        let dx = (to.0 - from.0) as f64;
        let dy = (to.1 - from.1) as f64;
        let dist = (dx * dx + dy * dy).sqrt();
        let ms = 500.0 + (dist * 0.15);
        (ms as u64).clamp(550, 750)
    }

    /// Eased glide to a point. `should_stop` is polled every ~16ms step (60 FPS) —
    /// voice stop / mouse grab / Esc aborts mid-glide, never mid-click.
    /// Registers the suppress window so takeover detection stays quiet.
    pub fn move_eased(x: i32, y: i32, ms: u64, should_stop: impl Fn() -> bool) -> Result<(), String> {
        // Windows virtual-desktop coordinates can be negative when a monitor
        // sits left of or above the primary display.
        let from = current_pos().unwrap_or((x, y));
        let step_ms = 16u64;
        let steps = eased_steps(from, (x, y), ((ms / step_ms).max(4)) as usize);
        // Suppress window covers motion + easing tail (see ghost.rs).
        crate::ghost::note_expected((x, y), ms + 150);
        let mut enigo = enigo()?;
        for (sx, sy) in steps {
            if should_stop() {
                crate::ghost::note_idle();
                return Err("Stopped, sir.".to_string());
            }
            enigo
                .move_mouse(sx, sy, Coordinate::Abs)
                .map_err(|e| format!("mouse move: {e}"))?;
            thread::sleep(Duration::from_millis(step_ms));
        }
        crate::ghost::note_idle();
        Ok(())
    }

    /// Move (eased) then left-click. Atomic at the click: abort applies
    /// between steps, never mid-click — half-clicks don't exist.
    pub fn click_at(x: i32, y: i32, should_stop: impl Fn() -> bool) -> Result<(), String> {
        click_at_checked(x, y, should_stop, || Ok(()))
    }

    /// Cursor action for GhostCenter: validate the live target again after
    /// the glide and immediately before the physical click.
    pub fn click_at_checked(
        x: i32,
        y: i32,
        should_stop: impl Fn() -> bool,
        validate: impl Fn() -> Result<(), String>,
    ) -> Result<(), String> {
        let from = current_pos().unwrap_or((x, y));
        let duration = calculate_glide_duration(from, (x, y));
        move_eased(x, y, duration, &should_stop)?;
        if should_stop() {
            return Err("Stopped, sir.".to_string());
        }
        // Human arrival dwell: pause 100ms before click so the user registers landing
        thread::sleep(Duration::from_millis(100));
        if should_stop() {
            return Err("Stopped, sir.".to_string());
        }
        if current_pos() != Some((x, y)) {
            return Err("The pointer moved before I could click.".to_string());
        }
        validate()?;
        enigo()?
            .button(Button::Left, Direction::Click)
            .map_err(|e| format!("mouse click: {e}"))
    }

    /// Double-click at a point (eased move first).
    #[allow(dead_code)]
    pub fn double_click_at(x: i32, y: i32, should_stop: impl Fn() -> bool) -> Result<(), String> {
        let from = current_pos().unwrap_or((x, y));
        let duration = calculate_glide_duration(from, (x, y));
        move_eased(x, y, duration, &should_stop)?;
        if should_stop() {
            return Err("Stopped, sir.".to_string());
        }
        thread::sleep(Duration::from_millis(100));
        let mut e = enigo()?;
        e.button(Button::Left, Direction::Click)
            .map_err(|e| format!("double-click 1: {e}"))?;
        thread::sleep(Duration::from_millis(80));
        e.button(Button::Left, Direction::Click)
            .map_err(|e| format!("double-click 2: {e}"))
    }

    /// Vertical scroll by lines (positive = down). No cursor motion —
    /// takeover-safe by construction.
    #[allow(dead_code)]
    pub fn scroll(lines: i32) -> Result<(), String> {
        enigo()?
            .scroll(lines, Axis::Vertical)
            .map_err(|e| format!("mouse scroll: {e}"))
    }

    /// Drag from current position to a point (press → eased glide → release).
    #[allow(dead_code)]
    pub fn drag_to(x: i32, y: i32, should_stop: impl Fn() -> bool) -> Result<(), String> {
        if x < 0 || y < 0 {
            return Err("drag_to: negative coordinates".to_string());
        }
        let mut e = enigo()?;
        e.button(Button::Left, Direction::Press)
            .map_err(|e| format!("drag press: {e}"))?;
        let from = current_pos().unwrap_or((x, y));
        crate::ghost::note_expected((x, y), 550);
        let steps = eased_steps(from, (x, y), 12);
        for (sx, sy) in steps {
            if should_stop() {
                let _ = e.button(Button::Left, Direction::Release);
                crate::ghost::note_idle();
                return Err("Stopped, sir.".to_string());
            }
            e.move_mouse(sx, sy, Coordinate::Abs)
                .map_err(|e| format!("drag move: {e}"))?;
            thread::sleep(Duration::from_millis(25));
        }
        e.button(Button::Left, Direction::Release)
            .map_err(|e| format!("drag release: {e}"))?;
        crate::ghost::note_idle();
        Ok(())
    }

    /// Glide back to a saved position (session restore on exit).
    pub fn restore(pos: (i32, i32)) -> Result<(), String> {
        let from = current_pos().unwrap_or(pos);
        let ms = calculate_glide_duration(from, pos);
        crate::ghost::note_expected(pos, ms + 150);
        let mut e = enigo()?;
        let num_steps = ((ms / 16).max(4)) as usize;
        for (sx, sy) in super::eased_steps(from, pos, num_steps) {
            e.move_mouse(sx, sy, Coordinate::Abs)
                .map_err(|e| format!("restore move: {e}"))?;
            thread::sleep(Duration::from_millis(16));
        }
        crate::ghost::note_idle();
        Ok(())
    }

    /// Resolve an element name to a click point via UIA bounds (exact,
    /// free, millisecond latency). Scoring: exact match beats
    /// starts-with beats contains. Password/secure fields never resolve
    /// (screen.rs skips them upstream — defense in depth, not the only
    /// layer: safety denylist + confirm gates still apply).
    pub fn resolve_element(name: &str) -> Option<crate::screen::UiElement> {
        let want = name.trim();
        if want.is_empty() {
            return None;
        }
        let els = crate::screen::list_actionables();
        let mut best: Option<(u8, crate::screen::UiElement)> = None;
        for el in els {
            let Some(score) = score_name(&el.name, want) else {
                continue;
            };
            if best.as_ref().map(|(s, _)| *s >= score).unwrap_or(false) {
                continue;
            }
            best = Some((score, el));
        }
        best.map(|(_, el)| el)
    }

    /// Center point of an element for clicking.
    pub fn element_center(el: &crate::screen::UiElement) -> (i32, i32) {
        (el.x + el.w / 2, el.y + el.h / 2)
    }

    /// Pure name scorer for UIA resolution (unit-tested): exact (3) >
    /// starts-with (2) > contains (1) > no match (None). Empty never matches.
    /// Also supports conversational noise-word stripping (e.g. "search bar" -> "search")
    /// and significant token overlap for natural voice grounding.
    pub fn score_name(element_name: &str, want: &str) -> Option<u8> {
        let lower = element_name.trim().to_lowercase();
        let w = want.trim().to_lowercase();
        if w.is_empty() || lower.is_empty() {
            return None;
        }
        if lower == w {
            return Some(3);
        } else if lower.starts_with(&w) {
            return Some(2);
        } else if lower.contains(&w) {
            return Some(1);
        }

        // Noise-word stripping: "search bar" -> "search", "close button" -> "close"
        let stripped_w = w
            .strip_suffix(" bar")
            .or_else(|| w.strip_suffix(" box"))
            .or_else(|| w.strip_suffix(" button"))
            .or_else(|| w.strip_suffix(" field"))
            .or_else(|| w.strip_suffix(" input"))
            .map(|s| s.trim())
            .unwrap_or("");

        if !stripped_w.is_empty() && stripped_w != w {
            if lower == stripped_w {
                return Some(2);
            } else if lower.starts_with(stripped_w) {
                return Some(2);
            } else if lower.contains(stripped_w) {
                return Some(1);
            }
        }

        // Significant token matching (>= 3 chars, skipping conversational fillers)
        for token in w.split_whitespace() {
            if token.len() >= 3
                && !matches!(token, "the" | "and" | "bar" | "box" | "btn" | "button" | "for")
                && lower.contains(token)
            {
                return Some(1);
            }
        }

        None
    }

    /// True when the foreground window is exclusive-fullscreen (its rect
    /// covers the whole monitor): games and some players swallow synthetic
    /// input or sit above the overlay. Ghost acts pause with a spoken
    /// message instead of clicking into the void.
    #[cfg(target_os = "windows")]
    pub fn is_foreground_fullscreen() -> bool {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, IsZoomed};
        unsafe {
            let fg = GetForegroundWindow();
            if fg.0 == 0 {
                return false;
            }
            // Standard maximized windows (Brave, VSCode, Explorer, etc.) are NOT exclusive-fullscreen games.
            if IsZoomed(fg).as_bool() {
                return false;
            }
            // Whitelist standard desktop application processes — they never block Ghost mode.
            if let Some(proc) = crate::browser_url::foreground_process_name() {
                let p = proc.to_lowercase();
                if p == "brave.exe"
                    || p == "chrome.exe"
                    || p == "msedge.exe"
                    || p == "firefox.exe"
                    || p == "code.exe"
                    || p == "whatsapp.exe"
                    || p == "whatsapp.root.exe"
                    || p == "explorer.exe"
                    || p == "slack.exe"
                    || p == "discord.exe"
                    || p == "spotify.exe"
                    || p == "windowsterminal.exe"
                    || p == "cmd.exe"
                    || p == "powershell.exe"
                    || p == "pwsh.exe"
                {
                    return false;
                }
            }
            let mut r = RECT::default();
            if GetWindowRect(fg, &mut r).as_bool() {
                if let Some(monitor) = crate::screen::primary_monitor_size() {
                    return r.left <= 0
                        && r.top <= 0
                        && r.right >= monitor.0
                        && r.bottom >= monitor.1;
                }
            }
            false
        }
    }

    /// Calibration probe: glide to grid points and read back landing
    /// error. Reports max/mean deviation in physical px — the number the
    /// DPI math lives or dies by. Fails honestly when unreadable.
    /// Aborts on `should_stop` (voice stop / takeover / Esc).
    pub fn calibration_probe(should_stop: impl Fn() -> bool) -> Result<CalibrationReport, String> {
        let monitor = crate::screen::primary_monitor_size()
            .ok_or_else(|| "calibration: no monitor size".to_string())?;
        let (mw, mh) = monitor;
        let points = vec![
            (mw / 4, mh / 4),
            (mw * 3 / 4, mh / 4),
            (mw / 2, mh / 2),
            (mw / 4, mh * 3 / 4),
            (mw * 3 / 4, mh * 3 / 4),
        ];
        let mut deviations = Vec::new();
        let mut e = enigo()?;
        for (tx, ty) in &points {
            for (sx, sy) in eased_steps(current_pos().unwrap_or((*tx, *ty)), (*tx, *ty), 8) {
                if should_stop() {
                    crate::ghost::note_idle();
                    return Err("Stopped, sir.".to_string());
                }
                e.move_mouse(sx, sy, Coordinate::Abs)
                    .map_err(|e| format!("calibration move: {e}"))?;
                thread::sleep(Duration::from_millis(15));
            }
            thread::sleep(Duration::from_millis(120));
            match current_pos() {
                Some((ax, ay)) => deviations.push(((ax - tx).abs() + (ay - ty).abs()) as u32),
                None => return Err("calibration: cursor unreadable".to_string()),
            }
        }
        let max = *deviations.iter().max().unwrap_or(&0);
        let mean = if deviations.is_empty() {
            0
        } else {
            deviations.iter().sum::<u32>() / deviations.len() as u32
        };
        Ok(CalibrationReport {
            points: points.len() as u32,
            max_deviation_px: max,
            mean_deviation_px: mean,
            monitor_w: mw,
            monitor_h: mh,
        })
    }

    /// Per-machine calibration numbers (serialized to the caller).
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct CalibrationReport {
        pub points: u32,
        pub max_deviation_px: u32,
        pub mean_deviation_px: u32,
        pub monitor_w: i32,
        pub monitor_h: i32,
    }

    /// Some(reason) when ghost acts must pause: an exclusive-fullscreen
    /// foreground (games, some players) swallows synthetic input or sits
    /// above the overlay. Acting anyway clicks into the void — pause with
    /// a spoken message instead. Non-Windows: never blocks.
    pub fn foreground_blocked() -> Option<String> {
        #[cfg(target_os = "windows")]
        if is_foreground_fullscreen() {
            return Some("paused — a fullscreen app is in front, sir.".to_string());
        }
        None
    }

    /// macOS: true once an app whose name contains `app_title` (the match `focus_app_by_title` asked System Events for)
    /// is in front. System Events raises the app and returns before macOS has switched, and the Accessibility Center
    /// acts on whichever app is in front, so the click waits for the switch. Gives up after 1.5 s.
    #[cfg(target_os = "macos")]
    async fn wait_until_front(app_title: &str) -> bool {
        let want = app_title.trim().to_lowercase();
        let deadline = std::time::Instant::now() + Duration::from_millis(1500);
        loop {
            if crate::macos_foreground::front_app().is_some_and(|f| f.name.to_lowercase().contains(&want)) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    }

    /// Windows: glide the pointer back where it was (the cursor fallback may have moved it). Returns "" or the spoken
    /// note for a failed glide.
    #[cfg(target_os = "windows")]
    fn glide_home(home: Option<(i32, i32)>) -> String {
        match home {
            Some(h) if restore(h).is_err() => " (couldn't glide back)".to_string(),
            _ => String::new(),
        }
    }

    /// macOS: the Accessibility Center presses through the API and never moves the pointer, so any movement since the
    /// start is the user's own; gliding "home" would only fight them for it.
    #[cfg(target_os = "macos")]
    fn glide_home(_home: Option<(i32, i32)>) -> String {
        String::new()
    }

    /// Ghost click: full session-wrapped accessible click with verify + restore.
    /// Focus app → Ghost Center accessibility sub-center (UI Automation on
    /// Windows, the Accessibility API on macOS) → verify → glide home (Windows)
    /// → exit. Any guard trip aborts at the next step boundary; the session
    /// always exits with narration.
    pub async fn ghost_click<R: tauri::Runtime>(
        app: tauri::AppHandle<R>,
        app_title: &str,
        element: &str,
    ) -> Result<String, String> {
        fn stop() -> bool {
            crate::ghost::stop_requested()
                || !crate::ghost::session_active()
                || foreground_blocked().is_some()
        }
        let home = current_pos();
        let mut timer = super::PhaseTimer::new();

        crate::ghost::ghost_enter(crate::ghost::ghost_wry::g_wry(app.clone())).await?;
        crate::ghost::drill_begin();
        crate::ghost::announce(crate::ghost::ghost_wry::g_wry_ref(&app), "Taking the mouse, sir. Tap Esc any time to cancel.");
        timer.mark("enter_announce");

        let run = async {
            if stop() {
                return Err("Stopped, sir.".to_string());
            }
            // Fullscreen foreground pauses (not aborts): acting would
            // click into the void, so say so instead.
            if let Some(reason) = foreground_blocked() {
                return Err(reason);
            }
            // 1. Focus the app (verify — never click blind).
            if !super::super::window::focus_app_by_title(app_title) {
                return Err(format!("couldn't find {app_title} running, sir."));
            }
            #[cfg(target_os = "macos")]
            if !wait_until_front(app_title).await {
                return Err(format!("{app_title} didn't come to the front, sir."));
            }
            // 1b. UIPI elevation gate: an elevated target silently eats
            // clicks from our unelevated process — reroute with speech
            // instead of clicking into the void. (A Mac has no elevated
            // windows; the Ghost Center refuses its security prompts.)
            #[cfg(target_os = "windows")]
            if let Some(hwnd) = super::super::window::find_window(app_title) {
                let target_elevated =
                    super::super::window::window_process_elevated(hwnd);
                if let Some(reason) = super::super::window::elevated_block_reason(
                    target_elevated,
                    super::super::window::our_process_elevated(),
                ) {
                    return Err(reason);
                }
            }
            timer.mark("focus_checks");
            if stop() {
                return Err("Stopped, sir.".to_string());
            }
            // The old IPC entry joins the same Ghost sub-center as voice
            // commands. It cannot use stale list coordinates or a vision
            // fallback to bypass the new live target checks.
            let task = crate::ghost_center::GhostTask {
                steps: vec![crate::ghost_center::GhostStep::AccessibleClick {
                    name: element.to_string(),
                }],
            };
            let store_dir = app.path().app_data_dir().ok();
            let receipts = tokio::task::spawn_blocking(move || {
                crate::ghost_center::execute_with_store(
                    &task, &std::sync::atomic::AtomicBool::new(false), store_dir.as_deref()
                )
            })
            .await
            .map_err(|e| e.to_string())?;
            timer.mark("ghost_center");
            let receipt = receipts.into_iter().next()
                .ok_or_else(|| "Ghost Center returned no action result.".to_string())?;
            if receipt.outcome != crate::ghost_center::Outcome::Verified {
                return Err(receipt.spoken());
            }
            Ok::<crate::ghost_center::Receipt, String>(receipt)
        }
        .await;

        // Restore pre-act position — but NEVER after a stop/takeover:
        // gliding home into the user's hands fights them for the cursor.
        // A failed glide home is reported, never silent.
        let calm = !stop();
        let restore_note = if calm { glide_home(home) } else { String::new() };

        timer.mark("restore_home");
        let clean =
            run.is_ok() && calm && crate::ghost::session_active();
        crate::ghost::drill_end();

        // Always leave the session — success, stop, takeover, or error.
        let _ = crate::ghost::ghost_exit(crate::ghost::ghost_wry::g_wry(app.clone())).await;
        timer.mark("exit");
        tracing::info!(
            "[GHOST-TIMING] click '{}' ok={} tier={} {}",
            element,
            run.is_ok(),
            if cfg!(target_os = "macos") { "ax" } else { "uia" },
            timer.summary()
        );

        match run {
            Ok(receipt) => {
                let done = format!("{}{}", receipt.spoken(), restore_note);
                crate::ghost::announce(crate::ghost::ghost_wry::g_wry_ref(&app), &done);
                if clean {
                    // Boxed-spawn: this path returns into process_transcript,
                    // so an inline await makes the future self-referential.
                    let drain_app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        crate::orchestrator::drain_ghost_followups(&drain_app).await;
                    });
                } else {
                    crate::ghost::drop_followups();
                }
                Ok(done)
            }
            Err(e) => {
                crate::ghost::drop_followups();
                crate::ghost::announce(crate::ghost::ghost_wry::g_wry_ref(&app), &e);
                Err(e)
            }
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod unix_stub {
    /// Non-Windows: mouse ghost is Windows-only for now (Ghost plan).
    pub fn current_pos() -> Option<(i32, i32)> {
        None
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use unix_stub::current_pos;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_summary_formats_marks_and_total() {
        let s = format_phase_summary(&[("focus", 30), ("resolve_uia", 85), ("glide_dwell_click", 720)], 900);
        assert_eq!(s, "focus=30ms resolve_uia=85ms glide_dwell_click=720ms total=900ms");
        assert_eq!(format_phase_summary(&[], 0), "total=0ms");
    }

    #[test]
    fn phase_timer_records_in_order_and_total_covers_marks() {
        let mut t = PhaseTimer::new();
        t.mark("a");
        t.mark("b");
        let s = t.summary();
        assert!(s.starts_with("a=") && s.contains(" b=") && s.contains(" total="), "got {s}");
    }

    #[test]
    fn test_eased_steps_endpoints_and_count() {
        let pts = eased_steps((0, 0), (100, 50), 10);
        assert_eq!(pts.len(), 11);
        assert_eq!(pts[0], (0, 0));
        assert_eq!(pts[10], (100, 50));
    }

    #[test]
    fn test_eased_steps_monotonic_and_eased() {
        let pts = eased_steps((0, 0), (100, 0), 10);
        for w in pts.windows(2) {
            assert!(w[1].0 >= w[0].0, "must never move backwards");
        }
        // Ease-in-out: first step smaller than a middle step.
        let first = pts[1].0 - pts[0].0;
        let mid = pts[6].0 - pts[5].0;
        assert!(first <= mid, "easing shape broken");
    }

    #[test]
    fn test_eased_steps_minimum_two() {
        assert_eq!(eased_steps((5, 5), (9, 9), 0).len(), 3);
        assert_eq!(eased_steps((5, 5), (9, 9), 1).len(), 3);
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn test_score_name_ranking() {
        assert_eq!(score_name("Save", "save"), Some(3));
        assert_eq!(score_name("Save As", "save"), Some(2));
        assert_eq!(score_name("AutoSave File", "save"), Some(1));
        assert_eq!(score_name("Open", "save"), None);
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn test_score_name_normalizes_and_rejects_empty() {
        assert_eq!(score_name("  SAVE  ", "save"), Some(3));
        assert_eq!(score_name("Save", "  "), None);
        assert_eq!(score_name("", "save"), None);
        assert_eq!(score_name("Save", ""), None);
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn test_score_name_password_shaped_names_still_score() {
        // Scoring is dumb on purpose: password EXCLUSION lives in
        // screen.rs listing (skipped upstream) + the safety denylist.
        // This test pins that the scorer itself doesn't special-case,
        // so exclusion can't silently migrate here and rot.
        assert_eq!(score_name("Password", "password"), Some(3));
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn test_score_name_noise_word_stripping() {
        assert_eq!(score_name("Search", "search bar"), Some(2));
        assert_eq!(score_name("Search or start new chat", "search bar"), Some(2));
        assert_eq!(score_name("Close", "close button"), Some(2));
    }
}
