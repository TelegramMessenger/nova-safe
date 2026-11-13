function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}
function units(s) {
  const out = [];
  for (let i = 0; i < s.length; i++) out.push(s.charCodeAt(i).toString(16));
  return out.join(" ");
}

const lone = "a\uD800bcdefghijk";
check("slice keeps a lone lead", units(lone.slice(0, 5)), "61 d800 62 63 64");
check("substring keeps a lone lead", units(lone.substring(0, 5)), "61 d800 62 63 64");
check("substr keeps a lone lead", units(lone.substr(0, 5)), "61 d800 62 63 64");
check("small slice keeps a lone lead", units("a\uD800b".slice(0, 2)), "61 d800");
check("lone trail", units("a\uDC00bcdefgh".slice(1, 3)), "dc00 62");

check("slice from inside a pair", units("😀abc".slice(1)), "de00 61 62 63");
check("slice to inside a pair", units("😀abc".slice(0, 1)), "d83d");
check("substring from inside a pair", units("😀abc".substring(1, 3)), "de00 61");
check("substr from inside a pair", units("😀abc".substr(1, 2)), "de00 61");
check("both ends inside pairs", units("x😀y😀z".slice(2, 5)), "de00 79 d83d");
check("substring swaps", units("x😀y😀z".substring(5, 2)), "de00 79 d83d");
check("negative start inside a pair", units("😀".slice(-1)), "de00");
check("heap string inside a pair", units("Привет, мир 😀!".substring(12, 13)), "d83d");
check("heap string after a pair", "Привет, мир 😀!".slice(-1), "!");

check("Cyrillic", "Привет, мир 😀!".slice(3, 9), "вет, м");
check("Cyrillic tail", "Привет, мир 😀!".slice(-4), " 😀!");
check("CJK", "日本語のテキスト".slice(2, 5), "語のテ");
check("CJK substr", "日本語のテキスト".substr(-4, 2), "テキ");
check("accents", "Ünïcödé".substring(4, 1), "nïc");
check("whole", "Ünïcödé".slice(0), "Ünïcödé");
check("empty", "Ünïcödé".slice(3, 3), "");
