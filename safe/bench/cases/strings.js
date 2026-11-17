// Strings: building, scanning, searching.
bench("string/append-20k-short", function () {
  return function () {
    var s = "";
    for (var i = 0; i < 20000; i++) s += "chunk";
    return s.length;
  };
});

bench("string/append-20k-numbers", function () {
  return function () {
    var s = "";
    for (var i = 0; i < 20000; i++) s += i + ",";
    return s.length;
  };
});

bench("string/append-100k-char", function () {
  return function () {
    var s = "";
    for (var i = 0; i < 100000; i++) s += "x";
    return s.length;
  };
});

bench("string/append-non-ascii", function () {
  return function () {
    var s = "";
    for (var i = 0; i < 20000; i++) s += "ж中";
    return s.length;
  };
});

bench("string/append-html-rows", function () {
  var rows = [];
  for (var i = 0; i < 400; i++) rows.push({ id: i, name: "Item " + i, price: (i * 1.25).toFixed(2) });
  return function () {
    var html = "<table>";
    for (var i = 0; i < rows.length; i++) {
      var r = rows[i];
      html += "<tr class=\"" + (i & 1 ? "odd" : "even") + "\"><td>" + r.id + "</td><td>" + r.name + "</td><td>" + r.price + "</td></tr>";
    }
    html += "</table>";
    return html.length;
  };
});

bench("string/prepend-5k", function () {
  return function () {
    var s = "";
    for (var i = 0; i < 5000; i++) s = "ab" + s;
    return s.length;
  };
});

bench("string/template-literal", function () {
  return function () {
    var n = 0;
    for (var i = 0; i < 100000; i++) n += `item-${i}-${i & 7}:${"x"}`.length;
    return n;
  };
});

bench("string/array-join-build", function () {
  return function () {
    var parts = [];
    for (var i = 0; i < 20000; i++) parts.push("chunk");
    return parts.join("").length;
  };
});

