// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod binding_methods;
mod execute_instructions;

use execute_instructions::*;

use std::sync::Arc;

use wtf8::Wtf8Buf;

#[cfg(feature = "array-buffer")]
use crate::ecmascript::{AnyTypedArray, TypedArrayAbstractOperations, TypedArrayPrototype};
use crate::{
    ecmascript::{
        Agent, ArgumentsList, BUILTIN_STRING_MEMORY, BigInt, ECMAScriptFunction, Environment,
        ExceptionType, InternalMethods, InternalSlots, JsError, JsResult, Number, Object,
        OrdinaryObject, Primitive, Promise, PropertyKey, Reference, SmallInteger, String,
        TryGetResult, Value, binding_value_at_site, call_function, get_method, get_this_value,
        is_callable, is_property_reference, is_same_type, is_strictly_equal,
        ordinary_add_own_data_by_cache, ordinary_has_instance, ordinary_set_own_data_at_offset,
        prepare_for_call_without_function_environment, reference_is_at_global_site,
        set_binding_value_at_site, take_pooled_arguments_vec, to_boolean, to_numeric,
        to_numeric_primitive, to_primitive, to_property_key, to_string_primitive,
        try_get_object_method, try_resolve_binding_at_site, try_result_into_option_js,
    },
    engine::{
        Bindable, GcScope, NoGcScope, Scopable, Scoped,
        bytecode::{
            CallEnvironment, Executable, IndexType, Instruction, InstructionIter,
            instructions::Instr, iterator::VmIteratorRecord,
        },
    },
    heap::{ArenaAccess, CompactionLists, HeapMarkAndSweep, WellKnownSymbols, WorkQueues},
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

/// What [`Vm::execute_plain_instruction`] made of an instruction.
enum Plain {
    /// Executed.
    Done,
    /// Executed, and it jumped backwards: one turn of a loop.
    Looped,
    /// Not executed and nothing changed: the instruction is not a plain one,
    /// or its operands are not what the short way takes.
    No,
    /// Not executed and nothing changed: a call of an ECMAScript function,
    /// which the Vm may run itself.
    Call,
}

/// The steps a backward jump or a call counts for in the interrupt countdown:
/// the instructions on the way to it are not counted one by one.
const STEPS_PER_TURN: u32 = 8;

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
    /// How many functions this `Vm` is running in place of their calls, one
    /// called by the other: the state of the code that made each call is in
    /// a frame of [`VmFrames`], the last `depth` of those in use. See
    /// [`Vm::enter_function`].
    depth: usize,
}

/// What a `Vm` puts aside of the code that calls a function it runs itself,
/// and takes back when the function returns or throws: the caller's stacks,
/// where it goes on, and its code.
#[derive(Debug)]
struct Frame {
    ip: usize,
    /// The caller's stacks while the frame is in use; empty stacks for the
    /// next function at this depth while it is not.
    stack: Vec<Value<'static>>,
    reference_stack: Vec<Reference<'static>>,
    iterator_stack: Vec<VmIteratorRecord<'static>>,
    exception_handler_stack: Vec<ExceptionHandler<'static>>,
    /// The caller's executable, which the collector keeps current here
    /// while the frame is in use.
    executable: Executable<'static>,
    instructions: Option<Arc<[u8]>>,
    /// What the Agent's scoped values and scoped collections are cut back
    /// to after each of the caller's instructions.
    stack_depth: usize,
    collections_depth: usize,
    /// Where the scoped values of the caller's own call start: its
    /// executable is the first of them. Not used for the code the `Vm` was
    /// started with.
    scope_base: usize,
}

/// The code that is running in a `Vm`, between two changes of it: the code
/// the `Vm` was started with, or a function that a call of it, or of a
/// function it called, runs in place of the call.
struct Running<'s> {
    executable: Scoped<'s, Executable<'static>>,
    instructions: Arc<[u8]>,
    /// What the Agent's scoped values and scoped collections are cut back
    /// to after each instruction.
    stack_depth: usize,
    collections_depth: usize,
    /// Where the scoped values of the call of this code start: see
    /// [`Frame::scope_base`].
    scope_base: usize,
}

/// Why the loop of instructions stops with the `Vm` not done.
enum Change {
    /// A call of a function that the `Vm` runs itself.
    Call(CalledInPlace, Instr),
    /// The function that is run in place of a call returns.
    Return,
    /// The code has no more instructions.
    End,
    /// An error that the running code did not handle.
    Throw(JsError<'static>),
    /// A function run in place of a call returned with a termination
    /// pending: every caller ends too.
    Terminate,
}

/// A function that a call instruction calls and the `Vm` runs itself.
#[derive(Clone, Copy)]
struct CalledInPlace {
    function: ECMAScriptFunction<'static>,
    executable: Executable<'static>,
    /// How many parameters the function keeps in its first stack slots.
    parameters: usize,
    /// The function's bindings need a Declarative Environment Record.
    declarative_environment: bool,
}

#[derive(Debug)]
pub(crate) struct SuspendedVm {
    ip: usize,
    /// The stacks are kept as the vectors they were, with at most twice the
    /// room they use (and room for a few values at least): cut to what they
    /// hold, each `yield` of a generator and each `await` allocated them
    /// anew and grew them again on the way back.
    ///
    /// Note: Stack is empty only if the code contains no local variables
    /// optimised into stack slots or temporarily stored Values.
    stack: Vec<Value<'static>>,
    /// Note: Reference stack is non-empty only if the code awaits inside a call
    /// expression.
    reference_stack: Vec<Reference<'static>>,
    /// Note: Iterator stack is non-empty only if the code awaits inside a
    /// for-in or for-of loop.
    iterator_stack: Vec<VmIteratorRecord<'static>>,
    /// Note: Exception jump stack is non-empty only if the code awaits inside a
    /// try block or an await for-of loop.
    exception_jump_target_stack: Vec<ExceptionHandler<'static>>,
}

/// Room a suspended stack keeps beyond what it holds: as much again, and
/// never less than this many entries in all.
const SUSPENDED_STACK_ROOM: usize = 8;

/// Cuts a stack that is being suspended down to twice what it holds.
fn fit_suspended_stack<T>(mut stack: Vec<T>) -> Vec<T> {
    let room = (stack.len() * 2).max(SUSPENDED_STACK_ROOM);
    if stack.is_empty() {
        // Nothing to keep: no allocation either.
        return Vec::new();
    }
    if stack.capacity() > room + SUSPENDED_STACK_ROOM {
        stack.shrink_to(room);
    }
    stack
}

impl SuspendedVm {
    /// Bytes held by the suspended stacks, for the heap size estimate.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.stack.capacity() * core::mem::size_of::<Value<'static>>()
            + self.reference_stack.capacity() * core::mem::size_of::<Reference<'static>>()
            + self.iterator_stack.capacity() * core::mem::size_of::<VmIteratorRecord<'static>>()
            + self.exception_jump_target_stack.capacity()
                * core::mem::size_of::<ExceptionHandler<'static>>()
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
/// How many frames, with their stacks, stay with an Agent while none is in
/// use.
const SPARE_FRAMES: usize = 64;
const VM_STACK_POOL_MAX_CAPACITY: usize = 1024;

/// The value and iterator stacks of a `Vm` that has run its code, emptied
/// and kept for the next call.
pub(crate) struct PooledVmStacks {
    stack: Vec<Value<'static>>,
    iterator_stack: Vec<VmIteratorRecord<'static>>,
}

/// Emptied Reference and exception handler stacks of `Vm`s that have run
/// their code. A `Vm` starts without either and takes one here when it
/// first pushes to it: most calls push to neither, and one that does (a
/// method that assigns to `this.x`, a function with a loop) would allocate
/// the stack and free it again. Kept apart from [`PooledVmStacks`], which
/// every call takes and gives back.
#[derive(Default)]
pub(crate) struct SpareVmStacks {
    references: Vec<Vec<Reference<'static>>>,
    handlers: Vec<Vec<ExceptionHandler<'static>>>,
}

/// The frames of every `Vm` of an Agent that runs functions in place of
/// their calls: those of the `Vm` that is running are the last in use,
/// under them those of the `Vm`s that wait for a call it is inside of. A
/// `Vm` leaves none in use when it is done or suspended.
///
/// The frames after those in use are kept: each holds the emptied stacks
/// of the last function that ran at its depth, for the next one.
#[derive(Default)]
pub(crate) struct VmFrames {
    frames: Vec<Frame>,
    in_use: usize,
}

impl HeapMarkAndSweep for VmFrames {
    fn mark_values(&self, queues: &mut WorkQueues) {
        for frame in &self.frames[..self.in_use] {
            let Frame {
                ip: _,
                stack,
                reference_stack,
                iterator_stack,
                exception_handler_stack,
                executable,
                instructions: _,
                stack_depth: _,
                collections_depth: _,
                scope_base: _,
            } = frame;
            stack.as_slice().mark_values(queues);
            reference_stack.as_slice().mark_values(queues);
            iterator_stack.as_slice().mark_values(queues);
            exception_handler_stack.as_slice().mark_values(queues);
            executable.mark_values(queues);
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        for frame in &mut self.frames[..self.in_use] {
            let Frame {
                ip: _,
                stack,
                reference_stack,
                iterator_stack,
                exception_handler_stack,
                executable,
                instructions: _,
                stack_depth: _,
                collections_depth: _,
                scope_base: _,
            } = frame;
            stack.as_mut_slice().sweep_values(compactions);
            reference_stack.as_mut_slice().sweep_values(compactions);
            iterator_stack.as_mut_slice().sweep_values(compactions);
            exception_handler_stack
                .as_mut_slice()
                .sweep_values(compactions);
            executable.sweep_values(compactions);
        }
    }
}

impl Vm {
    const EMPTY: Self = Self {
        ip: 0,
        stack: Vec::new(),
        reference_stack: Vec::new(),
        iterator_stack: Vec::new(),
        exception_handler_stack: Vec::new(),
        result: None,
        reference: None,
        depth: 0,
    };

    /// The value and iterator stacks for code that is about to run: emptied
    /// ones that other code left, or new ones.
    #[inline]
    fn take_stacks(agent: &mut Agent) -> PooledVmStacks {
        agent.vm_stack_pool.pop().unwrap_or_else(|| PooledVmStacks {
            stack: Vec::with_capacity(32),
            iterator_stack: Vec::new(),
        })
    }

    fn new(agent: &mut Agent) -> Self {
        let PooledVmStacks {
            stack,
            iterator_stack,
        } = Self::take_stacks(agent);
        Self {
            ip: 0,
            stack,
            reference_stack: Vec::new(),
            iterator_stack,
            exception_handler_stack: Vec::new(),
            result: None,
            reference: None,
            depth: 0,
        }
    }

