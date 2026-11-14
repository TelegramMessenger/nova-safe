// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Strict functions may run without a Function Environment Record of their
//! own when nothing observes it. These tests check that this, new.target,
//! super and arguments still reach the right record from every syntactic
//! position an expression can take in a function: patterns, rest elements,
//! parameters and their defaults, computed keys, spread, templates and
//! nested arrows.

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

/// Runs a script whose last statement sets `result`.
fn result_of(source: &str) -> std::string::String {
    run(source, "result", None)
}

#[test]
fn arrow_in_a_computed_key_of_an_assignment_pattern_sees_the_callers_this() {
    let source = r#"
        "use strict";
        var k = "GLOBAL";
        function f(o) { let x; ({ [(() => this.k)()]: x } = o); return x; }
        var result = String(f.call({ k: "a" }, { a: "right", GLOBAL: "wrong" }));
    "#;
    assert_eq!(result_of(source), "right");
}

#[test]
fn arrow_in_an_object_rest_target_sees_the_callers_this() {
    let source = r#"
        "use strict";
        var k = "GLOBAL";
        function g(o) { const t = {}; ({ ...t[(() => this.k)()] } = o); return Object.keys(t).join(); }
        var result = g.call({ k: "r" }, { a: 1 });
    "#;
    assert_eq!(result_of(source), "r");
}

#[test]
fn arrow_in_a_for_of_assignment_pattern_sees_the_callers_this() {
    let source = r#"
        "use strict";
        function h(arr) { let x; for ({ [(() => typeof this)()]: x } of arr) {} return x; }
        var result = String(h.call(5, [{ number: "right", object: "wrong", undefined: "wrong" }]));
    "#;
    assert_eq!(result_of(source), "right");
}

#[test]
fn arrow_in_an_array_binding_rest_sees_the_callers_this() {
    let source = r#"
        "use strict";
        var k = "GLOBAL";
        function f() { const [...[x = (() => this.k)()]] = []; return x; }
        var result = String(f.call({ k: "right" }));
    "#;
    assert_eq!(result_of(source), "right");
}

#[test]
fn this_in_an_arrows_rest_parameter_default_sees_the_callers_this() {
    let source = r#"
        "use strict";
        var k = "GLOBAL";
        function f() { const g = (...[a = this.k]) => a; return g(); }
        var result = String(f.call({ k: "right" }));
    "#;
    assert_eq!(result_of(source), "right");
}

#[test]
fn new_target_in_an_arrow_in_a_computed_key_does_not_panic() {
    let source = r#"
        "use strict";
        function f(o) { let x; ({ [(() => new.target)() ? "a" : "b"]: x } = o); return x; }
        var result = String(f({ a: 1, b: 2 })) + String(new f({ a: 1, b: 2 }) instanceof f);
    "#;
    assert_eq!(result_of(source), "2true");
}

#[test]
fn new_target_in_an_arrows_rest_parameter_default_does_not_panic() {
    let source = r#"
        "use strict";
        function f() { const g = (...[a = new.target]) => a; return String(g()); }
        var result = f();
    "#;
    assert_eq!(result_of(source), "undefined");
}

#[test]
fn new_target_in_an_arrow_is_not_taken_from_an_outer_function() {
    let source = r#"
        "use strict";
        function Outer() {
            function inner(o) { let x; ({ [String((() => new.target)())]: x } = o); return x; }
            this.r = inner({ undefined: "right" });
        }
        var result = String(new Outer().r);
    "#;
    assert_eq!(result_of(source), "right");
}

#[test]
fn super_in_an_arrow_in_a_computed_key_of_an_object_method() {
    let source = r#"
        "use strict";
        const o = { __proto__: { k: "a" }, m(src) { let x; ({ [(() => super.k)()]: x } = src); return x; } };
        var result = String(o.m({ a: "right" }));
    "#;
    assert_eq!(result_of(source), "right");
}

#[test]
fn super_in_an_arrow_in_a_computed_key_of_a_class_method() {
    let source = r#"
        "use strict";
        class A { get k() { return "a"; } }
        class B extends A { m(src) { let x; ({ [(() => super.k)()]: x } = src); return x; } }
        var result = String(new B().m({ a: "right" }));
    "#;
    assert_eq!(result_of(source), "right");
}

