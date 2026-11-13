import json
import re
import sys
from collections import defaultdict

OPEN = "([{"
CLOSE = ")]}"


def skip_trivia(s, i):
    while i < len(s):
        if s[i].isspace():
            i += 1
        elif s.startswith("//", i):
            j = s.find("\n", i)
            i = len(s) if j < 0 else j + 1
        elif s.startswith("/*", i):
            i = skip_block_comment(s, i)
        else:
            break
    return i


def skip_block_comment(s, i):
    depth = 0
    while i < len(s):
        if s.startswith("/*", i):
            depth += 1
            i += 2
        elif s.startswith("*/", i):
            depth -= 1
            i += 2
            if depth == 0:
                return i
        else:
            i += 1
    return i


RAW_STRING = re.compile(r'b?r(#*)"')


def skip_literal(s, i):
    m = RAW_STRING.match(s, i)
    if m and (i == 0 or not (s[i - 1].isalnum() or s[i - 1] == "_")):
        end = '"' + m.group(1)
        j = s.find(end, m.end())
        return j + len(end)
    if s[i] == '"' or (s[i] == "b" and s.startswith('b"', i)):
        j = i + (2 if s[i] == "b" else 1)
        while s[j] != '"':
            j += 2 if s[j] == "\\" else 1
        return j + 1
    if s[i] == "'":
        if s[i + 1] == "\\":
            j = i + 2
            while s[j] != "'":
                j += 1
            return j + 1
        if i + 2 < len(s) and s[i + 2] == "'":
            return i + 3
        return i + 1
    return None


def match_brace(s, i):
    assert s[i] == "{"
    depth = 0
    while i < len(s):
        if s.startswith("//", i) or s.startswith("/*", i):
            i = skip_trivia(s, i)
            continue
        c = s[i]
        if c in "\"'rb":
            j = skip_literal(s, i)
            if j is not None and j > i + (0 if c in "\"'" else 1):
                i = j
                continue
        if c in OPEN:
            depth += 1
        elif c in CLOSE:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise ValueError("unbalanced")


def top_level(body):
    out = []
    depth = 0
    i = 0
    while i < len(body):
        if body.startswith("//", i) or body.startswith("/*", i):
            i = skip_trivia(body, i)
            out.append(" ")
            continue
        c = body[i]
        if c in "\"'rb":
            j = skip_literal(body, i)
            if j is not None and j > i + (0 if c in "\"'" else 1):
                if depth == 0:
                    out.append("L")
                i = j
                continue
        if c in OPEN:
            if depth == 0:
                out.append(c)
            depth += 1
        elif c in CLOSE:
            depth -= 1
            if depth == 0:
                out.append(c)
        elif depth == 0:
            out.append(c)
        i += 1
    return "".join(out)


TURBOFISH = re.compile(r"::<[^<>]*(?:<[^<>]*>[^<>]*)*>")
POSTFIX = re.compile(r"^[A-Za-z0-9_:.\s?()\[\]{}'L]*$")


def is_postfix_expression(body):
    flat = TURBOFISH.sub("", top_level(body)).strip()
    if not flat or ";" in flat:
        return False
    if re.search(r"\b(let|if|match|loop|while|for|return|as|move)\b", flat):
        return False
    flat = re.sub(r"[A-Za-z_][A-Za-z0-9_]*!\s*[(\[{]", "f(", flat)
    return bool(POSTFIX.match(flat))


def previous_token(s, i):
    j = i - 1
    while j >= 0:
        if s[j].isspace():
            j -= 1
            continue
        line_start = s.rfind("\n", 0, j + 1) + 1
        line = s[line_start : j + 1]
        k = line.find("//")
        if k >= 0 and not ('"' in line[:k]):
            j = line_start + k - 1
            continue
        break
    if j < 0:
        return ""
    if s[j] == ">" and j > 0 and s[j - 1] == "=":
        return "=>"
    return s[j]


def safety_comment_start(s, unsafe_pos):
    line_start = s.rfind("\n", 0, unsafe_pos) + 1
    lines_start = line_start
    cursor = line_start
    first_safety = None
    while cursor > 0:
        prev_end = cursor - 1
        prev_start = s.rfind("\n", 0, prev_end) + 1
        text = s[prev_start:prev_end].strip()
        if not text.startswith("//") or text.startswith("///"):
            break
        if re.match(r"//\s*SAFETY\b", text):
            first_safety = prev_start
        cursor = prev_start
    if first_safety is None:
        return None
    return first_safety, lines_start


def strip_leading_safety(body):
    return re.sub(r"^\s*(//\s*SAFETY\b[^\n]*\n(\s*//(?!/)[^\n]*\n)*)", "\n", body)


def rewrite(s, positions):
    for pos in sorted(set(positions), reverse=True):
        assert s.startswith("unsafe", pos), (pos, s[pos : pos + 20])
        brace = skip_trivia(s, pos + len("unsafe"))
        if s[brace] != "{":
            continue
        end = match_brace(s, brace)
        body = strip_leading_safety(s[brace + 1 : end])
        if is_postfix_expression(body):
            replacement = " ".join(line.strip() for line in body.strip().splitlines())
            prev = previous_token(s, pos)
            after = skip_trivia(s, end + 1)
            nxt = s[after] if after < len(s) else ""
            if prev == "=>" and nxt not in ",}":
                replacement += ","
            elif prev in (";", "{", "}", "") and nxt not in "};)],.?":
                replacement += ";"
        else:
            replacement = "{" + body + "}"
        s = s[:pos] + replacement + s[end + 1 :]
        comment = safety_comment_start(s, pos)
        if comment:
            first, code_line = comment
            s = s[:first] + s[code_line:]
    return s


def main():
    by_file = defaultdict(list)
    for line in open(sys.argv[1], encoding="utf-8"):
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-message":
            continue
        diagnostic = message["message"]
        code = diagnostic.get("code") or {}
        if code.get("code") != "unused_unsafe":
            continue
        for span in diagnostic["spans"]:
            if span["is_primary"]:
                by_file[span["file_name"]].append(span["byte_start"])
    for path, positions in sorted(by_file.items()):
        raw = open(path, "rb").read()
        text = raw.decode("utf-8")
        char_positions = [len(raw[:b].decode("utf-8")) for b in positions]
        open(path, "w", encoding="utf-8").write(rewrite(text, char_positions))
        print(f"{len(set(positions)):4d} {path}")


if __name__ == "__main__":
    main()
