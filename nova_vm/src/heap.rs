// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! # Nova engine heap structures
//!
//! This module contains internal details of how the Nova engine lays out its
//! heap. It is likely to go away in the future.

mod element_array;
mod heap_bits;
mod heap_constants;
mod heap_gc;
mod heap_size;
mod indexes;
mod object_entry;
pub(crate) mod soa;

pub use element_array::*;
pub(crate) use heap_bits::*;
pub(crate) use heap_constants::*;
pub(crate) use heap_gc::*;
pub(crate) use indexes::*;
pub(crate) use object_entry::*;

use core::cell::RefCell;

#[cfg(feature = "date")]
use crate::ecmascript::DateHeapData;
#[cfg(feature = "array-buffer")]
use crate::ecmascript::{
    ArrayBuffer, ArrayBufferHeapData, DataView, DataViewRecord, DetachKey, TypedArrayRecord,
    VoidArray,
};
#[cfg(feature = "temporal")]
use crate::ecmascript::{DurationRecord, InstantRecord, PlainTimeRecord};
#[cfg(feature = "regexp")]
use crate::ecmascript::{RegExpHeapData, RegExpStringIteratorRecord};
#[cfg(feature = "set")]
use crate::ecmascript::{SetHeapData, SetIteratorHeapData};
#[cfg(feature = "shared-array-buffer")]
use crate::ecmascript::{
    SharedArrayBufferRecord, SharedDataView, SharedDataViewRecord, SharedTypedArrayRecord,
    SharedVoidArray,
};
#[cfg(feature = "weak-refs")]
use crate::ecmascript::{WeakMapRecord, WeakRefHeapData, WeakSetHeapData};
#[cfg(feature = "array-buffer")]
use crate::hashing::AHashMap;
use crate::{
    ecmascript::{
        Agent, ArrayHeapData, ArrayIteratorHeapData, AsyncGeneratorHeapData, AwaitReactionRecord,
        BUILTIN_STRING_MEMORY, BUILTIN_STRINGS_LIST, BigIntHeapData, BoundFunctionHeapData,
        BuiltinConstructorRecord, BuiltinFunctionHeapData, Caches, ECMAScriptFunctionHeapData,
        EmbedderObjectHeapData, Environments, ErrorHeapData, FinalizationRegistryRecord,
        GeneratorHeapData, HeapString, MapHeapData, MapIteratorHeapData, ModuleHeapData,
        ModuleRequestRecord, NumberHeapData, ObjectRecord, ObjectShapeRecord,
        ObjectShapeTransitionMap, PrimitiveObjectRecord, PromiseFinallyFunctionHeapData,
        PromiseGroupRecord, PromiseHeapData, PromiseReactionRecord,
        PromiseResolvingFunctionHeapData, PrototypeShapeTable, ProxyHeapData, RealmRecord,
        ScriptRecord, SourceCodeHeapData, SourceTextModuleHeap, String, StringIteratorHeapData,
        StringRecord, SymbolHeapData,
    },
    engine::{ExecutableHeapData, HeapRootData},
};
use crate::table::HashTable;
use crate::heap::soa::{SoAVec, SoAble};
use wtf8::{Wtf8Buf, Wtf8Str};

