//! System Exclusive messages, bank and drum kit selection, and the other
//! ways in: MIDI byte streams, UMP, rendering with events and sends.

mod common;

use std::sync::Arc;

use common::*;
use sf2synth::writer::{InstrumentDef, PresetDef, SampleDef, Sf2Builder, ZoneDef};
use sf2synth::{MidiFile, Sequencer, SoundFont, Synthesizer, SynthesizerSettings};

const GS_RESET: [u8; 11] = [0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7f, 0x00, 0x41, 0xf7];
const XG_ON: [u8; 9] = [0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7];
const GM2_ON: [u8; 6] = [0xf0, 0x7e, 0x7f, 0x09, 0x03, 0xf7];

fn sine_points() -> Vec<i16> {
    (0..SINE_LENGTH).map(|i| ((2.0 * std::f64::consts::PI * i as f64 / 100.0).sin() * 16000.0) as i16).collect()
}

/// Presets named after their bank and program, all playing the looping
/// 441 Hz sine.
fn bank_font(presets: &[(u16, u16)]) -> Arc<SoundFont> {
    let mut builder = Sf2Builder::new("Banks");
    let sample =
        builder.add_sample(SampleDef::mono("Sine", sine_points(), SAMPLE_RATE, 69).looped(LOOP_START, LOOP_END));
    let instrument = builder.add_instrument(InstrumentDef::new("Sine").zone(ZoneDef::sample(sample).generator(54, 1)));
    for &(bank, program) in presets {
        builder.add_preset(
            PresetDef::new(format!("{bank}:{program}"), bank, program).zone(ZoneDef::instrument(instrument)),
        );
    }
    Arc::new(SoundFont::from_bytes(builder.to_bytes()).unwrap())
}

fn synth_with(font: Arc<SoundFont>) -> Synthesizer {
    let mut settings = SynthesizerSettings::new(SAMPLE_RATE);
    settings.reverb.enabled = false;
    settings.chorus.enabled = false;
    // Exact frames and levels.
    settings.limiter = false;
    Synthesizer::new(font, &settings).unwrap()
}

fn sine_synth() -> Synthesizer {
    synth_with(bank_font(&[(0, 0)]))
}

/// The name of the preset a channel plays.
fn playing(synth: &Synthesizer, channel: u8) -> String {
    let index = synth.channel_preset(channel).unwrap();
    synth.sound_font().presets()[index].name().to_string()
}

fn select(synth: &mut Synthesizer, channel: u8, bank: u8, program: u8) {
    synth.process_midi_message(&[0xb0 | channel, 0, bank]);
    synth.process_midi_message(&[0xc0 | channel, program]);
}

fn render(synth: &mut Synthesizer, frames: usize) -> Vec<f32> {
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    synth.render(&mut left, &mut right);
    left
}

const PRESETS: [(u16, u16); 7] = [(0, 0), (8, 0), (0, 1), (127, 5), (128, 0), (128, 8), (128, 24)];

#[test]
fn missing_variations_fall_back_to_their_capital_tone() {
    let mut synth = synth_with(bank_font(&PRESETS));
    select(&mut synth, 0, 8, 0);
    assert_eq!(playing(&synth, 0), "8:0");
    select(&mut synth, 0, 9, 0);
    assert_eq!(playing(&synth, 0), "8:0");
    select(&mut synth, 0, 5, 0);
    assert_eq!(playing(&synth, 0), "0:0");
    select(&mut synth, 0, 8, 1);
    assert_eq!(playing(&synth, 0), "0:1");
}

#[test]
fn missing_drum_kits_fall_back_to_their_capital_kit() {
    let mut synth = synth_with(bank_font(&PRESETS));
    assert!(synth.is_channel_drum(9));
    assert_eq!(playing(&synth, 9), "128:0");
    for (program, kit) in [(8, "128:8"), (10, "128:8"), (25, "128:24"), (40, "128:0")] {
        synth.process_midi_message(&[0xc9, program]);
        assert_eq!(playing(&synth, 9), kit, "program {program}");
    }
    // Bank select doesn't take a drum channel out of the drums.
    select(&mut synth, 9, 0, 1);
    assert_eq!(playing(&synth, 9), "128:0");
}