bench("string/char-code-at-ascii", function () {
  var s = "";
  for (var i = 0; i < 2000; i++) s += "The quick brown fox jumps over the lazy dog. ";
  return function () {
    var h = 0;
    for (var i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
    return h;
  };
});

bench("string/char-code-at-non-ascii", function () {
  var s = "";
  for (var i = 0; i < 2000; i++) s += "Съешь ещё этих мягких булок. ";
  return function () {
    var h = 0;
    for (var i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0;
    return h;
  };
});

bench("string/bracket-index-ascii", function () {
  var s = "";
  for (var i = 0; i < 2000; i++) s += "abcdefghijklmnopqrstuvwxyz0123456789";
  return function () {
    var vowels = 0;
    for (var i = 0; i < s.length; i++) { var c = s[i]; if (c === "a" || c === "e" || c === "i" || c === "o" || c === "u") vowels++; }
    return vowels;
  };
});

bench("string/bracket-index-non-ascii", function () {
  var s = "";
  for (var i = 0; i < 1000; i++) s += "абвгдежзийклмноп";
  return function () {
    var hits = 0;
    for (var i = 0; i < s.length; i++) if (s[i] === "а" || s[i] === "о") hits++;
    return hits;
  };
});

bench("string/index-of", function () {
  var s = "";
  for (var i = 0; i < 500; i++) s += "lorem ipsum dolor sit amet, consectetur adipiscing elit ";
  s += "needle";
  return function () {
    var n = 0;
    for (var r = 0; r < 300; r++) {
      n += s.indexOf("needle");
      n += s.indexOf("amet", r * 10);
      if (s.includes("missing-word")) n++;
    }
    return n;
  };
});

bench("string/slice-substring", function () {
  var s = "";
  for (var i = 0; i < 200; i++) s += "0123456789abcdefghijklmnopqrstuvwxyz";
  return function () {
    var n = 0;
    for (var i = 0; i < 100000; i++) {
      var at = i % 7000;
      n += s.slice(at, at + 8).length + s.substring(at + 2, at + 5).length;
    }
    return n;
  };
});

bench("string/slice-non-ascii", function () {
  var s = "";
  for (var i = 0; i < 200; i++) s += "привет мир 世界 ";
  return function () {
    var n = 0;
    for (var i = 0; i < 30000; i++) {
      var at = i % 2400;
      n += s.slice(at, at + 8).length;
    }
    return n;
  };
});

bench("string/split-join", function () {
  var line = "";
  for (var i = 0; i < 200; i++) line += "field" + i + ",";
  return function () {
    var n = 0;
    for (var r = 0; r < 300; r++) n += line.split(",").join(";").length;
    return n;
  };
});

bench("string/replace-string-and-regex", function () {
  var s = "";
  for (var i = 0; i < 100; i++) s += "Hello <b>world</b> & friends; a < b > c \"quoted\" ";
  return function () {
    var n = 0;
    for (var r = 0; r < 100; r++) {
      n += s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").length;
      n += s.replace("world", "there").length;
    }
    return n;
  };
});

bench("string/case-and-trim", function () {
  var words = [];
  for (var i = 0; i < 500; i++) words.push("  Mixed Case Word " + i + "  ");
  return function () {
    var n = 0;
    for (var r = 0; r < 60; r++) for (var i = 0; i < words.length; i++) n += words[i].trim().toUpperCase().toLowerCase().length;
    return n;
  };
});

bench("string/equality-and-compare", function () {
  var a = [], b = [];
  for (var i = 0; i < 1000; i++) { a.push("key-number-" + i); b.push("key-number-" + (i ^ 1)); }
  return function () {
    var hits = 0;
    for (var r = 0; r < 100; r++) for (var i = 0; i < 1000; i++) {
      if (a[i] === b[i ^ 1]) hits++;
      if (a[i] < b[i]) hits++;
    }
    return hits;
  };
});

bench("string/number-to-string", function () {
  return function () {
    var n = 0;
    for (var i = 0; i < 100000; i++) n += String(i).length + (i * 0.37).toFixed(2).length + (i + "").length;
    return n;
  };
});

bench("string/string-to-number", function () {
  var inputs = [];
  for (var i = 0; i < 1000; i++) inputs.push(String(i * 3.25), String(i), "0x" + i.toString(16));
  return function () {
    var sum = 0;
    for (var r = 0; r < 60; r++) for (var i = 0; i < inputs.length; i++) sum += parseFloat(inputs[i]) + parseInt(inputs[i], 10) + Number(inputs[i]);
    return Math.round(sum);
  };
});

bench("string/from-char-code-build", function () {
  return function () {
    var s = "";
    for (var i = 0; i < 20000; i++) s += String.fromCharCode(65 + (i % 26));
    return s.length;
  };
});

bench("string/starts-ends-with", function () {
  var urls = [];
  for (var i = 0; i < 500; i++) urls.push((i & 1 ? "https://" : "data:") + "example.org/path/" + i + (i & 2 ? ".png" : ".svg"));
  return function () {
    var hits = 0;
    for (var r = 0; r < 200; r++) for (var i = 0; i < urls.length; i++) {
      if (urls[i].startsWith("https://")) hits++;
      if (urls[i].endsWith(".png")) hits++;
    }
    return hits;
  };
});

bench("string/pad-and-repeat", function () {
  return function () {
    var n = 0;
    for (var i = 0; i < 50000; i++) n += String(i).padStart(8, "0").length + "-".repeat(i & 15).length;
    return n;
  };
});

bench("string/code-points-iteration", function () {
  var s = "";
  for (var i = 0; i < 500; i++) s += "aж中😀b";
  return function () {
    var n = 0;
    for (var r = 0; r < 20; r++) for (var ch of s) n += ch.length;
    return n;
  };
});

bench("string/escape-html-per-char", function () {
  var s = "";
  for (var i = 0; i < 300; i++) s += "Tom & Jerry <cat> said \"meow\" to 'mouse' ";
  return function () {
    var out = "";
    for (var i = 0; i < s.length; i++) {
      var c = s.charAt(i);
      out += c === "&" ? "&amp;" : c === "<" ? "&lt;" : c === ">" ? "&gt;" : c === "\"" ? "&quot;" : c;
    }
    return out.length;
  };
});
