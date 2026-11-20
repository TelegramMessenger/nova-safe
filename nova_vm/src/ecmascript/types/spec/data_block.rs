// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ### [6.2.9 Data Blocks](https://tc39.es/ecma262/#sec-data-blocks)

use crate::width::usize_from_u64;
#[cfg(feature = "shared-array-buffer")]
use std::{
    collections::hash_map::Entry,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, OnceLock, WaitTimeoutResult,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering as RustOrdering},
    },
    time::Duration,
};

use core::ops::{Deref, DerefMut};
use std::ops::{BitAnd, BitOr, BitXor};

use num_bigint::Sign;

#[cfg_attr(not(feature = "shared-array-buffer"), allow(dead_code))]
mod shared_memory;
pub use shared_memory::Ordering;
#[cfg(feature = "shared-array-buffer")]
pub(crate) use shared_memory::SharedSlice;

use crate::{
    ecmascript::{
        Agent, BigInt, ExceptionType, JsResult, Number, Numeric, Value, to_big_int64_big_int,
        to_big_uint64_big_int, to_int8_number, to_int16_number, to_int32_number,
        to_uint8_clamp_number, to_uint8_number, to_uint16_number, to_uint32_number,
        truncate_modulo_2_64,
    },
    engine::{NoGcScope, trivially_bindable},
    heap::ArenaAccess,
};

#[cfg(feature = "array-buffer")]
use crate::ecmascript::execution::ProtoIntrinsics;

/// # [Data Block](https://tc39.es/ecma262/#sec-data-blocks)
///
/// The Data Block specification type is used to describe a distinct and
/// mutable sequence of byte-sized (8 bit) numeric values. A byte value is an
/// integer in the inclusive interval from 0 to 255. A Data Block value is
/// created with a fixed number of bytes that each have the initial value 0.
///
/// The bytes are owned by a boxed slice whose length is the byte length of
/// the block. The bytes are None if the block is detached.
pub(crate) struct DataBlock {
    bytes: Option<Box<[u8]>>,
}
trivially_bindable!(DataBlock);

impl core::fmt::Debug for DataBlock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.bytes {
            Some(bytes) => bytes.fmt(f),
            None => f.write_str("<detached>"),
        }
    }
}

impl Deref for DataBlock {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        match &self.bytes {
            Some(bytes) => bytes,
            None => &[],
        }
    }
}

impl DerefMut for DataBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match &mut self.bytes {
            Some(bytes) => bytes,
            None => &mut [],
        }
    }
}

impl DataBlock {
    /// A detached DataBlock.
    pub(crate) const DETACHED_DATA_BLOCK: DataBlock = DataBlock { bytes: None };

    /// An empty DataBlock.
    fn empty() -> Self {
        Self {
            bytes: Some(Box::default()),
        }
    }

    #[inline(always)]
    pub(crate) fn is_detached(&self) -> bool {
        self.bytes.is_none()
    }

    /// The bytes of the block, none of a detached one: what dereferencing
    /// it gives, as part of whoever asks.
    #[inline(always)]
    pub(crate) fn bytes(&self) -> &[u8] {
        match &self.bytes {
            Some(bytes) => bytes,
            None => &[],
        }
    }

    /// [`DataBlock::bytes`], to write them.
    #[inline(always)]
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        match &mut self.bytes {
            Some(bytes) => bytes,
            None => &mut [],
        }
    }

    /// Allocate a new zeroed DataBlock with the given size.
    ///
    /// Returns None if the allocation is impossible.
    fn new(len: usize) -> Option<Self> {
        /// Blocks larger than this are probed before they are allocated. A
        /// smaller allocation fails only when the process is out of memory,
        /// and then any other allocation would abort it as well.
        const PROBED_LEN: usize = 1 << 20;
        if len == 0 {
            return Some(Self::empty());
        }
        if len > PROBED_LEN {
            // Probe that the allocation is possible: a failed probe is
            // reported to the caller as a RangeError, while the zeroed
            // allocation below would abort the process. For blocks this large
            // the probe's map and unmap cost little next to the pages the
            // block uses.
            let mut probe: Vec<u8> = Vec::new();
            probe.try_reserve_exact(len).ok()?;
            drop(probe);
        }
        // The zeroed allocation lets the allocator hand out lazily zeroed
        // pages for large blocks.
        Some(Self {
            bytes: Some(vec![0u8; len].into_boxed_slice()),
        })
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.bytes.as_ref().map_or(0, |bytes| bytes.len())
    }

    /// Get the bytes of a T at `byte_offset`.
    ///
    /// Panics if the bytes are not within the block.
    #[inline(always)]
    fn element_bytes<T: Viewable>(&self, byte_offset: usize) -> &[u8] {
        let end = byte_offset
            .checked_add(core::mem::size_of::<T>())
            .expect("DataBlock byte offset overflows");
        &self[byte_offset..end]
    }

    /// Read the T at element index `offset`, or None if it is not within the
    /// block.
    #[allow(unused)]
    pub(crate) fn get<T: Viewable>(&self, offset: usize) -> Option<T> {
        let size = core::mem::size_of::<T>();
        let byte_offset = offset.checked_mul(size)?;
        let end = byte_offset.checked_add(size)?;
        <[u8]>::get(self, byte_offset..end).map(T::read_ne)
    }

    /// Read a T in native byte order from the buffer at `byte_offset`.
    ///
    /// ## Correctness
    ///
    /// The buffer must have enough room to read a T at `byte_offset`, else
    /// this panics.
    #[inline(always)]
    pub(crate) fn read_aligned<T: Viewable>(&self, byte_offset: usize) -> T {
        T::read_ne(self.element_bytes::<T>(byte_offset))
    }

    /// Read a T in native byte order from the buffer at `byte_offset`.
    ///
    /// ## Correctness
    ///
    /// The buffer must have enough room to read a T at `byte_offset`, else
    /// this panics.
    #[inline(always)]
    pub(crate) fn read_unaligned<T: Viewable>(&self, byte_offset: usize) -> T {
        T::read_ne(self.element_bytes::<T>(byte_offset))
    }

    /// Write the T at element index `offset`; does nothing if it is not
    /// within the block.
    #[allow(unused)]
    pub(crate) fn set<T: Viewable>(&mut self, offset: usize, value: T) {
        let size = core::mem::size_of::<T>();
        let Some(byte_offset) = offset.checked_mul(size) else {
            return;
        };
        let Some(end) = byte_offset.checked_add(size) else {
            return;
        };
        if let Some(bytes) = self.get_mut(byte_offset..end) {
            value.write_ne(bytes);
        }
    }

    /// Write a T in native byte order at `byte_offset`; does nothing if it is
    /// not within the block.
    pub(crate) fn set_offset_by_byte<T: Viewable>(&mut self, byte_offset: usize, value: T) {
        let Some(end) = byte_offset.checked_add(core::mem::size_of::<T>()) else {
            return;
        };
        if let Some(bytes) = self.get_mut(byte_offset..end) {
            value.write_ne(bytes);
        }
    }

    /// Copy `count` elements of T from `src` at element index `src_offset`
    /// to this block at element index `dst_offset`.
    ///
    /// Panics if either range is not within its block.
    #[allow(unused)]
    pub(crate) fn set_from<T: Viewable>(
        &mut self,
        dst_offset: usize,
        src: &DataBlock,
        src_offset: usize,
        count: usize,
    ) {
        let size = core::mem::size_of::<T>();
        let byte_length = count * size;
        if byte_length == 0 {
            return;
        }
        let dst_byte_offset = dst_offset * size;
        let src_byte_offset = src_offset * size;
        self[dst_byte_offset..dst_byte_offset + byte_length]
            .copy_from_slice(&src[src_byte_offset..src_byte_offset + byte_length]);
    }

    /// Copy `count` elements of T within this block from element index
    /// `src_offset` to element index `dst_offset`.
    ///
    /// Panics if either range is not within the block.
    #[allow(unused)]
    pub(crate) fn copy_within<T: Viewable>(
        &mut self,
        dst_offset: usize,
        src_offset: usize,
        count: usize,
    ) {
        let size = core::mem::size_of::<T>();
        let byte_length = count * size;
        if byte_length == 0 {
            return;
        }
        let dst_byte_offset = dst_offset * size;
        let src_byte_offset = src_offset * size;
        <[u8]>::copy_within(
            self,
            src_byte_offset..(src_byte_offset + byte_length),
            dst_byte_offset,
        );
    }

    /// Resize the block to `new_byte_length` bytes; new bytes are zero.
    ///
    /// A detached block stays detached.
    pub(crate) fn realloc(&mut self, new_byte_length: usize) {
        // Max byte length should be within safe integer length.
        debug_assert!((new_byte_length as u64) < 2u64.pow(53));
        let Some(bytes) = self.bytes.take() else {
            // Detached.
            return;
        };
        if new_byte_length == 0 {
            *self = Self::empty();
            return;
        }
        let mut bytes = bytes.into_vec();
        if new_byte_length > bytes.len() {
            bytes.reserve_exact(new_byte_length - bytes.len());
        }
        bytes.resize(new_byte_length, 0);
        self.bytes = Some(bytes.into_boxed_slice());
    }
}

