#!/usr/bin/env python3
"""One audit round: classify cargo-check diagnostics of the injected tree and
edit the tree in place.

- a non-borrowck error on an injected line turns that injection off
  (`let x = x; /*AUDIT-OFF#id*/`) so its body gets borrow-checked again;
- a conflict whose later use is the call that receives `gc` (the bound value
  is an argument of that call: the callee's job) rewrites the value's uses in
  that call's arguments to `x.unbind()`;
- a conflict whose later use comes after the GC-capable access is recorded as
  a candidate.

Usage: iterate.py <check.json> <tree-root> <state.json> <round-out.json>
Prints the number of edits made; 0 means converged.
"""
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from diag import load  # noqa: E402

BORROWCK = {"E0499", "E0502", "E0505", "E0506", "E0597", "E0515", "E0521", "E0716", "E0503", "E0713", "E0373", "E0382", "E0507"}
CONFLICT = {"E0499", "E0502", "E0505", "E0506"}


def main():
    check, root, state_path, out_path = sys.argv[1:5]
    state = json.load(open(state_path))
    diags, files = load(check, root)
    offs = set()
    rewrites = {}  # file -> list of (a, b, name)
    candidates = []
    stats = dict(type_off=0, pass_into_call=0, into_nogc=0, lifetime=0, candidate=0, closure=0, other=0, unattributed=0)
    for d in diags:
        code = d["code"]
        audit_ids = [s["audit"] for s in d["spans"] if s["audit"] is not None]
        if code not in BORROWCK and not (code is None and "lifetime" in d["msg"]):
            if audit_ids:
                for i in audit_ids:
                    offs.add(i)
                stats["type_off"] += 1
            else:
                prim = [s for s in d["spans"] if s["primary"]]
                near = nearest_audit(files, prim[0]) if prim else None
                if near is not None:
                    offs.add(near)
                    stats["type_off"] += 1
                else:
                    stats["unattributed"] += 1
                    candidates.append(dict(kind="unattributed", code=code, msg=d["msg"], where=loc(d)))
            continue
        if code not in CONFLICT:
            stats["lifetime"] += 1
            continue
        primary = [s for s in d["spans"] if s["primary"]]
        if not primary or not audit_ids:
            stats["other"] += 1
            continue
        p = primary[0]
        aid = audit_ids[0]
        rec = state[str(aid)]
        name = "self_audit_" if rec["param"] == "self" else rec["param"]
        pline = files.line_of(p["file"], p["a"]).decode("utf-8", "replace")
        if "into_nogc" in pline and "into_nogc" in files.text(p["file"], p["a"], p["b"] + 12):
            stats["into_nogc"] += 1
            continue
        later = [s for s in d["spans"] if not s["primary"] and s["audit"] is None and ("later" in s["label"] or "argument requires" in s["label"] or "captured" in s["label"])]
        if not later:
            stats["other"] += 1
            continue
        l = later[0]
        lab = l["label"]
        if "by call" in lab or "argument requires" in lab:
            data = files.get(l["file"])
            if "argument requires" in lab:
                lo, hi = l["a"], l["b"]
            else:
                j = l["b"]
                m = re.match(rb"\s*(::\s*<[^()]*>)?\s*\(", data[j:])
                if not m:
                    stats["other"] += 1
                    candidates.append(dict(kind="call-unparsed", id=aid, rec=rec, where=loc(d), label=lab))
                    continue
                open_i = j + m.end() - 1
                close_i = match_paren(data, open_i)
                lo, hi = open_i, close_i
                # method receiver: `name.method(`
                before = data[max(0, l["a"] - 200) : l["a"]]
                rm = re.search(rb"(?<![\w.])" + re.escape(name.encode()) + rb"\s*\.\s*$", before)
                if rm:
                    lo = l["a"] - (len(before) - rm.start())
            rewrites.setdefault(l["file"], []).append((lo, hi, name, aid, p["a"]))
            stats["pass_into_call"] += 1
            continue
        if "closure" in lab:
            stats["closure"] += 1
            candidates.append(dict(kind="closure", id=aid, rec=rec, where=loc(d), access=pline.strip(), use=files.line_of(l["file"], l["a"]).decode("utf-8", "replace").strip(), label=lab, use_line=l["line"]))
            continue
        stats["candidate"] += 1
        candidates.append(
            dict(
                kind="candidate",
                id=aid,
                rec=rec,
                where=loc(d),
                access=pline.strip(),
                access_line=p["line"],
                use=files.line_of(l["file"], l["a"]).decode("utf-8", "replace").strip(),
                use_line=l["line"],
                label=lab,
                code=code,
            )
        )
    edits = 0
    # turn injections off
    if offs:
        for fname in set(state[str(i)]["file"] for i in offs):
            path = os.path.join(root, "nova_vm/src", fname)
            txt = open(path).read()
            for i in offs:
                if state[str(i)]["file"] != fname:
                    continue
                pat = re.compile(r"let (mut )?(\w+) = (\w+)\.bind\(\w+\.nogc\(\)\); /\*AUDIT#" + str(i) + r"\*/")
                txt, n = pat.subn(lambda m: f"let {m.group(1) or ''}{m.group(2)} = {m.group(3)}; /*AUDIT-OFF#{i}*/", txt)
                edits += n
            open(path, "w").write(txt)
    # rewrite arguments
    for fname, items in rewrites.items():
        path = os.path.join(root, fname)
        data = open(path, "rb").read()
        # apply from the end; merge identical ranges
        seen = set()
        for lo, hi, name, aid, acc in sorted(items, key=lambda x: -x[0]):
            if (lo, hi, name) in seen:
                continue
            seen.add((lo, hi, name))
            seg = data[lo:hi]
            new = rewrite_uses(seg, name)
            if new == seg:
                for tv in tainted(data, aid, lo, name):
                    new = rewrite_uses(new, tv)
            if new != seg:
                data = data[:lo] + new + data[hi:]
                edits += 1
            else:
                candidates.append(dict(kind="pass-into-call-derived", id=aid, rec=state[str(aid)], file=fname, seg=seg.decode("utf-8", "replace")[:200]))
        open(path, "wb").write(data)
    json.dump(dict(stats=stats, candidates=candidates, offs=sorted(offs)), open(out_path, "w"), indent=1)
    print(json.dumps(stats), "edits", edits)


