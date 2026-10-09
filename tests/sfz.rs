//! SFZ instruments: parsing, samples (WAV and FLAC), and the region
//! behaviour beyond SF2 (release triggers, round robin, mute groups…).

use std::collections::HashMap;
use std::sync::Arc;

use sf2synth::writer::SfzFiles;
use sf2synth::{FileFormat, SoundFont, Synthesizer, SynthesizerSettings};

const RATE: u32 = 44100;

/// A 441 Hz sine (100 points a period) of `amplitude` (0…1), `frames` long.
fn sine(amplitude: f32, frames: usize) -> Vec<f32> {
    (0..frames).map(|i| (i as f32 / 100.0 * std::f32::consts::TAU).sin() * amplitude).collect()
}

/// A WAV file of `channels` interleaved channels: 16-bit, 24-bit or 32-bit
/// float.
fn wav(samples: &[f32], channels: u16, bits: u16, smpl: Option<(u32, u32, u32)>) -> Vec<u8> {
    let float = bits == 32;
    let mut data = Vec::new();
    for &s in samples {
        match bits {
            16 => data.extend_from_slice(&((s * 32767.0).round() as i16).to_le_bytes()),
            24 => data.extend_from_slice(&((s * 8_388_607.0).round() as i32).to_le_bytes()[..3]),
            _ => data.extend_from_slice(&s.to_le_bytes()),
        }
    }
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&(if float { 3u16 } else { 1u16 }).to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&RATE.to_le_bytes());
    let block = channels as u32 * bits as u32 / 8;
    fmt.extend_from_slice(&(RATE * block).to_le_bytes());
    fmt.extend_from_slice(&(block as u16).to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    let mut chunks = Vec::new();
    for (id, body) in [(b"fmt ", fmt), (b"data", data)] {
        chunks.extend_from_slice(id);
        chunks.extend_from_slice(&(body.len() as u32).to_le_bytes());
        chunks.extend_from_slice(&body);
    }
    if let Some((root, start, end)) = smpl {
        let mut body = vec![0u8; 36 + 24];
        body[12..16].copy_from_slice(&root.to_le_bytes());
        body[28..32].copy_from_slice(&1u32.to_le_bytes());
        body[36 + 8..36 + 12].copy_from_slice(&start.to_le_bytes());
        body[36 + 12..36 + 16].copy_from_slice(&end.to_le_bytes());
        chunks.extend_from_slice(b"smpl");
        chunks.extend_from_slice(&(body.len() as u32).to_le_bytes());
        chunks.extend_from_slice(&body);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + chunks.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(&chunks);
    out
}

fn mono(amplitude: f32) -> Vec<u8> {
    wav(&sine(amplitude, RATE as usize), 1, 16, None)
}

fn font(sfz: &str, files: Vec<(&str, Vec<u8>)>) -> Arc<SoundFont> {
    try_font(sfz, files).unwrap()
}

fn try_font(sfz: &str, files: Vec<(&str, Vec<u8>)>) -> sf2synth::Result<Arc<SoundFont>> {
    let files: HashMap<String, Vec<u8>> = files.into_iter().map(|(n, b)| (n.to_string(), b)).collect();
    let load = move |path: &str, _limit: Option<usize>| -> std::io::Result<Vec<u8>> {
        files.get(path).cloned().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, path.to_string()))
    };
    SoundFont::from_sfz(sfz, "test", &load).map(Arc::new)
}

fn synth(font: &Arc<SoundFont>) -> Synthesizer {
    let mut settings = SynthesizerSettings::new(RATE);
    settings.reverb.enabled = false;
    settings.chorus.enabled = false;
    settings.limiter = false;
    Synthesizer::new(Arc::clone(font), &settings).unwrap()
}

/// Renders `seconds` and returns the RMS level of the left channel.
fn level(synth: &mut Synthesizer, seconds: f32) -> f32 {
    let frames = (seconds * RATE as f32) as usize;
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    synth.render(&mut left, &mut right);
    (left.iter().map(|x| x * x).sum::<f32>() / frames as f32).sqrt()
}

/// The level of a note of `key` and `velocity` over its first 0.1 s.
fn note_level(font: &Arc<SoundFont>, key: u8, velocity: u8) -> f32 {
    let mut s = synth(font);
    s.note_on(0, key, velocity);
    level(&mut s, 0.1)
}

