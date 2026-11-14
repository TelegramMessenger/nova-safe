function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}
check("astral before U+FF61", "\u{1F600}" < "｡", true);
check("U+FF61 after astral", "｡" < "\u{1F600}", false);
check("U+E000 vs astral", "" > "\u{10000}", true);
check("ascii", "abc" < "abd", true);
check("prefix", "ab" < "abc", true);
check("equal", "abc" < "abc", false);
check("lone lead before astral trail", "\uD83D" < "\u{1F600}", true);
check("BMP below surrogates", "一" < "\u{1F600}", true);
