#!/bin/sh
# Builds the Rust player (../ffi) for the platform and architectures Xcode is
# building, as $BUILT_PRODUCTS_DIR/libsf2player_ffi.a. Xcode runs this before
# compiling; it can also be run by hand:
#
#   PLATFORM_NAME=macosx ARCHS="arm64 x86_64" BUILT_PRODUCTS_DIR=build ./build-rust.sh
set -eu

# Xcode's PATH doesn't include rustup's.
export PATH="$HOME/.cargo/bin:/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
# Xcode's SDKROOT is for the app's platform; build scripts run on the Mac.
unset SDKROOT

cd "$(dirname "$0")/.."

targets=""
for arch in $ARCHS; do
    case "$PLATFORM_NAME/$arch" in
        iphoneos/arm64) targets="$targets aarch64-apple-ios" ;;
        iphonesimulator/arm64) targets="$targets aarch64-apple-ios-sim" ;;
        iphonesimulator/x86_64) targets="$targets x86_64-apple-ios" ;;
        macosx/arm64) targets="$targets aarch64-apple-darwin" ;;
        macosx/x86_64) targets="$targets x86_64-apple-darwin" ;;
        *) echo "error: no Rust target for $PLATFORM_NAME/$arch" >&2; exit 1 ;;
    esac
done

libraries=""
for target in $targets; do
    if ! rustup target list --installed | grep -qx "$target"; then
        echo "error: run 'rustup target add $target'" >&2
        exit 1
    fi
    # Always optimized: a debug build of the synthesizer is too slow for
    # real time.
    cargo build --release --package sf2player-ffi --target "$target"
    libraries="$libraries target/$target/release/libsf2player_ffi.a"
done

mkdir -p "$BUILT_PRODUCTS_DIR"
# shellcheck disable=SC2086
lipo -create $libraries -output "$BUILT_PRODUCTS_DIR/libsf2player_ffi.a"
