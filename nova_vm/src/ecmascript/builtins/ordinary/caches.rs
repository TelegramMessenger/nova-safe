// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::{marker::PhantomData, num::NonZeroU32};

use crate::table::{HashTable, hash_table::Entry};

use crate::{
    ecmascript::{Agent, InternalMethods, Object, PropertyKey, TryResult, Value},
    engine::{Bindable, GcToken, HeapRootData, NoGcScope},
    heap::{
        AtomicBits, BitRange, CompactionLists, ElementArrayKey, HeapMarkAndSweep,
        HeapSweepWeakReference, PropertyKeyHeap, WeakReference, WorkQueues,
        sweep_heap_vector_values,
    },
};

use super::ObjectShape;

/// Heap structure holding all property lookup caches, cache-caches, and other
/// related features.
#[derive(Debug)]
pub(crate) struct Caches<'a> {
    property_lookup_cache_lookup_table:
        HashTable<(PropertyKey<'a>, WeakReference<PropertyLookupCache<'a>>)>,
    property_lookup_caches: Vec<PropertyLookupCacheRecord<'a>>,
    property_lookup_cache_prototypes: Vec<PropertyLookupCacheRecordPrototypes<'a>>,
    // property_lookup_cache_stack: Vec<PropertyLookupCache<'a>>,
    current_cache_to_populate: Option<CacheToPopulate<'a>>,
    /// The caches that [`PropertyLookupCache::new`] gave last, each with
    /// its key, in one of the two places of the key's identity. Native code
    /// asks for the cache of one name again and again (the method of an
    /// operator, "prototype" at every `new`, an Array's iterator method),
    /// and finds it here by the key itself, with no hash of the name's
    /// text. Not a root, and emptied by every collection: a collection
    /// moves keys and caches, and drops the caches nothing else holds.
    recent: [Option<(PropertyKey<'a>, PropertyLookupCache<'a>)>; RECENT_CACHES],
    /// The Object Shape of an arguments object of so many arguments, with
    /// the %Object.prototype% it is the shape under. An arguments object is
    /// made at every call of a function that uses one, and its shape was
    /// found by one transition for each of its properties, every time. Not
    /// a root, and emptied by every collection, as `recent` is.
    arguments_shapes: [Option<(Object<'a>, ObjectShape<'a>)>; ARGUMENTS_SHAPES],
}

/// How many caches [`Caches::recent`] has places for.
const RECENT_CACHES: usize = 64;

/// Arguments objects of fewer arguments than this have their shape kept
/// ([`Caches::arguments_shape`]).
const ARGUMENTS_SHAPES: usize = 9;

/// The first of the two places of `key` in [`Caches::recent`]; the other
/// is the one beside it.
#[inline]
fn recent_place(key: PropertyKey) -> usize {
    (key.identity_bits().wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 58) as usize
}

#[derive(Debug)]
pub(crate) struct CacheToPopulate<'a> {
    pub(crate) receiver: Value<'a>,
    pub(crate) cache: PropertyLookupCache<'a>,
    pub(crate) key: PropertyKey<'a>,
    pub(crate) shape: ObjectShape<'a>,
}

