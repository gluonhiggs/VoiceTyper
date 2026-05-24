# VoiceTyper

**Lightning-fast, free voice typing for Windows** — a more accurate alternative to the
built-in Voice Typing (`Win`+`H`).

Press `Ctrl`+`Win`, speak, and clean text lands in whatever app you're focused on —
editor, browser, chat, terminal, and more.
Lives quietly in your system tray. Single self-contained app, no runtime to install.

---

## Thanks to Groq 💜

VoiceTyper is lightning-fast and free thanks to **[Groq](https://groq.com)**'s generosity.

To use VoiceTyper, grab a (free) Groq key:

1. Go to **[console.groq.com](https://console.groq.com)** and sign in.
2. Open **API Keys → Create API Key** and copy it (it starts with `gsk_`).
3. After installing (below), right-click the tray icon → **Settings**, paste the key, **Save**.

Your key stays local in `%APPDATA%\VoiceTyper\config.toml`.

---

## Install

1. Download the latest **`VoiceTyper-Setup-x.y.z.exe`** from the
   [**Releases**](https://github.com/gluonhiggs/VoiceTyper/releases) page.
2. Run it. It installs **per-user (no admin needed)** and adds a Start Menu shortcut.
   - Windows SmartScreen may show *"Windows protected your PC"* because the app isn't
     code-signed yet — click **More info → Run anyway**. (One-time.)
3. VoiceTyper appears in your system tray.

---

## Using it

1. Click into any text field — a document, a browser box, a chat, a terminal.
2. Press **`Ctrl`+`Win`** and start talking.
3. Your words appear right where the cursor is.

Under the hood:

```
Ctrl+Win  →  record mic  →  (pause / press again / ~10s silence)  →  Groq Whisper  →  text typed into the focused window
```

Two modes (pick in **Settings**):

- **Hands-free** (default) — just start talking; each phrase is typed as you pause.
  Re-press `Ctrl`+`Win`, or stay silent ~10s, to stop.
- **Toggle** — press to start, press to stop; transcribes the whole utterance at once
  (best punctuation and capitalization).

The tray icon shows what it's doing: **blue** idle, **red** listening, **amber** processing.

---

## Settings

Right-click the tray icon → **Settings**:

- **Groq API key**
- **Mode** — Hands-free or Toggle
- **Silence timeout** — the pause length that ends a phrase (0.5–5s). Raise it if you tend
  to think mid-sentence; lower it for snappier results.

Changes take effect on your next `Ctrl`+`Win` press. Everything is saved to
`%APPDATA%\VoiceTyper\config.toml` (so it survives reboots and reinstalls).

---

## Roadmap

- **AI cleanup pass** — an optional toggle to polish grammar and punctuation with a Groq
  LLM before the text is typed (great when English isn't your first language).
- Custom hotkey, more languages, and macOS / Linux support.

---

## Build from source

VoiceTyper is pure Rust. To build it yourself:

```powershell
cargo build --release
```

For the full development setup (toolchain, tests, and the release process), see
**[CONTRIBUTING.md](CONTRIBUTING.md)**. For architecture and design decisions, see
**[DESIGN.md](DESIGN.md)**.

---

## License

[MIT](LICENSE) — use it however you like.