pub(crate) struct Heap {
    #[cfg(feature = "array-buffer")]
    pub(crate) array_buffers: Vec<ArrayBufferHeapData<'static>>,
    #[cfg(feature = "array-buffer")]
    pub(crate) array_buffer_detach_keys: AHashMap<ArrayBuffer<'static>, DetachKey>,
    pub(crate) arrays: SoAVec<ArrayHeapData<'static>>,
    pub(crate) array_iterators: Vec<ArrayIteratorHeapData<'static>>,
    pub(crate) async_generators: Vec<AsyncGeneratorHeapData<'static>>,
    pub(crate) await_reactions: Vec<AwaitReactionRecord<'static>>,
    pub(crate) bigints: Vec<BigIntHeapData>,
    pub(crate) bound_functions: Vec<BoundFunctionHeapData<'static>>,
    pub(crate) builtin_constructors: Vec<BuiltinConstructorRecord<'static>>,
    pub(crate) builtin_functions: Vec<BuiltinFunctionHeapData<'static>>,
    pub(crate) caches: Caches<'static>,
    #[cfg(feature = "date")]
    pub(crate) dates: Vec<DateHeapData<'static>>,
    #[cfg(feature = "temporal")]
    pub(crate) instants: Vec<InstantRecord<'static>>,
    #[cfg(feature = "temporal")]
    pub(crate) durations: Vec<DurationRecord<'static>>,
    #[cfg(feature = "temporal")]
    pub(crate) plain_times: Vec<PlainTimeRecord<'static>>,
    pub(crate) ecmascript_functions: Vec<ECMAScriptFunctionHeapData<'static>>,
    /// ElementsArrays is where all keys and values arrays live;
    /// Element arrays are static arrays of Values plus
    /// a HashMap of possible property descriptors.
    pub(crate) elements: ElementArrays,
    pub(crate) embedder_objects: Vec<EmbedderObjectHeapData<'static>>,
    pub(crate) environments: Environments,
    pub(crate) errors: Vec<ErrorHeapData<'static>>,
    /// Stores compiled bytecodes
    pub(crate) executables: Vec<ExecutableHeapData<'static>>,
    pub(crate) finalization_registries: SoAVec<FinalizationRegistryRecord<'static>>,
    pub(crate) generators: Vec<GeneratorHeapData<'static>>,
    pub(crate) globals: RefCell<Vec<HeapRootData>>,
    /// Indexes of `globals` released by `Global::take`; an entry may be stale
    /// (out of bounds after the GC truncated `globals`, or filled again), so
    /// users check that the slot is empty.
    pub(crate) free_globals: RefCell<Vec<u32>>,
    pub(crate) maps: SoAVec<MapHeapData<'static>>,
    pub(crate) map_iterators: Vec<MapIteratorHeapData<'static>>,
    pub(crate) numbers: Vec<NumberHeapData>,
    pub(crate) object_shapes: Vec<ObjectShapeRecord<'static>>,
    pub(crate) object_shape_transitions: Vec<ObjectShapeTransitionMap<'static>>,
    pub(crate) prototype_shapes: PrototypeShapeTable,
    pub(crate) objects: Vec<ObjectRecord<'static>>,
    pub(crate) primitive_objects: Vec<PrimitiveObjectRecord<'static>>,
    pub(crate) promise_reaction_records: Vec<PromiseReactionRecord<'static>>,
    pub(crate) promise_resolving_functions: Vec<PromiseResolvingFunctionHeapData<'static>>,
    pub(crate) promise_finally_functions: Vec<PromiseFinallyFunctionHeapData<'static>>,
    pub(crate) promises: Vec<PromiseHeapData<'static>>,
    pub(crate) proxies: Vec<ProxyHeapData<'static>>,
    pub(crate) realms: Vec<RealmRecord<'static>>,
    pub(crate) promise_group_records: Vec<PromiseGroupRecord<'static>>,
    #[cfg(feature = "regexp")]
    pub(crate) regexps: Vec<RegExpHeapData<'static>>,
    #[cfg(feature = "regexp")]
    pub(crate) regexp_string_iterators: Vec<RegExpStringIteratorRecord<'static>>,
    #[cfg(feature = "set")]
    pub(crate) sets: SoAVec<SetHeapData<'static>>,
    #[cfg(feature = "set")]
    pub(crate) set_iterators: Vec<SetIteratorHeapData<'static>>,
    #[cfg(feature = "shared-array-buffer")]
    pub(crate) shared_array_buffers: Vec<SharedArrayBufferRecord<'static>>,
    pub(crate) symbols: Vec<SymbolHeapData<'static>>,
    #[cfg(feature = "array-buffer")]
    pub(crate) typed_arrays: Vec<TypedArrayRecord<'static>>,
    #[cfg(feature = "array-buffer")]
    pub(crate) typed_array_byte_lengths: AHashMap<VoidArray<'static>, usize>,
    #[cfg(feature = "array-buffer")]
    pub(crate) typed_array_byte_offsets: AHashMap<VoidArray<'static>, usize>,
    #[cfg(feature = "array-buffer")]
    pub(crate) typed_array_array_lengths: AHashMap<VoidArray<'static>, usize>,
    #[cfg(feature = "array-buffer")]
    pub(crate) data_views: Vec<DataViewRecord<'static>>,
    #[cfg(feature = "array-buffer")]
    pub(crate) data_view_byte_lengths: AHashMap<DataView<'static>, usize>,
    #[cfg(feature = "array-buffer")]
    pub(crate) data_view_byte_offsets: AHashMap<DataView<'static>, usize>,
    #[cfg(feature = "shared-array-buffer")]
    pub(crate) shared_typed_arrays: Vec<SharedTypedArrayRecord<'static>>,
    #[cfg(feature = "shared-array-buffer")]
    pub(crate) shared_typed_array_byte_lengths: AHashMap<SharedVoidArray<'static>, usize>,
    #[cfg(feature = "shared-array-buffer")]
    pub(crate) shared_typed_array_byte_offsets: AHashMap<SharedVoidArray<'static>, usize>,
    #[cfg(feature = "shared-array-buffer")]
    pub(crate) shared_typed_array_array_lengths: AHashMap<SharedVoidArray<'static>, usize>,
    #[cfg(feature = "shared-array-buffer")]
    pub(crate) shared_data_views: Vec<SharedDataViewRecord<'static>>,
    #[cfg(feature = "shared-array-buffer")]
    pub(crate) shared_data_view_byte_lengths: AHashMap<SharedDataView<'static>, usize>,
    #[cfg(feature = "shared-array-buffer")]
    pub(crate) shared_data_view_byte_offsets: AHashMap<SharedDataView<'static>, usize>,
    #[cfg(feature = "weak-refs")]
    pub(crate) weak_maps: Vec<WeakMapRecord<'static>>,
    #[cfg(feature = "weak-refs")]
    pub(crate) weak_refs: Vec<WeakRefHeapData<'static>>,
    #[cfg(feature = "weak-refs")]
    pub(crate) weak_sets: Vec<WeakSetHeapData<'static>>,
    pub(crate) modules: Vec<ModuleHeapData<'static>>,
    pub(crate) module_request_records: Vec<ModuleRequestRecord<'static>>,
    pub(crate) source_text_module_records: SourceTextModuleHeap,
    pub(crate) scripts: Vec<ScriptRecord<'static>>,
    pub(crate) string_iterators: Vec<StringIteratorHeapData<'static>>,
    // Parsed ASTs referred by functions must be dropped after functions.
    // These are held in the SourceCodeHeapData structs.
    pub(crate) source_codes: Vec<SourceCodeHeapData<'static>>,
    // But: Source code string data is in the string heap. We need to thus drop
    // the strings only after the source ASTs drop.
    pub(crate) strings: Vec<StringRecord>,
    pub(crate) string_lookup_table: HashTable<HeapString<'static>>,
    pub(crate) string_hasher: crate::hashing::TextHasher,
    /// Whether an object that has an intrinsic Object Shape (a prototype,
    /// %Object.prototype% and %Array.prototype% among them) was ever given
    /// a property under an integer key. While none was, an element added
    /// to an Array past its length is not looked for on its prototypes.
    pub(crate) intrinsic_integer_keys: bool,
    /// What appended strings may join, and have joined since the strings
    /// were last measured.
    pub(crate) text_join_budget: std::sync::Arc<crate::ecmascript::JoinBudget>,
    /// Random keys under which Object Shape transition tables hash their
    /// property keys by identity.
    pub(crate) shape_key_hasher: crate::hashing::HandleState,
    /// Counts allocations for garbage collection triggering.
    pub(crate) alloc_counter: usize,
    /// How many collections have run: what holds a handle that only a
    /// collection moves (the instruction loop, its executable) looks the
    /// handle up again when this has changed.
    pub(crate) collections: u64,
    /// How many times the keys of an intrinsic object's shape were moved
    /// where they are: such a shape is changed in place, so that it is the
    /// same shape with a key removed from among the others, or with private
    /// names put in front of them. A key added at the end moves none. What
    /// remembers the place of a key under such a shape
    /// ([`GlobalScopeSeen`](crate::ecmascript::GlobalScopeSeen)) is good
    /// while this has not changed.
    pub(crate) intrinsic_key_moves: u64,
    /// How many times an object whose shape is changed in place (an
    /// intrinsic object, or any object that is the prototype of another)
    /// has had a property added or removed, its properties moved, or its
    /// own prototype changed: every time what the property caches found
    /// through such an object is set right
    /// ([`Caches`](crate::ecmascript::Caches)). What was found by a walk of
    /// prototypes and is remembered apart from those caches
    /// ([`TypedArrayLengthSeen`](crate::ecmascript::TypedArrayLengthSeen))
    /// is good while this has not changed.
    pub(crate) intrinsic_shape_changes: u64,
    /// The shape last found for the objects of one kind that have nothing
    /// of their own: see [`BareShapeSeen`](crate::ecmascript::BareShapeSeen).
    /// Its handles are of the collection it counted, and are not read
    /// after another.
    pub(crate) bare_shape_seen: core::cell::Cell<
        Option<
            crate::ecmascript::BareShapeSeen<
                core::mem::Discriminant<crate::ecmascript::Value<'static>>,
            >,
        >,
    >,
    /// Where the elements of the TypedArrays last read or written are: see
    /// [`TypedArrayViewSeen`](crate::ecmascript::TypedArrayViewSeen). Its
    /// handles are of the collection it counted, and are not read after
    /// another.
    #[cfg(feature = "array-buffer")]
    pub(crate) typed_array_views:
        [crate::ecmascript::TypedArrayViewSeen; crate::ecmascript::TYPED_ARRAY_VIEWS_SEEN],
    /// How many times a property that an object already had was given a
    /// descriptor, or had the one it had taken away: the one way an
    /// accessor property becomes another accessor or a data property with
    /// the shape, the key and the place it had. And how many times the
    /// properties of an object were moved (one removed, private names put
    /// in front), which puts another property, with its descriptor, at a
    /// place. While this has not changed, the accessor functions of the
    /// property at a place of an object are the ones they were
    /// ([`AccessorSeen`](crate::ecmascript::AccessorSeen)).
    pub(crate) descriptor_changes: u64,
    /// Bytes of string data, ArrayBuffer data and keyed collection entries
    /// allocated since the last garbage collection, for the heap limit. Not
    /// part of the garbage collector's pacing.
    pub(crate) data_bytes_since_gc: usize,
    /// Allocation count after which the next garbage collection runs when no
    /// heap limit is near.
    pub(crate) gc_alloc_limit: usize,
}

