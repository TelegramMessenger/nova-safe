// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[cfg(feature = "regexp")]
use crate::ecmascript::RegExpStringIterator;
#[cfg(feature = "shared-array-buffer")]
use crate::ecmascript::{SharedArrayBuffer, SharedVoidArray};
#[cfg(feature = "array-buffer")]
use crate::heap::heap_bits::sweep_side_table_values;
use crate::{
    ecmascript::{
        Agent, BUILTIN_STRINGS_LIST, BuiltinConstructorFunction, BuiltinPromiseFinallyFunction,
        BuiltinPromiseResolvingFunction, DeclarativeEnvironment, ECMAScriptFunction, Environments,
        FinalizationRegistry, FunctionEnvironment, GlobalEnvironment, ModuleEnvironment,
        ModuleRequest, ObjectEnvironment, ObjectShape, PrivateEnvironment, PromiseGroup,
        PromiseReaction, Realm, SourceTextModule,
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

        if !queues.modules.is_empty() {
            let mut module_marks = core::mem::take(&mut queues.modules);
            module_marks.retain(|idx| !bits.modules.get_bit(idx.get_index(), &bits.bits));
            module_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.modules.set_bit(index, &bits.bits) {
                    // Did mark.
                    modules.get(index).mark_values(&mut queues);
                }
            });
            module_marks.clear();
            if queues.modules.is_empty() {
                queues.modules = module_marks;
            }
        }
        if !queues.scripts.is_empty() {
            let mut script_marks = core::mem::take(&mut queues.scripts);
            script_marks.retain(|idx| !bits.scripts.get_bit(idx.get_index(), &bits.bits));
            script_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.scripts.set_bit(index, &bits.bits) {
                    // Did mark.
                    scripts.get(index).mark_values(&mut queues);
                }
            });
            script_marks.clear();
            if queues.scripts.is_empty() {
                queues.scripts = script_marks;
            }
        }
        if !queues.realms.is_empty() {
            let mut realm_marks = core::mem::take(&mut queues.realms);
            realm_marks.retain(|idx| !bits.realms.get_bit(idx.get_index(), &bits.bits));
            realm_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.realms.set_bit(index, &bits.bits) {
                    // Did mark.
                    realms.get(index).mark_values(&mut queues);
                }
            });
            realm_marks.clear();
            if queues.realms.is_empty() {
                queues.realms = realm_marks;
            }
        }

        if !queues.declarative_environments.is_empty() {
            let declarative_environment_marks: Box<[DeclarativeEnvironment]> = queues
                .declarative_environments
                .drain(..)
                .filter(|idx| {
                    !bits
                        .declarative_environments
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            declarative_environment_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.declarative_environments.set_bit(index, &bits.bits) {
                    // Did mark.
                    declarative_environments.get(index).mark_values(&mut queues);
                }
            });
        }

        if !queues.function_environments.is_empty() {
            let function_environment_marks: Box<[FunctionEnvironment]> = queues
                .function_environments
                .drain(..)
                .filter(|idx| {
                    !bits
                        .function_environments
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            function_environment_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.function_environments.set_bit(index, &bits.bits) {
                    // Did mark.
                    function_environments.get(index).mark_values(&mut queues);
                }
            });
        }

        if !queues.global_environments.is_empty() {
            let global_environment_marks: Box<[GlobalEnvironment]> = queues
                .global_environments
                .drain(..)
                .filter(|idx| {
                    !bits
                        .global_environments
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            global_environment_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.global_environments.set_bit(index, &bits.bits) {
                    // Did mark.
                    global_environments.get(index).mark_values(&mut queues);
                }
            });
        }

        if !queues.module_environments.is_empty() {
            let module_environment_marks: Box<[ModuleEnvironment]> = queues
                .module_environments
                .drain(..)
                .filter(|idx| {
                    !bits
                        .module_environments
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            module_environment_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.module_environments.set_bit(index, &bits.bits) {
                    // Did mark.
                    module_environments.get(index).mark_values(&mut queues);
                }
            });
        }

        if !queues.object_environments.is_empty() {
            let object_environment_marks: Box<[ObjectEnvironment]> = queues
                .object_environments
                .drain(..)
                .filter(|idx| {
                    !bits
                        .object_environments
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            object_environment_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.object_environments.set_bit(index, &bits.bits) {
                    // Did mark.
                    object_environments.get(index).mark_values(&mut queues);
                }
            });
        }

        if !queues.private_environments.is_empty() {
            let private_environment_marks: Box<[PrivateEnvironment]> = queues
                .private_environments
                .drain(..)
                .filter(|idx| {
                    !bits
                        .private_environments
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            private_environment_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.private_environments.set_bit(index, &bits.bits) {
                    // Did mark.
                    private_environments.get(index).mark_values(&mut queues);
                }
            });
        }

        if !queues.arrays.is_empty() {
            let mut array_marks = core::mem::take(&mut queues.arrays);
            array_marks.retain(|idx| !bits.arrays.get_bit(idx.get_index(), &bits.bits));
            array_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.arrays.set_bit(index, &bits.bits) {
                    // Did mark.
                    arrays.get(index as u32).mark_values(&mut queues);
                }
            });
            array_marks.clear();
            if queues.arrays.is_empty() {
                queues.arrays = array_marks;
            }
        }

        #[cfg(feature = "array-buffer")]
        {
            if !queues.array_buffers.is_empty() {
                let mut array_buffer_marks = core::mem::take(&mut queues.array_buffers);
                array_buffer_marks
                    .retain(|idx| !bits.array_buffers.get_bit(idx.get_index(), &bits.bits));
                array_buffer_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.array_buffers.set_bit(index, &bits.bits) {
                        // Did mark.
                        array_buffers.get(index).mark_values(&mut queues);
                    }
                });
                array_buffer_marks.clear();
                if queues.array_buffers.is_empty() {
                    queues.array_buffers = array_buffer_marks;
                }
            }
        }

        if !queues.array_iterators.is_empty() {
            let mut array_iterator_marks = core::mem::take(&mut queues.array_iterators);
            array_iterator_marks
                .retain(|idx| !bits.array_iterators.get_bit(idx.get_index(), &bits.bits));
            array_iterator_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.array_iterators.set_bit(index, &bits.bits) {
                    // Did mark.
                    array_iterators.get(index).mark_values(&mut queues);
                }
            });
            array_iterator_marks.clear();
            if queues.array_iterators.is_empty() {
                queues.array_iterators = array_iterator_marks;
            }
        }

        if !queues.async_generators.is_empty() {
            let mut async_generator_marks = core::mem::take(&mut queues.async_generators);
            async_generator_marks
                .retain(|idx| !bits.async_generators.get_bit(idx.get_index(), &bits.bits));
            async_generator_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.async_generators.set_bit(index, &bits.bits) {
                    // Did mark.
                    async_generators.get(index).mark_values(&mut queues);
                }
            });
            async_generator_marks.clear();
            if queues.async_generators.is_empty() {
                queues.async_generators = async_generator_marks;
            }
        }

        if !queues.await_reactions.is_empty() {
            let mut await_reaction_marks = core::mem::take(&mut queues.await_reactions);
            await_reaction_marks
                .retain(|idx| !bits.await_reactions.get_bit(idx.get_index(), &bits.bits));
            await_reaction_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.await_reactions.set_bit(index, &bits.bits) {
                    // Did mark.
                    await_reactions.get(index).mark_values(&mut queues);
                }
            });
            await_reaction_marks.clear();
            if queues.await_reactions.is_empty() {
                queues.await_reactions = await_reaction_marks;
            }
        }

        if !queues.bigints.is_empty() {
            let mut bigint_marks = core::mem::take(&mut queues.bigints);
            bigint_marks.retain(|idx| !bits.bigints.get_bit(idx.get_index(), &bits.bits));
            bigint_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.bigints.set_bit(index, &bits.bits) {
                    // Did mark.
                    bigints.get(index).mark_values(&mut queues);
                }
            });
            bigint_marks.clear();
            if queues.bigints.is_empty() {
                queues.bigints = bigint_marks;
            }
        }

        if !queues.bound_functions.is_empty() {
            let mut bound_function_marks = core::mem::take(&mut queues.bound_functions);
            bound_function_marks
                .retain(|idx| !bits.bound_functions.get_bit(idx.get_index(), &bits.bits));
            bound_function_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.bound_functions.set_bit(index, &bits.bits) {
                    // Did mark.
                    bound_functions.get(index).mark_values(&mut queues);
                }
            });
            bound_function_marks.clear();
            if queues.bound_functions.is_empty() {
                queues.bound_functions = bound_function_marks;
            }
        }
        if !queues.ecmascript_functions.is_empty() {
            let ecmascript_function_marks: Box<[ECMAScriptFunction]> = queues
                .ecmascript_functions
                .drain(..)
                .filter(|idx| {
                    !bits
                        .ecmascript_functions
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            ecmascript_function_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.ecmascript_functions.set_bit(index, &bits.bits) {
                    // Did mark.
                    ecmascript_functions.get(index).mark_values(&mut queues);
                }
            });
        }
        if !queues.errors.is_empty() {
            let mut error_marks = core::mem::take(&mut queues.errors);
            error_marks.retain(|idx| !bits.errors.get_bit(idx.get_index(), &bits.bits));
            error_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.errors.set_bit(index, &bits.bits) {
                    // Did mark.
                    errors.get(index).mark_values(&mut queues);
                }
            });
            error_marks.clear();
            if queues.errors.is_empty() {
                queues.errors = error_marks;
            }
        }
        if !queues.executables.is_empty() {
            let mut executable_marks = core::mem::take(&mut queues.executables);
            executable_marks.retain(|idx| !bits.executables.get_bit(idx.get_index(), &bits.bits));
            executable_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.executables.set_bit(index, &bits.bits) {
                    // Did mark.
                    executables.get(index).mark_values(&mut queues);
                }
            });
            executable_marks.clear();
            if queues.executables.is_empty() {
                queues.executables = executable_marks;
            }
        }
        if !queues.source_codes.is_empty() {
            let mut source_code_marks = core::mem::take(&mut queues.source_codes);
            source_code_marks.retain(|idx| !bits.source_codes.get_bit(idx.get_index(), &bits.bits));
            source_code_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.source_codes.set_bit(index, &bits.bits) {
                    // Did mark.
                    source_codes.get(index).mark_values(&mut queues);
                }
            });
            source_code_marks.clear();
            if queues.source_codes.is_empty() {
                queues.source_codes = source_code_marks;
            }
        }
        if !queues.builtin_constructors.is_empty() {
            let builtin_constructors_marks: Box<[BuiltinConstructorFunction]> = queues
                .builtin_constructors
                .drain(..)
                .filter(|idx| {
                    !bits
                        .builtin_constructors
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            builtin_constructors_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.builtin_constructors.set_bit(index, &bits.bits) {
                    // Did mark.
                    builtin_constructors.get(index).mark_values(&mut queues);
                }
            });
        }
        if !queues.builtin_functions.is_empty() {
            let mut builtin_functions_marks = core::mem::take(&mut queues.builtin_functions);
            builtin_functions_marks
                .retain(|idx| !bits.builtin_functions.get_bit(idx.get_index(), &bits.bits));
            builtin_functions_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.builtin_functions.set_bit(index, &bits.bits) {
                    // Did mark.
                    builtin_functions.get(index).mark_values(&mut queues);
                }
            });
            builtin_functions_marks.clear();
            if queues.builtin_functions.is_empty() {
                queues.builtin_functions = builtin_functions_marks;
            }
        }
        if !queues.caches.is_empty() {
            let mut caches_marks = core::mem::take(&mut queues.caches);
            caches_marks.retain(|idx| !bits.caches.get_bit(idx.get_index(), &bits.bits));
            caches_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.caches.set_bit(index, &bits.bits) {
                    // Did mark.
                    caches.mark_cache(index, &mut queues);
                }
            });
            caches_marks.clear();
            if queues.caches.is_empty() {
                queues.caches = caches_marks;
            }
        }
        #[cfg(feature = "array-buffer")]
        if !queues.data_views.is_empty() {
            let mut data_view_marks = core::mem::take(&mut queues.data_views);
            data_view_marks.retain(|idx| !bits.data_views.get_bit(idx.get_index(), &bits.bits));
            data_view_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.data_views.set_bit(index, &bits.bits) {
                    // Did mark.
                    data_views.get(index).mark_values(&mut queues);
                }
            });
            data_view_marks.clear();
            if queues.data_views.is_empty() {
                queues.data_views = data_view_marks;
            }
        }

        #[cfg(feature = "date")]
        if !queues.dates.is_empty() {
            let mut date_marks = core::mem::take(&mut queues.dates);
            date_marks.retain(|idx| !bits.dates.get_bit(idx.get_index(), &bits.bits));
            date_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.dates.set_bit(index, &bits.bits) {
                    // Did mark.
                    dates.get(index).mark_values(&mut queues);
                }
            });
            date_marks.clear();
            if queues.dates.is_empty() {
                queues.dates = date_marks;
            }
        }
        #[cfg(feature = "temporal")]
        {
            let mut instant_marks = core::mem::take(&mut queues.instants);
            instant_marks.retain(|idx| !bits.instants.get_bit(idx.get_index(), &bits.bits));
            instant_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.instants.set_bit(index, &bits.bits) {
                    // Did mark.
                    instants.get(index).mark_values(&mut queues);
                }
            });
            instant_marks.clear();
            if queues.instants.is_empty() {
                queues.instants = instant_marks;
            }
            let mut duration_marks = core::mem::take(&mut queues.durations);
            duration_marks.retain(|idx| !bits.durations.get_bit(idx.get_index(), &bits.bits));
            duration_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.durations.set_bit(index, &bits.bits) {
                    // Did mark.
                    durations.get(index).mark_values(&mut queues);
                }
            });
            duration_marks.clear();
            if queues.durations.is_empty() {
                queues.durations = duration_marks;
            }
            let mut plain_time_marks = core::mem::take(&mut queues.plain_times);
            plain_time_marks.retain(|idx| !bits.plain_times.get_bit(idx.get_index(), &bits.bits));
            plain_time_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.plain_times.set_bit(index, &bits.bits) {
                    // Did mark.
                    plain_times.get(index).mark_values(&mut queues);
                }
            });
            plain_time_marks.clear();
            if queues.plain_times.is_empty() {
                queues.plain_times = plain_time_marks;
            }
        }

        if !queues.embedder_objects.is_empty() {
            let mut embedder_object_marks = core::mem::take(&mut queues.embedder_objects);
            embedder_object_marks
                .retain(|idx| !bits.embedder_objects.get_bit(idx.get_index(), &bits.bits));
            embedder_object_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.embedder_objects.set_bit(index, &bits.bits) {
                    // Did mark.
                    embedder_objects.get(index).mark_values(&mut queues);
                }
            });
            embedder_object_marks.clear();
            if queues.embedder_objects.is_empty() {
                queues.embedder_objects = embedder_object_marks;
            }
        }
        if !queues.finalization_registries.is_empty() {
            let finalization_registry_marks: Box<[FinalizationRegistry]> =
                queues.finalization_registries.drain(..).collect();
            if !finalization_registry_marks.is_empty() {
                has_finalization_registries = true;
            }
            finalization_registry_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.finalization_registries.set_bit(index, &bits.bits) {
                    // Did mark.

                    finalization_registries
                        .get(index as u32)
                        .mark_values(&mut queues);
                }
            });
        }
        if !queues.generators.is_empty() {
            let mut generator_marks = core::mem::take(&mut queues.generators);
            generator_marks.retain(|idx| !bits.generators.get_bit(idx.get_index(), &bits.bits));
            generator_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.generators.set_bit(index, &bits.bits) {
                    // Did mark.
                    generators.get(index).mark_values(&mut queues);
                }
            });
            generator_marks.clear();
            if queues.generators.is_empty() {
                queues.generators = generator_marks;
            }
        }
        if !queues.object_shapes.is_empty() {
            let mut object_marks = core::mem::take(&mut queues.object_shapes);
            object_marks.retain(|idx| !bits.object_shapes.get_bit(idx.get_index(), &bits.bits));
            object_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.object_shapes.set_bit(index, &bits.bits) {
                    // Did mark.
                    object_shapes.get(index).mark_values(&mut queues);
                    object_shape_transitions.get(index).mark_values(&mut queues);
                }
            });
            object_marks.clear();
            if queues.object_shapes.is_empty() {
                queues.object_shapes = object_marks;
            }
        }
        if !queues.objects.is_empty() {
            let mut object_marks = core::mem::take(&mut queues.objects);
            object_marks.retain(|idx| !bits.objects.get_bit(idx.get_index(), &bits.bits));
            object_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.objects.set_bit(index, &bits.bits) {
                    // Did mark.
                    if let Some(rec) = objects.get(index) {
                        rec.mark_values(&mut queues, &agent.heap.object_shapes);
                    }
                }
            });
            object_marks.clear();
            if queues.objects.is_empty() {
                queues.objects = object_marks;
            }
        }
        if !queues.promises.is_empty() {
            let mut promise_marks = core::mem::take(&mut queues.promises);
            promise_marks.retain(|idx| !bits.promises.get_bit(idx.get_index(), &bits.bits));
            promise_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.promises.set_bit(index, &bits.bits) {
                    // Did mark.
                    promises.get(index).mark_values(&mut queues);
                }
            });
            promise_marks.clear();
            if queues.promises.is_empty() {
                queues.promises = promise_marks;
            }
        }
        if !queues.promise_reaction_records.is_empty() {
            let promise_reaction_record_marks: Box<[PromiseReaction]> = queues
                .promise_reaction_records
                .drain(..)
                .filter(|idx| {
                    !bits
                        .promise_reaction_records
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            promise_reaction_record_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.promise_reaction_records.set_bit(index, &bits.bits) {
                    // Did mark.
                    promise_reaction_records.get(index).mark_values(&mut queues);
                }
            });
        }
        if !queues.promise_group_records.is_empty() {
            let promise_group_record_marks: Box<[PromiseGroup]> = queues
                .promise_group_records
                .drain(..)
                .filter(|idx| {
                    !bits
                        .promise_group_records
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            promise_group_record_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.promise_group_records.set_bit(index, &bits.bits) {
                    // Did mark.
                    promise_group_records.get(index).mark_values(&mut queues);
                }
            });
        }
        if !queues.promise_resolving_functions.is_empty() {
            let promise_resolving_function_marks: Box<[BuiltinPromiseResolvingFunction]> = queues
                .promise_resolving_functions
                .drain(..)
                .filter(|idx| {
                    !bits
                        .promise_resolving_functions
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            promise_resolving_function_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.promise_resolving_functions.set_bit(index, &bits.bits) {
                    // Did mark.
                    promise_resolving_functions
                        .get(index)
                        .mark_values(&mut queues);
                }
            });
        }
        if !queues.promise_finally_functions.is_empty() {
            let promise_finally_function_marks: Box<[BuiltinPromiseFinallyFunction]> = queues
                .promise_finally_functions
                .drain(..)
                .filter(|idx| {
                    !bits
                        .promise_finally_functions
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            promise_finally_function_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.promise_finally_functions.set_bit(index, &bits.bits) {
                    // Did mark.
                    promise_finally_functions
                        .get(index)
                        .mark_values(&mut queues);
                }
            });
        }
        if !queues.proxies.is_empty() {
            let mut proxy_marks = core::mem::take(&mut queues.proxies);
            proxy_marks.retain(|idx| !bits.proxies.get_bit(idx.get_index(), &bits.bits));
            proxy_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.proxies.set_bit(index, &bits.bits) {
                    // Did mark.
                    proxies.get(index).mark_values(&mut queues);
                }
            });
            proxy_marks.clear();
            if queues.proxies.is_empty() {
                queues.proxies = proxy_marks;
            }
        }
        if !queues.maps.is_empty() {
            let mut map_marks = core::mem::take(&mut queues.maps);
            map_marks.retain(|idx| !bits.maps.get_bit(idx.get_index(), &bits.bits));
            map_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.maps.set_bit(index, &bits.bits) {
                    // Did mark.
                    maps.get(index as u32).mark_values(&mut queues);
                }
            });
            map_marks.clear();
            if queues.maps.is_empty() {
                queues.maps = map_marks;
            }
        }
        if !queues.map_iterators.is_empty() {
            let mut map_iterator_marks = core::mem::take(&mut queues.map_iterators);
            map_iterator_marks
                .retain(|idx| !bits.map_iterators.get_bit(idx.get_index(), &bits.bits));
            map_iterator_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.map_iterators.set_bit(index, &bits.bits) {
                    // Did mark.
                    map_iterators.get(index).mark_values(&mut queues);
                }
            });
            map_iterator_marks.clear();
            if queues.map_iterators.is_empty() {
                queues.map_iterators = map_iterator_marks;
            }
        }
        if !queues.module_request_records.is_empty() {
            let module_request_record_marks: Box<[ModuleRequest]> = queues
                .module_request_records
                .drain(..)
                .filter(|idx| {
                    !bits
                        .module_request_records
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            module_request_record_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.module_request_records.set_bit(index, &bits.bits) {
                    // Did mark.
                    module_request_records.get(index).mark_values(&mut queues);
                }
            });
        }
        if !queues.numbers.is_empty() {
            let mut number_marks = core::mem::take(&mut queues.numbers);
            number_marks.retain(|idx| !bits.numbers.get_bit(idx.get_index(), &bits.bits));
            number_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.numbers.set_bit(index, &bits.bits) {
                    // Did mark.
                    numbers.get(index).mark_values(&mut queues);
                }
            });
            number_marks.clear();
            if queues.numbers.is_empty() {
                queues.numbers = number_marks;
            }
        }
        if !queues.primitive_objects.is_empty() {
            let mut primitive_object_marks = core::mem::take(&mut queues.primitive_objects);
            primitive_object_marks
                .retain(|idx| !bits.primitive_objects.get_bit(idx.get_index(), &bits.bits));
            primitive_object_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.primitive_objects.set_bit(index, &bits.bits) {
                    // Did mark.
                    primitive_objects.get(index).mark_values(&mut queues);
                }
            });
            primitive_object_marks.clear();
            if queues.primitive_objects.is_empty() {
                queues.primitive_objects = primitive_object_marks;
            }
        }
        #[cfg(feature = "regexp")]
        {
            if !queues.regexps.is_empty() {
                let mut regexp_marks = core::mem::take(&mut queues.regexps);
                regexp_marks.retain(|idx| !bits.regexps.get_bit(idx.get_index(), &bits.bits));
                regexp_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.regexps.set_bit(index, &bits.bits) {
                        // Did mark.
                        regexps.get(index).mark_values(&mut queues);
                    }
                });
                regexp_marks.clear();
                if queues.regexps.is_empty() {
                    queues.regexps = regexp_marks;
                }
            }
            if !queues.regexp_string_iterators.is_empty() {
                let regexp_string_iterator_marks: Box<[RegExpStringIterator]> = queues
                    .regexp_string_iterators
                    .drain(..)
                    .filter(|idx| {
                        !bits
                            .regexp_string_iterators
                            .get_bit(idx.get_index(), &bits.bits)
                    })
                    .collect();
                regexp_string_iterator_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.regexp_string_iterators.set_bit(index, &bits.bits) {
                        // Did mark.
                        regexp_string_iterators.get(index).mark_values(&mut queues);
                    }
                });
            }
        }
        #[cfg(feature = "set")]
        {
            if !queues.sets.is_empty() {
                let mut set_marks = core::mem::take(&mut queues.sets);
                set_marks.retain(|idx| !bits.sets.get_bit(idx.get_index(), &bits.bits));
                set_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.sets.set_bit(index, &bits.bits) {
                        // Did mark.
                        sets.get(index as u32).mark_values(&mut queues);
                    }
                });
                set_marks.clear();
                if queues.sets.is_empty() {
                    queues.sets = set_marks;
                }
            }

            if !queues.set_iterators.is_empty() {
                let mut set_iterator_marks = core::mem::take(&mut queues.set_iterators);
                set_iterator_marks
                    .retain(|idx| !bits.set_iterators.get_bit(idx.get_index(), &bits.bits));
                set_iterator_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.set_iterators.set_bit(index, &bits.bits) {
                        // Did mark.
                        set_iterators.get(index).mark_values(&mut queues);
                    }
                });
                set_iterator_marks.clear();
                if queues.set_iterators.is_empty() {
                    queues.set_iterators = set_iterator_marks;
                }
            }
        }
        #[cfg(feature = "shared-array-buffer")]
        {
            if !queues.shared_array_buffers.is_empty() {
                let shared_array_buffer_marks: Box<[SharedArrayBuffer]> = queues
                    .shared_array_buffers
                    .drain(..)
                    .filter(|idx| {
                        !bits
                            .shared_array_buffers
                            .get_bit(idx.get_index(), &bits.bits)
                    })
                    .collect();
                shared_array_buffer_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.shared_array_buffers.set_bit(index, &bits.bits) {
                        // Did mark.
                        shared_array_buffers.get(index).mark_values(&mut queues);
                    }
                });
            }
            if !queues.shared_data_views.is_empty() {
                let mut shared_data_view_marks = core::mem::take(&mut queues.shared_data_views);
                shared_data_view_marks
                    .retain(|idx| !bits.shared_data_views.get_bit(idx.get_index(), &bits.bits));
                shared_data_view_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.shared_data_views.set_bit(index, &bits.bits) {
                        // Did mark.
                        shared_data_views.get(index).mark_values(&mut queues);
                    }
                });
                shared_data_view_marks.clear();
                if queues.shared_data_views.is_empty() {
                    queues.shared_data_views = shared_data_view_marks;
                }
            }
            if !queues.shared_typed_arrays.is_empty() {
                let shared_typed_array_marks: Box<[SharedVoidArray]> = queues
                    .shared_typed_arrays
                    .drain(..)
                    .filter(|idx| {
                        !bits
                            .shared_typed_arrays
                            .get_bit(idx.get_index(), &bits.bits)
                    })
                    .collect();
                shared_typed_array_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.shared_typed_arrays.set_bit(index, &bits.bits) {
                        // Did mark.
                        shared_typed_arrays.get(index).mark_values(&mut queues);
                    }
                });
            }
        }
        if !queues.source_text_module_records.is_empty() {
            let source_text_module_record_marks: Box<[SourceTextModule]> = queues
                .source_text_module_records
                .drain(..)
                .filter(|idx| {
                    !bits
                        .source_text_module_records
                        .get_bit(idx.get_index(), &bits.bits)
                })
                .collect();
            source_text_module_record_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.source_text_module_records.set_bit(index, &bits.bits) {
                    // Did mark.
                    source_text_module_records
                        .get(index)
                        .mark_values(&mut queues);
                }
            });
        }
        if !queues.string_iterators.is_empty() {
            let mut string_generator_marks = core::mem::take(&mut queues.string_iterators);
            string_generator_marks
                .retain(|idx| !bits.string_iterators.get_bit(idx.get_index(), &bits.bits));
            string_generator_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.string_iterators.set_bit(index, &bits.bits) {
                    // Did mark.
                    string_iterators.get(index).mark_values(&mut queues);
                }
            });
            string_generator_marks.clear();
            if queues.string_iterators.is_empty() {
                queues.string_iterators = string_generator_marks;
            }
        }
        if !queues.strings.is_empty() {
            let mut string_marks = core::mem::take(&mut queues.strings);
            string_marks.retain(|idx| !bits.strings.get_bit(idx.get_index(), &bits.bits));
            string_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.strings.set_bit(index, &bits.bits) {
                    // Did mark.
                    strings.get(index).mark_values(&mut queues);
                }
            });
            string_marks.clear();
            if queues.strings.is_empty() {
                queues.strings = string_marks;
            }
        }
        if !queues.symbols.is_empty() {
            let mut symbol_marks = core::mem::take(&mut queues.symbols);
            symbol_marks.retain(|idx| !bits.symbols.get_bit(idx.get_index(), &bits.bits));
            symbol_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.symbols.set_bit(index, &bits.bits) {
                    // Did mark.
                    symbols.get(index).mark_values(&mut queues);
                }
            });
            symbol_marks.clear();
            if queues.symbols.is_empty() {
                queues.symbols = symbol_marks;
            }
        }
        #[cfg(feature = "array-buffer")]
        if !queues.typed_arrays.is_empty() {
            let mut typed_arrays_marks = core::mem::take(&mut queues.typed_arrays);
            typed_arrays_marks
                .retain(|idx| !bits.typed_arrays.get_bit(idx.get_index(), &bits.bits));
            typed_arrays_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.typed_arrays.set_bit(index, &bits.bits) {
                    // Did mark.
                    typed_arrays.get(index).mark_values(&mut queues);
                }
            });
            typed_arrays_marks.clear();
            if queues.typed_arrays.is_empty() {
                queues.typed_arrays = typed_arrays_marks;
            }
        }
        #[cfg(feature = "weak-refs")]
        {
            if !queues.weak_maps.is_empty() {
                let mut weak_map_marks = core::mem::take(&mut queues.weak_maps);
                weak_map_marks.retain(|idx| !bits.weak_maps.get_bit(idx.get_index(), &bits.bits));
                weak_map_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.weak_maps.set_bit(index, &bits.bits) {
                        // Did mark.
                        weak_maps.get(index).mark_values(&mut queues);
                    }
                });
                weak_map_marks.clear();
                if queues.weak_maps.is_empty() {
                    queues.weak_maps = weak_map_marks;
                }
            }
            if !queues.weak_refs.is_empty() {
                let mut weak_ref_marks = core::mem::take(&mut queues.weak_refs);
                weak_ref_marks.retain(|idx| !bits.weak_refs.get_bit(idx.get_index(), &bits.bits));
                weak_ref_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.weak_refs.set_bit(index, &bits.bits) {
                        // Did mark.
                        weak_refs.get(index).mark_values(&mut queues);
                    }
                });
                weak_ref_marks.clear();
                if queues.weak_refs.is_empty() {
                    queues.weak_refs = weak_ref_marks;
                }
            }
            if !queues.weak_sets.is_empty() {
                let mut weak_set_marks = core::mem::take(&mut queues.weak_sets);
                weak_set_marks.retain(|idx| !bits.weak_sets.get_bit(idx.get_index(), &bits.bits));
                weak_set_marks.iter().for_each(|&idx| {
                    let index = idx.get_index();
                    if bits.weak_sets.set_bit(index, &bits.bits) {
                        // Did mark.
                        weak_sets.get(index).mark_values(&mut queues);
                    }
                });
                weak_set_marks.clear();
                if queues.weak_sets.is_empty() {
                    queues.weak_sets = weak_set_marks;
                }
            }
        }

        if !queues.e_2_1.is_empty() {
            let mut e_2_1_marks = core::mem::take(&mut queues.e_2_1);
            e_2_1_marks.retain(|idx| !bits.e_2_1.get_bit(idx.get_index(), &bits.bits));
            e_2_1_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_1.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow1.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow1.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_1_marks.clear();
            if queues.e_2_1.is_empty() {
                queues.e_2_1 = e_2_1_marks;
            }
        }
        if !queues.e_2_2.is_empty() {
            let mut e_2_2_marks = core::mem::take(&mut queues.e_2_2);
            e_2_2_marks.retain(|idx| !bits.e_2_2.get_bit(idx.get_index(), &bits.bits));
            e_2_2_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_2.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow2.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow2.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_2_marks.clear();
            if queues.e_2_2.is_empty() {
                queues.e_2_2 = e_2_2_marks;
            }
        }
        if !queues.e_2_3.is_empty() {
            let mut e_2_3_marks = core::mem::take(&mut queues.e_2_3);
            e_2_3_marks.retain(|idx| !bits.e_2_3.get_bit(idx.get_index(), &bits.bits));
            e_2_3_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_3.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow3.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow3.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_3_marks.clear();
            if queues.e_2_3.is_empty() {
                queues.e_2_3 = e_2_3_marks;
            }
        }
        if !queues.e_2_4.is_empty() {
            let mut e_2_4_marks = core::mem::take(&mut queues.e_2_4);
            e_2_4_marks.retain(|idx| !bits.e_2_4.get_bit(idx.get_index(), &bits.bits));
            e_2_4_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_4.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow4.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow4.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_4_marks.clear();
            if queues.e_2_4.is_empty() {
                queues.e_2_4 = e_2_4_marks;
            }
        }
        if !queues.e_2_6.is_empty() {
            let mut e_2_6_marks = core::mem::take(&mut queues.e_2_6);
            e_2_6_marks.retain(|idx| !bits.e_2_6.get_bit(idx.get_index(), &bits.bits));
            e_2_6_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_6.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow6.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow6.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_6_marks.clear();
            if queues.e_2_6.is_empty() {
                queues.e_2_6 = e_2_6_marks;
            }
        }
        if !queues.e_2_8.is_empty() {
            let mut e_2_8_marks = core::mem::take(&mut queues.e_2_8);
            e_2_8_marks.retain(|idx| !bits.e_2_8.get_bit(idx.get_index(), &bits.bits));
            e_2_8_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_8.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow8.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow8.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_8_marks.clear();
            if queues.e_2_8.is_empty() {
                queues.e_2_8 = e_2_8_marks;
            }
        }
        if !queues.e_2_10.is_empty() {
            let mut e_2_10_marks = core::mem::take(&mut queues.e_2_10);
            e_2_10_marks.retain(|idx| !bits.e_2_10.get_bit(idx.get_index(), &bits.bits));
            e_2_10_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_10.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow10.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow10.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_10_marks.clear();
            if queues.e_2_10.is_empty() {
                queues.e_2_10 = e_2_10_marks;
            }
        }
        if !queues.e_2_12.is_empty() {
            let mut e_2_12_marks = core::mem::take(&mut queues.e_2_12);
            e_2_12_marks.retain(|idx| !bits.e_2_12.get_bit(idx.get_index(), &bits.bits));
            e_2_12_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_12.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow12.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow12.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_12_marks.clear();
            if queues.e_2_12.is_empty() {
                queues.e_2_12 = e_2_12_marks;
            }
        }
        if !queues.e_2_16.is_empty() {
            let mut e_2_16_marks = core::mem::take(&mut queues.e_2_16);
            e_2_16_marks.retain(|idx| !bits.e_2_16.get_bit(idx.get_index(), &bits.bits));
            e_2_16_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_16.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow16.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow16.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_16_marks.clear();
            if queues.e_2_16.is_empty() {
                queues.e_2_16 = e_2_16_marks;
            }
        }
        if !queues.e_2_24.is_empty() {
            let mut e_2_24_marks = core::mem::take(&mut queues.e_2_24);
            e_2_24_marks.retain(|idx| !bits.e_2_24.get_bit(idx.get_index(), &bits.bits));
            e_2_24_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_24.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow24.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow24.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_24_marks.clear();
            if queues.e_2_24.is_empty() {
                queues.e_2_24 = e_2_24_marks;
            }
        }
        if !queues.e_2_32.is_empty() {
            let mut e_2_32_marks = core::mem::take(&mut queues.e_2_32);
            e_2_32_marks.retain(|idx| !bits.e_2_32.get_bit(idx.get_index(), &bits.bits));
            e_2_32_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.e_2_32.set_bit(index, &bits.bits) {
                    if let Some(descriptors) = e2pow32.descriptors.get(&idx) {
                        mark_descriptors(descriptors, &mut queues);
                    }
                    e2pow32.values.get(index).mark_values(&mut queues);
                } else {
                    panic!("ElementsVector was not unique");
                }
            });
            e_2_32_marks.clear();
            if queues.e_2_32.is_empty() {
                queues.e_2_32 = e_2_32_marks;
            }
        }

        if !queues.k_2_4.is_empty() {
            let mut k_2_4_marks = core::mem::take(&mut queues.k_2_4);
            k_2_4_marks.retain(|idx| !bits.k_2_4.get_bit(idx.get_index(), &bits.bits));
            k_2_4_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_4.set_bit(index, &bits.bits) {
                    k2pow4.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_4_marks.clear();
            if queues.k_2_4.is_empty() {
                queues.k_2_4 = k_2_4_marks;
            }
        }
        if !queues.k_2_1.is_empty() {
            let mut k_2_1_marks = core::mem::take(&mut queues.k_2_1);
            k_2_1_marks.retain(|idx| !bits.k_2_1.get_bit(idx.get_index(), &bits.bits));
            k_2_1_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_1.set_bit(index, &bits.bits) {
                    k2pow1.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_1_marks.clear();
            if queues.k_2_1.is_empty() {
                queues.k_2_1 = k_2_1_marks;
            }
        }
        if !queues.k_2_2.is_empty() {
            let mut k_2_2_marks = core::mem::take(&mut queues.k_2_2);
            k_2_2_marks.retain(|idx| !bits.k_2_2.get_bit(idx.get_index(), &bits.bits));
            k_2_2_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_2.set_bit(index, &bits.bits) {
                    k2pow2.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_2_marks.clear();
            if queues.k_2_2.is_empty() {
                queues.k_2_2 = k_2_2_marks;
            }
        }
        if !queues.k_2_3.is_empty() {
            let mut k_2_3_marks = core::mem::take(&mut queues.k_2_3);
            k_2_3_marks.retain(|idx| !bits.k_2_3.get_bit(idx.get_index(), &bits.bits));
            k_2_3_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_3.set_bit(index, &bits.bits) {
                    k2pow3.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_3_marks.clear();
            if queues.k_2_3.is_empty() {
                queues.k_2_3 = k_2_3_marks;
            }
        }
        if !queues.k_2_6.is_empty() {
            let mut k_2_6_marks = core::mem::take(&mut queues.k_2_6);
            k_2_6_marks.retain(|idx| !bits.k_2_6.get_bit(idx.get_index(), &bits.bits));
            k_2_6_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_6.set_bit(index, &bits.bits) {
                    k2pow6.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_6_marks.clear();
            if queues.k_2_6.is_empty() {
                queues.k_2_6 = k_2_6_marks;
            }
        }
        if !queues.k_2_8.is_empty() {
            let mut k_2_8_marks = core::mem::take(&mut queues.k_2_8);
            k_2_8_marks.retain(|idx| !bits.k_2_8.get_bit(idx.get_index(), &bits.bits));
            k_2_8_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_8.set_bit(index, &bits.bits) {
                    k2pow8.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_8_marks.clear();
            if queues.k_2_8.is_empty() {
                queues.k_2_8 = k_2_8_marks;
            }
        }
        if !queues.k_2_10.is_empty() {
            let mut k_2_10_marks = core::mem::take(&mut queues.k_2_10);
            k_2_10_marks.retain(|idx| !bits.k_2_10.get_bit(idx.get_index(), &bits.bits));
            k_2_10_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_10.set_bit(index, &bits.bits) {
                    k2pow10.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_10_marks.clear();
            if queues.k_2_10.is_empty() {
                queues.k_2_10 = k_2_10_marks;
            }
        }
        if !queues.k_2_12.is_empty() {
            let mut k_2_12_marks = core::mem::take(&mut queues.k_2_12);
            k_2_12_marks.retain(|idx| !bits.k_2_12.get_bit(idx.get_index(), &bits.bits));
            k_2_12_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_12.set_bit(index, &bits.bits) {
                    k2pow12.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_12_marks.clear();
            if queues.k_2_12.is_empty() {
                queues.k_2_12 = k_2_12_marks;
            }
        }
        if !queues.k_2_16.is_empty() {
            let mut k_2_16_marks = core::mem::take(&mut queues.k_2_16);
            k_2_16_marks.retain(|idx| !bits.k_2_16.get_bit(idx.get_index(), &bits.bits));
            k_2_16_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_16.set_bit(index, &bits.bits) {
                    k2pow16.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_16_marks.clear();
            if queues.k_2_16.is_empty() {
                queues.k_2_16 = k_2_16_marks;
            }
        }
        if !queues.k_2_24.is_empty() {
            let mut k_2_24_marks = core::mem::take(&mut queues.k_2_24);
            k_2_24_marks.retain(|idx| !bits.k_2_24.get_bit(idx.get_index(), &bits.bits));
            k_2_24_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_24.set_bit(index, &bits.bits) {
                    k2pow24.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_24_marks.clear();
            if queues.k_2_24.is_empty() {
                queues.k_2_24 = k_2_24_marks;
            }
        }
        if !queues.k_2_32.is_empty() {
            let mut k_2_32_marks = core::mem::take(&mut queues.k_2_32);
            k_2_32_marks.retain(|idx| !bits.k_2_32.get_bit(idx.get_index(), &bits.bits));
            k_2_32_marks.iter().for_each(|&idx| {
                let index = idx.get_index();
                if bits.k_2_32.set_bit(index, &bits.bits) {
                    k2pow32.keys.get(index).mark_values(&mut queues)
                }
            });
            k_2_32_marks.clear();
            if queues.k_2_32.is_empty() {
                queues.k_2_32 = k_2_32_marks;
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
