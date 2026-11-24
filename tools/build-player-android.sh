#!/usr/bin/env bash
# Builds the player crate for Android devices and emulators, and writes the shared
# libraries and generated Kotlin bindings to platforms/android/player/generated.
# See platforms/android/PLAYER.md.
set -euo pipefail

export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}"

cd "$(dirname "$0")/.."

generated=platforms/android/player/generated
rm -rf "$generated"

rustup target add aarch64-linux-android x86_64-linux-android
# 16 KB pages are required on Android 15 and later.
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
	cargo ndk -t arm64-v8a -t x86_64 -o "$generated/jniLibs" \
	build --locked --release -p player --features uniffi
cargo build --locked --release -p player --features uniffi-bindgen

target/release/player-bindgen generate \
	--library target/release/libplayer.dylib \
	--language kotlin --no-format --out-dir "$generated/kotlin"

echo "Done: $generated"