    fn recycle(self, agent: &mut Agent) {
        // Every part of the Vm goes somewhere: nothing is left to drop.
        let Vm {
            ip: _,
            stack,
            reference_stack,
            iterator_stack,
            exception_handler_stack,
            result: _,
            reference: _,
            depth,
        } = self;
        debug_assert_eq!(depth, 0);
        Self::recycle_stacks(
            agent,
            stack,
            reference_stack,
            iterator_stack,
            exception_handler_stack,
        );
    }

    /// Keeps the stacks of code that has run, emptied, for code that is
    /// about to: the value and iterator stacks together, the other two
    /// where they were ever pushed to.
    #[inline(always)]
    fn recycle_stacks(
        agent: &mut Agent,
        mut stack: Vec<Value<'static>>,
        mut reference_stack: Vec<Reference<'static>>,
        mut iterator_stack: Vec<VmIteratorRecord<'static>>,
        mut exception_handler_stack: Vec<ExceptionHandler<'static>>,
    ) {
        if reference_stack.capacity() != 0
            && reference_stack.capacity() <= VM_STACK_POOL_MAX_CAPACITY
            && agent.spare_vm_stacks.references.len() < VM_STACK_POOL_LEN
        {
            reference_stack.clear();
            agent.spare_vm_stacks.references.push(reference_stack);
        }
        if exception_handler_stack.capacity() != 0
            && exception_handler_stack.capacity() <= VM_STACK_POOL_MAX_CAPACITY
            && agent.spare_vm_stacks.handlers.len() < VM_STACK_POOL_LEN
        {
            exception_handler_stack.clear();
            agent.spare_vm_stacks.handlers.push(exception_handler_stack);
        }
        if stack.capacity() == 0
            || stack.capacity() > VM_STACK_POOL_MAX_CAPACITY
            || iterator_stack.capacity() > VM_STACK_POOL_MAX_CAPACITY
            || agent.vm_stack_pool.len() >= VM_STACK_POOL_LEN
        {
            return;
        }
        stack.clear();
        iterator_stack.clear();
        agent.vm_stack_pool.push(PooledVmStacks {
            stack,
            iterator_stack,
        });
    }

    fn suspend(self) -> SuspendedVm {
        let Vm {
            ip,
            stack,
            reference_stack,
            iterator_stack,
            exception_handler_stack,
            result: _,
            reference: _,
            depth,
        } = self;
        // Only the code a `Vm` was started with yields or awaits: a function
        // it runs in place of a call is neither a generator nor async.
        debug_assert_eq!(depth, 0);
        SuspendedVm {
            ip,
            stack: fit_suspended_stack(stack),
            reference_stack: fit_suspended_stack(reference_stack),
            iterator_stack: fit_suspended_stack(iterator_stack),
            exception_jump_target_stack: fit_suspended_stack(exception_handler_stack),
        }
    }

    fn from_suspended(suspended: SuspendedVm) -> Self {
        Self {
            ip: suspended.ip,
            stack: suspended.stack,
            reference_stack: suspended.reference_stack,
            iterator_stack: suspended.iterator_stack,
            exception_handler_stack: suspended.exception_jump_target_stack,
            result: None,
            reference: None,
            depth: 0,
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

        let stack_parameters = executable.stack_parameters(agent);
        let (arguments, direct) = match (arguments, stack_parameters) {
            (Some(arguments), Some(count)) => (None, Some((arguments, count))),
            (arguments, _) => (arguments, None),
        };
        if let Some((arguments, count)) = direct {
            // The code keeps its parameters in its first stack slots and
            // reads its arguments nowhere else: they go there, undefined for
            // those not given, and those beyond the parameters are dropped.
            // The caller's copies are not kept up to date through a
            // collection from here on, so none is left to be read.
            let mut given = arguments.iter_mut();
            for _ in 0..count {
                let value = given.next().map_or(Value::Undefined, |argument| {
                    core::mem::replace(argument, Value::Undefined)
                });
                vm.stack.push(value);
            }
            for argument in given {
                *argument = Value::Undefined;
            }
            if agent.options.print_internals {
                vm.print_internals(agent, executable.clone(), gc.nogc());
            }
            return vm.inner_execute(agent, executable, gc);
        }

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
        // What an instruction scoped it cannot use after it either: a loop of
        // object destructurings in one call would otherwise keep a key set
        // per iteration until the function returns, and run out of memory.
        let collections_depth = agent.stack_ref_collections.borrow().len();
        let print_internals = agent.options.print_internals;
        // The host is asked whether to interrupt, and the heap limit checked,
        // every so many calls and turns of loops: a script cannot run long
        // without either.
        if let Err(err) = agent.poll_interrupt_steps(STEPS_PER_TURN, gc.nogc()) {
            return ExecutionResult::Throw(err.unbind().bind(gc.into_nogc()));
        }
        // A collection is looked for where one can have become due: when the
        // code starts, after an instruction that is not plain (the plain ones
        // allocate nothing on the heap but the Number an operator gives,
        // and run only once between two of these places), and on every turn
        // of a loop, where the poll above can have asked for one.
        if agent.check_gc() {
            self.trigger_gc(agent, gc.reborrow());
        }
        // The code this Vm was started with. A function that one of its
        // calls runs in place of the call has its own executable, and so
        // has every function called from there: `running` is the code that
        // runs now. It changes only between two runs of the loop of
        // instructions below, which keeps what it reads of it in registers.
        let base_executable = executable;
        let mut running = Running {
            executable: base_executable.clone(),
            instructions: base_executable.get_instructions(agent),
            stack_depth,
            collections_depth,
            scope_base: stack_depth,
        };
        loop {
            let Running {
                executable,
                instructions,
                stack_depth,
                collections_depth,
                scope_base,
            } = running;
            // The instruction pointer is a local across the plain
            // instructions, which only jumps move: read and written in the
            // Vm between every two of them, it was the longest step of going
            // from one to the next. It is in the Vm, where everything else
            // looks for it, around every instruction that is not plain and
            // whenever the loop is left.
            let mut ip = self.ip;
            // The executable is looked up through its rooted handle here and
            // after every collection, which is all that moves it, not by
            // every instruction that fetches a constant, a name or a cache.
            let mut code = executable.get(agent);
            let mut collections = agent.heap.collections;
            let change = loop {
                let Some(instr) = Instr::consume_instruction(&instructions, &mut ip) else {
                    break Change::End;
                };
                if print_internals {
                    Self::print_executing(instr.kind);
                }
                // Most of what runs is plain: it goes straight to the next
                // instruction, with no result to look at and nothing to
                // release.
                match Self::execute_plain_instruction(
                    agent,
                    self,
                    &mut ip,
                    &instructions,
                    &executable,
                    code,
                    instr,
                    gc.nogc(),
                ) {
                    Plain::Done => continue,
                    Plain::Looped => {
                        self.ip = ip;
                        if let Err(err) = agent.poll_interrupt_steps(STEPS_PER_TURN, gc.nogc()) {
                            // No handler of the running code sees this one.
                            break Change::Throw(err.unbind());
                        }
                        if agent.check_gc() {
                            self.trigger_gc(agent, gc.reborrow());
                            code = executable.get(agent);
                            collections = agent.heap.collections;
                        }
                        continue;
                    }
                    // A call of a function that this Vm runs itself.
                    Plain::Call => {
                        if let Some(callee) = self.function_called_in_place(agent, instr) {
                            break Change::Call(callee, instr);
                        }
                    }
                    Plain::No => {}
                }
                self.ip = ip;
                let result = Self::execute_instruction(
                    agent,
                    self,
                    executable.clone(),
                    instr,
                    gc.reborrow(),
                );
                match result {
                    Ok(ContinuationKind::Normal) => {}
                    // The end of every call.
                    Ok(ContinuationKind::Return) if agent.termination().is_none() => {
                        if print_internals {
                            Self::print_exiting();
                        }
                        if self.depth == 0 {
                            let result = self.result.unwrap_or(Value::Undefined);
                            return ExecutionResult::Return(result.bind(gc.into_nogc()));
                        }
                        break Change::Return;
                    }
                    Err(err) => {
                        if !self.handle_error(agent, err) {
                            break Change::Throw(err.unbind());
                        }
                    }
                    Ok(kind) => {
                        if self.depth != 0 {
                            // Only the code a Vm is started with yields or
                            // awaits: a function run in place of its call
                            // returned with a termination pending.
                            break Change::Terminate;
                        }
                        if let Some(r) =
                            self.handle_execute_instruction_abnormal_result(agent, Ok(kind))
                        {
                            return r.unbind().bind(gc.into_nogc());
                        }
                    }
                }
                // A jump, or the handler of what was thrown.
                let turned = self.ip < ip;
                ip = self.ip;
                agent.stack_refs.get_mut().truncate(stack_depth);
                agent
                    .stack_ref_collections
                    .get_mut()
                    .truncate(collections_depth);
                if turned {
                    // One turn of a loop whose jump back was not a plain
                    // one (a test at its end, on operands that are not
                    // integers): the host is asked here too.
                    if let Err(err) = agent.poll_interrupt_steps(STEPS_PER_TURN, gc.nogc()) {
                        break Change::Throw(err.unbind());
                    }
                }
                if agent.check_gc() {
                    self.trigger_gc(agent, gc.reborrow());
                }
                if agent.heap.collections != collections {
                    // The instruction ran a collection, or one was due.
                    code = executable.get(agent);
                    collections = agent.heap.collections;
                }
            };
            // The running code changes: to a function called in place of
            // its call, or back to the code that made such a call, where
            // the function returns or throws.
            let mut thrown = None;
            running = Running {
                executable,
                instructions,
                stack_depth,
                collections_depth,
                scope_base,
            };
            match change {
                Change::Call(callee, instr) => {
                    running = self.enter_function(
                        agent,
                        instr,
                        callee,
                        ip,
                        code.unbind(),
                        running,
                        gc.nogc(),
                    );
                    if print_internals {
                        self.print_internals(agent, running.executable.clone(), gc.nogc());
                    }
                    // As for the code a Vm is started with: the host is
                    // asked, and a collection that is due is run.
                    if let Err(err) = agent.poll_interrupt_steps(STEPS_PER_TURN, gc.nogc()) {
                        thrown = Some(err.unbind());
                    } else if agent.check_gc() {
                        self.trigger_gc(agent, gc.reborrow());
                    }
                }
                Change::End if self.depth == 0 => {
                    self.ip = ip;
                    break;
                }
                Change::Return | Change::End => {
                    // The value of the function is the result of the call
                    // instruction: undefined at the end of its code, or
                    // where it returned none.
                    if self.result.is_none() || matches!(change, Change::End) {
                        self.result = Some(Value::Undefined);
                    }
                    drop(running);
                    running = self.leave_function(agent, &base_executable, gc.nogc());
                }
                Change::Throw(error) => thrown = Some(error),
                Change::Terminate => {
                    while self.depth != 0 {
                        drop(running);
                        running = self.leave_function(agent, &base_executable, gc.nogc());
                    }
                    return ExecutionResult::Throw(JsError::new(Value::Undefined));
                }
            }
            if let Some(error) = thrown {
                // What the running code did not handle goes to the nearest
                // handler of the code that called it in place; with none,
                // the Vm ends with the error.
                loop {
                    if self.depth == 0 {
                        if print_internals {
                            Self::print_exiting_with_error();
                        }
                        return ExecutionResult::Throw(error.bind(gc.into_nogc()));
                    }
                    drop(running);
                    running = self.leave_function(agent, &base_executable, gc.nogc());
                    if self.handle_error(agent, error) {
                        break;
                    }
                }
            }
        }

        if agent.termination().is_some() {
            return ExecutionResult::Throw(agent.termination_error(gc.into_nogc()));
        }
        ExecutionResult::Return(Value::Undefined)
    }

    /// The function that a call instruction is about to call, if this `Vm`
    /// runs it itself: an ordinary function (not a generator, not async, not
    /// a class constructor) that keeps its parameters in its first stack
    /// slots and is called without a Function Environment Record, called
    /// with a number of arguments that the instruction states, where one
    /// more call is not one too many.
    #[inline]
    fn function_called_in_place(&self, agent: &Agent, instr: Instr) -> Option<CalledInPlace> {
        let count = instr.get_first_index();
        if count == IndexType::MAX as usize {
            return None;
        }
        let callee = self.stack.len().checked_sub(count + 1)?;
        let Value::ECMAScriptFunction(function) = self.stack[callee] else {
            return None;
        };
        let data = function.get(agent);
        let kind = data.ecmascript_function.kind;
        if kind.is_generator()
            || kind.is_async()
            || data
                .ecmascript_function
                .constructor_status
                .is_class_constructor()
        {
            return None;
        }
        let executable = data.compiled_bytecode?;
        let code = executable.get(agent);
        let parameters = code.stack_parameters? as usize;
        if code.call_environment == CallEnvironment::Function || agent.call_depth_exceeded() {
            return None;
        }
        Some(CalledInPlace {
            function,
            executable,
            parameters,
            declarative_environment: code.call_environment == CallEnvironment::Declarative,
        })
    }

    /// Starts the call of `callee`, which the call instruction `instr`
    /// makes, in this `Vm`: what \[\[Call]] does up to the function's code.
    /// The calling code, which goes on at `ip`, is put aside with its
    /// stacks; the function's code finds them empty but for its parameters.
    /// Returns the function's code as the code to run.
    #[allow(clippy::too_many_arguments)]
    fn enter_function<'s>(
        &mut self,
        agent: &mut Agent,
        instr: Instr,
        callee: CalledInPlace,
        ip: usize,
        code: Executable<'static>,
        caller: Running,
        gc: NoGcScope<'_, 's>,
    ) -> Running<'s> {
        // EvaluateCall: the this value of the call, from its Reference.
        let reference = if instr.kind == Instruction::PopReferenceAndEvaluateCall {
            self.reference = None;
            self.reference_stack.pop()
        } else {
            self.reference.take()
        };
        let this_value = match reference {
            Some(reference) if is_property_reference(&reference) => {
                get_this_value(&reference).bind(gc)
            }
            Some(reference) => reference
                .base_env()
                .with_base_object(agent)
                .map_or(Value::Undefined, |object| object.into())
                .bind(gc),
            None => Value::Undefined,
        };
        // PrepareForOrdinaryCall and OrdinaryCallBindThis.
        prepare_for_call_without_function_environment(
            agent,
            callee.function,
            this_value,
            callee.declarative_environment,
            gc,
        );
        if agent.vm_frames.in_use == agent.vm_frames.frames.len() {
            let PooledVmStacks {
                stack,
                iterator_stack,
            } = Self::take_stacks(agent);
            agent.vm_frames.frames.push(Frame {
                ip: 0,
                stack,
                reference_stack: Vec::new(),
                iterator_stack,
                exception_handler_stack: Vec::new(),
                executable: code,
                instructions: None,
                stack_depth: 0,
                collections_depth: 0,
                scope_base: 0,
            });
        }
        let frame = &mut agent.vm_frames.frames[agent.vm_frames.in_use];
        // The arguments go where the function keeps its parameters:
        // undefined for those not given, and those beyond the parameters
        // are dropped.
        let first = self.stack.len() - instr.get_first_index();
        let arguments = &self.stack[first..];
        for index in 0..callee.parameters {
            frame
                .stack
                .push(arguments.get(index).copied().unwrap_or(Value::Undefined));
        }
        self.stack.truncate(first - 1);
        // The function's stacks are the Vm's, and the caller's wait in the
        // frame.
        core::mem::swap(&mut self.stack, &mut frame.stack);
        core::mem::swap(&mut self.reference_stack, &mut frame.reference_stack);
        core::mem::swap(&mut self.iterator_stack, &mut frame.iterator_stack);
        core::mem::swap(
            &mut self.exception_handler_stack,
            &mut frame.exception_handler_stack,
        );
        frame.ip = ip;
        frame.executable = code;
        frame.instructions = Some(caller.instructions);
        frame.stack_depth = caller.stack_depth;
        frame.collections_depth = caller.collections_depth;
        frame.scope_base = caller.scope_base;
        agent.vm_frames.in_use += 1;
        self.depth += 1;
        self.ip = 0;
        let stack_refs = agent.stack_refs.get_mut();
        let scope_base = stack_refs.len();
        let executable = callee.executable.scope(agent, gc);
        Running {
            executable,
            instructions: callee.executable.get_instructions(agent),
            stack_depth: agent.stack_refs.get_mut().len(),
            collections_depth: agent.stack_ref_collections.get_mut().len(),
            scope_base,
        }
    }

