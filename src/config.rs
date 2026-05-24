//! App configuration: locate, read, and write `config.toml` in the per-user
//! Windows config folder (`%APPDATA%\VoiceTyper\config.toml`).
//!
//! Why a fixed per-user path and not the working directory: an installed app is
//! launched from arbitrary places (Start Menu, Program Files), so a cwd-relative
//! `config.toml` wouldn't be found. On first run we migrate a legacy repo-root
//! `config.toml` here so an existing key survives the move to the installed layout.

use std::path::PathBuf;

/// App config shape. The settings window writes it; the key may instead come
/// from the GROQ_API_KEY env var.
#[derive(serde::Deserialize)]
struct Config {
    groq_api_key: Option<String>,
    model: Option<String>,
    /// "handsfree" (default) or "toggle".
    mode: Option<String>,
}

const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";

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

/// Parse config text into (api_key, model, handsfree). Total: a missing/blank key
/// becomes None, a corrupt file falls back to defaults. Env fallback is the caller's job.
fn parse_config(text: &str) -> (Option<String>, String, bool) {
    match toml::from_str::<Config>(text) {
        Ok(c) => {
            let key = c.groq_api_key.filter(|k| !k.trim().is_empty());
            let handsfree = c.mode.as_deref() != Some("toggle"); // default: hands-free
            (key, c.model.unwrap_or_else(|| DEFAULT_MODEL.to_string()), handsfree)
        }
        Err(_) => (None, DEFAULT_MODEL.to_string(), true),
    }
}

/// Set `groq_api_key` and `mode` in the config text, preserving any other keys,
/// comments, and formatting (toml_edit). Returns the new file contents.
fn render_config(existing: &str, api_key: &str, mode: &str) -> String {
    let mut doc = existing
        .parse::<toml_edit::DocumentMut>()
        .unwrap_or_default();
    doc["groq_api_key"] = toml_edit::value(api_key);
    doc["mode"] = toml_edit::value(mode);
    if doc.get("model").is_none() {
        doc["model"] = toml_edit::value(DEFAULT_MODEL);
    }
    doc.to_string()
}

/// Returns (api_key, model, handsfree): the per-user config.toml (migrating a
/// legacy one first), then the GROQ_API_KEY env var, then defaults.
pub fn load_config() -> (Option<String>, String, bool) {
    let path = config_path();
    migrate_legacy(&path);
    let (key, model, handsfree) = std::fs::read_to_string(&path)
        .map(|t| parse_config(&t))
        .unwrap_or_else(|_| (None, DEFAULT_MODEL.to_string(), true));
    let key = key.or_else(|| std::env::var("GROQ_API_KEY").ok());
    (key, model, handsfree)
}

/// Write `api_key` + `mode` to the per-user config.toml, preserving other keys and
/// comments. Creates the folder/file as needed. Used by the settings window.
pub fn save_config(api_key: &str, mode: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, render_config(&existing, api_key, mode))?;
    Ok(())
}

/// Open the config file in the OS default handler (fallback "edit raw" path; the
/// settings window is the primary editor). Seeds a default file if none exists.
pub fn open_config() {
    let path = config_path();
    if !path.exists() {
        let _ = save_config("", "handsfree");
    }
    if let Some(p) = path.to_str() {
        let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_config() {
        let (key, model, hf) = parse_config("groq_api_key = \"sk-x\"\nmodel = \"m\"\nmode = \"toggle\"\n");
        assert_eq!(key.as_deref(), Some("sk-x"));
        assert_eq!(model, "m");
        assert!(!hf); // toggle
    }

    #[test]
    fn blank_key_becomes_none() {
        let (key, _, _) = parse_config("groq_api_key = \"\"\nmode = \"handsfree\"\n");
        assert_eq!(key, None);
    }

    #[test]
    fn missing_mode_defaults_to_handsfree() {
        let (_, _, hf) = parse_config("groq_api_key = \"k\"\n");
        assert!(hf);
    }

    #[test]
    fn missing_model_uses_default() {
        let (_, model, _) = parse_config("mode = \"toggle\"\n");
        assert_eq!(model, DEFAULT_MODEL);
    }

    #[test]
    fn corrupt_config_falls_back_to_defaults() {
        let (key, model, hf) = parse_config("not = valid = toml [[[");
        assert_eq!(key, None);
        assert_eq!(model, DEFAULT_MODEL);
        assert!(hf);
    }

    #[test]
    fn render_then_parse_roundtrips() {
        let text = render_config("", "sk-new", "toggle");
        let (key, _, hf) = parse_config(&text);
        assert_eq!(key.as_deref(), Some("sk-new"));
        assert!(!hf);
    }

    #[test]
    fn render_preserves_existing_model_and_comments() {
        let existing = "# my config\nmodel = \"custom-model\"\n";
        let out = render_config(existing, "k", "handsfree");
        assert!(out.contains("# my config")); // comment preserved
        let (key, model, _) = parse_config(&out);
        assert_eq!(key.as_deref(), Some("k"));
        assert_eq!(model, "custom-model"); // not clobbered
    }
}
