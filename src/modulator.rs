//! SF2 modulators (SoundFont 2.04, section 8.2).

use crate::generator as g;

/// A modulator: `amount × source × amount_source`, added to the generator
/// `destination`.
///
/// `source` and `amount_source` use the SF2 bit layout:
/// bits 0–6 controller index, bit 7 MIDI CC flag, bit 8 direction
/// (1 = max → min), bit 9 polarity (1 = bipolar), bits 10–15 curve
/// (0 linear, 1 concave, 2 convex, 3 switch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modulator {
    pub source: u16,
    pub destination: u16,
    pub amount: i16,
    pub amount_source: u16,
    /// 0 linear, 2 absolute value.
    pub transform: u16,
}

/// General controller indices (CC flag clear).
pub mod controller {
    pub const NONE: u16 = 0;
    pub const NOTE_ON_VELOCITY: u16 = 2;
    pub const NOTE_ON_KEY: u16 = 3;
    pub const POLY_PRESSURE: u16 = 10;
    pub const CHANNEL_PRESSURE: u16 = 13;
    pub const PITCH_WHEEL: u16 = 14;
    pub const PITCH_WHEEL_SENSITIVITY: u16 = 16;
    pub const LINK: u16 = 127;
}

/// Flags of a modulator source.
pub mod source {
    pub const CC: u16 = 0x80;
    pub const NEGATIVE: u16 = 0x100;
    pub const BIPOLAR: u16 = 0x200;
    pub const LINEAR: u16 = 0 << 10;
    pub const CONCAVE: u16 = 1 << 10;
    pub const CONVEX: u16 = 2 << 10;
    pub const SWITCH: u16 = 3 << 10;
}

impl Modulator {
    /// The default modulators of SoundFont 2.04 (section 8.4). Pitch bend
    /// targets [`g::PITCH`].
    pub const SF2_DEFAULTS: [Modulator; 10] = [
        // Note-on velocity → initial attenuation (960 cB, concave).
        Modulator {
            source: controller::NOTE_ON_VELOCITY | source::NEGATIVE | source::CONCAVE,
            destination: g::INITIAL_ATTENUATION,
            amount: 960,
            amount_source: 0,
            transform: 0,
        },
        // Note-on velocity → filter cutoff (-2400 cents, linear).
        Modulator {
            source: controller::NOTE_ON_VELOCITY | source::NEGATIVE,
            destination: g::INITIAL_FILTER_FC,
            amount: -2400,
            amount_source: 0,
            transform: 0,
        },
        // Channel pressure → vibrato LFO pitch depth (50 cents).
        Modulator {
            source: controller::CHANNEL_PRESSURE,
            destination: g::VIB_LFO_TO_PITCH,
            amount: 50,
            amount_source: 0,
            transform: 0,
        },
        // CC1 modulation wheel → vibrato LFO pitch depth (50 cents).
        Modulator {
            source: 1 | source::CC,
            destination: g::VIB_LFO_TO_PITCH,
            amount: 50,
            amount_source: 0,
            transform: 0,
        },
        // CC7 volume → initial attenuation (960 cB, concave).
        Modulator {
            source: 7 | source::CC | source::NEGATIVE | source::CONCAVE,
            destination: g::INITIAL_ATTENUATION,
            amount: 960,
            amount_source: 0,
            transform: 0,
        },
        // CC10 pan → pan (±50 %).
        Modulator {
            source: 10 | source::CC | source::BIPOLAR,
            destination: g::PAN,
            amount: 1000,
            amount_source: 0,
            transform: 0,
        },
        // CC11 expression → initial attenuation (960 cB, concave).
        Modulator {
            source: 11 | source::CC | source::NEGATIVE | source::CONCAVE,
            destination: g::INITIAL_ATTENUATION,
            amount: 960,
            amount_source: 0,
            transform: 0,
        },
        // CC91 → reverb send (20 %).
        Modulator {
            source: 91 | source::CC,
            destination: g::REVERB_EFFECTS_SEND,
            amount: 200,
            amount_source: 0,
            transform: 0,
        },
        // CC93 → chorus send (20 %).
        Modulator {
            source: 93 | source::CC,
            destination: g::CHORUS_EFFECTS_SEND,
            amount: 200,
            amount_source: 0,
            transform: 0,
        },
        // Pitch wheel × pitch wheel sensitivity → pitch (12700 cents).
        Modulator {
            source: controller::PITCH_WHEEL | source::BIPOLAR,
            destination: g::PITCH,
            amount: 12700,
            amount_source: controller::PITCH_WHEEL_SENSITIVITY,
            transform: 0,
        },
    ];

    /// Whether two modulators are "identical" in the sense of SF2 2.04
    /// 9.5.1: same sources and destination (amount and transform may differ).
    pub(crate) fn same_kind(&self, other: &Modulator) -> bool {
        self.source == other.source
            && self.destination == other.destination
            && self.amount_source == other.amount_source
    }
}

