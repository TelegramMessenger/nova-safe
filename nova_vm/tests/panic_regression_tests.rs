// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scripts that made the engine panic. Each runs its script and the job
//! queue, and checks the outcome QuickJS gives.

use std::{cell::RefCell, collections::VecDeque};

use nova_vm::{
    ecmascript::{
        Agent, AgentOptions, GcAgent, HostHooks, Job, Object, String, parse_script,
        script_evaluation,
    },
    engine::{Bindable, GcScope},
};

#[derive(Default)]
struct Hooks {
    jobs: RefCell<VecDeque<Job>>,
}

impl core::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Hooks")
    }
}

impl HostHooks for Hooks {
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

/// Runs `source` and then its jobs; returns the completion value of
/// `result_source` (a second script run afterwards), "terminated: …" or
/// "thrown: …".
fn run(source: &str, result_source: &str, heap_limit: Option<usize>) -> std::string::String {
    let hooks: &'static Hooks = Box::leak(Box::default());
    let mut agent = GcAgent::new(AgentOptions::default(), hooks);
    let create_global_object: Option<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>> = None;
    let create_global_this_value: Option<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>> =
        None;
    let realm = agent.create_realm(
        create_global_object,
        create_global_this_value,
        None::<fn(&mut Agent, Object, GcScope)>,
    );
    agent.set_heap_limit(heap_limit);
    let mut outcome = None;
    for (index, source) in [source, result_source].into_iter().enumerate() {
        let source = source.to_owned();
        let result = agent.run_in_realm(&realm, |agent, mut gc| {
            let realm = agent.current_realm(gc.nogc());
            let source_text = String::from_string(agent, source, gc.nogc());
            let script = parse_script(agent, source_text, realm, false, None, gc.nogc()).unwrap();
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
                return Some(format!("terminated: {reason:?}"));
            }
            Some(match result {
                Ok(value) if index == 1 => value
                    .string_repr(agent, gc.reborrow())
                    .to_string_lossy(agent)
                    .into_owned(),
                Ok(_) => return None,
                Err(err) => format!(
                    "thrown: {}",
                    err.value()
                        .string_repr(agent, gc.reborrow())
                        .to_string_lossy(agent)
                ),
            })
        });
        if result.is_some() {
            outcome = result;
            break;
        }
    }
    outcome.unwrap()
}

#[test]
fn loose_equality_with_a_boolean_propagates_the_object_conversion_error() {
    let source = r#"
        var log = [];
        try { true == { valueOf() { throw 1; } }; } catch (e) { log.push("caught " + e); }
        try { ({ valueOf() { throw 2; } }) == false; } catch (e) { log.push("caught " + e); }
        log.push(String(false == { valueOf() { return 0; } }), String({ toString() { return "1"; } } == true));
    "#;
    assert_eq!(
        run(source, "log.join()", None),
        "caught 1,caught 2,true,true"
    );
}

#[test]
fn async_function_returning_a_thenable_getter_or_proxy() {
    let source = r#"
        var log = [];
        (async function () { return { get then() { log.push("then read"); return undefined; } }; })()
            .then((v) => log.push("fulfilled " + typeof v));
        (async function () { return new Proxy({}, {}); })().then((v) => log.push("proxy " + typeof v));
        (async function () { return { get then() { throw new Error("bad then"); } }; })()
            .catch((e) => log.push("rejected " + e.message));
        log.push("sync");
    "#;
    assert_eq!(
        run(source, "log.join()", None),
        "then read,sync,fulfilled object,proxy object,rejected bad then"
    );
}

#[test]
fn async_generator_results_with_a_then_getter_on_object_prototype() {
    let source = r#"
        var log = [];
        Object.defineProperty(Object.prototype, "then", {
            configurable: true,
            get() { log.push("then read"); return undefined; },
        });
        (async function* () { yield 1; })().next().then((r) => log.push("next " + r.value));
    "#;
    let result = run(source, "delete Object.prototype.then; log.join()", None);
    assert!(result.contains("next 1"), "{result}");
    assert!(result.contains("then read"), "{result}");
}

