//! One sounding sample: oscillator, filter, envelopes, LFOs and modulators.

use crate::channel::Channel;
use crate::dsp::{
    EnvelopeTimes, Lfo, LowPass, ModulationEnvelope, VolumeEnvelope, absolute_cents_to_hertz, centibels_to_gain,
    timecents_to_seconds,
};
use crate::generator::{self as g, PARAM_COUNT};
use crate::modulator::{self, Modulator, SourceInput};
use crate::soundfont::{SampleData, SampleHeader, Zone};

/// Frames rendered per control-rate step.
pub(crate) const BLOCK: usize = 64;
/// Most modulators a voice keeps (defaults + instrument + preset).
const MAX_MODULATORS: usize = 64;
/// Shortest release, to avoid clicks.
const MIN_RELEASE_SECONDS: f32 = 0.002;
/// How long a voice cut off by an exclusive class or All Sound Off fades.
pub(crate) const KILL_SECONDS: f32 = 0.005;

const FRACTION_BITS: u32 = 32;
const FRACTION_MASK: u64 = (1 << FRACTION_BITS) - 1;
const FRACTION_SCALE: f32 = 1.0 / (1u64 << FRACTION_BITS) as f32;

/// How sample points are interpolated when a sample is played at another
/// pitch or rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Interpolation {
    /// 2-point linear: cheapest, some aliasing and dulling.
    Linear,
    /// 4-point cubic Hermite: clean for piano at a small cost.
    #[default]
    Cubic,
}

/// What a voice is started with.
pub(crate) struct VoiceStart<'a> {
    pub channel_index: u8,
    pub key: u8,
    pub velocity: u8,
    pub note_id: u64,
    pub preset_zone: &'a Zone,
    pub instrument_zone: &'a Zone,
    pub sample: &'a SampleHeader,
    pub sample_count: usize,
    pub default_modulators: &'a [Modulator],
    pub attenuation_scale: f32,
    pub sample_rate: f32,
    pub channel: &'a Channel,
}

/// The mix buses a voice adds to.
pub(crate) struct Buses<'a> {
    pub left: &'a mut [f32],
    pub right: &'a mut [f32],
    pub reverb: &'a mut [f32],
    pub chorus: &'a mut [f32],
}

pub(crate) struct Voice {
    pub active: bool,
    pub channel: u8,
    pub key: u8,
    pub note_id: u64,
    /// The key is still held down (no note-off yet).
    pub key_down: bool,
    /// Held by the sostenuto pedal.
    pub sostenuto: bool,
    pub exclusive_class: i16,
    released: bool,

    /// Key and velocity the modulators and key scaling see.
    mod_key: u8,
    mod_velocity: u8,

    start: usize,
    end: usize,
    loop_start: usize,
    loop_end: usize,
    looping: bool,
    loop_mode: u16,
    phase: u64,
    sample_ratio: f64,
    root_key: f32,
    pitch_correction: f32,

    base: [f32; PARAM_COUNT],
    params: [f32; PARAM_COUNT],
    modulators: [Modulator; MAX_MODULATORS],
    modulator_count: usize,
    seen_version: u32,

    volume_envelope: VolumeEnvelope,
    modulation_envelope: ModulationEnvelope,
    modulation_lfo: Lfo,
    vibrato_lfo: Lfo,
    filter: LowPass,
    /// Bus gains at the end of the last block (ramped from, to avoid zipper).
    last_gains: [f32; 4],
    scratch: [f32; BLOCK],
}

const EMPTY_MODULATOR: Modulator = Modulator { source: 0, destination: 0, amount: 0, amount_source: 0, transform: 0 };

impl Default for Voice {
    fn default() -> Self {
        Voice {
            active: false,
            channel: 0,
            key: 0,
            note_id: 0,
            key_down: false,
            sostenuto: false,
            exclusive_class: 0,
            released: false,
            mod_key: 0,
            mod_velocity: 0,
            start: 0,
            end: 0,
            loop_start: 0,
            loop_end: 0,
            looping: false,
            loop_mode: 0,
            phase: 0,
            sample_ratio: 1.0,
            root_key: 60.0,
            pitch_correction: 0.0,
            base: [0.0; PARAM_COUNT],
            params: [0.0; PARAM_COUNT],
            modulators: [EMPTY_MODULATOR; MAX_MODULATORS],
            modulator_count: 0,
            seen_version: 0,
            volume_envelope: VolumeEnvelope::default(),
            modulation_envelope: ModulationEnvelope::default(),
            modulation_lfo: Lfo::default(),
            vibrato_lfo: Lfo::default(),
            filter: LowPass::default(),
            last_gains: [0.0; 4],
            scratch: [0.0; BLOCK],
        }
    }
}

