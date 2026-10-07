//! SF2 generator numbers (SoundFont 2.04, section 8.1.2).
//!
//! A generator sets one synthesis parameter of a zone. Values are in the
//! units of the specification: timecents, centibels, absolute cents, 0.1 %.

pub const START_ADDRS_OFFSET: u16 = 0;
pub const END_ADDRS_OFFSET: u16 = 1;
pub const STARTLOOP_ADDRS_OFFSET: u16 = 2;
pub const ENDLOOP_ADDRS_OFFSET: u16 = 3;
pub const START_ADDRS_COARSE_OFFSET: u16 = 4;
pub const MOD_LFO_TO_PITCH: u16 = 5;
pub const VIB_LFO_TO_PITCH: u16 = 6;
pub const MOD_ENV_TO_PITCH: u16 = 7;
pub const INITIAL_FILTER_FC: u16 = 8;
pub const INITIAL_FILTER_Q: u16 = 9;
pub const MOD_LFO_TO_FILTER_FC: u16 = 10;
pub const MOD_ENV_TO_FILTER_FC: u16 = 11;
pub const END_ADDRS_COARSE_OFFSET: u16 = 12;
pub const MOD_LFO_TO_VOLUME: u16 = 13;
pub const CHORUS_EFFECTS_SEND: u16 = 15;
pub const REVERB_EFFECTS_SEND: u16 = 16;
pub const PAN: u16 = 17;
pub const DELAY_MOD_LFO: u16 = 21;
pub const FREQ_MOD_LFO: u16 = 22;
pub const DELAY_VIB_LFO: u16 = 23;
pub const FREQ_VIB_LFO: u16 = 24;
pub const DELAY_MOD_ENV: u16 = 25;
pub const ATTACK_MOD_ENV: u16 = 26;
pub const HOLD_MOD_ENV: u16 = 27;
pub const DECAY_MOD_ENV: u16 = 28;
pub const SUSTAIN_MOD_ENV: u16 = 29;
pub const RELEASE_MOD_ENV: u16 = 30;
pub const KEYNUM_TO_MOD_ENV_HOLD: u16 = 31;
pub const KEYNUM_TO_MOD_ENV_DECAY: u16 = 32;
pub const DELAY_VOL_ENV: u16 = 33;
pub const ATTACK_VOL_ENV: u16 = 34;
pub const HOLD_VOL_ENV: u16 = 35;
pub const DECAY_VOL_ENV: u16 = 36;
pub const SUSTAIN_VOL_ENV: u16 = 37;
pub const RELEASE_VOL_ENV: u16 = 38;
pub const KEYNUM_TO_VOL_ENV_HOLD: u16 = 39;
pub const KEYNUM_TO_VOL_ENV_DECAY: u16 = 40;
pub const INSTRUMENT: u16 = 41;
pub const KEY_RANGE: u16 = 43;
pub const VEL_RANGE: u16 = 44;
pub const STARTLOOP_ADDRS_COARSE_OFFSET: u16 = 45;
pub const KEYNUM: u16 = 46;
pub const VELOCITY: u16 = 47;
pub const INITIAL_ATTENUATION: u16 = 48;
pub const ENDLOOP_ADDRS_COARSE_OFFSET: u16 = 50;
pub const COARSE_TUNE: u16 = 51;
pub const FINE_TUNE: u16 = 52;
pub const SAMPLE_ID: u16 = 53;
pub const SAMPLE_MODES: u16 = 54;
pub const SCALE_TUNING: u16 = 56;
pub const EXCLUSIVE_CLASS: u16 = 57;
pub const OVERRIDING_ROOT_KEY: u16 = 58;

/// Number of generator numbers defined by the specification (0…60).
pub const COUNT: usize = 61;

/// Destination of the default pitch-wheel modulator. The specification
/// names it "initial pitch" but gives it no generator number, so this crate
/// uses 61, which no SF2 file can contain. Its unit is cents.
pub const PITCH: u16 = 61;

/// Number of modulatable parameters: the generators plus [`PITCH`].
pub(crate) const PARAM_COUNT: usize = COUNT + 1;

/// Default value of each generator.
pub(crate) const fn default_value(generator: u16) -> i16 {
    match generator {
        INITIAL_FILTER_FC => 13500,
        DELAY_MOD_LFO | DELAY_VIB_LFO | DELAY_MOD_ENV | ATTACK_MOD_ENV | HOLD_MOD_ENV | DECAY_MOD_ENV
        | RELEASE_MOD_ENV | DELAY_VOL_ENV | ATTACK_VOL_ENV | HOLD_VOL_ENV | DECAY_VOL_ENV | RELEASE_VOL_ENV => -12000,
        KEYNUM | VELOCITY | OVERRIDING_ROOT_KEY => -1,
        SCALE_TUNING => 100,
        _ => 0,
    }
}

