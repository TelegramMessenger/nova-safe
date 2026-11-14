//! Hostile scripts against the resource limits: each must end with the right
//! error within a bounded time and leave the Agent usable.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    time::{Duration, Instant},
};

use nova_vm::{
    ecmascript::{
        Agent, AgentOptions, GcAgent, HostHooks, Job, Object, RealmRoot, String, TerminationReason,
        parse_script, script_evaluation,
    },
    engine::{Bindable, GcScope},
};

/// The tests run on threads with this much stack: unoptimised builds use far
/// more native stack per JavaScript call than release builds (a debug frame of
/// the interpreter's instruction dispatch alone is tens of kilobytes).
const THREAD_STACK: usize = 64 * 1024 * 1024;
const STACK_LIMIT: usize = 16 * 1024 * 1024;

fn on_big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(THREAD_STACK)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

#[derive(Default)]
struct Hooks {
    interrupt_at: Cell<Option<Instant>>,
    jobs: RefCell<VecDeque<Job>>,
}

impl core::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Hooks")
    }
}

impl HostHooks for Hooks {
    fn check_interrupt(&self) -> bool {
        self.interrupt_at
            .get()
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
    fn enqueue_generic_job(&self, job: Job) {
        self.jobs.borrow_mut().push_back(job);
    }
    fn enqueue_promise_job(&self, job: Job) {
        self.jobs.borrow_mut().push_back(job);
    }
    fn enqueue_timeout_job(&self, job: Job, _: u64) {
        self.jobs.borrow_mut().push_back(job);
    }
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Value(std::string::String),
    Thrown(std::string::String),
    Terminated(TerminationReason),
    ParseError,
}

struct Engine {
    hooks: &'static Hooks,
    agent: GcAgent,
    realm: RealmRoot,
}

impl Engine {
    fn new() -> Self {
        let hooks: &'static Hooks = Box::leak(Box::default());
        let mut agent = GcAgent::new(AgentOptions::default(), hooks);
        let create_global_object: Option<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>> =
            None;
        let create_global_this_value: Option<
            for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>,
        > = None;
        let realm = agent.create_realm(
            create_global_object,
            create_global_this_value,
            None::<fn(&mut Agent, Object, GcScope)>,
        );
        agent.set_stack_limit(STACK_LIMIT);
        Self {
            hooks,
            agent,
            realm,
        }
    }

    /// Runs `source` and then the job queue, interrupting after `interrupt`.
    fn run(&mut self, source: &str, interrupt: Option<Duration>) -> Outcome {
        self.hooks
            .interrupt_at
            .set(interrupt.map(|after| Instant::now() + after));
        let source = source.to_owned();
        let hooks = self.hooks;
        let outcome = self.agent.run_in_realm(&self.realm, |agent, mut gc| {
            let realm = agent.current_realm(gc.nogc());
            let source_text = String::from_string(agent, source, gc.nogc());
            let Ok(script) = parse_script(agent, source_text, realm, false, None, gc.nogc()) else {
                return Outcome::ParseError;
            };
            let mut result = script_evaluation(agent, script.unbind(), gc.reborrow())
                .map(|v| v.unbind())
                .map_err(|e| e.unbind());
            while result.is_ok() {
                let Some(job) = hooks.jobs.borrow_mut().pop_front() else {
                    break;
                };
                if let Err(err) = job.run(agent, gc.reborrow()) {
                    result = Err(err.unbind());
                }
            }
            if let Some(reason) = agent.take_termination() {
                return Outcome::Terminated(reason);
            }
            match result {
                Ok(value) => Outcome::Value(
                    value
                        .string_repr(agent, gc.reborrow())
                        .to_string_lossy(agent)
                        .into_owned(),
                ),
                Err(err) => Outcome::Thrown(
                    err.value()
                        .string_repr(agent, gc.reborrow())
                        .to_string_lossy(agent)
                        .into_owned(),
                ),
            }
        });
        self.hooks.jobs.borrow_mut().clear();
        self.hooks.interrupt_at.set(None);
        outcome
    }

