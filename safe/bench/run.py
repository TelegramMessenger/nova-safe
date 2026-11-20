#!/usr/bin/env python3
"""Runs the benchmark cases in QuickJS and in Nova and compares them.

    safe/bench/run.py --qjs-src <quickjs checkout> [--nova-lib <libnova_embed_ffi.a>]
                      [--filter TEXT] [--heap BYTES] [--out DIR] [--label NAME]
                      [--engines qjs,nova] [--baseline results.json] [--repeat N]

Both engines run embedded as the widget sandbox embeds them (bench.c): an
8 MB heap, an interrupt callback, a 6 MB stack limit. One process runs one
case. Without --nova-lib the library is built from this checkout with cargo
(release, as a static library alone, so the profile's fat LTO applies).

Writes <out>/<label>.json and prints a table: milliseconds per run (the
minimum of the runs) and Nova's time over QuickJS's. --baseline adds a column
against an earlier results file, for a change to Nova. --repeat runs every
case in that many processes and keeps the fastest, which is what a ratio
near a round number needs to be told from its neighbour.
"""
import argparse
import glob
import json
import math
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
QJS_FILES = ["quickjs.c", "libregexp.c", "libunicode.c", "dtoa.c"]


def sh(args, **kwargs):
    result = subprocess.run(args, **kwargs)
    if result.returncode != 0:
        sys.exit("failed: " + " ".join(args))
    return result


def newer(target, sources):
    if not os.path.exists(target):
        return True
    made = os.path.getmtime(target)
    return any(os.path.getmtime(s) > made for s in sources)


def build(qjs_src, nova_lib, work):
    os.makedirs(work, exist_ok=True)
    objects = []
    for name in QJS_FILES:
        source = os.path.join(qjs_src, name)
        obj = os.path.join(work, name[:-2] + ".o")
        if newer(obj, [source]):
            sh(["cc", "-O2", "-DNDEBUG", "-D_GNU_SOURCE", "-DQUICKJS_NG_BUILD", "-funsigned-char", "-w", "-std=gnu11", "-c", source, "-o", obj])
        objects.append(obj)
    if not nova_lib:
        cargo = os.environ.get("CARGO", "cargo")
        target = os.path.join(ROOT, "target", "bench")
        sh([cargo, "rustc", "--release", "--locked", "--package", "nova_embed_ffi", "--lib", "--crate-type", "staticlib", "--target-dir", target], cwd=ROOT)
        nova_lib = os.path.join(target, "release", "libnova_embed_ffi.a")
    binary = os.path.join(work, "bench")
    source = os.path.join(HERE, "bench.c")
    order = os.path.join(HERE, "order.txt")
    if newer(binary, [source, nova_lib, order] + objects):
        frameworks = ["-framework", "CoreFoundation"] if sys.platform == "darwin" else ["-ldl"]
        link = ["cc", "-O2", "-std=gnu11", "-Wall", "-I", qjs_src, "-I", os.path.join(ROOT, "nova_embed_ffi", "include"), source] + objects + [nova_lib, "-lm", "-lpthread"] + frameworks
        listed = placed_first(nova_lib, order)
        if not listed:
            sh(link + ["-o", binary])
        else:
            # Linked twice: once with the functions in their order, to learn
            # how long each is, then with what fills each up to its slot.
            path = os.path.join(work, "order.txt")
            with open(path, "w") as out:
                out.write("".join(name + "\n" for name in listed))
            sh(link + ["-Wl,-order_file," + path, "-o", binary])
            fill = slots(binary, listed, work)
            with open(path, "w") as out:
                out.write("".join(name + "\n" for name in fill["order"]))
            sh(link + [fill["object"], "-Wl,-order_file," + path, "-o", binary])
            check_slots(binary, listed)
    return binary


# Every function of order.txt starts at a multiple of this from the first of
# them.
SLOT = 0x1000


