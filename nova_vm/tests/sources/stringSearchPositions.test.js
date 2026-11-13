function check(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new Error(name + ": expected " + e + ", got " + a);
  }
}

const emoji = "😀abc";
check("indexOf inside a pair", emoji.indexOf("a", 1), 2);
check("indexOf empty inside a pair", emoji.indexOf("", 1), 1);
check("indexOf empty past the end", emoji.indexOf("", 9), 5);
check("indexOf trail half", emoji.indexOf("\uDE00"), 1);
check("indexOf trail half and more", emoji.indexOf("\uDE00a"), 1);
check("indexOf lead half", emoji.indexOf("\uD83D"), 0);
check("indexOf whole pair after a pair", "x😀y😀z".indexOf("😀", 2), 4);
check("indexOf Cyrillic", "Привет мир".indexOf("и", 3), 8);
check("indexOf past the end", "Привет мир".indexOf("мир", 100), -1);
check("indexOf lone surrogate", "a\uD800b".indexOf("\uD800"), 1);

check("lastIndexOf inside a pair", emoji.lastIndexOf("a", 1), -1);
check("lastIndexOf trail half", emoji.lastIndexOf("\uDE00"), 1);
check("lastIndexOf lead half", "😀abc😀".lastIndexOf("\uD83D"), 5);
check("lastIndexOf past the end", "Привет мир".lastIndexOf("и", 100), 8);
check("lastIndexOf Cyrillic", "Привет мир".lastIndexOf("и", 4), 2);
check("lastIndexOf before a two-byte char", "aéb".lastIndexOf("x", 1), -1);
check("lastIndexOf at a two-byte char", "aéb".lastIndexOf("é", 1), 1);
check("lastIndexOf empty", "aéb".lastIndexOf("", 1), 1);
check("lastIndexOf pair inside a pair", "x😀y😀z".lastIndexOf("😀", 3), 1);
check("lastIndexOf NaN position", "abab".lastIndexOf("ab", NaN), 2);

check("includes inside a pair", emoji.includes("a", 1), true);
check("includes trail half", emoji.includes("\uDE00"), true);
check("includes Cyrillic from 1", "Привет".includes("П", 1), false);
check("includes Cyrillic", "Привет".includes("ри", 1), true);
check("includes CJK", "日本語".includes("語", 2), true);

check("startsWith inside a pair", emoji.startsWith("a", 1), false);
check("startsWith trail half", emoji.startsWith("\uDE00a", 1), true);
check("startsWith lead half", emoji.startsWith("\uD83D"), true);
check("startsWith Cyrillic", "Привет".startsWith("ри", 1), true);
check("startsWith past the end", "Привет".startsWith("ри", 100), false);
check("startsWith empty past the end", "Привет".startsWith("", 100), true);

check("endsWith inside a pair", emoji.endsWith("a", 1), false);
check("endsWith lead half", emoji.endsWith("\uD83D", 1), true);
check("endsWith trail half", "😀".endsWith("\uDE00"), true);
check("endsWith Cyrillic", "Привет".endsWith("ив", 4), true);
check("endsWith past the end", "Привет".endsWith("ет", 100), true);

check("split by a pair", "a😀b😀c".split("😀"), ["a", "b", "c"]);
check("split Cyrillic", "раз,два,три".split(","), ["раз", "два", "три"]);
check("split limit", "раз два".split(" ", 1), ["раз"]);
check("split code units", "😀".split("").map((u) => u.charCodeAt(0)), [0xd83d, 0xde00]);
check("split code units limit", "abc".split("", 2), ["a", "b"]);
check("split keeps lone surrogates", "a\uD800b".split("").map((u) => u.charCodeAt(0)), [0x61, 0xd800, 0x62]);
check("split by a trail half", "x😀y".split("\uDE00"), ["x\uD83D", "y"]);
check("split by a lead half", "x😀y".split("\uD83D"), ["x", "\uDE00y"]);
check("split by a lone surrogate", "a\uD800b".split("\uD800"), ["a", "b"]);
check("split empty string", "".split(","), [""]);
check("split adjacent separators", "a,,b,".split(","), ["a", "", "b", ""]);
