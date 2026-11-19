// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use core::ops::ControlFlow;
use std::marker::PhantomData;

use crate::{
    ecmascript::{
        Agent, ArgumentsList, Array, ArrayIterator, ArrayIteratorPrototype,
        AsyncFromSyncIteratorPrototype, BUILTIN_STRING_MEMORY, CollectionIteratorKind,
        ExceptionType, Function, GeneratorPrototype, GeneratorStep, InternalMethods,
        IteratorRecord, JsError, JsResult, Object, OrdinaryObject, Promise, PropertyKey,
        PropertyKeySet, PropertyLookupCache, ScopedArgumentsList, StringIteratorPrototype,
        TryError, TryGetResult, TryResult, Value, call_function, create_async_from_sync_iterator,
        create_iter_result_object, get, get_iterator_from_method, get_method, get_object_method,
        throw_not_callable, to_boolean, try_get,
    },
    engine::{Bindable, GcScope, NoGcScope, Scopable, ScopeToken, Scoped},
    heap::{ArenaAccessMut, CompactionLists, HeapMarkAndSweep, WellKnownSymbols, WorkQueues},
};

/// Marker struct for working with the active iterator of the currently active,
/// scoped VM.
pub(crate) struct ActiveIterator<'a> {
    scope: PhantomData<&'a ScopeToken>,
}

fn convert_to_iter_result_object<'a>(
    agent: &mut Agent,
    result: Option<Value<'a>>,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, OrdinaryObject<'a>> {
    if let Some(result) = result {
        create_iter_result_object(agent, result, false, gc)
    } else {
        create_iter_result_object(agent, Value::Undefined, true, gc)
    }
}

impl<'a> ActiveIterator<'a> {
    pub(super) fn new(agent: &Agent, _: NoGcScope<'_, 'a>) -> Self {
        let iterator = Self { scope: PhantomData };
        // Check that the iterator is found: this panics if the VM stack is
        // empty or the active VM's iterator stack is empty.
        let _ = iterator.get(agent);
        iterator
    }

    fn reborrow(&'a self) -> ActiveIterator<'a> {
        Self { scope: PhantomData }
    }

    pub(super) fn call_next<'gc>(
        &mut self,
        agent: &mut Agent,
        value: Option<Value>,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        match self.get(agent) {
            VmIteratorRecord::InvalidIterator { .. } => {
                Err(throw_not_callable(agent, gc.into_nogc()).unbind())
            }
            VmIteratorRecord::ObjectProperties(_) => {
                // I believe it is impossible to do yield* on object properties.
                unreachable!()
            }
            VmIteratorRecord::ArrayValues(_) => ArrayValuesIterator::new(self)
                .next(agent, gc.reborrow())
                .unbind()
                .and_then(|r| convert_to_iter_result_object(agent, r.unbind(), gc.into_nogc()))
                .map(|o| o.into()),
            VmIteratorRecord::AsyncFromSyncGenericIterator(_) => {
                Ok(AsyncFromSyncGenericIterator::new(self)
                    .call_next(agent, value, gc)
                    .into())
            }
            VmIteratorRecord::GenericIterator(_) => {
                GenericIterator::new(self).call_next(agent, value, gc)
            }
            VmIteratorRecord::SliceIterator(slice_ref) => {
                let gc = gc.into_nogc();
                convert_to_iter_result_object(agent, slice_ref.unshift(agent, gc), gc)
                    .map(|o| o.into())
            }
            VmIteratorRecord::EmptySliceIterator => {
                create_iter_result_object(agent, Value::Undefined, true, gc.into_nogc())
                    .map(|o| o.into())
            }
        }
    }

    pub(super) fn throw<'gc>(
        &mut self,
        agent: &mut Agent,
        received_value: Value,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        match self.get(agent) {
            VmIteratorRecord::ArrayValues(_) => {
                // Its "throw" is looked for on the iterator, and called
                // with it if there is one.
                self.array_values_as_object(agent, gc.nogc());
                GenericIterator::new(self).throw(agent, received_value, gc)
            }
            VmIteratorRecord::AsyncFromSyncGenericIterator(_) => Ok(Some(
                AsyncFromSyncGenericIterator::new(self)
                    .throw(agent, received_value, gc)
                    .into(),
            )),
            VmIteratorRecord::GenericIterator(_) => {
                GenericIterator::new(self).throw(agent, received_value, gc)
            }
            _ => unreachable!(),
        }
    }

    pub(super) fn r#return<'gc>(
        &mut self,
        agent: &mut Agent,
        received_value: Option<Value>,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        match self.get(agent) {
            VmIteratorRecord::ArrayValues(_) => {
                if !array_iterator_record_requires_return_call(agent, gc.nogc()) {
                    return Ok(None);
                }
                // An Array Iterator has a "return" method to call, with
                // the iterator.
                let iterator = self.array_values_as_object(agent, gc.nogc()).unbind();
                GenericIterator::r#return(agent, iterator, received_value, gc)
            }
            VmIteratorRecord::AsyncFromSyncGenericIterator(_) => Ok(Some(
                AsyncFromSyncGenericIterator::new(self)
                    .r#return(agent, received_value, gc)
                    .into(),
            )),
            VmIteratorRecord::GenericIterator(IteratorRecord { iterator, .. })
            | VmIteratorRecord::InvalidIterator { iterator } => {
                GenericIterator::r#return(agent, *iterator, received_value, gc)
            }
            _ => unreachable!(),
        }
    }

    /// Puts the Array Iterator object that a [`VmIteratorRecord::ArrayValues`]
    /// record stands for in the record's place, as the iterator of a
    /// generic record whose next method is the original "next" (the record
    /// was made while that was the next method an iteration read), and
    /// returns it. An iteration that something must see as an object (its
    /// "return" or "throw" method is to be called with it) goes on from
    /// there as any other does.
    fn array_values_as_object<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> Object<'gc> {
        let VmIteratorRecord::ArrayValues(record) = self.get(agent) else {
            unreachable!()
        };
        let array = record.array.bind(gc);
        let index = record.index;
        let iterator =
            ArrayIterator::from_object(agent, array.into(), CollectionIteratorKind::Value).bind(gc);
        iterator.get_mut(agent).next_index = index.into();
        let next_method = agent
            .current_realm_record()
            .intrinsics()
            .array_iterator_prototype_next();
        *self.get_mut(agent) = VmIteratorRecord::GenericIterator(IteratorRecord {
            iterator: iterator.unbind().into(),
            next_method: next_method.into(),
        });
        iterator.into()
    }

    pub(super) fn step_value<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        match self.get(agent) {
            VmIteratorRecord::InvalidIterator { .. } => {
                Err(throw_not_callable(agent, gc.into_nogc()).unbind())
            }
            VmIteratorRecord::ObjectProperties(_) => {
                ObjectPropertiesIterator::new(self).step_value(agent, gc)
            }
            VmIteratorRecord::ArrayValues(_) => ArrayValuesIterator::new(self).next(agent, gc),
            VmIteratorRecord::AsyncFromSyncGenericIterator(_) => {
                // We should never call this for async iterators!
                unreachable!()
            }
            VmIteratorRecord::GenericIterator(_) => {
                GenericIterator::new(self).step_value(agent, gc)
            }
            VmIteratorRecord::SliceIterator(slice_ref) => {
                Ok(slice_ref.unshift(agent, gc.into_nogc()))
            }
            VmIteratorRecord::EmptySliceIterator => Ok(None),
        }
    }

    pub(super) fn remaining_length_estimate(&mut self, agent: &mut Agent) -> Option<usize> {
        self.get(agent).remaining_length_estimate(agent)
    }

    pub(super) fn get<'agent>(&self, agent: &'agent Agent) -> &'agent VmIteratorRecord<'a> {
        agent
            .vm_stack
            .last()
            .expect("No VM found")
            .get_active_iterator()
    }

    fn get_mut<'agent>(
        &mut self,
        agent: &'agent mut Agent,
    ) -> &'agent mut VmIteratorRecord<'static> {
        agent
            .vm_stack
            .last_mut()
            .expect("No VM found")
            .get_active_iterator_mut()
    }
}

