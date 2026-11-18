// Numbers, typed arrays, collections, JSON, RegExp.
bench("math/trig-and-sqrt", function () {
  return function () {
    var acc = 0;
    for (var i = 0; i < 200000; i++) acc += Math.sin(i * 0.01) * Math.cos(i * 0.02) + Math.sqrt(i) + Math.abs(i - 1000) * 0.001;
    return acc.toFixed(3);
  };
});

bench("math/floor-round-minmax", function () {
  return function () {
    var acc = 0;
    for (var i = 0; i < 300000; i++) acc += Math.floor(i / 7) + Math.round(i * 0.3) + Math.min(i, 500) - Math.max(i & 7, 2);
    return acc;
  };
});

bench("math/integer-to-double-mix", function () {
  return function () {
    var a = 1, b = 0.5;
    for (var i = 0; i < 300000; i++) { a = (a * 3 + i) % 1000003; b = b + a / 3; }
    return b.toFixed(1);
  };
});

bench("math/random-and-date", function () {
  return function () {
    var acc = 0;
    for (var i = 0; i < 100000; i++) acc += Math.random() < 2 ? 1 : 0;
    for (var j = 0; j < 20000; j++) acc += Date.now() > 0 ? 1 : 0;
    return acc;
  };
});

bench("typed/uint8-fill-loop", function () {
  var bytes = new Uint8Array(65536);
  return function () {
    var sum = 0;
    for (var r = 0; r < 8; r++) {
      for (var i = 0; i < bytes.length; i++) bytes[i] = (i * 7 + r) & 255;
      for (var j = 0; j < bytes.length; j++) sum += bytes[j];
    }
    return sum;
  };
});

bench("typed/float64-math", function () {
  var xs = new Float64Array(20000), ys = new Float64Array(20000);
  for (var i = 0; i < xs.length; i++) xs[i] = i * 0.5;
  return function () {
    var sum = 0;
    for (var r = 0; r < 10; r++) for (var i = 0; i < xs.length; i++) { ys[i] = xs[i] * 1.01 + ys[i] * 0.5; sum += ys[i]; }
    return sum.toFixed(2);
  };
});

bench("typed/pixels-rgba", function () {
  var width = 160, height = 120;
  var pixels = new Uint8ClampedArray(width * height * 4);
  return function () {
    for (var y = 0; y < height; y++) {
      for (var x = 0; x < width; x++) {
        var at = (y * width + x) * 4;
        pixels[at] = x * 2;
        pixels[at + 1] = y * 2;
        pixels[at + 2] = (x + y) * 1.5;
        pixels[at + 3] = 255;
      }
    }
    return pixels[4000] + pixels[40001];
  };
});

bench("typed/set-subarray-copy", function () {
  var source = new Uint8Array(4096), target = new Uint8Array(4096 * 8);
  for (var i = 0; i < source.length; i++) source[i] = i & 255;
  return function () {
    var n = 0;
    for (var r = 0; r < 300; r++) {
      for (var k = 0; k < 8; k++) target.set(source, k * 4096);
      n += target.subarray(100, 200).length + target.slice(0, 64).length;
    }
    return n;
  };
});

bench("map/number-keys", function () {
  return function () {
    var m = new Map();
    for (var i = 0; i < 30000; i++) m.set(i, i * 2);
    var sum = 0;
    for (var j = 0; j < 30000; j++) sum += m.get(j) & 7;
    for (var k = 0; k < 30000; k += 2) m.delete(k);
    return sum + m.size;
  };
});

bench("map/string-keys", function () {
  var keys = [];
  for (var i = 0; i < 5000; i++) keys.push("key:" + i);
  return function () {
    var m = new Map();
    for (var i = 0; i < keys.length; i++) m.set(keys[i], i);
    var sum = 0;
    for (var r = 0; r < 4; r++) for (var j = 0; j < keys.length; j++) sum += m.get(keys[j]) & 7;
    return sum + m.size;
  };
});

bench("map/iterate", function () {
  var m = new Map();
  for (var i = 0; i < 10000; i++) m.set(i, i);
  return function () {
    var sum = 0;
    for (var r = 0; r < 10; r++) {
      for (var [k, v] of m) sum += (k + v) & 3;
      m.forEach(function (v) { sum += v & 1; });
    }
    return sum;
  };
});

bench("set/add-has", function () {
  return function () {
    var s = new Set();
    for (var i = 0; i < 30000; i++) s.add(i % 20000);
    var hits = 0;
    for (var j = 0; j < 40000; j++) if (s.has(j)) hits++;
    return hits + s.size;
  };
});

bench("object-as-dictionary", function () {
  var keys = [];
  for (var i = 0; i < 3000; i++) keys.push("id_" + i);
  return function () {
    var d = {};
    for (var i = 0; i < keys.length; i++) d[keys[i]] = i;
    var sum = 0;
    for (var r = 0; r < 5; r++) for (var j = 0; j < keys.length; j++) sum += d[keys[j]] & 7;
    return sum;
  };
});

bench("json/stringify", function () {
  var data = [];
  for (var i = 0; i < 300; i++) data.push({ id: i, name: "Item " + i, tags: ["a", "b", "c"], price: i * 1.5, nested: { on: true, note: "x\"y" } });
  return function () {
    var n = 0;
    for (var r = 0; r < 20; r++) n += JSON.stringify(data).length;
    return n;
  };
});

bench("json/parse", function () {
  var data = [];
  for (var i = 0; i < 300; i++) data.push({ id: i, name: "Item " + i, tags: ["a", "b", "c"], price: i * 1.5, nested: { on: true, note: "x\"y" } });
  var text = JSON.stringify(data);
  return function () {
    var n = 0;
    for (var r = 0; r < 20; r++) n += JSON.parse(text).length;
    return n;
  };
});

bench("regexp/test-simple", function () {
  var re = /^[a-z]+-\d+$/;
  var inputs = [];
  for (var i = 0; i < 500; i++) inputs.push(i & 1 ? "item-" + i : "Item " + i);
  return function () {
    var hits = 0;
    for (var r = 0; r < 100; r++) for (var i = 0; i < inputs.length; i++) if (re.test(inputs[i])) hits++;
    return hits;
  };
});

bench("regexp/exec-groups-global", function () {
  var text = "";
  for (var i = 0; i < 300; i++) text += "key" + i + "=value" + i + "; ";
  return function () {
    var n = 0;
    for (var r = 0; r < 10; r++) {
      var re = /(\w+)=(\w+);/g, m;
      while ((m = re.exec(text))) n += m[1].length + m[2].length;
    }
    return n;
  };
});

bench("regexp/replace-callback", function () {
  var text = "";
  for (var i = 0; i < 300; i++) text += "The {name" + i + "} costs {price" + i + "} today. ";
  return function () {
    var n = 0;
    for (var r = 0; r < 10; r++) n += text.replace(/\{(\w+)\}/g, function (_, k) { return k.toUpperCase(); }).length;
    return n;
  };
});

bench("regexp/split-and-match", function () {
  var text = "";
  for (var i = 0; i < 400; i++) text += "word" + i + ",  other" + i + " ; ";
  return function () {
    var n = 0;
    for (var r = 0; r < 10; r++) n += text.split(/\s*[,;]\s*/).length + text.match(/\d+/g).length;
    return n;
  };
});
