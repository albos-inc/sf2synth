//! The synthesizer.

use std::sync::Arc;

use crate::channel::Channel;
use crate::effects::{Chorus, ChorusSettings, GsEffects, Reverb, ReverbSettings};
use crate::error::{Error, Result};
use crate::modulator::Modulator;
use crate::soundfont::SoundFont;
use crate::sysex::{self, SysEx, SysExBuffer};
use crate::voice::{BLOCK, Buses, Interpolation, Voice, VoiceStart};

/// Number of MIDI channels.
pub const CHANNEL_COUNT: usize = 16;

/// Settings of a [`Synthesizer`].
#[derive(Clone, Debug)]
pub struct SynthesizerSettings {
    /// Output sample rate, 8000…192000 Hz.
    pub sample_rate: u32,
    /// Voices that can sound at once. A piano with the sustain pedal down
    /// can use well over a hundred.
    pub max_voices: usize,
    pub interpolation: Interpolation,
    /// The drum channel after a reset (bank 128), 0-based: 9 is General
    /// MIDI's channel 10. `None` makes every channel melodic. More channels
    /// can play drums with [`Synthesizer::set_channel_drum`], GS "Use for
    /// Rhythm Part" and XG "Part Mode" messages, and XG and GM2 drum banks.
    pub drum_channel: Option<u8>,
    /// Gain applied to the mix (1.0 by default). The output isn't limited:
    /// at 1.0, GS piano pieces played with GeneralUser GS peak between
    /// about −13 and +3 dBFS. Apple's MIDI synth (`AVAudioUnitSampler`,
    /// `DLSMusicDevice`) playing the same SoundFont is about 1.6 dB louder;
    /// 1.2 matches it.
    pub master_gain: f32,
    /// Scale of the `initialAttenuation` generator. E-mu hardware, and most
    /// SF2 files made for it, apply 0.4 (each centibel counts as 0.04 dB);
    /// 1.0 follows the specification literally.
    pub initial_attenuation_scale: f32,
    /// Modulators every voice starts with, before the file's own. Defaults
    /// to [`Modulator::SF2_DEFAULTS`]; change it to match another
    /// synthesizer's velocity response, for example.
    pub default_modulators: Vec<Modulator>,
    /// A note-on releases the voices still sounding for the same key on the
    /// same channel (as a struck piano string stops its previous vibration).
    pub release_same_key: bool,
    pub reverb: ReverbSettings,
    pub chorus: ChorusSettings,
}

impl Default for SynthesizerSettings {
    fn default() -> Self {
        SynthesizerSettings {
            sample_rate: 44100,
            max_voices: 256,
            interpolation: Interpolation::Cubic,
            drum_channel: Some(9),
            master_gain: 1.0,
            initial_attenuation_scale: 0.4,
            default_modulators: Modulator::SF2_DEFAULTS.to_vec(),
            release_same_key: true,
            reverb: ReverbSettings::default(),
            chorus: ChorusSettings::default(),
        }
    }
}

impl SynthesizerSettings {
    /// Default settings at `sample_rate`.
    pub fn new(sample_rate: u32) -> Self {
        SynthesizerSettings { sample_rate, ..Default::default() }
    }
}

/// Which MIDI standard the last reset message set up: it decides how bank
/// select reaches the drum kits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// No reset message (or GM System On / Off): bank select MSB picks the
    /// bank, and MSB 120, 126 or 127 (GM2 and XG drums) a drum kit when the
    /// file has no such melodic bank.
    General,
    /// GM2 System On: MSB 120 selects drums, 121 melody.
    Gm2,
    /// GS Reset: drums only on the rhythm parts; MSB 127 (the MT-32 set)
    /// falls back to the capital tones.
    Gs,
    /// XG System On: MSB 126 and 127 select drums, anything else melody.
    Xg,
}

/// Parsing state of [`Synthesizer::process_midi_bytes`].
#[derive(Clone, Copy, Debug, Default)]
struct ByteStream {
    running_status: u8,
    data: [u8; 2],
    count: usize,
}

