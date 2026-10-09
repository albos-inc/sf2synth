//! Writing a preset as an SFZ file with FLAC samples.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;

use crate::generator::{self as g, GeneratorSet};
use crate::modulator::{self, Modulator, SourceInput, controller, source};
use crate::sfz::{SfzRegion, Trigger};
use crate::soundfont::{FileFormat, SoundFont, Zone};

/// An SFZ instrument and its FLAC samples, made from a preset of a
/// [`SoundFont`] (SF2, DLS or SFZ) so that sf2synth plays it as it plays
/// the preset: the same samples (lossless, 16 or 24-bit), keys, velocities,
/// tuning, envelopes, filter, velocity response and pan.
///
/// What SFZ can't say is left out and listed in [`warnings`](Self::warnings)
/// (modulators other than velocity → level and velocity → cutoff, key
/// scaling of the envelopes, LFO → pitch from the modulation LFO).
///
/// ```no_run
/// let font = sf2synth::SoundFont::open("piano.sf2")?;
/// let sfz = sf2synth::writer::SfzFiles::from_preset(&font, 0, "piano samples/");
/// sfz.write("out/piano.sfz")?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SfzFiles {
    /// The SFZ text.
    pub sfz: String,
    /// The sample files: path relative to the SFZ file, FLAC bytes.
    pub samples: Vec<(String, Vec<u8>)>,
    pub warnings: Vec<String>,
}

impl SfzFiles {
    /// The preset `preset` (an index into [`SoundFont::presets`]), its
    /// samples to go in `sample_dir` (relative to the SFZ file, ending in
    /// `/`, or empty for the SFZ file's own directory).
    ///
    /// SF2 attenuations are scaled by 0.4 as [`crate::SynthesizerSettings`]
    /// does by default.
    pub fn from_preset(font: &SoundFont, preset: usize, sample_dir: &str) -> SfzFiles {
        let scale = if font.info().format == FileFormat::Sf2 { 0.4 } else { 1.0 };
        export(font, preset, sample_dir, scale)
    }

    /// Writes the SFZ file at `path` and the samples beside it.
    pub fn write(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let directory = path.parent().unwrap_or(Path::new(""));
        for (relative, bytes) in &self.samples {
            let file = directory.join(relative);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(file, bytes)?;
        }
        std::fs::write(path, &self.sfz)
    }
}

/// A region to write: its zone (or a stereo pair of zones) and sample(s).
struct Out {
    keys: (u8, u8),
    velocities: (u8, u8),
    generators: GeneratorSet,
    modulators: Vec<Modulator>,
    /// Sample indices: one (mono) or left and right.
    samples: Vec<usize>,
    /// Pan of each channel, 0.1 % (SF2 units).
    pans: Vec<i16>,
    region: Option<std::sync::Arc<SfzRegion>>,
}

fn intersect(a: (u8, u8), b: (u8, u8)) -> Option<(u8, u8)> {
    let (low, high) = (a.0.max(b.0), a.1.min(b.1));
    (low <= high).then_some((low, high))
}

