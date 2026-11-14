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
