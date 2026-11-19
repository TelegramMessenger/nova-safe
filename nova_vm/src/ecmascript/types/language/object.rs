// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod data;
mod internal_methods;
mod internal_slots;
mod property_key;
mod property_key_set;
mod property_key_vec;
mod property_storage;

pub(crate) use data::*;
pub use internal_methods::*;
pub use internal_slots::*;
pub use property_key::*;
pub(crate) use property_key_set::*;
pub(crate) use property_key_vec::*;
pub(crate) use property_storage::*;

use crate::ecmascript::{
    ARGUMENTS_DISCRIMINANT, ARRAY_DISCRIMINANT, ARRAY_ITERATOR_DISCRIMINANT,
    ASYNC_GENERATOR_DISCRIMINANT, BOUND_FUNCTION_DISCRIMINANT,
    BUILTIN_CONSTRUCTOR_FUNCTION_DISCRIMINANT, BUILTIN_FUNCTION_DISCRIMINANT,
    BUILTIN_PROMISE_FINALLY_FUNCTION_DISCRIMINANT, BUILTIN_PROMISE_RESOLVING_FUNCTION_DISCRIMINANT,
    BUILTIN_PROXY_REVOKER_FUNCTION, ECMASCRIPT_FUNCTION_DISCRIMINANT, EMBEDDER_OBJECT_DISCRIMINANT,
    ERROR_DISCRIMINANT, FINALIZATION_REGISTRY_DISCRIMINANT, Function, GENERATOR_DISCRIMINANT,
    MAP_DISCRIMINANT, MAP_ITERATOR_DISCRIMINANT, MODULE_DISCRIMINANT, OBJECT_DISCRIMINANT,
    PRIMITIVE_OBJECT_DISCRIMINANT, PROMISE_DISCRIMINANT, PROXY_DISCRIMINANT,
    STRING_ITERATOR_DISCRIMINANT, UnmappedArguments, Value,
};
#[cfg(feature = "array-buffer")]
use crate::ecmascript::{
    ARRAY_BUFFER_DISCRIMINANT, ArrayBuffer, BIGINT_64_ARRAY_DISCRIMINANT,
    BIGUINT_64_ARRAY_DISCRIMINANT, BigInt64Array, BigUint64Array, DATA_VIEW_DISCRIMINANT, DataView,
    FLOAT_32_ARRAY_DISCRIMINANT, FLOAT_64_ARRAY_DISCRIMINANT, Float32Array, Float64Array,
    INT_8_ARRAY_DISCRIMINANT, INT_16_ARRAY_DISCRIMINANT, INT_32_ARRAY_DISCRIMINANT, Int8Array,
    Int16Array, Int32Array, UINT_8_ARRAY_DISCRIMINANT, UINT_8_CLAMPED_ARRAY_DISCRIMINANT,
    UINT_16_ARRAY_DISCRIMINANT, UINT_32_ARRAY_DISCRIMINANT, Uint8Array, Uint8ClampedArray,
    Uint16Array, Uint32Array,
};
#[cfg(feature = "date")]
use crate::ecmascript::{DATE_DISCRIMINANT, Date};
#[cfg(feature = "temporal")]
use crate::ecmascript::{
    DURATION_DISCRIMINANT, INSTANT_DISCRIMINANT, PLAIN_TIME_DISCRIMINANT, TemporalDuration,
    TemporalInstant, TemporalPlainTime,
};
#[cfg(feature = "proposal-float16array")]
use crate::ecmascript::{FLOAT_16_ARRAY_DISCRIMINANT, Float16Array};
#[cfg(feature = "regexp")]
use crate::ecmascript::{
    REGEXP_DISCRIMINANT, REGEXP_STRING_ITERATOR_DISCRIMINANT, RegExp, RegExpStringIterator,
};
#[cfg(feature = "set")]
use crate::ecmascript::{SET_DISCRIMINANT, SET_ITERATOR_DISCRIMINANT, Set, SetIterator};
#[cfg(feature = "shared-array-buffer")]
use crate::ecmascript::{
    SHARED_ARRAY_BUFFER_DISCRIMINANT, SHARED_BIGINT_64_ARRAY_DISCRIMINANT,
    SHARED_BIGUINT_64_ARRAY_DISCRIMINANT, SHARED_DATA_VIEW_DISCRIMINANT,
    SHARED_FLOAT_32_ARRAY_DISCRIMINANT, SHARED_FLOAT_64_ARRAY_DISCRIMINANT,
    SHARED_INT_8_ARRAY_DISCRIMINANT, SHARED_INT_16_ARRAY_DISCRIMINANT,
    SHARED_INT_32_ARRAY_DISCRIMINANT, SHARED_UINT_8_ARRAY_DISCRIMINANT,
    SHARED_UINT_8_CLAMPED_ARRAY_DISCRIMINANT, SHARED_UINT_16_ARRAY_DISCRIMINANT,
    SHARED_UINT_32_ARRAY_DISCRIMINANT, SharedArrayBuffer, SharedBigInt64Array,
    SharedBigUint64Array, SharedDataView, SharedFloat32Array, SharedFloat64Array, SharedInt8Array,
    SharedInt16Array, SharedInt32Array, SharedUint8Array, SharedUint8ClampedArray,
    SharedUint16Array, SharedUint32Array,
};
#[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
use crate::ecmascript::{SHARED_FLOAT_16_ARRAY_DISCRIMINANT, SharedFloat16Array};
#[cfg(feature = "weak-refs")]
use crate::ecmascript::{WEAK_MAP_DISCRIMINANT, WEAK_REF_DISCRIMINANT, WEAK_SET_DISCRIMINANT};
#[cfg(feature = "weak-refs")]
use crate::ecmascript::{WeakMap, WeakRef, WeakSet};
use crate::{
    ecmascript::{
        Agent, ArgumentsList, Array, ArrayIterator, AsyncGenerator, BoundFunction,
        BuiltinConstructorFunction, BuiltinFunction, BuiltinPromiseFinallyFunction,
        BuiltinPromiseResolvingFunction, ECMAScriptFunction, EmbedderObject, Error,
        FinalizationRegistry, Generator, JsResult, Map, MapIterator, Module, ObjectShape,
        ObjectShapeRecord, PrimitiveObject, Promise, PropertyDescriptor, PropertyLookupCache,
        PropertyOffset, ProtoIntrinsics, Proxy, StringIterator, TryResult,
        ordinary_object_create_with_intrinsics,
    },
    engine::{Bindable, GcScope, HeapRootData, NoGcScope},
    heap::{
        ArenaAccess, ArenaAccessMut, BaseIndex, CompactionLists, CreateHeapData, DirectArenaAccess,
        Heap, HeapMarkAndSweep, HeapSweepWeakReference, IntrinsicConstructorIndexes,
        IntrinsicObjectIndexes, IntrinsicPrimitiveObjectIndexes, ObjectEntry, WorkQueues,
        arena_vec_access,
        {
            ElementDescriptor, ElementStorageMut, ElementStorageRef, ElementStorageUninit,
            ElementsVector, PropertyStorageMut, PropertyStorageRef,
        },
    },
};

use crate::hashing::AHashMap;
use core::hash::Hash;
use std::collections::TryReserveError;

