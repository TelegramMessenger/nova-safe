//! Extension trait for slice iterators.
//!
//! Provides additional methods to inspect and advance iterators.
//!
//! See [`SliceIter`].

// All methods boil down to just a few instructions.
// https://godbolt.org/z/KrsTz9478
#![expect(clippy::inline_always)]

use std::{
    mem,
    slice::{Iter, IterMut},
};

/// Extension trait for slice iterators.
#[expect(private_bounds)]
pub trait SliceIter<'slice, T>: ExactSizeIterator + AsRef<[T]> + Sealed {
    /// The type returned by `peek` method.
    type Peeked<'iter>
    where
        'slice: 'iter,
        Self: 'iter;

    /// Peek the next item in the iterator, without advancing it.
    fn peek(&self) -> Option<Self::Peeked<'_>>;

    /// Get the next item in the iterator, without advancing it.
    ///
    /// If testing for a specific value, `peek` is more efficient.
    /// `iter.peek() == Some(&b' ')` is less instructions than `iter.peek_copy() == Some(b' ')`.
    /// <https://godbolt.org/z/a4TzsdY95>
    ///
    /// Only available when `T` is `Copy`.
    fn peek_copy(&self) -> Option<T>
    where
        T: Copy;

    /// Get next item of an iterator that is not empty.
    ///
    /// # Panics
    /// Panics if the iterator is empty.
    fn next_unchecked(&mut self) -> Self::Item;

    /// Advance iterator by `count` items.
    ///
    /// # Panics
    /// Panics if iterator does not contain at least `count` more items.
    #[inline]
    fn advance(&mut self, count: usize) {
        assert!(self.len() >= count, "Iterator does not have `count` items remaining");
        self.advance_unchecked(count);
    }

    /// Advance iterator by `count` items.
    ///
    /// # Panics
    /// Panics if iterator does not contain at least `count` more items.
    fn advance_unchecked(&mut self, count: usize);

    /// Advance iterator to end.
    #[inline(always)]
    fn advance_to_end(&mut self) {
        // This function boils down to just setting the current pointer to the end - 2 instructions.
        // https://godbolt.org/z/EceneefEe
        self.advance(self.len());
    }

    /// Get pointer to next item in the iterator.
    ///
    /// Pointer is only valid to read an item from if iterator is not empty.
    #[inline]
    fn ptr(&self) -> *const T {
        let slice = self.as_ref();
        slice.as_ptr()
    }

    /// Get pointer to after last item in the iterator.
    ///
    /// Pointer is the end bound of the slice, so is not valid for reads.
    #[inline]
    fn end_ptr(&self) -> *const T {
        let slice = self.as_ref();
        slice.as_ptr_range().end
    }
}

impl<'slice, T: 'slice> SliceIter<'slice, T> for Iter<'slice, T> {
    // `peek` method returns a reference which borrows the slice, not the iterator
    type Peeked<'iter>
        = &'slice T
    where
        'slice: 'iter;

    /// Peek the next item in the iterator, without advancing it.
    #[inline(always)]
    fn peek(&self) -> Option<&'slice T> {
        self.clone().next()
    }

    /// Get the next item in the iterator, without advancing it.
    ///
    /// If testing for a specific value, `peek` is more efficient.
    /// `iter.peek() == Some(&b' ')` is less instructions than `iter.peek_copy() == Some(b' ')`.
    /// <https://godbolt.org/z/a4TzsdY95>
    ///
    /// Only available when `T` is `Copy`.
    #[inline]
    fn peek_copy(&self) -> Option<T>
    where
        T: Copy,
    {
        self.peek().copied()
    }

    #[inline(always)]
    fn next_unchecked(&mut self) -> &'slice T {
        self.next().expect("iterator is not empty")
    }

    #[inline(always)]
    fn advance_unchecked(&mut self, count: usize) {
        let slice = self.as_slice().get(count..).expect("iterator has `count` items remaining");
        *self = slice.iter();
    }
}

