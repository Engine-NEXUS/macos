# 156 — One codebase for Windows and macOS, and how Antigravity does the coding (PLAN ONLY)

Asked for: research and a plan for making NEXUS work perfectly on a MacBook **from this same repository** (no separate Mac
repo), with Antigravity doing the editing and coding, and the Windows build staying perfect. **Nothing in this document has been
implemented. No source file was changed for it.** It builds on `155-macos-parity-audit-and-work-cards.md` (findings M1-M14).

## 0. Where things stand (2026-11-02)

* The audit (doc 155) found the P0 blockers: Cmd+Space hotkey, click-through stage never getting clicks, Windows-only Ctrl
  shortcut map, missing entitlements, Windows-only Ghost grounding / OCR / tiling / Focus Mode / glass.
* A real macOS compile ran for the first time on a free GitHub `macos-14` runner (run 41466573865). At the time of writing it
  was still in the `cargo check` step with no result. **The number and kind of macOS compile errors is unknown until it
  finishes**; card M1 and the effort estimates below will be re-sized from it.
* Antigravity (`agy`) is **not installed in the cloud session** (checked). It runs on the owner's Windows machine, and the
  Windows machine **cannot compile macOS code**. So Antigravity's macOS-specific code is written blind and can only be proven by
  the macOS CI runner.
* Per `AGENTS.md`, the owner controls runtime and builds; edits and documentation only, unless authorised.

## 1. One codebase: the rules

Goal: the same repo, same branch, same release pipeline produces the Windows installer and the macOS `.dmg`.

**1.1 Windows is protected by construction.**
Every macOS addition is either a new file or inside `#[cfg(target_os = "macos")]`. Windows code is not edited, except by pure
relocation (moving a function into `platform/windows.rs` with its tests unchanged). A Windows build therefore cannot
contain, call or link any new macOS code.

**1.2 One seam per capability, not `#[cfg]` scattered in callers.**
Today OS differences are spread over ~60 files as inline `cfg` blocks and stubs such as `screen::unix_extra`,
`live/commands/mouse::unix_stub`, `stage::cursor_pos`. The target shape is a small `src-tauri/src/platform/` layer:

| Capability (seam) | Windows (exists today) | macOS (to build) | Other |
|---|---|---|---|
| `cursor_pos()` | `GetCursorPos` | CoreGraphics (already written in `hand/mod.rs`) | stub |
| `foreground_window()` | Win32 | `NSWorkspace` / AppleScript | stub |
| `ui_controls()` (Ghost grounding) | UI Automation | AXUIElement | stub |
| `ocr_region()` | `Windows.Media.Ocr` | Vision `VNRecognizeTextRequest` | stub |
| `tile_windows()` / `display_off()` | Win32 | `NSScreen` frames / `pmset displaysleepnow` | stub |
| `lower_priority()` (Focus Mode) | `SetPriorityClass` + EcoQoS | `setpriority` / `taskpolicy` | stub |
| `launch_app()` | registry + Win-search | `open -a` / bundle id | stub |
| `glass_material()` | DWM + screenshot blur | `window-vibrancy` (already a dependency) | none |

Callers use `platform::cursor_pos()` and never mention an OS. **Migration is incremental ("strangler")**: the seam is
introduced the first time a card needs it; existing Windows code moves in by relocation only. No big-bang refactor, because a
big refactor of a Windows-first 100k-line app is the one thing that could hurt Windows.

**1.3 Pure logic is shared and tested on every OS.**
Anything that is just maths or tables lives in OS-neutral modules with unit tests that run on Linux, Windows and macOS CI:
point-to-stage-rect conversion (points vs pixels), the Ctrl-to-Cmd modifier mapping, tile layout maths, parsing of
`osascript` / `screencapture` output, hotkey tables. Only the thin OS call is cfg-gated.