pub(crate) trait CreateHeapData<T, F> {
    /// Creates a [`Value`] from the given data. Allocating the data is **not**
    /// guaranteed.
    fn create(&mut self, data: T) -> F;
}

impl CreateHeapData<&str, String<'static>> for Heap {
    fn create(&mut self, data: &str) -> String<'static> {
        if let Ok(value) = String::try_from(data) {
            value
        } else {
            self.alloc_str(data)
        }
    }
}

impl CreateHeapData<std::string::String, String<'static>> for Heap {
    fn create(&mut self, data: std::string::String) -> String<'static> {
        if let Ok(value) = String::try_from(data.as_str()) {
            value
        } else {
            self.alloc_string(data)
        }
    }
}

impl CreateHeapData<Wtf8Buf, String<'static>> for Heap {
    fn create(&mut self, data: Wtf8Buf) -> String<'static> {
        if let Ok(value) = String::try_from(data.as_wtf8()) {
            value
        } else {
            self.alloc_wtf8_buf(data)
        }
    }
}

impl Heap {
    pub(crate) fn new() -> Heap {
        let mut heap = Heap {
            #[cfg(feature = "array-buffer")]
            array_buffers: Vec::with_capacity(0),
            #[cfg(feature = "array-buffer")]
            array_buffer_detach_keys: crate::hashing::map_with_capacity(0),
            arrays: SoAVec::with_capacity(0).expect("Failed to allocate Heap"),
            array_iterators: Vec::with_capacity(0),
            async_generators: Vec::with_capacity(0),
            await_reactions: Vec::with_capacity(0),
            bigints: Vec::with_capacity(0),
            bound_functions: Vec::with_capacity(0),
            builtin_constructors: Vec::with_capacity(0),
            builtin_functions: Vec::with_capacity(0),
            caches: Caches::with_capacity(0),
            #[cfg(feature = "date")]
            dates: Vec::with_capacity(0),
            #[cfg(feature = "temporal")]
            instants: Vec::with_capacity(0),
            #[cfg(feature = "temporal")]
            durations: Vec::with_capacity(0),
            #[cfg(feature = "temporal")]
            plain_times: Vec::with_capacity(0),
            ecmascript_functions: Vec::with_capacity(0),
            elements: ElementArrays {
                e2pow1: ElementArray2Pow1::with_capacity(0),
                e2pow2: ElementArray2Pow2::with_capacity(0),
                e2pow3: ElementArray2Pow3::with_capacity(0),
                e2pow4: ElementArray2Pow4::with_capacity(0),
                e2pow6: ElementArray2Pow6::with_capacity(0),
                e2pow8: ElementArray2Pow8::default(),
                e2pow10: ElementArray2Pow10::default(),
                e2pow12: ElementArray2Pow12::default(),
                e2pow16: ElementArray2Pow16::default(),
                e2pow24: ElementArray2Pow24::default(),
                e2pow32: ElementArray2Pow32::default(),
                k2pow1: PropertyKeyArray2Pow1::with_capacity(0),
                k2pow2: PropertyKeyArray2Pow2::with_capacity(0),
                k2pow3: PropertyKeyArray2Pow3::with_capacity(0),
                k2pow4: PropertyKeyArray2Pow4::with_capacity(0),
                k2pow6: PropertyKeyArray2Pow6::with_capacity(0),
                k2pow8: PropertyKeyArray2Pow8::default(),
                k2pow10: PropertyKeyArray2Pow10::default(),
                k2pow12: PropertyKeyArray2Pow12::default(),
                k2pow16: PropertyKeyArray2Pow16::default(),
                k2pow24: PropertyKeyArray2Pow24::default(),
                k2pow32: PropertyKeyArray2Pow32::default(),
                key_offsets: Default::default(),
            },
            embedder_objects: Vec::with_capacity(0),
            environments: Default::default(),
            errors: Vec::with_capacity(0),
            executables: Vec::with_capacity(0),
            source_codes: Vec::with_capacity(0),
            finalization_registries: SoAVec::with_capacity(0).expect("Failed to allocate Heap"),
            generators: Vec::with_capacity(0),
            globals: RefCell::new(Vec::with_capacity(0)),
            free_globals: RefCell::new(Vec::new()),
            maps: SoAVec::with_capacity(0).expect("Failed to allocate Heap"),
            map_iterators: Vec::with_capacity(0),
            modules: Vec::with_capacity(0),
            module_request_records: Vec::with_capacity(0),
            numbers: Vec::with_capacity(0),
            object_shapes: Vec::with_capacity(0),
            object_shape_transitions: Vec::with_capacity(0),
            prototype_shapes: PrototypeShapeTable::with_capacity(0),
            objects: Vec::with_capacity(0),
            primitive_objects: Vec::with_capacity(0),
            promise_reaction_records: Vec::with_capacity(0),
            promise_resolving_functions: Vec::with_capacity(0),
            promise_finally_functions: Vec::with_capacity(0),
            promises: Vec::with_capacity(0),
            promise_group_records: Vec::with_capacity(0),
            proxies: Vec::with_capacity(0),
            realms: Vec::with_capacity(1),
            #[cfg(feature = "regexp")]
            regexps: Vec::with_capacity(0),
            #[cfg(feature = "regexp")]
            regexp_string_iterators: Vec::with_capacity(0),
            scripts: Vec::with_capacity(1),
            #[cfg(feature = "set")]
            sets: SoAVec::with_capacity(0).expect("Failed to allocate Heap"),
            #[cfg(feature = "set")]
            set_iterators: Vec::with_capacity(0),
            #[cfg(feature = "shared-array-buffer")]
            shared_array_buffers: Vec::with_capacity(0),
            source_text_module_records: SourceTextModuleHeap(Vec::with_capacity(0)),
            strings: Vec::with_capacity(0),
            string_iterators: Vec::with_capacity(0),
            string_lookup_table: HashTable::with_capacity(0),
            string_hasher: crate::hashing::TextHasher::new(),
            intrinsic_integer_keys: false,
            text_join_budget: Default::default(),
            shape_key_hasher: crate::hashing::HandleState::default(),
            symbols: Vec::with_capacity(0),
            #[cfg(feature = "array-buffer")]
            typed_arrays: Vec::with_capacity(0),
            #[cfg(feature = "array-buffer")]
            typed_array_byte_lengths: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "array-buffer")]
            typed_array_byte_offsets: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "array-buffer")]
            typed_array_array_lengths: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "array-buffer")]
            data_views: Vec::with_capacity(0),
            #[cfg(feature = "array-buffer")]
            data_view_byte_lengths: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "array-buffer")]
            data_view_byte_offsets: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "shared-array-buffer")]
            shared_typed_arrays: Vec::with_capacity(0),
            #[cfg(feature = "shared-array-buffer")]
            shared_typed_array_byte_lengths: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "shared-array-buffer")]
            shared_typed_array_byte_offsets: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "shared-array-buffer")]
            shared_typed_array_array_lengths: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "shared-array-buffer")]
            shared_data_views: Vec::with_capacity(0),
            #[cfg(feature = "shared-array-buffer")]
            shared_data_view_byte_lengths: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "shared-array-buffer")]
            shared_data_view_byte_offsets: crate::hashing::map_with_capacity(0),
            #[cfg(feature = "weak-refs")]
            weak_maps: Vec::with_capacity(0),
            #[cfg(feature = "weak-refs")]
            weak_refs: Vec::with_capacity(0),
            #[cfg(feature = "weak-refs")]
            weak_sets: Vec::with_capacity(0),
            alloc_counter: 0,
            collections: 0,
            intrinsic_key_moves: 0,
            intrinsic_shape_changes: 0,
            bare_shape_seen: core::cell::Cell::new(None),
            #[cfg(feature = "array-buffer")]
            typed_array_views: Default::default(),
            descriptor_changes: 0,
            data_bytes_since_gc: 0,
            gc_alloc_limit: crate::heap::heap_gc::MIN_GC_ALLOC_LIMIT,
        };

        const {
            assert!(BUILTIN_STRINGS_LIST.len() < u32::MAX as usize);
        }
        let strings = &mut heap.strings;
        let string_hasher = &mut heap.string_hasher;
        let string_lookup_table = &mut heap.string_lookup_table;
        for builtin_string in BUILTIN_STRINGS_LIST.iter() {
            let hash = string_hasher.text(builtin_string.as_bytes());
            let data = StringRecord::from_static_str(builtin_string, hash);
            String::insert_string(strings, string_lookup_table, string_hasher, data);
        }

        heap.symbols.extend_from_slice(&[
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_asyncIterator),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_hasInstance),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_isConcatSpreadable),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_iterator),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_match),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_matchAll),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_replace),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_search),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_species),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_split),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_toPrimitive),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_toStringTag),
            SymbolHeapData::new(BUILTIN_STRING_MEMORY.Symbol_unscopables),
        ]);

        // Set up the `{ __proto__: null }` shape; all null-proto objects are
        // children of this shape, regardless of their realm, so this is always
        // added in here.
        heap.object_shapes.push(ObjectShapeRecord::NULL);
        heap.object_shape_transitions
            .push(ObjectShapeTransitionMap::ROOT);

        heap
    }

    /// Allocate a borrowed string onto the Agent heap
    ///
    /// This method will hash the input and look for a matching string on the
    /// heap, and if found will return its HeapString instead of allocating a
    /// copy.
    ///
    /// # Correctness
    ///
    /// The string being allocated must not be representable as a
    /// SmallString. All SmallStrings must be kept on the stack to ensure that
    /// comparison between heap allocated strings and SmallStrings can be
    /// guaranteed to never equal true.
    pub(crate) fn alloc_str(&mut self, message: &str) -> String<'static> {
        let found = self.find_equal_string(message);
        match found {
            Ok(string) => string,
            Err(hash) => {
                let data = StringRecord::from_str(message, hash);
                self.create(data)
            }
        }
    }

    /// Allocate an owned string onto the Agent heap
    ///
    /// This method will hash the input and look for a matching string on the
    /// heap, and if found will return its HeapString instead of allocating a
    /// copy.
    ///
    /// # Correctness
    ///
    /// The string being allocated must not be representable as a
    /// SmallString. All SmallStrings must be kept on the stack to ensure that
    /// comparison between heap allocated strings and SmallStrings can be
    /// guaranteed to never equal true.
    fn alloc_string(&mut self, message: std::string::String) -> String<'static> {
        let found = self.find_equal_string(message.as_str());
        match found {
            Ok(string) => string,
            Err(hash) => {
                let data = StringRecord::from_string(message, hash);
                self.create(data)
            }
        }
    }

    /// Allocate an owned WTF-8 buffer onto the Agent heap
    ///
    /// This method will hash the input and look for a matching string on the
    /// heap, and if found will return its HeapString instead of allocating a
    /// copy.
    ///
    /// # Correctness
    ///
    /// The string being allocated must not be representable as a
    /// SmallString. All SmallStrings must be kept on the stack to ensure that
    /// comparison between heap allocated strings and SmallStrings can be
    /// guaranteed to never equal true.
    fn alloc_wtf8_buf(&mut self, message: Wtf8Buf) -> String<'static> {
        let found = self.find_equal_wtf8(message.as_wtf8());
        match found {
            Ok(string) => string,
            Err(hash) => {
                let data = StringRecord::from_wtf8_buf(message, hash);
                self.create(data)
            }
        }
    }

    /// The heap string with this text, which is too long for a SmallString:
    /// the one there is, or a new one. A text that the heap has already is
    /// not copied for the asking.
    pub(crate) fn alloc_wtf8(&mut self, message: Wtf8Str<'_>) -> String<'static> {
        match self.find_equal_wtf8(message) {
            Ok(string) => string,
            Err(hash) => {
                let mut text = Wtf8Buf::with_capacity(message.len());
                text.push_wtf8(message);
                let data = StringRecord::from_wtf8_buf(text, hash);
                self.create(data)
            }
        }
    }

    /// The heap string with the text of the bytes from `start` to `end` of
    /// the heap string `source`, which is too long for a SmallString: the
    /// one there is, or a new one. As [`Heap::alloc_wtf8`] for a text that
    /// lies in the heap itself, as a substring does: it is looked up where
    /// it lies, and copied only if no string has it yet.
    pub(crate) fn alloc_wtf8_slice(
        &mut self,
        source: HeapString,
        start: usize,
        end: usize,
    ) -> String<'static> {
        let index = source.get_index();
        match self.find_equal_wtf8(self.strings[index].as_wtf8().slice(start, end)) {
            Ok(string) => string,
            Err(hash) => {
                let message = self.strings[index].as_wtf8().slice(start, end);
                let mut text = Wtf8Buf::with_capacity(message.len());
                text.push_wtf8(message);
                let data = StringRecord::from_wtf8_buf(text, hash);
                self.create(data)
            }
        }
    }

    /// Allocates a long string that a concatenation made, as
    /// [`Heap::alloc_wtf8_buf`] does, marked so that appending to it again
    /// builds on it ([`Heap::alloc_appended`]).
    pub(crate) fn alloc_concatenation(&mut self, message: Wtf8Buf) -> String<'static> {
        let found = self.find_equal_wtf8(message.as_wtf8());
        match found {
            Ok(string) => string,
            Err(hash) => {
                let data = StringRecord::from_concatenation(message, hash);
                self.create(data)
            }
        }
    }

    /// Allocates the heap string `left` with `right` appended, as
    /// [`Heap::alloc_wtf8_buf`] would allocate their concatenation, without
    /// hashing `left` again, or copying it when it was itself built
    /// ([`StringRecord::appended`]).
    ///
    /// # Correctness
    ///
    /// As for [`Heap::alloc_wtf8_buf`]; `right` is not empty and does not
    /// start with a trail surrogate that the last code point of `left` would
    /// pair with.
    pub(crate) fn alloc_appended(
        &mut self,
        left: HeapString<'static>,
        right: String<'static>,
    ) -> String<'static> {
        // Read where it is: a copy of what is appended would be allocated
        // at every step of a loop that builds a text.
        let right = match &right {
            String::SmallString(right) => right.as_bytes(),
            String::String(right) => self.strings[right.get_index()].as_bytes(),
        };
        let data = StringRecord::appended(
            &self.strings[left.get_index()],
            right,
            &self.string_hasher,
            &self.text_join_budget,
        );
        self.find_or_create_built(data)
    }

    /// Allocates the heap string `right` with `left` put before it, as
    /// [`Heap::alloc_wtf8_buf`] would allocate their concatenation, without
    /// hashing `right` again, or copying it when it was itself built
    /// ([`StringRecord::prepended`]).
    ///
    /// # Correctness
    ///
    /// As for [`Heap::alloc_wtf8_buf`]; `left` is not empty, is not a built
    /// text itself, and does not end with a lead surrogate that the first
    /// code point of `right` would pair with.
    pub(crate) fn alloc_prepended(
        &mut self,
        left: String<'static>,
        right: HeapString<'static>,
    ) -> String<'static> {
        let (left, left_hash) = match &left {
            String::SmallString(left) => {
                (left.as_bytes(), self.string_hasher.text(left.as_bytes()))
            }
            String::String(left) => {
                let left = &self.strings[left.get_index()];
                (left.as_bytes(), left.text_hash(&self.string_hasher))
            }
        };
        let data = StringRecord::prepended(
            left,
            left_hash,
            &self.strings[right.get_index()],
            &self.string_hasher,
            &self.text_join_budget,
        );
        self.find_or_create_built(data)
    }

    /// Allocates the heap string of `left` and then `right`, as
    /// [`Heap::alloc_wtf8_buf`] would allocate their concatenation: its hash
    /// follows from that of `left`, and the two are put together only where
    /// the heap has no such string yet. A long one is marked as
    /// [`Heap::alloc_concatenation`] marks it.
    ///
    /// # Correctness
    ///
    /// As for [`Heap::alloc_wtf8_buf`]; no surrogate pair is joined across
    /// the two.
    pub(crate) fn alloc_joined(
        &mut self,
        left: String<'static>,
        right: String<'static>,
        long: usize,
    ) -> String<'static> {
        let hasher = &self.string_hasher;
        let (left, left_hash) = match &left {
            String::SmallString(left) => (left.as_bytes(), hasher.text(left.as_bytes())),
            String::String(left) => {
                let left = &self.strings[left.get_index()];
                (left.as_bytes(), left.text_hash(hasher))
            }
        };
        let (right, hash) = match &right {
            String::SmallString(right) => (
                right.as_bytes(),
                hasher.appended(left_hash, right.as_bytes()),
            ),
            String::String(right) => {
                let right = &self.strings[right.get_index()];
                let bytes = right.as_bytes();
                // Read for the hash while that is cheaper than the power
                // of the point that its own hash would be joined by.
                let hash = if bytes.len() < 256 {
                    hasher.appended(left_hash, bytes)
                } else {
                    hasher.joined(left_hash, right.text_hash(hasher), bytes.len())
                };
                (bytes, hash)
            }
        };
        let found = self
            .string_lookup_table
            .find(hasher.table_hash(hash), |heap_string| {
                self.strings[heap_string.get_index()].text_is_both(left, right)
            });
        if let Some(&found) = found {
            return found.into();
        }
        let len = left.len() + right.len();
        let mut text = Wtf8Buf::with_capacity(len);
        text.push_wtf8(Wtf8Str::from_wtf8_bytes(left));
        text.push_wtf8(Wtf8Str::from_wtf8_bytes(right));
        let data = if len >= long {
            StringRecord::from_concatenation(text, hash)
        } else {
            StringRecord::from_wtf8_buf(text, hash)
        };
        self.create(data)
    }

    /// The string of a built text: the one the heap has, or a new one.
    fn find_or_create_built(&mut self, data: StringRecord) -> String<'static> {
        // Neither text is joined to compare them: a loop that builds the
        // same text again finds, at every step, the string it built before.
        let hash = data.table_hash(&self.string_hasher);
        let found = self.string_lookup_table.find(hash, |heap_string| {
            let record = &self.strings[heap_string.get_index()];
            record.len() == data.len() && record.same_text(&data)
        });
        if let Some(&found) = found {
            return found.into();
        }
        self.create(data)
    }

    /// Allocate a static string onto the Agent heap
    ///
    /// This method will currently iterate through all heap strings to look for
    /// a possible matching string and if found will return its HeapString
    /// instead of allocating a copy.
    ///
    /// # Correctness
    ///
    /// The string being allocated must not be representable as a
    /// SmallString. All SmallStrings must be kept on the stack to ensure that
    /// comparison between heap allocated strings and SmallStrings can be
    /// guaranteed to never equal true.
    pub(crate) fn alloc_static_str(&mut self, message: &'static str) -> String<'static> {
        let found = self.find_equal_string(message);
        match found {
            Ok(string) => string,
            Err(hash) => {
                let data = StringRecord::from_static_str(message, hash);
                self.create(data)
            }
        }
    }

    /// Find existing heap String or return the value of the text.
    fn find_equal_string(&self, message: &str) -> Result<String<'static>, u64> {
        let message = Wtf8Str::from_str(message);
        self.find_equal_wtf8(message)
    }

    /// Find existing heap String or return the value of the text
    /// ([`crate::hashing::TextHasher::text`]).
    fn find_equal_wtf8(&self, message: Wtf8Str<'_>) -> Result<String<'static>, u64> {
        debug_assert!(message.len() > 7);
        let hash = self.string_hasher.text(message.as_bytes());
        self.string_lookup_table
            .find(self.string_hasher.table_hash(hash), |heap_string| {
                let record = &self.strings[heap_string.get_index()];
                record.len() == message.len() && record.text_is(message.as_bytes())
            })
            .map(|&heap_string| heap_string.into())
            .ok_or(hash)
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) trait DirectArenaAccess: HeapIndexHandle {
    type Data: 'static;
    type Output;

    /// Access arena data belonging to a handle.
    #[allow(clippy::ptr_arg)]
    fn get_direct(self, agent: &Vec<Self::Data>) -> &Self::Output;
}

pub(crate) trait DirectArenaAccessMut: DirectArenaAccess {
    /// Access arena data belonging to a handle as mutable.
    ///
    /// The record is returned as stored, with its `'static` GC lifetime:
    /// handles written into it must be unbound first.
    fn get_direct_mut(self, agent: &mut Vec<Self::Data>) -> &mut Self::Data;
}

pub(crate) trait ArenaAccess<T>: DirectArenaAccess {
    /// Access data belonging to a handle.
    fn get(self, agent: &T) -> &<Self as DirectArenaAccess>::Output;
}

pub(crate) trait ArenaAccessMut<T>: DirectArenaAccessMut {
    /// Access data belonging to a handle as mutable.
    ///
    /// The record is returned as stored, with its `'static` GC lifetime:
    /// handles written into it must be unbound first.
    fn get_mut(self, agent: &mut T) -> &mut <Self as DirectArenaAccess>::Data;
}

impl<K: DirectArenaAccess, T> ArenaAccess<T> for K
where
    T: AsRef<Vec<K::Data>>,
{
    /// Access data belonging to a handle.
    #[inline]
    fn get<'agent>(self, agent: &'agent T) -> &'agent <Self as DirectArenaAccess>::Output {
        self.get_direct(agent.as_ref())
    }
}