/// ### [6.1.7 The Object Type](https://tc39.es/ecma262/#sec-object-type)
///
/// In Nova
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Bindable)]
#[repr(u8)]
pub enum Object<'a> {
    /// ### [6.1.7 The Object Type](https://tc39.es/ecma262/#sec-object-type)
    Object(OrdinaryObject<'a>) = OBJECT_DISCRIMINANT,
    /// ### [10.4.1 Bound Function Exotic Objects](https://tc39.es/ecma262/#sec-bound-function-exotic-objects)
    BoundFunction(BoundFunction<'a>) = BOUND_FUNCTION_DISCRIMINANT,
    /// ## [10.3 Built-in Function Objects](https://tc39.es/ecma262/#sec-built-in-function-objects)
    BuiltinFunction(BuiltinFunction<'a>) = BUILTIN_FUNCTION_DISCRIMINANT,
    /// ## [10.2 ECMAScript Function Objects](https://tc39.es/ecma262/#sec-ecmascript-function-objects)
    ECMAScriptFunction(ECMAScriptFunction<'a>) = ECMASCRIPT_FUNCTION_DISCRIMINANT,
    /// ### [4.4.36 built-in constructor](https://tc39.es/ecma262/#sec-built-in-constructor)
    ///
    /// A class built-in default constructor created in step 14 of
    /// ClassDefinitionEvaluation.
    BuiltinConstructorFunction(BuiltinConstructorFunction<'a>) =
        BUILTIN_CONSTRUCTOR_FUNCTION_DISCRIMINANT,
    /// Special built-in functions created to resolve or reject native [`Promise`]
    /// objects.
    ///
    /// [`Promise`]: crate::ecmascript::Promise
    BuiltinPromiseResolvingFunction(BuiltinPromiseResolvingFunction<'a>) =
        BUILTIN_PROMISE_RESOLVING_FUNCTION_DISCRIMINANT,
    /// Special functions created as part of `Promise.prototype.finally`.
    BuiltinPromiseFinallyFunction(BuiltinPromiseFinallyFunction<'a>) =
        BUILTIN_PROMISE_FINALLY_FUNCTION_DISCRIMINANT,
    /// Placeholder.
    BuiltinProxyRevokerFunction = BUILTIN_PROXY_REVOKER_FUNCTION,
    /// Primitive objects are special objects that hold a primitive value in their
    /// internal data.
    PrimitiveObject(PrimitiveObject<'a>) = PRIMITIVE_OBJECT_DISCRIMINANT,
    /// ### [10.4.4 Arguments Exotic Objects](https://tc39.es/ecma262/#sec-arguments-exotic-objects)
    ///
    /// An unmapped arguments object is an ordinary object with an additional
    /// internal slot \[\[ParameterMap]] whose value is always **undefined**.
    Arguments(UnmappedArguments<'a>) = ARGUMENTS_DISCRIMINANT,
    /// ### [10.4.2 Array Exotic Objects](https://tc39.es/ecma262/#sec-array-exotic-objects)
    Array(Array<'a>) = ARRAY_DISCRIMINANT,
    #[cfg(feature = "date")]
    /// ## [21.4 Date Objects](https://tc39.es/ecma262/#sec-date-objects)
    Date(Date<'a>) = DATE_DISCRIMINANT,
    #[cfg(feature = "temporal")]
    /// # [8 Temporal.Instant Objects](https://tc39.es/proposal-temporal/#sec-temporal-instant-objects)
    Instant(TemporalInstant<'a>) = INSTANT_DISCRIMINANT,
    #[cfg(feature = "temporal")]
    /// # [7 Temporal.Duration Objects](https://tc39.es/proposal-temporal/#sec-temporal-duration-objects)
    Duration(TemporalDuration<'a>) = DURATION_DISCRIMINANT,
    #[cfg(feature = "temporal")]
    /// # [4 Temporal.PlainTime Objects](https://tc39.es/proposal-temporal/#sec-temporal-plaintime-objects)
    PlainTime(TemporalPlainTime<'a>) = PLAIN_TIME_DISCRIMINANT,
    /// ## [20.5 Error Objects](https://tc39.es/ecma262/#sec-error-objects)
    Error(Error<'a>) = ERROR_DISCRIMINANT,
    /// ## [26.2 FinalizationRegistry Objects](https://tc39.es/ecma262/#sec-finalization-registry-objects)
    FinalizationRegistry(FinalizationRegistry<'a>) = FINALIZATION_REGISTRY_DISCRIMINANT,
    /// ## [24.1 Map Objects](https://tc39.es/ecma262/#sec-map-objects)
    Map(Map<'a>) = MAP_DISCRIMINANT,
    /// ## [27.2 Promise Objects](https://tc39.es/ecma262/#sec-promise-objects)
    Promise(Promise<'a>) = PROMISE_DISCRIMINANT,
    /// ## [28.2 Proxy Objects](https://tc39.es/ecma262/#sec-proxy-objects)
    Proxy(Proxy<'a>) = PROXY_DISCRIMINANT,
    #[cfg(feature = "regexp")]
    /// ## [22.2 RegExp (Regular Expression) Objects](https://tc39.es/ecma262/#sec-regexp-regular-expression-objects)
    RegExp(RegExp<'a>) = REGEXP_DISCRIMINANT,
    #[cfg(feature = "set")]
    /// ## [24.2 Set Objects](https://tc39.es/ecma262/#sec-set-objects)
    Set(Set<'a>) = SET_DISCRIMINANT,
    #[cfg(feature = "weak-refs")]
    /// ## [24.3 WeakMap Objects](https://tc39.es/ecma262/#sec-weakmap-objects)
    WeakMap(WeakMap<'a>) = WEAK_MAP_DISCRIMINANT,
    #[cfg(feature = "weak-refs")]
    /// ## [26.1 WeakRef Objects](https://tc39.es/ecma262/#sec-weak-ref-objects)
    WeakRef(WeakRef<'a>) = WEAK_REF_DISCRIMINANT,
    #[cfg(feature = "weak-refs")]
    /// ## [24.4 WeakSet Objects](https://tc39.es/ecma262/#sec-weakset-objects)
    WeakSet(WeakSet<'a>) = WEAK_SET_DISCRIMINANT,

    /// ## [25.1 ArrayBuffer Objects](https://tc39.es/ecma262/#sec-arraybuffer-objects)
    #[cfg(feature = "array-buffer")]
    ArrayBuffer(ArrayBuffer<'a>) = ARRAY_BUFFER_DISCRIMINANT,
    /// ## [25.2 SharedArrayBuffer Objects](https://tc39.es/ecma262/#sec-sharedarraybuffer-objects)
    #[cfg(feature = "shared-array-buffer")]
    SharedArrayBuffer(SharedArrayBuffer<'a>) = SHARED_ARRAY_BUFFER_DISCRIMINANT,
    /// ## [25.3 DataView Objects](https://tc39.es/ecma262/#sec-dataview-objects)
    #[cfg(feature = "array-buffer")]
    DataView(DataView<'a>) = DATA_VIEW_DISCRIMINANT,
    /// ## [25.3 DataView Objects](https://tc39.es/ecma262/#sec-dataview-objects)
    ///
    /// A variant of DataView Objects viewing a SharedArrayBuffer.
    #[cfg(feature = "shared-array-buffer")]
    SharedDataView(SharedDataView<'a>) = SHARED_DATA_VIEW_DISCRIMINANT,
    // ### [23.2 TypedArray Objects](https://tc39.es/ecma262/#sec-typedarray-objects)
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.17 Int8Array ( . . . )](https://tc39.es/ecma262/#sec-int8array)
    ///
    /// An `i8` view into an ArrayBuffer.
    Int8Array(Int8Array<'a>) = INT_8_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.35 Uint8Array ( . . . )](https://tc39.es/ecma262/#sec-uint8array)
    ///
    /// A `u8` view into an ArrayBuffer.
    Uint8Array(Uint8Array<'a>) = UINT_8_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.36 Uint8ClampedArray ( . . . )](https://tc39.es/ecma262/#sec-uint8clampedarray)
    ///
    /// A `u8` view into an ArrayBuffer with clamping behaviour on assignment.
    Uint8ClampedArray(Uint8ClampedArray<'a>) = UINT_8_CLAMPED_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.18 Int16Array ( . . . )](https://tc39.es/ecma262/#sec-int16array)
    ///
    /// An `i16` view into an ArrayBuffer.
    Int16Array(Int16Array<'a>) = INT_16_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.37 Uint16Array ( . . . )](https://tc39.es/ecma262/#sec-uint16array)
    ///
    /// A `u16` view into an ArrayBuffer.
    Uint16Array(Uint16Array<'a>) = UINT_16_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.19 Int32Array ( . . . )](https://tc39.es/ecma262/#sec-int32array)
    ///
    /// An `i32` view into an ArrayBuffer.
    Int32Array(Int32Array<'a>) = INT_32_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.38 Uint32Array ( . . . )](https://tc39.es/ecma262/#sec-uint32array)
    ///
    /// A `u32` view into an ArrayBuffer.
    Uint32Array(Uint32Array<'a>) = UINT_32_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.5 BigInt64Array ( . . . )](https://tc39.es/ecma262/#sec-constructor-properties-of-the-global-object-bigint64array)
    ///
    /// An `i64` view into an ArrayBuffer.
    BigInt64Array(BigInt64Array<'a>) = BIGINT_64_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.6 BigUint64Array ( . . . )](https://tc39.es/ecma262/#sec-constructor-properties-of-the-global-object-biguint64array)
    ///
    /// A `u64` view into an ArrayBuffer.
    BigUint64Array(BigUint64Array<'a>) = BIGUINT_64_ARRAY_DISCRIMINANT,
    #[cfg(feature = "proposal-float16array")]
    /// ### [19.3.13 Float16Array ( . . . )](https://tc39.es/ecma262/#sec-float16array)
    ///
    /// An `f16` view into an ArrayBuffer.
    Float16Array(Float16Array<'a>) = FLOAT_16_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.13 Float32Array ( . . . )](https://tc39.es/ecma262/#sec-float32array)
    ///
    /// An `f32` view into an ArrayBuffer.
    Float32Array(Float32Array<'a>) = FLOAT_32_ARRAY_DISCRIMINANT,
    #[cfg(feature = "array-buffer")]
    /// ### [19.3.13 Float64Array ( . . . )](https://tc39.es/ecma262/#sec-float64array)
    ///
    /// An `f64` view into an ArrayBuffer.
    Float64Array(Float64Array<'a>) = FLOAT_64_ARRAY_DISCRIMINANT,

    // ### [23.2 TypedArray Objects](https://tc39.es/ecma262/#sec-typedarray-objects)
    //
    // Variants of TypedArray Objects viewing a SharedArrayBuffer.
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.17 Int8Array ( . . . )](https://tc39.es/ecma262/#sec-int8array)
    ///
    /// An `i8` view into a SharedArrayBuffer.
    SharedInt8Array(SharedInt8Array<'a>) = SHARED_INT_8_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.35 Uint8Array ( . . . )](https://tc39.es/ecma262/#sec-uint8array)
    ///
    /// A `u8` view into a SharedArrayBuffer.
    SharedUint8Array(SharedUint8Array<'a>) = SHARED_UINT_8_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.36 Uint8ClampedArray ( . . . )](https://tc39.es/ecma262/#sec-uint8clampedarray)
    ///
    /// A `u8` view into an ArrayBuffer with clamping behaviour o Sharedassignment.
    SharedUint8ClampedArray(SharedUint8ClampedArray<'a>) = SHARED_UINT_8_CLAMPED_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.18 Int16Array ( . . . )](https://tc39.es/ecma262/#sec-int16array)
    ///
    /// An `i16` view into a SharedArrayBuffer.
    SharedInt16Array(SharedInt16Array<'a>) = SHARED_INT_16_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.37 Uint16Array ( . . . )](https://tc39.es/ecma262/#sec-uint16array)
    ///
    /// A `u16` view into a SharedArrayBuffer.
    SharedUint16Array(SharedUint16Array<'a>) = SHARED_UINT_16_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.19 Int32Array ( . . . )](https://tc39.es/ecma262/#sec-int32array)
    ///
    /// An `i32` view into a SharedArrayBuffer.
    SharedInt32Array(SharedInt32Array<'a>) = SHARED_INT_32_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.38 Uint32Array ( . . . )](https://tc39.es/ecma262/#sec-uint32array)
    ///
    /// A `u32` view into a SharedArrayBuffer.
    SharedUint32Array(SharedUint32Array<'a>) = SHARED_UINT_32_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.5 BigInt64Array ( . . . )](https://tc39.es/ecma262/#sec-constructor-properties-of-the-global-object-bigint64array)
    ///
    /// An `i64` view into a SharedArrayBuffer.
    SharedBigInt64Array(SharedBigInt64Array<'a>) = SHARED_BIGINT_64_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.6 BigUint64Array ( . . . )](https://tc39.es/ecma262/#sec-constructor-properties-of-the-global-object-biguint64array)
    ///
    /// A `u64` view into a SharedArrayBuffer.
    SharedBigUint64Array(SharedBigUint64Array<'a>) = SHARED_BIGUINT_64_ARRAY_DISCRIMINANT,
    #[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
    /// ### [19.3.13 Float16Array ( . . . )](https://tc39.es/ecma262/#sec-float16array)
    ///
    /// An `f16` view into a SharedArrayBuffer.
    SharedFloat16Array(SharedFloat16Array<'a>) = SHARED_FLOAT_16_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.13 Float32Array ( . . . )](https://tc39.es/ecma262/#sec-float32array)
    ///
    /// An `f32` view into a SharedArrayBuffer.
    SharedFloat32Array(SharedFloat32Array<'a>) = SHARED_FLOAT_32_ARRAY_DISCRIMINANT,
    #[cfg(feature = "shared-array-buffer")]
    /// ### [19.3.13 Float64Array ( . . . )](https://tc39.es/ecma262/#sec-float64array)
    ///
    /// An `f64` view into a SharedArrayBuffer.
    SharedFloat64Array(SharedFloat64Array<'a>) = SHARED_FLOAT_64_ARRAY_DISCRIMINANT,

    /// ## [27.6 AsyncGenerator Objects](https://tc39.es/ecma262/#sec-asyncgenerator-objects)
    AsyncGenerator(AsyncGenerator<'a>) = ASYNC_GENERATOR_DISCRIMINANT,
    /// ### [23.1.5 Array Iterator Objects](https://tc39.es/ecma262/#sec-array-iterator-objects)
    ArrayIterator(ArrayIterator<'a>) = ARRAY_ITERATOR_DISCRIMINANT,
    #[cfg(feature = "set")]
    /// ### [24.2.6 Set Iterator Objects](https://tc39.es/ecma262/#sec-set-iterator-objects)
    SetIterator(SetIterator<'a>) = SET_ITERATOR_DISCRIMINANT,
    #[cfg(feature = "set")]
    /// ### [24.1.5 Map Iterator Objects](https://tc39.es/ecma262/#sec-map-iterator-objects)
    MapIterator(MapIterator<'a>) = MAP_ITERATOR_DISCRIMINANT,
    /// ### [22.1.5 String Iterator Objects](https://tc39.es/ecma262/#sec-string-iterator-objects)
    StringIterator(StringIterator<'a>) = STRING_ITERATOR_DISCRIMINANT,
    #[cfg(feature = "regexp")]
    /// ### [22.2.9 RegExp String Iterator Objects](https://tc39.es/ecma262/#sec-regexp-string-iterator-objects)
    RegExpStringIterator(RegExpStringIterator<'a>) = REGEXP_STRING_ITERATOR_DISCRIMINANT,
    /// ## [27.5 Generator Objects](https://tc39.es/ecma262/#sec-generator-objects)
    Generator(Generator<'a>) = GENERATOR_DISCRIMINANT,
    /// ### [10.4.6 Module Namespace Exotic Objects](https://tc39.es/ecma262/#sec-module-namespace-exotic-objects)
    Module(Module<'a>) = MODULE_DISCRIMINANT,
    /// Embedder objects are intended for embedders to create objects with
    /// native data embedded into them.
    EmbedderObject(EmbedderObject<'a>) = EMBEDDER_OBJECT_DISCRIMINANT,
}

impl Object<'_> {
    /// Returns true if this Object is a Module.
    pub fn is_module(self) -> bool {
        matches!(self, Object::Module(_))
    }

    /// Returns true if this Object is a Proxy.
    pub fn is_proxy(self) -> bool {
        matches!(self, Object::Proxy(_))
    }
}

/// ### [6.1.7 The Object Type](https://tc39.es/ecma262/#sec-object-type)
///
/// Each instance of the _Object type_, also referred to simply as “an Object”,
/// represents a collection of properties. [`OrdinaryObject`]s specifically are
/// the only type in the Nova JavaScript engine that are truly Objects in the
/// strictest sense. They are ["ordinary objects"] in the specification's sense,
/// but they are not the only "ordinary objects" in the engine. They are,
/// however, the only ones that actually own a collection of properties.
///
/// All other objects in the Nova JavaScript engine merely optionally refer to
/// an [`OrdinaryObject`], called then the "backing object", and delegate to the
/// backing object all things related to ordinary object features such as
/// prototype changes and property assignments.
///
/// [`OrdinaryObject`]: OrdinaryObject
/// ["ordinary objects"]: https://tc39.es/ecma262/#sec-ordinary-object
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct OrdinaryObject<'a>(BaseIndex<'a, ObjectRecord<'static>>);

/// What [`OrdinaryObject::set_value_by_cache`] made of an assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CachedSet {
    /// The property was set.
    Done,
    /// The cache knows that neither the Object nor any it inherits from
    /// has the property: nothing was done.
    Absent,
    /// Nothing was done, and the cache does not say what to do.
    Unknown,
}
object_handle!(OrdinaryObject, Object);
arena_vec_access!(
    OrdinaryObject,
    'a,
    ObjectRecord,
    objects
);

/// The getter, or the setter, that the property at one place of one object
/// was last found to have (`OrdinaryObject::getter_by_cache`), with what
/// the heap counted then of all that can make it another: the property
/// defined again, or the object's properties moved
/// ([`Heap::descriptor_changes`](crate::heap::Heap)). The object and the
/// function are handles that a collection moves, so what was seen before
/// one is not looked at after it.
///
/// Finding the function is finding the object's storage and looking its
/// descriptors up in two tables, at every read and assignment of an
/// accessor property: a fifteenth of a loop that does one of each.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AccessorSeen {
    holder: OrdinaryObject<'static>,
    slot: u32,
    function: ECMAScriptFunction<'static>,
    collections: u64,
    descriptor_changes: u64,
}

impl AccessorSeen {
    /// The getter (or with `setter` the setter) of the property at `slot`
    /// of `holder`, if it is the one last seen there and still is.
    #[inline]
    fn function(
        agent: &Agent,
        holder: OrdinaryObject,
        slot: u32,
        setter: bool,
    ) -> Option<ECMAScriptFunction<'static>> {
        let seen = agent.accessor_seen[usize::from(setter)].get()?;
        (seen.collections == agent.heap.collections
            && seen.descriptor_changes == agent.heap.descriptor_changes
            && seen.slot == slot
            && seen.holder == holder.unbind())
        .then_some(seen.function)
    }

    /// Remembers that `function` is that getter or setter now.
    fn see(
        agent: &Agent,
        holder: OrdinaryObject,
        slot: u32,
        setter: bool,
        function: ECMAScriptFunction,
    ) {
        agent.accessor_seen[usize::from(setter)].set(Some(Self {
            holder: holder.unbind(),
            slot,
            function: function.unbind(),
            collections: agent.heap.collections,
            descriptor_changes: agent.heap.descriptor_changes,
        }));
    }
}

impl<'a> OrdinaryObject<'a> {
    /// Allocate a a new blank OrdinaryObject and return its reference.
    ///
    /// The new OrdinaryObject is conceptually equivalent to a
    /// `{ __proto__: null }` object.
    pub(crate) fn new_uninitialised(agent: &mut Agent) -> Self {
        agent.heap.objects.push(ObjectRecord::BLANK);
        OrdinaryObject(BaseIndex::last(&agent.heap.objects))
    }

    /// Returns true if the Object has no properties.
    pub(crate) fn is_empty(self, agent: &Agent) -> bool {
        self.get(agent).is_empty(agent)
    }

    /// Returns the number of properties in the object.
    pub(crate) fn len(self, agent: &Agent) -> u32 {
        self.get(agent).len(agent)
    }

    /// Create an empty object with the default prototype.
    pub fn create_empty_object(agent: &mut Agent, gc: NoGcScope<'a, '_>) -> Self {
        let Object::Object(ordinary) =
            ordinary_object_create_with_intrinsics(agent, ProtoIntrinsics::Object, None, gc)
        else {
            unreachable!()
        };
        ordinary
    }

    /// Turn an OrdinaryObject's Object Shape into an intrinsic.
    ///
    /// For objects with an intrinsic shape, this is a no-op.
    pub(crate) fn make_intrinsic(self, agent: &mut Agent) -> Result<(), TryReserveError> {
        let shape = self.object_shape(agent);
        if shape.is_intrinsic(agent) {
            // Already an intrinsic shape, nothing to do.
            return Ok(());
        }
        let new_shape = shape.make_intrinsic(agent)?;
        self.get_mut(agent).set_shape(new_shape);
        Ok(())
    }

    pub(crate) fn get_property_storage<'b>(self, agent: &'b Agent) -> PropertyStorageRef<'b, 'a> {
        let Heap {
            object_shapes,
            elements,
            objects,
            ..
        } = &agent.heap;
        let data = self.get_direct(objects);
        let shape = data.get_shape();
        let keys = shape.keys(object_shapes, elements);
        let elements = data.get_storage(elements, object_shapes);
        debug_assert_eq!(keys.len(), elements.values.len());
        PropertyStorageRef::from_keys_and_elements(keys, elements)
    }

    pub(crate) fn get_property_storage_mut<'b>(
        self,
        agent: &'b mut Agent,
    ) -> Option<PropertyStorageMut<'b, 'a>> {
        let Heap {
            object_shapes,
            elements,
            objects,
            ..
        } = &mut agent.heap;
        let data = self.get_direct(objects);
        let shape = data.get_shape();
        elements.get_property_storage_mut_raw(
            shape.keys_index(object_shapes),
            shape.keys_capacity(object_shapes),
            data.get_values(),
            shape.values_capacity(object_shapes),
            shape.len(object_shapes),
        )
    }

    /// Get the elements backing storage of the Object as an ElementsVector.
    ///
    /// The Object owns this backing storage uniquely. Performing mutations on
    /// the ElementsVector needs to be done carefully to avoid causing
    /// "undefined behaviour" in JavaScript.
    pub(crate) fn get_elements_vector(self, agent: &Agent) -> ElementsVector<'a> {
        let data = self.get(agent);
        let shape = data.get_shape();
        let elements_index = data.get_values();
        let len_writable = shape.extensible();
        let cap = shape.values_capacity(agent);
        let len = shape.len(agent);
        ElementsVector {
            elements_index,
            cap,
            len,
            len_writable,
        }
    }

    pub(crate) fn get_elements_storage<'b>(self, agent: &'b Agent) -> ElementStorageRef<'b, 'a> {
        let Heap {
            elements,
            objects,
            object_shapes,
            ..
        } = &agent.heap;
        let data = self.get_direct(objects);
        elements.get_element_storage_raw(
            data.get_values(),
            data.values_capacity(object_shapes),
            data.len(object_shapes),
        )
    }

    /// The value of the Object's own data property `key`, found by the key
    /// among its properties with no descriptor read: None for a key the
    /// Object does not have itself and for an accessor.
    #[inline]
    pub(crate) fn own_data_value(self, agent: &Agent, key: PropertyKey) -> Option<Value<'a>> {
        let offset = find_key_offset(agent, self, key)?;
        self.get_elements_values(agent)
            .get(offset)
            .copied()
            .flatten()
    }

    /// Sets the Object's own data property `key` to `value` where it has
    /// one that is writable: what OrdinarySet comes to for such a property
    /// with the Object as the receiver. Returns whether it did; nothing has
    /// changed where it did not.
    #[inline]
    pub(crate) fn set_own_writable_value(
        self,
        agent: &mut Agent,
        key: PropertyKey,
        value: Value<'static>,
    ) -> bool {
        let Some(offset) = find_key_offset(agent, self, key) else {
            return false;
        };
        self.set_writable_value_at(agent, offset, value)
    }

    /// The Object's shape as it is now, read from its record.
    #[inline]
    pub(crate) fn shape_now(self, agent: &Agent) -> ObjectShape<'a> {
        self.get_direct(&agent.heap.objects).get_shape()
    }

    /// The keys of the Object's properties, in the order of its property
    /// storage.
    pub(crate) fn get_property_keys<'b>(self, agent: &'b Agent) -> &'b [PropertyKey<'a>] {
        let Heap {
            elements,
            objects,
            object_shapes,
            ..
        } = &agent.heap;
        self.get_direct(objects)
            .get_shape()
            .keys(object_shapes, elements)
    }

    /// The value slots of the Object's property storage, without its
    /// descriptors: None is an accessor property.
    #[inline]
    pub(crate) fn get_elements_values<'b>(self, agent: &'b Agent) -> &'b [Option<Value<'a>>] {
        let Heap {
            elements,
            objects,
            object_shapes,
            ..
        } = &agent.heap;
        let data = self.get_direct(objects);
        elements.get_element_values_raw(
            data.get_values(),
            data.values_capacity(object_shapes),
            data.len(object_shapes),
        )
    }

    /// What `object.name` is where `cache`, the property cache of a place
    /// that names the property, knows the shape of this Object: the value
    /// of a data property of its own or of the object it inherits it from,
    /// undefined where the cache knows that neither has the property. None
    /// leaves everything to \[\[Get]]: a shape the cache does not know, an
    /// accessor property.
    ///
    /// All of it in one function, which is called for every property read
    /// by name in the loop of instructions: the shape, the search of the
    /// cache and the one look at the storage, where the shape was read, the
    /// cache searched by a function, the property read by another through
    /// a third, and what that returned was taken apart again.
    #[inline(never)]
    pub(crate) fn value_by_cache<'gc>(
        self,
        agent: &Agent,
        cache: PropertyLookupCache,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<Value<'gc>> {
        let Heap {
            elements,
            objects,
            object_shapes,
            ..
        } = &agent.heap;
        let data = self.get_direct(objects);
        let (offset, prototype, capacity) = cache.find_cached_place(agent, data.get_shape())?;
        if capacity != crate::heap::ElementArrayKey::Empty {
            // A value of the Object's own, in a storage whose capacity the
            // cache has kept: the shape's record is not asked for it.
            debug_assert_eq!(capacity, data.values_capacity(object_shapes));
            debug_assert!((offset.get_property_offset() as u32) < data.len(object_shapes));
            return elements
                .get_slot_value_raw(
                    data.get_values(),
                    capacity,
                    offset.get_property_offset() as usize,
                )
                .bind(gc);
        }
        if offset.is_unset() {
            return Some(Value::Undefined);
        }
        let data = match prototype {
            None => data,
            Some(Object::Object(prototype)) => prototype.get_direct(objects),
            // An object with properties of its own kind beside those of
            // its backing object.
            Some(prototype) => {
                return match prototype.get_own_property_at_offset(agent, offset, gc) {
                    TryGetResult::Value(value) => Some(value),
                    _ => None,
                };
            }
        };
        elements
            .get_element_value_raw(
                data.get_values(),
                data.values_capacity(object_shapes),
                data.len(object_shapes),
                offset.get_property_offset() as usize,
            )
            .bind(gc)
    }

    /// Where `cache` places `object.name` for the shape of this Object, if
    /// that is in an ordinary object: the Object itself or the one it
    /// inherits the property from, and the slot there.
    fn place_by_cache(
        self,
        agent: &Agent,
        cache: PropertyLookupCache,
    ) -> Option<(OrdinaryObject<'static>, u32)> {
        let shape = self.get_direct(&agent.heap.objects).get_shape();
        let (offset, prototype) = cache.find_cached_property_offset(agent, shape)?;
        if offset.is_unset() {
            return None;
        }
        let holder = match prototype {
            None => self.unbind(),
            Some(Object::Object(prototype)) => prototype.unbind(),
            Some(_) => return None,
        };
        Some((holder, offset.get_property_offset() as u32))
    }

    /// The getter of `object.name` where `cache` knows the shape of this
    /// Object and the property is an accessor, of its own or inherited,
    /// whose getter is an ECMAScript function: what \[\[Get]] would call
    /// with the Object as the this value. Asked only where
    /// [`OrdinaryObject::value_by_cache`] found no value.
    #[inline(never)]
    pub(crate) fn getter_by_cache<'gc>(
        self,
        agent: &Agent,
        cache: PropertyLookupCache,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<ECMAScriptFunction<'gc>> {
        let (holder, slot) = self.place_by_cache(agent, cache)?;
        if let Some(getter) = AccessorSeen::function(agent, holder, slot, false) {
            return Some(getter.bind(gc));
        }
        let descriptor = holder.get_elements_storage(agent).descriptors?.get(&slot)?;
        match descriptor.getter_function(gc)? {
            Function::ECMAScriptFunction(getter) => {
                AccessorSeen::see(agent, holder, slot, false, getter);
                Some(getter)
            }
            _ => None,
        }
    }

    /// The setter of `object.name` where `cache` knows the shape of this
    /// Object and the property is an accessor, of its own or inherited,
    /// whose setter is an ECMAScript function: what \[\[Set]] would call
    /// with the Object as the this value and the value assigned. Asked only
    /// where [`OrdinaryObject::set_value_by_cache`] did not set a value.
    #[inline(never)]
    pub(crate) fn setter_by_cache<'gc>(
        self,
        agent: &Agent,
        cache: PropertyLookupCache,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<ECMAScriptFunction<'gc>> {
        let (holder, slot) = self.place_by_cache(agent, cache)?;
        if let Some(setter) = AccessorSeen::function(agent, holder, slot, true) {
            return Some(setter.bind(gc));
        }
        let descriptor = holder.get_elements_storage(agent).descriptors?.get(&slot)?;
        match descriptor.setter_function(gc)? {
            Function::ECMAScriptFunction(setter) => {
                AccessorSeen::see(agent, holder, slot, true, setter);
                Some(setter)
            }
            _ => None,
        }
    }

    /// `object.name = value` where `cache` knows the shape of this Object
    /// and places the property in it as its own: set where it is a data
    /// property that is writable. As [`OrdinaryObject::value_by_cache`],
    /// all of it in one function.
    #[inline(never)]
    pub(crate) fn set_value_by_cache(
        self,
        agent: &mut Agent,
        cache: PropertyLookupCache,
        value: Value,
    ) -> CachedSet {
        let data = self.get_direct(&agent.heap.objects);
        let (shape, values) = (data.get_shape(), data.get_values().unbind());
        let Some((offset, None, capacity)) = cache.find_cached_place(agent, shape) else {
            return CachedSet::Unknown;
        };
        if capacity != crate::heap::ElementArrayKey::Empty {
            // As in `value_by_cache`: the capacity of the storage is the
            // one the cache has kept.
            debug_assert_eq!(capacity, shape.values_capacity(agent));
            debug_assert!((offset.get_property_offset() as u32) < shape.len(agent));
            return if agent.heap.elements.set_writable_slot_raw(
                values,
                capacity,
                offset.get_property_offset() as usize,
                value.unbind(),
            ) {
                CachedSet::Done
            } else {
                CachedSet::Unknown
            };
        }
        if offset.is_unset() {
            return CachedSet::Absent;
        }
        if self.set_writable_value_at(agent, offset.get_property_offset() as usize, value.unbind())
        {
            CachedSet::Done
        } else {
            CachedSet::Unknown
        }
    }

    /// Sets the property at `offset` of the Object's property storage to
    /// `value` where it is an own data property that is writable, in one
    /// look at the storage. Returns whether it did.
    #[inline]
    pub(crate) fn set_writable_value_at(
        self,
        agent: &mut Agent,
        offset: usize,
        value: Value<'static>,
    ) -> bool {
        let Heap {
            elements,
            objects,
            object_shapes,
            ..
        } = &mut agent.heap;
        let data = self.get_direct(objects);
        elements.set_writable_value_raw(
            data.get_values().unbind(),
            data.values_capacity(object_shapes),
            data.len(object_shapes),
            offset,
            value,
        )
    }

    /// Exclusive access to the value slots of the Object's property storage,
    /// without its descriptors.
    pub(crate) fn get_elements_values_mut<'b>(
        self,
        agent: &'b mut Agent,
    ) -> &'b mut [Option<Value<'static>>] {
        let Heap {
            elements,
            objects,
            object_shapes,
            ..
        } = &mut agent.heap;
        let data = self.get_direct(objects);
        elements.get_element_values_mut_raw(
            data.get_values(),
            data.values_capacity(object_shapes),
            data.len(object_shapes),
        )
    }

    pub(crate) fn get_elements_storage_mut<'b>(
        self,
        agent: &'b mut Agent,
    ) -> ElementStorageMut<'b> {
        let Heap {
            elements,
            objects,
            object_shapes,
            ..
        } = &mut agent.heap;
        let data = self.get_direct(objects);
        elements.get_element_storage_mut_raw(
            data.get_values(),
            data.values_capacity(object_shapes),
            data.len(object_shapes),
        )
    }

    pub(crate) fn get_elements_storage_uninit<'b>(
        self,
        agent: &'b mut Agent,
    ) -> ElementStorageUninit<'b> {
        let Heap {
            elements,
            objects,
            object_shapes,
            ..
        } = &mut agent.heap;
        let data = self.get_direct(objects);
        elements
            .get_element_storage_uninit_raw(data.get_values(), data.values_capacity(object_shapes))
    }

    fn create_object_internal(
        agent: &mut Agent,
        shape: ObjectShape<'a>,
        entries: &[ObjectEntry<'a>],
    ) -> Self {
        let nontrivial_entry_count = entries.iter().filter(|p| !p.is_trivial()).count();
        agent.heap.alloc_counter += core::mem::size_of::<Option<Value>>() * entries.len()
            + if nontrivial_entry_count > 0 {
                core::mem::size_of::<Option<AHashMap<u32, ElementDescriptor<'static>>>>()
                    + core::mem::size_of::<(u32, ElementDescriptor<'static>)>()
                        * nontrivial_entry_count
            } else {
                0
            };
        let ElementsVector {
            elements_index: values,
            cap,
            len,
            len_writable: _,
        } = agent
            .heap
            .elements
            .allocate_object_property_storage_from_entries_slice(entries)
            .expect("Failed to create object");
        assert_eq!(len, shape.len(agent));
        assert_eq!(cap.capacity(), shape.values_capacity(agent).capacity());
        agent.heap.create(ObjectRecord::new(shape, values))
    }

    /// Create an object with the given prototype and properties.
    pub fn create_object(
        agent: &mut Agent,
        prototype: Option<Object<'a>>,
        entries: &[ObjectEntry<'a>],
    ) -> Result<Self, TryReserveError> {
        let base_shape = ObjectShape::get_shape_for_prototype(agent, prototype);
        let mut shape = base_shape;
        for e in entries {
            shape = shape.get_child_shape(agent, e.key)?;
        }
        Ok(Self::create_object_internal(agent, shape, entries))
    }

    pub(crate) fn create_object_with_shape_and_data_properties(
        agent: &mut Agent,
        shape: ObjectShape<'a>,
        values: &[Value<'a>],
    ) -> Self {
        let mut elements = agent
            .heap
            .elements
            .allocate_elements_with_length(values.len())
            .expect("Failed to create object");
        elements.len = u32::try_from(values.len()).expect("Failed to create object");
        for (slot, value) in elements.get_mut(agent).iter_mut().zip(values) {
            *slot = Some(value.unbind());
        }
        let ElementsVector {
            elements_index: values,
            cap,
            len,
            len_writable: _,
        } = elements;
        assert_eq!(cap, shape.values_capacity(agent));
        assert_eq!(len, shape.len(agent));
        agent.heap.create(ObjectRecord::new(shape, values))
    }

    pub(crate) fn create_object_with_shape(
        agent: &mut Agent,
        shape: ObjectShape<'a>,
    ) -> Result<Self, TryReserveError> {
        let ElementsVector {
            elements_index: values,
            cap,
            len: _,
            len_writable: _,
        } = agent
            .heap
            .elements
            .allocate_elements_with_capacity(shape.values_capacity(agent))?;
        assert_eq!(cap, shape.values_capacity(agent));
        let len = shape.len(agent);
        agent.heap.elements.ensure_values_len(cap, values, len)?;
        Ok(agent.heap.create(ObjectRecord::new(shape, values)))
    }

    /// Creates a new "intrinsic" object. An intrinsic object owns its Object
    /// Shape uniquely and thus any changes to the object properties mutate the
    /// Shape directly.
    pub(crate) fn create_intrinsic_object(
        agent: &mut Agent,
        prototype: Option<Object<'a>>,
        entries: &[ObjectEntry<'a>],
    ) -> Result<Self, TryReserveError> {
        let properties_count = entries.len();
        let (cap, index) = agent
            .heap
            .elements
            .allocate_keys_with_capacity(properties_count)?;
        let cap = cap.make_intrinsic();
        let keys_memory = agent.heap.elements.get_keys_uninit_raw(cap, index);
        for (slot, key) in keys_memory.iter_mut().zip(entries.iter().map(|e| e.key)) {
            *slot = key.unbind();
        }
        let shape = agent.heap.create(ObjectShapeRecord::create(
            prototype,
            index,
            cap,
            properties_count,
        ));
        Ok(Self::create_object_internal(agent, shape, entries))
    }

    /// Attempts to make this ordinary Object a copy of some source ordinary
    /// Object: what CopyDataProperties gives where this Object has no
    /// properties yet, the prototypes of the two match, and every own
    /// property of the source is a data property with a String key that is
    /// writable, enumerable and configurable. This Object then has the
    /// source's shape and a copy of its values.
    ///
    /// Returns true if the copy succeeded. If the copy does not succeed, no
    /// changes to either object are performed and the function returns false.
    pub(crate) fn try_copy_from_object(self, agent: &mut Agent, source: OrdinaryObject) -> bool {
        if source.is_empty(agent) {
            // It's always possible to copy an empty object.
            return true;
        }
        if self == source
            || !self.is_empty(agent)
            || !self.internal_extensible(agent)
            || source.internal_prototype(agent) != self.internal_prototype(agent)
            || source.get(agent).get_shape().is_intrinsic(agent)
        {
            // Our own object is not empty or takes no properties, our
            // prototypes don't match, or the source object is an intrinsic
            // object; can't perform the copy.
            // Note: for intrinsic objects the problem is that their Object
            // Shape is considered uniquely owned by the intrinsic object
            // itself.
            return false;
        }
        // Copying properties from one ordinary object to another and they
        // have the same prototype: we may be able to perform a trivial copy.
        let PropertyStorageRef {
            keys, descriptors, ..
        } = source.unbind().get_property_storage(agent);
        if descriptors.is_some() || keys.iter().any(|key| !key.is_string()) {
            // A property that is not a plain data property (an accessor, or
            // one that is not writable, not enumerable or not configurable:
            // the copy of it is a plain one, or none), or a key that is not
            // a String (a Symbol, which is copied after the Strings; an
            // integer, which is copied before them; a private name, which is
            // not copied). Cannot perform the copy.
            return false;
        }
        // The shape of the source and the self objects will be identical
        // after this operation.
        let mut source_shape = source.get(agent).get_shape();
        // Note: our source object can be non-extensible but we should not
        // become so just by copying the source properties.
        source_shape.set_extensible(true);
        let elements_vector = agent
            .heap
            .elements
            .shallow_clone(&source.get_elements_vector(agent));
        agent.heap.alloc_counter +=
            core::mem::size_of::<Option<Value>>() * elements_vector.len() as usize;
        let data = self.get_mut(agent);
        data.set_shape(source_shape);
        data.set_values(elements_vector.elements_index.unbind());
        true
    }
}

impl IntrinsicObjectIndexes {
    pub(crate) const fn get_backing_object<'a>(
        self,
        base: BaseIndex<'a, ObjectRecord<'static>>,
    ) -> OrdinaryObject<'a> {
        OrdinaryObject(BaseIndex::from_index_u32_const(
            self as u32 + base.get_index_u32_const() + Self::OBJECT_INDEX_OFFSET,
        ))
    }
}

impl IntrinsicConstructorIndexes {
    pub(crate) const fn get_backing_object<'a>(
        self,
        base: BaseIndex<'a, ObjectRecord<'static>>,
    ) -> OrdinaryObject<'a> {
        OrdinaryObject(BaseIndex::from_index_u32_const(
            self as u32 + base.get_index_u32_const() + Self::OBJECT_INDEX_OFFSET,
        ))
    }
}

impl IntrinsicPrimitiveObjectIndexes {
    pub(crate) const fn get_backing_object<'a>(
        self,
        base: BaseIndex<'a, ObjectRecord<'static>>,
    ) -> OrdinaryObject<'a> {
        OrdinaryObject(BaseIndex::from_index_u32_const(
            self as u32 + base.get_index_u32_const() + Self::OBJECT_INDEX_OFFSET,
        ))
    }
}

impl<'a> InternalSlots<'a> for OrdinaryObject<'a> {
    #[inline(always)]
    fn get_backing_object(self, _: &Agent) -> Option<OrdinaryObject<'static>> {
        Some(self.unbind())
    }

    fn set_backing_object(self, _agent: &mut Agent, _backing_object: OrdinaryObject<'static>) {
        unreachable!();
    }

    fn create_backing_object(self, _: &mut Agent) -> OrdinaryObject<'static> {
        unreachable!();
    }

    fn object_shape(self, agent: &mut Agent) -> ObjectShape<'static> {
        self.get(agent).get_shape().unbind()
    }

    fn internal_extensible(self, agent: &Agent) -> bool {
        self.get(agent).get_extensible()
    }

    fn internal_set_extensible(self, agent: &mut Agent, value: bool) {
        self.get_mut(agent).set_extensible(value)
    }

    fn internal_prototype(self, agent: &Agent) -> Option<Object<'static>> {
        self.get(agent).get_prototype(agent).unbind()
    }

    fn internal_set_prototype(self, agent: &mut Agent, prototype: Option<Object>) {
        let original_shape = self.get(agent).get_shape();
        if original_shape.get_prototype(agent) == prototype {
            return;
        }
        let new_shape = original_shape.get_shape_with_prototype(agent, prototype);
        self.get_mut(agent).set_shape(new_shape);
    }
}

impl<'a> From<Object<'a>> for Value<'a> {
    #[inline]
    fn from(value: Object<'a>) -> Self {
        match value {
            Object::Object(data) => Self::Object(data),
            Object::BoundFunction(data) => Self::BoundFunction(data),
            Object::BuiltinFunction(data) => Self::BuiltinFunction(data),
            Object::ECMAScriptFunction(data) => Self::ECMAScriptFunction(data),
            Object::BuiltinConstructorFunction(data) => Self::BuiltinConstructorFunction(data),
            Object::BuiltinPromiseResolvingFunction(data) => {
                Self::BuiltinPromiseResolvingFunction(data)
            }
            Object::BuiltinPromiseFinallyFunction(data) => {
                Self::BuiltinPromiseFinallyFunction(data)
            }
            Object::BuiltinProxyRevokerFunction => Self::BuiltinProxyRevokerFunction,
            Object::PrimitiveObject(data) => Self::PrimitiveObject(data),
            Object::Arguments(data) => Self::Arguments(data),
            Object::Array(data) => Self::Array(data),
            #[cfg(feature = "date")]
            Object::Date(data) => Self::Date(data),
            #[cfg(feature = "temporal")]
            Object::Instant(data) => Value::Instant(data),
            #[cfg(feature = "temporal")]
            Object::Duration(data) => Value::Duration(data),
            #[cfg(feature = "temporal")]
            Object::PlainTime(data) => Value::PlainTime(data),
            Object::Error(data) => Self::Error(data),
            Object::FinalizationRegistry(data) => Self::FinalizationRegistry(data),
            Object::Map(data) => Self::Map(data),
            Object::Promise(data) => Self::Promise(data),
            Object::Proxy(data) => Self::Proxy(data),
            #[cfg(feature = "regexp")]
            Object::RegExp(data) => Self::RegExp(data),
            #[cfg(feature = "set")]
            Object::Set(data) => Self::Set(data),
            #[cfg(feature = "weak-refs")]
            Object::WeakMap(data) => Self::WeakMap(data),
            #[cfg(feature = "weak-refs")]
            Object::WeakRef(data) => Self::WeakRef(data),
            #[cfg(feature = "weak-refs")]
            Object::WeakSet(data) => Self::WeakSet(data),

            #[cfg(feature = "array-buffer")]
            Object::ArrayBuffer(ab) => Self::ArrayBuffer(ab),
            #[cfg(feature = "array-buffer")]
            Object::DataView(dv) => Self::DataView(dv),
            #[cfg(feature = "array-buffer")]
            Object::Int8Array(ta) => Self::Int8Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::Uint8Array(ta) => Self::Uint8Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::Uint8ClampedArray(ta) => Self::Uint8ClampedArray(ta),
            #[cfg(feature = "array-buffer")]
            Object::Int16Array(ta) => Self::Int16Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::Uint16Array(ta) => Self::Uint16Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::Int32Array(ta) => Self::Int32Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::Uint32Array(ta) => Self::Uint32Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::BigInt64Array(ta) => Self::BigInt64Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::BigUint64Array(ta) => Self::BigUint64Array(ta),
            #[cfg(feature = "proposal-float16array")]
            Object::Float16Array(ta) => Self::Float16Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::Float32Array(ta) => Self::Float32Array(ta),
            #[cfg(feature = "array-buffer")]
            Object::Float64Array(ta) => Self::Float64Array(ta),

            #[cfg(feature = "shared-array-buffer")]
            Object::SharedArrayBuffer(sab) => Self::SharedArrayBuffer(sab),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedDataView(sdv) => Self::SharedDataView(sdv),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedInt8Array(sta) => Self::SharedInt8Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedUint8Array(sta) => Self::SharedUint8Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedUint8ClampedArray(sta) => Self::SharedUint8ClampedArray(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedInt16Array(sta) => Self::SharedInt16Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedUint16Array(sta) => Self::SharedUint16Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedInt32Array(sta) => Self::SharedInt32Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedUint32Array(sta) => Self::SharedUint32Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedBigInt64Array(sta) => Self::SharedBigInt64Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedBigUint64Array(sta) => Self::SharedBigUint64Array(sta),
            #[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
            Object::SharedFloat16Array(sta) => Self::SharedFloat16Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedFloat32Array(sta) => Self::SharedFloat32Array(sta),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedFloat64Array(sta) => Self::SharedFloat64Array(sta),

            Object::AsyncGenerator(data) => Self::AsyncGenerator(data),
            Object::ArrayIterator(data) => Self::ArrayIterator(data),
            #[cfg(feature = "set")]
            Object::SetIterator(data) => Self::SetIterator(data),
            Object::MapIterator(data) => Self::MapIterator(data),
            Object::StringIterator(data) => Self::StringIterator(data),
            #[cfg(feature = "regexp")]
            Object::RegExpStringIterator(data) => Self::RegExpStringIterator(data),
            Object::Generator(data) => Self::Generator(data),
            Object::Module(data) => Self::Module(data),
            Object::EmbedderObject(data) => Self::EmbedderObject(data),
        }
    }
}

impl<'a> TryFrom<Value<'a>> for Object<'a> {
    type Error = ();
    #[inline]
    fn try_from(value: Value<'a>) -> Result<Self, ()> {
        match value {
            Value::Undefined
            | Value::Null
            | Value::Boolean(_)
            | Value::String(_)
            | Value::SmallString(_)
            | Value::Symbol(_)
            | Value::Number(_)
            | Value::Integer(_)
            | Value::SmallF64(_)
            | Value::BigInt(_)
            | Value::SmallBigInt(_) => Err(()),
            Value::Object(data) => Ok(Object::Object(data)),
            Value::BoundFunction(data) => Ok(Object::BoundFunction(data)),
            Value::BuiltinFunction(data) => Ok(Object::BuiltinFunction(data)),
            Value::ECMAScriptFunction(data) => Ok(Object::ECMAScriptFunction(data)),
            Value::BuiltinConstructorFunction(data) => Ok(Object::BuiltinConstructorFunction(data)),
            Value::BuiltinPromiseResolvingFunction(data) => {
                Ok(Object::BuiltinPromiseResolvingFunction(data))
            }
            Value::BuiltinPromiseFinallyFunction(data) => {
                Ok(Object::BuiltinPromiseFinallyFunction(data))
            }
            Value::BuiltinProxyRevokerFunction => Ok(Object::BuiltinProxyRevokerFunction),
            Value::PrimitiveObject(data) => Ok(Object::PrimitiveObject(data)),
            Value::Arguments(data) => Ok(Object::Arguments(data)),
            Value::Array(data) => Ok(Object::Array(data)),
            #[cfg(feature = "date")]
            Value::Date(data) => Ok(Object::Date(data)),
            #[cfg(feature = "temporal")]
            Value::Instant(data) => Ok(Object::Instant(data)),
            #[cfg(feature = "temporal")]
            Value::Duration(data) => Ok(Object::Duration(data)),
            #[cfg(feature = "temporal")]
            Value::PlainTime(data) => Ok(Object::PlainTime(data)),
            Value::Error(data) => Ok(Object::Error(data)),
            Value::FinalizationRegistry(data) => Ok(Object::FinalizationRegistry(data)),
            Value::Map(data) => Ok(Object::Map(data)),
            Value::Promise(data) => Ok(Object::Promise(data)),
            Value::Proxy(data) => Ok(Object::Proxy(data)),
            #[cfg(feature = "regexp")]
            Value::RegExp(data) => Ok(Object::RegExp(data)),
            #[cfg(feature = "set")]
            Value::Set(data) => Ok(Object::Set(data)),
            #[cfg(feature = "weak-refs")]
            Value::WeakMap(data) => Ok(Object::WeakMap(data)),
            #[cfg(feature = "weak-refs")]
            Value::WeakRef(data) => Ok(Object::WeakRef(data)),
            #[cfg(feature = "weak-refs")]
            Value::WeakSet(data) => Ok(Object::WeakSet(data)),
            #[cfg(feature = "array-buffer")]
            Value::ArrayBuffer(data) => Ok(Object::ArrayBuffer(data)),
            #[cfg(feature = "array-buffer")]
            Value::DataView(data) => Ok(Object::DataView(data)),
            #[cfg(feature = "array-buffer")]
            Value::Int8Array(data) => Ok(Object::Int8Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::Uint8Array(data) => Ok(Object::Uint8Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::Uint8ClampedArray(data) => Ok(Object::Uint8ClampedArray(data)),
            #[cfg(feature = "array-buffer")]
            Value::Int16Array(data) => Ok(Object::Int16Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::Uint16Array(data) => Ok(Object::Uint16Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::Int32Array(data) => Ok(Object::Int32Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::Uint32Array(data) => Ok(Object::Uint32Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::BigInt64Array(data) => Ok(Object::BigInt64Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::BigUint64Array(data) => Ok(Object::BigUint64Array(data)),
            #[cfg(feature = "proposal-float16array")]
            Value::Float16Array(data) => Ok(Object::Float16Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::Float32Array(data) => Ok(Object::Float32Array(data)),
            #[cfg(feature = "array-buffer")]
            Value::Float64Array(data) => Ok(Object::Float64Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedArrayBuffer(data) => Ok(Object::SharedArrayBuffer(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedDataView(data) => Ok(Object::SharedDataView(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedInt8Array(data) => Ok(Object::SharedInt8Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedUint8Array(data) => Ok(Object::SharedUint8Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedUint8ClampedArray(data) => Ok(Object::SharedUint8ClampedArray(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedInt16Array(data) => Ok(Object::SharedInt16Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedUint16Array(data) => Ok(Object::SharedUint16Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedInt32Array(data) => Ok(Object::SharedInt32Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedUint32Array(data) => Ok(Object::SharedUint32Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedBigInt64Array(data) => Ok(Object::SharedBigInt64Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedBigUint64Array(data) => Ok(Object::SharedBigUint64Array(data)),
            #[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
            Value::SharedFloat16Array(data) => Ok(Object::SharedFloat16Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedFloat32Array(data) => Ok(Object::SharedFloat32Array(data)),
            #[cfg(feature = "shared-array-buffer")]
            Value::SharedFloat64Array(data) => Ok(Object::SharedFloat64Array(data)),
            Value::AsyncGenerator(data) => Ok(Object::AsyncGenerator(data)),
            Value::ArrayIterator(data) => Ok(Object::ArrayIterator(data)),
            #[cfg(feature = "set")]
            Value::SetIterator(data) => Ok(Object::SetIterator(data)),
            Value::MapIterator(data) => Ok(Object::MapIterator(data)),
            Value::StringIterator(data) => Ok(Object::StringIterator(data)),
            #[cfg(feature = "regexp")]
            Value::RegExpStringIterator(data) => Ok(Object::RegExpStringIterator(data)),
            Value::Generator(data) => Ok(Object::Generator(data)),
            Value::Module(data) => Ok(Object::Module(data)),
            Value::EmbedderObject(data) => Ok(Object::EmbedderObject(data)),
        }
    }
}

impl<'a> OrdinaryObject<'a> {
    pub(crate) fn property_storage(self) -> PropertyStorage<'a> {
        PropertyStorage::new(self)
    }
}

macro_rules! object_delegate {
    // A Proxy's internal methods root handles (and its target's do too) that
    // are dead once they return: drop them, so a native loop over a Proxy
    // holds a bounded number of roots instead of one set per element.
    (@rooted $value: ident, $method: ident, $agent: ident $(, $arg:expr)* $(,)?) => {
        if let Self::Proxy(data) = $value {
            let roots = $agent.stack_refs.borrow().len();
            let result = data.$method($agent $(, $arg)*);
            $agent.stack_refs.borrow_mut().truncate(roots);
            result
        } else {
            object_delegate!($value, $method, $agent $(, $arg)*)
        }
    };
    ($value: ident, $method: ident, $($arg:expr),*) => {
        match $value {
            Self::Object(data) => data.$method($($arg),+),
            Self::Array(data) => data.$method($($arg),+),
            #[cfg(feature = "date")]
            Self::Date(data) => data.$method($($arg),+),
            #[cfg(feature = "temporal")]
            Object::Instant(data) => data.$method($($arg),+),
            #[cfg(feature = "temporal")]
            Object::Duration(data) => data.$method($($arg),+),
            #[cfg(feature = "temporal")]
            Object::PlainTime(data) => data.$method($($arg),+),
            Self::Error(data) => data.$method($($arg),+),
            Self::BoundFunction(data) => data.$method($($arg),+),
            Self::BuiltinFunction(data) => data.$method($($arg),+),
            Self::ECMAScriptFunction(data) => data.$method($($arg),+),
            Self::BuiltinConstructorFunction(data) => data.$method($($arg),+),
            Self::BuiltinPromiseResolvingFunction(data) => data.$method($($arg),+),
            Self::BuiltinPromiseFinallyFunction(data) => data.$method($($arg),+),
            Self::BuiltinProxyRevokerFunction => todo!(),
            Self::PrimitiveObject(data) => data.$method($($arg),+),
            Self::Arguments(data) => data.$method($($arg),+),
            Self::FinalizationRegistry(data) => data.$method($($arg),+),
            Self::Map(data) => data.$method($($arg),+),
            Self::Promise(data) => data.$method($($arg),+),
            Self::Proxy(data) => data.$method($($arg),+),
            #[cfg(feature = "regexp")]
            Self::RegExp(data) => data.$method($($arg),+),
            #[cfg(feature = "set")]
            Self::Set(data) => data.$method($($arg),+),
            #[cfg(feature = "weak-refs")]
            Self::WeakMap(data) => data.$method($($arg),+),
            #[cfg(feature = "weak-refs")]
            Self::WeakRef(data) => data.$method($($arg),+),
            #[cfg(feature = "weak-refs")]
            Self::WeakSet(data) => data.$method($($arg),+),

            #[cfg(feature = "array-buffer")]
            Self::ArrayBuffer(ab) => ab.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::DataView(dv) => dv.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Int8Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Uint8Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Uint8ClampedArray(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Int16Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Uint16Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Int32Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Uint32Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::BigInt64Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::BigUint64Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "proposal-float16array")]
            Self::Float16Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Float32Array(ta) => ta.$method($($arg),+),
            #[cfg(feature = "array-buffer")]
            Self::Float64Array(ta) => ta.$method($($arg),+),

            #[cfg(feature = "shared-array-buffer")]
            Self::SharedArrayBuffer(sab) => sab.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedDataView(sdv) => sdv.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedInt8Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedUint8Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedUint8ClampedArray(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedInt16Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedUint16Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedInt32Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedUint32Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedBigInt64Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedBigUint64Array(sta) => sta.$method($($arg),+),
            #[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
            Self::SharedFloat16Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedFloat32Array(sta) => sta.$method($($arg),+),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedFloat64Array(sta) => sta.$method($($arg),+),

            Self::AsyncGenerator(data) => data.$method($($arg),+),
            Self::ArrayIterator(data) => data.$method($($arg),+),
            #[cfg(feature = "set")]
            Self::SetIterator(data) => data.$method($($arg),+),
            Self::MapIterator(data) => data.$method($($arg),+),
            Self::StringIterator(data) => data.$method($($arg),+),
            #[cfg(feature = "regexp")]
            Self::RegExpStringIterator(data) => data.$method($($arg),+),
            Self::Generator(data) => data.$method($($arg),+),
            Self::Module(data) => data.$method($($arg),+),
            Self::EmbedderObject(data) => data.$method($($arg),+),
        }
    };
}

impl<'a> InternalSlots<'a> for Object<'a> {
    fn get_backing_object(self, agent: &Agent) -> Option<OrdinaryObject<'static>> {
        object_delegate!(self, get_backing_object, agent)
    }

    fn set_backing_object(self, _agent: &mut Agent, _backing_object: OrdinaryObject<'static>) {
        unreachable!("Object should not try to set its backing object");
    }

    fn create_backing_object(self, _: &mut Agent) -> OrdinaryObject<'static> {
        unreachable!("Object should not try to create its backing object");
    }

    fn get_or_create_backing_object(self, agent: &mut Agent) -> OrdinaryObject<'static> {
        object_delegate!(self, get_or_create_backing_object, agent)
    }

    fn object_shape(self, agent: &mut Agent) -> ObjectShape<'static> {
        object_delegate!(self, object_shape, agent)
    }

    fn internal_extensible(self, agent: &Agent) -> bool {
        object_delegate!(self, internal_extensible, agent)
    }

    fn internal_set_extensible(self, agent: &mut Agent, value: bool) {
        object_delegate!(self, internal_set_extensible, agent, value)
    }

    fn internal_prototype(self, agent: &Agent) -> Option<Object<'static>> {
        object_delegate!(self, internal_prototype, agent)
    }

    fn internal_set_prototype(self, agent: &mut Agent, prototype: Option<Object>) {
        object_delegate!(self, internal_set_prototype, agent, prototype)
    }
}

impl<'a> InternalMethods<'a> for Object<'a> {
    fn try_get_prototype_of<'gc>(
        self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, Option<Object<'gc>>> {
        object_delegate!(self, try_get_prototype_of, agent, gc)
    }

    fn internal_get_prototype_of<'gc>(
        self,
        agent: &mut Agent,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Object<'gc>>> {
        object_delegate!(@rooted self, internal_get_prototype_of, agent, gc)
    }

    fn try_set_prototype_of<'gc>(
        self,
        agent: &mut Agent,
        prototype: Option<Object>,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, bool> {
        object_delegate!(self, try_set_prototype_of, agent, prototype, gc)
    }

    fn internal_set_prototype_of<'gc>(
        self,
        agent: &mut Agent,
        prototype: Option<Object>,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, bool> {
        object_delegate!(@rooted self, internal_set_prototype_of, agent, prototype, gc)
    }

    fn try_is_extensible<'gc>(
        self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, bool> {
        object_delegate!(self, try_is_extensible, agent, gc)
    }

    fn internal_is_extensible<'gc>(
        self,
        agent: &mut Agent,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, bool> {
        object_delegate!(@rooted self, internal_is_extensible, agent, gc)
    }

    fn try_prevent_extensions<'gc>(
        self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, bool> {
        object_delegate!(self, try_prevent_extensions, agent, gc)
    }

    fn internal_prevent_extensions<'gc>(
        self,
        agent: &mut Agent,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, bool> {
        object_delegate!(@rooted self, internal_prevent_extensions, agent, gc)
    }

    fn try_get_own_property<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        cache: Option<PropertyLookupCache>,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, Option<PropertyDescriptor<'gc>>> {
        object_delegate!(self, try_get_own_property, agent, property_key, cache, gc)
    }

    fn internal_get_own_property<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<PropertyDescriptor<'gc>>> {
        object_delegate!(@rooted self, internal_get_own_property, agent, property_key, gc)
    }

    fn try_define_own_property<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        property_descriptor: PropertyDescriptor,
        cache: Option<PropertyLookupCache>,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, bool> {
        object_delegate!(
            self,
            try_define_own_property,
            agent,
            property_key,
            property_descriptor,
            cache,
            gc
        )
    }

    fn internal_define_own_property<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        property_descriptor: PropertyDescriptor,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, bool> {
        object_delegate!(
            @rooted self,
            internal_define_own_property,
            agent,
            property_key,
            property_descriptor,
            gc
        )
    }

    fn try_has_property<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        cache: Option<PropertyLookupCache>,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, TryHasResult<'gc>> {
        object_delegate!(self, try_has_property, agent, property_key, cache, gc)
    }

    fn internal_has_property<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, bool> {
        object_delegate!(@rooted self, internal_has_property, agent, property_key, gc)
    }

    fn try_get<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        receiver: Value,
        cache: Option<PropertyLookupCache>,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, TryGetResult<'gc>> {
        object_delegate!(self, try_get, agent, property_key, receiver, cache, gc)
    }

    fn internal_get<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        receiver: Value,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        object_delegate!(@rooted self, internal_get, agent, property_key, receiver, gc)
    }

    fn try_set<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        value: Value,
        receiver: Value,
        cache: Option<PropertyLookupCache>,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, SetResult<'gc>> {
        object_delegate!(
            self,
            try_set,
            agent,
            property_key,
            value,
            receiver,
            cache,
            gc
        )
    }

    fn internal_set<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        value: Value,
        receiver: Value,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, bool> {
        object_delegate!(@rooted self, internal_set, agent, property_key, value, receiver, gc)
    }

    fn try_delete<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, bool> {
        object_delegate!(self, try_delete, agent, property_key, gc)
    }

    fn internal_delete<'gc>(
        self,
        agent: &mut Agent,
        property_key: PropertyKey,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, bool> {
        object_delegate!(@rooted self, internal_delete, agent, property_key, gc)
    }

    fn try_own_property_keys<'gc>(
        self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, Vec<PropertyKey<'gc>>> {
        object_delegate!(self, try_own_property_keys, agent, gc)
    }

    fn internal_own_property_keys<'gc>(
        self,
        agent: &mut Agent,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Vec<PropertyKey<'gc>>> {
        object_delegate!(@rooted self, internal_own_property_keys, agent, gc)
    }

    fn get_own_property_at_offset<'gc>(
        self,
        agent: &Agent,
        offset: PropertyOffset,
        gc: NoGcScope<'gc, '_>,
    ) -> TryGetResult<'gc> {
        object_delegate!(self, get_own_property_at_offset, agent, offset, gc)
    }

    fn set_at_offset<'gc>(
        self,
        agent: &mut Agent,
        props: &SetAtOffsetProps,
        offset: PropertyOffset,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, SetResult<'gc>> {
        object_delegate!(self, set_at_offset, agent, props, offset, gc)
    }

    fn internal_call<'gc>(
        self,
        agent: &mut Agent,
        this_value: Value,
        arguments: ArgumentsList,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        if let Ok(f) = Function::try_from(self) {
            f.internal_call(agent, this_value, arguments, gc)
        } else {
            unreachable!()
        }
    }

    fn internal_construct<'gc>(
        self,
        agent: &mut Agent,
        arguments: ArgumentsList,
        new_target: Function,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Object<'gc>> {
        if let Ok(f) = Function::try_from(self) {
            f.internal_construct(agent, arguments, new_target, gc)
        } else {
            unreachable!()
        }
    }
}

impl HeapMarkAndSweep for Object<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        object_delegate!(self, mark_values, queues)
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        object_delegate!(self, sweep_values, compactions)
    }
}

impl HeapSweepWeakReference for Object<'static> {
    fn sweep_weak_reference(self, compactions: &CompactionLists) -> Option<Self> {
        match self {
            Self::Object(data) => data.sweep_weak_reference(compactions).map(Self::Object),
            Self::Array(data) => data.sweep_weak_reference(compactions).map(Self::Array),
            #[cfg(feature = "date")]
            Self::Date(data) => data.sweep_weak_reference(compactions).map(Self::Date),
            #[cfg(feature = "temporal")]
            Self::Instant(data) => data.sweep_weak_reference(compactions).map(Self::Instant),
            #[cfg(feature = "temporal")]
            Self::Duration(data) => data.sweep_weak_reference(compactions).map(Self::Duration),
            #[cfg(feature = "temporal")]
            Self::PlainTime(data) => data.sweep_weak_reference(compactions).map(Self::PlainTime),
            Self::Error(data) => data.sweep_weak_reference(compactions).map(Self::Error),
            Self::BoundFunction(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::BoundFunction),
            Self::BuiltinFunction(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::BuiltinFunction),
            Self::ECMAScriptFunction(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::ECMAScriptFunction),
            Self::BuiltinConstructorFunction(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::BuiltinConstructorFunction),
            Self::BuiltinPromiseResolvingFunction(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::BuiltinPromiseResolvingFunction),
            Self::BuiltinPromiseFinallyFunction(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::BuiltinPromiseFinallyFunction),
            Self::BuiltinProxyRevokerFunction => todo!(),
            Self::PrimitiveObject(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::PrimitiveObject),
            Self::Arguments(data) => data.sweep_weak_reference(compactions).map(Self::Arguments),
            Self::FinalizationRegistry(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::FinalizationRegistry),
            Self::Map(data) => data.sweep_weak_reference(compactions).map(Self::Map),
            Self::Promise(data) => data.sweep_weak_reference(compactions).map(Self::Promise),
            Self::Proxy(data) => data.sweep_weak_reference(compactions).map(Self::Proxy),
            #[cfg(feature = "regexp")]
            Self::RegExp(data) => data.sweep_weak_reference(compactions).map(Self::RegExp),
            #[cfg(feature = "set")]
            Self::Set(data) => data.sweep_weak_reference(compactions).map(Self::Set),
            #[cfg(feature = "weak-refs")]
            Self::WeakMap(data) => data.sweep_weak_reference(compactions).map(Self::WeakMap),
            #[cfg(feature = "weak-refs")]
            Self::WeakRef(data) => data.sweep_weak_reference(compactions).map(Self::WeakRef),
            #[cfg(feature = "weak-refs")]
            Self::WeakSet(data) => data.sweep_weak_reference(compactions).map(Self::WeakSet),
            #[cfg(feature = "array-buffer")]
            Self::ArrayBuffer(ab) => ab.sweep_weak_reference(compactions).map(Self::ArrayBuffer),
            #[cfg(feature = "array-buffer")]
            Self::DataView(dv) => dv.sweep_weak_reference(compactions).map(Self::DataView),
            #[cfg(feature = "array-buffer")]
            Self::Int8Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Int8Array),
            #[cfg(feature = "array-buffer")]
            Self::Uint8Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Uint8Array),
            #[cfg(feature = "array-buffer")]
            Self::Uint8ClampedArray(ta) => ta
                .sweep_weak_reference(compactions)
                .map(Self::Uint8ClampedArray),
            #[cfg(feature = "array-buffer")]
            Self::Int16Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Int16Array),
            #[cfg(feature = "array-buffer")]
            Self::Uint16Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Uint16Array),
            #[cfg(feature = "array-buffer")]
            Self::Int32Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Int32Array),
            #[cfg(feature = "array-buffer")]
            Self::Uint32Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Uint32Array),
            #[cfg(feature = "array-buffer")]
            Self::BigInt64Array(ta) => ta
                .sweep_weak_reference(compactions)
                .map(Self::BigInt64Array),
            #[cfg(feature = "array-buffer")]
            Self::BigUint64Array(ta) => ta
                .sweep_weak_reference(compactions)
                .map(Self::BigUint64Array),
            #[cfg(feature = "proposal-float16array")]
            Self::Float16Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Float16Array),
            #[cfg(feature = "array-buffer")]
            Self::Float32Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Float32Array),
            #[cfg(feature = "array-buffer")]
            Self::Float64Array(ta) => ta.sweep_weak_reference(compactions).map(Self::Float64Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedArrayBuffer(sab) => sab
                .sweep_weak_reference(compactions)
                .map(Self::SharedArrayBuffer),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedDataView(sdv) => sdv
                .sweep_weak_reference(compactions)
                .map(Self::SharedDataView),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedInt8Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedInt8Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedUint8Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedUint8Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedUint8ClampedArray(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedUint8ClampedArray),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedInt16Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedInt16Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedUint16Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedUint16Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedInt32Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedInt32Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedUint32Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedUint32Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedBigInt64Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedBigInt64Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedBigUint64Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedBigUint64Array),
            #[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
            Self::SharedFloat16Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedFloat16Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedFloat32Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedFloat32Array),
            #[cfg(feature = "shared-array-buffer")]
            Self::SharedFloat64Array(sta) => sta
                .sweep_weak_reference(compactions)
                .map(Self::SharedFloat64Array),
            Self::AsyncGenerator(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::AsyncGenerator),
            Self::ArrayIterator(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::ArrayIterator),
            #[cfg(feature = "set")]
            Self::SetIterator(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::SetIterator),
            Self::MapIterator(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::MapIterator),
            Self::StringIterator(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::StringIterator),
            #[cfg(feature = "regexp")]
            Self::RegExpStringIterator(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::RegExpStringIterator),
            Self::Generator(data) => data.sweep_weak_reference(compactions).map(Self::Generator),
            Self::Module(data) => data.sweep_weak_reference(compactions).map(Self::Module),
            Self::EmbedderObject(data) => data
                .sweep_weak_reference(compactions)
                .map(Self::EmbedderObject),
        }
    }
}

impl HeapMarkAndSweep for OrdinaryObject<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.objects.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.objects.shift_index(&mut self.0);
    }
}

impl HeapSweepWeakReference for OrdinaryObject<'static> {
    fn sweep_weak_reference(self, compactions: &CompactionLists) -> Option<Self> {
        compactions.objects.shift_weak_index(self.0).map(Self)
    }
}

impl<'a> CreateHeapData<ObjectRecord<'a>, OrdinaryObject<'a>> for Heap {
    fn create(&mut self, data: ObjectRecord<'a>) -> OrdinaryObject<'a> {
        self.objects.push(data.unbind());
        self.alloc_counter += core::mem::size_of::<ObjectRecord<'static>>();
        OrdinaryObject(BaseIndex::last(&self.objects))
    }
}

impl From<Object<'_>> for HeapRootData {
    #[inline]
    fn from(value: Object<'_>) -> Self {
        match value {
            Object::Object(d) => Self::from(d),
            Object::BoundFunction(d) => Self::from(d),
            Object::BuiltinFunction(d) => Self::from(d),
            Object::ECMAScriptFunction(d) => Self::from(d),
            Object::BuiltinConstructorFunction(d) => Self::from(d),
            Object::BuiltinPromiseResolvingFunction(d) => Self::from(d),
            Object::BuiltinPromiseFinallyFunction(d) => Self::from(d),
            Object::BuiltinProxyRevokerFunction => Self::BuiltinProxyRevokerFunction,
            Object::PrimitiveObject(d) => Self::from(d),
            Object::Arguments(d) => Self::from(d),
            Object::Array(d) => Self::from(d),
            #[cfg(feature = "date")]
            Object::Date(d) => Self::from(d),
            #[cfg(feature = "temporal")]
            Object::Duration(d) => Self::from(d),
            #[cfg(feature = "temporal")]
            Object::Instant(d) => Self::from(d),
            #[cfg(feature = "temporal")]
            Object::PlainTime(d) => Self::from(d),
            Object::Error(d) => Self::from(d),
            Object::FinalizationRegistry(d) => Self::from(d),
            Object::Map(d) => Self::from(d),
            Object::Promise(d) => Self::from(d),
            Object::Proxy(d) => Self::from(d),
            #[cfg(feature = "regexp")]
            Object::RegExp(d) => Self::from(d),
            #[cfg(feature = "set")]
            Object::Set(d) => Self::from(d),
            #[cfg(feature = "weak-refs")]
            Object::WeakMap(d) => Self::from(d),
            #[cfg(feature = "weak-refs")]
            Object::WeakRef(d) => Self::from(d),
            #[cfg(feature = "weak-refs")]
            Object::WeakSet(d) => Self::from(d),

            #[cfg(feature = "array-buffer")]
            Object::ArrayBuffer(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::DataView(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Int8Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Uint8Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Uint8ClampedArray(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Int16Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Uint16Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Int32Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Uint32Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::BigInt64Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::BigUint64Array(d) => Self::from(d),
            #[cfg(feature = "proposal-float16array")]
            Object::Float16Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Float32Array(d) => Self::from(d),
            #[cfg(feature = "array-buffer")]
            Object::Float64Array(d) => Self::from(d),

            #[cfg(feature = "shared-array-buffer")]
            Object::SharedArrayBuffer(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedDataView(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedInt8Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedUint8Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedUint8ClampedArray(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedInt16Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedUint16Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedInt32Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedUint32Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedBigInt64Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedBigUint64Array(d) => Self::from(d),
            #[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
            Object::SharedFloat16Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedFloat32Array(d) => Self::from(d),
            #[cfg(feature = "shared-array-buffer")]
            Object::SharedFloat64Array(d) => Self::from(d),

            Object::AsyncGenerator(d) => Self::from(d),
            Object::ArrayIterator(d) => Self::from(d),
            #[cfg(feature = "set")]
            Object::SetIterator(d) => Self::from(d),
            Object::MapIterator(d) => Self::from(d),
            Object::StringIterator(d) => Self::from(d),
            #[cfg(feature = "regexp")]
            Object::RegExpStringIterator(d) => Self::from(d),
            Object::Generator(d) => Self::from(d),
            Object::Module(d) => Self::from(d),
            Object::EmbedderObject(d) => Self::from(d),
        }
    }
}

impl TryFrom<HeapRootData> for Object<'_> {
    type Error = ();

    fn try_from(value: HeapRootData) -> Result<Self, ()> {
        match value {
            HeapRootData::Empty
            | HeapRootData::String(_)
            | HeapRootData::Symbol(_)
            | HeapRootData::Number(_)
            | HeapRootData::BigInt(_) => Err(()),
            HeapRootData::Object(o) => Ok(Self::from(o)),
            HeapRootData::BoundFunction(f) => Ok(Self::from(f)),
            HeapRootData::BuiltinFunction(f) => Ok(Self::from(f)),
            HeapRootData::ECMAScriptFunction(f) => Ok(Self::from(f)),
            HeapRootData::BuiltinConstructorFunction(f) => Ok(Self::BuiltinConstructorFunction(f)),
            HeapRootData::BuiltinPromiseResolvingFunction(f) => Ok(Self::from(f)),
            HeapRootData::BuiltinPromiseFinallyFunction(f) => Ok(Self::from(f)),
            HeapRootData::BuiltinProxyRevokerFunction => Ok(Self::BuiltinProxyRevokerFunction),
            HeapRootData::PrimitiveObject(o) => Ok(Self::from(o)),
            HeapRootData::Arguments(o) => Ok(Self::from(o)),
            HeapRootData::Array(o) => Ok(Self::from(o)),
            #[cfg(feature = "date")]
            HeapRootData::Date(o) => Ok(Self::from(o)),
            #[cfg(feature = "temporal")]
            HeapRootData::Instant(o) => Ok(Self::from(o)),
            #[cfg(feature = "temporal")]
            HeapRootData::Duration(o) => Ok(Self::from(o)),
            #[cfg(feature = "temporal")]
            HeapRootData::PlainTime(o) => Ok(Self::from(o)),
            HeapRootData::Error(o) => Ok(Self::from(o)),
            HeapRootData::FinalizationRegistry(o) => Ok(Self::from(o)),
            HeapRootData::Map(o) => Ok(Self::from(o)),
            HeapRootData::Promise(o) => Ok(Self::from(o)),
            HeapRootData::Proxy(o) => Ok(Self::from(o)),
            #[cfg(feature = "regexp")]
            HeapRootData::RegExp(o) => Ok(Self::from(o)),
            #[cfg(feature = "set")]
            HeapRootData::Set(o) => Ok(Self::from(o)),
            #[cfg(feature = "weak-refs")]
            HeapRootData::WeakMap(o) => Ok(Self::from(o)),
            #[cfg(feature = "weak-refs")]
            HeapRootData::WeakRef(o) => Ok(Self::from(o)),
            #[cfg(feature = "weak-refs")]
            HeapRootData::WeakSet(o) => Ok(Self::from(o)),

            #[cfg(feature = "array-buffer")]
            HeapRootData::ArrayBuffer(ab) => Ok(Self::from(ab)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::DataView(dv) => Ok(Self::from(dv)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Int8Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Uint8Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Uint8ClampedArray(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Int16Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Uint16Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Int32Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Uint32Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::BigInt64Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::BigUint64Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "proposal-float16array")]
            HeapRootData::Float16Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Float32Array(ta) => Ok(Self::from(ta)),
            #[cfg(feature = "array-buffer")]
            HeapRootData::Float64Array(ta) => Ok(Self::from(ta)),

            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedArrayBuffer(sab) => Ok(Self::from(sab)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedDataView(sdv) => Ok(Self::from(sdv)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedInt8Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedUint8Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedUint8ClampedArray(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedInt16Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedUint16Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedInt32Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedUint32Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedBigInt64Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedBigUint64Array(sta) => Ok(Self::from(sta)),
            #[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
            HeapRootData::SharedFloat16Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedFloat32Array(sta) => Ok(Self::from(sta)),
            #[cfg(feature = "shared-array-buffer")]
            HeapRootData::SharedFloat64Array(sta) => Ok(Self::from(sta)),

            HeapRootData::AsyncGenerator(o) => Ok(Self::from(o)),
            HeapRootData::ArrayIterator(o) => Ok(Self::from(o)),
            #[cfg(feature = "set")]
            HeapRootData::SetIterator(o) => Ok(Self::from(o)),
            HeapRootData::MapIterator(o) => Ok(Self::from(o)),
            HeapRootData::StringIterator(o) => Ok(Self::from(o)),
            #[cfg(feature = "regexp")]
            HeapRootData::RegExpStringIterator(o) => Ok(Self::from(o)),
            HeapRootData::Generator(o) => Ok(Self::from(o)),
            HeapRootData::Module(o) => Ok(Self::from(o)),
            HeapRootData::EmbedderObject(o) => Ok(Self::from(o)),
            HeapRootData::AwaitReaction(_)
            | HeapRootData::PromiseReaction(_)
            | HeapRootData::PromiseGroup(_)
            | HeapRootData::Executable(_)
            | HeapRootData::Realm(_)
            | HeapRootData::Script(_)
            | HeapRootData::SourceCode(_)
            | HeapRootData::SourceTextModule(_)
            | HeapRootData::DeclarativeEnvironment(_)
            | HeapRootData::FunctionEnvironment(_)
            | HeapRootData::GlobalEnvironment(_)
            | HeapRootData::ModuleEnvironment(_)
            | HeapRootData::ObjectEnvironment(_)
            | HeapRootData::PrivateEnvironment(_)
            | HeapRootData::PropertyLookupCache(_) => Err(()),
        }
    }
}

macro_rules! object_handle {
    ($name: tt) => {
        crate::ecmascript::object_handle!($name, $name);
    };
    ($name: ident, $variant: ident) => {
        crate::ecmascript::value_handle!($name, $variant);

        impl<'a> From<$name<'a>> for crate::ecmascript::Object<'a> {
            #[inline(always)]
            fn from(value: $name<'a>) -> Self {
                Self::$variant(value)
            }
        }

        impl<'a> TryFrom<crate::ecmascript::Object<'a>> for $name<'a> {
            type Error = ();

            #[inline]
            fn try_from(value: crate::ecmascript::Object<'a>) -> Result<Self, Self::Error> {
                match value {
                    crate::ecmascript::Object::$variant(data) => Ok(data),
                    _ => Err(()),
                }
            }
        }
    };
}
pub(crate) use object_handle;
