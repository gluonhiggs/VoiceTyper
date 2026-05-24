use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};

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

// ── Tunables (energy VAD; tune by ear during testing) ─────────────────────────
/// RMS above this = speech. f32 samples in [-1,1]; silence ~0.001, speech ~0.02+.
const SPEECH_RMS: f32 = 0.012;
/// Silence after speech that ends an utterance (cut a chunk).
const CHUNK_GAP: Duration = Duration::from_millis(1500);
/// Silence that ends the whole session (like Windows ~10s).
const SESSION_GAP: Duration = Duration::from_secs(10);
/// Ignore chunks shorter than this (accidental blips).
const MIN_CHUNK_SECS: f32 = 0.3;
/// How often the session loop wakes to inspect audio.
const TICK: Duration = Duration::from_millis(100);
/// Audio kept just before speech crosses the threshold, so soft word onsets
/// (breathy/consonant starts below SPEECH_RMS) aren't clipped off the chunk.
const PREROLL: Duration = Duration::from_millis(250);
/// Warn (in logs) when this many chunks are queued for transcription — Groq is
/// falling behind, so injected text is lagging behind what you're saying.
const QUEUE_WARN: usize = 5;

/// Tray mic-glyph color: blue = idle, red = listening, amber = processing.
const IDLE_RGBA: [u8; 4] = [0x2e, 0x7d, 0xff, 0xff];
const LISTENING_RGBA: [u8; 4] = [0xff, 0x3b, 0x30, 0xff];
const PROCESSING_RGBA: [u8; 4] = [0xff, 0xa5, 0x00, 0xff];

/// Messages from the keyboard hook to the worker thread.
enum HookMsg {
    Toggle,
}

/// Worker -> UI-thread events (drives the tray icon state).
/// Listening = capturing mic, Processing = uploading/transcribing, Idle = done.
enum UiEvent {
    Listening,
    Processing,
    Idle,
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

    // Typed event loop so the worker can push tray-icon state changes to the UI
    // thread (the tray must be touched on the thread that owns it).
    let event_loop = EventLoopBuilder::<UiEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // Worker thread: owns the mic stream and the session loops.
    thread::spawn(move || {
        let host = cpal::default_host();
        let (api_key, model, handsfree) = load_config();
        // Shared with the cpal callback; each session drains it.
        let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));

        loop {
            // Block until Ctrl+Win starts a session.
            if rx.recv().is_err() {
                return; // channel closed
            }

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
            let config: cpal::StreamConfig = supported.into();

            buffer.lock().unwrap().clear();
            let stream = match build_stream(&device, &config, sample_format, buffer.clone()) {
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
            if handsfree {
                run_handsfree_session(&rx, &buffer, in_rate, channels, &api_key, &model, &proxy);
            } else {
                run_toggle_session(&rx, &buffer, in_rate, channels, &api_key, &model, &proxy);
            }
            let _ = proxy.send_event(UiEvent::Idle);

            drop(stream); // stops capture
        }
    });

    // Install the global low-level keyboard hook (kept for process lifetime).
    unsafe {
        let hmod = GetModuleHandleW(PCWSTR::null()).expect("GetModuleHandleW failed");
        let hinstance = HINSTANCE(hmod.0);
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(hinstance), 0)
            .expect("SetWindowsHookExW failed");
    }

    // Tray: Settings + Quit. The icon swaps idle<->listening on worker events.
    let menu = Menu::new();
    let settings = MenuItem::new("Settings (edit config.toml)", true, None);
    let quit = MenuItem::new("Quit VoiceTyper", true, None);
    menu.append(&settings).unwrap();
    menu.append(&quit).unwrap();

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("VoiceTyper")
        .with_icon(make_icon(IDLE_RGBA))
        .build()
        .unwrap();

    let menu_rx = MenuEvent::receiver();

    // tao event loop: pumps the Win32 messages the LL hook needs, swaps the tray
    // icon on worker state changes, and handles the tray menu.
    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        if let Event::UserEvent(ui) = event {
            let rgba = match ui {
                UiEvent::Listening => LISTENING_RGBA,
                UiEvent::Processing => PROCESSING_RGBA,
                UiEvent::Idle => IDLE_RGBA,
            };
            let _ = tray.set_icon(Some(make_icon(rgba)));
        }

        if let Ok(ev) = menu_rx.try_recv() {
            if ev.id == *settings.id() {
                open_config();
            } else if ev.id == *quit.id() {
                *control_flow = ControlFlow::Exit;
            }
        }
    });
}

