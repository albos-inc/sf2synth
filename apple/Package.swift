// swift-tools-version:5.9
// sf2synth as an Audio Unit (AUv3, in-process) for iOS and macOS apps.
//
// Build the Rust library first (it isn't checked in):
//
//   apple/build-xcframework.sh
//
// then add this directory to the app as a local package (or depend on the
// repository and run the script in its checkout).
import PackageDescription

let package = Package(
    name: "SF2Synth",
    platforms: [.iOS(.v16), .macOS(.v13)],
    products: [
        .library(name: "SF2Synth", targets: ["SF2Synth"]),
    ],
    targets: [
        .binaryTarget(name: "SF2SynthC", path: "SF2SynthC.xcframework"),
        .target(name: "SF2Synth", dependencies: ["SF2SynthC"]),
        // Needs a General MIDI SoundFont: SF2SYNTH_TEST_FONT=/path/to/font.sf2 swift test
        .testTarget(name: "SF2SynthTests", dependencies: ["SF2Synth"]),
    ]
)
