#!/usr/bin/env bash
# Build the core for Android: one shared library per ABI and the Kotlin file
# that calls it. Usage: scripts/android.sh <output directory>
# Needs cargo-ndk and ANDROID_NDK_HOME.
set -euo pipefail

OUT="${1:?usage: scripts/android.sh <output directory>}"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
cd "$(dirname "$0")/.."
LIB=libmagnetite_mobile_core

# Devices with 16 KB memory pages only load libraries aligned for them.
RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
  cargo ndk --platform 26 -t arm64-v8a -t armeabi-v7a -t x86_64 \
  -o "$OUT/jniLibs" build --release --lib

# The bindings are read out of a build for this machine.
cargo build --lib
case "$(uname -s)" in Darwin) ext=dylib ;; *) ext=so ;; esac
rm -rf "$OUT/kotlin"
cargo run --quiet --features bindgen --bin uniffi-bindgen -- \
  generate --library "target/debug/$LIB.$ext" --language kotlin --out-dir "$OUT/kotlin"
echo "Core for Android is in $OUT"
