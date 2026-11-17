// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod binding_methods;
mod execute_instructions;

use execute_instructions::*;

use wtf8::Wtf8Buf;

use crate::{
    ecmascript::{
        Agent, ArgumentsList, BUILTIN_STRING_MEMORY, BigInt, Environment, ExceptionType, JsError,
        JsResult, Number, Object, Primitive, Promise, Reference, String, Value, call_function,
        get_method, is_callable, ordinary_has_instance, take_pooled_arguments_vec, to_boolean,
        to_numeric, to_numeric_primitive, to_primitive, to_property_key, to_string_primitive,
        try_get_object_method, try_result_into_option_js,
    },
    engine::{
        Bindable, GcScope, NoGcScope, Scopable, Scoped,
        bytecode::{
            Executable, IndexType, Instruction, InstructionIter, instructions::Instr,
            iterator::VmIteratorRecord,
        },
    },
    heap::{CompactionLists, HeapMarkAndSweep, WellKnownSymbols, WorkQueues},
};

#[derive(Debug, Bindable)]
pub(crate) enum ExecutionResult<'a> {
    Return(Value<'a>),
    Throw(JsError<'a>),
    Await {
        vm: SuspendedVm,
        promise: Promise<'a>,
    },
    Yield {
        vm: SuspendedVm,
        yielded_value: Value<'a>,
    },
}
impl<'a> ExecutionResult<'a> {
    pub(crate) fn into_js_result(self) -> JsResult<'a, Value<'a>> {
        match self {
            ExecutionResult::Return(value) => Ok(value),
            ExecutionResult::Throw(err) => Err(err.unbind()),
            _ => panic!("Unexpected yield or await"),
        }
    }
}

/// Indicates how the execution of an instruction should affect the remainder of
/// execution that contains it.
#[must_use]
enum ContinuationKind {
    Normal,
    Return,
    Yield,
    Await,
}

/// VM exception handler.
#[derive(Debug)]
enum ExceptionHandler<'a> {
    /// Indicates a jump to catch block.
    CatchBlock {
        /// Instruction pointer.
        ip: u32,
        /// The lexical environment which contains this exception jump target.
        lexical_environment: Environment<'a>,
        /// The variable environment there: a class static element that
        /// throws has its own.
        variable_environment: Environment<'a>,
    },
    /// Indicates that any error should be ignored and the next instruction
    /// skipped. This is used in AsyncIteratorClose handling.
    IgnoreErrorAndNextInstruction,
}

/// ## Notes
///
/// - This is inspired by and/or copied from Kiesel engine:
///   Copyright (c) 2023-2024 Linus Groh
#[derive(Debug, Default)]
pub(crate) struct Vm {
    /// Instruction pointer.
    ip: usize,
    stack: Vec<Value<'static>>,
    reference_stack: Vec<Reference<'static>>,
    iterator_stack: Vec<VmIteratorRecord<'static>>,
    exception_handler_stack: Vec<ExceptionHandler<'static>>,
    result: Option<Value<'static>>,
    reference: Option<Reference<'static>>,
}

#[derive(Debug)]
pub(crate) struct SuspendedVm {
    ip: usize,
    /// Note: Stack is empty only if the code contains no local variables
    /// optimised into stack slots or temporarily stored Values. A heap clone is
    /// probably often performed by the `stack.into_boxed_slice()` call.
    stack: Box<[Value<'static>]>,
    /// Note: Reference stack is non-empty only if the code awaits inside a call
    /// expression. This means that usually no heap data clone is required.
    reference_stack: Box<[Reference<'static>]>,
    /// Note: Iterator stack is non-empty only if the code awaits inside a
    /// for-in or for-of loop. This means that often no heap data clone is
    /// required.
    iterator_stack: Box<[VmIteratorRecord<'static>]>,
    /// Note: Exception jump stack is non-empty only if the code awaits inside a
    /// try block or an await for-of loop. This means that often no heap data
    /// clone is required.
    exception_jump_target_stack: Box<[ExceptionHandler<'static>]>,
}

impl SuspendedVm {
    /// Bytes held by the suspended stacks, for the heap size estimate.
    pub(crate) fn heap_bytes(&self) -> usize {
        core::mem::size_of_val(&*self.stack)
            + core::mem::size_of_val(&*self.reference_stack)
            + core::mem::size_of_val(&*self.iterator_stack)
            + core::mem::size_of_val(&*self.exception_jump_target_stack)
    }

    pub(crate) fn resume<'gc>(
        self,
        agent: &mut Agent,
        executable: Scoped<Executable>,
        value: Value,
        gc: GcScope<'gc, '_>,
    ) -> ExecutionResult<'gc> {
        if agent.options.print_internals {
            eprintln!("Resuming function with value\n");
        }
        let vm = Vm::from_suspended(self);
        vm.resume(agent, executable, value, gc)
    }

    pub(crate) fn resume_throw<'gc>(
        self,
        agent: &mut Agent,
        executable: Scoped<Executable>,
        err: Value,
        gc: GcScope<'gc, '_>,
    ) -> ExecutionResult<'gc> {
        if agent.options.print_internals {
            eprintln!("Resuming function with error\n");
        }
        // Optimisation: Avoid unsuspending the Vm if we're just going to throw
        // out of it immediately.
        if self.exception_jump_target_stack.is_empty() || agent.termination().is_some() {
            let err = JsError::new(err.unbind());
            return ExecutionResult::Throw(err);
        }
        let vm = Vm::from_suspended(self);
        vm.resume_throw(agent, executable, err, gc)
    }

    pub(crate) fn resume_return<'gc>(
        mut self,
        agent: &mut Agent,
        executable: Scoped<Executable>,
        result: Value,
        gc: GcScope<'gc, '_>,
    ) -> ExecutionResult<'gc> {
        if agent.options.print_internals {
            eprintln!("Resuming function with return\n");
        }
        // Following a yield point, the next instruction is a Jump to the
        // Normal continue handling. We need to ignore that.
        let next_instruction = executable.get_instruction(agent, &mut self.ip);
        assert_eq!(next_instruction.map(|i| i.kind), Some(Instruction::Jump));
        let peek_next_instruction = executable.peek_instruction_byte(agent, self.ip);
        if peek_next_instruction == Some(Instruction::Return.as_u8()) {
            // Our return handling is to just return; we can do that without
            // unsuspending the VM.
            return ExecutionResult::Return(result.bind(gc.into_nogc()));
        }
        let vm = Vm::from_suspended(self);
        vm.resume(agent, executable, result, gc)
    }
}

const VM_STACK_POOL_LEN: usize = 32;
const VM_STACK_POOL_MAX_CAPACITY: usize = 1024;

pub(crate) type PooledVmStacks = (Vec<Value<'static>>, Vec<VmIteratorRecord<'static>>);

impl Vm {
    const EMPTY: Self = Self {
        ip: 0,
        stack: Vec::new(),
        reference_stack: Vec::new(),
        iterator_stack: Vec::new(),
        exception_handler_stack: Vec::new(),
        result: None,
        reference: None,
    };

    fn new(agent: &mut Agent) -> Self {
        let (stack, iterator_stack) = agent
            .vm_stack_pool
            .pop()
            .unwrap_or_else(|| (Vec::with_capacity(32), Vec::new()));
        Self {
            ip: 0,
            stack,
            reference_stack: Vec::new(),
            iterator_stack,
            exception_handler_stack: Vec::new(),
            result: None,
            reference: None,
        }
    }

    fn recycle(mut self, agent: &mut Agent) {
        if self.stack.capacity() == 0
            || self.stack.capacity() > VM_STACK_POOL_MAX_CAPACITY
            || self.iterator_stack.capacity() > VM_STACK_POOL_MAX_CAPACITY
            || agent.vm_stack_pool.len() >= VM_STACK_POOL_LEN
        {
            return;
        }
        let mut stack = core::mem::take(&mut self.stack);
        let mut iterator_stack = core::mem::take(&mut self.iterator_stack);
        stack.clear();
        iterator_stack.clear();
        agent.vm_stack_pool.push((stack, iterator_stack));
    }

    fn suspend(mut self) -> SuspendedVm {
        SuspendedVm {
            ip: self.ip,
            stack: core::mem::take(&mut self.stack).into_boxed_slice(),
            reference_stack: core::mem::take(&mut self.reference_stack).into_boxed_slice(),
            iterator_stack: core::mem::take(&mut self.iterator_stack).into_boxed_slice(),
            exception_jump_target_stack: core::mem::take(&mut self.exception_handler_stack)
                .into_boxed_slice(),
        }
    }