/// Hands-free: VAD cuts chunks at pauses and types each as you go. Returns when
/// the user re-presses Ctrl+Win or after a long silence.
fn run_handsfree_session(
    rx: &Receiver<HookMsg>,
    buffer: &Arc<Mutex<Vec<f32>>>,
    in_rate: u32,
    channels: u16,
    api_key: &Option<String>,
    model: &str,
    proxy: &EventLoopProxy<UiEvent>,
) {
    println!("SESSION START — hands-free (Ctrl+Win to stop)");

    // Producer/consumer split. THIS loop (producer) only runs the VAD and hands
    // cut chunks to a queue, so it keeps draining the mic every TICK. A separate
    // consumer thread does the slow Groq round-trip + paste. Running process_chunk
    // inline here is what dropped words: the loop froze for ~1s per chunk, went
    // deaf, then swept everything spoken during the upload into one block and
    // misjudged it. The consumer drives the amber<->red icon per chunk.
    let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<f32>>();
    let depth = Arc::new(AtomicUsize::new(0)); // chunks queued + in flight (backlog visibility)
    let consumer = {
        let (api_key, model, proxy, depth) =
            (api_key.clone(), model.to_string(), proxy.clone(), depth.clone());
        thread::spawn(move || {
            run_chunk_consumer(chunk_rx, in_rate, channels, api_key, model, proxy, depth)
        })
    };

    // Pre-roll: a rolling buffer of the most recent pre-speech audio, prepended
    // on onset so soft word starts (below SPEECH_RMS) aren't clipped off.
    let preroll_cap = (PREROLL.as_secs_f32() * in_rate as f32 * channels as f32) as usize;
    let mut preroll: Vec<f32> = Vec::new();
    let mut segment: Vec<f32> = Vec::new();
    let mut had_speech = false;
    let mut silence = Duration::ZERO;

    loop {
        if let Ok(HookMsg::Toggle) = rx.try_recv() {
            if had_speech {
                enqueue_chunk(&chunk_tx, &depth, std::mem::take(&mut segment));
            }
            println!("SESSION END (Ctrl+Win)");
            break;
        }
        thread::sleep(TICK);

        let new = {
            let mut b = buffer.lock().unwrap();
            std::mem::take(&mut *b)
        };

        if !new.is_empty() && rms(&new) > SPEECH_RMS {
            if !had_speech {
                segment.append(&mut preroll); // prepend recent audio so the onset survives
            }
            had_speech = true;
            silence = Duration::ZERO;
            segment.extend_from_slice(&new);
        } else {
            silence += TICK;
            if had_speech {
                segment.extend_from_slice(&new); // keep trailing audio
            } else {
                preroll.extend_from_slice(&new); // not speaking yet: roll the pre-roll
                if preroll.len() > preroll_cap {
                    preroll.drain(..preroll.len() - preroll_cap);
                }
            }
        }

        if had_speech && silence >= CHUNK_GAP {
            enqueue_chunk(&chunk_tx, &depth, std::mem::take(&mut segment)); // hand off; never block
            had_speech = false;
        }

        if silence >= SESSION_GAP {
            println!("SESSION END (silence timeout)");
            break;
        }
    }

    drop(chunk_tx);          // no more chunks -> consumer drains its queue and exits
    let _ = consumer.join(); // wait so every queued chunk is transcribed + injected before Idle
}

/// Consumer thread: transcribe + inject queued chunks in spoken order, off the
/// capture loop so the VAD never goes deaf during the Groq round-trip. Serialized
/// (one consumer) because inject() touches the global clipboard. Flashes the icon
/// amber per chunk; the worker sends Idle once this thread joins.
fn run_chunk_consumer(
    chunks: Receiver<Vec<f32>>,
    in_rate: u32,
    channels: u16,
    api_key: Option<String>,
    model: String,
    proxy: EventLoopProxy<UiEvent>,
    depth: Arc<AtomicUsize>,
) {
    for samples in chunks {
        let _ = proxy.send_event(UiEvent::Processing);
        process_chunk(samples, in_rate, channels, &api_key, &model);
        depth.fetch_sub(1, Ordering::Relaxed); // chunk done, shrink the backlog gauge
        let _ = proxy.send_event(UiEvent::Listening);
    }
}