/// What a modulator source reads, normalized to 0…1.
pub(crate) struct SourceInput<'a> {
    pub cc: &'a [u8; 128],
    pub velocity: u8,
    pub key: u8,
    pub channel_pressure: u8,
    pub pitch_wheel: u16,
    /// Semitones.
    pub pitch_wheel_sensitivity: f32,
}

impl SourceInput<'_> {
    /// The source's raw value in 0…1, or `None` when the modulator must be
    /// ignored (unknown or linked source).
    fn raw(&self, source: u16) -> Option<f32> {
        let index = source & 0x7f;
        if source & source::CC != 0 {
            // Bank select, data entry, (N)RPN and channel mode messages are
            // not valid modulator sources.
            return match index {
                0 | 6 | 32 | 38 | 98..=101 | 120..=127 => None,
                _ => Some(self.cc[index as usize] as f32 / 127.0),
            };
        }
        match index {
            controller::NONE => Some(1.0),
            controller::NOTE_ON_VELOCITY => Some(self.velocity as f32 / 127.0),
            controller::NOTE_ON_KEY => Some(self.key as f32 / 127.0),
            controller::POLY_PRESSURE => Some(0.0),
            controller::CHANNEL_PRESSURE => Some(self.channel_pressure as f32 / 127.0),
            controller::PITCH_WHEEL => Some(self.pitch_wheel as f32 / 16384.0),
            controller::PITCH_WHEEL_SENSITIVITY => Some(self.pitch_wheel_sensitivity / 127.0),
            _ => None,
        }
    }

    /// The source mapped through its direction, polarity and curve.
    pub fn value(&self, source: u16) -> Option<f32> {
        let index = source & 0x7f;
        if source & source::CC == 0 && index == controller::NONE {
            return Some(1.0);
        }
        let mut x = self.raw(source)?;
        if source & source::NEGATIVE != 0 {
            x = 1.0 - x;
        }
        let curve = source >> 10;
        Some(if source & source::BIPOLAR != 0 {
            let t = 2.0 * x - 1.0;
            t.signum() * apply_curve(curve, t.abs())
        } else {
            apply_curve(curve, x)
        })
    }
}

fn apply_curve(curve: u16, x: f32) -> f32 {
    match curve {
        1 => concave(x),
        2 => 1.0 - concave(1.0 - x),
        3 => {
            if x >= 0.5 {
                1.0
            } else {
                0.0
            }
        }
        _ => x,
    }
}

/// The SF2 concave curve: `-(20/96)·log10((1 - x)²)`, the attenuation
/// (as a fraction of 96 dB) of an amplitude that follows `(1 - x)²`.
fn concave(x: f32) -> f32 {
    if x >= 1.0 {
        1.0
    } else if x <= 0.0 {
        0.0
    } else {
        (-(400.0 / 960.0) * (1.0 - x).log10()).min(1.0)
    }
}

/// The value one modulator adds to its destination.
pub(crate) fn evaluate(modulator: &Modulator, input: &SourceInput<'_>) -> f32 {
    let Some(primary) = input.value(modulator.source) else {
        return 0.0;
    };
    let Some(secondary) = input.value(modulator.amount_source) else {
        return 0.0;
    };
    let value = modulator.amount as f32 * primary * secondary;
    if modulator.transform == 2 { value.abs() } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(cc: &[u8; 128], velocity: u8) -> SourceInput<'_> {
        SourceInput { cc, velocity, key: 60, channel_pressure: 0, pitch_wheel: 8192, pitch_wheel_sensitivity: 2.0 }
    }

    #[test]
    fn velocity_attenuation_follows_the_square_law() {
        let cc = [0; 128];
        let full = evaluate(&Modulator::SF2_DEFAULTS[0], &input(&cc, 127));
        assert!(full.abs() < 1e-3);
        // Velocity 64 is (64/127)² in amplitude: 11.95 dB = 119.5 cB.
        let half = evaluate(&Modulator::SF2_DEFAULTS[0], &input(&cc, 64));
        let expected = -400.0 * (64.0f32 / 127.0).log10();
        assert!((half - expected).abs() < 0.5, "{half} vs {expected}");
    }

    #[test]
    fn centered_pitch_wheel_is_zero_and_full_bend_is_the_sensitivity() {
        let cc = [0; 128];
        let mut i = input(&cc, 100);
        assert!(evaluate(&Modulator::SF2_DEFAULTS[9], &i).abs() < 1e-3);
        i.pitch_wheel = 16384;
        assert!((evaluate(&Modulator::SF2_DEFAULTS[9], &i) - 200.0).abs() < 0.1);
        i.pitch_wheel = 0;
        assert!((evaluate(&Modulator::SF2_DEFAULTS[9], &i) + 200.0).abs() < 0.1);
    }

    #[test]
    fn centered_pan_is_zero() {
        let mut cc = [0; 128];
        cc[10] = 64;
        let pan = evaluate(&Modulator::SF2_DEFAULTS[5], &input(&cc, 100));
        assert!(pan.abs() < 10.0);
    }
}