    fn from_suspended(suspended: SuspendedVm) -> Self {
        Self {
            ip: suspended.ip,
            stack: suspended.stack.into_vec(),
            reference_stack: suspended.reference_stack.into_vec(),
            iterator_stack: suspended.iterator_stack.into_vec(),
            exception_handler_stack: suspended.exception_jump_target_stack.into_vec(),
            result: None,
            reference: None,
        }
    }

    /// Executes an executable using the virtual machine.
    pub(crate) fn execute<'gc>(
        agent: &mut Agent,
        executable: Scoped<Executable>,
        arguments: Option<&mut [Value<'static>]>,
        gc: GcScope<'gc, '_>,
    ) -> ExecutionResult<'gc> {
        let mut vm = Vm::new(agent);

        if let Some(arguments) = arguments {
            ArgumentsList::from_mut_slice(arguments).with_scoped(
                agent,
                |agent, arguments, gc| {
                    // Note: awaits and yields are invalid syntax inside an
                    // arguments list, so this list isn't used after this
                    // function returns.
                    vm.iterator_stack
                        .push(VmIteratorRecord::SliceIterator(arguments.into_static()));
                    if agent.options.print_internals {
                        vm.print_internals(agent, executable.clone(), gc.nogc());
                    }

                    vm.inner_execute(agent, executable, gc)
                },
                gc,
            )
        } else {
            if agent.options.print_internals {
                vm.print_internals(agent, executable.clone(), gc.nogc());
            }

            vm.inner_execute(agent, executable, gc)
        }
    }

    fn print_internals(&self, agent: &mut Agent, executable: Scoped<Executable>, gc: NoGcScope) {
        eprintln!();
        eprintln!("=== Executing Executable ===");
        eprintln!("Constants: {:?}", executable.get_constants(agent, gc));
        eprintln!();

        eprintln!("Instructions:");
        let instructions = executable.get_instructions(agent);
        let iter = InstructionIter::new(&instructions);
        for (ip, instr) in iter {
            instr.debug_print(agent, ip, executable.clone(), gc);
        }
        eprintln!();
    }

    fn resume<'gc>(
        mut self,
        agent: &mut Agent,
        executable: Scoped<Executable>,
        value: Value,
        gc: GcScope<'gc, '_>,
    ) -> ExecutionResult<'gc> {
        self.result = Some(value.unbind());
        self.inner_execute(agent, executable, gc)
    }

    fn resume_throw<'gc>(
        mut self,
        agent: &mut Agent,
        executable: Scoped<Executable>,
        err: Value,
        gc: GcScope<'gc, '_>,
    ) -> ExecutionResult<'gc> {
        let err = err.bind(gc.nogc());
        let err = JsError::new(err.unbind());
        if !self.handle_error(agent, err) {
            if agent.options.print_internals {
                eprintln!("Exiting function with error\n");
            }
            return ExecutionResult::Throw(err);
        }
        self.inner_execute(agent, executable, gc)
    }

    fn inner_execute<'gc>(
        mut self,
        agent: &mut Agent,
        executable: Scoped<Executable>,
        gc: GcScope<'gc, '_>,
    ) -> ExecutionResult<'gc> {
        let result = self.run(agent, executable, gc);
        self.recycle(agent);
        result
    }

    fn run<'gc>(
        &mut self,
        agent: &mut Agent,
        executable: Scoped<Executable>,
        mut gc: GcScope<'gc, '_>,
    ) -> ExecutionResult<'gc> {
        if agent.stack_exhausted() {
            return ExecutionResult::Throw(agent.throw_stack_overflow(gc.into_nogc()));
        }
        let stack_depth = agent.stack_refs.borrow().len();
        let instructions = executable.get_instructions(agent);
        while let Some(instr) = Instr::consume_instruction(&instructions, &mut self.ip) {
            agent.limits.countdown -= 1;
            if agent.limits.countdown == 0
                && let Err(err) = agent.poll_interrupt_now(gc.nogc())
            {
                return ExecutionResult::Throw(err.unbind().bind(gc.into_nogc()));
            }
            if agent.check_gc() {
                self.trigger_gc(agent, gc.reborrow());
            }
            if agent.options.print_internals {
                Self::print_executing(instr.kind);
            }
            let result =
                Self::execute_instruction(agent, self, executable.clone(), instr, gc.reborrow());
            match result {
                Ok(ContinuationKind::Normal) => {}
                _ => {
                    if let Some(r) = self.handle_execute_instruction_abnormal_result(agent, result)
                    {
                        return r.unbind().bind(gc.into_nogc());
                    }
                }
            }
            agent.stack_refs.get_mut().truncate(stack_depth);
        }

        if agent.termination().is_some() {
            return ExecutionResult::Throw(agent.termination_error(gc.into_nogc()));
        }
        ExecutionResult::Return(Value::Undefined)
    }

    /// ## Panics
    ///
    /// If result is Ok(ContinuationKind::Normal).
    #[inline(never)]
    #[cold]
    fn handle_execute_instruction_abnormal_result<'a>(
        &mut self,
        agent: &mut Agent,
        result: JsResult<'a, ContinuationKind>,
    ) -> Option<ExecutionResult<'a>> {
        match result {
            Ok(ContinuationKind::Normal) => {
                unreachable!("only called with an abnormal result")
            }
            Ok(ContinuationKind::Return) if agent.termination().is_some() => {
                // A callee swallowed a termination (an async function
                // rejecting its promise with it): end this frame with it too.
                Some(ExecutionResult::Throw(JsError::new(Value::Undefined)))
            }
            Ok(ContinuationKind::Return) => {
                if agent.options.print_internals {
                    Self::print_exiting();
                }
                let result = self.result.unwrap_or(Value::Undefined);
                Some(ExecutionResult::Return(result))
            }
            Ok(ContinuationKind::Yield) => {
                let yielded_value = self.result.take().unwrap();
                if agent.options.print_internals {
                    Self::print_yielding(yielded_value);
                }
                Some(ExecutionResult::Yield {
                    vm: core::mem::take(self).suspend(),
                    yielded_value,
                })
            }
            Ok(ContinuationKind::Await) => {
                if agent.options.print_internals {
                    Self::print_awaiting();
                }
                let Value::Promise(promise) = self.result.take().unwrap() else {
                    unreachable!()
                };
                Some(ExecutionResult::Await {
                    vm: core::mem::take(self).suspend(),
                    promise,
                })
            }
            Err(err) => {
                if !self.handle_error(agent, err) {
                    if agent.options.print_internals {
                        Self::print_exiting_with_error();
                    }
                    Some(ExecutionResult::Throw(err.unbind()))
                } else {
                    None
                }
            }
        }
    }

    #[inline(never)]
    #[cold]
    fn trigger_gc(&mut self, agent: &mut Agent, gc: GcScope) {
        with_vm_gc(agent, self, |agent, gc| agent.gc(gc), gc);
    }

    #[inline(never)]
    #[cold]
    fn print_executing(instruction: Instruction) {
        eprintln!("Executing: {instruction:?}");
    }

    #[inline(never)]
    #[cold]
    fn print_exiting() {
        eprintln!("Exiting function with result\n");
    }

    #[inline(never)]
    #[cold]
    fn print_exiting_with_error() {
        eprintln!("Exiting function with error\n");
    }

    #[inline(never)]
    #[cold]
    fn print_awaiting() {
        eprintln!("Awaiting value in function\n");
    }

    #[inline(never)]
    #[cold]
    fn print_yielding(yielded_value: Value) {
        eprintln!("Yielding value from function {yielded_value:?}\n");
    }

    #[inline(never)]
    #[cold]
    #[must_use]
    fn handle_error(&mut self, agent: &mut Agent, err: JsError) -> bool {
        if agent.termination().is_some() {
            return false;
        }
        if let Some(handler) = self.exception_handler_stack.pop() {
            match handler {
                ExceptionHandler::CatchBlock {
                    ip,
                    lexical_environment,
                    variable_environment,
                } => {
                    if agent.options.print_internals {
                        eprintln!("Error: {:?}", err.value());
                        eprintln!("Jumping to catch block in {ip}\n");
                    }
                    self.ip = ip as usize;
                    agent.set_current_lexical_environment(lexical_environment);
                    agent.set_current_variable_environment(variable_environment);
                    self.result = Some(err.value().unbind());
                }
                ExceptionHandler::IgnoreErrorAndNextInstruction => {
                    if agent.options.print_internals {
                        eprintln!("Ignoring throw error and skipping to {}\n", self.ip + 1);
                    }
                    self.ip += 1;
                }
            }
            true
        } else {
            false
        }
    }

    // Inlined into the loop of `run` in optimised builds, where it saves a
    // call for every instruction; unoptimised, the merged frame would take
    // many times the stack for every JavaScript call.
    #[cfg_attr(not(debug_assertions), inline(always))]
    fn execute_instruction<'a>(
        agent: &mut Agent,
        vm: &mut Vm,
        executable: Scoped<Executable>,
        instr: Instr,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, ContinuationKind> {
        // Hot instructions; apply #[inline(always)] to the execute methods.
        match instr.kind {
            Instruction::Load => {
                vm.execute_load();
            }
            Instruction::LoadCopy => {
                vm.execute_load_copy();
            }
            Instruction::PutValueToIndex => {
                vm.execute_load_to_index(instr.get_first_index());
            }
            Instruction::Store => {
                vm.execute_store();
            }
            Instruction::GetValueFromIndex => {
                vm.execute_store_from_index(instr.get_first_index());
            }
            Instruction::StoreConstant => {
                execute_store_constant(agent, vm, executable, instr, gc.into_nogc());
            }
            Instruction::PopStack => {
                vm.execute_pop_stack();
            }
            Instruction::Jump => execute_jump(agent, vm, instr),
            Instruction::JumpIfNot => execute_jump_if_not(agent, vm, instr),
            Instruction::ResolveBinding => {
                execute_resolve_binding(agent, vm, executable, instr, gc)?;
            }
            Instruction::GetValue
            | Instruction::GetValueWithCache
            | Instruction::GetValueKeepReference
            | Instruction::GetValueWithCacheKeepReference => {
                let cache = matches!(
                    instr.kind,
                    Instruction::GetValueWithCache | Instruction::GetValueWithCacheKeepReference
                );
                let keep_reference = matches!(
                    instr.kind,
                    Instruction::GetValueKeepReference
                        | Instruction::GetValueWithCacheKeepReference
                );
                execute_get_value(agent, vm, executable, instr, cache, keep_reference, gc)?
            }
            Instruction::PutValue | Instruction::PutValueWithCache => {
                let cache = matches!(instr.kind, Instruction::PutValueWithCache);
                execute_put_value(agent, vm, executable, instr, cache, gc)?
            }
            Instruction::ToNumeric => execute_to_numeric(agent, vm, gc)?,
            Instruction::CreateImmutableBinding => {
                execute_create_immutable_binding(agent, executable, instr, gc.into_nogc())
            }
            Instruction::CreateMutableBinding => {
                execute_create_mutable_binding(agent, executable, instr, gc.into_nogc())
            }
            Instruction::InitializeReferencedBinding => {
                execute_initialize_referenced_binding(agent, vm, gc.into_nogc())
            }
            Instruction::PushReference => vm.execute_push_reference(),
            Instruction::PopReference => vm.execute_pop_reference(),
            Instruction::EvaluatePropertyAccessWithIdentifierKey => {
                execute_evaluate_property_access_with_identifier_key(
                    agent,
                    vm,
                    executable,
                    instr,
                    gc.into_nogc(),
                )
            }
            Instruction::EvaluatePropertyAccessWithExpressionKey => {
                execute_evaluate_property_access_with_expression_key(agent, vm, gc.into_nogc())
            }
            Instruction::LoadFromIndex => {
                vm.execute_store_from_index(instr.get_first_index());
                vm.execute_load();
            }
            Instruction::GetPropertyWithCache | Instruction::GetPropertyWithCacheKeepReference => {
                execute_evaluate_property_access_with_identifier_key(
                    agent,
                    vm,
                    executable.clone(),
                    instr,
                    gc.nogc(),
                );
                let keep_reference =
                    matches!(instr.kind, Instruction::GetPropertyWithCacheKeepReference);
                let get_value = Instr::new_with_arg(
                    if keep_reference {
                        Instruction::GetValueWithCacheKeepReference
                    } else {
                        Instruction::GetValueWithCache
                    },
                    instr.get_second_index() as IndexType,
                );
                execute_get_value(agent, vm, executable, get_value, true, keep_reference, gc)?
            }
            Instruction::PushReferenceAndLoad => {
                vm.execute_push_reference();
                vm.execute_load();
            }
            // What every loop and expression is made of: one call from here,
            // not two.
            Instruction::LessThan => execute_less_than(agent, vm, gc)?,
            Instruction::LessThanEquals => execute_less_than_equals(agent, vm, gc)?,
            Instruction::GreaterThan => execute_greater_than(agent, vm, gc)?,
            Instruction::GreaterThanEquals => execute_greater_than_equals(agent, vm, gc)?,
            Instruction::Increment => execute_increment(agent, vm, gc.into_nogc()),
            Instruction::Decrement => execute_decrement(agent, vm, gc.into_nogc()),
            Instruction::ApplyAdditionBinaryOperator => {
                execute_apply_addition_binary_operator(agent, vm, gc)?
            }
            Instruction::ApplySubtractionBinaryOperator
            | Instruction::ApplyMultiplicationBinaryOperator
            | Instruction::ApplyDivisionBinaryOperator
            | Instruction::ApplyRemainderBinaryOperator
            | Instruction::ApplyShiftLeftBinaryOperator
            | Instruction::ApplyShiftRightBinaryOperator
            | Instruction::ApplyShiftRightZeroFillBinaryOperator
            | Instruction::ApplyBitwiseORBinaryOperator
            | Instruction::ApplyBitwiseXORBinaryOperator
            | Instruction::ApplyBitwiseAndBinaryOperator => {
                execute_apply_binary_operator(agent, vm, instr.kind, gc)?
            }
            Instruction::IsStrictlyEqual => execute_is_strictly_equal(agent, vm, gc.into_nogc()),
            Instruction::JumpIfTrue => execute_jump_if_true(agent, vm, instr),
            Instruction::LoadConstant => {
                execute_load_constant(agent, vm, executable, instr, gc.into_nogc())
            }
            Instruction::EvaluateCall => execute_evaluate_call(agent, vm, instr, gc)?,
            Instruction::Return => return Ok(ContinuationKind::Return),
            _ => return Self::execute_cold_instruction(agent, vm, executable, instr, gc),
        }

        Ok(ContinuationKind::Normal)
    }

    #[inline(never)]
    fn execute_cold_instruction<'a>(
        agent: &mut Agent,
        vm: &mut Vm,
        executable: Scoped<Executable>,
        instr: Instr,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, ContinuationKind> {
        let _: () = match instr.kind {
            Instruction::Load
            | Instruction::LoadCopy
            | Instruction::PutValueToIndex
            | Instruction::Store
            | Instruction::StoreConstant
            | Instruction::GetValueFromIndex
            | Instruction::PopStack
            | Instruction::Jump
            | Instruction::JumpIfNot
            | Instruction::ResolveBinding
            | Instruction::GetValue
            | Instruction::GetValueWithCache
            | Instruction::GetValueKeepReference
            | Instruction::GetValueWithCacheKeepReference
            | Instruction::PutValue
            | Instruction::PutValueWithCache
            | Instruction::ToNumeric
            | Instruction::CreateImmutableBinding
            | Instruction::CreateMutableBinding
            | Instruction::InitializeReferencedBinding
            | Instruction::PushReference
            | Instruction::PopReference
            | Instruction::EvaluatePropertyAccessWithIdentifierKey
            | Instruction::EvaluatePropertyAccessWithExpressionKey
            | Instruction::LoadFromIndex
            | Instruction::GetPropertyWithCache
            | Instruction::GetPropertyWithCacheKeepReference
            | Instruction::PushReferenceAndLoad => {
                unreachable!("hot instruction not handled before execute_cold_instruction")
            }
            Instruction::Return => return Ok(ContinuationKind::Return),
            Instruction::Await => {
                execute_await_promise_resolve(agent, vm, gc)?;
                return Ok(ContinuationKind::Await);
            }
            Instruction::Yield => return Ok(ContinuationKind::Yield),
            Instruction::IsStrictlyEqual => execute_is_strictly_equal(agent, vm, gc.into_nogc()),
            Instruction::IsStrictlyEqualConstant => {
                vm.execute_load();
                execute_store_constant(agent, vm, executable, instr, gc.nogc());
                execute_is_strictly_equal(agent, vm, gc.into_nogc())
            }
            Instruction::IsNullOrUndefined => vm.execute_is_null_or_undefined(),
            Instruction::IsNull => vm.execute_is_null(),
            Instruction::IsUndefined => vm.execute_is_undefined(),
            Instruction::IsObject => vm.execute_is_object(),
            Instruction::IsConstructor => execute_is_constructor(agent, vm),
            Instruction::JumpIfTrue => execute_jump_if_true(agent, vm, instr),
            Instruction::LoadReplace => vm.execute_load_replace(),
            Instruction::LoadConstant => {
                execute_load_constant(agent, vm, executable, instr, gc.into_nogc())
            }
            Instruction::LoadConstantWide => {
                execute_load_constant_wide(agent, vm, executable, instr, gc.into_nogc())
            }
            Instruction::StoreConstantWide => {
                execute_store_constant_wide(agent, vm, executable, instr, gc.into_nogc())
            }
            Instruction::LoadStoreSwap => vm.execute_load_store_swap(),
            Instruction::UpdateEmpty => vm.execute_update_empty(),
            Instruction::Swap => vm.execute_swap(),
            Instruction::Empty => vm.execute_empty(),
            Instruction::LogicalNot => execute_logical_not(agent, vm),
            Instruction::ApplyAdditionBinaryOperator => {
                execute_apply_addition_binary_operator(agent, vm, gc)?
            }
            Instruction::ApplySubtractionBinaryOperator
            | Instruction::ApplyMultiplicationBinaryOperator
            | Instruction::ApplyDivisionBinaryOperator
            | Instruction::ApplyRemainderBinaryOperator
            | Instruction::ApplyExponentialBinaryOperator
            | Instruction::ApplyShiftLeftBinaryOperator
            | Instruction::ApplyShiftRightBinaryOperator
            | Instruction::ApplyShiftRightZeroFillBinaryOperator
            | Instruction::ApplyBitwiseORBinaryOperator
            | Instruction::ApplyBitwiseXORBinaryOperator
            | Instruction::ApplyBitwiseAndBinaryOperator => {
                execute_apply_binary_operator(agent, vm, instr.kind, gc)?
            }
            Instruction::ArrayCreate => execute_array_create(agent, vm, instr, gc.into_nogc())?,
            Instruction::ArrayPush => execute_array_push(agent, vm, gc)?,
            Instruction::ArrayElision => execute_array_elision(agent, vm, gc)?,
            Instruction::BitwiseNot => execute_bitwise_not(agent, vm, gc.into_nogc())?,
            Instruction::CreateUnmappedArgumentsObject => {
                execute_create_unmapped_arguments_object(agent, vm, gc.into_nogc())?
            }
            Instruction::CopyDataProperties => execute_copy_data_properties(agent, vm, gc)?,
            Instruction::CopyDataPropertiesIntoObject => {
                execute_copy_data_properties_into_object(agent, vm, instr, gc)?
            }
            Instruction::Delete => execute_delete(agent, vm, gc)?,
            Instruction::DirectEvalCall => execute_direct_eval_call(agent, vm, instr, gc)?,
            Instruction::EvaluateCall => execute_evaluate_call(agent, vm, instr, gc)?,
            Instruction::PopReferenceAndEvaluateCall => {
                vm.execute_pop_reference();
                execute_evaluate_call(agent, vm, instr, gc)?
            }
            Instruction::EvaluateNew => execute_evaluate_new(agent, vm, instr, gc)?,
            Instruction::EvaluateSuper => execute_evaluate_super(agent, vm, instr, gc)?,
            Instruction::MakePrivateReference => {
                execute_make_private_reference(agent, vm, executable, instr, gc.into_nogc())
            }
            Instruction::MakeSuperPropertyReferenceWithExpressionKey => {
                execute_make_super_property_reference_with_expression_key(
                    agent,
                    vm,
                    gc.into_nogc(),
                )?
            }
            Instruction::MakeSuperPropertyReferenceWithIdentifierKey => {
                execute_make_super_property_reference_with_identifier_key(
                    agent,
                    vm,
                    executable,
                    instr,
                    gc.into_nogc(),
                )?
            }
            Instruction::GreaterThan => execute_greater_than(agent, vm, gc)?,
            Instruction::GreaterThanEquals => execute_greater_than_equals(agent, vm, gc)?,
            Instruction::HasProperty => execute_has_property(agent, vm, gc)?,
            Instruction::HasPrivateElement => {
                execute_has_private_element(agent, vm, gc.into_nogc())?
            }
            Instruction::Increment => execute_increment(agent, vm, gc.into_nogc()),
            Instruction::Decrement => execute_decrement(agent, vm, gc.into_nogc()),
            Instruction::InstanceofOperator => execute_instanceof_operator(agent, vm, gc)?,
            Instruction::InstantiateArrowFunctionExpression => {
                execute_instantiate_arrow_function_expression(agent, vm, executable, instr, gc)?
            }
            Instruction::InstantiateOrdinaryFunctionExpression => {
                execute_instantiate_ordinary_function_expression(agent, vm, executable, instr, gc)?
            }
            Instruction::ClassDefineConstructor => {
                execute_class_define_constructor(agent, vm, executable, instr, gc)?
            }
            Instruction::ClassDefineDefaultConstructor => {
                execute_class_define_default_constructor(agent, vm, executable, instr, gc)?
            }
            Instruction::ClassDefinePrivateMethod => {
                execute_class_define_private_method(agent, vm, executable, instr, gc)?
            }
            Instruction::ClassDefinePrivateProperty => {
                execute_class_define_private_property(agent, vm, executable, instr, gc.into_nogc())?
            }
            Instruction::ClassInitializePrivateElements => {
                execute_class_initialize_private_elements(agent, vm, gc.into_nogc())?
            }
            Instruction::ClassInitializePrivateValue => {
                execute_class_initialize_private_value(agent, vm, instr, gc.into_nogc())?
            }
            Instruction::LessThan => execute_less_than(agent, vm, gc)?,
            Instruction::LessThanEquals => execute_less_than_equals(agent, vm, gc)?,
            Instruction::IsLooselyEqual => execute_is_loosely_equal(agent, vm, gc)?,
            Instruction::ObjectCreate => execute_object_create(agent, vm, gc.into_nogc()),
            Instruction::ObjectCreateWithShape => {
                execute_object_create_with_shape(agent, vm, executable, instr, gc.into_nogc())
            }
            Instruction::ObjectDefineProperty => execute_object_define_property(agent, vm, gc)?,
            Instruction::ObjectDefineMethod => {
                execute_object_define_method(agent, vm, executable, instr, gc)?
            }
            Instruction::ObjectDefineGetter => {
                execute_object_define_getter(agent, vm, executable, instr, gc)?
            }
            Instruction::ObjectDefineSetter => {
                execute_object_define_setter(agent, vm, executable, instr, gc)?
            }
            Instruction::ObjectSetPrototype => execute_object_set_prototype(agent, vm, gc)?,
            Instruction::PopExceptionJumpTarget => vm.execute_pop_exception_jump_target(),
            Instruction::PushExceptionJumpTarget => {
                execute_push_exception_jump_target(agent, vm, instr, gc.into_nogc())
            }
            Instruction::TruncateStack => vm.stack.truncate(instr.get_first_arg() as usize),
            Instruction::ResolveBindingWithCache => {
                execute_resolve_binding_with_cache(agent, vm, executable, instr, gc)?
            }
            Instruction::ResolveThisBinding => {
                execute_resolve_this_binding(agent, vm, gc.into_nogc())?
            }
            Instruction::StoreCopy => vm.execute_store_copy(),
            Instruction::StringConcat => execute_string_concat(agent, vm, instr, gc)?,
            Instruction::Throw => vm.execute_throw(gc.into_nogc())?,
            Instruction::ThrowError => execute_throw_error(agent, vm, instr, gc.into_nogc())?,
            Instruction::ToNumber => execute_to_number(agent, vm, gc)?,
            Instruction::ToObject => execute_to_object(agent, vm, gc.into_nogc())?,
            Instruction::Typeof => execute_typeof(agent, vm, gc)?,
            Instruction::UnaryMinus => execute_unary_minus(agent, vm, gc.into_nogc()),
            Instruction::InitializeVariableEnvironment => {
                execute_initialize_variable_environment(agent, vm, instr, gc.into_nogc())
            }
            Instruction::EnterDeclarativeEnvironment => {
                execute_enter_declarative_environment(agent, gc.into_nogc())
            }
            Instruction::EnterClassStaticElementEnvironment => {
                execute_enter_class_static_element_environment(agent, vm, gc.into_nogc())
            }
            Instruction::EnterPrivateEnvironment => {
                execute_enter_private_environment(agent, instr, gc.into_nogc())
            }
            Instruction::ExitDeclarativeEnvironment => {
                execute_exit_declarative_environment(agent, gc.into_nogc())
            }
            Instruction::ExitVariableEnvironment => {
                execute_exit_variable_environment(agent, gc.into_nogc())
            }
            Instruction::ExitPrivateEnvironment => {
                execute_exit_private_environment(agent, gc.into_nogc())
            }
            Instruction::BeginSimpleObjectBindingPattern => {
                execute_begin_simple_object_binding_pattern(agent, vm, executable, instr, gc)?
            }
            Instruction::BeginSimpleArrayBindingPattern => {
                execute_begin_simple_array_binding_pattern(agent, vm, executable, instr, gc)?
            }
            Instruction::BindingPatternBind
            | Instruction::BindingPatternBindNamed
            | Instruction::BindingPatternBindToIndex
            | Instruction::BindingPatternBindRest
            | Instruction::BindingPatternBindRestToIndex
            | Instruction::BindingPatternSkip
            | Instruction::BindingPatternGetValue
            | Instruction::BindingPatternGetValueNamed
            | Instruction::BindingPatternGetRestValue
            | Instruction::FinishBindingPattern => execute_binding_pattern(),
            Instruction::EnumerateObjectProperties => {
                execute_enumerate_object_properties(agent, vm, gc.into_nogc())
            }
            Instruction::GetIteratorSync => execute_get_iterator_sync(agent, vm, gc)?,
            Instruction::GetIteratorAsync => execute_get_iterator_async(agent, vm, gc)?,
            Instruction::IteratorStepValue => execute_iterator_step_value(agent, vm, instr, gc)?,
            Instruction::IteratorStepValueOrUndefined => {
                execute_iterator_step_value_or_undefined(agent, vm, gc)?
            }
            Instruction::IteratorCallNextMethod => {
                execute_iterator_call_next_method(agent, vm, gc)?
            }
            Instruction::IteratorComplete => execute_iterator_complete(agent, vm, instr, gc)?,
            Instruction::IteratorValue => execute_iterator_value(agent, vm, gc)?,
            Instruction::IteratorThrow => execute_iterator_throw(agent, vm, instr, gc)?,
            Instruction::IteratorReturn => execute_iterator_return(agent, vm, instr, gc)?,
            Instruction::IteratorRestIntoArray => execute_iterator_rest_into_array(agent, vm, gc)?,
            Instruction::IteratorClose => execute_iterator_close(agent, vm, gc)?,
            Instruction::AsyncIteratorClose => {
                if execute_async_iterator_close(agent, vm, gc)? {
                    return Ok(ContinuationKind::Await);
                }
            }
            Instruction::IteratorCloseWithError => execute_iterator_close_with_error(agent, vm, gc),
            Instruction::AsyncIteratorCloseWithError => {
                if execute_async_iterator_close_with_error(agent, vm, gc) {
                    return Ok(ContinuationKind::Await);
                }
            }
            Instruction::IteratorPop => {
                let _ = vm.pop_iterator(gc.into_nogc());
            }
            Instruction::GetNewTarget => execute_get_new_target(agent, vm, gc.into_nogc())?,
            Instruction::ImportCall => execute_import_call(agent, vm, gc),
            Instruction::ImportMeta => execute_import_meta(agent, vm, gc.into_nogc()),
            Instruction::VerifyIsObject => {
                execute_verify_is_object(agent, vm, executable, instr, gc.into_nogc())?
            }
            Instruction::Debug => execute_debug(agent, vm),
        };
        Ok(ContinuationKind::Normal)
    }

    fn get_call_args<'gc>(
        &mut self,
        agent: &mut Agent,
        instr: Instr,
        _gc: NoGcScope<'gc, '_>,
    ) -> Vec<Value<'gc>> {
        let instr_arg0 = instr.get_first_arg();
        if instr_arg0 != IndexType::MAX {
            // Static number of arguments less than IndexType::MAX.
            let arg_count = instr_arg0 as usize;
            debug_assert!(self.stack.len() >= arg_count);
            self.split_off_call_args(agent, arg_count)
        } else {
            // Dynamic number of arguments, or exactly IndexType::MAX or more
            // arguments. In this case the number of arguments is stored in the
            // result register for us. Additionally, an extra accumulator value
            // is stored on the stack before the arguments.
            let Value::Integer(integer) = self.result.take().unwrap() else {
                panic!("Expected the number of function arguments to be an integer")
            };
            let arg_count = usize::try_from(integer.into_i64()).unwrap();
            debug_assert!(self.stack.len() > arg_count);
            let args = self.split_off_call_args(agent, arg_count);
            let integer_copy = self.stack.pop().unwrap();
            debug_assert_eq!(Value::Integer(integer), integer_copy);
            args
        }
    }

    fn split_off_call_args(&mut self, agent: &mut Agent, arg_count: usize) -> Vec<Value<'static>> {
        let start = self.stack.len() - arg_count;
        let args = take_pooled_arguments_vec(agent, &self.stack[start..]);
        self.stack.truncate(start);
        args
    }

    /// Pop the active (top-most) iterator from the iterator stack.
    ///
    /// ### Panics
    ///
    /// Panics if the iterator stack is empty.
    pub(super) fn pop_iterator<'gc>(&mut self, gc: NoGcScope<'gc, '_>) -> VmIteratorRecord<'gc> {
        self.iterator_stack
            .pop()
            .expect("Iterator stack is empty")
            .bind(gc)
    }

    /// Get the active (top-most) iterator from the iterator stack.
    ///
    /// ### Panics
    ///
    /// Panics if the iterator stack is empty.
    pub(super) fn get_active_iterator(&self) -> &VmIteratorRecord<'static> {
        self.iterator_stack.last().expect("Iterator stack is empty")
    }

    /// Get the active (top-most) iterator from the iterator stack as mutable.
    ///
    /// ### Panics
    ///
    /// Panics if the iterator stack is empty.
    pub(super) fn get_active_iterator_mut(&mut self) -> &mut VmIteratorRecord<'static> {
        self.iterator_stack
            .last_mut()
            .expect("Iterator stack is empty")
    }

    #[inline(always)]
    fn execute_is_null_or_undefined(&mut self) {
        let val = self.result.take().unwrap();
        let result = val.is_null() || val.is_undefined();
        self.result = Some(result.into());
    }

    #[inline(always)]
    fn execute_load(&mut self) {
        self.stack.push(self.result.take().unwrap());
    }

    #[inline(always)]
    fn execute_load_copy(&mut self) {
        self.stack.push(self.result.unwrap());
    }

    #[inline(always)]
    fn execute_load_to_index(&mut self, index: usize) {
        self.stack[index] = self.result.take().unwrap();
    }

    #[inline(always)]
    fn execute_load_store_swap(&mut self) {
        let temp = self
            .result
            .take()
            .expect("Expected result value to not be empty");
        self.result = Some(self.stack.pop().expect("Trying to pop from empty stack"));
        self.stack.push(temp);
    }

    #[inline(always)]
    fn execute_load_replace(&mut self) {
        // Take result, if present, and replace the top of the stack
        // value with it.
        if let Some(result) = self.result.take() {
            let temp = self
                .stack
                .last_mut()
                .expect("Trying to replace top of empty stack");
            *temp = result;
        }
    }

    #[inline(always)]
    fn execute_update_empty(&mut self) {
        // Take top of the stack value, set it as the result if no
        // result exists yet.
        let temp = self.stack.pop().expect("Trying to pop from empty stack");
        if self.result.is_none() {
            self.result = Some(temp);
        }
    }

    #[inline(always)]
    fn execute_store(&mut self) {
        self.result = Some(self.stack.pop().expect("Trying to pop from empty stack"));
    }

    #[inline(always)]
    fn execute_store_from_index(&mut self, index: usize) {
        self.result = Some(self.stack[index]);
    }

    #[inline(always)]
    fn execute_store_copy(&mut self) {
        self.result = Some(*self.stack.last().expect("Trying to get from empty stack"));
    }

    #[inline(always)]
    fn execute_pop_stack(&mut self) {
        let _ = self.stack.pop().expect("Trying to pop from empty stack");
    }

    #[inline(always)]
    fn execute_push_reference(&mut self) {
        self.reference_stack.push(self.reference.take().unwrap());
    }

    #[inline(always)]
    fn execute_pop_reference(&mut self) {
        self.reference = Some(self.reference_stack.pop().unwrap());
    }

    #[inline(always)]
    fn execute_swap(&mut self) {
        let a = self.stack.pop().unwrap();
        let b = self.stack.pop().unwrap();
        self.stack.push(a);
        self.stack.push(b);
    }

    #[inline(always)]
    fn execute_empty(&mut self) {
        self.result = None;
    }

    #[inline(always)]
    fn execute_is_undefined(&mut self) {
        let val = self.result.take().unwrap();
        let result = val.is_undefined();
        self.result = Some(result.into());
    }

    #[inline(always)]
    fn execute_is_null(&mut self) {
        let val = self.result.take().unwrap();
        let result = val.is_null();
        self.result = Some(result.into());
    }

    #[inline(always)]
    fn execute_is_object(&mut self) {
        let val = self.result.take().unwrap();
        let result = val.is_object();
        self.result = Some(result.into());
    }

    #[inline(always)]
    fn execute_pop_exception_jump_target(&mut self) {
        self.exception_handler_stack.pop().unwrap();
    }

    #[inline(always)]
    fn execute_throw<'gc>(&mut self, gc: NoGcScope<'gc, '_>) -> JsResult<'gc, ()> {
        let result = self.result.take().unwrap();
        Err(JsError::new(result).bind(gc))
    }
}

