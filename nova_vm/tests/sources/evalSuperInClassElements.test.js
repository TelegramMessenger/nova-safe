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

class B {
  get x() { return "Bx"; }
  m() { return "Bm"; }
  static get sx() { return "BSx"; }
  static sm() { return "BSm"; }
}

class A extends B {
  field = eval("super.x");
  arrow = () => eval("super.m()");
  #private = eval("super.x");
  newTarget = eval("new.target");
  static staticField = eval("() => super.sm()");
  static staticBlock;
  static {
    A.staticBlock = eval("super.sx");
  }
  method() { return eval("super.m()"); }
  nested() { return eval("(() => super.m())()"); }
  strict() { "use strict"; return eval("super.m()"); }
  privateField() { return this.#private; }
}
var a = new A();
check("field", a.field, "Bx");
check("arrow in field", a.arrow(), "Bm");
check("private field", a.privateField(), "Bx");
check("new.target in field", a.newTarget, undefined);
check("static field", A.staticField(), "BSm");
check("static block", A.staticBlock, "BSx");
check("method", a.method(), "Bm");
check("arrow in method", a.nested(), "Bm");
check("strict method", a.strict(), "Bm");
check("object literal method", eval("({ __proto__: { z: 1 }, m() { return super.z; } }).m()"), 1);

check("super() in a field", errorName(() => { class C extends B { f = eval("super()"); } new C(); }), "SyntaxError");
check("arguments in a field", errorName(() => { class C { f = eval("arguments"); } new C(); }), "SyntaxError");
check("arguments in a static block", errorName(() => { class C { static { eval("arguments"); } } }), "SyntaxError");
check("super() in a method", errorName(() => { class C extends B { g() { return eval("super()"); } } new C().g(); }), "SyntaxError");
check("super outside methods", errorName(() => eval("super.x")), "SyntaxError");
check("new.target outside functions", errorName(() => eval("new.target")), "SyntaxError");
check("super in a function in eval", errorName(() => { class C extends B { g() { return eval("(function () { return super.m(); })()"); } } new C().g(); }), "SyntaxError");
check("indirect eval", errorName(() => { class C extends B { g() { return (0, eval)("super.m()"); } } new C().g(); }), "SyntaxError");

class D extends B {
  constructor() {
    eval("super()");
    this.v = eval("super.m()");
  }
}
check("super() in a derived constructor", new D().v, "Bm");
