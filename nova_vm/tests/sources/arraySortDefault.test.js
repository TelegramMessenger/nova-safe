function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}

check("numbers sort as strings", [3, 1, 10, 2].sort().join(), "1,10,2,3");
check("toSorted too", [3, 1, 10, 2].toSorted().join(), "1,10,2,3");
check("mixed numbers", [1.5, -1, 10, 9, -0.5, NaN, Infinity, 2e21, 1e-7].sort().join(),
  "-0.5,-1,1.5,10,1e-7,2e+21,9,Infinity,NaN");
check("UTF-16 order", ["｡", "\u{1F600}", "a", "\uD800"].sort()
  .map((s) => s.codePointAt(0).toString(16)).join(), "61,d800,1f600,ff61");
check("other primitives", [true, false, null, "null", 1, 10n, 9n].sort().join(), "1,10,9,false,,null,true");
const holes = [3, undefined, 1, , 2];
holes.sort();
check("undefined then holes", JSON.stringify(holes) + holes.length + (3 in holes) + (4 in holes),
  "[1,2,3,null,null]5truefalse");
const calls = [];
const objects = [{ toString() { calls.push("b"); return "b"; } }, "a", 3];
check("objects through toString", objects.sort().map(String).join(), "3,a,b");
check("toString called", calls.length > 0, true);
let threw = "";
try { [Symbol(), "a"].sort(); } catch (err) { threw = err.constructor.name; }
check("symbol throws", threw, "TypeError");

check("typed arrays sort numerically", new Int8Array([3, 1, 10, 2, -5]).sort().join(), "-5,1,2,3,10");
check("float typed arrays", new Float64Array([3, 1, 10, -0, 0, NaN]).sort().join(), "0,0,1,3,10,NaN");

const stable = Array.from({ length: 100 }, (_, i) => ({ k: i % 3, i }));
stable.sort((a, b) => a.k - b.k);
check("stable", stable.every((x, n) => n === 0 || stable[n - 1].k < x.k || stable[n - 1].i < x.i), true);

for (let round = 0; round < 20; round++) {
  const shuffled = Array.from({ length: 500 }, (_, i) => i).sort(() => Math.random() - 0.5);
  check("random comparator keeps the elements", shuffled.slice().sort((a, b) => a - b).join(),
    Array.from({ length: 500 }, (_, i) => i).join());
  const typed = new Float32Array(500).map((_, i) => i).sort(() => Math.random() - 0.5);
  check("random comparator on a typed array", Array.from(typed).sort((a, b) => a - b)[499], 499);
  Array.from({ length: 300 }, (_, i) => i).sort((a, b) => (a % 7 < b % 3 ? -1 : 1));
}

let compared = 0;
threw = "";
try {
  Array.from({ length: 100 }, (_, i) => i).sort(() => { compared++; throw new RangeError("stop"); });
} catch (err) { threw = err.constructor.name; }
check("comparator error stops the sort", threw + compared, "RangeError1");