impl Voice {
    pub fn start(&mut self, s: &VoiceStart<'_>) {
        let instrument = &s.instrument_zone.generators;
        let preset = &s.preset_zone.generators;

        self.active = true;
        self.channel = s.channel_index;
        self.key = s.key;
        self.note_id = s.note_id;
        self.key_down = true;
        self.sostenuto = false;
        self.released = false;
        self.exclusive_class = instrument.get(g::EXCLUSIVE_CLASS).unwrap_or(0);

        let keynum = instrument.get(g::KEYNUM).unwrap_or(-1);
        let velocity = instrument.get(g::VELOCITY).unwrap_or(-1);
        self.mod_key = if (0..=127).contains(&keynum) { keynum as u8 } else { s.key };
        self.mod_velocity = if (1..=127).contains(&velocity) { velocity as u8 } else { s.velocity };

        // Generators: instrument values (or defaults) plus preset offsets.
        for generator in 0..g::COUNT as u16 {
            let mut value = instrument.get(generator).unwrap_or(g::default_value(generator)) as f32;
            if g::allowed_in_preset(generator) {
                if let Some(offset) = preset.get(generator) {
                    value += offset as f32;
                }
            }
            self.base[generator as usize] = value;
        }
        self.base[g::PITCH as usize] = 0.0;
        self.base[g::INITIAL_ATTENUATION as usize] *= s.attenuation_scale;

        // Modulators: the defaults, replaced by identical instrument
        // modulators; preset modulators add to them.
        self.modulator_count = 0;
        for &m in s.default_modulators {
            self.push_modulator(m);
        }
        let defaults = self.modulator_count;
        for &m in &s.instrument_zone.modulators {
            match self.modulators[..defaults].iter_mut().find(|e| e.same_kind(&m)) {
                Some(existing) => *existing = m,
                None => self.push_modulator(m),
            }
        }
        for &m in &s.preset_zone.modulators {
            self.push_modulator(m);
        }

        // Sample addresses (instrument-level offsets only).
        let offset = |fine: u16, coarse: u16| -> i64 {
            instrument.get(fine).unwrap_or(0) as i64 + 32768 * instrument.get(coarse).unwrap_or(0) as i64
        };
        let total = s.sample_count as i64;
        let start = (s.sample.start as i64 + offset(g::START_ADDRS_OFFSET, g::START_ADDRS_COARSE_OFFSET))
            .clamp(0, (total - 1).max(0));
        let end = (s.sample.end as i64 + offset(g::END_ADDRS_OFFSET, g::END_ADDRS_COARSE_OFFSET)).clamp(start, total);
        let loop_start = (s.sample.loop_start as i64
            + offset(g::STARTLOOP_ADDRS_OFFSET, g::STARTLOOP_ADDRS_COARSE_OFFSET))
        .clamp(start, end);
        let loop_end = (s.sample.loop_end as i64 + offset(g::ENDLOOP_ADDRS_OFFSET, g::ENDLOOP_ADDRS_COARSE_OFFSET))
            .clamp(loop_start, end);
        self.start = start as usize;
        self.end = end as usize;
        self.loop_start = loop_start as usize;
        self.loop_end = loop_end as usize;
        self.loop_mode = instrument.get(g::SAMPLE_MODES).unwrap_or(0) as u16 & 3;
        // A loop too short to play (or reversed) is ignored: the sample
        // plays once.
        self.looping = matches!(self.loop_mode, 1 | 3) && self.loop_end - self.loop_start >= 2;
        self.phase = (self.start as u64) << FRACTION_BITS;

        let root = instrument.get(g::OVERRIDING_ROOT_KEY).unwrap_or(-1);
        self.root_key = if (0..=127).contains(&root) {
            root as f32
        } else if s.sample.original_pitch <= 127 {
            s.sample.original_pitch as f32
        } else {
            60.0
        };
        self.pitch_correction = s.sample.pitch_correction as f32;
        let rate = if s.sample.sample_rate == 0 { 44100 } else { s.sample.sample_rate };
        self.sample_ratio = rate as f64 / s.sample_rate as f64;

        self.update_params(s.channel, true);

        let key_offset = 60.0 - self.mod_key as f32;
        let p = &self.params;
        self.volume_envelope.start(EnvelopeTimes {
            delay: timecents_to_seconds(p[g::DELAY_VOL_ENV as usize]),
            attack: timecents_to_seconds(p[g::ATTACK_VOL_ENV as usize]),
            hold: timecents_to_seconds(
                p[g::HOLD_VOL_ENV as usize] + p[g::KEYNUM_TO_VOL_ENV_HOLD as usize] * key_offset,
            ),
            decay: timecents_to_seconds(
                p[g::DECAY_VOL_ENV as usize] + p[g::KEYNUM_TO_VOL_ENV_DECAY as usize] * key_offset,
            ),
            sustain: p[g::SUSTAIN_VOL_ENV as usize],
            release: 0.0,
        });
        self.modulation_envelope.start(EnvelopeTimes {
            delay: timecents_to_seconds(p[g::DELAY_MOD_ENV as usize]),
            attack: timecents_to_seconds(p[g::ATTACK_MOD_ENV as usize]),
            hold: timecents_to_seconds(
                p[g::HOLD_MOD_ENV as usize] + p[g::KEYNUM_TO_MOD_ENV_HOLD as usize] * key_offset,
            ),
            decay: timecents_to_seconds(
                p[g::DECAY_MOD_ENV as usize] + p[g::KEYNUM_TO_MOD_ENV_DECAY as usize] * key_offset,
            ),
            sustain: 1.0 - p[g::SUSTAIN_MOD_ENV as usize] / 1000.0,
            release: 0.0,
        });
        self.modulation_lfo.start(
            timecents_to_seconds(p[g::DELAY_MOD_LFO as usize]),
            absolute_cents_to_hertz(p[g::FREQ_MOD_LFO as usize]),
        );
        self.vibrato_lfo.start(
            timecents_to_seconds(p[g::DELAY_VIB_LFO as usize]),
            absolute_cents_to_hertz(p[g::FREQ_VIB_LFO as usize]),
        );
        self.filter.reset();
        self.last_gains = [0.0; 4];
    }

