// Arrays.
bench("array/push-then-pop", function () {
  return function () {
    var a = [];
    for (var i = 0; i < 200000; i++) a.push(i);
    var sum = 0;
    while (a.length) sum += a.pop() & 3;
    return sum;
  };
});

bench("array/index-read-write", function () {
  var a = new Array(10000).fill(1);
  return function () {
    for (var r = 0; r < 30; r++) {
      for (var i = 1; i < a.length; i++) a[i] = (a[i - 1] + a[i]) & 1023;
    }
    return a[a.length - 1];
  };
});

bench("array/for-of", function () {
  var a = [];
  for (var i = 0; i < 10000; i++) a.push(i);
  return function () {
    var sum = 0;
    for (var r = 0; r < 20; r++) for (var v of a) sum += v & 7;
    return sum;
  };
});

bench("array/map-filter-reduce", function () {
  var a = [];
  for (var i = 0; i < 5000; i++) a.push(i);
  return function () {
    var total = 0;
    for (var r = 0; r < 20; r++) {
      total += a.map(function (v) { return v * 2; }).filter(function (v) { return v % 3 === 0; }).reduce(function (s, v) { return s + v; }, 0);
    }
    return total;
  };
});

bench("array/sort-with-comparator", function () {
  return function () {
    var a = [], seed = 12345;
    for (var i = 0; i < 20000; i++) { seed = (seed * 1103515245 + 12345) & 0x7fffffff; a.push(seed % 100000); }
    a.sort(function (x, y) { return x - y; });
    return a[0] + "," + a[10000] + "," + a[19999];
  };
});

bench("array/sort-objects-by-key", function () {
  return function () {
    var a = [], seed = 99;
    for (var i = 0; i < 5000; i++) { seed = (seed * 1103515245 + 12345) & 0x7fffffff; a.push({ id: i, score: seed % 1000, name: "n" + (seed % 97) }); }
    a.sort(function (x, y) { return x.score - y.score || (x.name < y.name ? -1 : x.name > y.name ? 1 : 0); });
    return a[0].id + "," + a[4999].id;
  };
});

bench("array/join-numbers", function () {
  var a = [];
  for (var i = 0; i < 20000; i++) a.push(i);
  return function () {
    var n = 0;
    for (var r = 0; r < 10; r++) n += a.join(",").length;
    return n;
  };
});

bench("array/slice-concat-spread", function () {
  var a = [];
  for (var i = 0; i < 1000; i++) a.push(i);
  return function () {
    var n = 0;
    for (var r = 0; r < 500; r++) {
      var b = a.slice(100, 900).concat(a.slice(0, 50));
      var c = [...b, r];
      n += c.length;
    }
    return n;
  };
});

bench("array/index-of-includes", function () {
  var a = [];
  for (var i = 0; i < 2000; i++) a.push("item" + i);
  return function () {
    var hits = 0;
    for (var r = 0; r < 300; r++) {
      if (a.indexOf("item" + (r * 6)) >= 0) hits++;
      if (a.includes("missing")) hits++;
    }
    return hits;
  };
});

bench("array/literal-alloc", function () {
  return function () {
    var last;
    for (var i = 0; i < 200000; i++) last = [i, i + 1, i + 2];
    return last[2];
  };
});

bench("array/two-dimensional-grid", function () {
  var grid = [];
  for (var y = 0; y < 100; y++) { var row = []; for (var x = 0; x < 100; x++) row.push((x * y) & 1); grid.push(row); }
  return function () {
    var alive = 0;
    for (var r = 0; r < 5; r++) {
      for (var y = 1; y < 99; y++) {
        for (var x = 1; x < 99; x++) {
          var n = grid[y - 1][x - 1] + grid[y - 1][x] + grid[y - 1][x + 1] + grid[y][x - 1] + grid[y][x + 1] + grid[y + 1][x - 1] + grid[y + 1][x] + grid[y + 1][x + 1];
          if (n === 3 || (n === 2 && grid[y][x])) alive++;
        }
      }
    }
    return alive;
  };
});

bench("array/shift-unshift-splice", function () {
  return function () {
    var a = [];
    for (var i = 0; i < 2000; i++) a.push(i);
    var sum = 0;
    for (var j = 0; j < 2000; j++) {
      sum += a.shift();
      a.push(j);
      if ((j & 31) === 0) { a.splice(10, 2, j, j + 1); a.unshift(0); a.pop(); }
    }
    return sum;
  };
});

bench("array/find-some-every", function () {
  var a = [];
  for (var i = 0; i < 2000; i++) a.push({ id: i, on: (i & 1) === 0 });
  return function () {
    var hits = 0;
    for (var r = 0; r < 200; r++) {
      if (a.find(function (o) { return o.id === 1500; })) hits++;
      if (a.some(function (o) { return o.id === 1999; })) hits++;
      if (!a.every(function (o) { return o.on; })) hits++;
    }
    return hits;
  };
});

bench("array/from-and-fill", function () {
  return function () {
    var n = 0;
    for (var r = 0; r < 200; r++) {
      n += Array.from({ length: 500 }, function (_, i) { return i * 2; }).length;
      n += new Array(500).fill(0).length;
    }
    return n;
  };
});