#[derive(Debug, Bindable)]
pub(crate) enum VmIteratorRecord<'a> {
    /// Special type for iterators that do not have a callable next method.
    InvalidIterator {
        iterator: Object<'a>,
    },
    ObjectProperties(Box<ObjectPropertiesIteratorRecord<'static>>),
    ArrayValues(ArrayValuesIteratorRecord<'a>),
    AsyncFromSyncGenericIterator(IteratorRecord<'a>),
    GenericIterator(IteratorRecord<'a>),
    SliceIterator(ScopedArgumentsList<'static>),
    EmptySliceIterator,
}

/// IteratorStepValue from what the next method returned: the value of an
/// iterator result that is not done, None for one that is, and a TypeError
/// for what is not an object.
pub(super) fn step_value_of_result<'gc>(
    agent: &mut Agent,
    result: Value,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Option<Value<'gc>>> {
    let result = result.bind(gc.nogc());
    let Ok(result) = Object::try_from(result) else {
        return Err(throw_iterator_returned_non_object(agent, gc.into_nogc()));
    };
    let result = result.unbind().bind(gc.nogc());
    // The result object nearly every iterator returns is an ordinary one
    // with "done" and "value" as plain properties of its own (the one
    // CreateIteratorResultObject makes, or a literal): \[\[Get]] of
    // those is the value in the slot.
    if let Some(done) = own_data_property(agent, result, BUILTIN_STRING_MEMORY.done.into()) {
        if to_boolean(agent, done) {
            // The value of a result that is done is not read.
            return Ok(None);
        }
        if let Some(value) = own_data_property(agent, result, BUILTIN_STRING_MEMORY.value.into()) {
            return Ok(Some(value.unbind().bind(gc.into_nogc())));
        }
    }
    let scoped_result = result.scope(agent, gc.nogc());
    // 1. Return ToBoolean(? Get(iterResult, "done")).
    let done = get(
        agent,
        result.unbind(),
        BUILTIN_STRING_MEMORY.done.into(),
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    let done = to_boolean(agent, done);
    if done {
        Ok(None)
    } else {
        // 1. Return ? Get(iterResult, "value").
        let value = get(
            agent,
            scoped_result.get(agent),
            BUILTIN_STRING_MEMORY.value.into(),
            gc,
        )?;
        Ok(Some(value))
    }
}

impl<'a> VmIteratorRecord<'a> {
    pub(super) fn remaining_length_estimate(&self, agent: &Agent) -> Option<usize> {
        match self {
            VmIteratorRecord::InvalidIterator { .. } => Some(0),
            VmIteratorRecord::ObjectProperties(iter) => {
                Some(iter.remaining_keys.as_ref().map_or(0, |k| k.len()))
            }
            VmIteratorRecord::ArrayValues(iter) => {
                Some(iter.array.len(agent).saturating_sub(iter.index) as usize)
            }
            VmIteratorRecord::AsyncFromSyncGenericIterator(_) => todo!(),
            VmIteratorRecord::GenericIterator(_) => None,
            VmIteratorRecord::SliceIterator(slice) => Some(slice.len(agent)),
            VmIteratorRecord::EmptySliceIterator => Some(0),
        }
    }

    pub(super) fn requires_return_call(&self, agent: &mut Agent, gc: NoGcScope) -> bool {
        match self {
            VmIteratorRecord::ObjectProperties(_)
            | VmIteratorRecord::SliceIterator(_)
            | VmIteratorRecord::EmptySliceIterator => false,
            VmIteratorRecord::ArrayValues(_) => {
                array_iterator_record_requires_return_call(agent, gc)
            }
            VmIteratorRecord::InvalidIterator { iterator }
            | VmIteratorRecord::GenericIterator(IteratorRecord { iterator, .. })
            | VmIteratorRecord::AsyncFromSyncGenericIterator(IteratorRecord { iterator, .. }) => {
                generic_iterator_record_requires_return_call(agent, *iterator, gc)
            }
        }
    }

    /// GetIterator(obj, sync) where it runs nothing and makes no iterator:
    /// `obj` is an Array that is iterated the way the realm set out with,
    /// which is walked element by element. Nearly every Array is, and the
    /// method is found by the shape of the array
    /// ([`Array::is_iterated_by_values`]). None leaves it to
    /// [`VmIteratorRecord::from_value`].
    #[inline]
    pub(super) fn of_array_walked<'gc>(
        agent: &mut Agent,
        obj: Value<'gc>,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<VmIteratorRecord<'gc>> {
        let Value::Array(array) = obj else {
            return None;
        };
        array
            .is_iterated_by_values(agent, gc)
            .then(|| VmIteratorRecord::ArrayValues(ArrayValuesIteratorRecord::new(array)))
    }

    /// ### [7.4.4 GetIterator ( obj, kind )](https://tc39.es/ecma262/#sec-getiterator)
    ///
    /// The abstract operation GetIterator takes arguments obj (an ECMAScript
    /// language value) and returns either a normal completion containing an
    /// Iterator Record or a throw completion.
    ///
    /// This method version performs the SYNC version of the method.
    pub(super) fn from_value(
        agent: &mut Agent,
        obj: Value,
        mut gc: GcScope<'a, '_>,
    ) -> JsResult<'a, Self> {
        let value = obj.bind(gc.nogc());
        if let Some(walk) = VmIteratorRecord::of_array_walked(agent, value, gc.nogc()) {
            return Ok(walk.unbind());
        }
        let scoped_value = value.scope(agent, gc.nogc());
        // a. Let method be ? GetMethod(obj, %Symbol.iterator%).
        let method = get_method(
            agent,
            value.unbind(),
            PropertyKey::Symbol(WellKnownSymbols::Iterator.into()),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        // 3. If method is undefined, throw a TypeError exception.
        let Some(method) = method else {
            return Err(throw_iterator_method_undefined(agent, gc.into_nogc()));
        };

        let value = scoped_value.take(agent).bind(gc.nogc());
        // 4. Return ? GetIteratorFromMethod(obj, method).
        if let Some(array) = Array::is_iterable_array(agent, value, method) {
            // The same walk, where a getter gave the method.
            Ok(VmIteratorRecord::ArrayValues(
                ArrayValuesIteratorRecord::new(array.unbind()),
            ))
        } else {
            Ok(
                get_iterator_from_method(agent, value.unbind(), method.unbind(), gc)?
                    .into_vm_iterator_record(),
            )
        }
    }

    /// ### [7.4.4 GetIterator ( obj, kind )](https://tc39.es/ecma262/#sec-getiterator)
    ///
    /// The abstract operation GetIterator takes arguments obj (an ECMAScript
    /// language value) and returns either a normal completion containing an
    /// Iterator Record or a throw completion.
    ///
    /// This method version performs the ASYNC version of the method.
    pub(super) fn async_from_value(
        agent: &mut Agent,
        obj: Value,
        mut gc: GcScope<'a, '_>,
    ) -> JsResult<'a, Self> {
        let obj = obj.bind(gc.nogc());
        let scoped_obj = obj.scope(agent, gc.nogc());
        // a. Let method be ? GetMethod(obj, %Symbol.asyncIterator%).
        let method = get_method(
            agent,
            obj.unbind(),
            PropertyKey::Symbol(WellKnownSymbols::AsyncIterator.into()),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        // b. If method is undefined, then
        let Some(method) = method else {
            return Self::async_from_sync_value(agent, scoped_obj, gc);
        };

        let obj = scoped_obj.take(agent).bind(gc.nogc());

        // 4. Return ? GetIteratorFromMethod(obj, method).
        if let Some(array) = Array::is_iterable_array(agent, obj, method) {
            // Optimisation: if we're using the Array values iterator on
            // an Array then we can use a special iterator case.
            Ok(VmIteratorRecord::ArrayValues(
                ArrayValuesIteratorRecord::new(array.unbind()),
            ))
        } else {
            Ok(
                get_iterator_from_method(agent, obj.unbind(), method.unbind(), gc)?
                    .into_vm_iterator_record(),
            )
        }
    }

    fn async_from_sync_value(
        agent: &mut Agent,
        obj: Scoped<Value>,
        mut gc: GcScope<'a, '_>,
    ) -> JsResult<'a, Self> {
        // i. Let syncMethod be ? GetMethod(obj, %Symbol.iterator%).
        let sync_method = get_method(
            agent,
            obj.get(agent),
            WellKnownSymbols::Iterator.into(),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        // ii. If syncMethod is undefined, throw a TypeError exception.
        let Some(sync_method) = sync_method else {
            return Err(throw_iterator_method_undefined(agent, gc.into_nogc()));
        };
        let obj = obj.take(agent).bind(gc.nogc());
        // iii. Let syncIteratorRecord be ? GetIteratorFromMethod(obj, syncMethod).
        let sync_iterator_record =
            get_iterator_from_method(agent, obj.unbind(), sync_method.unbind(), gc)?;
        // iv. Return CreateAsyncFromSyncIterator(syncIteratorRecord).
        Ok(create_async_from_sync_iterator(sync_iterator_record))
    }

    pub(super) fn try_step_value<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, Option<Value<'gc>>> {
        match self {
            VmIteratorRecord::InvalidIterator { .. } => throw_not_callable(agent, gc).into(),
            VmIteratorRecord::ObjectProperties(iter) => iter.try_step_value(agent, gc),
            VmIteratorRecord::ArrayValues(iter) => iter.try_next(agent, gc),
            VmIteratorRecord::AsyncFromSyncGenericIterator(_) => {
                // We should never call this for async iterators!
                unreachable!()
            }
            VmIteratorRecord::GenericIterator(_) => TryError::GcError.into(),
            VmIteratorRecord::SliceIterator(slice_ref) => {
                TryResult::Continue(slice_ref.unshift(agent, gc))
            }
            VmIteratorRecord::EmptySliceIterator => TryResult::Continue(None),
        }
    }
}

struct ObjectPropertiesIterator<'a> {
    iter: ActiveIterator<'a>,
}

impl<'a> ObjectPropertiesIterator<'a> {
    fn new(iter: &'a ActiveIterator) -> Self {
        Self {
            iter: iter.reborrow(),
        }
    }

    /// ### [7.4.8 IteratorStepValue ( iteratorRecord )](https://tc39.es/ecma262/#sec-iteratorstepvalue)
    ///
    /// While not exactly equal to the IteratorStepValue method in usage, this
    /// function implements much the same intent. It does the IteratorNext
    /// step, followed by a completion check, and finally extracts the value
    /// if the iterator did not complete yet.
    fn step_value<'gc>(
        &mut self,
        agent: &mut Agent,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        // This step asks whether a key was met before it looks for the
        // property of the key, for every key.
        self.note_pending_keys(agent);
        loop {
            // A Proxy can make the prototype chain a cycle.
            agent.poll_interrupt(gc.nogc()).unbind()?;
            if !self.object_is_visited(agent) {
                let keys = self
                    .object(agent, gc.nogc())
                    .unbind()
                    .internal_own_property_keys(agent, gc.reborrow())
                    .unbind()?
                    .bind(gc.nogc());
                let mut remaining_keys = Vec::with_capacity(keys.len());
                for key in keys {
                    if let PropertyKey::Symbol(_) = key {
                        continue;
                    } else {
                        remaining_keys.push(key);
                    }
                }
                self.set_remaining_keys(agent, remaining_keys);
            }
            while let Some((object, next_key)) = self.next_remaining_key(agent, gc.nogc()) {
                let scoped_next_key = next_key.scope(agent, gc.nogc());
                let desc = object
                    .unbind()
                    .internal_get_own_property(agent, next_key.unbind(), gc.reborrow())
                    .unbind()?
                    .bind(gc.nogc());
                let next_key = scoped_next_key.take(agent).bind(gc.nogc());
                if let Some(desc) = desc {
                    self.mark_key_visited(agent, next_key);
                    if desc.enumerable == Some(true) {
                        return Ok(Some(Self::convert_result(
                            agent,
                            next_key.unbind(),
                            gc.into_nogc(),
                        )));
                    }
                }
            }
            let prototype = self
                .object(agent, gc.nogc())
                .unbind()
                .internal_get_prototype_of(agent, gc.reborrow())
                .unbind()?
                .bind(gc.nogc());
            if let Some(prototype) = prototype {
                self.set_object(agent, prototype);
            } else {
                return Ok(None);
            }
        }
    }

    fn convert_result<'gc>(
        agent: &mut Agent,
        result: PropertyKey,
        gc: NoGcScope<'gc, '_>,
    ) -> Value<'gc> {
        match result {
            PropertyKey::Integer(int) => crate::ecmascript::i64_text(int.into_i64())
                .to_js_string(agent, gc)
                .into(),
            PropertyKey::SmallString(data) => Value::SmallString(data),
            PropertyKey::String(data) => Value::String(data.bind(gc)),
            _ => unreachable!(),
        }
    }

    fn object<'gc>(&self, agent: &Agent, gc: NoGcScope<'gc, '_>) -> Object<'gc> {
        self.get(agent).object.bind(gc)
    }

    fn object_is_visited(&self, agent: &Agent) -> bool {
        self.get(agent).remaining_keys.is_some()
    }

    fn set_remaining_keys(&mut self, agent: &mut Agent, mut remaining_keys: Vec<PropertyKey>) {
        remaining_keys.reverse();
        self.get_mut(agent).remaining_keys = Some(remaining_keys.unbind());
    }

    fn set_object(&mut self, agent: &mut Agent, object: Object) {
        let iter = self.get_mut(agent);
        iter.object = object.unbind();
        iter.inherited = true;
        iter.remaining_keys = None;
    }

    /// Moves the keys that wait for it into the set of the visited ones
    /// ([`ObjectPropertiesIteratorRecord::pending_keys`]).
    fn note_pending_keys(&mut self, agent: &mut Agent) {
        let record = self.get_mut(agent);
        if record.pending_keys.is_empty() {
            return;
        }
        let pending_keys = core::mem::take(&mut record.pending_keys);
        let mut visited_keys = core::mem::take(&mut record.visited_keys);
        for key in pending_keys {
            visited_keys.insert(agent, key);
        }
        self.get_mut(agent).visited_keys = visited_keys;
    }

    fn next_remaining_key<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<(Object<'gc>, PropertyKey<'gc>)> {
        loop {
            let next_key = self.get_mut(agent).remaining_keys.as_mut().unwrap().pop()?;
            let iter_ref = self.get(agent);
            if iter_ref.visited_keys.contains(agent, next_key) {
                // Skip visited keys.
                continue;
            }
            return Some((iter_ref.object.bind(gc), next_key.bind(gc)));
        }
    }

    fn mark_key_visited(&mut self, agent: &mut Agent, key: PropertyKey) {
        let mut visited_keys = core::mem::take(&mut self.get_mut(agent).visited_keys);
        visited_keys.insert(agent, key);
        self.get_mut(agent).visited_keys = visited_keys;
    }

    fn get<'agent>(&self, agent: &'agent Agent) -> &'agent ObjectPropertiesIteratorRecord<'a> {
        let VmIteratorRecord::ObjectProperties(iter) = self.iter.get(agent) else {
            unreachable!()
        };
        iter
    }

    fn get_mut<'agent>(
        &mut self,
        agent: &'agent mut Agent,
    ) -> &'agent mut ObjectPropertiesIteratorRecord<'static> {
        let VmIteratorRecord::ObjectProperties(iter) = self.iter.get_mut(agent) else {
            unreachable!()
        };
        iter
    }
}

