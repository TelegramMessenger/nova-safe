import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import subprocess
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
DRIVER = open(os.path.join(HERE, "driver.js"), encoding="utf-8").read()
NUMBER = re.compile(r"-?\d+(?:\.\d+)?(?:e[-+]?\d+)?")

HEADERS = {
    "nova": (
        "globalThis.__evalScript = __nova__.evalScript;\n"
        "globalThis.__emit = ((p) => (s) => p(s))(print);\n"
        "globalThis.__BUNDLE_SRC = readTextFile({bundle});\n"
        "globalThis.__WIDGET_HTML = readTextFile({widget});\n"
        "for (const k of ['print', 'readTextFile', 'now', '__nova__']) delete globalThis[k];\n"
    ),
    "qjs": (
        "globalThis.__evalScript = ((m) => (s) => m.evalScript(s))(std);\n"
        "globalThis.__emit = ((p) => (s) => p(s))(print);\n"
        "globalThis.__BUNDLE_SRC = std.loadFile({bundle});\n"
        "globalThis.__WIDGET_HTML = std.loadFile({widget});\n"
        "for (const k of ['navigator', 'console', 'print', 'scriptArgs', 'std', 'os', 'bjson']) delete globalThis[k];\n"
    ),
}


def command(engine, binary, script):
    if engine == "nova":
        return [binary, "eval", "--no-strict", "--expose-internals", script]
    return [binary, "--std", "-C", script]


