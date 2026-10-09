package jp.albos.sf2player

import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.Uri
import android.provider.OpenableColumns
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.core.net.toUri
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** The player and its audio output, and their state for the UI. */
class PlayerViewModel(app: Application) : AndroidViewModel(app) {
    private val audioManager = app.getSystemService(AudioManager::class.java)
    private val prefs = app.getSharedPreferences("player", Context.MODE_PRIVATE)
    private val sampleRate =
        audioManager.getProperty(AudioManager.PROPERTY_OUTPUT_SAMPLE_RATE)?.toIntOrNull() ?: 48_000
    private val handle = NativePlayer.create(sampleRate)
    private val output = AudioOutput(
        handle,
        sampleRate,
        audioManager.getProperty(AudioManager.PROPERTY_OUTPUT_FRAMES_PER_BUFFER)?.toIntOrNull() ?: 256,
    )

    var state by mutableIntStateOf(NativePlayer.STOPPED)
        private set
    var canPlay by mutableStateOf(false)
        private set
    var position by mutableDoubleStateOf(0.0)
        private set
    var duration by mutableDoubleStateOf(0.0)
        private set
    var activeVoices by mutableIntStateOf(0)
        private set
    var soundFont by mutableStateOf<SoundFontSummary?>(null)
        private set
    var song by mutableStateOf<SongSummary?>(null)
        private set
    var loading by mutableIntStateOf(0)
        private set
    var audio by mutableStateOf(AudioInfo(sampleRate = sampleRate))
        private set
    var error by mutableStateOf<String?>(null)

    /** Play the song once the SF2 it waits for is loaded. */
    private var autoplay = false
    private var idleSince = 0L

    private val focusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
        .setAudioAttributes(
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build(),
        )
        .setOnAudioFocusChangeListener { change ->
            // Another app plays, or a call comes in.
            if (change == AudioManager.AUDIOFOCUS_LOSS || change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT) pause()
        }
        .build()