impl<'a> Caches<'a> {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            property_lookup_cache_lookup_table: HashTable::with_capacity(capacity),
            property_lookup_caches: Vec::with_capacity(capacity),
            property_lookup_cache_prototypes: Vec::with_capacity(capacity),
            // property_lookup_cache_stack: Vec::with_capacity(64),
            current_cache_to_populate: None,
            recent: [None; RECENT_CACHES],
            arguments_shapes: [None; ARGUMENTS_SHAPES],
        }
    }

    /// The Object Shape of an arguments object of `len` arguments under
    /// `prototype`, if one was made since the last collection.
    #[inline]
    pub(crate) fn arguments_shape(&self, prototype: Object, len: usize) -> Option<ObjectShape<'a>> {
        match self.arguments_shapes.get(len) {
            Some(Some((kept, shape))) if *kept == prototype.unbind() => Some(*shape),
            _ => None,
        }
    }

    /// Keeps `shape` for [`Caches::arguments_shape`].
    #[inline]
    pub(crate) fn set_arguments_shape(
        &mut self,
        prototype: Object,
        len: usize,
        shape: ObjectShape,
    ) {
        if let Some(place) = self.arguments_shapes.get_mut(len) {
            *place = Some((prototype.unbind(), shape.unbind()));
        }
    }

    /// Every property lookup cache that `PropertyLookupCache::new` handed
    /// out, with its property key.
    pub(crate) fn property_lookup_cache_keys(
        &self,
    ) -> impl Iterator<Item = (PropertyLookupCache<'a>, PropertyKey<'a>)> + '_ {
        self.property_lookup_cache_lookup_table
            .iter()
            .map(|(key, cache)| (cache.0, *key))
    }

    /// Invalidate property lookup caches on intrinsic shape property addition.
    ///
    /// When such a additions happens, it means that:
    /// * Caches which have failed to find such the key on the intrinsic object
    ///   itself or on any shape that uses the intrinsic object as a prototype
    ///   must be invalidated.
    /// * Caches which have found a found such the key in the intrinsic
    ///   object's prototype chain and pass through the intrinsic object must
    ///   change to point to the intrinsic object.
    pub(crate) fn invalidate_caches_on_intrinsic_shape_property_addition(
        agent: &mut Agent,
        o: Object,
        shape: ObjectShape,
        key: PropertyKey,
        addition_index: u32,
        gc: NoGcScope,
    ) {
        agent.heap.intrinsic_shape_changes += 1;
        let o = o.unbind();
        let shape = shape.unbind();
        let key = key.unbind();
        let self_index = PropertyOffset::new(addition_index);
        let prototype_index = PropertyOffset::new_prototype(addition_index);
        let hash = key.heap_hash(agent);
        let Some((_, WeakReference(cache))) = agent
            .heap
            .caches
            .property_lookup_cache_lookup_table
            .find(hash, |(k, _)| k == &key)
        else {
            // Couldn't find caches for this key; nothing to invalidate.
            return;
        };
        let cache = *cache;
        // Find our invalidated Object's prototypes. Any caches targeting these
        // must be checked along with the UNSET caches.
        let mut prototype_chain = if let Some(proto) = shape.get_prototype(agent) {
            let mut protos = vec![proto];
            let mut proto = proto.try_get_prototype_of(agent, gc);
            while let TryResult::Continue(Some(p)) = proto {
                protos.push(p);
                proto = p.try_get_prototype_of(agent, gc);
            }
            protos
        } else {
            vec![]
        };
        prototype_chain.sort();
        // Find the possible invalidated cache shapes.
        let affected_shapes = {
            let mut cache = cache;
            let mut shapes: Vec<ObjectShape> = vec![];
            let caches = &agent.heap.caches.property_lookup_caches;
            loop {
                let caches = &caches[cache.get_index()];
                shapes.extend(caches.shapes.iter().filter_map(|s| s.as_ref()));
                if let Some(next) = caches.next {
                    cache = next;
                    continue;
                }
                break;
            }
            shapes.sort();
            shapes.dedup();

            // Then filter the shapes to only those that have our intrinsic
            // object in their prototype chain.
            shapes.retain(|s| {
                s == &shape || prototype_chain_includes_object(agent, s.get_prototype(agent), o, gc)
            });
            shapes
        };
        let mut cache = cache;
        let caches = &mut agent.heap.caches.property_lookup_caches;
        let prototypes = &mut agent.heap.caches.property_lookup_cache_prototypes;
        loop {
            let caches = &mut caches[cache.get_index()];
            let prototypes = &mut prototypes[cache.get_index()];
            for (s, offset, proto) in caches
                .shapes
                .iter_mut()
                .zip(caches.offsets.iter_mut())
                .zip(prototypes.prototypes.iter_mut())
                .filter_map(|((s, offset), proto)| {
                    // If the cache is for an affected shape, and...
                    affected_shapes.binary_search(s.as_ref()?).ok()?;
                    if
                    // ...it is cached as unfound, or
                    offset.is_unset() ||
                    // ...it is cached as matching a property in our
                    // prototype chain...
                    prototype_chain.binary_search(proto.as_ref()?).is_ok()
                    {
                        // ...then the cache is invalid: our addition means
                        // that the cache should now point to our object!
                        Some((s, offset, proto))
                    } else {
                        None
                    }
                })
            {
                let self_cache = s.as_mut().unwrap() == &shape;
                let addition_index = if self_cache {
                    self_index
                } else {
                    prototype_index
                };
                let Some(addition_index) = addition_index else {
                    // We cannot add this index; we have to remove the cache.
                    *s = None;
                    *offset = PropertyOffset(0);
                    *proto = None;
                    continue;
                };

                *offset = addition_index;
                if s.as_mut().unwrap() == &shape {
                    // We remember this property lookup on our object itself.
                    *proto = None;
                } else {
                    *proto = Some(o);
                }
            }
            if let Some(next) = caches.next {
                cache = next;
                continue;
            }
            break;
        }
    }

    /// Invalidate property lookup caches on intrinsic shape property removal.
    ///
    /// When such a removal happens, it means that:
    /// * Caches which read the removed property from the intrinsic object must be
    ///   removed.
    /// * Caches which read properties at a later index from the intrinsic object
    ///   must be shifted down by one.
    pub(crate) fn invalidate_caches_on_intrinsic_shape_property_removal(
        agent: &mut Agent,
        o: Object,
        shape: ObjectShape,
        removal_index: u32,
    ) {
        agent.heap.intrinsic_shape_changes += 1;
        let caches = &mut agent.heap.caches;
        let o = o.unbind();
        let shape = shape.unbind();
        let self_index = PropertyOffset::new(removal_index);
        let proto_index = PropertyOffset::new_prototype(removal_index);
        if self_index.is_none() && proto_index.is_none() {
            // Too big an offset to cache; we don't need to look for this.
            return;
        }
        let removal_index = removal_index as u16;
        for ((s, offset), proto) in caches
            .property_lookup_caches
            .iter_mut()
            .zip(caches.property_lookup_cache_prototypes.iter_mut())
            .flat_map(|(cache, prototypes)| {
                cache
                    .shapes
                    .iter_mut()
                    .zip(cache.offsets.iter_mut())
                    .zip(prototypes.prototypes.iter_mut())
            })
            .filter(|((s, offset), proto)| {
                !offset.is_unset()
                    && (*s == &Some(shape)
                        && self_index.as_ref().is_some_and(|i| {
                            offset.get_property_offset() >= i.get_property_offset()
                        })
                        || *proto == &Some(o)
                            && proto_index.as_ref().is_some_and(|i| {
                                offset.get_property_offset() >= i.get_property_offset()
                            }))
            })
        {
            let index = if s == &Some(shape) {
                self_index.unwrap()
            } else {
                proto_index.unwrap()
            };
            // Cache references our shape directly or uses the object as a
            // prototype, and caches a property lookup that is affected by
            // the removal. Time to invalidate!
            let property_offset = index.get_property_offset();
            if property_offset == removal_index {
                // Money shot! This is a cache on the removed property itself.
                *s = None;
                *offset = PropertyOffset(0);
                *proto = None;
            } else {
                // Property after the removed property; shift the offset's
                // absolute value down by one.
                assert_ne!(property_offset, 0);
                offset.0 -= 1;
            }
        }
    }

    /// Invalidate property lookup caches where the properties of an
    /// intrinsic object were moved under its shape: private names were put
    /// in front of them. Every cache that read a property of the object,
    /// from the object itself or through a shape that inherits from it,
    /// has the place the property was at. This happens once for an object
    /// (a constructor that returns it to a class with private elements), so
    /// every cache forgets what it found, whatever it was of.
    pub(crate) fn invalidate_caches_on_intrinsic_shape_properties_moved(agent: &mut Agent) {
        agent.heap.intrinsic_shape_changes += 1;
        let caches = &mut agent.heap.caches;
        for ((s, offset), proto) in caches
            .property_lookup_caches
            .iter_mut()
            .zip(caches.property_lookup_cache_prototypes.iter_mut())
            .flat_map(|(cache, prototypes)| {
                cache
                    .shapes
                    .iter_mut()
                    .zip(cache.offsets.iter_mut())
                    .zip(prototypes.prototypes.iter_mut())
            })
        {
            *s = None;
            *offset = PropertyOffset(0);
            *proto = None;
        }
    }

    /// Invalidate property lookup caches on intrinsic shape prototype change.
    ///
    /// When such a change happens, it means that:
    /// * If the new prototype is non-null, then all caches which have failed
    ///   to find any key in the intrinsic object itself or on any shape that
    ///   uses the intrinsic object as a prototype must be invalidated (if the
    ///   new prototype is non-null).
    /// * If the new prototype is non-null, then all caches with a shape that
    ///   uses the intrinsic object as a prototype and have found any key in
    ///   the intrinsic object's old prototype chain must be invalidated.
    /// * If the new prototype is null, then all caches with a shape that uses
    ///   the intrinsic object as a prototype and have found any key in the
    ///   intrinsic object's old prototype chain must change to UNSET.
    pub(crate) fn invalidate_caches_on_intrinsic_shape_prototype_change(
        agent: &mut Agent,
        o: Object,
        shape: ObjectShape,
        old_prototype: Option<Object>,
        new_prototype: Option<Object>,
        gc: NoGcScope,
    ) {
        agent.heap.intrinsic_shape_changes += 1;
        // Find our invalidated Object's old prototypes.
        let mut prototype_chain = if let Some(proto) = old_prototype {
            let mut protos = vec![proto];
            let mut proto = proto.try_get_prototype_of(agent, gc);
            while let TryResult::Continue(Some(p)) = proto {
                protos.push(p);
                proto = p.try_get_prototype_of(agent, gc);
            }
            protos
        } else {
            vec![]
        };
        prototype_chain.sort();
        // Find the possible invalidated cache shapes.
        let affected_shapes = {
            let mut shapes = agent
                .heap
                .caches
                .property_lookup_caches
                .iter()
                .flat_map(|c| c.shapes.iter())
                .filter_map(|s| s.as_ref())
                .cloned()
                .collect::<Vec<_>>();
            shapes.sort();
            shapes.dedup();

            // Then filter the shapes to only those that have our intrinsic
            // object in their prototype chain.
            shapes.retain(|s| {
                s == &shape || prototype_chain_includes_object(agent, s.get_prototype(agent), o, gc)
            });
            shapes
        };
        for (s, offset, proto) in agent
            .heap
            .caches
            .property_lookup_caches
            .iter_mut()
            .zip(
                agent
                    .heap
                    .caches
                    .property_lookup_cache_prototypes
                    .iter_mut(),
            )
            .flat_map(|(caches, prototypes)| {
                caches
                    .shapes
                    .iter_mut()
                    .zip(caches.offsets.iter_mut())
                    .zip(prototypes.prototypes.iter_mut())
                    .filter_map(|((s, offset), proto)| {
                        // If the cache is for an affected shape, and...
                        affected_shapes.binary_search(s.as_ref()?).ok()?;
                        if
                        // ...it is cached as unfound and our new prototype is
                        // non-null, or...
                        offset.is_unset() && new_prototype.is_some() ||
                        // ...it is cached as matching a property in our
                        // prototype chain...
                        prototype_chain.binary_search(proto.as_ref()?).is_ok()
                        {
                            // ...then the cache is invalid: our prototype
                            // change means that the cache should invalidate!
                            Some((s, offset, proto))
                        } else {
                            None
                        }
                    })
            })
        {
            // ...then the cache is invalid: our prototype change means that
            // the cache should invalidate!

            if new_prototype.is_some() {
                // If we assigned a non-null prototype, then all matched caches
                // invalidate.
                *s = None;
                *offset = PropertyOffset(0);
                *proto = None;
            } else {
                // If we assigned a null prototype then all matched caches
                // should turn to UNSET.
                *offset = PropertyOffset::UNSET;
                *proto = None;
            }
        }
    }

    pub(crate) fn take_current_cache_to_populate(
        &mut self,
        key: PropertyKey,
    ) -> Option<CacheToPopulate<'a>> {
        if self
            .current_cache_to_populate
            .as_ref()
            .is_some_and(|p| p.key == key)
        {
            self.current_cache_to_populate.take()
        } else {
            None
        }
    }

    pub(crate) fn clear_current_cache_to_populate(&mut self) {
        self.current_cache_to_populate = None;
    }

    pub(crate) fn set_current_cache(
        &mut self,
        shape: ObjectShape,
        key: PropertyKey,
        receiver: Value,
        cache: PropertyLookupCache,
    ) {
        let previous = self.current_cache_to_populate.replace(CacheToPopulate {
            receiver: receiver.unbind(),
            cache: cache.unbind(),
            key: key.unbind(),
            shape: shape.unbind(),
        });
        debug_assert!(previous.is_none());
    }

    pub(crate) fn len(&self) -> usize {
        self.property_lookup_caches.len()
    }
}

