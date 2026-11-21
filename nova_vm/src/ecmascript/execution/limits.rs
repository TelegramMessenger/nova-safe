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

/// A heap whose live bytes are past twice [`GC_ALLOCATION_INTERVAL`] waits
/// for allocations of half its live bytes before the next collection, up to
/// this many: a collection costs in proportion to the live heap, so a few
/// megabytes of live bytecode, shapes and strings are not marked and
/// compacted again after every 2 MiB. The cap keeps the garbage that waits
/// between collections (and the size of a process) close to what it was.
const MAX_LIVE_BYTES_GC_ALLOCATION_INTERVAL: usize = 4 * 1024 * 1024;

/// Native stack the JavaScript code of one host entry may use when the
/// embedder does not say.
///
/// Rust's spawned threads get 2 MiB by default and the main thread 8 MiB;
/// half of the smaller one leaves room for the embedder's frames and for
/// native code running between two checks.
pub const DEFAULT_STACK_LIMIT: usize = 1024 * 1024;

/// The message of the RangeError that refuses one allocation over the heap
/// limit.
const ALLOCATION_REFUSED: &str = "Out of memory: allocation over the heap limit";

/// Largest length of a JavaScript string, in UTF-16 code units, the engine
/// creates (QuickJS uses the same bound).
pub const MAX_STRING_LENGTH: usize = (1 << 30) - 1;

/// Largest single buffer (ArrayBuffer bytes, element storage) the engine
/// allocates, whatever the heap limit.
pub const MAX_BUFFER_BYTES: usize = 1 << 31;

/// What ran between two polls of the host: counted in the tests of this
/// module only, which hold the interval that the countdown promises.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct Work {
    /// Instructions and steps of native loops since the last poll.
    since_poll: u32,
    /// The most of them between two polls.
    most: u32,
    polls: u32,
    total: u64,
}

#[derive(Debug)]
pub(crate) struct Limits {
    /// Counts down to the next interrupt poll.
    pub(crate) countdown: u32,
    /// Steps that the running `Vm` took out of the countdown for the
    /// instructions it runs without counting them one by one
    /// ([`Agent::lend_steps`]): a poll starts the countdown again without
    /// them, as they are still to be made.
    lent: u32,
    #[cfg(test)]
    work: Work,
    termination: Option<TerminationReason>,
    heap_limit: Option<usize>,
    /// Whether an allocation the heap limit refuses terminates the script.
    refusal_terminates: bool,
    /// Live bytes found by the last garbage collection: the embedder's count
    /// ([`HostHooks::allocated_bytes`](crate::ecmascript::HostHooks::allocated_bytes))
    /// when it keeps one, else the estimate.
    live_bytes_after_gc: usize,
    /// With the embedder's count: how many counted bytes one estimated byte
    /// stood for at the last garbage collection, in 256ths.
    counted_per_estimated: Option<usize>,
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
            lent: 0,
            #[cfg(test)]
            work: Work::default(),
            termination: None,
            heap_limit: None,
            refusal_terminates: false,
            live_bytes_after_gc: 0,
            counted_per_estimated: None,
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
                let headroom = limit.saturating_sub(self.live_bytes_after_gc);
                match self.counted_per_estimated {
                    // The embedder's count is exact, and the allocation
                    // counter paces in estimated bytes: collect when half the
                    // headroom is used, in the counter's bytes. (A heap that
                    // gets past the limit before that is collected by the
                    // next poll, before it is judged.)
                    Some(ratio) => {
                        let estimated = |counted: usize| counted.saturating_mul(256) / ratio;
                        interval
                            .min((estimated(headroom) / 2).max(estimated(MIN_GC_ALLOCATION_INTERVAL)))
                    }
                    // The allocation counter sees less than the estimate
                    // finds live afterwards (element and property storage
                    // moves), so collect after a quarter of the headroom.
                    None => interval.min((headroom / 4).max(MIN_GC_ALLOCATION_INTERVAL)),
                }
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
    /// past it is refused with a RangeError before anything is allocated (or
    /// terminates the script too, see
    /// [`Agent::set_heap_limit_refusal_terminates`]).
    pub fn set_heap_limit(&mut self, limit: Option<usize>) {
        self.limits.heap_limit = limit;
        self.heap.text_join_budget.set_cap(limit);
        self.limits.update_gc_threshold(self.heap.gc_alloc_limit);
    }

