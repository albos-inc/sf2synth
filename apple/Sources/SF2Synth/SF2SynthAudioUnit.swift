import AudioToolbox
import AVFoundation
import CoreMIDI
import os
import SF2SynthC

/// Why a SoundFont couldn't be loaded.
public struct SF2SynthError: Error, CustomStringConvertible {
    public let description: String
}

/// sf2synth as an in-process Audio Unit (a music device): MIDI events in,
/// stereo out. Each event is applied at its frame in the buffer.
///
/// Output bus 0 is the mix. Output bus 1 carries the reverb send (the
/// signal the channels send to the reverb with CC91, the same on both
/// channels): connect it to your own reverb and turn the built-in one off
/// with ``isReverbEnabled``. Bus 0 does the rendering and bus 1 hands out
/// the send of the latest rendering, so if a host pulls bus 1 first, the
/// send lags by one buffer.
///
/// MIDI arrives as MIDI 1.0 events (System Exclusive included, even when
/// split over several events) or as MIDI 2.0 event lists.
public final class SF2SynthAudioUnit: AUAudioUnit {
    public static let componentDescription = AudioComponentDescription(
        componentType: kAudioUnitType_MusicDevice,
        componentSubType: fourCharCode("sf2s"),
        componentManufacturer: fourCharCode("Albs"),
        componentFlags: 0,
        componentFlagsMask: 0
    )

    private static let registration: Void = {
        AUAudioUnit.registerSubclass(SF2SynthAudioUnit.self, as: componentDescription,
                                     name: "Albos: sf2synth", version: 1)
    }()

    /// Registers the Audio Unit in this process (done by
    /// ``SF2SynthInstrument``; call it before instantiating the component
    /// yourself).
    public static func register() {
        _ = registration
    }

    private static func fourCharCode(_ text: String) -> FourCharCode {
        text.utf8.reduce(0) { $0 << 8 | FourCharCode($1) }
    }

    /// Gain applied to the mix and the send. 1.0 is sf2synth's default; 1.2
    /// matches Apple's MIDI synth playing the same SoundFont.
    public var masterGain: Float {
        get { kernel.settings.withLock { $0.masterGain } }
        set { kernel.settings.withLock { $0.masterGain = newValue; $0.version &+= 1 } }
    }

    /// The built-in reverb (on by default). The reverb send (bus 1) is
    /// produced either way.
    public var isReverbEnabled: Bool {
        get { kernel.settings.withLock { $0.reverb } }
        set { kernel.settings.withLock { $0.reverb = newValue; $0.version &+= 1 } }
    }

    /// The built-in chorus (on by default).
    public var isChorusEnabled: Bool {
        get { kernel.settings.withLock { $0.chorus } }
        set { kernel.settings.withLock { $0.chorus = newValue; $0.version &+= 1 } }
    }

    /// Voices that can sound at once; takes effect at the next
    /// ``loadSoundFont(at:mapped:)`` or when render resources are allocated
    /// without a synth.
    public var maximumVoices: UInt32 = 256

    private let kernel = Kernel()
    private var mainBus: AUAudioUnitBus!
    private var sendBus: AUAudioUnitBus!
    private var busses: AUAudioUnitBusArray!

    override public init(componentDescription: AudioComponentDescription,
                         options: AudioComponentInstantiationOptions = []) throws {
        try super.init(componentDescription: componentDescription, options: options)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
        mainBus = try AUAudioUnitBus(format: format)
        sendBus = try AUAudioUnitBus(format: format)
        mainBus.maximumChannelCount = 2
        sendBus.maximumChannelCount = 2
        busses = AUAudioUnitBusArray(audioUnit: self, busType: .output, busses: [mainBus, sendBus])
        maximumFramesToRender = 4096
    }

    override public var outputBusses: AUAudioUnitBusArray { busses }

