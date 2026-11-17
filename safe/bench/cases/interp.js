// The interpreter itself: loops, locals, arithmetic, branches.
bench("loop/empty-for", function () {
  return function () {
    var i;
    for (i = 0; i < 1000000; i++) {}
    return i;
  };
});

bench("loop/int-sum", function () {
  return function () {
    var sum = 0;
    for (var i = 0; i < 1000000; i++) sum += i & 255;
    return sum;
  };
});

bench("loop/float-math", function () {
  return function () {
    var x = 0.5;
    for (var i = 0; i < 500000; i++) x = x * 1.0000001 + 0.25 / (i + 1);
    return x.toFixed(6);
  };
});

bench("loop/let-per-iteration", function () {
  return function () {
    let sum = 0;
    for (let i = 0; i < 500000; i++) {
      const twice = i * 2;
      sum += twice & 7;
    }
    return sum;
  };
});

bench("loop/while-nested", function () {
  return function () {
    var count = 0, i = 0;
    while (i < 1000) {
      var j = 0;
      while (j < 500) {
        if (j > i) count++;
        j++;
      }
      i++;
    }
    return count;
  };
});

bench("loop/bit-ops", function () {
  return function () {
    var h = 2166136261;
    for (var i = 0; i < 500000; i++) {
      h ^= i & 0xff;
      h = (h * 16777619) >>> 0;
      h = (h << 5) | (h >>> 27);
    }
    return h >>> 0;
  };
});

bench("loop/switch", function () {
  return function () {
    var a = 0;
    for (var i = 0; i < 500000; i++) {
      switch (i & 7) {
        case 0: a += 1; break;
        case 1: a += 2; break;
        case 2: a -= 1; break;
        case 3: a ^= 5; break;
        case 4: a += 3; break;
        case 5: a -= 2; break;
        case 6: a |= 1; break;
        default: a += 7;
      }
    }
    return a;
  };
});

bench("loop/closure-variable", function () {
  var counter = 0;
  function bump() { counter++; }
  return function () {
    counter = 0;
    for (var i = 0; i < 300000; i++) counter += i & 3;
    bump();
    return counter;
  };
});

var __globalCounter = 0;
bench("loop/global-variable", function () {
  return function () {
    __globalCounter = 0;
    for (var i = 0; i < 300000; i++) __globalCounter += i & 3;
    return __globalCounter;
  };
});

bench("loop/compare-strict-equal", function () {
  return function () {
    var hits = 0;
    for (var i = 0; i < 500000; i++) {
      if (i % 3 === 0) hits++;
      else if (i % 5 !== 0) hits += 2;
    }
    return hits;
  };
});

bench("loop/typeof-and-truthy", function () {
  var values = [1, "a", null, undefined, {}, [], true, 0, "", 2.5];
  return function () {
    var hits = 0;
    for (var i = 0; i < 300000; i++) {
      var v = values[i % 10];
      if (typeof v === "number") hits++;
      else if (typeof v === "string") hits += 2;
      else if (!v) hits += 3;
    }
    return hits;
  };
});