fn export(font: &SoundFont, preset_index: usize, sample_dir: &str, attenuation_scale: f32) -> SfzFiles {
    let mut warnings = std::collections::BTreeSet::new();
    let mut outs: Vec<Out> = Vec::new();
    let Some(preset) = font.presets().get(preset_index) else {
        return SfzFiles { sfz: String::new(), samples: Vec::new(), warnings: vec!["no such preset".into()] };
    };
    for preset_zone in preset.zones() {
        let Some(instrument) = font.instruments().get(preset_zone.link()) else { continue };
        let mut pending: Vec<Out> = Vec::new();
        for zone in instrument.zones() {
            let Some(sample) = font.samples().get(zone.link()) else { continue };
            if sample.is_rom() {
                continue;
            }
            let (Some(keys), Some(velocities)) = (
                intersect(preset_zone.key_range(), zone.key_range()),
                intersect(preset_zone.velocity_range(), zone.velocity_range()),
            ) else {
                continue;
            };
            let (generators, modulators) = combine(preset_zone, zone);
            let pan = generators.get(g::PAN).unwrap_or(0);
            pending.push(Out {
                keys,
                velocities,
                generators,
                modulators,
                samples: vec![zone.link()],
                pans: vec![pan],
                region: zone.sfz.clone(),
            });
        }
        // Stereo pairs become one stereo region.
        let mut used = vec![false; pending.len()];
        for i in 0..pending.len() {
            if used[i] {
                continue;
            }
            used[i] = true;
            let left = &font.samples()[pending[i].samples[0]];
            let partner = (left.sample_type & 0x7fff == 4)
                .then(|| {
                    (0..pending.len()).find(|&j| {
                        !used[j]
                            && pending[j].samples[0] == left.link as usize
                            && same_but_pan(&pending[i], &pending[j])
                    })
                })
                .flatten()
                .filter(|&j| {
                    let right = &font.samples()[pending[j].samples[0]];
                    right.end - right.start == left.end - left.start
                        && right.loop_start - right.start == left.loop_start - left.start
                        && right.loop_end - right.start == left.loop_end - left.start
                        && right.sample_rate == left.sample_rate
                        && right.original_pitch == left.original_pitch
                        && right.pitch_correction == left.pitch_correction
                });
            let mut out = std::mem::replace(
                &mut pending[i],
                Out {
                    keys: (0, 0),
                    velocities: (0, 0),
                    generators: GeneratorSet::default(),
                    modulators: Vec::new(),
                    samples: Vec::new(),
                    pans: Vec::new(),
                    region: None,
                },
            );
            if let Some(j) = partner {
                used[j] = true;
                out.samples.push(pending[j].samples[0]);
                out.pans.push(pending[j].pans[0]);
            }
            outs.push(out);
        }
    }

    // One FLAC per sample (or stereo pair) of points, named after the
    // sample; regions playing the same points share it.
    let points_of = |out: &Out| -> Vec<(u32, u32)> {
        out.samples.iter().map(|&s| (font.samples()[s].start, font.samples()[s].end)).collect()
    };
    let mut files: HashMap<Vec<(u32, u32)>, String> = HashMap::new();
    let mut names_taken: HashMap<String, usize> = HashMap::new();
    let mut jobs: Vec<(Vec<usize>, String)> = Vec::new();
    for out in &outs {
        if files.contains_key(&points_of(out)) {
            continue;
        }
        let base = font.samples()[out.samples[0]].name.clone();
        let base = if out.samples.len() == 2 { strip_side(&base) } else { base };
        let mut name = safe_file_name(&base);
        let count = names_taken.entry(name.to_lowercase()).or_insert(0);
        *count += 1;
        if *count > 1 {
            name = format!("{name} {count}");
        }
        let file = format!("{name}.flac");
        files.insert(points_of(out), file.clone());
        jobs.push((out.samples.clone(), file));
    }
    let bits = if font.is_24_bit() { 24 } else { 16 };
    let scale = (1i64 << (bits - 1)) as f32;
    let encoded: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    crate::sfz::run_workers(|| {
        loop {
            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some((samples, file)) = jobs.get(k) else { break };
            let channels: Vec<Vec<i32>> = samples
                .iter()
                .map(|&s| {
                    let h = &font.samples()[s];
                    font.sample_points(h.start as usize..h.end as usize)
                        .into_iter()
                        .map(|x| (x * scale).round() as i32)
                        .collect()
                })
                .collect();
            let slices: Vec<&[i32]> = channels.iter().map(|c| &c[..]).collect();
            let rate = font.samples()[samples[0]].sample_rate.clamp(1, 655_350);
            let bytes = crate::flac::encode(&slices, rate, bits);
            encoded.lock().unwrap().push((format!("{sample_dir}{file}"), bytes));
        }
    });
    let mut samples = encoded.into_inner().unwrap();
    samples.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sfz = String::new();
    let _ = writeln!(sfz, "// {} — made with sf2synth from {}", preset.name(), describe(font));
    let _ = writeln!(sfz, "\n<control>\ndefault_path={sample_dir}\n");
    for out in &outs {
        let line = region_line(font, out, &files[&points_of(out)], attenuation_scale, &mut warnings);
        let _ = writeln!(sfz, "{line}");
    }
    SfzFiles { sfz, samples, warnings: warnings.into_iter().collect() }
}

