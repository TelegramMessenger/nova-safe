// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use core::ops::ControlFlow;
use oxc_span::Span;

use crate::{
    ecmascript::{
        Agent, ArgumentsList, Array, BUILTIN_STRING_MEMORY, BigInt, BindingSite,
        BuiltinConstructorArgs, ConstructorStatus, ECMAScriptFunction, Environment, ExceptionType,
        Function, InternalMethods, InternalSlots, JsResult, Number, Numeric, Object,
        OrdinaryFunctionCreateParams, OrdinaryObject, Primitive, PrivateMethod, Promise,
        PropertyDescriptor, PropertyKey, PropertyKeySet, PropertyLookupCache, ProtoIntrinsics,
        Reference, ScriptOrModule, SetFunctionNamePrefix, SetResult, String, TryError,
        TryGetValueContinue, TryHasResult, TryResult, Value, array_create, binding_site_of, call,
        call_function, call_proxy_set, construct, copy_data_properties,
        copy_data_properties_into_object, create_builtin_constructor,
        create_data_property_or_throw, create_unmapped_arguments_object, define_property_or_throw,
        evaluate_import_call, get_this_environment, get_this_value, get_value,
        global_scope_reference, has_property, initialize_derived_instance_elements, is_constructor,
        is_less_than, is_loosely_equal, is_private_reference, is_property_reference,
        is_strictly_equal, is_super_reference, is_unresolvable_reference, iterator_complete,
        iterator_value, make_constructor, make_method, new_class_static_element_environment,
        new_declarative_environment, new_private_environment, ordinary_function_create,
        ordinary_object_create_with_intrinsics, perform_eval, private_element_find, put_value,
        resolve_binding, resolve_private_identifier, resolve_this_binding,
        return_pooled_arguments_vec, set, set_function_name, throw_no_proxy_private_names,
        throw_read_undefined_or_null_error, to_boolean, to_number, to_number_primitive, to_numeric,
        to_numeric_primitive, to_object, to_property_key, to_property_key_complex,
        to_property_key_primitive, to_property_key_simple, to_string, to_string_primitive,
        try_copy_data_properties_into_object, try_create_data_property,
        try_define_property_or_throw, try_get_value, try_has_property,
        try_initialize_referenced_binding, try_put_value, try_resolve_binding,
        try_resolve_binding_at_site, try_result_into_js, try_result_into_option_js, unwrap_try,
    },
    engine::{
        ActiveIterator, Bindable, GcScope, NoGcScope, Scopable, ScopableCollection, Scoped,
        bytecode::{
            Executable, FunctionExpression, Instruction, NamedEvaluationParameter,
            executable::ArrowFunctionExpression,
            instructions::Instr,
            iterator::{ObjectPropertiesIteratorRecord, VmIteratorRecord},
        },
        throw_iterator_returned_non_object,
    },
    heap::{ArenaAccess, ArenaAccessMut, ObjectEntry},
};

use super::{
    ExceptionHandler, Vm, apply_string_or_numeric_addition,
    apply_string_or_numeric_binary_operator, bigint_binary_operator,
    binding_methods::{execute_simple_array_binding, execute_simple_object_binding},
    instanceof_operator, number_binary_operator, set_class_name, throw_error_in_target_not_object,
    typeof_operator, verify_is_object, with_vm_gc,
};

// Perform step 2 of `Await( value )` in the VM context.
pub(super) fn execute_await_promise_resolve<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let value = vm.result.take().unwrap();
    // 1. Let asyncContext be the running execution context.
    // 2. Let promise be ? PromiseResolve(%Promise%, value).
    let promise = with_vm_gc(
        agent,
        vm,
        |agent, gc| Promise::resolve(agent, value, gc),
        gc,
    )?;
    vm.result = Some(promise.unbind().into());
    Ok(())
}

pub(super) fn execute_array_create<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let result = array_create(agent, 0, instr.get_first_index(), None, gc)?;
    vm.result = Some(result.unbind().into());
    Ok(())
}

/// An Array of the values on top of the stack, in the order they were
/// pushed in: what creating an empty Array and CreateDataPropertyOrThrow of
/// each value in turn comes to, on an Array that nothing else has seen.
pub(super) fn execute_array_from_stack<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let count = instr.get_first_index();
    let first = vm.stack.len() - count;
    let array = array_create(agent, count, count, None, gc)?;
    for (slot, element) in array.as_mut_slice(agent).iter_mut().zip(&vm.stack[first..]) {
        *slot = Some(*element);
    }
    // The elements count for the heap's size as elements defined one by
    // one do.
    agent.heap.alloc_counter += count * core::mem::size_of::<Option<Value>>();
    vm.stack.truncate(first);
    vm.result = Some(array.unbind().into());
    Ok(())
}

pub(super) fn execute_array_push<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let value = vm.result.take().unwrap().bind(gc.nogc());
    let array = vm.stack.last().unwrap().bind(gc.nogc());
    let Ok(array) = Array::try_from(array) else {
        unreachable!();
    };
    let len = array.len(agent);
    let key = PropertyKey::Integer(len.into());
    let array = array.unbind();
    let value = value.unbind();
    with_vm_gc(
        agent,
        vm,
        |agent, gc| create_data_property_or_throw(agent, array, key, value, gc),
        gc,
    )?;
    Ok(())
}

pub(super) fn execute_array_elision<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let array = vm.stack.last().unwrap().bind(gc.nogc());
    let Ok(array) = Array::try_from(array) else {
        unreachable!();
    };
    let length = array.len(agent) + 1;
    let array = array.unbind().into();
    with_vm_gc(
        agent,
        vm,
        |agent, gc| {
            set(
                agent,
                array,
                BUILTIN_STRING_MEMORY.length.into(),
                length.into(),
                true,
                gc,
            )
        },
        gc,
    )?;
    Ok(())
}

pub(super) fn execute_bitwise_not<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // 2. Let oldValue be ? ToNumeric(? GetValue(expr)).
    // Note: This step is a separate instruction.
    let old_value = Numeric::try_from(vm.result.take().unwrap())
        .unwrap()
        .bind(gc);

    // 3. If oldValue is a Number, then
    if let Ok(old_value) = Number::try_from(old_value) {
        // a. Return Number::bitwiseNOT(oldValue).
        vm.result = Some(Number::bitwise_not(agent, old_value).unbind().into());
    } else {
        // 4. Else,
        // a. Assert: oldValue is a BigInt.
        let Ok(old_value) = BigInt::try_from(old_value) else {
            unreachable!();
        };

        // b. Return BigInt::bitwiseNOT(oldValue).
        let result = BigInt::bitwise_not(agent, old_value).check_size(agent, gc)?;
        vm.result = Some(result.unbind().into());
    }
    Ok(())
}

#[inline(never)]
#[cold]
pub(super) fn execute_debug(agent: &Agent, vm: &Vm) {
    if agent.options.print_internals {
        eprintln!("Debug: {vm:#?}");
    }
}

#[inline(always)]
pub(super) fn execute_resolve_binding<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let identifier = executable.fetch_identifier(agent, instr.get_first_index(), gc.nogc());

    let reference = if let TryResult::Continue(reference) =
        try_resolve_binding(agent, identifier, None, gc.nogc())
    {
        reference
    } else {
        let identifier = identifier.unbind();
        with_vm_gc(
            agent,
            vm,
            |agent, gc| resolve_binding(agent, identifier, None, None, gc),
            gc,
        )?
    };

    vm.reference = Some(reference.unbind());
    Ok(())
}

/// ResolveBinding where the scope chain is fixed: the name is looked for
/// where it was found the last time (BindingSite), and only when it is not
/// there resolved through the chain, which is then remembered.
#[inline(always)]
pub(super) fn execute_resolve_binding_at_site<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let identifier = executable.fetch_identifier(agent, instr.get_first_index(), gc.nogc());
    let site = executable.fetch_binding_site(agent, instr.get_second_index());
    let strict = agent.is_evaluating_strict_code();
    if let Some(reference) = try_resolve_binding_at_site(agent, site, identifier, strict, gc.nogc())
    {
        vm.reference = Some(reference.unbind());
        return Ok(());
    }
    execute_resolve_binding_at_site_cold(agent, vm, executable, instr, site, false, gc)
}

/// What is left of the instructions for a name of the global scope when the
/// name is not where it is remembered to be: the name is resolved through
/// the chain, or the Reference already made of it is read or written, as
/// GetValue and PutValue do it.
///
/// One function of its own, so that the instruction loop's frame, which
/// every nested call repeats, holds none of this.
#[inline(never)]
pub(super) fn execute_global_name_the_long_way<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    match instr.kind {
        Instruction::ResolveGlobalAtSite => {
            let site = executable.fetch_binding_site(agent, instr.get_second_index());
            execute_resolve_binding_at_site_cold(agent, vm, executable, instr, site, true, gc)
        }
        Instruction::GetGlobalAtSite => {
            execute_get_binding_at_site(agent, vm, executable, instr, true, gc)
        }
        Instruction::PutGlobalAtSite => {
            execute_put_binding_at_site(agent, vm, executable, instr, true, gc)
        }
        Instruction::PutValueAtSite => execute_put_value(agent, vm, executable, instr, false, gc),
        _ => unreachable!(),
    }
}

/// ResolveBinding the long way, remembering where the name was found.
/// `to_root`: the scope chain is fixed out to the global scope of a Script,
/// so a place there is remembered as well.
#[inline(never)]
fn execute_resolve_binding_at_site_cold<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    site: BindingSite,
    to_root: bool,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    execute_resolve_binding(agent, vm, executable.clone(), instr, gc.reborrow()).unbind()?;
    // A name once found outside the declarative records stays there. In the
    // global scope it can move (a lexical declaration is added, a property
    // of the global object is deleted and made again), so there it is looked
    // for again whenever it is not where it was.
    if (site == BindingSite::UNKNOWN || (to_root && site != BindingSite::ELSEWHERE))
        && let Some(reference) = &vm.reference
    {
        let found = binding_site_of(agent, reference, to_root, gc.nogc());
        if found != site {
            executable.set_binding_site(agent, instr.get_second_index(), found);
        }
    }
    Ok(())
}

/// GetBindingAtSite when the name was not where it is remembered to be, or
/// is not to be read from there (uninitialized): it is resolved through the
/// chain, which is remembered, and read as GetValue reads it.
pub(super) fn execute_get_binding_at_site<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    to_root: bool,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let site = executable.fetch_binding_site(agent, instr.get_second_index());
    execute_resolve_binding_at_site_cold(
        agent,
        vm,
        executable.clone(),
        instr,
        site,
        to_root,
        gc.reborrow(),
    )
    .unbind()?;
    execute_get_value(agent, vm, executable, instr, false, false, gc)
}

/// PutBindingAtSite when the name was not where it is remembered to be, or
/// is not to be written there (immutable, uninitialized): it is resolved
/// through the chain, which is remembered, and written as PutValue writes
/// it.
pub(super) fn execute_put_binding_at_site<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    to_root: bool,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let site = executable.fetch_binding_site(agent, instr.get_second_index());
    if to_root && site.is_global() {
        // The name is declared in the global scope, or was found there when
        // this assignment read it: the Reference that was resolved then is
        // to the global scope, and resolving the name again would ask the
        // global object for it once more than the assignment does.
        let name = executable.fetch_identifier(agent, instr.get_first_index(), gc.nogc());
        let strict = agent.is_evaluating_strict_code();
        let reference = global_scope_reference(agent, name, strict, gc.nogc());
        // Where the name is now is remembered for the next assignment: a
        // place among the properties of the global object is good only in
        // the state of the global scope it was found in, and nothing but
        // this would find it again for a name that is only assigned to.
        // Looking for the place asks the global object for nothing.
        let found = binding_site_of(agent, &reference, true, gc.nogc());
        if found != site {
            executable.set_binding_site(agent, instr.get_second_index(), found);
        }
        vm.reference = Some(reference.unbind());
    } else {
        execute_resolve_binding_at_site_cold(
            agent,
            vm,
            executable.clone(),
            instr,
            site,
            to_root,
            gc.reborrow(),
        )
        .unbind()?;
    }
    execute_put_value(agent, vm, executable, instr, false, gc)
}

pub(super) fn execute_resolve_binding_with_cache<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let identifier = executable.fetch_identifier(agent, instr.get_first_index(), gc.nogc());
    let cache = executable.fetch_cache(agent, instr.get_second_index(), gc.nogc());

    let reference = if let TryResult::Continue(reference) =
        try_resolve_binding(agent, identifier, Some(cache), gc.nogc())
    {
        reference
    } else {
        execute_resolve_binding_with_cache_cold(agent, vm, identifier.unbind(), cache.unbind(), gc)
            .unbind()?
    };

    vm.reference = Some(reference.unbind());
    Ok(())
}

#[cold]
fn execute_resolve_binding_with_cache_cold<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    identifier: String,
    cache: PropertyLookupCache,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Reference<'gc>> {
    let identifier = identifier.unbind();
    let cache = cache.unbind();
    with_vm_gc(
        agent,
        vm,
        |agent, gc| resolve_binding(agent, identifier, Some(cache), None, gc),
        gc,
    )
}

pub(super) fn execute_resolve_this_binding<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let this = resolve_this_binding(agent, gc)?.unbind();
    vm.result = Some(this);
    Ok(())
}

#[inline(always)]
pub(super) fn execute_load_constant(
    agent: &Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let constant = executable.fetch_constant(agent, instr.get_first_index(), gc);
    vm.stack.push(constant.unbind());
}

pub(super) fn execute_load_constant_wide(
    agent: &Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let constant = executable.fetch_wide_constant(agent, instr.get_wide_index(), gc);
    vm.stack.push(constant.unbind());
}

pub(super) fn execute_store_constant_wide(
    agent: &Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let constant = executable.fetch_wide_constant(agent, instr.get_wide_index(), gc);
    vm.result = Some(constant.unbind());
}

#[inline(always)]
pub(super) fn execute_store_constant(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let constant = executable.fetch_constant(agent, instr.get_first_index(), gc);
    vm.result = Some(constant.unbind());
}

pub(super) fn execute_unary_minus(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) {
    let old_value = vm.result.unwrap().bind(gc);

    // 3. If oldValue is a Number, then
    let result: Value = if let Ok(old_value) = Number::try_from(old_value) {
        // a. Return Number::unaryMinus(oldValue).
        Number::unary_minus(agent, old_value).into()
    }
    // 4. Else,
    else {
        // a. Assert: oldValue is a BigInt.
        let old_value = BigInt::try_from(old_value).unwrap();

        // b. Return BigInt::unaryMinus(oldValue).
        BigInt::unary_minus(agent, old_value).into()
    };
    vm.result = Some(result.unbind());
}

pub(super) fn execute_to_number<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let arg0 = vm.result.unwrap().bind(gc.nogc());
    let result = if let Ok(arg0) = Primitive::try_from(arg0) {
        to_number_primitive(agent, arg0.unbind(), gc.into_nogc())
    } else {
        let arg0 = arg0.unbind();
        with_vm_gc(agent, vm, |agent, gc| to_number(agent, arg0, gc), gc)
    };
    vm.result = Some(result?.unbind().into());
    Ok(())
}

#[inline(always)]
pub(super) fn execute_to_numeric<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let arg0 = vm.result.unwrap().bind(gc.nogc());
    // A Number is its own numeric value: the counter of nearly every loop.
    if Number::try_from(arg0).is_ok() {
        return Ok(());
    }
    let result = if let Ok(arg0) = Primitive::try_from(arg0) {
        to_numeric_primitive(agent, arg0.unbind(), gc.into_nogc())
    } else {
        execute_to_numeric_cold(agent, vm, arg0.unbind(), gc)
    };
    vm.result = Some(result?.unbind().into());
    Ok(())
}

