//! Machine-side primitives for the "every agent finished" sequence (plan P3): tile the agents' windows into
//! equal columns, then blank the display so the user finds everything arranged on return.
//!
//! Nothing here decides WHEN to do that; the agent tracker calls it. The module is split so that every
//! decision is pure and tested, and only a thin Windows adapter touches Win32:
//!   * `columns` – equal-width layout that fills the work area exactly;
//!   * `compensate` – the invisible DWM border correction (pure arithmetic);
//!   * `find_agent_window` / `window_for_pid` / `resolve_agents` – which window belongs to which agent, and
//!     which agents share one window (a terminal hosting two agents is tiled once and said so);
//!   * `tile_windows` + `TileEnv` – the placement loop with per-window results (`PlaceResult`); an elevated
//!     window is reported and never attempted;
//!   * `may_blank_display` – the refuse-while-the-user-is-here guard; `display_off` – the Win32 call.
//!
//! Not run live in the build that added it: placement on real windows, DPI/multi-monitor behaviour and
//! whether the display wakes again on input (see `docs/changes/147-window-tile-and-display-off.md`).


use crate::session_observer::ProcSnap;

// ─── Geometry ────────────────────────────────────────────────────────

/// A screen rectangle in physical pixels, same meaning as Win32 `RECT` (right/bottom exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub(crate) fn width(&self) -> i32 {
        self.right - self.left
    }
    pub(crate) fn height(&self) -> i32 {
        self.bottom - self.top
    }
}

/// Split `work` into `n` side-by-side columns that fill it exactly: no gaps, no overlap, every column the
/// full height. When the width does not divide evenly the leftover pixels go one each to the leftmost
/// columns, so widths differ by at most one pixel. `n == 0` gives no columns.
pub(crate) fn columns(work: Rect, n: usize) -> Vec<Rect> {
    let w = work.width();
    // Fewer pixels than columns cannot be tiled: no zero-width columns are promised.
    if n == 0 || w < n as i32 || work.height() <= 0 {
        return Vec::new();
    }
    let n = n as i32;
    let base = w / n;
    let extra = w % n;
    let mut out = Vec::with_capacity(n as usize);
    let mut x = work.left;
    for i in 0..n {
        let cw = base + if i < extra { 1 } else { 0 };
        out.push(Rect { left: x, top: work.top, right: x + cw, bottom: work.bottom });
        x += cw;
    }
    out
}

/// A top-level window's own rectangle includes an invisible resize border on Windows 10/11, so asking for
/// the visible column straight away leaves visible gaps and overlaps. Grow the request by the difference
/// between the window rect (`win`) and what is actually drawn (`frame`, DWM extended frame bounds).
pub(crate) fn compensate(target: Rect, win: Rect, frame: Rect) -> Rect {
    Rect {
        left: target.left - (frame.left - win.left),
        top: target.top - (frame.top - win.top),
        right: target.right + (win.right - frame.right),
        bottom: target.bottom + (win.bottom - frame.bottom),
    }
}

// ─── Finding the windows ─────────────────────────────────────────────

/// One top-level window as the tiler sees it (same filter as the shutdown closer, plus its owning pid).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopWin {
    pub hwnd: isize,
    pub pid: u32,
    /// lower-case exe name; empty when unreadable (elevated / protected process)
    pub exe: String,
    pub class: String,
    pub title: String,
}

/// How to recognise an agent's window by name. A window matches when ANY exe word is part of its exe name
/// or ANY title word is part of its title (case-insensitive). Keep the words specific: "claude" as a title
/// word also matches a browser tab called "Claude".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AgentSpec {
    pub exe_words: Vec<String>,
    pub title_words: Vec<String>,
}

const SHELL_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

/// Windows that may be tiled at all: titled, not a shell surface, not NEXUS itself.
pub(crate) fn tileable(w: &TopWin) -> bool {
    !w.title.trim().is_empty() && w.exe != "nexus.exe" && !SHELL_CLASSES.contains(&w.class.as_str())
}

fn contains_any(hay: &str, words: &[String]) -> bool {
    let hay = hay.to_lowercase();
    words.iter().any(|w| !w.trim().is_empty() && hay.contains(&w.trim().to_lowercase()))
}

/// The topmost tileable window matching `spec`; a window matched by its exe beats one matched only by its
/// title (the title of a terminal tab is weaker evidence than the program that owns the window).
pub(crate) fn find_agent_window(spec: &AgentSpec, wins: &[TopWin]) -> Option<isize> {
    let ok: Vec<&TopWin> = wins.iter().filter(|w| tileable(w)).collect();
    ok.iter()
        .find(|w| !w.exe.is_empty() && contains_any(&w.exe, &spec.exe_words))
        .or_else(|| ok.iter().find(|w| contains_any(&w.title, &spec.title_words)))
        .map(|w| w.hwnd)
}

/// Walk from an agent's process (e.g. `node.exe` running claude) up through its parents to the first
/// process that owns a tileable top-level window, and return that window. `prefer_title` picks between
/// several windows of one process (Windows Terminal hosts all its windows in one process). The walk stops
/// at `explorer.exe`: a File Explorer window is never "the agent's window".
pub(crate) fn window_for_pid(pid: u32, procs: &[ProcSnap], wins: &[TopWin], prefer_title: &[String]) -> Option<isize> {
    let mut cur = pid;
    let mut seen: Vec<u32> = Vec::new();
    while cur != 0 && !seen.contains(&cur) && seen.len() < 64 {
        seen.push(cur);
        let owned: Vec<&TopWin> =
            wins.iter().filter(|w| w.pid == cur && tileable(w) && w.exe != "explorer.exe").collect();
        if !owned.is_empty() {
            let pick = owned.iter().find(|w| contains_any(&w.title, prefer_title)).unwrap_or(&owned[0]);
            return Some(pick.hwnd);
        }
        let p = procs.iter().find(|p| p.pid == cur)?;
        if p.name == "explorer.exe" {
            return None;
        }
        cur = p.ppid;
    }
    None
}

/// One tracked agent, as the tracker knows it. Either handle may be missing; the pid is tried first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentTarget {
    pub name: String,
    pub pid: Option<u32>,
    pub spec: Option<AgentSpec>,
}

/// One window to tile and the agents that live in it (more than one = a shared window).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tile {
    pub hwnd: isize,
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Resolution {
    /// Distinct windows in the order their first agent was given (that order becomes left-to-right).
    pub tiles: Vec<Tile>,
    /// Agents whose window could not be found.
    pub missing: Vec<String>,
}

impl Tile {
    /// "Claude Code and Codex share one window, so it is tiled once." Empty for an unshared window.
    pub(crate) fn shared_line(&self) -> Option<String> {
        match self.agents.as_slice() {
            [] | [_] => None,
            [a, b] => Some(format!("{a} and {b} share one window, so I tiled it once.")),
            [rest @ .., last] => Some(format!("{} and {last} share one window, so I tiled it once.", rest.join(", "))),
        }
    }
}

/// Resolve every agent to a window and merge agents that resolve to the same window.
pub(crate) fn resolve_agents(agents: &[AgentTarget], procs: &[ProcSnap], wins: &[TopWin]) -> Resolution {
    let mut out = Resolution::default();
    for a in agents {
        let prefer: &[String] = a.spec.as_ref().map(|s| s.title_words.as_slice()).unwrap_or(&[]);
        let hwnd = a
            .pid
            .and_then(|p| window_for_pid(p, procs, wins, prefer))
            .or_else(|| a.spec.as_ref().and_then(|s| find_agent_window(s, wins)));
        match hwnd {
            None => out.missing.push(a.name.clone()),
            Some(h) => match out.tiles.iter_mut().find(|t| t.hwnd == h) {
                Some(t) => t.agents.push(a.name.clone()),
                None => out.tiles.push(Tile { hwnd: h, agents: vec![a.name.clone()] }),
            },
        }
    }
    out
}

// ─── Placing ─────────────────────────────────────────────────────────

/// What happened to one window; the caller can speak each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlaceResult {
    Placed,
    /// The window runs elevated and NEXUS does not: Windows would ignore the move, so it is not tried.
    SkippedElevated,
    /// The window closed (or its handle went stale) before it could be placed.
    SkippedGone,
    /// Anything else, with the reason ("the window would not shrink to the column", ...).
    Failed(String),
    /// This platform has no window adapter.
    Unsupported,
}

/// The machine side of placement; `RealTileEnv` is the Windows one, tests use a fake.
pub(crate) trait TileEnv {
    /// Work area (taskbar excluded) of the monitor to tile on.
    fn work_area(&self) -> Option<Rect>;
    fn exists(&self, hwnd: isize) -> bool;
    fn elevated(&self, hwnd: isize) -> bool;
    fn self_elevated(&self) -> bool;
    /// Restore, move and size one window so that its VISIBLE frame is exactly `rect`.
    fn place(&self, hwnd: isize, rect: Rect) -> PlaceResult;
}

/// Tile `hwnds` left to right in equal columns. A window that is gone or elevated is reported and takes no
/// column, so the rest still fill the whole width. Duplicate handles are placed once.
pub(crate) fn tile_windows(env: &dyn TileEnv, hwnds: &[isize]) -> Vec<(isize, PlaceResult)> {
    let mut order: Vec<isize> = Vec::new();
    for h in hwnds {
        if !order.contains(h) {
            order.push(*h);
        }
    }
    let self_elev = env.self_elevated();
    let mut results: Vec<(isize, Option<PlaceResult>)> = Vec::new();
    let mut live: Vec<isize> = Vec::new();
    for h in &order {
        if !env.exists(*h) {
            results.push((*h, Some(PlaceResult::SkippedGone)));
        } else if crate::live::commands::window::elevated_block_reason(env.elevated(*h), self_elev).is_some() {
            results.push((*h, Some(PlaceResult::SkippedElevated)));
        } else {
            live.push(*h);
            results.push((*h, None));
        }
    }
    let cols = env.work_area().map(|w| columns(w, live.len()));
    for (h, slot) in results.iter_mut() {
        if slot.is_some() {
            continue;
        }
        let idx = live.iter().position(|l| l == h).unwrap_or(0);
        *slot = Some(match cols.as_ref().and_then(|c| c.get(idx)) {
            Some(rect) => env.place(*h, *rect),
            None => PlaceResult::Failed("no usable work area".to_string()),
        });
    }
    results.into_iter().map(|(h, r)| (h, r.unwrap_or(PlaceResult::Unsupported))).collect()
}

