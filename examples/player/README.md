# SF2 Player

A sample MIDI file player on sf2synth for **iOS, Android, Windows and
macOS**. Choose an SF2 (or DLS) file and a MIDI file, then play, pause, stop
and seek, with the song title, a progress bar, the time and information about
the song, the SoundFont and the audio output.

Ready-to-run builds are on the
[Releases page](https://github.com/albos-inc/sf2synth/releases?q=player)
(see [Installing](#installing)).

| | |
|---|---|
| Transport | Play / pause / stop, a seek bar, elapsed and remaining time |
| Song | Title (the MIDI file's sequence name, Shift_JIS or UTF-8, else the file name), length, SMF format and tracks, tempo, resolution, notes and channels |
| SoundFont | Bank name, file, SF2 version or DLS, size, presets / instruments / samples, copyright and comment |
| Audio | Output device, sample rate, buffer size, voices playing |

The last SF2 chosen is remembered. On Windows and macOS, the system's own
General MIDI sounds (`gm.dls`, `gs_instruments.dls`) play until an SF2 is
chosen, so a MIDI file plays right away.

## How it is built

```
examples/player/
├── core/      Rust: the player (Player, LoadedSoundFont, LoadedSong), on sf2synth
├── ffi/       Rust: core as a C library for Swift (include/sf2player.h) and JNI for Kotlin
├── apple/     iOS and macOS app: SwiftUI, AVAudioEngine (one Xcode target for both)
├── android/   Android app: Kotlin, Jetpack Compose, AudioTrack
├── windows/   Windows app: Rust, egui, cpal (WASAPI); runs on macOS and Linux too
└── icon/      make-icons.swift draws the icon for every app
```

Every app keeps the same split: the UI calls the player's transport functions
and reads its state about 20 times a second, while the platform's audio
callback calls the player's render function. Files are parsed off the UI
thread and swapped in under a lock that the audio callback only ever *tries*
(playing silence for one buffer if the UI holds it), so loading a large SF2
never stalls the audio.

| | Audio | Renders in |
|---|---|---|
| iOS / macOS | `AVAudioSourceNode` → `AVAudioEngine` | `sf2player_render` (C) |
| Android | `AudioTrack`, float PCM, from a render thread | `NativePlayer.render` (JNI) |
| Windows | cpal output stream (WASAPI) | `Player::render` (Rust) |

## Building

All builds need [Rust](https://rustup.rs). The apps build the Rust side
themselves (an Xcode build phase, a Gradle task, or Cargo).

### Windows

```sh
cargo run --release -p sf2player-windows -- [song.mid] [font.sf2]
```

from `examples/player`. The executable is `target/release/sf2player.exe`; it
needs nothing else. Files can also be dropped on the window.

### iOS and macOS

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin x86_64-apple-darwin
open apple/SF2Player.xcodeproj
```

Choose the *SF2Player* scheme and an iPhone, iPad or *My Mac*, and run. For a
device, set your team under Signing & Capabilities. `apple/build-rust.sh`
builds `libsf2player_ffi.a` for the platform being built.

The project is generated from `apple/project.yml` with
[XcodeGen](https://github.com/yonaskolb/XcodeGen); run `xcodegen generate` in
`apple` after changing it.

### Android

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
```

Then open `android` in Android Studio and run, or `./gradlew assembleDebug`.
The `buildRust` task builds `libsf2player_ffi.so` for each ABI into
`app/src/main/jniLibs` with cargo-ndk.

Release builds are signed with the key in `android/keystore.properties`
(`storeFile`, `storePassword`, `keyAlias`, `keyPassword`), or the debug key
if there is none.

## Installing

The GitHub workflow `.github/workflows/player.yml` builds every app and, for
a tag `player-v*`, attaches them to a release:

| File | |
|---|---|
| `SF2Player-windows-x64.zip` | Unzip and run `sf2player.exe`. If SmartScreen warns, choose *More info → Run anyway*. |
| `SF2Player-macOS.zip` | Unzip and move *SF2 Player* to Applications. If it isn't notarized, open it once with right-click → *Open* (or *System Settings → Privacy & Security → Open Anyway*). |
| `SF2Player-android.apk` | Open on the device and allow installing from that source. |

The iOS app can't be downloaded from GitHub: Apple only installs apps from the
App Store or TestFlight, or from Xcode onto your own device.

For signed builds, set these repository secrets (all optional):

| Secret | |
|---|---|
| `ANDROID_KEYSTORE_BASE64`, `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` | Android release key (`base64 -i release.jks`) |
| `MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD` | A *Developer ID Application* certificate |
| `APPLE_TEAM_ID`, `APPLE_ID`, `APPLE_APP_PASSWORD` | Notarization (an app-specific password) |

## License

MIT or Apache-2.0, like sf2synth.
