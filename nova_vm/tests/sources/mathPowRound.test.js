function check(name, actual, expected) {
  if (!Object.is(actual, expected)) {
    throw new Error(name + ": expected " + String(expected) + ", got " + String(actual));
  }
}
check("negative base, even exponent", Math.pow(-0.5, 2), 0.25);
check("operator too", (-0.5) ** 2, 0.25);
check("negative base, odd exponent", Math.pow(-0.5, 3), -0.125);
check("negative integer base", Math.pow(-2, 4), 16);
check("negative base, negative exponent", Math.pow(-2, -2), 0.25);
check("negative base, fraction", Math.pow(-0.5, 0.5), NaN);
check("easing outBack", 1 + 2.70158 * Math.pow(0.5 - 1, 3) + 1.70158 * Math.pow(0.5 - 1, 2), 1.0876975);
check("|base| > 1 to -Infinity", Math.pow(2, -Infinity), 0);
check("|base| < 1 to -Infinity", Math.pow(0.5, -Infinity), Infinity);
check("-2 to -Infinity", Math.pow(-2, -Infinity), 0);

check("round half up, negative", Math.round(-2.5), -2);
check("round -1.5", Math.round(-1.5), -1);
check("round 2.5", Math.round(2.5), 3);
check("round -0.5", Math.round(-0.5), -0);
check("round -0.2", Math.round(-0.2), -0);
check("round -3.7", Math.round(-3.7), -4);
check("round just under a half", Math.round(0.49999999999999994), 0);
check("round 1.4999999999999998", Math.round(1.4999999999999998), 1);
check("round big", Math.round(-4503599627370495.5), -4503599627370495);
