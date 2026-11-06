//! The unattended part of "watch this session, and if it ends perfectly, shut down my laptop"
//! (doc 142, phase W5). It ties the held plan (`power_plan`), what can be seen of the agent
//! (`session_observer`, the agent's own Stop hook) and the decision (`session_verdict`) to a countdown
//! that can power the laptop off.
//!
//! The decisions are a PURE state machine (`Engine`, injected clock, no I/O) so every path, above all every
//! way of NOT shutting down, is tested. The loop around it (`run_loop`) only gathers observations and
//! carries out the engine's actions.
//!
//! What it takes to power off (all of it, at once, every time):
//!   1. the `watchPower` setting is `on` (default `off`; `dry` = practice, says it but never does it);
//!   2. a plan is held, and it is not "just tell me";
//!   3. the session ENDED (two independent signals, nothing still busy) and the verdict is Go for 15 s in a row;
//!   4. a 30 s countdown, announced aloud, then runs, and during it ANY of these stops it:
//!      "cancel" / "stop" / "wait" ..., a key or mouse movement, a meeting starting, the plan cancelled, the
//!      setting turned off, or the verdict no longer being Go. Not knowing whether the user is back also stops it;
//!   5. the normal shutdown sequence (goodbye, graceful close of apps, a last 3 s cancel window) does the rest.

use crate::agent_tracker::{final_summary, progress_line, Outcome};
use crate::power_cmd::PowerKind;
use crate::power_plan::WatchPowerMode;
use crate::session_verdict::{spoken_hold, spoken_reasons, Verdict};

/// How often the loop looks.
pub const TICK_MS: u64 = 3_000;
/// The verdict must stay the same this long before anything is concluded (a flicker never counts).
pub const SETTLE_MS: u64 = 15_000;
/// The announced, cancellable wait before the power call.
pub const COUNTDOWN_MS: u64 = 30_000;
/// How long a plan is watched when it names no limit.
pub const DEFAULT_MAX_WAIT_MS: u64 = 3 * 60 * 60 * 1000;
/// No plan waits longer than this, whatever was said.
pub const MAX_WAIT_CEILING_MS: u64 = 12 * 60 * 60 * 1000;
/// Input this soon after the countdown began is the user's last action before it, not a reaction to it.
pub const INPUT_SLACK_MS: u64 = 1_500;
/// A stop that has been the agent's latest event this long without the watch concluding is said out loud once.
pub const STALL_NOTICE_MS: u64 = 150_000;

#[derive(Debug, Clone, PartialEq)]
pub struct PlanView {
    pub kind: PowerKind,
    pub notify_only: bool,
    /// "track all my agents": tell me as each finishes and when all are done; no power action at all
    pub track_only: bool,
    pub deadline_ms: Option<u64>,
    /// "Claude Code" / "Codex" / "this session"
    pub who: String,
}

#[derive(Debug, Clone)]
pub struct Obs {
    pub now_ms: u64,
    pub mode: WatchPowerMode,
    /// `None` = the plan was cancelled or replaced.
    pub plan: Option<PlanView>,
    /// `session_verdict::has_ended`: two independent signals and nothing still busy.
    pub ended: bool,
    pub verdict: Verdict,
    /// Milliseconds since the last keyboard / mouse input; `None` = cannot tell.
    pub user_idle_ms: Option<u64>,
    /// The user said cancel (or began another power command) since the countdown started.
    pub voice_cancelled: bool,
    pub meeting: bool,
    /// How long ago the agent's latest stop arrived, while it is still the latest (`None` = no stop to speak of).
    pub stop_age_ms: Option<u64>,
    /// The model overrode a failure word in the report; said together with the good news so the user knows.
    pub note: Option<String>,
    /// The plan said "shut down after X" with no condition: it powers off however the agents ended.
    pub unconditional: bool,
    /// Agents that have just settled as finished (each told once).
    pub finished_now: Vec<Outcome>,
    /// Labels of the tracked agents still working.
    pub working: Vec<String>,
    /// Every tracked agent that has ended, with how.
    pub outcomes: Vec<Outcome>,
    /// The plan follows nobody yet (all agents quiet, or the named ones never started).
    pub nothing_running: bool,
    /// A commit / pull request step is still running: nothing is concluded until it is done.
    pub followup_busy: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Tell the user (through the proactive-speech policy: it waits for a breakpoint).
    Say(String),
    /// Tell the user and keep a strict "yes" ready for "shut down anyway?".
    SayAndAsk(String, PowerKind),
    /// Start the cancellable countdown (and announce it). `lead` = what is said before "I'm shutting down in 30 seconds".
    BeginCountdown { kind: PowerKind, lead: String },
    /// The countdown ran out with nothing stopping it: power off now.
    Fire(PowerKind),
    /// All tracked agents are done and the user wanted them tracked: arrange their windows in equal columns and, if
    /// nobody is at the keyboard, turn the display off so everything is ready on return.
    TidyUp,
    /// The watch is over. Nothing more happens for this plan.
    End(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Watching { concluding: Option<(bool, u64)> },
    Countdown { start_ms: u64 },
    Finished,
}

#[derive(Debug)]
pub struct Engine {
    started_ms: u64,
    phase: Phase,
    /// The "it stopped but I can't tell it is finished" notice was already given for this plan.
    stall_told: bool,
    /// "I don't see any agent working" was already said.
    nothing_told: bool,
}

fn gerund(kind: PowerKind) -> &'static str {
    match kind {
        PowerKind::Shutdown => "shutting down your laptop",
        PowerKind::Restart => "restarting your laptop",
        PowerKind::Sleep => "putting your laptop to sleep",
        PowerKind::Lock => "locking your screen",
        PowerKind::SignOut => "signing you out",
    }
}

fn human_duration(ms: u64) -> String {
    let mins = ms / 60_000;
    if mins >= 120 {
        format!("{} hours", mins / 60)
    } else if mins >= 60 {
        "an hour".to_string()
    } else {
        format!("{} minute{}", mins.max(1), if mins <= 1 { "" } else { "s" })
    }
}

impl Engine {
    pub fn new(now_ms: u64) -> Engine {
        Engine { started_ms: now_ms, phase: Phase::Watching { concluding: None }, stall_told: false, nothing_told: false }
    }

    pub fn finished(&self) -> bool {
        self.phase == Phase::Finished
    }

    fn end(&mut self, reason: &'static str) -> Vec<Action> {
        self.phase = Phase::Finished;
        vec![Action::End(reason)]
    }

    fn abort(&mut self, why: &str, reason: &'static str) -> Vec<Action> {
        self.phase = Phase::Finished;
        vec![Action::Say(format!("Stopped, sir: {why}. I didn't touch your laptop.")), Action::End(reason)]
    }

    /// A countdown is running right now.
    pub fn counting_down(&self) -> bool {
        matches!(self.phase, Phase::Countdown { .. })
    }

    pub fn step(&mut self, o: &Obs) -> Vec<Action> {
        match self.phase.clone() {
            Phase::Finished => Vec::new(),
            Phase::Countdown { start_ms } => self.step_countdown(o, start_ms),
            Phase::Watching { concluding } => self.step_watching(o, concluding),
        }
    }

