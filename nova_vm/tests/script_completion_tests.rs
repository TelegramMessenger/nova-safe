// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use nova_vm::{
    ecmascript::{
        AgentOptions, DefaultHostHooks, GcAgent, String, parse_script, script_evaluation,
    },
    engine::Bindable,
};

fn completion_of(source: &str) -> std::string::String {
    let mut agent = GcAgent::new(AgentOptions::default(), &DefaultHostHooks);
    let realm = agent.create_default_realm();
    agent.run_in_realm(&realm, |agent, mut gc| {
        let realm = agent.current_realm(gc.nogc());
        let source_text = String::from_str(agent, source, gc.nogc());
        let script = parse_script(agent, source_text, realm, false, None, gc.nogc()).unwrap();
        let value = script_evaluation(agent, script.unbind(), gc.reborrow())
            .unbind()
            .expect("the script runs");
        value
            .unbind()
            .to_string(agent, gc)
            .unwrap()
            .to_string_lossy(agent)
            .into_owned()
    })
}

#[test]
fn script_of_directives_completes_with_the_last_directive() {
    assert_eq!(completion_of("'abc'"), "abc");
    assert_eq!(completion_of("'use strict'"), "use strict");
    assert_eq!(completion_of("'abc'; 'def'"), "def");
    assert_eq!(completion_of("'a\\u0062c'"), "abc");
    assert_eq!(completion_of("'abc'; function f() {}"), "abc");
    assert_eq!(completion_of("'abc'; 1"), "1");
    assert_eq!(completion_of("1"), "1");
    assert_eq!(completion_of(""), "undefined");
}

#[test]
fn eval_code_of_directives_completes_with_the_last_directive() {
    assert_eq!(completion_of("eval(\"'abc'\")"), "abc");
    assert_eq!(completion_of("eval(\"'abc'; function f() {}\")"), "abc");
    assert_eq!(completion_of("eval(\"'x'; {}\")"), "x");
}
