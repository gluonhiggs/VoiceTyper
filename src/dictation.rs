//! Dictation sessions and the per-chunk pipeline.
//!
//! ```text
//! Ctrl+Win ──▶ run_*_session (PRODUCER, this thread)
//!                VAD cuts chunks ──enqueue──▶ [mpsc queue] ──▶ run_chunk_consumer
//!                keeps draining mic                              (CONSUMER thread)
//!                never blocks                                    transcribe + inject
//!                                                                in spoken order
//! ```
//! Splitting producer from consumer is what keeps the capture loop from going
//! deaf during the ~1s Groq round-trip (which used to drop words).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tao::event_loop::EventLoopProxy;

use crate::audio::{encode_wav_16k_mono, rms, to_16k_mono};
use crate::hotkey::HookMsg;
use crate::inject::inject;
use crate::transcribe::transcribe_bytes;
use crate::tray::UiEvent;

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

/// Hands-free: VAD cuts chunks at pauses and types each as you go. Returns when
/// the user re-presses Ctrl+Win or after a long silence.
pub fn run_handsfree_session(
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
        let (api_key, model, proxy, depth) = (
            api_key.clone(),
            model.to_string(),
            proxy.clone(),
            depth.clone(),
        );
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

    drop(chunk_tx); // no more chunks -> consumer drains its queue and exits
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
pub fn run_toggle_session(
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