fn describe(font: &SoundFont) -> String {
    let format = match font.info().format {
        FileFormat::Sf2 => "SF2",
        FileFormat::Dls => "DLS",
        FileFormat::Sfz => "SFZ",
    };
    if font.info().name.is_empty() { format!("an {format} file") } else { format!("{format} \"{}\"", font.info().name) }
}

/// A zone's generators (instrument values plus preset offsets) and
/// modulators (the defaults, replaced by the instrument's, plus the
/// preset's), as a voice sees them.
fn combine(preset_zone: &Zone, zone: &Zone) -> (GeneratorSet, Vec<Modulator>) {
    let mut generators = zone.generators.clone();
    for generator in 0..g::COUNT as u16 {
        let Some(offset) = preset_zone.generators.get(generator) else { continue };
        if g::allowed_in_preset(generator) {
            let base = zone.generators.get(generator).unwrap_or(g::default_value(generator));
            generators.set(generator, base.saturating_add(offset));
        }
    }
    let mut modulators: Vec<Modulator> = Modulator::SF2_DEFAULTS.to_vec();
    for &m in &zone.modulators {
        match modulators.iter_mut().find(|e| e.same_kind(&m)) {
            Some(existing) => *existing = m,
            None => modulators.push(m),
        }
    }
    modulators.extend(preset_zone.modulators.iter().copied());
    (generators, modulators)
}

fn same_but_pan(a: &Out, b: &Out) -> bool {
    let mut x = a.generators.clone();
    let mut y = b.generators.clone();
    x.set(g::PAN, 0);
    y.set(g::PAN, 0);
    a.keys == b.keys
        && a.velocities == b.velocities
        && (0..g::COUNT as u16).all(|op| x.get(op) == y.get(op))
        && a.modulators == b.modulators
        && a.region.is_some() == b.region.is_some()
}

fn strip_side(name: &str) -> String {
    for suffix in ["_L", "_R", "-L", "-R", " L", " R", "(L)", "(R)", "L", "R"] {
        match name.strip_suffix(suffix) {
            Some(stem) if !stem.is_empty() => return stem.trim_end().to_string(),
            _ => {}
        }
    }
    name.to_string()
}

fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '#' | '+' | '.') { c } else { '_' })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() { "sample".into() } else { cleaned }
}

/// Seconds of a timecent value, with digits enough to read the same
/// timecents back.
fn seconds(timecents: i16) -> String {
    trim(format!("{:.7}", (timecents as f64 / 1200.0).exp2()))
}

fn hertz(cents: f64) -> String {
    trim(format!("{:.6}", 8.175_799 * (cents / 1200.0).exp2()))
}

fn trim(text: String) -> String {
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text }
}

fn number(value: f32) -> String {
    trim(format!("{value:.6}"))
}

