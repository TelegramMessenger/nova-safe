// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The allocator at its edges, as the process's allocator and called
//! directly: alignments above the header, sizes no layout or system can
//! hold, zero sizes, blocks that grow while another account is entered or
//! are freed on another thread, and allocations made while a thread's
//! locals are destroyed.

use core::hint::black_box;
use std::{
    alloc::{GlobalAlloc, Layout},
    sync::{Mutex, OnceLock},
};

use nova_meter::{Account, Metered};

#[global_allocator]
static METERED: Metered = Metered;

/// The allocator's header: what a block aligned to at most this much has in
/// front of it.
const HEADER: usize = 16;

#[repr(align(64))]
struct Wide([u8; 64]);

/// A block of `layout` from the process's allocator with `account` entered.
fn alloc_on(account: &Account, layout: Layout) -> *mut u8 {
    let _entered = account.enter();
    // SAFETY: every layout the tests pass has a non-zero size.
    black_box(unsafe { std::alloc::alloc(layout) })
}

/// A block aligned to more than the header has a whole alignment in front
/// of it; it keeps its alignment and its bytes when it grows (while another
/// account is entered: it stays on its own) and shrinks, and leaves its
/// account when it is freed.
#[test]
fn alignments_above_the_header() {
    let account = Account::new();
    let other = Account::new();
    let before = account.bytes().expect("a counted account");
    let other_before = other.bytes().expect("a counted account");
    for align in [32usize, 64, 256, 4096, 1 << 16, 1 << 20] {
        for size in [1, align - 1, align, align + 1, 3 * align + 5] {
            let layout = Layout::from_size_align(size, align).unwrap();
            let block = alloc_on(&account, layout);
            assert!(!block.is_null(), "{size} at {align}");
            assert_eq!(block as usize % align, 0, "{size} at {align}");
            assert_eq!(account.bytes(), Some(before + align + size));
            // SAFETY: a live block of `size` bytes.
            unsafe { block.write_bytes(0xA5, size) };

            let grown_size = 3 * size + 7;
            let grown = {
                let _elsewhere = other.enter();
                // SAFETY: `block` is live with `layout`; the new size is not
                // zero and fits a layout of the same alignment.
                black_box(unsafe { std::alloc::realloc(block, layout, grown_size) })
            };
            assert!(!grown.is_null());
            assert_eq!(grown as usize % align, 0, "grown {size} at {align}");
            // SAFETY: the first `size` bytes of the grown block are the old
            // block's.
            assert!((0..size).all(|at| unsafe { *grown.add(at) } == 0xA5));
            assert_eq!(account.bytes(), Some(before + align + grown_size));
            assert_eq!(other.bytes(), Some(other_before));

            let grown_layout = Layout::from_size_align(grown_size, align).unwrap();
            // SAFETY: `grown` is live with `grown_layout`; 1 is not zero.
            let shrunk = black_box(unsafe { std::alloc::realloc(grown, grown_layout, 1) });
            assert!(!shrunk.is_null());
            assert_eq!(shrunk as usize % align, 0, "shrunk {size} at {align}");
            // SAFETY: a live block of at least one byte.
            assert_eq!(unsafe { *shrunk }, 0xA5);
            assert_eq!(account.bytes(), Some(before + align + 1));
            // SAFETY: `shrunk` is live with this layout.
            unsafe { std::alloc::dealloc(shrunk, Layout::from_size_align(1, align).unwrap()) };
            assert_eq!(account.bytes(), Some(before));
        }

        let layout = Layout::from_size_align(align + 3, align).unwrap();
        let zeroed = {
            let _entered = account.enter();
            // SAFETY: a non-zero size.
            black_box(unsafe { std::alloc::alloc_zeroed(layout) })
        };
        assert!(!zeroed.is_null());
        assert_eq!(zeroed as usize % align, 0);
        // SAFETY: a live block of `align + 3` bytes.
        assert!((0..align + 3).all(|at| unsafe { *zeroed.add(at) } == 0));
        assert_eq!(account.bytes(), Some(before + align + align + 3));
        // SAFETY: `zeroed` is live with `layout`.
        unsafe { std::alloc::dealloc(zeroed, layout) };
        assert_eq!(account.bytes(), Some(before));
    }
}

