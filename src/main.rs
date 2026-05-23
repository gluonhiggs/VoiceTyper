use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use tao::event_loop::{ControlFlow, EventLoop};

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_LCONTROL, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RWIN,
    VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

/// Messages from the keyboard hook to the worker thread.
enum HookMsg {
    Toggle,
}

// The WH_KEYBOARD_LL callback is a plain `extern "system" fn`, so it can't
// capture state. It reaches the rest of the app through these process-lifetime
// statics. No GC, nothing to keep alive but the HHOOK (see main).
static HOOK_TX: OnceLock<Sender<HookMsg>> = OnceLock::new();
static CTRL_DOWN: AtomicBool = AtomicBool::new(false);
static WIN_DOWN: AtomicBool = AtomicBool::new(false);
// True while Ctrl+Win is held, so we fire once per chord, not on key-repeat.
static COMBO_ACTIVE: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // Track Ctrl/Win from the event stream rather than GetAsyncKeyState:
        // inside an LL hook the async state for the *current* key may not be
        // updated yet, which would miss the chord.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let vk = kb.vkCode;
        let msg = wparam.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;

        let is_ctrl = vk == VK_LCONTROL.0 as u32 || vk == VK_RCONTROL.0 as u32;
        let is_win = vk == VK_LWIN.0 as u32 || vk == VK_RWIN.0 as u32;

        if is_ctrl && is_down {
            CTRL_DOWN.store(true, Ordering::SeqCst);
        } else if is_ctrl && is_up {
            CTRL_DOWN.store(false, Ordering::SeqCst);
        }
        if is_win && is_down {
            WIN_DOWN.store(true, Ordering::SeqCst);
        } else if is_win && is_up {
            WIN_DOWN.store(false, Ordering::SeqCst);
        }

        let chord = CTRL_DOWN.load(Ordering::SeqCst) && WIN_DOWN.load(Ordering::SeqCst);
        if chord {
            // Fire once when the chord first closes; re-arms on release below.
            if !COMBO_ACTIVE.swap(true, Ordering::SeqCst) {
                if let Some(tx) = HOOK_TX.get() {
                    let _ = tx.send(HookMsg::Toggle);
                }
            }
        } else {
            COMBO_ACTIVE.store(false, Ordering::SeqCst);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn main() {
    // hook (producer) -> worker (consumer)
    let (tx, rx) = mpsc::channel::<HookMsg>();
    HOOK_TX.set(tx).ok();

    // Worker thread: owns the mic stream. Toggle starts capture into a shared
    // buffer; toggle again stops it and writes out.wav. The cpal Stream is !Send,
    // so it must be created and dropped here, never moved across threads.
    thread::spawn(move || {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .expect("no default input device (microphone)");
        let supported = device
            .default_input_config()
            .expect("no default input config");
        let sample_rate = supported.sample_rate();
        let channels = supported.channels();
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        // Captured samples, normalized to f32 in [-1, 1]. Shared with the cpal
        // callback thread.
        let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let mut stream: Option<cpal::Stream> = None;

        // Groq config: read from a gitignored config.toml (the eventual settings
        // button writes this same file), falling back to the GROQ_API_KEY env var.
        let (api_key, model) = load_config();

        while let Ok(HookMsg::Toggle) = rx.recv() {
            if stream.is_none() {
                buffer.lock().unwrap().clear();
                let buf = buffer.clone();
                let err_fn = |e| eprintln!("cpal stream error: {e}");
                let s = match sample_format {
                    cpal::SampleFormat::F32 => device.build_input_stream(
                        &config,
                        move |data: &[f32], _| buf.lock().unwrap().extend_from_slice(data),
                        err_fn,
                        None,
                    ),
                    cpal::SampleFormat::I16 => device.build_input_stream(
                        &config,
                        move |data: &[i16], _| {
                            let mut b = buf.lock().unwrap();
                            b.extend(data.iter().map(|&s| s as f32 / i16::MAX as f32));
                        },
                        err_fn,
                        None,
                    ),
                    cpal::SampleFormat::U16 => device.build_input_stream(
                        &config,
                        move |data: &[u16], _| {
                            let mut b = buf.lock().unwrap();
                            b.extend(data.iter().map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0));
                        },
                        err_fn,
                        None,
                    ),
                    other => panic!("unsupported sample format: {other:?}"),
                }
                .expect("failed to build input stream");
                s.play().expect("failed to start stream");
                stream = Some(s);
                println!("TOGGLE -> RECORDING ({sample_rate} Hz, {channels} ch)");
            } else {
                stream = None; // dropping the stream stops capture
                let samples = buffer.lock().unwrap().clone();
                let secs = samples.len() as f32 / (sample_rate as f32 * channels as f32);
                write_wav("out.wav", &samples, sample_rate, channels);
                println!(
                    "TOGGLE -> idle: wrote out.wav ({} samples, {:.1}s)",
                    samples.len(),
                    secs
                );
                match &api_key {
                    Some(key) => {
                        println!("transcribing via Groq ({model})...");
                        let t0 = std::time::Instant::now();
                        match transcribe("out.wav", key, &model) {
                            Ok(text) => {
                                println!(
                                    "\n===== TRANSCRIPT ({:.1}s) =====\n{text}\n==============================\n",
                                    t0.elapsed().as_secs_f32()
                                );
                                if let Err(e) = inject(&text) {
                                    eprintln!("inject failed: {e}");
                                }
                            }
                            Err(e) => eprintln!("transcription failed: {e}"),
                        }
                    }
                    None => {
                        eprintln!("GROQ_API_KEY not set — skipping transcription. Set it and re-run.")
                    }
                }
            }
        }
    });

    // Install the global low-level keyboard hook. The returned HHOOK is kept for
    // the process lifetime (we never unhook), so the hook stays installed.
    unsafe {
        let hmod = GetModuleHandleW(PCWSTR::null()).expect("GetModuleHandleW failed");
        let hinstance = HINSTANCE(hmod.0);
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(hinstance), 0)
            .expect("SetWindowsHookExW failed");
    }

    // Tray icon with a Quit item.
    let menu = Menu::new();
    let quit = MenuItem::new("Quit VoiceTyper", true, None);
    menu.append(&quit).unwrap();

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("VoiceTyper (spike)")
        .with_icon(make_icon())
        .build()
        .unwrap();

    let menu_rx = MenuEvent::receiver();

    // tao event loop: drives the Win32 message pump the LL hook depends on.
    let event_loop = EventLoop::new();
    event_loop.run(move |_event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Ok(ev) = menu_rx.try_recv() {
            if ev.id == *quit.id() {
                *control_flow = ControlFlow::Exit;
            }
        }
    });
}

