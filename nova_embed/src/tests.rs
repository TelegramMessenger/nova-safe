// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::{cell::RefCell, rc::Rc};

use super::*;

#[derive(Default)]
struct Recorder {
    calls: RefCell<Vec<(u32, Vec<HostArg>)>>,
    reply: RefCell<Option<HostReturn>>,
    panic_on: Option<u32>,
}

struct SharedRecorder(Rc<Recorder>);

impl Host for SharedRecorder {
    fn call(&self, id: u32, args: Vec<HostArg>) -> HostReturn {
        if self.0.panic_on == Some(id) {
            panic!("host asked to panic");
        }
        self.0.calls.borrow_mut().push((id, args));
        self.0
            .reply
            .borrow()
            .clone()
            .unwrap_or(HostReturn::Undefined)
    }
}

fn engine_with(recorder: Rc<Recorder>) -> Engine {
    Engine::new(
        EngineOptions {
            memory_limit: 0,
            stack_limit: 0,
        },
        Box::new(SharedRecorder(recorder)),
    )
    .expect("engine")
}

fn engine() -> (Engine, Rc<Recorder>) {
    let recorder = Rc::new(Recorder::default());
    (engine_with(recorder.clone()), recorder)
}

#[test]
fn evaluates_sloppy_scripts() {
    let (mut engine, _) = engine();
    assert_eq!(
        engine.evaluate("x = 20; var y = 22; x + y", false),
        Ok(Completion::Number(42.0))
    );
    assert_eq!(
        engine.evaluate(
            "(function () { return this === globalThis ? 'b' + 'c' : 'strict'; })()",
            false
        ),
        Ok(Completion::String("bc".into()))
    );
    assert_eq!(engine.evaluate("({})", false), Ok(Completion::Undefined));
    assert_eq!(
        engine.evaluate("('a\\ud83d' + '\\u00e9')", false),
        Ok(Completion::String(b"a\xed\xa0\xbd\xc3\xa9".to_vec()))
    );
}

#[test]
fn reports_exceptions_as_text() {
    let (mut engine, _) = engine();
    assert_eq!(
        engine.evaluate("throw new TypeError('nope')", false),
        Err(Error::Exception("TypeError: nope".into()))
    );
    assert_eq!(
        engine.evaluate("throw 'plain'", false),
        Err(Error::Exception("plain".into()))
    );
    match engine.evaluate("let = = 1", false) {
        Err(Error::Exception(text)) => assert!(text.starts_with("SyntaxError: "), "{text}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(engine.evaluate("1 + 1", false), Ok(Completion::Number(2.0)));
}

#[test]
fn evaluates_modules_without_imports() {
    let (mut engine, _) = engine();
    assert_eq!(
        engine.evaluate("globalThis.m = 7; export const a = 1;", true),
        Ok(Completion::Undefined)
    );
    assert_eq!(engine.evaluate("m", false), Ok(Completion::Number(7.0)));
    match engine.evaluate("import x from 'y'; globalThis.z = x;", true) {
        Err(Error::Exception(text)) => assert!(text.contains("TypeError"), "{text}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn host_functions_receive_their_id_and_owned_arguments() {
    let (mut engine, recorder) = engine();
    engine
        .register_function("__a", 70, 2, ArgumentMode::Values)
        .unwrap();
    engine
        .register_function("__b", 71, 3, ArgumentMode::ValuesAndBytes)
        .unwrap();
    *recorder.reply.borrow_mut() = Some(HostReturn::String("ok".into()));
    assert_eq!(
        engine.evaluate("__a('s', 1.5, {}, new Int32Array(2))", false),
        Ok(Completion::String("ok".into()))
    );
    engine
        .evaluate(
            "const b = new ArrayBuffer(24); const v = new Float64Array(b, 8, 2); v[0] = 1; v[1] = 2; __b(3, v, 'x'); __b(new Int32Array([1, -1]))",
            false,
        )
        .unwrap();
    let calls = recorder.calls.borrow();
    assert_eq!(calls[0].0, 70);
    assert_eq!(
        calls[0].1,
        vec![
            HostArg::String("s".into()),
            HostArg::Number(1.5),
            HostArg::Other,
            HostArg::Other
        ]
    );
    assert_eq!(calls[1].0, 71);
    let mut expected = 1f64.to_le_bytes().to_vec();
    expected.extend_from_slice(&2f64.to_le_bytes());
    assert_eq!(
        calls[1].1,
        vec![
            HostArg::Number(3.0),
            HostArg::Bytes {
                kind: TypedArrayKind::Float64,
                bytes: expected
            },
            HostArg::String("x".into())
        ]
    );
    let mut ints = 1i32.to_le_bytes().to_vec();
    ints.extend_from_slice(&(-1i32).to_le_bytes());
    assert_eq!(
        calls[2].1,
        vec![HostArg::Bytes {
            kind: TypedArrayKind::Int32,
            bytes: ints
        }]
    );
}

#[test]
fn host_functions_return_values() {
    let (mut engine, recorder) = engine();
    engine
        .register_function("__h", 1, 0, ArgumentMode::Values)
        .unwrap();
    *recorder.reply.borrow_mut() = Some(HostReturn::ArrayBuffer(vec![1, 2, 3]));
    assert_eq!(
        engine.evaluate(
            "const r = __h(); r instanceof ArrayBuffer ? new Uint8Array(r).join(',') : typeof r",
            false
        ),
        Ok(Completion::String("1,2,3".into()))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::Null);
    assert_eq!(
        engine.evaluate("String(__h())", false),
        Ok(Completion::String("null".into()))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::Number(2.5));
    assert_eq!(
        engine.evaluate("__h() * 2", false),
        Ok(Completion::Number(5.0))
    );
}

#[test]
fn a_script_cannot_reach_or_forge_the_id() {
    let (mut engine, recorder) = engine();
    engine
        .register_function("__a", 5, 0, ArgumentMode::Values)
        .unwrap();
    engine
        .register_function("__b", 6, 0, ArgumentMode::Values)
        .unwrap();
    engine
        .evaluate(
            "const a = __a; Object.getOwnPropertyNames(a).join(); a.call({}); Reflect.apply(__b, null, []); a.bind(null)(); new Function('return 1')(); __b.id = 5; __b();",
            false,
        )
        .unwrap();
    let ids: Vec<u32> = recorder.calls.borrow().iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![5, 6, 5, 6]);
    assert_eq!(
        engine.evaluate("Object.getOwnPropertyNames(__a).sort().join()", false),
        Ok(Completion::String("length,name".into()))
    );
}

#[test]
fn captured_functions_survive_deleting_the_global() {
    let (mut engine, _) = engine();
    engine
        .evaluate(
            "var k = 'a'; function __d(x, y) { return k + x + y; }",
            false,
        )
        .unwrap();
    assert_eq!(engine.capture_global_function("__d"), Ok(true));
    assert_eq!(engine.capture_global_function("k"), Ok(false));
    engine
        .evaluate("delete globalThis.__d; globalThis.__d = 1;", false)
        .unwrap();
    engine.collect_garbage().unwrap();
    assert_eq!(
        engine.call("__d", &["b", "c"]),
        Ok(Completion::String("abc".into()))
    );
    assert_eq!(
        engine.call("nothing", &[]),
        Err(Error::NotAFunction("nothing".into()))
    );
    engine
        .evaluate("function thrower() { throw new RangeError('r'); }", false)
        .unwrap();
    assert_eq!(
        engine.call("thrower", &[]),
        Err(Error::Exception("RangeError: r".into()))
    );
}

#[test]
fn promise_jobs_run_one_at_a_time() {
    let (mut engine, _) = engine();
    engine
        .evaluate(
            "globalThis.log = ''; Promise.resolve().then(() => { log += 'a'; }); Promise.reject(new Error('j')); Promise.resolve().then(() => { throw new Error('t'); }).catch(() => { log += 'c'; });",
            false,
        )
        .unwrap();
    assert!(engine.has_pending_job());
    let mut errors = vec![];
    let mut runs = 0;
    loop {
        match engine.run_pending_job() {
            Ok(true) => runs += 1,
            Ok(false) => break,
            Err(error) => errors.push(error),
        }
    }
    assert!(runs >= 2);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        engine.evaluate("log", false),
        Ok(Completion::String("ac".into()))
    );
    assert!(!engine.has_pending_job());
}

#[test]
fn jobs_without_a_realm_run() {
    let (mut engine, _) = engine();
    engine
        .evaluate(
            "globalThis.out = []; Promise.resolve(1).then(undefined).then((v) => out.push(v)); Promise.reject(2).then(null, undefined).catch((e) => out.push(e)); Promise.resolve(3).finally(5).then((v) => out.push(v));",
            false,
        )
        .unwrap();
    while engine
        .run_pending_job()
        .expect("no job fails or kills the engine")
    {}
    assert_eq!(
        engine.evaluate("out.join()", false),
        Ok(Completion::String("1,2,3".into()))
    );
}

#[test]
fn a_panic_kills_the_engine_for_good() {
    let recorder = Rc::new(Recorder {
        panic_on: Some(9),
        ..Default::default()
    });
    let mut engine = engine_with(recorder);
    engine
        .register_function("__boom", 9, 0, ArgumentMode::Values)
        .unwrap();
    engine.evaluate("var alive = 1", false).unwrap();
    match engine.evaluate("try { __boom() } catch (e) {}; 1", false) {
        Err(Error::Dead(reason)) => assert!(reason.contains("host asked to panic"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(engine.dead_reason().is_some());
    assert!(matches!(
        engine.evaluate("alive", false),
        Err(Error::Dead(_))
    ));
    assert!(matches!(engine.call("x", &[]), Err(Error::Dead(_))));
    assert!(matches!(
        engine.capture_global_function("x"),
        Err(Error::Dead(_))
    ));
    assert!(matches!(engine.run_pending_job(), Err(Error::Dead(_))));
    assert!(matches!(engine.collect_garbage(), Err(Error::Dead(_))));
    assert!(!engine.has_pending_job());
    assert_eq!(engine.memory_used(), 0);
    let (mut other, _) = self::engine();
    assert_eq!(other.evaluate("2", false), Ok(Completion::Number(2.0)));
}

#[test]
fn engines_nest_and_stay_apart() {
    struct Nested {
        inner: RefCell<Engine>,
    }
    impl Host for Nested {
        fn call(&self, _id: u32, args: Vec<HostArg>) -> HostReturn {
            let HostArg::String(code) = &args[0] else {
                return HostReturn::Undefined;
            };
            let code = core::str::from_utf8(code).expect("UTF-8");
            match self.inner.borrow_mut().evaluate(code, false) {
                Ok(Completion::Number(n)) => HostReturn::Number(n),
                Ok(Completion::String(s)) => {
                    HostReturn::String(String::from_utf8_lossy(&s).into_owned())
                }
                _ => HostReturn::Undefined,
            }
        }
    }
    let (inner, inner_recorder) = engine();
    let mut inner = inner;
    inner
        .register_function("__inner", 3, 0, ArgumentMode::Values)
        .unwrap();
    inner.evaluate("var secret = 41", false).unwrap();
    let mut outer = Engine::new(
        EngineOptions {
            memory_limit: 0,
            stack_limit: 0,
        },
        Box::new(Nested {
            inner: RefCell::new(inner),
        }),
    )
    .unwrap();
    outer
        .register_function("__outer", 1, 1, ArgumentMode::Values)
        .unwrap();
    assert_eq!(
        outer.evaluate("Promise.resolve().then(() => 0); __outer('__inner(); Promise.resolve().then(()=>0); secret + 1') + (typeof secret)", false),
        Ok(Completion::String("42undefined".into()))
    );
    assert_eq!(inner_recorder.calls.borrow().len(), 1);
    assert!(outer.has_pending_job());
}

#[test]
fn registration_limits() {
    let (mut engine, _) = engine();
    for slot in 0..MAX_HOST_FUNCTIONS {
        engine
            .register_function(&format!("__f{slot}"), slot as u32, 0, ArgumentMode::Values)
            .unwrap();
    }
    assert!(matches!(
        engine.register_function("__extra", 0, 0, ArgumentMode::Values),
        Err(Error::Invalid(_))
    ));
    let (mut engine, _) = self::engine();
    assert!(matches!(
        engine.register_function("", 0, 0, ArgumentMode::Values),
        Err(Error::Invalid(_))
    ));
}

struct Limited {
    stop: Rc<core::cell::Cell<bool>>,
    polls: Rc<core::cell::Cell<u32>>,
}

impl Host for Limited {
    fn call(&self, _id: u32, _args: Vec<HostArg>) -> HostReturn {
        self.stop.set(true);
        HostReturn::Undefined
    }
    fn should_interrupt(&self) -> bool {
        self.polls.set(self.polls.get() + 1);
        self.stop.get()
    }
}

fn limited(
    memory_limit: usize,
    stack_limit: usize,
) -> (
    Engine,
    Rc<core::cell::Cell<bool>>,
    Rc<core::cell::Cell<u32>>,
) {
    let stop = Rc::new(core::cell::Cell::new(false));
    let polls = Rc::new(core::cell::Cell::new(0));
    let engine = Engine::new(
        EngineOptions {
            memory_limit,
            stack_limit,
        },
        Box::new(Limited {
            stop: stop.clone(),
            polls: polls.clone(),
        }),
    )
    .unwrap();
    (engine, stop, polls)
}

#[test]
fn this_build_enforces_every_limit() {
    assert_eq!(ENFORCED_LIMITS, LIMIT_INTERRUPT | LIMIT_HEAP | LIMIT_STACK);
}

#[test]
fn an_endless_loop_is_interrupted_and_cannot_catch_it() {
    let (mut engine, stop, polls) = limited(0, 0);
    engine
        .register_function("__stop", 1, 0, ArgumentMode::Values)
        .unwrap();
    stop.set(true);
    assert_eq!(
        engine.evaluate("for (;;) {}", false),
        Err(Error::Interrupted)
    );
    assert!(polls.get() >= 1);
    assert_eq!(
        engine.evaluate(
            "for (;;) { try { for (;;) {} } catch (e) {} finally { globalThis.ran = 1; } }",
            false
        ),
        Err(Error::Interrupted)
    );
    stop.set(false);
    assert_eq!(engine.evaluate("1 + 1", false), Ok(Completion::Number(2.0)));
    assert_eq!(
        engine.evaluate("__stop(); var n = 0; while (true) n++;", false),
        Err(Error::Interrupted)
    );
    stop.set(false);
    engine
        .evaluate(
            "function spin() { for (;;) {} } Promise.resolve().then(spin);",
            false,
        )
        .unwrap();
    stop.set(true);
    assert_eq!(engine.run_pending_job(), Err(Error::Interrupted));
    stop.set(false);
    engine
        .evaluate("globalThis.f = () => 'still alive'", false)
        .unwrap();
    assert_eq!(
        engine.call("f", &[]),
        Ok(Completion::String("still alive".into()))
    );
}

#[test]
fn the_heap_limit_ends_a_script_that_goes_past_it() {
    let (mut engine, _, _) = limited(16 << 20, 0);
    assert_eq!(
        engine.evaluate(
            "var keep = []; for (var i = 0; i < 100; i++) keep.push(new Float64Array(1 << 20));",
            false
        ),
        Err(Error::OutOfMemory)
    );
    assert!(engine.memory_used() > 0);
    assert_eq!(
        engine.evaluate(
            "keep = null; var objects = []; for (;;) { try { objects.push({ n: objects.length, s: 'x' + objects.length }); } catch (e) {} }",
            false
        ),
        Err(Error::OutOfMemory)
    );
    engine.evaluate("objects = null", false).unwrap();
    engine.collect_garbage().unwrap();
    assert!(engine.memory_used() < 16 << 20);
    assert_eq!(
        engine.evaluate("[1, 2, 3].length", false),
        Ok(Completion::Number(3.0))
    );
}

#[test]
fn memory_used_follows_the_heap() {
    let (mut engine, _, _) = engine_pair();
    assert!(engine.memory_used() > 0);
    // What is in use is known at a collection; between two it is what the
    // last one found and what was allocated since, as far as that is
    // counted.
    engine.collect_garbage().unwrap();
    let before = engine.memory_used();
    engine
        .evaluate(
            "globalThis.big = Array.from({ length: 20000 }, (_, i) => ({ i, s: 'v' + i }));",
            false,
        )
        .unwrap();
    // The callback's calls leave no garbage that would stand in for the
    // objects' property storage, which is counted when it is found live.
    engine.collect_garbage().unwrap();
    let grown = engine.memory_used();
    assert!(grown > before + (3 << 18), "{before} -> {grown}");
    engine.evaluate("globalThis.big = null", false).unwrap();
    engine.collect_garbage().unwrap();
    assert!(engine.memory_used() < before + (grown - before) / 2);
}

fn engine_pair() -> (Engine, (), ()) {
    let (engine, _, _) = limited(0, 0);
    (engine, (), ())
}

/// The reviewer's native-recursion probes, on a thread with an 8 MB stack and
/// the stack limit a host derives from it: every one ends with a catchable
/// error or a termination, never a stack overflow.
#[test]
fn deep_recursion_is_a_range_error_not_a_stack_overflow() {
    const PROBES: &[&str] = &[
        "function f() { return f() + 1; } f()",
        "var f = function () {}; for (var i = 0; i < 1e5; i++) f = f.bind(null); f()",
        "var p = { x: 1 }; for (var i = 0; i < 1e5; i++) p = new Proxy(p, {}); p.x",
        "var a = []; for (var i = 0; i < 1e5; i++) a = [a]; JSON.stringify(a)",
        "eval('['.repeat(1e4) + ']'.repeat(1e4))",
        "eval('['.repeat(1e5) + ']'.repeat(1e5))",
        "var o = {}; for (var i = 0; i < 1e5; i++) o = { o }; String(JSON.parse(JSON.stringify(o)))",
        "function g(n) { return n ? [1].map(() => g(n - 1)) : 0; } g(1e6)",
        "var s = { toString() { return '' + s; } }; '' + s",
    ];
    let results = std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(|| {
            let mut out = vec![];
            for probe in PROBES {
                let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
                out.push((probe, engine.evaluate(probe, false), engine.memory_used()));
                assert_eq!(
                    engine.evaluate("1 + 1", false),
                    Ok(Completion::Number(2.0)),
                    "{probe}"
                );
            }
            out
        })
        .unwrap()
        .join()
        .unwrap();
    for (probe, result, used) in results {
        match result {
            Ok(_)
            | Err(Error::Exception(_))
            | Err(Error::Interrupted)
            | Err(Error::OutOfMemory) => {}
            other => panic!("{probe}: {other:?}"),
        }
        eprintln!("{probe}: {result:?} ({used} bytes)");
    }
}

#[test]
fn a_backtracking_regexp_is_interrupted_and_cannot_catch_it() {
    let (mut engine, stop, polls) = limited(0, 0);
    engine
        .evaluate(
            "var re = /(a+)+$/; var input = 'a'.repeat(28) + 'b';",
            false,
        )
        .unwrap();
    stop.set(true);
    let before = polls.get();
    assert_eq!(
        engine.evaluate("for (;;) { try { re.test(input); } catch (e) {} }", false),
        Err(Error::Interrupted)
    );
    assert_eq!(polls.get(), before + 1);
    assert_eq!(
        engine.evaluate(
            "try { input.replace(re, 'x') } catch (e) { 'caught' }",
            false
        ),
        Err(Error::Interrupted)
    );
    stop.set(false);
    assert_eq!(
        engine.evaluate("/b+/.exec('aabbb')[0]", false),
        Ok(Completion::String("bbb".into()))
    );
}

#[test]
fn local_time_is_the_hosts_time_zone() {
    struct Dubai;
    impl Host for Dubai {
        fn call(&self, _id: u32, _args: Vec<HostArg>) -> HostReturn {
            HostReturn::Undefined
        }
        fn local_time_zone_offset(&self, epoch_seconds: i64) -> i32 {
            if epoch_seconds >= 1_782_000_000 {
                4 * 3600 + 30 * 60 + 59
            } else {
                4 * 3600
            }
        }
    }
    let mut engine = Engine::new(
        EngineOptions {
            memory_limit: 0,
            stack_limit: 0,
        },
        Box::new(Dubai),
    )
    .unwrap();
    assert_eq!(
        engine.evaluate(
            "var d = new Date(Date.UTC(2026, 0, 15, 8, 30)); [d.getHours(), d.getMinutes(), d.getTimezoneOffset(), d.toString(), new Date(2026, 0, 15, 12, 30).getTime() === d.getTime(), new Date(Date.UTC(2026, 6, 15)).getTimezoneOffset()].join('|')",
            false
        ),
        Ok(Completion::String(
            "12|30|-240|Thu Jan 15 2026 12:30:00 GMT+0400|true|-270".into()
        ))
    );
    let (mut utc, _) = engine_with_default();
    assert_eq!(
        utc.evaluate("new Date(Date.UTC(2026, 0, 15, 8, 30)).getHours()", false),
        Ok(Completion::Number(8.0))
    );
}

fn engine_with_default() -> (Engine, ()) {
    let (engine, _) = engine();
    (engine, ())
}

#[test]
fn garbage_does_not_refuse_an_allocation_under_the_heap_limit() {
    let (mut engine, _, _) = limited(8 << 20, 0);
    assert_eq!(
        engine.evaluate(
            "var n = 0; for (var i = 0; i < 400; i++) { var s = 'x'.repeat(100000) + i; n += s.length; } var b = new ArrayBuffer(1 << 20); n + b.byteLength",
            false
        ),
        Ok(Completion::Number(400.0 * 100_000.0 + 1090.0 + 1048576.0))
    );
    assert_eq!(
        engine.evaluate("new ArrayBuffer(16 << 20)", false),
        Err(Error::OutOfMemory)
    );
    assert_eq!(
        engine.evaluate(
            "try { new ArrayBuffer(16 << 20); 'no' } catch (e) { e.name }",
            false
        ),
        Ok(Completion::String("RangeError".into()))
    );
    assert_eq!(
        engine.evaluate(
            "var keep = []; for (var i = 0; i < 100; i++) keep.push(new Float64Array(1 << 17));",
            false
        ),
        Err(Error::OutOfMemory)
    );
}

fn fatal(memory_limit: usize) -> Engine {
    let (mut engine, _, _) = limited(memory_limit, 0);
    engine.set_out_of_memory_fatal(true).unwrap();
    engine
}

/// With out of memory fatal, an allocation the limit has no room for ends
/// the script wherever a script could otherwise catch it and go on.
#[test]
fn a_refused_allocation_ends_the_script_when_out_of_memory_is_fatal() {
    for script in [
        "try { new ArrayBuffer(16 << 20); 'no' } catch (e) { e.name }",
        "try { new ArrayBuffer(16 << 20); } finally { globalThis.ran = 1; }",
        "var a = []; for (;;) { try { a.push(new Array(100000).fill(1.5)); } catch (e) {} }",
        "var a = []; for (;;) { try { a.push(new ArrayBuffer(3 << 20)); } catch (e) {} }",
        "throw { get name() { new ArrayBuffer(16 << 20); return 'Quiet'; }, message: 'm' };",
        "throw { get name() { try { new ArrayBuffer(16 << 20); } catch (e) {} return 'Quiet'; }, message: 'm' };",
        "(async function () { try { new ArrayBuffer(16 << 20); } catch (e) {} globalThis.ran = 1; })();",
        "try { 'x'.repeat(12 << 20); } catch (e) {} globalThis.ran = 1;",
        "try { btoa('a'.repeat(7 << 20)); } catch (e) {} globalThis.ran = 1;",
    ] {
        let mut engine = fatal(8 << 20);
        assert_eq!(
            engine.evaluate(script, false),
            Err(Error::OutOfMemory),
            "{script}"
        );
        assert!(engine.memory_used() < 16 << 20, "{script}");
    }
    // A promise job cannot swallow it either, and the queue goes on after.
    let mut engine = fatal(8 << 20);
    engine
        .evaluate(
            "Promise.resolve().then(function () { try { new ArrayBuffer(16 << 20); } catch (e) {} globalThis.ran = 1; }); Promise.resolve().then(function () { globalThis.next = 1; });",
            false,
        )
        .unwrap();
    assert_eq!(engine.run_pending_job(), Err(Error::OutOfMemory));
    assert_eq!(engine.run_pending_job(), Ok(true));
    assert_eq!(
        engine.evaluate("typeof ran + ',' + next", false),
        Ok(Completion::String("undefined,1".into()))
    );
    // The choice can be undone.
    engine.set_out_of_memory_fatal(false).unwrap();
    assert_eq!(
        engine.evaluate(
            "try { new ArrayBuffer(16 << 20); 'no' } catch (e) { e.name }",
            false
        ),
        Ok(Completion::String("RangeError".into()))
    );
}

/// A script that ends when it is out of memory does not end for garbage:
/// what it dropped makes room for what it allocates next.
#[test]
fn garbage_does_not_end_a_script_when_out_of_memory_is_fatal() {
    let mut engine = fatal(8 << 20);
    assert_eq!(
        engine.evaluate(
            "var n = 0; for (var i = 0; i < 40; i++) { var b = new ArrayBuffer(5 << 20); n += b.byteLength; b = null; } n",
            false
        ),
        Ok(Completion::Number(40.0 * 5242880.0))
    );
    assert_eq!(
        engine.evaluate(
            "var n = 0; for (var i = 0; i < 40; i++) { var s = 'x'.repeat(6 << 20); n += s.length; s = null; var a = new Array(600000).fill(1.5); n += a.length; a = null; } n",
            false
        ),
        Ok(Completion::Number(40.0 * (6291456.0 + 600000.0)))
    );
    assert_eq!(
        engine.evaluate(
            "var keep = new ArrayBuffer(5 << 20); keep.byteLength",
            false
        ),
        Ok(Completion::Number(5242880.0))
    );
    // Held, not dropped: there is no room for another.
    assert_eq!(
        engine.evaluate(
            "try { var more = new ArrayBuffer(5 << 20); } catch (e) {} 'went on'",
            false
        ),
        Err(Error::OutOfMemory)
    );
}

/// A string outside ASCII takes more bytes than code units: the limit counts
/// the bytes before the string is built, not after.
#[test]
fn strings_outside_ascii_are_refused_by_their_bytes() {
    let (mut engine, _, _) = limited(8 << 20, 0);
    for builder in [
        "'\\u0436'.repeat(5 << 20)",
        "'\\u4e2d'.repeat(3 << 20)",
        "''.padEnd(5 << 20, '\\u0436')",
        "'x'.padStart(3 << 20, '\\u4e2d\\u6587')",
    ] {
        assert_eq!(
            engine.evaluate(
                &format!("try {{ {builder}.length }} catch (e) {{ e.name }}"),
                false
            ),
            Ok(Completion::String("RangeError".into())),
            "{builder}"
        );
        assert!(engine.memory_used() < 8 << 20, "{builder}");
    }
    assert_eq!(
        engine.evaluate(
            "'\\u0436'.repeat(1 << 20).length + ''.padEnd(1 << 20, '\\u4e2d').length",
            false
        ),
        Ok(Completion::Number(2097152.0))
    );
}

/// A variable found where it was found the last time is the variable the
/// scope chain gives: every check runs its references more than once.
#[test]
fn remembered_variable_places_keep_the_language_semantics() {
    // An unoptimised build nests few calls within the default stack limit.
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(remembered_variable_places)
        .expect("thread")
        .join()
        .expect("checks");
}

fn remembered_variable_places() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let checks = [
        // Closure variables, read and written, at several distances.
        "(function () { var a = 1; function f() { return a; } var r = f() + f(); a = 5; return r === 2 && f() === 5; })()",
        "(function () { var n = 0; function inc() { n += 1; } for (var i = 0; i < 100; i++) inc(); return n === 100; })()",
        "(function () { var a = 1; return (function () { var b = 2; return (function () { var c = 3; var s = 0; for (var i = 0; i < 10; i++) s += a + b + c; return s; })(); })() === 60; })()",
        // Each closure keeps the variable of its own call or iteration.
        "(function () { function make(v) { return function () { return v; }; } var a = make(1), b = make(2); return a() === 1 && b() === 2 && a() === 1; })()",
        "(function () { var fs = []; for (let i = 0; i < 5; i++) fs.push(function () { return i; }); var s = ''; for (var k = 0; k < 5; k++) s += fs[k]() + '' + fs[k](); return s === '0011223344'; })()",
        "(function () { var fs = []; for (var i = 0; i < 3; i++) { let j = i * 2; fs.push(function () { j += 1; return j; }); } return fs[0]() === 1 && fs[0]() === 2 && fs[2]() === 5 && fs[1]() === 3; })()",
        // The same code at different depths of recursion.
        "(function () { var total = 0; function walk(d) { var here = d; function add() { total += here; } add(); if (d > 0) walk(d - 1); add(); } walk(4); return total === 20; })()",
        // Shadowing: the nearest declaration wins, in each block of its own.
        "(function () { var x = 'outer'; function f(flag) { var r = []; { let x = 'block'; r.push((function () { return x; })()); } r.push((function () { return x; })()); return r.join(); } return f() === 'block,outer' && f() === 'block,outer'; })()",
        "(function () { var x = 1; function g() { return x; } function h() { var x = 2; return g() + (function () { return x; })(); } return h() === 3 && h() === 3; })()",
        "(function () { function f(x) { return function () { try { throw 9; } catch (x) { return (function () { return x; })(); } }; } var g = f(1); return g() === 9 && g() === 9; })()",
        // The temporal dead zone throws every time, then the value is read.
        "(function () { var n = 0; function f() { function get() { return x; } try { get(); } catch (e) { if (e instanceof ReferenceError) n++; } let x = 7; return get(); } return f() === 7 && f() === 7 && n === 2; })()",
        "(function () { var n = 0; function f() { function set() { x = 3; } try { set(); } catch (e) { if (e instanceof ReferenceError) n++; } let x = 7; set(); return x; } return f() === 3 && f() === 3 && n === 2; })()",
        // A constant refuses every write; sloppy and strict.
        "(function () { const c = 1; var n = 0; function w() { c = 2; } for (var i = 0; i < 3; i++) { try { w(); } catch (e) { if (e instanceof TypeError) n++; } } return n === 3 && c === 1; })()",
        "(function () { 'use strict'; const c = 1; var n = 0; function w() { c = 2; } for (var i = 0; i < 3; i++) { try { w(); } catch (e) { if (e instanceof TypeError) n++; } } return n === 3 && c === 1; })()",
        // The name of a function expression is read-only and silently so.
        "(function () { var f = function me(n) { me = null; return n > 0 ? me(n - 1) + 1 : 0; }; return f(3) === 3 && f(3) === 3; })()",
        // Compound assignment, increment, typeof and delete through a closure.
        "(function () { var v = 1; function f() { v *= 2; v++; return typeof v; } return f() === 'number' && f() === 'number' && v === 7; })()",
        "(function () { var v = 1; function f() { return delete v; } return f() === false && f() === false && v === 1; })()",
        // A direct eval can put a variable between a reference and its
        // declaration: such references are resolved every time.
        "(function () { var x = 1; function mid(code) { function g() { return x; } var before = g(); eval(code); return before + ',' + g(); } return mid('') === '1,1' && mid('var x = 2') === '1,2' && mid('') === '1,1'; })()",
        "(function () { var x = 1; function mid(code) { { eval(code); } return (function () { return x; })(); } return mid('') === 1 && mid('var x = 2') === 2 && mid('') === 1; })()",
        "(function () { var x = 1; function mid(code) { var g = eval('(function () { return x; })'); var before = g(); eval(code); return before + ',' + g(); } return mid('') === '1,1' && mid('var x = 2') === '1,2'; })()",
        "(function () { var r = []; function f() { eval('var q = 1; var h = function () { return q; }'); r.push(h()); q = 5; r.push(h()); eval('var q = 8'); r.push(h()); } f(); f(); return r.join() === '1,5,8,1,5,8'; })()",
        // Parameters, their defaults and `arguments`.
        "(function () { function f(a, b = function () { return a; }) { var a2 = a; a = 9; return b() + a2; } return f(1) === 10 && f(2) === 11; })()",
        "(function () { function f(a) { function g() { return arguments.length + a; } return g(1, 2) + g(); } return f(10) === 22 && f(1) === 4; })()",
        // Generators and async functions come back to their own variables.
        "(function () { function* gen(a) { var b = a; var get = function () { return a + b; }; while (true) { b += yield get(); } } var g = gen(1), h = gen(10); g.next(); h.next(); return g.next(1).value === 3 && h.next(1).value === 21 && g.next(1).value === 4; })()",
        // Classes: methods see the class and what encloses it.
        "(function () { var k = 3; class A { static make() { return new A(); } get v() { return k; } } k = 4; return A.make().v === 4 && A.make().v === 4; })()",
        // What is done to a variable without a Reference, and what needs one.
        "(function () { var a = 1, b = 2; function swap() { [a, b] = [b, a]; } swap(); var one = a === 2 && b === 1; swap(); return one && a === 1 && b === 2; })()",
        "(function () { var x = 5; function f() { var before = x++; var after = ++x; x -= 2; x **= 2; return before + ',' + after + ',' + x; } return f() === '5,7,25' && f() === '25,27,625'; })()",
        "(function () { var x; var out = []; function f(list) { for (x of list) out.push(x); for (x in { k: 1 }) out.push(x); } f([1, 2]); f([3]); return out.join() === '1,2,k,3,k' && x === 'k'; })()",
        "(function () { var a = null, b = 0, c = 1; function f() { a ??= 4; b ||= 5; c &&= 6; return a + b + c; } return f() === 15 && f() === 15; })()",
        "(function () { var x = 1; function f() { try { x = (function () { throw 3; })(); } catch (e) { return x + e; } } return f() === 4 && f() === 4 && x === 1; })()",
        "(function () { var x = 0; function f() { x = (x = 1, x + 1); return x; } return f() === 2 && f() === 2; })()",
        "(function () { var g = function () { return this; }; function sloppy() { return g(); } function strict() { 'use strict'; return g(); } return sloppy() === sloppy() && strict() === sloppy() && sloppy() === globalThis; })()",
        "(function () { 'use strict'; var g = function () { return this; }; function f() { return g(); } return f() === undefined && f() === undefined; })()",
        "(function () { var x = 1; function f() { return typeof x + typeof f + typeof missing; } return f() === 'numberfunctionundefined' && f() === 'numberfunctionundefined'; })()",
        "(function () { var o = { v: 1 }; function f(k) { o.v += k; o = { v: o.v * 2 }; return o.v; } return f(1) === 4 && f(1) === 10; })()",
        "(function () { var fs = []; for (var i = 0; i < 3; i++) (function (j) { fs.push(function (d) { j += d; return j; }); })(i * 10); return fs[1](1) === 11 && fs[1](1) === 12 && fs[2](5) === 25 && fs[0](0) === 0; })()",
        // Top-level lexical declarations are not variables of a function.
        "(function () { return typeof topLexical === 'undefined'; })()",
    ];
    for check in checks {
        assert_eq!(
            engine.evaluate(&format!("String({check})"), false),
            Ok(Completion::String("true".into())),
            "{check}"
        );
    }
    // Script-level `let`: later scripts and their functions see one variable.
    assert_eq!(
        engine.evaluate("let shared = 1; function readShared() { return shared; } function writeShared(v) { shared = v; } String(readShared() + readShared())", false),
        Ok(Completion::String("2".into()))
    );
    assert_eq!(
        engine.evaluate(
            "writeShared(5); writeShared(6); String(readShared() + shared)",
            false
        ),
        Ok(Completion::String("12".into()))
    );
}

/// Properties and elements read and written where the cache or the Array has
/// them are the properties and elements the long way reads and writes: every
/// check runs its accesses more than once, before and after what changes them.
#[test]
fn direct_property_and_element_access_keeps_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(direct_property_and_element_access)
        .expect("thread")
        .join()
        .expect("checks");
}

fn direct_property_and_element_access() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let checks = [
        // Own, inherited and absent properties, and what moves them.
        "(function () { function P() {} P.prototype.v = 1; var o = new P(); function get() { return o.v; } var r = [get(), get()]; P.prototype.v = 2; r.push(get()); o.v = 3; r.push(get(), P.prototype.v); delete o.v; r.push(get()); return r.join() === '1,1,2,3,2,2'; })()",
        "(function () { var o = { a: 1 }; function get(x) { return x.missing; } var r = [get(o), get(o)]; o.missing = 5; r.push(get(o)); delete o.missing; r.push(get(o)); Object.prototype.missing = 7; r.push(get(o)); delete Object.prototype.missing; r.push(get(o)); return r.join() === ',,5,,7,'; })()",
        "(function () { function get(x) { return x.k; } var shapes = [{ k: 1 }, { j: 0, k: 2 }, { k: 3, j: 0 }, Object.create({ k: 4 }), { j: 1 }]; var s = ''; for (var n = 0; n < 3; n++) for (var i = 0; i < shapes.length; i++) s += get(shapes[i]); return s === '1234undefined1234undefined1234undefined'; })()",
        // Accessors are called, here and on the prototype.
        "(function () { var calls = 0; var o = { get g() { calls++; return 9; }, set g(v) { calls += v; } }; function get() { return o.g; } function set(v) { o.g = v; } var r = get() + get(); set(10); set(10); return r === 18 && calls === 22; })()",
        "(function () { var seen = []; var proto = { set s(v) { seen.push(v, this === o); } }; var o = Object.create(proto); function set(v) { o.s = v; } set(1); set(2); return seen.join() === '1,true,2,true' && !o.hasOwnProperty('s'); })()",
        "(function () { var o = { v: 1 }; function get() { return o.v; } function set(x) { o.v = x; } set(2); var a = get(); Object.defineProperty(o, 'v', { get: function () { return 'got'; }, set: function (x) { a = x; }, configurable: true }); set(5); return get() === 'got' && a === 5; })()",
        // A property that is not writable keeps its value; strict code throws.
        "(function () { var o = { v: 1 }; function set(x) { o.v = x; } set(2); set(3); Object.defineProperty(o, 'v', { writable: false }); set(4); set(5); return o.v === 3; })()",
        "(function () { 'use strict'; var o = { v: 1 }; function set(x) { o.v = x; } set(2); Object.freeze(o); var n = 0; for (var i = 0; i < 2; i++) { try { set(9); } catch (e) { if (e instanceof TypeError) n++; } } return o.v === 2 && n === 2; })()",
        "(function () { var o = { v: 1 }; function set(x) { o.v = x; } set(2); Object.freeze(o); set(9); set(9); return o.v === 2; })()",
        // An inherited data property is written to the object itself.
        "(function () { var proto = { v: 1 }; function set(x, v) { x.v = v; } var a = Object.create(proto), b = Object.create(proto); set(a, 2); set(b, 3); set(a, 4); return a.v === 4 && b.v === 3 && proto.v === 1 && a.hasOwnProperty('v'); })()",
        // Compound assignment, increment and method calls keep their object.
        "(function () { var o = { n: 1, bump: function (by) { this.n += by; return this; } }; function f() { o.n += 2; o.n++; ++o.n; return o.bump(1).bump(1) === o; } return f() && f() && o.n === 13; })()",
        "(function () { var a = { m: function () { return this; } }, b = { m: a.m }; function call(x) { return x.m(); } return call(a) === a && call(b) === b && call(a) === a && (0, a.m)() !== a; })()",
        "(function () { var o = { f: function () { return 1; } }; function call() { return o.f(); } var r = call() + call(); o.f = function () { return 5; }; return r + call() === 7; })()",
        // Primitives and other objects take the long way to the same result.
        "(function () { function len(x) { return x.length; } var f = function (a, b) {}; var r = []; for (var i = 0; i < 2; i++) r.push(len('abc'), len([1, 2]), len(f), len({ length: 9 }), len(new Uint8Array(4))); return r.join() === '3,2,2,9,4,3,2,2,9,4'; })()",
        "(function () { function len(a) { return a.length; } var a = [1, 2, 3]; var r = [len(a)]; a.push(4); r.push(len(a)); a.length = 1; r.push(len(a)); a[9] = 0; r.push(len(a)); return r.join() === '3,4,1,10'; })()",
        // Elements: present, absent, holes, and what the prototype holds.
        "(function () { function at(a, i) { return a[i]; } var a = [10, , 30]; var r = [at(a, 0), at(a, 1), at(a, 2), at(a, 3), at(a, -1), at(a, 1.5), at(a, '2')]; Array.prototype[1] = 'p'; r.push(at(a, 1)); delete Array.prototype[1]; r.push(at(a, 1)); a[-1] = 'neg'; r.push(at(a, -1)); return r.join() === '10,,30,,,,30,p,,neg'; })()",
        "(function () { function put(a, i, v) { a[i] = v; } var a = [1, 2, 3]; put(a, 0, 'a'); put(a, 0, 'b'); put(a, 5, 'f'); put(a, 3, 'd'); put(a, '1', 'c'); return a.join() === 'b,c,3,d,,f' && a.length === 6; })()",
        "(function () { function put(a, i, v) { a[i] = v; } var a = [1, 2, 3]; put(a, 1, 5); Object.freeze(a); put(a, 1, 9); put(a, 1, 9); return a.join() === '1,5,3'; })()",
        "(function () { 'use strict'; function put(a, i, v) { a[i] = v; } var a = [1, 2]; put(a, 1, 5); Object.defineProperty(a, 1, { writable: false }); var n = 0; for (var i = 0; i < 2; i++) { try { put(a, 1, 9); } catch (e) { if (e instanceof TypeError) n++; } } return a[1] === 5 && n === 2; })()",
        "(function () { var log = []; var a = [1, 2, 3]; Object.defineProperty(a, 1, { get: function () { log.push('get'); return 7; }, set: function (v) { log.push('set' + v); } }); function at(i) { return a[i]; } function put(i, v) { a[i] = v; } var r = at(1) + at(1) + at(0); put(1, 4); put(1, 5); put(0, 6); return r === 15 && log.join() === 'get,get,set4,set5' && a[0] === 6; })()",
        "(function () { var a = [1, 2, 3]; function f(i) { a[i] += 10; a[i]++; ++a[i]; return a[i]; } return f(0) === 13 && f(0) === 25 && f(2) === 15 && a.join() === '25,2,15'; })()",
        "(function () { var grid = [[1, 2], [3, 4]]; var s = 0; for (var n = 0; n < 2; n++) for (var y = 0; y < 2; y++) for (var x = 0; x < 2; x++) { grid[y][x] *= 2; s += grid[y][x]; } return s === 60 && grid.join() === '4,8,12,16'; })()",
        "(function () { function at(x, i) { return x[i]; } var t = new Uint8Array([5, 6]); var o = { 0: 'zero', k: 'key' }; var r = []; for (var n = 0; n < 2; n++) r.push(at(t, 1), at('str', 1), at(o, 0), at(o, 'k'), at([7], 0)); return r.join() === '6,t,zero,key,7,6,t,zero,key,7'; })()",
    ];
    for check in checks {
        assert_eq!(
            engine.evaluate(&format!("String({check})"), false),
            Ok(Completion::String("true".into())),
            "{check}"
        );
    }
}

/// A call binds its parameters, and keeps its lexical declarations, as the
/// long way did.
#[test]
fn calls_bind_their_parameters_and_declarations() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(calls_bind_parameters)
        .expect("thread")
        .join()
        .expect("checks");
}

fn calls_bind_parameters() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let checks = [
        "(function () { function f(a, b, c) { return [a, b, c].join(); } return f() === ',,' && f(1) === '1,,' && f(1, 2, 3, 4) === '1,2,3' && f(undefined, null, 0) === ',,0'; })()",
        "(function () { function f(a, b) { a = a || 5; b = b + 1; return a + b; } return f(0, 1) === 7 && f(2, 2) === 5 && isNaN(f()); })()",
        "(function () { function f(a, a) { return a; } function g(a, b, a) { return a + b; } return f(1, 2) === 2 && f(1) === undefined && g(1, 2, 3) === 5; })()",
        "(function () { function f(a, b) { return function () { return a + b; }; } function g(a, b, c) { var get = function () { return b; }; b = b * 2; return a + get() + c; } return f(1, 2)() === 3 && f(3, 4)() === 7 && g(1, 2, 3) === 8 && g(1, 2, 3) === 8; })()",
        "(function () { function f(a, b) { return arguments.length + ':' + a + ':' + b + ':' + arguments[2]; } return f(1, 2, 3) === '3:1:2:3' && f(1) === '1:1:undefined:undefined'; })()",
        "(function () { function f(a, b = a + 1, ...rest) { return a + b + rest.length; } function g({ x }, [y]) { return x + y; } return f(1) === 3 && f(1, 5, 0, 0) === 8 && g({ x: 1 }, [2]) === 3; })()",
        "(function () { function fact(n, acc) { return n < 2 ? acc : fact(n - 1, acc * n); } var arrow = (a, b) => a - b; return fact(5, 1) === 120 && fact(6, 1) === 720 && arrow(5, 2) === 3 && arrow(1) !== arrow(1); })()",
        "(function () { function C(a, b) { this.s = a + b; } var m = { f: function (a, b) { return this === m && a + b; } }; return new C(1, 2).s === 3 && new C(4, 5).s === 9 && m.f(1, 2) === 3 && m.f.call(m, 2, 3) === 5 && m.f.apply(m, [3, 4]) === 7; })()",
        "(function () { function* g(a, b) { yield a; yield b; } async function h(a, b) { return a + b; } var it = g(1, 2); var seen; h(3, 4).then(function (v) { seen = v; }); return it.next().value === 1 && it.next().value === 2 && seen === undefined; })()",
        // The lexical declarations of a sloppy function, with and without an eval.
        "(function () { function f(k) { let x = k; const y = 2; class Z { static v() { return x + y; } } function inner() { return x * y; } x += 1; return Z.v() + inner(); } return f(1) === 8 && f(2) === 11; })()",
        "(function () { function f() { var r = typeof x; let x = 1; return r; } var n = 0; for (var i = 0; i < 2; i++) { try { f(); } catch (e) { if (e instanceof ReferenceError) n++; } } return n === 2; })()",
        "(function () { function f(code) { let x = 1; var before = x; eval(code); return before + ':' + x + ':' + (typeof y); } return f('') === '1:1:undefined' && f('var y = 2; x = 5') === '1:5:number'; })()",
        "(function () { function f() { let x = 1; try { eval('var x = 2'); } catch (e) { return e instanceof SyntaxError && x === 1; } return false; } return f() && f(); })()",
        "(function () { function f() { var fs = []; for (let i = 0; i < 3; i++) { let j = i; fs.push(function () { return i + j; }); } return fs.map(function (g) { return g(); }).join(); } return f() === '0,2,4' && f() === '0,2,4'; })()",
    ];
    for check in checks {
        assert_eq!(
            engine.evaluate(&format!("String({check})"), false),
            Ok(Completion::String("true".into())),
            "{check}"
        );
    }
}

/// The interpreter's short ways for Numbers give what the long ways gave.
#[test]
fn number_shortcuts_keep_the_language_semantics() {
    let (mut engine, _) = engine();
    let checks = [
        "Object.is(-1 * 0, -0)",
        "Object.is(0 * -5, -0)",
        "Object.is(2 * 0, 0)",
        "Object.is(-0 + -0, -0)",
        "Object.is(-0 + 0, 0)",
        "Object.is(0 - 0, 0)",
        "Object.is(-0 - 0, -0)",
        "Object.is(5 - 5, 0)",
        "9007199254740991 + 1 === 9007199254740992",
        "9007199254740991 + 2 === 9007199254740992",
        "-9007199254740991 - 2 === -9007199254740992",
        "9007199254740991 + 9007199254740991 === 18014398509481982",
        "0.1 + 0.2 === 0.30000000000000004",
        "isNaN(Infinity - Infinity) && isNaN(Infinity + -Infinity) && isNaN(1 + NaN)",
        "Infinity + 1 === Infinity && -Infinity - 1 === -Infinity",
        "1 < 2 && !(2 < 1) && !(1 < 1) && 1 <= 1 && 2 > 1 && 2 >= 2",
        "!(NaN < 1) && !(1 < NaN) && !(NaN <= NaN) && !(NaN >= NaN) && !(NaN > 1)",
        "!(0 < -0) && !(-0 < 0) && 0 <= -0 && -0 >= 0",
        "-Infinity < 1 && 1 < Infinity && !(Infinity < Infinity) && Infinity <= Infinity",
        "1.5 < 2 && 2 > 1.5 && 0.1 + 0.2 > 0.3 && 1e300 * 10 < Infinity",
        "'10' < '9' && !(10 < '9') && null < 1 && !(undefined < 1) && '2' > 1",
        "(function () { var i = 9007199254740990; i++; i++; i++; return i === 9007199254740992; })()",
        "(function () { var i = 0.5; i++; return i === 1.5; })()",
        "(function () { var i = '4'; i++; return i === 5; })()",
        "(function () { var n = 0; for (var i = 0; i < 1000; i++) { if (i % 3 === 0) n += i; else n -= 1; } return n === 166167; })()",
        "(function () { var x = 1; while (x < 1e21) x *= 10; return x === 1e21; })()",
        "(function () { var o = { valueOf: function () { return 3; } }; return o < 4 && o + 1 === 4 && !(o > 3); })()",
        "(function () { return (true ? 1 : 2) === 1 && !!'a' && !'' && !0 && !NaN && !!{} ; })()",
        // Two integers: the operators that stay integers, and where they stop.
        "(function (a, b) { return a + b === 9007199254740992 && a - -b === 9007199254740992 && -a - b === -9007199254740992; })(9007199254740991, 1)",
        "(function (a, b) { return a * b === 1152921504606846976 && a * 3 === 3221225472 && Object.is(a * 0, 0) && Object.is(-a * 0, -0) && Object.is(0 * -1, -0); })(1073741824, 1073741824)",
        "(function (a, b) { return a * b === 9007199254740992 * 4; })(4503599627370496, 8)",
        "(function (a, b, z) { return a % b === 1 && Object.is(-4 % b, -1) && Object.is(-a % a, -0) && Object.is(z % a, 0) && a % -b === 1 && isNaN(a % z) && 5 % 5 === 0; })(7, 3, 0)",
        "(function (a) { return (a | 0) === 5 && (a & 255) === 5 && (a ^ 1) === 4 && (2147483648 | 0) === -2147483648 && (4294967295 | 0) === -1 && (-1 & 4294967295) === -1; })(1099511627781)",
        "(function (one) { return one << 31 === -2147483648 && one << 32 === 1 && one << 33 === 2 && one << -1 === -2147483648 && 5 >> 33 === 2 && -9 >> 1 === -5 && -1 >>> 0 === 4294967295 && -1 >>> 31 === 1 && -1 >>> 32 === 4294967295 && 1099511627781 >> 1 === 2 && 1099511627781 >>> 1 === 2; })(1)",
        "(function (a, b) { return a < b && !(b < a) && a <= a && b >= a && !(a > b) && b > a && -9007199254740991 < 9007199254740991; })(-3, 4)",
        "(function () { var i = 9007199254740990; i++; i++; i++; var j = -9007199254740990; j--; j--; j--; return i === 9007199254740992 && j === -9007199254740992; })()",
        "(function () { var n = 0; for (var i = 10; i > -10; i--) n += i; for (var k = 0; k <= 4; k += 2) n -= k; var m = 0; do { m++; } while (m < 5); return n === 4 && m === 5; })()",
    ];
    for check in checks {
        assert_eq!(
            engine.evaluate(&format!("String({check})"), false),
            Ok(Completion::String("true".into())),
            "{check}"
        );
    }
}

/// A string built by appending is the string it spells, whichever way it
/// was built: its place in the string table continues from the left part's.
#[test]
fn appended_strings_are_the_strings_they_spell() {
    let (mut engine, _) = engine();
    let checks = [
        "(function () { var s = ''; for (var i = 0; i < 3000; i++) s += 'chunk'; return s === 'chunk'.repeat(3000) && s.length === 15000; })()",
        "(function () { var s = ''; for (var i = 0; i < 2000; i++) s += '\\u0436\\u4e2d'; var o = {}; o[s] = 1; return o['\\u0436\\u4e2d'.repeat(2000)] === 1; })()",
        "(function () { var a = 'a'.repeat(300), b = 'b'.repeat(70), c = 'c'.repeat(5); return (a + b) + c === a + (b + c) && (a + b + c) === [a, b, c].join(''); })()",
        "(function () { var m = new Map(); var s = 'x'.repeat(250); for (var i = 0; i < 200; i++) { s += i; m.set(s, i); } var t = 'x'.repeat(250); for (var j = 0; j < 200; j++) { t = t + String(j); if (m.get(t) !== j) return false; } return m.size === 200; })()",
        "(function () { var lead = 'q'.repeat(300) + '\\ud83d', s = lead + '\\ude00'; return s.length === 302 && s === 'q'.repeat(300) + '\\ud83d\\ude00' && s.codePointAt(300) === 0x1f600; })()",
        "(function () { var s = 'k'.repeat(255); var a = s + 'x', b = s + 'x', c = (s + 'x') + 'yz', d = s + 'xyz'; return a === b && c === d && c.length === 258; })()",
        "(function () { var s = 'p'.repeat(1000); var t = s; for (var i = 0; i < 64; i++) t += 'z'; var u = s + 'z'.repeat(64); var set = new Set([t]); return set.has(u) && t.endsWith('z'.repeat(64)); })()",
    ];
    for check in checks {
        assert_eq!(
            engine.evaluate(&format!("String({check})"), false),
            Ok(Completion::String("true".into())),
            "{check}"
        );
    }
}

/// A string that is being built is not copied until it is read: building is
/// linear, and what reading the kept strings then allocates counts against
/// the memory limit like any other allocation.
#[test]
fn text_joined_on_reading_counts_against_the_memory_limit() {
    let mut engine = fatal(8 << 20);
    let started = std::time::Instant::now();
    assert_eq!(
        engine.evaluate(
            "var s = ''; for (var i = 0; i < 400000; i++) s += 'chunk'; s.length + ':' + s.slice(-7) + ':' + s.charCodeAt(1999999)",
            false
        ),
        Ok(Completion::String("2000000:nkchunk:107".into()))
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    // Strings that are kept count for their whole text each, shared or not:
    // 300 of them fit, are read (each read joins one) and still fit.
    let mut engine = fatal(8 << 20);
    assert_eq!(
        engine.evaluate(
            "var keep = []; var s = 'x'.repeat(300); for (var i = 0; i < 600; i++) { s += 'yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy'; if (i % 2) keep.push(s); } keep.length",
            false
        ),
        Ok(Completion::Number(300.0))
    );
    assert_eq!(
        engine.evaluate(
            "var n = 0; for (var r = 0; r < 4; r++) for (var k = 0; k < keep.length; k++) n += keep[k].charCodeAt(keep[k].length - 1); n",
            false
        ),
        Ok(Completion::Number(4.0 * 300.0 * 121.0))
    );
    assert!(engine.memory_used() < 16 << 20);
    engine.collect_garbage().unwrap();
    assert!(engine.memory_used() < 8 << 20);
    assert_eq!(
        engine.evaluate("keep[299].length + keep[0].charCodeAt(0)", false),
        Ok(Completion::Number(300.0 + 600.0 * 30.0 + 120.0))
    );
    // More of them than the limit has room for cost little to make, sharing
    // their pieces, but cannot all be read: the joins stop at the limit,
    // and with them the engine.
    let mut engine = fatal(8 << 20);
    let made = engine.evaluate(
        "var keep = []; var s = 'x'.repeat(300); for (var i = 0; i < 3000; i++) { s += 'yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy'; keep.push(s); } keep.length",
        false,
    );
    assert!(
        made == Ok(Completion::Number(3000.0)) || made == Err(Error::OutOfMemory),
        "{made:?}"
    );
    if made.is_ok() {
        assert_eq!(
            engine.evaluate(
                "var n = 0; for (var k = 0; k < keep.length; k++) n += keep[k].charCodeAt(0); n",
                false
            ),
            Err(Error::OutOfMemory)
        );
        assert!(matches!(
            engine.evaluate("1", false),
            Err(Error::Dead(_)) | Err(Error::OutOfMemory)
        ));
    }
    // A text of half the limit is built and read: the pieces and the joined
    // copy do not both stay.
    let mut engine = fatal(8 << 20);
    assert_eq!(
        engine.evaluate(
            "var s = ''; for (var i = 0; i < 100000; i++) s += '0123456789012345678901234567890123456789'; var n = 0; for (var k = 0; k < 20000; k++) n += s.charCodeAt(k * 200); s.length + ':' + n",
            false
        ),
        Ok(Completion::String("4000000:960000".into()))
    );
}

/// What one instruction scoped is released when it ends: a long loop of
/// object destructurings in one call does not fill the heap.
#[test]
fn a_loop_of_object_destructurings_stays_within_the_heap_limit() {
    let mut engine = fatal(8 << 20);
    assert_eq!(
        engine.evaluate(
            "var point = { x: 3, y: 4, meta: { id: 7 } }; var s = 0; for (var i = 0; i < 300000; i++) { var { x, y, meta: { id } } = point; var { x: again, ...rest } = point; s += x + y + id + again + rest.y; } s",
            false
        ),
        Ok(Completion::Number(300000.0 * 21.0))
    );
    assert!(engine.memory_used() < 4 << 20);
}

/// A typed array takes a key for an element only when it is the string of a
/// Number; any other name is an ordinary property, found without converting
/// it.
#[test]
fn typed_array_keys_that_are_not_numbers_are_ordinary_properties() {
    let (mut engine, _) = engine();
    let checks = [
        "(function () { var t = new Uint8Array(4); t[1] = 7; return t.length === 4 && t['1'] === 7 && t[1] === 7 && t.byteLength === 4; })()",
        "(function () { var t = new Uint8Array(4); t['01'] = 5; t.name = 'n'; return t['01'] === 5 && t.name === 'n' && t[1] === 0; })()",
        "(function () { var t = new Uint8Array(4); t['NaN'] = 5; t['Infinity'] = 6; t['-0'] = 7; t['1.5'] = 8; t['-1'] = 9; return t['NaN'] === undefined && t['Infinity'] === undefined && t['-0'] === undefined && t['1.5'] === undefined && t['-1'] === undefined && Object.keys(t).length === 4; })()",
        "(function () { var t = new Float64Array(2); t['Infinite'] = 1; t['Nan'] = 2; t['-x'] = 3; return t['Infinite'] === 1 && t['Nan'] === 2 && t['-x'] === 3; })()",
        "(function () { var t = new Uint8Array(3), n = 0; for (var i = 0; i < t.length; i++) { t[i] = i + 1; n += t[i]; } return n === 6 && ('length' in t) && !('5' in t) && ('2' in t); })()",
    ];
    for check in checks {
        assert_eq!(
            engine.evaluate(&format!("String({check})"), false),
            Ok(Completion::String("true".into())),
            "{check}"
        );
    }
}

/// A string a script handed out goes back in as it was: call arguments and
/// what a host function returns are WTF-8, as what comes out is.
#[test]
fn lone_surrogates_go_back_into_the_engine() {
    let (mut engine, recorder) = engine();
    engine
        .register_function("__h", 1, 0, ArgumentMode::Values)
        .unwrap();
    engine
        .evaluate(
            "function units(s) { var r = []; for (var i = 0; i < s.length; i++) r.push(s.charCodeAt(i).toString(16)); return r.join(' '); } function same(s) { return s; }",
            false,
        )
        .unwrap();
    let lone: &[u8] = b"a\xED\xA0\x80b\xED\xB0\x80";
    assert_eq!(
        engine.call_wtf8("units", &[lone]),
        Ok(Completion::String("61 d800 62 dc00".into()))
    );
    assert_eq!(
        engine.call_wtf8("same", &[lone]),
        Ok(Completion::String(lone.to_vec()))
    );
    // A lead and a trail encoded apart are the one code point they spell.
    assert_eq!(
        engine.call_wtf8("same", &[b"\xED\xA0\xBD\xED\xB2\xA9"]),
        Ok(Completion::String("\u{1F4A9}".into()))
    );
    // Bytes that are not WTF-8 are replaced, as before.
    assert_eq!(
        engine.call_wtf8("units", &[b"a\xFF\xC0\x80"]),
        Ok(Completion::String("61 fffd fffd fffd".into()))
    );
    assert_eq!(
        engine.call("units", &["\u{e9}\u{1F4A9}"]),
        Ok(Completion::String("e9 d83d dca9".into()))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::Wtf8(lone.to_vec()));
    assert_eq!(
        engine.evaluate("units(__h())", false),
        Ok(Completion::String("61 d800 62 dc00".into()))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::Wtf8(b"\xFFok".to_vec()));
    assert_eq!(
        engine.evaluate("units(__h())", false),
        Ok(Completion::String("fffd 6f 6b".into()))
    );
}

/// A host function can throw: a RangeError the script catches like any other.
#[test]
fn a_host_function_can_throw_a_range_error() {
    let (mut engine, recorder) = engine();
    engine
        .register_function("__h", 1, 0, ArgumentMode::Values)
        .unwrap();
    *recorder.reply.borrow_mut() = Some(HostReturn::RangeError("too much \u{e9}".into()));
    assert_eq!(
        engine.evaluate(
            "try { __h(); 'returned' } catch (e) { (e instanceof RangeError) + ':' + e.name + ':' + e.message }",
            false
        ),
        Ok(Completion::String("true:RangeError:too much \u{e9}".into()))
    );
    assert_eq!(
        engine.evaluate("__h()", false),
        Err(Error::Exception("RangeError: too much \u{e9}".into()))
    );
    // What a script does to RangeError does not reach the host's throw.
    assert_eq!(
        engine.evaluate(
            "var seen = 0; globalThis.RangeError = function () { seen++; }; Object.defineProperty(Error.prototype, 'message', { set: function () { seen++; } }); try { __h(); } catch (e) { e.message + ':' + seen }",
            false
        ),
        Ok(Completion::String("too much \u{e9}:0".into()))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::Number(1.0));
    assert_eq!(engine.evaluate("__h()", false), Ok(Completion::Number(1.0)));
}

/// `atob` and `btoa` behave as quickjs-ng's (the expected text is what its
/// `qjs` printed for the same script; only the engine's own messages for
/// ToString(Symbol) and for `new` on a non-constructor differ), so the trusted validator decodes `data:`
/// images on Nova as it does on QuickJS.
#[test]
fn atob_and_btoa_match_quickjs() {
    let (mut engine, _) = engine();
    let script = r#"var cases = [
 ['atob()', () => atob()], ['btoa()', () => btoa()],
 ['atob("a")', () => atob("a")], ['btoa("\\u0100")', () => btoa("Ā")],
 ['atob(" R G 9 v\\n")', () => atob(" R G 9 v\n")], ['atob("Zm9v=")', () => atob("Zm9v=")],
 ['atob("YQ")', () => atob("YQ")], ['atob("YQ=")', () => atob("YQ=")], ['atob("YQ==")', () => atob("YQ==")],
 ['atob("YR==")', () => atob("YR==")], ['atob("====")', () => atob("====")], ['atob("Y===")', () => atob("Y===")],
 ['atob("")', () => atob("")], ['atob("YQ==YQ==")', () => atob("YQ==YQ==")], ['atob("Y Q = =")', () => atob("Y Q = =")],
 ['atob("\\u00a0YQ")', () => atob(" YQ")], ['atob("YQ\\v")', () => atob("YQ\v")], ['atob("-_")', () => atob("-_")],
 ['atob(null)', () => atob(null)], ['atob(undefined)', () => atob(undefined)], ['atob(12)', () => atob(12)],
 ['btoa(null)', () => btoa(null)], ['btoa(undefined)', () => btoa(undefined)], ['btoa("\\xff\\xfe\\x00")', () => btoa("\xff\xfe\x00")],
 ['btoa("")', () => btoa("")], ['btoa("ab")', () => btoa("ab")], ['btoa({toString(){return "x"}})', () => btoa({toString(){return "x"}})],
 ['atob(Symbol())', () => atob(Symbol())],
 ['atob("/+8=")', () => atob("/+8=")], ['atob("AAAA")', () => atob("AAAA")],
 ['new atob("")', () => new atob("")],
 ['props', () => [atob.length, btoa.length, atob.name, btoa.name, JSON.stringify(Object.getOwnPropertyDescriptor(globalThis, "atob"), (k, v) => typeof v === "function" ? "fn" : v)].join(",")],
];
var out = [];
for (var [label, f] of cases) {
  try { var v = f(); out.push(label + " => " + (typeof v === "string" ? JSON.stringify(v) : String(v))); }
  catch (e) { out.push(label + " THROWS " + (e && e.name) + " / " + (e && e.message) + " / instanceofError=" + (e instanceof Error) + (e && e.code !== undefined ? " code=" + e.code : "")); }
}
out.join("\n");
"#;
    let expected = r#"atob() THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
btoa() => "dW5kZWZpbmVk"
atob("a") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
btoa("\u0100") THROWS InvalidCharacterError / String contains an invalid character / instanceofError=true code=5
atob(" R G 9 v\n") => "Doo"
atob("Zm9v=") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob("YQ") => "a"
atob("YQ=") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob("YQ==") => "a"
atob("YR==") => "a"
atob("====") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob("Y===") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob("") => ""
atob("YQ==YQ==") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob("Y Q = =") => "a"
atob("\u00a0YQ") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob("YQ\v") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob("-_") THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob(null) => "ée"
atob(undefined) THROWS InvalidCharacterError / The string to be decoded is not correctly encoded / instanceofError=true code=5
atob(12) => "×"
btoa(null) => "bnVsbA=="
btoa(undefined) => "dW5kZWZpbmVk"
btoa("\xff\xfe\x00") => "//4A"
btoa("") => ""
btoa("ab") => "YWI="
btoa({toString(){return "x"}}) => "eA=="
atob(Symbol()) THROWS TypeError / Cannot turn Symbol into string / instanceofError=true
atob("/+8=") => "ÿï"
atob("AAAA") => "\u0000\u0000\u0000"
new atob("") THROWS TypeError / 'function atob() { [ native code ] }' is not a constructor. / instanceofError=true
props => "1,1,atob,btoa,{\"value\":\"fn\",\"writable\":true,\"enumerable\":false,\"configurable\":true}""#;
    let Ok(Completion::String(bytes)) = engine.evaluate(script, false) else {
        panic!("the script did not complete with a string");
    };
    let got = std::string::String::from_utf8(bytes).expect("utf-8");
    for (got, expected) in got.lines().zip(expected.lines()) {
        assert_eq!(got, expected);
    }
    assert_eq!(got.lines().count(), expected.lines().count());
}

#[test]
fn atob_and_btoa_can_be_replaced_and_deleted() {
    let (mut engine, _) = engine();
    assert_eq!(
        engine.evaluate(
            "const own = Object.keys(globalThis).includes('atob'); globalThis.btoa = (s) => 'mine:' + s; delete globalThis.atob; [own, typeof atob, btoa('x')].join()",
            false
        ),
        Ok(Completion::String("false,undefined,mine:x".into()))
    );
}

#[test]
fn atob_decodes_a_large_body_in_linear_time() {
    let (mut engine, _) = engine();
    let started = std::time::Instant::now();
    assert_eq!(
        engine.evaluate(
            "const big = btoa('\\xff\\x00\\x7f'.repeat(1 << 20)); const back = atob(big); [big.length, back.length, back.charCodeAt(0), back.charCodeAt(back.length - 1)].join()",
            false
        ),
        Ok(Completion::String("4194304,3145728,255,127".into()))
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

/// A btoa result past the heap limit is refused as the engine's own string
/// builders refuse theirs (`padEnd` here), and leaves the engine as they do.
#[test]
fn btoa_past_the_heap_limit_ends_the_script_as_pad_end_does() {
    let outcomes = |builder: &str| {
        let mut engine = Engine::new(
            EngineOptions {
                memory_limit: 16 << 20,
                stack_limit: 0,
            },
            Box::new(SharedRecorder(Rc::new(Recorder::default()))),
        )
        .expect("engine");
        let script = format!(
            "const s = 'a'.repeat(9 << 20); let r; try {{ r = {builder}.length; }} catch (e) {{ r = e.name; }} r"
        );
        [
            engine.evaluate(&script, false),
            engine.evaluate("1 + 1", false),
        ]
    };
    let btoa = outcomes("btoa(s)");
    assert!(
        matches!(&btoa[0], Err(Error::OutOfMemory))
            || btoa[0] == Ok(Completion::String("RangeError".into())),
        "{btoa:?}"
    );
    assert_eq!(btoa, outcomes("s.padEnd(12 << 20)"));
}

const COMPILED_TRICKY: &str = r#"
var out = [];
function f(a, b = 1, ...c) { return a + b; }
out.push(f.toString(), f.name, f.length);
const g = function named() {}; out.push(g.name, g.toString(), (() => {}).name);
class A { #p = 1; static s = 'st'; x = 5; get p() { return this.#p; } #m() { return 2 } m() { return this.#m(); } }
class B extends A { constructor() { super(); this.y = 3; } z = 7; }
const b = new B(); out.push(b.p, b.m(), b.x, b.y, b.z, A.s, String(B));
function tag(s) { return s; }
function site() { return tag`a${1}b\x`; }
out.push(site() === site(), site().raw.join('|'), Object.isFrozen(site()));
function re() { return /a(?<n>b+)c/giu; } out.push(re() === re(), re().flags, re().test('xABBC'));
out.push(12345678901234567890n, 1e300, "a string longer than seven bytes", ({ __proto__: null, k: 1 }).k);
var declared = 1; let lexical = 2;
globalThis.later = () => lexical + declared;
out.map(String).join('\n')
"#;

const GLOBALS_FINGERPRINT: &str = "Object.getOwnPropertyNames(globalThis).sort().map((k) => { \
    const v = globalThis[k]; \
    return k + ':' + typeof v + (typeof v === 'function' ? ':' + v.name + '/' + v.length + '/' + String(v).length : ''); \
}).join('\\n')";

fn compare_compiled(source: &str, after: &[&str]) {
    let compiled = CompiledScript::compile(source).expect("compiles");
    let (mut parsed, _) = engine();
    let (mut instantiated, _) = engine();
    assert_eq!(
        instantiated.evaluate_compiled(&compiled),
        parsed.evaluate(source, false)
    );
    for script in after {
        assert_eq!(
            instantiated.evaluate(script, false),
            parsed.evaluate(script, false),
            "{script}"
        );
    }
}

#[test]
fn compiled_scripts_evaluate_as_their_source() {
    compare_compiled(
        COMPILED_TRICKY,
        &["later()", "lexical", GLOBALS_FINGERPRINT],
    );
    compare_compiled("'use strict'; var x = 1; x + 1", &["x"]);
    compare_compiled("throw new RangeError('at the top')", &["1"]);
    compare_compiled("", &[]);
}

#[test]
fn a_compiled_script_runs_in_many_engines_and_threads() {
    let compiled = CompiledScript::compile("var n = (globalThis.n || 0) + 1; `n=${n}`").unwrap();
    let (mut first, _) = engine();
    for expected in ["n=1", "n=2"] {
        assert_eq!(
            first.evaluate_compiled(&compiled),
            Ok(Completion::String(expected.into()))
        );
    }
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                let (mut other, _) = engine();
                assert_eq!(
                    other.evaluate_compiled(&compiled),
                    Ok(Completion::String("n=1".into()))
                );
            });
        }
    });
}

#[test]
fn compile_reports_syntax_errors_as_evaluate_does() {
    let (mut engine, _) = engine();
    let compiled = CompiledScript::compile("let = = 1");
    assert!(compiled.is_err());
    assert_eq!(compiled.err(), engine.evaluate("let = = 1", false).err());
}

/// The widget sandbox's bundles, when this checkout sits next to the
/// Telegram one (or NOVA_WIDGET_JS names their folder).
fn widget_bundle(name: &str) -> Option<std::string::String> {
    let folder = std::env::var("NOVA_WIDGET_JS").unwrap_or_else(|_| {
        "/Users/renoir/projects/Telegram/telegrammacos/third-party/widget-sandbox/packages/ios/Sources/WidgetSandbox/WidgetJS".to_owned()
    });
    std::fs::read_to_string(std::path::Path::new(&folder).join(name)).ok()
}

#[test]
fn compiled_widget_bundles_evaluate_as_their_source() {
    for name in ["sandbox-bundle.js", "validator-bundle.js"] {
        let Some(source) = widget_bundle(name) else {
            eprintln!("skipped: {name} not found");
            continue;
        };
        compare_compiled(
            &source,
            &[
                GLOBALS_FINGERPRINT,
                "typeof __twDispatch === 'function' ? String(__twDispatch('nothing', '{}')) : 'none'",
            ],
        );
    }
}

/// The parser's syntax tree is freed node by node (no arena), so dropping
/// it recurses as deep as the tree. The nesting pre-scan bounds that depth:
/// the deepest tree of each shape the pre-scan lets through is parsed,
/// compiled, run and dropped on a thread with the widget engine's stack
/// (8 MB, `JSThread.stackSize`); an unoptimized build's frames are several
/// times larger, so it gets 64 MB.
#[test]
fn the_deepest_tree_the_pre_scan_allows_parses_and_drops() {
    type Shape = (&'static str, fn(usize) -> String);
    const SHAPES: &[Shape] = &[
        ("binary chain", |n| format!("1{}", "+1".repeat(n))),
        ("member chain", |n| format!("var a = {{}}; a{}", ".b".repeat(n))),
        ("call chain", |n| format!("var f = () => f; f{}", "()".repeat(n))),
        ("index chain", |n| format!("var a = [[]]; a{}", "[0]".repeat(n))),
        ("tag chain", |n| format!("var t = () => t; t{}", "``".repeat(n))),
        ("parentheses", |n| format!("{}1{}", "(".repeat(n), ")".repeat(n))),
        ("unary", |n| format!("{}1", "!".repeat(n))),
        ("arrays", |n| format!("{}{}", "[".repeat(n), "]".repeat(n))),
        ("objects", |n| format!("var x = {}1{}", "{a:".repeat(n), "}".repeat(n))),
        ("assignments", |n| format!("var x; {}1", "x=".repeat(n))),
        ("conditionals", |n| format!("{}1", "1?1:".repeat(n))),
        ("arrows", |n| format!("{}1", "()=>".repeat(n))),
        ("functions", |n| format!("{}{}", "function f(){".repeat(n), "}".repeat(n))),
        ("blocks", |n| format!("{}{}", "{".repeat(n), "}".repeat(n))),
        ("ifs", |n| format!("{};", "if(1)".repeat(n))),
        ("templates", |n| format!("{}1{}", "`${".repeat(n), "}`".repeat(n))),
        ("new", |n| format!("{}Object", "new ".repeat(n))),
    ];
    fn rejected(result: &Result<Completion, Error>) -> bool {
        matches!(result, Err(Error::Exception(message)) if message.contains("nests too deeply"))
    }
    let stack = if cfg!(debug_assertions) { 64 << 20 } else { 8 << 20 };
    let results = std::thread::Builder::new()
        .stack_size(stack)
        .spawn(|| {
            let mut out = vec![];
            for (name, shape) in SHAPES {
                let (mut engine, _, _) = limited(256 << 20, (6 << 20) / 4 * 3);
                let (mut low, mut high) = (1usize, 4096usize);
                assert!(rejected(&engine.evaluate(&shape(high), false)), "{name}");
                let mut deepest = None;
                while low < high {
                    let n = (low + high) / 2;
                    let result = engine.evaluate(&shape(n), false);
                    if rejected(&result) {
                        high = n;
                    } else {
                        deepest = Some((n, result));
                        low = n + 1;
                    }
                }
                let (n, result) = deepest.expect("a shallow tree passes the pre-scan");
                assert_eq!(
                    engine.evaluate("1 + 1", false),
                    Ok(Completion::Number(2.0)),
                    "{name}"
                );
                out.push((name, n, result));
            }
            out
        })
        .unwrap()
        .join()
        .unwrap();
    for (name, n, result) in results {
        match result {
            Ok(_) | Err(Error::Exception(_)) | Err(Error::OutOfMemory) => {}
            other => panic!("{name} at {n}: {other:?}"),
        }
        eprintln!("{name}: {n} levels, {result:?}");
    }
}

struct Deadline(Rc<core::cell::Cell<Option<std::time::Instant>>>);

impl Host for Deadline {
    fn call(&self, _id: u32, _args: Vec<HostArg>) -> HostReturn {
        HostReturn::Undefined
    }
    fn should_interrupt(&self) -> bool {
        self.0
            .get()
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }
}

/// A class whose private references are all undeclared: the checker looks for
/// a close name among every private field for each of them, which is
/// quadratic and takes seconds before the parse budget polls the interrupt.
fn undeclared_private_names(n: usize) -> std::string::String {
    let mut source = std::string::String::from("class C{");
    for i in 0..n {
        source.push_str(&format!("#p{i};"));
    }
    source.push_str("m(){");
    for i in 0..n {
        source.push_str(&format!("this.#q{i};"));
    }
    source.push_str("}}");
    source
}

#[test]
fn parsing_obeys_the_interrupt() {
    const N: usize = 4000;
    const LIMIT: std::time::Duration = std::time::Duration::from_millis(150);
    let deadline = Rc::new(core::cell::Cell::new(None));
    let mut engine = Engine::new(
        EngineOptions {
            memory_limit: 0,
            stack_limit: 0,
        },
        Box::new(Deadline(deadline.clone())),
    )
    .unwrap();
    let hostile = undeclared_private_names(N);
    let build = format!(
        "var s = 'class C{{'; for (var i = 0; i < {N}; i++) s += '#p' + i + ';'; s += 'm(){{'; \
         for (var i = 0; i < {N}; i++) s += 'this.#q' + i + ';'; globalThis.src = s + '}}}}'; \
         src.length"
    );
    assert_eq!(
        engine.evaluate(&build, false),
        Ok(Completion::Number(hostile.len() as f64))
    );
    let mut run = |source: &str, as_module: bool| {
        let started = std::time::Instant::now();
        deadline.set(Some(started + LIMIT));
        let result = engine.evaluate(source, as_module);
        let elapsed = started.elapsed();
        deadline.set(None);
        assert_eq!(result, Err(Error::Interrupted), "{source:.60}");
        assert!(
            elapsed < LIMIT * 10,
            "{source:.60}: interrupted after {elapsed:?}"
        );
        assert_eq!(engine.evaluate("1 + 1", false), Ok(Completion::Number(2.0)));
    };
    run(&hostile, false);
    run(&hostile, true);
    run("try { eval(src) } catch (e) { 'caught' }", false);
    run("'use strict'; try { eval(src) } catch (e) { 'caught' }", false);
    run("try { (0, eval)(src) } catch (e) { 'caught' }", false);
    run("try { new Function(src) } catch (e) { 'caught' }", false);
    run("try { Function('a', src) } catch (e) { 'caught' }", false);
    assert_eq!(
        engine.evaluate("eval('class D { #x; m() { return this.#x; } }; 7')", false),
        Ok(Completion::Number(7.0))
    );
}

/// A name of the global scope read and written where it was found the last
/// time is the name the global scope gives: through three Scripts, with
/// properties of the global object that move, turn into accessors, stop
/// being writable or are shadowed by a later lexical declaration, and names
/// that are not bound when they are assigned to.
#[test]
fn names_of_the_global_scope_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(names_of_the_global_scope)
        .expect("thread")
        .join()
        .expect("checks");
}

fn names_of_the_global_scope() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let first = r#"var out = [];
function log(label, f) {
  var r;
  try { r = String(f()); } catch (e) { r = e.constructor.name; }
  out.push(label + "=" + r);
}
var gv = 1;
let gl = 2;
const gc = 3;
class GC { static v = 4; }
function gf() { return 5; }
globalThis.conf = 6;           // configurable property
Object.defineProperty(globalThis, "acc", { get() { return accStore * 2; }, set(v) { accStore = v; }, configurable: true });
var accStore = 10;
Object.prototype.inherited = 11;

function reads() { return [gv, gl, gc, GC.v, gf(), conf, acc, inherited, undefined, NaN, typeof missing, typeof gv, typeof gl].join(","); }
log("reads1", reads);
log("reads2", reads);

function writes() { gv = 21; gl = 22; conf = 26; acc = 30; return [gv, gl, conf, acc, accStore].join(","); }
log("writes1", writes);
log("writes2", writes);
log("constWrite", function () { gc = 9; return gc; });
log("constWriteStrict", function () { "use strict"; gc = 9; return gc; });

function compound() { gv += 1; gl *= 2; conf -= 1; acc += 1; gv++; --gl; return [gv, gl, conf, acc].join(","); }
log("compound1", compound);
log("compound2", compound);

function logical() { var a = (gv &&= 100), b = (conf ||= 7), c = (gl ??= 8); return [a, b, c, gv, conf, gl].join(","); }
log("logical", logical);

log("readMissing", function () { return missing1; });
log("typeofMissing", function () { return typeof missing1; });
log("assignMissingSloppy", function () { created1 = 41; return created1 + "," + globalThis.created1; });
log("assignMissingStrict", function () { "use strict"; created2 = 42; return created2; });
log("created2", function () { return typeof created2; });

// Resolved before the value is evaluated.
log("earlyStrict", function () { "use strict"; undeclared3 = (globalThis.undeclared3 = 5); return undeclared3; });
log("earlyStrictAfter", function () { return undeclared3; });
log("earlySloppy", function () { undeclared4 = (globalThis.undeclared4 = 5, 6); return undeclared4; });
globalThis.gone5 = 1;
log("deletedDuringStrict", function () { "use strict"; gone5 = (delete globalThis.gone5, 7); return 0; });
log("gone5", function () { return typeof gone5; });
globalThis.gone6 = 1;
log("deletedDuringSloppy", function () { gone6 = (delete globalThis.gone6, 7); return gone6; });
globalThis.gone7 = 1;
log("compoundDeletedStrict", function () { "use strict"; gone7 += (delete globalThis.gone7, 7); return 0; });
globalThis.gone8 = 1;
log("compoundDeletedSloppy", function () { gone8 += (delete globalThis.gone8, 7); return gone8; });

// A property that moves.
globalThis.m1 = "m1"; globalThis.m2 = "m2"; globalThis.m3 = "m3";
function moving() { return [typeof m1 === "undefined" ? "-" : m1, m2, m3].join(","); }
log("moving1", moving);
delete globalThis.m1;
log("moving2", moving);
globalThis.m1 = "again";
log("moving3", moving);
function setM2(v) { m2 = v; return m2; }
log("setM2a", function () { return setM2("x"); });
delete globalThis.m2; globalThis.m2 = "fresh";
log("setM2b", function () { return setM2("y") + "," + globalThis.m2; });

// A property that becomes an accessor, and back.
globalThis.turn = 1;
function turnRead() { return turn; }
function turnWrite(v) { turn = v; return turn; }
log("turn1", function () { return turnRead() + "," + turnWrite(2); });
var turnStore = 50;
Object.defineProperty(globalThis, "turn", { get() { return turnStore + 1; }, set(v) { turnStore = v; }, configurable: true });
log("turn2", function () { return turnRead() + "," + turnWrite(60) + "," + turnStore; });
Object.defineProperty(globalThis, "turn", { value: 70, writable: true, configurable: true });
log("turn3", function () { return turnRead() + "," + turnWrite(71); });
Object.defineProperty(globalThis, "turn", { value: 80, writable: false, configurable: true });
log("turn4", function () { return turnRead() + "," + turnWrite(81); });
log("turn4strict", function () { "use strict"; turn = 82; return turn; });

// Not writable.
log("undefinedWrite", function () { undefined = 1; return undefined; });
log("undefinedWriteStrict", function () { "use strict"; undefined = 1; return undefined; });

// `this` of a call through a global name.
function thisOf() { return this === globalThis ? "global" : String(this); }
function thisStrict() { "use strict"; return String(this); }
log("thisOf", function () { return thisOf() + "," + thisStrict(); });

// Names the code does not declare.
log("arguments", function () { return (function (a) { arguments[0] = 9; var n = arguments.length; arguments = 5; return [a, n, arguments].join(","); })(1, 2); });
log("destructure", function () { [gv, gl] = [61, 62]; ({ a: conf } = { a: 63 }); return [gv, gl, conf].join(","); });
log("forOf", function () { var s = 0; for (gv of [1, 2, 3]) s += gv; for (conf in { k: 1 }) s += conf; return s + "," + gv + "," + conf; });
log("deleteName", function () { globalThis.del1 = 1; var a = delete del1; return a + "," + (typeof del1); });
log("nested", function () { return (function () { return (() => gv + gl)(); })(); });
log("newFunction", function () { return new Function("return gv + conf.length")(); });
log("evalLocal", function () { var gv = "local"; return eval("gv"); });
log("evalGlobal", function () { return (0, eval)("gv"); });
log("tdz", function () { return later; });
log("tdzTypeof", function () { return typeof later; });
log("tdzWrite", function () { later = 1; return 0; });
let later = "later";
log("afterTdz", function () { later += "!"; return later; });
log("shadowBefore", function () { return shadowed; });
"#;
    let second = r#"// A second Script: a lexical declaration that comes to shadow a property of the global object.
function readShadowed() { return shadowed; }
function writeShadowed(v) { shadowed = v; return shadowed + "," + globalThis.shadowed; }
globalThis.shadowed = "property";
log("shadow1", readShadowed);
log("shadowWrite1", function () { return writeShadowed("p2"); });
"#;
    let third = r#"let shadowed = "lexical";
log("shadow2", readShadowed);
log("shadowWrite2", function () { return writeShadowed("l2"); });
log("crossScriptLet", function () { return gl + "," + later; });
out.join("\n")
"#;
    // An assignment to a name that is not bound throws in strict code even
    // when evaluating the value binds it (earlyStrict), and `arguments` is
    // not tied to the parameters (an open defect of the engine); the rest
    // is what other engines give.
    let expected = r#"reads1=1,2,3,4,5,6,20,11,,NaN,undefined,number,number
reads2=1,2,3,4,5,6,20,11,,NaN,undefined,number,number
writes1=21,22,26,60,30
writes2=21,22,26,60,30
constWrite=TypeError
constWriteStrict=TypeError
compound1=23,43,25,122
compound2=25,85,24,246
logical=100,24,85,100,24,85
readMissing=ReferenceError
typeofMissing=undefined
assignMissingSloppy=41,41
assignMissingStrict=ReferenceError
created2=undefined
earlyStrict=ReferenceError
earlyStrictAfter=5
earlySloppy=6
deletedDuringStrict=ReferenceError
gone5=undefined
deletedDuringSloppy=7
compoundDeletedStrict=ReferenceError
compoundDeletedSloppy=8
moving1=m1,m2,m3
moving2=-,m2,m3
moving3=again,m2,m3
setM2a=x
setM2b=y,y
turn1=1,2
turn2=51,61,60
turn3=70,71
turn4=80,80
turn4strict=TypeError
undefinedWrite=undefined
undefinedWriteStrict=TypeError
thisOf=global,undefined
arguments=1,2,5
destructure=61,62,63
forOf=6kinherited,3,inherited
deleteName=true,undefined
nested=65
newFunction=12
evalLocal=local
evalGlobal=3
tdz=ReferenceError
tdzTypeof=ReferenceError
tdzWrite=ReferenceError
afterTdz=later!
shadowBefore=ReferenceError
shadow1=property
shadowWrite1=p2,p2
shadow2=lexical
shadowWrite2=l2,p2
crossScriptLet=62,later!"#;
    assert!(engine.evaluate(first, false).is_ok());
    assert!(engine.evaluate(second, false).is_ok());
    let Ok(Completion::String(found)) = engine.evaluate(third, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The elements and the length of a TypedArray read and written without a
/// Reference are what \[\[Get]] and \[\[Set]] give: every element type,
/// indexes outside the length, values that need a conversion, buffers that
/// are resized, and a `length` that is not the accessor of
/// %TypedArray%.prototype.
#[test]
fn typed_array_elements_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(typed_array_elements)
        .expect("thread")
        .join()
        .expect("checks");
}

fn typed_array_elements() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 3; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name; }
    out.push(label + round + "=" + String(r));
  }
}
var kinds = [Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array, Uint32Array, Float32Array, Float64Array];
log("readWrite", function () {
  var parts = [];
  for (var k = 0; k < kinds.length; k++) {
    var a = new kinds[k](4);
    a[0] = 300.7; a[1] = -3; a[2] = 4294967295; a[3] = 0.1;
    parts.push(a[0] + "/" + a[1] + "/" + a[2] + "/" + a[3]);
  }
  return parts.join(" ");
});
log("outside", function () {
  var a = new Uint8Array(2);
  a[2] = 9; a[-1] = 9; a[5000000000] = 9; a[1.5] = 9; a["x"] = 9;
  return [a[2], a[-1], a[5000000000], a[1.5], a.x, a.length, Object.keys(a).join()].join("/");
});
log("converted", function () {
  var a = new Int16Array(6);
  a[0] = "12"; a[1] = { valueOf: function () { return 7; } }; a[2] = undefined; a[3] = null; a[4] = true; a[5] = NaN;
  var f = new Float64Array(3);
  f[0] = "1.5"; f[1] = undefined; f[2] = -0;
  return Array.prototype.join.call(a) + " " + f[0] + "," + f[1] + "," + (1 / f[2]);
});
log("bigint", function () {
  var a = new BigInt64Array(2), u = new BigUint64Array(1);
  a[0] = 5n; a[1] = -(2n ** 63n); u[0] = -1n;
  var r = a[0] + "/" + a[1] + "/" + u[0] + "/" + typeof a[0];
  try { a[0] = 1; } catch (e) { r += "/" + e.constructor.name; }
  return r + "/" + a[0];
});
log("bigintToNumberArray", function () { var a = new Uint8Array(1); a[0] = 1n; return a[0]; });
log("symbolValue", function () { var a = new Uint8Array(1); a[0] = Symbol(); return a[0]; });
log("sum", function () {
  var a = new Uint8Array(1000), s = 0;
  for (var i = 0; i < a.length; i++) a[i] = i * 7;
  for (var j = 0; j < a.length; j++) s += a[j];
  return s;
});
log("floats", function () {
  var x = new Float64Array(100), y = new Float32Array(100), s = 0;
  for (var i = 0; i < x.length; i++) { x[i] = i * 0.37; y[i] = x[i] / 3; }
  for (var j = 0; j < x.length; j++) s += x[j] + y[j];
  return s;
});
log("holesAndNaN", function () { var f = new Float32Array(3); f[0] = NaN; f[1] = Infinity; f[2] = -Infinity; return f[0] + "," + f[1] + "," + f[2] + "," + f[3]; });
log("subarray", function () { var a = new Uint16Array([1, 2, 3, 4, 5]); var b = a.subarray(1, 4); b[0] = 20; b[3] = 99; return a.join() + "|" + b.join() + "|" + b.length + "|" + b[3]; });
log("resizable", function () {
  var buffer = new ArrayBuffer(4, { maxByteLength: 16 }), tracking = new Uint8Array(buffer), fixed = new Uint8Array(buffer, 2, 2);
  tracking[3] = 3; fixed[1] = 7;
  var r = [tracking.length, fixed.length, tracking[3], fixed[1]];
  buffer.resize(8); tracking[7] = 8;
  r.push(tracking.length, tracking[7], fixed.length);
  buffer.resize(3); tracking[3] = 1;
  r.push(tracking.length, tracking[3], fixed.length, fixed[1]);
  return r.join("/");
});
log("lengthKinds", function () {
  class Sub extends Uint8Array { get length() { return 42; } }
  class Plain extends Uint8Array {}
  var own = new Uint8Array(3);
  Object.defineProperty(own, "length", { value: "own", configurable: true });
  function len(a) { return a.length; }
  var r = [len(new Uint8Array(5)), len(new Sub(2)), len(new Plain(7)), len(own), len(new Float64Array(9)), len([1, 2]), len("abc"), len({ length: "o" })];
  delete own.length;
  r.push(len(own));
  return r.join("/");
});
log("lengthPatched", function () {
  var proto = Object.getPrototypeOf(Uint8Array.prototype);
  var original = Object.getOwnPropertyDescriptor(proto, "length");
  function len(a) { return a.length; }
  var a = new Uint8Array(4);
  var r = [len(a), len(a)];
  Object.defineProperty(proto, "length", { get: function () { return "patched"; }, configurable: true });
  r.push(len(a));
  Object.defineProperty(proto, "length", { get: original.get, configurable: true });
  r.push(len(a));
  Object.defineProperty(proto, "length", { value: 77, configurable: true });
  r.push(len(a));
  Object.defineProperty(proto, "length", original);
  r.push(len(a), original.get.call(new Int8Array(3)));
  try { original.get.call({}); } catch (e) { r.push(e.constructor.name); }
  return r.join("/");
});
log("stringIndexKeys", function () { var a = new Uint8Array(3); a["1"] = 4; var k = "2"; a[k] = 5; return a[1] + "/" + a["2"] + "/" + a[0]; });
log("otherReceiver", function () { var a = new Uint8Array(2); var o = Object.create(a); o[0] = 5; o[5] = 6; return a[0] + "/" + o[0] + "/" + Object.keys(o).join() + "/" + o[5]; });
log("frozenStyle", function () { "use strict"; var a = new Uint8Array(2); a[9] = 1; a[0] += 5; a[1]++; a[1] *= 300; return a.join(); });
out.join("\n")
"#;
    let expected = r#"readWrite0=44/-3/-1/0 44/253/255/0 255/0/255/0 300/-3/-1/0 300/65533/65535/0 300/-3/-1/0 300/4294967293/4294967295/0 300.70001220703125/-3/4294967296/0.10000000149011612 300.7/-3/4294967295/0.1
readWrite1=44/-3/-1/0 44/253/255/0 255/0/255/0 300/-3/-1/0 300/65533/65535/0 300/-3/-1/0 300/4294967293/4294967295/0 300.70001220703125/-3/4294967296/0.10000000149011612 300.7/-3/4294967295/0.1
readWrite2=44/-3/-1/0 44/253/255/0 255/0/255/0 300/-3/-1/0 300/65533/65535/0 300/-3/-1/0 300/4294967293/4294967295/0 300.70001220703125/-3/4294967296/0.10000000149011612 300.7/-3/4294967295/0.1
outside0=////9/2/0,1,x
outside1=////9/2/0,1,x
outside2=////9/2/0,1,x
converted0=12,7,0,0,1,0 1.5,NaN,-Infinity
converted1=12,7,0,0,1,0 1.5,NaN,-Infinity
converted2=12,7,0,0,1,0 1.5,NaN,-Infinity
bigint0=5/-9223372036854775808/18446744073709551615/bigint/TypeError/5
bigint1=5/-9223372036854775808/18446744073709551615/bigint/TypeError/5
bigint2=5/-9223372036854775808/18446744073709551615/bigint/TypeError/5
bigintToNumberArray0=TypeError
bigintToNumberArray1=TypeError
bigintToNumberArray2=TypeError
symbolValue0=TypeError
symbolValue1=TypeError
symbolValue2=TypeError
sum0=126516
sum1=126516
sum2=126516
floats0=2442.0000000000005
floats1=2442.0000000000005
floats2=2442.0000000000005
holesAndNaN0=NaN,Infinity,-Infinity,undefined
holesAndNaN1=NaN,Infinity,-Infinity,undefined
holesAndNaN2=NaN,Infinity,-Infinity,undefined
subarray0=1,20,3,4,5|20,3,4|3|undefined
subarray1=1,20,3,4,5|20,3,4|3|undefined
subarray2=1,20,3,4,5|20,3,4|3|undefined
resizable0=4/2/7/7/8/8/2/3//0/
resizable1=4/2/7/7/8/8/2/3//0/
resizable2=4/2/7/7/8/8/2/3//0/
lengthKinds0=5/42/7/own/9/2/3/o/3
lengthKinds1=5/42/7/own/9/2/3/o/3
lengthKinds2=5/42/7/own/9/2/3/o/3
lengthPatched0=4/4/patched/4/77/4/3/TypeError
lengthPatched1=4/4/patched/4/77/4/3/TypeError
lengthPatched2=4/4/patched/4/77/4/3/TypeError
stringIndexKeys0=4/5/0
stringIndexKeys1=4/5/0
stringIndexKeys2=4/5/0
otherReceiver0=0/5/0/undefined
otherReceiver1=0/5/0/undefined
otherReceiver2=0/5/0/undefined
frozenStyle0=5,44
frozenStyle1=5,44
frozenStyle2=5,44"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The this value of a call that makes no Function Environment Record is
/// the one OrdinaryCallBindThis binds, for functions that are not strict as
/// for strict ones: the global this value for undefined and null, an object
/// made of a primitive (one for the whole call), and the Function
/// Environment Record where an arrow function, `arguments`, a parameter
/// default or an `eval` needs it.
#[test]
fn calls_bind_the_this_value_of_their_mode() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(calls_bind_this)
        .expect("thread")
        .join()
        .expect("checks");
}

fn calls_bind_this() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name + ":" + e.message; }
    out.push(label + round + "=" + String(r));
  }
}
function describe(v) {
  if (v === globalThis) return "global";
  if (v === undefined) return "undefined";
  if (v === null) return "null";
  var t = typeof v;
  if (t !== "object" && t !== "function") return t + ":" + String(v);
  return Object.prototype.toString.call(v) + (v.valueOf && typeof v.valueOf() !== "object" ? ":" + String(v.valueOf()) : "");
}
function sloppy() { return this; }
function strict() { "use strict"; return this; }
function sloppyNoThis(a) { return a + 1; }
function sloppyLocals(a) { var b = a * 2; let c = b + 1; return function () { return c; }; }
function sloppyTwice() { return this === this && typeof this; }
var thisArgs = [undefined, null, 5, "s", true, Symbol.iterator, 10n, {}, [], sloppy];
log("sloppyCall", function () { return thisArgs.map(function (t) { return describe(sloppy.call(t)); }).join(" | "); });
log("strictCall", function () { return thisArgs.map(function (t) { return describe(strict.call(t)); }).join(" | "); });
log("plain", function () { return describe(sloppy()) + " " + describe(strict()) + " " + sloppyNoThis(1) + " " + sloppyLocals(2)(); });
log("wrapperIdentity", function () { var a = sloppy.call(5), b = sloppy.call(5); return (a === b) + " " + (a == 5) + " " + sloppyTwice.call("x") + " " + (a instanceof Number); });
log("apply", function () { return describe(sloppy.apply(7)) + " " + describe(Reflect.apply(sloppy, "r", [])) + " " + describe(sloppy.bind(false)()) + " " + describe(sloppy.bind()()); });
Number.prototype.me = function () { return this; };
Number.prototype.strictMe = function () { "use strict"; return this; };
String.prototype.kind = function () { return typeof this; };
log("methodsOnPrimitives", function () { return describe((5).me()) + " " + describe((5).strictMe()) + " " + "abc".kind() + " " + describe(1.5.me()); });
Object.defineProperty(String.prototype, "viaGetter", { get: function () { return typeof this; }, configurable: true });
Object.defineProperty(String.prototype, "viaStrictGetter", { get: function () { "use strict"; return typeof this; }, configurable: true });
var seenBySetter;
Object.defineProperty(Number.prototype, "viaSetter", { set: function (v) { seenBySetter = typeof this + v; }, configurable: true });
log("accessors", function () { (3).viaSetter = "!"; return "s".viaGetter + " " + "s".viaStrictGetter + " " + seenBySetter; });
log("callbacks", function () {
  var a = [1].map(function () { return describe(this); })[0];
  var b = [1].map(function () { return describe(this); }, 5)[0];
  var c = [1].map(function () { "use strict"; return describe(this); }, 5)[0];
  var d; [1].forEach(function () { d = describe(this); }, "t");
  return [a, b, c, d].join(" | ");
});
log("arrowInside", function () {
  function outer() { return (() => this)(); }
  function deeper() { return (() => (() => typeof this)())(); }
  return describe(outer.call(5)) + " " + describe(outer()) + " " + deeper.call("s");
});
log("constructing", function () {
  function F(v) { this.v = v; }
  function G() { return typeof this; }
  var f = new F(3);
  return f.v + " " + (f instanceof F) + " " + typeof new G() + " " + describe(F.call(undefined, 4)) + " " + globalThis.v;
});
log("generators", function () {
  function* gen() { yield typeof this; yield this === globalThis; }
  var a = gen.call(5), b = gen();
  return a.next().value + " " + a.next().value + " " + b.next().value + " " + b.next().value;
});
log("asyncAndArguments", function () {
  async function asyncThis() { return typeof this; }
  function withArguments() { return typeof this + arguments.length; }
  function withDefault(a = typeof this) { return a; }
  function withEval() { return eval("typeof this"); }
  var seen; asyncThis.call(5).then(function (v) { seen = v; });
  return withArguments.call(5, 1, 2) + " " + withDefault.call(5) + " " + withEval.call(5) + " " + withArguments() + " " + withDefault();
});
log("recursion", function () { function fact(n) { return n < 2 ? 1 : n * fact(n - 1); } function count(n) { return n ? count.call(n, n - 1) + 1 : 0; } return fact(10) + " " + count(20); });
log("classesAndMethods", function () {
  var o = { m: function () { return this === o; }, n() { return describe(this); } };
  class C { m() { return this instanceof C; } static s() { return this === C; } }
  var m = o.m, n = o.n;
  return o.m() + " " + m() + " " + describe(n()) + " " + new C().m() + " " + C.s() + " " + describe(n.call(1));
});
log("globalThisIsRealm", function () { var f = new Function("return this"); return describe(f()) + " " + describe(f.call(null)) + " " + describe(f.call(0)); });
out.join("\n")
"#;
    let expected = r#"sloppyCall0=global | global | [object Number]:5 | [object String]:s | [object Boolean]:true | [object Symbol]:Symbol(Symbol.iterator) | [object BigInt]:10 | [object Object] | [object Array] | [object Function]:function sloppy() { return this; }
sloppyCall1=global | global | [object Number]:5 | [object String]:s | [object Boolean]:true | [object Symbol]:Symbol(Symbol.iterator) | [object BigInt]:10 | [object Object] | [object Array] | [object Function]:function sloppy() { return this; }
strictCall0=undefined | null | number:5 | string:s | boolean:true | symbol:Symbol(Symbol.iterator) | bigint:10 | [object Object] | [object Array] | [object Function]:function sloppy() { return this; }
strictCall1=undefined | null | number:5 | string:s | boolean:true | symbol:Symbol(Symbol.iterator) | bigint:10 | [object Object] | [object Array] | [object Function]:function sloppy() { return this; }
plain0=global undefined 2 5
plain1=global undefined 2 5
wrapperIdentity0=false true object true
wrapperIdentity1=false true object true
apply0=[object Number]:7 [object String]:r [object Boolean]:false global
apply1=[object Number]:7 [object String]:r [object Boolean]:false global
methodsOnPrimitives0=[object Number]:5 number:5 object [object Number]:1.5
methodsOnPrimitives1=[object Number]:5 number:5 object [object Number]:1.5
accessors0=object string object!
accessors1=object string object!
callbacks0=global | [object Number]:5 | number:5 | [object String]:t
callbacks1=global | [object Number]:5 | number:5 | [object String]:t
arrowInside0=[object Number]:5 global object
arrowInside1=[object Number]:5 global object
constructing0=3 true object undefined 4
constructing1=3 true object undefined 4
generators0=object false object true
generators1=object false object true
asyncAndArguments0=object2 object object object0 object
asyncAndArguments1=object2 object object object0 object
recursion0=3628800 20
recursion1=3628800 20
classesAndMethods0=true false string:global true true string:[object Number]:1
classesAndMethods1=true false string:global true true string:[object Number]:1
globalThisIsRealm0=global global [object Number]:0
globalThisIsRealm1=global global [object Number]:0"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// A function whose parameters are plain names in stack slots starts with
/// its arguments there: fewer and more arguments than parameters, parameters
/// that are assigned to or captured, every other kind of parameter list
/// beside them, every way of calling, and arguments that are objects kept
/// through calls that collect.
#[test]
fn calls_put_their_arguments_in_the_parameters_stack_slots() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(calls_put_arguments)
        .expect("thread")
        .join()
        .expect("checks");
}

fn calls_put_arguments() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name + ":" + e.message; }
    out.push(label + round + "=" + String(r));
  }
}
function two(a, b) { return String(a) + "/" + String(b); }
function none() { return "none"; }
function one(a) { a = a === undefined ? "dflt" : a; return a; }
function locals(a, b, c) { var d = a + b; let e = d * c; { let f = e - a; var g = f + 1; } return [a, b, c, d, e, g].join(); }
log("counts", function () { return [two(), two(1), two(1, 2), two(1, 2, 3), none(1, 2), one(), one(5)].join(" "); });
log("locals", function () { return locals(1, 2, 3) + " " + locals(4, 5, 6); });
log("reassign", function () { function f(a, b) { a = a + 1; b += a; a++; return a + "," + b; } return f(1, 2) + " " + f(10, 20); });
log("captured", function () { function f(a, b) { return function () { return a + b; }; } function g(a, b) { var h = function () { return b; }; b = b * 2; return a + h(); } return f(1, 2)() + " " + g(1, 2) + " " + g(3, 4); });
log("mixedKinds", function () {
  function dflt(a, b = a + 1) { return a + b; }
  function rest(a, ...r) { return a + ":" + r.join(); }
  function dstr({ x }, [y], z) { return x + y + z; }
  function args(a, b) { return arguments.length + ":" + a + ":" + arguments[1]; }
  function dup(a, a) { return a; }
  function evalIt(a, b) { return eval("a + b"); }
  return [dflt(1), dflt(1, 5), rest(1, 2, 3), rest(1), dstr({ x: 1 }, [2], 3), args(1, 2, 3), args(1), dup(1, 2), evalIt(2, 3)].join(" ");
});
log("strictAndArrows", function () { "use strict"; function f(a, b) { return a - b; } var g = (a, b) => a * b, h = a => a + 1, k = () => 7; return [f(5, 2), g(3, 4), h(1), k(9), f(1)].join(" "); });
log("recursion", function () { function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } function acc(n, total) { return n ? acc(n - 1, total + n) : total; } return fib(6) + " " + acc(6, 0); });
log("builtinCallers", function () {
  var seen = [];
  [10, 20].forEach(function (v, i) { seen.push(v + i); });
  var mapped = [1, 2, 3].map(function (v) { return v * 2; });
  var reduced = [1, 2, 3].reduce(function (a, v, i, all) { return a + v * i + all.length; }, 0);
  var sorted = [3, 1, 2].sort(function (a, b) { return a - b; });
  var filtered = [1, 2, 3, 4].filter(function (v, i) { return (v + i) % 3; });
  var replaced = "a-b".replace(/-/, function (m, at, all) { return m + at + all.length; });
  return [seen, mapped, reduced, sorted, filtered, replaced].join(" | ");
});
log("callApplyBind", function () {
  function f(a, b) { return String(this) + ":" + a + ":" + b; }
  var list = [1, 2, 3];
  var r = [f.call("t", 1), f.apply("t", list), f.bind("t", 9)(8), f.bind("t", 9, 8, 7)(6), Reflect.apply(f, "t", [1, 2])];
  return r.join(" ") + " " + list.join();
});
log("spread", function () { function f(a, b, c) { return [a, b, c].join(); } var xs = [1, 2]; return f(...xs) + " " + f(0, ...xs, 9) + " " + f(...[], ...xs) + " " + xs.join(); });
log("constructors", function () {
  function P(x, y) { this.x = x; this.y = y; }
  P.prototype.sum = function (k) { return (this.x + this.y) * k; };
  class C { constructor(a, b) { this.v = a + (b === undefined ? 100 : b); } add(n) { return this.v + n; } static make(a, b) { return new C(a, b); } }
  class D extends C { constructor(a, b) { super(a, b); this.w = a; } add(n, m) { return super.add(n) + m + this.w; } }
  return [new P(1, 2).sum(3), new P(1).y, new C(1, 2).add(3), new C(1).v, C.make(2, 3).v, new D(1, 2).add(3, 4), new D(5).v].join(" ");
});
log("accessors", function () { var o = { _v: 1, get v() { return this._v; }, set v(x) { this._v = x * 2; } }; o.v = 5; return o.v; });
log("generators", function () {
  function* gen(a, b) { var got = yield a; yield b + got; return a + b; }
  var g = gen(1, 2), h = gen(10);
  var r = [g.next().value, g.next(5).value, g.next().value, h.next().value, String(h.next(1).value)];
  function* inf(start, step) { for (;;) { yield start; start += step; } }
  var it = inf(1, 2); it.next(); it.next();
  return r.join(" ") + " " + it.next().value;
});
log("asyncFunctions", function () { var seen = []; async function f(a, b) { var c = await a; return c + b; } f(1, 2).then(function (v) { seen.push(v); }); return typeof f(1, 2).then; });
log("collectionDuringCalls", function () {
  // Arguments that are objects, kept through calls that allocate enough for collections.
  function build(tag, depth) { var junk = []; for (var i = 0; i < 300; i++) junk.push({ i: i, s: "x" + i }); return depth ? build(tag, depth - 1) : tag; }
  function keep(a, b, c) { var first = build(a, 3); var s = 0; for (var i = 0; i < 20; i++) s += build({ n: i }, 1).n; return first.name + b.name + c.name + s; }
  var r = "";
  for (var k = 0; k < 30; k++) r = keep({ name: "a" + k }, { name: "b" }, { name: "c" });
  return r;
});
log("manyParameters", function () { function f(a, b, c, d, e, f2, g, h, i, j) { return a + b + c + d + e + f2 + g + h + i + j; } return f(1, 2, 3, 4, 5, 6, 7, 8, 9, 10) + " " + f(1, 2, 3); });
log("valuesOfEveryKind", function () { function id(x) { return x; } var sym = Symbol("s"); return [id(1.5), id(-0) === 0 && 1 / id(-0), id("str"), id(null), id(undefined), id(true), typeof id(sym), id(10n), id([1, 2]).length, id(id)(3)].join(" "); });
log("tagged", function () { function tag(strings, a, b) { return strings.join("_") + a + b; } return tag`x${1}y${2}z`; });
log("throwing", function () { function f(a, b) { if (a > b) throw new RangeError("a>b"); return a; } var r = []; for (var i = 0; i < 4; i++) { try { r.push(f(i, 1)); } catch (e) { r.push(e.message); } } return r.join(); });
out.join("\n")
"#;
    let expected = r#"counts0=undefined/undefined 1/undefined 1/2 1/2 none dflt 5
counts1=undefined/undefined 1/undefined 1/2 1/2 none dflt 5
locals0=1,2,3,3,9,9 4,5,6,9,54,51
locals1=1,2,3,3,9,9 4,5,6,9,54,51
reassign0=3,4 12,31
reassign1=3,4 12,31
captured0=3 5 11
captured1=3 5 11
mixedKinds0=3 6 1:2,3 1: 6 3:1:2 1:1:undefined 2 5
mixedKinds1=3 6 1:2,3 1: 6 3:1:2 1:1:undefined 2 5
strictAndArrows0=3 12 2 7 NaN
strictAndArrows1=3 12 2 7 NaN
recursion0=8 21
recursion1=8 21
builtinCallers0=10,21 | 2,4,6 | 17 | 1,2,3 | 1,3,4 | a-13b
builtinCallers1=10,21 | 2,4,6 | 17 | 1,2,3 | 1,3,4 | a-13b
callApplyBind0=t:1:undefined t:1:2 t:9:8 t:9:8 t:1:2 1,2,3
callApplyBind1=t:1:undefined t:1:2 t:9:8 t:9:8 t:1:2 1,2,3
spread0=1,2, 0,1,2 1,2, 1,2
spread1=1,2, 0,1,2 1,2, 1,2
constructors0=9  6 101 5 11 105
constructors1=9  6 101 5 11 105
accessors0=10
accessors1=10
generators0=1 7 3 10 NaN 5
generators1=1 7 3 10 NaN 5
asyncFunctions0=function
asyncFunctions1=function
collectionDuringCalls0=a29bc190
collectionDuringCalls1=a29bc190
manyParameters0=55 NaN
manyParameters1=55 NaN
valuesOfEveryKind0=1.5 -Infinity str   true symbol 10 2 3
valuesOfEveryKind1=1.5 -Infinity str   true symbol 10 2 3
tagged0=x_y_z12
tagged1=x_y_z12
throwing0=0,1,a>b,a>b
throwing1=0,1,a>b,a>b"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// `i++` of a local as one instruction and a comparison with the jump on it
/// are what the instructions they stand for are: for integers, and for
/// everything that takes their general way (doubles, strings, BigInts,
/// objects with a `valueOf`, symbols, the largest integers, constants and
/// variables not yet initialized).
#[test]
fn fused_loop_instructions_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(fused_loop_instructions)
        .expect("thread")
        .join()
        .expect("checks");
}

fn fused_loop_instructions() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name + ":" + e.message; }
    out.push(label + round + "=" + String(r));
  }
}
log("forInts", function () { var s = 0; for (var i = 0; i < 10; i++) s += i; for (var j = 10; j > 0; j--) s += j; for (var k = 0; k <= 3; k++) s += k; for (var m = 3; m >= 0; m--) s += m; return s; });
log("whileAndDo", function () { var i = 0, n = 0; while (i < 5) { i++; n += 2; } do { n--; i--; } while (i > 0); return i + "," + n; });
log("letLoops", function () { var fs = []; for (let i = 0; i < 3; i++) fs.push(function () { return i; }); return fs.map(function (f) { return f(); }).join(); });
log("floats", function () { var x = 0.5, n = 0; for (var i = 0.5; i < 3; i++) n++; x++; x--; x++; return n + "," + x + "," + i; });
log("strings", function () { var s = "5", t = "abc", n = 0; s++; t++; for (var i = "0"; i < "3"; i++) n++; return s + "," + t + "," + n + "," + typeof i + "," + ("a" < "b") + "," + ("10" < "9") + "," + ("10" < 9); });
log("bigints", function () { var b = 5n; b++; b--; b++; var n = 0; for (var i = 0n; i < 3n; i++) n++; return b + "," + n + "," + typeof i + "," + (2n < 3) + "," + (2n > 1.5); });
log("mixedBigint", function () { var b = 1n; return b < "2"; });
log("objects", function () {
  var calls = [];
  var o = { valueOf: function () { calls.push("v"); return 3; } };
  var p = { valueOf: function () { calls.push("p"); return 1; } };
  var n = 0;
  for (var i = p; i < o; i++) n++;
  var q = { valueOf: function () { calls.push("q"); return 7; } };
  q++;
  return n + "," + i + "," + q + "," + calls.join("");
});
log("orderOfConversion", function () { var seen = []; var a = { valueOf() { seen.push("a"); return 1; } }, b = { valueOf() { seen.push("b"); return 2; } }; var r = [a < b, a > b, a <= b, a >= b]; return r.join() + ":" + seen.join(""); });
log("nanAndUndefined", function () { var u, n = NaN, c = 0; u++; n++; if (u < 1) c++; if (!(u < 1)) c += 10; if (n >= n) c += 100; if (undefined <= undefined) c += 1000; if (null <= 0) c += 10000; return u + "," + n + "," + c; });
log("symbols", function () { var s = Symbol(); try { s++; } catch (e) { return e.constructor.name + (function () { try { return s < 1; } catch (e2) { return ":" + e2.constructor.name; } })(); } });
log("largeIntegers", function () { var a = 9007199254740991, b = -9007199254740991, c = 2147483647; a++; b--; c++; var d = 9007199254740990; d++; d++; return [a, b, c, d, a < a + 1, 9007199254740992 < 9007199254740993].join(); });
log("negativeZero", function () { var z = -1; z++; var w = 0; w--; w++; return (1 / z) + "," + (1 / w) + "," + (0 < -0) + "," + (-0 >= 0); });
log("throwInTest", function () { var n = 0; var bad = { valueOf() { throw new RangeError("no"); } }; try { for (var i = 0; i < bad; i++) n++; } catch (e) { return e.message + n + i; } });
log("closureCounter", function () { var c = 0; function inc() { c++; return c; } inc(); inc(); var d = 10; (function () { d--; d--; })(); return c + "," + d; });
log("ifAndTernary", function () { var a = 3, b = 5, r = []; if (a < b) r.push("lt"); if (a > b) r.push("gt"); else r.push("ngt"); r.push(a <= 3 ? "le" : "nle", b >= 6 ? "ge" : "nge"); if (!(a < b)) r.push("x"); if (a < b && b < 10 || a > 100) r.push("and"); return r.join(); });
log("nested", function () { var t = 0; for (var i = 0; i < 4; i++) { for (var j = i; j < 4; j++) { if (j > 2) continue; if (i >= 3) break; t += i * j; } } return t; });
log("valueUsed", function () { var i = 1; var a = i++; var b = ++i; var c = i--; var d = --i; var e = (i++, i); var arr = [0, 0]; var k = 0; arr[k++] = 5; arr[k++] = 6; return [a, b, c, d, e, i, arr, k].join(); });
log("constUpdate", function () { const c = 1; try { c++; } catch (e) { return e.constructor.name + c; } });
log("tdzUpdate", function () { try { x++; } catch (e) { var m = e.constructor.name; } let x = 1; x++; return m + x; });
log("generatorLoop", function () { function* g(n) { for (var i = 0; i < n; i++) yield i; } var s = 0; for (var v of g(5)) s += v; var it = g(2); it.next(); it.next(); return s + "," + it.next().done; });
log("argumentsLoop", function () { function sum() { var s = 0; for (var i = 0; i < arguments.length; i++) s += arguments[i]; return s; } return sum(1, 2, 3, 4); });
log("switchInLoop", function () { var s = ""; for (var i = 0; i < 5; i++) { switch (i) { case 1: s += "a"; break; case 3: s += "c"; continue; default: s += i; } s += "."; } return s; });
log("labels", function () { var n = 0; outer: for (var i = 0; i < 5; i++) { for (var j = 0; j < 5; j++) { if (j > i) continue outer; if (i > 3) break outer; n++; } } return n; });
out.join("\n")
"#;
    let expected = r#"forInts0=112
forInts1=112
whileAndDo0=0,5
whileAndDo1=0,5
letLoops0=0,1,2
letLoops1=0,1,2
floats0=3,1.5,3.5
floats1=3,1.5,3.5
strings0=6,NaN,3,number,true,true,false
strings1=6,NaN,3,number,true,true,false
bigints0=6,3,bigint,true,true
bigints1=6,3,bigint,true,true
mixedBigint0=true
mixedBigint1=true
objects0=2,3,8,pvpvvq
objects1=2,3,8,pvpvvq
orderOfConversion0=true,false,true,false:abababab
orderOfConversion1=true,false,true,false:abababab
nanAndUndefined0=NaN,NaN,10010
nanAndUndefined1=NaN,NaN,10010
symbols0=TypeError:TypeError
symbols1=TypeError:TypeError
largeIntegers0=9007199254740992,-9007199254740992,2147483648,9007199254740992,false,false
largeIntegers1=9007199254740992,-9007199254740992,2147483648,9007199254740992,false,false
negativeZero0=Infinity,Infinity,false,true
negativeZero1=Infinity,Infinity,false,true
throwInTest0=no00
throwInTest1=no00
closureCounter0=2,8
closureCounter1=2,8
ifAndTernary0=lt,ngt,le,nge,and
ifAndTernary1=lt,ngt,le,nge,and
nested0=7
nested1=7
valueUsed0=1,3,3,1,2,2,5,6,2
valueUsed1=1,3,3,1,2,2,5,6,2
constUpdate0=TypeError1
constUpdate1=TypeError1
tdzUpdate0=ReferenceError2
tdzUpdate1=ReferenceError2
generatorLoop0=10,true
generatorLoop1=10,true
argumentsLoop0=10
argumentsLoop1=10
switchInLoop0=0.a.2.c4.
switchInLoop1=0.a.2.c4.
labels0=10
labels1=10"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// `a[i]` read and `a[i] = v` assigned by one instruction each are the
/// Reference way: the order in which the object, the key and the value are
/// evaluated, when the key is made a property key, a base that is null,
/// keys of every kind, accessors and read-only elements, Proxies, typed
/// arrays, strings and plain objects.
#[test]
fn element_access_keeps_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(element_access)
        .expect("thread")
        .join()
        .expect("checks");
}

fn element_access() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name + ":" + e.message; }
    out.push(label + round + "=" + String(r));
  }
}
log("arrayReadWrite", function () { var a = [1, 2, 3]; a[0] = 10; a[2] = a[0] + a[1]; var i = 1; a[i] = a[i] * 5; return a.join() + "|" + a[5] + "|" + a[-1] + "|" + a.length; });
log("arrayGrowAndHoles", function () { var a = [1, , 3]; a[1] = 2; a[5] = 6; a[3] = a[4]; var s = []; for (var i = 0; i < a.length; i++) s.push(i in a ? a[i] : "hole"); return s.join() + "|" + a.length; });
log("keysOfOtherKinds", function () { var a = [1, 2, 3]; var k = "1", f = 1.5, n = -0, big = 4294967295; a[k] = "s"; a[f] = "f"; a[n] = "z"; a[big] = "b"; a["x"] = "p"; a[true] = "t"; a[null] = "n"; var sym = Symbol("y"); a[sym] = "sy"; return [a[k], a[f], a[n], a[big], a.x, a[true], a[null], a[sym], a.length, Object.keys(a).join("/")].join(); });
log("objectKeys", function () { var seen = []; var a = [0, 0]; var key = { toString: function () { seen.push("key"); return "1"; } }; function val() { seen.push("val"); return 7; } a[key] = val(); var r = a[key]; return a.join() + ":" + r + ":" + seen.join(); });
log("order", function () {
  var seen = [];
  function obj() { seen.push("obj"); return target; }
  function key() { seen.push("key"); return 0; }
  function val() { seen.push("val"); return 5; }
  var target = [9];
  obj()[key()] = val();
  var got = obj()[key()];
  return seen.join() + ":" + got;
});
log("nullBase", function () { var seen = []; function key() { seen.push("key"); return 0; } function val() { seen.push("val"); return 5; } var n = null; try { n[key()] = val(); } catch (e) { seen.push(e.constructor.name); } try { var u; u[key()]; } catch (e) { seen.push(e.constructor.name); } return seen.join(); });
log("rhsThrows", function () { var a = [1, 2]; var i = 0; try { a[i] = (function () { throw new RangeError("r"); })(); } catch (e) { return e.message + a.join(); } });
log("accessorsOnIndices", function () { var a = [1, 2, 3], store = "s"; Object.defineProperty(a, 1, { get: function () { return store + "!"; }, set: function (v) { store = v; }, configurable: true }); var i = 1; a[i] = "w"; var r = a[i]; Object.defineProperty(a, 0, { value: "ro", writable: false }); var j = 0; a[j] = "changed"; return r + "," + a[j] + "," + store; });
log("frozenAndStrict", function () { "use strict"; var a = Object.freeze([1, 2]); var i = 0; try { a[i] = 9; } catch (e) { return e.constructor.name + a[i]; } });
log("lengthAndSubclass", function () { class A extends Array {} var a = new A(3); var i = 0; a[i] = "x"; a[3] = "y"; var b = [1, 2, 3]; b.length = 1; var k = 2; b[k] = "z"; return a.length + ":" + a[i] + ":" + b.length + ":" + b.join(); });
log("typedArrays", function () { var u = new Uint8Array(4), f = new Float64Array(2); var i = 1; u[i] = 300; u[9] = 1; u[i + 1] = "7"; f[0] = 0.5; f[i] = u[i] + f[0]; return Array.from(u).join() + "|" + f[1] + "|" + u[9] + "|" + f[i]; });
log("plainObjects", function () { var o = {}; var i = 3, k = "name"; o[i] = "three"; o[k] = "n"; o[i + 1] = o[i] + "+"; return o[3] + "," + o[4] + "," + o.name + "," + Object.keys(o).join("/"); });
log("strings", function () { var s = "héllo"; var i = 1; var r = s[i] + s[0] + s[9] + s[-1]; try { (function () { "use strict"; s[i] = "x"; })(); } catch (e) { r += e.constructor.name; } var t = "abc"; t[0] = "z"; return r + t; });
log("argumentsObject", function () { function f() { var i = 0; arguments[i] = "changed"; arguments[2] = "added"; return arguments[i] + arguments.length + arguments[2]; } return f("a", "b"); });
log("proxy", function () { var seen = []; var p = new Proxy([1, 2], { get: function (t, k, r) { seen.push("get:" + String(k)); return Reflect.get(t, k, r); }, set: function (t, k, v, r) { seen.push("set:" + String(k)); return Reflect.set(t, k, v, r); } }); var i = 0; p[i] = 5; var x = p[i] + p[i + 1]; return seen.join() + ":" + x; });
log("compoundAndUpdate", function () { var a = [1, 2, 3]; var i = 0; a[i] += 5; a[i]++; ++a[i]; a[i + 1] *= 2; a[2]--; var b = a[i]++; return a.join() + ":" + b; });
log("chained", function () { var a = [0, 0], b = [0, 0]; var i = 0, j = 1; a[i] = b[j] = 5; var c = (a[j] = 7); return a.join() + "|" + b.join() + "|" + c; });
log("nestedAccess", function () { var m = [[1, 2], [3, 4]]; var i = 1, j = 0; m[i][j] = m[j][i] + 10; m[j][i + 1] = m[i][j]; return m.join("|"); });
log("keyChangesBase", function () { var a = [1, 2, 3]; var b = a; function k() { a = [7, 8, 9]; return 1; } b[k()] = "set"; var r = a[k()]; return b.join() + "|" + r; });
log("valueChangesArray", function () { var a = [1, 2, 3]; var i = 2; a[i] = (a.length = 1, "late"); var b = [1, 2, 3]; b[0] = (Object.freeze(b), "frozen?"); return a.join() + "|" + a.length + "|" + b.join(); });
log("spreadAndCalls", function () { var fs = [function (x) { return this.length + x; }]; var i = 0; var r = fs[i](10); var o = { 0: function () { return this === o; } }; return r + "," + o[i](); });
log("deleteAndIn", function () { var a = [1, 2, 3]; var i = 1; delete a[i]; return (i in a) + "," + a[i] + "," + a.length; });
log("forLoops", function () { var a = new Array(50); for (var i = 0; i < a.length; i++) a[i] = i * 2; var s = 0; for (var j = 0; j < a.length; j++) s += a[j]; var t = new Int32Array(50); for (var k = 0; k < t.length; k++) t[k] = a[k] - k; var u = 0; for (var q = 0; q < t.length; q++) u += t[q]; return s + "," + u; });
out.join("\n")
"#;
    let expected = r#"arrayReadWrite0=10,10,12|undefined|undefined|3
arrayReadWrite1=10,10,12|undefined|undefined|3
arrayGrowAndHoles0=1,2,3,,hole,6|6
arrayGrowAndHoles1=1,2,3,,hole,6|6
keysOfOtherKinds0=s,f,z,b,p,t,n,sy,3,0/1/2/1.5/4294967295/x/true/null
keysOfOtherKinds1=s,f,z,b,p,t,n,sy,3,0/1/2/1.5/4294967295/x/true/null
objectKeys0=0,7:7:val,key,key
objectKeys1=0,7:7:val,key,key
order0=obj,key,val,obj,key:5
order1=obj,key,val,obj,key:5
nullBase0=key,val,TypeError,key,TypeError
nullBase1=key,val,TypeError,key,TypeError
rhsThrows0=r1,2
rhsThrows1=r1,2
accessorsOnIndices0=w!,ro,w
accessorsOnIndices1=w!,ro,w
frozenAndStrict0=TypeError1
frozenAndStrict1=TypeError1
lengthAndSubclass0=4:x:3:1,,z
lengthAndSubclass1=4:x:3:1,,z
typedArrays0=0,44,7,0|44.5|undefined|44.5
typedArrays1=0,44,7,0|44.5|undefined|44.5
plainObjects0=three,three+,n,3/4/name
plainObjects1=three,three+,n,3/4/name
strings0=éhundefinedundefinedTypeErrorabc
strings1=éhundefinedundefinedTypeErrorabc
argumentsObject0=changed2added
argumentsObject1=changed2added
proxy0=set:0,get:0,get:1:7
proxy1=set:0,get:0,get:1:7
compoundAndUpdate0=9,4,2:8
compoundAndUpdate1=9,4,2:8
chained0=5,7|0,5|7
chained1=5,7|0,5|7
nestedAccess0=1,2,12|12,4
nestedAccess1=1,2,12|12,4
keyChangesBase0=1,set,3|8
keyChangesBase1=1,set,3|8
valueChangesArray0=1,,late|3|1,2,3
valueChangesArray1=1,,late|3|1,2,3
spreadAndCalls0=11,true
spreadAndCalls1=11,true
deleteAndIn0=false,undefined,3
deleteAndIn1=false,undefined,3
forLoops0=2450,1225
forLoops1=2450,1225"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// `push` appends directly only where nothing else would happen: setters
/// and read-only properties under integer keys on %Array.prototype% and
/// %Object.prototype%, Proxies and other objects on the prototype chain,
/// Arrays that cannot grow, subclasses and array-likes all take the
/// general way. And a method or another named property of an Array found
/// through the property cache is the one \[\[Get]] finds.
#[test]
fn array_push_and_methods_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(array_push_and_methods)
        .expect("thread")
        .join()
        .expect("checks");
}

fn array_push_and_methods() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name + ":" + e.message.length; }
    out.push(label + round + "=" + String(r));
  }
}
log("plain", function () { var a = []; var r = [a.push(1), a.push(2, 3), a.push(), a.push(undefined, null)]; return r.join() + "|" + a.join() + "|" + a.length; });
log("many", function () { var a = []; for (var i = 0; i < 1000; i++) a.push(i); var s = 0; while (a.length) s += a.pop(); return s + "," + a.length; });
log("kinds", function () { var a = [0.5]; a.push("s", {}, [1], null, 10n, Symbol.iterator, -0, NaN); return a.length + "," + typeof a[2] + "," + (1 / a[7]) + "," + a[8]; });
log("holesAndLength", function () { var a = [1, , 3]; a.push(4); var b = []; b.length = 5; b.push("x"); return a.length + ":" + (1 in a) + ":" + b.length + ":" + b[5] + ":" + (0 in b); });
log("frozen", function () { var a = Object.freeze([1]); try { a.push(2); } catch (e) { var r = e.constructor.name; } try { Object.freeze([]).push(); } catch (e) { r += "/" + e.constructor.name; } return r + a.length; });
log("sealedAndNonExtensible", function () { var a = Object.seal([1]), b = Object.preventExtensions([1]), r = []; try { a.push(2); } catch (e) { r.push(e.constructor.name); } try { b.push(2); } catch (e) { r.push(e.constructor.name); } return r.join() + a.length + b.length; });
log("lengthNotWritable", function () { var a = [1, 2]; Object.defineProperty(a, "length", { writable: false }); try { a.push(3); } catch (e) { return e.constructor.name + a.length + (2 in a); } });
log("elementAccessors", function () { var a = [1]; var seen = []; Object.defineProperty(a, 1, { set: function (v) { seen.push(v); }, get: function () { return "g"; }, configurable: true }); a.push("x"); return a.length + ":" + a[2] + ":" + seen.join(); });
log("subclass", function () { class A extends Array { push(x) { return super.push(x, x); } } var a = new A(); a.push(1); class B extends Array {} var b = new B(); b.push(5, 6); return a.join() + "|" + b.join() + "|" + (b instanceof B) + b.length; });
log("arrayLike", function () { var o = { length: 2 }; var r = Array.prototype.push.call(o, "a", "b"); var s = { length: "1" }; Array.prototype.push.call(s, "z"); return r + ":" + o[2] + o[3] + o.length + ":" + s[1] + s.length; });
log("prototypeSetter", function () {
  var seen = [];
  Object.defineProperty(Array.prototype, "2", { set: function (v) { seen.push("set" + v); }, get: function () { return "proto"; }, configurable: true });
  var a = [1, 2];
  var r = a.push("x");
  var own = Object.prototype.hasOwnProperty.call(a, 2);
  delete Array.prototype[2];
  Array.prototype.length = 0;
  var b = [1, 2]; b.push("y");
  return r + ":" + own + ":" + a.length + ":" + seen.join() + ":" + b.join() + ":" + Array.prototype.length;
});
log("objectPrototypeReadOnly", function () {
  Object.defineProperty(Object.prototype, "1", { value: "ro", writable: false, configurable: true });
  var a = [0], r;
  try { r = a.push("x"); } catch (e) { r = e.constructor.name; }
  var kept = a.length + ":" + a[1];
  delete Object.prototype[1];
  var b = [0]; b.push("y");
  return r + ":" + kept + ":" + b.join();
});
log("objectPrototypeSetter", function () {
  var seen = "";
  Object.defineProperty(Object.prototype, "0", { set: function (v) { seen += v + ";"; }, configurable: true });
  var a = [];
  var r = a.push("first", "second");
  delete Object.prototype[0];
  return r + ":" + a.length + ":" + (0 in a) + ":" + a[1] + ":" + seen;
});
log("otherPrototype", function () { var a = [1]; Object.setPrototypeOf(a, { push: Array.prototype.push, 1: "inherited" }); var seen = []; var b = [1]; Object.setPrototypeOf(b, new Proxy(Array.prototype, { set: function (t, k, v, r) { seen.push(String(k)); return Reflect.set(t, k, v, r); } })); a.push("x"); b.push("y"); return a.length + a[1] + ":" + b.length + b[1] + ":" + seen.join(); });
log("nullPrototype", function () { var a = [1]; Object.setPrototypeOf(a, null); var r = Array.prototype.push.call(a, 2); return r + ":" + a.length + ":" + Array.prototype.join.call(a); });
log("arrayPrototypeChain", function () {
  var original = Object.getPrototypeOf(Array.prototype);
  var seen = "";
  Object.setPrototypeOf(Array.prototype, new Proxy(original, { set: function (t, k, v, r) { seen += String(k) + ";"; return Reflect.set(t, k, v, r); } }));
  var a = [1]; var r = a.push(2);
  Object.setPrototypeOf(Array.prototype, original);
  var b = [1]; b.push(3);
  return r + ":" + a.join() + ":" + seen + ":" + b.join();
});
log("pushOverride", function () { var a = [1]; a.push = function (x) { return "own" + x; }; var r = a.push(2); var saved = Array.prototype.push; Array.prototype.push = function () { return "patched"; }; var b = []; var r2 = b.push(1); Array.prototype.push = saved; var r3 = b.push(1); return r + r2 + r3 + a.length + b.length; });
log("methodsThroughCache", function () { var a = [3, 1, 2]; function use(x) { return x.slice().sort().join() + x.indexOf(1) + x.includes(2) + x.concat([9]).length + x.map(function (v) { return v * 2; }).join("") + x.at(-1); } var r = use(a) + use([5]); a.extra = "e"; a.join = function () { return "own"; }; return r + use(a) + a.extra + a.missing; });
log("namedOnArray", function () { var a = [1]; function name(x) { return x.tag; } function has(x) { return x.constructor === Array; } var r = [name(a), has(a)]; a.tag = "t"; r.push(name(a), name([2]), has(a)); Array.prototype.tag = "proto"; r.push(name([3]), name(a)); delete Array.prototype.tag; r.push(name([4])); return r.join(); });
log("duringIteration", function () { var a = [1, 2, 3], n = 0; for (var i = 0; i < a.length && n < 10; i++) { if (a[i] < 3) a.push(a[i] + 1); n++; } return a.join(); });
log("spreadAndApply", function () { var a = [1]; a.push(...[2, 3]); a.push.apply(a, [4, 5]); Array.prototype.push.apply(a, [6]); var p = a.push; var r; try { r = p(7); } catch (e) { r = e.constructor.name; } return a.join() + ":" + r; });
out.join("\n")
"#;
    let expected = r#"plain0=1,3,3,5|1,2,3,,|5
plain1=1,3,3,5|1,2,3,,|5
many0=499500,0
many1=499500,0
kinds0=9,object,-Infinity,NaN
kinds1=9,object,-Infinity,NaN
holesAndLength0=4:false:6:x:false
holesAndLength1=4:false:6:x:false
frozen0=TypeError/TypeError1
frozen1=TypeError/TypeError1
sealedAndNonExtensible0=TypeError,TypeError11
sealedAndNonExtensible1=TypeError,TypeError11
lengthNotWritable0=TypeError2false
lengthNotWritable1=TypeError2false
elementAccessors0=3:x:
elementAccessors1=3:x:
subclass0=1,1|5,6|true2
subclass1=1,1|5,6|true2
arrayLike0=4:ab4:z2
arrayLike1=4:ab4:z2
prototypeSetter0=3:false:3:setx:1,2,y:0
prototypeSetter1=3:false:3:setx:1,2,y:0
objectPrototypeReadOnly0=TypeError:1:ro:0,y
objectPrototypeReadOnly1=TypeError:1:ro:0,y
objectPrototypeSetter0=2:2:false:second:first;
objectPrototypeSetter1=2:2:false:second:first;
otherPrototype0=2x:2y:1
otherPrototype1=2x:2y:1
nullPrototype0=2:2:1,2
nullPrototype1=2:2:1,2
arrayPrototypeChain0=2:1,2:1;:1,3
arrayPrototypeChain1=2:1,2:1;:1,3
pushOverride0=own2patched111
pushOverride1=own2patched111
methodsThroughCache0=1,2,31true462425-1false21051,2,31true46242eundefined
methodsThroughCache1=1,2,31true462425-1false21051,2,31true46242eundefined
namedOnArray0=,true,t,,true,proto,t,
namedOnArray1=,true,t,,true,proto,t,
duringIteration0=1,2,3,2,3,3
duringIteration1=1,2,3,2,3,3
spreadAndApply0=1,2,3,4,5,6:TypeError
spreadAndApply1=1,2,3,4,5,6:TypeError"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// A `for…of` loop steps a String Iterator and reads the `done` and `value`
/// of an ordinary result object directly: what it sees is what the general
/// protocol sees, for strings of every kind of code point, generators,
/// result objects with accessors, inherited or missing properties, Proxies
/// and other exotic results, and a `next` or `Symbol.iterator` put in place
/// of the original.
#[test]
fn iteration_keeps_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(iteration_semantics)
        .expect("thread")
        .join()
        .expect("checks");
}

fn iteration_semantics() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name + ":" + e.message.length; }
    out.push(label + round + "=" + String(r));
  }
}
function codes(s) { var r = []; for (var i = 0; i < s.length; i++) r.push(s.charCodeAt(i).toString(16)); return r.join("."); }
log("stringForOf", function () { var r = []; for (var ch of "aж中😀b") r.push(ch.length + ":" + codes(ch)); return r.join(" "); });
log("stringEmptyAndSurrogates", function () { var r = []; for (var c of "") r.push("never"); for (var d of "\ud83d" + "x" + "\ude00\ud83d") r.push(codes(d)); return r.join(" ") + "|" + r.length; });
log("stringObjectAndSpread", function () { var a = [...new String("héy")], b = Array.from("a😀"), c = [..."xy"].join("-"); var [p, q, ...rest] = "hello"; return a.join() + "|" + b.length + "|" + c + "|" + p + q + rest.join(""); });
log("stringIteratorByHand", function () { var it = "ab"[Symbol.iterator](); var r = [it.next(), it.next(), it.next(), it.next()]; return r.map(function (x) { return x.value + ":" + x.done + ":" + Object.keys(x).join(""); }).join(" ") + "|" + Object.prototype.toString.call(it) + "|" + (it[Symbol.iterator]() === it); });
log("stringIteratorPatched", function () {
  var proto = Object.getPrototypeOf(""[Symbol.iterator]());
  var original = proto.next, calls = 0;
  proto.next = function () { calls++; return original.call(this); };
  var r = []; for (var ch of "ab") r.push(ch);
  proto.next = function () { return { done: calls++ > 3, value: "p" }; };
  for (var c2 of "zz") r.push(c2);
  proto.next = original;
  for (var c3 of "q") r.push(c3);
  return r.join() + "|" + calls;
});
log("stringSymbolIteratorPatched", function () { var original = String.prototype[Symbol.iterator]; String.prototype[Symbol.iterator] = function () { var i = 0, s = this; return { next: function () { return i < 2 ? { value: s + i++, done: false } : { done: true }; } }; }; var r = []; try { for (var x of "k") r.push(x); } finally { String.prototype[Symbol.iterator] = original; } for (var y of "k") r.push(y); return r.join(); });
log("generators", function () {
  function* g(n) { for (var i = 0; i < n; i++) yield i * 2; return "ret"; }
  var r = []; for (var v of g(4)) r.push(v);
  var it = g(2); var a = [it.next(), it.next(), it.next(), it.next()].map(function (x) { return x.value + ":" + x.done; });
  var closed = []; function* h() { try { yield 1; yield 2; } finally { closed.push("fin"); } }
  for (var w of h()) { closed.push(w); break; }
  return r.join() + "|" + a.join(" ") + "|" + closed.join() + "|" + [...g(3)].join("") + "|" + Array.from(g(2), function (x) { return x + 1; }).join();
});
log("generatorDelegation", function () { function* inner() { var got = yield "a"; yield got; return "r"; } function* outer() { var r = yield* inner(); yield r; yield* [7, 8]; yield* "xy"; } var it = outer(); var first = it.next().value, second = it.next("sent").value; var rest = []; for (var v of it) rest.push(v); return first + second + "|" + rest.join(); });
log("generatorThrowAndReturn", function () { function* g() { try { yield 1; } catch (e) { yield "caught:" + e; } yield 3; } var it = g(); it.next(); var a = it.throw("boom").value; var b = it.return("early"); var c = it.next(); return a + "|" + b.value + ":" + b.done + "|" + c.value + ":" + c.done; });
log("customIterators", function () {
  function make(results) { var i = 0; return { [Symbol.iterator]: function () { return { next: function () { return results[i++]; } }; } }; }
  var r = [];
  for (var a of make([{ value: 1, done: false }, { value: 2 }, { done: true, value: "ignored" }])) r.push(a);
  for (var b of make([{ done: 0, value: "zero" }, { done: "", value: "empty" }, { done: 1 }])) r.push(b);
  for (var c of make([{ done: false }, { done: true }])) r.push(String(c));
  return r.join();
});
log("resultWithAccessors", function () {
  var seen = [];
  var it = { [Symbol.iterator]: function () { var n = 0; return { next: function () { n++; return { get done() { seen.push("done" + n); return n > 2; }, get value() { seen.push("value" + n); return n * 10; } }; } }; } };
  var r = []; for (var v of it) r.push(v);
  return r.join() + "|" + seen.join();
});
log("resultInherited", function () { var proto = { done: false, value: "inherited" }; var n = 0; var it = { [Symbol.iterator]: function () { return { next: function () { n++; var o = Object.create(proto); if (n > 2) o.done = true; if (n === 2) o.value = "own"; return o; } }; } }; var r = []; for (var v of it) r.push(v); return r.join(); });
log("resultExotic", function () {
  var seen = [];
  var n = 0;
  var it = { [Symbol.iterator]: function () { return { next: function () { n++; return new Proxy({ done: n > 1, value: "p" + n }, { get: function (t, k) { seen.push(String(k)); return t[k]; } }); } }; } };
  var r = []; for (var v of it) r.push(v);
  var arr = { [Symbol.iterator]: function () { var k = 0; return { next: function () { k++; var a = [k]; a.done = k > 2; a.value = "arr" + k; return a; } }; } };
  for (var w of arr) r.push(w);
  var frozen = { [Symbol.iterator]: function () { var k = 0; return { next: function () { k++; return Object.freeze({ value: "f" + k, done: k > 1 }); } }; } };
  for (var x of frozen) r.push(x);
  return r.join() + "|" + seen.join();
});
log("resultNotObject", function () { var it = { [Symbol.iterator]: function () { return { next: function () { return 5; } }; } }; for (var v of it) return "no"; });
log("resultManyProperties", function () { var it = { [Symbol.iterator]: function () { var k = 0; return { next: function () { k++; return { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, value: "big" + k, done: k > 1 }; } }; } }; var r = []; for (var v of it) r.push(v); return r.join(); });
log("mapsAndSets", function () { var m = new Map([[1, "a"], [2, "b"]]), s = new Set(["x", "y"]); var r = []; for (var [k, v] of m) r.push(k + v); for (var e of s) r.push(e); for (var k2 of m.keys()) r.push(k2); for (var en of s.entries()) r.push(en.join("")); return r.join(); });
log("arraysAndArguments", function () { var r = []; for (var v of [1, , 3]) r.push(String(v)); (function () { for (var a of arguments) r.push(a); })("p", "q"); for (var [i, e] of ["x", "y"].entries()) r.push(i + e); var ta = new Uint8Array([7, 8]); for (var t of ta) r.push(t); return r.join(); });
log("breakAndReturnCalls", function () { var seen = []; var it = { [Symbol.iterator]: function () { var n = 0; return { next: function () { return { value: n++, done: false }; }, return: function () { seen.push("return"); return {}; } }; } }; for (var v of it) { if (v > 1) break; } try { for (var w of it) { throw new Error("x"); } } catch (e) { seen.push("caught"); } return seen.join(); });
log("destructuring", function () { function* g() { yield 1; yield 2; yield 3; } var [a, , c = "d", d = "dflt"] = g(); var [x, ...ys] = "h😀y"; return [a, c, d, x, ys.length].join(); });
out.join("\n")
"#;
    let expected = r#"stringForOf0=1:61 1:436 1:4e2d 2:d83d.de00 1:62
stringForOf1=1:61 1:436 1:4e2d 2:d83d.de00 1:62
stringEmptyAndSurrogates0=d83d 78 de00 d83d|4
stringEmptyAndSurrogates1=d83d 78 de00 d83d|4
stringObjectAndSpread0=h,é,y|2|x-y|hello
stringObjectAndSpread1=h,é,y|2|x-y|hello
stringIteratorByHand0=a:false:valuedone b:false:valuedone undefined:true:valuedone undefined:true:valuedone|[object String Iterator]|true
stringIteratorByHand1=a:false:valuedone b:false:valuedone undefined:true:valuedone undefined:true:valuedone|[object String Iterator]|true
stringIteratorPatched0=a,b,p,q|5
stringIteratorPatched1=a,b,p,q|5
stringSymbolIteratorPatched0=k0,k1,k
stringSymbolIteratorPatched1=k0,k1,k
generators0=0,2,4,6|0:false 2:false ret:true undefined:true|1,fin|024|1,3
generators1=0,2,4,6|0:false 2:false ret:true undefined:true|1,fin|024|1,3
generatorDelegation0=asent|r,7,8,x,y
generatorDelegation1=asent|r,7,8,x,y
generatorThrowAndReturn0=caught:boom|early:true|undefined:true
generatorThrowAndReturn1=caught:boom|early:true|undefined:true
customIterators0=1,2,zero,empty,undefined
customIterators1=1,2,zero,empty,undefined
resultWithAccessors0=10,20|done1,value1,done2,value2,done3
resultWithAccessors1=10,20|done1,value1,done2,value2,done3
resultInherited0=inherited,own
resultInherited1=inherited,own
resultExotic0=p1,arr1,arr2,f1|done,value,done
resultExotic1=p1,arr1,arr2,f1|done,value,done
resultNotObject0=TypeError:37
resultNotObject1=TypeError:37
resultManyProperties0=big1
resultManyProperties1=big1
mapsAndSets0=1a,2b,x,y,1,2,xx,yy
mapsAndSets1=1a,2b,x,y,1,2,xx,yy
arraysAndArguments0=1,undefined,3,p,q,0x,1y,7,8
arraysAndArguments1=1,undefined,3,p,q,0x,1y,7,8
breakAndReturnCalls0=return,return,caught
breakAndReturnCalls1=return,return,caught
destructuring0=1,3,dflt,h,2
destructuring1=1,3,dflt,h,2"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// A `switch` on literals jumps by its table and `===`, `!==`, `==` and `!=`
/// are tested by the instructions that jump on them: both do what comparing
/// in order with IsStrictlyEqual and IsLooselyEqual does, for integers
/// inside and outside the table, the zeros, NaN, doubles, strings of both
/// kinds, BigInts, symbols and objects, for duplicate and missing cases, a
/// `default` anywhere, selectors that are not literals, completion values,
/// and for operands that are converted, in the order they are.
#[test]
fn switch_and_equality_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(switch_and_equality)
        .expect("thread")
        .join()
        .expect("checks");
}

fn switch_and_equality() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name + ":" + e.message.length; }
    out.push(label + round + "=" + String(r));
  }
}
var big = 1e20, half = 0.5;
var values = [0, 1, 2, 3, 7, 8, -1, -2, 255, 256, 1000, 100000, -0, 0.5 + half, 1.5, 2.5, NaN, Infinity, -Infinity, big, "1", "", "a", "abc", "a long string of text", "a long" + " string of text", true, false, null, undefined, 1n, 2n, {}, [], [1], Symbol.iterator, function () {}];
function each(f) { var r = []; for (var i = 0; i < values.length; i++) r.push(f(values[i])); return r.join(","); }
log("dense", function () { return each(function (v) { switch (v) { case 0: return "z"; case 1: return "o"; case 2: return "t"; case 3: return "h"; case 7: return "s"; default: return "-"; } }); });
log("denseNoDefault", function () { return each(function (v) { var r = "n"; switch (v) { case 1: r = "o"; break; case 2: r = "t"; break; case 8: r = "e"; } return r; }); });
log("negative", function () { return each(function (v) { switch (v) { case -1: return "m1"; case -2: return "m2"; case -0: return "mz"; case 1: return "p1"; } return "-"; }); });
log("sparse", function () { return each(function (v) { switch (v) { case 1: return "a"; case 1000: return "b"; case 100000: return "c"; case 256: return "d"; default: return "-"; } }); });
log("floats", function () { return each(function (v) { switch (v) { case 1.5: return "a"; case 2.5: return "b"; case 1e20: return "c"; case 0: return "z"; case 1: return "o"; default: return "-"; } }); });
log("strings", function () { return each(function (v) { switch (v) { case "1": return "one"; case "": return "empty"; case "a": return "a"; case "abc": return "abc"; case "a long string of text": return "long"; default: return "-"; } }); });
log("mixed", function () { return each(function (v) { switch (v) { case 1: return "n1"; case "1": return "s1"; case true: return "T"; case false: return "F"; case null: return "N"; case 0: return "n0"; case "": return "se"; default: return "-"; } }); });
log("duplicates", function () { return each(function (v) { switch (v) { case 1: return "first"; case 2: return "two"; case 1: return "second"; case "a": return "sa"; case "a": return "sb"; default: return "-"; } }); });
log("defaultFirst", function () { return each(function (v) { var r = ""; switch (v) { default: r += "d"; case 1: r += "1"; break; case 2: r += "2"; case 3: r += "3"; } return r; }); });
log("defaultMiddle", function () { return each(function (v) { var r = ""; switch (v) { case 1: r += "1"; default: r += "d"; case 2: r += "2"; break; case 3: r += "3"; } return r; }); });
log("emptyCases", function () { return each(function (v) { var r = "x"; switch (v) { case 0: case 1: case 2: r = "low"; break; case 3: case 7: case 8: r = "mid"; case 255: case 256: } return r; }); });
log("onlyDefault", function () { return each(function (v) { switch (v) { default: return typeof v; } }); });
log("emptySwitch", function () { var n = 0; switch (n++) { } switch (n++) { case 5: } return n; });
log("loopBreakContinue", function () {
  var r = "";
  outer: for (var i = 0; i < 12; i++) {
    switch (i % 6) {
      case 0: r += "a"; continue;
      case 1: r += "b"; break;
      case 2: r += "c"; continue outer;
      case 3: for (var j = 0; j < 3; j++) { switch (j) { case 1: continue; case 2: break; default: r += "j"; } r += j; } break;
      case 4: if (i > 6) break outer; r += "e";
      default: r += "f";
    }
    r += "|";
  }
  return r;
});
log("lexical", function () {
  var fs = [];
  for (var i = 0; i < 4; i++) {
    switch (i) { case 0: let a = "A" + i; fs.push(function () { return a; }); break; case 1: const b = "B"; fs.push(function () { return b + i; }); break; case 2: function inner() { return "I"; } fs.push(inner); break; default: class C { m() { return "C"; } } fs.push(function () { return new C().m(); }); }
  }
  return fs.map(function (f) { return f(); }).join();
});
log("sideEffects", function () {
  var order = "";
  function k(v) { order += v; return v; }
  var r = [];
  for (var v = 0; v < 5; v++) { switch (v) { case k(1): r.push("a"); break; case 2: r.push("b"); break; case k(3): r.push("c"); break; default: r.push("d"); case k(9): r.push("e"); } order += ";"; }
  return r.join("") + "|" + order;
});
log("thrownSelector", function () { var r = ""; for (var v = 0; v < 3; v++) { try { switch (v) { case 0: r += "0"; break; case (function () { throw new RangeError("x"); })(): r += "never"; break; default: r += "d"; } } catch (e) { r += e.constructor.name; } } return r; });
log("getterDiscriminant", function () { var n = 0, o = { get v() { n++; return 2; } }; var r = ""; switch (o.v) { case 1: r = "1"; break; case 2: r = "2"; break; case 3: r = "3"; } return r + n; });
log("completion", function () {
  return [eval("switch (1) { case 1: 5 }"), eval("switch (2) { case 1: 5; default: }"), eval("switch (1) { case 1: 5; case 2: 6; break; case 3: 7 }"), eval("1; switch (3) { case 1: 5; break; }"), eval("2; switch ('b') { case 'a': 5; break; default: case 'b': }"), eval("3; do { switch (1) { case 1: 9; continue; } } while (false)"), eval("4; l: switch (1) { case 1: 8; break l; }"), eval("switch (2) { case 1: 'a'; case 2: case 3: 'c'; case 4: }"), eval("var q = 0; for (var i = 0; i < 3; i++) switch (i) { case 0: 'x'; break; case 1: q; break; default: }")].join();
});
log("generator", function () {
  function* g(v) { switch (v) { case 1: yield "a"; case 2: var got = yield "b"; yield got; break; case "s": try { yield "t"; } finally { yield "f"; } default: yield "d"; } return "r"; }
  var r = []; for (var v of [1, 2, "s", 9]) { var it = g(v), step = it.next(); while (!step.done) { r.push(step.value); step = it.next("sent"); } r.push(step.value); }
  return r.join();
});
log("asyncAndTry", function () {
  var r = "";
  function f(v) { try { switch (v) { case 1: return "one"; case 2: throw new Error("two"); case 3: break; default: return "dflt"; } return "after"; } catch (e) { return "caught" + e.message; } finally { r += v; } }
  return [f(1), f(2), f(3), f(4)].join() + r;
});
log("nested", function () { var r = ""; for (var a = 0; a < 3; a++) for (var b = 0; b < 3; b++) switch (a) { case 0: switch (b) { case 0: r += "00"; break; case 1: r += "01"; default: r += "0d"; } break; case 1: switch ("k" + b) { case "k0": r += "10"; break; case "k2": r += "12"; } default: r += "x" + b; } return r; });
log("manyCases", function () {
  var src = "switch (v) {"; for (var i = 0; i < 300; i++) src += "case " + (i * 3) + ": return " + i + ";"; src += "default: return -1; }";
  var f = new Function("v", src), g = new Function("v", src.replace("case 0:", "case 5000000:"));
  return [f(0), f(3), f(4), f(897), f(900), f(-3), f("3"), g(0), g(5000000), g(897), g(1.5)].join();
});
log("wideStrings", function () {
  var src = "switch (v) {"; for (var i = 0; i < 40; i++) src += "case 'key number " + i + "': return " + i + ";"; src += "}";
  var f = new Function("v", src);
  return [f("key number 0"), f("key number " + 39), f("key number 40"), f(39)].join();
});
log("arrowAndMethods", function () { var o = { m(v) { switch (v) { case 1: return this.k; default: return "d"; } }, k: "K" }; var a = (v) => { switch (v) { case "x": return 1; } return 0; }; return o.m(1) + o.m(2) + a("x") + a("y"); });
log("strictOps", function () { return each(function (v) { var r = ""; if (v === 1) r += "a"; if (v !== 1) r += "b"; if (v === "a") r += "c"; if (v === null) r += "d"; if (v !== undefined) r += "e"; if (v === v) r += "f"; if (v === 0) r += "g"; if (1.5 === v) r += "h"; if (v === true) r += "i"; return r; }); });
log("looseOps", function () { return each(function (v) { var r = ""; try { if (v == 1) r += "a"; if (v != 1) r += "b"; if (v == "a") r += "c"; if (v == null) r += "d"; if (v != undefined) r += "e"; if (v == v) r += "f"; if (v == 0) r += "g"; if ("1" == v) r += "h"; if (v == true) r += "i"; if (v == "") r += "j"; } catch (e) { r += e.constructor.name; } return r; }); });
log("pairs", function () { var r = []; for (var i = 0; i < values.length; i += 3) for (var j = 0; j < values.length; j += 2) { var a = values[i], b = values[j], bits = 0; try { if (a === b) bits |= 1; if (a !== b) bits |= 2; if (a == b) bits |= 4; if (a != b) bits |= 8; bits |= (a === b ? 16 : 0) | (a == b ? 32 : 0); } catch (e) { bits = "T"; } r.push(bits); } return r.join(""); });
log("coercionOrder", function () {
  var order = "";
  var a = { valueOf: function () { order += "a"; return 1; } }, b = { toString: function () { order += "b"; return "1"; }, valueOf: undefined };
  var r = ""; if (a == 1) r += "1"; if (1 != a) r += "2"; if (a == b) r += "3"; if (b == "1") r += "4"; if (a != "1") r += "5"; if (a === 1) r += "6"; if (b == true) r += "7";
  var thrower = { valueOf: function () { throw new TypeError("no"); } };
  try { if (thrower == 1) r += "8"; } catch (e) { r += "T"; }
  try { while (thrower != 1) r += "9"; } catch (e) { r += "U"; }
  return r + "|" + order;
});
log("backEdges", function () {
  var r = "", i = 0; do { i++; } while (i !== 5); r += i;
  i = 0; do { i++; } while (i != "7"); r += i;
  i = 0; do { i += 0.5; } while (!(i === 3)); r += i;
  var s = ""; do { s += "x"; } while (s != "xxx"); r += s;
  var n = 10; while (n !== 0) n--; r += n; for (var k = 0; k != 4; k++) r += k;
  var o = { valueOf: function () { return 2; } }, c = 0; do { c++; } while (c != o); r += c;
  return r;
});
log("conditionForms", function () { var a = 3, b = "3", u, r = ""; r += a === 3 ? "a" : "b"; r += a == b && a !== b ? "c" : "d"; r += (u == null || a === 4) ? "e" : "f"; r += !(a === 3) ? "g" : "h"; r += !(a != b) ? "i" : "j"; if (!(u !== undefined)) r += "k"; var t = a === 3; r += t; var arr = [1, 2, 3], hits = 0; for (var i = 0; i < arr.length; i++) if (arr[i] === a || arr[i] == "1") hits++; return r + hits; });
log("bigintAndSymbol", function () { var s = Symbol("s"), t = s, r = ""; if (s === t) r += "a"; if (s == t) r += "b"; if (1n == 1) r += "c"; if (1n === 1) r += "d"; if (2n == "2") r += "e"; if (10n ** 30n == 10n ** 30n) r += "f"; if (10n ** 30n === 10n ** 30n) r += "g"; try { if (s == "s") r += "h"; } catch (e) { r += "T"; } switch (2n) { case 2: r += "i"; break; default: r += "j"; } return r; });
log("nullish", function () { return each(function (v) { var r = ""; if (v == null) r += "n"; if (v === undefined) r += "u"; if (v === null) r += "l"; r += (v ?? "dflt") === "dflt" ? "d" : "v"; if (!v) r += "!"; if (!!v === true) r += "t"; return r; }); });
out.join("\n")
"#;
    let expected = r#"dense0=z,o,t,h,s,-,-,-,-,-,-,-,z,o,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-
dense1=z,o,t,h,s,-,-,-,-,-,-,-,z,o,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-
denseNoDefault0=n,o,t,n,n,e,n,n,n,n,n,n,n,o,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n
denseNoDefault1=n,o,t,n,n,e,n,n,n,n,n,n,n,o,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n,n
negative0=mz,p1,-,-,-,-,m1,m2,-,-,-,-,mz,p1,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-
negative1=mz,p1,-,-,-,-,m1,m2,-,-,-,-,mz,p1,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-
sparse0=-,a,-,-,-,-,-,-,-,d,b,c,-,a,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-
sparse1=-,a,-,-,-,-,-,-,-,d,b,c,-,a,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-
floats0=z,o,-,-,-,-,-,-,-,-,-,-,z,o,a,b,-,-,-,c,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-
floats1=z,o,-,-,-,-,-,-,-,-,-,-,z,o,a,b,-,-,-,c,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-
strings0=-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,one,empty,a,abc,long,long,-,-,-,-,-,-,-,-,-,-,-
strings1=-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,-,one,empty,a,abc,long,long,-,-,-,-,-,-,-,-,-,-,-
mixed0=n0,n1,-,-,-,-,-,-,-,-,-,-,n0,n1,-,-,-,-,-,-,s1,se,-,-,-,-,T,F,N,-,-,-,-,-,-,-,-
mixed1=n0,n1,-,-,-,-,-,-,-,-,-,-,n0,n1,-,-,-,-,-,-,s1,se,-,-,-,-,T,F,N,-,-,-,-,-,-,-,-
duplicates0=-,first,two,-,-,-,-,-,-,-,-,-,-,first,-,-,-,-,-,-,-,-,sa,-,-,-,-,-,-,-,-,-,-,-,-,-,-
duplicates1=-,first,two,-,-,-,-,-,-,-,-,-,-,first,-,-,-,-,-,-,-,-,sa,-,-,-,-,-,-,-,-,-,-,-,-,-,-
defaultFirst0=d1,1,23,3,d1,d1,d1,d1,d1,d1,d1,d1,d1,1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1
defaultFirst1=d1,1,23,3,d1,d1,d1,d1,d1,d1,d1,d1,d1,1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1,d1
defaultMiddle0=d2,1d2,2,3,d2,d2,d2,d2,d2,d2,d2,d2,d2,1d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2
defaultMiddle1=d2,1d2,2,3,d2,d2,d2,d2,d2,d2,d2,d2,d2,1d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2,d2
emptyCases0=low,low,low,mid,mid,mid,x,x,x,x,x,x,low,low,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x
emptyCases1=low,low,low,mid,mid,mid,x,x,x,x,x,x,low,low,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x,x
onlyDefault0=number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,string,string,string,string,string,string,boolean,boolean,object,undefined,bigint,bigint,object,object,object,symbol,function
onlyDefault1=number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,number,string,string,string,string,string,string,boolean,boolean,object,undefined,bigint,bigint,object,object,object,symbol,function
emptySwitch0=2
emptySwitch1=2
loopBreakContinue0=ab|cj02|ef|f|ab|cj02|
loopBreakContinue1=ab|cj02|ef|f|ab|cj02|
lexical0=A0,B4,I,C
lexical1=A0,B4,I,C
sideEffects0=deabcde|139;1;1;13;139;
sideEffects1=deabcde|139;1;1;13;139;
thrownSelector0=0RangeErrorRangeError
thrownSelector1=0RangeErrorRangeError
getterDiscriminant0=21
getterDiscriminant1=21
completion0=5,,6,,,9,8,c,
completion1=5,,6,,,9,8,c,
generator0=a,b,sent,r,b,sent,r,t,f,d,r,d,r
generator1=a,b,sent,r,b,sent,r,t,f,d,r,d,r
asyncAndTry0=one,caughttwo,after,dflt1234
asyncAndTry1=one,caughttwo,after,dflt1234
nested0=00010d0d10x0x112x2x0x1x2
nested1=00010d0d10x0x112x2x0x1x2
manyCases0=0,1,-1,299,-1,-1,-1,-1,0,299,-1
manyCases1=0,1,-1,299,-1,-1,-1,-1,0,299,-1
wideStrings0=0,39,,
wideStrings1=0,39,,
arrowAndMethods0=Kd10
arrowAndMethods1=Kd10
strictOps0=befg,aef,bef,bef,bef,bef,bef,bef,bef,bef,bef,bef,befg,aef,befh,bef,be,bef,bef,bef,bef,bef,bcef,bef,bef,bef,befi,bef,bdef,bf,bef,bef,bef,bef,bef,bef,bef
strictOps1=befg,aef,bef,bef,bef,bef,bef,bef,bef,bef,bef,bef,befg,aef,befh,bef,be,bef,bef,bef,bef,bef,bcef,bef,bef,bef,befi,bef,bdef,bf,bef,bef,bef,bef,bef,bef,bef
looseOps0=befgj,aefhi,bef,bef,bef,bef,bef,bef,bef,bef,bef,bef,befgj,aefhi,bef,bef,be,bef,bef,bef,aefhi,befgj,bcef,bef,bef,bef,aefhi,befgj,bdf,bdf,aefhi,bef,bef,befgj,aefhi,bef,bef
looseOps1=befgj,aefhi,bef,bef,bef,bef,bef,bef,bef,bef,bef,bef,befgj,aefhi,bef,bef,be,bef,bef,bef,aefhi,befgj,bcef,bef,bef,bef,aefhi,befgj,bdf,bdf,aefhi,bef,bef,befgj,aefhi,bef,bef
pairs0=53101010101053101010101010101010101010101010101010101010101010101010101010101010105310101010101010101010101010101010101010101010101010101010101010101010531010101010531010101010101010101010101010101010101010101010101010101010101010101010101010101053101010101010101010381010101010381010101010101010101010101010101010101010101010105310101010101038101010101038101010101010101010101010101010101010101010103810103810531038103810101010103810101010101010101010101010101010101010101010101010101010101053
pairs1=53101010101053101010101010101010101010101010101010101010101010101010101010101010105310101010101010101010101010101010101010101010101010101010101010101010531010101010531010101010101010101010101010101010101010101010101010101010101010101010101010101053101010101010101010381010101010381010101010101010101010101010101010101010101010105310101010101038101010101038101010101010101010101010101010101010101010103810103810531038103810101010103810101010101010101010101010101010101010101010101010101010101053
coercionOrder0=147TU|aabab
coercionOrder1=147TU|aabab
backEdges0=573xxx001232
backEdges1=573xxx001232
conditionForms0=acehiktrue2
conditionForms1=acehiktrue2
bigintAndSymbol0=abcefgj
bigintAndSymbol1=abcefgj
nullish0=v!,vt,vt,vt,vt,vt,vt,vt,vt,vt,vt,vt,v!,vt,vt,vt,v!,vt,vt,vt,vt,v!,vt,vt,vt,vt,vt,v!,nld!,nud!,vt,vt,vt,vt,vt,vt,vt
nullish1=v!,vt,vt,vt,vt,vt,vt,vt,vt,vt,vt,vt,v!,vt,vt,vt,v!,vt,vt,vt,vt,v!,vt,vt,vt,vt,vt,v!,nld!,nud!,vt,vt,vt,vt,vt,vt,vt"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The functions of Math that are computed in place of a call return what
/// the call returns: for every kind of Number, the zeros among them, for
/// arguments that are converted (in order, and where the conversion
/// throws), for the functions under another name or `this`, and not at all
/// once another function is put in their place.
#[test]
fn direct_builtin_calls_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(direct_builtin_calls)
        .expect("thread")
        .join()
        .expect("checks");
}

fn direct_builtin_calls() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = e.constructor.name + ":" + e.message.length; }
    out.push(label + round + "=" + String(r));
  }
}
function show(v) { return typeof v === "number" ? (Object.is(v, -0) ? "-0" : String(v)) : typeof v + ":" + String(v); }
var nums = [0, -0, 1, -1, 2, 7, 0.5, -0.5, 0.49999999999999994, 1.5, -1.5, 2.5, -2.5, 0.1, -0.1, 0.9, -0.9, 1e-320, 4, 9, 2.25, 1e21, -1e21, 4503599627370495.5, 9007199254740991, -9007199254740991, 9007199254740993, 4294967296, 2147483647.5, -2147483648.5, NaN, Infinity, -Infinity, 123456789.123, 1e300, 5e-324];
var names = ["floor", "ceil", "round", "trunc", "abs", "sqrt", "sign"];
log("direct", function () {
  var r = [];
  for (var i = 0; i < nums.length; i++) { var x = nums[i]; r.push([Math.floor(x), Math.ceil(x), Math.round(x), Math.trunc(x), Math.abs(x), Math.sqrt(x), Math.sign(x)].map(show).join(" ")); }
  return r.join("|");
});
log("directEqualsApplied", function () {
  var bad = [];
  for (var i = 0; i < nums.length; i++) {
    var x = nums[i], direct = [Math.floor(x), Math.ceil(x), Math.round(x), Math.trunc(x), Math.abs(x), Math.sqrt(x), Math.sign(x)];
    for (var k = 0; k < names.length; k++) { var applied = Reflect.apply(Math[names[k]], undefined, [x]); if (!Object.is(direct[k], applied)) bad.push(names[k] + "(" + show(x) + ")"); }
  }
  for (var a = 0; a < nums.length; a += 2) for (var b = 0; b < nums.length; b += 3) {
    var p = nums[a], q = nums[b];
    if (!Object.is(Math.max(p, q), Reflect.apply(Math.max, null, [p, q]))) bad.push("max" + show(p) + "," + show(q));
    if (!Object.is(Math.min(p, q), Reflect.apply(Math.min, null, [p, q]))) bad.push("min" + show(p) + "," + show(q));
  }
  return bad.length + ":" + bad.join();
});
log("maxMin", function () { return [Math.max(), Math.min(), Math.max(3), Math.min(3), Math.max(1, 2), Math.min(1, 2), Math.max(-1, -2, -3), Math.min(5, 4, 3, 2, 9), Math.max(0, -0), Math.min(0, -0), Math.max(-0, 0), Math.min(-0, 0), Math.max(1, 2.5), Math.min(1.5, 2), Math.max(1, NaN), Math.min(NaN, 1), Math.max(1, "3"), Math.min("0x10", 20), Math.max(1, undefined), Math.min(null, 1), Math.max(9007199254740991, 9007199254740990), Math.min(-9007199254740991, 1), Math.max(Infinity, 1), Math.min(-Infinity, 1), Math.max.apply(null, [4, 8, 2]), Math.max(...[1, 5, 3]), Math.min(...[]), Math.max(true, false)].map(show).join(" "); });
log("conversions", function () {
  var order = "";
  var o = { valueOf: function () { order += "v"; return 2.5; } }, s = { toString: function () { order += "s"; return "9"; }, valueOf: undefined };
  var r = [Math.floor(o), Math.ceil(s), Math.round("2.5"), Math.trunc("-7.9"), Math.abs("-3"), Math.sqrt("16"), Math.sign("-2"), Math.floor(), Math.ceil(undefined), Math.round(null), Math.trunc(true), Math.abs([]), Math.sqrt([4]), Math.sign({}), Math.floor("x"), Math.floor(1.5, o), Math.max(o, s, 1), Math.min(o, s)].map(show).join(" ");
  return r + "|" + order;
});
log("throws", function () {
  var r = [];
  var fs = [function () { return Math.floor(1n); }, function () { return Math.abs(Symbol()); }, function () { return Math.max(1, 2n); }, function () { return Math.min(1, { valueOf: function () { throw new RangeError("boom"); } }); }, function () { return Math.sqrt({ valueOf: function () { throw 5; } }); }, function () { return Math.round(Symbol.iterator); }];
  for (var i = 0; i < fs.length; i++) { try { r.push(show(fs[i]())); } catch (e) { r.push(e && e.constructor ? e.constructor.name : String(e)); } }
  return r.join();
});
log("aliases", function () {
  var f = Math.floor, mx = Math.max, o = { m: Math.min, r: Math.round }, arr = [Math.abs, Math.sqrt];
  function call(fn, a, b) { return fn(a, b); }
  return [f(2.7), mx(1, 9), o.m(4, 2), o.r(2.5), arr[0](-3), arr[1](49), call(Math.trunc, -2.9), call(Math.max, 2, 3), (0, Math.ceil)(1.2), Math["floor"](3.9), Math.floor.call(null, 4.9), Math.max.bind(null, 10)(3), [1.5, 2.5, -0.5].map(Math.round).map(show).join("/"), [3, 1, 2].reduce(function (a, b) { return Math.max(a, b); })].join();
});
log("patched", function () {
  var original = Math.floor, calls = 0, r = [];
  Math.floor = function (x) { calls++; return "patched" + x; };
  r.push(Math.floor(1.5));
  Math.floor = original; r.push(Math.floor(1.5));
  var m = Math.max; Math.max = Math.min; r.push(Math.max(1, 2)); Math.max = m; r.push(Math.max(1, 2));
  var desc = Object.getOwnPropertyDescriptor(Math, "abs");
  Object.defineProperty(Math, "abs", { get: function () { calls++; return function () { return "getter"; }; }, configurable: true });
  r.push(Math.abs(-1)); Object.defineProperty(Math, "abs", desc); r.push(Math.abs(-1));
  return r.join() + "|" + calls;
});
log("thisAndNew", function () { var r = []; r.push(Math.floor.call({}, 1.5), Math.max.call(5, 1, 2), Math.floor.length, Math.max.length, Math.floor.name, typeof Math.sqrt); try { new Math.floor(1); } catch (e) { r.push(e.constructor.name); } try { Math.floor.caller; r.push("ok"); } catch (e) { r.push(e.constructor.name); } return r.join(); });
log("inExpressions", function () {
  var total = 0, parts = [];
  for (var i = 0; i < 40; i++) total += Math.floor(Math.sqrt(i)) + Math.max(i & 7, 3) - Math.min(i, 5) + Math.abs(i - 20) * Math.sign(i - 10) + Math.round(i / 3) + Math.ceil(i / 7) + Math.trunc(-i / 4);
  parts.push(total);
  var x = 10.75; parts.push(Math.floor(x) + Math.floor(-x), Math.round(x) === 11, Math.floor(x / 0), Math.floor(-0) === 0, 1 / Math.floor(-0), 1 / Math.ceil(-0.5), 1 / Math.round(-0.2), 1 / Math.trunc(-0.9), 1 / Math.abs(-0), 1 / Math.sqrt(-0), 1 / Math.sign(-0), 1 / Math.max(-0, -0), 1 / Math.min(0, 0));
  function nested(n) { return n < 2 ? Math.max(n, 0) : nested(Math.floor(n / 2)) + Math.min(n & 1, 1); }
  parts.push(nested(1000), nested(12345.9));
  return parts.join();
});
log("argumentCounts", function () { function f() { return Math.max.apply(Math, arguments); } var big = []; for (var i = 0; i < 300; i++) big.push(i % 97); return [f(1, 2, 3), f(), Math.floor(1.5, 2, 3), Math.max(1, 2, 3, 4, 5, 6, 7, 8, 9, 10), Math.min(...big), Math.max(...big), Math.max(...big, 1.5, "200")].map(show).join(); });
log("generatorAndAsync", function () { function* g() { var x = yield Math.floor(1.5); yield Math.max(x, 3); yield* [Math.sqrt(81)]; } var it = g(); return [it.next().value, it.next(7).value, it.next().value].join(); });
log("trigAndPowEqualApplied", function () {
  var bad = [], n = 0;
  for (var i = 0; i < nums.length; i++) {
    var x = nums[i];
    if (!Object.is(Math.sin(x), Reflect.apply(Math.sin, null, [x]))) bad.push("sin" + show(x));
    if (!Object.is(Math.cos(x), Reflect.apply(Math.cos, null, [x]))) bad.push("cos" + show(x));
    for (var j = 0; j < nums.length; j++) { var y = nums[j]; n++; if (!Object.is(Math.pow(x, y), Reflect.apply(Math.pow, null, [x, y]))) bad.push("pow" + show(x) + "," + show(y)); if (!Object.is(Math.max(x, y), Reflect.apply(Math.max, null, [x, y]))) bad.push("max" + show(x) + "," + show(y)); if (!Object.is(Math.min(x, y), Reflect.apply(Math.min, null, [x, y]))) bad.push("min" + show(x) + "," + show(y)); }
  }
  for (var a = 0; a < nums.length; a += 5) for (var b = 0; b < nums.length; b += 4) for (var c = 0; c < nums.length; c += 3) { var p = nums[a], q = nums[b], t = nums[c]; n++; if (!Object.is(Math.max(p, q, t), Reflect.apply(Math.max, null, [p, q, t]))) bad.push("max3"); if (!Object.is(Math.min(p, q, t), Reflect.apply(Math.min, null, [p, q, t]))) bad.push("min3"); }
  return bad.length + "/" + n + ":" + bad.slice(0, 5).join();
});
log("floatMinMax", function () { return [Math.max(0.5, 1.5), Math.min(0.5, 1.5), Math.max(-0, 0), Math.max(0, -0), Math.min(-0, 0), Math.min(0, -0), Math.max(-0, -0), Math.min(-0, -0.0), Math.max(1, 2.5, 2), Math.min(3, 2.5, 4), Math.max(0.1, NaN, 5), Math.min(NaN, 0.1), Math.max(-Infinity, -1e308), Math.min(Infinity, 1e308), Math.max(1e21, 9007199254740991), Math.min(-1e21, -9007199254740991), Math.max(0.5, "1.5"), Math.min(0.5, null), Math.max(2.5, undefined), Math.min(0.5, true), Math.max(0, Math.min(1, 0.25)), Math.max(0, Math.min(1, 7.5)), Math.max(0, Math.min(1, -3.5)), Math.pow(2, 10), Math.pow(2, -2), Math.pow(-2, 3), Math.pow(0, 0), Math.pow(NaN, 0), Math.pow(2, 1024), Math.pow(7, 22), Math.pow(-0, -1), Math.pow("3", "2"), Math.pow(2), Math.pow(), Math.sin(0), Math.sin(-0), Math.cos(0), Math.sin("0.5") === Math.sin(0.5), Math.cos()].map(show).join(" "); });
log("trigConversionsAndPatches", function () {
  var order = "", o = { valueOf: function () { order += "v"; return 0.5; } }, r = [];
  r.push(Math.sin(o) === Math.sin(0.5), Math.cos(o) === Math.cos(0.5), Math.pow(o, o) === Math.pow(0.5, 0.5), Math.pow(2, o) === Math.pow(2, 0.5));
  try { Math.sin(1n); } catch (e) { r.push(e.constructor.name); } try { Math.pow(2, Symbol()); } catch (e) { r.push(e.constructor.name); } try { Math.pow({ valueOf: function () { throw new RangeError("b"); } }, { valueOf: function () { order += "never"; return 1; } }); } catch (e) { r.push(e.constructor.name); }
  var s = Math.sin, original = Math.pow; Math.pow = function () { return "patched"; }; r.push(Math.pow(2, 3)); Math.pow = original; r.push(Math.pow(2, 3), s(0), (0, Math.cos)(0), Math.sin.call(null, 0), [0, 0].map(Math.cos).join(""));
  return r.join() + "|" + order;
});
out.join("\n")
"#;
    let expected = r#"direct0=0 0 0 0 0 0 0|-0 -0 -0 -0 0 -0 -0|1 1 1 1 1 1 1|-1 -1 -1 -1 1 NaN -1|2 2 2 2 2 1.4142135623730951 1|7 7 7 7 7 2.6457513110645907 1|0 1 1 0 0.5 0.7071067811865476 1|-1 -0 -0 -0 0.5 NaN -1|0 1 0 0 0.49999999999999994 0.7071067811865475 1|1 2 2 1 1.5 1.224744871391589 1|-2 -1 -1 -1 1.5 NaN -1|2 3 3 2 2.5 1.5811388300841898 1|-3 -2 -2 -2 2.5 NaN -1|0 1 0 0 0.1 0.31622776601683794 1|-1 -0 -0 -0 0.1 NaN -1|0 1 1 0 0.9 0.9486832980505138 1|-1 -0 -1 -0 0.9 NaN -1|0 1 0 0 1e-320 9.99994433575849e-161 1|4 4 4 4 4 2 1|9 9 9 9 9 3 1|2 3 2 2 2.25 1.5 1|1e+21 1e+21 1e+21 1e+21 1e+21 31622776601.683792 1|-1e+21 -1e+21 -1e+21 -1e+21 1e+21 NaN -1|4503599627370495 4503599627370496 4503599627370496 4503599627370495 4503599627370495.5 67108863.99999999 1|9007199254740991 9007199254740991 9007199254740991 9007199254740991 9007199254740991 94906265.62425154 1|-9007199254740991 -9007199254740991 -9007199254740991 -9007199254740991 9007199254740991 NaN -1|9007199254740992 9007199254740992 9007199254740992 9007199254740992 9007199254740992 94906265.62425156 1|4294967296 4294967296 4294967296 4294967296 4294967296 65536 1|2147483647 2147483648 2147483648 2147483647 2147483647.5 46340.95000644678 1|-2147483649 -2147483648 -2147483648 -2147483648 2147483648.5 NaN -1|NaN NaN NaN NaN NaN NaN NaN|Infinity Infinity Infinity Infinity Infinity Infinity 1|-Infinity -Infinity -Infinity -Infinity Infinity NaN -1|123456789 123456790 123456789 123456789 123456789.123 11111.111066090556 1|1e+300 1e+300 1e+300 1e+300 1e+300 1e+150 1|0 1 0 0 5e-324 2.2227587494850775e-162 1
direct1=0 0 0 0 0 0 0|-0 -0 -0 -0 0 -0 -0|1 1 1 1 1 1 1|-1 -1 -1 -1 1 NaN -1|2 2 2 2 2 1.4142135623730951 1|7 7 7 7 7 2.6457513110645907 1|0 1 1 0 0.5 0.7071067811865476 1|-1 -0 -0 -0 0.5 NaN -1|0 1 0 0 0.49999999999999994 0.7071067811865475 1|1 2 2 1 1.5 1.224744871391589 1|-2 -1 -1 -1 1.5 NaN -1|2 3 3 2 2.5 1.5811388300841898 1|-3 -2 -2 -2 2.5 NaN -1|0 1 0 0 0.1 0.31622776601683794 1|-1 -0 -0 -0 0.1 NaN -1|0 1 1 0 0.9 0.9486832980505138 1|-1 -0 -1 -0 0.9 NaN -1|0 1 0 0 1e-320 9.99994433575849e-161 1|4 4 4 4 4 2 1|9 9 9 9 9 3 1|2 3 2 2 2.25 1.5 1|1e+21 1e+21 1e+21 1e+21 1e+21 31622776601.683792 1|-1e+21 -1e+21 -1e+21 -1e+21 1e+21 NaN -1|4503599627370495 4503599627370496 4503599627370496 4503599627370495 4503599627370495.5 67108863.99999999 1|9007199254740991 9007199254740991 9007199254740991 9007199254740991 9007199254740991 94906265.62425154 1|-9007199254740991 -9007199254740991 -9007199254740991 -9007199254740991 9007199254740991 NaN -1|9007199254740992 9007199254740992 9007199254740992 9007199254740992 9007199254740992 94906265.62425156 1|4294967296 4294967296 4294967296 4294967296 4294967296 65536 1|2147483647 2147483648 2147483648 2147483647 2147483647.5 46340.95000644678 1|-2147483649 -2147483648 -2147483648 -2147483648 2147483648.5 NaN -1|NaN NaN NaN NaN NaN NaN NaN|Infinity Infinity Infinity Infinity Infinity Infinity 1|-Infinity -Infinity -Infinity -Infinity Infinity NaN -1|123456789 123456790 123456789 123456789 123456789.123 11111.111066090556 1|1e+300 1e+300 1e+300 1e+300 1e+300 1e+150 1|0 1 0 0 5e-324 2.2227587494850775e-162 1
directEqualsApplied0=0:
directEqualsApplied1=0:
maxMin0=-Infinity Infinity 3 3 2 1 -1 2 0 -0 0 -0 2.5 1.5 NaN NaN 3 16 NaN 0 9007199254740991 -9007199254740991 Infinity -Infinity 8 5 Infinity 1
maxMin1=-Infinity Infinity 3 3 2 1 -1 2 0 -0 0 -0 2.5 1.5 NaN NaN 3 16 NaN 0 9007199254740991 -9007199254740991 Infinity -Infinity 8 5 Infinity 1
conversions0=2 9 3 -7 3 4 -1 NaN NaN 0 1 0 2 NaN NaN 1 9 2.5|vsvsvs
conversions1=2 9 3 -7 3 4 -1 NaN NaN 0 1 0 2 NaN NaN 1 9 2.5|vsvsvs
throws0=TypeError,TypeError,TypeError,RangeError,Number,TypeError
throws1=TypeError,TypeError,TypeError,RangeError,Number,TypeError
aliases0=2,9,2,3,3,7,-2,3,2,3,4,10,2/3/-0,3
aliases1=2,9,2,3,3,7,-2,3,2,3,4,10,2/3/-0,3
patched0=patched1.5,1,1,2,getter,1|2
patched1=patched1.5,1,1,2,getter,1|2
thisAndNew0=1,2,1,2,floor,function,TypeError,TypeError
thisAndNew1=1,2,1,2,floor,function,TypeError,TypeError
inExpressions0=423,-1,true,Infinity,true,-Infinity,-Infinity,-Infinity,-Infinity,Infinity,-Infinity,-Infinity,-Infinity,Infinity,6,6
inExpressions1=423,-1,true,Infinity,true,-Infinity,-Infinity,-Infinity,-Infinity,Infinity,-Infinity,-Infinity,-Infinity,Infinity,6,6
argumentCounts0=3,-Infinity,1,10,0,96,200
argumentCounts1=3,-Infinity,1,10,0,96,200
generatorAndAsync0=1,7,9
generatorAndAsync1=1,7,9
trigAndPowEqualApplied0=0/2160:
trigAndPowEqualApplied1=0/2160:
floatMinMax0=1.5 0.5 0 0 -0 -0 -0 -0 2.5 2.5 NaN NaN -1e+308 1e+308 1e+21 -1e+21 1.5 0 NaN 0.5 0.25 1 0 1024 0.25 -8 1 1 Infinity 3909821048582988300 -Infinity 9 NaN NaN 0 -0 1 boolean:true NaN
floatMinMax1=1.5 0.5 0 0 -0 -0 -0 -0 2.5 2.5 NaN NaN -1e+308 1e+308 1e+21 -1e+21 1.5 0 NaN 0.5 0.25 1 0 1024 0.25 -8 1 1 Infinity 3909821048582988300 -Infinity 9 NaN NaN 0 -0 1 boolean:true NaN
trigConversionsAndPatches0=true,true,true,true,TypeError,TypeError,RangeError,patched,8,0,1,0,11|vvvvv
trigConversionsAndPatches1=true,true,true,true,TypeError,TypeError,RangeError,patched,8,0,1,0,11|vvvvv"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// A function that the interpreter runs in place of its call, in the loop
/// of the calling code, does what a call of it does: its arguments and
/// `this`, what it returns with and without a `return`, errors that pass
/// through several such functions to a handler or a `finally` of any of
/// them, iterators closed on the way out, closures, recursion, collections
/// that run while many are entered, and the functions that are not run in
/// place (generators, async and class code, callbacks of built-ins) calling
/// and being called by those that are.
#[test]
fn functions_run_in_place_of_their_calls_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(functions_run_in_place)
        .expect("thread")
        .join()
        .expect("checks");
}

fn functions_run_in_place() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = (e && e.constructor ? e.constructor.name : typeof e) + ":" + (e && e.message ? e.message.length : String(e)); }
    out.push(label + round + "=" + String(r));
  }
}
log("plain", function () { function add(a, b) { return a + b; } function twice(f, x) { return f(f(x, 1), 2); } var s = 0; for (var i = 0; i < 50; i++) s = add(s, i & 7); return s + "," + twice(add, 5); });
log("argumentCounts", function () { function f(a, b, c) { return [a, b, c].map(String).join("/"); } function g() { return "none"; } function h(a) { a = a + 1; return a; } return [f(), f(1), f(1, 2), f(1, 2, 3), f(1, 2, 3, 4, 5), g(1, 2, 3), g(), h(4), h()].join(); });
log("noReturn", function () { function f(a) { a + 1; } function g(a) { if (a) return; } function h() { } function k(a) { if (a) { return 5; } } return [f(1), g(1), g(0), h(), k(0), k(1), typeof f(2)].join(); });
log("recursion", function () { function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } function even(n) { return n === 0 ? true : odd(n - 1); } function odd(n) { return n === 0 ? false : even(n - 1); } function sum(n) { return n === 0 ? 0 : n + sum(n - 1); } return [fib(15), even(200), odd(201), sum(1000)].join(); });
log("throwAcrossFrames", function () {
  var trail = "";
  function c(x) { trail += "c"; if (x) throw new TypeError("deep"); return "ok"; }
  function b(x) { trail += "b"; try { return c(x); } finally { trail += "B"; } }
  function a(x) { trail += "a"; var r = b(x); trail += "A"; return r; }
  var r = [];
  r.push(a(0));
  try { a(1); } catch (e) { r.push(e.constructor.name + e.message); }
  function mid(x) { try { return a(x); } catch (e) { trail += "M"; return "caught"; } }
  r.push(mid(1), mid(0));
  function rethrow(x) { try { return a(x); } catch (e) { trail += "R"; throw e; } }
  try { rethrow(1); } catch (e) { r.push("outer" + e.message); }
  return r.join() + "|" + trail;
});
log("finallyOverrides", function () {
  function f() { try { return "try"; } finally { return "finally"; } }
  function g() { try { throw new Error("x"); } finally { return "swallowed"; } }
  function h(n) { for (var i = 0; i < 3; i++) { try { if (i === n) return "ret" + i; } finally { if (i === 1) continue; } } return "end"; }
  function thrower() { throw 7; }
  function k() { try { thrower(); } catch (e) { return "caught" + e; } finally { out.length; } }
  function nonError() { try { thrower(); } catch (e) { throw e + 1; } }
  var r = [f(), g(), h(0), h(1), h(2), h(9), k()];
  try { nonError(); } catch (e) { r.push(typeof e + e); }
  return r.join();
});
log("thisBinding", function () {
  "use sloppy";
  var o = { v: 1, m: function () { return this === o ? "o" : typeof this; }, n: function (x) { return this.v + x; } };
  function sloppy() { return this === undefined ? "undef" : this === globalThis ? "global" : typeof this; }
  function strict() { "use strict"; return this === undefined ? "undef" : typeof this; }
  var bare = o.m, r = [];
  r.push(o.m(), bare(), o.n(o.n(1)), sloppy(), strict(), sloppy.call(5), strict.call(5), sloppy.call(null), strict.call(null), o["m"](), (0, o.m)(), (o.m)());
  var arrow = { m: function () { return (() => this === arrow)(); } };
  r.push(arrow.m());
  function Ctor() { this.x = helper(2); function helper(n) { return n * 3; } }
  r.push(new Ctor().x);
  var nested = { a: { b: function () { return this === nested.a; } } };
  r.push(nested.a.b());
  return r.join();
});
log("nestedArguments", function () {
  var order = "";
  function t(x) { order += x; return x; }
  function f(a, b, c) { return a + "-" + b + "-" + c; }
  var o = { m: function (a, b) { return this.k + a + b; }, k: "K", g: function () { return this; } };
  var r = [f(t("1"), f(t("2"), t("3"), t("4")), t("5")), o.m(o.g().k, o.m(t("x"), t("y"))), o.g().g().m(f(1, 2, 3), "")];
  return r.join() + "|" + order;
});
log("closures", function () {
  function counter() { var n = 0; return function () { n++; return n; }; }
  var a = counter(), b = counter(); a(); a();
  function make(i) { return function (x) { return function (y) { return i + x + y; }; }; }
  var fs = []; for (let i = 0; i < 4; i++) fs.push(function () { return i * 2; });
  function outer(x) { function inner(y) { return x * y; } return inner(3) + inner(4); }
  return [a(), b(), make(1)(2)(3), fs.map(function (f) { return f(); }).join(""), outer(5)].join();
});
log("iteratorsAcrossCalls", function () {
  var closed = [];
  function source(name) { var i = 0; return { [Symbol.iterator]: function () { return this; }, next: function () { return { value: name + i, done: i++ >= 3 }; }, return: function () { closed.push(name); return {}; } }; }
  function firstOf(it) { for (var v of it) return v; }
  function findIn(it, wanted) { for (var v of it) { if (helper(v, wanted)) return v; } return "none"; }
  function helper(a, b) { return a === b; }
  function inner() { var r = ""; for (var k in { p: 1, q: 2 }) r += k; for (var c of "xy") r += c; return r; }
  var r = [];
  for (var v of source("a")) { r.push(v + inner()); if (v === "a1") break; }
  r.push(firstOf(source("b")), findIn(source("c"), "c2"), findIn(source("d"), "zz"));
  for (var w of [1, 2, 3]) { r.push(firstOf([w * 10, 99])); for (var u of [7]) r.push(helper(u, 7)); }
  function thrower(it) { for (var v of it) throw new Error("mid"); }
  try { thrower(source("e")); } catch (e) { r.push(e.message); }
  var [p, q] = [firstOf("hello"), inner()];
  r.push(p, q);
  return r.join() + "|" + closed.join();
});
log("collectionsDeep", function () {
  function build(n, acc) {
    var mine = { id: n, text: "node" + n, list: [n, n + 1, n + 2], inner: { v: n * 2 } };
    var junk = []; for (var i = 0; i < 40; i++) junk.push({ a: i, s: "j" + i + n });
    if (n === 0) return mine.id + mine.text + mine.list.length + mine.inner.v;
    var below = build(n - 1, mine);
    return below.length > 0 && mine.text === "node" + n && mine.list[2] === n + 2 && mine.inner.v === n * 2 && junk[39].s === "j39" + n && (!acc || acc.id === n + 1) ? below + "." + n : "BROKEN" + n;
  }
  var r = build(300).length;
  var keep = []; function grow(n) { var local = "s" + n; keep.push([local, { n: n }]); return n > 0 ? grow(n - 1) + local.length : 0; }
  var total = grow(400);
  return r + "," + total + "," + keep.length + "," + keep[0][0] + keep[400][1].n;
});
log("builtinsCallBack", function () {
  function sq(x) { return x * x; } function cmp(a, b) { return lowLevel(a) - lowLevel(b); } function lowLevel(x) { return -x; }
  function boom(x) { if (x === 3) throw new RangeError("three"); return x; }
  var r = [[1, 2, 3].map(sq).join(""), [3, 1, 2].sort(cmp).join(""), [1, 2, 3, 4].reduce(function (a, b) { return a + sq(b); }, 0), [1, 2].flatMap(function (x) { return [x, sq(x)]; }).join("")];
  try { [1, 2, 3, 4].forEach(function (x) { boom(x); }); } catch (e) { r.push(e.message); }
  r.push("abc".replace(/b/, function (m) { return sq(4) + m; }), JSON.stringify({ a: 1 }, function (k, v) { return typeof v === "number" ? sq(v + 1) : v; }), Array.from({ length: 3 }, function (_, i) { return sq(i); }).join(""));
  var m = new Map(); m.set("k", sq); r.push(m.get("k")(9));
  return r.join();
});
log("accessorsAndProxies", function () {
  var log2 = "";
  function note(x) { log2 += x; return x; }
  var o = { get g() { return note("g") + helper(); }, set s(v) { note("s" + v); }, valueOf: function () { return helper2(20); } };
  function helper() { return "H"; } function helper2(n) { return n + 1; }
  var p = new Proxy({}, { get: function (t, k) { return k === "f" ? function (a) { return note("p") + a; } : helper(); } });
  o.s = 3;
  return [o.g, o + 1, p.f(5), p.other, `${o}`].join() + "|" + log2;
});
log("classes", function () {
  class A { constructor(v) { this.v = init(v); } get() { return this.v; } static make(v) { return new A(twice(v)); } sum(n) { return n === 0 ? 0 : n + this.sum(n - 1); } }
  class B extends A { constructor(v) { super(v + 1); } get() { return super.get() * 2; } }
  function init(v) { return v + 100; } function twice(v) { return v * 2; }
  var r = [new A(1).get(), new B(1).get(), A.make(3).get(), new A(0).sum(50)];
  try { A(); } catch (e) { r.push(e.constructor.name); }
  function Old(v) { if (!(this instanceof Old)) return new Old(v); this.v = twice(v); }
  Old.prototype.get = function () { return this.v; };
  r.push(Old(4).get(), new Old(5).get());
  return r.join();
});
log("generatorsAndAsync", function () {
  function inc(x) { return x + 1; }
  function* g(n) { var x = inc(n); var got = yield inc(x); yield inc(got) + helper(got); return inc(100); }
  function helper(v) { return v * 2; }
  var it = g(1), r = [it.next().value, it.next(10).value, it.next().value, it.next().done];
  function caller() { var total = 0; for (var v of g(5)) total += v; return total; }
  r.push(caller());
  var seq = [];
  async function af(x) { seq.push("a" + inc(x)); var y = await inc(x); seq.push("b" + inc(y)); return inc(y); }
  af(1).then(function (v) { seq.push("then" + v); });
  r.push(seq.join(""));
  function throwsInGen() { function* t() { yield 1; fail(); } function fail() { throw new SyntaxError("gen"); } var i = t(); i.next(); try { i.next(); } catch (e) { return e.message; } }
  r.push(throwsInGen());
  return r.join();
});
log("applyBindCall", function () {
  function f(a, b) { return (this && this.tag ? this.tag : "none") + a + b; }
  function viaApply() { return f.apply({ tag: "T" }, [1, 2]); }
  var bound = f.bind({ tag: "B" }, 7);
  function useBound(x) { return bound(x); }
  return [viaApply(), f.call({ tag: "C" }, 3, 4), useBound(8), Reflect.apply(f, null, [5, 6]), [1, 2].map(f.bind(null, 0)).join("")].join();
});
log("labelsAndSwitch", function () {
  function classify(n) { switch (n % 4) { case 0: return "zero"; case 1: return name(n); default: break; } return "other"; }
  function name(n) { return "one" + n; }
  var r = "";
  outer: for (var i = 0; i < 6; i++) { for (var j = 0; j < 3; j++) { if (classify(i + j) === "zero") continue outer; if (i === 4) break outer; r += classify(i) + j + ";"; } }
  return r;
});
log("valuesReturned", function () {
  function id(x) { return x; }
  var s = Symbol("s"), o = {}, big = 2n ** 70n;
  var vals = [undefined, null, true, 0, -0, 1.5, NaN, "str", "a longer string value", s, o, big, id];
  var same = vals.every(function (v) { return Object.is(id(v), v) && Object.is(id(id(v)), v); });
  function many() { return [id(1), id("a"), id(null)]; }
  return same + "," + many().length + "," + typeof id(id) + "," + (1 / id(-0));
});
log("errorsInCalleeSetup", function () {
  var r = [];
  function notHere() { return missing + 1; }
  try { notHere(); } catch (e) { r.push(e.constructor.name); }
  function tdz() { return early; let early = 1; }
  try { tdz(); } catch (e) { r.push(e.constructor.name); }
  function callsNothing(f) { return f(); }
  try { callsNothing(5); } catch (e) { r.push(e.constructor.name); }
  try { callsNothing(undefined); } catch (e) { r.push(e.constructor.name); }
  function ok() { return "fine"; }
  r.push(callsNothing(ok));
  var caughtInside = (function () { try { return callsNothing(null); } catch (e) { return "in"; } finally { r.push("fin"); } })();
  r.push(caughtInside);
  return r.join();
});
log("manyCallsOneLoop", function () { function a(x) { return b(x) + 1; } function b(x) { return c(x) + 1; } function c(x) { return d(x) + 1; } function d(x) { return x & 3; } var t = 0; for (var i = 0; i < 2000; i++) t += a(i); return t; });
out.join("\n")
"#;
    let expected = r#"plain0=169,8
plain1=169,8
argumentCounts0=undefined/undefined/undefined,1/undefined/undefined,1/2/undefined,1/2/3,1/2/3,none,none,5,NaN
argumentCounts1=undefined/undefined/undefined,1/undefined/undefined,1/2/undefined,1/2/3,1/2/3,none,none,5,NaN
noReturn0=,,,,,5,undefined
noReturn1=,,,,,5,undefined
recursion0=610,true,true,500500
recursion1=610,true,true,500500
throwAcrossFrames0=ok,TypeErrordeep,caught,ok,outerdeep|abcBAabcBabcBMabcBAabcBR
throwAcrossFrames1=ok,TypeErrordeep,caught,ok,outerdeep|abcBAabcBabcBMabcBAabcBR
finallyOverrides0=finally,swallowed,ret0,end,ret2,end,caught7,number8
finallyOverrides1=finally,swallowed,ret0,end,ret2,end,caught7,number8
thisBinding0=o,object,3,global,undef,object,number,global,object,o,object,o,true,6,true
thisBinding1=o,object,3,global,undef,object,number,global,object,o,object,o,true,6,true
nestedArguments0=1-2-3-4-5,KKKxy,K1-2-3|12345xy
nestedArguments1=1-2-3-4-5,KKKxy,K1-2-3|12345xy
closures0=3,1,6,0246,35
closures1=3,1,6,0246,35
iteratorsAcrossCalls0=a0pqxy,a1pqxy,b0,c2,none,10,true,20,true,30,true,mid,h,pqxy|a,b,c,e
iteratorsAcrossCalls1=a0pqxy,a1pqxy,b0,c2,none,10,true,20,true,30,true,mid,h,pqxy|a,b,c,e
collectionsDeep0=1100,1492,401,s4000
collectionsDeep1=1100,1492,401,s4000
builtinsCallBack0=149,321,30,1124,three,a16bc,{"a":4},014,81
builtinsCallBack1=149,321,30,1124,three,a16bc,{"a":4},014,81
accessorsAndProxies0=gH,22,p5,H,[object Object]|s3gp
accessorsAndProxies1=gH,22,p5,H,[object Object]|s3gp
classes0=101,204,106,1275,TypeError,8,10
classes1=101,204,106,1275,TypeError,8,10
generatorsAndAsync0=3,31,101,true,NaN,a2,gen
generatorsAndAsync1=3,31,101,true,NaN,a2,gen
applyBindCall0=T12,C34,B78,none56,none01none02
applyBindCall1=T12,C34,B78,none56,none01none02
labelsAndSwitch0=one10;one11;one12;other0;other1;other0;one50;one51;one52;
labelsAndSwitch1=one10;one11;one12;other0;other1;other0;one50;one51;one52;
valuesReturned0=true,3,function,-Infinity
valuesReturned1=true,3,function,-Infinity
errorsInCalleeSetup0=ReferenceError,ReferenceError,TypeError,TypeError,fine,fin,in
errorsInCalleeSetup1=ReferenceError,ReferenceError,TypeError,TypeError,fine,fin,in
manyCallsOneLoop0=9000
manyCallsOneLoop1=9000"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// `object.name = value` assigned with the object on the stack, and
/// `this.name` read by the instruction that fetches the this value, do what
/// a Reference does: for every kind of base (objects of changing shapes,
/// frozen and sealed ones, accessors, Proxies, arrays and other exotic
/// objects, primitives, null and undefined), in the order the object, the
/// value and the assignment are evaluated, and for every kind of `this`.
#[test]
fn named_assignment_and_this_properties_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(named_assignment_and_this)
        .expect("thread")
        .join()
        .expect("checks");
}

fn named_assignment_and_this() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = (e && e.constructor ? e.constructor.name : typeof e) + ":" + (e && e.message ? e.message.length : String(e)); }
    out.push(label + round + "=" + String(r));
  }
}
log("plainAssign", function () { var o = { a: 1 }; o.a = 2; o.b = 3; o.c = o.a + o.b; var p = {}; p.x = 1; p.y = 2; p.x = p.y; return JSON.stringify(o) + JSON.stringify(p) + Object.keys(o).join(""); });
log("loopAssign", function () { var o = { x: 0, y: 2, z: 3 }; for (var i = 0; i < 50; i++) { o.x = o.x + o.y + (o.z & 1); } var q = { n: 0 }; for (var j = 0; j < 10; j++) { q.n = j; q["k" + j] = j; q.last = q.n * 2; } return o.x + "," + q.n + "," + q.last + "," + Object.keys(q).length; });
log("order", function () {
  var trail = "";
  function obj(name) { trail += "o" + name; return holder; } function rhs(v) { trail += "r" + v; return v; }
  var holder = { set s(v) { trail += "S" + v; }, get g() { trail += "G"; return holder; } };
  obj(1).a = rhs(1); obj(2).s = rhs(2); holder.g.b = rhs(3); obj(4).g.c = obj(5).g.d = rhs(6);
  return trail + "|" + [holder.a, holder.b, holder.c, holder.d].join();
});
log("nullBase", function () {
  var trail = "", r = [];
  function rhs() { trail += "R"; return 1; }
  var n = null, u;
  try { n.x = rhs(); } catch (e) { r.push(e.constructor.name); }
  try { u.x = rhs(); } catch (e) { r.push(e.constructor.name); }
  try { missing.x = rhs(); } catch (e) { r.push(e.constructor.name); }
  function thrower() { throw new RangeError("rhs"); }
  var o = { v: 1 }; try { o.v = thrower(); } catch (e) { r.push(e.constructor.name + o.v); }
  try { n.x = thrower(); } catch (e) { r.push(e.constructor.name); }
  return r.join() + "|" + trail;
});
log("primitiveBase", function () {
  var r = [], s = "str", n = 5, b = true, sym = Symbol("q");
  s.x = 1; n.y = 2; b.z = 3; sym.w = 4; r.push(s.x, n.y, b.z, sym.w);
  function strict() { "use strict"; var t = "t"; try { t.x = 1; return "no"; } catch (e) { return e.constructor.name; } }
  r.push(strict());
  Object.defineProperty(Number.prototype, "viaSetter", { set: function (v) { r.push(typeof this + v); }, configurable: true });
  n.viaSetter = 9; delete Number.prototype.viaSetter;
  return r.map(String).join();
});
log("unwritable", function () {
  var o = Object.freeze({ a: 1 }), p = Object.defineProperty({}, "ro", { value: 1, writable: false }), sealed = Object.seal({ s: 1 }), noext = Object.preventExtensions({ k: 1 });
  o.a = 2; o.b = 3; p.ro = 5; sealed.s = 2; sealed.t = 3; noext.k = 2; noext.m = 3;
  var child = Object.create(p); child.ro = 7;
  var r = [o.a, o.b, p.ro, sealed.s, sealed.t, noext.k, noext.m, child.ro, Object.keys(child).length];
  function strict() { "use strict"; var e1, e2, e3; try { o.a = 2; } catch (e) { e1 = e.constructor.name; } try { noext.m = 1; } catch (e) { e2 = e.constructor.name; } try { child.ro = 1; } catch (e) { e3 = e.constructor.name; } return [e1, e2, e3].join("/"); }
  r.push(strict());
  return r.map(String).join();
});
log("accessors", function () {
  var trail = "";
  var o = { _v: 1, get v() { trail += "g"; return this._v; }, set v(x) { trail += "s" + x; this._v = x * 2; }, get onlyGet() { return 7; } };
  o.v = 5; o.v = o.v + 1; o.onlyGet = 9;
  var child = Object.create(o); child.v = 3; child.own = 4;
  var setterThrows = { set t(x) { throw new SyntaxError("set"); } }, caught = "";
  try { setterThrows.t = 1; } catch (e) { caught = e.constructor.name; }
  function strict() { "use strict"; try { o.onlyGet = 1; return "no"; } catch (e) { return e.constructor.name; } }
  return [o._v, o.onlyGet, child._v, Object.keys(child).join(""), caught, strict()].join() + "|" + trail;
});
log("proxyAndExotic", function () {
  var seen = [];
  var p = new Proxy({}, { set: function (t, k, v, r) { seen.push(String(k) + v + (r === p)); t[k] = v; return true; }, defineProperty: function (t, k, d) { seen.push("def" + String(k)); return Reflect.defineProperty(t, k, d); } });
  p.a = 1; p.b = p.a;
  var refuses = new Proxy({}, { set: function () { return false; } }); refuses.x = 1;
  var arr = [1, 2, 3, 4]; arr.length = 2; arr.extra = "e";
  function f() {} f.prototype = { marker: 1 }; f.custom = 5;
  var ta = new Uint8Array(2); ta.name = "n";
  var d = new Date(0); d.tag = "t";
  var m = new Map(); m.size = 9; var str = new String("ab"); str.length = 5; str.more = 1;
  function strictRefuse() { "use strict"; try { refuses.x = 1; return "no"; } catch (e) { return e.constructor.name; } }
  return [seen.join("/"), arr.join(""), arr.extra, new f().marker, f.custom, ta.name, d.tag, m.size, str.length, str.more, strictRefuse()].join();
});
log("globals", function () { globalThis.gA = 1; var g = globalThis; g.gB = gA + 1; this.gC = 3; return [gA, gB, typeof gC, delete globalThis.gA, delete g.gB, typeof gA].join(); });
log("chained", function () { var a = { b: { c: {} } }, x = {}, y = {}; a.b.c.d = 1; a.b.e = a.b.c.d + 1; x.p = y.p = a.b.e; var z = (x.q = 5); var arr = [{}]; arr[0].v = 9; return [a.b.c.d, a.b.e, x.p, y.p, z, x.q, arr[0].v].join(); });
log("shapes", function () {
  function make(i) { var o = {}; if (i & 1) o.a = i; o.b = i * 2; if (i & 2) o.c = i; o.b = o.b + 1; return o; }
  var r = []; for (var i = 0; i < 8; i++) { var o = make(i); o.z = i; delete o.b; o.b = "again"; r.push(Object.keys(o).join("") + o.b.length); }
  var proto = { inherited: 1 }, c = Object.create(proto); c.inherited = 2; proto.later = 5; c.later = c.later + 1;
  var dict = {}; for (var k = 0; k < 40; k++) { dict["p" + k] = k; } dict.p5 = "five"; dict.fresh = 1;
  return r.join() + "|" + proto.inherited + c.inherited + proto.later + c.later + "|" + dict.p5 + dict.fresh + Object.keys(dict).length;
});
log("thisInMethods", function () {
  var o = { v: 3, get: function () { return this.v; }, inc: function (by) { this.v = this.v + by; return this; }, twice: function () { return this.get() + this.get(); }, missing: function () { return typeof this.nope; } };
  class C { constructor(v) { this.v = v; this.w = this.v * 2; } sum() { return this.v + this.w; } static make(v) { return new this(v); } get dbl() { return this.v * 2; } set dbl(x) { this.v = x / 2; } chain() { return this.sum() + this.dbl; } }
  var c = C.make(4); c.dbl = 20;
  return [o.get(), o.inc(2).inc(3).v, o.twice(), o.missing(), c.sum(), c.dbl, c.chain(), new C(1).sum()].join();
});
log("thisKinds", function () {
  "use sloppy";
  var r = [];
  function sloppy() { return typeof this.Math + (this === globalThis); } function strict() { "use strict"; try { return this.x; } catch (e) { return e.constructor.name; } }
  r.push(sloppy(), strict(), strict.call({ x: "sx" }), strict.call(7) === undefined, strict.call("abc"));
  Number.prototype.probe = function () { "use strict"; return typeof this + this.toFixed(1); }; r.push((5).probe()); delete Number.prototype.probe;
  String.prototype.len2 = function () { return this.length * 2; }; r.push("abc".len2()); delete String.prototype.len2;
  var arrow = { v: 9, m: function () { var f = () => this.v + 1; var g = () => () => this.v + 2; return f() + g()(); } }; r.push(arrow.m());
  var topArrow = () => typeof this; r.push(topArrow());
  function Ctor() { this.a = 1; this.b = this.a + 1; } r.push(new Ctor().b);
  class Base { constructor() { this.k = "base"; } who() { return this.k; } } class Derived extends Base { constructor(early) { if (early) { try { this.k; } catch (e) { r.push(e.constructor.name); } } super(); this.k = this.k + "+derived"; } who() { return super.who() + "!"; } }
  r.push(new Derived(true).who(), new Derived(false).who());
  function* gen() { yield this.v; this.v = this.v + 1; yield this.v; } var holder = { v: 1, gen: gen }; r.push([...holder.gen()].join(""), holder.v);
  var getterObj = { get g() { return this.h; }, h: "hh", m: function () { return this.g; } }; r.push(getterObj.m());
  var p = new Proxy({ real: 1 }, { get: function (t, k, rcv) { return k === "viaThis" ? function () { return this.real; } : Reflect.get(t, k, rcv); } }); r.push(p.viaThis());
  return r.map(String).join();
});
log("thisCalls", function () {
  var trail = "";
  var o = { n: 0, a: function () { trail += "a"; return this.b(1) + this.c(); }, b: function (x) { trail += "b"; this.n = this.n + x; return this.n; }, c: function () { trail += "c"; return this.missing ? 1 : this.n; } };
  var r = [o.a(), o.a()];
  var detached = o.b; try { detached(1); r.push("sloppy" + typeof n); } catch (e) { r.push(e.constructor.name); }
  var notFn = { m: function () { return this.nf(); }, nf: 5 }; try { notFn.m(); } catch (e) { r.push(e.constructor.name); }
  var swapped = { m: function () { var v = this.f(); this.f = function () { return "second"; }; return v + this.f(); }, f: function () { return "first"; } }; r.push(swapped.m(), swapped.m());
  return r.join() + "|" + trail;
});
log("localReads", function () {
  var r = [], n = null, u, num = 5, str = "abc", sym = Symbol("s"), big = 3n, fn = function named(a, b) {}, arr = [1, 2], date = new Date(0);
  try { r.push(n.x); } catch (e) { r.push(e.constructor.name); } try { r.push(u.x); } catch (e) { r.push(e.constructor.name); } try { n.m(); } catch (e) { r.push(e.constructor.name); } try { u.m(); } catch (e) { r.push(e.constructor.name); }
  r.push(num.x, typeof num.toFixed, str.length, str.x, typeof sym.description, typeof big.toString, fn.name, fn.length, typeof fn.call, fn.x, arr.length, typeof arr.push, arr.x, typeof date.getTime, date.x);
  try { str.missing(); } catch (e) { r.push(e.constructor.name); } try { num.length.x; } catch (e) { r.push(e.constructor.name); }
  var calls = "", acc = { get g() { calls += "g"; return this === acc; }, m: function () { calls += "m"; return this === acc; } }, inh = Object.create(acc), px = new Proxy({ real: 1 }, { get: function (t, k, rcv) { calls += "p"; return k === "self" ? rcv === px : t[k]; } });
  r.push(acc.g, acc.m(), inh.g === false, inh.m() === false, px.real, px.self, px.nope);
  var o = { a: 1 }; r.push(o.a); o.b = 2; r.push(o.a + o.b); delete o.a; r.push(o.a); Object.defineProperty(o, "b", { get: function () { return "getter"; } }); r.push(o.b); Object.setPrototypeOf(o, { c: "proto" }); r.push(o.c, o.d);
  function swap(x) { return x.k; } r.push(swap({ k: 1 }), swap({ j: 0, k: 2 }), swap([]), swap("s"), swap(7), swap(function () {}), swap(Object.create({ k: "up" })));
  return r.map(String).join() + "|" + calls;
});
log("addThenPrototypeChanges", function () {
  function make(v) { var o = {}; o.fresh = v; return o; }
  var a = make(1), b = make(2), seen = [], r = [];
  Object.defineProperty(Object.prototype, "fresh", { set: function (v) { seen.push("s" + v); }, get: function () { return "proto"; }, configurable: true });
  var c = make(3); r.push(a.fresh, b.fresh, c.fresh, Object.keys(c).length);
  delete Object.prototype.fresh; var d = make(4); r.push(d.fresh, Object.keys(d).length);
  Object.defineProperty(Object.prototype, "fresh", { value: "ro", writable: false, configurable: true }); var e = make(5); r.push(e.fresh, Object.keys(e).length);
  function strictMake(v) { "use strict"; var o = {}; try { o.fresh = v; return "added"; } catch (err) { return err.constructor.name; } } r.push(strictMake(6)); delete Object.prototype.fresh; r.push(strictMake(7));
  var proto = {}; function Ctor(v) { this.k = v; } Ctor.prototype = proto; var x = new Ctor(1);
  Object.defineProperty(proto, "k", { set: function (v) { seen.push("p" + v); }, configurable: true }); var y = new Ctor(2); r.push(x.k, y.k, Object.keys(y).length);
  delete proto.k; var z = new Ctor(3); r.push(z.k); Object.setPrototypeOf(proto, { set k(v) { seen.push("g" + v); } }); var w = new Ctor(4); r.push(w.k, Object.keys(w).length);
  return r.map(String).join() + "|" + seen.join();
});
log("addInConstructors", function () {
  class P { constructor(x, y) { this.x = x; this.y = y; this.sum = this.x + this.y; } } class Q extends P { constructor(x, y, z) { super(x, y); this.z = z; this.x = "over"; } }
  var ps = []; for (var i = 0; i < 6; i++) ps.push(i % 2 ? new P(i, i * 2) : new Q(i, i * 2, "z" + i));
  function Old(n) { this.a = n; if (n & 1) this.odd = true; this.b = n * 2; if (n > 2) { this.c = 1; this.d = 2; } }
  var olds = []; for (var j = 0; j < 5; j++) olds.push(Object.keys(new Old(j)).join(""));
  function Wide() { for (var k = 0; k < 3; k++) { this.p0 = k; this.p1 = k; this.p2 = k; this.p3 = k; this.p4 = k; this.p5 = k; this.p6 = k; this.p7 = k; this.p8 = k; this.p9 = k; this.p10 = k; this.p11 = k; this.p12 = k; this.p13 = k; this.p14 = k; this.p15 = k; this.p16 = k; this.p17 = k; this.p18 = k; } }
  var wide = new Wide();
  function Frozen() { Object.preventExtensions(this); this.late = 1; } function Sealing() { this.first = 1; Object.seal(this); this.second = 2; this.first = 3; }
  var holder = { set viaSetter(v) { this._v = v; } }; function Inherits() { this.viaSetter = 9; this.own = 1; } Inherits.prototype = holder;
  var prox = new Proxy({}, { set: function (t, k, v) { t[k] = v + "!"; return true; } }); function IntoProxy(o) { o.added = "a"; return o.added; }
  function OnArray() { var arr = [1]; arr.tag = "t"; arr.extra = 2; return Object.keys(arr).join(""); } function OnFunction() { var f = function () {}; f.tag = 1; f.other = 2; return Object.keys(f).join(""); }
  return [ps.map(function (p) { return Object.keys(p).join("") + p.x + p.sum; }).join(), olds.join(), Object.keys(wide).length, wide.p18, Object.keys(new Frozen()).length, Object.keys(new Sealing()).join("") + new Sealing().first, Object.keys(new Inherits()).join(""), new Inherits()._v, IntoProxy(prox), OnArray(), OnFunction()].join("|");
});
out.join("\n")
"#;
    let expected = r#"plainAssign0={"a":2,"b":3,"c":5}{"x":2,"y":2}abc
plainAssign1={"a":2,"b":3,"c":5}{"x":2,"y":2}abc
loopAssign0=150,9,18,12
loopAssign1=150,9,18,12
order0=o1r1o2r2S2Gr3o4Go5Gr6|1,3,6,6
order1=o1r1o2r2S2Gr3o4Go5Gr6|1,3,6,6
nullBase0=TypeError,TypeError,ReferenceError,RangeError1,RangeError|RR
nullBase1=TypeError,TypeError,ReferenceError,RangeError1,RangeError|RR
primitiveBase0=undefined,undefined,undefined,undefined,TypeError,object9
primitiveBase1=undefined,undefined,undefined,undefined,TypeError,object9
unwritable0=1,undefined,1,2,undefined,2,undefined,1,0,TypeError/TypeError/TypeError
unwritable1=1,undefined,1,2,undefined,2,undefined,1,0,TypeError/TypeError/TypeError
accessors0=22,7,6,_vown,SyntaxError,TypeError|s5gs11s3
accessors1=22,7,6,_vown,SyntaxError,TypeError|s5gs11s3
proxyAndExotic0=a1true/b1true,12,e,1,5,n,t,0,2,1,TypeError
proxyAndExotic1=a1true/b1true,12,e,1,5,n,t,0,2,1,TypeError
globals0=1,2,number,true,true,undefined
globals1=1,2,number,true,true,undefined
chained0=1,2,2,2,5,5,9
chained1=1,2,2,2,5,5,9
shapes0=zb5,azb5,czb5,aczb5,zb5,azb5,czb5,aczb5|1256|five141
shapes1=zb5,azb5,czb5,aczb5,zb5,azb5,czb5,aczb5|1256|five141
thisInMethods0=3,8,16,undefined,18,20,38,3
thisInMethods1=3,8,16,undefined,18,20,38,3
thisKinds0=objecttrue,TypeError,sx,true,undefined,number5.0,6,21,object,2,ReferenceError,base+derived!,base+derived!,12,2,hh,1
thisKinds1=objecttrue,TypeError,sx,true,undefined,number5.0,6,21,object,2,ReferenceError,base+derived!,base+derived!,12,2,hh,1
thisCalls0=2,4,sloppynumber,TypeError,firstsecond,secondsecond|abcabcb
thisCalls1=2,4,sloppynumber,TypeError,firstsecond,secondsecond|abcabcb
localReads0=TypeError,TypeError,TypeError,TypeError,undefined,function,3,undefined,string,function,named,2,function,undefined,2,function,undefined,function,undefined,TypeError,TypeError,true,true,true,true,1,true,undefined,1,3,undefined,getter,proto,undefined,1,2,undefined,undefined,undefined,undefined,up|gmgmppp
localReads1=TypeError,TypeError,TypeError,TypeError,undefined,function,3,undefined,string,function,named,2,function,undefined,2,function,undefined,function,undefined,TypeError,TypeError,true,true,true,true,1,true,undefined,1,3,undefined,getter,proto,undefined,1,2,undefined,undefined,undefined,undefined,up|gmgmppp
addThenPrototypeChanges0=1,2,proto,0,4,1,ro,0,TypeError,added,1,undefined,0,3,undefined,0|s3,p2,g4
addThenPrototypeChanges1=1,2,proto,0,4,1,ro,0,TypeError,added,1,undefined,0,3,undefined,0|s3,p2,g4
addInConstructors0=xysumzover0,xysum13,xysumzover6,xysum39,xysumzover12,xysum515|ab,aoddb,ab,aoddbcd,abcd|19|2|0|first3|_vown|9|a!|0tagextra|tagother
addInConstructors1=xysumzover0,xysum13,xysumzover6,xysum39,xysumzover12,xysum515|ab,aoddb,ab,aoddbcd,abcd|19|2|0|first3|_vown|9|a!|0tagextra|tagother"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// A `for` loop with its test at the end, and the instructions that compare
/// a local with a constant or another local and jump, with `++` or `--`
/// before where that is the loop's increment, do what the test before the
/// body and its separate instructions do: for counters and bounds that are
/// not integers or change in the body, the largest integers, `continue`,
/// `break` and `return`, bounds read from properties, and generators.
#[test]
fn loops_and_tests_on_locals_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(loops_and_tests_on_locals)
        .expect("thread")
        .join()
        .expect("checks");
}

fn loops_and_tests_on_locals() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = (e && e.constructor ? e.constructor.name : typeof e) + ":" + (e && e.message ? e.message.length : String(e)); }
    out.push(label + round + "=" + String(r));
  }
}
log("basic", function () { var s = ""; for (var i = 0; i < 5; i++) s += i; for (var j = 5; j > 0; j--) s += j; for (var k = 0; k <= 3; k++) s += k; for (var m = 3; m >= 0; m--) s += m; for (var z = 0; z < 0; z++) s += "never"; for (var y = 3; y > 3; y--) s += "never"; return s + "|" + [i, j, k, m, z, y].join(); });
log("localBound", function () { var n = 4, s = ""; for (var i = 0; i < n; i++) { s += i; if (i === 1) n = 6; } var lo = 2; for (var j = 8; j >= lo; j--) { s += j; if (j === 5) lo = 4; } return s + "|" + i + j; });
log("continueBreak", function () {
  var s = "";
  for (var i = 0; i < 10; i++) { if (i === 2) continue; if (i === 6) break; s += i; }
  outer: for (var a = 0; a < 4; a++) { for (var b = 0; b < 4; b++) { if (b === 2) continue outer; if (a === 3) break outer; s += a + "" + b + ","; } }
  for (var c = 0; c < 3; c++) { try { if (c === 1) continue; s += "t" + c; } finally { s += "f" + c; } }
  function ret() { for (var q = 0; q < 5; q++) { if (q === 3) return "r" + q; } return "end"; }
  return s + ret() + "|" + [i, a, b, c].join();
});
log("updates", function () {
  var s = "", t = "";
  for (var i = 0; i < 10; i += 3) s += i; for (var j = 0; j < 4; ++j) s += j; for (var k = 4; k > 0; --k) s += k; for (var m = 0, n = 5; m < n; m++, n--) s += m + "" + n;
  function bump() { t += "u"; return 1; } for (var p = 0; p < 3; p += bump()) s += p; for (var q = 0; q < 3;) { s += q; q++; } for (var r = 1; r < 40; r *= 3) s += r;
  var o = { c: 0 }; for (o.c = 0; o.c < 3; o.c++) s += o.c;
  return s + "|" + t;
});
log("bodyChangesCounter", function () { var s = ""; for (var i = 0; i < 10; i++) { s += i; if (i === 2) i = 6; if (i === 8) i = "9"; } for (var j = 0; j < 5; j++) { if (j === 1) j += 1.5; s += "," + j; } return s + "|" + typeof i + i + "|" + j; });
log("nonIntegers", function () {
  var s = [];
  var n = 0; for (var f = 0.5; f < 3; f++) n++; s.push(n, f);
  n = 0; for (var g = 3; g > 0.5; g--) n++; s.push(n, g);
  n = 0; for (var str = "0"; str < 3; str++) n++; s.push(n, typeof str + str);
  n = 0; for (var st = "a"; st < "d" && n < 5; st += "b") n++; s.push(n, st);
  n = 0; for (var big = 0n; big < 3n; big++) n++; s.push(n, typeof big);
  n = 0; for (var u; u < 3; u++) n++; s.push(n, String(u));
  n = 0; for (var nan = NaN; nan <= 5; nan++) n++; s.push(n);
  n = 0; for (var nul = null; nul < 2; nul++) n++; s.push(n, nul);
  n = 0; for (var bo = true; bo < 3; bo++) n++; s.push(n, bo);
  n = 0; for (var inf = 0; inf < Infinity && n < 4; inf++) n++; s.push(n);
  n = 0; for (var neg = -0; neg >= -2; neg--) n++; s.push(n, neg);
  return s.join();
});
log("objectsInTest", function () {
  var calls = "";
  var a = { valueOf: function () { calls += "a"; return 0; } }, b = { valueOf: function () { calls += "b"; return 2; } };
  var n = 0; for (var i = a; i < b; i++) n++;
  var c = { valueOf: function () { calls += "c"; return 2; } }; for (var j = 3; j > c; j--) n++;
  var bounds = { get hi() { calls += "g"; return 2; } }; for (var k = 0; k < bounds.hi; k++) n++;
  var arr = [1, 2, 3]; for (var m = 0; m < arr.length; m++) { n++; if (m === 0) arr.push(4); }
  try { for (var s = Symbol(); s < 1; s++) n++; } catch (e) { calls += e.constructor.name; }
  try { for (var t = 0; t < 2n ** 70n && t < 2; t++) n++; } catch (e) { calls += e.constructor.name; }
  return n + "," + typeof i + i + "," + j + "|" + calls;
});
log("largeIntegers", function () {
  var s = [], n = 0;
  for (var i = 9007199254740989; i < 9007199254740992; i++) n++; s.push(n, i);
  n = 0; for (var j = 9007199254740990; j <= 9007199254740993 && n < 6; j++) n++; s.push(n, j);
  n = 0; for (var k = -9007199254740990; k >= -9007199254740993 && n < 6; k--) n++; s.push(n, k);
  n = 0; for (var m = 2147483646; m < 2147483650; m++) n++; s.push(n, m);
  n = 0; for (var p = -2147483647; p > -2147483651; p--) n++; s.push(n, p);
  return s.join();
});
log("equalityTests", function () { var s = ""; for (var i = 0; i !== 4; i++) s += i; for (var j = 3; j != 0; j--) s += j; var stop = 2; for (var k = 0; k === 0 || k < stop; k++) s += "k" + k; for (var m = "a"; m !== "aaaa"; m += "a") s += m.length; var u; for (var q = 0; u == null && q < 3; q++) { s += "q"; if (q === 1) u = 0; } return s; });
log("thisAndProperties", function () {
  var o = { count: 3, items: [7, 8, 9], run: function () { var s = ""; for (var i = 0; i < this.count; i++) s += this.items[i]; for (var j = this.count; j > 0; j--) { s += j; if (j === 2) this.count = 5; } return s + this.count; } };
  var limits = { a: { n: 2 } }, r = ""; for (var k = 0; k <= limits.a.n; k++) r += k;
  return o.run() + "|" + r;
});
log("letAndClosures", function () {
  var fs = []; for (let i = 0; i < 3; i++) fs.push(function () { return i; });
  var t = 0; for (let j = 0; j < 4; j++) t += j; for (const k = 0; t < 8; ) t++;
  var caught = ""; try { for (let m = 0; m < 2; m++) { const c = m; c++; } } catch (e) { caught = e.constructor.name; }
  var outerVar = 10; function usesOuter() { var s = 0; for (var i = 0; i < outerVar; i++) s += i; for (var j = outerVar; j > 7; j--) s += j; return s; }
  return fs.map(function (f) { return f(); }).join("") + "," + t + "," + caught + "," + usesOuter();
});
log("generatorsAndTry", function () {
  function* g(n) { for (var i = 0; i < n; i++) { var got = yield i; if (got) i += got; } return "done"; }
  var it = g(6), r = [it.next().value, it.next().value, it.next(2).value, it.next().value, it.next().done];
  var caught = ""; function thrower(n) { for (var i = 0; i < n; i++) { if (i === 2) throw new RangeError("at" + i); } }
  try { thrower(5); } catch (e) { caught = e.message; }
  var fin = ""; for (var k = 0; k < 3; k++) { try { if (k === 1) throw 1; fin += "b"; } catch (e) { fin += "c"; continue; } finally { fin += "f"; } fin += "e"; }
  async function af() { var s = 0; for (var i = 0; i < 3; i++) s += await i; return s; } var settled = "p"; af().then(function (v) { settled = v; });
  return r.join() + "|" + caught + "|" + fin;
});
log("nested", function () { var s = 0, t = ""; for (var i = 0; i < 4; i++) for (var j = i; j < 4; j++) for (var k = 0; k <= j; k++) s += k; for (var a = 0; a < 3; a++) { var b = a; while (b > 0) { b--; t += b; } do { t += "d"; } while (t.length < a); } return s + "|" + t; });
log("whileForms", function () { var i = 0, s = ""; while (i < 5) { i++; if (i === 2) continue; if (i === 4) break; s += i; } var j = 5; while (j-- > 0) s += j; var k = 0; do { s += k; k++; } while (k < 3); var n = 3; while (n !== 0) n--; return s + n; });
log("ifTests", function () { function cls(a, b) { var r = ""; if (a < b) r += "lt"; if (a <= b) r += "le"; if (a > b) r += "gt"; if (a >= b) r += "ge"; if (a === b) r += "eq"; if (a !== b) r += "ne"; if (a < 3) r += "<3"; if (a >= 3) r += ">=3"; if (a === 3) r += "=3"; if (a !== "3") r += "!s3"; return r; } var vals = [1, 3, 3.5, "3", NaN, null, undefined, -0, 0, 2n, "x", true, {}], out2 = []; for (var i = 0; i < vals.length; i++) for (var j = 0; j < vals.length; j += 3) { try { out2.push(cls(vals[i], vals[j])); } catch (e) { out2.push("T"); } } return out2.join(","); });
out.join("\n")
"#;
    let expected = r#"basic0=012345432101233210|5,0,4,-1,0,3
basic1=012345432101233210|5,0,4,-1,0,3
localBound0=01234587654|63
localBound1=01234587654|63
continueBreak0=0134500,01,10,11,20,21,t0f0f1t2f2r3|6,3,0,3
continueBreak1=0134500,01,10,11,20,21,t0f0f1t2f2r3|6,3,0,3
updates0=03690123432105142301201213927012|uuu
updates1=03690123432105142301201213927012|uuu
bodyChangesCounter0=01278,0,2.5,3.5,4.5|number10|5.5
bodyChangesCounter1=01278,0,2.5,3.5,4.5|number10|5.5
nonIntegers0=3,3.5,3,0,3,number3,5,abbbbb,3,bigint,0,undefined,0,2,2,2,3,4,3,-3
nonIntegers1=3,3.5,3,0,3,number3,5,abbbbb,3,bigint,0,undefined,0,2,2,2,3,4,3,-3
objectsInTest0=11,number2,2|ababbccgggTypeError
objectsInTest1=11,number2,2|ababbccgggTypeError
largeIntegers0=3,9007199254740992,6,9007199254740992,6,-9007199254740992,4,2147483650,4,-2147483651
largeIntegers1=3,9007199254740992,6,9007199254740992,6,-9007199254740992,4,2147483650,4,-2147483651
equalityTests0=0123321k0k1123qq
equalityTests1=0123321k0k1123qq
thisAndProperties0=7893215|012
thisAndProperties1=7893215|012
letAndClosures0=012,8,TypeError,72
letAndClosures1=012,8,TypeError,72
generatorsAndTry0=0,1,4,5,true|at2|bfecfbfe
generatorsAndTry1=0,1,4,5,true|at2|bfecfbfe
nested0=35|d0d10d
nested1=35|d0d10d
whileForms0=13432100120
whileForms1=13432100120
ifTests0=legeeq<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,gtgene>=3=3!s3,legene>=3=3!s3,ne>=3=3!s3,gtgene>=3=3!s3,ne>=3=3!s3,gtgene>=3!s3,gtgene>=3!s3,ne>=3!s3,gtgene>=3!s3,ne>=3!s3,gtgene>=3,legeeq>=3,ne>=3,gtgene>=3,ltlene>=3,ne!s3,ne!s3,ne!s3,ne!s3,ne!s3,ltlene<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,ne!s3,ne!s3,eq!s3,ne!s3,ne!s3,ltlene<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,gtgene<3!s3,ltlene<3!s3,ne<3!s3,legeeq<3!s3,ne<3!s3,ne!s3,gtgene!s3,ne!s3,ne!s3,gtgene!s3,legene<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,ne!s3,gtgene!s3,ne!s3,ne!s3,legeeq!s3
ifTests1=legeeq<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,gtgene>=3=3!s3,legene>=3=3!s3,ne>=3=3!s3,gtgene>=3=3!s3,ne>=3=3!s3,gtgene>=3!s3,gtgene>=3!s3,ne>=3!s3,gtgene>=3!s3,ne>=3!s3,gtgene>=3,legeeq>=3,ne>=3,gtgene>=3,ltlene>=3,ne!s3,ne!s3,ne!s3,ne!s3,ne!s3,ltlene<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,ne!s3,ne!s3,eq!s3,ne!s3,ne!s3,ltlene<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,gtgene<3!s3,ltlene<3!s3,ne<3!s3,legeeq<3!s3,ne<3!s3,ne!s3,gtgene!s3,ne!s3,ne!s3,gtgene!s3,legene<3!s3,ltlene<3!s3,ne<3!s3,ltlene<3!s3,ne<3!s3,ne!s3,gtgene!s3,ne!s3,ne!s3,legeeq!s3"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The length of a String value, the methods a String or a Number value
/// finds on the prototype object of its kind by the property cache, and
/// `charCodeAt` computed in place of its call, are what the general lookup
/// and the call give: for strings of every kind of code unit, positions of
/// every kind, other this values, methods and accessors put on the
/// prototypes and taken off again, and assignments that find a property
/// that is not writable.
#[test]
fn properties_of_strings_and_numbers_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(properties_of_strings_and_numbers)
        .expect("thread")
        .join()
        .expect("checks");
}

fn properties_of_strings_and_numbers() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function log(label, f) {
  for (var round = 0; round < 2; round++) {
    var r;
    try { r = f(); } catch (e) { r = (e && e.constructor ? e.constructor.name : typeof e) + ":" + (e && e.message ? e.message.length : String(e)); }
    out.push(label + round + "=" + String(r));
  }
}
var samples = ["", "a", "abcdefg", "abcdefgh", "a longer string of text", "ж", "жжжжжжжж", "中文字符串和更多的字", "😀", "a😀b😀c", "\ud83d", "x\ude00y\ud83d", "tab\tnew\nnul\0end", "a" + "b".repeat(40)];
log("length", function () { var r = []; for (var i = 0; i < samples.length; i++) { var s = samples[i]; r.push(s.length); } var t = "ab" + "cd"; r.push(t.length, ("x" + 1).length, `t${t}`.length, String(12345).length, new String("abc").length, Object("zz").length); return r.join(); });
log("lengthIsOwn", function () { var s = "abc", r = []; s.length = 9; r.push(s.length); r.push(Object.getOwnPropertyDescriptor(s, "length").writable, "length" in Object(s), s.hasOwnProperty("length")); function strict() { "use strict"; try { s.length = 1; return "no"; } catch (e) { return e.constructor.name; } } r.push(strict()); Object.defineProperty(String.prototype, "length2", { get: function () { return this.length * 2; }, configurable: true }); r.push(s.length2, "".length2); delete String.prototype.length2; r.push(s.length2); return r.join(); });
log("charCodeAt", function () { var r = []; for (var i = 0; i < samples.length; i++) { var s = samples[i], row = []; for (var p = -1; p <= s.length && p < 9; p++) row.push(s.charCodeAt(p)); r.push(row.join(" ")); } return r.join("|"); });
log("charCodeAtArguments", function () { var s = "abcdef"; return [s.charCodeAt(), s.charCodeAt(undefined), s.charCodeAt(null), s.charCodeAt("2"), s.charCodeAt(1.9), s.charCodeAt(-0.5), s.charCodeAt(NaN), s.charCodeAt(Infinity), s.charCodeAt(-Infinity), s.charCodeAt(true), s.charCodeAt({ valueOf: function () { return 3; } }), s.charCodeAt([4]), s.charCodeAt(1, 2, 3), s.charCodeAt(9007199254740991), s.charCodeAt(-9007199254740991), s.charCodeAt(2 ** 31), s.charCodeAt(-(2 ** 31))].join(); });
log("charCodeAtThis", function () {
  var r = [], f = String.prototype.charCodeAt, o = new String("xyz");
  r.push(o.charCodeAt(1), f.call("q", 0), f.call(123, 1), f.call(true, 0), f.call({ toString: function () { return "T"; } }, 0), f.call([7, 8], 1), Reflect.apply(f, "ab", [1]));
  try { f.call(null, 0); } catch (e) { r.push(e.constructor.name); } try { f.call(undefined, 0); } catch (e) { r.push(e.constructor.name); }
  try { "a".charCodeAt(Symbol()); } catch (e) { r.push(e.constructor.name); } try { "a".charCodeAt(1n); } catch (e) { r.push(e.constructor.name); }
  var bare = "abc".charCodeAt; try { r.push(bare(0)); } catch (e) { r.push(e.constructor.name); }
  var holder = { s: "hold", m: String.prototype.charCodeAt, toString: function () { return "HH"; } }; r.push(holder.m(0), holder.s.charCodeAt(0));
  return r.join();
});
log("directEqualsApplied", function () { var bad = 0, n = 0; for (var i = 0; i < samples.length; i++) { var s = samples[i]; for (var p = -2; p <= s.length + 1; p++) { n++; if (!Object.is(s.charCodeAt(p), Reflect.apply(String.prototype.charCodeAt, s, [p]))) bad++; } } return bad + "/" + n; });
log("methodsByCache", function () { var r = []; for (var i = 0; i < samples.length; i += 3) { var s = samples[i]; r.push(s.toUpperCase().length, s.indexOf("b"), s.slice(1, 3), typeof s.charAt, s.concat("!").length, s.trim === String.prototype.trim, s.constructor === String, s.toString() === s, s.valueOf() === s, typeof s.hasOwnProperty, s.nope); } return r.join(); });
log("patched", function () {
  var r = [], s = "abc", original = String.prototype.charCodeAt, calls = 0;
  String.prototype.charCodeAt = function (p) { calls++; return "patched" + p; };
  r.push(s.charCodeAt(1)); String.prototype.charCodeAt = original; r.push(s.charCodeAt(1));
  String.prototype.extra = function () { return this.length + "!"; }; r.push(s.extra(), "".extra()); String.prototype.extra = 5; r.push(s.extra); delete String.prototype.extra; r.push(s.extra);
  Object.prototype.fromObject = "inherited"; r.push(s.fromObject); delete Object.prototype.fromObject; r.push(s.fromObject);
  Object.defineProperty(String.prototype, "viaGetter", { get: function () { "use strict"; calls++; return typeof this + this; }, configurable: true }); r.push(s.viaGetter); delete String.prototype.viaGetter; r.push(s.viaGetter);
  var desc = Object.getOwnPropertyDescriptor(String.prototype, "charCodeAt"); Object.defineProperty(String.prototype, "charCodeAt", { get: function () { calls++; return function () { return "fromGetter"; }; }, configurable: true }); r.push(s.charCodeAt(0)); Object.defineProperty(String.prototype, "charCodeAt", desc); r.push(s.charCodeAt(0));
  return r.join() + "|" + calls;
});
log("tokenizer", function () {
  var source = ""; for (var i = 0; i < 12; i++) source += "(" + i + " + 3) * 2 - " + (i & 7) + " / 4 + "; source += "1";
  var tokens = [], at = 0;
  while (at < source.length) { var c = source.charCodeAt(at); if (c === 32) { at++; continue; } if (c >= 48 && c <= 57) { var start = at; while (at < source.length && source.charCodeAt(at) >= 48 && source.charCodeAt(at) <= 57) at++; tokens.push(+source.slice(start, at)); } else { tokens.push(source[at]); at++; } }
  var hash = 0; for (var k = 0; k < source.length; k++) hash = (hash * 31 + source.charCodeAt(k)) | 0;
  return tokens.length + "," + tokens.slice(0, 9).join("") + "," + hash;
});
log("otherPrimitives", function () { var n = 5, b = true, sym = Symbol("d"), big = 7n, r = []; r.push(n.length, b.length, sym.description, typeof n.toFixed, n.toFixed(1), typeof b.valueOf, big.toString(), typeof sym.toString, n.charCodeAt, (12).constructor === Number); try { null.length; } catch (e) { r.push(e.constructor.name); } try { undefined.charCodeAt; } catch (e) { r.push(e.constructor.name); } return r.map(String).join(); });
log("writesStillChecked", function () { var o = Object.freeze({ a: 1 }), p = {}; Object.defineProperty(p, "ro", { value: 1, writable: false, enumerable: true }); Object.defineProperty(p, "rw", { value: 1, writable: true, enumerable: false }); var q = { set s(v) { this._s = v + 1; }, plain: 0 }; for (var i = 0; i < 4; i++) { o.a = i + 5; p.ro = i + 5; p.rw = i + 5; q.s = i; q.plain = i; } var big = {}; for (var k = 0; k < 40; k++) big["k" + k] = k; Object.defineProperty(big, "k7", { writable: false }); for (var m = 0; m < 3; m++) { big.k7 = "x"; big.k8 = "y" + m; } return [o.a, p.ro, p.rw, q._s, q.plain, big.k7, big.k8, Object.keys(p).join("")].join(); });
log("numberMethods", function () {
  var vals = [0, -0, 1, -1, 255, 3.14159, -2.5, 1e21, 1e-7, 123456789.125, NaN, Infinity, -Infinity, 9007199254740991, 0.1 + 0.2], r = [];
  for (var i = 0; i < vals.length; i++) { var n = vals[i]; r.push([n.toFixed(2), n.toString(), n.toString(16), n.toPrecision(3), n.valueOf() === n || n !== n, typeof n.toLocaleString, n.nope, n.constructor === Number, n.hasOwnProperty("x")].join(" ")); }
  return r.join("|");
});
log("numberPatched", function () {
  var r = [], n = 7.5, original = Number.prototype.toFixed, calls = 0;
  Number.prototype.toFixed = function (d) { calls++; return "patched" + d; }; r.push(n.toFixed(1), (3).toFixed(0)); Number.prototype.toFixed = original; r.push(n.toFixed(1));
  Number.prototype.twice = function () { "use strict"; return typeof this + this * 2; }; r.push(n.twice(), (4).twice()); delete Number.prototype.twice; r.push(typeof n.twice);
  Object.defineProperty(Number.prototype, "half", { get: function () { calls++; return this / 2; }, configurable: true }); r.push(n.half, (9).half); delete Number.prototype.half; r.push(n.half);
  Object.prototype.shared = "s"; r.push(n.shared, "str".shared, true.shared); delete Object.prototype.shared; r.push(n.shared, "str".shared);
  n.own = 1; r.push(n.own); function strict() { "use strict"; try { n.own = 1; return "no"; } catch (e) { return e.constructor.name; } } r.push(strict());
  return r.map(String).join() + "|" + calls;
});
out.join("\n")
"#;
    let expected = r#"length0=0,1,7,8,23,1,8,10,2,7,1,4,15,41,4,2,5,5,3,2
length1=0,1,7,8,23,1,8,10,2,7,1,4,15,41,4,2,5,5,3,2
lengthIsOwn0=3,false,true,true,TypeError,6,0,
lengthIsOwn1=3,false,true,true,TypeError,6,0,
charCodeAt0=NaN NaN|NaN 97 NaN|NaN 97 98 99 100 101 102 103 NaN|NaN 97 98 99 100 101 102 103 104 NaN|NaN 97 32 108 111 110 103 101 114 32|NaN 1078 NaN|NaN 1078 1078 1078 1078 1078 1078 1078 1078 NaN|NaN 20013 25991 23383 31526 20018 21644 26356 22810 30340|NaN 55357 56832 NaN|NaN 97 55357 56832 98 55357 56832 99 NaN|NaN 55357 NaN|NaN 120 56832 121 55357 NaN|NaN 116 97 98 9 110 101 119 10 110|NaN 97 98 98 98 98 98 98 98 98
charCodeAt1=NaN NaN|NaN 97 NaN|NaN 97 98 99 100 101 102 103 NaN|NaN 97 98 99 100 101 102 103 104 NaN|NaN 97 32 108 111 110 103 101 114 32|NaN 1078 NaN|NaN 1078 1078 1078 1078 1078 1078 1078 1078 NaN|NaN 20013 25991 23383 31526 20018 21644 26356 22810 30340|NaN 55357 56832 NaN|NaN 97 55357 56832 98 55357 56832 99 NaN|NaN 55357 NaN|NaN 120 56832 121 55357 NaN|NaN 116 97 98 9 110 101 119 10 110|NaN 97 98 98 98 98 98 98 98 98
charCodeAtArguments0=97,97,97,99,98,97,97,NaN,NaN,98,100,101,98,NaN,NaN,NaN,NaN
charCodeAtArguments1=97,97,97,99,98,97,97,NaN,NaN,98,100,101,98,NaN,NaN,NaN,NaN
charCodeAtThis0=121,113,50,116,84,44,98,TypeError,TypeError,TypeError,TypeError,TypeError,72,104
charCodeAtThis1=121,113,50,116,84,44,98,TypeError,TypeError,TypeError,TypeError,TypeError,72,104
directEqualsApplied0=0/184
directEqualsApplied1=0/184
methodsByCache0=0,-1,,function,1,true,true,true,true,function,,8,1,bc,function,9,true,true,true,true,function,,8,-1,жж,function,9,true,true,true,true,function,,7,3,😀,function,8,true,true,true,true,function,,15,2,ab,function,16,true,true,true,true,function,
methodsByCache1=0,-1,,function,1,true,true,true,true,function,,8,1,bc,function,9,true,true,true,true,function,,8,-1,жж,function,9,true,true,true,true,function,,7,3,😀,function,8,true,true,true,true,function,,15,2,ab,function,16,true,true,true,true,function,
patched0=patched1,98,3!,0!,5,,inherited,,stringabc,,fromGetter,97|3
patched1=patched1,98,3!,0!,5,,inherited,,stringabc,,fromGetter,97|3
tokenizer0=145,(0+3)*2-0,1911844389
tokenizer1=145,(0+3)*2-0,1911844389
otherPrimitives0=undefined,undefined,d,function,5.0,function,7,function,undefined,true,TypeError,TypeError
otherPrimitives1=undefined,undefined,d,function,5.0,function,7,function,undefined,true,TypeError,TypeError
writesStillChecked0=1,1,8,4,3,7,y2,ro
writesStillChecked1=1,1,8,4,3,7,y2,ro
numberMethods0=0.00 0 0 0.00 true function  true false|0.00 0 0 0.00 true function  true false|1.00 1 1 1.00 true function  true false|-1.00 -1 -1 -1.00 true function  true false|255.00 255 ff 255 true function  true false|3.14 3.14159 3.243f3e0370cdc 3.14 true function  true false|-2.50 -2.5 -2.8 -2.50 true function  true false|1e+21 1e+21 3635c9adc5dea00000 1.00e+21 true function  true false|0.00 1e-7 0.000001ad7f29abcaf48 1.00e-7 true function  true false|123456789.13 123456789.125 75bcd15.2 1.23e+8 true function  true false|NaN NaN NaN NaN true function  true false|Infinity Infinity Infinity Infinity true function  true false|-Infinity -Infinity -Infinity -Infinity true function  true false|9007199254740991.00 9007199254740991 1fffffffffffff 9.01e+15 true function  true false|0.30 0.30000000000000004 0.4cccccccccccd 0.300 true function  true false
numberMethods1=0.00 0 0 0.00 true function  true false|0.00 0 0 0.00 true function  true false|1.00 1 1 1.00 true function  true false|-1.00 -1 -1 -1.00 true function  true false|255.00 255 ff 255 true function  true false|3.14 3.14159 3.243f3e0370cdc 3.14 true function  true false|-2.50 -2.5 -2.8 -2.50 true function  true false|1e+21 1e+21 3635c9adc5dea00000 1.00e+21 true function  true false|0.00 1e-7 0.000001ad7f29abcaf48 1.00e-7 true function  true false|123456789.13 123456789.125 75bcd15.2 1.23e+8 true function  true false|NaN NaN NaN NaN true function  true false|Infinity Infinity Infinity Infinity true function  true false|-Infinity -Infinity -Infinity -Infinity true function  true false|9007199254740991.00 9007199254740991 1fffffffffffff 9.01e+15 true function  true false|0.30 0.30000000000000004 0.4cccccccccccd 0.300 true function  true false
numberPatched0=patched1,patched0,7.5,number15,number8,undefined,3.75,4.5,undefined,s,s,s,undefined,undefined,undefined,TypeError|4
numberPatched1=patched1,patched0,7.5,number15,number8,undefined,3.75,4.5,undefined,s,s,s,undefined,undefined,undefined,TypeError|4"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// Properties that an object finds through its prototypes, and assignments
/// that add a property where the chain has none, when the chain changes
/// afterwards: a prototype gains the property as data, as an accessor or
/// as one that is not writable, loses it, or gets another prototype
/// itself. The prototype is given in every way there is (Object.create,
/// `new`, classes and their static side, subclasses of Array, typed arrays
/// and others, Reflect.construct, setPrototypeOf, `__proto__` in a
/// literal, generators, bound functions) and is of every kind (ordinary,
/// Array, function, typed array, String object, arguments and more), and
/// a Proxy or a typed array may stand in the chain. And an object that is
/// frozen, sealed or not extensible stays so when it becomes a prototype,
/// loses a property or is given private fields. What is expected is what
/// Node prints for the same script.
#[test]
fn properties_follow_the_changes_of_their_prototypes() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(properties_follow_prototypes)
        .expect("thread")
        .join()
        .expect("checks");
}

fn properties_follow_prototypes() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
var out = [];
function log(name, f) { var v; try { v = f(); } catch (e) { v = "threw " + (e && e.constructor && e.constructor.name) + ": " + (e && e.message); } out.push(name + ": " + v); }
log("typedArrayInChain", function () {
  function setNumeric(o) { o.NaN = 1; o.Infinity = 2; o.plain = 3; return Object.keys(o).join("") + "/" + o.NaN + "/" + o.plain; }
  var ta = new Uint8Array(2), r = [];
  for (var i = 0; i < 4; i++) r.push(setNumeric(Object.create(ta)));
  for (var j = 0; j < 3; j++) r.push(setNumeric({}));
  function onTyped(t) { t.NaN = 1; t.tag = 2; t.other = 3; return Object.keys(t).join("") + "/" + t.NaN; }
  for (var k = 0; k < 3; k++) r.push(onTyped(new Uint8Array(1)));
  return r.join();
});
log("stringAndArrayInChain", function () {
  function setLen(o) { o.length = 5; o.tag = 1; return Object.keys(o).join("") + "/" + o.length; }
  var r = [];
  for (var i = 0; i < 3; i++) r.push(setLen(Object.create(new String("ab"))));
  for (var j = 0; j < 3; j++) r.push(setLen(Object.create([1, 2])));
  for (var k = 0; k < 3; k++) r.push(setLen(Object.create(function (a, b, c) {})));
  function strictLen(o) { "use strict"; try { o.length = 5; return "set"; } catch (e) { return e.constructor.name; } }
  for (var m = 0; m < 3; m++) r.push(strictLen(Object.create(new String("ab"))));
  return r.join();
});
log("proxyInChain", function () {
  var seen = [];
  var px = new Proxy({}, {
    set: function (t, k, v, rc) { seen.push("set:" + String(k)); return Reflect.set(t, k, v, rc); },
    get: function (t, k, rc) { seen.push("get:" + String(k)); return Reflect.get(t, k, rc); },
    has: function (t, k) { seen.push("has:" + String(k)); return Reflect.has(t, k); },
    getOwnPropertyDescriptor: function (t, k) { seen.push("gopd:" + String(k)); return Reflect.getOwnPropertyDescriptor(t, k); },
    getPrototypeOf: function (t) { seen.push("proto"); return Reflect.getPrototypeOf(t); }
  });
  function add(o) { o.q = 1; o.w = 2; return Object.keys(o).join(""); }
  var r = [];
  for (var i = 0; i < 4; i++) r.push(add(Object.create(px)));
  var deep = Object.create(Object.create(px));
  for (var j = 0; j < 3; j++) r.push(add(Object.create(deep)));
  return r.join() + "|" + seen.join();
});
log("nullPrototype", function () {
  function add(o) { o.a = 1; o.b = 2; o.a = 3; return Object.keys(o).join("") + o.a; }
  var r = [];
  for (var i = 0; i < 4; i++) r.push(add(Object.create(null)));
  var bare = Object.create(null); bare.toString = 1; r.push(add(bare));
  return r.join();
});
log("getterOnlyAndReadOnlyInChain", function () {
  var p = { get g() { return "got"; } }; Object.defineProperty(p, "ro", { value: "fixed", writable: false });
  function add(o) { o.g = 1; o.ro = 2; o.z = 3; return Object.keys(o).join("") + "/" + o.g + "/" + o.ro; }
  function strictAdd(o) { "use strict"; var e1, e2; try { o.g = 1; } catch (e) { e1 = e.constructor.name; } try { o.ro = 1; } catch (e) { e2 = e.constructor.name; } return e1 + "+" + e2 + Object.keys(o).length; }
  var r = [];
  for (var i = 0; i < 4; i++) r.push(add(Object.create(p)), strictAdd(Object.create(p)));
  return r.join();
});
log("shapeChangesBetweenAdds", function () {
  function add(o, v) { o.late = v; return o; }
  var r = [], seen = [];
  var a = add({}, 1), b = add({}, 2);
  var withSetter = { set late(v) { seen.push("s" + v); } };
  var c = {}; Object.setPrototypeOf(c, withSetter); add(c, 3); r.push(Object.keys(c).length);
  var d = {}; add(d, 4); Object.setPrototypeOf(d, withSetter); add(d, 5); r.push(d.late, Object.keys(d).length);
  var e = { late: 0 }; add(e, 6); delete e.late; add(e, 7); r.push(e.late, Object.keys(e).join(""));
  var f = { x: 1, y: 2 }; delete f.x; add(f, 8); f.x = 9; r.push(Object.keys(f).join(""), f.late, f.x);
  var g = {}; Object.defineProperty(g, "late", { value: "own-ro", writable: false, configurable: true }); add(g, 9); r.push(g.late);
  var h = {}; Object.defineProperty(h, "late", { set: function (v) { seen.push("own" + v); }, configurable: true }); add(h, 10); r.push(Object.keys(h).length);
  var frozen = Object.freeze({}); add(frozen, 11); r.push(Object.keys(frozen).length, Object.isFrozen(frozen));
  var many = []; for (var i = 0; i < 40; i++) { var o = {}; o["k" + (i % 7)] = i; add(o, i); many.push(Object.keys(o).join("") + o.late); }
  return r.map(String).join() + "|" + seen.join() + "|" + a.late + b.late + "|" + many.slice(0, 9).join() + "|" + many[39];
});
log("classesAndSuper", function () {
  var seen = [];
  class Base { constructor() { this.a = 1; this.b = 2; } }
  class Mid extends Base { constructor() { super(); this.c = 3; } set late(v) { seen.push("mid" + v); } }
  class Leaf extends Mid { constructor() { super(); this.d = 4; this.late = 5; this.a = "re"; } }
  var r = [];
  for (var i = 0; i < 4; i++) { var l = new Leaf(); r.push(Object.keys(l).join("") + l.a); }
  Object.defineProperty(Base.prototype, "d", { set: function (v) { seen.push("base" + v); }, configurable: true });
  var after = new Leaf(); r.push(Object.keys(after).join(""));
  delete Base.prototype.d; var again = new Leaf(); r.push(Object.keys(again).join(""));
  Object.setPrototypeOf(Mid.prototype, { set c(v) { seen.push("swapped" + v); } }); var swapped = new Leaf(); r.push(Object.keys(swapped).join(""));
  return r.join() + "|" + seen.join();
});
log("valuesKept", function () {
  function P(i) { this.n = i; this.s = "s" + i; this.o = { i: i }; this.f = i / 2; this.u = undefined; this.z = null; this.big = i * 4294967296; }
  var all = []; for (var i = 0; i < 300; i++) all.push(new P(i));
  var sum = 0, bad = 0; for (var j = 0; j < all.length; j++) { var p = all[j]; sum += p.n + p.o.i + p.f; if (p.s !== "s" + j || p.u !== undefined || p.z !== null || p.big !== j * 4294967296 || Object.keys(p).length !== 7) bad++; }
  return sum + "|" + bad + "|" + JSON.stringify(all[299]);
});
print(out.join("\n"));
})();
(function () {
var print = function (s) { all.push(s); };
var out = [];
function log(name, f) { var v; try { v = f(); } catch (e) { v = "threw " + (e && e.constructor && e.constructor.name) + ": " + (e && e.message); } out.push(name + ": " + v); }
function readNaN(o) { return o.NaN; } function hasNaN(o) { return "NaN" in o; } function writeNaN(o) { o.NaN = 1; return Object.keys(o).join("") + "/" + o.NaN; }
function readInf(o) { return o.Infinity; } function writeInf(o) { o.Infinity = 1; return Object.keys(o).join("") + "/" + o.Infinity; }
function readLen(o) { return o.length; } function hasLen(o) { return "length" in o; } function writeLen(o) { o.length = 9; return Object.keys(o).join("") + "/" + o.length; }
function readTag(o) { return o.tag; } function hasTag(o) { return "tag" in o; } function writeTag(o) { o.tag = 7; return Object.keys(o).join("") + "/" + o.tag; }
function round(make, read, has, write) {
  var r = [];
  for (var i = 0; i < 3; i++) { var a = make(); r.push(read(a), read(a), has(a), write(a), read(a), has(a)); }
  for (var j = 0; j < 3; j++) { var b = make(); r.push(write(b), write(b)); }
  return r.map(String).join();
}
log("readThenWrite typed NaN", function () { var ta = new Float64Array(2); return round(function () { return Object.create(ta); }, readNaN, hasNaN, writeNaN); });
log("readThenWrite typed Infinity", function () { var ta = new Int8Array(2); return round(function () { return Object.create(ta); }, readInf, hasNaN, writeInf); });
log("readThenWrite typed deep", function () { var ta = new Uint16Array(2); var mid = Object.create(ta); return round(function () { return Object.create(mid); }, readNaN, hasNaN, writeNaN); });
log("readThenWrite typed tag", function () { var ta = new Uint16Array(2); return round(function () { return Object.create(ta); }, readTag, hasTag, writeTag); });
log("readThenWrite string length", function () { var s = new String("abc"); return round(function () { return Object.create(s); }, readLen, hasLen, writeLen); });
log("readThenWrite string tag", function () { var s = new String("abc"); return round(function () { return Object.create(s); }, readTag, hasTag, writeTag); });
log("readThenWrite array length", function () { var s = [1, 2, 3]; return round(function () { return Object.create(s); }, readLen, hasLen, writeLen); });
log("readThenWrite function length", function () { var s = function (a, b) {}; return round(function () { return Object.create(s); }, readLen, hasLen, writeLen); });
log("readThenWrite arguments", function () { var s = (function () { return arguments; })(1, 2); return round(function () { return Object.create(s); }, readLen, hasLen, writeLen) + ";" + round(function () { return Object.create(s); }, readTag, hasTag, writeTag); });
log("readThenWrite wrappers", function () {
  var kinds = [new Number(1), new Boolean(true), Object(Symbol("s")), new Date(0), new Map(), new Set(), /x/, new Error("e"), new ArrayBuffer(2), new DataView(new ArrayBuffer(2)), Promise.resolve(1), function* () {}(), Math, JSON, globalThis, (function () {}).bind(null), new WeakMap()];
  return kinds.map(function (k) { return round(function () { return Object.create(k); }, readTag, hasTag, writeTag).length; }).join();
});
log("readThenWrite proxy", function () {
  var seen = [];
  var px = new Proxy({}, { has: function (t, k) { seen.push("has:" + String(k)); return false; }, get: function (t, k) { seen.push("get:" + String(k)); return undefined; }, set: function (t, k, v, rc) { seen.push("set:" + String(k)); return true; } });
  return round(function () { return Object.create(px); }, readTag, hasTag, writeTag) + "|" + seen.join("").length + seen.slice(0, 8).join();
});
log("absent then prototype gains", function () {
  var r = [], seen = [];
  var proto = {}; function make() { return Object.create(proto); }
  r.push(round(make, readTag, hasTag, writeTag));
  Object.defineProperty(proto, "tag", { get: function () { return "getter"; }, configurable: true }); r.push(round(make, readTag, hasTag, writeTag));
  Object.defineProperty(proto, "tag", { set: function (v) { seen.push(v); }, configurable: true }); r.push(round(make, readTag, hasTag, writeTag));
  Object.defineProperty(proto, "tag", { value: "data", writable: true, configurable: true }); r.push(round(make, readTag, hasTag, writeTag));
  Object.defineProperty(proto, "tag", { writable: false }); r.push(round(make, readTag, hasTag, writeTag));
  delete proto.tag; r.push(round(make, readTag, hasTag, writeTag));
  Object.setPrototypeOf(proto, new Uint8Array(1)); r.push(round(make, readNaN, hasNaN, writeNaN));
  Object.setPrototypeOf(proto, null); r.push(round(make, readNaN, hasNaN, writeNaN));
  Object.preventExtensions(proto); r.push(round(make, readTag, hasTag, writeTag));
  return r.join(" ; ") + "|" + seen.join();
});
print(out.join("\n"));
})();
(function () {
var print = function (s) { all.push(s); };
var out = [];
function read(o) { return o.tag; }
function write(o) { o.tag = "w"; }
function own(o) { return Object.prototype.hasOwnProperty.call(o, "tag"); }
function scenario(name, make, proto) {
  var v;
  try {
    var r = [], seen = [];
    var o = make(); r.push(read(o), read(o));
    proto.tag = "data"; r.push(read(o), read(make()));
    delete proto.tag; r.push(read(o), read(make()));
    Object.defineProperty(proto, "tag", { get: function () { return "got"; }, set: function (v) { seen.push(v); }, configurable: true });
    r.push(read(o), read(make())); var w = make(); write(w); write(o); r.push(own(w), seen.join(""));
    Object.defineProperty(proto, "tag", { value: "fixed", writable: false, configurable: true });
    var f = make(); write(f); r.push(read(f), own(f));
    delete proto.tag; var w2 = make(); write(w2); r.push(read(w2), own(w2), read(make()));
    v = r.map(String).join();
  } catch (e) { v = "threw " + (e && e.constructor && e.constructor.name) + ": " + (e && e.message); }
  try { delete proto.tag; } catch (e) {}
  print(name + ": " + v);
}
(function () { var p = {}; scenario("create", function () { return Object.create(p); }, p); })();
(function () { var p = {}; scenario("createWithProps", function () { return Object.create(p, { x: { value: 1 } }); }, p); })();
(function () { var p = []; scenario("createArrayProto", function () { return Object.create(p); }, p); })();
(function () { var p = function () {}; scenario("createFunctionProto", function () { return Object.create(p); }, p); })();
(function () { var p = new Uint8Array(1); scenario("createTypedProto", function () { return Object.create(p); }, p); })();
(function () { var p = new Map(); scenario("createMapProto", function () { return Object.create(p); }, p); })();
(function () { var p = new Date(0); scenario("createDateProto", function () { return Object.create(p); }, p); })();
(function () { var p = (function () {}).bind(null); scenario("createBoundProto", function () { return Object.create(p); }, p); })();
(function () { var p = (function () { return arguments; })(1); scenario("createArgumentsProto", function () { return Object.create(p); }, p); })();
(function () { var p = new String("s"); scenario("createStringProto", function () { return Object.create(p); }, p); })();
(function () { var p = /x/; scenario("createRegExpProto", function () { return Object.create(p); }, p); })();
(function () { var p = new Error("e"); scenario("createErrorProto", function () { return Object.create(p); }, p); })();
(function () { var p = Promise.resolve(1); scenario("createPromiseProto", function () { return Object.create(p); }, p); })();
(function () { var p = Object.create(null); scenario("createNullBasedProto", function () { return Object.create(p); }, p); })();
(function () { function F() {} scenario("newDefault", function () { return new F(); }, F.prototype); })();
(function () { function F() {} var p = {}; F.prototype = p; scenario("newAssigned", function () { return new F(); }, p); })();
(function () { class C {} scenario("classInstance", function () { return new C(); }, C.prototype); })();
(function () { class X {} scenario("classStatic", function () { return class extends X {}; }, X); })();
(function () { class B {} class D extends B {} scenario("derivedInstance", function () { return new D(); }, B.prototype); })();
(function () { class A extends Array {} scenario("arraySubclass", function () { return new A(); }, A.prototype); })();
(function () { class T extends Uint8Array {} scenario("typedSubclass", function () { return new T(1); }, T.prototype); })();
(function () { class M extends Map {} scenario("mapSubclass", function () { return new M(); }, M.prototype); })();
(function () { class E extends Error {} scenario("errorSubclass", function () { return new E("m"); }, E.prototype); })();
(function () { class R extends RegExp {} scenario("regexpSubclass", function () { return new R("x"); }, R.prototype); })();
(function () { class Fn extends Function {} scenario("functionSubclass", function () { return new Fn(""); }, Fn.prototype); })();
(function () { function NT() {} var p = {}; NT.prototype = p; scenario("reflectConstructObject", function () { return Reflect.construct(Object, [], NT); }, p); })();
(function () { function NT() {} var p = {}; NT.prototype = p; scenario("reflectConstructArray", function () { return Reflect.construct(Array, [], NT); }, p); })();
(function () { function NT() {} var p = {}; NT.prototype = p; scenario("reflectConstructFunction", function () { return Reflect.construct(function () {}, [], NT); }, p); })();
(function () { function NT() {} var p = {}; NT.prototype = p; scenario("reflectConstructDate", function () { return Reflect.construct(Date, [0], NT); }, p); })();
(function () { var p = {}; scenario("setProto", function () { return Object.setPrototypeOf({}, p); }, p); })();
(function () { var p = {}; scenario("setProtoOnArray", function () { return Object.setPrototypeOf([], p); }, p); })();
(function () { var p = {}; scenario("setProtoOnFunction", function () { return Object.setPrototypeOf(function () {}, p); }, p); })();
(function () { var p = {}; scenario("reflectSetProto", function () { var o = {}; Reflect.setPrototypeOf(o, p); return o; }, p); })();
(function () { var p = {}; scenario("literalProto", function () { return { __proto__: p }; }, p); })();
(function () { var p = {}; scenario("literalProtoWithKeys", function () { return { a: 1, __proto__: p, b: 2 }; }, p); })();
(function () { function* g() {} scenario("generator", function () { return g(); }, g.prototype); })();
(function () { function* g() {} var p = {}; g.prototype = p; scenario("generatorAssigned", function () { return g(); }, p); })();
(function () { var p = {}; var mid = Object.create(p); scenario("deepChain", function () { return Object.create(mid); }, p); })();
(function () { var p = {}; var mid = Object.create(p); var mid2 = Object.create(mid); scenario("deeperChain", function () { return Object.create(mid2); }, p); })();
(function () { var p = {}; var mid = Object.create(p); scenario("deepChainMiddle", function () { return Object.create(mid); }, mid); })();
(function () { var p = Object.create(Function.prototype); function f() {} Object.setPrototypeOf(f, p); scenario("boundFunction", function () { return f.bind(null); }, p); })();
(function () { var p = {}; scenario("assignInto", function () { return Object.assign(Object.create(p), { z: 1 }); }, p); })();
(function () { scenario("objectPrototypeViaLiteral", function () { return {}; }, Object.prototype); })();
(function () { scenario("objectPrototypeViaArray", function () { return []; }, Object.prototype); })();
(function () { scenario("arrayPrototype", function () { return [1]; }, Array.prototype); })();
(function () { scenario("functionPrototype", function () { return function () {}; }, Function.prototype); })();
(function () { scenario("stringPrototype", function () { return "str"; }, String.prototype); })();
(function () { scenario("numberPrototype", function () { return 5; }, Number.prototype); })();
(function () { scenario("booleanPrototype", function () { return true; }, Boolean.prototype); })();
(function () { scenario("objectPrototypeViaString", function () { return "str"; }, Object.prototype); })();
(function () { scenario("objectPrototypeViaNumber", function () { return 1.5; }, Object.prototype); })();
(function () {
  var a = {}, b = { tag: "b" }; var o = Object.create(a); var r = [read(o), read(o)];
  Object.setPrototypeOf(a, b); r.push(read(o), read(Object.create(a))); Object.setPrototypeOf(a, null); r.push(read(o)); Object.setPrototypeOf(a, { get tag() { return "g"; } }); r.push(read(o)); var w = Object.create(a); write(w); r.push(own(w));
  print("prototypeOfPrototypeChanges: " + r.map(String).join());
})();
(function () {
  var p = { tag: 0 }; var o = Object.create(p); var r = [read(o), read(o)]; delete p.tag; r.push(read(o), read(o)); p.other = 1; p.tag = 2; r.push(read(o)); delete p.other; r.push(read(o), read(Object.create(p)));
  var q = { a: 1, tag: "q", c: 3 }; var x = Object.create(q); r.push(read(x), read(x)); delete q.a; r.push(read(x)); delete q.tag; r.push(read(x)); q.tag = "again"; r.push(read(x));
  print("prototypePropertyRemoved: " + r.map(String).join());
})();
})();
(function () {
var print = function (s) { all.push(s); };
function read(o) { return o.tag; }
function write(o) { o.tag = "w"; }
function own(o) { return Object.prototype.hasOwnProperty.call(o, "tag"); }
function scenario(name, proto) {
  var v;
  try {
    var make = function () { return Object.create(proto); };
    var r = [], seen = [];
    var o = make(); r.push(read(o), read(o));
    proto.tag = "data"; r.push(read(o), read(make()));
    delete proto.tag; r.push(read(o), read(make()));
    Object.defineProperty(proto, "tag", { get: function () { return "got"; }, set: function (v) { seen.push(v); }, configurable: true });
    r.push(read(o), read(make())); var w = make(); write(w); write(o); r.push(own(w), seen.join(""));
    delete proto.tag; var w2 = make(); write(w2); r.push(read(w2), own(w2), read(make()));
    v = r.map(String).join();
  } catch (e) { v = "threw " + (e && e.constructor && e.constructor.name); }
  try { delete proto.tag; } catch (e) {}
  print(name + ": " + v);
}
var gen = function* () { yield 1; };
var kinds = {
  set: new Set(), weakMap: new WeakMap(), weakSet: new WeakSet(), weakRef: new WeakRef({}), arrayBuffer: new ArrayBuffer(4), dataView: new DataView(new ArrayBuffer(4)),
  generatorObject: gen(), generatorFunction: gen, arrayIterator: [][Symbol.iterator](), mapIterator: new Map().entries(), setIterator: new Set().values(), stringIterator: ""[Symbol.iterator](),
  numberObject: new Number(1), booleanObject: new Boolean(false), symbolObject: Object(Symbol("s")), bigintObject: Object(1n),
  builtinFunction: Math.max, builtinConstructor: Map, classConstructor: class K {}, arrow: () => 1, asyncFunction: async function () {}, boundBuiltin: Math.max.bind(null),
  math: Math, json: JSON, reflect: Reflect, global: globalThis, float64: new Float64Array(2), bigInt64: new BigInt64Array(1), clamped: new Uint8ClampedArray(1),
  finalization: new FinalizationRegistry(function () {}), symbolPrototype: Symbol.prototype, errorType: new TypeError("t"), aggregate: new AggregateError([]), regexpResult: /a/.exec("a"),
  frozenThenNot: {}, nullProto: Object.create(null), date: new Date(1), promiseCapability: new Promise(function () {}), templateObject: (function (s) { return s.raw; })`x`
};
Object.keys(kinds).forEach(function (k) { scenario(k, kinds[k]); });
})();
(function () {
var print = function (s) { all.push(s); };
function t(name, f) { var v; try { v = f(); } catch (e) { v = "threw " + e.constructor.name; } print(name + ": " + v); }
function state(p) { p.added = 1; return [Object.isExtensible(p), Object.isFrozen(p), Object.isSealed(p), "added" in p].join(); }
function read(o) { return o.tag; }
function write(o) { o.tag = "w"; return Object.prototype.hasOwnProperty.call(o, "tag"); }
t("create", function () { var p = Object.preventExtensions({ a: 1 }); Object.create(p); return state(p); });
t("create frozen", function () { var p = Object.freeze({ a: 1 }); Object.create(p); return state(p); });
t("new", function () { var p = Object.preventExtensions({ a: 1 }); function F() {} F.prototype = p; new F(); return state(p); });
t("new frozen", function () { var p = Object.freeze({ a: 1 }); function F() {} F.prototype = p; new F(); return state(p); });
t("setPrototypeOf", function () { var p = Object.preventExtensions({ a: 1 }); Object.setPrototypeOf({}, p); return state(p); });
t("setPrototypeOf sealed", function () { var p = Object.seal({ a: 1 }); Object.setPrototypeOf({}, p); return state(p); });
t("literal", function () { var p = Object.preventExtensions({ a: 1 }); var o = { __proto__: p }; return state(p); });
t("class extends frozen", function () { function B() {} Object.freeze(B.prototype); Object.freeze(B); class D extends B {} new D(); return state(B.prototype) + "/" + state(B); });
t("frozen array", function () { var p = Object.freeze([1]); Object.create(p); return state(p) + p.length; });
t("frozen function", function () { var p = Object.freeze(function () {}); Object.create(p); return state(p); });
t("frozen after use", function () { var p = { a: 1 }; Object.create(p); Object.freeze(p); return state(p); });
t("template raw", function () { var p = (function (s) { return s.raw; })`x`; Object.create(p); return state(p); });
t("frozen with empty shape", function () { var p = Object.freeze({}); Object.create(p); return state(p); });
t("frozen twice used", function () { var p = Object.freeze({ a: 1 }); Object.create(p); function F() {} F.prototype = p; new F(); Object.setPrototypeOf({}, p); return state(p); });
t("strict add to frozen prototype", function () { "use strict"; var p = Object.freeze({ a: 1 }); Object.create(p); try { p.b = 1; return "added"; } catch (e) { return e.constructor.name + Object.keys(p).join(""); } });
t("non-extensible prototype loses a property", function () {
  var p = { tag: "p", other: 1 }; Object.preventExtensions(p); var o = Object.create(p), r = [read(o), read(o)];
  delete p.tag; r.push(read(o), read(Object.create(p)), write(Object.create(p))); p.tag = "back"; r.push(read(o), "tag" in p);
  return r.map(String).join();
});
t("non-extensible prototype redefines a property", function () {
  var seen = []; var p = { tag: "p" }; Object.preventExtensions(p); var o = Object.create(p), r = [read(o), read(o)];
  Object.defineProperty(p, "tag", { get: function () { return "got"; }, set: function (v) { seen.push(v); } }); r.push(read(o), write(o), write(Object.create(p)));
  Object.defineProperty(p, "tag", { value: "ro", writable: false }); r.push(read(o), write(Object.create(p)));
  return r.map(String).join() + "|" + seen.join("");
});
t("sealed prototype", function () {
  var p = Object.seal({ tag: "p" }); var o = Object.create(p), r = [read(o), read(o)]; p.tag = "changed"; r.push(read(o), delete p.tag, read(o), write(Object.create(p)));
  try { Object.setPrototypeOf(p, {}); r.push("moved"); } catch (e) { r.push(e.constructor.name); } r.push(read(Object.create(p)));
  return r.map(String).join();
});
t("frozen prototype shadows by assignment", function () {
  var p = Object.freeze({ tag: "p" }); var r = []; for (var i = 0; i < 3; i++) { var o = Object.create(p); r.push(read(o), write(o), read(o)); }
  function strict(o) { "use strict"; try { o.tag = 1; return "set"; } catch (e) { return e.constructor.name; } } r.push(strict(Object.create(p)), strict(Object.create(p)));
  return r.map(String).join();
});
t("absent through a frozen prototype", function () {
  var p = Object.freeze({ a: 1 }); var r = []; for (var i = 0; i < 3; i++) { var o = Object.create(p); r.push(read(o), write(o), read(o), Object.keys(o).join("")); }
  var q = Object.preventExtensions({ a: 1 }); for (var j = 0; j < 3; j++) { var x = Object.create(q); r.push(read(x), write(x), read(x)); } q.tag = "no"; r.push(read(Object.create(q)));
  return r.map(String).join();
});
t("receiver made non-extensible", function () {
  function add(o) { o.fresh = 1; return Object.keys(o).join(""); } var r = [];
  for (var i = 0; i < 3; i++) r.push(add({}), add(Object.preventExtensions({})), add(Object.freeze({ z: 1 })), add(Object.seal({ y: 1 })));
  var same = { m: 1 }; r.push(add(same)); Object.preventExtensions(same); delete same.fresh; r.push(add(same));
  return r.join();
});
})();
(function () {
var print = function (s) { all.push(s); };
function t(name, f) { var v; try { v = f(); } catch (e) { v = "threw " + e.constructor.name; } print(name + ": " + v); }
function add(o) { o.fresh = 1; return Object.keys(o).join("") + Object.isExtensible(o); }
t("A delete after preventExtensions", function () { var same = { m: 1 }; add(same); Object.preventExtensions(same); delete same.fresh; return add(same); });
t("B no prior add", function () { var same = { m: 1, fresh: 0 }; Object.preventExtensions(same); delete same.fresh; return add(same); });
t("C no function", function () { var same = { m: 1, fresh: 0 }; Object.preventExtensions(same); delete same.fresh; same.fresh = 1; return Object.keys(same).join("") + Object.isExtensible(same); });
t("D computed", function () { var same = { m: 1, fresh: 0 }; Object.preventExtensions(same); delete same.fresh; var k = "fresh"; same[k] = 1; return Object.keys(same).join("") + Object.isExtensible(same); });
t("E delete only", function () { var same = { m: 1, fresh: 0 }; Object.preventExtensions(same); delete same.fresh; return Object.keys(same).join("") + Object.isExtensible(same) + Object.isFrozen(same); });
t("F delete last then define", function () { var same = { m: 1, fresh: 0 }; Object.preventExtensions(same); delete same.fresh; try { Object.defineProperty(same, "fresh", { value: 1 }); } catch (e) { return e.constructor.name + Object.isExtensible(same); } return "defined"; });
t("G delete first key", function () { var same = { fresh: 0, m: 1 }; Object.preventExtensions(same); delete same.fresh; same.fresh = 1; return Object.keys(same).join("") + Object.isExtensible(same); });
t("H delete on prototype-used", function () { var same = { m: 1, fresh: 0 }; Object.create(same); Object.preventExtensions(same); delete same.fresh; same.fresh = 1; return Object.keys(same).join("") + Object.isExtensible(same); });
t("I array", function () { var a = [1]; a.fresh = 0; Object.preventExtensions(a); delete a.fresh; a.fresh = 1; return Object.keys(a).join("") + Object.isExtensible(a); });
})();
(function () {
var print = function (s) { all.push(s); };
function t(name, f) { var v; try { v = f(); } catch (e) { v = "threw " + e.constructor.name; } print(name + ": " + v); }
function state(p) { p.added = 1; return [Object.isExtensible(p), Object.isFrozen(p), Object.isSealed(p), "added" in p].join(); }
class Base { constructor(o) { return o; } }
class Stamper extends Base { #p = 1; #q = 2; static has(o) { return #p in o; } }
t("private onto frozen", function () { var f = Object.freeze({ a: 1 }); new Stamper(f); return Stamper.has(f) + "/" + state(f); });
t("private onto non-extensible", function () { var f = Object.preventExtensions({ a: 1, b: 2 }); new Stamper(f); return Stamper.has(f) + "/" + state(f); });
t("private onto frozen empty", function () { var f = Object.freeze({}); new Stamper(f); return Stamper.has(f) + "/" + state(f); });
t("private onto frozen prototype", function () { var f = Object.freeze({ a: 1 }); Object.create(f); new Stamper(f); return Stamper.has(f) + "/" + state(f); });
t("private onto frozen array", function () { var f = Object.freeze([1]); new Stamper(f); return Stamper.has(f) + "/" + state(f); });
t("private then delete on non-extensible", function () { var f = Object.preventExtensions({ a: 1, b: 2 }); new Stamper(f); delete f.a; return Stamper.has(f) + "/" + state(f) + Object.keys(f).join(""); });
})();
(function () {
var print = function (s) { all.push(s); };
function t(name, f) { var v; try { v = f(); } catch (e) { v = "threw " + e.constructor.name; } print(name + ": " + v); }
function rd(o) { return o.tag; } function wr(o, v) { o.tag = v; } function has(o) { return Object.prototype.hasOwnProperty.call(o, "tag"); }
t("add to a prototype itself", function () { var P = {}; var c = Object.create(P), r = []; for (var i = 0; i < 4; i++) { r.push(rd(P), rd(c)); wr(P, i); r.push(rd(P), rd(c), has(P), has(c)); delete P.tag; } return r.map(String).join(); });
t("add to a prototype then to its child", function () { var P = {}; var r = []; for (var i = 0; i < 4; i++) { var c = Object.create(P); r.push(rd(c)); wr(P, "p" + i); r.push(rd(c)); wr(c, "c" + i); r.push(rd(c), rd(P), Object.keys(c).join("")); delete P.tag; r.push(rd(c)); } return r.map(String).join(); });
t("add to Object.prototype and others", function () { var r = []; for (var i = 0; i < 3; i++) { r.push(rd({}), rd([]), rd("s"), rd(5)); wr(Object.prototype, i); r.push(rd({}), rd([]), rd("s"), rd(5), rd(Object.prototype)); delete Object.prototype.tag; wr(Array.prototype, "a" + i); r.push(rd([]), rd({})); delete Array.prototype.tag; } return r.map(String).join(); });
t("add to a class prototype and constructor", function () { class K {} var k = new K(), r = []; for (var i = 0; i < 3; i++) { r.push(rd(k), rd(K)); wr(K.prototype, i); wr(K, "s" + i); r.push(rd(k), rd(K), rd(class extends K {})); delete K.prototype.tag; delete K.tag; } return r.map(String).join(); });
t("add to a middle of a chain", function () { var a = {}, b = Object.create(a), c = Object.create(b), r = []; for (var i = 0; i < 3; i++) { r.push(rd(c)); wr(b, "b" + i); r.push(rd(c), rd(b), rd(a)); wr(a, "a" + i); r.push(rd(c), rd(a)); delete b.tag; r.push(rd(c)); delete a.tag; r.push(rd(c)); } return r.map(String).join(); });
t("add to a prototype with a setter above", function () { var seen = []; var top = { set tag(v) { seen.push(v); } }; var P = Object.create(top); Object.create(P); var r = []; for (var i = 0; i < 3; i++) { wr(P, i); r.push(has(P)); } delete top.tag; for (var j = 0; j < 2; j++) { wr(P, "own" + j); r.push(has(P), rd(P)); delete P.tag; } return r.map(String).join() + "|" + seen.join(""); });
t("add to a non-extensible prototype", function () { var P = { a: 1 }; Object.create(P); Object.preventExtensions(P); var r = []; for (var i = 0; i < 3; i++) { wr(P, i); r.push(has(P), rd(P)); } return r.map(String).join() + Object.keys(P).join(""); });
t("many properties on a prototype", function () { var P = {}; var c = Object.create(P); for (var i = 0; i < 40; i++) { P["k" + i] = i; } P.tag = "t"; var sum = 0; for (var j = 0; j < 40; j++) sum += c["k" + j]; return sum + rd(c) + Object.keys(P).length; });
})();
all.join("\n")"#;
    let expected = r#"typedArrayInChain: plain/undefined/3,plain/undefined/3,plain/undefined/3,plain/undefined/3,NaNInfinityplain/1/3,NaNInfinityplain/1/3,NaNInfinityplain/1/3,0tagother/undefined,0tagother/undefined,0tagother/undefined
stringAndArrayInChain: tag/2,tag/2,tag/2,lengthtag/5,lengthtag/5,lengthtag/5,tag/3,tag/3,tag/3,TypeError,TypeError,TypeError
proxyInChain: qw,qw,qw,qw,qw,qw,qw|set:q,set:w,set:q,set:w,set:q,set:w,set:q,set:w,set:q,set:w,set:q,set:w,set:q,set:w
nullPrototype: ab3,ab3,ab3,ab3,toStringab3
getterOnlyAndReadOnlyInChain: z/got/fixed,TypeError+TypeError0,z/got/fixed,TypeError+TypeError0,z/got/fixed,TypeError+TypeError0,z/got/fixed,TypeError+TypeError0
shapeChangesBetweenAdds: 0,5,1,7,late,ylatex,8,9,own-ro,0,0,true|s3,own10|12|k0late0,k1late1,k2late2,k3late3,k4late4,k5late5,k6late6,k0late7,k1late8|k4late39
classesAndSuper: abcdre,abcdre,abcdre,abcdre,abc,abcd,abd|mid5,mid5,mid5,mid5,base4,mid5,mid5,swapped3,mid5
valuesKept: 112125|0|{"n":299,"s":"s299","o":{"i":299},"f":149.5,"z":null,"big":1284195221504}
readThenWrite typed NaN: undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,/undefined,/undefined,/undefined,/undefined,/undefined,/undefined
readThenWrite typed Infinity: undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,/undefined,/undefined,/undefined,/undefined,/undefined,/undefined
readThenWrite typed deep: undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,/undefined,/undefined,/undefined,/undefined,/undefined,/undefined
readThenWrite typed tag: undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,tag/7,tag/7,tag/7,tag/7,tag/7,tag/7
readThenWrite string length: 3,3,true,/3,3,true,3,3,true,/3,3,true,3,3,true,/3,3,true,/3,/3,/3,/3,/3,/3
readThenWrite string tag: undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,tag/7,tag/7,tag/7,tag/7,tag/7,tag/7
readThenWrite array length: 3,3,true,length/9,9,true,3,3,true,length/9,9,true,3,3,true,length/9,9,true,length/9,length/9,length/9,length/9,length/9,length/9
readThenWrite function length: 2,2,true,/2,2,true,2,2,true,/2,2,true,2,2,true,/2,2,true,/2,/2,/2,/2,/2,/2
readThenWrite arguments: 2,2,true,length/9,9,true,2,2,true,length/9,9,true,2,2,true,length/9,9,true,length/9,length/9,length/9,length/9,length/9,length/9;undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,tag/7,tag/7,tag/7,tag/7,tag/7,tag/7
readThenWrite wrappers: 152,152,152,152,152,152,152,152,152,152,152,152,152,152,152,152,152
readThenWrite proxy: undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,/undefined,/undefined,/undefined,/undefined,/undefined,/undefined|231get:tag,get:tag,has:tag,set:tag,get:tag,get:tag,has:tag,get:tag
absent then prototype gains: undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,tag/7,tag/7,tag/7,tag/7,tag/7,tag/7 ; getter,getter,true,/getter,getter,true,getter,getter,true,/getter,getter,true,getter,getter,true,/getter,getter,true,/getter,/getter,/getter,/getter,/getter,/getter ; getter,getter,true,/getter,getter,true,getter,getter,true,/getter,getter,true,getter,getter,true,/getter,getter,true,/getter,/getter,/getter,/getter,/getter,/getter ; data,data,true,tag/7,7,true,data,data,true,tag/7,7,true,data,data,true,tag/7,7,true,tag/7,tag/7,tag/7,tag/7,tag/7,tag/7 ; data,data,true,/data,data,true,data,data,true,/data,data,true,data,data,true,/data,data,true,/data,/data,/data,/data,/data,/data ; undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,tag/7,tag/7,tag/7,tag/7,tag/7,tag/7 ; undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,undefined,undefined,false,/undefined,undefined,false,/undefined,/undefined,/undefined,/undefined,/undefined,/undefined ; undefined,undefined,false,NaN/1,1,true,undefined,undefined,false,NaN/1,1,true,undefined,undefined,false,NaN/1,1,true,NaN/1,NaN/1,NaN/1,NaN/1,NaN/1,NaN/1 ; undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,undefined,undefined,false,tag/7,7,true,tag/7,tag/7,tag/7,tag/7,tag/7,tag/7|7,7,7,7,7,7,7,7,7
create: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createWithProps: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createArrayProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createFunctionProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createTypedProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createMapProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createDateProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createBoundProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createArgumentsProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createStringProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createRegExpProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createErrorProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createPromiseProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
createNullBasedProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
newDefault: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
newAssigned: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
classInstance: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
classStatic: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
derivedInstance: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
arraySubclass: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
typedSubclass: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
mapSubclass: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
errorSubclass: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
regexpSubclass: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
functionSubclass: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
reflectConstructObject: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
reflectConstructArray: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
reflectConstructFunction: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
reflectConstructDate: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
setProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
setProtoOnArray: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
setProtoOnFunction: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
reflectSetProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
literalProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
literalProtoWithKeys: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
generator: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
generatorAssigned: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
deepChain: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
deeperChain: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
deepChainMiddle: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
boundFunction: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
assignInto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
objectPrototypeViaLiteral: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
objectPrototypeViaArray: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
arrayPrototype: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
functionPrototype: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,w,true,undefined
stringPrototype: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,undefined,false,undefined
numberPrototype: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,undefined,false,undefined
booleanPrototype: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,undefined,false,undefined
objectPrototypeViaString: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,undefined,false,undefined
objectPrototypeViaNumber: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,fixed,false,undefined,false,undefined
prototypeOfPrototypeChanges: undefined,undefined,b,b,undefined,g,false
prototypePropertyRemoved: 0,0,undefined,undefined,2,2,2,q,q,q,undefined,again
set: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
weakMap: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
weakSet: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
weakRef: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
arrayBuffer: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
dataView: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
generatorObject: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
generatorFunction: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
arrayIterator: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
mapIterator: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
setIterator: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
stringIterator: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
numberObject: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
booleanObject: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
symbolObject: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
bigintObject: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
builtinFunction: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
builtinConstructor: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
classConstructor: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
arrow: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
asyncFunction: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
boundBuiltin: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
math: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
json: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
reflect: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
global: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
float64: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
bigInt64: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
clamped: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
finalization: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
symbolPrototype: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
errorType: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
aggregate: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
regexpResult: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
frozenThenNot: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
nullProto: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
date: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
promiseCapability: undefined,undefined,data,data,undefined,undefined,got,got,false,ww,w,true,undefined
templateObject: threw TypeError
create: false,false,false,false
create frozen: false,true,true,false
new: false,false,false,false
new frozen: false,true,true,false
setPrototypeOf: false,false,false,false
setPrototypeOf sealed: false,false,true,false
literal: false,false,false,false
class extends frozen: false,true,true,false/false,true,true,false
frozen array: false,true,true,false1
frozen function: false,true,true,false
frozen after use: false,true,true,false
template raw: false,true,true,false
frozen with empty shape: false,true,true,false
frozen twice used: false,true,true,false
strict add to frozen prototype: TypeErrora
non-extensible prototype loses a property: p,p,undefined,undefined,true,undefined,false
non-extensible prototype redefines a property: p,p,got,false,false,ro,false|ww
sealed prototype: p,p,changed,false,changed,true,TypeError,changed
frozen prototype shadows by assignment: p,false,p,p,false,p,p,false,p,TypeError,TypeError
absent through a frozen prototype: undefined,true,w,tag,undefined,true,w,tag,undefined,true,w,tag,undefined,true,w,undefined,true,w,undefined,true,w,undefined
receiver made non-extensible: fresh,,z,y,fresh,,z,y,fresh,,z,y,mfresh,m
A delete after preventExtensions: mfalse
B no prior add: mfalse
C no function: mfalse
D computed: mfalse
E delete only: mfalsefalse
F delete last then define: TypeErrorfalse
G delete first key: mfalse
H delete on prototype-used: mfalse
I array: 0false
private onto frozen: true/false,true,true,false
private onto non-extensible: true/false,false,false,false
private onto frozen empty: true/false,true,true,false
private onto frozen prototype: true/false,true,true,false
private onto frozen array: true/false,true,true,false
private then delete on non-extensible: true/false,false,false,falseb
add to a prototype itself: undefined,undefined,0,0,true,false,undefined,undefined,1,1,true,false,undefined,undefined,2,2,true,false,undefined,undefined,3,3,true,false
add to a prototype then to its child: undefined,p0,c0,p0,tag,c0,undefined,p1,c1,p1,tag,c1,undefined,p2,c2,p2,tag,c2,undefined,p3,c3,p3,tag,c3
add to Object.prototype and others: undefined,undefined,undefined,undefined,0,0,0,0,0,a0,undefined,undefined,undefined,undefined,undefined,1,1,1,1,1,a1,undefined,undefined,undefined,undefined,undefined,2,2,2,2,2,a2,undefined
add to a class prototype and constructor: undefined,undefined,0,s0,s0,undefined,undefined,1,s1,s1,undefined,undefined,2,s2,s2
add to a middle of a chain: undefined,b0,b0,undefined,b0,a0,a0,undefined,undefined,b1,b1,undefined,b1,a1,a1,undefined,undefined,b2,b2,undefined,b2,a2,a2,undefined
add to a prototype with a setter above: false,false,false,true,own0,true,own1|012
add to a non-extensible prototype: false,undefined,false,undefined,false,undefineda
many properties on a prototype: 780t41"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// ToInt32 and its like (the bitwise operators, typed array elements, a
/// DataView's setters) of Numbers too large for a 64-bit integer, and the
/// remainder where the quotient rounded to a double is not exact: `1e21 |
/// 0`, `1e21 % 3`, `5.5 % 1.1`. What is expected is what Node prints for
/// the same script.
#[test]
fn conversions_and_remainders_of_large_numbers_are_exact() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(large_number_conversions)
        .expect("thread")
        .join()
        .expect("checks");
}

fn large_number_conversions() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
function show(v) { return typeof v === "number" && Object.is(v, -0) ? "-0" : String(v); }
var values = [1e21, -1e21, 9223372036854775808, -9223372036854775808, 9223372036854777856, 18446744073709551616, 18446744073709555712, 1.5e19, -9.3e18, 9.2e18, 4294967296, 4294967297.5, -4294967297.5, 2147483648, -2147483649, 1e300, -1e300, 1.7976931348623157e308, 5e-324, 0.9, -0.9, 12345678901234567890, 2 ** 53, 2 ** 53 * 3, 2 ** 80 + 2 ** 40, -(2 ** 70) - 2 ** 33, NaN, Infinity, -Infinity, -0];
var rows = [];
for (var i = 0; i < values.length; i++) {
  var x = values[i];
  rows.push([x | 0, x >>> 0, x >> 1, x << 1, x & 1023, x ^ 5, ~x, x % 3, x % 7, x % 1000000007, x % 0.7, x % -3, x % 4294967296, -x % 3, 5 % x].map(show).join(" "));
}
print("operators: " + rows.join("|"));
var typed = [];
for (var j = 0; j < values.length; j++) {
  var v = values[j];
  typed.push([new Int8Array([v])[0], new Uint8Array([v])[0], new Uint8ClampedArray([v])[0], new Int16Array([v])[0], new Uint16Array([v])[0], new Int32Array([v])[0], new Uint32Array([v])[0]].join(" "));
}
print("typed arrays: " + typed.join("|"));
var dv = new DataView(new ArrayBuffer(8)), views = [];
for (var k = 0; k < 8; k++) { var w = values[k]; dv.setInt32(0, w); dv.setUint16(4, w); dv.setInt8(6, w); views.push([dv.getInt32(0), dv.getUint16(4), dv.getInt8(6)].join(" ")); }
print("data view: " + views.join("|"));
print("others: " + [Math.imul(1e21, 3), Math.clz32(1e21), "abc".charCodeAt(1e21 % 3), (1e21).toString(2).length, 1e21 % 2 === 0, (2 ** 64 + 2 ** 12) % 4096, 123456789012345680000 % 97, -1e21 % 1e9, 1e21 % 1e20, 7.5 % 2, -7.5 % 2, 7.5 % -2, 0.1 % 0.01, 5 % 0.3, 1e308 % 3, 9007199254740993 % 10, 1e21 >> 33, 1e21 << 33, 1e21 >>> 33, String.fromCharCode(1e21 + 65), [1, 2, 3].at(1e21 % 3), 5.5 % 1.1, 10 % 0.1, 1e15 % 0.7, -5.5 % 1.1, 5.5 % -1.1, 1 % 0.1, 0.3 % 0.1, 100 % 0.01, 2.5 % 0.5, 1e-7 % 1e-9].map(show).join(" "));
})();
all.join("\n")"#;
    let expected = r#"operators: -559939584 3735027712 -279969792 -1119879168 0 -559939579 559939583 1 6 49000 0.015692866092049007 1 3735027712 -1 5|559939584 559939584 279969792 1119879168 0 559939589 -559939585 -1 -6 -49000 -0.015692866092049007 -1 -3735027712 1 5|0 0 0 0 0 5 -1 2 1 291172004 0.24285714285717996 2 0 -2 5|0 0 0 0 0 5 -1 -2 -1 -291172004 -0.24285714285717996 -2 -0 2 5|2048 2048 1024 4096 0 2053 -2049 1 5 291174052 0.042857142857309904 1 2048 -1 5|0 0 0 0 0 5 -1 1 2 582344008 0.48571428571435993 1 0 -1 5|4096 4096 2048 8192 0 4101 -4097 2 3 582348104 0.08571428571461981 2 4096 -2 5|-824442880 3470524416 -412221440 -1648885760 0 -824442875 824442879 0 1 735 0.6197353929913807 0 3470524416 -0 5|81657856 81657856 40828928 163315712 0 81657861 -81657857 -0 -3 -900000462 -0.10423594365465605 -0 -4213309440 0 5|-1650982912 2643984384 -825491456 993001472 0 -1650982907 1650982911 2 5 600000455 0.6601043743680468 2 2643984384 -2 5|0 0 0 0 0 5 -1 1 4 294967268 0.5000002724783761 1 0 -1 5|1 1 0 2 1 4 -2 2.5 5.5 294967269.5 0.6000002724783762 2.5 1.5 -2.5 5|-1 4294967295 -1 -2 1023 -6 0 -2.5 -5.5 -294967269.5 -0.6000002724783762 -2.5 -1.5 2.5 5|-2147483648 2147483648 -1073741824 0 0 -2147483643 2147483647 2 2 147483634 0.600000136239188 2 2147483648 -2 5|2147483647 2147483647 1073741823 -2 1023 2147483642 -2147483648 -0 -3 -147483635 -0.2000001362391881 -0 -2147483649 0 5|0 0 0 0 0 5 -1 0 1 673966564 0.3790560774266656 0 0 -0 5|0 0 0 0 0 5 -1 -0 -1 -673966564 -0.3790560774266656 -0 -0 0 5|0 0 0 0 0 5 -1 2 5 343596930 0.4649767750469902 2 0 -2 5|0 0 0 0 0 5 -1 5e-324 5e-324 5e-324 5e-324 5e-324 5e-324 -5e-324 0|0 0 0 0 0 5 -1 0.9 0.9 0.9 0.20000000000000007 0.9 0.9 -0.9 0.4999999999999999|0 0 0 0 0 5 -1 -0.9 -0.9 -0.9 -0.20000000000000007 -0.9 -0.9 0.9 0.4999999999999999|-350287872 3944679424 -175143936 -700575744 0 -350287867 350287871 1 0 814815470 0.6261126159783585 1 3944679424 -1 5|0 0 0 0 0 5 -1 2 4 191690599 0.37142857142857144 2 0 -2 5|0 0 0 0 0 5 -1 0 5 575071797 0.41428571428571437 0 0 -0 5|0 0 0 0 0 5 -1 2 6 8261216 0.5714983307585424 2 0 -2 5|0 0 0 0 0 5 -1 -0 -3 -859950789 -0.5857148306757896 -0 -0 0 5|0 0 0 0 0 5 -1 NaN NaN NaN NaN NaN NaN NaN NaN|0 0 0 0 0 5 -1 NaN NaN NaN NaN NaN NaN NaN 5|0 0 0 0 0 5 -1 NaN NaN NaN NaN NaN NaN NaN 5|0 0 0 0 0 5 -1 -0 -0 -0 -0 -0 -0 0 NaN
typed arrays: 0 0 255 0 0 -559939584 3735027712|0 0 0 0 0 559939584 559939584|0 0 255 0 0 0 0|0 0 0 0 0 0 0|0 0 255 2048 2048 2048 2048|0 0 255 0 0 0 0|0 0 255 4096 4096 4096 4096|0 0 255 0 0 -824442880 3470524416|0 0 0 0 0 81657856 81657856|0 0 255 0 0 -1650982912 2643984384|0 0 255 0 0 0 0|1 1 255 1 1 1 1|-1 255 0 -1 65535 -1 4294967295|0 0 255 0 0 -2147483648 2147483648|-1 255 0 -1 65535 2147483647 2147483647|0 0 255 0 0 0 0|0 0 0 0 0 0 0|0 0 255 0 0 0 0|0 0 0 0 0 0 0|0 0 1 0 0 0 0|0 0 0 0 0 0 0|0 0 255 2048 2048 -350287872 3944679424|0 0 255 0 0 0 0|0 0 255 0 0 0 0|0 0 255 0 0 0 0|0 0 0 0 0 0 0|0 0 0 0 0 0 0|0 0 255 0 0 0 0|0 0 0 0 0 0 0|0 0 0 0 0 0 0
data view: -559939584 0 0|559939584 0 0|0 0 0|0 0 0|2048 2048 0|0 0 0|4096 4096 0|-824442880 0 0
others: -1679818752 0 98 70 true 0 54 -0 0 1.5 -1.5 1.5 3.469446951953614e-18 0.20000000000000018 2 2 -279969792 -1119879168 1867513856   2 1.0999999999999996 0.09999999999999945 0.46344131569286606 -1.0999999999999996 1.0999999999999996 0.09999999999999995 0.09999999999999998 0.009999999999997919 0 9.999999999999893e-10"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The instructions that take an operand from where it is instead of from
/// the stack: an operator with a constant on its right (of a value or of a
/// local), `a[i]` with a local for its key, and a variable of an enclosing
/// function or a constant pushed by one instruction. Every operator with
/// operands of every type, the order of conversions and their effects, the
/// values that leave the integers, the bases and keys an element read can
/// have, References kept across compound assignments, what throws in the
/// middle of an expression, closures, generators and loops; and the
/// comparisons, the tests of loops and the operators on Numbers that are
/// not integers, which convert nothing either, with loops long enough
/// that the Numbers they make have to be collected; an operator whose
/// value goes to a local or onto the stack by the same instruction, the
/// function of a method call pushed with its Reference, and every way a
/// function run in place of its call returns. What is expected is what
/// Node prints for the same script.
#[test]
fn operands_taken_in_place_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(operands_taken_in_place)
        .expect("thread")
        .join()
        .expect("checks");
}

fn operands_taken_in_place() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
function show(v) { if (typeof v === "number") return Object.is(v, -0) ? "-0" : String(v); if (typeof v === "bigint") return v + "n"; if (typeof v === "string") return JSON.stringify(v); if (typeof v === "symbol") return "symbol"; if (typeof v === "function") return "function"; if (v === null) return "null"; if (typeof v === "object") { try { return Array.isArray(v) ? "[" + v.map(show).join(",") + "]" : "{" + Object.keys(v).join() + "}"; } catch (e) { return "object"; } } return String(v); }
function t(name, f) { for (var round = 0; round < 2; round++) { var v; try { v = show(f()); } catch (e) { v = "threw " + (e && e.constructor ? e.constructor.name : typeof e); } print(name + round + ": " + v); } }
var lefts = [0, 1, -1, 7, -7, 255, 1023, 2147483647, -2147483648, 4294967295, 9007199254740991, -9007199254740991, 0.5, -0.5, 1e21, -0, NaN, Infinity, -Infinity, "", "5", "abc", " 12 ", true, false, null, undefined, 10n, -3n, { valueOf: function () { return 6; } }, { toString: function () { return "9"; } }, [], [4], {}];
t("resultWithConstant", function () {
  var r = [];
  for (var i = 0; i < lefts.length; i++) {
    var x = lefts[i], row = [];
    var ops = [function (o) { return o.v + 1; }, function (o) { return o.v - 1; }, function (o) { return o.v * 2; }, function (o) { return o.v / 2; }, function (o) { return o.v % 3; }, function (o) { return o.v << 1; }, function (o) { return o.v >> 1; }, function (o) { return o.v >>> 1; }, function (o) { return o.v | 8; }, function (o) { return o.v ^ 5; }, function (o) { return o.v & 1023; },
      function (o) { return o.v + 0; }, function (o) { return o.v * 0; }, function (o) { return o.v * -1; }, function (o) { return o.v % -3; }, function (o) { return o.v % 0; }, function (o) { return o.v / 0; }, function (o) { return o.v << 33; }, function (o) { return o.v >>> 0; }, function (o) { return o.v + "s"; }, function (o) { return o.v + 0.5; }, function (o) { return o.v * 9007199254740991; }, function (o) { return o.v + 9007199254740991; }, function (o) { return o.v - -9007199254740991; }, function (o) { return o.v & -1; }, function (o) { return o.v | 0; }];
    for (var k = 0; k < ops.length; k++) { try { row.push(show(ops[k]({ v: x }))); } catch (e) { row.push(e.constructor.name); } }
    r.push(row.join(" "));
  }
  return r.join("|");
});
t("localWithConstant", function () {
  var r = [];
  for (var i = 0; i < lefts.length; i++) {
    var row = [];
    (function (x) { try { row.push(show(x + 1)); } catch (e) { row.push("E"); } try { row.push(show(x - 1)); } catch (e) { row.push("E"); } try { row.push(show(x * 3)); } catch (e) { row.push("E"); } try { row.push(show(x / 4)); } catch (e) { row.push("E"); } try { row.push(show(x % 7)); } catch (e) { row.push("E"); } try { row.push(show(x << 2)); } catch (e) { row.push("E"); } try { row.push(show(x >> 2)); } catch (e) { row.push("E"); } try { row.push(show(x >>> 2)); } catch (e) { row.push("E"); } try { row.push(show(x | 1)); } catch (e) { row.push("E"); } try { row.push(show(x ^ 1)); } catch (e) { row.push("E"); } try { row.push(show(x & 255)); } catch (e) { row.push("E"); } try { row.push(show(x * 0)); } catch (e) { row.push("E"); } try { row.push(show(x % 0)); } catch (e) { row.push("E"); } try { row.push(show(x + "")); } catch (e) { row.push("E"); } try { row.push(show(x - 0.25)); } catch (e) { row.push("E"); } })(lefts[i]);
    r.push(row.join(" "));
  }
  return r.join("|");
});
t("twoLocalsThenConstant", function () {
  function f(a, b) { return [a + (b & 255), a - (b * 2), a * (b + 1), a | (b << 3), a + b * 2 + 1, (a & 15) + (b & 15), a + (b % 5), a ^ (b >>> 1)].map(show).join(" "); }
  return [f(1, 2), f(1000, 1023), f(-1, -1), f(0.5, 3), f("s", 4), f(2, "7"), f(3, 2.5), f(1, null), f(undefined, 1)].join("|");
});
t("constantOverflowAndZero", function () {
  var big = 9007199254740991, r = [];
  function add(x) { return x + 9007199254740991; } function sub(x) { return x - 9007199254740991; } function mul(x) { return x * 94906267; } function mulz(x) { return x * 0; } function rem(x) { return x % 4; } function shl(x) { return x << 31; }
  r.push(add(1), add(big), add(-big), sub(-1), sub(-big), mul(94906267), mul(-94906267), mulz(-5), mulz(5), mulz(0), rem(-4), rem(-5), rem(5), rem(0), shl(1), shl(3), shl(-1));
  return r.map(show).join(" ");
});
t("valueOfOrderAndEffects", function () {
  var log = [];
  var o = { valueOf: function () { log.push("v"); return 3; } };
  function f(x) { return [x + 1, x * 2, x & 1, x - 1].join(); }
  var s = { toString: function () { log.push("s"); return "7"; }, valueOf: undefined };
  var d = new Date(0); var sym = Symbol("q"), r = [f(o), f(s)];
  try { f(sym); } catch (e) { r.push(e.constructor.name); } try { (function (x) { return x + 1; })(1n); } catch (e) { r.push(e.constructor.name); } r.push((function (x) { return x * 2n; })(4n) + "n");
  r.push(typeof (function (x) { return x + 1; })(d), (function (x) { return x - 1; })(d));
  var thrower = { valueOf: function () { throw new RangeError("no"); } }; try { (function (x) { return x | 1; })(thrower); } catch (e) { r.push(e.constructor.name); }
  return r.join("|") + "|" + log.join("");
});
t("compoundAndProperties", function () {
  var o = { x: 5, y: "s", z: 1.5 }, a = [1, 2, 3], r = [];
  o.x += 1; o.x *= 2; o.x -= 3; o.x <<= 2; o.x |= 1; o.x %= 7; o.y += 1; o.z *= 2; a[1] += 10; a[2] &= 2; a[0] -= 1;
  var i = 1; a[i] += 5; i += 1; i *= 3; i <<= 1; var c = 7; c %= 4; c **= 2;
  r.push(o.x, o.y, o.z, a, i, c);
  var g = { get p() { r.push("get"); return 2; }, set p(v) { r.push("set" + v); } }; g.p += 1; g.p <<= 3;
  return r.map(show).join(" ");
});
var outer = 10, counter = 0;
t("closureOperands", function () {
  var x = 4, y = "str", u, arr = [10, 20, 30], bump = function () { x = x + 1; return x; };
  function inner() {
    var r = [];
    r.push(x + 1, x - 1, x * 2, x & 3, y + 1, y * 2, u + 1, outer + 1, x + x, x + bump(), bump() + x, x + (x = 9), x);
    r.push(arr[0] + x, arr[x - 8], [x, 1, y, 2], Math.max(x, 1, 3), (function (p, q, s) { return p + q + s; })(x, 1, y));
    r.push(x === 9, x === 10, y === "str", x !== 10, u === undefined, x < 11, x > 100);
    return r.map(show).join(" ");
  }
  return inner() + "|" + inner();
});
t("closureTdzAndLate", function () {
  var r = [];
  function early() { return late + 1; } try { early(); } catch (e) { r.push(e.constructor.name); }
  function pushed() { return [late, 1]; } try { pushed(); } catch (e) { r.push(e.constructor.name); }
  let late = 5; r.push(early(), pushed().join());
  function del() { return typeof gone === "undefined" ? "gone" : gone + 1; } r.push(del()); globalThis.gone = 2; r.push(del()); delete globalThis.gone; r.push(del());
  try { (function () { return missingName + 1; })(); } catch (e) { r.push(e.constructor.name); }
  return r.join(" ");
});
t("localIndexReads", function () {
  var arr = [10, 20, , 40], ta = new Int16Array([5, -6, 7]), str = "héllo", obj = { 0: "zero", k: "kay", 1.5: "frac" }, r = [];
  var keys = [0, 1, 2, 3, 4, -1, 1.5, "1", "k", "length", NaN, null, undefined, true, 4294967295, 4294967296, { toString: function () { return "1"; } }, [0], Symbol.iterator];
  for (var j = 0; j < keys.length; j++) { var k = keys[j]; r.push([arr[k], ta[k], str[k], obj[k]].map(function (v) { return typeof v === "function" ? "fn" : show(v); }).join("/")); }
  return r.join(" ");
});
t("localIndexBases", function () {
  var r = [], i = 1, o = { items: [7, 8, 9], get list() { r.push("get"); return [1, 2, 3]; } }, holder = [[1, 2], [3, 4]];
  function make() { r.push("make"); return ["a", "b", "c"]; }
  var closureArr = [100, 200, 300];
  function inner(n) { return closureArr[n] + closureArr[i]; }
  r.push(o.items[i], o.list[i], make()[i], holder[i][i], holder[0][i], inner(2), "xyz"[i], [5, 6, 7][i], (i ? holder : null)[i].length, arguments.length ? arguments[i] : "none");
  var px = new Proxy([1, 2, 3], { get: function (t, k) { r.push("trap:" + String(k)); return t[k]; } }); r.push(px[i]);
  var getter = []; Object.defineProperty(getter, 1, { get: function () { r.push("elem"); return "g"; } }); r.push(getter[i]);
  var proto = Object.create([9, 8, 7]); r.push(proto[i]); Array.prototype[5] = "inherited"; var holes = [1]; var five = 5; r.push(holes[five]); delete Array.prototype[5]; r.push(holes[five]);
  try { var n = null; n[i]; } catch (e) { r.push(e.constructor.name); } try { var un; un[i]; } catch (e) { r.push(e.constructor.name); }
  var frozen = Object.freeze([4, 5]); r.push(frozen[i]); var big = new Float64Array([1.5, 2.5]); r.push(big[i]); var u8 = new Uint8Array(2); u8[i] = 300; r.push(u8[i]);
  return r.map(show).join(" ");
});
t("pushedConstants", function () {
  function args() { return Array.prototype.slice.call(arguments).map(show).join(); }
  var x = 3, o = { m: args }, r = [];
  r.push(args(x, 1), args(1, x), args(1, 2, 3), args(x, "s", null, undefined, true, 1.5, -0), o.m(x, 7), args(x + 1, 2, x * 2), [x, 1, 2], [1, x], { a: x, b: 1 }.b, x + 1 + x, 1 + x + 1, (x, 5), x ? 1 : 2, args(args(x, 1), 2));
  function noReturn(v) { args(v, 1); } function lastExpr(v) { v + 1; args(1); } r.push(noReturn(4), lastExpr(4));
  var s = "t" + 1 + x + "u" + 2; r.push(s, `a${x}b${1}c`, typeof x + 1, -x + 1, !x + 1, void x + 1);
  return r.map(show).join(" ");
});
print("eval: " + [eval("var e1 = [1, 2]; 5;"), eval("var e2 = 3; e2 + 1; if (false) { 9; }"), eval("var e3 = 4; f3(e3, 1); function f3(a, b) { return a + b; }"), eval("1; for (var e4 = 0; e4 < 2; e4++) { e4 + 1; }"), eval("var e5 = [7, 8], e6 = 1; e5[e6];"), eval("var e7 = 6; e7 & 3;"), eval("2; { }"), eval("var e8 = 1; switch (e8) { case 1: e8 + 1; }")].map(show).join(" "));
})();
(function () {
var print = function (s) { all.push(s); };
function show(v) { if (typeof v === "number") return Object.is(v, -0) ? "-0" : String(v); if (typeof v === "string") return JSON.stringify(v); if (typeof v === "function") return "function"; if (v === null) return "null"; if (Array.isArray(v)) return "[" + v.map(show).join(",") + "]"; if (typeof v === "object") return "{" + Object.keys(v).map(function (k) { return k + ":" + show(v[k]); }).join() + "}"; return String(v); }
function t(name, f) { for (var round = 0; round < 2; round++) { var v; try { v = show(f()); } catch (e) { v = "threw " + (e && e.constructor ? e.constructor.name : typeof e); } print(name + round + ": " + v); } }
t("referencesKept", function () {
  var log = [], o = { p: [1, 2, 3], q: [7, 8, 9], n: 5, get g() { log.push("g"); return 3; }, set g(v) { log.push("s" + v); } }, i = 1, k = "n";
  o.p[i] += 2; o.p[i]++; --o.q[i]; o.p[i] = o.p[i] + o.q[i]; o[k] += 1; o[k] <<= 2; o.g += 1; o.g *= 2; o.n -= 1; o.n %= 5; o.n **= 2;
  var d = delete o.p[i], ty = typeof o.q[i], inn = i in o.p, fnv = (o.q)[i].toString(), swap = [o.p[0], o.q[0]] = [o.q[0], o.p[0]];
  class C { #f = 6; static s = 2; bump() { this.#f += 1; this.#f <<= 1; C.s |= 4; return this.#f; } } class D extends C { m() { return super.bump() + 1; } }
  var arr2 = [[1, 2], [3, 4]]; arr2[i][i] *= 10; arr2[0][i] -= 1; var str = "abc"; var ch = str[i] + 1;
  return [o.p, o.q, o.n, d, ty, inn, fnv, swap, new D().m(), C.s, arr2, ch, log.join("")];
});
t("pushedInManyPlaces", function () {
  var x = 2, a = [1, 2], o = { m: function () { return Array.prototype.slice.call(arguments); } }, k = "m";
  function C(p, q) { this.v = [p, q]; } function tag(s, v, w) { return s.raw.join("-") + v + w; }
  return [new C(x, 1).v, o[k](1, x, 2), o.m(...a, 1), o.m?.(x, 1), tag`a${x}b${1}c`, `${x}${1}${"s"}`, x ?? 1, null ?? 1, x || 1, 0 || 1, x && 1, x ? 1 : 2, [x, 1, , 2], { a: 1, [x]: 2, b: x }, [...a, 1, ...a], Math.max(1, x, 3), (function () { return arguments.length; })(1, 2, x), (x, 1), [1, 2, 3].indexOf(x, 1), "s".padStart(x + 1, 1), String(x) + 1 + 2, 1 + 2 + String(x), +x + 1, -x - 1, typeof x + 1, void 0 === undefined];
});
t("throwsMidExpression", function () {
  var thrower = { valueOf: function () { throw new RangeError("v"); } }, r = [], i = 1, arr = [10, 20, 30];
  for (var n = 0; n < 4; n++) {
    var x = n === 2 ? thrower : n;
    try { r.push([1, arr[i], x & 1023, 4]); } catch (e) { r.push(e.constructor.name); }
    try { r.push(arr[i] + (x + 1) * 2 - arr[0]); } catch (e) { r.push(e.constructor.name); }
    try { r.push(Math.max(arr[i], x * 2, 7)); } catch (e) { r.push(e.constructor.name); }
    try { var un; r.push(arr[i] + un[i]); } catch (e) { r.push(e.constructor.name); }
    r.push(arr[i] + arr[n % 3]);
  }
  return r;
});
var shared = 5, sharedArr = [3, 1, 2];
t("closuresInCalls", function () {
  var base = 10, list = [4, 5, 6], calls = 0;
  function add(a, b) { calls++; return a + b; } function third(a, b, c) { return c; }
  function inner(i) { return add(base, i) + add(i, base) + third(base, list, shared) + list[i] + sharedArr[i] + add(list[i], sharedArr[i]) + (base & 7) + (shared << 2) + base % 3; }
  var r = []; for (var i = 0; i < 3; i++) r.push(inner(i)); base = "b"; r.push(inner(0)); base = 1.5; r.push(inner(1)); list = "xyz"; r.push(inner(2)); list = new Uint8Array([9, 8, 7]); base = 2; r.push(inner(2));
  var fns = []; for (let j = 0; j < 3; j++) fns.push(function () { return j * 2 + base + list[j]; }); r.push(fns.map(function (f) { return f(); }));
  return [r, calls];
});
t("generatorsAndAsync", function () {
  function* gen(a, i) { var x = yield a[i] + 1; var y = yield (x & 255) * 2; yield [a[i], x - 1, y % 3, a[i + 1]]; return x + y; }
  var g = gen([5, 6, 7], 1), out = [g.next().value, g.next(300).value, g.next(10).value, g.next().value, g.next().done];
  var seen = []; async function af(a, i) { var x = await a[i]; seen.push(x + 1, a[i] * 2, (x | 0) - 1); return x; } af([1, 2], 1); af(["s"], 0);
  function* counter(n) { for (var i = 0; i < n; i++) { var got = yield i * 2 + 1; if (got) i += got - 1; } }
  var c = counter(6), steps = [c.next().value, c.next().value, c.next(2).value, c.next().value, c.next().done];
  return [out, steps, seen.length];
});
t("loopsWithFusedBodies", function () {
  var a = [], sum = 0, bits = 0, s = "", f = 1.5, big = 9007199254740000;
  for (var i = 0; i < 40; i++) a.push(i * 3 % 7);
  for (var j = 1; j < a.length; j++) { a[j] = (a[j - 1] + a[j]) & 1023; sum += a[j] * 2 + 1; bits ^= a[j] << (j % 5); s += a[j] % 10; f = f * 2 % 1000; big += 100; }
  var k = 0, w = 0; while (k < 100) { w += k & 15; k += 3; } do { w -= k % 4; k -= 7; } while (k > 0);
  var t8 = new Int8Array(8); for (var m = 0; m < 20; m++) { t8[m & 7] += m * 9; }
  return [sum, bits, s, f, big, w, k, Array.prototype.slice.call(t8), a.slice(35)];
});
t("numberOperandsOfAnyKind", function () {
  var nums = [0, -0, 1, -1, 7, 0.5, -0.5, 1.5, 2.5, -2.5, 255.9, 2147483647, 2147483648, -2147483649, 4294967295.5, 9007199254740991, 9007199254740992, 1e21, -1e21, 5e-324, 1.7976931348623157e308, NaN, Infinity, -Infinity, 123456789.125, 0.1 + 0.2];
  var others = ["3", "", "x", true, null, undefined, { valueOf: function () { return 2.5; } }, [5], 3n];
  var rows = [], sum = 0;
  for (var i = 0; i < nums.length; i++) {
    for (var j = 0; j < nums.length; j++) {
      var a = nums[i], b = nums[j];
      var bits = (a < b ? 1 : 0) | (a <= b ? 2 : 0) | (a > b ? 4 : 0) | (a >= b ? 8 : 0);
      var v = [bits, a << b, a >> b, a >>> b, a | b, a ^ b, a & b];
      for (var k = 0; k < v.length; k++) sum = (sum * 31 + (v[k] | 0)) | 0;
      if ((i * 7 + j) % 23 === 0) rows.push(v.join("/"));
    }
  }
  var mixed = [];
  for (var m = 0; m < others.length; m++) {
    var o = others[m], x = 2.5;
    try { mixed.push([o < x, x <= o, o > x, x >= o, o | x, x & o, o << 1.5, x >>> o, o ^ 0.5].join("/")); } catch (e) { mixed.push(e.constructor.name); }
  }
  var f = 0.5, n = 0; while (f < 100.5) { f = f * 1.5 + 0.25; n++; } var g = 10.5; do { g -= 0.75; n++; } while (g >= 0.5);
  for (var h = 0.25; h <= 3; h += 0.5) n += h > 1.5 ? 2 : 1;
  return [sum, rows.join(" "), mixed.join(" "), n, f, g];
});
t("numberArithmeticOfAnyKind", function () {
  var nums = [0, -0, 1, -1, 3, 7, 0.5, -0.5, 1.5, -2.5, 0.1, 0.2, 255.9, 2147483647, -2147483648, 4294967296, 9007199254740991, -9007199254740991, 9007199254740992, 1e21, -1e21, 5e-324, 1.7976931348623157e308, NaN, Infinity, -Infinity, 123456789.125, 1 / 3];
  function hash(h, v) { var s = Object.is(v, -0) ? "-0" : String(v); for (var c = 0; c < s.length; c++) h = (h * 31 + s.charCodeAt(c)) | 0; return h; }
  var h = 7, rows = [];
  for (var i = 0; i < nums.length; i++) {
    for (var j = 0; j < nums.length; j++) {
      var a = nums[i], b = nums[j], v = [a + b, a - b, a * b, a / b, a % b, a + 1, a - 1, a * 2, a / 2, a % 3, a * 0.5, a + 0.1, b - 0.25, a / 0.1, a % 0.7, a * -1, a / 0, a * 1e308, a - 9007199254740991];
      for (var k = 0; k < v.length; k++) h = hash(h, v[k]);
      if ((i * 5 + j) % 37 === 0) rows.push(v.slice(0, 5).map(function (x) { return Object.is(x, -0) ? "-0" : String(x); }).join("/"));
    }
  }
  return [h, rows.join(" ")];
});
t("numbersMadeInLoops", function () {
  var acc = 0, x = 0.1, grid = new Float64Array(64), parts = [];
  for (var i = 0; i < 300000; i++) { acc += i * 0.01 - x * 0.5 + (i % 7) / 3; x = x * 1.0000001 + 0.000001; grid[i & 63] = acc * 0.25 + i / 7; }
  function straight(v) { v = v * 1.5 + 0.25; v = v / 3 - 0.125; v = v * v + 0.5; v = v % 9.75; v = v - 0.375; v = v * 2.5; v = v / 7; v = v + 1e-9; return v; }
  var s = 0.5; for (var j = 0; j < 100000; j++) s = straight(s) % 100.5;
  var kept = []; for (var k = 0; k < 2000; k++) kept.push(k * 1.1 + 0.3); var sum = 0; for (var m = 0; m < kept.length; m++) sum += kept[m] * 0.5;
  parts.push(acc.toFixed(3), x.toFixed(6), grid[5].toFixed(3), s.toFixed(6), sum.toFixed(3), kept[1999]);
  return parts;
});
t("numberTestsThatJump", function () {
  var nums = [0, -0, 1, -1, 2, 0.5, -0.5, 1.5, 2.5, 3, 2147483648, 9007199254740991, 9007199254740992, 1e21, -1e21, 5e-324, NaN, Infinity, -Infinity, 0.1 + 0.2, 0.3];
  var h = 3, locals = [];
  for (var i = 0; i < nums.length; i++) {
    for (var j = 0; j < nums.length; j++) {
      var a = nums[i], b = nums[j], bits = 0;
      if (a < b) bits |= 1; else bits |= 2; if (a <= b) bits |= 4; else bits |= 8; if (a > b) bits |= 16; else bits |= 32; if (a >= b) bits |= 64; else bits |= 128;
      if (!(a < b)) bits |= 256; if (!(a >= b)) bits |= 512; bits |= (a < b || a > b) ? 1024 : 0; bits |= (a <= b && a >= b) ? 2048 : 0;
      h = (h * 33 + bits) | 0;
    }
    var x = nums[i], row = 0;
    if (x < 1.5) row |= 1; if (x <= 0) row |= 2; if (x > -0.5) row |= 4; if (x >= 2.5) row |= 8; if (x < NaN) row |= 16; if (x >= -Infinity) row |= 32; if (x < 2) row |= 64; if (x <= 9007199254740992) row |= 128; if (x > 0.30000000000000004) row |= 256; if (x < "1") row |= 512; if (x >= null) row |= 1024;
    locals.push(row);
  }
  var loops = [], n, c, f;
  var bounds = [2.5, 3, -0, NaN, 0.5, 4.000001, "3", null, undefined, { valueOf: function () { return 2.5; } }, 1e21, -1];
  for (var k = 0; k < bounds.length; k++) {
    n = bounds[k]; c = 0; for (var p = 0; p < n; p++) { if (++c > 9) break; } var up = c;
    c = 0; for (var q = 0; q <= n; q++) { if (++c > 9) break; } var upEq = c;
    c = 0; for (var r = 6; r > n; r--) { if (++c > 9) break; } var down = c;
    c = 0; for (var s = 6; r >= n && s >= n; s--) { if (++c > 9) break; } loops.push([up, upEq, down, c].join(""));
  }
  c = 0; for (f = 0.5; f < 3; f += 0.5) c++; var w = 0; while (w < 2.75) { w += 0.75; c++; } var d = 5.5; do { d -= 1.25; c++; } while (d > 0.25);
  for (var y = 0; y < 6.5; y += 2) c++; for (var z = 10; z >= 0.5; z -= 3.5) c++; var e = 0; for (var v = 0.1; v <= 1; v += 0.1) e++;
  return [h, locals.join(), loops.join(), c, f, w, d, e];
});
})();
(function () {
var print = function (s) { all.push(s); };
function show(v) { if (typeof v === "number") return Object.is(v, -0) ? "-0" : String(v); if (typeof v === "bigint") return v + "n"; if (typeof v === "string") return JSON.stringify(v); if (typeof v === "function") return "function"; if (typeof v === "symbol") return "symbol"; if (v === null) return "null"; if (Array.isArray(v)) return "[" + v.map(show).join(",") + "]"; if (typeof v === "object") return "{" + Object.keys(v).map(function (k) { return k + ":" + show(v[k]); }).join() + "}"; return String(v); }
function t(name, f) { for (var round = 0; round < 2; round++) { var v; try { v = show(f()); } catch (e) { v = "threw " + (e && e.constructor ? e.constructor.name : typeof e); } print(name + round + ": " + v); } }
var values = [0, 1, -1, 7, 2.5, -0.5, 2147483647, 9007199254740991, 1e21, NaN, -0, Infinity, "", "5", "ab", true, null, undefined, 3n, { valueOf: function () { return 4; } }, [2], {}];
t("operatorResultToLocal", function () {
  var rows = [];
  for (var i = 0; i < values.length; i++) {
    for (var j = 0; j < values.length; j += 3) {
      var a = values[i], b = values[(i + j) % values.length], r = [], x;
      try { x = a + b; r.push(x); } catch (e) { r.push("E"); } try { x = a - b; r.push(x); } catch (e) { r.push("E"); } try { x = a * b; r.push(x); } catch (e) { r.push("E"); } try { x = a / b; r.push(x); } catch (e) { r.push("E"); } try { x = a % b; r.push(x); } catch (e) { r.push("E"); }
      try { x = a << b; r.push(x); } catch (e) { r.push("E"); } try { x = a >> b; r.push(x); } catch (e) { r.push("E"); } try { x = a >>> b; r.push(x); } catch (e) { r.push("E"); } try { x = a | b; r.push(x); } catch (e) { r.push("E"); } try { x = a ^ b; r.push(x); } catch (e) { r.push("E"); } try { x = a & b; r.push(x); } catch (e) { r.push("E"); }
      rows.push(r.map(show).join(" "));
    }
  }
  var h = 0, s = rows.join("|"); for (var c = 0; c < s.length; c++) h = (h * 31 + s.charCodeAt(c)) | 0;
  return [h, rows[0], rows[5], rows[40], rows[rows.length - 1]];
});
t("targetIsAnOperand", function () {
  var a = 5, b = 3, c = "s", d = 2.5, log = [];
  a = a + b; b = a - b; a = b * a; c = c + a; d = d / b; a = a % 7; b = b << a; a = a | b; b = b ^ a; a = a & 12; a += b; b -= a; a *= 2; b /= 4; a %= 5; a <<= 2; b >>= 1; a >>>= 1; a |= 9; b &= 6; a ^= 3; c += d; d **= 2;
  var o = { valueOf: function () { log.push("v"); return 10; } }, p = 1; p = p + o; p = o * p; p += o; var q = o; q = q + q; q -= 1;
  var u; u = u + 1; var n = null; n = n + 1; var e; try { e = 1n; e = e + 1; } catch (err) { e = err.constructor.name; }
  return [a, b, c, d, p, q, u, n, e, log.join("")];
});
t("operatorResultPushed", function () {
  function args() { return Array.prototype.slice.call(arguments); }
  var a = 6, b = 4, s = "x", f = 1.5, o = { m: args, k: 2 }, arr = [10, 20, 30, 40];
  return [args(a + b), args(a * b, a - b), args(a & b, 1, a | b), [a + b, a / b, s + a], o.m(a % b), o.m(a << 1, b >> 1), (a + b) * (a - b), (a * 2 + 1) * (b * 2 - 1), a + b + s + f, s + a + b, (a + b) & 7, (a * b) % 5 === 4, ((a + b) | 0) === 10, arr[a - b], arr[(a + b) % 4], o.k + a * b, -(a + b), typeof (a + b), !(a - 6), (a + b) + "", `${a + b}${a * b}`, a + b > 9 ? "big" : "small", args(a + b, a + b)[1], Math.max(a + b, a * b, a - b), (a, a + b), [a + b][0] + [a * 2][0]];
});
t("pushedThenThrows", function () {
  var r = [], thrower = { valueOf: function () { throw new RangeError("v"); } }, a = 2, b = 3;
  function args() { return arguments.length; }
  for (var n = 0; n < 3; n++) {
    var x = n === 1 ? thrower : n;
    try { r.push(args(a + b, x + 1, a * b)); } catch (e) { r.push(e.constructor.name); }
    try { r.push([a + b, x * 2, a - b].length); } catch (e) { r.push(e.constructor.name); }
    try { var y = 0; y = a + x; r.push(y); } catch (e) { r.push(e.constructor.name); }
    try { var z = 5; z += x; r.push(z); } catch (e) { r.push(e.constructor.name + z); }
    r.push(a + b);
  }
  return r;
});
t("methodCalls", function () {
  var log = [];
  var o = { n: 1, m: function () { return [this === o, arguments.length].concat(Array.prototype.slice.call(arguments)); }, get g() { log.push("g"); return function (x) { return this.n + x; }; }, inner: { deep: function (x) { return this === o.inner ? x : "lost"; } }, s: "str", nil: null };
  var k = "m", x = 3, r = [];
  r.push(o.m(), o.m(x), o.m(1), o.m(x, 1, "s"), o.m(x + 1, x * 2), o.m(o.m(x)[2]), o.g(x), o.g(1), o.inner.deep(x), o[k](x), o.s.charCodeAt(x - 2), o.s.toUpperCase().length, (5).toFixed(1), "abc".slice(1).concat(x), [3, 1, 2].sort().join(x), o.m?.(x), o.nil?.(x), o.missing?.(x));
  try { o.missing(x); } catch (e) { r.push(e.constructor.name); } try { o.nil(x); } catch (e) { r.push(e.constructor.name); } try { o.s(x); } catch (e) { r.push(e.constructor.name); } try { var un; un.m(x); } catch (e) { r.push(e.constructor.name); }
  try { o.m(x, (function () { throw new RangeError("arg"); })()); } catch (e) { r.push(e.constructor.name); } r.push(o.m(x).length);
  var px = new Proxy({ f: function (a) { return a * 2; } }, { get: function (target, key) { log.push("get:" + String(key)); return target[key]; } }); r.push(px.f(x), px.f(1));
  class B { v(x) { return "B" + x; } static s(x) { return "S" + x; } } class D extends B { v(x) { return super.v(x + 1) + this.w(x); } w(x) { return "w" + x; } static s(x) { return super.s(x) + "!"; } }
  var d = new D(); r.push(d.v(x), D.s(x), new o.inner.constructor().constructor === Object, o.m.call(null, x)[0], o.m.apply(o, [x, 1])[1]);
  function tag(strings, v) { return strings.raw.join("|") + v + (this === o); } o.tag = tag; r.push(o.tag`a${x}b`, new (function () { this.made = x; }).prototype.constructor().made);
  var counter = { n: 0, bump: function (by) { this.n += by; return this.n; } }; for (var i = 0; i < 50; i++) counter.bump(i & 3); r.push(counter.n);
  o.n += 1; o.n *= 2; o.s += x; o.inner.deep.count = 1; o.inner.deep.count += 2; r.push(o.n, o.s, o.inner.deep.count, o.g(0));
  return [r, log.join("")];
});
t("returnsInPlace", function () {
  function plain(a) { return a; } function none(a) { a + 1; } function early(a) { if (a > 2) return "big"; for (var i = 0; i < 5; i++) { if (i === a) return "at" + i; } return "end"; }
  function nested(a) { return plain(early(a)) + plain(a); } function finalizer(a) { try { return plain(a); } finally { a = "changed"; } } function over(a) { try { return "try"; } finally { if (a) return "finally"; } }
  function caught(a) { try { if (a) throw new RangeError("x"); return "ok"; } catch (e) { return e.constructor.name; } } function rec(n) { return n <= 0 ? 0 : n + rec(n - 1); } function sw(a) { switch (a) { case 1: return "one"; case 2: return "two"; default: return "many"; } }
  function voided() { return; } function comma(a) { return a, a + 1; } function cond(a) { return a ? plain(a) : none(a); } var arrow = (a) => a * 2, arrowBody = (a) => { return a * 3; };
  function inLoop() { var out = []; for (var i = 0; i < 4; i++) { out.push(plain(i), none(i), early(i), arrow(i)); } return out; }
  function* gen() { yield plain(1); return plain(2); } var g = gen(), genOut = [g.next().value, g.next().value, g.next().done];
  function alternate() { var r = 0; for (var i = 0; i < 40; i++) r += (i & 1 ? plain : arrow)(i) + (i % 3 ? arrowBody : rec)(i % 5); return r; }
  function deep(n) { return n === 0 ? plain("bottom") : deep(n - 1); }
  return [plain(1), none(1), early(1), early(3), early(9), nested(1), finalizer("kept"), over(0), over(1), caught(0), caught(1), rec(30), sw(1), sw(2), sw(3), voided(), comma(1), cond(1), cond(0), inLoop(), genOut, alternate(), deep(200)];
});
})();
all.join("\n")"#;
    let expected = r#"resultWithConstant0: "1 -1 0 0 0 0 0 0 8 5 0 0 0 -0 0 NaN NaN 0 0 \"0s\" 0.5 0 9007199254740991 9007199254740991 0 0|2 0 2 0.5 1 2 0 0 9 4 1 1 0 -1 1 NaN Infinity 2 1 \"1s\" 1.5 9007199254740991 9007199254740992 9007199254740992 1 1|0 -2 -2 -0.5 -1 -2 -1 2147483647 -1 -6 1023 -1 -0 1 -1 NaN -Infinity -2 4294967295 \"-1s\" -0.5 -9007199254740991 9007199254740990 9007199254740990 -1 -1|8 6 14 3.5 1 14 3 3 15 2 7 7 0 -7 1 NaN Infinity 14 7 \"7s\" 7.5 63050394783186936 9007199254740998 9007199254740998 7 7|-6 -8 -14 -3.5 -1 -14 -4 2147483644 -7 -4 1017 -7 -0 7 -1 NaN -Infinity -14 4294967289 \"-7s\" -6.5 -63050394783186936 9007199254740984 9007199254740984 -7 -7|256 254 510 127.5 0 510 127 127 255 250 255 255 0 -255 0 NaN Infinity 510 255 \"255s\" 255.5 2296835809958952700 9007199254741246 9007199254741246 255 255|1024 1022 2046 511.5 0 2046 511 511 1023 1018 1023 1023 0 -1023 0 NaN Infinity 2046 1023 \"1023s\" 1023.5 9214364837600034000 9007199254742014 9007199254742014 1023 1023|2147483648 2147483646 4294967294 1073741823.5 1 -2 1073741823 1073741823 2147483647 2147483642 1023 2147483647 0 -2147483647 1 NaN Infinity -2 2147483647 \"2147483647s\" 2147483647.5 1.9342813104826865e+25 9007201402224638 9007201402224638 2147483647 2147483647|-2147483647 -2147483649 -4294967296 -1073741824 -2 0 -1073741824 1073741824 -2147483640 -2147483643 0 -2147483648 -0 2147483648 -2 NaN -Infinity 0 2147483648 \"-2147483648s\" -2147483647.5 -1.9342813113834065e+25 9007197107257343 9007197107257343 -2147483648 -2147483648|4294967296 4294967294 8589934590 2147483647.5 0 -2 -1 2147483647 -1 -6 1023 4294967295 0 -4294967295 0 NaN Infinity -2 4294967295 \"4294967295s\" 4294967295.5 3.868562621866093e+25 9007203549708286 9007203549708286 -1 -1|9007199254740992 9007199254740990 18014398509481982 4503599627370495.5 1 -2 -1 2147483647 -1 -6 1023 9007199254740991 0 -9007199254740991 1 NaN Infinity -2 4294967295 \"9007199254740991s\" 9007199254740992 8.112963841460666e+31 18014398509481982 18014398509481982 -1 -1|-9007199254740990 -9007199254740992 -18014398509481982 -4503599627370495.5 -1 2 0 0 9 4 1 -9007199254740991 -0 9007199254740991 -1 NaN -Infinity 2 1 \"-9007199254740991s\" -9007199254740990 -8.112963841460666e+31 0 0 1 1|1.5 -0.5 1 0.25 0.5 0 0 0 8 5 0 0.5 0 -0.5 0.5 NaN Infinity 0 0 \"0.5s\" 1 4503599627370495.5 9007199254740992 9007199254740992 0 0|0.5 -1.5 -1 -0.25 -0.5 0 0 0 8 5 0 -0.5 -0 0.5 -0.5 NaN -Infinity 0 0 \"-0.5s\" 0 -4503599627370495.5 9007199254740990 9007199254740990 0 0|1e+21 1e+21 2e+21 500000000000000000000 1 -1119879168 -279969792 1867513856 -559939576 -559939579 0 1e+21 0 -1e+21 1 NaN Infinity -1119879168 3735027712 \"1e+21s\" 1e+21 9.007199254740991e+36 1.0000090071992547e+21 1.0000090071992547e+21 -559939584 -559939584|1 -1 -0 -0 -0 0 0 0 8 5 0 0 -0 0 -0 NaN NaN 0 0 \"0s\" 0.5 -0 9007199254740991 9007199254740991 0 0|NaN NaN NaN NaN NaN 0 0 0 8 5 0 NaN NaN NaN NaN NaN NaN 0 0 \"NaNs\" NaN NaN NaN NaN 0 0|Infinity Infinity Infinity Infinity NaN 0 0 0 8 5 0 Infinity NaN -Infinity NaN NaN Infinity 0 0 \"Infinitys\" Infinity Infinity Infinity Infinity 0 0|-Infinity -Infinity -Infinity -Infinity NaN 0 0 0 8 5 0 -Infinity NaN Infinity NaN NaN -Infinity 0 0 \"-Infinitys\" -Infinity -Infinity -Infinity -Infinity 0 0|\"1\" -1 0 0 0 0 0 0 8 5 0 \"0\" 0 -0 0 NaN NaN 0 0 \"s\" \"0.5\" 0 \"9007199254740991\" 9007199254740991 0 0|\"51\" 4 10 2.5 2 10 2 2 13 0 5 \"50\" 0 -5 2 NaN Infinity 10 5 \"5s\" \"50.5\" 45035996273704950 \"59007199254740991\" 9007199254740996 5 5|\"abc1\" NaN NaN NaN NaN 0 0 0 8 5 0 \"abc0\" NaN NaN NaN NaN NaN 0 0 \"abcs\" \"abc0.5\" NaN \"abc9007199254740991\" NaN 0 0|\" 12 1\" 11 24 6 0 24 6 6 12 9 12 \" 12 0\" 0 -12 0 NaN Infinity 24 12 \" 12 s\" \" 12 0.5\" 108086391056891890 \" 12 9007199254740991\" 9007199254741004 12 12|2 0 2 0.5 1 2 0 0 9 4 1 1 0 -1 1 NaN Infinity 2 1 \"trues\" 1.5 9007199254740991 9007199254740992 9007199254740992 1 1|1 -1 0 0 0 0 0 0 8 5 0 0 0 -0 0 NaN NaN 0 0 \"falses\" 0.5 0 9007199254740991 9007199254740991 0 0|1 -1 0 0 0 0 0 0 8 5 0 0 0 -0 0 NaN NaN 0 0 \"nulls\" 0.5 0 9007199254740991 9007199254740991 0 0|NaN NaN NaN NaN NaN 0 0 0 8 5 0 NaN NaN NaN NaN NaN NaN 0 0 \"undefineds\" NaN NaN NaN NaN 0 0|TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError \"10s\" TypeError TypeError TypeError TypeError TypeError TypeError|TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError \"-3s\" TypeError TypeError TypeError TypeError TypeError TypeError|7 5 12 3 0 12 3 3 14 3 6 6 0 -6 0 NaN Infinity 12 6 \"6s\" 6.5 54043195528445944 9007199254740996 9007199254740996 6 6|\"91\" 8 18 4.5 0 18 4 4 9 12 9 \"90\" 0 -9 0 NaN Infinity 18 9 \"9s\" \"90.5\" 81064793292668910 \"99007199254740991\" 9007199254741000 9 9|\"1\" -1 0 0 0 0 0 0 8 5 0 \"0\" 0 -0 0 NaN NaN 0 0 \"s\" \"0.5\" 0 \"9007199254740991\" 9007199254740991 0 0|\"41\" 3 8 2 1 8 2 2 12 1 4 \"40\" 0 -4 1 NaN Infinity 8 4 \"4s\" \"40.5\" 36028797018963964 \"49007199254740991\" 9007199254740996 4 4|\"[object Object]1\" NaN NaN NaN NaN 0 0 0 8 5 0 \"[object Object]0\" NaN NaN NaN NaN NaN 0 0 \"[object Object]s\" \"[object Object]0.5\" NaN \"[object Object]9007199254740991\" NaN 0 0"
resultWithConstant1: "1 -1 0 0 0 0 0 0 8 5 0 0 0 -0 0 NaN NaN 0 0 \"0s\" 0.5 0 9007199254740991 9007199254740991 0 0|2 0 2 0.5 1 2 0 0 9 4 1 1 0 -1 1 NaN Infinity 2 1 \"1s\" 1.5 9007199254740991 9007199254740992 9007199254740992 1 1|0 -2 -2 -0.5 -1 -2 -1 2147483647 -1 -6 1023 -1 -0 1 -1 NaN -Infinity -2 4294967295 \"-1s\" -0.5 -9007199254740991 9007199254740990 9007199254740990 -1 -1|8 6 14 3.5 1 14 3 3 15 2 7 7 0 -7 1 NaN Infinity 14 7 \"7s\" 7.5 63050394783186936 9007199254740998 9007199254740998 7 7|-6 -8 -14 -3.5 -1 -14 -4 2147483644 -7 -4 1017 -7 -0 7 -1 NaN -Infinity -14 4294967289 \"-7s\" -6.5 -63050394783186936 9007199254740984 9007199254740984 -7 -7|256 254 510 127.5 0 510 127 127 255 250 255 255 0 -255 0 NaN Infinity 510 255 \"255s\" 255.5 2296835809958952700 9007199254741246 9007199254741246 255 255|1024 1022 2046 511.5 0 2046 511 511 1023 1018 1023 1023 0 -1023 0 NaN Infinity 2046 1023 \"1023s\" 1023.5 9214364837600034000 9007199254742014 9007199254742014 1023 1023|2147483648 2147483646 4294967294 1073741823.5 1 -2 1073741823 1073741823 2147483647 2147483642 1023 2147483647 0 -2147483647 1 NaN Infinity -2 2147483647 \"2147483647s\" 2147483647.5 1.9342813104826865e+25 9007201402224638 9007201402224638 2147483647 2147483647|-2147483647 -2147483649 -4294967296 -1073741824 -2 0 -1073741824 1073741824 -2147483640 -2147483643 0 -2147483648 -0 2147483648 -2 NaN -Infinity 0 2147483648 \"-2147483648s\" -2147483647.5 -1.9342813113834065e+25 9007197107257343 9007197107257343 -2147483648 -2147483648|4294967296 4294967294 8589934590 2147483647.5 0 -2 -1 2147483647 -1 -6 1023 4294967295 0 -4294967295 0 NaN Infinity -2 4294967295 \"4294967295s\" 4294967295.5 3.868562621866093e+25 9007203549708286 9007203549708286 -1 -1|9007199254740992 9007199254740990 18014398509481982 4503599627370495.5 1 -2 -1 2147483647 -1 -6 1023 9007199254740991 0 -9007199254740991 1 NaN Infinity -2 4294967295 \"9007199254740991s\" 9007199254740992 8.112963841460666e+31 18014398509481982 18014398509481982 -1 -1|-9007199254740990 -9007199254740992 -18014398509481982 -4503599627370495.5 -1 2 0 0 9 4 1 -9007199254740991 -0 9007199254740991 -1 NaN -Infinity 2 1 \"-9007199254740991s\" -9007199254740990 -8.112963841460666e+31 0 0 1 1|1.5 -0.5 1 0.25 0.5 0 0 0 8 5 0 0.5 0 -0.5 0.5 NaN Infinity 0 0 \"0.5s\" 1 4503599627370495.5 9007199254740992 9007199254740992 0 0|0.5 -1.5 -1 -0.25 -0.5 0 0 0 8 5 0 -0.5 -0 0.5 -0.5 NaN -Infinity 0 0 \"-0.5s\" 0 -4503599627370495.5 9007199254740990 9007199254740990 0 0|1e+21 1e+21 2e+21 500000000000000000000 1 -1119879168 -279969792 1867513856 -559939576 -559939579 0 1e+21 0 -1e+21 1 NaN Infinity -1119879168 3735027712 \"1e+21s\" 1e+21 9.007199254740991e+36 1.0000090071992547e+21 1.0000090071992547e+21 -559939584 -559939584|1 -1 -0 -0 -0 0 0 0 8 5 0 0 -0 0 -0 NaN NaN 0 0 \"0s\" 0.5 -0 9007199254740991 9007199254740991 0 0|NaN NaN NaN NaN NaN 0 0 0 8 5 0 NaN NaN NaN NaN NaN NaN 0 0 \"NaNs\" NaN NaN NaN NaN 0 0|Infinity Infinity Infinity Infinity NaN 0 0 0 8 5 0 Infinity NaN -Infinity NaN NaN Infinity 0 0 \"Infinitys\" Infinity Infinity Infinity Infinity 0 0|-Infinity -Infinity -Infinity -Infinity NaN 0 0 0 8 5 0 -Infinity NaN Infinity NaN NaN -Infinity 0 0 \"-Infinitys\" -Infinity -Infinity -Infinity -Infinity 0 0|\"1\" -1 0 0 0 0 0 0 8 5 0 \"0\" 0 -0 0 NaN NaN 0 0 \"s\" \"0.5\" 0 \"9007199254740991\" 9007199254740991 0 0|\"51\" 4 10 2.5 2 10 2 2 13 0 5 \"50\" 0 -5 2 NaN Infinity 10 5 \"5s\" \"50.5\" 45035996273704950 \"59007199254740991\" 9007199254740996 5 5|\"abc1\" NaN NaN NaN NaN 0 0 0 8 5 0 \"abc0\" NaN NaN NaN NaN NaN 0 0 \"abcs\" \"abc0.5\" NaN \"abc9007199254740991\" NaN 0 0|\" 12 1\" 11 24 6 0 24 6 6 12 9 12 \" 12 0\" 0 -12 0 NaN Infinity 24 12 \" 12 s\" \" 12 0.5\" 108086391056891890 \" 12 9007199254740991\" 9007199254741004 12 12|2 0 2 0.5 1 2 0 0 9 4 1 1 0 -1 1 NaN Infinity 2 1 \"trues\" 1.5 9007199254740991 9007199254740992 9007199254740992 1 1|1 -1 0 0 0 0 0 0 8 5 0 0 0 -0 0 NaN NaN 0 0 \"falses\" 0.5 0 9007199254740991 9007199254740991 0 0|1 -1 0 0 0 0 0 0 8 5 0 0 0 -0 0 NaN NaN 0 0 \"nulls\" 0.5 0 9007199254740991 9007199254740991 0 0|NaN NaN NaN NaN NaN 0 0 0 8 5 0 NaN NaN NaN NaN NaN NaN 0 0 \"undefineds\" NaN NaN NaN NaN 0 0|TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError \"10s\" TypeError TypeError TypeError TypeError TypeError TypeError|TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError TypeError \"-3s\" TypeError TypeError TypeError TypeError TypeError TypeError|7 5 12 3 0 12 3 3 14 3 6 6 0 -6 0 NaN Infinity 12 6 \"6s\" 6.5 54043195528445944 9007199254740996 9007199254740996 6 6|\"91\" 8 18 4.5 0 18 4 4 9 12 9 \"90\" 0 -9 0 NaN Infinity 18 9 \"9s\" \"90.5\" 81064793292668910 \"99007199254740991\" 9007199254741000 9 9|\"1\" -1 0 0 0 0 0 0 8 5 0 \"0\" 0 -0 0 NaN NaN 0 0 \"s\" \"0.5\" 0 \"9007199254740991\" 9007199254740991 0 0|\"41\" 3 8 2 1 8 2 2 12 1 4 \"40\" 0 -4 1 NaN Infinity 8 4 \"4s\" \"40.5\" 36028797018963964 \"49007199254740991\" 9007199254740996 4 4|\"[object Object]1\" NaN NaN NaN NaN 0 0 0 8 5 0 \"[object Object]0\" NaN NaN NaN NaN NaN 0 0 \"[object Object]s\" \"[object Object]0.5\" NaN \"[object Object]9007199254740991\" NaN 0 0"
localWithConstant0: "1 -1 0 0 0 0 0 0 1 1 0 0 NaN \"0\" -0.25|2 0 3 0.25 1 4 0 0 1 0 1 0 NaN \"1\" 0.75|0 -2 -3 -0.25 -1 -4 -1 1073741823 -1 -2 255 -0 NaN \"-1\" -1.25|8 6 21 1.75 0 28 1 1 7 6 7 0 NaN \"7\" 6.75|-6 -8 -21 -1.75 -0 -28 -2 1073741822 -7 -8 249 -0 NaN \"-7\" -7.25|256 254 765 63.75 3 1020 63 63 255 254 255 0 NaN \"255\" 254.75|1024 1022 3069 255.75 1 4092 255 255 1023 1022 255 0 NaN \"1023\" 1022.75|2147483648 2147483646 6442450941 536870911.75 1 -4 536870911 536870911 2147483647 2147483646 255 0 NaN \"2147483647\" 2147483646.75|-2147483647 -2147483649 -6442450944 -536870912 -2 0 -536870912 536870912 -2147483647 -2147483647 0 -0 NaN \"-2147483648\" -2147483648.25|4294967296 4294967294 12884901885 1073741823.75 3 -4 -1 1073741823 -1 -2 255 0 NaN \"4294967295\" 4294967294.75|9007199254740992 9007199254740990 27021597764222972 2251799813685247.8 3 -4 -1 1073741823 -1 -2 255 0 NaN \"9007199254740991\" 9007199254740991|-9007199254740990 -9007199254740992 -27021597764222972 -2251799813685247.8 -3 4 0 0 1 0 1 -0 NaN \"-9007199254740991\" -9007199254740991|1.5 -0.5 1.5 0.125 0.5 0 0 0 1 1 0 0 NaN \"0.5\" 0.25|0.5 -1.5 -1.5 -0.125 -0.5 0 0 0 1 1 0 -0 NaN \"-0.5\" -0.75|1e+21 1e+21 3e+21 250000000000000000000 6 2055208960 -139984896 933756928 -559939583 -559939583 0 0 NaN \"1e+21\" 1e+21|1 -1 -0 -0 -0 0 0 0 1 1 0 -0 NaN \"0\" -0.25|NaN NaN NaN NaN NaN 0 0 0 1 1 0 NaN NaN \"NaN\" NaN|Infinity Infinity Infinity Infinity NaN 0 0 0 1 1 0 NaN NaN \"Infinity\" Infinity|-Infinity -Infinity -Infinity -Infinity NaN 0 0 0 1 1 0 NaN NaN \"-Infinity\" -Infinity|\"1\" -1 0 0 0 0 0 0 1 1 0 0 NaN \"\" -0.25|\"51\" 4 15 1.25 5 20 1 1 5 4 5 0 NaN \"5\" 4.75|\"abc1\" NaN NaN NaN NaN 0 0 0 1 1 0 NaN NaN \"abc\" NaN|\" 12 1\" 11 36 3 5 48 3 3 13 13 12 0 NaN \" 12 \" 11.75|2 0 3 0.25 1 4 0 0 1 0 1 0 NaN \"true\" 0.75|1 -1 0 0 0 0 0 0 1 1 0 0 NaN \"false\" -0.25|1 -1 0 0 0 0 0 0 1 1 0 0 NaN \"null\" -0.25|NaN NaN NaN NaN NaN 0 0 0 1 1 0 NaN NaN \"undefined\" NaN|E E E E E E E E E E E E E \"10\" E|E E E E E E E E E E E E E \"-3\" E|7 5 18 1.5 6 24 1 1 7 7 6 0 NaN \"6\" 5.75|\"91\" 8 27 2.25 2 36 2 2 9 8 9 0 NaN \"9\" 8.75|\"1\" -1 0 0 0 0 0 0 1 1 0 0 NaN \"\" -0.25|\"41\" 3 12 1 4 16 1 1 5 5 4 0 NaN \"4\" 3.75|\"[object Object]1\" NaN NaN NaN NaN 0 0 0 1 1 0 NaN NaN \"[object Object]\" NaN"
localWithConstant1: "1 -1 0 0 0 0 0 0 1 1 0 0 NaN \"0\" -0.25|2 0 3 0.25 1 4 0 0 1 0 1 0 NaN \"1\" 0.75|0 -2 -3 -0.25 -1 -4 -1 1073741823 -1 -2 255 -0 NaN \"-1\" -1.25|8 6 21 1.75 0 28 1 1 7 6 7 0 NaN \"7\" 6.75|-6 -8 -21 -1.75 -0 -28 -2 1073741822 -7 -8 249 -0 NaN \"-7\" -7.25|256 254 765 63.75 3 1020 63 63 255 254 255 0 NaN \"255\" 254.75|1024 1022 3069 255.75 1 4092 255 255 1023 1022 255 0 NaN \"1023\" 1022.75|2147483648 2147483646 6442450941 536870911.75 1 -4 536870911 536870911 2147483647 2147483646 255 0 NaN \"2147483647\" 2147483646.75|-2147483647 -2147483649 -6442450944 -536870912 -2 0 -536870912 536870912 -2147483647 -2147483647 0 -0 NaN \"-2147483648\" -2147483648.25|4294967296 4294967294 12884901885 1073741823.75 3 -4 -1 1073741823 -1 -2 255 0 NaN \"4294967295\" 4294967294.75|9007199254740992 9007199254740990 27021597764222972 2251799813685247.8 3 -4 -1 1073741823 -1 -2 255 0 NaN \"9007199254740991\" 9007199254740991|-9007199254740990 -9007199254740992 -27021597764222972 -2251799813685247.8 -3 4 0 0 1 0 1 -0 NaN \"-9007199254740991\" -9007199254740991|1.5 -0.5 1.5 0.125 0.5 0 0 0 1 1 0 0 NaN \"0.5\" 0.25|0.5 -1.5 -1.5 -0.125 -0.5 0 0 0 1 1 0 -0 NaN \"-0.5\" -0.75|1e+21 1e+21 3e+21 250000000000000000000 6 2055208960 -139984896 933756928 -559939583 -559939583 0 0 NaN \"1e+21\" 1e+21|1 -1 -0 -0 -0 0 0 0 1 1 0 -0 NaN \"0\" -0.25|NaN NaN NaN NaN NaN 0 0 0 1 1 0 NaN NaN \"NaN\" NaN|Infinity Infinity Infinity Infinity NaN 0 0 0 1 1 0 NaN NaN \"Infinity\" Infinity|-Infinity -Infinity -Infinity -Infinity NaN 0 0 0 1 1 0 NaN NaN \"-Infinity\" -Infinity|\"1\" -1 0 0 0 0 0 0 1 1 0 0 NaN \"\" -0.25|\"51\" 4 15 1.25 5 20 1 1 5 4 5 0 NaN \"5\" 4.75|\"abc1\" NaN NaN NaN NaN 0 0 0 1 1 0 NaN NaN \"abc\" NaN|\" 12 1\" 11 36 3 5 48 3 3 13 13 12 0 NaN \" 12 \" 11.75|2 0 3 0.25 1 4 0 0 1 0 1 0 NaN \"true\" 0.75|1 -1 0 0 0 0 0 0 1 1 0 0 NaN \"false\" -0.25|1 -1 0 0 0 0 0 0 1 1 0 0 NaN \"null\" -0.25|NaN NaN NaN NaN NaN 0 0 0 1 1 0 NaN NaN \"undefined\" NaN|E E E E E E E E E E E E E \"10\" E|E E E E E E E E E E E E E \"-3\" E|7 5 18 1.5 6 24 1 1 7 7 6 0 NaN \"6\" 5.75|\"91\" 8 27 2.25 2 36 2 2 9 8 9 0 NaN \"9\" 8.75|\"1\" -1 0 0 0 0 0 0 1 1 0 0 NaN \"\" -0.25|\"41\" 3 12 1 4 16 1 1 5 5 4 0 NaN \"4\" 3.75|\"[object Object]1\" NaN NaN NaN NaN 0 0 0 1 1 0 NaN NaN \"[object Object]\" NaN"
twoLocalsThenConstant0: "3 -3 3 17 6 3 3 0|1255 -1046 1024000 8184 3047 23 1003 535|254 1 -0 -1 -2 30 -2 -2147483648|3.5 -5.5 2 24 7.5 3 3.5 1|\"s4\" NaN NaN 32 \"s81\" 4 \"s4\" 2|9 -12 142 58 17 9 4 1|5 -2 10.5 19 9 5 5.5 2|1 1 1 1 2 1 1 1|NaN NaN NaN 8 NaN 1 NaN 0"
twoLocalsThenConstant1: "3 -3 3 17 6 3 3 0|1255 -1046 1024000 8184 3047 23 1003 535|254 1 -0 -1 -2 30 -2 -2147483648|3.5 -5.5 2 24 7.5 3 3.5 1|\"s4\" NaN NaN 32 \"s81\" 4 \"s4\" 2|9 -12 142 58 17 9 4 1|5 -2 10.5 19 9 5 5.5 2|1 1 1 1 2 1 1 1|NaN NaN NaN 8 NaN 1 NaN 0"
constantOverflowAndZero0: "9007199254740992 18014398509481982 0 -9007199254740992 -18014398509481982 9007199515875288 -9007199515875288 -0 0 0 -0 -1 1 0 -2147483648 -2147483648 -2147483648"
constantOverflowAndZero1: "9007199254740992 18014398509481982 0 -9007199254740992 -18014398509481982 9007199515875288 -9007199515875288 -0 0 0 -0 -1 1 0 -2147483648 -2147483648 -2147483648"
valueOfOrderAndEffects0: "4,6,1,2|71,14,1,6|TypeError|TypeError|8n|string|-1|RangeError|vvvvssss"
valueOfOrderAndEffects1: "4,6,1,2|71,14,1,6|TypeError|TypeError|8n|string|-1|RangeError|vvvvssss"
compoundAndProperties0: "2 \"s1\" 3 [0,17,2] 12 9 \"get\" \"set3\" \"get\" \"set16\""
compoundAndProperties1: "2 \"s1\" 3 [0,17,2] 12 9 \"get\" \"set3\" \"get\" \"set16\""
closureOperands0: "5 3 8 0 \"str1\" NaN NaN 11 8 9 12 15 9 19 20 [9,1,\"str\",2] 9 \"10str\" true false true true true true false|10 8 18 1 \"str1\" NaN NaN 11 18 19 22 20 9 19 20 [9,1,\"str\",2] 9 \"10str\" true false true true true true false"
closureOperands1: "5 3 8 0 \"str1\" NaN NaN 11 8 9 12 15 9 19 20 [9,1,\"str\",2] 9 \"10str\" true false true true true true false|10 8 18 1 \"str1\" NaN NaN 11 18 19 22 20 9 19 20 [9,1,\"str\",2] 9 \"10str\" true false true true true true false"
closureTdzAndLate0: "ReferenceError ReferenceError 6 5,1 gone 3 gone ReferenceError"
closureTdzAndLate1: "ReferenceError ReferenceError 6 5,1 gone 3 gone ReferenceError"
localIndexReads0: "10/5/\"h\"/\"zero\" 20/-6/\"é\"/undefined undefined/7/\"l\"/undefined 40/undefined/\"l\"/undefined undefined/undefined/\"o\"/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/\"frac\" 20/-6/\"é\"/undefined undefined/undefined/undefined/\"kay\" 4/3/5/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined 20/-6/\"é\"/undefined 10/5/\"h\"/\"zero\" fn/fn/fn/undefined"
localIndexReads1: "10/5/\"h\"/\"zero\" 20/-6/\"é\"/undefined undefined/7/\"l\"/undefined 40/undefined/\"l\"/undefined undefined/undefined/\"o\"/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/\"frac\" 20/-6/\"é\"/undefined undefined/undefined/undefined/\"kay\" 4/3/5/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined undefined/undefined/undefined/undefined 20/-6/\"é\"/undefined 10/5/\"h\"/\"zero\" fn/fn/fn/undefined"
localIndexBases0: "\"get\" \"make\" 8 2 \"b\" 4 2 500 \"y\" 6 2 \"none\" \"trap:1\" 2 \"elem\" \"g\" 8 \"inherited\" undefined \"TypeError\" \"TypeError\" 5 2.5 44"
localIndexBases1: "\"get\" \"make\" 8 2 \"b\" 4 2 500 \"y\" 6 2 \"none\" \"trap:1\" 2 \"elem\" \"g\" 8 \"inherited\" undefined \"TypeError\" \"TypeError\" 5 2.5 44"
pushedConstants0: "\"3,1\" \"1,3\" \"1,2,3\" \"3,\\\"s\\\",null,undefined,true,1.5,-0\" \"3,7\" \"4,2,6\" [3,1,2] [1,3] 1 7 5 5 1 \"\\\"3,1\\\",2\" undefined undefined \"t13u2\" \"a3b1c\" \"number1\" -2 1 NaN"
pushedConstants1: "\"3,1\" \"1,3\" \"1,2,3\" \"3,\\\"s\\\",null,undefined,true,1.5,-0\" \"3,7\" \"4,2,6\" [3,1,2] [1,3] 1 7 5 5 1 \"\\\"3,1\\\",2\" undefined undefined \"t13u2\" \"a3b1c\" \"number1\" -2 1 NaN"
eval: 5 undefined 5 2 8 2 2 2
referencesKept0: [[7,,3],[1,7,9],9,true,"number",false,"7",[7,1],15,6,[[1,1],[3,40]],"b1","gs4gs6"]
referencesKept1: [[7,,3],[1,7,9],9,true,"number",false,"7",[7,1],15,6,[[1,1],[3,40]],"b1","gs4gs6"]
pushedInManyPlaces0: [[2,1],[1,2,2],[1,2,1],[2,1],"a-b-c21","21s",2,1,2,1,1,1,[2,1,,2],{2:2,a:1,b:2},[1,2,1,1,2],3,3,1,1,"11s","212","32",3,-3,"number1",true]
pushedInManyPlaces1: [[2,1],[1,2,2],[1,2,1],[2,1],"a-b-c21","21s",2,1,2,1,1,1,[2,1,,2],{2:2,a:1,b:2},[1,2,1,1,2],3,3,1,1,"11s","212","32",3,-3,"number1",true]
throwsMidExpression0: [[1,20,0,4],12,20,"TypeError",30,[1,20,1,4],14,20,"TypeError",40,"RangeError","RangeError","RangeError","TypeError",50,[1,20,3,4],18,20,"TypeError",30]
throwsMidExpression1: [[1,20,0,4],12,20,"TypeError",30,[1,20,1,4],14,20,"TypeError",40,"RangeError","RangeError","RangeError","TypeError",50,[1,20,3,4],18,20,"TypeError",30]
closuresInCalls0: [[62,62,68,"b00b5437020NaN",44.5,"12z2z21201.5",55,[11,12,13]],21]
closuresInCalls1: [[62,62,68,"b00b5437020NaN",44.5,"12z2z21201.5",55,[11,12,13]],21]
generatorsAndAsync0: [[7,88,[6,299,1,7],310,true],[1,3,7,9,false],0]
generatorsAndAsync1: [[7,88,[6,299,1,7],310,true],[1,3,7,9,false],0]
loopsWithFusedBodies0: [4877,396,"391671140278225138933624904473501558461",832,9007199254743900,220,-3,[-40,-13,14,41,-112,-94,-76,-58],[105,108,114,116,121]]
loopsWithFusedBodies1: [4877,396,"391671140278225138933624904473501558461",832,9007199254743900,220,-3,[-40,-13,14,41,-112,-94,-76,-58],[105,108,114,116,121]]
numberOperandsOfAnyKind0: [1334600691,"10/0/0/0/0/0/0 12/0/0/0/0/0/0 3/0/0/0/0/0/0 12/1073741824/0/0/-1/-1/0 3/-2/-1/2147483647/-1/-2/1 3/-1/-1/4294967295/-1/-1/0 12/7/7/7/559939591/559939591/0 3/0/0/0/2147483647/2147483647/0 3/0/0/0/7/7/0 3/1/1/1/1/1/0 12/0/0/0/2147483647/2147483645/2 3/-2/-2/4294967294/-2/-2/0 3/255/255/255/255/255/0 3/-2147483648/0/0/-1/-2147483648/2147483647 12/0/-536870912/536870912/-2147483646/-2147483646/0 3/2147483647/2147483647/2147483647/2147483647/2147483647/0 3/-2097152/1023/1023/2147483647/2024026858/123456789 3/-1/-1/4294967295/-1/559939583/-559939584 12/-2147483648/-1/1/-1/-256/255 12/0/0/0/-1/-1/0 12/-559939584/-559939584/3735027712/-559939584/-559939584/0 3/559939584/559939584/559939584/-1587544064/-1587544064/0 3/0/0/0/0/0/0 0/0/0/0/0/0/0 0/0/0/0/-1/-1/0 12/0/0/0/1/1/0 3/0/0/0/0/0/0 10/0/0/0/0/0/0 3/123456789/123456789/123456789/123456789/123456789/0 12/0/0/0/-2/-2/0","false/true/true/false/3/2/6/0/3 true/false/false/true/2/0/0/2/0 false/false/false/false/2/0/0/2/0 true/false/false/true/3/0/2/1/1 true/false/false/true/2/0/0/2/0 false/false/false/false/2/0/0/2/0 false/true/false/true/2/2/4/0/2 false/true/true/false/7/0/10/0/5 TypeError",35,129.246337890625,0]
numberOperandsOfAnyKind1: [1334600691,"10/0/0/0/0/0/0 12/0/0/0/0/0/0 3/0/0/0/0/0/0 12/1073741824/0/0/-1/-1/0 3/-2/-1/2147483647/-1/-2/1 3/-1/-1/4294967295/-1/-1/0 12/7/7/7/559939591/559939591/0 3/0/0/0/2147483647/2147483647/0 3/0/0/0/7/7/0 3/1/1/1/1/1/0 12/0/0/0/2147483647/2147483645/2 3/-2/-2/4294967294/-2/-2/0 3/255/255/255/255/255/0 3/-2147483648/0/0/-1/-2147483648/2147483647 12/0/-536870912/536870912/-2147483646/-2147483646/0 3/2147483647/2147483647/2147483647/2147483647/2147483647/0 3/-2097152/1023/1023/2147483647/2024026858/123456789 3/-1/-1/4294967295/-1/559939583/-559939584 12/-2147483648/-1/1/-1/-256/255 12/0/0/0/-1/-1/0 12/-559939584/-559939584/3735027712/-559939584/-559939584/0 3/559939584/559939584/559939584/-1587544064/-1587544064/0 3/0/0/0/0/0/0 0/0/0/0/0/0/0 0/0/0/0/-1/-1/0 12/0/0/0/1/1/0 3/0/0/0/0/0/0 10/0/0/0/0/0/0 3/123456789/123456789/123456789/123456789/123456789/0 12/0/0/0/-2/-2/0","false/true/true/false/3/2/6/0/3 true/false/false/true/2/0/0/2/0 false/false/false/false/2/0/0/2/0 true/false/false/true/3/0/2/1/1 true/false/false/true/2/0/0/2/0 false/false/false/false/2/0/0/2/0 false/true/false/true/2/2/4/0/2 false/true/true/false/7/0/10/0/5 TypeError",35,129.246337890625,0]
numberArithmeticOfAnyKind0: [-1979055476,"0/0/0/NaN/NaN 1.3333333333333333/0.6666666666666667/0.3333333333333333/3/5.551115123125783e-17 1.7976931348623157e+308/-1.7976931348623157e+308/-1.7976931348623157e+308/-5.562684646268003e-309/-1 -9007199254740988/9007199254740994/-27021597764222972/-3.33066907387547e-16/3 262.9/-248.9/1791.3/0.027354435326299335/7 0/1/-0.25/-1/0 0.5/-1.5/-0.5/-0.5/-0.5 Infinity/-Infinity/Infinity/0/0.1 1e+21/-1e+21/200000000000000000000/2e-22/0.2 -2147483392.1/2147483903.9/-549541065523.2/-1.1916272342205048e-7/255.9 2147483644.5/2147483649.5/-5368709117.5/-858993458.8/2 -2147483645/-2147483651/-6442450944/-715827882.6666666/-2 -9007199131284202/-9007199378197780/-1.1119998989994157e+24/-72958314.5534524/-68327455.75 9007199254740992/9007199254740992/4.450147717014403e-308/Infinity/0 1.0000090071992547e+21/999990992800745300000/9.007199254740991e+36/111022.30246251567/2724340145697198 -1e+21/-1e+21/-200000000000000000000/-5e+21/-0.04876874217606031 0.5/-0.5/0/1e-323/5e-324 1.7976931348623157e+308/1.7976931348623157e+308/-0/-Infinity/NaN NaN/NaN/NaN/NaN/NaN 9007199378197782/-9007199131284203/1.1119998989994158e+24/1.3706456983286763e-8/123456789.125 2147483647.3333333/-2147483646.6666667/715827882.3333333/1.552204291748599e-10/0.3333333333333333"]
numberArithmeticOfAnyKind1: [-1979055476,"0/0/0/NaN/NaN 1.3333333333333333/0.6666666666666667/0.3333333333333333/3/5.551115123125783e-17 1.7976931348623157e+308/-1.7976931348623157e+308/-1.7976931348623157e+308/-5.562684646268003e-309/-1 -9007199254740988/9007199254740994/-27021597764222972/-3.33066907387547e-16/3 262.9/-248.9/1791.3/0.027354435326299335/7 0/1/-0.25/-1/0 0.5/-1.5/-0.5/-0.5/-0.5 Infinity/-Infinity/Infinity/0/0.1 1e+21/-1e+21/200000000000000000000/2e-22/0.2 -2147483392.1/2147483903.9/-549541065523.2/-1.1916272342205048e-7/255.9 2147483644.5/2147483649.5/-5368709117.5/-858993458.8/2 -2147483645/-2147483651/-6442450944/-715827882.6666666/-2 -9007199131284202/-9007199378197780/-1.1119998989994157e+24/-72958314.5534524/-68327455.75 9007199254740992/9007199254740992/4.450147717014403e-308/Infinity/0 1.0000090071992547e+21/999990992800745300000/9.007199254740991e+36/111022.30246251567/2724340145697198 -1e+21/-1e+21/-200000000000000000000/-5e+21/-0.04876874217606031 0.5/-0.5/0/1e-323/5e-324 1.7976931348623157e+308/1.7976931348623157e+308/-0/-Infinity/NaN NaN/NaN/NaN/NaN/NaN 9007199378197782/-9007199131284203/1.1119998989994158e+24/1.3706456983286763e-8/123456789.125 2147483647.3333333/-2147483646.6666667/715827882.3333333/1.552204291748599e-10/0.3333333333333333"]
numbersMadeInLoops0: ["450260545.113","0.407591","112588485.016","0.044776","1099750.000",2199.2000000000003]
numbersMadeInLoops1: ["450260545.113","0.407591","112588485.016","0.044776","1099750.000",2199.2000000000003]
numberTestsThatJump0: [1419684036,"1767,1767,1509,739,1444,2021,739,1508,1452,1452,1452,1452,1452,1324,739,1765,0,1324,739,1765,1765","3340,3434,0167,0000,1160,5520,3434,0167,0000,3340,101000,0078",21,3,3,-0.75,10]
numberTestsThatJump1: [1419684036,"1767,1767,1509,739,1444,2021,739,1508,1452,1452,1452,1452,1452,1324,739,1765,0,1324,739,1765,1765","3340,3434,0167,0000,1160,5520,3434,0167,0000,3340,101000,0078",21,3,3,-0.75,10]
operatorResultToLocal0: [-1279780843,"0 0 0 NaN NaN 0 0 0 0 0 0","1 -1 0 0 0 0 0 0 1 1 0","-1 0 0.25 1 -0 0 0 0 0 0 0","\"[object Object]2\" NaN NaN NaN NaN 0 0 0 2 2 0"]
operatorResultToLocal1: [-1279780843,"0 0 0 NaN NaN 0 0 0 0 0 0","1 -1 0 0 0 0 0 0 1 1 0","-1 0 0.25 1 -0 0 0 0 0 0 0","\"[object Object]2\" NaN NaN NaN NaN 0 0 0 2 2 0"]
targetIsAnOperand0: [12,6,"s400.5",0.25,120,19,NaN,1,"TypeError","vvvvv"]
targetIsAnOperand1: [12,6,"s400.5",0.25,120,19,NaN,1,"TypeError","vvvvv"]
operatorResultPushed0: [[10],[24,2],[4,1,6],[10,1.5,"x6"],[2],[12,2],20,91,"10x1.5","x64",2,true,true,30,30,26,-10,"number",true,"10","1024","big",10,24,10,22]
operatorResultPushed1: [[10],[24,2],[4,1,6],[10,1.5,"x6"],[2],[12,2],20,91,"10x1.5","x64",2,true,true,30,30,26,-10,"number",true,"10","1024","big",10,24,10,22]
pushedThenThrows0: [3,3,2,5,5,"RangeError","RangeError","RangeError","RangeError5",5,3,3,4,7,5]
pushedThenThrows1: [3,3,2,5,5,"RangeError","RangeError","RangeError","RangeError5",5,3,3,4,7,5]
methodCalls0: [[[true,0],[true,1,3],[true,1,1],[true,3,3,1,"s"],[true,2,4,6],[true,1,3],4,2,3,[true,1,3],116,3,"5.0","bc3","13233",[true,1,3],undefined,undefined,"TypeError","TypeError","TypeError","TypeError","RangeError",3,6,2,"B4w3","S3!",true,false,2,"a|b3true",3,73,4,"str3",3,4],"ggget:fget:fg"]
methodCalls1: [[[true,0],[true,1,3],[true,1,1],[true,3,3,1,"s"],[true,2,4,6],[true,1,3],4,2,3,[true,1,3],116,3,"5.0","bc3","13233",[true,1,3],undefined,undefined,"TypeError","TypeError","TypeError","TypeError","RangeError",3,6,2,"B4w3","S3!",true,false,2,"a|b3true",3,73,4,"str3",3,4],"ggget:fget:fg"]
returnsInPlace0: [1,undefined,"at1","big","big","at11","kept","try","finally","ok","RangeError",465,"one","two","many",undefined,2,1,undefined,[0,undefined,"at0",0,1,undefined,"at1",2,2,undefined,"at2",4,3,undefined,"big",6],[1,2,true],1373,"bottom"]
returnsInPlace1: [1,undefined,"at1","big","big","at11","kept","try","finally","ok","RangeError",465,"one","two","many",undefined,2,1,undefined,[0,undefined,"at0",0,1,undefined,"at1",2,2,undefined,"at2",4,3,undefined,"big",6],[1,2,true],1373,"bottom"]"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The `+` of a String and any other primitive, which converts the other
/// one where it stands, and of two strings that make a short one, which
/// the heap is asked for before it is copied: every primitive on either
/// side, Symbols and objects (which still convert the general way), the
/// lengths around the limits of a small string, a short and a long
/// concatenation, surrogates that join into a pair, and that equal texts
/// are one string however they were made. Then elements appended to
/// Arrays, which look at their prototypes' keys only once some prototype
/// was given an integer key: a setter, a read-only property and an
/// element under an index on %Object.prototype% and %Array.prototype%,
/// and Arrays that cannot grow. What is expected is what Node prints for
/// the same script.
#[test]
fn concatenations_and_appended_elements_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(concatenations_and_appended_elements)
        .expect("thread")
        .join()
        .expect("checks");
}

fn concatenations_and_appended_elements() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
function t(name, f) { for (var round = 0; round < 2; round++) { var v; try { v = f(); } catch (e) { v = "threw " + (e && e.constructor ? e.constructor.name : typeof e); } print(name + round + ": " + v); } }
function hash(s) { var h = 7; for (var i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) | 0; return h; }
var prims = [0, -0, 1, -1, 42, 255, 1234567, 12345678, 4294967296, 9007199254740991, 1e21, 1e-7, 0.1, 0.30000000000000004, -2.5, NaN, Infinity, -Infinity, true, false, null, undefined, 10n, -5n, 123456789012345678901234567890n, "", "a", "seven77", "eight888", "é", "😀", "\ud83d", "\ude00"];
t("stringWithPrimitive", function () {
  var rows = [], lefts = ["", "x", "rgb(", "a long enough string to live on the heap ", "é", "\ud83d"];
  for (var i = 0; i < lefts.length; i++) for (var j = 0; j < prims.length; j++) { var l = lefts[i], p = prims[j]; rows.push(l + p, p + l, l + p + l, p + l + p); }
  var all = rows.join("|");
  return [rows.length, all.length, hash(all), JSON.stringify([rows[5], rows[130], rows[400], rows[rows.length - 3]])].join(" ");
});
t("symbolsAndObjects", function () {
  var r = [], log = [];
  try { r.push("s" + Symbol("q")); } catch (e) { r.push(e.constructor.name); } try { r.push(Symbol("q") + "s"); } catch (e) { r.push(e.constructor.name); } try { r.push(1 + Symbol()); } catch (e) { r.push(e.constructor.name); } try { r.push("a" + Object(Symbol())); } catch (e) { r.push(e.constructor.name); }
  var o = { valueOf: function () { log.push("v"); return 7; }, toString: function () { log.push("s"); return "str"; } }, p = { toString: function () { log.push("p"); return "P"; }, valueOf: undefined }, d = new Date(0);
  r.push("a" + o, o + "a", o + o, "a" + p, p + 1, [1, 2] + "x", "x" + [3], "x" + {}, "x" + function f() {}.name, "y" + null + undefined + true, typeof (d + ""), ("" + d).length > 10, 1 + 2 + "3", "1" + 2 + 3, 1 + "2" + 3, "5" + -0, "" + -0, `${-0}` + 0, null + "x", undefined + "", true + "!", 1n + "n", "n" + 2n);
  var thrower = { toString: function () { throw new RangeError("t"); } }; try { r.push("a" + thrower); } catch (e) { r.push(e.constructor.name); } try { r.push(thrower + "a"); } catch (e) { r.push(e.constructor.name); }
  return r.join(",") + "|" + log.join("");
});
t("lengthsAroundTheLimits", function () {
  var r = [], base = "0123456789abcdef";
  var lens = [0, 1, 3, 6, 7, 8, 9, 15, 31, 32, 33, 62, 63, 64, 65, 127, 128, 254, 255, 256, 257, 300, 1000];
  for (var i = 0; i < lens.length; i++) {
    var n = lens[i], left = "", k = 0; while (left.length < n) left += base[k++ & 15];
    var a = left + "Z", b = "Z" + left, c = left + 5, d = left + left, e = left + "é", f = left + "\ud83d" + "\ude00";
    r.push([a.length, b.length, c.length, d.length, e.length, f.length, a === left + "Z", a === "Z" + left, d === left + left, a.charCodeAt(n), b.charCodeAt(0), f.codePointAt(n), hash(a + b + c)].join("/"));
  }
  return r.join(" ");
});
t("surrogatesJoin", function () {
  var lead = "\ud83d", trail = "\ude00", r = [];
  var joined = lead + trail, apart = trail + lead, text = "abcdefg" + lead, more = text + trail, pre = lead + "x", long = "0123456789012345678901234567890123456789" + lead;
  r.push(joined === "😀", joined.length, joined.codePointAt(0), apart.length, apart.codePointAt(0), more === "abcdefg😀", more.length, more.codePointAt(7), (pre + trail).length, (long + trail).codePointAt(40), (long + trail) === "0123456789012345678901234567890123456789😀", ("a" + lead + trail + "b").length, [..."x" + lead + trail].length, encodeURIComponent("q" + lead + trail), (lead + lead + trail + trail).length, (lead + 1 + trail).length);
  return r.join(",");
});
t("identityOfResults", function () {
  var o = {}, m = new Map(), s = new Set(), r = [];
  for (var i = 0; i < 40; i++) { var key = "key-" + (i % 8) + "-suffix"; o[key] = (o[key] || 0) + 1; m.set("m" + (i % 5) + "longer key text", i); s.add("member " + (i % 3)); }
  var a = "prefix-" + 12, b = "prefix-" + "12", c = "pre" + "fix-12", d = "prefix-1" + 2;
  r.push(Object.keys(o).length, o["key-3-suffix"], m.size, m.get("m4longer key text"), s.size, s.has("member " + 2), a === b, b === c, c === d, a === "prefix-12", [a, b, c, d].indexOf("prefix-12"), ("x" + 1 === "x1") + "", ("ab" + "cdefgh" === "abcdefgh") + "", "abcdefg" + "h" === "abcdefgh", ("same text here " + 1) === ("same text here " + 1), Object.is("a" + 1.5, "a1.5"));
  switch ("case-" + 2) { case "case-2": r.push("matched"); break; default: r.push("missed"); }
  var parts = []; for (var j = 0; j < 300; j++) parts.push("rgb(" + (j & 255) + ",10," + (j * 12) + ")"); r.push(parts[299], parts.length, hash(parts.join(";")));
  var built = ""; for (var k = 0; k < 2000; k++) built += "<" + k + ">"; r.push(built.length, hash(built));
  var pre = ""; for (var q = 0; q < 300; q++) pre = q + "," + pre; r.push(pre.length, hash(pre));
  return r.join(",");
});
})();
(function () {
var print = function (s) { all.push(s); };
function t(name, f) { var v; try { v = f(); } catch (e) { v = "threw " + (e && e.constructor ? e.constructor.name : typeof e); } print(name + ": " + v); }
function fill(a, n) { for (var i = 0; i < n; i++) a.push(i * 2); return a; }
function assign(a, n) { for (var i = 0; i < n; i++) a[a.length] = i + 1; return a; }
t("plain pushes", function () { var a = fill([], 50), b = assign([], 50), c = [1, 2]; c.push(3, "s", null, undefined, 1.5, {}, [], true, c.length, "end"); return [a.length, a[49], b.length, b[49], c.length, c[10], [].push(), [].push(1, 2, 3)].join(); });
t("push onto other arrays", function () {
  var frozen = Object.freeze([1]), r = [];
  try { frozen.push(2); } catch (e) { r.push(e.constructor.name); } var fixed = [1]; Object.defineProperty(fixed, "length", { writable: false }); try { fixed.push(2); } catch (e) { r.push(e.constructor.name); } r.push(fixed.length);
  var noext = Object.preventExtensions([1]); try { noext.push(2); } catch (e) { r.push(e.constructor.name); } r.push(noext.length);
  var holes = [1, , 3]; holes.push(4); r.push(holes.length, 1 in holes);
  var withGetter = [1]; Object.defineProperty(withGetter, 3, { get: function () { return "g"; }, configurable: true }); withGetter.push(5); r.push(withGetter.length, withGetter[4], withGetter[3]);
  class Sub extends Array {} var sub = new Sub(); sub.push(1, 2); r.push(sub.length, sub instanceof Sub);
  var proto = []; var child = Object.setPrototypeOf([], proto); child.push(7); r.push(child.length, proto.length);
  var like = { length: 2 }; Array.prototype.push.call(like, "x"); r.push(like.length, like[2]);
  return r.join();
});
t("a key on a class prototype", function () { class C {} C.prototype[0] = "zero"; C.prototype[7] = "seven"; var a = fill([], 12), b = assign([], 12); return [a.length, a[7], b[7], new C()[7]].join(); });
t("setter on Object.prototype", function () {
  var seen = [];
  Object.defineProperty(Object.prototype, 3, { set: function (v) { seen.push("set" + v); }, get: function () { return "inherited"; }, configurable: true });
  var a = fill([], 6), b = assign([], 6), c = [0, 1, 2]; c.push("p"); var r = [a.length, a[3], 3 in a, a.hasOwnProperty(3), b.length, b[3], b.hasOwnProperty(3), c.length, c[3], seen.join("")];
  delete Object.prototype[3]; var d = fill([], 6); r.push(d[3], d.hasOwnProperty(3)); return r.join();
});
t("read-only on Array.prototype", function () {
  Object.defineProperty(Array.prototype, 2, { value: "fixed", writable: false, configurable: true });
  var r = [], a = [0, 1]; try { a.push("x"); r.push("pushed"); } catch (e) { r.push(e.constructor.name); } r.push(a.length, a[2], a.hasOwnProperty(2));
  var b = [0, 1]; b[b.length] = "y"; r.push(b.length, b[2]); function strict() { "use strict"; var c = [0, 1]; try { c[c.length] = "z"; return "set"; } catch (e) { return e.constructor.name; } } r.push(strict());
  delete Array.prototype[2]; var d = fill([], 5); r.push(d[2], d.length, Array.prototype.length); return r.join();
});
t("element on Array.prototype", function () { Array.prototype[1] = "proto"; var a = [0]; var r = [a[1], a.length]; a.push("own"); r.push(a[1], a.length, a.hasOwnProperty(1)); var b = []; b.length = 3; r.push(b[1], 1 in b, b.hasOwnProperty(1)); Array.prototype.length = 0; var c = fill([], 4); r.push(c[1], Array.prototype.length); return r.join(); });
t("after the keys are gone", function () { var a = fill([], 30), b = assign([], 30), c = []; for (var i = 0; i < 20; i++) c.push(i, i + 1); return [a.length, a[29], b[29], c.length, c[39]].join(); });
t("accessor on a later-added key", function () { var seen = []; Object.defineProperty(Object.prototype, 10, { set: function (v) { seen.push(v); }, configurable: true }); var a = fill([], 12); var r = [a.length, a.hasOwnProperty(10), seen.join(), a[11]]; delete Object.prototype[10]; return r.join(); });
})();
all.join("\n")"#;
    let expected = r#"stringWithPrimitive0: 792 14201 2093109899 ["0","\ude00","a long enough string to live on the heap 0","\ude00\ud83d"]
stringWithPrimitive1: 792 14201 2093109899 ["0","\ude00","a long enough string to live on the heap 0","\ude00\ud83d"]
symbolsAndObjects0: TypeError,TypeError,TypeError,TypeError,a7,7a,14,aP,P1,1,2x,x3,x[object Object],xf,ynullundefinedtrue,string,true,33,123,123,50,0,00,nullx,undefined,true!,1n,n2,RangeError,RangeError|vvvvpp
symbolsAndObjects1: TypeError,TypeError,TypeError,TypeError,a7,7a,14,aP,P1,1,2x,x3,x[object Object],xf,ynullundefinedtrue,string,true,33,123,123,50,0,00,nullx,undefined,true!,1n,n2,RangeError,RangeError|vvvvpp
lengthsAroundTheLimits0: 1/1/1/0/1/2/true/true/true/90/90/128512/297870 2/2/2/2/2/3/true/false/true/90/90/128512/-917363828 4/4/4/6/4/5/true/false/true/90/90/128512/333120747 7/7/7/12/7/8/true/false/true/90/90/128512/-1686381083 8/8/8/14/8/9/true/false/true/90/90/128512/-674484951 9/9/9/16/9/10/true/false/true/90/90/128512/-1688690942 10/10/10/18/10/11/true/false/true/90/90/128512/-67939320 16/16/16/30/16/17/true/false/true/90/90/128512/-1174784770 32/32/32/62/32/33/true/false/true/90/90/128512/-389461850 33/33/33/64/33/34/true/false/true/90/90/128512/360233374 34/34/34/66/34/35/true/false/true/90/90/128512/-1456227620 63/63/63/124/63/64/true/false/true/90/90/128512/452431153 64/64/64/126/64/65/true/false/true/90/90/128512/-1749508106 65/65/65/128/65/66/true/false/true/90/90/128512/-2095568978 66/66/66/130/66/67/true/false/true/90/90/128512/792826412 128/128/128/254/128/129/true/false/true/90/90/128512/-698610026 129/129/129/256/129/130/true/false/true/90/90/128512/-1965505586 255/255/255/508/255/256/true/false/true/90/90/128512/-888485487 256/256/256/510/256/257/true/false/true/90/90/128512/-1296700458 257/257/257/512/257/258/true/false/true/90/90/128512/1214315534 258/258/258/514/258/259/true/false/true/90/90/128512/-1388978676 301/301/301/600/301/302/true/false/true/90/90/128512/1434181036 1001/1001/1001/2000/1001/1002/true/false/true/90/90/128512/-898866446
lengthsAroundTheLimits1: 1/1/1/0/1/2/true/true/true/90/90/128512/297870 2/2/2/2/2/3/true/false/true/90/90/128512/-917363828 4/4/4/6/4/5/true/false/true/90/90/128512/333120747 7/7/7/12/7/8/true/false/true/90/90/128512/-1686381083 8/8/8/14/8/9/true/false/true/90/90/128512/-674484951 9/9/9/16/9/10/true/false/true/90/90/128512/-1688690942 10/10/10/18/10/11/true/false/true/90/90/128512/-67939320 16/16/16/30/16/17/true/false/true/90/90/128512/-1174784770 32/32/32/62/32/33/true/false/true/90/90/128512/-389461850 33/33/33/64/33/34/true/false/true/90/90/128512/360233374 34/34/34/66/34/35/true/false/true/90/90/128512/-1456227620 63/63/63/124/63/64/true/false/true/90/90/128512/452431153 64/64/64/126/64/65/true/false/true/90/90/128512/-1749508106 65/65/65/128/65/66/true/false/true/90/90/128512/-2095568978 66/66/66/130/66/67/true/false/true/90/90/128512/792826412 128/128/128/254/128/129/true/false/true/90/90/128512/-698610026 129/129/129/256/129/130/true/false/true/90/90/128512/-1965505586 255/255/255/508/255/256/true/false/true/90/90/128512/-888485487 256/256/256/510/256/257/true/false/true/90/90/128512/-1296700458 257/257/257/512/257/258/true/false/true/90/90/128512/1214315534 258/258/258/514/258/259/true/false/true/90/90/128512/-1388978676 301/301/301/600/301/302/true/false/true/90/90/128512/1434181036 1001/1001/1001/2000/1001/1002/true/false/true/90/90/128512/-898866446
surrogatesJoin0: true,2,128512,2,56832,true,9,128512,3,128512,true,4,2,q%F0%9F%98%80,4,3
surrogatesJoin1: true,2,128512,2,56832,true,9,128512,3,128512,true,4,2,q%F0%9F%98%80,4,3
identityOfResults0: 8,5,5,39,3,true,true,true,true,true,0,true,true,true,true,true,matched,rgb(43,10,3588),300,552677117,10890,-953371419,1090,1717448851
identityOfResults1: 8,5,5,39,3,true,true,true,true,true,0,true,true,true,true,true,matched,rgb(43,10,3588),300,552677117,10890,-953371419,1090,1717448851
plain pushes: 50,98,50,50,12,2,0,3
push onto other arrays: TypeError,TypeError,1,TypeError,1,4,false,5,5,g,2,true,1,0,3,x
a key on a class prototype: 12,14,8,seven
setter on Object.prototype: threw RangeError
read-only on Array.prototype: threw TypeError
element on Array.prototype: threw TypeError
after the keys are gone: threw TypeError
accessor on a later-added key: threw TypeError"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// RegExp literals and lastIndex. Each evaluation of a literal returns a
/// RegExp of its own: its lastIndex and its properties are not those of
/// the evaluation before, in a function called again, a loop, a class
/// field, a default parameter, a generator. And lastIndex holds any value
/// it is given (a negative Number, a fraction, a String, an object, a
/// Symbol), which RegExpBuiltinExec converts when it comes to it. What is
/// expected is what Node prints for the same script.
#[test]
fn regexp_literals_and_last_index_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(regexp_literals_and_last_index)
        .expect("thread")
        .join()
        .expect("checks");
}

fn regexp_literals_and_last_index() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
function show(name, v) { print(name + ": " + v); }
function make() { return /a/g; }
var first = make(), second = make();
show("same object", first === second);
first.tag = 1; show("own property seen on the next", second.tag);
function test(s) { return /a/g.test(s); }
show("test twice", [test("a"), test("a"), test("a")].join());
function count(s) { var n = 0, rx = /a/g; while (rx.exec(s)) n++; return n; }
show("count twice", [count("aaa"), count("aaa")].join());
var objs = []; for (var i = 0; i < 3; i++) objs.push(/x/); show("in a loop", [objs[0] === objs[1], objs[1] === objs[2]].join());
var arrow = () => /y/y; show("arrow", arrow() === arrow());
show("literal vs constructor", make() === new RegExp("a", "g"));
var r = /a/; r.lastIndex = -5; show("negative", r.lastIndex);
r.lastIndex = 1.5; show("fraction", r.lastIndex);
r.lastIndex = "7"; show("string", typeof r.lastIndex + r.lastIndex);
var o = { valueOf: function () { return 2; } }; r.lastIndex = o; show("object", typeof r.lastIndex + (r.lastIndex === o));
r.lastIndex = undefined; show("undefined", String(r.lastIndex));
r.lastIndex = 4294967296; show("large", r.lastIndex);
r.lastIndex = NaN; show("NaN", String(r.lastIndex));
var g = /a/g; g.lastIndex = -1; show("exec with negative", [g.exec("aa") && g.exec("aa").index, g.lastIndex].join());
var h = /a/g; h.lastIndex = { valueOf: function () { return 1; } }; show("exec with object", [h.exec("aaa").index, h.lastIndex].join());
show("descriptor", JSON.stringify(Object.getOwnPropertyDescriptor(/a/, "lastIndex")));
(function () {
  var r = [];
  function sticky(s) { var rx = /a/y; var out = [rx.test(s), rx.lastIndex]; return out.join(":"); }
  r.push(sticky("aa"), sticky("aa"), sticky("ba"));
  function globalExec(s) { var rx = /b/g, m = rx.exec(s); return (m ? m.index : -1) + ":" + rx.lastIndex; }
  r.push(globalExec("abcb"), globalExec("abcb"));
  var made = [1, 2, 3].map(function () { return /m/g; }); made[0].lastIndex = 5; made[1].extra = true; r.push(made[0] === made[1], made[1].lastIndex, made[2].extra, made[2].lastIndex);
  function* gen() { for (var i = 0; i < 2; i++) yield /g/; } var g = Array.from(gen()); r.push(g[0] === g[1]);
  class K { field = /f/; static make() { return /s/i; } } r.push(new K().field === new K().field, K.make() === K.make(), K.make().flags, new K().field.source);
  function def(rx = /d/g) { return rx; } r.push(def() === def(), def().global);
  var holder = function () { return { rx: /o/m, list: [/l/, /l/] }; }; var h1 = holder(), h2 = holder(); r.push(h1.rx === h2.rx, h1.list[0] === h1.list[1], h1.list[0] === h2.list[0]);
  r.push(/a/ === /a/, /a/ == /a/, Object.is(/a/, /a/), typeof /a/, /a/ instanceof RegExp, Object.getPrototypeOf(make()) === RegExp.prototype, make().source, make().flags, String(make()), Object.keys(make()).length, Object.getOwnPropertyNames(make()).join());
  var seen = new Set(); for (var k = 0; k < 50; k++) seen.add(/x/); r.push(seen.size);
  var kept = []; for (var n = 0; n < 20000; n++) { var rx = /(\d+)-(\d+)/g; if (n % 4000 === 0) kept.push(rx); rx.lastIndex = n; } r.push(kept.length, kept[1].lastIndex, (kept[4].lastIndex = 0, kept[4].exec("xx 12-34")[2]), kept[0] === kept[1]);
  var same = make(); r.push(same === same, same.test("a"), same.test("a"), same.lastIndex, make().test("a"), make().lastIndex);
  print("more: " + r.join());
})();
(function () {
  var r = [], rx = /a/g;
  function state() { return typeof rx.lastIndex + ":" + String(rx.lastIndex); }
  var values = [3, 0, -1, 1.5, "2", "x", true, null, undefined, NaN, Infinity, 4294967295, 4294967296, 9007199254740993, 10n];
  for (var i = 0; i < values.length; i++) { rx.lastIndex = values[i]; var before = state(), m; try { m = rx.exec("aaaa"); m = m ? m.index : "null"; } catch (e) { m = e.constructor.name; } r.push(before + ">" + m + ">" + state()); }
  var log = [], o = { valueOf: function () { log.push("v"); return 2; }, toString: function () { log.push("s"); return "1"; } }; rx.lastIndex = o; r.push(rx.lastIndex === o, rx.test("aaaa"), state(), log.join(""));
  var sym = Symbol("i"); rx.lastIndex = sym; try { rx.test("a"); } catch (e) { r.push(e.constructor.name); } r.push(typeof rx.lastIndex);
  rx.lastIndex = 1; r.push(JSON.stringify(Object.getOwnPropertyDescriptor(rx, "lastIndex")), Object.keys(rx).length, rx.hasOwnProperty("lastIndex"), "lastIndex" in rx);
  var n = /a/; n.lastIndex = "5"; r.push(n.test("a"), typeof n.lastIndex + n.lastIndex); n.lastIndex = -3; r.push(n.exec("a").index, n.lastIndex);
  var d = /a/g; Object.defineProperty(d, "lastIndex", { value: "1" }); r.push(typeof d.lastIndex, d.exec("aaa").index, d.lastIndex); Object.defineProperty(d, "lastIndex", { value: -2, writable: false }); r.push(d.lastIndex); try { d.exec("aaa"); } catch (e) { r.push(e.constructor.name); } r.push(d.lastIndex);
  var s = /a/y; s.lastIndex = 1.9; r.push(s.test("aaa"), s.lastIndex); s.lastIndex = "1"; r.push(s.test("ba"), s.lastIndex);
  print("lastIndex values: " + r.join());
})();
})();
all.join("\n")"#;
    let expected = r#"same object: false
own property seen on the next: undefined
test twice: true,true,true
count twice: 3,3
in a loop: false,false
arrow: false
literal vs constructor: false
negative: -5
fraction: 1.5
string: string7
object: objecttrue
undefined: undefined
large: 4294967296
NaN: NaN
exec with negative: 1,2
exec with object: 1,2
descriptor: {"value":0,"writable":true,"enumerable":false,"configurable":false}
more: true:1,true:1,false:0,1:2,1:2,false,0,,0,false,false,false,i,f,false,true,false,false,false,false,false,false,object,true,true,a,g,/a/g,0,lastIndex,50,5,4000,34,false,true,true,false,0,true,0
lastIndex values: number:3>3>number:4,number:0>0>number:1,number:-1>0>number:1,number:1.5>1>number:2,string:2>2>number:3,string:x>0>number:1,boolean:true>1>number:2,object:null>0>number:1,undefined:undefined>0>number:1,number:NaN>0>number:1,number:Infinity>null>number:0,number:4294967295>null>number:0,number:4294967296>null>number:0,number:9007199254740992>null>number:0,bigint:10>TypeError>bigint:10,true,true,number:3,v,TypeError,symbol,{"value":1,"writable":true,"enumerable":false,"configurable":false},0,true,true,true,string5,0,-3,string,1,2,-2,TypeError,-2,true,2,true,2"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// RegExp.prototype\[%Symbol.replace%] by a RegExp that nothing was done
/// to, which is made from the RegExp's internal slots, gives what the
/// steps as written give: templates and callbacks, empty matches, the
/// unicode flag and lone surrogates, lastIndex before and after, named
/// groups and sticky RegExps (which take the steps as written), and every
/// way a script has of putting its own `exec`, `flags` or flag accessor in
/// the way: on the object, on its class, on %RegExp.prototype%, or while
/// the replacement is converted. What is expected is what Node prints for
/// the same script.
#[test]
fn regexp_replacements_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(regexp_replacements)
        .expect("thread")
        .join()
        .expect("checks");
}

fn regexp_replacements() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
function j(v) { return JSON.stringify(v); }
function t(name, f) { for (var round = 0; round < 2; round++) { var v; try { v = f(); } catch (e) { v = "threw " + (e && e.constructor ? e.constructor.name : typeof e); } print(name + round + ": " + v); } }
t("plain", function () {
  var r = [], s = "The quick brown fox & the <lazy> dog & co.";
  r.push(s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"), s.replace(/o/, "0"), s.replace(/o/g, "0"), s.replace(/z+/g, "Z"), s.replace(/nomatch/g, "x") === s, s.replace(/the/gi, "A"), s.replace(/^/, ">"), s.replace(/$/, "<"), s.replace(/\b/g, "|"), "".replace(/x*/g, "-"), "abc".replace(/x*/g, "-"), "abc".replace(/(?:)/g, "."), "a\nb\nc".replace(/^/gm, "> "), "aaa".replace(/a/g, ""), "abc".replace(/b/g, undefined), "abc".replace(/b/g, null), "abc".replace(/b/g, 5), "x".replace(/x/, ""), "a.b.c".replace(/\./g, "$"), "tabs\tand\nlines".replace(/\s/g, "_"));
  return j(r);
});
t("templates", function () {
  var r = [], s = "John Smith and Jane Doe";
  r.push(s.replace(/(\w+)\s(\w+)/, "$2, $1"), s.replace(/(\w+)\s(\w+)/g, "$2 $1"), s.replace(/and/, "[$&]"), s.replace(/and/, "[$`]"), s.replace(/and/, "[$']"), s.replace(/and/, "$$"), s.replace(/and/, "$$1"), s.replace(/(a)(n)(d)/, "$3$2$1$0$4$01$10"), s.replace(/(a)|(z)/g, "[$1|$2]"), s.replace(/and/, "$<name>"), s.replace(/(J)/g, "$"), s.replace(/(J)/g, "$1$"), "abcdefghijkl".replace(/(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)(k)(l)/, "$12$11$10$9$1$13$00"), "aXb".replace(/X/, "$`$'$&$$"), "é😀x".replace(/x/, "[$`]"), "x😀é".replace(/x/, "[$']"));
  return j(r);
});
t("callbacks", function () {
  var r = [], log = [];
  r.push("a1b22c333".replace(/\d+/g, function (m) { return "<" + m.length + ">"; }), "a1b22c333".replace(/(\d)(\d)?/g, function () { log.push(Array.prototype.slice.call(arguments).map(String).join("/")); return arguments.length; }), "abc".replace(/b/, function (m, offset, whole) { return typeof offset + offset + whole; }), "abc".replace(/(x)?b/, function (m, p1) { return String(p1); }), "abc".replace(/b/g, function () { return { toString: function () { return "T"; } }; }), "abc".replace(/b/g, function () { return undefined; }), "abc".replace(/b/g, function () { return 7; }), "aaa".replace(/a/g, function (m, i) { return i; }), "abc".replace(/x*/g, function (m, i) { return "[" + i + "]"; }));
  try { "abc".replace(/b/g, function () { throw new RangeError("cb"); }); } catch (e) { r.push(e.constructor.name); }
  var rx = /b/g; rx.lastIndex = 2; r.push("abcabc".replace(rx, function (m, i) { rx.lastIndex = 99; rx.exec = function () { return null; }; return "B" + i; }), rx.lastIndex, typeof rx.exec, "abcabc".replace(rx, "y"));
  return j(r) + "|" + log.join(",");
});
t("lastIndex", function () {
  var r = [], g = /a/g, n = /a/, y = /a/y, gy = /a/gy;
  g.lastIndex = 2; n.lastIndex = 2; y.lastIndex = 1; gy.lastIndex = 2;
  r.push("aaaa".replace(g, "b"), g.lastIndex, "aaaa".replace(n, "b"), n.lastIndex, "aaaa".replace(y, "b"), y.lastIndex, "aaaa".replace(gy, "b"), gy.lastIndex, "baaa".replace(y, "c"), y.lastIndex, "baaa".replace(gy, "c"), gy.lastIndex);
  var frozenG = Object.freeze(/a/g), frozenN = Object.freeze(/a/); try { r.push("aa".replace(frozenG, "b")); } catch (e) { r.push(e.constructor.name); } r.push("aa".replace(frozenN, "b"));
  var ro = /a/g; Object.defineProperty(ro, "lastIndex", { value: 0, writable: false }); try { r.push("aa".replace(ro, "b")); } catch (e) { r.push(e.constructor.name); }
  var log = [], odd = /a/; odd.lastIndex = { valueOf: function () { log.push("valueOf"); return 1; } }; r.push("aaa".replace(odd, "b"), typeof odd.lastIndex); var oddG = /a/g; oddG.lastIndex = { valueOf: function () { log.push("g"); return 1; } }; r.push("aaa".replace(oddG, "b"), oddG.lastIndex);
  var str = /a/g; str.lastIndex = "2"; r.push("aaaa".replace(str, "b"), str.lastIndex); var neg = /a/; neg.lastIndex = -5; r.push("aa".replace(neg, "b"), neg.lastIndex); var big = /a/g; big.lastIndex = 4294967296; r.push("aa".replace(big, "b"), big.lastIndex);
  return j(r) + "|" + log.join(",");
});
t("unicode", function () {
  var r = [], s = "a😀b😀c", lone = "x\ud83dy";
  function units(x) { var o = []; for (var i = 0; i < x.length; i++) o.push(x.charCodeAt(i).toString(16)); return o.join(" "); }
  r.push(units(s.replace(/(?:)/g, "-")), units(s.replace(/(?:)/gu, "-")), units(s.replace(/./g, "x")), units(s.replace(/./gu, "x")), units(s.replace(/\ud83d/g, "L")), units(s.replace(/\ud83d/gu, "L")), units(s.replace(/\ude00/g, "T")), units(lone.replace(/\ud83d/, "\ude00")), units(lone.replace(/y/, "\ude00")), units("x\ud83d".replace(/$/, "\ude00")), units("é".replace(/é/gi, "É")), units("ÀÉ".replace(/[à-é]/gi, "_")), units("😀".replace(/\u{1F600}/u, "ok")), units("abc".replace(/b/v, "V")), "ǅ".replace(/ǆ/i, "!") , "ſ".replace(/s/i, "!"), "ſ".replace(/s/iu, "!"));
  return j(r);
});
t("named and other flags", function () {
  var r = [];
  r.push("2024-05-06".replace(/(?<y>\d+)-(?<m>\d+)-(?<d>\d+)/, "$<d>/$<m>/$<y>"), "2024-05-06".replace(/(?<y>\d+)-(?<m>\d+)/g, function () { var a = arguments, g = a[a.length - 1]; return typeof g + ":" + g.y + g.m + ":" + a.length; }), "ab".replace(/(?<x>a)|(?<x>b)/g, "[$<x>]"), "aXbxc".replace(/x/gi, "-"), "a\nb".replace(/a.b/s, "S"), "a\nb".replace(/a.b/, "S"), "abc".replace(/b/d, "D"), "abc".replace(/B/dgimsu, "all"));
  return j(r);
});
t("customised", function () {
  var r = [], log = [];
  var own = /b/g; own.exec = function (s) { log.push("own-exec"); return null; }; r.push("abc".replace(own, "x"));
  var ownFlags = /b/g; Object.defineProperty(ownFlags, "flags", { value: "" }); r.push("abcb".replace(ownFlags, "x"), ownFlags.lastIndex);
  var ownGlobal = /b/g; Object.defineProperty(ownGlobal, "global", { value: false }); r.push("abcb".replace(ownGlobal, "x"), ownGlobal.lastIndex);
  var extra = /b/g; extra.note = 1; r.push("abcb".replace(extra, "x"));
  class Plain extends RegExp {} r.push("abcb".replace(new Plain("b", "g"), "x"));
  class Loud extends RegExp { exec(s) { log.push("sub-exec"); return super.exec(s); } } r.push("abcb".replace(new Loud("b", "g"), "x"));
  class Flagged extends RegExp { get flags() { log.push("sub-flags"); return ""; } } r.push("abcb".replace(new Flagged("b", "g"), "x"));
  class Globalish extends RegExp { get global() { log.push("sub-global"); return false; } } r.push("abcb".replace(new Globalish("b", "g"), "x"));
  var original = RegExp.prototype.exec; RegExp.prototype.exec = function (s) { log.push("proto-exec"); return original.call(this, s); }; r.push("abcb".replace(/b/g, "x")); RegExp.prototype.exec = original; r.push("abcb".replace(/b/g, "y"));
  var desc = Object.getOwnPropertyDescriptor(RegExp.prototype, "global"); Object.defineProperty(RegExp.prototype, "global", { get: function () { log.push("proto-global"); return false; }, configurable: true }); r.push("abcb".replace(/b/g, "x")); Object.defineProperty(RegExp.prototype, "global", desc); r.push("abcb".replace(/b/g, "y"));
  var fdesc = Object.getOwnPropertyDescriptor(RegExp.prototype, "flags"); Object.defineProperty(RegExp.prototype, "flags", { get: function () { log.push("proto-flags"); return ""; }, configurable: true }); r.push("abcb".replace(/b/g, "x")); Object.defineProperty(RegExp.prototype, "flags", fdesc); r.push("abcb".replace(/b/g, "y"));
  var sdesc = Object.getOwnPropertyDescriptor(RegExp.prototype, "sticky"); Object.defineProperty(RegExp.prototype, "sticky", { get: function () { log.push("proto-sticky"); return false; }, configurable: true }); r.push("abcb".replace(/b/g, "x")); Object.defineProperty(RegExp.prototype, "sticky", sdesc); r.push("abcb".replace(/b/g, "z"));
  var idesc = Object.getOwnPropertyDescriptor(RegExp.prototype, "ignoreCase"); Object.defineProperty(RegExp.prototype, "ignoreCase", { get: function () { log.push("proto-ignoreCase"); return true; }, configurable: true }); r.push("aBcb".replace(/b/g, "x")); Object.defineProperty(RegExp.prototype, "ignoreCase", idesc);
  delete RegExp.prototype.unicodeSets; r.push("abcb".replace(/b/g, "u")); Object.defineProperty(RegExp.prototype, "unicodeSets", { get: function () { return false; }, configurable: true }); r.push("abcb".replace(/b/g, "v"));
  var fake = { exec: function (s) { log.push("fake-exec"); if (this.done) return null; this.done = true; var m = ["b"]; m.index = 1; return m; }, flags: "", lastIndex: 0 }; r.push(RegExp.prototype[Symbol.replace].call(fake, "abc", "X"));
  var late = /b/g; r.push("abcb".replace(late, { toString: function () { late.exec = function () { log.push("late-exec"); return null; }; return "L"; } }));
  var proxied = new Proxy(/b/g, {}); try { r.push("abcb".replace(proxied, "x")); } catch (e) { r.push(e.constructor.name); }
  var viaProto = Object.create(/b/g); try { r.push("abcb".replace(viaProto, "x")); } catch (e) { r.push(e.constructor.name); }
  var rep = /b/g; rep[Symbol.replace] = function (s, v) { return "own-replace"; }; r.push("abcb".replace(rep, "x"), "abcb".replaceAll(/b/g, "z"));
  try { "abcb".replaceAll(/b/, "z"); } catch (e) { r.push(e.constructor.name); }
  return j(r) + "|" + log.join(",");
});
t("many and long", function () {
  var s = "", h = 0; for (var i = 0; i < 3000; i++) s += "item" + (i % 10) + ";";
  var a = s.replace(/;/g, ", "), b = s.replace(/item(\d)/g, "$1"), c = s.replace(/\d/g, function (d) { return d * 2; }), d = s.replace(/item/g, "");
  function hash(x) { var v = 7; for (var k = 0; k < x.length; k += 7) v = (v * 31 + x.charCodeAt(k)) | 0; return v; }
  var rx = /(\w)(\d)/g, count = 0; for (var n = 0; n < 200; n++) count += "a1 b2 c3".replace(rx, "$2$1").length; 
  return [a.length, hash(a), b.length, hash(b), c.length, hash(c), d.length, hash(d), count, rx.lastIndex].join();
});
})();
all.join("\n")"#;
    let expected = r#"plain0: ["The quick brown fox &amp; the &lt;lazy&gt; dog &amp; co.","The quick br0wn fox & the <lazy> dog & co.","The quick br0wn f0x & the <lazy> d0g & c0.","The quick brown fox & the <laZy> dog & co.",true,"A quick brown fox & A <lazy> dog & co.",">The quick brown fox & the <lazy> dog & co.","The quick brown fox & the <lazy> dog & co.<","|The| |quick| |brown| |fox| & |the| <|lazy|> |dog| & |co|.","-","-a-b-c-",".a.b.c.","> a\n> b\n> c","","aundefinedc","anullc","a5c","","a$b$c","tabs_and_lines"]
plain1: ["The quick brown fox &amp; the &lt;lazy&gt; dog &amp; co.","The quick br0wn fox & the <lazy> dog & co.","The quick br0wn f0x & the <lazy> d0g & c0.","The quick brown fox & the <laZy> dog & co.",true,"A quick brown fox & A <lazy> dog & co.",">The quick brown fox & the <lazy> dog & co.","The quick brown fox & the <lazy> dog & co.<","|The| |quick| |brown| |fox| & |the| <|lazy|> |dog| & |co|.","-","-a-b-c-",".a.b.c.","> a\n> b\n> c","","aundefinedc","anullc","a5c","","a$b$c","tabs_and_lines"]
templates0: ["Smith, John and Jane Doe","Smith John Jane and Doe","John Smith [and] Jane Doe","John Smith [John Smith ] Jane Doe","John Smith [ Jane Doe] Jane Doe","John Smith $ Jane Doe","John Smith $1 Jane Doe","John Smith dna$0$4aa0 Jane Doe","John Smith [a|]nd J[a|]ne Doe","John Smith $<name> Jane Doe","$ohn Smith and $ane Doe","J$ohn Smith and J$ane Doe","lkjiaa3$00","aabX$b","é😀[é😀]","[😀é]😀é"]
templates1: ["Smith, John and Jane Doe","Smith John Jane and Doe","John Smith [and] Jane Doe","John Smith [John Smith ] Jane Doe","John Smith [ Jane Doe] Jane Doe","John Smith $ Jane Doe","John Smith $1 Jane Doe","John Smith dna$0$4aa0 Jane Doe","John Smith [a|]nd J[a|]ne Doe","John Smith $<name> Jane Doe","$ohn Smith and $ane Doe","J$ohn Smith and J$ane Doe","lkjiaa3$00","aabX$b","é😀[é😀]","[😀é]😀é"]
callbacks0: ["a<1>b<2>c<3>","a5b5c55","anumber1abcc","aundefinedc","aTc","aundefinedc","a7c","012","[0]a[1]b[2]c[3]","RangeError","aB1caB4c",99,"function","abcabc"]|1/1/undefined/1/a1b22c333,22/2/2/3/a1b22c333,33/3/3/6/a1b22c333,3/3/undefined/8/a1b22c333
callbacks1: ["a<1>b<2>c<3>","a5b5c55","anumber1abcc","aundefinedc","aTc","aundefinedc","a7c","012","[0]a[1]b[2]c[3]","RangeError","aB1caB4c",99,"function","abcabc"]|1/1/undefined/1/a1b22c333,22/2/2/3/a1b22c333,33/3/3/6/a1b22c333,3/3/undefined/8/a1b22c333
lastIndex0: ["bbbb",0,"baaa",2,"abaa",2,"bbbb",0,"baca",3,"baaa",0,"TypeError","ba","TypeError","baa","object","bbb",0,"bbbb",0,"ba",-5,"bb",0]|valueOf
lastIndex1: ["bbbb",0,"baaa",2,"abaa",2,"bbbb",0,"baca",3,"baaa",0,"TypeError","ba","TypeError","baa","object","bbb",0,"bbbb",0,"ba",-5,"bb",0]|valueOf
unicode0: ["2d 61 2d d83d 2d de00 2d 62 2d d83d 2d de00 2d 63 2d","2d 61 2d d83d de00 2d 62 2d d83d de00 2d 63 2d","78 78 78 78 78 78 78","78 78 78 78 78","61 4c de00 62 4c de00 63","61 d83d de00 62 d83d de00 63","61 d83d 54 62 d83d 54 63","78 de00 79","78 d83d de00","78 d83d de00","c9","5f 5f","6f 6b","61 56 63","!","ſ","!"]
unicode1: ["2d 61 2d d83d 2d de00 2d 62 2d d83d 2d de00 2d 63 2d","2d 61 2d d83d de00 2d 62 2d d83d de00 2d 63 2d","78 78 78 78 78 78 78","78 78 78 78 78","61 4c de00 62 4c de00 63","61 d83d de00 62 d83d de00 63","61 d83d 54 62 d83d 54 63","78 de00 79","78 d83d de00","78 d83d de00","c9","5f 5f","6f 6b","61 56 63","!","ſ","!"]
named and other flags0: ["06/05/2024","object:202405:6-06","[a][b]","a-b-c","S","a\nb","aDc","aallc"]
named and other flags1: ["06/05/2024","object:202405:6-06","[a][b]","a-b-c","S","a\nb","aDc","aallc"]
customised0: ["abc","axcb",2,"axcb",2,"axcx","axcx","axcx","axcb","axcb","axcx","aycy","axcb","aycy","axcb","aycy","axcx","azcz","aBcx","aucu","avcv","aXc","abcb","TypeError","TypeError","own-replace","azcz","TypeError"]|own-exec,sub-exec,sub-exec,sub-exec,sub-flags,sub-global,proto-exec,proto-exec,proto-exec,proto-global,proto-flags,proto-sticky,proto-ignoreCase,fake-exec,late-exec
customised1: ["abc","axcb",2,"axcb",2,"axcx","axcx","axcx","axcb","axcb","axcx","aycy","axcb","aycy","axcb","aycy","axcx","azcz","aBcx","aucu","avcv","aXc","abcb","TypeError","TypeError","own-replace","azcz","TypeError"]|own-exec,sub-exec,sub-exec,sub-exec,sub-flags,sub-global,proto-exec,proto-exec,proto-exec,proto-global,proto-flags,proto-sticky,proto-ignoreCase,fake-exec,late-exec
many and long0: 21000,-1906719097,6000,-79641430,19500,-1921322380,6000,-79641430,1600,0
many and long1: 21000,-1906719097,6000,-79641430,19500,-1921322380,6000,-79641430,1600,0"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The instructions that the loop of the interpreter runs itself, each in
/// an arm of its own that moves past the instruction by a constant, and the
/// small Numbers they read: integers through each of their seven bytes and
/// both signs, doubles that are kept in a value and those that are not,
/// every test of a loop with a constant and with a local, upwards and
/// downwards, each comparison and operator on what it takes in place and on
/// what it leaves to the general way, and code that ends within the bytes
/// an instruction is read with. What is expected is what Node prints for
/// the same script.
#[test]
fn stepped_instructions_and_small_numbers_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(stepped_instructions_and_small_numbers)
        .expect("thread")
        .join()
        .expect("checks");
}

fn stepped_instructions_and_small_numbers() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
function show(name, v) { print(name + ": " + v); }
// Small integers through every byte of their seven, and their signs.
var ints = [0, 1, -1, 127, 128, 255, 256, -128, -129, 65535, 65536, -65536, 16777215, 16777216, -16777217,
  4294967295, 4294967296, -4294967296, 1099511627775, 1099511627776, 281474976710655, 281474976710656,
  -281474976710657, 9007199254740991, -9007199254740991, 9007199254740990, -9007199254740990];
show("ints", ints.map(function (n) { var m = n; m = m + 0; return [m, m + 1, m - 1, -m, m | 0, m >>> 0, m % 7, m < 0, m === n].join("/"); }).join(" "));
show("int edge add", [9007199254740991 + 1, 9007199254740991 + 2, -9007199254740991 - 1, -9007199254740991 - 2].join());
function sums() { var s = 0; for (var i = -5; i < 6; i++) s += i * 1099511627776; return s; }
show("sum over bytes", sums());
function counts() { var n = 0; for (var i = 9007199254740985; i < 9007199254740991; i++) n++; return n; }
show("count near max", counts());
function countsDown() { var n = 0; for (var i = -9007199254740985; i > -9007199254740991; i--) n++; return n; }
show("count near min", countsDown());
// Small doubles (those with a low byte of zero) and others.
var floats = [0.5, -0.5, 1.5, 2.25, -1024.125, 1e21, 1e300, -1e300, 5e-324, 1.7976931348623157e308, 0.1, -0.1, 1 / 3, NaN, Infinity, -Infinity, -0];
show("floats", floats.map(function (x) { var y = x; y = y * 1; return [y, y + 0.5, y * 2, y < 0, y === x, Object.is(y, x), 1 / y].join("/"); }).join(" "));
// Every test of a loop, with a constant and with a local, upwards and downwards.
function loops(a, b) {
  var out = [], i, n;
  n = 0; for (i = a; i < b; i++) n++; out.push(n);
  n = 0; for (i = a; i <= b; i++) n++; out.push(n);
  n = 0; for (i = b; i > a; i--) n++; out.push(n);
  n = 0; for (i = b; i >= a; i--) n++; out.push(n);
  n = 0; for (i = 0; i < 7; i++) n++; out.push(n);
  n = 0; for (i = 0; i <= 7; i++) n++; out.push(n);
  n = 0; for (i = 7; i > 0; i--) n++; out.push(n);
  n = 0; for (i = 7; i >= 0; i--) n++; out.push(n);
  n = 0; i = a; while (i < b) { i++; n++; } out.push(n);
  n = 0; i = a; while (i <= b) { i++; n++; } out.push(n);
  n = 0; i = b; while (i > a) { i--; n++; } out.push(n);
  n = 0; i = b; while (i >= a) { i--; n++; } out.push(n);
  n = 0; i = 0; while (i < 5) { i++; n++; } out.push(n);
  n = 0; i = 0; while (i <= 5) { i++; n++; } out.push(n);
  n = 0; i = 5; while (i > 0) { i--; n++; } out.push(n);
  n = 0; i = 5; while (i >= 0) { i--; n++; } out.push(n);
  n = 0; i = a; do { i++; n++; } while (i < b); out.push(n);
  n = 0; i = a; do { i++; n++; } while (i <= b); out.push(n);
  n = 0; i = b; do { i--; n++; } while (i > a); out.push(n);
  n = 0; i = b; do { i--; n++; } while (i >= a); out.push(n);
  n = 0; i = 0; do { i++; n++; } while (i !== 4); out.push(n);
  n = 0; i = 0; do { i++; n++; } while (i != 4); out.push(n);
  n = 0; i = 0; while (i === 0) { i++; n++; } out.push(n);
  n = 0; i = 0; while (i == 0) { i++; n++; } out.push(n);
  n = 0; i = a; while (i !== b) { i++; n++; } out.push(n);
  n = 0; i = a; while (i === a) { i++; n++; } out.push(n);
  return out.join();
}
show("loops 2 9", loops(2, 9));
show("loops -3 3", loops(-3, 3));
show("loops 0.5 4.5", loops(0.5, 4.5));
show("loops 1 1", loops(1, 1));
// The same with operands that are not integers, or not Numbers.
function tests(x, y) {
  var out = [];
  if (x < y) out.push("lt"); if (x <= y) out.push("le"); if (x > y) out.push("gt"); if (x >= y) out.push("ge");
  if (x === y) out.push("seq"); if (x !== y) out.push("sne"); if (x == y) out.push("eq"); if (x != y) out.push("ne");
  if (!(x < y)) out.push("nlt"); if (!(x <= y)) out.push("nle"); if (!(x > y)) out.push("ngt"); if (!(x >= y)) out.push("nge");
  if (x < 3) out.push("lt3"); if (x <= 3) out.push("le3"); if (x > 3) out.push("gt3"); if (x >= 3) out.push("ge3");
  if (x === 3) out.push("seq3"); if (x !== 3) out.push("sne3"); if (x == 3) out.push("eq3"); if (x != 3) out.push("ne3");
  return out.join("|");
}
var operands = [3, 4, 2.5, 3.0000001, NaN, "3", "abc", null, undefined, true, -0, 1e300, { valueOf: function () { return 3; } }];
operands.forEach(function (x, i) { show("tests " + i, operands.map(function (y) { return tests(x, y); }).join(" ; ")); });
// Each operator, on integers, on doubles, and on what is converted.
function operators(x, y) {
  return [x + y, x - y, x * y, x / y, x % y, x << y, x >> y, x >>> y, x | y, x ^ y, x & y, x < y, x <= y, x > y, x >= y].join("/");
}
show("operators", [[7, 3], [-7, 3], [7, -3], [7.5, 2], [1e15, 1e15], [9007199254740991, 1], [2147483647, 1], [-2147483648, -1], [1, 31], [1, 32], [5, 0], [0, -1], ["7", 3], [7, "3"], [null, 1], [undefined, 1], [true, true]].map(function (p) { return operators(p[0], p[1]); }).join(" "));
// Code that ends within a few bytes of an instruction, and code with none.
show("short code", [eval("1"), eval(""), eval("1;2"), eval("var q = 5; q"), (function () {})(), (function () { return; })(), (function (a) { return a; })(4), eval("if (0) 1"), eval("do {} while (0)"), eval("for (var z = 0; z < 2; z++) z")].join());
show("switch", [0, 1, 2, 3, "1", 2.5].map(function (v) { switch (v) { case 0: return "a"; case 1: return "b"; case 2: return "c"; default: return "d"; } }).join(""));
})();
all.join("\n")"#;
    let expected = r#"ints: 0/1/-1/0/0/0/0/false/true 1/2/0/-1/1/1/1/false/true -1/0/-2/1/-1/4294967295/-1/true/true 127/128/126/-127/127/127/1/false/true 128/129/127/-128/128/128/2/false/true 255/256/254/-255/255/255/3/false/true 256/257/255/-256/256/256/4/false/true -128/-127/-129/128/-128/4294967168/-2/true/true -129/-128/-130/129/-129/4294967167/-3/true/true 65535/65536/65534/-65535/65535/65535/1/false/true 65536/65537/65535/-65536/65536/65536/2/false/true -65536/-65535/-65537/65536/-65536/4294901760/-2/true/true 16777215/16777216/16777214/-16777215/16777215/16777215/0/false/true 16777216/16777217/16777215/-16777216/16777216/16777216/1/false/true -16777217/-16777216/-16777218/16777217/-16777217/4278190079/-2/true/true 4294967295/4294967296/4294967294/-4294967295/-1/4294967295/3/false/true 4294967296/4294967297/4294967295/-4294967296/0/0/4/false/true -4294967296/-4294967295/-4294967297/4294967296/0/0/-4/true/true 1099511627775/1099511627776/1099511627774/-1099511627775/-1/4294967295/1/false/true 1099511627776/1099511627777/1099511627775/-1099511627776/0/0/2/false/true 281474976710655/281474976710656/281474976710654/-281474976710655/-1/4294967295/0/false/true 281474976710656/281474976710657/281474976710655/-281474976710656/0/0/1/false/true -281474976710657/-281474976710656/-281474976710658/281474976710657/-1/4294967295/-2/true/true 9007199254740991/9007199254740992/9007199254740990/-9007199254740991/-1/4294967295/3/false/true -9007199254740991/-9007199254740990/-9007199254740992/9007199254740991/1/1/-3/true/true 9007199254740990/9007199254740991/9007199254740989/-9007199254740990/-2/4294967294/2/false/true -9007199254740990/-9007199254740989/-9007199254740991/9007199254740990/2/2/-2/true/true
int edge add: 9007199254740992,9007199254740992,-9007199254740992,-9007199254740992
sum over bytes: 0
count near max: 6
count near min: 6
floats: 0.5/1/1/false/true/true/2 -0.5/0/-1/true/true/true/-2 1.5/2/3/false/true/true/0.6666666666666666 2.25/2.75/4.5/false/true/true/0.4444444444444444 -1024.125/-1023.625/-2048.25/true/true/true/-0.0009764433052605883 1e+21/1e+21/2e+21/false/true/true/1e-21 1e+300/1e+300/2e+300/false/true/true/1e-300 -1e+300/-1e+300/-2e+300/true/true/true/-1e-300 5e-324/0.5/1e-323/false/true/true/Infinity 1.7976931348623157e+308/1.7976931348623157e+308/Infinity/false/true/true/5.562684646268003e-309 0.1/0.6/0.2/false/true/true/10 -0.1/0.4/-0.2/true/true/true/-10 0.3333333333333333/0.8333333333333333/0.6666666666666666/false/true/true/3 NaN/NaN/NaN/false/false/true/NaN Infinity/Infinity/Infinity/false/true/true/0 -Infinity/-Infinity/-Infinity/true/true/true/0 0/0.5/0/false/true/true/-Infinity
loops 2 9: 7,8,7,8,7,8,7,8,7,8,7,8,5,6,5,6,7,8,7,8,4,4,1,1,7,1
loops -3 3: 6,7,6,7,7,8,7,8,6,7,6,7,5,6,5,6,6,7,6,7,4,4,1,1,6,1
loops 0.5 4.5: 4,5,4,5,7,8,7,8,4,5,4,5,5,6,5,6,4,5,4,5,4,4,1,1,4,1
loops 1 1: 0,1,0,1,7,8,7,8,0,1,0,1,5,6,5,6,1,1,1,1,4,4,1,1,0,1
tests 0: le|ge|seq|eq|nlt|ngt|le3|ge3|seq3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|seq3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|seq3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|seq3|eq3 ; sne|ne|nlt|nle|ngt|nge|le3|ge3|seq3|eq3 ; le|ge|sne|eq|nlt|ngt|le3|ge3|seq3|eq3 ; sne|ne|nlt|nle|ngt|nge|le3|ge3|seq3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|seq3|eq3 ; sne|ne|nlt|nle|ngt|nge|le3|ge3|seq3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|seq3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|seq3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|seq3|eq3 ; le|ge|sne|eq|nlt|ngt|le3|ge3|seq3|eq3
tests 1: gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; le|ge|seq|eq|nlt|ngt|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3
tests 2: lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; le|ge|seq|eq|nlt|ngt|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|lt3|le3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3
tests 3: gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; le|ge|seq|eq|nlt|ngt|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3
tests 4: sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3
tests 5: le|ge|sne|eq|nlt|ngt|le3|ge3|sne3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|sne3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|sne3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|sne3|eq3 ; sne|ne|nlt|nle|ngt|nge|le3|ge3|sne3|eq3 ; le|ge|seq|eq|nlt|ngt|le3|ge3|sne3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|sne3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|sne3|eq3 ; sne|ne|nlt|nle|ngt|nge|le3|ge3|sne3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|sne3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|sne3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|sne3|eq3 ; le|ge|sne|eq|nlt|ngt|le3|ge3|sne3|eq3
tests 6: sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|sne3|ne3 ; le|ge|seq|eq|nlt|ngt|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3
tests 7: lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; le|ge|seq|eq|nlt|ngt|lt3|le3|sne3|ne3 ; sne|eq|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; le|ge|sne|ne|nlt|ngt|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3
tests 8: sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|eq|nlt|nle|ngt|nge|sne3|ne3 ; seq|eq|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|sne3|ne3
tests 9: lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; le|ge|seq|eq|nlt|ngt|lt3|le3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3
tests 10: lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; le|ge|sne|ne|nlt|ngt|lt3|le3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; le|ge|seq|eq|nlt|ngt|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3 ; lt|le|sne|ne|ngt|nge|lt3|le3|sne3|ne3
tests 11: gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; sne|ne|nlt|nle|ngt|nge|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3 ; le|ge|seq|eq|nlt|ngt|gt3|ge3|sne3|ne3 ; gt|ge|sne|ne|nlt|nle|gt3|ge3|sne3|ne3
tests 12: le|ge|sne|eq|nlt|ngt|le3|ge3|sne3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|sne3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|sne3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|sne3|eq3 ; sne|ne|nlt|nle|ngt|nge|le3|ge3|sne3|eq3 ; le|ge|sne|eq|nlt|ngt|le3|ge3|sne3|eq3 ; sne|ne|nlt|nle|ngt|nge|le3|ge3|sne3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|sne3|eq3 ; sne|ne|nlt|nle|ngt|nge|le3|ge3|sne3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|sne3|eq3 ; gt|ge|sne|ne|nlt|nle|le3|ge3|sne3|eq3 ; lt|le|sne|ne|ngt|nge|le3|ge3|sne3|eq3 ; le|ge|seq|eq|nlt|ngt|le3|ge3|sne3|eq3
operators: 10/4/21/2.3333333333333335/1/56/0/0/7/4/3/false/false/true/true -4/-10/-21/-2.3333333333333335/-1/-56/-1/536870911/-5/-6/1/true/true/false/false 4/10/-21/-2.3333333333333335/1/-536870912/0/0/-1/-6/5/false/false/true/true 9.5/5.5/15/3.75/1.5/28/1/1/7/5/2/false/false/true/true 2000000000000000/0/1e+30/1/0/-1530494976/-1530494976/2764472320/-1530494976/0/-1530494976/false/true/false/true 9007199254740992/9007199254740990/9007199254740991/9007199254740991/0/-2/-1/2147483647/-1/-2/1/false/false/true/true 2147483648/2147483646/2147483647/2147483647/0/-2/1073741823/1073741823/2147483647/2147483646/1/false/false/true/true -2147483649/-2147483647/2147483648/2147483648/0/0/-1/1/-1/2147483647/-2147483648/true/true/false/false 32/-30/31/0.03225806451612903/1/-2147483648/0/0/31/30/1/true/true/false/false 33/-31/32/0.03125/1/1/1/1/33/33/0/true/true/false/false 5/5/0/Infinity/NaN/5/5/5/5/5/0/false/false/true/true -1/1/0/0/0/0/0/0/-1/-1/0/false/false/true/true 73/4/21/2.3333333333333335/1/56/0/0/7/4/3/false/false/true/true 73/4/21/2.3333333333333335/1/56/0/0/7/4/3/false/false/true/true 1/-1/0/0/0/0/0/0/1/1/0/true/true/false/false NaN/NaN/NaN/NaN/NaN/0/0/0/1/1/0/false/false/false/false 2/0/1/1/0/2/0/0/1/0/1/false/true/false/true
short code: 1,,2,5,,,4,,,1
switch: abcddd"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The constants, property caches and binding sites of a function while it
/// runs, which the interpreter holds outside the heap: read after
/// collections that move what the constants refer to (long strings,
/// doubles, a BigInt, a RegExp, a template), at several depths of one
/// function at once, from closures made at different times, across the
/// yields of a generator, in code that `eval` and `Function` make and drop,
/// and by a `switch` over string constants. What is expected is what Node
/// prints for the same script.
#[test]
fn tables_of_running_code_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(tables_of_running_code)
        .expect("thread")
        .join()
        .expect("checks");
}

fn tables_of_running_code() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
var print = function (s) { all.push(s); };
function show(name, v) { print(name + ": " + v); }
// Garbage enough for several collections, made between uses of constants
// that live in the heap: long strings, doubles, a BigInt, a RegExp, a template.
function churn(n) { var keep = []; for (var i = 0; i < n; i++) { keep.push({ a: "garbage number " + i, b: [i, i + 0.5, "x" + i] }); if (keep.length > 500) keep = []; } return keep.length; }
function constants(rounds) {
  var out = [];
  for (var r = 0; r < rounds; r++) {
    churn(20000);
    out.push("a long string constant, longer than seven bytes".length, 1.2345678901234567e300 > 1e299, 123456789012345678901234567890n % 97n, /constant(\d+)/.exec("constant42")[1], `template ${r} with text`.length, "another long constant".charCodeAt(8), 0.1 + 0.2);
  }
  return out.join();
}
show("constants", constants(4));
// The same function at several depths, each holding the same tables, with
// collections between the levels.
function deep(n, tag) {
  if (n === 0) { churn(30000); return "bottom of the recursion " + tag; }
  var before = "level " + n + " before the call, in a long enough string";
  var below = deep(n - 1, tag);
  churn(2000);
  return below.length + before.length + ("after the call at level " + n).length;
}
show("deep", [deep(1, "a"), deep(6, "b"), deep(12, "c")].join());
// Variables of enclosing functions, found again after collections and from
// closures made at different times.
function counters() {
  var total = 0, label = "the label of the counters, a long string";
  function add(n) { total += n; return label.length + total; }
  function relabel(s) { label = s; }
  return [add, relabel, function () { return [total, label].join("|"); }];
}
var c1 = counters(), c2 = counters();
var seen = [];
for (var i = 0; i < 5; i++) { seen.push(c1[0](i), c2[0](i * 10)); churn(8000); if (i === 2) c2[1]("renamed while running, also long"); }
show("closures", seen.join() + " " + c1[2]() + " " + c2[2]());
// A generator that yields between collections, and its constants after.
function* generator() { var i = 0; while (true) { churn(6000); yield "yielded value number " + (i++) + " of the generator"; } }
var g = generator(), g2 = generator();
show("generator", [g.next().value, g2.next().value, g.next().value.length, g.next().value, g2.next().value].join(";"));
// Code made by eval and by Function, run, dropped and made again.
var evals = [];
for (var k = 0; k < 6; k++) {
  evals.push(eval("(function () { churn(4000); return 'made by eval, round ' + " + k + " + ', a long string'; })()").length);
  evals.push(new Function("x", "churn(3000); return x + ' passed through a Function made at run time';")("round " + k).length);
  evals.push(eval("var q" + k + " = 'a global made by eval ' + " + k + "; q" + k + ".length"));
}
show("eval", evals.join());
// Property caches of a function that is running while its objects change shape.
function shapes(n) {
  var objects = [], total = 0;
  for (var i = 0; i < n; i++) { var o = { first: i }; if (i % 3 === 0) o.second = i * 2; if (i % 5 === 0) o["key number " + (i % 7)] = i; objects.push(o); }
  for (var j = 0; j < n; j++) { total += objects[j].first + (objects[j].second || 0); if (j % 4000 === 0) churn(5000); }
  return total;
}
show("shapes", [shapes(10), shapes(20000)].join());
// A switch over string constants and one over integers, after collections.
function pick(v) { churn(1500); switch (v) { case "the first long case label": return 1; case "the second long case label": return 2; case 3: return 3; case 4: return 4; default: return 0; } }
show("switch", ["the first long case label", "the second long case label", 3, 4, "other", 5].map(pick).join(""));
all.join("\n")"#;
    let expected = r#"constants: 47,true,52,42,20,108,0.30000000000000004,47,true,52,42,20,108,0.30000000000000004,47,true,52,42,20,108,0.30000000000000004,47,true,52,42,20,108,0.30000000000000004
deep: 98,NaN,NaN
closures: 40,40,41,50,43,70,46,92,50,132 10|the label of the counters, a long string 100|renamed while running, also long
generator: yielded value number 0 of the generator;yielded value number 0 of the generator;39;yielded value number 2 of the generator;yielded value number 1 of the generator
eval: 36,50,23,36,50,23,36,50,23,36,50,23,36,50,23,36,50,23
shapes: 81,333316666
switch: 123400"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// A property read and written where the property cache of the place
/// knows the shape: in storage of every size, on the object, on what it
/// inherits from and nowhere, as an accessor with and without a getter or
/// a setter, not writable, frozen, sealed and not extensible, and on values
/// of other kinds at the same place. And calls that the interpreter runs
/// in place: on what a call returned, with calls as arguments, with too
/// few and too many arguments, and with errors thrown through them and
/// caught at every depth. What is expected is what Node prints for the
/// same script.
#[test]
fn cached_properties_and_calls_in_place_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(cached_properties_and_calls_in_place)
        .expect("thread")
        .join()
        .expect("checks");
}

fn cached_properties_and_calls_in_place() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
function show(name, v) { print(name + ": " + v); }
function read(o) { return o.p; }
function write(o, v) { o.p = v; return o.p; }
function twice(f) { var out = []; for (var round = 0; round < 3; round++) out.push(f()); return out.join("|"); }
// Objects of every size of storage, the property first, in the middle and last.
show("sizes", twice(function () {
  var out = [];
  [0, 1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 33, 63, 65, 127, 129, 255, 257, 300].forEach(function (n) {
    var first = { p: "first" }, middle = {}, last = {};
    for (var i = 0; i < n; i++) { first["k" + i] = i; if (i === (n >> 1)) middle.p = "middle"; middle["k" + i] = i; last["k" + i] = i; }
    if (!("p" in middle)) middle.p = "middle"; last.p = "last";
    out.push(read(first) + read(middle) + read(last) + write(first, n) + write(middle, -n) + write(last, n + 0.5) + read(first) + read(last));
  });
  return out.join(",");
}));
// A data property on the object, on its prototype, further up, and nowhere.
show("where", twice(function () {
  var top = { p: "top" }, mid = Object.create(top), low = Object.create(mid), none = Object.create(null), own = Object.create(top);
  own.p = "own";
  var out = [read(top), read(mid), read(low), read(none), read(own), read({}), read(Object.create(Object.create({})))];
  mid.p = "mid"; out.push(read(mid), read(low));
  delete mid.p; out.push(read(mid), read(low));
  top.p = "changed"; out.push(read(low), read(own));
  delete top.p; out.push(read(low), read(top));
  return out.join();
}));
// Accessors on the object and on its prototype, with and without a getter or a setter.
show("accessors", twice(function () {
  var log = [], stored = 1;
  var both = { get p() { log.push("get"); return stored; }, set p(v) { log.push("set" + v); stored = v; } };
  var getter = { get p() { return "only get"; } }, setter = { set p(v) { log.push("only set" + v); } };
  var child = Object.create(both), childOfGetter = Object.create(getter), childOfSetter = Object.create(setter);
  var out = [read(both), write(both, 2), read(child), write(child, 3), Object.keys(child).length, read(getter), write(getter, 9), read(setter), write(setter, 8), read(childOfGetter), write(childOfGetter, 7), Object.keys(childOfGetter).length, read(childOfSetter), write(childOfSetter, 6), Object.keys(childOfSetter).length];
  return out.join() + " " + log.join();
}));
// Properties that are not writable, objects that are frozen, sealed or not extensible.
show("protected", twice(function () {
  var readonly = Object.defineProperty({}, "p", { value: "fixed", writable: false, configurable: true, enumerable: true });
  var frozen = Object.freeze({ p: "frozen" }), sealed = Object.seal({ p: "sealed" }), closed = Object.preventExtensions({ q: 1 });
  var childOfReadonly = Object.create(readonly), childOfFrozen = Object.create(frozen);
  var out = [write(readonly, 1), write(frozen, 2), write(sealed, 3), write(closed, 4), "p" in closed, write(childOfReadonly, 5), Object.keys(childOfReadonly).length, write(childOfFrozen, 6), Object.keys(childOfFrozen).length];
  (function () { "use strict"; [readonly, frozen, closed, childOfReadonly].forEach(function (o) { try { o.p = "strict"; out.push("no error"); } catch (e) { out.push(e.constructor.name); } }); })();
  Object.defineProperty(readonly, "p", { writable: true }); out.push(write(readonly, "now writable"));
  return out.join();
}));
// The same places in the code seeing other kinds of values.
show("kinds", twice(function () {
  function F() {} F.prototype.p = "from a function's prototype"; F.p = "on the function";
  var array = [1, 2, 3]; array.p = "on an array";
  var out = [read(F), read(new F()), read(array), read([]), read("string"), read(5), read(true), read(function () {}), read(new Map()), read(/x/), read(new Date(0)), read(Symbol.iterator)];
  Array.prototype.p = "from Array.prototype"; String.prototype.p = "from String.prototype"; Number.prototype.p = "from Number.prototype"; Object.prototype.p = "from Object.prototype";
  out.push(read([]), read("string"), read(5), read(true), read(new Map()), read({}), read(Object.create(null)));
  delete Array.prototype.p; delete String.prototype.p; delete Number.prototype.p; delete Object.prototype.p;
  out.push(read([]), read("string"), read(5), read({}), write(array, "again"), write(F, "F"));
  try { read(null); } catch (e) { out.push(e.constructor.name); }
  try { write(undefined, 1); } catch (e) { out.push(e.constructor.name); }
  return out.join();
}));
// Methods called on what a call returned, arguments that are calls, too few and too many arguments.
show("calls", twice(function () {
  var log = [];
  var o = { n: 1, self: function () { log.push("self" + this.n); return this; }, add: function (a, b) { log.push("add"); return this.n + (a === undefined ? 100 : a) + (b === undefined ? 1000 : b); }, make: function (n) { return { n: n, self: o.self, add: o.add, make: o.make }; } };
  function id(x) { return x; }
  function three(a, b, c) { return [a, b, c].join("/"); }
  var out = [o.self().add(1, 2), o.make(5).self().add(id(3)), o.add(o.make(7).add(1, 1), id(id(2))), three(1), three(1, 2, 3, 4, 5), three(id(1), o.self().n, three()), id(o.make(9)).self().make(4).add(), 1 + id(2) * id(3) - o.add(0, 0)];
  return out.join() + " " + log.join("");
}));
// Errors thrown through functions that are run in place, caught at every depth.
show("throws", twice(function () {
  function thrower(depth, at) { if (depth === at) throw new RangeError("at " + at); return 1 + thrower(depth + 1, at); }
  function catcher(depth, catchAt, throwAt) {
    if (depth === catchAt) { try { return thrower(depth, throwAt); } catch (e) { return e.message + " caught at " + depth; } finally { depthSeen.push(depth); } }
    return catcher(depth + 1, catchAt, throwAt) + "<" + depth;
  }
  var depthSeen = [], out = [];
  for (var c = 0; c < 4; c++) for (var t = c; t < c + 4; t++) out.push(catcher(0, c, t));
  function overflow(n) { return overflow(n + 1) + 1; }
  try { overflow(0); } catch (e) { out.push(e.constructor.name); }
  out.push(catcher(0, 2, 5), depthSeen.length);
  return out.join(";");
}));
})();
all.join("\n")"#;
    let expected = r#"sizes: firstmiddlelast000.500.5,firstmiddlelast1-11.511.5,firstmiddlelast2-22.522.5,firstmiddlelast3-33.533.5,firstmiddlelast4-44.544.5,firstmiddlelast5-55.555.5,firstmiddlelast7-77.577.5,firstmiddlelast8-88.588.5,firstmiddlelast9-99.599.5,firstmiddlelast15-1515.51515.5,firstmiddlelast16-1616.51616.5,firstmiddlelast17-1717.51717.5,firstmiddlelast31-3131.53131.5,firstmiddlelast33-3333.53333.5,firstmiddlelast63-6363.56363.5,firstmiddlelast65-6565.56565.5,firstmiddlelast127-127127.5127127.5,firstmiddlelast129-129129.5129129.5,firstmiddlelast255-255255.5255255.5,firstmiddlelast257-257257.5257257.5,firstmiddlelast300-300300.5300300.5|firstmiddlelast000.500.5,firstmiddlelast1-11.511.5,firstmiddlelast2-22.522.5,firstmiddlelast3-33.533.5,firstmiddlelast4-44.544.5,firstmiddlelast5-55.555.5,firstmiddlelast7-77.577.5,firstmiddlelast8-88.588.5,firstmiddlelast9-99.599.5,firstmiddlelast15-1515.51515.5,firstmiddlelast16-1616.51616.5,firstmiddlelast17-1717.51717.5,firstmiddlelast31-3131.53131.5,firstmiddlelast33-3333.53333.5,firstmiddlelast63-6363.56363.5,firstmiddlelast65-6565.56565.5,firstmiddlelast127-127127.5127127.5,firstmiddlelast129-129129.5129129.5,firstmiddlelast255-255255.5255255.5,firstmiddlelast257-257257.5257257.5,firstmiddlelast300-300300.5300300.5|firstmiddlelast000.500.5,firstmiddlelast1-11.511.5,firstmiddlelast2-22.522.5,firstmiddlelast3-33.533.5,firstmiddlelast4-44.544.5,firstmiddlelast5-55.555.5,firstmiddlelast7-77.577.5,firstmiddlelast8-88.588.5,firstmiddlelast9-99.599.5,firstmiddlelast15-1515.51515.5,firstmiddlelast16-1616.51616.5,firstmiddlelast17-1717.51717.5,firstmiddlelast31-3131.53131.5,firstmiddlelast33-3333.53333.5,firstmiddlelast63-6363.56363.5,firstmiddlelast65-6565.56565.5,firstmiddlelast127-127127.5127127.5,firstmiddlelast129-129129.5129129.5,firstmiddlelast255-255255.5255255.5,firstmiddlelast257-257257.5257257.5,firstmiddlelast300-300300.5300300.5
where: top,top,top,,own,,,mid,mid,top,top,changed,own,,|top,top,top,,own,,,mid,mid,top,top,changed,own,,|top,top,top,,own,,,mid,mid,top,top,changed,own,,
accessors: 1,2,2,3,0,only get,only get,,,only get,only get,0,,,0 get,set2,get,get,set3,get,only set8,only set6|1,2,2,3,0,only get,only get,,,only get,only get,0,,,0 get,set2,get,get,set3,get,only set8,only set6|1,2,2,3,0,only get,only get,,,only get,only get,0,,,0 get,set2,get,get,set3,get,only set8,only set6
protected: fixed,frozen,3,,false,fixed,0,frozen,0,TypeError,TypeError,TypeError,TypeError,now writable|fixed,frozen,3,,false,fixed,0,frozen,0,TypeError,TypeError,TypeError,TypeError,now writable|fixed,frozen,3,,false,fixed,0,frozen,0,TypeError,TypeError,TypeError,TypeError,now writable
kinds: on the function,from a function's prototype,on an array,,,,,,,,,,from Array.prototype,from String.prototype,from Number.prototype,from Object.prototype,from Object.prototype,from Object.prototype,,,,,,again,F,TypeError,TypeError|on the function,from a function's prototype,on an array,,,,,,,,,,from Array.prototype,from String.prototype,from Number.prototype,from Object.prototype,from Object.prototype,from Object.prototype,,,,,,again,F,TypeError,TypeError|on the function,from a function's prototype,on an array,,,,,,,,,,from Array.prototype,from String.prototype,from Number.prototype,from Object.prototype,from Object.prototype,from Object.prototype,,,,,,again,F,TypeError,TypeError
calls: 4,1008,12,1//,1/2/3,1/1///,1104,6 self1addself5addaddaddself1self9addadd|4,1008,12,1//,1/2/3,1/1///,1104,6 self1addself5addaddaddself1self9addadd|4,1008,12,1//,1/2/3,1/1///,1104,6 self1addself5addaddaddself1self9addadd
throws: at 0 caught at 0;at 1 caught at 0;at 2 caught at 0;at 3 caught at 0;at 1 caught at 1<0;at 2 caught at 1<0;at 3 caught at 1<0;at 4 caught at 1<0;at 2 caught at 2<1<0;at 3 caught at 2<1<0;at 4 caught at 2<1<0;at 5 caught at 2<1<0;at 3 caught at 3<2<1<0;at 4 caught at 3<2<1<0;at 5 caught at 3<2<1<0;at 6 caught at 3<2<1<0;RangeError;at 5 caught at 2<1<0;17|at 0 caught at 0;at 1 caught at 0;at 2 caught at 0;at 3 caught at 0;at 1 caught at 1<0;at 2 caught at 1<0;at 3 caught at 1<0;at 4 caught at 1<0;at 2 caught at 2<1<0;at 3 caught at 2<1<0;at 4 caught at 2<1<0;at 5 caught at 2<1<0;at 3 caught at 3<2<1<0;at 4 caught at 3<2<1<0;at 5 caught at 3<2<1<0;at 6 caught at 3<2<1<0;RangeError;at 5 caught at 2<1<0;17|at 0 caught at 0;at 1 caught at 0;at 2 caught at 0;at 3 caught at 0;at 1 caught at 1<0;at 2 caught at 1<0;at 3 caught at 1<0;at 4 caught at 1<0;at 2 caught at 2<1<0;at 3 caught at 2<1<0;at 4 caught at 2<1<0;at 5 caught at 2<1<0;at 3 caught at 3<2<1<0;at 4 caught at 3<2<1<0;at 5 caught at 3<2<1<0;at 6 caught at 3<2<1<0;RangeError;at 5 caught at 2<1<0;17"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The default of an element of a pattern reads a name that an earlier
/// element of the same pattern bound, in a parameter, a `let`, a `const`,
/// a loop and a `catch`, nested or not; a name read before its element is
/// reached, in its own default, key or value, is a ReferenceError still.
/// Read as a variable kept on the stack, every such name was refused as
/// read before its initialization. What is expected is what Node prints
/// for the same script.
#[test]
fn names_bound_earlier_in_a_pattern_are_read_by_later_defaults() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(names_bound_earlier_in_a_pattern)
        .expect("thread")
        .join()
        .expect("checks");
}

fn names_bound_earlier_in_a_pattern() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
(function () {
var print = function (s) { all.push(s); };
function attempt(name, f) { var v; try { v = f(); } catch (e) { v = e.constructor.name; } print(name + ": " + v); }
// A default that reads a name bound earlier in the same pattern.
attempt("object parameter", function () { function f({ p, q = p + 1 }) { return p + "," + q; } return f({ p: 1 }); });
attempt("array parameter", function () { function f([r, s = r]) { return r + "," + s; } return f([2]); });
attempt("let", function () { let { p, q = p + 1 } = { p: 1 }; return p + "," + q; });
attempt("const", function () { const [a, b = a * 2, c = a + b] = [1]; return [a, b, c].join(); });
attempt("arrow", function () { var f = ({ p, q = p + 1 }) => p + "," + q; return f({ p: 1 }); });
attempt("nested", function () { let [a, [b = a, c = b], { d = c + a }] = [1, [], {}]; return [a, b, c, d].join(); });
attempt("nested object", function () { let { a: { p }, q = p, r: [s = q] } = { a: { p: 4 }, r: [] }; return [p, q, s].join(); });
attempt("rest", function () { const { a, ...rest } = { a: 1, b: 2 }, { c = a + rest.b } = {}; return c; });
attempt("for of", function () { for (const { a, b = a * 2 } of [{ a: 2 }]) return a + b; });
attempt("catch", function () { try { throw { a: 3 }; } catch ({ a, b = a + 1 }) { return a + b; } });
attempt("two parameters", function () { function f({ a, b = a }, { c = b, d = c } = {}) { return [a, b, c, d].join(); } return f({ a: 1 }); });
attempt("computed key of a later element", function () { let { a, [a]: b } = { a: "k", k: 5 }; return a + b; });
// And those that read it before it is bound.
attempt("later name", function () { let { q = p, p } = { p: 1 }; return q; });
attempt("own default", function () { let { p = p } = {}; return p; });
attempt("own value", function () { let { p } = p; return p; });
attempt("own key", function () { let { [p]: p } = { a: 1 }; return p; });
attempt("later element of an array", function () { let [a = b, b] = []; return a; });
attempt("later parameter", function () { function f({ a = b }, { b }) { return a; } return f({}, { b: 1 }); });
attempt("own initializer", function () { let x = x + 1; return x; });
attempt("default of the whole parameter", function () { function f({ a } = a) { return a; } return f(); });
attempt("nested own default", function () { let [a, [b = c, c]] = [1, []]; return b; });
})();
all.join("\n")"#;
    let expected = r#"object parameter: 1,2
array parameter: 2,2
let: 1,2
const: 1,2,3
arrow: 1,2
nested: 1,1,1,2
nested object: 4,4,4
rest: 3
for of: 6
catch: 7
two parameters: 1,1,1,1
computed key of a later element: k5
later name: ReferenceError
own default: ReferenceError
own value: ReferenceError
own key: ReferenceError
later element of an array: ReferenceError
later parameter: ReferenceError
own initializer: ReferenceError
default of the whole parameter: ReferenceError
nested own default: ReferenceError"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// Functions that bind their parameters with their own instructions, which
/// are run in place of their calls like any other: defaults, rest
/// parameters and patterns, with and without vars, lets and functions in
/// the body; what a default sees of the scope outside, of the parameters
/// before it and of the body; a direct eval in the parameters and in the
/// body; `this`, `arguments` and `new.target`; many and few arguments,
/// recursion and errors through such functions; and generators, classes
/// and bound functions, which are called as before. What is expected is
/// what Node prints for the same script.
#[test]
fn functions_that_bind_their_parameters_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(functions_that_bind_their_parameters)
        .expect("thread")
        .join()
        .expect("checks");
}

fn functions_that_bind_their_parameters() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
var print = function (s) { all.push(s); };
function show(name, v) { print(name + ": " + v); }
function twice(f) { var out = []; for (var round = 0; round < 3; round++) { try { out.push(f()); } catch (e) { out.push("threw " + e.constructor.name + " " + e.message); } } return out.join("|"); }
var x = "outer x", y = "outer y";
// Defaults, rest parameters and patterns, with and without vars, lets and functions in the body.
show("defaults", twice(function () {
  function plain(a, b = 2, ...rest) { return [a, b, rest.join("+"), rest.length].join("/"); }
  function withLet(a, b = a + 1, c = b * 2) { let d = a + b + c; const e = d * 2; return [a, b, c, d, e].join("/"); }
  function withVar(a, b = a + 1) { var c = a + b; var a; return [a, b, c].join("/"); }
  function withFunction(a = 1, b = inner) { function inner() { return "inner"; } return [a, typeof b, inner()].join("/"); }
  function pattern({ p, q = p + 1 } = {}, [r, s = 9] = []) { return [p, q, r, s].join("/"); }
  function undefinedArgs(a = 1, b = 2) { return a + "/" + b; }
  var out = [plain(1), plain(1, undefined), plain(1, null, 3), plain(1, 2, 3, 4, 5), plain(), withLet(1), withLet(1, 5), withLet(1, 5, 0), withVar(1), withVar(1, 10), pattern(), pattern({ p: 1 }, [2]), pattern({ p: 1, q: 5 }, [2, 3]), undefinedArgs(undefined, undefined), undefinedArgs(null, 0), undefinedArgs(void 0, "")];
  try { out.push(withFunction()); } catch (e) { out.push(e.constructor.name); }
  return out.join(" ");
}));
// What a default value sees: the parameters before it and the scope outside, never the body.
show("scopes", twice(function () {
  function seesOuter(a = x) { let x = "body let"; return a + "," + x; }
  function seesOuterVar(a = y) { var y = "body var"; return a + "," + y; }
  function seesEarlier(a, b = a, c = b) { a = "changed"; return [a, b, c].join(","); }
  function tdz(a = b, b = 1) { return a; }
  function selfTdz(a = a) { return a; }
  function closureInParams(a = function () { return x; }, b = () => typeof hidden) { var x = "body var"; let hidden = 1; return a() + "," + b() + "," + x; }
  function closureSeesParam(a, get = () => a, set = (v) => { a = v; }) { var a; set(5); return [a, get()].join(","); }
  function arrowInBody(a, b = a * 2) { let c = b + 1; return (() => a + b + c)(); }
  function shadows(a, b = 1) { let x = a; { let x = b; b = x + 10; } return [x, b].join(","); }
  function classInParams(C = class { m() { return typeof bodyLet; } }) { let bodyLet = 1; return new C().m(); }
  var out = [seesOuter(), seesOuterVar(), seesEarlier(1), arrowInBody(1), shadows(3), closureInParams(), closureSeesParam(1), classInParams()];
  [tdz, selfTdz].forEach(function (f) { try { out.push(f()); } catch (e) { out.push(e.constructor.name); } });
  out.push(tdz(7), selfTdz(8));
  return out.join(" ");
}));
// eval in the parameters and in the body of functions with defaults, strict and not.
show("eval", twice(function () {
  function evalInParam(a = eval("var z = 'declared by eval'; 1"), b = () => typeof z) { return [a, b(), typeof z].join(","); }
  function evalInBody(a = 1) { eval("var w = 2"); return a + w; }
  function evalSeesParam(a, b = eval("a + 1")) { return b; }
  var strictEval = (function () { "use strict"; return function (a = eval("var q = 1; q"), b = () => typeof q) { return a + b(); }; })();
  var out = [evalInBody(), evalSeesParam(1), evalSeesParam(1, 9)];
  try { out.push(evalInParam()); } catch (e) { out.push(e.constructor.name); }
  try { out.push(strictEval()); } catch (e) { out.push(e.constructor.name); }
  return out.join(" ");
}));
// this, arguments and new.target in functions that bind their parameters themselves.
show("this and arguments", twice(function () {
  var o = { n: 7,
    m: function (a = this.n, ...rest) { return [a, rest.length, arguments.length, this === o].join("/"); },
    arrow: function (a = 1) { return (() => this.n + a + arguments.length)(); },
    args: function () { arguments[0] = "changed"; return [arguments.length, arguments[0], Array.prototype.slice.call(arguments, 1).join("+"), Object.prototype.toString.call(arguments)].join("/"); },
    strictArgs: function (a) { "use strict"; arguments[0] = "changed"; return [a, arguments[0], arguments.length].join("/"); },
    nested: function () { var self = this; return (function () { return [arguments.length, this === undefined || this === globalThis, self.n].join("/"); })(1, 2); } };
  function sloppyThis(a = 1) { return this === globalThis || this === undefined; }
  function Ctor(a = 3, ...rest) { this.a = a; this.rest = rest.length; this.target = new.target === Ctor; }
  var made = new Ctor(), made2 = new Ctor(1, 2, 3);
  var detached = o.m;
  var out = [o.m(), o.m(1, 2, 3), o.arrow(), o.arrow(5, 6), o.args(1, 2, 3), o.args(), o.strictArgs(1, 2), o.nested(), sloppyThis(), sloppyThis.call(5) , [made.a, made.rest, made.target].join("/"), [made2.a, made2.rest].join("/"), o.m.call({ n: "other" }), o.m.apply(o, [undefined, 2])];
  try { out.push(detached()); } catch (e) { out.push(e.constructor.name); }
  return out.join(" ");
}));
// Many and few arguments, recursion and errors through such functions.
show("calls", twice(function () {
  function sum(first = 0, ...rest) { return rest.length ? first + sum(...rest) : first; }
  function count() { return arguments.length ? 1 + count.apply(null, Array.prototype.slice.call(arguments, 1)) : 0; }
  function thrower(depth = 0, limit = 3) { if (depth === limit) throw new RangeError("depth " + depth); try { return thrower(depth + 1, limit); } finally { log.push(depth); } }
  function deep(n, acc = []) { acc.push(n); return n ? deep(n - 1, acc) : acc.length; }
  var log = [], big = []; for (var i = 0; i < 12; i++) big.push(i);
  var out = [sum(), sum(1), sum(1, 2, 3, 4, 5), sum.apply(null, big), count(), count(1, 2, 3), count.apply(null, big), deep(50), deep(400)];
  try { thrower(); } catch (e) { out.push(e.message + " after " + log.join("")); }
  function forever(a = 1, ...rest) { return forever(a + 1, ...rest) + 1; }
  try { forever(0, 1, 2); } catch (e) { out.push(e.constructor.name); }
  return out.join(" ");
}));
// Generators, async functions, classes and bound functions are called as before.
show("others", twice(function () {
  function* gen(a = 1, ...rest) { yield a; yield rest.length; return arguments.length; }
  class K { constructor(a = 1) { this.a = a; } static make(...args) { return new K(...args).a; } get twice() { return this.a * 2; } m(b = this.a) { return b; } }
  class L extends K { constructor(a, b = a * 2) { super(b); this.b = b; } m(c = super.m()) { return c + this.b; } }
  var bound = function (a, b = 2, ...rest) { return [this.tag, a, b, rest.length].join("/"); }.bind({ tag: "bound" }, 1);
  var g = gen(5, 6, 7), l = new L(3);
  var out = [g.next().value, g.next().value, g.next().value, K.make(), K.make(4), new K(6).twice, new K(6).m(), new K(6).m(1), l.a, l.b, l.m(), l.m(1), bound(), bound(9, 8, 7)];
  try { K(); } catch (e) { out.push(e.constructor.name); }
  return out.join(" ");
}));
all.join("\n")"#;
    let expected = r#"defaults: 1/2//0 1/2//0 1//3/1 1/2/3+4+5/3 /2//0 1/2/4/7/14 1/5/10/16/32 1/5/0/6/12 1/2/3 1/10/11 /NaN//9 1/2/2/9 1/5/2/3 1/2 null/0 1/ ReferenceError|1/2//0 1/2//0 1//3/1 1/2/3+4+5/3 /2//0 1/2/4/7/14 1/5/10/16/32 1/5/0/6/12 1/2/3 1/10/11 /NaN//9 1/2/2/9 1/5/2/3 1/2 null/0 1/ ReferenceError|1/2//0 1/2//0 1//3/1 1/2/3+4+5/3 /2//0 1/2/4/7/14 1/5/10/16/32 1/5/0/6/12 1/2/3 1/10/11 /NaN//9 1/2/2/9 1/5/2/3 1/2 null/0 1/ ReferenceError
scopes: outer x,body let outer y,body var changed,1,1 6 3,11 outer x,undefined,body var 1,5 undefined ReferenceError ReferenceError 7 8|outer x,body let outer y,body var changed,1,1 6 3,11 outer x,undefined,body var 1,5 undefined ReferenceError ReferenceError 7 8|outer x,body let outer y,body var changed,1,1 6 3,11 outer x,undefined,body var 1,5 undefined ReferenceError ReferenceError 7 8
eval: 3 2 9 1,string,string 1undefined|3 2 9 1,string,string 1undefined|3 2 9 1,string,string 1undefined
this and arguments: 7/0/0/true 1/2/3/true 8 14 3/changed/2+3/[object Arguments] 0/changed//[object Arguments] 1/changed/2 2/true/7 true false 3/0/true 1/2 other/0/0/false 7/1/2/true /0/0/false|7/0/0/true 1/2/3/true 8 14 3/changed/2+3/[object Arguments] 0/changed//[object Arguments] 1/changed/2 2/true/7 true false 3/0/true 1/2 other/0/0/false 7/1/2/true /0/0/false|7/0/0/true 1/2/3/true 8 14 3/changed/2+3/[object Arguments] 0/changed//[object Arguments] 1/changed/2 2/true/7 true false 3/0/true 1/2 other/0/0/false 7/1/2/true /0/0/false
calls: 0 1 15 66 0 3 12 51 401 depth 3 after 210 RangeError|0 1 15 66 0 3 12 51 401 depth 3 after 210 RangeError|0 1 15 66 0 3 12 51 401 depth 3 after 210 RangeError
others: 5 2 3 1 4 12 6 1 6 6 12 7 bound/1/2/0 bound/1/9/2 TypeError|5 2 3 1 4 12 6 1 6 6 12 7 bound/1/2/0 bound/1/9/2 TypeError|5 2 3 1 4 12 6 1 6 6 12 7 bound/1/2/0 bound/1/9/2 TypeError"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The getter and the setter of a property, which the interpreter calls in
/// place of the read or the assignment where the property cache of the
/// place knows them: of the object itself, of a class and of what it
/// inherits from; what a getter sees of `this` and of `arguments`, a
/// getter that reads another, one that throws, and what a setter returns;
/// accessors redefined, made data properties and deleted under the code
/// that reads them; getters that are bound, native, arrows, generators or
/// have a default parameter, and one that calls itself until the stack is
/// full. And `instanceof`, with the method every function inherits and
/// with others. What is expected is what Node prints for the same script.
#[test]
fn accessors_called_in_place_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(accessors_called_in_place)
        .expect("thread")
        .join()
        .expect("checks");
}

fn accessors_called_in_place() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
var print = function (s) { all.push(s); };
function show(name, v) { print(name + ": " + v); }
function twice(f) { var out = []; for (var round = 0; round < 3; round++) { try { out.push(f()); } catch (e) { out.push("threw " + e.constructor.name + " " + e.message); } } return out.join("|"); }
function read(o) { return o.p; }
function write(o, v) { o.p = v; }
// Getters and setters of the object itself, of a class, of what it inherits from.
show("where", twice(function () {
  var log = [];
  var own = { _p: 1, get p() { log.push("get own"); return this._p; }, set p(v) { log.push("set own " + v); this._p = v * 2; return "ignored"; } };
  class K { constructor() { this._p = 10; } get p() { log.push("get K"); return this._p + 1; } set p(v) { log.push("set K " + v); this._p = v; } static get p() { return "static " + this.name; } }
  class L extends K { get p() { return "L:" + super.p; } }
  class M extends K { }
  var k = new K(), l = new L(), m = new M(), child = Object.create(own);
  var out = [read(own), read(k), read(l), read(m), read(child), read(K), read(M)];
  write(own, 3); write(k, 20); write(l, 30); write(m, 40); write(child, 5);
  out.push(read(own), read(k), read(l), read(m), read(child), own._p, child.hasOwnProperty("_p"), child.hasOwnProperty("p"), Object.keys(m).join("+"));
  return out.join() + " " + log.join(";");
}));
// What a getter sees and does: this, arguments, its own property, another getter, a throw.
show("inside", twice(function () {
  var depth = 0, log = [];
  var o = {
    n: 3,
    get self() { return this; },
    get count() { return arguments.length; },
    get nested() { return this.inner + 1; },
    get inner() { return this.n * 2; },
    get thrower() { throw new RangeError("from the getter"); },
    get recursive() { depth++; return depth < 20 ? this.recursive + 1 : 0; },
    get strictThis() { "use strict"; return typeof this; },
    get undefinedValue() { },
    get viaLocal() { var self = this; return self.inner + self.n; },
    set logged(v) { log.push(typeof this + ":" + v + ":" + arguments.length); },
    set thrower2(v) { throw new TypeError("from the setter " + v); },
    set chain(v) { this.logged = v + 1; this.n = v; },
  };
  var out = [o.self === o, o.count, o.nested, o.inner, o.recursive, o.strictThis, String(o.undefinedValue), o.viaLocal];
  try { o.thrower; } catch (e) { out.push(e.message); }
  o.logged = 1; o.chain = 7; out.push(o.n, log.join("+"));
  try { o.thrower2 = 9; } catch (e) { out.push(e.message); }
  var value = (o.logged = "kept"); out.push(value);
  out.push(eval("o.logged = 'by eval'"), (o.chain = 4, o.n));
  var primitive = Object.defineProperty(Number.prototype, "twice", { get: function () { return this * 2; }, configurable: true }); out.push((5).twice, typeof Object.getOwnPropertyDescriptor(Number.prototype, "twice").get); delete Number.prototype.twice;
  return out.join();
}));
// Accessors that change under the code that reads them.
show("changes", twice(function () {
  var o = { get p() { return "first getter"; } }, out = [read(o)];
  Object.defineProperty(o, "p", { get: function () { return "second getter"; }, configurable: true }); out.push(read(o));
  Object.defineProperty(o, "p", { value: "now data", writable: true, configurable: true }); out.push(read(o)); write(o, "written"); out.push(read(o));
  Object.defineProperty(o, "p", { get: undefined, set: undefined, configurable: true }); out.push(String(read(o))); write(o, 1); out.push(String(read(o)));
  Object.defineProperty(o, "p", { set: function (v) { this.q = v; }, configurable: true }); write(o, "through the setter"); out.push(String(read(o)), o.q);
  delete o.p; out.push(String(read(o))); write(o, "plain"); out.push(read(o));
  var proto = { get p() { return "from the prototype"; } }, child = Object.create(proto); out.push(read(child));
  Object.defineProperty(proto, "p", { get: function () { return "prototype changed"; }, configurable: true }); out.push(read(child));
  Object.defineProperty(child, "p", { value: "own now", configurable: true }); out.push(read(child));
  (function () { "use strict"; var g = { get p() { return 1; } }; try { g.p = 2; out.push("no error"); } catch (e) { out.push(e.constructor.name); } })();
  var sloppy = { get p() { return 1; } }; write(sloppy, 2); out.push(read(sloppy));
  return out.join();
}));
// Getters that are not plain functions, and getters called many levels deep.
show("kinds", twice(function () {
  var bound = Object.defineProperty({ tag: "object" }, "p", { get: function () { return this.tag; }.bind({ tag: "bound this" }) });
  var native = Object.defineProperty({}, "p", { get: Math.random.bind(null) });
  var arrow = Object.defineProperty({ tag: 1 }, "p", { get: () => typeof this });
  var withDefault = Object.defineProperty({ n: 2 }, "p", { get: function (a = 5) { return this.n + a; } });
  var generator = Object.defineProperty({}, "p", { get: function* () { yield 1; } });
  function Deep(n) { this.n = n; }
  Object.defineProperty(Deep.prototype, "p", { get: function () { return this.n ? new Deep(this.n - 1).p + 1 : 0; } });
  var out = [read(bound), typeof read(native), read(arrow), read(withDefault), typeof read(generator).next, new Deep(300).p];
  var proxy = new Proxy({ get p() { return this === proxy; } }, {}); out.push(read(proxy));
  var arrayLike = Object.defineProperty([1, 2], "p", { get: function () { return this.length; } }); out.push(read(arrayLike));
  function overflow() { return Object.defineProperty({}, "p", { get: function () { return this.p; } }).p; }
  try { overflow(); } catch (e) { out.push(e.constructor.name); }
  return out.join();
}));
// instanceof with the default method and with others.
show("instanceof", twice(function () {
  function A() {} function B() {} B.prototype = Object.create(A.prototype);
  class C {} class D extends C {}
  var custom = { [Symbol.hasInstance]: function (v) { return v === 1; } };
  class Even { static [Symbol.hasInstance](v) { return typeof v === "number" && v % 2 === 0; } }
  var boundA = A.bind(null), arrow = () => {}, noPrototype = function () {}; noPrototype.prototype = 5;
  var values = [new A(), new B(), new D(), [], {}, null, undefined, 1, 2, "s", function () {}, Object.create(null), Object.create(A.prototype)];
  var out = values.map(function (v) { return [v instanceof A, v instanceof B, v instanceof C, v instanceof Object, v instanceof Function, v instanceof Array, v instanceof custom, v instanceof Even, v instanceof boundA].map(Number).join(""); });
  [arrow, noPrototype, {}, 5, null].forEach(function (target) { try { out.push({} instanceof target); } catch (e) { out.push(e.constructor.name); } });
  A.prototype = { replaced: true }; out.push(values[0] instanceof A, Object.create(A.prototype) instanceof A);
  try { Object.defineProperty(Function.prototype, Symbol.hasInstance, { value: function () { return true; } }); out.push("redefined"); } catch (e) { out.push(e.constructor.name); }
  Object.defineProperty(A, Symbol.hasInstance, { value: function (v) { return v === "own method"; }, configurable: true });
  out.push({} instanceof A, "own method" instanceof A, Function.prototype[Symbol.hasInstance].call(B, new B()), Function.prototype[Symbol.hasInstance].call({}, {}));
  delete A[Symbol.hasInstance]; out.push(values[0] instanceof A, Object.create(A.prototype) instanceof A);
  return out.join();
}));
all.join("\n")"#;
    let expected = r#"where: 1,11,L:11,11,1,static K,static M,6,21,L:11,41,10,6,true,false,_p get own;get K;get K;get K;get own;set own 3;set K 20;set K 40;set own 5;get own;get K;get K;get K;get own|1,11,L:11,11,1,static K,static M,6,21,L:11,41,10,6,true,false,_p get own;get K;get K;get K;get own;set own 3;set K 20;set K 40;set own 5;get own;get K;get K;get K;get own|1,11,L:11,11,1,static K,static M,6,21,L:11,41,10,6,true,false,_p get own;get K;get K;get K;get own;set own 3;set K 20;set K 40;set own 5;get own;get K;get K;get K;get own
inside: true,0,7,6,19,object,undefined,9,from the getter,7,object:1:1+object:8:1,from the setter 9,kept,by eval,4,10,function|true,0,7,6,19,object,undefined,9,from the getter,7,object:1:1+object:8:1,from the setter 9,kept,by eval,4,10,function|true,0,7,6,19,object,undefined,9,from the getter,7,object:1:1+object:8:1,from the setter 9,kept,by eval,4,10,function
changes: first getter,second getter,now data,written,undefined,undefined,undefined,through the setter,undefined,plain,from the prototype,prototype changed,own now,TypeError,1|first getter,second getter,now data,written,undefined,undefined,undefined,through the setter,undefined,plain,from the prototype,prototype changed,own now,TypeError,1|first getter,second getter,now data,written,undefined,undefined,undefined,through the setter,undefined,plain,from the prototype,prototype changed,own now,TypeError,1
kinds: bound this,number,object,7,function,300,true,2,RangeError|bound this,number,object,7,function,300,true,2,RangeError|bound this,number,object,7,function,300,true,2,RangeError
instanceof: 100100001,110100001,001100000,000101000,000100000,000000000,000000000,000000100,000000010,000000000,000110000,000000000,100100001,TypeError,TypeError,TypeError,TypeError,TypeError,false,true,TypeError,false,true,true,false,false,true|100100001,110100001,001100000,000101000,000100000,000000000,000000000,000000100,000000010,000000000,000110000,000000000,100100001,TypeError,TypeError,TypeError,TypeError,TypeError,false,true,TypeError,false,true,true,false,false,true|100100001,110100001,001100000,000101000,000100000,000000000,000000000,000000100,000000010,000000000,000110000,000000000,100100001,TypeError,TypeError,TypeError,TypeError,TypeError,false,true,TypeError,false,true,true,false,false,true"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// The constructor, the methods, the getters and the setters of a class
/// are strict mode code in a script that is not: an assignment to a name
/// that is not declared and a write that fails throw, `this` is not made
/// an object and is undefined where no object is given, and the functions
/// made inside them inherit it. Functions outside a class keep the mode of
/// the code around them. What is expected is what Node prints for the same
/// script.
#[test]
fn class_elements_are_strict_mode_code() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(class_elements_are_strict)
        .expect("thread")
        .join()
        .expect("checks");
}

fn class_elements_are_strict() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
var print = function (s) { all.push(s); };
function attempt(name, f) { var v; try { v = f(); } catch (e) { v = e.constructor.name; } print(name + ": " + v); }
var frozen = Object.freeze({ x: 1 });
// Constructors, methods, getters, setters and static methods of a class are strict mode code.
attempt("constructor assigns an undeclared name", function () { class S { constructor() { undeclared1 = 1; } } new S(); return typeof undeclared1; });
attempt("method assigns an undeclared name", function () { class S { m() { undeclared2 = 1; } } new S().m(); return typeof undeclared2; });
attempt("static method", function () { class S { static m() { undeclared3 = 1; } } S.m(); return typeof undeclared3; });
attempt("getter", function () { class S { get p() { undeclared4 = 1; return 1; } } return new S().p; });
attempt("setter", function () { class S { set p(v) { undeclared5 = v; } } new S().p = 1; return typeof undeclared5; });
attempt("private method", function () { class S { #m() { undeclared6 = 1; } run() { this.#m(); } } new S().run(); return typeof undeclared6; });
attempt("generator method", function () { class S { *g() { undeclared7 = 1; yield 1; } } return new S().g().next().value; });
attempt("async method", function () { class S { async a() { return typeof this; } } return typeof new S().a().then; });
attempt("derived constructor", function () { class A {} class B extends A { constructor() { super(); undeclared8 = 1; } } new B(); return typeof undeclared8; });
attempt("class expression", function () { var S = class { m() { undeclared9 = 1; } }; new S().m(); return typeof undeclared9; });
attempt("constructor with fields", function () { class S { f = 1; constructor() { undeclared10 = 1; } } new S(); return typeof undeclared10; });
attempt("field initializer", function () { class S { f = (undeclared11 = 1); } new S(); return typeof undeclared11; });
attempt("this of a detached method", function () { class S { m() { return typeof this; } } var m = new S().m; return m(); });
attempt("this of a nested function", function () { class S { m() { return (function () { return typeof this; })(); } } return new S().m(); });
attempt("this of a nested function in a constructor", function () { class S { constructor() { this.t = (function () { return typeof this; })(); } } return new S().t; });
attempt("this of a method called with a primitive", function () { class S { m() { return typeof this; } } return S.prototype.m.call(5); });
attempt("write to a frozen object", function () { class S { m() { frozen.x = 2; } } new S().m(); return frozen.x; });
attempt("write to a getter only", function () { class S { get p() { return 1; } m() { this.p = 2; } } new S().m(); return "no error"; });
attempt("delete of a fixed property", function () { class S { m() { return delete Object.prototype; } } return new S().m(); });
attempt("arguments is not tied to parameters", function () { class S { m(a) { arguments[0] = 2; return a; } } return new S().m(1); });
attempt("nested arrow and function inherit it", function () { class S { m() { return [(() => { undeclared12 = 1; }), function () { undeclared13 = 1; }]; } } var fs = new S().m(), out = []; fs.forEach(function (f) { try { f(); out.push("no error"); } catch (e) { out.push(e.constructor.name); } }); return out.join(); });
// Functions outside a class keep the mode of the code around them.
attempt("sloppy function", function () { function f() { undeclared14 = 1; return typeof this; } return f() + typeof undeclared14; });
attempt("sloppy method of an object literal", function () { var o = { m() { undeclared15 = 1; return typeof this; } }; var m = o.m; return m() + typeof undeclared15; });
attempt("function with the directive", function () { function f() { "use strict"; undeclared16 = 1; } f(); return "no error"; });
attempt("function after a class", function () { class S { m() {} } function f() { undeclared17 = 1; return typeof this; } return f(); });
attempt("function made by a class method's caller", function () { class S { run(f) { return f(); } } return new S().run(function () { undeclared18 = 1; return typeof this; }); });
all.join("\n")"#;
    let expected = r#"constructor assigns an undeclared name: ReferenceError
method assigns an undeclared name: ReferenceError
static method: ReferenceError
getter: ReferenceError
setter: ReferenceError
private method: ReferenceError
generator method: ReferenceError
async method: function
derived constructor: ReferenceError
class expression: ReferenceError
constructor with fields: ReferenceError
field initializer: ReferenceError
this of a detached method: undefined
this of a nested function: undefined
this of a nested function in a constructor: undefined
this of a method called with a primitive: number
write to a frozen object: TypeError
write to a getter only: TypeError
delete of a fixed property: TypeError
arguments is not tied to parameters: 1
nested arrow and function inherit it: ReferenceError,ReferenceError
sloppy function: objectnumber
sloppy method of an object literal: objectnumber
function with the directive: ReferenceError
function after a class: object
function made by a class method's caller: object"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// `new` of a function or of a class that is not derived, which the
/// interpreter runs in place where the constructor's code needs no
/// Function Environment Record: what it gives where the constructor
/// returns an object, a primitive or nothing; the prototype of the object
/// as the function's "prototype" is when `new` runs, an object or not;
/// fields and private fields; arguments of every kind, new.target, arrow
/// functions that use `this`, derived classes and bound functions, which
/// go the general way; what is not a constructor; constructors that throw,
/// at every depth and until the stack is full; and many objects made with
/// collections between them. What is expected is what Node prints for the
/// same script.
#[test]
fn constructors_run_in_place_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(constructors_run_in_place)
        .expect("thread")
        .join()
        .expect("checks");
}

fn constructors_run_in_place() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var all = [];
var print = function (s) { all.push(s); };
function show(name, v) { print(name + ": " + v); }
function twice(f) { var out = []; for (var round = 0; round < 3; round++) { try { out.push(f()); } catch (e) { out.push("threw " + e.constructor.name + " " + e.message); } } return out.join("|"); }
function proto(o) { return Object.getPrototypeOf(o); }
// What `new` gives: the object made for the constructor, or the object the constructor returns.
show("results", twice(function () {
  function Plain(a) { this.a = a; }
  function ReturnsObject() { this.ignored = 1; return { returned: true }; }
  function ReturnsPrimitive() { this.kept = 1; return 5; }
  function ReturnsUndefined() { this.kept = 2; return; }
  function ReturnsNull() { this.kept = 3; return null; }
  function ReturnsFunction() { return ReturnsFunction; }
  function ReturnsArray() { return [1, 2]; }
  function Empty() {}
  class K { constructor(a, b) { this.a = a; this.b = b; } sum() { return this.a + this.b; } }
  class Fields { x = 1; y = this.x + 1; #z = 5; constructor(w) { this.w = w + this.#z; } static make() { return new Fields(10); } }
  var out = [new Plain(1).a, new ReturnsObject().returned, new ReturnsObject().ignored, new ReturnsPrimitive().kept, new ReturnsUndefined().kept, new ReturnsNull().kept, new ReturnsFunction() === ReturnsFunction, new ReturnsArray().length, Object.keys(new Empty()).length, new K(2, 3).sum(), new K().a, JSON.stringify(new Fields(1)), JSON.stringify(Fields.make())];
  out.push(new Plain(1) instanceof Plain, proto(new K(1, 2)) === K.prototype, new Plain(1).constructor === Plain, new Plain(1) !== new Plain(1), typeof new Empty(), new K(1, 2) instanceof Object);
  return out.join();
}));
// The prototype of the object: the function's "prototype" as it is when `new` runs.
show("prototypes", twice(function () {
  function F() {} var first = new F();
  F.prototype = { tag: "replaced" }; var second = new F();
  F.prototype = 5; var third = new F();
  F.prototype = null; var fourth = new F();
  function G() {} G.prototype.method = function () { return "method"; };
  var g = new G(); G.prototype.later = "added later";
  class Base { hello() { return "base"; } } class Derived extends Base { hello() { return "derived+" + super.hello(); } }
  var out = [proto(first) === proto(second), second.tag, proto(third) === Object.prototype, proto(fourth) === Object.prototype, g.method(), g.later, new Derived().hello(), proto(new Derived()) === Derived.prototype, new Derived() instanceof Base];
  var bound = F.bind(null); F.prototype = { tag: "for bound" }; out.push(new bound().tag, new bound() instanceof F);
  return out.join();
}));
// Arguments of every kind, new.target, and constructors that are not run in place.
show("arguments", twice(function () {
  function Defaults(a = 1, ...rest) { this.a = a; this.n = rest.length; this.count = arguments.length; }
  function Target() { this.target = new.target === Target; this.viaCall = new.target === undefined; }
  function Arrow() { this.get = () => this; }
  function Pattern({ x, y = x }, [z] = [9]) { this.sum = x + y + z; }
  class D extends Defaults { constructor(...args) { super(...args); this.derived = true; } }
  var a = new Defaults(), b = new Defaults(5, 6, 7), t = new Target(), arrow = new Arrow(), p = new Pattern({ x: 1 }), d = new D(3, 4);
  var out = [a.a, a.n, a.count, b.a, b.n, b.count, t.target, t.viaCall, arrow.get() === arrow, p.sum, d.a, d.n, d.derived, new Defaults(...[1, 2, 3]).n, Reflect.construct(Defaults, [8, 9]).n, Reflect.construct(Target, [], Object).target];
  var nested = new Defaults(new Defaults(1).a, new Defaults(2, 3).n); out.push(nested.a, nested.n);
  return out.join();
}));
// Errors: what is not a constructor, and constructors that throw.
show("errors", twice(function () {
  var out = [], log = [];
  [() => 1, { }, 5, null, undefined, "s", Math.max, function* () {}, async function () {}, { m() {} }.m, class { static m() {} }.m].forEach(function (target) { try { new target(); out.push("made"); } catch (e) { out.push(e.constructor.name); } });
  function Thrower(n) { this.n = n; if (n > 2) throw new RangeError("too big " + n); }
  class StrictClass { constructor() { undeclaredVariable = 1; } }
  for (var i = 0; i < 6; i++) { try { log.push(new Thrower(i).n); } catch (e) { log.push(e.message); } finally { log.push("f"); } }
  try { new StrictClass(); } catch (e) { out.push(e.constructor.name); }
  class C { constructor() { this.x = 1; } } try { C(); } catch (e) { out.push(e.constructor.name); }
  function Deep(n) { this.child = n ? new Deep(n - 1) : null; this.depth = n; }
  var deep = new Deep(200), depth = 0; while (deep.child) { deep = deep.child; depth++; } out.push(depth);
  function Forever() { this.next = new Forever(); } try { new Forever(); } catch (e) { out.push(e.constructor.name); }
  var sum = 0; for (var k = 0; k < 2000; k++) { var point = new Thrower(k % 3); sum += point.n; } out.push(sum);
  return out.join() + " " + log.join("");
}));
// Many objects, with collections between them, and what they keep.
show("many", twice(function () {
  function Node(value, next) { this.value = value; this.next = next; this.label = "node number " + value; }
  class Counter { static made = 0; constructor() { Counter.made++; this.id = Counter.made; } }
  var head = null; for (var i = 0; i < 30000; i++) head = new Node(i, i % 100 === 0 ? null : head);
  var length = 0, last = head; while (last) { length++; last = last.next; }
  var counters = []; for (var j = 0; j < 500; j++) counters.push(new Counter());
  return [length, head.label, head.next.value, counters[499].id - counters[0].id, Counter.made % 500].join();
}));
all.join("\n")"#;
    let expected = r#"results: 1,true,,1,2,3,true,2,0,5,,{"x":1,"y":2,"w":6},{"x":1,"y":2,"w":15},true,true,true,true,object,true|1,true,,1,2,3,true,2,0,5,,{"x":1,"y":2,"w":6},{"x":1,"y":2,"w":15},true,true,true,true,object,true|1,true,,1,2,3,true,2,0,5,,{"x":1,"y":2,"w":6},{"x":1,"y":2,"w":15},true,true,true,true,object,true
prototypes: false,replaced,true,true,method,added later,derived+base,true,true,for bound,true|false,replaced,true,true,method,added later,derived+base,true,true,for bound,true|false,replaced,true,true,method,added later,derived+base,true,true,for bound,true
arguments: 1,0,0,5,2,3,true,false,true,11,3,1,true,2,1,false,1,1|1,0,0,5,2,3,true,false,true,11,3,1,true,2,1,false,1,1|1,0,0,5,2,3,true,false,true,11,3,1,true,2,1,false,1,1
errors: TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,ReferenceError,TypeError,200,RangeError,1999 0f1f2ftoo big 3ftoo big 4ftoo big 5f|TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,ReferenceError,TypeError,200,RangeError,1999 0f1f2ftoo big 3ftoo big 4ftoo big 5f|TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,TypeError,ReferenceError,TypeError,200,RangeError,1999 0f1f2ftoo big 3ftoo big 4ftoo big 5f
many: 100,node number 29999,29998,499,0|100,node number 29999,29998,499,0|100,node number 29999,29998,499,0"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// An object pattern, which the compiler turns into the reads of `o.p`,
/// through their property caches, for every pattern and not only for one
/// with a default value or a computed key: patterns that are plain,
/// renamed and nested, with keys of every kind and a rest element; sources
/// that are not objects; the order in which getters and the traps of a
/// Proxy run, and what is bound when one throws; one pattern over objects
/// of many shapes, and over one whose properties come, change and go;
/// names bound in every way (var, let, const, parameters, a catch clause,
/// loops, a closure, the top level); and patterns across a yield. What is
/// expected is what Node prints for the same script.
#[test]
fn object_patterns_keep_the_language_semantics() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(object_patterns)
        .expect("thread")
        .join()
        .expect("checks");
}

fn object_patterns() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function say() { out.push(Array.prototype.map.call(arguments, function (v) {
  if (typeof v === 'symbol') return v.toString();
  if (typeof v === 'function') return 'fn:' + v.name;
  if (v && typeof v === 'object') { try { return JSON.stringify(v); } catch (e) { return '[obj]'; } }
  return String(v);
}).join(' ')); }
function attempt(name, f) { try { say(name, f()); } catch (e) { say(name, 'threw', e && e.constructor && e.constructor.name); } }

// Plain, renamed, nested, missing.
(function () {
  var { x, y, meta: { id }, none } = { x: 1, y: 2, meta: { id: 3 } };
  say('plain', x, y, id, none);
  let { a: p, b: q } = { a: 'A', b: 'B' };
  const { length } = 'four';
  say('renamed', p, q, length);
  var { 0: first, 1: second, length: len } = ['f', 's'];
  say('array source', first, second, len);
  var { 'quoted key': qk, 12: twelve, 1.5: frac, [`com${'puted'}`]: comp } = { 'quoted key': 'q', 12: 't', '1.5': 'fr', computed: 'c' };
  say('keys', qk, twelve, frac, comp);
  var sym = Symbol('s');
  var { [sym]: bySymbol, [Symbol.iterator]: iter } = { [sym]: 'sym', [Symbol.iterator]: 5 };
  say('symbols', bySymbol, iter);
})();

// Sources that are not objects.
attempt('null', function () { var { a } = null; return a; });
attempt('undefined', function () { var { a } = undefined; return a; });
attempt('empty null', function () { var {} = null; return 'no'; });
attempt('empty undefined', function () { let {} = void 0; return 'no'; });
attempt('empty number', function () { var {} = 5; return 'ok'; });
attempt('number', function () { var { toFixed, missing } = 5; return typeof toFixed + ' ' + missing; });
attempt('string', function () { var { length, 0: c, at } = 'xyz'; return length + c + typeof at; });
attempt('boolean', function () { const { valueOf } = true; return typeof valueOf; });
attempt('symbol', function () { const { description } = Symbol('d'); return description; });
attempt('bigint', function () { const { toString } = 10n; return typeof toString; });
attempt('nested null', function () { var { a: { b } } = { a: null }; return b; });
attempt('nested missing', function () { var { a: { b } } = {}; return b; });

// The order of what runs, and what is bound when a getter throws.
(function () {
  var log = [];
  var source = {
    get b() { log.push('b'); return 2; },
    get a() { log.push('a'); return 1; },
    get c() { log.push('c'); return 3; },
  };
  var { a, c, b } = source;
  say('order', log.join(), a, b, c);
  var one, two;
  try {
    var { one, two } = { one: 'set', get two() { throw new RangeError('no'); } };
  } catch (e) { say('getter threw', e.constructor.name, one, two); }
  try {
    let { u, v } = { u: 1, get v() { throw new EvalError('no'); } };
    say('unreachable', u, v);
  } catch (e) { say('let getter threw', e.constructor.name); }
  try {
    let { w } = { get w() { return w; } };
    say('unreachable', w);
  } catch (e) { say('tdz in getter', e.constructor.name); }
})();

// A Proxy sees every operation, in order.
(function () {
  var log = [];
  var proxy = new Proxy({ a: 1, b: 2, c: 3, [Symbol.for('s')]: 4 }, {
    get(t, k, r) { log.push('get ' + String(k)); return Reflect.get(t, k, r); },
    has(t, k) { log.push('has ' + String(k)); return Reflect.has(t, k); },
    ownKeys(t) { log.push('ownKeys'); return Reflect.ownKeys(t); },
    getOwnPropertyDescriptor(t, k) { log.push('gopd ' + String(k)); return Reflect.getOwnPropertyDescriptor(t, k); },
  });
  var { b, a } = proxy;
  say('proxy', a, b, log.join());
  log.length = 0;
  var { a: A, ...rest } = proxy;
  say('proxy rest', A, rest, Object.getOwnPropertySymbols(rest).length, log.join());
})();

// Rest, with every kind of key excluded.
(function () {
  var key = 'k';
  var { a, 1: one, [key]: k, ...others } = { a: 1, 1: 'one', k: 'K', z: 26, 2: 'two', [Symbol.for('r')]: 'sym' };
  say('rest', a, one, k, others, Object.getOwnPropertySymbols(others).length);
  var { ...copy } = 'ab';
  say('rest of string', copy);
  var { ...none } = 7;
  say('rest of number', none);
  let { x, ...last } = { x: 1 };
  say('empty rest', x, last);
})();

// The same pattern over objects of many shapes: what one remembers of a
// shape must not serve another.
(function () {
  function Base() {}
  Base.prototype.x = 'proto';
  var withGetter = { get x() { return 'getter'; } };
  var frozen = Object.freeze({ x: 'frozen' });
  var sources = [
    { x: 1 }, { y: 2, x: 3 }, { x: 4, z: 5 }, new Base(), Object.create(withGetter),
    new Proxy({ x: 7 }, {}), 'str', 5, [8], { x: undefined }, {}, frozen, withGetter,
    Object.create(null), (function () { var o = Object.create(null); o.x = 'bare'; return o; })(),
    new Map(), function x() {}, { x: null }, { x: 0 }, { x: '' }, { x: NaN },
  ];
  var seen = [];
  for (var round = 0; round < 3; round++) {
    for (var i = 0; i < sources.length; i++) {
      var { x } = sources[i];
      seen.push(String(x));
    }
    Base.prototype.x = 'proto' + round;
    sources[0].w = round;         // a new shape for the first
    delete sources[1].x;          // and none for the second
    sources[2] = { z: 1, x: 'moved' + round };
  }
  say('shapes', seen.join());
  // One place, many shapes, in a loop that runs long enough to be cached.
  var total = 0;
  for (var j = 0; j < 3000; j++) {
    var o = j % 3 === 0 ? { p: 1, q: 2 } : j % 3 === 1 ? { q: 2, p: 1 } : { p: 1 };
    var { p, q } = o;
    total += p + (q === undefined ? 100 : q);
  }
  say('many shapes', total);
  // A property that appears, turns into an accessor, and goes.
  var changing = { n: 1 };
  var got = [];
  for (var k = 0; k < 6; k++) {
    var { n, late } = changing;
    got.push(n + '/' + late);
    if (k === 1) changing.late = 'here';
    if (k === 2) Object.defineProperty(changing, 'n', { get: function () { return 'acc'; }, configurable: true });
    if (k === 3) delete changing.n;
    if (k === 4) Object.setPrototypeOf(changing, { n: 'inherited' });
  }
  say('changing', got.join());
})();

// Names bound in every way: var, let, const, parameters, catch, loops,
// captured by a closure, at the top level.
var { topVar, nested: { deep: topDeep } } = { topVar: 'tv', nested: { deep: 'td' } };
let { topLet } = { topLet: 'tl' };
const { topConst } = { topConst: 'tc' };
say('top', topVar, topDeep, topLet, topConst, globalThis.topVar, globalThis.topLet);
(function () {
  function params({ a, b }, { c: { d } }, e) { return [a, b, d, e, arguments.length].join(); }
  say('params', params({ a: 1, b: 2 }, { c: { d: 3 } }, 4));
  var arrow = ({ a, b }) => a + b;
  say('arrow', arrow({ a: 'x', b: 'y' }));
  attempt('params null', function () { return params(null, {}); });
  attempt('params missing', function () { return params({}, {}); });
  function captured({ a, b }) { return function () { a += 1; return a + b; }; }
  var inner = captured({ a: 1, b: 10 });
  say('captured', inner(), inner());
  try { throw { code: 7, info: { text: 'caught' } }; } catch ({ code, info: { text } }) { say('catch', code, text); }
  var forOf = [];
  for (var { id, tag } of [{ id: 1, tag: 'a' }, { id: 2 }, { tag: 'c', id: 3 }]) forOf.push(id + ':' + tag);
  for (const { id, tag } of [{ id: 4, tag: 'd' }]) forOf.push(id + ':' + tag);
  for (let { id: renamed } of [{ id: 5 }]) forOf.push(() => renamed);
  say('for-of', forOf.slice(0, 4).join(), forOf[4]());
  var closures = [];
  for (let { v } of [{ v: 1 }, { v: 2 }, { v: 3 }]) closures.push(function () { return v; });
  say('closures', closures.map(function (f) { return f(); }).join());
  var entries = [];
  for (const [key, { n }] of Object.entries({ p: { n: 1 }, q: { n: 2 } })) entries.push(key + n);
  say('entries', entries.join());
  var { a: again, a: twice } = { get a() { return Math.random() < 2 ? 'same' : ''; } };
  say('same key twice', again, twice);
  var o = {};
  ({ a: o.x, b: o['y'], c: { d: o.z } } = { a: 1, b: 2, c: { d: 3 } });
  say('assignment', o);
  var s, t;
  ({ s, t } = { s: 'S', t: 'T' });
  say('assignment names', s, t);
  var { fn = function () {}, plain } = { plain: function () {} };
  say('function names', fn.name, plain.name);
  var { withDefault = 'd', given = 'no', nul = 'no' } = { given: 'g', nul: null };
  say('defaults', withDefault, given, nul);
})();

// Getters of a primitive's prototype, and methods read off a pattern.
(function () {
  Object.defineProperty(Number.prototype, 'twice', { get: function () { return this * 2; }, configurable: true });
  var { twice } = 21;
  say('prototype getter', twice);
  delete Number.prototype.twice;
  var { twice: gone } = 21;
  say('after delete', gone);
  var { max, min, PI } = Math;
  say('math', max(1, 2), min(1, 2), PI > 3);
  var { stringify, parse } = JSON;
  say('json', stringify(parse('[1]')));
  var { prototype: { slice }, isArray, from } = Array;
  say('array', slice.call('abc', 1).join(''), isArray([]), typeof from);
})();

// Generators and async functions keep what a pattern bound across a yield.
(function () {
  function* gen({ a, b }) { var { c } = yield a; var { d } = yield b + c; return d; }
  var g = gen({ a: 1, b: 2 });
  say('generator', g.next().value, g.next({ c: 10 }).value, g.next({ d: 'done' }).value);
})();
out.join("\n")"#;
    let expected = r#"plain 1 2 3 undefined
renamed A B 4
array source f s 2
keys q t fr c
symbols sym 5
null threw TypeError
undefined threw TypeError
empty null threw TypeError
empty undefined threw TypeError
empty number ok
number function undefined
string 3xfunction
boolean function
symbol d
bigint function
nested null threw TypeError
nested missing threw TypeError
order a,c,b 1 2 3
getter threw RangeError set undefined
let getter threw EvalError
tdz in getter ReferenceError
proxy 1 2 get b,get a
proxy rest 1 {"b":2,"c":3} 1 get a,ownKeys,gopd b,get b,gopd c,get c,gopd Symbol(s),get Symbol(s)
rest 1 one K {"2":"two","z":26} 1
rest of string {"0":"a","1":"b"}
rest of number {}
empty rest 1 {}
shapes 1,3,4,proto,getter,7,undefined,undefined,undefined,undefined,undefined,frozen,getter,undefined,bare,undefined,undefined,null,0,,NaN,1,undefined,moved0,proto0,getter,7,undefined,undefined,undefined,undefined,undefined,frozen,getter,undefined,bare,undefined,undefined,null,0,,NaN,1,undefined,moved1,proto1,getter,7,undefined,undefined,undefined,undefined,undefined,frozen,getter,undefined,bare,undefined,undefined,null,0,,NaN
many shapes 107000
changing 1/undefined,1/undefined,1/here,acc/here,undefined/here,inherited/here
top tv td tl tc tv undefined
params 1,2,3,4,3
arrow xy
params null threw TypeError
params missing threw TypeError
captured 12 13
catch 7 caught
for-of 1:a,2:undefined,3:c,4:d 5
closures 1,2,3
entries p1,q2
same key twice same same
assignment {"x":1,"y":2,"z":3}
assignment names S T
function names fn plain
defaults d g null
prototype getter 42
after delete undefined
math 2 1 true
json [1]
array bc true function
generator 1 12 done"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// A pattern that ends before its iterator does closes the iterator as
/// IteratorClose does: "return" is called once, with no argument, and what
/// it gives back must be an object. What it gives back and throws, for
/// every kind of array pattern; an error of the pattern itself, which
/// closes the iterator too and is the error thrown; iterators that ran
/// out, that have no "return", and whose "next" fails, which are not
/// closed; and generators, closed through their `finally`. What is
/// expected is what Node prints for the same script.
#[test]
fn patterns_close_their_iterator_as_iterator_close_does() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(patterns_close_their_iterator)
        .expect("thread")
        .join()
        .expect("checks");
}

fn patterns_close_their_iterator() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function make(result, thrower) {
  return { [Symbol.iterator]() { return {
    next() { return { done: false, value: 1 }; },
    return() { out.push('  return called with ' + arguments.length); if (thrower) throw new RangeError('r'); return result; },
  }; } };
}
function attempt(name, f) { try { out.push(name + ': ' + f()); } catch (e) { out.push(name + ': threw ' + e.constructor.name); } }
// What "return" gives back: an object, or a TypeError.
attempt('object', function () { var [a] = make({}); return a; });
attempt('function', function () { var [a] = make(function () {}); return a; });
attempt('number', function () { var [a] = make(5); return a; });
attempt('string', function () { var [a] = make('s'); return a; });
attempt('null', function () { var [a] = make(null); return a; });
attempt('undefined', function () { var [a] = make(undefined); return a; });
attempt('throws', function () { var [a] = make({}, true); return a; });
// Every kind of pattern.
attempt('let', function () { let [a, b] = make({}); return a + b; });
attempt('const number', function () { const [a] = make(5); return a; });
attempt('skips', function () { var [, , a] = make({}); return a; });
attempt('empty', function () { var [] = make(5); return 'bound'; });
attempt('parameter', function () { return (function ([a]) { return a; })(make({})); });
attempt('parameter number', function () { return (function ([a]) { return a; })(make(5)); });
attempt('parameter throws', function () { return (function ([a]) { return a; })(make({}, true)); });
attempt('arrow', function () { return (([a, b]) => a + b)(make(5)); });
attempt('nested', function () { var [[a]] = [make(5)]; return a; });
attempt('nested object', function () { var [{ length }] = make({}); return length; });
attempt('for-of', function () { for (var [a] of [make(5)]) {} return 'ran'; });
attempt('catch', function () { try { throw make(5); } catch ([a]) { return a; } });
// An error from the pattern itself closes the iterator too, once, and is the error that is thrown.
attempt('element is not iterable', function () { var [[a], b] = make(5); return a; });
attempt('element is null', function () { var [{ a }] = { [Symbol.iterator]() { return { next() { return { done: false, value: null }; }, return() { out.push('  return called with ' + arguments.length); return {}; } }; } }; return a; });
attempt('element is null, return throws', function () { var [{ a }] = { [Symbol.iterator]() { return { next() { return { done: false, value: null }; }, return() { out.push('  return called with ' + arguments.length); throw new RangeError('r'); } }; } }; return a; });
// No close where there is nothing to close.
function counted(count) { return { [Symbol.iterator]() { var n = 0; return {
  next() { n++; return { done: n > count, value: n }; },
  return() { out.push('  return called with ' + arguments.length); return {}; },
}; } }; }
attempt('ran out', function () { var [a, b, c] = counted(2); return [a, b, c].join(); });
attempt('just enough', function () { var [a, b] = counted(2); return [a, b].join(); });
attempt('rest', function () { var [a, ...rest] = counted(3); return a + ';' + rest.join(); });
attempt('no return', function () { var [a] = { [Symbol.iterator]() { return { next() { return { done: false, value: 7 }; } }; } }; return a; });
attempt('return is null', function () { var [a] = { [Symbol.iterator]() { return { next() { return { done: false, value: 7 }; }, return: null }; } }; return a; });
attempt('return is a number', function () { var [a] = { [Symbol.iterator]() { return { next() { return { done: false, value: 7 }; }, return: 5 }; } }; return a; });
attempt('next throws', function () { var [a] = { [Symbol.iterator]() { return { next() { throw new EvalError('n'); }, return() { out.push('  return called with ' + arguments.length); return {}; } }; } }; return a; });
attempt('next gives a number', function () { var [a] = { [Symbol.iterator]() { return { next() { return 5; }, return() { out.push('  return called with ' + arguments.length); return {}; } }; } }; return a; });
// A generator is closed through its finally.
function* gen() { try { yield 1; yield 2; } finally { out.push('  finally'); } }
attempt('generator', function () { var [a] = gen(); return a; });
attempt('generator, just enough', function () { var [a, b] = gen(); return a + b; });
attempt('generator, ran out', function () { var [a, b, c] = gen(); return [a, b, c].join(); });
function* stubborn() { try { yield 1; } finally { return 'not an object, but a generator wraps it'; } }
attempt('generator that returns in finally', function () { var [a] = stubborn(); return a; });
out.join("\n")"#;
    let expected = r#"  return called with 0
object: 1
  return called with 0
function: 1
  return called with 0
number: threw TypeError
  return called with 0
string: threw TypeError
  return called with 0
null: threw TypeError
  return called with 0
undefined: threw TypeError
  return called with 0
throws: threw RangeError
  return called with 0
let: 2
  return called with 0
const number: threw TypeError
  return called with 0
skips: 1
  return called with 0
empty: threw TypeError
  return called with 0
parameter: 1
  return called with 0
parameter number: threw TypeError
  return called with 0
parameter throws: threw RangeError
  return called with 0
arrow: threw TypeError
  return called with 0
nested: threw TypeError
  return called with 0
nested object: undefined
  return called with 0
for-of: threw TypeError
  return called with 0
catch: threw TypeError
  return called with 0
element is not iterable: threw TypeError
  return called with 0
element is null: threw TypeError
  return called with 0
element is null, return throws: threw TypeError
ran out: 1,2,
  return called with 0
just enough: 1,2
rest: 1;2,3
no return: 7
return is null: 7
return is a number: threw TypeError
next throws: threw EvalError
next gives a number: threw TypeError
  finally
generator: 1
  finally
generator, just enough: 3
  finally
generator, ran out: 1,2,
generator that returns in finally: 1"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}

/// An Array that is iterated is walked element by element, with no
/// iterator object, only while that is what iterating it comes to: its
/// %Symbol.iterator% method is the original, and so is "next" of
/// %ArrayIteratorPrototype% as the iteration begins. Every reader of an
/// Array (a pattern, `for`-`of`, spread, the constructors of the keyed
/// collections, Object.fromEntries, Array.from, `yield*`) with "next"
/// replaced, put back, deleted and made an accessor; "return" on any of
/// the iterator's prototypes, looked for and called with an Array Iterator
/// that goes on where the walk was, once, with no argument, and what it
/// gives back and throws; `yield*` thrown into and returned from; holes,
/// accessors and arrays that change under the walk; and the array's own
/// ways of being iterated. What is expected is what Node prints for the
/// same script.
#[test]
fn arrays_are_walked_only_as_iterating_them_would() {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(arrays_walked)
        .expect("thread")
        .join()
        .expect("checks");
}

fn arrays_walked() {
    let (mut engine, _, _) = limited(64 << 20, (6 << 20) / 4 * 3);
    let script = r#"var out = [];
function say() { out.push(Array.prototype.map.call(arguments, function (v) {
  if (typeof v === 'function') return 'fn:' + v.name;
  if (v && typeof v === 'object') { try { return JSON.stringify(v); } catch (e) { return '[obj]'; } }
  return String(v);
}).join(' ')); }
function attempt(name, f) { try { say(name, f()); } catch (e) { say(name, 'threw', e && e.constructor ? e.constructor.name : e); } }
var IterProto = Object.getPrototypeOf([][Symbol.iterator]());
var originalNext = IterProto.next;
var values = Array.prototype.values;
function tag(o) { return Object.prototype.toString.call(o); }

// Every reader of an Array, with nothing changed.
function readers(label) {
  var log = [];
  var [a, b] = [1, 2, 3]; log.push('pattern ' + a + b);
  var [c, d, e] = [1, 2]; log.push('long pattern ' + c + d + e);
  var [f, ...g] = [1, 2, 3]; log.push('rest ' + f + g.join(''));
  var [, h, , i = 'i'] = [1, 2, 3]; log.push('skips ' + h + i);
  var [[j, k], { l }] = [[1, 2], { l: 3 }]; log.push('nested ' + j + k + l);
  var sum = 0; for (var x of [1, 2, 3]) sum += x; log.push('for-of ' + sum);
  for (var y of [1, 2, 3]) { if (y === 2) break; } log.push('break ' + y);
  log.push('spread ' + [...[1, 2], ...[3]].join(''));
  log.push('call spread ' + Math.max(...[1, 5, 2]));
  log.push('new spread ' + new Array(...[7, 8]).join(''));
  log.push('set ' + new Set([1, 2, 2]).size);
  log.push('map ' + new Map([[1, 'a'], [2, 'b']]).get(2));
  log.push('weakset ' + (new WeakSet([{}, {}]) instanceof WeakSet));
  log.push('fromEntries ' + JSON.stringify(Object.fromEntries([['p', 1], ['q', 2]])));
  log.push('from ' + Array.from([1, 2]).join(''));
  log.push('params ' + (function (p, [q, r]) { return p + q + r; })(1, [2, 3]));
  log.push('arrow ' + (([s, t]) => s + t)([4, 5]));
  function* gen() { yield* [1, 2]; }
  log.push('yield* ' + [...gen()].join(''));
  log.push('entries ' + Object.entries({ m: 1 }).map(([key, value]) => key + value).join());
  say(label, log.join(' | '));
}
readers('plain');

// "next" replaced: read once as each iteration begins, and called by every
// reader. Put back, and nothing calls it.
(function () {
  var calls = 0;
  IterProto.next = function () { calls++; return originalNext.call(this); };
  readers('patched next');
  say('patched next calls', calls);
  var perReader = [];
  function count(f) { calls = 0; f(); perReader.push(calls); }
  count(function () { var [a, b] = [1, 2, 3]; });
  count(function () { var [a, b, c] = [1, 2]; });
  count(function () { for (var x of [1, 2, 3]) {} });
  count(function () { return [...[1, 2]]; });
  count(function () { return Math.max(...[1, 2]); });
  count(function () { return new Set([1, 2]); });
  count(function () { return new Map([[1, 2]]); });
  count(function () { return new WeakSet([{}]); });
  count(function () { return Object.fromEntries([['a', 1]]); });
  count(function () { return Array.from([1, 2]); });
  say('per reader', perReader.join());
  // An iteration that began with the original goes on with it.
  calls = 0;
  IterProto.next = originalNext;
  var seen = [];
  for (var v of [1, 2, 3]) {
    seen.push(v);
    IterProto.next = function () { calls++; return { done: true }; };
  }
  say('replaced while running', seen.join(), calls);
  for (var w of [1, 2, 3]) seen.push(w);
  say('then', seen.join(), calls);
  IterProto.next = originalNext;
  calls = 0;
  readers('restored');
  say('restored calls', calls);
  // Ends early, returns nonsense, throws.
  IterProto.next = function () { return { done: true, value: 'never' }; };
  var [none] = [1, 2]; var n = 0; for (var q of [1, 2]) n++;
  say('done at once', none, n, [...[1, 2]].length);
  IterProto.next = function () { return 5; };
  attempt('not an object', function () { var [z] = [1]; return z; });
  attempt('not an object for-of', function () { for (var z of [1]) {} });
  IterProto.next = function () { throw new RangeError('next'); };
  attempt('throws', function () { var [z] = [1]; return z; });
  attempt('throws in spread', function () { return [...[1]]; });
  IterProto.next = function () { return { get done() { return false; }, get value() { return 'from getter'; } }; };
  var [viaGetter] = [1]; say('result getters', viaGetter);
  delete IterProto.next;
  attempt('deleted', function () { var [z] = [1]; return z; });
  attempt('deleted for-of', function () { for (var z of [1]) {} return 'ran'; });
  Object.defineProperty(IterProto, 'next', { get: function () { return originalNext; }, configurable: true });
  var [byAccessor, second] = [1, 2]; say('accessor next', byAccessor, second);
  Object.defineProperty(IterProto, 'next', { value: originalNext, writable: true, configurable: true, enumerable: false });
  readers('back');
})();

// "return": looked for on the iterator as an iteration ends early, and
// called with it.
(function () {
  var log = [];
  function early() {
    var [a] = [1, 2]; log.push('a' + a);
    var [b, c] = [1, 2]; log.push('exact');
    var [d, e, f] = [1, 2]; log.push('exhausted');
    for (var x of [1, 2]) break; log.push('break');
    for (var y of [1, 2]) {} log.push('ran out');
    (function () { for (var z of [1, 2]) return; })(); log.push('returned');
    try { for (var w of [1, 2]) throw new EvalError('body'); } catch (err) { log.push('threw ' + err.constructor.name); }
    outer: for (var o of [1, 2]) { for (var i of [1, 2]) continue outer; } log.push('continue outer');
    var [g, ...rest] = [1, 2, 3]; log.push('rest');
  }
  early(); early();
  say('no return', log.join());
  log.length = 0;
  IterProto.return = function (value) { log.push('RETURN ' + tag(this) + ' ' + arguments.length); return {}; };
  early();
  say('return on the prototype', log.join(' '));
  delete IterProto.return;
  log.length = 0; early();
  say('return deleted', log.join());
  log.length = 0;
  Object.prototype.return = function () { log.push('RETURN'); return {}; };
  early();
  say('return on Object.prototype', log.join(' '));
  delete Object.prototype.return;
  var IteratorProto = Object.getPrototypeOf(IterProto);
  log.length = 0;
  IteratorProto.return = function () { log.push('RETURN'); return {}; };
  var [h] = [1, 2];
  say('return on %IteratorPrototype%', log.join(' '));
  delete IteratorProto.return;
  // What "return" gives back and throws.
  IterProto.return = function () { return 5; };
  attempt('return gives a number', function () { var [a] = [1, 2]; return a; });
  attempt('return gives a number, break', function () { for (var x of [1, 2]) break; return 'ok'; });
  attempt('return gives a number, body threw', function () { for (var x of [1, 2]) throw new EvalError('body'); });
  IterProto.return = function () { throw new RangeError('return'); };
  attempt('return throws', function () { var [a] = [1, 2]; return a; });
  attempt('return throws, body threw', function () { for (var x of [1, 2]) throw new EvalError('body'); });
  IterProto.return = 5;
  attempt('return is a number', function () { var [a] = [1, 2]; return a; });
  IterProto.return = null;
  attempt('return is null', function () { var [a] = [1, 2]; return a; });
  IterProto.return = undefined;
  attempt('return is undefined', function () { for (var x of [1, 2]) break; return 'ok'; });
  var gets = 0;
  Object.defineProperty(IterProto, 'return', { get: function () { gets++; return function () { log.push('GOT'); return {}; }; }, configurable: true });
  log.length = 0;
  var [i] = [1, 2]; for (var x of [1, 2]) break;
  say('return by a getter', gets, log.join(' '));
  delete IterProto.return;
  // The iterator "return" is called with goes on where the walk was.
  var kept;
  IterProto.return = function () { kept = this; return {}; };
  var [first, second] = [10, 20, 30, 40];
  say('the iterator', tag(kept), JSON.stringify(kept.next()), JSON.stringify(kept.next()), JSON.stringify(kept.next()));
  delete IterProto.return;
})();

// yield* over an Array, thrown into and returned from.
(function () {
  function* gen() { try { var got = yield* [1, 2, 3]; say('got', got); } catch (e) { say('caught', e.constructor.name); return 'after'; } }
  var it = gen(); it.next();
  attempt('throw into', function () { return it.throw(new RangeError('x')); });
  it = gen(); it.next();
  attempt('return from', function () { return it.return('r'); });
  Array.prototype.throw = function () { return { done: true, value: 'wrong' }; };
  Array.prototype.return = function () { return { done: true, value: 'wrong' }; };
  it = gen(); it.next();
  attempt('throw into, Array.prototype.throw', function () { return it.throw(new RangeError('x')); });
  it = gen(); it.next();
  attempt('return from, Array.prototype.return', function () { return it.return('r'); });
  var [a] = [1, 2]; for (var x of [1, 2]) break;
  delete Array.prototype.throw; delete Array.prototype.return;
  IterProto.throw = function (e) { say('THROW', tag(this), e.constructor.name); return { done: false, value: 'thrown into' }; };
  it = gen(); it.next();
  attempt('throw method', function () { return it.throw(new RangeError('x')); });
  attempt('and on', function () { return it.next(); });
  delete IterProto.throw;
  IterProto.return = function (v) { say('RETURN', tag(this), v); return { done: true, value: 'returned ' + v }; };
  it = gen(); it.next();
  attempt('return method', function () { return it.return('r'); });
  delete IterProto.return;
  function* all() { yield* [1, 2]; yield* []; yield* [3]; return 'end'; }
  var seen = []; var step, runner = all();
  while (!(step = runner.next()).done) seen.push(step.value);
  say('yield* values', seen.join(), step.value);
})();

// Holes, accessors, and arrays that change under the walk.
(function () {
  Object.defineProperty(Array.prototype, 1, { get: function () { return 'proto'; }, configurable: true });
  var [a, b = 'default', c] = [1, , 3]; say('hole to getter, defaults', a, b, c);
  var [d, e, f] = [1, , 3]; say('hole to getter', d, e, f);
  var text = ''; for (var v of [1, , 3]) text += v + ';'; say('for-of', text);
  say('spread', [...[1, , 3]].length, [...[1, , 3]][1]);
  delete Array.prototype[1];
  var [g, h = 'default', i] = [1, , 3]; say('plain hole', g, h, i);
  var withAccessor = [1, 2, 3];
  var reads = 0;
  Object.defineProperty(withAccessor, 1, { get: function () { reads++; return 'acc'; } });
  var [j, k, l] = withAccessor; say('accessor element', j, k, l, reads);
  var [m, n = 'd', o] = withAccessor; say('accessor element, defaults', m, n, o, reads);
  text = ''; for (var w of withAccessor) text += w + ';'; say('accessor for-of', text, reads);
  var shrinking = [1, 2, 3, 4];
  Object.defineProperty(shrinking, 0, { get: function () { shrinking.length = 2; return 'cut'; } });
  var [p, q, r] = shrinking; say('shrinks', p, q, r);
  var growing = [1, 2];
  text = ''; for (var x of growing) { if (growing.length < 5) growing.push(x * 10); text += x + ';'; }
  say('grows', text);
  var cleared = [1, 2, 3];
  text = ''; for (var y of cleared) { text += y + ';'; cleared.length = 0; }
  say('cleared', text);
  var sparse = []; sparse[3] = 'last';
  var [s0, s1, s2, s3, s4] = sparse; say('sparse', s0, s1, s2, s3, s4);
  var frozen = Object.freeze([1, 2]);
  var [t0, t1] = frozen; say('frozen', t0, t1);
  var holder = [undefined, null, 0, '', NaN, false];
  var [u0 = 'd', u1 = 'd', u2 = 'd', u3 = 'd', u4 = 'd', u5 = 'd'] = holder; say('falsy', u0, u1, u2, u3, u4, u5);
})();

// The array's own way of being iterated.
(function () {
  var own = [1, 2];
  own[Symbol.iterator] = function* () { yield 'own'; };
  var [a] = own; say('own', a, [...own].join(), new Set(own).size, Array.from(own).join());
  say('own entries', JSON.stringify(Object.fromEntries(Object.assign([['x', 1]], { [Symbol.iterator]: function* () { yield ['y', 2]; } }))));
  say('own map', new Map(Object.assign([[1, 2]], { [Symbol.iterator]: function* () { yield [3, 4]; } })).get(3));
  class Sub extends Array { *[Symbol.iterator]() { yield 'sub'; } }
  var [b] = Sub.from([1]); say('subclass', b);
  class Plain extends Array {}
  var [c, d] = Plain.from([5, 6]); say('plain subclass', c, d);
  var calls = 0;
  Array.prototype[Symbol.iterator] = function () { calls++; return values.call(this); };
  var [e] = [7]; for (var x of [1]) {} say('replaced', e, calls, [...[1, 2]].join(''));
  Array.prototype[Symbol.iterator] = values;
  calls = 0; var [f] = [8]; say('restored', f, calls);
  Object.defineProperty(Array.prototype, Symbol.iterator, { get: function () { calls++; return values; }, configurable: true });
  var [g] = [9]; for (var y of [1, 2]) {} say('by a getter', g, calls);
  Object.defineProperty(Array.prototype, Symbol.iterator, { value: values, writable: true, configurable: true, enumerable: false });
  Array.prototype[Symbol.iterator] = Array.prototype.keys;
  var [h, i] = ['a', 'b']; say('keys', h, i);
  Array.prototype[Symbol.iterator] = Array.prototype.entries;
  var [[k0, v0]] = ['a']; say('entries', k0, v0);
  Array.prototype[Symbol.iterator] = values;
  delete Array.prototype[Symbol.iterator];
  attempt('none', function () { var [z] = [1]; return z; });
  attempt('none for-of', function () { for (var z of [1]) {} });
  Array.prototype[Symbol.iterator] = 5;
  attempt('a number', function () { var [z] = [1]; return z; });
  Object.defineProperty(Array.prototype, Symbol.iterator, { value: values, writable: true, configurable: true, enumerable: false });
  var proxied = new Proxy([1, 2], {});
  var [p0, p1] = proxied; say('proxy', p0, p1);
  var noProto = [1, 2]; Object.setPrototypeOf(noProto, null);
  attempt('null prototype', function () { var [z] = noProto; return z; });
  var other = [1, 2]; Object.setPrototypeOf(other, { [Symbol.iterator]: function* () { yield 'other'; } });
  var [o0] = other; say('other prototype', o0);
  var args = (function () { return arguments; })(1, 2, 3);
  var [a0, a1] = args; say('arguments', a0, a1, [...args].length);
  var [t0, t1] = new Uint8Array([4, 5]); say('typed', t0, t1);
  var [c0, c1] = 'hi'; say('string', c0, c1);
  var [m0] = new Map([[1, 2]]); var [s0] = new Set([3]); say('map and set', m0, s0);
  readers('at the end');
})();
out.join("\n")"#;
    let expected = r#"plain pattern 12 | long pattern 12undefined | rest 123 | skips 2i | nested 123 | for-of 6 | break 2 | spread 123 | call spread 5 | new spread 78 | set 2 | map b | weakset true | fromEntries {"p":1,"q":2} | from 12 | params 6 | arrow 9 | yield* 12 | entries m1
patched next pattern 12 | long pattern 12undefined | rest 123 | skips 2i | nested 123 | for-of 6 | break 2 | spread 123 | call spread 5 | new spread 78 | set 2 | map b | weakset true | fromEntries {"p":1,"q":2} | from 12 | params 6 | arrow 9 | yield* 12 | entries m1
patched next calls 60
per reader 2,3,4,3,3,3,2,2,2,3
replaced while running 1,2,3 0
then 1,2,3 1
restored pattern 12 | long pattern 12undefined | rest 123 | skips 2i | nested 123 | for-of 6 | break 2 | spread 123 | call spread 5 | new spread 78 | set 2 | map b | weakset true | fromEntries {"p":1,"q":2} | from 12 | params 6 | arrow 9 | yield* 12 | entries m1
restored calls 0
done at once undefined 0 0
not an object threw TypeError
not an object for-of threw TypeError
throws threw RangeError
throws in spread threw RangeError
result getters from getter
deleted threw TypeError
deleted for-of threw TypeError
accessor next 1 2
back pattern 12 | long pattern 12undefined | rest 123 | skips 2i | nested 123 | for-of 6 | break 2 | spread 123 | call spread 5 | new spread 78 | set 2 | map b | weakset true | fromEntries {"p":1,"q":2} | from 12 | params 6 | arrow 9 | yield* 12 | entries m1
no return a1,exact,exhausted,break,ran out,returned,threw EvalError,continue outer,rest,a1,exact,exhausted,break,ran out,returned,threw EvalError,continue outer,rest
return on the prototype RETURN [object Array Iterator] 0 a1 RETURN [object Array Iterator] 0 exact exhausted RETURN [object Array Iterator] 0 break ran out RETURN [object Array Iterator] 0 returned RETURN [object Array Iterator] 0 threw EvalError RETURN [object Array Iterator] 0 RETURN [object Array Iterator] 0 continue outer rest
return deleted a1,exact,exhausted,break,ran out,returned,threw EvalError,continue outer,rest
return on Object.prototype RETURN a1 RETURN exact exhausted RETURN break ran out RETURN returned RETURN threw EvalError RETURN RETURN continue outer rest
return on %IteratorPrototype% RETURN
return gives a number threw TypeError
return gives a number, break threw TypeError
return gives a number, body threw threw EvalError
return throws threw RangeError
return throws, body threw threw EvalError
return is a number threw TypeError
return is null 1
return is undefined ok
return by a getter 2 GOT GOT
the iterator [object Array Iterator] {"value":30,"done":false} {"value":40,"done":false} {"done":true}
caught TypeError
throw into {"value":"after","done":true}
return from {"value":"r","done":true}
caught TypeError
throw into, Array.prototype.throw {"value":"after","done":true}
return from, Array.prototype.return {"value":"r","done":true}
THROW [object Array Iterator] RangeError
throw method {"done":false,"value":"thrown into"}
and on {"value":2,"done":false}
RETURN [object Array Iterator] r
return method {"value":"returned r","done":true}
yield* values 1,2,3 end
hole to getter, defaults 1 proto 3
hole to getter 1 proto 3
for-of 1;proto;3;
spread 3 proto
plain hole 1 default 3
accessor element 1 acc 3 1
accessor element, defaults 1 acc 3 2
accessor for-of 1;acc;3; 3
shrinks cut 2 undefined
grows 1;2;10;20;100;
cleared 1;
sparse undefined undefined undefined last undefined
frozen 1 2
falsy d null 0  NaN false
own own own 1 own
own entries {"y":2}
own map 4
subclass sub
plain subclass 5 6
replaced 7 2 12
restored 8 0
by a getter 9 2
keys 0 1
entries [0,0] [1,"a"]
none threw TypeError
none for-of threw TypeError
a number threw TypeError
proxy 1 2
null prototype threw TypeError
other prototype other
arguments 1 2 3
typed 4 5
string h i
map and set [1,2] 3
at the end pattern 12 | long pattern 12undefined | rest 123 | skips 2i | nested 123 | for-of 6 | break 2 | spread 123 | call spread 5 | new spread 78 | set 2 | map b | weakset true | fromEntries {"p":1,"q":2} | from 12 | params 6 | arrow 9 | yield* 12 | entries m1"#;
    let Ok(Completion::String(found)) = engine.evaluate(script, false) else {
        panic!("the checks did not run");
    };
    let found = std::string::String::from_utf8(found).expect("text");
    for (found, expected) in found.lines().zip(expected.lines()) {
        assert_eq!(found, expected);
    }
    assert_eq!(found.lines().count(), expected.lines().count());
}
