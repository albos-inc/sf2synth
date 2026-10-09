//! # sf2synth
//!
//! A real-time SF2 sample-based MIDI synthesizer and a MIDI file sequencer,
//! in pure Rust with no dependencies and no `unsafe`.
//!
//! - Follows SoundFont 2.04: every generator, preset/instrument layering,
//!   the default modulators and the file's own modulators, 24-bit samples.
//! - Real-time safe: after setup, rendering and MIDI handling never
//!   allocate or lock.
//! - Light: sample data is read in place from shared bytes, so a
//!   memory-mapped SF2 costs no heap.
//! - No audio I/O: render into your buffers and play them with whatever the
//!   platform offers (AAudio, Core Audio, WASAPI, cpal…), on any target.
//!
//! ```no_run
//! use std::sync::Arc;
//! use sf2synth::{MidiFile, Sequencer, SoundFont, Synthesizer, SynthesizerSettings};
//!
//! let font = Arc::new(SoundFont::read(std::fs::File::open("piano.sf2")?)?);
//! let settings = SynthesizerSettings::new(48_000);
//! let mut synth = Synthesizer::new(font, &settings)?;
//!
//! // Play a note…
//! synth.note_on(0, 60, 100);
//! let (mut left, mut right) = (vec![0.0; 48_000], vec![0.0; 48_000]);
//! synth.render(&mut left, &mut right);
//!
//! // …or a MIDI file.
//! let midi = Arc::new(MidiFile::read(std::fs::File::open("song.mid")?)?);
//! let mut sequencer = Sequencer::new(48_000);
//! sequencer.load(midi);
//! sequencer.play();
//! sequencer.render(&mut synth, &mut left, &mut right);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! "SoundFont" is a registered trademark of Creative Technology Ltd.

#![forbid(unsafe_code)]

mod channel;
mod dls;
mod dsp;
mod effects;
mod error;
pub mod generator;
mod limiter;
mod midi_file;
pub mod modulator;
mod sequencer;
mod sinc;
mod soundfont;
mod synth;
mod sysex;
mod voice;
pub mod writer;

pub use effects::{ChorusSettings, ReverbSettings};
pub use error::{Error, Result};
pub use midi_file::{MidiEvent, MidiFile, TempoChange};
pub use modulator::Modulator;
pub use sequencer::Sequencer;
pub use soundfont::{FileFormat, Instrument, Preset, SampleHeader, SharedBytes, SoundFont, SoundFontInfo, Zone};
pub use synth::{CHANNEL_COUNT, STEAL_FADE_VOICES, Synthesizer, SynthesizerSettings};
pub use voice::Interpolation;