**1.4 Differences that are data, not code, stay data.**
* Shortcuts: `seed_windows.json` plus a new `seed_macos.json`, same schema, same validity tests run on both.
* Hotkeys: one per-OS table in `hotkey.rs` (the unit test that pins the three strings becomes per-OS).
* App discovery, paths (`app_dirs`), music players, media keys already branch per OS correctly.

**1.5 Frontend and config.**
* The frontend asks Rust once (`get_platform`) and uses it for labels (Ctrl vs Cmd, "Start menu" vs "Spotlight") and a
  `data-os` attribute for CSS differences. No user-agent sniffing.
* `tauri.conf.json` stays shared. Per-OS window options (decorations, transparency, vibrancy) go into Tauri's
  platform-specific config files (`tauri.windows.conf.json`, `tauri.macos.conf.json`, merged by target). *To confirm in the
  Tauri v2 docs before relying on it.* Capabilities are already per platform (`global-shortcut.json`).
* One `settings.json`; OS-specific defaults come from functions, not forks.

**1.6 Tooling.**
`nexus.mjs` is already cross-platform. The Windows-only parts (`run.ps1` log console, build provenance) get Node
equivalents later (card M13), they are never replaced on Windows.

## 2. Guard rails: how Windows stays perfect

| Rail | What it does | Status |
|---|---|---|
| R1 Windows CI on every push | `build-windows.yml` and the `ci.yml` Windows jobs must stay green; the owner builds by hand, so CI is the only automatic guard | exists |
| R2 macOS CI on every push | `macos-port.yml`: `cargo check --lib --target aarch64-apple-darwin`, frontend build, `cargo test --lib` | exists, now triggers on `claude/**` |
| R3 Linux CI | `ci.yml` Ubuntu jobs; catches code that accidentally needs a Windows or macOS symbol outside a cfg | exists |
| R4 Windows-diff gate | a planned script `scripts/check-windows-untouched.mjs`: for a card branch, fail if any file under a Windows-only path changed other than by a recorded relocation | **new, to build first** |
| R5 Seam gate | a planned script: fail if `windows::`, `winreg`, `uiautomation` or macOS framework symbols appear outside files/blocks gated for that OS | **new** |
| R6 Existing gates | `check-turn-ends`, `check-repo-hygiene`, `check-ipc-contract`, `dead-code-audit` (baseline 147-148, may only shrink), `check-window-opacity` | exist; note new shared seam functions can trip the dead-code ratchet on the OS where they are unused |
| R7 Mutation checks | each pure-logic card is mutation-checked (flip a mapping, test must fail), as for previous cards | practice |
| R8 Kill switches | each behavioural change gets an env kill switch like `NEXUS_SHORTCUTS_LEGACY=1` | practice |
| R9 Honest status | a macOS feature is "compiles in CI", then "works on a rented/real Mac" only after a recorded run in `docs/testing/` | practice |

## 3. Using Antigravity (`agy`)

