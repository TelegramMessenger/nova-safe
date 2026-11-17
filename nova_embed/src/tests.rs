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

/// A class field's initializer runs when an instance is made, after the
/// function that declared the class has returned: the bindings it reads must
/// not live in that function's stack slots.
#[test]
fn class_fields_read_the_bindings_of_the_enclosing_function() {
    let (mut engine, _) = engine();
    let source = r#"
        var out = [];
        function a() { let r = 1; return class { x = r; }; }
        out.push(new (a())().x);
        function b() { var r = 2; return class { x = r; }; }
        out.push(new (b())().x);
        function c(r) { return class { x = r; }; }
        out.push(new (c(3))().x);
        function d() { const r = 4; return class { static x = r; }; }
        out.push(d().x);
        function e() { class r { v = 5; } return class { x = new r; }; }
        out.push(new (e())().x.v);
        function f() { function r() { return 6; } return class { x = r(); }; }
        out.push(new (f())().x);
        function g() { for (const r of [7]) { return class { x = r; }; } }
        out.push(new (g())().x);
        function h() { let r = 8; return class { [r] = r + 1; }; }
        out.push(new (h())()[8]);
        function i() { let r = 10; class K { x = r; } r = 11; return new K().x; }
        out.push(i());
        function k() { try { throw 12; } catch (r) { return class { x = { y: [r] }; }; } }
        out.push(new (k())().x.y[0]);
        { let r = 13; var M = class { x = r; }; }
        out.push(new M().x);
        out.join(' ')
    "#;
    assert_eq!(
        engine.evaluate(source, false),
        Ok(Completion::String("1 2 3 4 5 6 7 9 11 12 13".into()))
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
    // A lone surrogate comes back as it went out, a pair joins, and a byte
    // that is no UTF-8 becomes U+FFFD.
    *recorder.reply.borrow_mut() = Some(HostReturn::StringBytes(
        b"a\xED\xA0\x80b\xED\xA0\xBD\xED\xB8\x80\xFFc".to_vec(),
    ));
    assert_eq!(
        engine.evaluate("__h() === 'a\\ud800b\\ud83d\\ude00\\ufffdc' ? 1 : 0", false),
        Ok(Completion::Number(1.0))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::StringBytes(b"\xED\xA0\x80".to_vec()));
    assert_eq!(
        engine.evaluate("__h() === '\\ud800' ? 1 : 0", false),
        Ok(Completion::Number(1.0))
    );
    assert_eq!(
        engine.evaluate("function same(a, b) { return a === b && a.length; }", false),
        Ok(Completion::Undefined)
    );
    assert_eq!(
        engine.call_with_bytes("same", &[b"x\xED\xB0\x80", b"x\xED\xB0\x80"]),
        Ok(Completion::Number(2.0))
    );
    assert_eq!(
        engine.evaluate("function echo(a) { return a; }", false),
        Ok(Completion::Undefined)
    );
    assert_eq!(
        engine.call_with_bytes("echo", &[b"\xED\xA0\x80"]),
        Ok(Completion::String(b"\xED\xA0\x80".to_vec()))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::RangeError("too long".into()));
    assert_eq!(
        engine.evaluate(
            "try { __h(); 'returned' } catch (e) { (e instanceof RangeError) + ' ' + e.message }",
            false
        ),
        Ok(Completion::String("true too long".into()))
    );
    assert_eq!(
        engine.evaluate("__h()", false),
        Err(Error::Exception("RangeError: too long".into()))
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
        engine.call_with_bytes("units", &[lone]),
        Ok(Completion::String("61 d800 62 dc00".into()))
    );
    assert_eq!(
        engine.call_with_bytes("same", &[lone]),
        Ok(Completion::String(lone.to_vec()))
    );
    // A lead and a trail encoded apart are the one code point they spell.
    assert_eq!(
        engine.call_with_bytes("same", &[b"\xED\xA0\xBD\xED\xB2\xA9"]),
        Ok(Completion::String("\u{1F4A9}".into()))
    );
    // Bytes that are not WTF-8 are replaced, as before.
    assert_eq!(
        engine.call_with_bytes("units", &[b"a\xFF\xC0\x80"]),
        Ok(Completion::String("61 fffd fffd fffd".into()))
    );
    assert_eq!(
        engine.call("units", &["\u{e9}\u{1F4A9}"]),
        Ok(Completion::String("e9 d83d dca9".into()))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::StringBytes(lone.to_vec()));
    assert_eq!(
        engine.evaluate("units(__h())", false),
        Ok(Completion::String("61 d800 62 dc00".into()))
    );
    *recorder.reply.borrow_mut() = Some(HostReturn::StringBytes(b"\xFFok".to_vec()));
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
