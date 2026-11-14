use std::{
    fmt::{self, Debug},
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops,
    slice::SliceIndex,
};

use crate::{Allocator, Box};

/// A vector tied to the lifetime of an [`Allocator`], backed by `std`'s
/// vector.
#[derive(PartialEq, Eq)]
pub struct Vec<'alloc, T>(std::vec::Vec<T>, PhantomData<&'alloc ()>);

impl<'alloc, T> Vec<'alloc, T> {
    #[inline]
    pub fn new_in(_allocator: &'alloc Allocator) -> Self {
        Self(std::vec::Vec::new(), PhantomData)
    }

    #[inline]
    pub fn with_capacity_in(capacity: usize, _allocator: &'alloc Allocator) -> Self {
        Self(std::vec::Vec::with_capacity(capacity), PhantomData)
    }

    #[inline]
    pub fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, _allocator: &'alloc Allocator) -> Self {
        Self(iter.into_iter().collect(), PhantomData)
    }

    #[inline]
    pub fn from_array_in<const N: usize>(array: [T; N], _allocator: &'alloc Allocator) -> Self {
        Self(std::vec::Vec::from(array), PhantomData)
    }

    #[inline]
    pub fn from_std(vec: std::vec::Vec<T>) -> Self {
        Self(vec, PhantomData)
    }

    #[inline]
    pub fn into_std(self) -> std::vec::Vec<T> {
        self.0
    }

    #[inline]
    pub fn push_fast(&mut self, value: T) {
        self.0.push(value);
    }

    #[inline]
    pub fn into_boxed_slice(self) -> Box<'alloc, [T]> {
        Box::from_std(self.0.into_boxed_slice())
    }
}

impl<T> ops::Deref for Vec<'_, T> {
    type Target = std::vec::Vec<T>;

    #[inline]
    fn deref(&self) -> &std::vec::Vec<T> {
        &self.0
    }
}

impl<T> ops::DerefMut for Vec<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut std::vec::Vec<T> {
        &mut self.0
    }
}

impl<T> IntoIterator for Vec<'_, T> {
    type IntoIter = std::vec::IntoIter<T>;
    type Item = T;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'i, T> IntoIterator for &'i Vec<'_, T> {
    type IntoIter = std::slice::Iter<'i, T>;
    type Item = &'i T;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'i, T> IntoIterator for &'i mut Vec<'_, T> {
    type IntoIter = std::slice::IterMut<'i, T>;
    type Item = &'i mut T;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}

impl<T, I: SliceIndex<[T]>> ops::Index<I> for Vec<'_, T> {
    type Output = I::Output;

    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        self.0.index(index)
    }
}

impl<T, I: SliceIndex<[T]>> ops::IndexMut<I> for Vec<'_, T> {
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        self.0.index_mut(index)
    }
}

impl<'a, T: 'a> From<Vec<'a, T>> for Box<'a, [T]> {
    #[inline]
    fn from(v: Vec<'a, T>) -> Box<'a, [T]> {
        v.into_boxed_slice()
    }
}

impl<T: Hash> Hash for Vec<'_, T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T: Debug> Debug for Vec<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("Vec").field(&self.0).finish()
    }
}
