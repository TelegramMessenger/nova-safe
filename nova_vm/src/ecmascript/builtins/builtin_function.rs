// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use core::{marker::PhantomData, ops::Deref};
use std::borrow::Cow;

#[cfg(feature = "set")]
use crate::ecmascript::SetPrototype;
use crate::{
    ecmascript::{
        Agent, ArrayConstructor, ArrayPrototype, BUILTIN_STRING_MEMORY, BuiltinFunctionHeapData,
        ExceptionType, ExecutionContext, Function, FunctionInternalProperties, FunctionPrototype,
        GlobalObject, InternalSlots, JsResult, MapPrototype, MathObject, NumberConstructor,
        NumberPrototype, Object, ObjectPrototype, OrdinaryObject, PropertyKey, Realm,
        ScopedValuesIterator, String, StringConstructor, StringPrototype, Value, function_handle,
    },
    engine::{Bindable, GcScope, HeapRootCollection, NoGcScope, ScopedArgumentsData},
    heap::{
        ArenaAccess, ArenaAccessMut, BaseIndex, CompactionLists, CreateHeapData, Heap,
        HeapMarkAndSweep, HeapSweepWeakReference, IntrinsicConstructorIndexes,
        IntrinsicFunctionIndexes, ObjectEntry, ObjectEntryPropertyDescriptor, WorkQueues,
        arena_vec_access,
    },
    ndt,
};

/// A list of [ECMAScript language values](Value).
///
/// Garbage collection invalidates the [`Value`](Value)s in the list but does
/// not automatically make it impossible to extract them from it. Thus, when
/// received as a parameter the list should immediately be [bound](Bindable) to
/// the [`GcScope`](GcScope)'s GC lifetime to ensure that it is not used after
/// garbage collection.
#[derive(Default)]
#[repr(transparent)]
pub struct ArgumentsList<'slice, 'value> {
    slice: &'slice mut [Value<'static>],
    value: PhantomData<Value<'value>>,
}

impl core::fmt::Debug for ArgumentsList<'_, '_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.slice.fmt(f)
    }
}

impl<'slice, 'value> ArgumentsList<'slice, 'value> {
    /// Create an ArgumentsList from a single Value.
    pub(crate) fn from_mut_value(value: &'slice mut Value<'static>) -> Self {
        Self {
            slice: core::slice::from_mut(value),
            value: PhantomData,
        }
    }

    /// Create an ArgumentsList from a Value slice.
    pub(crate) fn from_mut_slice(slice: &'slice mut [Value<'static>]) -> Self {
        Self {
            slice,
            value: PhantomData,
        }
    }

    /// Temporarily store the list on the [`Agent`](Agent) heap for the duration
    /// of a callback, ensuring that garbage collection does not invalidate any
    /// of the [`Value`](Value)s within.
    ///
    /// The list is copied onto the heap and the values that the callback did
    /// not consume are copied back afterwards, so garbage collection during the
    /// callback updates them. When the call returns, this list holds only
    /// those values.
    ///
    /// See [`ScopedArgumentsList`](ScopedArgumentsList) for accessing the
    /// [`Value`](Value)s within the callback.
    pub fn with_scoped<'a, R>(
        &mut self,
        agent: &mut Agent,
        work: impl FnOnce(&mut Agent, ScopedArgumentsList<'_>, GcScope<'a, '_>) -> R,
        mut gc: GcScope<'a, '_>,
    ) -> R
    where
        R: 'a,
    {
        let slice = core::mem::take(&mut self.slice);
        let values = take_pooled_arguments_vec(agent, slice);
        let (stack_refs_len, index) = {
            let stack_refs_len = agent.stack_refs.borrow().len();
            let mut stack_ref_collections = agent.stack_ref_collections.borrow_mut();
            let index = u32::try_from(stack_ref_collections.len())
                .expect("Stack reference collections overflowed");
            stack_ref_collections.push(HeapRootCollection::ArgumentsList(ScopedArgumentsData {
                values,
                start: 0,
            }));
            (stack_refs_len, index)
        };
        let result = {
            let sub_gc = gc.subscope();
            let scoped_args = ScopedArgumentsList::new(index, sub_gc.nogc());
            work(agent, scoped_args, sub_gc)
        };
        agent.stack_refs.borrow_mut().truncate(stack_refs_len);
        let ScopedArgumentsData { values, start } = {
            let mut stack_ref_collections = agent.stack_ref_collections.borrow_mut();
            let data = match stack_ref_collections
                .get_mut(index as usize)
                .map(|slot| core::mem::replace(slot, HeapRootCollection::Empty))
            {
                Some(HeapRootCollection::ArgumentsList(data)) => data,
                _ => panic!("ArgumentsList::with_scoped: the arguments slot was replaced"),
            };
            stack_ref_collections.truncate(index as usize);
            data
        };
        let tail = slice
            .split_at_mut_checked(start)
            .map_or(&mut [][..], |(_, tail)| tail);
        let remaining = values.get(start..).unwrap_or(&[]);
        let copied = tail.len().min(remaining.len());
        tail[..copied].copy_from_slice(&remaining[..copied]);
        self.slice = tail;
        return_pooled_arguments_vec(agent, values);
        result
    }

    pub(crate) fn slice_from(self, start: usize) -> ArgumentsList<'slice, 'value> {
        ArgumentsList {
            slice: self
                .slice
                .split_at_mut_checked(start)
                .map_or(&mut [], |(_, tail)| tail),
            value: PhantomData,
        }
    }

    /// Access the Values in an ArgumentsList as a slice.
    pub fn as_slice(&self) -> &[Value<'value>] {
        self.slice
    }

    /// Access the Values in an ArgumentsList as a mut slice.
    pub(crate) fn as_mut_slice(&mut self) -> &mut [Value<'static>] {
        self.slice
    }

    /// Get a Value by index from an ArgumentsList.
    ///
    /// If a Value with that index isn't present, `undefined` is returned.
    ///
    /// If the list itself has not been [bound](Bindable) then the returned
    /// Value should immediately be bound.
    #[inline]
    pub fn get(&self, index: usize) -> Value<'value> {
        *self.slice.get(index).unwrap_or(&Value::Undefined)
    }

    /// Get a Value by index from an ArgumentsList if present.
    ///
    /// If a Value with that index isn't present, None.
    ///
    /// If the list itself has not been [bound](Bindable) then the returned
    /// Value should immediately be bound.
    #[inline]
    pub fn get_if_present(&self, index: usize) -> Option<Value<'value>> {
        self.slice.get(index).copied()
    }
}

