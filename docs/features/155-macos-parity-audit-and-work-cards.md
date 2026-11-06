# 155 — macOS parity audit and work cards (AUDIT ONLY, nothing here has run on a Mac)

Asked for: "make NEXUS 100% compatible for the MacBook too". This document is the evidence-based starting
point. **No Rust, TypeScript or Python was changed for macOS.** Two small config edits are listed in §5.

## 0. Honest status

* Nothing in this repository has ever been compiled for macOS by the people writing it. `docs/features/117`
  says so, and `.github/workflows/macos-port.yml` exists to fix that, but see finding **P0-3**: that job only
  runs on the branch `feat/action-kernel`, so it does not run on `main`, on this branch, or on pull requests.
* This audit was done from a Linux cloud container with no Apple SDK. `cargo check --target
  aarch64-apple-darwin` was attempted twice and fails in C/Objective-C build scripts (`objc2-exception-helper`,
  `ring`) because no macOS headers exist. So **every claim below is read from source, not observed running**.
* "100%" is not a claim anyone can make until a real MacBook (or the `macos-14` GitHub runner for the compile
  half) has run each card in §4. Camera, microphone, Accessibility, Screen Recording and Automation behave
  differently per macOS version; they need hands-on checks.

## 1. What already works on the macOS code path (read from source)

| Area | macOS implementation found |
|---|---|
| Open / close apps, volume, mute, screenshot, lock | `command_executor.rs` (`open -a`, `osascript`, `screencapture`, `CGSession`) |
| App discovery | `app_registry.rs` (`/Applications` scan, bundle id, `open -b`) |
| Master volume lease | `volume.rs` (osascript backend) |
| Keep-awake | `keep_awake.rs` (macOS backend) |
| Power commands | `power_cmd.rs` |
| Media / Spotify / Apple Music | `media_player.rs` (AppleScript) |
| Browser URL of the active tab | `browser_url.rs` (AppleScript) |
| Screen capture | `screen_capture_mac.rs` (`screencapture -R`, points not pixels) |
| Safety-net speech | `tts_system.rs` (macOS branch) |
| Meeting detection | `meeting_detect.rs` (process-name list incl. `zoom.us`) |
| Paste (Cmd+V) | `live/commands/keyboard.rs` (`Key::Meta` on macOS) |
| Hand mode live pointer | `hand/mod.rs` (AXIsProcessTrusted guard, CoreGraphics cursor position) |
| Raw key chords | `shortcuts/exec.rs` (`vk_to_mac`, enigo `Key::Raw`) |
| Secrets | `keyring` with `apple-native` (Cargo.toml) |
| Bundle metadata | `Info.plist` (mic, camera, screen recording, Apple Events strings, `nexus://` scheme), `icons/icon.icns`, `macOSPrivateApi` |

## 2. Findings that make the app wrong or unusable on a Mac

### P0 — blockers

**P0-1. The main hotkey is Spotlight.** `src-tauri/src/hotkey.rs` registers `"CommandOrControl+Space"`.
Tauri resolves `CommandOrControl` to **Cmd on macOS**, so the "call / cancel NEXUS" key becomes Cmd+Space, which
macOS reserves for Spotlight. Registration will fail or fight the OS. Ctrl+Space is also taken on Macs with
more than one input source ("Select previous input source"). The same module registers `CommandOrControl+Shift+S`
(settings) and `CommandOrControl+Alt+X` (stage kill switch); these are not reserved by macOS but become
Cmd-based, which is a behaviour change from Windows. *Owner decision needed:* pick the macOS call key
(candidates: `Alt+Space`, `Control+Alt+Space`, `Cmd+Shift+Space`). The unit test
`hotkeys_has_exactly_three_documented_entries` pins the current strings and must change with it.

**P0-2. Every on-screen button is unclickable.** `stage.rs::cursor_pos()` returns `None` outside Windows. The
`stage-hitbox` loop turns that into `inside = false` and keeps `set_ignore_cursor_events(true)` forever, so the
fullscreen transparent stage never receives a click: confirm / cancel buttons, tour close, PR list actions,
orb calibration. (`hand/mod.rs` already has a working macOS `cursor_pos()` via CoreGraphics that can be reused.
Unit conversion needs care: stage hitbox rects are physical pixels on Windows, macOS reports points.)

**P0-3. The macOS compile gate is not running.** `.github/workflows/macos-port.yml` triggers on
`feat/action-kernel` only. Without it, a macOS compile error in any `cfg(target_os = "macos")` block is
invisible until a release build fails. See §5 (edited).

