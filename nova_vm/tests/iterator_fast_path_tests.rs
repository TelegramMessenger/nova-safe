// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nova_vm::{
    ecmascript::{
        AgentOptions, DefaultHostHooks, GcAgent, String, parse_script, script_evaluation,
    },
    engine::Bindable,
};

fn run(source: &str) {
    let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
    let realm = agent.create_default_realm();
    agent.run_in_realm(&realm, |agent, mut gc| {
        let realm = agent.current_realm(gc.nogc());
        let source_text = String::from_str(agent, source, gc.nogc());
        let script = parse_script(agent, source_text, realm, false, None, gc.nogc()).unwrap();
        if let Err(err) = script_evaluation(agent, script.unbind(), gc.reborrow()) {
            panic!(
                "{}",
                err.unbind()
                    .to_string(agent, gc)
                    .to_string_lossy(agent)
                    .into_owned()
            )
        }
    });
}

const CHECK: &str = r#"
function check(name, actual, expected) {
  if (actual !== expected) throw new Error(name + ": expected " + expected + ", got " + actual);
}
var AIP = Object.getPrototypeOf([][Symbol.iterator]());
var origNext = AIP.next;
"#;

#[test]
fn array_iterator_next_replaced_during_for_of() {
    run(&(CHECK.to_owned()
        + r#"
var seen = [];
var calls = 0;
for (const x of [10, 20, 30, 40].values()) {
  seen.push(x);
  if (x === 20) AIP.next = function () { calls++; return { done: true, value: 1 }; };
}
check("loop keeps the next it started with", seen.join(), "10,20,30,40");
check("replacement not called yet", calls, 0);
seen = [];
for (const x of [5, 6].values()) seen.push(x);
check("next loop uses the replacement", seen.join(), "");
check("replacement called once", calls, 1);
AIP.next = function () { calls++; return origNext.call(this); };
seen = [];
for (const x of [7, 8].entries()) seen.push(x.join(":"));
check("wrapping replacement", seen.join(), "0:7,1:8");
check("wrapping replacement calls", calls, 4);
AIP.next = origNext;
"#));
}

#[test]
fn array_iterator_fast_path_sees_mutation_getters_and_proxies() {
    run(&(CHECK.to_owned()
        + r#"
var seen = [];
var grow = [1, 2];
for (const x of grow.values()) { seen.push(x); if (grow.length < 5) grow.push(x * 10); }
check("grow", seen.join(), "1,2,10,20,100");
seen = [];
var shrink = [1, 2, 3, 4, 5];
for (const x of shrink.values()) { seen.push(x); shrink.length = 2; }
check("shrink", seen.join(), "1,2");
var holes = [1, , 3];
var getterCalls = 0;
Object.defineProperty(Array.prototype, 1, { get() { getterCalls++; return "g"; }, configurable: true });
var s = "";
for (const x of holes.values()) s += x + ",";
for (const [i, x] of holes.entries()) s += i + ":" + x + ",";
delete Array.prototype[1];
check("holes", s, "1,g,3,0:1,1:g,2:3,");
check("prototype getter calls", getterCalls, 2);
var traps = [];
var p = new Proxy([7, 8, 9], { get(t, k, r) { if (typeof k !== "symbol") traps.push(k); return Reflect.get(t, k, r); } });
seen = [];
for (const x of Array.prototype.values.call(p)) seen.push(x);
check("proxy values", seen.join(), "7,8,9");
check("proxy traps", traps.join(), "length,0,length,1,length,2,length");
var rab = new ArrayBuffer(4, { maxByteLength: 8 });
var ta = new Uint8Array(rab, 2, 2);
var threw = "";
seen = [];
try { for (const x of ta.values()) { seen.push(x); rab.resize(1); } } catch (e) { threw = e.constructor.name; }
check("typed array out of bounds", threw + seen.join(), "TypeError0");
threw = "";
try { for (const x of { [Symbol.iterator]() { return { next: origNext }; } }) {} } catch (e) { threw = e.constructor.name; }
check("next on a non Array Iterator", threw, "TypeError");
var own = [1, 2, 3].values();
var ownCalls = 0;
own.next = function () { ownCalls++; return { done: true }; };
for (const x of own) {}
check("own next property", ownCalls, 1);
var thrower = [1, 2, 3];
Object.defineProperty(thrower, 1, { get() { throw new Error("boom"); } });
var ti = thrower.values();
threw = "";
try { for (const x of ti) {} } catch (e) { threw = e.message; }
check("getter throws", threw, "boom");
check("iterator continues after a throw", ti.next().value, 3);
check("then finishes", ti.next().done, true);
var it = [1, 2, 3].values();
for (const x of it) { if (x === 2) break; }
check("break leaves the iterator resumable", it.next().value, 3);
var [d1, d2, ...rest] = [1, 2, 3, 4].values();
check("destructuring", [d1, d2, rest.join()].join(), "1,2,3,4");
function* gen() { yield* [1, 2].values(); }
check("yield*", [...gen()].join(), "1,2");
var reproto = [1, 2].values();
Object.setPrototypeOf(reproto, { next() { return { done: false, value: "p" }; }, [Symbol.iterator]() { return this; } });
seen = [];
for (const x of reproto) { seen.push(x); if (seen.length > 2) break; }
check("iterator with another prototype", seen.join(), "p,p,p");
"#));
}
