#!/bin/bash
set -u
cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.cargo/bin:$PATH"
WIDGET_SANDBOX=/Users/renoir/projects/Telegram/telegrammacos/third-party/widget-sandbox
LAB_WIDGETS=/Users/renoir/projects/Telegram/telegrammacos/tools/perflab/TwidgetLab/widgets
BUNDLE=$WIDGET_SANDBOX/packages/ios/Sources/WidgetSandbox/WidgetJS/sandbox-bundle.js
LOG=$(mktemp -d)
status=0

echo "== build"
if ! cargo build --profile dev-fast -p nova_cli -p tests -q 2>"$LOG/build.txt"; then
  cat "$LOG/build.txt" | tail -40
  echo "GATE FAIL: build"
  exit 1
fi
grep -E "^warning" "$LOG/build.txt" | sort | uniq -c | head -20

echo "== test262${GATE_GC:+ (--gc)}"
cargo run -q --bin test262 --profile dev-fast -- --noprogress ${GATE_GC:+--gc} ${GATE_T262_JOBS:+-j $GATE_T262_JOBS} >"$LOG/t262.txt" 2>&1
grep -E "^\s+\".*\" -- Expected" "$LOG/t262.txt" | grep -v -E "Expected Timeout, got Pass|Expected Pass, got Timeout" | sort >"$LOG/t262-unexpected.txt"
if [ -n "${GATE_GC:-}" ]; then
  baseline=safe/baseline/test262-gc-unexpected.txt
  if [ ! -f "$baseline" ]; then
    cp "$LOG/t262-unexpected.txt" "$baseline"
    echo "recorded $baseline ($(wc -l <"$baseline") lines)"
  elif ! diff -u "$baseline" "$LOG/t262-unexpected.txt" >"$LOG/t262-diff.txt"; then
    cat "$LOG/t262-diff.txt" | head -40
    echo "GATE FAIL: test262 --gc differs from $baseline"
    status=1
  fi
elif [ -s "$LOG/t262-unexpected.txt" ]; then
  head -40 "$LOG/t262-unexpected.txt"
  echo "GATE FAIL: test262 ($(wc -l <"$LOG/t262-unexpected.txt") unexpected results besides timeouts)"
  status=1
fi
grep -E "count mismatch" "$LOG/t262.txt"

echo "== cargo test"
if ! cargo test --workspace -q >"$LOG/test.txt" 2>&1; then
  grep -E "FAILED|panicked|error" "$LOG/test.txt" | head -30
  echo "GATE FAIL: cargo test"
  status=1
fi

echo "== widgets (Nova output must not change)"
if ! python3 -I safe/widgets/run.py --nova target/dev-fast/nova_cli --bundle "$BUNDLE" \
    --expect safe/baseline/widgets-nova.json --jobs "${GATE_JOBS:-6}" --timeout 300 \
    $(find "$WIDGET_SANDBOX/corpus" "$LAB_WIDGETS" -name "*.html"); then
  echo "GATE FAIL: widgets"
  status=1
fi

echo "== unsafe"
python3 -I safe/tools/unsafe_report.py .
[ $status = 0 ] && echo "GATE OK" || echo "GATE FAILED"
exit $status