/// Range the summed value of a generator is clamped to, if it has one.
pub(crate) const fn range(generator: u16) -> Option<(f32, f32)> {
    Some(match generator {
        MOD_LFO_TO_PITCH | VIB_LFO_TO_PITCH | MOD_ENV_TO_PITCH => (-12000.0, 12000.0),
        INITIAL_FILTER_FC => (1500.0, 13500.0),
        INITIAL_FILTER_Q => (0.0, 960.0),
        MOD_LFO_TO_FILTER_FC | MOD_ENV_TO_FILTER_FC => (-12000.0, 12000.0),
        MOD_LFO_TO_VOLUME => (-960.0, 960.0),
        CHORUS_EFFECTS_SEND | REVERB_EFFECTS_SEND => (0.0, 1000.0),
        PAN => (-500.0, 500.0),
        DELAY_MOD_LFO | DELAY_VIB_LFO | DELAY_MOD_ENV | DELAY_VOL_ENV => (-12000.0, 5000.0),
        FREQ_MOD_LFO | FREQ_VIB_LFO => (-16000.0, 4500.0),
        ATTACK_MOD_ENV | ATTACK_VOL_ENV => (-12000.0, 8000.0),
        HOLD_MOD_ENV | HOLD_VOL_ENV => (-12000.0, 5000.0),
        DECAY_MOD_ENV | DECAY_VOL_ENV => (-12000.0, 8000.0),
        SUSTAIN_MOD_ENV => (0.0, 1000.0),
        SUSTAIN_VOL_ENV => (0.0, 1440.0),
        RELEASE_MOD_ENV | RELEASE_VOL_ENV => (-12000.0, 8000.0),
        KEYNUM_TO_MOD_ENV_HOLD | KEYNUM_TO_MOD_ENV_DECAY | KEYNUM_TO_VOL_ENV_HOLD | KEYNUM_TO_VOL_ENV_DECAY => {
            (-1200.0, 1200.0)
        }
        INITIAL_ATTENUATION => (0.0, 1440.0),
        COARSE_TUNE => (-120.0, 120.0),
        FINE_TUNE => (-99.0, 99.0),
        SCALE_TUNING => (0.0, 1200.0),
        _ => return None,
    })
}

/// Whether a preset zone may set the generator (SF2 2.04, 8.1.2: sample
/// addresses, key/velocity overrides, sample modes, exclusive class and
/// root key are instrument-only).
pub(crate) const fn allowed_in_preset(generator: u16) -> bool {
    !matches!(
        generator,
        START_ADDRS_OFFSET
            | END_ADDRS_OFFSET
            | STARTLOOP_ADDRS_OFFSET
            | ENDLOOP_ADDRS_OFFSET
            | START_ADDRS_COARSE_OFFSET
            | END_ADDRS_COARSE_OFFSET
            | STARTLOOP_ADDRS_COARSE_OFFSET
            | ENDLOOP_ADDRS_COARSE_OFFSET
            | KEYNUM
            | VELOCITY
            | SAMPLE_MODES
            | EXCLUSIVE_CLASS
            | OVERRIDING_ROOT_KEY
    )
}

/// The generators a zone sets.
#[derive(Clone, Debug)]
pub(crate) struct GeneratorSet {
    values: [i16; COUNT],
    set: u64,
}

impl Default for GeneratorSet {
    fn default() -> Self {
        GeneratorSet { values: [0; COUNT], set: 0 }
    }
}

impl GeneratorSet {
    pub fn set(&mut self, generator: u16, value: i16) {
        if (generator as usize) < COUNT {
            self.values[generator as usize] = value;
            self.set |= 1 << generator;
        }
    }

    pub fn get(&self, generator: u16) -> Option<i16> {
        if (generator as usize) < COUNT && self.set & (1 << generator) != 0 {
            Some(self.values[generator as usize])
        } else {
            None
        }
    }

    /// `self` with every generator of `local` replacing its own: how a
    /// zone's generators override its global zone's.
    pub fn overridden_by(&self, local: &GeneratorSet) -> GeneratorSet {
        let mut merged = self.clone();
        for generator in 0..COUNT as u16 {
            if let Some(value) = local.get(generator) {
                merged.set(generator, value);
            }
        }
        merged
    }
}