#[inline(never)]
#[cold]
fn execute_to_numeric_cold<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    arg0: Value,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Numeric<'gc>> {
    with_vm_gc(agent, vm, |agent, gc| to_numeric(agent, arg0, gc), gc)
}

pub(super) fn execute_to_object<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    vm.result = Some(to_object(agent, vm.result.unwrap(), gc)?.unbind().into());
    Ok(())
}

pub(super) fn execute_apply_addition_binary_operator<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap();
    let rval = vm.result.take().unwrap();
    let result = if let (Ok(lnum), Ok(rnum)) = (Number::try_from(lval), Number::try_from(rval)) {
        Number::add(agent, lnum, rnum).into()
    } else if let (Ok(lstr), Ok(rstr)) = (String::try_from(lval), String::try_from(rval)) {
        let gc = gc.into_nogc();
        agent.check_concat_length(lstr, rstr, gc)?;
        String::concat(agent, [lstr, rstr], gc).into()
    } else if let (Ok(lnum), Ok(rnum)) = (BigInt::try_from(lval), BigInt::try_from(rval)) {
        BigInt::add(agent, lnum, rnum)
            .check_size(agent, gc.into_nogc())?
            .into()
    } else if let Some((lprim, rprim)) = string_and_primitive(lval, rval) {
        // A String and a Number, or another primitive that ToString takes:
        // nothing is converted that could run a script, and the Vm stays
        // where it is.
        let gc = gc.into_nogc();
        let lstr = to_string_primitive(agent, lprim.bind(gc), gc)?;
        let rstr = to_string_primitive(agent, rprim.bind(gc), gc)?;
        agent.check_concat_length(lstr, rstr, gc)?;
        String::concat(agent, [lstr, rstr], gc).into()
    } else {
        with_vm_gc(
            agent,
            vm,
            |agent, gc| apply_string_or_numeric_addition(agent, lval, rval, gc),
            gc,
        )?
    };
    vm.result = Some(result.unbind());
    Ok(())
}

/// The operands of a `+` as primitives, where one is a String and the other
/// a primitive that ToString converts: not a Symbol, which it throws for.
fn string_and_primitive<'a>(
    lval: Value<'a>,
    rval: Value<'a>,
) -> Option<(Primitive<'a>, Primitive<'a>)> {
    let (Ok(lprim), Ok(rprim)) = (Primitive::try_from(lval), Primitive::try_from(rval)) else {
        return None;
    };
    let is_string = |primitive: Primitive| {
        matches!(primitive, Primitive::String(_) | Primitive::SmallString(_))
    };
    let is_symbol = |primitive: Primitive| matches!(primitive, Primitive::Symbol(_));
    ((is_string(lprim) || is_string(rprim)) && !is_symbol(lprim) && !is_symbol(rprim))
        .then_some((lprim, rprim))
}

pub(super) fn execute_apply_binary_operator<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instruction: Instruction,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap();
    let rval = vm.result.take().unwrap();
    let result = if let (Ok(lnum), Ok(rnum)) = (Number::try_from(lval), Number::try_from(rval)) {
        number_binary_operator(agent, instruction, lnum, rnum, gc.into_nogc())?.into()
    } else if let (Ok(lnum), Ok(rnum)) = (BigInt::try_from(lval), BigInt::try_from(rval)) {
        bigint_binary_operator(agent, instruction, lnum, rnum, gc.into_nogc())?.into()
    } else {
        with_vm_gc(
            agent,
            vm,
            |agent, gc| apply_string_or_numeric_binary_operator(agent, lval, instruction, rval, gc),
            gc,
        )?
    };
    vm.result = Some(result.unbind());
    Ok(())
}

pub(super) fn execute_object_define_property<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let key = vm.stack.pop().unwrap();
    let key = with_vm_gc(
        agent,
        vm,
        |agent, gc| to_property_key(agent, key, gc),
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    let key = key.unbind().bind(gc.nogc());
    let value = vm.result.take().unwrap().bind(gc.nogc());
    let object = vm.stack.last().unwrap().bind(gc.nogc());
    let object = Object::try_from(object).unwrap();

    let (object, key, value) = (object.unbind(), key.unbind(), value.unbind());
    with_vm_gc(
        agent,
        vm,
        |agent, gc| create_data_property_or_throw(agent, object, key, value, gc),
        gc,
    )?;
    Ok(())
}

pub(super) fn execute_object_define_method<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let enumerable = instr.get_second_bool();
    // 1. Let propKey be ? Evaluation of ClassElementName.
    let prop_key = vm.stack.pop().unwrap();
    let prop_key = with_vm_gc(
        agent,
        vm,
        |agent, gc| to_property_key(agent, prop_key, gc),
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    let object = Object::try_from(*vm.stack.last().unwrap())
        .unwrap()
        .bind(gc.nogc());

    // 2. Let env be the running execution context's LexicalEnvironment.
    let env = agent.current_lexical_environment(gc.nogc());
    // 3. Let privateEnv be the running execution context's PrivateEnvironment.
    let private_env = agent.current_private_environment(gc.nogc());
    // Note: Non-constructor methods never have a function
    // prototype.
    // 4. If functionPrototype is present, then
    //     a. Let prototype be functionPrototype.
    // 5. Else,
    //     a. Let prototype be %Function.prototype%.
    let template = executable
        .fetch_function_expression(agent, instr.get_first_index(), gc.nogc())
        .template;
    let params = OrdinaryFunctionCreateParams {
        function_prototype: None,
        source_code: None,
        // 4. Let sourceText be the source text matched by MethodDefinition.
        template,
        lexical_this: false,
        env,
        private_env,
    };
    // 7. Let closure be OrdinaryFunctionCreate(
    //      prototype,
    //      sourceText,
    //      UniqueFormalParameters,
    //      FunctionBody,
    //      non-lexical-this,
    //      env,
    //      privateEnv
    //  ).
    let is_generator = params.template.kind.is_generator();
    let is_async = params.template.kind.is_async();
    let closure = ordinary_function_create(agent, params, gc.nogc());
    // 8. Perform MakeMethod(closure, object).
    make_method(agent, closure, object);
    // 2. Perform SetFunctionName(closure, propKey).
    set_function_name(agent, closure, prop_key, None, gc.nogc());
    if is_generator {
        define_generator_prototype(agent, closure, is_async, gc.nogc());
    }
    // 3. Return ? DefineMethodProperty(
    //      object,
    //      methodDef.[[Key]],
    //      methodDef.[[Closure]],
    //      enumerable
    // ).
    // 2. If key is a Private Name, then
    // a. Return PrivateElement {
    //      [[Key]]: key,
    //      [[Kind]]: method,
    //      [[Value]]: closure
    // }.
    // 3. Else,
    // a. Let desc be the PropertyDescriptor {
    //      [[Value]]: closure,
    //      [[Writable]]: true,
    //      [[Enumerable]]: enumerable,
    //      [[Configurable]]: true
    // }.
    let desc = PropertyDescriptor {
        value: Some(closure.unbind().into()),
        writable: Some(true),
        enumerable: Some(enumerable),
        configurable: Some(true),
        ..Default::default()
    };
    // b. Perform ? DefinePropertyOrThrow(homeObject, key, desc).
    // c. NOTE: DefinePropertyOrThrow only returns an abrupt
    // completion when attempting to define a class static method whose key is "prototype".

    let object = object.unbind();
    let prop_key = prop_key.unbind();
    with_vm_gc(
        agent,
        vm,
        |agent, gc| define_property_or_throw(agent, object, prop_key, desc, gc),
        gc,
    )?;
    // c. Return unused.
    Ok(())
}

/// The "prototype" property every generator function and generator method
/// gets: steps 7 and 8 of InstantiateGeneratorFunctionExpression, and of
/// GeneratorMethod's and AsyncGeneratorMethod's MethodDefinitionEvaluation.
fn define_generator_prototype(
    agent: &mut Agent,
    function: ECMAScriptFunction,
    is_async: bool,
    gc: NoGcScope,
) {
    // 7. Let prototype be OrdinaryObjectCreate(%GeneratorFunction.prototype.prototype%).
    // NOTE: Although `prototype` has the generator prototype, it doesn't have the generator
    // internals slots, so it's created as an ordinary object.
    let intrinsics = agent.current_realm_record().intrinsics();
    let proto = if is_async {
        intrinsics.async_generator_prototype().into()
    } else {
        intrinsics.generator_prototype().into()
    };
    let prototype =
        ordinary_object_create_with_intrinsics(agent, ProtoIntrinsics::Object, Some(proto), gc);
    // 8. Perform ! DefinePropertyOrThrow(F, "prototype", PropertyDescriptor { [[Value]]: prototype, [[Writable]]: true, [[Enumerable]]: false, [[Configurable]]: false }).
    unwrap_try(try_define_property_or_throw(
        agent,
        function,
        BUILTIN_STRING_MEMORY.prototype.to_property_key(),
        PropertyDescriptor {
            value: Some(prototype.unbind().into()),
            writable: Some(true),
            get: None,
            set: None,
            enumerable: Some(false),
            configurable: Some(false),
        },
        None,
        gc,
    ));
}

pub(super) fn execute_object_define_getter<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let enumerable = instr.get_second_bool();
    // 1. Let propKey be ? Evaluation of ClassElementName.
    let prop_key = vm.stack.pop().unwrap();
    let prop_key = with_vm_gc(
        agent,
        vm,
        |agent, gc| to_property_key(agent, prop_key, gc),
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    // 2. Let env be the running execution context's LexicalEnvironment.
    let env = agent.current_lexical_environment(gc.nogc());
    // 3. Let privateEnv be the running execution context's PrivateEnvironment.
    let private_env = agent.current_private_environment(gc.nogc());
    // 5. Let formalParameterList be an instance of the production FormalParameters : [empty] .
    // We have to create a temporary allocator to create the empty
    // items Vec. The allocator will never be asked to allocate
    // anything.
    let template = executable
        .fetch_function_expression(agent, instr.get_first_index(), gc.nogc())
        .template;
    let params = OrdinaryFunctionCreateParams {
        function_prototype: None,
        source_code: None,
        // 4. Let sourceText be the source text matched by MethodDefinition.
        template,
        lexical_this: false,
        env,
        private_env,
    };
    // 6. Let closure be OrdinaryFunctionCreate(
    //      %Function.prototype%,
    //      sourceText,
    //      formalParameterList,
    //      FunctionBody,
    //      non-lexical-this,
    //      env,
    //      privateEnv
    //  ).
    let closure = ordinary_function_create(agent, params, gc.nogc());
    // 7. Perform MakeMethod(closure, object).
    let object = Object::try_from(*vm.stack.last().unwrap())
        .unwrap()
        .bind(gc.nogc());
    make_method(agent, closure, object);
    // 8. Perform SetFunctionName(closure, propKey, "get").
    set_function_name(
        agent,
        closure,
        prop_key,
        Some(SetFunctionNamePrefix::Get),
        gc.nogc(),
    );
    // 9. If propKey is a Private Name, then
    // a. Return PrivateElement { [[Key]]: propKey, [[Kind]]: accessor, [[Get]]: closure, [[Set]]: undefined }.
    // 10. Else,
    // a. Let desc be the PropertyDescriptor {
    let desc = PropertyDescriptor {
        // [[Get]]: closure,
        get: Some(Some(closure.unbind().into())),
        // [[Enumerable]]: enumerable,
        enumerable: Some(enumerable),
        // [[Configurable]]: true
        configurable: Some(true),
        ..Default::default()
    };
    // }.
    // b. Perform ? DefinePropertyOrThrow(object, propKey, desc).
    let object = object.unbind();
    let prop_key = prop_key.unbind();
    with_vm_gc(
        agent,
        vm,
        |agent, gc| define_property_or_throw(agent, object, prop_key, desc, gc),
        gc,
    )?;
    // c. Return unused.
    Ok(())
}

pub(super) fn execute_object_define_setter<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let enumerable = instr.get_second_bool();
    // 1. Let propKey be ? Evaluation of ClassElementName.
    let prop_key = vm.stack.pop().unwrap();
    let prop_key = with_vm_gc(
        agent,
        vm,
        |agent, gc| to_property_key(agent, prop_key, gc),
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    // 2. Let env be the running execution context's LexicalEnvironment.
    let env = agent.current_lexical_environment(gc.nogc());
    // 3. Let privateEnv be the running execution context's PrivateEnvironment.
    let private_env = agent.current_private_environment(gc.nogc());
    let template = executable
        .fetch_function_expression(agent, instr.get_first_index(), gc.nogc())
        .template;
    let params = OrdinaryFunctionCreateParams {
        function_prototype: None,
        source_code: None,
        // 4. Let sourceText be the source text matched by MethodDefinition.
        template,
        lexical_this: false,
        env,
        private_env,
    };
    // 5. Let closure be OrdinaryFunctionCreate(
    //      %Function.prototype%,
    //      sourceText,
    //      PropertySetParameterList,
    //      FunctionBody,
    //      non-lexical-this,
    //      env,
    //      privateEnv
    //  ).
    let closure = ordinary_function_create(agent, params, gc.nogc());
    // 6. Perform MakeMethod(closure, object).
    let object = Object::try_from(*vm.stack.last().unwrap())
        .unwrap()
        .bind(gc.nogc());
    make_method(agent, closure, object);
    // 7. Perform SetFunctionName(closure, propKey, "set").
    set_function_name(
        agent,
        closure,
        prop_key,
        Some(SetFunctionNamePrefix::Set),
        gc.nogc(),
    );
    // 8. If propKey is a Private Name, then
    // a. Return PrivateElement { [[Key]]: propKey, [[Kind]]: accessor, [[Get]]: undefined, [[Set]]: closure }.
    // 9. Else,
    // a. Let desc be the PropertyDescriptor {
    let desc = PropertyDescriptor {
        // [[Set]]: closure,
        set: Some(Some(closure.unbind().into())),
        // [[Enumerable]]: enumerable,
        enumerable: Some(enumerable),
        // [[Configurable]]: true
        configurable: Some(true),
        ..Default::default()
    };
    // }.
    // b. Perform ? DefinePropertyOrThrow(object, propKey, desc).
    let object = object.unbind();
    let prop_key = prop_key.unbind();
    with_vm_gc(
        agent,
        vm,
        |agent, gc| define_property_or_throw(agent, object, prop_key, desc, gc),
        gc,
    )?;
    // c. Return unused.
    Ok(())
}

pub(super) fn execute_object_set_prototype<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let prop_value = vm.result.take().unwrap().bind(gc.nogc());
    // i. Perform ! object.[[SetPrototypeOf]](propValue).
    let object = Object::try_from(*vm.stack.last().unwrap())
        .unwrap()
        .bind(gc.nogc());

    // a. If propValue is an Object or propValue is null, then
    let prop_value = if prop_value.is_null() {
        None
    } else if let Ok(prop_value) = Object::try_from(prop_value) {
        Some(prop_value.unbind())
    } else {
        // b. Return unused.
        return Ok(());
    };

    let object = object.unbind();
    with_vm_gc(
        agent,
        vm,
        |agent, gc| object.internal_set_prototype_of(agent, prop_value, gc),
        gc,
    )?;
    // b. Return unused.
    Ok(())
}

#[inline(always)]
pub(super) fn execute_put_value<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    cache: bool,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let value = vm.result.take().unwrap().bind(gc.nogc());
    let mut reference = vm.reference.take().unwrap().bind(gc.nogc());

    let cache = if cache {
        Some(executable.fetch_cache(agent, instr.get_first_index(), gc.nogc()))
    } else {
        None
    };

    let result = try_put_value(agent, &mut reference, value, cache, gc.nogc());
    match result {
        ControlFlow::Continue(SetResult::Done)
        | ControlFlow::Continue(SetResult::Unwritable)
        | ControlFlow::Continue(SetResult::Accessor) => {}
        ControlFlow::Break(TryError::Err(err)) => {
            return Err(err.unbind().bind(gc.into_nogc()));
        }
        _ => handle_set_value_break(
            agent,
            vm,
            &reference.unbind(),
            result.unbind(),
            value.unbind(),
            gc,
        )?,
    };
    Ok(())
}

