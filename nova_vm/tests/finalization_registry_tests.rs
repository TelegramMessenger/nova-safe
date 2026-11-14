// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![cfg(feature = "weak-refs")]

use std::{cell::RefCell, collections::VecDeque};

use nova_vm::{
    ecmascript::{
        AgentOptions, GcAgent, HostHooks, Job, RealmRoot, String, parse_script, script_evaluation,
    },
    engine::Bindable,
};

#[derive(Default)]
struct CleanupHooks {
    jobs: RefCell<VecDeque<Job>>,
}

impl core::fmt::Debug for CleanupHooks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CleanupHooks").finish_non_exhaustive()
    }
}

impl HostHooks for CleanupHooks {
    fn enqueue_generic_job(&self, _job: Job) {}

    fn enqueue_promise_job(&self, _job: Job) {}

    fn enqueue_timeout_job(&self, _job: Job, _milliseconds: u64) {}

    fn enqueue_finalization_registry_cleanup_job(&self, job: Job) {
        self.jobs.borrow_mut().push_back(job);
    }
}

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

/// Collects the garbage and runs the cleanup jobs that requests, and the jobs
/// those request, until none is left. Returns how many jobs ran.
fn collect_and_clean_up(agent: &mut GcAgent, hooks: &CleanupHooks) -> usize {
    agent.gc();
    let mut jobs = 0;
    loop {
        let job = hooks.jobs.borrow_mut().pop_front();
        let Some(job) = job else { break };
        agent.run_job(job, |_, _, _| ());
        jobs += 1;
    }
    jobs
}

const CHECK: &str = r#"
function check(name, actual, expected) {
  if (actual !== expected) throw new Error(name + ": expected " + expected + ", got " + actual);
}
"#;

fn new_agent() -> (GcAgent, RealmRoot, &'static CleanupHooks) {
    let hooks: &'static CleanupHooks = Box::leak(Box::default());
    let mut agent = GcAgent::new(AgentOptions::default(), hooks);
    let realm = agent.create_default_realm();
    run(&mut agent, &realm, CHECK);
    (agent, realm, hooks)
}

#[test]
fn every_held_value_is_cleaned_up() {
    for count in 1..=4 {
        let (mut agent, realm, hooks) = new_agent();
        run(
            &mut agent,
            &realm,
            &format!(
                r#"
var held = [];
var registry = new FinalizationRegistry((value) => held.push(value));
(function () {{
  for (var i = 0; i < {count}; i++) registry.register({{}}, "value " + i);
}})();
"#
            ),
        );
        assert_eq!(collect_and_clean_up(&mut agent, hooks), 1);
        run(
            &mut agent,
            &realm,
            &format!(
                r#"
check("cleaned up", held.length, {count});
check("each once", new Set(held).size, {count});
"#
            ),
        );
    }
}

#[test]
fn a_throwing_callback_is_not_called_again() {
    for throw_at in 1..=3 {
        let (mut agent, realm, hooks) = new_agent();
        run(
            &mut agent,
            &realm,
            &format!(
                r#"
var calls = [];
var registry = new FinalizationRegistry((value) => {{
  calls.push(value);
  if (calls.length === {throw_at}) throw new Error("cleanup " + value);
}});
(function () {{
  for (var i = 0; i < 3; i++) registry.register({{}}, "value " + i);
}})();
"#
            ),
        );
        collect_and_clean_up(&mut agent, hooks);
        run(
            &mut agent,
            &realm,
            r#"
check("every value", calls.length, 3);
check("each once", new Set(calls).size, 3);
"#,
        );
    }
}