/// Hand a cut chunk to the consumer (never blocks) and bump the backlog gauge.
/// Warns in the log if the queue is deep, meaning Groq can't keep up and the
/// injected text is lagging behind speech.
fn enqueue_chunk(tx: &Sender<Vec<f32>>, depth: &Arc<AtomicUsize>, seg: Vec<f32>) {
    let queued = depth.fetch_add(1, Ordering::Relaxed) + 1;
    let _ = tx.send(seg);
    if queued > QUEUE_WARN {
        eprintln!("transcribe backlog: {queued} chunks queued — injects lagging behind speech");
    }
}

/// Toggle: record until the next Ctrl+Win, then transcribe the WHOLE utterance
/// as one Groq call (full context = best capitalization/punctuation).
fn run_toggle_session(
    rx: &Receiver<HookMsg>,
    buffer: &Arc<Mutex<Vec<f32>>>,
    in_rate: u32,
    channels: u16,
    api_key: &Option<String>,
    model: &str,
    proxy: &EventLoopProxy<UiEvent>,
) {
    println!("RECORDING (toggle) — Ctrl+Win to stop");
    loop {
        if let Ok(HookMsg::Toggle) = rx.try_recv() {
            break;
        }
        thread::sleep(TICK);
    }
    let all = {
        let mut b = buffer.lock().unwrap();
        std::mem::take(&mut *b)
    };
    // Amber during the ~1s upload+inject; caller flips to Idle right after we return.
    let _ = proxy.send_event(UiEvent::Processing);
    process_chunk(all, in_rate, channels, api_key, model);
    println!("SESSION END (toggle)");
}

/// Build a cpal input stream that appends normalized f32 samples to `buf`.
fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: cpal::SampleFormat,
    buf: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let err_fn = |e| eprintln!("cpal stream error: {e}");
    let stream = match fmt {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |d: &[f32], _| buf.lock().unwrap().extend_from_slice(d),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            config,
            move |d: &[i16], _| {
                let mut b = buf.lock().unwrap();
                b.extend(d.iter().map(|&s| s as f32 / i16::MAX as f32));
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_input_stream(
            config,
            move |d: &[u16], _| {
                let mut b = buf.lock().unwrap();
                b.extend(d.iter().map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0));
            },
            err_fn,
            None,
        )?,
        other => return Err(format!("unsupported sample format: {other:?}").into()),
    };
    Ok(stream)
}

/// One utterance: resample to 16kHz mono, encode WAV in memory, transcribe, inject.
fn process_chunk(
    samples: Vec<f32>,
    in_rate: u32,
    channels: u16,
    api_key: &Option<String>,
    model: &str,
) {
    let secs = samples.len() as f32 / (in_rate as f32 * channels as f32);
    if secs < MIN_CHUNK_SECS {
        return; // accidental blip
    }
    let Some(key) = api_key else {
        eprintln!("GROQ_API_KEY / config.toml not set — can't transcribe");
        return;
    };
    let mono16k = to_16k_mono(&samples, in_rate, channels);
    let wav = encode_wav_16k_mono(&mono16k);
    println!("chunk {:.1}s -> transcribing...", secs);
    let t0 = Instant::now();
    match transcribe_bytes(wav, key, model) {
        Ok(text) if !text.trim().is_empty() => {
            println!(">>> [{:.1}s] {text}", t0.elapsed().as_secs_f32());
            let out = format!("{text} "); // trailing space separates chunks within + across sessions
            if let Err(e) = inject(&out) {
                eprintln!("inject failed: {e}");
            }
        }
        Ok(_) => println!("(empty transcript — skipped)"),
        Err(e) => eprintln!("transcription failed: {e}"),
    }
}

/// RMS loudness of a sample block.
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Downmix to mono and resample to 16kHz by averaging windows (crude anti-alias).
fn to_16k_mono(samples: &[f32], in_rate: u32, channels: u16) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = if ch > 1 {
        samples
            .chunks(ch)
            .map(|f| f.iter().sum::<f32>() / ch as f32)
            .collect()
    } else {
        samples.to_vec()
    };
    if in_rate == 16_000 || mono.is_empty() {
        return mono;
    }
    let ratio = in_rate as f32 / 16_000.0;
    let out_len = (mono.len() as f32 / ratio) as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let start = (i as f32 * ratio) as usize;
        let end = (((i + 1) as f32 * ratio) as usize).min(mono.len()).max(start + 1);
        let slice = &mono[start..end.min(mono.len())];
        out.push(slice.iter().sum::<f32>() / slice.len() as f32);
    }
    out
}

