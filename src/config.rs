//! App configuration: locate, read, and write `config.toml` in the per-user
//! Windows config folder (`%APPDATA%\VoiceTyper\config.toml`).
//!
//! Why a fixed per-user path and not the working directory: an installed app is
//! launched from arbitrary places (Start Menu, Program Files), so a cwd-relative
//! `config.toml` wouldn't be found. On first run we migrate a legacy repo-root
//! `config.toml` here so an existing key survives the move to the installed layout.

use std::path::PathBuf;
use std::time::Duration;

/// On-disk config shape (what `config.toml` literally contains). The settings
/// window writes it; the key may instead come from the GROQ_API_KEY env var.
#[derive(serde::Deserialize)]
struct RawConfig {
    groq_api_key: Option<String>,
    model: Option<String>,
    /// "handsfree" (default) or "toggle".
    mode: Option<String>,
    /// Hands-free silence (seconds) that ends a chunk. Clamped on read.
    silence_timeout_secs: Option<f32>,
}

/// Resolved, validated config the rest of the app runs on.
pub struct Config {
    pub api_key: Option<String>,
    pub model: String,
    pub handsfree: bool,
    /// Silence after speech that cuts a hands-free chunk (the "Silence timeout").
    pub silence_timeout: Duration,
}

const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
/// Default hands-free Silence timeout, and the range the UI/parser clamp to.
/// 2.5s suits a slow, thinking speaker without fragmenting speech; the floor
/// keeps chunks from cutting mid-word, the ceiling keeps the app from feeling frozen.
pub const DEFAULT_SILENCE_SECS: f32 = 2.5;
pub const MIN_SILENCE_SECS: f32 = 0.5;
pub const MAX_SILENCE_SECS: f32 = 5.0;

impl Config {
    /// All-defaults config (no key) — used when the file is missing or corrupt.
    fn defaults() -> Config {
        Config {
            api_key: None,
            model: DEFAULT_MODEL.to_string(),
            handsfree: true,
            silence_timeout: Duration::from_secs_f32(DEFAULT_SILENCE_SECS),
        }
    }
}

/// The real config path: `%APPDATA%\VoiceTyper\config.toml` on Windows.
/// Falls back to a cwd-relative path only if the OS config dir can't be resolved.
pub fn config_path() -> PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.config_dir().join("VoiceTyper").join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("config.toml"))
}

/// Migrate a legacy repo-root `config.toml` into the per-user path on first run,
/// so an existing key isn't lost. No-op if the real config already exists, there's
/// nothing to migrate, or the resolver fell back to the same relative path.
fn migrate_legacy(real: &PathBuf) {
    if real.exists() {
        return;
    }
    let legacy = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("config.toml");
    if legacy == *real || !legacy.exists() {
        return;
    }
    if let Some(parent) = real.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::copy(&legacy, real) {
        Ok(_) => eprintln!("migrated config.toml -> {}", real.display()),
        Err(e) => eprintln!("config migration failed ({e}); still reading {}", legacy.display()),
    }
}

/// Clamp a silence-timeout value to the supported range, so a corrupt/extreme
/// file value can't make the app feel frozen (too high) or cut mid-word (too low).
fn clamp_silence(secs: f32) -> f32 {
    secs.clamp(MIN_SILENCE_SECS, MAX_SILENCE_SECS)
}

/// Parse config text into a `Config`: a missing/blank key becomes None, a corrupt
/// file falls back to defaults, the silence timeout is clamped. Env fallback is
/// the caller's job.
fn parse_config(text: &str) -> Config {
    match toml::from_str::<RawConfig>(text) {
        Ok(c) => {
            let secs = clamp_silence(c.silence_timeout_secs.unwrap_or(DEFAULT_SILENCE_SECS));
            Config {
                api_key: c.groq_api_key.filter(|k| !k.trim().is_empty()),
                model: c.model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
                handsfree: c.mode.as_deref() != Some("toggle"), // default: hands-free
                silence_timeout: Duration::from_secs_f32(secs),
            }
        }
        Err(_) => Config::defaults(),
    }
}

/// Set `groq_api_key`, `mode`, and `silence_timeout_secs` in the config text,
/// preserving any other keys, comments, and formatting (toml_edit). Returns the
/// new file contents.
fn render_config(existing: &str, api_key: &str, mode: &str, silence_secs: f32) -> String {
    let mut doc = existing
        .parse::<toml_edit::DocumentMut>()
        .unwrap_or_default();
    doc["groq_api_key"] = toml_edit::value(api_key);
    doc["mode"] = toml_edit::value(mode);
    doc["silence_timeout_secs"] = toml_edit::value(clamp_silence(silence_secs) as f64);
    if doc.get("model").is_none() {
        doc["model"] = toml_edit::value(DEFAULT_MODEL);
    }
    doc.to_string()
}