    fn assert_usable(&mut self) {
        assert_eq!(self.run("1 + 1", None), Outcome::Value("2".into()));
    }
}

fn thrown(message: &str) -> Outcome {
    Outcome::Thrown(message.into())
}

#[track_caller]
fn assert_interrupted(source: &str) {
    let mut engine = Engine::new();
    let started = Instant::now();
    let outcome = engine.run(source, Some(Duration::from_millis(50)));
    let elapsed = started.elapsed();
    assert_eq!(
        outcome,
        Outcome::Terminated(TerminationReason::Interrupted),
        "{source}"
    );
    assert!(elapsed < Duration::from_secs(2), "{source}: {elapsed:?}");
    engine.assert_usable();
}

#[test]
fn interrupt_ends_loops_that_catch() {
    on_big_stack(|| {
        assert_interrupted("while (true) {}");
        assert_interrupted("while (true) { try {} catch {} }");
        assert_interrupted("for (;;) { try { for (;;) {} } catch (e) {} finally { for (;;) {} } }");
        assert_interrupted(
            "function f() { try { for (;;) {} } finally { f(); } } for (;;) { try { f(); } catch {} }",
        );
        assert_interrupted("let i = 0; for (;;) { try { throw i++; } catch (e) { continue; } }");
    });
}

#[test]
fn interrupt_ends_promise_and_async_loops() {
    on_big_stack(|| {
        assert_interrupted(
            "function loop() { Promise.resolve().then(loop).catch(() => {}); } loop();",
        );
        assert_interrupted(
            "(async () => { for (;;) { try { await null; } catch {} } })().catch(() => {});",
        );
        assert_interrupted("(async () => { for (;;) {} })().catch(() => { for (;;) {} }); 'after'");
        assert_interrupted("new Promise(() => { for (;;) {} }).catch(() => {}); 'after'");
        assert_interrupted(
            "function* g() { try { for (;;) yield 1; } finally { for (;;) {} } } for (const _ of g()) {}",
        );
    });
}

#[test]
fn interrupt_ends_long_native_loops() {
    on_big_stack(|| {
        assert_interrupted("Array.prototype.join.call({ length: 2 ** 32 - 1 }).length");
        assert_interrupted("Array.prototype.indexOf.call({ length: 2 ** 53 - 1 }, 1)");
        assert_interrupted("Array.prototype.includes.call({ length: 2 ** 53 - 1 }, 1)");
        assert_interrupted("Array.prototype.fill.call({ length: 2 ** 53 - 1 }, 1)");
    });
}

#[test]
fn deep_recursion_throws_a_catchable_range_error() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        let stack = "RangeError: Maximum call stack size exceeded";
        assert_eq!(engine.run("function f() { f(); } f()", None), thrown(stack));
        assert_eq!(
            engine.run(
                "var d = 0; function f() { d++; f(); } try { f(); } catch (e) { e instanceof RangeError && d > 0 }",
                None
            ),
            Outcome::Value("true".into())
        );
        let depth = engine.run("d", None);
        eprintln!("JS call depth within the stack limit: {depth:?}");
        assert_eq!(
            engine.run(
                "function f(n) { try { return f(n + 1); } catch (e) { return n; } } f(0) > 0",
                None
            ),
            Outcome::Value("true".into())
        );
        assert_eq!(
            engine.run(
                "var p = {}; for (let i = 0; i < 1e5; i++) p = new Proxy(p, {}); p.x",
                None
            ),
            thrown(stack)
        );
        assert_eq!(
            engine.run(
                "var p = {}; for (let i = 0; i < 1e5; i++) p = new Proxy(p, {}); 'x' in p",
                None
            ),
            thrown(stack)
        );
        assert_eq!(
            engine.run(
                "var x = { toString() { return String(y); } }; var y = { toString() { return String(x); } }; String(x)",
                None
            ),
            thrown(stack)
        );
        assert_eq!(
            engine.run("function* g() { yield* g(); } g().next()", None),
            thrown(stack)
        );
        engine.assert_usable();
    });
}

#[test]
fn deep_data_in_native_recursion_throws() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        let stack = "RangeError: Maximum call stack size exceeded";
        let nested = "var a = []; for (let i = 0; i < 1e5; i++) a = [a]; ";
        assert_eq!(
            engine.run(&format!("{nested} JSON.stringify(a)"), None),
            thrown(stack)
        );
        assert_eq!(
            engine.run(&format!("{nested} String(a)"), None),
            thrown(stack)
        );
        assert_eq!(
            engine.run(&format!("{nested} a.flat(Infinity)"), None),
            thrown(stack)
        );
        assert_eq!(
            engine.run(
                "var o = {}; for (let i = 0; i < 1e5; i++) o = { o }; JSON.stringify(o)",
                None
            ),
            thrown(stack)
        );
        assert_eq!(
            engine.run("JSON.parse('['.repeat(1e6) + ']'.repeat(1e6))", None),
            thrown("RangeError: JSON text is nested deeper than 5000 levels")
        );
        assert_eq!(
            engine.run(
                "JSON.parse('['.repeat(1000) + ']'.repeat(1000)).length",
                None
            ),
            Outcome::Value("1".into())
        );
        engine.assert_usable();
    });
}

#[test]
fn deeply_nested_source_is_a_syntax_error() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        for (open, close) in [
            ("(", ")"),
            ("[", "]"),
            ("{", "}"),
            ("!", ""),
            ("-", ""),
            ("a=", ""),
            ("0?0:", ""),
            ("x=>", ""),
            ("if(0)", ""),
            ("typeof ", ""),
            ("new ", ""),
        ] {
            let source = format!("{}1{}", open.repeat(100_000), close.repeat(100_000));
            assert_eq!(engine.run(&source, None), Outcome::ParseError, "{open}");
            let eval = format!(
                "try {{ eval('{}1{}'); 'no' }} catch (e) {{ e instanceof SyntaxError }}",
                open.repeat(100_000),
                close.repeat(100_000)
            );
            assert_eq!(
                engine.run(&eval, None),
                Outcome::Value("true".into()),
                "{open}"
            );
        }
        let chain = format!("1{}", "+1".repeat(100_000));
        assert_eq!(engine.run(&chain, None), Outcome::ParseError);
        let member = format!("({{}}){}", ".a".repeat(100_000));
        assert_eq!(engine.run(&member, None), Outcome::ParseError);
        let else_if = format!("if (0) {{}}{}", " else if (0) {}".repeat(100_000));
        assert_eq!(engine.run(&else_if, None), Outcome::ParseError);
        let ok = format!("{}1{}", "(".repeat(500), ")".repeat(500));
        assert_eq!(engine.run(&ok, None), Outcome::Value("1".into()));
        engine.assert_usable();
    });
}

