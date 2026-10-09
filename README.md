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
- **DLS too.** DLS level 1 and 2 files (such as the General MIDI set built
  into macOS) load as the same presets and play the same way: regions,
  wave samples and loops, and articulation (envelopes, LFOs, filter,
  velocity and key scaling) translated to SF2 parameters.
- **Piano details.** Sustain and sostenuto pedals, re-striking a key releases
  its previous strike, 4-point cubic interpolation, click-free gain ramps.
- **GM, GS and XG.** System Exclusive messages: GM / GM2 System On, GS
  Reset, XG System On, master volume and tuning, GS rhythm parts and scale
  tuning (temperaments), XG part mode, GS reverb and chorus. Missing bank
  variations and drum kits fall back like a Sound Canvas; any channel can
  play drums.
- **Every way in.** Complete MIDI messages, raw MIDI byte streams (running
  status, split SysEx), MIDI 2.0 Universal MIDI Packets, and rendering with
  events at their frame offsets.
- **Sample-accurate sequencer.** MIDI events land on their exact frame,
  whatever the buffer size. Seeking restores programs, controllers and
  SysEx state; speed change, looping and track muting; a sink API for
  external MIDI output with frame offsets.
- **Effect sends out.** Render the reverb and chorus sends (CC91, CC93) to
  your own buffers to feed your own effects.
- **SF2 writer.** Build presets, instruments, zones and samples and write
  them as an SF2 file (`sf2synth::writer`); read any file's zones and sample
  points back.
- **No audio I/O.** It renders into your buffers; play them with AAudio,
  Core Audio, WASAPI, [cpal](https://crates.io/crates/cpal) or anything else.
- **C interface and Audio Unit.** [capi](capi) is the synthesizer as a C
  library (with memory-mapped loading); [apple](apple) is a Swift package
  wrapping it as an AUv3 instrument for iOS and macOS.

[sf2studio](https://github.com/albos-inc/sf2studio) is a desktop app built
on sf2synth for making, tuning and comparing SF2 instruments.
[examples/player](examples/player) is a sample MIDI file player for iOS,
Android, Windows and macOS, with
[ready-to-run builds](https://github.com/albos-inc/sf2synth/releases?q=player).

## Usage

```toml
[dependencies]
sf2synth = { git = "https://github.com/albos-inc/sf2synth", tag = "v0.1.0" }
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

### Volume

`master_gain` is 1.0 by default. The output isn't limited: at 1.0, GS piano
pieces played with GeneralUser GS peak between about −13 and +3 dBFS. Apple's
MIDI synth (`AVAudioUnitSampler`, `DLSMusicDevice`) playing the same
SoundFont is about 1.6 dB louder; a `master_gain` of 1.2 matches it. Songs
can lower the volume further with a master volume SysEx message.

### In an audio callback

```rust
// MIDI with frame offsets in this buffer, e.g. from an Audio Unit's events.
synth.render_with_events(&mut left, &mut right, [(0, &[0xC0, 5][..]), (128, &[0x90, 60, 100][..])]);

// Raw bytes from a MIDI port (running status, SysEx in pieces).
synth.process_midi_bytes(&bytes);

// MIDI 2.0 Universal MIDI Packets.
synth.process_ump(&[0x4090_3C00, 0x8000_0000]);
```

### Your own reverb

Turn the built-in reverb off and render the reverb send, the mono signal
each channel sends with CC91, into its own buffer:

```rust
settings.reverb.enabled = false;
// …
synth.render_with_sends(&mut left, &mut right, Some(&mut reverb_send), None);
```

### GS and XG

GS and XG songs reset the synthesizer with a System Exclusive message and
then select sounds by bank. Each standard is followed after its reset:

- A missing bank variation plays its capital tone (bank 9 falls back to
  bank 8, then bank 0); a missing drum kit plays its capital kit (kit 25,
  TR-808, falls back to 24, then the Standard kit 0).
- GS rhythm parts (`Use for Rhythm Part`), XG part mode, XG banks 126/127
  and GM2 bank 120 make any channel a drum channel;
  `Synthesizer::set_channel_drum` does it from code. Without a reset
  message, banks 120, 126 and 127 play drum kits when the file has no such
  melodic bank.
- GS reverb and chorus parameters adjust the built-in effects around the
  settings given; GS scale tuning retunes each pitch class of a part.

### Changing the sample rate

`Synthesizer::set_sample_rate` keeps the font, settings and MIDI state.
It allocates the effects' delay lines, so call it while not rendering.

### Memory-mapped SF2

`SoundFont::from_shared` takes any `Arc<dyn AsRef<[u8]> + Send + Sync>`, for
example a [`memmap2::Mmap`](https://crates.io/crates/memmap2):

```rust
let file = std::fs::File::open("piano.sf2")?;
let map = unsafe { memmap2::Mmap::map(&file)? };
let font = SoundFont::from_shared(Arc::new(map))?;
```

Without another dependency, `sf2synth_c::map_file` (from [capi](capi))
maps a file with the system's `mmap` and gives the same shared bytes.

### From C, Swift or Kotlin

[capi](capi) builds the synthesizer as a static or shared library with the
interface of [`capi/include/sf2synth.h`](capi/include/sf2synth.h): fonts
read or memory-mapped, MIDI messages, bytes and UMP, rendering with events
and effect sends.

```sh
cargo build --release --manifest-path capi/Cargo.toml --target aarch64-apple-ios
```

[apple](apple) is a Swift package on it: `SF2SynthAudioUnit`, an in-process
AUv3 music device (events at their frames, SysEx, MIDI 2.0 event lists, the
reverb send on output bus 1), and `SF2SynthInstrument`, the same as an
`AVAudioUnitMIDIInstrument` for `AVAudioEngine`:

```swift
let synth = try SF2SynthInstrument(soundFont: url)   // memory-mapped
synth.synth.masterGain = 1.2
engine.attach(synth)
engine.connect(synth, to: engine.mainMixerNode, format: nil)
synth.startNote(60, withVelocity: 100, onChannel: 0)
```

Run `apple/build-xcframework.sh` once to build the Rust library for the
package (`SF2SynthC.xcframework`), then add `apple` as a local package.

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
- More GS / XG part parameters (part level, pan, key shift by SysEx)
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
