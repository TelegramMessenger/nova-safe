(function () {
  "use strict";
  const evalScript = globalThis.__evalScript;
  const emit = globalThis.__emit;
  const html = globalThis.__WIDGET_HTML;
  const bundle = globalThis.__BUNDLE_SRC;
  const virtualEndMs = 1500;
  const maxTasks = 400;

  const out = [];
  let now = 1000;
  let seed = 0x2545f491;
  let sequence = 0;
  let frameWanted = false;
  let nextFrameAt = now;
  const timers = new Map();

  function nextRandom() {
    seed ^= seed << 13;
    seed >>>= 0;
    seed ^= seed >>> 17;
    seed ^= seed << 5;
    seed >>>= 0;
    return seed;
  }

  const RealDate = Date;
  class VirtualDate extends RealDate {
    constructor(...args) {
      if (args.length === 0) super(now);
      else super(...args);
    }
    static now() {
      return now;
    }
  }
  globalThis.Date = VirtualDate;
  Math.random = () => nextRandom() / 4294967296;

  globalThis.__host = function (kind, arg) {
    switch (kind) {
      case "ops":
        out.push(["ops", arg]);
        return undefined;
      case "query":
        out.push(["query", arg]);
        return '{"error":"no host"}';
      case "now":
        return now;
      case "timer": {
        const pair = JSON.parse(arg);
        const delay = Math.min(Math.max(0, Number(pair[1]) || 0), 60000);
        timers.set(pair[0], { at: now + delay, sequence: sequence++ });
        return undefined;
      }
      case "clearTimer":
        timers.delete(Number(arg));
        return undefined;
      case "raf":
        frameWanted = true;
        return undefined;
      case "log":
      case "diag":
      case "listen":
      case "pd":
      case "anim":
      case "bridge":
      case "storage":
        out.push([kind, arg]);
        return undefined;
      case "intl":
        out.push(["intl", arg]);
        return '{"error":"intl unavailable"}';
      case "random": {
        const n = Math.min(Math.max(Number(arg) | 0, 0), 65536);
        const bytes = [];
        for (let i = 0; i < n; i++) bytes.push(nextRandom() & 255);
        return "[" + bytes.join(",") + "]";
      }
      default:
        return undefined;
    }
  };
  globalThis.__hostBinary = function () {
    return null;
  };

  function describe(error) {
    if (error && typeof error === "object") return String(error.name) + ": " + String(error.message);
    return "throw " + String(error);
  }

  try {
    evalScript(bundle);
  } catch (error) {
    emit(JSON.stringify({ fatal: "bundle: " + describe(error), out }));
    return;
  }
  const dispatch = globalThis.__twDispatch;
  if (typeof dispatch !== "function") {
    emit(JSON.stringify({ fatal: "no __twDispatch", out }));
    return;
  }

  function call(kind, arg) {
    try {
      const value = dispatch(kind, arg);
      return value === undefined || value === null ? null : String(value);
    } catch (error) {
      out.push(["error", kind, describe(error)]);
      return null;
    }
  }

  async function drain() {
    for (let i = 0; i < 64; i++) await undefined;
  }

  async function settle() {
    for (let i = 0; i < 20; i++) {
      await drain();
      if (call("endTick", "") !== "1") break;
    }
  }

  async function pumpScripts() {
    for (let i = 0; i < 1000; i++) {
      const next = call("nextScript", "");
      if (!next) break;
      const script = JSON.parse(next);
      const id = script.id | 0;
      let ok = true;
      if (typeof script.src === "string") {
        ok = false;
        out.push(["script-skipped", script.src.slice(0, 200)]);
      } else if (typeof script.code === "string") {
        try {
          evalScript(script.code);
        } catch (error) {
          out.push(["script-error", id, describe(error)]);
        }
      }
      await settle();
      call("scriptDone", "[" + id + "," + ok + "]");
      await settle();
    }
  }

  async function task(kind, arg) {
    out.push(["task", kind, arg]);
    call(kind, arg);
    await settle();
    await pumpScripts();
  }

  async function main() {
    const config = {
      memoryLimitBytes: 8 << 20,
      cpuLimitMs: 500,
      cpuQuotaMs: 300,
      cpuQuotaWindowMs: 1000,
      cpuKillAfterMs: 0,
      mainThreadQuotaMs: 100,
      maxTimers: 64,
      visible: true,
      policy: "strict",
      viewport: { innerWidth: 320, innerHeight: 480, dpr: 2 },
      theme: "light",
      themeParams: {},
      locale: "en-US",
      contentSafeArea: { top: 0, right: 0, bottom: 0, left: 0, cornerLeft: 0, cornerRight: 0 },
      profile: 1,
      storage: {},
      features: [],
      isCopy: false,
      canQuery: false,
      isPreview: false,
      chatType: null,
      viewer: null,
      wallet: false,
    };
    await task("boot", JSON.stringify({ html, config }));
    await task("loaded", "");
    let tasks = 2;
    while (tasks < maxTasks) {
      let timerId = null;
      let timer = null;
      for (const [id, entry] of timers) {
        if (!timer || entry.at < timer.at || (entry.at === timer.at && entry.sequence < timer.sequence)) {
          timer = entry;
          timerId = id;
        }
      }
      const timerAt = timer ? timer.at : Infinity;
      const frameAt = frameWanted ? Math.max(nextFrameAt, now) : Infinity;
      const at = Math.min(timerAt, frameAt);
      if (at === Infinity || at > virtualEndMs + 1000) break;
      now = at;
      if (timerAt <= frameAt) {
        timers.delete(timerId);
        await task("timer", String(timerId));
      } else {
        frameWanted = false;
        nextFrameAt = Math.floor(now / 16) * 16 + 16;
        await task("frame", String(now));
      }
      tasks++;
    }
    emit(JSON.stringify({ out, tasks, pendingTimers: timers.size }));
  }

  main().catch((error) => emit(JSON.stringify({ fatal: "driver: " + describe(error), out })));
})();
