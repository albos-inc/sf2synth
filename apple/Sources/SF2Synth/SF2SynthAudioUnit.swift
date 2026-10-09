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
/// with ``isReverbEnabled``. Both busses come from one rendering per
/// render cycle, whichever the host pulls first.
///
/// MIDI arrives as MIDI 1.0 events (System Exclusive included, even when
/// split over several events) or as MIDI 2.0 event lists.
///
/// The master gain and the built-in reverb and chorus are parameters
/// (``ParameterAddress``) that hosts can show and automate. ``fullState``
/// keeps them and the SoundFont (as a bookmark), so a host's project
/// reopens with the same sound.
public final class SF2SynthAudioUnit: AUAudioUnit {
    /// The addresses of the unit's parameters.
    public enum ParameterAddress: AUParameterAddress {
        /// Gain applied to the mix, 0…2.
        case masterGain = 0
        /// The built-in reverb, 0 (off) or 1 (on).
        case reverb = 1
        /// The built-in chorus, 0 (off) or 1 (on).
        case chorus = 2
    }

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
        get { parameter(.masterGain).value }
        set { parameter(.masterGain).value = newValue }
    }

    /// The built-in reverb (on by default). The reverb send (bus 1) is
    /// produced either way.
    public var isReverbEnabled: Bool {
        get { parameter(.reverb).value >= 0.5 }
        set { parameter(.reverb).value = newValue ? 1 : 0 }
    }

    /// The built-in chorus (on by default).
    public var isChorusEnabled: Bool {
        get { parameter(.chorus).value >= 0.5 }
        set { parameter(.chorus).value = newValue ? 1 : 0 }
    }

    /// The SoundFont playing, if any.
    public private(set) var soundFontURL: URL?
    /// Whether ``soundFontURL`` was memory-mapped.
    public private(set) var isSoundFontMapped = true
    /// A security-scoped URL being accessed while its file plays.
    private var accessedURL: URL?

    /// How sample points are interpolated (real-time safe to change).
    public enum Interpolation: Int32, Sendable {
        case linear = 0
        /// The default.
        case cubic = 1
        /// Band-limited: no aliasing when samples play higher than
        /// recorded, at about 1.5–2× the rendering time.
        case sinc = 2
    }

    public var interpolation: Interpolation {
        get { kernel.settings.withLock { Interpolation(rawValue: $0.interpolation) ?? .cubic } }
        set { kernel.settings.withLock { $0.interpolation = newValue.rawValue; $0.version &+= 1 } }
    }

    /// The output limiter (on by default): keeps the mix below full scale
    /// without clipping, turning down only the passages that would go over.
    /// It delays the mix by 1 ms (``latency``); the reverb send (bus 1) is
    /// neither limited nor delayed.
    public var isLimiterEnabled: Bool {
        get { kernel.settings.withLock { $0.limiter } }
        set {
            willChangeValue(forKey: "latency")
            kernel.settings.withLock { $0.limiter = newValue; $0.version &+= 1 }
            didChangeValue(forKey: "latency")
        }
    }

    /// The limiter's look-ahead, so hosts can compensate for it.
    override public var latency: TimeInterval {
        guard isLimiterEnabled else { return 0 }
        let rate = mainBus.format.sampleRate
        return Double(sf2synth_limiter_latency(UInt32(rate.rounded()))) / rate
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
        parameterTree = Self.makeParameterTree(kernel: kernel)
    }

    deinit {
        accessedURL?.stopAccessingSecurityScopedResource()
    }

    private static func makeParameterTree(kernel: Kernel) -> AUParameterTree {
        let flags: AudioUnitParameterOptions = [.flag_IsReadable, .flag_IsWritable]
        func make(_ address: ParameterAddress, _ identifier: String, _ name: String, max: AUValue,
                  unit: AudioUnitParameterUnit, value: AUValue) -> AUParameter {
            let parameter = AUParameterTree.createParameter(
                withIdentifier: identifier, name: name, address: address.rawValue, min: 0, max: max,
                unit: unit, unitName: nil, flags: flags, valueStrings: nil, dependentParameters: nil)
            parameter.value = value
            return parameter
        }
        let tree = AUParameterTree.createTree(withChildren: [
            make(.masterGain, "masterGain", "Master Gain", max: 2, unit: .linearGain, value: 1),
            make(.reverb, "reverb", "Reverb", max: 1, unit: .boolean, value: 1),
            make(.chorus, "chorus", "Chorus", max: 1, unit: .boolean, value: 1),
        ])
        tree.implementorValueObserver = { parameter, value in
            kernel.settings.withLock { settings in
                switch ParameterAddress(rawValue: parameter.address) {
                case .masterGain: settings.masterGain = max(0, value)
                case .reverb: settings.reverb = value >= 0.5
                case .chorus: settings.chorus = value >= 0.5
                case nil: return
                }
                settings.version &+= 1
            }
        }
        tree.implementorValueProvider = { parameter in
            kernel.settings.withLock { settings in
                switch ParameterAddress(rawValue: parameter.address) {
                case .masterGain: settings.masterGain
                case .reverb: settings.reverb ? 1 : 0
                case .chorus: settings.chorus ? 1 : 0
                case nil: 0
                }
            }
        }
        return tree
    }

    private func parameter(_ address: ParameterAddress) -> AUParameter {
        parameterTree!.parameter(withAddress: address.rawValue)!
    }

    override public var outputBusses: AUAudioUnitBusArray { busses }

    /// No input; mono or stereo output (mono gets the left channel).
    override public var channelCapabilities: [NSNumber]? { [0, 1, 0, 2] }

    override public func shouldChange(to format: AVAudioFormat, for bus: AUAudioUnitBus) -> Bool {
        format.commonFormat == .pcmFormatFloat32 && !format.isInterleaved && (1...2).contains(format.channelCount)
    }

    /// Reads a SoundFont (SF2 or DLS) to play from now on. `mapped` maps an
    /// SF2 file into memory instead of reading it, so a large file costs no
    /// memory (it must not change while it plays). Can be called while
    /// rendering: the new synth takes over at the next buffer.
    ///
    /// A security-scoped URL (from a file picker or a bookmark) is accessed
    /// for as long as its file plays.
    public func loadSoundFont(at url: URL, mapped: Bool = true) throws {
        let accessing = url.startAccessingSecurityScopedResource()
        var error: UnsafeMutablePointer<CChar>?
        let font = url.withUnsafeFileSystemRepresentation { path -> OpaquePointer? in
            guard let path else { return nil }
            return mapped ? sf2synth_font_load_mapped(path, &error) : sf2synth_font_load(path, &error)
        }
        guard let font else {
            if accessing {
                url.stopAccessingSecurityScopedResource()
            }
            let message = error.map { String(cString: $0) } ?? "can't read \(url.path)"
            sf2synth_string_free(error)
            throw SF2SynthError(description: message)
        }
        kernel.replaceFont(font, sampleRate: renderResourcesAllocated ? mainBus.format.sampleRate : nil,
                           maximumVoices: maximumVoices)
        // A file read into memory no longer needs the access.
        if accessing && !mapped {
            url.stopAccessingSecurityScopedResource()
        }
        accessedURL?.stopAccessingSecurityScopedResource()
        accessedURL = accessing && mapped ? url : nil
        soundFontURL = url
        isSoundFontMapped = mapped
    }

    // MARK: - State

    private static let stateKey = "sf2synth"

    /// The host's saved state: the parameters, plus the SoundFont as a
    /// bookmark (and its path, for when the bookmark can't be resolved).
    override public var fullState: [String: Any]? {
        get {
            var state = super.fullState ?? [:]
            var synth: [String: Any] = [
                "masterGain": masterGain,
                "reverb": isReverbEnabled,
                "chorus": isChorusEnabled,
                "mapped": isSoundFontMapped,
                "interpolation": interpolation.rawValue,
                "limiter": isLimiterEnabled,
            ]
            if let url = soundFontURL {
                synth["soundFontPath"] = url.path
                synth["soundFontBookmark"] = Self.bookmark(for: url)
            }
            state[Self.stateKey] = synth
            return state
        }
        set {
            super.fullState = newValue
            guard let synth = newValue?[Self.stateKey] as? [String: Any] else { return }
            if let gain = synth["masterGain"] as? Float { masterGain = gain }
            if let reverb = synth["reverb"] as? Bool { isReverbEnabled = reverb }
            if let chorus = synth["chorus"] as? Bool { isChorusEnabled = chorus }
            if let limiter = synth["limiter"] as? Bool { isLimiterEnabled = limiter }
            if let value = synth["interpolation"] as? Int32, let interpolation = Interpolation(rawValue: value) {
                self.interpolation = interpolation
            }
            let mapped = synth["mapped"] as? Bool ?? true
            let bookmarked = (synth["soundFontBookmark"] as? Data).flatMap(Self.resolve(bookmark:))
            let path = (synth["soundFontPath"] as? String).map { URL(fileURLWithPath: $0) }
            for url in [bookmarked, path].compactMap({ $0 }) where url != soundFontURL {
                if (try? loadSoundFont(at: url, mapped: mapped)) != nil {
                    break
                }
            }
        }
    }

    private static func bookmark(for url: URL) -> Data? {
        #if os(macOS)
        if let data = try? url.bookmarkData(options: .withSecurityScope, includingResourceValuesForKeys: nil,
                                            relativeTo: nil) {
            return data
        }
        #endif
        return try? url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
    }

    private static func resolve(bookmark: Data) -> URL? {
        var stale = false
        #if os(macOS)
        if let url = try? URL(resolvingBookmarkData: bookmark, options: .withSecurityScope, relativeTo: nil,
                              bookmarkDataIsStale: &stale) {
            return url
        }
        #endif
        return try? URL(resolvingBookmarkData: bookmark, options: [], relativeTo: nil, bookmarkDataIsStale: &stale)
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
        var interpolation = SF2SynthAudioUnit.Interpolation.cubic.rawValue
        var limiter = true
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
    /// The render cycle last rendered, and which busses it has served: a
    /// bus pulled again starts the next cycle.
    private var renderedSampleTime = Double.nan
    private var renderedFrames = 0
    private var served = (false, false)

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
        guard let newSynth = makeSynth(font: newFont, sampleRate: sampleRate, maximumVoices: maximumVoices) else {
            sf2synth_font_free(newFont)
            return
        }
        // The render thread never reads `font`: the old one goes now (its
        // synth keeps it alive until it's retired).
        sf2synth_font_free(font)
        font = newFont
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
        renderedSampleTime = .nan
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
        // A host pulling both busses passes the cycle's events only to the
        // first call (AVAudioEngine does), so that call renders, whichever
        // bus it is for, and the other bus gets the same rendering.
        let sampleTime = timestamp.pointee.mSampleTime
        let isServed = bus == 0 ? served.0 : served.1
        if sampleTime != renderedSampleTime || frames != renderedFrames || isServed {
            renderCycle(sampleTime: sampleTime, frames: frames, events: events)
            renderedSampleTime = sampleTime
            renderedFrames = frames
            served = (false, false)
        } else if let synth, events != nil {
            // Events with the second call too: apply them now (they sound
            // from the next buffer).
            handle(events, synth: synth, sampleTime: sampleTime, frames: frames) { _ in }
        }
        if bus == 0 { served.0 = true } else { served.1 = true }
        // (No arrays here: they would allocate on the render thread.)
        let sources: (UnsafeMutablePointer<Float>, UnsafeMutablePointer<Float>)
        if bus == 0 {
            sources = (left, right)
        } else {
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
        handle(events, synth: synth, sampleTime: sampleTime, frames: frames, before: render(upTo:))
        render(upTo: frames)
    }

    /// Passes `events` to the synth, calling `before(offset)` before each
    /// with its frame offset in the buffer.
    private func handle(_ events: UnsafePointer<AURenderEvent>?, synth: OpaquePointer, sampleTime: Double,
                        frames: Int, before: (Int) -> Void) {
        let immediate = Int64(bitPattern: 0xffff_ffff_0000_0000)
        var event = events
        while let current = event {
            let head = current.pointee.head
            let time = head.eventSampleTime
            let offset = time >= immediate && time < 0 ? Int(time - immediate) : Int(Double(time) - sampleTime)
            before(min(max(offset, 0), frames))
            switch head.eventType {
            case .MIDI, .midiSysEx:
                // A System Exclusive event's bytes run past `data`'s three:
                // read them in place.
                let bytes = (UnsafeRawPointer(current) + MemoryLayout<AUMIDIEvent>.offset(of: \.data)!)
                    .assumingMemoryBound(to: UInt8.self)
                sf2synth_midi_bytes(synth, bytes, Int(current.pointee.MIDI.length))
            case .parameter, .parameterRamp:
                // Automation: at once (ramps jump to their end value).
                let parameter = current.pointee.parameter
                applyParameter(parameter.parameterAddress, parameter.value, to: synth)
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

    private func applyParameter(_ address: AUParameterAddress, _ value: AUValue, to synth: OpaquePointer) {
        switch SF2SynthAudioUnit.ParameterAddress(rawValue: address) {
        case .masterGain: sf2synth_set_master_gain(synth, max(0, value))
        case .reverb: sf2synth_set_reverb_enabled(synth, value >= 0.5)
        case .chorus: sf2synth_set_chorus_enabled(synth, value >= 0.5)
        case nil: break
        }
    }

    private func applySettings(_ synth: OpaquePointer) {
        guard let current = settings.withLockIfAvailable({ $0 }), current.version != appliedVersion else { return }
        sf2synth_set_master_gain(synth, current.masterGain)
        sf2synth_set_reverb_enabled(synth, current.reverb)
        sf2synth_set_chorus_enabled(synth, current.chorus)
        sf2synth_set_interpolation(synth, current.interpolation)
        sf2synth_set_limiter(synth, current.limiter)
        appliedVersion = current.version
    }
}
