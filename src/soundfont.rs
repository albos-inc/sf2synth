//! Loading SF2 files.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::generator::{self as g, GeneratorSet};
use crate::modulator::Modulator;

/// Bytes an SF2 file is read from: a `Vec<u8>`, a memory map, a static
/// slice… The sample data is read from these bytes as it plays and is never
/// copied, so a memory-mapped file costs no heap memory.
pub type SharedBytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// Reads a file an SFZ file refers to ([`SoundFont::from_sfz`]): its path
/// relative to the SFZ file (`/`-separated), and `Some(n)` when its first
/// `n` bytes are enough (more is fine).
pub type FileLoader<'a> = dyn Fn(&str, Option<usize>) -> std::io::Result<Vec<u8>> + Sync + 'a;

/// A parsed SF2 file.
pub struct SoundFont {
    info: SoundFontInfo,
    presets: Vec<Preset>,
    instruments: Vec<Instrument>,
    samples: Vec<SampleHeader>,
    pub(crate) data: SampleData,
    preset_index: HashMap<(u16, u16), usize>,
    /// Round robin counters an SFZ instrument's regions use.
    pub(crate) sfz_counters: usize,
    /// Whether some zone starts on a controller instead of a note.
    pub(crate) has_cc_triggers: bool,
    warnings: Vec<String>,
}

impl std::fmt::Debug for SoundFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SoundFont")
            .field("info", &self.info)
            .field("presets", &self.presets.len())
            .field("instruments", &self.instruments.len())
            .field("samples", &self.samples.len())
            .finish()
    }
}

/// The kind of file a [`SoundFont`] was read from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FileFormat {
    #[default]
    Sf2,
    /// Downloadable Sounds (levels 1 and 2), converted on loading.
    Dls,
    /// An SFZ instrument and its WAV or FLAC samples, converted on loading.
    Sfz,
}

/// The INFO chunk.
#[derive(Clone, Debug, Default)]
pub struct SoundFontInfo {
    pub format: FileFormat,
    /// File version (major, minor), e.g. (2, 1) for SF2 2.01.
    pub version: (u16, u16),
    pub name: String,
    pub date: String,
    pub engineers: String,
    pub product: String,
    pub copyright: String,
    pub comment: String,
    pub software: String,
}

/// A preset (bank, program).
#[derive(Clone, Debug)]
pub struct Preset {
    pub(crate) name: String,
    pub(crate) bank: u16,
    pub(crate) program: u16,
    pub(crate) zones: Vec<Zone>,
}

impl Preset {
    /// The preset's zones, each with its global zone merged in.
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn bank(&self) -> u16 {
        self.bank
    }
    pub fn program(&self) -> u16 {
        self.program
    }
}

/// An instrument.
#[derive(Clone, Debug)]
pub struct Instrument {
    pub(crate) name: String,
    pub(crate) zones: Vec<Zone>,
}