/// Maximum byte length of a SharedArrayBuffer.
///
/// If the top bit is set, then this is a growable SharedDataBlock, and the
/// byte length of the SharedArrayBuffer is stored in the buffer allocation.
/// Note that growable SharedDataBlocks can still be dangling; in this case
/// their maximum byte length has only the top bit set, ie. they have a
/// maximum byte length value of zero.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "shared-array-buffer")]
struct SharedDataBlockMaxByteLength(usize);

#[cfg(feature = "shared-array-buffer")]
impl SharedDataBlockMaxByteLength {
    /// Create a new max byte length value for a possibly growable SharedDataBlock.
    #[inline(always)]
    fn new(size: usize, growable: bool) -> Self {
        if growable {
            // The top bit of a usize, whatever its width.
            Self(size | !(usize::MAX >> 1))
        } else {
            Self(size)
        }
    }

    /// Get the maximum byte length value of a SharedDataBlock.
    ///
    /// Note that the maximum byte length of a growable SharedArrayBuffer or
    /// the byte length of an static SharedArrayBuffer.
    #[inline(always)]
    fn max_byte_length(self) -> usize {
        self.0 & (usize::MAX >> 1)
    }

    /// Returns true if the SharedDataBlock has a 0 maximum byte length.
    #[inline(always)]
    fn is_dangling(self) -> bool {
        self.max_byte_length() == 0
    }

    /// Returns true if the SharedDataBlock is growable.
    #[inline(always)]
    fn is_growable(self) -> bool {
        self.0.leading_ones() > 0
    }
}

#[cfg(feature = "shared-array-buffer")]
#[derive(Debug)]
pub(crate) struct WaiterRecord {
    condvar: Condvar,
    notified: AtomicBool,
    result: AtomicU8,
}

#[cfg(feature = "shared-array-buffer")]
impl Default for WaiterRecord {
    fn default() -> Self {
        Self {
            condvar: Condvar::default(),
            notified: AtomicBool::default(),
            result: AtomicU8::new(u8::MAX),
        }
    }
}

#[cfg(feature = "shared-array-buffer")]
impl WaiterRecord {
    pub(crate) fn new_shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn notify_waiters(self: Arc<Self>) {
        self.notified.store(true, RustOrdering::Relaxed);
        self.condvar.notify_all();
    }

    pub(crate) fn wait<'a, T>(self: &Arc<Self>, guard: MutexGuard<'a, T>) {
        let lock_result = self
            .condvar
            .wait_while(guard, |_| !self.notified.load(RustOrdering::Relaxed));

        match lock_result {
            Ok(_) => (),
            Err(e) => panic!(
                "Another thread panicked while holding the waiter list lock, poisoning it: {e:?}"
            ),
        }
    }

    pub(crate) fn wait_timeout<'a, T>(
        self: &Arc<Self>,
        guard: MutexGuard<'a, T>,
        dur: Duration,
    ) -> (MutexGuard<'a, T>, WaitTimeoutResult) {
        let lock_result = self
            .condvar
            .wait_timeout_while(guard, dur, |_| !self.notified.load(RustOrdering::Relaxed));

        match lock_result {
            Ok(result) => result,
            Err(e) => panic!(
                "Another thread panicked while holding the waiter list lock, poisoning it: {e:?}"
            ),
        }
    }

    pub(crate) fn is_notified(&self) -> bool {
        self.notified.load(RustOrdering::Relaxed)
    }

    pub(crate) fn set_result(&self, result: WaitResult) {
        self.result.store(result as u8, RustOrdering::Relaxed);
    }

    pub(crate) fn get_result(&self) -> Option<WaitResult> {
        match self.result.load(RustOrdering::Relaxed) {
            0 => Some(WaitResult::Ok),
            1 => Some(WaitResult::TimedOut),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "shared-array-buffer")]
#[repr(u8)]
pub(crate) enum WaitResult {
    Ok = 0,
    TimedOut = 1,
}

#[cfg(feature = "shared-array-buffer")]
impl WaitResult {
    pub(crate) fn to_string(self) -> crate::ecmascript::String<'static> {
        match self {
            WaitResult::Ok => crate::ecmascript::BUILTIN_STRING_MEMORY.ok,
            WaitResult::TimedOut => crate::ecmascript::BUILTIN_STRING_MEMORY.timed_out,
        }
    }
}

#[cfg(feature = "shared-array-buffer")]
#[derive(Default)]
#[repr(transparent)]
pub(crate) struct WaiterList {
    waiters: std::collections::VecDeque<Arc<WaiterRecord>>,
}

#[cfg(feature = "shared-array-buffer")]
impl WaiterList {
    pub(crate) fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }

    pub(crate) fn pop(&mut self) -> Option<Arc<WaiterRecord>> {
        self.waiters.pop_front()
    }

    pub(crate) fn push(&mut self, w: Arc<WaiterRecord>) {
        self.waiters.push_back(w);
    }

    pub(crate) fn remove(&mut self, w: &Arc<WaiterRecord>) -> bool {
        let Some(index) = self
            .waiters
            .iter()
            .enumerate()
            .find(|(_, e)| Arc::ptr_eq(e, w))
            .map(|(i, _)| i)
        else {
            return false;
        };
        self.waiters.remove(index);
        true
    }
}

#[cfg(feature = "shared-array-buffer")]
#[repr(transparent)]
#[derive(Default)]
pub(crate) struct WaiterLists {
    map: std::collections::HashMap<usize, WaiterList>,
}

#[cfg(feature = "shared-array-buffer")]
impl WaiterLists {
    pub(crate) fn get_list_mut(&mut self, index: usize) -> Option<&mut WaiterList> {
        self.map.get_mut(&index)
    }

    pub(crate) fn push_to_list(&mut self, index: usize, w: Arc<WaiterRecord>) {
        self.map.entry(index).or_default().push(w);
    }

    pub(crate) fn remove_from_list(&mut self, index: usize, w: &Arc<WaiterRecord>) {
        match self.map.entry(index) {
            Entry::Occupied(mut entry) => {
                if entry.get_mut().remove(w) && entry.get().is_empty() {
                    entry.remove();
                }
            }
            Entry::Vacant(_) => {}
        }
    }
}

/// ### [6.2.9 Data Blocks](https://tc39.es/ecma262/#sec-data-blocks)
///
/// The Shared Data Block specification type is used to describe a distinct and
/// mutable sequence of byte-sized (8 bit) racy atomic memory values. A byte
/// value is an integer in the inclusive interval from 0 to 255. A Shared Data
/// Block value is created with a fixed number of bytes that each have the
/// initial value 0.
///
/// The bytes live in a reference counted allocation of atomic words shared by
/// every clone of the block, together with the byte length of a growable block
/// and the lazily created waiter lists of `Atomics.wait` and
/// `Atomics.waitAsync`. A growable block allocates its maximum byte length up
/// front, so growing it only raises the shared byte length and every agent
/// sharing the block sees the growth. A block with a maximum byte length of
/// zero has no allocation.
///
/// ## Memory model
///
/// The [ECMAScript memory model](https://tc39.es/ecma262/#sec-memory-model) is
/// sequentially consistent when the program as no data races, but data races
/// are allowed by the model. Rust decrees data races on plain memory to be
/// undefined behaviour, so the bytes are only ever accessed through atomic
/// operations on their words; see [`SharedSlice`].
#[must_use]
#[derive(Clone)]
#[cfg(feature = "shared-array-buffer")]
pub struct SharedDataBlock {
    inner: Option<Arc<SharedDataBlockInner>>,
    max_byte_length: SharedDataBlockMaxByteLength,
}

#[cfg(feature = "shared-array-buffer")]
struct SharedDataBlockInner {
    words: Box<[AtomicU64]>,
    /// The current byte length of a growable block.
    byte_length: AtomicUsize,
    waiters: OnceLock<Mutex<WaiterLists>>,
}

