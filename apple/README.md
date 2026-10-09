# SF2Synth for iOS and macOS

sf2synth as an Audio Unit: the sources of the repository's Swift package
(`Package.swift` at its root), over the C interface in [../capi](../capi).
iOS 16 and macOS 13 or later.

- `SF2SynthAudioUnit`: an in-process AUv3 music device. MIDI events are
  applied at their frame in each buffer; System Exclusive messages work
  even when the host splits them; MIDI 2.0 event lists are read too.
  Output bus 0 is the mix, bus 1 the reverb send (CC91); both come from
  one rendering per cycle, whichever the host pulls first.
- `SF2SynthInstrument`: the same unit as an `AVAudioUnitMIDIInstrument`,
  to attach to an `AVAudioEngine`.
- `SF2SynthViewController` and `SF2SynthView`: the principal class and the
  settings view of an Audio Unit extension, for hosts such as GarageBand and
  Logic. [../examples/auv3](../examples/auv3) is an app with one.

The master gain, reverb and chorus are Audio Unit parameters (hosts can
show and automate them), and `fullState` keeps them with the SoundFont (as a
bookmark), so a host's project reopens with the same sound.

## Setup

In Xcode, *File → Add Package Dependencies…* with
`https://github.com/albos-inc/sf2synth` (from version 0.2.0), and link the
`SF2Synth` library. The package downloads the Rust library
(`SF2SynthC.xcframework`) attached to the release.

To build the Rust library from source instead (for changes to it):

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
apple/build-xcframework.sh
```

builds it as `apple/SF2SynthC.xcframework`, which the package then uses
instead of the released one (add the checkout as a local package).

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
let format = synth.outputFormat(forBus: 1)
engine.connect(synth, to: reverb, fromBus: 1, toBus: 0, format: format)
engine.connect(reverb, to: engine.mainMixerNode, fromBus: 0, toBus: 1, format: format)
```

Bus 1 carries the mono reverb send on both channels. Give the reverb's
*output* connection the synth's format too: with `format: nil` an
`AVAudioUnitReverb` keeps its default 44.1 kHz output, can't convert from a
48 kHz input, and `engine.start()` fails with -10868
(`kAudioUnitErr_FormatNotSupported`). A mixer between the synth and the
reverb avoids it as well.

## Tests

From the repository's root:

```sh
cargo run --manifest-path capi/Cargo.toml --example test_font -- /tmp/test.sf2
SF2SYNTH_TEST_FONT=/tmp/test.sf2 swift test
```

(any General MIDI SoundFont works as well).