impl Instrument {
    /// The instrument's zones, each with its global zone merged in.
    pub fn zones(&self) -> &[Zone] {
        &self.zones
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A sample header (shdr).
#[derive(Clone, Debug)]
pub struct SampleHeader {
    pub name: String,
    /// First sample point, in sample points from the start of the sample data.
    pub start: u32,
    /// One past the last sample point.
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub sample_rate: u32,
    pub original_pitch: u8,
    /// Cents.
    pub pitch_correction: i8,
    pub link: u16,
    pub sample_type: u16,
}

impl SampleHeader {
    /// Whether the sample is in ROM, which this crate can't play.
    pub fn is_rom(&self) -> bool {
        self.sample_type & 0x8000 != 0
    }
}

/// A zone of a preset or an instrument, with its global zone already
/// merged in.
#[derive(Clone, Debug)]
pub struct Zone {
    pub(crate) key_lo: u8,
    pub(crate) key_hi: u8,
    pub(crate) vel_lo: u8,
    pub(crate) vel_hi: u8,
    pub(crate) generators: GeneratorSet,
    pub(crate) modulators: Vec<Modulator>,
    /// Instrument index (preset zones) or sample index (instrument zones).
    pub(crate) link: usize,
    /// What an SFZ region does beyond SF2 (instrument zones of SFZ files).
    pub(crate) sfz: Option<Arc<crate::sfz::SfzRegion>>,
}

impl Zone {
    pub(crate) fn contains(&self, key: u8, velocity: u8) -> bool {
        (self.key_lo..=self.key_hi).contains(&key) && (self.vel_lo..=self.vel_hi).contains(&velocity)
    }

    /// Lowest and highest key.
    pub fn key_range(&self) -> (u8, u8) {
        (self.key_lo, self.key_hi)
    }

    /// Lowest and highest velocity.
    pub fn velocity_range(&self) -> (u8, u8) {
        (self.vel_lo, self.vel_hi)
    }

    /// The value the zone gives a generator ([`crate::generator`]), if any.
    pub fn generator(&self, generator: u16) -> Option<i16> {
        self.generators.get(generator)
    }

    /// Every generator the zone sets (key and velocity ranges and the
    /// instrument / sample link excluded).
    pub fn generators(&self) -> impl Iterator<Item = (u16, i16)> + '_ {
        (0..crate::generator::COUNT as u16).filter_map(|g| self.generators.get(g).map(|v| (g, v)))
    }

    pub fn modulators(&self) -> &[Modulator] {
        &self.modulators
    }

    /// The instrument (preset zones) or sample (instrument zones) played,
    /// as an index into [`SoundFont::instruments`] or [`SoundFont::samples`].
    pub fn link(&self) -> usize {
        self.link
    }

    /// Whether a first note-on plays the zone — no controller moved yet, the
    /// round robin at its start, the note's random number 0: every SF2 and
    /// DLS zone, and the SFZ regions that aren't release-, legato- or
    /// controller-triggered and are the first of their round robin and of
    /// their random range. The zones a single sample per key and velocity
    /// would be made from.
    pub fn plays_on_first_note_on(&self) -> bool {
        let Some(r) = &self.sfz else { return true };
        let default_cc = |cc: u8| match cc {
            7 => 100,
            10 => 64,
            11 => 127,
            _ => 0,
        };
        matches!(r.trigger, crate::sfz::Trigger::Attack | crate::sfz::Trigger::First)
            && r.cc_trigger.is_none()
            && r.sequence.is_none_or(|(_, position, _)| position == 0)
            && r.random.0 <= 0.0
            && r.cc_conditions.iter().all(|&(cc, low, high)| (low..=high).contains(&default_cc(cc)))
    }

    /// The zone's own gain on top of its generators: an SFZ region's volume
    /// and amplitude (1 for SF2 and DLS zones).
    pub fn gain(&self) -> f32 {
        self.sfz.as_ref().map_or(1.0, |r| r.gain)
    }
}

/// The 16-bit (and optional 24-bit) sample points.
pub(crate) struct SampleData {
    bytes: SharedBytes,
    /// Byte offset of the smpl chunk's data.
    smpl: usize,
    /// Byte offset of the sm24 chunk's data, if present and usable.
    sm24: Option<usize>,
    /// Number of sample points.
    pub len: usize,
}

impl SampleData {
    /// The sample points as slices of the file, for reading many of them
    /// (getting the bytes goes through a dynamic call).
    #[inline]
    pub fn points(&self) -> Points<'_> {
        let bytes = (*self.bytes).as_ref();
        let smpl = bytes.get(self.smpl..self.smpl + self.len * 2).unwrap_or_default();
        let sm24 = self.sm24.and_then(|at| bytes.get(at..at + self.len));
        Points { smpl, sm24 }
    }

    /// The sample point `index` as -1…1. Out of range reads give 0.
    pub fn get(&self, index: usize) -> f32 {
        self.points().get(index)
    }
}

/// The 16-bit sample points (and their 24-bit extension) in place.
#[derive(Clone, Copy)]
pub(crate) struct Points<'a> {
    smpl: &'a [u8],
    sm24: Option<&'a [u8]>,
}

