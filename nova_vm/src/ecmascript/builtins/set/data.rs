// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{OrdinaryObject, Value, rebuild_hash_table, sweep_keys_moved_an_object},
    engine::Bindable,
    heap::{CompactionLists, HeapMarkAndSweep, PrimitiveHeapAccess, WorkQueues},
};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
};
use hashbrown::HashTable;
use nova_vm_derive::SoAble;

#[derive(Debug, Default, SoAble, Bindable)]
pub(crate) struct SetHeapData<'a> {
    pub(crate) set_data: RefCell<HashTable<u32>>,
    pub(crate) values: Vec<Option<Value<'static>>>,
    pub(crate) object_index: Option<OrdinaryObject<'a>>,
    /// Set by a garbage collection that moved an Object value: set_data is
    /// rebuilt (rehash_set_data_if_needed) before it is used again.
    pub(crate) needs_primitive_rehashing: AtomicBool,
    // TODO: When an non-terminal (start or end) iterator exists for the Set,
    // the items in the set cannot be compacted.
    // pub(crate) observed: bool;
}

impl HeapMarkAndSweep for SetHeapData<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            object_index,
            values,
            ..
        } = self;
        object_index.mark_values(queues);
        values.iter().for_each(|value| value.mark_values(queues));
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            object_index,
            values,
            needs_primitive_rehashing,
            ..
        } = self;
        object_index.sweep_values(compactions);
        if sweep_keys_moved_an_object(values, compactions) {
            needs_primitive_rehashing.store(true, Ordering::Relaxed);
        }
    }
}

/// Rebuild the Set's hash table if a garbage collection moved an Object
/// value since it was last used (see rebuild_hash_table).
pub(crate) fn rehash_set_data_if_needed(
    set_data: &RefCell<HashTable<u32>>,
    values: &[Option<Value>],
    needs_rehashing: &AtomicBool,
    arena: &impl PrimitiveHeapAccess,
) {
    if !needs_rehashing.load(Ordering::Relaxed) {
        return;
    }
    rebuild_hash_table(values, &mut set_data.borrow_mut(), arena);
    needs_rehashing.store(false, Ordering::Relaxed);
}
