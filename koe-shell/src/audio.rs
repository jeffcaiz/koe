//! Microphone capture via cpal, resampled to 16kHz mono PCM16 LE.
//!
//! The audio stream runs continuously from program start. A gate
//! controls whether frames are pushed to koe-core. This avoids
//! device startup latency (~1s on some USB mics) on each session.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, Stream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

struct StreamHolder(#[allow(dead_code)] Stream);
unsafe impl Send for StreamHolder {}
unsafe impl Sync for StreamHolder {}

static STREAM: Mutex<Option<StreamHolder>> = Mutex::new(None);

/// Gate: when true, audio frames are pushed to koe-core.
static GATE_OPEN: AtomicBool = AtomicBool::new(false);
static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Current audio RMS level (0.0–1.0), stored as f32 bits in AtomicU32.
/// Updated from the audio callback, read by the overlay for waveform display.
static AUDIO_LEVEL: AtomicU32 = AtomicU32::new(0);

/// Read the current audio level (0.0–1.0).
#[allow(dead_code)]
pub fn audio_level() -> f32 {
    f32::from_bits(AUDIO_LEVEL.load(Ordering::Relaxed))
}

/// Initialize the audio stream at program startup. The stream runs
/// continuously but frames are only pushed when the gate is open.
pub fn init() {
    let host = cpal::default_host();
    let device = match host.default_input_device() {
        Some(d) => d,
        None => {
            log::error!("no input device available");
            return;
        }
    };

    log::info!("audio device: {}", device.name().unwrap_or_default());

    let supported = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => {
            log::error!("no supported input config: {e}");
            return;
        }
    };

    log::info!(
        "device config: {} Hz, {} channels, {:?}",
        supported.sample_rate().0,
        supported.channels(),
        supported.sample_format()
    );

    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let config = supported.into();

    let stream = match sample_format {
        SampleFormat::I16 => build_stream::<i16>(&device, &config, sample_rate, channels),
        SampleFormat::F32 => build_stream::<f32>(&device, &config, sample_rate, channels),
        _ => {
            log::error!("unsupported sample format: {sample_format:?}");
            return;
        }
    };

    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            log::error!("failed to build stream: {e}");
            return;
        }
    };

    if let Err(e) = stream.play() {
        log::error!("failed to start stream: {e}");
        return;
    }

    let mut slot = STREAM.lock().unwrap();
    *slot = Some(StreamHolder(stream));
    log::info!("audio stream running (gate closed)");
}

/// Open the gate: start pushing audio frames to koe-core.
pub fn start() {
    FRAME_COUNT.store(0, Ordering::SeqCst);
    GATE_OPEN.store(true, Ordering::SeqCst);
    log::info!("audio gate opened");
}

/// Close the gate: stop pushing audio frames.
pub fn stop() {
    GATE_OPEN.store(false, Ordering::SeqCst);
    let frames = FRAME_COUNT.load(Ordering::SeqCst);
    log::info!("audio gate closed ({frames} frames pushed)");
}

fn build_stream<T: cpal::Sample + cpal::SizedSample + Send + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_rate: u32,
    channels: usize,
) -> Result<Stream, String>
where
    f32: FromSample<T>,
{
    let target_rate = 16000u32;

    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                if !GATE_OPEN.load(Ordering::Relaxed) {
                    return; // gate closed, discard
                }
                process_audio::<T>(data, sample_rate, target_rate, channels);
            },
            |err| {
                log::error!("audio stream error: {err}");
            },
            None,
        )
        .map_err(|e| format!("failed to build input stream: {e}"))?;

    Ok(stream)
}

fn process_audio<T: cpal::Sample>(data: &[T], src_rate: u32, dst_rate: u32, channels: usize)
where
    f32: FromSample<T>,
{
    if data.is_empty() {
        return;
    }

    let mono: Vec<f32> = data
        .chunks(channels)
        .map(|frame| {
            let sum: f32 = frame
                .iter()
                .map(|s| <f32 as FromSample<T>>::from_sample_(*s))
                .sum();
            sum / channels as f32
        })
        .collect();

    // Compute RMS for overlay waveform
    let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len().max(1) as f32).sqrt();
    // Clamp to 0..1 (typical speech RMS is 0.01–0.15, scale up for visual)
    let level = (rms * 6.0).clamp(0.0, 1.0);
    AUDIO_LEVEL.store(level.to_bits(), Ordering::Relaxed);

    let resampled = if src_rate == dst_rate {
        mono
    } else {
        resample(&mono, src_rate, dst_rate)
    };

    let pcm_bytes: Vec<u8> = resampled
        .iter()
        .flat_map(|&sample| {
            let clamped = sample.clamp(-1.0, 1.0);
            let i16_val = (clamped * i16::MAX as f32) as i16;
            i16_val.to_le_bytes()
        })
        .collect();

    let n = FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
    if n == 0 {
        log::info!("first audio frame pushed to core");
    }
    let _ = koe_core::api::push_audio(&pcm_bytes);
}

fn resample(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    let ratio = src_rate as f64 / dst_rate as f64;
    let output_len = (input.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;

        let sample = if idx + 1 < input.len() {
            input[idx] * (1.0 - frac) + input[idx + 1] * frac
        } else if idx < input.len() {
            input[idx]
        } else {
            0.0
        };
        output.push(sample);
    }

    output
}