impl<K: DirectArenaAccessMut, T> ArenaAccessMut<T> for K
where
    T: AsMut<Vec<K::Data>>,
{
    /// Access data belonging to a handle as mutable.
    #[inline]
    fn get_mut<'agent>(
        self,
        agent: &'agent mut T,
    ) -> &'agent mut <Self as DirectArenaAccess>::Data {
        self.get_direct_mut(agent.as_mut())
    }
}

pub(crate) trait DirectArenaAccessSoA: HeapIndexHandle {
    type Data: SoAble;

    /// Access arena data belonging to a handle.
    fn get_direct(self, source: &SoAVec<Self::Data>) -> <Self::Data as SoAble>::Ref<'_>;
}

pub(crate) trait DirectArenaAccessSoAMut: DirectArenaAccessSoA {
    /// Access arena data belonging to a handle as mutable.
    fn get_direct_mut<'agent>(
        self,
        source: &'agent mut SoAVec<Self::Data>,
    ) -> <Self::Data as SoAble>::Mut<'agent>;
}

pub(crate) trait ArenaAccessSoA<T>: DirectArenaAccessSoA {
    /// Access data belonging to a handle.
    fn get<'a>(self, agent: &'a T) -> <Self::Data as SoAble>::Ref<'a>;
}

pub(crate) trait ArenaAccessSoAMut<T>: DirectArenaAccessSoAMut {
    /// Access data belonging to a handle as mutable.
    fn get_mut<'a>(self, agent: &'a mut T) -> <Self::Data as SoAble>::Mut<'a>;
}

