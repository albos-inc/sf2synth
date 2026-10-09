//! Reverb and chorus, fed by the voices' effect sends (CC91 / CC93).

/// Reverb settings. The reverb is a Schroeder–Moorer network (8 damped
/// combs and 4 all-passes per side), the structure of Jezar's public-domain
/// Freeverb.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReverbSettings {
    pub enabled: bool,
    /// 0…1.
    pub room_size: f32,
    /// High-frequency damping, 0…1.
    pub damping: f32,
    /// Stereo width, 0…1.
    pub width: f32,
    /// Wet level, 0…1.
    pub level: f32,
}

impl Default for ReverbSettings {
    fn default() -> Self {
        ReverbSettings { enabled: true, room_size: 0.6, damping: 0.4, width: 1.0, level: 0.8 }
    }
}

/// Chorus settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChorusSettings {
    pub enabled: bool,
    /// Wet level, 0…1.
    pub level: f32,
    /// Base delay, milliseconds.
    pub delay_ms: f32,
    /// Modulation depth, milliseconds.
    pub depth_ms: f32,
    /// Modulation rate, hertz.
    pub rate_hz: f32,
}

impl Default for ChorusSettings {
    fn default() -> Self {
        ChorusSettings { enabled: true, level: 0.6, delay_ms: 12.0, depth_ms: 2.5, rate_hz: 0.3 }
    }
}

struct Comb {
    buffer: Vec<f32>,
    index: usize,
    store: f32,
}

impl Comb {
    fn new(length: usize) -> Comb {
        Comb { buffer: vec![0.0; length.max(1)], index: 0, store: 0.0 }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp1: f32, damp2: f32) -> f32 {
        let output = self.buffer[self.index];
        self.store = output * damp2 + self.store * damp1;
        if self.store.abs() < 1e-20 {
            self.store = 0.0;
        }
        self.buffer[self.index] = input + self.store * feedback;
        self.index += 1;
        if self.index == self.buffer.len() {
            self.index = 0;
        }
        output
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.store = 0.0;
    }
}

struct AllPass {
    buffer: Vec<f32>,
    index: usize,
}

impl AllPass {
    fn new(length: usize) -> AllPass {
        AllPass { buffer: vec![0.0; length.max(1)], index: 0 }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.index];
        let output = buffered - input;
        let mut stored = input + buffered * 0.5;
        if stored.abs() < 1e-20 {
            stored = 0.0;
        }
        self.buffer[self.index] = stored;
        self.index += 1;
        if self.index == self.buffer.len() {
            self.index = 0;
        }
        output
    }

    fn clear(&mut self) {
        self.buffer.fill(0.0);
    }
}

const COMB_LENGTHS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_LENGTHS: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const INPUT_GAIN: f32 = 0.015;

pub(crate) struct Reverb {
    combs_left: Vec<Comb>,
    combs_right: Vec<Comb>,
    allpasses_left: Vec<AllPass>,
    allpasses_right: Vec<AllPass>,
    feedback: f32,
    damp1: f32,
    damp2: f32,
    wet1: f32,
    wet2: f32,
    pub enabled: bool,
}

impl Reverb {
    pub fn new(sample_rate: f32, settings: &ReverbSettings) -> Reverb {
        let scale = |length: usize| (length as f32 * sample_rate / 44100.0).round() as usize;
        let mut reverb = Reverb {
            combs_left: COMB_LENGTHS.iter().map(|&l| Comb::new(scale(l))).collect(),
            combs_right: COMB_LENGTHS.iter().map(|&l| Comb::new(scale(l + STEREO_SPREAD))).collect(),
            allpasses_left: ALLPASS_LENGTHS.iter().map(|&l| AllPass::new(scale(l))).collect(),
            allpasses_right: ALLPASS_LENGTHS.iter().map(|&l| AllPass::new(scale(l + STEREO_SPREAD))).collect(),
            feedback: 0.0,
            damp1: 0.0,
            damp2: 0.0,
            wet1: 0.0,
            wet2: 0.0,
            enabled: true,
        };
        reverb.configure(settings);
        reverb
    }