#[test]
fn drum_banks_play_kits_unless_the_file_has_them() {
    let mut synth = synth_with(bank_font(&PRESETS));
    // XG and GM2 drum banks without their System On.
    select(&mut synth, 2, 127, 8);
    assert_eq!(playing(&synth, 2), "128:8");
    select(&mut synth, 2, 120, 0);
    assert_eq!(playing(&synth, 2), "128:0");
    // The file's own bank 127 preset.
    select(&mut synth, 2, 127, 5);
    assert_eq!(playing(&synth, 2), "127:5");
}

#[test]
fn xg_selects_drums_with_banks_126_and_127() {
    let mut synth = synth_with(bank_font(&PRESETS));
    synth.process_midi_message(&XG_ON);
    select(&mut synth, 3, 127, 5);
    assert!(synth.is_channel_drum(3));
    assert_eq!(playing(&synth, 3), "128:0");
    select(&mut synth, 3, 0, 1);
    assert!(!synth.is_channel_drum(3));
    assert_eq!(playing(&synth, 3), "0:1");
    // Channel 10 starts on bank 127; bank 0 makes it melodic.
    synth.process_midi_message(&[0xc9, 8]);
    assert_eq!(playing(&synth, 9), "128:8");
    select(&mut synth, 9, 0, 1);
    assert_eq!(playing(&synth, 9), "0:1");
    // Part Mode: drum, whatever the bank.
    synth.process_midi_message(&[0xf0, 0x43, 0x10, 0x4c, 0x08, 0x04, 0x07, 0x01, 0xf7]);
    assert!(synth.is_channel_drum(4));
    select(&mut synth, 4, 0, 24);
    assert_eq!(playing(&synth, 4), "128:24");
}

#[test]
fn gs_rhythm_parts_and_the_mt32_bank() {
    let mut synth = synth_with(bank_font(&PRESETS));
    synth.process_midi_message(&GS_RESET);
    // In GS, bank 127 is the MT-32 set: melodic, falling back to bank 0.
    select(&mut synth, 2, 127, 1);
    assert_eq!(playing(&synth, 2), "0:1");
    // Part 11 (channel 10, 0-based) becomes a rhythm part.
    synth.process_midi_message(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x1a, 0x15, 0x01, 0x10, 0xf7]);
    assert!(synth.is_channel_drum(10));
    synth.process_midi_message(&[0xca, 24]);
    assert_eq!(playing(&synth, 10), "128:24");
    // A GS reset brings back the single drum channel.
    synth.process_midi_message(&GS_RESET);
    assert!(!synth.is_channel_drum(10));
    assert!(synth.is_channel_drum(9));
}

#[test]
fn gm2_selects_drums_with_bank_120() {
    let mut synth = synth_with(bank_font(&PRESETS));
    synth.process_midi_message(&GM2_ON);
    select(&mut synth, 1, 120, 8);
    assert!(synth.is_channel_drum(1));
    assert_eq!(playing(&synth, 1), "128:8");
    select(&mut synth, 1, 121, 1);
    assert!(!synth.is_channel_drum(1));
    assert_eq!(playing(&synth, 1), "0:1");
}

#[test]
fn set_channel_drum_adds_drum_channels() {
    let mut synth = synth_with(bank_font(&PRESETS));
    synth.set_channel_drum(0, true);
    synth.process_midi_message(&[0xc0, 8]);
    assert_eq!(playing(&synth, 0), "128:8");
    synth.set_channel_drum(0, false);
    assert_eq!(playing(&synth, 0), "0:0");
    synth.reset();
    assert!(!synth.is_channel_drum(0));
}