#[cfg(feature = "shared-array-buffer")]
impl PartialEq for SharedDataBlock {
    fn eq(&self, other: &Self) -> bool {
        self.max_byte_length == other.max_byte_length
            && match (&self.inner, &other.inner) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

#[cfg(feature = "shared-array-buffer")]
impl Eq for SharedDataBlock {}

#[cfg(feature = "shared-array-buffer")]
impl core::default::Default for SharedDataBlock {
    fn default() -> Self {
        Self::DANGLING_STATIC_SHARED_DATA_BLOCK
    }
}

#[cfg(feature = "shared-array-buffer")]
impl SharedDataBlock {
    const DANGLING_STATIC_SHARED_DATA_BLOCK: Self = Self {
        inner: None,
        max_byte_length: SharedDataBlockMaxByteLength(0),
    };

    const DANGLING_GROWABLE_SHARED_DATA_BLOCK: Self = Self {
        inner: None,
        max_byte_length: SharedDataBlockMaxByteLength(1usize.rotate_right(1)),
    };

    /// Allocate a new SharedDataBlock.
    ///
    /// ## Correctness
    ///
    /// `byte_length` is less or equal to `max_byte_length` if defined, else
    /// this panics.
    fn new(byte_length: usize, max_byte_length: Option<usize>) -> Option<Self> {
        let size = max_byte_length.unwrap_or(byte_length);
        assert!(byte_length <= size);
        let growable = max_byte_length.is_some();
        if size == 0 {
            if growable {
                Some(Self::DANGLING_GROWABLE_SHARED_DATA_BLOCK)
            } else {
                Some(Self::DANGLING_STATIC_SHARED_DATA_BLOCK)
            }
        } else {
            let words = shared_memory::allocate_words(size)?;
            Some(Self {
                inner: Some(Arc::new(SharedDataBlockInner {
                    words,
                    byte_length: AtomicUsize::new(byte_length),
                    waiters: OnceLock::new(),
                })),
                max_byte_length: SharedDataBlockMaxByteLength::new(size, growable),
            })
        }
    }

    /// Get a racy memory slice of the bytes of the SharedDataBlock.
    pub(crate) fn as_racy_slice(&self) -> SharedSlice<'_, u8> {
        let words = self
            .inner
            .as_ref()
            .map_or(&[][..], |inner| &inner.words[..]);
        SharedSlice::from_words(words, self.byte_length(Ordering::Unordered))
    }

    /// Returns the byte length of the SharedArrayBuffer.
    ///
    /// Note that if this is a growable SharedArrayBuffer, then the byte length
    /// can be grown from other threads and reading it is a sequentially
    /// consistent atomic operation.
    #[inline(always)]
    pub(crate) fn byte_length(&self, order: Ordering) -> usize {
        let Some(inner) = &self.inner else {
            return 0;
        };
        if self.is_growable() {
            inner.byte_length.load(match order {
                Ordering::Unordered => std::sync::atomic::Ordering::Relaxed,
                Ordering::SeqCst => std::sync::atomic::Ordering::SeqCst,
            })
        } else {
            self.max_byte_length()
        }
    }

    /// Get the maximum byte length of the SharedDataBlock.
    #[inline(always)]
    pub(crate) fn max_byte_length(&self) -> usize {
        self.max_byte_length.max_byte_length()
    }

    /// Return true if the SharedDataBlock is a dangling (has maximum byte
    /// length equal to zero).
    #[inline(always)]
    pub(crate) fn is_dangling(&self) -> bool {
        self.max_byte_length.is_dangling()
    }

    /// Returns true if the SharedDataBlock is growable.
    #[inline(always)]
    pub(crate) fn is_growable(&self) -> bool {
        self.max_byte_length.is_growable()
    }

    /// Get or lazily initialize the shared waiter map for this data block.
    ///
    /// ## Correctness
    ///
    /// Must not be a dangling SharedDataBlock, else this panics.
    pub(crate) fn get_or_init_waiters(&self) -> &Mutex<WaiterLists> {
        self.inner
            .as_ref()
            .expect("a dangling SharedDataBlock has no waiters")
            .waiters
            .get_or_init(Mutex::default)
    }

    /// Get the shared waiter map if it has been initialized.
    ///
    /// Returns `None` if no thread has ever called `get_or_init_waiters` on
    /// this block (i.e. no `Atomics.wait` / `Atomics.waitAsync` has occurred),
    /// or if the block is dangling.
    pub(crate) fn get_waiters(&self) -> Option<&Mutex<WaiterLists>> {
        self.inner.as_ref()?.waiters.get()
    }

    /// Read a value at the given aligned offset and with the given ordering.
    ///
    /// Returns `None` if the offset is not correctly aligned or the index is
    /// out of bounds.
    #[inline(always)]
    pub(crate) fn load<T: Viewable>(&self, byte_offset: usize, order: Ordering) -> Option<T> {
        let slice = self.as_racy_slice().slice_from(byte_offset);
        slice
            .as_aligned::<T::Storage>()
            .map(|t| T::from_storage(t.load(order)))
    }

    /// Read a value at the given possibly unaligned offset with no
    /// synchronisation. This read may tear.
    ///
    /// Returns `None` if the index is out of bounds.
    #[inline(always)]
    pub(crate) fn load_unaligned<T: Viewable>(&self, byte_offset: usize) -> Option<T> {
        let slice = self.as_racy_slice().slice_from(byte_offset);
        slice.load_unaligned::<T::Storage>().map(T::from_storage)
    }

    /// Write a value at the given offset and the given ordering.
    ///
    /// Returns `None` if the offset is not correctly aligned or the index is
    /// out of bounds.
    pub(crate) fn store<T: Viewable>(
        &self,
        byte_offset: usize,
        val: T,
        order: Ordering,
    ) -> Option<()> {
        let slice = self.as_racy_slice().slice_from(byte_offset);
        slice
            .as_aligned::<T::Storage>()
            .map(|t| t.store(T::into_storage(val), order))
    }

    /// Write a value at the given possibly unaligned offset with no
    /// synchronisation. This write may tear.
    ///
    /// Returns `None` if the index is out of bounds.
    pub(crate) fn store_unaligned<T: Viewable>(&self, byte_offset: usize, val: T) -> Option<()> {
        let slice = self.as_racy_slice().slice_from(byte_offset);
        slice.store_unaligned::<T::Storage>(T::into_storage(val))
    }

    /// Attempt to grow the SharedDataBlock. Returns false if `new_byte_length`
    /// is or becomes less than the current byte length.
    ///
    /// Note that this is a synchronising compare-and-exchange loop.
    ///
    /// ## Correctness
    ///
    /// Must be a growable, non-dangling SharedDataBlock with a maximum byte
    /// length greater or equal to `new_byte_length`, else this panics.
    pub(crate) fn grow(&self, new_byte_length: usize) -> bool {
        use std::sync::atomic::Ordering as RustOrdering;
        assert!(self.is_growable());
        assert!(new_byte_length <= self.max_byte_length());
        let byte_length = &self
            .inner
            .as_ref()
            .expect("a dangling SharedDataBlock cannot grow")
            .byte_length;
        // 9. Let currentByteLengthRawBytes be
        //    GetRawBytesFromSharedBlock(byteLengthBlock, 0, biguint64, true, seq-cst).
        let mut current_byte_length = byte_length.load(RustOrdering::SeqCst);
        loop {
            // c. If newByteLength = currentByteLength,
            if new_byte_length == current_byte_length {
                // return undefined.
                return true;
            }
            // d. If newByteLength < currentByteLength or
            //    newByteLength > O.[[ArrayBufferMaxByteLength]],
            //    throw a RangeError exception.
            if new_byte_length < current_byte_length {
                return false;
            }
            // h. Let readByteLengthRawBytes be
            //    AtomicCompareExchangeInSharedBlock(byteLengthBlock, 0, 8,
            //    currentByteLengthRawBytes, newByteLengthRawBytes).
            let Err(read_byte_length) = byte_length.compare_exchange(
                current_byte_length,
                new_byte_length,
                RustOrdering::SeqCst,
                RustOrdering::SeqCst,
            ) else {
                // i. If ByteListEqual(readByteLengthRawBytes,
                //    currentByteLengthRawBytes) is true, return undefined.
                return true;
            };
            // j. Set currentByteLengthRawBytes to readByteLengthRawBytes.
            current_byte_length = read_byte_length
        }
    }

    /// Returns true if both blocks share the same bytes.
    fn shares_bytes_with(&self, other: &Self) -> bool {
        match (&self.inner, &other.inner) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

const DATA_BLOCK_SIZE_LIMIT: u64 = u64::pow(2, 53) - 1;

/// ### [6.2.9.1 CreateByteDataBlock ( size )](https://tc39.es/ecma262/#sec-createbytedatablock)
///
/// The abstract operation CreateByteDataBlock takes argument size (a
/// non-negative integer) and returns either a normal completion containing
/// a Data Block or a throw completion.
pub(crate) fn create_byte_data_block<'a>(
    agent: &mut Agent,
    size: u64,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, DataBlock> {
    agent.check_allocation(usize::try_from(size).unwrap_or(usize::MAX), gc)?;
    agent.heap.data_bytes_since_gc += usize_from_u64(size);
    // 1. If size > 2**53 - 1, throw a RangeError exception.
    if let Some(db) = usize::try_from(size)
        .ok()
        .filter(|&size| size as u64 <= DATA_BLOCK_SIZE_LIMIT)
        .and_then(DataBlock::new)
    {
        // 2. Let db be a new Data Block value consisting of size bytes.
        // 3. Set all of the bytes of db to 0.
        // 4. Return db.
        Ok(db)
    } else {
        // 2. cont: If it is impossible to create such a Data Block, throw a
        //    RangeError exception.
        Err(agent.throw_exception_with_static_message(
            ExceptionType::RangeError,
            "Failed to allocate ArrayBuffer",
            gc,
        ))
    }
}

/// ### [6.2.9.2 CreateSharedByteDataBlock ( size )](https://tc39.es/ecma262/#sec-createsharedbytedatablock)
///
/// The abstract operation CreateSharedByteDataBlock takes argument size (a
/// non-negative integer) and returns either a normal completion containing
/// a Shared Data Block or a throw completion.
///
/// ## Correctness
///
/// `byte_length` must be less or equal than `max_byte_length` if it has been
/// defined, else this panics.
#[cfg(feature = "shared-array-buffer")]
pub(crate) fn create_shared_byte_data_block<'a>(
    agent: &mut Agent,
    byte_length: u64,
    max_byte_length: Option<u64>,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, SharedDataBlock> {
    let size = max_byte_length.unwrap_or(byte_length);
    assert!(byte_length <= size);
    agent.check_allocation(usize::try_from(size).unwrap_or(usize::MAX), gc)?;
    agent.heap.data_bytes_since_gc += usize_from_u64(size);
    // 1. Let db be a new Shared Data Block value consisting of size bytes. If
    //    it is impossible to create such a Shared Data Block, throw a
    //    RangeError exception.
    if let Some(db) = usize::try_from(size)
        .ok()
        .filter(|&size| size as u64 <= DATA_BLOCK_SIZE_LIMIT)
        .and_then(|_| {
            SharedDataBlock::new(
                usize_from_u64(byte_length),
                max_byte_length.map(|s| usize_from_u64(s)),
            )
        })
    {
        // 2. Let execution be the [[CandidateExecution]] field of the
        //    surrounding agent's Agent Record.
        // 3. Let eventsRecord be the Agent Events Record of
        //    execution.[[EventsRecords]] whose [[AgentSignifier]] is
        //    AgentSignifier().
        // 4. Let zero be « 0 ».
        // 5. For each index i of db, do
        // a. Append WriteSharedMemory { [[Order]]: INIT, [[NoTear]]: true,
        //    [[Block]]: db, [[ByteIndex]]: i, [[ElementSize]]: 1,
        //    [[Payload]]: zero } to eventsRecord.[[EventList]].
        // 6. Return db.
        Ok(db)
    } else {
        // 2. cont: If it is impossible to create such a Data Block, throw a
        //    RangeError exception.
        Err(agent.throw_exception_with_static_message(
            ExceptionType::RangeError,
            "Failed to allocate ArrayBuffer",
            gc,
        ))
    }
}

/// ### [6.2.9.3 CopyDataBlockBytes ( toBlock, toIndex, fromBlock, fromIndex, count )](https://tc39.es/ecma262/#sec-copydatablockbytes)
///
/// The abstract operation CopyDataBlockBytes takes arguments toBlock (a
/// Data Block or a Shared Data Block), toIndex (a non-negative integer),
/// fromBlock (a Data Block or a Shared Data Block), fromIndex (a
/// non-negative integer), and count (a non-negative integer) and returns
/// UNUSED.
pub(crate) fn copy_data_block_bytes(
    to_block: &mut DataBlock,
    to_index: usize,
    from_block: &DataBlock,
    from_index: usize,
    count: usize,
) {
    // 1. Assert: fromBlock and toBlock are distinct values.
    // NOTE: An exclusive and a shared reference cannot alias.
    // 2. Let fromSize be the number of bytes in fromBlock.
    let from_size = from_block.len();
    // 3. Assert: fromIndex + count ≤ fromSize.
    assert!(from_index + count <= from_size);
    // 4. Let toSize be the number of bytes in toBlock.
    let to_size = to_block.len();
    // 5. Assert: toIndex + count ≤ toSize.
    assert!(to_index + count <= to_size);
    // 6. Repeat, while count > 0,
    // i. Assert: toBlock is not a Shared Data Block.
    // ii. Set toBlock[toIndex] to fromBlock[fromIndex].
    // c. Set toIndex to toIndex + 1.
    // d. Set fromIndex to fromIndex + 1.
    // e. Set count to count - 1.
    to_block[to_index..to_index + count]
        .copy_from_slice(&from_block[from_index..from_index + count]);
    // 7. Return UNUSED.
}

/// ### [6.2.9.3 CopyDataBlockBytes ( toBlock, toIndex, fromBlock, fromIndex, count )](https://tc39.es/ecma262/#sec-copydatablockbytes)
///
/// The abstract operation CopyDataBlockBytes takes arguments toBlock (a
/// Data Block or a Shared Data Block), toIndex (a non-negative integer),
/// fromBlock (a Data Block or a Shared Data Block), fromIndex (a
/// non-negative integer), and count (a non-negative integer) and returns
/// UNUSED.
#[cfg(feature = "shared-array-buffer")]
pub(crate) fn copy_shared_data_block_bytes(
    to_block: &SharedDataBlock,
    to_index: usize,
    from_block: &SharedDataBlock,
    from_index: usize,
    count: usize,
) {
    // 1. Assert: fromBlock and toBlock are distinct values.
    debug_assert!(!to_block.shares_bytes_with(from_block));
    // 2. Let fromSize be the number of bytes in fromBlock.
    let from_size = from_block.max_byte_length();
    // 3. Assert: fromIndex + count ≤ fromSize.
    assert!(from_index + count <= from_size);
    // 4. Let toSize be the number of bytes in toBlock.
    let to_size = to_block.max_byte_length();
    // 5. Assert: toIndex + count ≤ toSize.
    assert!(to_index + count <= to_size);
    // 6. Repeat, while count > 0,
    // a. If fromBlock is a Shared Data Block, then
    // i. Let execution be the [[CandidateExecution]] field of the surrounding agent's Agent Record.
    // ii. Let eventsRecord be the Agent Events Record of execution.[[EventsRecords]] whose [[AgentSignifier]] is AgentSignifier().
    // iii. Let bytes be a List whose sole element is a nondeterministically chosen byte value.
    // iv. NOTE: In implementations, bytes is the result of a non-atomic read instruction on the underlying hardware. The nondeterminism is a semantic prescription of the memory model to describe observable behaviour of hardware with weak consistency.
    // v. Let readEvent be ReadSharedMemory { [[Order]]: UNORDERED, [[NoTear]]: true, [[Block]]: fromBlock, [[ByteIndex]]: fromIndex, [[ElementSize]]: 1 }.
    // vi. Append readEvent to eventsRecord.[[EventList]].
    // vii. Append Chosen Value Record { [[Event]]: readEvent, [[ChosenValue]]: bytes } to execution.[[ChosenValues]].
    // viii. If toBlock is a Shared Data Block, then
    //     1. Append WriteSharedMemory { [[Order]]: UNORDERED, [[NoTear]]: true, [[Block]]: toBlock, [[ByteIndex]]: toIndex, [[ElementSize]]: 1, [[Payload]]: bytes } to eventsRecord.[[EventList]].
    // ix. Else,
    //     1. Set toBlock[toIndex] to bytes[0].
    // Note: the copy reads and writes the bytes with atomic word operations,
    // so data races caused by the JavaScript code are well defined.
    to_block
        .as_racy_slice()
        .slice(to_index, to_index + count)
        .copy_from_racy_slice(
            &from_block
                .as_racy_slice()
                .slice(from_index, from_index + count),
        );
    // 7. Return UNUSED.
}

/// Wrapper type for defining the clamping behaviour of [`Uint8ClampedArray`]s.
///
/// [`Uint8ClampedArray`]: crate::ecmascript::Uint8ClampedArray
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct U8Clamped(pub u8);

impl core::fmt::Debug for U8Clamped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

mod private {
    use super::U8Clamped;

    pub trait Sealed {}
    impl Sealed for () {}
    impl Sealed for u8 {}
    impl Sealed for U8Clamped {}
    impl Sealed for i8 {}
    impl Sealed for u16 {}
    impl Sealed for i16 {}
    impl Sealed for u32 {}
    impl Sealed for i32 {}
    impl Sealed for u64 {}
    impl Sealed for i64 {}
    #[cfg(feature = "proposal-float16array")]
    impl Sealed for f16 {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

/// Conversion of a Viewable storage type from and to its bytes in native
/// byte order.
pub(crate) trait StorageBytes: Copy + PartialEq {
    /// The bytes of a value, `[u8; size_of::<Self>()]`.
    type Bytes: Copy;

    /// Read the value from exactly `size_of::<Self>()` bytes.
    ///
    /// Panics if `bytes` has a different length.
    fn read_ne(bytes: &[u8]) -> Self;

    /// Write the value into exactly `size_of::<Self>()` bytes.
    ///
    /// Panics if `bytes` has a different length.
    fn write_ne(self, bytes: &mut [u8]);

    /// Size of the value in bytes.
    const SIZE: usize;

    /// The value zero-extended to 64 bits.
    fn into_u64(self) -> u64;

    /// The low `size_of::<Self>()` bytes of `bits`.
    fn from_u64(bits: u64) -> Self;

    /// The value of the bytes.
    fn from_ne_array(bytes: Self::Bytes) -> Self;

    /// The bytes of the value.
    fn to_ne_array(self) -> Self::Bytes;

    /// View bytes as values' bytes; trailing bytes that do not make up a
    /// whole value are left out.
    fn as_elements(bytes: &[u8]) -> &[Self::Bytes];

    /// View bytes as values' bytes; trailing bytes that do not make up a
    /// whole value are left out.
    fn as_elements_mut(bytes: &mut [u8]) -> &mut [Self::Bytes];

    /// The bytes of values' bytes.
    fn flatten_mut(elements: &mut [Self::Bytes]) -> &mut [u8];
}

macro_rules! storage_bytes {
    ($($t:ty),*) => {
        $(
            impl StorageBytes for $t {
                type Bytes = [u8; size_of::<$t>()];

                #[inline(always)]
                fn read_ne(bytes: &[u8]) -> Self {
                    Self::from_ne_bytes(
                        bytes
                            .try_into()
                            .expect("byte slice length does not match the element size"),
                    )
                }

                #[inline(always)]
                fn write_ne(self, bytes: &mut [u8]) {
                    bytes.copy_from_slice(&self.to_ne_bytes());
                }

                const SIZE: usize = core::mem::size_of::<$t>();

                #[inline(always)]
                fn into_u64(self) -> u64 {
                    self as u64
                }

                #[inline(always)]
                fn from_u64(bits: u64) -> Self {
                    bits as $t
                }

                #[inline(always)]
                fn from_ne_array(bytes: Self::Bytes) -> Self {
                    Self::from_ne_bytes(bytes)
                }

                #[inline(always)]
                fn to_ne_array(self) -> Self::Bytes {
                    self.to_ne_bytes()
                }

                #[inline(always)]
                fn as_elements(bytes: &[u8]) -> &[Self::Bytes] {
                    bytes.as_chunks().0
                }

                #[inline(always)]
                fn as_elements_mut(bytes: &mut [u8]) -> &mut [Self::Bytes] {
                    bytes.as_chunks_mut().0
                }

                #[inline(always)]
                fn flatten_mut(elements: &mut [Self::Bytes]) -> &mut [u8] {
                    elements.as_flattened_mut()
                }
            }
        )*
    };
}

storage_bytes!(u8, u16, u32, u64);

/// The bytes of a Viewable value, `[u8; size_of::<T>()]`.
pub(crate) type ElementBytes<T> = <<T as Viewable>::Storage as StorageBytes>::Bytes;

pub(crate) trait Viewable:
    'static + private::Sealed + Copy + PartialEq + core::fmt::Debug
{
    /// Type of the data in its storage format. This is used with
    /// SharedDataBlock.
    type Storage: StorageBytes;

    /// Read a value in native byte order from exactly `size_of::<Self>()`
    /// bytes.
    ///
    /// Panics if `bytes` has a different length.
    #[inline(always)]
    fn read_ne(bytes: &[u8]) -> Self {
        Self::from_storage(Self::Storage::read_ne(bytes))
    }

    /// Write a value in native byte order into exactly `size_of::<Self>()`
    /// bytes.
    ///
    /// Panics if `bytes` has a different length.
    #[inline(always)]
    fn write_ne(self, bytes: &mut [u8]) {
        Self::into_storage(self).write_ne(bytes)
    }

    /// The value of its bytes in native byte order.
    #[inline(always)]
    fn from_ne_array(bytes: ElementBytes<Self>) -> Self {
        Self::from_storage(Self::Storage::from_ne_array(bytes))
    }

    /// The bytes of the value in native byte order.
    #[inline(always)]
    fn to_ne_array(self) -> ElementBytes<Self> {
        Self::into_storage(self).to_ne_array()
    }

    /// Functions as the \[\[ContentType\]\] internal slot of the TypedArray and
    /// as a marker for data views. Used to determine that the viewable type is
    /// a BigInt.
    const IS_BIGINT: bool = false;
    const IS_FLOAT: bool = false;
    const NAME: &str;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics;

    fn into_be_value<'a>(self, agent: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a>;
    fn into_le_value<'a>(self, agent: &mut Agent, gc: NoGcScope<'a, '_>) -> Numeric<'a>;
    #[inline(always)]
    fn into_ne_value<'a>(self, agent: &mut Agent, gc: NoGcScope<'a, '_>) -> Numeric<'a> {
        if cfg!(target_endian = "little") {
            self.into_le_value(agent, gc)
        } else {
            self.into_be_value(agent, gc)
        }
    }
    fn from_le_value(agent: &Agent, value: Numeric) -> Self;
    fn from_be_value(agent: &Agent, value: Numeric) -> Self;
    #[inline(always)]
    fn from_ne_value(agent: &Agent, value: Numeric) -> Self {
        if cfg!(target_endian = "little") {
            Self::from_le_value(agent, value)
        } else {
            Self::from_be_value(agent, value)
        }
    }
    /// Try reinterpret a Value to Viewable.
    ///
    /// This method is intended for cases where the ECMAScript specification
    /// requires repeatedly converting existing Viewable values to Value and
    /// comparing them with a search element. In this case, the Viewable to
    /// Value conversion is lossless and the comparison function is the only
    /// place where some value coercion may happen; this is generally the -0.0
    /// value being coerced to 0.
    ///
    /// Thus, this method must not do conversion, rounding, or clamping of
    /// numeric values.
    fn try_from_value(agent: &mut Agent, value: Value) -> Option<Self>;
    fn default() -> Self;

    /// Convert a Viewable value into a u64 holding an integer.
    ///
    /// This is used to convert Viewables to other Viewables without having to
    /// go through a conversion into Value.
    fn into_bits(self) -> u64;
    /// Convert a u64 holding an integer into a Viewable.
    ///
    /// This is used to convert Viewables to other Viewables without having to
    /// go through a conversion into Value.
    fn from_bits(bits: u64) -> Self;

    fn into_storage(value: Self) -> Self::Storage;

    fn from_storage(value: Self::Storage) -> Self;

    /// Convert a Viewable value into an f64.
    ///
    /// This is used to convert Viewables to other Viewables without having to
    /// go through a conversion into Value.
    fn into_f64(self) -> f64;
    /// Convert an f64 into a Viewable.
    ///
    /// This is used to convert Viewables to other Viewables without having to
    /// go through a conversion into Value.
    fn from_f64(value: f64) -> Self;

    /// Reverses the byte order of the value.
    fn flip_endian(self) -> Self;

    fn neg(self) -> Self;
    fn add(self, other: Self) -> Self;
    fn and(self, other: Self) -> Self;
    fn swap(self, other: Self) -> Self;
    fn or(self, other: Self) -> Self;
    fn sub(self, other: Self) -> Self;
    fn xor(self, other: Self) -> Self;

    /// Compare A and B of a Viewable type and always return an Ordering.
    ///
    /// This ordering is the usual total order for integers, and the special
    /// ECMAScript defined ordering for floats.
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering;
}

impl Viewable for () {
    // Note: this is not a valid storage format for () really. VoidArrays never
    // get stored so it's okay.
    type Storage = u8;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Uint8Array;
    const NAME: &str = "VoidArray";

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        panic!("VoidArray is a marker type");
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        panic!("VoidArray is a marker type");
    }

    fn from_be_value(_: &Agent, _: Numeric) -> Self {
        panic!("VoidArray is a marker type");
    }

    fn from_le_value(_: &Agent, _: Numeric) -> Self {
        panic!("VoidArray is a marker type");
    }

    fn try_from_value(_: &mut Agent, _: Value) -> Option<Self> {
        panic!("VoidArray is a marker type");
    }

    fn default() -> Self {
        panic!("VoidArray is a marker type");
    }

    fn into_bits(self) -> u64 {
        panic!("VoidArray is a marker type");
    }

    fn from_bits(_: u64) -> Self {
        panic!("VoidArray is a marker type");
    }

    fn from_storage(_: Self::Storage) -> Self {
        panic!("VoidArray is a marker type");
    }

    fn into_storage(_: Self) -> Self::Storage {
        panic!("VoidArray is a marker type");
    }

    fn into_f64(self) -> f64 {
        panic!("VoidArray is a marker type");
    }

    fn from_f64(_: f64) -> Self {
        panic!("VoidArray is a marker type");
    }

    fn flip_endian(self) -> Self {
        panic!("VoidArray is a marker type");
    }

    fn neg(self) -> Self {
        panic!("VoidArray is a marker type");
    }
    fn add(self, _: Self) -> Self {
        panic!("VoidArray is a marker type");
    }
    fn and(self, _: Self) -> Self {
        panic!("VoidArray is a marker type");
    }
    fn swap(self, _: Self) -> Self {
        panic!("VoidArray is a marker type");
    }
    fn or(self, _: Self) -> Self {
        panic!("VoidArray is a marker type");
    }
    fn sub(self, _: Self) -> Self {
        panic!("VoidArray is a marker type");
    }
    fn xor(self, _: Self) -> Self {
        panic!("VoidArray is a marker type");
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}

impl Viewable for u8 {
    type Storage = Self;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Uint8Array;
    const NAME: &str = "Uint8Array";

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_be()).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_uint8_number(agent, value).to_be()
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_uint8_number(agent, value).to_le()
    }

    fn try_from_value(_: &mut Agent, value: Value) -> Option<Self> {
        let Value::Integer(value) = value else {
            if value == Value::SmallF64((-0.0f32).into()) {
                return Some(0);
            }
            return None;
        };
        u8::try_from(value.into_i64()).ok()
    }

    fn default() -> Self {
        0
    }

    fn into_bits(self) -> u64 {
        self.into()
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        value
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value
    }

    fn into_f64(self) -> f64 {
        self.into()
    }

    fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return 0;
        }
        truncate_modulo_2_64(value) as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        self.swap_bytes()
    }

    #[inline(always)]
    fn neg(self) -> Self {
        self.wrapping_neg()
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        self.bitand(other)
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        self.bitor(other)
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        self.bitxor(other)
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
impl Viewable for U8Clamped {
    type Storage = u8;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Uint8ClampedArray;
    const NAME: &str = "Uint8ClampedArray";

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.0.to_be()).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.0.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        Self(to_uint8_clamp_number(agent, value).to_be())
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        Self(to_uint8_clamp_number(agent, value).to_le())
    }

    fn try_from_value(_: &mut Agent, value: Value) -> Option<Self> {
        let Value::Integer(value) = value else {
            if value == Value::SmallF64((-0.0f32).into()) {
                return Some(U8Clamped(0));
            }
            return None;
        };
        u8::try_from(value.into_i64()).ok().map(U8Clamped)
    }

    fn default() -> Self {
        U8Clamped(0)
    }

    fn into_bits(self) -> u64 {
        self.0.into()
    }

    fn from_bits(bits: u64) -> Self {
        U8Clamped(bits.clamp(0, 255) as u8)
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        Self(value)
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value.0
    }

    fn into_f64(self) -> f64 {
        self.0.into()
    }

    fn from_f64(value: f64) -> Self {
        if value.is_nan() {
            return Self(0);
        }
        U8Clamped(value.clamp(0.0, 255.0).round_ties_even() as u8)
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        Self(self.0.swap_bytes())
    }

    #[inline(always)]
    fn neg(self) -> Self {
        Self(self.0.wrapping_neg())
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self(self.0.wrapping_add(other.0))
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        Self(self.0.bitand(other.0))
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        Self(self.0.bitor(other.0))
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        Self(self.0.wrapping_sub(other.0))
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        Self(self.0.bitxor(other.0))
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
impl Viewable for i8 {
    type Storage = u8;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Int8Array;
    const NAME: &str = "Int8Array";

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_be()).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_int8_number(agent, value).to_be()
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_int8_number(agent, value).to_le()
    }

    fn try_from_value(_: &mut Agent, value: Value) -> Option<Self> {
        let Value::Integer(value) = value else {
            if value == Value::SmallF64((-0.0f32).into()) {
                return Some(0);
            }
            return None;
        };
        i8::try_from(value.into_i64()).ok()
    }

    fn default() -> Self {
        0
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        value.cast_signed()
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value.cast_unsigned()
    }

    fn into_f64(self) -> f64 {
        self.into()
    }

    fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return 0;
        }
        truncate_modulo_2_64(value) as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        self.swap_bytes()
    }

    #[inline(always)]
    fn neg(self) -> Self {
        self.wrapping_neg()
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        self.bitand(other)
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        self.bitor(other)
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        self.bitxor(other)
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
impl Viewable for u16 {
    type Storage = Self;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Uint16Array;
    const NAME: &str = "Uint16Array";

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_be()).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_uint16_number(agent, value).to_be()
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_uint16_number(agent, value).to_le()
    }

    fn try_from_value(_: &mut Agent, value: Value) -> Option<Self> {
        let Value::Integer(value) = value else {
            if value == Value::SmallF64((-0.0f32).into()) {
                return Some(0);
            }
            return None;
        };
        u16::try_from(value.into_i64()).ok()
    }

    fn default() -> Self {
        0
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        value
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value
    }

    fn into_f64(self) -> f64 {
        self.into()
    }

    fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return 0;
        }
        truncate_modulo_2_64(value) as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        self.swap_bytes()
    }

    #[inline(always)]
    fn neg(self) -> Self {
        self.wrapping_neg()
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        self.bitand(other)
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        self.bitor(other)
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        self.bitxor(other)
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
impl Viewable for i16 {
    type Storage = u16;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Int16Array;
    const NAME: &str = "Int16Array";

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_be()).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_int16_number(agent, value).to_be()
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_int16_number(agent, value).to_le()
    }

    fn try_from_value(_: &mut Agent, value: Value) -> Option<Self> {
        let Value::Integer(value) = value else {
            if value == Value::SmallF64((-0.0f32).into()) {
                return Some(0);
            }
            return None;
        };
        i16::try_from(value.into_i64()).ok()
    }

    fn default() -> Self {
        0
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        value.cast_signed()
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value.cast_unsigned()
    }

    fn into_f64(self) -> f64 {
        self.into()
    }

    fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return 0;
        }
        truncate_modulo_2_64(value) as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        self.swap_bytes()
    }

    #[inline(always)]
    fn neg(self) -> Self {
        self.wrapping_neg()
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        self.bitand(other)
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        self.bitor(other)
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        self.bitxor(other)
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
impl Viewable for u32 {
    type Storage = Self;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Uint32Array;
    const NAME: &str = "Uint32Array";

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_be()).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_uint32_number(agent, value).to_be()
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_uint32_number(agent, value).to_le()
    }

    fn try_from_value(_: &mut Agent, value: Value) -> Option<Self> {
        let Value::Integer(value) = value else {
            if value == Value::SmallF64((-0.0f32).into()) {
                return Some(0);
            }
            return None;
        };
        u32::try_from(value.into_i64()).ok()
    }

    fn default() -> Self {
        0
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        value
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value
    }

    fn into_f64(self) -> f64 {
        self.into()
    }

    fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return 0;
        }
        truncate_modulo_2_64(value) as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        self.swap_bytes()
    }

    #[inline(always)]
    fn neg(self) -> Self {
        self.wrapping_neg()
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        self.bitand(other)
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        self.bitor(other)
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        self.bitxor(other)
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
impl Viewable for i32 {
    type Storage = u32;

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Int32Array;
    const NAME: &str = "Int32Array";

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_be()).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(self.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_int32_number(agent, value).to_be()
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        to_int32_number(agent, value).to_le()
    }

    fn try_from_value(_: &mut Agent, value: Value) -> Option<Self> {
        let Value::Integer(value) = value else {
            if value == Value::SmallF64((-0.0f32).into()) {
                return Some(0);
            }
            return None;
        };
        i32::try_from(value.into_i64()).ok()
    }

    fn default() -> Self {
        0
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        value.cast_signed()
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value.cast_unsigned()
    }

    fn into_f64(self) -> f64 {
        self.into()
    }

    fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return 0;
        }
        truncate_modulo_2_64(value) as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        self.swap_bytes()
    }

    #[inline(always)]
    fn neg(self) -> Self {
        self.wrapping_neg()
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        self.bitand(other)
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        self.bitor(other)
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        self.bitxor(other)
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
impl Viewable for u64 {
    type Storage = Self;

    const IS_BIGINT: bool = true;
    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::BigUint64Array;
    const NAME: &str = "BigUint64Array";

    fn into_be_value<'a>(self, agent: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        BigInt::from_u64(agent, self.to_be()).into()
    }

    fn into_le_value<'a>(self, agent: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        BigInt::from_u64(agent, self.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = BigInt::try_from(value) else {
            unreachable!()
        };
        to_big_uint64_big_int(agent, value).to_be()
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = BigInt::try_from(value) else {
            unreachable!()
        };
        to_big_uint64_big_int(agent, value).to_le()
    }

    fn try_from_value(agent: &mut Agent, value: Value) -> Option<Self> {
        if let Value::SmallBigInt(value) = value {
            let value = value.into_i64();
            return u64::try_from(value).ok();
        };
        if let Value::BigInt(value) = value {
            let data = value.get(agent);
            let mut iter = data.data.iter_u64_digits();
            let sign = data.data.sign();
            if sign == Sign::Minus {
                return None;
            }
            if iter.len() > 1 {
                return None;
            }
            let value = iter.next().unwrap();
            return Some(value);
        };
        None
    }

    fn default() -> Self {
        0
    }

    fn into_bits(self) -> u64 {
        self
    }

    fn from_bits(bits: u64) -> Self {
        bits
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        value
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value
    }

    fn into_f64(self) -> f64 {
        self as f64
    }

    fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return 0;
        }
        value.trunc() as i64 as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        self.swap_bytes()
    }

    #[inline(always)]
    fn neg(self) -> Self {
        self.wrapping_neg()
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        self.bitand(other)
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        self.bitor(other)
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        self.bitxor(other)
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
impl Viewable for i64 {
    type Storage = u64;

    const IS_BIGINT: bool = true;
    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::BigInt64Array;
    const NAME: &str = "BigInt64Array";

    fn into_be_value<'a>(self, agent: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        BigInt::from_i64(agent, self.to_be()).into()
    }

    fn into_le_value<'a>(self, agent: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        BigInt::from_i64(agent, self.to_le()).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = BigInt::try_from(value) else {
            unreachable!()
        };
        to_big_int64_big_int(agent, value).to_be()
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = BigInt::try_from(value) else {
            unreachable!()
        };
        to_big_int64_big_int(agent, value).to_le()
    }

    fn try_from_value(agent: &mut Agent, value: Value) -> Option<Self> {
        if let Value::SmallBigInt(value) = value {
            return Some(value.into_i64());
        };
        if let Value::BigInt(value) = value {
            let data = value.get(agent);
            let mut iter = data.data.iter_u64_digits();
            if iter.len() > 1 {
                return None;
            }
            let sign = data.data.sign();
            let value = iter.next().unwrap();
            if sign == Sign::Minus {
                if value <= i64::MIN.unsigned_abs() {
                    return Some(value.wrapping_neg() as i64);
                } else {
                    return None;
                }
            } else if value <= i64::MAX as u64 {
                return Some(value as i64);
            } else {
                return None;
            }
        };
        None
    }

    fn default() -> Self {
        0
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        value.cast_signed()
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        value.cast_unsigned()
    }

    fn into_f64(self) -> f64 {
        self as f64
    }

    fn from_f64(value: f64) -> Self {
        if !value.is_finite() {
            return 0;
        }
        value.trunc() as i64 as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        self.swap_bytes()
    }

    #[inline(always)]
    fn neg(self) -> Self {
        self.wrapping_neg()
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self.wrapping_add(other)
    }
    #[inline(always)]
    fn and(self, other: Self) -> Self {
        self.bitand(other)
    }
    #[inline(always)]
    fn swap(self, other: Self) -> Self {
        other
    }
    #[inline(always)]
    fn or(self, other: Self) -> Self {
        self.bitor(other)
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self.wrapping_sub(other)
    }
    #[inline(always)]
    fn xor(self, other: Self) -> Self {
        self.bitxor(other)
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.cmp(other)
    }
}
#[cfg(feature = "proposal-float16array")]
impl Viewable for f16 {
    type Storage = u16;

