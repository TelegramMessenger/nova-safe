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
fn interrupt_ends_regexp_split_match_and_replace_loops() {
    on_big_stack(|| {
        assert_interrupted(
            "var re = /x/g; re.exec = () => ({ index: 0, length: 2 ** 53 - 1, 0: '' }); \
             re.constructor = { [Symbol.species]: function () { return re; } }; 'ab'.split(re)",
        );
        assert_interrupted("var s = 'a'.repeat(1 << 24); for (;;) s.split(/(?:)/)");
        assert_interrupted("var s = 'a'.repeat(1 << 24); for (;;) s.match(/a/g)");
        assert_interrupted("var s = 'a'.repeat(1 << 24); for (;;) s.replace(/a/g, 'b')");
        assert_interrupted("var s = 'a'.repeat(1 << 24); for (;;) s.replace(/(?:)/g, '')");
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

#[test]
fn callable_proxies_can_be_called() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        assert_eq!(
            engine.run("new Proxy(function () { return 1; }, {})()", None),
            Outcome::Value("1".into())
        );
        assert_eq!(
            engine.run(
                "new Proxy(new Proxy(function () { return 2; }, {}), {})()",
                None
            ),
            Outcome::Value("2".into())
        );
        assert_eq!(
            engine.run(
                "var f = function () { return 7; }; for (let i = 0; i < 1e5; i++) f = new Proxy(f, {}); typeof f",
                None
            ),
            Outcome::Value("function".into())
        );
        assert_eq!(
            engine.run("f()", None),
            thrown("RangeError: Maximum call stack size exceeded")
        );
        engine.assert_usable();
    });
}

#[test]
fn rebinding_does_not_grow_names_without_bound() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        let started = Instant::now();
        assert_eq!(
            engine.run(
                "var b = function f() {}; for (let i = 0; i < 1e5; i++) b = b.bind(null); b.name.length < 4200 && b.name.startsWith('bound ') && b.name.endsWith(' f')",
                None
            ),
            Outcome::Value("true".into())
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            engine.run("function g() {} g.bind().bind().name", None),
            Outcome::Value("bound bound g".into())
        );
    });
}

#[test]
fn heap_limit_holds_with_a_large_live_heap() {
    on_big_stack(|| {
        // A large live heap spaces collections further apart (16 bytes of
        // allocation per live item); near the limit they must come sooner.
        // (Arrays stay under 65536 elements: a longer one takes a storage
        // slot of 2^24 elements.)
        let mut engine = Engine::new();
        let limit = 24 * 1024 * 1024;
        engine.agent.set_heap_limit(Some(limit));
        // The interrupt is only a backstop: unoptimised builds under a loaded
        // gate take tens of seconds to fill the heap.
        let outcome = engine.run(
            "const keep = []; let chunk = []; for (let i = 0; ; i++) { chunk.push({ i }); if (chunk.length === 1000) { keep.push(chunk); chunk = []; } }",
            Some(Duration::from_secs(600)),
        );
        assert_eq!(outcome, Outcome::Terminated(TerminationReason::OutOfMemory));
        let size = engine.agent.heap_size();
        // The allocation counter undercounts what the estimate finds live
        // (element storage, property storage), so the last collection can
        // find the heap past the limit by a part of the headroom it allowed.
        assert!(size <= limit + limit / 2, "{size}");
    });
}

#[test]
fn large_arrays_take_memory_proportional_to_their_length() {
    on_big_stack(|| {
        // A chart with 100k points under a small heap limit: elements past
        // 2^16 used to take a whole 2^24-slot storage (128 MiB).
        let mut engine = Engine::new();
        engine.agent.set_heap_limit(Some(8 * 1024 * 1024));
        assert_eq!(
            engine.run(
                "var points = []; for (let i = 0; i < 100000; i++) points.push(i); points.length",
                None
            ),
            Outcome::Value("100000".into())
        );
        engine.agent.gc();
        let size = engine.agent.heap_size();
        assert!(size > 100_000 * 8, "{size}");
        assert!(size < 4 * 1024 * 1024, "{size}");
        assert_eq!(
            engine.run(
                "var b = new Array(70000); b[69999] = 1; b.fill(2, 0, 10); var c = b.slice(); c.length + ':' + c[5] + ':' + c[69999] + ':' + (30000 in c) + ':' + points.indexOf(99999)",
                None
            ),
            Outcome::Value("70000:2:1:false:99999".into())
        );
        assert_eq!(
            engine.run(
                "points.length = 50; points.push(7); points.length + ':' + points[50] + ':' + points.concat(points).length",
                None
            ),
            Outcome::Value("51:7:102".into())
        );
        engine.agent.set_heap_limit(None);
        engine.assert_usable();
    });
}