    /// Ends the function this `Vm` runs in place of a call, which returned
    /// or threw: its execution context is removed, and its stacks, emptied,
    /// stay in the frame for the next function, while those of the calling
    /// code are the `Vm`'s again. Returns the calling code as the code to
    /// run; `base` is the executable the `Vm` was started with.
    fn leave_function<'s>(
        &mut self,
        agent: &mut Agent,
        base: &Scoped<'s, Executable<'static>>,
        gc: NoGcScope<'_, 's>,
    ) -> Running<'s> {
        agent.remove_execution_context();
        self.depth -= 1;
        agent.vm_frames.in_use -= 1;
        let frame = &mut agent.vm_frames.frames[agent.vm_frames.in_use];
        self.stack.clear();
        self.reference_stack.clear();
        self.iterator_stack.clear();
        self.exception_handler_stack.clear();
        core::mem::swap(&mut self.stack, &mut frame.stack);
        core::mem::swap(&mut self.reference_stack, &mut frame.reference_stack);
        core::mem::swap(&mut self.iterator_stack, &mut frame.iterator_stack);
        core::mem::swap(
            &mut self.exception_handler_stack,
            &mut frame.exception_handler_stack,
        );
        if frame.stack.capacity() > VM_STACK_POOL_MAX_CAPACITY {
            // A stack that grew large is not kept for the next function.
            frame.stack = Vec::new();
        }
        self.ip = frame.ip;
        self.reference = None;
        let instructions = frame
            .instructions
            .take()
            .expect("the frame of the calling code is in use");
        let (stack_depth, collections_depth, scope_base) =
            (frame.stack_depth, frame.collections_depth, frame.scope_base);
        let caller_executable = frame.executable;
        if agent.vm_frames.in_use == 0 {
            // No frame is in use: the Agent keeps a few of them, and none
            // of a deep recursion.
            agent.vm_frames.frames.truncate(SPARE_FRAMES);
        }
        let executable = if self.depth == 0 {
            agent.stack_refs.get_mut().truncate(stack_depth);
            base.clone()
        } else {
            // The caller is itself run in place of a call: its executable
            // is scoped anew where it was.
            let executable = caller_executable;
            agent.stack_refs.get_mut().truncate(scope_base);
            let executable = executable.scope(agent, gc);
            debug_assert_eq!(agent.stack_refs.get_mut().len(), stack_depth);
            executable
        };
        agent
            .stack_ref_collections
            .get_mut()
            .truncate(collections_depth);
        Running {
            executable,
            instructions,
            stack_depth,
            collections_depth,
            scope_base,
        }
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

