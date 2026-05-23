# VoiceTyper

Lightning-fast, free voice typing for Windows. Press a key, speak, and clean text lands in whatever app you're in.

Speech-to-text via Groq Whisper; an optional Groq LLM pass cleans up filler and punctuation before the text is typed. Native Rust — a single ~5-10MB binary, no runtime install, negligible idle footprint.

> Status: design phase. See [DESIGN.md](DESIGN.md) for the architecture, build sequence, and decisions.

## The idea in one loop

```
press hotkey → record mic → (press again / click-away / silence) → Groq Whisper → (Groq LLM cleanup, v2) → paste into focused window
```

Toggle to start/stop, batch inject. No always-listening, no wake word, no subscription.

## Requirements (planned)

- Windows 10+ (x64)
- A free Groq API key ([console.groq.com](https://console.groq.com))
- Rust toolchain (stable, `x86_64-pc-windows-msvc`) to build

## Build (planned)

```powershell
cargo build --release
```

Produces a self-contained `.exe` (`target\release\`). No install step.
