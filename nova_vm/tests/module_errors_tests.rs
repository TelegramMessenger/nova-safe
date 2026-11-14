// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Dynamic import of a module whose evaluation threw.

use std::{cell::RefCell, collections::VecDeque};

use nova_vm::{
    ecmascript::{
        AbstractModule, Agent, AgentOptions, ExceptionType, GcAgent, GraphLoadingStateRecord,
        HostDefined, HostHooks, Job, ModuleRequest, Object, Referrer, String,
        finish_loading_imported_module, parse_module, parse_script, script_evaluation,
    },
    engine::{Bindable, GcScope, Global, NoGcScope},
};

#[derive(Default)]
struct Hooks {
    jobs: RefCell<VecDeque<Job>>,
    modules: RefCell<Vec<(std::string::String, Global<AbstractModule<'static>>)>>,
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
    fn load_imported_module<'gc>(
        &self,
        agent: &mut Agent,
        referrer: Referrer<'gc>,
        module_request: ModuleRequest<'gc>,
        _host_defined: Option<HostDefined>,
        payload: &mut GraphLoadingStateRecord<'gc>,
        gc: NoGcScope<'gc, '_>,
    ) {
        let specifier = module_request
            .specifier(agent)
            .to_string_lossy(agent)
            .into_owned();
        let cached = self
            .modules
            .borrow()
            .iter()
            .find(|(name, _)| *name == specifier)
            .map(|(_, module)| module.get(agent, gc));
        if let Some(module) = cached {
            finish_loading_imported_module(
                agent,
                referrer,
                module_request,
                payload,
                Ok(module),
                gc,
            );
            return;
        }
        let source = match specifier.as_str() {
            "throws" => "throw new Error('boom');",
            _ => "export default 1;",
        };
        let source_text = String::from_str(agent, source, gc);
        let result = parse_module(agent, source_text, referrer.realm(agent, gc), None, gc)
            .map(|m| {
                let module: AbstractModule = m.into();
                self.modules
                    .borrow_mut()
                    .push((specifier, Global::new(agent, module.unbind())));
                module
            })
            .map_err(|_| {
                agent.throw_exception_with_static_message(
                    ExceptionType::SyntaxError,
                    "bad module",
                    gc,
                )
            });
        finish_loading_imported_module(agent, referrer, module_request, payload, result, gc);
    }
}

#[test]
fn importing_a_module_that_threw_twice_rejects_both_times() {
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
    let mut results = vec![];
    for source in [
        "var log = []; import('throws').catch((e) => log.push('first ' + e.message)); \
         import('throws').catch((e) => log.push('second ' + e.message));",
        "import('throws').catch((e) => log.push('third ' + e.message));",
        "log.join()",
    ] {
        let result = agent.run_in_realm(&realm, |agent, mut gc| {
            let realm = agent.current_realm(gc.nogc());
            let source_text = String::from_str(agent, source, gc.nogc());
            let script = parse_script(agent, source_text, realm, false, None, gc.nogc()).unwrap();
            let value = script_evaluation(agent, script.unbind(), gc.reborrow())
                .unbind()
                .expect("the script runs");
            let text = value
                .unbind()
                .string_repr(agent, gc.reborrow())
                .to_string_lossy(agent)
                .into_owned();
            loop {
                let job = hooks.jobs.borrow_mut().pop_front();
                let Some(job) = job else {
                    break;
                };
                job.run(agent, gc.reborrow())
                    .unbind()
                    .expect("the job runs");
            }
            text
        });
        results.push(result);
    }
    assert_eq!(results[2], "first boom,second boom,third boom");
}
