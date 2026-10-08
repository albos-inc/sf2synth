//! Writing SF2 files.
//!
//! Build an [`Sf2Builder`] — samples, instruments made of zones that play
//! them, presets made of zones that play instruments — and turn it into the
//! bytes of an SF2 2.01 file with [`Sf2Builder::to_bytes`].
//!
//! ```
//! use sf2synth::generator as g;
//! use sf2synth::writer::{InstrumentDef, PresetDef, SampleDef, Sf2Builder, ZoneDef};
//!
//! let sine: Vec<i16> = (0..4410).map(|i| ((i as f32 / 100.0 * std::f32::consts::TAU).sin() * 16000.0) as i16).collect();
//! let mut sf2 = Sf2Builder::new("Sine");
//! let sample = sf2.add_sample(SampleDef::mono("sine", sine, 44100, 69).looped(0, 4400));
//! let instrument = sf2.add_instrument(InstrumentDef::new("Sine").zone(
//!     ZoneDef::sample(sample).generator(g::SAMPLE_MODES, 1).generator(g::RELEASE_VOL_ENV, -1200),
//! ));
//! sf2.add_preset(PresetDef::new("Sine", 0, 0).zone(ZoneDef::instrument(instrument)));
//! let bytes = sf2.to_bytes();
//! let font = sf2synth::SoundFont::from_bytes(bytes).unwrap();
//! assert_eq!(font.presets()[0].name(), "Sine");
//! ```

use crate::generator as g;
use crate::modulator::Modulator;

/// A sample: 16-bit mono points and how to play them.
#[derive(Clone, Debug)]
pub struct SampleDef {
    pub name: String,
    pub points: Vec<i16>,
    pub sample_rate: u32,
    /// MIDI key the sample was recorded at.
    pub original_pitch: u8,
    /// Cents.
    pub pitch_correction: i8,
    /// Loop start and end (end exclusive), in points from the sample's start.
    pub loop_range: (u32, u32),
    /// 1 mono, 2 right, 4 left (of a stereo pair).
    pub sample_type: u16,
    /// The other sample of a stereo pair (index), or 0.
    pub link: u16,
}

impl SampleDef {
    pub fn mono(name: impl Into<String>, points: Vec<i16>, sample_rate: u32, original_pitch: u8) -> SampleDef {
        let end = points.len() as u32;
        SampleDef {
            name: name.into(),
            points,
            sample_rate,
            original_pitch,
            pitch_correction: 0,
            loop_range: (end.saturating_sub(1), end.saturating_sub(1)),
            sample_type: 1,
            link: 0,
        }
    }

    pub fn looped(mut self, start: u32, end: u32) -> SampleDef {
        self.loop_range = (start, end);
        self
    }
}

/// A zone: key and velocity ranges, generators, modulators, and the sample
/// (instrument zones) or instrument (preset zones) it plays.
#[derive(Clone, Debug, Default)]
pub struct ZoneDef {
    pub key_range: Option<(u8, u8)>,
    pub velocity_range: Option<(u8, u8)>,
    pub generators: Vec<(u16, i16)>,
    pub modulators: Vec<Modulator>,
    /// Sample or instrument index; `None` for a global zone.
    pub link: Option<u16>,
}

impl ZoneDef {
    pub fn sample(index: u16) -> ZoneDef {
        ZoneDef { link: Some(index), ..Default::default() }
    }

    pub fn instrument(index: u16) -> ZoneDef {
        ZoneDef { link: Some(index), ..Default::default() }
    }

    pub fn global() -> ZoneDef {
        ZoneDef::default()
    }

    pub fn keys(mut self, low: u8, high: u8) -> ZoneDef {
        self.key_range = Some((low, high));
        self
    }

    pub fn velocities(mut self, low: u8, high: u8) -> ZoneDef {
        self.velocity_range = Some((low, high));
        self
    }

    /// Sets a generator (replacing an earlier value for it).
    pub fn generator(mut self, generator: u16, value: i16) -> ZoneDef {
        self.set(generator, value);
        self
    }

    pub fn set(&mut self, generator: u16, value: i16) {
        match self.generators.iter_mut().find(|(op, _)| *op == generator) {
            Some(entry) => entry.1 = value,
            None => self.generators.push((generator, value)),
        }
    }

    pub fn get(&self, generator: u16) -> Option<i16> {
        self.generators.iter().find(|(op, _)| *op == generator).map(|&(_, v)| v)
    }

    pub fn modulator(mut self, modulator: Modulator) -> ZoneDef {
        self.modulators.push(modulator);
        self
    }
}

