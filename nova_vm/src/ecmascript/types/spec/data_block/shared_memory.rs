// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Racy shared memory built from atomic words.
//!
//! The bytes of a Shared Data Block are stored in `AtomicU64` words: byte `i`
//! of the block is byte `i % 8` of word `i / 8` in native byte order, so the
//! block's bytes have the same order as they would in plain memory. Every
//! access is an atomic operation on whole words, which makes data races
//! between ECMAScript agents well defined in Rust:
//!
//! - an aligned element of at most 8 bytes lies inside one word and is read
//!   with one atomic load and written with one atomic store or
//!   compare-and-exchange loop, so it never tears;
//! - unaligned elements and bulk copies are performed word by word with
//!   Relaxed ordering and may tear, as the ECMAScript memory model allows.

use core::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering as RustOrdering};

use super::StorageBytes;

/// ECMAScript atomic memory orderings.
///
/// With [`Ordering::Unordered`], no synchronisation is performed. With
/// [`Ordering::SeqCst`], all threads see all sequentially consistent
/// operations in the same order.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Ordering {
    /// No ordering constraints.
    Unordered,
    /// All threads see all sequentially consistent operations in the same
    /// order.
    SeqCst,
}

impl Ordering {
    #[inline(always)]
    fn to_rust(self) -> RustOrdering {
        match self {
            Ordering::Unordered => RustOrdering::Relaxed,
            Ordering::SeqCst => RustOrdering::SeqCst,
        }
    }
}

const WORD: usize = core::mem::size_of::<u64>();

/// Allocate zeroed words for `byte_length` bytes.
///
/// Returns None if the allocation is impossible.
pub(super) fn allocate_words(byte_length: usize) -> Option<Box<[AtomicU64]>> {
    let count = byte_length.div_ceil(WORD);
    // Probe that the allocation is possible: a failed probe is reported to
    // the caller, while the zeroed allocation below would abort the process.
    // The zeroed allocation itself lets the allocator hand out lazily zeroed
    // pages for large blocks; collecting it into atomics of the same layout
    // reuses that allocation.
    let mut probe: Vec<u64> = Vec::new();
    probe.try_reserve_exact(count).ok()?;
    drop(probe);
    Some(
        vec![0u64; count]
            .into_iter()
            .map(AtomicU64::new)
            .collect::<Vec<AtomicU64>>()
            .into_boxed_slice(),
    )
}

/// Read the bytes starting at `byte_offset` into `dst` with Relaxed loads.
///
/// Panics if the bytes are not within `words`.
fn read_bytes(words: &[AtomicU64], byte_offset: usize, dst: &mut [u8]) {
    let mut position = byte_offset;
    let mut done = 0;
    while done < dst.len() {
        let in_word = position % WORD;
        let count = (WORD - in_word).min(dst.len() - done);
        let word = words[position / WORD]
            .load(RustOrdering::Relaxed)
            .to_ne_bytes();
        dst[done..done + count].copy_from_slice(&word[in_word..in_word + count]);
        done += count;
        position += count;
    }
}

/// Write `src` into the bytes starting at `byte_offset` with Relaxed stores.
///
/// Panics if the bytes are not within `words`.
fn write_bytes(words: &[AtomicU64], byte_offset: usize, src: &[u8]) {
    let mut position = byte_offset;
    let mut done = 0;
    while done < src.len() {
        let in_word = position % WORD;
        let count = (WORD - in_word).min(src.len() - done);
        let word = &words[position / WORD];
        let bytes = &src[done..done + count];
        if count == WORD {
            let bytes: [u8; WORD] = bytes.try_into().expect("a word is 8 bytes");
            word.store(u64::from_ne_bytes(bytes), RustOrdering::Relaxed);
        } else {
            let _ = word.try_update(RustOrdering::Relaxed, RustOrdering::Relaxed, |old| {
                let mut new = old.to_ne_bytes();
                new[in_word..in_word + count].copy_from_slice(bytes);
                Some(u64::from_ne_bytes(new))
            });
        }
        done += count;
        position += count;
    }
}

const CHUNK: usize = 256;

/// A racy view of `len` elements of type `T` starting at byte `byte_start`
/// of a Shared Data Block's words.
///
/// The view is always within the words: every constructor bounds it.
pub(crate) struct SharedSlice<'a, T: StorageBytes> {
    words: &'a [AtomicU64],
    byte_start: usize,
    len: usize,
    _marker: PhantomData<T>,
}

