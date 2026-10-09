//! Loading SFZ instruments and the WAV or FLAC samples they play. An SFZ
//! file becomes one preset whose zones are its regions, the opcodes
//! translated to SF2 generators and modulators; what SF2 can't say
//! (release triggers, round robin, crossfades, mute groups, CC-triggered
//! regions…) travels with each zone as an [`SfzRegion`].

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::generator::{self as g, GeneratorSet};
use crate::modulator::{Modulator, controller, source};
use crate::soundfont::{FileFormat, Instrument, Preset, SampleHeader, SoundFontInfo, Zone};

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidSfz(message.into())
}

// MARK: - Parsing

/// What starts a section of opcodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Header {
    Control,
    Global,
    Master,
    Group,
    Region,
    /// `<curve>`, `<effect>`, `<midi>`, `<sample>`…: read past.
    Other,
}

enum Token {
    Header(Header),
    Opcode(String, String),
}

/// Reads a file an SFZ file refers to, by its path relative to the SFZ
/// file's directory (`/`-separated).
pub(crate) type Loader<'a> = &'a (dyn Fn(&str) -> std::io::Result<Vec<u8>> + Sync);

/// The text without comments, `#include`s spliced in and `#define`d
/// variables replaced, as tokens.
fn tokenize(text: &str, load: Loader<'_>, defines: &mut Vec<(String, String)>, depth: usize) -> Result<Vec<Token>> {
    if depth > 16 {
        return Err(invalid("#include nested too deeply"));
    }
    let text = strip_comments(text);
    let mut tokens = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("#define") {
            let mut parts = rest.split_whitespace();
            if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
                defines.retain(|(n, _)| n != name);
                defines.push((name.to_string(), value.to_string()));
                // Longest names first, so `$AB` isn't read as `$A` + `B`.
                defines.sort_by_key(|d| std::cmp::Reverse(d.0.len()));
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#include") {
            let path = rest.trim().trim_matches('"').replace('\\', "/");
            let bytes = load(&path).map_err(|e| invalid(format!("#include \"{path}\": {e}")))?;
            let included = String::from_utf8_lossy(&bytes).into_owned();
            tokens.extend(tokenize(&included, load, defines, depth + 1)?);
            continue;
        }
        let mut line = line.to_string();
        if line.contains('$') {
            for (name, value) in defines.iter() {
                line = line.replace(name.as_str(), value);
            }
        }
        tokenize_line(&line, &mut tokens);
    }
    Ok(tokens)
}

/// Removes `//` line comments and `/* */` block comments.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        match (rest.find("//"), rest.find("/*")) {
            (Some(line), block) if block.is_none_or(|b| line < b) => {
                out.push_str(&rest[..line]);
                rest = &rest[line..];
                rest = rest.find('\n').map_or("", |end| &rest[end..]);
            }
            (_, Some(block)) => {
                out.push_str(&rest[..block]);
                rest = rest[block + 2..].find("*/").map_or("", |end| &rest[block + 2 + end + 2..]);
                out.push(' ');
            }
            _ => {
                out.push_str(rest);
                rest = "";
            }
        }
    }
    out
}