    fn step_watching(&mut self, o: &Obs, concluding: Option<(bool, u64)>) -> Vec<Action> {
        let Some(plan) = &o.plan else { return self.end("plan gone") };
        if o.mode == WatchPowerMode::Off && !plan.notify_only && !plan.track_only {
            return self.end("unattended shutdown is off");
        }
        let limit = plan.deadline_ms.unwrap_or(DEFAULT_MAX_WAIT_MS).min(MAX_WAIT_CEILING_MS);
        if o.now_ms.saturating_sub(self.started_ms) > limit {
            let tail = if plan.track_only { "" } else { ", so I left your laptop on" };
            let said = format!("I stopped waiting for {} after {}, sir{tail}.", plan.who, human_duration(limit));
            let mut out = vec![Action::Say(said)];
            out.extend(self.end("waited long enough"));
            return out;
        }
        if o.followup_busy {
            // the commit / pull request step is still running: the countdown does not start before it is done
            self.phase = Phase::Watching { concluding: None };
            return Vec::new();
        }
        if !o.ended {
            // still working (or not enough evidence that it stopped): nothing is concluded, however long it takes
            self.phase = Phase::Watching { concluding: None };
            let mut out = Vec::new();
            // agents that finished while others work are told one by one
            if !o.finished_now.is_empty() && !o.working.is_empty() {
                out.push(Action::Say(progress_line(&o.finished_now, &o.working)));
            }
            if o.nothing_running && !self.nothing_told {
                self.nothing_told = true;
                out.push(Action::Say("I don't see any of your agents working yet, sir. I'll keep watching until you cancel.".to_string()));
            }
            // ...but a stop that stays unexplained is never met with silence: say what is missing, once
            if !self.stall_told && o.stop_age_ms.map_or(false, |a| a >= STALL_NOTICE_MS) {
                self.stall_told = true;
                let why = spoken_reasons(&o.verdict);
                let why = if why.is_empty() { "I cannot confirm it".to_string() } else { why };
                out.push(Action::Say(format!(
                    "{} stopped a while ago, sir, but I can't tell that it is really finished: {why}. I'm still watching, and I haven't touched your laptop.",
                    plan.who
                )));
            }
            return out;
        }
        // "shut down after X" with no condition: the result does not matter, only that everything ended
        let is_go = o.verdict == Verdict::Go || (o.unconditional && !plan.track_only);
        let since = match concluding {
            Some((was_go, since)) if was_go == is_go => since,
            _ => o.now_ms,
        };
        self.phase = Phase::Watching { concluding: Some((is_go, since)) };
        if o.now_ms.saturating_sub(since) < SETTLE_MS {
            return Vec::new();
        }

        // "track all my agents": the last one is done; sum it all up and finish
        if plan.track_only {
            let mut said = final_summary(&o.outcomes);
            said.push_str(&note_clause(&o.note));
            let mut out = vec![Action::Say(said), Action::TidyUp];
            out.extend(self.end("tracked"));
            return out;
        }

        if !is_go {
            // Finished, but not perfect (or not sure). Tell the user; only a real "on" setting offers to go ahead.
            let ask = o.mode == WatchPowerMode::On && !plan.notify_only;
            let said = if o.outcomes.len() > 1 {
                let mut s = final_summary(&o.outcomes);
                s.push_str(" I left your laptop on, sir.");
                if ask {
                    s.push_str(" Shall I shut down anyway?");
                }
                s
            } else {
                spoken_hold(&plan.who, &o.verdict, ask)
            };
            let mut out = vec![if ask { Action::SayAndAsk(said, plan.kind) } else { Action::Say(said) }];
            out.extend(self.end("not perfect"));
            return out;
        }

        // Finished (and perfect, or the user said however it ends)
        let simple = o.outcomes.len() <= 1 && o.verdict == Verdict::Go;
        if plan.notify_only {
            let said = if simple {
                format!(
                    "{} finished and it looks perfect, sir.{} You asked me only to tell you, so I haven't touched your laptop.",
                    plan.who,
                    note_clause(&o.note)
                )
            } else {
                format!("{}{} You asked me only to tell you, so I haven't touched your laptop.", final_summary(&o.outcomes), note_clause(&o.note))
            };
            let mut out = vec![Action::Say(said)];
            out.extend(self.end("notify only"));
            return out;
        }
        match o.mode {
            WatchPowerMode::Off => self.end("unattended shutdown is off"),
            WatchPowerMode::Dry => {
                let said = if simple {
                    format!(
                        "Practice run, sir: {} finished and it looks perfect.{} I would have started {} after a 30 second countdown. Nothing was changed.",
                        plan.who,
                        note_clause(&o.note),
                        gerund(plan.kind)
                    )
                } else {
                    format!(
                        "Practice run, sir: {}{} I would have started {} after a 30 second countdown, as you asked. Nothing was changed.",
                        final_summary(&o.outcomes),
                        note_clause(&o.note),
                        gerund(plan.kind)
                    )
                };
                let mut out = vec![Action::Say(said)];
                out.extend(self.end("dry run"));
                out
            }
            WatchPowerMode::On => {
                self.phase = Phase::Countdown { start_ms: o.now_ms };
                let lead = if simple {
                    format!("{} finished and it looks perfect, sir.{}", plan.who, note_clause(&o.note))
                } else if o.verdict == Verdict::Go {
                    format!("{}{}", final_summary(&o.outcomes), note_clause(&o.note))
                } else {
                    format!("{} You asked me to shut down either way.", final_summary(&o.outcomes))
                };
                vec![Action::BeginCountdown { kind: plan.kind, lead }]
            }
        }
    }

    fn step_countdown(&mut self, o: &Obs, start_ms: u64) -> Vec<Action> {
        let Some(plan) = &o.plan else { return self.abort("the plan was cancelled", "cancelled: plan") };
        if o.mode != WatchPowerMode::On {
            return self.abort("unattended shutdown was switched off", "cancelled: setting");
        }
        if o.voice_cancelled {
            return self.abort("you said cancel", "cancelled: voice");
        }
        if o.meeting {
            return self.abort("a meeting started", "cancelled: meeting");
        }
        // the user's "however it ends" survives a change of verdict; new work on any agent does not
        if !o.ended || !(o.unconditional || o.verdict == Verdict::Go) {
            return self.abort("something changed on the session", "cancelled: session changed");
        }
        let elapsed = o.now_ms.saturating_sub(start_ms);
        match o.user_idle_ms {
            None => return self.abort("I can't tell whether you're back at the keyboard", "cancelled: input unknown"),
            Some(idle) if idle + INPUT_SLACK_MS < elapsed => {
                return self.abort("you're back at the keyboard", "cancelled: input");
            }
            Some(_) => {}
        }
        if elapsed >= COUNTDOWN_MS {
            self.phase = Phase::Finished;
            return vec![Action::Fire(plan.kind)];
        }
        Vec::new()
    }
}

fn note_clause(note: &Option<String>) -> String {
    note.as_ref().map(|n| format!(" {n}")).unwrap_or_default()
}

/// The announcement spoken the moment the countdown begins; `lead` says how the agents ended.
pub fn announcement_with(kind: PowerKind, lead: &str) -> String {
    format!("{lead} I'm {} in 30 seconds. Say cancel, or touch the keyboard, to stop me.", gerund(kind))
}

/// The announcement for a single agent that ended perfectly.
#[cfg(test)]
pub fn announcement(kind: PowerKind, who: &str, note: Option<&str>) -> String {
    announcement_with(kind, &format!("{who} finished and it looks perfect, sir.{}", note.map(|n| format!(" {n}")).unwrap_or_default()))
}

// ─── The loop around the engine ──────────────────────────────────────

use crate::agent_hooks::{self, Agent, EventKind};
use crate::agent_tracker::{aggregate, Announcer, JoinMode, Joiner, TrackSnapshot, NOTHING_RUNNING_MS, PENDING_DROP_MS};
use crate::power_plan;
use crate::session_observer::{find_roots, tree_snaps, Inputs, Observer, ProcSampler, ProcSnap, CPU_ACTIVE_PCT};
use crate::session_verdict::{cleared_failure_note, has_ended, judge, model_system_prompt, parse_model_json, wants_model_check, ModelVerdict, Reason};
use std::sync::atomic::{AtomicBool, Ordering};
use std::collections::BTreeMap;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

static RUNNING: AtomicBool = AtomicBool::new(false);
/// Commit / pull request steps in flight. The shutdown countdown waits until this is zero.
static FOLLOWUPS_RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// What those steps said, for the overnight record.
static FOLLOWUP_LINES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
/// How the followed agents ended as of the latest look: saved when the laptop is about to be powered off, so the
/// next morning's briefing can say what happened.
static LAST_OUTCOMES: std::sync::Mutex<Vec<Outcome>> = std::sync::Mutex::new(Vec::new());
/// The followed agents and the process each one runs in, as of the latest look: where the finish sequence finds
/// their windows.
static LAST_TARGETS: std::sync::Mutex<Vec<(Agent, Option<u32>)>> = std::sync::Mutex::new(Vec::new());

/// The `watchPower` setting, read fresh every time so "turn off unattended shutdown" works at once.
/// Anything missing or unreadable is `off`.
pub fn read_mode<R: Runtime>(app: &AppHandle<R>) -> WatchPowerMode {
    let Ok(dir) = app.path().app_data_dir() else { return WatchPowerMode::Off };
    std::fs::read_to_string(dir.join("settings.json"))
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .and_then(|j| j.get("watchPower").and_then(|v| v.as_str()).map(WatchPowerMode::from_setting))
        .unwrap_or(WatchPowerMode::Off)
}

/// Milliseconds since the last keyboard / mouse input (None if it cannot be read: then nothing is powered off).
#[cfg(target_os = "windows")]
fn user_idle_ms() -> Option<u64> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: `info` is a valid, correctly sized LASTINPUTINFO.
    let ok = unsafe { GetLastInputInfo(&mut info) };
    if !ok.as_bool() {
        return None;
    }
    let now = unsafe { GetTickCount() };
    Some(now.wrapping_sub(info.dwTime) as u64)
}

#[cfg(target_os = "macos")]
fn user_idle_ms() -> Option<u64> {
    crate::memcore::resume::mac_idle_seconds().map(|s| (s * 1000.0) as u64)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn user_idle_ms() -> Option<u64> {
    None
}

fn meeting_active<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<std::sync::Arc<crate::meeting_detect::MeetingState>>()
        .map(|m| m.should_suppress_tts())
        .unwrap_or(false)
}

fn is_hooked(agent: Agent) -> bool {
    agent_hooks::config_path(agent).map_or(false, |c| agent_hooks::is_installed(agent, &c))
}

/// Can this agent be followed at all: its hook is installed, or it has none and can only be observed.
fn followable(agent: Agent) -> bool {
    agent.observe_only() || is_hooked(agent)
}

/// Is there anything for the loop to do right now? A plan about at least one agent that can be followed (a hook, or
/// observation for the ones with none), and either "just tell me" / "track" or a setting that is not `off`.
fn wants_running<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(p) = power_plan::current() else { return false };
    let targets = p.plan.all || p.plan.named_agents().iter().filter_map(|n| Agent::from_name(n)).any(followable);
    targets && (p.notify_only || p.plan.track_only || read_mode(app) != WatchPowerMode::Off)
}

/// Start watching the held plan (once). Safe to call whenever the plan or the setting changes.
pub fn ensure_running<R: Runtime>(app: AppHandle<R>) {
    if !wants_running(&app) || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        run_loop(app.clone()).await;
        RUNNING.store(false, Ordering::SeqCst);
        // a plan made while the loop was winding down must not be left unwatched
        if wants_running(&app) {
            ensure_running(app);
        }
    });
}

/// Ask a free cloud model for a second opinion on the agent's final message (masked, 4 KB at most, logged in
/// the Memory page's "sent to cloud" list). It can only make the verdict stricter. Anything wrong = no opinion.
async fn ask_model<R: Runtime>(app: &AppHandle<R>, tail: &str) -> ModelVerdict {
    if tail.trim().is_empty() {
        return ModelVerdict::Unavailable;
    }
    let Ok(dir) = app.path().app_data_dir() else { return ModelVerdict::Unavailable };
    if crate::memcore::names::enabled(&dir) {
        return ModelVerdict::Unavailable; // name hiding is on: nothing about the session leaves the device
    }
    crate::memcore::log_egress(&dir, "watchverdict", tail, "");
    let keys = crate::router::read_provider_keys(app);
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(10)).build() else { return ModelVerdict::Unavailable };
    match tokio::time::timeout(Duration::from_secs(12), crate::router::route_with_system(model_system_prompt(), tail, 120, &keys, &client)).await {
        Ok(Some(reply)) => parse_model_json(&reply.text).unwrap_or(ModelVerdict::Unavailable),
        _ => ModelVerdict::Unavailable,
    }
}

