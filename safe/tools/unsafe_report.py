import os
import re
import sys

ROOTS = ["nova_vm/src", "small_string"]
KINDS = {
    "block": re.compile(r"\bunsafe\s*\{"),
    "fn": re.compile(r"\bunsafe\s+(?:extern\s+\"[^\"]*\"\s+)?fn\b"),
    "impl": re.compile(r"\bunsafe\s+impl\b"),
    "trait": re.compile(r"\bunsafe\s+trait\b"),
}


def strip_comments(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return "\n".join(line.split("//", 1)[0] for line in text.splitlines())


def scan(base):
    per_file = {}
    for root in ROOTS:
        for dirpath, _, files in os.walk(os.path.join(base, root)):
            for name in files:
                if not name.endswith(".rs"):
                    continue
                path = os.path.join(dirpath, name)
                code = strip_comments(open(path, encoding="utf-8").read())
                counts = {kind: len(rx.findall(code)) for kind, rx in KINDS.items()}
                if any(counts.values()):
                    per_file[os.path.relpath(path, base)] = counts
    return per_file


def main():
    base = sys.argv[1] if len(sys.argv) > 1 else "."
    per_file = scan(base)
    totals = {kind: sum(c[kind] for c in per_file.values()) for kind in KINDS}
    if "--files" in sys.argv:
        for path, counts in sorted(per_file.items(), key=lambda item: -sum(item[1].values())):
            print(f"{sum(counts.values()):5d}  {path}  " + " ".join(f"{k}={v}" for k, v in counts.items() if v))
    print("total", sum(totals.values()), " ".join(f"{k}={v}" for k, v in totals.items()), "files", len(per_file))


main()
