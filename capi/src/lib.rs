//! The sf2synth synthesizer behind the C interface of `include/sf2synth.h`.
//! [`map_file`] (from `sf2synth-mmap`) memory-maps a SoundFont without
//! dependencies.

use std::ffi::{CStr, CString, c_char};
use std::sync::Arc;

use sf2synth::{Interpolation, SoundFont, Synthesizer, SynthesizerSettings};

pub use sf2synth_mmap::map_file;

/// `Sf2Font`.
pub struct Font(Arc<SoundFont>);

/// `Sf2Synth`.
pub struct Synth(Synthesizer);

/// `Sf2SynthSettings`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub sample_rate: u32,
    pub max_voices: u32,
    pub master_gain: f32,
    pub drum_channel: i32,
    pub reverb: bool,
    pub chorus: bool,
    pub cubic_interpolation: bool,
}

impl Settings {
    fn to_rust(self) -> SynthesizerSettings {
        let mut settings = SynthesizerSettings::new(self.sample_rate);
        settings.max_voices = self.max_voices as usize;
        settings.master_gain = self.master_gain;
        settings.drum_channel = u8::try_from(self.drum_channel).ok().filter(|&c| c < 16);
        settings.reverb.enabled = self.reverb;
        settings.chorus.enabled = self.chorus;
        settings.interpolation = if self.cubic_interpolation { Interpolation::Cubic } else { Interpolation::Linear };
        settings
    }
}

/// `Sf2MidiEvent`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MidiEvent {
    pub frame: u32,
    pub length: u32,
    pub bytes: *const u8,
}

#[unsafe(no_mangle)]
pub extern "C" fn sf2synth_settings_default(sample_rate: u32) -> Settings {
    let defaults = SynthesizerSettings::new(sample_rate);
    Settings {
        sample_rate,
        max_voices: defaults.max_voices as u32,
        master_gain: defaults.master_gain,
        drum_channel: defaults.drum_channel.map_or(-1, i32::from),
        reverb: defaults.reverb.enabled,
        chorus: defaults.chorus.enabled,
        cubic_interpolation: defaults.interpolation == Interpolation::Cubic,
    }
}

// MARK: - Helpers

/// Stores `message` in `*error` (if `error` isn't NULL) and returns NULL.
///
/// # Safety
/// `error` is NULL or writable.
unsafe fn fail<T>(error: *mut *mut c_char, message: impl ToString) -> *mut T {
    if !error.is_null() {
        let text = CString::new(message.to_string().replace('\0', " ")).map_or(std::ptr::null_mut(), CString::into_raw);
        unsafe { *error = text };
    }
    std::ptr::null_mut()
}

/// # Safety
/// `pointer` is NULL or points to `length` readable items.
unsafe fn slice<'a, T>(pointer: *const T, length: usize) -> &'a [T] {
    if pointer.is_null() || length == 0 { &[] } else { unsafe { std::slice::from_raw_parts(pointer, length) } }
}

/// # Safety
/// `pointer` is NULL or points to `length` writable items.
unsafe fn slice_mut<'a, T>(pointer: *mut T, length: usize) -> Option<&'a mut [T]> {
    if pointer.is_null() { None } else { Some(unsafe { std::slice::from_raw_parts_mut(pointer, length) }) }
}

/// # Safety
/// `path` is a NUL-terminated string.
unsafe fn path_string(path: *const c_char) -> Result<String, &'static str> {
    if path.is_null() {
        return Err("no path");
    }
    Ok(unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned())
}

fn new_font(result: sf2synth::Result<SoundFont>) -> Result<*mut Font, String> {
    let font = result.map_err(|e| e.to_string())?;
    Ok(Box::into_raw(Box::new(Font(Arc::new(font)))))
}

// MARK: - Fonts

/// # Safety
/// `path` is a NUL-terminated string; `error` is NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_font_load(path: *const c_char, error: *mut *mut c_char) -> *mut Font {
    let result = unsafe { path_string(path) }
        .map_err(String::from)
        .and_then(|path| std::fs::read(&path).map_err(|e| format!("{path}: {e}")))
        .and_then(|bytes| new_font(SoundFont::from_bytes(bytes)));
    result.unwrap_or_else(|message| unsafe { fail(error, message) })
}