#[test]
fn master_volume_scales_the_output() {
    let mut synth = sine_synth();
    synth.note_on(0, 69, 100);
    render(&mut synth, 4410);
    let full = peak(&render(&mut synth, 4410));
    // Universal Master Volume: half.
    synth.process_midi_message(&[0xf0, 0x7f, 0x7f, 0x04, 0x01, 0x00, 0x40, 0xf7]);
    let half = peak(&render(&mut synth, 4410));
    assert!((half / full - 0.25).abs() < 0.01, "{half} / {full}");
    // GS Master Volume: full again.
    synth.process_midi_message(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x04, 0x7f, 0x3d, 0xf7]);
    assert!((peak(&render(&mut synth, 4410)) / full - 1.0).abs() < 0.01);
}

#[test]
fn resets_silence_and_restore_the_channels() {
    for reset in [&GS_RESET[..], &XG_ON[..], &[0xf0, 0x7e, 0x7f, 0x09, 0x01, 0xf7][..]] {
        let mut synth = sine_synth();
        synth.process_midi_message(&[0xb0, 7, 0]);
        synth.process_midi_message(&[0xf0, 0x7f, 0x7f, 0x04, 0x01, 0x00, 0x00, 0xf7]);
        synth.note_on(0, 69, 100);
        render(&mut synth, 1000);
        synth.process_midi_message(reset);
        render(&mut synth, 4410);
        assert_eq!(synth.active_voice_count(), 0);
        assert_eq!(synth.master_volume(), 1.0);
        // Volume is back to 100: the note sounds.
        synth.note_on(0, 69, 100);
        assert!(peak(&render(&mut synth, 4410)) > 0.1);
    }
}

#[test]
fn master_tuning_shifts_every_channel() {
    let mut synth = sine_synth();
    // Coarse +12 semitones.
    synth.process_midi_message(&[0xf0, 0x7f, 0x7f, 0x04, 0x04, 0x00, 0x4c, 0xf7]);
    synth.note_on(0, 69, 100);
    render(&mut synth, 4410);
    let measured = frequency(&render(&mut synth, 44100), SAMPLE_RATE);
    assert!((measured - SINE_HZ * 2.0).abs() < 0.5, "{measured}");
}

#[test]
fn render_with_events_applies_them_at_their_frame() {
    let mut synth = sine_synth();
    let (mut left, mut right) = (vec![0.0; 1000], vec![0.0; 1000]);
    let note_on = [0x90, 69, 100];
    let volume = [0xb0, 7, 127];
    synth.render_with_events(&mut left, &mut right, [(300, &volume[..]), (333, &note_on[..])]);
    let first = left.iter().position(|x| x.abs() > 1e-6).unwrap();
    assert!((333..340).contains(&first), "{first}");
    // Offsets past the end apply after the buffer.
    let mut synth = sine_synth();
    synth.render_with_events(&mut left, &mut right, [(5000, &note_on[..])]);
    assert!(peak(&left) == 0.0);
    assert_eq!(synth.active_voice_count(), 1);
}

