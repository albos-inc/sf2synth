# SF2Synth for iOS and macOS

sf2synth as an Audio Unit: a Swift package over the C interface in
[../capi](../capi). iOS 16 and macOS 13 or later.

- `SF2SynthAudioUnit`: an in-process AUv3 music device. MIDI events are
  applied at their frame in each buffer; System Exclusive messages work
  even when the host splits them; MIDI 2.0 event lists are read too.
  Output bus 0 is the mix, bus 1 the reverb send (CC91).
- `SF2SynthInstrument`: the same unit as an `AVAudioUnitMIDIInstrument`,
  to attach to an `AVAudioEngine`.

## Setup

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
apple/build-xcframework.sh
```

builds `SF2SynthC.xcframework` (the Rust library for macOS, iOS and the
Simulator) next to `Package.swift`. Then add the `apple` directory to the
Xcode project as a local package and link the `SF2Synth` library.

## Use

```swift
import SF2Synth

let synth = try SF2SynthInstrument(soundFont: url, sampleRate: outputRate)
synth.synth.masterGain = 1.2          // as loud as Apple's MIDI synth
engine.attach(synth)
engine.connect(synth, to: engine.mainMixerNode, fromBus: 0, toBus: 0, format: nil)

// MIDI: notes, controllers, SysEx (GS Reset, master volume…).
synth.startNote(60, withVelocity: 100, onChannel: 0)
synth.sendMIDISysExEvent(Data([0xF0, 0x41, 0x10, 0x42, 0x12, 0x40, 0x00, 0x7F, 0x00, 0x41, 0xF7]))
```

SoundFonts are memory-mapped by default (`mapped: false` reads them into
memory). `loadSoundFont(at:)` can be called while the engine runs: the new
synth takes over at the next buffer.

### Your own reverb

```swift
synth.synth.isReverbEnabled = false
engine.connect(synth, to: reverb, fromBus: 1, toBus: 0, format: nil)
```

Bus 1 carries the mono reverb send on both channels. Bus 0 does the
rendering; if the host pulls bus 1 first, the send lags by one buffer.

## Tests

```sh
cd apple
SF2SYNTH_TEST_FONT=/path/to/GeneralUser-GS.sf2 swift test
```
