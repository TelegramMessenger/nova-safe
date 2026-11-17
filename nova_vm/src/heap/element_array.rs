// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::hashing::{AHashMap, HandleState};
use core::cell::RefCell;
use crate::table::HashTable;

use super::{
    AtomicBits, BitRange, CompactionLists, ElementIndex, HeapMarkAndSweep, PropertyKeyIndex,
    WorkQueues,
    heap_bits::release_spare_capacity,
    object_entry::{ObjectEntry, ObjectEntryPropertyDescriptor},
};
use crate::{
    ecmascript::{Agent, Function, PrivateName, PropertyDescriptor, PropertyKey, Value},
    engine::{Bindable, NoGcScope},
    heap::HeapIndexHandle,
};
use core::ops::{Index, IndexMut};
use std::collections::{TryReserveError, hash_map::Entry};

/// Shared access to an element storage.
pub(crate) struct ElementStorageRef<'a, 'gc> {
    pub values: &'a [Option<Value<'gc>>],
    pub descriptors: Option<&'a ElementDescriptorMap<'gc>>,
}

impl ElementStorageRef<'_, '_> {
    const EMPTY: Self = Self {
        values: &[],
        descriptors: None,
    };
}

/// Exclusive access to an element storage.
pub(crate) struct ElementStorageMut<'a> {
    pub values: &'a mut [Option<Value<'static>>],
    pub descriptors: Entry<'a, ElementIndex<'static>, ElementDescriptorMap<'static>>,
}
/// Exclusive access to a full element storage, including uninitialised
/// elements.
pub(crate) struct ElementStorageUninit<'a> {
    pub values: &'a mut [Option<Value<'static>>],
    pub descriptors: Entry<'a, ElementIndex<'static>, ElementDescriptorMap<'static>>,
}

/// Shared access to an object's property storage.
pub(crate) struct PropertyStorageRef<'a, 'gc> {
    pub keys: &'a [PropertyKey<'gc>],
    pub values: &'a [Option<Value<'gc>>],
    pub descriptors: Option<&'a ElementDescriptorMap<'gc>>,
}

impl<'a, 'gc> PropertyStorageRef<'a, 'gc> {
    pub(crate) const fn from_keys_and_elements(
        keys: &'a [PropertyKey<'gc>],
        element_storage_ref: ElementStorageRef<'a, 'gc>,
    ) -> Self {
        Self {
            keys,
            values: element_storage_ref.values,
            descriptors: element_storage_ref.descriptors,
        }
    }

    pub(crate) const fn len(&self) -> usize {
        if self.keys.len() != self.values.len() {
            panic!("Mismatched PropertyStorageRef keys and values lengths");
        }
        self.keys.len()
    }
}

/// Exclusive access to an object's property storage.
pub(crate) struct PropertyStorageMut<'a, 'gc> {
    pub keys: &'a [PropertyKey<'gc>],
    pub values: &'a mut [Option<Value<'static>>],
    pub descriptors: Entry<'a, ElementIndex<'static>, ElementDescriptorMap<'static>>,
}

impl<'a, 'gc> PropertyStorageMut<'a, 'gc> {
    const fn from_keys_and_elements(
        keys: &'a [PropertyKey<'gc>],
        element_storage_mut: ElementStorageMut<'a>,
    ) -> Self {
        Self {
            keys,
            values: element_storage_mut.values,
            descriptors: element_storage_mut.descriptors,
        }
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ElementArrayKey {
    #[default]
    Empty,
    /// Uniquely owned zero-sized elements array
    EmptyIntrinsic,
    /// up to 2 elements
    E1,
    /// up to 4 elements
    E2,
    /// up to 8 elements
    E3,
    /// up to 16 elements
    E4,
    /// up to 64 elements
    E6,
    /// up to 256 elements
    E8,
    /// up to 1024 elements
    E10,
    /// up to 4096 elements
    E12,
    /// up to 65536 elements
    E16,
    /// up to 16777216 elements
    E24,
    /// up to 4294967296 elements
    E32,
}

impl ElementArrayKey {
    /// Get the capacity of this ElementArrayKey.
    pub(crate) fn capacity(self) -> u32 {
        match self {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => 0,
            ElementArrayKey::E1 => 2u32.pow(1),
            ElementArrayKey::E2 => 2u32.pow(2),
            ElementArrayKey::E3 => 2u32.pow(3),
            ElementArrayKey::E4 => 2u32.pow(4),
            ElementArrayKey::E6 => 2u32.pow(6),
            ElementArrayKey::E8 => 2u32.pow(8),
            ElementArrayKey::E10 => 2u32.pow(10),
            ElementArrayKey::E12 => 2u32.pow(12),
            ElementArrayKey::E16 => 2u32.pow(16),
            ElementArrayKey::E24 => 2u32.pow(24),
            ElementArrayKey::E32 => u32::MAX,
        }
    }

    /// Turn the ElementArrayKey into an intrinsic one.
    ///
    /// This is needed for intrinsic object shapes.
    pub(crate) fn make_intrinsic(self) -> Self {
        if self == ElementArrayKey::Empty {
            ElementArrayKey::EmptyIntrinsic
        } else {
            self
        }
    }
}

impl From<u32> for ElementArrayKey {
    fn from(value: u32) -> Self {
        if value == 0 {
            ElementArrayKey::Empty
        } else if value <= u32::pow(2, 1) {
            ElementArrayKey::E1
        } else if value <= u32::pow(2, 2) {
            ElementArrayKey::E2
        } else if value <= u32::pow(2, 3) {
            ElementArrayKey::E3
        } else if value <= u32::pow(2, 4) {
            ElementArrayKey::E4
        } else if value <= u32::pow(2, 6) {
            ElementArrayKey::E6
        } else if value <= u32::pow(2, 8) {
            ElementArrayKey::E8
        } else if value <= u32::pow(2, 10) {
            ElementArrayKey::E10
        } else if value <= u32::pow(2, 12) {
            ElementArrayKey::E12
        } else if value <= u32::pow(2, 16) {
            ElementArrayKey::E16
        } else if value <= u32::pow(2, 24) {
            ElementArrayKey::E24
        } else {
            ElementArrayKey::E32
        }
    }
}

impl From<usize> for ElementArrayKey {
    fn from(value: usize) -> Self {
        u32::try_from(value).map_or(Self::E32, Self::from)
    }
}

#[derive(Debug, Clone, Copy, Bindable)]
pub(crate) struct ElementsVector<'a> {
    pub(crate) elements_index: ElementIndex<'a>,
    pub(crate) cap: ElementArrayKey,
    pub(crate) len: u32,
    /// Array length property can be set to unwritable
    pub(crate) len_writable: bool,
}

impl Default for ElementsVector<'static> {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl<'gc> ElementsVector<'gc> {
    #[inline]
    pub(crate) fn get<'a>(&self, agent: &'a Agent) -> &'a [Option<Value<'gc>>] {
        agent.heap.elements.get_values(self)
    }

    /// Get the values as mutable, as stored with their `'static` GC lifetime:
    /// values written into the slice must be unbound first.
    #[inline]
    pub(crate) fn get_mut<'a>(&self, agent: &'a mut Agent) -> &'a mut [Option<Value<'static>>] {
        agent.heap.elements.get_values_mut(self)
    }

    pub(crate) const EMPTY: Self = Self {
        elements_index: ElementIndex::ZERO,
        cap: ElementArrayKey::Empty,
        len: 0,
        len_writable: true,
    };

    pub(crate) fn cap(&self) -> u32 {
        self.cap.capacity()
    }

    pub(crate) fn len(&self) -> u32 {
        self.len
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn is_full(&self) -> bool {
        self.len == self.cap()
    }

    pub(crate) fn writable(&self) -> bool {
        self.len_writable
    }

    /// Get shared access to the elements storage of an array.
    ///
    /// Value slots with None are either array holes, or accessor properties.
    /// For accessor properties a descriptor for the same index exists.
    pub(crate) fn get_storage<'a>(
        &self,
        arena: &'a impl AsRef<ElementArrays>,
    ) -> ElementStorageRef<'a, 'gc> {
        arena.as_ref().get_element_storage(self)
    }

    /// Get exclusive access to the elements storage of an array.
    ///
    /// Value slots with None are either array holes, or accessor properties.
    /// For accessor properties a descriptor for the same index exists.
    pub(crate) fn get_storage_mut<'a>(
        &self,
        arena: &'a mut impl AsMut<ElementArrays>,
    ) -> ElementStorageMut<'a> {
        arena.as_mut().get_element_storage_mut(self)
    }

    /// An elements vector is simple if it contains no accessor descriptors.
    pub(crate) fn is_simple(&self, arena: &impl AsRef<ElementArrays>) -> bool {
        let storage = arena.as_ref().get_element_storage(self);
        storage.descriptors.is_none_or(|hashmap| {
            !hashmap
                .iter()
                .any(|desc| desc.1.has_getter() || desc.1.has_setter())
        })
    }

    /// An elements vector is trivial if it contains no descriptors.
    pub(crate) fn is_trivial(&self, arena: &impl AsRef<ElementArrays>) -> bool {
        let ElementStorageRef { descriptors, .. } = arena.as_ref().get_element_storage(self);
        descriptors.is_none()
    }

    /// An elements vector is dense if it contains no holes or getters.
    pub(crate) fn is_dense(&self, arena: &impl AsRef<ElementArrays>) -> bool {
        let ElementStorageRef {
            descriptors,
            values,
        } = arena.as_ref().get_element_storage(self);
        if let Some(descriptors) = descriptors {
            for (index, ele) in values.iter().enumerate() {
                let index = index as u32;
                if ele.is_none() {
                    let ele_descriptor = descriptors.get(&index);
                    let Some(ele_descriptor) = ele_descriptor else {
                        // No value, no descriptor: That's a hole.
                        return false;
                    };
                    if !ele_descriptor.has_getter() {
                        // No value, no getter: That's effectively a hole.
                        return false;
                    }
                }
            }
            true
        } else {
            // No descriptors, no value: That's a hole.
            values.iter().all(|e| e.is_some())
        }
    }

    pub(crate) fn reserve(
        &mut self,
        elements: &mut ElementArrays,
        new_len: u32,
    ) -> Result<(), TryReserveError> {
        if new_len <= self.cap() {
            // Enough capacity present already
            return elements.ensure_values_len(self.cap, self.elements_index, new_len);
        }

        elements.reserve_elements(self, new_len)
    }

    pub(crate) fn push(
        &mut self,
        elements: &mut ElementArrays,
        value: Option<Value>,
        descriptor: Option<ElementDescriptor>,
    ) -> Result<(), TryReserveError> {
        if self.is_full() {
            self.reserve(elements, self.len() + 1)?;
        } else {
            elements.ensure_values_len(self.cap, self.elements_index, self.len + 1)?;
        }
        let next_over_end = match self.cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
            ElementArrayKey::E1 => {
                &mut elements.e2pow1.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E2 => {
                &mut elements.e2pow2.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E3 => {
                &mut elements.e2pow3.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E4 => {
                &mut elements.e2pow4.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E6 => {
                &mut elements.e2pow6.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E8 => {
                &mut elements.e2pow8.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E10 => {
                &mut elements.e2pow10.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E12 => {
                &mut elements.e2pow12.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E16 => {
                &mut elements.e2pow16.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E24 => {
                &mut elements.e2pow24.values[self.elements_index][self.len as usize]
            }
            ElementArrayKey::E32 => {
                &mut elements.e2pow32.values[self.elements_index][self.len as usize]
            }
        };
        *next_over_end = value.map(Value::unbind);
        if let Some(descriptor) = descriptor {
            let descriptors_map = match self.cap {
                ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
                ElementArrayKey::E1 => &mut elements.e2pow1.descriptors,
                ElementArrayKey::E2 => &mut elements.e2pow2.descriptors,
                ElementArrayKey::E3 => &mut elements.e2pow3.descriptors,
                ElementArrayKey::E4 => &mut elements.e2pow4.descriptors,
                ElementArrayKey::E6 => &mut elements.e2pow6.descriptors,
                ElementArrayKey::E8 => &mut elements.e2pow8.descriptors,
                ElementArrayKey::E10 => &mut elements.e2pow10.descriptors,
                ElementArrayKey::E12 => &mut elements.e2pow12.descriptors,
                ElementArrayKey::E16 => &mut elements.e2pow16.descriptors,
                ElementArrayKey::E24 => &mut elements.e2pow24.descriptors,
                ElementArrayKey::E32 => &mut elements.e2pow32.descriptors,
            };
            descriptors_map
                .entry(self.elements_index.unbind())
                .or_default()
                .insert(self.len, descriptor.unbind());
        }
        self.len += 1;
        Ok(())
    }
}

impl HeapMarkAndSweep for ElementsVector<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            elements_index,
            cap,
            len: _,
            len_writable: _,
        } = self;
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {}
            ElementArrayKey::E1 => queues.e_2_1.push(*elements_index),
            ElementArrayKey::E2 => queues.e_2_2.push(*elements_index),
            ElementArrayKey::E3 => queues.e_2_3.push(*elements_index),
            ElementArrayKey::E4 => queues.e_2_4.push(*elements_index),
            ElementArrayKey::E6 => queues.e_2_6.push(*elements_index),
            ElementArrayKey::E8 => queues.e_2_8.push(*elements_index),
            ElementArrayKey::E10 => queues.e_2_10.push(*elements_index),
            ElementArrayKey::E12 => queues.e_2_12.push(*elements_index),
            ElementArrayKey::E16 => queues.e_2_16.push(*elements_index),
            ElementArrayKey::E24 => queues.e_2_24.push(*elements_index),
            ElementArrayKey::E32 => queues.e_2_32.push(*elements_index),
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            elements_index,
            cap,
            len: _,
            len_writable: _,
        } = self;
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {}
            ElementArrayKey::E1 => compactions.e_2_1.shift_index(elements_index),
            ElementArrayKey::E2 => compactions.e_2_2.shift_index(elements_index),
            ElementArrayKey::E3 => compactions.e_2_3.shift_index(elements_index),
            ElementArrayKey::E4 => compactions.e_2_4.shift_index(elements_index),
            ElementArrayKey::E6 => compactions.e_2_6.shift_index(elements_index),
            ElementArrayKey::E8 => compactions.e_2_8.shift_index(elements_index),
            ElementArrayKey::E10 => compactions.e_2_10.shift_index(elements_index),
            ElementArrayKey::E12 => compactions.e_2_12.shift_index(elements_index),
            ElementArrayKey::E16 => compactions.e_2_16.shift_index(elements_index),
            ElementArrayKey::E24 => compactions.e_2_24.shift_index(elements_index),
            ElementArrayKey::E32 => compactions.e_2_32.shift_index(elements_index),
        };
    }
}

/// The descriptors of one element storage's slots that are not plain
/// writable, enumerable and configurable data properties, keyed by slot.
/// Up to [`SMALL_DESCRIPTOR_MAP`] of them are kept in a vector searched
/// linearly; past that, in a hash map, so that a script defining many slots
/// does not make every definition linear.
#[derive(Debug, Clone)]
pub enum ElementDescriptorMap<'a> {
    /// At most [`SMALL_DESCRIPTOR_MAP`] descriptors in insertion order.
    Small(Vec<(u32, ElementDescriptor<'a>)>),
    /// More descriptors, hashed by slot.
    Large(AHashMap<u32, ElementDescriptor<'a>>),
}

const SMALL_DESCRIPTOR_MAP: usize = 8;

impl Default for ElementDescriptorMap<'_> {
    fn default() -> Self {
        Self::Small(Vec::new())
    }
}

impl<'a> FromIterator<(u32, ElementDescriptor<'a>)> for ElementDescriptorMap<'a> {
    fn from_iter<I: IntoIterator<Item = (u32, ElementDescriptor<'a>)>>(iter: I) -> Self {
        let mut map = Self::default();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl<'a> ElementDescriptorMap<'a> {
    /// An empty map with room for `capacity` descriptors.
    pub fn with_capacity(capacity: usize) -> Self {
        if capacity > SMALL_DESCRIPTOR_MAP {
            Self::Large(crate::hashing::map_with_capacity(capacity))
        } else {
            Self::Small(Vec::with_capacity(capacity))
        }
    }

    /// The number of descriptors.
    pub fn len(&self) -> usize {
        match self {
            Self::Small(entries) => entries.len(),
            Self::Large(map) => map.len(),
        }
    }

    /// True if there are no descriptors.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The descriptor of slot `key`.
    pub fn get(&self, key: &u32) -> Option<&ElementDescriptor<'a>> {
        match self {
            Self::Small(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            Self::Large(map) => map.get(key),
        }
    }

    /// The descriptor of slot `key`, mutably.
    pub fn get_mut(&mut self, key: &u32) -> Option<&mut ElementDescriptor<'a>> {
        match self {
            Self::Small(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            Self::Large(map) => map.get_mut(key),
        }
    }

    /// True if slot `key` has a descriptor.
    pub fn contains_key(&self, key: &u32) -> bool {
        self.get(key).is_some()
    }

    /// Sets the descriptor of slot `key`, returning the one it replaces.
    pub fn insert(
        &mut self,
        key: u32,
        value: ElementDescriptor<'a>,
    ) -> Option<ElementDescriptor<'a>> {
        match self {
            Self::Small(entries) => {
                if let Some((_, slot)) = entries.iter_mut().find(|(k, _)| *k == key) {
                    return Some(core::mem::replace(slot, value));
                }
                if entries.len() < SMALL_DESCRIPTOR_MAP {
                    entries.push((key, value));
                    return None;
                }
                let mut map: AHashMap<u32, ElementDescriptor<'a>> =
                    crate::hashing::map_with_capacity(entries.len() + 1);
                map.extend(entries.drain(..));
                map.insert(key, value);
                *self = Self::Large(map);
                None
            }
            Self::Large(map) => map.insert(key, value),
        }
    }

    /// Removes and returns the descriptor of slot `key`.
    pub fn remove(&mut self, key: &u32) -> Option<ElementDescriptor<'a>> {
        match self {
            Self::Small(entries) => {
                let position = entries.iter().position(|(k, _)| k == key)?;
                Some(entries.remove(position).1)
            }
            Self::Large(map) => map.remove(key),
        }
    }

    /// Makes room for `additional` more descriptors.
    pub fn reserve(&mut self, additional: usize) {
        match self {
            Self::Small(entries) => {
                if entries.len() + additional <= SMALL_DESCRIPTOR_MAP {
                    entries.reserve(additional);
                }
            }
            Self::Large(map) => map.reserve(additional),
        }
    }

    /// Removes the entries whose key and descriptor `extract` accepts and
    /// returns them.
    pub fn extract_if(
        &mut self,
        mut extract: impl FnMut(&u32, &mut ElementDescriptor<'a>) -> bool,
    ) -> std::vec::IntoIter<(u32, ElementDescriptor<'a>)> {
        match self {
            Self::Small(entries) => entries
                .extract_if(.., |(k, v)| extract(k, v))
                .collect::<Vec<_>>()
                .into_iter(),
            Self::Large(map) => map
                .extract_if(|k, v| extract(k, v))
                .collect::<Vec<_>>()
                .into_iter(),
        }
    }

    /// The descriptors, in no particular order.
    pub fn values(&self) -> impl Iterator<Item = &ElementDescriptor<'a>> {
        let (small, large) = match self {
            Self::Small(entries) => (Some(entries.iter().map(|(_, v)| v)), None),
            Self::Large(map) => (None, Some(map.values())),
        };
        small
            .into_iter()
            .flatten()
            .chain(large.into_iter().flatten())
    }

    /// The descriptors, mutably, in no particular order.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut ElementDescriptor<'a>> {
        let (small, large) = match self {
            Self::Small(entries) => (Some(entries.iter_mut().map(|(_, v)| v)), None),
            Self::Large(map) => (None, Some(map.values_mut())),
        };
        small
            .into_iter()
            .flatten()
            .chain(large.into_iter().flatten())
    }

    /// The slots and their descriptors, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &ElementDescriptor<'a>)> {
        let (small, large) = match self {
            Self::Small(entries) => (Some(entries.iter().map(|(k, v)| (k, v))), None),
            Self::Large(map) => (None, Some(map.iter())),
        };
        small
            .into_iter()
            .flatten()
            .chain(large.into_iter().flatten())
    }
}

