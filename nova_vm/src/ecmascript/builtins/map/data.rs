// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::hashing::AHasher;
use crate::{
    ecmascript::types::{OrdinaryObject, Value},
    engine::Bindable,
    heap::{CompactionLists, HeapMarkAndSweep, PrimitiveHeapAccess, WorkQueues},
};
use core::{
    cell::RefCell,
    hash::Hasher,
    sync::atomic::{AtomicBool, Ordering},
};
use hashbrown::HashTable;
use soavec_derive::SoAble;

#[derive(Debug, Default, SoAble, Bindable)]
pub(crate) struct MapHeapData<'a> {
    /// Low-level hash table pointing to keys-values indexes.
    pub(super) map_data: RefCell<HashTable<u32>>,
    // TODO: Use a ParallelVec to remove one unnecessary allocation.
    // key_values: ParallelVec<Option<Value>, Option<Value>>
    pub(super) values: Vec<Option<Value<'static>>>,
    pub(super) keys: Vec<Option<Value<'static>>>,
    pub(super) object_index: Option<OrdinaryObject<'a>>,
    /// Set by a garbage collection that moved an Object key: map_data is
    /// rebuilt (rebuild_hash_table) before it is used again.
    needs_primitive_rehashing: AtomicBool,
    // TODO: When an non-terminal (start or end) iterator exists for the Map,
    // the items in the map cannot be compacted.
    // observed: bool;
}
impl MapHeapData<'_> {
    pub(crate) fn with_capacity(new_len: usize) -> Self {
        Self {
            keys: Vec::with_capacity(new_len),
            values: Vec::with_capacity(new_len),
            map_data: RefCell::new(HashTable::with_capacity(new_len)),
            needs_primitive_rehashing: AtomicBool::new(false),
            object_index: None,
        }
    }
}

impl<'map, 'soa> MapHeapDataMut<'map, 'soa> {
    #[inline(always)]
    pub(crate) fn clear(&mut self) {
        // 3. For each Record { [[Key]], [[Value]] } p of M.[[MapData]], do
        // a. Set p.[[Key]] to EMPTY.
        // b. Set p.[[Value]] to EMPTY.
        self.map_data.get_mut().clear();
        self.values.fill(None);
        self.keys.fill(None);
    }

    pub(crate) fn rehash_if_needed_mut(&mut self, arena: &impl PrimitiveHeapAccess) {
        if !*self.needs_primitive_rehashing.get_mut() {
            return;
        }

        rehash_map_data(self.keys, self.map_data.get_mut(), arena);
        self.needs_primitive_rehashing
            .store(false, Ordering::Relaxed);
    }
}

impl<'map, 'soa> MapHeapDataRef<'map, 'soa> {
    /// ### [24.2.1.5 MapDataSize ( setData )](https://tc39.es/ecma262/#sec-setdatasize)
    ///
    /// The abstract operation MapDataSize takes argument setData (a List of either
    /// ECMAScript language values or EMPTY) and returns a non-negative integer.
    #[inline(always)]
    pub(crate) fn size(&self) -> u32 {
        // 1. Let count be 0.
        // 2. For each element e of setData, do
        // a. If e is not EMPTY, set count to count + 1.
        // 3. Return count.
        self.map_data.borrow().len() as u32
    }

    pub(crate) fn entries_len(&self) -> u32 {
        self.values.len() as u32
    }
}

fn rehash_map_data(
    keys: &[Option<Value>],
    map_data: &mut HashTable<u32>,
    arena: &impl PrimitiveHeapAccess,
) {
    rebuild_hash_table(keys, map_data, arena);
}

/// Rebuild a Map's or Set's hash table from its live keys. A garbage
/// collection that moves an Object key changes the key's hash, and the
/// table cannot be fixed during the collection (a primitive key's hash
/// needs the heap), so the collection only marks the table and the next
/// access rebuilds it here.
pub(crate) fn rebuild_hash_table(
    keys: &[Option<Value>],
    table: &mut HashTable<u32>,
    arena: &impl PrimitiveHeapAccess,
) {
    let hasher = |value: Value| {
        let mut hasher = AHasher::default();
        value.unbind().hash(arena, &mut hasher);
        hasher.finish()
    };
    table.clear();
    for (index, key) in keys.iter().enumerate() {
        let Some(key) = key else {
            continue;
        };
        let index = u32::try_from(index).expect("keyed collection indexes are u32");
        table.insert_unique(hasher(*key), index, |index_to_hash| {
            hasher(keys[*index_to_hash as usize].expect("the table holds live keys only"))
        });
    }
}

impl HeapMarkAndSweep for MapHeapDataRef<'_, 'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            keys,
            values,
            object_index,
            ..
        } = self;
        object_index.mark_values(queues);
        keys.iter().for_each(|value| value.mark_values(queues));
        values.iter().for_each(|value| value.mark_values(queues));
    }

    fn sweep_values(&mut self, _compactions: &CompactionLists) {
        unreachable!()
    }
}

impl HeapMarkAndSweep for MapHeapDataMut<'_, 'static> {
    fn mark_values(&self, _queues: &mut WorkQueues) {
        unreachable!()
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            object_index,
            keys,
            values,
            needs_primitive_rehashing,
            map_data: _,
        } = self;
        object_index.sweep_values(compactions);
        values.sweep_values(compactions);
        if sweep_keys_moved_an_object(keys, compactions) {
            needs_primitive_rehashing.store(true, Ordering::Relaxed);
        }
    }
}

/// Sweep the keys of a Map or Set and tell if an Object key moved: its hash
/// changed, so the hash table must be rebuilt before it is used again.
pub(crate) fn sweep_keys_moved_an_object(
    keys: &mut [Option<Value<'static>>],
    compactions: &CompactionLists,
) -> bool {
    let mut moved = false;
    for key in keys.iter_mut().flatten() {
        let old_key = *key;
        key.sweep_values(compactions);
        moved |= old_key != *key && key.is_object();
    }
    moved
}

/// Bytes held by the entries of every Map, for the heap size estimate.
pub(crate) fn map_entries_bytes(maps: &soavec::SoAVec<MapHeapData<'static>>) -> usize {
    (0..maps.len())
        .filter_map(|index| maps.get(index))
        .map(|map| {
            (map.keys.capacity() + map.values.capacity())
                * core::mem::size_of::<Option<Value<'static>>>()
                + map.keys.capacity() * 8
        })
        .sum()
}
