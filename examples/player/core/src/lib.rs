//! A MIDI file player on sf2synth, shared by the player apps.
//!
//! [`Player`] owns a [`Synthesizer`] and a [`Sequencer`] and adds what a
//! player needs on top of them: play / pause / stop, a song title read from
//! the file, and a summary of the SF2 and MIDI files for display.
//!
//! Files are parsed with [`LoadedSoundFont::new`] and [`LoadedSong::new`]
//! *before* they are handed to the player, so an app that shares the player
//! with its audio callback behind a lock only holds the lock to swap them in.

use std::sync::Arc;

use serde::Serialize;
use sf2synth::{FileFormat, MidiFile, Sequencer, SoundFont, Synthesizer, SynthesizerSettings};

/// Where the transport is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// At the start (also after the song has played to its end).
    #[default]
    Stopped,
    Playing,
    Paused,
}

impl State {
    /// 0 stopped, 1 playing, 2 paused (for the C and JNI interfaces).
    pub fn code(self) -> i32 {
        match self {
            State::Stopped => 0,
            State::Playing => 1,
            State::Paused => 2,
        }
    }
}

/// What the player shows about an SF2 (or DLS) file.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SoundFontSummary {
    pub file_name: String,
    /// The bank name from the file's INFO chunk (the file name if empty).
    pub name: String,
    /// "SF2 2.04" or "DLS".
    pub format: String,
    pub file_size: u64,
    pub presets: usize,
    pub instruments: usize,
    pub samples: usize,
    pub copyright: String,
    pub engineers: String,
    pub comment: String,
}

/// What the player shows about a MIDI file.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SongSummary {
    pub file_name: String,
    /// The first track's name (the sequence name), or the file name.
    pub title: String,
    /// SMF format 0, 1 or 2.
    pub format: u16,
    pub tracks: u16,
    /// Ticks per quarter note (0 for SMPTE timing).
    pub ticks_per_quarter: u16,
    /// The tempo at the start.
    pub tempo_bpm: f64,
    pub tempo_changes: usize,
    pub notes: usize,
    /// How many of the 16 MIDI channels play notes.
    pub channels: usize,
    pub duration: f64,
}

/// An SF2 or DLS file, parsed, with a synthesizer ready to play it.
pub struct LoadedSoundFont {
    synth: Synthesizer,
    summary: SoundFontSummary,
}

impl LoadedSoundFont {
    /// Parses `bytes` (an SF2 or DLS file) and sets up a synthesizer for it
    /// at `sample_rate`.
    pub fn new(bytes: Vec<u8>, file_name: &str, sample_rate: u32) -> Result<LoadedSoundFont, String> {
        let file_size = bytes.len() as u64;
        let font = Arc::new(SoundFont::from_bytes(bytes).map_err(|e| e.to_string())?);
        let info = font.info();
        let format = match info.format {
            FileFormat::Sf2 => format!("SF2 {}.{:02}", info.version.0, info.version.1),
            FileFormat::Dls => "DLS".to_string(),
        };
        let name = if info.name.trim().is_empty() { file_stem(file_name) } else { info.name.trim().to_string() };
        let summary = SoundFontSummary {
            file_name: file_name.to_string(),
            name,
            format,
            file_size,
            presets: font.presets().len(),
            instruments: font.instruments().len(),
            samples: font.samples().len(),
            copyright: info.copyright.trim().to_string(),
            engineers: info.engineers.trim().to_string(),
            comment: info.comment.trim().to_string(),
        };
        let synth = Synthesizer::new(font, &SynthesizerSettings::new(sample_rate)).map_err(|e| e.to_string())?;
        Ok(LoadedSoundFont { synth, summary })
    }

    pub fn summary(&self) -> &SoundFontSummary {
        &self.summary
    }
}

/// A MIDI file, parsed.
pub struct LoadedSong {
    midi: Arc<MidiFile>,
    summary: SongSummary,
}

impl LoadedSong {
    /// Parses `bytes` (a Standard MIDI File).
    pub fn new(bytes: &[u8], file_name: &str) -> Result<LoadedSong, String> {
        let midi = MidiFile::from_bytes(bytes).map_err(|e| e.to_string())?;
        let mut channels = [false; 16];
        let mut notes = 0;
        for event in midi.events().iter().filter(|e| e.is_note_on()) {
            notes += 1;
            channels[event.channel() as usize] = true;
        }
        let tempo =
            midi.tempo_changes().first().filter(|t| t.tick == 0).map_or(500_000, |t| t.microseconds_per_quarter);
        let division = midi.division();
        let summary = SongSummary {
            file_name: file_name.to_string(),
            title: read_title(bytes).unwrap_or_else(|| file_stem(file_name)),
            format: midi.format(),
            tracks: midi.track_count(),
            ticks_per_quarter: if division & 0x8000 == 0 { division } else { 0 },
            tempo_bpm: 60_000_000.0 / tempo.max(1) as f64,
            tempo_changes: midi.tempo_changes().len(),
            notes,
            channels: channels.iter().filter(|&&c| c).count(),
            duration: midi.duration(),
        };
        Ok(LoadedSong { midi: Arc::new(midi), summary })
    }