impl<'slice> Bindable for ArgumentsList<'slice, '_> {
    type Of<'a> = ArgumentsList<'slice, 'a>;

    #[inline(always)]
    fn unbind(self) -> Self::Of<'static> {
        ArgumentsList {
            slice: self.slice,
            value: PhantomData,
        }
    }

    #[inline(always)]
    fn bind<'a>(self, _gc: NoGcScope<'a, '_>) -> Self::Of<'a> {
        ArgumentsList {
            slice: self.slice,
            value: PhantomData,
        }
    }
}

impl<'value> Deref for ArgumentsList<'_, 'value> {
    type Target = [Value<'value>];

    fn deref(&self) -> &Self::Target {
        self.slice
    }
}

const ARGUMENTS_POOL_LEN: usize = 16;
const ARGUMENTS_POOL_MAX_CAPACITY: usize = 64;

pub(crate) fn take_pooled_arguments_vec(
    agent: &mut Agent,
    values: &[Value<'static>],
) -> Vec<Value<'static>> {
    if values.is_empty() {
        return Vec::new();
    }
    let mut vec = agent.arguments_pool.pop().unwrap_or_default();
    vec.extend_from_slice(values);
    vec
}

/// Roots `values` as the arguments list of a call that a `Vm` runs in place:
/// the list is the last of the Agent's scoped collections, and stays until
/// [`release_scoped_collections`] cuts them back below it when the call
/// ends. What [`ArgumentsList::with_scoped`] does for a call that has a
/// native frame to do it in.
pub(crate) fn scope_arguments_of_call(
    agent: &mut Agent,
    values: Vec<Value<'static>>,
) -> ScopedArgumentsList<'static> {
    let collections = agent.stack_ref_collections.get_mut();
    let index = u32::try_from(collections.len()).expect("Stack reference collections overflowed");
    collections.push(HeapRootCollection::ArgumentsList(ScopedArgumentsData {
        values,
        start: 0,
    }));
    ScopedArgumentsList {
        index,
        value: PhantomData,
    }
}

/// Cuts the Agent's scoped collections back to `depth`. The vector of an
/// arguments list among those let go of ([`scope_arguments_of_call`]) is
/// kept for the next call.
#[inline]
pub(crate) fn release_scoped_collections(agent: &mut Agent, depth: usize) {
    while agent.stack_ref_collections.get_mut().len() > depth {
        if let Some(HeapRootCollection::ArgumentsList(data)) =
            agent.stack_ref_collections.get_mut().pop()
        {
            return_pooled_arguments_vec(agent, data.values);
        }
    }
}

pub(crate) fn return_pooled_arguments_vec(agent: &mut Agent, mut vec: Vec<Value<'static>>) {
    if vec.capacity() == 0
        || vec.capacity() > ARGUMENTS_POOL_MAX_CAPACITY
        || agent.arguments_pool.len() >= ARGUMENTS_POOL_LEN
    {
        return;
    }
    vec.clear();
    agent.arguments_pool.push(vec);
}

/// Arguments list stored temporarily on the Agent's heap.
///
/// The arguments can be accessed through the Agent for the duration of the
/// function call.
pub struct ScopedArgumentsList<'scope> {
    index: u32,
    value: PhantomData<Value<'scope>>,
}

impl<'scope> ScopedArgumentsList<'scope> {
    pub(crate) fn new(index: u32, _: NoGcScope<'_, 'scope>) -> Self {
        Self {
            index,
            value: PhantomData,
        }
    }

    /// The same list with its scope lifetime erased.
    ///
    /// ## Correctness
    ///
    /// The list is an index into the Agent's scoped collections: after its
    /// `with_scoped` call returns, using it panics or reads another call's
    /// arguments.
    pub(crate) fn into_static(self) -> ScopedArgumentsList<'static> {
        ScopedArgumentsList {
            index: self.index,
            value: PhantomData,
        }
    }

    fn with_data<R>(&self, agent: &Agent, f: impl FnOnce(&ScopedArgumentsData) -> R) -> R {
        let stack_ref_collections = agent.stack_ref_collections.borrow();
        let Some(HeapRootCollection::ArgumentsList(data)) =
            stack_ref_collections.get(self.index as usize)
        else {
            panic!("ScopedArgumentsList used outside of its with_scoped call");
        };
        f(data)
    }

    fn with_data_mut<R>(&self, agent: &Agent, f: impl FnOnce(&mut ScopedArgumentsData) -> R) -> R {
        let mut stack_ref_collections = agent.stack_ref_collections.borrow_mut();
        let Some(HeapRootCollection::ArgumentsList(data)) =
            stack_ref_collections.get_mut(self.index as usize)
        else {
            panic!("ScopedArgumentsList used outside of its with_scoped call");
        };
        f(data)
    }

    /// Get a Value by index from an ArgumentsList.
    ///
    /// If a Value with that index isn't present, `undefined` is returned.
    ///
    /// If the list itself has not been [bound](Bindable) then the returned
    /// Value should immediately be bound.
    pub fn get<'gc>(&self, agent: &Agent, index: u32, gc: NoGcScope<'gc, '_>) -> Value<'gc> {
        self.with_data(agent, |data| {
            data.remaining()
                .get(index as usize)
                .copied()
                .unwrap_or(Value::Undefined)
                .bind(gc)
        })
    }

    /// Get the number of provided arguments.
    pub fn len(&self, agent: &Agent) -> usize {
        self.with_data(agent, |data| data.remaining().len())
    }

    pub(crate) fn unshift<'gc>(&self, agent: &Agent, gc: NoGcScope<'gc, '_>) -> Option<Value<'gc>> {
        self.with_data_mut(agent, |data| {
            let first = data.values.get(data.start).copied()?;
            data.start += 1;
            Some(first.bind(gc))
        })
    }

    pub(crate) fn iter(&self, agent: &mut Agent) -> ScopedValuesIterator<'_> {
        self.with_data(agent, |data| {
            ScopedValuesIterator::new(self.index, data.start, data.values.len())
        })
    }

    /// Call `f` with the remaining arguments as a slice.
    ///
    /// The values are taken off the heap for the duration of the call, so
    /// `f` must not trigger garbage collection (hence the `NoGcScope`) nor
    /// access this list.
    pub(crate) fn with_slice<R>(
        &self,
        agent: &mut Agent,
        _gc: NoGcScope<'_, '_>,
        f: impl FnOnce(&mut Agent, &[Value<'static>]) -> R,
    ) -> R {
        let data = self.with_data_mut(agent, core::mem::take);
        let result = f(agent, data.remaining());
        self.with_data_mut(agent, |slot| *slot = data);
        result
    }
}

impl core::fmt::Debug for ScopedArgumentsList<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "ScopedArgumentsList({})", self.index)
    }
}

