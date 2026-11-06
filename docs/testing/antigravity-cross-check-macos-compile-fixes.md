# Antigravity / OpenCode cross-check brief — macOS compile fixes and close-safety fix (commits 76ec071 .. ed1ab93)

Purpose: an independent review of the 8-error macOS compile fix, by a different tool than the one that wrote it.
Status: **NOT RUN.** `agy` is not installed in the cloud session that wrote the fix, so no Antigravity review has happened.
Run it on the owner's PC in a **separate git worktree** (never in the main working tree: it carries uncommitted owner work).

## Run

```powershell
git fetch origin claude/bold-noether-fizcog
git worktree add ..\nexus-xcheck origin/claude/bold-noether-fizcog
cd ..\nexus-xcheck
agy --print "$(Get-Content docs/testing/antigravity-cross-check-macos-compile-fixes.md -Raw)" --mode default
```

Use `--mode default` (review only, no auto-accepted edits). Do not pass `accept-edits`: this is a **read-only review**.
If `agy` hangs with `--sandbox` on Windows (a reported issue), run without it.

## Prompt (give this to the reviewer)

You are reviewing the commits `76ec071`, `ae364bc` and `ed1ab93` (`git diff 76ec071~1 ed1ab93 -- src-tauri .github`). They change only these files:
`shortcuts/exec.rs`, `screen_capture_mac.rs`, `live/mod.rs`, `vision.rs`, `browser_url.rs`, `hand/mod.rs`, `command_executor.rs`,
`action_kernel/paths.rs`, `agent_hooks.rs`, `keep_awake.rs`, `live/state.rs`, `site_probe.rs` (all under `src-tauri/src/`) and `.github/workflows/macos-port.yml`.
Do NOT edit anything. Report findings only. For each finding give: file:line, severity (blocker / should-fix / note),
what is wrong, and the evidence you checked. If you cannot verify something, say so instead of guessing.

Check, in this order:

1. **Windows is unchanged.** For every hunk, state whether the code compiled for `target_os = "windows"` is byte-for-byte
   the same behaviour as before. The only Windows-visible edits should be `#[cfg(target_os = "windows")]` wrappers that keep
   the same statements. Flag any hunk where Windows behaviour could differ (control flow, return type, unused-variable or
   unused-import warnings that would fail a `-D warnings` build).
2. **`live/mod.rs` `live_ghost_click`.** On non-Windows the function must return an error result and not touch `app`
   incorrectly; on Windows the `match` must be the function's tail value exactly as before.
3. **`vision.rs` `resolve_named_target`.** A `#[cfg]` on an `if let` statement: confirm it is valid and that on non-Windows
   `name` is still used later, so no unused-variable warning.
4. **`shortcuts/exec.rs`.** Confirm against the `enigo` 0.5.0 source (cargo registry) that `Keyboard::raw(&mut self,
   keycode: u16, direction: Direction)` exists, takes a macOS `CGKeyCode`, and that the macOS backend updates modifier flags
   for modifier key codes (look for `add_event_flag` in `src/macos/macos_impl.rs`). Confirm `Key` is no longer imported
   where it is unused.
5. **`screen_capture_mac.rs`.** `dimension_to_i32`: any input where it returns a wrong value or panics (NaN, infinity,
   `-0.0`, values near `i32::MAX`, `.5` rounding)? `use image::GenericImageView;`: could it conflict with another import?
6. **`hand/mod.rs` and `browser_url.rs`.** The new macOS `guard_now` branch must refuse (fail closed) when both the foreground
   title and process are empty. Is there any other macOS path that sends clicks without passing through `guard_now`?
   Are there other callers of `foreground_process_name` / `foreground_window_title` on macOS that would now behave differently?
7. **What else would fail on macOS** after these 8 errors are gone? rustc stops before borrow-checking when name-resolution
   errors exist, so list any Windows-only symbol still referenced from shared (non-cfg) code. Search for calls into modules or
   functions that are `#[cfg(target_os = "windows")]` and that are reachable on macOS.

8. **Close-safety fix (`command_executor.rs`).** Voice "close X" used `pkill -x` on macOS and its protected list held only Windows
   names. Review `PROTECTED_PROCESSES_MACOS`, `is_protected_process`, `MACOS_QUIT_SCRIPT`, `macos_quit_args` and `close_app`:
   - Is the app name ever spliced into AppleScript text, or only passed as `argv`? Can any spoken or registry name reach the script?
   - Is `loginwindow`, `Finder`, `Dock`, `WindowServer`, `SystemUIServer` refused through every path to `close_app` (including other
     callers of `plan_close`, and names that arrive via the registry display name rather than an exe stem)?
   - With `macos == false` is `is_protected_process` exactly the old `PROTECTED_PROCESSES.contains(stem)` check?
   - AppleScript: is `tell application appName to quit` with a variable valid, and is the `System Events` running-check right?
     (A `cfg(macos)` test already compiles the script with `osacompile` in CI; runtime behaviour and Automation-permission
     prompts are NOT tested anywhere.)
   - `#[cfg(any(target_os = "macos", test))]` on the two helpers: any build configuration where they are used but missing?
9. **Test-only changes.** `paths.rs`, `agent_hooks.rs`, `keep_awake.rs`, `live/state.rs`, `site_probe.rs`: confirm each edit only
   changes test code, and that none weakens what the test proves (the timing tests widened their margins; check the assertions
   still distinguish pass from fail).
10. **CI (`macos-port.yml`).** The test step is no longer `continue-on-error`. Confirm a failing test fails the job and that the
    summary step always runs.

Output a table of findings, then a one-line verdict: SAFE TO MERGE / CHANGES NEEDED.

## What the cloud session already verified (so the reviewer can concentrate on the rest)

* `screen_capture_mac.rs` functions compile and their unit tests pass in a scratch crate on Linux against `image 0.25.10`.
* The close-safety helpers and their tests were extracted into an isolated crate and run on Linux; 7 mutations (drop `loginwindow`,
  leak the macOS list into Windows, put the name inside the script, drop or misplace the running check, allow control characters)
  are all caught by the tests.
* **CI result on `ed1ab93`:** macOS `cargo check` passed; macOS `cargo test --lib` = 2846 passed, 0 failed, 22 ignored (was 2835
  passed, 7 failed); Windows compile checks, dead-code ratchet, unit tests and installer build passed; frontend/Worker/Python
  checks passed.
* Not verified anywhere: AppleScript behaviour at runtime on a real Mac, macOS permission prompts, anything needing a microphone,
  camera, or a person at the screen.
* Known open issue (not part of these commits): some unit tests launch real apps (Safari, Google Chrome, System Events were found
  running at the end of the macOS CI job). On a developer Mac `cargo test` would open real browsers.