**What is known (from `AGENTS.md`, i.e. the owner's own use):**
* Headless invocation that worked: `agy --print ... --mode accept-edits --sandbox --dangerously-skip-permissions` (the skip flag is
  required in headless mode; the owner approved it).
* It built Focus Mode, owner-less GitHub, name-free notifications, notification state machine, GitHub Actions and deploy watchers
  from **written briefs**, then the work was reviewed and hand-fixed.
* Recorded failure modes, which the brief and review below are designed against: it rewrote glue code from memory, left dead
  "superseded" copies, dropped a `#[test]`, did not compile its own tests (missing `Debug`/`PartialEq` derives), treated every HTTP
  403 as a rate limit, and used a wrong module path in tests.
* Hooks are only loaded from a git project root.

**What web research adds (community sources, not official docs, treat as unverified):** `agy -p` is the print/CI mode; `--mode
accept-edits` auto-accepts edits; `--model <slug>` and `agy models` choose the model; `--print-timeout` defaults to 5 minutes; a
GitHub issue reports `agy -p --sandbox` hanging on Windows. Whether `agy` reads `AGENTS.md` was not confirmed, so every brief
must carry its own rules.

**Where it runs and the loop:**

```
brief (written here)  ->  Antigravity on the owner's PC, in its OWN git worktree on a card branch
      ->  commit + push to the card branch
      ->  Windows CI + Linux CI + macOS CI run automatically
      ->  logs read here (no Mac needed)  ->  hand-fixes / next brief  ->  merge into claude/bold-noether-fizcog
```

* Antigravity works in a **separate worktree**, not in the owner's main tree. `AGENTS.md` records a stale write dropping edits
  (2026-10-14) and a full workspace collision (2026-10-01); the main tree also carries uncommitted owner work.
* It cannot compile macOS locally. Its macOS code is **untrusted until the macOS CI job is green**.
* Alternative if the owner prefers: the same cards coded directly in the cloud session with the CI loop. Antigravity still adds
  value for large mechanical cards (a full `seed_macos.json`, AXUIElement and Vision bindings), but it is not required.

**Brief template (one per card; this is what makes the result safe):**
1. Goal in one sentence, and which doc 155 card it is.
2. Exact files it may create or edit (everything else is read-only). Windows-only files are listed as **do not edit**.
3. Rules: every macOS line behind `#[cfg(target_os = "macos")]` or in a new file; no change to Windows behaviour; no new
   dependency unless named in the brief; no refactors, no renaming, no "superseded" copies; do not touch `AGENTS.md`.
4. API discipline: use only APIs it can cite from Apple or crate documentation; if unsure, write `todo!()`-free code that returns
   `Err("unsupported")` rather than guessing a signature.
5. Tests it must add (pure logic, runnable on Linux) and the commands that must pass; state that it must run `cargo test` for
   the tests it adds before reporting done.
6. Output contract: list files changed, tests added, anything it was unsure about.

**Review checklist after each Antigravity run (every item, every time):**
`git diff` limited to the listed files; Windows-diff gate (R4) clean; seam gate (R5) clean; no dead copies; tests present and
actually compiled; Windows, Linux and macOS CI green; dead-code ratchet not grown; one reviewer read of the diff.

## 4. Phases

| Phase | Cards | Executor | Verified by | Exit criteria |
|---|---|---|---|---|
| 0 Foundations | build R4 and R5 gates; add `get_platform`; create the empty `platform/` module with the stub impls | Antigravity or cloud | Windows + Linux + macOS CI | gates exist, Windows diff is zero |
| 1 Compile | M1 (fix first macOS compile errors) | cloud (needs fast CI loop) | macOS CI | `cargo check --target aarch64-apple-darwin` green |
| 2 Blockers | M2 hotkey, M3 stage cursor position, M4 `seed_macos.json`, M5 entitlements | Antigravity from briefs; pure logic tested on Linux | CI matrix | stage click-through logic and Cmd map unit-tested; Cmd+Space no longer registered on macOS |
| 3 Parity | M6 AX grounding, M7 launcher/mouse, M8 Vision OCR, M9 tiling, M10 glass, M11 Focus Mode | Antigravity from briefs | macOS CI + one rented-Mac GUI pass | each feature returns real data on a Mac, or a spoken honest refusal |
| 4 Packaging | M12 interpreter discovery, M13 `nexus start` provenance, M14 CoreML | Antigravity or cloud | CI artifact + physical Mac | unsigned `.app` builds in CI; startup refuses a stale binary |
| 5 Hardware | mic/wake word, camera/Hand Mode, signing and notarisation, sleep/audio-device behaviour | owner on a real MacBook | `docs/testing/` run record | checklist in doc 155 section 6 filled in |

Only phases 0-2 can be fully proven without a person at a Mac. Phase 3 needs one GUI session (a free option that lets me
drive the GUI does not exist; see doc 155 section 7). Phase 5 needs a real MacBook; "100% working" cannot be claimed before it.

## 5. Risks

* **Blind macOS code from Antigravity** may compile-fail repeatedly. Mitigation: small cards, CI loop, hand-fix budget per card.
* **Seam refactor creeping into Windows code.** Mitigation: relocation-only rule and gate R4.
* **Dead-code ratchet** may count new shared functions as dead on one OS. Mitigation: seam functions are called from the
  shared caller on every OS, with the stub as the fallback.
* **Apple API drift** (macOS 14/15/26 differ on screen-recording and accessibility prompts). Mitigation: test on the CI image and
  one real Mac, record the OS version.
* **macOS CI cost/availability**: free for public repos is assumed (not confirmed from GitHub docs this session).
* **Signing and notarisation** need a paid Apple Developer membership and cannot be tested in CI without secrets.

## 6. Decisions needed from the owner

1. Who codes: Antigravity from briefs (recommended for the large mechanical cards) or the cloud session directly.
2. The macOS call hotkey (candidates: `Alt+Space`, `Control+Alt+Space`, `Cmd+Shift+Space`).
3. Approve the strangler seam (`platform/`) and the two new gates (R4, R5).
4. Whether a one-off rented Mac (about 5-25 EUR) is acceptable for the Phase 3 GUI pass, and whether a physical MacBook can
   be borrowed for Phase 5.

## 7. First real macOS compile result (2026-11-02, run 41466573865, `macos-14`, Apple silicon)

The whole dependency tree (cpal/CoreAudio, ort, tract, rusqlite, webrtc-vad, tauri, enigo, keyring, window-vibrancy) compiled
on macOS in about 3 minutes. NEXUS itself failed with **8 errors, 5 distinct causes**:

| # | Where | Cause | Class |
|---|---|---|---|
| 1 | `live/mod.rs:474` | calls `commands::mouse::ghost_click`, which exists only on Windows | missing macOS path (card M7) |
| 2 | `hand/mod.rs:330` | calls `browser_url::foreground_process_name`, Windows-only | missing macOS path |
| 3 | `vision.rs:498` | calls `mouse::resolve_element`, Windows-only | missing macOS path (M6/M7) |
| 4 | `screen_capture_mac.rs:37-38` (3 errors) | `i32::try_from(f64)` does not exist | plain bug in earlier macOS code |
| 5 | `screen_capture_mac.rs:69` | missing `use image::GenericImageView` | plain bug |
| 6 | `shortcuts/exec.rs:235` | `enigo::Key::Raw` does not exist in enigo 0.5 | wrong API assumption; needs the 0.5 raw-key API |

So the compile gap is small and mostly "a Windows-only function is called from shared code with no macOS stand-in", which is
exactly what the `platform/` seam fixes. Card M1 is a handful of edits, not a rewrite. (Fixes not started; plan only.)

## 8. Phase 5 with no money: what free testing can and cannot cover

| Phase 5 item | Free coverage | How | Limit |
|---|---|---|---|
| Wake-word model and DSP on Apple silicon | **Full** | `wake_eval` / `voice_eval` replay harness on the macOS runner, compare scores with Windows | proves inference parity, not the microphone |
| CoreAudio capture path | **Partial, experiment** | install the BlackHole virtual audio device on the runner (HAL plug-in, `sudo`, restart `coreaudiod`) and play WAVs into it; the app captures through cpal | unproven on hosted runners; microphone permission (TCC) on newer images unconfirmed |
| Hand Mode logic | **Partial** | replay recorded landmark traces (`handTrace`) through the pure engine | no camera, no WKWebView `getUserMedia` |
| Camera / Hand Mode in a real window | None | needs hardware | |
| Signing | **Partial** | ad-hoc `codesign --sign -` in CI; check entitlements and hardened-runtime flags are embedded | not trusted by Gatekeeper on other machines |
| Notarisation | None free | needs a paid Apple Developer Program membership; fee waivers exist only for nonprofits, accredited schools and governments (not individuals) | |
| Permission prompts (mic, camera, screen, accessibility) | None | need a GUI session | |
| Sleep/wake, lid, AirPods, battery | None | hardware | |

Free hardware routes: MacStadium's free open-source program (remote Mac mini; the project must be genuinely free/open source, not
commercially funded, 3+ months old, waitlist possible; **this repo has no LICENSE file in its root**, so eligibility is
unclear); borrowing a Mac for about an hour (a short checklist makes one hour enough). Free "trial" cloud Macs: one directory lists
MyRemoteMac with a 24 hour no-card trial (unverified); MacinCloud's trial is reportedly $0.99, not free.

