//! Damaged files, extreme instruments and random MIDI: never a panic, never
//! a NaN in the output.

mod common;

use std::sync::Arc;

use common::*;
use sf2synth::modulator::Modulator;
use sf2synth::writer::{InstrumentDef, PresetDef, SampleDef, Sf2Builder, ZoneDef};
use sf2synth::{MidiFile, Sequencer, SoundFont, Synthesizer, SynthesizerSettings};

/// xorshift64*: reproducible random numbers without dependencies.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }
}

/// Renders `frames` and checks the output is finite.
fn render_finite(synth: &mut Synthesizer, frames: usize, what: &str) {
    let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
    synth.render(&mut left, &mut right);
    assert!(left.iter().chain(&right).all(|x| x.is_finite()), "non-finite output: {what}");
}

/// Plays a few notes on every preset of `font`.
fn play_everything(font: SoundFont, what: &str) {
    let presets: Vec<(u16, u16)> = font.presets().iter().map(|p| (p.bank(), p.program())).collect();
    let mut synth = Synthesizer::new(Arc::new(font), &SynthesizerSettings::new(SAMPLE_RATE)).unwrap();
    for (i, (bank, program)) in presets.into_iter().take(16).enumerate() {
        let channel = (i % 16) as u8;
        if bank == 128 {
            synth.set_channel_drum(channel, true);
        } else {
            synth.control_change(channel, 0, bank as u8);
        }
        synth.program_change(channel, program as u8);
        for key in [0, 36, 60, 69, 127] {
            synth.note_on(channel, key, 1 + key % 127);
        }
    }
    render_finite(&mut synth, 2048, what);
    synth.note_off_all();
    render_finite(&mut synth, 1024, what);
}

#[test]
fn damaged_sound_fonts() {
    let original = sine_font(&[looping_zone(), TestZone { key_range: Some((0, 40)), ..looping_zone() }]);
    let mut random = Random(0x5eed_f00d);
    let mut played = 0;
    for round in 0..3000 {
        let mut bytes = original.clone();
        for _ in 0..1 + random.below(8) {
            let at = random.below(bytes.len());
            bytes[at] = random.byte();
        }
        if round % 7 == 0 {
            bytes.truncate(random.below(bytes.len()));
        }
        if let Ok(font) = SoundFont::from_bytes(bytes) {
            play_everything(font, &format!("damaged font, round {round}"));
            played += 1;
        }
    }
    assert!(played > 1000, "{played}");
}

/// Generators worth pushing to their limits (and past them).
const GENERATORS: [u16; 40] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 15, 16, 17, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34,
    35, 36, 37, 38, 45, 48, 51, 52, 56,
];

#[test]
fn extreme_instruments() {
    let mut random = Random(0xdead_beef);
    let points: Vec<i16> = (0..2000).map(|i| ((i as f32 * 0.07).sin() * 30000.0) as i16).collect();
    let extremes = [i16::MIN, -32768 / 2, -12000, -1200, -1, 0, 1, 127, 1200, 12000, 32767];
    for round in 0..400 {
        let mut builder = Sf2Builder::new("Extreme");
        let sample = builder.add_sample(
            SampleDef::mono("S", points.clone(), 1 + random.below(200_000) as u32, 60)
                .looped(random.below(2500) as u32, random.below(2500) as u32),
        );
        let mut zone = ZoneDef::sample(sample).generator(54, random.below(4) as i16);
        for _ in 0..random.below(12) {
            let generator = GENERATORS[random.below(GENERATORS.len())];
            zone.set(generator, extremes[random.below(extremes.len())]);
        }
        for _ in 0..random.below(4) {
            zone = zone.modulator(Modulator {
                source: random.next() as u16,
                destination: GENERATORS[random.below(GENERATORS.len())],
                amount: extremes[random.below(extremes.len())],
                amount_source: random.next() as u16,
                transform: random.below(3) as u16,
            });
        }
        let instrument = builder.add_instrument(InstrumentDef::new("I").zone(zone));
        let mut preset_zone = ZoneDef::instrument(instrument);
        for _ in 0..random.below(4) {
            preset_zone.set(GENERATORS[random.below(GENERATORS.len())], extremes[random.below(extremes.len())]);
        }
        builder.add_preset(PresetDef::new("P", 0, 0).zone(preset_zone));
        let Ok(font) = SoundFont::from_bytes(builder.to_bytes()) else { continue };
        let mut synth = Synthesizer::new(Arc::new(font), &SynthesizerSettings::new(SAMPLE_RATE)).unwrap();
        // Every controller a modulator might read, at random.
        for controller in [1, 7, 10, 11, 64, 66, 91, 93] {
            synth.control_change(0, controller, random.byte() & 0x7f);
        }
        synth.pitch_bend(0, random.below(16384) as u16);
        synth.channel_pressure(0, random.byte() & 0x7f);
        for key in [0, 60, 127] {
            synth.note_on(0, key, 1 + random.byte() % 127);
        }
        render_finite(&mut synth, 4096, &format!("extreme instrument, round {round}"));
        synth.note_off_all();
        render_finite(&mut synth, 2048, &format!("extreme instrument released, round {round}"));
    }
}

