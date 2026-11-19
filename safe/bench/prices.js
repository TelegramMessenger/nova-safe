// The cases of the price list (safe/bench/prices.py): each is a loop of a million turns, run once. A case
// named "x@y" is the case "y" with one statement changed: it does one operation more in every turn, or one
// in place of the cheapest of its kind (a property read in place of the read of a local, an assignment to
// a global in place of one to a local), so the instructions the two processes retire differ by a million
// times the price of that operation. The loops are spelled out in every case, so that nothing but the
// operation differs: a shared helper would be a call in every turn.
bench("base/loop@", function () { return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = i & 7; return last; }; });
bench("base/assign@", function () { return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = i; return last; }; });
bench("call/0-args@base/loop", function () { function f() { return 1; } return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = f(); return last; }; });
bench("call/1-arg@base/loop", function () { function f(x) { return x; } return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = f(i & 7); return last; }; });
bench("call/2-args@base/loop", function () { function f(a, b) { return a; } return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = f(i, 7); return last; }; });
bench("call/method@base/loop", function () { var o = { m: function (x) { return x; } }; return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = o.m(i & 7); return last; }; });
bench("call/method-this@base/loop", function () { var o = { n: 1, m: function (x) { return this.n; } }; return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = o.m(i & 7); return last; }; });
bench("call/proto-method@base/loop", function () { function P() {} P.prototype.m = function (x) { return x; }; var o = new P(); return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = o.m(i & 7); return last; }; });
bench("call/closure-1-arg@base/loop", function () { function make(step) { return function (x) { return x; }; } var f = make(2); return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = f(i & 7); return last; }; });
bench("closure/read@call/closure-1-arg", function () { function make(step) { return function (x) { return step; }; } var f = make(2); return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = f(i & 7); return last; }; });
bench("closure/write@call/closure-1-arg", function () { function make(step) { var total = 0; return function (x) { total = x; return x; }; } var f = make(2); return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = f(i & 7); return last; }; });
bench("closure/read-in-loop@base/assign", function () { var step = 3; return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = step; return last; }; });
bench("closure/write-in-loop@base/loop", function () { var total = 0; return function () { var last = 0; for (var i = 0; i < 1000000; i++) total = i & 7; return total; }; });
bench("global/read@base/assign", function () { globalThis.gPrice = 3; return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = gPrice; return last; }; });
bench("global/write@base/loop", function () { globalThis.gPrice2 = 3; return function () { var last = 0; for (var i = 0; i < 1000000; i++) gPrice2 = i & 7; return gPrice2; }; });
bench("prop/get@base/assign", function () { var o = { a: 1, b: 2 }; return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = o.b; return last; }; });
bench("prop/set@base/loop", function () { var o = { a: 1, b: 2 }; return function () { var last = 0; for (var i = 0; i < 1000000; i++) o.b = i & 7; return o.b; }; });
bench("prop/get-proto@base/assign", function () { function P() {} P.prototype.k = 5; var o = new P(); return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = o.k; return last; }; });
bench("prop/getter@base/assign", function () { var o = { get g() { return 1; } }; return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = o.g; return last; }; });
bench("prop/setter@base/loop", function () { var o = { v: 0, set s(x) { } }; return function () { var last = 0; for (var i = 0; i < 1000000; i++) o.s = i & 7; return last; }; });
bench("base/float-loop@", function () { return function () { var x = 0.5; for (var i = 0; i < 1000000; i++) x = 1.5; return x; }; });
bench("arith/int-add@base/loop", function () { return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = (last + i) & 1023; return last; }; });
bench("arith/float-mul-add@base/float-loop", function () { return function () { var x = 0.5; for (var i = 0; i < 1000000; i++) x = x * 0.999 + 0.25; return x; }; });
bench("math/floor@base/loop", function () { return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = Math.floor(i / 3); return last; }; });
bench("math/sqrt@base/float-loop", function () { return function () { var x = 0.5; for (var i = 0; i < 1000000; i++) x = Math.sqrt(i); return x; }; });
bench("math/sin@base/float-loop", function () { return function () { var x = 0.5; for (var i = 0; i < 1000000; i++) x = Math.sin(i); return x; }; });
bench("array/get@base/loop", function () { var a = []; for (var k = 0; k < 256; k++) a.push(k); return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = a[i & 255]; return last; }; });
bench("array/set@base/loop", function () { var a = []; for (var k = 0; k < 256; k++) a.push(k); return function () { var last = 0; for (var i = 0; i < 1000000; i++) a[i & 255] = i; return a[5]; }; });
bench("typed/get-u8@base/loop", function () { var a = new Uint8Array(256); return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = a[i & 255]; return last; }; });
bench("typed/set-u8@base/loop", function () { var a = new Uint8Array(256); return function () { var last = 0; for (var i = 0; i < 1000000; i++) a[i & 255] = i; return a[5]; }; });
bench("typed/get-f64@base/float-loop", function () { var a = new Float64Array(256); a.fill(1.5); return function () { var x = 0.5; for (var i = 0; i < 1000000; i++) x = a[i & 255]; return x; }; });
bench("typed/set-f64@base/loop", function () { var a = new Float64Array(256); return function () { var last = 0; for (var i = 0; i < 1000000; i++) a[i & 255] = 1.5; return a[5]; }; });
bench("new/function@base/loop", function () { function P(x) { this.x = x; } return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = new P(i); return last.x; }; });
bench("new/class@base/loop", function () { class P { constructor(x) { this.x = x; } } return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = new P(i); return last.x; }; });
bench("alloc/object-literal@base/loop", function () { return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = { a: i, b: 2 }; return last.a; }; });
bench("callback/forEach", function () { var a = []; for (var k = 0; k < 1000; k++) a.push(k); return function () { var s = 0; for (var r = 0; r < 1000; r++) a.forEach(function (v) { s = v; }); return s; }; });
bench("generator/next", function () { function* g() { for (var k = 0; k < 1000000; k++) yield k; } return function () { var s = 0; for (var v of g()) s = v; return s; }; });
bench("string/charCodeAt@base/loop", function () { var t = "abcdefghijklmnopqrstuvwxyz012345"; return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = t.charCodeAt(i & 31); return last; }; });
bench("native/Math.max@base/loop", function () { return function () { var last = 0; for (var i = 0; i < 1000000; i++) last = Math.max(i, 7); return last; }; });
