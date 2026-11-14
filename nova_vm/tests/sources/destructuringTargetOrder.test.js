function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}

const log = [];
const target = {
  get t() { log.push("target"); return { set v(x) { log.push("put " + x); } }; },
};
const source = { get a() { log.push("get"); return 1; } };
({ a: target.t.v } = source);
check("target before property", log.join(), "target,get,put 1");

log.length = 0;
const empty = { get a() { log.push("get"); return undefined; } };
({ a: target.t.v = (log.push("init"), 2) } = empty);
check("target, property, initializer", log.join(), "target,get,init,put 2");

class C extends class {} {
  #field;
  constructor() {
    const init = () => super();
    const object = { get a() { init(); } };
    ({ a: this.#field } = object);
  }
}
let threw = "";
try { new C(); } catch (err) { threw = err.constructor.name; }
check("this before the getter", threw, "ReferenceError");

let x, y;
({ a: x, b: y = 5 } = { a: 1 });
check("plain targets", x + "," + y, "1,5");
const out = {};
({ a: out.p, ["b"]: out.q, ...out.rest } = { a: 1, b: 2, c: 3 });
check("with rest", JSON.stringify(out), '{"p":1,"q":2,"rest":{"c":3}}');