**P0-4. Hardened-runtime entitlements are incomplete.** `entitlements.plist` has audio-input but no
`com.apple.security.device.camera` (Hand Mode's camera would be denied after signing) and no
`com.apple.security.automation.apple-events` (the `osascript` calls for browser URL, media and volume would be
denied). See §5 (edited). `signingIdentity` in `tauri.conf.json` is the placeholder
`Developer ID Application: YOUR_TEAM`; the owner must supply a real identity and notarisation credentials.

**P0-5. Keyboard shortcuts send Windows chords.** `shortcuts/map.rs` embeds only `seed_windows.json`, and the
macOS sender maps VK `0x11` (Ctrl) to `kVK_Control`. So "new tab", "close tab", "paste", "refresh" etc. send
**Control+T / Control+W / ...**, which do nothing on a Mac (they need Command). The router's title-change
verification will then report the failure rather than lie, but the feature does not work. A
`seed_macos.json` (or a Ctrl→Cmd mapping layer plus a reviewed override list) is required. The legacy fallback
`screen.rs::switch_browser_tab` has the same defect (`Key::Control` + digit).

### P1 — features that silently do nothing on macOS

| Feature | Evidence | Effect on a Mac |
|---|---|---|
| Ghost "click the X button" grounding | `screen.rs::unix_extra::list_actionables()` returns `Vec::new()` | Named clicks never resolve; falls to vision (needs a Gemini/Groq key) |
| Ghost mouse / app launcher | `live/commands/mouse.rs` stub returns `None`; `launcher.rs` returns `Err("Windows-only")` | "open X" via Ghost drill fails; Ghost cursor ring has no position |
| Local OCR fallback | `ocr.rs` is `#[cfg(target_os = "windows")]` in `lib.rs` | "what's on my screen" with no cloud key has no offline path |
| "Tidy up" windows / display off | `window_tile.rs` non-Windows: empty window list, `display_off()` returns Err | Tile and "turn the screen off" refuse |
| Focus Mode throttling | `focus_throttle.rs` non-Windows: `lower()` returns `None` | Focus Mode pauses NEXUS loops but never lowers other apps (safe, but half a feature) |
| Adaptive glass luminance | `luminance_probe.rs` returns `None` (defaults to 30.0 = always "dark") | Light wallpapers get dark-mode text contrast |
| Liquid-glass sidebar blur | `sidebar_backdrop.rs`, `dwm_corners.rs` are Windows-only | No blur / rounded-corner treatment; `window-vibrancy` is already a dependency and is the likely route |
| WebView permission auto-approve | `mic_permissions.rs` handler is WebView2-only | WKWebView will show its own camera/mic prompt per window |
| Esc cancels Ghost via global shortcut | works only where `tauri-plugin-global-shortcut` exists (not Linux) | Expected to work; unverified |

### P2 — packaging and tooling

* `nexus start` on macOS launches the binary directly (no `run.ps1`). No unified log console, and the release
  provenance check described in AGENTS.md is a Windows PowerShell flow.
* STT / NLU sidecars need Python 3 plus `moonshine-voice` on the Mac (not bundled). `nexus.mjs` already uses
  `python3` on non-Windows, but `lazy_stt.rs` probes `python`, `python3`, `py` and has a Windows-registry
  fallback only; on macOS the Xcode stub `python3` can exist without a working interpreter.
* `agent_watch` needs Node plus `node-pty` prebuilds for darwin (upstream ships them; unverified here).
* ONNX Runtime (`ort = =2.0.0-rc.12`) execution provider on Apple Silicon is CPU only unless CoreML is enabled;
  Kokoro and wake-word latency on a MacBook are unmeasured.
* No macOS wake-word, STT-accuracy, or Retina/multi-monitor measurement exists. The voice-accuracy scorecard
  numbers are Windows-only.
* The Windows-only scripts (`scripts/*.ps1`, `.cmd`) have no shell equivalents for `nexus build` provenance.

## 3. Priorities

1. Turn the compile gate on (done in §5), read the errors it produces, fix them. Until this passes nothing else
   can be trusted.
2. P0-1 … P0-5 (hotkey, stage click-through, entitlements/signing, shortcut map).
3. Ghost grounding on macOS (AXUIElement) + OCR via the Vision framework: these two make "analyse my screen"
   and "click X" work without cloud keys.
4. Glass blur, luminance, tiling, Focus Mode.
5. Real-hardware checklist (§6).

## 4. Work cards (each sized to be given to one coding-agent run, e.g. GLM 5.3 Flash via OpenCode)

Rules for every card: edits only under the listed files; keep Windows behaviour byte-identical (cfg-gate every
change); add a unit test for any pure logic (key maps, coordinate conversion, parsers) that runs on Linux CI;
the macOS-specific glue is verified by the `macos-14` job (`cargo check --lib --target aarch64-apple-darwin` and
`cargo test --lib`); do not run or restart NEXUS; do not touch `AGENTS.md` ownership rules.

| # | Card | Files | Done when |
|---|---|---|---|
| M1 | Fix whatever `cargo check --target aarch64-apple-darwin` reports on the first CI run | whatever it names | CI job green |
| M2 | macOS hotkey set (owner picks keys first, see P0-1) | `hotkey.rs` (+ its test) | no `Cmd+Space`; test updated; Windows strings unchanged |
| M3 | macOS `cursor_pos()` for the stage + point→stage-rect conversion | `stage.rs`, reuse `hand/mod.rs` CoreGraphics call | pure conversion function with tests; loop toggles click-through when cursor is inside a rect |
| M4 | `seed_macos.json` + loader selecting by OS; Cmd instead of Ctrl; fix `switch_browser_tab` | `shortcuts/map.rs`, `shortcuts/seed_macos.json`, `screen.rs` | existing seed validity tests also run on the mac seed; no scope binds two actions to one chord |
| M5 | Entitlements/Info.plist review and signing notes | `entitlements.plist`, `Info.plist`, `tauri.conf.json`, `docs/` | camera + apple-events present (§5), notarisation steps written, placeholder identity documented |
| M6 | AXUIElement `list_actionables()` (role, title, frame; skip secure text fields; Accessibility permission check with spoken guidance) | `screen.rs` `unix_extra` | returns controls for Finder and Safari; empty with spoken guide when permission missing |
| M7 | Ghost app launcher + mouse position on macOS (`open -a` / CoreGraphics) | `live/commands/launcher.rs`, `live/commands/mouse.rs` | `open_app_via_search` replaced by a mac path; stub removed |
| M8 | Vision-framework OCR (VNRecognizeTextRequest) behind the same function shape as `ocr.rs` | new `ocr_mac.rs`, `lib.rs` | `capture_region_text_rects` equivalent returns text + rects |
| M9 | Window tiling + display off (`NSScreen` visible frame, `pmset displaysleepnow`) | `window_tile.rs` | tile math reuses the existing pure layout code |
| M10 | Glass: `window-vibrancy` sidebar material, luminance sampling via CoreGraphics | `dyn_windows.rs`, `luminance_probe.rs` | no blur-on-inactive-window regression (see the AGENTS.md note about non-activating windows and DWM; macOS needs the same care with `NSVisualEffectView` state) |
| M11 | Focus Mode on macOS (`setpriority` / `taskpolicy`, never the foreground app) | `focus_throttle.rs` | restore-after-crash baseline file works the same as Windows |
| M12 | Python/Node discovery on macOS (Homebrew, pyenv, reject Xcode stub) | `lazy_stt.rs`, `lazy_nlu.rs`, `agent_watch.rs` | clear spoken error when no real interpreter exists |
| M13 | macOS `nexus start` / build provenance without PowerShell | `nexus.mjs`, `scripts/` | start refuses a stale binary the same way Windows does |
| M14 | Enable the CoreML execution provider for ONNX Runtime and measure | `Cargo.toml`, wake/NLU/Kokoro loaders | latency numbers recorded in `docs/` before claiming a win |

Suggested order: M1, M5, M2, M3, M4, then M6, M8, M7, then the rest.

## 5. Two small config edits made with this document

1. `.github/workflows/macos-port.yml`: also run on pushes to `main` and on every pull request (it ran only for
   `feat/action-kernel`). This is what lets a cloud agent get real macOS compiler output without a Mac. The
   `cargo test --lib` step is now `continue-on-error` so a first-run test failure does not turn every PR red;
   the `cargo check` step is the gate. **Side effect:** pull requests will now show a macOS check, and it may be
   red on its first run (that is the point: it will list the real compile errors for card M1).
2. `src-tauri/entitlements.plist`: added `com.apple.security.device.camera` and
   `com.apple.security.automation.apple-events`. Neither takes effect until a signed build is made; they cannot
   break the Windows build.

## 6. Real-hardware checklist (the only way to earn the word "works")

Run on an Apple-Silicon MacBook, a fresh user account, after a signed build:

1. First launch: microphone prompt, then wake word fires on "Nexus" / "Hey Nexus"; note CPU and RAM.
2. Accessibility, Screen Recording, Automation and Camera prompts each appear once with the Info.plist wording.
3. The call key (after M2) opens/cancels a turn and does not open Spotlight.
4. A confirm card's buttons are clickable (M3).
5. "Open Safari", "new tab", "close tab", "paste" (M4).
6. "What's on my screen" with no cloud key (M8) and with a key.
7. Ghost: "click the …" in Finder/Safari (M6/M7); Esc stops it.
8. Hand Mode: camera, arm gesture, live pointer, Retina scale, second monitor.
9. Sleep/wake, lid close, audio device change (AirPods), external display hot-plug.
10. Quit from the menu bar; confirm no orphan Python/Node processes.

Record each result in `docs/testing/` with macOS version and chip. Until that table exists, treat macOS as
"experimental, compiles in CI".

## 7. Where to test: free and cheap Mac options (researched 2026-11-02)

Source quality: runner specs, the TCC issue, Codemagic/Xcode Cloud allowances and the cloud-Mac price list come from web
search results and one fetched GitHub issue. Several vendor/docs hosts (docs.github.com, macincloud.com, aws.amazon.com)
were unreachable from the cloud session, so **prices and free-tier terms below must be re-checked on the vendor page before
spending money**. Nothing here was run on a Mac.

| Option | Cost | What it can test | What it cannot |
|---|---|---|---|
| **GitHub Actions macOS runners** (`macos-14/15/26`, Apple-silicon M1, 3 cores, 7 GB; Intel labels exist) | This repo is public; standard hosted runners are free for public repos per GitHub's billing policy (not re-confirmed this session) | Compile (`cargo check/build --target aarch64-apple-darwin`), `cargo test --lib`, frontend build, unsigned `.app`/`.dmg` artifact, headless smoke tests, screenshots | No microphone/camera/speaker hardware, no interactive desktop, no nested virtualization |
| Same, with TCC edits | Free | Screen Recording permission can be pre-granted by inserting into `TCC.db` on the macOS 13/14 images (runner-images issue 9529). Mic, camera, Accessibility were **not** confirmed | Newer images (15/26) unconfirmed |
| Codemagic | Reported 500 free macOS (M2) minutes/month, 1 build at a time | Same as above; redundant while GitHub is free | Built for mobile apps |
| Xcode Cloud | About 25 compute hours/month with an Apple Developer Program membership | Xcode projects only | Poor fit for Tauri/Rust |
| Cirrus CI | Reported shut down in 2026 (from a competitor's blog, unconfirmed) | Do not plan on it | |
| MacinCloud pay-as-you-go | About $1/hour, prepaid credit (page is old, re-check) | Real GUI session over remote desktop: permission prompts, hotkeys, tray, stage click-through, window look | Cloud Macs usually have no mic/camera |
| Scaleway Mac mini M2 | About 0.17 EUR/hour but **24 hour minimum** (about 4 EUR a lease) | Same, plus SSH and a full day to iterate | Same |
| AWS EC2 Mac | About $0.88-1.23/host-hour, **24 hour minimum** (about $21+) | Same | Most expensive |
| MacStadium | From about $109/month | Long-running test box | Not hourly |
| A physical MacBook (own, borrowed, refurbished) | Free if borrowed | Everything, including microphone, wake word, camera/Hand Mode, Retina, sleep/wake, AirPods | |

Do not use Hackintosh, OSX-KVM or Docker-OSX: Apple's licence allows macOS virtualisation only on Apple hardware, and audio/GPU
would be unrepresentative anyway.

### What each tier can prove

* **GitHub runner (free):** it compiles, unit tests pass, the bundle builds. Cards M1-M5 and the pure-logic parts of M3/M4/M9
  (coordinate conversion, key maps, tiling maths) belong here.
* **Rented GUI Mac (about 5-25 EUR):** the app starts, the call hotkey works and does not open Spotlight, stage buttons are
  clickable, tray and settings windows look right, AppleScript calls get their permission prompt, Ghost grounding (M6) finds
  controls in Finder/Safari.
* **Physical Mac only:** microphone and wake word, camera and Hand Mode, signing and notarisation, AirPods/Bluetooth, lid and
  sleep behaviour, battery behaviour.

### Plan

1. **P0, free, now:** get the existing `macos-port` job to run on this branch (it needs a pull request or a push to `main`;
   the file was edited in section 5) and fix whatever it reports. A cloud agent can read the job logs and iterate
   push -> CI -> fix with no Mac at all. That is the free "Mac" for the compile half.
2. **P1, free:** cards M2-M5 (hotkey, stage cursor position, Cmd shortcut seed, entitlements), each with Linux unit tests plus the
   macOS compile check.
3. **P2, free:** add a CI step that builds an unsigned `.app`/`.dmg` artifact and a headless smoke test (needs a small
   `--smoke` startup flag that exits after the stage window and wake engine initialise).
4. **P3, about 5-25 EUR:** one 24 hour Scaleway lease (or MacinCloud prepaid hours) for a single GUI pass over the section 6
   checklist items that do not need a microphone or camera. Do M6-M10 first so the session is not wasted on known gaps.
5. **P4:** a physical MacBook for the audio/camera/signing items. Signing and notarisation need an Apple Developer Program
   membership (paid annual fee, confirm current price).