fn db(ratio: f32) -> f32 {
    20.0 * ratio.log10()
}

#[test]
fn parses_headers_comments_defines_and_note_names() {
    let sfz = r#"
        // A comment <region> sample=nothing.wav
        /* a block
           comment */
        #define $LOW 48
        <control> default_path=Samples\Sub/
        <global> lovel=10 hivel=100
        <group> pitch_keycenter=c4 tune=5
        <region> sample=a sine.wav lokey=$LOW hikey=f#4
        <region> sample=a sine.wav key=a4 lovel=101 hivel=127 tune=-150
        <region>sample=a sine.wav lokey=c#5 hikey=eb5 // trailing comment
    "#;
    let font = font(sfz, vec![("Samples/Sub/a sine.wav", mono(0.5))]);
    assert_eq!(font.info().format, FileFormat::Sfz);
    assert_eq!(font.info().name, "test");
    assert!(font.warnings().is_empty(), "{:?}", font.warnings());
    let zones = font.instruments()[0].zones();
    assert_eq!(zones.len(), 3);
    assert_eq!(zones[0].key_range(), (48, 66));
    assert_eq!(zones[0].velocity_range(), (10, 100));
    assert_eq!(zones[0].generator(sf2synth::generator::OVERRIDING_ROOT_KEY), Some(60));
    assert_eq!(zones[0].generator(sf2synth::generator::FINE_TUNE), Some(5));
    // `key` sets the range and the pitch centre, over the group's.
    assert_eq!(zones[1].key_range(), (69, 69));
    assert_eq!(zones[1].generator(sf2synth::generator::OVERRIDING_ROOT_KEY), Some(69));
    assert_eq!(zones[1].velocity_range(), (101, 127));
    assert_eq!(zones[1].generator(sf2synth::generator::COARSE_TUNE), Some(-1));
    assert_eq!(zones[1].generator(sf2synth::generator::FINE_TUNE), Some(-50));
    assert_eq!(zones[2].key_range(), (73, 75));
    // A sample header for each root key the shared file plays at.
    assert_eq!(font.samples().len(), 2);
}

#[test]
fn reports_unsupported_opcodes_and_missing_samples() {
    let sfz = "<region> sample=there.wav sw_last=40 fil_type=hpf_2p cutoff=500\n<region> sample=gone.wav";
    let font = font(sfz, vec![("there.wav", mono(0.5))]);
    let warnings = font.warnings().join("\n");
    assert!(warnings.contains("sw_last"), "{warnings}");
    assert!(warnings.contains("fil_type=hpf_2p"), "{warnings}");
    assert!(warnings.contains("gone.wav"), "{warnings}");
    assert_eq!(font.instruments()[0].zones().len(), 1);
    // Nothing playable at all is an error.
    assert!(try_font("<region> sample=gone.wav", vec![]).is_err());
    assert!(try_font("<group> volume=3", vec![]).is_err());
}

#[test]
fn reads_wav_formats_stereo_and_loops() {
    let stereo: Vec<f32> = sine(0.5, 2000).into_iter().flat_map(|s| [s, -s]).collect();
    let files = vec![
        ("16.wav", wav(&sine(0.5, 2000), 1, 16, Some((57, 100, 1099)))),
        ("24.wav", wav(&sine(0.5, 2000), 1, 24, None)),
        ("float.wav", wav(&sine(0.5, 2000), 1, 32, None)),
        ("stereo.wav", wav(&stereo, 2, 16, None)),
    ];
    let sfz = "<region> sample=16.wav pitch_keycenter=sample\n<region> sample=24.wav\n\
               <region> sample=float.wav\n<region> sample=stereo.wav";
    let font = font(sfz, files);
    assert!(font.is_24_bit());
    let samples = font.samples();
    assert_eq!(samples.len(), 5, "three mono, one stereo pair");
    let first = &samples[font.instruments()[0].zones()[0].link()];
    assert_eq!(first.original_pitch, 57, "root from the smpl chunk");
    assert_eq!((first.loop_start - first.start, first.loop_end - first.start), (100, 1100));
    assert_eq!(font.instruments()[0].zones()[0].generator(sf2synth::generator::SAMPLE_MODES), Some(1));
    // The same sine in each format reads back alike.
    for zone in &font.instruments()[0].zones()[..3] {
        let h = &samples[zone.link()];
        let points = font.sample_points(h.start as usize..h.end as usize);
        assert!((points[25] - 0.5).abs() < 1e-3, "{} reads {}", h.name, points[25]);
    }
    // The stereo pair: left and right, panned apart, the right inverted.
    let zones = &font.instruments()[0].zones()[3..];
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0].generator(sf2synth::generator::PAN), Some(-500));
    assert_eq!(zones[1].generator(sf2synth::generator::PAN), Some(500));
    let (l, r) = (&samples[zones[0].link()], &samples[zones[1].link()]);
    assert_eq!((l.sample_type, r.sample_type), (4, 2));
    let left = font.sample_points(l.start as usize + 25..l.start as usize + 26)[0];
    let right = font.sample_points(r.start as usize + 25..r.start as usize + 26)[0];
    assert!((left + right).abs() < 1e-4 && left > 0.4);
}