fn prototype_chain_includes_object(
    agent: &mut Agent,
    prototype: Option<Object>,
    object: Object,
    gc: NoGcScope,
) -> bool {
    let Some(prototype) = prototype else {
        return false;
    };
    if prototype == object {
        true
    } else if let TryResult::Continue(prototype) = prototype.try_get_prototype_of(agent, gc) {
        prototype_chain_includes_object(agent, prototype, object, gc)
    } else {
        false
    }
}

impl Caches<'static> {
    pub(crate) fn mark_cache(&self, index: usize, queues: &mut WorkQueues) {
        self.property_lookup_caches[index].mark_values(queues);
        self.property_lookup_cache_prototypes[index].mark_values(queues);
    }

    pub(crate) fn sweep_cache(
        &mut self,
        compactions: &CompactionLists,
        range: &BitRange,
        bits: &[AtomicBits],
    ) {
        sweep_heap_vector_values(&mut self.property_lookup_caches, compactions, range, bits);
        sweep_heap_vector_values(
            &mut self.property_lookup_cache_prototypes,
            compactions,
            range,
            bits,
        );
    }
}

/// Property lookup caches are an internal optimisation that the engine uses in
/// bytecode execution to speed up property lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Bindable)]
#[repr(transparent)]
pub struct PropertyLookupCache<'a>(NonZeroU32, PhantomData<&'a GcToken>);

