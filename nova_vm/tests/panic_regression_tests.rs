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

#[test]
fn sloppy_block_function_declared_twice() {
    let source = r#"
        var out = [];
        { function f() { return 1; } out.push(f()); }
        { function f() { return 2; } function f() { return 3; } out.push(f()); }
        { out.push(g()); function g() { return "a"; } function g() { return "b"; } }
        (function () { { function k() { return 1; } function k() { return 2; } out.push(k()); } })();
    "#;
    assert_eq!(run(source, "out.join()", None), "1,3,b,2");
}

#[test]
fn regexp_last_index_past_the_safe_integers() {
    let source = r#"
        var r = /./g;
        var called = false;
        r.exec = function () {
            if (called) return null;
            called = true;
            r.lastIndex = 2 ** 54;
            return { length: 1, 0: "", index: 0 };
        };
        r[Symbol.replace]("", "");
    "#;
    assert_eq!(run(source, "r.lastIndex", None), "9007199254740992");
}

#[test]
fn number_to_locale_string_ignores_locales() {
    let source =
        r#"var result = [(1234.5).toLocaleString("de"), (12n).toLocaleString("de", {})].join();"#;
    assert_eq!(run(source, "result", None), "1234.5,12");
}

#[test]
fn scripts_with_more_constants_than_16_bit_indices() {
    let count = 70_000;
    let mut source = std::string::String::from("var a = [");
    for i in 0..count {
        source.push_str(&format!("\"s{i}\","));
    }
    source.push_str("];\nvar nums = [");
    for i in 0..count {
        source.push_str(&format!("{i}.5,"));
    }
    source.push_str("];\nvar table = [");
    for i in 0..count {
        source.push_str(&format!("{{n:\"name{i}\",v:{i}}},"));
    }
    source.push_str(
        "];\nvar after = { someNewProperty: 'late' };\n\
         function late() { return 'late literal ' + after.someNewProperty; }\n\
         var result = [a.length, a[65536], a[69999], nums[69999], table[69999].n, late()].join();",
    );
    assert_eq!(
        run(&source, "result", None),
        "70000,s65536,s69999,69999.5,name69999,late literal late"
    );
}

#[test]
fn code_with_more_names_than_16_bit_indices() {
    let source = r#"
        var log = [];
        var o = { p0: 1, p65536: 2, p69999: 3 };
        function list(count, item) {
            return Array.from({ length: count }, (_, i) => item(i)).join("");
        }
        log.push(eval("var n = 0;" + list(70000, (i) => "if (o.p" + i + ") n++;") +
            " o.p69998 = n; [n, o.p65536, o.p69998, o.p69999].join(' ');"));
        function tooMany(name, source) {
            try {
                eval("log.push('" + name + " started');" + source);
                log.push(name + " ran");
            } catch (e) {
                log.push(name + ": " + e.name);
            }
        }
        tooMany("template parts", "`" + "${o.p0}".repeat(70000) + "`;");
        tooMany("classes", "[" + list(70000, () => "class {},") + "];");
        tooMany("arrows", "[" + list(70000, () => "() => 0,") + "];");
        try {
            new Function("return [" + list(70000, () => "class {},") + "];");
            log.push("Function ran");
        } catch (e) {
            log.push("Function: " + e.name);
        }
    "#;
    assert_eq!(
        run(source, "log.join()", None),
        "3 2 3 3,template parts: SyntaxError,classes: SyntaxError,arrows: SyntaxError,\
         Function: SyntaxError"
    );
}

#[test]
fn sloppy_eval_after_a_class_static_element() {
    let source = r#"
        var log = [];
        class A { static f = 1; static { var inBlock = 1; } }
        log.push(eval("var afterClass = 2; afterClass"));
        try { class C { static { throw 1; } } } catch (e) {}
        log.push(eval("var afterThrow = 3; afterThrow"));
        try { class D { static f = (() => { throw 2; })(); } } catch (e) {}
        log.push(eval("var afterThrow2 = 4; afterThrow2"));
        function g() { class F { static x = 1; } return eval("var inG = 5; inG"); }
        log.push(g(), typeof inG, typeof inBlock, typeof afterClass + typeof afterThrow2);
    "#;
    assert_eq!(
        run(source, "log.join()", None),
        "2,3,4,5,undefined,undefined,numbernumber"
    );
}