#[test]
fn volume_amplitude_and_velocity_tracking() {
    let files = || vec![("s.wav", mono(0.5))];
    let plain = font("<region> sample=s.wav pitch_keycenter=69", files());
    let quieter = font("<region> sample=s.wav pitch_keycenter=69 volume=-6 amplitude=50", files());
    let ratio = note_level(&quieter, 69, 127) / note_level(&plain, 69, 127);
    assert!((db(ratio) - (-12.02)).abs() < 0.1, "{} dB", db(ratio));
    // The default velocity response is the square law, as SF2's.
    let soft = note_level(&plain, 69, 64) / note_level(&plain, 69, 127);
    assert!((db(soft) - 40.0 * (64f32 / 127.0).log10()).abs() < 0.1, "{} dB", db(soft));
    // No velocity tracking: every velocity alike.
    let flat = font("<region> sample=s.wav pitch_keycenter=69 amp_veltrack=0", files());
    assert!((note_level(&flat, 69, 1) / note_level(&flat, 69, 127) - 1.0).abs() < 1e-3);
    // A curve: linear in gain.
    let curve = font("<region> sample=s.wav pitch_keycenter=69 amp_velcurve_1=0 amp_velcurve_127=1", files());
    let half = note_level(&curve, 69, 64) / note_level(&curve, 69, 127);
    assert!((half - 64.0 / 127.0).abs() < 0.01, "{half}");
}

#[test]
fn release_triggers_wait_for_the_key_and_the_pedal() {
    let sfz = "<region> sample=attack.wav pitch_keycenter=69 ampeg_release=0.001\n\
               <region> sample=release.wav pitch_keycenter=69 trigger=release rt_decay=6";
    let font = font(sfz, vec![("attack.wav", mono(0.2)), ("release.wav", mono(0.8))]);
    let mut s = synth(&font);
    s.note_on(0, 69, 127);
    let held = level(&mut s, 0.2);
    s.note_off(0, 69);
    level(&mut s, 0.01);
    let released = level(&mut s, 0.1);
    assert!(released > held * 3.0, "held {held}, released {released}");

    // Held 1 s instead of 0.2 s: rt_decay (6 dB/s) takes 4.8 dB more off.
    let mut s = synth(&font);
    s.note_on(0, 69, 127);
    level(&mut s, 1.0);
    s.note_off(0, 69);
    level(&mut s, 0.01);
    let later = level(&mut s, 0.1);
    assert!((db(later / released) + 4.8).abs() < 0.1, "{} dB", db(later / released));

    // With the pedal down the release region waits for the pedal.
    let mut s = synth(&font);
    s.control_change(0, 64, 127);
    s.note_on(0, 69, 127);
    level(&mut s, 0.1);
    s.note_off(0, 69);
    let pedalled = level(&mut s, 0.1);
    assert!(pedalled < held * 1.5, "no release sound under the pedal: {pedalled}");
    s.control_change(0, 64, 0);
    level(&mut s, 0.01);
    assert!(level(&mut s, 0.1) > held * 3.0);
}

