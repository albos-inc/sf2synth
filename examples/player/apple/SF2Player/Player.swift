import AVFoundation
import MediaPlayer
import Observation
#if os(macOS)
import CoreAudio
#endif

/// The Rust player (`sf2player.h`) wrapped for SwiftUI, playing through
/// AVAudioEngine.
@MainActor @Observable
final class Player {
    enum State { case stopped, playing, paused }

    private(set) var state = State.stopped
    private(set) var canPlay = false
    private(set) var position = 0.0
    private(set) var duration = 0.0
    private(set) var activeVoices = 0
    private(set) var soundFont: SoundFontSummary?
    private(set) var song: SongSummary?
    private(set) var loading = 0
    private(set) var audio = AudioInfo()
    var error: String?

    private let handle: Handle
    private let engine = AVAudioEngine()
    private let stats = RenderStats()
    private var source: AVAudioSourceNode!
    private var idleSince: Date?
    /// Play the song once the SF2 it waits for is loaded.
    private var autoplay = false

    init() {
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try? session.setCategory(.playback)
        try? session.setActive(true)
        #endif
        var sampleRate = engine.outputNode.outputFormat(forBus: 0).sampleRate
        if sampleRate <= 0 { sampleRate = 48_000 }
        handle = Handle(raw: sf2player_new(UInt32(sampleRate)))
        audio.sampleRate = sampleRate

        let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2)!
        source = Self.makeSource(format: format, player: handle, stats: stats)
        engine.attach(source)
        engine.connect(source, to: engine.mainMixerNode, format: format)
        engine.prepare()