    /// The instructions that cannot fail, run a script or scope a value,
    /// and, of the comparisons and the arithmetic, those of two integers.
    /// The loop of `run` goes from these straight to the next instruction.
    // Inlined into the loop of `run` in optimised builds, like
    // `execute_instruction`.
    #[cfg_attr(not(debug_assertions), inline(always))]
    #[allow(clippy::too_many_arguments)]
    fn execute_plain_instruction(
        agent: &mut Agent,
        vm: &mut Vm,
        ip: &mut usize,
        instructions: &[u8],
        executable: &Scoped<Executable>,
        code: Executable,
        instr: Instr,
        gc: NoGcScope,
    ) -> Plain {
        match instr.kind {
            Instruction::Load => vm.execute_load(),
            Instruction::LoadCopy => vm.execute_load_copy(),
            Instruction::PutValueToIndex => vm.execute_load_to_index(instr.get_first_index()),
            Instruction::Store => vm.execute_store(),
            Instruction::GetValueFromIndex => vm.execute_store_from_index(instr.get_first_index()),
            Instruction::LoadFromIndex => {
                vm.execute_store_from_index(instr.get_first_index());
                vm.execute_load();
            }
            // A local and a constant, two locals: the operands of most
            // binary expressions, and the arguments of many calls.
            Instruction::LoadFromIndexStoreConstant => {
                let value = vm.stack[instr.get_first_index()];
                vm.stack.push(value);
                let constant = code.fetch_constant(agent, instr.get_second_index(), gc);
                vm.result = Some(constant.unbind());
            }
            Instruction::LoadFromIndexGetFromIndex => {
                let value = vm.stack[instr.get_first_index()];
                vm.stack.push(value);
                vm.result = Some(vm.stack[instr.get_second_index()]);
            }
            Instruction::LoadFromIndexLoadFromIndex => {
                let value = vm.stack[instr.get_first_index()];
                vm.stack.push(value);
                let value = vm.stack[instr.get_second_index()];
                vm.stack.push(value);
            }
            // An operator with a constant as its right operand: neither
            // operand goes by the stack.
            Instruction::ApplyBinaryOperatorWithConstant => {
                let Some(left) = vm.result else {
                    return Plain::No;
                };
                let right = code.fetch_constant(agent, instr.get_first_index(), gc);
                let Some(value) = Instruction::fused_operator(instr.get_second_index())
                    .and_then(|kind| Self::number_operation(agent, kind, left, right, gc))
                else {
                    return Plain::No;
                };
                vm.result = Some(value);
            }
            Instruction::ApplyBinaryOperatorToIndexWithConstant => {
                let (slot, operator) =
                    Instruction::fused_slot_and_operator(instr.get_first_index());
                let left = vm.stack[slot];
                let right = code.fetch_constant(agent, instr.get_second_index(), gc);
                let Some(value) = Instruction::fused_operator(operator)
                    .and_then(|kind| Self::number_operation(agent, kind, left, right, gc))
                else {
                    return Plain::No;
                };
                vm.result = Some(value);
            }
            Instruction::StoreConstant => {
                let constant = code.fetch_constant(agent, instr.get_first_index(), gc);
                vm.result = Some(constant.unbind());
            }
            Instruction::LoadConstant => {
                let constant = code.fetch_constant(agent, instr.get_first_index(), gc);
                vm.stack.push(constant.unbind());
            }
            Instruction::PopStack => vm.execute_pop_stack(),
            Instruction::PushReference => vm.execute_push_reference(agent),
            Instruction::PopReference => vm.execute_pop_reference(),
            Instruction::PushReferenceAndLoad => {
                vm.execute_push_reference(agent);
                vm.execute_load();
            }
            Instruction::Jump => {
                let from = *ip;
                execute_jump(agent, ip, instr);
                if *ip < from {
                    return Plain::Looped;
                }
            }
            Instruction::JumpIfNot => {
                let from = *ip;
                execute_jump_if_not(agent, vm, ip, instr);
                if *ip < from {
                    return Plain::Looped;
                }
            }
            Instruction::JumpIfTrue => {
                let from = *ip;
                execute_jump_if_true(agent, vm, ip, instr);
                if *ip < from {
                    return Plain::Looped;
                }
            }
            // The test of most loops, on two Numbers: before the body with
            // the jump out where it fails, or after it with the jump back
            // where it holds.
            Instruction::JumpIfNotLessThan
            | Instruction::JumpIfNotLessThanEquals
            | Instruction::JumpIfNotGreaterThan
            | Instruction::JumpIfNotGreaterThanEquals
            | Instruction::JumpIfLessThan
            | Instruction::JumpIfLessThanEquals
            | Instruction::JumpIfGreaterThan
            | Instruction::JumpIfGreaterThanEquals => {
                let (Some(&left), Some(right)) = (vm.stack.last(), vm.result) else {
                    return Plain::No;
                };
                let jumps = if let (Value::Integer(left), Value::Integer(right)) = (left, right) {
                    let (left, right) = (left.into_i64(), right.into_i64());
                    match instr.kind {
                        Instruction::JumpIfNotLessThan => left >= right,
                        Instruction::JumpIfNotLessThanEquals => left > right,
                        Instruction::JumpIfNotGreaterThan => left <= right,
                        Instruction::JumpIfNotGreaterThanEquals => left < right,
                        Instruction::JumpIfLessThan => left < right,
                        Instruction::JumpIfLessThanEquals => left <= right,
                        Instruction::JumpIfGreaterThan => left > right,
                        _ => left >= right,
                    }
                } else {
                    let (comparison, negated) = match instr.kind {
                        Instruction::JumpIfNotLessThan => (Instruction::LessThan, true),
                        Instruction::JumpIfNotLessThanEquals => (Instruction::LessThanEquals, true),
                        Instruction::JumpIfNotGreaterThan => (Instruction::GreaterThan, true),
                        Instruction::JumpIfNotGreaterThanEquals => {
                            (Instruction::GreaterThanEquals, true)
                        }
                        Instruction::JumpIfLessThan => (Instruction::LessThan, false),
                        Instruction::JumpIfLessThanEquals => (Instruction::LessThanEquals, false),
                        Instruction::JumpIfGreaterThan => (Instruction::GreaterThan, false),
                        _ => (Instruction::GreaterThanEquals, false),
                    };
                    let Some(holds) = Self::numbers_compare(agent, comparison, left, right) else {
                        return Plain::No;
                    };
                    holds != negated
                };
                vm.stack.pop();
                vm.result = None;
                if jumps {
                    let from = *ip;
                    *ip = instr.get_jump_slot();
                    if *ip < from {
                        return Plain::Looped;
                    }
                }
            }
            // The same tests on a local and a constant, or on two locals,
            // which are read where they are.
            Instruction::JumpIfIndexNotLessThanConstant
            | Instruction::JumpIfIndexNotLessThanIndex
            | Instruction::JumpIfIndexNotLessThanEqualsConstant
            | Instruction::JumpIfIndexNotLessThanEqualsIndex
            | Instruction::JumpIfIndexNotGreaterThanConstant
            | Instruction::JumpIfIndexNotGreaterThanIndex
            | Instruction::JumpIfIndexNotGreaterThanEqualsConstant
            | Instruction::JumpIfIndexNotGreaterThanEqualsIndex
            | Instruction::JumpIfIndexNotStrictlyEqualConstant
            | Instruction::JumpIfIndexNotStrictlyEqualIndex
            | Instruction::JumpIfIndexStrictlyEqualConstant
            | Instruction::JumpIfIndexStrictlyEqualIndex => {
                let Some((comparison, constant)) = instr.kind.compared_local() else {
                    return Plain::No;
                };
                let left = vm.stack[instr.get_first_index()];
                let right = if constant {
                    code.fetch_constant(agent, instr.get_second_index(), gc)
                        .unbind()
                } else {
                    vm.stack[instr.get_second_index()]
                };
                let jumps = match (comparison, left, right) {
                    (Instruction::JumpIfNotStrictlyEqual, left, right) => {
                        !Self::strictly_equal(agent, left, right)
                    }
                    (Instruction::JumpIfStrictlyEqual, left, right) => {
                        Self::strictly_equal(agent, left, right)
                    }
                    (comparison, Value::Integer(left), Value::Integer(right)) => {
                        let (left, right) = (left.into_i64(), right.into_i64());
                        !match comparison {
                            Instruction::JumpIfNotLessThan => left < right,
                            Instruction::JumpIfNotLessThanEquals => left <= right,
                            Instruction::JumpIfNotGreaterThan => left > right,
                            _ => left >= right,
                        }
                    }
                    (comparison, left, right) => {
                        let comparison = match comparison {
                            Instruction::JumpIfNotLessThan => Instruction::LessThan,
                            Instruction::JumpIfNotLessThanEquals => Instruction::LessThanEquals,
                            Instruction::JumpIfNotGreaterThan => Instruction::GreaterThan,
                            _ => Instruction::GreaterThanEquals,
                        };
                        let Some(holds) = Self::numbers_compare(agent, comparison, left, right)
                        else {
                            return Plain::No;
                        };
                        !holds
                    }
                };
                vm.result = None;
                if jumps {
                    let from = *ip;
                    *ip = Instr::wide_jump_target(instructions, from);
                    if *ip < from {
                        return Plain::Looped;
                    }
                }
            }
            // The test at the end of a loop, after `i++` or `i--` where the
            // instruction makes that too: all of it here, or none. The step
            // is made of an integer only; the test takes any two Numbers.
            Instruction::JumpIfIndexLessThanConstant
            | Instruction::JumpIfIndexLessThanIndex
            | Instruction::JumpIfIndexLessThanEqualsConstant
            | Instruction::JumpIfIndexLessThanEqualsIndex
            | Instruction::JumpIfIndexGreaterThanConstant
            | Instruction::JumpIfIndexGreaterThanIndex
            | Instruction::JumpIfIndexGreaterThanEqualsConstant
            | Instruction::JumpIfIndexGreaterThanEqualsIndex
            | Instruction::IncrementIndexJumpIfLessThanConstant
            | Instruction::IncrementIndexJumpIfLessThanIndex
            | Instruction::IncrementIndexJumpIfLessThanEqualsConstant
            | Instruction::IncrementIndexJumpIfLessThanEqualsIndex
            | Instruction::DecrementIndexJumpIfGreaterThanConstant
            | Instruction::DecrementIndexJumpIfGreaterThanIndex
            | Instruction::DecrementIndexJumpIfGreaterThanEqualsConstant
            | Instruction::DecrementIndexJumpIfGreaterThanEqualsIndex => {
                let Some((step, comparison, constant)) = instr.kind.loop_test() else {
                    return Plain::No;
                };
                let slot = instr.get_first_index();
                let right = if constant {
                    code.fetch_constant(agent, instr.get_second_index(), gc)
                        .unbind()
                } else {
                    vm.stack[instr.get_second_index()]
                };
                let holds = match (vm.stack[slot], right) {
                    (Value::Integer(left), Value::Integer(right)) => {
                        let (mut left, right) = (left.into_i64(), right.into_i64());
                        if step != 0 {
                            left += step as i64;
                            let Ok(stepped) = SmallInteger::try_from(left) else {
                                return Plain::No;
                            };
                            vm.stack[slot] = Value::Integer(stepped);
                        }
                        match comparison {
                            Instruction::LessThan => left < right,
                            Instruction::LessThanEquals => left <= right,
                            Instruction::GreaterThan => left > right,
                            _ => left >= right,
                        }
                    }
                    (left, right) => {
                        // The counter after its step, where it has one.
                        let stepped = match left {
                            _ if step == 0 => None,
                            Value::Integer(left) => {
                                let Ok(stepped) =
                                    SmallInteger::try_from(left.into_i64() + step as i64)
                                else {
                                    return Plain::No;
                                };
                                Some(Value::Integer(stepped))
                            }
                            _ => return Plain::No,
                        };
                        let Some(holds) = Self::numbers_compare(
                            agent,
                            comparison,
                            stepped.unwrap_or(left),
                            right,
                        ) else {
                            return Plain::No;
                        };
                        if let Some(stepped) = stepped {
                            vm.stack[slot] = stepped;
                        }
                        holds
                    }
                };
                vm.result = None;
                if holds {
                    let from = *ip;
                    *ip = Instr::wide_jump_target(instructions, from);
                    if *ip < from {
                        return Plain::Looped;
                    }
                }
            }
            // `i++` and `i--` of an integer in a stack slot.
            Instruction::IncrementIndex | Instruction::DecrementIndex => {
                let slot = instr.get_first_index();
                let Value::Integer(value) = vm.stack[slot] else {
                    return Plain::No;
                };
                let step = if instr.kind == Instruction::IncrementIndex {
                    1
                } else {
                    -1
                };
                let Ok(value) = SmallInteger::try_from(value.into_i64() + step) else {
                    return Plain::No;
                };
                vm.stack[slot] = Value::Integer(value);
            }
            // `this` of a call that keeps it in its execution context.
            Instruction::ResolveThisBinding => {
                let Some(this) = agent.running_call_this_value() else {
                    return Plain::No;
                };
                vm.result = Some(this);
            }
            Instruction::IsStrictlyEqual => {
                let left = vm.stack.pop().unwrap();
                let right = vm.result.unwrap();
                vm.result = Some(Self::strictly_equal(agent, left, right).into());
            }
            // `x === 3`: the constant is compared where it is.
            Instruction::IsStrictlyEqualConstant => {
                let left = vm.result.unwrap();
                let right = code.fetch_constant(agent, instr.get_first_index(), gc);
                vm.result = Some(Self::strictly_equal(agent, left, right).into());
            }
            // Two values of one type are loosely equal where they are
            // strictly equal: nothing is converted.
            Instruction::IsLooselyEqual => {
                let (Some(&left), Some(right)) = (vm.stack.last(), vm.result) else {
                    return Plain::No;
                };
                if !is_same_type(left, right) {
                    return Plain::No;
                }
                vm.stack.pop();
                vm.result = Some(Self::strictly_equal(agent, left, right).into());
            }
            // The test of an `if` on an equality, and the jump on it.
            Instruction::JumpIfNotStrictlyEqual
            | Instruction::JumpIfStrictlyEqual
            | Instruction::JumpIfNotLooselyEqual
            | Instruction::JumpIfLooselyEqual => {
                let (Some(&left), Some(right)) = (vm.stack.last(), vm.result) else {
                    return Plain::No;
                };
                let strict = matches!(
                    instr.kind,
                    Instruction::JumpIfNotStrictlyEqual | Instruction::JumpIfStrictlyEqual
                );
                if !strict && !is_same_type(left, right) {
                    return Plain::No;
                }
                let equal = Self::strictly_equal(agent, left, right);
                let jumps_if_equal = matches!(
                    instr.kind,
                    Instruction::JumpIfStrictlyEqual | Instruction::JumpIfLooselyEqual
                );
                vm.stack.pop();
                vm.result = None;
                if equal == jumps_if_equal {
                    let from = *ip;
                    *ip = instr.get_jump_slot();
                    if *ip < from {
                        return Plain::Looped;
                    }
                }
            }
            // The tests of a `switch` on literals: its cases only follow it.
            Instruction::JumpToCase => {
                let value = vm.stack[instr.get_first_index()];
                if let Some(target) = code.switch_target(agent, instr.get_second_index(), value) {
                    *ip = target;
                }
            }
            Instruction::UpdateEmpty => vm.execute_update_empty(),
            Instruction::LoadReplace => vm.execute_load_replace(),
            Instruction::Empty => vm.execute_empty(),
            Instruction::StoreCopy => vm.execute_store_copy(),
            Instruction::IsNullOrUndefined => vm.execute_is_null_or_undefined(),
            Instruction::IsNull => vm.execute_is_null(),
            Instruction::IsUndefined => vm.execute_is_undefined(),
            Instruction::LogicalNot => execute_logical_not(agent, vm),
            Instruction::PushExceptionJumpTarget => {
                execute_push_exception_jump_target(agent, vm, instr, gc)
            }
            Instruction::PopExceptionJumpTarget => vm.execute_pop_exception_jump_target(),
            Instruction::CreateImmutableBinding => {
                execute_create_immutable_binding(agent, executable.clone(), instr, gc)
            }
            Instruction::CreateMutableBinding => {
                execute_create_mutable_binding(agent, executable.clone(), instr, gc)
            }
            Instruction::InitializeReferencedBinding => {
                execute_initialize_referenced_binding(agent, vm, gc)
            }
            Instruction::EvaluatePropertyAccessWithIdentifierKey => {
                Self::property_reference_with_identifier_key(agent, vm, code, instr, gc)
            }
            Instruction::EvaluatePropertyAccessWithExpressionKey => {
                execute_evaluate_property_access_with_expression_key(agent, vm, gc)
            }
            // A variable of an enclosing function, where it was found the
            // last time.
            Instruction::GetBindingAtSite | Instruction::GetGlobalAtSite => {
                let name = code.fetch_identifier(agent, instr.get_first_index(), gc);
                let site = code.fetch_binding_site(agent, instr.get_second_index());
                let Some(value) = binding_value_at_site(agent, site, name, gc) else {
                    return Plain::No;
                };
                vm.result = Some(value.unbind());
            }
            // The same variable, pushed: an operand or an argument.
            Instruction::LoadBindingAtSite => {
                let name = code.fetch_identifier(agent, instr.get_first_index(), gc);
                let site = code.fetch_binding_site(agent, instr.get_second_index());
                let Some(value) = binding_value_at_site(agent, site, name, gc) else {
                    return Plain::No;
                };
                vm.result = None;
                vm.stack.push(value.unbind());
            }
            Instruction::PutBindingAtSite | Instruction::PutGlobalAtSite => {
                let Some(value) = vm.result else {
                    return Plain::No;
                };
                let name = code.fetch_identifier(agent, instr.get_first_index(), gc);
                let site = code.fetch_binding_site(agent, instr.get_second_index());
                if !set_binding_value_at_site(agent, site, name, value, gc) {
                    return Plain::No;
                }
                vm.result = None;
            }
            // A name of the global scope that is about to be assigned to:
            // its Reference, made where the name was found the last time.
            Instruction::ResolveGlobalAtSite => {
                let name = code.fetch_identifier(agent, instr.get_first_index(), gc);
                let site = code.fetch_binding_site(agent, instr.get_second_index());
                let strict = agent.is_evaluating_strict_code();
                let Some(reference) = try_resolve_binding_at_site(agent, site, name, strict, gc)
                else {
                    return Plain::No;
                };
                vm.reference = Some(reference.unbind());
            }
            // The assignment through that Reference, at the place in the
            // global scope it is to.
            Instruction::PutValueAtSite => {
                let Some(value) = vm.result else {
                    return Plain::No;
                };
                let site = code.fetch_binding_site(agent, instr.get_second_index());
                if !vm.reference.as_ref().is_some_and(|reference| {
                    reference_is_at_global_site(agent, site, reference, gc)
                }) {
                    return Plain::No;
                }
                let name = code.fetch_identifier(agent, instr.get_first_index(), gc);
                if !set_binding_value_at_site(agent, site, name, value, gc) {
                    return Plain::No;
                }
                vm.result = None;
                vm.reference = None;
            }
            // The parameters of a call, from its arguments.
            Instruction::BindArguments => {
                let first = instr.get_first_index();
                let count = instr.get_second_index();
                if let Some(VmIteratorRecord::SliceIterator(arguments)) = vm.iterator_stack.last() {
                    for slot in first..first + count {
                        let Some(value) = arguments.unshift(agent, gc) else {
                            break;
                        };
                        vm.stack[slot] = value.unbind();
                    }
                }
            }
            // A call of a built-in function that only computes a value from
            // arguments like these: computed here, with no call made.
            Instruction::EvaluateCall | Instruction::PopReferenceAndEvaluateCall => {
                let count = instr.get_first_index();
                let Some(first) = vm.stack.len().checked_sub(count) else {
                    return Plain::No;
                };
                if count == IndexType::MAX as usize || first == 0 {
                    return Plain::No;
                }
                let function = match vm.stack[first - 1] {
                    Value::BuiltinFunction(function) => function,
                    Value::ECMAScriptFunction(_) => return Plain::Call,
                    _ => return Plain::No,
                };
                let Some(direct) = function.get(agent).direct else {
                    return Plain::No;
                };
                // The this value of the call, for the functions that take
                // it: the base of the Reference the call is made through.
                let this = if !direct.takes_this() {
                    None
                } else if instr.kind == Instruction::PopReferenceAndEvaluateCall {
                    vm.reference_stack
                        .last()
                        .and_then(Reference::named_property_base)
                } else {
                    vm.reference
                        .as_ref()
                        .and_then(Reference::named_property_base)
                };
                let Some(result) = direct.run(agent, this, &vm.stack[first..], gc) else {
                    return Plain::No;
                };
                let result = result.unbind();
                vm.stack.truncate(first - 1);
                if instr.kind == Instruction::PopReferenceAndEvaluateCall {
                    vm.reference_stack.pop();
                }
                vm.reference = None;
                vm.result = Some(result);
            }
            // `o.p` where the property cache knows the shape of `o`, and the
            // length of an Array.
            Instruction::GetPropertyWithCache | Instruction::GetPropertyWithCacheKeepReference => {
                let Some(value) = Self::cached_property_value(agent, vm, code, instr, gc) else {
                    return Plain::No;
                };
                if instr.kind == Instruction::GetPropertyWithCacheKeepReference {
                    Self::property_reference_with_identifier_key(agent, vm, code, instr, gc);
                } else {
                    vm.reference = None;
                }
                vm.result = Some(value);
            }
            // `p.x` of a local: the stack slot, and its property by the
            // cache.
            Instruction::GetIndexPropertyWithCache
            | Instruction::GetIndexPropertyWithCacheKeepReference => {
                let base = vm.stack[Instr::wide_third_index(instructions, *ip)];
                let Some(value) = Self::cached_property_value_of(agent, base, code, instr, gc)
                else {
                    return Plain::No;
                };
                if instr.kind == Instruction::GetIndexPropertyWithCacheKeepReference {
                    vm.result = Some(base);
                    Self::property_reference_with_identifier_key(agent, vm, code, instr, gc);
                } else {
                    vm.reference = None;
                }
                vm.result = Some(value);
            }
            // `this.p`: the this value that the call keeps, and its
            // property by the cache.
            Instruction::GetThisPropertyWithCache
            | Instruction::GetThisPropertyWithCacheKeepReference => {
                let Some(this) = agent.running_call_this_value() else {
                    return Plain::No;
                };
                let Some(value) = Self::cached_property_value_of(agent, this, code, instr, gc)
                else {
                    return Plain::No;
                };
                if instr.kind == Instruction::GetThisPropertyWithCacheKeepReference {
                    vm.result = Some(this);
                    Self::property_reference_with_identifier_key(agent, vm, code, instr, gc);
                } else {
                    vm.reference = None;
                }
                vm.result = Some(value);
            }
            // `o.p = v` with `o` on the stack, where the property cache
            // knows its shape and `p` is a writable data property of its
            // own.
            Instruction::PutPropertyWithCache => {
                let (Some(value), Some(&Value::Object(object))) = (vm.result, vm.stack.last())
                else {
                    return Plain::No;
                };
                let cache = code.fetch_cache(agent, instr.get_second_index(), gc);
                let shape = object.object_shape(agent);
                let Some((offset, None)) = cache.find_cached_property_offset(agent, shape) else {
                    return Plain::No;
                };
                if offset.is_unset() {
                    // Neither the object nor any it inherits from has the
                    // property: it is added, as most properties of an
                    // object are.
                    let key = code.fetch_property_key(agent, instr.get_first_index(), gc);
                    if !ordinary_add_own_data_by_cache(agent, object, key, value, cache, gc) {
                        return Plain::No;
                    }
                } else if !ordinary_set_own_data_at_offset(agent, object, offset, value) {
                    return Plain::No;
                }
                vm.stack.pop();
                vm.result = None;
            }
            // `o.p = v` where the property cache knows the shape of `o` and
            // `p` is a writable data property of its own.
            Instruction::PutValueWithCache => {
                let (Some(value), Some(reference)) = (vm.result, vm.reference.as_ref()) else {
                    return Plain::No;
                };
                let Some(Value::Object(object)) = reference.named_property_base() else {
                    return Plain::No;
                };
                let cache = code.fetch_cache(agent, instr.get_first_index(), gc);
                let shape = object.object_shape(agent);
                let Some((offset, None)) = cache.find_cached_property_offset(agent, shape) else {
                    return Plain::No;
                };
                if offset.is_unset()
                    || !ordinary_set_own_data_at_offset(agent, object, offset, value)
                {
                    return Plain::No;
                }
                vm.result = None;
                vm.reference = None;
            }
            // `a[i]` of an Array that has an element there, or of a
            // TypedArray within its length.
            Instruction::GetValue | Instruction::GetValueKeepReference => {
                let Some((base, index)) = vm
                    .reference
                    .as_ref()
                    .and_then(Reference::element_base_and_index)
                else {
                    return Plain::No;
                };
                let value = if let Value::Array(array) = base {
                    let Some(&Some(value)) = usize::try_from(index)
                        .ok()
                        .and_then(|index| array.as_slice(agent).get(index))
                    else {
                        return Plain::No;
                    };
                    value.unbind()
                } else {
                    let Some(value) = Self::typed_array_element(agent, base, index, gc) else {
                        return Plain::No;
                    };
                    value
                };
                if instr.kind == Instruction::GetValue {
                    vm.reference = None;
                }
                vm.result = Some(value);
            }
            // The same read with the base and the key where the expression
            // left them: no Reference is made of them first.
            Instruction::GetValueWithExpressionKey => {
                let (Some(&base), Some(Value::Integer(index))) = (vm.stack.last(), vm.result)
                else {
                    return Plain::No;
                };
                let index = index.into_i64();
                let value = if let Value::Array(array) = base {
                    let Some(&Some(value)) = usize::try_from(index)
                        .ok()
                        .and_then(|index| array.as_slice(agent).get(index))
                    else {
                        return Plain::No;
                    };
                    value.unbind()
                } else {
                    let Some(value) = Self::typed_array_element(agent, base, index, gc) else {
                        return Plain::No;
                    };
                    value
                };
                vm.stack.pop();
                vm.result = Some(value);
            }
            // The same read with the base as the result and the key in a
            // local: neither goes by the stack.
            Instruction::GetValueWithIndexKey => {
                let (Some(base), Value::Integer(index)) =
                    (vm.result, vm.stack[instr.get_first_index()])
                else {
                    return Plain::No;
                };
                let index = index.into_i64();
                let value = if let Value::Array(array) = base {
                    let Some(&Some(value)) = usize::try_from(index)
                        .ok()
                        .and_then(|index| array.as_slice(agent).get(index))
                    else {
                        return Plain::No;
                    };
                    value.unbind()
                } else {
                    let Some(value) = Self::typed_array_element(agent, base, index, gc) else {
                        return Plain::No;
                    };
                    value
                };
                vm.result = Some(value);
            }
            // `a[i] = v` for an element the Array has, as a plain value, or
            // for a TypedArray and a Number.
            Instruction::PutValue => {
                let (Some(value), Some((base, index))) = (
                    vm.result,
                    vm.reference
                        .as_ref()
                        .and_then(Reference::element_base_and_index),
                ) else {
                    return Plain::No;
                };
                if let Value::Array(array) = base {
                    let Ok(index) = u32::try_from(index) else {
                        return Plain::No;
                    };
                    let storage = array.get_storage(agent);
                    if !matches!(storage.values.get(index as usize), Some(Some(_)))
                        || storage
                            .descriptors
                            .is_some_and(|descriptors| descriptors.contains_key(&index))
                    {
                        return Plain::No;
                    }
                    array.as_mut_slice(agent)[index as usize] = Some(value);
                } else if !Self::set_typed_array_element(agent, base, index, value) {
                    return Plain::No;
                }
                vm.result = None;
                vm.reference = None;
            }
            // The same assignment with the object and the key on the stack.
            Instruction::PutValueWithExpressionKey => {
                let (Some(value), [.., base, Value::Integer(index)]) =
                    (vm.result, vm.stack.as_slice())
                else {
                    return Plain::No;
                };
                let (base, index) = (*base, index.into_i64());
                if let Value::Array(array) = base {
                    let Ok(index) = u32::try_from(index) else {
                        return Plain::No;
                    };
                    let storage = array.get_storage(agent);
                    if !matches!(storage.values.get(index as usize), Some(Some(_)))
                        || storage
                            .descriptors
                            .is_some_and(|descriptors| descriptors.contains_key(&index))
                    {
                        return Plain::No;
                    }
                    array.as_mut_slice(agent)[index as usize] = Some(value);
                } else if !Self::set_typed_array_element(agent, base, index, value) {
                    return Plain::No;
                }
                vm.stack.truncate(vm.stack.len() - 2);
                vm.result = None;
            }
            // A Number is its own numeric value: the counter of nearly every
            // loop.
            Instruction::ToNumeric => {
                if !matches!(
                    vm.result,
                    Some(Value::Integer(_) | Value::SmallF64(_) | Value::Number(_))
                ) {
                    return Plain::No;
                }
            }
            Instruction::Increment => return vm.step_integer(1),
            Instruction::Decrement => return vm.step_integer(-1),
            Instruction::LessThan
            | Instruction::LessThanEquals
            | Instruction::GreaterThan
            | Instruction::GreaterThanEquals
            | Instruction::ApplyAdditionBinaryOperator
            | Instruction::ApplySubtractionBinaryOperator
            | Instruction::ApplyMultiplicationBinaryOperator
            | Instruction::ApplyDivisionBinaryOperator
            | Instruction::ApplyRemainderBinaryOperator
            | Instruction::ApplyShiftLeftBinaryOperator
            | Instruction::ApplyShiftRightBinaryOperator
            | Instruction::ApplyShiftRightZeroFillBinaryOperator
            | Instruction::ApplyBitwiseORBinaryOperator
            | Instruction::ApplyBitwiseXORBinaryOperator
            | Instruction::ApplyBitwiseAndBinaryOperator => {
                return vm.operate_on_integers(agent, instr.kind, gc);
            }
            _ => return Plain::No,
        }
        Plain::Done
    }