/// A real-time SF2 synthesizer.
///
/// Feed it MIDI with [`note_on`](Self::note_on),
/// [`control_change`](Self::control_change)… or
/// [`process_midi_message`](Self::process_midi_message), and pull audio with
/// [`render`](Self::render). After construction nothing allocates, so these
/// can be called from an audio callback.
///
/// System Exclusive messages it understands: GM System On / Off, GM2 System
/// On, GS Reset, XG System On; master volume, tuning and key shift
/// (Universal, GS and XG); GS and XG part parameters (program, level, pan,
/// reverb and chorus send, key shift, drum part); GS Scale Tuning
/// (temperaments); GS reverb and chorus parameters.
///
/// After a GS, XG or GM2 reset, every channel starts with a reverb send
/// (CC91) of 40, as those sound modules do; without one, it starts at 0.
pub struct Synthesizer {
    font: Arc<SoundFont>,
    sample_rate: f32,
    interpolation: Interpolation,
    master_gain: f32,
    attenuation_scale: f32,
    default_modulators: Vec<Modulator>,
    release_same_key: bool,
    drum_channel: Option<u8>,
    channels: Vec<Channel>,
    voices: Vec<Voice>,
    next_note_id: u64,
    left: [f32; BLOCK],
    right: [f32; BLOCK],
    reverb_send: [f32; BLOCK],
    chorus_send: [f32; BLOCK],
    reverb: Reverb,
    chorus: Chorus,
    reverb_settings: ReverbSettings,
    chorus_settings: ChorusSettings,
    // What System Exclusive messages set.
    mode: Mode,
    /// 0…1.
    master_volume: f32,
    /// Cents.
    master_fine_tuning: f32,
    master_coarse_tuning: f32,
    master_key_shift: f32,
    gs_effects: GsEffects,
    // Input parsing.
    sysex: SysExBuffer,
    stream: ByteStream,
}

impl Synthesizer {
    pub fn new(font: Arc<SoundFont>, settings: &SynthesizerSettings) -> Result<Synthesizer> {
        check_sample_rate(settings.sample_rate)?;
        if settings.max_voices == 0 || settings.max_voices > 4096 {
            return Err(Error::InvalidSettings(format!("max voices {} is not in 1…4096", settings.max_voices)));
        }
        let sample_rate = settings.sample_rate as f32;
        let mut synth = Synthesizer {
            font,
            sample_rate,
            interpolation: settings.interpolation,
            master_gain: settings.master_gain,
            attenuation_scale: settings.initial_attenuation_scale,
            default_modulators: settings.default_modulators.clone(),
            release_same_key: settings.release_same_key,
            drum_channel: settings.drum_channel,
            channels: (0..CHANNEL_COUNT).map(|i| Channel::new(settings.drum_channel == Some(i as u8))).collect(),
            voices: (0..settings.max_voices).map(|_| Voice::default()).collect(),
            next_note_id: 0,
            left: [0.0; BLOCK],
            right: [0.0; BLOCK],
            reverb_send: [0.0; BLOCK],
            chorus_send: [0.0; BLOCK],
            reverb: Reverb::new(sample_rate, &settings.reverb),
            chorus: Chorus::new(sample_rate, &settings.chorus),
            reverb_settings: settings.reverb,
            chorus_settings: settings.chorus,
            mode: Mode::General,
            master_volume: 1.0,
            master_fine_tuning: 0.0,
            master_coarse_tuning: 0.0,
            master_key_shift: 0.0,
            gs_effects: GsEffects::default(),
            sysex: SysExBuffer::new(),
            stream: ByteStream::default(),
        };
        synth.reset_midi_state(Mode::General);
        Ok(synth)
    }

    pub fn sound_font(&self) -> &Arc<SoundFont> {
        &self.font
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate as u32
    }

    /// Changes the output sample rate, keeping the font, the settings and
    /// the MIDI state (programs, controllers…). Sounding voices stop at
    /// once and the reverb and chorus are cleared.
    ///
    /// This allocates the effects' delay lines: call it while not
    /// rendering (in an Audio Unit, from `allocateRenderResources`).
    pub fn set_sample_rate(&mut self, sample_rate: u32) -> Result<()> {
        check_sample_rate(sample_rate)?;
        for voice in &mut self.voices {
            voice.active = false;
        }
        self.sample_rate = sample_rate as f32;
        self.reverb = Reverb::new(self.sample_rate, &self.reverb_settings);
        self.chorus = Chorus::new(self.sample_rate, &self.chorus_settings);
        self.apply_effect_settings();
        Ok(())
    }

    pub fn master_gain(&self) -> f32 {
        self.master_gain
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        self.master_gain = gain.max(0.0);
    }

