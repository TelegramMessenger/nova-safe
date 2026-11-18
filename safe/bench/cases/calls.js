// Calling functions.
bench("call/plain-function", function () {
  function add(a, b) { return a + b; }
  return function () {
    var sum = 0;
    for (var i = 0; i < 300000; i++) sum = add(sum, i & 15);
    return sum;
  };
});

bench("call/method", function () {
  var counter = { n: 0, bump: function (by) { this.n += by; return this.n; } };
  return function () {
    counter.n = 0;
    for (var i = 0; i < 300000; i++) counter.bump(i & 3);
    return counter.n;
  };
});

bench("call/closure", function () {
  function make(step) { var total = 0; return function (x) { total += x * step; return total; }; }
  return function () {
    var f = make(2), last = 0;
    for (var i = 0; i < 300000; i++) last = f(i & 7);
    return last;
  };
});

bench("call/recursive-fib", function () {
  function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
  return function () { return fib(24); };
});

bench("call/arrow-callback", function () {
  var data = [];
  for (var i = 0; i < 1000; i++) data.push(i);
  return function () {
    var sum = 0;
    for (var r = 0; r < 100; r++) data.forEach((v) => { sum += v & 7; });
    return sum;
  };
});

bench("call/spread-and-apply", function () {
  function sum3(a, b, c) { return a + b + c; }
  var args = [1, 2, 3];
  return function () {
    var total = 0;
    for (var i = 0; i < 100000; i++) {
      total += sum3(...args);
      total += sum3.apply(null, args);
    }
    return total;
  };
});

bench("call/rest-and-default", function () {
  function f(a, b = 2, ...rest) { return a + b + rest.length; }
  return function () {
    var total = 0;
    for (var i = 0; i < 150000; i++) total += f(i & 3) + f(1, 2, 3, 4);
    return total;
  };
});

bench("call/arguments-object", function () {
  function count() { var n = 0; for (var i = 0; i < arguments.length; i++) n += arguments[i]; return n; }
  return function () {
    var total = 0;
    for (var i = 0; i < 150000; i++) total += count(1, 2, 3);
    return total;
  };
});

bench("call/bound-function", function () {
  var base = { k: 3 };
  var f = function (x) { return this.k + x; }.bind(base);
  return function () {
    var total = 0;
    for (var i = 0; i < 300000; i++) total += f(i & 3);
    return total;
  };
});

bench("call/new-class-instance", function () {
  class Point {
    constructor(x, y) { this.x = x; this.y = y; }
    length2() { return this.x * this.x + this.y * this.y; }
  }
  return function () {
    var total = 0;
    for (var i = 0; i < 150000; i++) total += new Point(i & 7, 2).length2();
    return total;
  };
});

bench("call/class-inheritance-super", function () {
  class A { constructor(v) { this.v = v; } get() { return this.v; } }
  class B extends A { constructor(v) { super(v + 1); } get() { return super.get() * 2; } }
  return function () {
    var total = 0;
    for (var i = 0; i < 100000; i++) total += new B(i & 7).get();
    return total;
  };
});

bench("call/native-math", function () {
  return function () {
    var total = 0;
    for (var i = 0; i < 300000; i++) total += Math.floor(Math.sqrt(i)) + Math.max(i & 7, 3);
    return total;
  };
});

bench("call/try-catch-throw", function () {
  function risky(i) { if ((i & 15) === 0) throw new Error("x"); return i; }
  return function () {
    var caught = 0;
    for (var i = 0; i < 50000; i++) {
      try { risky(i); } catch (e) { caught++; }
    }
    return caught;
  };
});

bench("call/generator", function () {
  function* range(n) { for (var i = 0; i < n; i++) yield i; }
  return function () {
    var sum = 0;
    for (var v of range(100000)) sum += v & 7;
    return sum;
  };
});