#[inline(always)]
pub(super) fn execute_get_value<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    cache: bool,
    keep_reference: bool,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // 1. If V is not a Reference Record, return V.
    let reference = if keep_reference {
        handle_keep_reference(agent, vm, gc.reborrow())
            .unbind()?
            .bind(gc.nogc())
    } else {
        vm.reference.take().unwrap()
    };

    let cache = if cache {
        Some(executable.fetch_cache(agent, instr.get_first_index(), gc.nogc()))
    } else {
        None
    };

    let result = try_get_value(agent, &reference, cache, gc.nogc());
    let result = match result {
        ControlFlow::Continue(TryGetValueContinue::Unset) => Value::Undefined,
        ControlFlow::Continue(TryGetValueContinue::Value(value)) => value,
        ControlFlow::Break(TryError::Err(err)) => {
            return Err(err.unbind().bind(gc.into_nogc()));
        }
        _ => handle_get_value_break(agent, vm, &reference.unbind(), result.unbind(), gc)?,
    };
    vm.result = Some(result.unbind());
    Ok(())
}

#[cold]
fn handle_keep_reference<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Reference<'gc>> {
    let reference = vm.reference.as_mut().unwrap();
    if is_property_reference(reference) && !reference.is_static_property_reference() {
        if let Ok(referenced_name) =
            Primitive::try_from(reference.referenced_name_value().bind(gc.nogc()))
        {
            let referenced_name = to_property_key_primitive(agent, referenced_name, gc.nogc());
            reference.set_referenced_name_to_property_key(referenced_name);
            Ok(reference.clone().bind(gc.into_nogc()))
        } else {
            mutate_reference_property_key(agent, vm, gc)
        }
    } else {
        Ok(reference.clone().bind(gc.into_nogc()))
    }
}

pub(super) fn execute_typeof<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // 2. If val is a Reference Record, then
    let val = if let Some(reference) = vm.reference.take() {
        // a. If IsUnresolvableReference(val) is true,
        if is_unresolvable_reference(&reference) {
            // return "undefined".
            Value::Undefined
        } else {
            // 3. Set val to ? GetValue(val).
            let result = try_get_value(agent, &reference, None, gc.nogc());
            match result {
                ControlFlow::Continue(TryGetValueContinue::Unset) => Value::Undefined,
                ControlFlow::Continue(TryGetValueContinue::Value(value)) => value,
                ControlFlow::Break(TryError::Err(err)) => {
                    return Err(err.unbind().bind(gc.into_nogc()));
                }
                _ => with_vm_gc(
                    agent,
                    vm,
                    |agent, gc| get_value(agent, &reference, gc),
                    gc.reborrow(),
                )
                .unbind()?
                .bind(gc.nogc()),
            }
        }
    } else {
        vm.result.unwrap().bind(gc.nogc())
    };
    vm.result = Some(typeof_operator(agent, val, gc.nogc()).into());
    Ok(())
}

pub(super) fn execute_object_create(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) {
    let object = ordinary_object_create_with_intrinsics(agent, ProtoIntrinsics::Object, None, gc);
    vm.stack.push(object.unbind().into());
}

pub(super) fn execute_object_create_with_shape(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let shape = executable.fetch_object_shape(agent, instr.get_first_index(), gc);
    let len = shape.len(agent);
    let first_property_index = vm.stack.len() - len as usize;
    let obj = OrdinaryObject::create_object_with_shape_and_data_properties(
        agent,
        shape,
        &vm.stack[first_property_index..],
    );
    vm.stack.truncate(first_property_index);
    vm.result = Some(obj.unbind().into());
}

pub(super) fn execute_copy_data_properties<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let source = vm.result.take().unwrap();
    let Value::Object(target) = *vm.stack.last().unwrap() else {
        unreachable!()
    };
    with_vm_gc(
        agent,
        vm,
        |agent, gc| copy_data_properties(agent, target, source, gc),
        gc,
    )?;
    Ok(())
}

pub(super) fn execute_copy_data_properties_into_object<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let from = Object::try_from(vm.result.unwrap())
        .unwrap()
        .bind(gc.nogc());

    let num_excluded_items = instr.get_first_index();
    let mut excluded_items = PropertyKeySet::with_capacity(num_excluded_items, gc.nogc());
    assert!(vm.reference.is_none());
    for _ in 0..num_excluded_items {
        let reference = vm.reference_stack.pop().unwrap();
        debug_assert_eq!(reference.base_value(), from.into());
        debug_assert!(!is_super_reference(&reference));
        excluded_items.insert(agent, reference.referenced_name_property_key());
    }

    if let TryResult::Continue(result) =
        try_copy_data_properties_into_object(agent, from, &excluded_items, gc.nogc())
    {
        vm.result = Some(result.unbind().into());
    } else {
        let from = from.unbind();
        let excluded_items = excluded_items.scope(agent, gc.nogc());
        let result = with_vm_gc(
            agent,
            vm,
            |agent, gc| copy_data_properties_into_object(agent, from, excluded_items, gc),
            gc,
        )?;
        vm.result = Some(result.unbind().into());
    }
    Ok(())
}

pub(super) fn execute_instantiate_arrow_function_expression<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // ArrowFunction : ArrowParameters => ConciseBody
    let ArrowFunctionExpression {
        template,
        identifier,
    } = *executable.fetch_arrow_function_expression(agent, instr.get_first_index(), gc.nogc());
    // 2. Let env be the LexicalEnvironment of the running execution context.
    let env = agent.current_lexical_environment(gc.nogc());
    // 3. Let privateEnv be the running execution context's PrivateEnvironment.
    let private_env = agent.current_private_environment(gc.nogc());
    // 4. Let sourceText be the source text matched by ArrowFunction.
    // 5. Let closure be OrdinaryFunctionCreate(%Function.prototype%, sourceText, ArrowParameters, ConciseBody, LEXICAL-THIS, env, privateEnv).
    // 6. Perform SetFunctionName(closure, name).
    // 7. Return closure.
    // 1. If name is not present, set name to "".
    let params = OrdinaryFunctionCreateParams {
        function_prototype: None,
        source_code: None,
        template,
        lexical_this: true,
        env,
        private_env,
    };
    let mut function = ordinary_function_create(agent, params, gc.nogc());
    let name = if let Some(parameter) = &identifier {
        let pk_result = match parameter {
            NamedEvaluationParameter::Result => {
                let value = vm.result.take().unwrap().bind(gc.nogc());
                if let Some(pk) = to_property_key_simple(agent, value, gc.nogc()) {
                    Ok(pk)
                } else {
                    Err(value)
                }
            }
            NamedEvaluationParameter::Stack => {
                let value = vm.stack.last().unwrap().bind(gc.nogc());
                if let Some(pk) = to_property_key_simple(agent, value, gc.nogc()) {
                    Ok(pk)
                } else {
                    Err(value)
                }
            }
        };

        match pk_result {
            Ok(pk) => pk.bind(gc.nogc()),
            Err(pk_value) => {
                let scoped_function = function.scope(agent, gc.nogc());
                let pk_value = pk_value.unbind();
                let pk = with_vm_gc(
                    agent,
                    vm,
                    |agent, gc| to_property_key_complex(agent, pk_value, gc),
                    gc.reborrow(),
                )
                .unbind()?
                .bind(gc.nogc());
                function = scoped_function.take(agent).bind(gc.nogc());
                pk
            }
        }
    } else {
        let pk: PropertyKey = String::EMPTY_STRING.into();
        pk.bind(gc.nogc())
    };
    set_function_name(agent, function, name, None, gc.nogc());
    vm.result = Some(function.unbind().into());
    Ok(())
}

pub(super) fn execute_instantiate_ordinary_function_expression<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let FunctionExpression {
        template,
        identifier,
        binding_name,
        is_expression,
        is_declaration,
    } = *executable.fetch_function_expression(agent, instr.get_first_index(), gc.nogc());
    let binding_name = binding_name.unbind();
    let kind = template.kind;

    let (name, env, initialize_binding) = if let Some(parameter) = identifier {
        debug_assert!(binding_name.is_none());
        // 1. If name is not present, set name to "".
        let pk = match parameter {
            NamedEvaluationParameter::Result => vm.result.take().unwrap(),
            NamedEvaluationParameter::Stack => *vm.stack.last().unwrap(),
        };
        let name = with_vm_gc(
            agent,
            vm,
            |agent, gc| to_property_key(agent, pk, gc),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        // 2. Let env be the LexicalEnvironment of the running execution
        //    context.
        let env = agent.current_lexical_environment(gc.nogc());
        // 3. Let privateEnv be the running execution context's
        //    PrivateEnvironment.
        // 4. Let sourceText be the source text matched by FunctionExpression.
        // 5. Let closure be OrdinaryFunctionCreate(%Function.prototype%,
        //    sourceText, FormalParameters, FunctionBody, non-lexical-this, env,
        //    privateEnv).
        // 6. Perform SetFunctionName(closure, name).
        // 7. Perform MakeConstructor(closure).
        let initialize_binding = false;
        // 8. Return closure.

        (name, env, initialize_binding)
    } else if let Some(binding_name) = binding_name
        && is_expression
    {
        // 1. Assert: name is not present.
        assert!(identifier.is_none());
        // 2. Set name to the StringValue of BindingIdentifier.
        let name = binding_name.bind(gc.nogc());
        // 3. Let outerEnv be the running execution context's LexicalEnvironment.
        let outer_env = agent.current_lexical_environment(gc.nogc());
        // 4. Let funcEnv be NewDeclarativeEnvironment(outerEnv).
        let func_env = new_declarative_environment(agent, Some(outer_env), gc.nogc());
        // 5. Perform ! funcEnv.CreateImmutableBinding(name, false).
        func_env.create_immutable_binding(agent, name, false);
        // 7. Let sourceText be the source text matched by FunctionExpression.
        // 8. Let closure be OrdinaryFunctionCreate(%Function.prototype%,
        //    sourceText, FormalParameters, FunctionBody, non-lexical-this,
        //    funcEnv, privateEnv).
        // 9. Perform SetFunctionName(closure, name).
        // 10. Perform MakeConstructor(closure).
        // 11. Perform ! funcEnv.InitializeBinding(name, closure).
        let initialize_binding = true;
        // 12. Return closure.

        // > Note
        // >
        // > The BindingIdentifier in a FunctionExpression can be referenced
        // > from inside the FunctionExpression's FunctionBody to allow the
        // > function to call itself recursively. However, unlike in a
        // > FunctionDeclaration, the BindingIdentifier in a FunctionExpression
        // > cannot be referenced from and does not affect the scope enclosing
        // > the FunctionExpression.

        (
            name.into(),
            Environment::Declarative(func_env),
            initialize_binding,
        )
    } else if let Some(binding_name) = binding_name
        && is_declaration
    {
        // 1. Let name be the StringValue of BindingIdentifier.
        let name = binding_name.bind(gc.nogc());
        // 2. Let sourceText be the source text matched by FunctionDeclaration.
        // 3. Let F be OrdinaryFunctionCreate(%Function.prototype%, sourceText,
        //    FormalParameters, FunctionBody, non-lexical-this, env,
        //    privateEnv).
        // 4. Perform SetFunctionName(F, name).
        // 5. Perform MakeConstructor(F).
        let initialize_binding = false;
        // 6. Return F.
        (
            name.into(),
            agent.current_lexical_environment(gc.nogc()),
            initialize_binding,
        )
    } else {
        (
            String::EMPTY_STRING.into(),
            agent.current_lexical_environment(gc.nogc()),
            false,
        )
    };

    // 6. Let privateEnv be the running execution context's PrivateEnvironment.
    let private_env = agent.current_private_environment(gc.nogc());
    let template = executable
        .fetch_function_expression(agent, instr.get_first_index(), gc.nogc())
        .template;
    let params = OrdinaryFunctionCreateParams {
        function_prototype: None,
        source_code: None,
        template,
        lexical_this: false,
        env,
        private_env,
    };
    let function = ordinary_function_create(agent, params, gc.nogc());
    set_function_name(agent, function, name, None, gc.nogc());
    if !kind.is_async() && !kind.is_generator() {
        make_constructor(agent, function, None, None, gc.nogc());
    }

    if kind.is_generator() {
        // InstantiateGeneratorFunctionExpression
        // 7.-8.
        define_generator_prototype(agent, function, kind.is_async(), gc.nogc());
    }

    if initialize_binding {
        let name = match name {
            PropertyKey::SmallString(data) => data.into(),
            PropertyKey::String(data) => data.unbind().into(),
            _ => unreachable!(),
        };

        unwrap_try(env.try_initialize_binding(agent, name, None, function.into(), gc.nogc()));
    }

    vm.result = Some(function.unbind().into());
    Ok(())
}

pub(super) fn execute_class_define_constructor<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let name = vm.stack.pop().unwrap();
    let class_name = set_class_name(agent, vm, name, gc.reborrow())
        .unbind()?
        .bind(gc.nogc());

    let template = executable
        .fetch_function_expression(agent, instr.get_first_index(), gc.nogc())
        .template;
    let has_constructor_parent = instr.get_second_bool();

    let function_prototype = if has_constructor_parent {
        Some(Object::try_from(vm.stack.pop().unwrap()).unwrap())
    } else {
        None
    };
    let proto = OrdinaryObject::try_from(*vm.stack.last().unwrap()).unwrap();

    let is_null_derived_class = !has_constructor_parent
        && unwrap_try(proto.try_get_prototype_of(agent, gc.nogc())).is_none();

    let env = agent.current_lexical_environment(gc.nogc());
    let private_env = agent.current_private_environment(gc.nogc());

    let params = OrdinaryFunctionCreateParams {
        function_prototype,
        source_code: None,
        template,
        lexical_this: false,
        env,
        private_env,
    };
    let function = ordinary_function_create(agent, params, gc.nogc());
    set_function_name(agent, function, class_name.into(), None, gc.nogc());
    make_constructor(agent, function, Some(false), Some(proto), gc.nogc());
    function.get_mut(agent).ecmascript_function.home_object = Some(proto.into());
    function
        .get_mut(agent)
        .ecmascript_function
        .constructor_status = if has_constructor_parent || is_null_derived_class {
        ConstructorStatus::DerivedClass
    } else {
        ConstructorStatus::BaseClass
    };

    unwrap_try(proto.try_define_own_property(
        agent,
        BUILTIN_STRING_MEMORY.constructor.into(),
        PropertyDescriptor {
            value: Some(function.unbind().into()),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(true),
            ..Default::default()
        },
        None,
        gc.nogc(),
    ));

    vm.result = Some(function.unbind().into());
    Ok(())
}

pub(super) fn execute_class_define_default_constructor<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let name = vm.stack.pop().unwrap();
    let class_name = set_class_name(agent, vm, name, gc.reborrow())
        .unbind()?
        .bind(gc.nogc());

    let class_initializer_bytecode_index = instr.get_first_index();
    let (compiled_initializer_bytecode, has_constructor_parent) = executable
        .fetch_class_initializer_bytecode(agent, class_initializer_bytecode_index, gc.nogc());
    let function_prototype = if has_constructor_parent {
        Some(Object::try_from(vm.stack.pop().unwrap()).unwrap())
    } else {
        Some(
            agent
                .current_realm_record()
                .intrinsics()
                .function_prototype()
                .into(),
        )
    };
    let proto = Object::try_from(*vm.stack.last().unwrap()).unwrap();

    let env = agent.current_lexical_environment(gc.nogc());
    let private_env = agent.current_private_environment(gc.nogc());
    let source_code = agent.current_source_code(gc.nogc());

    let function = create_builtin_constructor(
        agent,
        BuiltinConstructorArgs {
            class_name,
            is_derived: has_constructor_parent,
            prototype: function_prototype,
            prototype_property: proto,
            compiled_initializer_bytecode,
            env,
            private_env,
            source_code,
            source_text: Span::new(0, 0),
        },
        gc.nogc(),
    );

    unwrap_try(proto.try_define_own_property(
        agent,
        BUILTIN_STRING_MEMORY.constructor.into(),
        PropertyDescriptor {
            value: Some(function.unbind().into()),
            writable: Some(true),
            enumerable: Some(false),
            configurable: Some(true),
            ..Default::default()
        },
        None,
        gc.nogc(),
    ));

    vm.result = Some(function.unbind().into());
    Ok(())
}