fn concat_string_from_slice<'a, 'gc>(
    agent: &mut Agent,
    strings: impl IntoIterator<Item = String<'a>>,
    string_length: usize,
    gc: NoGcScope<'gc, '_>,
) -> String<'gc> {
    let mut result_string = Wtf8Buf::with_capacity(string_length);
    for string in strings {
        result_string.push_wtf8(string.as_wtf8_(agent));
    }
    String::from_wtf8_buf(agent, result_string, gc)
}

/// ### [13.15.3 ApplyStringOrNumericBinaryOperator ( lval, opText, rval )](https://tc39.es/ecma262/#sec-applystringornumericbinaryoperator)
///
/// The abstract operation ApplyStringOrNumericBinaryOperator takes
/// arguments lval (an ECMAScript language value), opText (**, *, /, %,
/// -, <<, >>, >>>, &, ^, or |), and rval (an ECMAScript language value) and
/// returns either a normal completion containing either a String, a BigInt,
/// or a Number, or a throw completion.
fn apply_string_or_numeric_binary_operator<'gc>(
    agent: &mut Agent,
    lval: Value,
    op_text: Instruction,
    rval: Value,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    let lval = lval.bind(gc.nogc());
    let rval = rval.bind(gc.nogc());
    // 1. If opText is +, then
    let rval = rval.scope(agent, gc.nogc());
    // 3. Let lnum be ? ToNumeric(lval).
    let lnum = to_numeric(agent, lval.unbind(), gc.reborrow())
        .unbind()?
        .scope(agent, gc.nogc());
    // 4. Let rnum be ? ToNumeric(rval).
    let rnum = to_numeric(agent, rval.take(agent), gc.reborrow()).unbind()?;
    let gc = gc.into_nogc();
    let rnum = rnum.bind(gc);
    let lnum = lnum.take(agent).bind(gc);

    // 6. If lnum is a BigInt, then
    if let (Ok(lnum), Ok(rnum)) = (BigInt::try_from(lnum), BigInt::try_from(rnum)) {
        bigint_binary_operator(agent, op_text, lnum, rnum, gc).map(|v| v.into())
    } else if let (Ok(lnum), Ok(rnum)) = (Number::try_from(lnum), Number::try_from(rnum)) {
        number_binary_operator(agent, op_text, lnum, rnum, gc).map(|v| v.into())
    } else {
        // 5. If Type(lnum) is not Type(rnum), throw a TypeError exception.
        Err(agent.throw_exception_with_static_message(
            ExceptionType::TypeError,
            "The left and right-hand sides do not have the same type.",
            gc,
        ))
    }
}

