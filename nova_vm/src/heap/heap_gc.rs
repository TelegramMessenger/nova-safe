// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "array-buffer")]
use crate::heap::heap_bits::sweep_side_table_values;
use crate::{
    ecmascript::{
        Agent, BUILTIN_STRINGS_LIST, Environments, FinalizationRegistry, ObjectShape, Realm,
    },
    engine::{Bindable, GcScope},
    heap::{
        Heap, HeapIndexHandle, WellKnownSymbols,
        element_array::ElementArrays,
        heap_bits::{
            CompactionLists, HeapBits, HeapMarkAndSweep, WorkQueues, mark_descriptors,
            sweep_heap_elements_vector_descriptors, sweep_heap_soa_vector_values,
            sweep_heap_vector_values, sweep_lookup_table,
        },
    },
    ndt,
};

/// Allocations between two garbage collections of a small heap.
pub(crate) const MIN_GC_ALLOC_LIMIT: usize = 2 * 1024 * 1024;
/// A larger heap waits for this many allocated bytes per live item, so that
/// collections stay proportional to the work done between them.
const GC_ALLOC_BYTES_PER_LIVE_ITEM: usize = 16;

#[cfg_attr(feature = "probes", allow(unsafe_code))]
pub(crate) fn heap_gc(agent: &mut Agent, root_realms: &mut [Option<Realm<'static>>], gc: GcScope) {
    ndt::gc_start!(|| ());
    agent.heap.collections += 1;

    let mut bits = HeapBits::new(&agent.heap);
    bits.strings
        .mark_range(0..(BUILTIN_STRINGS_LIST.len() as u32), &mut bits.bits);
    bits.symbols
        .mark_range(0..(WellKnownSymbols::Unscopables as u32), &mut bits.bits);
    let mut queues = WorkQueues::new(&agent.heap, &bits);
    root_realms.iter().for_each(|realm| {
        if let Some(realm) = realm {
            queues.realms.push(realm.unbind());
        }
    });
    queues.object_shapes.push(ObjectShape::NULL);
    agent.heap.prototype_shapes.mark_values(&mut queues);
    agent.heap.caches.mark_values(&mut queues);
    agent.mark_values(&mut queues);
    let mut has_finalization_registries = false;

    loop {
        if queues.is_empty() {
            if queues.pending_ephemerons.is_empty() {
                break;
            }
            let new_values_to_mark = queues
                .pending_ephemerons
                .extract_if(.., |(key, _)| queues.bits.is_marked(key))
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            for value in new_values_to_mark {
                value.mark_values(&mut queues);
            }
            if queues.is_empty() {
                break;
            }
        }
        let Heap {
            #[cfg(feature = "array-buffer")]
            array_buffers,
            #[cfg(feature = "array-buffer")]
                array_buffer_detach_keys: _,
            arrays,
            array_iterators,
            async_generators,
            await_reactions,
            bigints,
            bound_functions,
            builtin_constructors,
            builtin_functions,
            caches,
            #[cfg(feature = "date")]
            dates,
            #[cfg(feature = "temporal")]
            instants,
            #[cfg(feature = "temporal")]
            durations,
            #[cfg(feature = "temporal")]
            plain_times,
            ecmascript_functions,
            elements,
            embedder_objects,
            environments,
            errors,
            executables,
            source_codes,
            finalization_registries,
            generators,
            globals: _,
            free_globals: _,
            gc_alloc_limit: _,
            maps,
            map_iterators,
            modules,
            module_request_records,
            numbers,
            object_shapes,
            object_shape_transitions,
            prototype_shapes: _,
            objects,
            primitive_objects,
            promise_reaction_records,
            promise_resolving_functions,
            promise_finally_functions,
            promises,
            promise_group_records,
            proxies,
            realms,
            #[cfg(feature = "regexp")]
            regexps,
            #[cfg(feature = "regexp")]
            regexp_string_iterators,
            scripts,
            #[cfg(feature = "set")]
            sets,
            #[cfg(feature = "set")]
            set_iterators,
            #[cfg(feature = "shared-array-buffer")]
            shared_array_buffers,
            source_text_module_records,
            string_iterators,
            strings,
            string_lookup_table: _,
            string_hasher: _,
            text_join_budget: _,
            shape_key_hasher: _,
            symbols,
            #[cfg(feature = "array-buffer")]
            typed_arrays,
            #[cfg(feature = "array-buffer")]
                typed_array_byte_lengths: _,
            #[cfg(feature = "array-buffer")]
                typed_array_byte_offsets: _,
            #[cfg(feature = "array-buffer")]
                typed_array_array_lengths: _,
            #[cfg(feature = "array-buffer")]
            data_views,
            #[cfg(feature = "array-buffer")]
                data_view_byte_lengths: _,
            #[cfg(feature = "array-buffer")]
                data_view_byte_offsets: _,
            #[cfg(feature = "shared-array-buffer")]
            shared_typed_arrays,
            #[cfg(feature = "shared-array-buffer")]
                shared_typed_array_byte_lengths: _,
            #[cfg(feature = "shared-array-buffer")]
                shared_typed_array_byte_offsets: _,
            #[cfg(feature = "shared-array-buffer")]
                shared_typed_array_array_lengths: _,
            #[cfg(feature = "shared-array-buffer")]
            shared_data_views,
            #[cfg(feature = "shared-array-buffer")]
                shared_data_view_byte_lengths: _,
            #[cfg(feature = "shared-array-buffer")]
                shared_data_view_byte_offsets: _,
            #[cfg(feature = "weak-refs")]
            weak_maps,
            #[cfg(feature = "weak-refs")]
            weak_refs,
            #[cfg(feature = "weak-refs")]
            weak_sets,
            alloc_counter: _,
            data_bytes_since_gc: _,
            collections: _,
            intrinsic_key_moves: _,
            intrinsic_shape_changes: _,
            bare_shape_seen: _,
            descriptor_changes: _,
            intrinsic_integer_keys: _,
        } = &agent.heap;
        let Environments {
            declarative: declarative_environments,
            function: function_environments,
            global: global_environments,
            module: module_environments,
            object: object_environments,
            private: private_environments,
        } = environments;
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
            key_offsets: _,
        } = elements;

        while let Some(idx) = queues.modules.pop() {
            let index = idx.get_index();
            if bits.modules.set_bit(index, &bits.bits) {
                modules.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.scripts.pop() {
            let index = idx.get_index();
            if bits.scripts.set_bit(index, &bits.bits) {
                scripts.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.realms.pop() {
            let index = idx.get_index();
            if bits.realms.set_bit(index, &bits.bits) {
                realms.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.declarative_environments.pop() {
            let index = idx.get_index();
            if bits.declarative_environments.set_bit(index, &bits.bits) {
                declarative_environments.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.function_environments.pop() {
            let index = idx.get_index();
            if bits.function_environments.set_bit(index, &bits.bits) {
                function_environments.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.global_environments.pop() {
            let index = idx.get_index();
            if bits.global_environments.set_bit(index, &bits.bits) {
                global_environments.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.module_environments.pop() {
            let index = idx.get_index();
            if bits.module_environments.set_bit(index, &bits.bits) {
                module_environments.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.object_environments.pop() {
            let index = idx.get_index();
            if bits.object_environments.set_bit(index, &bits.bits) {
                object_environments.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.private_environments.pop() {
            let index = idx.get_index();
            if bits.private_environments.set_bit(index, &bits.bits) {
                private_environments.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.arrays.pop() {
            let index = idx.get_index();
            if bits.arrays.set_bit(index, &bits.bits) {
                arrays.get(index as u32).mark_values(&mut queues);
            }
        }

        #[cfg(feature = "array-buffer")]
        {
            while let Some(idx) = queues.array_buffers.pop() {
                let index = idx.get_index();
                if bits.array_buffers.set_bit(index, &bits.bits) {
                    array_buffers.get(index).mark_values(&mut queues);
                }
            }
        }

        while let Some(idx) = queues.array_iterators.pop() {
            let index = idx.get_index();
            if bits.array_iterators.set_bit(index, &bits.bits) {
                array_iterators.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.async_generators.pop() {
            let index = idx.get_index();
            if bits.async_generators.set_bit(index, &bits.bits) {
                async_generators.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.await_reactions.pop() {
            let index = idx.get_index();
            if bits.await_reactions.set_bit(index, &bits.bits) {
                await_reactions.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.bigints.pop() {
            let index = idx.get_index();
            if bits.bigints.set_bit(index, &bits.bits) {
                bigints.get(index).mark_values(&mut queues);
            }
        }

        while let Some(idx) = queues.bound_functions.pop() {
            let index = idx.get_index();
            if bits.bound_functions.set_bit(index, &bits.bits) {
                bound_functions.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.ecmascript_functions.pop() {
            let index = idx.get_index();
            if bits.ecmascript_functions.set_bit(index, &bits.bits) {
                ecmascript_functions.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.errors.pop() {
            let index = idx.get_index();
            if bits.errors.set_bit(index, &bits.bits) {
                errors.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.executables.pop() {
            let index = idx.get_index();
            if bits.executables.set_bit(index, &bits.bits) {
                executables.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.source_codes.pop() {
            let index = idx.get_index();
            if bits.source_codes.set_bit(index, &bits.bits) {
                source_codes.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.builtin_constructors.pop() {
            let index = idx.get_index();
            if bits.builtin_constructors.set_bit(index, &bits.bits) {
                builtin_constructors.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.builtin_functions.pop() {
            let index = idx.get_index();
            if bits.builtin_functions.set_bit(index, &bits.bits) {
                builtin_functions.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.caches.pop() {
            let index = idx.get_index();
            if bits.caches.set_bit(index, &bits.bits) {
                caches.mark_cache(index, &mut queues);
            }
        }
        #[cfg(feature = "array-buffer")]
        while let Some(idx) = queues.data_views.pop() {
            let index = idx.get_index();
            if bits.data_views.set_bit(index, &bits.bits) {
                data_views.get(index).mark_values(&mut queues);
            }
        }

        #[cfg(feature = "date")]
        while let Some(idx) = queues.dates.pop() {
            let index = idx.get_index();
            if bits.dates.set_bit(index, &bits.bits) {
                dates.get(index).mark_values(&mut queues);
            }
        }
        #[cfg(feature = "temporal")]
        {
            while let Some(idx) = queues.instants.pop() {
                let index = idx.get_index();
                if bits.instants.set_bit(index, &bits.bits) {
                    instants.get(index).mark_values(&mut queues);
                }
            }
            while let Some(idx) = queues.durations.pop() {
                let index = idx.get_index();
                if bits.durations.set_bit(index, &bits.bits) {
                    durations.get(index).mark_values(&mut queues);
                }
            }
            while let Some(idx) = queues.plain_times.pop() {
                let index = idx.get_index();
                if bits.plain_times.set_bit(index, &bits.bits) {
                    plain_times.get(index).mark_values(&mut queues);
                }
            }
        }

        while let Some(idx) = queues.embedder_objects.pop() {
            let index = idx.get_index();
            if bits.embedder_objects.set_bit(index, &bits.bits) {
                embedder_objects.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.finalization_registries.pop() {
            has_finalization_registries = true;
            let index = idx.get_index();
            if bits.finalization_registries.set_bit(index, &bits.bits) {
                finalization_registries
                    .get(index as u32)
                    .mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.generators.pop() {
            let index = idx.get_index();
            if bits.generators.set_bit(index, &bits.bits) {
                generators.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.object_shapes.pop() {
            let index = idx.get_index();
            if bits.object_shapes.set_bit(index, &bits.bits) {
                object_shapes.get(index).mark_values(&mut queues);
                object_shape_transitions.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.objects.pop() {
            let index = idx.get_index();
            if bits.objects.set_bit(index, &bits.bits) {
                if let Some(rec) = objects.get(index) {
                    rec.mark_values(&mut queues, &agent.heap.object_shapes);
                }
            }
        }
        while let Some(idx) = queues.promises.pop() {
            let index = idx.get_index();
            if bits.promises.set_bit(index, &bits.bits) {
                promises.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.promise_reaction_records.pop() {
            let index = idx.get_index();
            if bits.promise_reaction_records.set_bit(index, &bits.bits) {
                promise_reaction_records.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.promise_group_records.pop() {
            let index = idx.get_index();
            if bits.promise_group_records.set_bit(index, &bits.bits) {
                promise_group_records.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.promise_resolving_functions.pop() {
            let index = idx.get_index();
            if bits.promise_resolving_functions.set_bit(index, &bits.bits) {
                promise_resolving_functions
                    .get(index)
                    .mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.promise_finally_functions.pop() {
            let index = idx.get_index();
            if bits.promise_finally_functions.set_bit(index, &bits.bits) {
                promise_finally_functions
                    .get(index)
                    .mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.proxies.pop() {
            let index = idx.get_index();
            if bits.proxies.set_bit(index, &bits.bits) {
                proxies.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.maps.pop() {
            let index = idx.get_index();
            if bits.maps.set_bit(index, &bits.bits) {
                maps.get(index as u32).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.map_iterators.pop() {
            let index = idx.get_index();
            if bits.map_iterators.set_bit(index, &bits.bits) {
                map_iterators.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.module_request_records.pop() {
            let index = idx.get_index();
            if bits.module_request_records.set_bit(index, &bits.bits) {
                module_request_records.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.numbers.pop() {
            let index = idx.get_index();
            if bits.numbers.set_bit(index, &bits.bits) {
                numbers.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.primitive_objects.pop() {
            let index = idx.get_index();
            if bits.primitive_objects.set_bit(index, &bits.bits) {
                primitive_objects.get(index).mark_values(&mut queues);
            }
        }
        #[cfg(feature = "regexp")]
        {
            while let Some(idx) = queues.regexps.pop() {
                let index = idx.get_index();
                if bits.regexps.set_bit(index, &bits.bits) {
                    regexps.get(index).mark_values(&mut queues);
                }
            }
            while let Some(idx) = queues.regexp_string_iterators.pop() {
                let index = idx.get_index();
                if bits.regexp_string_iterators.set_bit(index, &bits.bits) {
                    regexp_string_iterators.get(index).mark_values(&mut queues);
                }
            }
        }
        #[cfg(feature = "set")]
        {
            while let Some(idx) = queues.sets.pop() {
                let index = idx.get_index();
                if bits.sets.set_bit(index, &bits.bits) {
                    sets.get(index as u32).mark_values(&mut queues);
                }
            }

            while let Some(idx) = queues.set_iterators.pop() {
                let index = idx.get_index();
                if bits.set_iterators.set_bit(index, &bits.bits) {
                    set_iterators.get(index).mark_values(&mut queues);
                }
            }
        }
        #[cfg(feature = "shared-array-buffer")]
        {
            while let Some(idx) = queues.shared_array_buffers.pop() {
                let index = idx.get_index();
                if bits.shared_array_buffers.set_bit(index, &bits.bits) {
                    shared_array_buffers.get(index).mark_values(&mut queues);
                }
            }
            while let Some(idx) = queues.shared_data_views.pop() {
                let index = idx.get_index();
                if bits.shared_data_views.set_bit(index, &bits.bits) {
                    shared_data_views.get(index).mark_values(&mut queues);
                }
            }
            while let Some(idx) = queues.shared_typed_arrays.pop() {
                let index = idx.get_index();
                if bits.shared_typed_arrays.set_bit(index, &bits.bits) {
                    shared_typed_arrays.get(index).mark_values(&mut queues);
                }
            }
        }
        while let Some(idx) = queues.source_text_module_records.pop() {
            let index = idx.get_index();
            if bits.source_text_module_records.set_bit(index, &bits.bits) {
                source_text_module_records
                    .get(index)
                    .mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.string_iterators.pop() {
            let index = idx.get_index();
            if bits.string_iterators.set_bit(index, &bits.bits) {
                string_iterators.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.strings.pop() {
            let index = idx.get_index();
            if bits.strings.set_bit(index, &bits.bits) {
                strings.get(index).mark_values(&mut queues);
            }
        }
        while let Some(idx) = queues.symbols.pop() {
            let index = idx.get_index();
            if bits.symbols.set_bit(index, &bits.bits) {
                symbols.get(index).mark_values(&mut queues);
            }
        }
        #[cfg(feature = "array-buffer")]
        while let Some(idx) = queues.typed_arrays.pop() {
            let index = idx.get_index();
            if bits.typed_arrays.set_bit(index, &bits.bits) {
                typed_arrays.get(index).mark_values(&mut queues);
            }
        }
        #[cfg(feature = "weak-refs")]
        {
            while let Some(idx) = queues.weak_maps.pop() {
                let index = idx.get_index();
                if bits.weak_maps.set_bit(index, &bits.bits) {
                    weak_maps.get(index).mark_values(&mut queues);
                }
            }
            while let Some(idx) = queues.weak_refs.pop() {
                let index = idx.get_index();
                if bits.weak_refs.set_bit(index, &bits.bits) {
                    weak_refs.get(index).mark_values(&mut queues);
                }
            }
            while let Some(idx) = queues.weak_sets.pop() {
                let index = idx.get_index();
                if bits.weak_sets.set_bit(index, &bits.bits) {
                    weak_sets.get(index).mark_values(&mut queues);
                }
            }
        }

        while let Some(idx) = queues.e_2_1.pop() {
            let index = idx.get_index();
            if bits.e_2_1.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow1.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow1.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_2.pop() {
            let index = idx.get_index();
            if bits.e_2_2.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow2.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow2.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_3.pop() {
            let index = idx.get_index();
            if bits.e_2_3.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow3.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow3.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_4.pop() {
            let index = idx.get_index();
            if bits.e_2_4.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow4.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow4.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_6.pop() {
            let index = idx.get_index();
            if bits.e_2_6.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow6.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow6.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_8.pop() {
            let index = idx.get_index();
            if bits.e_2_8.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow8.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow8.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_10.pop() {
            let index = idx.get_index();
            if bits.e_2_10.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow10.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow10.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_12.pop() {
            let index = idx.get_index();
            if bits.e_2_12.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow12.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow12.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_16.pop() {
            let index = idx.get_index();
            if bits.e_2_16.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow16.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow16.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_24.pop() {
            let index = idx.get_index();
            if bits.e_2_24.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow24.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow24.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }
        while let Some(idx) = queues.e_2_32.pop() {
            let index = idx.get_index();
            if bits.e_2_32.set_bit(index, &bits.bits) {
                if let Some(descriptors) = e2pow32.descriptors.get(&idx) {
                    mark_descriptors(descriptors, &mut queues);
                }
                e2pow32.values.get(index).mark_values(&mut queues);
            } else if cfg!(debug_assertions) {
                panic!("ElementsVector was not unique");
            }
        }

        while let Some(idx) = queues.k_2_4.pop() {
            let index = idx.get_index();
            if bits.k_2_4.set_bit(index, &bits.bits) {
                k2pow4.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_1.pop() {
            let index = idx.get_index();
            if bits.k_2_1.set_bit(index, &bits.bits) {
                k2pow1.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_2.pop() {
            let index = idx.get_index();
            if bits.k_2_2.set_bit(index, &bits.bits) {
                k2pow2.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_3.pop() {
            let index = idx.get_index();
            if bits.k_2_3.set_bit(index, &bits.bits) {
                k2pow3.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_6.pop() {
            let index = idx.get_index();
            if bits.k_2_6.set_bit(index, &bits.bits) {
                k2pow6.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_8.pop() {
            let index = idx.get_index();
            if bits.k_2_8.set_bit(index, &bits.bits) {
                k2pow8.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_10.pop() {
            let index = idx.get_index();
            if bits.k_2_10.set_bit(index, &bits.bits) {
                k2pow10.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_12.pop() {
            let index = idx.get_index();
            if bits.k_2_12.set_bit(index, &bits.bits) {
                k2pow12.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_16.pop() {
            let index = idx.get_index();
            if bits.k_2_16.set_bit(index, &bits.bits) {
                k2pow16.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_24.pop() {
            let index = idx.get_index();
            if bits.k_2_24.set_bit(index, &bits.bits) {
                k2pow24.keys.get(index).mark_values(&mut queues)
            }
        }
        while let Some(idx) = queues.k_2_32.pop() {
            let index = idx.get_index();
            if bits.k_2_32.set_bit(index, &bits.bits) {
                k2pow32.keys.get(index).mark_values(&mut queues)
            }
        }
    }

    sweep(agent, &bits, root_realms, gc);
    if has_finalization_registries {
        FinalizationRegistry::enqueue_cleanup_jobs(agent);
    }
    ndt::gc_done!(|| ());
}

// NOTE: This is the one true use of the `GcScope` which is why we allow a lint
// exception here. For future reference see [this comment](https://github.com/trynova/nova/pull/913#discussion_r2616482397).
#[allow(unknown_lints, can_use_no_gc_scope)]
fn sweep(
    agent: &mut Agent,
    bits: &HeapBits,
    root_realms: &mut [Option<Realm<'static>>],
    _: GcScope,
) {
    let compactions = CompactionLists::create_from_bits(bits, agent.heap.shape_key_hasher);

    for realm in root_realms {
        realm.sweep_values(&compactions);
    }

    agent.sweep_values(&compactions);

    let Heap {
        #[cfg(feature = "array-buffer")]
        array_buffers,
        #[cfg(feature = "array-buffer")]
        array_buffer_detach_keys,
        arrays,
        array_iterators,
        async_generators,
        await_reactions,
        bigints,
        bound_functions,
        builtin_constructors,
        builtin_functions,
        caches,
        #[cfg(feature = "date")]
        dates,
        #[cfg(feature = "temporal")]
        instants,
        #[cfg(feature = "temporal")]
        durations,
        #[cfg(feature = "temporal")]
        plain_times,
        ecmascript_functions,
        elements,
        embedder_objects,
        environments,
        errors,
        executables,
        source_codes,
        finalization_registries,
        generators,
        globals,
        maps,
        map_iterators,
        modules,
        module_request_records,
        numbers,
        object_shapes,
        object_shape_transitions,
        prototype_shapes,
        objects,
        primitive_objects,
        promise_reaction_records,
        promise_resolving_functions,
        promise_finally_functions,
        promises,
        promise_group_records,
        proxies,
        realms,
        #[cfg(feature = "regexp")]
        regexps,
        #[cfg(feature = "regexp")]
        regexp_string_iterators,
        scripts,
        #[cfg(feature = "set")]
        sets,
        #[cfg(feature = "set")]
        set_iterators,
        #[cfg(feature = "shared-array-buffer")]
        shared_array_buffers,
        source_text_module_records,
        string_iterators,
        strings,
        string_lookup_table,
        string_hasher: _,
        text_join_budget: _,
        shape_key_hasher: _,
        symbols,
        #[cfg(feature = "array-buffer")]
        typed_arrays,
        #[cfg(feature = "array-buffer")]
        typed_array_byte_lengths,
        #[cfg(feature = "array-buffer")]
        typed_array_byte_offsets,
        #[cfg(feature = "array-buffer")]
        typed_array_array_lengths,
        #[cfg(feature = "array-buffer")]
        data_views,
        #[cfg(feature = "array-buffer")]
        data_view_byte_lengths,
        #[cfg(feature = "array-buffer")]
        data_view_byte_offsets,
        #[cfg(feature = "shared-array-buffer")]
        shared_typed_arrays,
        #[cfg(feature = "shared-array-buffer")]
        shared_typed_array_byte_lengths,
        #[cfg(feature = "shared-array-buffer")]
        shared_typed_array_byte_offsets,
        #[cfg(feature = "shared-array-buffer")]
        shared_typed_array_array_lengths,
        #[cfg(feature = "shared-array-buffer")]
        shared_data_views,
        #[cfg(feature = "shared-array-buffer")]
        shared_data_view_byte_lengths,
        #[cfg(feature = "shared-array-buffer")]
        shared_data_view_byte_offsets,
        #[cfg(feature = "weak-refs")]
        weak_maps,
        #[cfg(feature = "weak-refs")]
        weak_refs,
        #[cfg(feature = "weak-refs")]
        weak_sets,
        alloc_counter,
        data_bytes_since_gc: _,
        free_globals: _,
        gc_alloc_limit,
        collections: _,
        intrinsic_key_moves: _,
        intrinsic_shape_changes: _,
        bare_shape_seen: _,
        descriptor_changes: _,
        intrinsic_integer_keys: _,
    } = &mut agent.heap;
    // Reset the allocation counter.
    *alloc_counter = 0;
    *gc_alloc_limit = MIN_GC_ALLOC_LIMIT.max(bits.marked_count() * GC_ALLOC_BYTES_PER_LIVE_ITEM);
    let Environments {
        declarative,
        function,
        global,
        module,
        object,
        private,
    } = environments;
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
        key_offsets,
    } = elements;
    key_offsets.clear();

    let mut globals = globals.borrow_mut();
    let globals_iter = globals.iter_mut();
    {
        let s = SequentialSweep;
        s.spawn(|| {
            prototype_shapes.sweep_values(&compactions);
        });

        s.spawn(|| {
            caches.sweep_cache(&compactions, &bits.caches, &bits.bits);
            caches.sweep_values(&compactions);
        });

        s.spawn(|| {
            for value in globals_iter {
                value.sweep_values(&compactions);
            }
        });
        if !e2pow1.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow1.descriptors,
                    &compactions,
                    &compactions.e_2_1,
                    &bits.e_2_1,
                    &bits.bits,
                );
                e2pow1.values.sweep(&compactions, &bits.e_2_1, &bits.bits);
            });
        }
        if !e2pow2.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow2.descriptors,
                    &compactions,
                    &compactions.e_2_2,
                    &bits.e_2_2,
                    &bits.bits,
                );
                e2pow2.values.sweep(&compactions, &bits.e_2_2, &bits.bits);
            });
        }
        if !e2pow3.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow3.descriptors,
                    &compactions,
                    &compactions.e_2_3,
                    &bits.e_2_3,
                    &bits.bits,
                );
                e2pow3.values.sweep(&compactions, &bits.e_2_3, &bits.bits);
            });
        }
        if !e2pow4.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow4.descriptors,
                    &compactions,
                    &compactions.e_2_4,
                    &bits.e_2_4,
                    &bits.bits,
                );
                e2pow4.values.sweep(&compactions, &bits.e_2_4, &bits.bits);
            });
        }
        if !e2pow6.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow6.descriptors,
                    &compactions,
                    &compactions.e_2_6,
                    &bits.e_2_6,
                    &bits.bits,
                );
                e2pow6.values.sweep(&compactions, &bits.e_2_6, &bits.bits);
            });
        }
        if !e2pow8.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow8.descriptors,
                    &compactions,
                    &compactions.e_2_8,
                    &bits.e_2_8,
                    &bits.bits,
                );
                e2pow8.values.sweep(&compactions, &bits.e_2_8, &bits.bits);
            });
        }
        if !e2pow10.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow10.descriptors,
                    &compactions,
                    &compactions.e_2_10,
                    &bits.e_2_10,
                    &bits.bits,
                );
                e2pow10.values.sweep(&compactions, &bits.e_2_10, &bits.bits);
            });
        }
        if !e2pow12.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow12.descriptors,
                    &compactions,
                    &compactions.e_2_12,
                    &bits.e_2_12,
                    &bits.bits,
                );
                e2pow12.values.sweep(&compactions, &bits.e_2_12, &bits.bits);
            });
        }
        if !e2pow16.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow16.descriptors,
                    &compactions,
                    &compactions.e_2_16,
                    &bits.e_2_16,
                    &bits.bits,
                );
                e2pow16.values.sweep(&compactions, &bits.e_2_16, &bits.bits);
            });
        }
        if !e2pow24.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow24.descriptors,
                    &compactions,
                    &compactions.e_2_24,
                    &bits.e_2_24,
                    &bits.bits,
                );
                e2pow24.values.sweep(&compactions, &bits.e_2_24, &bits.bits);
            });
        }
        if !e2pow32.values.is_empty() {
            s.spawn(|| {
                sweep_heap_elements_vector_descriptors(
                    &mut e2pow32.descriptors,
                    &compactions,
                    &compactions.e_2_32,
                    &bits.e_2_32,
                    &bits.bits,
                );
                e2pow32.values.sweep(&compactions, &bits.e_2_32, &bits.bits);
            });
        }
        if !k2pow1.keys.is_empty() {
            s.spawn(|| {
                k2pow1.keys.sweep(&compactions, &bits.k_2_1, &bits.bits);
            });
        }
        if !k2pow2.keys.is_empty() {
            s.spawn(|| {
                k2pow2.keys.sweep(&compactions, &bits.k_2_2, &bits.bits);
            });
        }
        if !k2pow3.keys.is_empty() {
            s.spawn(|| {
                k2pow3.keys.sweep(&compactions, &bits.k_2_3, &bits.bits);
            });
        }
        if !k2pow4.keys.is_empty() {
            s.spawn(|| {
                k2pow4.keys.sweep(&compactions, &bits.k_2_4, &bits.bits);
            });
        }
        if !k2pow6.keys.is_empty() {
            s.spawn(|| {
                k2pow6.keys.sweep(&compactions, &bits.k_2_6, &bits.bits);
            });
        }
        if !k2pow8.keys.is_empty() {
            s.spawn(|| {
                k2pow8.keys.sweep(&compactions, &bits.k_2_8, &bits.bits);
            });
        }
        if !k2pow10.keys.is_empty() {
            s.spawn(|| {
                k2pow10.keys.sweep(&compactions, &bits.k_2_10, &bits.bits);
            });
        }
        if !k2pow12.keys.is_empty() {
            s.spawn(|| {
                k2pow12.keys.sweep(&compactions, &bits.k_2_12, &bits.bits);
            });
        }
        if !k2pow16.keys.is_empty() {
            s.spawn(|| {
                k2pow16.keys.sweep(&compactions, &bits.k_2_16, &bits.bits);
            });
        }
        if !k2pow24.keys.is_empty() {
            s.spawn(|| {
                k2pow24.keys.sweep(&compactions, &bits.k_2_24, &bits.bits);
            });
        }
        if !k2pow32.keys.is_empty() {
            s.spawn(|| {
                k2pow32.keys.sweep(&compactions, &bits.k_2_32, &bits.bits);
            });
        }
        #[cfg(feature = "array-buffer")]
        if !array_buffers.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    array_buffers,
                    &compactions,
                    &bits.array_buffers,
                    &bits.bits,
                );
                sweep_side_table_values(array_buffer_detach_keys, &compactions);
            });
        }
        if !arrays.is_empty() {
            s.spawn(|| {
                sweep_heap_soa_vector_values(arrays, &compactions, &bits.arrays, &bits.bits);
            });
        }
        if !array_iterators.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    array_iterators,
                    &compactions,
                    &bits.array_iterators,
                    &bits.bits,
                );
            });
        }
        if !async_generators.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    async_generators,
                    &compactions,
                    &bits.async_generators,
                    &bits.bits,
                );
            });
        }
        if !await_reactions.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    await_reactions,
                    &compactions,
                    &bits.await_reactions,
                    &bits.bits,
                );
            });
        }
        if !bigints.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(bigints, &compactions, &bits.bigints, &bits.bits);
            });
        }
        if !bound_functions.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    bound_functions,
                    &compactions,
                    &bits.bound_functions,
                    &bits.bits,
                );
            });
        }
        if !builtin_constructors.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    builtin_constructors,
                    &compactions,
                    &bits.builtin_constructors,
                    &bits.bits,
                );
            });
        }
        if !builtin_functions.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    builtin_functions,
                    &compactions,
                    &bits.builtin_functions,
                    &bits.bits,
                );
            });
        }
        #[cfg(feature = "array-buffer")]
        if !data_views.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(data_views, &compactions, &bits.data_views, &bits.bits);
                sweep_side_table_values(data_view_byte_lengths, &compactions);
                sweep_side_table_values(data_view_byte_offsets, &compactions);
            });
        }
        #[cfg(feature = "shared-array-buffer")]
        if !shared_data_views.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    shared_data_views,
                    &compactions,
                    &bits.shared_data_views,
                    &bits.bits,
                );
                sweep_side_table_values(shared_data_view_byte_lengths, &compactions);
                sweep_side_table_values(shared_data_view_byte_offsets, &compactions);
            });
        }
        #[cfg(feature = "date")]
        if !dates.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(dates, &compactions, &bits.dates, &bits.bits);
            });
        }
        #[cfg(feature = "temporal")]
        if !instants.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(instants, &compactions, &bits.instants, &bits.bits);
            });
        }
        #[cfg(feature = "temporal")]
        if !durations.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(durations, &compactions, &bits.durations, &bits.bits);
            });
        }
        #[cfg(feature = "temporal")]
        if !plain_times.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(plain_times, &compactions, &bits.plain_times, &bits.bits);
            });
        }
        if !declarative.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    declarative,
                    &compactions,
                    &bits.declarative_environments,
                    &bits.bits,
                );
            });
        }
        if !embedder_objects.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    embedder_objects,
                    &compactions,
                    &bits.embedder_objects,
                    &bits.bits,
                );
            });
        }
        if !errors.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(errors, &compactions, &bits.errors, &bits.bits);
            });
        }
        if !finalization_registries.is_empty() {
            s.spawn(|| {
                sweep_heap_soa_vector_values(
                    finalization_registries,
                    &compactions,
                    &bits.finalization_registries,
                    &bits.bits,
                );
            });
        }
        if !function.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    function,
                    &compactions,
                    &bits.function_environments,
                    &bits.bits,
                );
            });
        }
        if !generators.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(generators, &compactions, &bits.generators, &bits.bits);
            });
        }
        if !global.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    global,
                    &compactions,
                    &bits.global_environments,
                    &bits.bits,
                );
            });
        }
        if !module.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    module,
                    &compactions,
                    &bits.module_environments,
                    &bits.bits,
                );
            });
        }
        if !maps.is_empty() {
            s.spawn(|| {
                sweep_heap_soa_vector_values(maps, &compactions, &bits.maps, &bits.bits);
            });
        }
        if !map_iterators.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    map_iterators,
                    &compactions,
                    &bits.map_iterators,
                    &bits.bits,
                );
            });
        }
        if !modules.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(modules, &compactions, &bits.modules, &bits.bits);
            });
        }
        if !module_request_records.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    module_request_records,
                    &compactions,
                    &bits.module_request_records,
                    &bits.bits,
                );
            });
        }
        if !numbers.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(numbers, &compactions, &bits.numbers, &bits.bits);
            });
        }
        if !object.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    object,
                    &compactions,
                    &bits.object_environments,
                    &bits.bits,
                );
            });
        }
        if !private.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    private,
                    &compactions,
                    &bits.private_environments,
                    &bits.bits,
                );
            });
        }
        if !object_shapes.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    object_shape_transitions,
                    &compactions,
                    &bits.object_shapes,
                    &bits.bits,
                );
            });
        }
        if !object_shapes.is_empty() || !objects.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    object_shapes,
                    &compactions,
                    &bits.object_shapes,
                    &bits.bits,
                );
                assert_eq!(objects.len(), bits.objects.len());
                let mut iter = bits.objects.iter(&bits.bits);
                objects.retain_mut(|item| {
                    let do_retain = iter.next().unwrap();
                    if do_retain {
                        item.sweep_values(&compactions, object_shapes);
                        true
                    } else {
                        false
                    }
                });
            });
        }
        if !primitive_objects.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    primitive_objects,
                    &compactions,
                    &bits.primitive_objects,
                    &bits.bits,
                );
            });
        }
        if !promise_reaction_records.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    promise_reaction_records,
                    &compactions,
                    &bits.promise_reaction_records,
                    &bits.bits,
                );
            });
        }
        if !promise_resolving_functions.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    promise_resolving_functions,
                    &compactions,
                    &bits.promise_resolving_functions,
                    &bits.bits,
                );
            });
        }
        if !promise_finally_functions.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    promise_finally_functions,
                    &compactions,
                    &bits.promise_finally_functions,
                    &bits.bits,
                );
            });
        }
        if !promises.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(promises, &compactions, &bits.promises, &bits.bits);
            });
        }
        if !promise_group_records.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    promise_group_records,
                    &compactions,
                    &bits.promise_group_records,
                    &bits.bits,
                );
            });
        }
        if !proxies.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(proxies, &compactions, &bits.proxies, &bits.bits);
            });
        }
        #[cfg(feature = "regexp")]
        if !regexps.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(regexps, &compactions, &bits.regexps, &bits.bits);
            });
        }
        #[cfg(feature = "regexp")]
        if !regexp_string_iterators.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    regexp_string_iterators,
                    &compactions,
                    &bits.regexp_string_iterators,
                    &bits.bits,
                );
            });
        }
        #[cfg(feature = "set")]
        if !sets.is_empty() {
            s.spawn(|| {
                sweep_heap_soa_vector_values(sets, &compactions, &bits.sets, &bits.bits);
            });
        }
        #[cfg(feature = "set")]
        if !set_iterators.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    set_iterators,
                    &compactions,
                    &bits.set_iterators,
                    &bits.bits,
                );
            });
        }
        #[cfg(feature = "shared-array-buffer")]
        if !shared_array_buffers.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    shared_array_buffers,
                    &compactions,
                    &bits.shared_array_buffers,
                    &bits.bits,
                );
            });
        }
        if !string_iterators.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    string_iterators,
                    &compactions,
                    &bits.string_iterators,
                    &bits.bits,
                );
            });
        }
        if !strings.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(strings, &compactions, &bits.strings, &bits.bits);
                sweep_lookup_table(string_lookup_table, &compactions);
            });
        }
        if !symbols.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(symbols, &compactions, &bits.symbols, &bits.bits);
            });
        }
        #[cfg(feature = "array-buffer")]
        if !typed_arrays.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    typed_arrays,
                    &compactions,
                    &bits.typed_arrays,
                    &bits.bits,
                );
                sweep_side_table_values(typed_array_byte_lengths, &compactions);
                sweep_side_table_values(typed_array_byte_offsets, &compactions);
                sweep_side_table_values(typed_array_array_lengths, &compactions);
            });
        }
        #[cfg(feature = "shared-array-buffer")]
        if !shared_typed_arrays.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(
                    shared_typed_arrays,
                    &compactions,
                    &bits.shared_typed_arrays,
                    &bits.bits,
                );
                sweep_side_table_values(shared_typed_array_byte_lengths, &compactions);
                sweep_side_table_values(shared_typed_array_byte_offsets, &compactions);
                sweep_side_table_values(shared_typed_array_array_lengths, &compactions);
            });
        }
        #[cfg(feature = "weak-refs")]
        if !weak_maps.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(weak_maps, &compactions, &bits.weak_maps, &bits.bits);
            });
        }
        #[cfg(feature = "weak-refs")]
        if !weak_refs.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(weak_refs, &compactions, &bits.weak_refs, &bits.bits);
            });
        }
        #[cfg(feature = "weak-refs")]
        if !weak_sets.is_empty() {
            s.spawn(|| {
                sweep_heap_vector_values(weak_sets, &compactions, &bits.weak_sets, &bits.bits);
            });
        }

        sweep_heap_vector_values(
            ecmascript_functions,
            &compactions,
            &bits.ecmascript_functions,
            &bits.bits,
        );
        sweep_heap_vector_values(executables, &compactions, &bits.executables, &bits.bits);
        sweep_heap_vector_values(realms, &compactions, &bits.realms, &bits.bits);
        sweep_heap_vector_values(scripts, &compactions, &bits.scripts, &bits.bits);
        sweep_heap_vector_values(
            source_text_module_records,
            &compactions,
            &bits.source_text_module_records,
            &bits.bits,
        );
        sweep_heap_vector_values(source_codes, &compactions, &bits.source_codes, &bits.bits);
    }
}

struct SequentialSweep;

impl SequentialSweep {
    #[inline(always)]
    fn spawn(&self, sweep: impl FnOnce()) {
        sweep()
    }
}

#[test]
fn test_heap_gc() {
    use crate::engine::GcScope;
    use crate::{
        ecmascript::{AgentOptions, DefaultHostHooks, OrdinaryObject},
        engine::HeapRootData,
    };

    let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);

    let (mut gc, mut scope) = GcScope::create_root();
    let mut gc = GcScope::new(&mut gc, &mut scope);
    assert!(agent.heap.objects.is_empty());
    let obj = HeapRootData::from(
        OrdinaryObject::create_object(&mut agent, None, &[]).expect("Should perform GC here"),
    );
    agent.heap.globals.borrow_mut().push(obj);
    heap_gc(&mut agent, &mut [], gc.reborrow());

    assert_eq!(agent.heap.objects.len(), 1);
    assert_eq!(agent.heap.elements.e2pow4.values.len(), 0);
    assert!(agent.heap.globals.borrow().last().is_some());
    println!(
        "Global #1: {:#?}",
        agent.heap.globals.borrow().last().unwrap()
    );
}
