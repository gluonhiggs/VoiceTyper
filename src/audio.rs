//! Mic capture and the DSP helpers: RMS loudness, downmix + resample to 16kHz
//! mono, and in-memory WAV encoding. No disk I/O — everything stays in RAM.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use cpal::traits::DeviceTrait;

/// Build a cpal input stream that appends normalized f32 samples to `buf`.
pub fn build_stream(
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

/// RMS loudness of a sample block.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Downmix to mono and resample to 16kHz by averaging windows (crude anti-alias).
pub fn to_16k_mono(samples: &[f32], in_rate: u32, channels: u16) -> Vec<f32> {
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
        let end = (((i + 1) as f32 * ratio) as usize)
            .min(mono.len())
            .max(start + 1);
        let slice = &mono[start..end.min(mono.len())];
        out.push(slice.iter().sum::<f32>() / slice.len() as f32);
    }
    out
}

/// Encode f32 mono samples ([-1,1], 16kHz) as a 16-bit PCM WAV in memory.
pub fn encode_wav_16k_mono(samples: &[f32]) -> Vec<u8> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rms_of_empty_is_zero() {
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0.0; 100]), 0.0);
    }

    #[test]
    fn rms_of_square_wave_equals_amplitude() {
        // RMS of a ±0.5 block is 0.5.
        assert!((rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn to_16k_mono_passthrough_when_already_mono_16k() {
        let input = [0.1, 0.2, 0.3];
        assert_eq!(to_16k_mono(&input, 16_000, 1), input);
    }

    #[test]
    fn to_16k_mono_empty_input_is_empty() {
        assert!(to_16k_mono(&[], 48_000, 2).is_empty());
    }

    #[test]
    fn to_16k_mono_downmixes_stereo_pairs() {
        // Interleaved L,R at 16k (no resample): (0+1)/2=0.5, (0.5+0.5)/2=0.5.
        let stereo = [0.0, 1.0, 0.5, 0.5];
        assert_eq!(to_16k_mono(&stereo, 16_000, 2), vec![0.5, 0.5]);
    }

    #[test]
    fn to_16k_mono_resamples_48k_to_one_third_length() {
        // 48k -> 16k is 3:1 decimation; 6 mono samples -> 2, averaged per window.
        let input = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        assert_eq!(to_16k_mono(&input, 48_000, 1), vec![0.0, 1.0]);
    }

    #[test]
    fn encode_wav_roundtrips_spec_and_samples() {
        let bytes = encode_wav_16k_mono(&[0.0, 0.5, -0.5, 1.0]);
        let reader = hound::WavReader::new(Cursor::new(bytes)).expect("valid wav");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.bits_per_sample, 16);
        let samples: Vec<i16> = reader.into_samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(samples, vec![0, 16383, -16383, i16::MAX]);
    }

    #[test]
    fn encode_wav_clamps_out_of_range_samples() {
        // 2.0 -> clamp 1.0 -> i16::MAX; -2.0 -> clamp -1.0 -> -i16::MAX.
        let bytes = encode_wav_16k_mono(&[2.0, -2.0]);
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        let samples: Vec<i16> = reader.into_samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(samples, vec![i16::MAX, -i16::MAX]);
    }
}