/// ### [13.15.3 ApplyStringOrNumericBinaryOperator ( lval, opText, rval )](https://tc39.es/ecma262/#sec-applystringornumericbinaryoperator)
///
/// The abstract operation ApplyStringOrNumericBinaryOperator takes
/// arguments lval (an ECMAScript language value), opText (+), and rval (an
/// ECMAScript language value) and returns either a normal completion
/// containing either a String, a BigInt, or a Number, or a throw completion.
fn apply_string_or_numeric_addition<'gc>(
    agent: &mut Agent,
    lval: Value,
    rval: Value,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    let lval = lval.bind(gc.nogc());
    let rval = rval.bind(gc.nogc());
    // 1. If opText is +, then
    // a. Let lprim be ? ToPrimitive(lval).
    // b. Let rprim be ? ToPrimitive(rval).
    let (lprim, rprim, gc) = match (Primitive::try_from(lval), Primitive::try_from(rval)) {
        (Ok(lprim), Ok(rprim)) => {
            let lprim = lprim.unbind();
            let rprim = rprim.unbind();
            let gc = gc.into_nogc();
            (lprim.bind(gc), rprim.bind(gc), gc)
        }
        (Ok(lprim), Err(_)) => {
            let lprim = lprim.scope(agent, gc.nogc());
            let rprim = to_primitive(agent, rval.unbind(), None, gc.reborrow()).unbind()?;
            let gc = gc.into_nogc();
            let lprim = lprim.take(agent);
            (lprim.bind(gc), rprim.bind(gc), gc)
        }
        (Err(_), Ok(rprim)) => {
            let rprim = rprim.scope(agent, gc.nogc());
            let lprim = to_primitive(agent, lval.unbind(), None, gc.reborrow()).unbind()?;
            let gc = gc.into_nogc();
            let rprim = rprim.take(agent);
            (lprim.bind(gc), rprim.bind(gc), gc)
        }
        (Err(_), Err(_)) => {
            let rval = rval.scope(agent, gc.nogc());
            let lprim = to_primitive(agent, lval.unbind(), None, gc.reborrow())
                .unbind()?
                .scope(agent, gc.nogc());
            // SAFETY: not shared.
            let rprim = to_primitive(agent, rval.take(agent), None, gc.reborrow()).unbind()?;
            let gc = gc.into_nogc();
            let lprim = lprim.take(agent);
            (lprim.bind(gc), rprim.bind(gc), gc)
        }
    };

    // c. If lprim is a String or rprim is a String, then
    match (String::try_from(lprim), String::try_from(rprim)) {
        (Ok(lstr), Ok(rstr)) => {
            // iii. Return the string-concatenation of lstr and rstr.
            agent.check_concat_length(lstr, rstr, gc)?;
            return Ok(String::concat(agent, [lstr, rstr], gc).into());
        }
        (Ok(lstr), Err(_)) => {
            let lstr = lstr.scope(agent, gc);
            // ii. Let rstr be ? ToString(rprim).
            let rstr = to_string_primitive(agent, rprim, gc)?;
            // iii. Return the string-concatenation of lstr and rstr.
            let lstr = lstr.get(agent).bind(gc);
            agent.check_concat_length(lstr, rstr, gc)?;
            return Ok(String::concat(agent, [lstr, rstr], gc).into());
        }
        (Err(_), Ok(rstr)) => {
            let rstr = rstr.scope(agent, gc);
            // i. Let lstr be ? ToString(lprim).
            let lstr = to_string_primitive(agent, lprim, gc)?;
            // iii. Return the string-concatenation of lstr and rstr.
            let rstr = rstr.get(agent).bind(gc);
            agent.check_concat_length(lstr, rstr, gc)?;
            return Ok(String::concat(agent, [lstr, rstr], gc).into());
        }
        (Err(_), Err(_)) => {}
    }

    // d. Set lval to lprim.
    // e. Set rval to rprim.
    // 2. NOTE: At this point, it must be a numeric operation.
    // 3. Let lnum be ? ToNumeric(lval).
    let lnum = to_numeric_primitive(agent, lprim, gc)?;
    // 4. Let rnum be ? ToNumeric(rval).
    let rnum = to_numeric_primitive(agent, rprim, gc)?;

    // 6. If lnum is a BigInt, then
    if let (Ok(lnum), Ok(rnum)) = (BigInt::try_from(lnum), BigInt::try_from(rnum)) {
        Ok(BigInt::add(agent, lnum, rnum).check_size(agent, gc)?.into())
    } else if let (Ok(lnum), Ok(rnum)) = (Number::try_from(lnum), Number::try_from(rnum)) {
        Ok(Number::add(agent, lnum, rnum).into())
    } else {
        // 5. If Type(lnum) is not Type(rnum), throw a TypeError exception.
        Err(agent.throw_exception_with_static_message(
            ExceptionType::TypeError,
            "The left and right-hand sides do not have the same type.",
            gc,
        ))
    }
}

