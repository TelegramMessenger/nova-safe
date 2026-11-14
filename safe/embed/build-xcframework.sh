#!/bin/sh
# Builds NovaEngineFFI.xcframework (nova_embed_ffi as a static library with its
# header and module map) for macOS: arm64 from 11.0 and x86_64 from 10.13, release
# with fat LTO and without debug info.
#
# Each architecture's library is prelinked into one object that exports only the
# nova_* functions: the Rust standard library inside it (rust_eh_personality and
# friends) stays private, so the library links next to another Rust static
# library, such as the app's MTProto engine, without duplicate symbols.
#
#   safe/embed/build-xcframework.sh OUTPUT_DIR [BUILD_DIR]
#
# OUTPUT_DIR receives NovaEngineFFI.xcframework (replaced) and manifest.sha256.

set -eu

OUTPUT_DIR="${1:-}"
if [ -z "$OUTPUT_DIR" ]; then
    echo "Usage: $0 OUTPUT_DIR [BUILD_DIR]"
    exit 1
fi
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BUILD_DIR="${2:-${TMPDIR:-/tmp}/nova-embed-xcframework}"
mkdir -p "$OUTPUT_DIR" "$BUILD_DIR"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"
BUILD_DIR="$(cd "$BUILD_DIR" && pwd)"

if [ -n "${CARGO:-}" ]; then
    CARGO_BIN="$CARGO"
elif command -v cargo >/dev/null 2>&1; then
    CARGO_BIN="$(command -v cargo)"
else
    CARGO_BIN="$HOME/.cargo/bin/cargo"
fi
RUSTUP_BIN="$(dirname "$CARGO_BIN")/rustup"

cd "$ROOT"
COMMIT="$(git rev-parse --short=12 HEAD)"
if [ -n "$(git status --porcelain -- nova_vm nova_embed nova_embed_ffi small_string wtf8 nova_vm_derive Cargo.toml)" ]; then
    COMMIT="$COMMIT-dirty"
fi

build() {
    target="$1"
    deployment="$2"
    if [ -x "$RUSTUP_BIN" ]; then
        "$RUSTUP_BIN" target add "$target" >/dev/null 2>&1 || true
    fi
    NOVA_EMBED_COMMIT="$COMMIT" MACOSX_DEPLOYMENT_TARGET="$deployment" CARGO_TARGET_DIR="$BUILD_DIR/target" \
        "$CARGO_BIN" build --package nova_embed_ffi --release --target "$target"
    arch="$3"
    printf '_nova_*\n' > "$BUILD_DIR/exported-symbols.txt"
    if ! ld -r -arch "$arch" -platform_version macos "$deployment" "$deployment" \
        -exported_symbols_list "$BUILD_DIR/exported-symbols.txt" \
        -all_load "$BUILD_DIR/target/$target/release/libnova_embed_ffi.a" \
        -o "$BUILD_DIR/nova_embed_ffi-$target.o" 2> "$BUILD_DIR/ld-$target.log"; then
        cat "$BUILD_DIR/ld-$target.log"
        exit 1
    fi
    strip -S "$BUILD_DIR/nova_embed_ffi-$target.o"
    rm -f "$BUILD_DIR/libnova_embed_ffi-$target.a"
    libtool -static -o "$BUILD_DIR/libnova_embed_ffi-$target.a" "$BUILD_DIR/nova_embed_ffi-$target.o"
    exported="$(nm -gU "$BUILD_DIR/libnova_embed_ffi-$target.a" | awk 'NF == 3 { print $3 }' | grep -v '^_nova_' || true)"
    if [ -n "$exported" ]; then
        echo "symbols other than nova_* are exported for $target:"
        echo "$exported"
        exit 1
    fi
}

build aarch64-apple-darwin 11.0 arm64
build x86_64-apple-darwin 10.13 x86_64

lipo -create \
    "$BUILD_DIR/libnova_embed_ffi-aarch64-apple-darwin.a" \
    "$BUILD_DIR/libnova_embed_ffi-x86_64-apple-darwin.a" \
    -output "$BUILD_DIR/libnova_embed_ffi.a"

HEADERS_DIR="$BUILD_DIR/headers"
rm -rf "$HEADERS_DIR"
mkdir -p "$HEADERS_DIR"
cp nova_embed_ffi/include/nova_engine.h "$HEADERS_DIR/"
cat > "$HEADERS_DIR/module.modulemap" <<EOF
module NovaEngineFFI {
    header "nova_engine.h"
    export *
}
EOF

rm -rf "$OUTPUT_DIR/NovaEngineFFI.xcframework"
xcodebuild -create-xcframework \
    -library "$BUILD_DIR/libnova_embed_ffi.a" \
    -headers "$HEADERS_DIR" \
    -output "$OUTPUT_DIR/NovaEngineFFI.xcframework" >/dev/null

(cd "$OUTPUT_DIR" && find NovaEngineFFI.xcframework -type f | LC_ALL=C sort | xargs shasum -a 256 > manifest.sha256)
echo "nova commit: $COMMIT" > "$OUTPUT_DIR/BUILD_INFO"
echo "rustc: $(rustc --version)" >> "$OUTPUT_DIR/BUILD_INFO"

lipo -info "$BUILD_DIR/libnova_embed_ffi.a"
for target in aarch64-apple-darwin x86_64-apple-darwin; do
    echo "$target: $(stat -f %z "$BUILD_DIR/libnova_embed_ffi-$target.a") bytes"
done
du -sh "$OUTPUT_DIR/NovaEngineFFI.xcframework"
echo "NovaEngineFFI.xcframework (nova $COMMIT) written to $OUTPUT_DIR"
