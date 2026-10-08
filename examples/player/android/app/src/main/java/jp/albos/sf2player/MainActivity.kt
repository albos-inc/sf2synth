package jp.albos.sf2player

import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.text.format.Formatter
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import java.util.Locale

class MainActivity : ComponentActivity() {
    private val player: PlayerViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (savedInstanceState == null) openFrom(intent)
        setContent {
            PlayerTheme {
                Surface(Modifier.fillMaxSize()) { PlayerScreen(player) }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        openFrom(intent)
    }

    /** "Open with" a MIDI file. */
    private fun openFrom(intent: Intent?) {
        if (intent?.action == Intent.ACTION_VIEW) intent.data?.let(player::openSong)
    }
}

@Composable
fun PlayerTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    val context = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    MaterialTheme(colorScheme = colors, content = content)
}

@Composable
fun PlayerScreen(player: PlayerViewModel) {
    val pickSoundFont = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri?.let { player.openSoundFont(it) }
    }
    val pickSong = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        uri?.let(player::openSong)
    }
    Box(Modifier.fillMaxSize().safeDrawingPadding().verticalScroll(rememberScrollState()), Alignment.TopCenter) {
        Column(
            Modifier.widthIn(max = 560.dp).padding(16.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(20.dp),
        ) {
            NowPlaying(player)
            Progress(player)
            Transport(player)
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                // SF2 files have no MIME type of their own.
                OutlinedButton({ pickSoundFont.launch(arrayOf("*/*")) }, Modifier.weight(1f)) {
                    Icon(painterResource(R.drawable.ic_waveform), null)
                    Spacer(Modifier.width(8.dp))
                    Text("SF2")
                }
                OutlinedButton(
                    { pickSong.launch(arrayOf("audio/midi", "audio/x-midi", "audio/sp-midi", "audio/mid")) },
                    Modifier.weight(1f),
                ) {
                    Icon(painterResource(R.drawable.ic_music_note), null)
                    Spacer(Modifier.width(8.dp))
                    Text("MIDI")
                }
            }
            player.error?.let {
                Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.fillMaxWidth())
            }
            Information(player)
        }
    }
}

@Composable
private fun NowPlaying(player: PlayerViewModel) {
    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(
            Modifier
                .size(160.dp)
                .clip(RoundedCornerShape(24.dp))
                .background(Brush.linearGradient(listOf(Color(0xFF594DF2), Color(0xFF9E38D9)))),
            Alignment.Center,
        ) {
            Icon(painterResource(R.drawable.ic_music_note), null, Modifier.size(80.dp), tint = Color.White)
        }
        Spacer(Modifier.size(4.dp))
        Text(
            player.song?.title ?: "No Song",
            style = MaterialTheme.typography.headlineSmall,
            fontWeight = FontWeight.Bold,
            textAlign = TextAlign.Center,
        )
        val song = player.song
        val font = player.soundFont
        Text(
            when {
                song != null && font != null -> "${song.fileName} · ${font.name}"
                song != null -> "${song.fileName} · choose an SF2"
                font != null -> "${font.name} · choose a MIDI file"
                else -> "Choose an SF2 and a MIDI file"
            },
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = TextAlign.Center,
        )
        if (player.loading > 0) CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
    }
}

@Composable
private fun Progress(player: PlayerViewModel) {
    // Where the slider is dragged to, until it is let go.
    var scrub by remember { mutableStateOf<Float?>(null) }
    val shown = scrub?.toDouble() ?: player.position
    Column {
        Slider(
            value = shown.toFloat(),
            onValueChange = { scrub = it },
            onValueChangeFinished = {
                scrub?.let { player.seek(it.toDouble()) }
                scrub = null
            },
            valueRange = 0f..player.duration.toFloat().coerceAtLeast(0.001f),
            enabled = player.song != null,
        )
        Row {
            TimeText(formatTime(shown))
            Spacer(Modifier.weight(1f))
            TimeText("-" + formatTime(player.duration - shown))
        }
    }
}