        observeSystem()
        setUpRemoteCommands()
        restoreSoundFont()
        Task { [weak self] in
            while self != nil {
                self?.poll()
                try? await Task.sleep(for: .milliseconds(50))
            }
        }
    }

    /// The audio callback: the Rust player renders straight into the
    /// engine's buffers. Made outside the main actor, as it runs on the
    /// audio thread.
    nonisolated private static func makeSource(
        format: AVAudioFormat, player: Handle, stats: RenderStats
    ) -> AVAudioSourceNode {
        AVAudioSourceNode(format: format) { @Sendable _, _, frameCount, bufferList in
            let buffers = UnsafeMutableAudioBufferListPointer(bufferList)
            guard buffers.count >= 2,
                  let left = buffers[0].mData?.assumingMemoryBound(to: Float.self),
                  let right = buffers[1].mData?.assumingMemoryBound(to: Float.self)
            else { return noErr }
            sf2player_render(player.raw, left, right, Int(frameCount))
            stats.frames.pointee = frameCount
            return noErr
        }
    }

    // MARK: - Transport

    func play() {
        guard canPlay else { return }
        startEngine()
        sf2player_play(handle.raw)
        poll()
    }

    func pause() {
        sf2player_pause(handle.raw)
        poll()
    }

    func toggle() {
        state == .playing ? pause() : play()
    }

    func stop() {
        sf2player_stop(handle.raw)
        poll()
    }

    func seek(to seconds: Double) {
        sf2player_seek(handle.raw, seconds)
        poll()
    }

    // MARK: - Files

    func openSoundFont(_ url: URL) {
        load(url, remember: true) { handle, path in sf2player_load_sound_font(handle.raw, path) }
    }

    func openSong(_ url: URL) {
        autoplay = true
        load(url, remember: false) { handle, path in sf2player_load_midi(handle.raw, path) }
    }

    /// Parses the file off the main thread (an SF2 can be hundreds of MB).
    private func load(
        _ url: URL, remember: Bool,
        _ call: @escaping @Sendable (Handle, UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>?
    ) {
        loading += 1
        let handle = handle
        Task {
            let message: String? = await Task.detached {
                let accessing = url.startAccessingSecurityScopedResource()
                defer { if accessing { url.stopAccessingSecurityScopedResource() } }
                guard let error = url.path.withCString({ call(handle, $0) }) else { return nil }
                defer { sf2player_string_free(error) }
                return "\(url.lastPathComponent): \(String(cString: error))"
            }.value
            loading -= 1
            error = message
            if message == nil, remember { rememberSoundFont(url) }
            refreshInfo()
            if autoplay, loading == 0, canPlay {
                autoplay = false
                play()
            }
        }
    }

    /// The last SF2 chosen, else (on macOS) the system's General MIDI set.
    private func restoreSoundFont() {
        if let data = UserDefaults.standard.data(forKey: "soundFont") {
            var stale = false
            if let url = try? URL(resolvingBookmarkData: data, options: bookmarkOptions, bookmarkDataIsStale: &stale) {
                openSoundFont(url)
                return
            }
        }
        #if os(macOS)
        let builtIn = URL(fileURLWithPath:
            "/System/Library/Components/CoreAudio.component/Contents/Resources/gs_instruments.dls")
        if FileManager.default.isReadableFile(atPath: builtIn.path) {
            load(builtIn, remember: false) { handle, path in sf2player_load_sound_font(handle.raw, path) }
        }
        #endif
    }

    private func rememberSoundFont(_ url: URL) {
        let accessing = url.startAccessingSecurityScopedResource()
        defer { if accessing { url.stopAccessingSecurityScopedResource() } }
        if let data = try? url.bookmarkData(options: bookmarkCreationOptions) {
            UserDefaults.standard.set(data, forKey: "soundFont")
        }
    }

    #if os(macOS)
    private let bookmarkOptions: URL.BookmarkResolutionOptions = .withSecurityScope
    private let bookmarkCreationOptions: URL.BookmarkCreationOptions = .withSecurityScope
    #else
    private let bookmarkOptions: URL.BookmarkResolutionOptions = []
    private let bookmarkCreationOptions: URL.BookmarkCreationOptions = .minimalBookmark
    #endif

    // MARK: - State

    private func refreshInfo() {
        guard let json = sf2player_info_json(handle.raw) else { return }
        defer { sf2player_string_free(json) }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let info = try? decoder.decode(Info.self, from: Data(String(cString: json).utf8))
        soundFont = info?.soundFont
        song = info?.song
        duration = sf2player_duration(handle.raw)
        canPlay = sf2player_can_play(handle.raw)
        updateNowPlaying()
    }

    private func poll() {
        let newState: State = switch sf2player_state(handle.raw) {
        case SF2PLAYER_PLAYING: .playing
        case SF2PLAYER_PAUSED: .paused
        default: .stopped
        }
        position = sf2player_position(handle.raw)
        activeVoices = Int(sf2player_active_voices(handle.raw))
        audio.bufferFrames = Int(stats.frames.pointee)
        if newState != state {
            state = newState
            updateNowPlaying()
        }
        // Let the engine sleep a while after the music stops (and the last
        // notes have rung out), so an idle app costs no power.
        if state == .playing || activeVoices > 0 {
            idleSince = nil
        } else if engine.isRunning {
            if let since = idleSince {
                if Date().timeIntervalSince(since) > 3 { engine.pause() }
            } else {
                idleSince = Date()
            }
        }
    }

    private func startEngine() {
        guard !engine.isRunning else { return }
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(true)
        #endif
        do {
            try engine.start()
        } catch {
            self.error = "Audio output: \(error.localizedDescription)"
        }
        updateAudioInfo()
    }

    private func updateAudioInfo() {
        let output = engine.outputNode.outputFormat(forBus: 0)
        audio.outputSampleRate = output.sampleRate
        audio.channels = Int(output.channelCount)
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        audio.device = session.currentRoute.outputs.map(\.portName).joined(separator: ", ")
        audio.latency = session.outputLatency + session.ioBufferDuration
        #else
        audio.device = defaultOutputDeviceName() ?? ""
        audio.latency = 0
        #endif
    }

    // MARK: - System

    private func observeSystem() {
        let center = NotificationCenter.default
        center.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                // The output changed (e.g. headphones): restart on it.
                guard let self else { return }
                if self.state == .playing { self.engine.stop(); self.startEngine() }
                self.updateAudioInfo()
            }
        }
        #if os(iOS)
        center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
            let began = (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt)
                == AVAudioSession.InterruptionType.began.rawValue
            MainActor.assumeIsolated { if began { self?.pause() } }
        }
        center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] note in
            // Headphones unplugged: pause rather than play out loud.
            let lost = (note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt)
                == AVAudioSession.RouteChangeReason.oldDeviceUnavailable.rawValue
            MainActor.assumeIsolated {
                if lost { self?.pause() }
                self?.updateAudioInfo()
            }
        }
        #endif
    }

    /// The lock screen, Control Center and media keys.
    private func setUpRemoteCommands() {
        let commands = MPRemoteCommandCenter.shared()
        commands.playCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.play() }
            return .success
        }
        commands.pauseCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.pause() }
            return .success
        }
        commands.togglePlayPauseCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.toggle() }
            return .success
        }
        commands.stopCommand.addTarget { [weak self] _ in
            MainActor.assumeIsolated { self?.stop() }
            return .success
        }
        commands.changePlaybackPositionCommand.addTarget { [weak self] event in
            guard let event = event as? MPChangePlaybackPositionCommandEvent else { return .commandFailed }
            MainActor.assumeIsolated { self?.seek(to: event.positionTime) }
            return .success
        }
    }

    private func updateNowPlaying() {
        let center = MPNowPlayingInfoCenter.default()
        guard let song else {
            center.nowPlayingInfo = nil
            return
        }
        center.nowPlayingInfo = [
            MPMediaItemPropertyTitle: song.title,
            MPMediaItemPropertyArtist: soundFont?.name ?? "",
            MPMediaItemPropertyPlaybackDuration: duration,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: position,
            MPNowPlayingInfoPropertyPlaybackRate: state == .playing ? 1.0 : 0.0,
        ]
        #if os(macOS)
        center.playbackState = switch state {
        case .playing: .playing
        case .paused: .paused
        case .stopped: .stopped
        }
        #endif
    }
}

