#!/bin/sh
# Builds the C interface (../capi) for macOS, iOS and the iOS Simulator and
# packs it as SF2SynthC.xcframework next to this script, for Package.swift.
#
#   apple/build-xcframework.sh
#
# Needs the Rust targets:
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
set -eu

# Xcode's PATH (when run from a build phase) doesn't include rustup's.
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
unset SDKROOT
# The package's minimum versions.
export MACOSX_DEPLOYMENT_TARGET=13.0
export IPHONEOS_DEPLOYMENT_TARGET=16.0

here="$(cd "$(dirname "$0")" && pwd)"
manifest="$here/../capi/Cargo.toml"
target_dir="$here/build/cargo"
build="$here/build"

for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios; do
    if ! rustup target list --installed | grep -qx "$target"; then
        echo "error: run 'rustup target add $target'" >&2
        exit 1
    fi
    cargo build --release --manifest-path "$manifest" --target "$target" --target-dir "$target_dir"
done

library() {
    echo "$target_dir/$1/release/libsf2synth_c.a"
}

rm -rf "$build/libraries" "$build/include" "$here/SF2SynthC.xcframework"
mkdir -p "$build/libraries/macos" "$build/libraries/ios" "$build/libraries/ios-simulator" "$build/include"
lipo -create "$(library aarch64-apple-darwin)" "$(library x86_64-apple-darwin)" \
    -output "$build/libraries/macos/libsf2synth_c.a"
cp "$(library aarch64-apple-ios)" "$build/libraries/ios/libsf2synth_c.a"
lipo -create "$(library aarch64-apple-ios-sim)" "$(library x86_64-apple-ios)" \
    -output "$build/libraries/ios-simulator/libsf2synth_c.a"

cp "$here/../capi/include/sf2synth.h" "$build/include/"
cat > "$build/include/module.modulemap" <<'MODULEMAP'
module SF2SynthC {
    header "sf2synth.h"
    export *
}
MODULEMAP

xcodebuild -create-xcframework \
    -library "$build/libraries/macos/libsf2synth_c.a" -headers "$build/include" \
    -library "$build/libraries/ios/libsf2synth_c.a" -headers "$build/include" \
    -library "$build/libraries/ios-simulator/libsf2synth_c.a" -headers "$build/include" \
    -output "$here/SF2SynthC.xcframework"
echo "built $here/SF2SynthC.xcframework"
