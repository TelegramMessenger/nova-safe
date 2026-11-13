function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + expected + ", got " + actual);
  }
}

check("y", /a/y.flags, "y");
check("gy", /a/gy.flags, "gy");
check("m", /a/m.flags, "m");
check("ms", /a/ms.flags, "ms");
check("all", /a/dgimsuy.flags, "dgimsuy");
check("constructor", new RegExp("a", "ym").flags, "my");
check("v", new RegExp("a", "v").flags, "v");
check(
  "generic",
  Object.getOwnPropertyDescriptor(RegExp.prototype, "flags").get.call({
    hasIndices: true,
    multiline: 1,
    sticky: "yes",
  }),
  "dmy",
);
