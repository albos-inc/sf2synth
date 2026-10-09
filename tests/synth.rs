mod common;

use std::sync::Arc;

use common::*;
use sf2synth::{Interpolation, SoundFont, Synthesizer, SynthesizerSettings};

fn synth_for(zones: &[TestZone]) -> Synthesizer {
    let font = Arc::new(SoundFont::from_bytes(sine_font(zones)).unwrap());
    let mut settings = SynthesizerSettings::new(SAMPLE_RATE);
    settings.reverb.enabled = false;
    settings.chorus.enabled = false;
    Synthesizer::new(font, &settings).unwrap()
}

fn render(synth: &mut Synthesizer, seconds: f64) -> Vec<f32> {
    let frames = (seconds * SAMPLE_RATE as f64) as usize;
    let mut left = vec![0.0; frames];
    let mut right = vec![0.0; frames];
    synth.render(&mut left, &mut right);
    left
}

#[test]
fn parses_the_file() {
    let font = SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap();
    assert_eq!(font.info().name, "Test Sine");
    assert_eq!(font.info().version, (2, 1));
    assert_eq!(font.presets().len(), 1);
    assert_eq!(font.presets()[0].name(), "Sine");
    assert_eq!(font.instruments().len(), 1);
    assert_eq!(font.samples().len(), 1);
    assert_eq!(font.samples()[0].sample_rate, SAMPLE_RATE);
    assert_eq!(font.find_preset(0, 0), Some(0));
}

#[test]
fn plays_at_the_right_pitch() {
    for (key, expected) in
        [(69u8, SINE_HZ), (81, SINE_HZ * 2.0), (57, SINE_HZ / 2.0), (70, SINE_HZ * 2f64.powf(1.0 / 12.0))]
    {
        let mut synth = synth_for(&[looping_zone()]);
        synth.note_on(0, key, 100);
        render(&mut synth, 0.1);
        let measured = frequency(&render(&mut synth, 1.0), SAMPLE_RATE);
        assert!((measured - expected).abs() < expected * 0.0005, "key {key}: {measured} Hz, expected {expected}");
    }
}

#[test]
fn pitch_bend_follows_the_sensitivity() {
    let mut synth = synth_for(&[looping_zone()]);
    synth.note_on(0, 69, 100);
    synth.pitch_bend(0, 16383);
    render(&mut synth, 0.1);
    let measured = frequency(&render(&mut synth, 1.0), SAMPLE_RATE);
    let expected = SINE_HZ * 2f64.powf(2.0 * (16383.0 - 8192.0) / 8192.0 / 12.0);
    assert!((measured - expected).abs() < 0.3, "{measured} vs {expected}");

    // RPN 0 = 12 semitones.
    for (cc, value) in [(101, 0), (100, 0), (6, 12), (38, 0)] {
        synth.control_change(0, cc, value);
    }
    synth.pitch_bend(0, 0);
    render(&mut synth, 0.05);
    let measured = frequency(&render(&mut synth, 1.0), SAMPLE_RATE);
    assert!((measured - SINE_HZ / 2.0).abs() < 0.2, "{measured}");
}

#[test]
fn velocity_follows_the_square_law() {
    let level = |velocity| {
        let mut synth = synth_for(&[looping_zone()]);
        synth.note_on(0, 69, velocity);
        render(&mut synth, 0.1);
        rms(&render(&mut synth, 0.5))
    };
    let difference = 20.0 * (level(127) / level(64)).log10();
    let expected = -40.0 * (64.0f64 / 127.0).log10();
    assert!((difference - expected).abs() < 0.3, "{difference} dB vs {expected} dB");
}