/// Property descriptor enumeration used for creating objects.
#[derive(Debug, Clone, Copy, PartialEq, Bindable)]
#[repr(u8)]
pub enum ElementDescriptor<'a> {
    /// ```js
    /// { value, writable: true, enumerable: true, configurable: true }
    /// ```
    WritableEnumerableConfigurableData,
    /// ```js
    /// { value, writable: true, enumerable: true, configurable: false }
    /// ```
    WritableEnumerableUnconfigurableData,
    /// ```js
    /// { value, writable: true, enumerable: false, configurable: true }
    /// ```
    WritableUnenumerableConfigurableData,
    /// ```js
    /// { value, writable: true, enumerable: false, configurable: false }
    /// ```
    WritableUnenumerableUnconfigurableData,
    /// ```js
    /// { value, writable: false, enumerable: true, configurable: true }
    /// ```
    ReadOnlyEnumerableConfigurableData,
    /// ```js
    /// { value, writable: false, enumerable: true, configurable: false }
    /// ```
    ReadOnlyEnumerableUnconfigurableData,
    /// ```js
    /// { value, writable: false, enumerable: false, configurable: true }
    /// ```
    ReadOnlyUnenumerableConfigurableData,
    /// ```js
    /// { value, writable: false, enumerable: false, configurable: false }
    /// ```
    ReadOnlyUnenumerableUnconfigurableData,
    /// ```js
    /// { get: undefined, set: undefined, enumerable: true, configurable: true }
    /// ```
    UndefinedEnumerableConfigurableAccessor,
    /// ```js
    /// { get: undefined, set: undefined, enumerable: true, configurable: true }
    /// ```
    UndefinedEnumerableUnconfigurableAccessor,
    /// ```js
    /// { get: undefined, set: undefined, enumerable: true, configurable: true }
    /// ```
    UndefinedUnenumerableConfigurableAccessor,
    /// ```js
    /// { get: undefined, set: undefined, enumerable: true, configurable: true }
    /// ```
    UndefinedUnenumerableUnconfigurableAccessor,
    /// ```js
    /// { get, set: undefined, enumerable: true, configurable: true }
    /// ```
    ReadOnlyEnumerableConfigurableAccessor {
        /// Getter function of a read-only accessor property.
        get: Function<'a>,
    },
    /// ```js
    /// { get, set: undefined, enumerable: true, configurable: false }
    /// ```
    ReadOnlyEnumerableUnconfigurableAccessor {
        /// Getter function of a read-only accessor property.
        get: Function<'a>,
    },
    /// ```js
    /// { get, set: undefined, enumerable: false, configurable: true }
    /// ```
    ReadOnlyUnenumerableConfigurableAccessor {
        /// Getter function of a read-only accessor property.
        get: Function<'a>,
    },
    /// ```js
    /// { get, set: undefined, enumerable: false, configurable: false }
    /// ```
    ReadOnlyUnenumerableUnconfigurableAccessor {
        /// Getter function of a read-only accessor property.
        get: Function<'a>,
    },
    /// ```js
    /// { get: undefined, set, enumerable: true, configurable: true }
    /// ```
    WriteOnlyEnumerableConfigurableAccessor {
        /// Setter function of a write-only accessor property.
        set: Function<'a>,
    },
    /// ```js
    /// { get: undefined, set, enumerable: true, configurable: false }
    /// ```
    WriteOnlyEnumerableUnconfigurableAccessor {
        /// Setter function of a write-only accessor property.
        set: Function<'a>,
    },
    /// ```js
    /// { get: undefined, set, enumerable: false, configurable: true }
    /// ```
    WriteOnlyUnenumerableConfigurableAccessor {
        /// Setter function of a write-only accessor property.
        set: Function<'a>,
    },
    /// ```js
    /// { get: undefined, set, enumerable: false, configurable: false }
    /// ```
    WriteOnlyUnenumerableUnconfigurableAccessor {
        /// Setter function of a write-only accessor property.
        set: Function<'a>,
    },
    /// ```js
    /// { get, set, enumerable: true, configurable: true }
    /// ```
    ReadWriteEnumerableConfigurableAccessor {
        /// Getter function of a read-write accessor property.
        get: Function<'a>,
        /// Setter function of a read-write accessor property.
        set: Function<'a>,
    },
    /// ```js
    /// { get, set, enumerable: true, configurable: false }
    /// ```
    ReadWriteEnumerableUnconfigurableAccessor {
        /// Getter function of a read-write accessor property.
        get: Function<'a>,
        /// Setter function of a read-write accessor property.
        set: Function<'a>,
    },
    /// ```js
    /// { get, set, enumerable: false, configurable: true }
    /// ```
    ReadWriteUnenumerableConfigurableAccessor {
        /// Getter function of a read-write accessor property.
        get: Function<'a>,
        /// Setter function of a read-write accessor property.
        set: Function<'a>,
    },
    /// ```js
    /// { get, set, enumerable: false, configurable: false }
    /// ```
    ReadWriteUnenumerableUnconfigurableAccessor {
        /// Getter function of a read-write accessor property.
        get: Function<'a>,
        /// Setter function of a read-write accessor property.
        set: Function<'a>,
    },
}

impl<'a> ElementDescriptor<'a> {
    pub(crate) fn has_getter(&self) -> bool {
        matches!(
            self,
            ElementDescriptor::ReadOnlyEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadOnlyEnumerableUnconfigurableAccessor { .. }
                | ElementDescriptor::ReadOnlyUnenumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadOnlyUnenumerableUnconfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteEnumerableUnconfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteUnenumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteUnenumerableUnconfigurableAccessor { .. }
        )
    }

    pub(crate) fn has_setter(&self) -> bool {
        matches!(
            self,
            ElementDescriptor::WriteOnlyEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::WriteOnlyEnumerableUnconfigurableAccessor { .. }
                | ElementDescriptor::WriteOnlyUnenumerableConfigurableAccessor { .. }
                | ElementDescriptor::WriteOnlyUnenumerableUnconfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteEnumerableUnconfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteUnenumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteUnenumerableUnconfigurableAccessor { .. }
        )
    }

    pub(crate) const fn new_with_wec(w: bool, e: bool, c: bool) -> Option<Self> {
        match (w, e, c) {
            (true, true, true) => None,
            (true, true, false) => Some(Self::WritableEnumerableUnconfigurableData),
            (true, false, true) => Some(Self::WritableUnenumerableConfigurableData),
            (true, false, false) => Some(Self::WritableUnenumerableUnconfigurableData),
            (false, true, true) => Some(Self::ReadOnlyEnumerableConfigurableData),
            (false, true, false) => Some(Self::ReadOnlyEnumerableUnconfigurableData),
            (false, false, true) => Some(Self::ReadOnlyUnenumerableConfigurableData),
            (false, false, false) => Some(Self::ReadOnlyUnenumerableUnconfigurableData),
        }
    }

    pub(crate) const fn new_with_get_ec(get: Function<'a>, e: bool, c: bool) -> Self {
        match (e, c) {
            (true, true) => Self::ReadOnlyEnumerableConfigurableAccessor { get },
            (true, false) => Self::ReadOnlyEnumerableUnconfigurableAccessor { get },
            (false, true) => Self::ReadOnlyUnenumerableConfigurableAccessor { get },
            (false, false) => Self::ReadOnlyUnenumerableUnconfigurableAccessor { get },
        }
    }

    pub(crate) const fn new_with_set_ec(set: Function<'a>, e: bool, c: bool) -> Self {
        match (e, c) {
            (true, true) => Self::WriteOnlyEnumerableConfigurableAccessor { set },
            (true, false) => Self::WriteOnlyEnumerableUnconfigurableAccessor { set },
            (false, true) => Self::WriteOnlyUnenumerableConfigurableAccessor { set },
            (false, false) => Self::WriteOnlyUnenumerableUnconfigurableAccessor { set },
        }
    }

    pub(crate) const fn new_with_get_set_ec(
        get: Function<'a>,
        set: Function<'a>,
        e: bool,
        c: bool,
    ) -> Self {
        match (e, c) {
            (true, true) => Self::ReadWriteEnumerableConfigurableAccessor { get, set },
            (true, false) => Self::ReadWriteEnumerableUnconfigurableAccessor { get, set },
            (false, true) => Self::ReadWriteUnenumerableConfigurableAccessor { get, set },
            (false, false) => Self::ReadWriteUnenumerableUnconfigurableAccessor { get, set },
        }
    }

    pub(crate) fn from_object_entry_property_descriptor(
        desc: &ObjectEntryPropertyDescriptor<'a>,
    ) -> (Option<Self>, Option<Value<'a>>) {
        match desc {
            ObjectEntryPropertyDescriptor::Data {
                value,
                writable,
                enumerable,
                configurable,
            } => match (writable, enumerable, configurable) {
                (true, true, true) => (None, Some(*value)),
                (true, true, false) => (
                    Some(Self::WritableEnumerableUnconfigurableData),
                    Some(*value),
                ),
                (true, false, true) => (
                    Some(Self::WritableUnenumerableConfigurableData),
                    Some(*value),
                ),
                (true, false, false) => (
                    Some(Self::WritableUnenumerableUnconfigurableData),
                    Some(*value),
                ),
                (false, true, true) => {
                    (Some(Self::ReadOnlyEnumerableConfigurableData), Some(*value))
                }
                (false, true, false) => (
                    Some(Self::ReadOnlyEnumerableUnconfigurableData),
                    Some(*value),
                ),
                (false, false, true) => (
                    Some(Self::ReadOnlyUnenumerableConfigurableData),
                    Some(*value),
                ),
                (false, false, false) => (
                    Some(Self::ReadOnlyUnenumerableUnconfigurableData),
                    Some(*value),
                ),
            },
            ObjectEntryPropertyDescriptor::Blocked { .. } => unreachable!(),
            ObjectEntryPropertyDescriptor::ReadOnly {
                get,
                enumerable,
                configurable,
            } => match (enumerable, configurable) {
                (true, true) => (
                    Some(Self::ReadOnlyEnumerableConfigurableAccessor { get: *get }),
                    None,
                ),
                (true, false) => (
                    Some(Self::ReadOnlyEnumerableUnconfigurableAccessor { get: *get }),
                    None,
                ),
                (false, true) => (
                    Some(Self::ReadOnlyUnenumerableConfigurableAccessor { get: *get }),
                    None,
                ),
                (false, false) => (
                    Some(Self::ReadOnlyUnenumerableUnconfigurableAccessor { get: *get }),
                    None,
                ),
            },
            ObjectEntryPropertyDescriptor::WriteOnly {
                set,
                enumerable,
                configurable,
            } => match (enumerable, configurable) {
                (true, true) => (
                    Some(Self::WriteOnlyEnumerableConfigurableAccessor { set: *set }),
                    None,
                ),
                (true, false) => (
                    Some(Self::WriteOnlyEnumerableUnconfigurableAccessor { set: *set }),
                    None,
                ),
                (false, true) => (
                    Some(Self::WriteOnlyUnenumerableConfigurableAccessor { set: *set }),
                    None,
                ),
                (false, false) => (
                    Some(Self::WriteOnlyUnenumerableUnconfigurableAccessor { set: *set }),
                    None,
                ),
            },
            ObjectEntryPropertyDescriptor::ReadWrite {
                get,
                set,
                enumerable,
                configurable,
            } => match (enumerable, configurable) {
                (true, true) => (
                    Some(Self::ReadWriteEnumerableConfigurableAccessor {
                        get: *get,
                        set: *set,
                    }),
                    None,
                ),
                (true, false) => (
                    Some(Self::ReadWriteEnumerableUnconfigurableAccessor {
                        get: *get,
                        set: *set,
                    }),
                    None,
                ),
                (false, true) => (
                    Some(Self::ReadWriteUnenumerableConfigurableAccessor {
                        get: *get,
                        set: *set,
                    }),
                    None,
                ),
                (false, false) => (
                    Some(Self::ReadWriteUnenumerableUnconfigurableAccessor {
                        get: *get,
                        set: *set,
                    }),
                    None,
                ),
            },
        }
    }