impl Points<'_> {
    /// Fills `out` with the points from `start`, if they are all there.
    #[inline(always)]
    pub fn read<const N: usize>(&self, start: usize, out: &mut [f32; N]) -> bool {
        let Some(bytes) = self.smpl.get(start * 2..(start + N) * 2) else { return false };
        match self.sm24 {
            None => {
                for (point, pair) in out.iter_mut().zip(bytes.chunks_exact(2)) {
                    *point = i16::from_le_bytes([pair[0], pair[1]]) as f32 * (1.0 / 32768.0);
                }
            }
            Some(_) => {
                for (i, point) in out.iter_mut().enumerate() {
                    *point = self.get(start + i);
                }
            }
        }
        true
    }

    /// The sample point `index` as -1…1. Out of range reads give 0.
    #[inline(always)]
    pub fn get(&self, index: usize) -> f32 {
        let Some(&[low_byte, high_byte]) = self.smpl.get(index * 2..index * 2 + 2) else { return 0.0 };
        let high = i16::from_le_bytes([low_byte, high_byte]);
        match self.sm24 {
            None => high as f32 * (1.0 / 32768.0),
            Some(sm24) => {
                let low = sm24.get(index).copied().unwrap_or(0);
                (((high as i32) << 8) | low as i32) as f32 * (1.0 / 8_388_608.0)
            }
        }
    }
}

impl SoundFont {
    /// Parses an SF2 file held in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<SoundFont> {
        Self::from_shared(Arc::new(bytes))
    }

    /// Reads and parses an SF2 file.
    pub fn read<R: Read>(mut reader: R) -> Result<SoundFont> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Self::from_bytes(bytes)
    }

    /// Parses an SF2 file from shared bytes, e.g. a memory map
    /// (`Arc::new(mmap)`), without copying its sample data.
    ///
    /// DLS files are read too (their sample data is then copied).
    pub fn from_shared(bytes: SharedBytes) -> Result<SoundFont> {
        let file: &[u8] = bytes.as_ref().as_ref();
        if file.len() >= 12 && &file[8..12] == b"DLS " {
            let converted = crate::dls::parse(file)?;
            let len = converted.points.len() / 2;
            let mut preset_index = HashMap::new();
            for (i, preset) in converted.presets.iter().enumerate() {
                preset_index.entry((preset.bank, preset.program)).or_insert(i);
            }
            return Ok(SoundFont {
                info: converted.info,
                presets: converted.presets,
                instruments: converted.instruments,
                samples: converted.samples,
                data: SampleData { bytes: Arc::new(converted.points), smpl: 0, sm24: None, len },
                preset_index,
                sfz_counters: 0,
                has_cc_triggers: false,
                warnings: Vec::new(),
            });
        }
        let parsed = parse(file)?;
        let mut preset_index = HashMap::new();
        for (i, preset) in parsed.presets.iter().enumerate() {
            preset_index.entry((preset.bank, preset.program)).or_insert(i);
        }
        Ok(SoundFont {
            info: parsed.info,
            presets: parsed.presets,
            instruments: parsed.instruments,
            samples: parsed.samples,
            data: SampleData { bytes, smpl: parsed.smpl.0, sm24: parsed.sm24, len: parsed.smpl.1 },
            preset_index,
            sfz_counters: 0,
            has_cc_triggers: false,
            warnings: Vec::new(),
        })
    }

    /// Reads an instrument file: an SFZ file (by its `.sfz` extension) with
    /// the WAV or FLAC samples it plays, or else an SF2 or DLS file.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<SoundFont> {
        let path = path.as_ref();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sfz")) {
            Self::open_sfz(path)
        } else {
            Self::from_bytes(std::fs::read(path)?)
        }
    }

    /// Reads an SFZ file and the samples it plays (WAV or FLAC, 8 to 32
    /// bits, mono or stereo), found from the SFZ file's directory. The
    /// samples are decoded into memory (in parallel).
    ///
    /// Regions whose sample can't be read are left out, and opcodes this
    /// crate doesn't follow are ignored; [`warnings`](Self::warnings) lists
    /// both.
    pub fn open_sfz(path: impl AsRef<std::path::Path>) -> Result<SoundFont> {
        let path = path.as_ref();
        let text = std::fs::read(path)?;
        let directory = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let name = path.file_stem().map_or("SFZ".into(), |s| s.to_string_lossy().into_owned());
        let load = |relative: &str, limit: Option<usize>| -> std::io::Result<Vec<u8>> {
            let file = directory.join(relative);
            match limit {
                None => std::fs::read(file),
                Some(limit) => {
                    let mut bytes = Vec::with_capacity(limit);
                    std::fs::File::open(file)?.take(limit as u64).read_to_end(&mut bytes)?;
                    Ok(bytes)
                }
            }
        };
        Self::from_sfz(&String::from_utf8_lossy(&text), &name, &load)
    }

    /// Parses SFZ text, `load(path, limit)` reading the files it refers to
    /// (`#include`s and samples) by their `/`-separated paths relative to
    /// the SFZ file: all of a file, or at least its first `limit` bytes.
    pub fn from_sfz(text: &str, name: &str, load: &FileLoader<'_>) -> Result<SoundFont> {
        let converted = crate::sfz::load(text, name, load)?;
        let mut preset_index = HashMap::new();
        preset_index.insert((0, 0), 0);
        let has_cc_triggers = converted
            .instruments
            .iter()
            .flat_map(|i| &i.zones)
            .any(|z| z.sfz.as_ref().is_some_and(|r| r.cc_trigger.is_some()));
        let len = converted.len;
        Ok(SoundFont {
            info: converted.info,
            presets: converted.presets,
            instruments: converted.instruments,
            samples: converted.samples,
            data: SampleData {
                bytes: Arc::new(converted.points),
                smpl: 0,
                sm24: converted.has_sm24.then_some(len * 2),
                len,
            },
            preset_index,
            sfz_counters: converted.counters,
            has_cc_triggers,
            warnings: converted.warnings,
        })
    }

    /// What loading left out: SFZ opcodes ignored, sample files that
    /// couldn't be read. Empty for SF2 and DLS files.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Whether the samples have 24-bit resolution (an SF2 sm24 chunk, or
    /// SFZ samples of more than 16 bits).
    pub fn is_24_bit(&self) -> bool {
        self.data.sm24.is_some()
    }

    pub fn info(&self) -> &SoundFontInfo {
        &self.info
    }

    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    pub fn instruments(&self) -> &[Instrument] {
        &self.instruments
    }

    pub fn samples(&self) -> &[SampleHeader] {
        &self.samples
    }

    /// Number of sample points in the file's sample data.
    pub fn sample_point_count(&self) -> usize {
        self.data.len
    }

    /// Sample points `range` (indices into the whole sample data, as
    /// [`SampleHeader::start`] etc. count them) as -1…1.
    pub fn sample_points(&self, range: std::ops::Range<usize>) -> Vec<f32> {
        range.map(|i| self.data.get(i)).collect()
    }

    /// The index of the preset `(bank, program)`, if the file has it.
    pub fn find_preset(&self, bank: u16, program: u16) -> Option<usize> {
        self.preset_index.get(&(bank, program)).copied()
    }

    /// The melodic preset to play for bank select MSB `bank` and `program`,
    /// falling back like GS sound modules do: the variation's capital tone
    /// (bank `bank & !7`, as bank 8 is for banks 9…15), then bank 0, then the
    /// first preset.
    pub(crate) fn resolve_melodic(&self, bank: u16, program: u16) -> Option<usize> {
        self.find_preset(bank, program)
            .or_else(|| self.find_preset(bank & !7, program))
            .or_else(|| self.find_preset(0, program))
            .or_else(|| self.first_preset())
    }

    /// The drum kit to play for `program` (bank 128): the kit itself, its
    /// capital kit (program `program & !7`, as Room is for 9…15), the
    /// Standard kit (program 0), then the first preset.
    pub(crate) fn resolve_drum_kit(&self, program: u16) -> Option<usize> {
        self.find_preset(128, program)
            .or_else(|| self.find_preset(128, program & !7))
            .or_else(|| self.find_preset(128, 0))
            .or_else(|| self.first_preset())
    }

    fn first_preset(&self) -> Option<usize> {
        if self.presets.is_empty() { None } else { Some(0) }
    }
}

