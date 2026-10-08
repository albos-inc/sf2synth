# sf2synth

A real-time SF2 MIDI synthesizer and MIDI file sequencer in pure Rust,
tuned for faithful piano reproduction.

- **Pure Rust, no dependencies, no `unsafe`.** Builds for every Rust target:
  Android, iOS, Windows, macOS, Linux, WebAssembly.
- **Real-time safe.** After setup, rendering and MIDI handling never
  allocate or lock, so they can run inside an audio callback.
- **Light.** Sample data is read in place: load an SF2 from a memory map and
  it costs no heap memory, however large it is.
- **SoundFont 2.04.** Every generator, preset/instrument layering, global
  zones, the default modulators and the file's own modulators, 24-bit
  samples (sm24), exclusive classes, loop modes.
- **Piano details.** Sustain and sostenuto pedals, re-striking a key releases
  its previous strike, 4-point cubic interpolation, click-free gain ramps.
- **Sample-accurate sequencer.** MIDI events land on their exact frame,
  whatever the buffer size. Seeking restores programs and controllers;
  speed change, looping and track muting; a sink API for external MIDI
  output with frame offsets.
- **SF2 writer.** Build presets, instruments, zones and samples and write
  them as an SF2 file (`sf2synth::writer`); read any file's zones and sample
  points back.
- **No audio I/O.** It renders into your buffers; play them with AAudio,
  Core Audio, WASAPI, [cpal](https://crates.io/crates/cpal) or anything else.

## Usage

```toml
[dependencies]
sf2synth = { git = "https://github.com/albos-inc/sf2synth" }
```

```rust
use std::sync::Arc;
use sf2synth::{MidiFile, Sequencer, SoundFont, Synthesizer, SynthesizerSettings};

let font = Arc::new(SoundFont::read(std::fs::File::open("piano.sf2")?)?);
let mut synth = Synthesizer::new(font, &SynthesizerSettings::new(48_000))?;

// Live MIDI
synth.note_on(0, 60, 100);
synth.process_midi_message(&[0xB0, 64, 127]); // sustain pedal down

// A MIDI file
let midi = Arc::new(MidiFile::read(std::fs::File::open("song.mid")?)?);
let mut sequencer = Sequencer::new(48_000);
sequencer.load(midi);
sequencer.play();

// In the audio callback
let (mut left, mut right) = (vec![0.0; 256], vec![0.0; 256]);
sequencer.render(&mut synth, &mut left, &mut right);
```

### Memory-mapped SF2

`SoundFont::from_shared` takes any `Arc<dyn AsRef<[u8]> + Send + Sync>`, for
example a [`memmap2::Mmap`](https://crates.io/crates/memmap2):

```rust
let file = std::fs::File::open("piano.sf2")?;
let map = unsafe { memmap2::Mmap::map(&file)? };
let font = SoundFont::from_shared(Arc::new(map))?;
```

### External MIDI output

`Sequencer::advance` plays the file without a synthesizer and hands each
event to a closure with its frame offset in the block, ready to be sent to a
MIDI device with a timestamp:

```rust
sequencer.advance(frames, |offset, message| send(message, block_start + offset));
```

## Matching another synthesizer

Synthesizers interpret SF2 files differently — above all the velocity
response, which for a piano file without velocity layers decides how soft
notes sound. `SynthesizerSettings` exposes what differs:

- `default_modulators`: the modulators every voice starts with (SF2 2.04
  defaults: velocity → attenuation with the concave curve, velocity → filter
  cutoff −2400 cents, CC7/CC11 → attenuation, CC10 → pan, CC91/CC93 →
  effect sends, pitch wheel). Replace or edit them to change the velocity
  curve or turn off the velocity filter.
- `initial_attenuation_scale`: 0.4 like E-mu hardware (the default), or 1.0
  for the specification's literal centibels.
- `interpolation`, `release_same_key`, `reverb`, `chorus`, `master_gain`.

The `notes` example renders single notes across velocities to compare a
font's response with another synthesizer:

```sh
cargo run --release --example notes -- piano.sf2 notes.wav 36,60,84 16,32,64,96,127
cargo run --release --example render -- piano.sf2 song.mid song.wav
```

## Performance

With [YDP Grand Piano](https://freepats.zenvoid.org/Piano/acoustic-grand-piano.html)
(118 MB) on an Apple M-series core: loading takes 15–40 ms, and rendering
piano pieces at 48 kHz runs about 100–160× faster than real time.

## Roadmap

- Half-pedaling and the soft pedal
- Polyphonic aftertouch as a modulator source
- Windowed-sinc interpolation
- SIMD voice mixing
- Sympathetic string resonance for piano

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in the work by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any additional
terms or conditions.

"SoundFont" is a registered trademark of Creative Technology Ltd. This
project is not affiliated with Creative Technology Ltd. or E-mu Systems.
