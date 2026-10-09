// sf2synth: an SF2 (and DLS) synthesizer, behind a C interface. Implemented
// in Rust in ../src/lib.rs; link libsf2synth_c.a (or the shared library).
//
// Threads: load fonts and create synths anywhere (this allocates). A synth
// is not locked: call the functions taking an Sf2Synth from one thread at a
// time, normally the audio thread. MIDI input, rendering and the setters
// marked real-time safe never allocate, lock or block there.

#ifndef SF2SYNTH_H
#define SF2SYNTH_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct Sf2Font Sf2Font;
typedef struct Sf2Synth Sf2Synth;

typedef struct Sf2SynthSettings {
    // Output sample rate, 8000…192000 Hz.
    uint32_t sample_rate;
    // Voices that can sound at once (1…4096).
    uint32_t max_voices;
    // Gain applied to the mix. 1.0 by default; 1.2 matches Apple's MIDI
    // synth playing the same SoundFont.
    float master_gain;
    // The drum channel after a reset, 0-based (9 = General MIDI channel 10),
    // or -1 for none.
    int32_t drum_channel;
    // The built-in reverb and chorus. The effect sends of
    // sf2synth_render_events work either way.
    bool reverb;
    bool chorus;
    // 4-point cubic interpolation (true) or linear.
    bool cubic_interpolation;
} Sf2SynthSettings;

// A MIDI message at a frame of the buffer being rendered.
typedef struct Sf2MidiEvent {
    // Frame offset in the buffer.
    uint32_t frame;
    // Bytes of one complete message: a channel message (no running status),
    // a System Exclusive message (F0 … F7) or System Reset (FF).
    uint32_t length;
    const uint8_t *bytes;
} Sf2MidiEvent;

// The default settings at `sample_rate`.
Sf2SynthSettings sf2synth_settings_default(uint32_t sample_rate);

// MARK: - Fonts
//
// A font can play in several synths at once. Each synth keeps the font's data
// alive, so a font can be freed while synths still play it.
//
// The loaders return NULL on failure; then, if `error` isn't NULL, *error is
// set to a message to free with sf2synth_string_free.

// Reads an SF2 or DLS file into memory.
Sf2Font *sf2synth_font_load(const char *path, char **error);
// Maps an SF2 file into memory instead of reading it: the sample data is read
// from the file as it plays and costs no heap memory, however large the file.
// The file must not change while mapped. Pages not yet read are read on
// first use, possibly on the audio thread; the whole file is prefetched in
// the background. Mapped on Unix (macOS, iOS, Linux, Android) and Windows;
// read like sf2synth_font_load where mapping isn't available. DLS files are
// converted on loading, so their samples end up in memory anyway.
Sf2Font *sf2synth_font_load_mapped(const char *path, char **error);
// Parses an SF2 or DLS file held in memory (the bytes are copied).
Sf2Font *sf2synth_font_load_memory(const uint8_t *bytes, size_t length, char **error);
void sf2synth_font_free(Sf2Font *font);

size_t sf2synth_font_preset_count(const Sf2Font *font);
// Preset `index` (0…count-1): its bank (128 for drum kits), program and name
// (NUL-terminated, cut to fit `name_capacity` bytes; `name` may be NULL).
// False if there's no such preset.
bool sf2synth_font_preset(const Sf2Font *font, size_t index, uint16_t *bank, uint16_t *program, char *name,
                          size_t name_capacity);

// MARK: - Synthesizer

// A synth playing `font`, or NULL (with *error set as for the loaders) when
// the settings are invalid. `settings` may be NULL for the defaults at
// 44.1 kHz.
Sf2Synth *sf2synth_new(const Sf2Font *font, const Sf2SynthSettings *settings, char **error);
void sf2synth_free(Sf2Synth *synth);

