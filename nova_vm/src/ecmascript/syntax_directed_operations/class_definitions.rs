// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{
        Agent, ArgumentsList, BuiltinConstructorFunction, ECMAScriptCodeEvaluationState,
        ECMAScriptFunction, Environment, ExceptionType, ExecutionContext, Function,
        InternalMethods, JsResult, Object, ProtoIntrinsics, construct,
        initialize_instance_elements, is_constructor, new_class_field_initializer_environment,
        ordinary_create_from_constructor, unwrap_try,
    },
    engine::{Bindable, GcScope, Scopable, Vm},
    heap::ArenaAccess,
};

/// ### [7.3.33 InitializeInstanceElements ( O, constructor )](https://tc39.es/ecma262/#sec-initializeinstanceelements)
///
/// For a derived class constructor written in the class body: SuperCall
/// performs this on the object that super() returned. The constructor's
/// field and private method initializers are compiled into one Executable
/// that runs with `this` bound to that object.
pub(crate) fn initialize_derived_instance_elements<'a>(
    agent: &mut Agent,
    o: Object,
    constructor: ECMAScriptFunction,
    gc: GcScope<'a, '_>,
) -> JsResult<'a, ()> {
    let o = o.bind(gc.nogc());
    let constructor = constructor.bind(gc.nogc());
    let Some(bytecode) = constructor
        .get_executable(agent)
        .get(agent)
        .instance_initializer
        .bind(gc.nogc())
    else {
        return Ok(());
    };
    let data = &constructor.get(agent).ecmascript_function;
    let outer_env = data.environment.bind(gc.nogc());
    let outer_priv_env = data.private_environment.bind(gc.nogc());
    let source_code = data.source_code.bind(gc.nogc());
    let realm = data.realm.bind(gc.nogc());
    let script_or_module = data.script_or_module.bind(gc.nogc());
    let f = Function::from(constructor);
    // The initializers are methods whose HomeObject is the class prototype,
    // as the constructor's is.
    let home_object = data.home_object.bind(gc.nogc());
    let decl_env =
        new_class_field_initializer_environment(agent, f, o, home_object, outer_env, gc.nogc());
    agent.push_execution_context(ExecutionContext {
        ecmascript_code: Some(ECMAScriptCodeEvaluationState {
            lexical_environment: Environment::Function(decl_env.unbind()),
            variable_environment: Environment::Function(decl_env.unbind()),
            private_environment: outer_priv_env.unbind(),
            is_strict_mode: true,
            this_value: None,
            source_code: source_code.unbind(),
        }),
        function: Some(f.unbind()),
        realm: realm.unbind(),
        script_or_module: Some(script_or_module.unbind()),
    });
    let bytecode = bytecode.scope(agent, gc.nogc());
    let result = Vm::execute(agent, bytecode, None, gc).into_js_result();
    agent.pop_execution_context();
    result.map(|_| ())
}

pub(crate) fn base_class_default_constructor<'a>(
    agent: &mut Agent,
    new_target: Object,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, Object<'a>> {
    let new_target = new_target.bind(gc.nogc());
    // ii. If NewTarget is undefined, throw a TypeError exception.
    // Note: We've already checked this at an earlier level.

    // iii. Let F be the active function object.
    let f = BuiltinConstructorFunction::try_from(agent.active_function_object(gc.nogc()))
        .unwrap()
        .scope(agent, gc.nogc());

    // iv. If F.[[ConstructorKind]] is derived, then
    // v. Else,
    // 1. NOTE: This branch behaves similarly to constructor() {}.
    // 2. Let result be ? OrdinaryCreateFromConstructor(NewTarget, "%Object.prototype%").
    let result = ordinary_create_from_constructor(
        agent,
        Function::try_from(new_target.unbind()).unwrap(),
        ProtoIntrinsics::Object,
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    let scoped_result = result.scope(agent, gc.nogc());
    // vi. Perform ? InitializeInstanceElements(result, F).
    initialize_instance_elements(agent, result.unbind(), f.get(agent), gc.reborrow()).unbind()?;

    // vii. Return result.
    Ok(scoped_result.get(agent).bind(gc.into_nogc()))
}

pub(crate) fn derived_class_default_constructor<'a>(
    agent: &mut Agent,
    args: ArgumentsList,
    new_target: Object,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, Object<'a>> {
    let new_target = new_target.bind(gc.nogc());
    // i. Let args be the List of arguments that was passed to this function by [[Call]] or [[Construct]].
    // ii. If NewTarget is undefined, throw a TypeError exception.
    // Note: We've already checked this at an earlier level.

    // iii. Let F be the active function object.
    let f = BuiltinConstructorFunction::try_from(agent.active_function_object(gc.nogc())).unwrap();

    // iv. If F.[[ConstructorKind]] is derived, then
    // 1. NOTE: This branch behaves similarly to constructor(...args) { super(...args); }.
    // The most notable distinction is that while the aforementioned ECMAScript
    // source text observably calls the %Symbol.iterator% method on
    // %Array.prototype%, this function does not.

    // 2. Let func be ! F.[[GetPrototypeOf]]().
    let func = unwrap_try(f.try_get_prototype_of(agent, gc.nogc()));
    // 3. If IsConstructor(func) is false, throw a TypeError exception.
    let Some(func) = func.and_then(|func| is_constructor(agent, func)) else {
        return Err(agent.throw_exception_with_static_message(
            ExceptionType::TypeError,
            "Expected callable function",
            gc.into_nogc(),
        ));
    };
    let f = f.scope(agent, gc.nogc());
    // 4. Let result be ? Construct(func, args, NewTarget).
    let result = construct(
        agent,
        func.unbind(),
        Some(args),
        Some(Function::try_from(new_target.unbind()).unwrap()),
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    let scoped_result = result.scope(agent, gc.nogc());
    // vi. Perform ? InitializeInstanceElements(result, F).
    initialize_instance_elements(agent, result.unbind(), f.get(agent), gc.reborrow()).unbind()?;

    // vii. Return result.
    Ok(scoped_result.get(agent).bind(gc.into_nogc()))
}
