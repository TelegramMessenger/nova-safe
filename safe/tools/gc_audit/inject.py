#!/usr/bin/env python3
"""Inject `let p = p.bind(gc.nogc());` for by-value handle parameters of every
fn taking `GcScope` under a source root, so rustc's borrow checker reports
uses of those parameters after a GC-capable call.

Usage: inject.py <src-root> <state.json> [--skip ids-file]
The state file records every insertion: id, file, fn name, fn line, param.
Insertions listed in the skip file (one id per line) are not made.
"""
import json
import os
import re
import sys

NON_HANDLE = re.compile(
    r"^(bool|u8|u16|u32|u64|u128|usize|i8|i16|i32|i64|i128|isize|f32|f64|char|"
    r"GcScope|NoGcScope|Scoped|ScopedCollection|ScopedArgumentsList|Option<(bool|u\d+|i\d+|usize|isize|f64|f32|char)>|"
    r"impl |dyn |fn\(|Box<dyn|PhantomData|Ordering|ExceptionType|PreferredType|"
    r"ProtoIntrinsics|DynamicFunctionKind|EnumerateObjectsKind|IntegrityLevel|"
    r"&|\*)"
)


def scan_code_mask(text):
    """Return a list of booleans: True where the char is code (not comment/string)."""
    n = len(text)
    mask = [True] * n
    i = 0
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            j = n if j == -1 else j
            for k in range(i, j):
                mask[k] = False
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 1
            j = i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            for k in range(i, j):
                mask[k] = False
            i = j
            continue
        if c == "r" and i + 1 < n and text[i + 1] in "#\"" and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            m = re.match(r'r(#*)"', text[i:])
            if m:
                hashes = m.group(1)
                end = text.find('"' + hashes, i + len(m.group(0)))
                end = n if end == -1 else end + 1 + len(hashes)
                for k in range(i, end):
                    mask[k] = False
                i = end
                continue
        if c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                if text[j] == "\\":
                    j += 1
                j += 1
            for k in range(i, min(j + 1, n)):
                mask[k] = False
            i = j + 1
            continue
        if c == "'":
            if i + 1 < n and text[i + 1] == "\\":
                j = text.find("'", i + 2)
                for k in range(i, j + 1):
                    mask[k] = False
                i = j + 1
                continue
            if i + 2 < n and text[i + 2] == "'":
                for k in range(i, i + 3):
                    mask[k] = False
                i += 3
                continue
        i += 1
    return mask


def match_close(text, mask, i, open_c, close_c):
    depth = 0
    j = i
    n = len(text)
    while j < n:
        if mask[j]:
            if text[j] == open_c:
                depth += 1
            elif text[j] == close_c:
                depth -= 1
                if depth == 0:
                    return j
        j += 1
    return -1


def split_top(s):
    parts = []
    depth = 0
    cur = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "-" and i + 1 < len(s) and s[i + 1] == ">":
            cur.append("->")
            i += 2
            continue
        if c in "([{<":
            depth += 1
        elif c in ")]}>":
            depth -= 1
        if c == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(c)
        i += 1
    if "".join(cur).strip():
        parts.append("".join(cur))
    return parts


def strip_comments(s):
    s = re.sub(r"//[^\n]*", "", s)
    s = re.sub(r"/\*.*?\*/", "", s, flags=re.S)
    return s


FN_RE = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)\s*")


