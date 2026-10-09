//! The player for Swift and Kotlin: a C interface (`include/sf2player.h`)
//! and, on Android, JNI functions for `jp.albos.sf2player.NativePlayer`.
//!
//! The player sits behind a mutex. The audio callback only ever tries the
//! lock (and plays silence for that buffer if the UI holds it), and files are
//! parsed before the lock is taken, so loading never stalls the audio.

use std::ffi::{CStr, CString, c_char};
use std::sync::{Mutex, MutexGuard};

use sf2player::{LoadedSong, LoadedSoundFont, Player};

#[cfg(target_os = "android")]
mod android;

/// The `Sf2Player` handle.
pub struct Handle {
    inner: Mutex<Inner>,
}

struct Inner {
    player: Player,
    /// Scratch buffers for interleaved rendering.
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Handle {
    fn new(sample_rate: u32) -> Handle {
        let inner = Inner { player: Player::new(sample_rate), left: Vec::new(), right: Vec::new() };
        Handle { inner: Mutex::new(inner) }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn with<T>(&self, f: impl FnOnce(&mut Player) -> T) -> T {
        f(&mut self.lock().player)
    }

    /// Loads an SF2 (or DLS) file, memory-mapped.
    fn load_sound_font(&self, file: &std::fs::File, file_name: &str) -> Result<(), String> {
        let sample_rate = self.with(|p| p.sample_rate());
        let loaded = LoadedSoundFont::open(file, file_name, sample_rate)?;
        let previous = self.with(|p| p.set_sound_font(loaded));
        drop(previous);
        Ok(())
    }

    fn load_song(&self, bytes: &[u8], file_name: &str) -> Result<(), String> {
        let loaded = LoadedSong::new(bytes, file_name)?;
        let previous = self.with(|p| p.set_song(loaded));
        drop(previous);
        Ok(())
    }

    fn info_json(&self) -> String {
        self.with(|p| {
            let info = serde_json::json!({ "sound_font": p.sound_font(), "song": p.song() });
            info.to_string()
        })
    }

    /// Renders into separate channels; false (and silence) if the UI holds
    /// the lock.
    fn render(&self, left: &mut [f32], right: &mut [f32]) -> bool {
        match self.inner.try_lock() {
            Ok(mut inner) => {
                inner.player.render(left, right);
                true
            }
            Err(_) => {
                left.fill(0.0);
                right.fill(0.0);
                false
            }
        }
    }

    /// Renders `output.len() / 2` interleaved stereo frames; false (leaving
    /// `output` untouched) if the UI holds the lock.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    fn render_interleaved(&self, output: &mut [f32]) -> bool {
        let Ok(mut inner) = self.inner.try_lock() else { return false };
        let frames = output.len() / 2;
        let Inner { player, left, right } = &mut *inner;
        if left.len() < frames {
            left.resize(frames, 0.0);
            right.resize(frames, 0.0);
        }
        player.render(&mut left[..frames], &mut right[..frames]);
        for (i, frame) in output.chunks_exact_mut(2).enumerate() {
            frame[0] = left[i];
            frame[1] = right[i];
        }
        true
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn error_string(result: Result<(), String>) -> *mut c_char {
    match result {
        Ok(()) => std::ptr::null_mut(),
        Err(message) => CString::new(message.replace('\0', " ")).map_or(std::ptr::null_mut(), CString::into_raw),
    }
}

fn into_c_string(text: String) -> *mut c_char {
    CString::new(text.replace('\0', " ")).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// # Safety
/// `handle` must come from `sf2player_new` and not have been freed.
unsafe fn handle<'a>(handle: *const Handle) -> &'a Handle {
    unsafe { &*handle }
}

/// # Safety
/// `path` must be a valid NUL-terminated string.
unsafe fn path_string(path: *const c_char) -> String {
    unsafe { CStr::from_ptr(path) }.to_string_lossy().into_owned()
}

#[unsafe(no_mangle)]
pub extern "C" fn sf2player_new(sample_rate: u32) -> *mut Handle {
    Box::into_raw(Box::new(Handle::new(sample_rate)))
}

/// # Safety
/// `player` must come from `sf2player_new`, and the audio callback must be
/// stopped.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_free(player: *mut Handle) {
    if !player.is_null() {
        drop(unsafe { Box::from_raw(player) });
    }
}

/// # Safety
/// `player` must be live and `path` a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_load_sound_font(player: *const Handle, path: *const c_char) -> *mut c_char {
    let path = unsafe { path_string(path) };
    let result = std::fs::File::open(&path).map_err(|e| e.to_string());
    error_string(result.and_then(|file| unsafe { handle(player) }.load_sound_font(&file, file_name(&path))))
}

/// # Safety
/// `player` must be live and `path` a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_load_midi(player: *const Handle, path: *const c_char) -> *mut c_char {
    let path = unsafe { path_string(path) };
    let result = std::fs::read(&path).map_err(|e| e.to_string());
    error_string(result.and_then(|bytes| unsafe { handle(player) }.load_song(&bytes, file_name(&path))))
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_play(player: *const Handle) {
    unsafe { handle(player) }.with(Player::play);
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_pause(player: *const Handle) {
    unsafe { handle(player) }.with(Player::pause);
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_stop(player: *const Handle) {
    unsafe { handle(player) }.with(Player::stop);
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_seek(player: *const Handle, seconds: f64) {
    unsafe { handle(player) }.with(|p| p.seek(seconds));
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_state(player: *const Handle) -> i32 {
    unsafe { handle(player) }.with(|p| p.state().code())
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_can_play(player: *const Handle) -> bool {
    unsafe { handle(player) }.with(|p| p.can_play())
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_position(player: *const Handle) -> f64 {
    unsafe { handle(player) }.with(|p| p.position())
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_duration(player: *const Handle) -> f64 {
    unsafe { handle(player) }.with(|p| p.duration())
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_active_voices(player: *const Handle) -> u32 {
    unsafe { handle(player) }.with(|p| p.active_voices() as u32)
}

/// # Safety
/// `player` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_info_json(player: *const Handle) -> *mut c_char {
    into_c_string(unsafe { handle(player) }.info_json())
}

/// # Safety
/// `player` must be live, and `left` and `right` must each hold `frames`
/// floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_render(
    player: *const Handle,
    left: *mut f32,
    right: *mut f32,
    frames: usize,
) -> bool {
    let (left, right) =
        unsafe { (std::slice::from_raw_parts_mut(left, frames), std::slice::from_raw_parts_mut(right, frames)) };
    unsafe { handle(player) }.render(left, right)
}

/// # Safety
/// `text` must come from this library (or be null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sf2player_string_free(text: *mut c_char) {
    if !text.is_null() {
        drop(unsafe { CString::from_raw(text) });
    }
}
