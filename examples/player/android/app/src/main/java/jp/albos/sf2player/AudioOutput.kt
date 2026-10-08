package jp.albos.sf2player

import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioTrack
import android.os.Process

/**
 * Plays the Rust player through an [AudioTrack]: a thread renders a chunk of
 * 32-bit float stereo and writes it, blocking while the track's buffer is
 * full.
 */
class AudioOutput(private val player: Long, val sampleRate: Int, framesPerBurst: Int) {
    /** Frames rendered per write: whole bursts, about 5 ms or more. */
    private val chunk = framesPerBurst.coerceAtLeast(64).let { burst -> ((240 + burst - 1) / burst) * burst }

    private val track: AudioTrack = run {
        val minimum = AudioTrack.getMinBufferSize(sampleRate, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_FLOAT)
        AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                    .setSampleRate(sampleRate)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                    .build(),
            )
            .setBufferSizeInBytes(maxOf(minimum, chunk * 4 * 2 * 4))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .build()
    }

    @Volatile private var running = false
    private var thread: Thread? = null

    val isRunning: Boolean get() = running

    /** The track's buffer: what lies between rendering and the speaker. */
    val bufferFrames: Int get() = track.bufferSizeInFrames

    val device: String
        get() = track.routedDevice?.let { "${it.productName} (${deviceType(it.type)})" } ?: ""

    fun start() {
        if (running) return
        running = true
        track.play()
        thread = Thread({ renderLoop() }, "sf2player-audio").apply { start() }
    }

    fun stop() {
        if (!running) return
        running = false
        thread?.join()
        thread = null
        track.pause()
        track.flush()
    }

    fun release() {
        stop()
        track.release()
    }

    private fun renderLoop() {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val buffer = FloatArray(chunk * 2)
        while (running) {
            if (!NativePlayer.render(player, buffer, chunk)) buffer.fill(0f)
            track.write(buffer, 0, buffer.size, AudioTrack.WRITE_BLOCKING)
        }
    }

    private fun deviceType(type: Int) = when (type) {
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> "speaker"
        AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET -> "wired"
        AudioDeviceInfo.TYPE_BLUETOOTH_A2DP -> "Bluetooth"
        AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET -> "USB"
        AudioDeviceInfo.TYPE_HDMI -> "HDMI"
        else -> "type $type"
    }
}
