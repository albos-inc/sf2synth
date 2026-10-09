import AudioToolbox
import CoreAudioKit
import SwiftUI
import UniformTypeIdentifiers

/// The principal class of an Audio Unit extension that offers sf2synth to
/// hosts such as GarageBand and Logic: it creates ``SF2SynthAudioUnit``s and
/// shows ``SF2SynthView`` for them.
///
/// Subclass it in the extension (the principal class must belong to the
/// extension's module) and name the subclass in its Info.plist as both
/// `NSExtensionPrincipalClass` and the component's `factoryFunction`:
///
/// ```swift
/// final class AudioUnitViewController: SF2SynthViewController {}
/// ```
///
/// See `examples/auv3` for a complete app with its extension.
open class SF2SynthViewController: AUViewController, AUAudioUnitFactory {
    private var audioUnit: SF2SynthAudioUnit?
    private var hosted: PlatformViewController?

    public func createAudioUnit(with componentDescription: AudioComponentDescription) throws -> AUAudioUnit {
        let unit = try SF2SynthAudioUnit(componentDescription: componentDescription, options: [])
        audioUnit = unit
        DispatchQueue.main.async { [weak self] in
            self?.showView()
        }
        return unit
    }

    override open func viewDidLoad() {
        super.viewDidLoad()
        showView()
    }

    private func showView() {
        guard let audioUnit, hosted == nil, isViewLoaded else { return }
        let controller = PlatformHostingController(rootView: SF2SynthView(audioUnit: audioUnit))
        addChild(controller)
        controller.view.frame = view.bounds
        #if os(macOS)
        controller.view.autoresizingMask = [.width, .height]
        #else
        controller.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        #endif
        view.addSubview(controller.view)
        hosted = controller
        preferredContentSize = CGSize(width: 400, height: 300)
    }
}

#if os(macOS)
private typealias PlatformViewController = NSViewController
private typealias PlatformHostingController = NSHostingController
#else
private typealias PlatformViewController = UIViewController
private typealias PlatformHostingController = UIHostingController
#endif

/// Settings of an ``SF2SynthAudioUnit``: its SoundFont (chosen with a file
/// picker), master gain, reverb and chorus. Follows changes the host makes
/// to the parameters.
public struct SF2SynthView: View {
    @StateObject private var model: Model
    @State private var choosing = false

    public init(audioUnit: SF2SynthAudioUnit) {
        _model = StateObject(wrappedValue: Model(audioUnit: audioUnit))
    }

    public var body: some View {
        Form {
            Section("SoundFont") {
                HStack {
                    Text(model.soundFontName ?? "None")
                        .foregroundStyle(model.soundFontName == nil ? .secondary : .primary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Button("Choose…") { choosing = true }
                }
                if let error = model.error {
                    Text(error).font(.caption).foregroundStyle(.red)
                }
            }
            Section("Output") {
                LabeledContent("Gain") {
                    HStack {
                        Slider(value: $model.gain, in: 0...2)
                        Text(String(format: "%.2f", model.gain))
                            .monospacedDigit()
                            .frame(width: 40, alignment: .trailing)
                    }
                }
                Toggle("Reverb", isOn: $model.reverb)
                Toggle("Chorus", isOn: $model.chorus)
                Picker("Quality", selection: $model.interpolation) {
                    Text("Standard").tag(SF2SynthAudioUnit.Interpolation.cubic)
                    Text("High").tag(SF2SynthAudioUnit.Interpolation.sinc)
                }
            }
        }
        .formStyle(.grouped)
        .fileImporter(isPresented: $choosing, allowedContentTypes: Self.soundFontTypes) { result in
            if case .success(let url) = result {
                model.load(url)
            }
        }
    }

    private static let soundFontTypes: [UTType] =
        ["sf2", "dls"].compactMap { UTType(filenameExtension: $0) } + [.data]

    @MainActor
    final class Model: ObservableObject {
        private let audioUnit: SF2SynthAudioUnit
        private var observer: AUParameterObserverToken?
        @Published var soundFontName: String?
        @Published var error: String?
        @Published var gain: Float { didSet { audioUnit.masterGain = gain } }
        @Published var reverb: Bool { didSet { audioUnit.isReverbEnabled = reverb } }
        @Published var chorus: Bool { didSet { audioUnit.isChorusEnabled = chorus } }
        @Published var interpolation: SF2SynthAudioUnit.Interpolation {
            didSet { audioUnit.interpolation = interpolation }
        }

        init(audioUnit: SF2SynthAudioUnit) {
            self.audioUnit = audioUnit
            soundFontName = audioUnit.soundFontURL?.lastPathComponent
            gain = audioUnit.masterGain
            reverb = audioUnit.isReverbEnabled
            chorus = audioUnit.isChorusEnabled
            interpolation = audioUnit.interpolation == .linear ? .cubic : audioUnit.interpolation
            observer = audioUnit.parameterTree?.token(byAddingParameterObserver: { [weak self] address, value in
                DispatchQueue.main.async { self?.update(address, value) }
            })
        }

        deinit {
            if let observer {
                audioUnit.parameterTree?.removeParameterObserver(observer)
            }
        }

        private func update(_ address: AUParameterAddress, _ value: AUValue) {
            switch SF2SynthAudioUnit.ParameterAddress(rawValue: address) {
            case .masterGain where gain != value: gain = value
            case .reverb where reverb != (value >= 0.5): reverb = value >= 0.5
            case .chorus where chorus != (value >= 0.5): chorus = value >= 0.5
            default: break
            }
        }

        func load(_ url: URL) {
            do {
                try audioUnit.loadSoundFont(at: url)
                soundFontName = url.lastPathComponent
                error = nil
            } catch {
                self.error = "\(error)"
            }
        }
    }
}
