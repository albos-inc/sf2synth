//! Loading DLS (Downloadable Sounds, levels 1 and 2) files, such as the
//! General MIDI set built into macOS. A DLS file becomes the same presets,
//! instruments, zones and samples an SF2 file does; its articulation
//! (connection blocks) is translated to SF2 generators and modulators.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::generator::{self as g, GeneratorSet};
use crate::modulator::{Modulator, controller, source};
use crate::soundfont::{FileFormat, Instrument, Preset, SampleHeader, SoundFontInfo, Zone};

/// A DLS file as SF2 structures, with its sample points gathered into one
/// block of 16-bit little-endian points.
pub(crate) struct Converted {
    pub info: SoundFontInfo,
    pub presets: Vec<Preset>,
    pub instruments: Vec<Instrument>,
    pub samples: Vec<SampleHeader>,
    pub points: Vec<u8>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSoundFont(message.into())
}

fn le16(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

struct Chunk<'a> {
    id: [u8; 4],
    data: &'a [u8],
}

impl Chunk<'_> {
    /// For a LIST: its type and its sub-chunks.
    fn list(&self) -> Option<([u8; 4], Vec<Chunk<'_>>)> {
        if &self.id != b"LIST" || self.data.len() < 4 {
            return None;
        }
        Some(([self.data[0], self.data[1], self.data[2], self.data[3]], chunks(&self.data[4..])))
    }
}

fn chunks(data: &[u8]) -> Vec<Chunk<'_>> {
    let mut result = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let id = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
        let size = le32(data, pos + 4) as usize;
        let start = pos + 8;
        let end = start.saturating_add(size).min(data.len());
        result.push(Chunk { id, data: &data[start..end] });
        pos = start.saturating_add(size).saturating_add(size & 1);
    }
    result
}

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim_end().to_string()
}

// Connection sources, controls and destinations (DLS 1 and 2).
const SRC_NONE: u16 = 0x0000;
const SRC_LFO: u16 = 0x0001;
const SRC_KEY_ON_VELOCITY: u16 = 0x0002;
const SRC_KEY_NUMBER: u16 = 0x0003;
const SRC_EG2: u16 = 0x0005;
const SRC_CC1: u16 = 0x0081;
const SRC_VIBRATO: u16 = 0x0009;

const DST_ATTENUATION: u16 = 0x0001;
const DST_PITCH: u16 = 0x0003;
const DST_PAN: u16 = 0x0004;
const DST_CHORUS: u16 = 0x0080;
const DST_REVERB: u16 = 0x0081;
const DST_LFO_FREQUENCY: u16 = 0x0104;
const DST_LFO_START_DELAY: u16 = 0x0105;
const DST_VIB_FREQUENCY: u16 = 0x0114;
const DST_VIB_START_DELAY: u16 = 0x0115;
const DST_EG1_ATTACK: u16 = 0x0206;
const DST_EG1_DECAY: u16 = 0x0207;
const DST_EG1_RELEASE: u16 = 0x0209;
const DST_EG1_SUSTAIN: u16 = 0x020a;
const DST_EG1_DELAY: u16 = 0x020b;
const DST_EG1_HOLD: u16 = 0x020c;
const DST_EG2_ATTACK: u16 = 0x030a;
const DST_EG2_DECAY: u16 = 0x030b;
const DST_EG2_RELEASE: u16 = 0x030d;
const DST_EG2_SUSTAIN: u16 = 0x030e;
const DST_EG2_DELAY: u16 = 0x030f;
const DST_EG2_HOLD: u16 = 0x0310;
const DST_FILTER_CUTOFF: u16 = 0x0500;
const DST_FILTER_Q: u16 = 0x0501;

/// A connection block: `source × control × scale` added to `destination`.
#[derive(Clone, Copy, Debug)]
struct Connection {
    source: u16,
    control: u16,
    destination: u16,
    scale: i32,
}

/// The connections of an `art1` / `art2` chunk.
fn connections(data: &[u8]) -> Vec<Connection> {
    let header = le32(data, 0) as usize;
    let count = (le32(data, 4) as usize).min(data.len() / 12);
    (0..count)
        .map_while(|i| {
            let at = header + i * 12;
            (at + 12 <= data.len()).then(|| Connection {
                source: le16(data, at),
                control: le16(data, at + 2),
                destination: le16(data, at + 4),
                scale: le32(data, at + 8) as i32,
            })
        })
        .collect()
}

