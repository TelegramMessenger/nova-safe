// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![allow(clippy::undocumented_unsafe_blocks)]

use core::{cell::Cell, ffi::c_void, ptr};

use super::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let len = self.below(max as u64 + 1) as usize;
        (0..len).map(|_| self.next() as u8).collect()
    }
}

struct Context {
    rng: Cell<u64>,
    engine: Cell<*const NovaEngine>,
    calls: Cell<usize>,
    refused: Cell<usize>,
    interrupts: Cell<usize>,
}

unsafe extern "C" fn host(
    context: *mut c_void,
    id: u32,
    args: *const NovaArg,
    argc: usize,
    ret: *mut NovaReturn,
) {
    let context = unsafe { &*(context as *const Context) };
    context.calls.set(context.calls.get() + 1);
    let mut checksum = 0u64;
    for index in 0..argc {
        let arg = unsafe { &*args.add(index) };
        if !arg.bytes.is_null() {
            let bytes = unsafe { core::slice::from_raw_parts(arg.bytes, arg.len) };
            checksum = bytes
                .iter()
                .fold(checksum, |sum, byte| sum.wrapping_add(*byte as u64));
        }
    }
    let engine = context.engine.get();
    if !engine.is_null() && id == 98 {
        unsafe { nova_engine_free(engine as *mut NovaEngine) };
        let mut out = blank();
        assert_eq!(
            unsafe { nova_engine_eval(engine, b"1".as_ptr(), 1, false, &mut out) },
            NOVA_REFUSED
        );
        unsafe { nova_result_free(&mut out) };
        context.engine.set(ptr::null());
    }
    if !engine.is_null() && id == 99 {
        let mut out = blank();
        let status = unsafe { nova_engine_eval(engine, b"1".as_ptr(), 1, false, &mut out) };
        assert_eq!(status, NOVA_REFUSED);
        unsafe { nova_result_free(&mut out) };
        assert_eq!(
            unsafe { nova_engine_set_memory_limit(engine, 1) },
            NOVA_REFUSED
        );
        context.refused.set(context.refused.get() + 1);
    }
    let mut rng = Rng(context.rng.get() ^ checksum ^ 0x9e37_79b9);
    let choice = rng.below(6);
    let payload = rng.bytes(40);
    context.rng.set(rng.next());
    unsafe {
        match choice {
            0 => {}
            1 => nova_return_null(ret),
            2 => nova_return_number(ret, f64::from_bits(rng.next())),
            3 => nova_return_string(ret, payload.as_ptr(), payload.len()),
            4 => nova_return_bytes(ret, payload.as_ptr(), payload.len()),
            _ => {
                nova_return_string(ret, ptr::null(), 5);
                nova_return_bytes(ret, ptr::null(), 0);
            }
        }
    }
}

unsafe extern "C" fn interrupt(context: *mut c_void) -> bool {
    let context = unsafe { &*(context as *const Context) };
    context.interrupts.set(context.interrupts.get() + 1);
    false
}

fn blank() -> NovaResult {
    NovaResult {
        kind: -1,
        number: 0.0,
        bytes: ptr::null_mut(),
        len: 0,
    }
}

fn text(result: &NovaResult) -> String {
    if result.bytes.is_null() {
        return String::new();
    }
    String::from_utf8_lossy(unsafe { core::slice::from_raw_parts(result.bytes, result.len) })
        .into_owned()
}

const SNIPPETS: &[&str] = &[
    "__h('a', 1, 2.5, new Float64Array([1, 2]), new Int32Array(3), {}, null)",
    "__b(new Float64Array(new ArrayBuffer(24), 8, 1), 'x')",
    "__b(new Int32Array(0), '')",
    "var r = __h(); typeof r === 'object' && r ? r.byteLength : r",
    "Promise.resolve(1).then(v => __h(v))",
    "Promise.reject(new Error('rej'))",
    "throw new TypeError('t')",
    "function g(a, b) { return a + b; } 'ok'",
    "delete globalThis.__h",
    "for (let i = 0; i < 100; i++) __h(i)",
    "JSON.stringify({a: [1, 2, {b: 'c'}]})",
    "new Array(1000).fill(0).map((_, i) => i * 2).join(',').length",
    "'\\ud800' + 'x'",
    "__reenter()",
    "(",
    "export const x = 1",
];