    /// IsStrictlyEqual(x, y), with two integers compared in place.
    #[inline(always)]
    fn strictly_equal(agent: &Agent, x: Value, y: Value) -> bool {
        if let (Value::Integer(x), Value::Integer(y)) = (x, y) {
            return x.into_i64() == y.into_i64();
        }
        is_strictly_equal(agent, x, y)
    }

    /// EvaluatePropertyAccessWithIdentifierKey, with the key fetched from the
    /// executable the loop has at hand.
    #[inline(always)]
    fn property_reference_with_identifier_key(
        agent: &mut Agent,
        vm: &mut Vm,
        code: Executable,
        instr: Instr,
        gc: NoGcScope,
    ) {
        let property_key = code.fetch_property_key(agent, instr.get_first_index(), gc);
        let base_value = vm.result.take().unwrap().bind(gc);
        let strict = agent.is_evaluating_strict_code();
        vm.reference =
            Some(Reference::new_property_reference(base_value, property_key, strict).unbind());
    }

    /// The value of `o.p` for the object in the result register, when it is
    /// read without a call: a property the cache places for the shape of an
    /// ordinary object (its own or of a prototype, or known to be absent),
    /// or the length of an Array.
    #[inline(always)]
    fn cached_property_value(
        agent: &mut Agent,
        vm: &Vm,
        code: Executable,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        Self::cached_property_value_of(agent, vm.result?, code, instr, gc)
    }

