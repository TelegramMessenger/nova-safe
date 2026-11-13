import os
import re
import sys

sys.path.insert(0, os.path.dirname(__file__))
from strip_unused_unsafe import match_brace, skip_trivia  # noqa: E402

UNSAFE_FN = re.compile(r"\bunsafe\s+fn\s+([A-Za-z_][A-Za-z0-9_]*)")


def body_span(src, start):
    depth = 0
    i = start
    while i < len(src):
        c = src[i]
        if c in "(<[":
            depth += 1
        elif c in ")>]":
            if not (c == ">" and src[i - 1] == "-"):
                depth -= 1
        elif c == ";" and depth == 0:
            return None
        elif c == "{" and depth == 0:
            return i, match_brace(src, i)
        i += 1
    return None


for root in ("nova_vm/src", "small_string"):
    for dirpath, _, files in os.walk(root):
        for name in files:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(dirpath, name)
            src = open(path, encoding="utf-8").read()
            for m in UNSAFE_FN.finditer(src):
                line_start = src.rfind("\n", 0, m.start()) + 1
                if src[line_start:m.start()].lstrip().startswith("//"):
                    continue
                span = body_span(src, m.end())
                if not span:
                    continue
                body = src[span[0] + 1 : span[1]]
                uses = len(re.findall(r"\bunsafe\b", re.sub(r"//[^\n]*", "", body)))
                line = src.count("\n", 0, m.start()) + 1
                kind = "contract-only" if uses == 0 else f"has-unsafe({uses})"
                print(f"{kind:15s} {path}:{line} {m.group(1)}")
