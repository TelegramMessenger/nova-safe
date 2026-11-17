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
