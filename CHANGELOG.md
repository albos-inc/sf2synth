# Changelog

## 0.3.1 (2026-10-09)

- **No more clipping:** a look-ahead peak limiter on the output, on by
  default (`SynthesizerSettings::limiter`, `Synthesizer::set_limiter`). It
  holds the mix below −0.45 dBFS by turning down only the passages that
  would go over (ramping down 1 ms ahead of a peak, back up over about
  150 ms); quieter output passes unchanged. The mix is delayed by 1 ms:
  `Synthesizer::latency` and `Synthesizer::limiter_latency`. The effect
  sends are neither limited nor delayed.
- C: `sf2synth_set_limiter`, `sf2synth_latency`, `sf2synth_limiter_latency`.
  AUv3: `isLimiterEnabled` (kept in `fullState`), the unit's `latency`
  reports the look-ahead, and `SF2SynthView` has a Limiter switch.

## 0.3.0 (2026-10-09)

Faster, cleaner, steadier.

- About twice as fast: sample points are read in place once per block
  (instead of through a dynamic call per point), mostly without bounds or
  loop checks, in one loop per interpolation method. Output is unchanged.
- An idle reverb and chorus are skipped: a synthesizer with nothing
  sounding costs about a seventh of what it did.
- `Interpolation::Sinc`: 32-point windowed sinc, band-limited to the pitch,
  so samples played higher than recorded don't alias (also
  `sf2synth_set_interpolation` and `SF2SynthAudioUnit.interpolation`, and a
  quality choice in `SF2SynthView`). `Interpolation` is now
  `#[non_exhaustive]`.
- Voice stealing fades the stolen voice out (5 ms) instead of cutting it,
  which clicked; up to `STEAL_FADE_VOICES` voices beyond `max_voices` may
  be fading.
- A moving filter cutoff (modulation envelope or LFO) follows its course
  through each block instead of jumping once a block: less zipper noise.
- Robustness tests: thousands of damaged SF2, DLS and MIDI files, extreme
  generators and modulators, random MIDI bytes, SysEx and UMP — no panic,
  no NaN.
- Sample player: `SharedPlayer` (core) publishes the state, position and
  voices in atomics, so the UI's frequent reads no longer take the lock the
  audio callback tries (which could drop a buffer); the Windows app and the
  iOS/macOS/Android FFI use it.
- AUv3: replacing the SoundFont while rendering releases the previous one's
  file at once.

## 0.2.0 (2026-10-09)

- GS and XG part parameters by SysEx: program (GS tone number, XG bank and
  program), level, pan, reverb and chorus send, key shift; GS master
  key-shift and XG transpose.
- After a GS, XG or GM2 reset, every channel starts with a reverb send
  (CC91) of 40, as on those sound modules (General MIDI still starts at 0).
- MIDI files: SysEx messages continued in `F7` packets are joined, and
  complete messages escaped in `F7` packets are read.
- `sf2synth-mmap` (`mmap/`): memory-mapped files with no dependencies, now
  on Windows too. `capi` uses it; the sample player memory-maps SF2 files.
- Swift package: `Package.swift` moved to the repository's root, so it can
  be added by URL; it downloads the Rust library attached to the release
  unless one is built in `apple/`. `SF2SynthAudioUnit` gets parameters
  (master gain, reverb, chorus; automatable), `fullState` (with the
  SoundFont as a bookmark), declared channel capabilities and
  security-scoped SoundFont URLs. `SF2SynthViewController` and
  `SF2SynthView` make an Audio Unit extension; `examples/auv3` is an app
  with one.
- CI tests `mmap` and `capi` on Linux, macOS and Windows, and builds and
  tests the Swift package and the AUv3 example.

## 0.1.1 (2026-10-09)

- apple: MIDI events were lost when a host pulled the reverb send bus (1)
  before the mix (0), as AVAudioEngine does when bus 1 is connected first:
  the engine passes a cycle's events only to its first render call. That
  call now renders, whichever bus it is for.
- apple: the README's reverb example gives the reverb's output connection
  the synth's format (with `nil`, `AVAudioUnitReverb` stays at 44.1 kHz and
  the engine fails to start at 48 kHz).

## 0.1.0 (2026-10-09)

First version.

- SF2 2.04 loading: presets, instruments, global zones, generators,
  modulators, 24-bit samples; shared/memory-mapped sample data.
- Synthesizer: cubic and linear interpolation, volume and modulation
  envelopes, two LFOs, resonant low-pass filter, default and file
  modulators, sustain and sostenuto pedals, RPN pitch-bend range and tuning,
  exclusive classes, voice stealing, reverb and chorus.
- Standard MIDI File parser (formats 0, 1, 2; RMID) with tempo maps.
- Sample-accurate sequencer with seeking, speed, looping, track muting and
  an external-MIDI sink.
- Read access to zones and sample points, and an SF2 writer (`writer`).
- DLS level 1 and 2 files load as SF2 structures (`SoundFontInfo::format`).
- System Exclusive messages: GM / GM2 System On and Off, GS Reset, XG System
  On, master volume and tuning (Universal, GS, XG), GS Use for Rhythm Part,
  GS Scale Tuning, XG Part Mode, GS reverb and chorus parameters. MIDI files
  keep their SysEx messages (`MidiFile::message`, `MidiEvent::is_sysex`),
  and seeking replays them.
- Bank selection: missing variations fall back to their capital tone,
  missing drum kits to their capital kit; GS, XG and GM2 drum parts; several
  drum channels (`Synthesizer::set_channel_drum`, `is_channel_drum`).
- `Synthesizer::render_with_events` (MIDI at frame offsets),
  `render_with_sends` (reverb and chorus sends to your own buffers),
  `process_midi_bytes` (raw MIDI streams), `process_ump` (MIDI 2.0 UMP),
  `set_sample_rate`, `master_volume`.
- Rendering never panics: buffers of different lengths render the shortest
  length and silence the rest.
- `master_gain` defaults to 1.0 (was 0.5).
- `capi`: the synthesizer as a C library, with memory-mapped loading.
- `apple`: a Swift package with an AUv3 instrument for iOS and macOS.