    pub(crate) fn from_property_descriptor(descriptor: PropertyDescriptor<'a>) -> Option<Self> {
        let configurable = descriptor.configurable.unwrap_or(false);
        let enumerable = descriptor.enumerable.unwrap_or(false);
        let writable = descriptor.writable.unwrap_or(false);
        Some(match (descriptor.get, descriptor.set) {
            (None, None) => match (writable, enumerable, configurable) {
                // Default data descriptor, return None.
                (true, true, true) => return None,
                (true, true, false) => Self::WritableEnumerableUnconfigurableData,
                (true, false, true) => Self::WritableUnenumerableConfigurableData,
                (true, false, false) => Self::WritableUnenumerableUnconfigurableData,
                (false, true, true) => Self::ReadOnlyEnumerableConfigurableData,
                (false, true, false) => Self::ReadOnlyEnumerableUnconfigurableData,
                (false, false, true) => Self::ReadOnlyUnenumerableConfigurableData,
                (false, false, false) => Self::ReadOnlyUnenumerableUnconfigurableData,
            },
            (None | Some(None), None | Some(None)) => match (enumerable, configurable) {
                (true, true) => Self::UndefinedEnumerableConfigurableAccessor,
                (true, false) => Self::UndefinedEnumerableUnconfigurableAccessor,
                (false, true) => Self::UndefinedUnenumerableConfigurableAccessor,
                (false, false) => Self::UndefinedUnenumerableUnconfigurableAccessor,
            },
            (None | Some(None), Some(Some(set))) => match (enumerable, configurable) {
                (true, true) => Self::WriteOnlyEnumerableConfigurableAccessor { set },
                (true, false) => Self::WriteOnlyEnumerableUnconfigurableAccessor { set },
                (false, true) => Self::WriteOnlyUnenumerableConfigurableAccessor { set },
                (false, false) => Self::WriteOnlyUnenumerableUnconfigurableAccessor { set },
            },
            (Some(Some(get)), None | Some(None)) => match (enumerable, configurable) {
                (true, true) => Self::ReadOnlyEnumerableConfigurableAccessor { get },
                (true, false) => Self::ReadOnlyEnumerableUnconfigurableAccessor { get },
                (false, true) => Self::ReadOnlyUnenumerableConfigurableAccessor { get },
                (false, false) => Self::ReadOnlyUnenumerableUnconfigurableAccessor { get },
            },
            (Some(Some(get)), Some(Some(set))) => match (enumerable, configurable) {
                (true, true) => Self::ReadWriteEnumerableConfigurableAccessor { get, set },
                (true, false) => Self::ReadWriteEnumerableUnconfigurableAccessor { get, set },
                (false, true) => Self::ReadWriteUnenumerableConfigurableAccessor { get, set },
                (false, false) => Self::ReadWriteUnenumerableUnconfigurableAccessor { get, set },
            },
        })
    }

    pub(crate) fn from_data_descriptor(descriptor: PropertyDescriptor<'a>) -> Option<Self> {
        debug_assert!(descriptor.is_data_descriptor());
        let configurable = descriptor.configurable.unwrap_or(false);
        let enumerable = descriptor.enumerable.unwrap_or(false);
        let writable = descriptor.writable.unwrap_or(false);
        if configurable && enumerable && writable {
            // Default data descriptor, return None.
            return None;
        }
        Some(match (writable, enumerable, configurable) {
            (true, true, true) => unreachable!(),
            (true, true, false) => Self::WritableEnumerableUnconfigurableData,
            (true, false, true) => Self::WritableUnenumerableConfigurableData,
            (true, false, false) => Self::WritableUnenumerableUnconfigurableData,
            (false, true, true) => Self::ReadOnlyEnumerableConfigurableData,
            (false, true, false) => Self::ReadOnlyEnumerableUnconfigurableData,
            (false, false, true) => Self::ReadOnlyUnenumerableConfigurableData,
            (false, false, false) => Self::ReadOnlyUnenumerableUnconfigurableData,
        })
    }

    pub(crate) fn from_accessor_descriptor(descriptor: PropertyDescriptor<'a>) -> Self {
        let enumerable = descriptor.enumerable.unwrap_or(false);
        let configurable = descriptor.configurable.unwrap_or(false);
        match (descriptor.get, descriptor.set) {
            (None | Some(None), Some(Some(set))) => match (enumerable, configurable) {
                (true, true) => Self::WriteOnlyEnumerableConfigurableAccessor { set },
                (true, false) => Self::WriteOnlyEnumerableUnconfigurableAccessor { set },
                (false, true) => Self::WriteOnlyUnenumerableConfigurableAccessor { set },
                (false, false) => Self::WriteOnlyUnenumerableUnconfigurableAccessor { set },
            },
            (Some(Some(get)), None | Some(None)) => match (enumerable, configurable) {
                (true, true) => Self::ReadOnlyEnumerableConfigurableAccessor { get },
                (true, false) => Self::ReadOnlyEnumerableUnconfigurableAccessor { get },
                (false, true) => Self::ReadOnlyUnenumerableConfigurableAccessor { get },
                (false, false) => Self::ReadOnlyUnenumerableUnconfigurableAccessor { get },
            },
            (Some(Some(get)), Some(Some(set))) => match (enumerable, configurable) {
                (true, true) => Self::ReadWriteEnumerableConfigurableAccessor { get, set },
                (true, false) => Self::ReadWriteEnumerableUnconfigurableAccessor { get, set },
                (false, true) => Self::ReadWriteUnenumerableConfigurableAccessor { get, set },
                (false, false) => Self::ReadWriteUnenumerableUnconfigurableAccessor { get, set },
            },
            _ => unreachable!(),
        }
    }

    pub(crate) fn from_accessor_descriptor_fields(
        get: Option<Function<'a>>,
        set: Option<Function<'a>>,
        enumerable: bool,
        configurable: bool,
    ) -> Self {
        match (get, set) {
            (None, None) => match (enumerable, configurable) {
                (true, true) => Self::UndefinedEnumerableConfigurableAccessor,
                (true, false) => Self::UndefinedEnumerableUnconfigurableAccessor,
                (false, true) => Self::UndefinedUnenumerableConfigurableAccessor,
                (false, false) => Self::UndefinedUnenumerableUnconfigurableAccessor,
            },
            (None, Some(set)) => match (enumerable, configurable) {
                (true, true) => Self::WriteOnlyEnumerableConfigurableAccessor { set },
                (true, false) => Self::WriteOnlyEnumerableUnconfigurableAccessor { set },
                (false, true) => Self::WriteOnlyUnenumerableConfigurableAccessor { set },
                (false, false) => Self::WriteOnlyUnenumerableUnconfigurableAccessor { set },
            },
            (Some(get), None) => match (enumerable, configurable) {
                (true, true) => Self::ReadOnlyEnumerableConfigurableAccessor { get },
                (true, false) => Self::ReadOnlyEnumerableUnconfigurableAccessor { get },
                (false, true) => Self::ReadOnlyUnenumerableConfigurableAccessor { get },
                (false, false) => Self::ReadOnlyUnenumerableUnconfigurableAccessor { get },
            },
            (Some(get), Some(set)) => match (enumerable, configurable) {
                (true, true) => Self::ReadWriteEnumerableConfigurableAccessor { get, set },
                (true, false) => Self::ReadWriteEnumerableUnconfigurableAccessor { get, set },
                (false, true) => Self::ReadWriteUnenumerableConfigurableAccessor { get, set },
                (false, false) => Self::ReadWriteUnenumerableUnconfigurableAccessor { get, set },
            },
        }
    }

    pub(crate) fn to_property_descriptor(
        descriptor: Option<&Self>,
        value: Option<Value>,
    ) -> PropertyDescriptor<'a> {
        let descriptor =
            descriptor.unwrap_or(&ElementDescriptor::WritableEnumerableConfigurableData);
        let value = value.map(Value::unbind);
        match descriptor {
            ElementDescriptor::WritableEnumerableConfigurableData => PropertyDescriptor {
                value,
                configurable: Some(true),
                enumerable: Some(true),
                writable: Some(true),
                ..Default::default()
            },
            ElementDescriptor::WritableEnumerableUnconfigurableData => PropertyDescriptor {
                value,
                configurable: Some(false),
                enumerable: Some(true),
                writable: Some(true),
                ..Default::default()
            },
            ElementDescriptor::WritableUnenumerableConfigurableData => PropertyDescriptor {
                value,
                configurable: Some(true),
                enumerable: Some(false),
                writable: Some(true),
                ..Default::default()
            },
            ElementDescriptor::WritableUnenumerableUnconfigurableData => PropertyDescriptor {
                value,
                configurable: Some(false),
                enumerable: Some(false),
                writable: Some(true),
                ..Default::default()
            },
            ElementDescriptor::ReadOnlyEnumerableConfigurableData => PropertyDescriptor {
                value,
                configurable: Some(true),
                enumerable: Some(true),
                writable: Some(false),
                ..Default::default()
            },
            ElementDescriptor::ReadOnlyEnumerableUnconfigurableData => PropertyDescriptor {
                value,
                configurable: Some(false),
                enumerable: Some(true),
                writable: Some(false),
                ..Default::default()
            },
            ElementDescriptor::ReadOnlyUnenumerableConfigurableData => PropertyDescriptor {
                value,
                configurable: Some(true),
                enumerable: Some(false),
                writable: Some(false),
                ..Default::default()
            },
            ElementDescriptor::ReadOnlyUnenumerableUnconfigurableData => PropertyDescriptor {
                value,
                configurable: Some(false),
                enumerable: Some(false),
                writable: Some(false),
                ..Default::default()
            },
            ElementDescriptor::UndefinedEnumerableConfigurableAccessor => PropertyDescriptor {
                configurable: Some(true),
                enumerable: Some(true),
                get: Some(None),
                set: Some(None),
                ..Default::default()
            },
            ElementDescriptor::UndefinedEnumerableUnconfigurableAccessor => PropertyDescriptor {
                configurable: Some(false),
                enumerable: Some(true),
                get: Some(None),
                set: Some(None),
                ..Default::default()
            },
            ElementDescriptor::UndefinedUnenumerableConfigurableAccessor => PropertyDescriptor {
                configurable: Some(true),
                enumerable: Some(false),
                get: Some(None),
                set: Some(None),
                ..Default::default()
            },
            ElementDescriptor::UndefinedUnenumerableUnconfigurableAccessor => PropertyDescriptor {
                configurable: Some(false),
                enumerable: Some(false),
                get: Some(None),
                set: Some(None),
                ..Default::default()
            },
            ElementDescriptor::ReadOnlyEnumerableConfigurableAccessor { get } => {
                PropertyDescriptor {
                    configurable: Some(true),
                    enumerable: Some(true),
                    get: Some(Some(*get)),
                    set: Some(None),
                    ..Default::default()
                }
            }
            ElementDescriptor::ReadOnlyEnumerableUnconfigurableAccessor { get } => {
                PropertyDescriptor {
                    configurable: Some(false),
                    enumerable: Some(true),
                    get: Some(Some(*get)),
                    set: Some(None),
                    ..Default::default()
                }
            }
            ElementDescriptor::ReadOnlyUnenumerableConfigurableAccessor { get } => {
                PropertyDescriptor {
                    configurable: Some(true),
                    enumerable: Some(false),
                    get: Some(Some(*get)),
                    set: Some(None),
                    ..Default::default()
                }
            }
            ElementDescriptor::ReadOnlyUnenumerableUnconfigurableAccessor { get } => {
                PropertyDescriptor {
                    configurable: Some(false),
                    enumerable: Some(false),
                    get: Some(Some(*get)),
                    set: Some(None),
                    ..Default::default()
                }
            }
            ElementDescriptor::WriteOnlyEnumerableConfigurableAccessor { set } => {
                PropertyDescriptor {
                    configurable: Some(true),
                    enumerable: Some(true),
                    get: Some(None),
                    set: Some(Some(*set)),
                    ..Default::default()
                }
            }
            ElementDescriptor::WriteOnlyEnumerableUnconfigurableAccessor { set } => {
                PropertyDescriptor {
                    configurable: Some(false),
                    enumerable: Some(true),
                    get: Some(None),
                    set: Some(Some(*set)),
                    ..Default::default()
                }
            }
            ElementDescriptor::WriteOnlyUnenumerableConfigurableAccessor { set } => {
                PropertyDescriptor {
                    configurable: Some(true),
                    enumerable: Some(false),
                    get: Some(None),
                    set: Some(Some(*set)),
                    ..Default::default()
                }
            }
            ElementDescriptor::WriteOnlyUnenumerableUnconfigurableAccessor { set } => {
                PropertyDescriptor {
                    configurable: Some(false),
                    enumerable: Some(false),
                    get: Some(None),
                    set: Some(Some(*set)),
                    ..Default::default()
                }
            }
            ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { get, set } => {
                PropertyDescriptor {
                    configurable: Some(true),
                    enumerable: Some(true),
                    get: Some(Some(*get)),
                    set: Some(Some(*set)),
                    ..Default::default()
                }
            }
            ElementDescriptor::ReadWriteEnumerableUnconfigurableAccessor { get, set } => {
                PropertyDescriptor {
                    configurable: Some(false),
                    enumerable: Some(true),
                    get: Some(Some(*get)),
                    set: Some(Some(*set)),
                    ..Default::default()
                }
            }
            ElementDescriptor::ReadWriteUnenumerableConfigurableAccessor { get, set } => {
                PropertyDescriptor {
                    configurable: Some(true),
                    enumerable: Some(false),
                    get: Some(Some(*get)),
                    set: Some(Some(*set)),
                    ..Default::default()
                }
            }
            ElementDescriptor::ReadWriteUnenumerableUnconfigurableAccessor { get, set } => {
                PropertyDescriptor {
                    configurable: Some(false),
                    enumerable: Some(false),
                    get: Some(Some(*get)),
                    set: Some(Some(*set)),
                    ..Default::default()
                }
            }
        }
    }

    pub(crate) fn getter_function<'gc>(&self, gc: NoGcScope<'gc, '_>) -> Option<Function<'gc>> {
        match self {
            ElementDescriptor::ReadOnlyEnumerableConfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyEnumerableUnconfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyUnenumerableConfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyUnenumerableUnconfigurableAccessor { get }
            | ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { get, .. }
            | ElementDescriptor::ReadWriteEnumerableUnconfigurableAccessor { get, .. }
            | ElementDescriptor::ReadWriteUnenumerableConfigurableAccessor { get, .. }
            | ElementDescriptor::ReadWriteUnenumerableUnconfigurableAccessor { get, .. } => {
                Some(get.bind(gc))
            }
            _ => None,
        }
    }

    pub(crate) fn setter_function<'gc>(&self, gc: NoGcScope<'gc, '_>) -> Option<Function<'gc>> {
        match self {
            ElementDescriptor::WriteOnlyEnumerableConfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyEnumerableUnconfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyUnenumerableConfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyUnenumerableUnconfigurableAccessor { set }
            | ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { set, .. }
            | ElementDescriptor::ReadWriteEnumerableUnconfigurableAccessor { set, .. }
            | ElementDescriptor::ReadWriteUnenumerableConfigurableAccessor { set, .. }
            | ElementDescriptor::ReadWriteUnenumerableUnconfigurableAccessor { set, .. } => {
                Some(set.bind(gc))
            }
            _ => None,
        }
    }

    pub(crate) fn is_writable(&self) -> Option<bool> {
        match self {
            ElementDescriptor::WritableEnumerableConfigurableData
            | ElementDescriptor::WritableEnumerableUnconfigurableData
            | ElementDescriptor::WritableUnenumerableConfigurableData
            | ElementDescriptor::WritableUnenumerableUnconfigurableData => Some(true),
            ElementDescriptor::ReadOnlyEnumerableConfigurableData
            | ElementDescriptor::ReadOnlyEnumerableUnconfigurableData
            | ElementDescriptor::ReadOnlyUnenumerableConfigurableData
            | ElementDescriptor::ReadOnlyUnenumerableUnconfigurableData => Some(false),
            _ => None,
        }
    }

    pub(crate) fn is_enumerable(&self) -> bool {
        matches!(
            self,
            ElementDescriptor::WritableEnumerableConfigurableData
                | ElementDescriptor::WritableEnumerableUnconfigurableData
                | ElementDescriptor::ReadOnlyEnumerableConfigurableData
                | ElementDescriptor::ReadOnlyEnumerableUnconfigurableData
                | ElementDescriptor::UndefinedEnumerableConfigurableAccessor
                | ElementDescriptor::UndefinedEnumerableUnconfigurableAccessor
                | ElementDescriptor::ReadOnlyEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadOnlyEnumerableUnconfigurableAccessor { .. }
                | ElementDescriptor::WriteOnlyEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::WriteOnlyEnumerableUnconfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteEnumerableUnconfigurableAccessor { .. }
        )
    }

    pub(crate) fn is_configurable(&self) -> bool {
        matches!(
            self,
            ElementDescriptor::WritableEnumerableConfigurableData
                | ElementDescriptor::WritableUnenumerableConfigurableData
                | ElementDescriptor::ReadOnlyEnumerableConfigurableData
                | ElementDescriptor::ReadOnlyUnenumerableConfigurableData
                | ElementDescriptor::UndefinedEnumerableConfigurableAccessor
                | ElementDescriptor::UndefinedUnenumerableConfigurableAccessor
                | ElementDescriptor::ReadOnlyEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadOnlyUnenumerableConfigurableAccessor { .. }
                | ElementDescriptor::WriteOnlyEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::WriteOnlyUnenumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { .. }
                | ElementDescriptor::ReadWriteUnenumerableConfigurableAccessor { .. },
        )
    }

    pub(crate) fn is_accessor_descriptor(&self) -> bool {
        !matches!(
            self,
            ElementDescriptor::WritableEnumerableConfigurableData
                | ElementDescriptor::WritableEnumerableUnconfigurableData
                | ElementDescriptor::WritableUnenumerableConfigurableData
                | ElementDescriptor::WritableUnenumerableUnconfigurableData
                | ElementDescriptor::ReadOnlyEnumerableConfigurableData
                | ElementDescriptor::ReadOnlyEnumerableUnconfigurableData
                | ElementDescriptor::ReadOnlyUnenumerableConfigurableData
                | ElementDescriptor::ReadOnlyUnenumerableUnconfigurableData
        )
    }

    pub(crate) fn is_data_descriptor(&self) -> bool {
        !self.is_accessor_descriptor()
    }
}