pub(super) fn execute_class_define_private_method<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let description = String::try_from(vm.result.take().unwrap().bind(gc.nogc())).unwrap();
    let template = executable
        .fetch_function_expression(agent, instr.get_first_index(), gc.nogc())
        .template;
    let bits = instr.get_second_index() as u8;
    let is_static = bits & 0b100 == 0b100;
    let is_setter = bits & 0b10 == 0b10;
    let is_getter = bits & 0b1 == 0b1;
    assert!(vm.stack.len() >= 2);
    let object = if is_static {
        vm.stack[vm.stack.len() - 1]
    } else {
        vm.stack[vm.stack.len() - 2]
    };
    let object = Object::try_from(object).unwrap().bind(gc.nogc());
    // 2. Let env be the running execution context's LexicalEnvironment.
    let env = agent.current_lexical_environment(gc.nogc());
    // 3. Let privateEnv be the running execution context's PrivateEnvironment.
    let private_env = agent.current_private_environment(gc.nogc()).unwrap();
    // 1. Let propKey be ? Evaluation of ClassElementName.
    // 5. Let formalParameterList be ...
    let params = OrdinaryFunctionCreateParams {
        function_prototype: None,
        source_code: None,
        // 4. Let sourceText be the source text matched by MethodDefinition.
        template,
        lexical_this: false,
        env,
        private_env: Some(private_env),
    };
    // 6. Let closure be OrdinaryFunctionCreate(
    //      %Function.prototype%,
    //      sourceText,
    //      formalParameterList,
    //      FunctionBody,
    //      non-lexical-this,
    //      env,
    //      privateEnv
    //  ).
    let is_generator = params.template.kind.is_generator();
    let is_async = params.template.kind.is_async();
    let closure = ordinary_function_create(agent, params, gc.nogc());
    if is_generator {
        define_generator_prototype(agent, closure, is_async, gc.nogc());
    }
    // 7. Perform MakeMethod(closure, object).
    make_method(agent, closure, object);
    // 8. Perform SetFunctionName(closure, propKey).
    let function_name = format!("#{}", description.to_string_lossy_(agent));
    let function_name = String::from_string(agent, function_name, gc.nogc());
    set_function_name(
        agent,
        closure,
        // Note: it should be guaranteed that description is a
        // non-numeric PropertyKey.
        function_name.into(),
        if is_getter {
            Some(SetFunctionNamePrefix::Get)
        } else if is_setter {
            Some(SetFunctionNamePrefix::Set)
        } else {
            None
        },
        gc.nogc(),
    );
    if is_static {
        let desc = PropertyDescriptor {
            value: if !is_getter && !is_setter {
                Some(closure.unbind().into())
            } else {
                None
            },
            writable: if !is_getter && !is_setter {
                Some(false)
            } else {
                None
            },
            get: if is_getter {
                Some(Some(closure.unbind().into()))
            } else {
                None
            },
            set: if is_setter {
                Some(Some(closure.unbind().into()))
            } else {
                None
            },
            enumerable: Some(false),
            configurable: Some(true),
        };
        // b. Perform ? DefinePropertyOrThrow(object, propKey, desc).
        let private_name = private_env.add_static_private_method(agent, description);
        let object = object.unbind();
        with_vm_gc(
            agent,
            vm,
            |agent, gc| define_property_or_throw(agent, object, private_name.into(), desc, gc),
            gc,
        )?;
    } else {
        // a. Return PrivateElement {
        //      [[Key]]: propKey,
        //      [[Kind]]: ...,
        //      [[Get]]: ...,
        //      [[Set]]: ...
        //    }.
        let private_method = if is_getter {
            PrivateMethod::Getter(closure)
        } else if is_setter {
            PrivateMethod::Setter(closure)
        } else {
            PrivateMethod::Method(closure)
        };
        private_env.add_instance_private_method(agent, description, private_method);
    }
    // c. Return unused.
    Ok(())
}

pub(super) fn execute_class_define_private_property<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let description = executable.fetch_identifier(agent, instr.get_first_index(), gc);
    let is_static = instr.get_second_bool();
    let private_env = agent
        .current_private_environment(gc)
        .expect("Attempted to define private property with no PrivateEnvironment");
    if is_static {
        let private_name = private_env.add_static_private_field(agent, description);
        let object = vm.stack.last().unwrap().bind(gc);
        let object = Object::try_from(object).unwrap();
        if let Err(err) = object
            .get_or_create_backing_object(agent)
            .bind(gc)
            .property_storage()
            .add_private_field_slot(agent, private_name)
        {
            return Err(agent.throw_allocation_exception(err, gc));
        };
    } else {
        private_env.add_instance_private_field(agent, description);
    }
    Ok(())
}

pub(super) fn execute_class_initialize_private_elements<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let target = Object::try_from(vm.stack.last().unwrap().bind(gc)).unwrap();
    if target.is_proxy() {
        return Err(throw_no_proxy_private_names(agent, gc));
    }
    target
        .get_or_create_backing_object(agent)
        .property_storage()
        .initialize_private_elements(agent, gc)?;
    Ok(())
}

pub(super) fn execute_class_initialize_private_value<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let target = Object::try_from(vm.stack.last().unwrap().bind(gc)).unwrap();
    let value = vm.result.take().unwrap().bind(gc);
    if target.is_proxy() {
        return Err(throw_no_proxy_private_names(agent, gc));
    }
    let offset = instr.get_first_index();
    let private_env = agent
        .current_private_environment(gc)
        .expect("Attempted to define private property with no PrivateEnvironment");
    let private_name = private_env.get_private_name(agent, offset);
    assert!(
        target
            .get_or_create_backing_object(agent)
            .property_storage()
            .set_private_field_value(agent, private_name, offset, value)
    );
    Ok(())
}

pub(super) fn execute_direct_eval_call<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let func = with_vm_gc(
        agent,
        vm,
        |agent, mut gc| {
            let func_ref =
                resolve_binding(agent, BUILTIN_STRING_MEMORY.eval, None, None, gc.reborrow())
                    .unbind()?
                    .bind(gc.nogc());
            get_value(agent, &func_ref.unbind(), gc)
        },
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    let args = vm.get_call_args(agent, instr, gc.nogc());

    // a. If SameValue(func, %eval%) is true, then
    let result = if func == agent.current_realm_record().intrinsics().eval().into() {
        // i. Let argList be ? ArgumentListEvaluation of arguments.
        // ii. If argList has no elements, return undefined.
        if args.is_empty() {
            Value::Undefined
        } else {
            // iii. Let evalArg be the first element of argList.
            let eval_arg = args[0];
            // iv. If IsStrict(this CallExpression) is true, let
            //     strictCaller be true. Otherwise let strictCaller
            //     be false.
            let strict_caller = agent.is_evaluating_strict_code();
            // v. Return ? PerformEval(evalArg, strictCaller, true).
            let eval_arg = eval_arg.unbind();
            with_vm_gc(
                agent,
                vm,
                |agent, gc| perform_eval(agent, eval_arg, true, strict_caller, gc),
                gc,
            )?
        }
    } else {
        let func = func.unbind();
        let mut args = args.unbind();
        with_vm_gc(
            agent,
            vm,
            |agent, gc| {
                call(
                    agent,
                    func,
                    Value::Undefined,
                    Some(ArgumentsList::from_mut_slice(args.as_mut_slice())),
                    gc,
                )
            },
            gc,
        )?
    };
    vm.result = Some(result.unbind());
    Ok(())
}

pub(super) fn execute_evaluate_call<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let reference = vm.reference.take();
    // 1. If ref is a Reference Record, then
    let this_value = if let Some(reference) = reference {
        // a. If IsPropertyReference(ref) is true, then
        if is_property_reference(&reference) {
            // i. Let thisValue be GetThisValue(ref).
            get_this_value(&reference).bind(gc.nogc())
        } else {
            // b. Else,
            // i. Let refEnv be ref.[[Base]].
            // ii. Assert: refEnv is an Environment Record.
            let ref_env = reference.base_env();
            // iii. Let thisValue be refEnv.WithBaseObject().
            ref_env
                .with_base_object(agent)
                .map_or(Value::Undefined, |object| object.into())
                .bind(gc.nogc())
        }
    } else {
        // 2. Else,
        // a. Let thisValue be undefined.
        Value::Undefined
    };
    let mut args = vm.get_call_args(agent, instr, gc.nogc()).unbind();
    let func = vm.stack.pop().unwrap().unbind();
    let this_value = this_value.unbind();
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| {
            call(
                agent,
                func,
                this_value,
                Some(ArgumentsList::from_mut_slice(args.as_mut_slice())),
                gc,
            )
        },
        gc,
    );
    return_pooled_arguments_vec(agent, args);
    vm.result = Some(result?.unbind());
    Ok(())
}

pub(super) fn execute_evaluate_new<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let args = vm.get_call_args(agent, instr, gc.nogc());
    let constructor = vm.stack.pop().unwrap().bind(gc.nogc());
    let Some(constructor) = is_constructor(agent, constructor) else {
        let constructor_string = {
            let constructor = constructor.unbind();
            with_vm_gc(
                agent,
                vm,
                |agent, gc| constructor.string_repr(agent, gc),
                gc.reborrow(),
            )
        };
        let error_message = format!(
            "'{}' is not a constructor.",
            constructor_string.to_string_lossy_(agent)
        );
        return Err(agent.throw_exception(ExceptionType::TypeError, error_message, gc.into_nogc()));
    };

    let constructor = constructor.unbind();
    let mut args = args.unbind();
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| {
            construct(
                agent,
                constructor,
                Some(ArgumentsList::from_mut_slice(args.as_mut_slice())),
                None,
                gc,
            )
        },
        gc,
    )?;
    vm.result = Some(result.unbind().into());
    Ok(())
}

/// The error for code that finds no Function Environment Record where the
/// grammar promises one (new.target or super outside a function or method).
/// Early errors make this unreachable; a script must never turn it into a
/// panic.
#[cold]
#[inline(never)]
fn throw_missing_function_environment<'gc>(
    agent: &mut Agent,
    gc: NoGcScope<'gc, '_>,
) -> crate::ecmascript::JsError<'gc> {
    agent.throw_exception_with_static_message(
        ExceptionType::SyntaxError,
        "new.target or super is not available here",
        gc,
    )
}

pub(super) fn execute_evaluate_super<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let Environment::Function(this_env) = get_this_environment(agent, gc.nogc()) else {
        return Err(throw_missing_function_environment(agent, gc.into_nogc()));
    };
    // 1. Let newTarget be GetNewTarget().
    // 2. Assert: newTarget is an Object.
    let Some(Ok(new_target)) = this_env.get_new_target(agent).map(Function::try_from) else {
        return Err(throw_missing_function_environment(agent, gc.into_nogc()));
    };
    // 3. Let func be GetSuperConstructor().
    let (new_target, func) = {
        let function_object = this_env.get_function_object(agent);
        (
            new_target.bind(gc.nogc()),
            unwrap_try(function_object.try_get_prototype_of(agent, gc.nogc())),
        )
    };
    // 4. Let argList be ? ArgumentListEvaluation of Arguments.
    let arg_list = vm.get_call_args(agent, instr, gc.nogc());
    // 5. If IsConstructor(func) is false, throw a TypeError exception.
    let Some(func) = func.and_then(|func| is_constructor(agent, func)) else {
        let constructor = func.map_or(Value::Null, |f| f.unbind().into());
        let error_message = with_vm_gc(
            agent,
            vm,
            |agent, gc| {
                format!(
                    "'{}' is not a constructor.",
                    constructor.string_repr(agent, gc).to_string_lossy_(agent)
                )
            },
            gc.reborrow(),
        );
        return Err(agent.throw_exception(ExceptionType::TypeError, error_message, gc.into_nogc()));
    };
    // 6. Let result be ? Construct(func, argList, newTarget).
    let result = {
        let func = func.unbind();
        let mut arg_list = arg_list.unbind();
        let new_target = new_target.unbind();
        let result = with_vm_gc(
            agent,
            vm,
            |agent, gc| {
                construct(
                    agent,
                    func,
                    Some(ArgumentsList::from_mut_slice(arg_list.as_mut_slice())),
                    Some(new_target),
                    gc,
                )
            },
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        result.unbind().bind(gc.nogc())
    };
    let result = result.unbind();
    finish_super_call(agent, vm, result, gc)
}

/// What SuperCall does once the parent constructor has returned `result`:
/// the object is the this value of the constructor that called `super`,
/// whose instance elements are then initialized, and is the result of the
/// call. For a `super(…)` whose parent constructor a `Vm` ran itself as
/// for one that \[\[Construct]] ran.
pub(super) fn finish_super_call<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    result: Object,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let result = result.bind(gc.nogc());
    // 7. Let thisER be GetThisEnvironment().
    let Environment::Function(this_er) = get_this_environment(agent, gc.nogc()) else {
        return Err(throw_missing_function_environment(agent, gc.into_nogc()));
    };
    // 8. Perform ? thisER.BindThisValue(result).
    this_er
        .bind_this_value(agent, result.into(), gc.nogc())
        .unbind()?
        .bind(gc.nogc());
    // 9. Let F be thisER.[[FunctionObject]].
    // 10. Assert: F is an ECMAScript function object.
    let Function::ECMAScriptFunction(f) = this_er.get_function_object(agent) else {
        unreachable!();
    };
    // 11. Perform ? InitializeInstanceElements(result, F).
    let result = if f
        .get_executable(agent)
        .get(agent)
        .instance_initializer
        .is_some()
    {
        let scoped_result = result.scope(agent, gc.nogc());
        let f = f.unbind();
        let o = result.unbind();
        with_vm_gc(
            agent,
            vm,
            |agent, gc| initialize_derived_instance_elements(agent, o, f, gc),
            gc.reborrow(),
        )
        .unbind()?;
        scoped_result.get(agent).bind(gc.nogc())
    } else {
        result
    };
    // 12. Return result.
    vm.result = Some(result.unbind().into());
    Ok(())
}

#[inline(always)]
pub(super) fn execute_evaluate_property_access_with_expression_key(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope,
) {
    let property_name_value = vm.result.take().unwrap().bind(gc);
    let base_value = vm.stack.pop().unwrap().bind(gc);
    let strict = agent.is_evaluating_strict_code();

    vm.reference = Some(
        Reference::new_property_expression_reference(base_value, property_name_value, strict)
            .unbind(),
    );
}

#[inline(always)]
pub(super) fn execute_evaluate_property_access_with_identifier_key(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let property_key = executable.fetch_property_key(agent, instr.get_first_index(), gc);
    let base_value = vm.result.take().unwrap().bind(gc);
    let strict = agent.is_evaluating_strict_code();

    vm.reference =
        Some(Reference::new_property_reference(base_value, property_key, strict).unbind());
}