    /// GetPropertyWithCache or GetPropertyWithCacheKeepReference of the
    /// value in the result register, where the cache places the property:
    /// what the plain instruction does. Returns whether it did.
    pub(super) fn get_property_by_cache(
        agent: &mut Agent,
        vm: &mut Vm,
        executable: &Scoped<Executable>,
        instr: Instr,
        keep_reference: bool,
        gc: NoGcScope,
    ) -> bool {
        let code = executable.get(agent);
        let Some(value) = Self::cached_property_value(agent, vm, code, instr, gc) else {
            return false;
        };
        if keep_reference {
            Self::property_reference_with_identifier_key(agent, vm, code, instr, gc);
        } else {
            vm.reference = None;
        }
        vm.result = Some(value);
        true
    }

    /// The value of a property of a primitive value that is found on
    /// `prototype`, the prototype object of its kind in the running realm,
    /// or further up: by the cache, under the shape of `prototype`, where
    /// \[\[Get]] of that object left it.
    #[inline]
    fn cached_prototype_property_value(
        agent: &mut Agent,
        prototype: OrdinaryObject<'static>,
        code: Executable,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let cache = code.fetch_cache(agent, instr.get_second_index(), gc);
        let shape = prototype.object_shape(agent);
        let (offset, holder) = cache.find_cached_property_offset(agent, shape)?;
        if offset.is_unset() {
            return Some(Value::Undefined);
        }
        let found = match holder {
            Some(holder) => holder.get_own_property_at_offset(agent, offset, gc),
            None => prototype.get_own_property_at_offset(agent, offset, gc),
        };
        match found {
            TryGetResult::Value(value) => Some(value.unbind()),
            _ => None,
        }
    }

    /// The same for `base`, wherever the instruction has it from.
    #[inline(always)]
    fn cached_property_value_of(
        agent: &mut Agent,
        base: Value,
        code: Executable,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        match base {
            Value::Object(object) => {
                let cache = code.fetch_cache(agent, instr.get_second_index(), gc);
                let shape = object.object_shape(agent);
                let (offset, prototype) = cache.find_cached_property_offset(agent, shape)?;
                if offset.is_unset() {
                    return Some(Value::Undefined);
                }
                let found = match prototype {
                    Some(prototype) => prototype.get_own_property_at_offset(agent, offset, gc),
                    None => object.get_own_property_at_offset(agent, offset, gc),
                };
                match found {
                    TryGetResult::Value(value) => Some(value.unbind()),
                    _ => None,
                }
            }
            Value::Array(array) => {
                let key = code.fetch_property_key(agent, instr.get_first_index(), gc);
                if key == PropertyKey::from(BUILTIN_STRING_MEMORY.length) {
                    return Some(array.len(agent).into());
                }
                if key.into_u32().is_some() {
                    return None;
                }
                // A named property (`a.push`): what \[\[Get]] of an Array
                // finds in the cache, under the shape of the Array's own
                // properties or the one of all Arrays that have none.
                let object = Object::from(array);
                let cache = code.fetch_cache(agent, instr.get_second_index(), gc);
                let shape = object.object_shape(agent);
                let (offset, prototype) = cache.find_cached_property_offset(agent, shape)?;
                if offset.is_unset() {
                    return Some(Value::Undefined);
                }
                match prototype
                    .unwrap_or(object)
                    .get_own_property_at_offset(agent, offset, gc)
                {
                    TryGetResult::Value(value) => Some(value.unbind()),
                    _ => None,
                }
            }
            // A String value: its length, or what \[\[Get]] of the
            // %String.prototype% of the running realm finds in the cache
            // (`s.charCodeAt`), which is where a name that is not `length`
            // is looked for.
            Value::String(_) | Value::SmallString(_) => {
                let key = code.fetch_property_key(agent, instr.get_first_index(), gc);
                if key == PropertyKey::from(BUILTIN_STRING_MEMORY.length) {
                    let string = String::try_from(base).ok()?;
                    let length = i64::try_from(string.utf16_len_(agent)).ok()?;
                    return SmallInteger::try_from(length).ok().map(Value::Integer);
                }
                if key.into_u32().is_some() {
                    return None;
                }
                let prototype = agent
                    .current_realm_record()
                    .intrinsics()
                    .string_prototype_backing_object();
                Self::cached_prototype_property_value(agent, prototype, code, instr, gc)
            }
            // A Number value has no properties of its own: what \[\[Get]]
            // of %Number.prototype% finds in the cache (`n.toFixed`).
            Value::Integer(_) | Value::SmallF64(_) | Value::Number(_) => {
                let prototype = agent
                    .current_realm_record()
                    .intrinsics()
                    .number_prototype_backing_object();
                Self::cached_prototype_property_value(agent, prototype, code, instr, gc)
            }
            #[cfg(feature = "array-buffer")]
            value => Self::typed_array_length(agent, value, code, instr, gc),
            #[cfg(not(feature = "array-buffer"))]
            _ => None,
        }
    }