#[test]
fn round_robin_and_random_pick_regions() {
    let files = || vec![("a.wav", mono(0.2)), ("b.wav", mono(0.6))];
    let sfz = "<group> pitch_keycenter=69 seq_length=2\n<region> sample=a.wav seq_position=1\n\
               <region> sample=b.wav seq_position=2";
    let font = font(sfz, files());
    let mut s = synth(&font);
    let mut levels = Vec::new();
    for _ in 0..4 {
        s.note_on(0, 69, 127);
        levels.push(level(&mut s, 0.05));
        s.note_off(0, 69);
        level(&mut s, 0.3);
    }
    assert!(levels[0] < levels[1] && levels[2] < levels[3], "{levels:?}");
    assert!((levels[0] - levels[2]).abs() < 1e-3, "{levels:?}");

    let sfz = "<group> pitch_keycenter=69\n<region> sample=a.wav hirand=0.5\n<region> sample=b.wav lorand=0.5";
    let font = self::font(sfz, files());
    let mut s = synth(&font);
    let (mut quiet, mut loud) = (0, 0);
    for _ in 0..40 {
        s.note_on(0, 69, 127);
        let l = level(&mut s, 0.02);
        s.note_off(0, 69);
        level(&mut s, 0.3);
        if l < 0.1 { quiet += 1 } else { loud += 1 }
    }
    assert!(quiet > 5 && loud > 5, "{quiet} quiet, {loud} loud");
}

#[test]
fn mute_groups_one_shots_and_first_legato() {
    let files = || vec![("open.wav", mono(0.5)), ("closed.wav", mono(0.1))];
    let sfz = "<region> sample=open.wav key=46 group=1 off_by=2\n<region> sample=closed.wav key=42 group=2";
    let font = font(sfz, files());
    let mut s = synth(&font);
    s.note_on(0, 46, 127);
    level(&mut s, 0.1);
    s.note_on(0, 42, 127);
    level(&mut s, 0.02);
    // Only the closed hat is left.
    let after = level(&mut s, 0.1);
    assert!(after < 0.1, "{after}");

    let sfz = "<region> sample=open.wav pitch_keycenter=69 loop_mode=one_shot";
    let font = self::font(sfz, files());
    let mut s = synth(&font);
    s.note_on(0, 69, 127);
    let held = level(&mut s, 0.1);
    s.note_off(0, 69);
    let after = level(&mut s, 0.2);
    assert!(after > held * 0.9, "a one-shot plays on after note-off: {held} → {after}");

    let sfz =
        "<group> pitch_keycenter=60\n<region> sample=open.wav trigger=first\n<region> sample=closed.wav trigger=legato";
    let font = self::font(sfz, files());
    let mut s = synth(&font);
    s.note_on(0, 60, 127);
    let first = level(&mut s, 0.05);
    s.note_on(0, 62, 127);
    s.note_off(0, 60);
    level(&mut s, 0.05);
    let legato = level(&mut s, 0.05);
    // The open sample (0.5) first, then only the closed one (0.1).
    assert!((first / legato - 5.0).abs() < 0.1, "first {first}, legato {legato}");
}

#[test]
fn controllers_gate_and_trigger_regions() {
    let files = || vec![("up.wav", mono(0.2)), ("down.wav", mono(0.6)), ("pedal.wav", mono(0.5))];
    let sfz = "<group> pitch_keycenter=69\n<region> sample=up.wav hicc64=63\n<region> sample=down.wav locc64=64\n\
               <region> sample=pedal.wav on_locc64=64 on_hicc64=127 key=30 amp_veltrack=0";
    let font = font(sfz, files());
    let mut s = synth(&font);
    s.note_on(0, 69, 127);
    let up = level(&mut s, 0.05);
    s.note_off(0, 69);
    level(&mut s, 0.3);
    s.control_change(0, 64, 127);
    // The pedal itself sounds once, then the pedalled note.
    let pedal = level(&mut s, 0.05);
    assert!(pedal > 0.1, "pedal noise {pedal}");
    s.control_change(0, 64, 120);
    s.note_on(0, 69, 127);
    let down = level(&mut s, 0.05);
    assert!(down > up * 2.0, "up {up}, down {down}");
}

