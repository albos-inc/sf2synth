# Changelog

## 0.1.0 (unreleased)

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