impl<'slice, T: 'slice> SliceIter<'slice, T> for IterMut<'slice, T> {
    // `peek` method returns a reference which borrows the iterator
    type Peeked<'iter>
        = &'iter T
    where
        'slice: 'iter;

    /// Peek the next item in the iterator, without advancing it.
    #[inline(always)]
    fn peek(&self) -> Option<&T> {
        self.as_slice().first()
    }

    /// Get the next item in the iterator, without advancing it.
    ///
    /// If testing for a specific value, `peek` is more efficient.
    /// `iter.peek() == Some(&b' ')` is less instructions than `iter.peek_copy() == Some(b' ')`.
    /// <https://godbolt.org/z/a4TzsdY95>
    ///
    /// Only available when `T` is `Copy`.
    #[inline]
    fn peek_copy(&self) -> Option<T>
    where
        T: Copy,
    {
        self.peek().copied()
    }

    #[inline(always)]
    fn next_unchecked(&mut self) -> &'slice mut T {
        self.next().expect("iterator is not empty")
    }

    #[inline(always)]
    fn advance_unchecked(&mut self, count: usize) {
        let slice = mem::take(self).into_slice();
        let rest = slice.get_mut(count..).expect("iterator has `count` items remaining");
        *self = rest.iter_mut();
    }
}

/// Private trait.
/// [`SliceIter`] extends `Sealed`, which prevents code outside this file implementing
/// `SliceIter` on other types.
trait Sealed {}

impl<'slice, T: 'slice> Sealed for Iter<'slice, T> {}

impl<'slice, T: 'slice> Sealed for IterMut<'slice, T> {}

#[cfg(test)]
mod test_iter {
    use super::*;

    #[test]
    fn peek() {
        let mut iter = [11, 22, 33].iter();
        assert_eq!(iter.peek(), Some(&11));
        assert_eq!(iter.peek(), Some(&11));
        let first = iter.peek();

        iter.next();
        assert_eq!(iter.peek(), Some(&22));
        let second = iter.peek();

        iter.next();
        assert_eq!(iter.peek(), Some(&33));
        let third = iter.peek();

        iter.next();
        assert_eq!(iter.peek(), None);

        // Check peeked values don't borrow iterator
        assert_eq!(first, Some(&11));
        assert_eq!(second, Some(&22));
        assert_eq!(third, Some(&33));
    }

    #[test]
    fn peek_copy() {
        let mut iter = [11, 22, 33].iter();
        assert_eq!(iter.peek_copy(), Some(11));
        assert_eq!(iter.peek_copy(), Some(11));

        iter.next();
        assert_eq!(iter.peek_copy(), Some(22));

        iter.next();
        assert_eq!(iter.peek_copy(), Some(33));

        iter.next();
        assert_eq!(iter.peek_copy(), None);
    }

