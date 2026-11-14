function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}
class A {}
class B extends A { constructor(a, b, c) { super(); } }
const bound = B.bind(null, 1);
check("bound derived class name", bound.name, "bound B");
check("bound derived class length", bound.length, 2);
check("bound derived class keys", Object.getOwnPropertyNames(bound).join(), "length,name");
function f(a, b) {}
check("bound function", f.bind(null).name + f.bind(null).length, "bound f2");
const long = function () {};
Object.defineProperty(long, "length", { value: 1000 });
check("bound length over 255", long.bind(null, 1, 2).length, 998);
const infinite = function () {};
Object.defineProperty(infinite, "length", { value: Infinity });
check("bound length Infinity", infinite.bind().length, Infinity);
const named = function () {};
Object.defineProperty(named, "name", { get() { return "got"; } });
check("bound name through a getter", named.bind().name, "bound got");