#[derive(Debug)]
pub(crate) struct ObjectPropertiesIteratorRecord<'a> {
    object: Object<'a>,
    /// The keys of the properties met so far, on `object` and on the
    /// objects before it: a key among them is not given again.
    visited_keys: PropertyKeySet<'a>,
    /// Keys that belong in `visited_keys` and are not there yet: those met
    /// by steps that run nothing, which ask about a key only where an
    /// object after the first has an enumerable property of that key. Most
    /// loops end without that, and with no key ever hashed.
    pending_keys: Vec<PropertyKey<'a>>,
    /// `object` is not the object the loop is over, but one of its
    /// prototypes.
    inherited: bool,
    remaining_keys: Option<Vec<PropertyKey<'a>>>,
}

impl<'a> ObjectPropertiesIteratorRecord<'a> {
    pub(super) fn new(object: Object<'a>) -> Self {
        Self {
            object,
            visited_keys: Default::default(),
            pending_keys: Vec::new(),
            inherited: false,
            remaining_keys: Default::default(),
        }
    }

    /// Moves the pending keys into the set of the visited ones.
    fn note_pending_keys(&mut self, agent: &Agent) {
        for key in self.pending_keys.drain(..) {
            self.visited_keys.insert(agent, key);
        }
    }

    /// True if a property of `key` was met before.
    fn was_visited(&mut self, agent: &Agent, key: PropertyKey) -> bool {
        self.note_pending_keys(agent);
        self.visited_keys.contains(agent, key)
    }

