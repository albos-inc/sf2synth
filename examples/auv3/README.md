# sf2synth AU

An iOS and macOS app with an Audio Unit extension that offers sf2synth to
hosts such as GarageBand, Logic, AUM or MainStage: a music device ("Albos:
sf2synth") whose view chooses the SoundFont and sets the gain, reverb and
chorus. The app itself plays the same instrument on a one-octave keyboard.

```
examples/auv3/
├── App/          the containing app (SwiftUI)
├── Extension/    the extension: one subclass of SF2SynthViewController and
│                 its Info.plist (AudioComponents: aumu / sf2s / Albs)
└── project.yml   XcodeGen project (SF2SynthAU.xcodeproj is generated from it)
```

## Building

```sh
open examples/auv3/SF2SynthAU.xcodeproj
```

Choose the *SF2SynthAU* scheme and a device or *My Mac*, set your team under
Signing & Capabilities for both targets, and run. Once the app is installed
(on macOS, opened once), hosts list the instrument. The project uses the
Swift package at the repository's root: with the Rust library attached to
the release, or one built from the checkout after
`apple/build-xcframework.sh`.

On macOS, `auval -v aumu sf2s Albs` validates the installed extension.

After changing `project.yml`, run `xcodegen generate` here.

## Notes

- The SoundFont is memory-mapped and kept in the host's project as a
  bookmark. On iOS, choose a file stored on the device: the extension keeps
  reading it while it plays.
- The extension's module must not be called `sf2synth`: on a
  case-insensitive disk it would clash with the package's `SF2Synth`.
