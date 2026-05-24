//! App configuration: read `config.toml`, and open it for editing from the tray.

/// App config, read from `config.toml` (gitignored). The v1.1 settings UI will
/// write this same file. The key may instead come from the GROQ_API_KEY env var.
#[derive(serde::Deserialize)]
struct Config {
    groq_api_key: Option<String>,
    model: Option<String>,
    /// "handsfree" (default) or "toggle".
    mode: Option<String>,
}

/// Returns (api_key, model, handsfree): config.toml if present, else env var / default.
pub fn load_config() -> (Option<String>, String, bool) {
    let default_model = "whisper-large-v3-turbo".to_string();
    if let Ok(text) = std::fs::read_to_string("config.toml") {
        match toml::from_str::<Config>(&text) {
            Ok(c) => {
                let key = c
                    .groq_api_key
                    .or_else(|| std::env::var("GROQ_API_KEY").ok());
                let handsfree = c.mode.as_deref() != Some("toggle"); // default: hands-free
                return (key, c.model.unwrap_or(default_model), handsfree);
            }
            Err(e) => eprintln!("config.toml parse error: {e}"),
        }
    }
    (std::env::var("GROQ_API_KEY").ok(), default_model, true)
}

/// Open config.toml in the OS default handler (the "Settings" tray item).
/// On first run config.toml doesn't exist yet, so seed it from the committed
/// config.toml.example template — otherwise the click is a silent no-op.
pub fn open_config() {
    let dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let path = dir.join("config.toml");
    if !path.exists() {
        let example = dir.join("config.toml.example");
        if let Err(e) = std::fs::copy(&example, &path) {
            eprintln!("couldn't create config.toml from config.toml.example: {e}");
        }
    }
    if let Some(p) = path.to_str() {
        let _ = std::process::Command::new("explorer.exe").arg(p).spawn();
    }
}