pub(super) fn execute_make_private_reference(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let private_identifier = executable.fetch_identifier(agent, instr.get_first_index(), gc);
    let base_value = vm.result.take().unwrap().bind(gc);
    // 1. Let privateEnv be the running execution context's
    //    PrivateEnvironment.
    // 2. Assert: privateEnv is not null.
    let private_env = agent
        .current_private_environment(gc)
        .expect("Attempted to make private reference in non-class environment");
    // 3. Let privateName be ResolvePrivateIdentifier(privateEnv, privateIdentifier).
    let private_name = resolve_private_identifier(agent, private_env, private_identifier);
    // 4. Return the Reference Record {
    //    [[Base]]: baseValue,
    //    [[ReferencedName]]: privateName,
    //    [[Strict]]: true,
    //    [[ThisValue]]: empty
    // }.
    vm.reference
        .replace(Reference::new_private_reference(base_value, private_name).unbind());
}

pub(super) fn execute_make_super_property_reference_with_expression_key<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // ### SuperProperty : super [ Expression ]

    // 1. Let env be GetThisEnvironment().
    let env = get_this_environment(agent, gc);
    // 2. Let actualThis be ? env.GetThisBinding().
    let actual_this = env.get_this_binding(agent, gc).unbind()?.bind(gc);
    // 3. Let propertyNameReference be ? Evaluation of Expression.
    // 4. Let propertyNameValue be ? GetValue(propertyNameReference).
    let property_name_value = vm.result.take().unwrap().bind(gc);
    // 5. Let strict be IsStrict(this SuperProperty).
    let strict = agent.is_evaluating_strict_code();
    // 6. NOTE: In most cases, ToPropertyKey will be performed on
    //    propertyNameValue immediately after this step. However,
    //    in the case of super[b] = c, it will not be performed
    //    until after evaluation of c.
    // 7. Return MakeSuperPropertyReference(
    //        actualThis,
    //        propertyNameValue,
    //        strict
    //    ).
    // 1. Let env be GetThisEnvironment().
    // 2. Assert: env.HasSuperBinding() is true.
    // 3. Assert: env is a Function Environment Record.
    let Environment::Function(env) = env else {
        return Err(throw_missing_function_environment(agent, gc));
    };
    if !Environment::Function(env).has_super_binding(agent) {
        return Err(throw_missing_function_environment(agent, gc));
    }
    // 4. Let baseValue be GetSuperBase(env).
    let base_value = env.get_super_base(agent, gc);

    // 5. Return the Reference Record {
    // [[Base]]: baseValue,
    vm.reference = Some(
        Reference::new_super_expression_reference(
            base_value,
            // [[ReferencedName]]: propertyKey,
            property_name_value,
            // [[ThisValue]]: actualThis
            actual_this,
            // [[Strict]]: strict,
            strict,
        )
        .unbind(),
    );
    // }.
    Ok(())
}

pub(super) fn execute_make_super_property_reference_with_identifier_key<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // ### SuperProperty : super . IdentifierName

    // 1. Let env be GetThisEnvironment().
    let env = get_this_environment(agent, gc);
    // 2. Let actualThis be ? env.GetThisBinding().
    let actual_this = env.get_this_binding(agent, gc).unbind()?.bind(gc);
    // 3. Let propertyKey be the StringValue of IdentifierName.
    let property_key = executable.fetch_property_key(agent, instr.get_first_index(), gc);
    // 4. Let strict be IsStrict(this SuperProperty).
    let strict = agent.is_evaluating_strict_code();
    // 5. Return MakeSuperPropertyReference(actualThis, propertyKey, strict).
    // 1. Let env be GetThisEnvironment().
    // 2. Assert: env.HasSuperBinding() is true.
    // 3. Assert: env is a Function Environment Record.
    let Environment::Function(env) = env else {
        return Err(throw_missing_function_environment(agent, gc));
    };
    if !Environment::Function(env).has_super_binding(agent) {
        return Err(throw_missing_function_environment(agent, gc));
    }
    // 4. Let baseValue be GetSuperBase(env).
    let base_value = env.get_super_base(agent, gc);
    // 4. Let baseValue be GetSuperBase(env).
    // 5. Return the Reference Record {
    vm.reference = Some(
        Reference::new_super_reference(
            // [[Base]]: baseValue,
            base_value,
            // [[ReferencedName]]: propertyKey,
            property_key,
            // [[ThisValue]]: actualThis
            actual_this,
            // [[Strict]]: strict,
            strict,
        )
        .unbind(),
    );
    // }.
    Ok(())
}

/// A comparison with the jump on it, or `++` or `--` of a stack slot, as
/// the instructions it was fused from: the comparison or the conversion can
/// run a script.
///
/// One function of its own, like the fallbacks for global names: the
/// instruction loop's frame holds none of this.
#[inline(never)]
pub(super) fn execute_fused_instruction<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    match instr.kind {
        Instruction::GetValueWithExpressionKey => {
            // EvaluatePropertyAccessWithExpressionKey, GetValue
            execute_evaluate_property_access_with_expression_key(agent, vm, gc.nogc());
            execute_get_value(agent, vm, executable, instr, false, false, gc)
        }
        Instruction::PutValueWithExpressionKey => {
            // The key goes where EvaluatePropertyAccessWithExpressionKey
            // takes it from, the value back where PutValue does.
            let value = vm.result.take().unwrap();
            vm.result = vm.stack.pop();
            execute_evaluate_property_access_with_expression_key(agent, vm, gc.nogc());
            vm.result = Some(value);
            execute_put_value(agent, vm, executable, instr, false, gc)
        }
        Instruction::GetValueWithIndexKey => {
            // Load, GetValueFromIndex
            let key = vm.stack[instr.get_first_index()];
            vm.execute_load();
            vm.result = Some(key);
            // EvaluatePropertyAccessWithExpressionKey, GetValue
            execute_evaluate_property_access_with_expression_key(agent, vm, gc.nogc());
            execute_get_value(agent, vm, executable, instr, false, false, gc)
        }
        Instruction::LoadBindingAtSite | Instruction::LoadBindingAtSiteOnce => {
            // GetBindingAtSite, Load
            execute_get_binding_at_site(agent, vm, executable, instr, false, gc)?;
            vm.execute_load();
            Ok(())
        }
        Instruction::ApplyBinaryOperatorWithConstant
        | Instruction::ApplyBinaryOperatorToIndexWithConstant
        | Instruction::ApplyBinaryOperatorAndMove => {
            let operation = if instr.kind == Instruction::ApplyBinaryOperatorWithConstant {
                instr.get_second_index()
            } else {
                instr.get_first_index()
            };
            let (slot, operator) = Instruction::fused_slot_and_operator(operation);
            if instr.kind != Instruction::ApplyBinaryOperatorAndMove {
                let (left, constant) = if instr.kind == Instruction::ApplyBinaryOperatorWithConstant
                {
                    // Load
                    (vm.result.take().unwrap(), instr.get_first_index())
                } else {
                    // LoadFromIndex
                    (vm.stack[slot], instr.get_second_index())
                };
                vm.stack.push(left);
                // StoreConstant
                vm.result = Some(
                    executable
                        .fetch_constant(agent, constant, gc.nogc())
                        .unbind(),
                );
            }
            // The operator
            match Instruction::fused_operator(operator) {
                Some(Instruction::ApplyAdditionBinaryOperator) => {
                    execute_apply_addition_binary_operator(agent, vm, gc)?
                }
                Some(kind) => execute_apply_binary_operator(agent, vm, kind, gc)?,
                None => unreachable!("a fused instruction without its operator"),
            }
            if Instruction::fused_pushes(operation) {
                // Load
                vm.execute_load();
            } else if instr.kind == Instruction::ApplyBinaryOperatorAndMove {
                // PutValueToIndex
                vm.stack[slot] = vm.result.take().unwrap();
            }
            Ok(())
        }
        Instruction::LoadPropertyWithCachePushReference => {
            // GetPropertyWithCacheKeepReference:
            // EvaluatePropertyAccessWithIdentifierKey,
            // GetValueWithCacheKeepReference
            execute_evaluate_property_access_with_identifier_key(
                agent,
                vm,
                executable.clone(),
                instr,
                gc.nogc(),
            );
            let get_value = Instr::new_with_arg(
                Instruction::GetValueWithCacheKeepReference,
                instr.get_second_index() as u16,
            );
            execute_get_value(agent, vm, executable, get_value, true, true, gc)?;
            // PushReference, Load
            vm.execute_push_reference(agent);
            vm.execute_load();
            Ok(())
        }
        Instruction::IncrementIndex | Instruction::DecrementIndex => {
            let slot = instr.get_first_index();
            // GetValueFromIndex, ToNumeric
            vm.result = Some(vm.stack[slot]);
            execute_to_numeric(agent, vm, gc.reborrow()).unbind()?;
            // Increment or Decrement
            if instr.kind == Instruction::IncrementIndex {
                execute_increment(agent, vm, gc.into_nogc());
            } else {
                execute_decrement(agent, vm, gc.into_nogc());
            }
            // PutValueToIndex
            vm.stack[slot] = vm.result.take().unwrap();
            Ok(())
        }
        kind if kind.loop_test().is_some() => {
            let Some((step, comparison, constant)) = kind.loop_test() else {
                unreachable!()
            };
            let target = Instr::wide_jump_target(&executable.get_instructions(agent), vm.ip);
            let slot = instr.get_first_index();
            if step != 0 {
                // IncrementIndex or DecrementIndex: GetValueFromIndex,
                // ToNumeric, Increment or Decrement, PutValueToIndex
                vm.result = Some(vm.stack[slot]);
                execute_to_numeric(agent, vm, gc.reborrow()).unbind()?;
                if step > 0 {
                    execute_increment(agent, vm, gc.nogc());
                } else {
                    execute_decrement(agent, vm, gc.nogc());
                }
                vm.stack[slot] = vm.result.take().unwrap();
            }
            // LoadFromIndexStoreConstant or LoadFromIndexGetFromIndex
            let left = vm.stack[slot];
            vm.stack.push(left);
            vm.result = Some(if constant {
                executable
                    .fetch_constant(agent, instr.get_second_index(), gc.nogc())
                    .unbind()
            } else {
                vm.stack[instr.get_second_index()]
            });
            // The comparison
            match comparison {
                Instruction::LessThan => execute_less_than(agent, vm, gc),
                Instruction::LessThanEquals => execute_less_than_equals(agent, vm, gc),
                Instruction::GreaterThan => execute_greater_than(agent, vm, gc),
                _ => execute_greater_than_equals(agent, vm, gc),
            }?;
            // JumpIfTrue
            let result = vm.result.take().unwrap();
            let truthy = match result {
                Value::Boolean(value) => value,
                _ => to_boolean(agent, result),
            };
            if truthy {
                vm.ip = target;
            }
            Ok(())
        }
        kind if kind.compared_local().is_some() => {
            let Some((comparison, constant)) = kind.compared_local() else {
                unreachable!()
            };
            let target = Instr::wide_jump_target(&executable.get_instructions(agent), vm.ip);
            // LoadFromIndexStoreConstant or LoadFromIndexGetFromIndex
            let left = vm.stack[instr.get_first_index()];
            vm.stack.push(left);
            vm.result = Some(if constant {
                executable
                    .fetch_constant(agent, instr.get_second_index(), gc.nogc())
                    .unbind()
            } else {
                vm.stack[instr.get_second_index()]
            });
            // The comparison
            match comparison {
                Instruction::JumpIfNotLessThan => execute_less_than(agent, vm, gc),
                Instruction::JumpIfNotLessThanEquals => execute_less_than_equals(agent, vm, gc),
                Instruction::JumpIfNotGreaterThan => execute_greater_than(agent, vm, gc),
                Instruction::JumpIfNotGreaterThanEquals => {
                    execute_greater_than_equals(agent, vm, gc)
                }
                _ => {
                    execute_is_strictly_equal(agent, vm, gc.into_nogc());
                    Ok(())
                }
            }?;
            // JumpIfNot, or JumpIfTrue
            let result = vm.result.take().unwrap();
            let truthy = match result {
                Value::Boolean(value) => value,
                _ => to_boolean(agent, result),
            };
            if truthy == (comparison == Instruction::JumpIfStrictlyEqual) {
                vm.ip = target;
            }
            Ok(())
        }
        Instruction::JumpIfNotStrictlyEqual
        | Instruction::JumpIfStrictlyEqual
        | Instruction::JumpIfNotLooselyEqual
        | Instruction::JumpIfLooselyEqual => {
            // IsStrictlyEqual or IsLooselyEqual
            if matches!(
                instr.kind,
                Instruction::JumpIfNotStrictlyEqual | Instruction::JumpIfStrictlyEqual
            ) {
                execute_is_strictly_equal(agent, vm, gc.nogc());
            } else {
                execute_is_loosely_equal(agent, vm, gc)?;
            }
            // JumpIfNot or JumpIfTrue
            let mut ip = vm.ip;
            if matches!(
                instr.kind,
                Instruction::JumpIfStrictlyEqual | Instruction::JumpIfLooselyEqual
            ) {
                execute_jump_if_true(agent, vm, &mut ip, instr);
            } else {
                execute_jump_if_not(agent, vm, &mut ip, instr);
            }
            vm.ip = ip;
            Ok(())
        }
        _ => {
            match instr.kind {
                Instruction::JumpIfNotLessThan | Instruction::JumpIfLessThan => {
                    execute_less_than(agent, vm, gc)
                }
                Instruction::JumpIfNotLessThanEquals | Instruction::JumpIfLessThanEquals => {
                    execute_less_than_equals(agent, vm, gc)
                }
                Instruction::JumpIfNotGreaterThan | Instruction::JumpIfGreaterThan => {
                    execute_greater_than(agent, vm, gc)
                }
                Instruction::JumpIfNotGreaterThanEquals | Instruction::JumpIfGreaterThanEquals => {
                    execute_greater_than_equals(agent, vm, gc)
                }
                _ => unreachable!(),
            }?;
            // JumpIfNot, or JumpIfTrue
            let mut ip = vm.ip;
            if matches!(
                instr.kind,
                Instruction::JumpIfLessThan
                    | Instruction::JumpIfLessThanEquals
                    | Instruction::JumpIfGreaterThan
                    | Instruction::JumpIfGreaterThanEquals
            ) {
                execute_jump_if_true(agent, vm, &mut ip, instr);
            } else {
                execute_jump_if_not(agent, vm, &mut ip, instr);
            }
            vm.ip = ip;
            Ok(())
        }
    }
}

/// What the instructions that read `this.name` or assign `object.name` in
/// one step stand for, where the plain way does not take them.
#[inline(never)]
pub(super) fn execute_fused_property_instruction<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    if instr.kind == Instruction::PutPropertyWithCache {
        if let Some(&Value::Object(object)) = vm.stack.last() {
            // The cache does not know the object's shape yet. What a read
            // of the property finds is what the assignment goes by (the
            // object's own property, one it inherits, or none anywhere),
            // and the lookup of a read leaves that in the cache while
            // calling nothing: the next assignment to an object of this
            // shape is made by it, also the one that adds the property.
            let cache = executable.fetch_cache(agent, instr.get_second_index(), gc.nogc());
            let shape = object.object_shape(agent);
            if cache.find_cached_property_offset(agent, shape).is_none() {
                let key = executable.fetch_property_key(agent, instr.get_first_index(), gc.nogc());
                let _ = object.try_get(agent, key, object.into(), Some(cache), gc.nogc());
            }
        }
        // EvaluatePropertyAccessWithIdentifierKey of the object, with the
        // value kept aside, then PutValueWithCache.
        let value = vm.result.take().unwrap();
        vm.result = vm.stack.pop();
        execute_evaluate_property_access_with_identifier_key(
            agent,
            vm,
            executable.clone(),
            instr,
            gc.nogc(),
        );
        vm.result = Some(value);
        let put_value = Instr::new_with_arg(
            Instruction::PutValueWithCache,
            instr.get_second_index() as u16,
        );
        return execute_put_value(agent, vm, executable, put_value, true, gc);
    }
    let keep_reference = matches!(
        instr.kind,
        Instruction::GetThisPropertyWithCacheKeepReference
            | Instruction::GetIndexPropertyWithCacheKeepReference
    );
    if matches!(
        instr.kind,
        Instruction::GetIndexPropertyWithCache
            | Instruction::GetIndexPropertyWithCacheKeepReference
    ) {
        // GetValueFromIndex: the plain way found the property not to be
        // one the cache places.
        let slot = Instr::wide_third_index(&executable.get_instructions(agent), vm.ip);
        vm.result = Some(vm.stack[slot]);
    } else {
        // ResolveThisBinding, where the call does not keep the this value
        // itself; the property is still read by the cache where it can be.
        execute_resolve_this_binding(agent, vm, gc.nogc()).unbind()?;
        if Vm::get_property_by_cache(agent, vm, &executable, instr, keep_reference, gc.nogc()) {
            return Ok(());
        }
    }
    // EvaluatePropertyAccessWithIdentifierKey, GetValueWithCache
    execute_evaluate_property_access_with_identifier_key(
        agent,
        vm,
        executable.clone(),
        instr,
        gc.nogc(),
    );
    let get_value = Instr::new_with_arg(
        if keep_reference {
            Instruction::GetValueWithCacheKeepReference
        } else {
            Instruction::GetValueWithCache
        },
        instr.get_second_index() as u16,
    );
    execute_get_value(agent, vm, executable, get_value, true, keep_reference, gc)
}

