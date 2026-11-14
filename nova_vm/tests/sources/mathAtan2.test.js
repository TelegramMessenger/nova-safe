function check(name, actual, expected) {
  if (!Object.is(actual, expected)) {
    throw new Error(name + ": expected " + String(expected) + ", got " + String(actual));
  }
}
check("second quadrant", Math.atan2(1, -1), 3 * Math.PI / 4);
check("third quadrant", Math.atan2(-1, -1), -3 * Math.PI / 4);
check("fourth quadrant", Math.atan2(-1, 1), -Math.PI / 4);
check("first quadrant", Math.atan2(1, 1), Math.PI / 4);
check("negative y, +Infinity x", Math.atan2(-1, Infinity), -0);
check("negative y, -Infinity x", Math.atan2(-1, -Infinity), -Math.PI);
check("negative y, zero x", Math.atan2(-1, 0), -Math.PI / 2);
check("-0 y, -0 x", Math.atan2(-0, -0), -Math.PI);
check("NaN", Math.atan2(NaN, 1), NaN);