/// # Safety
/// `path` is a NUL-terminated string; `error` is NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_font_load_mapped(path: *const c_char, error: *mut *mut c_char) -> *mut Font {
    let result = unsafe { path_string(path) }
        .map_err(String::from)
        .and_then(|path| map_file(&path).map_err(|e| format!("{path}: {e}")))
        .and_then(|bytes| new_font(SoundFont::from_shared(bytes)));
    result.unwrap_or_else(|message| unsafe { fail(error, message) })
}

/// # Safety
/// `bytes` points to `length` readable bytes; `error` is NULL or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_font_load_memory(
    bytes: *const u8,
    length: usize,
    error: *mut *mut c_char,
) -> *mut Font {
    let bytes = unsafe { slice(bytes, length) }.to_vec();
    new_font(SoundFont::from_bytes(bytes)).unwrap_or_else(|message| unsafe { fail(error, message) })
}

/// # Safety
/// `font` is NULL or from a loader, freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_font_free(font: *mut Font) {
    if !font.is_null() {
        drop(unsafe { Box::from_raw(font) });
    }
}

/// # Safety
/// `font` is NULL or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_font_preset_count(font: *const Font) -> usize {
    unsafe { font.as_ref() }.map_or(0, |font| font.0.presets().len())
}

/// # Safety
/// `font` is NULL or live; `bank` and `program` are NULL or writable;
/// `name` is NULL or holds `name_capacity` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_font_preset(
    font: *const Font,
    index: usize,
    bank: *mut u16,
    program: *mut u16,
    name: *mut c_char,
    name_capacity: usize,
) -> bool {
    let Some(preset) = (unsafe { font.as_ref() }).and_then(|font| font.0.presets().get(index)) else { return false };
    unsafe {
        if let Some(bank) = bank.as_mut() {
            *bank = preset.bank();
        }
        if let Some(program) = program.as_mut() {
            *program = preset.program();
        }
        if let Some(name) = slice_mut(name as *mut u8, name_capacity).filter(|n| !n.is_empty()) {
            let text = preset.name().as_bytes();
            // Cut on a character boundary, leaving room for the NUL.
            let mut length = text.len().min(name.len() - 1);
            while !preset.name().is_char_boundary(length) {
                length -= 1;
            }
            name[..length].copy_from_slice(&text[..length]);
            name[length] = 0;
        }
    }
    true
}

// MARK: - Synthesizer

/// # Safety
/// `font` is NULL or live; `settings` is NULL or readable; `error` is NULL
/// or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_new(
    font: *const Font,
    settings: *const Settings,
    error: *mut *mut c_char,
) -> *mut Synth {
    let Some(font) = (unsafe { font.as_ref() }) else { return unsafe { fail(error, "no font") } };
    let settings = unsafe { settings.as_ref() }.copied().unwrap_or_else(|| sf2synth_settings_default(44100));
    match Synthesizer::new(Arc::clone(&font.0), &settings.to_rust()) {
        Ok(synth) => Box::into_raw(Box::new(Synth(synth))),
        Err(e) => unsafe { fail(error, e) },
    }
}

/// # Safety
/// `synth` is NULL or from `sf2synth_new`, freed once and not in use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_free(synth: *mut Synth) {
    if !synth.is_null() {
        drop(unsafe { Box::from_raw(synth) });
    }
}

/// Runs `f` on the synth unless it's NULL.
///
/// # Safety
/// `synth` is NULL or live and not in use on another thread.
unsafe fn with<T: Default>(synth: *mut Synth, f: impl FnOnce(&mut Synthesizer) -> T) -> T {
    match unsafe { synth.as_mut() } {
        Some(synth) => f(&mut synth.0),
        None => T::default(),
    }
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_set_sample_rate(synth: *mut Synth, sample_rate: u32) -> bool {
    unsafe { with(synth, |s| s.set_sample_rate(sample_rate).is_ok()) }
}

/// # Safety
/// `synth` is NULL or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_sample_rate(synth: *const Synth) -> u32 {
    unsafe { synth.as_ref() }.map_or(0, |s| s.0.sample_rate())
}

/// # Safety
/// See [`with`]; `bytes` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_midi(synth: *mut Synth, bytes: *const u8, length: usize) {
    let message = unsafe { slice(bytes, length) };
    unsafe { with(synth, |s| s.process_midi_message(message)) }
}

