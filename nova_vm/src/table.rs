// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A hash table whose callers supply the hash of each element, for tables
//! whose elements are hashed and compared with outside data (Map and Set
//! entries, interned strings, shape transitions). It has the subset of
//! `hashbrown::HashTable`'s API the engine uses.
//!
//! The layout is a SwissTable without SIMD, in aligned groups: each group
//! holds eight control bytes in a `u64` (empty, deleted, or the top 7 bits
//! of the element's hash) and the eight slots they describe, so a table is
//! one allocation and the table value is as small as hashbrown's. A lookup
//! probes whole groups (triangular steps over a power-of-two group count)
//! and stops at the first group with an empty byte; at most 7 of every 8
//! slots are ever used, so every table has an empty byte to stop at.

use core::fmt;

const GROUP: usize = 8;
const EMPTY: u8 = 0xFF;
const DELETED: u8 = 0x80;
const LSB: u64 = 0x0101_0101_0101_0101;
const MSB: u64 = 0x8080_8080_8080_8080;
const ALL_EMPTY: u64 = u64::MAX;

#[inline(always)]
fn h2(hash: u64) -> u8 {
    (hash >> 57) as u8
}

#[inline(always)]
fn match_byte(group: u64, byte: u8) -> u64 {
    let cmp = group ^ (LSB * u64::from(byte));
    cmp.wrapping_sub(LSB) & !cmp & MSB
}

#[inline(always)]
fn match_empty(group: u64) -> u64 {
    group & (group << 1) & MSB
}

#[inline(always)]
fn match_empty_or_deleted(group: u64) -> u64 {
    group & MSB
}

#[inline(always)]
fn lowest(bits: u64) -> usize {
    (bits.trailing_zeros() / 8) as usize
}

fn capacity_to_groups(capacity: usize) -> usize {
    if capacity == 0 {
        return 0;
    }
    capacity
        .div_ceil(GROUP - 1)
        .checked_next_power_of_two()
        .expect("hash table capacity overflow")
}

#[inline(always)]
fn groups_capacity(groups: usize) -> usize {
    groups * (GROUP - 1)
}

#[derive(Clone)]
struct Group<T> {
    ctrl: u64,
    slots: [Option<T>; GROUP],
}

impl<T> Group<T> {
    #[inline]
    fn new() -> Self {
        Self {
            ctrl: ALL_EMPTY,
            slots: [const { None }; GROUP],
        }
    }

    #[inline(always)]
    fn set_ctrl(&mut self, slot: usize, byte: u8) {
        let shift = slot * 8;
        self.ctrl = (self.ctrl & !(0xFF << shift)) | (u64::from(byte) << shift);
    }
}

pub(crate) struct HashTable<T> {
    groups: Vec<Group<T>>,
    items: u32,
    growth_left: u32,
}

impl<T> HashTable<T> {
    pub(crate) const fn new() -> Self {
        Self {
            groups: Vec::new(),
            items: 0,
            growth_left: 0,
        }
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let mut table = Self::new();
        table.allocate(capacity_to_groups(capacity));
        table
    }

    fn allocate(&mut self, groups: usize) {
        let capacity =
            u32::try_from(groups_capacity(groups)).expect("hash table capacity overflow");
        let mut storage = Vec::with_capacity(groups);
        storage.resize_with(groups, Group::new);
        self.groups = storage;
        self.items = 0;
        self.growth_left = capacity;
    }

    #[inline(always)]
    pub(crate) fn len(&self) -> usize {
        self.items as usize
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.items == 0
    }

    /// Takes every element out, in table order, and leaves the table empty
    /// with its allocation.
    pub(crate) fn drain(&mut self) -> std::vec::IntoIter<T> {
        let mut out = Vec::with_capacity(self.len());
        for group in &mut self.groups {
            out.extend(group.slots.iter_mut().filter_map(Option::take));
        }
        self.clear();
        out.into_iter()
    }

    pub(crate) fn clear(&mut self) {
        let capacity = groups_capacity(self.groups.len()) as u32;
        if self.items == 0 && self.growth_left == capacity {
            return;
        }
        for group in &mut self.groups {
            *group = Group::new();
        }
        self.items = 0;
        self.growth_left = capacity;
    }