/// POST a WAV file to Groq's Whisper endpoint and return the transcript text.
fn transcribe(path: &str, api_key: &str, model: &str) -> Result<String, Box<dyn std::error::Error>> {
    let form = reqwest::blocking::multipart::Form::new()
        .file("file", path)?
        .text("model", model.to_string())
        .text("response_format", "text") // plain-text body, no JSON to parse
        .text("language", "en")
        .text("temperature", "0");
    let resp = reqwest::blocking::Client::new()
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .bearer_auth(api_key)
        .multipart(form)
        .send()?;
    let status = resp.status();
    let body = resp.text()?;
    if !status.is_success() {
        return Err(format!("Groq API {status}: {body}").into());
    }
    Ok(body.trim().to_string())
}

/// Paste `text` into the focused window: snapshot the clipboard, set our text,
/// send Ctrl+V, then restore the old clipboard. Text-only restore for now.
fn inject(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    if text.is_empty() {
        return Ok(());
    }
    let mut clipboard = arboard::Clipboard::new()?;
    let previous = clipboard.get_text().ok();
    clipboard.set_text(text.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(30)); // let the clipboard settle
    wait_modifiers_released(); // avoid the held Win key turning Ctrl+V into Win+V
    send_ctrl_v();
    std::thread::sleep(std::time::Duration::from_millis(150)); // let the target app paste
    if let Some(prev) = previous {
        let _ = clipboard.set_text(prev);
    }
    Ok(())
}

/// Spin (briefly) until Ctrl/Win/Alt/Shift are all up, so the synthetic Ctrl+V
/// isn't poisoned by a still-held modifier (e.g. Win+V opens clipboard history).
fn wait_modifiers_released() {
    let down = |vk: VIRTUAL_KEY| unsafe { (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0 };
    for _ in 0..50 {
        if !down(VK_CONTROL) && !down(VK_LWIN) && !down(VK_RWIN) && !down(VK_MENU) && !down(VK_SHIFT)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Synthesize a Ctrl+V keystroke via SendInput.
fn send_ctrl_v() {
    const VK_V: VIRTUAL_KEY = VIRTUAL_KEY(0x56);
    let inputs = [
        key_event(VK_CONTROL, false),
        key_event(VK_V, false),
        key_event(VK_V, true),
        key_event(VK_CONTROL, true),
    ];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn key_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Write f32 samples (interleaved, [-1,1]) to a 16-bit PCM WAV.
fn write_wav(path: &str, samples: &[f32], sample_rate: u32, channels: u16) {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        writer.write_sample(v).expect("write sample");
    }
    writer.finalize().expect("finalize wav");
}

fn make_icon() -> Icon {
    // 16x16 solid blue square — avoids needing an .ico file for the spike.
    let (w, h) = (16u32, 16u32);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        rgba.extend_from_slice(&[0x2e, 0x7d, 0xff, 0xff]);
    }
    Icon::from_rgba(rgba, w, h).expect("icon from_rgba")
}

/// App config, read from `config.toml` (gitignored). The v1.1 settings UI will
/// write this same file. The key may instead come from the GROQ_API_KEY env var.
#[derive(serde::Deserialize)]
struct Config {
    groq_api_key: Option<String>,
    model: Option<String>,
}

/// Returns (api_key, model): config.toml if present, else env var / default.
fn load_config() -> (Option<String>, String) {
    let default_model = "whisper-large-v3-turbo".to_string();
    if let Ok(text) = std::fs::read_to_string("config.toml") {
        match toml::from_str::<Config>(&text) {
            Ok(c) => {
                let key = c.groq_api_key.or_else(|| std::env::var("GROQ_API_KEY").ok());
                return (key, c.model.unwrap_or(default_model));
            }
            Err(e) => eprintln!("config.toml parse error: {e}"),
        }
    }
    (std::env::var("GROQ_API_KEY").ok(), default_model)
}