impl<'a> PropertyLookupCache<'a> {
    /// Get a property lookup cache entry for the given key if one exists.
    pub(crate) fn get(agent: &Agent, key: PropertyKey<'a>) -> Option<PropertyLookupCache<'a>> {
        let hash = key.heap_hash(agent);
        agent
            .heap
            .caches
            .property_lookup_cache_lookup_table
            .find(hash, |(k, _)| *k == key)
            .map(|(_, c)| c.0)
    }

    /// Get or create a property lookup cache entry for the given key.
    pub(crate) fn new(agent: &mut Agent, key: PropertyKey<'a>) -> PropertyLookupCache<'a> {
        // One of the caches given last, found by the key itself.
        let place = recent_place(key);
        let recent = &agent.heap.caches.recent;
        for place in [place, place ^ 1] {
            if let Some((recent_key, cache)) = recent[place]
                && recent_key == key
            {
                return cache;
            }
        }
        let hash = key.heap_hash(agent);
        let caches = &mut agent.heap.caches;
        let property_key_heap =
            PropertyKeyHeap::new(&mut agent.heap.strings, &mut agent.heap.symbols);
        let entry = caches.property_lookup_cache_lookup_table.entry(
            hash,
            |(k, _)| *k == key,
            |(k, _)| k.heap_hash(&property_key_heap),
        );
        let cache = match entry {
            Entry::Occupied(e) => e.get().1.0,
            Entry::Vacant(e) => {
                caches
                    .property_lookup_caches
                    .push(PropertyLookupCacheRecord::new());
                caches
                    .property_lookup_cache_prototypes
                    .push(PropertyLookupCacheRecordPrototypes::new());
                let cache = PropertyLookupCache::last(&caches.property_lookup_caches);
                e.insert((key.unbind(), WeakReference(cache.unbind())));
                cache
            }
        };
        // The one that was in the key's first place keeps the second.
        caches.recent[place ^ 1] = caches.recent[place];
        caches.recent[place] = Some((key.unbind(), cache.unbind()));
        cache
    }

    /// Find a cached property offset (possibly in a prototype) in this
    /// property lookup cache for the given Object Shape.
    #[inline]
    pub(crate) fn find_cached_property_offset(
        self,
        agent: &Agent,
        shape: ObjectShape<'a>,
    ) -> Option<(PropertyOffset, Option<Object<'a>>)> {
        self.find_cached_place(agent, shape)
            .map(|(offset, prototype, _)| (offset, prototype))
    }

    /// As [`PropertyLookupCache::find_cached_property_offset`], with the
    /// capacity of the storage that the offset is in where the cache knows
    /// it: for a property of the object's own among its values, under a
    /// shape that is not an intrinsic object's. It is
    /// [`ElementArrayKey::Empty`] for every other entry.
    #[inline]
    pub(crate) fn find_cached_place(
        self,
        agent: &Agent,
        shape: ObjectShape<'a>,
    ) -> Option<(PropertyOffset, Option<Object<'a>>, ElementArrayKey)> {
        let caches = &agent.heap.caches;
        // The records of one cache, each of a few shapes, one after another.
        let mut cache = self;
        loop {
            let index = cache.get_index();
            let record = &caches.property_lookup_caches[index];
            let Some((i, offset)) = record.find(shape) else {
                cache = record.next?;
                continue;
            };
            // The capacity is believed only of an offset that still is
            // what it was put in with: an entry may be changed where it is
            // (see the invalidations above), and none of those makes one of
            // this kind.
            if offset.is_own_value() {
                let capacity = record.capacities[i as usize];
                debug_assert!(
                    capacity == ElementArrayKey::Empty
                        || !shape.is_intrinsic(agent) && capacity == shape.values_capacity(agent)
                );
                return Some((offset, None, capacity));
            }
            let prototype = if !offset.is_unset() && offset.is_prototype_property() {
                let prototype =
                    caches.property_lookup_cache_prototypes[index].prototypes[i as usize].unwrap();
                Some(prototype)
            } else {
                debug_assert!(
                    caches.property_lookup_cache_prototypes[index].prototypes[i as usize].is_none()
                );
                None
            };
            return Some((offset, prototype, ElementArrayKey::Empty));
        }
    }

    /// The capacity to keep with an entry that places a property among the
    /// values of an object of `shape`.
    ///
    /// The shape decides the capacity of that storage, and the record of a
    /// shape is never changed: but for the shape of an intrinsic object,
    /// which is that object's alone and is changed where it is as the
    /// object gains and loses properties. Of such a shape nothing is kept,
    /// and the capacity is asked of it at every use.
    fn capacity_to_keep(agent: &Agent, shape: ObjectShape) -> ElementArrayKey {
        if shape.is_intrinsic(agent) {
            return ElementArrayKey::Empty;
        }
        match shape.values_capacity(agent) {
            ElementArrayKey::EmptyIntrinsic => ElementArrayKey::Empty,
            capacity => capacity,
        }
    }

    pub(crate) fn insert_unset(self, agent: &mut Agent, shape: ObjectShape<'a>) {
        debug_assert!(self.find_cached_property_offset(agent, shape).is_none());
        let offset = PropertyOffset::UNSET;
        let capacity = ElementArrayKey::Empty;
        let caches = &mut agent.heap.caches;
        let mut cache = self;
        let next_to_create = PropertyLookupCache::from_index(caches.property_lookup_caches.len());
        loop {
            let (record, _) = cache.get_record_mut(caches);
            if record.insert(shape, offset, capacity).is_some() {
                return;
            }
            if let Some(next) = record.next {
                cache = next;
                continue;
            }
            record.next = Some(next_to_create);
            caches.property_lookup_caches.push(
                PropertyLookupCacheRecord::with_shape_and_offset(shape, offset, capacity).unbind(),
            );
            caches
                .property_lookup_cache_prototypes
                .push(PropertyLookupCacheRecordPrototypes::new());
            let cache = PropertyLookupCache::last(&caches.property_lookup_caches);
            debug_assert_eq!(cache, next_to_create);
            break;
        }
    }

    /// Insert a new lookup offset for a shape.
    ///
    /// There should be no lookup offset for this shape in the cache.
    ///
    /// ## Panics
    ///
    /// If there is an existing lookup offset for this shape in the cache
    /// (debug mode only).
    pub(crate) fn insert_lookup_offset(
        self,
        agent: &mut Agent,
        shape: ObjectShape<'a>,
        index: u32,
    ) {
        debug_assert!(self.find_cached_property_offset(agent, shape).is_none());
        let Some(offset) = PropertyOffset::new(index) else {
            return;
        };
        let capacity = Self::capacity_to_keep(agent, shape);
        let caches = &mut agent.heap.caches;
        let mut cache = self;
        let next_to_create = PropertyLookupCache::from_index(caches.property_lookup_caches.len());
        loop {
            let (record, _) = cache.get_record_mut(caches);
            if record.insert(shape, offset, capacity).is_some() {
                return;
            }
            if let Some(next) = record.next {
                cache = next;
                continue;
            }
            record.next = Some(next_to_create);
            caches.property_lookup_caches.push(
                PropertyLookupCacheRecord::with_shape_and_offset(shape, offset, capacity).unbind(),
            );
            caches
                .property_lookup_cache_prototypes
                .push(PropertyLookupCacheRecordPrototypes::new());
            let cache = PropertyLookupCache::last(&caches.property_lookup_caches);
            debug_assert_eq!(cache, next_to_create);
            break;
        }
    }

    /// Insert a new lookup offset for a shape.
    ///
    /// This is a no-op if the offset is already present in the cache.
    pub(crate) fn insert_lookup_offset_if_not_found(
        self,
        agent: &mut Agent,
        shape: ObjectShape<'a>,
        index: u32,
    ) {
        let Some(offset) = PropertyOffset::new(index) else {
            return;
        };
        let capacity = Self::capacity_to_keep(agent, shape);
        let caches = &mut agent.heap.caches;
        let mut cache = self;
        let next_to_create = PropertyLookupCache::from_index(caches.property_lookup_caches.len());
        loop {
            let (record, _) = cache.get_record_mut(caches);
            if record
                .insert_if_not_found(shape, offset, capacity)
                .is_some()
            {
                return;
            }
            if let Some(next) = record.next {
                cache = next;
                continue;
            }
            record.next = Some(next_to_create);
            caches.property_lookup_caches.push(
                PropertyLookupCacheRecord::with_shape_and_offset(shape, offset, capacity).unbind(),
            );
            caches
                .property_lookup_cache_prototypes
                .push(PropertyLookupCacheRecordPrototypes::new());
            let cache = PropertyLookupCache::last(&caches.property_lookup_caches);
            debug_assert_eq!(cache, next_to_create);
            break;
        }
    }

    pub(crate) fn insert_prototype_lookup_offset(
        self,
        agent: &mut Agent,
        shape: ObjectShape<'a>,
        index: u32,
        prototype: Object<'a>,
    ) {
        debug_assert!(self.find_cached_property_offset(agent, shape).is_none());
        let Some(offset) = PropertyOffset::new_prototype(index) else {
            return;
        };
        let capacity = ElementArrayKey::Empty;
        let caches = &mut agent.heap.caches;
        let mut cache = self;
        let next_to_create = PropertyLookupCache::from_index(caches.property_lookup_caches.len());
        loop {
            let (record, prototypes) = cache.get_record_mut(caches);
            if let Some(i) = record.insert(shape, offset, capacity) {
                debug_assert!(offset.is_prototype_property());
                let previous = prototypes.prototypes[i as usize].replace(prototype.unbind());
                debug_assert!(previous.is_none());
                return;
            }
            if let Some(next) = record.next {
                cache = next;
                continue;
            }
            record.next = Some(next_to_create);
            caches.property_lookup_caches.push(
                PropertyLookupCacheRecord::with_shape_and_offset(shape, offset, capacity).unbind(),
            );
            caches
                .property_lookup_cache_prototypes
                .push(PropertyLookupCacheRecordPrototypes::with_prototype(prototype).unbind());
            let cache = PropertyLookupCache::last(&caches.property_lookup_caches);
            debug_assert_eq!(cache, next_to_create);
            break;
        }
    }

    #[inline(always)]
    fn from_index(index: usize) -> Self {
        Self(
            NonZeroU32::new(u32::try_from(index).unwrap().checked_add(1).unwrap()).unwrap(),
            PhantomData,
        )
    }

    #[inline(always)]
    fn last(caches: &[PropertyLookupCacheRecord<'a>]) -> Self {
        Self(
            NonZeroU32::new(u32::try_from(caches.len()).unwrap()).unwrap(),
            PhantomData,
        )
    }

    #[inline(always)]
    pub(crate) fn get_index(self) -> usize {
        (self.0.get() - 1) as usize
    }

    #[inline(always)]
    fn get_record_mut<'c>(
        self,
        caches: &'c mut Caches<'static>,
    ) -> (
        &'c mut PropertyLookupCacheRecord<'static>,
        &'c mut PropertyLookupCacheRecordPrototypes<'static>,
    ) {
        let index = self.get_index();
        (
            &mut caches.property_lookup_caches[index],
            &mut caches.property_lookup_cache_prototypes[index],
        )
    }
}

impl From<PropertyLookupCache<'_>> for HeapRootData {
    fn from(value: PropertyLookupCache<'_>) -> Self {
        Self::PropertyLookupCache(value.unbind())
    }
}

impl TryFrom<HeapRootData> for PropertyLookupCache<'_> {
    type Error = ();

