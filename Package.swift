// swift-tools-version:5.9
// sf2synth as an Audio Unit (AUv3) for iOS and macOS apps: the Swift sources
// are in apple/, the Rust synthesizer comes as SF2SynthC.xcframework.
//
// Added by URL, the package uses the xcframework attached to the release.
// In a checkout, apple/build-xcframework.sh builds it from the Rust sources
// into apple/, and the package then uses that one.
import Foundation
import PackageDescription

let localFramework = "apple/SF2SynthC.xcframework"
let useLocalFramework = FileManager.default.fileExists(atPath: Context.packageDirectory + "/" + localFramework)

let package = Package(
    name: "SF2Synth",
    platforms: [.iOS(.v16), .macOS(.v13)],
    products: [
        .library(name: "SF2Synth", targets: ["SF2Synth"]),
    ],
    targets: [
        useLocalFramework
            ? .binaryTarget(name: "SF2SynthC", path: localFramework)
            : .binaryTarget(
                name: "SF2SynthC",
                url: "https://github.com/albos-inc/sf2synth/releases/download/v0.4.1/SF2SynthC.xcframework.zip",
                checksum: "caba94c9876bf77472aa5eb8bf697f3c5d62ac2e2bbbff6f18fba1aa68a8dfbf"
            ),
        .target(name: "SF2Synth", dependencies: ["SF2SynthC"], path: "apple/Sources/SF2Synth"),
        // Needs a General MIDI SoundFont: SF2SYNTH_TEST_FONT=/path/to/font.sf2 swift test
        .testTarget(name: "SF2SynthTests", dependencies: ["SF2Synth"], path: "apple/Tests/SF2SynthTests"),
    ]
)