impl<K: DirectArenaAccessSoA, T> ArenaAccessSoA<T> for K
where
    T: AsRef<SoAVec<K::Data>>,
{
    /// Access data belonging to a handle.
    #[inline]
    fn get<'a>(self, agent: &'a T) -> <Self::Data as SoAble>::Ref<'a> {
        self.get_direct(agent.as_ref())
    }
}

impl<K: DirectArenaAccessSoAMut, T> ArenaAccessSoAMut<T> for K
where
    T: AsMut<SoAVec<K::Data>>,
{
    /// Access data belonging to a handle as mutable.
    #[inline]
    fn get_mut<'a>(self, agent: &'a mut T) -> <Self::Data as SoAble>::Mut<'a> {
        self.get_direct_mut(agent.as_mut())
    }
}

/// A partial view to the Agent's heap that allows accessing primitive value
/// heap data.
pub(crate) struct PrimitiveHeap<'a> {
    pub(crate) bigints: &'a Vec<BigIntHeapData>,
    pub(crate) numbers: &'a Vec<NumberHeapData>,
    pub(crate) strings: &'a Vec<StringRecord>,
}

impl AsRef<Vec<BigIntHeapData>> for PrimitiveHeap<'_> {
    #[inline]
    fn as_ref(&self) -> &Vec<BigIntHeapData> {
        self.bigints
    }
}

