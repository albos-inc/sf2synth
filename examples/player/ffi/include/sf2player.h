// The sf2player C interface (implemented in Rust in ../src/lib.rs).
//
// Every function is thread-safe. Call sf2player_render from the audio
// callback and everything else from the UI; files are parsed before the
// player's lock is taken, so loading doesn't interrupt playback.

#ifndef SF2PLAYER_H
#define SF2PLAYER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct Sf2Player Sf2Player;

// sf2player_state
#define SF2PLAYER_STOPPED 0
#define SF2PLAYER_PLAYING 1
#define SF2PLAYER_PAUSED 2

Sf2Player *sf2player_new(uint32_t sample_rate);
// Stop the audio callback first.
void sf2player_free(Sf2Player *player);

// Load an SF2 (or DLS) file / a Standard MIDI File. NULL on success, else an
// error message to free with sf2player_string_free.
char *sf2player_load_sound_font(const Sf2Player *player, const char *path);
char *sf2player_load_midi(const Sf2Player *player, const char *path);

void sf2player_play(const Sf2Player *player);
void sf2player_pause(const Sf2Player *player);
// Silences every note and goes back to the start.
void sf2player_stop(const Sf2Player *player);
void sf2player_seek(const Sf2Player *player, double seconds);

int32_t sf2player_state(const Sf2Player *player);
// Whether both an SF2 and a MIDI file are loaded.
bool sf2player_can_play(const Sf2Player *player);
double sf2player_position(const Sf2Player *player);
double sf2player_duration(const Sf2Player *player);
uint32_t sf2player_active_voices(const Sf2Player *player);

// {"sound_font": {...} | null, "song": {...} | null}; free the string with
// sf2player_string_free. See SoundFontSummary and SongSummary in
// ../../core/src/lib.rs for the fields.
char *sf2player_info_json(const Sf2Player *player);

// Renders `frames` frames into two channel buffers. Real-time safe. Returns
// false (with silence) if the UI held the lock.
bool sf2player_render(const Sf2Player *player, float *left, float *right, size_t frames);

void sf2player_string_free(char *text);

#ifdef __cplusplus
}
#endif

#endif
