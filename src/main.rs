// Release builds are a GUI app (no console window). Debug keeps the console so
// dev `cargo run` still shows the per-chunk logs. Without this, the tray app pops
// an empty terminal window on launch — ugly for end users.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! VoiceTyper: press Ctrl+Win, speak, clean text lands in the focused app.
//! Cloud-only (Groq Whisper), tray-only, Windows.
//!
//! main() is the glue: it wires the keyboard hook -> worker thread -> tray icon.
//! The real work lives in the modules below.

mod audio;
mod config;
mod dictation;
mod hotkey;
mod inject;
mod settings;
mod transcribe;
mod tray;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};

use tray_icon::TrayIconBuilder;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};

use crate::config::load_config;
use crate::hotkey::HookMsg;
use crate::tray::UiEvent;

fn main() {
    // Launched as the settings window (from the tray): run that and exit. It's a
    // separate process so its native message loop never tangles with the tray's.
    if std::env::args().skip(1).any(|a| a == "--settings") {
        settings::run();
        return;
    }

    // hook (producer) -> worker (consumer)
    let (tx, rx) = mpsc::channel::<HookMsg>();

    // Typed event loop so the worker can push tray-icon state changes to the UI
    // thread (the tray must be touched on the thread that owns it).
    let event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Worker thread: owns the mic stream and the session loops.
    thread::spawn(move || {
        let host = cpal::default_host();
        // Shared with the cpal callback; each session drains it.
        let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

        loop {
            // Block until Ctrl+Win starts a session.
            if rx.recv().is_err() {
                return; // channel closed
            }

            // Re-read config on every press so a Save in the settings window applies
            // on the next Ctrl+Win — no need to quit and relaunch the tray app.
            let cfg = load_config();

            // Acquire the mic PER SESSION. A transient failure (mic unplugged,
            // device busy) logs and waits for the next press, rather than the
            // worker thread dying silently and recording never working again.
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    eprintln!("no microphone found — connect one and press Ctrl+Win again");
                    continue;
                }
            };
            let supported = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("mic config error: {e} — press Ctrl+Win to retry");
                    continue;
                }
            };
            let in_rate = supported.sample_rate();
            let channels = supported.channels();
            let sample_format = supported.sample_format();
            let stream_config: cpal::StreamConfig = supported.into();

            buffer.lock().unwrap().clear();
            let stream =
                match audio::build_stream(&device, &stream_config, sample_format, buffer.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("couldn't open mic stream: {e} — press Ctrl+Win to retry");
                        continue;
                    }
                };
            if let Err(e) = stream.play() {
                eprintln!("couldn't start mic: {e} — press Ctrl+Win to retry");
                continue;
            }

            let _ = proxy.send_event(UiEvent::Listening);
            let ctx = dictation::SessionCtx {
                rx: &rx,
                buffer: &buffer,
                in_rate,
                channels,
                api_key: &cfg.api_key,
                model: &cfg.model,
                proxy: &proxy,
                language: cfg.language,
            };
            if cfg.handsfree {
                dictation::run_handsfree_session(&ctx, cfg.silence_timeout);
            } else {
                dictation::run_toggle_session(&ctx);
            }
            let _ = proxy.send_event(UiEvent::Idle);

            drop(stream); // stops capture
        }
    });

    // Install the global low-level keyboard hook (kept for process lifetime).
    hotkey::install(tx);

    // Tray: Settings + Quit. The icon swaps idle<->listening<->processing on worker events.
    let menu = Menu::new();
    let settings = MenuItem::new("Settings", true, None);
    let quit = MenuItem::new("Quit VoiceTyper", true, None);
    menu.append(&settings).unwrap();
    menu.append(&quit).unwrap();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("VoiceTyper")
        .with_icon(tray::idle_icon())
        .build()
        .unwrap();

    let menu_rx = MenuEvent::receiver();

    // tao event loop: pumps the Win32 messages the LL hook needs, swaps the tray
    // icon on worker state changes, and handles the tray menu.
    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Event::UserEvent(ui) = event {
            let _ = tray.set_icon(Some(tray::icon_for_event(&ui)));
        }

        if let Ok(ev) = menu_rx.try_recv() {
            if ev.id == *settings.id() {
                settings::open_settings();
            } else if ev.id == *quit.id() {
                *control_flow = ControlFlow::Exit;
            }
        }
    });
}
