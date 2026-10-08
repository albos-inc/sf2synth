//! The synthesizer.

use std::sync::Arc;

use crate::channel::Channel;
use crate::effects::{Chorus, ChorusSettings, Reverb, ReverbSettings};
use crate::error::{Error, Result};
use crate::modulator::Modulator;
use crate::soundfont::SoundFont;
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
    /// The General MIDI drum channel (bank 128), 0-based. `None` makes every
    /// channel melodic.
    pub drum_channel: Option<u8>,
    /// Gain applied to the mix.
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
            master_gain: 0.5,
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

/// A real-time SF2 synthesizer.
///
/// Feed it MIDI with [`note_on`](Self::note_on),
/// [`control_change`](Self::control_change)… or
/// [`process_midi_message`](Self::process_midi_message), and pull audio with
/// [`render`](Self::render). After construction nothing allocates, so these
/// can be called from an audio callback.
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
}

impl Synthesizer {
    pub fn new(font: Arc<SoundFont>, settings: &SynthesizerSettings) -> Result<Synthesizer> {
        if !(8000..=192_000).contains(&settings.sample_rate) {
            return Err(Error::InvalidSettings(format!("sample rate {} is not in 8000…192000", settings.sample_rate)));
        }
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
        };
        for channel in 0..CHANNEL_COUNT {
            synth.resolve_preset(channel);
        }
        Ok(synth)
    }

    pub fn sound_font(&self) -> &Arc<SoundFont> {
        &self.font
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate as u32
    }

    pub fn master_gain(&self) -> f32 {
        self.master_gain
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        self.master_gain = gain.max(0.0);
    }

    pub fn reverb_settings(&self) -> ReverbSettings {
        self.reverb_settings
    }

    pub fn set_reverb_settings(&mut self, settings: ReverbSettings) {
        self.reverb_settings = settings;
        self.reverb.configure(&settings);
    }

    pub fn chorus_settings(&self) -> ChorusSettings {
        self.chorus_settings
    }

    pub fn set_chorus_settings(&mut self, settings: ChorusSettings) {
        self.chorus_settings = settings;
        self.chorus.configure(self.sample_rate, &settings);
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

    fn resolve_preset(&mut self, channel: usize) {
        let c = &self.channels[channel];
        let preset = self.font.resolve_preset(c.bank(), c.program as u16);
        self.channels[channel].preset = preset;
    }

    // MARK: - MIDI

    /// Handles one MIDI channel message (status byte first; no running
    /// status). System Reset (0xFF) resets the synthesizer; other system
    /// messages are ignored.
    pub fn process_midi_message(&mut self, message: &[u8]) {
        let Some(&status) = message.first() else { return };
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
        self.channels[index].program = program & 0x7f;
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

    /// Silences everything at once and returns every channel and the
    /// effects to their initial state.
    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.active = false;
        }
        for (i, channel) in self.channels.iter_mut().enumerate() {
            *channel = Channel::new(self.drum_channel == Some(i as u8));
        }
        for channel in 0..CHANNEL_COUNT {
            self.resolve_preset(channel);
        }
        self.reverb.clear();
        self.chorus.clear();
    }

    /// Resets every channel's controllers, pedals and programs without
    /// touching voices (used when a sequencer jumps).
    pub(crate) fn reset_channels(&mut self) {
        for (i, channel) in self.channels.iter_mut().enumerate() {
            *channel = Channel::new(self.drum_channel == Some(i as u8));
        }
        for channel in 0..CHANNEL_COUNT {
            self.resolve_preset(channel);
        }
    }

    // MARK: - Rendering

    /// Renders `left.len()` frames (overwriting both buffers).
    ///
    /// # Panics
    /// If `left` and `right` differ in length.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len(), "left and right must have the same length");
        let mut position = 0;
        while position < left.len() {
            let n = (left.len() - position).min(BLOCK);
            self.render_block(n);
            let gain = self.master_gain;
            for i in 0..n {
                left[position + i] = self.left[i] * gain;
                right[position + i] = self.right[i] * gain;
            }
            position += n;
        }
    }

    /// Renders `output.len() / 2` frames of interleaved stereo.
    pub fn render_interleaved(&mut self, output: &mut [f32]) {
        let frames = output.len() / 2;
        let mut position = 0;
        while position < frames {
            let n = (frames - position).min(BLOCK);
            self.render_block(n);
            let gain = self.master_gain;
            for i in 0..n {
                output[(position + i) * 2] = self.left[i] * gain;
                output[(position + i) * 2 + 1] = self.right[i] * gain;
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
