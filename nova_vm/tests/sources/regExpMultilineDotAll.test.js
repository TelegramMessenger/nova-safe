function check(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new Error(name + ": expected " + e + ", got " + a);
  }
}

check("^ without m", /^b/.test("a\nb"), false);
check("^ with m", /^b/m.test("a\nb"), true);
check("$ without m", /a$/.test("a\nb"), false);
check("$ with m", /a$/m.test("a\nb"), true);
check("m lines", "раз\nдва\nтри".match(/^\S+$/gm), ["раз", "два", "три"]);
check("m replace", "a\nb".replace(/^/gm, "> "), "> a\n> b");
check(". without s", /a.b/.test("a\nb"), false);
check(". with m", /a.b/m.test("a\nb"), false);
check(". with s", /a.b/s.test("a\nb"), true);
check(". with ms", /^a.b$/ms.test("a\nb"), true);
check("dotAll getter", [/a/s.dotAll, /a/m.dotAll, /a/m.multiline, /a/s.multiline], [true, false, true, false]);