    /// The master volume a System Exclusive message set, 0…1 (1 after a
    /// reset). It scales the output on top of the master gain, as a squared
    /// curve like CC7's.
    pub fn master_volume(&self) -> f32 {
        self.master_volume
    }

    /// The reverb settings given (GS reverb messages adjust the reverb on
    /// top of them).
    pub fn reverb_settings(&self) -> ReverbSettings {
        self.reverb_settings
    }

    pub fn set_reverb_settings(&mut self, settings: ReverbSettings) {
        self.reverb_settings = settings;
        self.apply_effect_settings();
    }

    /// The chorus settings given (GS chorus messages adjust the chorus on
    /// top of them).
    pub fn chorus_settings(&self) -> ChorusSettings {
        self.chorus_settings
    }

    pub fn set_chorus_settings(&mut self, settings: ChorusSettings) {
        self.chorus_settings = settings;
        self.apply_effect_settings();
    }

    fn apply_effect_settings(&mut self) {
        self.reverb.configure(&self.gs_effects.reverb(&self.reverb_settings));
        self.chorus.configure(self.sample_rate, &self.gs_effects.chorus(&self.chorus_settings));
    }

    pub fn interpolation(&self) -> Interpolation {
        self.interpolation
    }

    pub fn set_interpolation(&mut self, interpolation: Interpolation) {
        self.interpolation = interpolation;
    }

    /// Voices sounding now.
    pub fn active_voice_count(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }

    /// The preset index (into [`SoundFont::presets`]) a channel plays.
    pub fn channel_preset(&self, channel: u8) -> Option<usize> {
        self.channels.get(channel as usize).and_then(|c| c.preset)
    }

    /// Whether a channel plays drum kits (bank 128).
    pub fn is_channel_drum(&self, channel: u8) -> bool {
        self.channels.get(channel as usize).is_some_and(|c| c.is_drum)
    }

    /// Makes a channel play drum kits (bank 128, the kit chosen by its
    /// program) or melodic presets, until the next reset. Notes already
    /// sounding go on.
    pub fn set_channel_drum(&mut self, channel: u8, drum: bool) {
        let index = channel as usize;
        if index >= CHANNEL_COUNT {
            return;
        }
        self.channels[index].is_drum = drum;
        self.channels[index].drum_part = drum;
        self.resolve_preset(index);
    }

    /// Picks the preset for a channel's bank and program. Missing variations
    /// fall back to their capital tone and then bank 0; missing drum kits to
    /// their capital kit and then the Standard kit.
    fn resolve_preset(&mut self, channel: usize) {
        let c = &self.channels[channel];
        let program = c.program as u16;
        let bank = c.bank_msb as u16;
        let preset = if c.is_drum {
            self.font.resolve_drum_kit(program)
        } else if matches!(bank, 120 | 126 | 127)
            && self.mode != Mode::Gs
            && self.font.find_preset(bank, program).is_none()
        {
            // A GM2 or XG drum bank.
            self.font.resolve_drum_kit(program)
        } else {
            self.font.resolve_melodic(bank, program)
        };
        self.channels[channel].preset = preset;
    }

    // MARK: - MIDI

    /// Handles one complete MIDI message (status byte first; no running
    /// status): a channel message, a System Exclusive message (`F0` … `F7`)
    /// or System Reset (`FF`, which resets the synthesizer). Other system
    /// messages are ignored.
    pub fn process_midi_message(&mut self, message: &[u8]) {
        let Some(&status) = message.first() else { return };
        if status == 0xf0 {
            if let Some(message) = sysex::parse(message) {
                self.system_exclusive(message);
            }
            return;
        }
        let data1 = message.get(1).copied().unwrap_or(0) & 0x7f;
        let data2 = message.get(2).copied().unwrap_or(0) & 0x7f;
        let channel = status & 0x0f;
        match status & 0xf0 {
            0x80 => self.note_off(channel, data1),
            0x90 => self.note_on(channel, data1, data2),
            0xb0 => self.control_change(channel, data1, data2),
            0xc0 => self.program_change(channel, data1),
            0xd0 => self.channel_pressure(channel, data1),
            0xe0 => self.pitch_bend(channel, data1 as u16 | ((data2 as u16) << 7)),
            0xf0 if status == 0xff => self.reset(),
            _ => {}
        }
    }