fn bigint_binary_operator<'a>(
    agent: &mut Agent,
    op_text: Instruction,
    lnum: BigInt<'a>,
    rnum: BigInt<'a>,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, BigInt<'a>> {
    if matches!(
        op_text,
        Instruction::ApplyMultiplicationBinaryOperator
            | Instruction::ApplyDivisionBinaryOperator
            | Instruction::ApplyRemainderBinaryOperator
    ) {
        let steps = crate::ecmascript::bigint_work_steps(
            lnum.magnitude_bits(agent),
            rnum.magnitude_bits(agent),
        );
        agent.charge_steps(steps);
    }
    match op_text {
        // a. If opText is **, return ? BigInt::exponentiate(lnum, rnum).
        Instruction::ApplyExponentialBinaryOperator => BigInt::exponentiate(agent, lnum, rnum, gc),
        // b. If opText is /, return ? BigInt::divide(lnum, rnum).
        Instruction::ApplyDivisionBinaryOperator => BigInt::divide(agent, lnum, rnum, gc),
        // c. If opText is %, return ? BigInt::remainder(lnum, rnum).
        Instruction::ApplyRemainderBinaryOperator => BigInt::remainder(agent, lnum, rnum, gc),
        // d. If opText is >>>, return ? BigInt::unsignedRightShift(lnum, rnum).
        Instruction::ApplyShiftRightZeroFillBinaryOperator => {
            BigInt::unsigned_right_shift(agent, lnum, rnum, gc)
        }
        // <<	BigInt	BigInt::leftShift
        Instruction::ApplyShiftLeftBinaryOperator => BigInt::left_shift(agent, lnum, rnum, gc),
        // >>	BigInt	BigInt::signedRightShift
        Instruction::ApplyShiftRightBinaryOperator => {
            BigInt::signed_right_shift(agent, lnum, rnum, gc)
        }
        // -	BigInt	BigInt::subtract
        Instruction::ApplySubtractionBinaryOperator => {
            BigInt::subtract(agent, lnum, rnum).check_size(agent, gc)
        }
        // *	BigInt	BigInt::multiply
        Instruction::ApplyMultiplicationBinaryOperator => {
            BigInt::multiply(agent, lnum, rnum).check_size(agent, gc)
        }
        // |	BigInt	BigInt::bitwiseOR
        Instruction::ApplyBitwiseORBinaryOperator => {
            BigInt::bitwise_or(agent, lnum, rnum).check_size(agent, gc)
        }
        // ^	BigInt	BigInt::bitwiseXOR
        Instruction::ApplyBitwiseXORBinaryOperator => {
            BigInt::bitwise_xor(agent, lnum, rnum).check_size(agent, gc)
        }
        // &	BigInt	BigInt::bitwiseAND
        Instruction::ApplyBitwiseAndBinaryOperator => {
            BigInt::bitwise_and(agent, lnum, rnum).check_size(agent, gc)
        }
        _ => unreachable!(),
    }
}