/// What the loop keeps for one followed agent.
struct Track {
    agent: Agent,
    observer: Observer,
    model_cache: Option<(u64, ModelVerdict)>,
    last_logged: Option<String>,
}

/// One agent, one look: its evidence, verdict and the details the log line prints.
struct Look {
    snapshot: TrackSnapshot,
    log: String,
    state_key: String,
    /// The process the agent runs in (the first root found for it).
    root_pid: Option<u32>,
}

async fn look_at<R: Runtime>(app: &AppHandle<R>, t: &mut Track, snaps: &[ProcSnap], now_ms: u64, wall_ms: u64, created_ms: u64, require_clean: bool) -> Look {
    let agent = t.agent;
    let roots = find_roots(Some(agent.name()), snaps, None, std::process::id());
    let tree = tree_snaps(&roots, snaps);
    let tree_cpu: f32 = tree.iter().map(|p| p.cpu_pct).sum();
    let hook_tail = agent_hooks::last_stop_tail(agent);
    let stop_info = agent_hooks::last_stop_info(agent);
    let mut ev = t.observer.step(Inputs {
        now_ms,
        tree: &tree,
        window_text: None,
        hook_stopped: agent_hooks::stop_is_latest(agent, created_ms),
        hook_tail: hook_tail.as_deref(),
        hook_needs_user: agent_hooks::needs_user(agent, created_ms),
        hook_failed: stop_info.ok == Some(false),
        hook_not_idle: stop_info.fully_idle == Some(false),
        hook_quiet_ms: agent_hooks::hook_quiet_ms_at(agent, created_ms, wall_ms),
        ignore_process_activity: agent.is_ide(),
    });

    let (ended, verdict, note) = if agent.observe_only() {
        // no hook: only "went quiet" can be seen, never "finished and perfect"
        let quiet = ev.signals.contains(&crate::session_verdict::Signal::ProcessIdle) && !ev.still_busy;
        (quiet, if quiet { Verdict::Unsure(vec![Reason::NoHook]) } else { Verdict::Unsure(Vec::new()) }, None)
    } else {
        let mut verdict = judge(&ev, require_clean);
        if wants_model_check(has_ended(&ev), &verdict) {
            let key = agent_hooks::last_event_ms(agent, EventKind::Stop).unwrap_or(0);
            if t.model_cache.as_ref().map(|(k, _)| *k) != Some(key) {
                let m = ask_model(app, &ev.tail).await;
                println!("[WATCH-POWER] {} model cross-check: {m:?}", agent.name());
                t.model_cache = Some((key, m));
            }
            if let Some((_, m)) = &t.model_cache {
                ev.model = m.clone();
                verdict = judge(&ev, require_clean);
            }
        }
        let note = if verdict == Verdict::Go { cleared_failure_note(&ev, require_clean) } else { None };
        (has_ended(&ev), verdict, note)
    };

    let stop_age = agent_hooks::stop_age_ms_at(agent, created_ms, wall_ms);
    let state_key = format!("{verdict:?}|{:?}|{}|{ended}|{note:?}", ev.signals, ev.still_busy);
    let log = format!(
        "agent={} ended={ended} verdict={verdict:?} signals={:?} busy={} roots={} stop_age_s={} tail_len={} stop_ok={:?} fully_idle={:?} tree_cpu={tree_cpu:.1} cleared_note={note:?}",
        agent.name(),
        ev.signals,
        ev.still_busy,
        roots.len(),
        stop_age.map_or("-".to_string(), |a| (a / 1000).to_string()),
        ev.tail.len(),
        stop_info.ok,
        stop_info.fully_idle,
    );
    Look {
        snapshot: TrackSnapshot { name: agent.name().to_string(), label: agent.label().to_string(), joined: true, ended, verdict, note },
        log,
        state_key,
        root_pid: roots.first().copied(),
    }
}

/// Does this agent show signs of doing something? A report from its hook since the plan began, or (for agents that are
/// not IDEs, whose background processes never rest) a busy process tree.
fn shows_activity(agent: Agent, snaps: &[ProcSnap], created_ms: u64) -> bool {
    if agent_hooks::any_event_since(agent, created_ms) {
        return true;
    }
    if agent.is_ide() && !agent.observe_only() {
        return false;
    }
    let roots = find_roots(Some(agent.name()), snaps, None, std::process::id());
    let cpu: f32 = tree_snaps(&roots, snaps).iter().map(|p| p.cpu_pct).sum();
    cpu > CPU_ACTIVE_PCT
}

async fn run_loop<R: Runtime>(app: AppHandle<R>) {
    let t0 = std::time::Instant::now();
    let mut sampler = Some(ProcSampler::new());
    let mut engine: Option<(u64, Engine)> = None;
    let mut tracks: BTreeMap<&'static str, Track> = BTreeMap::new();
    let mut joiner = Joiner::new(JoinMode::All, 0);
    let mut announcer = Announcer::default();
    let mut awake: Option<crate::keep_awake::KeepAwakeLease> = None;
    let mut all_candidates: Vec<Agent> = Vec::new();
    let mut countdown_gen: Option<u64> = None;
    let mut ticks: u64 = 0;
    let mut last_agg_logged: Option<String> = None;
    let mut followed_up: std::collections::BTreeSet<&'static str> = std::collections::BTreeSet::new();
    println!("[WATCH-POWER] watching");

    'watch: loop {
        tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
        ticks += 1;
        let now_ms = t0.elapsed().as_millis() as u64;
        let counting = engine.as_ref().map_or(false, |(_, e)| e.counting_down());
        let stored = power_plan::current();
        let mode = read_mode(&app);

        let Some(stored) = stored else {
            if counting {
                // the plan was cancelled mid-countdown: let the engine say so and stop
                let acts = engine.as_mut().map(|(_, e)| e.step(&Obs { plan: None, ..idle_obs(now_ms, mode) })).unwrap_or_default();
                carry_out(&app, acts, &mut countdown_gen).await;
            }
            break 'watch;
        };
        if mode == WatchPowerMode::Off && !stored.notify_only && !stored.plan.track_only && !counting {
            println!("[WATCH-POWER] unattended shutdown is off: nothing to do");
            break 'watch;
        }
        let named: Vec<Agent> = stored.plan.named_agents().iter().filter_map(|n| Agent::from_name(n)).collect();
        if named.is_empty() && !stored.plan.all {
            println!("[WATCH-POWER] no hookable agent for this plan");
            break 'watch;
        }

        // a new plan replaces the old one: start over
        if engine.as_ref().map(|(g, _)| *g) != Some(stored.generation) {
            if let Some(g) = countdown_gen.take() {
                crate::power_cmd::finish(g);
            }
            engine = Some((stored.generation, Engine::new(now_ms)));
            tracks.clear();
            followed_up.clear();
            if let Ok(mut g) = FOLLOWUP_LINES.lock() {
                g.clear();
            }
            announcer = Announcer::default();
            awake = None;
            last_agg_logged = None;
            let join_mode = if stored.plan.all {
                JoinMode::All
            } else if named.len() >= 2 {
                JoinMode::ExplicitMulti
            } else {
                JoinMode::Single
            };
            joiner = Joiner::new(join_mode, now_ms);
            all_candidates.clear();
        }

        // ── look ──
        let snaps = match sampler.take() {
            Some(mut s) => match tauri::async_runtime::spawn_blocking(move || {
                let v = s.sample();
                (s, v)
            })
            .await
            {
                Ok((s, v)) => {
                    sampler = Some(s);
                    v
                }
                Err(_) => {
                    sampler = Some(ProcSampler::new());
                    Vec::new()
                }
            },
            None => {
                sampler = Some(ProcSampler::new());
                Vec::new()
            }
        };
        let wall_ms = agent_hooks::now_ms();

        // who is part of this plan: the named ones, or (for "all") every agent on the machine that can be followed
        if stored.plan.all && (all_candidates.is_empty() || ticks % 10 == 1) {
            all_candidates = agent_hooks::agents_present().into_iter().filter(|a| followable(*a)).collect();
        }
        let candidates: Vec<Agent> = if stored.plan.all { all_candidates.clone() } else { named.clone() };
        let cands: Vec<(String, bool)> =
            candidates.iter().map(|a| (a.name().to_string(), shows_activity(*a, &snaps, stored.created_ms))).collect();
        let update = joiner.step(now_ms, &cands);
        for name in &update.joined {
            if let Some(agent) = Agent::from_name(name) {
                println!("[WATCH-POWER] following {}", agent.label());
                tracks.insert(agent.name(), Track { agent, observer: Observer::default(), model_cache: None, last_logged: None });
            }
        }
        if !update.dropped.is_empty() {
            let labels: Vec<String> = update.dropped.iter().filter_map(|n| Agent::from_name(n)).map(|a| a.label().to_string()).collect();
            let said = format!("{} doesn't seem to be running, sir, so I'm not waiting for {}.", labels.join(" and "), if labels.len() == 1 { "it" } else { "them" });
            carry_out(&app, vec![Action::Say(said)], &mut countdown_gen).await;
        }

        let require_clean = stored.plan.require_clean;
        let mut snapshots: Vec<TrackSnapshot> = Vec::new();
        let mut targets: Vec<(Agent, Option<u32>)> = Vec::new();
        let names: Vec<&'static str> = tracks.keys().copied().collect();
        for name in names {
            let Some(t) = tracks.get_mut(name) else { continue };
            let look = look_at(&app, t, &snaps, now_ms, wall_ms, stored.created_ms, require_clean).await;
            if ticks % 10 == 1 || t.last_logged.as_deref() != Some(look.state_key.as_str()) {
                println!("[WATCH-POWER] tick {ticks}: mode={} {}", mode.as_str(), look.log);
                t.last_logged = Some(look.state_key.clone());
            }
            targets.push((t.agent, look.root_pid));
            snapshots.push(look.snapshot);
        }
        if !targets.is_empty() {
            if let Ok(mut g) = LAST_TARGETS.lock() {
                *g = targets;
            }
        }

        // ── decide ──
        let agg = aggregate(&snapshots);
        if let Ok(mut g) = LAST_OUTCOMES.lock() {
            *g = agg.outcomes.clone();
        }
        let finished_now = announcer.step(now_ms, &snapshots);
        // while the user is away every finish is also kept for the moment they are back
        if !finished_now.is_empty() && user_idle_ms().map_or(false, |i| i >= crate::agent_attention::AWAY_MS) {
            if let Ok(dir) = app.path().app_data_dir() {
                for o in &finished_now {
                    crate::agent_attention::remember(&dir, &o.label, &crate::agent_tracker::outcome_line(o));
                }
            }
        }
        // "... and commit and raise a PR": per finished agent, only after the user said yes, once per plan
        if stored.plan.followup && stored.followup_ok {
            for o in &finished_now {
                let Some(t) = tracks.values().find(|t| t.agent.label() == o.label) else { continue };
                if followed_up.insert(t.agent.name()) {
                    spawn_followup(&app, t.agent, o.verdict.clone());
                }
            }
        }
        // the screen stays awake exactly while some followed agent is still working
        if agg.working.is_empty() {
            awake = None;
        } else {
            if awake.is_none() {
                awake = crate::keep_awake::acquire_pinned(crate::keep_awake::MAX_WATCH_HOLD);
            }
            crate::keep_awake::heartbeat();
        }
        let nothing_running = joiner.idle()
            && match (stored.plan.all, named.len()) {
                (true, _) => joiner.age_ms(now_ms) >= NOTHING_RUNNING_MS,
                (false, n) if n >= 2 => joiner.age_ms(now_ms) >= PENDING_DROP_MS,
                _ => false,
            };
        let stop_age_ms = if tracks.len() == 1 {
            tracks.values().next().and_then(|t| agent_hooks::stop_age_ms_at(t.agent, stored.created_ms, wall_ms))
        } else {
            None
        };
        let note = if agg.verdict == Verdict::Go { agg.note.clone() } else { None };
        let view = PlanView {
            kind: stored.plan.kind,
            notify_only: stored.notify_only,
            track_only: stored.plan.track_only,
            deadline_ms: stored.plan.deadline_mins.map(|m| u64::from(m) * 60_000),
            who: if tracks.len() == 1 && !stored.plan.all {
                tracks.values().next().map_or_else(|| crate::watch_power::subject(&stored.plan), |t| t.agent.label().to_string())
            } else {
                crate::watch_power::subject(&stored.plan)
            },
        };
        let obs = Obs {
            now_ms,
            mode,
            plan: Some(view),
            ended: agg.ended,
            verdict: agg.verdict.clone(),
            user_idle_ms: user_idle_ms(),
            voice_cancelled: countdown_gen.map_or(false, |g| !crate::power_cmd::is_current(g)),
            meeting: meeting_active(&app),
            stop_age_ms,
            note,
            unconditional: !stored.plan.require_clean,
            finished_now,
            working: agg.working.clone(),
            outcomes: agg.outcomes.clone(),
            nothing_running,
            followup_busy: FOLLOWUPS_RUNNING.load(Ordering::SeqCst) > 0,
        };
        let agg_key = format!("{}|{}|{:?}|{:?}", agg.tracked, agg.ended, agg.verdict, agg.working);
        if tracks.len() > 1 && last_agg_logged.as_deref() != Some(agg_key.as_str()) {
            println!("[WATCH-POWER] all: tracked={} ended={} working={:?} verdict={:?}", agg.tracked, agg.ended, agg.working, agg.verdict);
            last_agg_logged = Some(agg_key);
        }
        let Some((_, eng)) = engine.as_mut() else { break 'watch };
        let acts = eng.step(&obs);
        let finished = eng.finished();
        let done = carry_out(&app, acts, &mut countdown_gen).await;
        if done || finished {
            break 'watch;
        }
    }
    if let Some(g) = countdown_gen.take() {
        crate::power_cmd::finish(g);
    }
    drop(awake);
    println!("[WATCH-POWER] stopped watching");
}

