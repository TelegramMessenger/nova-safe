// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Function bodies are compiled without tracking completion values, which
//! only Script and eval code can observe. These tests run every statement
//! shape below as Script code, as eval code at the top level and inside
//! functions, and as the body of every kind of function, and check that the
//! completion values seen by Script and eval code and the side effects seen
//! everywhere stay what the specification says (the expectations were
//! recorded with QuickJS). A concise arrow body returns the value of its
//! expression, so expressions are also run as concise arrow bodies.

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

/// (statement, completion value, whether it is a primitive, `x|o` after it)
const SHAPES: &[(&str, &str, bool, &str)] = &[
    ("x = 1;", "1", true, "1|{}"),
    ("x = 1; x++;", "1", true, "2|{}"),
    ("x = 1; ++x;", "2", true, "2|{}"),
    ("x = 5; x--;", "5", true, "4|{}"),
    ("x = 2; x += 3;", "5", true, "5|{}"),
    ("o.p = 4;", "4", true, "undefined|{\"p\":4}"),
    ("o.p = 1; o.p++;", "1", true, "undefined|{\"p\":2}"),
    ("o.p = 1; o.p *= 3;", "3", true, "undefined|{\"p\":3}"),
    ("o[\"q\"] = 2; o.q -= 1;", "1", true, "undefined|{\"q\":1}"),
    ("(x = 7, o.q = 8);", "8", true, "7|{\"q\":8}"),
    ("(x = 7, o.q = 8, 9);", "9", true, "7|{\"q\":8}"),
    ("x = 1; if (x) {}", "undefined", true, "1|{}"),
    ("x = 1; if (!x) 2;", "undefined", true, "1|{}"),
    ("x = 1; if (x) 2;", "2", true, "1|{}"),
    ("if (0) 1; else 3;", "3", true, "undefined|{}"),
    ("3; if (1) {}", "undefined", true, "undefined|{}"),
    ("3; if (0) {} else {}", "undefined", true, "undefined|{}"),
    (
        "x = 0; if (x === 0) { x = 5; } else { x = 6; }",
        "5",
        true,
        "5|{}",
    ),
    (
        "x = 0; if (x !== 0 || o.p) x = 5; else x = 6;",
        "6",
        true,
        "6|{}",
    ),
    (
        "x = 0; if (!(x && o)) { o.r = 1; }",
        "1",
        true,
        "0|{\"r\":1}",
    ),
    ("4; for (x = 0; x < 2; x++) {}", "undefined", true, "2|{}"),
    ("5; for (x = 0; x < 2; x++) x;", "1", true, "2|{}"),
    (
        "6; for (x = 0; x < 2; x++) { if (x) break; 9; }",
        "undefined",
        true,
        "1|{}",
    ),
    (
        "6; for (x = 0; x < 3; x++) { if (x == 1) continue; o.c = x; }",
        "2",
        true,
        "3|{\"c\":2}",
    ),
    ("x = 0; while (x < 3) x++;", "2", true, "3|{}"),
    (
        "x = 0; while (x < 3 && !o.stop) { x++; if (x == 2) o.stop = true; }",
        "true",
        true,
        "2|{\"stop\":true}",
    ),
    ("x = 0; do x++; while (x < 3);", "2", true, "3|{}"),
    ("x = 0; do { x += 2; } while (!(x > 5));", "6", true, "6|{}"),
    ("7; x = 0; while (x < 2) { x++; }", "1", true, "2|{}"),
    ("7; for (;;) { x = 4; break; }", "4", true, "4|{}"),
    ("8; do { x = 3; } while (false);", "3", true, "3|{}"),
    ("try { x = 1; } finally { 2; }", "1", true, "1|{}"),
    ("try { throw 1; } catch (e) { x = e; }", "1", true, "1|{}"),
    (
        "try { x = 1; o.t = 2; } catch (e) {}",
        "2",
        true,
        "1|{\"t\":2}",
    ),
    ("x = 1; switch (x) { case 1: 2; }", "2", true, "1|{}"),
    (
        "x = 1; switch (x) { case 1: x = 3; break; default: 4; }",
        "3",
        true,
        "3|{}",
    ),
    ("x = 0; for (var k of a) x = k;", "2", true, "2|{}"),
    ("x = 0; for (var k in a) x += 1;", "2", true, "2|{}"),
    ("x = a.length ? 1 : 2;", "1", true, "1|{}"),
    ("x = !a ? 1 : 2;", "2", true, "2|{}"),
    ("x = !a && 1;", "false", true, "false|{}"),
    ("x = a && 1;", "1", true, "1|{}"),
    ("x = 0 || 5;", "5", true, "5|{}"),
    ("void (x = 2);", "undefined", true, "2|{}"),
    ("x = 1; x = x === 1 ? \"y\" : \"n\";", "y", true, "y|{}"),
    ("x = 1; { x++; }", "1", true, "2|{}"),
    (
        "x = function () {}; x.name;",
        "x",
        true,
        "function () {}|{}",
    ),
    (
        "x = function () {};",
        "function () {}",
        false,
        "function () {}|{}",
    ),
    ("x = 0; x ||= 2;", "2", true, "2|{}"),
    ("x = 1; x &&= 3;", "3", true, "3|{}"),
    ("x = null; x ??= 4;", "4", true, "4|{}"),
    ("[x] = a;", "1,2", false, "1|{}"),
    ("[x] = a; x;", "1", true, "1|{}"),
    ("({ p: x } = { p: 9 });", "[object Object]", false, "9|{}"),
    (
        "x = 0; o.n = { valueOf() { x++; return 1; } }; o.n++;",
        "1",
        true,
        "1|{\"n\":2}",
    ),
    (
        "x = 0; o.n = { valueOf() { x++; return 1; } }; o.n += 1;",
        "2",
        true,
        "1|{\"n\":2}",
    ),
    (
        "x = 0; o.n = { valueOf() { x++; return 1; } }; if (!(o.n == 1)) x = 9;",
        "undefined",
        true,
        "1|{\"n\":{}}",
    ),
    ("var y1 = 3;", "undefined", true, "undefined|{}"),
    ("var y1; y1 = 4;", "4", true, "undefined|{}"),
    ("let z1 = 1; z1 = 2;", "2", true, "undefined|{}"),
    ("x = 1; function q1() {}", "1", true, "1|{}"),
    ("x = 1; ;", "1", true, "1|{}"),
    ("x = 1; var y2;", "1", true, "1|{}"),
    ("x = 1; x;", "1", true, "1|{}"),
    ("x = 1; if (x) { var y3 = 2; }", "undefined", true, "1|{}"),
    ("x = 1; while (x) { x--; }", "1", true, "0|{}"),
    (
        "x = 2; while (x) { if (x--) continue; }",
        "undefined",
        true,
        "0|{}",
    ),
    (
        "x = 0; for (;;) { x++; if (x > 2) break; }",
        "undefined",
        true,
        "3|{}",
    ),
    (
        "x = 0; for (var k of a) { try { if (k == 2) break; x += k; } finally { o.f = k; } }",
        "undefined",
        true,
        "1|{\"f\":2}",
    ),
    (
        "x = 0; for (var k of a) { try { continue; } finally { x += k; } }",
        "undefined",
        true,
        "3|{}",
    ),
    (
        "x = 0; outer: for (var k of a) { for (;;) { x += k; if (k) continue outer; } }",
        "undefined",
        true,
        "3|{}",
    ),
    (
        "x = 0; try { x = 1; } catch (e) {} finally { o.f = 1; }",
        "1",
        true,
        "1|{\"f\":1}",
    ),
    (
        "x = 0; try { throw 2; } catch (e) { x = e; } finally { o.f = 1; }",
        "2",
        true,
        "2|{\"f\":1}",
    ),
    (
        "x = 0; switch (1) { case 1: x = 2; break; }",
        "2",
        true,
        "2|{}",
    ),
    (
        "x = 0; for (var k of a) { if (k == 1) continue; x = k; }",
        "2",
        true,
        "2|{}",
    ),
    ("x = 0; for (var k of a) x += k;", "3", true, "3|{}"),
    ("x = 0; for (var k in o) x++;", "undefined", true, "0|{}"),
    (
        "x = 0; while (true) { if (x++ > 1) { o.b = x; break; } }",
        "3",
        true,
        "3|{\"b\":3}",
    ),
    (
        "x = 0; do { if (x++ < 2) continue; o.c = x; } while (x < 4);",
        "4",
        true,
        "4|{\"c\":4}",
    ),
    (
        "x = 0; for (var k of a) { for (var j of a) { if (j == 2) break; x += j * k; } }",
        "undefined",
        true,
        "3|{}",
    ),
    (
        "x = 0; try { for (var k of a) { if (k == 2) throw k; } } catch (e) { x = e; }",
        "2",
        true,
        "2|{}",
    ),
    (
        "x = 0; lp: while (x < 5) { x++; try { if (x == 3) break lp; } finally { o.z = x; } }",
        "undefined",
        true,
        "3|{\"z\":3}",
    ),
];