impl<T: StorageBytes> Clone for SharedSlice<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: StorageBytes> Copy for SharedSlice<'_, T> {}

impl<'a> SharedSlice<'a, u8> {
    /// Create a byte slice over the first `byte_length` bytes of `words`.
    ///
    /// Panics if `words` holds fewer than `byte_length` bytes.
    pub(super) fn from_words(words: &'a [AtomicU64], byte_length: usize) -> Self {
        assert!(byte_length <= words.len() * WORD);
        Self {
            words,
            byte_start: 0,
            len: byte_length,
            _marker: PhantomData,
        }
    }
}

impl<'a, T: StorageBytes> SharedSlice<'a, T> {
    /// Number of elements in the slice.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the slice has no elements.
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn byte_length(&self) -> usize {
        self.len * T::SIZE
    }

    #[inline]
    fn with_range(&self, start: usize, len: usize) -> Self {
        Self {
            words: self.words,
            byte_start: self.byte_start + start * T::SIZE,
            len,
            _marker: PhantomData,
        }
    }

    /// Slice starting at the given element offset; empty if the offset is
    /// beyond the end of this slice.
    #[inline]
    pub(crate) fn slice_from(&self, offset: usize) -> Self {
        let start = offset.min(self.len);
        self.with_range(start, self.len - start)
    }

    /// Slice ending at the given element offset; `self` if the offset is
    /// beyond the end of this slice.
    #[inline]
    pub(crate) fn slice_to(&self, offset: usize) -> Self {
        self.with_range(0, self.len.min(offset))
    }

    /// Slice with the given element offsets, bounded to the end of this
    /// slice.
    #[inline]
    pub(crate) fn slice(&self, start: usize, end: usize) -> Self {
        let start = start.min(self.len);
        let end = end.min(self.len);
        self.with_range(start, end.saturating_sub(start))
    }

    /// Split the slice into a byte prefix, the largest middle slice of `U`
    /// elements aligned to their size in the block, and a byte suffix.
    pub(crate) fn align_to<U: StorageBytes>(
        self,
    ) -> (SharedSlice<'a, u8>, SharedSlice<'a, U>, SharedSlice<'a, u8>) {
        let byte_length = self.byte_length();
        let head_length = ((U::SIZE - self.byte_start % U::SIZE) % U::SIZE).min(byte_length);
        let remaining = byte_length - head_length;
        let body_length = remaining / U::SIZE;
        let tail_length = remaining % U::SIZE;
        let body_start = self.byte_start + head_length;
        let tail_start = body_start + body_length * U::SIZE;
        (
            SharedSlice {
                words: self.words,
                byte_start: self.byte_start,
                len: head_length,
                _marker: PhantomData,
            },
            SharedSlice {
                words: self.words,
                byte_start: body_start,
                len: body_length,
                _marker: PhantomData,
            },
            SharedSlice {
                words: self.words,
                byte_start: tail_start,
                len: tail_length,
                _marker: PhantomData,
            },
        )
    }