#[test]
fn huge_allocations_are_refused() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        assert_eq!(
            engine.run("new ArrayBuffer(2 ** 31 + 1)", None),
            thrown("RangeError: Allocation too large")
        );
        assert_eq!(
            engine.run("'x'.repeat(2 ** 30)", None),
            thrown("RangeError: Invalid string length")
        );
        assert_eq!(
            engine.run(
                "var s = 'x'.repeat(2 ** 27); try { s.concat(s, s, s, s, s, s, s); } catch (e) { String(e) }",
                None
            ),
            Outcome::Value("RangeError: Invalid string length".into())
        );
        assert_eq!(
            engine.run("'x'.padStart(2 ** 31)", None),
            thrown("RangeError: Invalid string length")
        );
        engine.agent.set_heap_limit(Some(64 * 1024 * 1024));
        assert_eq!(
            engine.run("new ArrayBuffer(2 ** 27)", None),
            thrown("RangeError: Out of memory: allocation over the heap limit")
        );
        assert_eq!(
            engine.run("'x'.repeat(2 ** 27)", None),
            thrown("RangeError: Out of memory: allocation over the heap limit")
        );
        engine.assert_usable();
    });
}

#[test]
fn heap_limit_terminates_growing_scripts() {
    on_big_stack(|| {
        for source in [
            "const a = []; for (;;) a.push({ x: 1, y: [1, 2, 3] });",
            "const a = []; for (;;) a.push(new ArrayBuffer(1 << 16));",
            "const a = []; for (let i = 0; ; i++) a.push('s' + i);",
            "const a = []; for (;;) { try { a.push(new Array(100).fill(0)); } catch {} }",
            "const m = new Map(); for (let i = 0; ; i++) m.set(i, i);",
            "const t = new Set(); for (let i = 0; ; i++) t.add(i);",
            "let s = 'x'.repeat(1 << 20); for (;;) s = s + s;",
        ] {
            let mut engine = Engine::new();
            engine.agent.set_heap_limit(Some(8 * 1024 * 1024));
            let started = Instant::now();
            let outcome = engine.run(source, Some(Duration::from_secs(60)));
            assert!(
                outcome == Outcome::Terminated(TerminationReason::OutOfMemory)
                    || outcome
                        == thrown("RangeError: Out of memory: allocation over the heap limit"),
                "{source}: {outcome:?}"
            );
            eprintln!("{source}: {outcome:?} after {:?}", started.elapsed());
            assert!(engine.agent.heap_size() > 0);
            engine.agent.set_heap_limit(None);
            engine.assert_usable();
        }
    });
}

#[test]
fn many_live_globals_are_cheap() {
    on_big_stack(|| {
        use nova_vm::{
            ecmascript::{OrdinaryObject, Value},
            engine::Global,
        };
        let mut engine = Engine::new();
        let started = Instant::now();
        engine.agent.run_in_realm(&engine.realm, |agent, _| {
            let mut globals = Vec::new();
            for _ in 0..100_000u32 {
                let object: Value = OrdinaryObject::create_object(agent, None, &[])
                    .expect("no GC needed")
                    .into();
                globals.push(Global::new(agent, object.unbind()));
            }
            let mut kept = Vec::new();
            for (i, global) in globals.into_iter().enumerate() {
                if i % 2 == 0 {
                    let _ = global.take(agent);
                } else {
                    kept.push(global);
                }
            }
            for _ in 0..100_000u32 {
                let object: Value = OrdinaryObject::create_object(agent, None, &[])
                    .expect("no GC needed")
                    .into();
                kept.push(Global::new(agent, object.unbind()));
            }
            for global in kept {
                assert!(matches!(global.take(agent), Value::Object(_)));
            }
        });
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
        engine.assert_usable();
    });
}

#[test]
fn math_random_is_per_agent() {
    on_big_stack(|| {
        let source = "var r = []; for (let i = 0; i < 4; i++) { const x = Math.random(); if (!(x >= 0 && x < 1)) throw x; r.push(x); } r.join()";
        let mut first = Engine::new();
        let mut second = Engine::new();
        let a = first.run(source, None);
        let b = second.run(source, None);
        let c = first.run(source, None);
        assert!(matches!(a, Outcome::Value(_)), "{a:?}");
        assert_ne!(a, b, "two Agents on one thread share a Math.random stream");
        assert_ne!(a, c);
    });
}
