// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Counts the bytes each engine's allocations hold, for the memory limit.
//!
//! The engine's own estimate of its heap counts the live records of the
//! heap's vectors; what those vectors keep spare, the hash tables beside
//! them, bytecode being compiled and every temporary of native code are
//! outside it, and a script chooses which of them it grows. This allocator
//! counts what the engine asks of the system instead: every block carries the
//! account of the engine that was running when it was allocated, in a header
//! in front of it, and gives its bytes back to that account when it is freed,
//! whichever thread or call frees it. So an account holds exactly the bytes
//! of the live blocks made on its engine's behalf.
//!
//! Blocks allocated with no engine running (a compiled script shared by
//! engines, the standard library's own) go to account 0, which nobody reads.

use core::{
    alloc::{GlobalAlloc, Layout},
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::alloc::System;

/// Engines counted at once; one more runs uncounted (by the engine's
/// estimate).
const ACCOUNTS: usize = 1024;

/// Bytes in front of every block: the account, and padding up to the
/// alignment every block has at least.
const HEADER: usize = 16;

static BYTES: [AtomicUsize; ACCOUNTS] = [const { AtomicUsize::new(0) }; ACCOUNTS];
static TAKEN: [AtomicBool; ACCOUNTS] = [const { AtomicBool::new(false) }; ACCOUNTS];
static NEXT: AtomicUsize = AtomicUsize::new(1);

thread_local! {
    /// The account of the engine running on this thread; 0 for none. A plain
    /// `Cell` with a constant initialiser: reading it allocates nothing and
    /// it has no destructor, so the allocator may read it at any time.
    static CURRENT: Cell<usize> = const { Cell::new(0) };
}

/// An engine's count of allocated bytes. Dropping it frees the account for
/// another engine; blocks still alive then (a result the caller has not
/// freed) go on counting on it, and leave it as they are freed.
pub(crate) struct Account(usize);

impl Account {
    /// A free account, or the uncounted one when every account is taken.
    pub(crate) fn new() -> Account {
        for _ in 0..ACCOUNTS {
            let index = NEXT.fetch_add(1, Ordering::Relaxed) % ACCOUNTS;
            if index != 0
                && TAKEN[index]
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
            {
                return Account(index);
            }
        }
        Account(0)
    }

    /// Bytes held by the live blocks allocated while this account's engine
    /// ran, or `None` for the uncounted account.
    pub(crate) fn bytes(&self) -> Option<usize> {
        if self.0 == 0 {
            return None;
        }
        // A block of an earlier engine that had this account can be freed
        // after the account was taken again; the count then dips below the
        // bytes of this engine by that block, never below zero for long.
        let bytes = BYTES[self.0].load(Ordering::Relaxed);
        Some(if bytes > isize::MAX as usize {
            0
        } else {
            bytes
        })
    }

    /// Makes this the account allocations on this thread go to, until the
    /// guard is dropped (then the account before it is back: calls nest when
    /// a host function calls into another engine).
    pub(crate) fn enter(&self) -> Entered {
        Entered(CURRENT.with(|current| current.replace(self.0)))
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        if self.0 != 0 {
            TAKEN[self.0].store(false, Ordering::Release);
        }
    }
}

pub(crate) struct Entered(usize);

impl Drop for Entered {
    fn drop(&mut self) {
        CURRENT.with(|current| current.set(self.0));
    }
}

/// Where the block starts after the system's allocation, and the layout
/// asked of the system: the block's own with the header in front, the header
/// rounded up to the block's alignment so that the block stays aligned.
fn outer(layout: Layout) -> Option<(usize, Layout)> {
    let offset = layout.align().max(HEADER);
    let size = layout.size().checked_add(offset)?;
    let outer = Layout::from_size_align(size, offset).ok()?;
    Some((offset, outer))
}

struct Metered;

impl Metered {
    /// # Safety
    /// `base` must be a live allocation of at least `offset` bytes aligned to
    /// 16, `offset` at least [`HEADER`].
    unsafe fn open(base: *mut u8, offset: usize, bytes: usize) -> *mut u8 {
        let account = CURRENT.try_with(Cell::get).unwrap_or(0);
        BYTES[account].fetch_add(bytes, Ordering::Relaxed);
        // SAFETY: `offset >= HEADER`, so both pointers are inside the
        // allocation, and `base + offset - HEADER` is aligned to 16 (`base`
        // is aligned to `offset`, a power of two of at least 16), which a
        // usize needs at most.
        unsafe {
            let block = base.add(offset);
            block.sub(HEADER).cast::<usize>().write(account);
            block
        }
    }

    /// The account in the header of `block`.
    ///
    /// # Safety
    /// `block` must have come from [`Metered::open`] and still be allocated.
    unsafe fn account_of(block: *mut u8) -> usize {
        // SAFETY: `open` wrote a usize there, inside the allocation.
        let account = unsafe { block.sub(HEADER).cast::<usize>().read() };
        // Never index out of the table, whatever a damaged header says.
        if account < ACCOUNTS { account } else { 0 }
    }
}

// SAFETY: every method forwards to `System` with a layout made of the
// caller's by `outer` (the same function for the same block every time, so
// allocation, reallocation and deallocation agree), and returns or takes the
// pointer `offset` bytes inside it, which keeps the caller's alignment. The
// counters are plain atomics and the thread-local is read without
// allocating, so nothing here calls back into the allocator or unwinds.
unsafe impl GlobalAlloc for Metered {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some((offset, outer)) = outer(layout) else {
            return core::ptr::null_mut();
        };
        // SAFETY: `outer` has a non-zero size (it holds the header).
        let base = unsafe { System.alloc(outer) };
        if base.is_null() {
            return base;
        }
        // SAFETY: `base` is `outer.size() >= offset` bytes aligned to `offset`.
        unsafe { Metered::open(base, offset, outer.size()) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let Some((offset, outer)) = outer(layout) else {
            return core::ptr::null_mut();
        };
        // SAFETY: as in `alloc`.
        let base = unsafe { System.alloc_zeroed(outer) };
        if base.is_null() {
            return base;
        }
        // SAFETY: as in `alloc`.
        unsafe { Metered::open(base, offset, outer.size()) }
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        // `layout` is the one the block was allocated with, for which
        // `outer` succeeded then.
        let Some((offset, outer)) = outer(layout) else {
            return;
        };
        // SAFETY: the caller's block came from `alloc`, `alloc_zeroed` or
        // `realloc` here, all of which return `base + offset` of an `outer`
        // allocation with the header written.
        unsafe {
            let account = Metered::account_of(block);
            BYTES[account].fetch_sub(outer.size(), Ordering::Relaxed);
            System.dealloc(block.sub(offset), outer);
        }
    }

    unsafe fn realloc(&self, block: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let Some((offset, outer)) = outer(layout) else {
            return core::ptr::null_mut();
        };
        let Some(new_outer_size) = new_size.checked_add(offset) else {
            return core::ptr::null_mut();
        };
        if Layout::from_size_align(new_outer_size, offset).is_err() {
            return core::ptr::null_mut();
        }
        // SAFETY: as in `dealloc` for the block; the new size is non-zero and
        // fits a layout of the same alignment (checked above). The system
        // keeps the first bytes, the header among them, so the block stays on
        // the account that allocated it.
        unsafe {
            let account = Metered::account_of(block);
            let base = System.realloc(block.sub(offset), outer, new_outer_size);
            if base.is_null() {
                return base;
            }
            if new_outer_size >= outer.size() {
                BYTES[account].fetch_add(new_outer_size - outer.size(), Ordering::Relaxed);
            } else {
                BYTES[account].fetch_sub(outer.size() - new_outer_size, Ordering::Relaxed);
            }
            base.add(offset)
        }
    }
}

#[global_allocator]
static METERED: Metered = Metered;