impl AsRef<Vec<NumberHeapData>> for PrimitiveHeap<'_> {
    #[inline]
    fn as_ref(&self) -> &Vec<NumberHeapData> {
        self.numbers
    }
}

impl AsRef<Vec<StringRecord>> for PrimitiveHeap<'_> {
    #[inline]
    fn as_ref(&self) -> &Vec<StringRecord> {
        self.strings
    }
}

impl PrimitiveHeap<'_> {
    pub(crate) fn new<'a>(
        bigints: &'a Vec<BigIntHeapData>,
        numbers: &'a Vec<NumberHeapData>,
        strings: &'a Vec<StringRecord>,
    ) -> PrimitiveHeap<'a> {
        PrimitiveHeap {
            bigints,
            numbers,
            strings,
        }
    }
}

/// Helper trait for primitive heap data indexing.
pub(crate) trait PrimitiveHeapAccess:
    StringHeapAccess + NumberHeapAccess + AsRef<Vec<BigIntHeapData>>
{
}

impl PrimitiveHeapAccess for PrimitiveHeap<'_> {}

/// A partial view to the Agent's heap that allows accessing PropertyKey heap
/// data.
pub(crate) struct PropertyKeyHeap<'a> {
    pub(crate) strings: &'a mut Vec<StringRecord>,
    pub(crate) symbols: &'a mut Vec<SymbolHeapData<'static>>,
}

