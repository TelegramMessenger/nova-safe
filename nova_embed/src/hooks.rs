// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The host hooks every engine shares, and how they find the engine that is
//! running.
//!
//! An agent needs `&'static dyn HostHooks`. Without `unsafe` the hooks can be
//! neither borrowed from the engine for `'static` nor leaked per engine
//! without leaking memory, so all engines share one zero-sized [`HOOKS`]. The
//! per-engine state ([`Shared`]) is pushed on a thread-local stack for the
//! length of every call into an engine ([`EntryGuard`]); a hook or host
//! function reads the top of the stack. Nothing stays in the thread-local
//! between calls, so an engine may move to another thread between calls, and
//! an engine called from inside another engine's host function is on top
//! while it runs.

use std::{cell::RefCell, rc::Rc};

use nova_vm::{
    ecmascript::{
        Agent, ExceptionType, GraphLoadingStateRecord, HostDefined, HostHooks, Job, ModuleRequest,
        Referrer, finish_loading_imported_module,
    },
    engine::NoGcScope,
};

use crate::Shared;

thread_local! {
    static CURRENT: RefCell<Vec<Rc<Shared>>> = const { RefCell::new(Vec::new()) };
}

/// The engine running on this thread, if any.
pub(crate) fn current() -> Option<Rc<Shared>> {
    CURRENT.with(|stack| stack.borrow().last().cloned())
}

/// Keeps an engine on top of the thread's stack until dropped (also while
/// unwinding from a panic).
pub(crate) struct EntryGuard(());

impl EntryGuard {
    pub(crate) fn enter(shared: Rc<Shared>) -> EntryGuard {
        CURRENT.with(|stack| stack.borrow_mut().push(shared));
        EntryGuard(())
    }
}

impl Drop for EntryGuard {
    fn drop(&mut self) {
        CURRENT.with(|stack| {
            let _ = stack.borrow_mut().pop();
        });
    }
}

#[derive(Debug)]
pub(crate) struct EmbedHooks;

pub(crate) static HOOKS: EmbedHooks = EmbedHooks;

impl HostHooks for EmbedHooks {
    /// Polled by the engine every few thousand instructions: asks the running
    /// engine's host.
    fn check_interrupt(&self) -> bool {
        current().is_some_and(|shared| shared.host.should_interrupt())
    }

    fn enqueue_generic_job(&self, _job: Job) {
        // Only Atomics.waitAsync makes these, and the widget build has no
        // Atomics.
    }

    fn enqueue_promise_job(&self, job: Job) {
        if let Some(shared) = current() {
            shared.jobs.borrow_mut().push_back(job);
        }
    }

    fn enqueue_timeout_job(&self, _timeout_job: Job, _milliseconds: u64) {
        // No timers: the embedder runs its own.
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
        let error = agent.throw_exception_with_static_message(
            ExceptionType::TypeError,
            "modules cannot be imported here",
            gc,
        );
        finish_loading_imported_module(agent, referrer, module_request, payload, Err(error), gc);
    }

    fn get_host_data(&self) -> &dyn core::any::Any {
        self
    }
}