    override public func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved && (1...2).contains(format.channelCount)
    }

    /// Reads a SoundFont (SF2 or DLS) to play from now on. `mapped` maps an
    /// SF2 file into memory instead of reading it, so a large file costs no
    /// memory (it must not change while it plays). Can be called while
    /// rendering: the new synth takes over at the next buffer.
    public func loadSoundFont(at url: URL, mapped: Bool = true) throws {
        var error: UnsafeMutablePointer<CChar>?
        let font = url.withUnsafeFileSystemRepresentation { path -> OpaquePointer? in
            guard let path else { return nil }
            return mapped ? sf2synth_font_load_mapped(path, &error) : sf2synth_font_load(path, &error)
        }
        guard let font else {
            let message = error.map { String(cString: $0) } ?? "can't read \(url.path)"
            sf2synth_string_free(error)
            throw SF2SynthError(description: message)
        }
        kernel.replaceFont(font, sampleRate: renderResourcesAllocated ? mainBus.format.sampleRate : nil,
                           maximumVoices: maximumVoices)
    }

    override public func allocateRenderResources() throws {
        try super.allocateRenderResources()
        guard sendBus.format.sampleRate == mainBus.format.sampleRate else {
            throw NSError(domain: NSOSStatusErrorDomain, code: Int(kAudioUnitErr_FormatNotSupported))
        }
        kernel.allocate(frames: Int(maximumFramesToRender), sampleRate: mainBus.format.sampleRate,
                        maximumVoices: maximumVoices)
    }

    override public func deallocateRenderResources() {
        kernel.deallocate()
        super.deallocateRenderResources()
    }

    override public var internalRenderBlock: AUInternalRenderBlock {
        let kernel = self.kernel
        return { _, timestamp, frameCount, outputBusNumber, outputData, realtimeEventListHead, _ in
            kernel.render(timestamp: timestamp, frames: Int(frameCount), bus: outputBusNumber,
                          output: outputData, events: realtimeEventListHead)
        }
    }
}

/// What the render block uses. The synth itself is only touched by the
/// render thread while render resources are allocated; the UI hands over
/// settings and new synths under a lock the render thread only tries.
private final class Kernel {
    struct Settings {
        var masterGain: Float = 1
        var reverb = true
        var chorus = true
        var version = 0
    }

    let settings = OSAllocatedUnfairLock(initialState: Settings())

    /// An `Sf2Synth`, passed between threads (one at a time uses it).
    private struct SynthPointer: @unchecked Sendable {
        let pointer: OpaquePointer
    }

    /// A synth (and its font) waiting to be taken over by the render
    /// thread, and the one it replaced, to be freed off the render thread.
    private struct Handover {
        var incoming: SynthPointer?
        var retired: SynthPointer?
    }

    private let handover = OSAllocatedUnfairLock(initialState: Handover())

    private var font: OpaquePointer?
    private var synth: OpaquePointer?
    private var appliedVersion = -1
    private var left: UnsafeMutablePointer<Float>?
    private var right: UnsafeMutablePointer<Float>?
    private var send: UnsafeMutablePointer<Float>?
    /// The send for bus 1's second channel, when the host asks for buffers.
    private var sendCopy: UnsafeMutablePointer<Float>?
    private var capacity = 0
    private var renderedFrames = 0

    deinit {
        deallocate()
        sf2synth_free(synth)
        sf2synth_font_free(font)
    }

    /// Takes a loaded font. With a sample rate (rendering), a synth for it is
    /// handed to the render thread; else it's created at allocation.
    func replaceFont(_ newFont: OpaquePointer, sampleRate: Double?, maximumVoices: UInt32) {
        guard let sampleRate else {
            sf2synth_font_free(font)
            font = newFont
            sf2synth_free(synth)
            synth = nil
            return
        }
        let newSynth = makeSynth(font: newFont, sampleRate: sampleRate, maximumVoices: maximumVoices)
        sf2synth_font_free(newFont) // the synth keeps it
        guard let newSynth else { return }
        let handed = SynthPointer(pointer: newSynth)
        let replaced = handover.withLock { state -> [SynthPointer?] in
            let replaced = [state.incoming, state.retired]
            state.incoming = handed
            state.retired = nil
            return replaced
        }
        replaced.forEach { sf2synth_free($0?.pointer) }
    }

    private func makeSynth(font: OpaquePointer, sampleRate: Double, maximumVoices: UInt32) -> OpaquePointer? {
        var settings = sf2synth_settings_default(UInt32(sampleRate.rounded()))
        settings.max_voices = maximumVoices
        return sf2synth_new(font, &settings, nil)
    }

    func allocate(frames: Int, sampleRate: Double, maximumVoices: UInt32) {
        deallocate()
        left = .allocate(capacity: frames)
        right = .allocate(capacity: frames)
        send = .allocate(capacity: frames)
        sendCopy = .allocate(capacity: frames)
        send?.update(repeating: 0, count: frames)
        capacity = frames
        renderedFrames = 0
        appliedVersion = -1
        let rate = UInt32(sampleRate.rounded())
        if let synth, sf2synth_sample_rate(synth) != rate {
            sf2synth_set_sample_rate(synth, rate)
        } else if synth == nil, let font {
            synth = makeSynth(font: font, sampleRate: sampleRate, maximumVoices: maximumVoices)
        }
    }

