function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}

class A {}
class B extends A { constructor(a, b) { super(); } }
check("derived constructor own keys", Object.getOwnPropertyNames(B).join(), "length,name,prototype");
check("derived constructor name", B.name, "B");
check("derived constructor length", B.length, 2);
check("instance constructor name", new B().constructor.name, "B");
const Anonymous = class extends A { constructor() { super(); } };
check("named by assignment", Anonymous.name, "Anonymous");
check("name descriptor", JSON.stringify(Object.getOwnPropertyDescriptor(B, "name")),
  '{"value":"B","writable":false,"enumerable":false,"configurable":true}');

const params = Array.from({ length: 300 }, (_, i) => "a" + i).join(",");
const many = new Function(params, "return a299;");
check("300 parameters", many.length, 300);
check("300 parameters name", many.name, "anonymous");
check("300 parameters call", many(...Array.from({ length: 300 }, (_, i) => i)), 299);
const declared = eval("(function f(" + params + ") { return a0; })");
check("declared with 300 parameters", declared.length, 300);