    fn object_is_visited(&self) -> bool {
        self.remaining_keys.is_some()
    }

    fn set_remaining_keys(&mut self, mut remaining_keys: Vec<PropertyKey>) {
        remaining_keys.reverse();
        self.remaining_keys = Some(remaining_keys.unbind());
    }

    fn set_object(&mut self, object: Object) {
        let iter = self;
        iter.object = object.unbind();
        iter.inherited = true;
        iter.remaining_keys = None;
    }

    /// ### [7.4.8 IteratorStepValue ( iteratorRecord )](https://tc39.es/ecma262/#sec-iteratorstepvalue)
    ///
    /// While not exactly equal to the IteratorStepValue method in usage, this
    /// function implements much the same intent. It does the IteratorNext
    /// step, followed by a completion check, and finally extracts the value
    /// if the iterator did not complete yet.
    ///
    /// The step, where nothing has to run for it. Where something has to (a
    /// trap of a Proxy), nothing is lost: the key that the step stopped at
    /// is still the next one for the step that runs it.
    ///
    /// Nothing runs for the property of a key here, so whether a property
    /// of the key was met before may be asked after the property is found,
    /// and only where the answer changes anything: where the property is
    /// enumerable, on an object that is not the first. The keys of the
    /// first object are all different, and none was met before them.
    pub(super) fn try_step_value<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, Option<Value<'gc>>> {
        loop {
            if let Err(err) = agent.poll_interrupt(gc) {
                return TryResult::Break(TryError::Err(err));
            }
            let object = self.object.bind(gc);
            if !self.object_is_visited() {
                let keys = object.try_own_property_keys(agent, gc)?;
                let mut remaining_keys = Vec::with_capacity(keys.len());
                for key in keys {
                    if let PropertyKey::Symbol(_) = key {
                        continue;
                    } else {
                        remaining_keys.push(key);
                    }
                }
                self.set_remaining_keys(remaining_keys);
            }
            while let Some(next_key) = self.remaining_keys.as_ref().unwrap().last().copied() {
                let next_key = next_key.bind(gc);
                let desc = object.try_get_own_property(agent, next_key, None, gc)?;
                self.remaining_keys.as_mut().unwrap().pop();
                let Some(desc) = desc else {
                    continue;
                };
                if desc.enumerable == Some(true) {
                    if self.inherited && self.was_visited(agent, next_key) {
                        continue;
                    }
                    self.pending_keys.push(next_key.unbind());
                    return TryResult::Continue(Some(ObjectPropertiesIterator::convert_result(
                        agent, next_key, gc,
                    )));
                }
                self.pending_keys.push(next_key.unbind());
            }
            let prototype = object.try_get_prototype_of(agent, gc)?;
            if let Some(prototype) = prototype {
                self.set_object(prototype);
            } else {
                return TryResult::Continue(None);
            }
        }
    }
}