    fn try_from(value: HeapRootData) -> Result<Self, Self::Error> {
        match value {
            HeapRootData::PropertyLookupCache(p) => Ok(p),
            _ => Err(()),
        }
    }
}

const N: usize = 4;

#[derive(Debug, Bindable)]
pub(crate) struct PropertyLookupCacheRecord<'a> {
    shapes: [Option<ObjectShape<'a>>; N],
    offsets: [PropertyOffset; N],
    /// With each entry, the capacity of the storage its offset is in, where
    /// that is kept (see [`PropertyLookupCache::capacity_to_keep`]). Written
    /// whenever an entry is given its shape, and read only for an offset
    /// among the values of the object itself.
    capacities: [ElementArrayKey; N],
    next: Option<PropertyLookupCache<'a>>,
}

impl<'a> PropertyLookupCacheRecord<'a> {
    const fn new() -> Self {
        Self {
            shapes: [None; N],
            offsets: [PropertyOffset(0); N],
            capacities: [ElementArrayKey::Empty; N],
            next: None,
        }
    }

    fn with_shape_and_offset(
        shape: ObjectShape<'a>,
        offset: PropertyOffset,
        capacity: ElementArrayKey,
    ) -> Self {
        Self {
            shapes: [Some(shape), None, None, None],
            offsets: [
                offset,
                PropertyOffset(0),
                PropertyOffset(0),
                PropertyOffset(0),
            ],
            capacities: [
                capacity,
                ElementArrayKey::Empty,
                ElementArrayKey::Empty,
                ElementArrayKey::Empty,
            ],
            next: None,
        }
    }