    pub fn note_on(&mut self, channel: u8, key: u8, velocity: u8) {
        let (channel, key, velocity) = (channel as usize, key & 0x7f, velocity & 0x7f);
        if channel >= CHANNEL_COUNT {
            return;
        }
        if velocity == 0 {
            self.note_off(channel as u8, key);
            return;
        }
        let Some(preset_index) = self.channels[channel].preset else { return };

        if self.release_same_key {
            for voice in &mut self.voices {
                if voice.active && voice.channel as usize == channel && voice.key == key && !voice.is_released() {
                    voice.release();
                }
            }
        }

        let note_id = self.next_note_id;
        self.next_note_id += 1;
        let font = Arc::clone(&self.font);
        let preset = &font.presets()[preset_index];
        for preset_zone in preset.zones.iter().filter(|z| z.contains(key, velocity)) {
            let Some(instrument) = font.instruments().get(preset_zone.link) else { continue };
            for instrument_zone in instrument.zones.iter().filter(|z| z.contains(key, velocity)) {
                let Some(sample) = font.samples().get(instrument_zone.link) else { continue };
                if sample.is_rom() {
                    continue;
                }
                let exclusive_class = instrument_zone.generators.get(crate::generator::EXCLUSIVE_CLASS).unwrap_or(0);
                if exclusive_class != 0 {
                    for voice in &mut self.voices {
                        if voice.active
                            && voice.channel as usize == channel
                            && voice.exclusive_class == exclusive_class
                            && voice.note_id != note_id
                        {
                            voice.kill();
                        }
                    }
                }
                let slot = self.allocate_voice(note_id);
                let start = VoiceStart {
                    channel_index: channel as u8,
                    key,
                    velocity,
                    note_id,
                    preset_zone,
                    instrument_zone,
                    sample,
                    sample_count: font.data.len,
                    default_modulators: &self.default_modulators,
                    // DLS attenuations are exact centibels, not E-mu's.
                    attenuation_scale: if font.info().format == crate::FileFormat::Dls {
                        1.0
                    } else {
                        self.attenuation_scale
                    },
                    sample_rate: self.sample_rate,
                    channel: &self.channels[channel],
                };
                self.voices[slot].start(&start);
            }
        }
    }

    /// A free voice, or the one that matters least.
    fn allocate_voice(&self, note_id: u64) -> usize {
        if let Some(free) = self.voices.iter().position(|v| !v.active) {
            return free;
        }
        let mut best = 0;
        let mut best_priority = f32::INFINITY;
        let mut best_age = u64::MAX;
        for (i, voice) in self.voices.iter().enumerate() {
            if voice.note_id == note_id {
                continue;
            }
            let priority = voice.priority();
            if priority < best_priority || (priority == best_priority && voice.note_id < best_age) {
                best = i;
                best_priority = priority;
                best_age = voice.note_id;
            }
        }
        best
    }

    pub fn note_off(&mut self, channel: u8, key: u8) {
        let Some(c) = self.channels.get(channel as usize) else { return };
        let sustain = c.sustain();
        for voice in &mut self.voices {
            if voice.active && voice.channel == channel && voice.key == key && voice.key_down {
                voice.key_down = false;
                if !sustain && !voice.sostenuto {
                    voice.release();
                }
            }
        }
    }

    pub fn control_change(&mut self, channel: u8, controller: u8, value: u8) {
        let index = channel as usize;
        if index >= CHANNEL_COUNT {
            return;
        }
        let (controller, value) = (controller & 0x7f, value & 0x7f);
        match controller {
            120 => {
                self.channels[index].control_change(controller, value);
                self.all_sound_off_channel(channel);
            }
            121 => {
                self.channels[index].reset_controllers();
                self.release_unheld(channel);
                for voice in self.voices.iter_mut().filter(|v| v.active && v.channel == channel) {
                    voice.sostenuto = false;
                }
                self.release_unheld(channel);
            }
            123..=127 => {
                self.channels[index].control_change(controller, value);
                self.all_notes_off_channel(channel);
            }
            64 => {
                self.channels[index].control_change(controller, value);
                if !self.channels[index].sustain() {
                    self.release_unheld(channel);
                }
            }
            66 => {
                let was_on = self.channels[index].sostenuto();
                self.channels[index].control_change(controller, value);
                let is_on = self.channels[index].sostenuto();
                if is_on && !was_on {
                    for voice in self.voices.iter_mut().filter(|v| v.active && v.channel == channel) {
                        voice.sostenuto = voice.key_down;
                    }
                } else if !is_on && was_on {
                    for voice in self.voices.iter_mut().filter(|v| v.active && v.channel == channel) {
                        voice.sostenuto = false;
                    }
                    self.release_unheld(channel);
                }
            }
            _ => {
                self.channels[index].control_change(controller, value);
            }
        }
    }