// MARK: - Parsing

struct Parsed {
    info: SoundFontInfo,
    presets: Vec<Preset>,
    instruments: Vec<Instrument>,
    samples: Vec<SampleHeader>,
    /// (byte offset, sample points)
    smpl: (usize, usize),
    sm24: Option<usize>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSoundFont(message.into())
}

struct Chunk<'a> {
    id: [u8; 4],
    data: &'a [u8],
    offset: usize,
}

/// The sub-chunks of `data` (which starts at byte `base` of the file).
fn chunks(data: &[u8], base: usize) -> Vec<Chunk<'_>> {
    let mut result = Vec::new();
    let mut pos = 0;
    while pos + 8 <= data.len() {
        let id = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
        let size = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]) as usize;
        let start = pos + 8;
        // Tolerate a truncated last chunk.
        let end = start.saturating_add(size).min(data.len());
        result.push(Chunk { id, data: &data[start..end], offset: base + start });
        pos = start.saturating_add(size).saturating_add(size & 1);
    }
    result
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).trim_end().to_string()
}

fn parse(file: &[u8]) -> Result<Parsed> {
    if file.len() < 12 || &file[0..4] != b"RIFF" || &file[8..12] != b"sfbk" {
        return Err(invalid("not a RIFF sfbk file"));
    }
    let riff_size = le32(file, 4) as usize;
    let body_end = 8usize.saturating_add(riff_size).min(file.len());

    let mut info = SoundFontInfo::default();
    let mut smpl: Option<(usize, usize)> = None;
    let mut sm24: Option<(usize, usize)> = None;
    let mut pdta: Option<&[u8]> = None;

    for list in chunks(&file[12..body_end], 12) {
        if &list.id != b"LIST" || list.data.len() < 4 {
            continue;
        }
        let kind = &list.data[0..4];
        let subchunks = chunks(&list.data[4..], list.offset + 4);
        match kind {
            b"INFO" => {
                for c in subchunks {
                    match &c.id {
                        b"ifil" if c.data.len() >= 4 => info.version = (le16(c.data, 0), le16(c.data, 2)),
                        b"INAM" => info.name = text(c.data),
                        b"ICRD" => info.date = text(c.data),
                        b"IENG" => info.engineers = text(c.data),
                        b"IPRD" => info.product = text(c.data),
                        b"ICOP" => info.copyright = text(c.data),
                        b"ICMT" => info.comment = text(c.data),
                        b"ISFT" => info.software = text(c.data),
                        _ => {}
                    }
                }
            }
            b"sdta" => {
                for c in subchunks {
                    match &c.id {
                        b"smpl" => smpl = Some((c.offset, c.data.len() / 2)),
                        b"sm24" => sm24 = Some((c.offset, c.data.len())),
                        _ => {}
                    }
                }
            }
            b"pdta" => pdta = Some(&list.data[4..]),
            _ => {}
        }
    }

    let smpl = smpl.ok_or_else(|| invalid("no sample data (smpl)"))?;
    let pdta = pdta.ok_or_else(|| invalid("no preset data (pdta)"))?;
    // sm24 is only valid in 2.04 files and must cover every sample point.
    let sm24 = sm24.filter(|&(_, len)| info.version >= (2, 4) && len >= smpl.1).map(|(offset, _)| offset);

    let mut hydra: HashMap<[u8; 4], &[u8]> = HashMap::new();
    for c in chunks(pdta, 0) {
        hydra.insert(c.id, c.data);
    }
    let get = |id: &[u8; 4], record: usize| -> Result<&[u8]> {
        let data =
            hydra.get(id).copied().ok_or_else(|| invalid(format!("missing {} chunk", String::from_utf8_lossy(id))))?;
        let usable = data.len() / record * record;
        Ok(&data[..usable])
    };

    let phdr = get(b"phdr", 38)?;
    let pbag = get(b"pbag", 4)?;
    let pmod = get(b"pmod", 10)?;
    let pgen = get(b"pgen", 4)?;
    let inst = get(b"inst", 22)?;
    let ibag = get(b"ibag", 4)?;
    let imod = get(b"imod", 10)?;
    let igen = get(b"igen", 4)?;
    let shdr = get(b"shdr", 46)?;

    let samples: Vec<SampleHeader> = (0..(shdr.len() / 46).saturating_sub(1))
        .map(|i| {
            let r = &shdr[i * 46..(i + 1) * 46];
            SampleHeader {
                name: text(&r[0..20]),
                start: le32(r, 20),
                end: le32(r, 24),
                loop_start: le32(r, 28),
                loop_end: le32(r, 32),
                sample_rate: le32(r, 36),
                original_pitch: r[40],
                pitch_correction: r[41] as i8,
                link: le16(r, 42),
                sample_type: le16(r, 44),
            }
        })
        .collect();

    let bags = |bag: &[u8]| -> Vec<(usize, usize)> {
        (0..bag.len() / 4).map(|i| (le16(bag, i * 4) as usize, le16(bag, i * 4 + 2) as usize)).collect()
    };
    let pbags = bags(pbag);
    let ibags = bags(ibag);

    let instrument_records: Vec<(String, usize)> =
        (0..inst.len() / 22).map(|i| (text(&inst[i * 22..i * 22 + 20]), le16(inst, i * 22 + 20) as usize)).collect();
    let mut instruments = Vec::new();
    for i in 0..instrument_records.len().saturating_sub(1) {
        let (name, first) = &instrument_records[i];
        let last = instrument_records[i + 1].1;
        let zones = read_zones(*first, last, &ibags, igen, imod, g::SAMPLE_ID, samples.len());
        instruments.push(Instrument { name: name.clone(), zones });
    }

    let mut presets = Vec::new();
    let preset_count = (phdr.len() / 38).saturating_sub(1);
    for i in 0..preset_count {
        let r = &phdr[i * 38..(i + 1) * 38];
        let first = le16(r, 24) as usize;
        let last = le16(&phdr[(i + 1) * 38..], 24) as usize;
        let zones = read_zones(first, last, &pbags, pgen, pmod, g::INSTRUMENT, instruments.len());
        presets.push(Preset { name: text(&r[0..20]), program: le16(r, 20), bank: le16(r, 22), zones });
    }

    Ok(Parsed { info, presets, instruments, samples, smpl, sm24 })
}