def tainted(data, aid, lo, name):
    k = data.find(b"/*AUDIT#" + str(aid).encode() + b"*/")
    if k < 0 or k > lo:
        return []
    body = data[k:lo].decode("utf-8", "replace")
    names = {name}
    out = []
    for m in re.finditer(r"let\s+(?:mut\s+)?([a-z_][a-z0-9_]*)\s*=\s*([^;]*);", body, re.S):
        var, expr = m.group(1), " ".join(m.group(2).split())
        for nm in list(names):
            if re.match(r"^\*?" + re.escape(nm) + r"(\s*\.\s*(get|get_if_present)\([^()]*\)|\[[^\]]*\])?(\s*\.\s*(bind\([^()]*(\([^()]*\))?\)|unbind\(\)))*$", expr):
                if var not in names:
                    names.add(var)
                    out.append(var)
                break
    return out


def nearest_audit(files, span):
    data = files.get(span["file"])
    k = data.rfind(b"/*AUDIT#", 0, span["a"])
    if k < 0:
        return None
    m = re.match(rb"/\*AUDIT#(\d+)\*/", data[k:])
    return int(m.group(1)) if m else None


def loc(d):
    p = [s for s in d["spans"] if s["primary"]]
    if not p:
        return None
    return f'{p[0]["file"]}:{p[0]["line"]}'


def match_paren(data, i):
    depth = 0
    j = i
    while j < len(data):
        c = data[j : j + 1]
        if c in (b"(", b"[", b"{"):
            depth += 1
        elif c in (b")", b"]", b"}"):
            depth -= 1
            if depth == 0:
                return j + 1
        elif c == b'"':
            j += 1
            while j < len(data) and data[j : j + 1] != b'"':
                if data[j : j + 1] == b"\\":
                    j += 1
                j += 1
        j += 1
    return len(data)


ACCESSORS = (b"get", b"get_if_present", b"first", b"iter")


def rewrite_uses(seg, name):
    nb = name.encode()
    out = bytearray()
    i = 0
    pat = re.compile(rb"(?<![\w.:])" + re.escape(nb) + rb"\b(?!\s*:[^:])(?!\s*!)")
    pos = 0
    changed = False
    for m in pat.finditer(seg):
        e = m.end()
        rest = seg[e:]
        if rest.startswith(b".unbind()") or rest.startswith(b".scope(") or rest.startswith(b".len()") or rest.startswith(b".is_empty()"):
            continue
        am = re.match(rb"\.(get|get_if_present)\(", rest)
        out += seg[pos:e]
        if am:
            close = match_paren(seg, e + am.end() - 1)
            out += seg[e:close] + b".unbind()"
            pos = close
        else:
            out += b".unbind()"
            pos = e
        changed = True
    out += seg[pos:]
    return bytes(out) if changed else seg


if __name__ == "__main__":
    main()
