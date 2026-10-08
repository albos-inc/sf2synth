package jp.albos.sf2player

/**
 * The Rust player (`examples/player/ffi/src/android.rs`). Every function
 * takes the handle from [create]; all are thread-safe, and [render] is meant
 * for the audio thread.
 */
object NativePlayer {
    const val STOPPED = 0
    const val PLAYING = 1
    const val PAUSED = 2

    init {
        System.loadLibrary("sf2player_ffi")
    }

    @JvmStatic external fun create(sampleRate: Int): Long
    @JvmStatic external fun destroy(player: Long)

    /** Reads an SF2 (or DLS) from [fd], which it closes. Null, or an error message. */
    @JvmStatic external fun loadSoundFont(player: Long, fd: Int, fileName: String): String?

    /** Reads a Standard MIDI File from [fd], which it closes. Null, or an error message. */
    @JvmStatic external fun loadMidi(player: Long, fd: Int, fileName: String): String?

    @JvmStatic external fun play(player: Long)
    @JvmStatic external fun pause(player: Long)
    @JvmStatic external fun stop(player: Long)
    @JvmStatic external fun seek(player: Long, seconds: Double)

    @JvmStatic external fun state(player: Long): Int
    @JvmStatic external fun canPlay(player: Long): Boolean
    @JvmStatic external fun position(player: Long): Double
    @JvmStatic external fun duration(player: Long): Double
    @JvmStatic external fun activeVoices(player: Long): Int

    /** `{"sound_font": {...} | null, "song": {...} | null}` */
    @JvmStatic external fun infoJson(player: Long): String

    /**
     * Renders [frames] interleaved stereo frames into [buffer]; false if the
     * UI held the player's lock (the buffer is then left as it was).
     */
    @JvmStatic external fun render(player: Long, buffer: FloatArray, frames: Int): Boolean
}