    /// Iterate over the elements of the slice.
    #[inline]
    pub(crate) fn iter(&self) -> SharedIter<'a, T> {
        SharedIter {
            slice: *self,
            front: 0,
            back: self.len,
        }
    }

    /// The element of type `U` at the start of the slice, or None if the
    /// slice is shorter than `U` or its start is not aligned to the size of
    /// `U`.
    #[inline]
    pub(crate) fn as_aligned<U: StorageBytes>(&self) -> Option<SharedElement<'a, U>> {
        if self.byte_length() < U::SIZE || self.byte_start % U::SIZE != 0 {
            None
        } else {
            Some(SharedElement::new(self.words, self.byte_start))
        }
    }

    /// Load a `U` from the start of the slice with Unordered ordering. The
    /// start need not be aligned; an unaligned load may tear.
    ///
    /// Returns None if the slice is shorter than `U`.
    #[inline]
    pub(crate) fn load_unaligned<U: StorageBytes>(&self) -> Option<U> {
        if self.byte_length() < U::SIZE {
            return None;
        }
        if self.byte_start % U::SIZE == 0 {
            return Some(
                SharedElement::<U>::new(self.words, self.byte_start).load(Ordering::Unordered),
            );
        }
        let mut bytes = [0u8; WORD];
        let bytes = &mut bytes[..U::SIZE];
        read_bytes(self.words, self.byte_start, bytes);
        Some(U::read_ne(bytes))
    }

    /// Store a `U` into the start of the slice with Unordered ordering. The
    /// start need not be aligned; an unaligned store may tear.
    ///
    /// Returns None if the slice is shorter than `U`.
    #[inline]
    pub(crate) fn store_unaligned<U: StorageBytes>(&self, val: U) -> Option<()> {
        if self.byte_length() < U::SIZE {
            return None;
        }
        if self.byte_start % U::SIZE == 0 {
            SharedElement::<U>::new(self.words, self.byte_start).store(val, Ordering::Unordered);
            return Some(());
        }
        let mut bytes = [0u8; WORD];
        let bytes = &mut bytes[..U::SIZE];
        val.write_ne(bytes);
        write_bytes(self.words, self.byte_start, bytes);
        Some(())
    }

    /// The element at `index`, or None if it is out of bounds.
    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<SharedElement<'a, T>> {
        if index >= self.len {
            None
        } else {
            Some(SharedElement::new(
                self.words,
                self.byte_start + index * T::SIZE,
            ))
        }
    }

    /// Copy the bytes of the slice into `dst` with Unordered ordering.
    ///
    /// Panics if `dst` is not exactly as long as the slice in bytes.
    pub(crate) fn copy_into_bytes(&self, dst: &mut [u8]) {
        assert_eq!(
            self.byte_length(),
            dst.len(),
            "source slice length does not match destination slice length"
        );
        read_bytes(self.words, self.byte_start, dst);
    }

    /// Copy `src` into the bytes of the slice with Unordered ordering.
    ///
    /// Panics if `src` is not exactly as long as the slice in bytes.
    pub(crate) fn copy_from_bytes(&self, src: &[u8]) {
        assert_eq!(
            self.byte_length(),
            src.len(),
            "source slice length does not match destination slice length"
        );
        write_bytes(self.words, self.byte_start, src);
    }

    /// Copy all elements of `src` into the slice with Unordered ordering.
    ///
    /// Panics if the lengths differ.
    pub(crate) fn copy_from_slice(&self, src: &[T]) {
        assert_eq!(
            self.len,
            src.len(),
            "source slice length does not match destination slice length"
        );
        let mut buffer = [0u8; CHUNK];
        let per_chunk = CHUNK / T::SIZE;
        for (index, chunk) in src.chunks(per_chunk).enumerate() {
            let bytes = &mut buffer[..chunk.len() * T::SIZE];
            for (value, target) in chunk.iter().zip(bytes.chunks_exact_mut(T::SIZE)) {
                value.write_ne(target);
            }
            write_bytes(
                self.words,
                self.byte_start + index * per_chunk * T::SIZE,
                bytes,
            );
        }
    }

    /// Copy all elements of the slice into `dst` with Unordered ordering.
    ///
    /// Panics if the lengths differ.
    pub(crate) fn copy_into_slice(&self, dst: &mut [T]) {
        assert_eq!(
            self.len,
            dst.len(),
            "source slice length does not match destination slice length"
        );
        let mut buffer = [0u8; CHUNK];
        let per_chunk = CHUNK / T::SIZE;
        for (index, chunk) in dst.chunks_mut(per_chunk).enumerate() {
            let bytes = &mut buffer[..chunk.len() * T::SIZE];
            read_bytes(
                self.words,
                self.byte_start + index * per_chunk * T::SIZE,
                bytes,
            );
            for (value, source) in chunk.iter_mut().zip(bytes.chunks_exact(T::SIZE)) {
                *value = T::read_ne(source);
            }
        }
    }

    /// Copy all elements of `other` into the slice with Unordered ordering,
    /// as a memmove when both are views of the same block.
    ///
    /// Panics if the lengths differ.
    pub(crate) fn copy_from_racy_slice(&self, other: &Self) {
        assert_eq!(
            self.len, other.len,
            "source slice length does not match destination slice length"
        );
        let total = self.byte_length();
        let same_block = core::ptr::eq(self.words.as_ptr(), other.words.as_ptr());
        let backwards = same_block && self.byte_start > other.byte_start;
        let mut buffer = [0u8; CHUNK];
        let chunks = total.div_ceil(CHUNK);
        for step in 0..chunks {
            let chunk = if backwards { chunks - 1 - step } else { step };
            let offset = chunk * CHUNK;
            let bytes = &mut buffer[..CHUNK.min(total - offset)];
            read_bytes(other.words, other.byte_start + offset, bytes);
            write_bytes(self.words, self.byte_start + offset, bytes);
        }
    }
}