struct ArrayValuesIterator<'a> {
    iter: ActiveIterator<'a>,
}

impl<'a> ArrayValuesIterator<'a> {
    fn new(iter: &'a ActiveIterator) -> Self {
        Self {
            iter: iter.reborrow(),
        }
    }

    pub(super) fn next<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        // b. Repeat,
        let Some((array, index)) = self.prep_step(agent, gc.nogc()) else {
            // The iterator is exhausted.
            return Ok(None);
        };
        if let Some(element_value) = array.as_slice(agent)[index as usize] {
            // Fast path: If the element at this index has a Value, then it is
            // not an accessor nor a hole. Yield the result as-is.
            return Ok(Some(element_value.unbind()));
        }
        // 1. Let elementKey be ! ToString(indexNumber).
        // 2. Let elementValue be ? Get(array, elementKey).
        let element_value = get(agent, array.unbind(), index.into(), gc)?;
        // a. Let result be elementValue.
        // vii. Perform ? GeneratorYield(CreateIterResultObject(result, false)).
        Ok(Some(element_value))
    }

    fn prep_step<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<(Array<'gc>, u32)> {
        let VmIteratorRecord::ArrayValues(iter) = self.iter.get_mut(agent) else {
            unreachable!()
        };
        let array = iter.array.bind(gc);
        // iv. Let indexNumber be 𝔽(index).
        let index = iter.index;
        // 1. Let len be ? LengthOfArrayLike(array).
        let len = array.len(agent);
        // iii. If index ≥ len, return NormalCompletion(undefined).
        if index >= len {
            return None;
        }
        // viii. Set index to index + 1.
        let VmIteratorRecord::ArrayValues(iter) = self.iter.get_mut(agent) else {
            unreachable!()
        };
        iter.index += 1;
        Some((array, index))
    }
}

