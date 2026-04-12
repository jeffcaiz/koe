//! Microphone capture via cpal, resampled to 16kHz mono PCM16 LE.
//!
//! Supports pre-buffering: audio capture starts immediately on `start()`,
//! frames accumulate in a local buffer. Once `flush_to_core()` is called
//! (after session_begin creates the audio channel), buffered frames are
//! pushed to koe-core, and subsequent frames go directly.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, Stream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

struct StreamHolder(#[allow(dead_code)] Stream);
unsafe impl Send for StreamHolder {}
unsafe impl Sync for StreamHolder {}

static STREAM: Mutex<Option<StreamHolder>> = Mutex::new(None);

/// Pre-buffer for audio captured before session_begin completes.
static PRE_BUFFER: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// Whether koe-core's audio channel is ready (session_begin completed).
static CORE_READY: AtomicBool = AtomicBool::new(false);

/// Start capturing audio immediately. Frames are buffered locally
/// until `flush_to_core()` is called.
pub fn start() -> Result<(), String> {
    // Reset state
    CORE_READY.store(false, Ordering::SeqCst);
    {
        let mut buf = PRE_BUFFER.lock().unwrap();
        buf.clear();
    }

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or("no input device available")?;

    log::info!("audio device: {}", device.name().unwrap_or_default());

    let supported = device
        .default_input_config()
        .map_err(|e| format!("no supported input config: {e}"))?;

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
        SampleFormat::I16 => build_stream::<i16>(&device, &config, sample_rate, channels)?,
        SampleFormat::F32 => build_stream::<f32>(&device, &config, sample_rate, channels)?,
        _ => return Err(format!("unsupported sample format: {sample_format:?}")),
    };

    stream.play().map_err(|e| format!("failed to start stream: {e}"))?;

    let mut slot = STREAM.lock().unwrap();
    *slot = Some(StreamHolder(stream));

    log::info!("audio capture started (pre-buffering)");
    Ok(())
}

/// Flush pre-buffered audio to koe-core and switch to direct mode.
/// Call this after `session_begin()` completes.
pub fn flush_to_core() {
    let frames: Vec<Vec<u8>> = {
        let mut buf = PRE_BUFFER.lock().unwrap();
        std::mem::take(&mut *buf)
    };

    let frame_count = frames.len();
    for frame in frames {
        let _ = koe_core::api::push_audio(&frame);
    }

    CORE_READY.store(true, Ordering::SeqCst);
    log::info!("flushed {frame_count} pre-buffered audio frames to core");
}

/// Stop capturing audio.
pub fn stop() {
    CORE_READY.store(false, Ordering::SeqCst);
    let mut slot = STREAM.lock().unwrap();
    if slot.take().is_some() {
        log::info!("audio capture stopped");
    }
    PRE_BUFFER.lock().unwrap().clear();
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

    // Convert to mono f32
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

    // Resample
    let resampled = if src_rate == dst_rate {
        mono
    } else {
        resample(&mono, src_rate, dst_rate)
    };

    // Convert to PCM16 LE bytes
    let pcm_bytes: Vec<u8> = resampled
        .iter()
        .flat_map(|&sample| {
            let clamped = sample.clamp(-1.0, 1.0);
            let i16_val = (clamped * i16::MAX as f32) as i16;
            i16_val.to_le_bytes()
        })
        .collect();

    // Either buffer or push directly
    if CORE_READY.load(Ordering::SeqCst) {
        let _ = koe_core::api::push_audio(&pcm_bytes);
    } else {
        let mut buf = PRE_BUFFER.lock().unwrap();
        buf.push(pcm_bytes);
    }
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
