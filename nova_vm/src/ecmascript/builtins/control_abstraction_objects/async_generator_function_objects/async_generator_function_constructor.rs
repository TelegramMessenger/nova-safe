// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{
        Agent, ArgumentsList, BUILTIN_STRING_MEMORY, Behaviour, Builtin,
        BuiltinIntrinsicConstructor, DynamicFunctionKind, Function, JsResult, Object,
        PropertyDescriptor, ProtoIntrinsics, Realm, String, Value,
        builders::BuiltinFunctionBuilder, create_dynamic_function,
        ordinary_object_create_with_intrinsics, try_define_property_or_throw, unwrap_try,
    },
    engine::{Bindable, GcScope},
    heap::IntrinsicConstructorIndexes,
};

pub(crate) struct AsyncGeneratorFunctionConstructor;
impl Builtin for AsyncGeneratorFunctionConstructor {
    const NAME: String<'static> = BUILTIN_STRING_MEMORY.AsyncGeneratorFunction;

    const LENGTH: u8 = 1;

    const BEHAVIOUR: Behaviour = Behaviour::Constructor(Self::constructor);
}
impl BuiltinIntrinsicConstructor for AsyncGeneratorFunctionConstructor {
    const INDEX: IntrinsicConstructorIndexes = IntrinsicConstructorIndexes::AsyncGeneratorFunction;
}

impl AsyncGeneratorFunctionConstructor {
    fn constructor<'gc>(
        agent: &mut Agent,
        _this_value: Value,
        arguments: ArgumentsList,
        new_target: Option<Object>,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        // 2. If bodyArg is not present, set bodyArg to the empty String.
        let (parameter_args, body_arg) = if arguments.is_empty() {
            (&[] as &[Value], String::EMPTY_STRING.into())
        } else {
            let (last, others) = arguments.split_last().unwrap();
            (others, *last)
        };
        let constructor = if let Some(new_target) = new_target {
            Function::try_from(new_target).unwrap()
        } else {
            agent.running_execution_context().function.unwrap()
        };
        // 3. Return ? CreateDynamicFunction(C, NewTarget, async, parameterArgs, bodyArg).
        let f = create_dynamic_function(
            agent,
            constructor,
            DynamicFunctionKind::AsyncGenerator,
            parameter_args,
            body_arg,
            gc.reborrow(),
        )
        .unbind()?;
        let gc = gc.into_nogc();
        let f = f.bind(gc);
        // 20.2.1.1.1 CreateDynamicFunction ( constructor, newTarget, kind, parameterArgs, bodyArg )
        // 31. Else if kind is async-generator, then
        //   a. Let prototype be OrdinaryObjectCreate(%AsyncGeneratorFunction.prototype.prototype%).
        let prototype = ordinary_object_create_with_intrinsics(
            agent,
            ProtoIntrinsics::Object,
            Some(
                agent
                    .current_realm_record()
                    .intrinsics()
                    .async_generator_prototype()
                    .into(),
            ),
            gc,
        );
        //   b. Perform ! DefinePropertyOrThrow(F, "prototype", PropertyDescriptor {
        //      [[Value]]: prototype, [[Writable]]: true, [[Enumerable]]: false,
        //      [[Configurable]]: false }).
        unwrap_try(try_define_property_or_throw(
            agent,
            f,
            BUILTIN_STRING_MEMORY.prototype.to_property_key(),
            PropertyDescriptor {
                value: Some(prototype.unbind().into()),
                writable: Some(true),
                enumerable: Some(false),
                configurable: Some(false),
                ..Default::default()
            },
            None,
            gc,
        ));

        Ok(f.into())
    }

    pub(crate) fn create_intrinsic(agent: &mut Agent, realm: Realm<'static>) {
        let intrinsics = agent.get_realm_record_by_id(realm).intrinsics();
        let function_constructor = intrinsics.function();
        let async_generator_function_prototype = intrinsics.async_generator_function_prototype();

        BuiltinFunctionBuilder::new_intrinsic_constructor::<AsyncGeneratorFunctionConstructor>(
            agent, realm,
        )
        .with_prototype(function_constructor)
        .with_property_capacity(1)
        .with_prototype_property(async_generator_function_prototype.into())
        .build();
    }
}
