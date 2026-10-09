import AVFoundation
import SF2Synth
import XCTest

final class SF2SynthTests: XCTestCase {
    private func fontURL() throws -> URL {
        guard let path = ProcessInfo.processInfo.environment["SF2SYNTH_TEST_FONT"] else {
            throw XCTSkip("set SF2SYNTH_TEST_FONT to a General MIDI SoundFont")
        }
        return URL(fileURLWithPath: path)
    }

    /// An engine rendering offline: the synth into the main mixer, and its
    /// reverb send (bus 1) into a second mixer input.
    private func engine(with synth: SF2SynthInstrument) throws -> (AVAudioEngine, AVAudioMixerNode) {
        let engine = AVAudioEngine()
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let sendMixer = AVAudioMixerNode()
        engine.attach(synth)
        engine.attach(sendMixer)
        engine.connect(synth, to: engine.mainMixerNode, fromBus: 0, toBus: 0, format: format)
        engine.connect(synth, to: sendMixer, fromBus: 1, toBus: 0, format: format)
        engine.connect(sendMixer, to: engine.mainMixerNode, fromBus: 0, toBus: 1, format: format)
        try engine.start()
        return (engine, sendMixer)
    }

    private func render(_ engine: AVAudioEngine, frames: AVAudioFrameCount) throws -> Float {
        let buffer = AVAudioPCMBuffer(pcmFormat: engine.manualRenderingFormat, frameCapacity: frames)!
        let status = try engine.renderOffline(frames, to: buffer)
        XCTAssertEqual(status, .success)
        var peak: Float = 0
        for channel in 0..<Int(buffer.format.channelCount) {
            for i in 0..<Int(buffer.frameLength) {
                peak = max(peak, abs(buffer.floatChannelData![channel][i]))
            }
        }
        return peak
    }

    func testPlaysNotesAndSysEx() throws {
        let synth = try SF2SynthInstrument(soundFont: fontURL())
        synth.synth.isReverbEnabled = false
        synth.synth.isChorusEnabled = false
        let (engine, _) = try engine(with: synth)
        XCTAssertEqual(try render(engine, frames: 4096), 0)
        synth.startNote(60, withVelocity: 100, onChannel: 0)
        XCTAssertGreaterThan(try render(engine, frames: 4096), 0.01)
        // GS Reset silences.
        synth.sendMIDISysExEvent(Data([0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7]))
        _ = try render(engine, frames: 4096)
        XCTAssertLessThan(try render(engine, frames: 4096), 1e-4)
        // Master volume 0 (Universal SysEx) silences new notes.
        synth.sendMIDISysExEvent(Data([0xF0, 0x7F, 0x7F, 0x04, 0x01, 0x00, 0x00, 0xF7]))
        synth.startNote(64, withVelocity: 100, onChannel: 0)
        XCTAssertEqual(try render(engine, frames: 4096), 0)
        engine.stop()
    }

    func testSendBusCarriesTheSend() throws {
        let synth = try SF2SynthInstrument(soundFont: fontURL())
        let engine = AVAudioEngine()
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        engine.attach(synth)
        // The dry mix goes nowhere audible: a muted mixer.
        let muted = AVAudioMixerNode()
        engine.attach(muted)
        engine.connect(synth, to: muted, fromBus: 0, toBus: 0, format: format)
        engine.connect(muted, to: engine.mainMixerNode, fromBus: 0, toBus: 0, format: format)
        muted.outputVolume = 0
        engine.connect(synth, to: engine.mainMixerNode, fromBus: 1, toBus: 1, format: format)
        try engine.start()
        synth.startNote(60, withVelocity: 100, onChannel: 0)
        let low = try render(engine, frames: 4096)
        synth.sendController(120, withValue: 0, onChannel: 0)
        _ = try render(engine, frames: 4096)
        synth.sendController(91, withValue: 127, onChannel: 1)
        synth.startNote(60, withVelocity: 100, onChannel: 1)
        let high = try render(engine, frames: 4096)
        // GeneralUser GS's instruments send some reverb of their own.
        XCTAssertGreaterThan(high, low * 2)
        engine.stop()
    }

    /// The send straight into a reverb, connected first so that the engine
    /// pulls bus 1 before bus 0 (and passes the events with bus 1). Both
    /// connections of the reverb need the synth's format: with `nil`, its
    /// output stays at 44.1 kHz and the engine fails to start (-10868).
    func testReverbOnTheSendBus() throws {
        let synth = try SF2SynthInstrument(soundFont: fontURL())
        synth.synth.isReverbEnabled = false
        let engine = AVAudioEngine()
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4096)
        let reverb = AVAudioUnitReverb()
        engine.attach(synth)
        engine.attach(reverb)
        let synthFormat = synth.outputFormat(forBus: 1)
        engine.connect(synth, to: reverb, fromBus: 1, toBus: 0, format: synthFormat)
        engine.connect(reverb, to: engine.mainMixerNode, fromBus: 0, toBus: 0, format: synthFormat)
        engine.connect(synth, to: engine.mainMixerNode, fromBus: 0, toBus: 1, format: synthFormat)
        try engine.start()
        XCTAssertEqual(try render(engine, frames: 4096), 0)
        synth.startNote(60, withVelocity: 100, onChannel: 0)
        XCTAssertGreaterThan(try render(engine, frames: 4096), 0.01, "the note-on reached the synth")
        engine.stop()
    }

    func testLoadingWhileRendering() throws {
        let url = try fontURL()
        let synth = try SF2SynthInstrument(soundFont: url)
        let (engine, _) = try engine(with: synth)
        synth.startNote(60, withVelocity: 100, onChannel: 0)
        XCTAssertGreaterThan(try render(engine, frames: 4096), 0.01)
        try synth.loadSoundFont(at: url, mapped: false)
        // The new synth starts silent.
        XCTAssertEqual(try render(engine, frames: 4096), 0)
        synth.startNote(60, withVelocity: 100, onChannel: 0)
        XCTAssertGreaterThan(try render(engine, frames: 4096), 0.01)
        XCTAssertThrowsError(try synth.loadSoundFont(at: URL(fileURLWithPath: "/nonexistent.sf2")))
        engine.stop()
    }
}
