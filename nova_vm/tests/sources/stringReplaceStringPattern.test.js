function check(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new Error(name + ": expected " + e + ", got " + a);
  }
}

check("$&", "abc".replace("b", "<$&>"), "a<b>c");
check("$` $'", "abc".replace("b", "[$`|$']"), "a[a|c]c");
check("$$ and $1 stay", "abc".replace("b", "$$|$1|$<x>"), "a$|$1|$<x>c");
check("Cyrillic $&", "Привет мир".replace("мир", "<$&>"), "Привет <мир>");
check("Cyrillic $` $'", "раз два три".replace("два", "[$`|$']"), "раз [раз | три] три");
check("replacer offset", "Привет мир".replace("мир", (m, off, s) => m + off + s.length), "Привет мир710");
check("replacer offset after emoji", "😀x".replace("x", (m, off) => String(off)), "😀2");
check("not found", "Привет".replace("x", "y"), "Привет");
check("empty search", "мир".replace("", "<$&>"), "<>мир");
check("lone surrogate kept", "a\uD800b".replace("b", "c"), "a\uD800c");
check("lone surrogate searched", "a\uD800b".replace("\uD800", "_"), "a_b");
check("trail half", "x😀y".replace("\uDE00", "_"), "x\uD83D_y");
check("lead half", "x😀y".replace("\uD83D", "_"), "x_\uDE00y");
check("template joins a pair", "x\uDE00".replace("x", "\uD83D"), "😀");

check("replaceAll $&", "a-b-c".replaceAll("-", "<$&>"), "a<->b<->c");
check("replaceAll Cyrillic", "мир мир".replaceAll("мир", "[$`]"), "[] [мир ]");
check("replaceAll $'", "ab".replaceAll("a", "$'"), "bb");
check(
  "replaceAll replacer offsets",
  "ёжик ёлка".replaceAll("ё", (m, off) => "<" + off + ">"),
  "<0>жик <5>лка",
);
check("replaceAll empty search", "аб".replaceAll("", "-"), "-а-б-");
check("replaceAll emoji", "😀a😀".replaceAll("😀", "x"), "xax");
check("replaceAll trail halves", "😀😀".replaceAll("\uDE00", "_"), "\uD83D_\uD83D_");
check("replaceAll overlapping", "aaa".replaceAll("aa", "b"), "ba");