const VARS: &str = "var x, o = {}, a = [1, 2];";
const DIGEST: &str = "String(x) + \"|\" + JSON.stringify(o)";

fn quoted(statement: &str) -> std::string::String {
    format!("{statement:?}")
}

/// Each context runs `statement` and stores what it saw under its name.
fn contexts(statement: &str) -> Vec<(&'static str, std::string::String, std::string::String)> {
    let s = statement;
    let q = quoted(statement);
    vec![
        (
            "function",
            format!("(function () {{ {VARS} {s}\n; return {DIGEST}; }})()"),
            "effects".into(),
        ),
        (
            "arrow",
            format!("(() => {{ {VARS} {s}\n; return {DIGEST}; }})()"),
            "effects".into(),
        ),
        (
            "method",
            format!("new (class {{ m() {{ {VARS} {s}\n; return {DIGEST}; }} }})().m()"),
            "effects".into(),
        ),
        (
            "generator",
            format!("(function* () {{ {VARS} {s}\n; yield {DIGEST}; }})().next().value"),
            "effects".into(),
        ),
        (
            "static block",
            format!(
                "(() => {{ let r; class C {{ static {{ {VARS} {s}\n; r = {DIGEST}; }} }} return r; }})()"
            ),
            "effects".into(),
        ),
        (
            "new Function",
            format!(
                "new Function({})()",
                quoted(&format!("{VARS} {s}\n; return {DIGEST};"))
            ),
            "effects".into(),
        ),
        (
            "loop body",
            format!(
                "(function () {{ {VARS} for (var once = 0; once < 1; once++) {{ {s}\n }} return {DIGEST}; }})()"
            ),
            "effects".into(),
        ),
        (
            "catch block",
            format!(
                "(function () {{ {VARS} try {{ throw 0; }} catch (e) {{ {s}\n }} return {DIGEST}; }})()"
            ),
            "effects".into(),
        ),
        (
            "finally after return",
            format!("(function () {{ {VARS} try {{ return \"R\"; }} finally {{ {s}\n }} }})()"),
            "R".into(),
        ),
        (
            "falls off the end",
            format!("String((function () {{ {VARS} {s}\n }})())"),
            "undefined".into(),
        ),
        (
            "generator falls off the end",
            format!("String((function* () {{ {VARS} {s}\n }})().next().value)"),
            "undefined".into(),
        ),
        (
            "eval in a function",
            format!("String((function () {{ {VARS} return eval({q}); }})())"),
            "completion".into(),
        ),
        (
            "eval in an arrow",
            format!("String((function () {{ {VARS} return (() => eval({q}))(); }})())"),
            "completion".into(),
        ),
        (
            "eval in a generator",
            format!("String((function* () {{ {VARS} yield eval({q}); }})().next().value)"),
            "completion".into(),
        ),
        (
            "eval in a loop",
            format!(
                "String((function () {{ {VARS} var r; for (var once = 0; once < 1; once++) r = eval({q}); return r; }})())"
            ),
            "completion".into(),
        ),
        (
            "eval at the top level",
            format!("String(eval({q}))"),
            "completion".into(),
        ),
    ]
}