#[derive(Debug, Bindable)]
pub(crate) struct ArrayValuesIteratorRecord<'a> {
    array: Array<'a>,
    index: u32,
}

impl<'a> ArrayValuesIteratorRecord<'a> {
    pub(super) fn new(array: Array<'a>) -> Self {
        Self {
            array,
            // a. Let index be 0.
            index: 0,
        }
    }

    /// The step, where it is no more than reading a value: the walk is
    /// over (None for the step), or the next element has a value, which is
    /// then neither a hole nor an accessor. Where it is one of those this
    /// returns None with nothing changed, and the step is left to
    /// [`ArrayValuesIteratorRecord::try_next`] or \[\[Get]].
    #[inline]
    pub(super) fn plain_next<'gc>(
        &mut self,
        agent: &Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<Option<Value<'gc>>> {
        let array = self.array.bind(gc);
        // iv. Let indexNumber be 𝔽(index).
        let index = self.index;
        // 1. Let len be ? LengthOfArrayLike(array).
        // iii. If index ≥ len, return NormalCompletion(undefined).
        if index >= array.len(agent) {
            return Some(None);
        }
        let element_value = (*array.as_slice(agent).get(index as usize)?)?;
        // viii. Set index to index + 1.
        self.index += 1;
        Some(Some(element_value))
    }

    /// The step, where nothing has to run for it. Where something has to (a
    /// getter, a Proxy's trap), nothing has changed: the element is still
    /// the next one for the step that runs it.
    fn try_next<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, Option<Value<'gc>>> {
        // b. Repeat,
        if let Some(step) = self.plain_next(agent, gc) {
            return TryResult::Continue(step);
        }
        let array = self.array.bind(gc);
        let index = self.index;
        // 1. Let elementKey be ! ToString(indexNumber).
        // 2. Let elementValue be ? Get(array, elementKey).
        let element_value = match try_get(agent, array, index.into(), None, gc)? {
            TryGetResult::Unset => Value::Undefined,
            TryGetResult::Value(value) => value,
            TryGetResult::Get(_) | TryGetResult::Proxy(_) => {
                return TryError::GcError.into();
            }
        };
        // viii. Set index to index + 1.
        self.index += 1;
        // a. Let result be elementValue.
        // vii. Perform ? GeneratorYield(CreateIterResultObject(result, false)).
        TryResult::Continue(Some(element_value))
    }
}

/// The value of `object`'s own plain data property `key`, where reading it
/// is no more than that: an ordinary object with few properties that has
/// `key` among its own, with a value there and not an accessor. None leaves
/// the read to \[\[Get]].
fn own_data_property<'a>(agent: &Agent, object: Object<'a>, key: PropertyKey) -> Option<Value<'a>> {
    let Object::Object(object) = object else {
        return None;
    };
    let keys = object.get_property_keys(agent);
    if keys.len() > 8 {
        return None;
    }
    let index = keys.iter().position(|k| *k == key)?;
    object
        .get_elements_values(agent)
        .get(index)
        .copied()
        .flatten()
}

struct GenericIterator<'a> {
    iter: ActiveIterator<'a>,
}

impl<'a> GenericIterator<'a> {
    fn new(iter: &'a ActiveIterator) -> Self {
        Self {
            iter: iter.reborrow(),
        }
    }

    fn call_next<'gc>(
        &mut self,
        agent: &mut Agent,
        value: Option<Value>,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        let iter = self.get(agent);
        let next_method = iter.next_method.bind(gc.nogc());
        let iterator = iter.iterator.bind(gc.nogc());
        let value = value.bind(gc.nogc());