#[test]
fn random_and_invalid_inputs_never_misbehave() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for round in 0..24 {
        let context = Box::new(Context {
            rng: Cell::new(rng.next()),
            engine: Cell::new(ptr::null()),
            calls: Cell::new(0),
            refused: Cell::new(0),
            interrupts: Cell::new(0),
        });
        let context_ptr = &*context as *const Context as *mut c_void;
        let mut error = blank();
        let engine = unsafe {
            nova_engine_new(
                if round % 3 == 0 { 0 } else { 64 << 20 },
                0,
                Some(host),
                Some(interrupt),
                context_ptr,
                &mut error,
            )
        };
        assert!(!engine.is_null(), "{}", text(&error));
        unsafe { nova_result_free(&mut error) };
        context.engine.set(engine);
        let mut out = blank();
        assert_eq!(
            unsafe {
                nova_engine_register(engine, b"__h".as_ptr(), 3, 1, 2, NOVA_ARGS_VALUES, &mut out)
            },
            NOVA_OK
        );
        assert_eq!(
            unsafe {
                nova_engine_register(
                    engine,
                    b"__b".as_ptr(),
                    3,
                    2,
                    2,
                    NOVA_ARGS_VALUES_AND_BYTES,
                    &mut out,
                )
            },
            NOVA_OK
        );
        assert_eq!(
            unsafe {
                nova_engine_register(
                    engine,
                    b"__reenter".as_ptr(),
                    9,
                    99,
                    0,
                    NOVA_ARGS_VALUES,
                    &mut out,
                )
            },
            NOVA_OK
        );
        assert_eq!(
            unsafe { nova_engine_register(engine, b"__x".as_ptr(), 3, 3, 0, 7, &mut out) },
            NOVA_INVALID
        );
        assert_eq!(
            unsafe { nova_engine_register(engine, b"\xff".as_ptr(), 1, 3, 0, 0, &mut out) },
            NOVA_INVALID
        );
        assert_eq!(
            unsafe { nova_engine_register(engine, ptr::null(), 4, 3, 0, 0, &mut out) },
            NOVA_INVALID
        );
        assert_eq!(
            unsafe { nova_engine_register(engine, ptr::null(), 0, 3, 0, 0, &mut out) },
            NOVA_INVALID
        );
        unsafe { nova_result_free(&mut out) };

        for _ in 0..40 {
            let mut out = blank();
            let status = match rng.below(7) {
                0 | 1 => {
                    let snippet = SNIPPETS[rng.below(SNIPPETS.len() as u64) as usize];
                    let as_module = rng.below(5) == 0;
                    unsafe {
                        nova_engine_eval(
                            engine,
                            snippet.as_ptr(),
                            snippet.len(),
                            as_module,
                            &mut out,
                        )
                    }
                }
                2 => {
                    let source = rng.bytes(64);
                    unsafe {
                        nova_engine_eval(engine, source.as_ptr(), source.len(), false, &mut out)
                    }
                }
                3 => {
                    let names: [&[u8]; 5] = [b"g", b"__h", b"nothing", b"", b"\xc3("];
                    let name = names[rng.below(5) as usize];
                    let arg_bytes: Vec<Vec<u8>> =
                        (0..rng.below(4)).map(|_| rng.bytes(12)).collect();
                    let args: Vec<NovaStr> = arg_bytes
                        .iter()
                        .map(|bytes| NovaStr {
                            ptr: if bytes.is_empty() {
                                ptr::null()
                            } else {
                                bytes.as_ptr()
                            },
                            len: bytes.len(),
                        })
                        .collect();
                    if rng.below(4) == 0 {
                        let mut captured = true;
                        unsafe {
                            nova_engine_capture(
                                engine,
                                name.as_ptr(),
                                name.len(),
                                &mut captured,
                                &mut out,
                            )
                        }
                    } else {
                        unsafe {
                            nova_engine_call(
                                engine,
                                name.as_ptr(),
                                name.len(),
                                args.as_ptr(),
                                args.len(),
                                &mut out,
                            )
                        }
                    }
                }
                4 => {
                    let mut ran = false;
                    unsafe { nova_engine_run_job(engine, &mut ran, &mut out) }
                }
                5 => {
                    let bad = NovaStr {
                        ptr: ptr::null(),
                        len: 3,
                    };
                    let status =
                        unsafe { nova_engine_call(engine, b"g".as_ptr(), 1, &bad, 1, &mut out) };
                    assert_eq!(status, NOVA_INVALID);
                    unsafe { nova_result_free(&mut out) };
                    let status = unsafe {
                        nova_engine_call(engine, b"g".as_ptr(), 1, ptr::null(), 2, &mut out)
                    };
                    assert_eq!(status, NOVA_INVALID);
                    unsafe { nova_result_free(&mut out) };
                    unsafe { nova_engine_eval(engine, ptr::null(), 10, false, ptr::null_mut()) }
                }
                _ => {
                    let _ = unsafe { nova_engine_has_pending_job(engine) };
                    let _ = unsafe { nova_engine_memory_used(engine) };
                    unsafe { nova_engine_collect_garbage(engine) }
                }
            };
            assert!(
                (NOVA_OK..=NOVA_INVALID).contains(&status),
                "status {status}"
            );
            assert_ne!(status, NOVA_DEAD, "{}", text(&out));
            assert_ne!(status, NOVA_REFUSED);
            if status != NOVA_OK || out.kind == NOVA_VALUE_STRING {
                let _ = text(&out);
            }
            unsafe { nova_result_free(&mut out) };
            unsafe { nova_result_free(&mut out) };
        }
        let mut ran = true;
        while ran {
            let mut out = blank();
            unsafe { nova_engine_run_job(engine, &mut ran, &mut out) };
            unsafe { nova_result_free(&mut out) };
        }
        unsafe { nova_engine_free(engine) };
        assert!(context.calls.get() > 0 || round > 0);
    }
}