impl PropertyKeyHeap<'_> {
    pub(crate) fn new<'a>(
        strings: &'a mut Vec<StringRecord>,
        symbols: &'a mut Vec<SymbolHeapData<'static>>,
    ) -> PropertyKeyHeap<'a> {
        PropertyKeyHeap { strings, symbols }
    }
}

impl AsRef<Vec<StringRecord>> for PropertyKeyHeap<'_> {
    #[inline]
    fn as_ref(&self) -> &Vec<StringRecord> {
        self.strings
    }
}

impl AsRef<Vec<SymbolHeapData<'static>>> for PropertyKeyHeap<'_> {
    #[inline]
    fn as_ref(&self) -> &Vec<SymbolHeapData<'static>> {
        self.symbols
    }
}

/// Helper trait for primitive heap data indexing.
pub(crate) trait PropertyKeyHeapAccess:
    StringHeapAccess + AsRef<Vec<StringRecord>> + AsRef<Vec<SymbolHeapData<'static>>>
{
}

impl PropertyKeyHeapAccess for PropertyKeyHeap<'_> {}
impl PropertyKeyHeapAccess for Agent {}

pub(crate) trait StringHeapAccess: AsRef<Vec<StringRecord>> {}

impl StringHeapAccess for Vec<StringRecord> {}
impl StringHeapAccess for PrimitiveHeap<'_> {}
impl StringHeapAccess for PropertyKeyHeap<'_> {}
impl StringHeapAccess for Agent {}

