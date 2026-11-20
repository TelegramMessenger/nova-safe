#!/usr/bin/env python3
"""The price list: how many machine instructions one operation costs in QuickJS and in Nova.

    safe/bench/prices.py [--bench target/bench/work/bench] [--filter TEXT] [--runs 3] [--save FILE] [--against FILE]

Runs every case of safe/bench/prices.js once in each engine, in the benchmark runner that safe/bench/run.py
built, and reads what the process retired from `/usr/bin/time -l` (macOS). A case is a loop of a million
turns; one named "x@y" is priced against the case "y", so what is printed for it is the instructions of
the one operation it adds to a turn. What the script itself retires is the same from run to run
whatever else the machine does, which no timing is: a change of path length shows here exactly, on a
busy machine too. The count of a process has the kernel's work for it in it as well (its pages), which
varies by some millions of instructions, a few per turn: every case is run several times and the least
count is taken, which leaves under one instruction per turn of it. Instructions are not time (the two
engines run a different number of them per cycle), so the table of run.py still decides; this says
where to look.
"""
import argparse, json, os, re, subprocess, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
PRELUDE = """var __index = 0;
function bench(name, setup) {
  var index = __index++;
  if (__select < 0) { __emit(JSON.stringify({ list: name })); return; }
  if (index !== __select) return;
  var result = { name: name };
  try { result.check = String(setup()()).slice(0, 40); }
  catch (e) { result.error = String(e && e.name ? e.name + ": " + e.message : e).slice(0, 200); }
  __emit(JSON.stringify(result));
}
"""


def retired(bench, engine, prelude, cases, index, runs):
    """(the case's report, the fewest instructions a process of it retired in `runs` runs)."""
    report, least = None, None
    for _ in range(runs):
        run = subprocess.run(["/usr/bin/time", "-l", bench, engine, prelude, cases, str(index), "0"],
                             capture_output=True, text=True)
        for line in run.stdout.splitlines():
            if line.startswith("{"):
                report = json.loads(line)
        found = re.search(r"(\d+)\s+instructions retired", run.stderr)
        if report is None or found is None:
            sys.exit(f"no result for case {index} in {engine}: {run.stdout[-200:]} {run.stderr[-200:]}")
        least = int(found.group(1)) if least is None else min(least, int(found.group(1)))
    return report, least


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bench", default=os.path.join(ROOT, "target", "bench", "work", "bench"))
    parser.add_argument("--filter", default="", help="only the cases whose name has this text (and their bases)")
    parser.add_argument("--runs", type=int, default=3, help="runs of every case, of which the least count is taken")
    parser.add_argument("--save", help="write the prices as JSON")
    parser.add_argument("--against", help="a file written by --save: print the change of Nova's prices from it")
    args = parser.parse_args()
    if sys.platform != "darwin":
        sys.exit("prices.py reads `/usr/bin/time -l`, which counts instructions on macOS only")
    if not os.path.exists(args.bench):
        sys.exit(f"{args.bench} is not built: run safe/bench/run.py first")
    cases = os.path.join(HERE, "prices.js")
    prelude = os.path.join(os.path.dirname(args.bench), "prices-prelude.js")
    with open(prelude, "w") as out:
        out.write(PRELUDE)
    names = re.findall(r'^bench\("([^"]+)"', open(cases).read(), re.M)
    bases = {name.split("@")[0]: name.split("@")[1] if "@" in name else "" for name in names}
    wanted = {short for short in bases if args.filter in short}
    wanted |= {bases[short] for short in wanted if bases[short]}
    counts = {}
    for index, name in enumerate(names):
        short = name.split("@")[0]
        if short not in wanted:
            continue
        for engine in ("qjs", "nova"):
            report, count = retired(args.bench, engine, prelude, cases, index, max(1, args.runs))
            if "error" in report:
                sys.exit(f"{short} in {engine}: {report['error']}")
            counts[(engine, short)] = (count, report.get("check"))
        if counts[("qjs", short)][1] != counts[("nova", short)][1]:
            sys.exit(f"{short}: the engines disagree: {counts[('qjs', short)][1]} and {counts[('nova', short)][1]}")
    before = json.load(open(args.against)) if args.against else {}
    prices = {}
    print(f"{'instructions per turn':<30}{'qjs':>9}{'nova':>9}{'ratio':>8}")
    for name in names:
        short = name.split("@")[0]
        if short not in wanted:
            continue
        base = bases[short]
        qjs, nova = counts[("qjs", short)][0] / 1e6, counts[("nova", short)][0] / 1e6
        if base:
            qjs -= counts[("qjs", base)][0] / 1e6
            nova -= counts[("nova", base)][0] / 1e6
        prices[short] = {"qjs": round(qjs, 1), "nova": round(nova, 1)}
        ratio = f"{nova / qjs:7.2f}x" if qjs > 0 and nova > 0 else "       -"
        note = f"over {base}" if base else "the whole process, start included"
        was = before.get(short)
        change = f"  was {was['nova']:.1f} ({nova - was['nova']:+.1f})" if was else ""
        print(f"{short:<30}{qjs:9.1f}{nova:9.1f}{ratio}   {note}{change}")
    if args.save:
        with open(args.save, "w") as out:
            json.dump(prices, out, indent=1, sort_keys=True)


if __name__ == "__main__":
    main()