#[test]
fn null_handles_and_results_are_refused() {
    let mut out = blank();
    unsafe {
        assert_eq!(
            nova_engine_eval(ptr::null(), b"1".as_ptr(), 1, false, &mut out),
            NOVA_INVALID
        );
        nova_result_free(&mut out);
        assert_eq!(
            nova_engine_call(
                ptr::null(),
                b"f".as_ptr(),
                1,
                ptr::null(),
                0,
                ptr::null_mut()
            ),
            NOVA_INVALID
        );
        assert_eq!(
            nova_engine_run_job(ptr::null(), ptr::null_mut(), ptr::null_mut()),
            NOVA_INVALID
        );
        assert!(!nova_engine_has_pending_job(ptr::null()));
        assert_eq!(nova_engine_memory_used(ptr::null()), 0);
        nova_engine_free(ptr::null_mut());
        nova_result_free(ptr::null_mut());
        nova_return_null(ptr::null_mut());
        nova_return_string(ptr::null_mut(), b"x".as_ptr(), 1);
        let engine = nova_engine_new(0, 0, None, None, ptr::null_mut(), &mut out);
        assert!(engine.is_null());
        assert_eq!(text(&out), "no host function callback");
        nova_result_free(&mut out);
    }
    let version = unsafe { core::ffi::CStr::from_ptr(nova_embed_version().cast()) };
    assert!(version.to_str().unwrap().starts_with("nova_embed "));
}