// ─── Display off ─────────────────────────────────────────────────────

/// The user touched keyboard or mouse more recently than this: the screen stays on.
pub(crate) const MIN_IDLE_SECS: u64 = 60;

/// Why the display may not be blanked right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// The user used the keyboard or mouse less than `MIN_IDLE_SECS` ago.
    UserActive { idle_secs: u64 },
    /// A meeting is on (a dark screen on a shared call is not a harmless thing to do).
    Meeting,
    /// The idle time could not be read; not knowing counts as "the user may be here".
    IdleUnknown,
}

/// Pure guard. `user_idle_secs` comes from `memcore::resume::user_idle_secs()` (None = unreadable),
/// `meeting_active` from the meeting detector. Both conditions are checked every time; the meeting wins
/// the explanation because it holds however long the user has been away.
pub(crate) fn may_blank_display(user_idle_secs: Option<u64>, meeting_active: bool) -> Result<(), Why> {
    if meeting_active {
        return Err(Why::Meeting);
    }
    match user_idle_secs {
        None => Err(Why::IdleUnknown),
        Some(s) if s < MIN_IDLE_SECS => Err(Why::UserActive { idle_secs: s }),
        Some(_) => Ok(()),
    }
}

// ─── Windows adapter ─────────────────────────────────────────────────

#[cfg(target_os = "windows")]
pub(crate) struct RealTileEnv;

/// Every tileable top-level window now, topmost first, with its owning pid. Reuses the shutdown closer's
/// enumeration (`power_close::RealEnv::list`), which keeps its state on the stack and is thread-safe.
#[cfg(target_os = "windows")]
pub(crate) fn list_top_windows() -> Vec<TopWin> {
    use crate::power_close::{CloseEnv, RealEnv};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    RealEnv
        .list()
        .into_iter()
        .map(|w| {
            let mut pid = 0u32;
            // SAFETY: a stale handle just leaves pid at 0.
            unsafe {
                GetWindowThreadProcessId(HWND(w.hwnd), &mut pid);
            }
            TopWin { hwnd: w.hwnd, pid, exe: w.exe, class: w.class, title: w.title }
        })
        .filter(tileable)
        .collect()
}

#[cfg(target_os = "windows")]
fn to_rect(r: windows::Win32::Foundation::RECT) -> Rect {
    Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

#[cfg(target_os = "windows")]
impl TileEnv for RealTileEnv {
    /// Primary monitor's work area (taskbar excluded). Per-monitor tiling is a v2 option.
    fn work_area(&self) -> Option<Rect> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
        // SAFETY: `info` is a correctly sized MONITORINFO; a null window handle selects the primary monitor.
        unsafe {
            let mon = MonitorFromWindow(HWND(0), MONITOR_DEFAULTTOPRIMARY);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(mon, &mut info).as_bool() {
                Some(to_rect(info.rcWork))
            } else {
                None
            }
        }
    }

    fn exists(&self, hwnd: isize) -> bool {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        // SAFETY: IsWindow accepts any value.
        unsafe { IsWindow(HWND(hwnd)).as_bool() }
    }

    fn elevated(&self, hwnd: isize) -> bool {
        use windows::Win32::Foundation::HWND;
        crate::live::commands::window::window_process_elevated(HWND(hwnd))
    }

    fn self_elevated(&self) -> bool {
        crate::live::commands::window::our_process_elevated()
    }

    fn place(&self, hwnd: isize, rect: Rect) -> PlaceResult {
        use std::ffi::c_void;
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowRect, IsIconic, IsWindow, IsZoomed, SetWindowPos, ShowWindow, SW_RESTORE, SWP_NOACTIVATE,
            SWP_NOZORDER,
        };
        let h = HWND(hwnd);
        // SAFETY: every call takes a window handle that may be stale (they then fail, which is handled) and
        // RECTs that live on this stack frame.
        unsafe {
            if !IsWindow(h).as_bool() {
                return PlaceResult::SkippedGone;
            }
            // A minimised or maximised window ignores SetWindowPos until it is back to its normal state.
            if IsIconic(h).as_bool() || IsZoomed(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            let mut wr = RECT::default();
            if !GetWindowRect(h, &mut wr).as_bool() {
                return PlaceResult::SkippedGone;
            }
            let mut fb = RECT::default();
            let dwm_ok = DwmGetWindowAttribute(
                h,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut fb as *mut RECT as *mut c_void,
                std::mem::size_of::<RECT>() as u32,
            )
            .is_ok();
            // No DWM answer (classic theme, odd window): no border to compensate.
            let ask = if dwm_ok { compensate(rect, to_rect(wr), to_rect(fb)) } else { rect };
            if !SetWindowPos(h, HWND(0), ask.left, ask.top, ask.width(), ask.height(), SWP_NOACTIVATE | SWP_NOZORDER)
                .as_bool()
            {
                return PlaceResult::Failed("Windows refused to move the window".to_string());
            }
            // A window with a minimum size wider than the column silently keeps its own size: say so.
            let mut after = RECT::default();
            let seen = if dwm_ok {
                DwmGetWindowAttribute(
                    h,
                    DWMWA_EXTENDED_FRAME_BOUNDS,
                    &mut after as *mut RECT as *mut c_void,
                    std::mem::size_of::<RECT>() as u32,
                )
                .is_ok()
            } else {
                GetWindowRect(h, &mut after).as_bool()
            };
            if seen && !close_enough(rect, to_rect(after)) {
                return PlaceResult::Failed("the window would not fit the column (minimum size?)".to_string());
            }
            PlaceResult::Placed
        }
    }
}

/// Same rectangle within a few pixels (rounding at fractional DPI scales).
#[cfg(any(target_os = "windows", test))]
fn close_enough(want: Rect, got: Rect) -> bool {
    near(want, got, 3)
}

/// Same rectangle within `tol` on every edge.
#[cfg(any(target_os = "windows", target_os = "macos", test))]
pub(crate) fn near(want: Rect, got: Rect, tol: i32) -> bool {
    (want.left - got.left).abs() <= tol
        && (want.top - got.top).abs() <= tol
        && (want.right - got.right).abs() <= tol
        && (want.bottom - got.bottom).abs() <= tol
}

/// Turn the display off (it comes back on the next key press or mouse move). The message is sent with a
/// timeout and `SMTO_ABORTIFHUNG` to the desktop window: never a blocking `SendMessage(HWND_BROADCAST)`,
/// which can hang the caller on one unresponsive window. Callers must check `may_blank_display` first.
#[cfg(target_os = "windows")]
pub(crate) fn display_off() -> Result<(), String> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetDesktopWindow, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_NORMAL, WM_SYSCOMMAND,
    };
    /// SC_MONITORPOWER
    const SC_MONITORPOWER: usize = 0xF170;
    /// lParam 2 = power the monitor off (1 = low power, -1 = on)
    const MONITOR_OFF: isize = 2;
    // SAFETY: plain message send to the desktop window with a bounded wait.
    let ok = unsafe {
        let mut result = 0usize;
        SendMessageTimeoutW(
            GetDesktopWindow(),
            WM_SYSCOMMAND,
            WPARAM(SC_MONITORPOWER),
            LPARAM(MONITOR_OFF),
            SMTO_ABORTIFHUNG | SMTO_NORMAL,
            1000,
            &mut result,
        )
    };
    if ok.0 != 0 {
        Ok(())
    } else {
        Err("the desktop did not answer the display-off request".to_string())
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) struct RealTileEnv;

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub(crate) fn list_top_windows() -> Vec<TopWin> {
    Vec::new()
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
impl TileEnv for RealTileEnv {
    fn work_area(&self) -> Option<Rect> {
        None
    }
    fn exists(&self, _hwnd: isize) -> bool {
        true
    }
    fn elevated(&self, _hwnd: isize) -> bool {
        false
    }
    fn self_elevated(&self) -> bool {
        false
    }
    fn place(&self, _hwnd: isize, _rect: Rect) -> PlaceResult {
        PlaceResult::Unsupported
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub(crate) fn display_off() -> Result<(), String> {
    Err("unsupported on this platform".to_string())
}

// ─── macOS adapter ───────────────────────────────────────────────────
//
// Same contract as the Windows adapter, through the Accessibility API (needs the Accessibility permission; without it
// the window list is empty and nothing moves). Geometry is in points, top-left origin, the space AXPosition/AXSize use,
// so `columns` and `tile_windows` run unchanged. A window "handle" is a small number standing for an AXUIElement kept in
// a process-wide table (`mac::handle_for`): macOS has no public window id that Accessibility can act on.
//
// Decisions are pure and tested on every OS (`flip_to_top_left`, `screens_top_left`, `screen_at`, `inner_rect`,
// `rounded_rect`, `mac_app_considered`, `mac_window_considered`, `mac_title`, `front_first`); `mac` is the thin glue.

/// A rectangle in points (macOS screen and Accessibility geometry): origin and size.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PtRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Cocoa (`NSScreen`) rectangles have their origin at the bottom-left of the primary screen with y growing upwards; the
/// Accessibility API puts it at the top-left of the primary screen with y growing downwards. `primary_height` is the
/// height of the primary screen (`NSScreen.screens[0]`, the one holding the menu bar origin), whichever screen `r` is on.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn flip_to_top_left(r: PtRect, primary_height: f64) -> PtRect {
    PtRect { x: r.x, y: primary_height - (r.y + r.h), w: r.w, h: r.h }
}

/// One display in top-left points: its whole frame, and the part windows may use (menu bar and Dock excluded).
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScreenFrames {
    pub frame: PtRect,
    pub visible: PtRect,
}

/// Convert every screen's Cocoa `(frame, visibleFrame)`, primary screen first (the order `NSScreen.screens` gives), to
/// top-left points. Empty in, empty out.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn screens_top_left(cocoa: &[(PtRect, PtRect)]) -> Vec<ScreenFrames> {
    let Some((primary, _)) = cocoa.first() else { return Vec::new() };
    let ph = primary.h;
    cocoa
        .iter()
        .map(|(f, v)| ScreenFrames { frame: flip_to_top_left(*f, ph), visible: flip_to_top_left(*v, ph) })
        .collect()
}

/// The screen whose frame holds the point (left/top edges inside, right/bottom edges outside), first match wins.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn screen_at(screens: &[ScreenFrames], x: f64, y: f64) -> Option<usize> {
    screens.iter().position(|s| {
        let f = s.frame;
        x >= f.x && x < f.x + f.w && y >= f.y && y < f.y + f.h
    })
}