/// Fixed-size storages of `N` slots each, kept back to back in one flat
/// vector: storage `i` is `data[i * N..(i + 1) * N]`.
///
/// A `Vec<[T; N]>` cannot grow without building a `[T; N]` on the stack, and
/// `N` goes up to 2^32.
///
/// A *dynamic* StridedVec (the element values of the classes above 2^16
/// slots) instead keeps every storage in a vector of its own that holds only
/// the slots in use: its slices are shorter than `N`, and the users grow a
/// storage with [`StridedVec::ensure_len`] before writing past its end. An
/// array of 70 000 elements then takes 70 000 slots, not 2^24.
#[derive(Debug)]
pub(crate) struct StridedVec<T, const N: usize> {
    data: Vec<T>,
    storages: Vec<Vec<T>>,
    dynamic: bool,
}

impl<T, const N: usize> Default for StridedVec<T, N> {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            storages: Vec::new(),
            dynamic: false,
        }
    }
}

impl<T: Copy, const N: usize> StridedVec<T, N> {
    fn with_capacity(cap: usize) -> Self {
        Self {
            data: Vec::with_capacity(
                cap.checked_mul(N)
                    .expect("Element storage capacity overflow"),
            ),
            storages: Vec::new(),
            dynamic: false,
        }
    }

    /// An empty dynamic StridedVec (see the type's documentation).
    fn new_dynamic() -> Self {
        Self {
            data: Vec::new(),
            storages: Vec::new(),
            dynamic: true,
        }
    }

    /// Number of storages.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        if self.dynamic {
            self.storages.len()
        } else {
            self.data.len() / N
        }
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Slots held by all storages, for the heap size estimate.
    pub(crate) fn slot_count(&self) -> usize {
        if self.dynamic {
            self.storages.iter().map(Vec::capacity).sum()
        } else {
            self.data.len()
        }
    }

    #[inline]
    fn range(index: usize) -> Option<core::ops::Range<usize>> {
        let start = index.checked_mul(N)?;
        Some(start..start.checked_add(N)?)
    }

    /// The storage at `index`: all `N` slots, or the slots in use of a
    /// dynamic storage.
    #[inline]
    pub(crate) fn get(&self, index: usize) -> Option<&[T]> {
        if self.dynamic {
            self.storages.get(index).map(Vec::as_slice)
        } else {
            self.data.get(Self::range(index)?)
        }
    }

    /// The storage at `index` as mutable: all `N` slots, or the slots in use
    /// of a dynamic storage.
    #[inline]
    pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut [T]> {
        if self.dynamic {
            self.storages.get_mut(index).map(Vec::as_mut_slice)
        } else {
            self.data.get_mut(Self::range(index)?)
        }
    }

    /// Makes the dynamic storage at `index` hold at least `len` slots, the
    /// new ones set to `filler`. Growth is amortised by `Vec`. Does nothing
    /// for a StridedVec that is not dynamic, whose storages have `N` slots.
    pub(crate) fn ensure_len(
        &mut self,
        index: usize,
        len: usize,
        filler: T,
    ) -> Result<(), TryReserveError> {
        if !self.dynamic {
            return Ok(());
        }
        let len = len.min(N);
        let storage = self.storages.get_mut(index).expect("Invalid storage index");
        if storage.len() < len {
            storage.try_reserve(len - storage.len())?;
            storage.resize(len, filler);
        }
        Ok(())
    }

    /// All slots of all storages as one flat slice; None for a dynamic
    /// StridedVec.
    fn as_flat_mut(&mut self) -> Option<&mut [T]> {
        if self.dynamic {
            None
        } else {
            Some(&mut self.data)
        }
    }

    fn last_index<I: HeapIndexHandle>(&self) -> I {
        assert!(!self.is_empty());
        I::from_index(self.len() - 1)
    }

    /// Append a storage holding `items` followed by `filler` up to `N` slots
    /// (a dynamic storage holds only `items`).
    fn try_push(
        &mut self,
        items: impl IntoIterator<Item = T>,
        filler: T,
    ) -> Result<usize, TryReserveError> {
        if self.dynamic {
            self.storages.try_reserve(1)?;
            let mut storage = Vec::new();
            for item in items.into_iter().take(N) {
                storage.try_reserve(1)?;
                storage.push(item);
            }
            self.storages.push(storage);
            return Ok(self.storages.len() - 1);
        }
        self.data.try_reserve(N)?;
        let start = self.data.len();
        self.data.extend(items.into_iter().take(N));
        self.data.resize(start + N, filler);
        Ok(start / N)
    }

    /// Append a storage holding the first `len` slots of the storage at
    /// `index`, followed by `filler` up to `N` slots (for a dynamic storage,
    /// up to the source's length).
    fn try_push_within(
        &mut self,
        index: usize,
        len: usize,
        filler: T,
    ) -> Result<usize, TryReserveError> {
        assert!(len <= N);
        if self.dynamic {
            let source = self.storages.get(index).expect("Invalid storage index");
            let len = len.min(source.len());
            let total = source.len();
            let mut storage = Vec::new();
            storage.try_reserve(total)?;
            storage.extend_from_slice(&source[..len]);
            storage.resize(total, filler);
            self.storages.try_reserve(1)?;
            self.storages.push(storage);
            return Ok(self.storages.len() - 1);
        }
        let source = Self::range(index).expect("Invalid storage index");
        assert!(source.end <= self.data.len(), "Invalid storage index");
        self.data.try_reserve(N)?;
        let start = self.data.len();
        self.data
            .extend_from_within(source.start..source.start + len);
        self.data.resize(start + N, filler);
        Ok(start / N)
    }

    /// Append a copy of the storage at `index`, all its slots.
    fn push_copy(&mut self, index: usize) -> usize {
        if self.dynamic {
            let copy = self
                .storages
                .get(index)
                .expect("Invalid storage index")
                .clone();
            self.storages.push(copy);
            return self.storages.len() - 1;
        }
        let source = Self::range(index).expect("Invalid storage index");
        let start = self.data.len();
        self.data.extend_from_within(source);
        start / N
    }

    /// Keep only the storages for which `keep` returns true, moving the kept
    /// storages down in order.
    pub(crate) fn retain_mut(&mut self, mut keep: impl FnMut(&mut [T]) -> bool) {
        if self.dynamic {
            self.storages.retain_mut(|storage| keep(storage));
            return;
        }
        let count = self.len();
        let mut kept = 0;
        for index in 0..count {
            let range = index * N..(index + 1) * N;
            if keep(&mut self.data[range.clone()]) {
                if kept != index {
                    self.data.copy_within(range, kept * N);
                }
                kept += 1;
            }
        }
        self.data.truncate(kept * N);
    }

    /// Drop the storages not marked in `range` and sweep the slots of the
    /// kept ones.
    pub(crate) fn sweep(
        &mut self,
        compactions: &CompactionLists,
        range: &BitRange,
        bits: &[AtomicBits],
    ) where
        T: HeapMarkAndSweep,
    {
        assert_eq!(self.len(), range.len());
        let mut iter = range.iter(bits);
        self.retain_mut(|storage| {
            let do_retain = iter.next().unwrap();
            if do_retain {
                for slot in storage.iter_mut() {
                    slot.sweep_values(compactions);
                }
            }
            do_retain
        });
        release_spare_capacity(&mut self.data);
        release_spare_capacity(&mut self.storages);
    }
}

impl<const N: usize> Index<ElementIndex<'_>> for StridedVec<Option<Value<'static>>, N> {
    type Output = [Option<Value<'static>>];

    fn index(&self, index: ElementIndex) -> &Self::Output {
        self.get(index.get_index())
            .expect("Invalid ElementsVector: No item at index")
    }
}

impl<const N: usize> IndexMut<ElementIndex<'_>> for StridedVec<Option<Value<'static>>, N> {
    fn index_mut(&mut self, index: ElementIndex<'_>) -> &mut Self::Output {
        self.get_mut(index.get_index())
            .expect("Invalid ElementsVector: No item at index")
    }
}

impl<const N: usize> Index<PropertyKeyIndex<'_>> for StridedVec<PropertyKey<'static>, N> {
    type Output = [PropertyKey<'static>];

    fn index(&self, index: PropertyKeyIndex) -> &Self::Output {
        self.get(index.get_index())
            .expect("Invalid PropertyKeyIndex: No item at index")
    }
}

impl<const N: usize> IndexMut<PropertyKeyIndex<'_>> for StridedVec<PropertyKey<'static>, N> {
    fn index_mut(&mut self, index: PropertyKeyIndex<'_>) -> &mut Self::Output {
        self.get_mut(index.get_index())
            .expect("Invalid PropertyKeyIndex: No item at index")
    }
}

#[derive(Debug)]
pub(crate) struct ElementArray<const N: usize> {
    pub values: StridedVec<Option<Value<'static>>, N>,
    pub descriptors: AHashMap<ElementIndex<'static>, ElementDescriptorMap<'static>>,
}

/// Element storages with more slots than this keep only the slots in use
/// (see [`StridedVec`]).
const DYNAMIC_STORAGE_SLOTS: usize = 1 << 16;

impl<const N: usize> Default for ElementArray<N> {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl<const N: usize> ElementArray<N> {
    /// All value slots of this backing store as one flat slice, with the
    /// stride of one elements storage; None if the storages are dynamic.
    fn as_flat_mut(&mut self) -> Option<(&mut [Option<Value<'static>>], usize)> {
        self.values.as_flat_mut().map(|flat| (flat, N))
    }

    pub(crate) fn with_capacity(cap: usize) -> Self {
        const { assert!(N.is_power_of_two()) }
        Self {
            values: if N > DYNAMIC_STORAGE_SLOTS {
                StridedVec::new_dynamic()
            } else {
                StridedVec::with_capacity(cap)
            },
            descriptors: Default::default(),
        }
    }

    fn get_values(&self, vector: &ElementsVector) -> &[Option<Value<'static>>] {
        self.get_values_raw(vector.elements_index, vector.len())
    }

    #[inline]
    fn get_values_raw(&self, index: ElementIndex, len: u32) -> &[Option<Value<'static>>] {
        &self.values[index][0..len as usize]
    }

    fn get_values_mut(&mut self, vector: &ElementsVector) -> &mut [Option<Value<'static>>] {
        &mut self.values[vector.elements_index][0..vector.len() as usize]
    }

    #[inline]
    fn get_values_mut_raw(
        &mut self,
        index: ElementIndex,
        len: u32,
    ) -> &mut [Option<Value<'static>>] {
        &mut self.values[index][0..len as usize]
    }

    fn get_descriptors_and_values_raw<'gc>(
        &self,
        index: ElementIndex<'gc>,
        len: u32,
    ) -> ElementStorageRef<'_, 'gc> {
        ElementStorageRef {
            values: &self.values[index][0..len as usize],
            descriptors: self.descriptors.get(&index),
        }
    }

    #[inline]
    fn get_descriptors_and_values_mut(&mut self, vector: &ElementsVector) -> ElementStorageMut<'_> {
        ElementStorageMut {
            values: &mut self
                .values
                .get_mut(vector.elements_index.get_index())
                .unwrap()[0..vector.len() as usize],
            descriptors: self.descriptors.entry(vector.elements_index.unbind()),
        }
    }

    fn get_descriptors_and_values_mut_raw(
        &mut self,
        index: ElementIndex<'static>,
        len: u32,
    ) -> ElementStorageMut<'_> {
        ElementStorageMut {
            values: &mut self.values[index][0..len as usize],
            descriptors: self.descriptors.entry(index),
        }
    }

    /// Get the currently reserved values storage and any possible descriptors
    /// as mutable.
    fn get_descriptors_and_values_uninit_raw(
        &mut self,
        index: ElementIndex<'static>,
    ) -> ElementStorageUninit<'_> {
        ElementStorageUninit {
            values: &mut self.values[index],
            descriptors: self.descriptors.entry(index),
        }
    }