        // 1. If value is not present, then
        if let Some(value) = value {
            // 2. Else,
            // a. Let result be Completion(Call(iteratorRecord.[[NextMethod]], iteratorRecord.[[Iterator]], « value »)).
            call_function(
                agent,
                next_method.unbind(),
                iterator.unbind().into(),
                Some(ArgumentsList::from_mut_value(&mut value.unbind())),
                gc,
            )
        } else {
            // a. Let result be Completion(Call(iteratorRecord.[[NextMethod]], iteratorRecord.[[Iterator]])).
            call_function(
                agent,
                next_method.unbind(),
                iterator.unbind().into(),
                None,
                gc,
            )
        }
    }

    fn step_value<'gc>(
        &mut self,
        agent: &mut Agent,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        let iter = self.get(agent);
        let next_method = iter.next_method.bind(gc.nogc());
        let iterator = iter.iterator.bind(gc.nogc());

        // The original %ArrayIteratorPrototype%.next on an Array Iterator
        // only wraps its step in a fresh iterator result object, whose own
        // "done" and "value" data properties IteratorStepValue then reads.
        if let Object::ArrayIterator(array_iterator) = iterator
            && next_method
                == agent
                    .current_realm_record()
                    .intrinsics()
                    .array_iterator_prototype_next()
                    .into()
        {
            return ArrayIteratorPrototype::step(agent, array_iterator.unbind(), gc);
        }
        // The same for a String Iterator and its original next.
        if let Object::StringIterator(string_iterator) = iterator
            && let Ok(next_method) = Function::try_from(next_method)
            && StringIteratorPrototype::is_next(agent, next_method)
        {
            return Ok(StringIteratorPrototype::step(
                agent,
                string_iterator.unbind(),
                gc.into_nogc(),
            ));
        }

        // The original %GeneratorPrototype%.next on a generator resumes it
        // and puts what it yielded into an iterator result object, which
        // this step would take apart again: the generator is resumed here
        // and what it yielded taken as it is. Only what `yield*` passes on
        // is a result object already, and is read as any other.
        let result = if let Object::Generator(generator) = iterator
            && GeneratorPrototype::is_next(agent, next_method, gc.nogc())
        {
            match generator
                .unbind()
                .step(agent, gc.reborrow())
                .unbind()?
                .bind(gc.nogc())
            {
                GeneratorStep::Yielded(value) => {
                    return Ok(Some(value.unbind().bind(gc.into_nogc())));
                }
                GeneratorStep::Done => return Ok(None),
                GeneratorStep::Result(result) => result,
            }
        } else {
            call_function(
                agent,
                next_method.unbind(),
                iterator.unbind().into(),
                None,
                gc.reborrow(),
            )
            .unbind()?
            .bind(gc.nogc())
        };
        step_value_of_result(agent, result.unbind(), gc)
    }

    fn throw<'gc>(
        &mut self,
        agent: &mut Agent,
        received_value: Value,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        let received_value = received_value.scope(agent, gc.nogc());
        let iterator = self.get(agent).iterator.bind(gc.nogc());
        // 1. Let throw be ? GetMethod(iterator, "throw").
        let throw = get_object_method(
            agent,
            iterator.unbind(),
            BUILTIN_STRING_MEMORY.throw.into(),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        // 2. If throw is not undefined, then
        if let Some(throw) = throw {
            let iterator = self.get(agent).iterator.bind(gc.nogc());
            // i. Return Call(throw, iterator, « received.[[Value]] »).
            Ok(Some(call_function(
                agent,
                throw.unbind(),
                iterator.unbind().into(),
                Some(ArgumentsList::from_mut_value(
                    &mut received_value.take(agent),
                )),
                gc,
            )?))
        } else {
            Ok(None)
        }
    }

    fn r#return<'gc>(
        agent: &mut Agent,
        iterator: Object,
        value: Option<Value>,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        let value = value.map(|v| v.scope(agent, gc.nogc()));
        let iterator = iterator.bind(gc.nogc());
        let scoped_iterator = iterator.scope(agent, gc.nogc());
        // 1. Assert: iteratorRecord.[[Iterator]] is an Object.
        // 2. Let iterator be iteratorRecord.[[Iterator]].
        // 3. Let innerResult be Completion(GetMethod(iterator, "return")).
        let r#return = get_object_method(
            agent,
            iterator.unbind(),
            BUILTIN_STRING_MEMORY.r#return.into(),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        // 4. If completion is a throw completion, return ? completion.
        // 5. If innerResult is a throw completion, return ? innerResult.
        // 4. If innerResult is a normal completion, then
        // a. Let return be innerResult.[[Value]].
        // b. If return is undefined, ...
        if let Some(r#return) = r#return {
            let iterator = scoped_iterator.take(agent).bind(gc.nogc());
            // c. Set innerResult to Completion(Call(return, iterator)).
            // d. If innerResult is a normal completion, set innerResult to
            //    Completion(innerResult.[[Value]]).
            let mut value = value.map(|v| v.take(agent));
            Ok(Some(call_function(
                agent,
                r#return.unbind(),
                iterator.unbind().into(),
                value.as_mut().map(ArgumentsList::from_mut_value),
                gc,
            )?))
        } else {
            // ... return ? completion.
            Ok(None)
        }
    }

    fn get<'agent>(&self, agent: &'agent Agent) -> &'agent IteratorRecord<'a> {
        match self.iter.get(agent) {
            VmIteratorRecord::GenericIterator(iter) => iter,
            _ => unreachable!(),
        }
    }
}

struct AsyncFromSyncGenericIterator<'a> {
    iter: ActiveIterator<'a>,
}

impl<'a> AsyncFromSyncGenericIterator<'a> {
    fn new(iter: &'a ActiveIterator) -> Self {
        Self {
            iter: iter.reborrow(),
        }
    }

    /// ### [27.1.6.2.1 %AsyncFromSyncIteratorPrototype%.next ( \[ value \] )](https://tc39.es/ecma262/#sec-%asyncfromsynciteratorprototype%.next)
    fn call_next<'gc>(
        &mut self,
        agent: &mut Agent,
        value: Option<Value>,
        gc: GcScope<'gc, '_>,
    ) -> Promise<'gc> {
        // 1. Let O be the this value.
        // 2. Assert: O is an Object that has a [[SyncIteratorRecord]] internal slot.
        // 4. Let syncIteratorRecord be O.[[SyncIteratorRecord]].
        // 5. If value is present, then
        let iter = self.get(agent);
        let sync_iterator_record = iter.bind(gc.nogc());
        AsyncFromSyncIteratorPrototype::next(agent, sync_iterator_record.unbind(), value, gc)
    }

    /// ### [27.1.6.2.2 %AsyncFromSyncIteratorPrototype%.return ( \[ value \] )](https://tc39.es/ecma262/#sec-%asyncfromsynciteratorprototype%.return)
    fn r#return<'gc>(
        &mut self,
        agent: &mut Agent,
        value: Option<Value>,
        gc: GcScope<'gc, '_>,
    ) -> Promise<'gc> {
        // 1. Let O be the this value.
        // 2. Assert: O is an Object that has a [[SyncIteratorRecord]] internal slot.
        // 4. Let syncIteratorRecord be O.[[SyncIteratorRecord]].
        // 5. Let syncIterator be syncIteratorRecord.[[Iterator]].
        let iter = self.get(agent);
        let sync_iterator = iter.iterator.bind(gc.nogc());
        AsyncFromSyncIteratorPrototype::r#return(agent, sync_iterator.unbind(), value, gc)
    }

    /// ### [27.1.6.2.3 %AsyncFromSyncIteratorPrototype%.throw ( \[ value \] )](https://tc39.es/ecma262/#sec-%asyncfromsynciteratorprototype%.throw)
    ///
    /// > NOTE: In this specification, value is always provided, but is left
    /// > optional for consistency with
    /// > %AsyncFromSyncIteratorPrototype%.return ( [ value ] ).
    fn throw<'gc>(
        &mut self,
        agent: &mut Agent,
        value: Value,
        gc: GcScope<'gc, '_>,
    ) -> Promise<'gc> {
        // 1. Let O be the this value.
        // 2. Assert: O is an Object that has a [[SyncIteratorRecord]] internal slot.
        // 4. Let syncIteratorRecord be O.[[SyncIteratorRecord]].
        // 5. Let syncIterator be syncIteratorRecord.[[Iterator]].
        let iter = self.get(agent);
        let sync_iterator = iter.iterator.bind(gc.nogc());
        AsyncFromSyncIteratorPrototype::throw(agent, sync_iterator.unbind(), value, gc)
    }

    fn get<'agent>(&self, agent: &'agent Agent) -> &'agent IteratorRecord<'a> {
        match self.iter.get(agent) {
            VmIteratorRecord::AsyncFromSyncGenericIterator(iter) => iter,
            _ => unreachable!(),
        }
    }
}

