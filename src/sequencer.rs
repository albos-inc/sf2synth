//! Playing a MIDI file, sample-accurately.

use std::sync::Arc;

use crate::midi_file::{MidiEvent, MidiFile};
use crate::synth::Synthesizer;

/// Plays a [`MidiFile`] into a [`Synthesizer`] (or any MIDI sink), with
/// seeking, speed change, looping and track muting.
///
/// Events land on the exact frame they fall on, not on block boundaries.
/// Nothing allocates after [`load`](Self::load), so the sequencer can run in
/// an audio callback.
pub struct Sequencer {
    midi: Option<Arc<MidiFile>>,
    sample_rate: f64,
    /// Seconds into the file.
    position: f64,
    next: usize,
    speed: f64,
    playing: bool,
    looping: bool,
    muted: Vec<bool>,
}

impl Sequencer {
    pub fn new(sample_rate: u32) -> Sequencer {
        Sequencer {
            midi: None,
            sample_rate: sample_rate.max(1) as f64,
            position: 0.0,
            next: 0,
            speed: 1.0,
            playing: false,
            looping: false,
            muted: Vec::new(),
        }
    }

    /// Sets the file to play, from its start, paused. Unmutes every track.
    pub fn load(&mut self, midi: Arc<MidiFile>) {
        self.muted = vec![false; midi.track_count() as usize];
        self.midi = Some(midi);
        self.position = 0.0;
        self.next = 0;
        self.playing = false;
    }

    pub fn midi_file(&self) -> Option<&Arc<MidiFile>> {
        self.midi.as_ref()
    }

    /// Starts (or resumes) playing.
    pub fn play(&mut self) {
        if self.midi.is_some() {
            self.playing = true;
        }
    }

    /// Pauses. The sounding notes are released.
    pub fn pause(&mut self, synth: &mut Synthesizer) {
        if self.playing {
            self.playing = false;
            synth.note_off_all();
        }
    }

    /// Pauses without touching a synthesizer (for external MIDI output: send
    /// note-offs yourself).
    pub fn pause_silently(&mut self) {
        self.playing = false;
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Whether the file has played to its end (and isn't looping).
    pub fn is_finished(&self) -> bool {
        match &self.midi {
            Some(midi) => !self.looping && self.position >= midi.duration(),
            None => true,
        }
    }

    /// Seconds into the file.
    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn duration(&self) -> f64 {
        self.midi.as_ref().map_or(0.0, |m| m.duration())
    }

    /// Playback speed: 1.0 is the file's tempo.
    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed.clamp(0.01, 100.0);
    }

    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }

    pub fn is_looping(&self) -> bool {
        self.looping
    }

    /// Mutes or unmutes a track: its note-ons are skipped (notes already
    /// sounding go on). Controllers still apply.
    pub fn set_track_muted(&mut self, track: usize, muted: bool) {
        if let Some(m) = self.muted.get_mut(track) {
            *m = muted;
        }
    }

    pub fn is_track_muted(&self, track: usize) -> bool {
        self.muted.get(track).copied().unwrap_or(false)
    }

    /// Jumps to `seconds`. The synthesizer's notes are cut, then every
    /// program, controller and pitch bend before that point is replayed, so
    /// the music continues as it would have.
    pub fn seek(&mut self, seconds: f64, synth: &mut Synthesizer) {
        synth.sound_off_all();
        synth.reset_channels();
        self.seek_with(seconds, |_, message| synth.process_midi_message(message));
    }

    /// [`seek`](Self::seek) for an external sink: `sink(0, message)` gets
    /// the state-restoring messages (no note-ons). Send All Notes Off to the
    /// device yourself first.
    pub fn seek_with(&mut self, seconds: f64, mut sink: impl FnMut(usize, &[u8])) {
        let Some(midi) = &self.midi else { return };
        let target = seconds.clamp(0.0, midi.duration());
        let events = midi.events();
        let next = events.partition_point(|e| e.time < target);
        for event in &events[..next] {
            if is_state(event) {
                sink(0, event.message());
            }
        }
        self.next = next;
        self.position = target;
    }