def placed_first(nova_lib, order):
    """The functions order.txt names, by their symbols, in its order: what the
    binary is to have at its start. Where a function is in the binary changes
    how fast it runs by a few percent, and without this that place moves with
    any change to what is laid out before it."""
    if sys.platform != "darwin":
        return []
    symbols = subprocess.run(["nm", nova_lib], capture_output=True).stdout.decode("utf-8", "replace").split()
    listed = []
    for line in open(order):
        part = line.strip()
        if not part or part.startswith("#"):
            continue
        if part.startswith("_"):
            # A function of QuickJS, by its whole name.
            listed.append(part)
            continue
        # The function itself: its name ends with the part and a hash.
        whole = re.compile("_+ZN.*" + re.escape(part) + "[0-9a-f]{16}E")
        found = sorted(set(name for name in symbols if whole.fullmatch(name)))
        if len(found) != 1:
            print("order.txt: %s names %d functions of the library, not one" % (part, len(found)), file=sys.stderr)
        listed += found
    return listed


def text_symbols(binary):
    """(address, name) of what is in the text of the binary, by address."""
    found = []
    for line in subprocess.run(["nm", "-n", binary], capture_output=True).stdout.decode("utf-8", "replace").splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[1] in "tT":
            found.append((int(parts[0], 16), parts[2]))
    return found


def slots(binary, listed, work):
    """What fills each listed function up to the next multiple of SLOT, as an
    object file of one symbol per function, and the order with those symbols
    in it. A function that grows or shrinks within its slot then moves no
    other: not the listed ones after it, and not the rest of the binary, which
    starts where the last slot ends."""
    symbols = text_symbols(binary)
    following = {name: symbols[at + 1][0] - address for at, (address, name) in enumerate(symbols[:-1])}
    order, source, used = [], [".text\n"], 0
    for at, name in enumerate(listed):
        order.append(name)
        if name not in following:
            continue
        used += following[name]
        fill = -used % SLOT
        if fill:
            pad = "_nova_bench_slot_%d" % at
            source.append(".globl %s\n.p2align 2\n%s:\n.space %d\n" % (pad, pad, fill))
            order.append(pad)
            used += fill
    source.append(".subsections_via_symbols\n")
    path = os.path.join(work, "slots.s")
    with open(path, "w") as out:
        out.write("".join(source))
    obj = os.path.join(work, "slots.o")
    sh(["cc", "-c", path, "-o", obj])
    return {"order": order, "object": obj}


def check_slots(binary, listed):
    """Says so if a listed function is not at the start of a slot."""
    where = {name: address for address, name in text_symbols(binary)}
    placed = [where[name] for name in listed if name in where]
    if placed and any((address - placed[0]) % SLOT for address in placed):
        print("order.txt: not every function starts a slot of %d bytes" % SLOT, file=sys.stderr)


def run_case(binary, engine, case, index, heap):
    args = [binary, engine, os.path.join(HERE, "prelude.js"), case, str(index), str(heap)]
    try:
        proc = subprocess.run(args, capture_output=True, timeout=180)
    except subprocess.TimeoutExpired:
        return {"fatal": "timeout"}
    if proc.returncode < 0:
        return {"fatal": "signal %d" % -proc.returncode}
    rows = []
    for line in proc.stdout.decode("utf-8", "replace").splitlines():
        if line.startswith("{"):
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    if index < 0:
        return [row["list"] for row in rows if "list" in row]
    for row in rows:
        if "fatal" in row or "error" in row or "min" in row:
            return row
    return {"fatal": "no output"}


def fastest(repeat, binary, engine, case, index, heap):
    """The case in `repeat` processes, one after another: the one whose fastest
    run is the fastest of all, or the first that the engine ended. One process
    times a case for 300 ms, and where it lands in memory moves its time by
    several percent: a table of single processes cannot tell 1.95x from 2.05x."""
    best = None
    for _ in range(max(1, repeat)):
        row = run_case(binary, engine, case, index, heap)
        if "min" not in row:
            return row
        if best is None or row["min"] < best["min"]:
            best = row
    return best