/// The connections of a `lart` / `lar2` list, if the chunks hold one.
fn articulation(subchunks: &[Chunk<'_>]) -> Option<Vec<Connection>> {
    let mut found = None;
    for c in subchunks {
        if let Some((kind, inner)) = c.list() {
            if &kind == b"lart" || &kind == b"lar2" {
                let all: Vec<Connection> = inner
                    .iter()
                    .filter(|a| &a.id == b"art1" || &a.id == b"art2")
                    .flat_map(|a| connections(a.data))
                    .collect();
                found = Some(all);
            }
        }
    }
    found
}

/// Sample information of a `wsmp` chunk.
#[derive(Clone, Copy, Debug, Default)]
struct WaveSample {
    unity_note: u16,
    fine_tune: i16,
    /// Centibels (positive = louder).
    gain: f32,
    /// (start, length, release loop)
    lp: Option<(u32, u32, bool)>,
}

fn wave_sample(data: &[u8]) -> WaveSample {
    let size = le32(data, 0) as usize;
    let gain = le32(data, 8) as i32;
    let loops = le32(data, 16);
    let lp = (loops > 0 && size + 16 <= data.len()).then(|| {
        let at = size;
        let kind = le32(data, at + 4);
        (le32(data, at + 8), le32(data, at + 12), kind == 1)
    });
    WaveSample { unity_note: le16(data, 4), fine_tune: le16(data, 6) as i16, gain: gain as f32 / 65536.0, lp }
}

struct Wave {
    rate: u32,
    /// Index of the first point in the gathered points, and point count.
    start: u32,
    length: u32,
    sample: Option<WaveSample>,
    name: String,
}

/// Seconds-style timecents of a connection scale (`0x80000000` = instant).
fn timecents(scale: i32) -> i16 {
    if scale == i32::MIN { -12000 } else { (scale as f64 / 65536.0).round().clamp(-12000.0, 8000.0) as i16 }
}

fn clamp16(value: f64) -> i16 {
    value.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16
}

/// Translates DLS connections to SF2 generators and modulators.
fn translate(connections: &[Connection], generators: &mut GeneratorSet, modulators: &mut Vec<Modulator>) {
    // DLS has no velocity → filter connection by default; cancel SF2's.
    modulators.push(Modulator {
        source: controller::NOTE_ON_VELOCITY | source::NEGATIVE,
        destination: g::INITIAL_FILTER_FC,
        amount: 0,
        amount_source: 0,
        transform: 0,
    });
    // DLS's LFO defaults to 5 Hz; SF2's to 8.176 Hz.
    let five_hz = -851;
    generators.set(g::FREQ_VIB_LFO, five_hz);
    generators.set(g::FREQ_MOD_LFO, five_hz);
    let mut key_to_eg1_decay = 0.0;
    let mut key_to_eg1_hold = 0.0;
    let mut key_to_eg2_decay = 0.0;
    let mut key_to_eg2_hold = 0.0;
    for c in connections {
        let v = c.scale as f64 / 65536.0;
        match (c.source, c.control, c.destination) {
            (SRC_NONE, SRC_NONE, DST_EG1_ATTACK) => generators.set(g::ATTACK_VOL_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG1_DECAY) => generators.set(g::DECAY_VOL_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG1_RELEASE) => generators.set(g::RELEASE_VOL_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG1_DELAY) => generators.set(g::DELAY_VOL_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG1_HOLD) => generators.set(g::HOLD_VOL_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG1_SUSTAIN) => {
                // A level in 0.1 %, as an attenuation in centibels.
                let level = (v / 1000.0).clamp(0.0, 1.0);
                let attenuation = if level <= 0.0 { 1440.0 } else { -200.0 * level.log10() };
                generators.set(g::SUSTAIN_VOL_ENV, clamp16(attenuation.min(1440.0)));
            }
            (SRC_NONE, SRC_NONE, DST_EG2_ATTACK) => generators.set(g::ATTACK_MOD_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG2_DECAY) => generators.set(g::DECAY_MOD_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG2_RELEASE) => generators.set(g::RELEASE_MOD_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG2_DELAY) => generators.set(g::DELAY_MOD_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG2_HOLD) => generators.set(g::HOLD_MOD_ENV, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_EG2_SUSTAIN) => {
                generators.set(g::SUSTAIN_MOD_ENV, clamp16((1000.0 - v).clamp(0.0, 1000.0)));
            }
            (SRC_NONE, SRC_NONE, DST_LFO_FREQUENCY) => {
                // DLS 1 has one LFO, for vibrato and tremolo alike.
                generators.set(g::FREQ_MOD_LFO, clamp16(v));
                generators.set(g::FREQ_VIB_LFO, clamp16(v));
            }
            (SRC_NONE, SRC_NONE, DST_LFO_START_DELAY) => {
                generators.set(g::DELAY_MOD_LFO, timecents(c.scale));
                generators.set(g::DELAY_VIB_LFO, timecents(c.scale));
            }
            (SRC_NONE, SRC_NONE, DST_VIB_FREQUENCY) => generators.set(g::FREQ_VIB_LFO, clamp16(v)),
            (SRC_NONE, SRC_NONE, DST_VIB_START_DELAY) => generators.set(g::DELAY_VIB_LFO, timecents(c.scale)),
            (SRC_NONE, SRC_NONE, DST_PAN) => generators.set(g::PAN, clamp16(v.clamp(-500.0, 500.0))),
            (SRC_NONE, SRC_NONE, DST_ATTENUATION) => generators.set(g::INITIAL_ATTENUATION, clamp16((-v).max(0.0))),
            (SRC_NONE, SRC_NONE, DST_REVERB) => generators.set(g::REVERB_EFFECTS_SEND, clamp16(v.clamp(0.0, 1000.0))),
            (SRC_NONE, SRC_NONE, DST_CHORUS) => generators.set(g::CHORUS_EFFECTS_SEND, clamp16(v.clamp(0.0, 1000.0))),
            (SRC_NONE, SRC_NONE, DST_FILTER_CUTOFF) => {
                // 0x7FFFFFFF means no filter.
                if c.scale != i32::MAX {
                    generators.set(g::INITIAL_FILTER_FC, clamp16(v.clamp(1500.0, 13500.0)));
                }
            }
            (SRC_NONE, SRC_NONE, DST_FILTER_Q) => generators.set(g::INITIAL_FILTER_Q, clamp16(v.clamp(0.0, 960.0))),
            (SRC_LFO, SRC_NONE, DST_PITCH) => generators.set(g::MOD_LFO_TO_PITCH, clamp16(v)),
            (SRC_VIBRATO, SRC_NONE, DST_PITCH) => generators.set(g::VIB_LFO_TO_PITCH, clamp16(v)),
            (SRC_LFO, SRC_NONE, DST_ATTENUATION) => generators.set(g::MOD_LFO_TO_VOLUME, clamp16(v)),
            (SRC_LFO, SRC_NONE, DST_FILTER_CUTOFF) => generators.set(g::MOD_LFO_TO_FILTER_FC, clamp16(v)),
            (SRC_LFO | SRC_VIBRATO, SRC_CC1, DST_PITCH) => modulators.push(Modulator {
                source: 1 | source::CC,
                destination: g::VIB_LFO_TO_PITCH,
                amount: clamp16(v),
                amount_source: 0,
                transform: 0,
            }),
            (SRC_LFO, SRC_CC1, DST_ATTENUATION) => modulators.push(Modulator {
                source: 1 | source::CC,
                destination: g::MOD_LFO_TO_VOLUME,
                amount: clamp16(v),
                amount_source: 0,
                transform: 0,
            }),
            (SRC_EG2, SRC_NONE, DST_PITCH) => generators.set(g::MOD_ENV_TO_PITCH, clamp16(v)),
            (SRC_EG2, SRC_NONE, DST_FILTER_CUTOFF) => generators.set(g::MOD_ENV_TO_FILTER_FC, clamp16(v)),
            (SRC_KEY_ON_VELOCITY, SRC_NONE, DST_EG1_ATTACK) => modulators.push(Modulator {
                source: controller::NOTE_ON_VELOCITY,
                destination: g::ATTACK_VOL_ENV,
                amount: clamp16(v),
                amount_source: 0,
                transform: 0,
            }),
            (SRC_KEY_ON_VELOCITY, SRC_NONE, DST_ATTENUATION) => {
                // Replaces the default velocity curve (DLS's is the same,
                // concave over 96 dB) with this file's depth.
                modulators.push(Modulator {
                    source: controller::NOTE_ON_VELOCITY | source::NEGATIVE | source::CONCAVE,
                    destination: g::INITIAL_ATTENUATION,
                    amount: clamp16((-v).clamp(0.0, 1440.0)),
                    amount_source: 0,
                    transform: 0,
                });
            }
            (SRC_KEY_ON_VELOCITY, SRC_NONE, DST_FILTER_CUTOFF) => modulators.push(Modulator {
                source: controller::NOTE_ON_VELOCITY,
                destination: g::INITIAL_FILTER_FC,
                amount: clamp16(v),
                amount_source: 0,
                transform: 0,
            }),
            (SRC_KEY_NUMBER, SRC_NONE, DST_EG1_DECAY) => key_to_eg1_decay = v,
            (SRC_KEY_NUMBER, SRC_NONE, DST_EG1_HOLD) => key_to_eg1_hold = v,
            (SRC_KEY_NUMBER, SRC_NONE, DST_EG2_DECAY) => key_to_eg2_decay = v,
            (SRC_KEY_NUMBER, SRC_NONE, DST_EG2_HOLD) => key_to_eg2_hold = v,
            // Pitch wheel, CC7, CC10, CC11, CC91, CC93: the SF2 defaults do the same.
            _ => {}
        }
    }
    // DLS adds `scale × key / 128`; SF2 adds `keynumTo… × (60 - key)` to a
    // base time at key 60.
    for (scale, base, per_key) in [
        (key_to_eg1_decay, g::DECAY_VOL_ENV, g::KEYNUM_TO_VOL_ENV_DECAY),
        (key_to_eg1_hold, g::HOLD_VOL_ENV, g::KEYNUM_TO_VOL_ENV_HOLD),
        (key_to_eg2_decay, g::DECAY_MOD_ENV, g::KEYNUM_TO_MOD_ENV_DECAY),
        (key_to_eg2_hold, g::HOLD_MOD_ENV, g::KEYNUM_TO_MOD_ENV_HOLD),
    ] {
        if scale != 0.0 {
            let at_60 = generators.get(base).unwrap_or(g::default_value(base)) as f64 + scale * 60.0 / 128.0;
            generators.set(base, clamp16(at_60.clamp(-12000.0, 8000.0)));
            generators.set(per_key, clamp16((-scale / 128.0).clamp(-1200.0, 1200.0)));
        }
    }
}

pub(crate) fn parse(file: &[u8]) -> Result<Converted> {
    if file.len() < 12 || &file[0..4] != b"RIFF" || &file[8..12] != b"DLS " {
        return Err(invalid("not a RIFF DLS file"));
    }
    let end = (8 + le32(file, 4) as usize).min(file.len());
    let top = chunks(&file[12..end]);

    let mut info = SoundFontInfo { format: FileFormat::Dls, ..Default::default() };
    let mut cues: Vec<u32> = Vec::new();
    let mut pool: &[u8] = &[];
    let mut instrument_lists = Vec::new();
    for c in &top {
        match &c.id {
            b"ptbl" => {
                let header = le32(c.data, 0) as usize;
                // No more cues than the chunk holds, whatever it claims.
                let count = (le32(c.data, 4) as usize).min(c.data.len().saturating_sub(header) / 4);
                cues = (0..count).map(|i| le32(c.data, header + i * 4)).collect();
            }
            b"vers" => info.version = (le16(c.data, 2), le16(c.data, 0)),
            b"LIST" => match c.list() {
                Some((kind, inner)) if &kind == b"wvpl" => {
                    // Cue offsets count from just after the list type.
                    pool = &c.data[4..];
                    let _ = inner;
                }
                Some((kind, inner)) if &kind == b"lins" => instrument_lists = inner,
                Some((kind, inner)) if &kind == b"INFO" => {
                    for i in inner {
                        match &i.id {
                            b"INAM" => info.name = text(i.data),
                            b"ICOP" => info.copyright = text(i.data),
                            b"ICMT" => info.comment = text(i.data),
                            b"IENG" => info.engineers = text(i.data),
                            b"ICRD" => info.date = text(i.data),
                            b"ISFT" => info.software = text(i.data),
                            b"IPRD" => info.product = text(i.data),
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    // Waves, gathered into one block of 16-bit points (46 silent points after each).
    let mut points: Vec<u8> = Vec::new();
    let mut waves: Vec<Option<Wave>> = Vec::with_capacity(cues.len());
    let mut by_offset: HashMap<u32, usize> = HashMap::new();
    for &offset in &cues {
        if let Some(&index) = by_offset.get(&offset) {
            waves.push(waves[index].as_ref().map(|w| Wave { name: w.name.clone(), ..*w }));
            continue;
        }
        by_offset.insert(offset, waves.len());
        let wave = pool.get(offset as usize..).and_then(|rest| chunks(rest).into_iter().next()).and_then(|chunk| {
            let (kind, inner) = chunk.list()?;
            if &kind != b"wave" {
                return None;
            }
            let format = inner.iter().find(|c| &c.id == b"fmt ")?;
            let data = inner.iter().find(|c| &c.id == b"data")?;
            let (tag, channels, rate, bits) = (
                le16(format.data, 0),
                le16(format.data, 2).max(1) as usize,
                le32(format.data, 4),
                le16(format.data, 14),
            );
            if tag != 1 || !(bits == 8 || bits == 16) {
                return None;
            }
            let frame = channels * bits as usize / 8;
            let frames = data.data.len() / frame;
            let start = (points.len() / 2) as u32;
            for f in 0..frames {
                // The first channel of each frame.
                let at = f * frame;
                let value = if bits == 8 {
                    ((data.data[at] as i16) - 128) << 8
                } else {
                    i16::from_le_bytes([data.data[at], data.data[at + 1]])
                };
                points.extend_from_slice(&value.to_le_bytes());
            }
            points.extend_from_slice(&[0u8; 92]);
            let sample = inner.iter().find(|c| &c.id == b"wsmp").map(|c| wave_sample(c.data));
            let name = inner
                .iter()
                .filter_map(Chunk::list)
                .find(|(kind, _)| kind == b"INFO")
                .and_then(|(_, list)| list.into_iter().find(|i| &i.id == b"INAM").map(|i| text(i.data)))
                .unwrap_or_default();
            Some(Wave { rate, start, length: frames as u32, sample, name })
        });
        waves.push(wave);
    }

    let mut samples: Vec<SampleHeader> = Vec::new();
    let mut instruments = Vec::new();
    let mut presets = Vec::new();
    for list in &instrument_lists {
        let Some((kind, parts)) = list.list() else { continue };
        if &kind != b"ins " {
            continue;
        }
        let Some(header) = parts.iter().find(|c| &c.id == b"insh") else { continue };
        let bank_bits = le32(header.data, 4);
        let program = (le32(header.data, 8) & 0x7f) as u16;
        let drums = bank_bits & 0x8000_0000 != 0;
        let bank = if drums { 128 } else { ((bank_bits >> 8) & 0x7f) as u16 };
        let name = parts
            .iter()
            .filter_map(Chunk::list)
            .find(|(kind, _)| kind == b"INFO")
            .and_then(|(_, list)| list.into_iter().find(|i| &i.id == b"INAM").map(|i| text(i.data)))
            .unwrap_or_else(|| format!("Instrument {bank}:{program}"));
        let instrument_articulation = articulation(&parts);

        let mut zones = Vec::new();
        for region_list in parts.iter().filter_map(Chunk::list).filter(|(k, _)| k == b"lrgn").flat_map(|(_, r)| r) {
            let Some((kind, region)) = region_list.list() else { continue };
            if &kind != b"rgn " && &kind != b"rgn2" {
                continue;
            }
            let Some(rgnh) = region.iter().find(|c| &c.id == b"rgnh") else { continue };
            let Some(wlnk) = region.iter().find(|c| &c.id == b"wlnk") else { continue };
            let cue = le32(wlnk.data, 8) as usize;
            let Some(Some(wave)) = waves.get(cue) else { continue };
            let key = (le16(rgnh.data, 0).min(127) as u8, le16(rgnh.data, 2).min(127) as u8);
            let velocity = (le16(rgnh.data, 4).min(127) as u8, le16(rgnh.data, 6).min(127) as u8);
            let velocity = if velocity == (0, 0) { (0, 127) } else { velocity };
            let key_group = le16(rgnh.data, 10);

            // The region's sample settings override the wave's.
            let sample_info = region
                .iter()
                .find(|c| &c.id == b"wsmp")
                .map(|c| wave_sample(c.data))
                .or(wave.sample)
                .unwrap_or(WaveSample { unity_note: 60, ..Default::default() });
            let (loop_start, loop_end, release_loop) = match sample_info.lp {
                Some((start, length, release)) if length >= 2 && start + length <= wave.length => {
                    (wave.start + start, wave.start + start + length, Some(release))
                }
                _ => (wave.start, wave.start, None),
            };
            let sample_index = samples.len();
            samples.push(SampleHeader {
                name: if wave.name.is_empty() { format!("{name} {}", zones.len()) } else { wave.name.clone() },
                start: wave.start,
                end: wave.start + wave.length,
                loop_start,
                loop_end,
                sample_rate: wave.rate,
                original_pitch: sample_info.unity_note.min(127) as u8,
                pitch_correction: 0,
                link: 0,
                sample_type: 1,
            });

            let mut generators = GeneratorSet::default();
            let mut modulators = Vec::new();
            let connections = articulation(&region).or_else(|| instrument_articulation.clone()).unwrap_or_default();
            translate(&connections, &mut generators, &mut modulators);
            if let Some(release) = release_loop {
                generators.set(g::SAMPLE_MODES, if release { 3 } else { 1 });
            }
            if sample_info.fine_tune != 0 {
                let total = sample_info.fine_tune as i32;
                generators.set(g::COARSE_TUNE, (total / 100) as i16);
                generators.set(g::FINE_TUNE, (total % 100) as i16);
            }
            if sample_info.gain != 0.0 {
                let current = generators.get(g::INITIAL_ATTENUATION).unwrap_or(0) as f32;
                generators.set(g::INITIAL_ATTENUATION, (current - sample_info.gain).clamp(0.0, 1440.0).round() as i16);
            }
            if key_group != 0 {
                generators.set(g::EXCLUSIVE_CLASS, key_group as i16);
            }
            zones.push(Zone {
                key_lo: key.0,
                key_hi: key.1.max(key.0),
                vel_lo: velocity.0,
                vel_hi: velocity.1.max(velocity.0),
                generators,
                modulators,
                link: sample_index,
            });
        }
        if zones.is_empty() {
            continue;
        }
        let instrument_index = instruments.len();
        instruments.push(Instrument { name: name.clone(), zones });
        presets.push(Preset {
            name,
            bank,
            program,
            zones: vec![Zone {
                key_lo: 0,
                key_hi: 127,
                vel_lo: 0,
                vel_hi: 127,
                generators: GeneratorSet::default(),
                modulators: Vec::new(),
                link: instrument_index,
            }],
        });
    }
    if presets.is_empty() {
        return Err(invalid("the DLS file has no playable instruments"));
    }
    Ok(Converted { info, presets, instruments, samples, points })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn list(kind: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
        let mut data = kind.to_vec();
        for p in parts {
            data.extend_from_slice(p);
        }
        chunk(b"LIST", &data)
    }

    /// A DLS file with one melodic and one drum instrument playing a sine.
    fn sine_dls() -> Vec<u8> {
        let mut pcm = Vec::new();
        for i in 0..22050 {
            let v = (2.0 * std::f64::consts::PI * i as f64 / 50.0).sin() * 12000.0; // 441 Hz at 22.05 kHz
            pcm.extend_from_slice(&(v as i16).to_le_bytes());
        }
        let mut fmt = Vec::new();
        for v in [1u16, 1] {
            fmt.extend_from_slice(&v.to_le_bytes());
        }
        fmt.extend_from_slice(&22050u32.to_le_bytes());
        fmt.extend_from_slice(&44100u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        // wsmp: unity note 69, a loop over the last second's middle.
        let mut wsmp = Vec::new();
        wsmp.extend_from_slice(&20u32.to_le_bytes());
        wsmp.extend_from_slice(&69u16.to_le_bytes());
        wsmp.extend_from_slice(&0i16.to_le_bytes());
        wsmp.extend_from_slice(&0i32.to_le_bytes());
        wsmp.extend_from_slice(&0u32.to_le_bytes());
        wsmp.extend_from_slice(&1u32.to_le_bytes());
        wsmp.extend_from_slice(&16u32.to_le_bytes());
        wsmp.extend_from_slice(&0u32.to_le_bytes());
        wsmp.extend_from_slice(&1000u32.to_le_bytes());
        wsmp.extend_from_slice(&20000u32.to_le_bytes());
        let wave = list(b"wave", &[chunk(b"fmt ", &fmt), chunk(b"wsmp", &wsmp), chunk(b"data", &pcm)]);

        let mut ptbl = Vec::new();
        ptbl.extend_from_slice(&8u32.to_le_bytes());
        ptbl.extend_from_slice(&1u32.to_le_bytes());
        ptbl.extend_from_slice(&0u32.to_le_bytes());

        // Release 0.5 s (-1200 timecents) for the melodic instrument.
        let mut art = Vec::new();
        art.extend_from_slice(&8u32.to_le_bytes());
        art.extend_from_slice(&1u32.to_le_bytes());
        art.extend_from_slice(&SRC_NONE.to_le_bytes());
        art.extend_from_slice(&SRC_NONE.to_le_bytes());
        art.extend_from_slice(&DST_EG1_RELEASE.to_le_bytes());
        art.extend_from_slice(&0u16.to_le_bytes());
        art.extend_from_slice(&((-1200i32) << 16).to_le_bytes());

        let instrument = |bank: u32, program: u32, name: &str, with_art: bool| {
            let mut insh = Vec::new();
            insh.extend_from_slice(&1u32.to_le_bytes());
            insh.extend_from_slice(&bank.to_le_bytes());
            insh.extend_from_slice(&program.to_le_bytes());
            let mut rgnh = Vec::new();
            for v in [0u16, 127, 0, 127, 0, 0] {
                rgnh.extend_from_slice(&v.to_le_bytes());
            }
            let mut wlnk = Vec::new();
            wlnk.extend_from_slice(&0u16.to_le_bytes());
            wlnk.extend_from_slice(&0u16.to_le_bytes());
            wlnk.extend_from_slice(&1u32.to_le_bytes());
            wlnk.extend_from_slice(&0u32.to_le_bytes());
            let region = list(b"rgn ", &[chunk(b"rgnh", &rgnh), chunk(b"wlnk", &wlnk)]);
            let mut name_bytes = name.as_bytes().to_vec();
            name_bytes.push(0);
            let mut parts = vec![chunk(b"insh", &insh), list(b"lrgn", &[region])];
            if with_art {
                parts.push(list(b"lart", &[chunk(b"art1", &art)]));
            }
            parts.push(list(b"INFO", &[chunk(b"INAM", &name_bytes)]));
            list(b"ins ", &parts)
        };
        let mut colh = Vec::new();
        colh.extend_from_slice(&2u32.to_le_bytes());
        let mut body = b"DLS ".to_vec();
        body.extend(chunk(b"colh", &colh));
        body.extend(list(b"lins", &[instrument(0, 5, "Sine", true), instrument(0x8000_0000, 0, "Drums", false)]));
        body.extend(chunk(b"ptbl", &ptbl));
        body.extend(list(b"wvpl", &[wave]));
        body.extend(list(b"INFO", &[chunk(b"INAM", b"Test DLS\0")]));
        chunk(b"RIFF", &body)
    }

    #[test]
    fn truncated_or_corrupt_files_never_panic() {
        let bytes = sine_dls();
        for cut in (0..bytes.len()).step_by(7) {
            let _ = parse(&bytes[..cut]);
        }
        let pcm_start = bytes.windows(4).position(|w| w == b"data").unwrap();
        for i in (12..pcm_start).chain(bytes.len() - 600..bytes.len()) {
            let mut corrupt = bytes.clone();
            corrupt[i] ^= 0xff;
            let _ = parse(&corrupt);
        }
    }

    #[test]
    fn converts_instruments_regions_and_articulation() {
        let converted = parse(&sine_dls()).unwrap();
        assert_eq!(converted.info.name, "Test DLS");
        assert_eq!(converted.presets.len(), 2);
        let sine = &converted.presets[0];
        assert_eq!((sine.bank, sine.program), (0, 5));
        assert_eq!(converted.presets[1].bank, 128);
        let zone = &converted.instruments[0].zones[0];
        assert_eq!(zone.generators.get(g::RELEASE_VOL_ENV), Some(-1200));
        assert_eq!(zone.generators.get(g::SAMPLE_MODES), Some(1));
        let sample = &converted.samples[zone.link];
        assert_eq!(sample.original_pitch, 69);
        assert_eq!(sample.end - sample.start, 22050);
        assert_eq!(sample.loop_end - sample.loop_start, 20000);
        assert_eq!(sample.sample_rate, 22050);
        assert_eq!(converted.points.len(), (22050 + 46) * 2, "a wave used twice is gathered once");
        assert_eq!(converted.samples[0].start, converted.samples[1].start);
    }
}