/// Load the resolved `Config`: the per-user config.toml (migrating a legacy one
/// first), then the GROQ_API_KEY env var as a key fallback, then defaults.
pub fn load_config() -> Config {
    let path = config_path();
    migrate_legacy(&path);
    let mut config = std::fs::read_to_string(&path)
        .map(|t| parse_config(&t))
        .unwrap_or_else(|_| Config::defaults());
    config.api_key = config.api_key.or_else(|| std::env::var("GROQ_API_KEY").ok());
    config
}

/// Write `api_key` + `mode` + `silence_secs` to the per-user config.toml, preserving
/// other keys and comments. Creates the folder/file as needed. Used by the settings window.
pub fn save_config(
    api_key: &str,
    mode: &str,
    silence_secs: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, render_config(&existing, api_key, mode, silence_secs))?;
    Ok(())
}

/// Open the config file in the OS default handler (fallback "edit raw" path; the
/// settings window is the primary editor). Seeds a default file if none exists.
pub fn open_config() {
    let path = config_path();
    if !path.exists() {
        let _ = save_config("", "handsfree", DEFAULT_SILENCE_SECS);
    }
    if let Some(p) = path.to_str() {
        let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(c: &Config) -> f32 {
        c.silence_timeout.as_secs_f32()
    }

    #[test]
    fn parse_full_config() {
        let c = parse_config("groq_api_key = \"sk-x\"\nmodel = \"m\"\nmode = \"toggle\"\n");
        assert_eq!(c.api_key.as_deref(), Some("sk-x"));
        assert_eq!(c.model, "m");
        assert!(!c.handsfree); // toggle
    }

    #[test]
    fn blank_key_becomes_none() {
        let c = parse_config("groq_api_key = \"\"\nmode = \"handsfree\"\n");
        assert_eq!(c.api_key, None);
    }

    #[test]
    fn missing_mode_defaults_to_handsfree() {
        assert!(parse_config("groq_api_key = \"k\"\n").handsfree);
    }

    #[test]
    fn missing_model_uses_default() {
        assert_eq!(parse_config("mode = \"toggle\"\n").model, DEFAULT_MODEL);
    }

    #[test]
    fn missing_silence_timeout_uses_default() {
        assert_eq!(secs(&parse_config("mode = \"handsfree\"\n")), DEFAULT_SILENCE_SECS);
    }

    #[test]
    fn silence_timeout_is_clamped() {
        assert_eq!(secs(&parse_config("silence_timeout_secs = 30.0\n")), MAX_SILENCE_SECS);
        assert_eq!(secs(&parse_config("silence_timeout_secs = 0.01\n")), MIN_SILENCE_SECS);
        assert_eq!(secs(&parse_config("silence_timeout_secs = 1.5\n")), 1.5); // in range, untouched
    }

    #[test]
    fn corrupt_config_falls_back_to_defaults() {
        let c = parse_config("not = valid = toml [[[");
        assert_eq!(c.api_key, None);
        assert_eq!(c.model, DEFAULT_MODEL);
        assert!(c.handsfree);
        assert_eq!(secs(&c), DEFAULT_SILENCE_SECS);
    }

    #[test]
    fn render_then_parse_roundtrips() {
        let text = render_config("", "sk-new", "toggle", 1.5);
        let c = parse_config(&text);
        assert_eq!(c.api_key.as_deref(), Some("sk-new"));
        assert!(!c.handsfree);
        assert_eq!(secs(&c), 1.5);
    }

    #[test]
    fn render_clamps_out_of_range_silence() {
        let c = parse_config(&render_config("", "k", "handsfree", 99.0));
        assert_eq!(secs(&c), MAX_SILENCE_SECS);
    }

    #[test]
    fn render_preserves_existing_model_and_comments() {
        let existing = "# my config\nmodel = \"custom-model\"\n";
        let out = render_config(existing, "k", "handsfree", DEFAULT_SILENCE_SECS);
        assert!(out.contains("# my config")); // comment preserved
        let c = parse_config(&out);
        assert_eq!(c.api_key.as_deref(), Some("k"));
        assert_eq!(c.model, "custom-model"); // not clobbered
    }
}
