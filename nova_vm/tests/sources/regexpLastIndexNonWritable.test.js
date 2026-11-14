function check(name, actual, expected) {
  if (!Object.is(actual, expected)) {
    throw new Error(name + ": expected " + String(expected) + ", got " + String(actual));
  }
}
function errorName(f) {
  try {
    f();
    return "none";
  } catch (e) {
    return e.name;
  }
}
function frozenLastIndex(source, flags, value) {
  var r = new RegExp(source, flags);
  Object.defineProperty(r, "lastIndex", { writable: false, value: value });
  return r;
}

check("global exec match", errorName(() => frozenLastIndex("a", "g", 0).exec("aaa")), "TypeError");
check("global exec no match", errorName(() => frozenLastIndex("b", "g", 0).exec("aaa")), "TypeError");
check("global exec past the end", errorName(() => frozenLastIndex("a", "g", 9).exec("aaa")), "TypeError");
check("sticky test match", errorName(() => frozenLastIndex("a", "y", 0).test("aaa")), "TypeError");
check("sticky test no match", errorName(() => frozenLastIndex("b", "y", 0).test("aaa")), "TypeError");
check("global test past the end", errorName(() => frozenLastIndex("a", "g", 9).test("aaa")), "TypeError");
var r = frozenLastIndex("a", "g", 1);
errorName(() => r.exec("aaa"));
check("lastIndex unchanged", r.lastIndex, 1);
check("non-global exec", frozenLastIndex("a", "", 0).exec("ba").index, 1);
check("non-global test", frozenLastIndex("a", "", 0).test("ba"), true);
check("frozen global replace", errorName(() => "aa".replace(Object.freeze(/a/g), "b")), "TypeError");
var writable = /a/g;
check("writable exec", writable.exec("ba").index + "," + writable.lastIndex, "1,2");
