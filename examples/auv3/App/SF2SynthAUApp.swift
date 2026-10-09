import AVFoundation
import SF2Synth
import SwiftUI

/// The containing app of the Audio Unit extension. Hosts find the extension
/// once the app has been installed (on macOS, opened once). The app also
/// plays the same synthesizer itself, to try a SoundFont.
@main
struct SF2SynthAUApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}

struct ContentView: View {
    @StateObject private var player = Player()

    var body: some View {
        VStack(spacing: 0) {
            Text("The sf2synth instrument is now available to Audio Unit hosts such as GarageBand and Logic. Try it here first:")
                .font(.callout)
                .foregroundStyle(.secondary)
                .padding()
            if let synth = player.synth {
                SF2SynthView(audioUnit: synth.synth)
            }
            Keyboard(player: player)
                .frame(height: 120)
                .padding()
        }
        #if os(macOS)
        .frame(minWidth: 420, minHeight: 480)
        #endif
    }
}

/// An octave of keys from middle C.
struct Keyboard: View {
    let player: Player
    private let keys: [UInt8] = [60, 62, 64, 65, 67, 69, 71, 72]

    var body: some View {
        HStack(spacing: 2) {
            ForEach(keys, id: \.self) { key in
                RoundedRectangle(cornerRadius: 4)
                    .fill(.white)
                    .overlay(RoundedRectangle(cornerRadius: 4).stroke(.gray))
                    .gesture(DragGesture(minimumDistance: 0)
                        .onChanged { _ in player.press(key) }
                        .onEnded { _ in player.release(key) })
            }
        }
    }
}

@MainActor
final class Player: ObservableObject {
    private let engine = AVAudioEngine()
    @Published private(set) var synth: SF2SynthInstrument?
    private var down = Set<UInt8>()

    init() {
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setCategory(.playback)
        try? AVAudioSession.sharedInstance().setActive(true)
        #endif
        let rate = engine.outputNode.outputFormat(forBus: 0).sampleRate
        guard let synth = try? SF2SynthInstrument(sampleRate: rate > 0 ? rate : 48_000) else { return }
        engine.attach(synth)
        engine.connect(synth, to: engine.mainMixerNode, fromBus: 0, toBus: 0, format: nil)
        try? engine.start()
        self.synth = synth
    }

    func press(_ key: UInt8) {
        guard down.insert(key).inserted else { return }
        synth?.startNote(key, withVelocity: 100, onChannel: 0)
    }

    func release(_ key: UInt8) {
        down.remove(key)
        synth?.stopNote(key, onChannel: 0)
    }
}