/// Calling convention of a builtin method.
pub type RegularFn =
    for<'gc> fn(&mut Agent, Value, ArgumentsList, GcScope<'gc, '_>) -> JsResult<'gc, Value<'gc>>;
/// Calling convention of a builtin constructor.
pub type ConstructorFn = for<'gc> fn(
    &mut Agent,
    Value,
    ArgumentsList,
    Option<Object>,
    GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>>;

/// A built-in function that, for a this value and arguments it takes as they
/// are, runs no script, throws nothing and needs no collection: the
/// interpreter runs it in place for those, with no call made. Most only
/// compute a value; an Array's `push` and `pop` also do what the call would
/// have done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectBuiltin {
    MathAbs,
    MathCeil,
    MathFloor,
    MathMax,
    MathMin,
    MathRound,
    MathSign,
    MathSqrt,
    MathTrunc,
    MathSin,
    MathCos,
    MathPow,
    MathRandom,
    DateNow,
    /// `isNaN` and `isFinite` of the global object, of a Number.
    IsNaN,
    IsFinite,
    /// The four of the Number constructor that ask what a value is, of
    /// any value: none converts it.
    NumberIsFinite,
    NumberIsInteger,
    NumberIsNaN,
    NumberIsSafeInteger,
    /// `f.call(thisArg, ...)` of a function that is one of these.
    FunctionCall,
    ObjectHasOwnProperty,
    /// `String(value)` and `Number(value)`, called and not constructed.
    StringCall,
    NumberCall,
    NumberToFixed,
    StringRepeat,
    StringPadStart,
    StringPadEnd,
    StringCharCodeAt,
    StringCharAt,
    StringSlice,
    StringSubstring,
    StringStartsWith,
    StringEndsWith,
    StringIndexOf,
    StringIncludes,
    ArrayPush,
    ArrayPop,
    ArrayIsArray,
    MapGet,
    MapHas,
    MapSet,
    SetHas,
    SetAdd,
}

impl DirectBuiltin {
    /// The function of `behaviour`, if it is one of these.
    pub(crate) fn of(behaviour: Behaviour) -> Option<Self> {
        match behaviour {
            Behaviour::Regular(function) => MathObject::direct_builtin(function)
                .or_else(|| StringPrototype::direct_builtin(function))
                .or_else(|| ArrayPrototype::direct_builtin(function))
                .or_else(|| ArrayConstructor::is_is_array(function).then_some(Self::ArrayIsArray))
                .or_else(|| MapPrototype::direct_builtin(function))
                .or_else(|| Self::of_set(function))
                .or_else(|| Self::of_date(function))
                .or_else(|| FunctionPrototype::is_call(function).then_some(Self::FunctionCall))
                .or_else(|| {
                    ObjectPrototype::is_has_own_property(function)
                        .then_some(Self::ObjectHasOwnProperty)
                })
                .or_else(|| NumberPrototype::is_to_fixed(function).then_some(Self::NumberToFixed))
                .or_else(|| NumberConstructor::direct_builtin(function))
                .or_else(|| GlobalObject::direct_builtin(function)),
            Behaviour::Constructor(function) => {
                if StringConstructor::is_constructor(function) {
                    Some(Self::StringCall)
                } else if NumberConstructor::is_constructor(function) {
                    Some(Self::NumberCall)
                } else {
                    None
                }
            }
        }
    }

    #[cfg(feature = "set")]
    fn of_set(function: RegularFn) -> Option<Self> {
        if SetPrototype::is_has(function) {
            Some(Self::SetHas)
        } else if SetPrototype::is_add(function) {
            Some(Self::SetAdd)
        } else {
            None
        }
    }

    #[cfg(not(feature = "set"))]
    fn of_set(_: RegularFn) -> Option<Self> {
        None
    }

    #[cfg(feature = "date")]
    fn of_date(function: RegularFn) -> Option<Self> {
        crate::ecmascript::DateConstructor::is_now(function).then_some(Self::DateNow)
    }

    #[cfg(not(feature = "date"))]
    fn of_date(_: RegularFn) -> Option<Self> {
        None
    }

    /// Whether the function takes its this value: the others are run
    /// without looking for it.
    #[inline]
    pub(crate) fn takes_this(self) -> bool {
        matches!(
            self,
            Self::StringCharCodeAt
                | Self::StringCharAt
                | Self::StringSlice
                | Self::StringSubstring
                | Self::StringStartsWith
                | Self::StringEndsWith
                | Self::StringIndexOf
                | Self::StringIncludes
                | Self::ArrayPush
                | Self::ArrayPop
                | Self::StringRepeat
                | Self::StringPadStart
                | Self::StringPadEnd
                | Self::NumberToFixed
                | Self::MapGet
                | Self::MapHas
                | Self::MapSet
                | Self::SetHas
                | Self::SetAdd
                | Self::FunctionCall
                | Self::ObjectHasOwnProperty
        )
    }

    /// Whether what the function does depends on the realm it runs in. A
    /// call of a function runs it in the function's realm, and no call is
    /// made here: such a function is run only where its realm is the
    /// running one ([`DirectBuiltin::run`]). `push` asks the running realm
    /// for %Array.prototype% and %Object.prototype%, which an Array with
    /// no properties of its own inherits from
    /// ([`Array::appends_plainly`](crate::ecmascript::Array)): the `push`
    /// of another realm, called on an Array of that realm, found none of
    /// what that realm's prototypes have under an index, a setter among
    /// it. A function that asks the realm for anything belongs here.
    #[inline(always)]
    const fn asks_realm(self) -> bool {
        matches!(self, Self::ArrayPush)
    }

