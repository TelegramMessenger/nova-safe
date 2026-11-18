// A case file calls bench(name, setup): setup() builds what the case needs
// and returns the function to time. The runner picks one case per process
// with __select (-1 lists the names). A case is timed until it has run for
// 300 ms (3 to 40 runs, at most about 3 s); min and median are reported in
// milliseconds, with what the function returned, so that the two engines can
// be checked for the same answer.
var __index = 0;
function bench(name, setup) {
  var index = __index++;
  if (__select < 0) {
    __emit(JSON.stringify({ list: name }));
    return;
  }
  if (index !== __select) return;
  var result = { name: name };
  try {
    var run = setup();
    var started = __now();
    var check = run();
    var first = __now() - started;
    var times = [first];
    var total = first;
    while ((times.length < 3 || total < 300) && times.length < 40 && total < 3000) {
      started = __now();
      run();
      var took = __now() - started;
      times.push(took);
      total += took;
    }
    times.sort(function (a, b) { return a - b; });
    result.runs = times.length;
    result.min = times[0];
    result.med = times[times.length >> 1];
    result.first = first;
    result.check = String(check).slice(0, 60);
  } catch (e) {
    result.error = String(e && e.name ? e.name + ": " + e.message : e).slice(0, 200);
  }
  __emit(JSON.stringify(result));
}