/// The start of each opcode (`name=`) in `line`: (name start, `=` index).
fn opcode_starts(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut starts = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let at_word_start = i == 0 || bytes[i - 1].is_ascii_whitespace() || bytes[i - 1] == b'>';
        if at_word_start && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'=' {
                starts.push((i, j));
                i = j + 1;
                continue;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    starts
}

fn tokenize_line(line: &str, tokens: &mut Vec<Token>) {
    // Headers split the line; each piece holds opcodes.
    let mut rest = line;
    loop {
        let (before, after) = match rest.find('<') {
            Some(open) => (&rest[..open], Some(&rest[open..])),
            None => (rest, None),
        };
        tokenize_opcodes(before, tokens);
        let Some(after) = after else { break };
        let Some(close) = after.find('>') else { break };
        let name = after[1..close].trim().to_ascii_lowercase();
        tokens.push(Token::Header(match name.as_str() {
            "control" => Header::Control,
            "global" => Header::Global,
            "master" => Header::Master,
            "group" => Header::Group,
            "region" => Header::Region,
            _ => Header::Other,
        }));
        rest = &after[close + 1..];
    }
}

fn tokenize_opcodes(text: &str, tokens: &mut Vec<Token>) {
    let starts = opcode_starts(text);
    for (k, &(name_start, equals)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(text.len(), |next| next.0);
        let name = text[name_start..equals].to_ascii_lowercase();
        let value = text[equals + 1..end].trim().to_string();
        tokens.push(Token::Opcode(name, value));
    }
}

/// A key number from a number or a note name (`c4` = 60, `f#2`, `eb-1`).
fn parse_key(value: &str, note_offset: i32) -> Option<u8> {
    let key: i64 = if let Ok(number) = value.parse::<i64>() {
        number
    } else {
        let lower = value.to_ascii_lowercase();
        let mut chars = lower.chars().peekable();
        let base = match chars.next()? {
            'c' => 0,
            'd' => 2,
            'e' => 4,
            'f' => 5,
            'g' => 7,
            'a' => 9,
            'b' => 11,
            _ => return None,
        };
        let accidental = match chars.peek() {
            Some('#') => {
                chars.next();
                1
            }
            Some('b') => {
                chars.next();
                -1
            }
            _ => 0,
        };
        let octave: i64 = chars.collect::<String>().parse().ok()?;
        octave.checked_add(1)?.checked_mul(12)? + base + accidental
    };
    let key = key.checked_add(note_offset as i64)?;
    (0..=127).contains(&key).then_some(key as u8)
}

/// The regions of an SFZ text: each its own opcodes merged over those of
/// its group, master, global and control sections.
fn regions(tokens: Vec<Token>) -> Vec<HashMap<String, String>> {
    let mut levels: [HashMap<String, String>; 4] = Default::default(); // control, global, master, group
    let mut current: Option<(Header, HashMap<String, String>)> = None;
    let mut out = Vec::new();

    let finish = |current: &mut Option<(Header, HashMap<String, String>)>,
                  levels: &mut [HashMap<String, String>; 4],
                  out: &mut Vec<HashMap<String, String>>| {
        let Some((header, opcodes)) = current.take() else { return };
        match header {
            Header::Control => levels[0] = opcodes,
            Header::Global => {
                levels[1] = opcodes;
                levels[2].clear();
                levels[3].clear();
            }
            Header::Master => {
                levels[2] = opcodes;
                levels[3].clear();
            }
            Header::Group => levels[3] = opcodes,
            Header::Region => {
                let mut merged: HashMap<String, String> = HashMap::new();
                for level in levels.iter().chain(std::iter::once(&opcodes)) {
                    // `key` sets the range and pitch centre, replacing those
                    // a broader section gave.
                    if level.contains_key("key") {
                        for name in ["lokey", "hikey", "pitch_keycenter"] {
                            merged.remove(name);
                        }
                    }
                    merged.extend(level.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                out.push(merged);
            }
            Header::Other => {}
        }
    };

    for token in tokens {
        match token {
            Token::Header(header) => {
                finish(&mut current, &mut levels, &mut out);
                current = Some((header, HashMap::new()));
            }
            Token::Opcode(name, value) => match &mut current {
                Some((_, opcodes)) => {
                    opcodes.insert(name, value);
                }
                // Opcodes before any header act as global ones.
                None => {
                    levels[1].insert(name, value);
                }
            },
        }
    }
    finish(&mut current, &mut levels, &mut out);
    out
}

// MARK: - Regions

/// When a region sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// On note-on (the default).
    Attack,
    /// On note-off, or when the sustain pedal lets the note go.
    Release,
    /// On note-off, whatever the pedal.
    ReleaseKey,
    /// On note-on when no other key is down on the channel.
    First,
    /// On note-on while another key is down.
    Legato,
}

/// A crossfade in or out over a range of velocities or keys.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fade {
    pub low: u8,
    pub high: u8,
}

/// What an SFZ region does beyond its SF2 generators and modulators.
#[derive(Clone, Debug)]
pub(crate) struct SfzRegion {
    pub trigger: Trigger,
    /// Round robin: (length, position 0-based, counter index).
    pub sequence: Option<(u32, u32, usize)>,
    /// Plays when the note's random number (0…1) is in `low..high`.
    pub random: (f32, f32),
    /// Plays when each of these controllers (number, low, high) is in range.
    pub cc_conditions: Vec<(u8, u8, u8)>,
    /// Plays when this controller moves into the range (number, low, high),
    /// instead of on a note.
    pub cc_trigger: Option<(u8, u8, u8)>,
    /// MIDI channels it plays on, 0-based.
    pub channels: (u8, u8),
    /// Mute group (0 = none) and the group whose start stops it.
    pub group: i64,
    pub off_by: Option<i64>,
    /// Stopped by its off group at once (true) or released.
    pub off_fast: bool,
    /// Plays to its end, whatever note-offs come.
    pub one_shot: bool,
    /// Linear gain: volume and amplitude.
    pub gain: f32,
    /// Gain by velocity, when not the SF2 default (concave) response.
    pub velocity_gain: Option<Box<[f32; 128]>>,
    /// dB per key from `amp_keycenter`.
    pub amp_keytrack: f32,
    pub amp_keycenter: u8,
    pub velocity_fade_in: Option<Fade>,
    pub velocity_fade_out: Option<Fade>,
    pub key_fade_in: Option<Fade>,
    pub key_fade_out: Option<Fade>,
    /// Equal-power (true) or linear crossfades.
    pub power_fades: (bool, bool),
    /// A release region's loss, dB per second the key was held.
    pub rt_decay: f32,
}

impl SfzRegion {
    /// The gain a note of `key` and `velocity` starts with.
    pub fn note_gain(&self, key: u8, velocity: u8) -> f32 {
        let mut gain = self.gain;
        if let Some(table) = &self.velocity_gain {
            gain *= table[velocity as usize & 127];
        }
        if self.amp_keytrack != 0.0 {
            gain *= 10f32.powf(self.amp_keytrack * (key as f32 - self.amp_keycenter as f32) / 20.0);
        }
        let fade = |value: u8, fade: Option<Fade>, rising: bool, power: bool| -> f32 {
            let Some(Fade { low, high }) = fade else { return 1.0 };
            let t = if value <= low {
                0.0
            } else if value >= high {
                1.0
            } else {
                (value - low) as f32 / (high - low) as f32
            };
            let t = if rising { t } else { 1.0 - t };
            if power { t.sqrt() } else { t }
        };
        let (velocity_power, key_power) = self.power_fades;
        gain * fade(velocity, self.velocity_fade_in, true, velocity_power)
            * fade(velocity, self.velocity_fade_out, false, velocity_power)
            * fade(key, self.key_fade_in, true, key_power)
            * fade(key, self.key_fade_out, false, key_power)
    }
}

/// What makes regions share a round robin counter: their keys and
/// velocities, trigger and sequence length.
type SequenceId = (u32, u32, Vec<(String, String)>);

/// A region's opcodes, read.
struct Spec {
    sample: String,
    key_range: (u8, u8),
    velocity_range: (u8, u8),
    /// None: the sample's own root key (or 60).
    keycenter: Option<u8>,
    generators: GeneratorSet,
    modulators: Vec<Modulator>,
    /// Loop: mode (SF2 sampleModes) and points (end exclusive), if given.
    loop_mode: Option<u16>,
    loop_points: (Option<u32>, Option<u32>),
    offset: u32,
    end: Option<u32>,
    pan: f32,
    width: f32,
    extra: SfzRegion,
}

/// Opcodes this crate reads; anything else is reported as unsupported.
fn known(name: &str) -> bool {
    const NAMES: &[&str] = &[
        "sample",
        "default_path",
        "note_offset",
        "octave_offset",
        "key",
        "lokey",
        "hikey",
        "lovel",
        "hivel",
        "pitch_keycenter",
        "tune",
        "pitch",
        "transpose",
        "pitch_keytrack",
        "volume",
        "gain",
        "amplitude",
        "pan",
        "width",
        "amp_veltrack",
        "amp_keytrack",
        "amp_keycenter",
        "ampeg_delay",
        "ampeg_attack",
        "ampeg_hold",
        "ampeg_decay",
        "ampeg_sustain",
        "ampeg_release",
        "fil_type",
        "filtype",
        "cutoff",
        "resonance",
        "fil_veltrack",
        "fil_keytrack",
        "fil_keycenter",
        "loop_mode",
        "loopmode",
        "loop_start",
        "loopstart",
        "loop_end",
        "loopend",
        "offset",
        "end",
        "trigger",
        "seq_length",
        "seq_position",
        "lorand",
        "hirand",
        "group",
        "off_by",
        "off_mode",
        "xfin_lovel",
        "xfin_hivel",
        "xfout_lovel",
        "xfout_hivel",
        "xfin_lokey",
        "xfin_hikey",
        "xfout_lokey",
        "xfout_hikey",
        "xf_velcurve",
        "xf_keycurve",
        "rt_decay",
        "lochan",
        "hichan",
        "effect1",
        "effect2",
        "delay",
        "pitchlfo_freq",
        "pitchlfo_depth",
        "pitchlfo_delay",
        "amplfo_freq",
        "amplfo_depth",
        "amplfo_delay",
        "fillfo_freq",
        "fillfo_depth",
        "fillfo_delay",
        "fileg_depth",
        "fileg_delay",
        "fileg_attack",
        "fileg_hold",
        "fileg_decay",
        "fileg_sustain",
        "fileg_release",
        "pitcheg_depth",
        "pitcheg_delay",
        "pitcheg_attack",
        "pitcheg_hold",
        "pitcheg_decay",
        "pitcheg_sustain",
        "pitcheg_release",
        // Read past without effect: their defaults are what this synthesizer does.
        "polyphony",
        "note_polyphony",
        "bend_up",
        "bend_down",
        "sw_default",
    ];
    NAMES.contains(&name)
        || ["amp_velcurve_", "locc", "hicc", "on_locc", "on_hicc"]
            .iter()
            .any(|p| name.strip_prefix(p).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
}

fn timecents(seconds: f32) -> i16 {
    if seconds <= 0.0 { -12000 } else { (1200.0 * seconds.log2()).round().clamp(-12000.0, 8000.0) as i16 }
}

/// Absolute cents of a frequency.
fn absolute_cents(hertz: f32) -> f32 {
    1200.0 * (hertz.max(0.001) / 8.175_799).log2()
}

/// Attenuation (centibels) of a level in percent.
fn percent_to_centibels(percent: f32) -> i16 {
    if percent <= 0.0 { 1440 } else { (-200.0 * (percent / 100.0).log10()).round().clamp(0.0, 1440.0) as i16 }
}

/// Reads one region's opcodes; `unsupported` collects the names it ignores.
fn read_spec(
    opcodes: &HashMap<String, String>,
    counters: &mut HashMap<SequenceId, usize>,
    unsupported: &mut BTreeSet<String>,
) -> Option<Spec> {
    let get = |name: &str| opcodes.get(name).map(|s| s.as_str());
    let number = |name: &str| get(name).and_then(|v| v.parse::<f32>().ok());
    let note_offset = get("note_offset").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).clamp(-127, 127)
        + 12 * get("octave_offset").and_then(|v| v.parse::<i32>().ok()).unwrap_or(0).clamp(-10, 10);
    let key = |name: &str| get(name).and_then(|v| parse_key(v, note_offset));
    let byte = |name: &str, default: u8| number(name).map_or(default, |v| v.clamp(0.0, 127.0) as u8);

    for name in opcodes.keys() {
        if !known(name) {
            unsupported.insert(name.clone());
        }
    }

    let sample = get("sample")?.replace('\\', "/");
    let path = format!("{}{}", get("default_path").unwrap_or("").replace('\\', "/"), sample);

    let mut key_range = (0u8, 127u8);
    let mut keycenter = None;
    if let Some(k) = key("key") {
        key_range = (k, k);
        keycenter = Some(k);
    }
    if let Some(k) = key("lokey") {
        key_range.0 = k;
    }
    if let Some(k) = key("hikey") {
        key_range.1 = k;
    }
    match get("pitch_keycenter") {
        Some("sample") => keycenter = None,
        Some(_) => keycenter = key("pitch_keycenter").or(keycenter),
        None if keycenter.is_none() => keycenter = Some(60),
        None => {}
    }
    let velocity_range = (byte("lovel", 0), byte("hivel", 127));
    if key_range.0 > key_range.1 || velocity_range.0 > velocity_range.1 {
        return None;
    }
    // `hikey=-1` (or `key=-1`): no key plays it; only a controller can.
    let keyless = [get("hikey"), get("key")].iter().any(|v| v.and_then(|v| v.parse::<i32>().ok()) == Some(-1));
    if keyless && !opcodes.keys().any(|name| name.starts_with("on_locc") || name.starts_with("on_hicc")) {
        return None;
    }

    let mut generators = GeneratorSet::default();
    let mut modulators = Vec::new();

    // Pitch.
    let cents = number("tune").or(number("pitch")).unwrap_or(0.0) + 100.0 * number("transpose").unwrap_or(0.0);
    let cents = cents.round() as i32;
    let coarse = cents / 100;
    if coarse != 0 {
        generators.set(g::COARSE_TUNE, coarse.clamp(-120, 120) as i16);
    }
    if cents % 100 != 0 {
        generators.set(g::FINE_TUNE, (cents % 100) as i16);
    }
    if let Some(track) = number("pitch_keytrack") {
        generators.set(g::SCALE_TUNING, track.round().clamp(0.0, 1200.0) as i16);
    }

    // Amplifier envelope.
    let envelope = |generators: &mut GeneratorSet, opcode: &str, generator: u16| {
        if let Some(seconds) = number(opcode) {
            generators.set(generator, timecents(seconds));
        }
    };
    let delay = number("ampeg_delay").unwrap_or(0.0) + number("delay").unwrap_or(0.0);
    if delay > 0.0 {
        generators.set(g::DELAY_VOL_ENV, timecents(delay));
    }
    envelope(&mut generators, "ampeg_attack", g::ATTACK_VOL_ENV);
    envelope(&mut generators, "ampeg_hold", g::HOLD_VOL_ENV);
    envelope(&mut generators, "ampeg_decay", g::DECAY_VOL_ENV);
    envelope(&mut generators, "ampeg_release", g::RELEASE_VOL_ENV);
    if let Some(percent) = number("ampeg_sustain") {
        generators.set(g::SUSTAIN_VOL_ENV, percent_to_centibels(percent));
    }

    // Filter. Velocity tracking replaces the SF2 default (-2400 cents at
    // velocity 0); the cutoff is raised by it so that the default
    // modulator's form (velocity, negative, linear) gives the SFZ cutoff at
    // velocity 0 and cutoff + fil_veltrack at 127.
    let lowpass = match get("fil_type").or(get("filtype")) {
        None => true,
        Some(t) if t.starts_with("lpf") => true,
        Some(t) => {
            unsupported.insert(format!("fil_type={t}"));
            false
        }
    };
    let mut velocity_filter = 0i16;
    if let (Some(cutoff), true) = (number("cutoff"), lowpass) {
        let veltrack = number("fil_veltrack").unwrap_or(0.0).round().clamp(-9600.0, 9600.0);
        let mut base = absolute_cents(cutoff) + veltrack;
        if let Some(track) = number("fil_keytrack").filter(|t| *t != 0.0) {
            let center = key("fil_keycenter").unwrap_or(60) as f32;
            base -= track * center;
            modulators.push(Modulator {
                source: controller::NOTE_ON_KEY,
                destination: g::INITIAL_FILTER_FC,
                amount: (track * 127.0).round().clamp(-32768.0, 32767.0) as i16,
                amount_source: 0,
                transform: 0,
            });
        }
        generators.set(g::INITIAL_FILTER_FC, base.round().clamp(-32768.0, 32767.0) as i16);
        if let Some(db) = number("resonance") {
            generators.set(g::INITIAL_FILTER_Q, (db * 10.0).round().clamp(0.0, 960.0) as i16);
        }
        velocity_filter = -veltrack as i16;
    }
    modulators.push(Modulator {
        source: controller::NOTE_ON_VELOCITY | source::NEGATIVE,
        destination: g::INITIAL_FILTER_FC,
        amount: velocity_filter,
        amount_source: 0,
        transform: 0,
    });

    // Velocity → level: amp_veltrack 100 without a curve is the SF2 default
    // (the square law); anything else replaces it with a table.
    let veltrack = number("amp_veltrack").unwrap_or(100.0) / 100.0;
    let mut curve_points: Vec<(u8, f32)> = opcodes
        .iter()
        .filter_map(|(name, value)| {
            let index: u8 = name.strip_prefix("amp_velcurve_")?.parse().ok()?;
            Some((index.min(127), value.parse::<f32>().ok()?.clamp(0.0, 1.0)))
        })
        .collect();
    let velocity_gain = if veltrack == 1.0 && curve_points.is_empty() {
        None
    } else {
        modulators.push(Modulator {
            source: controller::NOTE_ON_VELOCITY | source::NEGATIVE | source::CONCAVE,
            destination: g::INITIAL_ATTENUATION,
            amount: 0,
            amount_source: 0,
            transform: 0,
        });
        curve_points.sort_by_key(|p| p.0);
        let curve = |velocity: u8| -> f32 {
            if curve_points.is_empty() {
                let v = velocity as f32 / 127.0;
                return v * v;
            }
            // Linear between the points given, from (0, 0) to (127, 1).
            let mut points = curve_points.clone();
            if points.first().is_none_or(|p| p.0 > 0) {
                points.insert(0, (0, 0.0));
            }
            if points.last().is_none_or(|p| p.0 < 127) {
                points.push((127, 1.0));
            }
            let i = points.iter().position(|p| p.0 >= velocity).unwrap_or(points.len() - 1);
            if i == 0 || points[i].0 == velocity {
                return points[i].1;
            }
            let (a, b) = (points[i - 1], points[i]);
            a.1 + (b.1 - a.1) * (velocity - a.0) as f32 / (b.0 - a.0) as f32
        };
        let mut table = Box::new([0f32; 128]);
        for (velocity, gain) in table.iter_mut().enumerate() {
            let level = curve(velocity as u8);
            // As sfizz: a negative tracking turns the curve upside down.
            *gain = if veltrack >= 0.0 { 1.0 - veltrack * (1.0 - level) } else { -veltrack * (1.0 - level) };
        }
        Some(table)
    };

    // LFOs and the modulation envelope.
    if number("pitchlfo_depth").is_some_and(|d| d != 0.0) {
        generators.set(g::VIB_LFO_TO_PITCH, number("pitchlfo_depth").unwrap().round() as i16);
        generators.set(g::FREQ_VIB_LFO, absolute_cents(number("pitchlfo_freq").unwrap_or(5.0)).round() as i16);
        envelope(&mut generators, "pitchlfo_delay", g::DELAY_VIB_LFO);
    }
    let amp_lfo = number("amplfo_depth").filter(|d| *d != 0.0);
    let filter_lfo = number("fillfo_depth").filter(|d| *d != 0.0);
    if amp_lfo.is_some() || filter_lfo.is_some() {
        let prefix = if amp_lfo.is_some() { "amplfo" } else { "fillfo" };
        let freq = number(&format!("{prefix}_freq")).unwrap_or(5.0);
        generators.set(g::FREQ_MOD_LFO, absolute_cents(freq).round() as i16);
        envelope(&mut generators, &format!("{prefix}_delay"), g::DELAY_MOD_LFO);
        if let Some(db) = amp_lfo {
            generators.set(g::MOD_LFO_TO_VOLUME, (-db * 10.0).round() as i16);
        }
        if let Some(cents) = filter_lfo {
            generators.set(g::MOD_LFO_TO_FILTER_FC, cents.round() as i16);
        }
    }
    let filter_eg = number("fileg_depth").filter(|d| *d != 0.0);
    let pitch_eg = number("pitcheg_depth").filter(|d| *d != 0.0);
    if filter_eg.is_some() || pitch_eg.is_some() {
        let prefix = if filter_eg.is_some() { "fileg" } else { "pitcheg" };
        envelope(&mut generators, &format!("{prefix}_delay"), g::DELAY_MOD_ENV);
        envelope(&mut generators, &format!("{prefix}_attack"), g::ATTACK_MOD_ENV);
        envelope(&mut generators, &format!("{prefix}_hold"), g::HOLD_MOD_ENV);
        envelope(&mut generators, &format!("{prefix}_decay"), g::DECAY_MOD_ENV);
        envelope(&mut generators, &format!("{prefix}_release"), g::RELEASE_MOD_ENV);
        if let Some(percent) = number(&format!("{prefix}_sustain")) {
            generators.set(g::SUSTAIN_MOD_ENV, ((100.0 - percent.clamp(0.0, 100.0)) * 10.0).round() as i16);
        }
        if let Some(cents) = filter_eg {
            generators.set(g::MOD_ENV_TO_FILTER_FC, cents.round() as i16);
        }
        if let Some(cents) = pitch_eg {
            generators.set(g::MOD_ENV_TO_PITCH, cents.round() as i16);
        }
    }

    // Effects.
    if let Some(percent) = number("effect1") {
        generators.set(g::REVERB_EFFECTS_SEND, (percent * 10.0).round().clamp(0.0, 1000.0) as i16);
    }
    if let Some(percent) = number("effect2") {
        generators.set(g::CHORUS_EFFECTS_SEND, (percent * 10.0).round().clamp(0.0, 1000.0) as i16);
    }

    let loop_mode = get("loop_mode").or(get("loopmode")).map(|mode| match mode {
        "loop_continuous" => 1,
        "loop_sustain" => 3,
        _ => 0,
    });
    let one_shot = get("loop_mode").or(get("loopmode")) == Some("one_shot");
    let loop_points = (
        number("loop_start").or(number("loopstart")).map(|v| v.max(0.0) as u32),
        number("loop_end").or(number("loopend")).map(|v| (v.max(0.0) as u32).saturating_add(1)),
    );

    let trigger = match get("trigger") {
        Some("release") => Trigger::Release,
        Some("release_key") => Trigger::ReleaseKey,
        Some("first") => Trigger::First,
        Some("legato") => Trigger::Legato,
        _ => Trigger::Attack,
    };
    let sequence = match number("seq_length").map(|l| l.max(1.0) as u32) {
        Some(length) if length > 1 => {
            let position = number("seq_position").map_or(1, |p| p.max(1.0) as u32) - 1;
            // Regions of one round robin share a counter: same keys,
            // velocities and length.
            let id = (
                (key_range.0 as u32) << 8 | key_range.1 as u32,
                (velocity_range.0 as u32) << 8 | velocity_range.1 as u32,
                vec![
                    ("trigger".to_string(), get("trigger").unwrap_or("").to_string()),
                    ("seq_length".into(), length.to_string()),
                ],
            );
            let next = counters.len();
            let counter = *counters.entry(id).or_insert(next);
            Some((length, position % length, counter))
        }
        _ => None,
    };
    let cc_range = |prefix_low: &str, prefix_high: &str| -> Vec<(u8, u8, u8)> {
        let mut found: HashMap<u8, (u8, u8)> = HashMap::new();
        for (name, value) in opcodes {
            let Ok(value) = value.parse::<f32>() else { continue };
            let value = value.clamp(0.0, 127.0) as u8;
            if let Some(cc) = name.strip_prefix(prefix_low).and_then(|n| n.parse::<u8>().ok()) {
                found.entry(cc.min(127)).or_insert((0, 127)).0 = value;
            } else if let Some(cc) = name.strip_prefix(prefix_high).and_then(|n| n.parse::<u8>().ok()) {
                found.entry(cc.min(127)).or_insert((0, 127)).1 = value;
            }
        }
        let mut list: Vec<(u8, u8, u8)> = found.into_iter().map(|(cc, (lo, hi))| (cc, lo, hi)).collect();
        list.sort();
        list
    };
    // `on_locc` starts with `locc`'s letters after "on_", so read it first.
    let cc_trigger = cc_range("on_locc", "on_hicc").into_iter().next();
    let cc_conditions = cc_range("locc", "hicc");
    let fade = |low: &str, high: &str, default: (u8, u8)| -> Option<Fade> {
        let range = (byte(low, default.0), byte(high, default.1));
        (range != default).then_some(Fade { low: range.0, high: range.1.max(range.0) })
    };
    let gain = 10f32.powf((number("volume").unwrap_or(0.0) + number("gain").unwrap_or(0.0)) / 20.0)
        * number("amplitude").unwrap_or(100.0)
        / 100.0;
    let extra = SfzRegion {
        trigger,
        sequence,
        random: (number("lorand").unwrap_or(0.0), number("hirand").unwrap_or(1.0)),
        cc_conditions,
        cc_trigger,
        channels: (byte("lochan", 1).clamp(1, 16) - 1, byte("hichan", 16).clamp(1, 16) - 1),
        group: number("group").map_or(0, |v| v as i64),
        off_by: number("off_by").map(|v| v as i64).filter(|&v| v != 0),
        off_fast: get("off_mode") != Some("normal"),
        one_shot,
        gain,
        velocity_gain,
        amp_keytrack: number("amp_keytrack").unwrap_or(0.0),
        amp_keycenter: key("amp_keycenter").unwrap_or(60),
        velocity_fade_in: fade("xfin_lovel", "xfin_hivel", (0, 0)),
        velocity_fade_out: fade("xfout_lovel", "xfout_hivel", (127, 127)),
        key_fade_in: fade("xfin_lokey", "xfin_hikey", (0, 0)),
        key_fade_out: fade("xfout_lokey", "xfout_hikey", (127, 127)),
        power_fades: (get("xf_velcurve") != Some("gain"), get("xf_keycurve") != Some("gain")),
        rt_decay: number("rt_decay").unwrap_or(0.0),
    };

    Some(Spec {
        sample: path,
        key_range,
        velocity_range,
        keycenter,
        generators,
        modulators,
        loop_mode,
        loop_points,
        offset: number("offset").map_or(0, |v| v.max(0.0) as u32),
        end: number("end").filter(|v| *v > 0.0).map(|v| (v as u32).saturating_add(1)),
        pan: number("pan").unwrap_or(0.0).clamp(-100.0, 100.0),
        width: number("width").unwrap_or(100.0).clamp(-100.0, 100.0),
        extra,
    })
}

// MARK: - Sample files

/// Zero points after each sample.
const PAD: usize = 8;
/// Bytes read to find a sample file's format.
const HEADER_BYTES: usize = 1 << 16;

/// What a sample file's start says.
#[derive(Clone, Copy)]
struct Format {
    frames: usize,
    channels: usize,
    bits: u32,
}

/// A decoded sample file's facts.
#[derive(Clone, Copy, Default)]
struct Facts {
    frames: usize,
    sample_rate: u32,
    root: Option<u8>,
    /// First loop: start, end (exclusive), in frames.
    loop_points: Option<(u32, u32)>,
}

/// Interleaved integer samples of a WAV file: (format, rate, samples, facts).
struct Wav<'a> {
    format_tag: u16,
    channels: usize,
    rate: u32,
    bits: u32,
    data: &'a [u8],
    /// Frames the data chunk declares (it may be cut short in `data`).
    frames: usize,
    root: Option<u8>,
    loop_points: Option<(u32, u32)>,
}