    pub fn configure(&mut self, settings: &ReverbSettings) {
        self.enabled = settings.enabled;
        self.feedback = settings.room_size.clamp(0.0, 1.0) * 0.28 + 0.7;
        self.damp1 = settings.damping.clamp(0.0, 1.0) * 0.4;
        self.damp2 = 1.0 - self.damp1;
        let wet = settings.level.clamp(0.0, 1.0) * 3.0;
        let width = settings.width.clamp(0.0, 1.0);
        self.wet1 = wet * (width / 2.0 + 0.5);
        self.wet2 = wet * ((1.0 - width) / 2.0);
    }

    pub fn clear(&mut self) {
        self.combs_left.iter_mut().for_each(Comb::clear);
        self.combs_right.iter_mut().for_each(Comb::clear);
        self.allpasses_left.iter_mut().for_each(AllPass::clear);
        self.allpasses_right.iter_mut().for_each(AllPass::clear);
    }

    /// Adds the reverb of `input` to `left` and `right`.
    pub fn process(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        let (feedback, damp1, damp2) = (self.feedback, self.damp1, self.damp2);
        for i in 0..input.len() {
            let x = input[i] * INPUT_GAIN;
            let mut out_left = 0.0;
            let mut out_right = 0.0;
            for comb in &mut self.combs_left {
                out_left += comb.process(x, feedback, damp1, damp2);
            }
            for comb in &mut self.combs_right {
                out_right += comb.process(x, feedback, damp1, damp2);
            }
            for allpass in &mut self.allpasses_left {
                out_left = allpass.process(out_left);
            }
            for allpass in &mut self.allpasses_right {
                out_right = allpass.process(out_right);
            }
            left[i] += out_left * self.wet1 + out_right * self.wet2;
            right[i] += out_right * self.wet1 + out_left * self.wet2;
        }
    }
}

pub(crate) struct Chorus {
    buffer: Vec<f32>,
    write: usize,
    phase: f32,
    phase_step: f32,
    delay: f32,
    depth: f32,
    level: f32,
    pub enabled: bool,
}

impl Chorus {
    pub fn new(sample_rate: f32, settings: &ChorusSettings) -> Chorus {
        // Room for up to 50 ms of delay plus 20 ms of depth.
        let length = (0.075 * sample_rate) as usize + 4;
        let mut chorus = Chorus {
            buffer: vec![0.0; length],
            write: 0,
            phase: 0.0,
            phase_step: 0.0,
            delay: 0.0,
            depth: 0.0,
            level: 0.0,
            enabled: true,
        };
        chorus.configure(sample_rate, settings);
        chorus
    }

    pub fn configure(&mut self, sample_rate: f32, settings: &ChorusSettings) {
        self.enabled = settings.enabled;
        self.delay = settings.delay_ms.clamp(1.0, 50.0) * 0.001 * sample_rate;
        self.depth = settings.depth_ms.clamp(0.0, 20.0) * 0.001 * sample_rate;
        self.phase_step = settings.rate_hz.clamp(0.0, 10.0) / sample_rate;
        self.level = settings.level.clamp(0.0, 1.0);
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
    }

    #[inline]
    fn tap(&self, delay: f32) -> f32 {
        let length = self.buffer.len();
        let position = self.write as f32 - delay;
        let position = if position < 0.0 { position + length as f32 } else { position };
        let index = position as usize % length;
        let next = (index + 1) % length;
        let fraction = position.fract();
        self.buffer[index] + (self.buffer[next] - self.buffer[index]) * fraction
    }

    /// Adds the chorus of `input` to `left` and `right` (two taps, a
    /// quarter of a cycle apart).
    pub fn process(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        let length = self.buffer.len();
        for i in 0..input.len() {
            self.buffer[self.write] = input[i];
            let angle = 2.0 * core::f32::consts::PI * self.phase;
            let delay_left = self.delay + self.depth * angle.sin();
            let delay_right = self.delay + self.depth * angle.cos();
            left[i] += self.tap(delay_left) * self.level;
            right[i] += self.tap(delay_right) * self.level;
            self.write = (self.write + 1) % length;
            self.phase += self.phase_step;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
        }
    }
}

/// The GS reverb and chorus parameters (addresses 40 01 30…3F) a song has
/// sent, applied on top of the [`ReverbSettings`] and [`ChorusSettings`]
/// given to the synthesizer: at the GS defaults those are used as they are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GsEffects {
    /// Macro, character, pre-LPF, level, time, delay feedback, (unused),
    /// pre-delay.
    reverb: [u8; 8],
    /// Macro, pre-LPF, level, feedback, delay, rate, depth, send to reverb.
    chorus: [u8; 8],
}