    /// What a call of `function`, which is this one, with `this` (where
    /// the function takes it) and `arguments` returns, with what the call
    /// does done, where it needs nothing but them; None where the call is
    /// to be made, with nothing done.
    #[inline]
    pub(crate) fn run<'gc>(
        self,
        agent: &mut Agent,
        function: BuiltinFunction<'gc>,
        this: Option<Value<'gc>>,
        arguments: &[Value<'gc>],
        gc: NoGcScope<'gc, '_>,
    ) -> Option<Value<'gc>> {
        if self != Self::FunctionCall {
            return self.run_in_realm_of(agent, function, this, arguments, gc);
        }
        // The function is the this value of `call`, and its this value the
        // first argument.
        let Value::BuiltinFunction(function) = this? else {
            return None;
        };
        let called = function.get(agent).direct?;
        if called == Self::FunctionCall {
            return None;
        }
        let (this, arguments) = match arguments.split_first() {
            Some((this, arguments)) => (*this, arguments),
            None => (Value::Undefined, arguments),
        };
        // `call` of whichever realm calls the function in the function's.
        called.run_in_realm_of(agent, function, Some(this), arguments, gc)
    }

    /// [`DirectBuiltin::run_own`] where running `function` here is running
    /// it in its realm.
    #[inline(always)]
    fn run_in_realm_of<'gc>(
        self,
        agent: &mut Agent,
        function: BuiltinFunction<'gc>,
        this: Option<Value<'gc>>,
        arguments: &[Value<'gc>],
        gc: NoGcScope<'gc, '_>,
    ) -> Option<Value<'gc>> {
        if self.asks_realm() && function.get(agent).realm != agent.current_realm(gc) {
            return None;
        }
        self.run_own(agent, this, arguments, gc)
    }

    /// [`DirectBuiltin::run`] of a function that is not
    /// Function.prototype.call.
    #[inline]
    fn run_own<'gc>(
        self,
        agent: &mut Agent,
        this: Option<Value<'gc>>,
        arguments: &[Value<'gc>],
        gc: NoGcScope<'gc, '_>,
    ) -> Option<Value<'gc>> {
        match self {
            Self::StringCharCodeAt
            | Self::StringCharAt
            | Self::StringSlice
            | Self::StringSubstring
            | Self::StringStartsWith
            | Self::StringEndsWith
            | Self::StringIndexOf
            | Self::StringIncludes
            | Self::StringRepeat
            | Self::StringPadStart
            | Self::StringPadEnd => {
                let string = String::try_from(this?).ok()?;
                StringPrototype::run_direct(agent, self, string, arguments, gc)
            }
            Self::IsNaN | Self::IsFinite => {
                GlobalObject::is_one_directly(agent, self, arguments).map(Value::from)
            }
            Self::NumberIsFinite
            | Self::NumberIsInteger
            | Self::NumberIsNaN
            | Self::NumberIsSafeInteger => {
                let value = arguments.first().copied().unwrap_or(Value::Undefined);
                Some(NumberConstructor::is_one_directly(agent, self, value).into())
            }
            Self::StringCall => StringConstructor::call_directly(agent, arguments, gc),
            Self::NumberCall => NumberConstructor::call_directly(agent, arguments, gc),
            Self::NumberToFixed => NumberPrototype::to_fixed_directly(agent, this?, arguments, gc),
            Self::MapSet => {
                let Value::Map(map) = this? else {
                    return None;
                };
                let key = arguments.first().copied().unwrap_or(Value::Undefined);
                let value = arguments.get(1).copied().unwrap_or(Value::Undefined);
                MapPrototype::set_of(agent, map, key, value);
                Some(map.into())
            }
            Self::ArrayPush | Self::ArrayPop => {
                ArrayPrototype::run_direct(agent, self, this?, arguments, gc)
            }
            // IsArray of anything but a Proxy, which may have been revoked.
            Self::ArrayIsArray => match arguments.first() {
                Some(Value::Array(_)) => Some(true.into()),
                Some(Value::Proxy(_)) => None,
                _ => Some(false.into()),
            },
            Self::MapGet | Self::MapHas => {
                let Value::Map(map) = this? else {
                    return None;
                };
                let key = arguments.first().copied().unwrap_or(Value::Undefined);
                Some(if self == Self::MapGet {
                    MapPrototype::get_of(agent, map, key, gc)
                } else {
                    MapPrototype::has_of(agent, map, key).into()
                })
            }
            Self::SetHas | Self::SetAdd => Self::set_has_or_add(agent, self, this?, arguments),
            Self::ObjectHasOwnProperty => {
                ObjectPrototype::has_own_property_directly(agent, this?, arguments, gc)
            }
            Self::DateNow => Self::date_now(),
            Self::FunctionCall => None,
            _ => MathObject::run_direct(agent, self, arguments, gc),
        }
    }

    #[cfg(feature = "set")]
    fn set_has_or_add<'gc>(
        agent: &mut Agent,
        direct: Self,
        this: Value<'gc>,
        arguments: &[Value<'gc>],
    ) -> Option<Value<'gc>> {
        let Value::Set(set) = this else {
            return None;
        };
        let value = arguments.first().copied().unwrap_or(Value::Undefined);
        if direct == Self::SetAdd {
            SetPrototype::add_of(agent, set, value);
            return Some(set.into());
        }
        Some(SetPrototype::has_of(agent, set, value).into())
    }

    #[cfg(not(feature = "set"))]
    fn set_has_or_add<'gc>(
        _: &mut Agent,
        _: Self,
        _: Value<'gc>,
        _: &[Value<'gc>],
    ) -> Option<Value<'gc>> {
        None
    }

    #[cfg(feature = "date")]
    fn date_now<'gc>() -> Option<Value<'gc>> {
        Some(crate::ecmascript::DateConstructor::now_value())
    }

    #[cfg(not(feature = "date"))]
    fn date_now<'gc>() -> Option<Value<'gc>> {
        None
    }
}

/// Calling convention of a builtin function.
#[allow(unpredictable_function_pointer_comparisons)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behaviour {
    /// Regular JavaScript function not callable with the `new` keyword.
    Regular(RegularFn),
    /// JavaScript function callable with the `new` keyword, and possibly
    /// without it.
    Constructor(ConstructorFn),
}

impl Behaviour {
    pub(crate) fn is_constructor(&self) -> bool {
        matches!(self, Behaviour::Constructor(_))
    }
}