#[test]
fn reentrant_calls_are_refused() {
    let context = Box::new(Context {
        rng: Cell::new(7),
        engine: Cell::new(ptr::null()),
        calls: Cell::new(0),
        refused: Cell::new(0),
        interrupts: Cell::new(0),
    });
    let engine = unsafe {
        nova_engine_new(
            0,
            0,
            Some(host),
            None,
            &*context as *const Context as *mut c_void,
            ptr::null_mut(),
        )
    };
    context.engine.set(engine);
    let mut out = blank();
    unsafe {
        assert_eq!(
            nova_engine_register(
                engine,
                b"__reenter".as_ptr(),
                9,
                99,
                0,
                NOVA_ARGS_VALUES,
                &mut out
            ),
            NOVA_OK
        );
        assert_eq!(
            nova_engine_eval(engine, b"__reenter(); 5".as_ptr(), 14, false, &mut out),
            NOVA_OK
        );
        assert_eq!(out.kind, NOVA_VALUE_NUMBER);
        assert_eq!(out.number, 5.0);
        nova_result_free(&mut out);
        assert_eq!(context.refused.get(), 1);
        assert_eq!(
            nova_engine_register(
                engine,
                b"__free".as_ptr(),
                6,
                98,
                0,
                NOVA_ARGS_VALUES,
                &mut out
            ),
            NOVA_OK
        );
        nova_result_free(&mut out);
        let status = nova_engine_eval(engine, b"__free(); 6".as_ptr(), 11, false, &mut out);
        assert_eq!(status, NOVA_OK);
        assert_eq!(out.number, 6.0);
        nova_result_free(&mut out);
        assert_eq!(context.calls.get(), 2);
        assert!(
            context.engine.get().is_null(),
            "freed when the eval returned"
        );
    }
}

unsafe extern "C" fn plus_four(_context: *mut c_void, _epoch_seconds: i64) -> i32 {
    4 * 3600
}

#[test]
fn the_time_zone_callback_sets_local_time() {
    let mut out = blank();
    unsafe {
        let engine = nova_engine_new(0, 0, Some(host), None, ptr::null_mut(), ptr::null_mut());
        let source = b"new Date(Date.UTC(2026, 0, 1, 20)).getHours()";
        assert_eq!(
            nova_engine_eval(engine, source.as_ptr(), source.len(), false, &mut out),
            NOVA_OK
        );
        assert_eq!(out.number, 20.0);
        assert_eq!(nova_engine_set_time_zone(engine, Some(plus_four)), NOVA_OK);
        assert_eq!(
            nova_engine_eval(engine, source.as_ptr(), source.len(), false, &mut out),
            NOVA_OK
        );
        assert_eq!(out.number, 0.0);
        assert_eq!(nova_engine_set_time_zone(engine, None), NOVA_OK);
        assert_eq!(nova_engine_set_time_zone(ptr::null(), None), NOVA_INVALID);
        nova_engine_free(engine);
    }
}

unsafe extern "C" fn quiet_host(
    _context: *mut c_void,
    _id: u32,
    _args: *const NovaArg,
    _argc: usize,
    _ret: *mut NovaReturn,
) {
}

#[test]
fn compiled_scripts_evaluate_in_many_engines() {
    let source = b"var n = (globalThis.n || 0) + 1; `n=${n}`";
    let mut out = blank();
    let script = unsafe { nova_script_compile(source.as_ptr(), source.len(), &mut out) };
    assert!(!script.is_null());
    assert_eq!(out.kind, NOVA_VALUE_UNDEFINED);
    let shared = script as usize;
    std::thread::scope(|scope| {
        for _ in 0..3 {
            scope.spawn(move || unsafe {
                let script = shared as *const NovaScript;
                let mut out = blank();
                let engine =
                    nova_engine_new(0, 0, Some(quiet_host), None, ptr::null_mut(), &mut out);
                assert!(!engine.is_null());
                for expected in ["n=1", "n=2"] {
                    assert_eq!(nova_engine_eval_script(engine, script, &mut out), NOVA_OK);
                    assert_eq!(text(&out), expected);
                    nova_result_free(&mut out);
                }
                nova_engine_free(engine);
            });
        }
    });
    unsafe {
        assert_eq!(
            nova_engine_eval_script(ptr::null(), script, &mut out),
            NOVA_INVALID
        );
        nova_result_free(&mut out);
        let engine = nova_engine_new(0, 0, Some(quiet_host), None, ptr::null_mut(), &mut out);
        assert_eq!(
            nova_engine_eval_script(engine, ptr::null(), &mut out),
            NOVA_INVALID
        );
        assert_eq!(text(&out), "no script");
        nova_result_free(&mut out);
        nova_engine_free(engine);
        nova_script_free(script);
        nova_script_free(ptr::null_mut());
    }
}