    pub fn summary(&self) -> &SongSummary {
        &self.summary
    }
}

/// The player: a synthesizer (once an SF2 is loaded), a sequencer (once a
/// MIDI file is loaded) and the transport state.
pub struct Player {
    sample_rate: u32,
    synth: Option<Synthesizer>,
    sequencer: Sequencer,
    state: State,
    sound_font: Option<SoundFontSummary>,
    song: Option<SongSummary>,
}

impl Player {
    pub fn new(sample_rate: u32) -> Player {
        Player {
            sample_rate,
            synth: None,
            sequencer: Sequencer::new(sample_rate),
            state: State::Stopped,
            sound_font: None,
            song: None,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Switches to another SF2. A song that is playing goes on with the new
    /// sounds. Returns the previous synthesizer so that the caller can drop
    /// it (and the SF2's memory) outside any lock it holds.
    pub fn set_sound_font(&mut self, loaded: LoadedSoundFont) -> Option<Synthesizer> {
        let mut synth = loaded.synth;
        // Bring the new synthesizer's programs and controllers to where the
        // song is.
        if self.song.is_some() && self.state != State::Stopped {
            let position = self.sequencer.position();
            self.sequencer.seek(position, &mut synth);
        }
        self.sound_font = Some(loaded.summary);
        self.synth.replace(synth)
    }

    /// Loads a song, stopped at its start. Returns the previous song's data,
    /// to drop outside any lock.
    pub fn set_song(&mut self, loaded: LoadedSong) -> Option<Arc<MidiFile>> {
        let previous = self.sequencer.midi_file().cloned();
        if let Some(synth) = &mut self.synth {
            synth.reset();
        }
        self.sequencer.load(loaded.midi);
        self.song = Some(loaded.summary);
        self.state = State::Stopped;
        previous
    }

    pub fn sound_font(&self) -> Option<&SoundFontSummary> {
        self.sound_font.as_ref()
    }

    pub fn song(&self) -> Option<&SongSummary> {
        self.song.as_ref()
    }

    /// Whether there are both an SF2 and a song to play.
    pub fn can_play(&self) -> bool {
        self.synth.is_some() && self.song.is_some()
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn play(&mut self) {
        let Some(synth) = &mut self.synth else { return };
        if self.song.is_none() || self.state == State::Playing {
            return;
        }
        // From the start after a stop, or after the song has ended.
        if self.state == State::Stopped && self.sequencer.position() > 0.0 {
            self.sequencer.seek(0.0, synth);
        }
        self.sequencer.play();
        self.state = State::Playing;
    }

    /// Pauses; the sounding notes are released and ring out.
    pub fn pause(&mut self) {
        if self.state != State::Playing {
            return;
        }
        match &mut self.synth {
            Some(synth) => self.sequencer.pause(synth),
            None => self.sequencer.pause_silently(),
        }
        self.state = State::Paused;
    }

    pub fn toggle(&mut self) {
        match self.state {
            State::Playing => self.pause(),
            _ => self.play(),
        }
    }

    /// Stops at once (silencing every note) and goes back to the start.
    pub fn stop(&mut self) {
        self.sequencer.pause_silently();
        self.seek_sequencer(0.0);
        self.state = State::Stopped;
    }

    /// Jumps to `seconds`. A stopped player becomes paused there.
    pub fn seek(&mut self, seconds: f64) {
        if self.song.is_none() {
            return;
        }
        self.seek_sequencer(seconds);
        if self.state == State::Stopped {
            self.state = State::Paused;
        }
    }

    fn seek_sequencer(&mut self, seconds: f64) {
        match &mut self.synth {
            Some(synth) => self.sequencer.seek(seconds, synth),
            None => self.sequencer.seek_with(seconds, |_, _| {}),
        }
    }

    /// Seconds into the song (0 when stopped).
    pub fn position(&self) -> f64 {
        match self.state {
            State::Stopped => 0.0,
            _ => self.sequencer.position(),
        }
    }

    /// The song's length in seconds.
    pub fn duration(&self) -> f64 {
        self.sequencer.duration()
    }

    pub fn active_voices(&self) -> usize {
        self.synth.as_ref().map_or(0, |s| s.active_voice_count())
    }

    /// Renders `left.len()` frames. Real-time safe: call it from the audio
    /// callback.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        match &mut self.synth {
            Some(synth) => self.sequencer.render(synth, left, right),
            None => {
                left.fill(0.0);
                right.fill(0.0);
            }
        }
        if self.state == State::Playing && !self.sequencer.is_playing() {
            // Played to the end. The last notes still ring out.
            self.state = State::Stopped;
        }
    }
}

/// "1:05" or "1:02:05".
pub fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// "118.2 MB".
pub fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

fn file_stem(file_name: &str) -> String {
    let name = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[..dot].to_string(),
        _ => name.to_string(),
    }
}