    /// Find the property lookup cache for the given Object Shape.
    fn find(&self, shape: ObjectShape<'a>) -> Option<(u8, PropertyOffset)> {
        self.shapes
            .iter()
            .enumerate()
            .find(|(_, s)| **s == Some(shape))
            .map(|(i, _)| (i as u8, self.offsets[i]))
    }

    /// Insert the given Object Shape and lookup cache index into the property
    /// lookup cache record. Returns false if the record is full.
    fn insert(
        &mut self,
        shape: ObjectShape,
        offset: PropertyOffset,
        capacity: ElementArrayKey,
    ) -> Option<u8> {
        if let Some((i, slot)) = self
            .shapes
            .iter_mut()
            .enumerate()
            .find(|(_, s)| s.is_none())
        {
            *slot = Some(shape.unbind());
            self.offsets[i] = offset;
            self.capacities[i] = capacity;
            Some(i as u8)
        } else {
            None
        }
    }

    /// Insert the given Object Shape and lookup cache index into the property
    /// lookup cache record. Returns false if the record is full.
    fn insert_if_not_found(
        &mut self,
        shape: ObjectShape,
        offset: PropertyOffset,
        capacity: ElementArrayKey,
    ) -> Option<u8> {
        if let Some((i, slot)) = self
            .shapes
            .iter_mut()
            .enumerate()
            .find(|(_, s)| s.is_none_or(|s| s == shape))
        {
            if slot == &Some(shape.unbind()) {
                // Duplicate found.
                debug_assert_eq!(self.offsets[i], offset);
                return Some(i as u8);
            }
            *slot = Some(shape.unbind());
            self.offsets[i] = offset;
            self.capacities[i] = capacity;
            Some(i as u8)
        } else {
            None
        }
    }
}

