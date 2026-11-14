#!/usr/bin/env python3
"""Print the candidates of an audit round, grouped by function.

Usage: report.py <round-out.json>
"""
import json
import sys


def main():
    result = json.load(open(sys.argv[1]))
    by_fn = {}
    for c in result["candidates"]:
        if c["kind"] not in ("candidate", "closure"):
            continue
        rec = c["rec"]
        key = (rec["file"], rec["line"], rec["fn"])
        by_fn.setdefault(key, []).append(c)
    for (file, line, fn), cands in sorted(by_fn.items()):
        print(f"{file}:{line} {fn}")
        for c in cands:
            print(f"   [{c['rec']['param']}] GC-capable L{c.get('access_line')}: {c.get('access', '')[:90]}")
            print(f"      used after at L{c.get('use_line')}: {c.get('use', '')[:90]}")
    print(f"{len(by_fn)} functions; stats: {json.dumps(result['stats'])}", file=sys.stderr)


if __name__ == "__main__":
    main()