/// Helper trait for defining builtin functions using the
/// [`BuiltinFunctionBuilder`].
///
/// [`BuiltinFunctionBuilder`]: crate::ecmascript::builders::BuiltinFunctionBuilder
pub trait Builtin {
    /// Name of the function
    ///
    /// This currently has to be a statically knowable name, which makes this
    /// trait mostly useless for embedders. This name also controls the property
    /// key that this function is assigned to if used to create a property.
    const NAME: String<'static>;
    /// Length of the function
    ///
    /// This is reported into JavaScript when the function object's `length`
    /// property is read.
    const LENGTH: u8;
    /// Behaviour of the function
    ///
    /// A builtin function can either be a normal function (not callable using
    /// `new ff`) or a constructor function (callable using `new f`). The enum
    /// choice here dictates which one it is, and the function pointer dictates
    /// the Rust code to run when JavaScript calls the function.
    const BEHAVIOUR: Behaviour;

    /// If the builtin function is created as a property then setting this to
    /// Some can be used to override the property key is this is assigned to. By
    /// default the `NAME` value is used.
    const KEY: Option<PropertyKey<'static>> = None;

    /// If the builtin function is created as a property then this controls the
    /// property's `[[Writable]]` value.
    const WRITABLE: bool = true;
    /// If the builtin function is created as a property then this controls the
    /// property's `[[Enumerable]]` value.
    const ENUMERABLE: bool = false;
    /// If the builtin function is created as a property then this controls the
    /// property's `[[Configurable]]` value.
    const CONFIGURABLE: bool = true;
}

pub(crate) trait BuiltinIntrinsicConstructor: Builtin {
    const INDEX: IntrinsicConstructorIndexes;
}

pub(crate) trait BuiltinIntrinsic: Builtin {
    const INDEX: IntrinsicFunctionIndexes;
}
/// Helper trait for defining builtin getter function properties.
pub trait BuiltinGetter: Builtin {
    /// Accessor property's getter function's `name` property value.
    const GETTER_NAME: String<'static> = Self::NAME;
    /// Accessor property's getter function's call behaviour.
    const GETTER_BEHAVIOUR: Behaviour = Self::BEHAVIOUR;
}
/// Helper trait for defining builtin setter function properties.
pub trait BuiltinSetter: Builtin {
    /// Accessor property's setter function's `name` property value.
    const SETTER_NAME: String<'static> = Self::NAME;
    /// Accessor property's setter function's call behaviour.
    const SETTER_BEHAVIOUR: Behaviour = Self::BEHAVIOUR;
}

/// Builtin function creation arguments.
#[derive(Debug, Default)]
pub struct BuiltinFunctionArgs<'a> {
    /// The builtin function's `length` property initial value.
    pub length: u32,
    /// The builtin function's `name` property initial value.
    pub name: &'static str,
    /// The builtin function's Realm. Defaults to the current Realm.
    pub realm: Option<Realm<'a>>,
    /// The builtin function's prototype. Defaults to the current Realm's
    /// `Function.prototype`
    pub prototype: Option<Object<'a>>,
    /// An optional prefix for the builtin function's name.
    pub prefix: Option<&'static str>,
}

impl<'a> BuiltinFunctionArgs<'a> {
    /// Create new builtin function creation arguments with length and name.
    pub fn new(length: u32, name: &'static str) -> Self {
        Self {
            length,
            name,
            ..Default::default()
        }
    }

    /// Create new builtin function creation arguments with length, name, and a realm.
    pub fn new_with_realm(length: u32, name: &'static str, realm: Realm<'a>) -> Self {
        Self {
            length,
            name,
            realm: Some(realm),
            ..Default::default()
        }
    }
}

/// ## [10.3 Built-in Function Objects](https://tc39.es/ecma262/#sec-built-in-function-objects)
///
/// A built-in function object is an ordinary object; it must satisfy the
/// requirements for ordinary objects set out in [10.1].
///
/// In addition to the internal slots required of every ordinary object (see
/// [10.1]), a built-in function object must also have the following internal
/// slots:
///
/// * \[\[Realm]], a Realm Record that represents the realm in which the
///   function was created.
/// * \[\[InitialName]], a String that is the initial name of the function. It
///   is used by [20.2.3.5].
/// * \[\[Async]], a Boolean that indicates whether the function has async
///   function call and construct behaviour in BuiltinCallOrConstruct.
///
/// [10.1]: https://tc39.es/ecma262/#sec-ordinary-object-internal-methods-and-internal-slots
/// [20.2.3.5]: https://tc39.es/ecma262/#sec-function.prototype.tostring
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct BuiltinFunction<'a>(BaseIndex<'a, BuiltinFunctionHeapData<'static>>);
arena_vec_access!(
    BuiltinFunction,
    'a,
    BuiltinFunctionHeapData,
    builtin_functions
);

impl<'f> BuiltinFunction<'f> {
    /// Allocate a a new uninitialised (None) BuiltinFunction and return its reference.
    pub(crate) fn new_uninitialised(agent: &mut Agent) -> Self {
        agent
            .heap
            .builtin_functions
            .push(BuiltinFunctionHeapData::BLANK);
        BuiltinFunction(BaseIndex::last(&agent.heap.builtin_functions))
    }

    /// Returns `true` if the builtin function is also callable as a constructor
    /// function.
    pub fn is_constructor(self, agent: &Agent) -> bool {
        // A builtin function has the [[Construct]] method if its behaviour is
        // a constructor behaviour.
        self.get(agent).behaviour.is_constructor()
    }

    /// ### \[\[Realm]]
    pub fn realm(self, agent: &Agent) -> Realm<'f> {
        self.get(agent).realm
    }
}
function_handle!(BuiltinFunction);

impl IntrinsicFunctionIndexes {
    pub(crate) const fn get_builtin_function<'a>(
        self,
        base: BaseIndex<'a, BuiltinFunctionHeapData<'static>>,
    ) -> BuiltinFunction<'a> {
        BuiltinFunction(BaseIndex::from_index_u32_const(
            self as u32 + base.get_index_u32_const() + Self::BUILTIN_FUNCTION_INDEX_OFFSET,
        ))
    }
}

impl IntrinsicConstructorIndexes {
    pub(crate) const fn get_builtin_function<'a>(
        self,
        base: BaseIndex<'a, BuiltinFunctionHeapData<'static>>,
    ) -> BuiltinFunction<'a> {
        BuiltinFunction(BaseIndex::from_index_u32_const(
            self as u32 + base.get_index_u32_const() + Self::BUILTIN_FUNCTION_INDEX_OFFSET,
        ))
    }
}

