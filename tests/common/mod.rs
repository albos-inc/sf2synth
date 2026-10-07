//! Builds small SF2 and MIDI files in memory for the tests.
#![allow(dead_code)]

pub const SAMPLE_RATE: u32 = 44100;
/// The test sample is a sine with a period of exactly 100 points: 441 Hz at
/// 44.1 kHz, recorded as key 69.
pub const SINE_HZ: f64 = 441.0;
pub const SINE_LENGTH: u32 = 44100;
pub const LOOP_START: u32 = 1000;
pub const LOOP_END: u32 = 41000;

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn list(kind: &[u8; 4], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut data = kind.to_vec();
    for c in chunks {
        data.extend_from_slice(c);
    }
    chunk(b"LIST", &data)
}

fn name20(name: &str) -> [u8; 20] {
    let mut out = [0u8; 20];
    out[..name.len()].copy_from_slice(name.as_bytes());
    out
}

/// An instrument zone of the test font.
#[derive(Clone, Default)]
pub struct TestZone {
    pub key_range: Option<(u8, u8)>,
    pub generators: Vec<(u16, i16)>,
    /// (source, destination, amount, amount source, transform)
    pub modulators: Vec<(u16, u16, i16, u16, u16)>,
}

/// One preset (bank 0, program 0) with one instrument whose zones all play
/// the 441 Hz sine.
pub fn sine_font(zones: &[TestZone]) -> Vec<u8> {
    sine_font_with(zones, LOOP_START, LOOP_END)
}