    fn push_modulator(&mut self, m: Modulator) {
        if self.modulator_count < MAX_MODULATORS {
            self.modulators[self.modulator_count] = m;
            self.modulator_count += 1;
        }
    }

    /// Recomputes the modulated parameters if the channel changed.
    fn update_params(&mut self, channel: &Channel, force: bool) {
        if !force && channel.version == self.seen_version {
            return;
        }
        self.seen_version = channel.version;
        self.params = self.base;
        let input = SourceInput {
            cc: &channel.cc,
            velocity: self.mod_velocity,
            key: self.mod_key,
            channel_pressure: channel.channel_pressure,
            pitch_wheel: channel.pitch_wheel,
            pitch_wheel_sensitivity: channel.pitch_wheel_sensitivity,
        };
        for m in &self.modulators[..self.modulator_count] {
            let destination = m.destination as usize;
            if destination < PARAM_COUNT {
                self.params[destination] += modulator::evaluate(m, &input);
            }
        }
        for generator in 0..g::COUNT as u16 {
            if let Some((low, high)) = g::range(generator) {
                let value = &mut self.params[generator as usize];
                *value = value.clamp(low, high);
            }
        }
    }

    pub fn is_released(&self) -> bool {
        self.released
    }

    /// Note-off: the envelopes enter their release.
    pub fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        self.key_down = false;
        let release = timecents_to_seconds(self.params[g::RELEASE_VOL_ENV as usize]).max(MIN_RELEASE_SECONDS);
        self.volume_envelope.release(release);
        self.modulation_envelope.release(timecents_to_seconds(self.params[g::RELEASE_MOD_ENV as usize]));
        if self.loop_mode == 3 {
            self.looping = false;
        }
    }

    /// Fades out in a few milliseconds.
    pub fn kill(&mut self) {
        self.released = true;
        self.key_down = false;
        self.volume_envelope.kill(KILL_SECONDS);
    }

    /// How much the voice matters, for voice stealing (lower goes first).
    pub fn priority(&self) -> f32 {
        let gain = self.volume_envelope.gain();
        if self.released { gain * 0.25 } else { gain + 1.0 }
    }

    /// Renders `n` (≤ [`BLOCK`]) frames, adding to the buses.
    pub fn render(
        &mut self,
        n: usize,
        data: &SampleData,
        channel: &Channel,
        sample_rate: f32,
        interpolation: Interpolation,
        buses: &mut Buses<'_>,
    ) {
        debug_assert!(n <= BLOCK);
        self.update_params(channel, false);
        let p = &self.params;
        let modulation_lfo = self.modulation_lfo.value();
        let vibrato_lfo = self.vibrato_lfo.value();
        let modulation_envelope = self.modulation_envelope.value();

        let cents = (self.mod_key as f32 - self.root_key) * p[g::SCALE_TUNING as usize]
            + p[g::COARSE_TUNE as usize] * 100.0
            + p[g::FINE_TUNE as usize]
            + self.pitch_correction
            + channel.tuning
            + p[g::PITCH as usize]
            + p[g::MOD_LFO_TO_PITCH as usize] * modulation_lfo
            + p[g::VIB_LFO_TO_PITCH as usize] * vibrato_lfo
            + p[g::MOD_ENV_TO_PITCH as usize] * modulation_envelope;
        let ratio = self.sample_ratio * ((cents / 1200.0) as f64).exp2();
        let step = (ratio.min(1024.0) * (1u64 << FRACTION_BITS) as f64) as u64;

        let cutoff = p[g::INITIAL_FILTER_FC as usize]
            + p[g::MOD_LFO_TO_FILTER_FC as usize] * modulation_lfo
            + p[g::MOD_ENV_TO_FILTER_FC as usize] * modulation_envelope;
        self.filter.set(cutoff, p[g::INITIAL_FILTER_Q as usize], sample_rate);

        let attenuation = p[g::INITIAL_ATTENUATION as usize] - p[g::MOD_LFO_TO_VOLUME as usize] * modulation_lfo;
        let pan = (p[g::PAN as usize] / 1000.0).clamp(-0.5, 0.5);
        let angle = (pan + 0.5) * core::f32::consts::FRAC_PI_2;
        let reverb = p[g::REVERB_EFFECTS_SEND as usize] / 1000.0;
        let chorus = p[g::CHORUS_EFFECTS_SEND as usize] / 1000.0;

        let dt = n as f32 / sample_rate;
        self.volume_envelope.advance(dt);
        self.modulation_envelope.advance(dt);
        self.modulation_lfo.advance(dt);
        self.vibrato_lfo.advance(dt);

        let gain = centibels_to_gain(attenuation) * self.volume_envelope.gain();
        let (sin, cos) = angle.sin_cos();
        let targets = [gain * cos, gain * sin, gain * reverb, gain * chorus];

        let mut scratch = self.scratch;
        let playing = self.generate(&mut scratch[..n], data, step, interpolation);
        self.filter.process(&mut scratch[..n]);

        let from = self.last_gains;
        let inverse = 1.0 / n as f32;
        let deltas = [
            (targets[0] - from[0]) * inverse,
            (targets[1] - from[1]) * inverse,
            (targets[2] - from[2]) * inverse,
            (targets[3] - from[3]) * inverse,
        ];
        let send_reverb = from[2] > 0.0 || targets[2] > 0.0;
        let send_chorus = from[3] > 0.0 || targets[3] > 0.0;
        for (i, &s) in scratch[..n].iter().enumerate() {
            let t = (i + 1) as f32;
            buses.left[i] += s * (from[0] + deltas[0] * t);
            buses.right[i] += s * (from[1] + deltas[1] * t);
            if send_reverb {
                buses.reverb[i] += s * (from[2] + deltas[2] * t);
            }
            if send_chorus {
                buses.chorus[i] += s * (from[3] + deltas[3] * t);
            }
        }
        self.scratch = scratch;
        self.last_gains = targets;

        if !playing || self.volume_envelope.is_done() {
            self.active = false;
        }
    }

    /// Reads `out.len()` interpolated sample points. Returns false once the
    /// sample has played to its end (the rest of `out` is then silent).
    fn generate(&mut self, out: &mut [f32], data: &SampleData, step: u64, interpolation: Interpolation) -> bool {
        let loop_length = (self.loop_end - self.loop_start) as u64;
        for i in 0..out.len() {
            let mut index = (self.phase >> FRACTION_BITS) as usize;
            if self.looping {
                while index >= self.loop_end {
                    self.phase -= loop_length << FRACTION_BITS;
                    index = (self.phase >> FRACTION_BITS) as usize;
                }
            } else if index >= self.end {
                out[i..].fill(0.0);
                return false;
            }
            let fraction = (self.phase & FRACTION_MASK) as f32 * FRACTION_SCALE;
            out[i] = match interpolation {
                Interpolation::Linear => {
                    let x0 = self.point(data, index);
                    let x1 = self.point(data, index + 1);
                    x0 + (x1 - x0) * fraction
                }
                Interpolation::Cubic => {
                    let xm1 = if index > self.start { self.point(data, index - 1) } else { self.point(data, index) };
                    let x0 = self.point(data, index);
                    let x1 = self.point(data, index + 1);
                    let x2 = self.point(data, index + 2);
                    // Catmull-Rom (4-point, 3rd-order Hermite).
                    let c1 = 0.5 * (x1 - xm1);
                    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
                    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
                    ((c3 * fraction + c2) * fraction + c1) * fraction + x0
                }
            };
            self.phase = self.phase.wrapping_add(step);
        }
        true
    }

    /// The sample point `index`, wrapped into the loop while looping and
    /// silent past the end.
    #[inline]
    fn point(&self, data: &SampleData, index: usize) -> f32 {
        if self.looping {
            if index >= self.loop_end {
                return data.get(index - (self.loop_end - self.loop_start));
            }
        } else if index >= self.end {
            return 0.0;
        }
        data.get(index)
    }
}