impl<'a> FunctionInternalProperties<'a> for BuiltinFunction<'a> {
    fn get_name(self, agent: &Agent) -> &String<'a> {
        self.get(agent)
            .initial_name
            .as_ref()
            .unwrap_or(&String::EMPTY_STRING)
    }

    fn get_length(self, agent: &Agent) -> u8 {
        self.get(agent).length
    }

    #[inline(always)]
    fn get_function_backing_object(self, agent: &Agent) -> Option<OrdinaryObject<'static>> {
        self.get(agent).object_index.unbind()
    }

    fn set_function_backing_object(
        self,
        agent: &mut Agent,
        backing_object: OrdinaryObject<'static>,
    ) {
        assert!(
            self.get_mut(agent)
                .object_index
                .replace(backing_object.unbind())
                .is_none()
        );
    }

    /// ### [10.3.1 \[\[Call\]\] ( thisArgument, argumentsList )](https://tc39.es/ecma262/#sec-built-in-function-objects-call-thisargument-argumentslist)
    ///
    /// The [[Call]] internal method of a built-in function object F takes
    /// arguments thisArgument (an ECMAScript language value) and argumentsList
    /// (a List of ECMAScript language values) and returns either a normal
    /// completion containing an ECMAScript language value or a throw
    /// completion.
    #[cfg_attr(feature = "probes", allow(unsafe_code))]
    fn function_call<'gc>(
        self,
        agent: &mut Agent,
        this_argument: Value,
        arguments_list: ArgumentsList,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        let mut id = 0;
        ndt::builtin_call_start!(|| {
            let args = create_name_and_id(agent, self);
            id = args.1;
            args
        });
        // 1. Return ? BuiltinCallOrConstruct(F, thisArgument, argumentsList, undefined).
        let result =
            builtin_call_or_construct(agent, self, Some(this_argument), arguments_list, None, gc);
        ndt::builtin_call_done!(|| id);
        result
    }

    /// ### [10.3.2 \[\[Construct\]\] ( argumentsList, newTarget )](https://tc39.es/ecma262/#sec-built-in-function-objects-construct-argumentslist-newtarget)
    ///
    /// The [[Construct]] internal method of a built-in function object F (when
    /// the method is present) takes arguments argumentsList (a List of
    /// ECMAScript language values) and newTarget (a constructor) and returns
    /// either a normal completion containing an Object or a throw completion.
    #[cfg_attr(feature = "probes", allow(unsafe_code))]
    fn function_construct<'gc>(
        self,
        agent: &mut Agent,
        arguments_list: ArgumentsList,
        new_target: Function,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Object<'gc>> {
        let mut id = 0;
        ndt::builtin_constructor_start!(|| {
            let args = create_name_and_id(agent, self);
            id = args.1;
            args
        });
        // 1. Return ? BuiltinCallOrConstruct(F, uninitialized, argumentsList, newTarget).
        let result =
            builtin_call_or_construct(agent, self, None, arguments_list, Some(new_target), gc)
                .map(|result| result.try_into().unwrap());
        ndt::builtin_constructor_done!(|| id);
        result
    }
}

#[inline(never)]
fn create_name_and_id<'a>(agent: &'a Agent, f: BuiltinFunction<'a>) -> (Cow<'a, str>, u64) {
    let id = match f.get(agent).behaviour {
        Behaviour::Regular(f) => f as usize as u64,
        Behaviour::Constructor(f) => f as usize as u64,
    };
    let name = f.get_name(agent).to_string_lossy_(agent);
    (name, id)
}

/// ### [10.3.3 BuiltinCallOrConstruct ( F, thisArgument, argumentsList, newTarget )](https://tc39.es/ecma262/#sec-builtincallorconstruct)
///
/// The abstract operation BuiltinCallOrConstruct takes arguments F (a built-in
/// function object), thisArgument (an ECMAScript language value or
/// uninitialized), argumentsList (a List of ECMAScript language values), and
/// newTarget (a constructor or undefined) and returns either a normal
/// completion containing an ECMAScript language value or a throw completion.
fn builtin_call_or_construct<'gc>(
    agent: &mut Agent,
    f: BuiltinFunction,
    this_argument: Option<Value>,
    arguments_list: ArgumentsList,
    new_target: Option<Function>,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    agent.check_call_depth(gc.nogc()).unbind()?;

    let f = f.bind(gc.nogc());
    let this_argument = this_argument.bind(gc.nogc());
    let arguments_list = arguments_list.bind(gc.nogc());
    let new_target = new_target.bind(gc.nogc());
    // 1. Let callerContext be the running execution context.
    let caller_context = agent.running_execution_context();
    // 2. If callerContext is not already suspended, suspend callerContext.
    caller_context.suspend();
    // 5. Let calleeRealm be F.[[Realm]].
    let heap_data = &f.get(agent);
    let callee_realm = heap_data.realm;
    let func = heap_data.behaviour;
    // 3. Let calleeContext be a new execution context.
    let callee_context = ExecutionContext {
        // 8. Perform any necessary implementation-defined initialization of calleeContext.
        ecmascript_code: None,
        // 4. Set the Function of calleeContext to F.
        function: Some(f.unbind().into()),
        // 6. Set the Realm of calleeContext to calleeRealm.
        realm: callee_realm.unbind(),
        // 7. Set the ScriptOrModule of calleeContext to null.
        script_or_module: None,
    };
    // 9. Push calleeContext onto the execution context stack; calleeContext is now the running execution context.
    agent.push_execution_context(callee_context);
    // The texts that the function is given are held to the heap limit before
    // it reads them: it reads them where nothing can be refused.
    // 10. Let result be the Completion Record that is the result of evaluating F in a manner that conforms to
    // the specification of F. If thisArgument is uninitialized, the this value is uninitialized; otherwise,
    // thisArgument provides the this value. argumentsList provides the named parameters. newTarget provides the NewTarget value.
    let result = match func {
        Behaviour::Regular(func) => {
            if new_target.is_some() {
                Err(agent.throw_exception_with_static_message(
                    ExceptionType::TypeError,
                    "Not a constructor",
                    gc.into_nogc(),
                ))
            } else {
                func(
                    agent,
                    this_argument.unwrap_or(Value::Undefined).unbind(),
                    arguments_list.unbind(),
                    gc,
                )
            }
        }
        Behaviour::Constructor(func) => func(
            agent,
            this_argument.unwrap_or(Value::Undefined).unbind(),
            arguments_list.unbind(),
            new_target.map(|target| target.unbind().into()),
            gc,
        ),
    };
    // 11. NOTE: If F is defined in this document, “the specification of F” is the behaviour specified for it via
    // algorithm steps or other means.
    // 12. Remove calleeContext from the execution context stack and restore callerContext as the running
    // execution context.
    // Note
    // When calleeContext is removed from the execution context stack it must not be destroyed if it has been
    // suspended and retained by an accessible Generator for later resumption.
    let _callee_context = agent.pop_execution_context();
    // 13. Return ? result.
    result
}

