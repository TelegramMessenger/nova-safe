function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}
const re = /a/g;
check("compile is a function", typeof re.compile, "function");
check("compile length", RegExp.prototype.compile.length, 2);
check("returns the RegExp", re.compile("b", "i"), re);
check("new source", re.source, "b");
check("new flags", re.flags, "i");
check("matches the new pattern", re.test("B") && !re.test("a"), true);
re.lastIndex = 5;
re.compile(/x/gm);
check("from a RegExp", re.source + "/" + re.flags + "/" + re.lastIndex, "x/gm/0");
let threw = "";
try { re.compile(/x/, "g"); } catch (err) { threw = err.constructor.name; }
check("flags with a RegExp", threw, "TypeError");
threw = "";
try { re.compile("("); } catch (err) { threw = err.constructor.name; }
check("bad pattern", threw, "SyntaxError");
check("unchanged after a bad pattern", re.source, "x");
threw = "";
try { RegExp.prototype.compile.call({}, "a"); } catch (err) { threw = err.constructor.name; }
check("not a RegExp", threw, "TypeError");
check("empty", new RegExp("q").compile().source, "(?:)");