    fn push(
        &mut self,
        source: &[Option<Value>],
        descriptors: Option<ElementDescriptorMap<'static>>,
    ) -> Result<ElementIndex<'static>, TryReserveError> {
        assert!(source.len() <= N);
        if descriptors.is_some() {
            self.descriptors.try_reserve(1)?;
        }
        self.values
            .try_push(source.iter().map(|&v| v.unbind()), None)?;
        let index = self.values.last_index();
        if let Some(descriptors) = descriptors {
            self.descriptors.insert(index, descriptors);
        }
        Ok(index)
    }

    fn push_with_removal(
        &mut self,
        source: ElementStorageRef,
        removal_index: u32,
    ) -> Result<ElementIndex<'static>, TryReserveError> {
        let source_length = source.values.len();
        let target_length = source_length - 1;
        assert!((removal_index as usize) < source_length);
        assert!(target_length <= N);
        if source.descriptors.is_some() {
            self.descriptors.try_reserve(1)?;
        }
        let removal_index_usize = removal_index as usize;
        self.values.try_push(
            source.values[..removal_index_usize]
                .iter()
                .chain(&source.values[removal_index_usize + 1..])
                .map(|&v| v.unbind()),
            None,
        )?;
        let key = self.values.last_index();
        if let Some(descriptors) = source.descriptors {
            let descriptors = descriptors
                .iter()
                .filter(|(k, _)| **k != removal_index)
                .map(|(k, v)| {
                    let k = if *k > removal_index { k - 1 } else { *k };
                    (k, v.unbind())
                })
                .collect::<ElementDescriptorMap>();
            let inserted_new = self.descriptors.insert(key, descriptors).is_none();
            debug_assert!(inserted_new);
        }
        Ok(key)
    }

    /// Append a copy of the storage at `index`, all `N` slots, without its
    /// descriptors.
    fn push_copy(&mut self, index: ElementIndex) -> ElementIndex<'static> {
        self.values.push_copy(index.get_index());
        self.values.last_index()
    }
}

/// Element arrays of up to 2 elements
pub(crate) type ElementArray2Pow1 = ElementArray<2>;
/// Element arrays of up to 4 elements
pub(crate) type ElementArray2Pow2 = ElementArray<4>;
/// Element arrays of up to 8 elements
pub(crate) type ElementArray2Pow3 = ElementArray<8>;
/// Element arrays of up to 16 elements
pub(crate) type ElementArray2Pow4 = ElementArray<16>;
/// Element arrays of up to 64 elements
pub(crate) type ElementArray2Pow6 = ElementArray<64>;
/// Element arrays of up to 256 elements
pub(crate) type ElementArray2Pow8 = ElementArray<256>;
/// Element arrays of up to 1024 elements
pub(crate) type ElementArray2Pow10 = ElementArray<1024>;
/// Element arrays of up to 4096 elements
pub(crate) type ElementArray2Pow12 = ElementArray<4096>;
/// Element arrays of up to 65536 elements
pub(crate) type ElementArray2Pow16 = ElementArray<65536>;
/// Element arrays of up to 16777216 elements
pub(crate) type ElementArray2Pow24 = ElementArray<16777216>;
/// Slots of the largest size class: 2^32. On a 32-bit target, where a `usize`
/// cannot hold 2^32, it is 2^31, the largest power of two that fits. The
/// storages of this class are dynamic (they hold only the slots in use), so
/// this is an upper bound and nothing else, and 2^31 slots of eight bytes
/// each do not fit a 32-bit address space anyway: an allocation that large
/// fails before the bound is reached.
#[cfg(target_pointer_width = "64")]
pub(crate) const LARGEST_CLASS_SLOTS: usize = 1 << 32;
#[cfg(not(target_pointer_width = "64"))]
pub(crate) const LARGEST_CLASS_SLOTS: usize = 1 << (usize::BITS - 1);
const _LARGEST_CLASS_IS_DYNAMIC: () = assert!(LARGEST_CLASS_SLOTS > DYNAMIC_STORAGE_SLOTS);

/// Element arrays of up to 4294967296 elements
pub(crate) type ElementArray2Pow32 = ElementArray<LARGEST_CLASS_SLOTS>;

/// The value of a key slot past the keys of every shape using the storage. It
/// is a Private Name that is never handed out: `Agent::create_private_names`
/// only hands out names below `u32::MAX`.
pub(crate) const FREE_KEY_SLOT: PropertyKey<'static> =
    PropertyKey::PrivateName(PrivateName::from_u32(u32::MAX));

/// Property key arrays of up to N keys
#[derive(Debug)]
#[repr(transparent)]
pub(crate) struct PropertyKeyArray<const N: usize> {
    pub keys: StridedVec<PropertyKey<'static>, N>,
}

impl<const N: usize> Default for PropertyKeyArray<N> {
    fn default() -> Self {
        Self::with_capacity(0)
    }
}

impl<const N: usize> PropertyKeyArray<N> {
    pub(crate) fn with_capacity(cap: usize) -> Self {
        const { assert!(N.is_power_of_two()) }
        Self {
            keys: if N > DYNAMIC_STORAGE_SLOTS {
                StridedVec::new_dynamic()
            } else {
                StridedVec::with_capacity(cap)
            },
        }
    }

    fn get_raw<'a>(&self, keys_index: PropertyKeyIndex<'a>, len: u32) -> &[PropertyKey<'a>] {
        let keys = &self.keys[keys_index][0..len as usize];
        debug_assert!(keys.last() != Some(&FREE_KEY_SLOT));
        keys
    }

    fn get_uninit(&mut self, index: PropertyKeyIndex) -> &mut [PropertyKey<'static>] {
        &mut self.keys[index]
    }

    fn push(
        &mut self,
        source: &[PropertyKey],
    ) -> Result<PropertyKeyIndex<'static>, TryReserveError> {
        assert!(source.len() <= N);
        self.keys
            .try_push(source.iter().map(|&k| k.unbind()), FREE_KEY_SLOT)?;
        Ok(self.keys.last_index())
    }

    fn push_with_removal(
        &mut self,
        source: &[PropertyKey],
        removal_index: usize,
    ) -> Result<PropertyKeyIndex<'static>, TryReserveError> {
        let source_length = source.len();
        let target_length = source_length - 1;
        assert!(removal_index < source_length);
        assert!(target_length <= N);
        self.keys.try_push(
            source[..removal_index]
                .iter()
                .chain(&source[removal_index.wrapping_add(1)..])
                .map(|&k| k.unbind()),
            FREE_KEY_SLOT,
        )?;
        Ok(self.keys.last_index())
    }

    fn push_key(&mut self, index: PropertyKeyIndex, len: u32, key: PropertyKey) {
        let slot = &mut self.keys[index][len as usize];
        debug_assert!(*slot == FREE_KEY_SLOT);
        *slot = key.unbind();
    }

    fn remove(&mut self, index: PropertyKeyIndex, len: u32, removal_index: u32) {
        let len = usize::try_from(len).unwrap_or(usize::MAX);
        let keys = &mut self.keys[index][..len];
        let Some(next_index) = usize::try_from(removal_index)
            .ok()
            .and_then(|i| i.checked_add(1))
        else {
            // Removing at the very edge of memory.
            return;
        };
        keys.copy_within(next_index.., removal_index as usize);
        if let Some(slot) = keys.last_mut() {
            *slot = FREE_KEY_SLOT;
        }
    }

    /// Push a copy of a PropertyKey storage into the PropertyKeyArray, copying
    /// the first len keys.
    fn push_within<'a>(
        &mut self,
        key_index: PropertyKeyIndex<'a>,
        len: u32,
    ) -> Result<PropertyKeyIndex<'a>, TryReserveError> {
        self.keys
            .try_push_within(key_index.get_index(), len as usize, FREE_KEY_SLOT)?;
        Ok(self.keys.last_index())
    }

    /// Push a copy of a PropertyKey storage into the PropertyKeyArray, copying
    /// the first len keys.
    fn push_within_with_removal<'a>(
        &mut self,
        key_index: PropertyKeyIndex<'a>,
        len: u32,
        removal_index: usize,
    ) -> Result<PropertyKeyIndex<'a>, TryReserveError> {
        let len = len as usize;
        self.keys
            .try_push_within(key_index.get_index(), N, FREE_KEY_SLOT)?;
        let last = self.keys.get_mut(self.keys.len() - 1).unwrap();
        debug_assert!(removal_index < last.len());
        debug_assert!(last[removal_index] != FREE_KEY_SLOT);
        debug_assert!(removal_index < len);
        last.copy_within(removal_index.saturating_add(1)..len, removal_index);
        last[len.wrapping_sub(1)] = FREE_KEY_SLOT;
        Ok(self.keys.last_index())
    }
}

/// Property key arrays of up to 2 elements
pub(crate) type PropertyKeyArray2Pow1 = PropertyKeyArray<2>;
/// Property key arrays of up to 4 elements
pub(crate) type PropertyKeyArray2Pow2 = PropertyKeyArray<4>;
/// Property key arrays of up to 8 elements
pub(crate) type PropertyKeyArray2Pow3 = PropertyKeyArray<8>;
/// Property key arrays of up to 16 elements
pub(crate) type PropertyKeyArray2Pow4 = PropertyKeyArray<16>;
/// Property key arrays of up to 64 elements
pub(crate) type PropertyKeyArray2Pow6 = PropertyKeyArray<64>;
/// Property key arrays of up to 256 elements
pub(crate) type PropertyKeyArray2Pow8 = PropertyKeyArray<256>;
/// Property key arrays of up to 1024 elements
pub(crate) type PropertyKeyArray2Pow10 = PropertyKeyArray<1024>;
/// Property key arrays of up to 4096 elements
pub(crate) type PropertyKeyArray2Pow12 = PropertyKeyArray<4096>;
/// Property key arrays of up to 65536 elements
pub(crate) type PropertyKeyArray2Pow16 = PropertyKeyArray<65536>;
/// Property key arrays of up to 16777216 elements
pub(crate) type PropertyKeyArray2Pow24 = PropertyKeyArray<16777216>;
/// Property key arrays of up to 4294967296 elements
pub(crate) type PropertyKeyArray2Pow32 = PropertyKeyArray<LARGEST_CLASS_SLOTS>;

#[derive(Debug)]
pub(crate) struct ElementArrays {
    /// up to 2 elements
    pub(crate) k2pow1: PropertyKeyArray2Pow1,
    pub(crate) e2pow1: ElementArray2Pow1,
    /// up to 4 elements
    pub(crate) k2pow2: PropertyKeyArray2Pow2,
    pub(crate) e2pow2: ElementArray2Pow2,
    /// up to 8 elements
    pub(crate) k2pow3: PropertyKeyArray2Pow3,
    pub(crate) e2pow3: ElementArray2Pow3,
    /// up to 16 elements
    pub(crate) k2pow4: PropertyKeyArray2Pow4,
    pub(crate) e2pow4: ElementArray2Pow4,
    /// up to 64 elements
    pub(crate) k2pow6: PropertyKeyArray2Pow6,
    pub(crate) e2pow6: ElementArray2Pow6,
    /// up to 256 elements
    pub(crate) k2pow8: PropertyKeyArray2Pow8,
    pub(crate) e2pow8: ElementArray2Pow8,
    /// up to 1024 elements
    pub(crate) k2pow10: PropertyKeyArray2Pow10,
    pub(crate) e2pow10: ElementArray2Pow10,
    /// up to 4096 elements
    pub(crate) k2pow12: PropertyKeyArray2Pow12,
    pub(crate) e2pow12: ElementArray2Pow12,
    /// up to 65536 elements
    pub(crate) k2pow16: PropertyKeyArray2Pow16,
    pub(crate) e2pow16: ElementArray2Pow16,
    /// up to 16777216 elements
    pub(crate) k2pow24: PropertyKeyArray2Pow24,
    pub(crate) e2pow24: ElementArray2Pow24,
    /// up to 4294967296 elements
    pub(crate) k2pow32: PropertyKeyArray2Pow32,
    pub(crate) e2pow32: ElementArray2Pow32,
    /// Hashed key offsets of the larger key storages.
    pub(crate) key_offsets: KeyOffsets,
}

/// From this many keys on, a key is looked up in a shape through
/// [`KeyOffsets`] instead of a linear search of the shape's keys.
pub(crate) const KEY_OFFSETS_MIN_LEN: usize = 32;

/// Hashed offsets of the keys in the larger key storages, so that looking up
/// a key in, or adding a key to, an object with many properties is not
/// linear in its number of properties.
///
/// A storage's table is built on its first lookup and extended as shapes
/// that share the storage append keys to it. The slots below a shape's
/// length never change, except in the storages of intrinsic shapes, whose
/// in-place removals and insertions drop the table
/// ([`KeyOffsets::invalidate`]). Every garbage collection moves storages and
/// keys and drops all tables. Storages are never reused between collections,
/// so a table always describes the storage it is keyed by.
///
/// Keys hash by identity ([`PropertyKey::identity_hash`]) under random
/// per-Agent keys, like the other maps keyed by engine handles.
#[derive(Debug, Default)]
pub(crate) struct KeyOffsets {
    hasher: HandleState,
    storages: RefCell<AHashMap<u64, StorageKeyOffsets>>,
}

#[derive(Debug, Default)]
struct StorageKeyOffsets {
    /// Number of leading storage slots entered in the table.
    covered: u32,
    /// The first offset of each key among the covered slots.
    table: HashTable<(PropertyKey<'static>, u32)>,
}

impl KeyOffsets {
    #[inline]
    fn storage_id(cap: ElementArrayKey, index: PropertyKeyIndex) -> u64 {
        (u64::from(cap as u8) << 56) | index.get_index() as u64
    }

    /// The offset of `key` in `keys`, which are the keys of a shape whose
    /// key storage is `(cap, index)`. Same result as a linear search.
    pub(crate) fn find(
        &self,
        cap: ElementArrayKey,
        index: PropertyKeyIndex,
        keys: &[PropertyKey],
        key: PropertyKey,
    ) -> Option<usize> {
        if keys.len() < KEY_OFFSETS_MIN_LEN {
            return keys.iter().position(|k| *k == key);
        }
        let hasher = &self.hasher;
        let mut storages = self.storages.borrow_mut();
        let offsets = storages.entry(Self::storage_id(cap, index)).or_default();
        while (offsets.covered as usize) < keys.len() {
            let offset = offsets.covered;
            let slot = keys[offset as usize].unbind();
            let hash = slot.identity_hash(hasher);
            if offsets.table.find(hash, |(k, _)| *k == slot).is_none() {
                offsets
                    .table
                    .insert_unique(hash, (slot, offset), |(k, _)| k.identity_hash(hasher));
            }
            offsets.covered = offset + 1;
        }
        let key = key.unbind();
        let found = offsets
            .table
            .find(key.identity_hash(hasher), |(k, _)| *k == key)
            .map(|(_, offset)| *offset as usize)
            .filter(|offset| *offset < keys.len());
        debug_assert_eq!(found, keys.iter().position(|k| *k == key));
        found
    }

    /// Drops the table of a key storage whose slots were changed in place.
    pub(crate) fn invalidate(&mut self, cap: ElementArrayKey, index: PropertyKeyIndex) {
        let storages = self.storages.get_mut();
        if !storages.is_empty() {
            storages.remove(&Self::storage_id(cap, index));
        }
    }

