//! Renders a MIDI file with an SF2 file to a WAV file.
//!
//! ```text
//! cargo run --release --example render -- <font.sf2/.sfz> <song.mid> <out.wav> [sample rate] [speed]
//! ```

#[path = "common/wav.rs"]
mod wav;

use std::sync::Arc;
use std::time::Instant;

use sf2synth::{MidiFile, Sequencer, SoundFont, Synthesizer, SynthesizerSettings};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: render <font.sf2/.sfz> <song.mid> <out.wav> [sample rate] [speed]");
        std::process::exit(2);
    }
    let sample_rate: u32 = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(48_000);
    let speed: f64 = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(1.0);

    let loading = Instant::now();
    let font = Arc::new(SoundFont::open(&args[1])?);
    let midi = Arc::new(MidiFile::from_bytes(&std::fs::read(&args[2])?)?);
    println!("loaded {} ({} presets) in {:.0?}", font.info().name, font.presets().len(), loading.elapsed());

    let mut synth = Synthesizer::new(font, &SynthesizerSettings::new(sample_rate))?;
    let mut sequencer = Sequencer::new(sample_rate);
    sequencer.load(midi.clone());
    sequencer.set_speed(speed);
    sequencer.play();

    // The song plus 3 s for the releases and the reverb.
    let total = ((midi.duration() / speed + 3.0) * sample_rate as f64) as usize;
    let mut left = vec![0.0f32; total];
    let mut right = vec![0.0f32; total];
    let block = 256;
    let rendering = Instant::now();
    let mut peak_voices = 0;
    let mut position = 0;
    while position < total {
        let end = (position + block).min(total);
        sequencer.render(&mut synth, &mut left[position..end], &mut right[position..end]);
        peak_voices = peak_voices.max(synth.active_voice_count());
        position = end;
    }
    let elapsed = rendering.elapsed().as_secs_f64();
    let audio = total as f64 / sample_rate as f64;
    let peak = left.iter().chain(&right).fold(0.0f32, |m, &x| m.max(x.abs()));
    println!(
        "rendered {audio:.1} s in {elapsed:.2} s ({:.0}× real time), up to {peak_voices} voices, peak {peak:.3}",
        audio / elapsed
    );

    wav::write_wav(&args[3], sample_rate, &left, &right)?;
    println!("wrote {}", args[3]);
    Ok(())
}