fn number_binary_operator<'a>(
    agent: &mut Agent,
    op_text: Instruction,
    lnum: Number<'a>,
    rnum: Number<'a>,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, Number<'a>> {
    // 7. Let operation be the abstract operation associated with opText and
    // Type(lnum) in the following table:
    // 8. Return operation(lnum, rnum).
    // NOTE: We do step 8. explicitly in branch.
    Ok(match op_text {
        // opText	Type(lnum)	operation
        // **	Number	Number::exponentiate
        Instruction::ApplyExponentialBinaryOperator => Number::exponentiate(agent, lnum, rnum),
        // *	Number	Number::multiply
        Instruction::ApplyMultiplicationBinaryOperator => Number::multiply(agent, lnum, rnum, gc),
        // /	Number	Number::divide
        Instruction::ApplyDivisionBinaryOperator => Number::divide(agent, lnum, rnum, gc),
        // %	Number	Number::remainder
        Instruction::ApplyRemainderBinaryOperator => Number::remainder(agent, lnum, rnum, gc),
        // -	Number	Number::subtract
        Instruction::ApplySubtractionBinaryOperator => Number::subtract(agent, lnum, rnum),
        // <<	Number	Number::leftShift
        Instruction::ApplyShiftLeftBinaryOperator => Number::left_shift(agent, lnum, rnum),
        // >>	Number	Number::signedRightShift
        Instruction::ApplyShiftRightBinaryOperator => Number::signed_right_shift(agent, lnum, rnum),
        // >>>	Number	Number::unsignedRightShift
        Instruction::ApplyShiftRightZeroFillBinaryOperator => {
            Number::unsigned_right_shift(agent, lnum, rnum)
        }
        // |	Number	Number::bitwiseOR
        Instruction::ApplyBitwiseORBinaryOperator => Number::bitwise_or(agent, lnum, rnum).into(),
        // ^	Number	Number::bitwiseXOR
        Instruction::ApplyBitwiseXORBinaryOperator => Number::bitwise_xor(agent, lnum, rnum).into(),
        // &	Number	Number::bitwiseAND
        Instruction::ApplyBitwiseAndBinaryOperator => Number::bitwise_and(agent, lnum, rnum).into(),
        _ => unreachable!(),
    })
}

