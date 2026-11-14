#!/bin/sh
# Compiles nova_embed_ffi/tests/c_api.c against include/nova_engine.h and the
# debug staticlib, strictly, with AddressSanitizer and UBSan on the C side, and
# runs it.
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-${TMPDIR:-/tmp}/nova-embed-c-abi}"
mkdir -p "$OUT"
cd "$ROOT"
cargo build -p nova_embed_ffi
cc -std=c11 -Wall -Wextra -Werror -pedantic -fsanitize=address,undefined -g \
    -I nova_embed_ffi/include nova_embed_ffi/tests/c_api.c target/debug/libnova_embed_ffi.a \
    -framework CoreFoundation -o "$OUT/c_api"
"$OUT/c_api"
