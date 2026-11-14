// [[Set]] of "lastIndex" with a receiver other than the RegExp is the
// ordinary one: it writes the receiver and leaves the RegExp alone.
function check(name, actual, expected) {
  if (actual !== expected) throw new Error(name + ": expected " + String(expected) + ", got " + String(actual));
}
var r = /a/g;
var o = {};
check("plain receiver", Reflect.set(r, "lastIndex", 5, o), true);
check("regexp untouched", r.lastIndex, 0);
check("receiver written", o.lastIndex, 5);
var r2 = /b/g;
r2.foo = 1;
var o2 = {};
check("with backing object", Reflect.set(r2, "lastIndex", 6, o2), true);
check("regexp untouched 2", r2.lastIndex, 0);
check("receiver written 2", o2.lastIndex, 6);
var r3 = /c/g;
r3.foo = 1;
var log = [];
var p = new Proxy({}, {
  getOwnPropertyDescriptor(t, k) { log.push("gopd " + String(k)); return Reflect.getOwnPropertyDescriptor(t, k); },
  defineProperty(t, k, d) { log.push("define " + String(k) + " " + d.value); return Reflect.defineProperty(t, k, d); },
});
check("proxy receiver", Reflect.set(r3, "lastIndex", 7, p), true);
check("proxy traps", log.join("; "), "gopd lastIndex; define lastIndex 7");
check("proxy written", p.lastIndex, 7);
check("regexp untouched 3", r3.lastIndex, 0);
var r4 = /d/g;
check("self receiver", Reflect.set(r4, "lastIndex", 4, r4), true);
check("self written", r4.lastIndex, 4);
var child = Object.create(/e/g);
child.lastIndex = 9;
check("inherited", Object.getOwnPropertyNames(child).join(), "lastIndex");
check("inherited value", child.lastIndex, 9);
check("primitive receiver", Reflect.set(/g/, "lastIndex", 1, 5), false);
var frozen = Object.freeze(/h/g);
check("frozen with receiver", Reflect.set(frozen, "lastIndex", 2, {}), false);
check("frozen", Reflect.set(frozen, "lastIndex", 2), false);
var r5 = /i/g;
r5.foo = 1;
var forwarding = new Proxy(r5, { set(t, k, v, recv) { return Reflect.set(t, k, v, recv); } });
forwarding.lastIndex = 3;
check("forwarding proxy", r5.lastIndex, 3);
var r6 = /j/g;
Object.defineProperty(r6, "lastIndex", { value: 1 });
check("define", r6.lastIndex, 1);
var r7 = /k/g;
r7.foo = 1;
check("reflect define", Reflect.defineProperty(r7, "lastIndex", { value: 2 }), true);
check("reflect define value", r7.lastIndex, 2);
var r8 = /l/g;
Object.defineProperty(r8, "lastIndex", { value: 1.5 });
check("define non-index", r8.lastIndex, 1.5);
var r9 = /m/g;
var target9 = new Proxy(r9, {});
var proxy9 = new Proxy(target9, {});
proxy9.lastIndex = 1;
check("proxy of proxy", r9.lastIndex, 1);