pub(crate) trait NumberHeapAccess: AsRef<Vec<NumberHeapData>> {}

impl NumberHeapAccess for Vec<NumberHeapData> {}
impl NumberHeapAccess for PrimitiveHeap<'_> {}
impl NumberHeapAccess for Agent {}

#[test]
fn init_heap() {
    let _ = Heap::new();
}

macro_rules! arena_vec_access {
    (soa: $name: ident, $lt: lifetime, $data: ident, $member: ident, $output_ref: ident, $output_mut: ident) => {
        #[doc(hidden)]
        impl<$lt> crate::heap::DirectArenaAccessSoA for $name<$lt> {
            type Data = $data<'static>;

            #[inline]
            fn get_direct(
                self,
                source: &crate::heap::soa::SoAVec<Self::Data>,
            ) -> <Self::Data as crate::heap::soa::SoAble>::Ref<'_> {
                source
                    .get(self.get_index_u32())
                    .unwrap_or_else(|| panic!("Invalid handle {:?}", self))
            }
        }

        #[doc(hidden)]
        impl<$lt> crate::heap::DirectArenaAccessSoAMut for $name<$lt> {
            #[inline]
            fn get_direct_mut<'agent>(
                self,
                source: &'agent mut crate::heap::soa::SoAVec<Self::Data>,
            ) -> <Self::Data as crate::heap::soa::SoAble>::Mut<'agent> {
                source
                    .get_mut(self.get_index_u32())
                    .unwrap_or_else(|| panic!("Invalid handle {:?}", self))
            }
        }

        #[doc(hidden)]
        impl AsRef<crate::heap::soa::SoAVec<$data<'static>>> for crate::ecmascript::Agent {
            #[inline(always)]
            fn as_ref(&self) -> &crate::heap::soa::SoAVec<$data<'static>> {
                &self.heap.$member
            }
        }

        #[doc(hidden)]
        impl AsMut<crate::heap::soa::SoAVec<$data<'static>>> for crate::ecmascript::Agent {
            #[inline(always)]
            fn as_mut(&mut self) -> &mut crate::heap::soa::SoAVec<$data<'static>> {
                &mut self.heap.$member
            }
        }
    };
    ($name: ident, $lt: lifetime, $data: ident, $member: ident) => {
        #[doc(hidden)]
        impl<$lt> crate::heap::DirectArenaAccess for $name<$lt> {
            type Data = $data<'static>;
            type Output = $data<$lt>;

            #[inline]
            fn get_direct(self, source: &Vec<Self::Data>) -> &Self::Output {
                source
                    .get(crate::heap::HeapIndexHandle::get_index(self))
                    .unwrap_or_else(|| panic!("Invalid handle {:?}", self))
            }
        }

        #[doc(hidden)]
        impl<$lt> crate::heap::DirectArenaAccessMut for $name<$lt> {
            #[inline]
            fn get_direct_mut(self, source: &mut Vec<Self::Data>) -> &mut Self::Data {
                source
                    .get_mut(crate::heap::HeapIndexHandle::get_index(self))
                    .unwrap_or_else(|| panic!("Invalid handle {:?}", self))
            }
        }

        #[doc(hidden)]
        impl AsRef<Vec<$data<'static>>> for crate::ecmascript::Agent {
            #[inline(always)]
            fn as_ref(&self) -> &Vec<$data<'static>> {
                &self.heap.$member
            }
        }

        #[doc(hidden)]
        impl AsMut<Vec<$data<'static>>> for crate::ecmascript::Agent {
            #[inline(always)]
            fn as_mut(&mut self) -> &mut Vec<$data<'static>> {
                &mut self.heap.$member
            }
        }
    };
    ($name: ident, $data: ty, $member: ident, $output: ty) => {
        #[doc(hidden)]
        impl crate::heap::DirectArenaAccess for $name<'_> {
            type Data = $data;
            type Output = $output;

            #[inline]
            fn get_direct(self, source: &Vec<Self::Data>) -> &Self::Output {
                source
                    .get(crate::heap::HeapIndexHandle::get_index(self))
                    .unwrap_or_else(|| panic!("Invalid handle {:?}", self))
            }
        }

        #[doc(hidden)]
        impl crate::heap::DirectArenaAccessMut for $name<'_> {
            #[inline]
            fn get_direct_mut(self, source: &mut Vec<Self::Data>) -> &mut Self::Data {
                source
                    .get_mut(crate::heap::HeapIndexHandle::get_index(self))
                    .unwrap_or_else(|| panic!("Invalid handle {:?}", self))
            }
        }

        #[doc(hidden)]
        impl AsRef<Vec<$data>> for crate::ecmascript::Agent {
            #[inline(always)]
            fn as_ref(&self) -> &Vec<$data> {
                &self.heap.$member
            }
        }

        #[doc(hidden)]
        impl AsMut<Vec<$data>> for crate::ecmascript::Agent {
            #[inline(always)]
            fn as_mut(&mut self) -> &mut Vec<$data> {
                &mut self.heap.$member
            }
        }
    };
}

pub(crate) use arena_vec_access;
