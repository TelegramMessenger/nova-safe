// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where the engine's limits are wired: the one file that changes when the
//! limits track (`safe-b-bindable`, PLAN.md round 4) lands its API.
//!
//! At that merge:
//! - `EmbedHooks::poll_interrupt` (hooks.rs) becomes the body of
//!   `HostHooks::check_interrupt`.
//! - [`set_heap_limit`] calls `Agent::set_heap_limit`, [`heap_size`] returns
//!   `Agent::heap_size()`.
//! - [`apply`] sets the native-stack guard from `stack_limit`.
//! - [`termination`] maps the uncatchable termination of a script to
//!   `Error::Interrupted` / `Error::OutOfMemory`.

use nova_vm::ecmascript::{GcAgent, RealmRoot};

use crate::{EngineOptions, Error, Shared};

/// [`Host::should_interrupt`](crate::Host::should_interrupt) is polled while scripts run.
pub const LIMIT_INTERRUPT: u32 = 1;
/// The memory limit ends a script that goes past it.
pub const LIMIT_HEAP: u32 = 2;
/// Deep recursion is a `RangeError`, not a native stack overflow.
pub const LIMIT_STACK: u32 = 4;

/// Which limits this build enforces; every bit is set once the limits track
/// is merged and wired here.
pub const ENFORCED: u32 = 0;

pub(crate) fn apply(agent: &mut GcAgent, _realm: &RealmRoot, options: EngineOptions) {
    set_heap_limit(agent, options.memory_limit);
}

pub(crate) fn set_heap_limit(_agent: &mut GcAgent, _bytes: usize) {}

pub(crate) fn heap_size(_agent: &mut GcAgent) -> usize {
    0
}

/// The error a script ended with when the engine ended it rather than the
/// script throwing.
pub(crate) fn termination(shared: &Shared) -> Option<Error> {
    if shared.interrupted.get() {
        Some(Error::Interrupted)
    } else if shared.out_of_memory.get() {
        Some(Error::OutOfMemory)
    } else {
        None
    }
}