fn le16(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parses a WAV file's chunks (`bytes` may be only its start).
fn parse_wav(bytes: &[u8]) -> std::result::Result<Wav<'_>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let mut wav =
        Wav { format_tag: 0, channels: 0, rate: 0, bits: 0, data: &[], frames: 0, root: None, loop_points: None };
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = le32(bytes, pos + 4) as usize;
        let start = pos + 8;
        let end = start.saturating_add(size).min(bytes.len());
        let chunk = &bytes[start..end];
        match id {
            b"fmt " => {
                wav.format_tag = le16(chunk, 0);
                wav.channels = le16(chunk, 2) as usize;
                wav.rate = le32(chunk, 4);
                wav.bits = le16(chunk, 14) as u32;
                if wav.format_tag == 0xfffe {
                    wav.format_tag = le16(chunk, 24);
                }
            }
            b"data" => {
                wav.data = chunk;
                let width = (wav.bits as usize).div_ceil(8) * wav.channels.max(1);
                wav.frames = size.checked_div(width).unwrap_or(0);
            }
            b"smpl" if chunk.len() >= 36 => {
                let unity = le32(chunk, 12);
                if unity <= 127 {
                    wav.root = Some(unity as u8);
                }
                if le32(chunk, 28) > 0 && chunk.len() >= 36 + 24 {
                    let (start, end) = (le32(chunk, 36 + 8), le32(chunk, 36 + 12));
                    if end > start {
                        wav.loop_points = Some((start, end + 1));
                    }
                }
            }
            _ => {}
        }
        pos = start.saturating_add(size).saturating_add(size & 1);
    }
    if wav.channels == 0 || wav.rate == 0 {
        return Err("no fmt chunk".into());
    }
    if !matches!((wav.format_tag, wav.bits), (1, 8 | 16 | 24 | 32) | (3, 32 | 64)) {
        return Err(format!("unsupported WAV format {} ({} bits)", wav.format_tag, wav.bits));
    }
    Ok(wav)
}

