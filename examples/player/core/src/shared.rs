//! A player shared by a UI thread and an audio callback.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::{Player, SongSummary, SoundFontSummary, State};

/// A [`Player`] for an app whose UI and audio callback run on different
/// threads.
///
/// The audio callback renders with [`render`](Self::render), which only
/// ever *tries* the player's lock (playing silence for that buffer if the
/// UI holds it) and never allocates. The UI changes the player with
/// [`with`](Self::with), taking the lock only for that, and reads its state
/// with [`status`](Self::status), [`song`](Self::song) and
/// [`sound_font`](Self::sound_font) without touching the lock at all: the
/// status is published in atomics after every change and every buffer, so
/// polling it many times a second can't make the audio drop out.
pub struct SharedPlayer {
    sample_rate: u32,
    inner: Mutex<Inner>,
    status: Status,
    /// The files' summaries, which only change on loading (never touched
    /// by the audio callback).
    summaries: Mutex<(Option<SongSummary>, Option<SoundFontSummary>)>,
}

struct Inner {
    player: Player,
    /// Scratch buffers for interleaved rendering.
    left: Vec<f32>,
    right: Vec<f32>,
}

#[derive(Default)]
struct Status {
    state: AtomicI32,
    can_play: AtomicBool,
    position: AtomicU64,
    duration: AtomicU64,
    active_voices: AtomicU32,
}

/// What [`SharedPlayer::status`] reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerStatus {
    pub state: State,
    pub can_play: bool,
    /// Seconds into the song.
    pub position: f64,
    /// The song's length in seconds.
    pub duration: f64,
    pub active_voices: usize,
}

impl SharedPlayer {
    pub fn new(sample_rate: u32) -> SharedPlayer {
        let inner = Inner { player: Player::new(sample_rate), left: Vec::new(), right: Vec::new() };
        let shared = SharedPlayer {
            sample_rate,
            inner: Mutex::new(inner),
            status: Status::default(),
            summaries: Mutex::new((None, None)),
        };
        shared.publish(&shared.lock().player);
        shared
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the player under its lock, then publishes the status
    /// and summaries. For the UI's actions: play, seek, load…
    pub fn with<T>(&self, f: impl FnOnce(&mut Player) -> T) -> T {
        let mut inner = self.lock();
        let result = f(&mut inner.player);
        self.publish(&inner.player);
        let summaries = (inner.player.song().cloned(), inner.player.sound_font().cloned());
        drop(inner);
        *self.summaries.lock().unwrap_or_else(|e| e.into_inner()) = summaries;
        result
    }

    /// The player's state, position and voices, without locking.
    pub fn status(&self) -> PlayerStatus {
        let status = &self.status;
        PlayerStatus {
            state: match status.state.load(Ordering::Relaxed) {
                1 => State::Playing,
                2 => State::Paused,
                _ => State::Stopped,
            },
            can_play: status.can_play.load(Ordering::Relaxed),
            position: f64::from_bits(status.position.load(Ordering::Relaxed)),
            duration: f64::from_bits(status.duration.load(Ordering::Relaxed)),
            active_voices: status.active_voices.load(Ordering::Relaxed) as usize,
        }
    }

    /// The song loaded, if any (without the player's lock).
    pub fn song(&self) -> Option<SongSummary> {
        self.summaries.lock().unwrap_or_else(|e| e.into_inner()).0.clone()
    }

    /// The SoundFont loaded, if any (without the player's lock).
    pub fn sound_font(&self) -> Option<SoundFontSummary> {
        self.summaries.lock().unwrap_or_else(|e| e.into_inner()).1.clone()
    }

    fn publish(&self, player: &Player) {
        let status = &self.status;
        status.state.store(player.state().code(), Ordering::Relaxed);
        status.can_play.store(player.can_play(), Ordering::Relaxed);
        status.position.store(player.position().to_bits(), Ordering::Relaxed);
        status.duration.store(player.duration().to_bits(), Ordering::Relaxed);
        status.active_voices.store(player.active_voices() as u32, Ordering::Relaxed);
    }

    /// Renders into separate channels. Real-time safe. False (and silence)
    /// if the UI held the lock.
    pub fn render(&self, left: &mut [f32], right: &mut [f32]) -> bool {
        let Ok(mut inner) = self.inner.try_lock() else {
            left.fill(0.0);
            right.fill(0.0);
            return false;
        };
        inner.player.render(left, right);
        self.publish(&inner.player);
        true
    }

    /// Renders `output.len() / 2` frames of interleaved stereo. Real-time
    /// safe once the scratch buffers have grown to the callback's size
    /// (the first call of a size allocates). False (leaving `output`
    /// untouched) if the UI held the lock.
    pub fn render_interleaved(&self, output: &mut [f32]) -> bool {
        let Ok(mut inner) = self.inner.try_lock() else { return false };
        let frames = output.len() / 2;
        let Inner { player, left, right } = &mut *inner;
        if left.len() < frames {
            left.resize(frames, 0.0);
            right.resize(frames, 0.0);
        }
        player.render(&mut left[..frames], &mut right[..frames]);
        for (i, frame) in output.chunks_exact_mut(2).enumerate() {
            frame[0] = left[i];
            frame[1] = right[i];
        }
        self.publish(player);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_follows_without_the_lock() {
        let shared = SharedPlayer::new(48_000);
        assert_eq!(shared.status().state, State::Stopped);
        assert!(!shared.status().can_play);
        // While the UI holds the lock, the audio renders silence and the
        // status can still be read.
        let guard = shared.lock();
        let (mut left, mut right) = (vec![1.0; 64], vec![1.0; 64]);
        assert!(!shared.render(&mut left, &mut right));
        assert!(left.iter().all(|&x| x == 0.0));
        assert_eq!(shared.status().position, 0.0);
        drop(guard);
        assert!(shared.render(&mut left, &mut right));
        assert!(shared.song().is_none());
    }
}
