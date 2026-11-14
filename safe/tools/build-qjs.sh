#!/bin/bash
set -eu
cd "$(git rev-parse --show-toplevel)"
SRC=${QJS_SRC:-/Users/renoir/projects/Telegram/telegrammacos/third-party/widget-sandbox/packages/ios/Sources/CQuickJS/quickjs}
OUT=target/qjs
if [ -x "$OUT/qjs" ] && [ "$OUT/qjs" -nt "$SRC/quickjs.c" ]; then
  echo "$OUT/qjs"
  exit 0
fi
mkdir -p "$OUT"
cmake -S "$SRC" -B "$OUT" -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$OUT" --target qjs_exe -j 8 >/dev/null
echo "$OUT/qjs"