#[test]
fn completion_values_and_effects_are_unchanged_in_every_context() {
    let mut failures = vec![];
    for &(statement, completion, primitive, effects) in SHAPES {
        let mut source = format!("{VARS} var R = [];\n");
        let names: Vec<_> = contexts(statement)
            .into_iter()
            .map(|(name, code, expected)| {
                source.push_str(&format!(
                    "try {{ R.push(String({code})); }} catch (e) {{ R.push(\"thrown: \" + e); }}\n"
                ));
                let expected = match expected.as_str() {
                    "effects" => effects.to_owned(),
                    "completion" => completion.to_owned(),
                    other => other.to_owned(),
                };
                (name, expected)
            })
            .collect();
        let got = run(&source, "R.join(\"\\u0000\")", None);
        let got: Vec<_> = got.split('\u{0}').collect();
        for (index, (name, expected)) in names.iter().enumerate() {
            if got.get(index).copied() != Some(expected.as_str()) {
                failures.push(format!(
                    "{statement:?} in {name}: expected {expected:?}, got {:?}",
                    got.get(index)
                ));
            }
        }
        if primitive {
            let got = run(VARS, statement, None);
            if got != completion {
                failures.push(format!(
                    "{statement:?} as Script code: expected {completion:?}, got {got:?}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// (expression, `value|x|o` after it)
const EXPRESSIONS: &[(&str, &str)] = &[
    ("x = 1", "1|1|{}"),
    ("(x = 1, x++)", "1|2|{}"),
    ("(x = 1, ++x)", "2|2|{}"),
    ("x++", "NaN|NaN|{}"),
    ("o.p = 4", "4|undefined|{\"p\":4}"),
    ("(o.p = 1, o.p++)", "1|undefined|{\"p\":2}"),
    ("(o.p = 1, o.p *= 3)", "3|undefined|{\"p\":3}"),
    ("(x = 7, o.q = 8, 9)", "9|7|{\"q\":8}"),
    ("(x = 7, o.q = 8)", "8|7|{\"q\":8}"),
    ("x = a.length ? 1 : 2", "1|1|{}"),
    ("x = !a && 1", "false|false|{}"),
    ("(x = 0, x ||= 2)", "2|2|{}"),
    ("(x = 1, x &&= 3)", "3|3|{}"),
    ("(x = null, x ??= 4)", "4|4|{}"),
    ("[x] = a", "1,2|1|{}"),
    ("({ p: x } = { p: 9 })", "[object Object]|9|{}"),
    ("void (x = 2)", "undefined|2|{}"),
    ("(x = 5, x--)", "5|4|{}"),
    ("(x = 2, x += 3)", "5|5|{}"),
    ("(x = function () {}, x.name)", "x|function () {}|{}"),
    ("o[\"q\"] = (x = 3)", "3|3|{\"q\":3}"),
    ("(0, 1, x = 6)", "6|6|{}"),
    ("(x = 1, 2)", "2|1|{}"),
    ("!(x = 0) ? (o.t = 1) : (o.t = 2)", "1|0|{\"t\":1}"),
    ("(x = 1) && (o.u = 2)", "2|1|{\"u\":2}"),
];

#[test]
fn concise_arrow_bodies_return_the_value_of_their_expression() {
    let mut failures = vec![];
    let after = format!("String(v) + \"|\" + {DIGEST}");
    for &(expression, expected) in EXPRESSIONS {
        let e = expression;
        let q = quoted(expression);
        let contexts = [
            (
                "concise arrow",
                format!("(function () {{ {VARS} var v = (() => {e})(); return {after}; }})()"),
            ),
            (
                "nested concise arrow",
                format!(
                    "(function () {{ {VARS} var v = (() => () => {e})()(); return {after}; }})()"
                ),
            ),
            (
                "concise arrow in a field",
                format!(
                    "(function () {{ {VARS} var v = new (class {{ f = () => {e}; }})().f(); return {after}; }})()"
                ),
            ),
            (
                "concise arrow in eval",
                format!(
                    "(function () {{ {VARS} var v = eval({:?})(); return {after}; }})()",
                    format!("() => {e}")
                ),
            ),
            (
                "return",
                format!(
                    "(function () {{ {VARS} var v = (function () {{ return {e}; }})(); return {after}; }})()"
                ),
            ),
            (
                "eval in a function",
                format!("(function () {{ {VARS} var v = eval({q}); return {after}; }})()"),
            ),
        ];
        let mut source = "var R = [];\n".to_owned();
        for (_, code) in &contexts {
            source.push_str(&format!(
                "try {{ R.push(String({code})); }} catch (e) {{ R.push(\"thrown: \" + e); }}\n"
            ));
        }
        let got = run(&source, "R.join(\"\\u0000\")", None);
        let got: Vec<_> = got.split('\u{0}').collect();
        for (index, (name, _)) in contexts.iter().enumerate() {
            if got.get(index).copied() != Some(expected) {
                failures.push(format!(
                    "{expression:?} in {name}: expected {expected:?}, got {:?}",
                    got.get(index)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
