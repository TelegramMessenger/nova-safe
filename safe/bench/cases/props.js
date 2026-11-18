// Objects and their properties.
bench("prop/get-set-one-shape", function () {
  var o = { x: 1, y: 2, z: 3 };
  return function () {
    o.x = 0;
    for (var i = 0; i < 500000; i++) o.x = o.x + o.y + (o.z & 1);
    return o.x;
  };
});

bench("prop/four-shapes", function () {
  var objects = [{ a: 1, v: 1 }, { b: 1, v: 2 }, { c: 1, d: 2, v: 3 }, { v: 4, e: 0 }];
  return function () {
    var sum = 0;
    for (var i = 0; i < 400000; i++) sum += objects[i & 3].v;
    return sum;
  };
});

bench("prop/dynamic-string-key", function () {
  var o = { alpha: 1, beta: 2, gamma: 3, delta: 4, epsilon: 5, zeta: 6, eta: 7, theta: 8 };
  var keys = Object.keys(o);
  return function () {
    var sum = 0;
    for (var i = 0; i < 300000; i++) sum += o[keys[i & 7]];
    return sum;
  };
});

bench("prop/prototype-chain-method", function () {
  function A() {}
  A.prototype.value = function () { return 3; };
  function B() {}
  B.prototype = Object.create(A.prototype);
  function C() {}
  C.prototype = Object.create(B.prototype);
  var c = new C();
  return function () {
    var sum = 0;
    for (var i = 0; i < 300000; i++) sum += c.value();
    return sum;
  };
});

bench("prop/getter-setter", function () {
  var o = { _v: 0, get v() { return this._v; }, set v(x) { this._v = x & 1023; } };
  return function () {
    o._v = 0;
    for (var i = 0; i < 200000; i++) o.v = o.v + 3;
    return o.v;
  };
});

bench("prop/object-literal-alloc", function () {
  return function () {
    var last = null;
    for (var i = 0; i < 200000; i++) last = { x: i, y: i + 1, tag: "p" };
    return last.x + last.y;
  };
});

bench("prop/add-properties-one-by-one", function () {
  return function () {
    var total = 0;
    for (var i = 0; i < 40000; i++) {
      var o = {};
      o.a = i; o.b = 2; o.c = 3; o.d = 4; o.e = 5; o.f = 6;
      total += o.a + o.f;
    }
    return total;
  };
});

bench("prop/dictionary-churn", function () {
  return function () {
    var dict = {};
    for (var i = 0; i < 20000; i++) dict["k" + (i % 500)] = i;
    var n = 0;
    for (var j = 0; j < 20000; j++) {
      var key = "k" + (j % 500);
      if (key in dict) n += dict[key] & 1;
      if ((j & 7) === 0) delete dict[key];
    }
    return n;
  };
});

bench("prop/for-in-and-keys", function () {
  var o = {};
  for (var i = 0; i < 50; i++) o["field" + i] = i;
  return function () {
    var sum = 0;
    for (var r = 0; r < 1000; r++) {
      for (var k in o) sum += o[k];
      sum += Object.keys(o).length;
    }
    return sum;
  };
});

bench("prop/destructuring", function () {
  var point = { x: 3, y: 4, meta: { id: 7 } };
  var pair = [1, 2, 3];
  return function () {
    var sum = 0;
    for (var i = 0; i < 150000; i++) {
      var { x, y, meta: { id } } = point;
      var [a, b] = pair;
      sum += x + y + id + a + b;
    }
    return sum;
  };
});

bench("prop/object-spread-assign", function () {
  var base = { a: 1, b: 2, c: 3, d: 4 };
  return function () {
    var last;
    for (var i = 0; i < 50000; i++) {
      last = { ...base, e: i };
      last = Object.assign({}, last, { f: 1 });
    }
    return last.e + last.f;
  };
});

bench("prop/has-own-and-in", function () {
  var o = { a: 1, b: 2, c: 3 };
  var names = ["a", "x", "b", "y", "c", "toString"];
  return function () {
    var hits = 0;
    for (var i = 0; i < 200000; i++) {
      var name = names[i % 6];
      if (name in o) hits++;
      if (Object.prototype.hasOwnProperty.call(o, name)) hits++;
    }
    return hits;
  };
});

bench("prop/instanceof-and-array-check", function () {
  class A {}
  class B extends A {}
  var values = [new A(), new B(), [], {}, null];
  return function () {
    var hits = 0;
    for (var i = 0; i < 200000; i++) {
      var v = values[i % 5];
      if (v instanceof A) hits++;
      if (Array.isArray(v)) hits += 2;
    }
    return hits;
  };
});