#[test]
fn note_off_releases_in_the_release_time() {
    let mut synth = synth_for(&[looping_zone()]);
    synth.note_on(0, 69, 100);
    render(&mut synth, 0.2);
    synth.note_off(0, 69);
    let tail = render(&mut synth, 0.25);
    // Half the 0.5 s release: 48 dB down by the end.
    let start = rms(&tail[..441]);
    let end = rms(&tail[tail.len() - 441..]);
    let drop = 20.0 * (start / end).log10();
    assert!((drop - 48.0).abs() < 3.0, "{drop} dB");
    render(&mut synth, 0.3);
    assert_eq!(synth.active_voice_count(), 0);
}

#[test]
fn sustain_pedal_holds_notes_until_released() {
    let mut synth = synth_for(&[looping_zone()]);
    synth.note_on(0, 69, 100);
    synth.control_change(0, 64, 127);
    synth.note_off(0, 69);
    let held = render(&mut synth, 1.0);
    assert!(rms(&held[held.len() - 4410..]) > 0.02);
    synth.control_change(0, 64, 0);
    render(&mut synth, 0.6);
    assert_eq!(synth.active_voice_count(), 0);
}

#[test]
fn sostenuto_holds_only_the_notes_down_when_pressed() {
    let mut synth = synth_for(&[looping_zone()]);
    synth.note_on(0, 69, 100);
    synth.control_change(0, 66, 127);
    synth.note_on(0, 72, 100);
    synth.note_off(0, 69);
    synth.note_off(0, 72);
    render(&mut synth, 0.7);
    assert_eq!(synth.active_voice_count(), 1);
    let remaining = frequency(&render(&mut synth, 0.5), SAMPLE_RATE);
    assert!((remaining - SINE_HZ).abs() < 0.5, "{remaining}");
    synth.control_change(0, 66, 0);
    render(&mut synth, 0.6);
    assert_eq!(synth.active_voice_count(), 0);
}

#[test]
fn a_repeated_key_releases_the_previous_strike() {
    let mut synth = synth_for(&[looping_zone()]);
    synth.control_change(0, 64, 127);
    for _ in 0..5 {
        synth.note_on(0, 69, 100);
        synth.note_off(0, 69);
        render(&mut synth, 0.6);
    }
    assert_eq!(synth.active_voice_count(), 1);
}

#[test]
fn exclusive_class_cuts_the_other_voice() {
    let zone = |lo, hi| TestZone {
        key_range: Some((lo, hi)),
        generators: vec![(54, 1), (38, 2400), (57, 1)],
        ..Default::default()
    };
    let mut synth = synth_for(&[zone(0, 64), zone(65, 127)]);
    synth.note_on(0, 60, 100);
    synth.note_on(0, 70, 100);
    render(&mut synth, 0.05);
    assert_eq!(synth.active_voice_count(), 1);
}

#[test]
fn a_sample_without_a_valid_loop_plays_once() {
    let font = Arc::new(SoundFont::from_bytes(sine_font_with(&[looping_zone()], 5000, 5000)).unwrap());
    let mut synth = Synthesizer::new(font, &SynthesizerSettings::new(SAMPLE_RATE)).unwrap();
    synth.note_on(0, 69, 100);
    render(&mut synth, 0.9);
    assert_eq!(synth.active_voice_count(), 1);
    render(&mut synth, 0.2);
    assert_eq!(synth.active_voice_count(), 0);
}

#[test]
fn volume_and_expression_attenuate() {
    let level = |cc: u8, value: u8| {
        let mut synth = synth_for(&[looping_zone()]);
        synth.control_change(0, cc, value);
        synth.note_on(0, 69, 127);
        render(&mut synth, 0.1);
        rms(&render(&mut synth, 0.3))
    };
    let full = level(7, 127);
    assert!(level(7, 64) < full * 0.5);
    assert!(level(11, 0) < full * 1e-3);
}

#[test]
fn pan_moves_the_sound() {
    let mut synth = synth_for(&[looping_zone()]);
    synth.control_change(0, 10, 0);
    synth.note_on(0, 69, 100);
    let (mut left, mut right) = (vec![0.0; 22050], vec![0.0; 22050]);
    synth.render(&mut left, &mut right);
    assert!(rms(&left) > 100.0 * rms(&right));
}