/// ### [10.3.4 CreateBuiltinFunction ( behaviour, length, name, additionalInternalSlotsList \[ , realm \[ , prototype \[ , prefix \] \] \] )](https://tc39.es/ecma262/#sec-createbuiltinfunction)
///
/// The abstract operation CreateBuiltinFunction takes arguments behaviour (an
/// Abstract Closure, a set of algorithm steps, or some other definition of a
/// function's behaviour provided in this specification), length (a
/// non-negative integer or +∞), name (a property key or a Private Name), and
/// additionalInternalSlotsList (a List of names of internal slots) and
/// optional arguments realm (a Realm Record), prototype (an Object or null),
/// and prefix (a String) and returns a function object.
/// additionalInternalSlotsList contains the names of additional internal slots
/// that must be defined as part of the object. This operation creates a
/// built-in function object.
pub fn create_builtin_function<'a>(
    agent: &mut Agent,
    behaviour: Behaviour,
    args: BuiltinFunctionArgs,
    gc: NoGcScope<'a, '_>,
) -> BuiltinFunction<'a> {
    // 1. If realm is not present, set realm to the current Realm Record.
    let realm = args.realm.unwrap_or(agent.current_realm(gc));

    // 9. Set func.[[InitialName]] to null.
    // Note: SetFunctionName inlined here: We know name is a string
    let initial_name = if let Some(prefix) = args.prefix {
        // 12. Else,
        // a. Perform SetFunctionName(func, name, prefix).
        String::from_string(agent, format!("{} {}", args.name, prefix), gc)
    } else {
        // 11. If prefix is not present, then
        // a. Perform SetFunctionName(func, name).
        String::from_str(agent, args.name, gc)
    };

    // 2. If prototype is not present, set prototype to realm.[[Intrinsics]].[[%Function.prototype%]].

    // 3. Let internalSlotsList be a List containing the names of all the internal slots that 10.3
    //    requires for the built-in function object that is about to be created.
    // 4. Append to internalSlotsList the elements of additionalInternalSlotsList.
    // Note: The BuiltinFunctionHeapData implements all internal slots that 10.3 requires.
    // The currently appearing spec-defined additional slots are:
    // * [[ConstructorKind]] and [[SourceText]] for class constructors.
    // * [[Promise]] and [[AlreadyResolved]] for Promise resolver functions
    // * [[AlreadyCalled]], [[Index]], [[Values]], [[Capability]], and [[RemainingElements]] for
    //   Promise.all's onFulfilled function.
    // We do not yet support these, and how these end up supported is not yet fully clear.

    // 5. Let func be a new built-in function object that, when called, performs the action
    //    described by behaviour using the provided arguments as the values of the corresponding
    //    parameters specified by behaviour. The new function object has internal slots whose names
    //    are the elements of internalSlotsList, and an [[InitialName]] internal slot.
    let object_index = if let Some(prototype) = args.prototype {
        // If a prototype is set, then check that it is not the %Function.prototype%
        let realm_function_prototype = agent
            .get_realm_record_by_id(realm)
            .intrinsics()
            .get_intrinsic_default_proto(BuiltinFunction::DEFAULT_PROTOTYPE);
        if prototype == realm_function_prototype {
            // If the prototype matched the realm function prototype, then ignore it
            // as the BuiltinFunctionHeapData indirectly implies this prototype.
            None
        } else {
            // If some other prototype is defined then we need to create a backing object.
            // 6. Set func.[[Prototype]] to prototype.
            // 7. Set func.[[Extensible]] to true.
            let length_entry = ObjectEntry {
                key: PropertyKey::from(BUILTIN_STRING_MEMORY.length),
                value: ObjectEntryPropertyDescriptor::Data {
                    value: args.length.into(),
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            };
            let name_entry = ObjectEntry {
                key: PropertyKey::from(BUILTIN_STRING_MEMORY.name),
                value: ObjectEntryPropertyDescriptor::Data {
                    value: initial_name.into(),
                    writable: false,
                    enumerable: false,
                    configurable: true,
                },
            };
            Some(
                OrdinaryObject::create_object(agent, Some(prototype), &[length_entry, name_entry])
                    .expect("Should perform GC here"),
            )
        }
    } else {
        None
    };

    // 13. Return func.
    agent
        .heap
        .create(BuiltinFunctionHeapData {
            behaviour,
            direct: DirectBuiltin::of(behaviour),
            initial_name: Some(initial_name),
            // 10. Perform SetFunctionLength(func, length).
            length: args.length as u8,
            // 8. Set func.[[Realm]] to realm.
            realm,
            object_index,
        })
        .bind(gc)
}

impl<'a> CreateHeapData<BuiltinFunctionHeapData<'a>, BuiltinFunction<'a>> for Heap {
    fn create(&mut self, data: BuiltinFunctionHeapData<'a>) -> BuiltinFunction<'a> {
        self.builtin_functions.push(data.unbind());
        self.alloc_counter += core::mem::size_of::<BuiltinFunctionHeapData<'static>>();
        BuiltinFunction(BaseIndex::last(&self.builtin_functions))
    }
}

impl HeapMarkAndSweep for BuiltinFunction<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.builtin_functions.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.builtin_functions.shift_index(&mut self.0);
    }
}

impl HeapSweepWeakReference for BuiltinFunction<'static> {
    fn sweep_weak_reference(self, compactions: &CompactionLists) -> Option<Self> {
        compactions
            .builtin_functions
            .shift_weak_index(self.0)
            .map(Self)
    }
}