#[test]
fn heap_estimate_counts_bytecode_environments_and_stacks() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        engine.agent.gc();
        let before = engine.agent.heap_size();
        assert_eq!(
            engine.run(
                "var big = Function('let x = 0;' + 'x += 1;'.repeat(100000) + 'return x;'); var keep = []; for (let i = 0; i < 20000; i++) { let v = i; keep.push(() => v); } function* g() { let a = [1, 2, 3]; yield a; } var gens = []; for (let i = 0; i < 20000; i++) { const it = g(); it.next(); gens.push(it); } typeof big",
                None
            ),
            Outcome::Value("function".into())
        );
        engine.agent.gc();
        let grown = engine.agent.heap_size() - before;
        // 100k statements of bytecode, 20k closure environments and 20k
        // suspended generator stacks.
        eprintln!("estimate grew by {grown} bytes");
        assert!(grown > 10 * 1024 * 1024, "{grown}");
    });
}

#[test]
fn large_dictionaries_take_memory_proportional_to_their_keys() {
    on_big_stack(|| {
        // Keys past 2^16 used to take a whole 2^24-key storage (128 MiB) plus
        // a 2^24-slot value storage.
        let mut engine = Engine::new();
        engine.agent.set_heap_limit(Some(8 * 1024 * 1024));
        assert_eq!(
            engine.run(
                "var dict = Object.fromEntries(Array.from({ length: 70000 }, (_, i) => ['k' + i, i])); dict.extra = 1; delete dict.k5; Object.keys(dict).length + ':' + dict.k69999 + ':' + ('k5' in dict) + ':' + dict.extra",
                None
            ),
            Outcome::Value("70000:69999:false:1".into())
        );
        engine.agent.gc();
        let size = engine.agent.heap_size();
        assert!(size > 70_000 * 16, "{size}");
        assert!(size < 6 * 1024 * 1024, "{size}");
        engine.agent.set_heap_limit(None);
        engine.assert_usable();
    });
}

#[test]
fn lists_sized_by_scripts_are_bounded() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        assert_eq!(
            engine.run(
                "Reflect.apply(function () {}, null, { length: 2 ** 53 - 1 })",
                None
            ),
            thrown("RangeError: Too many arguments in function call (only 65535 allowed)")
        );
        assert_eq!(
            engine.run(
                "Function.prototype.apply.call(Math.max, null, { length: 2 ** 32 })",
                None
            ),
            thrown("RangeError: Too many arguments in function call (only 65535 allowed)")
        );
        assert_eq!(
            engine.run(
                "Math.max.apply(null, Array.from({ length: 65535 }, (_, i) => i))",
                None
            ),
            Outcome::Value("65534".into())
        );
        assert_eq!(
            engine.run(
                "Reflect.ownKeys(new Proxy({}, { ownKeys() { return { length: 2 ** 53 - 1 }; } }))",
                None
            ),
            thrown(
                "TypeError: proxy [[OwnPropertyKeys]] must return an array with only string and symbol elements"
            )
        );
        let started = Instant::now();
        assert_eq!(
            engine.run(
                "var keys = []; for (let i = 0; i < 100000; i++) keys.push('k' + i); Reflect.ownKeys(new Proxy({}, { ownKeys() { return keys; } })).length",
                None
            ),
            Outcome::Value("100000".into())
        );
        assert!(started.elapsed() < Duration::from_secs(30));
        engine.assert_usable();
    });
}