#[test]
fn super_in_an_arrow_is_not_taken_from_an_outer_method() {
    let source = r#"
        "use strict";
        class A { get k() { return "a-from-A-with-this-" + this.tag; } }
        class B extends A {
            outer() {
                super.k;
                const inner = { __proto__: { get k() { return "a"; } }, tag: "inner",
                    m(src) { let x; ({ [(() => super.k)()]: x } = src); return x === undefined ? "missing key" : x; } };
                return inner.m({ a: "right", "a-from-A-with-this-outer-B": "wrong" });
            }
        }
        const b = new B(); b.tag = "outer-B";
        var result = String(b.outer());
    "#;
    assert_eq!(result_of(source), "right");
}

#[test]
fn arguments_in_a_computed_key_of_an_assignment_pattern() {
    let source = r#"
        "use strict";
        function f(o) { let x; ({ [arguments[1]]: x } = o); return x; }
        function g() { const h = (...[a = arguments[0]]) => a; return h(); }
        var result = String(f({ a: "right" }, "a")) + " " + String(g("right"));
    "#;
    assert_eq!(result_of(source), "right right");
}

#[test]
fn class_static_block_in_a_function_without_an_environment_takes_its_own_this() {
    let source = r#"
        "use strict";
        function run() { class B { static { let y = 5; this.s = y; } } return B.s; }
        var result = String(run());
    "#;
    assert_eq!(result_of(source), "5");
}

/// Statements of a function body; `$` (or `@` inside a template literal) is
/// where the expression under test goes.
const BODIES: &[&str] = &[
    r#"let x; ({ [$]: x } = o);"#,
    r#"const t = {}; ({ ...t[$] } = o);"#,
    r#"let x; [...[x = $]] = [];"#,
    r#"const [...[y = $]] = [];"#,
    r#"const { [$]: z } = o;"#,
    r#"const { q = $ } = o;"#,
    r#"const [w = $] = [];"#,
    r#"let x; ({ a: x = $ } = o);"#,
    r#"let x; [x = $] = [];"#,
    r#"let x; for ({ [$]: x } of [o]);"#,
    r#"let x; for ([x = $] of [[]]);"#,
    r#"for (const { [$]: x } of [o]);"#,
    r#"let x; ({ a: { [$]: x } } = { a: o });"#,
    r#"const { a: [, ...{ [$]: r }] } = { a: [] };"#,
    r#"`${@}`;"#,
    r#"String.raw`${@}`;"#,
    r#"`${`${@}`}`;"#,
    r#"[...[$]];"#,
    r#"({ ...{ v: $ } });"#,
    r#"Math.max(...[$]);"#,
    r#"({ [$]: 1 });"#,
    r#"({ get [$]() { return 1; } });"#,
    r#"(class { [$]() {} });"#,
    r#"(class { [$] = 1; });"#,
    r#"(class { static [$] = 1; });"#,
    r#"(class extends ($, Object) {});"#,
    r#"((a = $) => a)();"#,
    r#"((...[a = $]) => a)();"#,
    r#"(({ [$]: a }) => a)(o);"#,
    r#"((a, ...{ [$]: b }) => b)(1);"#,
    r#"(async () => $)();"#,
    r#"try { throw o; } catch ({ [$]: e }) {}"#,
    r#"try { throw []; } catch ([e = $]) {}"#,
    r#"$?.toString();"#,
    r#"o?.[$];"#,
    r#"o[$];"#,
    r#"switch ($) { case $: break; }"#,
    r#"if ($) {}"#,
    r#"let i = 0; while (i++ < 1 && $) {}"#,
    r#"do {} while (!$);"#,
    r#"for (let i = $ ? 0 : 1; i < 0; i++);"#,
    r#"for (const k in { [$]: 1 });"#,
    r#"for (const k of [$]);"#,
    r#"(0, $);"#,
    r#"o.p ??= $;"#,
    r#"o.p ||= $; o.p &&= $;"#,
    r#"new Object($);"#,
    r#"typeof $;"#,
    r#"void $;"#,
    r#"{ let b = $; (() => b)(); }"#,
    r#"function inner() {} inner($);"#,
    r#"label: { $; }"#,
    r#"const f = () => { return $; }; f();"#,
    r#"const g = () => () => $; g()();"#,
    r#"let x; [{ [$]: x }] = [o];"#,
    r#"let x; [...{ [$]: x }] = [];"#,
    r#"const { [$]: { [$]: n } = {} } = {};"#,
];