/// Encode f32 mono samples ([-1,1], 16kHz) as a 16-bit PCM WAV in memory.
fn encode_wav_16k_mono(samples: &[f32]) -> Vec<u8> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::<u8>::new());
    {
        let mut w = hound::WavWriter::new(&mut cursor, spec).expect("wav writer");
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            w.write_sample(v).expect("write sample");
        }
        w.finalize().expect("finalize wav");
    }
    cursor.into_inner()
}

/// POST WAV bytes to Groq's Whisper endpoint and return the transcript text.
fn transcribe_bytes(
    audio: Vec<u8>,
    api_key: &str,
    model: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let part = reqwest::blocking::multipart::Part::bytes(audio)
        .file_name("audio.wav")
        .mime_str("audio/wav")?;
    let form = reqwest::blocking::multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("response_format", "text")
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
    thread::sleep(Duration::from_millis(30)); // let the clipboard settle
    wait_modifiers_released(); // avoid a held Win key turning Ctrl+V into Win+V
    send_ctrl_v();
    thread::sleep(Duration::from_millis(150)); // let the target app paste
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
        thread::sleep(Duration::from_millis(10));
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

/// Draw a 32x32 microphone glyph in `rgba` on a transparent background.
/// Color encodes state (IDLE_RGBA = idle, LISTENING_RGBA = listening).
///
///   ▟▙     capsule (rounded mic body)
///  (   )   cradle  (U-ring around the lower body)
///    |     stem
///   ───    base
fn make_icon(rgba: [u8; 4]) -> Icon {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4]; // transparent background
    let cx = 16.0_f32;
    for y in 0..S {
        for x in 0..S {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let dx = fx - cx;
            // Mic capsule: rounded vertical body (segment (16,9)-(16,15), r=5).
            let cyy = fy.clamp(9.0, 15.0);
            let head = (dx * dx + (fy - cyy) * (fy - cyy)).sqrt() <= 5.0;
            // Cradle: lower half of a ring (r≈8) around (16,15).
            let dr = (dx * dx + (fy - 15.0) * (fy - 15.0)).sqrt();
            let cradle = (15.0..=23.0).contains(&fy) && (dr - 8.0).abs() <= 1.5;
            // Stand: stem + base.
            let stem = dx.abs() <= 1.5 && (22.0..=27.0).contains(&fy);
            let base = dx.abs() <= 6.0 && (26.0..=28.0).contains(&fy);
            if head || cradle || stem || base {
                let i = (y * S + x) * 4;
                px[i..i + 4].copy_from_slice(&rgba);
            }
        }
    }
    Icon::from_rgba(px, S as u32, S as u32).expect("icon from_rgba")
}

/// Open config.toml in the OS default handler (the "Settings" tray item).
/// On first run config.toml doesn't exist yet, so seed it from the committed
/// config.toml.example template — otherwise the click is a silent no-op.
fn open_config() {
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

/// App config, read from `config.toml` (gitignored). The v1.1 settings UI will
/// write this same file. The key may instead come from the GROQ_API_KEY env var.
#[derive(serde::Deserialize)]
struct Config {
    groq_api_key: Option<String>,
    model: Option<String>,
    /// "handsfree" (default) or "toggle".
    mode: Option<String>,
}

/// Returns (api_key, model): config.toml if present, else env var / default.
fn load_config() -> (Option<String>, String, bool) {
    let default_model = "whisper-large-v3-turbo".to_string();
    if let Ok(text) = std::fs::read_to_string("config.toml") {
        match toml::from_str::<Config>(&text) {
            Ok(c) => {
                let key = c.groq_api_key.or_else(|| std::env::var("GROQ_API_KEY").ok());
                let handsfree = c.mode.as_deref() != Some("toggle"); // default: hands-free
                return (key, c.model.unwrap_or(default_model), handsfree);
            }
            Err(e) => eprintln!("config.toml parse error: {e}"),
        }
    }
    (std::env::var("GROQ_API_KEY").ok(), default_model, true)
}