    const IS_FLOAT: bool = true;
    const NAME: &str = "Float16Array";

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Float16Array;

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(Self::from_ne_bytes(self.to_be_bytes())).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(Self::from_ne_bytes(self.to_le_bytes())).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        Self::from_ne_bytes((value.to_real(agent) as Self).to_be_bytes())
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        Self::from_ne_bytes((value.to_real(agent) as Self).to_le_bytes())
    }

    fn try_from_value(agent: &mut Agent, value: Value) -> Option<Self> {
        let Ok(value) = Number::try_from(value) else {
            return None;
        };
        let value = value.into_f64(agent);
        if value.is_nan() {
            return Some(f16::NAN);
        }
        if value as f16 as f64 == value {
            Some(value as f16)
        } else {
            None
        }
    }
    fn default() -> Self {
        f16::NAN
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        f16::from_bits(value)
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        f16::to_bits(value)
    }

    fn into_f64(self) -> f64 {
        self as f64
    }

    fn from_f64(value: f64) -> Self {
        value as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        Self::from_bits(self.to_bits().swap_bytes())
    }

    #[inline(always)]
    fn neg(self) -> Self {
        core::ops::Neg::neg(self)
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self + other
    }
    #[inline(always)]
    fn and(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn swap(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn or(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self - other
    }
    #[inline(always)]
    fn xor(self, _other: Self) -> Self {
        unreachable!()
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        if self.is_nan() {
            if other.is_nan() {
                return core::cmp::Ordering::Equal;
            }
            return core::cmp::Ordering::Greater;
        }
        if other.is_nan() {
            return core::cmp::Ordering::Less;
        }
        if *self == 0.0 && *other == 0.0 {
            if self.is_sign_negative() && other.is_sign_positive() {
                return core::cmp::Ordering::Less;
            }
            if self.is_sign_positive() && other.is_sign_negative() {
                return core::cmp::Ordering::Greater;
            }
            return core::cmp::Ordering::Equal;
        }
        self.partial_cmp(other).unwrap()
    }
}
impl Viewable for f32 {
    type Storage = u32;

    const IS_FLOAT: bool = true;
    const NAME: &str = "Float32Array";

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Float32Array;

    fn into_be_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(Self::from_ne_bytes(self.to_be_bytes())).into()
    }

    fn into_le_value<'a>(self, _: &mut Agent, _: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from(Self::from_ne_bytes(self.to_le_bytes())).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        Self::from_ne_bytes((value.to_real(agent) as Self).to_be_bytes())
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        Self::from_ne_bytes((value.to_real(agent) as Self).to_le_bytes())
    }

    fn try_from_value(agent: &mut Agent, value: Value) -> Option<Self> {
        let Ok(value) = Number::try_from(value) else {
            return None;
        };
        let value = value.into_f64_(agent);
        if value.is_nan() {
            return Some(f32::NAN);
        }
        if value as f32 as f64 == value {
            Some(value as f32)
        } else {
            None
        }
    }

    fn default() -> Self {
        f32::NAN
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        f32::from_bits(value)
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        f32::to_bits(value)
    }

    fn into_f64(self) -> f64 {
        self.into()
    }

    fn from_f64(value: f64) -> Self {
        value as Self
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        Self::from_bits(self.to_bits().swap_bytes())
    }

    #[inline(always)]
    fn neg(self) -> Self {
        core::ops::Neg::neg(self)
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self + other
    }
    #[inline(always)]
    fn and(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn swap(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn or(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self - other
    }
    #[inline(always)]
    fn xor(self, _other: Self) -> Self {
        unreachable!()
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        if self.is_nan() {
            if other.is_nan() {
                return core::cmp::Ordering::Equal;
            }
            return core::cmp::Ordering::Greater;
        }
        if other.is_nan() {
            return core::cmp::Ordering::Less;
        }
        if *self == 0.0 && *other == 0.0 {
            if self.is_sign_negative() && other.is_sign_positive() {
                return core::cmp::Ordering::Less;
            }
            if self.is_sign_positive() && other.is_sign_negative() {
                return core::cmp::Ordering::Greater;
            }
            return core::cmp::Ordering::Equal;
        }
        self.partial_cmp(other).unwrap()
    }
}
impl Viewable for f64 {
    type Storage = u64;

    const IS_FLOAT: bool = true;
    const NAME: &str = "Float64Array";

    #[cfg(feature = "array-buffer")]
    const PROTO: ProtoIntrinsics = ProtoIntrinsics::Float64Array;

    fn into_be_value<'a>(self, agent: &mut Agent, gc: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from_f64(agent, Self::from_ne_bytes(self.to_be_bytes()), gc).into()
    }

    fn into_le_value<'a>(self, agent: &mut Agent, gc: NoGcScope<'a, '_>) -> Numeric<'a> {
        Number::from_f64(agent, Self::from_ne_bytes(self.to_le_bytes()), gc).into()
    }

    fn from_be_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        Self::from_ne_bytes((value.to_real(agent) as Self).to_be_bytes())
    }

    fn from_le_value(agent: &Agent, value: Numeric) -> Self {
        let Ok(value) = Number::try_from(value) else {
            unreachable!()
        };
        Self::from_ne_bytes((value.to_real(agent) as Self).to_le_bytes())
    }

    fn try_from_value(agent: &mut Agent, value: Value) -> Option<Self> {
        let Ok(value) = Number::try_from(value) else {
            return None;
        };
        Some(value.into_f64_(agent))
    }

    fn default() -> Self {
        f64::NAN
    }

    fn into_bits(self) -> u64 {
        self as u64
    }

    fn from_bits(bits: u64) -> Self {
        bits as Self
    }

    #[inline(always)]
    fn from_storage(value: Self::Storage) -> Self {
        f64::from_bits(value)
    }

    #[inline(always)]
    fn into_storage(value: Self) -> Self::Storage {
        f64::to_bits(value)
    }

    fn into_f64(self) -> f64 {
        self
    }

    fn from_f64(value: f64) -> Self {
        value
    }

    #[inline(always)]
    fn flip_endian(self) -> Self {
        Self::from_bits(self.to_bits().swap_bytes())
    }

    #[inline(always)]
    fn neg(self) -> Self {
        core::ops::Neg::neg(self)
    }
    #[inline(always)]
    fn add(self, other: Self) -> Self {
        self + other
    }
    #[inline(always)]
    fn and(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn swap(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn or(self, _other: Self) -> Self {
        unreachable!()
    }
    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        self - other
    }
    #[inline(always)]
    fn xor(self, _other: Self) -> Self {
        unreachable!()
    }

    #[inline(always)]
    fn ecmascript_cmp(&self, other: &Self) -> core::cmp::Ordering {
        if self.is_nan() {
            if other.is_nan() {
                return core::cmp::Ordering::Equal;
            }
            return core::cmp::Ordering::Greater;
        }
        if other.is_nan() {
            return core::cmp::Ordering::Less;
        }
        if *self == 0.0 && *other == 0.0 {
            if self.is_sign_negative() && other.is_sign_positive() {
                return core::cmp::Ordering::Less;
            }
            if self.is_sign_positive() && other.is_sign_negative() {
                return core::cmp::Ordering::Greater;
            }
            return core::cmp::Ordering::Equal;
        }
        self.partial_cmp(other).unwrap()
    }
}

