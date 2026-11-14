function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}
check("BigInt < unparsable string", 1n < "abc", false);
check("unparsable string < BigInt", "abc" < 1n, false);
check("BigInt >= unparsable string", 1n >= "x", false);
check("BigInt < parsable string", 1n < "2", true);
check("string > BigInt", "10" > 9n, true);