    /// Releases the voices of `channel` that nothing holds any more.
    fn release_unheld(&mut self, channel: u8) {
        let sustain = self.channels[channel as usize].sustain();
        for voice in &mut self.voices {
            if voice.active
                && voice.channel == channel
                && !voice.key_down
                && !voice.sostenuto
                && !sustain
                && !voice.is_released()
            {
                voice.release();
            }
        }
    }

    pub fn program_change(&mut self, channel: u8, program: u8) {
        let index = channel as usize;
        if index >= CHANNEL_COUNT {
            return;
        }
        let mode = self.mode;
        let c = &mut self.channels[index];
        c.program = program & 0x7f;
        // A bank select takes effect here.
        match (mode, c.bank_msb) {
            (Mode::Xg, msb) => c.is_drum = msb >= 126 || c.drum_part,
            (Mode::Gm2, 120) => c.is_drum = true,
            (Mode::Gm2, 121) => c.is_drum = false,
            _ => {}
        }
        self.resolve_preset(index);
    }

    /// `value` is 14-bit, 8192 = center.
    pub fn pitch_bend(&mut self, channel: u8, value: u16) {
        if let Some(c) = self.channels.get_mut(channel as usize) {
            c.set_pitch_wheel(value);
        }
    }

    pub fn channel_pressure(&mut self, channel: u8, value: u8) {
        if let Some(c) = self.channels.get_mut(channel as usize) {
            c.set_channel_pressure(value);
        }
    }

    fn all_notes_off_channel(&mut self, channel: u8) {
        let sustain = self.channels[channel as usize].sustain();
        for voice in self.voices.iter_mut().filter(|v| v.active && v.channel == channel) {
            voice.key_down = false;
            if !sustain && !voice.sostenuto {
                voice.release();
            }
        }
    }

    fn all_sound_off_channel(&mut self, channel: u8) {
        for voice in self.voices.iter_mut().filter(|v| v.active && v.channel == channel) {
            voice.kill();
        }
    }

    /// Releases every note on every channel (ignoring the pedals).
    pub fn note_off_all(&mut self) {
        for voice in self.voices.iter_mut().filter(|v| v.active) {
            voice.release();
        }
    }

    /// Fades every voice out within a few milliseconds.
    pub fn sound_off_all(&mut self) {
        for voice in self.voices.iter_mut().filter(|v| v.active) {
            voice.kill();
        }
    }