const GS_REVERB_DEFAULTS: [u8; 8] = [4, 4, 0, 64, 64, 0, 0, 0];
const GS_CHORUS_DEFAULTS: [u8; 8] = [2, 0, 64, 8, 80, 3, 19, 0];

impl Default for GsEffects {
    fn default() -> Self {
        GsEffects { reverb: GS_REVERB_DEFAULTS, chorus: GS_CHORUS_DEFAULTS }
    }
}

impl GsEffects {
    pub fn set_reverb(&mut self, parameter: u8, value: u8) {
        let Some(slot) = self.reverb.get_mut(parameter as usize) else { return };
        *slot = value;
        if parameter == 0 {
            // A macro sets the type and returns the rest to its defaults.
            self.reverb = GS_REVERB_DEFAULTS;
            self.reverb[0] = value.min(7);
            self.reverb[1] = value.min(7);
        }
    }

    pub fn set_chorus(&mut self, parameter: u8, value: u8) {
        let Some(slot) = self.chorus.get_mut(parameter as usize) else { return };
        *slot = value;
        if parameter == 0 {
            self.chorus = GS_CHORUS_DEFAULTS;
            self.chorus[0] = value.min(7);
        }
    }

    /// `base` adjusted to the GS reverb: the character (Room 1…3, Hall 1…2,
    /// Plate, Delay, Panning Delay) and time set the room size, the pre-LPF
    /// the damping, the level the wet level. An approximation: the reverb
    /// stays the same Freeverb network.
    pub fn reverb(&self, base: &ReverbSettings) -> ReverbSettings {
        let [_, character, pre_lpf, level, time, ..] = self.reverb;
        let (room, damping) = match character {
            0 => (-0.3, 0.0),  // Room 1
            1 => (-0.2, 0.0),  // Room 2
            2 => (-0.1, 0.0),  // Room 3
            3 => (-0.05, 0.0), // Hall 1
            4 => (0.0, 0.0),   // Hall 2: the base settings
            5 => (-0.1, -0.2), // Plate: brighter
            _ => (-0.2, 0.0),  // Delay, Panning Delay
        };
        ReverbSettings {
            enabled: base.enabled,
            room_size: (base.room_size + room + (time as f32 - 64.0) / 64.0 * 0.25).clamp(0.0, 1.0),
            damping: (base.damping + damping + pre_lpf.min(7) as f32 / 7.0 * 0.4).clamp(0.0, 1.0),
            width: base.width,
            level: (base.level * level as f32 / 64.0).clamp(0.0, 1.0),
        }
    }

    /// `base` adjusted to the GS chorus level, delay, rate and depth (each
    /// relative to its GS default).
    pub fn chorus(&self, base: &ChorusSettings) -> ChorusSettings {
        let [_, _, level, _, delay, rate, depth, _] = self.chorus;
        ChorusSettings {
            enabled: base.enabled,
            level: (base.level * level as f32 / 64.0).clamp(0.0, 1.0),
            delay_ms: base.delay_ms * delay.max(1) as f32 / 80.0,
            depth_ms: base.depth_ms * depth as f32 / 19.0,
            rate_hz: base.rate_hz * rate as f32 / 3.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gs_defaults_keep_the_base_settings() {
        let gs = GsEffects::default();
        let reverb = ReverbSettings::default();
        let chorus = ChorusSettings::default();
        assert_eq!(gs.reverb(&reverb), reverb);
        assert_eq!(gs.chorus(&chorus), chorus);
    }

    #[test]
    fn gs_parameters_move_the_settings() {
        let mut gs = GsEffects::default();
        gs.set_reverb(3, 127);
        gs.set_reverb(4, 0);
        let base = ReverbSettings::default();
        let reverb = gs.reverb(&base);
        assert!(reverb.level > base.level && reverb.room_size < base.room_size);
        gs.set_reverb(0, 0); // Room 1 macro
        assert_eq!(gs.reverb[3], 64);
        assert!(gs.reverb(&base).room_size < base.room_size);
        gs.set_chorus(2, 0);
        assert_eq!(gs.chorus(&ChorusSettings::default()).level, 0.0);
    }
}