#[test]
fn crossfades_follow_velocity() {
    let files = || vec![("soft.wav", mono(0.5)), ("loud.wav", mono(0.5))];
    let sfz = "<group> pitch_keycenter=69 amp_veltrack=0\n\
               <region> sample=soft.wav xfout_lovel=40 xfout_hivel=80\n\
               <region> sample=loud.wav xfin_lovel=40 xfin_hivel=80";
    let font = font(sfz, files());
    let low = note_level(&font, 69, 20);
    let middle = note_level(&font, 69, 60);
    let high = note_level(&font, 69, 100);
    // Equal power: in the middle each is at √½, in phase they sum to √2.
    assert!((low - high).abs() < 1e-3, "{low} {high}");
    assert!((middle / low - 2f32.sqrt()).abs() < 0.01, "{}", middle / low);
}

#[test]
fn flac_samples_and_writing_back() {
    // An SFZ with WAV samples, written as SFZ + FLAC and read again, plays
    // the same.
    let stereo: Vec<f32> = sine(0.5, 4000).into_iter().flat_map(|s| [s, s * 0.5]).collect();
    let sfz = "<region> sample=a.wav lokey=0 hikey=70 pitch_keycenter=69 ampeg_release=0.3 cutoff=3000 fil_veltrack=1200 \
               seq_length=2 seq_position=1 group=3 off_by=4 trigger=attack amp_veltrack=50\n\
               <region> sample=b.wav lokey=0 hikey=70 pitch_keycenter=69 seq_length=2 seq_position=2 pan=-30 width=60 \
               loop_mode=loop_continuous loop_start=100 loop_end=3099 volume=-3\n\
               <region> sample=a.wav lokey=71 hikey=127 pitch_keycenter=72 trigger=release rt_decay=3 tune=12";
    let original = font(sfz, vec![("a.wav", wav(&sine(0.5, 4000), 1, 24, None)), ("b.wav", wav(&stereo, 2, 16, None))]);
    let files = SfzFiles::from_preset(&original, 0, "s/");
    assert!(files.warnings.is_empty(), "{:?}", files.warnings);
    assert_eq!(files.samples.len(), 2);
    assert!(files.samples.iter().all(|(name, bytes)| name.ends_with(".flac") && bytes.starts_with(b"fLaC")));
    let written: Vec<(String, Vec<u8>)> = files.samples.clone();
    let again = font(&files.sfz, written.iter().map(|(n, b)| (n.as_str(), b.clone())).collect());
    assert!(again.warnings().is_empty(), "{:?}", again.warnings());
    assert!(again.is_24_bit());

    let play = |font: &Arc<SoundFont>| -> Vec<f32> {
        let mut s = synth(font);
        let mut out = Vec::new();
        for (key, velocity) in [(69, 100), (69, 50), (75, 127), (60, 30)] {
            s.note_on(0, key, velocity);
            let (mut l, mut r) = (vec![0.0; 6000], vec![0.0; 6000]);
            s.render(&mut l, &mut r);
            s.note_off(0, key);
            out.extend(l);
            out.extend(r);
            let (mut l, mut r) = (vec![0.0; 6000], vec![0.0; 6000]);
            s.render(&mut l, &mut r);
            out.extend(l);
            out.extend(r);
        }
        out
    };
    let (a, b) = (play(&original), play(&again));
    let peak = a.iter().fold(0f32, |m, x| m.max(x.abs()));
    let difference = a.iter().zip(&b).fold(0f32, |m, (x, y)| m.max((x - y).abs()));
    assert!(peak > 0.05);
    assert!(difference <= peak * 1e-5, "differs by {difference} (peak {peak})");
}

#[test]
fn tells_the_first_layer_apart() {
    let files = || vec![("a.wav", mono(0.5))];
    let sfz = "<group> seq_length=2\n<region> sample=a.wav seq_position=1 volume=-6\n<region> sample=a.wav seq_position=2\n\
               <region> sample=a.wav trigger=release\n<region> sample=a.wav lorand=0.5\n<region> sample=a.wav locc64=64\n\
               <region> sample=a.wav on_locc64=64\n<region> sample=a.wav hicc64=63 locc7=90";
    let font = font(sfz, files());
    let firsts: Vec<bool> = font.instruments()[0].zones().iter().map(|z| z.plays_on_first_note_on()).collect();
    assert_eq!(firsts, [true, false, false, false, false, false, true]);
    assert!((font.instruments()[0].zones()[0].gain() - 0.501).abs() < 0.001);
}