impl HeapMarkAndSweep for BuiltinFunctionHeapData<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            object_index,
            length: _,
            realm,
            initial_name,
            behaviour: _,
            direct: _,
        } = self;
        realm.mark_values(queues);
        initial_name.mark_values(queues);
        object_index.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            object_index,
            length: _,
            realm,
            initial_name,
            behaviour: _,
            direct: _,
        } = self;
        realm.sweep_values(compactions);
        initial_name.sweep_values(compactions);
        object_index.sweep_values(compactions);
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        ecmascript::{
            Agent, AgentOptions, DefaultHostHooks, Object, PropertyKey, String,
            create_data_property_or_throw, initialize_default_realm,
        },
        engine::{Bindable, GcScope},
    };

    /// Runs `source` in a realm whose global object has the global object
    /// of a second realm as `foreign`, and returns its value, a string.
    fn with_a_second_realm(source: &'static str) -> std::string::String {
        let (mut gc, mut scope) = GcScope::create_root();
        let mut gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        initialize_default_realm(&mut agent, gc.reborrow());
        let none: Option<for<'a> fn(&mut Agent, GcScope<'a, '_>) -> Object<'a>> = None;
        let initialize: Option<fn(&mut Agent, Object, GcScope)> = None;
        let foreign = agent
            .create_realm(none, none, initialize, gc.reborrow())
            .unbind();
        let foreign = foreign.global_object(&mut agent).unbind();
        let global = agent.current_global_object(gc.nogc()).unbind();
        let key = PropertyKey::from_static_str(&mut agent, "foreign", gc.nogc()).unbind();
        create_data_property_or_throw(&mut agent, global, key, foreign.into(), gc.reborrow())
            .unwrap();
        let source_text = String::from_static_str(&mut agent, source, gc.nogc());
        let result = agent
            .run_script(source_text.unbind(), gc.reborrow())
            .unwrap_or_else(|_| panic!("the script throws: {source}"));
        let Ok(result) = String::try_from(result) else {
            panic!("the value of the script is no string: {source}");
        };
        result.to_string_lossy(&agent).into_owned()
    }

    /// `push` of another realm, called here on an Array of that realm, is
    /// run in that realm: an Array with no properties of its own inherits
    /// from the %Array.prototype% of the realm that runs, and a setter that
    /// the other realm's has under an index takes the element. Run in
    /// place of the call it asked this realm's prototypes, found none, and
    /// made the element the Array's own. By `call` of either realm, with
    /// the setter on %Object.prototype%, and a property that cannot be
    /// written, whose TypeError is the other realm's.
    #[test]
    fn a_function_of_another_realm_is_run_in_its_realm() {
        const SCRIPT: &str = r#"
var seen = [];
function show(name, f) { var r; try { r = f(); } catch (e) { r = "threw " + (e && e.name); } seen.push(name + ": " + JSON.stringify(r)); }
function setterOn(object) { var box = { observed: 0 }; Object.defineProperty(object, "0", { configurable: true, set: function (value) { box.observed = value; } }); return box; }
function fresh() { delete foreign.Array.prototype[0]; delete foreign.Object.prototype[0]; return foreign.eval("[]"); }
show("push.call", function () { var array = fresh(), box = setterOn(foreign.Array.prototype); foreign.Array.prototype.push.call(array, 7); return [box.observed, array.length, Object.hasOwn(array, "0")]; });
show("push.call again", function () { var out = []; for (var i = 0; i < 4; i++) { var array = fresh(), box = setterOn(foreign.Array.prototype); foreign.Array.prototype.push.call(array, i + 1); out.push(box.observed, array.length, Object.hasOwn(array, "0")); } return out; });
show("call of the other realm", function () { var array = fresh(), box = setterOn(foreign.Array.prototype); foreign.Function.prototype.call.call(foreign.Array.prototype.push, array, 7); return [box.observed, array.length, Object.hasOwn(array, "0")]; });
show("call of this realm", function () { var array = fresh(), box = setterOn(foreign.Array.prototype); Function.prototype.call.call(foreign.Array.prototype.push, array, 7); return [box.observed, array.length, Object.hasOwn(array, "0")]; });
show("setter of Object.prototype", function () { var array = fresh(), box = setterOn(foreign.Object.prototype); foreign.Array.prototype.push.call(array, 7); return [box.observed, array.length, Object.hasOwn(array, "0")]; });
show("no setter", function () { var array = fresh(); array = foreign.eval("[1]"); var n = foreign.Array.prototype.push.call(array, 7); return [n, array.length, array[1], Object.hasOwn(array, "1")]; });
show("read-only", function () { var array = fresh(); Object.defineProperty(foreign.Array.prototype, "0", { configurable: true, value: "ro", writable: false }); var threw = null; try { foreign.Array.prototype.push.call(array, 7); } catch (e) { threw = e instanceof foreign.TypeError ? "their TypeError" : e instanceof TypeError ? "our TypeError" : String(e); } return [threw, array.length, Object.hasOwn(array, "0")]; });
show("this realm's push, this realm's setter", function () { var array = [], box = setterOn(Array.prototype); try { array.push(7); return [box.observed, array.length, Object.hasOwn(array, "0")]; } finally { delete Array.prototype[0]; } });
show("this realm's push", function () { var array = [1]; return [array.push(7), array.length, array[1], Object.hasOwn(array, "1")]; });
show("their other functions", function () { fresh(); var m = new foreign.Map(); foreign.Map.prototype.set.call(m, 1, 2); return [foreign.Math.abs(-3), foreign.String.prototype.charCodeAt.call("a", 0), foreign.Number.isInteger(2), foreign.String(5), foreign.Array.isArray([]), m.get(1), foreign.Array.prototype.pop.call(foreign.eval("[1, 2]"))]; });
seen.join("\n")
"#;
        let expected = [
            "push.call: [7,1,false]",
            "push.call again: [1,1,false,2,1,false,3,1,false,4,1,false]",
            "call of the other realm: [7,1,false]",
            "call of this realm: [7,1,false]",
            "setter of Object.prototype: [7,1,false]",
            "no setter: [2,2,7,true]",
            "read-only: [\"their TypeError\",0,false]",
            "this realm's push, this realm's setter: [7,1,false]",
            "this realm's push: [2,2,7,true]",
            "their other functions: [3,97,true,\"5\",true,2,2]",
        ]
        .join("\n");
        assert_eq!(with_a_second_realm(SCRIPT), expected);
    }
}
