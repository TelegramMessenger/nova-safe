#!/usr/bin/env python3
"""Runs the benchmark cases in QuickJS and in Nova and compares them.

    safe/bench/run.py --qjs-src <quickjs checkout> [--nova-lib <libnova_embed_ffi.a>]
                      [--filter TEXT] [--heap BYTES] [--out DIR] [--label NAME]
                      [--engines qjs,nova] [--baseline results.json]

Both engines run embedded as the widget sandbox embeds them (bench.c): an
8 MB heap, an interrupt callback, a 6 MB stack limit. One process runs one
case. Without --nova-lib the library is built from this checkout with cargo
(release, as a static library alone, so the profile's fat LTO applies).

Writes <out>/<label>.json and prints a table: milliseconds per run (the
minimum of the runs) and Nova's time over QuickJS's. --baseline adds a column
against an earlier results file, for a change to Nova.
"""
import argparse
import glob
import json
import math
import os
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
    if newer(binary, [source, nova_lib] + objects):
        frameworks = ["-framework", "CoreFoundation"] if sys.platform == "darwin" else ["-ldl"]
        sh(["cc", "-O2", "-std=gnu11", "-Wall", "-I", qjs_src, "-I", os.path.join(ROOT, "nova_embed_ffi", "include"), source] + objects + [nova_lib, "-lm", "-lpthread"] + frameworks + ["-o", binary])
    return binary


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
                row[engine] = run_case(binary, engine, case, index, args.heap)
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