/// Snap a value that is an integer up to floating-point noise (Cocoa conversions give 24.999999 for 25).
#[cfg(any(target_os = "macos", test))]
fn snap(v: f64) -> f64 {
    if (v - v.round()).abs() < 0.01 {
        v.round()
    } else {
        v
    }
}

/// Whole-point edges with every edge inside `p` (rounded inwards), so a window put there never slides under the menu bar
/// or the Dock. None when `p` is not finite, does not fit an i32, or has no area left.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn inner_rect(p: PtRect) -> Option<Rect> {
    edges(p, |l| l.ceil(), |r| r.floor())
}

/// Whole-point edges nearest to `p` (a frame read back from a window, for comparison).
#[cfg(any(target_os = "macos", test))]
pub(crate) fn rounded_rect(p: PtRect) -> Option<Rect> {
    edges(p, f64::round, f64::round)
}

#[cfg(any(target_os = "macos", test))]
fn edges(p: PtRect, low: impl Fn(f64) -> f64, high: impl Fn(f64) -> f64) -> Option<Rect> {
    let vals = [p.x, p.y, p.x + p.w, p.y + p.h];
    if !vals.iter().all(|v| v.is_finite() && v.abs() < 1e9) {
        return None;
    }
    let [l, t, r, b] = vals.map(snap);
    let (left, top, right, bottom) = (low(l), low(t), high(r), high(b));
    (right > left && bottom > top)
        .then(|| Rect { left: left as i32, top: top as i32, right: right as i32, bottom: bottom as i32 })
}

/// A whole-point rectangle as points.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn pt_rect(r: Rect) -> PtRect {
    PtRect { x: r.left as f64, y: r.top as f64, w: r.width() as f64, h: r.height() as f64 }
}

/// Whose windows the tiler looks at: ordinary Dock apps (activation policy "regular", so no menu-bar extras or
/// background agents), not hidden with Cmd-H, never NEXUS itself.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac_app_considered(regular: bool, hidden: bool, pid: u32, own_pid: u32) -> bool {
    regular && !hidden && pid != 0 && pid != own_pid
}

/// Which windows are tiled: standard app/document windows (not dialogs, sheets, floating panels or palettes) that are on
/// screen (not minimised into the Dock) and not in full screen (such a window owns a Space of its own and cannot be
/// moved).
#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac_window_considered(subrole: &str, minimized: bool, fullscreen: bool) -> bool {
    subrole == "AXStandardWindow" && !minimized && !fullscreen
}

/// The title the resolver and `tileable` see: the window's own, or the app's name for an untitled window (so it is not
/// dropped as "untitled" and can still be found by the app's name).
#[cfg(any(target_os = "macos", test))]
pub(crate) fn mac_title(title: &str, app_name: &str) -> String {
    let t = title.trim();
    if t.is_empty() {
        app_name.trim().to_string()
    } else {
        t.to_string()
    }
}

/// The front app's entries first, everything else in its original order (macOS lists apps by launch, not by depth, and
/// the resolver takes the first match as the topmost).
#[cfg(any(target_os = "macos", test))]
pub(crate) fn front_first<T>(items: Vec<T>, is_front: impl Fn(&T) -> bool) -> Vec<T> {
    let (mut front, rest): (Vec<T>, Vec<T>) = items.into_iter().partition(|i| is_front(i));
    front.extend(rest);
    front
}

/// How far a placed window may end up from its column, in points. Terminals snap their size to whole character cells
/// (up to a line height short); a window with a minimum size wider than the column misses by far more.
#[cfg(any(target_os = "macos", test))]
pub(crate) const MAC_FIT_TOL: i32 = 20;

/// Every tileable window now, the front app's first (then in app launch order; within one app front to back).
#[cfg(target_os = "macos")]
pub(crate) fn list_top_windows() -> Vec<TopWin> {
    mac::list_windows().into_iter().filter(tileable).collect()
}

#[cfg(target_os = "macos")]
impl TileEnv for RealTileEnv {
    /// Visible frame (menu bar and Dock excluded) of the main screen, the one with the key window.
    fn work_area(&self) -> Option<Rect> {
        let s = mac::screens()?;
        inner_rect(s.list.get(s.main)?.visible)
    }
    fn exists(&self, hwnd: isize) -> bool {
        mac::window_alive(hwnd)
    }
    /// No UAC on macOS: Accessibility either may move every window or none.
    fn elevated(&self, _hwnd: isize) -> bool {
        false
    }
    fn self_elevated(&self) -> bool {
        false
    }
    fn place(&self, hwnd: isize, rect: Rect) -> PlaceResult {
        mac::place(hwnd, rect)
    }
}

/// Put the displays to sleep now (they wake on the next key press or mouse move), with `pmset displaysleepnow`: no
/// admin rights needed. If the user's "require password after the screen saver begins or display is turned off" is set
/// to immediately, macOS also locks the screen, which the Windows monitor-off does not. Callers must check
/// `may_blank_display` first.
#[cfg(target_os = "macos")]
pub(crate) fn display_off() -> Result<(), String> {
    let mut cmd = std::process::Command::new("/usr/bin/pmset");
    cmd.arg("displaysleepnow");
    crate::macos_foreground::run_capture(&mut cmd, std::time::Duration::from_secs(5))
        .map(|_| ())
        .ok_or_else(|| "pmset did not put the display to sleep".to_string())
}

/// The macOS glue: running apps and activation (`NSWorkspace` / `NSRunningApplication`), screen frames (`NSScreen`, on
/// the main thread), and window geometry through the Accessibility helpers in `ax_mac::mac`. Also used by
/// `live::commands::window` for focus, minimise and maximise.
#[cfg(target_os = "macos")]
pub(crate) mod mac {
    use super::{PlaceResult, PtRect, Rect, ScreenFrames, TopWin};
    use crate::ax_mac::mac::{self as ax, Owned, CGPoint, CGSize, AX_VALUE_CGPOINT, AX_VALUE_CGSIZE};
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation_sys::base::{CFEqual, CFTypeRef};
    use core_foundation_sys::string::CFStringRef;
    use objc2::encode::{Encode, Encoding};
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    use std::ffi::{c_char, c_void, CStr};
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::{mpsc, Mutex, Once};
    use std::time::Duration;