#[derive(Clone, Debug)]
pub struct InstrumentDef {
    pub name: String,
    pub global: Option<ZoneDef>,
    pub zones: Vec<ZoneDef>,
}

impl InstrumentDef {
    pub fn new(name: impl Into<String>) -> InstrumentDef {
        InstrumentDef { name: name.into(), global: None, zones: Vec::new() }
    }

    pub fn zone(mut self, zone: ZoneDef) -> InstrumentDef {
        self.zones.push(zone);
        self
    }
}

#[derive(Clone, Debug)]
pub struct PresetDef {
    pub name: String,
    pub bank: u16,
    pub program: u16,
    pub global: Option<ZoneDef>,
    pub zones: Vec<ZoneDef>,
}

impl PresetDef {
    pub fn new(name: impl Into<String>, bank: u16, program: u16) -> PresetDef {
        PresetDef { name: name.into(), bank, program, global: None, zones: Vec::new() }
    }

    pub fn zone(mut self, zone: ZoneDef) -> PresetDef {
        self.zones.push(zone);
        self
    }
}

/// An SF2 file being built.
#[derive(Clone, Debug)]
pub struct Sf2Builder {
    pub name: String,
    pub copyright: String,
    pub comment: String,
    pub engineers: String,
    pub samples: Vec<SampleDef>,
    pub instruments: Vec<InstrumentDef>,
    pub presets: Vec<PresetDef>,
}

impl Sf2Builder {
    pub fn new(name: impl Into<String>) -> Sf2Builder {
        Sf2Builder {
            name: name.into(),
            copyright: String::new(),
            comment: String::new(),
            engineers: String::new(),
            samples: Vec::new(),
            instruments: Vec::new(),
            presets: Vec::new(),
        }
    }

    /// Adds a sample; returns its index.
    pub fn add_sample(&mut self, sample: SampleDef) -> u16 {
        self.samples.push(sample);
        (self.samples.len() - 1) as u16
    }

    /// Adds an instrument; returns its index.
    pub fn add_instrument(&mut self, instrument: InstrumentDef) -> u16 {
        self.instruments.push(instrument);
        (self.instruments.len() - 1) as u16
    }

    pub fn add_preset(&mut self, preset: PresetDef) {
        self.presets.push(preset);
    }

    /// The SF2 file.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut info = Vec::new();
        info.extend(chunk(b"ifil", &[2, 0, 1, 0]));
        info.extend(chunk(b"isng", &text(b"EMU8000", 8)));
        info.extend(chunk(b"INAM", &zstring(&self.name)));
        if !self.engineers.is_empty() {
            info.extend(chunk(b"IENG", &zstring(&self.engineers)));
        }
        if !self.copyright.is_empty() {
            info.extend(chunk(b"ICOP", &zstring(&self.copyright)));
        }
        if !self.comment.is_empty() {
            info.extend(chunk(b"ICMT", &zstring(&self.comment)));
        }
        info.extend(chunk(b"ISFT", &zstring(concat!("sf2synth ", env!("CARGO_PKG_VERSION")))));

        // Sample data: each sample followed by 46 zero points.
        let mut smpl = Vec::new();
        let mut shdr = Vec::new();
        let mut position = 0u32;
        for sample in &self.samples {
            for point in &sample.points {
                smpl.extend_from_slice(&point.to_le_bytes());
            }
            smpl.extend_from_slice(&[0u8; 92]);
            let start = position;
            let end = start + sample.points.len() as u32;
            shdr.extend_from_slice(&text(sample.name.as_bytes(), 20));
            for value in [
                start,
                end,
                start + sample.loop_range.0.min(sample.points.len() as u32),
                start + sample.loop_range.1.min(sample.points.len() as u32),
                sample.sample_rate,
            ] {
                shdr.extend_from_slice(&value.to_le_bytes());
            }
            shdr.push(sample.original_pitch);
            shdr.push(sample.pitch_correction as u8);
            shdr.extend_from_slice(&sample.link.to_le_bytes());
            shdr.extend_from_slice(&sample.sample_type.to_le_bytes());
            position = end + 46;
        }
        shdr.extend_from_slice(&text(b"EOS", 20));
        shdr.extend_from_slice(&[0u8; 26]);

        let mut hydra = Hydra::default();
        let mut phdr = Vec::new();
        for preset in &self.presets {
            phdr.extend_from_slice(&text(preset.name.as_bytes(), 20));
            phdr.extend_from_slice(&preset.program.to_le_bytes());
            phdr.extend_from_slice(&preset.bank.to_le_bytes());
            phdr.extend_from_slice(&(hydra.preset_bags.len() as u16).to_le_bytes());
            phdr.extend_from_slice(&[0u8; 12]);
            hydra.add_zones(true, preset.global.as_ref(), &preset.zones);
        }
        phdr.extend_from_slice(&text(b"EOP", 20));
        phdr.extend_from_slice(&[0u8; 4]);
        phdr.extend_from_slice(&(hydra.preset_bags.len() as u16).to_le_bytes());
        phdr.extend_from_slice(&[0u8; 12]);