#[test]
fn array_length_growth_respects_the_heap_limit() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        engine.agent.set_heap_limit(Some(64 * 1024 * 1024));
        let over = thrown("RangeError: Out of memory: allocation over the heap limit");
        assert_eq!(engine.run("[].length = 2 ** 32 - 1", None), over);
        assert_eq!(engine.run("var a = []; a[2 ** 32 - 2] = 1", None), over);
        assert_eq!(
            engine.run(
                "Object.defineProperty([], 'length', { value: 2 ** 32 - 1 })",
                None
            ),
            over
        );
        assert_eq!(engine.run("var b = []; b.length = 2 ** 26", None), over);
        assert_eq!(
            engine.run("var c = []; c.length = 1000; c.length", None),
            Outcome::Value("1000".into())
        );
        let outcome = engine.run(
            "try { 'a'.repeat(2 ** 23).split(''); 'ok' } catch (e) { String(e) }",
            Some(Duration::from_secs(60)),
        );
        assert!(
            matches!(
                outcome,
                Outcome::Value(_) | Outcome::Terminated(TerminationReason::OutOfMemory)
            ),
            "{outcome:?}"
        );
        engine.agent.set_heap_limit(None);
        engine.assert_usable();
    });
}

#[test]
fn infallible_allocations_near_the_heap_limit_do_not_panic() {
    on_big_stack(|| {
        let template = format!(
            "(function (strings) {{ return strings.length + ' ' + strings.raw.length; }})`{}`",
            "b${1}".repeat(60000)
        );
        let rest = "var big = new Array(200000).fill(0); var [first, ...rest] = big; rest.length";
        for limit in [100_000, 400_000, 1_000_000, 2_500_000] {
            for source in [template.as_str(), rest] {
                let mut engine = Engine::new();
                engine.agent.set_heap_limit(Some(limit));
                let outcome = engine.run(source, None);
                assert!(
                    matches!(
                        &outcome,
                        Outcome::Value(_)
                            | Outcome::Thrown(_)
                            | Outcome::Terminated(TerminationReason::OutOfMemory)
                    ),
                    "{limit}: {outcome:?}"
                );
                engine.agent.set_heap_limit(None);
                engine.assert_usable();
            }
        }
    });
}

#[test]
fn interrupt_ends_proxy_prototype_cycles_and_huge_array_likes() {
    let cycle = "var t = {}; var p = new Proxy(t, {}); Object.setPrototypeOf(t, p);";
    assert_interrupted(&format!("{cycle} p instanceof Function"));
    assert_interrupted(&format!("{cycle} ({{}}).isPrototypeOf(p)"));
    assert_interrupted(&format!("{cycle} for (var k in p) {{}}"));
    assert_interrupted("Array.prototype.copyWithin.call({ length: 2 ** 53 - 1 }, 0, 1)");
    assert_interrupted("Array.prototype.flat.call({ length: 2 ** 53 - 1 }, Infinity)");
    assert_interrupted("Array.prototype.flatMap.call({ length: 2 ** 53 - 1 }, x => x)");
    assert_interrupted("Array.prototype.lastIndexOf.call({ length: 2 ** 53 - 1 }, 1)");
    assert_interrupted("Array.prototype.reduceRight.call({ length: 2 ** 53 - 1 }, (a, b) => a, 0)");
    assert_interrupted(
        "JSON.stringify({}, new Proxy([], { get: (t, k) => k === 'length' ? 2 ** 53 - 1 : undefined }))",
    );
}

#[test]
fn string_raw_with_a_huge_literal_count_is_bounded() {
    assert_interrupted("String.raw({ raw: { length: 2 ** 53 - 1 } })");
    let mut engine = Engine::new();
    assert_eq!(
        engine.run("String.raw({ raw: ['a', 'b', 'c'] }, 1, 2)", None),
        Outcome::Value("a1b2c".into())
    );
}

