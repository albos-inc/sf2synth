import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    // Declared in Info.plist (UTImportedTypeDeclarations).
    static let soundFont = UTType(importedAs: "jp.albos.sf2player.sf2", conformingTo: .data)
    static let dls = UTType(importedAs: "jp.albos.sf2player.dls", conformingTo: .data)
}

struct ContentView: View {
    @Environment(Player.self) private var player
    @State private var importing = false
    @State private var picking = Picking.soundFont
    /// Where the progress bar is dragged to, until it is let go.
    @State private var scrub: Double?

    enum Picking { case soundFont, song }

    var body: some View {
        ScrollView {
            VStack(spacing: 24) {
                nowPlaying
                progress
                transport
                files
                if let error = player.error {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .font(.callout)
                        .foregroundStyle(.red)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                information
            }
            .padding()
            .frame(maxWidth: 560)
            .frame(maxWidth: .infinity)
        }
        .fileImporter(
            isPresented: $importing,
            allowedContentTypes: picking == .song ? [.midi] : [.soundFont, .dls]
        ) { result in
            guard case .success(let url) = result else { return }
            if picking == .song { player.openSong(url) } else { player.openSoundFont(url) }
        }
        .dropDestination(for: URL.self) { urls, _ in
            for url in urls { open(url) }
            return !urls.isEmpty
        }
        .onOpenURL { open($0) }
    }

    private func open(_ url: URL) {
        switch url.pathExtension.lowercased() {
        case "sf2", "dls": player.openSoundFont(url)
        default: player.openSong(url)
        }
    }

    // MARK: - Now playing

    private var nowPlaying: some View {
        VStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 24)
                .fill(.linearGradient(colors: [.indigo, .purple], startPoint: .topLeading, endPoint: .bottomTrailing))
                .frame(width: 160, height: 160)
                .overlay {
                    Image(systemName: player.state == .playing ? "music.note.list" : "music.note")
                        .font(.system(size: 64, weight: .medium))
                        .foregroundStyle(.white)
                        .contentTransition(.symbolEffect(.replace))
                }
                .shadow(radius: 8, y: 4)
            Text(player.song?.title ?? "No Song")
                .font(.title2.bold())
                .multilineTextAlignment(.center)
            Text(subtitle)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            if player.loading > 0 {
                ProgressView().controlSize(.small)
            }
        }
    }

    private var subtitle: String {
        switch (player.song, player.soundFont) {
        case let (song?, font?): "\(song.fileName) · \(font.name)"
        case let (song?, nil): "\(song.fileName) · choose an SF2"
        case let (nil, font?): "\(font.name) · choose a MIDI file"
        case (nil, nil): "Choose an SF2 and a MIDI file"
        }
    }

    // MARK: - Progress and transport

    private var progress: some View {
        let shown = scrub ?? player.position
        return VStack(spacing: 4) {
            Slider(
                value: Binding(get: { shown }, set: { scrub = $0 }),
                in: 0...max(player.duration, 0.001)
            ) { editing in
                if !editing, let to = scrub {
                    player.seek(to: to)
                    scrub = nil
                }
            }
            .disabled(player.song == nil)
            HStack {
                Text(formatTime(shown))
                Spacer()
                Text("-" + formatTime(player.duration - shown))
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
        }
    }

    private var transport: some View {
        VStack(spacing: 6) {
            HStack(spacing: 40) {
                Button {
                    player.stop()
                } label: {
                    Image(systemName: "stop.fill").font(.title).frame(width: 44, height: 44)
                }
                .disabled(player.state == .stopped)
                .help("Stop")

                Button {
                    player.toggle()
                } label: {
                    Image(systemName: player.state == .playing ? "pause.circle.fill" : "play.circle.fill")
                        .font(.system(size: 64))
                        .contentTransition(.symbolEffect(.replace))
                }
                .disabled(!player.canPlay)
                .keyboardShortcut(.space, modifiers: [])
                .help(player.state == .playing ? "Pause" : "Play")

                // Balances the stop button.
                Color.clear.frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .foregroundStyle(.tint)
            Text(stateLabel)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var stateLabel: String {
        switch player.state {
        case .stopped: "Stopped"
        case .playing: "Playing"
        case .paused: "Paused"
        }
    }

    private var files: some View {
        HStack {
            Button {
                picking = .soundFont
                importing = true
            } label: {
                Label("SF2", systemImage: "waveform").frame(maxWidth: .infinity)
            }
            Button {
                picking = .song
                importing = true
            } label: {
                Label("MIDI", systemImage: "music.note").frame(maxWidth: .infinity)
            }
        }
        .buttonStyle(.bordered)
        .controlSize(.large)
    }

    // MARK: - Information

    private var information: some View {
        VStack(spacing: 16) {
            InfoSection(title: "Song") {
                if let song = player.song {
                    InfoRow("Title", song.title)
                    InfoRow("File", song.fileName)
                    InfoRow("Length", formatTime(song.duration))
                    InfoRow("Format", "SMF \(song.format) · \(song.tracks) tracks"
                        + (song.standard.isEmpty ? "" : " · \(song.standard)"))
                    InfoRow("Tempo", song.tempoChanges > 1
                        ? String(format: "%.1f BPM (%d changes)", song.tempoBpm, song.tempoChanges)
                        : String(format: "%.1f BPM", song.tempoBpm))
                    if song.ticksPerQuarter > 0 {
                        InfoRow("Resolution", "\(song.ticksPerQuarter) ticks / quarter")
                    }
                    InfoRow("Notes", "\(song.notes) on \(song.channels) channels")
                } else {
                    Text("Not loaded").font(.callout).foregroundStyle(.secondary)
                }
            }
            InfoSection(title: "SoundFont") {
                if let font = player.soundFont {
                    InfoRow("Name", font.name)
                    InfoRow("File", font.fileName)
                    InfoRow("Format", "\(font.format) · "
                        + ByteCountFormatter.string(fromByteCount: Int64(font.fileSize), countStyle: .file))
                    InfoRow("Contents", "\(font.presets) presets · \(font.instruments) instruments · \(font.samples) samples")
                    if !font.copyright.isEmpty { InfoRow("Copyright", font.copyright) }
                    if !font.engineers.isEmpty { InfoRow("Engineers", font.engineers) }
                    if !font.comment.isEmpty { InfoRow("Comment", font.comment) }
                } else {
                    Text("Not loaded").font(.callout).foregroundStyle(.secondary)
                }
            }
            InfoSection(title: "Audio") {
                let audio = player.audio
                if !audio.device.isEmpty { InfoRow("Output", audio.device) }
                InfoRow("Synthesizer", String(format: "%.1f kHz · stereo · 32-bit float", audio.sampleRate / 1000))
                if audio.outputSampleRate > 0 {
                    InfoRow("Device", String(format: "%.1f kHz · %d ch", audio.outputSampleRate / 1000, audio.channels))
                }
                if audio.bufferFrames > 0 {
                    InfoRow("Buffer", String(format: "%d frames (%.1f ms)", audio.bufferFrames,
                                             Double(audio.bufferFrames) * 1000 / audio.sampleRate))
                }
                if audio.latency > 0 {
                    InfoRow("Latency", String(format: "%.1f ms", audio.latency * 1000))
                }
                InfoRow("Voices", "\(player.activeVoices) playing")
                InfoRow("Engine", "sf2synth")
            }
        }
    }
}

private struct InfoSection<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.headline)
            Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 16, verticalSpacing: 4) {
                content
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 12))
    }
}

private struct InfoRow: View {
    let label: String
    let value: String

    init(_ label: String, _ value: String) {
        self.label = label
        self.value = value
    }

    var body: some View {
        GridRow {
            Text(label)
                .foregroundStyle(.secondary)
                .frame(minWidth: 90, alignment: .leading)
            Text(value)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(.callout)
    }
}

/// "1:05" or "1:02:05".
func formatTime(_ seconds: Double) -> String {
    let total = Int(max(seconds, 0))
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60)
    return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%d:%02d", m, s)
}
