# VoiceTyper — Design

A lightning-fast, free voice-typing daemon for Windows. Press a hotkey, speak, and clean text lands in whatever app you're in. Powered by Groq (Whisper for speech-to-text, a fast LLM for cleanup). Built in Rust — single self-contained binary, no runtime install on Windows 10+.

Status: design. No code yet. Created from a `/office-hours` session on 2026-05-23; stack revised from C#/.NET to Rust in a follow-up `/office-hours` session the same day (see Decision log).

---

## Why this exists

Windows' built-in voice typing is slow and low-quality. The good third-party tools (Wispr Flow and friends) are Electron apps in the ~200MB range and charge a subscription (~$15/mo). Groq's API does top-tier Whisper transcription for free and fast. So the honest pitch: **a tool I own, free on Groq, that turns my rambling into clean text** — without paying a subscription or running a 200MB Electron process in the background all day.

"Tiny" is a constraint, not the selling point. It forces a native always-on daemon with fast startup and near-zero idle RAM. The selling point is *free + the LLM cleanup pass*.

---

## Decisions (from the office-hours session)

| Decision | Choice | Why |
|----------|--------|-----|
| Audience | Personal tool first, maybe productize later | Scratch own itch; keep momentum, skip premature business theater |
| Soul | Speech-to-**clean**-text | Groq also serves fast LLMs; a cleanup pass costs ~1s and $0 on the same free API. This is the "whoa" incumbents charge for. |
| Platform | Windows first | Where the daily pain is |
| Stack | Rust | Cloud-only thin client (no local inference), so language is chosen for footprint + glue, not engine perf. Smaller self-contained binary (~5-10MB), near-zero idle RAM, no runtime install, none of the AOT-trimming gotchas. (Originally C#/.NET NativeAOT — see Decision log.) |
| Interaction | Toggle (press to start/stop), batch inject | Ctrl+Win to start; stop on press-again, click-away, or ~10s silence (v1.1); then the full transcript pastes at once. Toggle, not hold, so you can pause and think mid-dictation. Matches Groq's batch API. |

---

## Architecture

Toggle dictation daemon. The whole app is three Windows system tricks glued to a Groq call (two once cleanup lands).

```
  [press hotkey] ──▶ start WASAPI mic capture (cpal)
                         │
  [press again / click-away / ~10s silence] ──▶ stop capture ──▶ encode WAV (hound)
                         │
                         ▼
              POST audio ──▶ Groq Whisper API ──▶ raw transcript
                         │
                         ▼   (v2 — the soul)
              POST text ──▶ Groq LLM (cleanup prompt) ──▶ clean text
                         │
                         ▼
              inject text into focused window (clipboard + Ctrl+V)
                         │
                         ▼
              tray icon: idle ▸ recording ▸ processing ▸ idle
```

A `tao` event loop drives the Windows message pump the low-level keyboard hook needs, with a `tray-icon` hung off it. v1 is tray-only (no rendered window); v1.1 adds a small floating box (`egui`) with an animated mic + settings button. Capture + network + injection run on worker threads / `tokio` so the hook callback stays fast — which matters, because a slow hook callback gets silently uninstalled by Windows (see "Where the effort goes").

### State machine

```
        ┌─────────┐  hotkey press  ┌───────────┐   stop trigger    ┌────────────┐
        │  IDLE   │ ─────────────▶ │ RECORDING │ ────────────────▶ │ PROCESSING │
        └─────────┘                └───────────┘                   └────────────┘
             ▲                          │                               │
             │                          │ stop with <300ms audio        │ text injected
             │                          ▼                               │ OR error
             └──────────── (discard, too short) ◀───────────────────────┘

  stop trigger = hotkey press again │ focus leaves target field (click-away) │ ~10s silence (v1.1)
```

Guard: ignore captures under ~300ms (an accidental double-press that starts then immediately stops). On any error (network, empty transcript), return to IDLE and flash the tray icon red — never silently swallow.

---

## Key design choices (and the tradeoffs)

**Text injection: clipboard-paste, not synthetic keystrokes.**
Two ways to get text into the focused app:
- `SendInput` with per-character Unicode key events. Works, but some apps mishandle fast synthetic input, and IME/emoji get flaky.
- Put text on the clipboard, send Ctrl+V, then restore the old clipboard. More reliable across VS Code, browsers, Slack, terminals.

Default: **clipboard-paste with save/restore**. The risk is clobbering whatever the user had copied — so we snapshot the clipboard before, restore it ~200ms after paste. Document this; it's the #1 thing that'll feel broken if it regresses.

**Two-stage Groq, cleanup is toggleable.**
v1 ships raw transcript only (Whisper → inject). v2 adds the LLM cleanup pass with a raw/clean toggle (a tray menu item or a modifier key). Keep the cleanup prompt in config so it's tunable without recompiling.

**Config over hardcoding.**
`%APPDATA%\VoiceTyper\config.toml`: Groq API key (or read `GROQ_API_KEY` env), hotkey binding, model names, cleanup on/off, cleanup prompt. API key never compiled in.

**Toggle + batch inject, not live streaming.**
Press to start; stop on press-again, click-away, or ~10s silence (v1.1); then the whole clip goes to Groq and the full text pastes at once. No live word-by-word insertion — Groq is batch-only, and the reference tools (whisper-writer) batch too. Toggle beats hold because you can pause to think without holding a key down. No wake word, no continuous always-listening. Silence-based auto-stop (~10s, like Windows Voice Typing) is v1.1; v1 stops on press-again / click-away.

---

## Where the effort actually goes

Not the STT (Groq solves it). Not the size (Rust solves it). The real work:

1. **Text injection that lands correctly in every app** — expect this to be ~60% of debugging. VS Code, browser address bars, Slack, terminals, password fields (which may block paste) all behave differently.
2. **Global toggle hotkey that fires when the app isn't focused** — the chosen bind, **Ctrl+Win**, is a modifier-only chord, and `RegisterHotKey` can't register a chord with no non-modifier key. So a low-level keyboard hook (`SetWindowsHookEx` with `WH_KEYBOARD_LL`) is still required — now to detect the Ctrl+Win combo, not (as in the old push-to-talk plan) to catch key-up. Toggle actually simplifies the logic: act on the key-down combo, flip IDLE↔RECORDING, ignore key-up. Avoid Win+H (reserved by Windows Voice Typing); Ctrl+Win is free. Rust caveats: (1) the callback is an `extern "system" fn` (a plain function pointer, so no GC to worry about), but the `HHOOK` must be kept alive for the process lifetime (a `static`/leaked alloc). (2) The callback runs under `LowLevelHooksTimeout` (~300ms default) — it must signal a worker thread and return immediately, never start/stop capture inline, or Windows silently uninstalls the hook. This is the "why does my hook randomly die" trap.
3. **Mic capture lifecycle** — start/stop cleanly on every hotkey cycle without leaking audio streams or dropping the first 100ms of speech.

---

## Build sequence

Each step is a working checkpoint. Don't skip ahead.

1. **Capture spike** — Ctrl+Win toggles mic capture on/off → write `out.wav` to disk. Prove the `WH_KEYBOARD_LL` hook (chord detection) + `cpal` capture + `tao` event loop + `tray-icon` coexist (the event loop's message pump is what makes the hook fire). No network yet.
2. **Transcribe** — POST the WAV to Groq Whisper, print transcript to console. Prove the API call + auth. (The sibling Articulate app already proves Groq Whisper works on this machine/key — see `Articulate/backend/services/transcription.py`.)
3. **Inject** — paste the transcript into the focused window via clipboard save → set → Ctrl+V → restore.
4. **Toggle stop + tray state** — stop on press-again and click-away (focus loss); tray icon reflects idle/recording/processing; quit menu item. **This is v1: the raw, tray-only daily driver.** Start using it.
5. **v1.1 — box + VAD + settings** — floating box (`egui`) with animated mic + settings gear; ~10s silence auto-stop (VAD); auto-punctuation toggle; settings window (Groq key, model, shortcut).
6. **v2 — cleanup pass (the soul)** — pipe transcript through a Groq LLM with a conservative cleanup prompt; raw/clean toggle.
7. **Polish** — clipboard save/restore hardening (non-text formats), error toasts, autostart-on-login, installer (`Setup.exe` via Inno Setup / NSIS).

v1 (steps 1-4) is the first daily driver. v1.1 (step 5) makes it feel like Windows Voice Typing. The soul (step 6) is the cleanup pass.

---

## Libraries & APIs

- **Tray + message loop** — `tray-icon` + a `tao` (or `winit`) event loop. The event loop runs the Win32 message pump the low-level hook depends on. No main window in v1.
- **Global hotkey (toggle)** — the `windows` crate: raw `SetWindowsHookExW(WH_KEYBOARD_LL, ...)` to detect the Ctrl+Win chord. (`RegisterHotKey` can't bind a modifier-only chord; see "Where the effort goes".)
- **Floating box (v1.1)** — `egui` / `eframe` for the small always-on-top window: settings + mic buttons, recording animation.
- **Mic capture** — `cpal` (WASAPI backend); `hound` for WAV encoding.
- **HTTP** — `reqwest` (with `multipart`) over `tokio` for the Groq audio upload; plain JSON POST for the LLM cleanup call.
- **Clipboard** — `arboard` (or the `windows` crate clipboard APIs); used for paste + save/restore.
- **Paste / keystroke injection** — the `windows` crate `SendInput` to synthesize Ctrl+V (and as fallback, Unicode keystrokes).
- **Config + JSON** — `serde` / `serde_json` for the Groq wire types; `toml` for the config file. No reflection, no source generators, no trimming model to fight.

Build: `cargo build --release` (`x86_64-pc-windows-msvc`). Shrink the binary later with `strip`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, and a lean TLS backend (rustls or schannel) if size matters.

---

## Open questions

- **Hotkey.** Resolved: **Ctrl+Win** (toggle). Modifier-only chord, so it needs the low-level hook, not `RegisterHotKey`. Avoids Win+H (Windows Voice Typing). Configurable later (v1.1 settings).
- **Click-away stop detection.** "Focus leaves the target field" stops recording. How to detect cheaply — `WinEvent` focus hook (`SetWinEventHook`/`EVENT_OBJECT_FOCUS`) vs. polling foreground window? Resolve in step 4.
- **Silence auto-stop threshold (v1.1).** Windows uses ~10s. Tune for thinking pauses vs. responsiveness. RMS threshold over a rolling window on the captured stream.
- **Paste into paste-blocking fields** (some password managers, secure fields). Detect and fall back to keystroke injection, or just accept the limitation?
- **Cleanup prompt (v2)** — how aggressive? Strip filler + punctuate only, or allow light rewording? Over-eager rewriting erodes trust. Lean conservative.
- **Latency budget.** Target: stop-to-text under ~2s for raw, under ~3s with cleanup. Measure early.

---

## Success criteria

- v1: I reach for VoiceTyper instead of typing, for real messages, within a week of building it.
- Stop-to-text feels instant enough that I don't wait on it (sub-2s raw, for short clips).
- Text lands correctly in the apps I actually use daily (editor, browser, Slack).
- Idle footprint is negligible — I forget it's running.
- Binary is a single self-contained `.exe` in the ~5-10MB range (Rust release build before size tuning; verify in the spike), no install, no runtime download.

---

## Decision log

**2026-05-23 — Stack: C#/.NET NativeAOT → Rust.**
The first office-hours session picked C#/.NET NativeAOT for "best-documented Win32
interop" and fastest path to a daily driver. That was a defensible call. A follow-up
session reversed it after fixing two things:

- *Cloud-only is permanent.* Groq does all STT; there is no on-device inference, ever.
  That removes the only structural reason to favor C# (there is no engine to bind,
  nothing for "easy interop" to earn). It also means VoiceTyper is, forever, a thin
  push-to-talk HTTP client — language should optimize footprint + glue, not engine
  perf.
- *"STT projects are C/C++" was about engines, not apps.* whisper.cpp et al. are C/C++
  because they run neural-net inference on-device. VoiceTyper runs none, so that
  precedent does not apply. (The cited reference, OpenWhispr, is at the app layer a
  ~200MB Electron/TypeScript app — the heavyweight thing this project avoids.)

Net: for a cloud-only thin client, Rust beats C# on binary size (~5-10MB vs ~10-15MB),
idle RAM, and by deleting the whole AOT-trimming gotcha class (GC'd hook delegate, JSON
source-gen, NAudio AOT paths). C/C++ was considered as a fallback and rejected: half
the app is network code (multipart upload + JSON) that Rust crates hand you for free
and C++ makes you hand-roll. C# stays a sane choice if local inference is ever wanted —
it is not.

**2026-05-23 — Interaction: push-to-talk → toggle; windowless → tray-now / box-v1.1.**
A `/plan-ceo-review` session reshaped v1 toward a Windows Voice Typing (Win+H) style tool:

- *Push-to-talk (hold) → toggle (press to start/stop).* You need to pause and think
  mid-dictation; you can't hold a key for two minutes. Stop triggers: press-again,
  click-away (focus loss), or ~10s silence (v1.1).
- *Batch inject confirmed, not live streaming.* Groq is batch-only and the reference
  tool (whisper-writer) batches too, so the full text lands at once on stop, not
  word-by-word. Expectation set deliberately.
- *Windowless → tray-only v1, floating box in v1.1.* The box (`egui`: settings + mic
  buttons, animated mic) pulls in a GUI rendering layer, so it is phased to v1.1 to
  reach daily use faster. v1 uses `tray-icon` state only.
- *Dropped: auto-launch on editable-field focus* (+ the "select a text box" message).
  Needs UI Automation focus-detection that's unreliable across browsers/Electron/
  terminals. Not worth it.
- *Wedge: raw-first.* v1 ships raw Whisper (already looks great); conservative LLM
  cleanup is the v2 fast-follow.

## Later (not v1)

- Groq LLM cleanup tuning + per-app tone (the "context-aware" idea — code comments in editor, casual in Slack).
- Always-listening / continuous dictation (VAD as a *trigger*, not just auto-stop). Note: silence-based auto-stop is v1.1, not this.
- Cross-platform (macOS/Linux) if it ever goes beyond personal use.
- Signed/code-signed installer + auto-update — only if productizing. (A basic `Setup.exe` and a settings window are already in scope: see build sequence steps 5 and 7.)