/// ### [13.5.3 The typeof operator](https://tc39.es/ecma262/#sec-typeof-operator)
#[inline]
pub(crate) fn typeof_operator(agent: &Agent, val: Value, gc: NoGcScope) -> String<'static> {
    match val {
        // 4. If val is undefined, return "undefined".
        Value::Undefined => BUILTIN_STRING_MEMORY.undefined,
        // 8. If val is a Boolean, return "boolean".
        Value::Boolean(_) => BUILTIN_STRING_MEMORY.boolean,
        // 6. If val is a String, return "string".
        Value::String(_) |
        Value::SmallString(_) => BUILTIN_STRING_MEMORY.string,
        // 7. If val is a Symbol, return "symbol".
        Value::Symbol(_) => BUILTIN_STRING_MEMORY.symbol,
        // 9. If val is a Number, return "number".
        Value::Number(_) |
        Value::Integer(_) |
        Value::SmallF64(_) => BUILTIN_STRING_MEMORY.number,
        // 10. If val is a BigInt, return "bigint".
        Value::BigInt(_) |
        Value::SmallBigInt(_) => BUILTIN_STRING_MEMORY.bigint,
        // 5. If val is null, return "object".
        Value::Null |
        // 11. Assert: val is an Object.
        // 12. NOTE: This step is replaced in section B.3.6.3.
        Value::Object(_)  |
        Value::Array(_)  |
        Value::Error(_)  |
        // 14. Return "object".
        Value::PrimitiveObject(_) |
        Value::Arguments(_) |
        Value::FinalizationRegistry(_) |
        Value::Map(_) |
        Value::Promise(_) |
        Value::AsyncGenerator(_) |
        Value::ArrayIterator(_) |
        Value::MapIterator(_) |
        Value::StringIterator(_) |
        Value::Generator(_) |
        Value::Module(_) |
        Value::EmbedderObject(_) => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "regexp")]
        Value::RegExp(_) | Value::RegExpStringIterator(_) => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "weak-refs")]
        Value::WeakMap(_) |
        Value::WeakRef(_) |
        Value::WeakSet(_)  => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "set")]
        Value::Set(_) |
        Value::SetIterator(_) => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "array-buffer")]
        Value::ArrayBuffer(_) |
        Value::Int8Array(_) |
        Value::Uint8Array(_) |
        Value::Uint8ClampedArray(_) |
        Value::Int16Array(_) |
        Value::Uint16Array(_) |
        Value::Int32Array(_) |
        Value::Uint32Array(_) |
        Value::BigInt64Array(_) |
        Value::BigUint64Array(_) |
        Value::Float32Array(_) |
        Value::Float64Array(_) |
        Value::DataView(_) => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "proposal-float16array")]
        Value::Float16Array(_) => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "shared-array-buffer")]
        Value::SharedArrayBuffer(_) |
        Value::SharedInt8Array(_) |
        Value::SharedUint8Array(_) |
        Value::SharedUint8ClampedArray(_) |
        Value::SharedInt16Array(_) |
        Value::SharedUint16Array(_) |
        Value::SharedInt32Array(_) |
        Value::SharedUint32Array(_) |
        Value::SharedBigInt64Array(_) |
        Value::SharedBigUint64Array(_) |
        Value::SharedFloat32Array(_) |
        Value::SharedFloat64Array(_) |
        Value::SharedDataView(_) => BUILTIN_STRING_MEMORY.object,
        #[cfg(all(feature = "proposal-float16array", feature = "shared-array-buffer"))]
        Value::SharedFloat16Array(_) => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "date")]
        Value::Date(_)  => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "temporal")]
        Value::Instant(_)  => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "temporal")]
        Value::Duration(_)  => BUILTIN_STRING_MEMORY.object,
        #[cfg(feature = "temporal")]
        Value::PlainTime(_)  => BUILTIN_STRING_MEMORY.object,
        // 13. If val has a [[Call]] internal slot, return "function".
        Value::BoundFunction(_) | Value::BuiltinFunction(_) | Value::ECMAScriptFunction(_) |
        Value::BuiltinConstructorFunction(_) |
        Value::BuiltinPromiseResolvingFunction(_) |
        Value::BuiltinPromiseFinallyFunction(_) |
        Value::BuiltinProxyRevokerFunction => BUILTIN_STRING_MEMORY.function,
        Value::Proxy(proxy) => {
            if proxy.is_callable(agent, gc) {
                BUILTIN_STRING_MEMORY.function
            } else {
                BUILTIN_STRING_MEMORY.object
            }
        },
    }
}

