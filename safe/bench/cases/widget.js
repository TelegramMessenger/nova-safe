// What widgets do: garbage under a small heap, closures, and whole small programs.
bench("gc/short-lived-objects", function () {
  return function () {
    var keep = null, n = 0;
    for (var i = 0; i < 300000; i++) {
      keep = { v: i, next: null, tag: "t" };
      n += keep.v & 1;
    }
    return n;
  };
});

bench("gc/linked-list-build-drop", function () {
  return function () {
    var total = 0;
    for (var r = 0; r < 10; r++) {
      var head = null;
      for (var i = 0; i < 20000; i++) head = { v: i, next: head };
      for (var p = head; p; p = p.next) total += p.v & 1;
    }
    return total;
  };
});

bench("gc/retained-megabytes-with-churn", function () {
  var retained = [];
  for (var i = 0; i < 30000; i++) retained.push({ id: i, name: "row" + i, v: [i, i + 1] });
  return function () {
    var n = 0;
    for (var i = 0; i < 100000; i++) { var t = { a: i, b: [i] }; n += t.b.length; }
    return n + retained.length;
  };
});

bench("gc/string-garbage", function () {
  return function () {
    var n = 0;
    for (var i = 0; i < 100000; i++) n += ("row-" + i + "-" + (i & 15)).length;
    return n;
  };
});

bench("gc/closures-created", function () {
  return function () {
    var fns = [], sum = 0;
    for (var i = 0; i < 50000; i++) fns.push(function () { return i; });
    for (var j = 0; j < fns.length; j += 97) sum += fns[j]();
    return sum;
  };
});

bench("app/render-list-to-html", function () {
  var items = [];
  for (var i = 0; i < 300; i++) items.push({ id: i, title: "Title <" + i + ">", done: (i & 3) === 0, tags: ["x", "y" + (i & 7)] });
  function escape(text) { return String(text).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); }
  return function () {
    var html = items.map(function (item) {
      return "<li data-id=\"" + item.id + "\" class=\"" + (item.done ? "done" : "") + "\"><span>" + escape(item.title) + "</span>" +
        item.tags.map(function (t) { return "<i>" + escape(t) + "</i>"; }).join("") + "</li>";
    }).join("");
    return html.length;
  };
});

bench("app/particles-step", function () {
  var particles = [];
  for (var i = 0; i < 2000; i++) particles.push({ x: i % 320, y: (i * 7) % 240, vx: (i % 5) - 2, vy: (i % 3) - 1, life: 100 + (i & 31) });
  return function () {
    var alive = 0;
    for (var step = 0; step < 20; step++) {
      for (var i = 0; i < particles.length; i++) {
        var p = particles[i];
        p.vy += 0.05;
        p.x += p.vx;
        p.y += p.vy;
        if (p.x < 0 || p.x > 320) p.vx = -p.vx;
        if (p.y > 240) { p.y = 240; p.vy = -p.vy * 0.6; }
        if (--p.life > 0) alive++; else p.life = 100;
      }
    }
    return alive;
  };
});

bench("app/canvas-command-buffer", function () {
  return function () {
    var commands = [];
    for (var frame = 0; frame < 20; frame++) {
      commands.length = 0;
      for (var i = 0; i < 500; i++) {
        commands.push("beginPath", "arc", i * 0.5, frame + i * 0.25, 3, 0, 6.283, "fillStyle", "rgb(" + (i & 255) + ",10," + (frame * 12) + ")", "fill");
      }
    }
    return commands.length + commands[8].length;
  };
});

bench("app/tree-build-and-walk", function () {
  function node(tag, depth) {
    var n = { tag: tag, attrs: { id: tag + depth, "class": "c" + (depth & 3) }, children: [] };
    if (depth > 0) for (var i = 0; i < 4; i++) n.children.push(node("n" + i, depth - 1));
    return n;
  }
  function walk(n) {
    var count = 1 + (n.attrs["class"] === "c1" ? 1 : 0);
    for (var i = 0; i < n.children.length; i++) count += walk(n.children[i]);
    return count;
  }
  return function () {
    var total = 0;
    for (var r = 0; r < 5; r++) total += walk(node("root", 5));
    return total;
  };
});

bench("app/serialize-ops-json", function () {
  return function () {
    var total = 0;
    for (var batch = 0; batch < 20; batch++) {
      var ops = [];
      for (var i = 0; i < 200; i++) ops.push({ op: "set", id: i, name: "style", value: "left:" + i + "px;top:" + batch + "px" }, { op: "text", id: i, value: "Label " + i });
      total += JSON.stringify(ops).length;
    }
    return total;
  };
});

bench("app/event-emitter", function () {
  var listeners = {};
  function on(type, fn) { (listeners[type] || (listeners[type] = [])).push(fn); }
  function emit(type, payload) { var list = listeners[type]; if (list) for (var i = 0; i < list.length; i++) list[i](payload); }
  var state = { clicks: 0, moves: 0 };
  on("click", function (e) { state.clicks += e.x & 1; });
  on("click", function (e) { state.clicks += 1; });
  on("move", function (e) { state.moves += e.y & 1; });
  return function () {
    state.clicks = 0; state.moves = 0;
    for (var i = 0; i < 50000; i++) { emit("click", { x: i, y: 2 }); emit("move", { x: 1, y: i }); }
    return state.clicks + state.moves;
  };
});

bench("app/tokenize-and-eval", function () {
  var source = "";
  for (var i = 0; i < 200; i++) source += "(" + i + " + 3) * 2 - " + (i & 7) + " / 4 + ";
  source += "1";
  return function () {
    var tokens = [], at = 0;
    while (at < source.length) {
      var c = source.charCodeAt(at);
      if (c === 32) { at++; continue; }
      if (c >= 48 && c <= 57) {
        var start = at;
        while (at < source.length && source.charCodeAt(at) >= 48 && source.charCodeAt(at) <= 57) at++;
        tokens.push(+source.slice(start, at));
      } else { tokens.push(source[at]); at++; }
    }
    return tokens.length;
  };
});

bench("app/easing-animation-frame", function () {
  var nodes = [];
  for (var i = 0; i < 300; i++) nodes.push({ from: i, to: i * 2 + 10, style: { left: "", opacity: "" } });
  function ease(t) { return t < 0.5 ? 2 * t * t : -1 + (4 - 2 * t) * t; }
  return function () {
    var n = 0;
    for (var frame = 0; frame < 30; frame++) {
      var t = ease(frame / 30);
      for (var i = 0; i < nodes.length; i++) {
        var node = nodes[i];
        node.style.left = (node.from + (node.to - node.from) * t).toFixed(1) + "px";
        node.style.opacity = String(Math.round(t * 100) / 100);
        n += node.style.left.length;
      }
    }
    return n;
  };
});