mod tests {
    #[test]
    fn new_data_block() {
        use super::DataBlock;
        let db = DataBlock::new(0).unwrap();
        assert_eq!(db.len(), 0);
        assert_eq!(db.get::<u8>(0), None);

        let db = DataBlock::new(8).unwrap();
        assert_eq!(db.len(), 8);
        for i in 0..8 {
            assert_eq!(db.get::<u8>(i), Some(0));
        }
    }

    #[test]
    fn data_block_set() {
        use super::DataBlock;
        let mut db = DataBlock::new(8).unwrap();
        assert_eq!(db.len(), 8);
        for i in 0..8 {
            assert_eq!(db.get::<u8>(i), Some(0));
        }

        for i in 0..8 {
            db.set::<u8>(i as usize, i + 1);
        }

        for i in 0..8 {
            assert_eq!(db.get::<u8>(i as usize), Some(i + 1));
        }
    }

    #[test]
    fn data_block_set_from() {
        use super::DataBlock;
        let mut db = DataBlock::new(8).unwrap();
        let mut db2 = DataBlock::new(8).unwrap();
        for i in 0..8 {
            assert_eq!(db.get::<u8>(0), Some(0));
            db2.set::<u8>(i as usize, i + 1);
        }
        assert_eq!(db2.get::<u8>(0), Some(1));
        assert_eq!(db2.get::<u8>(1), Some(2));
        assert_eq!(db2.get::<u8>(2), Some(3));
        assert_eq!(db2.get::<u8>(3), Some(4));
        assert_eq!(db2.get::<u8>(4), Some(5));
        assert_eq!(db2.get::<u8>(5), Some(6));
        assert_eq!(db2.get::<u8>(6), Some(7));
        assert_eq!(db2.get::<u8>(7), Some(8));
        db.set_from::<u8>(0, &db2, 4, 4);
        assert_eq!(db.get::<u8>(0), Some(5));
        assert_eq!(db.get::<u8>(1), Some(6));
        assert_eq!(db.get::<u8>(2), Some(7));
        assert_eq!(db.get::<u8>(3), Some(8));
        assert_eq!(db.get::<u8>(4), Some(0));
        assert_eq!(db.get::<u8>(5), Some(0));
        assert_eq!(db.get::<u8>(6), Some(0));
        assert_eq!(db.get::<u8>(7), Some(0));

        // Reset
        for i in 0..8 {
            db.set::<u8>(i as usize, i + 1);
        }
        db.copy_within::<u8>(2, 4, 4);
        assert_eq!(db.get::<u8>(0), Some(1));
        assert_eq!(db.get::<u8>(1), Some(2));
        assert_eq!(db.get::<u8>(2), Some(5));
        assert_eq!(db.get::<u8>(3), Some(6));
        assert_eq!(db.get::<u8>(4), Some(7));
        assert_eq!(db.get::<u8>(5), Some(8));
        assert_eq!(db.get::<u8>(6), Some(7));
        assert_eq!(db.get::<u8>(7), Some(8));
    }