/// The `Sf2Player *`, which is safe to use from any thread.
struct Handle: @unchecked Sendable {
    let raw: OpaquePointer
}

/// Written by the audio callback, read by the UI.
final class RenderStats: @unchecked Sendable {
    let frames = UnsafeMutablePointer<UInt32>.allocate(capacity: 1)
    init() { frames.initialize(to: 0) }
    deinit { frames.deallocate() }
}

struct AudioInfo {
    /// The rate the synthesizer renders at.
    var sampleRate = 0.0
    var outputSampleRate = 0.0
    var channels = 0
    /// Frames in the latest render callback.
    var bufferFrames = 0
    var latency: TimeInterval = 0
    var device = ""
}

/// `sf2player_info_json`; the fields are those of sf2player's summaries.
private struct Info: Decodable {
    var soundFont: SoundFontSummary?
    var song: SongSummary?
}

struct SoundFontSummary: Decodable, Equatable {
    var fileName: String
    var name: String
    var format: String
    var fileSize: UInt64
    var presets: Int
    var instruments: Int
    var samples: Int
    var copyright: String
    var engineers: String
    var comment: String
}

struct SongSummary: Decodable, Equatable {
    var fileName: String
    var title: String
    var format: Int
    var tracks: Int
    var ticksPerQuarter: Int
    var tempoBpm: Double
    var tempoChanges: Int
    var notes: Int
    var channels: Int
    var duration: Double
}

#if os(macOS)
private func defaultOutputDeviceName() -> String? {
    var device = AudioObjectID(kAudioObjectUnknown)
    var size = UInt32(MemoryLayout<AudioObjectID>.size)
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDefaultOutputDevice,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain)
    guard AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device) == noErr
    else { return nil }
    var name: Unmanaged<CFString>?
    size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
    address.mSelector = kAudioObjectPropertyName
    guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, &name) == noErr, let name else { return nil }
    return name.takeRetainedValue() as String
}
#endif