    /// Drops every table; a garbage collection moves storages and keys.
    pub(crate) fn clear(&mut self) {
        let storages = self.storages.get_mut();
        if !storages.is_empty() {
            *storages = AHashMap::default();
        }
    }
}

impl ElementArrays {
    fn push_values(
        &mut self,
        key: ElementArrayKey,
        source: &[Option<Value>],
        descriptors: Option<ElementDescriptorMap<'static>>,
    ) -> Result<ElementIndex<'static>, TryReserveError> {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        match key {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                assert!(source.is_empty() && descriptors.is_none());
                Ok(ElementIndex::ZERO)
            }
            ElementArrayKey::E1 => e2pow1.push(source, descriptors),
            ElementArrayKey::E2 => e2pow2.push(source, descriptors),
            ElementArrayKey::E3 => e2pow3.push(source, descriptors),
            ElementArrayKey::E4 => e2pow4.push(source, descriptors),
            ElementArrayKey::E6 => e2pow6.push(source, descriptors),
            ElementArrayKey::E8 => e2pow8.push(source, descriptors),
            ElementArrayKey::E10 => e2pow10.push(source, descriptors),
            ElementArrayKey::E12 => e2pow12.push(source, descriptors),
            ElementArrayKey::E16 => e2pow16.push(source, descriptors),
            ElementArrayKey::E24 => e2pow24.push(source, descriptors),
            ElementArrayKey::E32 => e2pow32.push(source, descriptors),
        }
    }

    fn reserve_elements(
        &mut self,
        elements_vector: &mut ElementsVector,
        new_len: u32,
    ) -> Result<(), TryReserveError> {
        if new_len <= elements_vector.cap.capacity() {
            // Already big enough, no need to grow
            return self.ensure_values_len(
                elements_vector.cap,
                elements_vector.elements_index,
                new_len,
            );
        }
        self.reserve_elements_raw(
            &mut elements_vector.elements_index,
            &mut elements_vector.cap,
            elements_vector.len,
            new_len,
        )
    }

    pub(crate) fn reserve_elements_raw(
        &mut self,
        index: &mut ElementIndex,
        cap: &mut ElementArrayKey,
        old_len: u32,
        new_len: u32,
    ) -> Result<(), TryReserveError> {
        let new_key = ElementArrayKey::from(new_len);
        assert_ne!(new_key, *cap);
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        let new_index = match new_key {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                // 0 <= elements_vector.cap for all possible values.
                unreachable!();
            }
            ElementArrayKey::E1 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        unreachable!()
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow1.push(source, descriptors.cloned())
            }
            ElementArrayKey::E2 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        unreachable!()
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow2.push(source, descriptors.cloned())
            }
            ElementArrayKey::E3 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        unreachable!()
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow3.push(source, descriptors.cloned())
            }
            ElementArrayKey::E4 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => unreachable!(),
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow4.push(source, descriptors.cloned())
            }
            ElementArrayKey::E6 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => unreachable!(),
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow6.push(source, descriptors.cloned())
            }
            ElementArrayKey::E8 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => unreachable!(),
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow8.push(source, descriptors.cloned())
            }
            ElementArrayKey::E10 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => unreachable!(),
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow10.push(source, descriptors.cloned())
            }
            ElementArrayKey::E12 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => unreachable!(),
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow12.push(source, descriptors.cloned())
            }
            ElementArrayKey::E16 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => unreachable!(),
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow16.push(source, descriptors.cloned())
            }
            ElementArrayKey::E24 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => unreachable!(),
                    ElementArrayKey::E32 => {
                        e2pow32.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                };
                e2pow24.push(source, descriptors.cloned())
            }
            ElementArrayKey::E32 => {
                let ElementStorageRef {
                    values: source,
                    descriptors,
                } = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                        ElementStorageRef::EMPTY
                    }
                    ElementArrayKey::E1 => {
                        e2pow1.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E2 => {
                        e2pow2.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E3 => {
                        e2pow3.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E4 => {
                        e2pow4.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E6 => {
                        e2pow6.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E8 => {
                        e2pow8.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E10 => {
                        e2pow10.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E12 => {
                        e2pow12.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E16 => {
                        e2pow16.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E24 => {
                        e2pow24.get_descriptors_and_values_raw(index.unbind(), old_len)
                    }
                    ElementArrayKey::E32 => unreachable!(),
                };
                e2pow32.push(source, descriptors.cloned())
            }
        };
        *index = new_index?;
        *cap = new_key;
        self.ensure_values_len(new_key, *index, new_len)?;
        Ok(())
    }

    pub(crate) fn reserve_keys_raw(
        &mut self,
        index: &mut PropertyKeyIndex,
        cap: &mut ElementArrayKey,
        old_len: u32,
        additional: u32,
    ) -> Result<(), TryReserveError> {
        let new_len = old_len
            .checked_add(additional)
            .expect("Ridiculous amount of keys");
        let new_cap = ElementArrayKey::from(new_len);
        if new_cap == *cap {
            return self.ensure_keys_len(*cap, *index, new_len);
        }
        let new_index = self.grow_keys_internal(*cap, *index, new_cap, old_len);
        *index = new_index?;
        *cap = new_cap;
        self.ensure_keys_len(new_cap, *index, new_len)
    }

    /// Allocates an empty elements storage that can hold `length` elements:
    /// above 2^16, only `length` slots are allocated.
    pub(crate) fn allocate_elements_with_length(
        &mut self,
        length: usize,
    ) -> Result<ElementsVector<'static>, TryReserveError> {
        let cap = ElementArrayKey::from(length);
        let elements = ElementsVector {
            elements_index: self.push_values(cap, &[], None)?,
            cap,
            len: 0,
            len_writable: true,
        };
        self.ensure_values_len(
            cap,
            elements.elements_index,
            u32::try_from(length).unwrap_or(u32::MAX),
        )?;
        Ok(elements)
    }

    /// Allocates an empty elements storage of the class `cap`. A dynamic
    /// storage (above 2^16 slots) starts with no slots: grow it with
    /// [`ElementArrays::ensure_values_len`] before use.
    pub(crate) fn allocate_elements_with_capacity(
        &mut self,
        cap: ElementArrayKey,
    ) -> Result<ElementsVector<'static>, TryReserveError> {
        let elements = ElementsVector {
            elements_index: self.push_values(cap, &[], None)?,
            cap,
            len: 0,
            len_writable: true,
        };
        Ok(elements)
    }

    /// Makes a dynamic key storage (the classes above 2^16 slots) hold at
    /// least `len` slots, the new ones free. Other storages always hold all
    /// their slots.
    #[inline]
    pub(crate) fn ensure_keys_len(
        &mut self,
        cap: ElementArrayKey,
        index: PropertyKeyIndex,
        len: u32,
    ) -> Result<(), TryReserveError> {
        match cap {
            ElementArrayKey::E24 => {
                self.k2pow24
                    .keys
                    .ensure_len(index.get_index(), len as usize, FREE_KEY_SLOT)
            }
            ElementArrayKey::E32 => {
                self.k2pow32
                    .keys
                    .ensure_len(index.get_index(), len as usize, FREE_KEY_SLOT)
            }
            _ => Ok(()),
        }
    }

    /// Makes a dynamic elements storage (the classes above 2^16 slots) hold
    /// at least `len` slots, so that the first `len` slots can be read and
    /// written. Other storages always hold all their slots.
    #[inline]
    pub(crate) fn ensure_values_len(
        &mut self,
        cap: ElementArrayKey,
        index: ElementIndex,
        len: u32,
    ) -> Result<(), TryReserveError> {
        match cap {
            ElementArrayKey::E24 => {
                self.e2pow24
                    .values
                    .ensure_len(index.get_index(), len as usize, None)
            }
            ElementArrayKey::E32 => {
                self.e2pow32
                    .values
                    .ensure_len(index.get_index(), len as usize, None)
            }
            _ => Ok(()),
        }
    }

    fn allocate_object_property_storage(
        &mut self,
        length: usize,
        values: &[Option<Value>],
        descriptors: Option<ElementDescriptorMap<'static>>,
    ) -> Result<ElementsVector<'static>, TryReserveError> {
        let cap = ElementArrayKey::from(length);
        let len = length as u32;
        let elements_index = self.push_values(cap, values, descriptors)?;
        self.ensure_values_len(cap, elements_index, len)?;
        Ok(ElementsVector {
            elements_index,
            cap,
            len,
            len_writable: true,
        })
    }

    /// Allocate an empty PropertyKey backing store with the given capacity.
    pub(crate) fn allocate_keys_with_capacity(
        &mut self,
        capacity: usize,
    ) -> Result<(ElementArrayKey, PropertyKeyIndex<'static>), TryReserveError> {
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        let key = ElementArrayKey::from(capacity);
        let index = match key {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => PropertyKeyIndex::ZERO,
            ElementArrayKey::E1 => k2pow1.push(&[])?,
            ElementArrayKey::E2 => k2pow2.push(&[])?,
            ElementArrayKey::E3 => k2pow3.push(&[])?,
            ElementArrayKey::E4 => k2pow4.push(&[])?,
            ElementArrayKey::E6 => k2pow6.push(&[])?,
            ElementArrayKey::E8 => k2pow8.push(&[])?,
            ElementArrayKey::E10 => k2pow10.push(&[])?,
            ElementArrayKey::E12 => k2pow12.push(&[])?,
            ElementArrayKey::E16 => k2pow16.push(&[])?,
            ElementArrayKey::E24 => k2pow24.push(&[])?,
            ElementArrayKey::E32 => k2pow32.push(&[])?,
        };
        self.ensure_keys_len(key, index, u32::try_from(capacity).unwrap_or(u32::MAX))?;
        Ok((key, index))
    }

    /// Allocate a new PropertyKey backing store with an added key.
    pub(crate) fn copy_keys_with_addition<'a>(
        &mut self,
        cap: ElementArrayKey,
        index: PropertyKeyIndex<'a>,
        len: u32,
        key: PropertyKey<'a>,
    ) -> Result<(ElementArrayKey, PropertyKeyIndex<'a>), TryReserveError> {
        let new_len = len.checked_add(1).expect("Ridiculous amount of keys");
        let (new_cap, new_key) = self.copy_keys_with_capacity(new_len as usize, cap, index, len)?;
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        match new_cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
            ElementArrayKey::E1 => {
                k2pow1.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E2 => {
                k2pow2.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E3 => {
                k2pow3.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E4 => {
                k2pow4.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E6 => {
                k2pow6.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E8 => {
                k2pow8.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E10 => {
                k2pow10.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E12 => {
                k2pow12.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E16 => {
                k2pow16.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E24 => {
                k2pow24.get_uninit(new_key)[len as usize] = key.unbind();
            }
            ElementArrayKey::E32 => {
                k2pow32.get_uninit(new_key)[len as usize] = key.unbind();
            }
        }
        Ok((new_cap, new_key))
    }

    /// Mutate a property key storage by removing a key at index.
    ///
    /// ## Correctness
    ///
    /// This method can only be correctly used for mutating the property key
    /// storage of intrinsic objects' Object Shapes. Using this on normal
    /// objects will cause other objects using the same Shape to perform
    /// JavaScript-wise undefined behaviour.
    ///
    /// Effectively, those objects would find that their object key-value pairs
    /// no longer match the expected values.
    pub(crate) fn remove_key(
        &mut self,
        cap: ElementArrayKey,
        index: PropertyKeyIndex,
        len: &mut u32,
        removal_index: u32,
    ) {
        if *len == 0 {
            return;
        }
        self.key_offsets.invalidate(cap, index);
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {}
            ElementArrayKey::E1 => k2pow1.remove(index, *len, removal_index),
            ElementArrayKey::E2 => k2pow2.remove(index, *len, removal_index),
            ElementArrayKey::E3 => k2pow3.remove(index, *len, removal_index),
            ElementArrayKey::E4 => k2pow4.remove(index, *len, removal_index),
            ElementArrayKey::E6 => k2pow6.remove(index, *len, removal_index),
            ElementArrayKey::E8 => k2pow8.remove(index, *len, removal_index),
            ElementArrayKey::E10 => k2pow10.remove(index, *len, removal_index),
            ElementArrayKey::E12 => k2pow12.remove(index, *len, removal_index),
            ElementArrayKey::E16 => k2pow16.remove(index, *len, removal_index),
            ElementArrayKey::E24 => k2pow24.remove(index, *len, removal_index),
            ElementArrayKey::E32 => k2pow32.remove(index, *len, removal_index),
        }
        *len -= 1;
    }

    /// Mutate a property key storage by push a key at the end.
    ///
    /// ## Correctness
    ///
    /// This method can only be correctly used for mutating the property key
    /// storage of intrinsic objects' Object Shapes. Using this on normal
    /// objects will cause other objects using the same Shape to perform
    /// JavaScript-wise undefined behaviour.
    ///
    /// Effectively, those objects would find that their object key-value pairs
    /// no longer match the expected values.
    pub(crate) fn push_key(
        &mut self,
        cap: &mut ElementArrayKey,
        index: &mut PropertyKeyIndex,
        len: &mut u32,
        key: PropertyKey,
    ) -> Result<(), TryReserveError> {
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        let new_cap = ElementArrayKey::from(len.checked_add(1).expect("Ridiculous amount of keys"));
        if new_cap.capacity() <= cap.capacity() {
            // We're within our capacity, mutate directly.
            match cap {
                ElementArrayKey::E24 => {
                    k2pow24
                        .keys
                        .ensure_len(index.get_index(), *len as usize + 1, FREE_KEY_SLOT)?
                }
                ElementArrayKey::E32 => {
                    k2pow32
                        .keys
                        .ensure_len(index.get_index(), *len as usize + 1, FREE_KEY_SLOT)?
                }
                _ => {}
            }
            match cap {
                ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
                ElementArrayKey::E1 => k2pow1.push_key(*index, *len, key),
                ElementArrayKey::E2 => k2pow2.push_key(*index, *len, key),
                ElementArrayKey::E3 => k2pow3.push_key(*index, *len, key),
                ElementArrayKey::E4 => k2pow4.push_key(*index, *len, key),
                ElementArrayKey::E6 => k2pow6.push_key(*index, *len, key),
                ElementArrayKey::E8 => k2pow8.push_key(*index, *len, key),
                ElementArrayKey::E10 => k2pow10.push_key(*index, *len, key),
                ElementArrayKey::E12 => k2pow12.push_key(*index, *len, key),
                ElementArrayKey::E16 => k2pow16.push_key(*index, *len, key),
                ElementArrayKey::E24 => k2pow24.push_key(*index, *len, key),
                ElementArrayKey::E32 => k2pow32.push_key(*index, *len, key),
            }
        } else {
            // We need to grow our backing store.
            let (new_cap, new_index) = self.copy_keys_with_addition(*cap, *index, *len, key)?;
            *cap = new_cap;
            *index = new_index.unbind();
        }
        *len += 1;
        Ok(())
    }

    pub(crate) fn copy_keys_with_removal<'a>(
        &mut self,
        cap: ElementArrayKey,
        index: PropertyKeyIndex<'a>,
        len: u32,
        removal_index: usize,
    ) -> Result<(ElementArrayKey, PropertyKeyIndex<'a>), TryReserveError> {
        if len <= 1 {
            // Removing the last key.
            debug_assert_eq!(removal_index, 0);
            return Ok((ElementArrayKey::Empty, PropertyKeyIndex::ZERO));
        }
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        let new_cap = ElementArrayKey::from(len.wrapping_sub(1));
        let new_index = if new_cap == cap {
            // No change in capacity.
            match new_cap {
                ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
                ElementArrayKey::E1 => k2pow1.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E2 => k2pow2.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E3 => k2pow3.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E4 => k2pow4.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E6 => k2pow6.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E8 => k2pow8.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E10 => k2pow10.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E12 => k2pow12.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E16 => k2pow16.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E24 => k2pow24.push_within_with_removal(index, len, removal_index),
                ElementArrayKey::E32 => k2pow32.push_within_with_removal(index, len, removal_index),
            }
        } else {
            // Change in capacity.
            match new_cap {
                ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
                ElementArrayKey::E1 => {
                    let source = match cap {
                        ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                        ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                        ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                        ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                        ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                        ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                        ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                        ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow1.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E2 => {
                    let source = match cap {
                        ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                        ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                        ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                        ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                        ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                        ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                        ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow2.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E3 => {
                    let source = match cap {
                        ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                        ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                        ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                        ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                        ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                        ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow3.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E4 => {
                    let source = match cap {
                        ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                        ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                        ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                        ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                        ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow4.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E6 => {
                    let source = match cap {
                        ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                        ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                        ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                        ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow6.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E8 => {
                    let source = match cap {
                        ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                        ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                        ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow8.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E10 => {
                    let source = match cap {
                        ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                        ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow10.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E12 => {
                    let source = match cap {
                        ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow12.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E16 => {
                    let source = match cap {
                        ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow16.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E24 => {
                    let source = match cap {
                        ElementArrayKey::E32 => k2pow32.get_raw(index, len),
                        _ => unreachable!(),
                    };
                    k2pow24.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E32 => unreachable!(),
            }
        };
        Ok((new_cap, new_index?))
    }

    /// Allocate a new PropertyKey backing store with the given capacity,
    /// copying the first len values into the new allocation from the source
    /// backing store.
    pub(crate) fn copy_keys_with_capacity<'a>(
        &mut self,
        capacity: usize,
        cap: ElementArrayKey,
        index: PropertyKeyIndex<'a>,
        len: u32,
    ) -> Result<(ElementArrayKey, PropertyKeyIndex<'a>), TryReserveError> {
        if capacity == 0 {
            return Ok((ElementArrayKey::Empty, PropertyKeyIndex::ZERO));
        }
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        let new_cap = ElementArrayKey::from(capacity);
        let new_index = if new_cap == cap {
            // No change in capacity.
            match new_cap {
                ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
                ElementArrayKey::E1 => k2pow1.push_within(index, len),
                ElementArrayKey::E2 => k2pow2.push_within(index, len),
                ElementArrayKey::E3 => k2pow3.push_within(index, len),
                ElementArrayKey::E4 => k2pow4.push_within(index, len),
                ElementArrayKey::E6 => k2pow6.push_within(index, len),
                ElementArrayKey::E8 => k2pow8.push_within(index, len),
                ElementArrayKey::E10 => k2pow10.push_within(index, len),
                ElementArrayKey::E12 => k2pow12.push_within(index, len),
                ElementArrayKey::E16 => k2pow16.push_within(index, len),
                ElementArrayKey::E24 => k2pow24.push_within(index, len),
                ElementArrayKey::E32 => k2pow32.push_within(index, len),
            }
        } else if new_cap.capacity() < cap.capacity() {
            // A shape shares the storage of a descendant that filled more of
            // it than the shape's own keys need: copy into a smaller one.
            self.shrink_keys_internal(cap, index, new_cap, len)
        } else {
            // Change in capacity.
            self.grow_keys_internal(cap, index, new_cap, len)
        };
        let new_index = new_index?;
        self.ensure_keys_len(
            new_cap,
            new_index,
            u32::try_from(capacity).unwrap_or(u32::MAX),
        )?;
        Ok((new_cap, new_index))
    }

    /// Copy the first `len` keys of a storage into a new storage of a smaller
    /// capacity that holds them.
    fn shrink_keys_internal<'a>(
        &mut self,
        cap: ElementArrayKey,
        index: PropertyKeyIndex<'a>,
        new_cap: ElementArrayKey,
        len: u32,
    ) -> Result<PropertyKeyIndex<'a>, TryReserveError> {
        debug_assert!(len <= new_cap.capacity());
        let mut keys: Vec<PropertyKey<'a>> = Vec::new();
        keys.try_reserve_exact(len as usize)?;
        keys.extend_from_slice(self.get_keys_raw(cap, index, len));
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        match new_cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => Ok(PropertyKeyIndex::ZERO),
            ElementArrayKey::E1 => k2pow1.push(&keys),
            ElementArrayKey::E2 => k2pow2.push(&keys),
            ElementArrayKey::E3 => k2pow3.push(&keys),
            ElementArrayKey::E4 => k2pow4.push(&keys),
            ElementArrayKey::E6 => k2pow6.push(&keys),
            ElementArrayKey::E8 => k2pow8.push(&keys),
            ElementArrayKey::E10 => k2pow10.push(&keys),
            ElementArrayKey::E12 => k2pow12.push(&keys),
            ElementArrayKey::E16 => k2pow16.push(&keys),
            ElementArrayKey::E24 => k2pow24.push(&keys),
            ElementArrayKey::E32 => k2pow32.push(&keys),
        }
    }

    pub(crate) fn realloc_values_with_removal<'a>(
        &mut self,
        src_cap: ElementArrayKey,
        src_index: ElementIndex<'a>,
        dst_cap: ElementArrayKey,
        len: u32,
        removal_index: u32,
    ) -> Result<ElementIndex<'a>, TryReserveError> {
        if dst_cap.capacity() == 0 {
            // Removing the last key.
            debug_assert_eq!(removal_index, 0);
            return Ok(ElementIndex::ZERO);
        }

        if dst_cap == src_cap {
            // No change in capacity.
            panic!("Should not request realloc with same capacity");
        } else {
            // Change in capacity.
            let ElementArrays {
                e2pow1,
                e2pow2,
                e2pow3,
                e2pow4,
                e2pow6,
                e2pow8,
                e2pow10,
                e2pow12,
                e2pow16,
                e2pow24,
                e2pow32,
                ..
            } = self;
            match dst_cap {
                ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
                ElementArrayKey::E1 => {
                    let source = match src_cap {
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow1.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E2 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow2.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E3 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow3.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E4 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow4.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E6 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow6.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E8 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow8.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E10 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow10.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E12 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow12.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E16 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E24 => {
                            e2pow24.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow16.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E24 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E32 => {
                            e2pow32.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow24.push_with_removal(source, removal_index)
                }
                ElementArrayKey::E32 => {
                    let source = match src_cap {
                        ElementArrayKey::E1 => {
                            e2pow1.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E2 => {
                            e2pow2.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E3 => {
                            e2pow3.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E4 => {
                            e2pow4.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E6 => {
                            e2pow6.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E8 => {
                            e2pow8.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E10 => {
                            e2pow10.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E12 => {
                            e2pow12.get_descriptors_and_values_raw(src_index, len)
                        }
                        ElementArrayKey::E16 => {
                            e2pow16.get_descriptors_and_values_raw(src_index, len)
                        }
                        _ => unreachable!(),
                    };
                    e2pow32.push_with_removal(source, removal_index)
                }
            }
        }
    }

    /// Grow a keys storage to new capacity.
    fn grow_keys_internal<'a>(
        &mut self,
        cap: ElementArrayKey,
        index: PropertyKeyIndex,
        new_cap: ElementArrayKey,
        len: u32,
    ) -> Result<PropertyKeyIndex<'a>, TryReserveError> {
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        match new_cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
            ElementArrayKey::E1 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    _ => unreachable!(),
                };
                k2pow1.push(source)
            }
            ElementArrayKey::E2 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow2.push(source)
            }
            ElementArrayKey::E3 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow3.push(source)
            }
            ElementArrayKey::E4 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow4.push(source)
            }
            ElementArrayKey::E6 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                    ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow6.push(source)
            }
            ElementArrayKey::E8 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                    ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                    ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow8.push(source)
            }
            ElementArrayKey::E10 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                    ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                    ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                    ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow10.push(source)
            }
            ElementArrayKey::E12 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                    ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                    ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                    ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                    ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow12.push(source)
            }
            ElementArrayKey::E16 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                    ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                    ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                    ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                    ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                    ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow16.push(source)
            }
            ElementArrayKey::E24 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                    ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                    ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                    ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                    ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                    ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                    ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow24.push(source)
            }
            ElementArrayKey::E32 => {
                let source = match cap {
                    ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
                    ElementArrayKey::E1 => k2pow1.get_raw(index, len),
                    ElementArrayKey::E2 => k2pow2.get_raw(index, len),
                    ElementArrayKey::E3 => k2pow3.get_raw(index, len),
                    ElementArrayKey::E4 => k2pow4.get_raw(index, len),
                    ElementArrayKey::E6 => k2pow6.get_raw(index, len),
                    ElementArrayKey::E8 => k2pow8.get_raw(index, len),
                    ElementArrayKey::E10 => k2pow10.get_raw(index, len),
                    ElementArrayKey::E12 => k2pow12.get_raw(index, len),
                    ElementArrayKey::E16 => k2pow16.get_raw(index, len),
                    ElementArrayKey::E24 => k2pow24.get_raw(index, len),
                    _ => unreachable!(),
                };
                k2pow32.push(source)
            }
        }
    }

    pub(crate) fn allocate_object_property_storage_from_entries_vec<'a>(
        &mut self,
        mut entries: Vec<(
            PropertyKey<'a>,
            Option<ElementDescriptor>,
            Option<Value<'a>>,
        )>,
    ) -> Result<ElementsVector<'a>, TryReserveError> {
        let length = entries.len();
        let mut values: Vec<Option<Value>> = Vec::with_capacity(length);
        let mut descriptors: Option<ElementDescriptorMap<'static>> = None;
        entries.drain(..).enumerate().for_each(|(index, entry)| {
            let (_, maybe_descriptor, maybe_value) = entry;
            values.push(maybe_value);
            if let Some(descriptor) = maybe_descriptor {
                if descriptors.is_none() {
                    descriptors = Some(Default::default());
                }
                descriptors
                    .as_mut()
                    .unwrap()
                    .insert(index as u32, descriptor.unbind());
            }
        });
        self.allocate_object_property_storage(length, &values, descriptors)
    }

    pub(crate) fn allocate_object_property_storage_from_entries_slice<'a>(
        &mut self,
        entries: &[ObjectEntry<'a>],
    ) -> Result<ElementsVector<'a>, TryReserveError> {
        let length = entries.len();
        // The values of a few entries are gathered on the stack: an object
        // made by a built-in (the result of an iterator's `next`, a
        // property descriptor) allocated and freed two vectors here.
        let mut few: [Option<Value>; 8] = [None; 8];
        let mut many: Vec<Option<Value>> = Vec::new();
        let values: &mut [Option<Value>] = if length <= few.len() {
            &mut few[..length]
        } else {
            many.resize(length, None);
            &mut many
        };
        let mut descriptors: Option<ElementDescriptorMap<'static>> = None;
        for (index, entry) in entries.iter().enumerate() {
            let ObjectEntry { key: _, value } = entry;
            let (maybe_descriptor, maybe_value) =
                ElementDescriptor::from_object_entry_property_descriptor(value);
            values[index] = maybe_value;
            if let Some(descriptor) = maybe_descriptor {
                if descriptors.is_none() {
                    descriptors = Some(Default::default());
                }
                descriptors
                    .as_mut()
                    .unwrap()
                    .insert(index as u32, descriptor.unbind());
            }
        }
        self.allocate_object_property_storage(length, values, descriptors)
    }

    pub(crate) fn get_keys_raw<'a>(
        &self,
        cap: ElementArrayKey,
        index: PropertyKeyIndex<'a>,
        len: u32,
    ) -> &[PropertyKey<'a>] {
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
            ElementArrayKey::E1 => k2pow1.get_raw(index, len),
            ElementArrayKey::E2 => k2pow2.get_raw(index, len),
            ElementArrayKey::E3 => k2pow3.get_raw(index, len),
            ElementArrayKey::E4 => k2pow4.get_raw(index, len),
            ElementArrayKey::E6 => k2pow6.get_raw(index, len),
            ElementArrayKey::E8 => k2pow8.get_raw(index, len),
            ElementArrayKey::E10 => k2pow10.get_raw(index, len),
            ElementArrayKey::E12 => k2pow12.get_raw(index, len),
            ElementArrayKey::E16 => k2pow16.get_raw(index, len),
            ElementArrayKey::E24 => k2pow24.get_raw(index, len),
            ElementArrayKey::E32 => k2pow32.get_raw(index, len),
        }
    }

    /// Get the currently reserved key storage as a mutable slice.
    /// [`FREE_KEY_SLOT`] values may appear at the end of the storage and stand
    /// for unset key/value slots in the object.
    pub(crate) fn get_keys_uninit_raw(
        &mut self,
        cap: ElementArrayKey,
        keys_index: PropertyKeyIndex,
    ) -> &mut [PropertyKey<'static>] {
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &mut [],
            ElementArrayKey::E1 => k2pow1.get_uninit(keys_index),
            ElementArrayKey::E2 => k2pow2.get_uninit(keys_index),
            ElementArrayKey::E3 => k2pow3.get_uninit(keys_index),
            ElementArrayKey::E4 => k2pow4.get_uninit(keys_index),
            ElementArrayKey::E6 => k2pow6.get_uninit(keys_index),
            ElementArrayKey::E8 => k2pow8.get_uninit(keys_index),
            ElementArrayKey::E10 => k2pow10.get_uninit(keys_index),
            ElementArrayKey::E12 => k2pow12.get_uninit(keys_index),
            ElementArrayKey::E16 => k2pow16.get_uninit(keys_index),
            ElementArrayKey::E24 => k2pow24.get_uninit(keys_index),
            ElementArrayKey::E32 => k2pow32.get_uninit(keys_index),
        }
    }

    pub(crate) fn get_values<'a>(&self, vector: &ElementsVector) -> &[Option<Value<'a>>] {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        match vector.cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
            ElementArrayKey::E1 => e2pow1.get_values(vector),
            ElementArrayKey::E2 => e2pow2.get_values(vector),
            ElementArrayKey::E3 => e2pow3.get_values(vector),
            ElementArrayKey::E4 => e2pow4.get_values(vector),
            ElementArrayKey::E6 => e2pow6.get_values(vector),
            ElementArrayKey::E8 => e2pow8.get_values(vector),
            ElementArrayKey::E10 => e2pow10.get_values(vector),
            ElementArrayKey::E12 => e2pow12.get_values(vector),
            ElementArrayKey::E16 => e2pow16.get_values(vector),
            ElementArrayKey::E24 => e2pow24.get_values(vector),
            ElementArrayKey::E32 => e2pow32.get_values(vector),
        }
    }

    pub(crate) fn get_values_mut(
        &mut self,
        vector: &ElementsVector,
    ) -> &mut [Option<Value<'static>>] {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        match vector.cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &mut [],
            ElementArrayKey::E1 => e2pow1.get_values_mut(vector),
            ElementArrayKey::E2 => e2pow2.get_values_mut(vector),
            ElementArrayKey::E3 => e2pow3.get_values_mut(vector),
            ElementArrayKey::E4 => e2pow4.get_values_mut(vector),
            ElementArrayKey::E6 => e2pow6.get_values_mut(vector),
            ElementArrayKey::E8 => e2pow8.get_values_mut(vector),
            ElementArrayKey::E10 => e2pow10.get_values_mut(vector),
            ElementArrayKey::E12 => e2pow12.get_values_mut(vector),
            ElementArrayKey::E16 => e2pow16.get_values_mut(vector),
            ElementArrayKey::E24 => e2pow24.get_values_mut(vector),
            ElementArrayKey::E32 => e2pow32.get_values_mut(vector),
        }
    }

    /// Copy the values in `source_range` of the `source` elements into the
    /// start of the `destination` elements.
    ///
    /// The two may live in the same or in different backing stores; the copy
    /// borrows each backing store once, as a flat slice.
    pub(crate) fn copy_values_between(
        &mut self,
        source: &ElementsVector,
        source_range: core::ops::Range<usize>,
        destination: &ElementsVector,
    ) {
        let count = source_range.len();
        if count == 0 {
            return;
        }
        assert!(source_range.end <= source.len() as usize);
        assert!(count <= destination.len() as usize);
        let dynamic =
            |cap: ElementArrayKey| matches!(cap, ElementArrayKey::E24 | ElementArrayKey::E32);
        if dynamic(source.cap) || dynamic(destination.cap) {
            // Dynamic storages have no flat slice: copy through a buffer.
            let buffer = self.get_values(source)[source_range].to_vec();
            self.get_values_mut(destination)[..count].copy_from_slice(&buffer);
            return;
        }
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        let mut stores: [Option<(&mut [Option<Value<'static>>], usize)>; 11] = [
            e2pow1.as_flat_mut(),
            e2pow2.as_flat_mut(),
            e2pow3.as_flat_mut(),
            e2pow4.as_flat_mut(),
            e2pow6.as_flat_mut(),
            e2pow8.as_flat_mut(),
            e2pow10.as_flat_mut(),
            e2pow12.as_flat_mut(),
            e2pow16.as_flat_mut(),
            e2pow24.as_flat_mut(),
            e2pow32.as_flat_mut(),
        ];
        let store_index = |cap: ElementArrayKey| match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => {
                unreachable!("non-empty elements have a backing store")
            }
            ElementArrayKey::E1 => 0,
            ElementArrayKey::E2 => 1,
            ElementArrayKey::E3 => 2,
            ElementArrayKey::E4 => 3,
            ElementArrayKey::E6 => 4,
            ElementArrayKey::E8 => 5,
            ElementArrayKey::E10 => 6,
            ElementArrayKey::E12 => 7,
            ElementArrayKey::E16 => 8,
            ElementArrayKey::E24 => 9,
            ElementArrayKey::E32 => 10,
        };
        let source_store = store_index(source.cap);
        let destination_store = store_index(destination.cap);
        let (source_values, source_stride) = stores[source_store]
            .take()
            .expect("backing store taken once");
        let source_start = source.elements_index.get_index() * source_stride + source_range.start;
        if source_store == destination_store {
            let destination_start = destination.elements_index.get_index() * source_stride;
            source_values.copy_within(source_start..source_start + count, destination_start);
            return;
        }
        let (destination_values, destination_stride) = stores[destination_store]
            .take()
            .expect("backing store taken once");
        let destination_start = destination.elements_index.get_index() * destination_stride;
        destination_values[destination_start..destination_start + count]
            .copy_from_slice(&source_values[source_start..source_start + count]);
    }

    /// Get shared access to the elements storage of an object or an array.
    ///
    /// Value slots with None are either array holes, or accessor properties.
    /// For accessor properties a descriptor for the same index exists.
    #[inline]
    pub(crate) fn get_element_storage(
        &self,
        vector: &ElementsVector,
    ) -> ElementStorageRef<'_, 'static> {
        self.get_element_storage_raw(vector.elements_index.unbind(), vector.cap, vector.len())
    }

    /// Get shared access to the elements storage of an object or an array.
    ///
    /// Value slots with None are either array holes, or accessor properties.
    /// For accessor properties a descriptor for the same index exists.
    pub(crate) fn get_element_storage_raw<'gc>(
        &self,
        index: ElementIndex<'gc>,
        cap: ElementArrayKey,
        len: u32,
    ) -> ElementStorageRef<'_, 'gc> {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => ElementStorageRef::EMPTY,
            ElementArrayKey::E1 => e2pow1.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E2 => e2pow2.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E3 => e2pow3.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E4 => e2pow4.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E6 => e2pow6.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E8 => e2pow8.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E10 => e2pow10.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E12 => e2pow12.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E16 => e2pow16.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E24 => e2pow24.get_descriptors_and_values_raw(index, len),
            ElementArrayKey::E32 => e2pow32.get_descriptors_and_values_raw(index, len),
        }
    }

    /// Get shared access to the value slots of an elements storage, without
    /// looking up its descriptors.
    ///
    /// Value slots with None are either array holes, or accessor properties.
    pub(crate) fn get_element_values_raw(
        &self,
        index: ElementIndex,
        cap: ElementArrayKey,
        len: u32,
    ) -> &[Option<Value<'static>>] {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &[],
            ElementArrayKey::E1 => e2pow1.get_values_raw(index, len),
            ElementArrayKey::E2 => e2pow2.get_values_raw(index, len),
            ElementArrayKey::E3 => e2pow3.get_values_raw(index, len),
            ElementArrayKey::E4 => e2pow4.get_values_raw(index, len),
            ElementArrayKey::E6 => e2pow6.get_values_raw(index, len),
            ElementArrayKey::E8 => e2pow8.get_values_raw(index, len),
            ElementArrayKey::E10 => e2pow10.get_values_raw(index, len),
            ElementArrayKey::E12 => e2pow12.get_values_raw(index, len),
            ElementArrayKey::E16 => e2pow16.get_values_raw(index, len),
            ElementArrayKey::E24 => e2pow24.get_values_raw(index, len),
            ElementArrayKey::E32 => e2pow32.get_values_raw(index, len),
        }
    }

    /// Get exclusive access to the value slots of an elements storage,
    /// without looking up its descriptors.
    ///
    /// Value slots with None are either array holes, or accessor properties.
    pub(crate) fn get_element_values_mut_raw(
        &mut self,
        index: ElementIndex,
        cap: ElementArrayKey,
        len: u32,
    ) -> &mut [Option<Value<'static>>] {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => &mut [],
            ElementArrayKey::E1 => e2pow1.get_values_mut_raw(index, len),
            ElementArrayKey::E2 => e2pow2.get_values_mut_raw(index, len),
            ElementArrayKey::E3 => e2pow3.get_values_mut_raw(index, len),
            ElementArrayKey::E4 => e2pow4.get_values_mut_raw(index, len),
            ElementArrayKey::E6 => e2pow6.get_values_mut_raw(index, len),
            ElementArrayKey::E8 => e2pow8.get_values_mut_raw(index, len),
            ElementArrayKey::E10 => e2pow10.get_values_mut_raw(index, len),
            ElementArrayKey::E12 => e2pow12.get_values_mut_raw(index, len),
            ElementArrayKey::E16 => e2pow16.get_values_mut_raw(index, len),
            ElementArrayKey::E24 => e2pow24.get_values_mut_raw(index, len),
            ElementArrayKey::E32 => e2pow32.get_values_mut_raw(index, len),
        }
    }

    /// Get exclusive access to the elements storage of an object.
    ///
    /// Value slots with None are either array holes, or accessor properties.
    /// For accessor properties a descriptor for the same index exists.
    pub(crate) fn get_element_storage_mut_raw(
        &mut self,
        index: ElementIndex,
        cap: ElementArrayKey,
        len: u32,
    ) -> ElementStorageMut<'_> {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        let index = index.unbind();
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
            ElementArrayKey::E1 => e2pow1.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E2 => e2pow2.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E3 => e2pow3.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E4 => e2pow4.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E6 => e2pow6.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E8 => e2pow8.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E10 => e2pow10.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E12 => e2pow12.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E16 => e2pow16.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E24 => e2pow24.get_descriptors_and_values_mut_raw(index, len),
            ElementArrayKey::E32 => e2pow32.get_descriptors_and_values_mut_raw(index, len),
        }
    }

    /// Get exclusive access to the elements storage of an object or an array.
    ///
    /// Value slots with None are either array holes, or accessor properties.
    /// For accessor properties a descriptor for the same index exists.
    pub(crate) fn get_element_storage_mut(
        &mut self,
        vector: &ElementsVector,
    ) -> ElementStorageMut<'_> {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        match vector.cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
            ElementArrayKey::E1 => e2pow1.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E2 => e2pow2.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E3 => e2pow3.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E4 => e2pow4.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E6 => e2pow6.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E8 => e2pow8.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E10 => e2pow10.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E12 => e2pow12.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E16 => e2pow16.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E24 => e2pow24.get_descriptors_and_values_mut(vector),
            ElementArrayKey::E32 => e2pow32.get_descriptors_and_values_mut(vector),
        }
    }

    /// Get exclusive access to the full elements storage of an object or an
    /// array, including uninitialised elements.
    ///
    /// Value slots with None are either uninitialised, array holes, or
    /// accessor properties. For accessor properties a descriptor for the same
    /// index exists.
    pub(crate) fn get_element_storage_uninit_raw(
        &mut self,
        index: ElementIndex,
        cap: ElementArrayKey,
    ) -> ElementStorageUninit<'_> {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        let index = index.unbind();
        match cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => unreachable!(),
            ElementArrayKey::E1 => e2pow1.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E2 => e2pow2.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E3 => e2pow3.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E4 => e2pow4.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E6 => e2pow6.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E8 => e2pow8.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E10 => e2pow10.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E12 => e2pow12.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E16 => e2pow16.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E24 => e2pow24.get_descriptors_and_values_uninit_raw(index),
            ElementArrayKey::E32 => e2pow32.get_descriptors_and_values_uninit_raw(index),
        }
    }

    /// Get the currently allocated keys, values, and descriptors as mutable.
    /// If no storage exists, None is returned.
    ///
    /// Note: keys are not given out as mutable, since mutating keys is never
    /// correct.
    pub(crate) fn get_property_storage_mut_raw<'gc>(
        &mut self,
        keys_index: PropertyKeyIndex<'gc>,
        keys_cap: ElementArrayKey,
        values_index: ElementIndex<'gc>,
        values_cap: ElementArrayKey,
        len: u32,
    ) -> Option<PropertyStorageMut<'_, 'gc>> {
        let Self {
            k2pow1,
            k2pow2,
            k2pow3,
            k2pow4,
            k2pow6,
            k2pow8,
            k2pow10,
            k2pow12,
            k2pow16,
            k2pow24,
            k2pow32,
            ..
        } = self;
        let keys = match keys_cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => return None,
            ElementArrayKey::E1 => k2pow1.get_raw(keys_index, len),
            ElementArrayKey::E2 => k2pow2.get_raw(keys_index, len),
            ElementArrayKey::E3 => k2pow3.get_raw(keys_index, len),
            ElementArrayKey::E4 => k2pow4.get_raw(keys_index, len),
            ElementArrayKey::E6 => k2pow6.get_raw(keys_index, len),
            ElementArrayKey::E8 => k2pow8.get_raw(keys_index, len),
            ElementArrayKey::E10 => k2pow10.get_raw(keys_index, len),
            ElementArrayKey::E12 => k2pow12.get_raw(keys_index, len),
            ElementArrayKey::E16 => k2pow16.get_raw(keys_index, len),
            ElementArrayKey::E24 => k2pow24.get_raw(keys_index, len),
            ElementArrayKey::E32 => k2pow32.get_raw(keys_index, len),
        };
        let elements = match values_cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => return None,
            ElementArrayKey::E1 => self
                .e2pow1
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E2 => self
                .e2pow2
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E3 => self
                .e2pow3
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E4 => self
                .e2pow4
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E6 => self
                .e2pow6
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E8 => self
                .e2pow8
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E10 => self
                .e2pow10
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E12 => self
                .e2pow12
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E16 => self
                .e2pow16
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E24 => self
                .e2pow24
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
            ElementArrayKey::E32 => self
                .e2pow32
                .get_descriptors_and_values_mut_raw(values_index.unbind(), len),
        };
        Some(PropertyStorageMut::from_keys_and_elements(keys, elements))
    }

    /// It does not do anything with descriptors and assumes there is a previous validation in place.
    pub(crate) fn shallow_clone<'a>(
        &mut self,
        elements_vector: &ElementsVector<'a>,
    ) -> ElementsVector<'a> {
        let ElementArrays {
            e2pow1,
            e2pow2,
            e2pow3,
            e2pow4,
            e2pow6,
            e2pow8,
            e2pow10,
            e2pow12,
            e2pow16,
            e2pow24,
            e2pow32,
            ..
        } = self;
        let new_index = match elements_vector.cap {
            ElementArrayKey::Empty | ElementArrayKey::EmptyIntrinsic => ElementIndex::ZERO,
            ElementArrayKey::E1 => e2pow1.push_copy(elements_vector.elements_index),
            ElementArrayKey::E2 => e2pow2.push_copy(elements_vector.elements_index),
            ElementArrayKey::E3 => e2pow3.push_copy(elements_vector.elements_index),
            ElementArrayKey::E4 => e2pow4.push_copy(elements_vector.elements_index),
            ElementArrayKey::E6 => e2pow6.push_copy(elements_vector.elements_index),
            ElementArrayKey::E8 => e2pow8.push_copy(elements_vector.elements_index),
            ElementArrayKey::E10 => e2pow10.push_copy(elements_vector.elements_index),
            ElementArrayKey::E12 => e2pow12.push_copy(elements_vector.elements_index),
            ElementArrayKey::E16 => e2pow16.push_copy(elements_vector.elements_index),
            ElementArrayKey::E24 => e2pow24.push_copy(elements_vector.elements_index),
            ElementArrayKey::E32 => e2pow32.push_copy(elements_vector.elements_index),
        };

        ElementsVector {
            cap: elements_vector.cap,
            elements_index: new_index,
            len: elements_vector.len(),
            len_writable: true,
        }
    }
}