fn is_flac(bytes: &[u8]) -> bool {
    bytes.starts_with(b"fLaC") || bytes.starts_with(b"ID3")
}

/// The longest sample file read: 2³⁰ frames (over six hours at 48 kHz), so
/// a damaged header can't ask for absurd memory.
const MAX_FRAMES: usize = 1 << 30;

/// A sample file's format from its first bytes.
fn read_format(bytes: &[u8]) -> std::result::Result<Format, String> {
    let format = read_header(bytes)?;
    if format.frames > MAX_FRAMES {
        return Err(format!("{} frames is too long", format.frames));
    }
    Ok(format)
}

fn read_header(bytes: &[u8]) -> std::result::Result<Format, String> {
    if is_flac(bytes) {
        let info = crate::flac::stream_info(bytes).map_err(|e| e.0)?;
        if info.total_samples == 0 {
            return Err("FLAC file of unknown length".into());
        }
        return Ok(Format { frames: info.total_samples as usize, channels: info.channels, bits: info.bits_per_sample });
    }
    let wav = parse_wav(bytes)?;
    let bits = if wav.format_tag == 3 { 24 } else { wav.bits };
    Ok(Format { frames: wav.frames, channels: wav.channels, bits })
}

/// Decodes a sample file into `smpl` (16-bit points) and `sm24` (their
/// low bytes, when the font is 24-bit): channel `c` from point
/// `c × (frames + PAD)`, of the first `channels` channels.
fn decode_into(
    bytes: &[u8],
    format: Format,
    smpl: &mut [u8],
    mut sm24: Option<&mut [u8]>,
) -> std::result::Result<Facts, String> {
    let stride = format.frames + PAD;
    let used = format.channels.min(2);
    let mut put = |channel: usize, frame: usize, value24: i32| {
        let at = channel * stride + frame;
        let high = (value24 >> 8) as i16;
        smpl[at * 2..at * 2 + 2].copy_from_slice(&high.to_le_bytes());
        if let Some(low) = sm24.as_deref_mut() {
            low[at] = value24 as u8;
        }
    };
    if is_flac(bytes) {
        let audio = crate::flac::decode(bytes).map_err(|e| e.0)?;
        let shift = 24 - audio.bits_per_sample as i32;
        let frames = audio.channels[0].len().min(format.frames);
        for (c, channel) in audio.channels.iter().take(used).enumerate() {
            for (i, &v) in channel[..frames].iter().enumerate() {
                put(c, i, if shift >= 0 { v << shift } else { v >> -shift });
            }
        }
        return Ok(Facts { frames, sample_rate: audio.sample_rate, root: None, loop_points: None });
    }
    let wav = parse_wav(bytes)?;
    let width = (wav.bits as usize).div_ceil(8);
    let frame_bytes = width * wav.channels;
    let frames = (wav.data.len() / frame_bytes).min(format.frames);
    for i in 0..frames {
        for c in 0..used {
            let s = &wav.data[i * frame_bytes + c * width..][..width];
            let value24 = match (wav.format_tag, wav.bits) {
                (1, 8) => (s[0] as i32 - 128) << 16,
                (1, 16) => (i16::from_le_bytes([s[0], s[1]]) as i32) << 8,
                (1, 24) => i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8,
                (1, _) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) >> 8,
                (_, 32) => float_to_24(f32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64),
                _ => float_to_24(f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])),
            };
            put(c, i, value24);
        }
    }
    Ok(Facts { frames, sample_rate: wav.rate, root: wav.root, loop_points: wav.loop_points })
}

