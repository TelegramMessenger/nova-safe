function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + expected + ", got " + actual);
  }
}

const lead = "\uD83D";
const trail = "\uDE00";

check("small + small", lead + trail, "😀");
check("small + small length", (lead + trail).length, 2);
check("small + small code point", (lead + trail).codePointAt(0), 0x1f600);
check("small + small code points", [...(lead + trail)].length, 1);
check("small + small to heap size", "ab" + lead + trail, "ab😀");
check("joined fits a small string", ("ab" + lead) + trail, "ab😀");
check("joined heap string", ("abcdefgh" + lead) + trail, "abcdefgh😀");
check("non-ASCII before", ("é" + lead) + trail, "é😀");
check("trail then text", lead + (trail + "y"), "😀y");
check("concat", lead.concat(trail), "😀");
check("concat many", "x".concat(lead, trail, "y"), "x😀y");
check("template", `${lead}${trail}`, "😀");
check("join", [lead, trail].join(""), "😀");
let built = "";
for (const unit of [0xd83d, 0xde00]) built += String.fromCharCode(unit);
check("+= fromCharCode", built, "😀");
check("sliced halves join back", "😀abc".slice(0, 1) + "😀abc".slice(1, 2), "😀");
check("lone lead stays", (lead + "a").charCodeAt(0), 0xd83d);
check("two leads stay", (lead + lead).length, 2);
check("trail + lead stay apart", [...(trail + lead)].length, 2);