    func deallocate() {
        // Not rendering now: take over a pending synth and free retired ones.
        let (incoming, retired) = handover.withLock { state in
            defer { state = Handover() }
            return (state.incoming, state.retired)
        }
        sf2synth_free(retired?.pointer)
        if let incoming {
            sf2synth_free(synth)
            synth = incoming.pointer
        }
        left?.deallocate()
        right?.deallocate()
        send?.deallocate()
        sendCopy?.deallocate()
        left = nil
        right = nil
        send = nil
        sendCopy = nil
        capacity = 0
    }

    func render(timestamp: UnsafePointer<AudioTimeStamp>, frames: Int, bus: Int,
                output: UnsafeMutablePointer<AudioBufferList>,
                events: UnsafePointer<AURenderEvent>?) -> AUAudioUnitStatus {
        guard let left, let right, let send, let sendCopy, frames <= capacity else {
            return kAudioUnitErr_TooManyFramesToProcess
        }
        // (No arrays here: they would allocate on the render thread.)
        let sources: (UnsafeMutablePointer<Float>, UnsafeMutablePointer<Float>)
        if bus == 0 {
            renderCycle(sampleTime: timestamp.pointee.mSampleTime, frames: frames, events: events)
            renderedFrames = frames
            sources = (left, right)
        } else {
            // The latest rendering's send (silence past its end).
            if frames > renderedFrames {
                (send + renderedFrames).update(repeating: 0, count: frames - renderedFrames)
            }
            sendCopy.update(from: send, count: frames)
            sources = (send, sendCopy)
        }
        let buffers = UnsafeMutableAudioBufferListPointer(output)
        for (index, buffer) in buffers.enumerated() {
            let source = index == 0 ? sources.0 : sources.1
            if let data = buffer.mData {
                data.assumingMemoryBound(to: Float.self).update(from: source, count: frames)
            } else {
                buffers[index].mData = UnsafeMutableRawPointer(source)
            }
            buffers[index].mDataByteSize = UInt32(frames * MemoryLayout<Float>.size)
        }
        return noErr
    }

    private func renderCycle(sampleTime: Double, frames: Int, events: UnsafePointer<AURenderEvent>?) {
        takeOverIfHanded()
        guard let synth, let left, let right, let send else {
            left?.update(repeating: 0, count: frames)
            right?.update(repeating: 0, count: frames)
            send?.update(repeating: 0, count: frames)
            return
        }
        applySettings(synth)
        var rendered = 0
        func render(upTo end: Int) {
            guard end > rendered else { return }
            sf2synth_render_events(synth, left + rendered, right + rendered, send + rendered, nil,
                                   end - rendered, nil, 0)
            rendered = end
        }
        let immediate = Int64(bitPattern: 0xffff_ffff_0000_0000)
        var event = events
        while let current = event {
            let head = current.pointee.head
            let time = head.eventSampleTime
            let offset = time >= immediate && time < 0 ? Int(time - immediate) : Int(Double(time) - sampleTime)
            render(upTo: min(max(offset, rendered), frames))
            switch head.eventType {
            case .MIDI, .midiSysEx:
                // A System Exclusive event's bytes run past `data`'s three:
                // read them in place.
                let bytes = (UnsafeRawPointer(current) + MemoryLayout<AUMIDIEvent>.offset(of: \.data)!)
                    .assumingMemoryBound(to: UInt8.self)
                sf2synth_midi_bytes(synth, bytes, Int(current.pointee.MIDI.length))
            case .midiEventList:
                // The list runs past the struct: read it in place.
                let list = (UnsafeRawPointer(current) + MemoryLayout<AUMIDIEventList>.offset(of: \.eventList)!)
                    .assumingMemoryBound(to: MIDIEventList.self)
                for packet in list.unsafeSequence() {
                    let words = UnsafeRawPointer(packet) + MemoryLayout<MIDIEventPacket>.offset(of: \.words)!
                    sf2synth_ump(synth, words.assumingMemoryBound(to: UInt32.self), Int(packet.pointee.wordCount))
                }
            default:
                break
            }
            event = UnsafePointer(head.next)
        }
        render(upTo: frames)
    }

    private func takeOverIfHanded() {
        let current = synth.map(SynthPointer.init)
        guard let incoming = handover.withLockIfAvailable({ state -> SynthPointer? in
            guard let incoming = state.incoming, state.retired == nil else { return nil }
            state.incoming = nil
            state.retired = current
            return incoming
        }) ?? nil else { return }
        synth = incoming.pointer
        appliedVersion = -1
    }

    private func applySettings(_ synth: OpaquePointer) {
        guard let current = settings.withLockIfAvailable({ $0 }), current.version != appliedVersion else { return }
        sf2synth_set_master_gain(synth, current.masterGain)
        sf2synth_set_reverb_enabled(synth, current.reverb)
        sf2synth_set_chorus_enabled(synth, current.chorus)
        appliedVersion = current.version
    }
}
