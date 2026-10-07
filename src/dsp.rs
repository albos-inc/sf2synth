//! Envelopes, LFOs and the voice filter.

/// Seconds of a time in timecents. -12000 (and below) is instant.
#[inline]
pub(crate) fn timecents_to_seconds(timecents: f32) -> f32 {
    if timecents <= -12000.0 { 0.0 } else { (timecents / 1200.0).exp2() }
}

/// Linear gain of an attenuation in centibels.
#[inline]
pub(crate) fn centibels_to_gain(centibels: f32) -> f32 {
    if centibels <= 0.0 { 1.0 } else { 10f32.powf(-centibels / 200.0) }
}

/// Hertz of an absolute pitch in cents (cent 0 = MIDI key 0 = 8.176 Hz).
#[inline]
pub(crate) fn absolute_cents_to_hertz(cents: f32) -> f32 {
    8.175_799 * (cents / 1200.0).exp2()
}

/// The attenuation (96 dB) at which a voice is silent and stops.
pub(crate) const SILENCE_CENTIBELS: f32 = 960.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    Done,
}

/// Times of an envelope's stages, in seconds, and its sustain level.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EnvelopeTimes {
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    /// Volume envelope: attenuation in centibels. Modulation envelope:
    /// level 0…1.
    pub sustain: f32,
    pub release: f32,
}

/// The SF2 volume envelope: attack rises linearly in amplitude; decay and
/// release fall linearly in decibels (96 dB over the decay/release time).
#[derive(Clone, Debug)]
pub(crate) struct VolumeEnvelope {
    times: EnvelopeTimes,
    stage: Stage,
    time: f32,
    attenuation: f32,
    gain: f32,
}

impl Default for VolumeEnvelope {
    fn default() -> Self {
        VolumeEnvelope { times: EnvelopeTimes::default(), stage: Stage::Done, time: 0.0, attenuation: 0.0, gain: 0.0 }
    }
}

impl VolumeEnvelope {
    pub fn start(&mut self, times: EnvelopeTimes) {
        self.times = times;
        self.stage = Stage::Delay;
        self.time = 0.0;
        self.attenuation = 0.0;
        self.gain = 0.0;
    }

    pub fn release(&mut self, release_seconds: f32) {
        if self.stage == Stage::Done || self.stage == Stage::Release {
            return;
        }
        self.times.release = release_seconds;
        if self.gain <= 1e-5 {
            self.stage = Stage::Done;
            self.gain = 0.0;
            return;
        }
        self.attenuation = -200.0 * self.gain.log10();
        self.stage = Stage::Release;
        self.time = 0.0;
    }

    /// Fades out within a few milliseconds (voice stealing, exclusive class).
    pub fn kill(&mut self, seconds: f32) {
        self.release(seconds);
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn is_done(&self) -> bool {
        self.stage == Stage::Done
    }

    /// Advances by `dt` seconds; the gain is then the gain at the end.
    pub fn advance(&mut self, mut dt: f32) {
        let t = self.times;
        loop {
            match self.stage {
                Stage::Delay => {
                    let left = t.delay - self.time;
                    self.gain = 0.0;
                    if dt < left {
                        self.time += dt;
                        return;
                    }
                    dt -= left.max(0.0);
                    self.enter(Stage::Attack);
                }
                Stage::Attack => {
                    let left = t.attack - self.time;
                    if dt < left {
                        self.time += dt;
                        self.gain = self.time / t.attack;
                        return;
                    }
                    dt -= left.max(0.0);
                    self.gain = 1.0;
                    self.enter(Stage::Hold);
                }
                Stage::Hold => {
                    let left = t.hold - self.time;
                    self.gain = 1.0;
                    if dt < left {
                        self.time += dt;
                        return;
                    }
                    dt -= left.max(0.0);
                    self.attenuation = 0.0;
                    self.enter(Stage::Decay);
                }
                Stage::Decay => {
                    if t.decay <= 0.0 {
                        self.attenuation = t.sustain;
                    } else {
                        self.attenuation += SILENCE_CENTIBELS / t.decay * dt;
                    }
                    if self.attenuation >= t.sustain {
                        self.attenuation = t.sustain;
                        self.stage = Stage::Sustain;
                    }
                    self.update_gain();
                    return;
                }
                Stage::Sustain => {
                    self.update_gain();
                    return;
                }
                Stage::Release => {
                    if t.release <= 0.0 {
                        self.attenuation = SILENCE_CENTIBELS;
                    } else {
                        self.attenuation += SILENCE_CENTIBELS / t.release * dt;
                    }
                    self.update_gain();
                    return;
                }
                Stage::Done => {
                    self.gain = 0.0;
                    return;
                }
            }
        }
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.time = 0.0;
    }

    fn update_gain(&mut self) {
        if self.attenuation >= SILENCE_CENTIBELS {
            self.stage = Stage::Done;
            self.gain = 0.0;
        } else {
            self.gain = centibels_to_gain(self.attenuation);
        }
    }
}

/// The SF2 modulation envelope: linear segments between 0 and 1.
#[derive(Clone, Debug)]
pub(crate) struct ModulationEnvelope {
    times: EnvelopeTimes,
    stage: Stage,
    time: f32,
    value: f32,
}

impl Default for ModulationEnvelope {
    fn default() -> Self {
        ModulationEnvelope { times: EnvelopeTimes::default(), stage: Stage::Done, time: 0.0, value: 0.0 }
    }
}

impl ModulationEnvelope {
    pub fn start(&mut self, times: EnvelopeTimes) {
        self.times = times;
        self.stage = Stage::Delay;
        self.time = 0.0;
        self.value = 0.0;
    }

