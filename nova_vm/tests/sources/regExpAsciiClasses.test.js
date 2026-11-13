function check(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new Error(name + ": expected " + e + ", got " + a);
  }
}

check("\\w is ASCII", "приветé_1".match(/\w+/)[0], "_1");
check("\\w is ASCII with u", [..."à1 é2 ö3".matchAll(/(\w)(\d)/gu)].length, 0);
check("\\W takes non-ASCII", "ab, мир".match(/\W+/)[0], ", мир");
check("\\d is ASCII", "٣3".match(/\d/g), ["3"]);
check("\\d is ASCII with u", "٣".match(/\d/u), null);
check("\\D takes non-ASCII digits", "٣3".match(/\D/)[0], "٣");
check("\\b around ASCII words", "мир word".match(/\bw\w+\b/)[0], "word");
check("\\b is not at Cyrillic", /\bмир/.test("мир"), false);
check("\\B", "мир".replace(/\B/g, "-"), "-м-и-р-");
check("class \\w", "é_a".match(/[\w]+/)[0], "_a");
check("class \\d", "٣7".match(/[\d]/)[0], "7");
check("class \\W", "a,b".match(/[\W]/)[0], ",");
check("negated class \\W", "é_".match(/[^\W]/)[0], "_");
check("class \\b is backspace", /[\b]/.test("\b"), true);
check("escaped backslash", /\\d/.test("\\d"), true);
check("\\w with u and i", ["ſ".match(/\w/iu) !== null, "K".match(/\w/iu) !== null], [true, true]);
check("\\w with i only", "ſ".match(/\w/i), null);
check("replace with \\w", "Привет, world!".replace(/\w+/g, "X"), "Привет, X!");
check("split with \\s and \\d", "a1 b2".split(/\d\s?/), ["a", "b", ""]);
