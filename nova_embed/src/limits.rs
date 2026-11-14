// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The engine's limits (nova_vm's `execution/limits.rs`): the interrupt poll
//! (`EmbedHooks::check_interrupt` in hooks.rs), the heap limit, the native
//! stack guard, and how a terminated script surfaces as an [`Error`].

use nova_vm::ecmascript::{Agent, GcAgent, TerminationReason};

use crate::{EngineOptions, Error};

/// [`Host::should_interrupt`](crate::Host::should_interrupt) is polled while scripts run.
pub const LIMIT_INTERRUPT: u32 = 1;
/// The memory limit ends a script that goes past it.
pub const LIMIT_HEAP: u32 = 2;
/// Deep recursion is a `RangeError`, not a native stack overflow.
pub const LIMIT_STACK: u32 = 4;

/// Which limits this build enforces.
pub const ENFORCED: u32 = LIMIT_INTERRUPT | LIMIT_HEAP | LIMIT_STACK;

/// The message of the RangeError with which nova_vm refuses one allocation
/// that would go past the heap limit (`Agent::check_allocation`).
const ALLOCATION_REFUSED: &str = "Out of memory: allocation over the heap limit";

pub(crate) fn apply(agent: &mut GcAgent, options: EngineOptions) {
    set_heap_limit(agent, options.memory_limit);
    if options.stack_limit > 0 {
        agent.set_stack_limit(options.stack_limit);
    }
}

pub(crate) fn set_heap_limit(agent: &mut GcAgent, bytes: usize) {
    agent.set_heap_limit(if bytes == 0 { None } else { Some(bytes) });
}

pub(crate) fn heap_size(agent: &GcAgent) -> usize {
    agent.heap_size()
}

/// The error of a script the engine terminated, clearing the termination so
/// that the next call starts clean.
pub(crate) fn take_termination(agent: &mut Agent) -> Option<Error> {
    match agent.take_termination()? {
        TerminationReason::Interrupted => Some(Error::Interrupted),
        TerminationReason::OutOfMemory => Some(Error::OutOfMemory),
    }
}

/// An uncaught refusal of an allocation over the heap limit ends the script
/// as the heap limit itself does.
pub(crate) fn is_allocation_refused(agent: &Agent, text: &str) -> bool {
    agent.heap_limit().is_some()
        && text
            .strip_prefix("RangeError: ")
            .is_some_and(|message| message.starts_with(ALLOCATION_REFUSED))
}