    pub fn release(&mut self, release_seconds: f32) {
        if self.stage != Stage::Done {
            self.times.release = release_seconds;
            self.stage = Stage::Release;
            self.time = 0.0;
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn advance(&mut self, mut dt: f32) {
        let t = self.times;
        loop {
            match self.stage {
                Stage::Delay => {
                    let left = t.delay - self.time;
                    if dt < left {
                        self.time += dt;
                        return;
                    }
                    dt -= left.max(0.0);
                    self.stage = Stage::Attack;
                    self.time = 0.0;
                }
                Stage::Attack => {
                    let left = t.attack - self.time;
                    if dt < left {
                        self.time += dt;
                        self.value = self.time / t.attack;
                        return;
                    }
                    dt -= left.max(0.0);
                    self.value = 1.0;
                    self.stage = Stage::Hold;
                    self.time = 0.0;
                }
                Stage::Hold => {
                    let left = t.hold - self.time;
                    if dt < left {
                        self.time += dt;
                        return;
                    }
                    dt -= left.max(0.0);
                    self.stage = Stage::Decay;
                    self.time = 0.0;
                }
                Stage::Decay => {
                    self.value = if t.decay <= 0.0 { t.sustain } else { self.value - dt / t.decay };
                    if self.value <= t.sustain {
                        self.value = t.sustain;
                        self.stage = Stage::Sustain;
                    }
                    return;
                }
                Stage::Sustain => return,
                Stage::Release => {
                    self.value = if t.release <= 0.0 { 0.0 } else { self.value - dt / t.release };
                    if self.value <= 0.0 {
                        self.value = 0.0;
                        self.stage = Stage::Done;
                    }
                    return;
                }
                Stage::Done => return,
            }
        }
    }
}

/// A triangle LFO (-1…1) that starts at 0, rising, after its delay.
#[derive(Clone, Debug, Default)]
pub(crate) struct Lfo {
    delay: f32,
    frequency: f32,
    time: f32,
}

impl Lfo {
    pub fn start(&mut self, delay: f32, frequency: f32) {
        self.delay = delay;
        self.frequency = frequency;
        self.time = 0.0;
    }

    pub fn value(&self) -> f32 {
        if self.time < self.delay {
            return 0.0;
        }
        let phase = ((self.time - self.delay) * self.frequency).fract();
        if phase < 0.25 {
            4.0 * phase
        } else if phase < 0.75 {
            2.0 - 4.0 * phase
        } else {
            4.0 * phase - 4.0
        }
    }

    pub fn advance(&mut self, dt: f32) {
        self.time += dt;
        // Keep the time small so it stays precise in f32.
        if self.time > self.delay + 1000.0 && self.frequency > 0.0 {
            let period = 1.0 / self.frequency;
            let cycles = ((self.time - self.delay) / period).floor();
            self.time -= cycles * period;
        }
    }
}

/// The SF2 resonant low-pass filter (a 2-pole biquad).
#[derive(Clone, Debug, Default)]
pub(crate) struct LowPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
    active: bool,
    cutoff: f32,
    resonance: f32,
}

impl LowPass {
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
        self.active = false;
        self.cutoff = -1.0;
        self.resonance = -1.0;
    }

