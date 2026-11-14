function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}
function units(s) {
  const out = [];
  for (let i = 0; i < s.length; i++) out.push(s.charCodeAt(i).toString(16));
  return out.join(" ");
}

const x = 1;
check("lone lead", units(`\uD800`), "d800");
check("lone trail between parts", units(`a\uDC00b${x}\uDBFF`), "61 dc00 62 31 dbff");
check("replacement character stays", units(`�\uD83D`), "fffd d83d");
check("pair from escapes", units(`😀`), "d83d de00");
check("pair then lone trail", units(`😀\uDE00`), "d83d de00 de00");
check("length", `\uD800\uD800`.length, 2);
const tag = (strings) => units(strings[0]) + "/" + units(strings.raw[0]);
check("tagged cooked and raw", tag`\uD800z`, "d800 7a/5c 75 44 38 30 30 7a");
check("object literal key", units(Object.keys({ "\uD801": 1 })[0]), "d801");

const table = `ᵁ\uDAA5x`;
const codes = new Uint16Array(table.length);
for (let i = 0; i < table.length; i++) codes[i] = table.charCodeAt(i);
check("decode table", Array.from(codes).join(), "7489,55973,120,57344");