## 9. Phase 2 results (2026-11-05) — what was built, and what was actually proven

Everything here is in the one codebase; each macOS path is behind `cfg(target_os = "macos")` (or `cfg!`) so the Windows build is
unchanged. "Proven" below means a green CI job on a real runner; nothing has run on a person's Mac.

| Card | Built | Proof so far |
|---|---|---|
| M1 compile | 8 compile errors fixed (`76ec071`), safety fix for voice "close X" (`ae364bc`, `ed1ab93`) | macOS `cargo check` + 2846 tests green; Windows green |
| M2 hotkeys | per-OS tables with roles; Mac = Control+Space (call), Control+Shift+S, Control+Alt+X; Windows strings identical | chords parsed with the real `global-hotkey` library in a test; green |
| M3 overlay clicks | stage cursor = Tauri cursor − window origin on non-Windows (pure `window_relative`) | pure tests green; **not** clicked on a real Mac |
| M4 shortcut map | `shortcuts/seed_macos.json` (Cmd chords, bundle-id keys, terminals never inherit the global table, Safari has no number-row tab switch); `active_seed()` is exactly the Windows map off macOS; Mac `WinEnv::foreground()`; `press_shortcut` maps NEXUS's own `ctrl` chords to Cmd on a Mac; tab switching and browser keys now use the shortcut brain on macOS | seed tests run on every OS (mutation: Cmd→Ctrl caught); macOS-only test that every seed chord translates to a Mac key code; macOS `cargo check` + `cargo test --lib` green on `7ce59f3` (run 11), Windows `ci` 268 (incl. dead-code ratchet) and installer build 250 green |
| (foreground) | `macos_foreground.rs`: front app from `lsappinfo list`, window title from one System Events call; parsers written against output captured on a real `macos-14` runner | parsers + real-runner tests; permission prompt on a user's Mac untested |
| (safety) | password-manager bundle ids (1Password, Keychain Access, Passwords) refused for keystrokes | unit tests |
| M12 Python | `mac_interpreters.rs`: PATH (skipping Apple's `/usr/bin/python3` stub), Homebrew, pyenv, python.org; the stub only when `xcode-select -p` succeeds | pure tests + a runner test that a Python is found and runs; green on the same `7ce59f3` run |

Facts learned from the real runner (macOS 14.8.9): `lsappinfo info` prints nothing in CI but `lsappinfo list` records carry `(in front)`,
`bundleID` and `pid`; a System Events call returns app, bundle, pid and title in about 0.12 s; a window-less app raises -1719 (wrapped
in `try`); the polite-quit script works against a real TextEdit.

Test-reliability lesson: real-time `sleep` margins flaked twice on the loaded macOS runner (a 250 ms sleep overshot a 400 ms timeout).
Prefer moving the clock (`Instant::now() - d`) over waiting.

Not done / next: M5 notarisation notes, M6 Accessibility (AXUIElement) grounding, M8 Vision OCR, M7 launcher + mouse, M9 tiling,
M10 glass, M11 Focus Mode, M13 `nexus start` on macOS, M14 CoreML. Still open: unit tests that launch real apps on a developer Mac;
the Antigravity/OpenCode review (`docs/testing/antigravity-cross-check-macos-compile-fixes.md`, not run: neither tool exists in the cloud
session) and it does not yet cover the Phase 2 changes.
