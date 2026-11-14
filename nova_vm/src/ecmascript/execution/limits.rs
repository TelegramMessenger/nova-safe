// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Resource limits of an Agent: interrupts, the heap limit and the native
//! stack guard.
//!
//! A script that is interrupted or goes over the heap limit is *terminated*:
//! the Agent remembers a [`TerminationReason`] and every exception handler
//! (`catch`, `finally`, promise reactions, async functions) lets the error pass
//! while it is set, so the error reaches the embedder. The embedder reads and
//! clears the reason with [`Agent::take_termination`]; entering the Agent again
//! from the host ([`GcAgent::run_in_realm`] or [`GcAgent::run_job`]) also
//! clears it, so the Agent stays usable after a termination.
//!
//! [`GcAgent::run_in_realm`]: crate::ecmascript::GcAgent::run_in_realm
//! [`GcAgent::run_job`]: crate::ecmascript::GcAgent::run_job

use crate::{
    ecmascript::{Agent, ExceptionType, JsError, JsResult, String},
    engine::NoGcScope,
    engine::{HeapRootCollection, HeapRootData},
};

/// Why a script was terminated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationReason {
    /// [`HostHooks::check_interrupt`](crate::ecmascript::HostHooks::check_interrupt)
    /// returned true.
    Interrupted,
    /// The heap grew past the limit set with [`Agent::set_heap_limit`].
    OutOfMemory,
}

impl TerminationReason {
    /// The message of the error value that carries the termination.
    pub fn message(self) -> &'static str {
        match self {
            TerminationReason::Interrupted => "Script terminated: interrupted",
            TerminationReason::OutOfMemory => "Script terminated: out of memory",
        }
    }
}

/// Instructions (or native loop steps, or RegExp matcher steps) between two
/// interrupt polls.
pub(crate) const POLL_INTERVAL: u32 = 4096;

/// Allocations between two garbage collections when no heap limit is near.
const GC_ALLOCATION_INTERVAL: usize = 2 * 1024 * 1024;

/// Allocations between two garbage collections near the heap limit, so that
/// a heap close to its limit does not collect on every allocation.
const MIN_GC_ALLOCATION_INTERVAL: usize = 256 * 1024;

/// Native stack the JavaScript code of one host entry may use when the
/// embedder does not say.
///
/// Rust's spawned threads get 2 MiB by default and the main thread 8 MiB;
/// half of the smaller one leaves room for the embedder's frames and for
/// native code running between two checks.
pub const DEFAULT_STACK_LIMIT: usize = 1024 * 1024;

/// Largest length of a JavaScript string, in UTF-16 code units, the engine
/// creates (QuickJS uses the same bound).
pub const MAX_STRING_LENGTH: usize = (1 << 30) - 1;

/// Largest single buffer (ArrayBuffer bytes, element storage) the engine
/// allocates, whatever the heap limit.
pub const MAX_BUFFER_BYTES: usize = 1 << 31;

#[derive(Debug)]
pub(crate) struct Limits {
    /// Counts down to the next interrupt poll.
    pub(crate) countdown: u32,
    termination: Option<TerminationReason>,
    heap_limit: Option<usize>,
    /// Live bytes found by the last garbage collection.
    live_bytes_after_gc: usize,
    /// Allocations after which the next garbage collection runs.
    pub(crate) gc_threshold: usize,
    /// Address of a local in the outermost host entry, 0 outside one.
    stack_base: usize,
    stack_limit: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            countdown: POLL_INTERVAL,
            termination: None,
            heap_limit: None,
            live_bytes_after_gc: 0,
            gc_threshold: GC_ALLOCATION_INTERVAL,
            stack_base: 0,
            stack_limit: DEFAULT_STACK_LIMIT,
        }
    }
}

#[inline(always)]
fn stack_address() -> usize {
    let marker = 0u8;
    core::hint::black_box(&marker) as *const u8 as usize
}

impl Limits {
    /// [`Agent::should_terminate`] for a matcher that holds borrows of the
    /// heap while it runs (the RegExp search): the host's interrupt and the
    /// heap limit, with the heap's size taken before the search (it does not
    /// allocate in the heap). `collectable` says whether a collection could
    /// still bring the heap under the limit, in which case the next
    /// instruction collects instead.
    pub(crate) fn should_terminate_matcher(
        &mut self,
        host_hooks: &dyn crate::ecmascript::HostHooks,
        heap_size: usize,
        collectable: bool,
    ) -> bool {
        if self.termination.is_none() {
            if host_hooks.check_interrupt() {
                self.termination = Some(TerminationReason::Interrupted);
            } else if let Some(limit) = self.heap_limit
                && heap_size > limit
            {
                if collectable {
                    self.gc_threshold = 0;
                } else {
                    self.termination = Some(TerminationReason::OutOfMemory);
                }
            }
        }
        if self.termination.is_some() {
            self.countdown = 1;
            true
        } else {
            false
        }
    }