/// Parameter lists of the function; `$` is where the expression goes.
const PARAMETERS: &[&str] = &[
    r#"o, ...[r = $]"#,
    r#"o, { [$]: r } = {}"#,
    r#"o, p = $"#,
    r#"o, [p = $] = []"#,
];

const THIS_EXPRESSIONS: &[&str] = &["(() => this)()", "(() => (() => this)())()", "this"];
const NEW_TARGET_EXPRESSIONS: &[&str] = &["(() => new.target)()", "new.target"];
const SUPER_EXPRESSIONS: &[&str] = &[
    "(() => super.k)()",
    "(() => (() => super.k)())()",
    "super.k",
];

fn place(template: &str, expression: &str) -> std::string::String {
    let recorded = format!("rec({expression})");
    if template.contains('@') {
        template.replace('@', &recorded)
    } else {
        template.replace('$', &recorded)
    }
}

fn check(expected: &str) -> std::string::String {
    format!(
        "seen.length > 0 && seen.every(v => v === {expected}) ? \"ok\" : \"bad \" + seen.length + \" \" + seen.map(v => typeof v === \"function\" ? \"fn:\" + v.name : String(v)).join()"
    )
}

/// A function with the expression in one position; the script records every
/// value the expression has and checks it against the right one.
fn program(kind: &str, expression: &str, parameters: &str, body: &str) -> std::string::String {
    let parameters = place(parameters, expression);
    let body = place(body, expression);
    let prelude =
        "\"use strict\";\nvar seen = [];\nfunction rec(v) { seen.push(v); return \"k\"; }\n";
    match kind {
        "this" => format!(
            "{prelude}function F({parameters}) {{ {body} }}\nconst T = {{ k: 1 }};\nF.call(T, {{}});\nvar result = {};",
            check("T")
        ),
        "new.target" => format!(
            "{prelude}function F({parameters}) {{ {body} }}\nnew F({{}});\nconst a = {};\nseen = [];\nF({{}});\nconst b = {};\nvar result = a + \"/\" + b;",
            check("F"),
            check("undefined")
        ),
        _ => format!(
            "{prelude}const P = {{ k: \"P-k\" }};\nconst O = {{ __proto__: P, m({parameters}) {{ {body} }} }};\nO.m({{}});\nconst a = {};\nseen = [];\nclass C extends class {{ get k() {{ return \"C-k\"; }} }} {{ m({parameters}) {{ {body} }} }}\nnew C().m({{}});\nconst b = {};\nvar result = a + \"/\" + b;",
            check("\"P-k\""),
            check("\"C-k\"")
        ),
    }
}

#[test]
fn this_new_target_and_super_from_every_position() {
    let mut failures = Vec::new();
    let mut count = 0;
    for (kind, expressions, expected) in [
        ("this", THIS_EXPRESSIONS, "ok"),
        ("new.target", NEW_TARGET_EXPRESSIONS, "ok/ok"),
        ("super", SUPER_EXPRESSIONS, "ok/ok"),
    ] {
        for expression in expressions {
            let cases = BODIES
                .iter()
                .map(|body| ("o", *body))
                .chain(PARAMETERS.iter().map(|parameters| (*parameters, "")));
            for (parameters, body) in cases {
                let source = program(kind, expression, parameters, body);
                let result = result_of(&source);
                count += 1;
                if result != expected {
                    failures.push(format!("{result}\n{source}"));
                }
            }
        }
    }
    assert_eq!(count, 488);
    assert!(
        failures.is_empty(),
        "{} of {count} failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