        let mut inst = Vec::new();
        for instrument in &self.instruments {
            inst.extend_from_slice(&text(instrument.name.as_bytes(), 20));
            inst.extend_from_slice(&(hydra.instrument_bags.len() as u16).to_le_bytes());
            hydra.add_zones(false, instrument.global.as_ref(), &instrument.zones);
        }
        inst.extend_from_slice(&text(b"EOI", 20));
        inst.extend_from_slice(&(hydra.instrument_bags.len() as u16).to_le_bytes());

        let bags = |bags: &[(u16, u16)], generators: usize, modulators: usize| -> Vec<u8> {
            let mut out = Vec::new();
            for &(gen_index, mod_index) in bags.iter().chain([(generators as u16, modulators as u16)].iter()) {
                out.extend_from_slice(&gen_index.to_le_bytes());
                out.extend_from_slice(&mod_index.to_le_bytes());
            }
            out
        };
        let generators = |list: &[(u16, [u8; 2])]| -> Vec<u8> {
            let mut out = Vec::new();
            for (op, amount) in list {
                out.extend_from_slice(&op.to_le_bytes());
                out.extend_from_slice(amount);
            }
            out.extend_from_slice(&[0u8; 4]);
            out
        };
        let modulators = |list: &[Modulator]| -> Vec<u8> {
            let mut out = Vec::new();
            for m in list {
                out.extend_from_slice(&m.source.to_le_bytes());
                out.extend_from_slice(&m.destination.to_le_bytes());
                out.extend_from_slice(&m.amount.to_le_bytes());
                out.extend_from_slice(&m.amount_source.to_le_bytes());
                out.extend_from_slice(&m.transform.to_le_bytes());
            }
            out.extend_from_slice(&[0u8; 10]);
            out
        };

        let mut pdta = b"pdta".to_vec();
        pdta.extend(chunk(b"phdr", &phdr));
        pdta.extend(chunk(
            b"pbag",
            &bags(&hydra.preset_bags, hydra.preset_generators.len(), hydra.preset_modulators.len()),
        ));
        pdta.extend(chunk(b"pmod", &modulators(&hydra.preset_modulators)));
        pdta.extend(chunk(b"pgen", &generators(&hydra.preset_generators)));
        pdta.extend(chunk(b"inst", &inst));
        pdta.extend(chunk(
            b"ibag",
            &bags(&hydra.instrument_bags, hydra.instrument_generators.len(), hydra.instrument_modulators.len()),
        ));
        pdta.extend(chunk(b"imod", &modulators(&hydra.instrument_modulators)));
        pdta.extend(chunk(b"igen", &generators(&hydra.instrument_generators)));
        pdta.extend(chunk(b"shdr", &shdr));

        let mut body = b"sfbk".to_vec();
        let mut info_list = b"INFO".to_vec();
        info_list.extend(info);
        body.extend(chunk(b"LIST", &info_list));
        let mut sdta = b"sdta".to_vec();
        sdta.extend(chunk(b"smpl", &smpl));
        body.extend(chunk(b"LIST", &sdta));
        body.extend(chunk(b"LIST", &pdta));
        chunk(b"RIFF", &body)
    }
}

/// The bags, generators and modulators of the presets and instruments.
#[derive(Default)]
struct Hydra {
    preset_bags: Vec<(u16, u16)>,
    preset_generators: Vec<(u16, [u8; 2])>,
    preset_modulators: Vec<Modulator>,
    instrument_bags: Vec<(u16, u16)>,
    instrument_generators: Vec<(u16, [u8; 2])>,
    instrument_modulators: Vec<Modulator>,
}