// Changes the output sample rate, keeping the MIDI state; sounding notes
// stop. Allocates: call it while not rendering (in an Audio Unit, from
// allocateRenderResources). False if the rate is out of range.
bool sf2synth_set_sample_rate(Sf2Synth *synth, uint32_t sample_rate);
uint32_t sf2synth_sample_rate(const Sf2Synth *synth);

// MIDI input (real-time safe).
//
// One complete message: a channel message (no running status), a System
// Exclusive message (F0 … F7: GM/GM2 System On, GS Reset, XG System On,
// master volume and tuning, GS rhythm parts and scale tuning, XG part mode,
// GS reverb and chorus) or System Reset (FF).
void sf2synth_midi(Sf2Synth *synth, const uint8_t *bytes, size_t length);
// Raw MIDI bytes as a port delivers them: running status, System Exclusive
// messages split over several calls, real-time bytes in between.
void sf2synth_midi_bytes(Sf2Synth *synth, const uint8_t *bytes, size_t length);
// Universal MIDI Packets (MIDI 2.0), as 32-bit words: MIDI 1.0 and 2.0
// channel voice messages (reduced to MIDI 1.0 resolution), SysEx7 and
// System Reset. Groups are ignored.
void sf2synth_ump(Sf2Synth *synth, const uint32_t *words, size_t count);

// Rendering (real-time safe). Buffers are overwritten.

// Renders `frames` frames into two channel buffers.
void sf2synth_render(Sf2Synth *synth, float *left, float *right, size_t frames);
// Renders `frames` frames, applying each event at its frame (events in time
// order; a frame past the end applies after the buffer). `reverb_send` and
// `chorus_send`, if not NULL, get the mono signal the voices send to the
// reverb (CC91) and chorus (CC93), scaled like the mix: feed them to your own
// effects, with the built-in ones off (sf2synth_set_reverb_enabled).
void sf2synth_render_events(Sf2Synth *synth, float *left, float *right, float *reverb_send, float *chorus_send,
                            size_t frames, const Sf2MidiEvent *events, size_t event_count);

// Releases every note (ignoring the pedals).
void sf2synth_note_off_all(Sf2Synth *synth);
// Fades every voice out within a few milliseconds.
void sf2synth_sound_off(Sf2Synth *synth);
// Silences everything and returns every channel and effect to its initial
// state (as System Reset).
void sf2synth_reset(Sf2Synth *synth);

// Setters (real-time safe).
void sf2synth_set_master_gain(Sf2Synth *synth, float gain);

// sf2synth_set_interpolation
#define SF2SYNTH_INTERPOLATION_LINEAR 0
#define SF2SYNTH_INTERPOLATION_CUBIC 1
// 32-point windowed sinc, band-limited to the pitch: no aliasing when a
// sample plays higher than recorded. About 1.5–2× the rendering time.
#define SF2SYNTH_INTERPOLATION_SINC 2
void sf2synth_set_interpolation(Sf2Synth *synth, int32_t interpolation);
void sf2synth_set_reverb_enabled(Sf2Synth *synth, bool enabled);
void sf2synth_set_chorus_enabled(Sf2Synth *synth, bool enabled);
// The output limiter (on by default): keeps the mix below full scale
// (−0.45 dBFS) without clipping, turning down only the passages that would
// go over. It delays the mix by sf2synth_latency frames (1 ms); the effect
// sends are neither limited nor delayed.
void sf2synth_set_limiter(Sf2Synth *synth, bool enabled);
// Frames the mix comes out late by (the limiter's look-ahead when it's on).
uint32_t sf2synth_latency(const Sf2Synth *synth);
// The limiter's latency at `sample_rate`, in frames.
uint32_t sf2synth_limiter_latency(uint32_t sample_rate);

// Makes a channel (0…15) play drum kits (bank 128) or melodic presets, until
// the next reset.
void sf2synth_set_channel_drum(Sf2Synth *synth, uint8_t channel, bool drum);

uint32_t sf2synth_active_voices(const Sf2Synth *synth);

void sf2synth_string_free(char *text);

#ifdef __cplusplus
}
#endif

#endif