fn array_iterator_record_requires_return_call(agent: &mut Agent, gc: NoGcScope) -> bool {
    // Note: no one can access the array values iterator while it is
    // iterating so we know its prototype is the intrinsic
    // ArrayIteratorPrototype. But that may have a return method
    // set on it by a horrible-horrible person somewhere.
    // TODO: This should actually maybe be the proto of the Array's
    // realm?
    let array_iterator_prototype = agent
        .current_realm_record()
        .intrinsics()
        .array_iterator_prototype();
    // IteratorClose calls GetMethod on the iterator: if a
    // non-nullable value is found this way then things happen. Every walk
    // that ends early asks (a pattern with fewer elements than the array,
    // a `break`): the property cache of the name remembers that the
    // prototypes have none.
    let key = PropertyKey::from(BUILTIN_STRING_MEMORY.r#return);
    let cache = PropertyLookupCache::new(agent, key);
    match array_iterator_prototype.try_get(
        agent,
        key,
        array_iterator_prototype.into(),
        Some(cache),
        gc,
    ) {
        ControlFlow::Continue(TryGetResult::Unset) => false,
        ControlFlow::Continue(TryGetResult::Value(v)) => !v.is_undefined() && !v.is_null(),
        // A getter, a Proxy's trap or an error: \[\[Get]] of the method,
        // with the iterator, is what tells.
        _ => true,
    }
}

fn generic_iterator_record_requires_return_call(
    agent: &mut Agent,
    iterator: Object,
    gc: NoGcScope,
) -> bool {
    match iterator.try_get(
        agent,
        BUILTIN_STRING_MEMORY.r#return.into(),
        iterator.into(),
        None,
        gc,
    ) {
        ControlFlow::Continue(TryGetResult::Unset) => false,
        ControlFlow::Continue(TryGetResult::Value(v)) => !v.is_undefined() && !v.is_null(),
        // TODO: consider passing the error on here; not sure if this is
        // possible though.
        ControlFlow::Break(TryError::Err(_)) => unreachable!(),
        _ => true,
    }
}

impl HeapMarkAndSweep for ObjectPropertiesIteratorRecord<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            object,
            visited_keys,
            pending_keys,
            inherited: _,
            remaining_keys,
        } = self;
        object.mark_values(queues);
        visited_keys.mark_values(queues);
        for key in pending_keys.iter() {
            key.mark_values(queues);
        }
        if let Some(remaining_keys) = remaining_keys {
            for key in remaining_keys.iter() {
                key.mark_values(queues);
            }
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            object,
            visited_keys,
            pending_keys,
            inherited: _,
            remaining_keys,
        } = self;
        object.sweep_values(compactions);
        visited_keys.sweep_values(compactions);
        for key in pending_keys.iter_mut() {
            key.sweep_values(compactions);
        }
        if let Some(remaining_keys) = remaining_keys {
            for key in remaining_keys.iter_mut() {
                key.sweep_values(compactions);
            }
        }
    }
}

impl HeapMarkAndSweep for ArrayValuesIteratorRecord<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        self.array.mark_values(queues)
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        self.array.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for VmIteratorRecord<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        match self {
            VmIteratorRecord::InvalidIterator { iterator } => iterator.mark_values(queues),
            VmIteratorRecord::ObjectProperties(iter) => iter.mark_values(queues),
            VmIteratorRecord::ArrayValues(iter) => iter.mark_values(queues),
            VmIteratorRecord::AsyncFromSyncGenericIterator(iter) => iter.mark_values(queues),
            VmIteratorRecord::GenericIterator(iter) => iter.mark_values(queues),
            VmIteratorRecord::SliceIterator(_) => {}
            VmIteratorRecord::EmptySliceIterator => {}
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        match self {
            VmIteratorRecord::InvalidIterator { iterator } => iterator.sweep_values(compactions),
            VmIteratorRecord::ObjectProperties(iter) => iter.sweep_values(compactions),
            VmIteratorRecord::ArrayValues(iter) => iter.sweep_values(compactions),
            VmIteratorRecord::AsyncFromSyncGenericIterator(iter) => iter.sweep_values(compactions),
            VmIteratorRecord::GenericIterator(iter) => iter.sweep_values(compactions),
            VmIteratorRecord::SliceIterator(_) => {}
            VmIteratorRecord::EmptySliceIterator => {}
        }
    }
}

pub(crate) fn throw_iterator_returned_non_object<'a>(
    agent: &mut Agent,
    gc: NoGcScope<'a, '_>,
) -> JsError<'a> {
    agent.throw_exception_with_static_message(
        ExceptionType::TypeError,
        "Iterator returned a non-object result",
        gc,
    )
}

fn throw_iterator_method_undefined<'a>(agent: &mut Agent, gc: NoGcScope<'a, '_>) -> JsError<'a> {
    agent.throw_exception_with_static_message(
        ExceptionType::TypeError,
        "Iterator method cannot be undefined",
        gc,
    )
}
