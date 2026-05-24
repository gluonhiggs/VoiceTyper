# VoiceTyper

**Lightning-fast, free voice typing for Windows**
Press `Ctrl`+`Win`, speak, and clean text lands in whatever app you're focused on —
editor, browser, chat, terminal, and more. Lives quietly in your system tray; one
self-contained app, no runtime to install.

## Thanks to Groq 💜

VoiceTyper is lightning-fast and free thanks to **[Groq](https://groq.com)**'s generosity.

You'll need a free Groq key:

1. Sign in at **[console.groq.com](https://console.groq.com)**.
2. **API Keys → Create API Key**, and copy it (starts with `gsk_`).
3. After installing, right-click the tray icon → **Settings**, paste it, **Save**.

Your key stays local in `%APPDATA%\VoiceTyper\config.toml`.

## Install

1. Download the latest **`VoiceTyper-Setup-x.y.z.exe`** from [**Releases**](https://github.com/gluonhiggs/VoiceTyper/releases).
2. Run it — installs per-user (no admin), adds a Start Menu shortcut. SmartScreen may warn
   (unsigned app); click **More info → Run anyway**.
3. VoiceTyper appears in your tray.

## Using it

Click into any text field, press **`Ctrl`+`Win`**, and talk — your words appear at the
cursor. Two modes (set in **Settings**):

- **Hands-free** (default) — types each phrase as you pause; stop by re-pressing
  `Ctrl`+`Win` or staying silent ~10s.
- **Toggle** — press to start, press to stop; transcribes the whole utterance at once
  (best punctuation).

Tray icon: **blue** idle · **red** listening · **amber** processing.

## Settings

Right-click the tray icon → **Settings**:

- **Groq API key**
- **Mode** — Hands-free or Toggle
- **Silence timeout** — the pause that ends a phrase (0.5–5s); raise it if you think mid-sentence.

Changes apply on your next `Ctrl`+`Win` press.

## Roadmap

- **AI cleanup pass** — optional toggle to polish grammar & punctuation via a Groq LLM before typing.
- Custom hotkey, more languages, macOS / Linux.

## Build from source

```powershell
cargo build --release
```

Dev setup, tests, and releasing: **[CONTRIBUTING.md](CONTRIBUTING.md)**.
Architecture and decisions: **[DESIGN.md](DESIGN.md)**.

## License

[MIT](LICENSE) — use it however you like.
