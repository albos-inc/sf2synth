mod common;

use std::sync::Arc;

use common::*;
use sf2synth::{MidiFile, Sequencer, SoundFont, Synthesizer, SynthesizerSettings};

fn synth() -> Synthesizer {
    let font = Arc::new(SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap());
    let mut settings = SynthesizerSettings::new(SAMPLE_RATE);
    settings.reverb.enabled = false;
    settings.chorus.enabled = false;
    // Exact frames.
    settings.limiter = false;
    Synthesizer::new(font, &settings).unwrap()
}

fn first_sound(signal: &[f32]) -> Option<usize> {
    signal.iter().position(|&x| x.abs() > 1e-6)
}

/// 1 tick = 1/960 s.
fn note_at(tick: u32, length: u32) -> Vec<(u32, Vec<u8>)> {
    vec![(tick, vec![0x90, 69, 100]), (tick + length, vec![0x80, 69, 0])]
}

fn render(sequencer: &mut Sequencer, synth: &mut Synthesizer, seconds: f64, block: usize) -> Vec<f32> {
    let frames = (seconds * SAMPLE_RATE as f64) as usize;
    let mut out = Vec::with_capacity(frames);
    let (mut left, mut right) = (vec![0.0; block], vec![0.0; block]);
    while out.len() < frames {
        sequencer.render(synth, &mut left, &mut right);
        out.extend_from_slice(&left);
    }
    out.truncate(frames);
    out
}

#[test]
fn parses_times_with_tempo_changes() {
    // 480 ticks at 120 BPM = 0.5 s; then 60 BPM: 480 ticks = 1 s.
    let midi = MidiFile::from_bytes(&midi_file(&[
        (0, vec![0x90, 60, 100]),
        (480, vec![0xff, 0x51, 0x03, 0x0f, 0x42, 0x40]),
        (960, vec![0x80, 60, 0]),
    ]))
    .unwrap();
    let events = midi.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].time, 0.0);
    assert!((events[1].time - 1.5).abs() < 1e-9, "{}", events[1].time);
    assert!((midi.duration() - 1.5).abs() < 1e-9);
    assert_eq!(midi.tempo_changes().len(), 1);
}

#[test]
fn running_status_is_read() {
    let mut bytes = midi_file(&[]);
    // Replace the track with: note on, running-status note on, end.
    let track = [0x00, 0x90, 60, 100, 0x10, 62, 100, 0x00, 0xff, 0x2f, 0x00];
    bytes.truncate(14);
    bytes.extend_from_slice(b"MTrk");
    bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&track);
    let midi = MidiFile::from_bytes(&bytes).unwrap();
    assert_eq!(midi.events().len(), 2);
    assert_eq!(midi.events()[1].message(), &[0x90, 62, 100]);
}

#[test]
fn events_land_on_their_exact_frame() {
    let midi = Arc::new(MidiFile::from_bytes(&midi_file(&note_at(480, 480))).unwrap());
    for block in [64, 100, 512, 4096] {
        let mut synth = synth();
        let mut sequencer = Sequencer::new(SAMPLE_RATE);
        sequencer.load(midi.clone());
        sequencer.play();
        let out = render(&mut sequencer, &mut synth, 1.0, block);
        let first = first_sound(&out).unwrap();
        assert!((first as i64 - 22050).abs() <= 1, "block {block}: first sound at {first}");
    }
}

#[test]
fn speed_scales_time() {
    let midi = Arc::new(MidiFile::from_bytes(&midi_file(&note_at(480, 480))).unwrap());
    let mut synth = synth();
    let mut sequencer = Sequencer::new(SAMPLE_RATE);
    sequencer.load(midi);
    sequencer.set_speed(2.0);
    sequencer.play();
    let out = render(&mut sequencer, &mut synth, 0.5, 256);
    let first = first_sound(&out).unwrap();
    assert!((first as i64 - 11025).abs() <= 1, "{first}");
    assert!((sequencer.position() - 1.0).abs() < 0.01);
}