def cell(row):
    if row is None:
        return "-"
    if "min" in row:
        return "%.2f" % row["min"]
    return "FAIL"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--qjs-src", default=os.environ.get("QJS_SRC"))
    parser.add_argument("--nova-lib")
    parser.add_argument("--filter", default="")
    parser.add_argument("--heap", type=int, default=8 << 20)
    parser.add_argument("--out", default=os.path.join(ROOT, "target", "bench", "results"))
    parser.add_argument("--label", default="run")
    parser.add_argument("--engines", default="qjs,nova")
    parser.add_argument("--baseline")
    parser.add_argument("--repeat", type=int, default=1, help="processes per case and engine, of which the fastest counts")
    args = parser.parse_args()
    if not args.qjs_src or not os.path.exists(os.path.join(args.qjs_src, "quickjs.c")):
        sys.exit("--qjs-src (or QJS_SRC) must be a QuickJS checkout")
    binary = build(args.qjs_src, args.nova_lib, os.path.join(ROOT, "target", "bench", "work"))
    engines = args.engines.split(",")
    baseline = {}
    if args.baseline:
        baseline = {row["name"]: row for row in json.load(open(args.baseline))["cases"]}
    results = []
    for case in sorted(glob.glob(os.path.join(HERE, "cases", "*.js"))):
        names = run_case(binary, "qjs", case, -1, 0)
        for index, name in enumerate(names):
            if args.filter not in name:
                continue
            row = {"name": name, "file": os.path.basename(case)}
            for engine in engines:
                row[engine] = fastest(args.repeat, binary, engine, case, index, args.heap)
            results.append(row)
            q, n = row.get("qjs"), row.get("nova")
            ratio = "%6.1fx" % (n["min"] / q["min"]) if q and n and "min" in q and "min" in n and q["min"] > 0 else "      -"
            note = ""
            if n and "min" not in n:
                note = "  nova: " + (n.get("fatal") or n.get("error") or "")[:70]
            if q and "min" not in q:
                note += "  qjs: " + (q.get("fatal") or q.get("error") or "")[:70]
            if q and n and "check" in q and "check" in n and q["check"] != n["check"]:
                note += "  DIFFERENT RESULT %r vs %r" % (q["check"], n["check"])
            before = ""
            old = baseline.get(name, {}).get("nova")
            if old and "min" in old and n and "min" in n:
                before = "  was %.2f (%+.0f%%)" % (old["min"], (n["min"] / old["min"] - 1) * 100)
            print("%-36s qjs %9s  nova %9s  %s%s%s" % (name, cell(q), cell(n), ratio, before, note), flush=True)
    os.makedirs(args.out, exist_ok=True)
    path = os.path.join(args.out, args.label + ".json")
    with open(path, "w") as f:
        json.dump({"heap": args.heap, "cases": results}, f, indent=1)
    groups = {}
    for row in results:
        q, n = row.get("qjs"), row.get("nova")
        if q and n and "min" in q and "min" in n and q["min"] > 0 and n["min"] > 0:
            groups.setdefault(row["name"].split("/")[0], []).append(n["min"] / q["min"])
    print()
    every = []
    for group in sorted(groups):
        ratios = groups[group]
        every += ratios
        print("%-10s %2d cases  geometric mean %5.1fx  worst %6.1fx" % (group, len(ratios), math.exp(sum(map(math.log, ratios)) / len(ratios)), max(ratios)))
    if every:
        failed = [row["name"] for row in results if row.get("nova") and "min" not in row["nova"]]
        print("%-10s %2d cases  geometric mean %5.1fx  worst %6.1fx  nova failed: %d %s" % ("all", len(every), math.exp(sum(map(math.log, every)) / len(every)), max(every), len(failed), ", ".join(failed)))
    print(path)


if __name__ == "__main__":
    main()
