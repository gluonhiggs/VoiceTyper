//! Settings window: a separate `voicetyper --settings` process so its egui event
//! loop never fights the tray's `tao` loop in the main process.
//!
//! Four fields the everyday user touches — Groq API key, mode, Silence timeout,
//! and language — written to `%APPDATA%\VoiceTyper\config.toml`. Model and the
//! VAD sensitivity stay in the file (rarely changed; see TODOS.md).

use eframe::egui;

use crate::config::{self, Language, MAX_SILENCE_SECS, MIN_SILENCE_SECS};

/// Brand blue, shared with the tray icon's idle color (tray.rs IDLE_RGBA).
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x7d, 0xff);

/// Launch the settings window as a separate process: re-run this exe with
/// `--settings` so its event loop never tangles with the tray's `tao` loop.
/// Falls back to opening the raw config file if the process can't spawn.
pub fn open_settings() {
    let launched = std::env::current_exe()
        .and_then(|exe| std::process::Command::new(exe).arg("--settings").spawn())
        .is_ok();
    if !launched {
        crate::config::open_config();
    }
}

/// Live form state, seeded from the current config and written back on Save.
struct SettingsApp {
    api_key: String,
    handsfree: bool,
    silence_secs: f32,
    language: Language,
    /// Set when a Save fails, shown in red under the fields.
    error: Option<String>,
}

impl SettingsApp {
    fn new(cfg: config::Config) -> Self {
        Self {
            api_key: cfg.api_key.unwrap_or_default(),
            handsfree: cfg.handsfree,
            silence_secs: cfg.silence_timeout.as_secs_f32(),
            language: cfg.language,
            error: None,
        }
    }

    /// Write the form to config, then close the window. The config layer clamps
    /// the timeout, so the slider's range is the only validation needed here.
    fn save(&mut self, ctx: &egui::Context) {
        let mode = if self.handsfree { "handsfree" } else { "toggle" };
        match config::save_config(self.api_key.trim(), mode, self.silence_secs, self.language) {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => self.error = Some(format!("Couldn't save: {e}")),
        }
    }
}

impl eframe::App for SettingsApp {
    // eframe 0.34 hands the root `Ui` directly (no margin/background), so we wrap
    // in a Frame for padding. `ui.ctx()` is how we send the close command.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::new()
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;

                ui.label("Groq API key");
                ui.add(
                    egui::TextEdit::singleline(&mut self.api_key)
                        .password(true) // masked so a screenshot can't leak the key
                        .hint_text("gsk_...")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(10.0);

                ui.label("Mode");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.handsfree, true, "Hands-free");
                    ui.radio_value(&mut self.handsfree, false, "Toggle");
                });
                ui.add_space(10.0);

                ui.label("Language");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.language, Language::English, "English");
                    ui.radio_value(&mut self.language, Language::Vietnamese, "Vietnamese");
                });
                ui.add_space(10.0);

                ui.label("Silence timeout");
                ui.add(
                    egui::Slider::new(&mut self.silence_secs, MIN_SILENCE_SECS..=MAX_SILENCE_SECS)
                        .suffix(" s")
                        .fixed_decimals(1),
                );
                ui.label(
                    egui::RichText::new(
                        "How long a pause ends a phrase. Applies on the next Ctrl+Win press.",
                    )
                    .weak()
                    .small(),
                );

                if let Some(msg) = &self.error {
                    ui.add_space(4.0);
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3b, 0x30), msg);
                }

                // Actions pinned bottom-right: accent Save, plain Close.
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let save =
                        egui::Button::new(egui::RichText::new("Save").color(egui::Color32::WHITE))
                            .fill(ACCENT)
                            .min_size(egui::vec2(80.0, 28.0));
                    if ui.add(save).clicked() {
                        self.save(ui.ctx());
                    }
                    ui.add_space(8.0);
                    if ui
                        .add(egui::Button::new("Close").min_size(egui::vec2(80.0, 28.0)))
                        .clicked()
                    {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
    }
}

/// Open the settings window, pre-filled from the current config. Blocks on the
/// egui event loop until the window closes, then returns (the process exits).
pub fn run() {
    let cfg = config::load_config();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 320.0])
            .with_resizable(false),
        centered: true, // open in the middle of the screen, not the top-left corner
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "Settings", // OS title bar; the tray already brands it "VoiceTyper"
        options,
        Box::new(move |_cc| Ok(Box::new(SettingsApp::new(cfg)))),
    ) {
        eprintln!("settings window failed to open: {e}");
    }
}
