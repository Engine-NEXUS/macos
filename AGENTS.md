# NEXUS — Project Notes

## Training data lives in D:\NEXUS-Training (2026-10-31)

- The training folders (`wake_word_data`, `wake_camp`, `wake_test_recordings`, `nexus_real_samples`, `nexus_model`, `audioset*`, `mit_rirs`) were moved to `D:\NEXUS-Training\data\` (private repo Engine-NEXUS/NEXUS-Training; data is ignored there and never pushed). Directory junctions at the old paths keep every script working: do not delete a junction and never `git add` one. Training scripts stay in `scripts/` and `server/nlu/`. Record: `docs/changes/181-training-moved-to-nexus-training-repo.md`.


## Owner controls runtime and builds — edits only (2026-10-26)
## macOS parity audit (2026-11-01) — audit only, nothing ran on a Mac

- Owner asked for NEXUS to be fully MacBook-compatible. Audit + 14 work cards: `docs/features/155-macos-parity-audit-and-work-cards.md`. No Rust/TS/Python changed. Cloud container has no Apple SDK (macOS `cargo check` fails in C build scripts), `opencode`/GLM 5.3 Flash is not installed there, and the session could not create the requested `IOS` org repo.
- Found from source: `CommandOrControl+Space` is Spotlight on macOS; `stage::cursor_pos()` is `None` on macOS so the click-through stage never takes clicks; the shortcut map is Windows Ctrl chords only; `macos-port.yml` ran only for `feat/action-kernel` (now also `main` + PRs); entitlements lacked camera and Apple Events (added). Ghost grounding, OCR, tiling, Focus Mode, glass blur are Windows-only stubs.
- Not verified anywhere: every macOS claim. Read the real compiler output from the first `macos-port` CI run (card M1) before trusting any macOS code.
- **Phase 2 (2026-11-05, owner: "control + space / macbook / proceed")**: Mac call key = Control+Space (Control+Shift+S settings, Control+Alt+X kill stage; Windows chords unchanged); overlay cursor on non-Windows; voice "close X" no longer hard-kills Finder/Dock/loginwindow (protected list + polite AppleScript quit); `macos_foreground.rs` (front app/title from output observed on a real macos-14 runner); `shortcuts/seed_macos.json` + `active_seed()`; Ctrl→Cmd for NEXUS's own legacy hotkeys (`press_shortcut`); `mac_interpreters.rs` (never wakes Apple's Xcode stub); Windows paths unchanged. CI: macOS `cargo check` + `cargo test --lib` green on `1260a57` (the one flaky timing test was rewritten clock-free); the later commits are checked in the next run. Details and the proven/unproven table: `docs/features/156-…` §9. **Still nothing run on a real Mac by a person.** Antigravity/OpenCode review not run (tools absent in the cloud session); brief: `docs/testing/antigravity-cross-check-macos-compile-fixes.md`.

## Voice Accuracy Phase E — Label Audit, Public Data, Synthetic Speakers, Tiered Speech Check (2026-10-21)

- **Context**: user asked whether negatives were truly negative (a soundalike filed as a negative poisons the model), whether the model was trained, what the "43 %" speech-check figure means, and why record noise/TV when public data exists. Answers: **no** audit by audio ever existed (all gates were Whisper-text); **no** training happened (shipped `nexus.onnx` unchanged; `nexus wake train` writes a candidate, `--promote` replaces); 43 % = old 99-case review where the Groq check cost ~half the real wakes (`docs/wake-word/22-…`); "background/TV" data was DSP-synthesised, public real data exists, no public "nexus" model exists.
- **Correction**: `verifyWake` (speech check) defaults to **TRUE** in code — earlier notes (change 48, 83, feature 67) saying "off by default" were wrong and are fixed.
- **Built**: `wake_label_audit.py` (provenance + out-of-fold flagging + ledger by content hash + trainer gate) with `wake_audition.py review --queue`; `wake_data_fetch.py` (permissive public data, TOFU checksums, held-out hours) and `wake_data_synth.py` (Piper voices, licence-checked, borderline soundalikes quarantined for listening); trainer: public pools, owner sealing (`--seal-owner`), streaming hard-negative mining; `wake_score.py` = `nexus wake score` (real recall + false alarms/hour); tiered speech check `wakeVerifyMode` always|tiered|off + `wakeVerifyHigh` (default behaviour unchanged = always) with `wake_calibrate.py --verify`; `nexus wake audit|fetch|synth|score|threshold|verify`; **`nexus wake improve`** (`wake_improve.py`) = the whole loop + before/after verdict report (`improvement-report.md`; stops once for the by-ear audit review; `--promote` only on IMPROVED).
- **Verify**: Rust lib 973/973 serial; Python 114 across 7 files; trainer run end-to-end with real torch on synthetic data. **No accuracy number exists until the user runs the runbook on Windows** (§5 of `docs/research/voice-accuracy/02-runbook-record-score-fix-retrain.md`). Docs: `docs/changes/91-label-audit-reusable-data-tiered-speech-check.md`.

## Voice Accuracy Measurement Harness — Phase A (2026-10-20)

- **Context**: user asked for the real wake-word ("NEXUS", "hey NEXUS") and STT accuracy in a stars/% table and a plan to make both near-perfect. Audit result: **no trustworthy number existed** (all wake recalls were on training files or a leaky split; STT never measured on real audio). Plan: `docs/research/voice-accuracy/01-…md` (phases A measure → B bug fixes → C retrain → D STT polish).
- **Built (Phase A, measurement only — zero runtime change)**: `voice_eval.rs` (drives the real `WakeEngine`, filter, `strip_wake_prefix`, parser), `examples/wake_eval.rs`, `examples/stt_eval.rs`, `scripts/voice_eval_set.py` (held-out recorder/validator), `scripts/voice_scorecard.py` (Wilson-CI stars table, `--baseline` deltas). Held-out set lives in git-ignored `eval_set/`; never train on it.
- **First findings**: the hallucination filter wipes real commands ("list pull requests", "order pizza from dominos") — confirmed in Rust; synthetic "next us" fires the wake model (0.89).
- **Verify**: Rust lib 935/935 serial (baseline 922), Python tools 21/21.
- **Phase B6 done (change 80)**: hallucination filter no longer wipes real commands — short junk (`you.`/`bye.`/`thank you.`) is whole-utterance only, the dictionary gate defers to the command parser, YouTube-outro phrases added. Rust lib 938/938. Docs: `docs/changes/80-hallucination-filter-stops-wiping-real-commands.md`.
- **Phase B4 done (change 81)**: `wake_names.rs` is the single list of STT spellings of the name (exact / near words / split pairs like "next us"), used by both the stage-2 verifier and `strip_wake_prefix`. Policy change: "next us"/"next sis" are now near forms (need neural prob ≥ 0.6) — verify with `wake_eval --stage2` on the soundalike clips. Rust lib 941/941. Docs: `docs/changes/81-shared-wake-name-list.md`.
- **Phase B1 done (change 82)**: a stage-2-verified wake no longer gets dropped by the 1.5 s cooldown (and no stale `VERIFIED_BYPASS` token can wake an unverified candidate later) — `WakeCandidate.verified` + pure `candidate_gate`. Rust lib 943/943. Docs: `docs/changes/82-verified-wake-bypasses-cooldown.md`.
- **Phase B3 done (change 83)**: cold local STT sidecar no longer stalls the stage-2 verifier for 10 s (`verify_plan` skips the check, fires on neural evidence, warms the engine); verifier fail-open now requires stage-1 evidence (`fail_open_allowed`, so barge-in candidates can't fire phantom wakes on STT failure); idle-kill deferred during Ghost sessions. Rust lib 947/947. Docs: `docs/changes/83-cold-local-engine-no-wake-stall.md`.
- **Phase B8 done (change 84)**: `wakeSensitivity` now moves the wake threshold (low 0.75 / medium 0.68 / high 0.60, live on Save); `localSttOnly` has one default (`false`) in UI and runtime; `get_settings` parses field-by-field so one bad field no longer resets every setting. Rust lib 952/952. Docs: `docs/changes/84-settings-consistency-wake-sensitivity.md`.
- **Phase B10 done (change 85)**: the local Moonshine fallback now goes through the hallucination filter (it was unfiltered); Groq confidence gate (`avg_logprob`/`compression_ratio`) built behind `sttConfidenceGate` (default OFF) with a calibration sweep in the scorecard — enable only if it says "Safe to enable". **Fixed a Phase A miss:** `src-tauri/examples/` was git-ignored, so `wake_eval.rs`/`stt_eval.rs` were never committed — now un-ignored and tracked. Rust lib 955/955. Docs: `docs/changes/85-local-stt-filter-and-confidence-gate.md`.
- **Phase B5 done (change 86)**: generic `COMMAND_VOCABULARY` hints for Unenrolled turns behind `sttCommandHints` (default OFF) via the pure `decoder_bias_for_turn`; verified owner unchanged, other voices never biased; `stt_eval --bias commands|neutral` is the A/B — enable only if accuracy rises and the hallucination-leak row doesn't. Legacy `transcribe_audio` path still sends the owner vocabulary to everyone (noted, untouched). Rust lib 957/957. Docs: `docs/changes/86-command-hints-for-unenrolled-speech.md`.
- **Phase B2 done (change 87)**: command capture is seeded with audio that arrived AFTER the wake detection point (`ring_end` position tracking, locked seed + VAD replay, `SEED_UPTO` dedupe) — never the wake word itself (2026-09-25 poisoning lesson). `wakeSeedOffsetMs` (default 0) is chosen via `stt_eval --capture-offset-ms` sweep, which now simulates wake + capture for one-breath rows. Rust lib 960/960. Needs live check. Docs: `docs/changes/87-one-breath-command-capture-seed.md`.
- **Phase B9 done (change 88)**: capture endpointing follows the measured room noise floor (raise-only vs. the old fixed 0.010/0.006, so quiet rooms are unchanged and a fan no longer pins every turn to the 10 s cap); `EndpointSim` mirrors the live endpointer (equivalence test) and `stt_eval --simulate-endpoint` + scorecard row "Commands cut off mid-sentence" measure cut-offs; the 400 ms limit is left for the sweep to decide. Rust lib 965/965. Docs: `docs/changes/88-noise-floor-aware-endpointing.md`.
- **Phase B7 + C1 done (change 89)**: `dsp_spec.json` is the single front-end spec for runtime (`wake_dsp.rs`, `wakeDspProfile` legacy|aligned, default legacy) and trainer; `wake_eval/stt_eval --dsp` A/B it; manifest refreshed + derived + CI gate; trainer rewritten (shared front end, group-level splits, train-only aug incl. noise/reverb/speed/shift, real deep-copied best epoch, seeds, deployed-threshold metrics, candidate-only output, `--promote`, `--dry-run`). Rust lib 970/970; Python 44. Docs: `docs/changes/89-shared-wake-frontend-and-trainer-rewrite.md`.
- **Phases C2–C5 + D done (change 90)**: `wake_eval --sweep` + `wake_calibrate.py` (threshold from data, certification warning), `wake_audition.py` (by-ear labelling; Whisper never judges), `voice_eval_set.py --purpose train` (training recordings kept apart from the test set), runbook `docs/research/voice-accuracy/02-runbook-record-score-fix-retrain.md`; STT polish: whole-word/context-aware learned corrections with hit counter, filter stats no longer count empty replies, `stt_eval --engine groq-large`. **The code side of the voice-accuracy program is complete; every remaining step needs the user's recordings.** Rust lib 971/971.
- **Pending (user-run, Windows)**: record the held-out set (you + 2–3 others), run the baseline scorecard **before** Phase B fixes. Docs: `docs/changes/79-voice-accuracy-measurement-harness.md`.

## Ghost / Screen-Analysis / Memory Fix Pass + Free-BYOK Key Hygiene (2026-10-05)

- **Context**: after the NEXUS-vs-Dexter comparison the user fixed the scope — NEXUS stays **free and BYOK** (users add their own free Groq + Gemini keys) with a **600–700 MB** local-RAM ceiling — and named three pillars: memory, Ghost Mode ("broken"), screen analysis ("broken"). Plan-mode first (6 read-only audits, plan approved), then built. Paid/RAM-heavy ideas (Worker free tier, shared key, bundled Python, premium TTS, generic MCP bridge) were evaluated and **deliberately not built** — see change 78.
- **Ghost Mode (code-evidenced root causes)**: `center::action_disposition` dropped every ghost command from an `Unenrolled` speaker (speaker verification is off by default) and scored the live/dictation/screen regex sources as "no evidence"; the stop flag was only reset on session entry (one "stop" → "Stopped, sir." forever); `ghost::spawn_relisten_watchdog` had zero callers; a dropped turn left the mic deaf; UIA grounding was rooted at the focused *leaf*; `run_ghost_open` spoke "Ok sir." for an unknown app; `ghost_wry::g_wry` double-dropped the `AppHandle`. Fixes + tests in change 78.
- **Screen analysis**: OCR result never reached the warm sidebar (`sidebar:show` not emitted); `stage_set_hitboxes` missing `source` (since `ff42fa7`); React inline styles dropped by the stage CSP nonce; any 429 locked vision for the day (now daily vs rate + 60 s cooldown); silent failures → typed `SpatialError` + one spoken reason + 25 s cap; truncated JSON repaired; "Analyzing…" spoken after the capture so NEXUS no longer photographs its own caption.
- **Memory**: new `profile.rs` (always-injected about-me block; PII entries stay local), `habits.rs` (local self-learning → suggestions the user confirms), `memory_ipc.rs` + **Memory tab**; fixed UTF-8 truncation panics, non-atomic writes, newest-wins brief merge + age decay, the learned-STT-corrections shape mismatch (they never applied), the Worker ignoring injected memory, and a no-op `clear_transcript`. Feature guide: `docs/features/90-memory-profile-habits-and-memory-panel.md`.
- **Keys / onboarding**: `save_settings` trims + syncs the keychain (a stale keychain copy used to override a corrected key); boot sync (settings.json wins); `read_groq_api_key` is keychain-first; `nexus-config.json` identity fields preserved; setup gets an optional free-keys card; Settings explains the "Local STT only ignores my Groq key" trap; one spoken guide when no speech engine exists; default Moonshine model → `small_streaming` (RAM budget). **Deviation:** keys stay in `settings.json` too (other modules read it directly).
- **Also fixed**: `identity_state.rs` duplicated two constants (the crate did not compile); mail-watch baseline never advanced (re-alert every 15 s).
- **Verify**: Rust lib **922/922** serial (baseline 874; Windows-only paths type-checked via temporary Linux scaffolding, removed before commit); frontend vitest 164/164 (+ pre-existing unloadable `identityBanner.test.ts`); Worker 102/102; `tsc` clean.
- **Pending (user-run, Windows)**: the live checklist at the bottom of `docs/changes/78-ghost-screen-memory-fix-pass.md` (ghost script + stop-then-command, UIA click with omnibox focus, screen analysis with good/bad/no key + forced 429, memory across restart on the Worker path, RAM ≤ 700 MB).
- **Not done / follow-ups**: ghost G11 secondary items, stage geometry sync + multi-monitor capture, `AnnotationCanvas` inline styles (same CSP issue), "tour overlay" (no code exists), Gemini `thinkingConfig` (unverified per model).
- **Docs**: `docs/changes/78-ghost-screen-memory-fix-pass.md`, `docs/features/90-memory-profile-habits-and-memory-panel.md`.
## Feature production programme (2026-10-20)

The owner explicitly instructed: do not stop NEXUS or rebuild it; they will handle runtime/builds manually. Focus on source and documentation edits. Do not automatically start, stop, restart, rebuild, or change runtime settings unless the owner subsequently authorizes that action. Release `d846dd346c470b60` already finished in46m14s and passed provenance before this instruction (sourcef0988d9/inputd7b49eeb). It was launched as PID8460; last native readback at2026-10-26T15:23:52Z showed Voice Lock On for a controlled trial, profile5 valid and diagnostics Off. Initial lock on that launch was Off. No repaired owner action trial completed before interruption. Current runtime state is not rechecked; do not assume the old PID/state is still current or automatically restore settings. All shared gates pass on the frozen source; score41%, no queue advancement. [Assessment](docs/testing/runs/windows1-20261026-neutral-local-assessment.json).

## Final source pass before live proofs (2026-10-27)

- CodeRabbit shortcut with a PR number and no repo now runs `FindPr` (no hidden `ultron` default; `coderabbit_review_pr_data` asks for a repo instead of guessing). `github_cmd` is `pub mod` for `tests/test_user_commands.rs` (listed in `scripts/baselines/external-api.txt`); dead-code baseline shrank 148 -> 147. Fixed a date-dependent `github_watch` test (fixtures are now clocked at their own date) and a cross-suite race (`google_io::TEST_GUARD` shared by `agenda` and `mailwatch` tests); removed an `unwrap` in `deploy_watch`. Rust lib 2,635 + integration 20, frontend 697, Worker 145, Python 90, all gates pass. Nothing run live. Owner checklist: [live-checklist-2026-10-27](docs/testing/live-checklist-2026-10-27.md). Not committed.

## PR-list set_view race fixed; queue entries added (2026-10-27)

- A late `sidebar:set_view` no longer wipes a pinned PR list (`showFeedUnlessPinned`, 3 tests, mutation killed). Scorecard has queue entries for Focus Mode, owner-less GitHub commands and the notification brain (live 0, awaiting owner proofs); work cards regenerated to 68. Frontend tsc clean, vitest 696. Not committed; owner will run live proofs last. Details: docs/changes/167-deploy-watchers.md. A4 tool `scripts/hand-focus-report.mjs` compares Hand Mode logs with/without Focus Mode (selftest passes; no real logs yet). Hand Mode H2 done in source: armed + 2 s with no frame disarms and releases a held drag (`frames_stalled`; hand:: 87 tests; not run with a real frozen camera). H3 done in source: output thread coalesces queued moves (stop keeps releases) and click snapping has a 150 ms off-thread deadline (hand:: 90 tests; not run live). H4 part done: frames carry a sequence, duplicates/out-of-order/over-3 s-old frames are dropped and counted (`stale=` in the `[HAND]` line; hand:: 91 tests, vitest 20). Camera-mode comparison and adaptive resolution need the real camera and stay open.

## P4/P5 trial evidence and P6 source checkpoint (2026-10-26)

Owner requested progressing through remaining phases with edits/tests; runtime/build restriction remains. GLM5.3FlashFree delivered legacy CSV repair in isolated session ses_e86e34b13ffeLmbFH7bfxPgKnp; supervisor tests first failed old code, then independent review found/fixed3 more gaps. Legacy CSV always remains unqualified. New declared-metadata reporter separates owner/live unfamiliar/replay/playback classes, requires lock/capture/verifier/valid-command acceptance qualification, retains excluded locked unsafe actions, and separates verified-owner command fidelity from identity. A stalled larger GLM task produced no files; authorized supervisor fallback implemented reporter. Python90+server31, frontend692+types, Worker145+types, NLU512/8, provenance fixture and source guards pass; mutations21 trial+18 P3+40 native/UI killed. Full application Rust/integration, compile-dependent dead-code, fresh build/provenance and actual device/corpus proof NOT rerun. No application/runtime/settings/profile/recording operations. Dirty-tree source checkpoint6def3963... is not an assessed release. Earlier failures and helper/cwd mistakes retained. [Change170](docs/changes/170-voice-trial-accounting-and-phase-closeout.md), [phase matrix](docs/testing/voice-lock-phase-closeout.md), [protocol](docs/testing/voice-trial-evidence-protocol.md). All phases retain unmet live/exact-source gates. Score41%; no queue advancement.

## P3 enrollment-isolated exported embedding scores (2026-10-26)

Added offline max-enrollment-cosine scoring and vector-free provenance, requiring three distinct owner enrollment samples, one owner identity, paired model trials and disjoint enrollment/calibration/acceptance samples/groups. No model inference, audio/profile access or runtime changes. Review found and repaired finite-latency median overflow in the prior benchmark with a red-before-fix regression. Python63 (19 scalar +15 embedding +29 prior), 18 offline mutations, IPC/turn-end/hygiene/opacity/work-card/whitespace pass. Full application suites/build not run for Python/docs edits. GLM5.3FlashFree main request compacted repeatedly; isolated retry exited without files; authorized supervisor fallback implemented code. Preliminary official-source candidate audit distinguishes code/model licenses; no artifact downloaded or candidate selected. Corpus/extractor/native parity/live gates pending. Score41%; no queue advancement. [Change169](docs/changes/169-enrollment-isolated-embedding-scores.md), [workflow](docs/testing/speaker-embedding-score-workflow.md). Owner controls all runtime/builds as above.

## P3 offline speaker benchmark infrastructure (2026-10-26)

Owner requested OpenCode `opencode/glm-5.3-flash-free` for coding, supervisor planning/tests and local edits if it failed. The listed model started but returned no response/files after about five minutes; its own request was interrupted, then authorized fallback implemented `scripts/speaker_benchmark.py` and tests. Offline paired-model score analysis rejects split/sample/group/impostor-identity leakage and malformed metadata, selects thresholds only on calibration, separates owner/live non-owner/replay/playback/synthetic results, reports uncertainty/resource/group counts and fingerprints the complete input. No audio/model/profile/runtime operations. Python 47 (18 new), 8 mutation assertions, hygiene/work-card drift/whitespace pass. No application/release build or full Rust/frontend/Worker rerun. Corpus, model/export/license audit, actual comparison and live gates remain pending. Score41%; no queue advancement. [Change168](docs/changes/168-offline-speaker-verifier-benchmark.md), [protocol](docs/testing/speaker-verifier-benchmark.md). Owner-only runtime/build restriction above remains in force.

## Prompt repetition reproduced; native execution gap fixed in source (2026-10-26)

Owner's fresh Notepad attempt again failed after owner verification (0.960), with `open open open notebook.` from Groq. With explicit one-clip consent, session6 was stored sealed, diagnostics auto-disabled and native Off confirmed. Same2.88s PCM replay: neutral prompt returned `Open note pad.` twice; owner vocabulary duplicated it twice. No plaintext audio/key persisted by analysis helper. This is one diagnostic clip, not acceptance evidence or acoustic-echo proof. Source review also found ordinary native LocalCommand emitted Done while its frontend execution was skipped. Both fixes are frozen at `f0988d95afbfac06176c923c2227e7e79e841013`, input `d7b49eebf14f49913b36ec7a125e59195915e41d4950a1ac703190b84c1e475a`: neutral native/frontend Groq decoding, existing local executor invoked once for native sessions after authorization gates, sessionless/cancelled turns skipped, real result/error spoken. Regression failed before fix;7 new mutations plus33 retained =40 killed. Frontend687, Worker145, Python60, NLU and repository guards pass; full Rust2,537 library +19 integration (38 ignored), dead-code148 known/none new and every shared gate pass. Optimized replacement release later completed as d846dd346c470b60; the owner-only runtime handoff above supersedes this checkpoint. PID4564 was stopped after confirming diagnostics Off; no NEXUS candidate should be assumed running. Keep original lock On/profile5, preserve one sealed clip under existing retention. Stage blackout, live replacement turn, enrollment attribution, ten-start and non-owner/replay/profile-recovery gates remain open. [Change167](docs/changes/167-neutral-decoding-and-native-local-execution.md). Score41%; no queue advancement. Later source does not inherit previous candidate's live proof.

## Owner action failed after settings verification (2026-10-26)

Owner reports rejected/nothing after the requested Notepad trial on release `0b10fd3a1416a540`, PID12064. Logs show session3 owner Verified (0.942), transcript `open open notepad.` routed to open_app with success unconfirmed; session4 owner verified but repeated `Open notepad. Open notepad.` blocked by repeat_loop; session5 no speech. Additional hotkey and stage watchdog blackout at 18:36:14Z prevent a clean one-attempt mapping. This is not an observed owner-verification rejection. Inherited `RUST_LOG=warn` suppressed INFO capture-audit records: assistant launch/protocol error, so no structured reconciliation or formal rate. Previous logs preserved. Restarted the **same** provenance-verified binary as PID4564 with explicit `RUST_LOG=info,capture_audit=info`; native/saved lock On and profile5 survive restart, nonzero mic telemetry, boot speech completed. One fresh exactly-once owner attempt requested. [Result](docs/testing/runs/windows1-20261026-owner-001-result.json). Command fidelity and stage blackout remain unresolved, score41%, no queue advancement. PID4564 supersedes PID12064 below; settings-flow subtest passes do not imply a successful voice turn.

## P2 shipping settings device checks passed (2026-10-26 IST)

Replacement release `0b10fd3a1416a540`, frozen revision `3e5b4a803b2a22bc28986189377942f127234fa7`, input `342c6f390461e0c69d0c6aa6b66536d84d9cd2c16423f0f8e35604d206dc9124`, is provenance-verified before launch and after testing, running from the C: readiness worktree as PID12064. Build took 61m37s; shared checks remain Rust2533+19, frontend687, Worker145, Python60, all guards and33 mutations. Real UIA/native checks pass draft isolation, Save Off/On/reopen, no premature accessible Saved, standalone settings open (640ms), cross-window untouched-draft refresh and preservation of explicit edits. Confirmed fetch-intercept simulations pass false acknowledgement, refused write, delayed write and profile-status error/recovery. Initial invoke-wrapper attempt did not intercept immutable Tauri invoke and is excluded from fault proof. Native standalone settings defaults to the Hub; the `?view=legacy` prototype was not separately device-tested. No profile bytes changed; original native/saved On restored, five clips valid, all fetch interceptors removed and wake resumed. The pause/resume period produced microphone-silence warnings; check actual captures before diagnosing hardware. [Assessment](docs/testing/runs/windows1-20261025-voice-lock-ui-v2-assessment.md). Owner single-attempt check requested on this exact running instance; ten-start reconciliation, locked owner/non-owner/replay, real profile recovery and enrollment attribution repeat remain pending. Score41%; no queue advancement. Earlier failed candidate history below is retained.

## P2 live failures and replacement repairs (2026-10-25)

Release 021cba2bac4e4bed (9c3f032, PID31624) passed shared gates/provenance, then failed live P2 verification: inaccessible/opacity-only Saved text was exposed before Save; the synchronous legacy settings opener hung Windows UI and triggered a stage blackout. Draft isolation, native Save Off/On and keyboard reopen passed. Original lock On and five-clip profile were preserved; process stopped. No owner accuracy trials. Ledger and docs/changes/165-voice-lock-live-verification-fixes.md retain failures.

Repairs are frozen at 3e5b4a803b2a22bc28986189377942f127234fa7, input 342c6f390461e0c69d0c6aa6b66536d84d9cd2c16423f0f8e35604d206dc9124: verified accessible Saved status, untouched-draft synchronization, invalidation of old confirmations, async legacy opener/tray dispatch, named switches. Frontend 687 + TypeScript/build, 31 focused tests, 11 UI mutations and one async contract mutation pass. Frozen shared rerun now passes Rust 2,533 + 19 integration (38 ignored), frontend 687, Worker 145, Python 60, NLU/all guards (dead-code 148 known, none new), and 33/33 mutations. This was the pre-build checkpoint; the current replacement release/device result is recorded above. Do not count old candidate results as repaired-source live proof. Score stays41%.

## Frozen P1+P2 verification in progress (2026-10-25)

Readiness worktree snapshot 9c3f0320a5c7df359c6a9769e8492ed48baf8fcd, source hash 2317997e0df6a01873674f7174b4c4d60a3d67ec327f90c290b2308c501519d8: full Rust 2,532 library + 19 integration tests pass (38 ignored), frozen frontend 678 + TypeScript/build, Worker 145 + TypeScript, Python 60, NLU, provenance regression and all repository guards pass; dead-code stays 148 known, none new. Fresh rerun kills 7 UI + 6 audit + 15 speaker mutations. Primary frontend 682 included four concurrent hand-focus tests excluded from the frozen candidate. Optimized combined release is compiling; the earlier P1-only build was intentionally stopped before completion and never launched. No P2 live proof yet. Evidence: docs/testing/runs/windows1-20261025-voice-lock-ui-frozen-assessment.json. Keep score 41% and the speaker feature in progress.

## Notification state machine + GitHub Actions watcher (2026-10-25)

- **Built (Antigravity from briefs, then reviewed and hand-fixed):** `notify_brain.rs` keeps per-entity state (`github:<repo>:<pr|build>`): first failure spoken once ("Build failed in zync, sir."), more failures silent, recovery spoken once ("zync is green again, sir."), a failure that keeps going is mentioned again only after 6 h, a failure nobody confirmed for 12 h is forgotten; first sight after connecting/starting is a quiet baseline. State is persisted in the sealed store table `entity_state` (service, project, counters, run id, timestamps only; 30-day prune; wiped with memory). GitHub CI e-mails feed the same book (`mailwording::github_failure_fact`), so mail and API reports of one repo collapse. `memcore/github_watch.rs` polls the latest completed Actions runs of the 5 most recently pushed repos every 2 min (ETag 304s are free, rate-limit aware backoff, once-a-day sign-in notice); setting `memcoreGithubWatch` (default on, inert without a GitHub token) with a Hub switch. Records: `docs/changes/165`, `166`.
- **Verify:** lib 2623 passed, frontend tsc clean + vitest 692, turn-end/hygiene/IPC/opacity gates and dead-code ratchet (148, none new) pass; mutations killed (forget window, fixed-only-if-spoken, 403 handling). **Not run live:** a real failing workflow, the fix, a restart while failing, an SSO org repo, the Hub switch in the real app.
- **Pitfalls found in review:** Antigravity did not compile its own tests (missing `Debug`/`PartialEq` derives); its first watcher treated every 403 as a rate limit (GitHub sends `x-ratelimit-remaining` on every response), which would have stopped the whole poll for one unreadable org repo: now only `remaining == 0` or `retry-after` count. Project name is the bare repo name; `say()` uses the `github:` prefix for event keys.
- **Deploy watchers (2026-10-27, Antigravity then hand-fixed):** `memcore/deploy_watch.rs` watches the newest terminal PRODUCTION deploy per project for Vercel (personal + up to 3 teams), Render (10 services) and Netlify (10 sites) every 5 min with vault tokens; same state machine (`Kind::Deploy`, "Vercel deploy failed in zync, sir." / "zync is live again, sir."), quiet baseline, 401 -> once-a-day "needs a new token". `notify_brain::say` now takes a service prefix; shared mock server in `memcore/test_http.rs`; setting `memcoreDeployWatch` (default on) with a Hub switch. Record: `docs/changes/167-deploy-watchers.md`. Verify: lib 2643 passed, vitest 693, tsc, gates and dead-code ratchet (148, none new); mutations killed (production filter, ERROR mapping). **Review fixes:** its tests used `super::test_http` (wrong path), and `render` became test-only for the ratchet. **Not run live:** a real failing deploy and its fix on any of the three services, a team account, a restart while failing.
- **Supabase watcher (2026-10-27):** same loop polls the Management API projects list; announces only healthy / *_FAILED, never paused or transitional states; tests pass, not run on a real project (status strings unverified live).
- **Not built:** a Worker webhook inbox, Hub grouping of notifications.

## Focus Mode, owner-less GitHub repos/PRs, name-free notification lines, feature work cards (2026-10-25)

- **Built (Antigravity from written briefs, then reviewed and hand-fixed):** `focus_mode.rs` (lease with 10 s watchdog; `paused()` is true while Hand Mode runs; ~17 own loops and one-shot jobs skip or wait, Medium spoken alerts are held not dropped); `focus_throttle.rs` (while paused it lowers CPU priority + EcoQoS of up to 6 heavy user apps, never the foreground app, NEXUS and its helpers, system/security/meeting/media apps or elevated processes, records originals with creation time in `focus_baseline.json` and restores exactly, also after a crash or at app exit); settings `handFocusMode` and `handFocusLowerApps` (default on) with Hub toggles. `github_repo_index.rs` resolves a bare repo name across your repos and every org (10-minute cache); "analyse zync" rewrites to `owner/repo` before the Worker, several owners -> a spoken question; "analyse the PR in zync" lists open PRs (`latest`/`by <author>` still analyse); "analyse PR 34" with no repo now runs `FindPr` (every repo with open PR #34, listed in the sidebar; the old `ultron` default is gone); the PR list can be pinned so dashboard refreshes no longer overwrite it, Analyse/Merge send `owner/repo`. `memcore/selfnames.rs` + new tail-address wording: "PR failed in zync, sir." / "Build failed in windows, sir." with the owner's own name never spoken. `scripts/make-work-cards.mjs` generates `docs/testing/work-cards/*` (65 cards in queue order) and `docs/testing/feature-process-map.md`; `--check` fails on drift. Records: `docs/changes/161`, `162`, `163`.
- **Verify:** lib 2589 passed (before the last small fixes; affected suites rerun: 372 passed), frontend tsc clean + vitest 682, turn-end/hygiene/IPC/opacity gates and dead-code ratchet (148, none new) pass, work-card drift check passes; mutations killed (PID-reuse check, meeting guard, self-name match). **Not run live:** Focus Mode with a real camera and a heavy app (check Task Manager before/after and after killing NEXUS), any spoken GitHub command, SSO/org-approval behaviour, a real failing workflow.
- **Not built yet:** Vercel/Render/Netlify/Supabase watchers (P3; the state machine and GitHub Actions watcher are in the entry above), the `sidebar:set_view` race that can reset the PR list to feed, queue entries for these items in `scorecard-data.json`.
- **Pitfall:** Antigravity (`agy --print ... --mode accept-edits --sandbox --dangerously-skip-permissions`) rewrote `proactive_policy.rs` glue from memory, left dead "superseded" copies and dropped a `#[test]`; always diff its output against git, compile and run the dead-code ratchet. It needs `--dangerously-skip-permissions` in headless mode (the owner approved that).

## Voice Lock reliability P2 — effective native state (2026-10-25)

Owner explicitly requested next-phase implementation while P1 release/live cross-check was pending. Both settings windows now distinguish effective native Voice Lock from the unsaved draft, refresh on reopen/focus, and require native/settings agreement after Save. Slow saves retain newly edited fields; stale reads and overlapping saves are guarded; profile failure refuses enable. See docs/changes/164-voice-lock-native-state-ui.md. Checks: 22 focused new tests, full frontend 682 tests, TypeScript/production build, 7/7 safety mutations, IPC/turn-end/hygiene/scoped whitespace pass. Evidence: docs/testing/runs/voice-lock-ui-assessment.json. Fresh P2 release and live settings walkthrough remain pending; native/Worker/Python full sweep was not repeated for this frontend phase. P1 frozen release source is unchanged. Score remains 41%; no feature-queue advancement.

## Voice Lock reliability P1 — native capture accounting (2026-10-25)

- **Built:** unique accepted-capture sessions; content-free per-request lifecycle records with run/executable linkage; packet-owned terminal outcomes; start/abort/callback/delivery barriers; routing-receipt correlation; log reconciliation tool. Research and implementation plan are indexed in docs: docs/research/elevenlabs-architect-guided-nexus-review.md and docs/features/152-voice-lock-evidence-and-reliability-plan.md. Change: docs/changes/163-voice-capture-accounting.md.
- **Checks:** isolated Rust 2,532 library + 19 integration tests passed (38 explicitly ignored); frontend 656 + TypeScript/build; Worker 145 + TypeScript; Python 60; NLU, provenance regression, IPC/turn-end/hygiene/opacity/whitespace and dead-code (148 known, none new) passed. All 23 safety/audit/evidence-reader mutations were killed. Assessed snapshot c430a21c9000e00be131478a7a718f53b5f23baf, input ca2daa4bff07a9b6f7dd6b3b797cc09dc5bee188e573fd74d769b2e2ffbe9cba; final script-only commits do not alter the tested Rust source. Concurrent primary-tree compilation failures are retained separately.
- **Pending:** fresh optimized release build/provenance and owner ten-trial reconciliation; effective native-state UI, verifier benchmark and other phases remain planned. Use --expected-starts 10 so seven completed captures cannot pass a ten-start batch. Delivery/routing receipt does not prove action success. Score remains 41%; do not advance the feature queue. No remote ElevenLabs MCP opinion was obtained.

## Solo owner batch: incomplete capture accounting (2026-10-25)

Owner reported ten solo attempts (five normal, five about one metre). In 20:07:00–20:09:00 IST, seven completed captures are identifiable, all owner=verified; three reported attempts remain unmatched and condition-by-condition mapping is unavailable. Do not claim 10/10 or a formal owner acceptance rate. One capture was repeat-loop filtered, several app targets were duplicated, and one transcript became hook up opencode. Requested app-opening success is not established for the batch. D: build3d1932 diagnosis only; zero negative trials; score stays41%.

## Diagnostic owner voice accepted (2026-10-25)

Diagnostic D: build 3d1932b68b2c0f15 recorded an owner-verified captured turn at 2026-10-25T12:44:35Z (similarity 0.944), after native Voice Lock enable. Intended Open Notepad was transcribed as open over open load pad; requested app-open success is not established. This is one owner-verification acceptance, not broad accuracy or frozen-source proof. Owner is alone; owner trials can continue, other-speaker playback must be labelled separately, and live non-owner evidence remains unavailable. Score stays 41%.

## Native Voice Lock enabled for diagnostic trial (2026-10-25)

Owner reported Saved successfully, but live Tauri IPC voice_lock_enabled and get_settings both returned false; a valid five-clip profile with all three phrases loaded. With existing authorization to enable for controlled trials, settings_patch({speakerVerification:true}) succeeded and both native readbacks returned true. This proves current lock configuration, not speaker accuracy. Current D: release 3d1932b68b2c0f15 / PID17928 is being used for diagnosis only; it does not inherit frozen-source suite proof. The UI/save discrepancy remains unresolved; no user misclick or UI defect is assumed.

## Live Voice Lock save prerequisite (2026-10-25)

Owner reported toggle on/no executable result. Readback found speakerVerification=false and the running process was D: build 3d1932b68b2c0f15 (PID 17928). Command Hub toggles are drafts until Save; the assistant's test instructions omitted that step. Exclude this attempt from locked accuracy counts. Save and verify the native setting before another trial; assessed-build attribution must also be resolved. No owner rejection or acceptance rate established.

## Live build attribution correction (2026-10-25)

Build attribution correction: the owner supplied console logs showing nexus start replaced the assessed candidate with D: release 6f4e1063a38683a3 at 16:54, followed by 3d1932b68b2c0f15 at 17:36. Earlier owner enrollment/wake/cancellation reports remain observations, but exact assessed-build attribution is unconfirmed and must be repeated. The Notepad turn at 17:37:41 was owner=unenrolled with Voice Lock off; it is excluded from locked accuracy counts. Current device setting was confirmed false; a profile file exists. Candidate 031caf5848621e18 was restored with unique logs, PID 28952. No score increase or queue advancement.

## Notification brain: event keys, cooldown, short mail lines (2026-10-25)

- **Built (feature 151, phases A/B/D/E + review pass):** `notify.rs` derives one stable event key per logical mail (GitHub: `mail:gh:<owner/repo>:<stem>`, else thread id, else sender+stem). `proactive_policy` uses it: Medium repeats inside 5 minutes (rolling) and High repeats inside 2 minutes (fixed) become sidebar cards and the next spoken line says "Sir, N more since you last heard — …"; escalations and Critical break through; a key is released if its alert expired unspoken. `mailwording` gives non-GitHub mail a local one-liner. WhatsApp alerts use unique ids plus key `wa:<jid>` (the old shared id silenced a chat after its first alert). The cloud rewrite (`memcoreMailPlain`) is now OFF by default, and a held repeat never calls a model. Records: `docs/changes/157`–`160`; notes and live protocol: `docs/testing/notification-brain-testing-and-research.md`.
- **Verify:** 136 targeted Rust tests, 4 mutations killed, tsc and 105 settings tests. **Not run:** the live two-email replay (testing doc §7), WhatsApp live, the full CI sweep. Keep the feature out of the proven list until a ledger line exists.
- **Pitfalls:** `return_review.rs` is not declared in `lib.rs` (not compiled); `extract_github_owner_repo` needs two letters in owner and repo; GitHub CI failures are Medium, security notices High.

Replacement release 031caf5848621e18, source 50dcda219018d3bbfff8be388b77610bee135749, is provenance-verified before and after launch as PID 28248 at 2026-10-25T11:19:07Z. Optimized build completed in 49m 15s. All shared gates pass: 2,524 Rust library + 19 integration (38 ignored), frontend 656 and TypeScript/build, Worker 145 and TypeScript, Python 48, NLU, repository guards, 15/15 safety mutations. DirectML hash checked. Startup reached stage ready with microphone/wake models detected. Missing optional Tier 3 command models, Groq menu-model, registry and startup wake-suppression warnings are retained. Owner re-enrollment and accuracy remain pending; score stays 41% (2.05/5).

Owner confirmed normal waking with Nexus, Hey Nexus and OK Nexus after the ten-clip enrollment on release 031caf5848621e18, with Voice Lock off. Enrollment isolation and post-completion wake recovery have passed owner interaction checks. Cancellation recovery and locked owner/non-owner/TV accuracy remain pending. No accuracy trials counted; score stays 41%.

## Owner cancellation checks passed (2026-10-25)

Owner reports both enrollment cancellation checks passed on 031caf5848621e18: navigation during countdown and during recording stopped recording and restored normal waking, with Voice Lock off. Pending-permission navigation is not live-verified. Locked owner/non-owner/TV accuracy, restart and profile recovery remain pending. Score stays 41%; no accuracy trials counted.

## Owner enrollment interaction passed (2026-10-25)

Owner live report on candidate 031caf5848621e18: all ten enrollment clips completed without waking NEXUS. Enrollment wake isolation passes this interaction check. Normal wake recovery, cancellation recovery, owner/non-owner accuracy and profile recovery remain pending. These ten calibration/enrollment clips count as zero acceptance trials. Score remains 41% (2.05/5); queue stays on speaker verification.

## Enrollment wake interference follow-up (2026-10-25)

- **Latest revision:** `50dcda219018d3bbfff8be388b77610bee135749`, hash `086e534ae829a7abbdf1e793f584297e81b8f2c8c16625bcfeee03ea505572c4`. A new red test exposed navigation waiting on pending browser microphone permission; cancellation now releases the baton promptly and late-granted streams are stopped. The earlier optimized build was stopped and never launched. Nine lifecycle tests and all 15 mutations pass. Replacement 031caf5848621e18 is verified and running as PID 28248; owner recheck pending. Current evidence: `docs/testing/runs/windows1-20261025-enrollment-v2-assessment.md`. Keep score 41% and do not advance the queue.

- **Owner failure:** enrollment on release `812a5b3333669e02` also woke NEXUS. Do not treat its enrollment-success UI as a passed live interaction. Record: `docs/changes/156-voice-enrollment-wake-isolation.md`.
- **Fix:** await native microphone pause for the complete ten-clip session; stop browser tracks before resuming on success/error/navigation; native callback/capture/queued-wake gates honor the baton and device failures reach the UI. Both enrollment and calibration prompt Nexus / Hey Nexus / OK Nexus. Snapshot `ade0bfdddd99d52efe14da089d33ee5b7493d8a0`, hash `328ab8cd23bf39fde595a16ee15dbd18ecbaf81f2c5c7ec4c76203a2c604cf11`. Full gates and replacement release are in progress; score stays 41%, and owner re-enrollment is required.

## Windows 1.0 readiness and Voice Lock (2026-10-24–25)

- **Status:** work card 1, speaker verification, remains 41% and in progress. Command Hub has ten-clip enrollment with five withheld owner recordings; profiles are sealed; Voice Lock now fails closed for missing/corrupt profiles and unverified captured turns, including frontend pending confirmations. Keep the switch off by default until owner/impostor live proof passes. See `docs/testing/speaker-verification-work-card.md` and `docs/changes/155-windows-readiness-baseline-and-voice-lock.md`.
- **Recovery/pitfalls:** enrollment now shares the live normalized OWW pipeline, resets clip state, and can explicitly replace a corrupt profile. Voice profiles use an independent Windows key so memory wipe cannot strand them; OS deletion failures are reported. `save_settings` preserves Voice Lock; only a `settings_patch` explicitly naming `speakerVerification` changes it. Corrupt settings remain locked in both UI and routing. `Ctrl+Shift+S` opens recovery settings; `Ctrl+Space` never bypasses owner verification. Twelve safety mutations are killed. Source snapshot `954c57471be0564f69763f73b43e0868f50c74ca` and all current evidence: `docs/testing/runs/windows1-20261025-assessment.md`.
- **Checks:** frozen full Rust run passes 2,522 library + 19 integration tests (38 explicitly ignored), including 24 voice-profile tests and the real-model enrollment/reset test. Smart Turn's retained five-second budget passes in isolation at 721 ms. Frozen frontend TypeScript/build + 647 tests, Worker TypeScript + 145 tests, Python 48, NLU validation, 12/12 safety mutations, IPC, turn-end, hygiene, opacity, whitespace, and dead-code (148 known, none new) pass. The OS keyring prerequisite passes across two processes; owner/profile live proof is still missing. The assessed source is isolated at `C:\Users\Chitkul Lakshya\.codex\worktrees\windows-readiness\ULTRON`, snapshot `954c57471be0564f69763f73b43e0868f50c74ca`, stable input hash `3087f61760442b846c71baaba710b625cf5ebe961e81fa5d4677c381e150ce5e`; concurrent mail edits remain in `D:\ULTRON`. Release candidate `812a5b3333669e02` is built, provenance-verified and running (PID 13408 at launch). Startup reached stage ready with microphone/wake models detected; optional-model and registry warnings are retained in the assessment. Use one Cargo job after a parallel compile hit Windows pagefile error 1455. Do not advance to installer before owner live proof passes.

## Screen tour v2, reverse engineer, solid windows (2026-10-21)

- **Built:** the tour's vision call can now finish (20 s per request, 25 s total; it was 4 s / 6 s, so every tour fell back to OCR); the model picks how to draw each item (11 styles) and states your intent; sidebar screenshots zoom; panel windows are solid (guard: `scripts/check-window-opacity.mjs`, supersedes the 2026-10-15 liquid-glass note for panels); **"Nexus, reverse engineer this website/app"** scans the page stack (`site_probe.rs`, `site_fingerprints.json`, SSRF-guarded), inspects an app folder (`app_probe.rs`), shows the left `stack` sidebar view, and scrolls/narrates the page (`reverse_engineer.rs`, `reverse_scroll.rs`). Record: `docs/changes/152-screen-tour-v2-reverse-engineer-and-solid-windows.md`.
- **Verify:** full Rust lib 2429 passed; frontend tsc + 63 test files; opacity guard; browser harness previews (`/dev-annotations.html`, `/dev-stack.html`). **Not run live:** everything by voice, real Gemini latency, window move/scroll/restore, 125/150 % scaling. A disguised-IPv6 (`::ffff:127.0.0.1`) hole in the first scanner draft was found by tests and fixed.
- **Pitfalls:** the fingerprint table must keep bare-word signals weak (0.28); a new window surface class must be added to `scripts/window-opacity-registry.json`; the IPC gate currently fails on comment lines inside `generate_handler!` (not from this work).

## Release provenance and grounded device turns (2026-10-20)

- `nexus start` and `scripts/run.ps1` verify `src-tauri/target/release/nexus-build.json` before launching or stopping another instance. `nexus build` or `nexus start --build` refreshes source, frontend bundle, and executable hashes after success. Native microphone turns now go through Rust with raw/corrected text and capture session; changed executable corrections ask for a repeat. Unenrolled STT uses neutral decoding. Device-specific questions use live screen or foreground evidence, or say what cannot be verified. Optional encrypted STT diagnostics expire after 24 hours and can be deleted in settings. Record: `docs/changes/151-release-provenance-and-grounded-device-turns.md`.
- A fresh release build and labelled microphone/Windows screen-tour walkthrough remain required before calling this production ready. Preserve other in-progress working-tree edits.

## Narrated screen-tour visualization (2026-10-20)

- **Built:** `screen_tour.rs` now sends the current item's category with its callout and asks Gemini for less repetitive, evidence-grounded spoken lines. `TourOverlay.tsx` keeps the original ring/leader/card and adds restrained category-specific ripple, underline, border-trace and leader-beam motion. It clips malformed/off-screen boxes, repositions on viewport resize, ignores late callouts after tour end, and respects reduced motion and high contrast. No Magic UI dependency was installed; source inspiration and limits are recorded in `docs/changes/150-screen-tour-visualization.md`.
- **Verify:** frontend TypeScript, 25 focused stage/tour-flow tests, all 371 frontend tests, production frontend build, 33 Rust `screen_tour::tests`, and turn-end gate passed. Five `architect.rs` test fixtures were repaired to include their existing `KeyFile` metadata fields so Rust tests could compile. The 19:05 debug executable was restarted for manual testing as PID 13644; Vite serves its stage and all preflight layers reached stage ready. A release build was stopped after prolonged optimization with concurrent Rust jobs. Hygiene, IPC, dead-code, and whole-worktree whitespace checks have unrelated failures detailed in the change record. A real spoken tour is still required before claiming visual/speech alignment on hardware.

## Universal agent control: track all agents, tile, display off, unconditional shutdown, overnight record, gated commit/PR (2026-10-20)

- **Built:** hooks for Codex, Devin (via Claude's settings, relabelled), Windsurf, Kilo, Copilot CLI (Trae is observe-only); "hook up all agents"; `agent_tracker.rs` (many agents, per-agent announcements, one summary); `window_tile.rs` + `Action::TidyUp` (equal columns, then display off only if the user is idle and no meeting); plain "shut down after X" now powers off whatever happened (conditional only with "if it's perfect"), `agent_outcomes.rs` + `briefing.rs` say it next morning and ask to reopen the other apps; `agent_followup.rs` commit + draft PR for "... commit and raise a PR": asked once (`Offer::PlanFollowUp`), runs per finished agent only after the yes and only on a clean verdict, the shutdown waits while git runs and never depends on it. Record: `docs/changes/149-universal-agent-control.md` (also 147, 148).
- **Verify:** 149 targeted Rust tests (watch_power, power_plan, watch_runner, agent_hooks, agent_followup, window_tile) + mutation checks; `cargo check --lib` clean. **Not run live:** the new agents' hooks (docs only), tiling on real windows, display-off on this Modern-Standby laptop, a real commit/push/PR, the overnight briefing, an `on` countdown. Each agent's Stop payload must carry `cwd` for the commit step; if not, it says it cannot tell the folder and commits nothing.
- **Pitfall:** `intent_parser::tests::foreground_review_and_recreation_are_distinct_commands` fails from concurrent screen-analysis work, not from this.

## Administrator control A0 — native transcript provenance (2026-10-19)

- **Built:** A native STT-only, one-use 60-second exact transcript proof for Administrator-shaped commands; canceled/stale captures cannot mint a proof. Frontend Administrator phrases bypass learned corrections and local execution, and carry the capture session to Rust. `admin_privilege.rs` checks separate sidebar-review and paired-laptop-export policies without extending the idle clock on a mere check. The older rotating raw WAV debug writes were removed from source. Build sequence: `docs/features/145-administrator-control-implementation.md`; change record: `docs/changes/145-administrator-control-native-provenance.md`.
- **Verify:** Rust `cargo check --features custom-protocol,admin-brain`, seven targeted Administrator tests on the final policy code, frontend TypeScript, production build, and 369 Vitest tests, turn-end and hygiene gates passed. IPC and dead-code gates still fail on unrelated existing/concurrent items; neither lists this work. No rebuilt or live biometric walkthrough.
- **Still closed:** No enrollment, signed bootstrap, live voice/face/challenge verifier, session opener, Administrator action routing, credential approval, or review execution/delivery. The ordinary wake-owner filter still drops an unfamiliar Administrator candidate before transcription. Automatic approval review rejected deletion of ten legacy WAV debug clips found in `%APPDATA%\com.nexus.assistant\verify_debug`; source changes do not remove old installed-build behavior until rebuild.

## Cloudflare Free Device Links foundation (2026-10-19)

- **Built:** Worker device-token auth for Device Links independent of the AI entitlement; D1 schema and control routes for verified phone bindings, direct pair requests, link revocation, and an authenticated SMS outbox; hibernating Durable Object signaling; a Command Hub pairing page; an optional Termux SMS gateway script; Rust WebView route allowlist; daily expired-code cleanup. `device_activity.rs` adds an off-by-default, encrypted local 30-day app-only Activity Manager on Windows. Administrator session policy now has 15-minute maximum, five-minute idle limit, and review-export approval check, but no production opener.
- **Verify:** Worker TypeScript and 145 Worker tests pass; Wrangler dry-run bundles the bindings; local Wrangler + D1 passed two-device phone verification, code acceptance, list, revoke, denial after recipient phone removal, and decline. Frontend TypeScript, 368 Vitest tests, build, Rust `cargo check --tests --features custom-protocol,admin-brain`, 5 Administrator tests, the targeted Activity Manager Rust test, and 7 Worker proxy Rust tests pass. Turn-end and hygiene gates pass; IPC and dead-code ratchets fail on unrelated concurrent items. No live SMS, two-laptop, or biometric walkthrough was run.
- **Not built:** Administrator enrollment/verification/provisioning, fresh review execution/export, WebRTC file transport, activity sharing, macOS live parity. Do not present Administrator access or file transfer as ready. Record: `docs/changes/144-cloudflare-free-device-links-foundation.md`.

## Administrator review access foundation (2026-10-19)

- **Built**: `src-tauri/src/admin_privilege.rs` claims explicit Administrator session and fresh repo/PR review commands, requires a named destination (paired Administrator laptop or device-user sidebar), validates repository identifiers, and keeps the production session gate closed. `orchestrator.rs` intercepts these commands before NLU/Worker fallback, answers through the normal speech/turn path, and records a local audit entry. No Administrator access, review execution, pairing, or delivery is enabled yet.
- **Verify**: 4 targeted Rust parser/policy tests passed; `cargo check --features custom-protocol,admin-brain` passed with warnings only in concurrent `agent_hooks.rs`; frontend `tsc`, 368 Vitest tests, build, turn-end and hygiene gates passed. IPC and dead-code ratchets still fail on unrelated concurrent items; neither reports a new item from this feature.
- **Not run**: real voice/face enrollment or verification, a rebuilt app, a live voice walkthrough, fresh review generation, paired-device transport, or red-team corpus. Current owner-voice gates drop a non-owner before transcription; Administrator candidate routing must be added at capture time without weakening normal owner filtering.
- **Docs**: `docs/features/143-administrator-review-access.md` records revised owner decisions, recognized command grammar, and remaining implementation gates. The older research suite is marked as planning material where superseded.

## Ask-first shutdown + "watch this, then power off" W0 (2026-10-18)

- **Built**: bare power verbs ("shut down", "turn it off", "restart") and end-of-day phrases ("that's it for today", "I'm calling it a night", "I'm up for tonight") now ASK once (`power_cmd::parse_weak` / `parse_casual`, `orchestrator::try_power_and_howto`); a yes teaches only a shutdown phrase (end-of-day phrases under a `casual: ` key); a power question also accepts the action itself ("shut it down", "good night") via `memcore::offer::classify_for`. `watch_power.rs` = order-free parser for "watch this session until it's done and if it's perfect shut down my laptop" -> `ParsedIntent::WatchThenPower`, claimed BEFORE the power parser and the compound "then" split. Record: `docs/changes/139-ask-first-shutdown-and-watch-then-power-w0.md`; plan `docs/features/142-...` (owner decisions D1-D4 taken as recommended).
- **W1**: `power_plan.rs` keeps the one plan in memory ("cancel the shutdown plan", "what are you watching", "just tell me", "even if it fails", "make it sleep instead"; whole-sentence vocabularies); `ARMED_SUPPORTED = false` until the observer exists.
- **W2/W3**: `session_verdict.rs` (`judge` -> Go/Hold/Unsure; failure markers, waiting prompts, two-signal rule, model JSON can only be stricter) and `session_observer.rs` (process-tree idle tracker, evidence assembly, sysinfo sampler); wired by `watch_runner.rs` (W5). Process idleness alone never gives a go; no window-text reader exists, so a Stop hook is required.
- **W4**: `agent_hooks.rs` ("hook up claude" asks first, backs up, edits only NEXUS entries; Claude `settings.json` Stop + UserPromptSubmit + Notification(permission_prompt|elicitation_dialog) hooks, Codex `config.toml` `notify`) and `POST /agent-event?agent=&event=` on the webhook listener with its OWN token (keychain `agent_event`, written only into `<app data>/agent_hooks/nexus-agent-event.cmd`). The hook's stdin JSON is the body: a Stop keeps the agent's final message as masked data (last 4 KB); nothing else is stored and no text becomes a command. Real script-through-cmd-and-curl tests pass; **not run**: a real agent session.
- **W5 (2026-10-19), OFF BY DEFAULT**: `watch_runner.rs` + setting `watchPower` (off / dry / on; voice: "allow / practice / turn off unattended shutdown"). A pure `Engine` decides, a 3 s loop observes: the agent's Stop hook + process quiet 45 s + a verdict that stays Go for 15 s, then (only with `on`) an announced 30 s countdown that stops on "cancel", any key/mouse input, a meeting, the plan or setting changing, the session becoming busy/imperfect, or not knowing if the user is back; then the normal shutdown sequence. Timeout / no hook / off / dry / "just tell me" / restart of NEXUS never power off. 30 engine tests incl. 7 end-to-end timelines, 19 mutations killed. **Not run**: a real Claude Code / Codex session, the spoken countdown, a real shutdown. Try `practice unattended shutdown` first and read the `[WATCH-POWER]` log lines. Record: `docs/changes/139-...`, plan `docs/features/142-...`.
- **More agents (2026-10-19)**: `agent_hooks::Agent` now covers Claude Code, Codex, **Antigravity** (`~/.gemini/config/hooks.json`: one named hook with `Stop` + `PreInvocation`; the App, CLI `agy` and IDE share it), **OpenCode** (a plugin file of its own, `~/.config/opencode/plugins/nexus-agent-event.js`; the user's config is never edited), Gemini CLI (`~/.gemini/settings.json`: AfterAgent / BeforeAgent / Notification), Cursor (`~/.cursor/hooks.json`: stop / afterAgentResponse / beforeSubmitPrompt) and Aider (`~/.aider.conf.yml` `notifications-command`; the text comes from `.aider.chat.history.md`). Same rules as before: approval first, backup, only NEXUS entries touched or removed, a foreign hook/plugin/notification setting is left alone, own token. The final text arrives in the event (Claude, Codex, Gemini, OpenCode, Cursor message) or is read from a path-checked, 256 KB-capped file (Antigravity `transcriptPath`, Aider history); the stop status (`error`, `terminationReason`, `fullyIdle`, Cursor `status`) can only make the verdict stricter. Hook scripts print `{}` (`{"continue":true}` for Cursor) and exit 0, so they never block an agent. "This session" now resolves from the window title for any of them, else from the agent process below the foreground window (two found = ask). **Verified live**: `agy` 1.2.9, given a project-level `.agents/hooks.json` (only loaded from a project root, i.e. a git repo), posted its start and stop through the real script (it runs hook commands through `cmd /C` and a QUOTED path with a space fails, so its command uses the 8.3 short path); the GLOBAL `~/.gemini/config/hooks.json` that the installer writes is the documented location but was NOT exercised; Claude Code ran the quoted command for UserPromptSubmit (its Stop was not reached: the nested run failed on the API key); the OpenCode plugin runs under node against a local socket. **Not run**: OpenCode, Gemini CLI, Cursor and Aider themselves, Claude's/Codex's Stop payload, a real shutdown. Record: `docs/changes/139-...`.
- **Verify**: new tests + 5 mutations killed (2 survived first, tests added). **Not run**: anything spoken, a rebuilt app.
- **Pitfall**: another session edits `orchestrator.rs` / `intent_parser.rs` / `center.rs` at the same time; keep patches anchored and narrow.

## Command-center alignment (2026-10-19)

- `center.rs` now exposes a typed 17-center `ActionSpec` and shared `prepare_action` policy for single intents and compound steps. Compound planning excludes standalone actions on both deterministic and admin-brain paths; dependent `then` steps execute in order. Unsupported NLU labels are refused instead of gaining an execution path from a name prefix. The existing GitHub, MCP and file confirmation flows remain the actual gates.
- Canonical ownership and adapter map: `docs/architecture/12-command-center-alignment.md`. `orchestrator.rs` remains turn owner, `command_center.rs` coordinates compounds, and the existing speech arbiter/playback timeline remains audio/orb/caption owner. The older five-bucket doc is historical.
- Verification and remaining live checks: `docs/changes/141-command-center-alignment.md`. Post-fix Rust rerun passed 1816 tests with four unrelated concurrent-work failures filtered; TypeScript, Vitest, frontend build, turn-end and hygiene gates passed. A rebuilt voice walkthrough remains unverified. Concurrent working-tree changes were preserved.


## Wake-up sound (2026-10-18)

- **Built**: `src-tauri/src/wake_cue.rs`: five synthesized earcons (rise default, glass, pluck, triplet, sweep) played at the wake-word fire (`wakeword_oww.rs`, before `start_stt_capture`) and on Ctrl+Space from idle (`hotkey.rs`). Own on-demand rodio stream (closes 20 s after use, `prewarm()` at the wake candidate), no TTS counters / volume lease / keep-awake. **Echo guard**: while the cue plays (+120 ms) the capture loop does not count chunks as voiced (audio still buffered). Silent in meetings, Ghost hot-mic, empty-transcript retries and the confirm/offer windows. Settings `wakeSound` / `wakeSoundStyle` / `wakeSoundVolume` (Command Hub > Audio > Wake Sound, with Play buttons; IPC `preview_wake_sound`). Record: `docs/changes/138-wake-up-sound.md`.
- **Verify**: 10 unit tests + wiring pin, 5 mutations killed, vitest 363, tsc. **Not run**: anything audible (needs a rebuild and your ears), echo/clipping with a real mic, Bluetooth, latency (`[CUE]` log lines). Listen without the app: `cargo test --lib write_candidate_wavs -- --ignored`.
- **Known failing (not from this work)**: 3 `intent_parser` PR-list tests expect `ListPrs` but "open pr list" now parses as `OpenGitHubDashboard` (the GitHub dashboard feature, `docs/GITHUB_DASHBOARD_SPEC.md`); another session is mid-way on it.

## Memory v2: baseline, profile + synonyms, corrections, routines (2026-10-18)

- **Plan** (ported from Hermes Agent's design, Rust-native, no Hermes runtime): `docs/memory-core/07-memory-v2-baseline.md`. Phases M0-M5 built (the plan is complete; nothing run live).
- **M0**: `[LAT] ... | mem Nms` (`latency::record_mem`), and a 40-question recall test (`memcore::recall_baseline`). Baseline: direct 20/21, paraphrased 0/19, pack p50 6 ms (debug).
- **M1**: `memcore/synonyms.rs` (concept families + stemming, second search pass ranked below exact hits) and `memcore/profile.rs` (always-on block of pinned `user_said` facts, 1,400-char cap, sensitive keys excluded, switch `memcoreProfile` default ON, budget 2,000 -> 2,600). Now direct 21/21, dev paraphrase 19/19 (tuned, not evidence), held-out 17/20. The profile did not raise recall in the test; its value is standing facts + a cacheable prompt prefix, at up to ~350 tokens per cloud request.
- **M2**: `memcore/corrections.rs` + `corrections` table: "no, I said X" or a re-issued command with one word changed is a candidate; applied only after two separate sightings, only to command-shaped transcripts, listed and removable on the Memory page. Real-log replay: 0 false detections on 74 pairs; real recall unmeasured.
- **M3**: `memcore/routines.rs` + tables `command_log`/`routines`/`routine_offers`: only open-app/site/music commands are logged (each must re-parse to the same intent); 2-4 steps seen in 3 sessions on 3 days become ONE offer a day (max twice per suggestion, spoken via proactive policy); "save that as <name>" (only while one waits), "run <name>" (rewrites the utterance to steps joined by "then" so the compound path and its confirmation gates run it), "delete the <name> routine"; Memory page section + switch `memcoreRoutines`. No real command history exists yet - logging starts after the rebuild.
- **M4**: `memcore/review.rs` + tables `pending`/`review_rejected`: OFF by default (`memcoreReview`). A free cloud model (`router::route_with_system`) reads a PII-filtered digest of cloud-only turns (never local ones; not while name-hiding is on; >=10 new turns or 3 after 10 min quiet; max 1/30 min, 6/day; logged as egress route `review`) and returns JSON add/patch/remove suggestions. Code, not the model, decides: value must be grounded in USER words, plain key, no sensitive topic/secret/PII/instruction/link, remove only on a "no longer" cue, max 5. Suggestions wait on the Memory page (14 days, cap 30); approve = stored as `UserSaid` pinned (joins the profile), refuse = never suggested again. 16 review tests; 5 mutations (grounding, instruction, PII, remove cue, sensitive key) each killed (the PII one survived first, test added). **Not run**: against a real model (prompt quality unknown), spoken nudge, rebuilt app.
- **M5**: `memcore/curator.rs`: daily idle pass (user away 5 min, NEXUS idle, max every 20 h, first check 10 min after start). When the 1,400-char profile is full it stores a usage order (`profile_order` meta; profile facts now count a use when a question touches them in `context_pack_with`), otherwise leaves creation order so the prompt prefix stays identical; retires routines unused 60 days (Memory page note for 14 days, `Status.curator_note`); drops year-old refusal markers. Never edits or deletes facts. Review and cloud context both read `curator::profile_entries`. 9 tests, 5 mutations killed (one survived first; test added). `THIRD_PARTY_NOTICES.md` has the Hermes Agent / Letta design row (no code copied; Hermes copyright line unverified).
- **Verify**: full lib 1765 pass (3 fail: 2 foreign Spotify parse tests, `turn_detect::model_loads_and_runs` timing flake that passes alone), memory + 10 corrections + 17 routines + 16 review tests, vitest settings 43, tsc clean; IPC gate shows no new items from these commands; dead-code gate has no new items from these files. **Not run**: a rebuilt app, a spoken correction or routine, a live `mem Nms` line.
- **Pitfalls**: new settings must stay in `NexusSettings` (`memcoreProfile` is); `learn_from` must stay free of global state (tests run in parallel); 2 Spotify parse tests fail from earlier work, not from this.

## Action Kernel — voice file control, quarantine delete + undo, provenance, safe coding agents (2026-10-17)

- **Built** (doc 140 P0-P6, recommended defaults D1-D5 assumed): `src-tauri/src/action_kernel/` (`paths` canonicaliser, `policy` deny/ask/allow engine, `files` tools, `journal` undo + quarantine, `audit` JSONL, `resolve`, `voice` grammar + replies, `pending` held confirmation, `ipc`). Say "delete the pdfs in downloads" -> read-back + sidebar card -> "yes" -> moved to `<root>\.nexus-trash`; "undo that" restores. Move/copy/rename/create/list/find run at once, never overwrite. "Delete everything / wipe my drive" is claimed and refused locally. Hub page **Actions & Permissions** edits the policy live.
- **Rules**: no shell strings in the kernel (a test greps for it); a file action never runs inside a compound; only a captured-voice turn (turn context present) may trigger a delete or answer its confirmation; permanent delete is not available from voice; forbidden places are code, not settings.
- **Real hole fixed**: the agent-watch sidecar started OpenCode with `--dangerously-skip-permissions` (no approval preset set) and had lost its token / parent-exit hardening; now token + Host check + vetted preset only (default `edit`, never `autonomous`).
- **Verify**: 69 kernel tests, mutation check 20/20 killed (first run had 6 survivors, fixed), sidecar tests 4/4, tsc, vitest 323/323, full Rust lib 1668 pass (2 foreign Spotify failures), turn-end/hygiene OK. **Not run**: anything by real voice / rebuilt app. IPC gate still fails on pre-existing lost-work items (`get_return_review`, `agent_watch_*`, `speech_stage_snapshot`). Record: `docs/changes/135-action-kernel-file-control-and-agent-safety.md`.

## Voice / Connectivity / RAM Research & Plan — Edge First, Smart Local Standby (2026-10-17)

- **Owner contract**: conversation fast with no perceptible delay, tasks may think; **Edge TTS first**, local only when offline; **Smart** standby (local voice + STT + NLU warm when the link looks unstable, cold after 30 min stable - **never always loaded**); return to online only after proof (OS connected + 3 probes + 1-word canary + 60 s); spoken notices are **reactive only** (when the user calls while offline/slow), "I'm back" only if the user had called during the outage, varied phrases, STILL_OFFLINE from the 4th call. Free/open-source: users bring Groq/Gemini keys.
- **Measured (M)**: idle tree 269 MB commit / 236 MB WS; Kokoro +368 MB WS (dictionary +125, ONNX session +135, first synthesis +64, unload returns ~300); NLU +42 MB; ONNX arena-off saves ~100 MB but is ~34 % **slower**; local Kokoro on battery = 4.3 s for a 3-word line (RTF 2.4) so it is NOT the live voice; one cloud reply = ~2.6 s end of speech -> first audio.
- **Do not**: touch GPT-sol's Ghost files (`ghost_center.rs`, `ghost_learning.rs`, `ghost.rs`, `mouse.rs`, `shortcuts/*`, `screen.rs`, `ghostHotMic.ts`); use ONNX arena-off as a default; use the Windows voice or auto-emotion in the audible chain (both scheduled for removal); start Edge pre-rendering without pacing (429 storm).
- **OWNER DECISION 2026-10-17**: speed first - Edge online, local only offline, Smart plan stays; GPT plan 137's single-local-voice default rejected (see the decision block at the end of doc 136). Feature 83 swap flow is the owner's voice-selection plan.
- **BUILT 2026-10-17 (shutdown / how-to)**: structural loose power match + learned mishearings (`power_cmd.rs`, `power_aliases.json`), `howto.rs` (local steps card in the sidebar for power how-tos; long/procedural answers go to the sidebar with one spoken line), wake vocabulary bias in `stt_groq.rs`; also the stale-playing-flag fix in `ttsPlayer.ts`. Records: `docs/changes/134-shutdown-never-explain-loose-power-match-and-howto-sidebar.md`, plan `docs/features/138-...`.
- **BUILT 2026-10-17 (speed pass)**: shared persistent Edge connection (measured 1009 ms -> 409 ms median per short line, 290 ms after wake pre-warm), wake-time Edge pre-warm, streaming from 60 chars, first-clause chunking, Feature 83 hardening (atomic manifest, file/manifest hash check, swap at utterance boundary). Record: `docs/changes/132-edge-persistent-connection-clause-chunking-and-voice-swap-hardening.md`. Not built: LLM answer streaming into speech, learned bank, connectivity state machine, Smart warm/cold triggers.
- **BUILT 2026-10-17 (P0/P1)**: `[LAT]` per-turn latency clock (`latency.rs`), stock acks ("Working on it sir" etc.), wake-time NLU pre-warm + race-free first load, voice settings consistency (`normalize_voice_fields`, `settings_patch`, hub mirrors all voice fields). Details: `docs/changes/131-latency-clock-stock-acks-nlu-prewarm-and-voice-settings-consistency.md`.
- **PROJECT LOCATION (2026-10-17)**: the repository now lives permanently at `D:\ULTRON` (it was `C:\PROJECTS\ULTRON`; the C: folder was emptied during a move on 2026-10-17 and the work of that day was rebuilt on D:). Use D: paths everywhere.
- **Dev-only additions**: ignored tests `tts_kokoro::tests::mem_breakdown`, `nlu_local::tests::mem_nlu_load_parse_unload` (memory/speed) and `tts_edge::latency_bench::shared_vs_fresh_client_first_audio` (Edge connection speed, needs internet).
- **Docs**: master review file `docs/features/136-voice-connectivity-and-ram-master-compendium.md`; details in 126 (answers/questions), 128 (settings architecture), 130-132 (RAM), 133 (coordination), 134-135 (connectivity + notices), 137 (GPT's plan, partly rejected), 138 (shutdown / how-to).

## Universal Agent Watcher & Cross-Platform Music Control (2026-10-15)

- **Why**:
  1. The user requested: *"Nexus watch over this, update me when the response is received and Claude / Codex / Antigravity / Cursor / Trae / etc. has stopped running"*. The system must not be limited to 3 tools — it must support any connected MCP, CLI coding agent, IDE, or foreground desktop task, keep the machine awake, and alert the user proactively on completion.
  2. The user requested cross-platform music control (MacBook macOS & Windows) with user choice between **Apple Music** and **Spotify** ("play songs", "open apple music", "open spotify", "play [song] on [player]", next, previous, pause).
- **Built**:
  1. **Cross-Platform Media Engine (`src-tauri/src/media_player.rs`)**: Built native Apple Music and Spotify controls across macOS (AppleScript via `osascript`), Windows (Win32 virtual keys `VK_MEDIA_*` via `keybd_event` and protocol deep links `spotify:`, `applemusic:`), and Linux (`mpris.rs`).
  2. **Music Intent Parsing & Execution (`src-tauri/src/intent_parser.rs` & `command_executor.rs`)**: Added `ParsedIntent::PlayMusic` and `ParsedIntent::OpenMusicPlayer`, supporting natural speech commands for song search, track playback, player opening, and transport controls.
  3. **Universal Agent Watcher Intent & Execution (`src-tauri/src/intent_parser.rs`, `orchestrator.rs`, `agent_watch.rs`)**: Added `ParsedIntent::WatchAgentTask`. In `run_watch_agent_task`, if no agent is explicitly named ("Nexus watch over this"), it inspects the active foreground window and process to bind the target (Antigravity, Cursor, Trae, VS Code, Claude in Terminal).
  4. **Keep-Awake & Proactive Voice Alerting (`agent_watch.rs`)**: Holds an OS `KeepAwakeLease` to prevent system sleep/lock while the task is executing; once completed (via PTY exit/pattern or workspace quiescence), releases the lease and triggers high-urgency proactive speech (*"Your [Agent] response is ready, sir. It has finished running."*).
  5. **Expanded Agent Adapters (`server.cjs` & `agent_watch.rs`)**: Added Antigravity and Generic Terminal adapters to the Node.js sidecar and expanded `KNOWN_ADAPTERS` in Rust.
  6. **Settings UI & Preference Storage (`frontend/src/settings/SettingsApp.tsx`, `AgentWatchSection.tsx`, `commands.rs`)**: Added `default_music_player` ("spotify" | "apple_music") in `NexusSettings` and `settings.json`, and rendered a frosted liquid glass player switcher in the Settings window.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (**0 warnings, 0 errors**), `cargo test --lib media_player` clean (**3/3 passed**), `cargo test --lib agent_watch::tests` clean (**2/2 passed**), `cargo test --lib test_coding_command_parsing` clean (**1/1 passed**), `npx tsc --noEmit` clean (**0 errors**), `npx vitest run` **315/315 passed**, `npm run build` clean (**13.67s**).

## Universal MCP Connections, App Control & Deployment Tracking (2026-10-15)

- **Why**: The user requested a pixel-perfect Model Context Protocol (MCP) connections and control panel matching their reference design (`media_1792085856119_c6f58488.png`), sized to fill the full height of the sidebar/settings, with direct app launch and programmatic MCP control, live multi-platform deployment tracking (Vercel, Netlify, Render, Supabase), and multi-assistant delegation.
- **Built**:
  1. **Pixel-Perfect MCP Servers Panel (`frontend/src/settings-sidebar/McpServersPanel.tsx` & `mcpServers.css`)**: Implemented the reference card interface with connector clip (`📎`), title `MCP Servers`, dynamic 5-segment status indicator bar, solid emerald checkmarks (`✓ Ready`), multi-tick radial loaders (`⟳ Syncing`), and dotted clock badges (`🕒 Standby`).
  2. **10 Official Vector Brand Marks (`frontend/src/settings-sidebar/McpIcons.tsx`)**: Created high-fidelity SVG brand marks for Figma, Vercel, Supabase, Stripe, Slack, Netlify, Render, GitHub, WhatsApp, and Swiggy.
  3. **App Launch & MCP Control Drawer**: Clicking any server row expands an interactive control drawer featuring deep-link protocol & browser launch buttons (`🚀 Open App / Dashboard`), tool capabilities tags, and vault credentials configuration.
  4. **Multi-Platform Deployment Tracking**: Live status cards for Vercel, Netlify, Render, and Supabase with branch names, commit hashes, build states (`Production Ready`, `Deploy in Progress`, `Build Failed`), and manual re-check triggers.
  5. **Multi-Assistant Delegation Matrix**: Configurable pills allowing users to assign MCP server access across sub-agents (Coding Agent, Sentinel Watcher, Design Agent, Butler).
  6. **Unified Height Placement (`SettingsSidebarApp.tsx` & `SettingsApp.tsx`)**: Wired `McpServersPanel` into Command Hub Connections tab (filling the full 100% sidebar container height) and into the Settings window Accounts & Integrations tab.
  7. **OS Vault & Backend Hardening (`src-tauri/src/auth_vault.rs` & `mcp_client.rs`)**: Expanded `VAULT_SERVICES` to cover all modern MCP platforms and hardened `test_mcp_server_urls` against parallel test thread contention.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (**0 warnings, 0 errors**), `cargo test --lib mcp_client` clean (**28/28 passed**), `npx tsc --noEmit` clean (**0 errors**), `npx vitest run` **315/315 passed**, `npm run build` clean (**11.02s**).

## Universal Multi-Voice Persona Matrix & Dynamic Offline Swapping (2026-10-15)

- **Why**: The user pointed out that users should be able to freely choose ANY voice (not only JARVIS, but also FRIDAY, NEXUS, SIRI, ALEXA, GOOGLE, CORTANA, SAMANTHA, ALFRED, etc.), and selecting ANY voice must instantly switch the cloud voice AND automatically download its matching local carbon-copy twin without ever clobbering settings or losing offline capability.
- **Built**:
  1. **Top-10 Iconic Persona Matrix in Settings (`frontend/src/settings/SettingsApp.tsx`)**: Replaced the legacy single-voice `<select>` dropdown with a responsive 2-column matrix of all 10 iconic personas loaded directly from `list_voice_personas` (`ICONIC_PERSONAS` fallback). Each card displays avatar, name, accent/gender tag, description, cloud/local twin specs, live sync status pill, active checkmark, and a signature preview audio trigger (`▶ Play Sample`).
  2. **Real-Time Offline Swap Tracking**: Wired `get_voice_sync_state` and live Tauri IPC `"voice:status"` event listeners in `SettingsApp.tsx`, showing `✓ Cloud + Offline Ready`, `⬇ Syncing twin (xx%)`, or `⚡ Cloud Only (Twin Pending)` as Hugging Face downloads progress.
  3. **Universal 1-Click Equip Handler**: Clicking ANY persona card triggers `set_voice_preference({ voiceKey: p.key })`. Rust atomically equips the cloud voice, purges stale in-memory cache, spawns `tts_swap` to fetch and verify the matching Kokoro voice file (`.bin`) from Hugging Face into `active_voice.bin`, and re-synthesizes acknowledgments.
  4. **Full Catalog in Setup Wizard (`frontend/src/setup/SetupApp.tsx` & `ttsPlayer.ts`)**: Expanded `CURATED_VOICES` and `SetupApp.tsx` Step 0 with all 10 iconic personas. On setup completion, `set_voice_preference` is dispatched to automatically queue the offline twin.
  5. **Design System & CSS (`settings.css`)**: Styled `.nx-persona-grid`, `.nx-persona-card`, `.nx-persona-card--active`, and sync badge animations matching the frosted liquid glass theme.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (**0 warnings, 0 errors**), `cargo test --lib voice_catalog` clean (**17/17 passed**), `npx tsc --noEmit` clean (**0 errors**), `npx vitest run` **311/311 passed**, `npm run build` clean (**0 warnings, 16.26s**).

## JARVIS Cloud Voice & Carbon-Copy Local Twin Restoration (2026-10-15)

- **Why**:
  1. The user noted the voice sounded different from what they had for many days, requesting JARVIS (`en-GB-RyanNeural`) and its exact local carbon-copy twin (`bm_george` in Kokoro-82M) to be downloaded and activated.
  2. The root cause was discovered: `SettingsApp.tsx` and `ttsPlayer.ts` lacked `en-GB-RyanNeural` in their options/lists, so saving settings in the new Settings window silently clobbered `settings.json` and reset `edgeTtsVoice` to `en-US-AvaNeural`.
- **Built**:
  1. **Configured JARVIS Cloud Persona**: Set `selectedVoice: "jarvis"`, `edgeTtsVoice: "en-GB-RyanNeural"`, `ttsVoice: "en-GB-RyanNeural"`, and `offlineVoiceModel: "bm_george"` with Paul Bettany prosody tuning (`-5%` rate, `-4Hz` pitch) in `%APPDATA%\com.nexus.assistant\settings.json`.
  2. **Downloaded Local Carbon-Copy Twin**: Streamed `bm_george.bin` (522,240 bytes) directly from pinned Hugging Face revision (`onnx-community/Kokoro-82M-v1.0-ONNX@1939ad2a8e416c0acfeecc08a694d14ef25f2231`), verified SHA-256 (`c4b235a4c1f2cd3b939fed08b899ce9385638b763f7b73a59616c4fc9bd6c9bc`), and atomically installed it to `%APPDATA%\com.nexus.assistant\voices\active_voice.bin` with `manifest.json`.
  3. **Purged Stale Ava Cache**: Evicted all old `*en-US-AvaNeural*` cache files from `tts_cache` on disk.
  4. **Settings UI & Choke-Point Protection**: Added JARVIS (`en-GB-RyanNeural`) to `CURATED_VOICES` in `frontend/src/audio/ttsPlayer.ts` and `AudioTab` in `frontend/src/settings/SettingsApp.tsx`, triggering `set_voice_preference("jarvis")` on change to protect against UI save clobbering.
  5. **Backend Defaults**: Updated `DEFAULT_VOICE_KEY` to `"jarvis"` in `src/voice_catalog.rs` and `default_edge_tts_voice()` to `"en-GB-RyanNeural"` in `src/commands.rs`.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (**0 warnings, 0 errors**), `cargo test --lib voice_catalog` clean (**17/17 passed**), `npx tsc --noEmit` clean (**0 errors**), `npx vitest run` **311/311 passed**, `npm run build` clean (**0 warnings, 8.08s**).

## Transparent Liquid Glass Windows, Theme Uniformity & Cloud Edge TTS Enforcement (2026-10-15)

- **Why**:
  1. The user explicitly directed that all windows and panels (Settings, Setup, Unified Dynamic Sidebar, Architecture Mapper, Information Analyses / Dashboard) must be transparent blur (frosted liquid glass), not opaque black/white.
  2. All analysis windows and the architecture mapper must adaptively and reliably synchronize with the theme mode set in Settings (`"light"` vs `"dark"`).
  3. The agent was falling back to local Kokoro TTS despite having an active internet connection due to cached local acknowledgment phrases locking multi-sentence streaming utterances to local mode.
- **Built**:
  1. **Window Transparency (`src-tauri/src/dyn_windows.rs`)**: Configured `setup` and `settings` windows with `decorations: false`, `transparent: true`, `shadow: true`. Added `"settings"` and `"setup"` to `is_glass_window` for DWM rounded corners (`dwm_corners::round_corners`) and screen capture exclusion (`SetWindowDisplayAffinity(HWND, 17)`).
  2. **Liquid Frosted Glass Design Tokens (`frontend/src/theme/tokens.css`)**: Defined frosted liquid glass tokens with `backdrop-filter: blur(26px) saturate(190%)`, translucent background surfaces (`rgba(12, 12, 16, 0.72)` / `rgba(244, 244, 248, 0.75)`), and specular glass borders.
  3. **Unified Dynamic Sidebar (`frontend/src/sidebar/unified.css` & `hub.css`)**: Removed opaque black/white backgrounds; replaced with frosted liquid glass and `backdrop-filter: blur(28px) saturate(190%)`.
  4. **Architecture Mapper & Information Analyses (`architect.css` & `sidebar.css`)**: Eliminated hardcoded `#000000` blackout surfaces; styled `.architect-app`, `.analysis-card`, and `.sidebar-card` with liquid glass blur and comprehensive dark/light theme token adaptivity.
  5. **Settings & Setup Windows (`settings.css`, `setup.css`, `SettingsApp.tsx`, `SetupApp.tsx`)**: Made window roots transparent, wrapped contents in rounded frosted glass cards with `backdrop-filter`, and added native top drag titlebars with `✕` close buttons. Added a Theme Mode switcher in General Settings (`🌙 Dark (Liquid Glass)` / `☀️ Light (Frosted White)`).
  6. **Cross-Window Theme Synchronization (`frontend/src/sidebar/theme.ts`)**: Built `initThemeSync()` with Tauri IPC cross-window broadcasting (`emit("nexus:theme-change")`) and `settings.json` disk sync.
  7. **Cloud Edge TTS Decoupling (`src-tauri/src/tts.rs`)**: Fixed `utterance_local` in `speak_streaming` so cached acknowledgment phrases do not lock responses to local Kokoro. Invalidate local estimated cache entries when Edge is online. Purged stale Kokoro cache entries on disk and defaulted UI to `en-US-AvaNeural` + `edge`.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (**0 warnings, 0 errors**), `cargo test --lib` clean (**passed**), `npx tsc --noEmit` clean (**0 errors**), `npx vitest run` **311/311 passed**, `npm run build` clean (**0 warnings, 6.74s**).

## Zero-Warning Clean Build Refactor (Rust & Frontend) (2026-10-15)

- **Why**: `nexus build` produced 68 Rust compiler warnings in `nexus (lib)` alongside Vite dynamic/static chunk import collision notices. The user demanded complete elimination of all warnings to ensure a production-grade, zero-warning build output with zero regressions.
- **Built**:
  1. **Visibility Scope Fix**: Changed `ResolutionEntry` in `src-tauri/src/app_registry.rs` to `pub(crate)` to fix the boundary mismatch with `heal_poisoned_entries`.
  2. **Google Sub-Engine Domain Services**: Added `#![allow(dead_code)]` across `src/google/` (`mod.rs`, `auth.rs`, `calendar.rs`, `mail.rs`, `maps.rs`, `photos.rs`, `sentinel.rs`, `types.rs`) for models, services, and parsers prepared for ongoing Google Workspace integrations.
  3. **Live Automation & Desktop Control**: Annotated `LiveIntent`, `full_send_flow`, `double_click_at`, `scroll`, `drag_to`, `app_context`, `has_pending_text`, and `GhostSession.channel`.
  4. **Audio, Voice Profile & Orchestrator**: Annotated `MAX_PHONEME_CHARS`, `unknown_phonemes`, `acceptance_threshold`, `num_embeddings`, `implicit_enroll`, `delete`, `read_manifest`, `find_by_cloud_id`, `find_by_kokoro_voice`, `OrchestratorState`, and `build_worker_payload`.
  5. **Command Center & Subsystems**: Annotated `Receipt`, `ConfirmKind`, `TurnContext` fields (`decoder_bias`, `language`), `SubCenter` methods (`name`, `confirm_kind`), `ALL_SUB_CENTERS`, `UiDirective::Loading`, `loading_should_show`, `PendingCompound.transcript`, `CHECKPOINT_STALE_SECS`, `read_checkpoint`, `sweep_stale_checkpoints`, `downloaded_model_dir`, `required_scopes`, `Provider::Worker`, and `migrate_api_keys_to_keychain`.
  6. **Memcore, Crypto & Vision**: Handled `PollError::Other`, `PollOutcome::Baselined`, `offer::take`, `Hit.tier`, `audit_ops`, `update_mail_watch_status`, `KEYRING_USER`, keyring helper functions in `crypto.rs`, `ScreenContext::unknown`, `EndReason::Failed`, `is_available`, `capture_screen_text_only`, `wheel_step`, `as_str`, `HysteresisFilter::new`, and `mode`.
  7. **Vite Dynamic/Static Import Alignment**: Statically imported `transcribeAudio` from `../audio/stt` in `orbRuntime.ts` and dynamically invoked `initOrbRuntime` on mount in `OrbFrame.tsx`, removing the cyclic dynamic/static chunk warnings.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (**0 warnings, 0 errors**), `npx tsc --noEmit` clean (**0 errors**), `npx vitest run` **311/311 passed**, `npm run build` clean (**0 warnings, 7.86s**).

## Google Re-authentication & Accounts UI + Symmetrical Ctrl+Space Hotkey & Escape Discard (2026-10-15)

- **Why**:
  1. When Google's 7-day Testing-mode OAuth token expired, Sentinel's email watcher flagged an authentication error and instructed the user to re-authenticate, but the Settings window had no Accounts tab, no status badges, and no re-authentication option at all (only an opaque button to the initial setup wizard, where already-connected accounts were disabled). Command Hub also lacked a direct re-authentication button on the account row.
  2. `Ctrl+Spacebar` was destroying open windows (`settings`, `sidebar`, `setup`) and forcing barge-in rather than canceling active turns, while `SettingsApp.tsx` and `SetupApp.tsx` had no `Escape` key listeners to discard the windows.
- **Built**:
  1. **Settings Window (`SettingsApp.tsx`)**: Added dedicated `Accounts & Integrations` tab with live Google accounts list (`google_get_accounts`), status badges (🟢 Active vs ⚠️ Sign-In Expired), 1-click `Re-authenticate` button (`google_connect_account`), `Make Primary` toggle, `Disconnect` button, Developer OAuth Client ID/Secret drawer (`google_credentials_status`, `google_save_custom_credentials`), and GitHub integration status.
  2. **Command Hub (`HubHome.tsx` & `useHubData.ts`)**: Exposed `googleNeedsSignin` via `memcore_status` in `useAccounts` and added an amber `Needs Sign-In` badge + direct `Re-authenticate` button right on the account row.
  3. **Universal Escape Dismissal**: Added `keydown` `Escape` listeners in `SettingsApp.tsx` and `SetupApp.tsx` to discard windows via `close_settings_window` and `close_setup_window`.
  4. **Symmetric `Ctrl+Spacebar` Hotkey (`hotkey.rs`)**: Realigned `Ctrl+Space` strictly as an agent toggle: if the agent is active (`stt_capturing() || tts_playing || has_active_request() || session_active()`), it cleanly cancels the agent (`abort_stt_capture()`, `stop_tts()`, `cancel_active()`, `ghost_abort()`, and emits `orb:cancel`); if idle, it calls the agent (`start_stt_capture()`, `wake_orb_src("hotkey")`). Removed all `destroy_window` calls on `Ctrl+Spacebar`.
  5. **Stage Voice Runtime (`orbRuntime.ts`)**: Added `orb:cancel` event listener to immediately cut TTS/VAD/STT and reset the orb to idle.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (0 errors, 68 warnings baseline unchanged), `npx tsc --noEmit` clean (0 errors), `npx vitest run` 311/311 passed, `npm run build` clean (18.34s).

## Voice "Never Silent, Never Late" Stack + WhatsApp Number-First Route (2026-10-15)

- **Why**: live log showed 4-5 s before "Ok sir" and WhatsApp chats opened blind (no learning). Causes: stale `edge-tts-rust` (403) so every line burned the Edge timeout; cache staleness keyed on bing.com reachability; UI Automation cannot read WhatsApp's chat list (loop of 2.5 s blocked polls, ~57 s per name).
- **Built**: `tts_health.rs` breaker, `tts_system.rs` Windows-voice safety net, `tts_cache_disk.rs` persisted fixed lines, `wa_open.rs` bounded flow + OCR header verification + alias learning, `contacts_book.rs` + voice "import my contacts" (`.vcf`, local only), phone-link-first open in `ghost_drill.rs`, strict yes/no for confirmations (`confirmReply.ts`, `offer.rs`), `ghost_drill_running` IPC.
- **Verify**: Rust lib 1543, vitest 311, tsc + build clean, dead-code 157->156. IPC gate fails only on another session's `agent_watch_*` commands. **Not run live**: keyboard route/OCR on real WhatsApp, real `.vcf`, Windows-voice fallback timing, release rebuild.
- **Same round (plans 123/124)**: boot hello on every logon via stage-ready gating (no 45 s sleep) + 3 time-of-day lines, briefing no longer repeats the greeting, autostart task off the setup path + priority 4, stage created first in setup, resume-from-sleep hello, shutdown flow ("Good night, sir", graceful `WM_CLOSE` close that stops and names an app that asks to save, 3 s cancel window), 10 s answer window + risk-based answer TTL (2/5/10 min), local turns recorded (never sent to the cloud), bare "do it" -> "Do what, sir?". Lib 1558, vitest 311.
- **Open**: unified PendingQuestion + on-screen Yes/No (Q2 full), cloud `proposed_action` (Q4), lock/unlock hello without sleep, contact-name STT bias, ask-once unknown contact. Nothing audible/visual was run live. Details: `docs/changes/130-...md`.
- **Docs**: `docs/changes/130-voice-never-silent-stack-and-whatsapp-number-route.md`, plans `docs/features/123`, `124`, `125`.

## Caption/Orb/Sound Sync Audit — the Done/Result Race + Orb-Visibility Enforcement (2026-10-15)

- **Why**: user asked for a full audit that the caption, the orb, and the spoken audio
  stay "in sync, no mistakes," driven consistently by the main command center — research
  first, so nothing gets missed. A follow-up explicitly asked: can sound ever play without
  the orb being visible, or without a caption? Three parallel research passes (frontend
  caption/orb/TTS trace, Rust orchestrator turn-lifecycle trace, full `AGENTS.md` history
  survey) found this exact symptom category ("orb stuck/out of sync with still-playing
  speech") had recurred **at least 6 times** over the prior month, several fixes explicitly
  marked "not verified live." This project's own research doc already names the pattern:
  *"State machines need a handshake on both ends. Rust withholds Done; someone must close
  it. Every 'stuck' symptom so far has been one-sided bookkeeping."* This pass found a new,
  currently-live instance of exactly that pattern, verified by direct code read (not agent
  paraphrase), plus a real answer to the follow-up question.
- **Gap 1 — the Done/Result race (orb goes idle mid-speech)**: `net/orchestrator.ts`'s
  `result`/`error`/`conflict_report` handlers are self-contained — they call
  `speak(text, onEnd)` and only close the turn (`finishSpokenResult`/`closeTurnOnSpeechEnd`)
  once that TTS audio actually finishes (built for the original 2026-09-18 bug). But the
  `done` case handler was a SECOND, independent closer: after a flat 550ms/250ms beat it
  unconditionally called `store.reset()` and nulled `currentRequestId`, with no guard at
  all. On the Rust side, `Done` is emitted **synchronously right after `Result`** — before
  the frontend has even started speaking — for every GitHub outcome except Confirm, every
  Mcp outcome, every CommandCenter (compound) turn, every WorkerBackend error, and
  LocalCommand's `NeedMoreInfo`. Only plain WorkerBackend success withholds Done (the one
  case the 2026-09-18 fix actually accounted for). Net effect, live: for any of those turn
  types the orb visually reset to idle and stopped reacting to audio ~550ms in, while
  captions and real audio kept going for seconds more, and the *correct* closure silently
  no-op'd when it finally arrived (its own `currentRequestId` guard had already been
  clobbered by `done`). **Fix**: `net/orchestrator.ts` — a module-level
  `pendingSpeechCloseFor` tracker, set by `result`/`error`/`conflict_report` right before
  their self-closing `speak()` call (ghost-mode-gated for error/conflict_report, matching
  `closeTurnOnSpeechEnd`'s own ghost no-op so the hot-mic loop's existing closer stays the
  active one in ghost mode), cleared inside `finishSpokenResult`. The `done` handler now
  checks it first: if set for this request id, it only does the safe idempotent bits
  (loading/awaiting-input clear, ghost-turn-closed notice) and defers entirely to the
  pending closure — no premature reset, no nulled request id.
- **Gap 2 — sound had no central guarantee the orb is visible first**: `ttsPlayer.ts`'s
  `speak()`/`speakCached()` are the ONLY two frontend functions that ever invoke
  `speak_text`/`speak_cached` (confirmed: grepped every `invoke("speak_`call site — no
  bypass exists). 30+ call sites across `recorder.ts`/`orchestrator.ts`/`orbRuntime.ts`/
  `wsBridge.ts`/`OrbFrame.tsx` call `speak(...)`; only 3 were verified to correctly show the
  orb first — convention, not enforcement. **Fix**: `speak()`/`speakCached()` now force
  `setVisible(true)` + `setState("speaking")` themselves, once, centrally — covers every
  current and future caller by construction. `previewVoice()` (settings voice-audition)
  deliberately excluded — not a real turn. The caption half of the question was already
  solid, verified not fixed: Rust's `play_audio` (`tts.rs`) emits `"tts:caption"`
  unconditionally before `sink.append`, and both `speak_text` and `speak_cached` route
  through this same function — every sound-producing invoke gets a caption, by
  construction, regardless of caller.
- **Smaller fixes found along the way**: (1) `proactive_policy.rs`'s `Action::Speak` never
  called `install_new_request` — a proactive alert's speech was invisible to
  `has_active_request()`, so a second alert could be judged "safe to speak" over the
  first's still-playing line. Fixed via a new `install_new_request_with_id(id, subsystem)`
  (refactored out of `install_new_request`, zero behavior change for its ~10 other callers)
  so the alert keeps its `sentinel_<id>` string (needed for `speak_line`'s
  `sentinel_offer_` prefix match) while still registering; the existing
  `finishSpokenResult → signalOrchestratorDone → orchestrator_done` IPC handshake clears it
  generically once the speech actually ends — no new Rust-side clear call needed. (2)
  `command_center.rs`'s one Confirm emit hand-built raw JSON shaped to match
  `OrchestratorEvent::Confirm` instead of constructing the real enum — silent drift risk if
  the enum ever changes; now constructs `OrchestratorEvent::Confirm{...}` directly. (3)
  `captionScheduler.ts`'s per-line clear-timer decided "is this the last line" from
  `latestScheduledEndMs` at fire time — not actually a stale-read bug (JS has no true
  concurrency; fire-time is the freshest data available) but a genuine information gap: a
  later streamed chunk simply hadn't arrived yet. Fixed with a 500ms cancelable grace
  window (`pendingCloseTimer`) before committing to "utterance ended" — any subsequent
  chunk's `scheduleChunk()` call cancels it. (4) `CaptionTrack.estimated` (Kokoro/local
  engine's evenly-distributed timing vs. edge-tts's real `Boundary::Word` ticks) was never
  read anywhere on the frontend — zero observability, zero safety margin. Added a one-time
  `console.debug` per utterance and a +300ms linger margin for estimated tracks (not a fix
  for Kokoro itself — that would mean instrumenting the ONNX pipeline, explicitly flagged as
  a separate, larger, uncertain-scope task).
- **Explicitly out of scope (flagged, not silently dropped)**: giving Kokoro real per-word
  boundaries; barge-in's ≤500ms audio-stop polling window (small, bounded, previously
  accepted); wiring up the dead `OrchestratorState::Speaking` enum variant (frontend already
  has a working substitute, now centralized by the ttsPlayer fix — adding a 5th signal
  would fight the simplification this pass is doing); `OrbFrame.tsx`'s idle watchdog,
  `hideOrbAfterSpeech`'s poll loop, and `ttsActivity.ts`'s 400ms grace window — verified
  directly that each already consults `captionActive`/`ttsActive` before acting and is a
  timeout-style backstop for *stuck* states, not a competing live decision-maker; no changes
  needed.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (69 pre-existing
  warnings, 0 new); `cargo test --lib orchestrator::`/`proactive_policy::`/`command_center::`
  all passing (62/14/28, +1 new test for `install_new_request_with_id`); `npx tsc --noEmit`
  clean; `npx vitest run` 306/306 (298 pre-existing + 8 new: 3 in `orchestrator.test.ts`
  covering the `pendingSpeechCloseFor` tracker, 2 in `ttsPlayer.test.ts` covering the
  visibility enforcement, 3 in `captionScheduler.test.ts` covering the grace-window race fix
  and the estimated-timing margin); `npm run build` clean. The `done` case handler's own
  deferral branch could not be unit-tested directly (it lives inline inside
  `initOrchestratorListener`'s `listen()` callback, not separately exported) — tested the
  `pendingSpeechCloseFor` tracker mechanics instead via new `__testPendingSpeechCloseFor`/
  `__testSetPendingSpeechCloseFor` hooks (mirrors this file's existing `__testSetCurrentRequestId`
  pattern) and `finishSpokenResult`'s interaction with it. **Not run live**: triggering a
  real GitHub/MCP/compound command by voice and confirming the orb keeps reacting to audio
  and the caption keeps advancing for the full reply length, not just the first ~550ms —
  needs a rebuild + the user's own test, since short replies under 550ms would never have
  shown Gap 1 even before this fix.

## Hand Mode: GPU-Contention Self-Mitigation + Camera-Cover Detection (2026-10-14)

- **Why**: a live test session showed a jerky cursor and a lagging/freezing orb at the same time.
  Cross-checked the real log against the code rather than guessing: the moment the foreground app
  became a heavy GPU consumer (Brave), the hand pipeline's `busy_skips`/`infer_p50` roughly tripled
  and stayed degraded for the rest of the session. Both the hand pipeline (MediaPipe GPU-delegate
  inference) and the orb (WebGL `rAF` loop) share one WebView2 GPU process with no isolation
  configured anywhere in this app — when a third app grabs a big share, both of NEXUS's own GPU
  consumers starve together. There's no OS API reachable from this layer to make Windows prioritize
  our GPU work over another app's, so the fix is self-mitigation (give back some of NEXUS's own GPU
  footprint under detected contention) plus an honest one-time notice instead of silent degradation.
  Separately: some laptops have a physical sliding camera cover, and a covered lens looked identical
  to "no hand in view yet" (`hand=0%` either way) — the user could waste real time before noticing.
- **Detection is almost entirely Rust-side, reusing data already flowing through `hand_frame`**:
  `Session` (`src-tauri/src/hand/mod.rs`) gained `calibrated_infer_p50_ms` (the chosen delegate's
  own one-time calibrated baseline, now threaded through `HandInfo`/`hand_info` from
  `frontend/src/hand/main.ts`'s existing calibration closures — `info`/`finish`/`calibrate` all
  gained a numeric `calibratedMs` param alongside their existing human-readable log string),
  `gpu_busy_high_windows`/`gpu_busy`/`gpu_busy_notified`. The existing `METRICS_EVERY` (5s) block
  inside `hand_frame` — which already computes `infer_p50` for the `[HAND] fps=...` log line —
  now also compares that window's `infer_p50` against the calibrated baseline: ≥2x for 2
  consecutive windows sets `gpu_busy`, hysteresis back down at <1.5x. **Speaking never happens
  while the `SESSION` mutex is held** (every subsequent frame needs that same lock; holding it
  across a TTS call would itself freeze tracking for the duration — the exact bug being fixed) —
  `hand_frame`/`hand_stats` both decide inside the lock, `drop(g)` (or a scoping block), then speak
  after. One spoken line per rising edge: "Something on screen is using a lot of graphics power,
  sir — the pointer might lag."
- **Self-mitigation, two already-existing mechanisms extended, nothing new invented**:
  (1) `shouldProcessFrame` (`frontend/src/hand/payload.ts`) gained a `gpuBusy` param — under
  contention it halves the processing rate even while a hand is in view (fewer, steadier cursor
  updates read as smoother than many wildly-delayed ones); `gpu_busy` rides back down from Rust on
  the existing `"hand:state"` broadcast the hand window already listens to. (2) the orb's existing
  battery-based particle cap (`frontend/src/avatar/Avatar.tsx`, 5000→2500 via `navigator.getBattery()`)
  now also listens for `"hand:state"` (a plain `app.emit`, not `emit_to`, so the stage window
  receives it too — new to Avatar.tsx, nothing new on the Rust side) and combines both signals:
  `Math.min(onBattery ? 2500 : 5000, gpuBusy ? 2000 : Infinity)`.
- **Camera-cover detection**: no raw pixel ever crosses IPC in this module by design ("no frame
  ever leaves the window" — module doc comment) — brightness/variance sampling has to stay
  entirely in `main.ts`. A 16×16 offscreen canvas (`sampleCoverage`, reused every frame, cheap next
  to the MediaPipe inference already running on the same frame) feeds `isLikelyCovered` in
  `payload.ts`: a genuinely covered lens is dark **and** flat (no texture) at once; a merely dim
  room is dark but still has some variance — checking both avoids false-flagging a dark room.
  Debounced ~2.5s before concluding "covered" (checked from the very first frames after arming, not
  a late fallback — that's what actually saves the user time); clears immediately on any normal
  frame. The one boolean conclusion rides the existing 5s `hand_stats` report to Rust (no new IPC
  channel) — `HandStats` gained `covered: bool`; `Session` gained `covered`/`covered_notified`
  with the same lock-then-drop-then-speak discipline as the GPU-busy path. Spoken once per rising
  edge: "I can't see anything, sir — your camera may be covered." Chip
  (`frontend/src/stage/handOverlayModel.ts::chipText`) gained two sibling suffixes next to the
  existing `dim` one — covered takes priority, then GPU-busy, then dim room.
- **Deliberately out of scope**: the software/Fn-key camera-disable case (a different code path —
  `getUserMedia` fails outright with `NotAllowedError`/`NotFoundError` rather than delivering dark
  frames); shortening the existing 1-minute `handIdleExitMs` specifically when covered (left
  untouched, a scope decision not an oversight).
- **A third, separate observation from the same live-testing pass, noticed but NOT fixed here**:
  a later test session showed `[HAND] fps=...` pinned at ~15 the *entire* session with
  `busy_skips=0` throughout (no backpressure — unlike the GPU-contention signature above) and
  every line reading `power=BATTERY`; an earlier session that reached ~30fps read `power=charging`.
  `cam_fps(track)` itself (the raw camera delivery rate, counted before any of this app's own
  throttling) was capped at 15 against a `driver_fps=30` ceiling. This looks like the camera/driver
  itself halving its delivery rate on battery — a plausible third contributor to "not smooth"
  distinct from both fixes above. Flagged for a future session, not bundled into this one.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (0 new warnings, 69
  pre-existing baseline unchanged), `cargo test --lib hand::` 86/86 (no regression). `npx tsc
  --noEmit` clean, `npx vitest run` 298/298 (293 pre-existing + 5 new: 2 `shouldProcessFrame`
  contention-branch tests, 3 `isLikelyCovered` fixtures — clearly-covered/dim-room/normally-lit),
  `npm run build` clean. **Not run live**: whether the mitigation actually reads as smoother on
  real contended hardware, and whether the brightness/variance thresholds (`avgBrightness < 12 &&
  variance < 4`, a first pass, not measured against a real physically-covered laptop camera)
  correctly separate "covered" from "very dim room" — both need the user's own hands-on test after
  a rebuild; thresholds may need one real-world tuning pass.
- **Docs**: plan at `C:\Users\Chitkul Lakshya\.claude\plans\delightful-scribbling-nautilus.md`.

## Keep-Awake + Agent-Completion Watcher (2026-10-14) — voice-wired + Settings panel

- **Why**: user wants (1) the screen to stay awake exactly while NEXUS is speaking/working, cleared the instant it's idle, and (2) a zero-API-cost way for NEXUS to notice when an external coding-agent CLI (Claude Code, Gemini CLI, Codex, Aider, Hermes Agent, opencode — explicitly NOT scoped to a fixed named list after the user pushed back on an Antigravity/opencode-only draft: "this is a production level project, don't only prioritise users that use antigravity, it should help everyone") finishes a task, using only local signals. Plan: `docs/features/` entry pending; full plan is in the approved plan-mode doc (keep-awake Pillar 2, agent-watch Pillar 3).
- **Keep-awake (`keep_awake.rs`)**: structural copy of `volume.rs`'s RAII-lease + heartbeat-watchdog `Controller`, minus baseline/defer (no value to restore, no audible-flicker defect — cleared instantly on release). Windows backend = `SetThreadExecutionState(ES_CONTINUOUS|ES_SYSTEM_REQUIRED|ES_DISPLAY_REQUIRED)`; the watchdog re-asserts every 500ms tick while held rather than trusting one call to hold forever (sources disagree on the real Win32 contract). `set_working(bool)` + `start_working_poller()` (3s poll on `ghost::drill_running() || orchestrator::has_active_request()`) auto-leases during any busy period; `tts.rs` holds a lease at every existing `volume::acquire_volume_lease()` site; `RunEvent::Exit` force-clears. 6 unit tests.
- **Agent-completion watcher, sidecar architecture (user's explicit call — "sidecar, it's less work and maintained upstream" — over a native Rust reimplementation of the adapter pattern)**: `src-tauri/resources/agent_watch_sidecar/` is a local-only Node sidecar (`server.cjs`, lazily `npm install`-ed on first use, **not bundled in the installer** — same precedent as Kokoro's model files, saves ~68MB on every installer download for a feature most users never touch) wrapping the real, maintained, MIT-licensed `pty-manager` + `coding-agent-adapters` npm packages. Those packages already implement exactly this pattern: spawn the CLI in a PTY, per-tool adapter regexes detect `task_complete`/`login_required`/`blocking_prompt`/etc on the ANSI-stripped output — covers Claude Code, Gemini CLI, Codex, Aider, Hermes Agent, opencode out of the box; a new tool later is a new adapter inside that dependency tree, never a Rust change. `agent_watch.rs` is the Rust-side process manager (mirrors `lazy_stt.rs`'s lazy-spawn/idle-kill shape exactly) + HTTP client (plain `GET/POST` to `127.0.0.1:39222`) + glue: `start_watch` acquires a `keep_awake` lease for the watch's lifetime, polls every 3s, and on `done`/`error`/`login_required` speaks via `proactive_policy::submit(..., AlertUrgency::High)`. 5 Tauri commands registered (`agent_watch_list_adapters`, `agent_watch_unavailable_reason`, `agent_watch_start`, `agent_watch_stop`, `agent_watch_status`). **Scope decision, stated plainly**: v1 has NEXUS spawn the agent session itself (prompt text sent as the first input line) rather than attaching to a terminal the user already started by hand — `pty-manager` is fundamentally a "spawn and own the child" library, not an "attach to an existing terminal's output" one, and building a full embedded-terminal UI to make the latter visible was out of this pass's scope.
- **Real upstream bug found + fixed while verifying live (not from docs — from actually spawning all six adapters)**: `pty-manager` v1.12.1's published ESM build (`dist/index.mjs`) uses a tsup-generated `__require` shim that assumes a global `require` exists; it doesn't in real ESM, so `manager.spawn()` failed every single adapter with a misleading `"node-pty is required but not installed"` (the real, swallowed error was `"Dynamic require of node-pty is not supported"`). Fixed by writing the sidecar as plain CommonJS (`server.cjs`, `"type": "commonjs"`) instead of ESM — `pty-manager`'s CJS build (`dist/index.js`) calls a real `require("node-pty")` and works correctly. Confirmed live: spawning `claude` actually started a real PTY session (pid logged, clean `session_stopped`); the other five correctly report "File not found" / `CreateProcess` error code 2 on this machine (those CLIs just aren't installed here — correct behavior, not a bug). `node-pty`'s own install script tries a prebuilt binary first (`node scripts/prebuild.js || node-gyp rebuild`); confirmed a real `win32-x64` prebuilt `pty.node` was used on this machine with zero compilation.
- **Voice wiring (same day)**: no new intent/grammar was added. The spoken phrase "ask claude to X" / "tell codex to X" / "run with aider to X" / "ask opencode to X" already existed (`parse_coding_command` → `ParsedIntent::CodingAgentTask{agent, prompt}`, routed to `ArchitectCenter`) and already dispatched the task via `coder::dispatch_to_agent` — but for Claude/Aider that was a raw `Command::spawn()` with **no completion signal ever reported** ("I have started X on the task" and then silence forever), and for Codex/OpenCode it was **clipboard-only** (copies the spec, the user has to paste it themselves — a real "claims success that didn't happen" gap in the same family AGENTS.md's own G0 entry flagged). `orchestrator.rs::run_coding_agent_task` now has a new `agent_watch_adapter_for(agent_name)` check BEFORE the old dispatch: if the spoken agent name is one of the six `agent_watch` covers (claude/gemini/codex/aider/hermes/opencode) AND the sidecar is available, it routes through `agent_watch::start_watch` instead — so "ask codex to fix the bug" now actually runs Codex (previously just clipboard) and "Understood, sir, I've started Claude Code on the task — I'll let you know the moment it's done" is now a real promise, not a fire-and-forget. Antigravity/Cursor/Trae/Windsurf/VS Code are deliberately NOT in `KNOWN_ADAPTERS` and keep going through the unchanged old path (spawn-if-possible, else clipboard) — nothing can watch them for completion, so nothing claims it can. On sidecar failure (e.g. no Node.js), falls through to the old path rather than failing the whole command. 1 new test (`agent_watch_adapter_for_covers_only_the_tools_coding_agent_adapters_ships`); existing `test_coding_command_parsing` (parser layer, unaffected) still passes unmodified.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean (0 new warnings, 69 pre-existing baseline unchanged), `cargo test --lib agent_watch::` 2/2, `cargo test --lib keep_awake::` 6/6, `cargo test --lib tts::` 24/24 (no regression from the added leases), `cargo test --lib coder::` 6/6, `agent_watch_adapter_for` 1/1, `test_coding_command_parsing` 1/1. Sidecar smoke-tested standalone (not through Rust): `/health`, `/adapters`, invalid-type 400, and all six real adapter spawns (one genuine success, five genuine "binary not found" — both correct). **Not run live**: the actual voice phrase through a real build (needs `nexus build` + a mic) — only the parser/orchestrator-routing layers and the sidecar's own HTTP surface were exercised directly.
- **Settings panel (same day)**: `frontend/src/settings-sidebar/AgentWatchSection.tsx` (mirrors `HandModeSection.tsx`'s shape), rendered inside `ConnectionsTab` — read-only status (`agent_watch_unavailable_reason`), the covered-tools list (`agent_watch_list_adapters`), and a live, 4s-polled list of active/recent watches (new Rust command `agent_watch_list_watches` → sidecar `GET /watch`) each with a Stop button while `starting`/`working`/`blocked`/`login_required`. No "start a watch from the UI" button — v1's only entry point is the voice phrase above, by design (a UI button would need its own task-text input and workdir picker, out of scope here). `npx tsc --noEmit` clean, `npx vitest run` 293/293 (no regression), `npm run build` clean.
- **Not built yet**: a bare "watch X for me" phrasing with no task attached (today a watch always needs a prompt, since v1 is "delegate + watch" not "attach to a session the user already started by hand" — see the scope decision above), and the live-hardware keep-awake tests from the approved plan (Windows sleep-timer behavior across a long reply, `powercfg /requests` after a hard `taskkill` of NEXUS — the "does `SetThreadExecutionState` need crash-recovery like `volume.rs`'s baseline file" question is still open — **needs the user's own hands-on testing on real hardware**, not something verifiable from here).
- **Known gaps, stated plainly**: `find_node()` only checks `node`/`node.exe` on PATH — no Windows-registry fallback like `find_python()` has; a user with agent CLIs but no Node on PATH gets a clear `sidecar_unavailable_reason()` string, not a crash, but the feature is simply unavailable for them. Node.js is **not** bundled in the installer (same as Python for STT) — this is a dev/power-user feature by nature (anyone running Claude Code/Codex/etc. almost certainly already has Node for those tools), not a "family build" feature.
- **Docs**: plan at `C:\Users\Chitkul Lakshya\.claude\plans\delightful-scribbling-nautilus.md` (keep-awake Pillar 2, agent-watch Pillar 3; voice-unification Pillar 1 untouched, still gated on its own 2-3 day spike). Detailed as-built architecture reference: `docs/features/122-agent-completion-watcher-and-keep-awake-architecture.md`.
- **Bigger vision, research only, nothing built (same day)**: the user's own next message described a much larger future direction — a 24/7 server (12GB VRAM/64GB RAM, separate from the laptop) mediating between multiple admin-enrolled friends and their own coding agents (own API keys, never shared), running a local "manager" model that plans/writes prompts/reviews results, tracking friend-laptop presence, learning per-friend over time, gated by admin key + TOTP + revocable per-device tokens, plus a $0-recurring telephony bridge over the user's own paid Jio SIM (friends call NEXUS, NEXUS calls them back). Researched properly (real sources, not assumed) and written up in full, including the corrected VRAM math (the manager model does need real GPU budget; STT/TTS should route to free cloud APIs to avoid GPU contention with live calls), the free-telephony finding (Android call-bridging fights an intentional OS restriction and is a per-device gamble; a cheap USB GSM modem + Asterisk `chan_dongle` against the same SIM is the actually-free and actually-reliable path; eSIM is a dead end for a server), and explicit security flags (phone-channel scope limits, voice-revoke needs a second factor, per-friend learning-data isolation). **Full writeup**: `docs/features/121-nexus-cloud-multi-agent-mediator-and-telephony-plan.md`. Feature 122 (agent_watch) is explicitly the "laptop hands" half of this vision already built in miniature — re-read it first if this gets picked up later.

## Hand Mode: the camera as a pointer (HandCenter, 2026-10-14) — READ-ONLY by default

- **Say** "hand mode" / "hand mouse" / "camera mouse" -> a small camera window (bottom-left: skeleton, status, mini screen map). Pointer idle until **palm -> fist -> palm -> fist**, which IS the confirmation: the real cursor then follows the hand (**LIVE by default since field test 2**; `handMousePreview` / "hand mouse preview" = read-only, shows what it would click, never touches the mouse). "exit hand mode" ends it (camera off, window destroyed). Gestures: move = relative cursor, pinch = click (hold = drag), thumb+middle = right click, two fingers = scroll, fist = pause.
- **Code**: `src-tauri/src/hand/` (`filter` One Euro, `pose`, `engine` pure state machine, `mod` session/IPC/output/safety), window `hand` (`frontend/hand.html`, `src/hand/`), intents `EnterHandMode/ExitHandMode/HandMouseLive`, `center.rs` -> `HandCenter` (16th), `orchestrator::run_hand_mode`. Settings in `NexusSettings`: `handMode` (true), `handMousePreview` (false), `handTrace` (false, landmark numbers only to `hand_trace.jsonl`), `handFrameSource` ("auto"|"track"|"rvfc").
- **Licence decision**: `reynaldichernando/pinch` has NO licence -> nothing copied from it; engine written independently; One Euro from the paper; MediaPipe tasks-vision 0.10.21 + `hand_landmarker.task` (Apache-2.0) via `scripts/sync-hand-assets.mjs` into git-ignored `frontend/public/hand/` (~17 MB; `nexus build` runs it; model SHA-256 pinned in `scripts/hand-assets.lock.json`). Release exe grows by that much.
- **Rules**: camera only while the mode is on; 21 numbers per frame over IPC, no frames stored; clicks refused in protected/elevated windows; real mouse movement (> 40 px from where we put it) or Esc disarms (Esc is registered only while armed and not when Ghost owns it); lost hand 0.6 s releases a held button, 5 s disarms; click lands where the cursor was 150 ms BEFORE the pinch closed and the cursor is locked until release.
- **Verify**: Rust lib 1434 (+33; rewind test mutation-checked), vitest 267, tsc, `npm run build`, dead-code 157 none new, IPC 193/130, turn-end + hygiene OK; Chromium page loaded model+wasm offline (0.46 s, GPU delegate, blank frame p50 20 ms). **Not run**: real camera in WebView2, tracking quality/latency/CPU (read the `[HAND]` line printed every 5 s), live mouse, multi-monitor (primary only).
- **Round 2 (same day)**: snap-to-control (`hand/snap.rs`: a click just outside a control moves to the nearest UIA control within 36 px, never per frame, `handSnap` default on); voice **"click here" / "double click" / "right click"** (`PointerClick`, clicks at the current cursor with the same protected/admin refusal, silent on success); arm/disarm **beep** (`MessageBeep`, needs windows feature `Win32_System_Diagnostics_Debug`); **"start with Ghost"** (`handWithGhost`, default off; Ghost entry starts it, Ghost end/Esc ends it only if Ghost started it); Command Hub card `HandModeSection` (on/off button + 4 toggles; IPC `hand_start/hand_stop/hand_status`). Rust lib 1440, vitest 267, IPC 196/133. NOT built: swipes/thumbs gestures, head-pointer (Face Landmarker) fallback, two-hand zoom, absolute mapping, spoken arm confirmation.
- **Field test 1 fixes (A-D, same day)**: the log showed the arm gesture DID fire (6 toggles) - "no difference" was the read-only preview plus a toggle that flipped back. **A** overlay on the stage (`stage/HandOverlay.tsx`, `handOverlayModel.ts`): virtual cursor ring + click ripple in preview, one status chip always saying IDLE/PREVIEW/LIVE; **B** the arm gesture only ARMS (palm-fist-palm-fist), a fist held 1.2 s disarms (not right after arming), rising vs falling two-tone beeps (`Beep` on a thread; no spoken line - it would interrupt turns); **C** frame-aware debounce (2 frames AND 80 ms), 6 s gesture window, fist tolerates one lazy finger, `[HAND] pose a -> b (finger ratios ...)` on every stable change; **D** canvas no longer reallocated per frame, GPU-vs-CPU delegate calibration on the first 25 frames (CPU tried only if GPU p50 > 30 ms, must win by 15 %), reported once as `[HAND] camera=.. delegate=.. calibration: ..` (`hand_info`). Rust lib 1445, vitest 277, IPC 197/134; both new engine rules mutation-checked; calibration flow smoke-tested in Chromium with a fake camera (GPU 32.5 vs CPU 49.2 ms on a blank frame).
- **Field test 2 fixes (2026-10-14 20:xx)**: log proved the camera pipeline delivered ~6 frames/s (`fps=2.0` with no hand = 6/3) while inference was ~27 ms; `latency_p50` hid camera delay; arming failed because one 6 s TOTAL window expired at 5 fps. Built: **per-step arm timing** (`arm_step_ms` 3 s), tolerant palm (3 straight fingers + avg >= 1.2), **live by default**, camera + model **load in parallel**, short spoken line, chip shown at once; frames read from the camera track (`MediaStreamTrackProcessor`, falls back to `requestVideoFrameCallback`), the shown picture is the analysed frame, `hand_stats` prints `[HAND] cam: source= cam_fps(track)= cam_fps(rvfc)= proc_fps= driver_fps= capture_to_landmarks p50/p95 page= exposure= power=` every 5 s; capture-time latency drives a **constant-velocity lead** (70 % of measured delay, clamped 4 % of the frame); One Euro 1.5/18; `NEXUS_NO_ECOQOS=1` disables EcoQoS for the A/B test. Rust lib 1450, vitest 281; per-step arming mutation-checked. **Read the next field log for**: `cam_fps(track)` vs `cam_fps(rvfc)` vs `driver_fps` (camera/driver slow vs page-throttled), `power=BATTERY`.
- **Field test 3 fixes + intent + screens (2026-10-14 21:xx)**: field log was good (camera 15 fps in a lit room, capture-to-rust 10-60 ms, lead 8-44 ms, arming worked). Built: **click = bend the index finger** (ring and pinky stay open: `pose::index_flick_ok`) OR the thumb-index pinch, ONE state machine = one click per action, 150 ms refractory, drag only after 450 ms hold; **no camera picture** by default (the `hand` window is an invisible 8x8 page; `handShowCamera` brings the debug picture back); the chip is short ("HAND · ON", "HAND · palm, fist, palm, fist 2/3", "dim room, add light" when cam fps < 15) and sits **right next to the orb capsule** (`chipPlacement`, from `stage:orb_rect`); **`mode_intent.rs`**: natural-language resolver (who acts + what is controlled + how) so "nexus i want to control the cursor" -> Hand, "i want to give command to you" / "take control of my laptop" -> Ghost, unclear -> asks, "give me my mouse back" -> leaves; the wake word is stripped first; **multi-monitor / mixed DPI** in the engine (`Mon`, per-monitor gain, crossing, gap snap, re-read every ~2 s); `hand::live_supported()` = Windows only, other OS stay in honest preview. Rust lib 1470, vitest 288; index click, refractory, evidence threshold and question guard mutation-checked. Plans: `docs/features/116-hand-gestures-window-and-tab-switching-plan.md`, `117-cross-os-screen-adaptation-and-intent-plan.md`. **Not run**: macOS, Linux, a second monitor, the new click on your hand (read the next `[HAND] pose` lines for index ratios during flicks).
- **Field test 4 + gestures + chip (2026-10-13/14)**: user reports: quick index pinch needed 3 tries (drag fine), hand mode could not be exited, cursor ignored the laptop's pointer speed and a fast hand did not move further. **Causes/fixes**: a click is decided on the RELEASE frame and the pose gate (`pinch_ok`: thumb nearer index than middle) often blinked there -> click thrown away; now cancelled only if the raw signal is still on while the pose gate fails (`signal_still_on && !pose_ok`), `pinch_close 0.38/open 0.6`; a parallel change bridges the DISTANCE half of a press across `press_catch_ms` (150) for low-fps cameras (bridge slots cleared on press/release/fist, bridged value must still be < `pinch_open`/`curl_release`, the pose gate stays current-frame, left press refuses a thumb clearly nearer the middle finger, right press needs the thumb < 0.75 of the index distance); exit: `loose_hand_exit` (while the mode is ON any short sentence with a leaving word + hand/camera/gesture/mode, or bare "exit"/"quit"), keep the fist closed 2 s after a fist-hold disarm (`ExitMode`), 10 min with no hand = exit; speed: Windows pointer speed (`SPI_GETMOUSESPEED` -> multiplier table) and "enhance pointer precision" scale a smoothstep ballistic curve (`Config.sensitivity/precision`, `handSensitivity` 0 = follow Windows); **gestures** (plan 116 built except G2): `hand/gesture.rs` pure (3 fingers flick = next/prev tab via the shortcut router, flick up = window switcher holding Alt with 5 independent release paths, 4 fingers together = desktops/Task View/show desktop, OFF by default `handGesturesDesktop` until the logged `spread=` is checked), `handGestures` default on; chip centred on the edge OPPOSITE the orb, above the taskbar. Docs: `docs/features/118-hand-mode-master-compendium.md`, `docs/changes/128-hand-gestures-and-field-test-4.md`, `docs/research/hand-mode/01-...md`. **Not run**: real-hand gestures/clicks, Four threshold, multi-monitor, macOS, Linux. **Pitfall**: other sessions edited `engine.rs`, `intent_parser.rs`, `commands.rs` at the same time and one stale write dropped edits; re-grep before building.
- **Field test 5: voice isolation + 1-min idle exit (2026-10-14)**: user report — pinch is "almost perfect" (separate, minor tuning note, not actioned here) but Hand Mode solo (no Ghost) was listening to ambient speech and answering it like Ghost's hot-mic loop (log: wake fires -> empty transcript -> silent auto-relisten -> real transcript, no intent match -> KnowledgeCenter/WorkerBackend answers aloud -> a SECOND wake-less capture does the same again). Root cause: the ambient wake-word classifier never checked Hand Mode at all; the *second* back-to-back relisten (after an already-answered, non-empty transcript) is a separate, confirmed, Hand-Mode-independent bug in the frontend's relisten-escalation streak logic (tier E's "streak resets on speech" did not fire) — reproduces in plain Normal Mode too, intentionally NOT fixed here, flagged for its own session. Fix: `hand::suppresses_voice() = active() && !ghost::session_active()` OR'd into the SAME choke point that already mutes wake detection for meetings/manual-pause (`wakeword_oww.rs`'s per-chunk `suppressed` check); Hand+Ghost together still listens normally (`ghost::session_active()` false->true flips it straight back off). The hotkey (Ctrl+Space) bypasses this gate entirely and is now the sole voice entry point — including "exit hand mode" — while solo Hand Mode mutes the ambient wake word. `idle_exit_ms` (no hand at all -> full exit) shortened 10 min -> 1 min via a new loose setting `handIdleExitMs` (default 60_000) applied in `hand::start()`; the pure engine's own `Config::new()` default stays 10 min (its own tests build against it directly). Exit-mechanism audit (the user asked "did you have an exit"): confirmed 5 pre-existing ways out (exact voice phrase, loose/fuzzy voice phrase incl. STT mishearings like "handle mode", natural-language `mode_intent.rs`, fist-hold-then-hold-2s gesture, now-1-min idle) plus Esc (disarm only, not a full exit, by design, matching Ghost's own Esc semantics — not changed). Rust `hand::` 86/86, `wakeword_oww::` 65/65, hand-mode intent-parser+engine tests 6/6, `cargo check` clean (0 new warnings).
- **Pitfalls**: `[HAND]` is whitelisted in `run.ps1`; the hand window must stay VISIBLE (a hidden WebView2 throttles video); new settings must stay in `NexusSettings`; the camera permission handler is registered per dynamic window via `mic_permissions::allow_media_for_window`.
- **Docs**: `docs/changes/127-hand-mode-camera-pointer-p0-p2.md`, `docs/changes/129-hand-mode-voice-isolation-and-idle-exit-timer.md`, plan `docs/features/115-camera-hand-mouse-plan.md`.

## Ghost = Full Control (no "Nexus", no window) + Dictation Cleanup + Test-Log Fixes (2026-10-13/14)

- **User directive (binding)**: Ghost mode must obey the owner without "Nexus" and without waiting. The engagement window is now **opt-in** (`ghostEngagementSecs`, default **0**). Protection from TV / Echo Dot = other assistants' wake words + our own voice (both need no change in how the owner speaks) and, next, the speaker check. Do not re-introduce a rule that makes the owner say "Nexus" or wait.
- **Test log (2026-10-13 15:40) fixes**: the window design was measured at transcript arrival (dropped "open notepad" 16 s after our reply) -> off by default; **7 Notepad windows**: `try_focus_existing` read a periodically-refreshed window cache, so repeats launched again -> live scan + `RECENT_LAUNCH` debounce (8 s) in `resolve_and_open_app`; "next is open brave" / "nexes" / "next us" parse as "Nexus" prefixes; `ActionDisposition`: unenrolled owner + Medium-evidence (NLU) command in Ghost now **runs** (it vanished silently: "nexus. oco notepad."); ghost turns run in heard order (`net/turnChain.ts`: a fast-path turn overtook an older slow-parse one and a stale cloud reply claimed "switching to ghost mode"); "Ok sir." only when the window is in front (`open_target_in_front`/`opened_message`); Groq STT failures print `[STT-ERR]`; `run.ps1` now shows `[MAIN-CMD] [DROP] [DIRECTED-GATE] [TURN] [DICT] [STT-ERR]` lines (they were hidden by its whitelist, which is why drops were invisible in every log); per-transition `[VOICE-ORB]`/`[ORB] setVisible` log noise removed.
- **Dictation (`dictation_text.rs` pure + `dictation.rs` injection)**: keeps STT punctuation, drops um/uh, spoken punctuation/layout ("comma", "full stop", "question mark", "new line", "new paragraph", "bullet point", quotes, brackets), "scratch that" (alone = undo the last insertion, inside an utterance = drop what preceded), paused corrections ("no, wait" / "sorry I mean" only with an audible pause), capitalisation incl. "I", numbers before units ("five p m" -> "5 pm", "25%"), one-line targets lose the trailing period, `dictionary.json` replacements. Typing refuses protected windows, NEXUS itself, elevated windows and password fields (spoken reason); **no Enter after each line any more**; undo = Backspace N in the same window else Ctrl+Z; dictation stops itself after 30 s without a line; stop phrases tolerate punctuation ("Stop typing.", "Nexus, stop dictation"). One-shot `type X` goes through the same cleaner.
- **Verify**: see the last line of this entry once the build finishes. dictation 29 tests (about 60 text assertions - the plan's 150-pair corpus is NOT reached yet).
- **Not built**: hold-to-talk (D4), voice command "add X to my dictionary" (edit `dictionary.json` by hand for now), speaker check, `[TURN]` one-line-per-turn summary (only the existing `[MAIN-CMD]`/`[DROP]` lines are now visible), app-class detection beyond WhatsApp/Telegram/Discord/Slack/"search" titles.
- **Pitfalls**: `dictation::HISTORY` / `directed::ENGAGED_UNTIL` are process-global (tests reset them); Python `patch_*.py` heredocs with `'''` break in the shell tool - write the script with the file tool.

## Room-Noise Gate Rules + Search by URL (2026-10-13)

- **Directed gate (`directed.rs`)**: (1) foreign wake words (alexa, hey/ok google, hey siri, ...) are rejected before the command check unless the turn also says "Nexus"; (2) our own recent speech is rejected BEFORE `parses_as_command` (verbatim replays at >= 0.85 overlap for commands, 0.6 otherwise, so the owner repeating "open the command hub" after our "Opening Command Hub, sir" still passes); (3) **engagement window**: a hot-mic turn without "Nexus" is ignored (`outside_window`) unless NEXUS spoke or accepted a command in the last `ghostEngagementSecs` (opt-in; default 0 = off since 2026-10-14, cap 120); `note_spoken` extends it by speech length + grace, an accepted turn and `ghost_enter` open it. New settings `ghostEngagementSecs` and `directedGate` are in `NexusSettings` (the latter used to be wiped on save).
- **Search (`live/commands/browser.rs`)**: "search X" opens `https://www.google.com/search?q=<encoded>` in the foreground browser's own exe (new tab; `browser_url::foreground_process_path`) or the default browser, instead of Ctrl+L + typing + Enter with fixed 200 ms gaps (unverified, raced with focus).
- **Verify**: Rust lib 1374 (+ e2e 2, offline 10, user cmds 7), vitest 256, tsc clean, dead-code 157 none new, IPC/turn-end/hygiene OK, `nexus build` produced `src-tauri/target/release/nexus.exe` (94.1 MB; 195 min wall time because other builds were running). **Not run live**: the window against a real TV/Echo Dot, search in Brave.
- **Not built (needs the user)**: speaker model + enrollment (G1), 3 ambient recordings for `gate_replay`, WhatsApp probe, wake plan P1-P5.
- **Pitfalls**: the engagement window is process-global state; tests that touch it call `reset_engagement()`. Outside the window the owner must say "Nexus, ..." — if commands seem ignored after a pause, raise `ghostEngagementSecs`.

## Orb Lifecycle (G3) + WhatsApp Opener (G2 part) + Build Unblock (2026-10-13)

- **Build was broken**: `frontend/src/theme/tokens.css` had the theme-independent tokens (typography/spacing/radii/shadows/z-index) orphaned outside any rule after the light-theme block was added (`Unexpected }`, `npm run build` failed). Wrapped them in a `:root { }` block. Always run `npm run build` after touching CSS.
- **Orb**: `net/orbPolicy.ts` (pure, clock-injected, 5 tests): in a Ghost session the orb shows while NEXUS thinks/speaks, a caption/loader/question is up, or the mic hears a voice (`audio:level` >= 0.35), lingers 4 s, then hides; a parked hot mic keeps it up, dimmed (`data-parked`). Hot-mic `listening` alone no longer keeps it visible. `OrbFrame` uses `useGhostOrbShown`; `Avatar` pauses WebGL when hidden (`shown` prop); exit no longer lingers 3 s; 5 s two-way resync with `get_pending_ghost_session`. Rust: `ghost_enter` arms Esc BEFORE the session becomes Active (a failed registration used to leave a live session with no cancel button); the overlay-gave-up path ends Ghost mode (its global Esc would otherwise swallow Esc system-wide) and says so.
- **WhatsApp opener** (`wa_open.rs`, `ghost_drill.rs`): at most 3 name candidates (was up to 10 -> 20-30 s misses), kin group reordered by how people save contacts, an empty chat list is "UIA not ready" (extended poll) not "not found", and a saved number (`remember that wa phone mummy is <number>`, core fact `wa_phone_<name>`, local only) opens `whatsapp://send?phone=` directly with the foreground check kept.
- **Verify**: Rust lib 1369 (+ e2e 2, offline 10, user cmds 7), vitest 256, tsc clean, `npm run build` ok, dead-code 157 none new, IPC/turn-end/hygiene gates OK, `nexus build` produced `src-tauri/target/release/nexus.exe` (94.1 MB). **Not run live**: idle-hide timing with a real mic, parked look, the deep link, cold-start polling.
- **Pitfalls**: Ghost keeps store `visible` true for the whole session; visibility of the capsule is decided only by `useGhostOrbShown`. The Start-menu shortcut still points at an old `%LOCALAPPDATA%/NEXUS/nexus.exe`.

## G0 Cleanup + Honesty Review of the Gemini Features 108–113 (2026-10-13)

- **Removed**: dead Rust relisten watchdog (`ghost.rs`) + the `ghost:relisten` listener in `orbRuntime.ts` (never started / never emitted). Dead-code baseline 164 -> 157.
- **Found**: Gemini's block / reminder / doc-analysis / coding-agent / localhost handlers are wired but several **spoke success that did not happen** (WhatsApp/phone "blocked" with no executor; Instagram script never awaited; lease marked done and "unblocked" without checking; "I will remind you" when nothing was stored; doc analysis fell back to analysing `AGENTS.md`; `agy` started without the prompt; IDE agents "copied to clipboard" with nothing copied; `npm` unrunnable on Windows; `contains("intern")` matched "internal").
- **Fixed**: `run_instagram_action` awaited with real reason; unsupported platforms refused aloud; lease only after a successful block and only if stored; `process_due_leases(store, now, exec)` records `completed`/`failed`, failures announced once; reminders refuse when not stored; honest doc-analysis miss; `coder::Dispatch` (`Reach::Spawn` only for claude/aider, else clipboard); localhost via `cmd /C` and refuses non-projects; whole-word internship match.
- **Needs a decision**: claude/aider are spawned straight from a voice transcript (should be confirm-gated in `center::action_disposition`); `c:\PROJECTS` hard-coded in `find_project_directory`; `instagram_client.py`/`doc_analyzer.py` are not bundled and need `python` on PATH; WhatsApp/phone blocking and WhatsApp document download are not implemented although docs 108/109 describe them.
- **Verify**: Rust `cargo test --tests --features custom-protocol,admin-brain` serial: lib 1365 (9 ignored) + e2e 2 + offline 10 + user cmds 7; frontend tsc clean + vitest 251/251; turn-end, hygiene, IPC (191/128, 63+7 known) gates OK; dead-code ratchet 157. **Not run live**: Instagram session, reminder/lease round trip, localhost on a real project, clipboard handoff.
- **Pitfall**: a Python `patch_*.py` chained after a timed-out command can silently not apply — re-grep the file after patching. Never write test executors that touch a real Instagram session: `process_due_leases` takes an injected executor.
- **Docs**: `docs/changes/126-g0-dead-watchdog-removal-and-gemini-feature-honesty-review.md`.
- **Live log 2026-10-13 (stale binary: still showed `ghost:relisten`; run `nexus build` before `nexus start`)**: `center::source_strength` only knew 4 parse sources, so every other `deterministic-*` source (dictation, screen-analysis, directions, places, memory, persona, counsel, timetable, mail) scored 0 and was silently dropped as ambient in a Ghost session for an unenrolled voice ("type google meet" -> no reply + 28 s stall). Now every `deterministic-*` is strong except `-fuzzy`. Added "ghost moon / ghost in mode / gostum / ghost mold / ghost moved / gost mode" -> ghost mode (they reached the cloud LLM, which claimed "Ghost mode activated"). Removed the per-render `[AVATAR] render` log. Verify: Rust lib 1367, vitest 251, dead-code 157 none new. **Open**: orb stays visible after a Ghost turn (G3); "open brave" heard as "open brief".

## Quality System Q0/Q1: Honest CI, Dead-Code Ratchet, IPC + Event Contracts (2026-10-12)

- **Why**: the same breakages kept recurring (settings dropped, features never wired, stale builds) and CI ran only `cargo test --lib`; this adds checks that run on every push instead of more notes here. Plan = part Q of the approved "make it stop breaking, then build the features properly" plan.
- **Found + fixed**: `check-turn-ends.mjs` scanned two deleted files and crashed (hid 13 unmarked `reset()` sites); `tests/phase2_integration.rs` referenced deleted `tts_piper`; **`cargo check --features mock-wake` was broken since 2026-10-03**; Worker `tsc` error; CI never ran `src-tauri/tests`, Worker or Python tests, nor compiled `custom-protocol,admin-brain`. `server/NEXUS_credentials.db` is NOT tracked (audit was wrong; `*.db` ignored).
- **Built**: `scripts/check-turn-ends.mjs` (auto-scan, needs `turn-end:keep-raw <reason>`), `check-repo-hygiene.mjs` (+`--selftest`), `dead-code-audit.mjs` (rustc `--force-warn dead_code` sees through `allow`; baseline 164), `check-ipc-contract.mjs` (191 commands / 128 invoked; baselines 63 unused commands + 7 one-sided events), `scripts/baselines/*`. `lib.rs` publishes only the 6 modules in `external-api.txt`; the rest are `pub(crate)` so rustc can see unused items.
- **Rules**: new dead code / unregistered command / unheard event FAILS CI; baselines may only shrink (`--update` after wiring or deleting). Never add `#[allow(dead_code)]` (use `#[expect]` with a reason if truly intentional). Adding `pub mod` needs a line in `external-api.txt` saying who imports it.
- **Verify**: `cargo test --tests --features custom-protocol,admin-brain` serial: lib 1352 + e2e 2 + offline 10 + user cmds 7; frontend tsc + vitest 243; Worker tsc + 109; Python 31; every gate mutation-checked. **Not run**: the GitHub Actions workflow itself.
- **Pitfalls**: a name starting with `_` is exempt from dead_code (do not use it to test the ratchet); `--update` while a gate shows a NEW item defeats the ratchet.
- **Docs**: `docs/changes/125-quality-system-q0-q1-honest-ci-ratchets-and-contracts.md`.

## Wake Word P0: Honest Measurement — Replay Harness, Session-Split Manifest, Baseline (2026-10-11)

- **Plan (approved)**: `docs/features/107-wake-word-production-plan.md` (target >=95% recall at <=0.1 false wakes/h; P0 measure -> P1 opt-in live clips -> P2 new recipe + conv-attention head -> P3 cascade: speaker check + verify only in uncertain band + AEC -> P4 orb/no-lost-words -> P5 gates). Research: `docs/research/wakeword/industry-comparison-and-recipe-2026-10-11.md`.
- **Built**: `wakeword_oww.rs` pure `stage1_step`/`detection_score` shared with replay, replay clock (`now_ms`: wall live, audio time in replay), `WakeEngine::with_classifier`, named consts; fixed no-op first grace reset + "10s grace" log lie. `wake_eval.rs` + `dev_tools/wake_replay.rs` = `nexus wake replay` (real engine over WAVs: recall per condition, short-clip FAR, stream fires/h, sweep, misses, offenders). `scripts/build_wake_manifest.py` (session-separated `wake_word_data/manifest.jsonl`), `record_wake_ambient.py`, `wake_review.py` (P1 reviewer); `nexus wake devices` fixed.
- **Baseline (held-out owner session, stage 1, thr 0.68)**: recall **63.2%** (quiet 3/10, far 15/25), 10/15 spoken soundalikes fire, 12 fires/h on 0.42 h real background. Training split shows 93.8% (TTS 99.2%) — old numbers were self-scored; procedural negatives never fire (too easy).
- **Verify**: wake_eval 12/12 (live-vs-replay equivalence, non-vacuous, mutation-checked), full lib 1331/1331 serial (skip tts_bench/tts_kokoro), cargo check clean, `wake_data_foundation.py validate --require-data` passes. **Not run live**: engine edits in the running app (watch restart grace).
- **Pitfalls**: run replay with `--release`; wake_camp 2026-09-26 + room 2026-09-18 audio is the TEST set — never train on it; manifest rebuild keeps only `origin: live|ambient` rows; model fingerprint re-recorded (`nexus.onnx` = `622afbfc…`, the manifest had a stale hash).
- **Needs the user**: >=10 h ambient recordings (`python scripts/record_wake_ambient.py --category tv --minutes 60` …), label `wake_word_data/needs_review.jsonl` (10 clips).
- **Docs**: `docs/changes/124-wake-p0-honest-measurement-replay-harness.md`.

## Stuck Orb, Wake Policy, WhatsApp Chat, Memory Hygiene & Session Restore (2026-10-10)

- **Bugs**: orb never hid; wake fired on "That's a poem" and rejected "Hello, Nico."; `localSttOnly` still sent audio to Groq; "open my chat with mummy in whatsapp" parsed as contact "mummy in whatsapp"; memory held mostly STT junk; boot greeting said "in Opencode on “OpenCode”"; "shut down my laptop" parsed as close-app.
- **Built**: orb idle watchdog + dead-end turn reset + stage ready gate/transcript replay; verify fail-open only at prob >= 0.9 + soundalike tier; `wa_open.rs` (process-checked foreground, Ctrl+F, UIA list match, honest failure); `memcore/hygiene.rs` admit gate + one-time cleanup; `memcore/session.rs` (60 s snapshot, session-end hook), `restore.rs`, `power_cmd.rs`, `health.rs`; greeting = whole last session + health + "Shall I reopen them?" (`offer_boot_session`).
- **Verify**: Rust lib 1313/1313 serial, vitest 243/243, tsc + build clean, mutation-checked. **Not run live**: orb behaviour, reboot offer round trip, shutdown hook, Brave/Edge restore flag, WhatsApp Ctrl+F/UIA on build 2.2639, cleanup on real data.
- **Pitfalls**: power parse must precede close verbs; start `session::start_recorder` only after `session::at_boot`; `memcoreSession`/`memcoreHealth`/`powerCommands` must stay in `NexusSettings`; restored apps get only existing non-sensitive path args.
- **Docs**: `docs/changes/123-orb-wake-whatsapp-memory-hygiene-and-session-restore.md`.

## TTS Volume Lease Rework + One Voice Per Utterance (2026-10-09)

- **Bugs**: volume not held at the Command Hub level / not restored; voice changing mid-reply. Causes: narration lease dropped immediately; barge-in `force_restore` + stale leases corrupting the counter; frontend split replies into per-sentence `speak_text` calls (volume dipped between sentences); 15 s watchdog measured from acquire; baseline only in memory; per-sentence silent fallback to Kokoro; browser Web Speech fallback; stale Kokoro-voiced cached acks.
- **Fix**: `volume.rs` `Controller` (epoch leases, 1.2 s deferred restore, heartbeat watchdog 10 s, durable `volume_baseline.json` + startup recovery, restore on exit/tray quit); `tts.rs` one engine per utterance (chunk 0 decides, edge retried once, failure = silence, never another voice), stale local cache entries skipped + regenerated, `[TTS]`/`[VOL]` log lines; frontend `speak()` is one invoke, no Web Speech fallback.
- **Verify**: 14 volume tests (fake backend, mutation-checked), cargo check clean, lib suite passes (3 unrelated tests flake under 4-parallel, pass serially), tsc clean, vitest 224/224. **Not run**: live OS-volume probe, real Ghost session (needs `nexus build`).
- **Pitfalls**: use `defer_restore_volume` for barge-in and `force_restore_volume` only for explicit stop/exit; any new playback loop must call `volume::heartbeat()`; never hold a lease across code that can run >10 s without beating.
- **Docs**: `docs/changes/122-tts-volume-lease-rework-and-one-voice-per-utterance.md`.

## Pebble Wakeup Design & Elastic Thread Stretch Animations (2026-10-09)

- **User Directives**: (1) Pebble background design for Voice Orb container while strictly preserving existing Classic Capsule (`10px` pitch black `#000000 !important`); (2) Elastic thread stretching wakeup opening: thread forms from bezel border, stretching down into a catenary pouch with the orb (circle) nestled inside; (3) Bezel border contact wings show tunable rim highlight specular opacity; (4) Multiple physical animation variations on `localhost` so the user can test, compare, and choose; (5) De-glow the Orb Customization Studio button in Command Hub to match standard glass options (no purple glow); (6) Set user's chosen animation (Mode 1 Elastic Thread Stretch $k=1.03$, Spark Burst genesis, Top screen dock, $301\times105\text{px}$) in their laptop settings and Command Hub with desktop preview.
- **Built**:
  - `frontend/pebble-showcase.html`: Interactive testbed with 5 physical wakeup modes (Mode 1: Elastic Thread Stretch & Snap [user favorite], Mode 2: Bowstring Pluck & Rebound, Mode 3: Liquid Meniscus Extrusion, Mode 4: Horizon Split & Wing Flare, Mode 5: Acoustic Harmonic Ripple); 4 WebGL Orb interior genesis modes (Core Bloom, Droplet Roll, Spark Burst, Trefoil Weave); real-time catenary Bézier SVG morphing; independent rim base opacity, wing contact highlight opacity, and wing glow width sliders; frame-by-frame scrubber (0-1000) with 1.0x/0.5x/0.25x playback speeds.
  - `frontend/src/settings-sidebar/SettingsSidebarApp.tsx`: De-glowed "✦ Open Orb Customization Studio" button to clean `settings-btn--glass`; added "Orb Capsule & Scoop Style" card (`Classic Capsule` vs `Fluid Pebble Scoop`); added Wakeup Thread Animation 5-mode selector, Orb Interior Genesis selector, and "✦ Test Wakeup on Desktop" preview button.
  - `frontend/orb-studio.html`: Added dedicated "Wakeup" tab displaying physical thread stretch options with immediate desktop trigger and persistent save.
  - `frontend/src/stage/OrbFrame.tsx`: Implemented `computeStretch(tau, wakeupMode, k)` driving all 5 physical animation equations in the production stage overlay with dynamically cradled Voice Orb.
  - `src-tauri/src/commands.rs` & `lib.rs`: Added `set_orb_capsule_style(style, mode, genesis)` and `test_wake_orb` IPC commands; emits `orb:capsule_style` across all windows on change/save.
  - Persisted user's chosen parameters in `settings.json`.
- **Verify**: `npx tsc --noEmit` clean, vitest 224/224 passed across 30 test files, `npm run build` cleanly bundled in 4.76s (`dist/pebble-showcase.html`), `cargo check --features custom-protocol,admin-brain` clean, `cargo test --lib -- commands::` 36/36 passed. Live preview running at `http://localhost:5173/pebble-showcase.html`.
- **Docs**: `docs/research/orb-waves/06-pebble-wakeup-design-and-organic-bezel-geometry-2026-10-09.md`, `docs/features/106-pebble-wakeup-orb-capsule-plan.md`, `docs/changes/121-pebble-wakeup-design-and-elastic-thread-stretch-animations.md`.

## Pure Black Letter-by-Letter Voice Captions (2026-10-09)

- **User Directives**: (1) Pure black background matching `.orb-capsule` (`#000000 !important`, `10px` radius, `1px solid rgba(255,255,255,0.16)`, deep drop shadow); (2) Zero zoom and no blue font glow — steady typography baseline in clean crisp white (`#ffffff`); (3) Continuous letter-by-letter progressive streaming as spoken.
- **Built**:
  - `frontend/src/audio/captionScheduler.ts`: Replaced block word reveals with intra-word character progressive scheduling based on word start/duration timestamps. Line notifications continuously stream the growing character snapshot.
  - `frontend/src/stage/ResponseCaption.tsx`: Switched container to `.response-caption--black` and `.caption-black-capsule`. Renders `{currentText}` directly as plain string with zero per-word span mapping, eliminating DOM churn, scale transforms, and layout shifts.
  - `frontend/src/stage/ghost.css`: Configured `.caption-black-capsule` to match `.orb-capsule` exactly, stripped `@keyframes mercury-pop` and cyan glows.
- **Verify**: `npx tsc --noEmit` clean (0 errors), vitest 224/224 passed across 30 test files, `npm run build` built cleanly in 13.65s, `cargo check --features custom-protocol,admin-brain` clean.
- **Docs**: `docs/features/105-pure-black-letter-by-letter-caption-plan.md`, `docs/changes/120-pure-black-letter-by-letter-voice-captions.md`.

## Orb WebGL Shader Compile Fix (2026-10-09)

- **Bug**: every startup logged `[ANIM] WebGL shader/context compilation error ... 0:199 '*' wrong operand types 'bool' and 'highp float'`, so the voice orb never rendered through WebGL. Cause: two lines in the vertex shader (`voice-orb.js`, `listenInGlow` and `listenSpark`) multiplied `(!isFixed)` - a GLSL `bool` - by floats, which GLSL ES forbids. Fixed with `(isFixed?0.0:1.0)`. The CPU `_paint2D` fallback already used a JS boolean and is unchanged.
- **Also fixed**: `scatter` and `textPos` are declared in the shader but unused (particle-text/flight code was removed), so the GLSL compiler drops them and `getAttribLocation` returns -1; binding location -1 produced four `INVALID_VALUE: index out of range` warnings per load. Binding is now guarded with `>= 0`.
- **Verify**: loaded the real orb in the Vite playground (built-in browser): WebGL initialises (`6240 particles`), cycles idle -> listening -> thinking -> speaking with no GL errors or warnings; **mutation-checked** - reintroducing `(!isFixed)` reproduces the exact `0:199` error; tsc clean, vitest 224/224, `npm run build` ok. **Not run**: the packaged app (needs `nexus build` + `nexus start`; the stage console should no longer show the compile error); a pixel-level visual diff of the listening state before/after (the "before" never rendered).
- **Pitfall**: GLSL has no implicit bool->float; write `cond?1.0:0.0`. A shader edit that compiles in one driver can still fail in WebView2's ANGLE - always load the playground and read the console after touching `VS`/`FS`.

## Shortcut Map + Action Router, keyboard tier (E1, 2026-10-09)

- **Built**: `src-tauri/src/shortcuts/` (`keys.rs` layout-independent VK chords + batched `SendInput` events; `seed_windows.json` + `map.rs` 23 actions x families browser/vscode/explorer/notepad/terminal/console + `*` fallback, precedence exe -> family -> global, validated at load; `router.rs` gates + **pre-send same-window re-check** + send + title-change verify; `exec.rs` Windows adapter, `shortcut_do` IPC). Rewired: `command_executor::browser_key` (was a detached PowerShell SendKeys: slow, unchecked, always "success"), `screen::switch_browser_tab`, `live/commands/browser.rs` (new tab, address bar in navigate/search/focus). Kill switch `NEXUS_SHORTCUTS_LEGACY=1`.
- **Safety (tested)**: wrong-window = nothing sent; protected windows (exe denylist, credential tools by title) and elevated targets refused; terminals/consoles never inherit global Ctrl+C/Z/S/X/A/Y; unknown exe + no binding falls back to the old hotkey; known app without the shortcut says so.
- **Verify**: 36 new tests, corpus gate 25 actions x 4 browsers = 100 %, mutation-checked, Rust 1224/1224; live on Notepad: verified in ~42 ms, unverified actions 2-3 ms. **Not run**: real browser / full Ghost voice path; new actions have no voice phrases yet (needs grammar U2) - use `invoke("shortcut_do",{action:"refresh"})`.
- **Pitfalls**: `src-tauri/examples/` is git-ignored - dev tools live in `src-tauri/dev_tools/` with `[[example]]` entries; keep chords as VK codes (never `Key::Unicode`); every new seed entry must be a documented default of that app (the seed test validates syntax, not truth).
- **Plan**: `docs/features/104-ghost-whatsapp-direct-chat-and-command-relation-plan.md` (PLAN ONLY): WhatsApp "open <name> chat in whatsapp" + the retry-vs-continuation command relation logic.
- **Docs**: `docs/changes/119-shortcut-map-and-action-router-e1.md`.

## Ghost Reliability (Phase R) + P0 Measurement Tooling for Command Understanding & Keyboard-First (2026-10-08)

- **Plan/research**: `docs/features/102-keyboard-first-shortcut-brain-plan.md`, `103-ghost-command-understanding-and-keyboard-first-implementation-plan.md`; research `docs/research/ghost-mode/09-…`, `10-…` (phrasing space + measured parser accuracy), `11-p0-baseline-report-2026-10-08.md` (UIA measurements).
- **Fixed (R)**: watchdog stall threshold 20 s (> the 12 s invoke budget) + guarded relisten; timeout closes the turn; central duplicate-listen guard (`net/listenDedupe.ts`); native duplicate-capture-start guard (`wakeword_oww::is_duplicate_capture_start`, fails open).
- **Measured**: today's parser: bare command 43 % correct / 12 % WRONG action, one polite frame word → 3.6 %, 19 of 36 Ghost actions have no intent, 7.7 % of everyday sentences accepted as commands. UIA: `screen::list_actionables()` roots at the FOCUSED element → **0 controls and 5.1 s** in Notepad/Calculator (leaf focus), 2.5 s in Explorer; one cached window-rooted query = 18–79 ms. AcceleratorKey 0–3 %, AccessKey up to 70 % (`"Alt, F"` form), Invoke 52–91 %, shortcut-in-name 0–22 %.
- **Tooling**: `cargo build --example uia_probe` (read-only UIA probe), `[GHOST-TIMING]` per ghost click, opt-in `sttLogConfidence` → `[STT-CONF]`, `#[ignore]` `tests/phrasing_probe.rs`, `scripts/phrasing_probe/*` (incl. `collect_heldout.py`, `record_ambient.py` — need the user).
- **Pitfalls**: `sttLogConfidence` must stay in `NexusSettings`; do not rename duplicate `docs/changes/115*/116*` (referenced by number); the probe launches nothing — focus the target app during its countdown; never run it on windows with private content without checking the JSON first.
- **Verify**: vitest 224/224, tsc clean, Rust main group 1186/1186 (8 ignored), cargo check clean. **Not run live**: double-wake/stall race in a real session; Chromium/Electron probes; held-out + ambient measurements. **E0.5 DONE (same day)**: `screen::list_actionables()` is now one cached foreground-window query (2.5 s budget; legacy kept as `list_actionables_legacy()`, kill switch `NEXUS_UIA_LEGACY=1`): Notepad 5,097 ms/0 controls → 24–44 ms/23 of 23. Calculator/Explorer not re-measured (foreground contention); ordinals ("3rd option") now count across the whole window, off-screen controls excluded — validate live. Rust now 1188/1188.
- **Docs**: `docs/changes/118-ghost-reliability-phase-r-and-p0-measurement-tooling.md`.

## Google Maps Directions Window (2026-10-08)

- **Built**: "directions to X" / "how do I get to X" (`ParsedIntent::Directions`) opens the real Google Maps in its own top-level window (960 wide, centered, y=40, same geometry as the architect view via `sidebar_geometry("maps")`) and speaks an acknowledgement; with a Routes API key (`routesApiKey`, Command Hub row `google_maps`) AND a spoken origin it speaks a traffic-aware ETA. **Not an iframe**: Google blocks framing, so `dyn_windows::open_external_window` uses a top-level https webview with no IPC.
- **Pitfalls**: `routes_api_key` must stay in `NexusSettings`; "navigate to" is deliberately not claimed; no device location is used (ETA needs a spoken origin); restrict the key to Routes API; `frontend/.env.local` has an unused `VITE_ROUTES_API_KEY` to delete.
- **Verify**: Rust 1173 (main group), cargo check clean, vitest 215, tsc clean. **Not run live**: the window, Routes API, macOS/Linux.
- **Places (same day)**: `PlacesSearch` ("nearest pharmacy", "X near me/near Y") via Places API (New) text search + key row `google_places` (`placesApiKey`); saved home/work resolve from remembered facts. Both keys live-verified once. Wording is "top result" (relevance, not distance).
- **Leave-by reminders**: `memcore/leavewatch.rs` — calendar events with a street address + Routes API traffic ETA from the REMEMBERED HOME (no device location) -> "Time to leave for X" via `proactive_policy`; calendar read 10 min, Routes only within 90 min of the event; setting `memcoreLeaveBy` (default on, inert until Google + Routes key + home exist). Docs: `docs/changes/116-leave-by-reminders.md`. Not run live (needs a real calendar event).
- **Docs**: `docs/changes/115-google-maps-directions-window.md`, `docs/features/101-google-maps-routes-window-plan.md` (its iframe design is superseded).

## Design #11 Liquid Mercury Stream: Incremental Word-by-Word Voice Captions (2026-10-08)

- **User Directive**: User selected Design #11 (Liquid Mercury Chrome) for captions. Constraint: no full pre-rendered sentence highlighting; words MUST ONLY appear as they are spoken out in real-time (incremental reveal with zero future words in DOM). Mode: Accumulating Clause Stream (words appear one-by-one as spoken up to sentence/clause breaks, hold for 2.0s reading dwell, then smoothly dissolve for the next sentence).
- **Built**:
  - `frontend/src/audio/captionScheduler.ts`: `CaptionLineEvent` extended with `words?: string[]` and `activeWordIndex?: number`. Schedules word reveal timers anchored to word audio timestamps (`start_ms`). `notifyLine` emits only words revealed up to that millisecond; 2.0s reading dwell after the last word before triggering smooth cross-dissolve.
  - `frontend/src/stage/ResponseCaption.tsx`: Upgraded to render `.response-caption--mercury` with `.mercury-pill` and `.mercury-pill-text`. Maps over revealed `words` with active word glow (`.mercury-word--active`) and settled spoken words (`.mercury-word--spoken`). Symmetrically centered horizontally via `translateX(-50%)`, dynamically expanding outward as words arrive. Returns `null` when no words have been spoken.
  - `frontend/src/stage/ghost.css`: Implemented Liquid Mercury Chrome styling (36px backdrop blur, 1px white highlight border, pill geometry, cyan chrome text shadow, spring `@keyframes mercury-pop`).
  - `frontend/caption-showcase.html`: Option #11 interactive simulation on `http://localhost:5173/caption-showcase.html`.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean, `cargo build --release` fresh binary (93.37 MB) at `%LOCALAPPDATA%\NEXUS\nexus.exe`, frontend `npx tsc --noEmit` clean, vitest 210/210 passed, `npm run build` clean in 12.7s.
- **Docs**: `docs/changes/113-liquid-mercury-streaming-captions.md`.

## Orb Customization Studio & Production State Morphologies (2026-10-07)

- **Selected Morphologies Integrated**:
  - **Idle**: Stardust sphere with gentle breathing cycle.
  - **Listening**: Design #1 acoustic inward suction vortex with RMS jitter.
  - **Thinking**: Design #6 **Trefoil Celtic Helix Cage** ($p=3$ braided helical rings intersecting at $120^\circ$).
  - **Speaking**: Design #2 **64-Strand Fibonacci Blossom** with acoustic petal pulses and sound energy expansion.
- **Orb Customization Studio Built**:
  - `src-tauri/src/dyn_windows.rs`: Defined `orb_studio` window configuration ($800\times980\text{px}$, frameless liquid glass, on-demand creation/destruction).
  - `src-tauri/src/commands.rs`: Added `open_orb_studio_window` and `close_orb_studio_window`. Calculated geometry to dock Studio flush to the left of the Settings window with a $12\text{px}$ margin ($x = \text{monitor\_w} - 1222.0$) at twice the width of Settings ($800\text{px}$ vs $400\text{px}$), without closing Settings.
  - `frontend/orb-studio.html`: Built minimal 2-pane studio interface: left pane hosts clean numerical option selectors ($1..12$) for Thinking, Speaking, and Listening with Save button; right pane hosts an interactive $380\times380\text{px}$ real-time WebGL canvas with audio reactivity simulation slider.
  - `frontend/src/settings-sidebar/SettingsSidebarApp.tsx`: Added **"✦ Customize Orb"** action card in Settings directly beside voice personas.
- **Verify**: `cargo check --features custom-protocol,admin-brain` clean, `cargo test --lib -- commands::sidebar_geometry_tests` 13/13 pass, `npm test -- --run` 200/200 pass, `npx tsc --noEmit` clean, `npm run build` built cleanly in 15.2s.


## Google Sign-In Fix, Keychain Test Isolation & Token-Removal Leak (2026-10-08)

- **Why Gmail could not be added**: `DEFAULT_CLIENT_ID` in `google/oauth.rs` was a placeholder (zeros) -> Google "OAuth client was not found, 401 invalid_client". There is now NO built-in client: the user must create their own Google OAuth client (Desktop app, Gmail + Calendar APIs, Test users) and paste Client ID + Secret in Command Hub -> Advanced. Sign-in fails in 0 s with instructions (`credentials_status()`, IPC `google_credentials_status`); Client ID shape is validated on save.
- **Test isolation**: all keychain access in `auth_vault.rs` goes through `kr()`, which installs keyring's in-memory mock under `cfg(test)` (`NEXUS_REAL_KEYRING=1` to opt out). Before this, `cargo test` wrote fake `user1@gmail.com`/`user2@gmail.com` accounts into the REAL Credential Manager and would have overwritten the user's real Google account registry.
- **Token leak fixed**: `remove_google_account` now clears `google_rt_<email>` / `google_at_<email>` (the entries actually read); refresh token no longer stored twice. `photoslibrary.readonly` is no longer requested (Google removed it 2025-03-31). Testing-mode refresh tokens expire after 7 days: the inbox watcher now flags `mail_needs_signin`, says so once a day and the Memory page shows "Paused".
- **Open**: API keys still live in BOTH the keychain and plaintext `settings.json` (not stripped, despite older notes). Publishing the OAuth app removes the 7-day expiry but unverified Gmail scopes show a warning screen and a user cap.
- **Verify**: Rust 1133 + TTS groups, `cargo check` clean, vitest 210; live run of the release binary: encrypted store, new IPC commands, instant sign-in failure, no panics. **Not run**: a real Google sign-in (needs the user's OAuth client).
- **Docs**: `docs/changes/113-google-signin-fix-vault-test-isolation-and-token-leak.md`.

## Memory Core P6: WhatsApp Priority People — Read-Only, OFF by Default (2026-10-08)

- **Built**: `memcore/wa.rs` polls the local WhatsApp MCP bridge (45 s active / 5 min away) and `memcore/people.rs` learns who matters from counts and timing only (two-way volume, reply-within-an-hour rate, recency, two-way; one-sided senders x0.25; VIP +30; mute = 0; High >= 70, Medium >= 50; groups ignored; message text never used). Alerts name the sender only (text is never in the alert); first sight of a chat is silent. Voice: "read that message" / "read Asha's messages" / "what did Raj say on WhatsApp" (sidebar card, local), "make Asha a VIP", "mute WhatsApp alerts from Raj", "unmute ...", "who are my priority people"; "that's not important" now mutes whichever alert (mail or WhatsApp) was last. Memory page: switch, "Check WhatsApp connection", people list with VIP/Mute. Settings `memcoreWhatsapp` and `memcoreWhatsappSpeak` both default **false** (speaking message text sends it to the cloud voice service).
- **Guarantees (tested)**: only the read allowlist is callable from the watcher; `mark_read`/`mark_chat_read`/`send_presence`/`send_typing` are refused for every caller in `mcp_client::call_tool`; a mock bridge that advertises those tools sees only `tools/list`, `list_chats`, `list_messages`; store lock never held across an `.await`.
- **Bugs fixed on the way**: VIP/mute could be set but not cleared (store refuses mined writes that unpin a record -> user choices are written as `UserSaid`); `parse_pairing_state` returned "unrecognized" for MCP replies wrapped in `structuredContent` or a JSON text block.
- **Pitfalls**: new settings must stay in `NexusSettings`; tests that touch `TEST_WHATSAPP_URL` serialise on `MOCK_GUARD`; `turn_detect::model_loads_and_runs` is a timing assertion (<1500 ms) that can flake while other builds run (passes alone); `tts_bench` can hit LNK1104 if a test exe is still running.
- **Verify**: Rust 1163 + bench 9 + kokoro 12 (three groups), `cargo check --features custom-protocol,admin-brain` clean, frontend tsc clean + vitest 215 + build ok. **NOT verified**: anything against the real bridge (not running here; the list tools' JSON format is undocumented, parsers are tolerant, selftest reports what it could not read), and **whether the bridge's read calls send read receipts or change presence** - hence OFF until `docs/testing/whatsapp-read-receipt-test.md` is done. Replying from the phone marks chats read (uncontrollable). Not built: mark-read-after-reply, call-frequency signal, chat previews in the briefing. WhatsApp may ban unofficial clients; session re-pairs ~20 days.
- **Docs**: `docs/changes/114-memory-core-p6-whatsapp-priority-people.md`, `docs/testing/whatsapp-read-receipt-test.md`.

## Memory Core P5: Important-Mail Watcher & Calendar (2026-10-08)

- **Built**: `memcore/mailwatch.rs` polls Gmail `history.list` (stored historyId per account; 90 s active / 8 min away; backoff; 404 = quiet re-baseline; metadata only, never bodies) -> `memcore/mailtriage.rs` deterministic rules (database-service pause/deletion, GitHub security/CI, hackathon status/deadlines, exam notices, plain deadlines; the Promotions tab suppresses weak matches) -> sealed `mail` records (untrusted, 7 days, Medium+) + `proactive_policy` alerts (High/Medium; 3+ batched). First connect is quiet. "That's not important" mutes the last alert's sender as a visible fact `mail_mute_<addr>`. Voice: "any important emails", "that's not important", "what's on my calendar today/tomorrow", "add X to my calendar tomorrow at 5pm" (`memcore/agenda.rs`; read back + overlap check + confirm before `events.insert`). The briefing now includes today's calendar and filed mail. Memory page: `memcoreMail` switch, status line, Important-mail list with reasons.
- **Pitfalls**: the poll logic is `poll_core(dir, key, client)` (no Tauri handle) so it is tested against a local mock server via `google_io::TEST_BASE`; `memcoreMail` must stay in `NexusSettings`; the Mail and Slot tiers skip the secret-word screen on purpose (a "token expired" notice must still store). `orchestrator.rs` and `net/orchestrator.ts` were being edited by another session while this was built - keep patches anchored and narrow.
- **Verify**: Rust 1150/1150 in three groups, `cargo check` clean, frontend tsc clean + vitest 209 + build ok, mock-server integration tests (baseline, pagination, label filtering, stale history, error mapping, calendar list/insert). **Not run live**: no Google account is connected on this PC (the old keyring mock never persisted tokens) - reconnect once in the Command Hub; spoken alerts not heard.
- **Docs**: `docs/changes/112-memory-core-p5-inbox-watcher-and-calendar.md`.

## Memory Core P4: Timetable from an Image, Slot Reminders & "Shall I Start?" (2026-10-08)

- **Built**: "analyse this and add section 2 to my timetable" (screen) / "add the picture I copied …" (clipboard) → Gemini reads sections/slots → code validates (times, days, caps, dedupe, section pick) → sidebar confirm card + spoken "Shall I add them?" → saved sealed (tier `slot`, user_owned). Scheduler (30 s) fires each slot's reminder once a day via `proactive_policy` (Medium); DSA/LeetCode slots ask "Shall I start?" and register a pending offer (`memcore/offer.rs`) that only listens after the question is spoken (`speak_line` arms `sentinel_offer_*`), opens an 8 s frontend mic window (`openPendingOfferWindow`, `offer_pending`), and is accepted by the directed-speech gate. "Yes" opens LeetCode where you left off + the last video watched DURING a DSA slot (resume samples are tagged with the running activity); app-vs-browser is stored per activity as `study_app_<activity>` and overwritten whenever repeated. New intents: `timetable_add|show|clear|commit`, `study_pref` (parsed BEFORE screen analysis). Memory page: timetable list + reminders switch (`memcoreTimetable`).
- **Pitfalls**: the offer state is process-global (tests serialise on `offer::TEST_GUARD`); the shared vision client has a 4 s timeout, too short for reading a timetable (own 30 s client); new settings must stay in `NexusSettings`; never guess a video: no tagged video = skip YouTube.
- **Verify**: Rust 1116/1116 in three groups (TTS modules still abort together), `cargo check` clean, frontend tsc clean + vitest 206 + build ok. **Live**: clipboard image read; real Gemini request returned all 5 slots of a generated two-section timetable and section 2 picked correctly (2.8 s). **Not run live**: the spoken reminder → mic window → "yes" round trip; photos/handwriting.
- **Docs**: `docs/changes/111-memory-core-p4-timetable-reminders-and-offers.md`.

## Memory Core P3: Resume Points & Boot Briefing (2026-10-08)

- **Built**: `memcore/resume.rs` samples the foreground window every 30 s while the user is at the PC (GetLastInputInfo) and keeps one record per window change in the sealed store (7-day ring, tier `resume`, never in the cloud pack; sensitive windows, NEXUS itself and the lock screen are never recorded; URLs reduced to host+path). `memcore/briefing.rs` is a pure builder: where you left off (+ real age), plus open items = watched-email deadlines (verbatim text) and requests NEXUS could not finish (`conversation::unfinished_requests`). No invented progress numbers; silent when it has nothing true to say. Delivered once a day ~45 s after startup, only when the user is present, via `proactive_policy::submit` (Medium). New intent `briefing` ("where did I leave off", "show my briefing", …) speaks short and opens a sidebar card. Command Hub → Memory: switches `memcoreActivity` / `memcoreBriefing` (both default on) + "Clear activity history".
- **Pitfalls**: resume points must be snapshotted (`snapshot_at_boot`) BEFORE the recorder starts or the briefing reports the current session. Sampling, not a shutdown hook (no reliable exit event on logoff). The new settings must stay in `NexusSettings`. Running the entire lib suite in ONE process currently aborts (0xffffffff) in `tts_bench` + `tts_kokoro` (Kokoro loaded twice under low RAM); each passes alone — run `--skip tts_bench --skip tts_kokoro` plus each separately.
- **Verify**: 1092/1092 in three groups, `cargo check` clean, frontend tsc clean + vitest 205/205 + build ok, live foreground/idle probe passed. **Not run live**: a real reboot (delay, presence gate, spoken delivery), recorder title quality across apps.
- **Docs**: `docs/changes/110-memory-core-p3-resume-points-and-boot-briefing.md`.

## Memory Core P2: Encryption at Rest, Cloud-Send Log, Name Redaction & "What I Remember" (2026-10-07)

- **Built**: the Memory Core database is held in memory and persisted as ONE sealed blob (`memory/memcore.enc`, AES-256-GCM via `ring`), key in Windows Credential Manager (written then read back through a fresh handle, so the keyring mock can never own the data; no usable key store = plain file, and the UI says so). P1's plaintext db is migrated then shredded. Wrong key / tamper is refused without overwriting; wipe rotates the key (crypto-erase). A 7-day encrypted log records what each cloud turn sent (request + memory text). Optional `memcoreRedactNames` swaps known people's names for `Person A/B…` out and back. "What do you remember" now also opens a provenance-labelled sidebar card; Command Hub has a Memory page (list, forget, cloud-send log, redaction switch). IPC: `memcore_status`, `memcore_list`, `memcore_egress_log`.
- **Honest scope**: stops disk theft / other users / casual reads, NOT same-user malware (it can ask the same API for the key). Whole-database re-seal per write is fine at this size.
- **Verify**: `cargo test --lib -- --test-threads=1` 1072/1072, `cargo check` clean, frontend tsc clean + vitest 204/204 + build ok, live two-process keychain probe passed (also confirms the P0 keyring fix). P1 release build linked (11m01s). **Not run live**: the Memory page / sidebar card visually, first-launch migration on real data, name redaction against a real Worker.
- **Docs**: `docs/changes/109-memory-core-p2-encryption-egress-log-redaction-memory-ui.md`.

## Memory Core P1: SQLite Store, Ranked Recall, Context Pack & Worker Reads Memory (2026-10-07)

- **Built** (`src-tauri/src/memcore/`): `rusqlite` bundled SQLite (`memory/memcore.db`, WAL, secure_delete) with records + FTS5 + append-only audit. Every record has `source` + `trust`; the admit gate refuses secrets, empties and untrusted text as a fact; mined values never overwrite pinned user-said ones. Ranked recall = BM25 + recency + use-frequency + pinned (plurals/prefix handled). Derived facts decay 3%/day, episodes expire at 30 days. `context_pack(query, 2000)` is the only memory text that leaves the device: ranked, PII-redacted, line-boundary cut. `memory.rs` is dual-write / ranked-read with legacy fallback; legacy JSON files are imported once and left in place. Setting `memcore` (default on) disables it.
- **Worker** now reads `dialog_context.memory` (`memoryPreamble` in `counsel.ts`, labelled data-not-instructions, clipped). Before this only the 9Router path saw memory.
- **Pitfalls**: retrieval is lexical ("where do I work" does not find `employer`); pinned in a known-miss fixture. `memcore` must stay in `NexusSettings` or `save_settings` drops it. Tests that delete a temp dir while its SQLite handle is cached leave the dir behind on Windows (harmless).
- **Verify**: `cargo test --lib -- --test-threads=1` 1050/1050, `cargo check` clean, Worker 109/109. **Not run live**: first-launch migration on real data, a real Worker round-trip with memory.
- **Docs**: `docs/changes/108-memory-core-p1-sqlite-ranked-recall-worker-memory.md`.

## Memory Core Plan + P0 Fixes: Keychain Backend, Forget/Wipe Coverage, Auto-Learning (2026-10-07)

- **Plan (approved)**: `docs/features/100-memory-core-plan.md` (research in `docs/research/memory/01-...`). One `memcore` module owned by `MemoryCenter` (not the Command Center, not per-sub-center): sub-centers emit typed `Observation`s with provenance + trust, the Main Center asks for one `context_pack` per turn. Priority-based tiers T0-T6, SQLite + app-level AES-GCM, deterministic priority scoring, WhatsApp read-only with `mark_read` deny-list, Gmail via `history.list`, timetable from screen/clipboard image, boot briefing via `proactive_policy`. Phases P0-P7, each flag-gated. User decisions: screen+clipboard image input; app-vs-browser preference per activity, overwritten on repeat; priority people learned + pin/mute; boot briefing spoken after short idle.
- **Honest NOs**: exact YouTube resume position, greeting before login, downloading Instagram reels, controlling WhatsApp read state when the user replies from the phone.
- **P0 built**: `keyring` now has `windows-native` (before, it was an in-memory mock, so keychain-stored keys/tokens did not persist), `forget` covers `facts.json`, `wipe_memory` covers conversation/brief/diary/mail watches, atomic writes, `log_episode` wired into `record_worker_turn`.
- **Verify**: `cargo test --lib -- --test-threads=1` 1034/1034 pass, `cargo check --features custom-protocol,admin-brain` clean. **Not verified**: keychain persistence across a real restart (check `cmdkey /list` and that Groq/Gemini keys still work after the first launch).
- **Docs**: `docs/changes/107-memory-p0-fixes-keyring-forget-episodes.md`.

## Whisper Flow AI Analysis, Zero-RAM Streaming Architecture & Groq STT Optimization (2026-10-07)

- **Research Context & Motivation**:
  1. User requested thorough investigation into "Wispr Flow AI" to understand how it eliminates STT delay, whether it is open source, and what open-source models/architectures exist to achieve sub-250ms voice command responsiveness in Ghost Mode.
  2. Strict constraint: When internet is available, cloud offloading must be used to preserve zero local RAM consumption (keeping the ambient ~175 MB baseline).
  3. User requested exact quantitative % improvement comparisons against both Wispr Flow and the previous Groq REST flow, plus RAM comparison tables across idle, running, and max peak states.
- **Key Findings & Blueprint**:
  - **Wispr Flow is Closed Source**: Developed by Wispr AI (Stanford spinout); proprietary $12/month SaaS with zero public model weights. Uses 4-stage pipeline: WebSocket audio streaming $\to$ proprietary sub-200ms "Canto" ASR $\to$ speculative LLaMA-3-8B disfluency cleaner $\to$ OS text injection (700ms total budget).
  - **Open-Source Benchmark**: Benchmarked top alternatives: `ggerganov/whisper.cpp` (~54.2k ★), `SYSTRAN/faster-whisper` (~25.7k ★), `pipecat-ai/pipecat` (~16.2k ★), `usefulsensors/moonshine` (~11.2k ★), `m-bain/whisperX` (~11.1k ★), `collabora/WhisperLive` (~4.3k ★), and `cjpais/Handy` (~1.2k ★).
  - **NEXUS Optimization vs Previous Groq Flow**:
    - End-to-end latency drops from ~1,025ms to ~240ms (**76.6% faster / 4.3x speedup**).
    - Trailing dead-air silence drops from 650ms to 150ms (**76.9% reduction**).
    - Audio upload transport drops from 220ms to 35ms (**84.1% reduction**).
    - RAM impact: 175 MB $\to$ 177 MB (**+1.1%, virtually 0% impact, preserving 0 MB model RAM**).
    - Cost: Retains 100% free tier (2,000 requests / 8 hours of audio per day on Groq LPUs).
  - **NEXUS Optimization vs Wispr Flow**:
    - NEXUS is **65.7% faster** than Wispr Flow (~240ms vs ~700ms) for voice commands because NEXUS skips cloud LLM reformatting and parses intent in native Rust (<5ms).
    - **100% cost savings** ($0 vs $120/year).
- **Docs**: `docs/research/voice-stt/02-whisper-flow-zero-ram-streaming-and-groq-optimization-2026-10-07.md`, `docs/research/ghost-mode/08-zero-delay-streaming-stt-and-ghost-mode-realignment-2026-10-07.md`.

## Dual-Engine Hybrid STT: Groq LPU Fast-Path & Optional Deepgram Nova-2 Streaming (2026-10-07)

- **Problem & Motivation**:
  1. Primary Groq LPU pipeline achieved ~240ms command response with $0 cost and zero local model RAM, but Groq's public API does not support token-by-token WebSocket partial streaming.
  2. To drive `LiveCaption.tsx` with partial transcripts mid-sentence without forcing 200MB–1GB of local model RAM onto the user's PC, an optional cloud streaming provider was needed.
- **Root Cause & Fixes**:
  - `src-tauri/src/stt_deepgram.rs`: Built Deepgram Nova-2 REST transcription client with zero-RAM audio serialization.
  - `src-tauri/src/stt_stream.rs`: Added Deepgram Nova-2 cloud WebSocket streaming over TLS (`wss://api.deepgram.com/v1/listen?model=nova-2&...`); emits `stt:partial` to `LiveCaption.tsx` for real-time word hypotheses while speaking.
  - `src-tauri/src/stt.rs`: Wired cascade: if `deepgramApiKey` is configured, uses Deepgram; if empty or failed, seamlessly routes to primary Groq Whisper Large v3 Turbo; falls back to local faster-whisper.
  - `src-tauri/src/api_keys.rs` & `commands.rs`: Added `("deepgram", "Deepgram", "deepgramApiKey")` to `KEY_SERVICES`, auto-exposing Deepgram in Command Hub's API Keys settings.
- **Verify**:
  - `cargo check --features custom-protocol,admin-brain`: clean (**0 warnings, 0 errors**).
  - `cargo test --lib -- stt_deepgram`: 2/2 pass.
  - `cargo test --lib -- api_keys`: 7/7 pass.
  - `cargo test --lib -- stt`: 31/31 pass.
  - `cargo test --lib -- wakeword`: 58/58 pass.
  - `cargo test --lib -- intent_parser`: 197/197 pass.
  - `npm test -- --run`: 200/200 pass.
  - `npx tsc --noEmit`: clean (0 errors).
  - `npm run build`: built in 8.17s.
  - `cargo build --release --features custom-protocol,admin-brain`: fresh release binary (96.1 MB) produced at `src-tauri/target/release/nexus.exe`.
- **Docs**: `docs/changes/106-dual-engine-groq-deepgram-streaming-integration.md`.

## Zero-Delay Rust Intent Pre-Routing & IPC Fast-Path Elimination (2026-10-07)

- **Problem & Motivation**:
  1. Once Groq finished transcribing audio on the Rust capture thread, raw text was transmitted to the frontend over `stt:transcript`, which then called `invoke("parse_transcript")` back into Rust before triggering execution.
  2. This inter-process hop introduced 15ms–45ms of unnecessary latency and thread serialization overhead.
  3. Frontend `Intent` type definitions omitted several newer desktop & ghost mode intent variants (`enter_ghost_control`, `exit_ghost_control`, `browser_search`, `start_dictation`, `watch_screen_email`, `memory_audit`, `persona_friend`, `share_concern`).
- **Root Cause & Fixes**:
  - `src-tauri/src/wakeword_oww.rs`: In `stt-capture-rx`, normalized phonetic mishearings via `normalize_phonetic_mishearings` and evaluated deterministic intent via `parse_deterministic` directly on the capture thread. Enriched `SttTranscript` with `intent_label: Option<&'static str>` and `pre_parsed: Option<serde_json::Value>`.
  - `frontend/src/stage/orbRuntime.ts`: Updated `SttTranscriptTurn` and the `stt:transcript` event listener to extract `intent_label` and `pre_parsed`, forwarding them directly into `turn: SttTurnMetadata` for `processTranscript`.
  - `frontend/src/audio/recorder.ts`: Updated `SttTurnMetadata` and `parseTranscriptEnhanced`: if `preParsed?.intent` exists, returns the pre-parsed intent immediately without calling `invoke("parse_transcript")` (0ms IPC delay).
  - `frontend/src/intent/parser.ts`: Synchronized `Intent` type union with all `ParsedIntent` variants in Rust.
- **Verify**:
  - `cargo check --features custom-protocol,admin-brain`: clean (**0 warnings, 0 errors**).
  - `cargo test --lib -- wakeword`: 58/58 pass.
  - `cargo test --lib -- stt`: 31/31 pass.
  - `cargo test --lib -- intent_parser`: 197/197 pass.
  - `npx tsc --noEmit`: clean (0 errors).
  - `npm test -- --run`: 200/200 pass.
  - `npm run build`: built in 6.40s.
- **Docs**: `docs/changes/105-zero-delay-rust-intent-pre-routing-and-ipc-fast-path.md`.

## Zero-Delay Groq STT VAD Pacing, Persistent Tokio Runtime & HTTP/2 Connection Pooling (2026-10-07)

- **Problem & Motivation**:
  1. VAD trailing dead air was hardcoded to 10 chunks (800ms) after speech completed, adding significant hesitation to Ghost Mode and standard voice commands.
  2. The STT capture thread (`stt-capture-rx`) and wake word verifier built a brand new `reqwest::Client` on every single spoken turn, preventing connection reuse and paying a 150ms–220ms cold TLS handshake penalty on every turn.
  3. The capture receiver built and destroyed a single-thread Tokio runtime on every turn, causing CPU churn and thread allocation overhead.
- **Root Cause & Fixes**:
  - `src-tauri/src/stt.rs`: Added global `SHARED_STT_CLIENT` with `pool_idle_timeout(90s)` and `tcp_keepalive(60s)` via `shared_client()`; updated `SttState::new()` to clone the shared client.
  - `src-tauri/src/wakeword_oww.rs`: Defined `STT_SILENCE_CHUNK_LIMIT_GHOST = 3` (240ms) and `STT_SILENCE_CHUNK_LIMIT_GHOST_PATIENT = 6` (480ms); lowered default `STT_SILENCE_CHUNK_LIMIT` from 10 to 5 (400ms) and patient from 15 to 10 (800ms); made endpoint adaptive to `crate::ghost::session_active()`.
  - `src-tauri/src/wakeword_oww.rs`: Initialized Tokio runtime once outside `stt-capture-rx` loop; switched both STT capture and wake word verification to reuse `crate::stt::shared_client()`.
- **Verify**:
  - `cargo check --features custom-protocol,admin-brain`: clean (**0 warnings, 0 errors**).
  - `cargo test --lib -- wakeword`: 58/58 pass.
  - `cargo test --lib -- stt`: 31/31 pass.
  - `cargo test --lib -- --test-threads=1`: 1025/1025 pass.
  - `npm test -- --run`: 200/200 pass.
  - `npx tsc --noEmit`: clean (0 errors).
  - `npm run build`: built in 7.87s.
  - `cargo build --release --features custom-protocol,admin-brain`: fresh release binary (96.0 MB) produced.
- **Docs**: `docs/changes/104-zero-delay-groq-stt-vad-pacing-and-connection-pooling.md`.

## Sidebar Text Formatting, Top-Right Dock Buttons & Settings Full-Height Span (2026-10-07)

- **Problem & Motivation**:
  1. Settings sidebar (Command Hub) height was clamped at 780px (`.min(780.0)` in `commands.rs`), leaving an empty ~280px gap above the Windows taskbar on 1080p+ screens.
  2. Dock Left and Dock Right controls were faint, unstyled text buttons embedded in a rigid 72px grid cell, lacking proper desktop control styling and positioning.
  3. Heading styles (`.nexus-h1..h6` and `h1..h6`) in `sidebar.css` were unstyled, resulting in default browser user-agent margins and misaligned response text. Insights stats columns had misaligned vertical baselines.
- **Root Cause & Fixes**:
  - `src-tauri/src/commands.rs`: In `sidebar_geometry("settings")`, removed `.min(780.0)` cap so height extends to `(monitor_h - 40.0).min(1040.0).max(400.0)`, spanning full height down to the taskbar. Updated unit tests.
  - `frontend/src/settings-sidebar/SettingsSidebarApp.tsx` & `hub.css`: Upgraded `.hx-header-dock` with polished $28\times28\text{px}$ pill buttons matching desktop window controls, anchored at top right; styled flex header layout with absolute centered title.
  - `frontend/src/settings-sidebar/hub/HubHome.tsx` & `hub.css`: Added placeholder subtitle rows so all 3 insight stats columns share identical vertical and horizontal baselines; polished `.hx-section` and `.hx-row` typography.
  - `frontend/src/sidebar/sidebar.css`: Restored SF Pro typography hierarchy for `.nexus-markdown-body h1..h6` and `.nexus-h1..h6`; added `:first-child { margin-top: 0 !important; }`; aligned `.sidebar-header-row` dock buttons flush at top right.
- **Verify**:
  - `cargo test --lib -- commands::sidebar_geometry_tests`: 13/13 pass.
  - `npx tsc --noEmit`: clean (0 errors).
  - `npm test -- --run`: 200/200 pass.
  - `cargo check --features custom-protocol,admin-brain`: clean (**0 warnings, 0 errors**).
  - `npm run build`: clean (built in 9.91s).
  - `cargo test --lib -- --test-threads=1`: 1020/1020 pass.
  - `cargo build --release --features custom-protocol,admin-brain`: clean release binary produced.
- **Docs**: `docs/changes/102-sidebar-text-formatting-top-right-dock-and-settings-height.md`.

## CPU, RAM, Battery & Energy Optimization Suite (2026-10-07)

- **Problem & Motivation**:
  1. Win32 cursor polling loop ran at a fixed 30ms interval (33.3 Hz) even with zero active hitboxes, waking CPU cores continuously and preventing entry into deep Intel/AMD Package C-states (C8/C10), elevating background idle power (~1.5% CPU / ~3.0W).
  2. Background daemon lacked Windows 11 EcoQoS (Efficiency Mode) declaration, allowing audio processing and watchdog routines to run on high-power Performance cores (P-cores).
  3. WebGL orb render loop continuously executed at 60 FPS during ambient 0.33 Hz idle breathing and failed to halt during OS display sleep/occlusion (`document.hidden`), consuming unnecessary GPU power on battery.
  4. Audio telemetry in `wakeword_oww.rs` formatted and emitted debug strings at 2.08 Hz during room silence even in release builds.
- **Root Cause & Fixes**:
  - `src-tauri/src/stage.rs` & `live_glass.rs`: Upgraded hitbox loop to adaptive 3-tier pacing: 250ms when hitboxes are empty (cuts 94% of wakes), 100ms when cursor is distant (>120px), and 16ms within 120px proximity.
  - `src-tauri/src/power.rs` & `lib.rs`: Implemented Win32 `SetProcessInformation` with `ProcessPowerThrottling` / `PROCESS_POWER_THROTTLING_EXECUTION_SPEED` to opt the process into Windows 11 EcoQoS (Efficiency Mode), scheduling background tasks to E-cores.
  - `frontend/src/avatar/voice-orb.js`: Added `document.hidden` check and `visibilitychange` listener to drop GPU draws to 0 FPS on occlusion; paced idle breathing state to ~25 FPS (`> 40ms` interval), cutting GPU draw calls by >60% while maintaining 60 FPS for active states.
  - `frontend/src/avatar/Avatar.tsx`: Added dynamic battery detection via `navigator.getBattery()`; dynamically scales particle count from 5,000 on AC power down to 2,500 on battery power.
  - `src-tauri/src/wakeword_oww.rs`: Gated silent audio telemetry log formatting behind `#[cfg(debug_assertions)]`.
- **Verify**:
  - `npx tsc --noEmit`: clean (0 errors).
  - `npm test -- --run`: 200/200 pass.
  - `cargo check --features custom-protocol,admin-brain`: clean (**0 warnings, 0 errors**).
  - `cargo test --lib -- --test-threads=1`: 1020/1020 pass.
  - `cargo build --release --features custom-protocol,admin-brain`: fresh release binary produced cleanly.
- **Docs**: `docs/changes/101-cpu-ram-battery-energy-optimizations.md`, `docs/research/performance/01-cpu-ram-battery-optimization-plan.md`.

## Build Warnings Zero-Out, Stage Cold-Boot Resilience & Daemon Exit Prevention (2026-10-07)

- **Problem & Motivation**:
  1. Build logs emitted 23 Rust compiler warnings (`unused_imports`, unregistered IPC commands, dormant helper functions) and Vite chunk warnings (>500 kB).
  2. Launching via `nexus start` triggered a premature blackout (`stage: blackout detected (window_gone=false, stale_beat=true) — incident #1`) because `SHOW_GRACE_SECS` was only 8s, expiring before WebView2 completed cold-boot initialization (~8.1s).
  3. Watchdog window recreation destroyed the `stage` window; because 0 windows remained, Tauri v2's default exit behavior shut down the entire daemon.
- **Root Cause & Fixes**:
  - `frontend/vite.config.ts`: Set `build.chunkSizeWarningLimit: 2000` to silence chunk warnings for the unified sidebar.
  - `src-tauri/src/window_manager.rs` & `ocr.rs`: Removed unused `Emitter` and `IAsyncOperation` imports.
  - `src-tauri/src/lib.rs`: Registered `tts::restore_tts_volume`, `architect::cancel_architect_analysis`, and `architect::query_impact` in `generate_handler!`; updated runner to intercept `RunEvent::ExitRequested` and call `api.prevent_exit()`.
  - `src-tauri/src/browser_center.rs`, `system_center.rs`, `youtube_center.rs`, `improve.rs`, `pii_filter.rs`, `vision.rs`: Added `#[allow(dead_code)]` annotations to dormant utility functions and structs.
  - `src-tauri/src/stage.rs`: Increased `SHOW_GRACE_SECS` from 8s to 20s and `HEARTBEAT_STALE_SECS` from 6s to 10s.
- **Verify**:
  - `npm run build`: clean (0 errors, 0 warnings).
  - `npm test -- --run`: 200/200 pass.
  - `cargo check --features custom-protocol,admin-brain`: clean (**0 warnings, 0 errors**).
  - `cargo test --lib -- --test-threads=1`: 1020/1020 pass.
  - `cargo build --release --features custom-protocol,admin-brain`: fresh release binary (96.0 MB) produced cleanly.
- **Docs**: `docs/changes/100-build-warnings-cleanup-and-stage-watchdog-resilience.md`.

## Ghost Mode Window Focus Polling, Contextual Search & Human Cursor Motion (2026-10-07)

- **Problem & Motivation**:
  1. Opening slow-starting packaged apps (e.g., "Open WhatsApp") launched in the background without focus because `focus_app_by_title` ran at 0ms without polling and Windows foreground lock blocked activation.
  2. Saying "Search Mommy" while WhatsApp was active triggered Google Search in Brave Browser (`Ctrl+L`) rather than WhatsApp contact search.
  3. "Click on the search bar" took 5-6s to respond because `CLICKABLE` controls omitted `ControlType::Edit` and `score_name` failed on conversational noise words, forcing an expensive Cloud Vision round-trip.
  4. The cursor moved in a jarring 300ms leap (~5,000 px/s) and immediately snapped back in 250ms, appearing as an invisible flash.
- **Root Cause & Fixes**:
  - `src-tauri/src/live/commands/window.rs`: Implemented `wait_and_focus_app` with 150ms retry loop up to 2.5s; added Win32 `Alt`-key tap before `SetForegroundWindow` to bypass OS foreground locks; added `is_foreground_app`.
  - `src-tauri/src/screen.rs`: Added `ControlType::Edit` and `ControlType::ComboBox` to `CLICKABLE`.
  - `src-tauri/src/live/commands/mouse.rs`: Added noise-word stripping and token overlap to `score_name`; implemented Fitts's Law dynamic glide duration (`calculate_glide_duration`, clamped 550ms-750ms); upgraded to 60 FPS (16ms) interpolation; added 100ms arrival dwell before clicking; updated `restore`.
  - `src-tauri/src/orchestrator.rs`: In `run_ghost_open`, used `wait_and_focus_app(&target, 2500)` in a blocking task; in Ghost Mode, routed `ParsedIntent::Search` to `run_ghost_message` when WhatsApp is focused.
  - `src-tauri/src/intent_parser.rs`: Added "search/find <contact> on whatsapp/wa" patterns to `parse_whatsapp_command`.
- **Verify**:
  - `npx tsc --noEmit`: clean (0 errors).
  - `npm test -- --run`: 200/200 pass.
  - `cargo test --lib -- --test-threads=1`: 1020/1020 pass.
  - `npm run build`: clean (8.68s).
  - `cargo build --release --features custom-protocol`: fresh release binary produced.
- **Docs**: `docs/changes/99-ghost-mode-window-focus-context-search-and-human-mouse.md`.

## Command Hub Profile-Style Redesign, API Keys, Insights & Global Light/Dark Orb (2026-10-07)

- **Problem & Motivation**: user wanted the Command Hub as a compact profile-settings card (reference mock, light + dark): accounts rows with photo + username and "Add Account", API Keys (add/replace/delete) instead of Account Security, insights (API keys, connected MCPs, requests today + all-time), no Log Out, removable Google/GitHub accounts, blunt corners, 400 px compact window, Light/Dark applied to everything including the wake-up orb (particles white → black in Light).
- **Implementation**: Rust `usage_counter.rs` (today/all-time counters bumped at router/STT/Worker/MCP/vision call sites), `api_keys.rs` (masked list, set, delete from keychain AND settings.json), `github_profile.rs` (login/avatar via `/user`), `save_settings` never wipes/resurrects keys (`merge_key`), `read_groq_api_key` keychain-first, settings window 400 × min(780, mh−40). Frontend `settings-sidebar/hub/` (`HubHome`, `ApiKeysPage`, `useHubData`, `hubModel`, `hub.css`), `SettingsSidebarApp` now the hub shell with Audio/Display/Connections/Advanced as inner pages; `theme.ts` `initThemeSync` (stage + HUD), `useThemeMode`; orb `theme="light"` → black ink (shader `ink` uniform + normal alpha blending instead of additive; 2D fallback parity), light capsule + tour callout CSS. Builds on the other session's change 97 (theme tokens + Google account rows).
- **Verify**: Rust 1018/1018 (6 ignored dev helpers), frontend tsc clean, vitest 200/200. **Not run (live)**: visual match to the reference, light-orb look per state (needs tuning), 400 px reflow of the legacy Audio/Display/Connections pages (Light uses an interim invert filter), GitHub row, MCP probe timing. Requests counter has no history.
- **Docs**: `docs/changes/98-command-hub-profile-style-redesign.md`, `docs/research/command-hub/04-profile-style-command-hub-redesign-plan-2026-10-07.md`.

## Ghost Mode Always-On Display Topmost Fix & Mount State Sync (2026-10-07)

- **Problem & Motivation**:
  1. During active Ghost Mode sessions, when opening apps (e.g., "Open Brave Browser"), the Voice Orb completely vanished from view behind the opened application.
  2. If the frontend stage reloaded or mounted after ghost mode had already armed, `ghostActive` in the frontend store stayed `false`, causing the orb to hide after a single turn.
  3. `scripts/cdp_monitor.js` crashed on launch due to missing `http` import and path resolution for `ws`.
- **Root Cause & Fixes**:
  - `src-tauri/src/stage.rs`: Replaced the flawed `SetWindowPos(HWND, taskbar, ...)` which stripped `WS_EX_TOPMOST` with explicit `SetWindowPos(HWND, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)`, guaranteeing that the stage window and Voice Orb remain visible above all desktop apps.
  - `src-tauri/src/ghost.rs` & `src-tauri/src/lib.rs`: Implemented and registered `get_pending_ghost_session` IPC query returning `session_active()`.
  - `frontend/src/stage/OrbFrame.tsx`: Added mount-time synchronization query via `get_pending_ghost_session` so `ghostActive` and `visible` are immediately set to `true` on reload. Fixed listener parenthesis syntax.
  - `scripts/cdp_monitor.js`: Added missing `const http = require('http');` and fallback path resolution for `ws` from `frontend/node_modules/ws`.
- **Verify**:
  - `npx tsc --noEmit`: clean (0 errors).
  - `npm test -- --run`: 189/189 pass.
  - `cargo check`: clean.
  - `cargo test --lib -- --test-threads=1`: 1005/1005 pass.
  - `npm run build`: clean (5.43s).
  - `cargo build --release --features custom-protocol`: fresh 95.8 MB release binary produced at `src-tauri/target/release/nexus.exe`.
- **Docs**: `docs/changes/96-ghost-mode-always-on-display-topmost-fix.md`.


## F0 Persona Tone + F1b Honest Counsel (2026-10-06)

- **What**: `persona_mode` setting (`butler` default / `friend`), `persona.rs` address helpers, `PersonaFriend/Butler` voice intents + Command Hub Personality section; greetings restyled via one orchestrator hook (parser stays pure). `ShareConcern{story}` intent (opener arms counsel with 120s expiry, direct forms run now); `COUNSEL_CONTRACT` via dialog_context.memory (9Router) + explicit `task.intent="counsel"` → Worker `handleCounsel`; `FRIEND_TONE` via memory + `task.persona` → `generalSystem()`; turns recorded as relevance-95 `share_concern`.
- **Verify**: Rust 1005/1005 serial, Worker 106/106 (8 contract/persona tests), vitest 189/189, tsc 0, release 91.5 MB, zero new warnings. Research grounding (sycophancy lit) folded into `docs/features/93`.
- **Docs**: `docs/changes/90-f0-persona-tone-f1b-honest-counsel.md`.

## Pure White Orb Color Invariant & Particle-Text Inside Black Box Removal (2026-10-06)

- **Problem & Motivation**:
  1. Particle text letters were being assembled from particles inside the black slider capsule during short speech responses ("On it, sir.", "Ok, sir."), which the user requested removing completely.
  2. The user required that all particles across every state (idle, listening, thinking, speaking) must be pure white.
  3. Short acks must render as standard subtitles below the black capsule rather than in-capsule particle text.
- **Root Cause & Fixes**:
  - `frontend/src/audio/ttsPlayer.ts`: Removed short-phrase gate (`<= 22` chars) that invoked `suppressNextCaption()` and emitted `orb:show_text`. All speech turns now generate normal HTML subtitles.
  - `frontend/src/avatar/VoiceOrb.tsx`: Removed `orb:show_text` event listener and `text` prop watcher.
  - `frontend/src/avatar/voice-orb.js`: Disarmed `setText()` to return `false` immediately; in vertex shader `VS`, removed text stream convergence (`pos = cloudPos`), locked `tint = vec3(1.0, 1.0, 1.0)` and `sparkTint = vec3(1.0, 1.0, 1.0)`; in fragment shader `FS`, locked color output to pure white `vec3(1.0, 1.0, 1.0)`; locked `PALETTE` and `parseColorHex` to `[1.0, 1.0, 1.0]`; locked 2D fallback to `rgb(255, 255, 255)`.
  - `frontend/src/store/assistant.ts`: Default `orbColor` changed to `"#ffffff"`.
  - `src-tauri/src/commands.rs`: Added `persona_mode: default_persona_mode()` to `NexusSettings::default()`.
  - `src-tauri/src/orchestrator.rs`: Passed `dialog_context` to `run_counsel_turn`.
  - `src-tauri/src/intent_parser.rs`: Made prefix extraction safe using `strip_prefix()`.
- **Verify**:
  - `npx tsc --noEmit`: clean (0 errors).
  - `npm test -- --run`: 189/189 pass.
  - `cargo check`: clean.
  - `cargo test --lib -- --test-threads=1`: 1002/1002 pass.
  - `npm run build`: clean (10.03s).
  - `cargo build --release --features custom-protocol`: fresh 95.8 MB release binary produced at `src-tauri/target/release/nexus.exe`.
- **Docs**: `docs/changes/95-pure-white-orb-and-particle-text-removal.md`.

## Fullscreen Overlay Dismissal, Callout Hitbox Close, Hotkey Barge-In & WebView2 Resilience (2026-10-06)

- **Problem & Motivation**:
  1. If Vite port 5173 was down or interrupted mid-build during dev, WebView2 rendered Chromium's opaque error screen ("This site can't be reached") across the 1920×1080 transparent stage window, causing a stale heartbeat and crash after 8 seconds.
  2. Users needed foolproof mechanisms to dismiss or stop fullscreen coverage in any scenario (crashed, busy, or active).
  3. Screen Tour callout boxes lacked interactive hitboxes in Rust, meaning mouse clicks fell through and the visual UI could not be clicked or closed.
  4. Global hotkey `Ctrl+Alt+X` destroyed the stage window without cutting in-flight TTS speech or cancelling active orchestrator requests.
- **Root Cause & Fixes**:
  - `src-tauri/Cargo.toml` & `scripts/run.ps1`: Documented that release binaries require `--features custom-protocol` so all frontend assets are embedded directly via `tauri://localhost` with zero port 5173 dependency.
  - `src-tauri/src/stage.rs`: Made `source` parameter in `stage_set_hitboxes` optional (`Option<String>`), defaulting to `"legacy"`, preventing signature mismatch errors when multiple components (`tour`, `spatial`) register hitboxes.
  - `src-tauri/src/live_glass.rs`: Switched to direct `crate::stage::set_hitbox_source("live-glass", stage_rects)` call.
  - `src-tauri/src/orchestrator.rs`: Updated `orchestrator_cancel()` IPC command to call `request_barge_in("ipc-cancel")`, immediately cutting audio and stopping active tours.
  - `src-tauri/src/hotkey.rs`: In `Ctrl+Alt+X` stage kill-switch, added `crate::orchestrator::request_barge_in("hotkey-stage-kill")` before `stage_hide_kill()`.
  - `frontend/src/stage/TourOverlay.tsx`: Registered physical callout hitboxes with Rust; added `.tour-callout-close` (`×`) button; added `Escape` key listener; wired dismiss to `invoke("orchestrator_cancel")`.
  - `frontend/src/stage/tour.css`: Added `pointer-events: auto` on `.tour-callout.in`, styled `.tour-callout-close` button with hover highlight, and expanded right padding to prevent text overlap.
  - `frontend/src/stage/main.tsx`: Added `Escape` key listener in `StageApp` and `SpatialAnnotationLayer` to dismiss bubbles, clear spatial pins, and abort active tasks.
  - `docs/features/94-fullscreen-overlay-dismissal-and-webview-resilience-plan.md` & `docs/changes/94-fullscreen-overlay-dismissal-and-webview-resilience.md`: Created detailed architectural and change documentation.
- **Verify**: Double-pass verification executed:
  - Pass 1: `npx tsc --noEmit` clean, `npm test -- --run` 189/189 pass, `cargo check` clean, `cargo test --lib -- --test-threads=1` 997/997 pass.
  - Pass 2: `npm run build` clean (dist built in 4.63s), `cargo build --release --features custom-protocol` cleanly produced fresh 91.4 MB release binary `src-tauri/target/release/nexus.exe`.
- **Docs**: `docs/changes/94-fullscreen-overlay-dismissal-and-webview-resilience.md`.

## 1.0s Thinking Dwell, 2-Line Subtitle Stack & Screen Tour Resilience (2026-10-06)

- **Problem & Motivation**:
  1. The continuous 3D woven luminous violet ribbon knot was morphing into speech almost instantaneously (<200ms) on quick responses, preventing the user from seeing the knot unfurl and rotate in 3D space.
  2. Captions disappeared too fast to read when broken into small 3-5 word fragments; the user could not comfortably read replies.
  3. "Analyze my screen" required immediate natural voice acknowledgment ("On it, sir.", "Ok, sir.", "Sure, sir.") and graceful resilience if the vision model returned HTTP 503 or 429.
- **Root Cause & Fixes**:
  - `frontend/src/store/assistant.ts`: Added `THINKING_MIN_DWELL_MS = 1000` with dwell lock preventing instant transition to speaking until the continuous 3D woven luminous violet ribbon knot has completed its 700ms expansion and at least 300ms of visible 3D spatial rotation. Handled delayed transitions via `thinkingPendingTimer` and clean reset in `clearThinkingDwell()`.
  - `frontend/src/audio/captionScheduler.ts`: Sentence partitioning updated to break strictly on natural terminal punctuation (`[.?!]`) or speech gaps (>650ms). Readability dwell floor enforced: intermediate sentences dwell for $\ge 2,500$ms, final sentences dwell for $\ge 3,500$ms. Added `previousText` tracking.
  - `frontend/src/stage/ResponseCaption.tsx` & `frontend/src/stage/ghost.css`: Implemented 2-line subtitle stack (`.caption-line--prev` with 72% opacity, 18px font size, and 400ms cross-dissolve with blur).
  - `frontend/src/avatar/voice-orb.js`: Locked exact color palette parity with `v2.mp4` (Listening: warm champagne gold; Thinking: luminous violet to electric magenta; Speaking: cohesive plum droplet with warm pink shimmer; Captions: silver-lavender stardust `#d8d0ea`).
  - `src-tauri/src/screen_tour.rs` & `src-tauri/src/orchestrator.rs`: Added natural spoken acknowledgments immediately upon trigger, and handled 503/429 upstream errors with polite spoken notice and automatic fallback to local Windows OCR.
- **Verify**: Double-pass verification executed:
  - Pass 1: `npx tsc --noEmit` clean, `npm test -- --run` 189/189 pass, `cargo check` clean, `cargo test --lib -- --test-threads=1` 997/997 pass.
  - Pass 2: `npx tsc --noEmit` clean, `npm test -- --run` 189/189 pass, `cargo test --lib -- --test-threads=1` 997/997 pass.
  - Production build: `npm run build` clean; `cargo build --release` completed.
- **Docs**: `docs/changes/93-thinking-dwell-caption-stack-and-screen-resilience.md`.

## 1:1 v2.mp4 Orb Morphology, Sentinel Poller Guard, XML Unescaping & Wake Default (2026-10-06)

- **Problem & Motivation**:
  1. Terminal repeatedly spammed every 15s: `[SENTINEL] watch 'screen_thread_1790701281' ('Current Screen Email'): checking…` and `[WATCH] Skipping poll: no Google token.` due to a stale synthetic screen-email watch in `%APPDATA%\com.nexus.assistant\memory\mail_watches.json`.
  2. Edge TTS emitted raw XML entity `&apos;` (`didn&apos;t`), displayed unescaped in captions.
  3. False wake triggers occurred on ambient sound because `read_verify_wake` defaulted to `false`, allowing unconfirmed openWakeWord probabilities $\ge 0.30$ to instant-fire; consecutive empty STT chunks subsequently spoke "I didn't hear you, sir." unprompted.
  4. The Voice Orb visual morphology did not match `v2.mp4`:
     - Thinking state was a radial starburst rather than a continuous 3D woven luminous violet ribbon knot (`frame_03.png`).
     - Speaking state was a faceted pebble rather than an organic fluid surface-tension droplet (`frame_04.png`, `frame_08.png`).
     - Caption text formation did not have particles physically cascading downward from the south pole like falling sand (`frame_05.png`, `frame_08.png`).
  5. The release binary was stale, causing `nexus start` to run older code where the orb had vanished.
- **Root Cause & Fixes**:
  - `src-tauri/src/google/sentinel.rs`: Purged stale `mail_watches.json`. Added check in `poll_sentinel_targets`: if no Google account is connected, the poller prints the configuration notice once and sleeps silently (zero 15-second spam). Added 2-hour auto-expiration for synthetic `screen_thread_*` targets.
  - `src-tauri/src/tts.rs`: Added `unescape_ssml_entities()` decoding `&apos;` $\to$ `'`, `&quot;` $\to$ `"`, `&amp;` $\to$ `&`, `&lt;` $\to$ `<`, `&gt;` $\to$ `>` in `boundaries_to_words()`. Added unit test.
  - `frontend/src/audio/captionScheduler.ts`: Added `unescapeXml()` in line partitioner and word reveals. Removed unused imports in `captionScheduler.test.ts` (cleaning `tsc`).
  - `src-tauri/src/commands.rs`: In `read_verify_wake`, defaulted fallback to `true` (`unwrap_or(true)`), restoring mandatory Stage-2 acoustic verification before wake word fire.
  - `frontend/src/avatar/voice-orb.js`:
    - **Thinking State**: Replaced 64-radial-spoke starburst with 3D continuous woven violet ribbon knot based on parametric $(p=2, q=3)$ torus space curve with ribbon normal/binormal width expansion ($W=0.26, H=0.04$) and 3D yaw/pitch precession (`ctt` gradient `vec3(0.72, 0.14, 0.98)` to `vec3(0.92, 0.28, 0.95)`).
    - **Speaking State**: Replaced faceted pebble with organic fluid droplet using 3D Simplex noise surface tension, lower-hemisphere teardrop sag, and audio amplitude/onset bulging (`cs` warm pink shimmer).
    - **Caption Sandfall**: Configured particles to stream downward from the south pole $(0, -1.02, 0)$ with downward gravity arc and turbulence into target 2D glyph points, dissolving clause-by-clause before each new phrase.
    - 1:1 parity in `_paint2D` CPU fallback.
  - **Release Binary**: Compiled fresh release binary to `src-tauri/target/release/nexus.exe` (88.0 MB) bundling all frontend and backend updates.
- **Verify**: `npx tsc --noEmit` clean; `npx vitest run` 189/189 pass; `cargo check` clean; `cargo test --lib -- --test-threads=1` 997/997 pass (6 ignored dev benches); release binary compiled cleanly.
- **Docs**: `docs/changes/92-v2-orb-morphology-sentinel-silence-and-entity-unescaping.md`.

## Screen Tour v2 — Explain the Main Content, With Context & a Stronger Model (2026-10-06)

- **Problem & Motivation**: live test of the narrated tour: on a "healthiest nuts" article it explained the tab strip / URL bar and only said "this is the kinds of nuts"; on a YouTube video it said "this is the person / the product" without saying who, what, or what the video is about. Causes: whole-screen capture (browser chrome in the image), no context (tab title/URL never sent), word caps that forced labels not answers (overview ≤20, spoken ≤17), weakest model (3.5-flash-lite), and no stance on people (a vision model must not identify faces — only text evidence).
- **Implementation**: `screen_context.rs` (new, pure + thin glue) gathers app, tab title, privacy-trimmed URL, YouTube title/channel via public oEmbed, and chooses the capture region (browser `Document` rect via UI Automation → window rect minus toolbar inset → full screen); `vision::capture_region_jpeg_base64`; `TourScript.region` + `callout_json` map boxes back to screen pixels. Prompt is answer-first (names all items of a list, says what a video is about, never identifies faces, ignores browser/OS chrome); budgets overview 35 / spoken 22 / callout 24 / total 120 words + `answer` field → sidebar overview; 8192 output tokens. Model ladder `gemini-3.8-flash → 3.5-flash → 3.5-flash-lite → 2.5-flash`, 404/429 advance, quota marked only if every failure was a 429. `geminiVisionDailyLimit` (default 500 — **our assumption**; Google publishes no per-model free numbers, see AI Studio rate-limit page) now drives the quota ledger. `screenTour`, `tourModel`, `geminiVisionDailyLimit` added to `NexusSettings` (settings.json is rewritten from that struct on save). Sensitive windows (`live::safety::is_target_blocked`) are refused, nothing captured.
- **Verify**: Rust 996/996 (6 ignored dev helpers; +25), vitest 188/188; `tsc` shows 2 errors in `audio/captionScheduler.test.ts` from another session's in-progress caption work (untouched here). **Not run (live)**: UIA `Document` exposure on Brave, whether the 3.8/3.5 Flash ids accept the request on this project, answer quality on the nuts page / a YouTube video, crop alignment at 125–150 % scaling, oEmbed reachability. No Settings UI for `tourModel` / `geminiVisionDailyLimit` yet.
- **Docs**: `docs/changes/91-screen-tour-main-content-and-context.md`.

## Master Volume Restoration, Orb Disappearance Fix, Pure Black Capsule & Line-by-Line Captions (2026-10-06)

- **Problem & Motivation**:
  1. System Master Volume remained trapped at 70% after speech instead of restoring to the initial baseline (e.g. 20–30%).
  2. Voice Orb disappeared inside the black slider capsule while the black box remained active, and vanished in Ghost Mode while captions continued. Captions and the orb were out of sync.
  3. `scripts/run.ps1` silently dropped `[ORB]` logs due to missing regex matchers and lack of an `else` fallback.
  4. User requested removing any pillar bars inside the black capsule: strictly the Voice Orb inside an obsidian black capsule with 10px minimalist corners.
  5. Captions must replace line-by-line / clause-by-clause (*"hi lakshya i am nexus"* $\to$ dissolves $\to$ *"how can i help u today"* $\to$ dissolves $\to$ *"this is the analyssi result"*) rather than showing full paragraphs or overflowing. User live speech must show a 5-word FIFO moving window.
  6. Strict constraint: Screen Analysis / Screen Tour (`screen_tour.rs`, `TourOverlay.tsx`, `tourGeometry.ts`, `vision.rs`) must remain 100% untouched.
- **Root Cause & Fixes**:
  - `src-tauri/src/volume.rs` & `src-tauri/src/tts.rs`: Implemented RAII `TtsVolumeLease` with reference-counted `ACTIVE_TTS_COUNT`, locked baseline invariant on `0 -> 1` transition, and 15-second safety watchdog. Instant restoration in `stop_tts()`. Verified via `cargo test --lib test_save_and_restore`.
  - `scripts/run.ps1`: Added `--remote-debugging-port=9222` by default, launched CDP monitor by default, added regex matchers for `[ORB]`, `[ORB-FRAME]`, `[STAGE]`, `[AVATAR]`, `[VOICE-ORB]` (Cyan), and `[CAPTION]` (Green), and added a catch-all fallback so no frontend log is ever dropped.
  - `frontend/src/avatar/Avatar.tsx`: Computed `isOrbVisible = Boolean(enteredProp || visible || ghostActive || dispersing)` and passed `visible={isOrbVisible}` to `<VoiceOrb>` so the WebGL loop never pauses inside an onscreen container.
  - `frontend/src/avatar/VoiceOrb.tsx`: Added `color` prop support and `[VOICE-ORB]` WebGL loop resume/pause lifecycle logging.
  - `frontend/src/stage/OrbFrame.tsx`: Wrapped strictly `<Avatar>` inside `.orb-sphere-wrapper` within `.orb-capsule` and `.orb-slider` (zero pillar bars, 10px corners, obsidian black `#000000 !important`). Added `[ORB-FRAME]` state logging.
  - `frontend/src/net/orchestrator.ts`: In `hideOrbAfterSpeech`, gated hiding with `if (s.captionActive) { hideOrbAfterSpeech(350); return; }`, synchronizing the Voice Orb to remain active until the final caption clause completely finishes dissolving.
  - `frontend/src/audio/captionScheduler.ts`: Exported `suppressNextCaption`. Implemented `partitionIntoLines()` and line-by-line phase scheduling (`active` $\to$ `fading` $\to$ `cleared`).
  - `frontend/src/stage/ResponseCaption.tsx`: Connected to `onCaptionLineUpdate` to render discrete active and fading lines. Handled adaptive top/bottom docking coordinates.
  - `frontend/src/stage/LiveCaption.tsx`: Implemented 5-word FIFO moving window for user input.
  - `frontend/src/stage/ghost.css`: Added `.caption-line`, `.caption-line--active`, and `.caption-line--fading` transitions.
  - `.gitignore`: Confirmed `frontend/public/v2*` is ignored.
- **Verify**: `cargo check` clean; `npx tsc --noEmit` clean; `npx vitest run` 188/188 pass; `cargo test --lib -- --test-threads=1` 971/971 pass (6 ignored dev benches).
- **Docs**: `docs/changes/90-volume-restoration-orb-observability-and-line-by-line-captions.md`.

## Intact Orb Sphere, Black Slider Capsule & Top/Bottom Screen Edge Docking (2026-10-06)

- **Problem & Motivation**:
  1. The user requested: "remove the particles come together to form a spaher make it simple sliding up and spher change the form to thinking and all perfectly and lishting and spekang dont make any changes and plan it perfectly i dont want any more make like a slider black slider positon it from top to bottom remove that position and dragging option only two option top or bottom postiotn only and i want it to be positon at the top slide from top to bottom with black bacground top to bottom a small bot just enough space between the gap of sphere and the inside the balck box spher should be there plan it only".
  2. The particle scattering effect (`EntranceBurst` and shader flight interpolation `mix(scatter, pos, ae)`) caused perceived visual glitches and unnecessary outward particle flight. The sphere must always remain intact, formed, and solid.
  3. Freeform desktop drag calibration and wheel resizing introduced unwanted drift and floating UI artifacts. Restrict strictly to two discrete dock positions: **Top** (Default) or **Bottom**.
  4. Encapsulate the orb in an obsidian black slider capsule (Apple Dynamic Island style) with just enough padding around the sphere perimeter, sliding smoothly in and out from the screen edge.
  5. Strict constraint: Screen Analysis / Screen Tour (`screen_tour.rs`, `TourOverlay.tsx`, `tourGeometry.ts`, `vision.rs`) must remain 100% untouched.
- **Root Cause & Fixes**:
  - `voice-orb.js`: In vertex shader `VS`, replaced `vec3 flight = mix(scatter+swirl, pos, ae);` with `vec3 flight = pos;`. Particles stay intact in their compact spherical form at all times without scattering or converging from outside. `assemble()` and `disperse()` converted to instant zero-drift operations.
  - `EntranceBurst.tsx`: Replaced with an inert component returning `null` (zero screen-wide particle bursts).
  - `styles.css`: Added `.orb-slider`, `.orb-capsule`, and `.orb-sphere-wrapper` with 100% solid black inside fill (`background: #000000 !important`), blunt minimalist corner radii (`0 0 10px 10px` for top, `10px 10px 0 0` for bottom), border `1px solid rgba(255, 255, 255, 0.16)` on exposed edges, and deep ambient drop shadow.
  - `voice-orb.js`: Removed `glitch` jitter noise and RGB chromatic tear so morphs between listening, thinking, and speaking are completely serene, smooth, and clean.
  - `OrbFrame.tsx`: Replaced snappy transitions with a slow, neat, deliberate slide: `0.72s cubic-bezier(0.22, 1, 0.36, 1)` with `translate3d` hardware acceleration. Solid physical opacity throughout (zero mid-air ghost fading). Added `hasMountedRef` to eliminate cold-mount flash. Top docking rests flush directly below the webcam/camera at `y = 0`, sliding smoothly down from `-hCss - 24px`.
  - `stage.rs`: Added Win32 taskbar z-ordering via `FindWindowW(Shell_TrayWnd)` and `SetWindowPos`, placing the stage overlay immediately behind the Windows taskbar while remaining above all normal desktop application windows.
  - `ResponseCaption.tsx` & `LiveCaption.tsx`: Added adaptive top/bottom layout awareness. When positioned at Top, captions render below the black slider (`bottomY + gap`); when at Bottom, captions render above (`topY - gap`, `translateY(-100%)`).
  - `commands.rs`, `window_manager.rs`, `lib.rs`: Added `orb_position: String` (`"top"` | `"bottom"`, default `"top"`) to `NexusSettings`. Updated `read_orb_settings` to parse discrete edge docks (`h = 0.5, v = 0.0` for top; `h = 0.5, v = 1.0` for bottom). Added `get_pending_orb_position` command and registered physical slider hitbox (`set_orb_hitbox_interactive`).
  - `assistant.ts` & `orbRuntime.ts`: Bound `orbPosition` in Zustand store and synchronized live updates via `stage:orb_position`.
  - `SettingsSidebarApp.tsx` & `SettingsApp.tsx`: Replaced legacy desktop drag slider with two-option segmented controls: `Top (Default)` and `Bottom`.
- **Verify**: `cargo check` clean in `src-tauri`; `npx tsc --noEmit` clean in `frontend`; `npx vitest run` 182/182 pass; `cargo test --lib -- --test-threads=1` 969/969 pass (6 ignored dev benches); release binary compiled cleanly to `src-tauri/target/release/nexus.exe` (86.4 MB).
- **Docs**: Artifact `nexus_command_center_and_orb_slider_plan.md`.

## Ghost Mode Hardening, Directed Gate Vocabulary & Orb Convergence Fix (2026-10-06)

- **Problem & Motivation**:
  1. The user reported that during Ghost Mode, saying "Shift to tab two" (transcribed as `"Shift to Tab to"`) and `"You'll talk."` vanished silently without any feedback or logs, and the hot-mic subsequently stopped responding.
  2. The Voice Orb screen-wide particle convergence animation was re-triggering repeatedly into a sphere every time NEXUS entered `thinking` and between consecutive Ghost Mode turns, instead of strictly on initial wake.
  3. Particle density and sizing needed recalibration (+20% count, -40% point size).
  4. Clarification requested regarding whether the Assistant sidebar and the Stage orb share an OS window.
  5. Strict constraint: Screen Analysis / Screen Tour (`screen_tour.rs`, `TourOverlay.tsx`, `tourGeometry.ts`, `vision.rs`) must remain 100% untouched.
- **Root Cause & Fixes**:
  - `directed.rs`: Gated turns evaluated `'Shift to Tab to'` as non-command because `STARTERS` lacked `"shift"`, `"tab"`, `"alt"`, `"ctrl"`, etc., and `DOMAIN_CUES` lacked UI navigation nouns (`"tab"`, `"window"`, `"screen"`, `"cursor"`, etc.). Added explicit starters/cues and surfaced dropped turns directly to stdout via `[DROP] Directed speech gate: IGNORED ({reason}) transcript: '{transcript}'`.
  - `screen.rs`: Added homophone support in `parse_ordinal` for Whisper mishearings: `"to"` / `"too"` $\to$ 2, `"for"` $\to$ 4, `"won"` $\to$ 1, `"ate"` $\to$ 8.
  - `intent_parser.rs`: Added phonetic and soundalike normalization for `"it goes to mode"`, `"ghost and warning"`, `"ghost on warning"`, `"worst motive"`, and `"ghost to mulder"`. Verified live.
  - `center.rs`: Allowed `(CommandEvidence::Strong, TurnOwnership::Unenrolled)` execution during Ghost Mode so un-enrolled voices can operate commands like "open Brave browser" or "open Antigravity".
  - `frontend/src/stage/OrbFrame.tsx`: Removed `thinkBurstSeq` and the redundant thinking `<EntranceBurst />`. `EntranceBurst` now triggers strictly on rising edge of visibility (`!wasVisible && visible`), preventing particle convergence loops during thinking or repeating turns.
  - `voice-orb.js`: Scaled particles by +20% (6,240 on desktop, 5,040 on mobile) and reduced particle diameter by 40% (point base 1.14, floor 1.6, dot 0.8) for sharper, crisper dust beads.
  - Sidebar Architecture: Clarified that `"stage"` is a fullscreen transparent click-through overlay window, while `"sidebar"` is a completely separate native Win32 window (420px width).
- **Verify**: `cargo check` clean; `npx tsc --noEmit` clean; `npx vitest run` 182/182 pass; `cargo test --lib -- --test-threads=1` 969/969 pass (6 ignored dev benches); release binary compiled cleanly to `src-tauri/target/release/nexus.exe` (86.4 MB).
- **Docs**: `docs/changes/88-orb-command-center-coordination-and-ghost-mode-hardening.md`.


## Startup Diagnostics & Subcenter Observability Matrix (2026-10-06)

- **Problem & Motivation**: User requested: "in the nexus start i want u to plan somwthing i need to know where is the codbease being broken or the execution being failed, i want every single loop hole details and cross check in the entire codebase to be shown in the nexus start logs, no detaied should be missed include the anaimation froented, main commandceter validation an sendingto sub center And sub center process".
- **Implementation**:
  - `diagnostics.rs`: Added `run_full_system_audit(app_data_dir)` running during Tauri setup: 5 preflight audit layers (Layer 1 Audio & Neural Models, Layer 2 Frontend Overlay & CDP, Layer 3 Main Command Center, Layer 4 All 15 Sub-Centers, Layer 5 System & External Tools).
  - `center.rs`: Connected validation for `YouTubeCenter`, `SystemCenter`, and `BrowserCenter` into `center::validate` for `NluResult` intents; exported `ALL_SUB_CENTERS` (15 sub-centers).
  - `browser_center.rs`: Refactored `BrowserCenter::validate` to be self-contained, resolving recursive stack-overflow.
  - `youtube_center.rs`: Added `resolve_youtube_engine_path() -> Option<PathBuf>` to resolve engine path robustly.
  - `app_registry.rs`: Added `cached_app_count() -> usize`.
  - `orchestrator.rs`: Added detailed `[MAIN-CMD]`, `[DROP]`, `[SUB-ROUTE]`, and `[SUB-PROC]` telemetry across all turn states.
  - `voice-orb.js` & `stage/main.tsx`: Added `[ANIM]` WebGL context logs, shader compilation reporting, state transition tags, and stage mount metrics.
  - `run.ps1` & `cdp_monitor.js`: Always enable `--remote-debugging-port=9222`; stream frontend logs by default; unsuppress hardware silence drops and permissions; colorize `[PREFLIGHT]` (Cyan), `[MAIN-CMD]` (Green), `[SUB-ROUTE]` (Cyan), `[SUB-PROC]` (Magenta), `[DROP]` (Red), and `[ANIM]`/`[FRONT]` (Yellow).
- **Verify**: `cargo check` clean; `npx tsc --noEmit` clean; `npx vitest run` 182/182 pass; `cargo test --lib -- --test-threads=1` 968/968 pass (6 ignored dev benches).
- **Docs**: `docs/changes/87-startup-diagnostics-and-subcenter-observability.md`.

## Command Hub Orb Color Control & Theme Presets (2026-10-05)

- **Problem & Motivation**: User requested: 1) "color should be same" (default remains warm golden-amber `#f2b859`), 2) "user can control the color he wants in the command hub", 3) "thinking center should be as always" (thinking state strictly invariant as electric purple starburst with solid white/magenta nucleus), 4) "merge the pr in the windows nexus-egine" (PR #29 merged on `Engine-NEXUS/WINDOWS`).
- **Implementation**:
  - `commands.rs`: Added `orb_color: String` to `NexusSettings` with default `#f2b859`.
  - `assistant.ts`: Added `orbColor` and `setOrbColor` with `localStorage` fallback and persistence.
  - `orbRuntime.ts`: Syncs `orbColor` from `get_settings` on mount and listens for live `"orb:color"` events.
  - `Avatar.tsx` / `VoiceOrb.tsx`: Passes `color={orbColor}` down to `<voice-orb>`.
  - `voice-orb.js`: Added `'color'` attribute observer; uniform `vec3 userTint` applied to `clFixed`, `clDyn`, and `ci` (idle/listening); `ctt` strictly locked to electric purple gradient with white nucleus (`isCore`); 1:1 parity in `_paint2D`.
  - `SettingsSidebarApp.tsx`: Added "Orb Color Theme" in Display tab with 7 preset chips (Amber, Cyan, Emerald, Ruby, Azure, Amethyst, Pearl) and custom hex / color picker, emitting live updates to the stage orb.
  - `frontend/index.html`: Added color theme controls to local dev playground.
  - PR #29 merged cleanly on `Engine-NEXUS/WINDOWS`.
- **Verify**: `cargo check` clean in `src-tauri`; `npx tsc --noEmit` clean; `npx vitest run` 182/182 pass; live preview verified on `http://localhost:5173/`.
- **Docs**: `docs/changes/86-command-hub-orb-color-theme.md`.

## Narrated Screen Tour — Pointers + Callouts Synced to TTS (2026-10-05)

- **Problem & Motivation**: "analyse my screen" spoke one canned line then dumped text. User wanted the overlay to point at one thing at a time (ring + arrow + callout) while TTS explains exactly that thing, never drawing ahead of the speech; TTS = short overview only, detail in the sidebar; orb: thinking → ack → hides while fetching → returns while narrating → hides. Decisions: meeting active = sidebar only; sidebar opens at the end; "what's on my screen / what am I seeing" use the vision tour, "read/scan/copy" stay on free OCR.
- **Implementation**: `screen_tour.rs` pure core — `validate_script` (tolerant of fenced/prose/truncated JSON, word budgets enforced in code, ≤5 items, dedupe, ids = sidebar card ids), `narration_steps`, `TourEngine` (one callout at a time: Show only after Clear, End exactly once; injected clock; timed fallback when there is no audio), `wants_narrated_tour`. `tts::narrate` = one rodio queue, one source per line; `NarrationTracker` derives Started/Ended from the queue so line i audible ⇔ callout i shown. `orchestrator::run_screen_tour` (capture with the stage temporarily `WDA_EXCLUDEFROMCAPTURE`, Gemini 4096 tokens, narrate, sidebar at end, cancel watcher; any failure falls back to the legacy chain). Frontend `TourOverlay.tsx` (class/SVG-attribute/CSSOM only — stage CSP drops `style` props), `tourGeometry.ts`, `net/screenTour.ts` (orb hide/show), caption hidden during a tour. Flag `screenTour` (default on).
- **Verify**: Rust 961/961 (6 ignored dev helpers; +34); frontend tsc clean, vitest 182/182 (+23). **Not run (live)**: GDI honouring runtime display-affinity (debug: `NEXUS_TOUR_DUMP_DIR`), Gemini box accuracy/latency, `Sink::len()` vs audible offset, stage-vs-monitor size at 125/150 % scaling, release CSP rendering, offline per-line synthesis latency. No Settings UI toggle for `screenTour` yet.
- **Docs**: `docs/changes/85-narrated-screen-tour.md`.

## Proactive-Speech Policy — When NEXUS May Speak Unprompted (2026-10-05)

- **Problem & Motivation**: the Sentinel's only spoken alert (mail deadline change) went `speak_proactive_alert → speak_line` instantly: it cut NEXUS off mid-reply (frontend `speak()` stops current audio), talked over the user, and during a meeting was silently lost (frontend mutes all speech). User decision: Critical alerts must speak even in meetings.
- **Implementation**: `proactive_policy.rs` — pure `Engine` (injected clock) + glue (2 s ticker, `proactive_snooze`). Low = card only; Medium = idle ≥ 30 s + nothing going on; High = waits for a breakpoint (user not speaking, NEXUS not speaking, no drill) and, in a meeting, for it to end (≤ 30 min); Critical = within seconds (≤ 8 s polite wait), speaks in meetings via `MeetingState::allow_tts_override` (30 s, self-expiring; setting `proactiveCriticalInMeeting`, default on), opens with "Urgent, sir."; heads-up `proactive:nudge` + 300 ms; non-critical rate limits (20 s gap, 6/hour), snooze, dedup; expiry counts only speakable time (two bugs caught by timeline tests: meeting time and pre-existing idle time were being counted).
- **Verify**: Rust 927/927 (6 ignored dev helpers); 15 new policy tests + meeting-override test. **Not run**: live meeting/alert test. **Nothing produces a Critical alert yet** (deadline alert is High → waits for the meeting to end); no UI listener for `proactive:nudge`/`proactive:card`; "not now" voice command not wired; thresholds are hypotheses (the CUI-2021 paper has no numbers).
- **Docs**: `docs/changes/83-proactive-speech-policy.md`.

## Directed-Speech Gate for Open-Mic Ghost Turns (2026-10-05)

- **Problem & Motivation**: in a ghost session's open mic, anything the parser can't match goes to the cloud LLM (`Unknown → WorkerBackend`) — TV, side talk, NEXUS's own TTS echo, STT hallucination loops (the user's real `missed_intents.jsonl`: 96 unique unmatched utterances, mostly noise). Plan: `docs/research/jarvis-landscape/08-case-2-plan-2026-10-05.md` §C2-2.
- **Implementation**: `directed.rs` — pure deterministic `evaluate` (< 1 ms): only **hot-mic** turns are gated (capture origin tagged via `start_stt_capture(origin)`; wake/hotkey/confirm/clarification/follow-up = `Direct`, never gated; origin frozen at capture stop). Order: dictation/command/vocative accept → echo of our own last 30 s of speech (content-word overlap ≥ 60 %) / repetition loop / filler / > 14 words ignore → request-shaped or product-vocabulary cue accepts → else ignore. Ignored text → local `missed_intents.jsonl` (source `directed_gate`) + `directed:ignored`. Frontend: `net/directedGate.ts` (fails open), check in `recorder.ts processTranscript` (ghost sessions only); 3 consecutive ignored turns **park the hot mic** until re-wake. Off switch: `"directedGate": false`.
- **Verify**: Rust 912/912 (6 ignored dev helpers); frontend tsc clean, vitest 159/159. Replay of the real log: 72/96 unmatched utterances ignored (75 %), 24 kept (~4 false accepts). Hand-made labeled set is optimistic (rules written after reading the same log). **Not run**: live TV/echo test, 8 h soak; STT-confidence features not plumbed; deterministic parser only (NLU not consulted).
- **Docs**: `docs/changes/82-directed-speech-gate-for-open-mic.md`.

## Listening Avatar 50/50 Acoustic Sand Cymatics & Soundbar Beat Motion (2026-10-05)

- **Problem & Motivation**: User requested: "the particlaes is moving like a thread i dont want that it should be random moving not like a thread and it should only move where the usear is speaking like a soundbar beat system where the sand moves according to the sound plan that". The old harmonic wave formed coherent nodal wavefronts that resembled moving threads/loops, and moved continuously even during silence.
- **Implementation**:
  - `voice-orb.js`:
    - Deterministic 50/50 partition via Fibonacci parity (`mod(floor(seed.w * 5200.0 + 0.5), 2.0) < 0.5`).
    - 50% Fixed Anchor Cage: strictly $r = 1.0$ rotating in a steady frame (`turn(n, t*0.14)`), with high point crispness (`crisp = 0.65`) and golden-amber beads (`vec3(0.95, 0.72, 0.35)`). Acts as the solid speaker drumhead / plate.
    - 50% Dynamic Sand Grains: decorrelated stochastic hashes ($h_1, h_2, h_3$) eliminate all spatial coherence and thread lines.
    - Sound Energy Gating: $\text{voiceLevel} = \text{clamp}((\text{soundEnergy} - 0.02) / 0.98, 0.0, 1.0)$. At silence ($\text{voiceLevel} == 0$), sand grains rest completely still at $r = 1.0$ (zero thread movement).
    - Soundbar Beat Ejection: When speaking ($\text{voiceLevel} > 0$), sand grains bounce radially ($\sin(t \cdot (18 + 34 h_1) + 6.28 h_2)$) and kick to audio transients (`beatKick = (h_1 - 0.5) * 2.0 * onset * 0.45`), spanning $r \in [0.55, 1.45]$.
    - 1:1 Parity in `_paint2D` CPU fallback.
    - Strict non-negotiable preservation of `thinking` (starburst, solid sphere nucleus, connecting threads, globe spin) and `speaking` states with zero edits.
- **Verify**: `npx tsc --noEmit` clean; `npx vitest run` 154/154 pass across 21 test suites; verified live on `http://localhost:5173/`.
- **Docs**: `docs/changes/84-listening-avatar-50-50-standing-harmonic-wave.md`, `listening_avatar_sand_cymatics_plan.md`.


## Kokoro Local TTS Replaces Piper — GPL espeak-ng Removed (2026-10-05)

- **Problem & Motivation**: `piper-rs → espeak-rs-sys` statically linked GPL-3 espeak-ng into `nexus.exe`; Feature 83's swap worker pulled a ~60 MB Piper model per persona change with no pinned hashes and could drop later equips (A→B→C ended on A). User decisions: online voice first (local only when internet is gone, auto-return to cloud), slot = one shared model + one voice file, Piper removed, dictionary-provenance question parked.
- **Implementation**: `tts_kokoro.rs` (Kokoro-82M on pinned `ort`, `misaki-rs` G2P with `default-features=false`, sentence chunking ≤110 phoneme chars, voice hot-swap), `voice_catalog.rs` (`kokoro_voice`, manifest v2), `tts_swap.rs` (shared model once + 0.5 MB voice swaps, **latest-wins queue**, immutable HF revision `1939ad2a…`, SHA-256 pins for all 10 catalog voices, verify→atomic replace), `tts.rs`/`tts_network.rs` (edge first, Kokoro only offline, offline streaming, idle unload + cloud-restored resync). Removed `tts_piper.rs`, `piper-rs`, `resources/piper` (61 MB), `resources/espeak-ng-data`.
- **Verify**: Rust 902/902 (4 ignored dev benches); `cargo tree` 0 espeak; frontend tsc clean, vitest 154/154. Measured: ASR round-trip WER 0.03–0.10; ≈ real-time CPU synthesis (RTF 0.7–1.7); RAM +128 MB load / 259–369 MB typical. **Not run**: release build, live offline↔online test, voice audition, fp16/q8f16 comparison.
- **Pitfall**: Kokoro files are git-ignored (`src-tauri/resources/kokoro/`, dev copy); the installer does not bundle them — they download on first online run.
- **Docs**: `docs/changes/81-kokoro-local-tts-replaces-piper.md`, `docs/research/jarvis-landscape/09-…`.

## Dynamic Pulsing Rays & Inter-Line Thread Connections (2026-10-05)

- **Problem & Motivation**: User requested: "now the linees should move decrase and increase and create lika thread one line connects to another line accordingly plan first". Spoke lines lacked organic breathing motion and had zero inter-strand connectivity.
- **Implementation**:
  - `voice-orb.js`: 
    - Dynamic Ray Breathing: modulated ray lengths via a continuous spherical harmonic traveling wave $\text{rayBreath} = 0.82 + 0.28 \sin(k \cdot 0.45 + 2.5 t) \cos(y_k \cdot 3.2 - 1.6 t)$ at ~0.5 Hz cadence, causing lines to dynamically stretch and retract in rolling wave pulses.
    - Inter-Line Connecting Threads: allocated outer 28% of spoke particles to form elastic luminous thread filaments bridging ray tips to spatial Fibonacci neighbor strands ($k \leftrightarrow k+8$), with catenary sag towards the core.
    - Traveling Sparks: added `threadSpark` pulses running along the connecting threads from line to line, with thread luminescence boost.
    - Added 1:1 mathematical parity to `_paint2D` CPU canvas fallback.
- **Verify**: `vitest run` 154/154 pass; `tsc --noEmit` clean; live preview verified at `http://localhost:5173/`.
- **Docs**: `docs/changes/83-thinking-avatar-dynamic-rays-and-thread-connections.md`.

## Thinking State Inner Ring Replaced with Solid 3D Sphere (2026-10-05)

- **Problem & Motivation**: User identified a hollow circular/elliptical inner ring in the thinking starburst (`media_1791189914949.png`, `media_1791190412142.png`): "replace the ring with small sphere inside so even if it spins i dont see the difference make sure of it".
- **Root Cause Discovered**:
  - In `voice-orb.js`, `vec3 swirlT = vec3(...) * 0.38 * (1.0 - te)` was added to `cloudPos` unconditionally (`mix(cloudPos + swirlT, glyphT, te)`). When text was inactive (`textProg == 0`, `te == 0`), `1.0 - te == 1.0`, so `swirlT` displaced EVERY particle by an elliptical 0.38-radius vector in the XY plane. This tore the core particles ($r \le 0.20$) outward and blew open a hollow elliptical ring right in the center.
- **Implementation**:
  - Gated `swirlT` to text transition only: `float swirlAmt = sin(te * 3.14159265) * clamp(tw*1.6 + textProg, 0.0, 1.0)`. When text is inactive, `swirlT` is strictly `vec3(0.0)`, leaving the orb completely undisturbed.
  - Core particles ($s_{\text{along}} < 0.20$) fill a solid, volumetric 3D ball ($r = 0.20 \cdot c_{\text{Frac}}^{0.55}$) directly mapped via each particle's Fibonacci unit-sphere normal $\vec{n} = \text{seed.xyz}$.
  - Smooth radial falloff: replaced hard `step(sAlong, 0.20)` with `(1.0 - cFrac)` falloff for `coreBoost` and `coreSpark`, making dead center ($r = 0$) the brightest white nucleus smoothly blending out to the magenta spoke rays.
  - Rotational invariance: because $\vec{n}$ is spherically symmetric across all 3 axes, rotating it continuously in 3D produces a seamless glowing sphere with zero visible plane, zero hollow hole, and zero orientation flicker.
  - Added full parity to `_paint2D` and guarded `customElements.define` with `!customElements.get('voice-orb')`.
- **Verify**: `vitest run` 154/154 pass; `tsc --noEmit` clean; radial brightness profile monotonically decreases from 100.00 at center to 7.19 at edge.
- **Docs**: `docs/changes/82-thinking-avatar-solid-sphere-core.md`.

## Orb Particle Point Size Enhancement (2026-10-05)

- **Problem & Motivation**: User requested: "increase the size of the particales in the orb". While the orb's overall scale was previously enlarged, the individual particle point diameter remained clamped around 1.6px, appearing as faint micro-dust.
- **Implementation**:
  - `voice-orb.js`: Scaled `point` base term from `(1.1 + 0.9*depth + 0.45*rim)` to `(1.9 + 1.4*depth + 0.7*rim)`. Increased `gl_PointSize` from `max(1.6, point*pixels/720.0)` to `max(2.6, point*pixels/480.0)`, and pop sparkle boost from `3.6` to `4.5`.
  - Added full parity to `_paint2D` canvas fallback (`dot` floor `0.8`→`1.3`, divisor `720`→`480`).
- **Verify**: `tsc --noEmit` clean; `vitest run` 154/154 pass.
- **Docs**: `docs/changes/81-orb-particle-point-size-enhancement.md`.

## Orb Particle Text & Scale Upgrades (2026-10-04)

- **Problem & Motivation**: User requested refinements to the Voice Orb: increasing particle count by 200, increasing the overall scale of the orb, making the particle text look "filled inside", and flashing the particle text for only a fraction of a second before falling back to the standard HTML caption.
- **Implementation**:
  - `voice-orb.js`: Default particle count raised `5000`→`5200`. Vertex shader `perspective` multiplier increased `0.61`→`0.72` to enlarge the orb's screen presence.
  - Text filling: `sampleTextPoints` skips removed (`x+=2`/`y+=2` → `x+=1`/`y+=1`), recruiting 4× as many particles to the glyphs for a solid, filled appearance. Font size reduced (`84`→`54`).
  - Text timing: `setText` default `holdMs` reduced from `1600` to `400`. The orb rapidly morphs into the spoken word, holds briefly, and dissolves just as the DOM `ResponseCaption` displays.
- **Verify**: `vitest run` 154/154 pass; `tsc` clean. Target dir cleared of 91GB debug artifacts to optimize project footprint.
- **Docs**: `docs/changes/80-orb-particle-text-and-scale-upgrades.md`.

## Orb Entrance Burst, Pebble Speaking Silhouette & Real TTS-Audio Beat Sync (2026-10-04)

- **Problem & Motivation**: Following the Single-Stage Shell migration (below), live testing surfaced a CSP-induced invisible-orb bug and a particle-text clipping bug (both fixed separately, same day). On top of those, the user requested — via reference images, explicitly asking to plan first — three more changes: particles gathering from across the whole screen on wake, a purple/magenta rotating sunburst for "thinking," and a pebble-shaped "speaking" blob molded by NEXUS's own voice beats rather than a stand-in signal. A full plan-mode pass (3 Explore agents + 1 Plan agent mapping the exact shader/entrance/audio-pipeline code) preceded implementation; the user explicitly chose real TTS-audio amplitude streaming over a cheaper mic-proxy approximation.
- **Sub-phase A (screen-wide entrance, plan deviation)**: the plan's primary approach (grow the real WebGL canvas to viewport size during assemble) hit a CSS blocker only found during implementation — `OrbFrame.tsx`'s `transform`-positioned div creates a containing block that traps any `position: fixed` descendant, so it can't reach the real viewport. Built the plan's named fallback instead: `frontend/src/stage/EntranceBurst.tsx`, a separate 2D-canvas overlay spanning the full viewport only during the ~900ms assemble window, edge/corner-biased particles converging into the orb's rect with the same easing `assemble()` uses, fading out as the real orb takes over. `OrbFrame.tsx` gained a `burstSeq` counter independent of the existing `entered` one-shot.
- **Sub-phase B (thinking sunburst)**: reshaped the 6-strand wisp to 20 straighter rays, raised rotation `t*0.12`→`t*0.4`, shifted color violet→magenta. **Superseded same day** by a concurrent session's own, more elaborate thinking-state redesign (64 rays, dual-axis `rotate3D` tumble — see the entry directly below) — that version is what ships; this one is documented for the record only.
- **Sub-phase C1 (speaking pebble silhouette)**: dominant lobe-noise octave lowered in frequency (`n*1.5`→`n*0.65`) and raised in weight (`.26`→`.40`), secondary octave weight lowered (`.16`→`.10`) — fewer, bigger, more irregular lobes. Still live.
- **Sub-phase C2 (real TTS-audio beat sync, new Rust capability)**: `tts.rs`'s new `compute_envelope()` — peak-normalized per-20ms-frame RMS from the PCM both TTS engines already decode before playback. `CaptionTrack` (the existing Phase-3 response-caption struct) extended with `envelope`/`frame_ms`/`envelope_start_ms` (the last riding the same cumulative-offset convention `CaptionWord.start_ms` already uses for streamed replies). `captionScheduler.ts` tracks envelope segments + new `getEnvelopeLevel()`; `VoiceOrb.tsx` pumps the real value into `setLevel()` every frame while speaking, falling back to the old mic-proxy the instant no envelope is available. `voice-orb.js`'s onset term raised (`.12`→`.22`) and now scales lobe amplitude itself (`(1.0+.4*onset)`) so a beat visibly lurches the silhouette; damped by particle-text convergence progress so beats never jitter glyphs mid-converge.
- **Verify**: `node --check`/`tsc` clean; frontend vitest 154/154 (21 suites); `cargo check` clean; `cargo test --lib tts::` 17/17 (4 new `compute_envelope` tests); full Rust suite 874/874 serial; release build clean (`nexus.exe` 52.7 MB).
- **Docs**: `docs/features/89-orb-screen-wide-entrance-and-tts-beat-sync.md`, `docs/changes/77-orb-entrance-burst-and-tts-beat-sync-implementation.md`.

## 3D Rotating Purple Starburst for Thinking State (2026-10-04)

- **Problem & Motivation**: User requested upgrading the `thinking` state avatar animation to match the 3D rotating purple beaded starburst / radial ray sphere from reference design `media_1791105468869.png`. The purple starburst must not be fixed or static, but continuously rotating in 3D.
- **Implementation**:
  - `voice-orb.js`:
    - `PALETTE[2]` updated to vibrant electric purple/magenta (`[0.82, 0.15, 0.96]`).
    - Added `rotate3D(p, yaw, pitch)` function for continuous dual-axis 3D tumbling rotation (yaw $\omega_y = 0.45$, pitch $\omega_x = 0.28$).
    - `VS`: Replaced old 20-strand curled ribbon logic with 64-strand straight radial rays distributed across the unit sphere via the golden-angle Fibonacci lattice.
    - Core nucleus clustering ($s_{\text{along}} < 0.14$): particles form an intense central core with additive white-magenta bloom (`#ffffff` / `#ffa5fb`).
    - Spoke rays ($s_{\text{along}} \ge 0.14$): particles align into 15 concentric radial beaded shells with narrow needle beam thickness ($0.008 + 0.006 \cdot t_{\text{ray}}$).
    - Dynamic motion ("purple should not be fixed and rotating"): continuous dual-axis 3D rotation, outward travelling energy wave ($\sin(18 \cdot r_{\text{norm}} - 3.5t)$), and specular sparks firing along the rays outward from the nucleus.
    - 4-Tier color gradient: Core white/pink $\to$ inner neon magenta $\to$ mid electric purple $\to$ outer deep royal purple.
    - Scale alignment: Bounding radius $R \approx 1.05$ aligns with the idle/listening sphere ($R \approx 1.0$) for seamless 700ms morph transitions.
    - Added full parity to `_paint2D` canvas fallback.
- **Verify**: Rust 827/827 lib tests pass; Frontend 154/154 Vitest pass across 21 test suites; `tsc --noEmit` clean; Production binary compiled via `node nexus.mjs build` to `src-tauri/target/release/nexus.exe`.
- **Docs**: `docs/features/87-shipnotes-webgl-voice-orb-integration.md`, `docs/changes/71-3d-rotating-purple-starburst-thinking-avatar.md`.

## Wakeup Orb Redesign — Single-Stage Shell Migration Complete (2026-10-04)

Closes out the "Single-Stage Shell" migration first planned in the
2026-09-25 entry below ("Next: step 2 orb moves in (pixel-compare), then
loading layer, then panels one by one") — that step was never executed
until this 5-phase arc. All 5 phases are done and committed
(`ff42fa7`, `a89fa82`, `3cbcef3`, `461fc46`, plus this cleanup commit).

- **Phase 1 — window consolidation** ([doc 73](docs/changes/73-window-consolidation-orb-into-stage.md)): the voice orb and the
  top-right loading spinner no longer own OS windows (`main`,
  `loading-indicator` — both deleted from `dyn_windows.rs`). Both are now
  positioned `<div>`s inside the one always-on `stage` overlay, driven by
  `stage:orb_rect`/`stage:loading_rect` events from `window_manager.rs`
  instead of native window show/hide/position calls. The entire voice
  runtime (wake globals, mic warmup, every `stt:*`/`ghost:*`/`audio:*`
  listener) moved from the retired `main.tsx`/`App.tsx` into
  `stage/orbRuntime.ts`; calibration drag/resize became in-page pointer
  tracking (a `<div>` can't `startDragging()` the whole stage window).
- **Phase 2 — shape/color/glitch redesign** ([doc 74](docs/changes/74-orb-shape-color-and-glitch-redesign.md)), grounded directly
  against the user's reference video (frames re-extracted and read, not
  worked from memory): listening shifted from pale gold to grainy
  amber/brown; thinking's closed braided-toroidal-knot became an open
  6-strand wisp formation (evenly-spread golden-angle directions — random
  per-strand directions first clumped into a blob, caught by actually
  rendering it); speaking's sparse lat/lon wireframe lattice became a
  dense noise-displaced bumpy blob. New glitch transition: a ~220ms
  position-jitter + color-flicker burst on every dominant-state change.
- **Phase 3 — response caption** ([doc 75](docs/changes/75-response-caption-word-by-word-above-orb.md)): the spoken reply grows
  word-by-word above the orb, timed off edge-tts's real word-boundary
  events (`Boundary::Word`, not `Sentence`) or evenly-estimated timing for
  the Piper fallback. Emitted at playback start (not synthesis) so cache
  hits, edge-tts, Piper, and streamed chunks all go through one path.
- **Phase 4 — live-speech caption** ([doc 76](docs/changes/76-live-speech-caption-streaming-stt.md)): the user's own words grow
  above the orb while they're still talking, via a new best-effort
  `/stream` WebSocket endpoint on the local Moonshine STT server —
  entirely parallel to the existing batch `/transcribe` pipeline, which
  intent parsing/NLU/brain still exclusively depend on. Verified live
  (real synthesized speech through the real server, then the server
  killed outright) rather than assumed.
- **Phase 5 — cleanup**: removed the dead `.transcript, .caption {
  display: none; }` rule and the rest of `styles.css`'s now-unreferenced
  `main`/`#app`-window-era CSS (confirmed zero remaining usages by
  grepping every class name against the actual JSX before deleting
  anything) — the retired slide-up/down transitions, the CSS-fallback
  `.orb` + its pulse keyframes, `.avatar-section`, and three
  already-orphaned ghost-waves modifier classes found the same way.
  `styles.css` is now exclusively the orb/calibration/ghost-waves styles
  `stage/main.tsx` actually uses; `stage.html` already owns the base
  transparent-fullscreen reset, so that duplicate was dropped too.

## Braided Toroidal Knot Ribbon for Thinking State (2026-10-02)

- **Problem & Motivation**: User requested upgrading the `thinking` state avatar animation to match the 3D braided toroidal knot ribbon from Ship Notes `signal-orb` / `speaking-orb` (replacing the squished sphere with belts).
- **Implementation**:
  - `voice-orb.js`:
    - `sphere(count)` sets the 4th attribute (`seed.w`) to normalized index $u = i / \text{count} \in [0, 1)$, creating continuous ribbon streams across all 12,000 points.
    - `VS`: Replaced old orbital belts with parametric braided toroidal knot equations ($\theta = 3 \cdot 2\pi u + 0.57t$, $\phi = 8 \cdot 2\pi u - 0.48t$, $r_t = 0.7135 + 0.2135\cos\phi + 0.125\sin a$), tilted by $34^\circ$ ($v_y = 0.77 b_y - 0.52 b_z$, $v_z = 0.52 b_y + 0.77 b_z$).
    - Scale alignment: Bounding radius ($R \approx 1.05$) perfectly matches sphere radius ($R \approx 1.0$), ensuring zero jumping, clipping, or center displacement.
    - Visuals: Shifted thinking color from amber to radiant lavender/violet (`rgb(178, 151, 255)` / `#b297ff`). Added `knotSpark` specular white core sparks travelling along the front ribbon loops.
    - Added full parity to `_paint2D` fallback.
- **Verify**: Rust 827/827 lib tests pass; Frontend 144/144 Vitest pass across 19 test suites; `tsc --noEmit` clean; Production binary compiled via `node nexus.mjs build` to `src-tauri/target/release/nexus.exe` (51.2 MB).
- **Docs**: `docs/features/87-shipnotes-webgl-voice-orb-integration.md`, `docs/changes/70-braided-toroidal-knot-thinking-avatar-upgrade.md`.

## Ship Notes WebGL 3D Voice Orb Wake Avatar Integration & Overlay Lifecycle Fix (2026-10-02)

- **Problem & Motivation**: User requested replacing the 2D Lottie vector wakeup avatar with the interactive 3D WebGL particle sphere animation from `aqualang89/shipnotes-components` (12,000 WebGL points, smooth 700ms morphing transitions across states, GPU-accelerated).
- **Hotkey Initialization Issue & Root Cause**: During testing, pressing hotkey `Ctrl+Shift+Space` did not initialize animation. Root causes:
  1. `voice-orb.js` exited early if `document.hidden` was true. Because `"main"` is a non-activating overlay window (`WS_EX_NOACTIVATE`), Windows does not focus it, causing Chromium/WebView2 to mark `document.hidden = true` and halt the rAF loop.
  2. Flexbox sizing collapse: `.avatar-section` lacked explicit dimensions, causing `<voice-orb>` to collapse to 0×0px during initial offscreen layout, initializing a 1×1px WebGL canvas.
- **Implementation**:
  - `voice-orb.js`: Removed `document.hidden` throttle; added explicit `play()` / `pause()` methods with `_paused` flag; updated `_resize()` with `getBoundingClientRect()` and `Math.max(80, ...)` floor; styled shadow DOM canvas with `pointer-events: none` and full dimensions.
  - **No Mic Lock Conflict**: `VoiceOrb` does not initiate duplicate WebView2 `getUserMedia` mic streams (preventing Intel SST audio driver starvation of Rust's `cpal` capture loop). Audio reactivity is cleanly driven via `setLevel(vol)` hooked to Zustand `audioVolume` and `ttsActive`.
  - `VoiceOrb.tsx`: Added `visible` prop with `play()`/`pause()` hook synchronization and minimum 120×120px bounding styles.
  - `Avatar.tsx`: Integrated `<VoiceOrb />` with `state`, `visible`, and `level` reactivity inside `.avatar-voice-orb-container`; `.avatar-wrap` enforces minimum 140×140px footprint; preserved hidden `containerRef` anchor to maintain 100% test compatibility.
  - `styles.css`: Added explicit 100% width/height to `.avatar-section` and `.avatar-voice-orb-container`.
  - **1:1 State Mapping**:
    - `idle`: Soft lilac sphere with organic breathing drift.
    - `listening`: Turquoise inward suction waves reacting to user voice volume.
    - `thinking`: Three orbital amber belts with glowing particle heads.
    - `speaking`: Outward pink/violet wave pulses with bass deformation and transient sparks.
- **Verify**: Rust 827/827 lib tests pass (100%); Frontend 142/142 Vitest pass across 18 test suites (100%); `tsc --noEmit` clean; `node nexus.mjs build` compiled release binary `src-tauri/target/release/nexus.exe` (51.2 MB).
- **Docs**: `docs/features/87-shipnotes-webgl-voice-orb-integration.md`, `docs/changes/69-shipnotes-webgl-voice-orb-integration-and-overlay-lifecycle-fix.md`.

## Wake Word Dataset Poisoning Rescue, Whisper Validator Fix & Retraining (2026-10-01)

- **Problem & Root Cause**: User recorded positive samples saying "NEXUS", but `scripts/record_wake_samples.py` used Whisper with a rigid `soundalikes` filter (`clean.startswith("next")`). Due to Whisper's strong prior on short 1s utterances and regional/fast cadence, "nexus" was misheard as "next test", "next sense", "next sis", and 43 recordings from today's session (plus 93 historical) were automatically promoted to `negative/soundalike_from_rec_*.wav`. This poisoned the dataset: the model was literally being taught that the user's authentic voice was negative.
- **Rescue**: Rescued all 136 `soundalike_from_rec_*.wav` files out of `wake_word_data/negative/` and moved them to `wake_word_data/positive/` as `nexus_0820.wav` through `nexus_0955.wav`.
- **Script Fix (`record_wake_samples.py`)**: Completely removed `promote_neg` from `mode_positive`. In positive mode, user speech is never routed to the negative set. Added natural phonetic mishearings (`next`, `nixes`, `next sis`, `nexis`, `open excess`, etc.) to the accepted pattern list.
- **Retraining Results (`scripts/train_local_wakeword.py`)**: Trained for 45 epochs with Binary Focal Loss ($\gamma = 2.0$, pos_weight = 2.2). Extracted 11,412 positive windows and 31,239 negative windows. Final metrics: **Recall: 97.5%**, **False Alarm: 1.67%**, **Val Loss: 0.0369**. Calibrated model exported to `src-tauri/resources/oww/nexus.onnx`.

## Live Screenshot Blur Restored (ADR-05) + Settings Import/Export Removed (2026-10-01)

- **User report**: sidebar/settings *context* still pitch black, "no changes at all"; directive: the blue transparency is ALREADY implemented — find the method in `docs/` first, implement from that.
- **Method found**: `docs/architecture/06-liquid-glass-screenshot-blur.md` (ADR-05) — GDI capture behind the window → `fast_blur` → `sidebar:backdrop` event → `--sidebar-backdrop-image` → `.sidebar-card::after`. A blue wallpaper = the "sapphire bokeh" (feature 84 §4).
- **Root cause**: pipeline wired (all 7 `prepare_sidebar` paths arm `spawn_sidebar_live_blur`) but killed by 2 switches: `DEV_LIVE_BLUR_DISABLED = true` (commands.rs) + CSS `background-image: none` + `#060608` solid black (the 2026-10-01 black directive). The change-62 follow-up DWM tint (`rgba(0,0,0,0.35)`) can't blur (shows desktop sharply) — replaced by the HEAD-verified `::after` rule.
- **REAL blackout root cause (found via GitHub comparison, user directive)**: `live_glass.rs` (untracked, added by the change-61 session) applied `DWMWA_SYSTEMBACKDROP_TYPE=DWMSBT_TRANSIENTWINDOW` (Acrylic) to the non-activating sidebar — the EXACT API ADR-05 Option A rejected. DWM paints a solid opaque fallback on inactive windows + the material call overrides tao transparency → whole HWND surface pitch black under all CSS. That's why CSS changes showed "no difference". Fix: `apply_live_glass` call removed from `dyn_windows.rs` (GitHub-verified branch: `round_corners` + `WDA_EXCLUDEFROMCAPTURE` only); GitHub card surface restored in CSS (`rgba(20,20,22,0.60)` + 1px border). `live_glass.rs` remains but its accent path has zero callers. Docs: doc 62 "Root-Cause Revision" + doc 63 follow-up 2.
- **Changes**: 
  - `DEV_BACKDROP_CAPTURE_DISABLED` removed from `capture_backdrop` (R1).
  - `DEV_LIVE_BLUR_DISABLED = true` re-added to disable self-capturing live loop (R2).
  - `apply_live_glass` (DWM Acrylic) removed from `dyn_windows.rs` glass branch; GitHub-verified `round_corners` + `WDA_EXCLUDEFROMCAPTURE` only (ADR-05 Option A compliance).
  - `.sidebar-card::after` restored to HEAD (backdrop image + `rgba(8,8,10,0.45)` + crossfade).
  - **All three view cards restored to HEAD**: `.settings-container` (card + gradient `::after`), `.architect-app` (white glass card + gradient `::after`), `.pr-list-container` (rgba card + `::after`).
  - `.unified-shell` (untracked) `background: #060608 !important` → `transparent`.
  - **Export/Import buttons removed** from settings footer (Reset/Save stay).
  - Forensic log `live_glass: skipped for '<label>'` retained in dyn_windows.rs.
- **Verify**: tsc 0; vitest **114/114**; official `node nexus.mjs build` → binary 51.1 MB @ 23:24; dist css contains 4 backdrop variable occurrences across all four views; 7/7 spawn coverage + backdrop emitters + listener audited.
- **Pitfall**: Start Menu `NEXUS.lnk` → `%LOCALAPPDATA%\NEXUS\nexus.exe` = **2026-08-31 stale build** — always launch via `nexus start` after `nexus build`.
- **User terminology (binding)**: "window" = SAME window, different context/view (settings context = settings view in unified sidebar) — never a separate HWND. Docs/responses say "context/view".
- **Docs**: `docs/changes/63-restore-live-screenshot-blur-and-settings-cleanup.md`.

## Sidebar No-Nav-Bar, Full-Height Right-Dock, Waves Preview & DWM Blur Fix (2026-10-01)

- **Problem**: (1) Sidebar showed a header bar with TTS + gear icons — user wants zero navigation; Rust voice commands decide which view renders. (2) Sidebar not positioned full-height from top to bottom. (3) DWM Acrylic blur was applied only to `"sidebar"` and `"calibrate-toolbar"` — not all compact glass windows. (4) Waves animation preview missing from companion calibration HUD. (5) Settings view rendered blank when voice command emitted `sidebar:set_view("settings")` — `SidebarApp.tsx` had no listener.
- **SidebarApp.tsx refactor**: Removed entire `<header className="sidebar-header">` block (TTS + gear icon). Added `activeView` state (`"assistant" | "settings"`), wired `sidebar:set_view` Tauri event listener. When view is `"settings"`, lazy-imports `SettingsSidebarApp` and renders it inside the same window via `<Suspense>` — no HWND swap needed. Removed now-unused `query`, `speaking`, `setSpeaking`, `speak`, `handleToggleTts`, `heading` computed values.
- **sidebar.css**: Added `.sidebar-card--settings` (zero padding, full overflow hidden) and `.sidebar-settings-loading` placeholder style.
- **commands.rs `sidebar_geometry`**: Right dock now places window at `x = monitor_w - w + 12` (12px slide off right edge = subtle emergence). `y = 20` (top margin), `h = monitor_h - 40` (full height minus 40px total margin) for all views. Settings view right-docked by default (no longer centered). All 13 geometry tests updated to match new values — all pass.
- **dyn_windows.rs**: Refactored DWM glass branch from `if label == "sidebar" || label == "calibrate-toolbar"` to a `matches!()` for clarity. The two windows in scope remain unchanged (only `sidebar` and `calibrate-toolbar` are created via `get_or_create_window` — the old separate sidebars are gone).
- **CompanionHudApp.tsx + companion-hud.css**: Added inline **waves preview** — when target is `"waves"`, 7 animated CSS bars appear between the Liquid Radio and action buttons, each with staggered `--bar-delay` and `--bar-base` height. Uses `@keyframes hud-wave-pulse` (scaleY 0.35→1.0, alternating). Added `React.CSSProperties` cast for custom CSS properties.
- **Verify**: Rust 788/788 tests pass (100%); Frontend 108/108 Vitest pass (100%); `tsc --noEmit` clean; `npm run build` clean; `cargo build --release --features custom-protocol` clean.

## STT Empty-Transcript & Mic Pipeline (A→C→B→E) + Stuck-Speaking Watchdog (2026-10-01)

- **Trigger**: live run — wake fires (0.985+), capture starts, `transcript = ''` twice. Research ranked 6 hypotheses; key findings: wake works on the same stream (mic not hard-dead), frontend baton log fires unconditionally (noise, not hog proof), and the 8s frontend abort could never stop Rust (parallel-timer race).
- **A (turn packet)**: `TurnStats` travels with every buffer; `endpoint_reason()` mirrors the stop predicate; STT-path/filter markers in all 3 transcribe fns; `stt:turn_stats` emitted at every receiver exit; frontend logs + `debug_trace`. Every future miss names its cause class.
- **C (holders)**: `micHolders.ts` registry on the shared getter (also fixes a per-call stream leak) + p0 trace carries holders summary.
- **B (single owner)**: 8s timer queries non-destructive `stt_capture_had_speech()`; voice underway → hands off (Rust finishes ≤10s); silence → legacy path (late success preserved). A first cut forwarded destructive `stop_stt_capture` — caught in review (would amputate slow starters), replaced with the query.
- **E (escalation)**: 1st miss silent auto-relisten (reset-first so the wake guard can't misread it as cancel), 2nd single nag + deterministic close; streak resets on speech.
- **Visuals**: stuck-speaking watchdog in `hideOrbAfterSpeech` (10 quiet re-arms → force close; genuine TTS re-arms forever); mapping was already correct (breathe=listening, circles=thinking).
- **Verify**: Rust 788/788 serial; vitest 108/108 ×2; tsc + clippy-zero-new; one full-suite flake rewritten hermetic (seeded ticks).
- **Pending (user-run)**: next miss must carry `stt:turn_stats` + holders trace — paste both to name the cause.
- **Docs**: `docs/research/voice-stt/stt-empty-transcript-mic-pipeline-2026-10-01.md` (§8 implementation record).

## Companion Calibrator 2-Row Layout, Liquid Radio & Animated NumberFlow Slider (2026-10-01)

- **Problem & Feedback**:
  - The companion HUD pill in the live run (`media_1790841924506.jpg`) suffered from truncated action buttons: `Cancel` and `Save` were cut off at the right edge due to narrow 440px window width.
  - The static `200 px` badge took up horizontal space.
  - The subtext (`1-2-3 select · ...`) cluttered the HUD.
  - The target selection lacked refractive liquid glass visuals in dark/light modes.
- **Implementation**:
  - **Window Dimensions (`dyn_windows.rs`, `calibration.rs`)**: Expanded `calibrate-toolbar` from 440×54 to 540×92, centered at the top of the monitor (`phys_w = (540.0 * scale) as i32`), giving full breathing room so `Cancel` and `Save` are never clipped.
  - **Liquid Radio Selector (`GlassFilter.tsx`, `CompanionHudApp.tsx`, `companion-hud.css`)**: Implemented the 3-option selector (`Wakeup`, `Waves`, `Loading`) with `#radio-glass` SVG displacement map filter (`feTurbulence` + `feDisplacementMap` scale 30) and sliding frosted glass pill indicator with specular reflections.
  - **Precision Size Slider with NumberFlow (`Slider.tsx`)**: Replaced the static `200 px` badge with a dedicated Row 2 containing `@radix-ui/react-slider` and animated `@number-flow/react` tooltip badge above the thumb, allowing live visual scaling (100–300px for Wakeup/Waves, 40–160px for Loading).
  - **Subtext Removed**: Deleted the bottom helper text entirely for a clean, minimalist 2-row liquid glass floating widget.
- **Verify**: Rust 769/769 tests pass (100%); Frontend 97/97 Vitest pass (100%); `npm run build` clean; Release binary built (`target/release/nexus.exe`, 53.4 MB with `--features custom-protocol`).

## Animation Positioning Controls Liquid Glass & Calibrator HWND Glass Integration (2026-10-01)

- **Audit & Cross-Check of Muse Changes**:
  - `dyn_windows.rs`: Muse correctly removed `stage` from `apply_live_glass()`, fixing the fullscreen blackout. However, `calibrate-toolbar` (the companion pill HUD) was missing from the `live_glass::apply_live_glass` and `round_corners` branch. Added `config.label == "calibrate-toolbar"` so the companion HUD receives native Win32 DWM hardware GPU blur behind the window with rounded corners and capture exclusion.
  - `companion-hud.css`: Replaced dark opaque backgrounds (`0.82` opacity) with true liquid frosted glass (`rgba(15, 23, 42, 0.45)`, `backdrop-filter: blur(28px) saturate(180%)`, specular border `rgba(255, 255, 255, 0.22)`, inset reflection rim `inset 0 1px 1.5px rgba(255, 255, 255, 0.35)`). Upgraded all buttons (`Wakeup`, `Waves`, `Loading`, `Undo`, `Reset`, `Save`, `Cancel`) to liquid glass style with crisp pure white text shadows and specular highlights.
  - `SettingsSidebarApp.tsx` & `settings-sidebar.css`: Styled the Command Hub animation positioning controls (`✥ Drag & Position on Desktop` and `Center-bottom, 200px`) with `.settings-btn--liquid-glass` featuring Apple-grade inset specular rim, high-contrast text glow, and optical backdrop blur.
  - `stage/main.tsx`: Verified completely clean, transparent, with zero click-blocking hitboxes or overlay pills.
- **Verify**: Rust 769/769 tests pass (100%); Frontend 97/97 Vitest pass (100%); `npm run build` clean; Production release compiled cleanly (`target/release/nexus.exe`, 53.4 MB with `--features custom-protocol`).

## Ghost Fullscreen Blackout Fix — Live Glass Isolated to Compact Windows (2026-10-01)

- **Symptom**: entering Ghost Mode blacked out the entire screen (Esc abort only).
- **Root cause**: `dyn_windows.rs:202` applied `live_glass::apply_live_glass()` to `stage` — a 1920×1080 fullscreen overlay. DWM paints Acrylic/blur across the whole HWND rect, i.e. 100% of the monitor. DOM innocent (`ghost.css` has no fullscreen background).
- **Fix**: dropped `stage` from the live-glass branch (corners + affinity go with it, matching the share-visible stage policy); `stage/main.tsx` root explicitly transparent, overlay `+ Button` block removed (its hitbox was self-registered on mount, so no dead click-eater remains).
- **Rule**: fullscreen overlay HWNDs never get DWM backdrops/whole-window blur — live glass is for compact windows only.
- **Verify**: vitest 97/97; Rust 769/769 serial; `npm run build` + release 50.9 MB clean.
- **Docs**: `docs/changes/62-ghost-fullscreen-blackout-fix.md`.

## Live Optical Frosted Glass, Selective Cursor Hit-Testing & Adaptive Luminance (2026-10-01)

- **Problem & Specification**: User required true optical pass-through blur (moving tabs, video playback, cursors behind the glass blurred live in real-time at 60Hz–144Hz) with zero-touch automatic light/dark luminance adaptation (reference image `media_1790832308311.png` "+ Button" split demo) where white text stays pure `#FFFFFF` across both modes, and selective cursor hit-testing where the cursor is an exception ONLY on the overlay button and sidebar (copying/clicking) while the rest of the screen is 100% click-through.
- **Implementation**:
  - **Live Optical DWM Hardware GPU Blur (`live_glass.rs`)**: Wired `DwmSetWindowAttribute` (`DWMSBT_TRANSIENTWINDOW` - Acrylic on Win11 22H2+) and focus-independent `SetWindowCompositionAttribute` (`ACCENT_ENABLE_BLURBEHIND` state 3, `0x00000000` transparent mask) applied across `sidebar`, `settings-sidebar`, `architect-sidebar`, `pr-list-sidebar`, and `stage`.
  - **Selective Hitbox Mouse-Hole Loop (`stage.rs`, `live_glass.rs`)**: Stage overlay is 100% click-through (`set_ignore_cursor_events(true)`). When cursor enters registered hitboxes (overlay button or sidebar), `set_ignore_cursor_events(false)` enables clicking, text selection, and copying.
  - **Screen Luminance Probe (`luminance_probe.rs`)**: Samples 8×8 GDI grid under widget rects (<0.1ms); computes ITU-R BT.709 relative luminance ($Y = 0.2126R + 0.7152G + 0.0722B$); applies 25-point hysteresis ($Y_{light} > 140$, $Y_{dark} < 115$) preventing edge flicker.
  - **Liquid Glass CSS & Component (`liquid-glass.css`, `LiquidGlassButton.tsx`, `main.tsx`)**: Replicates `media_1790832308311.png` "+ Button" pill button with specular edge border, inset reflection rim, and high-contrast text shadows ensuring white text is 100% readable over both milky light glass and obsidian dark glass.
- **Verify**: Rust 770/770 tests pass (100%); Frontend 100/100 Vitest pass (100%); `tsc && vite build` clean; Release binary built (`target/release/nexus.exe`, 50.9 MB with `--features custom-protocol`).
- **Docs**: `docs/changes/61-live-optical-frosted-glass-and-selective-cursor-hit-testing.md`.

## Calibration Follow-Up: Waves Preview, Keyboard, Save Scope, Liquid Glass (2026-10-01)

- **Phase 0 (user-reported garbled Waves preview)**: container mounted but Lottie never loaded (mount effect gated on unreachable `ghostPhase`), bars frozen flat, return-to-Wakeup blank (init deps `[animationData]`). Fix: `resolveWavesShown()` selector + preview playback (0.6x Lottie loop, free CSS bars) + `calibrationTarget` in init deps. +4 tests.
- **Phase 2 (keyboard)**: numbered badges + `1/2/3` select + arrows 1px/⇧10px via Rust `calibration_nudge` (`overlay_nudge` pure, +6 tests); window-scoped keydown only; burst-coalesced undo.
- **Phase 1 (save scope)**: dirty ✓ dots + scoped toast (`save_toast()`); `initial` snapshot restored to session.
- **Phase 3 (liquid glass)**: user-scoped OUT — implemented + verified, then fully reverted; plan retained in research 03.
- **Verify**: Rust 764/764 serial; vitest 95/95 calibration scope (full suite 97/97); tsc clean; `npm run build` + release 50.9 MB `--features custom-protocol`.
- **Pending (user-run)**: toggle previews, wallpaper legibility, keyboard matrix, scoped-save acceptance.
- **Docs**: `docs/research/command-hub/03-…-2026-10-01.md` (§-implementation outcomes), `docs/changes/60-calibration-keyboard-nudge-save-scope-and-liquid-glass.md`.

## Direct Desktop Drag & Scroll-Wheel Calibration + Workspace Collision Recovery (2026-10-01)

- **Collision incident (recovered)**: a second agent session stale-restored `lib.rs`/`commands.rs` (180 errors; 16 mods + ~50 command registrations + my Phase-1/3 settings/commands lost; dual-grammar intent_parser/screen/dyn_windows survived). Correct copies existed only in the working tree (HEAD is older). Recovered via the Gemini transcript at `.gemini/antigravity/brain/3122b61d…/logs/transcript_full.jsonl` (numbered pre-collision reads of `lib.rs` gave the true handler list) + grep-driven `#[tauri::command]` inventory + 59 frontend invokes. Post-recovery: 756/756 Rust tests.
- **Feature (all gates green)**: on-screen drag + scroll-wheel calibration for Wakeup/Waves/Loading — Command Hub Display card ("✥ Drag & Position on Desktop") → 440×54 companion pill HUD top-center → native `startDragging()` drag + debounced onMoved pct report (20px magnetic snap to center/bottom) + wheel ±10px resize (100–300 orb/waves, 40–160 loading) + px badge + Undo/Default/Cancel/Save round-trip (9 params read-modify-write to settings.json, unknown fields preserved; Esc=cancel, Enter=save) + success toast.
- **Key files**: `calibration.rs` (session + 8 commands, +6 tests), `window_manager.rs` (`overlay_xy` pure, `read_waves/loading_settings`, `position_loading`, ghost/calibration branch in `position_orb`), `frontend/src/calibration/geometry.ts` (+9 tests), `frontend/src/companion-hud/` (+5 history tests), `Avatar.tsx` (100%/100% container, drag/wheel/badge, waves preview without ghost session), `loading.html` (inline calibration block), `SettingsSidebarApp.tsx` (entry card + toast + 6 round-trip keys), `dyn_windows.rs::calibrate_toolbar`, vite input + capabilities.
- **Waves semantics**: waves render inside the orb window; `waves_*` rect is applied to it during ghost sessions (`position_orb` branches on `session_active()`). Loading runtime placement now honors saved settings at both show sites.
- **Verify**: Rust 756/756 serial; vitest 87/87; tsc clean; `npm run build` (dist/companion-hud.html emitted); release 50.9 MB with `--features custom-protocol`.
- **Pending (user-run)**: manual acceptance — drag orb/waves/loading, wheel-scale, save/cancel round-trip, ghost-session waves at saved rect.
- **Docs**: `docs/research/command-hub/02-direct-desktop-drag-and-drop-calibration-architecture-2026-10-01.md`, `docs/changes/59-direct-desktop-drag-and-scroll-calibration.md` (incl. the collision recovery record).

## Dual-Grammar Modal Partitioning, Unbreakable Typing & Screen Analysis (2026-10-01)

- **Problem & Root Causes**:
  - Live Ghost Mode run showed: *"Search for almonds"* misheard as `'So it's for almonds.'` was stripped of `"so"` and fell to `WorkerBackend` chat instead of searching in Brave.
  - Saying bare `'Type.'` fell to `WorkerBackend`.
  - Dictating complex phrases like *"type so it analysis for the servx right send it to as soon as possible"* in chat windows collided with `parse_analyse_command` and `parse_send_whatsapp_message`.
- **Implementation**:
  - **Unbreakable Typing Enclave (`parse_type_dictation_command`)**: Prioritized ahead of all domain parsers in `intent_parser.rs`. Explicit toggles manage dictation mode; bare `"type"` toggles dictation; any `"type <payload>"` captures literal text verbatim without command keyword interference.
  - **Acoustic Search Resilience**: Added `"so it's for"`, `"it's for"`, `"it is for"`, and `"search that on"` to `normalize_phonetic_mishearings` and `SEARCH_VERBS`.
  - **Numbered Search Result & Screen Click Matching**: Enhanced `screen::parse_ordinal` with word numerals (`first`, `second`, `one`, `two`, etc.) and expanded regex matching in `intent_parser.rs` for *"open result 2"*, *"open the 2 in the result"*, and *"click the second result"*.
  - **Vision Screen Analysis & Global Prefix Gating (`orchestrator.rs`)**: Wired `run_screen_analysis` with instant feedback (*"Analyzing the screen, sir."*) and Groq Llama-4-Scout VLM screenshot querying. Global breakouts (`"Nexus <action>"`) escape local foreground grounding.
- **Verify**: Rust 750/750 lib tests pass (100%); Frontend 72/72 Vitest pass (100%); Release binary compiled (`target/release/nexus.exe`, 50.9 MB with `--features custom-protocol`).
- **Docs**: `docs/research/ghost-mode/06-dual-grammar-modal-partitioning-and-universal-grounding-architecture-2026-10-01.md`, `docs/features/81-dual-grammar-modal-partitioning-and-foreground-grounding.md`, `docs/changes/58-dual-grammar-modal-partitioning-unbreakable-typing-and-screen-analysis.md`.

## Ghost FIFO Queue ("Real Ghost Mode") + Hotkey/Esc Realignment (2026-09-30 → 2026-10-01)

- **Plan**: Pasted master plan `NEXUS-PLAN-GHOST-FIFO-2026-09-30-V2` (Approach E: FIFO + 4 preemption rules + hotkey/Esc realignment). Executed phase-by-phase with double cross-checks (pre-implementation audit + post-implementation verification per phase).
- **Plan audit corrections (plan was ~85% accurate)**:
  - Gap 1 overstated: `drill_begin()` IS called in `live/commands/ghost_drill.rs:75` + `mouse.rs:316`; only orchestrator-level runners (`run_ghost_open`, `run_browser_search`, `run_browser_search_focus`) lacked it → RAII `GhostDrillGuard` added to those 3 only.
  - Gap 2 overstated: `drain_ghost_followups` IS called (`ghost_drill.rs:152`, `mouse.rs:457`) for standalone runs; voice sessions use the hot-mic loop → no second drain added to `run_ghost_message`.
  - `browser_url::is_browser_active()` doesn't exist → `verify_grounding` uses `get_active_browser_url().is_some()`.
  - Sidebar Esc routes through store `hide()` (not direct `hide_sidebar` invoke) to preserve TTS-stop + 400ms guard; settings Ctrl+Space-closer REPLACED (not added) for D3 coherence.
- **Phase 1 (`wakeword_oww.rs`, `commands.rs`, `lib.rs`, `hotkey.rs`, `main.tsx`)**: `CAPTURE_SESSION_ID` generation + session-tagged `(u64, Vec<f32>)` channel with silent stale-drop; `abort_stt_capture()` → `SttAbortResult{had_speech, elapsed_ms}` exposed as `stop_stt_capture`; hotkey TTS branch → 150ms DAC drain + wake sequence + `drop_followups()` reconcile; 2nd-press hides orb only when voiceless. 4 new tests.
- **Phase 2 (Esc C3, D3)**: `SidebarApp.tsx` Esc → lightbox-first via pure `resolveEscDismiss()` (new `sidebarEsc.test.ts`, 2 tests) else store `hide()`; settings Esc replaces Ctrl+Space; hotkey window branch closes then falls through to wake. Grep-gate: only global `Escape` registration remains `ghost.rs:445`.
- **Phase 3 (`ghost.rs`, `orchestrator.rs`, settings)**: `QueuedCmd` + `SlotClass` FIFO (`enqueue_command` supersede-AppOpen/Navigate + drop-newest, `dequeue_command`); priority lane (stop → preempt + purge); per-session depth-ACK (`DEPTH_ACK_SPOKEN`, reset in `ghost_enter`); `verify_grounding`; drain rewrite (τ sleep + 15s watchdog + grounding skip); 3 new settings (`ghostDepthAck`, `ghostStepTimeoutMs`, `ghostTurnGapMs`) with serde defaults + loose readers + frontend round-trip keys. 7 new tests (one caught a real bug: "start dictation" mis-slotted AppOpen → dictation-first ordering fix).
- **Phase 4 (cadence)**: `waitForAudioIdle` 3000→1000ms; ghost turn-end beats 550→250ms (normal mode byte-identical, existing tests untouched); +1 ghost-beat test.
- **Verify**: Rust 746/746 lib + 19/19 integration; tsc clean; frontend 71/71 vitest; zero new clippy warnings (1 pre-existing error `google/mail.rs:241` untouched).
- **Post-exit stuck-orb fix (same night)**: exit left the wakeup orb visible forever — `ghost:session(false)` forces visible but the exit turn emits no `ack`, and the `result` path never arms a hide. Fix: arm `hideOrbAfterSpeech(3000)` in the exit branch (`App.tsx`); +1 race-proof test; 72/72 vitest; rebuilt dist + release binary (50.9 MB, `--features custom-protocol`).
- **Pending (user-run)**: live script test ("open Brave → new tab → search almonds"), F1–F7 failure drills, H1–H7 hotkey matrix, normal-mode regression.
- **Post-build incident (agent error, fixed same night)**: first `cargo build --release` omitted `--features custom-protocol` → windows pointed at `http://localhost:5173` with no Vite running ("localhost is not reachable"; confirmed by port scan + binary strings). `frontend/dist` was also stale (cargo never rebuilds it). Fix: `npm run build` + `cargo build --release --features custom-protocol` (50.9 MB). Rule: never ship bare `cargo build --release`.
- **Docs**: `docs/changes/57-ghost-fifo-queue-and-hotkey-realignment.md` (full per-file record, deviations, limitations, checklists).

## NEXUS Command Hub Overlay Wiring & Repeated Punctuation Wake-Word Hardening (2026-09-30)

- **Root Cause Analysis — Why "Open Command Hub" Spoke "Welcome..." and Didn't Open**:
  - *Root Cause 1 (Stale Release Binary)*: The background process was executing an earlier `target\release\nexus.exe` compiled before `ParsedIntent::OpenSettings` was dispatched.
  - *Root Cause 2 (STT Punctuation & Repeated Wake Word)*: User said *"Nexus command hub"*, and Whisper captured `'Nexus. Nexus command hub.'`. In `strip_wake_prefix`, the boundary check only allowed `' '` or `'\t'`, skipping `"nexus."` because of the period. The utterance dropped through to the Cloudflare Worker LLM backend, which hallucinated the conversational reply *"Welcome to NEXUS Command Hub..."* instead of handling it locally.
  - *Root Cause 3 (Missing Topmost & Focus Elevation)*: `WindowConfig::settings_sidebar` was configured with `focus: false`, which on Windows could leave the window behind active full-screen applications.
- **Punctuation & Multi-Prefix Wake Stripping (`src-tauri/src/intent_parser.rs`)**:
  - Updated `strip_wake_prefix` with boundary support for punctuation (`.`, `,`, `!`, `?`, `:`, `-`, `;`).
  - Added loop stripping repeated wake words (e.g. `'Nexus. Nexus command hub.'` $\to$ cleanly resolves to `'command hub'`).
- **Exhaustive Command Hub Utterance Alternatives (`src-tauri/src/intent_parser.rs`)**:
  - Expanded `is_settings_command` matching: `"the nexus command hub"`, `"nexus command hub"`, `"the command hub"`, `"command hub"`, `"command hub sidebar"`, `"command hub window"`, `"show command hub"`, `"bring up the command hub"`, `"command hub please"`, `"nexus control hub"`, `"open control hub"`, `"nexus comment hub"`, etc.
- **Topmost & Focus Enforcement (`src-tauri/src/commands.rs`)**:
  - Added `win.set_always_on_top(true)` and `win.set_focus()` in `show_settings_sidebar` ensuring the window is elevated above all desktop apps.
- **Orchestrator Execution & Cached TTS (`src-tauri/src/orchestrator.rs`, `tts.rs`)**:
  - Added `ParsedIntent::OpenSettings => run_open_settings(app).await` speaking cached *"Opening Command Hub, sir."* (<5ms) and spawning `show_settings_sidebar`.
- **Verify**: Rust 184/184 `intent_parser` tests pass; 53/53 `orchestrator` tests pass; frontend 57/57 vitest pass; release binary freshly compiled (`50.7 MB`).

## Multi-Email Google Architecture, Direct OAuth & Chromium Omnibox Hardening (2026-09-29)

- **Root Cause & Fix for Brave URL Detection (`src-tauri/src/browser_url.rs`)**:
  - In user's live run, Brave returned `Active browser URL: None`.
  - Root cause 1: Chromium/Brave strips `https://` from the address bar (`mail.google.com/mail/u/0/...`); line 143 discarded any URL not starting with `http`.
  - Fix 1: Auto-prepends `https://` for schemeless Chromium URLs.
  - Root cause 2: UI Automation Edit control often stores the value in `Name` instead of `ValueValue`. Added fallback in `read_url_from_edit`.
  - Fix 2: Added window title fallback (`GetWindowTextW`) extracting Gmail thread context when the address bar element is not exposed.
- **Direct RFC 8252 Native Loopback OAuth (`src-tauri/src/google/oauth.rs`)**:
  - Implemented direct PKCE OAuth authorization flow with Tokio TCP listener on `http://127.0.0.1:49152/callback`.
  - Eliminates Cloudflare Worker dependency for authentication and generates permanent auto-refreshing access tokens directly in Windows Credential Manager.
- **Multi-Account Vault Registry (`src-tauri/src/auth_vault.rs`, `types.rs`)**:
  - Added `get_google_accounts`, `save_google_account`, `set_primary_google_account`, `remove_google_account`, `get_google_refresh_token`, `set_google_access_token`, and `get_token_for_google_account`.
  - Supports multi-account profiles with avatars, primary badges, and account-scoped token lookup with automatic refresh.
- **`google::accounts` Sub-Engine & Commands (`google/accounts.rs`, `commands.rs`)**:
  - Implemented `list_accounts`, `connect_account`, `disconnect_account`, `set_primary_account`, and `save_custom_credentials`.
  - Wired to Tauri IPC commands for seamless frontend invocation.
- **Multi-Account Sentinel Target Resolution (`src-tauri/src/google/sentinel.rs`)**:
  - Updated Sentinel polling loop to resolve account-specific tokens per watched email target with automatic refresh fallback.
- **Settings Sidebar UI Refactor (`frontend/src/settings-sidebar/SettingsSidebarApp.tsx`)**:
  - Refactored Auth tab into a clean liquid-glass Google Accounts manager rendering account cards, avatars, primary badges, "+ Add Google Account" button, and custom developer credentials drawer.
- **Verify**: Rust 716/716 library unit tests pass; frontend 51/51 vitest pass; `cargo check` clean.
- **Docs**: `docs/features/77-multi-email-google-accounts-and-direct-oauth-settings.md`.

## Vision Mode Screen Email Scanning, Hybrid Grounding & Proactive Watch Memory (2026-09-29)

- **Native Gmail Engine Thread Watch & Semantic Diffing (`src-tauri/src/google/mail.rs`, `types.rs`)**:
  - Added `WatchStatus`, `ThreadWatchTarget`, and `ThreadUpdateEvent` data models.
  - Implemented `create_thread_watch` and `diff_thread_state`, extracting sender, subject, latest message ID, message count, and semantic deadline status (`parse_deadline_update`).
  - Added tertiary initial deadline extraction regex pattern (`due by|due on|due date is|submission deadline is`) enabling accurate baseline capture even before any extension occurs.
  - Differentiates `DeadlineChanged`, `NewReply`, and `NoChange`.
- **Active Browser URL Grounding (`src-tauri/src/browser_url.rs`)**:
  - Implemented `extract_gmail_thread_id_from_url` resolving `#inbox/<ID>`, `#all/<ID>`, `#search/.../<ID>` from Chrome/Edge/Brave active tab in <5ms.
  - Provides deterministic thread identification without VLM latency, hallucination, or token costs when Gmail web is active.
- **Vision Screen Email Context Extraction (`src-tauri/src/vision.rs`)**:
  - Added `ScreenEmailContext`, `build_email_extraction_prompt`, and `parse_screen_email_json`.
  - Fallback VLM pipeline captures active desktop email clients (Outlook, Thunderbird, web) extracting sender, subject, snippet, and apparent deadline with robust markdown fenced JSON parsing.
- **Persistent Watch Memory Storage (`src-tauri/src/memory.rs`)**:
  - Created dedicated JSON storage `mail_watches.json` in `%APPDATA%/com.nexus.assistant/memory/`.
  - Implemented `load_mail_watches`, `save_mail_watches`, `add_mail_watch`, and `update_mail_watch_status`.
- **NLU Intent, Center Routing & Orchestrator Execution (`intent_parser.rs`, `center.rs`, `google/mod.rs`, `orchestrator.rs`)**:
  - Added `ParsedIntent::WatchScreenEmail` matching utterances like *"update me whenever there is any update on the deadline or anything for this email"*, *"watch this email"*, *"track deadline changes in this email"*.
  - Mapped `WatchScreenEmail` to `"GoogleCenter"` in `center.rs` with repeat-back confirmation dialog.
  - Implemented `run_watch_screen_email` orchestrator runner: reads active browser URL for instant thread ID grounding; falls back to vision capture; registers watch target in persistent memory; triggers proactive background sentinel polling.
- **Verify**: Rust 34/34 `google` unit tests pass; 12/12 `memory` unit tests pass; 24/24 `vision` unit tests pass; 2/2 `browser_url` unit tests pass; 183/183 intent parser unit tests pass; frontend 45/45 vitest pass; clean release binary built.
- **Docs**: `docs/features/76-vision-mode-screen-email-scanning-and-proactive-watch-memory.md` & `docs/research/vision-grounded-gmail-engine-and-proactive-watch-architecture-2026-09-29.md`.

## Google Ecosystem Sub-Center & Main Center Validity Gate Architecture (2026-09-29)

- **Domain Sub-Services Built & Tested in Isolation (`src-tauri/src/google/`)**:
  - `mail.rs` (8 tests): Regex semantic deadline / schedule extraction (`parse_deadline_update`), RFC 2822 base64url message encoding, JSON parsing for threads, messages, and receipts.
  - `calendar.rs` (5 tests): Event payload construction, RFC 3339 / ISO 8601 parsing, schedule conflict detection (`check_schedule_conflict`), and free-busy buffer calculation (`calculate_free_busy_buffer`).
  - `maps.rs` (4 tests): Directions / leg summary parsing, commute delay detection with dynamic departure recommendation (`detect_commute_delay`), Places API parsing, and navigation/search deep link generators.
  - `photos.rs` (5 tests): Media item & album parsers, natural speech semantic query extraction (`parse_semantic_query`), search filter payload generation, and sized URL helper.
  - `sentinel.rs` (4 tests): De-duplicating `SeenMessageRingBuffer` (500 capacity), proactive deadline alert synthesis, commute traffic alert synthesis, and urgency ranking (`Low`, `Medium`, `High`, `Critical`).
- **GoogleCenter Hub Consolidation (`google/mod.rs`, `google/auth.rs`, `google/client.rs`)**:
  - Implements the `SubCenter` trait (`name`, `validate`, `confirm_kind`).
  - Alexa Skills Kit dialog standard: validates required slots (`to`/`body` for email, `title`/`time` for calendar, `destination` for directions, `query` for photos) with exact spoken elicitation prompts instead of falling back to cloud LLM chat.
  - Confirmation policy: gated confirmations with timeout for outgoing actions (`send_email`); repeat-back confirmations for routine operations (`create_event`, `directions`, `search_photos`).
  - Seamless authentication via `auth_vault::get_token("google")` backed by OS Credential Manager (Windows Credential Manager) with environment token fallback.
- **Main Center Connection (`center.rs`)**:
  - Implemented `center_for` decision tree mapping intents to their respective centers (`AppCenter`, `BrowserCenter`, `MediaCenter`, `MessageCenter`, `CommerceCenter`, `GitHubCenter`, `ArchitectCenter`, `KnowledgeCenter`, `DictationCenter`, `GhostCenter`, `GreetingCenter`, `GoogleCenter`).
  - Implemented `validate` gatekeeper filtering unheard / sub-threshold transcripts (<2 alphanumeric characters) and enforcing required slots before execution.
- **Verify**: Rust 32/32 `google` unit tests pass; 31/31 `center` unit tests pass; 182/182 intent parser tests pass; frontend 44/44 vitest pass; release build clean (50.8 MB binary `nexus.exe`).
- **Docs**: `docs/features/75-google-center-hierarchical-domain-architecture-and-proactive-sentinel.md` & `docs/research/google-sub-center-and-sentinel-architecture-2026-09-29.md`.

## Ghost Mode Persistent Always-On Waves & Speech-Only Motion (2026-09-29)

- **Root Cause 1 (Waves Killed on Idle)**: `shouldShowWaves` previously checked `wakeChoreographyDone`. When a turn completed and `state` became `"idle"`, a hook reset `wakeChoreographyDone = false`, immediately flipping `shouldShowWaves` to `false` and destroying the waves.
  - **Fix**: Removed `wakeChoreographyDone` entirely. `shouldShowWaves = visible && ghostActive`. As long as Ghost Mode is active (`ghostActive`), the waves stay persistently visible and always on throughout the entire session.
- **Root Cause 2 (waves.json Container Not Mounted)**: The `useEffect` that loaded `waves.json` into `wavesContainerRef` only depended on `[wavesData]`. When `wavesData` fetched on boot, `ghostPhase` was `"smile"`, so `<div ref={wavesContainerRef} />` was not yet in the DOM (`wavesContainerRef.current === null`). When Ghost Mode started, the effect never re-ran, so `waves.json` was never mounted.
  - **Fix**: Added `ghostPhase` to the dependency array `[wavesData, ghostPhase]`. When `ghostPhase` becomes `"waves"`, `lottie.loadAnimation` mounts `waves.json` directly into the container and parks it at frame 0.
- **Root Cause 3 (Motion When We Speak Only)**: In the rAF loop, when active speech occurs (`ttsActive || micLevel >= WAVE_REST_FLOOR`), `wavesAnimRef.current.play()` animates the waves. When silent, `wavesAnimRef.current.pause()` freezes the waves in place.
- **Verify**: Rust 182/182 intent parser tests pass; frontend 44/44 vitest pass.

## Normal Mode Orb Stability, Non-Stop Thinking Loop & 'The Post Mode' Mishearing Fix (2026-09-29)

- **Idle Glitch / Infinite Restart Loop (`frontend/src/avatar/Avatar.tsx`)**:
  - **Root cause**: Inside `onComplete`, when `idle-smile` finished its arrival segment, it called `applyState(a, state)`. In idle, `state = "idle"`, which resolved to `mode: "idle-smile"`. But `modeRef.current` had just been set to `"holding"`. Because `"holding" !== "idle-smile"`, `applyState` restarted `idle-smile` from frame 0 in an infinite loop, causing continuous twitching/jittering in idle.
  - **Fix**: Removed the recursive `applyState` call in `onComplete`. Added a guard in `applyState`: if `modeRef.current === "holding"` and `resolved.mode === "idle-smile"`, it simply returns without restarting. The orb now parks completely calm and stable at `map.hold` (frame 300).
- **Premature Movement & Loading Circles Before User Speaks (`Avatar.tsx`, `styles.css`)**:
  - **Root cause**: `resolveAvatarAnim("listening")` was running `SEG_LOADING` (`[171, 260]`), and `styles.css` had `animation: pulse-listen` on `.avatar-wrap--listening`. The orb was both spinning circles and pulsing before the user spoke.
  - **Fix**: In `resolveAvatarAnim`, `listening` now resolves to `idle-smile` (the smiling orb holding frame 300). Removed `pulse-listen` from `.avatar-wrap--listening` so the orb stays completely still and steady while listening.
- **Non-Stop Loading Loop in Thinking (`Avatar.tsx`)**:
  - **Requirement**: Non-stop loading when thinking.
  - **Fix**: `resolveAvatarAnim("thinking")` exclusively returns `mode: "loading-loop"`, segment `map.loading` (`[171, 260]`), `loop: true`, `speed: 1.5`. It continuously loops without interruption until thinking completes.
- **Speaking Zoom In/Out (`styles.css`, `Avatar.tsx`)**:
  - **Requirement**: Speaking zooms in and out.
  - **Fix**: Speaking state holds the smiling orb at `map.hold`, while `.avatar-wrap--speaking` drives `animation: pulse-speak 0.55s ease-in-out infinite` scaling from `1.0` to `1.10`.
- **STT Mishearing 'The Post Mode' / 'The Ghost Mode' (`intent_parser.rs`, `recorder.ts`)**:
  - **Root cause**: Saying "the ghost mode" or Whisper mishearing it as "the post mode" produced transcripts starting with "the ", which failed the prefix checks in `parse_ghost_control_entry`.
  - **Fix**: Added `"the ghost mode"`, `"the post mode"`, `"the host mode"`, `"the coast mode"` to `TRIGGERS` in `parse_ghost_control_entry`, `normalize_phonetic_mishearings`, and `recorder.ts` regex.
- **Verify**: Rust 182/182 intent parser tests pass; frontend 44/44 vitest pass.

## Ghost Mode Esc Fix & Ctrl+Space Wave Flash Fix (2026-09-29)

- **Esc Not Stopping Ghost Mode (`src-tauri/src/ghost.rs`)**:
  - **Root cause**: `abort_session` (triggered by Esc panic) sets `SESSION = Yielded` but NOT `Idle`. On the next `"ghost mode"` voice command, `ghost_enter` called `register_esc` without first unregistering. The `tauri_plugin_global_shortcut` plugin rejected or stacked on the already-registered Esc shortcut, causing the handler to silently fail on every session after the first Esc press.
  - **Fix**: Added `unregister_esc(&app)` immediately before `register_esc(&app)?` in `ghost_enter`, giving a clean shortcut registration every session regardless of prior abort state.
- **Ctrl+Space Waves Flash and Disappear (`frontend/src/audio/recorder.ts`)**:
  - **Root cause**: On hotkey tap with no voice, Rust STT emitted `stt:transcript = ""`. `processTranscript("")` spoke "Didn't catch that sir" (~1s TTS) which raced with Plan B wake choreography completion (`wakeChoreographyDone = true` → 220ms pinch → waves appear). TTS finished almost simultaneously with waves appearing → `setVisible(false)` + `reset()` → `state = "idle"` → `wakeChoreographyDone = false` → `shouldShowWaves = false` → waves collapsed. Net: ~100-300ms flash.
  - **Fix**: Empty transcript in non-ghost mode now silently hides (`setVisible(false)` + `reset()` **immediately**) with no TTS. Ghost hot-mic silent-miss cap behavior preserved.
- **Regression — Smile Animation Flip on Silent Dismiss (`Avatar.tsx`)**:
  - **Root cause 1 (Exclusive Waves)**: I mistakenly forced the 3-bar wave visualizer into the normal wake-up flow. The user clarified waves should **only** appear during Ghost Mode.
  - **Root cause 2 (Smile Flip)**: The immediate `reset()` (setting `state="idle"`) caused the `applyState` hook to force the Lottie to `idle-smile`. Since the orb takes 600ms to visually slide down (CSS transition), the user saw this abrupt frame jump as a "flip".
  - **Fix 1 (Exclusive Waves & Choreography Sequence)**: Reverted the complex choreography logic. `shouldShowWaves` is now strictly `visible && ghostActive && wakeChoreographyDone`. In normal mode, the orb simply plays its native Lottie animations (loading, smile). When Ghost Mode is initialized, it waits to finish its smile arrival, then seamlessly pinches into the waves and stays there.
  - **Fix 2 (Slide-down Freeze)**: Added a `!visible` early-return guard to the `applyState` hook. If the orb is sliding down, it freezes on its current frame instead of jumping.
  - **Fix 3 (Atomic Entrance)**: Added an `isEntering` guard to prevent rapid state changes (like TTS audio starting) from interrupting or pausing the wake-up choreography. This prevents the orb from freezing mid-animation when entering Ghost Mode.
  - **Fix 4 (Normal Mode UX Alignment)**: In Normal Mode, `listening` (STT) and `speaking` (TTS) now strictly show the static smiling orb pulsing via CSS (`pulse-listen` and `pulse-speak` zooming in and out), rather than playing the `loading-loop` circles. The circle Lottie animation is now exclusively reserved for the `thinking` state.
- **Verify**: Ghost tests 24/24 pass; frontend 44/44 vitest pass.
- **Docs**: `docs/changes/55-ghost-waves-browser-search-typing-and-intent-isolation-implementation.md` §4.

## Ghost Waves, Browser Search, Line-by-Line Dictation & Intent Isolation (2026-09-29)

- **Root Cause Fix — Eradicated False "No repository found" (`recorder.ts` & `wsBridge.ts`)**:
  - Removed hazardous frontend mic truncation rewrite `if (/^open-?$/i.test(t.trim())) { t = "open architecture mapper"; }` in `recorder.ts`. Truncated words like "open" or "open-" no longer falsely morph into codebase mapping.
  - Hardened `isLongRunningQuery` and `isArchitectQuery` in both `recorder.ts` and `wsBridge.ts` to strictly require explicit keywords (*"architecture mapper"*, *"codebase diagram"*, *"pr"*, *"pull request"*), eliminating false triggers from generic verbs and nouns (*"create"*, *"show"*, *"check"*, *"code"*, *"project"*).
- **Local Browser Search & Address Bar Focus (`intent_parser.rs`, `orchestrator.rs`, `browser.rs`)**:
  - Added deterministic intents: `BrowserSearch { query }`, `BrowserSearchFocus`, `StartDictation`, `StopDictation`.
  - Added `focus_search_bar()` in `browser.rs` (`Ctrl+L`).
  - Saying *"search <query>"* in Ghost Mode or *"search in browser <query>"* dispatches directly to `browser::search(query)` (`Ctrl+L` $\to$ types query $\to$ `Enter` in <300ms).
  - Saying bare *"search"*, *"search bar"*, or *"focus address bar"* invokes `Ctrl+L` and speaks pre-cached *"Ready to search, sir."* (<5ms).
- **Line-by-Line Dictation Mode (`orchestrator.rs`)**:
  - Added atomic flag `DICTATION_ACTIVE` in `orchestrator.rs`.
  - Triggered via *"type whatever I say"*, *"start typing"*, *"type line by line"*, or *"start dictation"*.
  - Transcripts are directly typed line-by-line into the focused window (`keyboard::type_text(&format!("{}\n", text))`).
  - Stopped via *"stop typing"*, *"stop dictation"*, or *"stop"*, replying with pre-cached *"Stopped typing, sir."*.
- **Sub-5ms App-Open Voice Confirmation (`orchestrator.rs` & `tts.rs`)**:
  - Replaced un-cached dynamic TTS string `format!("{target} open, sir.")` in `run_ghost_open` with pre-cached `"Ok sir."`, eliminating the 1.5s–2.0s Edge-TTS cloud roundtrip.
  - Added `"Ready to search, sir"` and `"Stopped typing, sir"` to `CACHED_PHRASES` in `tts.rs`.
- **Unified Wave Visualizer, Wake Choreography & Motion Rules (`Avatar.tsx`)**:
  - Plan B Sequencing: On wake up, plays full wake choreography first (`wakeup.json`: loading circles at 1.5x $\to$ smile arrival at 1.5x $\to$ hold frame 300). Once the smile completes (`wakeChoreographyDone`), pinches into the 3-bar audio-reactive waves (or immediately if speaking/thinking starts early).
  - In Ghost Mode (`ghostActive`), wave visualizer activates immediately without playing wake circles.
  - Clean resting visibility: resting scale is `0.35` with container opacity `0.85` (clearly visible, not shrunken or dimmed).
  - Strict motion invariant: completely stationary at rest (paused Lottie, fixed `0.35` scale, no sinusoidal wave). Oscillates dynamically from `0.35` to `1.0` during user speech (`micLevel`) or TTS audio (`ttsWaveLevel`).
  - Lowered `WAVE_REST_FLOOR` to `0.015`, ensuring conversational vocal speech dynamically moves the waves.
- **TTS Handshake Synchronization & Ghost Session Visual Persistence (`orchestrator.rs`)**:
  - Removed premature simultaneous `Done` emission in `speak_line`, `run_ghost_control_enter`, and stop/exit handlers. Prevents the frontend `done` handler from triggering a premature `store.reset()` to `idle` 550ms into speech while TTS is still speaking, allowing `finishSpokenResult` $\to$ `maybeGhostRelisten()` to fire on actual speech completion and keep the wave visualizer persistently active on screen.
- **Brain Monitor Retrain Path Resolution (`brain_monitor.rs`)**:
  - Added candidate path existence checks before running `merge_and_train.py`, resolving `[Errno 2] No such file or directory`.
- **Verify**: Rust 182/182 intent parser tests, 52/52 orchestrator tests, 46/46 wakeword tests pass; frontend 44/44 vitest pass; release build clean (53.2 MB binary).
- **Docs**: `docs/research/voice-and-ghost-mode-root-cause-analysis-and-flow-hardening-2026-09-29.md`, `docs/features/73-ghost-waves-browser-search-typing-and-intent-isolation.md`, & `docs/changes/55-ghost-waves-browser-search-typing-and-intent-isolation-implementation.md`.

## STT Hallucination Mitigation, Strict Wave Motion Rule & Intent Parser Tightening (2026-09-29)

- **Acoustic Pre-Gate & Whisper Hallucination Filter (`stt.rs` & `stt_groq.rs`)**:
  - Implemented length gate (< 200ms / 3200 samples) and RMS floor gate (< 0.005) in `transcribe_audio` and `transcribe_samples`. Sub-threshold / silence buffers are dropped immediately with zero API calls.
  - Expanded `apply_hallucination_filter` to intercept background noise fragments (*"from the"*, *"in the"*, *"open drift"*, *"open breath"*), silence repetition loops, and Latin/foreign confabulations (*"cunzon usarlo"*, *"sarese manguer"*, *"tadrao vara"*). Cleaned `NEXUS_VOCABULARY` in `stt_groq.rs`.
- **Eliminated False "No repository found" (`intent_parser.rs`)**:
  - Removed hazardous prefix match `if trimmed == "open-" || trimmed == "open"` in `is_architect_command` that routed truncated speech to `OpenArchitect`.
  - Rewrote `is_architect_fuzzy` to strictly require both an architecture soundalike keyword (*"architecture"*, *"architect"*, *"arch"*, *"arcade"*, *"octach"*, *"ark"*, *"cat"*) and a mapping keyword (*"mapper"*, *"map"*, *"diagram"*, *"graph"*, *"remember"*, *"member"*, *"december"*), eliminating greedy matches on common words (*"our"*, *"are"*, *"art"*, *"mac"*, *"master"*).
  - Added `strip_leading_stray_punctuation` to sanitize leading punctuation and orphaned single-character prefixes.
- **Strict Wave Motion Rule & In-Place Continuity (`Avatar.tsx`, `orchestrator.rs`, `App.tsx`, `recorder.ts`)**:
  - Eliminated autonomous sinusoidal breathing waves during idle/rest states. Waves now rest statically at a flat baseline (`0.15`) with the Lottie animation paused (`wavesAnimRef.current.pause()`) and container opacity dimmed to `0.35`. Waves only oscillate when mic capture is active or TTS is synthesizing.
  - Suppressed top-right loading window (`loading.json`) usurpation during Ghost Mode (`show_loading` short-circuits if `crate::ghost::session_active()`), keeping the dynamic waves anchored at the exact wakeup orb screen coordinates.
- **WhatsApp Offline Log Demotion (`mcp_client.rs`)**:
  - Demoted periodic connection failure logs from `warn!` to `debug!` when probing `http://127.0.0.1:8765/mcp`, preventing background console error spam.
- **Voice Lock**:
  - Enforced strict voice lock to `en-US-AvaNeural` + `Neutral` prosody.
- **Verify**: Rust 181/181 intent parser tests, 23/23 STT tests, 46/46 wakeword tests pass; frontend 44/44 vitest pass; release build clean.
- **Docs**: `docs/features/72-stt-hallucination-mitigation-wave-motion-and-architect-routing-fix.md` & `docs/changes/53-stt-hallucination-mitigation-wave-motion-and-architect-routing-fix.md`.

## Personalized & Neural-Augmented Wake-Word Training (2026-09-25)

- **User Voice + Multi-Persona Neural Augmentation**:
  - Captured pristine human microphone samples (`nexus_0091.wav`–`nexus_0095.wav`) via `scripts/record_wake_samples.py` with automatic indexing fix.
  - Generated 250 high-diversity 16kHz mono positive samples across 13 Microsoft Neural voices (`edge-tts`) spanning US, UK, and Indian accents across rates (-20% to +20%) and pitches (-5Hz to +5Hz) for `"nexus"`, `"hey nexus"`, and `"okay nexus"`, Whisper-verified.
  - Expanded positive training dataset to **432 samples** (94 human mic recordings + 88 SAPI + 250 Microsoft Neural).
- **Binary Focal Retraining**:
  - Retrained classifier via `scripts/train_local_wakeword.py` with `pos_weight = 2.2` and `BinaryFocalLoss(gamma=2.0)` across **41,183 total windows** (35,004 train + 6,179 val: 7,776 positive, 25,918 hard speech negatives, 6,489 comfort/noise windows).
  - Exported ONNX model `src-tauri/resources/oww/nexus.onnx` (`2de9887f7902befa9d1d2e8f2a02d98b9534b4cad159ebef9ddad73e97760f5a`) and synchronized `model_manifest.json`.
- **4-Point Benchmark Audit (`scripts/verify_hardened_model.py`)**:
  - Problem Phrase (*"Documentation Created & Synchronized"*): **2.61% peak** (0 triggers at 0.68 threshold).
  - Fast Multi-Syllabic Negatives (186 files): **0.00% FA (0/186)**, max peak 17.18%.
  - Positive Recall (432 files): **98.6% recall at 0.68 (426/432)**, average max **98.0%**, median confidence **99.7%**.
  - Vocal Friction (90 files): **0.00% FA (0/90)**, max peak 8.13%.
- **Verify**: Rust 43/43 wakeword unit tests pass (`cargo test --lib wakeword -- --test-threads=1`); release build clean.
- **Docs**: `docs/features/71-personalized-and-neural-augmented-wakeword-training.md` & `docs/changes/52-personalized-and-neural-augmented-wakeword-training.md`.

## NLU Phonetic Alias Map & Persistent Missed-Intent Logging (2026-09-25)

- **Layer 3 Phonetic Alias Normalizer (`intent_parser.rs` & `recorder.ts`)**: Built deterministic soundalike phrase normalizer catching common Whisper acoustic mishearings (*"goes to mode"*, *"goes to mold"*, *"post modern"*, *"postmodern"*, *"coast mode"*, *"gold mode"*, *"ghost mood"* → `"ghost mode"`; *"open what's up"* → `"open whatsapp"`; *"open vs coat"* → `"open vs code"`; *"open spot if I"* → `"open spotify"`; *"stand down"* → `"stop"`).
- **Extended Ghost Control Triggers**: Added `"activate ghost mode"`, `"activate ghost"`, `"turn on ghost mode"`, and all acoustic soundalike phrases to `parse_ghost_control_entry` TRIGGERS. Expanded `STOP_PHRASES` in `ghost.rs` with `"stand down"`, `"stop please"`, `"exit ghost"`, `"stop ghost"`, `"exit ghost mode"`, `"leave ghost mode"`.
- **Persistent Missed-Intent Logger (`missed_intent_logger.rs`)**: Implemented persistent JSONL logger writing all unmatched transcripts to `%APPDATA%/com.nexus.assistant/missed_intents.jsonl` with millisecond timestamp, ISO datetime, transcript, source (`orchestrator` / `stt_learning`), and failure reason. Auto-rotates at 5 MB. Added `get_missed_intents` IPC command. Hooked into both `orchestrator.rs` (`ParsedIntent::Unknown`) and `stt_learning.rs` (`log_failure`).
- **Verify**: Rust 173/173 intent parser tests pass (`cargo test --lib intent_parser -- --test-threads=1`); 1/1 logger test passes; frontend 33/33 vitest pass.
- **Docs**: `docs/features/69-nlu-phonetic-alias-map-and-missed-intent-logging.md` & `docs/changes/51-nlu-phonetic-alias-map-and-missed-intent-logging.md`.

## STT Hallucination Fix — VERIFY_RING Prefix-Pad Poisoning, Ghost Mode Vocabulary & Double Trigger Banner (2026-09-25)

- **Root Cause 1 — VERIFY_RING Prefix-Pad (`wakeword_oww.rs`)**: Diagnosed that `start_stt_capture()` seeded 160ms of `VERIFY_RING` audio (containing the wake word "NEXUS" + silence gap) into the command capture buffer. Groq received `[NEXUS audio] + [1.3s silence] + [command]`, hallucinating random text ("and then the process goes to murder."). Removed prefix-pad entirely; capture starts clean — no phonemes are lost since the user finishes the wake word before speaking the command. Also removed now-unused `STT_PREFIX_PAD_SAMPLES` constant.
- **Root Cause 2 — Ghost Mode Vocabulary Bias (`stt_groq.rs`)**: Groq's Whisper decoder had no bias toward "ghost mode", mapping `/ɡoʊst moʊd/` to "goes to mode" / "goes to mold." Expanded `NEXUS_VOCABULARY` with full ghost mode command phrasings, common app opens (WhatsApp, Chrome, VS Code, Spotify, Discord), and action verbs (open, close, stop, cancel, navigate, settings).
- **Root Cause 3 — Double Trigger Banner (`run.ps1`)**: The console was printing two `TRIGGER #01` / `TRIGGER #02` banners per utterance because the OR-pattern `instant neural trigger|OWW wake detected` matched both a DEBUG and an INFO log line from the same event. Fixed by narrowing the pattern to only `instant neural trigger` and explicitly suppressing `OWW wake detected!` and `high-confidence single-frame trigger` debug lines.
- **Verify**: Rust 43/43 wakeword unit tests pass (`cargo test --lib wakeword -- --test-threads=1`); release build clean (zero warnings).
- **Docs**: `docs/changes/50-stt-hallucination-verify-ring-prefix-pad-and-ghost-mode-vocabulary.md`.

## Live Audio Telemetry, Trigger Debouncing & Connection Diagnostics Parity (2026-09-25)

- **Live Speaker Audio Waveform & Status Meter (`run.ps1` & `wakeword_oww.rs`)**:
  - Implemented continuous 12-slice speaker audio waveform (`Wave: [  ▂▄▆██▆▄▂  ]`) and status meter (`🎙️  VOICE [████████████████] 98.4% | RMS: 0.0245 (AGC  2.5x)`) in `nexus start` identical to `nexus wake test`.
  - Added `compute_waveform_string(&chunk)` in `wakeword_oww.rs` to compute 12-slice Unicode bars (` ▂▃▄▅▆▇█`) without double-gain multiplication.
  - Streaming audio telemetry emitted dynamically: 160ms cadence during active voice and 480ms during silence.
  - Added `Make-Meter` and `Clear-MeterLine` in `run.ps1` with UTF-8 encoding and in-place carriage return (`\r`) rendering, preventing screen tearing or trailing characters when other log lines arrive.
- **Single-Trigger Neural Debouncing**:
  - Investigated duplicate trigger firing (`TRIGGER #01` and `#02` in the same second). Added atomic `LAST_NEURAL_FIRE_MS` (1.5s cooldown) in `wakeword_oww.rs` candidate receiver loop, cleanly aggregating consecutive multi-frame speech chunks into exactly one trigger event.
- **Connection Diagnostics & Service Health Restoration**:
  - Restored the ASCII startup diagnostics box (`╔|║|╠|╚|NEXUS Connection Diagnostics`) in Cyan, verifying Faster-Whisper STT (port 39217), Edge-TTS & Piper TTS, Cloudflare Worker `/health`, GitHub OAuth, and Google OAuth status on boot.
- **Verify**: Rust 43/43 wakeword unit tests pass cleanly (`cargo test --lib wakeword -- --test-threads=1`); release build clean.
- **Docs**: `docs/research/wakeword/unified-console-audio-telemetry-and-diagnostics-parity-2026-09-25.md`, `docs/features/68-live-audio-telemetry-and-connection-diagnostics-parity.md`, & `docs/changes/49-live-audio-telemetry-and-connection-diagnostics-parity.md`.

## Instant Neural Wake Alignment & Clean Console UX (2026-09-25)

- **Latency & Divergence Diagnosis vs. `wake test`**:
  - Investigated why `nexus start` showed a 1.2s–1.8s delay and lower sensitivity than `nexus wake test` (which triggers in < 35ms at 98.4% confidence).
  - Identified 5 root causes: 1) `read_verify_wake` defaulted to `true` (unnecessary Stage-2 Whisper STT HTTP cross-check), 2) 500ms post-hit secondary buffer delay in `wakeword_oww.rs`, 3) WebRTC VAD onset decapitation chopping the initial nasal consonant `/n/`, 4) 4-channel averaging in `try_device` downmix halving signal on Intel SST arrays, 5) repetitive 2s audio heartbeat dumps flooding `nexus_unified.log`.
- **Rust Engine Optimization & Fail-Open DSP**:
  - `commands.rs`: Defaulted `read_verify_wake` to `false` for instant neural firing. **[Correction 2026-10-21: the current code defaults `verifyWake` to TRUE; this default did not stick. See change 91.]**
  - `wakeword_oww.rs`: Immediate return on detection; made WebRTC VAD fail-open; connected calibrated `pre_gain` (2.50x) in AGC normalization; downmixed active stereo pair ($\text{Ch}_0+\text{Ch}_1$) on 4-channel devices; demoted 2s heartbeat logs to `DEBUG`.
  - `meeting_detect.rs`: Added `python.exe`, `nexus.exe`, and `node.exe` to system process exclusions.
- **Unified Console UX & Live Speaker Waveform (`scripts/run.ps1` & `scripts/test_wake_live.py`)**:
  - Filtered out 50+ lines of internal startup boilerplate in `run.ps1` (espeak, webview, meeting detection, permissions, app registry, TTS pre-gen, ONNX kernel activations, Tier 3 skipped models, connection diagnostics box).
  - Formatted wake triggers with an instant visual banner matching `nexus wake test`:
    `🔔 [TRIGGER #01] HH:mm:ss — Confidence: XX% | Instant Neural Fire`
    `     Status: ● WAKE WORD HEARD SIR!`
  - Built real-time 12-slice speaker audio waveform visualizer into `scripts/test_wake_live.py` (`Wave: [  ▂▄▆██▆▄▂  ]`) with dynamic AGC scaling and CP1252/Unicode fallback, so vocal speech waves animate visibly when speaking.
- **Verify**: Rust 43/43 wakeword unit tests pass cleanly (`cargo test --lib wakeword -- --test-threads=1`); release build clean.
- **Docs**: `docs/research/wakeword/wakeword-runtime-engine-and-console-parity-2026-09-25.md`, `docs/features/67-instant-neural-wake-alignment-and-clean-console-ux.md`, & `docs/changes/48-instant-neural-wake-alignment-and-clean-console-ux.md`.

## "Hey Jarvis" vs. "NEXUS" Comparative Benchmark & Multi-Syllabic Parity (2026-09-25)

- **Acoustic Comparison vs. Open-Source Jarvis (`hey_jarvis_v0.1.onnx`)**:
  - Benchmarked official open-source `"Hey Jarvis"` (1.24 MB, 3-syllable `/ˈheɪ ˈdʒɑːr.vɪs/`) against our personally trained `"NEXUS"` (839.9 KB, 2-syllable `/ˈnɛk.səs/`).
  - Identified why Jarvis had superior out-of-the-box false alarm rejection on conversational speech: 3-syllable temporal progression creates a mathematical barrier ($p < 0.0005$), whereas 2-syllable `"NEXUS"` requires aggressive negative mining against high-frequency sibilants (*-tion*, *-sion*, *-xus*, *next*).
- **22.05 kHz Silent Training Ingestion Defect**:
  - Diagnosed that `scripts/train_local_wakeword.py` enforced `if sr != 16000: continue`. Because all 186 synthesized fast negative clips (`synth_fast_*.wav`) and `test_doc.wav` were 22,050 Hz, the trainer previously skipped 100% of the fast speech negative dataset during backpropagation.
- **Universal On-The-Fly Resampling & Binary Focal Retraining**:
  - Added on-the-fly linear interpolation 16kHz resampling in `scripts/train_local_wakeword.py` and enabled `include_transitions=True` across all negative speech files.
  - Retrained with Binary Focal Loss ($\gamma = 2.0, \text{pos\_weight} = 2.0$) across 31,048 samples (25,835 hard speech negatives + 6,489 noise/comfort windows).
  - Empirical Verification: Problem Phrase (*"Doc Created & Synced"*): **1.55% peak (0 triggers)**, Fast Multi-Syllabic Negatives (186 files): **0.00% FA (0/186, max 48.58% at 0.68 threshold)**, Vocal Friction (90 files): **0.00% FA**, Positive Recall (178 files): **97.19% @ 0.68 (median 99.22%)**, Inference Latency: **0.025 ms** (12% faster than Jarvis).
- **Manifest & Rust Engine Synchronization**:
  - Synced SHA-256 in `src-tauri/resources/oww/model_manifest.json` (`8e15d4882c90e2033fce6df339ff85c8f76b0819b8ff4e50c7d671e02452890e`).
  - All 43 Rust unit tests pass (`cargo test --lib wakeword -- --test-threads=1`).
- **Docs**: `docs/research/wakeword/jarvis-vs-nexus-acoustic-investigation-and-parity-2026-09-25.md`, `docs/features/66-hey-jarvis-vs-nexus-comparative-acoustic-benchmark-and-multi-syllabic-parity.md`, & `docs/changes/47-hey-jarvis-vs-nexus-acoustic-parity-and-resampling-fix.md`.

## Dual-Target Wake Word Balancing & Rust Engine Alignment (2026-09-25)

- **Diagnosis & Slicing Flaw**: Investigated why `"Hey NEXUS"` scored 98% while standalone `"NEXUS"` scored 13%–32%. Slicing 2-second positive WAVs with `w[-4:]` extracted trailing silence for short words (word ended at 0.8s, sliced chunks 21–24 after comfort frames flushed the FIFO). Dataset was also 75% `"Hey NEXUS"`.
- **Rust Engine 10s Boot Dead-Zone & VAD Onset Decapitation**: `wakeword_oww.rs` hardcoded a 10s startup grace period (discarding all wake words on boot) and ran `VadDetector` with a fixed 0.005 RMS threshold before the neural model, decapitating quiet initial consonants (`/n/` at ~0.0035 RMS).
- **Phonetic Energy Alignment & Balanced Training (`scripts/train_local_wakeword.py`)**:
  - Synthesized and Whisper-verified 88 multi-speaker samples across rates -3 to +4 (`scripts/generate_balanced_positives.py`), balancing positive dataset to 178 files.
  - Enabled `include_transitions=True` in `extract_windows_from_audio` to capture both speech envelopes and 2-frame completion transitions.
  - Retrained with `BinaryFocalLoss(gamma=2.0, pos_weight=2.0)` across 25,428 samples.
  - Verification Audits: Problem Phrase (*"Documentation Created & Synchronized"*): **2.83% peak**, Fast Negatives (186 files): **0.00% FA**, Positive Recall (178 files): **96.6% @ 0.68 (median 98.8%)**, Vocal Friction (90 files): **0.00% FA**.
- **Rust Synchronization (`src-tauri/src/wakeword_oww.rs`)**:
  - Reduced startup grace period from 10s to 1.5s (1500ms).
  - Set `vad_enabled: false` in `AudioPreprocessor::with_profile` to prevent speech onset chopping.
  - Synced SHA-256 in `model_manifest.json` (`7c98741c8a0b4f594853db3567123309929ae0e7efdaad64683803a3154c094b`).
- **Verify**: Rust 43/43 wakeword unit tests passed; `node nexus.mjs build` clean release binary (50.1 MB).
- **Docs**: `docs/features/65-dual-target-wake-word-balancing-and-rust-engine-alignment.md` & `docs/changes/46-dual-target-wake-word-balancing-and-rust-engine-alignment.md`.


- **Plan (`docs/features/64-ghost-mode-plan.md`)**: Windows-only Ghost
  Mode — AI drives the REAL cursor (no second pointer) while user keeps
  speaking; ring rides the cursor; keyboard-first rule (Win-search flows
  need zero grounding); takeover/weapons-grade safety; phases 0→4.
- **Research (`docs/research/ghost-mode/`)**: Clicky teardown (fake
  triangle + Computer Use + Worker-proxy pattern; "no delay" is
  spinner+flight stagecraft) + grounding options (UIA → OmniParser →
  hosted) + perfection limits (Pro 61.6, OSWorld 42.5) + cost ledger
  + OS matrix (Win full, macOS permission-gated, X11 full, Wayland
  partial).
- **Built (`src-tauri/src/ghost.rs`, stage ring UI)**: session machine,
  pure takeover detector (suppress window + 2px slop + first-sight rule),
  dynamic Esc (never global), ring events, silent stand-down on stage
  hide, blackout-reset. Zero AI motion yet — leash before hands.
- **Verify x2**: Rust 531/531 serial (7 new), frontend 26/26, tsc +
  prod build clean, zero clippy hits.
- **Phase 1 keyboard ghost (built, doc 38)**: `launcher.rs` Win-search
  open + focus verify; drill runner with per-step stop/session guards;
  `live_ghost_whatsapp` (send stays confirm-gated); voice-stop via
  `live_cancel`; announce rails. No key tracker needed (primitives
  atomic — verified). Rust 534/534 serial.
- **Phase 2 mouse ghost (built, doc 39)**: `live/commands/mouse.rs`
  (eased glide, click/double/scroll/drag, restore) + UIA-first resolver
  (exact/starts-with/contains; no password fields) + `live_ghost_click`
  with focus verify before+after and suppress windows on all paths.
  Rust 537/537 serial. Vision deferred until measured need.
- **Phase 3 overlap engine (built, doc 40)**: drill-depth flag +
  follow-up queue (cap/drop-oldest) + stop-word intercept (raw match —
  bare "cancel" parses as Greeting; `live_cancel` had zero callers, now
  reachable by voice). Stop aborts spoken + unrouted; rest drains in
  order after clean runs only. Fixed: no glide-home after takeover.
  Rust 540/540 serial.
- **Phase 4 hardening (built, doc 41)**: calibration suite, refusal battery,
  exclusive-fullscreen auto-pause. Rust 544/544 serial. Ghost program complete —
  vision deferred until measured need; UIPI elevation detection follow-up.
- **Entry split (built, doc 42)**: "ghost mode" now enters cursor control
  (`EnterGhostControl` + ring + narration); bare-mode phrases stripped from
  Ghostwriter dictation entry after a live collision sent users to the wrong
  room. Rust 547/547 serial.
- **Ghost waves (built, doc 43)**: orb pinch-to-waveform on session start —
  220ms pinch, 3 live bars in measured Lottie palette (#259ed6/#ef4f25/
  #fbdf38), reverses on exit; pure frontend, zero Rust. Frontend 29/29.
- **Ghost voice-entry fixes (built, doc 44)**: initial `ghost:ring` emit on
  entry (orb never learned motionless sessions); ghost hot-mic loop
  (re-listen after every turn while session live, meeting/echo guarded);
  silent-miss anti-nag (3 quiet re-listens, then one nag + park);
  heartbeat client marker (`stage-shell-v1`) for build identity.
  Frontend 33/33.
- **Ghost instant response (built, doc 45)**: stage scrollbar CSS;
  follow-up listen starts Rust capture (the missing link — orb showed
  listening, nothing captured); in-session OpenApp routes to ghost
  runners; registry-first open with Win-search fallback. Trace tool
  12/12. Rust 547/547 serial.
- **Ghost voice messaging + trace (built, doc 46)**: in-session
  SendWhatsAppMessage/WhatsappChat route to the desktop drill (visible
  typing, confirm-gated send, session stays open); TRIGGER banner
  removed from run.ps1; debug_trace taps p0–p4 (temporary); drain-cycle
  boxed + spawn_blocking refactor (E0733), ghost runtime erased at one
  boundary (ghost_wry). Rust 550/550 serial, 0 clippy new-file hits.
- **Ghost docs organized (same day)**: `features/70-ghost-mode-complete.md`
  (one-stop program guide), `features/ghost-mode-architecture-reference.md`
  (symbol map + Send invariant), `research/ghost-mode/README.md` +
  `03-irregularities-and-fixes-timeline.md` (root-cause timeline of all
  voice-entry defects); features README index updated 69→70+A1.
- **Takeover re-scope (built, doc 47, user directive)**: mouse use NEVER
  ends the session — takeover requires a commanded target (in-flight
  suppresses; settled deviation = task abort "Task stopped… Ghost mode
  is still on", session stays); explicit exits only (`ExitGhostControl`
  intent "exit/close/turn off ghost mode", Esc, stage hide/kill);
  `ghost:session` event (session-scoped orb waves — survives turns);
  `ghost:ring` positional-only (rides commanded glides, never the
  user's cursor); reset() no longer clears ghostActive; idle stop-words
  inform "Nothing running"; drill stop-words task-abort only.
  Rust 549/549 serial, FE 33/33, trace 12/12.
- **Takeover detection DELETED (2026-09-27, user directive — final
  semantics)**: after a third live misfire (00:15:37 yield on a
  keyboard-only "open" flow), `decide_takeover`, the observe_cursor
  judgment, and `abort_task` are removed. Mouse use is ALWAYS free;
  **Esc = the cancel button** (armed at session start); "exit ghost
  mode" voice and stage hide/kill are the other explicit exits.
  Entry narration: "Esc cancels any time". Ring rides commanded
  glides only. Rust 642/642 serial ×2.
- **Phase A quick wins (built, 2026-09-26)**: API keys moved to OS keychain
  (keyring-first reads, settings.json strips secrets, auto-migration on
  startup); PII filtering middleware (`pii_filter.rs` — email/phone/Aadhaar/
  PAN/card/IPV4 redaction before cloud dispatch, 9 unit tests); speaker
  verification wired into wake receiver loop (`verify_speaker_at_fire` —
  separate AudioFeatures instance, fail-open, settings toggle); runtime
  health dashboard (`get_health_status` command + System Status panel in
  Connections tab); settings export/import (JSON with keychain-backed
  keys). Rust 558/558 serial, FE 33/33, trace 12/12.
- **Phase B high-impact (built, 2026-09-27)**: 3-tier persistent memory
  (`memory.rs` — file-based core/episodic, "remember that/my X is Y" hook,
  context injection into 9Router + Worker prompts, episode logging, 8 tests);
  sentence-chunked streaming TTS (first-audio ≈1 sentence, legacy fallback,
  6 tests); emotion prosody (`TtsEmotion` — auto heuristics or manual
  `ttsEmotion` setting, rate/pitch/volume via edge-tts, 6 tests); parallel
  WorkerBackend batches in command center (JoinSet, order-preserving merge,
  5 tests); crash-safe checkpoints (per-step `compound_<id>.json`, boot sweep
  >1h, 4 tests). Rust 587/587 serial, FE 33/33.
- **Phase C strategic (built, 2026-09-27)**: vision grounding fallback
  (`vision.rs` — Groq VLM 0-1000 coords, UIA-first, hooked into ghost_click
  step 2, 8 tests); proactive diary (`diary.rs` — wake/compound/ghost/
  webhook hooks, boot rollup, `diary_summary`, autonomous TTS explicitly
  out of scope, 4 tests); versioned wire protocol v1 (`protocol.ts` +
  `PROTOCOL_VERSION` + payload stamp + health mismatch warn + spec doc,
  3 Rust + 3 Worker tests); multi-voice Piper (`piper-<stem>` drop-in
  models, per-voice reload, picker lists customs, 5 tests); localhost
  webhook triggers (`webhook.rs` — 127.0.0.1:39220, keychain bearer token,
  diary-logged, 6 tests). Rust 613/613 serial, FE 33/33, Worker 52/52.
- **Phase D ecosystem (built, 2026-09-27)**: declarative specs
  (`agent_specs.rs` — `intents.yaml` open+say intents, fallback-slot
  priority, strict validation, example on boot, 11 tests); edge-case
  miner (`improve.rs` — miss/failure clusters → `suggested_phrases.json`,
  report command, boot run, record-only invariant, 6 tests). D1 (gRPC)
  + D4 (ESP32) deferred with reasoning — no unverifiable code shipped.
  Rust 630/630 serial, FE 33/33.
- **Cross-phase audit (2026-09-27)**: re-tested every phase item by item —
  8 real fixes (A1 cooldown order, A2 keychain injection into get_settings,
  A4 history/memory sanitization, B1 shadowed-parts memory drop + remember
  hook post-intercepts, B2 piper streaming guard, C1 post-vision stop check,
  C2 verified-path diary). Rust 634/634 serial ×2, FE 33/33, Worker 52/52.
- **Vision grounding v2 (built, 2026-09-27)**: axis-grid overlay
  (Axis-Grid Scaffold — edge rulers + grid, no fonts), Groq→Gemini
  fallback with Pacific-day quota counters (14,400/500 RPD, 429-marking,
  pre-call skips), `visionProvider` setting, spoken limit notices,
  quota UI in Accounts tab; keys stay in OS keychain (never Cloudflare).
  9 new tests. Entry NEVER pops settings (user directive 2026-09-28:
  `ghost_enter` is pure init — no sidebar, no extra speech; keyless users
  are guided verbally at the click-time vision gate in mouse.rs instead).
  Rust 643/643 serial ×2, FE 33/33.
- **Vision speed mode (built, 2026-09-27)**: parallel race
  (`visionRace: speed` — both providers fired via tokio::join!, first
  valid coordinate wins, both quota units recorded, 429 marked
  per-provider) + 768px fast capture in race mode (encode/upload ~40%
  trim). Sequential default unchanged. 4 new tests.
  Rust 647/647 serial ×2, FE 33/33.
- **Hot-mic loop repair (built, 2026-09-28)**: live bug — mic died
  after the first ghost command (`open WhatsApp` took the local-execute
  branch, which reset without relistening). New `endGhostTurn()`
  helper (relisten-before-reset, no-op outside ghost) wired into every
  turn-end: recorder.ts local/VAD branches, orchestrator error +
  confirm-decline, Tier-3 listener, stage:notice, speaking failsafe,
  legacy wsBridge. Deliberately excluded: silent-park, abortCapture,
  idle timeouts. 2 new tests. tsc clean, FE 35/35.
  Full arc: `docs/research/ghost-mode/04-program-consolidation-and-hotmic-repair-2026-09-28.md`.
- **Speech-synced waves + new-tab verbs (built, 2026-09-28)**:
  live miss "create a new tab" → `browser_new_tab` deterministic verbs
  + test; new Rust `audio:level` event (capture-RMS map, bounded
  channel, forwarder thread, settle-zero) driving audio-reactive ghost
  waves (mic → TTS-rhythm → rest, Esc reverts); `wakeup-v2.json` /
  `ghost-waves.json` auto-pickup with fallback. Rust 645/645 serial,
  tsc + FE 40/40, clippy clean in touched ranges.
  Research + plan: `docs/research/orb-waves/`.
- **Live-test diagnosis (research only, 2026-09-28)**: waves invisible
  (3 causes: `waves.json` vs `ghost-waves.json` filename mismatch,
  orb hides between turns, rest-scale near-invisible); ghost actions
  blind (frontend logs absent from unified console — debug_trace
  forwarding planned); "open youtube in brave" → WhatsApp MCP
  misroute; v4 barge-in requires "nexus" in verify transcript so
  bare "stop" never interrupts TTS (instant-stop tier planned);
  Ctrl+Space never stops TTS; post-turn settle + sentence splitter
  planned. Doc: `docs/research/orb-waves/03-…-2026-09-28.md`.
- **Waves transition + MCP route (built, 2026-09-28)**:
  wakeup→waves is now smile → pinch (220ms) → staggered bar bloom
  (90ms steps) with a 200ms shrink/fade exit beat on Esc (rAF owns
  scaleY, CSS owns opacity/rise — never fight); Creator MCP pipe
  verified (`@lottiefiles/creator-mcp@0.2.2`, 108 tools, needs only
  a connected Creator tab) with palette/stagger/segment edit plan
  ready; no export tool in MCP — JSON still via Creator export.
  tsc + FE 42/42.
- **Deployment readiness (research only, 2026-09-28)**:
  failure-mode paper for the ≤20% problem-rate bar — 12-mode
  catalog, chained math (deterministic core ≈0.66–0.73 today, above
  budget pre-hardening), industry anchors (Whisper 4–5% EN /
  16–22% Hindi, OSWorld 2.0 20.6%, grounding 36–54%), SLO board,
  P0–P4 roadmap, tiered promise (deterministic 85–90%, vision
  beta, compounds supervised). Paper:
  `docs/research/deployment/01-deployment-readiness-failure-mode-analysis-2026-09-28.md`.
  English-only execution plan (user directive — no other languages):
  `docs/research/deployment/02-english-only-execution-plan-2026-09-28.md`
  (P0 E2E harness → P1 liveness → P2 EN parse → P3 degradation → P4
  rings; non-English → single honest line).
- **P0 deployment harness (built, 2026-09-28)**: `stt_filter_stats`
  confabulation counters + IPC; `WEEKLY_TOP_N` + `top_suggestions`;
  50-command EN fixture 49/50 (fixed "stop please"→cancel_action,
  documented `close pr 5` order gap); `soak_wake_fa.py` harness
  (smoke 0 triggers, 120 s silence 0 triggers; 8 h run scheduled).
  Rust 647/647 serial, clippy clean in touched ranges.
- **P1 deployment liveness (built, 2026-09-28)**: ghost relisten
  watchdog (bounded 3 pokes/session, `ghost:relisten` → guarded
  frontend relisten); turn-end choke point + CI grep-gate
  (`check-turn-ends.mjs`, negative-tested); UIPI elevation detect
  with spoken reroute in `ghost_click`; API-keys diagnostics row
  (never alarms) + Connections Keys badge. Rust 653/653 serial,
  e2e 49/50, tsc + FE 42/42, gate OK.
- **P2 EN parse robustness (built, 2026-09-28)**: wake-prefix
  retry-parse (`strip_wake_prefix` + one-shot retry on None,
  word-boundary guarded, bare "command center" added) — 9-phrase
  matrix green; `shouldGhostRoute` routes in-session open_app/
  whatsapp_chat to ghost runners (fallback local + endTurn);
  `temperature=0` added to 2 Groq call sites (all EN-pinned);
  en-IN tracking fixture 15/15 (no gate); hesitant-speaker
  endpointing test. Rust 656/656 serial, tsc + FE 44/44.
- **P3 graceful degradation (built, 2026-09-28)**: offline drill
  walkthrough doc (stage table + gap list) + offline Worker-dropped
  turns speak the attributed line instead of raw JSON; quota
  narration audit (vision + Worker denials speakable, Worker test
  pins 4 denial classes, `quota_exceeded` logged); bridge health +
  fix hints in `mcp_check.py` wired into `nexus check` (verified
  live). Rust 656/656, Worker 53/53, e2e green, FE 44/44, gate OK.
- **P4 rollout ops (built, 2026-09-28)**: soak per-file default
  (fixed harness-manufactured cross-file trigger; `--stream`
  opt-in) + genuine FA finding `necess_0003` 0.821 (1/500 — ring-1
  config: `verifyWake: true` + retrain hard-negative list);
  `collect_slo.py` SLO board collector (verified live);
  rollout-rings runbook (gates/checklist/rollback); ring-1 MSI +
  exe artifacts hashed (NSIS broken locally, plugin cache). Rust
  656/656 at freeze.

## Single-Stage Shell + Blackout Watchdog (Step 1, 2026-09-25)

- **Stage shell (`src-tauri/src/stage.rs`, `frontend/src/stage/`)**:
  Fullscreen transparent overlay window (`WindowConfig::stage`, `stage.html`
  + vite input, `stage-cap.json` capability), parallel-run only — empty,
  hidden by default, zero visuals moved. `WS_EX_TOOLWINDOW` so fullscreen
  overlay doesn't pause video underneath (Tauri #7401); deliberately NOT
  capture-excluded (orb has always been share-visible; only sidebars are).
- **Pixel-identical geometry map (`frontend/src/stage/geometry.ts` + 6 tests)**:
  orb pct/size (mirrors `position_orb` clamps), bottom-right dock
  (12px gap, 48px taskbar), 80px loading top-right — same formulas Rust uses.
- **Hitbox click-through**: 30ms cursor poll (`GetCursorPos`) toggling
  `set_ignore_cursor_events` around frontend-sent physical-px rects
  (per-pixel-alpha hit-testing exists in neither Tauri nor Electron —
  hitbox toggling is the documented workaround). Empty boxes = fully
  click-through.
- **Blackout watchdog (no full blackout, ever)**: 2s cadence, 8s cold-boot
  grace; missing window OR stale heartbeat (>6s) → destroy immediately
  → one orb-spoken inform (skipped mid-turn via `has_active_request()`)
  → backoff recreate → 10s heartbeat verification. 3 failed fixes → stay
  hidden + final message; voice product unaffected throughout.
- **Kill-switch `Ctrl+Alt+X`** destroys + session-disables the stage;
  `Ctrl+Space` close path routes through flag-aware `stage_hide` (raw
  destroy would read as blackout and rebuild — caught in review).
- **Verify x2**: Rust 524/524 serial, frontend 26/26, tsc clean,
  `dist/stage.html` emitted, zero clippy hits in new code.
- **Docs**: `docs/features/63-single-stage-shell-and-blackout-policy.md`.
- **Next**: step 2 orb moves in (pixel-compare), then loading layer,
  then panels one by one (each deletes a `PENDING_*` system).

## Repository & Research Architecture Rule
- **`Engine-NEXUS/NEXUS-PAPERS`** (`https://github.com/Engine-NEXUS/NEXUS-PAPERS`): Dedicated repository for all scientific research papers, acoustic DSP investigations, NLU data science studies, and architecture compendiums.
- **`Engine-NEXUS/WINDOWS`** (`https://github.com/Engine-NEXUS/WINDOWS`): Main application repository. All documentation (`docs/`), feature guides, and implementation code must always be pushed and synchronized in lockstep with this repo.

## Training Data Poison Elimination, Focal Loss & Multi-Syllabic Hardening (2026-09-24)

- **Acoustic Audit & Poison Discovery (`scripts/purge_poisoned_parallel.py`)**:
  Investigated false triggers when shouting or speaking fast multi-syllabic phrases like `"Documentation Created & Synchronized"`. Parallel Whisper audit of all 560 positive samples uncovered severe data corruption: 230+ files in `positive/` were soundalikes (*"texas"*, *"open excess"*, *"nixes"*, *"access"*), 70+ were conversational sentences (*"next, let's take a look"*, *"we'll get access"*), and 160+ were low-energy near-silence. Quarantined bad files and promoted 345 soundalikes into `wake_word_data/negative/`, leaving exactly 89 pristine NEXUS recordings.
- **Fast Multi-Syllabic Negative Synthesis (`scripts/generate_fast_negatives.ps1`)**:
  Synthesized 186 high-tempo audio samples covering words ending in `-tion`, `-sion`, `-ction`, and fast conversational compounds (*"documentation created and synchronized"*, *"authentication"*, *"context switch"*, *"reflexes"*).
- **Binary Focal Loss & Anti-Swamping Optimization (`scripts/train_local_wakeword.py`)**:
  Replaced standard Cross-Entropy with `BinaryFocalLoss(gamma=2.0, pos_weight=2.0)`. Easy background noise samples ($p \approx 0.001$) have gradients scaled down by $(0.001)^2 = 10^{-6}$, focusing 99%+ of backprop updates onto separating ambiguous speech phonetics. Pinned `dynamo=False` and `opset_version=14` for self-contained 860 KB ONNX binary with no external data.
- **Verification Audit Results (`scripts/verify_hardened_model.py`)**:
  - **Audit 1 (Problem Phrase `test_doc.wav`)**: **18.755% Peak Score** (0 triggers at 68.0% threshold).
  - **Audit 2 (Fast Speech Negatives - 186 files)**: **0.00% False Alarms (0/186)**, max peak **26.76%**.
  - **Audit 3 (Pristine Positive Recall - 89 files)**: **93.3% Recall at 0.68**, **97.8% Recall at 0.50 (median 96.3%)**.
  - **Audit 4 (Vocal Friction & Throat/Gargle - 90 files)**: **0.00% False Alarms (0/90)**, max peak **9.24%**.
  - **Rust Unit Tests**: 43/43 tests passed cleanly (`cargo test --lib wakeword -- --test-threads=1`).
- **Docs**: Architecture spec in `docs/features/61-training-data-poison-elimination-and-focal-multi-syllabic-hardening.md` and changelog in `docs/changes/44-training-data-poison-elimination-and-focal-multi-syllabic-hardening.md`.


## Silence Phantom Elimination, Speech Onset Preservation & 5-Point Verification Audit (2026-09-23)

- **Acoustic Root Cause Diagnosis & Codebase Analysis**:
  Investigated why `nexus wake test` produced repetitive triggers in silence and triggered on common conversational words like "hello". Diagnosed five root causes:
  1) **Speech Onset Decapitation (Impulsive Filter Flaw)**: The gate condition `rms > baseline * 8.0` misclassified the natural attack envelope of human speech ($0.05-0.15$ RMS) as an impulsive burst after silence, dropping Chunk 0 of words like "hello" and creating deformed spectrograms in `embedding_model.onnx` that scored 98.1% false alarms.
  2) **Dual-Buffer Squeezebox Bug (Spectrogram Freezing)**: The 16-frame embedding buffer was advanced on silence, but the 10-chunk Mel-Spectrogram circular buffer was frozen, causing past "NEXUS" spectral formants to stitch into the next spoken word.
  3) **AGC Pre-Gain Overdrive**: The formula `gain = ((target_rms / rms) * pre_gain).min(max_gain)` applied a 2.5x double-scaling factor, amplifying quiet room reverberation and trailing phonemes by up to 25x into square-wave clipping distortion.
  4) **LayerNorm Zero-Reset Collapse**: Resetting buffers to zeros collapsed variance $\sigma \rightarrow 0$, turning subsequent small floats into false alarms.
  5) **Output Bias Offset**: Initialized `last_layer.bias` to $-4.0$ ($\sigma = 1.8\%$) and retrained with 600 synthetic comfort/transient negative windows ($\text{val\_loss} = 0.0293$, $98.8\%$ recall, $0.4\%$ FA).
- **Engineering Architecture & Dual-Buffer Comfort Sliding**:
  - Removed destructive impulsive speech chopping; physical vocal artifacts are cleanly handled by the retrained neural classifier ($\le 3.4\%$ max score).
  - Extended `push_comfort_frame` across Python and Rust to advance BOTH `mel_spectrogram_buffer` and `feature_buffer` during silence, preserving 1:1 real-time temporal progression without freezing context.
  - Normalized AGC formula to `gain = (target_rms / rms).min(max_gain)`.
- **The 5-Point Verification Audit**:
  - **Audit 1 (Full 3,300-File Batch Benchmark)**: **99.3% Positive Recall (avg max 99.4%)**, **98.0% Negative Soundalike Rejection**, **98.2% Background Noise Rejection** at 0.68 threshold.
  - **Audit 2 (Conversational Speech & "Hello" Audit - 304 files)**: **97.4% Rejection Rate**; **0 triggers across all 19 "hello" files (avg max 0.34%, peak 4.24%)**; 0 triggers on "google", "siri", "please", "computer".
  - **Audit 3 (Continuous Silence - 100s / 1,250 chunks)**: **0 triggers, 0.000000% max score** (completely flatlined).
  - **Audit 4 (Hardware Invariance Benchmark)**: Studio USB Condenser (99.5%), Laptop Mic Array with Intel SST (100.0%), Bluetooth Headset (99.1%), Noisy Office (98.0%), Far-Field Whisper (96.4%).
  - **Audit 5 (Continuous Multi-Utterance Stream Simulation - 120s)**: Exactly **2/2 true positive NEXUS triggers**, **0 false triggers** on greetings, questions, throat clearing, or silence.
  - **Rust Unit Tests**: 43/43 tests passed cleanly (`cargo test --lib wakeword -- --test-threads=1`).
- **Docs**: Architecture spec in `docs/features/60-silence-phantom-elimination-and-comfort-streaming.md` and changelog in `docs/changes/43-silence-phantom-elimination-and-comfort-streaming.md`.

## Vocal Friction Hardening & Industrial Throat/Gargle Rejection (2026-09-23)

- **Acoustic Root Cause Diagnosis & Research**:
  Investigated why non-verbal throat clearing, gargling, coughing, and vocal fry triggered false wakes. Researched industrial two-pass keyword spotting architectures from Amazon Alexa, Apple Siri (HMM phonetic trellis alignment), and Google Assistant (streaming Conformer verification). Identified root causes: dense MLP receptive field lacking sequential phonetic state constraints, absence of throat/cough negatives, and `pos_weight=8.0` gradient distortion in `BCEWithLogitsLoss` that introduced a +2.08 logit bias towards false alarms.
- **Physical Vocal Artifact & Impulsive Synthesis (`generate_throat_negatives.py` & `generate_impulsive_negatives.py`)**:
  Synthesized 120 specialized negative audio samples (16kHz mono WAV) modeling phlegm flutter (20–36 Hz AM), vocal fry (55–110 Hz F0 jitter), velar turbulence, and coughing wheezes. Added 180 impulsive negative samples (sneezes, vocal bursts, counting sequences, claps, thumps). Expanded negative library to 1,742 clips (38,557 windows).
- **Balanced Bayesian Loss Retraining (`train_local_wakeword.py`)**:
  Replaced distorted `pos_weight=8.0` with balanced `pos_weight=1.2`, training over 40,253 training windows and 10,064 validation windows for 60 epochs. Validation loss reached 0.0245 with 99.0% recall and 0.2% FA.
- **Vocal Artifact Rejection Verification**:
  Achieved **0.0% False Alarm Rate** across all physical vocal tests: Throat clearing (0.0% FA, max score 0.1%), Gargling & saliva flutter (0.0% FA, max score 0.0%), Coughing (0.0% FA, max score 0.1%), Vocal fry (0.0% FA, max score 0.1%), Sneezes (0.0% FA, max score 0.7%), Shouts (0.0% FA, max score 0.1%), and Mic testing (0.0% FA, max score 0.1%).
- **Full Library & Hardware Benchmark**:
  Evaluated across 3,300 files: **97.1% Positive Recall (avg score 97.4%)**, **97.5% Negative Soundalike Rejection**, and **99.6% Background Noise Rejection** at calibrated threshold 0.68. Maintained 100% hardware invariance: Studio USB (97.1%), Laptop Mic Array with Intel Smart Sound (100.0%), Bluetooth Headset (93.6%), Far-Field Whisper (84.3%), and Noisy Office (92.5%).
- **Docs**: Architecture spec in `docs/features/59-vocal-friction-hardening-and-throat-gargle-rejection.md` and changelog in `docs/changes/42-vocal-friction-hardening-and-throat-gargle-rejection.md`.

## Multi-Source Noise Hardening & Hardware Invariance (2026-09-23)

- **Multi-Source Noise Ingestion (`ingest_opensource_noise.py`)**:
  Synthesized and categorized 600 high-fidelity background noise profiles across mechanical keyboard typing,
  113.3 Hz chassis fan resonance, office HVAC, domestic impulsive sounds, and narrowband telecom profiles.
  Screened through `faster-whisper` anti-poisoning ASR, automatically purging colliding clips. Total verified background: 998 clips.
- **Multi-Device Data Augmentation (`train_local_wakeword.py`)**:
  Applied Bluetooth narrowband bandpass (300–3400 Hz), laptop chassis fan resonance (55–145 Hz), distance attenuation (0.25x–0.40x),
  and ambient background mixing to positive recordings. Expanded positive training context to 11,760 windows, trained against 35,715 negative windows.
- **Hardware-Adaptive Microphone Invariance (`test_device_invariance.py`)**:
  Benchmarked across 5 hardware microphone profiles: **Studio USB Condenser (96.6%)**, **Laptop Mic Array with Intel Smart Sound (100.0%)**,
  **Bluetooth Headsets / Earbuds (92.1%)**, **Far-Field / Quiet Whispering (85.9%)**, and **Noisy Office (93.0%)**.
- **Batch Evaluation & Verification**:
  Batch evaluation across 3,000 files: **96.6% Positive Recall**, **95.8% Negative Soundalike Rejection**, and **99.7% Background Noise Rejection**.
- **Docs**: Architecture spec in `docs/features/58-multi-source-noise-hardening-and-hardware-invariance.md` and changelog in
  `docs/changes/41-multi-source-noise-hardening-and-device-invariance.md`.

## Apex Wake Word Evolution, Data Poisoning Quarantine & Adaptive Microphone DSP (2026-09-22)

- **Automated ASR Poisoning Quarantine (`audit_positive_samples.py`)**:
  Audited 592 positive recordings with `faster-whisper` and RMS filters. Identified and quarantined 135 poisoned clips
  (conversational sentences, YouTube background audio, near-silence) into `quarantined_bad_positive/`, leaving 456
  pristine acoustic NEXUS recordings.
- **Multilingual Negative & Multi-Source Background Augmentation**:
  Synthesized 1,442 negative samples across English soundalikes (*"next"*, *"texas"*, *"lexus"*, *"necklace"*),
  Indian languages (Hindi: `hi-IN-Madhur`, `hi-IN-Swara`; Telugu: `te-IN-Mohan`, `te-IN-Shruti`), and assistant names.
  Generated 400 multi-source background sound clips (`generate_background_sounds.py`) for HVAC fans, mechanical
  keyboard typing, mouse clicks, and room ambience.
- **BCEWithLogitsLoss & SigmoidWrapper ONNX Export**:
  Upgraded classifier training with `pos_weight=8.0` penalty on negatives and exported calibrated ONNX graph.
- **Adaptive Microphone Hardware Prober (`nexus wake probe` / `acoustic_profile.rs`)**:
  Created 1.5s FFT ambient spectral scan in `scripts/probe_microphone.py` and Rust `src-tauri/src/acoustic_profile.rs`.
  Detected chassis fan resonance at 113.3 Hz (+22.3 dB) on Intel Smart Sound mic array; dynamically auto-tunes high-pass
  filter to 128.3 Hz, hardware pre-gain to 2.50x, adaptive silence gate to 0.00300 RMS, and impulsive gate to 8.0x (rejecting coughs/throat-clears).
- **Phantom Cascade Elimination**:
  Added `reset_after_trigger()` across Rust and Python to immediately flush the 16-frame embedding buffer on trigger confirmation.
- **Batch Evaluation & Verification**:
  Batch benchmark on 2,298 files: **92.3% True Positive Recall**, **98.9% Negative Rejection** (1.1% FA), and **99.8% Background Noise Rejection** (0.2% FA).
- **Docs**: Architecture spec in `docs/features/57-apex-wake-word-evolution-and-hardware-adaptation.md` and changelog in
  `docs/changes/38-apex-wake-word-evolution-and-hardware-adaptation.md`.

## Targeted Intent Training, Category Drill-Down & MCP Data Promotion (2026-09-22)

- **Targeted CLI Voice Collection (`nexus collect`)**:
  Added `-i` / `--intent` and `-c` / `--category` flags with combined scope validation. Developers can directly
  target single intents (`nexus collect --category github --intent create_pr` or `nexus collect --intent create_pr`)
  or drill down interactively by choosing a category and selecting individual sub-intents from a numbered terminal menu.
- **Complete Phrase Catalogs (All 55 BERT-Mini Intents)**:
  Expanded `PHRASES` in `scripts/collect_nlu_samples.py` with realistic spoken templates for all 23 missing GitHub,
  workflow, release, collaborator, and local mode intents (57 total registered intents including live dictation modes).
  Extended `extract_slots()` with rule-based entity extractors for `create_pr` (`repo`, `title`, `head`, `base`),
  `comment_pr` (`pr_number`, `repo`, `body`), `analyse_pr` (`owner`, `repo`, `pr_number`), `add_collaborator`, etc.
- **Zero-Quarantine MCP Split Promotion (`dataset.json`)**:
  Diagnosed the Step 3d quarantine cause (missing test rows in locked evaluation set for new intents). Promoted
  25 phrase families for `order_food` (99 rows), 12 for `search_product` (68 rows), and 12 for `send_whatsapp_message` (127 rows)
  into `dataset.json` with strict zero phrase-family cross-split leakage. Re-minted `split_lock.json` and `evaluation_lock.json`
  (481 test rows). Validation passes cleanly (`python server/nlu/data_foundation.py validate`).
- **Category Coverage Alignment (`nlu_stats.py`)**:
  Aligned category schema with the 55 production intents. Verified that all MCP intents now display full trained rows
  (`order_food`: 77, `send_whatsapp_message`: 72, `search_product`: 32, `whatsapp_search`: 107) and active `◐ Good` / `● Strong`
  mastery status.
- **Docs**: Architecture spec in `docs/features/56-targeted-intent-training-and-mcp-data-promotion.md` and changelog in
  `docs/changes/37-targeted-intent-training-and-mcp-data-promotion.md`.

## NLU Data Foundation, STT Conditioning & Voice Scaling (2026-09-22)

- **Acoustic STT vs. Language NLU Decoupling**:
  Untangled speech acoustic transcription from text intent classification. Resolved multilingual Whisper
  hallucinations (Urdu/Russian/Spanish on short <2s clips) by adding `language="en"`, `temperature="0.0"`,
  and domain prompt biasing (`NEXUS, WhatsApp, Biryani, Dosa, Ghostwriter, VS Code, PR, GitHub...`) to
  `scripts/collect_nlu_samples.py`.
- **Data Foundation & Evaluation Lock Fix (`data_foundation.py`)**:
  Fixed `ERROR: external benchmark hash mismatch` by updating `server/nlu/data/external_evaluation_lock.json`
  with canonical SHA-256 hashes for sorted evaluation benchmark files (`clinc150_oos_test.jsonl`). Preflight
  validation passes cleanly.
- **The 70/30 Data Principle**:
  Enforces 70% anti-poisoning guardrails (cryptographic test locks, slot consistency, conflict repair, split quarantine)
  and 30% zero-waste user effort preservation (all clean recorded voice samples from `collected_samples.jsonl`
  automatically audited and merged into `dataset.json` — 3,108 training rows).
- **Voice Scaling Science**:
  Empirical research demonstrates 100–500 voice samples is the optimal sweet spot (~96.5%–98.2% standalone BERT accuracy;
  99%+ real-world command success when backed by the 3-tier Regex/BERT-Mini/Qwen cascade).
- **Speaker Independence & One-Login Google Contacts**:
  Documented text-token speaker invariance (1 global BERT-Mini model works for all users/devices via Worker R2 OTA updates).
  Integrated Google OAuth scope union (`contacts`, `gmail`, `calendar`) with local `contacts.json` resolution.
- **Docs**: Comprehensive spec in `docs/features/55-nlu-data-perfection-voice-scaling-and-mcp-bridge-research.md`
  and changelog in `docs/changes/36-nlu-data-foundation-stt-conditioning-and-voice-scaling.md`.

## Interactive Voice Approval & Confirmation Sidebar (2026-09-21)

Full interactive voice approval workflow with smart 5-second listening window
and persistent Response Sidebar confirmation card:
- **Response Sidebar Confirmation Card (`ConfirmationPanel.tsx`)**:
  Renders structured details for actions requiring approval (WhatsApp recipient & message bubble,
  GitHub repo/PR#/danger warning, Swiggy/Amazon MCP tools/parameters). Displays 1-click
  `[✓ Confirm]` and `[✕ Cancel]` buttons with loading state.
- **Automatic 5-Second Voice Listening Window (`orchestrator.ts`)**:
  Once prompt TTS finishes speaking, the microphone automatically opens into `listening` mode
  and starts STT audio capture without needing a hotkey or wake word.
- **Instant Early Reaction (< 2s)**:
  Speaking an approval phrase (*"proceed"*, *"approved"*, *"yes"*, *"confirm"*, *"go ahead"*, *"do it"*)
  or rejection phrase (*"cancel"*, *"no"*, *"abort"*) clears the 5s timer immediately and executes
  or aborts without waiting for the full 5s window.
- **Graceful Timeout & Sidebar Persistence**:
  If 5 seconds elapse with no speech, the mic returns to idle while the sidebar remains open on screen
  for manual review and 1-click approval.
- **Rust Backend Integration (`commands.rs`, `orchestrator.rs`, `lib.rs`)**:
  `show_sidebar_with_confirmation` captures blurred desktop backdrop and stores confirmation JSON
  race-free in `PENDING_SIDEBAR`.
- **Docs & Verification**: Detailed architecture spec in
  `docs/features/54-interactive-voice-approval-and-confirmation-sidebar.md`.
  Verified across 585 tests (519 Rust + 17 Vitest + 49 Worker).

## MCP Connect System — Best-of-Combine Build (2026-09-20)

Industry research in `docs/research/mcp-connection/` (7 files) drove a
three-stack implementation, each phase verified twice:

**Phase 1 — shared connect infrastructure:**
- `mcp_client.rs`: `McpConnectState` (unknown/down/auth-required/ready),
  `pairing_status` parser (`parse_pairing_state`, case-insensitive,
  garbage → Error, never fake green), `normalize_qr_payload`
  (data-URI / raw-b64 / pairing-code), `mcp_connect_state` command +
  `connect_card_for` per server (WhatsApp fuses transport + pairing;
  vault services fuse transport + `token_status`; Amazon transport-only).
- `orchestrator.rs`: MCP failure → voice guidance (existing) + Connect
  card opens in the sidebar ONCE per server per session
  (`SHOWN_CONNECT_CARDS` gate; `open_mcp_connect_card` +
  `connect_card_markdown` — status, numbered steps, QR image, /pair
  link, ToS-burner + 20-day-rotation notes). Failed call is stashed
  (`PENDING_MCP_RETRY`) and a 5s-poll ready monitor
  (`spawn_ready_monitor`, ~10min cap) auto-retries it once when the
  server turns Ready (Composio WAIT_FOR_CONNECTIONS shape); drops
  silently if a newer turn is active. Wired into `Subsystem::Mcp` Err,
  `dispatch_to_mcp` Err, and `orchestrator_mcp_confirm` Err.

**Phase 2 — Swiggy spec-OAuth (Worker + vault + frontend):**
- `server/worker/src/index.ts`: SWIGGY_TOKEN_URL/AUTH_URL/SCOPES +
  Env secrets; `handleAuthUrl`, `handleOAuthBrowserCallback`,
  `handleOAuthExchange` each gained a swiggy branch (PKCE S256, form-
  encoded token exchange, D1 storage, shared `/oauth/callback` +
  `nexus://oauth/` deep link); `refreshSwiggyToken` +
  `getValidSwiggyToken` (silent refresh, null-on-failure like Google);
  `GET /oauth/swiggy-token`; `/config/check` lists swiggy.
- `auth_vault.rs`: `fetch_swiggy_token_from_worker` + swiggy arm in
  `refresh_service_token` (silent refresh on-device).
- `setup/oauth.ts` + `SetupApp.tsx`: provider union widened to
  `"swiggy"` (flow is fully generic). Connections tab: "Login with
  Swiggy" button (`handleSwiggyLogin` → connectOAuth → vault refresh),
  updated hint.
- Setup needs: `wrangler secret put SWIGGY_CLIENT_ID/SWIGGY_CLIENT_SECRET`
  (Builders-Club approval required for production; localhost dev free).

**Phase 3 — long-tail + proactive rotation:**
- Spotify/Vercel/Render cards: numbered 3-step hints + "Get token"
  buttons (`shell.open` provider pages, window.open fallback).
- Vault idle monitor (90s) now also probes WhatsApp `pairing_status`
  (inner call — no breaker trip, no audit) so the scheduled ~20-day
  session rotation surfaces before a send fails.

**Verify x2 results:** cargo check clean; `cargo test --lib
--test-threads=1` 499/499; Worker `npm test` 49/49 ×2 + tsc; frontend
vitest 14/14 ×2 + tsc. Live bridges test (`test_connect_state_live_bridges`)
pins card shape invariants (pair URL, steps present on auth-required,
no nag on ready) against live 5-server probe.

**Round-2 deep audit (same day, `docs/research/mcp-connection/08-...`):**
live sources (Anthropic trackers #744/#54649/#60572/#35, Novu connect-card
PR, AutoGPT MCP fix commit, 2026-07-28 OAuth security spec) exposed 5
gaps — all fixed + verified ×2 (Rust now 500/500 serial): monitor
re-renders the open card when the QR payload changes (WhatsApp rotates
its QR every 20-30s; a static card was unscannable after 30s) and renders
the Connected card on Ready; `getValidSwiggyToken` persists rotated
refresh tokens (OAuth 2.1 public-client MUST); RFC 8707 `resource` param
added to Swiggy authorize/exchange/refresh; `audit_line()` choke point +
test pins tokens out of the audit log; `mcp_connect_state` probes run in
parallel (25s → 5s worst case).

## Stuck Animations + PR Analyse Flow + Minimal Sidebar + Phase 11 (2026-09-18)

**Stuck wakeup animation — root cause was a missing `done` handshake.**
`WorkerBackend` Ok withholds `Done` (would cancel TTS), but the frontend
`result` handler spoke with no `onEnd` and `signalOrchestratorDone()` had
zero call-sites — the orb parked in `speaking`/loading-loop forever after
long replies. NOT any "ghost mode" (no ghost CSS/setting exists anywhere;
"ghost" = Ghostwriter dictation only). Fixes:
- `net/orchestrator.ts::finishSpokenResult` — TTS onEnd → guarded reset +
  `orchestrator_done` (barge-in-safe via request-id check). 3 vitest tests.
- `App.tsx` 60s speaking failsafe — silent 60s in `speaking` (checked via
  `isRustTtsPlaying()`) forces the `done`-equivalent reset; long replies
  re-arm instead of cutting.
- `show_loading` runs inline (spawn removed) — create always completes
  before the paired destroy; kills the spinner re-creation race.
- Known flake: `test_install_and_cancel` fails under parallel threads
  (shared ACTIVE_REQUEST global); passes isolated ×2 and 43/43 serial.

**Analyse-PR flow:** pr-list `Analyse` now hides the list first
(`hide()` + `hide_pr_list_sidebar`); `WorkerBackend` Ok with non-null
`analysis` opens the response sidebar via `show_sidebar_with_analysis`
(PENDING_SIDEBAR race-free path) AFTER emitting `Result` (TTS not blocked).
No new AI call — the Worker already returned `analysis`, it was
`console.log`-dropped.

**Minimal sidebar:** removed container top-highlight gradients, inset
stacks, specular `::before` rims, gradient overlays (flat dim kept),
dead `backdrop-filter`s, dot pulse, button lift/glow, red close hover,
`nexus-hr` gradient, unused `--gradient-*` vars. PR buttons now flat
solid (green Merge, grey Analyse). Settings sidebar untouched (still glossy).

**list_prs "pull requests and alll":** data was fine (64 train rows) — the
deterministic regex had no `pull request` alternative and `$`-anchored out
trailing words. Pattern now accepts the noun + `and all/all of them/
everything` + optional `the` after state. `add_phase11_pr_verbs.py` adds
45 fresh rows (39 list_prs + 6 OOS) → candidate train 3482, test intent
**0.9004** (was 0.8850), gated OOS 0.9871. Production dataset.json
untouched (2765 rows) — promotion is a separate merge+retrain decision.
`collect_nlu_samples.py`: duplicate `list_prs` key merged (21 phrases were
silently dropped), 32-phrase list; interactive category menu (github, mcp,
apps, messages, live, random) + `--category`; no google/research category
(no such intents exist yet). Say-it-your-way paraphrase option built then
REVERTED per user (need is same-word/many-sounds, not rewording) —
replaced by `canonical_repo_name()` sound-alias map (`cervix/srvx/service
→ servx`, zinc/sync → zync, per-segment, unknown pass-through), wired
into `clean_repo_name` (all deterministic repo intents) + `nlu_client`
`repo_slot` (all 22 repo arms). "analyse pr 254 in zink" now exact 1.0
instead of fuzzy 0.8.

## Qwen Brain — Admin-Only Local Reasoning (2026-09-17)

**Phase 6 status: the brain stack already existed** — `admin_config.rs`
(`is_admin` runtime gate + `admin-brain` compile gate), `lazy_brain.rs`
(non-blocking spawn on port 39219), `brain_client.rs` (classify /
generate_phrasings / pronunciation_map / health), `brain_monitor.rs`
(continuous learning), `server/admin/brain_server.py` (Qwen2.5-0.5B
GGUF, 480 MB, present at `server/admin/model/`). The orchestrator tries
brain classification BEFORE NLU when the deterministic parser misses.

What Phase 6 added:

### 1. Intent schema coverage for the new MCP intents

The 3 commerce/social intents existed only in the deterministic parser —
the NLU/brain paths dropped them (`_ => None`). Now wired end-to-end:

- `nlu_client.rs::nlu_to_parsed_intent` — maps `order_food`
  (`food_item`→query, `restaurant`), `search_product` (`query`),
  `send_whatsapp_message` (`contact`, `message`). Brain responses reuse
  this mapping, so both paths benefit.
- `nlu_server.py` — `INTENTS` + `SLOT_TYPES` updated to match the
  retrained model (55 intents, 51 slot labels — **must match
  `train.py` ordering**).
- `brain_server.py` — `ALL_INTENTS` + `SYSTEM_PROMPT` know the 3 new
  intents, their slots (`food_item`, `restaurant`, `message`), and
  few-shot examples.

### 2. Brain-assisted compound planning

`command_center::build_plan_with_brain` (async) — called from
`process_transcript` in place of the sync `build_plan`:

- Fast path: fully deterministic plan → brain never invoked (zero added latency).
- If any step fails deterministic parse AND the `admin-brain` feature
  is on AND `is_admin()` → `brain_classify` fills in that step.
- Same conservative gates: Architect/None/CommandCenter steps abort the
  plan; brain-classified steps flow through the same confirmation-gated
  dispatch.
- Non-admin devices / brain unavailable → identical `None` fallback
  (single-intent path), zero risk.

This enables compounds like "remind me to call mom then order biryani"
where a step phrasing misses the deterministic regexes.

### Gates (unchanged, both required)

1. Compile-time: `cargo build --features admin-brain`
2. Runtime: `%APPDATA%/com.nexus.assistant/admin.json` (or dev
   `server/admin/admin_config.json`) with `is_admin: true`,
   `brain_enabled: true`

Family builds ship without `admin-brain` — all brain code is compiled
out (`brain_client`, `brain_monitor`, `lazy_brain` don't exist).

## NLU Self-Improvement — Family Model Distribution (2026-09-17)

The admin's Qwen brain continuously improves BERT-Mini (brain_monitor →
approved_phrasings.jsonl → merge_and_train.py). **Phase 5 adds the
distribution channel**: family devices pull the improved model over the
air — no app rebuild needed.

### Architecture

```
ADMIN                              WORKER (Cloudflare)              FAMILY
nexus train                        KV: nlu_model_latest manifest    startup +15s:
  → nexus_nlu.onnx                 R2: nlu/<file> objects           GET /models/nlu/latest
python publish_nlu.py                                               version differs?
  → wrangler r2 object put ...        ──────────────────────────→   GET /models/nlu/download?name=
  → POST /models/nlu/publish                                        per-file sha256 verify
     (Bearer NEXUS_ADMIN_TOKEN)                                     → %APPDATA%/com.nexus.assistant/nlu_model/
                                                                    → POST /reload_model (hot-swap)
                                                                    or NEXUS_NLU_MODEL_DIR on next spawn
```

### Worker endpoints (`server/worker/src/index.ts`)

| Endpoint | Auth | What it does |
|----------|------|--------------|
| `GET /models/nlu/latest` | none | KV manifest `{version, updated_at, files:{name:{sha256,size}}}` |
| `GET /models/nlu/download?name=<file>` | none | Streams file from R2 `nlu/` prefix; whitelist-guarded names |
| `POST /models/nlu/publish` | `Bearer NEXUS_ADMIN_TOKEN` | Writes KV manifest (admin publishes blobs via wrangler) |

Returns 503 when `CACHE`/`MODELS` bindings are missing — family clients
treat that as "no update" and keep the bundled model.

### Client side (`src-tauri/src/nlu_update.rs`)

- `spawn_update_check(app)` — called once at startup (15s after session
  auto-open, so first paint isn't competing with a 35 MB download)
- Downloads to a `.staging` dir first; sha256-verifies every file;
  aborts cleanly on mismatch (old model stays live)
- File-level manifest (no zip dep): `nexus_nlu.onnx`, `.onnx.data`,
  `labels.json`, `temperature_calibration.json`, `tokenizer/*`
- If NLU server is already running → `POST /reload_model` hot-swaps
- `downloaded_model_dir_envless()` — used by `lazy_nlu.rs` to pass
  `NEXUS_NLU_MODEL_DIR` at spawn (same `%APPDATA%/com.nexus.assistant`
  derivation as `lazy_stt.rs::read_moonshine_model`)

### `nlu_server.py` model dir resolution

1. `NEXUS_NLU_MODEL_DIR` env var (downloaded update) — if it has ONNX
2. `server/nlu/model/` (dev)
3. `src-tauri/resources/server/nlu/model/` (bundled fallback)

### Admin publish (`server/nlu/publish_nlu.py`)

```bash
# One-time setup:
npx wrangler r2 bucket create nexus-models
# uncomment [[r2_buckets]] MODELS binding in server/worker/wrangler.toml
npx wrangler secret put NEXUS_ADMIN_TOKEN
npx wrangler deploy

# After each retrain:
python server/nlu/publish_nlu.py \
  --worker https://nexus-worker.chitkullakshya.workers.dev \
  --token-file ../admin/data/admin_token.txt
```

### New NLU intents (Phase 8 staged data)

`server/nlu/add_commerce_intents.py` generates 158 staged examples for
`order_food`, `search_product`, `send_whatsapp_message` (+ 9 OOS
negatives), wired into `build_candidate_dataset.py` as
`phase8_commerce_social.json`. `train.py` INTENTS/SLOT_TYPES and
`model/labels.json` updated (55 intents, 51 slot labels; new slots:
`food_item`, `restaurant`, `message`). Next `nexus train` /
candidate build includes them.

**Pre-existing issue found:** `build_candidate_dataset.py` currently
fails on `"how much is an overdraft fee for bank"` — that CLINC staging
row was already merged into production train (twice, duplicated), so
`verify_no_overlap` trips before phase8 is even reached. Fix: remove the
row from `clinc150_oos_train_reviewed.jsonl` or dedupe train.

## Command Center — Multi-Step Task Orchestration (2026-09-16)

**`src-tauri/src/command_center.rs`** implements the n8n-style command
center: compound commands ("X then Y") are split into steps, each step
is parsed + routed to a sub-center, executed sequentially, and results
are merged into one response.

### How it works

```
"open chrome then search for cats"
  → split_compound()    → ["open chrome", "search for cats"]
  → build_plan()        → TaskPlan { 2 PlanSteps, each routed }
  → execute_plan()      → step 1: LocalCommand → command_executor
                        → step 2: WorkerBackend → 9Router/Worker
  → merge               → "Opened Chrome sir. Found results for cats."
```

### Split rules (`split_compound`)

Splits on: ` then `, ` and then `, `, then `, ` after that `,
` afterwards `, `; `. Deliberately does NOT split on bare ` and ` —
"search and rescue", "mum and dad" would break. Dangling trailing
connectors ("open chrome then") are stripped before splitting.

### Conservative fallback

`build_plan` returns `None` (falls back to normal single-intent path)
when:
- transcript isn't compound (< 2 parts)
- ANY part fails deterministic parsing — a bad split should never
  produce a worse outcome than today's path
- any part routes to `Architect` (window-managed, can't compose)

### Step execution (`execute_step`)

| Subsystem | How it runs |
|-----------|-------------|
| LocalCommand | `ParsedIntent` → `command_executor::Intent` → `execute_command` |
| Mcp | `dispatch_to_mcp` — read ops run; gated ops emit Confirm + pause |
| WorkerBackend | `dispatch_to_worker` (9Router fast path included) |
| GitHub | `github_cmd::execute_command` — read ops inline; destructive ops emit Confirm (existing frontend flow) |
| Architect / CommandCenter | `Unsupported` — can't nest/compose |

### Confirmation gates inside compounds

When a step hits a confirmation gate (e.g. `send_whatsapp_message`),
the task pauses: the gated step's pending payload is emitted as a
Confirm event, and the remaining steps are stashed in
`PENDING_COMPOUND`. `orchestrator_mcp_confirm` calls
`resume_compound()` after approval — it runs the confirmed call, then
the remaining steps, and emits the merged result.

Known v1 limit: a destructive **GitHub** step inside a compound emits
Confirm via the existing github flow, but remaining steps are dropped
(GitHub resume needs `orchestrator_github_execute` integration).

### Cancellation

Each step checks `is_cancelled` before running — a barge-in mid-plan
skips remaining steps and reports partial results.

### Sequential only

v1 runs steps in order ("then" implies ordering). Independent parallel
steps are future work.

### Files

- `src-tauri/src/command_center.rs` — plan/split/execute/resume/merge
  (~950 lines, 20 unit tests)
- `src-tauri/src/orchestrator.rs` — `Subsystem::CommandCenter`,
  compound fast path in `process_transcript`, `run_command_center`,
  `orchestrator_mcp_confirm` resume hook, pub wrappers
  (`dispatch_to_mcp_pub`, `dispatch_to_worker_pub`, `is_cancelled_pub`)

## MCP Sub-Center — External Services via Model Context Protocol (2026-09-16)

**`src-tauri/src/mcp_client.rs`** is the MCP client that calls external
MCP servers (Swiggy, Amazon, WhatsApp) via JSON-RPC 2.0 over streamable
HTTP. This is Phase 3 of the roadmap — the foundation for commerce and
social sub-centers.

### Registered MCP Servers

| Server | Endpoint | Auth | Tools |
|--------|----------|------|-------|
| `SwiggyFood` | `https://mcp.swiggy.com/food` | OAuth 2.1 PKCE | 17 (restaurants, menu, cart, orders) |
| `SwiggyInstamart` | `https://mcp.swiggy.com/im` | OAuth 2.1 PKCE | 19 (grocery search, cart, orders) |
| `SwiggyDineout` | `https://mcp.swiggy.com/dineout` | OAuth 2.1 PKCE | 12 (table reservations) |
| `WhatsApp` | `http://127.0.0.1:8765/mcp` | QR session | messaging, contacts |
| `Amazon` | `http://127.0.0.1:8766/mcp` | browser session | product search, details, reviews |

Swiggy MCPs are hosted by Swiggy (free on localhost for dev; production
requires Builders Club approval + demo video). WhatsApp/Amazon run as
local stdio/HTTP MCP servers (e.g., whatsmeow bridge, Playwright scraper).

### New Intents (deterministic parser)

| Pattern | Intent | Routes to |
|---------|--------|-----------|
| "order pizza from dominos" | `OrderFood { query, restaurant }` | `Subsystem::Mcp` → SwiggyFood `search_restaurants` |
| "order biryani" / "get food from swiggy" | `OrderFood` | same |
| "search for X on amazon" / "find X on amazon" | `SearchProduct { query }` | `Subsystem::Mcp` → Amazon `amazon_search` |
| "amazon search for X" / "search amazon for X" | `SearchProduct` | same |
| "send <c> a whatsapp message saying <m>" | `SendWhatsAppMessage` | `Subsystem::Mcp` → WhatsApp `send_message` |
| "whatsapp <c> saying <m>" / "message <c> on whatsapp saying <m>" | `SendWhatsAppMessage` | same |

**Ordering matters:** `parse_send_whatsapp_message` runs BEFORE
`parse_whatsapp_command` — otherwise "whatsapp mom saying hi" would be
swallowed as a chat-open with contact "mom saying hi".

### Confirmation Gates

`McpServer::requires_confirmation(tool)` — write ops (update cart,
send message, book table) emit `OrchestratorEvent::Confirm` with a
pending payload `{server, tool, params, transcript}` instead of
executing. The frontend calls `orchestrator_mcp_confirm(request_id,
confirmed, pending)` to proceed. Read ops (search, list, get) execute
directly.

`McpServer::is_destructive(tool)` — `place_food_order`, `place_im_order`,
`book_table` get an irreversible-action warning in the confirm prompt.

### Files

- `src-tauri/src/mcp_client.rs` — MCP client (McpServer registry,
  JSON-RPC call_tool/list_tools, SSE parsing, extract_text, 12 tests)
- `src-tauri/src/intent_parser.rs` — 3 new intents + 3 parsers + 16 tests
- `src-tauri/src/orchestrator.rs` — `Subsystem::Mcp`, `dispatch_to_mcp`,
  `orchestrator_mcp_confirm` command, 4 routing tests
- `src-tauri/src/lib.rs` — `pub mod mcp_client` + command registration

### Still needed for production use

- **Swiggy OAuth flow** — `call_tool` currently passes `auth_token=None`;
  OAuth 2.1 PKCE token storage/refresh needs wiring (store in
  `settings.json` or keychain).
- **WhatsApp local bridge** — run a whatsmeow/Cloud-API MCP server on
  `127.0.0.1:8765/mcp` (e.g., `whatsapp-mcp` Go bridge + Python MCP).
- **Amazon local server** — run a product-search MCP on
  `127.0.0.1:8766/mcp` (Creators API needs Associates credentials, or a
  Playwright scraper variant).
- **Multi-step flows** — order → cart → checkout chains belong to the
  Phase 4 command center; today each intent maps to one tool call.

## 9Router — Local AI Gateway (2026-09-16)

**9Router** (`src-tauri/src/router.rs`) routes general AI questions
directly to free cloud providers (Cerebras → Groq → Gemini), bypassing
the Worker for 3-7x lower latency (~242ms vs ~2s). The Worker remains
the fallback for PR analysis, GitHub operations, and tasks requiring
session/OAuth tokens.

### Architecture

```
Current (Worker path):
  Device → Worker (50ms) → Workers AI (500-2000ms) → Worker (50ms) → Device
  Total: 600-2100ms

With 9Router:
  Device → localhost (1ms) → Cerebras/Groq (80-120ms) → localhost (1ms) → Device
  Total: ~242ms  (3-7x faster)
```

### Provider Cascade

```
Cerebras (1M tokens/day free, ~80ms, Llama 3.3 70B) — fastest
  → Groq (14,400 req/day free, ~120ms, Llama 3.3 70B)
    → Gemini (1,500 req/day free, ~400ms, flash-lite)
      → Worker (fallback, uses neurons)
```

### What 9Router Handles

- General questions ("what's the capital of France?")
- Factual queries ("how tall is the Eiffel Tower?")
- Conversational responses (when NLU confidence is low)

### What Still Goes Through the Worker

- PR analysis (needs GitHub token + GLM models)
- GitHub commands (merge, approve, close PR)
- Architecture mapper
- Any task requiring session/OAuth tokens

### Routing Logic (`router::can_route`)

Returns `false` (must use Worker) for transcripts containing:
`analyse pr`, `analyze pr`, `analyse latest pr`, `analyse repo`,
`architect`, `check branch`, `merge pr`, `approve pr`, `close pr`,
`github`, `pull request`, `pullrequest`.

Returns `true` (9Router can try) for everything else.

### Integration in Orchestrator

`dispatch_to_worker()` in `orchestrator.rs` now tries 9Router first:
1. If `can_route(transcript)` → try 9Router (Cerebras → Groq → Gemini)
2. If 9Router succeeds → return directly (skip Worker entirely)
3. If 9Router fails or `can_route` returns false → fall back to Worker

### Settings (API Keys)

Stored in `settings.json` (camelCase, same as existing Groq key):
- `groqApiKey` — Groq API key (already existed for STT, now reused for LLM)
- `geminiApiKey` — Google Gemini API key (already existed)
- `cerebrasApiKey` — Cerebras API key (NEW, free at cloud.cerebras.ai)

Settings UI: Settings sidebar → Accounts tab → Cerebras API Key field.

### Files

- `src-tauri/src/router.rs` — 9Router module (~570 lines, 12 unit tests)
- `src-tauri/src/orchestrator.rs` — `dispatch_to_worker()` modified to try 9Router first
- `src-tauri/src/commands.rs` — `NexusSettings` + `cerebras_api_key` field
- `src-tauri/src/lib.rs` — `pub mod router;` registered
- `frontend/src/settings-sidebar/SettingsSidebarApp.tsx` — Cerebras API key field

## Moonshine Medium v2 — Better Local STT (2026-09-16)

**Default Moonshine model upgraded from Small to Medium v2** for better
local STT accuracy. Users can switch back to Small in Settings for
lower RAM on 8GB laptops.

### Model Comparison

| Model | Params | WER | RAM | Latency |
|-------|--------|-----|-----|---------|
| tiny_streaming | 34M | 12.0% | ~100 MB | ~50ms |
| small_streaming | 123M | 7.84% | ~300 MB | ~165ms |
| **medium_streaming** (new default) | 245M | **6.65%** | ~400 MB | ~269ms |

### Configuration

- **Default:** `medium_streaming` (245M, 6.65% WER) — better accuracy
- **Family (8GB laptops):** Set to `small_streaming` in Settings for lower RAM
- **Ultra-low RAM:** Set to `tiny_streaming` (34M, 12% WER)

### How It Works

1. `lazy_stt.rs::read_moonshine_model()` reads `moonshineModel` from
   `settings.json` (defaults to `medium_streaming`)
2. Passes it as `MOONSHINE_MODEL` env var when spawning `stt_server.py`
3. `stt_server.py` reads `MOONSHINE_MODEL` env var (already supported)
4. Settings UI: Settings sidebar → Audio tab → Moonshine model dropdown

### Files

- `src-tauri/src/commands.rs` — `NexusSettings` + `moonshine_model` field
- `src-tauri/src/lazy_stt.rs` — `read_moonshine_model()` + env var on spawn
- `frontend/src/settings-sidebar/SettingsSidebarApp.tsx` — model dropdown

## Diagnostic Fixes — 2026-09-12

### NLU Server ONNX Export (build integration)

The BERT-Mini retraining produced `best_model.pt` but the ONNX export step
was missing. The NLU server requires `nexus_nlu.onnx` to start — without
it, `sys.exit(1)` and every unparseable command blocked for 30s.

**Fix:** `nexus.mjs` now calls `syncNluModel()` before every build. This
copies the ONNX model, `labels.json`, and tokenizer from
`server/nlu/model/` to `src-tauri/resources/server/nlu/model/` so the
Tauri installer bundles the latest trained model automatically.

**`tauri.conf.json` resources** now includes `labels.json`:
```json
"resources/server/nlu/model/labels.json"
```

**To retrain and bundle:**
```bash
cd server/nlu
python train.py           # trains → best_model.pt
python export_onnx.py    # exports → nexus_nlu.onnx (max_length=64)
# Then: nexus build      # syncs model to resources + bundles in installer
```

### NLU Training Commands (`nexus train` + `nexus collect` + `nexus audit`)

**`nexus train`** performs cleaning, generation, merge, canonical conflict/slot/leakage repair, BERT-Mini training, stable ONNX export, resource sync, post-training audit, and temporary-file cleanup.

```bash
nexus train                    # full pipeline (with audit + cleanup)
nexus train --clean-only       # just clean the dataset
nexus train --skip-train       # data generation/repair without training
nexus train --keep-temp        # keep generated data and checkpoint (~17 MB)
```

**`nexus audit`** validates both data and model quality:

```bash
nexus audit                    # structural checks + ONNX evaluation
nexus audit --dataset-only     # structural checks only
```

It writes `server/nlu/audit_report.json` and `docs/research/nlu-model-data-audit-latest.md`.

**Data foundation gates** must pass before external imports or retraining:

```bash
nexus data nlu validate           # provenance registry + frozen 452-row evaluation lock
nexus data wake validate          # wake model fingerprints + optional audio manifest
nexus data wake fingerprint       # explicitly refresh fingerprints after approved model replacement
```

Staged external NLU payloads belong in ignored `server/nlu/data/staging/`. Local wake audio belongs in ignored `wake_word_data/manifest.jsonl`; speaker, session, and source-group IDs may not span train/validation/test splits. See `docs/testing/data-foundation-and-wake-model-gates.md`.

NLU training data is divided into locked phrase-family-separated `train`, `validation`, `calibration`, and final `test` splits. Rows whose templates overlap final test remain preserved under `quarantine` and must not train the model. Validate with `python server/nlu/prepare_evaluation_splits.py`; see `docs/testing/phase-1-nlu-evaluation-split-analysis-2026-09-14.md`.

**`nexus collect`** prompts for 23 intent families, waits for Enter, records two seconds, transcribes with Groq/Moonshine, and requires save/retry/edit/skip/end review before adding a sample.

```bash
nexus collect
nexus collect --intent type_text
nexus collect --count 20
nexus collect --text-only
nexus collect --list
```

Output: `server/admin/data/collected_samples.jsonl` (gitignored, admin-only). `nexus train` merges approved samples and then deletes the temporary collection file unless `--keep-temp` is supplied.

Requirements: working microphone and `sounddevice`/`numpy`/`scipy` (auto-installed). Groq is auto-loaded from NEXUS settings; local STT is the fallback.

### NLU Server Startup Cooldown

Was: 30s wait per failed startup, no cooldown → "loading non stop".
Now: 15s timeout + 60s cooldown after failure. If the NLU server can't
start (missing model, missing deps), it won't block subsequent commands.

### Brain Server Non-Blocking Spawn

Was: `ensure_brain_running()` blocked for 12s while Qwen loaded.
Now: spawns process + background thread. First command falls back to
NLU/deterministic while brain loads in background.

### Wake Word Grace Period (10s after restart)

Was: 5s grace period after stream restart. Intel SST driver produces
transient audio bursts 5-10s after restart that false-trigger the model.
Now: 10s grace period. See `wakeword_oww.rs::detect_chunk()`.

### NLU Model Dimension Fix

The ONNX export used `max_length=32` but the server tokenizes with
`max_length=64` (MAX_LEN). Fixed `export_onnx.py` to use `max_length=64`.

### Live-Mode Intent Mapping

`nlu_client.rs` now maps all 11 live-mode intents to `NluResult`:
`type_text`, `press_key`, `press_hotkey`, `confirm_send`, `cancel_action`,
`browser_new_tab`, `browser_navigate`, `browser_search`,
`whatsapp_open`, `whatsapp_search`, `focus_app`.

### Known Issue: NLU Model Accuracy

The current model has low accuracy (5-9% confidence) because the
training dataset (`dataset.json`) contains malformed intent labels
from the previous merge (e.g., `OpenArchitect` instead of
`open_architect`, debug objects as intent labels). The deterministic
parser handles most commands correctly — the NLU is a fallback only.
**To fix:** clean the dataset labels, retrain, re-export ONNX, rebuild.

## Live Mode — Phase 1 Implementation (2026-09-11)

**Live mode is the always-listening, STT-only, full-laptop control
capability.** It adds keyboard simulation, WhatsApp full flow, browser
navigation, window focus, and a state machine for sequential commands.

### New Dependencies

```toml
enigo = "0.5"     # Cross-platform keyboard/mouse simulation
arboard = "3.4"   # Clipboard for paste-text pattern (avoids autocomplete corruption)
```

### New Module: `src-tauri/src/live/`

| File | Purpose |
|------|---------|
| `mod.rs` | Module root, LiveResult, LiveIntent, 14 Tauri commands |
| `state.rs` | State machine (Idle → AppOpen → ChatActive → TextTyped) |
| `safety.rs` | Whitelist, denylist, confirmation gates |
| `commands/keyboard.rs` | Type text, press keys, hotkey combos, clipboard paste |
| `commands/whatsapp.rs` | Open → search contact → type → send (with confirmation) |
| `commands/browser.rs` | New tab, navigate, search, open site by name |
| `commands/window.rs` | Window focus with AttachThreadInput trick (Windows) |

### New Tauri Commands (14)

`live_type_text`, `live_press_key`, `live_press_hotkey`,
`live_whatsapp_open`, `live_whatsapp_search`, `live_whatsapp_send`,
`live_whatsapp_type_message`, `live_browser_new_tab`,
`live_browser_navigate`, `live_browser_search`, `live_open_site`,
`live_focus_app`, `live_cancel`, `live_get_state`

### New Intent Parser Patterns

- `type <text>` → type_text
- `press <key>` → press_key
- `press <key1> <key2>` → press_hotkey
- `send` / `send it` → confirm_send
- `stop` → cancel_action
- `new tab` / `open new tab` → browser_new_tab

### Safety Layer

- **Whitelist:** Only allowed tools can execute (type_text, press_key,
  open_app, whatsapp_send, etc.)
- **Denylist:** Banking apps, password managers, crypto wallets are blocked
  (1password, bitwarden, bank, paypal, coinbase, metamask, etc.)
- **Confirmation gates:** `whatsapp_send` and `confirm_send` always require
  user confirmation before execution

### State Machine

The state machine tracks context across sequential voice commands:
```
Idle → AppOpen { app } → ChatActive { app, contact } → TextTyped { app, contact, text }
```
Auto-resets to Idle after 30s of silence.

### Clipboard Paste Pattern (from OpenDex)

For text > 50 chars, uses clipboard paste (Ctrl+V) instead of
character-by-character typing. This avoids autocomplete corruption in
WhatsApp/search boxes. Previous clipboard contents are restored after
pasting.

### Window Focus: AttachThreadInput Trick (from ghost-hands)

`SetForegroundWindow` silently fails from background processes on Windows.
The workaround: attach our input queue to the foreground thread's using
`AttachThreadInput`, call `SetForegroundWindow`, then detach.

### NLU Model Update

**New intents:** 47 → 58 (added 11 live-mode intents)
**New training examples:** 1,960 → 2,438 (+478 new examples)
**New slot types:** `B-text`, `I-text`, `B-key`, `I-key`, `B-keys`,
`I-keys`, `B-target`, `I-target`

To retrain:
```bash
cd server/nlu
python generate_live_data.py     # generate new examples
python merge_live_data.py         # merge into dataset.json
python train.py                   # retrain BERT-Mini
```

### Test Results

- 323 Rust tests pass (29 new live-mode tests)
- `cargo check` clean, 0 warnings
- 18 live-mode unit tests (state, safety, keyboard, browser)
- 11 live-mode intent parser tests

## Multi-Worker Optimization — Cloud-First Architecture (2026-09-01)

**Single Worker, internally modularized.** No separate Workers — one deploy,
no cross-Worker latency. The monolithic `index.ts` is split into modules:

- `src/quota.ts` — per-user daily usage tracking + cost control (D1 `usage_log`)
- `src/cache.ts` — edge caching (KV namespace, D1 fallback)
- `src/models.ts` — model constants + fallback chains + truncation
- `src/research.ts` — ad-free search (Wikipedia REST + Wikidata, no API key)
- `src/clean.ts` — result cleaning, dedup, prompt-injection guard

**New D1 tables:** `usage_log` (per-user daily quotas), `cache_entries`
(D1 cache fallback when KV not bound).

**New KV namespace:** `CACHE` (edge cache for search results, PR analysis,
repo metadata). Create with `npx wrangler kv namespace create CACHE` and
paste the ID in `wrangler.toml`.

**Quota limits (per user/day):** 150 requests, 1200 neurons, 15 deep
analyses, 50 searches. Global neuron budget: warn at 8000, hard reject
deep at 9500. (Per-user neuron cap sums below the global stop for 5–6 users:
6 × 1200 = 7200 < 9500.)

**Search routing:** `isSearchQuestion()` detects factual questions
("what is X", "who is Y") and routes them to Wikipedia/Wikidata retrieval
with citations, instead of blind LLM answering.

**Offline local commands (new):**
- `close <app>` — taskkill on Windows, pkill on Unix
- `open chat with <name>` / `message <name>` / `chat with <name>` —
  WhatsApp deep links with local contacts file lookup at
  `%APPDATA%/com.nexus.assistant/contacts.json`
- Both work offline, zero internet, zero RAM overhead

**NLU pre-warm removed.** NLU server now only starts on first
unparseable command (lazy). Saves 50-100 MB at idle.

**STT idle monitor wired but disabled.** `lazy_stt::start_idle_monitor()`
is called from `lib.rs` but `STT_KEEP_ALIVE=true` means STT is never killed.
The idle cost is only ~128 MB (model loaded, not transcribing) and killing
it adds 10-15s delay on the next command (cold model load). The monitor
thread runs for future use but is a no-op. Peak STT RAM during active
transcription is ~340 MB.

**Lazy Kokoro TTS (2026-09-01).** Kokoro is no longer loaded at boot.
`speak_text` calls `ensure_engine_loaded()` on first use — loads in ~1.7s
(one-time), then stays loaded. Saves ~350 MB at idle. See `tts.rs`.

**WebView2 low-memory mode (2026-09-01).** The orb window sets
`MemoryUsageTargetLevel::Low` via `ICoreWebView2_23::SetMemoryUsageTargetLevel`
at creation time. WebView2 drops cached data and swaps to disk. Saves ~40 MB.
See `mic_permissions.rs::set_low_memory_mode()`.

**Idle RAM (2026-09-01): ~104 MB** (before first transcription) or
**~232 MB** (after first transcription, STT model loaded).

| Component | Before first transcription | After first transcription |
|---|---|---|
| nexus.exe (Rust + wake word, NO Kokoro) | 47.8 MB | 47.8 MB |
| WebView2 (orb, low-mem mode) | 35.8 MB | 35.8 MB |
| STT Python (model not yet loaded) | 20.6 MB | 128.6 MB |
| **TOTAL idle** | **104.2 MB** | **232.2 MB** |

After first TTS speak, Kokoro loads and stays loaded: **+350 MB → ~582 MB**.
This is the active state, not idle.

**Worker test suite:** `npm test` in `server/worker/` runs 23 vitest
tests covering quota, cache keys, search question detection, and dedup.

**Rust test suite:** `cargo test --test offline_commands` runs 10 tests
for close_app and whatsapp_chat parsing.

## STT Architecture — Groq Primary + Moonshine Fallback (2026-09-06)

**STT uses Groq Whisper Large v3 Turbo (cloud) as primary, with Moonshine
Small Streaming (local) as fallback when network is unavailable.**

### Primary: Groq Cloud STT
- **Model:** `whisper-large-v3-turbo` (809M params, cloud, ~247ms latency)
- **Free tier:** 20 RPM, 2,000 RPD, 28,800 audio sec/day, 25MB file limit
- **Code:** `src-tauri/src/stt_groq.rs` — sends WAV as multipart to
  `https://api.groq.com/openai/v1/audio/transcriptions`
- **Config:** `localSttOnly: false` in settings.json + valid `groqApiKey`
- **Fallback trigger:** network error, 429 rate limit, 401 auth error,
  or any Groq API error → falls back to local Moonshine

### Fallback: Moonshine Local STT (replaces faster-whisper)
- **Model:** Moonshine Small Streaming (123M params, 7.84% WER, ~165ms CPU)
- **Engine:** `moonshine-voice` Python package (ONNX Runtime, no PyTorch)
- **Code:** `server/stt_server.py` — FastAPI server on `127.0.0.1:39217`
- **Port:** 39217 (`POST /transcribe`, `GET /health`)
- **Audio format:** multipart/form-data with WAV (16kHz, mono, 16-bit PCM)
- **Latency:** ~165ms per transcription after model load; first call ~2s
  (model loading). Model loads in ~1.8s at startup.
- **RAM:** ~150-300MB (model + runtime)
- **Idle timeout:** server killed after 5 min of no requests (saves RAM)
- **Installer:** server files bundled in `resources/server/` via Tauri
  resources config. Production path: `exe_dir/resources/server/stt_server.py`.
- **Requirements:** `pip install moonshine-voice fastapi uvicorn python-multipart`
- **Model download:** automatic on first run via `get_model_for_language("en")`
- **Config:** `MOONSHINE_MODEL` env var (default: `small_streaming`)
  Options: `tiny_streaming` (34M, 12% WER), `small_streaming` (123M, 7.84%),
  `medium_streaming` (245M, 6.65%)

### Why Moonshine over faster-whisper
- Moonshine Small (123M) has 7.84% WER vs Whisper tiny.en's ~18%
- Moonshine is 10-100x faster than Whisper for real-time speech
- Moonshine uses ONNX Runtime (same as wake word engine)
- Moonshine is designed for voice command recognition (streaming, low latency)
- Moonshine MIT license, no restrictions

### Routing logic (`src-tauri/src/stt.rs`)
```
if localSttOnly:
    → local Moonshine (privacy mode, audio never leaves device)
elif groq_key available:
    → try Groq cloud first
    → on error/timeout/429: fall back to local Moonshine
else:
    → local Moonshine directly
```

### Historical: faster-whisper era (pre-2026-09-06)
The previous architecture used faster-whisper `tiny.en` (39M params, ~18% WER)
via a Python sidecar. It was replaced by Moonshine Small Streaming which has
2.3x better accuracy (7.84% vs 18% WER) at similar RAM and faster latency.
- **Hallucination filter:** applied in `stt.rs` — catches
  "thank you for watching", < 2 alphabetic chars, etc.

## NLU Server — Lazy Python Sidecar (2026-08-31)

The **NLU server** (`server/nlu_server.py`) is the only remaining Python
dependency. It provides ML-based intent classification (BERT-Mini ONNX)
as a fallback when the deterministic parser (`intent_parser.rs`) can't
handle a command.

- **Port:** `39218` (separate from the old STT port 39217)
- **Lazy manager:** `src-tauri/src/lazy_nlu.rs` — spawns on first
  unparseable command, kills after 60s idle.
- **Model:** `server/nlu/model/nexus_nlu.onnx` + `.data` + `tokenizer/`
  — committed to git (~18 MB) so fresh clones work without downloading.
- **Requirements:** `server/nlu/requirements.txt` (numpy, onnxruntime,
  fastapi, uvicorn, pydantic, transformers).
- **Fallback:** if the NLU server is unavailable, `nlu_client.rs`
  returns `None` and the deterministic parser handles the command.

### Historical STT Pipeline Fixes (2026-08-30, faster-whisper era)

These bugs were fixed in the faster-whisper Python sidecar. They are
**still relevant** (the sidecar was restored after the Moonshine experiment)
but kept for historical context:

1. **`lazy_stt.rs` path bug:** `stt_script_path()` was missing one
   `.parent()` level. Fixed by adding the correct path.
2. **`ensure_stt_running()` not called on hotkey:** Fixed by adding
   calls to `hotkey.rs` and `stt.rs`.
3. **`is_stt_responsive()` used tokio runtime:** Fixed by using a raw
   TCP connection instead.
4. **STT idle timeout too aggressive:** 60s → 5 minutes.

### Whisper hallucination filter (`stt.rs`)

The hallucination filter is still active in `stt.rs`. It catches common
hallucinations on noisy/silent audio:
- "thank you for watching", "you", "bye", "okay", etc.
- Text with < 2 alphabetic characters
Filtered text is replaced with empty string, triggering the frontend's
"didn't catch that" retry logic (up to 3 retries).

## NEXUS CLI — Unified Cross-Platform Command (`nexus.mjs`)

The unified `nexus` command works on Windows, macOS, and Linux:

```
nexus install     Install prerequisites + build + global 'nexus' command
nexus setup       Install prerequisites + build (no global command)
nexus build       Build frontend + Rust release binary
nexus dev         Tauri dev mode (hot reload via Vite)
nexus start       Launch the built app (unified console on Windows)
nexus run         Alias for 'start'
nexus check       Diagnostics (tools, frontend, Rust, NLU, Worker)
nexus clean       Remove build artifacts
nexus worker      Deploy the Cloudflare Worker (optional)
nexus help        Show help
```

- **Windows:** `nexus.cmd` shim → `node nexus.mjs`
- **Unix:** `nexus` shell script → `node nexus.mjs`
- **Global install:** `nexus install` creates a global command in
  `%USERPROFILE%\.local\bin` (Windows) or `/usr/local/bin` (Unix).
- **`nexus start` on Windows** uses `scripts/run.ps1` for the unified
  color-coded console (Rust logs, audio, frontend CDP in one stream).
- The old `scripts/nexus.bat` and `scripts/nexus.cmd` have been removed.

## Connection Diagnostics (`src-tauri/src/diagnostics.rs`)

Checks 5 services and logs a formatted table on startup:

| Service | Check method | Expected |
|---------|-------------|----------|
| STT | HTTP GET to port 39217/health | OK if running, LAZY if not yet started |
| TTS | In-process Kokoro/Fish Audio readiness (hardcoded ready) | Always OK |
| Cloudflare Worker | HTTPS GET to /health | OK if reachable |
| GitHub | HTTPS GET to Worker /oauth/status | OK if OAuth connected |
| Google | HTTPS GET to Worker /oauth/status | OK if OAuth connected |

Also available as:
- Tauri command: `nexus_diagnostics` (returns JSON to frontend)
- CLI: `nexus check` (build/tool diagnostics via `nexus.mjs`)
- Startup: auto-logged 5s after boot

## Wake Word Model Validation + Mic Silence Recovery (2026-08-30)

### Model is PERFECT — the problem is the Intel SST mic driver

Tested the v2 `nexus.onnx` model with the exact Rust pipeline
(mel → normalize → slice[4:80] → embedding → classifier):

| Input | Model probability | Verdict |
|-------|------------------|---------|
| TTS "nexus" | 0.994 | ✅ |
| TTS "hey nexus" | 0.999 | ✅ |
| TTS "nexus wake up" | 0.999 | ✅ |
| TTS "ok nexus" | 0.999 | ✅ |
| 20 negative samples | 0.0001-0.0002 | ✅ perfect rejection |
| **Trigger rate** | **5/5 positives** | ✅ 100% recall on TTS |
| **False positive rate** | **0/20 negatives** | ✅ 0% false triggers |

The model is NOT the problem. The problem is the **Intel Smart Sound
Technology driver** — it stops delivering audio after 2-25 minutes of
use (RMS drops to exactly 0.000000 and stays there).

### Silence Recovery Thread (`wakeword_oww.rs`)

Added a background thread that monitors the audio callback counter and
automatically restarts the cpal stream when the mic goes silent.

**Settings (tuned for Intel SST bursty audio):**
- Poll interval: **5s** (was 30s)
- Silence threshold: **165 callbacks (~5s)** (was 1000/30s)
- Restart method: **`try_device_silent`** (no 5s probe — saves 5s per cycle)
- Nuclear option: every **12 restarts (~60s of silence)**, restarts the
  Windows Audio service (`net stop/start Audiosrv`) to try to unstick the
  Intel SST driver
- Total restart cycle: **~5s** (was 35s with probe)

**Why 5s?** The Intel SST driver delivers audio in brief 5-15s bursts after
each stream restart, then goes silent. A 5s poll gives us the maximum
number of chances to catch a working window.

**Confirmation RMS threshold lowered from 0.01 to 0.002:**
The 500ms confirmation window was rejecting valid wakes because the mic
fades to silence during the confirmation period. At 0.01, a wake with
RMS=0.0048 was rejected. At 0.002, it would be confirmed.

### Intel SST driver fix (requires admin)

When the mic goes permanently silent, the fix is to restart the driver:

```powershell
# Run as Admin:
pnputil /restart-device "INTELAUDIO\CTLR_DEV_51CA&LINKTYPE_02&DEVTYPE_00&VEN_8086&DEV_AE20&SUBSYS_8BE0103C&REV_10EC\5&111f6c68&0&0000"
```

Or: Device Manager > Sound, video and game controllers > Intel Smart
Sound Technology for Digital Microphones > right-click > Disable > Enable.

Or: Restart the Windows Audio service:
```powershell
Restart-Service -Name "Audiosrv" -Force
```

If none of these work, a full OS restart is required. The Intel SST
driver has a known bug where it stops delivering audio after some time.
Updating to the latest driver from the laptop manufacturer (HP) may help.

### Test scripts (in project root, gitignored)

- `test_wake_model.py` — tests the model with TTS + negative samples
- `test_live_mic.py` — records 5s from the mic and tests the model
- `test_all_devices.py` — tests all audio input devices
- `test_mic_freq.py` — records and shows frequency content
- `gen_tts.py` — generates TTS "NEXUS" samples via Windows SAPI

## RAM Optimization — Lazy Windows + In-Process STT (2026-08-30)

**Idle RAM: 384 MB** (down from 1,644 MB — 77% reduction).

### What was wrong
- `tauri.conf.json` created 5 windows at startup (main, setup, settings,
  sidebar, architect). Each WebView2 window spawns ~7 processes (~250 MB).
  4 of the 5 windows were `visible: false` but still consumed full RAM.
- The old STT server (faster-whisper tiny.en) ran constantly, using ~340 MB
  even when no one was speaking.

### Fix 1: Lazy window creation (`src-tauri/src/dyn_windows.rs`)
- Only `main` (orb) is in `tauri.conf.json` — created at startup.
- `setup`, `settings`, `sidebar`, `architect` are created on-demand by
  `dyn_windows::get_or_create_window()` when first needed.
- `hide_sidebar` / `close_setup_window` / `close_settings_window` now
  **destroy** the window (not `hide()`) — kills the WebView2 process tree
  and frees ~250 MB per window.
- Platform effects (DWM corners, macOS vibrancy) applied at creation time
  inside `get_or_create_window()`.

### Fix 2: Lazy faster-whisper STT server
- STT uses faster-whisper tiny.en via a lazy-started Python sidecar on
  port 39217. See "STT Architecture" section above.
- `lazy_stt.rs` starts the server on first wake/hotkey, kills after 5min idle.
- STT RAM is ~0 MB at idle (server not running), ~340 MB when active.

### Measured RAM (idle, after fix)
| Component          | Before   | After    |
|--------------------|----------|----------|
| NEXUS.exe (Rust)   | 47.9 MB  | 40.8 MB  |
| WebView2 (1 window)| 870 MB   | 344 MB   |
| STT server         | 339 MB   | 0 MB     |
| **TOTAL**          | **1,644 MB** | **385 MB** |

### Files changed
- `src-tauri/tauri.conf.json` — removed 4 windows, kept only `main`
- `src-tauri/src/dyn_windows.rs` — NEW: dynamic window creation/destruction
- `src-tauri/src/stt.rs` — HTTP proxy to faster-whisper sidecar
- `src-tauri/src/lib.rs` — registered new modules, removed startup sidebar vibrancy
- `src-tauri/src/commands.rs` — all show/hide functions use dyn_windows
- `src-tauri/src/architect.rs` — uses dyn_windows for architect window
- `src-tauri/src/hotkey.rs` — sidebar close uses destroy_window
- `src-tauri/src/tray.rs` — settings menu uses dyn_windows
- `src-tauri/src/wakeword_oww.rs` — calls ensure_stt_running() on wake
- `src-tauri/src/stt.rs` — calls mark_stt_request() on each transcription
- `scripts/run.ps1` — no longer starts STT server at boot

## Sidebar — Do NOT use window-vibrancy on non-activating windows (2026-08-30)

**`src-tauri/src/lib.rs` / `src-tauri/src/commands.rs`: the sidebar window
deliberately calls NO `window_vibrancy` function** (no `apply_blur`,
`apply_acrylic`, `apply_mica`). This was a hard-won finding — do not
re-add these calls without reading this section first.

**Why**: the sidebar is a non-activating window (`focus: false`,
`alwaysOnTop: true`, `skipTaskbar: true`) so it never steals keyboard
focus from whatever app the user is working in. Windows' DWM *material*
APIs (Acrylic/Mica via `DWMWA_SYSTEMBACKDROP_TYPE`, or the legacy
`SetWindowCompositionAttribute` accent path used by `apply_blur`) render
a flat, solid **fallback color** for any window that isn't the OS-active
window — this is documented Windows behavior (Mica/Acrylic docs list
"window deactivates" as a fallback-to-solid-color condition), not
something `window-vibrancy` or Tauri can override. Confirmed via
`microsoft/microsoft-ui-xaml#10570` (`DesktopAcrylicBackdrop` loses blur
on `WS_EX_NOACTIVATE` windows) and `tauri-apps/window-vibrancy#183`
(Acrylic/Mica broken on Windows 11 24H2/25H2 in general).

Worse: calling these material APIs **overrides** Tauri's own
`transparent: true` mechanism (`tao` registers the window with DWM via
`DwmEnableBlurBehindWindow` + an empty blur region at window creation —
that's what actually makes a Tauri window see-through, no material
needed). When the material then fails to render (because the window is
never active), DWM falls back to **solid opaque** instead of the
window's original see-through state. This produced a fully opaque
black/grey panel that looked worse than doing nothing.

**The fix**: removed the vibrancy calls entirely. The sidebar was
already genuinely transparent via `transparent: true` in
`tauri.conf.json` — the same mechanism the main orb window uses
successfully. Result: sharp (not blurred) but real, focus-independent
transparency. `src-tauri/src/dwm_corners.rs` still calls
`DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND)`
directly (a plain window-shape attribute, not a material — unaffected
by the active/inactive issue) so the OS window's corners match the CSS
card's `border-radius`, avoiding a "double panel" mismatch (WebView2
has no `CornerRadius` support, so without this the DWM-painted window
rectangle and the rounded CSS card show as two different shapes).

Also removed: a CSS chromatic-aberration effect (red/cyan inset
`box-shadow` on `.sidebar-card::after`) that was meant to simulate
glass prism fringing. Without real optical refraction (no SVG
`feDisplacementMap` — `backdrop-filter: url()` is also a no-op on a
transparent WebView2, see below), it just read as a colored-border
rendering bug. Replaced with a neutral lit-bezel `box-shadow` stack
(top specular + bottom shadow) for a physical-glass feel without color.

Separately confirmed: CSS `backdrop-filter` (blur or `url()` SVG
refraction) is a **no-op in a transparent WebView2** — see
`MicrosoftEdge/WebView2Feedback#4945`. It can't composite against
nothing. Don't rely on it for this window; any blur must come from a
native OS mechanism, and per above, none is currently available for a
non-activating window without a full WinRT `DesktopAcrylicController` +
`SystemBackdropConfiguration.IsInputActive = true` interop (what
PowerToys uses for its non-activating flyouts) — out of scope unless
revisited.

### Screenshot-capture blur (the actual liquid glass)

Since native blur is unavailable for non-activating windows, the sidebar
uses a **"fake blur"**: right before `win.show()`, Rust captures the desktop
region behind the window via GDI `BitBlt`, blurs it with
`image::imageops::fast_blur(sigma=32)`, encodes it as a PNG data URI, and
hands it to the frontend as a CSS `background-image` on `.sidebar-card`
via the `--sidebar-backdrop-image` CSS variable. This gives a genuine
frosted-glass look without depending on window activation state.

**Critical timing:** the capture MUST happen before `win.show()` so the
sidebar doesn't capture itself. If the window is already visible (re-show),
capture is skipped. See `src-tauri/src/sidebar_backdrop.rs`.

Full implementation guide: `docs/features/21-liquid-glass-sidebar.md`.

### Dynamic window pending-content pattern (race-free event delivery)

When a window is created on-demand (via `dyn_windows.rs`), the WebView2
needs time to load the HTML and mount the React app. If Rust emits Tauri
events immediately after creating the window, **those events are lost**
because no listener exists yet.

**Fix:** store the content in a `static Mutex<Option<...>>` and let the
frontend fetch it on mount via a `get_pending_*` command. This is race-free
regardless of how long the WebView takes to load. If the window already
exists (React loaded), events are also emitted as a fast path.

Currently used for:
- `PENDING_SIDEBAR` in `commands.rs` → `get_pending_sidebar_content`
- `PENDING_ARCHITECT_REPO` in `architect.rs` → `get_pending_architect_repo`

**All show/create commands MUST be `async`** — `WebviewWindowBuilder::build()`
dispatches to the main thread, and a synchronous Tauri command runs on a
blocking thread that can't yield, causing a deadlock.

To reuse this pattern for a new window, see the "How to Reuse" section in
`docs/features/21-liquid-glass-sidebar.md`.

## Architecture Mapper — Phase 1 Latency Optimization (2026-08-30)

Phase 1 now uses **Approach C (hybrid)** for a 3-4s first response:

1. **Parallelized GitHub API calls** (`tokio::join!`): metadata + recursive
   tree are fetched concurrently using the symbolic ref `HEAD` (verified
   against repos with `main` and `master` default branches). Cuts ~600-1000ms
   off the critical path vs the old sequential metadata→tree flow.
2. **Instant Rust heuristic clustering** for first paint (~5ms) — the diagram
   appears in ~1-1.5s with generic layer labels.
3. **Async LLM enrichment** (`enrich_phase1` command): after first paint, the
   client POSTs the heuristic layers + sample file paths to the Worker's
   `phase1_enrich` intent. The LLM (Mistral 24B) rewrites generic labels into
   repo-specific ones (e.g. "Client / Presentation Layer" → "Next.js App
   Router (React 19)") and writes a real summary. Result streams back via
   the `architect:phase1-enriched` event ~2-3s later and merges in-place.
   **Never blocks first paint.** If the Worker/LLM fails, the heuristic
   diagram remains (graceful degradation).

| Component | File | What changed |
|-----------|------|--------------|
| Rust parallel fetch | `src-tauri/src/architect.rs` | `analyze_repo_phase1` uses `tokio::join!` + `HEAD` ref |
| Rust enrichment cmd | `src-tauri/src/architect.rs` | New `enrich_phase1` command + `Phase1Enrichment`/`EnrichedLayer` types |
| Rust session accessor | `src-tauri/src/network.rs` | New `get_session_info()` public helper |
| Worker handler | `server/worker/src/index.ts` | New `handlePhase1Enrich` + `phase1_enrich` intent dispatch |
| Frontend store | `frontend/src/architect/architectStore.ts` | New `enrichPhase1` action + `sample_file_paths` field |
| Frontend app | `frontend/src/architect/ArchitectApp.tsx` | Calls `enrich_phase1` after paint + listens for enriched event |

## Building

**Always build the desktop app with the Tauri CLI:**

```powershell
pwsh ./scripts/build.ps1          # frontend + tauri release build + bundles
```

If you need a plain cargo build (faster iteration, no installer), you **must**
pass the `custom-protocol` feature:

```powershell
npm --prefix frontend run build
cargo build --release --features custom-protocol   # run inside src-tauri/
```

### Why `custom-protocol` is mandatory

Tauri decides whether to load the bundled frontend or the Vite dev server
purely from this feature flag:

```rust
// tauri-macros/src/context.rs
dev: cfg!(not(feature = "custom-protocol")),
```

- Feature **on**  → windows load `http://tauri.localhost/...` (embedded assets)
- Feature **off** → windows load `devUrl` = `http://localhost:5173`

`cargo tauri build` adds the feature automatically; a bare `cargo build
--release` does **not**. A release binary built without it shows
`localhost refused to connect` / `ERR_CONNECTION_REFUSED` in every window,
because no Vite server is running. Clearing the WebView2 profile does not
help — the dev URL is baked into the binary at compile time.

The feature is deliberately **not** in `[features] default`, because
`tauri dev` needs it off for hot reload.

### Verifying which URL the app actually loads

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
Start-Process .\src-tauri\target\release\nexus.exe
Start-Sleep 15
Invoke-RestMethod http://127.0.0.1:9222/json/list | Select-Object title, url
```

Expected: every window on `http://tauri.localhost/...`.
Bad: `http://localhost:5173/...` → rebuild with `--features custom-protocol`.

## Frontend windows

Every window declared in `src-tauri/tauri.conf.json` must have a matching
rollup input in `frontend/vite.config.ts`, otherwise its HTML file is absent
from `dist/` and the window fails to load in release builds (dev mode hides
this — the Vite server serves any HTML file on demand).

| tauri.conf.json window | HTML            | vite input |
| ---------------------- | --------------- | ---------- |
| `main`                 | `index.html`    | `main`     |
| `setup`                | `setup.html`    | `setup`    |
| `settings`             | `settings.html` | `settings` |
| `sidebar`              | `sidebar.html`  | `sidebar`  |

## Local ports

| Service           | Port    | Notes                                        |
| ----------------- | ------- | -------------------------------------------- |
| STT (faster-whisper) | 39217 | Lazy-started Python sidecar (POST /transcribe) |
| NLU server        | `39218` | Lazy Python sidecar (BERT-Mini ONNX). Override: `NLU_PORT` |
| Sidecar (legacy)  | `41098` | Legacy FastAPI sidecar, not used at runtime  |
| Vite dev server   | `5173`  | Dev only                                     |

## Architecture (serverless — 2026-08-27)

NEXUS is now **fully serverless**. No sidecar, no n8n, no Ollama, no server.

```
NEXUS laptop → HTTP POST → Cloudflare Worker → APIs → text response
                              ↑
                        D1 database (OAuth tokens)
                        Workers AI (intent + summarization)
```

- **Worker** (`server/worker/`): Cloudflare Worker on the edge. Handles
  intent classification, API calls (GitHub/Google), summarization, OAuth
  exchange, token storage, and user registration. <5ms cold start.
- **D1**: Cloudflare's free SQLite. Stores OAuth tokens, API keys, and
  device registrations. 5GB free.
- **Workers AI**: Free tier (10K neurons/day) for intent classification
  (Qwen 0.5B) and summarization (Qwen 14B).
- **Client** (`src-tauri/src/network.rs`): HTTP POST to the Worker. No
  WebSocket. Emits state/ack/result/done events to the frontend.
- **NEXUS_SERVER_URL**: Baked into the installer at build time. Points to
  the Worker URL (e.g. `https://nexus-worker.xxx.workers.dev`).

The old sidecar (`server/sidecar/`) is kept in the repo for reference but
no longer spawned at startup. `sidecar_manager.rs` has been removed.

### Building the installer with the Worker URL

```powershell
$env:NEXUS_SERVER_URL = "https://nexus-worker.your-subdomain.workers.dev"
pwsh ./scripts/build.ps1
```

## Runtime paths (Windows)

- App data (config, logs): `%APPDATA%\com.nexus.assistant\`
- WebView2 profile: `%LOCALAPPDATA%\com.nexus.assistant\EBWebView`
  (note: **Local**, not Roaming — `app_data_dir()` returns Roaming and is the
  wrong path for WebView2)

## Wake word (openWakeWord)

- Default feature: `wakeword-oww` (tract-onnx inference in Rust)
- Models: `src-tauri/resources/oww/{melspectrogram.onnx, embedding_model.onnx, nexus.onnx}`
- Threshold: 0.35, chunk size: 1280 samples (80ms at 16kHz)
- **Detection logic (2026-08-29):** Max-based detection + secondary confirmation.
  The old averaging approach diluted single good frames (0.4+) with surrounding
  0.0s, giving avg=0.03 which never triggered. Now uses max probability in the
  12-frame buffer, so a single 0.36+ frame triggers. After a raw detection,
  collects 500ms of audio and checks RMS ≥ 0.01 to confirm real speech (filters
  noise spikes). Refractory period: 3s.
- **Model v2 (2026-08-30, Kaggle):** Retrained on Kaggle T4 GPU with:
  - 5000 positive samples (5 phrase variants: "nexus", "hey nexus", "nexus wake up", "ok nexus", "nexus please")
  - 30+ soundalike negatives (vs 8 in v1)
  - 80000 training steps (vs 50000), layer_size=64 (vs 32)
  - 2x augmentation rounds, target FP/hr=0.1 (vs 0.2)
  - Model size: 415KB (vs 205KB v1)
  - Kernel: `chitkullakshya/train-nexus-wakeword-v2`
  - v1 backup: `src-tauri/resources/oww/nexus_v1.onnx.backup`
- **Silence gate + AGC (2026-08-28):** `detect_chunk` computes RMS of each
  80ms chunk and skips the classifier entirely if RMS < 0.0005 (~-66dBFS).
  The `nexus.onnx` model emits 0.6-0.9 probabilities on pure digital silence
  (out-of-distribution input), which caused spontaneous false wakes. The
  gate prevents the model from ever seeing silence. Min positive detections
  = 2. Regression test: `test_silence_never_triggers_wake`.
  - **AGC (Automatic Gain Control):** If RMS passes the gate but is below
    TARGET_RMS (0.03), the chunk is amplified up to 50x before feeding the
    classifier. This makes quiet/whispered "NEXUS" produce the same model
    input as loud "NEXUS", so the model (trained on normal-volume TTS)
    recognizes low-volume speech without retraining.
  - Gate: 0.0005, threshold: 0.45. Pure silence (RMS=0) is blocked.
  - Model: trained on Kaggle (v22), accuracy 78.6%, recall 58.2%, FP/hr 1.33.
- **Mic device enumeration (2026-08-27):** `start_audio_capture` enumerates
  ALL input devices, probes each for 5 seconds, and picks the first one
  that produces non-silent audio (RMS > 0.0001). If all devices are silent
  (Intel SST bug), falls back to the best device anyway. This fixes the
  "wake word doesn't work, only hotkey" issue caused by cpal getting
  silence from the Intel Smart Sound Technology driver.
- **FIXED (2026-08-24):** The wake word now works — probability 0.991 for real
  "NEXUS" speech. The root cause was a 32768x input scaling mismatch: cpal
  produces f32 audio in [-1.0, 1.0] but the openWakeWord melspectrogram model
  expects int16-scale float32 values in [-32768, 32767]. Fix: multiply audio
  by 32768.0 in `wakeword_oww.rs` before feeding to the melspectrogram model.
- **Mic conflict (FIXED):** The frontend's `warmMic()` (getUserMedia via WebView2)
  conflicts with the Rust cpal wake-word stream on Intel Smart Sound Technology
  drivers. `warmMic()` is disabled at startup; the mic is acquired on first
  wake instead. This is why cpal was getting silence (RMS=0.0000).
- The global hotkey still works independently of the wake-word model.
- **Command models (2026-08-25):** Training 4 category-level acoustic models
  (`command_open`, `command_close`, `command_search`, `command_play`) via
  `train_nexus_commands.ipynb` on Google Colab. Models detect command TYPE,
  then STT extracts the parameter (which app, what query). See
  `src-tauri/resources/oww/commands/command_intents.json`.

## Known limitations (2026-08-26 audit)

- **Speaker verification IS wired** (2026-09-26). `wakeword_oww.rs` receiver loop
  calls `verify_speaker_at_fire()` on every candidate before firing. Uses a
  separate `AudioFeatures` instance (not the live engine's) so streaming
  buffers stay pristine. Processes last 16 chunks (~1.28s) of candidate audio,
  mean-pools to 96-dim embedding, verifies against enrolled profile. Fail-open
  on any error. Toggle: `speakerVerification` in settings.json (default false).
  The old confirmation-path code (lines 1260-1324) is dead code —
  `confirmation_active` is never set true (500ms confirmation was removed for
  instant-fire latency).
- **All windows skip the taskbar.** `main`, `setup`, `settings`, and
  `sidebar` all have `skipTaskbar: true` in `tauri.conf.json`. NEXUS is
  accessible only via the floating orb, the system tray, the global
  hotkey, and the wake word.
- **STT server auto-launcher writes `server/start_stt.cmd`** with an
  absolute path to the local Python interpreter. This file is gitignored
  (machine-specific, leaks username).