#[test]
fn render_with_sends_writes_the_reverb_send() {
    let mut synth = sine_synth();
    let frames = 4410;
    let (mut left, mut right, mut reverb, mut chorus) =
        (vec![0.0; frames], vec![0.0; frames], vec![0.0; frames], vec![0.0; frames]);
    synth.note_on(0, 69, 100);
    synth.render_with_sends(&mut left, &mut right, Some(&mut reverb), Some(&mut chorus));
    assert_eq!(peak(&reverb), 0.0);
    assert_eq!(peak(&chorus), 0.0);
    synth.control_change(0, 91, 127);
    synth.render_with_sends(&mut left, &mut right, Some(&mut reverb), None);
    let send = peak(&reverb[frames / 2..]);
    let dry = peak(&left[frames / 2..]);
    // CC91 127 sends 20% (200 per mille) of the voice; the dry mix is
    // panned to the center (×0.707).
    assert!((send / dry - 0.2 / std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02, "{send} / {dry}");
}

#[test]
fn mismatched_buffers_never_panic() {
    let mut synth = sine_synth();
    synth.note_on(0, 69, 100);
    let (mut left, mut right) = (vec![1.0; 100], vec![1.0; 300]);
    synth.render(&mut left, &mut right);
    assert!(right[100..].iter().all(|&x| x == 0.0));
    let mut reverb = vec![1.0; 50];
    synth.render_with_sends(&mut left, &mut right, Some(&mut reverb), None);
    assert!(left[50..].iter().all(|&x| x == 0.0));
    synth.render_with_events(&mut right, &mut left, [(200, &[0x80, 69, 0][..])]);
    let mut odd = vec![1.0; 7];
    synth.render_interleaved(&mut odd);
    assert_eq!(odd[6], 0.0);
    let mut sequencer = Sequencer::new(SAMPLE_RATE);
    sequencer.render(&mut synth, &mut left, &mut right);
}

#[test]
fn byte_streams_keep_running_status_and_split_sysex() {
    let mut synth = sine_synth();
    // Volume 0, then a GS reset cut in three pieces with a clock in between.
    synth.process_midi_bytes(&[0xb0, 7, 0, 10, 64]);
    synth.process_midi_bytes(&[0x90, 69]);
    synth.process_midi_bytes(&[100]);
    assert_eq!(synth.active_voice_count(), 1);
    assert!(peak(&render(&mut synth, 4410)) < 1e-3);
    synth.process_midi_bytes(&GS_RESET[..4]);
    synth.process_midi_bytes(&[0xf8]);
    synth.process_midi_bytes(&GS_RESET[4..9]);
    synth.process_midi_bytes(&GS_RESET[9..]);
    render(&mut synth, 4410);
    assert_eq!(synth.active_voice_count(), 0);
    // Running status after the SysEx needs a new status byte.
    synth.process_midi_bytes(&[69, 100]);
    assert_eq!(synth.active_voice_count(), 0);
    synth.process_midi_bytes(&[0x90, 69, 100, 70, 100]);
    assert_eq!(synth.active_voice_count(), 2);
    assert!(peak(&render(&mut synth, 4410)) > 0.1);
}

#[test]
fn ump_plays_midi_1_and_2_messages() {
    let mut synth = sine_synth();
    // MIDI 2.0 note-on, key 69, velocity 0xffff; then a MIDI 1.0 note-on.
    synth.process_ump(&[0x4090_4500, 0xffff_0000, 0x2090_4664]);
    assert_eq!(synth.active_voice_count(), 2);
    // MIDI 2.0 CC7 = 0 silences.
    synth.process_ump(&[0x40b0_0700, 0x0000_0000]);
    render(&mut synth, 4410);
    assert!(peak(&render(&mut synth, 4410)) < 1e-3);
    // RPN 0 (pitch bend range) = 12 semitones, then full bend up.
    let mut synth = sine_synth();
    synth.process_ump(&[0x4020_0000, 12 << 25, 0x40e0_0000, 0xffff_ffff, 0x4090_4500, 0x8000_0000]);
    render(&mut synth, 4410);
    let measured = frequency(&render(&mut synth, 44100), SAMPLE_RATE);
    let expected = SINE_HZ * 2f64.powf(16383.0 / 8192.0 - 1.0);
    assert!((measured - expected).abs() < 1.0, "{measured} vs {expected}");
}

#[test]
fn ump_sysex7_packets_are_joined() {
    let mut synth = sine_synth();
    synth.note_on(0, 69, 100);
    // GS reset without F0/F7: 41 10 42 12 40 00 7F 00 41, as start + end.
    synth.process_ump(&[0x3016_4110, 0x4212_4000, 0x3033_7f00, 0x4100_0000]);
    render(&mut synth, 4410);
    assert_eq!(synth.active_voice_count(), 0);
}

#[test]
fn set_sample_rate_keeps_the_pitch() {
    let mut synth = sine_synth();
    synth.process_midi_message(&[0xc0, 0]);
    synth.set_sample_rate(22050).unwrap();
    assert_eq!(synth.sample_rate(), 22050);
    synth.note_on(0, 69, 100);
    render(&mut synth, 2205);
    let measured = frequency(&render(&mut synth, 22050), 22050);
    assert!((measured - SINE_HZ).abs() < 0.5, "{measured}");
    assert!(synth.set_sample_rate(1000).is_err());
}

#[test]
fn midi_files_keep_sysex_and_seeking_replays_it() {
    // Master volume 0 at tick 96 (0.1 s), a note at 0.5 s.
    let mut events = vec![(96, vec![0xf0, 7, 0x7f, 0x7f, 0x04, 0x01, 0x00, 0x00, 0xf7])];
    events.push((480, vec![0x90, 69, 100]));
    events.push((580, vec![0x80, 69, 0]));
    let midi = Arc::new(MidiFile::from_bytes(&midi_file(&events)).unwrap());
    let sysex = &midi.events()[0];
    assert!(sysex.is_sysex());
    assert_eq!(midi.message(sysex), &[0xf0, 0x7f, 0x7f, 0x04, 0x01, 0x00, 0x00, 0xf7]);
    assert_eq!(midi.message(&midi.events()[1]), &[0x90, 69, 100]);

    let mut synth = sine_synth();
    let mut sequencer = Sequencer::new(SAMPLE_RATE);
    sequencer.load(midi.clone());
    sequencer.seek(0.3, &mut synth);
    assert_eq!(synth.master_volume(), 0.0);
    sequencer.play();
    let (mut left, mut right) = (vec![0.0; 22050], vec![0.0; 22050]);
    sequencer.render(&mut synth, &mut left, &mut right);
    assert_eq!(peak(&left), 0.0);
    // Seeking back before it restores the volume.
    sequencer.seek(0.0, &mut synth);
    assert_eq!(synth.master_volume(), 1.0);
}

#[test]
fn gs_scale_tuning_retunes_pitch_classes() {
    let mut synth = sine_synth();
    // Part 1, A (pitch class 9) +50 cents.
    synth.process_midi_message(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x49, 64 + 50, 0x00, 0xf7]);
    synth.note_on(0, 69, 100);
    render(&mut synth, 4410);
    let measured = frequency(&render(&mut synth, 44100), SAMPLE_RATE);
    let expected = SINE_HZ * 2f64.powf(50.0 / 1200.0);
    assert!((measured - expected).abs() < 0.5, "{measured} vs {expected}");
    // Another part is untouched.
    synth.note_on(1, 69, 100);
    synth.process_midi_message(&[0xb0, 7, 0]);
    render(&mut synth, 4410);
    let measured = frequency(&render(&mut synth, 44100), SAMPLE_RATE);
    assert!((measured - SINE_HZ).abs() < 0.5, "{measured}");
}