#[test]
fn random_midi_input() {
    let font = Arc::new(SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap());
    let mut synth = Synthesizer::new(font, &SynthesizerSettings::new(SAMPLE_RATE)).unwrap();
    let mut random = Random(0x1234_5678);
    let resets: [&[u8]; 3] = [
        &[0xf0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7f, 0x00, 0x41, 0xf7],
        &[0xf0, 0x43, 0x10, 0x4c, 0x00, 0x00, 0x7e, 0x00, 0xf7],
        &[0xf0, 0x7e, 0x7f, 0x09, 0x03, 0xf7],
    ];
    for round in 0..2000 {
        let bytes: Vec<u8> = (0..random.below(64)).map(|_| random.byte()).collect();
        synth.process_midi_bytes(&bytes);
        synth.process_midi_message(&bytes);
        // SysEx shaped messages with random contents (GS, XG, universal).
        let mut sysex = resets[random.below(3)].to_vec();
        for byte in sysex.iter_mut().skip(4).take(4) {
            if random.below(2) == 0 {
                *byte = random.byte() & 0x7f;
            }
        }
        synth.process_midi_message(&sysex);
        let words: Vec<u32> = (0..random.below(8)).map(|_| random.next() as u32).collect();
        synth.process_ump(&words);
        if round % 20 == 0 {
            render_finite(&mut synth, 256, &format!("random MIDI, round {round}"));
        }
    }
}

#[test]
fn damaged_midi_files() {
    let mut events = vec![
        (0, vec![0xf0, 10, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7f, 0x00, 0x41, 0xf7]),
        (0, vec![0xff, 0x51, 0x03, 0x07, 0xa1, 0x20]),
        (5, vec![0xf0, 4, 0x7f, 0x7f, 0x04, 0x01]),
        (6, vec![0xf7, 3, 0x00, 0x40, 0xf7]),
        (10, vec![0xb0, 64, 127]),
    ];
    for i in 0..20 {
        events.push((20 + i * 10, vec![0x90, 60 + i as u8, 100]));
        events.push((25 + i * 10, vec![0x80, 60 + i as u8, 0]));
    }
    let original = midi_file(&events);
    let font = Arc::new(SoundFont::from_bytes(sine_font(&[looping_zone()])).unwrap());
    let mut random = Random(0xfeed_face);
    let mut played = 0;
    for round in 0..2000 {
        let mut bytes = original.clone();
        for _ in 0..1 + random.below(6) {
            let at = random.below(bytes.len());
            bytes[at] = random.byte();
        }
        let Ok(midi) = MidiFile::from_bytes(&bytes) else { continue };
        let mut synth = Synthesizer::new(font.clone(), &SynthesizerSettings::new(SAMPLE_RATE)).unwrap();
        let mut sequencer = Sequencer::new(SAMPLE_RATE);
        sequencer.load(Arc::new(midi));
        sequencer.seek(0.1, &mut synth);
        sequencer.play();
        let (mut left, mut right) = (vec![0.0; 1024], vec![0.0; 1024]);
        for _ in 0..4 {
            sequencer.render(&mut synth, &mut left, &mut right);
        }
        assert!(left.iter().all(|x| x.is_finite()), "non-finite output: damaged MIDI file, round {round}");
        played += 1;
    }
    assert!(played > 500, "{played}");
}

#[test]
fn damaged_dls() {
    let Ok(original) =
        std::fs::read("/System/Library/Components/CoreAudio.component/Contents/Resources/gs_instruments.dls")
    else {
        return;
    };
    let mut random = Random(0xd15_d15);
    let mut played = 0;
    for round in 0..40 {
        let mut bytes = original.clone();
        for _ in 0..1 + random.below(64) {
            let at = random.below(bytes.len().min(400_000));
            bytes[at] = random.byte();
        }
        if let Ok(font) = SoundFont::from_bytes(bytes) {
            play_everything(font, &format!("damaged DLS, round {round}"));
            played += 1;
        }
    }
    assert!(played > 10, "{played}");
}
