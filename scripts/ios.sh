#!/usr/bin/env bash
# Build the core for iOS: an XCFramework (device and simulator) and the Swift
# file that calls it. Usage: scripts/ios.sh <output directory>
set -euo pipefail

OUT="${1:?usage: scripts/ios.sh <output directory>}"
cd "$(dirname "$0")/.."
LIB=libmagnetite_mobile_core
# The oldest iOS the app runs on; must match the app's deployment target.
export IPHONEOS_DEPLOYMENT_TARGET=17.0

for target in aarch64-apple-ios aarch64-apple-ios-sim; do
  cargo build --release --lib --target "$target"
done

# The bindings are read out of a build for this machine.
cargo build --lib
gen="$(mktemp -d)"
trap 'rm -r "$gen"' EXIT
cargo run --quiet --features bindgen --bin uniffi-bindgen -- \
  generate --library "target/debug/$LIB.dylib" --language swift --out-dir "$gen"

# Xcode wants the C header beside a file named module.modulemap.
mkdir "$gen/headers"
mv "$gen"/*.h "$gen/headers/"
mv "$gen"/*.modulemap "$gen/headers/module.modulemap"

mkdir -p "$OUT"
rm -rf "$OUT/MagnetiteCoreFFI.xcframework"
xcodebuild -create-xcframework \
  -library "target/aarch64-apple-ios/release/$LIB.a" -headers "$gen/headers" \
  -library "target/aarch64-apple-ios-sim/release/$LIB.a" -headers "$gen/headers" \
  -output "$OUT/MagnetiteCoreFFI.xcframework" > /dev/null
cp "$gen"/*.swift "$OUT/"
echo "Core for iOS is in $OUT"
