function check(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new Error(name + ": expected " + e + ", got " + a);
  }
}

check("template around $&", "abc".replace(/b/, "<$&>"), "a<b>c");
check("$& accented", "éa wörld".replace(/w(ö)rld/g, "<$&>"), "éa <wörld>");
check("$` $1 $'", "éa wörld".replace(/w(ö)rld/, "[$`|$1|$']"), "éa [éa |ö|]");
check("Cyrillic $& $` $'", "Привет мир".replace(/мир/, "<$&|$`|$'>"), "Привет <мир|Привет |>");
check("Cyrillic global $1", "Привет мир мир".replace(/м(и)р/g, "[$1]"), "Привет [и] [и]");
check("CJK $'", "日本語 テキスト".replace(/テ/g, "<$&$'>"), "日本語 <テキスト>キスト");
check("emoji $` $'", "😀 smile 😀".replace(/smile/, "{$`}{$'}"), "😀 {😀 }{ 😀} 😀");
check("$' at the end of a small string", "ééé".replace(/é/g, "$'"), "ééé");
check("$$ $0 $10", "ab".replace(/(b)/, "$$|$0|$10|$2"), "a$|$0|b0|$2");
check("$<name> without groups", "ab".replace(/b/, "$<x>"), "a$<x>");
check(
  "replacer arguments",
  "абв".replace(/б/g, (m, off, str) => m + off + str.length),
  "аб13в",
);
check("lone surrogate in template", "ab".replace(/b/, "\uD83D$&"), "a\uD83Db");
check("template joins a pair", "x\uDE00".replace(/x/, "\uD83D"), "😀");

check("match index", "Привет мир".match(/м(и)р/).index, 7);
check("match global", "мир мир мир".match(/мир/g), ["мир", "мир", "мир"]);
check(
  "matchAll",
  [..."мир1 мир2".matchAll(/мир(\d)/g)].map((m) => m.index + ":" + m[1]),
  ["0:1", "5:2"],
);
check("search", "Привет мир".search(/мир/), 7);
check("search after emoji", "😀😀x".search(/x/), 4);
check("exec index", /(é)(t)/.exec("café été").index, 5);
check("split", "раз, два, три".split(/,\s*/), ["раз", "два", "три"]);
check("split CJK", "日本,語,テ".split(/,/), ["日本", "語", "テ"]);
check("split emoji", "a😀b😀c".split(/😀/), ["a", "b", "c"]);
check("split captures", "раз два".split(/(\s)/), ["раз", " ", "два"]);

function execAll(re, s) {
  const out = [];
  let m;
  while ((m = re.exec(s))) out.push(m.index, re.lastIndex);
  return out;
}
check("global exec", execAll(/д/g, "адбдв"), [1, 2, 3, 4]);
check("global exec emoji", execAll(/😀/gu, "a😀b😀"), [1, 3, 4, 6]);
const sticky = /б/y;
sticky.lastIndex = 1;
check("sticky match", [sticky.exec("абв").index, sticky.lastIndex], [1, 2]);
const stickyMiss = /в/y;
stickyMiss.lastIndex = 1;
check("sticky miss", [stickyMiss.exec("абв"), stickyMiss.lastIndex], [null, 0]);
const past = /x/g;
past.lastIndex = 5;
check("lastIndex past the end", [past.exec("ééé"), past.lastIndex], [null, 0]);
const inside = /é/g;
inside.lastIndex = 2;
check("lastIndex in UTF-16", [inside.exec("ééé").index, inside.lastIndex], [2, 3]);
const testRe = /b/g;
testRe.lastIndex = 3;
check("test lastIndex", [testRe.test("😀ab"), testRe.lastIndex], [true, 4]);

check("empty matches around a lone surrogate", "\uD800".replace(/(?:)/g, "-"), "-\uD800-");
check("empty matches in a heap string", "a\uD800bcdefgh".replace(/(?:)/g, "-").length, 19);
check("empty split around a lone surrogate", "a\uD800bcdefgh".split(/(?:)/).length, 9);
check("empty match count", "\uD800x".match(/(?:)/g).length, 3);
