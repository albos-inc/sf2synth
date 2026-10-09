//! Band-limited interpolation: windowed-sinc tables.
//!
//! A point between samples is a weighted sum of the 32 sample points around
//! it, the weights a sinc windowed with a Blackman-Harris window (side lobes
//! at −92 dB). Played higher than recorded, a sample has to lose its top
//! octave(s) or they fold back as aliasing; so there is one table per
//! pitch-ratio band, each a low-pass for the band's highest ratio.

use std::sync::OnceLock;

/// Points a value is computed from: `index - 15 ..= index + 16`.
pub(crate) const TAPS: usize = 32;
/// The first tap's offset from the point's index.
pub(crate) const FIRST_TAP: isize = 1 - (TAPS as isize) / 2;
/// Partial sums of a dot product.
const LANES: usize = 8;
/// Fractional positions per table (values in between are interpolated).
const PHASES: usize = 256;
/// Bands: the highest pitch ratio (source points per output sample) of the
/// first, the last, and how many in all (equally spaced in pitch). Above the
/// last one, some aliasing remains.
const FIRST_BAND: f32 = 1.1;
const LAST_BAND: f32 = 4.0;
const BANDS: usize = 12;
/// Width of the window's transition band, as a fraction of Nyquist.
const TRANSITION: f32 = 8.0 / TAPS as f32;
/// Aliasing may fold back down to this fraction of the output's Nyquist
/// frequency (above 19.8 kHz at 44.1 kHz): content up to `ALIAS_FLOOR`
/// above Nyquist is kept. So a sample played up to 10% higher than
/// recorded loses nothing.
const ALIAS_FLOOR: f32 = 0.9;

pub(crate) struct SincTable {
    /// `[band][phase 0..=PHASES][tap]`.
    weights: Vec<f32>,
}

impl SincTable {
    /// The tables, computed once per process.
    pub fn shared() -> &'static SincTable {
        static TABLE: OnceLock<SincTable> = OnceLock::new();
        TABLE.get_or_init(SincTable::new)
    }

    fn new() -> SincTable {
        let mut weights = Vec::with_capacity(BANDS * (PHASES + 1) * TAPS);
        for band in 0..BANDS {
            // Content above the stop edge (as a fraction of the source's
            // Nyquist frequency) would alias below ALIAS_FLOOR: put the
            // cutoff half a transition below it.
            let stop = (2.0 - ALIAS_FLOOR) / band_ratio(band);
            let cutoff = (stop - TRANSITION / 2.0).min(1.0);
            for phase in 0..=PHASES {
                let fraction = phase as f64 / PHASES as f64;
                let start = weights.len();
                for tap in 0..TAPS {
                    // Distance from the point computed to this sample point.
                    let x = (tap as isize + FIRST_TAP) as f64 - fraction;
                    weights.push(kernel(x, cutoff as f64) as f32);
                }
                // Unity gain at DC.
                let sum: f32 = weights[start..].iter().sum();
                weights[start..].iter_mut().for_each(|w| *w /= sum);
            }
        }
        SincTable { weights }
    }

    /// The table for playing at `ratio` source points per output sample.
    #[inline]
    pub fn band(&self, ratio: f32) -> Band<'_> {
        let band = (0..BANDS).find(|&band| ratio <= band_ratio(band) * 1.0001).unwrap_or(BANDS - 1);
        let size = (PHASES + 1) * TAPS;
        Band { weights: &self.weights[band * size..(band + 1) * size] }
    }
}

/// The highest ratio of `band`.
fn band_ratio(band: usize) -> f32 {
    FIRST_BAND * (LAST_BAND / FIRST_BAND).powf(band as f32 / (BANDS - 1) as f32)
}

/// A windowed sinc low-pass at `cutoff` (fraction of Nyquist), `x` points
/// from its center.
fn kernel(x: f64, cutoff: f64) -> f64 {
    use std::f64::consts::PI;
    let half = (TAPS / 2) as f64;
    if x.abs() >= half {
        return 0.0;
    }
    let sinc = if x == 0.0 { 1.0 } else { (PI * cutoff * x).sin() / (PI * cutoff * x) };
    // 4-term Blackman-Harris over -half…half.
    let t = PI * x / half;
    let window = 0.35875 + 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() + 0.01168 * (3.0 * t).cos();
    cutoff * sinc * window
}

/// The weights of one band.
#[derive(Clone, Copy)]
pub(crate) struct Band<'a> {
    weights: &'a [f32],
}

impl Band<'_> {
    /// The value at `fraction` (0…1) past `points[TAPS / 2 - 1]`.
    #[inline(always)]
    pub fn interpolate(&self, points: &[f32; TAPS], fraction: f32) -> f32 {
        let position = fraction * PHASES as f32;
        let phase = (position as usize).min(PHASES - 1);
        let between = position - phase as f32;
        let (a, b) = self.weights[phase * TAPS..(phase + 2) * TAPS].split_at(TAPS);
        // The two neighboring phases blended, then one dot product, in
        // LANES partial sums (which the compiler turns into SIMD).
        let mut sums = [0.0f32; LANES];
        for ((a, b), points) in a.chunks_exact(LANES).zip(b.chunks_exact(LANES)).zip(points.chunks_exact(LANES)) {
            for lane in 0..LANES {
                let weight = a[lane] + (b[lane] - a[lane]) * between;
                sums[lane] += weight * points[lane];
            }
        }
        sums.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_constant_signal_constant() {
        let table = SincTable::shared();
        let ones = [1.0; TAPS];
        for ratio in [0.5, 1.0, 1.3, 2.0, 8.0] {
            for fraction in [0.0, 0.3, 0.5, 0.99] {
                assert!((table.band(ratio).interpolate(&ones, fraction) - 1.0).abs() < 1e-5);
            }
        }
    }
}