    /// Silences everything at once and returns every channel, the master
    /// volume and tuning, and the effects to their initial state.
    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
        }
        self.reset_midi_state(Mode::General);
        self.sysex.cancel();
        self.stream = ByteStream::default();
        self.reverb.clear();
        self.chorus.clear();
    }

    /// Resets every channel's controllers, pedals and programs and what
    /// System Exclusive messages set, without touching voices (used when a
    /// sequencer jumps).
    pub(crate) fn reset_channels(&mut self) {
        self.reset_midi_state(Mode::General);
    }

    fn reset_midi_state(&mut self, mode: Mode) {
        self.mode = mode;
        self.master_volume = 1.0;
        self.master_fine_tuning = 0.0;
        self.master_coarse_tuning = 0.0;
        self.master_key_shift = 0.0;
        for (i, channel) in self.channels.iter_mut().enumerate() {
            *channel = Channel::new(self.drum_channel == Some(i as u8));
            if mode == Mode::Xg && channel.is_drum {
                channel.bank_msb = 127;
            }
            if mode != Mode::General {
                // GS, XG and GM2 sound modules start every part with some
                // reverb (General MIDI leaves it to the song).
                channel.cc[91] = 40;
            }
        }
        for channel in 0..CHANNEL_COUNT {
            self.resolve_preset(channel);
        }
        self.gs_effects = GsEffects::default();
        self.apply_effect_settings();
    }

    fn system_exclusive(&mut self, message: SysEx) {
        match message {
            SysEx::GmOn { gm2 } => {
                self.sound_off_all();
                self.reset_midi_state(if gm2 { Mode::Gm2 } else { Mode::General });
            }
            SysEx::GmOff => {
                self.sound_off_all();
                self.reset_midi_state(Mode::General);
            }
            SysEx::GsReset => {
                self.sound_off_all();
                self.reset_midi_state(Mode::Gs);
            }
            SysEx::XgOn => {
                self.sound_off_all();
                self.reset_midi_state(Mode::Xg);
            }
            SysEx::MasterVolume(volume) => self.master_volume = volume.clamp(0.0, 1.0),
            SysEx::MasterFineTuning(cents) => {
                self.master_fine_tuning = cents;
                self.apply_master_tuning();
            }
            SysEx::MasterCoarseTuning(semitones) => {
                self.master_coarse_tuning = semitones * 100.0;
                self.apply_master_tuning();
            }
            SysEx::DrumPart { channel, drum } => self.set_channel_drum(channel, drum),
            SysEx::GsReverb { parameter, value } => {
                self.gs_effects.set_reverb(parameter, value);
                self.apply_effect_settings();
            }
            SysEx::GsChorus { parameter, value } => {
                self.gs_effects.set_chorus(parameter, value);
                self.apply_effect_settings();
            }
            SysEx::PartControl { channel, controller, value } => self.control_change(channel, controller, value),
            SysEx::PartProgram { channel, bank, program } => {
                if let Some(bank) = bank {
                    self.control_change(channel, 0, bank);
                }
                self.program_change(channel, program);
            }
            SysEx::PartKeyShift { channel, semitones } => {
                if let Some(c) = self.channels.get_mut(channel as usize) {
                    c.set_key_shift(semitones);
                }
            }
            SysEx::MasterKeyShift(semitones) => {
                self.master_key_shift = semitones as f32 * 100.0;
                self.apply_master_tuning();
            }
            SysEx::GsScaleTuning { channel, start, values, count } => {
                if let Some(c) = self.channels.get_mut(channel as usize) {
                    let classes = &mut c.scale_tuning[start as usize..start as usize + count as usize];
                    for (cents, &value) in classes.iter_mut().zip(&values) {
                        *cents = value as f32 - 64.0;
                    }
                }
            }
        }
    }

    fn apply_master_tuning(&mut self) {
        let cents = self.master_fine_tuning + self.master_coarse_tuning + self.master_key_shift;
        for channel in &mut self.channels {
            channel.set_master_tuning(cents);
        }
    }

    /// Handles raw MIDI bytes as they come from a port: running status,
    /// System Exclusive messages split over several calls, real-time bytes
    /// in between. Messages may be cut anywhere.
    pub fn process_midi_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                0xf8..=0xff => {
                    // Real-time bytes don't interrupt anything.
                    if byte == 0xff {
                        self.reset();
                    }
                }
                0xf0 => {
                    self.finish_sysex();
                    self.sysex.start();
                    self.stream = ByteStream::default();
                }
                0xf7 => self.finish_sysex(),
                0x80..=0xf6 => {
                    self.finish_sysex();
                    // System common messages cancel running status.
                    let running_status = if byte < 0xf0 { byte } else { 0 };
                    self.stream = ByteStream { running_status, ..ByteStream::default() };
                }
                _ if self.sysex.is_open() => self.sysex.push(byte),
                _ => {
                    let stream = &mut self.stream;
                    if stream.running_status == 0 {
                        continue;
                    }
                    stream.data[stream.count] = byte;
                    stream.count += 1;
                    let status = stream.running_status;
                    let length = if matches!(status & 0xf0, 0xc0 | 0xd0) { 1 } else { 2 };
                    if stream.count == length {
                        stream.count = 0;
                        let message = [status, stream.data[0], stream.data[1]];
                        self.process_midi_message(&message[..1 + length]);
                    }
                }
            }
        }
    }

    fn finish_sysex(&mut self) {
        if let Some(message) = self.sysex.finish().and_then(sysex::parse) {
            self.system_exclusive(message);
        }
    }

    /// Handles Universal MIDI Packets (MIDI 2.0), as 32-bit words: MIDI 1.0
    /// and MIDI 2.0 channel voice messages, SysEx7 (Data 64) and System
    /// Reset. MIDI 2.0 values are reduced to MIDI 1.0 resolution (7-bit
    /// velocity and controllers, 14-bit pitch bend); registered controllers
    /// (RPN) apply like MIDI 1.0 data entry. Groups are ignored: every group
    /// plays the same 16 channels. Other messages are skipped.
    pub fn process_ump(&mut self, words: &[u32]) {
        let mut i = 0;
        while i < words.len() {
            let first = words[i];
            let size = match first >> 28 {
                0x0..=0x2 | 0x6 | 0x7 => 1,
                0x3 | 0x4 | 0x8..=0xa => 2,
                0xb | 0xc => 3,
                _ => 4,
            };
            let Some(packet) = words.get(i..i + size) else { return };
            i += size;
            match first >> 28 {
                0x1 if (first >> 16) & 0xff == 0xff => self.reset(),
                0x2 => {
                    let message = [(first >> 16) as u8, (first >> 8) as u8 & 0x7f, first as u8 & 0x7f];
                    if message[0] >= 0x80 && message[0] < 0xf0 {
                        let length = if matches!(message[0] & 0xf0, 0xc0 | 0xd0) { 2 } else { 3 };
                        self.process_midi_message(&message[..length]);
                    }
                }
                0x3 => self.ump_sysex7(packet[0], packet[1]),
                0x4 => self.ump_midi2(packet[0], packet[1]),
                _ => {}
            }
        }
    }

    fn ump_sysex7(&mut self, first: u32, second: u32) {
        let count = (((first >> 16) & 0xf) as usize).min(6);
        let bytes = [
            (first >> 8) as u8,
            first as u8,
            (second >> 24) as u8,
            (second >> 16) as u8,
            (second >> 8) as u8,
            second as u8,
        ];
        let status = (first >> 20) & 0xf;
        if matches!(status, 0 | 1) {
            self.sysex.start();
        } else if !self.sysex.is_open() {
            return;
        }
        for &byte in &bytes[..count] {
            self.sysex.push(byte & 0x7f);
        }
        if matches!(status, 0 | 3) {
            self.finish_sysex();
        }
    }

    fn ump_midi2(&mut self, first: u32, data: u32) {
        let channel = ((first >> 16) & 0xf) as u8;
        let index = ((first >> 8) & 0x7f) as u8;
        let index2 = (first & 0x7f) as u8;
        match (first >> 20) & 0xf {
            0x8 => self.note_off(channel, index),
            0x9 => {
                // A MIDI 2.0 note-on of velocity 0 is still a note-on.
                let velocity = ((data >> 25) as u8).max(1);
                self.note_on(channel, index, velocity);
            }
            0xb => self.control_change(channel, index, (data >> 25) as u8),
            0xc => {
                if first & 1 != 0 {
                    self.control_change(channel, 0, ((data >> 8) & 0x7f) as u8);
                    self.control_change(channel, 32, (data & 0x7f) as u8);
                }
                self.program_change(channel, ((data >> 24) & 0x7f) as u8);
            }
            0xd => self.channel_pressure(channel, (data >> 25) as u8),
            0xe => self.pitch_bend(channel, (data >> 18) as u16),
            0x2 => {
                // Registered controller: through RPN data entry, then RPN null.
                for (controller, value) in [
                    (101, index),
                    (100, index2),
                    (6, (data >> 25) as u8),
                    (38, ((data >> 18) & 0x7f) as u8),
                    (101, 127),
                    (100, 127),
                ] {
                    self.control_change(channel, controller, value);
                }
            }
            _ => {}
        }
    }

    // MARK: - Rendering

    /// Renders `left.len()` frames (overwriting both buffers).
    ///
    /// If `left` and `right` differ in length, the shorter length is
    /// rendered and the rest of the longer buffer is silenced.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        left[frames..].fill(0.0);
        right[frames..].fill(0.0);
        self.render_span(&mut left[..frames], &mut right[..frames], None, None);
    }

    /// Renders like [`render`](Self::render), applying each MIDI message
    /// (as [`process_midi_message`](Self::process_midi_message) does) at its
    /// frame offset in the buffers: `events` are `(offset, message)` pairs in
    /// time order. An offset past the end applies after the last frame; one
    /// earlier than the event before it applies at that event's frame.
    pub fn render_with_events<'m>(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        events: impl IntoIterator<Item = (usize, &'m [u8])>,
    ) {
        let frames = left.len().min(right.len());
        left[frames..].fill(0.0);
        right[frames..].fill(0.0);
        let mut done = 0;
        for (offset, message) in events {
            let at = offset.clamp(done, frames);
            if at > done {
                self.render_span(&mut left[done..at], &mut right[done..at], None, None);
                done = at;
            }
            self.process_midi_message(message);
        }
        self.render_span(&mut left[done..frames], &mut right[done..frames], None, None);
    }

    /// Renders like [`render`](Self::render) and also writes the effect
    /// sends: the mono signal the voices send to the reverb (CC91 and the
    /// `reverbEffectsSend` generator) and to the chorus (CC93), scaled by the
    /// master gain and volume like the mix. Feed them to your own effects;
    /// turn the built-in ones off ([`ReverbSettings::enabled`]) to hear only
    /// yours.
    ///
    /// Buffers of different lengths: the shortest length is rendered and the
    /// rest of the others is silenced.
    pub fn render_with_sends(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        mut reverb: Option<&mut [f32]>,
        mut chorus: Option<&mut [f32]>,
    ) {
        let frames =
            [Some(left.len()), Some(right.len()), reverb.as_ref().map(|r| r.len()), chorus.as_ref().map(|c| c.len())]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(0);
        left[frames..].fill(0.0);
        right[frames..].fill(0.0);
        if let Some(reverb) = reverb.as_deref_mut() {
            reverb[frames..].fill(0.0);
        }
        if let Some(chorus) = chorus.as_deref_mut() {
            chorus[frames..].fill(0.0);
        }
        self.render_span(
            &mut left[..frames],
            &mut right[..frames],
            reverb.map(|r| &mut r[..frames]),
            chorus.map(|c| &mut c[..frames]),
        );
    }

    /// Renders `output.len() / 2` frames of interleaved stereo (a last odd
    /// sample is silenced).
    pub fn render_interleaved(&mut self, output: &mut [f32]) {
        let frames = output.len() / 2;
        output[frames * 2..].fill(0.0);
        let mut position = 0;
        while position < frames {
            let n = (frames - position).min(BLOCK);
            self.render_block(n);
            let gain = self.output_gain();
            for i in 0..n {
                output[(position + i) * 2] = self.left[i] * gain;
                output[(position + i) * 2 + 1] = self.right[i] * gain;
            }
            position += n;
        }
    }

    /// The gain applied to the output: master gain times master volume.
    fn output_gain(&self) -> f32 {
        self.master_gain * self.master_volume * self.master_volume
    }

    /// Renders into buffers of the same length.
    fn render_span(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        mut reverb: Option<&mut [f32]>,
        mut chorus: Option<&mut [f32]>,
    ) {
        let mut position = 0;
        while position < left.len() {
            let n = (left.len() - position).min(BLOCK);
            self.render_block(n);
            let gain = self.output_gain();
            let range = position..position + n;
            for ((l, r), (&dry_l, &dry_r)) in
                left[range.clone()].iter_mut().zip(&mut right[range.clone()]).zip(self.left.iter().zip(&self.right))
            {
                *l = dry_l * gain;
                *r = dry_r * gain;
            }
            if let Some(reverb) = reverb.as_deref_mut() {
                for (out, &send) in reverb[range.clone()].iter_mut().zip(&self.reverb_send) {
                    *out = send * gain;
                }
            }
            if let Some(chorus) = chorus.as_deref_mut() {
                for (out, &send) in chorus[range].iter_mut().zip(&self.chorus_send) {
                    *out = send * gain;
                }
            }
            position += n;
        }
    }

    fn render_block(&mut self, n: usize) {
        let Synthesizer {
            font,
            sample_rate,
            interpolation,
            channels,
            voices,
            left,
            right,
            reverb_send,
            chorus_send,
            reverb,
            chorus,
            ..
        } = self;
        left[..n].fill(0.0);
        right[..n].fill(0.0);
        reverb_send[..n].fill(0.0);
        chorus_send[..n].fill(0.0);
        {
            let mut buses = Buses {
                left: &mut left[..n],
                right: &mut right[..n],
                reverb: &mut reverb_send[..n],
                chorus: &mut chorus_send[..n],
            };
            for voice in voices.iter_mut().filter(|v| v.active) {
                let channel = &channels[voice.channel as usize];
                voice.render(n, &font.data, channel, *sample_rate, *interpolation, &mut buses);
            }
        }
        if reverb.enabled {
            reverb.process(&reverb_send[..n], &mut left[..n], &mut right[..n]);
        }
        if chorus.enabled {
            chorus.process(&chorus_send[..n], &mut left[..n], &mut right[..n]);
        }
    }
}

fn check_sample_rate(sample_rate: u32) -> Result<()> {
    if !(8000..=192_000).contains(&sample_rate) {
        return Err(Error::InvalidSettings(format!("sample rate {sample_rate} is not in 8000…192000")));
    }
    Ok(())
}