    /// `a[i]` of a TypedArray, for an index within its length: the element,
    /// read from the buffer. Outside the length the general way gives
    /// undefined.
    #[cfg(feature = "array-buffer")]
    #[inline]
    fn typed_array_element(
        agent: &mut Agent,
        base: Value,
        index: i64,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let array = AnyTypedArray::try_from(base).ok()?;
        let element = array.typed_array_get_element(agent, index, gc)?;
        Some(Value::from(element).unbind())
    }

    #[cfg(not(feature = "array-buffer"))]
    #[inline]
    fn typed_array_element(
        _: &mut Agent,
        _: Value,
        _: i64,
        _: NoGcScope,
    ) -> Option<Value<'static>> {
        None
    }

    /// `a[i] = v` of a TypedArray and a value that needs no conversion (a
    /// Number, or a BigInt where the elements are BigInts): written to the
    /// buffer, or dropped outside the length, as TypedArraySetElement has
    /// it.
    #[cfg(feature = "array-buffer")]
    #[inline]
    fn set_typed_array_element(agent: &mut Agent, base: Value, index: i64, value: Value) -> bool {
        AnyTypedArray::try_from(base)
            .is_ok_and(|array| array.set_element_unconverted(agent, index, value))
    }

    #[cfg(not(feature = "array-buffer"))]
    #[inline]
    fn set_typed_array_element(_: &mut Agent, _: Value, _: i64, _: Value) -> bool {
        false
    }

    /// `a.length` of a TypedArray, where the property cache finds for its
    /// shape the accessor of %TypedArray%.prototype: what the accessor
    /// returns, without the call.
    #[cfg(feature = "array-buffer")]
    fn typed_array_length(
        agent: &mut Agent,
        value: Value,
        code: Executable,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let array = AnyTypedArray::try_from(value).ok()?;
        let key = code.fetch_property_key(agent, instr.get_first_index(), gc);
        if key != PropertyKey::from(BUILTIN_STRING_MEMORY.length) {
            return None;
        }
        let cache = code.fetch_cache(agent, instr.get_second_index(), gc);
        let shape = array.object_shape(agent);
        let (offset, Some(prototype)) = cache.find_cached_property_offset(agent, shape)? else {
            return None;
        };
        if offset.is_unset() {
            return None;
        }
        let TryGetResult::Get(getter) = prototype.get_own_property_at_offset(agent, offset, gc)
        else {
            return None;
        };
        TypedArrayPrototype::is_length_getter(agent, getter).then(|| array.length_value(agent))
    }

    /// `++` or `--` of an integer that stays one.
    #[inline(always)]
    fn step_integer(&mut self, step: i64) -> Plain {
        let Some(Value::Integer(value)) = self.result else {
            return Plain::No;
        };
        let Ok(value) = SmallInteger::try_from(value.into_i64() + step) else {
            return Plain::No;
        };
        self.result = Some(Value::Integer(value));
        Plain::Done
    }

    /// A comparison or an arithmetic operator on two Numbers, the left one
    /// on the stack and the right one the result: nothing to convert.
    #[inline(always)]
    fn operate_on_integers(
        &mut self,
        agent: &mut Agent,
        kind: Instruction,
        gc: NoGcScope,
    ) -> Plain {
        let (Some(&left), Some(right)) = (self.stack.last(), self.result) else {
            return Plain::No;
        };
        let Some(value) = Self::number_operation(agent, kind, left, right, gc) else {
            return Plain::No;
        };
        self.finish_operation(value)
    }

    /// What a comparison or an arithmetic operator gives for two Numbers,
    /// which no conversion that could run a script comes into: in place for
    /// two integers whose result is a Boolean or an integer again.
    #[inline(always)]
    fn number_operation(
        agent: &mut Agent,
        kind: Instruction,
        left: Value,
        right: Value,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        if let (Value::Integer(left), Value::Integer(right)) = (left, right)
            && let Some(value) = Self::integer_operation(kind, left.into_i64(), right.into_i64())
        {
            return Some(value);
        }
        // Seen here, so that the `+` of two strings is not a call first.
        if !(left.is_number() && right.is_number()) {
            return None;
        }
        Self::float_operation(agent, kind, left, right, gc)
    }

    /// Whether `comparison` (LessThan or one like it) holds for two Numbers
    /// of any kind, compared as doubles: it does not where one is NaN. None
    /// where an operand is not a Number, and would be converted.
    #[inline(always)]
    fn numbers_compare(
        agent: &Agent,
        comparison: Instruction,
        left: Value,
        right: Value,
    ) -> Option<bool> {
        // Seen here, so that a comparison of two strings is not a call first.
        if !(left.is_number() && right.is_number()) {
            return None;
        }
        Self::doubles_compare(agent, comparison, left, right)
    }

    #[inline(never)]
    fn doubles_compare(
        agent: &Agent,
        comparison: Instruction,
        left: Value,
        right: Value,
    ) -> Option<bool> {
        let (Ok(left), Ok(right)) = (Number::try_from(left), Number::try_from(right)) else {
            return None;
        };
        let (left, right) = (left.into_f64_(agent), right.into_f64_(agent));
        Some(match comparison {
            Instruction::LessThan => left < right,
            Instruction::LessThanEquals => left <= right,
            Instruction::GreaterThan => left > right,
            _ => left >= right,
        })
    }