#[test]
fn seeking_restores_controllers_and_skips_earlier_notes() {
    // Volume 0 at 0.1 s, a note at 0.5 s and another at 1 s.
    let mut events = vec![(96, vec![0xb0, 7, 0])];
    events.extend(note_at(480, 100));
    events.extend(note_at(960, 100));
    let midi = Arc::new(MidiFile::from_bytes(&midi_file(&events)).unwrap());
    let mut synth = synth();
    let mut sequencer = Sequencer::new(SAMPLE_RATE);
    sequencer.load(midi);
    sequencer.seek(0.75, &mut synth);
    sequencer.play();
    let out = render(&mut sequencer, &mut synth, 0.6, 512);
    // The note at 1 s plays at volume 0: practically silent.
    assert!(peak(&out) < 1e-3, "{}", peak(&out));
    // The file ends at 1 s + 100 ticks.
    assert!(sequencer.is_finished());
    assert!((sequencer.position() - (1.0 + 100.0 / 960.0)).abs() < 1e-9);
}

#[test]
fn stops_at_the_end() {
    let midi = Arc::new(MidiFile::from_bytes(&midi_file(&note_at(0, 240))).unwrap());
    let mut synth = synth();
    let mut sequencer = Sequencer::new(SAMPLE_RATE);
    sequencer.load(midi);
    sequencer.play();
    render(&mut sequencer, &mut synth, 0.5, 512);
    assert!(sequencer.is_finished());
    assert!(!sequencer.is_playing());
    assert!((sequencer.position() - 0.25).abs() < 1e-9);
    // The release still rings out after the end.
    render(&mut sequencer, &mut synth, 1.0, 512);
    assert_eq!(synth.active_voice_count(), 0);
}

#[test]
fn loops_back_to_the_start() {
    let midi = Arc::new(MidiFile::from_bytes(&midi_file(&note_at(0, 240))).unwrap());
    let mut sequencer = Sequencer::new(SAMPLE_RATE);
    sequencer.load(midi);
    sequencer.set_looping(true);
    sequencer.play();
    let mut note_ons = Vec::new();
    let mut frame = 0;
    for _ in 0..100 {
        sequencer.advance(1000, |offset, message| {
            if message[0] & 0xf0 == 0x90 {
                note_ons.push(frame + offset);
            }
        });
        frame += 1000;
    }
    // Every 0.25 s.
    assert!(note_ons.len() >= 9);
    for (i, &at) in note_ons.iter().enumerate() {
        let expected = i as i64 * 11025;
        assert!((at as i64 - expected).abs() <= 1, "note {i} at {at}, expected {expected}");
    }
}

#[test]
fn advance_reports_frame_offsets() {
    let midi = Arc::new(MidiFile::from_bytes(&midi_file(&note_at(96, 96))).unwrap());
    let mut sequencer = Sequencer::new(SAMPLE_RATE);
    sequencer.load(midi);
    sequencer.play();
    let mut seen = Vec::new();
    sequencer.advance(44100, |offset, message| seen.push((offset, message.to_vec())));
    assert_eq!(seen.len(), 2);
    assert!((seen[0].0 as i64 - 4410).abs() <= 1);
    assert!((seen[1].0 as i64 - 8820).abs() <= 1);
    assert_eq!(seen[0].1, vec![0x90, 69, 100]);
}

#[test]
fn muted_tracks_skip_note_ons() {
    let midi = Arc::new(MidiFile::from_bytes(&midi_file(&note_at(0, 480))).unwrap());
    let mut synth = synth();
    let mut sequencer = Sequencer::new(SAMPLE_RATE);
    sequencer.load(midi);
    sequencer.set_track_muted(0, true);
    sequencer.play();
    let out = render(&mut sequencer, &mut synth, 0.3, 512);
    assert_eq!(peak(&out), 0.0);
}

#[test]
fn rejects_files_that_are_not_midi() {
    assert!(MidiFile::from_bytes(b"RIFF").is_err());
    assert!(MidiFile::from_bytes(&[]).is_err());
}

#[test]
fn truncated_files_never_panic() {
    let mut events = vec![(0, vec![0xc0, 5]), (10, vec![0xff, 0x51, 0x03, 0x07, 0xa1, 0x20])];
    events.extend(note_at(20, 100));
    let bytes = midi_file(&events);
    for cut in 0..bytes.len() {
        let _ = MidiFile::from_bytes(&bytes[..cut]);
    }
}