impl Hydra {
    fn add_zones(&mut self, preset: bool, global: Option<&ZoneDef>, zones: &[ZoneDef]) {
        let terminal = if preset { g::INSTRUMENT } else { g::SAMPLE_ID };
        let (bags, generators, modulators) = if preset {
            (&mut self.preset_bags, &mut self.preset_generators, &mut self.preset_modulators)
        } else {
            (&mut self.instrument_bags, &mut self.instrument_generators, &mut self.instrument_modulators)
        };
        let global = global.map(|z| ZoneDef { link: None, ..z.clone() });
        for zone in global.iter().chain(zones.iter()) {
            bags.push((generators.len() as u16, modulators.len() as u16));
            // keyRange first, velRange second, the link last (SF2 2.04, 8.1.2).
            if let Some((low, high)) = zone.key_range {
                generators.push((g::KEY_RANGE, [low, high]));
            }
            if let Some((low, high)) = zone.velocity_range {
                generators.push((g::VEL_RANGE, [low, high]));
            }
            for &(op, value) in &zone.generators {
                if op != g::KEY_RANGE
                    && op != g::VEL_RANGE
                    && op != terminal
                    && op != g::INSTRUMENT
                    && op != g::SAMPLE_ID
                {
                    generators.push((op, value.to_le_bytes()));
                }
            }
            if let Some(link) = zone.link {
                generators.push((terminal, link.to_le_bytes()));
            }
            modulators.extend_from_slice(&zone.modulators);
        }
    }
}

fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 9);
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

/// `bytes` in a zero-padded field of `length` bytes (truncated to leave a
/// terminating zero).
fn text(bytes: &[u8], length: usize) -> Vec<u8> {
    let mut out = vec![0u8; length];
    let n = bytes.len().min(length - 1);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

/// A zero-terminated string padded to an even length.
fn zstring(value: &str) -> Vec<u8> {
    let mut out: Vec<u8> = value.bytes().filter(|b| b.is_ascii()).take(255).collect();
    out.push(0);
    if out.len() % 2 == 1 {
        out.push(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SoundFont, Synthesizer, SynthesizerSettings};
    use std::sync::Arc;

    fn sine(points: usize) -> Vec<i16> {
        (0..points).map(|i| ((i as f32 / 100.0 * std::f32::consts::TAU).sin() * 16000.0) as i16).collect()
    }

    #[test]
    fn round_trips_through_the_reader() {
        let mut sf2 = Sf2Builder::new("Test");
        sf2.copyright = "(c) nobody".into();
        let low = sf2.add_sample(SampleDef::mono("low", sine(4410), 44100, 57).looped(100, 4100));
        let high = sf2.add_sample(SampleDef::mono("high", sine(2205), 22050, 81));
        let instrument = sf2.add_instrument(
            InstrumentDef {
                name: "Two".into(),
                global: Some(ZoneDef::global().generator(g::RELEASE_VOL_ENV, -1200)),
                zones: Vec::new(),
            }
            .zone(ZoneDef::sample(low).keys(0, 63).generator(g::SAMPLE_MODES, 1))
            .zone(ZoneDef::sample(high).keys(64, 127).velocities(1, 100).generator(g::PAN, -200)),
        );
        sf2.add_preset(PresetDef::new("Two", 0, 5).zone(ZoneDef::instrument(instrument).generator(g::COARSE_TUNE, 1)));

        let font = SoundFont::from_bytes(sf2.to_bytes()).unwrap();
        assert_eq!(font.info().name, "Test");
        assert_eq!(font.info().copyright, "(c) nobody");
        assert_eq!(font.info().version, (2, 1));
        assert_eq!(font.find_preset(0, 5), Some(0));
        let preset = &font.presets()[0];
        assert_eq!(preset.zones()[0].generator(g::COARSE_TUNE), Some(1));
        let instrument = &font.instruments()[preset.zones()[0].link()];
        assert_eq!(instrument.zones().len(), 2);
        let (first, second) = (&instrument.zones()[0], &instrument.zones()[1]);
        assert_eq!(first.key_range(), (0, 63));
        assert_eq!(first.generator(g::RELEASE_VOL_ENV), Some(-1200), "global zone merged");
        assert_eq!(second.velocity_range(), (1, 100));
        assert_eq!(second.generator(g::PAN), Some(-200));
        let samples = font.samples();
        assert_eq!(samples[0].end - samples[0].start, 4410);
        assert_eq!(samples[0].loop_start - samples[0].start, 100);
        assert_eq!(samples[1].sample_rate, 22050);
        assert_eq!(samples[1].start, 4410 + 46);
        let points = font.sample_points(samples[1].start as usize..samples[1].start as usize + 30);
        assert!((points[25] - 16000.0 / 32768.0).abs() < 1e-3);

        let mut synth = Synthesizer::new(Arc::new(font), &SynthesizerSettings::new(44100)).unwrap();
        synth.program_change(0, 5);
        synth.note_on(0, 60, 100);
        let (mut left, mut right) = (vec![0.0; 4410], vec![0.0; 4410]);
        synth.render(&mut left, &mut right);
        assert!(left.iter().any(|&x| x.abs() > 0.01));
    }
}
