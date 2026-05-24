# VoiceTyper — TODOS

Deferred work from the 2026-05-23 `/plan-ceo-review`. v1 scope lives in DESIGN.md
(build sequence). This file is what's intentionally NOT in v1.

## v1.1 (after the raw, tray-only daily driver works)

- **Floating box (egui).** Small always-on-top window: settings (gear) button + mic
  button, recording animation. Replaces tray-only state feedback. Effort M.
  Why deferred: pulls in a GUI rendering layer; tray-only reaches daily use faster.
- **Silence auto-stop (VAD, ~10s).** RMS-threshold silence detection on the captured
  stream to auto-stop, like Windows Voice Typing (~10s). Effort M.
  Why deferred: threshold tuning eats time; press-again / click-away stop works day one.
- **Settings window + auto-punctuation toggle.** GUI for Groq key, model, shortcut,
  auto-punctuation. Effort S. v1 uses a plain config file instead.

## v2 — the soul

- **Conservative LLM cleanup pass.** Pipe the transcript through a Groq LLM with a
  conservative prompt (strip filler + fix punctuation, do NOT reword/change meaning).
  Raw/clean toggle. Effort M. Keep the prompt in config so it's tunable without a
  rebuild. NOT the sibling app's `improve.py` (that's a pedagogical IELTS rewrite —
  wrong tool).

## Polish

- **Clipboard restore hardening.** Snapshot/restore non-text clipboard formats (images,
  files), not just text. The #1 "feels broken" regression risk since injection is
  clipboard-paste. Effort M.
- **Error toasts**, **autostart-on-login** (HKCU `...\Run` key or Startup shortcut),
  and **installable `Setup.exe`** (Inno Setup or NSIS — wraps the built exe, no MSVC
  needed). The installer is a hard requirement, just not for the capture spike.

## Settings + installer milestone (building 2026-05-24) — deferred bits

- **Mic sensitivity in the settings window.** Expose `SPEECH_RMS` (maybe the VAD
  timings too) as a field/slider. Fast-follow after the first window ships. It's the one
  hidden knob that affects transcription quality per mic/room, unreachable today without
  a recompile. The first window ships key + mode only (user's call). Effort S.
- **Live-apply settings.** Make changes take effect immediately instead of on next launch.
  v1 is restart-to-apply. Effort S-M (worker re-reads config between sessions).
- (autostart-on-login: user explicitly declined for the installable build — see Polish.)

## Future / bigger bets

- **Cross-OS support (macOS / Linux).** Run VoiceTyper beyond Windows. Parked because the
  settings window is the EASY part — the real work is rewriting the two deeply OS-specific
  pieces: the global hotkey listener (`WH_KEYBOARD_LL` via the windows crate) and the
  paste-into-any-app injection (`SendInput` + clipboard). Every OS does both completely
  differently; they dwarf the window. The native Windows-only settings window chosen
  2026-05-24 would be rebuilt then, but that's cheap next to the hotkey + inject rewrites.
  Revisit a cross-platform GUI toolkit (e.g. egui) only when tackling this epic. Effort XL.

## Dropped (revisit only if the product direction changes)

- **Auto-launch on editable-field focus** (+ "select a text box first" message).
  Needs UI Automation focus-detection that's unreliable across browsers/Electron/
  terminals. Aborted 2026-05-23.
- **True live word-by-word streaming.** Groq is batch-only; local STT ruled out
  (cloud-only forever). Not feasible without changing that decision.