fn reverb_send_peak(synth: &mut Synthesizer) -> f32 {
    let frames = 4410;
    let (mut left, mut right, mut reverb) = (vec![0.0; frames], vec![0.0; frames], vec![0.0; frames]);
    synth.render_with_sends(&mut left, &mut right, Some(&mut reverb), None);
    peak(&reverb)
}

#[test]
fn gs_xg_and_gm2_start_with_some_reverb() {
    let mut synth = sine_synth();
    synth.note_on(0, 69, 100);
    assert_eq!(reverb_send_peak(&mut synth), 0.0, "General MIDI: no reverb send");
    for reset in [&GS_RESET[..], &XG_ON[..], &GM2_ON[..]] {
        synth.process_midi_message(reset);
        synth.note_on(0, 69, 100);
        assert!(reverb_send_peak(&mut synth) > 0.001);
    }
}

#[test]
fn gs_part_parameters() {
    let mut synth = synth_with(bank_font(&PRESETS));
    synth.process_midi_message(&GS_RESET);
    // Part 1 tone number: bank 8, program 0.
    synth.process_midi_message(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x00, 0x08, 0x00, 0x27, 0xf7]);
    assert_eq!(playing(&synth, 0), "8:0");
    // Part 2 program only.
    synth.process_midi_message(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x12, 0x01, 0x01, 0x2c, 0xf7]);
    assert_eq!(playing(&synth, 1), "0:1");
    // Part 1 level 0 silences it.
    synth.note_on(0, 69, 100);
    synth.process_midi_message(&[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x19, 0x00, 0x16, 0xf7]);
    render(&mut synth, 4410);
    assert!(peak(&render(&mut synth, 4410)) < 1e-3);
}

