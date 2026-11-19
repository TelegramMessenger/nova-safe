// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{
        Agent, ArgumentsList, BUILTIN_STRING_MEMORY, Behaviour, Builtin, CollectionIteratorKind,
        ExceptionType, Function, JsResult, Realm, RegularFn, SetIterator, String, Value,
        builders::OrdinaryObjectBuilder, create_array_from_list, create_iter_result_object,
    },
    engine::{Bindable, GcScope, NoGcScope},
    heap::{ArenaAccess, ArenaAccessMut, ArenaAccessSoA, WellKnownSymbols},
};

pub(crate) struct SetIteratorPrototype;

struct SetIteratorPrototypeNext;
impl Builtin for SetIteratorPrototypeNext {
    const NAME: String<'static> = BUILTIN_STRING_MEMORY.next;

    const LENGTH: u8 = 0;

    const BEHAVIOUR: Behaviour = Behaviour::Regular(SetIteratorPrototype::next);
}

impl SetIteratorPrototype {
    fn next<'gc>(
        agent: &mut Agent,
        this_value: Value,
        _arguments: ArgumentsList,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        let gc = gc.into_nogc();
        // 27.5.3.2 GeneratorValidate ( generator, generatorBrand )
        // 3. If generator.[[GeneratorBrand]] is not generatorBrand, throw a TypeError exception.
        let Value::SetIterator(iterator) = this_value else {
            return Err(agent.throw_exception_with_static_message(
                ExceptionType::TypeError,
                "SetIterator expected",
                gc,
            ));
        };
        match Self::step(agent, iterator, gc) {
            // b. Perform ? GeneratorYield(CreateIteratorResultObject(result, false)).
            Some(result) => create_iter_result_object(agent, result, false, gc),
            // e. Return undefined.
            None => create_iter_result_object(agent, Value::Undefined, true, gc),
        }
        .map(|o| o.into())
    }

    /// One step of the Set Iterator: the next value that is there, or the
    /// value twice in an Array, as its kind has it, which `next` wraps in
    /// an iterator result object; None where it reports done.
    pub(crate) fn step<'gc>(
        agent: &mut Agent,
        iterator: SetIterator,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<Value<'gc>> {
        let iterator = iterator.bind(gc);

        // 24.2.6.1 CreateSetIterator ( set, kind )
        // NOTE: We set `set` to None when the generator in the spec text has returned.
        let set = iterator.get(agent).set?;

        // b. Let entries be set.[[SetData]].
        // c. Let numEntries be the number of elements in entries.
        // d. Repeat, while index < numEntries,
        while iterator.get(agent).next_index < set.get(agent).values.len() {
            // i. Let e be entries[index].
            // ii. Set index to index + 1.
            let index = iterator.get(agent).next_index;
            iterator.get_mut(agent).next_index += 1;

            // iii. if e is not EMPTY, then
            let Some(e) = set.get(agent).values[index] else {
                continue;
            };

            let result = match iterator.get(agent).kind {
                CollectionIteratorKind::Key => unreachable!(),
                CollectionIteratorKind::KeyAndValue => {
                    // 1. If kind is KEY+VALUE, then
                    //   a. Let result be CreateArrayFromList(« e, e »).
                    //   b. Perform ? GeneratorYield(CreateIteratorResultObject(result, false)).
                    create_array_from_list(agent, &[e.unbind(), e.unbind()], gc).into()
                }
                CollectionIteratorKind::Value => {
                    // 2. Else,
                    //   a. Assert: kind is VALUE.
                    //   b. Perform ? GeneratorYield(CreateIteratorResultObject(e, false)).
                    e
                }
            };

            return Some(result.unbind().bind(gc));
        }

        debug_assert_eq!(iterator.get(agent).next_index, set.get(agent).values.len());

        // e. Return undefined.
        iterator.get_mut(agent).set = None;
        None
    }

    /// Whether `function` is %SetIteratorPrototype%.next, in any realm:
    /// what a call of it on a Set Iterator returns is [`Self::step`] in an
    /// iterator result object.
    pub(crate) fn is_next(agent: &Agent, function: Function) -> bool {
        let Function::BuiltinFunction(function) = function else {
            return false;
        };
        // Two functions at one address do the same thing.
        matches!(
            function.get(agent).behaviour,
            Behaviour::Regular(behaviour)
                if core::ptr::fn_addr_eq(behaviour, Self::next as RegularFn)
        )
    }

    pub(crate) fn create_intrinsic(agent: &mut Agent, realm: Realm<'static>) {
        let intrinsics = agent.get_realm_record_by_id(realm).intrinsics();
        let this = intrinsics.set_iterator_prototype();
        let iterator_prototype = intrinsics.iterator_prototype();

        OrdinaryObjectBuilder::new_intrinsic_object(agent, realm, this)
            .with_property_capacity(2)
            .with_prototype(iterator_prototype)
            .with_builtin_function_property::<SetIteratorPrototypeNext>()
            .with_property(|builder| {
                builder
                    .with_key(WellKnownSymbols::ToStringTag.into())
                    .with_value_readonly(BUILTIN_STRING_MEMORY.Set_Iterator.into())
                    .with_enumerable(false)
                    .with_configurable(true)
                    .build()
            })
            .build();
    }
}