fn idle_obs(now_ms: u64, mode: WatchPowerMode) -> Obs {
    Obs {
        now_ms,
        mode,
        plan: None,
        ended: false,
        verdict: Verdict::Unsure(Vec::new()),
        user_idle_ms: user_idle_ms(),
        voice_cancelled: false,
        meeting: false,
        stop_age_ms: None,
        note: None,
        unconditional: false,
        finished_now: Vec::new(),
        working: Vec::new(),
        outcomes: Vec::new(),
        nothing_running: false,
        followup_busy: false,
    }
}

/// The commit-and-pull-request step for one finished agent, off the watch loop: git and the network can be slow, and the
/// shutdown countdown waits for it (`followup_busy`). It runs only on a clean verdict (the module's own gate) and says
/// plainly why it did not.
fn spawn_followup<R: Runtime>(app: &AppHandle<R>, agent: Agent, verdict: Verdict) {
    use crate::agent_followup::{default_stamp, run_followup, FollowUp, RealGit};
    let app = app.clone();
    FOLLOWUPS_RUNNING.fetch_add(1, Ordering::SeqCst);
    tauri::async_runtime::spawn(async move {
        let said = match agent_hooks::last_cwd(agent) {
            None => format!("I can't tell which folder {} worked in, so I didn't commit anything.", agent.label()),
            Some(dir) => {
                let hint = agent_hooks::last_stop_tail(agent);
                let step = FollowUp::CommitAndPr {
                    dir: std::path::PathBuf::from(dir),
                    agent: agent.label().to_string(),
                    stamp: default_stamp(),
                    message_hint: hint,
                    base: "main".to_string(),
                };
                let (worker_url, user_id) = crate::network::get_session_info().map(|(w, u, _)| (w, u)).unwrap_or_default();
                let report = run_followup(std::sync::Arc::new(RealGit::default()), step, verdict, &worker_url, &user_id).await;
                println!("[WATCH-POWER] follow-up for {}: committed={} pushed={} pr_opened={} skipped={}", agent.name(), report.committed, report.pushed, report.pr_opened, report.skipped);
                report.spoken
            }
        };
        if let Ok(mut g) = FOLLOWUP_LINES.lock() {
            g.push(said.clone());
        }
        crate::proactive_policy::submit(&app, format!("watchpower_followup_{}_{}", agent.name(), agent_hooks::now_ms()), said, crate::google::types::AlertUrgency::High);
        FOLLOWUPS_RUNNING.fetch_sub(1, Ordering::SeqCst);
    });
}

/// How to recognise an IDE agent's window by name (terminal agents are found through their process instead).
fn window_spec(agent: Agent) -> Option<crate::window_tile::AgentSpec> {
    let words: &[&str] = match agent {
        Agent::Antigravity => &["antigravity"],
        Agent::Cursor => &["cursor"],
        Agent::Windsurf => &["windsurf"],
        Agent::Trae => &["trae"],
        _ => return None,
    };
    let w: Vec<String> = words.iter().map(|s| s.to_string()).collect();
    Some(crate::window_tile::AgentSpec { exe_words: w.clone(), title_words: w })
}