impl HeapMarkAndSweep for ElementDescriptor<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        match self {
            ElementDescriptor::WritableEnumerableConfigurableData
            | ElementDescriptor::WritableEnumerableUnconfigurableData
            | ElementDescriptor::WritableUnenumerableConfigurableData
            | ElementDescriptor::WritableUnenumerableUnconfigurableData
            | ElementDescriptor::ReadOnlyEnumerableConfigurableData
            | ElementDescriptor::ReadOnlyEnumerableUnconfigurableData
            | ElementDescriptor::ReadOnlyUnenumerableConfigurableData
            | ElementDescriptor::ReadOnlyUnenumerableUnconfigurableData
            | ElementDescriptor::UndefinedEnumerableConfigurableAccessor
            | ElementDescriptor::UndefinedEnumerableUnconfigurableAccessor
            | ElementDescriptor::UndefinedUnenumerableConfigurableAccessor
            | ElementDescriptor::UndefinedUnenumerableUnconfigurableAccessor => {}
            ElementDescriptor::ReadOnlyEnumerableConfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyEnumerableUnconfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyUnenumerableConfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyUnenumerableUnconfigurableAccessor { get } => {
                get.mark_values(queues)
            }
            ElementDescriptor::WriteOnlyEnumerableConfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyEnumerableUnconfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyUnenumerableConfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyUnenumerableUnconfigurableAccessor { set } => {
                set.mark_values(queues)
            }
            ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { get, set }
            | ElementDescriptor::ReadWriteEnumerableUnconfigurableAccessor { get, set }
            | ElementDescriptor::ReadWriteUnenumerableConfigurableAccessor { get, set }
            | ElementDescriptor::ReadWriteUnenumerableUnconfigurableAccessor { get, set } => {
                get.mark_values(queues);
                set.mark_values(queues);
            }
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        match self {
            ElementDescriptor::WritableEnumerableConfigurableData
            | ElementDescriptor::WritableEnumerableUnconfigurableData
            | ElementDescriptor::WritableUnenumerableConfigurableData
            | ElementDescriptor::WritableUnenumerableUnconfigurableData
            | ElementDescriptor::ReadOnlyEnumerableConfigurableData
            | ElementDescriptor::ReadOnlyEnumerableUnconfigurableData
            | ElementDescriptor::ReadOnlyUnenumerableConfigurableData
            | ElementDescriptor::ReadOnlyUnenumerableUnconfigurableData
            | ElementDescriptor::UndefinedEnumerableConfigurableAccessor
            | ElementDescriptor::UndefinedEnumerableUnconfigurableAccessor
            | ElementDescriptor::UndefinedUnenumerableConfigurableAccessor
            | ElementDescriptor::UndefinedUnenumerableUnconfigurableAccessor => {}
            ElementDescriptor::ReadOnlyEnumerableConfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyEnumerableUnconfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyUnenumerableConfigurableAccessor { get }
            | ElementDescriptor::ReadOnlyUnenumerableUnconfigurableAccessor { get } => {
                get.sweep_values(compactions)
            }
            ElementDescriptor::WriteOnlyEnumerableConfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyEnumerableUnconfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyUnenumerableConfigurableAccessor { set }
            | ElementDescriptor::WriteOnlyUnenumerableUnconfigurableAccessor { set } => {
                set.sweep_values(compactions)
            }
            ElementDescriptor::ReadWriteEnumerableConfigurableAccessor { get, set }
            | ElementDescriptor::ReadWriteEnumerableUnconfigurableAccessor { get, set }
            | ElementDescriptor::ReadWriteUnenumerableConfigurableAccessor { get, set }
            | ElementDescriptor::ReadWriteUnenumerableUnconfigurableAccessor { get, set } => {
                get.sweep_values(compactions);
                set.sweep_values(compactions);
            }
        }
    }
}

#[doc(hidden)]
impl AsRef<ElementArrays> for Agent {
    fn as_ref(&self) -> &ElementArrays {
        &self.heap.elements
    }
}

#[doc(hidden)]
impl AsMut<ElementArrays> for Agent {
    fn as_mut(&mut self) -> &mut ElementArrays {
        &mut self.heap.elements
    }
}

impl AsRef<ElementArrays> for ElementArrays {
    fn as_ref(&self) -> &ElementArrays {
        self
    }
}

impl AsMut<ElementArrays> for ElementArrays {
    fn as_mut(&mut self) -> &mut ElementArrays {
        self
    }
}
