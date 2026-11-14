function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}

check("i and u", String(/a/iu), "/a/iu");
check("all flags in getter order", String(/a/ysmigd), "/a/dgimsy");
check("v", String(new RegExp("x", "v")), "/x/v");
check("template", `${/b/g}`, "/b/g");
check("concatenation", /c/m + "", "/c/m");
check("matches flags getter", String(/q/gimsuy), "/q/" + /q/gimsuy.flags);
check("slash escaped", String(new RegExp("a/b")), "/a\\/b/");
check("empty pattern", String(new RegExp("")), "/(?:)/");
check("line feed escaped", String(new RegExp("\n")), "/\\n/");
check("toString call", RegExp.prototype.toString.call(/z/s), "/z/s");