#[inline(always)]
pub(super) fn execute_jump(agent: &Agent, ip: &mut usize, instr: Instr) {
    let target = instr.get_jump_slot();
    if agent.options.print_internals {
        eprintln!("Jumping to {target}");
    }
    *ip = target;
}

#[inline(always)]
pub(super) fn execute_jump_if_not(agent: &Agent, vm: &mut Vm, ip: &mut usize, instr: Instr) {
    let result = vm.result.take().unwrap();
    let target = instr.get_jump_slot();
    // A comparison left a Boolean: no conversion to make.
    let truthy = match result {
        Value::Boolean(value) => value,
        _ => to_boolean(agent, result),
    };
    if !truthy {
        if agent.options.print_internals {
            eprintln!("Comparison failed, jumping to {target}");
        }
        *ip = target;
    }
}

#[inline(always)]
pub(super) fn execute_jump_if_true(agent: &Agent, vm: &mut Vm, ip: &mut usize, instr: Instr) {
    let result = vm.result.take().unwrap();
    let truthy = match result {
        Value::Boolean(value) => value,
        _ => to_boolean(agent, result),
    };
    if truthy {
        let target = instr.get_jump_slot();
        if agent.options.print_internals {
            eprintln!("Comparison succeeded, jumping to {target}");
        }
        *ip = target;
    }
}

pub(super) fn execute_increment(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) {
    let lhs = vm.result.take().unwrap().bind(gc);
    // Note: This is done by the previous instruction.
    let old_value = Numeric::try_from(lhs).unwrap();
    let new_value: Value = if let Ok(old_value) = Number::try_from(old_value) {
        Number::add(agent, old_value, 1.into()).into()
    } else {
        let old_value = BigInt::try_from(old_value).unwrap();
        BigInt::add(agent, old_value, 1.into()).into()
    };
    vm.result = Some(new_value.unbind());
}

pub(super) fn execute_decrement(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) {
    let lhs = vm.result.take().unwrap().bind(gc);
    // Note: This is done by the previous instruction.
    let old_value = Numeric::try_from(lhs).unwrap();
    let new_value: Value = if let Ok(old_value) = Number::try_from(old_value) {
        Number::subtract(agent, old_value, 1.into()).into()
    } else {
        let old_value = BigInt::try_from(old_value).unwrap();
        BigInt::subtract(agent, old_value, 1.into()).into()
    };
    vm.result = Some(new_value.unbind());
}

pub(super) fn execute_less_than<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap();
    let rval = vm.result.take().unwrap();
    // Two Numbers compare without conversions, so nothing can run or collect.
    if let (Ok(lnum), Ok(rnum)) = (Number::try_from(lval), Number::try_from(rval)) {
        vm.result = Some((Number::less_than(agent, lnum, rnum) == Some(true)).into());
        return Ok(());
    }
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| is_less_than::<true>(agent, lval, rval, gc),
        gc,
    )?;
    let result = result == Some(true);
    vm.result = Some(result.into());
    Ok(())
}

pub(super) fn execute_less_than_equals<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap();
    let rval = vm.result.take().unwrap();
    // Two Numbers compare without conversions, so nothing can run or collect.
    if let (Ok(lnum), Ok(rnum)) = (Number::try_from(lval), Number::try_from(rval)) {
        vm.result = Some((Number::less_than(agent, rnum, lnum) == Some(false)).into());
        return Ok(());
    }
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| is_less_than::<false>(agent, rval, lval, gc),
        gc,
    )?;
    let result = result == Some(false);
    vm.result = Some(result.into());
    Ok(())
}

pub(super) fn execute_greater_than<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap();
    let rval = vm.result.take().unwrap();
    // Two Numbers compare without conversions, so nothing can run or collect.
    if let (Ok(lnum), Ok(rnum)) = (Number::try_from(lval), Number::try_from(rval)) {
        vm.result = Some((Number::less_than(agent, rnum, lnum) == Some(true)).into());
        return Ok(());
    }
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| is_less_than::<false>(agent, rval, lval, gc),
        gc,
    )?;
    let result = result == Some(true);
    vm.result = Some(result.into());
    Ok(())
}

pub(super) fn execute_greater_than_equals<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap();
    let rval = vm.result.take().unwrap();
    // Two Numbers compare without conversions, so nothing can run or collect.
    if let (Ok(lnum), Ok(rnum)) = (Number::try_from(lval), Number::try_from(rval)) {
        vm.result = Some((Number::less_than(agent, lnum, rnum) == Some(false)).into());
        return Ok(());
    }
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| is_less_than::<true>(agent, lval, rval, gc),
        gc,
    )?;
    let result = result == Some(false);
    vm.result = Some(result.into());
    Ok(())
}

pub(super) fn execute_has_property<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap().bind(gc.nogc());
    let rval = vm.result.take().unwrap().bind(gc.nogc());
    // RelationalExpression : RelationalExpression in ShiftExpression
    // 5. If rval is not an Object, throw a TypeError exception.
    let Ok(rval) = Object::try_from(rval) else {
        return Err(throw_error_in_target_not_object(
            agent,
            rval.unbind(),
            gc.into_nogc(),
        ));
    };
    // 6. Return ? HasProperty(rval, ? ToPropertyKey(lval)).
    let property_key = if let Ok(lval) = Primitive::try_from(lval) {
        to_property_key_primitive(agent, lval, gc.nogc())
    } else {
        let scoped_rval = rval.scope(agent, gc.nogc());
        let lval = lval.unbind();
        let result = with_vm_gc(
            agent,
            vm,
            |agent, mut gc| {
                let property_key = to_property_key(agent, lval, gc.reborrow())
                    .unbind()?
                    .bind(gc.nogc());
                has_property(agent, scoped_rval.take(agent), property_key.unbind(), gc)
            },
            gc.reborrow(),
        )
        .unbind()?;
        vm.result = Some(result.into());
        return Ok(());
    };
    let result = match try_has_property(agent, rval, property_key, None, gc.nogc()) {
        ControlFlow::Continue(c) => match c {
            TryHasResult::Unset => false,
            TryHasResult::Offset(_, _) | TryHasResult::Custom(_, _) => true,
            TryHasResult::Proxy(proxy) => {
                let proxy = proxy.unbind();
                let property_key = property_key.unbind();
                with_vm_gc(
                    agent,
                    vm,
                    |agent, gc| proxy.internal_has_property(agent, property_key, gc),
                    gc,
                )?
            }
        },
        ControlFlow::Break(TryError::Err(err)) => {
            return Err(err.unbind().bind(gc.into_nogc()));
        }
        ControlFlow::Break(TryError::GcError) => {
            let rval = rval.unbind();
            let property_key = property_key.unbind();
            with_vm_gc(
                agent,
                vm,
                |agent, gc| has_property(agent, rval.unbind(), property_key.unbind(), gc),
                gc,
            )?
        }
    };
    vm.result = Some(result.into());
    Ok(())
}

pub(super) fn execute_has_private_element<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let Some(reference) = vm.reference.take() else {
        unreachable!()
    };
    let (r_val, private_name) = reference.into_private_reference_data();
    // 4. If rVal is not an Object,
    if let Ok(r_val) = Object::try_from(r_val) {
        // 8. If PrivateElementFind(rVal, privateName) is not
        //    empty, return true.
        // 9. Return false.
        let result = private_element_find(agent, r_val, private_name).is_some();
        vm.result = Some(result.into());
        Ok(())
    } else {
        // throw a TypeError exception.
        Err(throw_error_in_target_not_object(agent, r_val, gc))
    }
}

#[inline(always)]
pub(super) fn execute_is_strictly_equal(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) {
    let lval = vm.stack.pop().unwrap().bind(gc);
    let rval = vm.result.take().unwrap().bind(gc);
    let result = is_strictly_equal(agent, lval, rval);
    vm.result = Some(result.into());
}

pub(super) fn execute_is_loosely_equal<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap();
    let rval = vm.result.take().unwrap();
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| is_loosely_equal(agent, lval, rval, gc),
        gc,
    )?;
    vm.result = Some(result.into());
    Ok(())
}

#[inline(always)]
pub(super) fn execute_is_constructor(agent: &Agent, vm: &mut Vm) {
    let val = vm.result.take().unwrap();
    let result = if let Ok(val) = Function::try_from(val) {
        val.is_constructor(agent)
    } else {
        false
    };
    vm.result = Some(result.into());
}

#[inline(always)]
pub(super) fn execute_logical_not(agent: &Agent, vm: &mut Vm) {
    // 2. Let oldValue be ToBoolean(? GetValue(expr)).
    let old_value = to_boolean(agent, vm.result.take().unwrap());

    // 3. If oldValue is true, return false.
    // 4. Return true.
    vm.result = Some((!old_value).into());
}

#[inline(always)]
pub(super) fn execute_initialize_referenced_binding(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) {
    let v = vm.reference.take().unwrap();
    let w = vm.result.take().unwrap();
    // Note: https://tc39.es/ecma262/#sec-initializereferencedbinding
    // suggests this cannot call user code, hence NoGC.
    unwrap_try(try_initialize_referenced_binding(agent, v, w, gc));
}

pub(super) fn execute_initialize_variable_environment<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) {
    let num_variables = instr.get_first_index();
    let strict = instr.get_second_bool();

    // 10.2.11 FunctionDeclarationInstantiation
    // 28.b. Let varEnv be NewDeclarativeEnvironment(env).
    let env = agent.current_lexical_environment(gc);
    let var_env = new_declarative_environment(agent, Some(env), gc);
    // c. Set the VariableEnvironment of calleeContext to varEnv.
    agent.set_current_variable_environment(var_env.into());

    // e. For each element n of varNames, do
    for _ in 0..num_variables {
        let n = String::try_from(vm.stack.pop().unwrap()).unwrap();
        let initial_value = vm.stack.pop().unwrap();
        // 2. Perform ! varEnv.CreateMutableBinding(n, false).
        var_env.create_mutable_binding(agent, n, false);
        // 5. Perform ! varEnv.InitializeBinding(n, initialValue).
        var_env.initialize_binding(agent, n, initial_value);
    }

    // 30. If strict is false, then
    let lex_env = if !strict {
        // a. Let lexEnv be NewDeclarativeEnvironment(varEnv).
        new_declarative_environment(agent, Some(Environment::Declarative(var_env)), gc)
    } else {
        // 31. Else,
        // a. Let lexEnv be varEnv.
        var_env
    };

    // 32. Set the LexicalEnvironment of calleeContext to lexEnv.
    agent.set_current_lexical_environment(lex_env.into());
}

pub(super) fn execute_enter_declarative_environment(agent: &mut Agent, gc: NoGcScope) {
    let outer_env = agent.current_lexical_environment(gc);
    let new_env = new_declarative_environment(agent, Some(outer_env), gc);
    agent.set_current_lexical_environment(new_env.into());
}

pub(super) fn execute_enter_class_static_element_environment(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope,
) {
    let class_constructor = Function::try_from(*vm.stack.last().unwrap())
        .unwrap()
        .bind(gc);
    let local_env = new_class_static_element_environment(agent, class_constructor, gc);
    let local_env = Environment::Function(local_env);

    agent.set_current_lexical_environment(local_env);
    agent.set_current_variable_environment(local_env);
}

pub(super) fn execute_enter_private_environment(agent: &mut Agent, instr: Instr, gc: NoGcScope) {
    let outer_env = agent.current_private_environment(gc);
    let new_env = new_private_environment(agent, outer_env, instr.get_first_index(), gc);
    agent.set_current_private_environment(new_env.into());
}

pub(super) fn execute_exit_declarative_environment(agent: &mut Agent, gc: NoGcScope) {
    let old_env = agent
        .current_lexical_environment(gc)
        .get_outer_env(agent)
        .unwrap();
    agent.set_current_lexical_environment(old_env);
}

pub(super) fn execute_exit_variable_environment(agent: &mut Agent, gc: NoGcScope) {
    let var_env = agent.current_variable_environment(gc);
    let old_env = match var_env {
        Environment::Function(env) => env.get_outer_variable_environment(agent),
        _ => None,
    }
    .or_else(|| var_env.get_outer_env(agent))
    .unwrap();
    agent.set_current_variable_environment(old_env);
}

pub(super) fn execute_exit_private_environment(agent: &mut Agent, gc: NoGcScope) {
    let old_env = agent
        .current_private_environment(gc)
        .unwrap()
        .get_outer_env(agent);
    agent.set_current_private_environment(old_env);
}

#[inline(always)]
pub(super) fn execute_create_mutable_binding(
    agent: &mut Agent,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let lex_env = agent.current_lexical_environment(gc);
    let name = executable.fetch_identifier(agent, instr.get_first_index(), gc);

    unwrap_try(lex_env.try_create_mutable_binding(agent, name.unbind(), false, None, gc));
}

#[inline(always)]
pub(super) fn execute_create_immutable_binding(
    agent: &mut Agent,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope,
) {
    let lex_env = agent.current_lexical_environment(gc);
    let name = executable.fetch_identifier(agent, instr.get_first_index(), gc);
    lex_env
        .create_immutable_binding(agent, name, true, gc)
        .unwrap();
}

pub(super) fn execute_throw_error<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let exception_type_immediate = instr.get_first_arg();
    let message = String::try_from(vm.result.take().unwrap()).unwrap();
    if exception_type_immediate == crate::engine::bytecode::UNSUPPORTED_SYNTAX_IMMEDIATE {
        // Code the compiler does not support compiles to a stub that throws.
        return Err(agent.throw_exception_with_message(ExceptionType::SyntaxError, message, gc));
    }

    let exception_type = ExceptionType::try_from(exception_type_immediate).unwrap();

    Err(agent.throw_exception_with_message(exception_type, message, gc))
}

pub(super) fn execute_push_exception_jump_target(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: NoGcScope,
) {
    let handler = ExceptionHandler::CatchBlock {
        // Note: jump slots are passed to us in 32 bits, this
        // conversion is lossless.
        ip: instr.get_jump_slot() as u32,
        lexical_environment: agent.current_lexical_environment(gc).unbind(),
        variable_environment: agent.current_variable_environment(gc).unbind(),
    };
    vm.exception_handlers(agent).push(handler);
}

pub(super) fn execute_instanceof_operator<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lval = vm.stack.pop().unwrap();
    let rval = vm.result.take().unwrap();
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| instanceof_operator(agent, lval, rval, gc),
        gc,
    )?;
    vm.result = Some(result.into());
    Ok(())
}