/// An internal engine optimisation used to speed up property lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PropertyOffset(u16);

impl PropertyOffset {
    /// Bit set if this property offset refers into the shape's prototype
    /// chain.
    const PROTOTYPE_BIT_MASK: u16 = 0x8000;
    /// Bit set if this property is found in the target object or prototype's
    /// custom property storage.
    const CUSTOM_STORAGE_BIT_MASK: u16 = 0x4000;
    const UNSET_BIT_MASK: u16 = 0x2000;
    const OFFSET_BIT_MASK: u16 = 0x1FFF;
    /// Property lookup index indicating that the property was not set in the
    /// Object Shape or in its prototype chain.
    const UNSET: Self = Self(Self::UNSET_BIT_MASK);

    /// Create a new property lookup offset.
    ///
    /// Returns None if the offset is beyond supported limits.
    #[inline(always)]
    pub(crate) const fn new(offset: u32) -> Option<Self> {
        let masked = offset & Self::OFFSET_BIT_MASK as u32;
        if masked == offset {
            Some(Self(masked as u16))
        } else {
            None
        }
    }

    /// Create a new prototype property lookup offset.
    ///
    /// Returns None if the offset is beyond supported limits.
    #[inline(always)]
    pub(crate) const fn new_prototype(offset: u32) -> Option<Self> {
        let masked = offset & Self::OFFSET_BIT_MASK as u32;
        if masked == offset {
            Some(Self(masked as u16 | Self::PROTOTYPE_BIT_MASK))
        } else {
            None
        }
    }

    /// Create a new property lookup offset for custom property storage.
    ///
    /// Returns None if the offset is beyond supported limits.
    #[inline(always)]
    pub(crate) const fn new_custom(offset: u32) -> Option<Self> {
        let masked = offset & Self::OFFSET_BIT_MASK as u32;
        if masked == offset {
            Some(Self(masked as u16 | Self::CUSTOM_STORAGE_BIT_MASK))
        } else {
            None
        }
    }

    /// Returns true if the property was found among the values of the
    /// object itself: not unset, not in a prototype, not in custom storage.
    #[inline(always)]
    pub(crate) const fn is_own_value(self) -> bool {
        self.0 & !Self::OFFSET_BIT_MASK == 0
    }

    /// Returns true if the property was not set on the Object with this
    /// Object Shape or in its prototype chain.
    #[inline(always)]
    pub(crate) const fn is_unset(self) -> bool {
        (self.0 & Self::UNSET_BIT_MASK) > 0
    }

    /// Returns true if the property was found on the Object Shape's prototype.
    #[inline(always)]
    pub(crate) const fn is_prototype_property(self) -> bool {
        (self.0 & Self::PROTOTYPE_BIT_MASK) > 0
    }

    /// Returns true if the property was found in the target object or its
    /// prototype's custom property storage.
    #[inline(always)]
    pub(crate) const fn is_custom_property(self) -> bool {
        (self.0 & Self::CUSTOM_STORAGE_BIT_MASK) > 0
    }

    /// Returns the offset that the property was found at.
    #[inline(always)]
    pub(crate) const fn get_property_offset(self) -> u16 {
        debug_assert!(!self.is_unset());
        self.0 & Self::OFFSET_BIT_MASK
    }
}

#[derive(Debug, Bindable)]
#[repr(transparent)]
pub(crate) struct PropertyLookupCacheRecordPrototypes<'a> {
    prototypes: [Option<Object<'a>>; N],
}

impl<'a> PropertyLookupCacheRecordPrototypes<'a> {
    pub(crate) const fn new() -> Self {
        Self {
            prototypes: [None; N],
        }
    }

    pub(crate) const fn with_prototype(prototype: Object<'a>) -> Self {
        Self {
            prototypes: [Some(prototype), None, None, None],
        }
    }
}

impl HeapMarkAndSweep for Caches<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            property_lookup_cache_lookup_table,
            // Note: caches are not marked unconditionally; the heap_gc function
            // will call mark_cache as needed.
            property_lookup_caches: _,
            property_lookup_cache_prototypes: _,
            // property_lookup_cache_stack,
            current_cache_to_populate: current_property_lookup_cache,
            // Not a root: emptied when the heap is swept.
            recent: _,
            arguments_shapes: _,
        } = self;
        // Note: we do not mark the caches in the lookup table; they're weakly
        // held.
        for (key, _) in property_lookup_cache_lookup_table.iter() {
            key.mark_values(queues);
        }
        // property_lookup_cache_stack.mark_values(queues);
        current_property_lookup_cache.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            property_lookup_cache_lookup_table,
            // Note: caches are not sweeped here; the heap_gc function
            // will call sweep_cache.
            property_lookup_caches: _,
            property_lookup_cache_prototypes: _,
            // property_lookup_cache_stack,
            current_cache_to_populate: current_property_lookup_cache,
            recent,
            arguments_shapes,
        } = self;
        // Its keys and caches are of the heap before this collection.
        *recent = [None; RECENT_CACHES];
        *arguments_shapes = [None; ARGUMENTS_SHAPES];
        property_lookup_cache_lookup_table.retain(|(key, cache)| {
            let Some(new_cache) = cache.sweep_weak_reference(compactions) else {
                return false;
            };
            key.sweep_values(compactions);
            *cache = new_cache;
            true
        });
        // property_lookup_cache_stack.sweep_values(compactions);
        current_property_lookup_cache.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for CacheToPopulate<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            receiver,
            cache,
            key,
            shape,
        } = self;
        receiver.mark_values(queues);
        cache.mark_values(queues);
        key.mark_values(queues);
        shape.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            receiver,
            cache,
            key,
            shape,
        } = self;
        receiver.sweep_values(compactions);
        cache.sweep_values(compactions);
        key.sweep_values(compactions);
        shape.sweep_values(compactions);
    }
}