/// Iterator over the elements of a [`SharedSlice`].
pub(crate) struct SharedIter<'a, T: StorageBytes> {
    slice: SharedSlice<'a, T>,
    front: usize,
    back: usize,
}

impl<'a, T: StorageBytes> Iterator for SharedIter<'a, T> {
    type Item = SharedElement<'a, T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        let item = self.slice.get(self.front);
        self.front += 1;
        item
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.back - self.front;
        (len, Some(len))
    }
}

impl<T: StorageBytes> DoubleEndedIterator for SharedIter<'_, T> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front >= self.back {
            return None;
        }
        self.back -= 1;
        self.slice.get(self.back)
    }
}

impl<T: StorageBytes> ExactSizeIterator for SharedIter<'_, T> {}

/// One aligned element of type `T` inside a word of a Shared Data Block.
pub(crate) struct SharedElement<'a, T: StorageBytes> {
    word: &'a AtomicU64,
    in_word: usize,
    _marker: PhantomData<T>,
}

impl<T: StorageBytes> Clone for SharedElement<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: StorageBytes> Copy for SharedElement<'_, T> {}

impl<'a, T: StorageBytes> SharedElement<'a, T> {
    /// Panics if `byte_offset` is not aligned to the size of `T` or the
    /// element is not within `words`.
    #[inline]
    fn new(words: &'a [AtomicU64], byte_offset: usize) -> Self {
        assert_eq!(byte_offset % T::SIZE, 0, "shared element is not aligned");
        Self {
            word: &words[byte_offset / WORD],
            in_word: byte_offset % WORD,
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    fn extract(&self, word: u64) -> T {
        T::read_ne(&word.to_ne_bytes()[self.in_word..self.in_word + T::SIZE])
    }

    #[inline(always)]
    fn replace(&self, word: u64, value: T) -> u64 {
        let mut bytes = word.to_ne_bytes();
        value.write_ne(&mut bytes[self.in_word..self.in_word + T::SIZE]);
        u64::from_ne_bytes(bytes)
    }

    /// Atomically load the element.
    #[inline]
    pub(crate) fn load(&self, order: Ordering) -> T {
        self.extract(self.word.load(order.to_rust()))
    }

    /// Atomically store the element.
    #[inline]
    pub(crate) fn store(&self, value: T, order: Ordering) {
        let order = order.to_rust();
        if T::SIZE == WORD {
            self.word.store(value.into_u64(), order);
        } else {
            let _ = self.word.try_update(order, RustOrdering::Relaxed, |word| {
                Some(self.replace(word, value))
            });
        }
    }

    #[inline]
    fn update(&self, f: impl Fn(T) -> T) -> T {
        let previous = self
            .word
            .try_update(RustOrdering::SeqCst, RustOrdering::SeqCst, |word| {
                Some(self.replace(word, f(self.extract(word))))
            });
        match previous {
            Ok(word) | Err(word) => self.extract(word),
        }
    }

    /// Sequentially consistent compare-and-exchange of the element. Returns
    /// the previous value: Ok if it equalled `current` and was replaced.
    #[inline]
    pub(crate) fn compare_exchange(&self, current: T, new: T) -> Result<T, T> {
        let mut word = self.word.load(RustOrdering::SeqCst);
        loop {
            let previous = self.extract(word);
            if previous.into_u64() != current.into_u64() {
                return Err(previous);
            }
            match self.word.compare_exchange_weak(
                word,
                self.replace(word, new),
                RustOrdering::SeqCst,
                RustOrdering::SeqCst,
            ) {
                Ok(_) => return Ok(previous),
                Err(actual) => word = actual,
            }
        }
    }

    /// Sequentially consistent wrapping addition; returns the previous value.
    #[inline]
    pub(crate) fn fetch_add(&self, value: T) -> T {
        let value = value.into_u64();
        self.update(|old| T::from_u64(old.into_u64().wrapping_add(value)))
    }

    /// Sequentially consistent bitwise and; returns the previous value.
    #[inline]
    pub(crate) fn fetch_and(&self, value: T) -> T {
        let value = value.into_u64();
        self.update(|old| T::from_u64(old.into_u64() & value))
    }

    /// Sequentially consistent bitwise or; returns the previous value.
    #[inline]
    pub(crate) fn fetch_or(&self, value: T) -> T {
        let value = value.into_u64();
        self.update(|old| T::from_u64(old.into_u64() | value))
    }

    /// Sequentially consistent bitwise xor; returns the previous value.
    #[inline]
    pub(crate) fn fetch_xor(&self, value: T) -> T {
        let value = value.into_u64();
        self.update(|old| T::from_u64(old.into_u64() ^ value))
    }

    /// Sequentially consistent exchange; returns the previous value.
    #[inline]
    pub(crate) fn swap(&self, value: T) -> T {
        self.update(|_| value)
    }
}

#[cfg(test)]
mod tests {
    use super::{Ordering, SharedSlice, allocate_words};

    #[test]
    fn bytes_keep_native_order() {
        let words = allocate_words(16).unwrap();
        let bytes = SharedSlice::from_words(&words, 16);
        let (head, u32s, tail) = bytes.slice_from(4).align_to::<u32>();
        assert!(head.is_empty() && tail.is_empty() && u32s.len() == 3);
        u32s.get(0).unwrap().store(0x0403_0201, Ordering::SeqCst);
        let mut read = [0u8; 4];
        bytes.slice(4, 8).copy_into_bytes(&mut read);
        assert_eq!(u32::from_ne_bytes(read), 0x0403_0201);
        assert_eq!(bytes.slice_from(5).load_unaligned::<u16>(), {
            let b = 0x0403_0201u32.to_ne_bytes();
            Some(u16::from_ne_bytes([b[1], b[2]]))
        });
        bytes.slice_from(7).store_unaligned(u64::MAX).unwrap();
        assert_eq!(bytes.slice_from(14).load_unaligned::<u8>(), Some(0xFF));
        assert_eq!(bytes.slice_from(15).load_unaligned::<u8>(), Some(0));
        assert_eq!(bytes.slice_from(9).load_unaligned::<u64>(), None);
        let (head, _, _) = bytes.slice_from(3).align_to::<u64>();
        assert_eq!(head.len(), 5);
    }

    #[test]
    fn read_modify_write() {
        let words = allocate_words(8).unwrap();
        let bytes = SharedSlice::from_words(&words, 8);
        let (_, u16s, _) = bytes.align_to::<u16>();
        let e = u16s.get(1).unwrap();
        assert_eq!(e.fetch_add(0xFFFF), 0);
        assert_eq!(e.fetch_add(2), 0xFFFF);
        assert_eq!(e.load(Ordering::Unordered), 1);
        assert_eq!(e.compare_exchange(2, 5), Err(1));
        assert_eq!(e.compare_exchange(1, 5), Ok(1));
        assert_eq!(e.swap(7), 5);
        assert_eq!(e.fetch_or(8), 7);
        assert_eq!(e.fetch_and(0xC), 15);
        assert_eq!(e.fetch_xor(0xF), 12);
        assert_eq!(e.load(Ordering::SeqCst), 3);
        assert_eq!(u16s.get(0).unwrap().load(Ordering::SeqCst), 0);
        assert_eq!(u16s.get(2).unwrap().load(Ordering::SeqCst), 0);
        assert!(u16s.get(4).is_none());
    }

    #[test]
    fn overlapping_copies_move() {
        let words = allocate_words(1000).unwrap();
        let bytes = SharedSlice::from_words(&words, 1000);
        let data: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect();
        bytes.copy_from_slice(&data);
        bytes
            .slice(3, 803)
            .copy_from_racy_slice(&bytes.slice(0, 800));
        let mut out = vec![0u8; 1000];
        bytes.copy_into_slice(&mut out);
        let mut expected = data.clone();
        expected.copy_within(0..800, 3);
        assert_eq!(out, expected);
        bytes
            .slice(0, 900)
            .copy_from_racy_slice(&bytes.slice(97, 997));
        bytes.copy_into_slice(&mut out);
        expected.copy_within(97..997, 0);
        assert_eq!(out, expected);
    }
}