    /// `interval` is the collector's own pacing (Heap::gc_alloc_limit). Near
    /// the heap limit the next collection comes sooner, so that the heap is
    /// collected before a script is found over the limit.
    fn update_gc_threshold(&mut self, interval: usize) {
        self.gc_threshold = match self.heap_limit {
            Some(limit) => {
                // The allocation counter sees less than the estimate finds
                // live afterwards (element and property storage moves), so
                // collect after a quarter of the headroom.
                let headroom = limit.saturating_sub(self.live_bytes_after_gc);
                interval.min((headroom / 4).max(MIN_GC_ALLOCATION_INTERVAL))
            }
            None => interval,
        };
    }
}

impl Agent {
    /// Sets the heap limit in bytes, or removes it with `None`.
    ///
    /// When the estimate of the bytes the heap holds ([`Agent::heap_size`])
    /// goes past the limit, the running script is terminated with
    /// [`TerminationReason::OutOfMemory`]. A single allocation that would go
    /// past it is refused with a RangeError before anything is allocated.
    pub fn set_heap_limit(&mut self, limit: Option<usize>) {
        self.limits.heap_limit = limit;
        self.limits.update_gc_threshold(self.heap.gc_alloc_limit);
    }

    /// The heap limit set with [`Agent::set_heap_limit`].
    pub fn heap_limit(&self) -> Option<usize> {
        self.limits.heap_limit
    }

    /// A cheap running estimate of the bytes the heap holds: what the last
    /// garbage collection found live plus what was allocated since.
    pub fn heap_size(&self) -> usize {
        self.limits
            .live_bytes_after_gc
            .saturating_add(self.heap.alloc_counter)
            .saturating_add(self.heap.data_bytes_since_gc)
            .saturating_add(self.root_stack_bytes())
    }

    /// Bytes held by the native code's rooted handles (`Scoped` values and
    /// scoped collections). A native loop over a Proxy can root a handle per
    /// iteration; they are live until the loop returns, so no collection can
    /// free them.
    fn root_stack_bytes(&self) -> usize {
        let refs = self.stack_refs.try_borrow().map_or(0, |refs| {
            refs.capacity() * core::mem::size_of::<HeapRootData>()
        });
        let collections = self
            .stack_ref_collections
            .try_borrow()
            .map_or(0, |collections| {
                collections.capacity() * core::mem::size_of::<HeapRootCollection>()
            });
        refs + collections
    }

    /// Sets how many bytes of native stack the JavaScript code run by one
    /// host entry ([`GcAgent::run_in_realm`], [`GcAgent::run_job`]) may use,
    /// counted from that entry. Deeper calls throw a RangeError "Maximum call
    /// stack size exceeded". Leave room under the thread's stack size for the
    /// embedder's own frames and a margin (a quarter of the stack is plenty).
    ///
    /// [`GcAgent::run_in_realm`]: crate::ecmascript::GcAgent::run_in_realm
    /// [`GcAgent::run_job`]: crate::ecmascript::GcAgent::run_job
    pub fn set_stack_limit(&mut self, bytes: usize) {
        self.limits.stack_limit = bytes;
    }

    /// The stack limit set with [`Agent::set_stack_limit`].
    pub fn stack_limit(&self) -> usize {
        self.limits.stack_limit
    }

    /// Why the running script is being terminated, if it is.
    pub fn termination(&self) -> Option<TerminationReason> {
        self.limits.termination
    }

    /// Returns why the last script was terminated and clears it, so that
    /// exception handlers work again.
    pub fn take_termination(&mut self) -> Option<TerminationReason> {
        self.limits.countdown = POLL_INTERVAL;
        self.limits.termination.take()
    }

    /// True if `error` ended a script because of a termination that has not
    /// been taken yet.
    pub fn is_termination(&self, error: JsError) -> bool {
        let _ = error;
        self.limits.termination.is_some()
    }