impl HeapSweepWeakReference for PropertyLookupCache<'_> {
    fn sweep_weak_reference(self, compactions: &CompactionLists) -> Option<Self> {
        compactions
            .caches
            .shift_weak_non_zero_u32_index(self.0)
            .map(|i| Self(i, PhantomData))
    }
}

impl HeapMarkAndSweep for PropertyLookupCache<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.caches.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.caches.shift_non_zero_u32_index(&mut self.0);
    }
}

impl HeapMarkAndSweep for PropertyLookupCacheRecord<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            shapes,
            offsets: _,
            capacities: _,
            next: next_cache,
        } = self;
        shapes.as_slice().mark_values(queues);
        next_cache.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            shapes,
            offsets: _,
            capacities: _,
            next: next_cache,
        } = self;
        shapes.as_mut_slice().sweep_values(compactions);
        next_cache.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for PropertyLookupCacheRecordPrototypes<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self { prototypes } = self;
        prototypes.as_slice().mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self { prototypes } = self;
        prototypes.as_mut_slice().sweep_values(compactions);
    }
}

/// That `length` of a TypedArray of one kind, in one Realm, is the accessor
/// of %TypedArray%.prototype, as the last look found it by the property
/// cache and the accessor it read from that prototype
/// (`Vm::typed_array_length`), with what the heap counted then of
/// everything that can make it otherwise.
///
/// A TypedArray with no properties of its own and the prototype of its kind
/// has `length` from a walk of three objects whose shapes are changed in
/// place: a property added to or removed from one of them, or a prototype
/// of one changed, is counted
/// ([`Heap::intrinsic_shape_changes`](crate::heap::Heap)); and the accessor
/// found is that one until the property is defined again
/// ([`Heap::descriptor_changes`](crate::heap::Heap)). While neither count
/// has moved, the walk and the read of the accessor would find what they
/// found. The Realm is a handle that a collection moves, so what was seen
/// before one is not looked at after it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypedArrayLengthSeen<K> {
    kind: K,
    realm: crate::ecmascript::Realm<'static>,
    collections: u64,
    shape_changes: u64,
    descriptor_changes: u64,
}

impl<K: Copy + PartialEq> TypedArrayLengthSeen<K> {
    /// What is seen now, for TypedArrays of `kind` in the running Realm.
    pub(crate) fn now(agent: &Agent, kind: K) -> Self {
        Self {
            kind,
            realm: agent.current_realm_id_internal(),
            collections: agent.heap.collections,
            shape_changes: agent.heap.intrinsic_shape_changes,
            descriptor_changes: agent.heap.descriptor_changes,
        }
    }

    /// Whether what was seen is what a look now, for TypedArrays of `kind`
    /// in the running Realm, would see.
    #[inline]
    pub(crate) fn holds(self, agent: &Agent, kind: K) -> bool {
        self.collections == agent.heap.collections
            && self.shape_changes == agent.heap.intrinsic_shape_changes
            && self.descriptor_changes == agent.heap.descriptor_changes
            && self.kind == kind
            && self.realm == agent.current_realm_id_internal()
    }
}

/// The shape of the objects of one kind that have nothing of their own, in
/// one Realm, as the last look found it (`Vm::inherited_property_value`).
///
/// A Map, a RegExp or a Uint8Array with no properties of its own has the
/// prototype that the running Realm has for its kind, and the shape that
/// the heap keeps for that prototype: one shape for each prototype, put in
/// its table once and taken from it by a collection only. So the shape
/// found for a kind in a Realm is the shape of that kind there until the
/// next collection, which also moves the handles, and what was seen before
/// one is not looked at after it. Finding it anew was the prototype asked
/// of the Realm and the table searched by its hash, at every call of a
/// method of a Map.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BareShapeSeen<K> {
    kind: K,
    realm: crate::ecmascript::Realm<'static>,
    collections: u64,
    shape: ObjectShape<'static>,
}

impl<K: Copy + PartialEq> BareShapeSeen<K> {
    /// `shape` as what is seen now, for the objects of `kind` in the
    /// running Realm.
    pub(crate) fn now(agent: &Agent, kind: K, shape: ObjectShape) -> Self {
        Self {
            kind,
            realm: agent.current_realm_id_internal(),
            collections: agent.heap.collections,
            shape: shape.unbind(),
        }
    }

    /// The shape seen, if a look now, for the objects of `kind` in the
    /// running Realm, would find it.
    #[inline]
    pub(crate) fn shape(self, agent: &Agent, kind: K) -> Option<ObjectShape<'static>> {
        (self.collections == agent.heap.collections
            && self.kind == kind
            && self.realm == agent.current_realm_id_internal())
        .then_some(self.shape)
    }
}
