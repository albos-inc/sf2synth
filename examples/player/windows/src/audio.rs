//! The audio output: a cpal stream (WASAPI on Windows) that renders the
//! player.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use sf2player::SharedPlayer;

/// The most frames rendered in one go; larger callbacks render in pieces.
const CHUNK: usize = 4096;

/// What is shown about the output.
pub struct OutputInfo {
    pub host: String,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
}

pub struct Output {
    shared: Arc<SharedPlayer>,
    pub info: OutputInfo,
    /// Frames in the latest audio callback.
    callback_frames: Arc<AtomicU32>,
    _stream: cpal::Stream,
}

impl Output {
    pub fn open() -> Result<Output, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let info = OutputInfo {
            host: host.id().name().to_string(),
            device: device.description().map(|d| d.name().to_string()).unwrap_or_default(),
            sample_rate: config.sample_rate(),
            channels: config.channels(),
            sample_format: config.sample_format().to_string(),
        };
        let shared = Arc::new(SharedPlayer::new(info.sample_rate));
        let callback_frames = Arc::new(AtomicU32::new(0));
        let shared_ = (Arc::clone(&shared), Arc::clone(&callback_frames));
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config.into(), shared_),
            SampleFormat::I16 => build::<i16>(&device, config.into(), shared_),
            SampleFormat::I32 => build::<i32>(&device, config.into(), shared_),
            SampleFormat::U16 => build::<u16>(&device, config.into(), shared_),
            format => Err(format!("unsupported sample format {format}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Output { shared, info, callback_frames, _stream: stream })
    }

    /// The player, shared with the audio callback.
    pub fn player(&self) -> &SharedPlayer {
        &self.shared
    }

    pub fn callback_frames(&self) -> u32 {
        self.callback_frames.load(Ordering::Relaxed)
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    (shared, callback_frames): (Arc<SharedPlayer>, Arc<AtomicU32>),
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let (mut left, mut right) = (vec![0.0f32; CHUNK], vec![0.0f32; CHUNK]);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                callback_frames.store((data.len() / channels) as u32, Ordering::Relaxed);
                for block in data.chunks_mut(CHUNK * channels) {
                    let frames = block.len() / channels;
                    // Never waits for the UI: if it holds the lock, silence.
                    shared.render(&mut left[..frames], &mut right[..frames]);
                    for (i, frame) in block.chunks_mut(channels).enumerate() {
                        for (c, sample) in frame.iter_mut().enumerate() {
                            let value = match (channels, c) {
                                (1, _) => (left[i] + right[i]) * 0.5,
                                (_, 0) => left[i],
                                (_, 1) => right[i],
                                _ => 0.0,
                            };
                            *sample = T::from_sample(value.clamp(-1.0, 1.0));
                        }
                    }
                }
            },
            |error| eprintln!("audio stream error: {error}"),
            None,
        )
        .map_err(|e| e.to_string())
}
