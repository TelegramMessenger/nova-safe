#!/bin/bash
# Finds handles that a function taking `GcScope` uses after a call that may
# run JavaScript (and so collect garbage) without rooting them first.
#
# In a scratch worktree of HEAD, every by-value handle parameter of a fn that
# takes `gc: GcScope` gets `let p = p.bind(gc.nogc());` as its first
# statement, so the borrow checker reports each use after `gc.reborrow()`.
# Each round classifies the errors: injections that do not type-check are
# turned off, values passed into the GC-capable call itself are unbound
# there (the callee roots them), and uses after the call returned are
# candidates. Settles in about six rounds.
#
# Usage: safe/tools/gc_audit/run.sh [out-dir]   (default target/gc-audit)
# Prints the candidates of the last round; parameters whose injection was
# turned off and values that reach a call through a derived local are not
# audited (see the stats line).
set -eu
ROOT=$(git rev-parse --show-toplevel)
TOOLS=$ROOT/safe/tools/gc_audit
OUT=${1:-$ROOT/target/gc-audit}
export PATH="$HOME/.cargo/bin:$PATH"
mkdir -p "$OUT"
TREE=$OUT/tree
if [ -d "$TREE" ]; then git -C "$ROOT" worktree remove --force "$TREE"; fi
git -C "$ROOT" worktree add -q --detach "$TREE" HEAD
trap 'git -C "$ROOT" worktree remove --force "$TREE"' EXIT
: >"$OUT/skip.txt"
python3 -I "$TOOLS/inject.py" "$TREE/nova_vm/src" "$OUT/state.json" --skip "$OUT/skip.txt"
last=""
prev=""
for r in $(seq 1 "${ROUNDS:-12}"); do
  (cd "$TREE" && CARGO_TARGET_DIR="$OUT/target" cargo check -p nova_vm --message-format=json 2>/dev/null) >"$OUT/round$r.json" || true
  out=$(python3 -I "$TOOLS/iterate.py" "$OUT/round$r.json" "$TREE" "$OUT/state.json" "$OUT/result$r.json")
  echo "round $r: $out" >&2
  last=$OUT/result$r.json
  case "$out" in *"edits 0") break ;; esac
  [ "$out" = "$prev" ] && break
  prev=$out
done
python3 -I "$TOOLS/report.py" "$last"