/// The opcodes of one region.
fn region_line(
    font: &SoundFont,
    out: &Out,
    file: &str,
    attenuation_scale: f32,
    warnings: &mut std::collections::BTreeSet<String>,
) -> String {
    let get = |op: u16| out.generators.get(op).unwrap_or(g::default_value(op));
    let sample = &font.samples()[out.samples[0]];
    let mut o: Vec<String> = vec![format!("<region> sample={file}")];
    let mut add = |name: &str, value: String| o.push(format!("{name}={value}"));

    add("lokey", out.keys.0.to_string());
    add("hikey", out.keys.1.to_string());
    if out.velocities != (0, 127) {
        add("lovel", out.velocities.0.to_string());
        add("hivel", out.velocities.1.to_string());
    }
    let root = match out.generators.get(g::OVERRIDING_ROOT_KEY) {
        Some(k) if (0..=127).contains(&k) => k as u8,
        _ if sample.original_pitch <= 127 => sample.original_pitch,
        _ => 60,
    };
    add("pitch_keycenter", root.to_string());
    let tune = get(g::COARSE_TUNE) as i32 * 100 + get(g::FINE_TUNE) as i32 + sample.pitch_correction as i32;
    if tune != 0 {
        add("tune", tune.to_string());
    }
    if get(g::SCALE_TUNING) != 100 {
        add("pitch_keytrack", get(g::SCALE_TUNING).to_string());
    }

    // Level: attenuation, and an SFZ region's own gain.
    let mut volume = -(get(g::INITIAL_ATTENUATION) as f32 * attenuation_scale) / 10.0;
    if let Some(region) = &out.region {
        volume += 20.0 * region.gain.max(1e-9).log10();
    }
    if volume.abs() > 1e-6 {
        add("volume", number(volume));
    }
    // Pan: one channel's, or a stereo pair's centre and width.
    match out.pans[..] {
        [pan] if pan != 0 => add("pan", number(pan as f32 / 5.0)),
        [left, right] => {
            let pan = (left as f32 + right as f32) / 10.0;
            let width = (right as f32 - left as f32) / 10.0;
            if pan != 0.0 {
                add("pan", number(pan));
            }
            if width != 100.0 {
                add("width", number(width));
            }
        }
        _ => {}
    }

    // Amplifier envelope.
    for (op, name) in [
        (g::DELAY_VOL_ENV, "ampeg_delay"),
        (g::ATTACK_VOL_ENV, "ampeg_attack"),
        (g::HOLD_VOL_ENV, "ampeg_hold"),
        (g::DECAY_VOL_ENV, "ampeg_decay"),
        (g::RELEASE_VOL_ENV, "ampeg_release"),
    ] {
        let value = get(op);
        if value > -12000 {
            add(name, seconds(value));
        }
    }
    let sustain = get(g::SUSTAIN_VOL_ENV);
    if sustain > 0 {
        add("ampeg_sustain", trim(format!("{:.9}", 100.0 * 10f64.powf(-sustain as f64 / 200.0))));
    }
    if get(g::KEYNUM_TO_VOL_ENV_HOLD) != 0 || get(g::KEYNUM_TO_VOL_ENV_DECAY) != 0 {
        warnings.insert("key scaling of the volume envelope".into());
    }

    // Velocity → level.
    let velocity_attenuation: Vec<&Modulator> = out
        .modulators
        .iter()
        .filter(|m| m.destination == g::INITIAL_ATTENUATION && m.source & 0xff == controller::NOTE_ON_VELOCITY)
        .collect();
    let default = Modulator::SF2_DEFAULTS[0];
    let is_default = velocity_attenuation.len() == 1 && *velocity_attenuation[0] == default;
    let region_table = out.region.as_ref().and_then(|r| r.velocity_gain.as_ref());
    if !is_default || region_table.is_some() {
        let cc = [0u8; 128];
        let level = |velocity: u8| -> f32 {
            let input = SourceInput {
                cc: &cc,
                velocity,
                key: 60,
                channel_pressure: 0,
                pitch_wheel: 8192,
                pitch_wheel_sensitivity: 2.0,
            };
            let centibels: f32 = velocity_attenuation.iter().map(|m| modulator::evaluate(m, &input)).sum();
            let gain = 10f32.powf(-centibels.max(0.0) / 200.0);
            gain * region_table.map_or(1.0, |t| t[velocity as usize])
        };
        let top = level(127).max(1e-9);
        for velocity in 1..=127u8 {
            add(&format!("amp_velcurve_{velocity}"), number((level(velocity) / top).min(1.0)));
        }
    }

    // Filter, with velocity → cutoff and key → cutoff.
    let velocity_filter_kind = controller::NOTE_ON_VELOCITY | source::NEGATIVE;
    let velocity_filter = out
        .modulators
        .iter()
        .find(|m| m.destination == g::INITIAL_FILTER_FC && m.source == velocity_filter_kind)
        .map_or(0, |m| m.amount);
    let key_filter = out
        .modulators
        .iter()
        .find(|m| m.destination == g::INITIAL_FILTER_FC && m.source == controller::NOTE_ON_KEY && m.amount_source == 0)
        .copied();
    let cutoff = get(g::INITIAL_FILTER_FC) as i32;
    if cutoff < 13500 || velocity_filter != 0 || key_filter.is_some() {
        add("fil_type", "lpf_2p".into());
        add("cutoff", hertz((cutoff + velocity_filter as i32) as f64));
        if velocity_filter != 0 {
            add("fil_veltrack", (-(velocity_filter as i32)).to_string());
        }
        if get(g::INITIAL_FILTER_Q) > 0 {
            add("resonance", number(get(g::INITIAL_FILTER_Q) as f32 / 10.0));
        }
        if let Some(k) = key_filter {
            // k.amount / 127 cents per key, from key 0.
            add("fil_keytrack", number(k.amount as f32 / 127.0));
            add("fil_keycenter", "0".into());
        }
    }
    for m in &out.modulators {
        let handled = Modulator::SF2_DEFAULTS.contains(m)
            || (m.destination == g::INITIAL_ATTENUATION && m.source & 0xff == controller::NOTE_ON_VELOCITY)
            || (m.destination == g::INITIAL_FILTER_FC && m.source == velocity_filter_kind)
            || key_filter == Some(*m);
        if !handled && m.amount != 0 {
            warnings.insert(format!(
                "modulator from source {:#06x} to generator {} (amount {})",
                m.source, m.destination, m.amount
            ));
        }
    }

    // Modulation envelope and LFOs.
    let filter_eg = get(g::MOD_ENV_TO_FILTER_FC);
    let pitch_eg = get(g::MOD_ENV_TO_PITCH);
    for (depth, prefix) in [(filter_eg, "fileg"), (pitch_eg, "pitcheg")] {
        if depth == 0 {
            continue;
        }
        add(&format!("{prefix}_depth"), depth.to_string());
        for (op, name) in [
            (g::DELAY_MOD_ENV, "delay"),
            (g::ATTACK_MOD_ENV, "attack"),
            (g::HOLD_MOD_ENV, "hold"),
            (g::DECAY_MOD_ENV, "decay"),
            (g::RELEASE_MOD_ENV, "release"),
        ] {
            if get(op) > -12000 {
                add(&format!("{prefix}_{name}"), seconds(get(op)));
            }
        }
        if get(g::SUSTAIN_MOD_ENV) > 0 {
            add(&format!("{prefix}_sustain"), number(100.0 - get(g::SUSTAIN_MOD_ENV) as f32 / 10.0));
        }
    }
    let lfo_volume = get(g::MOD_LFO_TO_VOLUME);
    let lfo_filter = get(g::MOD_LFO_TO_FILTER_FC);
    for (depth, prefix) in [(-(lfo_volume as f32) / 10.0, "amplfo"), (lfo_filter as f32, "fillfo")] {
        if depth != 0.0 {
            add(&format!("{prefix}_depth"), number(depth));
            add(&format!("{prefix}_freq"), hertz(get(g::FREQ_MOD_LFO) as f64));
            if get(g::DELAY_MOD_LFO) > -12000 {
                add(&format!("{prefix}_delay"), seconds(get(g::DELAY_MOD_LFO)));
            }
        }
    }
    if get(g::MOD_LFO_TO_PITCH) != 0 {
        warnings.insert("modulation LFO → pitch".into());
    }
    if get(g::VIB_LFO_TO_PITCH) != 0 {
        add("pitchlfo_depth", get(g::VIB_LFO_TO_PITCH).to_string());
        add("pitchlfo_freq", hertz(get(g::FREQ_VIB_LFO) as f64));
        if get(g::DELAY_VIB_LFO) > -12000 {
            add("pitchlfo_delay", seconds(get(g::DELAY_VIB_LFO)));
        }
    }

    // Effects and exclusive class.
    if get(g::REVERB_EFFECTS_SEND) > 0 {
        add("effect1", number(get(g::REVERB_EFFECTS_SEND) as f32 / 10.0));
    }
    if get(g::CHORUS_EFFECTS_SEND) > 0 {
        add("effect2", number(get(g::CHORUS_EFFECTS_SEND) as f32 / 10.0));
    }
    let exclusive = get(g::EXCLUSIVE_CLASS);
    if exclusive != 0 && out.region.is_none() {
        add("group", exclusive.to_string());
        add("off_by", exclusive.to_string());
    }

    // Sample addresses and loop, relative to the written file.
    let offset = |fine: u16, coarse: u16| get(fine) as i64 + 32768 * get(coarse) as i64;
    let length = (sample.end - sample.start) as i64;
    let start = offset(g::START_ADDRS_OFFSET, g::START_ADDRS_COARSE_OFFSET).clamp(0, length);
    if start > 0 {
        add("offset", start.to_string());
    }
    let end = (length + offset(g::END_ADDRS_OFFSET, g::END_ADDRS_COARSE_OFFSET)).clamp(start, length);
    if end < length {
        add("end", (end - 1).max(0).to_string());
    }
    let one_shot = out.region.as_ref().is_some_and(|r| r.one_shot);
    match get(g::SAMPLE_MODES) & 3 {
        mode @ (1 | 3) => {
            let loop_start = sample.loop_start as i64 - sample.start as i64
                + offset(g::STARTLOOP_ADDRS_OFFSET, g::STARTLOOP_ADDRS_COARSE_OFFSET);
            let loop_end = sample.loop_end as i64 - sample.start as i64
                + offset(g::ENDLOOP_ADDRS_OFFSET, g::ENDLOOP_ADDRS_COARSE_OFFSET);
            add("loop_mode", if mode == 1 { "loop_continuous" } else { "loop_sustain" }.into());
            add("loop_start", loop_start.clamp(0, length).to_string());
            add("loop_end", (loop_end - 1).clamp(0, length).to_string());
        }
        _ if one_shot => add("loop_mode", "one_shot".into()),
        _ => {}
    }

    // What an SFZ region does beyond SF2.
    if let Some(r) = &out.region {
        match r.trigger {
            Trigger::Attack => {}
            Trigger::Release => add("trigger", "release".into()),
            Trigger::ReleaseKey => add("trigger", "release_key".into()),
            Trigger::First => add("trigger", "first".into()),
            Trigger::Legato => add("trigger", "legato".into()),
        }
        if let Some((length, position, _)) = r.sequence {
            add("seq_length", length.to_string());
            add("seq_position", (position + 1).to_string());
        }
        if r.random != (0.0, 1.0) {
            add("lorand", number(r.random.0));
            add("hirand", number(r.random.1));
        }
        for &(cc, low, high) in &r.cc_conditions {
            add(&format!("locc{cc}"), low.to_string());
            add(&format!("hicc{cc}"), high.to_string());
        }
        if let Some((cc, low, high)) = r.cc_trigger {
            add(&format!("on_locc{cc}"), low.to_string());
            add(&format!("on_hicc{cc}"), high.to_string());
        }
        if r.channels != (0, 15) {
            add("lochan", (r.channels.0 + 1).to_string());
            add("hichan", (r.channels.1 + 1).to_string());
        }
        if r.group != 0 {
            add("group", r.group.to_string());
        }
        if let Some(off_by) = r.off_by {
            add("off_by", off_by.to_string());
            if !r.off_fast {
                add("off_mode", "normal".into());
            }
        }
        if r.amp_keytrack != 0.0 {
            add("amp_keytrack", number(r.amp_keytrack));
            add("amp_keycenter", r.amp_keycenter.to_string());
        }
        for (fade, low, high) in [
            (r.velocity_fade_in, "xfin_lovel", "xfin_hivel"),
            (r.velocity_fade_out, "xfout_lovel", "xfout_hivel"),
            (r.key_fade_in, "xfin_lokey", "xfin_hikey"),
            (r.key_fade_out, "xfout_lokey", "xfout_hikey"),
        ] {
            if let Some(fade) = fade {
                add(low, fade.low.to_string());
                add(high, fade.high.to_string());
            }
        }
        if !r.power_fades.0 {
            add("xf_velcurve", "gain".into());
        }
        if !r.power_fades.1 {
            add("xf_keycurve", "gain".into());
        }
        if r.rt_decay != 0.0 {
            add("rt_decay", number(r.rt_decay));
        }
    }
    o.join(" ")
}