#[test]
fn render_and_render_interleaved_agree() {
    let mut a = synth_for(&[looping_zone()]);
    let mut b = synth_for(&[looping_zone()]);
    a.note_on(0, 69, 100);
    b.note_on(0, 69, 100);
    let (mut left, mut right) = (vec![0.0; 1000], vec![0.0; 1000]);
    a.render(&mut left, &mut right);
    let mut interleaved = vec![0.0; 2000];
    b.render_interleaved(&mut interleaved);
    for i in 0..1000 {
        assert_eq!(left[i], interleaved[i * 2]);
        assert_eq!(right[i], interleaved[i * 2 + 1]);
    }
}

#[test]
fn voices_are_stolen_when_all_are_busy() {
    let font = Arc::new(SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap());
    let mut settings = SynthesizerSettings::new(SAMPLE_RATE);
    settings.max_voices = 4;
    let mut synth = Synthesizer::new(font, &settings).unwrap();
    for key in 60..72 {
        synth.note_on(0, key, 100);
    }
    render(&mut synth, 0.01);
    assert_eq!(synth.active_voice_count(), 4);
}

#[test]
fn a_stolen_voice_fades_out_instead_of_clicking() {
    let font = Arc::new(SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap());
    let mut settings = SynthesizerSettings::new(SAMPLE_RATE);
    settings.max_voices = 1;
    settings.reverb.enabled = false;
    settings.chorus.enabled = false;
    let mut synth = Synthesizer::new(font, &settings).unwrap();
    synth.note_on(0, 69, 100);
    let before = render(&mut synth, 0.1);
    let amplitude = before.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    // A new note steals the only voice: the old one fades while the new one
    // starts in a spare voice.
    synth.note_on(0, 81, 100);
    assert_eq!(synth.active_voice_count(), 2);
    let after = render(&mut synth, 0.02);
    let signal: Vec<f32> = before.iter().chain(&after).copied().collect();
    let largest_step = signal.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
    // Two sines (441 and 882 Hz) move at most 0.19 of their amplitude per
    // sample; a cut would jump by up to a whole amplitude.
    assert!(largest_step < 0.3 * amplitude, "{largest_step} vs {amplitude}");
    assert_eq!(synth.active_voice_count(), 1);
}

#[test]
fn reverb_adds_a_tail() {
    let font = Arc::new(SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap());
    let mut synth = Synthesizer::new(font, &SynthesizerSettings::new(SAMPLE_RATE)).unwrap();
    synth.control_change(0, 91, 127);
    synth.note_on(0, 69, 100);
    render(&mut synth, 0.3);
    synth.sound_off_all();
    render(&mut synth, 0.05);
    let tail = render(&mut synth, 0.2);
    assert_eq!(synth.active_voice_count(), 0);
    assert!(rms(&tail) > 1e-4);
}

#[test]
fn truncated_or_corrupt_files_never_panic() {
    let bytes = sine_font(&[looping_zone()]);
    let mut cuts: Vec<usize> = (0..400).collect();
    cuts.extend((400..bytes.len()).step_by(997));
    for cut in cuts {
        if let Ok(font) = SoundFont::from_bytes(bytes[..cut].to_vec()) {
            let mut synth = Synthesizer::new(Arc::new(font), &SynthesizerSettings::new(SAMPLE_RATE)).unwrap();
            synth.note_on(0, 69, 100);
            render(&mut synth, 0.01);
        }
    }
    // Flip bytes of the preset data.
    let pdta = bytes.windows(4).position(|w| w == b"pdta").unwrap();
    for i in pdta..bytes.len() {
        let mut corrupt = bytes.clone();
        corrupt[i] ^= 0xff;
        if let Ok(font) = SoundFont::from_bytes(corrupt) {
            let mut synth = Synthesizer::new(Arc::new(font), &SynthesizerSettings::new(SAMPLE_RATE)).unwrap();
            synth.note_on(0, 69, 100);
            render(&mut synth, 0.002);
        }
    }
}