/// A size that leaves no room for the header, or rounds up past what a
/// layout can hold at the block's alignment, is refused before the system
/// is asked; one the system cannot give is refused by it. Neither changes a
/// count, and a block that cannot grow is left as it was.
#[test]
fn sizes_no_layout_or_system_can_hold() {
    let account = Account::new();
    let before = account.bytes();
    let max = isize::MAX as usize;
    let half = max / 2 + 1;
    let refused = [
        (max - HEADER + 1, 1),
        (max - 15, 16),
        (max - 63, 64),
        (1, half),
    ];
    for (size, align) in refused {
        let layout = Layout::from_size_align(size, align).unwrap();
        let _entered = account.enter();
        // SAFETY: non-zero sizes.
        assert!(
            unsafe { std::alloc::alloc(layout) }.is_null(),
            "{size} at {align}"
        );
        // SAFETY: as above.
        assert!(
            unsafe { std::alloc::alloc_zeroed(layout) }.is_null(),
            "{size} at {align}"
        );
        // SAFETY: as above; called directly.
        assert!(
            unsafe { Metered.alloc(layout) }.is_null(),
            "{size} at {align}"
        );
    }
    assert_eq!(account.bytes(), before);

    for align in [1usize, 16, 64] {
        let layout = Layout::from_size_align(10, align).unwrap();
        let block = alloc_on(&account, layout);
        assert!(!block.is_null());
        // SAFETY: a live block of 10 bytes.
        unsafe { block.write_bytes(7, 10) };
        let with = account.bytes();
        // A size realloc's contract allows (at most isize::MAX once rounded
        // to the alignment) that the header pushes past it.
        let in_front = align.max(HEADER);
        let new_size = (max - in_front + 1) / align * align;
        assert!(new_size > max - in_front);
        // SAFETY: `block` is live with `layout`; `new_size` is not zero and
        // is at most isize::MAX at this alignment.
        assert!(
            unsafe { std::alloc::realloc(block, layout, new_size) }.is_null(),
            "{new_size} at {align}"
        );
        #[cfg(not(miri))]
        {
            // More than any system gives.
            // SAFETY: as above.
            assert!(unsafe { std::alloc::realloc(block, layout, 1 << 46) }.is_null());
        }
        assert_eq!(account.bytes(), with);
        // SAFETY: the block is still live and its bytes unchanged.
        assert!((0..10).all(|at| unsafe { *block.add(at) } == 7));
        // SAFETY: `block` is live with `layout`.
        unsafe { std::alloc::dealloc(block, layout) };
    }
    #[cfg(not(miri))]
    {
        let layout = Layout::from_size_align(1 << 46, 16).unwrap();
        let _entered = account.enter();
        // SAFETY: a non-zero size.
        assert!(unsafe { std::alloc::alloc(layout) }.is_null());
    }
    assert_eq!(account.bytes(), before);
}

/// Values of no size never reach the allocator. GlobalAlloc's contract
/// keeps its callers from asking for zero bytes; called directly with a
/// zero size anyway, this allocator makes a block of the header alone,
/// aligned and counted, and refuses an alignment no layout can hold with
/// the header.
#[test]
fn zero_sizes() {
    let account = Account::new();
    let before = account.bytes().expect("a counted account");
    {
        let _entered = account.enter();
        let unit = black_box(Box::new(()));
        let units: Vec<()> = black_box(Vec::with_capacity(1000));
        let none: Vec<u64> = black_box(Vec::new());
        let empty = black_box(String::with_capacity(0));
        let boxed: Box<[u8]> = black_box(Vec::new().into_boxed_slice());
        assert_eq!(account.bytes(), Some(before));
        drop((unit, units, none, empty, boxed));
    }
    assert_eq!(account.bytes(), Some(before));

    for align in [1usize, 8, 16, 64, 4096] {
        let layout = Layout::from_size_align(0, align).unwrap();
        let block = {
            let _entered = account.enter();
            // SAFETY: Metered itself accepts a zero size (only the trait's
            // callers are asked not to pass one).
            black_box(unsafe { Metered.alloc(layout) })
        };
        assert!(!block.is_null());
        assert_eq!(block as usize % align, 0);
        assert_eq!(account.bytes(), Some(before + align.max(HEADER)));
        // SAFETY: `block` came from Metered.alloc with `layout`.
        unsafe { Metered.dealloc(block, layout) };
        assert_eq!(account.bytes(), Some(before));
    }
    let widest = Layout::from_size_align(0, isize::MAX as usize + 1).unwrap();
    // SAFETY: as above.
    assert!(unsafe { Metered.alloc(widest) }.is_null());
    // SAFETY: as above.
    assert!(unsafe { Metered.alloc_zeroed(widest) }.is_null());
    assert_eq!(account.bytes(), Some(before));
}

/// A block leaves the account it was made on wherever it is freed: on
/// another thread inside another account, aligned to the header or past it,
/// grown there first.
#[test]
fn blocks_freed_on_another_thread() {
    let account = Account::new();
    let other = Account::new();
    let before = account.bytes().expect("a counted account");
    let other_before = other.bytes().expect("a counted account");
    let (mut bytes, wide) = {
        let _entered = account.enter();
        (
            black_box(vec![1u8; 100]),
            black_box(Box::new(Wide([2; 64]))),
        )
    };
    assert_eq!(account.bytes(), Some(before + 100 + HEADER + 64 + 64));
    std::thread::scope(|scope| {
        let other = &other;
        scope.spawn(move || {
            let _elsewhere = other.enter();
            bytes.reserve_exact(10_000);
            assert_eq!(other.bytes(), Some(other_before));
            drop(bytes);
            assert_eq!(wide.0, [2; 64]);
            drop(wide);
        });
    });
    assert_eq!(account.bytes(), Some(before));
    assert_eq!(other.bytes(), Some(other_before));
}

static LATE: OnceLock<Account> = OnceLock::new();
static KEPT: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// Allocates on [`LATE`] while its thread's locals are destroyed.
struct AtExit;

impl Drop for AtExit {
    fn drop(&mut self) {
        let account = LATE.get().expect("the test's account");
        let block = {
            let _entered = account.enter();
            black_box(vec![7u8; 333])
        };
        KEPT.lock().unwrap().push(block);
    }
}

thread_local! {
    static ON_EXIT: AtExit = const { AtExit };
}

/// The thread-local the allocator reads has no destructor, so a destructor
/// of another thread-local, run as its thread ends, still enters an
/// account and allocates on it.
#[test]
fn allocations_while_a_thread_ends() {
    let account = LATE.get_or_init(Account::new);
    let before = account.bytes().expect("a counted account");
    std::thread::spawn(|| ON_EXIT.with(|_| ()))
        .join()
        .expect("the thread");
    assert_eq!(account.bytes(), Some(before + 333 + HEADER));
    KEPT.lock().unwrap().clear();
    assert_eq!(account.bytes(), Some(before));
}
