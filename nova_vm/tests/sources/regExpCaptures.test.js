function check(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new Error(name + ": expected " + e + ", got " + a);
  }
}

check("accented", Array.from(/(é)(t)/.exec("café été")), ["ét", "é", "t"]);
check("Cyrillic", Array.from(/(\S+) (\S+)/.exec("Привет мир")), ["Привет мир", "Привет", "мир"]);
check("CJK small", Array.from(/(語)/.exec("日本語")), ["語", "語"]);
check("emoji", /(😀+)/u.exec("a😀😀b")[1], "😀😀");
check("optional", Array.from(/(x)?(ё)/.exec("ё")), ["ё", undefined, "ё"]);
check("empty capture after a lone surrogate", Array.from("\uD800x".match(/()x/)), ["x", ""]);
check("empty capture inside a lone surrogate", Array.from("\uD800".match(/(?:)()/)), ["", ""]);
const all = [..."\uD800é\uDC00".matchAll(/()/g)].map((m) => m.index + ":" + m[1]);
check("empty captures around lone surrogates", all, ["0:", "1:", "2:", "3:"]);
