//! JNI functions for `jp.albos.sf2player.NativePlayer` (see
//! `android/app/src/main/java/jp/albos/sf2player/NativePlayer.kt`).
//!
//! Files arrive as file descriptors: Android hands out documents as
//! `content://` URIs, not paths, and reading them here keeps a large SF2 out
//! of the Java heap.

use std::fs::File;
use std::io::Read;
use std::os::fd::FromRawFd;

use jni::JNIEnv;
use jni::objects::{JClass, JFloatArray, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jdouble, jint, jlong, jstring};
use sf2player::Player;

use super::Handle;

fn handle<'a>(handle: jlong) -> &'a Handle {
    // SAFETY: Kotlin passes back the pointer `create` returned, until `destroy`.
    unsafe { &*(handle as *const Handle) }
}

/// Reads the file of `fd`, taking ownership of (and closing) it.
fn read_fd(fd: jint) -> Result<Vec<u8>, String> {
    // SAFETY: Kotlin detaches the descriptor from its ParcelFileDescriptor,
    // so it is ours to read and close.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn error_string(env: &mut JNIEnv, result: Result<(), String>) -> jstring {
    match result {
        Ok(()) => std::ptr::null_mut(),
        Err(message) => env.new_string(message).map_or(std::ptr::null_mut(), |s| s.into_raw()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_create(_: JNIEnv, _: JClass, sample_rate: jint) -> jlong {
    Box::into_raw(Box::new(Handle::new(sample_rate.max(1) as u32))) as jlong
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_destroy(_: JNIEnv, _: JClass, player: jlong) {
    if player != 0 {
        // SAFETY: as in `handle`; Kotlin stops the audio thread first.
        drop(unsafe { Box::from_raw(player as *mut Handle) });
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_loadSoundFont<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    player: jlong,
    fd: jint,
    name: JString<'l>,
) -> jstring {
    let name: String = env.get_string(&name).map(Into::into).unwrap_or_default();
    let result = read_fd(fd).and_then(|bytes| handle(player).load_sound_font(bytes, &name));
    error_string(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_loadMidi<'l>(
    mut env: JNIEnv<'l>,
    _: JClass<'l>,
    player: jlong,
    fd: jint,
    name: JString<'l>,
) -> jstring {
    let name: String = env.get_string(&name).map(Into::into).unwrap_or_default();
    let result = read_fd(fd).and_then(|bytes| handle(player).load_song(&bytes, &name));
    error_string(&mut env, result)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_play(_: JNIEnv, _: JClass, player: jlong) {
    handle(player).with(Player::play);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_pause(_: JNIEnv, _: JClass, player: jlong) {
    handle(player).with(Player::pause);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_stop(_: JNIEnv, _: JClass, player: jlong) {
    handle(player).with(Player::stop);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_seek(
    _: JNIEnv,
    _: JClass,
    player: jlong,
    seconds: jdouble,
) {
    handle(player).with(|p| p.seek(seconds));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_state(_: JNIEnv, _: JClass, player: jlong) -> jint {
    handle(player).with(|p| p.state().code())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_canPlay(_: JNIEnv, _: JClass, player: jlong) -> jboolean {
    if handle(player).with(|p| p.can_play()) { JNI_TRUE } else { JNI_FALSE }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_position(_: JNIEnv, _: JClass, player: jlong) -> jdouble {
    handle(player).with(|p| p.position())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_duration(_: JNIEnv, _: JClass, player: jlong) -> jdouble {
    handle(player).with(|p| p.duration())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_activeVoices(_: JNIEnv, _: JClass, player: jlong) -> jint {
    handle(player).with(|p| p.active_voices() as jint)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_infoJson<'l>(
    env: JNIEnv<'l>,
    _: JClass<'l>,
    player: jlong,
) -> jstring {
    env.new_string(handle(player).info_json()).map_or(std::ptr::null_mut(), |s| s.into_raw())
}

/// Renders `frames` interleaved stereo frames into `buffer`. False if the UI
/// held the lock (Kotlin then writes silence).
#[unsafe(no_mangle)]
pub extern "system" fn Java_jp_albos_sf2player_NativePlayer_render<'l>(
    env: JNIEnv<'l>,
    _: JClass<'l>,
    player: jlong,
    buffer: JFloatArray<'l>,
    frames: jint,
) -> jboolean {
    thread_local! {
        static SCRATCH: std::cell::RefCell<Vec<f32>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let samples = frames.max(0) as usize * 2;
    SCRATCH.with_borrow_mut(|scratch| {
        if scratch.len() < samples {
            scratch.resize(samples, 0.0);
        }
        let output = &mut scratch[..samples];
        if !handle(player).render_interleaved(output) {
            return JNI_FALSE;
        }
        match env.set_float_array_region(&buffer, 0, output) {
            Ok(()) => JNI_TRUE,
            Err(_) => JNI_FALSE,
        }
    })
}
