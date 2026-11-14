// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::hashing::AHashMap;

use crate::{
    ecmascript::{OrdinaryObject, Value, execution::WeakKey},
    engine::Bindable,
    heap::{CompactionLists, HeapMarkAndSweep, HeapSweepWeakReference, WorkQueues},
};

#[derive(Debug, Default, Bindable)]
pub(crate) struct WeakMapRecord<'a> {
    pub(crate) weak_map_data: AHashMap<WeakKey<'static>, Value<'static>>,
    pub(super) object_index: Option<OrdinaryObject<'a>>,
}
impl<'a> WeakMapRecord<'a> {
    pub(super) fn delete(&mut self, key: WeakKey<'a>) -> bool {
        // 4. For each Record { [[Key]], [[Value]] } p of M.[[WeakMapData]], do
        // a. If p.[[Key]] is not EMPTY and SameValue(p.[[Key]], key) is true, then
        // i. Set p.[[Key]] to EMPTY.
        // ii. Set p.[[Value]] to EMPTY.
        // iii. Return true.
        // 5. Return false.
        self.weak_map_data.remove(&key.unbind()).is_some()
    }

    pub(super) fn get(&mut self, key: WeakKey<'a>) -> Option<Value<'a>> {
        // 4. For each Record { [[Key]], [[Value]] } p of M.[[WeakMapData]], do
        // a. If p.[[Key]] is not empty and SameValue(p.[[Key]], key) is true, return p.[[Value]].
        // 5. Return undefined.
        self.weak_map_data.get(&key.unbind()).cloned()
    }

    pub(super) fn has(&mut self, key: WeakKey<'a>) -> bool {
        // 4. For each Record { [[Key]], [[Value]] } p of M.[[WeakMapData]], do
        // a. If p.[[Key]] is not empty and SameValue(p.[[Key]], key) is true, return true.
        // 5. Return false.
        self.weak_map_data.contains_key(&key.unbind())
    }

    pub(super) fn set(&mut self, key: WeakKey<'a>, value: Value<'a>) {
        // 4. For each Record { [[Key]], [[Value]] } p of M.[[WeakMapData]], do
        // a. If p.[[Key]] is not empty and SameValue(p.[[Key]], key) is true, then
        // i. Set p.[[Value]] to value.
        // ii. Return M.
        // 5. Let p be the Record { [[Key]]: key, [[Value]]: value }.
        // 6. Append p to M.[[WeakMapData]].
        self.weak_map_data.insert(key.unbind(), value.unbind());
    }
}

impl HeapMarkAndSweep for WeakMapRecord<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            weak_map_data: map,
            object_index,
        } = self;
        for (key, value) in map.iter() {
            if queues.bits.is_marked(key) {
                value.mark_values(queues);
            } else {
                queues.pending_ephemerons.push((*key, *value));
            }
        }
        object_index.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            weak_map_data: map,
            object_index,
        } = self;
        let old_map = core::mem::replace(map, AHashMap::with_capacity(map.len()));
        for (key, mut value) in old_map {
            if let Some(key) = key.sweep_weak_reference(compactions) {
                value.sweep_values(compactions);
                map.insert(key, value);
            }
        }
        object_index.sweep_values(compactions);
    }
}