    /** Headphones unplugged: pause rather than play out loud. */
    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = pause()
    }

    init {
        ContextCompat.registerReceiver(
            app, noisy, IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY), ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        prefs.getString(SOUND_FONT, null)?.let { openSoundFont(it.toUri(), remember = false) }
        viewModelScope.launch {
            while (isActive) {
                poll()
                delay(50)
            }
        }
    }

    override fun onCleared() {
        getApplication<Application>().unregisterReceiver(noisy)
        audioManager.abandonAudioFocusRequest(focusRequest)
        output.release()
        NativePlayer.destroy(handle)
    }

    // Transport

    fun play() {
        if (!canPlay) return
        audioManager.requestAudioFocus(focusRequest)
        output.start()
        NativePlayer.play(handle)
        poll()
    }

    fun pause() {
        NativePlayer.pause(handle)
        poll()
    }

    fun toggle() = if (state == NativePlayer.PLAYING) pause() else play()

    fun stop() {
        NativePlayer.stop(handle)
        poll()
    }

    fun seek(seconds: Double) {
        NativePlayer.seek(handle, seconds)
        poll()
    }

    // Files

    fun openSoundFont(uri: Uri, remember: Boolean = true) {
        load(uri) { fd, name ->
            val error = NativePlayer.loadSoundFont(handle, fd, name)
            if (error == null && remember) rememberSoundFont(uri)
            error
        }
    }

    fun openSong(uri: Uri) {
        autoplay = true
        load(uri) { fd, name -> NativePlayer.loadMidi(handle, fd, name) }
    }

    /** Reads and parses the file off the main thread (an SF2 can be hundreds of MB). */
    private fun load(uri: Uri, call: (fd: Int, name: String) -> String?) {
        loading++
        viewModelScope.launch {
            val message = withContext(Dispatchers.IO) {
                val name = displayName(uri)
                try {
                    val file = getApplication<Application>().contentResolver.openFileDescriptor(uri, "r")
                        ?: return@withContext "$name: can't open"
                    // The native side takes the descriptor and closes it.
                    call(file.detachFd(), name)?.let { "$name: $it" }
                } catch (e: Exception) {
                    "$name: ${e.message}"
                }
            }
            loading--
            error = message
            refreshInfo()
            if (autoplay && loading == 0 && canPlay) {
                autoplay = false
                play()
            }
        }
    }

    private fun displayName(uri: Uri): String {
        val resolver = getApplication<Application>().contentResolver
        runCatching {
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) return cursor.getString(0)
            }
        }
        return uri.lastPathSegment?.substringAfterLast('/') ?: uri.toString()
    }

    /** Keeps access to the SF2 so that it loads again next time. */
    private fun rememberSoundFont(uri: Uri) {
        runCatching {
            getApplication<Application>().contentResolver
                .takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            prefs.edit { putString(SOUND_FONT, uri.toString()) }
        }
    }

    // State

    private fun refreshInfo() {
        val info = JSONObject(NativePlayer.infoJson(handle))
        soundFont = info.optJSONObject("sound_font")?.let(SoundFontSummary::from)
        song = info.optJSONObject("song")?.let(SongSummary::from)
        duration = NativePlayer.duration(handle)
        canPlay = NativePlayer.canPlay(handle)
    }

    private fun poll() {
        state = NativePlayer.state(handle)
        position = NativePlayer.position(handle)
        activeVoices = NativePlayer.activeVoices(handle)
        if (output.isRunning) {
            audio = audio.copy(bufferFrames = output.bufferFrames, device = output.device)
        }
        // Stop the output a while after the music stops (and the last notes
        // have rung out), so an idle app costs no power.
        val now = System.currentTimeMillis()
        if (state == NativePlayer.PLAYING || activeVoices > 0) {
            idleSince = 0
        } else if (output.isRunning) {
            if (idleSince == 0L) idleSince = now
            else if (now - idleSince > 3_000) output.stop()
        }
    }

    private companion object {
        const val SOUND_FONT = "soundFont"
    }
}

data class AudioInfo(
    val sampleRate: Int,
    val bufferFrames: Int = 0,
    val device: String = "",
)

/** sf2player's SoundFontSummary. */
data class SoundFontSummary(
    val fileName: String,
    val name: String,
    val format: String,
    val fileSize: Long,
    val presets: Int,
    val instruments: Int,
    val samples: Int,
    val copyright: String,
    val engineers: String,
    val comment: String,
) {
    companion object {
        fun from(json: JSONObject) = SoundFontSummary(
            fileName = json.getString("file_name"),
            name = json.getString("name"),
            format = json.getString("format"),
            fileSize = json.getLong("file_size"),
            presets = json.getInt("presets"),
            instruments = json.getInt("instruments"),
            samples = json.getInt("samples"),
            copyright = json.getString("copyright"),
            engineers = json.getString("engineers"),
            comment = json.getString("comment"),
        )
    }
}

/** sf2player's SongSummary. */
data class SongSummary(
    val fileName: String,
    val title: String,
    val format: Int,
    /** "GM", "GM2", "GS", "XG" or empty. */
    val standard: String,
    val tracks: Int,
    val ticksPerQuarter: Int,
    val tempoBpm: Double,
    val tempoChanges: Int,
    val notes: Int,
    val channels: Int,
    val duration: Double,
) {
    companion object {
        fun from(json: JSONObject) = SongSummary(
            fileName = json.getString("file_name"),
            title = json.getString("title"),
            format = json.getInt("format"),
            standard = json.optString("standard"),
            tracks = json.getInt("tracks"),
            ticksPerQuarter = json.getInt("ticks_per_quarter"),
            tempoBpm = json.getDouble("tempo_bpm"),
            tempoChanges = json.getInt("tempo_changes"),
            notes = json.getInt("notes"),
            channels = json.getInt("channels"),
            duration = json.getDouble("duration"),
        )
    }
}