/// # Safety
/// See [`with`]; `bytes` points to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_midi_bytes(synth: *mut Synth, bytes: *const u8, length: usize) {
    let bytes = unsafe { slice(bytes, length) };
    unsafe { with(synth, |s| s.process_midi_bytes(bytes)) }
}

/// # Safety
/// See [`with`]; `words` points to `count` readable words.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_ump(synth: *mut Synth, words: *const u32, count: usize) {
    let words = unsafe { slice(words, count) };
    unsafe { with(synth, |s| s.process_ump(words)) }
}

/// # Safety
/// See [`with`]; `left` and `right` are NULL or hold `frames` floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_render(synth: *mut Synth, left: *mut f32, right: *mut f32, frames: usize) {
    unsafe {
        sf2synth_render_events(
            synth,
            left,
            right,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            frames,
            std::ptr::null(),
            0,
        )
    }
}

/// # Safety
/// See [`with`]; `left`, `right`, `reverb_send` and `chorus_send` are NULL
/// or hold `frames` floats; `events` points to `event_count` events whose
/// `bytes` point to `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_render_events(
    synth: *mut Synth,
    left: *mut f32,
    right: *mut f32,
    reverb_send: *mut f32,
    chorus_send: *mut f32,
    frames: usize,
    events: *const MidiEvent,
    event_count: usize,
) {
    let (Some(left), Some(right)) = (unsafe { slice_mut(left, frames) }, unsafe { slice_mut(right, frames) }) else {
        return;
    };
    let mut reverb = unsafe { slice_mut(reverb_send, frames) };
    let mut chorus = unsafe { slice_mut(chorus_send, frames) };
    let events = unsafe { slice(events, event_count) };
    let Some(synth) = (unsafe { synth.as_mut() }) else {
        for buffer in [Some(left), Some(right), reverb, chorus].into_iter().flatten() {
            buffer.fill(0.0);
        }
        return;
    };
    let synth = &mut synth.0;
    let mut done = 0;
    let mut render_to = |synth: &mut Synthesizer, end: usize| {
        if end > done {
            let range = done..end;
            synth.render_with_sends(
                &mut left[range.clone()],
                &mut right[range.clone()],
                reverb.as_deref_mut().map(|r| &mut r[range.clone()]),
                chorus.as_deref_mut().map(|c| &mut c[range]),
            );
            done = end;
        }
    };
    for event in events {
        render_to(synth, (event.frame as usize).min(frames));
        synth.process_midi_message(unsafe { slice(event.bytes, event.length as usize) });
    }
    render_to(synth, frames);
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_note_off_all(synth: *mut Synth) {
    unsafe { with(synth, Synthesizer::note_off_all) }
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_sound_off(synth: *mut Synth) {
    unsafe { with(synth, Synthesizer::sound_off_all) }
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_reset(synth: *mut Synth) {
    unsafe { with(synth, Synthesizer::reset) }
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_set_master_gain(synth: *mut Synth, gain: f32) {
    unsafe { with(synth, |s| s.set_master_gain(gain)) }
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_set_interpolation(synth: *mut Synth, interpolation: i32) {
    let interpolation = match interpolation {
        0 => Interpolation::Linear,
        2 => Interpolation::Sinc,
        _ => Interpolation::Cubic,
    };
    unsafe { with(synth, |s| s.set_interpolation(interpolation)) }
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_set_reverb_enabled(synth: *mut Synth, enabled: bool) {
    unsafe {
        with(synth, |s| {
            let settings = sf2synth::ReverbSettings { enabled, ..s.reverb_settings() };
            s.set_reverb_settings(settings)
        })
    }
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_set_chorus_enabled(synth: *mut Synth, enabled: bool) {
    unsafe {
        with(synth, |s| {
            let settings = sf2synth::ChorusSettings { enabled, ..s.chorus_settings() };
            s.set_chorus_settings(settings)
        })
    }
}

/// # Safety
/// See [`with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_set_channel_drum(synth: *mut Synth, channel: u8, drum: bool) {
    unsafe { with(synth, |s| s.set_channel_drum(channel, drum)) }
}

/// # Safety
/// `synth` is NULL or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_active_voices(synth: *const Synth) -> u32 {
    unsafe { synth.as_ref() }.map_or(0, |s| s.0.active_voice_count() as u32)
}

/// # Safety
/// `text` is NULL or a string this library returned, freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2synth_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(unsafe { CString::from_raw(text) });
    }
}