fn float_to_24(x: f64) -> i32 {
    (x * 8_388_608.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32
}

// MARK: - Loading

/// An SFZ instrument as SF2 structures, its samples in one block of
/// 16-bit little-endian points (followed by their low bytes when 24-bit).
pub(crate) struct Converted {
    pub info: SoundFontInfo,
    pub presets: Vec<Preset>,
    pub instruments: Vec<Instrument>,
    pub samples: Vec<SampleHeader>,
    pub points: Vec<u8>,
    /// Number of sample points.
    pub len: usize,
    pub has_sm24: bool,
    /// Round robin counters the regions use.
    pub counters: usize,
    /// Opcodes ignored and sample files that couldn't be read.
    pub warnings: Vec<String>,
}

/// Reads the SFZ `text` and the samples it plays, with `load(path, limit)`
/// reading a file by its path relative to the SFZ file (all of it, or at
/// least its first `limit` bytes).
pub(crate) fn load(text: &str, name: &str, load: &crate::soundfont::FileLoader<'_>) -> Result<Converted> {
    let whole = |path: &str| load(path, None);
    let tokens = tokenize(text, &whole, &mut Vec::new(), 0)?;
    let mut unsupported = BTreeSet::new();
    let mut counters = HashMap::new();
    let specs: Vec<Spec> =
        regions(tokens).iter().filter_map(|opcodes| read_spec(opcodes, &mut counters, &mut unsupported)).collect();
    if specs.is_empty() {
        return Err(invalid("no regions with a sample"));
    }
    let mut warnings: Vec<String> = unsupported.into_iter().map(|name| format!("unsupported opcode: {name}")).collect();

    // The sample files, each read once; their formats from their starts.
    let mut paths: Vec<String> = specs.iter().map(|s| s.sample.clone()).collect();
    paths.sort();
    paths.dedup();
    let formats: Vec<std::result::Result<Format, String>> = parallel_map(&paths, |path| {
        let bytes = load(path, Some(HEADER_BYTES)).map_err(|e| e.to_string())?;
        read_format(&bytes)
    });

    // Where each file's channels go.
    let mut offsets = vec![0usize; paths.len()];
    let mut total = 0usize;
    let mut has_sm24 = false;
    for (i, format) in formats.iter().enumerate() {
        match format {
            Ok(format) => {
                offsets[i] = total;
                total += format.channels.min(2) * (format.frames + PAD);
                has_sm24 |= format.bits > 16;
            }
            Err(e) => warnings.push(format!("sample {}: {e}", paths[i])),
        }
    }
    if total > u32::MAX as usize {
        return Err(invalid("more than 4 G sample points"));
    }

    // Decode every file into its part of the block, in parallel.
    let mut points = vec![0u8; total * if has_sm24 { 3 } else { 2 }];
    let (smpl, sm24) = points.split_at_mut(total * 2);
    // (file, its 16-bit points, their low bytes)
    type Part<'a> = (usize, &'a mut [u8], Option<&'a mut [u8]>);
    let mut parts: Vec<Part<'_>> = Vec::new();
    {
        let mut smpl_rest = smpl;
        let mut sm24_rest = if has_sm24 { Some(sm24) } else { None };
        for (i, format) in formats.iter().enumerate() {
            let Ok(format) = format else { continue };
            let size = format.channels.min(2) * (format.frames + PAD);
            let (part, rest) = std::mem::take(&mut smpl_rest).split_at_mut(size * 2);
            smpl_rest = rest;
            let low = sm24_rest.take().map(|r| {
                let (part, rest) = r.split_at_mut(size);
                sm24_rest = Some(rest);
                part
            });
            parts.push((i, part, low));
        }
    }
    let queue = Mutex::new(parts);
    let facts: Mutex<Vec<Option<std::result::Result<Facts, String>>>> = Mutex::new(vec![None; paths.len()]);
    run_workers(|| {
        loop {
            let Some((i, part, low)) = queue.lock().unwrap().pop() else { break };
            let Ok(format) = formats[i] else { continue };
            let result = load(&paths[i], None)
                .map_err(|e| e.to_string())
                .and_then(|bytes| decode_into(&bytes, format, part, low));
            facts.lock().unwrap()[i] = Some(result);
        }
    });
    let facts = facts.into_inner().unwrap();
    for (i, fact) in facts.iter().enumerate() {
        if let Some(Err(e)) = fact {
            warnings.push(format!("sample {}: {e}", paths[i]));
        }
    }
    let file_index: HashMap<&str, usize> = paths.iter().enumerate().map(|(i, p)| (p.as_str(), i)).collect();

    // Sample headers (shared by regions playing a file alike) and zones.
    let mut samples: Vec<SampleHeader> = Vec::new();
    // By file, channel, root key and loop.
    type HeaderId = (usize, usize, u8, Option<(u32, u32)>);
    let mut made: HashMap<HeaderId, usize> = HashMap::new();
    let mut zones = Vec::new();
    for spec in specs {
        let i = file_index[spec.sample.as_str()];
        let (Ok(format), Some(Ok(fact))) = (&formats[i], &facts[i]) else { continue };
        let channels = format.channels.min(2);
        let frames = fact.frames;
        let root = spec.keycenter.or(fact.root).unwrap_or(60);
        let loop_points = match spec.loop_points {
            (None, None) => fact.loop_points,
            (start, end) => {
                let start = start.or(fact.loop_points.map(|l| l.0)).unwrap_or(0);
                let end = end.or(fact.loop_points.map(|l| l.1)).unwrap_or(frames as u32);
                Some((start, end))
            }
        }
        .map(|(s, e)| (s.min(frames as u32), e.min(frames as u32)))
        .filter(|(s, e)| e > s);
        let loop_mode = spec.loop_mode.unwrap_or(if fact.loop_points.is_some() { 1 } else { 0 });
        let loop_mode = if loop_points.is_some() { loop_mode } else { 0 };

        let mut indices = [0usize; 2];
        for (c, index) in indices.iter_mut().enumerate().take(channels) {
            let key = (i, c, root, loop_points);
            *index = *made.entry(key).or_insert_with(|| {
                let start = (offsets[i] + c * (format.frames + PAD)) as u32;
                let (loop_start, loop_end) = loop_points.unwrap_or((0, 0));
                samples.push(SampleHeader {
                    name: sample_name(&spec.sample, c, channels),
                    start,
                    end: start + frames as u32,
                    loop_start: start + loop_start,
                    loop_end: start + loop_end,
                    sample_rate: fact.sample_rate,
                    original_pitch: root,
                    pitch_correction: 0,
                    link: 0,
                    sample_type: if channels == 1 {
                        1
                    } else if c == 0 {
                        4
                    } else {
                        2
                    },
                });
                samples.len() - 1
            });
        }
        if channels == 2 {
            samples[indices[0]].link = indices[1] as u16;
            samples[indices[1]].link = indices[0] as u16;
        }

        let extra = Arc::new(spec.extra);
        for (c, &index) in indices.iter().enumerate().take(channels) {
            let mut generators = spec.generators.clone();
            generators.set(g::OVERRIDING_ROOT_KEY, root as i16);
            if loop_mode != 0 {
                generators.set(g::SAMPLE_MODES, loop_mode as i16);
            }
            let offset = (spec.offset as usize).min(frames) as i64;
            if offset > 0 {
                generators.set(g::START_ADDRS_OFFSET, (offset % 32768) as i16);
                generators.set(g::START_ADDRS_COARSE_OFFSET, (offset / 32768) as i16);
            }
            if let Some(end) = spec.end.filter(|&e| (e as usize) < frames) {
                let cut = end as i64 - frames as i64;
                generators.set(g::END_ADDRS_OFFSET, (cut % 32768) as i16);
                generators.set(g::END_ADDRS_COARSE_OFFSET, (cut / 32768) as i16);
            }
            let pan = if channels == 1 {
                spec.pan
            } else if c == 0 {
                spec.pan - spec.width
            } else {
                spec.pan + spec.width
            };
            let pan = (pan * 5.0).round().clamp(-500.0, 500.0) as i16;
            if pan != 0 {
                generators.set(g::PAN, pan);
            }
            zones.push(Zone {
                key_lo: spec.key_range.0,
                key_hi: spec.key_range.1,
                vel_lo: spec.velocity_range.0,
                vel_hi: spec.velocity_range.1,
                generators,
                modulators: spec.modulators.clone(),
                link: index,
                sfz: Some(Arc::clone(&extra)),
            });
        }
    }
    if zones.is_empty() {
        return Err(invalid(warnings.first().cloned().unwrap_or_else(|| "no playable regions".into())));
    }

    let instrument = Instrument { name: name.to_string(), zones };
    let preset_zone = Zone {
        key_lo: 0,
        key_hi: 127,
        vel_lo: 0,
        vel_hi: 127,
        generators: GeneratorSet::default(),
        modulators: Vec::new(),
        link: 0,
        sfz: None,
    };
    let preset = Preset { name: name.to_string(), bank: 0, program: 0, zones: vec![preset_zone] };
    let info = SoundFontInfo { format: FileFormat::Sfz, name: name.to_string(), ..Default::default() };
    Ok(Converted {
        info,
        presets: vec![preset],
        instruments: vec![instrument],
        samples,
        points,
        len: total,
        has_sm24,
        counters: counters.len(),
        warnings,
    })
}

/// A sample header's name: the file's name, and the channel of a stereo one.
fn sample_name(path: &str, channel: usize, channels: usize) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    if channels == 2 { format!("{stem}{}", if channel == 0 { "_L" } else { "_R" }) } else { stem.to_string() }
}

/// `f` over `items` on a few threads, the results in order.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    run_workers(|| {
        loop {
            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(item) = items.get(i) else { break };
            let result = f(item);
            results.lock().unwrap()[i] = Some(result);
        }
    });
    results.into_inner().unwrap().into_iter().map(|r| r.unwrap()).collect()
}

/// Runs `work` (which takes jobs from a shared queue until it's empty) on a
/// few threads, or on this one where threads can't be made (WebAssembly).
pub(crate) fn run_workers(work: impl Fn() + Sync) {
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    std::thread::scope(|scope| {
        let spawned = (0..workers).filter(|_| std::thread::Builder::new().spawn_scoped(scope, &work).is_ok()).count();
        if spawned == 0 {
            work();
        }
    });
}
