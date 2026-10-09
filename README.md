# sf2synth

A real-time SF2 and SFZ MIDI synthesizer and MIDI file sequencer in pure
Rust, tuned for faithful piano reproduction.

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
- **SFZ and FLAC too.** SFZ instruments with WAV or FLAC samples (8–32-bit,
  mono or stereo, decoded in parallel with the crate's own FLAC decoder):
  keys, velocities, tuning, envelopes, filter, velocity curves, loops, and
  what SF2 can't say — release triggers with `rt_decay`, round robin and
  random regions, velocity and key crossfades, mute groups (`group` /
  `off_by`), controller conditions and controller-triggered regions (pedal
  noises), first / legato triggers, one-shots.
- **Piano details.** Sustain and sostenuto pedals, re-striking a key releases
  its previous strike, 4-point cubic interpolation (or band-limited 32-point
  sinc, free of aliasing), click-free gain ramps, filter sweeps and voice
  stealing.
- **GM, GS and XG.** System Exclusive messages: GM / GM2 System On, GS
  Reset, XG System On, master volume, tuning and key shift, GS and XG part
  parameters (program, level, pan, effect sends, key shift, drum parts), GS
  scale tuning (temperaments), GS reverb and chorus. Missing bank
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
- **SF2 and SFZ writers.** Build presets, instruments, zones and samples and
  write them as an SF2 file (`sf2synth::writer`); write any preset as SFZ
  with lossless FLAC samples (`SfzFiles`), which plays back bit for bit
  the same; read any file's zones and sample points back.
- **No audio I/O.** It renders into your buffers; play them with AAudio,
  Core Audio, WASAPI, [cpal](https://crates.io/crates/cpal) or anything else.
- **C interface and Audio Unit.** [capi](capi) is the synthesizer as a C
  library (with memory-mapped loading); the Swift package wraps it as an
  AUv3 instrument for iOS and macOS apps and Audio Unit extensions.

[sf2studio](https://github.com/albos-inc/sf2studio) is a desktop app built
on sf2synth for making, tuning and comparing SF2 instruments.
[examples/player](examples/player) is a sample MIDI file player for iOS,
Android, Windows and macOS, with
[ready-to-run builds](https://github.com/albos-inc/sf2synth/releases?q=player).

## Usage

```toml
[dependencies]
sf2synth = { git = "https://github.com/albos-inc/sf2synth", tag = "v0.4.0" }
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

`master_gain` is 1.0 by default. Apple's MIDI synth (`AVAudioUnitSampler`,
`DLSMusicDevice`) playing the same SoundFont is about 1.6 dB louder; a
`master_gain` of 1.2 matches it. Songs can lower the volume further with a
master volume SysEx message.

The output never clips: a look-ahead limiter (on by default,
`SynthesizerSettings::limiter`) holds the mix below −0.45 dBFS, turning down
only the passages that would go over — ramping down 1 ms before a peak and
back up over about 150 ms — and leaving everything else untouched. It
delays the mix by 1 ms (`Synthesizer::latency`). How much it works depends
on the SoundFont:

| At `master_gain` 1.0 | Loudest reduction | Time turned down by over 0.5 dB / 3 dB |
|---|---|---|
| GeneralUser GS, Chopin étude | 2.8 dB | 0.8% / 0% |
| YDP Grand Piano, Chopin étude | 7.5 dB | 17.7% / 2.1% |

A hot SoundFont like YDP sounds more natural a little lower (`master_gain`
0.5–0.7) with the limiter only catching the peaks.

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
- GS and XG part parameters (program, level, pan, reverb and chorus send,
  key shift) and master key shift apply as their MIDI equivalents.
- After a GS, XG or GM2 reset every channel starts with a reverb send
  (CC91) of 40, as on those sound modules; General MIDI files without one
  start dry.
- GS reverb and chorus parameters adjust the built-in effects around the
  settings given; GS scale tuning retunes each pitch class of a part.
- In MIDI files, SysEx messages continued in `F7` packets are joined.

### SFZ instruments

```rust
let font = SoundFont::open_sfz("Salamander/SalamanderGrandPiano.sfz")?;
// Opcodes it ignored and samples it couldn't read, if any.
for warning in font.warnings() {
    eprintln!("{warning}");
}
// `SoundFont::open` picks SFZ by the extension, else reads SF2 or DLS.
let font = SoundFont::open("piano.sfz")?;
// From anywhere else: a loader gets each file by its path relative to the
// SFZ file (and how much of its start would do).
let font = SoundFont::from_sfz(&text, "piano", &|path, _limit| read_from_my_archive(path))?;
```

An SFZ file becomes one preset (bank 0, program 0) whose zones are its
regions. The samples are decoded into memory when the file is read (an SFZ
can't be memory-mapped): the original Salamander Grand Piano, 707 MB of
24-bit FLAC, loads in about 1.3 s and takes 2 GB. Only the sample points are
kept, as 16-bit points and, for samples of more than 16 bits, their low bytes.

Opcodes read: `sample`, `default_path`, `#define`, `#include`, `note_offset`
/ `octave_offset`, `key` / `lokey` / `hikey` (numbers or names, `c4` = 60),
`lovel` / `hivel`, `pitch_keycenter` (or `sample`), `tune` / `pitch`,
`transpose`, `pitch_keytrack`, `volume` / `gain`, `amplitude`, `pan`,
`width`, `amp_veltrack`, `amp_velcurve_N`, `amp_keytrack` /
`amp_keycenter`, `ampeg_delay` / `attack` / `hold` / `decay` / `sustain` /
`release`, `delay`, `fil_type` (low-pass), `cutoff`, `resonance`,
`fil_veltrack`, `fil_keytrack` / `fil_keycenter`, `fileg_*`, `pitcheg_*`,
`pitchlfo_*`, `amplfo_*`, `fillfo_*`, `loop_mode`, `loop_start` /
`loop_end` (or the WAV file's `smpl` loop), `offset`, `end`, `trigger`
(`attack`, `release`, `release_key`, `first`, `legato`), `rt_decay`,
`seq_length` / `seq_position`, `lorand` / `hirand`, `loccN` / `hiccN`,
`on_loccN` / `on_hiccN`, `lochan` / `hichan`, `group` / `off_by` /
`off_mode`, `xfin_*` / `xfout_*` (velocity and key), `xf_velcurve` /
`xf_keycurve`, `effect1` / `effect2`. Others are listed in `warnings()`.

How it plays:

- Velocity → level is SF2's square law, as `amp_veltrack=100` is in SFZ; it
  then follows the synthesizer's `default_modulators` like an SF2 does.
  Other `amp_veltrack` values or an `amp_velcurve` replace it with the
  region's own curve. Velocity → cutoff is only `fil_veltrack` (SF2's
  default −2400 cents doesn't apply).
- `trigger=release` regions sound at note-off, or when the sustain pedal
  lets the note go, at the note-on's velocity, quieter by `rt_decay` dB per
  second the key was down; `release_key` ones at note-off whatever the
  pedal. They, one-shots and controller-triggered regions play to their end
  whatever note-offs and pedals come.
- A controller-triggered region (`on_loccN`) starts when the controller
  moves into its range, at its own pitch and full velocity.

`SfzFiles` writes any preset (of an SF2, DLS or SFZ file) as SFZ with FLAC
samples — 16 or 24-bit as the file is, stereo pairs as stereo files — so
that sf2synth plays it back exactly as it plays the preset:

```rust
let font = SoundFont::open("Salamander.sf2")?;
let sfz = sf2synth::writer::SfzFiles::from_preset(&font, 0, "samples/");
sfz.write("Salamander/Salamander.sfz")?; // 1,266 MB of samples → 185 MB
```

The FLAC encoder (fixed and LPC prediction up to order 12, partitioned Rice
coding, the best of four stereo decorrelations) writes files any FLAC
decoder reads, about 4% smaller than `flac -8` on piano samples.

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

Without another dependency, [`sf2synth-mmap`](mmap) maps a file with the
system's own `mmap` (Unix) or file mapping (Windows):

```toml
sf2synth-mmap = { git = "https://github.com/albos-inc/sf2synth", tag = "v0.4.0" }
```

```rust
let font = SoundFont::from_shared(sf2synth_mmap::map_file("piano.sf2")?)?;
```

### From C, Swift or Kotlin

[capi](capi) builds the synthesizer as a static or shared library with the
interface of [`capi/include/sf2synth.h`](capi/include/sf2synth.h): fonts
read or memory-mapped, MIDI messages, bytes and UMP, rendering with events
and effect sends.

```sh
cargo build --release --manifest-path capi/Cargo.toml --target aarch64-apple-ios
```

The Swift package of this repository (sources in [apple](apple)) builds on
it: `SF2SynthAudioUnit`, an AUv3 music device (events at their frames,
SysEx, MIDI 2.0 event lists, the reverb send on output bus 1, parameters
and saved state), `SF2SynthInstrument`, the same as an
`AVAudioUnitMIDIInstrument` for `AVAudioEngine`, and
`SF2SynthViewController` for an Audio Unit extension
([examples/auv3](examples/auv3)). Add it in Xcode with the repository's
URL; it uses the Rust library attached to the release.

```swift
let synth = try SF2SynthInstrument(soundFont: url)   // memory-mapped
synth.synth.masterGain = 1.2
engine.attach(synth)
engine.connect(synth, to: engine.mainMixerNode, format: nil)
synth.startNote(60, withVelocity: 100, onChannel: 0)
```

In a checkout, `apple/build-xcframework.sh` builds the Rust library from
source instead (see [apple](apple)).

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

On one Apple M-series core, rendering at 48 kHz with reverb and chorus:

| SoundFont | Music | Cubic | Sinc |
|---|---|---|---|
| [YDP Grand Piano](https://freepats.zenvoid.org/Piano/acoustic-grand-piano.html) (118 MB) | Chopin, piano | 140× real time | 87× |
| GeneralUser GS (32 MB) | GS song, 60 voices | 56× | 30× |

Loading YDP takes 15–40 ms (or nothing much when memory-mapped). With no
sound playing, the reverb and chorus stop computing, so an idle synthesizer
costs almost nothing.

## Roadmap

- Half-pedaling and the soft pedal
- Polyphonic aftertouch as a modulator source
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
