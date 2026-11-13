// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Typed views over the bytes of an ArrayBuffer.
//!
//! The views never reinterpret the bytes as `[T]`: they hold the bytes as
//! `[[u8; size_of::<T>()]]`, which has the alignment of `u8`, and every
//! element is read and written through `from_ne_bytes` / `to_ne_bytes`, so
//! the bytes need no alignment and any bit pattern is a valid value.

use core::{marker::PhantomData, ops::Range};

use crate::ecmascript::{ElementBytes, StorageBytes, Viewable};

/// A shared view of bytes as a sequence of T in native byte order.
///
/// Trailing bytes that do not make up a whole T are not part of the view.
pub(crate) struct ViewableSlice<'a, T: Viewable> {
    elements: &'a [ElementBytes<T>],
    _type: PhantomData<T>,
}

impl<T: Viewable> Clone for ViewableSlice<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Viewable> Copy for ViewableSlice<'_, T> {}

impl<'a, T: Viewable> ViewableSlice<'a, T> {
    /// View `bytes` as T's; trailing bytes that do not make up a whole T are
    /// left out.
    #[inline(always)]
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            elements: T::Storage::as_elements(bytes),
            _type: PhantomData,
        }
    }

    /// A view of no elements.
    #[inline(always)]
    pub(crate) fn empty() -> Self {
        Self {
            elements: &[],
            _type: PhantomData,
        }
    }

    /// Number of elements in the view.
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.elements.len()
    }

    /// Read the element at `index`, or None if it is not within the view.
    #[inline(always)]
    pub(crate) fn get(&self, index: usize) -> Option<T> {
        self.elements
            .get(index)
            .map(|&bytes| T::from_ne_array(bytes))
    }

    /// The elements in `range` as a view.
    ///
    /// Panics like slice indexing if the range is not within the view.
    #[inline(always)]
    pub(crate) fn slice(self, range: Range<usize>) -> Self {
        Self {
            elements: &self.elements[range],
            _type: PhantomData,
        }
    }

    /// Iterate over the elements of the view.
    #[inline(always)]
    pub(crate) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = T> + ExactSizeIterator + use<'a, T> {
        self.elements.iter().map(|&bytes| T::from_ne_array(bytes))
    }

    /// Copy the elements into a Vec.
    pub(crate) fn to_vec(self) -> Vec<T> {
        self.iter().collect()
    }
}

/// An exclusive view of bytes as a sequence of T in native byte order.
///
/// Trailing bytes that do not make up a whole T are not part of the view.
pub(crate) struct ViewableSliceMut<'a, T: Viewable> {
    elements: &'a mut [ElementBytes<T>],
    _type: PhantomData<T>,
}

impl<'a, T: Viewable> ViewableSliceMut<'a, T> {
    /// View `bytes` as T's; trailing bytes that do not make up a whole T are
    /// left out.
    #[inline(always)]
    pub(crate) fn new(bytes: &'a mut [u8]) -> Self {
        Self {
            elements: T::Storage::as_elements_mut(bytes),
            _type: PhantomData,
        }
    }

    /// A view of no elements.
    #[inline(always)]
    pub(crate) fn empty() -> Self {
        Self {
            elements: &mut [],
            _type: PhantomData,
        }
    }

    /// Number of elements in the view.
    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.elements.len()
    }

    /// A shared view of the same elements.
    #[inline(always)]
    pub(crate) fn as_slice(&self) -> ViewableSlice<'_, T> {
        ViewableSlice {
            elements: self.elements,
            _type: PhantomData,
        }
    }

    /// Write the element at `index`.
    ///
    /// Panics like slice indexing if `index` is not within the view.
    #[inline(always)]
    pub(crate) fn set(&mut self, index: usize, value: T) {
        self.elements[index] = value.to_ne_array();
    }

    /// The elements in `range` as a view.
    ///
    /// Panics like slice indexing if the range is not within the view.
    #[inline(always)]
    pub(crate) fn into_slice(self, range: Range<usize>) -> Self {
        Self {
            elements: &mut self.elements[range],
            _type: PhantomData,
        }
    }

    /// Write `value` into every element.
    #[inline]
    pub(crate) fn fill(&mut self, value: T) {
        self.elements.fill(value.to_ne_array());
    }

    /// Reverse the order of the elements.
    #[inline]
    pub(crate) fn reverse(&mut self) {
        if size_of::<ElementBytes<T>>() == 1 {
            T::Storage::flatten_mut(self.elements).reverse();
        } else {
            let len = self.elements.len();
            let (front, back) = self.elements.split_at_mut(len / 2);
            for (a, b) in front.iter_mut().zip(back.iter_mut().rev()) {
                (*a, *b) = (*b, *a);
            }
        }
    }

    /// Copy the elements in `src` to the elements starting at `dest`, as
    /// [`slice::copy_within`] does.
    ///
    /// Panics if either range is not within the view.
    #[inline]
    pub(crate) fn copy_within(&mut self, src: Range<usize>, dest: usize) {
        self.elements.copy_within(src, dest);
    }

    /// Copy all elements of `src`, which must have the same length.
    ///
    /// Panics if the lengths differ.
    #[inline]
    pub(crate) fn copy_from(&mut self, src: ViewableSlice<T>) {
        self.elements.copy_from_slice(src.elements);
    }

    /// Write all elements from `values`, which must have the same length.
    ///
    /// Panics if the lengths differ.
    pub(crate) fn copy_from_values(&mut self, values: &[T]) {
        assert_eq!(
            self.len(),
            values.len(),
            "source and destination have different lengths"
        );
        self.write_from(values.iter().copied());
    }

    /// Write each element from the value `values` yields for it.
    ///
    /// Elements past the end of `values` are left as they are.
    #[inline]
    pub(crate) fn write_from(&mut self, values: impl Iterator<Item = T>) {
        for (element, value) in self.elements.iter_mut().zip(values) {
            *element = value.to_ne_array();
        }
    }
}