    #[inline]
    fn find_index(&self, hash: u64, mut eq: impl FnMut(&T) -> bool) -> Option<usize> {
        if self.groups.is_empty() {
            return None;
        }
        let tag = h2(hash);
        let mask = self.groups.len() - 1;
        let mut g = hash as usize & mask;
        let mut stride = 0;
        loop {
            let group = &self.groups[g];
            let mut matches = match_byte(group.ctrl, tag);
            while matches != 0 {
                let slot = lowest(matches);
                if let Some(value) = &group.slots[slot]
                    && eq(value)
                {
                    return Some(g * GROUP + slot);
                }
                matches &= matches - 1;
            }
            if match_empty(group.ctrl) != 0 {
                return None;
            }
            stride += 1;
            g = (g + stride) & mask;
        }
    }

    #[inline]
    fn find_insert_slot(&self, hash: u64) -> usize {
        let mask = self.groups.len() - 1;
        let mut g = hash as usize & mask;
        let mut stride = 0;
        loop {
            let free = match_empty_or_deleted(self.groups[g].ctrl);
            if free != 0 {
                return g * GROUP + lowest(free);
            }
            stride += 1;
            g = (g + stride) & mask;
        }
    }

    #[inline]
    fn insert_no_grow(&mut self, hash: u64, value: T) -> usize {
        let index = self.find_insert_slot(hash);
        let group = &mut self.groups[index / GROUP];
        let slot = index % GROUP;
        if (group.ctrl >> (slot * 8)) as u8 == EMPTY {
            self.growth_left = self
                .growth_left
                .checked_sub(1)
                .expect("hash table insert without room");
        }
        group.set_ctrl(slot, h2(hash));
        group.slots[slot] = Some(value);
        self.items += 1;
        index
    }

    fn erase(&mut self, index: usize) -> T {
        let group = &mut self.groups[index / GROUP];
        let slot = index % GROUP;
        let value = group.slots[slot].take().expect("erased an empty bucket");
        if match_empty(group.ctrl) != 0 {
            group.set_ctrl(slot, EMPTY);
            self.growth_left += 1;
        } else {
            group.set_ctrl(slot, DELETED);
        }
        self.items -= 1;
        value
    }

    fn resize(&mut self, capacity: usize, hasher: impl Fn(&T) -> u64) {
        let old = core::mem::take(&mut self.groups);
        self.allocate(capacity_to_groups(capacity));
        for group in old {
            for value in group.slots.into_iter().flatten() {
                let hash = hasher(&value);
                self.insert_no_grow(hash, value);
            }
        }
    }

    #[cold]
    #[inline(never)]
    fn reserve_rehash(&mut self, additional: usize, hasher: impl Fn(&T) -> u64) {
        let needed = (self.items as usize)
            .checked_add(additional)
            .expect("hash table capacity overflow");
        let full_capacity = groups_capacity(self.groups.len());
        if needed <= full_capacity / 2 {
            self.resize(full_capacity, hasher);
        } else {
            self.resize(needed.max(full_capacity + 1), hasher);
        }
    }

    #[inline]
    pub(crate) fn reserve(&mut self, additional: usize, hasher: impl Fn(&T) -> u64) {
        if additional > self.growth_left as usize {
            self.reserve_rehash(additional, hasher);
        }
    }

    #[inline]
    pub(crate) fn find(&self, hash: u64, eq: impl FnMut(&T) -> bool) -> Option<&T> {
        let index = self.find_index(hash, eq)?;
        self.groups[index / GROUP].slots[index % GROUP].as_ref()
    }