    #[test]
    fn next_unchecked() {
        let mut iter = [11, 22, 33].iter();
        assert_eq!(iter.next_unchecked(), &11);
        assert_eq!(iter.next_unchecked(), &22);
        assert_eq!(iter.next_unchecked(), &33);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn advance() {
        let mut iter = [11, 22, 33].iter();
        iter.advance(0);
        assert_eq!(iter.next(), Some(&11));

        let mut iter = [11, 22, 33].iter();
        iter.advance(1);
        assert_eq!(iter.next(), Some(&22));

        let mut iter = [11, 22, 33].iter();
        iter.advance(2);
        assert_eq!(iter.next(), Some(&33));

        let mut iter = [11, 22, 33].iter();
        iter.advance(3);
        assert_eq!(iter.next(), None);
    }

    #[test]
    #[should_panic(expected = "Iterator does not have `count` items remaining")]
    fn advance_panic() {
        let mut iter = [11, 22, 33].iter();
        iter.advance(4);
    }

    #[test]
    fn advance_unchecked() {
        let mut iter = [11, 22, 33].iter();
        iter.advance_unchecked(0);
        assert_eq!(iter.next(), Some(&11));

        let mut iter = [11, 22, 33].iter();
        iter.advance_unchecked(1);
        assert_eq!(iter.next(), Some(&22));

        let mut iter = [11, 22, 33].iter();
        iter.advance_unchecked(2);
        assert_eq!(iter.next(), Some(&33));

        let mut iter = [11, 22, 33].iter();
        iter.advance_unchecked(3);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn advance_to_end() {
        let arr = [11, 22, 33];

        let mut iter = arr.iter();
        iter.advance_to_end();
        assert_eq!(iter.next(), None);

        let mut iter = arr.iter();
        assert_eq!(iter.next(), Some(&11));
        iter.advance_to_end();
        assert_eq!(iter.next(), None);

        let mut iter = arr.iter();
        assert_eq!(iter.next(), Some(&11));
        assert_eq!(iter.next(), Some(&22));
        iter.advance_to_end();
        assert_eq!(iter.next(), None);

        let mut iter = arr.iter();
        assert_eq!(iter.next(), Some(&11));
        assert_eq!(iter.next(), Some(&22));
        assert_eq!(iter.next(), Some(&33));
        assert_eq!(iter.next(), None);
        iter.advance_to_end();
        assert_eq!(iter.next(), None);

        let empty_arr: [u32; 0] = [];
        let mut iter = empty_arr.iter();
        iter.advance_to_end();
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn ptr() {
        let slice = [11u32, 22, 33];
        let start_addr = slice.as_ptr() as usize;

        let mut iter = slice.iter();
        assert_eq!(iter.ptr() as usize, start_addr);

        iter.next();
        assert_eq!(iter.ptr() as usize, start_addr + size_of::<u32>());

        iter.next();
        assert_eq!(iter.ptr() as usize, start_addr + size_of::<u32>() * 2);

        iter.next();
        assert_eq!(iter.ptr() as usize, start_addr + size_of::<u32>() * 3);

        iter.next();
        assert_eq!(iter.ptr() as usize, start_addr + size_of::<u32>() * 3);
    }

    #[test]
    fn end_ptr() {
        let slice = [11u32, 22, 33];
        let end_addr = slice.as_ptr() as usize + size_of::<u32>() * 3;

        let mut iter = slice.iter();
        assert_eq!(iter.end_ptr() as usize, end_addr);

        iter.next();
        assert_eq!(iter.end_ptr() as usize, end_addr);

        iter.next();
        assert_eq!(iter.end_ptr() as usize, end_addr);

        iter.next();
        assert_eq!(iter.end_ptr() as usize, end_addr);

        iter.next();
        assert_eq!(iter.end_ptr() as usize, end_addr);
    }
}

#[cfg(test)]
mod test_iter_mut {
    use super::*;

    #[test]
    fn peek() {
        let mut arr = [11, 22, 33];
        let mut iter = arr.iter_mut();

        assert_eq!(iter.peek(), Some(&11));
        assert_eq!(iter.peek(), Some(&11));

        iter.next();
        assert_eq!(iter.peek(), Some(&22));

        iter.next();
        assert_eq!(iter.peek(), Some(&33));

        iter.next();
        assert_eq!(iter.peek(), None);
    }

    #[test]
    fn peek_copy() {
        let mut arr = [11, 22, 33];
        let mut iter = arr.iter_mut();

        assert_eq!(iter.peek_copy(), Some(11));
        assert_eq!(iter.peek_copy(), Some(11));

        iter.next();
        assert_eq!(iter.peek_copy(), Some(22));

        iter.next();
        assert_eq!(iter.peek_copy(), Some(33));

        iter.next();
        assert_eq!(iter.peek_copy(), None);
    }

    #[test]
    fn next_unchecked() {
        let mut arr = [11, 22, 33];
        let mut iter = arr.iter_mut();
        assert_eq!(iter.next_unchecked(), &11);
        assert_eq!(iter.next_unchecked(), &22);
        assert_eq!(iter.next_unchecked(), &33);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn advance() {
        let mut arr = [11, 22, 33];

        let mut iter = arr.iter_mut();
        iter.advance(0);
        assert_eq!(iter.next(), Some(&mut 11));

        let mut iter = arr.iter_mut();
        iter.advance(1);
        assert_eq!(iter.next(), Some(&mut 22));

        let mut iter = arr.iter_mut();
        iter.advance(2);
        assert_eq!(iter.next(), Some(&mut 33));

        let mut iter = arr.iter_mut();
        iter.advance(3);
        assert_eq!(iter.next(), None);
    }

    #[test]
    #[should_panic(expected = "Iterator does not have `count` items remaining")]
    fn advance_panic() {
        let mut arr = [11, 22, 33];
        let mut iter = arr.iter_mut();
        iter.advance(4);
    }

    #[test]
    fn advance_unchecked() {
        let mut arr = [11, 22, 33];

        let mut iter = arr.iter_mut();
        iter.advance_unchecked(0);
        assert_eq!(iter.next(), Some(&mut 11));

        let mut iter = arr.iter_mut();
        iter.advance_unchecked(1);
        assert_eq!(iter.next(), Some(&mut 22));

        let mut iter = arr.iter_mut();
        iter.advance_unchecked(2);
        assert_eq!(iter.next(), Some(&mut 33));

        let mut iter = arr.iter_mut();
        iter.advance_unchecked(3);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn advance_to_end() {
        let mut arr = [11, 22, 33];

        let mut iter = arr.iter_mut();
        iter.advance_to_end();
        assert_eq!(iter.next(), None);

        let mut iter = arr.iter_mut();
        assert_eq!(iter.next(), Some(&mut 11));
        iter.advance_to_end();
        assert_eq!(iter.next(), None);

        let mut iter = arr.iter_mut();
        assert_eq!(iter.next(), Some(&mut 11));
        assert_eq!(iter.next(), Some(&mut 22));
        iter.advance_to_end();
        assert_eq!(iter.next(), None);

        let mut iter = arr.iter_mut();
        assert_eq!(iter.next(), Some(&mut 11));
        assert_eq!(iter.next(), Some(&mut 22));
        assert_eq!(iter.next(), Some(&mut 33));
        assert_eq!(iter.next(), None);
        iter.advance_to_end();
        assert_eq!(iter.next(), None);

        let mut empty_arr: [u32; 0] = [];
        let mut iter = empty_arr.iter_mut();
        iter.advance_to_end();
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn ptr() {
        let mut slice = [11u32, 22, 33];
        let start_addr = slice.as_ptr() as usize;

        let mut iter = slice.iter_mut();
        assert_eq!(iter.ptr() as usize, start_addr);

        iter.next();
        assert_eq!(iter.ptr() as usize, start_addr + size_of::<u32>());

        iter.next();
        assert_eq!(iter.ptr() as usize, start_addr + size_of::<u32>() * 2);

        iter.next();
        assert_eq!(iter.ptr() as usize, start_addr + size_of::<u32>() * 3);

        iter.next();
        assert_eq!(iter.ptr() as usize, start_addr + size_of::<u32>() * 3);
    }

    #[test]
    fn end_ptr() {
        let mut slice = [11u32, 22, 33];
        let end_addr = slice.as_ptr() as usize + size_of::<u32>() * 3;

        let mut iter = slice.iter_mut();
        assert_eq!(iter.end_ptr() as usize, end_addr);

        iter.next();
        assert_eq!(iter.end_ptr() as usize, end_addr);

        iter.next();
        assert_eq!(iter.end_ptr() as usize, end_addr);

        iter.next();
        assert_eq!(iter.end_ptr() as usize, end_addr);

        iter.next();
        assert_eq!(iter.end_ptr() as usize, end_addr);
    }
}
