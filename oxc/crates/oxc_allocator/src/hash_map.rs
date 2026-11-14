use std::{
    fmt,
    hash::{BuildHasher, Hash},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use rustc_hash::FxBuildHasher;

use crate::Allocator;

pub use hashbrown::{Equivalent, hash_map::{Entry, IntoKeys, IntoValues, OccupiedEntry, VacantEntry}};

/// A hash map tied to the lifetime of an [`Allocator`], backed by `hashbrown`
/// (`std`'s own hash table) on the global allocator.
pub struct HashMap<'alloc, K, V, S = FxBuildHasher>(pub(crate) hashbrown::HashMap<K, V, S>, PhantomData<&'alloc ()>);

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for HashMap<'_, K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter()).finish()
    }
}

impl<'alloc, K, V, S> HashMap<'alloc, K, V, S> {
    #[inline]
    pub fn with_hasher_in(hasher: S, _allocator: &'alloc Allocator) -> Self {
        Self(hashbrown::HashMap::with_hasher(hasher), PhantomData)
    }

    #[inline]
    pub fn with_capacity_and_hasher_in(capacity: usize, hasher: S, _allocator: &'alloc Allocator) -> Self {
        Self(hashbrown::HashMap::with_capacity_and_hasher(capacity, hasher), PhantomData)
    }

    #[inline]
    pub fn into_keys(self) -> IntoKeys<K, V> {
        self.0.into_keys()
    }

    #[inline]
    pub fn into_values(self) -> IntoValues<K, V> {
        self.0.into_values()
    }
}

impl<K, V, S: Default> Default for HashMap<'_, K, V, S> {
    /// An empty map. It needs no `Allocator`: the map's storage is on the global heap.
    #[inline]
    fn default() -> Self {
        Self(hashbrown::HashMap::with_hasher(S::default()), PhantomData)
    }
}

impl<'alloc, K, V, S: Default> HashMap<'alloc, K, V, S> {
    #[inline]
    pub fn new_in(_allocator: &'alloc Allocator) -> Self {
        Self(hashbrown::HashMap::with_hasher(S::default()), PhantomData)
    }

    #[inline]
    pub fn with_capacity_in(capacity: usize, _allocator: &'alloc Allocator) -> Self {
        Self(hashbrown::HashMap::with_capacity_and_hasher(capacity, S::default()), PhantomData)
    }
}

impl<'alloc, K: Eq + Hash, V, S: BuildHasher + Default> HashMap<'alloc, K, V, S> {
    pub fn from_iter_in<I: IntoIterator<Item = (K, V)>>(iter: I, _allocator: &'alloc Allocator) -> Self {
        Self(iter.into_iter().collect(), PhantomData)
    }
}

impl<K, V, S> Deref for HashMap<'_, K, V, S> {
    type Target = hashbrown::HashMap<K, V, S>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K, V, S> DerefMut for HashMap<'_, K, V, S> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<K, V, S> IntoIterator for HashMap<'_, K, V, S> {
    type IntoIter = <hashbrown::HashMap<K, V, S> as IntoIterator>::IntoIter;
    type Item = (K, V);

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'i, K, V, S> IntoIterator for &'i HashMap<'_, K, V, S> {
    type IntoIter = <&'i hashbrown::HashMap<K, V, S> as IntoIterator>::IntoIter;
    type Item = <&'i hashbrown::HashMap<K, V, S> as IntoIterator>::Item;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<K: Eq + Hash, V, S: BuildHasher> PartialEq for HashMap<'_, K, V, S>
where
    V: PartialEq + Eq,
{
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<K: Eq + Hash, V, S: BuildHasher> Eq for HashMap<'_, K, V, S> where V: PartialEq + Eq, {}