/// The song title: the first Sequence/Track Name meta event of the first
/// track (the conductor track in format 1).
pub fn read_title(smf: &[u8]) -> Option<String> {
    let be32 = |at: usize| -> Option<usize> {
        let b = smf.get(at..at + 4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    if smf.get(0..4)? != b"MThd" {
        return None;
    }
    let mut pos = 8usize.checked_add(be32(4)?)?;
    while pos + 8 <= smf.len() {
        let len = be32(pos + 4)?;
        let body = &smf[pos + 8..(pos + 8).saturating_add(len).min(smf.len())];
        if &smf[pos..pos + 4] == b"MTrk" {
            return track_name(body);
        }
        pos = pos.checked_add(8 + len)?;
    }
    None
}

fn track_name(track: &[u8]) -> Option<String> {
    let mut pos = 0;
    let mut running = 0u8;
    let variable_length = |pos: &mut usize| -> Option<usize> {
        let mut value = 0usize;
        for _ in 0..4 {
            let b = *track.get(*pos)?;
            *pos += 1;
            value = (value << 7) | (b & 0x7f) as usize;
            if b & 0x80 == 0 {
                break;
            }
        }
        Some(value)
    };
    while pos < track.len() {
        variable_length(&mut pos)?;
        let mut status = *track.get(pos)?;
        if status & 0x80 != 0 {
            pos += 1;
        } else {
            status = running;
        }
        match status {
            0xff => {
                let kind = *track.get(pos)?;
                pos += 1;
                let len = variable_length(&mut pos)?;
                let text = track.get(pos..pos.checked_add(len)?)?;
                pos += len;
                match kind {
                    0x03 => {
                        let name = decode_text(text);
                        // Some sequencers write a placeholder.
                        if !name.is_empty() && !name.eq_ignore_ascii_case("untitled") {
                            return Some(name);
                        }
                    }
                    0x2f => return None,
                    _ => {}
                }
            }
            0xf0 | 0xf7 => {
                let len = variable_length(&mut pos)?;
                pos = pos.checked_add(len)?;
            }
            0x80..=0xef => {
                running = status;
                pos += if matches!(status & 0xf0, 0xc0 | 0xd0) { 1 } else { 2 };
            }
            _ => return None,
        }
    }
    None
}

/// MIDI text is UTF-8 in newer files and Shift_JIS in most Japanese ones.
fn decode_text(bytes: &[u8]) -> String {
    let bytes = bytes.split(|&b| b == 0).next().unwrap_or_default();
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => {
            let (text, _, errors) = encoding_rs::SHIFT_JIS.decode(bytes);
            if errors { encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned() } else { text.into_owned() }
        }
    };
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A format-0 file: a track name, one note, end of track.
    fn smf(name: &[u8]) -> Vec<u8> {
        let mut track = vec![0x00, 0xff, 0x03, name.len() as u8];
        track.extend_from_slice(name);
        track.extend_from_slice(&[0x00, 0x90, 60, 100, 0x60, 0x80, 60, 0, 0x00, 0xff, 0x2f, 0x00]);
        let mut file = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk".to_vec();
        file.extend_from_slice(&(track.len() as u32).to_be_bytes());
        file.extend_from_slice(&track);
        file
    }

    #[test]
    fn titles() {
        assert_eq!(read_title(&smf(b"Nocturne")).as_deref(), Some("Nocturne"));
        assert_eq!(read_title(&smf("夜想曲".as_bytes())).as_deref(), Some("夜想曲"));
        // 夜想曲 in Shift_JIS.
        assert_eq!(read_title(&smf(&[0x96, 0xe9, 0x91, 0x7a, 0x8b, 0xc8])).as_deref(), Some("夜想曲"));
        assert_eq!(read_title(&smf(b"  ")), None);
        assert_eq!(read_title(b"RIFF"), None);
    }

    #[test]
    fn song_summary() {
        let song = LoadedSong::new(&smf(b"Test"), "dir/test.mid").unwrap();
        let summary = song.summary();
        assert_eq!(summary.title, "Test");
        assert_eq!(summary.notes, 1);
        assert_eq!(summary.channels, 1);
        assert_eq!(summary.ticks_per_quarter, 96);
        assert!((summary.tempo_bpm - 120.0).abs() < 1e-9);
        assert!((summary.duration - 0.5).abs() < 1e-9);
        assert_eq!(file_stem("C:\\music\\a.b.mid"), "a.b");
    }

    #[test]
    fn transport_without_a_sound_font() {
        let mut player = Player::new(48_000);
        player.set_song(LoadedSong::new(&smf(b"Test"), "test.mid").unwrap());
        player.play();
        assert_eq!(player.state(), State::Stopped, "nothing to play it with");
        player.seek(0.25);
        assert_eq!(player.state(), State::Paused);
        assert!((player.position() - 0.25).abs() < 1e-9);
        player.stop();
        assert_eq!((player.state(), player.position()), (State::Stopped, 0.0));
    }

    #[test]
    fn times() {
        assert_eq!(format_time(65.9), "1:05");
        assert_eq!(format_time(3725.0), "1:02:05");
        assert_eq!(format_size(118_200_000), "118.2 MB");
    }
}