/// A global zone: key range, velocity range, generators, modulators.
type GlobalZone = (Option<(u8, u8)>, Option<(u8, u8)>, GeneratorSet, Vec<Modulator>);

/// The zones of bags `first..last`, with the global zone merged into each.
/// Zones without a valid terminal generator (`terminal` < `link_count`) are
/// dropped, except a first zone, which is the global zone.
fn read_zones(
    first: usize,
    last: usize,
    bags: &[(usize, usize)],
    generators: &[u8],
    modulators: &[u8],
    terminal: u16,
    link_count: usize,
) -> Vec<Zone> {
    let gen_count = generators.len() / 4;
    let mod_count = modulators.len() / 10;
    let last = last.min(bags.len().saturating_sub(1));

    let mut global: Option<GlobalZone> = None;
    let mut zones = Vec::new();

    for bag in first..last {
        let (gen_start, mod_start) = bags[bag];
        let (gen_end, mod_end) = bags[bag + 1];
        let mut set = GeneratorSet::default();
        let mut key_range = None;
        let mut vel_range = None;
        let mut link = None;
        for i in gen_start..gen_end.min(gen_count) {
            let operator = le16(generators, i * 4);
            let amount = [generators[i * 4 + 2], generators[i * 4 + 3]];
            match operator {
                g::KEY_RANGE => key_range = Some((amount[0], amount[1])),
                g::VEL_RANGE => vel_range = Some((amount[0], amount[1])),
                op if op == terminal => {
                    link = Some(u16::from_le_bytes(amount) as usize);
                    // Generators after the terminal one are ignored.
                    break;
                }
                op => set.set(op, i16::from_le_bytes(amount)),
            }
        }
        let mods: Vec<Modulator> = (mod_start..mod_end.min(mod_count))
            .map(|m| {
                let r = &modulators[m * 10..m * 10 + 10];
                Modulator {
                    source: le16(r, 0),
                    destination: le16(r, 2),
                    amount: i16::from_le_bytes([r[4], r[5]]),
                    amount_source: le16(r, 6),
                    transform: le16(r, 8),
                }
            })
            .collect();

        match link {
            None => {
                if bag == first {
                    global = Some((key_range, vel_range, set, mods));
                }
            }
            Some(link) if link < link_count => {
                let (key, vel, generators, modulators) = match &global {
                    Some((g_key, g_vel, g_set, g_mods)) => {
                        let mut merged = g_mods.clone();
                        for m in mods {
                            match merged.iter_mut().find(|e| e.same_kind(&m)) {
                                Some(existing) => *existing = m,
                                None => merged.push(m),
                            }
                        }
                        (key_range.or(*g_key), vel_range.or(*g_vel), g_set.overridden_by(&set), merged)
                    }
                    None => (key_range, vel_range, set, mods),
                };
                let (key_lo, key_hi) = key.unwrap_or((0, 127));
                let (vel_lo, vel_hi) = vel.unwrap_or((0, 127));
                zones.push(Zone { key_lo, key_hi, vel_lo, vel_hi, generators, modulators, link, sfz: None });
            }
            Some(_) => {}
        }
    }
    zones
}