    // NSRect / NSPoint / NSSize are the CoreGraphics structs on 64-bit macOS; these carry the encodings `msg_send!`
    // checks in debug builds.
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NsPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NsSize {
        w: f64,
        h: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct NsRect {
        origin: NsPoint,
        size: NsSize,
    }
    // SAFETY: the layouts above are exactly CGPoint {double x, y}, CGSize {double width, height} and CGRect {origin, size}.
    unsafe impl Encode for NsPoint {
        const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl Encode for NsSize {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl Encode for NsRect {
        const ENCODING: Encoding = Encoding::Struct("CGRect", &[NsPoint::ENCODING, NsSize::ENCODING]);
    }

    impl NsRect {
        fn pt(self) -> PtRect {
            PtRect { x: self.origin.x, y: self.origin.y, w: self.size.w, h: self.size.h }
        }
    }

    /// The string inside an NSString, empty for nil.
    ///
    /// SAFETY: `s` is null or a live NSString, and the caller holds an autorelease pool.
    unsafe fn ns_string(s: *mut AnyObject) -> String {
        if s.is_null() {
            return String::new();
        }
        let p: *const c_char = msg_send![s, UTF8String];
        if p.is_null() {
            String::new()
        } else {
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }

    // ── apps ──

    /// One running application, as `NSWorkspace` reports it.
    #[derive(Debug, Clone)]
    pub(crate) struct RunningApp {
        pub pid: u32,
        /// Display name, e.g. `Google Chrome`.
        pub name: String,
        /// Activation policy "regular": an ordinary app with a Dock icon (not a menu-bar extra or background agent).
        pub regular: bool,
        pub hidden: bool,
    }

    /// Every running application. Needs no permission.
    pub(crate) fn running_apps() -> Vec<RunningApp> {
        let Some(cls) = AnyClass::get(c"NSWorkspace") else { return Vec::new() };
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: every receiver is checked for nil before it is messaged; selectors and types follow NSWorkspace.h
            // and NSRunningApplication.h (pid_t, NSInteger, BOOL, NSString *).
            unsafe {
                let ws: *mut AnyObject = msg_send![cls, sharedWorkspace];
                if ws.is_null() {
                    return Vec::new();
                }
                let apps: *mut AnyObject = msg_send![ws, runningApplications];
                if apps.is_null() {
                    return Vec::new();
                }
                let n: usize = msg_send![apps, count];
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    let a: *mut AnyObject = msg_send![apps, objectAtIndex: i];
                    if a.is_null() {
                        continue;
                    }
                    let pid: i32 = msg_send![a, processIdentifier];
                    if pid <= 0 {
                        continue;
                    }
                    let policy: isize = msg_send![a, activationPolicy];
                    let hidden: bool = msg_send![a, isHidden];
                    let name: *mut AnyObject = msg_send![a, localizedName];
                    out.push(RunningApp { pid: pid as u32, name: ns_string(name), regular: policy == 0, hidden });
                }
                out
            }
        })
    }

    /// Ask macOS to bring the app with this pid to the front, un-hiding it first. Needs no permission (no Apple Events:
    /// no Automation prompt). From macOS 14 a background app's request may be declined (cooperative activation); callers
    /// fall back to the Accessibility `AXFrontmost` attribute.
    pub(crate) fn activate_app(pid: u32) -> bool {
        let Some(cls) = AnyClass::get(c"NSRunningApplication") else { return false };
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: the app object is checked for nil; selectors and types follow NSRunningApplication.h.
            unsafe {
                let app: *mut AnyObject = msg_send![cls, runningApplicationWithProcessIdentifier: pid as i32];
                if app.is_null() {
                    return false;
                }
                let hidden: bool = msg_send![app, isHidden];
                if hidden {
                    let _: bool = msg_send![app, unhide];
                }
                // NSApplicationActivateIgnoringOtherApps (1 << 1); ignored from macOS 14 on, needed before it.
                msg_send![app, activateWithOptions: 2usize]
            }
        })
    }

    // ── screens ──

    /// Every display in top-left points, primary first, and which one is the main screen (the one with the key window).
    pub(crate) struct Screens {
        pub list: Vec<ScreenFrames>,
        pub main: usize,
    }

    /// The displays now. None when AppKit does not answer (no window server, or the main thread is busy for over 1.5 s).
    pub(crate) fn screens() -> Option<Screens> {
        let (cocoa, main) = on_main(read_screens_cocoa, Duration::from_millis(1500)).flatten()?;
        let list = super::screens_top_left(&cocoa);
        if list.is_empty() {
            return None;
        }
        let main = main.min(list.len() - 1);
        Some(Screens { list, main })
    }

    /// `(frame, visibleFrame)` of every screen in Cocoa coordinates, primary first, and the index of the main screen.
    /// AppKit declares NSScreen main-thread only: run through `on_main`.
    fn read_screens_cocoa() -> Option<(Vec<(PtRect, PtRect)>, usize)> {
        let cls = AnyClass::get(c"NSScreen")?;
        objc2::rc::autoreleasepool(|_| {
            // SAFETY: on the main thread (see `on_main`); every object is checked for nil before it is messaged.
            unsafe {
                let arr: *mut AnyObject = msg_send![cls, screens];
                if arr.is_null() {
                    return None;
                }
                let main: *mut AnyObject = msg_send![cls, mainScreen];
                let n: usize = msg_send![arr, count];
                let mut out = Vec::with_capacity(n);
                let mut main_idx = 0;
                for i in 0..n {
                    let s: *mut AnyObject = msg_send![arr, objectAtIndex: i];
                    if s.is_null() {
                        continue;
                    }
                    if s == main {
                        main_idx = out.len();
                    }
                    let f: NsRect = msg_send![s, frame];
                    let v: NsRect = msg_send![s, visibleFrame];
                    out.push((f.pt(), v.pt()));
                }
                (!out.is_empty()).then_some((out, main_idx))
            }
        })
    }

    /// Run `f` on the main thread and wait up to `wait` for its answer: directly when already there, otherwise queued on
    /// the main dispatch queue (which the app's run loop drains). Never blocks the main thread on the caller, so it
    /// cannot deadlock; a late answer is dropped. None on timeout or if `f` panicked.
    ///
    /// Test builds call `f` on the calling thread: libtest runs every test off the main thread and its main thread runs
    /// no run loop, so a queued call would never be answered there.
    fn on_main<T, F>(f: F, wait: Duration) -> Option<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        if cfg!(test) || objc2::MainThreadMarker::new().is_some() {
            return Some(f());
        }
        // libdispatch is part of libSystem; `dispatch_get_main_queue()` is a macro for `&_dispatch_main_q`.
        #[allow(non_upper_case_globals)]
        extern "C" {
            static _dispatch_main_q: u8;
            fn dispatch_async_f(queue: *const u8, context: *mut c_void, work: extern "C" fn(*mut c_void));
        }
        extern "C" fn trampoline<T, F: FnOnce() -> T>(ctx: *mut c_void) {
            // SAFETY: `ctx` is the box leaked below, handed to exactly one call of this function.
            let job = unsafe { Box::from_raw(ctx as *mut (F, mpsc::SyncSender<T>)) };
            let (f, tx) = *job;
            if let Ok(v) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
                let _ = tx.send(v);
            }
        }
        let (tx, rx) = mpsc::sync_channel::<T>(1);
        let ctx = Box::into_raw(Box::new((f, tx))) as *mut c_void;
        // SAFETY: the main queue is a static that lives for the process; `trampoline` takes ownership of `ctx`.
        unsafe { dispatch_async_f(std::ptr::addr_of!(_dispatch_main_q), ctx, trampoline::<T, F>) };
        rx.recv_timeout(wait).ok()
    }

    // ── Accessibility ──

    static TIMEOUT_SET: Once = Once::new();

    /// The application element for `pid`, with the process-wide per-call limit set (a hung app must not stall the voice
    /// pipeline for the 6 s system default; same 0.5 s as `ax_mac`).
    pub(crate) fn app_element(pid: u32) -> Option<Owned> {
        TIMEOUT_SET.call_once(|| {
            // SAFETY: plain constructor (+1 reference, released by `Owned`) and a plain setter on it.
            if let Some(sys) = Owned::new(unsafe { ax::AXUIElementCreateSystemWide() }) {
                unsafe { ax::AXUIElementSetMessagingTimeout(sys.0, 0.5) };
            }
        });
        // SAFETY: plain constructor; returns a +1 reference.
        Owned::new(unsafe { ax::AXUIElementCreateApplication(pid as i32) })
    }

    /// The app's windows, front to back (the current Space's; minimised ones included).
    pub(crate) fn windows_of(app: &Owned) -> Vec<Owned> {
        // SAFETY: `children_of` checks the value is a CFArray before reading it.
        ax::copy_attr(app, "AXWindows").map(|v| unsafe { ax::children_of(v.0) }).unwrap_or_default()
    }

    pub(crate) fn string_attr(el: &Owned, attr: &str) -> String {
        // SAFETY: `string_of` checks the type.
        ax::copy_attr(el, attr).map(|v| unsafe { ax::string_of(v.0) }).unwrap_or_default()
    }

    pub(crate) fn bool_attr(el: &Owned, attr: &str) -> Option<bool> {
        // SAFETY: `bool_of` checks the type.
        ax::copy_attr(el, attr).and_then(|v| unsafe { ax::bool_of(v.0) })
    }

    /// The window's frame in top-left points.
    pub(crate) fn frame_of(w: &Owned) -> Option<PtRect> {
        // SAFETY: `ax_value` checks the AXValue type before reading it.
        let p: CGPoint = ax::copy_attr(w, "AXPosition").and_then(|v| unsafe { ax::ax_value(v.0, AX_VALUE_CGPOINT) })?;
        let s: CGSize = ax::copy_attr(w, "AXSize").and_then(|v| unsafe { ax::ax_value(v.0, AX_VALUE_CGSIZE) })?;
        Some(PtRect { x: p.x, y: p.y, w: s.w, h: s.h })
    }

    fn set_value(el: &Owned, attr: &str, kind: u32, value: *const c_void) -> bool {
        // SAFETY: `value` points at a live CGPoint/CGSize matching `kind`; AXValueCreate copies it (+1 reference).
        let Some(v) = Owned::new(unsafe { ax::AXValueCreate(kind, value) }) else { return false };
        let name = ax::cf_string(attr);
        // SAFETY: `el` is a live AXUIElement, `name` a CFString and `v` an AXValue.
        unsafe { ax::AXUIElementSetAttributeValue(el.0, name.0 as CFStringRef, v.0 as CFTypeRef) == 0 }
    }

    /// Move the window to `r` (top-left points), size it, then move it again: moving first keeps a growing window from
    /// being pushed back by the screen edge, and some apps shift the window while they resize. True when macOS accepted
    /// the move or the resize; callers read the frame back to see what the app really did.
    pub(crate) fn set_frame(w: &Owned, r: PtRect) -> bool {
        let p = CGPoint { x: r.x, y: r.y };
        let s = CGSize { w: r.w, h: r.h };
        let pp = &p as *const CGPoint as *const c_void;
        let moved = set_value(w, "AXPosition", AX_VALUE_CGPOINT, pp);
        let sized = set_value(w, "AXSize", AX_VALUE_CGSIZE, &s as *const CGSize as *const c_void);
        let _ = set_value(w, "AXPosition", AX_VALUE_CGPOINT, pp);
        moved || sized
    }

    pub(crate) fn perform(el: &Owned, action: &str) -> bool {
        let name = ax::cf_string(action);
        // SAFETY: `el` is a live AXUIElement and `name` a CFString.
        unsafe { ax::AXUIElementPerformAction(el.0, name.0 as CFStringRef) == 0 }
    }

    /// Several attributes of one element in a single cross-process call, one entry per name (None where missing).
    fn read_attrs(el: &Owned, names: &ax::Names) -> Option<Vec<Option<Owned>>> {
        let mut values: CFArrayRef = std::ptr::null();
        // SAFETY: `el` is a live AXUIElement and `names.array` a CFArray of CFStrings; `values` receives a +1 array.
        let err = unsafe { ax::AXUIElementCopyMultipleAttributeValues(el.0, names.array.0 as CFArrayRef, 0, &mut values) };
        if err != 0 {
            return None;
        }
        let values = Owned::new(values as CFTypeRef)?;
        let arr = values.0 as CFArrayRef;
        // SAFETY: indices stay below the count; each entry is retained before `values` is released.
        let n = unsafe { CFArrayGetCount(arr) };
        Some((0..n).map(|i| Owned::retain(unsafe { CFArrayGetValueAtIndex(arr, i) })).collect())
    }

    // ── window handles ──

    /// A window element kept for a handle.
    struct Kept {
        id: isize,
        el: Owned,
    }
    // SAFETY: an AXUIElementRef is an immutable CF object; retain/release are thread-safe and Accessibility calls may be
    // made with it from any thread.
    unsafe impl Send for Kept {}

    static KEPT: Mutex<Vec<Kept>> = Mutex::new(Vec::new());
    static NEXT_ID: AtomicIsize = AtomicIsize::new(1);
    /// Handles remembered at once; the oldest are forgotten first (a forgotten handle reads as a closed window).
    const KEEP_MAX: usize = 256;

    /// The handle of a window element: the same window always gets the same handle (CFEqual), a new one the next
    /// number. 0 only when the table cannot be used.
    fn handle_for(el: &Owned) -> isize {
        let Ok(mut kept) = KEPT.lock() else { return 0 };
        // SAFETY: both are live CF objects; CFEqual on two AXUIElements compares them locally.
        if let Some(k) = kept.iter().find(|k| unsafe { CFEqual(k.el.0, el.0) } != 0) {
            return k.id;
        }
        let Some(el) = Owned::retain(el.0) else { return 0 };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        kept.push(Kept { id, el });
        if kept.len() > KEEP_MAX {
            let extra = kept.len() - KEEP_MAX;
            kept.drain(..extra);
        }
        id
    }

    fn window_for(id: isize) -> Option<Owned> {
        let kept = KEPT.lock().ok()?;
        Owned::retain(kept.iter().find(|k| k.id == id)?.el.0)
    }

    /// True while the window behind the handle still answers.
    pub(crate) fn window_alive(id: isize) -> bool {
        window_for(id).is_some_and(|w| ax::copy_attr(&w, "AXRole").is_some())
    }

    /// Test helper: the frame of the window behind a handle, in whole points.
    #[cfg(test)]
    pub(crate) fn frame_by_handle(id: isize) -> Option<Rect> {
        window_for(id).and_then(|w| frame_of(&w)).and_then(super::rounded_rect)
    }

    // ── the adapter ──

    /// Every standard, on-screen window of the ordinary apps (see `mac_app_considered` / `mac_window_considered`), the
    /// front app's first. Empty without the Accessibility permission.
    pub(crate) fn list_windows() -> Vec<TopWin> {
        if !ax::trusted() {
            tracing::warn!("[TILE] Accessibility permission missing: no windows to list");
            return Vec::new();
        }
        let own = std::process::id();
        let front = crate::macos_foreground::front_app().map(|f| f.pid);
        let apps: Vec<RunningApp> = running_apps()
            .into_iter()
            .filter(|a| super::mac_app_considered(a.regular, a.hidden, a.pid, own))
            .collect();
        let apps = super::front_first(apps, |a| Some(a.pid) == front);
        let names = ax::names(&["AXSubrole", "AXTitle", "AXMinimized", "AXFullScreen"]);
        let mut out = Vec::new();
        for app in &apps {
            let Some(el) = app_element(app.pid) else { continue };
            for w in windows_of(&el) {
                let Some(v) = read_attrs(&w, &names) else { continue };
                let get = |i: usize| v.get(i).and_then(|o| o.as_ref()).map(|o| o.0).unwrap_or(std::ptr::null());
                // SAFETY: the readers check each value's CF type; the values live while `v` is held.
                let (subrole, title, minimized, fullscreen) = unsafe {
                    (
                        ax::string_of(get(0)),
                        ax::string_of(get(1)),
                        ax::bool_of(get(2)).unwrap_or(false),
                        ax::bool_of(get(3)).unwrap_or(false),
                    )
                };
                if !super::mac_window_considered(&subrole, minimized, fullscreen) {
                    continue;
                }
                let hwnd = handle_for(&w);
                if hwnd == 0 {
                    continue;
                }
                out.push(TopWin {
                    hwnd,
                    pid: app.pid,
                    exe: app.name.to_lowercase(),
                    class: subrole,
                    title: super::mac_title(&title, &app.name),
                });
            }
        }
        out
    }

    /// Put one window exactly on `rect` (top-left points), restoring it from the Dock first.
    pub(crate) fn place(hwnd: isize, rect: Rect) -> PlaceResult {
        let Some(w) = window_for(hwnd) else { return PlaceResult::SkippedGone };
        if ax::copy_attr(&w, "AXRole").is_none() {
            return PlaceResult::SkippedGone;
        }
        if bool_attr(&w, "AXFullScreen") == Some(true) {
            return PlaceResult::Failed("the window is in full screen".to_string());
        }
        if bool_attr(&w, "AXMinimized") == Some(true) {
            ax::set_bool(&w, "AXMinimized", false);
            // the window animates out of the Dock first; a move during that animation is lost
            std::thread::sleep(Duration::from_millis(400));
        }
        if !set_frame(&w, super::pt_rect(rect)) {
            return PlaceResult::Failed("macOS refused to move the window".to_string());
        }
        match frame_of(&w).and_then(super::rounded_rect) {
            Some(got) if !super::near(rect, got, super::MAC_FIT_TOL) => {
                PlaceResult::Failed("the window would not fit the column (minimum size?)".to_string())
            }
            _ => PlaceResult::Placed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn r(l: i32, t: i32, rr: i32, b: i32) -> Rect {
        Rect { left: l, top: t, right: rr, bottom: b }
    }

    // ── layout ──

    /// Every property a tiling must have, for one work area and column count.
    fn check_tiling(work: Rect, n: usize) {
        let cols = columns(work, n);
        assert_eq!(cols.len(), n);
        let mut x = work.left;
        for c in &cols {
            assert_eq!(c.left, x, "gap or overlap before {c:?} in {work:?} n={n}");
            assert!(c.width() > 0);
            assert_eq!((c.top, c.bottom), (work.top, work.bottom));
            x = c.right;
        }
        assert_eq!(x, work.right, "columns must end exactly at the right edge");
        let (min, max) = cols.iter().fold((i32::MAX, i32::MIN), |(lo, hi), c| (lo.min(c.width()), hi.max(c.width())));
        assert!(max - min <= 1, "widths differ by more than one pixel: {cols:?}");
        // the leftover pixels go to the leftmost columns
        let widths: Vec<i32> = cols.iter().map(Rect::width).collect();
        assert!(widths.windows(2).all(|p| p[0] >= p[1]), "wider columns must come first: {widths:?}");
    }

    #[test]
    fn columns_fill_the_area_for_one_to_six() {
        for n in 1..=6 {
            check_tiling(r(0, 0, 1920, 1040), n);
            check_tiling(r(0, 0, 1921, 1040), n); // odd
            check_tiling(r(0, 0, 1366, 728), n);
            check_tiling(r(0, 0, 2559, 1400), n);
        }
    }

    #[test]
    fn columns_odd_width_1919_fills_exactly_for_one_to_six() {
        for n in 1..=6 {
            check_tiling(r(0, 0, 1919, 1040), n);
            check_tiling(r(10, 20, 1929, 1060), n);
        }
    }

    #[test]
    fn columns_with_negative_origin_monitor() {
        // a monitor to the left of and above the primary
        for n in 1..=6 {
            check_tiling(r(-1920, -200, 0, 840), n);
            check_tiling(r(-1367, -1, 0, 727), n);
        }
        let c = columns(r(-1920, -200, 0, 840), 2);
        assert_eq!(c[0], r(-1920, -200, -960, 840));
        assert_eq!(c[1], r(-960, -200, 0, 840));
    }

    #[test]
    fn columns_edge_cases() {
        let w = r(10, 20, 1930, 1060);
        assert!(columns(w, 0).is_empty());
        assert_eq!(columns(w, 1), vec![w]);
        assert!(columns(r(0, 0, 0, 100), 3).is_empty());
        // narrower than the column count: nothing is promised rather than zero-width columns
        assert!(columns(r(0, 0, 3, 100), 4).is_empty());
        assert_eq!(columns(r(0, 0, 4, 100), 4).len(), 4);
    }

    #[test]
    fn remainder_pixels_go_one_each_to_the_leftmost_columns() {
        // 1000 / 3 = 333 remainder 1 -> 334, 333, 333
        let w: Vec<i32> = columns(r(0, 0, 1000, 10), 3).iter().map(Rect::width).collect();
        assert_eq!(w, vec![334, 333, 333]);
        // 1000 / 6 = 166 remainder 4 -> four 167s then two 166s
        let w: Vec<i32> = columns(r(0, 0, 1000, 10), 6).iter().map(Rect::width).collect();
        assert_eq!(w, vec![167, 167, 167, 167, 166, 166]);
        assert_eq!(w.iter().sum::<i32>(), 1000);
    }

    #[test]
    fn compensate_grows_the_request_by_the_invisible_border() {
        // typical Win10/11: 7 px invisible border left/right/bottom, none on top
        let win = r(100, 100, 900, 700);
        let frame = r(107, 100, 893, 693);
        let ask = compensate(r(0, 0, 960, 1040), win, frame);
        assert_eq!(ask, r(-7, 0, 967, 1047));
        // after the window is placed at `ask`, its visible frame is the column again
        assert_eq!(ask.left + 7, 0);
        assert_eq!(ask.right - 7, 960);
        // no border: unchanged
        assert_eq!(compensate(r(0, 0, 10, 10), win, win), r(0, 0, 10, 10));
    }

    // ── display-off guard ──

    #[test]
    fn guard_truth_table() {
        assert_eq!(may_blank_display(Some(61), false), Ok(()));
        assert_eq!(may_blank_display(Some(60), false), Ok(())); // exactly 60 s away is enough
        assert_eq!(may_blank_display(Some(59), false), Err(Why::UserActive { idle_secs: 59 }));
        assert_eq!(may_blank_display(Some(0), false), Err(Why::UserActive { idle_secs: 0 }));
        assert_eq!(may_blank_display(Some(3600), true), Err(Why::Meeting)); // away for an hour, still a meeting
        assert_eq!(may_blank_display(Some(5), true), Err(Why::Meeting));
        assert_eq!(may_blank_display(None, false), Err(Why::IdleUnknown));
        assert_eq!(may_blank_display(None, true), Err(Why::Meeting));
    }

    // ── resolver ──

    fn ps(pid: u32, ppid: u32, name: &str) -> ProcSnap {
        ProcSnap { pid, ppid, name: name.into(), cmd: name.into(), cpu_pct: 0.0, io_total: 0 }
    }
    fn tw(hwnd: isize, pid: u32, exe: &str, class: &str, title: &str) -> TopWin {
        TopWin { hwnd, pid, exe: exe.into(), class: class.into(), title: title.into() }
    }
    fn spec(exe: &[&str], title: &[&str]) -> AgentSpec {
        AgentSpec {
            exe_words: exe.iter().map(|s| s.to_string()).collect(),
            title_words: title.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn world() -> (Vec<ProcSnap>, Vec<TopWin>) {
        let procs = vec![
            ps(1, 0, "explorer.exe"),
            ps(100, 1, "windowsterminal.exe"),
            ps(101, 100, "pwsh.exe"),
            ps(102, 101, "node.exe"), // claude
            ps(201, 100, "pwsh.exe"),
            ps(202, 201, "node.exe"), // codex, same terminal
            ps(300, 1, "antigravity.exe"),
            ps(400, 1, "code.exe"),
            ps(401, 400, "node.exe"), // an agent inside an IDE window
            ps(900, 1, "orphan.exe"), // no window anywhere above it
        ];
        let wins = vec![
            tw(500, 100, "windowsterminal.exe", "CASCADIA_HOSTING_WINDOW_CLASS", "claude - pwsh"),
            tw(600, 300, "antigravity.exe", "Chrome_WidgetWin_1", "NEXUS - Antigravity"),
            tw(700, 400, "code.exe", "Chrome_WidgetWin_1", "main.rs - Visual Studio Code"),
            tw(800, 1, "explorer.exe", "CabinetWClass", "Downloads"),
            tw(801, 1, "explorer.exe", "Shell_TrayWnd", "Taskbar"),
        ];
        (procs, wins)
    }

    #[test]
    fn pid_walk_reaches_the_terminal_window() {
        let (procs, wins) = world();
        assert_eq!(window_for_pid(102, &procs, &wins, &[]), Some(500));
        assert_eq!(window_for_pid(401, &procs, &wins, &[]), Some(700));
        assert_eq!(window_for_pid(300, &procs, &wins, &[]), Some(600)); // the process owns its own window
    }

    #[test]
    fn pid_walk_never_lands_on_explorer_or_loops() {
        let (procs, wins) = world();
        // orphan.exe -> explorer.exe: the Downloads folder window must NOT be taken
        assert_eq!(window_for_pid(900, &procs, &wins, &[]), None);
        // unknown pid
        assert_eq!(window_for_pid(5555, &procs, &wins, &[]), None);
        // a parent cycle terminates
        let cyc = vec![ps(1, 2, "a.exe"), ps(2, 1, "b.exe")];
        assert_eq!(window_for_pid(1, &cyc, &wins, &[]), None);
    }

    #[test]
    fn pid_walk_prefers_the_window_named_like_the_agent() {
        let (procs, mut wins) = world();
        wins.insert(0, tw(499, 100, "windowsterminal.exe", "CASCADIA_HOSTING_WINDOW_CLASS", "ssh server"));
        assert_eq!(window_for_pid(102, &procs, &wins, &[]), Some(499)); // topmost without a hint
        assert_eq!(window_for_pid(102, &procs, &wins, &["claude".to_string()]), Some(500));
    }

    #[test]
    fn name_match_prefers_exe_over_title_and_skips_shell_windows() {
        let (_, wins) = world();
        assert_eq!(find_agent_window(&spec(&["antigravity"], &[]), &wins), Some(600));
        assert_eq!(find_agent_window(&spec(&[], &["visual studio code"]), &wins), Some(700));
        // exe hit on a later window beats a title hit on an earlier one
        let mixed = vec![tw(1, 10, "brave.exe", "Chrome_WidgetWin_1", "Claude"), tw(2, 11, "claude.exe", "X", "whatever")];
        assert_eq!(find_agent_window(&spec(&["claude"], &["claude"]), &mixed), Some(2));
        // the taskbar and NEXUS itself are never candidates
        assert_eq!(find_agent_window(&spec(&["explorer"], &["taskbar"]), &wins), Some(800));
        let ours = vec![tw(3, 12, "nexus.exe", "X", "NEXUS")];
        assert_eq!(find_agent_window(&spec(&["nexus"], &["nexus"]), &ours), None);
        // no words = no match
        assert_eq!(find_agent_window(&AgentSpec::default(), &wins), None);
    }

    fn target(name: &str, pid: Option<u32>, s: Option<AgentSpec>) -> AgentTarget {
        AgentTarget { name: name.into(), pid, spec: s }
    }

    #[test]
    fn shared_window_is_deduped_and_reported() {
        let (procs, wins) = world();
        let agents = vec![
            target("Claude Code", Some(102), None),
            target("Antigravity", None, Some(spec(&["antigravity"], &[]))),
            target("Codex", Some(202), None), // same terminal as Claude Code
        ];
        let res = resolve_agents(&agents, &procs, &wins);
        assert_eq!(res.missing, Vec::<String>::new());
        assert_eq!(
            res.tiles,
            vec![
                Tile { hwnd: 500, agents: vec!["Claude Code".into(), "Codex".into()] },
                Tile { hwnd: 600, agents: vec!["Antigravity".into()] },
            ]
        );
        assert_eq!(res.tiles[0].shared_line().as_deref(), Some("Claude Code and Codex share one window, so I tiled it once."));
        assert_eq!(res.tiles[1].shared_line(), None);
    }

    #[test]
    fn missing_window_is_reported_not_tiled() {
        let (procs, wins) = world();
        let agents = vec![
            target("Aider", Some(900), Some(spec(&[], &["aider"]))), // pid walk fails, title finds nothing
            target("Ghost", None, None),
            target("Cursor", None, Some(spec(&["cursor"], &[]))),
        ];
        let res = resolve_agents(&agents, &procs, &wins);
        assert!(res.tiles.is_empty());
        assert_eq!(res.missing, vec!["Aider", "Ghost", "Cursor"]);
    }

    #[test]
    fn pid_that_fails_falls_back_to_the_name() {
        let (procs, wins) = world();
        let res = resolve_agents(&[target("Antigravity", Some(900), Some(spec(&["antigravity"], &[])))], &procs, &wins);
        assert_eq!(res.tiles, vec![Tile { hwnd: 600, agents: vec!["Antigravity".into()] }]);
    }

    #[test]
    fn three_way_share_line() {
        let t = Tile { hwnd: 1, agents: vec!["A".into(), "B".into(), "C".into()] };
        assert_eq!(t.shared_line().as_deref(), Some("A, B and C share one window, so I tiled it once."));
    }

    // ── tile_windows with a fake machine ──

    struct Fake {
        work: Option<Rect>,
        gone: Vec<isize>,
        elevated: Vec<isize>,
        self_elevated: bool,
        refuse: Vec<isize>,
        placed: RefCell<Vec<(isize, Rect)>>,
    }
    impl Fake {
        fn new() -> Self {
            Fake { work: Some(r(0, 0, 1000, 600)), gone: vec![], elevated: vec![], self_elevated: false, refuse: vec![], placed: RefCell::new(vec![]) }
        }
    }
    impl TileEnv for Fake {
        fn work_area(&self) -> Option<Rect> {
            self.work
        }
        fn exists(&self, h: isize) -> bool {
            !self.gone.contains(&h)
        }
        fn elevated(&self, h: isize) -> bool {
            self.elevated.contains(&h)
        }
        fn self_elevated(&self) -> bool {
            self.self_elevated
        }
        fn place(&self, h: isize, rect: Rect) -> PlaceResult {
            if self.refuse.contains(&h) {
                return PlaceResult::Failed("nope".into());
            }
            self.placed.borrow_mut().push((h, rect));
            PlaceResult::Placed
        }
    }

    #[test]
    fn tiles_in_order_across_the_whole_width() {
        let env = Fake::new();
        let out = tile_windows(&env, &[5, 6, 7]);
        assert!(out.iter().all(|(_, r)| *r == PlaceResult::Placed));
        let placed = env.placed.borrow();
        assert_eq!(placed[0], (5, r(0, 0, 334, 600)));
        assert_eq!(placed[1], (6, r(334, 0, 667, 600)));
        assert_eq!(placed[2], (7, r(667, 0, 1000, 600)));
    }

    #[test]
    fn elevated_and_gone_windows_are_reported_and_take_no_column() {
        let mut env = Fake::new();
        env.elevated = vec![6];
        env.gone = vec![8];
        let out = tile_windows(&env, &[5, 6, 7, 8]);
        assert_eq!(
            out,
            vec![
                (5, PlaceResult::Placed),
                (6, PlaceResult::SkippedElevated),
                (7, PlaceResult::Placed),
                (8, PlaceResult::SkippedGone),
            ]
        );
        let placed = env.placed.borrow();
        // two live windows share the full width; the elevated one was never touched
        assert_eq!(*placed, vec![(5, r(0, 0, 500, 600)), (7, r(500, 0, 1000, 600))]);
    }

    #[test]
    fn elevated_window_is_placed_when_nexus_is_elevated_too() {
        let mut env = Fake::new();
        env.elevated = vec![5];
        env.self_elevated = true;
        assert_eq!(tile_windows(&env, &[5]), vec![(5, PlaceResult::Placed)]);
    }

    #[test]
    fn duplicates_failures_and_missing_work_area() {
        let mut env = Fake::new();
        env.refuse = vec![6];
        let out = tile_windows(&env, &[5, 5, 6]);
        assert_eq!(out, vec![(5, PlaceResult::Placed), (6, PlaceResult::Failed("nope".into()))]);
        assert_eq!(env.placed.borrow().len(), 1);

        let mut none = Fake::new();
        none.work = None;
        let out = tile_windows(&none, &[5]);
        assert!(matches!(out[0].1, PlaceResult::Failed(_)));
        assert!(tile_windows(&Fake::new(), &[]).is_empty());
    }

    #[test]
    fn display_off_has_the_expected_shape_and_is_never_called_here() {
        // taken as a fn pointer only: no test may blank the machine running it
        let f: fn() -> Result<(), String> = display_off;
        let _ = f;
    }

    #[test]
    fn close_enough_tolerates_rounding_only() {
        assert!(close_enough(r(0, 0, 100, 100), r(1, -2, 102, 99)));
        assert!(!close_enough(r(0, 0, 100, 100), r(0, 0, 140, 100)));
    }

    // ── macOS geometry and filters (pure; run on every OS) ──

    fn pt(x: f64, y: f64, w: f64, h: f64) -> PtRect {
        PtRect { x, y, w, h }
    }

    #[test]
    fn near_uses_the_given_tolerance_on_every_edge() {
        assert!(near(r(0, 25, 720, 900), r(0, 25, 720, 900), 0));
        assert!(near(r(0, 25, 720, 900), r(0, 25, 714, 884), MAC_FIT_TOL), "a terminal snapped to its character grid");
        assert!(!near(r(0, 25, 720, 900), r(0, 25, 800, 900), MAC_FIT_TOL), "a minimum width wider than the column");
        assert!(!near(r(0, 25, 720, 900), r(0, 60, 720, 900), MAC_FIT_TOL));
    }

    #[test]
    fn cocoa_rects_flip_to_top_left_points() {
        // MacBook: 1512x982 screen, 33 pt menu bar, 60 pt Dock at the bottom
        let ph = 982.0;
        let frame = pt(0.0, 0.0, 1512.0, 982.0);
        let visible = pt(0.0, 60.0, 1512.0, 889.0); // Cocoa: starts above the Dock, ends under the menu bar
        assert_eq!(flip_to_top_left(frame, ph), frame);
        assert_eq!(flip_to_top_left(visible, ph), pt(0.0, 33.0, 1512.0, 889.0));
        // a second display above the primary one: negative top in AX space
        assert_eq!(flip_to_top_left(pt(0.0, 982.0, 1920.0, 1080.0), ph), pt(0.0, -1080.0, 1920.0, 1080.0));
        // and one to the left, bottom-aligned
        assert_eq!(flip_to_top_left(pt(-1920.0, 0.0, 1920.0, 1080.0), ph), pt(-1920.0, -98.0, 1920.0, 1080.0));
        // flipping twice is the identity
        let x = pt(12.0, 34.0, 56.0, 78.0);
        assert_eq!(flip_to_top_left(flip_to_top_left(x, ph), ph), x);
    }

    #[test]
    fn every_screen_converts_against_the_primary_height() {
        let cocoa = vec![
            (pt(0.0, 0.0, 1512.0, 982.0), pt(0.0, 60.0, 1512.0, 889.0)),
            // external display on the right, taller, with its own menu bar (24 pt) and no Dock
            (pt(1512.0, -200.0, 2560.0, 1440.0), pt(1512.0, -200.0, 2560.0, 1416.0)),
        ];
        let s = screens_top_left(&cocoa);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].visible, pt(0.0, 33.0, 1512.0, 889.0));
        // top edge: 982 - (-200 + 1440) = -258; the visible part starts under its own menu bar
        assert_eq!(s[1].frame, pt(1512.0, -258.0, 2560.0, 1440.0));
        assert_eq!(s[1].visible, pt(1512.0, -234.0, 2560.0, 1416.0));
        assert!(screens_top_left(&[]).is_empty());
    }

    #[test]
    fn the_screen_under_a_point_is_found_and_edges_belong_to_one_screen() {
        let s = screens_top_left(&[
            (pt(0.0, 0.0, 1512.0, 982.0), pt(0.0, 60.0, 1512.0, 889.0)),
            (pt(1512.0, 0.0, 1920.0, 1080.0), pt(1512.0, 0.0, 1920.0, 1055.0)),
        ]);
        assert_eq!(screen_at(&s, 700.0, 400.0), Some(0));
        assert_eq!(screen_at(&s, 2000.0, 100.0), Some(1));
        assert_eq!(screen_at(&s, 1512.0, 100.0), Some(1), "the shared edge is the right screen's left edge");
        assert_eq!(screen_at(&s, 0.0, 0.0), Some(0));
        assert_eq!(screen_at(&s, -5.0, 100.0), None);
        assert_eq!(screen_at(&s, 700.0, 5000.0), None);
        assert_eq!(screen_at(&[], 1.0, 1.0), None);
    }

    #[test]
    fn inner_rect_rounds_inwards_and_rejects_nonsense() {
        assert_eq!(inner_rect(pt(0.0, 33.0, 1512.0, 889.0)), Some(r(0, 33, 1512, 922)));
        // fractional edges move inwards, never under the menu bar or the Dock
        assert_eq!(inner_rect(pt(0.5, 24.5, 100.0, 100.0)), Some(r(1, 25, 100, 124)));
        // floating-point noise from the conversion is not a fraction
        assert_eq!(inner_rect(pt(-0.000001, 24.999999, 1440.000001, 875.0)), Some(r(0, 25, 1440, 900)));
        assert_eq!(inner_rect(pt(0.0, 0.0, 0.0, 100.0)), None);
        assert_eq!(inner_rect(pt(0.2, 0.0, 0.5, 100.0)), None, "nothing whole is left");
        assert_eq!(inner_rect(pt(f64::NAN, 0.0, 10.0, 10.0)), None);
        assert_eq!(inner_rect(pt(0.0, 0.0, f64::INFINITY, 10.0)), None);
        assert_eq!(inner_rect(pt(1e12, 0.0, 10.0, 10.0)), None, "does not fit an i32");
        // negative origins (displays left of / above the primary)
        assert_eq!(inner_rect(pt(-1920.0, -258.0, 1920.0, 1080.0)), Some(r(-1920, -258, 0, 822)));
    }

    #[test]
    fn rounded_rect_takes_the_nearest_edges_and_round_trips_through_points() {
        assert_eq!(rounded_rect(pt(0.4, 24.6, 100.2, 100.0)), Some(r(0, 25, 101, 125)));
        let col = r(504, 33, 1008, 922);
        assert_eq!(rounded_rect(pt_rect(col)), Some(col));
        assert_eq!(inner_rect(pt_rect(col)), Some(col));
        assert_eq!(pt_rect(col), pt(504.0, 33.0, 504.0, 889.0));
    }

    #[test]
    fn the_columns_of_a_mac_visible_frame_fill_it() {
        let work = inner_rect(pt(0.0, 33.0, 1512.0, 889.0)).unwrap();
        for n in 1..=6 {
            check_tiling(work, n);
        }
    }

    #[test]
    fn only_ordinary_visible_apps_other_than_nexus_are_considered() {
        let own = 4242;
        assert!(mac_app_considered(true, false, 100, own));
        assert!(!mac_app_considered(false, false, 100, own), "menu-bar extras and background agents");
        assert!(!mac_app_considered(true, true, 100, own), "hidden with Cmd-H");
        assert!(!mac_app_considered(true, false, own, own), "never NEXUS itself");
        assert!(!mac_app_considered(true, false, 0, own));
    }

    #[test]
    fn only_standard_on_screen_windows_are_tiled() {
        assert!(mac_window_considered("AXStandardWindow", false, false));
        assert!(!mac_window_considered("AXStandardWindow", true, false), "minimised into the Dock");
        assert!(!mac_window_considered("AXStandardWindow", false, true), "full screen owns its own Space");
        for sub in ["AXDialog", "AXSystemDialog", "AXFloatingWindow", "AXSystemFloatingWindow", "AXUnknown", ""] {
            assert!(!mac_window_considered(sub, false, false), "{sub:?}");
        }
    }

    #[test]
    fn an_untitled_window_takes_its_apps_name_and_stays_tileable() {
        assert_eq!(mac_title("main.rs — NEXUS", "Code"), "main.rs — NEXUS");
        assert_eq!(mac_title("  ", "Antigravity"), "Antigravity");
        assert_eq!(mac_title("", ""), "");
        let w = tw(1, 10, "antigravity", "AXStandardWindow", &mac_title("", "Antigravity"));
        assert!(tileable(&w));
        assert_eq!(find_agent_window(&spec(&["antigravity"], &[]), &[w]), Some(1));
    }

    #[test]
    fn the_front_apps_windows_come_first_and_the_rest_keep_their_order() {
        let apps = vec![(1, "a"), (2, "b"), (3, "c"), (2, "b2")];
        let out = front_first(apps, |a| a.0 == 2);
        assert_eq!(out, vec![(2, "b"), (2, "b2"), (1, "a"), (3, "c")]);
        assert_eq!(front_first(vec![1, 2, 3], |_| false), vec![1, 2, 3], "no front app: unchanged");
        assert!(front_first(Vec::<u8>::new(), |_| true).is_empty());
    }

    /// Live, on this Mac: lists the screens and the tileable windows, tiles the first two into columns, checks they
    /// landed, then puts both back where they were. Moves real windows for a moment. Needs the Accessibility permission
    /// for the terminal and at least two ordinary, non-full-screen windows on the current Space:
    ///   cargo test --lib window_tile::tests::live_mac_tiles_two_windows_and_puts_them_back -- --ignored --nocapture
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "moves two real windows on this Mac and back; needs Accessibility; run by hand"]
    fn live_mac_tiles_two_windows_and_puts_them_back() {
        assert!(crate::ax_mac::trusted(), "grant Accessibility to the terminal first");
        let screens = mac::screens().expect("NSScreen answers");
        for (i, s) in screens.list.iter().enumerate() {
            println!("[PROOF] screen {i}{}: frame {:?} visible {:?}", if i == screens.main { " (main)" } else { "" }, s.frame, s.visible);
        }
        let work = RealTileEnv.work_area().expect("a work area");
        println!("[PROOF] work area {work:?}");
        let wins = list_top_windows();
        for w in &wins {
            println!("[PROOF] window {} pid {} {:?} {:?}", w.hwnd, w.pid, w.exe, w.title);
        }
        assert!(wins.len() >= 2, "open at least two ordinary windows first");
        let pick: Vec<isize> = wins.iter().take(2).map(|w| w.hwnd).collect();
        let before: Vec<Option<Rect>> = pick.iter().map(|h| mac::frame_by_handle(*h)).collect();
        let results = tile_windows(&RealTileEnv, &pick);
        let after: Vec<Option<Rect>> = pick.iter().map(|h| mac::frame_by_handle(*h)).collect();
        for (i, h) in pick.iter().enumerate() {
            if let Some(b) = before[i] {
                let _ = mac::place(*h, b);
            }
        }
        println!("[PROOF] results {results:?} frames {after:?} (restored to {before:?})");
        let cols = columns(work, 2);
        for (i, (h, res)) in results.iter().enumerate() {
            assert_eq!(*res, PlaceResult::Placed, "window {h}");
            let got = after[i].expect("frame readable");
            assert!(near(cols[i], got, MAC_FIT_TOL), "window {h}: wanted {:?}, got {got:?}", cols[i]);
        }
    }

    // ── dev: tile two Notepad windows (run by hand; blanks nothing) ──

    /// Opens two Notepad windows, tiles them side by side on the primary monitor, prints what happened and
    /// closes the two it opened. Never calls `display_off`.
    ///   cargo test --lib --features custom-protocol,admin-brain -- window_tile::tests::dev_tile_two_notepads --ignored --nocapture
    #[test]
    #[ignore = "opens real Notepad windows; run by hand"]
    #[cfg(target_os = "windows")]
    fn dev_tile_two_notepads() {
        let known: Vec<isize> = list_top_windows().iter().filter(|w| w.exe == "notepad.exe").map(|w| w.hwnd).collect();
        for _ in 0..2 {
            std::process::Command::new("notepad.exe").spawn().expect("start notepad");
            std::thread::sleep(std::time::Duration::from_millis(700));
        }
        let mut mine: Vec<isize> = Vec::new();
        for _ in 0..20 {
            mine = list_top_windows()
                .iter()
                .filter(|w| w.exe == "notepad.exe" && !known.contains(&w.hwnd))
                .map(|w| w.hwnd)
                .collect();
            if mine.len() >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        println!("new notepad windows: {mine:?}");
        if mine.len() < 2 {
            println!("fewer than two new Notepad windows (new-tab behaviour?); nothing to tile");
        } else {
            let results = tile_windows(&RealTileEnv, &mine[..2]);
            for (h, res) in &results {
                println!("{h}: {res:?}");
            }
            std::thread::sleep(std::time::Duration::from_secs(6));
        }
        {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
            for h in &mine {
                // SAFETY: posting WM_CLOSE to a window we opened.
                unsafe {
                    let _ = PostMessageW(HWND(*h), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
    }

    // ── live proofs on real windows (run by hand) ──

    #[cfg(target_os = "windows")]
    struct OwnWindowsGuard {
        thread_id: u32,
        join_handle: Option<std::thread::JoinHandle<()>>,
        hwnds: Vec<isize>,
    }

    #[cfg(target_os = "windows")]
    impl Drop for OwnWindowsGuard {
        fn drop(&mut self) {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, PostThreadMessageW, WM_QUIT};

            unsafe {
                for _ in 0..10 {
                    if PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)).as_bool() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }

            if let Some(join) = self.join_handle.take() {
                let _ = join.join();
            }

            for &h in &self.hwnds {
                unsafe {
                    let _ = DestroyWindow(HWND(h));
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn spawn_test_windows(pid: u32) -> OwnWindowsGuard {
        use std::sync::mpsc;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::System::Threading::GetCurrentThreadId;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, RegisterClassW,
            ShowWindow, TranslateMessage, CW_USEDEFAULT, MSG, SW_SHOWNOACTIVATE,
            WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        };

        unsafe extern "system" fn test_wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        let (tx, rx) = mpsc::channel();

        let join_handle = std::thread::spawn(move || unsafe {
            let thread_id = GetCurrentThreadId();
            let class_name: Vec<u16> = format!("NXTILE_CLASS_{pid}\0").encode_utf16().collect();
            let hinst = GetModuleHandleW(PCWSTR(std::ptr::null())).unwrap_or(HINSTANCE(0));
            let wc = WNDCLASSW {
                lpfnWndProc: Some(test_wnd_proc),
                hInstance: hinst,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let atom = RegisterClassW(&wc);
            assert_ne!(atom, 0, "RegisterClassW failed");

            let mut win_hwnds: Vec<HWND> = Vec::with_capacity(3);
            let mut raw_hwnds: Vec<isize> = Vec::with_capacity(3);
            for n in 1..=3 {
                let title: Vec<u16> = format!("NXTILE_{pid}_{n}\0").encode_utf16().collect();
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(class_name.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    HWND(0),
                    None,
                    hinst,
                    std::ptr::null(),
                );
                assert_ne!(hwnd.0, 0, "CreateWindowExW failed for window {n}");
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                win_hwnds.push(hwnd);
                raw_hwnds.push(hwnd.0);
            }

            let _ = tx.send((thread_id, raw_hwnds));

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND(0), 0, 0).0 > 0 {
                if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            for h in win_hwnds {
                let _ = DestroyWindow(h);
            }
            let _ = windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(
                PCWSTR(class_name.as_ptr()),
                hinst,
            );
        });

        let (thread_id, hwnds) = rx.recv().expect("receive test window handles");
        OwnWindowsGuard {
            thread_id,
            join_handle: Some(join_handle),
            hwnds,
        }
    }

    #[cfg(target_os = "windows")]
    fn visible_frame(hwnd: isize) -> Option<Rect> {
        use std::ffi::c_void;
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
        let h = HWND(hwnd);
        unsafe {
            let mut fb = RECT::default();
            if DwmGetWindowAttribute(
                h,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut fb as *mut RECT as *mut c_void,
                std::mem::size_of::<RECT>() as u32,
            )
            .is_ok()
            {
                Some(to_rect(fb))
            } else {
                let mut wr = RECT::default();
                if GetWindowRect(h, &mut wr).as_bool() {
                    Some(to_rect(wr))
                } else {
                    None
                }
            }
        }
    }

    /// Creates three visible top-level windows owned by the test process, tiles them into equal columns,
    /// asserts visible frames are ordered left-to-right, widths and heights match the work area, and
    /// windows do not overlap, then closes all three.
    ///
    /// Run by hand:
    ///   cargo test --lib --features custom-protocol,admin-brain -- window_tile::tests::live_three_own_windows_are_tiled_into_equal_columns --ignored --nocapture
    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "opens three real test windows; run by hand"]
    fn live_three_own_windows_are_tiled_into_equal_columns() {
        let pid = std::process::id();
        let marker = format!("NXTILE_{pid}_");
        let _guard = spawn_test_windows(pid);

        // The windows belong to this process, and `list_top_windows` deliberately reports our own process as
        // "nexus.exe", which is never tileable, so the handles come straight from the helper thread.
        let hwnds: Vec<isize> = _guard.hwnds.clone();
        assert_eq!(hwnds.len(), 3, "found {} of 3 own windows with marker {marker}", hwnds.len());
        for &h in &hwnds {
            assert!(RealTileEnv.exists(h), "window {h} should exist");
        }

        let results = tile_windows(&RealTileEnv, &hwnds);
        assert_eq!(results.len(), 3, "expected 3 placement results");
        for (h, res) in &results {
            assert_eq!(*res, PlaceResult::Placed, "window {h} placement was not Placed: {res:?}");
        }

        let mut frames: Vec<Rect> = Vec::new();
        for &h in &hwnds {
            let f = visible_frame(h).expect("failed to get visible frame for tiled window");
            println!("[PROOF] tiled hwnd={h} rect=({},{},{},{})", f.left, f.top, f.right, f.bottom);
            frames.push(f);
        }

        // The three visible frames are left to right in the order given
        assert!(
            frames[0].left < frames[1].left && frames[1].left < frames[2].left,
            "visible frames must be ordered left to right: {:?}",
            frames
        );

        // Their widths are equal within 2 px
        assert!(
            (frames[0].width() - frames[1].width()).abs() <= 2,
            "widths of frame 0 and 1 must be equal within 2 px: {} vs {}",
            frames[0].width(),
            frames[1].width()
        );
        assert!(
            (frames[1].width() - frames[2].width()).abs() <= 2,
            "widths of frame 1 and 2 must be equal within 2 px: {} vs {}",
            frames[1].width(),
            frames[2].width()
        );
        assert!(
            (frames[0].width() - frames[2].width()).abs() <= 2,
            "widths of frame 0 and 2 must be equal within 2 px: {} vs {}",
            frames[0].width(),
            frames[2].width()
        );

        // Their heights equal the work-area height within 2 px
        let work = RealTileEnv.work_area().expect("primary monitor work area must exist");
        let work_h = work.height();
        for (i, f) in frames.iter().enumerate() {
            assert!(
                (f.height() - work_h).abs() <= 2,
                "frame {i} height must equal work area height ({work_h}) within 2 px, got {}",
                f.height()
            );
        }

        // They do not overlap by more than 2 px
        assert!(
            frames[0].right - frames[1].left <= 2,
            "frame 0 and 1 must not overlap by more than 2 px: right {} vs next left {}",
            frames[0].right,
            frames[1].left
        );
        assert!(
            frames[1].right - frames[2].left <= 2,
            "frame 1 and 2 must not overlap by more than 2 px: right {} vs next left {}",
            frames[1].right,
            frames[2].left
        );

        println!("[PROOF] tiled 3 own windows into equal columns");
    }

    /// Turns the physical display off via Win32 SC_MONITORPOWER for ~3 seconds,
    /// then injects a relative mouse move (+1 px, -1 px) via SendInput to wake the display.
    ///
    /// WARNING: When run by hand, this briefly blanks the screen. The mouse move restores it.
    ///
    /// Run by hand:
    ///   cargo test --lib --features custom-protocol,admin-brain -- window_tile::tests::live_display_off_then_input_wakes_it --ignored --nocapture
    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "turns the real display off for about 3 seconds; run by hand"]
    fn live_display_off_then_input_wakes_it() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
        };

        if may_blank_display(Some(120), false).is_err() {
            println!("[PROOF] SKIPPED: may_blank_display returned error for 120s idle");
            return;
        }

        let first = display_off();
        assert!(first.is_ok(), "display_off first call failed: {first:?}");

        // A second call right after is also Ok (idempotent)
        let second = display_off();
        assert!(second.is_ok(), "display_off second call (idempotent) failed: {second:?}");

        std::thread::sleep(std::time::Duration::from_secs(3));

        let inputs = [
            INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 1,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
            INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: -1,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            },
        ];
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        assert_eq!(sent, 2, "SendInput must deliver mouse wake events");

        println!("[PROOF] display_off returned Ok");
    }
}