    #[test]
    fn data_block_copy_within() {
        use super::DataBlock;
        let mut db = DataBlock::new(8).unwrap();
        for i in 0..8 {
            db.set::<u8>(i as usize, i + 1);
        }
        assert_eq!(db.get::<u8>(0), Some(1));
        assert_eq!(db.get::<u8>(1), Some(2));
        assert_eq!(db.get::<u8>(2), Some(3));
        assert_eq!(db.get::<u8>(3), Some(4));
        assert_eq!(db.get::<u8>(4), Some(5));
        assert_eq!(db.get::<u8>(5), Some(6));
        assert_eq!(db.get::<u8>(6), Some(7));
        assert_eq!(db.get::<u8>(7), Some(8));
        db.copy_within::<u8>(0, 4, 4);
        assert_eq!(db.get::<u8>(0), Some(5));
        assert_eq!(db.get::<u8>(1), Some(6));
        assert_eq!(db.get::<u8>(2), Some(7));
        assert_eq!(db.get::<u8>(3), Some(8));
        assert_eq!(db.get::<u8>(4), Some(5));
        assert_eq!(db.get::<u8>(5), Some(6));
        assert_eq!(db.get::<u8>(6), Some(7));
        assert_eq!(db.get::<u8>(7), Some(8));

        // Reset
        for i in 0..8 {
            db.set::<u8>(i as usize, i + 1);
        }
        db.copy_within::<u8>(2, 4, 4);
        assert_eq!(db.get::<u8>(0), Some(1));
        assert_eq!(db.get::<u8>(1), Some(2));
        assert_eq!(db.get::<u8>(2), Some(5));
        assert_eq!(db.get::<u8>(3), Some(6));
        assert_eq!(db.get::<u8>(4), Some(7));
        assert_eq!(db.get::<u8>(5), Some(8));
        assert_eq!(db.get::<u8>(6), Some(7));
        assert_eq!(db.get::<u8>(7), Some(8));
    }

    #[test]
    #[cfg(feature = "shared-array-buffer")]
    fn new_shared_data_block() {
        use super::SharedDataBlock;
        let _ = SharedDataBlock::new(1024, None).unwrap();
    }

    #[test]
    #[cfg(feature = "shared-array-buffer")]
    fn clone_shared_data_block() {
        use super::SharedDataBlock;
        let a = SharedDataBlock::new(1024, None).unwrap();
        let _ = a.clone();
    }

    #[test]
    #[cfg(feature = "shared-array-buffer")]
    fn clone_shared_data_block_thread_safe() {
        use super::SharedDataBlock;
        use std::thread;
        let a = SharedDataBlock::new(1024, None).unwrap();
        thread::scope(|s| {
            s.spawn(|| a.clone());
            s.spawn(|| a.clone());
            s.spawn(|| a.clone());
            s.spawn(|| a.clone());
        });
    }
}
