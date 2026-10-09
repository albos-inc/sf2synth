//! The output limiter: keeps the mix below full scale without clipping.

/// The level the output never exceeds (−0.45 dBFS).
pub(crate) const CEILING: f32 = 0.95;
/// How far ahead the limiter looks, in seconds: gain reductions start this
/// much before the peak they make room for, as a ramp instead of a step.
const LOOKAHEAD_SECONDS: f32 = 0.001;
/// How fast the gain comes back after a peak (time constant, seconds).
const RELEASE_SECONDS: f32 = 0.15;

/// A look-ahead peak limiter, linked across both channels.
///
/// For every sample it works out the gain that would bring it down to the
/// ceiling; the gain applied is the smallest of those over the look-ahead,
/// averaged over the look-ahead (a ramp down to it), then released
/// smoothly. The output is the input delayed by the look-ahead minus one
/// frame, and never above [`CEILING`]. Signals that stay below it pass
/// unchanged (apart from the delay).
pub(crate) struct Limiter {
    pub enabled: bool,
    /// Look-ahead frames: the length of the rings below.
    length: usize,
    left: Vec<f32>,
    right: Vec<f32>,
    /// The smallest required gain over the look-ahead, kept as a monotonic
    /// queue (ring) of (gain, frame) candidates: O(1) per frame.
    queue_gain: Vec<f32>,
    queue_frame: Vec<u64>,
    queue_head: usize,
    queue_len: usize,
    frame: u64,
    /// The smallest required gain over the look-ahead, at each frame, and
    /// the running sum of those (for their average).
    minimum: Vec<f32>,
    sum: f64,
    write: usize,
    /// Frames since one needed a gain below 1.
    since_over: usize,
    gain: f32,
    release: f32,
}

impl Limiter {
    /// Look-ahead frames at `sample_rate`.
    pub fn length(sample_rate: f32) -> usize {
        ((LOOKAHEAD_SECONDS as f64 * sample_rate as f64).round() as usize).max(1)
    }

    pub fn new(sample_rate: f32, enabled: bool) -> Limiter {
        let length = Self::length(sample_rate);
        Limiter {
            enabled,
            length,
            left: vec![0.0; length],
            right: vec![0.0; length],
            queue_gain: vec![1.0; length],
            queue_frame: vec![0; length],
            queue_head: 0,
            queue_len: 0,
            frame: 0,
            minimum: vec![1.0; length],
            sum: length as f64,
            write: 0,
            since_over: usize::MAX,
            gain: 1.0,
            release: 1.0 - (-1.0 / (RELEASE_SECONDS * sample_rate)).exp(),
        }
    }

    /// Frames the output is delayed by (while enabled).
    pub fn latency(&self) -> usize {
        self.length - 1
    }

    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.queue_len = 0;
        self.minimum.fill(1.0);
        self.sum = self.length as f64;
        self.since_over = usize::MAX;
        self.gain = 1.0;
    }

    /// Adds this frame's required gain to the queue; returns the smallest
    /// over the look-ahead.
    #[inline]
    fn push(&mut self, required: f32) -> f32 {
        let length = self.length;
        // Candidates no smaller than the new one will never be the minimum.
        while self.queue_len > 0 {
            let back = (self.queue_head + self.queue_len - 1) % length;
            if self.queue_gain[back] < required {
                break;
            }
            self.queue_len -= 1;
        }
        // Candidates that left the look-ahead.
        while self.queue_len > 0 && self.queue_frame[self.queue_head] + length as u64 <= self.frame {
            self.queue_head = (self.queue_head + 1) % length;
            self.queue_len -= 1;
        }
        let at = (self.queue_head + self.queue_len) % length;
        self.queue_gain[at] = required;
        self.queue_frame[at] = self.frame;
        self.queue_len += 1;
        self.queue_gain[self.queue_head]
    }

    /// Limits `left` and `right` in place (delaying them by
    /// [`latency`](Self::latency) frames).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.enabled {
            return;
        }
        let length = self.length;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let peak = l.abs().max(r.abs());
            let required = if peak > CEILING { CEILING / peak } else { 1.0 };
            let w = self.write;
            self.since_over = if required < 1.0 { 0 } else { self.since_over.saturating_add(1) };
            // The smallest gain any frame still in the ring needs.
            let minimum = if self.since_over >= length {
                self.queue_len = 0;
                1.0
            } else {
                self.push(required)
            };
            self.frame += 1;
            self.sum += minimum as f64 - self.minimum[w] as f64;
            self.minimum[w] = minimum;
            if self.since_over >= 2 * length {
                // Every minimum is 1 again: drop any rounding error.
                self.sum = length as f64;
            }
            let average = (self.sum / length as f64) as f32;
            // Down with the (already ramped) average, back up slowly.
            self.gain = if average < self.gain { average } else { self.gain + (average - self.gain) * self.release };
            // Out goes the oldest frame, in comes this one.
            self.left[w] = *l;
            self.right[w] = *r;
            let oldest = if w + 1 == length { 0 } else { w + 1 };
            *l = self.left[oldest] * self.gain;
            *r = self.right[oldest] * self.gain;
            self.write = oldest;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_exceeds_the_ceiling() {
        let mut limiter = Limiter::new(48_000.0, true);
        // Bursts up to +12 dB over full scale, sudden and sustained.
        let mut left: Vec<f32> = (0..48_000)
            .map(|i| {
                let envelope = if (i / 3000) % 2 == 0 { 4.0 } else { 0.3 };
                envelope * (i as f32 * 0.05).sin()
            })
            .collect();
        left[10_000] = 8.0; // a lone spike
        let mut right = left.iter().map(|x| -x * 0.5).collect::<Vec<_>>();
        limiter.process(&mut left, &mut right);
        let peak = left.iter().chain(&right).fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!(peak <= CEILING * 1.0001, "{peak}");
    }

    #[test]
    fn quiet_signals_pass_unchanged_but_delayed() {
        let mut limiter = Limiter::new(44_100.0, true);
        let input: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.1).sin() * 0.9).collect();
        let (mut left, mut right) = (input.clone(), input.clone());
        limiter.process(&mut left, &mut right);
        let latency = limiter.latency();
        assert_eq!(latency, 43);
        assert!(left[..latency].iter().all(|&x| x == 0.0));
        assert_eq!(&left[latency..], &input[..input.len() - latency]);
    }

    #[test]
    fn the_gain_ramps_down_before_a_peak() {
        let mut limiter = Limiter::new(48_000.0, true);
        let mut left = vec![0.5f32; 400];
        left[200] = 2.0;
        let mut right = left.clone();
        limiter.process(&mut left, &mut right);
        let latency = limiter.latency();
        // The peak comes out at the ceiling, the frames before it a little
        // lower than they went in (the ramp), none of them cut abruptly.
        assert!((left[200 + latency] - CEILING).abs() < 1e-4);
        let steps = left.windows(2).skip(latency + 1).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(steps < 1.5, "{steps}");
        assert!(left[200 + latency - 10] < 0.5);
    }
}