#[test]
fn compile_errors_are_reported() {
    let mut out = blank();
    unsafe {
        let script = nova_script_compile(b"let = = 1".as_ptr(), 9, &mut out);
        assert!(script.is_null());
        assert!(text(&out).starts_with("SyntaxError: "), "{}", text(&out));
        nova_result_free(&mut out);
        let script = nova_script_compile(b"\xff".as_ptr(), 1, &mut out);
        assert!(script.is_null());
        assert_eq!(text(&out), "the source is not UTF-8");
        nova_result_free(&mut out);
        let script = nova_script_compile(ptr::null(), 3, ptr::null_mut());
        assert!(script.is_null());
    }
}

fn eval(engine: *const NovaEngine, source: &str) -> i32 {
    let mut out = blank();
    let status =
        unsafe { nova_engine_eval(engine, source.as_ptr(), source.len(), false, &mut out) };
    unsafe { nova_result_free(&mut out) };
    status
}

fn quiet_engine(memory_limit: usize) -> *mut NovaEngine {
    let engine = unsafe {
        nova_engine_new(
            memory_limit,
            0,
            Some(quiet_host),
            None,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert!(!engine.is_null());
    engine
}

#[test]
fn memory_used_is_what_the_engine_allocated() {
    let engine = quiet_engine(0);
    let fresh = unsafe { nova_engine_memory_used(engine) };
    // A realm with its builtins is some hundred kilobytes.
    assert!(fresh > 100 << 10, "{fresh}");
    assert_eq!(
        eval(engine, "var a = new Uint8Array(4 << 20); a.length"),
        NOVA_OK
    );
    let grown = unsafe { nova_engine_memory_used(engine) };
    assert!(grown >= fresh + (4 << 20), "{fresh} {grown}");
    assert_eq!(eval(engine, "a = null"), NOVA_OK);
    assert_eq!(unsafe { nova_engine_collect_garbage(engine) }, NOVA_OK);
    let released = unsafe { nova_engine_memory_used(engine) };
    assert!(released < fresh + (1 << 20), "{fresh} {released}");
    unsafe { nova_engine_free(engine) };
}

/// Whatever a script grows, the limit ends it close to the limit's bytes of
/// real allocations: structures the heap's own estimate leaves out (hash
/// tables, spare capacity, shapes) among them.
#[test]
fn the_memory_limit_bounds_allocated_bytes() {
    const LIMIT: usize = 16 << 20;
    let growth = [
        "var keep = []; for (;;) keep.push({ n: keep.length, s: 'x' + keep.length });",
        "var keep = {}; for (var i = 0;; i++) keep['key' + i] = i;",
        "var keep = new Map(); for (var i = 0;; i++) keep.set('k' + i, [i]);",
        "var keep = new Set(); for (var i = 0;; i++) keep.add({ i });",
        "var keep = []; for (var i = 0;; i++) { var o = {}; o['p' + i] = 1; o['q' + i] = 2; keep.push(o); }",
        "var keep = []; for (var i = 0;; i++) { let a = i, b = i + 1; keep.push(() => a + b); }",
        "var keep = []; for (var i = 0;; i++) keep.push(function named() { return i; }.bind(null, i));",
        "var keep = []; for (var i = 0;; i++) keep.push(new RegExp('a{' + i + '}b'));",
        "var keep = []; for (var i = 0;; i++) keep.push(Symbol('s' + i), new Promise(() => {}));",
        "var keep = 'x'; for (;;) { try { keep = keep + keep + 'y'; } catch (e) {} }",
        "var keep = []; for (var i = 0;; i++) { try { keep.push(new Float64Array(1 << 16)); } catch (e) {} }",
        "var keep = []; keep[1e6] = 1; for (var i = 0;; i++) keep[1e6 + i * 7] = i;",
    ];
    for source in growth {
        let engine = quiet_engine(LIMIT);
        assert_eq!(eval(engine, source), NOVA_OUT_OF_MEMORY, "{source}");
        let used = unsafe { nova_engine_memory_used(engine) };
        assert!(used <= LIMIT * 2, "{used} bytes after: {source}");
        // The engine runs on once the script lets go.
        assert_eq!(eval(engine, "keep = null"), NOVA_OK, "{source}");
        assert_eq!(unsafe { nova_engine_collect_garbage(engine) }, NOVA_OK);
        assert_eq!(eval(engine, "[1, 2, 3].length"), NOVA_OK, "{source}");
        let used = unsafe { nova_engine_memory_used(engine) };
        assert!(used < LIMIT / 2, "{used} bytes kept after: {source}");
        unsafe { nova_engine_free(engine) };
    }
}

/// Blocks return their bytes to the engine that allocated them, so engines
/// that come and go leave nothing on an account for the next one.
#[test]
fn accounts_do_not_drift_between_engines() {
    let mut first = 0;
    for round in 0..40 {
        let engine = quiet_engine(0);
        let fresh = unsafe { nova_engine_memory_used(engine) };
        if round == 0 {
            first = fresh;
        }
        // The same realm every time: nothing of the engines before it is
        // on this one's account. (Other tests' engines run at the same
        // time, on other accounts.)
        assert!(fresh.abs_diff(first) < 64 << 10, "{first} {fresh}");
        let mut out = blank();
        let source = "var a = []; for (var i = 0; i < 20000; i++) a.push('v' + i); a.join(',')";
        assert_eq!(
            unsafe { nova_engine_eval(engine, source.as_ptr(), source.len(), false, &mut out) },
            NOVA_OK
        );
        // The result outlives the engine: its bytes leave the account when
        // it is freed, whoever has the account by then.
        unsafe { nova_engine_free(engine) };
        assert!(out.len > 100_000);
        unsafe { nova_result_free(&mut out) };
    }
}

/// A script that builds the same text again, step by step, makes at every
/// step the string it made the time before, and the string table finds it.
/// Finding it joined both texts, the whole text copied at every step: with
/// the memory limit counting what the engine allocates, and collecting less
/// often for it, the joins of a few runs were more than the limit allows
/// between two collections, and the engine ended as out of memory with a
/// few megabytes in use.
#[test]
fn a_text_built_again_is_not_out_of_memory() {
    const LIMIT: usize = 16 << 20;
    let engine = quiet_engine(LIMIT);
    assert_eq!(
        unsafe { nova_engine_set_out_of_memory_fatal(engine, true) },
        NOVA_OK
    );
    let rows = "var rows = [];
        for (var i = 0; i < 400; i++) rows.push({ id: i, name: 'Item ' + i, price: (i * 1.25).toFixed(2) });
        function run() {
            var html = '<table>';
            for (var i = 0; i < rows.length; i++) {
                var r = rows[i];
                html += '<tr class=\"' + (i & 1 ? 'odd' : 'even') + '\"><td>' + r.id + '</td><td>'
                    + r.name + '</td><td>' + r.price + '</td></tr>';
            }
            html += '</table>';
            return html.length;
        }";
    let escape = "var s = '';
        for (var i = 0; i < 300; i++) s += 'Tom & Jerry <cat> said \"meow\" to a mouse ';
        function run() {
            var out = '';
            for (var i = 0; i < s.length; i++) {
                var c = s.charAt(i);
                out += c === '&' ? '&amp;' : c === '<' ? '&lt;' : c === '>' ? '&gt;' : c === '\"' ? '&quot;' : c;
            }
            return out.length;
        }";
    for setup in [rows, escape] {
        assert_eq!(eval(engine, setup), NOVA_OK);
        for run in 0..40 {
            assert_eq!(eval(engine, "run()"), NOVA_OK, "run {run}");
            let used = unsafe { nova_engine_memory_used(engine) };
            assert!(used < LIMIT, "run {run}: {used} bytes");
        }
    }
    unsafe { nova_engine_free(engine) };
}