    /// Renders `left.len()` frames into the synthesizer's output, playing
    /// the events that fall in them. When paused (or finished) the
    /// synthesizer still renders, so releases ring out.
    pub fn render(&mut self, synth: &mut Synthesizer, left: &mut [f32], right: &mut [f32]) {
        assert_eq!(left.len(), right.len(), "left and right must have the same length");
        let frames = left.len();
        let mut done = 0;
        // Split the buffer at every event's frame.
        loop {
            let next = self.next_step(frames - done);
            let until = match next {
                Step::None => frames,
                Step::Events(offset, _) | Step::Restart(offset) | Step::Stop(offset) => done + offset,
            };
            if until > done {
                synth.render(&mut left[done..until], &mut right[done..until]);
                self.advance_position(until - done);
                done = until;
            }
            match next {
                Step::None => break,
                Step::Events(_, range) => self.dispatch(range, |_, message| synth.process_midi_message(message)),
                Step::Restart(_) => self.restart(),
                Step::Stop(_) => self.stop_at_end(),
            }
        }
    }

    /// Advances `frames` frames without a synthesizer: `sink(frame, message)`
    /// gets each event with its frame offset in this block, e.g. to send it
    /// to an external MIDI device with a timestamp.
    pub fn advance(&mut self, frames: usize, mut sink: impl FnMut(usize, &[u8])) {
        let mut done = 0;
        loop {
            let next = self.next_step(frames - done);
            let until = match next {
                Step::None => frames,
                Step::Events(offset, _) | Step::Restart(offset) | Step::Stop(offset) => done + offset,
            };
            self.advance_position(until - done);
            done = until;
            match next {
                Step::None => break,
                Step::Events(_, range) => self.dispatch(range, |_, message| sink(done, message)),
                Step::Restart(_) => self.restart(),
                Step::Stop(_) => self.stop_at_end(),
            }
        }
    }

    /// What happens next within `frames` frames.
    fn next_step(&mut self, frames: usize) -> Step {
        if !self.playing {
            return Step::None;
        }
        let Some(midi) = self.midi.as_ref() else { return Step::None };
        let events = midi.events();
        if self.next >= events.len() {
            // Past the last event: wait for the end, then loop or stop.
            let offset = self.frames_for(midi.duration() - self.position);
            if offset >= frames {
                return Step::None;
            }
            return if self.looping && midi.duration() > 0.0 { Step::Restart(offset) } else { Step::Stop(offset) };
        }
        let time = events[self.next].time;
        let offset = self.frames_for(time - self.position);
        if offset >= frames {
            return Step::None;
        }
        let start = self.next;
        let mut end = start + 1;
        while end < events.len() && events[end].time <= time {
            end += 1;
        }
        self.next = end;
        Step::Events(offset, (start, end))
    }

    fn restart(&mut self) {
        self.position = 0.0;
        self.next = 0;
    }

    fn stop_at_end(&mut self) {
        self.position = self.duration();
        self.playing = false;
    }

    /// Frames until `seconds` from now: the first frame at or after it
    /// (allowing for rounding error in the event times).
    fn frames_for(&self, seconds: f64) -> usize {
        let frames = seconds / self.speed * self.sample_rate - 1e-6;
        if frames <= 0.0 { 0 } else { frames.ceil() as usize }
    }

    fn advance_position(&mut self, frames: usize) {
        if self.playing {
            self.position += frames as f64 * self.speed / self.sample_rate;
        }
    }

    fn dispatch(&mut self, (start, end): (usize, usize), mut sink: impl FnMut(usize, &[u8])) {
        let Some(midi) = &self.midi else { return };
        let events = &midi.events()[start..end];
        if let Some(first) = events.first() {
            // Land exactly on the event time (the frame was rounded up).
            self.position = self.position.max(first.time);
        }
        for event in events {
            if event.is_note_on() && self.muted.get(event.track as usize).copied().unwrap_or(false) {
                continue;
            }
            sink(0, event.message());
        }
    }
}

enum Step {
    /// Nothing within the frames asked for.
    None,
    /// Events (index range) at a frame offset.
    Events(usize, (usize, usize)),
    /// The end of a looping file at a frame offset.
    Restart(usize),
    /// The end of the file at a frame offset.
    Stop(usize),
}

/// Whether an event sets state a seek must restore (everything but notes).
fn is_state(event: &MidiEvent) -> bool {
    matches!(event.status() & 0xf0, 0xb0 | 0xc0 | 0xd0 | 0xe0)
}