#[test]
fn xg_part_parameters() {
    let mut synth = synth_with(bank_font(&PRESETS));
    synth.process_midi_message(&XG_ON);
    // Part 3: bank MSB 127, program 8 (a drum kit in XG).
    synth.process_midi_message(&[0xf0, 0x43, 0x10, 0x4c, 0x08, 0x02, 0x01, 0x7f, 0xf7]);
    synth.process_midi_message(&[0xf0, 0x43, 0x10, 0x4c, 0x08, 0x02, 0x03, 0x08, 0xf7]);
    assert_eq!(playing(&synth, 2), "128:8");
    // Part 1 volume 0.
    synth.note_on(0, 69, 100);
    synth.process_midi_message(&[0xf0, 0x43, 0x10, 0x4c, 0x08, 0x00, 0x0b, 0x00, 0xf7]);
    render(&mut synth, 4410);
    assert!(peak(&render(&mut synth, 4410)) < 1e-3);
}

#[test]
fn key_shifts_transpose() {
    let measure = |messages: &[&[u8]]| {
        let mut synth = sine_synth();
        for message in messages {
            synth.process_midi_message(message);
        }
        synth.note_on(0, 69, 100);
        render(&mut synth, 4410);
        frequency(&render(&mut synth, 44100), SAMPLE_RATE)
    };
    // GS part 1 key shift +12.
    let up = measure(&[&GS_RESET, &[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x11, 0x16, 0x4c, 0x1d, 0xf7]]);
    assert!((up - SINE_HZ * 2.0).abs() < 0.5, "{up}");
    // GS master key shift −12.
    let down = measure(&[&GS_RESET, &[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x05, 0x34, 0x07, 0xf7]]);
    assert!((down - SINE_HZ / 2.0).abs() < 0.5, "{down}");
    // XG part 1 note shift +12 and transpose −12 cancel out.
    let same = measure(&[
        &XG_ON,
        &[0xf0, 0x43, 0x10, 0x4c, 0x08, 0x00, 0x08, 0x4c, 0xf7],
        &[0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x06, 0x34, 0xf7],
    ]);
    assert!((same - SINE_HZ).abs() < 0.5, "{same}");
}

#[test]
fn midi_files_join_sysex_continued_in_f7_packets() {
    let events = vec![
        // F0 7F 7F 04 01, then 00 00 F7 in a continuation packet.
        (0, vec![0xf0, 4, 0x7f, 0x7f, 0x04, 0x01]),
        (10, vec![0xf7, 3, 0x00, 0x00, 0xf7]),
        // A whole message escaped in an F7 packet.
        (20, vec![0xf7, 6, 0xf0, 0x7e, 0x7f, 0x09, 0x01, 0xf7]),
        // Other escaped bytes are skipped.
        (30, vec![0xf7, 1, 0xfa]),
        (40, vec![0x90, 69, 100]),
    ];
    let midi = MidiFile::from_bytes(&midi_file(&events)).unwrap();
    let messages: Vec<&[u8]> = midi.events().iter().map(|e| midi.message(e)).collect();
    assert_eq!(
        messages,
        [
            &[0xf0, 0x7f, 0x7f, 0x04, 0x01, 0x00, 0x00, 0xf7][..],
            &[0xf0, 0x7e, 0x7f, 0x09, 0x01, 0xf7],
            &[0x90, 69, 100]
        ]
    );
    assert_eq!(midi.events()[0].tick, 0);
}