def run_one(engine, binary, bundle, widget, workdir, timeout):
    header = HEADERS[engine].format(bundle=json.dumps(bundle), widget=json.dumps(widget))
    fd, script = tempfile.mkstemp(suffix=".js", dir=workdir)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(header + DRIVER)
    started = time.monotonic()
    try:
        proc = subprocess.run(command(engine, binary, script), capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return {"status": "timeout", "seconds": timeout}
    finally:
        os.unlink(script)
    seconds = time.monotonic() - started
    stdout = proc.stdout.decode("utf-8", "replace")
    stderr = proc.stderr.decode("utf-8", "replace")
    result = None
    for line in reversed(stdout.splitlines()):
        if line.startswith("{"):
            try:
                result = json.loads(line)
                break
            except json.JSONDecodeError:
                continue
    if proc.returncode < 0:
        return {"status": "signal", "signal": -proc.returncode, "stderr": stderr[-2000:], "seconds": seconds}
    if result is None:
        return {"status": "no-output", "code": proc.returncode, "stderr": stderr[-2000:], "stdout": stdout[-500:], "seconds": seconds}
    if "fatal" in result:
        return {"status": "fatal", "fatal": result["fatal"], "stderr": stderr[-2000:], "seconds": seconds}
    return {"status": "ok", "out": result["out"], "tasks": result["tasks"], "seconds": seconds, "stderr": stderr[-500:]}


def same_number(p, q):
    return p == q or abs(p - q) <= 1e-9 * max(abs(p), abs(q)) or (p != p and q != q)


def same_string(x, y):
    if x == y:
        return True
    mx, my = NUMBER.findall(x), NUMBER.findall(y)
    if len(mx) != len(my) or not mx or NUMBER.sub("#", x) != NUMBER.sub("#", y):
        return False
    return all(same_number(float(a), float(b)) for a, b in zip(mx, my))


def same(a, b):
    if isinstance(a, bool) or isinstance(b, bool):
        return a == b
    if isinstance(a, (int, float)) and isinstance(b, (int, float)):
        return same_number(float(a), float(b))
    if isinstance(a, str) and isinstance(b, str):
        return same_string(a, b)
    if isinstance(a, list) and isinstance(b, list):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    if isinstance(a, dict) and isinstance(b, dict):
        return a.keys() == b.keys() and all(same(a[k], b[k]) for k in a)
    return a == b


def first_difference(a, b):
    for i, (x, y) in enumerate(zip(a, b)):
        if not same(x, y):
            return i
    return min(len(a), len(b)) if len(a) != len(b) else None


def compare(widget, nova, qjs):
    entry = {"widget": widget, "nova": nova["status"], "qjs": qjs["status"],
             "novaSeconds": round(nova.get("seconds", 0), 3), "qjsSeconds": round(qjs.get("seconds", 0), 3)}
    if nova["status"] != "ok" or qjs["status"] != "ok":
        entry["verdict"] = "nova-failed" if qjs["status"] == "ok" else ("both-failed" if nova["status"] != "ok" else "qjs-failed")
        for name, run in (("novaDetail", nova), ("qjsDetail", qjs)):
            if run["status"] != "ok":
                entry[name] = {k: v for k, v in run.items() if k not in ("out",)}
        return entry
    a, b = nova["out"], qjs["out"]
    entry["ops"] = sum(1 for item in b if item[0] == "ops")
    index = first_difference(a, b)
    if index is None:
        entry["verdict"] = "same"
        return entry
    first_ops = next((i for i, item in enumerate(b) if item[0] == "ops"), None)
    entry["verdict"] = "differ"
    entry["firstOpsSame"] = first_ops is not None and first_ops < index
    entry["at"] = index
    entry["novaItem"] = json.dumps(a[index] if index < len(a) else None)[:600]
    entry["qjsItem"] = json.dumps(b[index] if index < len(b) else None)[:600]
    entry["lengths"] = [len(a), len(b)]
    return entry


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--nova", required=True)
    parser.add_argument("--qjs")
    parser.add_argument("--record", help="run Nova only and store a hash of each widget's output")
    parser.add_argument("--expect", help="run Nova only and compare with a --record file")
    parser.add_argument("--bundle", required=True)
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 8)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--report")
    parser.add_argument("widgets", nargs="+")
    args = parser.parse_args()
    widgets = sorted({os.path.abspath(w) for w in args.widgets})
    workdir = tempfile.mkdtemp(prefix="nova-widgets-")
    results = []
    if args.record or args.expect:
        return nova_only(args, widgets, workdir)

    def job(widget):
        nova = run_one("nova", args.nova, args.bundle, widget, workdir, args.timeout)
        qjs = run_one("qjs", args.qjs, args.bundle, widget, workdir, args.timeout)
        return compare(widget, nova, qjs)

    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for entry in pool.map(job, widgets):
            results.append(entry)
    counts = {}
    for entry in results:
        counts[entry["verdict"]] = counts.get(entry["verdict"], 0) + 1
    nova_time = sum(e["novaSeconds"] for e in results)
    qjs_time = sum(e["qjsSeconds"] for e in results)
    summary = {"widgets": len(results), "verdicts": counts, "novaSeconds": round(nova_time, 1), "qjsSeconds": round(qjs_time, 1)}
    json.dump({"summary": summary, "results": results}, open(args.report, "w"), indent=1)
    print(json.dumps(summary))


def fingerprint(run):
    if run["status"] != "ok":
        return run["status"] + ": " + str(run.get("fatal") or run.get("signal") or run.get("code"))
    return hashlib.sha256(json.dumps(run["out"], sort_keys=True).encode()).hexdigest()


def nova_only(args, widgets, workdir):
    if args.expect:
        expected = json.load(open(args.expect))
        widgets = [w for w in widgets if w in expected and not expected[w].startswith("timeout")]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        runs = list(pool.map(lambda w: run_one("nova", args.nova, args.bundle, w, workdir, args.timeout), widgets))
    prints = {w: fingerprint(r) for w, r in zip(widgets, runs)}
    seconds = round(sum(r.get("seconds", 0) for r in runs), 1)
    if args.record:
        json.dump(prints, open(args.record, "w"), indent=1, sort_keys=True)
        print(json.dumps({"recorded": len(prints), "ok": sum(1 for r in runs if r["status"] == "ok"), "novaSeconds": seconds}))
        return
    changed = [w for w in widgets if w in expected and expected[w] != prints[w]]
    for w in changed:
        print(f"CHANGED {w}: {expected[w][:80]} -> {prints[w][:200]}")
    print(json.dumps({"widgets": len(widgets), "changed": len(changed), "novaSeconds": seconds}))
    if changed:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
