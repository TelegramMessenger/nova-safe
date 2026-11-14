function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}
const o = { *m(a) {}, async *am() {}, async af() {}, f() {} };
check("generator method prototype", Object.getOwnPropertyNames(o.m).join(), "length,name,prototype");
check("generator method prototype's prototype",
  Object.getPrototypeOf(o.m.prototype), Object.getPrototypeOf(function* () {}).prototype);
check("async generator method prototype", Object.getOwnPropertyNames(o.am).join(), "length,name,prototype");
check("async method has none", Object.getOwnPropertyNames(o.af).join(), "length,name");
check("method has none", Object.getOwnPropertyNames(o.f).join(), "length,name");
class P { static *s() {} *i() {} static *#q() {} static q() { return P.#q; } }
check("static generator method", Object.getOwnPropertyNames(P.s).join(), "length,name,prototype");
check("generator method", Object.getOwnPropertyNames(P.prototype.i).join(), "length,name,prototype");
check("private generator method", Object.getOwnPropertyNames(P.q()).join(), "length,name,prototype");
check("prototype descriptor", JSON.stringify(Object.getOwnPropertyDescriptor(o.m, "prototype")),
  '{"value":{},"writable":true,"enumerable":false,"configurable":false}');
const AsyncGeneratorFunction = Object.getPrototypeOf(async function* () {}).constructor;
const dynamic = AsyncGeneratorFunction("yield 1");
check("dynamic async generator prototype", Object.getOwnPropertyNames(dynamic).join(), "length,name,prototype");
check("dynamic async generator prototype's prototype",
  Object.getPrototypeOf(dynamic.prototype), AsyncGeneratorFunction.prototype.prototype);