pub fn sine_font_with(zones: &[TestZone], loop_start: u32, loop_end: u32) -> Vec<u8> {
    let mut smpl = Vec::new();
    for i in 0..SINE_LENGTH {
        let v = (2.0 * std::f64::consts::PI * i as f64 / 100.0).sin() * 16000.0;
        smpl.extend_from_slice(&(v as i16).to_le_bytes());
    }
    smpl.extend_from_slice(&[0u8; 46 * 2]);

    let info =
        list(b"INFO", &[chunk(b"ifil", &[2, 0, 1, 0]), chunk(b"isng", b"EMU8000\0"), chunk(b"INAM", b"Test Sine\0")]);
    let sdta = list(b"sdta", &[chunk(b"smpl", &smpl)]);

    // Preset: one zone pointing at instrument 0.
    let mut phdr = Vec::new();
    for (name, bag) in [("Sine", 0u16), ("EOP", 1)] {
        phdr.extend_from_slice(&name20(name));
        phdr.extend_from_slice(&0u16.to_le_bytes()); // program
        phdr.extend_from_slice(&0u16.to_le_bytes()); // bank
        phdr.extend_from_slice(&bag.to_le_bytes());
        phdr.extend_from_slice(&[0u8; 12]);
    }
    let pbag = [0u16, 0, 1, 0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
    let pmod = vec![0u8; 10];
    let mut pgen = Vec::new();
    pgen.extend_from_slice(&41u16.to_le_bytes());
    pgen.extend_from_slice(&0u16.to_le_bytes());
    pgen.extend_from_slice(&[0u8; 4]);

    let mut inst = Vec::new();
    inst.extend_from_slice(&name20("Sine"));
    inst.extend_from_slice(&0u16.to_le_bytes());
    inst.extend_from_slice(&name20("EOI"));
    inst.extend_from_slice(&(zones.len() as u16).to_le_bytes());

    let mut ibag = Vec::new();
    let mut igen = Vec::new();
    let mut imod = Vec::new();
    let (mut gen_index, mut mod_index) = (0u16, 0u16);
    for zone in zones {
        ibag.extend_from_slice(&gen_index.to_le_bytes());
        ibag.extend_from_slice(&mod_index.to_le_bytes());
        if let Some((lo, hi)) = zone.key_range {
            igen.extend_from_slice(&43u16.to_le_bytes());
            igen.extend_from_slice(&[lo, hi]);
            gen_index += 1;
        }
        for &(op, amount) in &zone.generators {
            igen.extend_from_slice(&op.to_le_bytes());
            igen.extend_from_slice(&amount.to_le_bytes());
            gen_index += 1;
        }
        igen.extend_from_slice(&53u16.to_le_bytes());
        igen.extend_from_slice(&0u16.to_le_bytes());
        gen_index += 1;
        for &(source, destination, amount, amount_source, transform) in &zone.modulators {
            imod.extend_from_slice(&source.to_le_bytes());
            imod.extend_from_slice(&destination.to_le_bytes());
            imod.extend_from_slice(&amount.to_le_bytes());
            imod.extend_from_slice(&amount_source.to_le_bytes());
            imod.extend_from_slice(&transform.to_le_bytes());
            mod_index += 1;
        }
    }
    ibag.extend_from_slice(&gen_index.to_le_bytes());
    ibag.extend_from_slice(&mod_index.to_le_bytes());
    igen.extend_from_slice(&[0u8; 4]);
    imod.extend_from_slice(&[0u8; 10]);

    let mut shdr = Vec::new();
    shdr.extend_from_slice(&name20("Sine441"));
    for v in [0u32, SINE_LENGTH, loop_start, loop_end, SAMPLE_RATE] {
        shdr.extend_from_slice(&v.to_le_bytes());
    }
    shdr.push(69); // original pitch
    shdr.push(0); // pitch correction
    shdr.extend_from_slice(&0u16.to_le_bytes()); // link
    shdr.extend_from_slice(&1u16.to_le_bytes()); // mono
    shdr.extend_from_slice(&name20("EOS"));
    shdr.extend_from_slice(&[0u8; 26]);

    let pdta = list(
        b"pdta",
        &[
            chunk(b"phdr", &phdr),
            chunk(b"pbag", &pbag),
            chunk(b"pmod", &pmod),
            chunk(b"pgen", &pgen),
            chunk(b"inst", &inst),
            chunk(b"ibag", &ibag),
            chunk(b"imod", &imod),
            chunk(b"igen", &igen),
            chunk(b"shdr", &shdr),
        ],
    );

    let mut body = b"sfbk".to_vec();
    body.extend_from_slice(&info);
    body.extend_from_slice(&sdta);
    body.extend_from_slice(&pdta);
    chunk(b"RIFF", &body)
}

/// A looping zone with a 0.5 s release (-1200 timecents).
pub fn looping_zone() -> TestZone {
    TestZone { generators: vec![(54, 1), (38, -1200)], ..Default::default() }
}

/// A format 0 MIDI file at 480 ticks per quarter and 120 BPM (1 tick =
/// 1/960 s) with `events` = (tick, bytes).
pub fn midi_file(events: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut track = Vec::new();
    let mut last = 0;
    for (tick, bytes) in events {
        write_variable_length(&mut track, tick - last);
        last = *tick;
        track.extend_from_slice(bytes);
    }
    write_variable_length(&mut track, 0);
    track.extend_from_slice(&[0xff, 0x2f, 0x00]);

    let mut out = b"MThd".to_vec();
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&480u16.to_be_bytes());
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&(track.len() as u32).to_be_bytes());
    out.extend_from_slice(&track);
    out
}

fn write_variable_length(out: &mut Vec<u8>, mut value: u32) {
    let mut bytes = vec![(value & 0x7f) as u8];
    value >>= 7;
    while value > 0 {
        bytes.push((value & 0x7f) as u8 | 0x80);
        value >>= 7;
    }
    bytes.reverse();
    out.extend_from_slice(&bytes);
}

/// The frequency of a (roughly sinusoidal) signal from its upward zero
/// crossings.
pub fn frequency(signal: &[f32], sample_rate: u32) -> f64 {
    let mut crossings = Vec::new();
    for i in 1..signal.len() {
        let (a, b) = (signal[i - 1], signal[i]);
        if a < 0.0 && b >= 0.0 {
            crossings.push(i as f64 - 1.0 + (-a / (b - a)) as f64);
        }
    }
    assert!(crossings.len() > 2, "no oscillation");
    let span = crossings.last().unwrap() - crossings.first().unwrap();
    (crossings.len() - 1) as f64 / span * sample_rate as f64
}

pub fn rms(signal: &[f32]) -> f64 {
    (signal.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / signal.len().max(1) as f64).sqrt()
}

pub fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
}