pub(super) fn execute_begin_simple_array_binding_pattern<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lexical = instr.get_second_bool();
    let env = if lexical {
        // Lexical binding, const [] = a; or let [] = a;
        Some(
            agent
                .current_lexical_environment(gc.nogc())
                .scope(agent, gc.nogc()),
        )
    } else {
        // Var binding, var [] = a;
        None
    };
    execute_simple_array_binding(agent, vm, executable, env, gc).unbind()?;
    Ok(())
}

pub(super) fn execute_begin_simple_object_binding_pattern<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let lexical = instr.get_first_bool();
    let env = if lexical {
        // Lexical binding, const {} = a; or let {} = a;
        Some(
            agent
                .current_lexical_environment(gc.nogc())
                .scope(agent, gc.nogc()),
        )
    } else {
        // Var binding, var {} = a;
        None
    };
    let object = to_object(agent, vm.result.take().unwrap(), gc.nogc())
        .unbind()?
        .bind(gc.nogc());
    execute_simple_object_binding(agent, vm, executable, object.unbind(), env, gc)
}

#[inline(never)]
pub(super) fn execute_binding_pattern() -> ! {
    unreachable!("BeginArrayBindingPattern should take care of stepping over these");
}

/// The longest text that a concatenation puts together without asking
/// whether it fits ([`Agent::check_concat_length`] lets as much through).
const UNASKED_CONCATENATION: usize = 4096;

/// The longest text that a concatenation puts together on the native stack
/// before it asks the heap for the String: most are shorter, and many are
/// Strings the heap has already.
const SHORT_CONCATENATION: usize = 64;

/// What a chain of `+` or a template literal left on the stack for one of
/// its parts: a String, by the instruction before the one that put it
/// there.
#[inline]
fn string_part<'a>(value: Value<'a>) -> String<'a> {
    debug_assert!(value.is_string());
    String::try_from(value).unwrap_or(String::EMPTY_STRING)
}

/// The parts of a short concatenation, none of them being built, as one
/// String: put together here, and copied to the heap only where it has no
/// such String yet. None where a lead surrogate at the end of one part and
/// a trail surrogate at the start of the next make one code point, which
/// the general way does.
fn concat_short_text<'gc>(
    agent: &mut Agent,
    values: &[Value<'gc>],
    gc: NoGcScope<'gc, '_>,
) -> Option<String<'gc>> {
    let mut text = [0u8; SHORT_CONCATENATION];
    let mut len = 0;
    let mut ends_with_lead = false;
    for value in values {
        let part = string_part(*value);
        let part = part.as_wtf8_(agent);
        if part.is_empty() {
            continue;
        }
        if ends_with_lead && part.initial_trail_surrogate().is_some() {
            return None;
        }
        let end = len + part.len();
        text.get_mut(len..end)?.copy_from_slice(part.as_bytes());
        len = end;
        ends_with_lead = part.final_lead_surrogate().is_some();
    }
    let text = wtf8::Wtf8Str::from_wtf8_bytes(&text[..len]);
    if let Ok(small) = String::try_from(text) {
        return Some(small);
    }
    Some(agent.heap.alloc_wtf8(text).bind(gc))
}

/// The Strings that a chain of `+` or a template literal left on the stack,
/// as one String. Strings that are not being built are put together at once,
/// with no String made of those before them. One that is being built takes
/// what stands before it and after it as the operators would have given it
/// that, so that a loop that builds a text with such a chain adds to the
/// text and does not copy it.
///
/// With `ask`, a text that does not fit is refused before it is made;
/// without it nothing is refused, and None is what a text long enough to be
/// asked for gets.
fn concat_strings<'gc>(
    agent: &mut Agent,
    values: &[Value<'gc>],
    ask: bool,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, Option<String<'gc>>> {
    let mut bytes = 0usize;
    let mut being_built = false;
    for value in values {
        match string_part(*value) {
            String::SmallString(part) => bytes += part.len(),
            String::String(part) => {
                let record = part.get(agent);
                bytes = bytes.saturating_add(record.len());
                being_built |= record.is_being_built();
            }
        }
    }
    if bytes > UNASKED_CONCATENATION && !ask {
        return Ok(None);
    }
    let ask = ask && bytes > UNASKED_CONCATENATION;
    if !being_built
        && bytes <= SHORT_CONCATENATION
        && let Some(text) = concat_short_text(agent, values, gc)
    {
        return Ok(Some(text));
    }
    let mut few = [String::EMPTY_STRING; 8];
    let many: Vec<String<'gc>>;
    let strings: &[String<'gc>] = if values.len() <= few.len() {
        for (slot, value) in few.iter_mut().zip(values) {
            *slot = string_part(*value);
        }
        &few[..values.len()]
    } else {
        many = values.iter().map(|value| string_part(*value)).collect();
        &many
    };
    let mut text: Option<String<'gc>> = None;
    let mut rest = strings;
    while !rest.is_empty() {
        // The Strings up to the next one that is being built, as one.
        let run = rest
            .iter()
            .position(|string| string.is_being_built_(agent))
            .unwrap_or(rest.len());
        let (next, after) = if run == 0 {
            (rest[0], &rest[1..])
        } else {
            let (run, after) = rest.split_at(run);
            if ask {
                agent.check_concat_lengths(run, gc)?;
            }
            (String::concat(agent, run, gc), after)
        };
        text = Some(match text {
            None => next,
            Some(text) => {
                if ask {
                    agent.check_concat_length(text, next, gc)?;
                }
                String::concat(agent, [text, next], gc)
            }
        });
        rest = after;
    }
    Ok(Some(text.unwrap_or(String::EMPTY_STRING)))
}

/// [`concat_strings`] of a text short enough that nothing is asked; None
/// for a longer one.
pub(super) fn concat_short_strings<'gc>(
    agent: &mut Agent,
    values: &[Value<'gc>],
    gc: NoGcScope<'gc, '_>,
) -> Option<String<'gc>> {
    concat_strings(agent, values, false, gc).ok().flatten()
}

pub(super) fn execute_concat_strings<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let count = instr.get_first_index();
    let first = vm.stack.len() - count;
    let string =
        concat_strings(agent, &vm.stack[first..], true, gc)?.unwrap_or(String::EMPTY_STRING);
    vm.stack.truncate(first);
    vm.result = Some(string.unbind().into());
    Ok(())
}

/// Makes the result value the String that a part of a concatenation is: a
/// substitution of a template literal (ToString), or an operand of `+`
/// whose other operand is a String (ToPrimitive, then ToString).
pub(super) fn execute_to_string_part<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    substitution: bool,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let value = vm.result.take().unwrap().bind(gc.nogc());
    let string = if let Ok(primitive) = Primitive::try_from(value) {
        to_string_primitive(agent, primitive.unbind(), gc.into_nogc())?
    } else {
        let value = value.unbind();
        with_vm_gc(
            agent,
            vm,
            |agent, mut gc| {
                if substitution {
                    return to_string(agent, value, gc);
                }
                let primitive =
                    crate::ecmascript::to_primitive(agent, value, None, gc.reborrow()).unbind()?;
                to_string_primitive(agent, primitive, gc.into_nogc())
            },
            gc,
        )?
    };
    vm.result = Some(string.unbind().into());
    Ok(())
}

pub(super) fn execute_enumerate_object_properties(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) {
    let object = to_object(agent, vm.result.take().unwrap(), gc).unwrap();
    vm.iterator_stack.push(
        VmIteratorRecord::ObjectProperties(Box::new(ObjectPropertiesIteratorRecord::new(
            object.unbind(),
        )))
        .unbind(),
    );
}

pub(super) fn execute_get_iterator_sync<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let expr_value = vm.result.take().unwrap();
    // An Array that is walked element by element: nothing runs to find
    // that out, and the `Vm` stays where it is.
    if let Some(walk) = VmIteratorRecord::of_array_walked(agent, expr_value, gc.nogc()) {
        vm.iterator_stack.push(walk.unbind());
        return Ok(());
    }
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| VmIteratorRecord::from_value(agent, expr_value, gc),
        gc,
    )?;
    vm.iterator_stack.push(result.unbind());
    Ok(())
}

pub(super) fn execute_get_iterator_async<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let expr_value = vm.result.take().unwrap();
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| VmIteratorRecord::async_from_value(agent, expr_value, gc),
        gc,
    )?;
    vm.iterator_stack.push(result.unbind());
    Ok(())
}

pub(super) fn execute_iterator_step_value<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // A step that runs nothing is taken here. Of an Array walked element
    // by element: where the next element is a value, the step is no more
    // than reading it. And of the properties `for`-`in` goes through,
    // where no object among them is a Proxy.
    let plain = match vm.get_active_iterator_mut() {
        VmIteratorRecord::ArrayValues(iterator) => iterator.plain_next(agent, gc.nogc()).unbind(),
        VmIteratorRecord::ObjectProperties(iterator) => {
            match try_result_into_option_js(iterator.try_step_value(agent, gc.nogc())) {
                Some(Ok(step)) => Some(step.unbind()),
                Some(Err(err)) => return Err(err.unbind().bind(gc.into_nogc())),
                None => None,
            }
        }
        _ => None,
    };
    let result = match plain {
        Some(step) => step.bind(gc.into_nogc()),
        None => with_vm_gc(agent, vm, step_value_of_waiting_vm, gc)?,
    };
    vm.result = result.unbind();
    if result.is_none() {
        // Iterator finished: jump to escape the iterator loop.
        let ip = instr.get_jump_slot();
        if agent.options.print_internals {
            eprintln!("Iterator finished, jumping to {ip}");
        }
        vm.ip = ip;
    }
    Ok(())
}

/// IteratorStepValue of the active iterator of the `Vm` that waits on top
/// of the Agent's while this runs. Where the step throws, the iterator is
/// left exhausted: it is not to be closed (7.4.8 IteratorStep and those
/// that call it set iteratorRecord.\[\[Done]] to true), which the handler
/// around a `for`-`of` loop would do for whatever is thrown in the loop.
/// The properties that `for`-`in` walks are nothing to close.
// A function of its own: seen to in the instruction, which is part of the
// loop of instructions, it moved that loop.
#[inline(never)]
fn step_value_of_waiting_vm<'gc>(
    agent: &mut Agent,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Option<Value<'gc>>> {
    let step = ActiveIterator::new(agent, gc.nogc()).step_value(agent, gc);
    if step.is_err()
        && let Some(vm) = agent.vm_stack.last_mut()
    {
        vm.exhaust_active_iterator();
    }
    step
}

pub(super) fn execute_iterator_step_value_or_undefined<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let result = if let Some(r) = try_result_into_option_js(
        vm.get_active_iterator_mut()
            .try_step_value(agent, gc.nogc()),
    ) {
        r.unbind().bind(gc.into_nogc())
    } else {
        with_vm_gc(
            agent,
            vm,
            |agent, gc| ActiveIterator::new(agent, gc.nogc()).step_value(agent, gc),
            gc,
        )
    };
    if result.map_or(true, |r| r.is_none()) {
        // We have exhausted the iterator or it threw an error;
        // replace the top iterator with an empty slice iterator so
        // further instructions aren't observable.
        *vm.get_active_iterator_mut() = VmIteratorRecord::EmptySliceIterator;
    }
    vm.result = Some(result?.unwrap_or(Value::Undefined).unbind());
    Ok(())
}

pub(super) fn execute_iterator_call_next_method<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let result = vm.result.take();
    vm.result = Some(
        with_vm_gc(
            agent,
            vm,
            |agent, gc| ActiveIterator::new(agent, gc.nogc()).call_next(agent, result, gc),
            gc,
        )?
        .unbind(),
    );
    Ok(())
}

pub(super) fn execute_iterator_complete<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // 1. If innerResult is not an Object, throw a TypeError
    //    exception.
    let result = vm
        .result
        .expect("No iterator result object")
        .bind(gc.nogc());
    let Ok(result) = Object::try_from(result) else {
        return Err(throw_iterator_returned_non_object(agent, gc.into_nogc()));
    };
    // 2. Let done be ? IteratorComplete(innerResult).
    let done = {
        let result = result.unbind();
        with_vm_gc(
            agent,
            vm,
            |agent, gc| iterator_complete(agent, result, gc),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc())
    };
    // 3. If done is true, then
    if done {
        let result = Object::try_from(vm.result.expect("result was checked to be an Object"))
            .expect("result was checked to be an Object")
            .bind(gc.nogc());
        // i. Return ? IteratorValue(innerResult).
        let result = {
            let result = result.unbind();
            with_vm_gc(
                agent,
                vm,
                // SAFETY: not shared.
                |agent, gc| iterator_value(agent, result, gc),
                gc,
            )?
        };
        vm.result = Some(result.unbind());
        let ip = instr.get_jump_slot();
        if agent.options.print_internals {
            eprintln!("IteratorComplete returned true, jumping to {ip}");
        }
        vm.ip = ip;
    }
    Ok(())
}

pub(super) fn execute_iterator_value<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let result = vm
        .result
        .expect("No iterator result object")
        .bind(gc.nogc());
    // NOTE: We crash here because this check should've been done
    // already.
    let result = Object::try_from(result).expect("Iterator returned a non-object result");
    // 1. Return ? IteratorValue(innerResult).
    let result = {
        let result = result.unbind();
        with_vm_gc(
            agent,
            vm,
            // SAFETY: not shared.
            |agent, gc| iterator_value(agent, result, gc),
            gc,
        )?
    };
    vm.result = Some(result.unbind());
    Ok(())
}

pub(super) fn execute_iterator_throw<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let result = vm.result.take().expect("IteratorThrow with no error");
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| ActiveIterator::new(agent, gc.nogc()).throw(agent, result, gc),
        gc,
    )?;
    if let Some(result) = result {
        // Throw method was found and called successfully.
        vm.result = Some(result.unbind());
    } else {
        // No throw method was found, we should jump to provided
        // instruction.
        let ip = instr.get_jump_slot();
        if agent.options.print_internals {
            eprintln!("No iterator throw method found, jumping to {ip}");
        }
        vm.ip = ip;
    }
    Ok(())
}

pub(super) fn execute_iterator_return<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    instr: Instr,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let result = vm.result.expect("IteratorReturn with no received value");
    let value = with_vm_gc(
        agent,
        vm,
        |agent, gc| ActiveIterator::new(agent, gc.nogc()).r#return(agent, Some(result), gc),
        gc,
    )?;
    if let Some(value) = value {
        // Return method was found and called successfully.
        vm.result = Some(value.unbind());
    } else {
        // No return method was found, we should jump to provided
        // instruction.
        let ip = instr.get_jump_slot();
        if agent.options.print_internals {
            eprintln!("No iterator return method found, jumping to {ip}");
        }
        vm.ip = ip;
    }
    Ok(())
}

pub(super) fn execute_iterator_rest_into_array<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let capacity = vm
        .get_active_iterator()
        .remaining_length_estimate(agent)
        .unwrap_or(0);
    let array = array_create(agent, 0, capacity, None, gc.nogc())
        .unbind()?
        .scope(agent, gc.nogc());

    let result = with_vm_gc::<JsResult<()>>(
        agent,
        vm,
        |agent, mut gc| {
            let mut idx: u32 = 0;
            while let Some(value) = ActiveIterator::new(agent, gc.nogc())
                .step_value(agent, gc.reborrow())
                .unbind()?
                .bind(gc.nogc())
            {
                let key = PropertyKey::Integer(idx.into());
                unwrap_try(try_create_data_property(
                    agent,
                    array.get(agent),
                    key,
                    value.unbind(),
                    None,
                    gc.nogc(),
                ));
                idx += 1;
            }
            Ok(())
        },
        gc,
    );
    // We have exhausted the iterator or it threw an error; replace
    // the top iterator with an empty slice iterator so further
    // instructions aren't observable.
    *vm.get_active_iterator_mut() = VmIteratorRecord::EmptySliceIterator;
    // Now we're ready to throw the possible error.;
    result?;

    // Store the array as the result.
    vm.result = Some(array.take(agent).into());
    Ok(())
}

