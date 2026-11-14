#!/usr/bin/env python3
"""Load cargo JSON diagnostics into simple records with the AUDIT ids they touch."""
import json
import os
import re
import sys

AUDIT_RE = re.compile(rb"/\*AUDIT#(\d+)\*/")


class Files:
    def __init__(self, root):
        self.root = root
        self.cache = {}

    def get(self, name):
        if name not in self.cache:
            p = os.path.join(self.root, name)
            try:
                with open(p, "rb") as f:
                    self.cache[name] = f.read()
            except OSError:
                self.cache[name] = b""
        return self.cache[name]

    def line_of(self, name, byte):
        data = self.get(name)
        s = data.rfind(b"\n", 0, byte) + 1
        e = data.find(b"\n", byte)
        e = len(data) if e == -1 else e
        return data[s:e]

    def text(self, name, a, b):
        return self.get(name)[a:b].decode("utf-8", "replace")


def load(path, root):
    files = Files(root)
    out = []
    for l in open(path):
        try:
            d = json.loads(l)
        except ValueError:
            continue
        if d.get("reason") != "compiler-message":
            continue
        m = d["message"]
        if m["level"] != "error":
            continue
        spans = []
        for sp in m["spans"]:
            line = files.line_of(sp["file_name"], sp["byte_start"])
            am = AUDIT_RE.search(line)
            spans.append(
                dict(
                    file=sp["file_name"],
                    a=sp["byte_start"],
                    b=sp["byte_end"],
                    line=sp["line_start"],
                    label=sp.get("label") or "",
                    primary=sp["is_primary"],
                    audit=int(am.group(1)) if am else None,
                    text=files.text(sp["file_name"], sp["byte_start"], sp["byte_end"]),
                )
            )
        out.append(
            dict(
                code=(m.get("code") or {}).get("code"),
                msg=m["message"],
                spans=spans,
                rendered=m.get("rendered", ""),
            )
        )
    return out, files


if __name__ == "__main__":
    diags, _ = load(sys.argv[1], sys.argv[2])
    want = sys.argv[3] if len(sys.argv) > 3 else None
    k = int(sys.argv[4]) if len(sys.argv) > 4 else 3
    shown = 0
    for d in diags:
        if want and str(d["code"]) != want:
            continue
        print(d["rendered"])
        shown += 1
        if shown >= k:
            break