@Composable
private fun TimeText(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelMedium,
        fontFamily = FontFamily.Monospace,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun Transport(player: PlayerViewModel) {
    val playing = player.state == NativePlayer.PLAYING
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(32.dp)) {
            IconButton(player::stop, Modifier.size(56.dp), enabled = player.state != NativePlayer.STOPPED) {
                Icon(painterResource(R.drawable.ic_stop), "Stop", Modifier.size(32.dp))
            }
            FilledIconButton(
                player::toggle,
                Modifier.size(80.dp),
                enabled = player.canPlay,
                colors = IconButtonDefaults.filledIconButtonColors(),
            ) {
                Icon(
                    painterResource(if (playing) R.drawable.ic_pause else R.drawable.ic_play),
                    if (playing) "Pause" else "Play",
                    Modifier.size(44.dp),
                )
            }
            // Balances the stop button.
            Spacer(Modifier.size(56.dp))
        }
        Text(
            when (player.state) {
                NativePlayer.PLAYING -> "Playing"
                NativePlayer.PAUSED -> "Paused"
                else -> "Stopped"
            },
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun Information(player: PlayerViewModel) {
    val context = LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        InfoCard("Song") {
            val song = player.song
            if (song == null) {
                Missing()
            } else {
                InfoRow("Title", song.title)
                InfoRow("File", song.fileName)
                InfoRow("Length", formatTime(song.duration))
                InfoRow("Format", "SMF ${song.format} · ${song.tracks} tracks")
                InfoRow(
                    "Tempo",
                    if (song.tempoChanges > 1) "%.1f BPM (%d changes)".format(Locale.ROOT, song.tempoBpm, song.tempoChanges)
                    else "%.1f BPM".format(Locale.ROOT, song.tempoBpm),
                )
                if (song.ticksPerQuarter > 0) InfoRow("Resolution", "${song.ticksPerQuarter} ticks / quarter")
                InfoRow("Notes", "${song.notes} on ${song.channels} channels")
            }
        }
        InfoCard("SoundFont") {
            val font = player.soundFont
            if (font == null) {
                Missing()
            } else {
                InfoRow("Name", font.name)
                InfoRow("File", font.fileName)
                InfoRow("Format", "${font.format} · ${Formatter.formatShortFileSize(context, font.fileSize)}")
                InfoRow("Contents", "${font.presets} presets · ${font.instruments} instruments · ${font.samples} samples")
                if (font.copyright.isNotEmpty()) InfoRow("Copyright", font.copyright)
                if (font.engineers.isNotEmpty()) InfoRow("Engineers", font.engineers)
                if (font.comment.isNotEmpty()) InfoRow("Comment", font.comment)
            }
        }
        InfoCard("Audio") {
            val audio = player.audio
            if (audio.device.isNotEmpty()) InfoRow("Output", audio.device)
            InfoRow("Format", "%.1f kHz · stereo · 32-bit float".format(Locale.ROOT, audio.sampleRate / 1000.0))
            if (audio.bufferFrames > 0) {
                InfoRow(
                    "Buffer",
                    "%d frames (%.1f ms)".format(Locale.ROOT, audio.bufferFrames, audio.bufferFrames * 1000.0 / audio.sampleRate),
                )
            }
            InfoRow("Voices", "${player.activeVoices} playing")
            InfoRow("Engine", "sf2synth")
        }
    }
}

@Composable
private fun InfoCard(title: String, content: @Composable () -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
            content()
        }
    }
}

@Composable
private fun InfoRow(label: String, value: String) {
    Row {
        Text(
            label,
            Modifier.width(96.dp),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(value, style = MaterialTheme.typography.bodyMedium)
    }
}

@Composable
private fun Missing() {
    Text("Not loaded", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** "1:05" or "1:02:05". */
fun formatTime(seconds: Double): String {
    val total = seconds.coerceAtLeast(0.0).toLong()
    val (h, m, s) = Triple(total / 3600, total / 60 % 60, total % 60)
    return if (h > 0) "%d:%02d:%02d".format(Locale.ROOT, h, m, s) else "%d:%02d".format(Locale.ROOT, m, s)
}
