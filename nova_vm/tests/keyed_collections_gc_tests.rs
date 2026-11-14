// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nova_vm::{
    ecmascript::{
        AgentOptions, DefaultHostHooks, GcAgent, RealmRoot, String, parse_script, script_evaluation,
    },
    engine::Bindable,
};

fn run(agent: &mut GcAgent, realm: &RealmRoot, source: &str) {
    agent.run_in_realm(realm, |agent, mut gc| {
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

const SETUP: &str = r#"
var garbage = Array.from({ length: 3000 }, (_, i) => ({ g: i }));
var objs = Array.from({ length: 64 }, (_, i) => ({ i }));
var set = new Set([...objs, "a string long enough to be on the heap " + objs.length, 1.5, 2n ** 70n]);
var map = new Map(objs.map((o, i) => [o, i]));
map.set("another string long enough to be on the heap", -1);
map.set(2n ** 80n, -2);
garbage = null;
"#;

const CHECK: &str = r#"
function check(name, actual, expected) {
  if (actual !== expected) throw new Error(name + ": expected " + expected + ", got " + actual);
}
for (let i = 0; i < objs.length; i++) {
  check("set has " + i, set.has(objs[i]), true);
  check("map get " + i, map.get(objs[i]), i);
}
check("set has a heap string", set.has("a string long enough to be on the heap " + objs.length), true);
check("set has a BigInt", set.has(2n ** 70n), true);
check("map gets a heap string", map.get("another string long enough to be on the heap"), -1);
check("map gets a BigInt", map.get(2n ** 80n), -2);
objs.forEach((o) => set.add(o));
objs.forEach((o, i) => map.set(o, i));
check("set size", set.size, 67);
check("map size", map.size, 66);
check("set delete", set.delete(objs[3]) && !set.has(objs[3]), true);
set.add(objs[3]);
check("map delete", map.delete(objs[5]) && !map.has(objs[5]), true);
map.set(objs[5], 5);
"#;

#[test]
fn map_and_set_lookups_survive_compaction() {
    let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
    let realm = agent.create_default_realm();
    run(&mut agent, &realm, SETUP);
    agent.gc();
    run(&mut agent, &realm, CHECK);
    run(
        &mut agent,
        &realm,
        "var more = Array.from({ length: 500 }, (_, i) => ({ m: i })); more = null; set.delete(\"a string long enough to be on the heap \" + objs.length); set.add(\"a string long enough to be on the heap \" + objs.length);",
    );
    agent.gc();
    run(&mut agent, &realm, CHECK);
}
