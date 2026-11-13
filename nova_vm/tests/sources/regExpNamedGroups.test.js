function check(name, actual, expected) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new Error(name + ": expected " + e + ", got " + a);
  }
}

const date = /(?<year>\d{4})-(?<month>\d{2})(?:-(?<day>\d{2}))?/;
const m = date.exec("Дата: 2026-11");
check("groups", { ...m.groups }, { year: "2026", month: "11", day: undefined });
check("groups has no prototype", Object.getPrototypeOf(m.groups), null);
check("groups keys in order", Object.keys(m.groups), ["year", "month", "day"]);
check("index", m.index, 6);
check("no named groups", /(\d)/.exec("1").groups, undefined);
check("match", "мир 42".match(/(?<n>\d+)/).groups.n, "42");
check(
  "matchAll",
  [..."a=1, б=2".matchAll(/(?<k>\S)=(?<v>\d)/g)].map((m) => m.groups.k + m.groups.v),
  ["a1", "б2"],
);
check("$<name>", "2026-11".replace(date, "$<month>/$<year>"), "11/2026");
check("$<missing>", "2026-11".replace(date, "[$<day>]"), "[]");
check("$<unknown>", "2026-11".replace(date, "[$<nope>]"), "[]");
check("$< without >", "2026-11".replace(date, "[$<year]"), "[$<year]");
check("$<name> global Cyrillic", "мир1 мир2".replace(/мир(?<d>\d)/g, "<$<d>>"), "<1> <2>");
check(
  "replacer gets groups",
  "x=5".replace(/(?<k>\w)=(?<v>\d)/, (...args) => JSON.stringify(args[args.length - 1])),
  '{"k":"x","v":"5"}',
);
check(
  "replacer without groups",
  "x=5".replace(/(\w)=(\d)/, (...args) => typeof args[args.length - 1]),
  "string",
);
check("split keeps named captures", "a1b".split(/(?<d>\d)/), ["a", "1", "b"]);