/// The finish sequence: wait until NEXUS has finished speaking, put the followed agents' windows in equal columns, say
/// what could not be done, and turn the display off if the user is away. Every step is best effort and never
/// touches anything but window positions and the display.
async fn tidy_up<R: Runtime>(app: AppHandle<R>) {
    use crate::window_tile::{display_off, list_top_windows, may_blank_display, resolve_agents, tile_windows, AgentTarget, PlaceResult, RealTileEnv};
    // let the summary be spoken first (the policy may hold it back for a breakpoint; at most 40 s)
    tokio::time::sleep(Duration::from_millis(1500)).await;
    for _ in 0..40 {
        if !crate::orchestrator::has_active_request() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    let followed: Vec<(Agent, Option<u32>)> = LAST_TARGETS.lock().map(|g| g.clone()).unwrap_or_default();
    let targets: Vec<AgentTarget> =
        followed.iter().map(|(a, pid)| AgentTarget { name: a.label().to_string(), pid: *pid, spec: window_spec(*a) }).collect();
    let (resolution, procs) = tauri::async_runtime::spawn_blocking(move || {
        let procs = ProcSampler::new().sample();
        let wins = list_top_windows();
        (resolve_agents(&targets, &procs, &wins), procs)
    })
    .await
    .unwrap_or_default();
    drop(procs);

    let mut notes: Vec<String> = Vec::new();
    for tile in &resolution.tiles {
        if let Some(line) = tile.shared_line() {
            notes.push(line);
        }
    }
    if !resolution.missing.is_empty() {
        notes.push(format!("I couldn't find a window for {}.", resolution.missing.join(" and ")));
    }
    let hwnds: Vec<isize> = resolution.tiles.iter().map(|t| t.hwnd).collect();
    if !hwnds.is_empty() {
        let results = tauri::async_runtime::spawn_blocking(move || tile_windows(&RealTileEnv, &hwnds)).await.unwrap_or_default();
        let placed = results.iter().filter(|(_, r)| *r == PlaceResult::Placed).count();
        println!("[WATCH-POWER] tidy-up: {placed} of {} windows placed", results.len());
        for (_, r) in &results {
            match r {
                PlaceResult::SkippedElevated => notes.push("One window runs as administrator, so I left it where it was.".to_string()),
                PlaceResult::Failed(why) => notes.push(format!("One window wouldn't move: {why}.")),
                _ => {}
            }
        }
    }
    if !notes.is_empty() {
        notes.dedup();
        crate::proactive_policy::submit_event(&app, format!("watchpower_tidy_{}", agent_hooks::now_ms()), notes.join(" "), crate::google::types::AlertUrgency::Medium, Some("watchpower:tidy".into()));
    }

    // the screen goes off only when nobody is using the laptop
    tokio::time::sleep(Duration::from_millis(1500)).await;
    match may_blank_display(crate::memcore::resume::user_idle_secs(), meeting_active(&app)) {
        Ok(()) => match display_off() {
            Ok(()) => println!("[WATCH-POWER] tidy-up: display off"),
            Err(e) => println!("[WATCH-POWER] tidy-up: display not turned off: {e}"),
        },
        Err(why) => println!("[WATCH-POWER] tidy-up: display left on ({why:?})"),
    }
}

/// Do what the engine decided. Returns true when the watch is over.
async fn carry_out<R: Runtime>(app: &AppHandle<R>, actions: Vec<Action>, countdown_gen: &mut Option<u64>) -> bool {
    let mut over = false;
    for action in actions {
        match action {
            Action::Say(text) => {
                println!("[WATCH-POWER] say: {text}");
                crate::proactive_policy::submit(app, format!("watchpower_{}", agent_hooks::now_ms()), text, crate::google::types::AlertUrgency::High);
            }
            Action::SayAndAsk(text, kind) => {
                println!("[WATCH-POWER] say+ask: {text}");
                // the answer window opens only once the question is actually being spoken (`sentinel_offer_` ids)
                crate::memcore::offer::set(crate::memcore::offer::Offer::Power { kind: kind.as_str().to_string() }, false);
                crate::proactive_policy::submit(
                    app,
                    format!("sentinel_offer_watchpower_{}", agent_hooks::now_ms()),
                    text,
                    crate::google::types::AlertUrgency::High,
                );
            }
            Action::BeginCountdown { kind, lead } => {
                // the same cancel generation the spoken "cancel" listens to (see `power_cmd::cancel`)
                *countdown_gen = Some(crate::power_cmd::begin());
                println!("[WATCH-POWER] countdown started: {} in 30 s", kind.as_str());
                crate::orchestrator::speak_unprompted(app, announcement_with(kind, &lead));
            }
            Action::Fire(kind) => {
                // last look, right before the call: the setting and the plan must both still say yes
                let power_allowed = app.path().app_data_dir().map_or(false, |d| crate::memcore::flag(&d, "powerCommands", true));
                let still_on = power_allowed && read_mode(app) == WatchPowerMode::On && power_plan::current().map_or(false, |p| !p.notify_only);
                if let Some(g) = countdown_gen.take() {
                    crate::power_cmd::finish(g);
                }
                if still_on {
                    println!("[WATCH-POWER] countdown finished: starting {}", kind.as_str());
                    // remember how the agents ended: told on the next boot, even if the laptop is gone by then
                    let outcomes = LAST_OUTCOMES.lock().map(|g| g.clone()).unwrap_or_default();
                    if let (Ok(dir), Some(mut rec)) = (app.path().app_data_dir(), crate::agent_outcomes::record_from(&outcomes, kind.as_str(), chrono::Utc::now().timestamp())) {
                        // what the commit / pull request step said (or why it did not run) belongs to the same story
                        if let Ok(g) = FOLLOWUP_LINES.lock() {
                            rec.lines.extend(g.iter().map(|l| l.trim_end_matches('.').to_string()));
                        }
                        crate::agent_outcomes::save(&dir, &rec);
                    }
                    let line = crate::orchestrator::start_power(app, kind).await;
                    crate::orchestrator::speak_unprompted(app, line);
                } else {
                    println!("[WATCH-POWER] last check failed: not powering off");
                }
                power_plan::clear();
                over = true;
            }
            Action::TidyUp => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move { tidy_up(app).await });
            }
            Action::End(reason) => {
                println!("[WATCH-POWER] end: {reason}");
                if let Some(g) = countdown_gen.take() {
                    crate::power_cmd::finish(g);
                }
                power_plan::clear();
                over = true;
            }
        }
    }
    over
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_verdict::Reason;

    fn plan(kind: PowerKind) -> PlanView {
        PlanView { kind, notify_only: false, track_only: false, deadline_ms: None, who: "Claude Code".into() }
    }

    fn obs(now_ms: u64, mode: WatchPowerMode) -> Obs {
        Obs {
            now_ms,
            mode,
            plan: Some(plan(PowerKind::Shutdown)),
            ended: true,
            verdict: Verdict::Go,
            user_idle_ms: Some(10 * 60_000),
            voice_cancelled: false,
            meeting: false,
            stop_age_ms: None,
            note: None,
            unconditional: false,
            finished_now: Vec::new(),
            working: Vec::new(),
            outcomes: vec![Outcome { label: "Claude Code".into(), verdict: Verdict::Go }],
            nothing_running: false,
            followup_busy: false,
        }
    }

    /// Run the engine over `[from, to]` in ticks; return (time, action) pairs.
    fn run(engine: &mut Engine, from: u64, to: u64, make: impl Fn(u64) -> Obs) -> Vec<(u64, Action)> {
        let mut out = Vec::new();
        let mut t = from;
        while t <= to {
            for a in engine.step(&make(t)) {
                out.push((t, a));
            }
            t += TICK_MS;
        }
        out
    }

    fn fired(actions: &[(u64, Action)]) -> bool {
        actions.iter().any(|(_, a)| matches!(a, Action::Fire(_)))
    }

    fn began(actions: &[(u64, Action)]) -> Option<u64> {
        actions.iter().find(|(_, a)| matches!(a, Action::BeginCountdown { .. })).map(|(t, _)| *t)
    }

    // ── the one path that powers off ──

    #[test]
    fn a_perfect_finish_with_the_setting_on_counts_down_and_then_fires_exactly_once() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 120_000, |t| obs(t, WatchPowerMode::On));
        let begin = began(&acts).expect("a countdown began");
        assert!(begin >= SETTLE_MS, "the verdict had to hold for {SETTLE_MS} ms first, began at {begin}");
        let fire = acts.iter().find(|(_, a)| matches!(a, Action::Fire(_))).expect("it fired").0;
        assert!(fire - begin >= COUNTDOWN_MS, "fired {} ms after the announcement", fire - begin);
        assert_eq!(acts.iter().filter(|(_, a)| matches!(a, Action::Fire(_))).count(), 1, "exactly once");
        assert!(e.finished());
        // and nothing more, ever
        assert!(e.step(&obs(500_000, WatchPowerMode::On)).is_empty());
    }

    #[test]
    fn the_announced_action_follows_the_plan() {
        for (kind, word) in [
            (PowerKind::Shutdown, "shutting down"),
            (PowerKind::Restart, "restarting"),
            (PowerKind::Sleep, "putting your laptop to sleep"),
            (PowerKind::Lock, "locking"),
            (PowerKind::SignOut, "signing you out"),
        ] {
            assert!(announcement(kind, "Claude Code", None).contains(word), "{kind:?}");
            assert!(announcement(kind, "Claude Code", None).contains("30 seconds") && announcement(kind, "Claude Code", None).contains("cancel"));
        }
    }

    // ── the ways it must NOT power off ──

    #[test]
    fn with_the_setting_off_it_never_fires_or_counts_down_and_stops_quietly() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 600_000, |t| obs(t, WatchPowerMode::Off));
        assert!(!fired(&acts) && began(&acts).is_none());
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::End(_))));
    }

    #[test]
    fn a_practice_run_says_what_it_would_do_and_never_does_it() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 600_000, |t| obs(t, WatchPowerMode::Dry));
        assert!(!fired(&acts) && began(&acts).is_none());
        let said: Vec<&Action> = acts.iter().map(|(_, a)| a).filter(|a| matches!(a, Action::Say(_))).collect();
        assert_eq!(said.len(), 1);
        assert!(matches!(said[0], Action::Say(t) if t.contains("Practice run") && t.contains("Nothing was changed")));
        assert!(e.finished());
    }

    #[test]
    fn just_tell_me_never_powers_off_even_with_the_setting_on() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 600_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.plan.as_mut().unwrap().notify_only = true;
            o
        });
        assert!(!fired(&acts) && began(&acts).is_none());
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::Say(t) if t.contains("only to tell you"))));
        // and it still tells the user when the setting is off, because that is what was asked
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 600_000, |t| {
            let mut o = obs(t, WatchPowerMode::Off);
            o.plan.as_mut().unwrap().notify_only = true;
            o
        });
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::Say(_))) && !fired(&acts));
    }

    #[test]
    fn a_session_that_has_not_ended_is_waited_for_and_never_concluded() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 2 * 60 * 60_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.ended = false;
            o
        });
        assert!(acts.is_empty(), "{acts:?}");
        assert!(!e.finished());
    }

    #[test]
    fn a_flicker_of_go_is_not_enough_the_verdict_must_hold() {
        let mut e = Engine::new(0);
        // Go for 12 s, then not ended for a tick, then Go again: the 15 s clock restarts each time
        let acts = run(&mut e, 0, 600_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.ended = t % 15_000 != 12_000 || t == 0;
            o
        });
        assert!(began(&acts).is_none() || began(&acts).unwrap() > 0);
        // with an interruption every 12 s it never reaches 15 s of Go in a row
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 600_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.ended = t % 12_000 != 9_000;
            o
        });
        assert!(began(&acts).is_none() && !fired(&acts), "{acts:?}");
    }

    #[test]
    fn not_perfect_tells_the_user_and_only_a_real_on_setting_asks_to_go_ahead() {
        let hold = Verdict::Hold(vec![Reason::OpenQuestion("Run the tests?".into())]);
        for (mode, notify, asks) in [
            (WatchPowerMode::On, false, true),
            (WatchPowerMode::On, true, false),
            (WatchPowerMode::Dry, false, false),
            (WatchPowerMode::Off, true, false),
        ] {
            let mut e = Engine::new(0);
            let acts = run(&mut e, 0, 120_000, |t| {
                let mut o = obs(t, mode);
                o.verdict = hold.clone();
                o.plan.as_mut().unwrap().notify_only = notify;
                o
            });
            assert!(!fired(&acts) && began(&acts).is_none(), "{mode:?}");
            let ask = acts.iter().any(|(_, a)| matches!(a, Action::SayAndAsk(..)));
            assert_eq!(ask, asks, "{mode:?} notify={notify}");
            let said = acts.iter().find_map(|(_, a)| match a {
                Action::Say(t) | Action::SayAndAsk(t, _) => Some(t.clone()),
                _ => None,
            });
            let said = said.expect("it tells the user");
            assert!(said.contains("not perfect") && said.contains("left your laptop on"), "{said}");
            assert_eq!(said.contains("Shall I shut down anyway"), asks, "{said}");
            assert!(e.finished());
        }
    }

    #[test]
    fn it_gives_up_after_the_time_limit_and_never_powers_off_on_a_timeout() {
        for (limit, check_at) in [(None, DEFAULT_MAX_WAIT_MS + 10_000), (Some(30 * 60_000), 31 * 60_000), (Some(99 * 60 * 60_000), MAX_WAIT_CEILING_MS + 10_000)] {
            let mut e = Engine::new(0);
            let acts = run(&mut e, 0, check_at, |t| {
                let mut o = obs(t, WatchPowerMode::On);
                o.ended = false;
                o.plan.as_mut().unwrap().deadline_ms = limit;
                o
            });
            assert!(!fired(&acts) && began(&acts).is_none());
            assert!(acts.iter().any(|(_, a)| matches!(a, Action::Say(t) if t.contains("stopped waiting") && t.contains("left your laptop on"))), "{limit:?}");
            assert!(e.finished());
        }
    }

    #[test]
    fn a_cancelled_plan_ends_the_watch_in_silence() {
        let mut e = Engine::new(0);
        let acts = e.step(&Obs { plan: None, ..obs(3_000, WatchPowerMode::On) });
        assert_eq!(acts, vec![Action::End("plan gone")]);
    }

    // ── every way to stop the countdown ──

    /// Begin a countdown at `begin_at`, then apply `change` to every observation after it.
    fn countdown_with(change: impl Fn(&mut Obs)) -> (Vec<(u64, Action)>, u64) {
        let mut e = Engine::new(0);
        let mut begun: Option<u64> = None;
        let mut t = 0;
        while t < 60_000 && begun.is_none() {
            if e.step(&obs(t, WatchPowerMode::On)).iter().any(|a| matches!(a, Action::BeginCountdown { .. })) {
                begun = Some(t);
            }
            t += TICK_MS;
        }
        let begin_at = begun.expect("the countdown should have begun");
        let acts = run(&mut e, begin_at + TICK_MS, begin_at + 90_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            if t >= begin_at + 9_000 {
                change(&mut o);
            }
            o
        });
        (acts, begin_at)
    }

    fn assert_stopped(acts: &[(u64, Action)], why: &str) {
        assert!(!fired(acts), "{why}: it still fired");
        let end = acts.iter().find_map(|(_, a)| match a {
            Action::End(r) => Some(*r),
            _ => None,
        });
        assert!(end.map_or(false, |r| r.starts_with("cancelled")), "{why}: ended with {end:?}");
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::Say(t) if t.contains("didn't touch your laptop"))), "{why}: it did not say so");
    }

    #[test]
    fn saying_cancel_stops_the_countdown() {
        let (acts, _) = countdown_with(|o| o.voice_cancelled = true);
        assert_stopped(&acts, "voice");
    }

    #[test]
    fn touching_the_keyboard_or_mouse_stops_the_countdown() {
        // input 4 s before the check = after the countdown began
        let (acts, _) = countdown_with(|o| o.user_idle_ms = Some(4_000));
        assert_stopped(&acts, "input");
        // not knowing whether the user is back also stops it: it never guesses
        let (acts, _) = countdown_with(|o| o.user_idle_ms = None);
        assert_stopped(&acts, "idle unknown");
    }

    #[test]
    fn the_last_input_before_the_countdown_does_not_stop_it() {
        let mut e = Engine::new(0);
        let mut began_at = None;
        // the user typed at t=14 s (just before the verdict settled): their idle time grows with the clock
        let acts = run(&mut e, 0, 200_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.user_idle_ms = Some(t.saturating_sub(14_000));
            o
        });
        if let Some(b) = began(&acts) {
            began_at = Some(b);
        }
        assert!(began_at.is_some() && fired(&acts), "{acts:?}");
    }

    #[test]
    fn a_meeting_starting_stops_the_countdown() {
        let (acts, _) = countdown_with(|o| o.meeting = true);
        assert_stopped(&acts, "meeting");
    }

    #[test]
    fn cancelling_the_plan_stops_the_countdown() {
        let (acts, _) = countdown_with(|o| o.plan = None);
        assert_stopped(&acts, "plan cancelled");
    }

    #[test]
    fn switching_the_setting_off_or_to_practice_stops_the_countdown() {
        let (acts, _) = countdown_with(|o| o.mode = WatchPowerMode::Off);
        assert_stopped(&acts, "setting off");
        let (acts, _) = countdown_with(|o| o.mode = WatchPowerMode::Dry);
        assert_stopped(&acts, "setting dry");
    }

    #[test]
    fn the_session_becoming_busy_or_imperfect_again_stops_the_countdown() {
        let (acts, _) = countdown_with(|o| o.ended = false);
        assert_stopped(&acts, "busy again");
        let (acts, _) = countdown_with(|o| o.verdict = Verdict::Unsure(vec![Reason::LowConfidence]));
        assert_stopped(&acts, "unsure");
        let (acts, _) = countdown_with(|o| o.verdict = Verdict::Hold(vec![Reason::WaitingForYou]));
        assert_stopped(&acts, "hold");
    }

    #[test]
    fn a_stop_in_the_last_tick_still_wins_over_the_fire() {
        let mut e = Engine::new(0);
        let mut begun: Option<u64> = None;
        let mut t = 0;
        while t < 120_000 && begun.is_none() {
            if e.step(&obs(t, WatchPowerMode::On)).iter().any(|a| matches!(a, Action::BeginCountdown { .. })) {
                begun = Some(t);
            }
            t += TICK_MS;
        }
        let begin_at = begun.expect("the countdown should have begun");
        // right at the end of the countdown the user says cancel
        let mut o = obs(begin_at + COUNTDOWN_MS, WatchPowerMode::On);
        o.voice_cancelled = true;
        let acts = e.step(&o);
        assert!(!acts.iter().any(|a| matches!(a, Action::Fire(_))), "{acts:?}");
        assert!(e.finished());
    }

    #[test]
    fn nothing_fires_before_the_countdown_is_over() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 40_000, |t| obs(t, WatchPowerMode::On));
        let begin = began(&acts).unwrap();
        assert!(!acts.iter().any(|(t, a)| matches!(a, Action::Fire(_)) && *t < begin + COUNTDOWN_MS));
        assert!(human_duration(30 * 60_000).contains("30 minutes"));
        assert_eq!(human_duration(3 * 60 * 60_000), "3 hours");
        assert_eq!(human_duration(60_000), "1 minute");
    }

    #[test]
    fn a_verdict_that_keeps_changing_between_go_and_unsure_never_settles() {
        // the session stays "ended" but the verdict flips every 12 s: neither side lasts 15 s in a row
        for first_go in [true, false] {
            let mut e = Engine::new(0);
            let acts = run(&mut e, 0, 900_000, |t| {
                let mut o = obs(t, WatchPowerMode::On);
                let go = (t / 12_000) % 2 == 0;
                if go != first_go {
                    o.verdict = Verdict::Unsure(vec![Reason::LowConfidence]);
                }
                o
            });
            assert!(!fired(&acts) && began(&acts).is_none(), "{first_go}: {acts:?}");
            assert!(!e.finished(), "no conclusion either way");
        }
    }

    #[test]
    fn input_within_the_first_moments_of_the_countdown_is_the_last_action_before_it_but_later_input_stops_it() {
        let go = |input_after_start_ms: u64| {
            let mut e = Engine::new(0);
            let mut begun: Option<u64> = None;
            let mut out = Vec::new();
            let mut t = 0;
            while t <= 200_000 && !e.finished() {
                let mut o = obs(t, WatchPowerMode::On);
                // idle time the way the OS reports it: the last input was `input_at` (never before the start of the watch)
                if let Some(b) = begun {
                    let input_at = b + input_after_start_ms;
                    o.user_idle_ms = Some(if t >= input_at { t - input_at } else { 30 * 60_000 });
                }
                for a in e.step(&o) {
                    if matches!(a, Action::BeginCountdown { .. }) {
                        begun = Some(t);
                    }
                    out.push((t, a));
                }
                t += TICK_MS;
            }
            out
        };
        assert!(fired(&go(500)), "an input 0.5 s after the announcement began was the user's last action before it");
        assert!(!fired(&go(2_000)), "an input 2 s after the announcement began is a reaction to it");
    }

    // ── the whole chain, with the real observer and the real verdict, on a timeline ──

    use crate::session_observer::{Inputs, Observer, ProcSnap, HOOK_QUIET_NEEDED_MS};
    use crate::session_verdict::{has_ended, judge};

    fn quiet_tree() -> Vec<ProcSnap> {
        vec![
            ProcSnap { pid: 10, ppid: 1, name: "node.exe".into(), cmd: "node claude".into(), cpu_pct: 0.3, io_total: 5_000 },
            ProcSnap { pid: 11, ppid: 10, name: "node.exe".into(), cmd: "node child".into(), cpu_pct: 0.0, io_total: 1_000 },
        ]
    }

    /// Drive Observer + judge + Engine every 3 s. `stopped_at`: when the agent's Stop hook arrives (None = never);
    /// `tail`: its final message; `prompt_at`: a new prompt that starts another turn (stop no longer counts).
    fn timeline(mode: WatchPowerMode, tail: &str, stopped_at: Option<u64>, prompt_at: Option<u64>, user_input_at: Option<u64>, cancel_at: Option<u64>) -> (Vec<(u64, Action)>, Engine) {
        timeline_with(false, mode, tail, stopped_at, prompt_at, user_input_at, cancel_at)
    }

    /// `ide`: an IDE agent: its process tree is always busy and only its hooks tell when it ended.
    fn timeline_with(ide: bool, mode: WatchPowerMode, tail: &str, stopped_at: Option<u64>, prompt_at: Option<u64>, user_input_at: Option<u64>, cancel_at: Option<u64>) -> (Vec<(u64, Action)>, Engine) {
        let mut observer = Observer::default();
        let mut engine = Engine::new(0);
        let mut out = Vec::new();
        let tree = quiet_tree();
        let mut t = 0;
        while t <= 400_000 && !engine.finished() {
            let hook_stopped = stopped_at.map_or(false, |s| t >= s) && !prompt_at.map_or(false, |p| t >= p);
            let busy_tree;
            let tree_now: &[ProcSnap] = if ide || prompt_at.map_or(false, |p| t >= p && t < p + 40_000) {
                // the new turn is working: CPU
                busy_tree = vec![ProcSnap { cpu_pct: 60.0, ..tree[0].clone() }, tree[1].clone()];
                &busy_tree
            } else {
                &tree
            };
            let ev = observer.step(Inputs {
                now_ms: t,
                tree: tree_now,
                window_text: None,
                hook_stopped,
                hook_tail: if hook_stopped { Some(tail) } else { None },
                hook_needs_user: false,
                hook_failed: false,
                hook_not_idle: false,
                hook_quiet_ms: if hook_stopped { stopped_at.map(|s| t - s) } else { None },
                ignore_process_activity: ide,
            });
            let obs = Obs {
                now_ms: t,
                mode,
                plan: Some(plan(PowerKind::Shutdown)),
                ended: has_ended(&ev),
                verdict: judge(&ev, true),
                user_idle_ms: Some(user_input_at.map_or(30 * 60_000, |u| if t >= u { t - u } else { 30 * 60_000 })),
                voice_cancelled: cancel_at.map_or(false, |c| t >= c),
                meeting: false,
                stop_age_ms: if hook_stopped { stopped_at.map(|s| t - s) } else { None },
                note: None,
                unconditional: false,
                finished_now: Vec::new(),
                working: Vec::new(),
                outcomes: vec![Outcome { label: "Claude Code".into(), verdict: Verdict::Go }],
                nothing_running: false,
                followup_busy: false,
            };
            for a in engine.step(&obs) {
                out.push((t, a));
            }
            t += TICK_MS;
        }
        (out, engine)
    }

    const PERFECT: &str = "Implemented the parser and wired it in.\nAll 42 tests passed.\nDone.";

    #[test]
    fn end_to_end_a_perfect_finish_with_the_hook_installed_counts_down_and_fires() {
        let (acts, _) = timeline(WatchPowerMode::On, PERFECT, Some(20_000), None, None, None);
        let begin = began(&acts).expect("a countdown began");
        // the process had to stay quiet 45 s before the ending counted, and the verdict then had to hold 15 s
        assert!(begin >= 45_000 + SETTLE_MS, "{begin}");
        assert!(fired(&acts));
    }

    #[test]
    fn end_to_end_an_ide_agent_with_a_never_quiet_process_tree_ends_on_its_stop_and_is_told_about() {
        // Antigravity: busy tree for the whole run, Stop at 20 s, nothing after it
        let (acts, _) = timeline_with(true, WatchPowerMode::Dry, PERFECT, Some(20_000), None, None, None);
        let said = acts.iter().find_map(|(t, a)| match a {
            Action::Say(s) => Some((*t, s.clone())),
            _ => None,
        });
        let (at, text) = said.expect("it reports the finish instead of staying silent");
        assert!(text.contains("Practice run") && text.contains("looks perfect"), "{text}");
        assert!(at >= 20_000 + HOOK_QUIET_NEEDED_MS + SETTLE_MS, "reported at {at}");
        assert!(!fired(&acts));
        // with the setting on the same finish counts down and fires
        let (acts, _) = timeline_with(true, WatchPowerMode::On, PERFECT, Some(20_000), None, None, None);
        assert!(began(&acts).is_some() && fired(&acts), "{acts:?}");
    }

    #[test]
    fn end_to_end_an_ide_agent_that_asks_a_question_is_held_and_told() {
        let (acts, _) = timeline_with(true, WatchPowerMode::Dry, "Implemented it.\nWould you like me to run the tests?", Some(20_000), None, None, None);
        assert!(!fired(&acts));
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::Say(t) if t.contains("not perfect") && t.contains("question"))), "{acts:?}");
    }

    #[test]
    fn when_the_model_overrode_a_failure_word_every_good_news_message_says_so() {
        let note = Some("Its report mentions \"3 failing\", which I read as a finding, not a failure.".to_string());
        // practice run
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 120_000, |t| Obs { note: note.clone(), ..obs(t, WatchPowerMode::Dry) });
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::Say(t) if t.contains("looks perfect") && t.contains("3 failing") && t.contains("finding"))), "{acts:?}");
        // just tell me
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 120_000, |t| {
            let mut o = Obs { note: note.clone(), ..obs(t, WatchPowerMode::On) };
            o.plan.as_mut().unwrap().notify_only = true;
            o
        });
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::Say(t) if t.contains("3 failing"))), "{acts:?}");
        // the countdown announcement carries it
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 120_000, |t| Obs { note: note.clone(), ..obs(t, WatchPowerMode::On) });
        let begin = acts.iter().find_map(|(_, a)| match a {
            Action::BeginCountdown { lead, .. } => Some(lead.clone()),
            _ => None,
        });
        assert!(begin.map_or(false, |l| l.contains("3 failing") && l.contains("looks perfect")));
        assert!(announcement(PowerKind::Shutdown, "Antigravity", note.as_deref()).contains("3 failing"));
        // no override, no mention
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 120_000, |t| obs(t, WatchPowerMode::Dry));
        assert!(acts.iter().all(|(_, a)| !matches!(a, Action::Say(t) if t.contains("finding"))));
    }

    fn out(label: &str, verdict: Verdict) -> Outcome {
        Outcome { label: label.into(), verdict }
    }

    fn bad() -> Verdict {
        Verdict::Hold(vec![Reason::FailureMarker("3 failing".into(), "Fix 3 failing tests".into())])
    }

    fn says(acts: &[(u64, Action)]) -> Vec<String> {
        acts.iter()
            .filter_map(|(_, a)| match a {
                Action::Say(t) | Action::SayAndAsk(t, _) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// Three agents: Claude is done at 20 s, Codex at 100 s, Cursor at 160 s.
    fn three_agents(t: u64, mode: WatchPowerMode) -> Obs {
        let mut o = obs(t, mode);
        let finished: Vec<(&str, u64, Verdict)> = vec![("Claude Code", 20_000, Verdict::Go), ("Codex", 100_000, bad()), ("Cursor", 160_000, Verdict::Go)];
        o.outcomes = finished.iter().filter(|(_, at, _)| t >= *at).map(|(l, _, v)| out(l, v.clone())).collect();
        o.working = finished.iter().filter(|(_, at, _)| t < *at).map(|(l, _, _)| l.to_string()).collect();
        o.ended = o.working.is_empty();
        o.verdict = if o.ended { bad() } else { Verdict::Unsure(Vec::new()) };
        // each agent is told once, 15 s after it stopped
        o.finished_now = finished.iter().filter(|(_, at, _)| t >= at + 15_000 && t < at + 15_000 + TICK_MS).map(|(l, _, v)| out(l, v.clone())).collect();
        o
    }

    #[test]
    fn tracking_all_agents_tells_each_finish_then_sums_up_and_never_touches_the_power() {
        for mode in [WatchPowerMode::Off, WatchPowerMode::Dry, WatchPowerMode::On] {
            let mut e = Engine::new(0);
            let acts = run(&mut e, 0, 400_000, |t| {
                let mut o = three_agents(t, mode);
                o.plan.as_mut().unwrap().track_only = true;
                o.plan.as_mut().unwrap().who = "all your agents".into();
                o.unconditional = true;
                o
            });
            let said = says(&acts);
            // Claude and Codex are told while others still work; the last is covered by the summary
            assert!(said[0].starts_with("Claude Code finished clean.") && said[0].contains("Codex and Cursor are still working"), "{said:?}");
            assert!(said[1].contains("Codex stopped, but the output mentions a failure") && said[1].ends_with("Cursor is still working."), "{said:?}");
            assert!(said[2].starts_with("All 3 of your agents are done, sir.") && said[2].contains("Codex stopped, but"), "{said:?}");
            assert_eq!(said.len(), 3, "{said:?}");
            assert!(acts.iter().any(|(_, a)| matches!(a, Action::End("tracked"))), "{mode:?}");
            assert_eq!(acts.iter().filter(|(_, a)| matches!(a, Action::TidyUp)).count(), 1, "windows are arranged once, after the summary ({mode:?})");
            let tidy_at = acts.iter().position(|(_, a)| matches!(a, Action::TidyUp)).unwrap();
            let summary_at = acts.iter().position(|(_, a)| matches!(a, Action::Say(t) if t.starts_with("All 3"))).unwrap();
            assert!(summary_at < tidy_at);
            assert!(!fired(&acts) && began(&acts).is_none(), "tracking never counts down or powers off ({mode:?})");
            assert!(e.finished());
        }
    }

    #[test]
    fn however_it_ends_means_the_countdown_runs_even_after_a_failure_and_says_so() {
        // On: a failed agent does not stop the shutdown the user asked for
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 200_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.unconditional = true;
            o.verdict = bad();
            o.outcomes = vec![out("Antigravity", bad())];
            o
        });
        let lead = acts.iter().find_map(|(_, a)| match a {
            Action::BeginCountdown { lead, .. } => Some(lead.clone()),
            _ => None,
        });
        let lead = lead.expect("it counts down");
        assert!(lead.contains("Antigravity stopped, but the output mentions a failure") && lead.contains("You asked me to shut down either way."), "{lead}");
        assert!(fired(&acts), "and it fires after the 30 s");
        // the same result with a condition ("only if it's perfect") holds instead
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 200_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.verdict = bad();
            o.outcomes = vec![out("Antigravity", bad())];
            o
        });
        assert!(!fired(&acts) && began(&acts).is_none());
        // practice run says it would have done it anyway
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 200_000, |t| {
            let mut o = obs(t, WatchPowerMode::Dry);
            o.unconditional = true;
            o.verdict = bad();
            o.outcomes = vec![out("Antigravity", bad())];
            o
        });
        let s = says(&acts);
        assert!(s[0].starts_with("Practice run, sir: Antigravity stopped, but") && s[0].contains("as you asked") && s[0].contains("Nothing was changed"), "{s:?}");
        // the countdown survives the verdict changing, but not new work on an agent
        let mut e = Engine::new(0);
        let mut t = 0;
        let mut began_at: Option<u64> = None;
        while t < 300_000 && !e.finished() {
            let mut o = obs(t, WatchPowerMode::On);
            o.unconditional = true;
            o.verdict = bad();
            o.outcomes = vec![out("Antigravity", bad())];
            if let Some(b) = began_at {
                if t >= b + 9_000 {
                    o.verdict = Verdict::Unsure(vec![Reason::NothingToJudge]); // flips, still ended
                }
            }
            if e.step(&o).iter().any(|a| matches!(a, Action::BeginCountdown { .. })) {
                began_at = Some(t);
            }
            t += TICK_MS;
        }
        assert!(e.finished(), "it ran to the end despite the verdict changing");
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 200_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.unconditional = true;
            o.verdict = bad();
            o.outcomes = vec![out("Antigravity", bad())];
            o.ended = t < 40_000; // a new turn starts after the countdown began
            o
        });
        assert!(!fired(&acts), "new work on an agent cancels it");
    }

    #[test]
    fn several_agents_held_by_a_condition_are_summed_up_and_a_yes_can_still_shut_down() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 120_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.verdict = bad();
            o.outcomes = vec![out("Claude Code", Verdict::Go), out("Codex", bad())];
            o
        });
        let s = says(&acts);
        assert!(s[0].starts_with("All 2 of your agents are done, sir.") && s[0].contains("I left your laptop on, sir.") && s[0].ends_with("Shall I shut down anyway?"), "{s:?}");
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::SayAndAsk(..))) && !fired(&acts));
        assert!(!acts.iter().any(|(_, a)| matches!(a, Action::TidyUp)), "a plan that powers off does not rearrange windows");
    }

    #[test]
    fn the_countdown_waits_for_the_commit_step_and_starts_after_it() {
        // while git runs nothing is concluded, however long it takes
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 5 * 60_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.followup_busy = true;
            o
        });
        assert!(!fired(&acts));
        assert!(!acts.iter().any(|(_, a)| matches!(a, Action::BeginCountdown { .. })), "no countdown while git is running");
        // once it is done the normal rules apply
        let acts = run(&mut e, 5 * 60_000 + TICK_MS, 8 * 60_000, |t| obs(t, WatchPowerMode::On));
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::BeginCountdown { .. })), "{acts:?}");
    }

    #[test]
    fn a_plan_that_follows_nobody_says_so_once_and_a_tracking_plan_gives_up_without_a_power_line() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 120_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.ended = false;
            o.outcomes = Vec::new();
            o.nothing_running = true;
            o
        });
        let s = says(&acts);
        assert_eq!(s.len(), 1);
        assert!(s[0].contains("don't see any of your agents working yet"), "{s:?}");
        // a tracking plan that waited out its limit does not talk about the laptop
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 31 * 60_000, |t| {
            let mut o = obs(t, WatchPowerMode::Off);
            o.ended = false;
            let p = o.plan.as_mut().unwrap();
            p.track_only = true;
            p.deadline_ms = Some(30 * 60_000);
            p.who = "all your agents".into();
            o
        });
        let s = says(&acts);
        assert!(s.iter().any(|t| t.contains("stopped waiting for all your agents") && !t.contains("laptop")), "{s:?}");
    }

    #[test]
    fn a_stop_that_stays_unexplained_is_said_out_loud_exactly_once() {
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 600_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.ended = false;
            o.verdict = Verdict::Unsure(vec![Reason::StillBusy, Reason::NotEnoughSignals]);
            o.stop_age_ms = if t >= 10_000 { Some(t - 10_000) } else { None };
            o
        });
        let notices: Vec<&(u64, Action)> = acts.iter().filter(|(_, a)| matches!(a, Action::Say(_))).collect();
        assert_eq!(notices.len(), 1, "{acts:?}");
        assert!(notices[0].0 >= 10_000 + STALL_NOTICE_MS && notices[0].0 < 10_000 + STALL_NOTICE_MS + 2 * TICK_MS);
        assert!(matches!(&notices[0].1, Action::Say(t) if t.contains("stopped a while ago") && t.contains("something still looks busy") && t.contains("haven't touched")));
        assert!(!e.finished() && !fired(&acts), "it keeps watching and never acts on a doubt");
        // no stop, no notice: a session that is simply working says nothing
        let mut e = Engine::new(0);
        let acts = run(&mut e, 0, 600_000, |t| {
            let mut o = obs(t, WatchPowerMode::On);
            o.ended = false;
            o
        });
        assert!(acts.is_empty());
    }

    #[test]
    fn end_to_end_nothing_happens_without_the_agents_stop_hook() {
        // however quiet the process is, one signal is not an ending
        let (acts, e) = timeline(WatchPowerMode::On, PERFECT, None, None, None, None);
        assert!(acts.is_empty(), "it keeps waiting, silently: {acts:?}");
        assert!(!e.finished());
    }

    #[test]
    fn end_to_end_a_failure_in_the_final_message_holds_and_asks() {
        for tail in [
            "Ran the suite.\n2 failed, 40 passed.\nDone.",
            "I couldn't finish the migration.",
            "Implemented it.\nWould you like me to run the tests?",
            "Traceback (most recent call last):\n  File \"x.py\", line 1",
        ] {
            let (acts, _) = timeline(WatchPowerMode::On, tail, Some(20_000), None, None, None);
            assert!(!fired(&acts) && began(&acts).is_none(), "{tail}");
            assert!(acts.iter().any(|(_, a)| matches!(a, Action::SayAndAsk(..))), "{tail}: {acts:?}");
        }
    }

    #[test]
    fn end_to_end_a_final_message_that_never_arrived_is_not_trusted() {
        // the hook sent no text: with nothing to judge and no model there is no go
        let (acts, _) = timeline(WatchPowerMode::On, "", Some(20_000), None, None, None);
        assert!(!fired(&acts) && began(&acts).is_none());
        assert!(acts.iter().any(|(_, a)| matches!(a, Action::SayAndAsk(t, _) if t.contains("could not read"))), "{acts:?}");
    }

    #[test]
    fn end_to_end_a_new_prompt_after_the_stop_keeps_the_watch_going() {
        // stop at 20 s, but the user sends another prompt at 30 s and the agent works for 40 s: no conclusion
        let (acts, _) = timeline(WatchPowerMode::On, PERFECT, Some(20_000), Some(30_000), None, None);
        assert!(!fired(&acts));
        assert!(began(&acts).is_none() || began(&acts).unwrap() > 70_000, "{acts:?}");
    }

    #[test]
    fn end_to_end_every_cancel_path_stops_a_real_countdown() {
        // a key press shortly after the countdown starts
        let (acts, _) = timeline(WatchPowerMode::On, PERFECT, Some(20_000), None, Some(75_000), None);
        assert!(!fired(&acts) && acts.iter().any(|(_, a)| matches!(a, Action::End(r) if *r == "cancelled: input")), "{acts:?}");
        // "cancel" said aloud
        let (acts, _) = timeline(WatchPowerMode::On, PERFECT, Some(20_000), None, None, Some(75_000));
        assert!(!fired(&acts) && acts.iter().any(|(_, a)| matches!(a, Action::End(r) if *r == "cancelled: voice")), "{acts:?}");
    }

    #[test]
    fn end_to_end_off_and_practice_never_fire_on_the_same_perfect_finish() {
        for mode in [WatchPowerMode::Off, WatchPowerMode::Dry] {
            let (acts, e) = timeline(mode, PERFECT, Some(20_000), None, None, None);
            assert!(!fired(&acts) && began(&acts).is_none(), "{mode:?}");
            assert!(e.finished());
        }
    }
}