    /// Sets the cutoff (absolute cents) and resonance (centibels above DC
    /// gain). At 13500 cents and no resonance the filter is bypassed.
    pub fn set(&mut self, cutoff_cents: f32, q_centibels: f32, sample_rate: f32) {
        if cutoff_cents == self.cutoff && q_centibels == self.resonance {
            return;
        }
        self.cutoff = cutoff_cents;
        self.resonance = q_centibels;
        let bypass = cutoff_cents >= 13500.0 && q_centibels <= 0.0;
        if bypass {
            if self.active {
                self.active = false;
                self.z1 = 0.0;
                self.z2 = 0.0;
            }
            return;
        }
        let nyquist_limit = 0.45 * sample_rate;
        let frequency = absolute_cents_to_hertz(cutoff_cents).clamp(5.0, nyquist_limit);
        // Q 0 cB is a flat (Butterworth) response; the resonance peak rises
        // by the given amount above that.
        let q = core::f32::consts::FRAC_1_SQRT_2 * 10f32.powf(q_centibels / 200.0);
        let w0 = 2.0 * core::f32::consts::PI * frequency / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 - cos) / 2.0 / a0;
        self.b1 = (1.0 - cos) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
        self.active = true;
    }

    pub fn process(&mut self, block: &mut [f32]) {
        if !self.active {
            return;
        }
        let (b0, b1, b2, a1, a2) = (self.b0, self.b1, self.b2, self.a1, self.a2);
        let (mut z1, mut z2) = (self.z1, self.z2);
        for x in block.iter_mut() {
            let input = *x;
            let y = b0 * input + z1;
            z1 = b1 * input - a1 * y + z2;
            z2 = b2 * input - a2 * y;
            *x = y;
        }
        // Flush denormals.
        if z1.abs() < 1e-20 {
            z1 = 0.0;
        }
        if z2.abs() < 1e-20 {
            z2 = 0.0;
        }
        self.z1 = z1;
        self.z2 = z2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_envelope_attack_hold_decay_sustain() {
        let mut env = VolumeEnvelope::default();
        env.start(EnvelopeTimes { delay: 0.0, attack: 0.1, hold: 0.1, decay: 1.0, sustain: 480.0, release: 0.5 });
        env.advance(0.05);
        assert!((env.gain() - 0.5).abs() < 1e-4);
        env.advance(0.1);
        assert_eq!(env.gain(), 1.0);
        // Halfway through the hold, then 0.25 s of decay = 240 cB.
        env.advance(0.05 + 0.25);
        assert!((env.gain() - centibels_to_gain(240.0)).abs() < 1e-3);
        env.advance(10.0);
        assert!((env.gain() - centibels_to_gain(480.0)).abs() < 1e-4);
    }

    #[test]
    fn volume_envelope_release_reaches_silence_in_the_release_time_from_full_level() {
        let mut env = VolumeEnvelope::default();
        env.start(EnvelopeTimes { delay: 0.0, attack: 0.0, hold: 0.0, decay: 0.0, sustain: 0.0, release: 0.0 });
        env.advance(0.01);
        assert_eq!(env.gain(), 1.0);
        env.release(1.0);
        env.advance(0.5);
        assert!((env.gain() - centibels_to_gain(480.0)).abs() < 1e-4);
        env.advance(0.51);
        assert!(env.is_done());
    }

    #[test]
    fn lfo_is_a_triangle() {
        let mut lfo = Lfo::default();
        lfo.start(0.0, 1.0);
        lfo.advance(0.25);
        assert!((lfo.value() - 1.0).abs() < 1e-4);
        lfo.advance(0.5);
        assert!((lfo.value() + 1.0).abs() < 1e-4);
    }
}
