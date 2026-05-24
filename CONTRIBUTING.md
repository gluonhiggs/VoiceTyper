# Contributing to VoiceTyper

Thanks for wanting to help. VoiceTyper is a small, native **Rust** Windows app
(tray + global hotkey + Groq Whisper). This guide gets you from a fresh clone to a
build, and explains what to do before you push or cut a release.

For *why* things are built the way they are, see [DESIGN.md](DESIGN.md).

---

## Dev environment (one-time setup)

VoiceTyper builds with the **GNU** Rust toolchain (no Visual Studio needed). Two pieces
are required, and the repo's `.cargo/config.toml` is already tuned for them:

1. **Rust, GNU host.** Install [rustup](https://rustup.rs) and use the GNU toolchain:
   ```powershell
   rustup toolchain install stable-x86_64-pc-windows-gnu
   rustup default stable-x86_64-pc-windows-gnu
   ```

2. **w64devkit on your PATH.** Download [w64devkit](https://github.com/skeeto/w64devkit/releases)
   (the x64 release), extract it (e.g. to `%USERPROFILE%\w64devkit`), and add its `bin`
   folder to your **PATH**. It supplies the `dlltool` + `as` that the `windows` crate needs
   to link — rustup's bundled toolchain alone can't.
   ```powershell
   # verify (in a fresh terminal):
   x86_64-w64-mingw32-gcc --version
   ```

3. **A free Groq API key** for testing — [console.groq.com](https://console.groq.com)
   → API Keys → Create. (See the README for where to paste it.)

> Why GNU and not MSVC? To avoid the multi-GB Visual Studio C++ Build Tools install.
> CI builds with MSVC (it's preinstalled on GitHub's runners) by overriding `RUSTFLAGS`,
> so both toolchains are supported — but local dev is set up for GNU. If you build with
> MSVC locally, prefix commands with `RUSTFLAGS="-C target-feature=+crt-static"` to
> override the repo's GNU link flag.

---

## Build & run

```powershell
cargo run                 # debug build: shows a console with per-chunk logs
cargo build --release     # optimized, console-free GUI build -> target\release\voicetyper.exe
```

On first run, set your Groq key: right-click the tray icon → **Settings**, paste the key,
**Save**. (Config lives at `%APPDATA%\VoiceTyper\config.toml`.)

---

## Test, lint, format

```powershell
cargo test                    # unit tests (config + audio DSP)
cargo clippy --all-targets    # lints — keep this clean (zero warnings)
cargo fmt                     # format
```

---

## Project layout

```
src/
  main.rs        glue: wires hotkey -> worker thread -> tray
  hotkey.rs      global Ctrl+Win low-level keyboard hook
  audio.rs       mic capture helpers + DSP (resample, WAV, RMS) — unit-tested
  dictation.rs   the session loops + VAD chunking (hands-free / toggle)
  transcribe.rs  Groq Whisper HTTP call
  inject.rs      types/pastes text into the focused window
  config.rs      %APPDATA% config load/save — unit-tested
  settings.rs    the egui settings window (runs as `voicetyper --settings`)
  tray.rs        tray icon + state colors
build.rs         supplies Win32 import stubs the GNU self-contained link misses
```

---

## Before you push

Run all three and make sure they're clean:

```powershell
cargo fmt
cargo clippy --all-targets    # 0 warnings
cargo test                    # all green
```

A feature isn't "done" when it compiles — it's done when the user-facing behavior works.
Build the release exe and actually dictate with it before pushing UI/pipeline changes.

---

## Releasing a new version (maintainers)

Releases are automated. One command bumps the version, tags it, and pushes — then GitHub
Actions builds the installer and attaches it to the GitHub Release. No hand-edited tags.

```powershell
cargo release patch --no-publish --execute   # 0.1.1 -> 0.1.2  (bug fixes)
cargo release minor --no-publish --execute   # 0.1.x -> 0.2.0  (features)
cargo release major --no-publish --execute   # 0.x   -> 1.0.0  (big / breaking)
```

(Drop `--execute` for a dry run that changes nothing.) `--no-publish` is belt-and-suspenders;
the crate is also marked `publish = false` in `Cargo.toml` (it's an app, not a crates.io
library), so it can never be published there.

What happens after the tag is pushed:
1. `.github/workflows/release.yml` runs on the `v*` tag.
2. It builds the release exe (MSVC + static CRT = self-contained), installs Inno Setup,
   and compiles `installer\voicetyper.iss` into `VoiceTyper-Setup-<version>.exe`.
3. The installer is attached to the GitHub Release. Watch the **Actions** tab on the first
   run; if it goes red, the usual suspect is the `choco install innosetup` step.

**Building the installer locally** (optional — CI does this for releases):
```powershell
cargo build --release
& "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\voicetyper.iss
# -> installer\Output\VoiceTyper-Setup-<version>.exe
```

---

## Gotchas

- **Kill the running app before rebuilding.** A running tray/exe locks
  `target\...\voicetyper.exe`, so `cargo build` silently fails to relink. Run
  `Stop-Process -Name voicetyper -Force` first.
- **Installing cargo tools fails with `cannot find -lgcc_eh`.** `cargo install` builds in a
  temp dir that doesn't see this repo's `.cargo/config.toml`, so the self-contained link
  flag is missing. Pass it via env:
  `RUSTFLAGS="-Clink-self-contained=yes" cargo install <tool>`.
- **The `-Clink-self-contained=yes` flag in `.cargo/config.toml` must stay in `[build]`**
  (not `[target.*]`), or build scripts fail to link on a clean build.
