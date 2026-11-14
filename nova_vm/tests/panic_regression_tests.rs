// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Scripts that made the engine panic. Each runs its script and the job
//! queue, and checks the outcome QuickJS gives.

use std::{cell::RefCell, collections::VecDeque};

use nova_vm::{
    ecmascript::{
        Agent, AgentOptions, GcAgent, HostHooks, Job, Object, String,
        parse_script, script_evaluation,
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
