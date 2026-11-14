import json
import os
import re
import subprocess
import sys

SKIP_DIRS = {"tests", "benches", "examples", "target"}
UNSAFE = re.compile(r"\bunsafe\b")
TRUSTED = {"hashbrown"}


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return "\n".join(line.split("//", 1)[0] for line in text.splitlines())


def count_unsafe(root):
    total = 0
    for dirpath, dirnames, files in os.walk(root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in files:
            if name.endswith(".rs"):
                with open(os.path.join(dirpath, name), encoding="utf-8", errors="replace") as f:
                    total += len(UNSAFE.findall(strip_comments(f.read())))
    return total


def linked_crates(base, package, args):
    out = subprocess.run(
        ["cargo", "tree", "-p", package, "-e", "normal,no-proc-macro", "--prefix", "none", *args],
        cwd=base, check=True, capture_output=True, text=True,
    ).stdout
    crates = set()
    for line in out.splitlines():
        parts = line.replace(" (*)", "").split()
        if len(parts) >= 2:
            crates.add((parts[0], parts[1].lstrip("v")))
    return crates


def manifests(base):
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=base, check=True, capture_output=True, text=True,
    ).stdout)
    return {(p["name"], p["version"]): (os.path.dirname(p["manifest_path"]), p["source"]) for p in meta["packages"]}


def main():
    argv = sys.argv[1:]
    base = argv[0] if argv and not argv[0].startswith("-") else "."
    tree_args = ["--no-default-features", "--features", "widget"]
    if "--default-features" in argv:
        tree_args = []
    base = os.path.abspath(base)
    known = manifests(base)
    rows = []
    for name, version in linked_crates(base, "nova_vm", tree_args):
        path, source = known[(name, version)]
        if source is None:
            rel = os.path.relpath(path, base)
            kind = "vendored" if rel.startswith("oxc" + os.sep) else "workspace"
        else:
            kind = "registry"
        rows.append((count_unsafe(os.path.join(path, "src") if os.path.isdir(os.path.join(path, "src")) else path), name, version, kind))
    rows.sort(key=lambda r: (-r[0], r[1]))
    if "--markdown" in argv:
        print("| crate | version | source | unsafe |")
        print("|---|---|---|---:|")
        for n, name, version, kind in rows:
            print(f"| {name} | {version} | {kind} | {n} |")
    else:
        for n, name, version, kind in rows:
            print(f"{n:6d}  {name} {version} ({kind})")
    groups = {}
    for n, _, _, kind in rows:
        g = groups.setdefault(kind, [0, 0, 0])
        g[0] += 1
        g[1] += 1 if n else 0
        g[2] += n
    for kind, (crates, with_unsafe, n) in sorted(groups.items()):
        print(f"{kind}: {crates} crates, {with_unsafe} with unsafe, {n} unsafe")
    trusted = sum(r[0] for r in rows if r[1] in TRUSTED)
    total = sum(r[0] for r in rows)
    print(f"total {total} unsafe in {len(rows)} crates ({sum(1 for r in rows if r[0])} with unsafe); without std's hashbrown: {total - trusted}")


main()
