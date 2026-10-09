# Changelog

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
