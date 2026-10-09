import AVFoundation

/// sf2synth as a MIDI instrument node of an `AVAudioEngine`: attach it,
/// connect it, and send it MIDI like any `AVAudioUnitMIDIInstrument`
/// (`startNote`, `sendMIDIEvent`, `sendMIDISysExEvent`, an
/// `AVAudioSequencer` track's destination…).
///
/// ```swift
/// let synth = try SF2SynthInstrument(soundFont: url)
/// engine.attach(synth)
/// engine.connect(synth, to: engine.mainMixerNode, format: nil)
/// ```
public final class SF2SynthInstrument: AVAudioUnitMIDIInstrument, @unchecked Sendable {
    /// The Audio Unit inside, for its settings (``SF2SynthAudioUnit/masterGain``…).
    public var synth: SF2SynthAudioUnit {
        auAudioUnit as! SF2SynthAudioUnit
    }

    /// An instrument rendering at `sampleRate` (best the output's, so
    /// nothing is converted), playing `soundFont` if given (see
    /// ``SF2SynthAudioUnit/loadSoundFont(at:mapped:)``).
    public init(soundFont: URL? = nil, mapped: Bool = true, sampleRate: Double = 48_000) throws {
        SF2SynthAudioUnit.register()
        super.init(audioComponentDescription: SF2SynthAudioUnit.componentDescription)
        if let format = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: 2) {
            for bus in 0..<auAudioUnit.outputBusses.count {
                try auAudioUnit.outputBusses[bus].setFormat(format)
            }
        }
        if let soundFont {
            try synth.loadSoundFont(at: soundFont, mapped: mapped)
        }
    }

    /// Reads a SoundFont to play from now on (see
    /// ``SF2SynthAudioUnit/loadSoundFont(at:mapped:)``).
    public func loadSoundFont(at url: URL, mapped: Bool = true) throws {
        try synth.loadSoundFont(at: url, mapped: mapped)
    }
}