    /// The comparisons and the operators on two Numbers of any kind. A
    /// comparison is of two doubles, where one that is NaN compares as
    /// false; an operator is what the Number type defines for it. A result
    /// that is a double too wide for a Value is a Number of the heap: the
    /// one thing a plain instruction allocates, and no collection is due
    /// before the loop looks for one again.
    #[inline(never)]
    fn float_operation(
        agent: &mut Agent,
        kind: Instruction,
        left: Value,
        right: Value,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let (Ok(left), Ok(right)) = (Number::try_from(left), Number::try_from(right)) else {
            return None;
        };
        let (left, right) = (left.bind(gc), right.bind(gc));
        let value: Value = match kind {
            Instruction::LessThan
            | Instruction::LessThanEquals
            | Instruction::GreaterThan
            | Instruction::GreaterThanEquals => {
                let (left, right) = (left.into_f64_(agent), right.into_f64_(agent));
                let holds = match kind {
                    Instruction::LessThan => left < right,
                    Instruction::LessThanEquals => left <= right,
                    Instruction::GreaterThan => left > right,
                    _ => left >= right,
                };
                holds.into()
            }
            Instruction::ApplyAdditionBinaryOperator => Number::add(agent, left, right).into(),
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
                number_binary_operator(agent, kind, left, right, gc)
                    .ok()?
                    .into()
            }
            _ => return None,
        };
        Some(value.unbind())
    }

    /// What a comparison or an arithmetic operator gives for two integers,
    /// where that is a Boolean or an integer again.
    #[inline(always)]
    fn integer_operation(kind: Instruction, left: i64, right: i64) -> Option<Value<'static>> {
        // ToInt32 and ToUint32 of an integer are its low 32 bits.
        let (left32, right32) = (left as i32, right as i32);
        let integer = match kind {
            Instruction::LessThan => return Some((left < right).into()),
            Instruction::LessThanEquals => return Some((left <= right).into()),
            Instruction::GreaterThan => return Some((left > right).into()),
            Instruction::GreaterThanEquals => return Some((left >= right).into()),
            Instruction::ApplyAdditionBinaryOperator => left + right,
            Instruction::ApplySubtractionBinaryOperator => left - right,
            Instruction::ApplyMultiplicationBinaryOperator => {
                let product = left.checked_mul(right)?;
                // A zero product of a negative factor is -0: not an integer.
                if product == 0 && (left < 0 || right < 0) {
                    return None;
                }
                product
            }
            Instruction::ApplyRemainderBinaryOperator => {
                // A zero remainder of a negative dividend is -0, and the
                // remainder of a division by zero is NaN.
                if left < 0 || right <= 0 {
                    return None;
                }
                left % right
            }
            Instruction::ApplyShiftLeftBinaryOperator => {
                i64::from(left32.wrapping_shl(right32 as u32))
            }
            Instruction::ApplyShiftRightBinaryOperator => {
                i64::from(left32.wrapping_shr(right32 as u32))
            }
            Instruction::ApplyShiftRightZeroFillBinaryOperator => {
                i64::from((left32 as u32).wrapping_shr(right32 as u32))
            }
            Instruction::ApplyBitwiseORBinaryOperator => i64::from(left32 | right32),
            Instruction::ApplyBitwiseXORBinaryOperator => i64::from(left32 ^ right32),
            Instruction::ApplyBitwiseAndBinaryOperator => i64::from(left32 & right32),
            _ => return None,
        };
        SmallInteger::try_from(integer).ok().map(Value::Integer)
    }

    /// Replaces the two operands of a binary operator with its result.
    #[inline(always)]
    fn finish_operation(&mut self, result: Value<'static>) -> Plain {
        self.stack.pop();
        self.result = Some(result);
        Plain::Done
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
            Instruction::Load
            | Instruction::LoadCopy
            | Instruction::PutValueToIndex
            | Instruction::Store
            | Instruction::GetValueFromIndex
            | Instruction::LoadFromIndex
            | Instruction::StoreConstant
            | Instruction::LoadConstant
            | Instruction::PopStack
            | Instruction::PushReference
            | Instruction::PopReference
            | Instruction::PushReferenceAndLoad
            | Instruction::LoadFromIndexStoreConstant
            | Instruction::LoadFromIndexGetFromIndex
            | Instruction::LoadFromIndexLoadFromIndex
            | Instruction::Jump
            | Instruction::JumpIfNot
            | Instruction::JumpIfTrue
            | Instruction::IsStrictlyEqual
            | Instruction::JumpToCase
            | Instruction::CreateImmutableBinding
            | Instruction::CreateMutableBinding
            | Instruction::InitializeReferencedBinding
            | Instruction::EvaluatePropertyAccessWithIdentifierKey
            | Instruction::EvaluatePropertyAccessWithExpressionKey => {
                unreachable!("plain instruction not handled before execute_instruction")
            }
            Instruction::ResolveBinding => {
                execute_resolve_binding(agent, vm, executable, instr, gc)?;
            }
            Instruction::ResolveBindingAtSite => {
                execute_resolve_binding_at_site(agent, vm, executable, instr, gc)?;
            }
            Instruction::GetBindingAtSite => {
                execute_get_binding_at_site(agent, vm, executable, instr, false, gc)?;
            }
            Instruction::PutBindingAtSite => {
                execute_put_binding_at_site(agent, vm, executable, instr, false, gc)?;
            }
            Instruction::ResolveGlobalAtSite
            | Instruction::GetGlobalAtSite
            | Instruction::PutGlobalAtSite
            | Instruction::PutValueAtSite => {
                execute_global_name_the_long_way(agent, vm, executable, instr, gc)?;
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
            // What the fused instructions stand for, where their operands
            // are not two integers, or not an integer.
            Instruction::JumpIfNotLessThan
            | Instruction::JumpIfNotLessThanEquals
            | Instruction::JumpIfNotGreaterThan
            | Instruction::JumpIfNotGreaterThanEquals
            | Instruction::JumpIfLessThan
            | Instruction::JumpIfLessThanEquals
            | Instruction::JumpIfGreaterThan
            | Instruction::JumpIfGreaterThanEquals
            | Instruction::JumpIfNotStrictlyEqual
            | Instruction::JumpIfStrictlyEqual
            | Instruction::JumpIfNotLooselyEqual
            | Instruction::JumpIfLooselyEqual
            | Instruction::JumpIfIndexNotLessThanConstant
            | Instruction::JumpIfIndexNotLessThanIndex
            | Instruction::JumpIfIndexNotLessThanEqualsConstant
            | Instruction::JumpIfIndexNotLessThanEqualsIndex
            | Instruction::JumpIfIndexNotGreaterThanConstant
            | Instruction::JumpIfIndexNotGreaterThanIndex
            | Instruction::JumpIfIndexNotGreaterThanEqualsConstant
            | Instruction::JumpIfIndexNotGreaterThanEqualsIndex
            | Instruction::JumpIfIndexNotStrictlyEqualConstant
            | Instruction::JumpIfIndexNotStrictlyEqualIndex
            | Instruction::JumpIfIndexStrictlyEqualConstant
            | Instruction::JumpIfIndexStrictlyEqualIndex
            | Instruction::JumpIfIndexLessThanConstant
            | Instruction::JumpIfIndexLessThanIndex
            | Instruction::JumpIfIndexLessThanEqualsConstant
            | Instruction::JumpIfIndexLessThanEqualsIndex
            | Instruction::JumpIfIndexGreaterThanConstant
            | Instruction::JumpIfIndexGreaterThanIndex
            | Instruction::JumpIfIndexGreaterThanEqualsConstant
            | Instruction::JumpIfIndexGreaterThanEqualsIndex
            | Instruction::IncrementIndexJumpIfLessThanConstant
            | Instruction::IncrementIndexJumpIfLessThanIndex
            | Instruction::IncrementIndexJumpIfLessThanEqualsConstant
            | Instruction::IncrementIndexJumpIfLessThanEqualsIndex
            | Instruction::DecrementIndexJumpIfGreaterThanConstant
            | Instruction::DecrementIndexJumpIfGreaterThanIndex
            | Instruction::DecrementIndexJumpIfGreaterThanEqualsConstant
            | Instruction::DecrementIndexJumpIfGreaterThanEqualsIndex
            | Instruction::IncrementIndex
            | Instruction::DecrementIndex
            | Instruction::GetValueWithExpressionKey
            | Instruction::GetValueWithIndexKey
            | Instruction::PutValueWithExpressionKey
            | Instruction::LoadBindingAtSite
            | Instruction::ApplyBinaryOperatorWithConstant
            | Instruction::ApplyBinaryOperatorToIndexWithConstant => {
                execute_fused_instruction(agent, vm, executable, instr, gc)?;
            }
            Instruction::PutPropertyWithCache
            | Instruction::GetThisPropertyWithCache
            | Instruction::GetThisPropertyWithCacheKeepReference
            | Instruction::GetIndexPropertyWithCache
            | Instruction::GetIndexPropertyWithCacheKeepReference => {
                // Kept out of this function, whose frame every native call
                // of a function is on top of.
                execute_fused_property_instruction(agent, vm, executable, instr, gc)?;
            }
            Instruction::GetPropertyWithCache | Instruction::GetPropertyWithCacheKeepReference => {
                let keep_reference =
                    matches!(instr.kind, Instruction::GetPropertyWithCacheKeepReference);
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
                    instr.get_second_index() as IndexType,
                );
                execute_get_value(agent, vm, executable, get_value, true, keep_reference, gc)?
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
            | Instruction::ResolveBindingAtSite
            | Instruction::GetBindingAtSite
            | Instruction::PutBindingAtSite
            | Instruction::ResolveGlobalAtSite
            | Instruction::GetGlobalAtSite
            | Instruction::PutGlobalAtSite
            | Instruction::PutValueAtSite
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
            | Instruction::GetThisPropertyWithCache
            | Instruction::GetThisPropertyWithCacheKeepReference
            | Instruction::GetIndexPropertyWithCache
            | Instruction::GetIndexPropertyWithCacheKeepReference
            | Instruction::PutPropertyWithCache
            | Instruction::PushReferenceAndLoad
            | Instruction::LoadFromIndexStoreConstant
            | Instruction::LoadFromIndexGetFromIndex
            | Instruction::LoadFromIndexLoadFromIndex
            | Instruction::JumpIfNotLessThan
            | Instruction::JumpIfNotLessThanEquals
            | Instruction::JumpIfNotGreaterThan
            | Instruction::JumpIfNotGreaterThanEquals
            | Instruction::JumpIfLessThan
            | Instruction::JumpIfLessThanEquals
            | Instruction::JumpIfGreaterThan
            | Instruction::JumpIfGreaterThanEquals
            | Instruction::JumpIfNotStrictlyEqual
            | Instruction::JumpIfStrictlyEqual
            | Instruction::JumpIfNotLooselyEqual
            | Instruction::JumpIfLooselyEqual
            | Instruction::JumpIfIndexNotLessThanConstant
            | Instruction::JumpIfIndexNotLessThanIndex
            | Instruction::JumpIfIndexNotLessThanEqualsConstant
            | Instruction::JumpIfIndexNotLessThanEqualsIndex
            | Instruction::JumpIfIndexNotGreaterThanConstant
            | Instruction::JumpIfIndexNotGreaterThanIndex
            | Instruction::JumpIfIndexNotGreaterThanEqualsConstant
            | Instruction::JumpIfIndexNotGreaterThanEqualsIndex
            | Instruction::JumpIfIndexNotStrictlyEqualConstant
            | Instruction::JumpIfIndexNotStrictlyEqualIndex
            | Instruction::JumpIfIndexStrictlyEqualConstant
            | Instruction::JumpIfIndexStrictlyEqualIndex
            | Instruction::JumpIfIndexLessThanConstant
            | Instruction::JumpIfIndexLessThanIndex
            | Instruction::JumpIfIndexLessThanEqualsConstant
            | Instruction::JumpIfIndexLessThanEqualsIndex
            | Instruction::JumpIfIndexGreaterThanConstant
            | Instruction::JumpIfIndexGreaterThanIndex
            | Instruction::JumpIfIndexGreaterThanEqualsConstant
            | Instruction::JumpIfIndexGreaterThanEqualsIndex
            | Instruction::IncrementIndexJumpIfLessThanConstant
            | Instruction::IncrementIndexJumpIfLessThanIndex
            | Instruction::IncrementIndexJumpIfLessThanEqualsConstant
            | Instruction::IncrementIndexJumpIfLessThanEqualsIndex
            | Instruction::DecrementIndexJumpIfGreaterThanConstant
            | Instruction::DecrementIndexJumpIfGreaterThanIndex
            | Instruction::DecrementIndexJumpIfGreaterThanEqualsConstant
            | Instruction::DecrementIndexJumpIfGreaterThanEqualsIndex
            | Instruction::JumpToCase
            | Instruction::IncrementIndex
            | Instruction::DecrementIndex
            | Instruction::GetValueWithExpressionKey
            | Instruction::GetValueWithIndexKey
            | Instruction::PutValueWithExpressionKey
            | Instruction::LoadBindingAtSite
            | Instruction::ApplyBinaryOperatorWithConstant
            | Instruction::ApplyBinaryOperatorToIndexWithConstant
            | Instruction::BindArguments => {
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
            Instruction::JumpIfTrue => {
                let mut ip = vm.ip;
                execute_jump_if_true(agent, vm, &mut ip, instr);
                vm.ip = ip;
            }
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
    fn execute_push_reference(&mut self, agent: &mut Agent) {
        if self.reference_stack.capacity() == 0
            && let Some(spare) = agent.spare_vm_stacks.references.pop()
        {
            self.reference_stack = spare;
        }
        self.reference_stack.push(self.reference.take().unwrap());
    }

    /// The exception handler stack to push to: a spare one for the first
    /// handler of this `Vm`.
    #[inline(always)]
    pub(super) fn exception_handlers(
        &mut self,
        agent: &mut Agent,
    ) -> &mut Vec<ExceptionHandler<'static>> {
        if self.exception_handler_stack.capacity() == 0
            && let Some(spare) = agent.spare_vm_stacks.handlers.pop()
        {
            self.exception_handler_stack = spare;
        }
        &mut self.exception_handler_stack
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
    // What is replaced is the empty Vm put there above, which nothing could
    // reach in between: it holds nothing to free.
    core::mem::forget(core::mem::replace(vm, agent.vm_stack.pop().unwrap()));
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
            depth: _,
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
            depth: _,
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
        stack.as_slice().mark_values(queues);
        reference_stack.as_slice().mark_values(queues);
        iterator_stack.as_slice().mark_values(queues);
        exception_jump_target_stack.as_slice().mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            ip: _,
            stack,
            reference_stack,
            iterator_stack,
            exception_jump_target_stack,
        } = self;
        stack.as_mut_slice().sweep_values(compactions);
        reference_stack.as_mut_slice().sweep_values(compactions);
        iterator_stack.as_mut_slice().sweep_values(compactions);
        exception_jump_target_stack
            .as_mut_slice()
            .sweep_values(compactions);
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
