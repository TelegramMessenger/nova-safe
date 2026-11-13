function sliceInto(Source, values, start, end, Target, targetByteOffset) {
  const buf = new ArrayBuffer(64);
  const src = new Source(buf, 0, values.length);
  src.set(values);
  src.constructor = {};
  src.constructor[Symbol.species] = function (n) {
    return new Target(buf, targetByteOffset, n);
  };
  const result = src.slice(start, end);
  return Array.from(result).concat(["|"], Array.from(src));
}

function check(name, actual, expected) {
  const a = actual.join(",");
  const e = expected.join(",");
  if (a !== e) {
    throw new Error(name + ": expected " + e + ", got " + a);
  }
}

const bytes = [];
for (let i = 0; i < 64; i++) bytes.push(i);

check(
  "Float64Array -> Uint8Array",
  sliceInto(Float64Array, [0.5, 1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5], 4, 8, Uint8Array, 0).slice(0, 4),
  [4, 5, 6, 7],
);
check(
  "Uint8Array -> Float64Array",
  sliceInto(Uint8Array, bytes, 8, 10, Float64Array, 32).slice(0, 2),
  [8, 9],
);
check(
  "Uint8Array -> Uint16Array",
  sliceInto(Uint8Array, bytes.slice(0, 16), 4, 8, Uint16Array, 8).slice(0, 4),
  [4, 5, 6, 7],
);
check(
  "Int16Array -> Int32Array, overlapping",
  sliceInto(Int16Array, [-3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12], 1, 6, Int32Array, 4),
  [-2, -2, -1, -2, -1, "|", -3, -2, -2, -1, -2, -1, -1, -1, -2, -1, -1, -1, 9, 10, 11, 12],
);
check(
  "Float32Array -> Float64Array, overlapping",
  sliceInto(Float32Array, [0.25, 1.25, 2.25, 3.25, 4.25, 5.25, 6.25, 7.25, 8.25, 9.25, 10.25, 11.25], 0, 4, Float64Array, 8),
  [0.25, 1.25, 0, 1.625, "|", 0.25, 1.25, 0, 1.625, 0, 1.90625, 0, 0, 0, 1.953125, 10.25, 11.25],
);
check(
  "Float64Array -> Int32Array",
  sliceInto(Float64Array, [-1.5, 4294967297.7, -0, NaN], 0, 4, Int32Array, 32).slice(0, 4),
  [-1, 1, 0, 0],
);
check(
  "Float64Array -> Uint8ClampedArray",
  sliceInto(Float64Array, [-1.5, 300.7, 255.9, -0], 0, 4, Uint8ClampedArray, 40).slice(0, 4),
  [0, 255, 255, 0],
);
check(
  "Int32Array -> Float32Array",
  sliceInto(Int32Array, [-7, 16777217, 3, -2147483648], 0, 4, Float32Array, 32).slice(0, 4),
  [-7, 16777216, 3, -2147483648],
);
check(
  "Uint8Array -> Uint8Array, overlapping forward",
  sliceInto(Uint8Array, [1, 2, 3, 4, 5, 6, 7, 8], 0, 6, Uint8Array, 2).slice(0, 6),
  [1, 2, 1, 2, 1, 2],
);
check(
  "Uint16Array -> Uint16Array, overlapping backward",
  sliceInto(Uint16Array, [1, 2, 3, 4, 5, 6, 7, 8, 9], 3, 9, Uint16Array, 0).slice(0, 6),
  [4, 5, 6, 7, 8, 9],
);