    /// Terminates the running script: from now on until the termination is
    /// taken, no exception handler catches errors and every poll throws.
    pub fn terminate(&mut self, reason: TerminationReason) {
        if self.limits.termination.is_none() {
            self.limits.termination = Some(reason);
        }
        self.limits.countdown = 1;
    }

    /// The error value that carries a termination out of the running script.
    #[must_use]
    pub fn termination_error<'a>(&mut self, gc: NoGcScope<'a, '_>) -> JsError<'a> {
        let reason = self
            .limits
            .termination
            .unwrap_or(TerminationReason::Interrupted);
        self.throw_exception_with_static_message(ExceptionType::Error, reason.message(), gc)
    }

    /// Counts one step of a long loop and, every few thousand steps, asks the
    /// host whether to interrupt and checks the heap limit. Returns the
    /// termination error when the script must end.
    ///
    /// Native loops whose length the script controls call this once per
    /// iteration.
    #[inline(always)]
    pub fn poll_interrupt<'a>(&mut self, gc: NoGcScope<'a, '_>) -> JsResult<'a, ()> {
        self.limits.countdown -= 1;
        if self.limits.countdown == 0 {
            self.poll_interrupt_now(gc)
        } else {
            Ok(())
        }
    }

    /// Like [`Agent::poll_interrupt`] for `steps` steps at once.
    #[inline]
    pub fn poll_interrupt_steps<'a>(
        &mut self,
        steps: u32,
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        if self.limits.countdown <= steps {
            self.limits.countdown = 1;
            self.poll_interrupt_now(gc)
        } else {
            self.limits.countdown -= steps;
            Ok(())
        }
    }

    /// Counts `steps` steps of work done outside the interpreter loop (one
    /// large BigInt operation) without polling: the next poll then asks the
    /// host at once if the countdown ran out.
    #[inline]
    pub(crate) fn charge_steps(&mut self, steps: u64) {
        let steps = u32::try_from(steps).unwrap_or(u32::MAX);
        if self.limits.countdown <= steps {
            self.limits.countdown = 1;
        } else {
            self.limits.countdown -= steps;
        }
    }

    /// For engines that run without a GC scope (the RegExp matcher): true
    /// when the running script must end. The caller then returns
    /// [`Agent::termination_error`].
    pub fn should_terminate(&mut self) -> bool {
        self.limits.countdown = POLL_INTERVAL;
        if self.limits.termination.is_none() {
            self.check_termination_conditions();
        }
        if self.limits.termination.is_some() {
            self.limits.countdown = 1;
            true
        } else {
            false
        }
    }

    fn check_termination_conditions(&mut self) {
        if self.host_hooks.check_interrupt() {
            self.terminate(TerminationReason::Interrupted);
        } else if let Some(limit) = self.limits.heap_limit
            && self.heap_size() > limit
        {
            let uncollectable = self
                .limits
                .live_bytes_after_gc
                .saturating_add(self.root_stack_bytes());
            if (self.heap.alloc_counter == 0 && self.heap.data_bytes_since_gc == 0)
                || uncollectable > limit
                // A native loop that allocates never reaches the collection
                // the next instruction would run.
                || self.heap_size() > limit.saturating_mul(2)
            {
                self.terminate(TerminationReason::OutOfMemory);
            } else {
                // Collect at the next instruction; the heap is only over the
                // limit if it still is after that.
                self.limits.gc_threshold = 0;
            }
        }
    }

    #[inline(never)]
    #[cold]
    pub(crate) fn poll_interrupt_now<'a>(&mut self, gc: NoGcScope<'a, '_>) -> JsResult<'a, ()> {
        if self.should_terminate() {
            Err(self.termination_error(gc))
        } else {
            Ok(())
        }
    }

    /// Refuses an allocation of `bytes` that the heap limit (or the largest
    /// buffer the engine makes) does not leave room for, with a RangeError,
    /// before anything is allocated.
    pub fn check_allocation<'a>(
        &mut self,
        bytes: usize,
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        if bytes > MAX_BUFFER_BYTES {
            return Err(self.throw_exception_with_static_message(
                ExceptionType::RangeError,
                "Allocation too large",
                gc,
            ));
        }
        if self.allocation_over_heap_limit(bytes) {
            return Err(self.throw_exception_with_static_message(
                ExceptionType::RangeError,
                "Out of memory: allocation over the heap limit",
                gc,
            ));
        }
        Ok(())
    }

    /// Whether `bytes` more cannot fit under the heap limit. The estimate
    /// ([`Agent::heap_size`]) counts everything allocated since the last
    /// collection, garbage included, so going over it only refuses when the
    /// heap the last collection found live has no room either; otherwise the
    /// next instruction collects, and the poll after it terminates the script
    /// if the heap is still over the limit.
    fn allocation_over_heap_limit(&mut self, bytes: usize) -> bool {
        let Some(limit) = self.limits.heap_limit else {
            return false;
        };
        if bytes == 0 || self.heap_size().saturating_add(bytes) <= limit {
            return false;
        }
        if self.limits.live_bytes_after_gc.saturating_add(bytes) > limit {
            return true;
        }
        self.limits.gc_threshold = 0;
        false
    }

    /// Refuses an allocation of `bytes` that the heap limit does not leave
    /// room for, with a RangeError. Unlike [`Agent::check_allocation`], no
    /// fixed cap applies without a heap limit.
    pub fn check_heap_limit<'a>(
        &mut self,
        bytes: usize,
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        if self.allocation_over_heap_limit(bytes) {
            return Err(self.throw_exception_with_static_message(
                ExceptionType::RangeError,
                "Out of memory: allocation over the heap limit",
                gc,
            ));
        }
        Ok(())
    }

    /// Refuses a string of `length` UTF-16 code units (RangeError "Invalid
    /// string length") before it is built.
    pub fn check_string_length<'a>(
        &mut self,
        length: usize,
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        if length > MAX_STRING_LENGTH {
            return Err(self.throw_exception_with_static_message(
                ExceptionType::RangeError,
                "Invalid string length",
                gc,
            ));
        }
        self.check_allocation(length, gc)
    }

    /// Refuses the concatenation of two strings that would be too long (see
    /// [`Agent::check_string_length`]); the byte lengths bound the code unit
    /// count from above.
    #[inline]
    pub fn check_concat_length<'a>(
        &mut self,
        a: String,
        b: String,
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        let bytes = a.len_(self).saturating_add(b.len_(self));
        if bytes > 4096 {
            self.check_string_length(bytes, gc)
        } else {
            Ok(())
        }
    }

    /// [`Agent::check_concat_length`] for a list of strings.
    pub fn check_concat_lengths<'a>(
        &mut self,
        strings: &[String],
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        let bytes = strings
            .iter()
            .fold(0usize, |total, s| total.saturating_add(s.len_(self)));
        self.check_string_length(bytes, gc)
    }

    /// Throws a RangeError "Maximum call stack size exceeded" when the native
    /// stack used since the host entered the Agent is over the stack limit.
    #[inline]
    pub fn check_stack<'a>(&mut self, gc: NoGcScope<'a, '_>) -> JsResult<'a, ()> {
        if self.stack_exhausted() {
            Err(self.throw_stack_overflow(gc))
        } else {
            Ok(())
        }
    }

    /// True when the native stack used since the host entered the Agent is
    /// over the stack limit. For recursive code that cannot throw where it
    /// checks (the bytecode compiler).
    #[inline]
    pub fn stack_exhausted(&self) -> bool {
        let base = self.limits.stack_base;
        base != 0 && base.saturating_sub(stack_address()) > self.limits.stack_limit
    }

    #[inline(never)]
    #[cold]
    pub(crate) fn throw_stack_overflow<'a>(&mut self, gc: NoGcScope<'a, '_>) -> JsError<'a> {
        self.throw_exception_with_static_message(
            ExceptionType::RangeError,
            "Maximum call stack size exceeded",
            gc,
        )
    }

    /// Called when the host enters the Agent with no JavaScript running:
    /// records where the stack starts and clears an earlier termination.
    pub(crate) fn enter_from_host(&mut self) {
        self.limits.stack_base = stack_address();
        self.limits.termination = None;
        self.limits.countdown = POLL_INTERVAL;
    }

    /// Called after a garbage collection: records the live heap size and
    /// terminates the running script if it is over the heap limit.
    pub(crate) fn after_gc(&mut self) {
        self.heap.data_bytes_since_gc = 0;
        self.limits.live_bytes_after_gc = self.heap.estimate_live_bytes();
        self.limits.update_gc_threshold(self.heap.gc_alloc_limit);
        if let Some(limit) = self.limits.heap_limit
            && self.limits.live_bytes_after_gc > limit
        {
            self.terminate(TerminationReason::OutOfMemory);
        }
    }
}
