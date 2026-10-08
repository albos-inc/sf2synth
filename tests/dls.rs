//! Reads the General MIDI set built into macOS, when it is there.

use std::sync::Arc;

use sf2synth::{FileFormat, SoundFont, Synthesizer, SynthesizerSettings};

const MAC_GS: &str = "/System/Library/Components/CoreAudio.component/Contents/Resources/gs_instruments.dls";

fn mac_gs() -> Option<Arc<SoundFont>> {
    let bytes = std::fs::read(MAC_GS).ok()?;
    Some(Arc::new(SoundFont::from_bytes(bytes).expect("macOS's GS set should load")))
}

fn render(font: &Arc<SoundFont>, channel: u8, bank: u8, program: u8, key: u8, seconds: f32) -> Vec<f32> {
    let mut settings = SynthesizerSettings::new(44100);
    settings.reverb.enabled = false;
    settings.chorus.enabled = false;
    let mut synth = Synthesizer::new(Arc::clone(font), &settings).unwrap();
    synth.control_change(channel, 0, bank);
    synth.program_change(channel, program);
    synth.note_on(channel, key, 100);
    let frames = (seconds * 44100.0) as usize;
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    synth.render(&mut left, &mut right);
    left
}

fn rms(signal: &[f32]) -> f32 {
    (signal.iter().map(|x| x * x).sum::<f32>() / signal.len().max(1) as f32).sqrt()
}

#[test]
fn loads_the_macos_gs_set() {
    let Some(font) = mac_gs() else { return };
    assert_eq!(font.info().format, FileFormat::Dls);
    let melodic = font.presets().iter().filter(|p| p.bank() == 0).count();
    assert!(melodic >= 128, "only {melodic} melodic presets");
    assert!(font.find_preset(128, 0).is_some(), "no standard drum kit");
    assert!(font.find_preset(0, 0).is_some());
}

#[test]
fn plays_melodic_programs_and_drums() {
    let Some(font) = mac_gs() else { return };
    for program in [0u8, 19, 48, 73] {
        let sound = render(&font, 0, 0, program, 60, 0.6);
        assert!(rms(&sound[2000..]) > 0.002, "program {program} is silent");
        assert!(sound.iter().all(|x| x.is_finite() && x.abs() < 1.5));
    }
    // Kick (36) and snare (38) on the drum channel.
    for key in [36u8, 38] {
        let sound = render(&font, 9, 0, 0, key, 0.4);
        assert!(rms(&sound) > 0.002, "drum {key} is silent");
    }
}

#[test]
fn a_flute_a4_is_440_hz() {
    let Some(font) = mac_gs() else { return };
    let sound = render(&font, 0, 0, 73, 69, 1.5);
    // The autocorrelation peak (a flute's strong second harmonic fools
    // zero-crossing counts), refined between lags.
    let x = &sound[22050..22050 + 8192];
    let correlation = |lag: usize| -> f64 { (0..4096).map(|i| x[i] as f64 * x[i + lag] as f64).sum() };
    let lag = (60..400).max_by(|&a, &b| correlation(a).total_cmp(&correlation(b))).unwrap();
    let (a, b, c) = (correlation(lag - 1), correlation(lag), correlation(lag + 1));
    let period = lag as f64 + 0.5 * (a - c) / (a - 2.0 * b + c);
    let hz = 44100.0 / period;
    // Within 30 cents (the file tunes its samples by a few cents).
    let cents = 1200.0 * (hz / 440.0).log2();
    assert!(cents.abs() < 30.0, "{hz} Hz ({cents} cents)");
}