    #[inline]
    pub(crate) fn find_entry(
        &mut self,
        hash: u64,
        eq: impl FnMut(&T) -> bool,
    ) -> Result<OccupiedEntry<'_, T>, AbsentEntry> {
        match self.find_index(hash, eq) {
            Some(index) => Ok(OccupiedEntry { table: self, index }),
            None => Err(AbsentEntry),
        }
    }

    #[inline]
    pub(crate) fn entry(
        &mut self,
        hash: u64,
        eq: impl FnMut(&T) -> bool,
        hasher: impl Fn(&T) -> u64,
    ) -> Entry<'_, T> {
        match self.find_index(hash, eq) {
            Some(index) => Entry::Occupied(OccupiedEntry { table: self, index }),
            None => {
                self.reserve(1, hasher);
                Entry::Vacant(VacantEntry { table: self, hash })
            }
        }
    }

    #[inline]
    pub(crate) fn insert_unique(
        &mut self,
        hash: u64,
        value: T,
        hasher: impl Fn(&T) -> u64,
    ) -> OccupiedEntry<'_, T> {
        self.reserve(1, hasher);
        let index = self.insert_no_grow(hash, value);
        OccupiedEntry { table: self, index }
    }

    pub(crate) fn retain(&mut self, mut f: impl FnMut(&mut T) -> bool) {
        if self.items == 0 {
            return;
        }
        for index in 0..self.groups.len() * GROUP {
            let keep = match &mut self.groups[index / GROUP].slots[index % GROUP] {
                Some(value) => f(value),
                None => continue,
            };
            if !keep {
                self.erase(index);
            }
        }
    }

    pub(crate) fn iter(&self) -> Iter<'_, T> {
        Iter {
            groups: self.groups.iter(),
            slots: [].iter(),
        }
    }

    pub(crate) fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            groups: self.groups.iter_mut(),
            slots: [].iter_mut(),
        }
    }
}

pub(crate) struct Iter<'a, T> {
    groups: core::slice::Iter<'a, Group<T>>,
    slots: core::slice::Iter<'a, Option<T>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<&'a T> {
        loop {
            for slot in self.slots.by_ref() {
                if let Some(value) = slot {
                    return Some(value);
                }
            }
            self.slots = self.groups.next()?.slots.iter();
        }
    }
}

pub(crate) struct IterMut<'a, T> {
    groups: core::slice::IterMut<'a, Group<T>>,
    slots: core::slice::IterMut<'a, Option<T>>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;

    #[inline]
    fn next(&mut self) -> Option<&'a mut T> {
        loop {
            for slot in self.slots.by_ref() {
                if let Some(value) = slot {
                    return Some(value);
                }
            }
            self.slots = self.groups.next()?.slots.iter_mut();
        }
    }
}

impl<T> Default for HashTable<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Clone for HashTable<T> {
    fn clone(&self) -> Self {
        Self {
            groups: self.groups.clone(),
            items: self.items,
            growth_left: self.growth_left,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for HashTable<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<'a, T> IntoIterator for &'a HashTable<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub(crate) enum Entry<'a, T> {
    Occupied(OccupiedEntry<'a, T>),
    Vacant(VacantEntry<'a, T>),
}

pub(crate) struct OccupiedEntry<'a, T> {
    table: &'a mut HashTable<T>,
    index: usize,
}

impl<'a, T> OccupiedEntry<'a, T> {
    #[inline]
    pub(crate) fn get(&self) -> &T {
        self.table.groups[self.index / GROUP].slots[self.index % GROUP]
            .as_ref()
            .expect("occupied entry is full")
    }

    pub(crate) fn remove(self) -> T {
        self.table.erase(self.index)
    }
}

pub(crate) struct VacantEntry<'a, T> {
    table: &'a mut HashTable<T>,
    hash: u64,
}

impl<'a, T> VacantEntry<'a, T> {
    #[inline]
    pub(crate) fn insert(self, value: T) -> OccupiedEntry<'a, T> {
        let index = self.table.insert_no_grow(self.hash, value);
        OccupiedEntry {
            table: self.table,
            index,
        }
    }
}

pub(crate) struct AbsentEntry;

pub(crate) mod hash_table {
    pub(crate) use super::Entry;
}

#[cfg(test)]
mod tests {

    #[test]
    fn drain_takes_every_element_and_empties_the_table() {
        let mut table = HashTable::new();
        let hash = |v: &u64| v.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        for v in 0..100u64 {
            table.insert_unique(hash(&v), v, hash);
        }
        let mut drained: Vec<u64> = table.drain().collect();
        drained.sort_unstable();
        assert_eq!(drained, (0..100).collect::<Vec<_>>());
        assert!(table.is_empty());
        assert!(table.find(hash(&7), |v| *v == 7).is_none());
        table.insert_unique(hash(&7), 7, hash);
        assert_eq!(table.find(hash(&7), |v| *v == 7), Some(&7));
    }