#[test]
fn rejects_files_that_are_not_sf2() {
    assert!(SoundFont::from_bytes(b"RIFF\0\0\0\0WAVE".to_vec()).is_err());
    assert!(SoundFont::from_bytes(Vec::new()).is_err());
}

#[test]
fn rejects_bad_settings() {
    let font = Arc::new(SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap());
    assert!(Synthesizer::new(font.clone(), &SynthesizerSettings::new(1000)).is_err());
    let mut settings = SynthesizerSettings::new(48000);
    settings.max_voices = 0;
    assert!(Synthesizer::new(font, &settings).is_err());
}

/// A font whose only sample is a sine at 0.4 of its sample rate (17.64 kHz
/// at 44.1 kHz), recorded as key 60.
fn bright_font() -> Arc<SoundFont> {
    use sf2synth::writer::{InstrumentDef, PresetDef, SampleDef, Sf2Builder, ZoneDef};
    let points = (0..44100).map(|i| ((i as f64 * 0.4 * std::f64::consts::TAU).sin() * 16000.0) as i16).collect();
    let mut builder = Sf2Builder::new("Bright");
    let sample = builder.add_sample(SampleDef::mono("Bright", points, SAMPLE_RATE, 60).looped(1000, 41000));
    let instrument =
        builder.add_instrument(InstrumentDef::new("Bright").zone(ZoneDef::sample(sample).generator(54, 1)));
    builder.add_preset(PresetDef::new("Bright", 0, 0).zone(ZoneDef::instrument(instrument)));
    Arc::new(SoundFont::from_bytes(builder.to_bytes()).unwrap())
}

fn play(font: &Arc<SoundFont>, interpolation: Interpolation, key: u8) -> Vec<f32> {
    let mut settings = SynthesizerSettings::new(SAMPLE_RATE);
    settings.reverb.enabled = false;
    settings.chorus.enabled = false;
    settings.interpolation = interpolation;
    let mut synth = Synthesizer::new(font.clone(), &settings).unwrap();
    synth.note_on(0, key, 127);
    render(&mut synth, 0.1);
    render(&mut synth, 0.5)
}

#[test]
fn sinc_interpolation_removes_aliasing() {
    let font = bright_font();
    // At the recorded pitch both keep the 17.64 kHz tone.
    let cubic = rms(&play(&font, Interpolation::Cubic, 60));
    let sinc = rms(&play(&font, Interpolation::Sinc, 60));
    assert!(sinc > cubic * 0.7, "{sinc} vs {cubic}");
    // An octave up, 35.28 kHz folds back to 8.82 kHz: cubic lets it through,
    // sinc removes it.
    let cubic_alias = rms(&play(&font, Interpolation::Cubic, 72));
    let sinc_alias = rms(&play(&font, Interpolation::Sinc, 72));
    assert!(cubic_alias > cubic * 0.05, "{cubic_alias}");
    assert!(sinc_alias < sinc * 0.01, "{sinc_alias} vs {sinc}");
}

#[test]
fn sinc_interpolation_keeps_pitch_and_level() {
    let font = Arc::new(SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap());
    for key in [57, 69, 76, 81] {
        let cubic = play(&font, Interpolation::Cubic, key);
        let sinc = play(&font, Interpolation::Sinc, key);
        let (f_cubic, f_sinc) = (frequency(&cubic, SAMPLE_RATE), frequency(&sinc, SAMPLE_RATE));
        assert!((f_sinc - f_cubic).abs() < f_cubic * 0.001, "key {key}: {f_sinc} vs {f_cubic}");
        assert!((rms(&sinc) / rms(&cubic) - 1.0).abs() < 0.01, "key {key}: {} vs {}", rms(&sinc), rms(&cubic));
    }
}