    /// The heap limit set with [`Agent::set_heap_limit`].
    pub fn heap_limit(&self) -> Option<usize> {
        self.limits.heap_limit
    }

    /// Chooses what an allocation the heap limit has no room for does. By
    /// default it is refused with a RangeError, which a script can catch and
    /// so go on just under the limit. With `terminates` it ends the script
    /// with [`TerminationReason::OutOfMemory`], as going over the limit does:
    /// for an embedder whose contract is that a script out of memory stops.
    /// The script then ends only for memory it really holds: an allocation
    /// that garbage may still make room for is made, and the collection at
    /// the next instruction decides.
    pub fn set_heap_limit_refusal_terminates(&mut self, terminates: bool) {
        self.limits.refusal_terminates = terminates;
    }

    /// Whether a refused allocation terminates the script
    /// ([`Agent::set_heap_limit_refusal_terminates`]).
    pub fn heap_limit_refusal_terminates(&self) -> bool {
        self.limits.refusal_terminates
    }

    /// The error of an allocation that does not fit under the heap limit: the
    /// RangeError `message`, or the termination when a refusal terminates
    /// ([`Agent::set_heap_limit_refusal_terminates`]) and a heap limit is set.
    #[must_use]
    pub fn throw_out_of_memory<'a>(
        &mut self,
        message: &'static str,
        gc: NoGcScope<'a, '_>,
    ) -> JsError<'a> {
        if self.limits.refusal_terminates && self.limits.heap_limit.is_some() {
            self.terminate(TerminationReason::OutOfMemory);
            self.termination_error(gc)
        } else {
            self.throw_exception_with_static_message(ExceptionType::RangeError, message, gc)
        }
    }

    /// The bytes the heap limit counts: the embedder's count of the engine's
    /// allocations when it keeps one, else a cheap running estimate of the
    /// bytes the heap holds (what the last garbage collection found live plus
    /// what was allocated since).
    pub fn heap_size(&self) -> usize {
        if let Some(counted) = self.host_hooks.allocated_bytes() {
            return counted;
        }
        self.limits
            .live_bytes_after_gc
            .saturating_add(self.heap.alloc_counter)
            .saturating_add(self.heap.data_bytes_since_gc)
            .saturating_add(self.heap.text_join_budget.joined())
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
        self.limits.lent = 0;
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
        // Nothing that is given back makes the countdown longer again.
        self.limits.lent = 0;
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
        #[cfg(test)]
        self.count_work();
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

    /// Takes half of what is left of the countdown out of it, for the
    /// instructions that the loop of a `Vm` runs without counting them one
    /// by one: it stops after as many steps as this returns, and gives back
    /// what it did not make ([`Agent::return_steps`]) before anything else
    /// runs instructions. The host is asked first where the countdown has
    /// run out, so that what is lent is never nothing.
    ///
    /// The other half stays for the native loops that the instructions
    /// call meanwhile, which count their steps as ever: what is lent and
    /// what they count are never more than [`POLL_INTERVAL`] together
    /// between two polls.
    #[inline(always)]
    pub(crate) fn lend_steps<'a>(&mut self, gc: NoGcScope<'a, '_>) -> JsResult<'a, u32> {
        self.limits.lent = 0;
        if self.limits.countdown <= 1 {
            self.poll_interrupt_now(gc)?;
        }
        let lent = self.limits.countdown / 2;
        self.limits.countdown -= lent;
        self.limits.lent = lent;
        Ok(lent)
    }

    /// One instruction that the loop of a `Vm` runs, or one step of a
    /// native loop.
    #[cfg(test)]
    #[inline(always)]
    pub(crate) fn count_work(&mut self) {
        self.limits.work.since_poll += 1;
        self.limits.work.total += 1;
    }

    /// Puts the steps that were lent and not made back into the countdown:
    /// `unused` of them, and never more than were lent.
    #[inline(always)]
    pub(crate) fn return_steps(&mut self, unused: usize) {
        let lent = self.limits.lent;
        self.limits.lent = 0;
        self.limits.countdown += if unused < lent as usize {
            unused as u32
        } else {
            lent
        };
    }

    /// For engines that run without a GC scope (the RegExp matcher): true
    /// when the running script must end. The caller then returns
    /// [`Agent::termination_error`].
    pub fn should_terminate(&mut self) -> bool {
        #[cfg(test)]
        {
            let work = &mut self.limits.work;
            work.most = work.most.max(work.since_poll);
            work.since_poll = 0;
            work.polls += 1;
        }
        // The steps lent are made after this poll: they are part of what
        // may pass before the next one.
        self.limits.countdown = POLL_INTERVAL - self.limits.lent.min(POLL_INTERVAL / 2);
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
            // The embedder's count has the rooted handles in it already.
            let uncollectable = match self.host_hooks.allocated_bytes() {
                Some(_) => self.limits.live_bytes_after_gc,
                None => self
                    .limits
                    .live_bytes_after_gc
                    .saturating_add(self.root_stack_bytes()),
            };
            // Text joined since the last collection counts as allocated
            // since then: the collection lets go of the pieces it was
            // joined from.
            let joined = self.heap.text_join_budget.joined();
            if (self.heap.alloc_counter == 0 && self.heap.data_bytes_since_gc == 0 && joined == 0)
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
            return Err(self.throw_out_of_memory(ALLOCATION_REFUSED, gc));
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
        let size = self.heap_size();
        if bytes == 0 || size.saturating_add(bytes) <= limit {
            return false;
        }
        if self.limits.live_bytes_after_gc.saturating_add(bytes) > limit {
            // What became garbage since the last collection is not known
            // here. A refusal a script can catch may err on that; one that
            // ends the script must not, so unless no collection could make
            // room the allocation goes through, the next instruction
            // collects, and the heap found over the limit then ends the
            // script. The heap stays under twice the limit, as a native loop
            // between two polls keeps it.
            if !self.limits.refusal_terminates
                || bytes > limit
                || size.saturating_add(bytes) > limit.saturating_mul(2)
            {
                return true;
            }
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
            return Err(self.throw_out_of_memory(ALLOCATION_REFUSED, gc));
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

    /// [`Agent::check_string_length`] for a string whose storage is known to
    /// take `bytes` of WTF-8: text outside ASCII takes up to three bytes for
    /// a code unit, so the length alone would let a string of three times the
    /// heap limit be built before the next poll finds the heap over it.
    pub fn check_string_size<'a>(
        &mut self,
        length: usize,
        bytes: usize,
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        if length > MAX_STRING_LENGTH {
            return Err(self.throw_exception_with_static_message(
                ExceptionType::RangeError,
                "Invalid string length",
                gc,
            ));
        }
        self.check_allocation(bytes.max(length), gc)
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
        if bytes <= 4096 {
            return Ok(());
        }
        // Appending to a string that is being built allocates what is
        // appended, not the whole text again: the text only has to fit the
        // limit, which reading it is then held to.
        if a.is_being_built_(self) {
            if bytes > MAX_STRING_LENGTH {
                return Err(self.throw_exception_with_static_message(
                    ExceptionType::RangeError,
                    "Invalid string length",
                    gc,
                ));
            }
            if self.limits.heap_limit.is_some_and(|limit| bytes > limit) {
                return Err(self.throw_out_of_memory(ALLOCATION_REFUSED, gc));
            }
            // A few bytes more are let through as a short string is above:
            // the poll after it holds the heap to the limit.
            let added = b.len_(self);
            if added <= 4096 {
                return Ok(());
            }
            return self.check_allocation(added, gc);
        }
        // And what is put before one: the left string, which is not being
        // built here, is what is allocated.
        if b.is_being_built_(self) {
            if bytes > MAX_STRING_LENGTH {
                return Err(self.throw_exception_with_static_message(
                    ExceptionType::RangeError,
                    "Invalid string length",
                    gc,
                ));
            }
            if self.limits.heap_limit.is_some_and(|limit| bytes > limit) {
                return Err(self.throw_out_of_memory(ALLOCATION_REFUSED, gc));
            }
            let added = a.len_(self);
            if added <= 4096 {
                return Ok(());
            }
            return self.check_allocation(added, gc);
        }
        self.check_string_length(bytes, gc)
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
        self.limits.lent = 0;
    }

    /// Called after a garbage collection: records the live heap size and
    /// terminates the running script if it is over the heap limit.
    pub(crate) fn after_gc(&mut self) {
        self.heap.data_bytes_since_gc = 0;
        self.heap.text_join_budget.settled();
        let estimated = self.heap.estimate_live_bytes();
        match self.host_hooks.allocated_bytes() {
            Some(counted) => {
                self.limits.live_bytes_after_gc = counted;
                self.limits.counted_per_estimated =
                    Some((counted.saturating_mul(256) / estimated.max(1)).clamp(256, 16 * 256));
            }
            None => {
                self.limits.live_bytes_after_gc = estimated;
                self.limits.counted_per_estimated = None;
            }
        }
        self.heap.gc_alloc_limit = self.heap.gc_alloc_limit.max(
            (self.limits.live_bytes_after_gc / 2).min(MAX_LIVE_BYTES_GC_ALLOCATION_INTERVAL),
        );
        self.limits.update_gc_threshold(self.heap.gc_alloc_limit);
        if let Some(limit) = self.limits.heap_limit
            && self.limits.live_bytes_after_gc > limit
        {
            self.terminate(TerminationReason::OutOfMemory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::POLL_INTERVAL;
    use crate::{
        ecmascript::{Agent, AgentOptions, DefaultHostHooks, String, initialize_default_realm},
        engine::{Bindable, GcScope},
    };

    /// Runs `source` and returns the most instructions and steps of native
    /// loops that passed between two polls of the host, how many polls
    /// there were, and how much ran in all.
    fn work_between_polls(source: &str) -> (u32, u32, u64) {
        let (mut gc, mut scope) = GcScope::create_root();
        let mut gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        initialize_default_realm(&mut agent, gc.reborrow());
        agent.limits.work = Default::default();
        let source_text = String::from_string(&mut agent, source.to_string(), gc.nogc());
        let result = agent.run_script(source_text.unbind(), gc.reborrow());
        assert!(result.is_ok(), "the script throws: {source}");
        let work = &agent.limits.work;
        (work.most.max(work.since_poll), work.polls, work.total)
    }

    /// No more than the interval runs between two polls, and the host is
    /// not asked much more often than the interval says either.
    fn holds_the_interval(name: &str, source: &str) {
        let (most, polls, total) = work_between_polls(source);
        assert!(
            most <= POLL_INTERVAL,
            "{name}: {most} instructions and steps between two polls"
        );
        assert!(
            total >= u64::from(POLL_INTERVAL) * 8,
            "{name}: only {total} instructions and steps, which shows little"
        );
        // An instruction is a few bytes, each a step of the countdown.
        let least = total / u64::from(POLL_INTERVAL);
        assert!(
            u64::from(polls) >= least && u64::from(polls) <= least * 12 + 16,
            "{name}: {polls} polls for {total} instructions and steps"
        );
    }

    fn repeated(statement: &str, times: usize) -> std::string::String {
        statement.repeat(times)
    }

    /// No instruction of the loop of a `Vm` counts steps of its own so far:
    /// the two ways of counting are put together here as such an
    /// instruction would, a native loop running while steps are lent.
    #[test]
    fn steps_lent_and_steps_of_native_loops_share_the_interval() {
        let (mut gc, mut scope) = GcScope::create_root();
        let gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        let mut seed = 0x2545_f491_u32;
        let mut next = |below: u32| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) % below
        };
        for turn in 0..20_000 {
            let lent = agent.lend_steps(gc.nogc()).unwrap();
            assert!(lent >= 1 && lent <= POLL_INTERVAL / 2);
            // Mostly a few instructions, sometimes all that were lent.
            let made = if turn % 7 == 0 {
                lent
            } else {
                next(lent.min(40) + 1)
            };
            let before = next(made + 1);
            for _ in 0..before {
                agent.count_work();
            }
            // A native loop of a length that the script chooses.
            let steps = match turn % 5 {
                0 => next(3 * POLL_INTERVAL),
                1 => next(300),
                _ => 0,
            };
            for _ in 0..steps {
                agent.poll_interrupt(gc.nogc()).unwrap();
            }
            for _ in before..made {
                agent.count_work();
            }
            agent.return_steps((lent - made) as usize);
            assert!(agent.limits.countdown >= 1 && agent.limits.countdown <= POLL_INTERVAL);
            assert!(agent.limits.work.since_poll <= POLL_INTERVAL);
        }
        let work = &agent.limits.work;
        assert!(work.polls > 1000);
        assert!(
            work.most <= POLL_INTERVAL,
            "{} instructions and steps between two polls",
            work.most
        );
        // More is never given back than was lent.
        let lent = agent.lend_steps(gc.nogc()).unwrap();
        let countdown = agent.limits.countdown;
        agent.return_steps(usize::MAX);
        assert_eq!(agent.limits.countdown, countdown + lent);
        agent.return_steps(usize::MAX);
        assert_eq!(agent.limits.countdown, countdown + lent);
    }

    #[test]
    fn a_straight_line_is_polled() {
        holds_the_interval(
            "global statements",
            &format!("var x = 0; {} x", repeated("x = x + 1; ", 40_000)),
        );
        holds_the_interval(
            "local statements",
            &format!(
                "(function () {{ let x = 0; {} return x; }})()",
                repeated("x = x + 1; ", 60_000)
            ),
        );
        holds_the_interval(
            "calls of a native function",
            &format!("var x = 0; {} x", repeated("x = Math.abs(x) + 1; ", 20_000)),
        );
        holds_the_interval(
            "calls in place",
            &format!(
                "function f(a) {{ return a + 1; }} var x = 0; {} x",
                repeated("x = f(x); ", 20_000)
            ),
        );
    }

    #[test]
    fn loops_are_polled() {
        holds_the_interval("empty", "for (let i = 0; i < 300000; i++) {}");
        holds_the_interval(
            "sum",
            "let s = 0; for (let i = 0; i < 200000; i++) { s += i; } s",
        );
        holds_the_interval(
            "in a function",
            "(function () { let s = 0, i = 0; while (i < 200000) { s += i; i++; } \
             do { i--; } while (i > 0); let n = 100000; while (n) { n--; } return s; })()",
        );
        holds_the_interval(
            "a long body",
            &format!(
                "(function () {{ let x = 0; for (let i = 0; i < 60; i++) {{ {} }} return x; }})()",
                repeated("x = x + 1; ", 3000)
            ),
        );
        holds_the_interval(
            "a body that is mostly passed over",
            &format!(
                "(function () {{ let x = 0; for (let i = 0; i < 100000; i++) {{ \
                 if (i < 0) {{ {} }} x += i & 3; }} return x; }})()",
                repeated("x = x + 1; ", 2000)
            ),
        );
        holds_the_interval(
            "a switch",
            &format!(
                "(function () {{ let x = 0; for (let i = 0; i < 100000; i++) {{ \
                 switch (i & 7) {{ case 0: {body} break; case 1: x += 1; break; \
                 case 2: {body} break; case 3: x += 3; continue; case 4: {body} break; \
                 default: x += 5; }} }} return x; }})()",
                body = repeated("x = x + 1; ", 20)
            ),
        );
        holds_the_interval(
            "tests that jump",
            "(function () { let a = 0, b = null, o = { k: 1 }, n = 0; \
             for (let i = 0; i < 100000; i++) { \
             a = (i & 1) || (i & 2) || 7; b = b ?? i; n += (i % 3 === 0) ? 1 : (a && o.k) ? 2 : 3; \
             if (i === 5 || typeof b !== 'number' || o.k == null) { n--; } \
             if (!(i & 1)) continue; n++; } return n; })()",
        );
        holds_the_interval(
            "nested",
            "let n = 0; for (let i = 0; i < 400; i++) { for (let j = 0; j < 400; j++) { n += j; } } n",
        );
        holds_the_interval(
            "iterations",
            "(function () { const a = []; for (let i = 0; i < 3000; i++) a.push(i); \
             const o = {}; for (let i = 0; i < 200; i++) o['k' + i] = i; let n = 0; \
             for (let t = 0; t < 20; t++) { for (const v of a) n += v; for (const k in o) n += o[k]; \
             for (const [i, v] of a.entries()) n += i - v; } return n; })()",
        );
    }

    #[test]
    fn calls_are_polled() {
        holds_the_interval(
            "recursion",
            "function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(22)",
        );
        holds_the_interval(
            "closures, methods and accessors",
            "(function () { function counter() { let c = 0; return () => ++c; } \
             const next = counter(); \
             const o = { v: 1, get g() { return this.v; }, set s(x) { this.v = x; }, \
             m(x) { return x + this.v; } }; const b = o.m.bind(o); let n = 0; \
             for (let i = 0; i < 30000; i++) { n += next() + o.g + o.m(i) + b(i); o.s = i & 7; } \
             return n; })()",
        );
        holds_the_interval(
            "classes",
            "(function () { class A { constructor(x) { this.x = x; } get d() { return this.x * 2; } } \
             class B extends A { y = 1; constructor(x) { super(x); this.z = x; } \
             sum() { return this.x + this.y + this.z + this.d; } } \
             class C extends B { w = 2; } let n = 0; \
             for (let i = 0; i < 20000; i++) { n += new B(i).sum() + new C(i).w + new A(i).d; } \
             return n; })()",
        );
        holds_the_interval(
            "generators",
            "(function () { function* count(n) { for (let i = 0; i < n; i++) yield i; } \
             function* twice(n) { yield* count(n); yield* count(n); } let n = 0; \
             for (let t = 0; t < 200; t++) { for (const v of twice(100)) n += v; } \
             const g = count(40000); for (let r = g.next(); !r.done; r = g.next()) n += r.value; \
             return n; })()",
        );
        holds_the_interval(
            "functions that native loops call",
            "(function () { const a = []; for (let i = 0; i < 2000; i++) a.push(i); let n = 0; \
             for (let t = 0; t < 10; t++) { a.forEach((v) => { n += v; }); \
             n += a.map((v) => v * 2).filter((v) => v % 3 === 0).reduce((s, v) => s + v, 0); \
             a.slice().sort((x, y) => y - x); } \
             function walk(d) { return d === 0 ? 1 : [0, 1, 2].map(() => walk(d - 1)).reduce((s, v) => s + v, 0); } \
             return n + walk(8); })()",
        );
        holds_the_interval(
            "what is thrown and caught",
            "(function () { function f(i) { if (i & 1) throw new Error('odd'); return i; } \
             let n = 0; for (let i = 0; i < 20000; i++) { \
             try { n += f(i); } catch (e) { n -= 1; } finally { n += 2; } } return n; })()",
        );
    }

    #[test]
    fn native_loops_among_instructions_are_polled() {
        holds_the_interval(
            "searches",
            "(function () { const a = []; for (let i = 0; i < 1500; i++) a.push(i); let n = 0; \
             for (let i = 0; i < 3000; i++) { n += a.indexOf(1400 + (i & 63)) + (a.includes(-1) ? 1 : 0); \
             n += a.lastIndexOf(i); } return n; })()",
        );
        holds_the_interval(
            "short searches in a long loop",
            "(function () { const a = [1, 2, 3, 4, 5, 6, 7, 8]; let n = 0; \
             for (let i = 0; i < 60000; i++) { n += a.indexOf(i & 15); } return n; })()",
        );
        holds_the_interval(
            "arrays built and joined",
            "(function () { let n = 0; for (let i = 0; i < 300; i++) { \
             const a = new Array(500).fill(i); a.reverse(); n += a.join('').length + a.concat(a).length; \
             n += a.slice(10).length + String(i).padStart(40, 'x').length; } return n; })()",
        );
        holds_the_interval(
            "text",
            "(function () { let s = ''; let n = 0; for (let i = 0; i < 20000; i++) { \
             s += 'ab'; if (s.length > 2000) { n += s.indexOf('ba', 1000) + s.split('a').length; s = ''; } \
             n += 'x'.repeat(i & 31).length; } return n; })()",
        );
    }
}
