function check(name, actual, expected) {
  if (actual !== expected) {
    throw new Error(name + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }
}
function message(f) {
  try { f(); } catch (err) { return err.constructor.name + ": " + err.message; }
  return "no error";
}
check("calling a non-function", message(() => { const o = {}; o.nope(); }), "TypeError: not a function");
check("calling undefined", message(() => { let u; u(); }), "TypeError: not a function");
check("calling a number", message(() => (1)()), "TypeError: not a function");