    use super::HashTable;
    use std::hash::{BuildHasher, RandomState};

    #[test]
    fn table_is_as_small_as_hashbrowns() {
        assert_eq!(core::mem::size_of::<HashTable<u32>>(), 32);
    }

    #[test]
    fn insert_find_remove_matches_a_model() {
        let state = RandomState::new();
        let hash = |v: &u32| state.hash_one(*v);
        let mut table: HashTable<u32> = HashTable::new();
        let mut model = std::collections::HashSet::new();
        let mut x: u32 = 12345;
        for step in 0..200_000u32 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let key = x % 5000;
            match step % 3 {
                0 | 1 => {
                    let inserted = match table.entry(hash(&key), |v| *v == key, hash) {
                        super::Entry::Occupied(_) => false,
                        super::Entry::Vacant(entry) => {
                            entry.insert(key);
                            true
                        }
                    };
                    assert_eq!(inserted, model.insert(key));
                }
                _ => {
                    let removed = match table.find_entry(hash(&key), |v| *v == key) {
                        Ok(entry) => entry.remove() == key,
                        Err(_) => false,
                    };
                    assert_eq!(removed, model.remove(&key));
                }
            }
            assert_eq!(table.len(), model.len());
        }
        for key in 0..5000 {
            assert_eq!(
                table.find(hash(&key), |v| *v == key).is_some(),
                model.contains(&key)
            );
        }
        table.retain(|v| *v % 2 == 0);
        model.retain(|v| *v % 2 == 0);
        assert_eq!(table.len(), model.len());
        assert_eq!(table.iter().count(), model.len());
        table.reserve(20_000, hash);
        for key in 0..5000 {
            assert_eq!(
                table.find(hash(&key), |v| *v == key).is_some(),
                model.contains(&key)
            );
        }
        table.clear();
        assert!(table.is_empty());
        assert!(table.find(hash(&2), |v| *v == 2).is_none());
    }

    #[test]
    fn colliding_hashes_with_tombstones() {
        let hash = |v: &u32| u64::from(*v % 3).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let mut table: HashTable<u32> = HashTable::new();
        let mut model = std::collections::HashSet::new();
        for round in 0..50u32 {
            for key in 0..40u32 {
                let key = key + round;
                if let super::Entry::Vacant(entry) = table.entry(hash(&key), |v| *v == key, hash) {
                    entry.insert(key);
                }
                model.insert(key);
            }
            for key in 0..40u32 {
                if key % 2 == 0 {
                    let key = key + round;
                    if let Ok(entry) = table.find_entry(hash(&key), |v| *v == key) {
                        entry.remove();
                    }
                    model.remove(&key);
                }
            }
            for key in 0..100u32 {
                let present = table.find(hash(&key), |v| *v == key).is_some();
                assert_eq!(present, model.contains(&key), "{round} {key}");
            }
            assert_eq!(table.len(), model.len());
        }
    }

    #[test]
    fn small_tables() {
        let state = RandomState::new();
        let hash = |v: &u32| state.hash_one(*v);
        for seed in 0..2000u32 {
            let mut table: HashTable<u32> = HashTable::with_capacity((seed % 4) as usize);
            let mut model = std::collections::HashSet::new();
            let mut x = seed.wrapping_mul(2_654_435_761) | 1;
            for _ in 0..12 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let key = x % 6;
                if x % 3 == 0 {
                    if let Ok(entry) = table.find_entry(hash(&key), |v| *v == key) {
                        entry.remove();
                    }
                    model.remove(&key);
                } else {
                    if let super::Entry::Vacant(entry) = table.entry(hash(&key), |v| *v == key, hash) {
                        entry.insert(key);
                    }
                    model.insert(key);
                }
                for key in 0..6 {
                    assert_eq!(
                        table.find(hash(&key), |v| *v == key).is_some(),
                        model.contains(&key)
                    );
                }
                assert_eq!(table.len(), model.len());
            }
        }
    }
}