def process_file(path, rel, next_id, skip, records):
    with open(path) as f:
        text = f.read()
    if "GcScope" not in text:
        return next_id, None
    mask = scan_code_mask(text)
    inserts = []
    for m in FN_RE.finditer(text):
        if not mask[m.start()]:
            continue
        name = m.group(1)
        i = m.end()
        generics_names = set()
        if i < len(text) and text[i] == "<":
            gend = match_close(text, mask, i, "<", ">")
            if gend < 0:
                continue
            gtxt = text[i + 1 : gend]
            for part in split_top(gtxt):
                part = part.strip()
                gm = re.match(r"([A-Z][A-Za-z0-9_]*)", part)
                if gm:
                    generics_names.add(gm.group(1))
            i = gend + 1
        while i < len(text) and text[i].isspace():
            i += 1
        if i >= len(text) or text[i] != "(":
            continue
        pend = match_close(text, mask, i, "(", ")")
        if pend < 0:
            continue
        params_txt = strip_comments(text[i + 1 : pend])
        j = pend + 1
        body = -1
        depth = 0
        while j < len(text):
            if mask[j]:
                c = text[j]
                if c == "-" and text[j + 1] == ">":
                    j += 2
                    continue
                if c in "(<[":
                    depth += 1
                elif c in ")>]":
                    depth -= 1
                elif c == ";" and depth <= 0:
                    break
                elif c == "{" and depth <= 0:
                    body = j
                    break
            j += 1
        if body < 0:
            continue
        sig = text[m.start() : body]
        if "$" in sig:
            continue
        params = [p.strip() for p in split_top(params_txt) if p.strip()]
        gc_name = None
        handle_params = []
        self_by_value = None
        for p in params:
            if p in ("self", "mut self"):
                self_by_value = p
                continue
            pm = re.match(r"^(mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+)$", p, re.S)
            if not pm:
                continue
            is_mut, pname, ptype = pm.group(1), pm.group(2), " ".join(pm.group(3).split())
            if ptype.startswith("GcScope"):
                gc_name = pname
                continue
            if pname == "_" or pname == "agent":
                continue
            if NON_HANDLE.match(ptype):
                continue
            base = re.match(r"([A-Za-z_][A-Za-z0-9_]*)", ptype)
            if base and base.group(1) in generics_names:
                continue
            handle_params.append((bool(is_mut), pname, ptype))
        if gc_name is None or gc_name == "_":
            continue
        line_no = text.count("\n", 0, m.start()) + 1
        lines = []
        rename_self = False
        if self_by_value is not None:
            rid = next_id
            next_id += 1
            if str(rid) not in skip:
                mut = "mut " if self_by_value.startswith("mut") else ""
                lines.append(f"let {mut}self_audit_ = self.bind({gc_name}.nogc()); /*AUDIT#{rid}*/")
                records[rid] = dict(file=rel, fn=name, line=line_no, param="self", type="Self")
                rename_self = True
        for is_mut, pname, ptype in handle_params:
            rid = next_id
            next_id += 1
            if str(rid) in skip:
                continue
            mut = "mut " if is_mut else ""
            lines.append(f"let {mut}{pname} = {pname}.bind({gc_name}.nogc()); /*AUDIT#{rid}*/")
            records[rid] = dict(file=rel, fn=name, line=line_no, param=pname, type=ptype)
        arg_lists = [pn for (_m, pn, pt) in handle_params if pt.startswith("ArgumentsList") and str(records and 0) is not None]
        if lines:
            bend = match_close(text, mask, body, "{", "}")
            for pn in arg_lists:
                lines.insert(0, f"let {pn}_audit_len_ = {pn}.len();")
            inserts.append((body, bend, lines, rename_self))
    if not inserts:
        return next_id, None
    # Apply from the end so offsets stay valid; skip nested overlap issues by
    # applying self-renames on the original body range first.
    out = text
    for body, bend, lines, rename_self in sorted(inserts, key=lambda x: -x[0]):
        head = out[: body + 1]
        inner = out[body + 1 : bend]
        tail = out[bend:]
        if rename_self:
            inner = rename_self_tokens(inner)
        for ln in lines:
            mm = re.match(r"let (\w+)_audit_len_ = ", ln)
            if mm:
                pn = mm.group(1)
                inner = re.sub(r"(?<![\w.])" + pn + r"\.len\(\)", pn + "_audit_len_", inner)
                inner = re.sub(r"(?<![\w.])" + pn + r"\.is_empty\(\)", "(" + pn + "_audit_len_ == 0)", inner)
        out = head + "\n" + "\n".join(lines) + inner + tail
    return next_id, out


def rename_self_tokens(inner):
    mask = scan_code_mask(inner)
    res = []
    last = 0
    for m in re.finditer(r"\bself\b", inner):
        if not mask[m.start()]:
            continue
        if m.start() > 0 and inner[m.start() - 1] == "&":
            pass
        res.append(inner[last : m.start()])
        res.append("self_audit_")
        last = m.end()
    res.append(inner[last:])
    return "".join(res)


def main():
    root = sys.argv[1]
    state = sys.argv[2]
    skip = set()
    if "--skip" in sys.argv:
        with open(sys.argv[sys.argv.index("--skip") + 1]) as f:
            skip = set(l.strip() for l in f if l.strip())
    records = {}
    next_id = 0
    changed = 0
    for dirpath, _, files in sorted(os.walk(root)):
        for fn in sorted(files):
            if not fn.endswith(".rs"):
                continue
            path = os.path.join(dirpath, fn)
            rel = os.path.relpath(path, root)
            next_id, out = process_file(path, rel, next_id, skip, records)
            if out is not None:
                with open(path, "w") as f:
                    f.write(out)
                changed += 1
    with open(state, "w") as f:
        json.dump(records, f)
    print(f"files changed {changed}, insertions {len(records)}, ids {next_id}")


if __name__ == "__main__":
    main()