pub(super) fn execute_iterator_close<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    if !vm
        .get_active_iterator()
        .requires_return_call(agent, gc.nogc())
    {
        return Ok(());
    }
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| ActiveIterator::new(agent, gc.nogc()).r#return(agent, None, gc),
        gc.reborrow(),
    )
    .unbind()?
    .bind(gc.nogc());
    if let Some(result) = result {
        // We did get innerResult from return method call: we have
        // to check that it is an object.
        verify_is_object(agent, result.unbind(), gc.into_nogc())?;
    }
    Ok(())
}

pub(super) fn execute_async_iterator_close<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, bool> {
    if !vm
        .get_active_iterator()
        .requires_return_call(agent, gc.nogc())
    {
        // Skip over VerifyIsObject, message, and Store.
        vm.ip += 4;
        return Ok(false);
    }
    let result = with_vm_gc(
        agent,
        vm,
        |agent, gc| ActiveIterator::new(agent, gc.nogc()).r#return(agent, None, gc),
        gc,
    )?;
    if let Some(result) = result {
        // AsyncIteratorClose
        // Iterator return method did return a value: we should
        // put it into the result slot, place our original
        // result into the stack, and perform an Await.
        let result = vm.result.replace(result.unbind());
        vm.stack.push(result.unwrap_or(Value::Undefined));
        return Ok(true);
    } else {
        // Skip over VerifyIsObject, message, and Store.
        vm.ip += 4;
    }
    Ok(false)
}

pub(super) fn execute_iterator_close_with_error(agent: &mut Agent, vm: &mut Vm, gc: GcScope) {
    // If our current active iterator requires a return call,
    // perform said call.
    if vm
        .get_active_iterator()
        .requires_return_call(agent, gc.nogc())
    {
        // We don't care if the return call throws an error or not,
        // nor if its returned value is an object or not: the
        // original throw completion will be rethrown before that.
        let _ = with_vm_gc(
            agent,
            vm,
            |agent, gc| ActiveIterator::new(agent, gc.nogc()).r#return(agent, None, gc),
            gc,
        );
    }
    // Continue to the next instruction which should either be a
    // throw or an IteratorPop followed by a throw.
}

pub(super) fn execute_async_iterator_close_with_error(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope,
) -> bool {
    if vm
        .get_active_iterator()
        .requires_return_call(agent, gc.nogc())
    {
        let inner_result_value = with_vm_gc(
            agent,
            vm,
            |agent, gc| ActiveIterator::new(agent, gc.nogc()).r#return(agent, None, gc),
            gc,
        );
        if let Ok(Some(value)) = inner_result_value {
            // ### 7.4.13 AsyncIteratorClose
            // 4.d. If innerResult is a normal completion, set
            //    innerResult to Completion(Await(innerResult.[[Value]])).
            // 5. If completion is a throw completion, return ? completion.

            // We need to await the value and ignore any errors it
            // might throw, then rethrow the error. First, we need
            // to load the error to the stack for later throwing.
            vm.stack.push(vm.result.take().unwrap());
            // Then we can put our value as the result.
            vm.result = Some(value.unbind());
            // Before we await we need to make sure that any error
            // throw by the await gets ignored. This also ignores
            // the next instruction coming after this, which would
            // pop the exception handler stack: the ignore is
            // necessary because handling the error pops the
            // exception handler stack as well. Without the ignore
            // we'd pop twice.
            vm.exception_handler_stack
                .push(ExceptionHandler::IgnoreErrorAndNextInstruction);
            // Now we're ready to await: if the await succeeds then
            // we'll continue execution which will pop the above
            // exception handler, store the error as the result and
            // then rethrow it. If the await throws an error, it
            // will trigger our exception handler which will skip
            // the exception handler pop instruction but otherwise
            // continue as above. As a result, our error is always
            // rethrown.
            return true;
        }
    }
    // If we did not find a return method or get a value to await
    // then we'll skip the PopExceptionJumpTarget and Store
    // instructions, and go straight to rethrow handling. Note, we
    // do not manually rethrow as there may be more steps between
    // this and the final Throw instruction.
    vm.ip += 2;
    false
}

pub(super) fn execute_create_unmapped_arguments_object<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let Some(VmIteratorRecord::SliceIterator(slice)) = vm.iterator_stack.last() else {
        unreachable!()
    };
    match create_unmapped_arguments_object(agent, slice, gc) {
        Ok(o) => {
            vm.result = Some(o.unbind().into());
            Ok(())
        }
        Err(err) => Err(agent.throw_allocation_exception(err, gc)),
    }
}

pub(super) fn execute_get_new_target<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    // 1. Let envRec be GetThisEnvironment().
    let env_rec = get_this_environment(agent, gc);
    // 2. Assert: envRec has a [[NewTarget]] field.
    let Environment::Function(env_rec) = env_rec else {
        return Err(throw_missing_function_environment(agent, gc));
    };
    // 3. Return envRec.[[NewTarget]].
    vm.result = Some(
        env_rec
            .get_new_target(agent)
            .map_or(Value::Undefined, |v| v.into())
            .unbind(),
    );
    Ok(())
}

pub(super) fn execute_import_call(agent: &mut Agent, vm: &mut Vm, gc: GcScope) {
    let specifier = vm.stack.pop().unwrap().bind(gc.nogc());
    let options = vm.result.take().bind(gc.nogc());
    vm.result = {
        let specifier = specifier.unbind();
        let options = options.unbind();
        Some(
            with_vm_gc(
                agent,
                vm,
                |agent, gc| evaluate_import_call(agent, specifier, options, gc),
                gc,
            )
            .unbind()
            .into(),
        )
    };
}

pub(super) fn execute_import_meta(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) {
    // 1. Let module be GetActiveScriptOrModule().
    let module = agent.get_active_script_or_module(gc);
    // 2. Assert: module is a Source Text Module Record.
    let Some(ScriptOrModule::SourceTextModule(module)) = module else {
        unreachable!()
    };
    // 3. Let importMeta be module.[[ImportMeta]].
    let import_meta = module.get_import_meta(agent);
    // 4. If importMeta is empty, then
    let import_meta = match import_meta {
        None => {
            // b. Let importMetaValues be HostGetImportMetaProperties(module).
            let import_meta_values = agent
                .host_hooks
                .get_import_meta_properties(agent, module, gc);
            // a. Set importMeta to OrdinaryObjectCreate(null).
            // c. For each Record { [[Key]], [[Value]] } p of importMetaValues, do
            // i. Perform ! CreateDataPropertyOrThrow(importMeta, p.[[Key]], p.[[Value]]).
            let import_meta = OrdinaryObject::create_object(
                agent,
                None,
                &import_meta_values
                    .into_iter()
                    .map(|(key, value)| ObjectEntry::new_data_entry(key, value))
                    .collect::<Box<[ObjectEntry]>>(),
            )
            .expect("Should perform GC here");
            // d. Perform HostFinalizeImportMeta(importMeta, module).
            agent
                .host_hooks
                .finalize_import_meta(agent, import_meta, module, gc);
            // e. Set module.[[ImportMeta]] to importMeta.
            module.set_import_meta(agent, import_meta);
            // f. Return importMeta.
            import_meta
        }
        Some(import_meta) => {
            // 5. Else,
            // a. Assert: importMeta is an Object.
            // b. Return importMeta.
            import_meta
        }
    };
    vm.result = Some(import_meta.unbind().into());
}

/// ### [13.5.1.2 Runtime Semantics: Evaluation](https://tc39.es/ecma262/#sec-delete-operator-runtime-semantics-evaluation)
pub(super) fn execute_delete<'a>(
    agent: &mut Agent,
    vm: &mut Vm,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, ()> {
    let reference = vm.reference.take().unwrap().bind(gc.nogc());
    // 3. If IsUnresolvableReference(ref) is true, then
    if is_unresolvable_reference(&reference) {
        // a. Assert: ref.[[Strict]] is false.
        debug_assert!(!reference.strict());
        // b. Return true.
        vm.result = Some(true.into());
    } else if is_property_reference(&reference) {
        // 4. If IsPropertyReference(ref) is true, then

        // a. Assert: IsPrivateReference(ref) is false.
        debug_assert!(!is_private_reference(&reference));
        // b. If IsSuperReference(ref) is true, throw a ReferenceError exception.
        if is_super_reference(&reference) {
            return Err(agent.throw_exception_with_static_message(
                ExceptionType::ReferenceError,
                "Invalid delete involving 'super'.",
                gc.into_nogc(),
            ));
        }
        // c. Let baseObj be ? ToObject(ref.[[Base]]).
        let base = reference.base_value();
        let mut base_obj = to_object(agent, base, gc.nogc()).unbind()?.bind(gc.nogc());
        let strict = reference.strict();
        // d. If ref.[[ReferencedName]] is not a property key, then
        let referenced_name = if !reference.is_static_property_reference() {
            // i. Set ref.[[ReferencedName]] to ? ToPropertyKey(ref.[[ReferencedName]]).
            let referenced_name = reference.referenced_name_value();
            if let Some(referenced_name) = to_property_key_simple(agent, referenced_name, gc.nogc())
            {
                referenced_name
            } else {
                let referenced_name = referenced_name.unbind();
                let scoped_base_obj = base_obj.scope(agent, gc.nogc());
                let referenced_name = with_vm_gc(
                    agent,
                    vm,
                    |agent, gc| to_property_key_complex(agent, referenced_name, gc),
                    gc.reborrow(),
                )
                .unbind()?
                .bind(gc.nogc());
                base_obj = scoped_base_obj.take(agent).bind(gc.nogc());
                referenced_name
            }
        } else {
            reference.referenced_name_property_key()
        };
        // e. Let deleteStatus be ? baseObj.[[Delete]](ref.[[ReferencedName]]).
        let delete_status = if let TryResult::Continue(delete_status) =
            base_obj.try_delete(agent, referenced_name, gc.nogc())
        {
            delete_status
        } else {
            let base_obj = base_obj.unbind();
            let referenced_name = referenced_name.unbind();
            with_vm_gc(
                agent,
                vm,
                |agent, gc| base_obj.internal_delete(agent, referenced_name, gc),
                gc.reborrow(),
            )
            .unbind()?
            .bind(gc.nogc())
        };
        // f. If deleteStatus is false and ref.[[Strict]] is true, throw a TypeError exception.
        if !delete_status && strict {
            return Err(agent.throw_exception_with_static_message(
                ExceptionType::TypeError,
                "Cannot delete property",
                gc.into_nogc(),
            ));
        }
        // g. Return deleteStatus.
        vm.result = Some(delete_status.into());
    } else {
        // 5. Else,

        // a. Let base be ref.[[Base]].
        // b. Assert: base is an Environment Record.
        let base = reference.base_env();
        let referenced_name = reference.referenced_name_string();
        // c. Return ? base.DeleteBinding(ref.[[ReferencedName]]).
        let result = if let Some(result) =
            try_result_into_js(base.try_delete_binding(agent, referenced_name, gc.nogc()))
                .unbind()?
        {
            result
        } else {
            let referenced_name = referenced_name.unbind();
            let base = base.unbind();
            with_vm_gc(
                agent,
                vm,
                |agent, gc| base.delete_binding(agent, referenced_name, gc),
                gc,
            )?
        };
        vm.result = Some(result.into());
    }
    Ok(())

    // Note 1

    // When a delete operator occurs within strict mode code, a
    // SyntaxError exception is thrown if its UnaryExpression is a
    // direct reference to a variable, function argument, or
    // function name. In addition, if a delete operator occurs
    // within strict mode code and the property to be deleted has
    // the attribute { [[Configurable]]: false } (or otherwise
    // cannot be deleted), a TypeError exception is thrown.

    // Note 2

    // The object that may be created in step 4.c is not accessible
    // outside of the above abstract operation and the ordinary
    // object [[Delete]] internal method. An implementation might
    // choose to avoid the actual creation of that object.
}

pub(super) fn execute_verify_is_object<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    executable: Scoped<Executable>,
    instr: Instr,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    let result = vm.result.unwrap();
    if !result.is_object() {
        let message = executable.fetch_identifier(agent, instr.get_first_index(), gc);
        return Err(agent.throw_exception_with_message(ExceptionType::TypeError, message, gc));
    }
    Ok(())
}

#[inline(never)]
#[cold]
fn handle_set_value_break<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    reference: &Reference,
    result: TryResult<SetResult>,
    value: Value,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, ()> {
    match result {
        ControlFlow::Continue(SetResult::Done)
        | ControlFlow::Continue(SetResult::Unwritable)
        | ControlFlow::Continue(SetResult::Accessor) => Ok(()),
        ControlFlow::Continue(SetResult::Proxy(proxy)) => {
            let p = reference.referenced_name_property_key();
            let receiver = reference.this_value(agent);
            let strict = reference.strict();
            with_vm_gc(
                agent,
                vm,
                |agent, gc| call_proxy_set(agent, proxy, p, value, receiver, strict, gc),
                gc,
            )
        }
        ControlFlow::Continue(SetResult::Set(setter)) => {
            let receiver = reference.this_value(agent);
            with_vm_gc(
                agent,
                vm,
                |agent, gc| {
                    call_function(
                        agent,
                        setter,
                        receiver,
                        Some(ArgumentsList::from_mut_value(&mut value.unbind())),
                        gc,
                    )
                },
                gc,
            )
            .map(|_| ())
        }
        ControlFlow::Break(TryError::Err(err)) => Err(err.unbind().bind(gc.into_nogc())),
        ControlFlow::Break(TryError::GcError) => with_vm_gc(
            agent,
            vm,
            |agent, gc| put_value(agent, reference, value, gc),
            gc,
        ),
    }
}

#[inline(never)]
#[cold]
fn mutate_reference_property_key<'gc>(
    agent: &mut Agent,
    vm: &mut Vm,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Reference<'gc>> {
    let reference = vm.reference.as_ref().unwrap();
    // Expression reference; we need to convert to PropertyKey
    // first.
    let referenced_name = reference.referenced_name_value().bind(gc.nogc());
    let referenced_name =
        if let Some(referenced_name) = to_property_key_simple(agent, referenced_name, gc.nogc()) {
            referenced_name
        } else {
            let base = reference.base_value().bind(gc.nogc());
            if base.is_undefined() || base.is_null() {
                // Undefined and null should throw an error from
                // ToObject before ToPropertyKey gets called.
                return Err(throw_read_undefined_or_null_error(
                    agent,
                    referenced_name.unbind(),
                    base.unbind(),
                    gc.into_nogc(),
                ));
            }
            let referenced_name = referenced_name.unbind();
            with_vm_gc(
                agent,
                vm,
                |agent, gc| to_property_key_complex(agent, referenced_name, gc),
                gc,
            )?
        };
    let reference = vm.reference.as_mut().unwrap();
    reference.set_referenced_name_to_property_key(referenced_name);
    Ok(reference.clone())
}

#[inline(never)]
#[cold]
fn handle_get_value_break<'a>(
    agent: &mut Agent,
    vm: &mut Vm,
    reference: &Reference,
    result: ControlFlow<TryError, TryGetValueContinue>,
    gc: GcScope<'a, '_>,
) -> JsResult<'a, Value<'a>> {
    let result = result.unbind();
    with_vm_gc(
        agent,
        vm,
        |agent, gc| match result {
            ControlFlow::Continue(TryGetValueContinue::Get { getter, receiver }) => {
                call_function(agent, getter, receiver, None, gc)
            }
            ControlFlow::Continue(TryGetValueContinue::Proxy {
                proxy,
                receiver,
                property_key,
            }) => proxy.internal_get(agent, property_key, receiver, gc),
            ControlFlow::Break(TryError::Err(err)) => Err(err.bind(gc.into_nogc())),
            ControlFlow::Break(TryError::GcError) => get_value(agent, reference, gc),
            _ => unreachable!(),
        },
        gc,
    )
}
