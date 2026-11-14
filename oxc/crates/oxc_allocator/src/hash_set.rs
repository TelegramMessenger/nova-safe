use std::{
    fmt,
    hash::{BuildHasher, Hash},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use rustc_hash::FxBuildHasher;

use crate::Allocator;

pub use hashbrown::hash_set::Entry;

use crate::HashMap;

/// A hash set tied to the lifetime of an [`Allocator`], backed by `hashbrown`
/// (`std`'s own hash table) on the global allocator.
pub struct HashSet<'alloc, T, S = FxBuildHasher>(pub(crate) hashbrown::HashSet<T, S>, PhantomData<&'alloc ()>);

impl<T: fmt::Debug, S> fmt::Debug for HashSet<'_, T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.iter()).finish()
    }
}

impl<'alloc, T, S> HashSet<'alloc, T, S> {
    #[inline]
    pub fn with_hasher_in(hasher: S, _allocator: &'alloc Allocator) -> Self {
        Self(hashbrown::HashSet::with_hasher(hasher), PhantomData)
    }

    #[inline]
    pub fn with_capacity_and_hasher_in(capacity: usize, hasher: S, _allocator: &'alloc Allocator) -> Self {
        Self(hashbrown::HashSet::with_capacity_and_hasher(capacity, hasher), PhantomData)
    }
}

impl<'alloc, T, S: Default> HashSet<'alloc, T, S> {
    #[inline]
    pub fn new_in(_allocator: &'alloc Allocator) -> Self {
        Self(hashbrown::HashSet::with_hasher(S::default()), PhantomData)
    }

    #[inline]
    pub fn with_capacity_in(capacity: usize, _allocator: &'alloc Allocator) -> Self {
        Self(hashbrown::HashSet::with_capacity_and_hasher(capacity, S::default()), PhantomData)
    }
}

impl<'alloc, T: Eq + Hash, S: BuildHasher + Default> HashSet<'alloc, T, S> {
    pub fn from_iter_in<I: IntoIterator<Item = T>>(iter: I, _allocator: &'alloc Allocator) -> Self {
        Self(iter.into_iter().collect(), PhantomData)
    }
}

impl<T, S> Deref for HashSet<'_, T, S> {
    type Target = hashbrown::HashSet<T, S>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, S> DerefMut for HashSet<'_, T, S> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T, S> IntoIterator for HashSet<'_, T, S> {
    type IntoIter = <hashbrown::HashSet<T, S> as IntoIterator>::IntoIter;
    type Item = T;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'alloc, T, S> From<HashMap<'alloc, T, (), S>> for HashSet<'alloc, T, S> {
    fn from(map: HashMap<'alloc, T, (), S>) -> Self {
        Self(hashbrown::HashSet::from(map.0), PhantomData)
    }
}

impl<'i, T, S> IntoIterator for &'i HashSet<'_, T, S> {
    type IntoIter = <&'i hashbrown::HashSet<T, S> as IntoIterator>::IntoIter;
    type Item = <&'i hashbrown::HashSet<T, S> as IntoIterator>::Item;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Eq + Hash, S: BuildHasher> PartialEq for HashSet<'_, T, S>
where
    T: Eq,
{
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq + Hash, S: BuildHasher> Eq for HashSet<'_, T, S> where T: Eq, {}
