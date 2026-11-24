#!/usr/bin/env bash
# Builds the player crate for iOS devices, the iOS simulator and macOS, and writes
# platforms/ios/PlayerFFI.xcframework and the generated Swift bindings in
# platforms/ios/Sources/PlayerRS/Player.swift. See platforms/ios/PLAYER.md.
set -euo pipefail

cd "$(dirname "$0")/.."

targets=(aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios)
rustup target add "${targets[@]}"
for target in "${targets[@]}"; do
	cargo build --locked --release -p player --features uniffi --target "$target"
done
cargo build --locked --release -p player --features uniffi-bindgen

simulator_universal=target/simulator-universal
mkdir -p "$simulator_universal"
lipo -create \
	target/aarch64-apple-ios-sim/release/libplayer.a \
	target/x86_64-apple-ios/release/libplayer.a \
	-output "$simulator_universal/libplayer.a"

bindings=target/player-bindings-swift
rm -rf "$bindings"
target/release/player-bindgen generate \
	--library target/release/libplayer.dylib \
	--language swift --out-dir "$bindings"

include=target/player-xcframework-include
rm -rf "$include"
mkdir -p "$include"
cp "$bindings/playerFFI.h" "$include/"
cp "$bindings/playerFFI.modulemap" "$include/module.modulemap"

xcframework=platforms/ios/PlayerFFI.xcframework
rm -rf "$xcframework"
xcodebuild -create-xcframework \
	-library target/aarch64-apple-ios/release/libplayer.a -headers "$include" \
	-library "$simulator_universal/libplayer.a" -headers "$include" \
	-library target/release/libplayer.a -headers "$include" \
	-output "$xcframework"

bindings_source=platforms/ios/Sources/PlayerRS/Player.swift
cp "$bindings/player.swift" "$bindings_source"

echo "Done: $xcframework + $bindings_source"