/// ### [13.10.2 InstanceofOperator ( V, target )](https://tc39.es/ecma262/#sec-instanceofoperator)
///
/// The abstract operation InstanceofOperator takes arguments V (an ECMAScript
/// language value) and target (an ECMAScript language value) and returns
/// either a normal completion containing a Boolean or a throw completion. It
/// implements the generic algorithm for determining if V is an instance of
/// target either by consulting target's @@hasInstance method or, if absent,
/// determining whether the value of target's "prototype" property is present
/// in V's prototype chain.
///
/// > #### Note
/// > Steps 4 and 5 provide compatibility with previous editions of ECMAScript
/// > that did not use a @@hasInstance method to define the instanceof operator
/// > semantics. If an object does not define or inherit @@hasInstance it uses
/// > the default instanceof semantics.
pub(crate) fn instanceof_operator<'a, 'b>(
    agent: &mut Agent,
    value: impl Into<Value<'b>>,
    target: impl Into<Value<'b>>,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, bool> {
    let mut value = value.into().bind(gc.nogc());
    let target = target.into().bind(gc.nogc());
    // 1. If target is not an Object, throw a TypeError exception.
    let Ok(mut target) = Object::try_from(target) else {
        let error_message = format!(
            "Invalid instanceof target {}.",
            target
                .unbind()
                .string_repr(agent, gc.reborrow())
                .to_string_lossy_(agent)
        );
        return Err(agent.throw_exception(ExceptionType::TypeError, error_message, gc.into_nogc()));
    };
    // 2. Let instOfHandler be ? GetMethod(target, @@hasInstance).
    let inst_of_handler = if let Some(handler) = try_result_into_option_js(try_get_object_method(
        agent,
        target,
        WellKnownSymbols::HasInstance.into(),
        gc.nogc(),
    )) {
        handler.unbind()?.bind(gc.nogc())
    } else {
        let scoped_value = value.scope(agent, gc.nogc());
        let scoped_target = target.scope(agent, gc.nogc());
        let inst_of_handler = get_method(
            agent,
            target.unbind().into(),
            WellKnownSymbols::HasInstance.into(),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        value = scoped_value.take(agent).bind(gc.nogc());
        target = scoped_target.take(agent).bind(gc.nogc());
        inst_of_handler
    };
    // 3. If instOfHandler is not undefined, then
    if let Some(inst_of_handler) = inst_of_handler {
        // a. Return ToBoolean(? Call(instOfHandler, target, « V »)).
        let result = call_function(
            agent,
            inst_of_handler.unbind(),
            target.unbind().into(),
            Some(ArgumentsList::from_mut_slice(&mut [value.unbind()])),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        Ok(to_boolean(agent, result))
    } else {
        // 4. If IsCallable(target) is false, throw a TypeError exception.
        let Some(target) = is_callable(target, gc.nogc()) else {
            let error_message = format!(
                "Invalid instanceof target {} is not a function.",
                Value::from(target)
                    .unbind()
                    .string_repr(agent, gc.reborrow())
                    .to_string_lossy_(agent)
            );
            return Err(agent.throw_exception(
                ExceptionType::TypeError,
                error_message,
                gc.into_nogc(),
            ));
        };
        // 5. Return ? OrdinaryHasInstance(target, V).
        Ok(ordinary_has_instance(
            agent,
            target.unbind(),
            value.unbind(),
            gc,
        )?)
    }
}

#[inline(always)]
fn with_vm_gc<'a, 'b, R: 'a>(
    agent: &mut Agent,
    vm: &mut Vm,
    work: impl FnOnce(&mut Agent, GcScope<'a, 'b>) -> R,
    gc: GcScope<'a, 'b>,
) -> R {
    let depth = agent.vm_stack.len();
    agent.vm_stack.push(core::mem::replace(vm, Vm::EMPTY));
    let result = work(agent, gc);
    assert_eq!(agent.vm_stack.len(), depth + 1, "VM Stack was misused");
    *vm = agent.vm_stack.pop().unwrap();
    result
}

impl HeapMarkAndSweep for ExceptionHandler<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        match self {
            ExceptionHandler::CatchBlock {
                ip: _,
                lexical_environment,
                variable_environment,
            } => {
                lexical_environment.mark_values(queues);
                variable_environment.mark_values(queues);
            }
            ExceptionHandler::IgnoreErrorAndNextInstruction => {}
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        match self {
            ExceptionHandler::CatchBlock {
                ip: _,
                lexical_environment,
                variable_environment,
            } => {
                lexical_environment.sweep_values(compactions);
                variable_environment.sweep_values(compactions);
            }
            ExceptionHandler::IgnoreErrorAndNextInstruction => {}
        }
    }
}

impl HeapMarkAndSweep for Vm {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Vm {
            ip: _,
            stack,
            reference_stack,
            iterator_stack,
            exception_handler_stack: exception_jump_target_stack,
            result,
            reference,
        } = self;
        stack.as_slice().mark_values(queues);
        reference_stack.as_slice().mark_values(queues);
        iterator_stack.as_slice().mark_values(queues);
        exception_jump_target_stack.as_slice().mark_values(queues);
        result.mark_values(queues);
        reference.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Vm {
            ip: _,
            stack,
            reference_stack,
            iterator_stack,
            exception_handler_stack: exception_jump_target_stack,
            result,
            reference,
        } = self;
        stack.as_mut_slice().sweep_values(compactions);
        reference_stack.as_mut_slice().sweep_values(compactions);
        iterator_stack.as_mut_slice().sweep_values(compactions);
        exception_jump_target_stack
            .as_mut_slice()
            .sweep_values(compactions);
        result.sweep_values(compactions);
        reference.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for SuspendedVm {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            ip: _,
            stack,
            reference_stack,
            iterator_stack,
            exception_jump_target_stack,
        } = self;
        stack.mark_values(queues);
        reference_stack.mark_values(queues);
        iterator_stack.mark_values(queues);
        exception_jump_target_stack.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            ip: _,
            stack,
            reference_stack,
            iterator_stack,
            exception_jump_target_stack,
        } = self;
        stack.sweep_values(compactions);
        reference_stack.sweep_values(compactions);
        iterator_stack.sweep_values(compactions);
        exception_jump_target_stack.sweep_values(compactions);
    }
}

/// SetFunctionName version for class constructors, only calculates the correct
/// function name and returns it.
fn set_class_name<'a>(
    agent: &mut Agent,
    vm: &mut Vm,
    name: Value,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, String<'a>> {
    if let Ok(name) = String::try_from(name) {
        Ok(name.bind(gc.into_nogc()))
    } else if let Value::Symbol(name) = name {
        // OPTIMISATION: Specification wise, we should go to the
        // below else-branch and perform ToPropertyKey, but that'd
        // just return our Symbol at the cost of some scoping.
        // Symbols are the most likely non-String value here, so
        // we'll check them separately first.
        Ok(name
            .unbind()
            .get_symbol_function_name(agent, gc.into_nogc()))
    } else {
        // ## 13.2.5.5 Runtime Semantics: PropertyDefinitionEvaluation
        // ### PropertyDefinition : PropertyName : AssignmentExpression
        // 1. Let propKey be ? Evaluation of PropertyName.
        // 3. Return ? ToPropertyKey(propName).
        let prop_key = {
            let name = name.unbind();
            with_vm_gc(
                agent,
                vm,
                |agent, gc| to_property_key(agent, name, gc),
                gc.reborrow(),
            )
            .unbind()?
            .bind(gc.nogc())
        };

        let name = prop_key.convert_to_value(agent, gc.nogc());
        set_class_name(agent, vm, name.unbind().into(), gc)
    }
}

fn verify_is_object<'a>(
    agent: &mut Agent,
    value: Value,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, ()> {
    if !value.is_object() {
        let message =
            String::from_static_str(agent, "iterator.return() returned a non-object value", gc);
        Err(agent.throw_exception_with_message(ExceptionType::TypeError, message.unbind(), gc))
    } else {
        Ok(())
    }
}

fn throw_error_in_target_not_object<'a>(
    agent: &mut Agent,
    value: Value,
    gc: NoGcScope<'a, '_>,
) -> JsError<'a> {
    let error_message = format!(
        "right-hand side of 'in' should be an object, got {}.",
        typeof_operator(agent, value, gc).to_string_lossy_(agent)
    );
    agent.throw_exception(ExceptionType::TypeError, error_message, gc)
}
