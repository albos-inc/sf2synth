//! Renders single notes at several velocities to a WAV file, to compare the
//! velocity response and timbre with another synthesizer.
//!
//! ```text
//! cargo run --release --example notes -- <font.sf2> <out.wav> [keys] [velocities] [seconds]
//! ```
//!
//! `keys` and `velocities` are comma-separated (default `36,60,84` and
//! `16,32,48,64,80,96,112,127`). Each note is held for `seconds` (default
//! 2), then released for 1 s.

#[path = "common/wav.rs"]
mod wav;

use std::sync::Arc;

use sf2synth::{SoundFont, Synthesizer, SynthesizerSettings};

fn list(text: Option<&String>, default: &[u8]) -> Result<Vec<u8>, std::num::ParseIntError> {
    match text {
        Some(text) => text.split(',').map(|s| s.trim().parse()).collect(),
        None => Ok(default.to_vec()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: notes <font.sf2> <out.wav> [keys] [velocities] [seconds]");
        std::process::exit(2);
    }
    let keys = list(args.get(3), &[36, 60, 84])?;
    let velocities = list(args.get(4), &[16, 32, 48, 64, 80, 96, 112, 127])?;
    let hold: f64 = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(2.0);
    let sample_rate = 48_000;

    let font = Arc::new(SoundFont::from_bytes(std::fs::read(&args[1])?)?);
    let mut settings = SynthesizerSettings::new(sample_rate);
    settings.reverb.enabled = false;
    settings.chorus.enabled = false;
    let mut synth = Synthesizer::new(font, &settings)?;

    let hold_frames = (hold * sample_rate as f64) as usize;
    let release_frames = sample_rate as usize;
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for &key in &keys {
        for &velocity in &velocities {
            let start = left.len();
            left.resize(start + hold_frames + release_frames, 0.0);
            right.resize(start + hold_frames + release_frames, 0.0);
            synth.note_on(0, key, velocity);
            synth.render(&mut left[start..start + hold_frames], &mut right[start..start + hold_frames]);
            synth.note_off(0, key);
            synth.render(&mut left[start + hold_frames..], &mut right[start + hold_frames..]);
            let peak = left[start..].iter().fold(0.0f32, |m, &x| m.max(x.abs()));
            println!("key {key:3} velocity {velocity:3}: peak {:6.1} dBFS", 20.0 * peak.max(1e-9).log10());
        }
    }
    wav::write_wav(&args[2], sample_rate, &left, &right)?;
    println!("wrote {}", args[2]);
    Ok(())
}
