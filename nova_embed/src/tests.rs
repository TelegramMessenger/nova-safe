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
    let before = engine.memory_used();
    assert!(before > 0);
    engine
        .evaluate(
            "globalThis.big = Array.from({ length: 20000 }, (_, i) => ({ i, s: 'v' + i }));",
            false,
        )
        .unwrap();
    let grown = engine.memory_used();
    assert!(grown > before + (1 << 20), "{before} -> {grown}");
    engine.evaluate("globalThis.big = null", false).unwrap();
    engine.collect_garbage().unwrap();
    assert!(engine.memory_used() < grown / 2);
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