/// An Object Shape that shares the keys storage of a descendant with more
/// keys (after `setPrototypeOf` and deletes) copies its keys into a smaller
/// storage when a different key is added, instead of reaching
/// `unreachable!()` in the keys storage's grow.
#[test]
fn adding_a_key_to_a_shape_that_shares_a_larger_keys_storage() {
    let source = r#"
        var log = [];
        var p = { a: 1, b: 2, c: 3 };
        Object.setPrototypeOf(p, null);
        delete p.c;
        delete p.b;
        p.n = 4;
        log.push(Object.keys(p).join(""));
        var q = { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6, g: 7, h: 8, i: 9 };
        Object.setPrototypeOf(q, null);
        for (var k of ["i", "h", "g", "f", "e", "d", "c", "b"]) delete q[k];
        q.x = 1;
        q.y = 2;
        log.push(Object.keys(q).join(""), q.a + q.x + q.y);
        var r = { a: 1, b: 2, c: 3, d: 4, e: 5 };
        Object.setPrototypeOf(r, null);
        delete r.e; delete r.d; delete r.c; delete r.b;
        var child = Object.create(r);
        r.z = 1;
        log.push(Object.keys(r).join(""), child.a + child.z);
    "#;
    assert_eq!(run(source, "log.join()", None), "an,axy,4,az,2");
}

/// `%TypedArray%.prototype.filter` whose species constructor returns a
/// typed array of another element type converts the kept elements as `Set`
/// does, instead of copying raw bytes (a slice index panic when the result's
/// elements are smaller).
#[test]
fn typed_array_filter_species_of_another_element_type() {
    let source = r#"
        var log = [];
        function species(ta, C) {
            ta.constructor = { [Symbol.species]: function (n) { return new C(n); } };
            return ta;
        }
        function show(r) { log.push(r.constructor.name + " " + Array.from(r).join(" ")); }
        show(species(new Uint16Array([1, 2, 3, 300, 5, 6, 7, 8, 9]), Uint8Array).filter(() => true));
        show(species(new Int8Array([-1, 2, -128]), Float64Array).filter(() => true));
        show(species(new Uint16Array([65535, 1]), Int16Array).filter(() => true));
        show(species(new Float64Array([1.5, -2, 300]), Uint8ClampedArray).filter(() => true));
        show(species(new BigInt64Array([-1n, 5n]), BigUint64Array).filter(() => true));
        show(species(new Uint8Array([1, 2, 3, 4]), Uint16Array).filter((x) => x & 1));
        show(species(new Float32Array([0.5, 2]), Float32Array).filter(() => true));
    "#;
    assert_eq!(
        run(source, "log.join()", None),
        "Uint8Array 1 2 3 44 5 6 7 8 9,Float64Array -1 2 -128,Int16Array -1 1,\
         Uint8ClampedArray 2 0 255,BigUint64Array 18446744073709551615 5,Uint16Array 1 3,\
         Float32Array 0.5 2"
    );
}

/// `Set.prototype.clear` keeps the list of entries (as EMPTY), so an
/// iterator suspended in it goes on to the values added after the clear, as
/// the spec, QuickJS and Map do. Truncating the list ended the iteration and
/// tripped a debug assertion in the iterator's `next`.
#[test]
fn set_iterator_visits_values_added_after_clear() {
    let source = r#"
        var log = [];
        var s = new Set([1, 2, 3, 4]);
        var it = s.values();
        it.next(); it.next(); it.next();
        s.clear();
        s.add(5); s.add(6);
        log.push(JSON.stringify([...it]), s.size);
        var e = new Set(["a", "b"]);
        var entries = e.entries();
        entries.next();
        e.clear();
        e.add("c");
        log.push(JSON.stringify([...entries]));
        var f = new Set([1, 2, 3]);
        var seen = [];
        f.forEach(function (v) { seen.push(v); if (v === 2) { f.clear(); f.add(7); } });
        log.push(seen.join(" "), [...f].join(" "));
        var g = new Set([1, 2]);
        for (var i = 0; i < 3; i++) { g.clear(); g.add(i); }
        log.push([...g].join(" "), g.has(2), g.has(0), g.size);
    "#;
    assert_eq!(
        run(source, "log.join()", None),
        "[5,6],2,[[\"c\",\"c\"]],1 2 7,7,2,true,false,1"
    );
}

/// `codePointAt` on the second code unit of a surrogate pair that starts a
/// heap string (longer than an inline string) read the pair's start from an
/// index mapping that stores index 0 as `None`, and unwrapped it.
#[test]
fn code_point_at_inside_a_leading_surrogate_pair() {
    let source = r#"
        var log = [];
        for (var s of ["\u{1F600}aaba", "\u{1F600}", "\u{1F600}\u{1F600}xyzw", "a\u{1F600}bcdef"]) {
            var units = [];
            for (var i = 0; i < s.length; i++) units.push(s.codePointAt(i), s.charCodeAt(i));
            log.push(units.join(" "));
        }
    "#;
    assert_eq!(
        run(source, "log.join()", None),
        "128512 55357 56832 56832 97 97 97 97 98 98 97 97,128512 55357 56832 56832,\
         128512 55357 56832 56832 128512 55357 56832 56832 120 120 121 121 122 122 119 119,\
         97 97 128512 55357 56832 56832 98 98 99 99 100 100 101 101 102 102"
    );
}