#[test]
fn defining_far_array_indices_respects_the_heap_limit() {
    on_big_stack(|| {
        let mut engine = Engine::new();
        engine.agent.set_heap_limit(Some(64 * 1024 * 1024));
        let over = thrown("RangeError: Out of memory: allocation over the heap limit");
        for source in [
            "Object.defineProperty([], '100000000', { value: 1 })",
            "Object.defineProperties([], { 100000000: { value: 1 } })",
            "Reflect.defineProperty([], '100000000', { value: 1 })",
            "Object.assign([], { 100000000: 1 })",
            "[].concat({ [Symbol.isConcatSpreadable]: true, length: 100000001, 100000000: 1 })",
        ] {
            assert_eq!(engine.run(source, None), over, "{source}");
        }
        assert_eq!(
            engine.run(
                "var a = []; a[1000] = 1; Object.defineProperty(a, '2000', { value: 2 }); a.length",
                None
            ),
            Outcome::Value("2001".into())
        );
        engine.agent.set_heap_limit(None);
        engine.assert_usable();
    });
}

#[test]
fn replace_with_script_sized_results_is_bounded() {
    on_big_stack(|| {
        assert_interrupted(
            "var r = /a/; r.exec = function () { return { index: 0, length: 2 ** 53 - 1, 0: 'a' }; }; 'a'.replace(r, 'x')",
        );
        assert_interrupted(
            "var r = /a/g; var n = 0; r.exec = function () { return n++ ? null : { index: 0, length: 2 ** 31, 0: 'a' }; }; 'a'.replace(r, () => 'x')",
        );
        let mut engine = Engine::new();
        engine.agent.set_heap_limit(Some(64 * 1024 * 1024));
        assert_eq!(
            engine.run(
                "function puff(x, n) { while (x.length < n) x += x; return x.substring(0, n); } var x = puff('1', 1 << 20); var rep = puff('$1', 1 << 16); x.replace(/(.+)/g, rep)",
                Some(Duration::from_secs(60)),
            ),
            thrown("RangeError: Out of memory: allocation over the heap limit")
        );
        assert_eq!(
            engine.run("'a-b-a'.replaceAll('a', '$&$&')", None),
            Outcome::Value("aa-b-aa".into())
        );
        engine.agent.set_heap_limit(None);
        engine.assert_usable();
    });
}

#[test]
fn native_loops_over_proxies_hold_bounded_roots() {
    on_big_stack(|| {
        // Endless loops through trapless Proxies: their per-element roots are
        // dropped, so they stay under the limit until the interrupt.
        for source in [
            "var t = {}; var p = new Proxy(t, {}); Object.setPrototypeOf(t, p); p instanceof Function",
            "Array.prototype.indexOf.call(new Proxy({ length: 2 ** 53 - 1 }, {}), 1)",
            "Array.prototype.every.call(new Proxy({ length: 2 ** 53 - 1 }, {}), x => true)",
        ] {
            let mut engine = Engine::new();
            engine.agent.set_heap_limit(Some(32 * 1024 * 1024));
            let outcome = engine.run(source, Some(Duration::from_secs(2)));
            assert_eq!(
                outcome,
                Outcome::Terminated(TerminationReason::Interrupted),
                "{source}"
            );
            assert!(engine.agent.heap_size() < 32 * 1024 * 1024, "{source}");
            engine.agent.set_heap_limit(None);
            engine.assert_usable();
        }
        // A loop that really grows still ends at the limit.
        let mut engine = Engine::new();
        engine.agent.set_heap_limit(Some(32 * 1024 * 1024));
        assert_eq!(
            engine.run(
                "Array.from((function* () { for (;;) yield { a: 1 }; })())",
                Some(Duration::from_secs(300)),
            ),
            Outcome::Terminated(TerminationReason::OutOfMemory)
        );
        // Builtins writing through a Proxy over a big array finish under a
        // small limit (reactive proxies over chart data).
        let mut engine = Engine::new();
        engine.agent.set_heap_limit(Some(8 * 1024 * 1024));
        assert_eq!(
            engine.run(
                "var base = []; for (var i = 0; i < 100000; i++) base.push(i); var p = new Proxy(base, {}); Array.prototype.fill.call(p, 2); Array.prototype.reverse.call(p); Array.prototype.copyWithin.call(p, 0, 1); Array.prototype.splice.call(p, 0, 1); Array.prototype.shift.call(p); Array.prototype.unshift.call(p, 1); p.length",
                Some(Duration::from_secs(300)),
            ),
            Outcome::Value("99999".into())
        );
        engine.agent.set_heap_limit(None);
        engine.assert_usable();
    });
}
