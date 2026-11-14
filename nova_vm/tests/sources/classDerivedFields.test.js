function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}

class Base {
  constructor(type, override) {
    this.type = type;
    if (override) return override;
  }
}

class Event1 extends Base {
  altKey;
  ctrlKey = false;
  constructor(type, init = {}) {
    super(type, undefined);
    this.altKey = !!init.altKey;
  }
}
const e = new Event1("click", { altKey: true });
check("public fields after super", JSON.stringify(e), '{"type":"click","altKey":true,"ctrlKey":false}');

class Pointer extends Event1 {
  constructor(type, init = {}) {
    super(type, init), (this.pointerId = init.pointerId ?? 1);
  }
}
check("two derived levels", JSON.stringify(new Pointer("down", { pointerId: 3 })),
  '{"type":"down","altKey":false,"ctrlKey":false,"pointerId":3}');

const order = [];
class Ordered extends Base {
  a = order.push("field");
  constructor() {
    order.push("before");
    super("x");
    order.push("after");
  }
}
new Ordered();
check("fields run when super returns", order.join(), "before,field,after");

class Private extends Base {
  #x = 1;
  #m() { return this.#x + 1; }
  constructor(override) { super("p", override); }
  static read(o) { return o.#m(); }
}
const p = new Private();
check("private field and method after super", Private.read(p), 2);
let threw = "";
try { new Private(p); } catch (err) { threw = err.constructor.name; }
check("private field added twice", threw, "TypeError");

class Arrow extends Base {
  v = this instanceof Arrow;
  constructor() { const call = () => super("a"); call(); }
}
check("super call in an arrow", new Arrow().v, true);

class Twice extends Base {
  n = 1;
  constructor() {
    super("t");
    try { super("t"); } catch (err) { this.err = err.constructor.name; }
  }
}
const twice = new Twice();
check("second super call throws", twice.err, "ReferenceError");
check("fields once", twice.n, 1);

class Home extends Base {
  s = super.constructor === Base;
  constructor() { super("h"); }
}
check("super property in a field", new Home().s, true);

class OnProxy extends Base {
  f = 5;
  constructor(override) { super("q", override); }
}
const target = {};
check("public field on a returned proxy", new OnProxy(new Proxy(target, {})).f, 5);
check("proxy forwards the field", target.f, 5);