#[test]
fn private_name_in_a_proxy_is_false() {
    let source = r#"
        class A { #x = 1; static t(o) { return #x in o; } }
        var result = [A.t(new Proxy({}, {})), A.t(new A())].join();
    "#;
    assert_eq!(run(source, "result", None), "false,true");
}

#[test]
fn typed_array_methods_survive_a_shrink_during_argument_conversion() {
    let source = r#"
        var types = [Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array,
            Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array];
        var calls = [
            (t, v) => t.copyWithin(v, 0),
            (t, v) => t.copyWithin(0, 1, v),
            (t, v) => t.copyWithin(0, v),
            (t, v) => t.slice(v),
            (t, v) => t.slice(0, v),
            (t, v) => t.with(v, 0),
            (t, v) => t.with(1, { valueOf() { v.valueOf(); return 0; } }),
        ];
        var results = [];
        for (var T of types) for (var call of calls) for (var shrinkTo of [0, 1, 3]) {
            var b = new ArrayBuffer(8 * T.BYTES_PER_ELEMENT, { maxByteLength: 64 * T.BYTES_PER_ELEMENT });
            var t = new T(b);
            var once = false;
            var v = { valueOf() { if (!once) { once = true; b.resize(shrinkTo * T.BYTES_PER_ELEMENT); } return 2; } };
            try { results.push(call(t, v).length); } catch (e) { results.push(e.constructor.name); }
        }
        var copied = new Int8Array(new ArrayBuffer(8, { maxByteLength: 8 }));
        copied.set([1, 2, 3, 4, 5, 6, 7, 8]);
        var slice = copied.slice({ valueOf() { copied.buffer.resize(5); return 2; } });
    "#;
    assert_eq!(run(source, "slice.join()", None), "3,4,5,0,0,0");
}

#[test]
fn typed_array_with_reads_undefined_past_a_shrunk_end() {
    let source = r#"
        function shrunk(T, value) {
            var b = new ArrayBuffer(3 * T.BYTES_PER_ELEMENT, { maxByteLength: 3 * T.BYTES_PER_ELEMENT });
            var t = new T(b);
            var index = { valueOf() { b.resize(T.BYTES_PER_ELEMENT); return 0; } };
            try { return Array.from(t.with(index, value)).join(); } catch (e) { return e.constructor.name; }
        }
        var result = [shrunk(Int8Array, 9), shrunk(Float64Array, 9), shrunk(BigInt64Array, 9n)].join(" ");
    "#;
    assert_eq!(run(source, "result", None), "9,0,0 9,NaN,NaN TypeError");
}

#[test]
fn json_stringify_replacer_key_conversion_throws() {
    let source = r#"
        var key = new String("a");
        key.toString = function () { throw new Error("no key"); };
        var result;
        try { JSON.stringify({ a: 1 }, [key]); result = "no error"; } catch (e) { result = "caught " + e.message; }
    "#;
    assert_eq!(run(source, "result", None), "caught no key");
}

#[test]
fn super_property_in_field_initializers_and_static_blocks() {
    let source = r#"
        class P { m() { return "P.m"; } static s() { return "P.s"; } }
        class A extends P {
            constructor() { super(); }
            f = () => super.m(); g = super.m(); static h = super.s();
            static { this.k = super.s(); }
        }
        var a = new A();
        var out = [a.f(), a.g, A.h, A.k];
        class B extends P { f = () => super.m(); static { this.k = super.s(); } }
        out.push(new B().f(), B.k);
        class C { f = () => super.toString === Object.prototype.toString; x = new.target; }
        out.push(new C().f(), String(new C().x));
    "#;
    assert_eq!(
        run(source, "out.join()", None),
        "P.m,P.m,P.s,P.s,P.m,P.s,true,undefined"
    );
}

#[test]
fn unsupported_syntax_throws_a_syntax_error() {
    let source = r#"
        var out = [];
        for (var code of ["class A { accessor x = 1; }", "class B { accessor #x = 1; }", "{ using y = null; }"]) {
            try { eval(code); out.push("ran"); } catch (e) { out.push(e.constructor.name); }
        }
    "#;
    assert_eq!(
        run(source, "out.join()", None),
        "SyntaxError,SyntaxError,SyntaxError"
    );
}
