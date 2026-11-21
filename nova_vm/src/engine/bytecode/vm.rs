// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod binding_methods;
mod execute_instructions;

use execute_instructions::*;

use std::rc::Rc;

#[cfg(feature = "array-buffer")]
use crate::ecmascript::{AnyTypedArray, TypedArrayAbstractOperations, TypedArrayPrototype};
use crate::{
    ecmascript::{
        Agent, ArgumentsList, BUILTIN_STRING_MEMORY, Behaviour, BigInt, CachedSet,
        ECMAScriptFunction, Environment, ExceptionType, Function, FunctionEnvironment,
        FunctionPrototype, Generator, GeneratorPrototype, GeneratorState, InternalMethods,
        InternalSlots, IteratorRecord, JsError, JsResult, Number, Object, OrdinaryObject,
        Primitive, Promise, PropertyKey, PropertyLookupCache, Reference, RegularFn, SmallF64,
        SmallInteger, String, SuspendedGeneratorState, TryGetResult, Value, binding_value_at_site,
        call_function, get_method, get_this_environment, get_this_value, is_callable,
        is_property_reference, is_same_type, is_strictly_equal, name_is_bound_at_global_site,
        ordinary_add_own_data_by_cache, ordinary_call_bind_this, ordinary_has_instance,
        prepare_for_call_without_function_environment, prepare_for_ordinary_call,
        put_value_at_global_site, release_scoped_collections, scope_arguments_of_call,
        set_binding_value_at_site, take_pooled_arguments_vec, to_boolean, to_numeric,
        to_numeric_primitive, to_primitive, to_property_key, to_property_key_simple,
        to_string_primitive, try_get_object_method_by_cache, try_resolve_binding_at_site,
        try_result_into_option_js,
    },
    engine::{
        Bindable, GcScope, NoGcScope, Scopable, Scoped,
        bytecode::{
            CallEnvironment, CodeTables, Executable, IndexType, Instruction, InstructionIter,
            TablesView,
            instructions::Instr,
            iterator::{VmIteratorRecord, step_value_of_result},
        },
    },
    heap::{
        ArenaAccess, ArenaAccessMut, CompactionLists, HeapMarkAndSweep, WellKnownSymbols,
        WorkQueues,
    },
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
        /// Whether `yielded_value` is the value `yield` was given, with no
        /// iterator result object around it yet
        /// ([`Instruction::YieldValue`]).
        plain: bool,
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
    YieldValue,
    Await,
    /// `super(…)`, not executed: the `Vm` may run the parent constructor
    /// itself, and executes the instruction where it does not.
    Super,
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
    /// Not executed and nothing changed: `new` of an ECMAScript function,
    /// which the Vm may run itself.
    Construct,
    /// Not executed: a read or an assignment of a property that is an
    /// accessor. Its getter or setter is on the stack (under the value
    /// assigned, for a setter) and the object is the result: a call of
    /// the function with the object as its this value, which the Vm may
    /// run itself.
    Accessor,
    /// Not executed and nothing changed: the instruction that ends a
    /// function.
    Return,
    /// Not executed and nothing changed: the step of an iteration over a
    /// generator, which the Vm may resume itself.
    Resume,
    /// Not executed and nothing changed: a `yield`, which suspends the
    /// generator where the Vm resumed it itself.
    Yield,
}

/// What the code that a function is run in place for makes of what the
/// function returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Completion {
    /// The result of the call.
    Value,
    /// Nothing: the function is a setter, run in place of an assignment.
    Discarded,
    /// The result of `new`, where it is an object; where it is not, the
    /// object that was made for the constructor, which waits on top of the
    /// caller's stack.
    Constructed,
    /// The result of `new` of a derived class constructor: the object it
    /// returned, or the this value that a `super(…)` in it bound, which
    /// its Function Environment Record has ([`Frame::derived`]).
    ConstructedDerived,
    /// The result of `super(…)` whose parent is no derived class
    /// constructor: as [`Completion::Constructed`], and then the this value
    /// of the constructor that called `super`.
    SuperConstructed,
    /// The result of `super(…)` whose parent is a derived class
    /// constructor: as [`Completion::ConstructedDerived`], and then the
    /// this value of the constructor that called `super`.
    SuperConstructedDerived,
    /// The end of an iteration: what ran from the frame is a generator
    /// that a step of the iteration resumed ([`Frame::resumed`]), and it
    /// is done.
    Stepped,
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
    /// How many of the exception handlers on their stack are not of the
    /// code that is running, but of the code that called it in place, and
    /// of what called that: an error that the running code throws goes to
    /// the handlers above these, and with none there leaves the function.
    handler_base: usize,
}

/// What a `Vm` puts aside of the code that calls a function it runs itself,
/// and takes back when the function returns or throws: the caller's stacks,
/// where it goes on, and its code.
#[derive(Debug)]
struct Frame {
    ip: usize,
    /// The caller's value stack while the frame is in use; an empty stack
    /// for the next function at this depth while it is not. A function has
    /// a value stack of its own because its instructions name the slots
    /// of its variables from the bottom of it.
    stack: Vec<Value<'static>>,
    /// The other three stacks are the caller's and the function's at once:
    /// the function pushes to them above what the caller left there, and
    /// balances what it pushes or leaves it to be cut off here when it is
    /// left. How long the Reference stack and the iterator stack were when
    /// the function was entered, and the caller's own
    /// [`Vm::handler_base`].
    reference_base: usize,
    iterator_base: usize,
    handler_base: usize,
    /// The caller's executable, which the collector keeps current here
    /// while the frame is in use.
    executable: Executable<'static>,
    /// The caller's instructions and tables while the frame is in use: see
    /// [`Running::tables`].
    tables: Option<Rc<CodeTables>>,
    /// What the caller makes of what the function that runs from this
    /// frame returns.
    completion: Completion,
    /// What the Agent's scoped values and scoped collections are cut back
    /// to after each of the caller's instructions.
    stack_depth: usize,
    collections_depth: usize,
    /// The rooted handle of the caller's executable. It stays where it is
    /// among the Agent's scoped values while the function runs (what the
    /// function's instructions scope is above it), and is the caller's
    /// again when the function is left.
    scoped: Scoped<'static, Executable<'static>>,
    /// The generator that runs from this frame, resumed in place of the
    /// step of an iteration ([`Vm::enter_generator`]); None for the frame
    /// of a call.
    resumed: Option<Generator<'static>>,
    /// The record of the suspended state of the generator that runs from
    /// this frame, which waits here for the generator to yield: the value
    /// stack in it is the frame's spare one (the generator's own is the
    /// `Vm`'s meanwhile), its other three stacks are empty (what they held
    /// is on the `Vm`'s, above what the caller has there), and its
    /// execution context is the running one. Nothing in it is for the
    /// collector to see: its executable is written again when the
    /// generator yields. None for the frame of a call.
    kept: Option<Box<SuspendedGeneratorState>>,
    /// Where the iteration goes when the generator that runs from this
    /// frame is done.
    done: usize,
    /// Whether the caller's Reference register waits on the Reference
    /// stack, right under [`Frame::reference_base`], while a generator runs
    /// from this frame: the step of an iteration leaves it as it was.
    reference_kept: bool,
    /// The Function Environment Record of the derived class constructor
    /// that runs from this frame, which has the this value that a
    /// `super(…)` binds: asked for it when the constructor returns. None
    /// for the frame of anything else.
    derived: Option<FunctionEnvironment<'static>>,
}

/// What [`Vm::leave_function`] has to finish once the frame is left, where
/// the frame was not that of a plain call.
struct Left {
    /// The generator that a step of an iteration resumed, where the loop
    /// goes when it is done, and the caller's Reference register.
    stepped: Option<(Generator<'static>, usize, Option<Reference<'static>>)>,
    settled: Settled,
    /// The record of a derived class constructor, and whether `super(…)`
    /// called it.
    derived: Option<(Option<FunctionEnvironment<'static>>, bool)>,
}

impl Frame {
    /// A frame that is not in use yet, for the code `executable`.
    fn new(executable: Executable<'static>, scoped: Scoped<'static, Executable<'static>>) -> Self {
        Self {
            ip: 0,
            stack: Vec::with_capacity(32),
            reference_base: 0,
            iterator_base: 0,
            handler_base: 0,
            executable,
            tables: None,
            completion: Completion::Value,
            stack_depth: 0,
            collections_depth: 0,
            scoped,
            resumed: None,
            kept: None,
            done: 0,
            reference_kept: false,
            derived: None,
        }
    }
}

/// The code that is running in a `Vm`, between two changes of it: the code
/// the `Vm` was started with, or a function that a call of it, or of a
/// function it called, runs in place of the call.
struct Running<'s> {
    executable: Scoped<'s, Executable<'static>>,
    /// The executable itself, as it is when the code starts to run or goes
    /// on after a call: what `executable` gives until a collection moves
    /// it. The loop of instructions starts from it, and asks the rooted
    /// handle only after a collection.
    code: Executable<'static>,
    /// The instructions of the executable with its constants, property
    /// caches and binding sites, held with it so that the loop reads them
    /// without the heap.
    tables: Rc<CodeTables>,
    /// What the Agent's scoped values and scoped collections are cut back
    /// to after each instruction.
    stack_depth: usize,
    collections_depth: usize,
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
    /// The step of an iteration over a generator that the `Vm` resumes
    /// itself.
    Resume(Generator<'static>, Instr),
    /// The generator that is resumed in place of a step yields: the value
    /// that `yield` was given where this is true, an iterator result
    /// object where it is not ([`ExecutionResult::Yield`]).
    Yield(bool),
}

/// The most arguments that a rest parameter takes into its Array in the
/// loop of instructions: of a longer list the general way makes it.
const REST_IN_PLACE: usize = 16;

/// A function that a call instruction calls and the `Vm` runs itself.
#[derive(Clone, Copy)]
struct CalledInPlace {
    function: ECMAScriptFunction<'static>,
    executable: Executable<'static>,
    /// How many parameters the function keeps in its first stack slots,
    /// where the call puts the arguments. None for a function that binds
    /// its parameters with its own instructions (a default value, a rest
    /// parameter, a pattern, an arguments object): the call leaves it the
    /// list of arguments.
    parameters: Option<usize>,
    /// The environment that the call makes for the function.
    environment: CallEnvironment,
    /// See [`Frame::completion`].
    completion: Completion,
}

/// What is left to do in the calling code when a function run in place
/// for `new` or `super(…)` has returned ([`Vm::leave_function`]).
enum Settled {
    /// Nothing: the result is the result.
    Done,
    /// The result is the this value of the constructor that called
    /// `super(…)`.
    Super,
    /// A derived class constructor returned what is neither an object nor
    /// undefined; the value is the result.
    NotAnObject,
    /// A derived class constructor returned without a this value: no
    /// `super(…)` of it had returned.
    NoThisValue,
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

/// Cuts a stack of a generator that yields where it was resumed in place
/// down to twice what it holds, as [`fit_suspended_stack`] does, but for
/// its room when it holds nothing: the generator that pushes to it between
/// two `yield`s would allocate it at every step.
fn refit_suspended_stack<T>(stack: &mut Vec<T>) {
    let room = (stack.len() * 2).max(SUSPENDED_STACK_ROOM);
    if stack.capacity() > room + SUSPENDED_STACK_ROOM {
        stack.shrink_to(room);
    }
}

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
    /// What is suspended, taken from where it was: nothing is left there.
    pub(crate) fn take(&mut self) -> Self {
        core::mem::replace(self, Self::EMPTY)
    }

    /// Nothing suspended: no stack holds or has room for anything.
    const EMPTY: Self = Self {
        ip: 0,
        stack: Vec::new(),
        reference_stack: Vec::new(),
        iterator_stack: Vec::new(),
        exception_jump_target_stack: Vec::new(),
    };

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

/// A function that native code calls again and again: the callback of an
/// Array method, called for every element; a comparator. \[\[Call]] of an
/// ECMAScript function from native code makes a `Vm` for the call and
/// lets go of it after, finds out each time what kind of function it is
/// and how its code takes its arguments, and roots its executable; here
/// that is done once, and each call is the execution context of the call
/// (PrepareForOrdinaryCall and OrdinaryCallBindThis, as for any call), the
/// arguments put where the function's parameters are, and the code run.
///
/// For a function that the interpreter could run in place of a call
/// instruction with its arguments in its first stack slots: an ordinary
/// function (not a generator, not async, not a class constructor) that is
/// compiled, keeps its parameters on the stack, and needs no Function
/// Environment Record. Any other is called the general way by whoever
/// would have used this.
pub(crate) struct RepeatedCall<'s> {
    function: Scoped<'s, ECMAScriptFunction<'static>>,
    executable: Scoped<'s, Executable<'static>>,
    /// How many parameters the function keeps in its first stack slots.
    parameters: usize,
    /// The function's bindings need a Declarative Environment Record.
    declarative_environment: bool,
    /// The `Vm` of every call: empty between two of them.
    vm: Vm,
}

impl<'s> RepeatedCall<'s> {
    /// None where `function` is not one that this is for.
    pub(crate) fn new(
        agent: &mut Agent,
        function: Function,
        gc: NoGcScope<'_, 's>,
    ) -> Option<Self> {
        let Function::ECMAScriptFunction(function) = function else {
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
        let parameters = usize::from(code.stack_parameters?);
        let declarative_environment = match code.call_environment {
            CallEnvironment::Function => return None,
            environment => environment == CallEnvironment::Declarative,
        };
        let function = function.scope(agent, gc);
        let executable = executable.scope(agent, gc);
        Some(Self {
            function,
            executable,
            parameters,
            declarative_environment,
            vm: Vm::new(agent),
        })
    }

    /// Call(F, thisArgument, arguments).
    pub(crate) fn call<'gc>(
        &mut self,
        agent: &mut Agent,
        this_argument: Value,
        arguments: &[Value],
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        agent.check_call_depth(gc.nogc()).unbind()?;
        // 2. Let calleeContext be PrepareForOrdinaryCall(F, undefined).
        // 5. Perform OrdinaryCallBindThis(F, calleeContext, thisArgument).
        prepare_for_call_without_function_environment(
            agent,
            self.function.get(agent),
            this_argument,
            self.declarative_environment,
            gc.nogc(),
        );
        // The arguments go where the function keeps its parameters:
        // undefined for those not given, and those beyond the parameters
        // are dropped.
        let vm = &mut self.vm;
        vm.stack.extend((0..self.parameters).map(|index| {
            arguments
                .get(index)
                .map_or(Value::Undefined, |argument| argument.unbind())
        }));
        // 6. Let result be Completion(OrdinaryCallEvaluateBody(F,
        //    argumentsList)).
        let result = vm.run(agent, self.executable.clone(), gc);
        vm.reset();
        // 7. Remove calleeContext from the execution context stack and
        //    restore callerContext as the running execution context.
        agent.remove_execution_context();
        // 8. If result is a return completion, return result.[[Value]].
        // 9. ReturnIfAbrupt(result).
        result.into_js_result()
    }

    /// Gives the stacks of the `Vm` back for other code to run with.
    pub(crate) fn finish(self, agent: &mut Agent) {
        self.vm.recycle(agent);
    }
}

/// The callback of a built-in that calls it again and again (once for every
/// element, for every comparison): called through a [`RepeatedCall`] where
/// it is a function that is for, and the general way where it is not.
pub(crate) struct Callback<'s> {
    function: Scoped<'s, Function<'static>>,
    repeated: Option<RepeatedCall<'s>>,
}

impl<'s> Callback<'s> {
    pub(crate) fn new(
        agent: &mut Agent,
        function: Scoped<'s, Function<'static>>,
        gc: NoGcScope<'_, 's>,
    ) -> Self {
        let repeated = RepeatedCall::new(agent, function.get(agent), gc);
        Self { function, repeated }
    }

    /// Call(callbackfn, thisArgument, arguments). The arguments are not
    /// to be read after it.
    #[inline]
    pub(crate) fn call<'gc>(
        &mut self,
        agent: &mut Agent,
        this_argument: Value,
        arguments: &mut [Value<'static>],
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        match &mut self.repeated {
            Some(repeated) => repeated.call(agent, this_argument, arguments, gc),
            None => call_function(
                agent,
                self.function.get(agent),
                this_argument,
                Some(ArgumentsList::from_mut_slice(arguments)),
                gc,
            ),
        }
    }

    /// The built-in is done calling: the stacks of the `Vm` it called with
    /// are given back. One that is dropped instead (the built-in threw)
    /// lets go of them.
    pub(crate) fn finish(self, agent: &mut Agent) {
        if let Some(repeated) = self.repeated {
            repeated.finish(agent);
        }
    }
}

/// The frames of every `Vm` of an Agent that runs functions in place of
/// their calls: those of the `Vm` that is running are the last in use,
/// under them those of the `Vm`s that wait for a call it is inside of. A
/// `Vm` leaves none in use when it is done or suspended.
///
/// The frames after those in use are kept: each holds the emptied stack
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
                reference_base: _,
                iterator_base: _,
                handler_base: _,
                executable,
                tables: _,
                completion: _,
                stack_depth: _,
                collections_depth: _,
                scoped: _,
                resumed,
                // Nothing to see while the frame is in use: see the field.
                kept: _,
                done: _,
                reference_kept: _,
                derived,
            } = frame;
            stack.as_slice().mark_values(queues);
            executable.mark_values(queues);
            resumed.mark_values(queues);
            derived.mark_values(queues);
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        for frame in &mut self.frames[..self.in_use] {
            let Frame {
                ip: _,
                stack,
                reference_base: _,
                iterator_base: _,
                handler_base: _,
                executable,
                tables: _,
                completion: _,
                stack_depth: _,
                collections_depth: _,
                scoped: _,
                resumed,
                kept: _,
                done: _,
                reference_kept: _,
                derived,
            } = frame;
            stack.as_mut_slice().sweep_values(compactions);
            executable.sweep_values(compactions);
            resumed.sweep_values(compactions);
            derived.sweep_values(compactions);
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
        handler_base: 0,
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
            handler_base: 0,
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
            handler_base: _,
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
            handler_base: _,
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
            handler_base: 0,
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
        let iter = InstructionIter::new(instructions.code());
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

    /// Leaves the `Vm` as a new one is, with its stacks emptied: what the
    /// code that ran left on them (the values of its variables; an
    /// iterator or a handler it was thrown out of) is no one's.
    fn reset(&mut self) {
        debug_assert_eq!(self.depth, 0);
        debug_assert_eq!(self.handler_base, 0);
        self.ip = 0;
        self.result = None;
        self.reference = None;
        self.stack.clear();
        self.reference_stack.clear();
        self.iterator_stack.clear();
        self.exception_handler_stack.clear();
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
            code: base_executable.get(agent),
            tables: base_executable.get_tables(agent),
            stack_depth,
            collections_depth,
        };
        loop {
            // The executable is looked up through its rooted handle after
            // every collection, which is all that moves it, not by every
            // instruction that fetches a constant, a name or a cache; and
            // not here, where the code that starts or goes on comes with
            // it.
            let Running {
                executable,
                mut code,
                tables: code_tables,
                stack_depth,
                collections_depth,
            } = running;
            // The instruction pointer is a local across the plain
            // instructions, which only jumps move: read and written in the
            // Vm between every two of them, it was the longest step of going
            // from one to the next. It is in the Vm, where everything else
            // looks for it, around every instruction that is not plain and
            // whenever the loop is left.
            let mut ip = self.ip;
            let mut collections = agent.heap.collections;
            // The code as the loop reads an instruction from it with one
            // look at how much is left: an instruction and the eight bytes
            // after it, the longest there is, which the padding after the
            // code gives its last instruction too (the Return of a
            // function). An instruction of four arguments has its third
            // and fourth in that window, and its arm reads them there.
            // None of it when the instructions are printed, so that every
            // one of them goes the other way, where the end of the code
            // goes.
            let instructions = code_tables.padded();
            let windows: &[u8] = if print_internals { &[] } else { instructions };
            // The tables as three slices, which stay what they are while
            // this code runs: a collection rewrites their entries in place.
            let tables = code_tables.view();
            let change = loop {
                let (instr, window) = if let Some(window) = windows
                    .get(ip..)
                    .and_then(|rest| rest.first_chunk::<{ Instr::WINDOW }>())
                {
                    (Instr::in_window(window), window)
                } else {
                    let mut next = ip;
                    let Some(instr) =
                        Instr::consume_instruction(code_tables.instructions(), &mut next)
                    else {
                        break Change::End;
                    };
                    if print_internals {
                        Self::print_executing(instr.kind);
                    }
                    // An instruction of the code has its window in the
                    // padded code, whichever way it was read.
                    let Some(window) = instructions
                        .get(ip..)
                        .and_then(|rest| rest.first_chunk::<{ Instr::WINDOW }>())
                    else {
                        break Change::End;
                    };
                    (instr, window)
                };
                // Most of what runs is plain: it goes straight to the next
                // instruction, with no result to look at and nothing to
                // release. Every instruction leaves `ip` after itself
                // there, or where it jumps.
                match Self::execute_plain_instruction(
                    agent,
                    self,
                    &mut ip,
                    window,
                    &executable,
                    code,
                    tables,
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
                        if let Some(callee) =
                            self.bound_function_called_in_place(agent, instr, gc.nogc())
                        {
                            break Change::Call(callee, instr);
                        }
                    }
                    // `new` of a function that this Vm runs itself.
                    Plain::Construct => {
                        if let Some((callee, call)) =
                            self.constructor_called_in_place(agent, instr, gc.nogc())
                        {
                            break Change::Call(callee, call);
                        }
                    }
                    // The getter or setter of the property that the
                    // instruction reads or assigns, called in place of it.
                    Plain::Accessor => {
                        if let Some((callee, call)) =
                            self.accessor_called_in_place(agent, instr.kind)
                        {
                            break Change::Call(callee, call);
                        }
                    }
                    // The step of an iteration over a generator that this
                    // Vm resumes itself.
                    Plain::Resume => {
                        if let Some(generator) = self.generator_resumed_in_place(agent, gc.nogc()) {
                            break Change::Resume(generator, instr);
                        }
                    }
                    // A `yield` of the generator that this Vm resumed
                    // itself, in place of the step of an iteration: it is
                    // suspended where the loop is left, without the way
                    // through the general instructions, which only says
                    // that it is a `yield`.
                    Plain::Yield => {
                        if self.runs_resumed_generator(agent) {
                            self.ip = ip;
                            break Change::Yield(instr.kind == Instruction::YieldValue);
                        }
                    }
                    // The end of every call, taken here where nothing is
                    // out of the ordinary: a termination that is pending
                    // goes the general way below.
                    Plain::Return => {
                        if agent.termination().is_none() {
                            self.ip = ip;
                            if print_internals {
                                Self::print_exiting();
                            }
                            if self.depth == 0 {
                                let result = self.result.unwrap_or(Value::Undefined);
                                return ExecutionResult::Return(result.bind(gc.into_nogc()));
                            }
                            break Change::Return;
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
                    // `super(…)`: its parent constructor is run by this Vm
                    // or the instruction is executed now. Seen here,
                    // among what is rare: looked for among the plain
                    // instructions, or before every one that is not, it
                    // moved the loop of instructions.
                    Ok(ContinuationKind::Super) => {
                        if let Some((callee, call)) =
                            self.super_called_in_place(agent, instr, gc.nogc())
                        {
                            break Change::Call(callee, call);
                        }
                        if let Err(err) =
                            execute_evaluate_super(agent, self, instr, gc.reborrow()).unbind()
                            && !self.handle_error(agent, err)
                        {
                            break Change::Throw(err);
                        }
                    }
                    Ok(kind) => {
                        if self.depth != 0 {
                            // The generator that a step of an iteration
                            // resumed in place yields.
                            if let ContinuationKind::Yield | ContinuationKind::YieldValue = kind
                                && self.runs_resumed_generator(agent)
                            {
                                break Change::Yield(matches!(kind, ContinuationKind::YieldValue));
                            }
                            // Nothing else that runs in place yields or
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
                code,
                tables: code_tables,
                stack_depth,
                collections_depth,
            };
            match change {
                Change::Call(callee, instr) => {
                    running = self.enter_function(agent, instr, callee, ip, running, gc.nogc());
                    if print_internals {
                        self.print_internals(agent, running.executable.clone(), gc.nogc());
                    }
                    // As for the code a Vm is started with: the host is
                    // asked, and a collection that is due is run.
                    if let Err(err) = agent.poll_interrupt_steps(STEPS_PER_TURN, gc.nogc()) {
                        thrown = Some(err.unbind());
                    } else if agent.check_gc() {
                        self.trigger_gc(agent, gc.reborrow());
                        running.code = running.executable.get(agent);
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
                    let (caller, settled) = self.leave_function(agent, running, true);
                    running = caller;
                    if !matches!(settled, Settled::Done) {
                        // The rest of `new` of a derived class or of
                        // `super(…)`, in the calling code: its handlers
                        // have what this throws.
                        thrown = self.finish_construction(agent, settled, gc.reborrow());
                        running.code = running.executable.get(agent);
                    }
                }
                Change::Throw(error) => thrown = Some(error),
                Change::Terminate => {
                    while self.depth != 0 {
                        running = self.leave_function_thrown(agent, running);
                    }
                    return ExecutionResult::Throw(JsError::new(Value::Undefined));
                }
                Change::Resume(generator, instr) => {
                    running = self.enter_generator(agent, instr, generator, ip, running, gc.nogc());
                    // As for the code a Vm is started with, which a
                    // generator resumed by a call of its own is: the host
                    // is asked, and a collection that is due is run.
                    if let Err(err) = agent.poll_interrupt_steps(STEPS_PER_TURN, gc.nogc()) {
                        thrown = Some(err.unbind());
                    } else if agent.check_gc() {
                        self.trigger_gc(agent, gc.reborrow());
                        running.code = running.executable.get(agent);
                    }
                }
                Change::Yield(plain) => {
                    let yielded = self.result.take().expect("a generator yields a value");
                    if print_internals {
                        Self::print_yielding(yielded);
                    }
                    let (caller, done) = self.suspend_generator(agent, ip, running);
                    running = caller;
                    if plain {
                        // The value of the step.
                        self.result = Some(yielded);
                    } else {
                        // What `yield*` passes on is an iterator result
                        // object already, which the step reads as any
                        // other's: in the calling code, whose handlers an
                        // error of that reading goes to.
                        let step = with_vm_gc(
                            agent,
                            self,
                            |agent, gc| Self::step_value_of_yielded(agent, yielded, gc),
                            gc.reborrow(),
                        )
                        .unbind();
                        running.code = running.executable.get(agent);
                        match step {
                            Ok(Some(value)) => self.result = Some(value),
                            Ok(None) => {
                                self.result = None;
                                self.ip = done;
                            }
                            Err(error) => {
                                if !self.handle_error(agent, error) {
                                    thrown = Some(error);
                                }
                            }
                        }
                    }
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
                    running = self.leave_function_thrown(agent, running);
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
    /// a class constructor) that has been compiled, called with a number
    /// of arguments that the instruction states, where one more call is
    /// not one too many.
    // Part of the loop of instructions, for every call there, whoever
    // else asks: as a function of its own it was a twentieth of a call.
    #[inline(always)]
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
        if agent.calls_in_place_exceeded() {
            return None;
        }
        let code = executable.get(agent);
        Some(CalledInPlace {
            function,
            executable,
            parameters: code.stack_parameters.map(usize::from),
            environment: code.call_environment,
            completion: Completion::Value,
        })
    }

    /// The call of a bound function, if this `Vm` runs the function it was
    /// bound from itself: \[\[Call]] of a bound function is a call of that
    /// function with the bound this value, and the arguments after the bound
    /// ones. For a bound function with no bound arguments, whose target is
    /// a function that [`Vm::function_called_in_place`] takes. The this
    /// value of a call is the base of its Reference: the Reference of this
    /// call becomes one of the bound this value. Nothing has changed where
    /// this returns None.
    // Not part of the loop of instructions: few instructions end here.
    #[inline(never)]
    fn bound_function_called_in_place(
        &mut self,
        agent: &Agent,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<CalledInPlace> {
        let count = instr.get_first_index();
        if count == IndexType::MAX as usize {
            return None;
        }
        let at = self.stack.len().checked_sub(count + 1)?;
        let Value::BoundFunction(bound) = self.stack[at] else {
            return None;
        };
        if bound.bound_arguments(agent).len() != 0 {
            return None;
        }
        let Function::ECMAScriptFunction(function) = bound.bound_target_function(agent) else {
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
        if agent.calls_in_place_exceeded() {
            return None;
        }
        let code = executable.get(agent);
        // Only the base of the Reference is read, as the this value.
        let key = BUILTIN_STRING_MEMORY.prototype.to_property_key();
        let reference =
            Reference::new_property_reference(bound.bound_this(agent).bind(gc), key, false)
                .unbind();
        if instr.kind == Instruction::EvaluateCallOnThis {
            // The this value of this call is under the function: the bound
            // one takes its place.
            *self.stack.get_mut(at.checked_sub(1)?)? = bound.bound_this(agent).unbind();
        } else if instr.kind == Instruction::PopReferenceAndEvaluateCall {
            *self.reference_stack.last_mut()? = reference;
        } else {
            self.reference = Some(reference);
        }
        Some(CalledInPlace {
            function: function.unbind(),
            executable,
            parameters: code.stack_parameters.map(usize::from),
            environment: code.call_environment,
            completion: Completion::Value,
        })
    }

    /// The call that `new F(…)` comes to, if this `Vm` runs the constructor
    /// itself: \[\[Construct]] of a function that is not a derived class
    /// constructor, with itself as the new target, up to the function's
    /// code. The object that OrdinaryCreateFromConstructor makes is the
    /// this value of the call, and waits on the caller's stack, under the
    /// function and its arguments, for the constructor to return
    /// ([`Completion::Constructed`]).
    ///
    /// Not for a constructor whose code needs a Function Environment
    /// Record (it reads new.target or super, an arrow function in it uses
    /// its this, or it has a direct eval): \[\[Construct]] gives that
    /// record the new target. And not where the function's "prototype" is
    /// not an object found without running anything: the object's
    /// prototype is then the realm's, which the general way knows. Nothing
    /// has changed where this returns None.
    // Not part of the loop of instructions: few instructions end here.
    #[inline(never)]
    fn constructor_called_in_place(
        &mut self,
        agent: &mut Agent,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<(CalledInPlace, Instr)> {
        let count = instr.get_first_index();
        let at = self.stack.len().checked_sub(count + 1)?;
        let Value::ECMAScriptFunction(function) = self.stack[at] else {
            return None;
        };
        let data = function.get(agent);
        let status = data.ecmascript_function.constructor_status;
        if !status.is_constructor() {
            return None;
        }
        let executable = data.compiled_bytecode?;
        if agent.calls_in_place_exceeded() {
            return None;
        }
        let code = executable.get(agent);
        let parameters = code.stack_parameters.map(usize::from);
        let environment = code.call_environment;
        if status.is_derived_class() {
            // 4. Let calleeContext be PrepareForOrdinaryCall(F, newTarget):
            // no object is made, and the this value is the one a
            // `super(…)` binds. The function is its own new target.
            if environment != CallEnvironment::Function {
                return None;
            }
            self.result = None;
            self.reference = None;
            return Some((
                CalledInPlace {
                    function,
                    executable,
                    parameters,
                    environment,
                    completion: Completion::ConstructedDerived,
                },
                Instr::new_with_arg(Instruction::EvaluateCall, count as u16),
            ));
        }
        if environment == CallEnvironment::Function {
            return None;
        }
        // 3. a. Let thisArgument be ? OrdinaryCreateFromConstructor(newTarget,
        //       "%Object.prototype%").
        let key = BUILTIN_STRING_MEMORY.prototype.to_property_key();
        let cache = PropertyLookupCache::new(agent, key);
        // The function's own "prototype", read as a property is read by
        // its cache in the loop of instructions: one look at the storage
        // of the function's properties. Where that does not answer (the
        // cache has not seen the shape, or the property is none of the
        // function's own values), Get does, and the cache sees the shape.
        let prototype = match function
            .get_backing_object(agent)
            .and_then(|properties| properties.value_by_cache(agent, cache, gc))
        {
            Some(prototype) => prototype,
            None => {
                let core::ops::ControlFlow::Continue(TryGetResult::Value(prototype)) =
                    crate::ecmascript::try_get(agent, function, key, Some(cache), gc)
                else {
                    return None;
                };
                prototype.unbind()
            }
        };
        // An ordinary object, nearly always: asked first, and what other
        // kind of object it is only then.
        let prototype = match prototype {
            Value::Object(prototype) => Object::Object(prototype),
            prototype => Object::try_from(prototype).ok()?,
        };
        let object: Value = Self::object_with_prototype(agent, prototype, gc)?.into();
        self.stack.insert(at, object.unbind());
        self.result = None;
        // 6. a. Perform OrdinaryCallBindThis(F, calleeContext, thisArgument):
        // the this value of the call is the base of its Reference.
        self.reference = Some(Reference::new_property_reference(object, key, false).unbind());
        Some((
            CalledInPlace {
                function,
                executable,
                parameters,
                environment,
                completion: Completion::Constructed,
            },
            Instr::new_with_arg(Instruction::EvaluateCall, count as u16),
        ))
    }

    /// OrdinaryObjectCreate(`prototype`): the object that `new` of a
    /// function of the script starts from. Made with the shape of the
    /// objects that have this prototype and nothing else, which is what
    /// the general way arrives at by making an object with the Realm's
    /// %Object.prototype% and then giving it this one: two shapes looked
    /// up by their prototypes for every object made, and the prototype's
    /// own shape seen to each time, where the first object made with a
    /// prototype sees to it once. None where there is no room for it,
    /// which the general way reports.
    fn object_with_prototype<'gc>(
        agent: &mut Agent,
        prototype: Object,
        gc: NoGcScope<'gc, '_>,
    ) -> Option<Object<'gc>> {
        let object = OrdinaryObject::create_object(agent, Some(prototype.unbind()), &[]).ok()?;
        Some(Object::Object(object).bind(gc))
    }

    /// The call that `super(…)` comes to, if this `Vm` runs the parent
    /// constructor itself: the steps of SuperCall up to the parent's code,
    /// where none of them runs anything. The parent is the prototype of
    /// the function whose Function Environment Record gives `this` here,
    /// and the new target is that record's.
    ///
    /// A parent that is no derived class constructor is run as `new` of it
    /// is ([`Vm::constructor_called_in_place`]), with the object made from
    /// the prototype of the new target, and under the same conditions; one
    /// that is a derived class constructor is run with the new target and
    /// no this value. What is left of SuperCall when the parent returns
    /// (the this value bound, the instance elements initialized) is done
    /// then ([`Settled::Super`]). A built-in parent, a bound function or a
    /// Proxy, a parent that is no constructor, and arguments that are
    /// spread go the general way. Nothing has changed where this returns
    /// None.
    // Not part of the loop of instructions: few instructions end here.
    #[inline(never)]
    fn super_called_in_place(
        &mut self,
        agent: &mut Agent,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<(CalledInPlace, Instr)> {
        let count = instr.get_first_index();
        if count == IndexType::MAX as usize {
            return None;
        }
        let at = self.stack.len().checked_sub(count)?;
        let Environment::Function(this_env) = get_this_environment(agent, gc) else {
            return None;
        };
        // 1. Let newTarget be GetNewTarget().
        let new_target = Function::try_from(this_env.get_new_target(agent)?).ok()?;
        // 3. Let func be GetSuperConstructor().
        let core::ops::ControlFlow::Continue(Some(Object::ECMAScriptFunction(parent))) = this_env
            .get_function_object(agent)
            .try_get_prototype_of(agent, gc)
        else {
            return None;
        };
        // 5. If IsConstructor(func) is false, throw a TypeError exception.
        let parent = parent.unbind();
        let data = parent.get(agent);
        let status = data.ecmascript_function.constructor_status;
        if !status.is_constructor() {
            return None;
        }
        let executable = data.compiled_bytecode?.unbind();
        if agent.calls_in_place_exceeded() {
            return None;
        }
        let code = executable.get(agent);
        let parameters = code.stack_parameters.map(usize::from);
        let environment = code.call_environment;
        let call = Instr::new_with_arg(Instruction::EvaluateCall, count as u16);
        // 6. Let result be ? Construct(func, argList, newTarget).
        if status.is_derived_class() {
            if environment != CallEnvironment::Function {
                return None;
            }
            // The parent under its arguments, where a call has its
            // function.
            self.stack.insert(at, parent.unbind().into());
            self.result = None;
            self.reference = None;
            return Some((
                CalledInPlace {
                    function: parent.unbind(),
                    executable,
                    parameters,
                    environment,
                    completion: Completion::SuperConstructedDerived,
                },
                call,
            ));
        }
        if environment == CallEnvironment::Function {
            return None;
        }
        // OrdinaryCreateFromConstructor(newTarget, "%Object.prototype%").
        let key = BUILTIN_STRING_MEMORY.prototype.to_property_key();
        let cache = PropertyLookupCache::new(agent, key);
        // The new target's own "prototype" by its cache, as `new` of a
        // function reads it ([`Vm::constructor_called_in_place`]).
        let own = match new_target {
            Function::ECMAScriptFunction(new_target) => new_target
                .get_backing_object(agent)
                .and_then(|properties| properties.value_by_cache(agent, cache, gc)),
            _ => None,
        };
        let prototype = match own {
            Some(prototype) => prototype,
            None => {
                let core::ops::ControlFlow::Continue(TryGetResult::Value(prototype)) =
                    crate::ecmascript::try_get(agent, new_target, key, Some(cache), gc)
                else {
                    return None;
                };
                prototype.unbind()
            }
        };
        // An ordinary object, nearly always: asked first, and what other
        // kind of object it is only then.
        let prototype = match prototype {
            Value::Object(prototype) => Object::Object(prototype),
            prototype => Object::try_from(prototype).ok()?,
        };
        let object: Value = Self::object_with_prototype(agent, prototype, gc)?.into();
        // The object and the parent under the arguments, as `new` of the
        // parent leaves them.
        self.stack.insert(at, parent.unbind().into());
        self.stack.insert(at, object.unbind());
        self.result = None;
        self.reference = Some(Reference::new_property_reference(object, key, false).unbind());
        Some((
            CalledInPlace {
                function: parent.unbind(),
                executable,
                parameters,
                environment,
                completion: Completion::SuperConstructed,
            },
            call,
        ))
    }

    /// The new target that a derived class constructor is entered with:
    /// itself for `new` of it; for `super(…)`, the new target of the
    /// constructor that calls it, which is the running code's still.
    #[inline(never)]
    fn derived_new_target<'a>(
        agent: &Agent,
        callee: CalledInPlace,
        gc: NoGcScope<'a, '_>,
    ) -> Option<Object<'a>> {
        if callee.completion == Completion::ConstructedDerived {
            return Some(Object::from(callee.function).bind(gc));
        }
        let Environment::Function(this_env) = get_this_environment(agent, gc) else {
            unreachable!("`super(…)` was seen to have a Function Environment Record")
        };
        this_env.get_new_target(agent)
    }

    /// What a derived class constructor that was run in place and has
    /// returned is constructed to: \[\[Construct]] steps 10 and 12, with the
    /// this value asked of the constructor's Function Environment Record
    /// `environment`. The object is the result; an error that those steps
    /// throw is left for the calling code to throw ([`Settled`]).
    #[inline(never)]
    fn settle_derived(
        &mut self,
        agent: &Agent,
        environment: Option<FunctionEnvironment<'static>>,
        for_super: bool,
    ) -> Settled {
        let returned = self.result.unwrap_or(Value::Undefined);
        let settled = if for_super {
            Settled::Super
        } else {
            Settled::Done
        };
        // 10. a. If result.[[Value]] is an Object, return result.[[Value]].
        if returned.is_object() {
            return settled;
        }
        // c. If result.[[Value]] is not undefined, throw a TypeError
        // exception.
        if !returned.is_undefined() {
            return Settled::NotAnObject;
        }
        // 12. Let thisBinding be ? constructorEnv.GetThisBinding().
        let Some(this_value) =
            environment.and_then(|environment| environment.bound_this_value(agent))
        else {
            return Settled::NoThisValue;
        };
        self.result = Some(this_value.unbind());
        settled
    }

    /// What [`Vm::leave_function`] left for the calling code, which is the
    /// code that runs again: the error of a derived class
    /// constructor that returned no object, or the rest of SuperCall.
    /// Returns what was thrown and no handler of the calling code took.
    #[inline(never)]
    fn finish_construction(
        &mut self,
        agent: &mut Agent,
        settled: Settled,
        gc: GcScope,
    ) -> Option<JsError<'static>> {
        let error = self
            .construction_finished(agent, settled, gc)
            .err()?
            .unbind();
        (!self.handle_error(agent, error)).then_some(error)
    }

    fn construction_finished<'gc>(
        &mut self,
        agent: &mut Agent,
        settled: Settled,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, ()> {
        match settled {
            Settled::Done => Ok(()),
            Settled::Super => {
                let Some(Ok(result)) = self.result.take().map(Object::try_from) else {
                    unreachable!("a constructor run in place is constructed to an object")
                };
                finish_super_call(agent, self, result, gc)
            }
            Settled::NotAnObject => {
                let value = self.result.take().unwrap_or(Value::Undefined);
                let message = format!(
                    "derived class constructor returned invalid value {}",
                    value
                        .string_repr(agent, gc.reborrow())
                        .to_string_lossy_(agent)
                );
                let message = String::from_string(agent, message, gc.nogc());
                Err(agent.throw_exception_with_message(
                    ExceptionType::TypeError,
                    message.unbind(),
                    gc.into_nogc(),
                ))
            }
            Settled::NoThisValue => {
                self.result = None;
                Err(agent.throw_exception_with_static_message(
                    ExceptionType::ReferenceError,
                    "Uninitialized this binding",
                    gc.into_nogc(),
                ))
            }
        }
    }

    /// The call that [`Plain::Accessor`] stands for, if this `Vm` runs the
    /// getter or setter itself: a call of no arguments, or of the value
    /// assigned, whose this value is the object, as that of a method call
    /// is the object of its Reference. What a getter returns is the result
    /// of the read; what a setter returns is dropped.
    ///
    /// Where the `Vm` does not run the function after all, everything is
    /// put back as the instruction found it, for the general way.
    // Not part of the loop of instructions: few instructions end here.
    #[inline(never)]
    fn accessor_called_in_place(
        &mut self,
        agent: &Agent,
        kind: Instruction,
    ) -> Option<(CalledInPlace, Instr)> {
        let setter = matches!(
            kind,
            Instruction::PutPropertyWithCache | Instruction::PutIndexPropertyWithCache
        );
        let call = Instr::new_with_arg(Instruction::EvaluateCall, u16::from(setter));
        if let Some(mut callee) = self.function_called_in_place(agent, call)
            && let Some(object) = self.result.take()
        {
            if setter {
                callee.completion = Completion::Discarded;
            }
            // Only the base of this Reference is asked for.
            self.reference = Some(Reference::new_property_reference(
                object,
                BUILTIN_STRING_MEMORY.length.to_property_key(),
                false,
            ));
            return Some((callee, call));
        }
        if setter {
            let value = self.stack.pop();
            self.stack.pop();
            // The object goes back on the stack where the instruction took
            // it from there, and nowhere where it is in a slot.
            let object = self.result.take();
            if kind == Instruction::PutPropertyWithCache {
                self.stack.extend(object);
            }
            self.result = value;
        } else {
            self.stack.pop();
        }
        None
    }

    /// Starts the call of `callee`, which the call instruction `instr`
    /// makes, in this `Vm`: what \[\[Call]] does up to the function's code.
    /// The calling code, which goes on at `ip`, is put aside with its
    /// stacks; the function's code finds them empty but for its parameters.
    /// Returns the function's code as the code to run.
    fn enter_function<'s>(
        &mut self,
        agent: &mut Agent,
        instr: Instr,
        callee: CalledInPlace,
        ip: usize,
        caller: Running,
        gc: NoGcScope<'_, 's>,
    ) -> Running<'s> {
        // The caller's rooted handle waits in the frame: the scoped value
        // it names is below everything the function scopes.
        let code = caller.code;
        let scoped = Scoped::from_root_repr(caller.executable.into_root_repr());
        if agent.vm_frames.in_use == agent.vm_frames.frames.len() {
            agent
                .vm_frames
                .frames
                .push(Frame::new(code, scoped.clone()));
        }
        let frame = &mut agent.vm_frames.frames[agent.vm_frames.in_use];
        // The function's value stack is the Vm's, and the caller's waits in
        // the frame. Exchanged before anything here writes to one of them:
        // a stack is exchanged several words at a time, and reading a word
        // that was written just before, together with its neighbour, waits
        // for the write to be done.
        core::mem::swap(&mut self.stack, &mut frame.stack);
        // EvaluateCall: the this value of the call, from its Reference; or
        // from under the function, where a method call keeps it and makes
        // no Reference. `cut` is where the caller's stack ends once the
        // arguments are taken off it with the function, and with that
        // this value. (Decided in one place, the plain call first: asked
        // again further down, a call that is no method call was twelve
        // instructions longer for it.)
        let first = frame.stack.len() - instr.get_first_index();
        let (reference, kept_this, cut) = if instr.kind == Instruction::EvaluateCall {
            (self.reference.take(), Value::Undefined, first - 1)
        } else if instr.kind == Instruction::PopReferenceAndEvaluateCall {
            self.reference = None;
            (self.reference_stack.pop(), Value::Undefined, first - 1)
        } else {
            self.reference = None;
            let this = frame.stack.get(first.wrapping_sub(2)).copied();
            (
                None,
                this.unwrap_or(Value::Undefined),
                first.saturating_sub(2),
            )
        };
        // The other stacks go on where the caller left them.
        frame.reference_base = self.reference_stack.len();
        frame.iterator_base = self.iterator_stack.len();
        frame.handler_base = self.handler_base;
        self.handler_base = self.exception_handler_stack.len();
        let arguments = &frame.stack[first..];
        let list = if let Some(parameters) = callee.parameters {
            // The arguments go where the function keeps its parameters:
            // undefined for those not given, and those beyond the
            // parameters are dropped.
            self.stack.extend(
                (0..parameters)
                    .map(|index| arguments.get(index).copied().unwrap_or(Value::Undefined)),
            );
            None
        } else if arguments.is_empty() {
            Some(Vec::new())
        } else {
            // The function binds its parameters itself, from the list of
            // its arguments: a vector kept from an earlier call.
            let mut list = agent.arguments_pool.pop().unwrap_or_default();
            list.extend_from_slice(arguments);
            Some(list)
        };
        frame.stack.truncate(cut);
        frame.ip = ip;
        frame.executable = code;
        frame.tables = Some(caller.tables);
        frame.completion = callee.completion;
        frame.stack_depth = caller.stack_depth;
        frame.collections_depth = caller.collections_depth;
        frame.scoped = scoped;
        agent.vm_frames.in_use += 1;
        self.depth += 1;
        self.ip = 0;
        let this_value = match reference {
            Some(reference) if is_property_reference(&reference) => {
                get_this_value(&reference).bind(gc)
            }
            Some(reference) => reference
                .base_env()
                .with_base_object(agent)
                .map_or(Value::Undefined, |object| object.into())
                .bind(gc),
            None => kept_this.bind(gc),
        };
        // PrepareForOrdinaryCall and OrdinaryCallBindThis.
        if callee.environment == CallEnvironment::Function {
            // A derived class constructor is run with its new target and
            // has no this value until a `super(…)` in it binds one.
            let derived = matches!(
                callee.completion,
                Completion::ConstructedDerived | Completion::SuperConstructedDerived
            );
            let new_target = if derived {
                Self::derived_new_target(agent, callee, gc)
            } else {
                None
            };
            let context = prepare_for_ordinary_call(agent, callee.function, new_target, gc);
            let Some(Environment::Function(environment)) = context
                .ecmascript_code
                .as_ref()
                .map(|code| code.lexical_environment)
            else {
                unreachable!("PrepareForOrdinaryCall made no Function Environment Record")
            };
            if derived {
                agent.vm_frames.frames[agent.vm_frames.in_use - 1].derived =
                    Some(environment.unbind());
            } else {
                ordinary_call_bind_this(agent, callee.function, environment, this_value, gc);
            }
        } else {
            prepare_for_call_without_function_environment(
                agent,
                callee.function,
                this_value,
                callee.environment == CallEnvironment::Declarative,
                gc,
            );
        }
        if let Some(list) = list {
            // The list is rooted until the function is left, under what
            // the function's own instructions scope: they find it on the
            // iterator stack, as the code of a `Vm` of its own does.
            let list = scope_arguments_of_call(agent, list);
            self.iterator_stack
                .push(VmIteratorRecord::SliceIterator(list));
        }
        // The function's instructions and tables, held while it runs.
        let tables = callee.executable.get_tables(agent);
        let executable = Scoped::new_exclusive(agent, callee.executable, gc);
        Running {
            executable,
            code: callee.executable,
            tables,
            stack_depth: agent.stack_refs.get_mut().len(),
            collections_depth: agent.stack_ref_collections.get_mut().len(),
        }
    }

    /// Ends the function this `Vm` runs in place of a call, which returned
    /// or threw: its execution context is removed, and its stacks, emptied,
    /// stay in the frame for the next function, while those of the calling
    /// code are the `Vm`'s again. Returns the calling code as the code to
    /// run; `callee` is the function's code.
    ///
    /// The same ends a generator that was resumed in place of the step of
    /// an iteration ([`Vm::enter_generator`]) and returned or threw: it is
    /// completed, and where it returned (`returned`; an error is the
    /// caller's to handle) the iteration is over.
    // Part of the loop of instructions where a function returns, which is
    // the end of every call: as a function of its own it saved and
    // restored the registers it works in and handed the calling code back
    // through memory, a twentieth of a plain call. What only some returns
    // do is in functions of its own, so that what every return does is
    // short; a function that throws is left by
    // [`Vm::leave_function_thrown`].
    #[inline(always)]
    fn leave_function<'s>(
        &mut self,
        agent: &mut Agent,
        callee: Running,
        returned: bool,
    ) -> (Running<'s>, Settled) {
        agent.remove_execution_context();
        self.depth -= 1;
        agent.vm_frames.in_use -= 1;
        let frame = &mut agent.vm_frames.frames[agent.vm_frames.in_use];
        // The function's instructions and tables are let go of.
        drop(callee);
        // The caller's value stack is the Vm's again, and the function's is
        // emptied where it waits for the next one: after the exchange, as
        // in [`Vm::enter_function`]. What the function left on the other
        // stacks (it returned out of a loop, or threw) is cut off.
        core::mem::swap(&mut self.stack, &mut frame.stack);
        frame.stack.clear();
        // Most functions leave nothing on these two: cutting a vector of
        // what has a destructor calls for the dropping of the nothing that
        // is cut off, which was a twentieth of a plain call's time with the
        // release below.
        if self.reference_stack.len() > frame.reference_base
            || self.iterator_stack.len() > frame.iterator_base
        {
            self.cut_stacks(frame.reference_base, frame.iterator_base);
        }
        self.exception_handler_stack.truncate(self.handler_base);
        self.handler_base = frame.handler_base;
        let mut left = Left {
            stepped: None,
            settled: Settled::Done,
            derived: None,
        };
        match frame.completion {
            Completion::Value => {}
            Completion::Discarded => self.result = None,
            _ => self.leave_special(frame, &mut left),
        }
        if frame.stack.capacity() > VM_STACK_POOL_MAX_CAPACITY {
            // A stack that grew large is not kept for the next function.
            Self::shed_stack(frame);
        }
        self.ip = frame.ip;
        self.reference = None;
        let tables = frame
            .tables
            .take()
            .expect("the frame of the calling code is in use");
        let (stack_depth, collections_depth) = (frame.stack_depth, frame.collections_depth);
        // The caller's executable, which the collector kept current in the
        // frame, and its rooted handle: the scoped value that names was
        // below what the function scoped, and is there still.
        let code = frame.executable;
        let executable = frame.scoped.clone();
        if agent.vm_frames.in_use == 0 && agent.vm_frames.frames.len() > SPARE_FRAMES {
            // No frame is in use: the Agent keeps a few of them, and none
            // of a deep recursion.
            Self::drop_spare_frames(agent);
        }
        agent.stack_refs.get_mut().truncate(stack_depth);
        debug_assert_eq!(executable.get(agent), code);
        if agent.stack_ref_collections.get_mut().len() > collections_depth {
            release_scoped_collections(agent, collections_depth);
        }
        let Left {
            stepped,
            mut settled,
            derived,
        } = left;
        if let Some((generator, done, reference)) = stepped {
            self.end_resumed_generator(agent, generator, done, reference, returned);
        }
        if returned && let Some((environment, for_super)) = derived {
            settled = self.settle_derived(agent, environment, for_super);
        }
        (
            Running {
                executable,
                code,
                tables,
                stack_depth,
                collections_depth,
            },
            settled,
        )
    }

    /// [`Vm::leave_function`] for a function that threw, or that a
    /// termination ends: the calling code, which has the error next.
    // Not part of the loop of instructions: few functions end this way.
    #[inline(never)]
    fn leave_function_thrown<'s>(&mut self, agent: &mut Agent, callee: Running) -> Running<'s> {
        self.leave_function(agent, callee, false).0
    }

    /// What a function left on the Reference stack and the iterator stack
    /// (it returned out of a loop, or threw) is cut off.
    #[inline(never)]
    fn cut_stacks(&mut self, reference_base: usize, iterator_base: usize) {
        self.reference_stack.truncate(reference_base);
        self.iterator_stack.truncate(iterator_base);
    }

    /// The value stack of a frame that grew large is let go of.
    #[inline(never)]
    fn shed_stack(frame: &mut Frame) {
        frame.stack = Vec::new();
    }

    /// The frames of a deep recursion are let go of, all but a few.
    #[inline(never)]
    fn drop_spare_frames(agent: &mut Agent) {
        agent.vm_frames.frames.truncate(SPARE_FRAMES);
    }

    /// What is done where the frame that is left is not that of a plain
    /// call: `new` takes the object the function was constructed to, a
    /// derived constructor's result waits for [`Vm::settle_derived`], and
    /// the generator of a step is taken out of the frame.
    #[inline(never)]
    fn leave_special(&mut self, frame: &mut Frame, left: &mut Left) {
        match frame.completion {
            Completion::Value | Completion::Discarded => {}
            Completion::Constructed | Completion::SuperConstructed => {
                // 10. If result is a return completion, then
                //   a. If result.[[Value]] is an Object, return it.
                //   b. If kind is base, return thisArgument.
                let object = self.stack.pop();
                if !self.result.is_some_and(|value| value.is_object()) {
                    self.result = object;
                }
                if frame.completion == Completion::SuperConstructed {
                    left.settled = Settled::Super;
                }
            }
            Completion::ConstructedDerived | Completion::SuperConstructedDerived => {
                left.derived = Some((
                    frame.derived.take(),
                    frame.completion == Completion::SuperConstructedDerived,
                ));
            }
            Completion::Stepped => {
                // The generator's value stack stays in the frame for the
                // next function, as a function's does; the record of its
                // suspended state is let go of, and the frame's spare
                // stack with it.
                drop(frame.kept.take());
                let generator = frame
                    .resumed
                    .take()
                    .expect("the frame of a resumed generator has it");
                let reference = if core::mem::take(&mut frame.reference_kept) {
                    self.reference_stack.pop()
                } else {
                    None
                };
                left.stepped = Some((generator, frame.done, reference));
            }
        }
    }

    /// What is left to do where a generator that was resumed in place of
    /// the step of an iteration returned or threw, and its frame is left.
    #[inline(never)]
    fn end_resumed_generator(
        &mut self,
        agent: &mut Agent,
        generator: Generator<'static>,
        done: usize,
        reference: Option<Reference<'static>>,
        returned: bool,
    ) {
        // GeneratorStart 4.g. Set acGenerator.[[GeneratorState]] to
        // completed: for a return and for a throw completion.
        generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
        // The step leaves the caller's Reference register as it was.
        self.reference = reference;
        if !returned {
            // The step threw: its iterator is not to be closed.
            self.exhaust_active_iterator();
        }
        if returned {
            // Iterator finished: the step has no value, and the calling
            // code goes on where its loop ends.
            self.result = None;
            self.ip = done;
        }
    }

    /// IteratorStepValue from the iterator result object that a generator
    /// resumed in place passed on from `yield*`, for the `Vm` that waits on
    /// top of the Agent's while this reads the object: where the reading
    /// throws, the step has thrown, and that `Vm`'s iterator is left
    /// exhausted ([`Vm::exhaust_active_iterator`]).
    // A function of its own, so that the loop of instructions has one call
    // here as it had: with the iterator seen to in the loop itself, an
    // empty loop was 7% slower for where the loop's code then lay.
    #[inline(never)]
    fn step_value_of_yielded<'gc>(
        agent: &mut Agent,
        result: Value,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Option<Value<'gc>>> {
        let step = step_value_of_result(agent, result, gc);
        if step.is_err()
            && let Some(vm) = agent.vm_stack.last_mut()
        {
            vm.exhaust_active_iterator();
        }
        step
    }

    /// Leaves the iterator that a step was taken of exhausted: the step
    /// threw, and an iterator whose step throws is done and is not closed.
    /// The handler around a `for`-`of` loop closes the iterator for
    /// whatever is thrown in the loop, the step included; of an exhausted
    /// one there is nothing to close.
    pub(super) fn exhaust_active_iterator(&mut self) {
        if let Some(iterator) = self.iterator_stack.last_mut() {
            *iterator = VmIteratorRecord::EmptySliceIterator;
        }
    }

    /// Whether the code that runs is a generator's, resumed in place of the
    /// step of an iteration.
    fn runs_resumed_generator(&self, agent: &Agent) -> bool {
        self.depth != 0
            && agent
                .vm_frames
                .in_use
                .checked_sub(1)
                .and_then(|top| agent.vm_frames.frames.get(top))
                .is_some_and(|frame| frame.completion == Completion::Stepped)
    }

    /// The generator that the step of an iteration is about to resume, if
    /// this `Vm` runs it itself: the iterator of the step is a generator
    /// that is suspended, with the next method IteratorStepValue would call
    /// being the original one of the running realm (which resumes the
    /// generator and does nothing else that can be seen: see
    /// [`ActiveIterator::step_value`]), where one more call is not one too
    /// many. Nothing has changed where this returns None.
    ///
    /// [`ActiveIterator::step_value`]: super::iterator::ActiveIterator
    #[inline(never)]
    fn generator_resumed_in_place(
        &self,
        agent: &Agent,
        gc: NoGcScope,
    ) -> Option<Generator<'static>> {
        let Some(VmIteratorRecord::GenericIterator(IteratorRecord {
            iterator: Object::Generator(generator),
            next_method,
        })) = self.iterator_stack.last()
        else {
            return None;
        };
        if !GeneratorPrototype::is_next(agent, *next_method, gc) {
            return None;
        }
        // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
        // One that is running throws, and one that is completed is done:
        // both the general way.
        if !matches!(
            generator.get(agent).generator_state,
            Some(GeneratorState::SuspendedStart(_) | GeneratorState::SuspendedYield(_))
        ) {
            return None;
        }
        if agent.calls_in_place_exceeded() {
            return None;
        }
        Some(generator.unbind())
    }

    /// Resumes `generator` in this `Vm`, in place of the step `instr` of an
    /// iteration over it: GeneratorResume with undefined, up to the
    /// generator's code. The calling code, which goes on at `ip`, is put
    /// aside as for a call ([`Vm::enter_function`]). The generator's own
    /// value stack is the `Vm`'s while it runs, and what it kept on its
    /// other three stacks is on the `Vm`'s above what the caller has there.
    /// Returns the generator's code as the code to run, which goes on
    /// where it yielded, or starts.
    fn enter_generator<'s>(
        &mut self,
        agent: &mut Agent,
        instr: Instr,
        generator: Generator<'static>,
        ip: usize,
        caller: Running,
        gc: NoGcScope<'_, 's>,
    ) -> Running<'s> {
        // 7. Set generator.[[GeneratorState]] to executing.
        let Some(
            GeneratorState::SuspendedStart(mut state) | GeneratorState::SuspendedYield(mut state),
        ) = generator
            .get_mut(agent)
            .generator_state
            .replace(GeneratorState::Executing)
        else {
            unreachable!("the generator was seen to be suspended")
        };
        let code = caller.code;
        let scoped = Scoped::from_root_repr(caller.executable.into_root_repr());
        if agent.vm_frames.in_use == agent.vm_frames.frames.len() {
            agent
                .vm_frames
                .frames
                .push(Frame::new(code, scoped.clone()));
        }
        let frame = &mut agent.vm_frames.frames[agent.vm_frames.in_use];
        // The caller's value stack waits in the frame, the generator's own
        // is the Vm's, and the frame's spare one waits in the record of the
        // suspended state for the generator to yield.
        core::mem::swap(&mut self.stack, &mut frame.stack);
        core::mem::swap(&mut self.stack, &mut state.vm.stack);
        // The step leaves the caller's Reference register as it was: it
        // waits under what the generator has on the Reference stack.
        frame.reference_kept = match self.reference.take() {
            Some(reference) => {
                self.reference_stack.push(reference);
                true
            }
            None => false,
        };
        frame.reference_base = self.reference_stack.len();
        frame.iterator_base = self.iterator_stack.len();
        frame.handler_base = self.handler_base;
        self.handler_base = self.exception_handler_stack.len();
        // What the generator kept on its other stacks goes above what the
        // caller has on the Vm's. Most keep no Reference; one that yields
        // in a loop keeps the one handler that every loop has, and in a
        // loop over an iterator that iterator: one entry is moved as the
        // entry it is, where the vector appended whole was a copy of its
        // bytes by a call, at every step.
        if !state.vm.reference_stack.is_empty() {
            self.reference_stack.append(&mut state.vm.reference_stack);
        }
        match state.vm.iterator_stack.len() {
            0 => {}
            1 => self.iterator_stack.extend(state.vm.iterator_stack.pop()),
            _ => self.iterator_stack.append(&mut state.vm.iterator_stack),
        }
        match state.vm.exception_jump_target_stack.len() {
            0 => {}
            1 => self
                .exception_handler_stack
                .extend(state.vm.exception_jump_target_stack.pop()),
            _ => self
                .exception_handler_stack
                .append(&mut state.vm.exception_jump_target_stack),
        }
        let execution_context = state
            .execution_context
            .take()
            .expect("a suspended generator has its execution context");
        let executable = state.executable;
        let resumed_at = state.vm.ip;
        frame.resumed = Some(generator);
        frame.kept = Some(state);
        frame.done = instr.get_jump_slot();
        frame.ip = ip;
        frame.executable = code;
        frame.tables = Some(caller.tables);
        frame.completion = Completion::Stepped;
        frame.stack_depth = caller.stack_depth;
        frame.collections_depth = caller.collections_depth;
        frame.scoped = scoped;
        agent.vm_frames.in_use += 1;
        self.depth += 1;
        self.ip = resumed_at;
        // 9. Resume the suspended evaluation of genContext using
        // NormalCompletion(value) as the result of the operation that
        // suspended it: the step resumes with undefined.
        self.result = Some(Value::Undefined);
        // 4. Let genContext be generator.[[GeneratorContext]].
        // 8. Push genContext onto the execution context stack; genContext
        // is now the running execution context.
        agent.push_execution_context(execution_context);
        // The generator's instructions and tables, held while it runs.
        let executable = executable.bind(gc);
        let tables = executable.get_tables(agent);
        let rooted = executable.scope(agent, gc);
        Running {
            executable: rooted,
            code: executable.unbind(),
            tables,
            stack_depth: agent.stack_refs.get_mut().len(),
            collections_depth: agent.stack_ref_collections.get_mut().len(),
        }
    }

    /// GeneratorYield of the generator that this `Vm` resumed in place of
    /// the step of an iteration, which goes on at `ip` when it is resumed
    /// again: its stacks are its own again, as a suspended `Vm` of its own
    /// would leave them, its execution context is taken off, and the
    /// stacks of the calling code are the `Vm`'s. Returns the calling code
    /// as the code to run, and where it goes when the iteration is over;
    /// `callee` is the generator's code.
    fn suspend_generator<'s>(
        &mut self,
        agent: &mut Agent,
        ip: usize,
        callee: Running,
    ) -> (Running<'s>, usize) {
        // GeneratorYield 6. Remove genContext from the execution context
        // stack and restore the execution context that is at the top of the
        // execution context stack as the running execution context.
        let execution_context = agent.pop_execution_context();
        assert!(
            execution_context.is_some(),
            "a generator that runs has its execution context"
        );
        self.depth -= 1;
        agent.vm_frames.in_use -= 1;
        // The generator's code as the last collection left it.
        let generator_code = callee.executable.get(agent);
        let frame = &mut agent.vm_frames.frames[agent.vm_frames.in_use];
        // The generator's instructions and tables are let go of.
        drop(callee);
        let generator = frame
            .resumed
            .take()
            .expect("the frame of a resumed generator has it");
        let mut state = frame
            .kept
            .take()
            .expect("the frame of a resumed generator has the record of its state");
        let done = frame.done;
        // The generator's value stack goes back into the record, the
        // caller's is the Vm's again, and the frame has its spare one for
        // the next function.
        core::mem::swap(&mut self.stack, &mut state.vm.stack);
        core::mem::swap(&mut self.stack, &mut frame.stack);
        refit_suspended_stack(&mut state.vm.stack);
        // What the generator has on the other stacks, above what the
        // caller has, is the generator's to keep: most have nothing.
        if self.reference_stack.len() > frame.reference_base {
            state
                .vm
                .reference_stack
                .extend(self.reference_stack.drain(frame.reference_base..));
            refit_suspended_stack(&mut state.vm.reference_stack);
        }
        // One entry goes back as the entry it is, into the room its vector
        // kept from the last step: see [`Vm::enter_generator`].
        if self.iterator_stack.len() == frame.iterator_base + 1
            && state.vm.iterator_stack.capacity() != 0
        {
            state.vm.iterator_stack.extend(self.iterator_stack.pop());
        } else if self.iterator_stack.len() > frame.iterator_base {
            state
                .vm
                .iterator_stack
                .extend(self.iterator_stack.drain(frame.iterator_base..));
            refit_suspended_stack(&mut state.vm.iterator_stack);
        }
        if self.exception_handler_stack.len() == self.handler_base + 1
            && state.vm.exception_jump_target_stack.capacity() != 0
        {
            state
                .vm
                .exception_jump_target_stack
                .extend(self.exception_handler_stack.pop());
        } else if self.exception_handler_stack.len() > self.handler_base {
            state
                .vm
                .exception_jump_target_stack
                .extend(self.exception_handler_stack.drain(self.handler_base..));
            refit_suspended_stack(&mut state.vm.exception_jump_target_stack);
        }
        self.handler_base = frame.handler_base;
        // The step leaves the caller's Reference register as it was.
        self.reference = if core::mem::take(&mut frame.reference_kept) {
            self.reference_stack.pop()
        } else {
            None
        };
        self.ip = frame.ip;
        frame.completion = Completion::Value;
        let tables = frame
            .tables
            .take()
            .expect("the frame of the calling code is in use");
        let (stack_depth, collections_depth) = (frame.stack_depth, frame.collections_depth);
        let code = frame.executable;
        let executable = frame.scoped.clone();
        // As where a function is left ([`Vm::leave_function`]): the frames
        // of a deep recursion and the collections the generator scoped are
        // let go of where there are any, by calls that few steps make.
        if agent.vm_frames.in_use == 0 && agent.vm_frames.frames.len() > SPARE_FRAMES {
            Self::drop_spare_frames(agent);
        }
        agent.stack_refs.get_mut().truncate(stack_depth);
        debug_assert_eq!(executable.get(agent), code);
        if agent.stack_ref_collections.get_mut().len() > collections_depth {
            release_scoped_collections(agent, collections_depth);
        }
        // GeneratorYield 5. Set generator.[[GeneratorState]] to
        // suspended-yield.
        state.vm.ip = ip;
        state.executable = generator_code;
        state.execution_context = execution_context;
        generator.get_mut(agent).generator_state = Some(GeneratorState::SuspendedYield(state));
        (
            Running {
                executable,
                code,
                tables,
                stack_depth,
                collections_depth,
            },
            done,
        )
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
            Ok(kind @ (ContinuationKind::Yield | ContinuationKind::YieldValue)) => {
                let yielded_value = self.result.take().unwrap();
                if agent.options.print_internals {
                    Self::print_yielding(yielded_value);
                }
                Some(ExecutionResult::Yield {
                    vm: core::mem::take(self).suspend(),
                    yielded_value,
                    plain: matches!(kind, ContinuationKind::YieldValue),
                })
            }
            Ok(ContinuationKind::Super) => {
                unreachable!("`super(…)` is executed where it is met")
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
        // A handler of the code that is running: those under
        // `handler_base` are its callers'.
        if self.exception_handler_stack.len() > self.handler_base
            && let Some(handler) = self.exception_handler_stack.pop()
        {
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
        window: &[u8; Instr::WINDOW],
        executable: &Scoped<Executable>,
        code: Executable,
        tables: TablesView,
        instr: Instr,
        gc: NoGcScope,
    ) -> Plain {
        // `ip` is at the instruction, and every arm moves it past the
        // instruction first, whatever it then makes of it: by a length that
        // is a constant of the arm. Found from a table by the byte that was
        // read, the place of the next instruction waited for that byte and
        // for the table. The instructions of one arm are of one length.
        macro_rules! step {
            ($kind:ident $(, $same:ident)*) => {{
                const LENGTH: usize = Instruction::$kind.length();
                $(const _: () = assert!(Instruction::$same.length() == LENGTH);)*
                *ip += LENGTH;
            }};
        }
        // The instructions of one family (the comparisons that jump, the
        // operators) have an arm each, in which the kind is a constant: one
        // arm for all of them found out by the kind again what the
        // instruction was.
        macro_rules! compare_and_jump {
            ($kind:ident) => {{
                step!($kind);
                return Self::compare_and_jump(agent, vm, ip, instr, Instruction::$kind);
            }};
        }
        macro_rules! compare_local_and_jump {
            ($kind:ident) => {{
                step!($kind);
                return Self::compare_local_and_jump(
                    agent,
                    vm,
                    ip,
                    window,
                    tables,
                    instr,
                    Instruction::$kind,
                    gc,
                );
            }};
        }
        macro_rules! step_local_compare_and_jump {
            ($kind:ident) => {{
                step!($kind);
                return Self::step_local_compare_and_jump(
                    agent,
                    vm,
                    ip,
                    window,
                    tables,
                    instr,
                    Instruction::$kind,
                    gc,
                );
            }};
        }
        macro_rules! operate_on_integers {
            ($kind:ident) => {{
                step!($kind);
                return vm.operate_on_integers(agent, Instruction::$kind, gc);
            }};
        }
        match instr.kind {
            Instruction::Load => {
                step!(Load);
                vm.execute_load()
            }
            Instruction::Return => {
                step!(Return);
                return Plain::Return;
            }
            Instruction::Yield | Instruction::YieldValue => {
                step!(Yield, YieldValue);
                return Plain::Yield;
            }
            Instruction::LoadCopy => {
                step!(LoadCopy);
                vm.execute_load_copy()
            }
            Instruction::PutValueToIndex => {
                step!(PutValueToIndex);
                vm.execute_load_to_index(instr.get_first_index())
            }
            Instruction::Store => {
                step!(Store);
                vm.execute_store()
            }
            Instruction::GetValueFromIndex => {
                step!(GetValueFromIndex);
                vm.execute_store_from_index(instr.get_first_index())
            }
            Instruction::LoadFromIndex => {
                step!(LoadFromIndex);
                vm.execute_store_from_index(instr.get_first_index());
                vm.execute_load();
            }
            // A local and a constant, two locals: the operands of most
            // binary expressions, and the arguments of many calls.
            Instruction::LoadFromIndexStoreConstant => {
                step!(LoadFromIndexStoreConstant);
                let value = vm.stack[instr.get_first_index()];
                vm.stack.push(value);
                let constant = tables.constant(instr.get_second_index(), gc);
                vm.result = Some(constant.unbind());
            }
            Instruction::LoadFromIndexGetFromIndex => {
                step!(LoadFromIndexGetFromIndex);
                let value = vm.stack[instr.get_first_index()];
                vm.stack.push(value);
                vm.result = Some(vm.stack[instr.get_second_index()]);
            }
            Instruction::LoadFromIndexLoadFromIndex => {
                step!(LoadFromIndexLoadFromIndex);
                let value = vm.stack[instr.get_first_index()];
                vm.stack.push(value);
                let value = vm.stack[instr.get_second_index()];
                vm.stack.push(value);
            }
            // An operator with a constant as its right operand: neither
            // operand goes by the stack, and the result goes where the
            // instruction says.
            Instruction::ApplyBinaryOperatorWithConstant => {
                step!(ApplyBinaryOperatorWithConstant);
                let Some(left) = vm.result else {
                    return Plain::No;
                };
                let right = tables.constant(instr.get_first_index(), gc);
                let operation = instr.get_second_index();
                let (_, operator) = Instruction::fused_slot_and_operator(operation);
                let Some(value) = Instruction::fused_operator(operator)
                    .and_then(|kind| Self::number_operation(agent, kind, left, right, gc))
                else {
                    return Plain::No;
                };
                if Instruction::fused_pushes(operation) {
                    vm.result = None;
                    vm.stack.push(value);
                } else {
                    vm.result = Some(value);
                }
            }
            Instruction::ApplyBinaryOperatorToIndexWithConstant => {
                step!(ApplyBinaryOperatorToIndexWithConstant);
                let operation = instr.get_first_index();
                let (slot, operator) = Instruction::fused_slot_and_operator(operation);
                let left = vm.stack[slot];
                let right = tables.constant(instr.get_second_index(), gc);
                let Some(value) = Instruction::fused_operator(operator)
                    .and_then(|kind| Self::number_operation(agent, kind, left, right, gc))
                else {
                    return Plain::No;
                };
                if Instruction::fused_pushes(operation) {
                    vm.result = None;
                    vm.stack.push(value);
                } else {
                    vm.result = Some(value);
                }
            }
            // An operator with a local as its left operand and the result
            // as its right one.
            Instruction::ApplyBinaryOperatorToIndexWithResult => {
                step!(ApplyBinaryOperatorToIndexWithResult);
                let operation = instr.get_first_index();
                let (slot, operator) = Instruction::fused_slot_and_operator(operation);
                let left = vm.stack[slot];
                let Some(right) = vm.result else {
                    return Plain::No;
                };
                let Some(value) = Instruction::fused_operator(operator)
                    .and_then(|kind| Self::number_operation(agent, kind, left, right, gc))
                else {
                    return Plain::No;
                };
                if Instruction::fused_pushes(operation) {
                    vm.result = None;
                    vm.stack.push(value);
                } else {
                    vm.result = Some(value);
                }
            }
            // An operator on the top of the stack and the result, whose
            // value replaces the top of the stack or goes to a local.
            Instruction::ApplyBinaryOperatorAndMove => {
                step!(ApplyBinaryOperatorAndMove);
                let operation = instr.get_first_index();
                let (slot, operator) = Instruction::fused_slot_and_operator(operation);
                let (Some(&left), Some(right)) = (vm.stack.last(), vm.result) else {
                    return Plain::No;
                };
                let Some(value) = Instruction::fused_operator(operator)
                    .and_then(|kind| Self::number_operation(agent, kind, left, right, gc))
                else {
                    return Plain::No;
                };
                vm.result = None;
                if Instruction::fused_pushes(operation) {
                    if let Some(top) = vm.stack.last_mut() {
                        *top = value;
                    }
                } else {
                    vm.stack.pop();
                    vm.stack[slot] = value;
                }
            }
            Instruction::StoreConstant => {
                step!(StoreConstant);
                let constant = tables.constant(instr.get_first_index(), gc);
                vm.result = Some(constant.unbind());
            }
            Instruction::LoadConstant => {
                step!(LoadConstant);
                let constant = tables.constant(instr.get_first_index(), gc);
                vm.stack.push(constant.unbind());
            }
            Instruction::PopStack => {
                step!(PopStack);
                vm.execute_pop_stack()
            }
            // The step of `for`-`of` over an Array that is walked element
            // by element, where the element is a value or the walk is over;
            // and the step over a generator, which the Vm may resume itself.
            Instruction::IteratorStepValue => {
                step!(IteratorStepValue);
                let walk = match vm.iterator_stack.last_mut() {
                    Some(VmIteratorRecord::ArrayValues(walk)) => walk,
                    Some(VmIteratorRecord::GenericIterator(IteratorRecord {
                        iterator: Object::Generator(_),
                        ..
                    })) => return Plain::Resume,
                    _ => return Plain::No,
                };
                let Some(step) = walk.plain_next(agent, gc) else {
                    return Plain::No;
                };
                vm.result = step.unbind();
                if step.is_none() {
                    // Iterator finished: jump to escape the iterator loop.
                    *ip = instr.get_jump_slot();
                }
            }
            Instruction::PushReference => {
                step!(PushReference);
                vm.execute_push_reference(agent)
            }
            Instruction::PopReference => {
                step!(PopReference);
                vm.execute_pop_reference()
            }
            Instruction::PushReferenceAndLoad => {
                step!(PushReferenceAndLoad);
                vm.execute_push_reference(agent);
                vm.execute_load();
            }
            Instruction::Jump => {
                step!(Jump);
                let from = *ip;
                execute_jump(agent, ip, instr);
                if *ip < from {
                    return Plain::Looped;
                }
            }
            Instruction::JumpIfNot => {
                step!(JumpIfNot);
                let from = *ip;
                execute_jump_if_not(agent, vm, ip, instr);
                if *ip < from {
                    return Plain::Looped;
                }
            }
            Instruction::JumpIfTrue => {
                step!(JumpIfTrue);
                let from = *ip;
                execute_jump_if_true(agent, vm, ip, instr);
                if *ip < from {
                    return Plain::Looped;
                }
            }
            // The test of most loops, on two Numbers: before the body with
            // the jump out where it fails, or after it with the jump back
            // where it holds.
            Instruction::JumpIfNotLessThan => compare_and_jump!(JumpIfNotLessThan),
            Instruction::JumpIfNotLessThanEquals => compare_and_jump!(JumpIfNotLessThanEquals),
            Instruction::JumpIfNotGreaterThan => compare_and_jump!(JumpIfNotGreaterThan),
            Instruction::JumpIfNotGreaterThanEquals => {
                compare_and_jump!(JumpIfNotGreaterThanEquals)
            }
            Instruction::JumpIfLessThan => compare_and_jump!(JumpIfLessThan),
            Instruction::JumpIfLessThanEquals => compare_and_jump!(JumpIfLessThanEquals),
            Instruction::JumpIfGreaterThan => compare_and_jump!(JumpIfGreaterThan),
            Instruction::JumpIfGreaterThanEquals => compare_and_jump!(JumpIfGreaterThanEquals),
            // The same tests on a local and a constant, or on two locals,
            // which are read where they are.
            Instruction::JumpIfIndexNotLessThanConstant => {
                compare_local_and_jump!(JumpIfIndexNotLessThanConstant)
            }
            Instruction::JumpIfIndexNotLessThanIndex => {
                compare_local_and_jump!(JumpIfIndexNotLessThanIndex)
            }
            Instruction::JumpIfIndexNotLessThanEqualsConstant => {
                compare_local_and_jump!(JumpIfIndexNotLessThanEqualsConstant)
            }
            Instruction::JumpIfIndexNotLessThanEqualsIndex => {
                compare_local_and_jump!(JumpIfIndexNotLessThanEqualsIndex)
            }
            Instruction::JumpIfIndexNotGreaterThanConstant => {
                compare_local_and_jump!(JumpIfIndexNotGreaterThanConstant)
            }
            Instruction::JumpIfIndexNotGreaterThanIndex => {
                compare_local_and_jump!(JumpIfIndexNotGreaterThanIndex)
            }
            Instruction::JumpIfIndexNotGreaterThanEqualsConstant => {
                compare_local_and_jump!(JumpIfIndexNotGreaterThanEqualsConstant)
            }
            Instruction::JumpIfIndexNotGreaterThanEqualsIndex => {
                compare_local_and_jump!(JumpIfIndexNotGreaterThanEqualsIndex)
            }
            Instruction::JumpIfIndexNotStrictlyEqualConstant => {
                compare_local_and_jump!(JumpIfIndexNotStrictlyEqualConstant)
            }
            Instruction::JumpIfIndexNotStrictlyEqualIndex => {
                compare_local_and_jump!(JumpIfIndexNotStrictlyEqualIndex)
            }
            Instruction::JumpIfIndexStrictlyEqualConstant => {
                compare_local_and_jump!(JumpIfIndexStrictlyEqualConstant)
            }
            Instruction::JumpIfIndexStrictlyEqualIndex => {
                compare_local_and_jump!(JumpIfIndexStrictlyEqualIndex)
            }
            // What was just computed compared with a constant, and the
            // jump on it: one arm for the ten comparisons, which the
            // instruction has as an argument.
            Instruction::JumpIfResultComparesWithConstant => {
                step!(JumpIfResultComparesWithConstant);
                return Self::compare_result_with_constant_and_jump(
                    agent, vm, ip, window, tables, instr, gc,
                );
            }
            // The test at the end of a loop, after `i++` or `i--` where the
            // instruction makes that too: all of it here, or none. The step
            // is made of an integer only; the test takes any two Numbers.
            Instruction::JumpIfIndexLessThanConstant => {
                step_local_compare_and_jump!(JumpIfIndexLessThanConstant)
            }
            Instruction::JumpIfIndexLessThanIndex => {
                step_local_compare_and_jump!(JumpIfIndexLessThanIndex)
            }
            Instruction::JumpIfIndexLessThanEqualsConstant => {
                step_local_compare_and_jump!(JumpIfIndexLessThanEqualsConstant)
            }
            Instruction::JumpIfIndexLessThanEqualsIndex => {
                step_local_compare_and_jump!(JumpIfIndexLessThanEqualsIndex)
            }
            Instruction::JumpIfIndexGreaterThanConstant => {
                step_local_compare_and_jump!(JumpIfIndexGreaterThanConstant)
            }
            Instruction::JumpIfIndexGreaterThanIndex => {
                step_local_compare_and_jump!(JumpIfIndexGreaterThanIndex)
            }
            Instruction::JumpIfIndexGreaterThanEqualsConstant => {
                step_local_compare_and_jump!(JumpIfIndexGreaterThanEqualsConstant)
            }
            Instruction::JumpIfIndexGreaterThanEqualsIndex => {
                step_local_compare_and_jump!(JumpIfIndexGreaterThanEqualsIndex)
            }
            Instruction::IncrementIndexJumpIfLessThanConstant => {
                step_local_compare_and_jump!(IncrementIndexJumpIfLessThanConstant)
            }
            Instruction::IncrementIndexJumpIfLessThanIndex => {
                step_local_compare_and_jump!(IncrementIndexJumpIfLessThanIndex)
            }
            Instruction::IncrementIndexJumpIfLessThanEqualsConstant => {
                step_local_compare_and_jump!(IncrementIndexJumpIfLessThanEqualsConstant)
            }
            Instruction::IncrementIndexJumpIfLessThanEqualsIndex => {
                step_local_compare_and_jump!(IncrementIndexJumpIfLessThanEqualsIndex)
            }
            Instruction::DecrementIndexJumpIfGreaterThanConstant => {
                step_local_compare_and_jump!(DecrementIndexJumpIfGreaterThanConstant)
            }
            Instruction::DecrementIndexJumpIfGreaterThanIndex => {
                step_local_compare_and_jump!(DecrementIndexJumpIfGreaterThanIndex)
            }
            Instruction::DecrementIndexJumpIfGreaterThanEqualsConstant => {
                step_local_compare_and_jump!(DecrementIndexJumpIfGreaterThanEqualsConstant)
            }
            Instruction::DecrementIndexJumpIfGreaterThanEqualsIndex => {
                step_local_compare_and_jump!(DecrementIndexJumpIfGreaterThanEqualsIndex)
            }
            // `i++` and `i--` of an integer in a stack slot.
            Instruction::IncrementIndex | Instruction::DecrementIndex => {
                step!(IncrementIndex, DecrementIndex);
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
                step!(ResolveThisBinding);
                let Some(this) = agent.running_call_this_value() else {
                    return Plain::No;
                };
                vm.result = Some(this);
            }
            Instruction::IsStrictlyEqual => {
                step!(IsStrictlyEqual);
                let left = vm.stack.pop().unwrap();
                let right = vm.result.unwrap();
                vm.result = Some(Self::strictly_equal(agent, left, right).into());
            }
            // `x === 3`: the constant is compared where it is.
            Instruction::IsStrictlyEqualConstant => {
                step!(IsStrictlyEqualConstant);
                let left = vm.result.unwrap();
                let right = tables.constant(instr.get_first_index(), gc);
                vm.result = Some(Self::strictly_equal(agent, left, right).into());
            }
            // Two values of one type are loosely equal where they are
            // strictly equal: nothing is converted.
            Instruction::IsLooselyEqual => {
                step!(IsLooselyEqual);
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
                step!(
                    JumpIfNotStrictlyEqual,
                    JumpIfStrictlyEqual,
                    JumpIfNotLooselyEqual,
                    JumpIfLooselyEqual
                );
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
                step!(JumpToCase);
                let value = vm.stack[instr.get_first_index()];
                if let Some(target) = code.switch_target(agent, instr.get_second_index(), value) {
                    *ip = target;
                }
            }
            Instruction::UpdateEmpty => {
                step!(UpdateEmpty);
                vm.execute_update_empty()
            }
            Instruction::LoadReplace => {
                step!(LoadReplace);
                vm.execute_load_replace()
            }
            Instruction::Empty => {
                step!(Empty);
                vm.execute_empty()
            }
            Instruction::StoreCopy => {
                step!(StoreCopy);
                vm.execute_store_copy()
            }
            Instruction::IsNullOrUndefined => {
                step!(IsNullOrUndefined);
                vm.execute_is_null_or_undefined()
            }
            Instruction::IsNull => {
                step!(IsNull);
                vm.execute_is_null()
            }
            Instruction::IsUndefined => {
                step!(IsUndefined);
                vm.execute_is_undefined()
            }
            Instruction::LogicalNot => {
                step!(LogicalNot);
                execute_logical_not(agent, vm)
            }
            // `typeof` of a value: nothing is run for it. A name that may
            // not be bound comes with its Reference, and goes the general
            // way, where every `typeof` went.
            Instruction::Typeof => {
                step!(Typeof);
                if vm.reference.is_some() {
                    return Plain::No;
                }
                let Some(value) = vm.result else {
                    return Plain::No;
                };
                vm.result = Some(typeof_operator(agent, value, gc).into());
            }
            Instruction::PushExceptionJumpTarget => {
                step!(PushExceptionJumpTarget);
                execute_push_exception_jump_target(agent, vm, instr, gc)
            }
            Instruction::PopExceptionJumpTarget => {
                step!(PopExceptionJumpTarget);
                vm.execute_pop_exception_jump_target()
            }
            Instruction::CreateImmutableBinding => {
                step!(CreateImmutableBinding);
                execute_create_immutable_binding(agent, executable.clone(), instr, gc)
            }
            Instruction::CreateMutableBinding => {
                step!(CreateMutableBinding);
                execute_create_mutable_binding(agent, executable.clone(), instr, gc)
            }
            Instruction::InitializeReferencedBinding => {
                step!(InitializeReferencedBinding);
                execute_initialize_referenced_binding(agent, vm, gc)
            }
            Instruction::EvaluatePropertyAccessWithIdentifierKey => {
                step!(EvaluatePropertyAccessWithIdentifierKey);
                Self::property_reference_with_identifier_key(agent, vm, tables, instr, gc)
            }
            Instruction::EvaluatePropertyAccessWithExpressionKey => {
                step!(EvaluatePropertyAccessWithExpressionKey);
                execute_evaluate_property_access_with_expression_key(agent, vm, gc)
            }
            // A variable of an enclosing function, where it was found the
            // last time.
            Instruction::GetBindingAtSite | Instruction::GetGlobalAtSite => {
                step!(GetBindingAtSite, GetGlobalAtSite);
                let (site, name) =
                    tables.keyed_binding_site(instr.get_second_index(), instr.get_first_index());
                let Some(value) = binding_value_at_site(agent, site, name, gc) else {
                    return Plain::No;
                };
                vm.result = Some(value.unbind());
            }
            // The same variable, pushed: an operand or an argument.
            Instruction::LoadBindingAtSite => {
                step!(LoadBindingAtSite);
                let (site, name) =
                    tables.keyed_binding_site(instr.get_second_index(), instr.get_first_index());
                let Some(value) = binding_value_at_site(agent, site, name, gc) else {
                    return Plain::No;
                };
                vm.result = None;
                vm.stack.push(value.unbind());
            }
            // A variable of an enclosing function that has one value for
            // as long as the call lasts: read from the stack slot that
            // keeps it, which the first read in the call fills. Undefined
            // there is nothing kept.
            Instruction::GetBindingAtSiteOnce => {
                step!(GetBindingAtSiteOnce);
                let slot = Instr::window_third_index(window);
                let Some(&kept) = vm.stack.get(slot) else {
                    return Plain::No;
                };
                let value = if kept.is_undefined() {
                    let (site, name) = tables
                        .keyed_binding_site(instr.get_second_index(), instr.get_first_index());
                    let Some(value) = binding_value_at_site(agent, site, name, gc) else {
                        return Plain::No;
                    };
                    vm.stack[slot] = value.unbind();
                    value.unbind()
                } else {
                    kept
                };
                vm.result = Some(value);
            }
            Instruction::LoadBindingAtSiteOnce => {
                step!(LoadBindingAtSiteOnce);
                let slot = Instr::window_third_index(window);
                let Some(&kept) = vm.stack.get(slot) else {
                    return Plain::No;
                };
                let value = if kept.is_undefined() {
                    let (site, name) = tables
                        .keyed_binding_site(instr.get_second_index(), instr.get_first_index());
                    let Some(value) = binding_value_at_site(agent, site, name, gc) else {
                        return Plain::No;
                    };
                    vm.stack[slot] = value.unbind();
                    value.unbind()
                } else {
                    kept
                };
                vm.result = None;
                vm.stack.push(value);
            }
            // The assignment to such a variable; and the assignment and
            // the read of it that follows, where what was assigned stays
            // the result: the variable has it, once it is set.
            Instruction::PutBindingAtSite
            | Instruction::PutGlobalAtSite
            | Instruction::PutBindingAtSiteAndGet => {
                step!(PutBindingAtSite, PutGlobalAtSite, PutBindingAtSiteAndGet);
                let Some(value) = vm.result else {
                    return Plain::No;
                };
                let (site, name) =
                    tables.keyed_binding_site(instr.get_second_index(), instr.get_first_index());
                if !set_binding_value_at_site(agent, site, name, value, gc) {
                    return Plain::No;
                }
                if instr.kind != Instruction::PutBindingAtSiteAndGet {
                    vm.result = None;
                }
            }
            // A name of the global scope that is about to be assigned to:
            // its Reference, made where the name was found the last time.
            Instruction::ResolveGlobalAtSite => {
                step!(ResolveGlobalAtSite);
                let (site, name) = tables.binding_site_with_name(instr.get_second_index(), gc);
                let strict = agent.is_evaluating_strict_code();
                let Some(reference) = try_resolve_binding_at_site(agent, site, name, strict, gc)
                else {
                    return Plain::No;
                };
                vm.reference = Some(reference.unbind());
            }
            // The same where the value is worked out without the reference
            // register: the name is found bound at its place and no
            // Reference is made of it.
            Instruction::CheckGlobalAtSite => {
                step!(CheckGlobalAtSite);
                let (site, name) =
                    tables.keyed_binding_site(instr.get_second_index(), instr.get_first_index());
                if !name_is_bound_at_global_site(agent, site, name, gc) {
                    return Plain::No;
                }
                vm.reference = None;
            }
            // The assignment through that Reference, at the place in the
            // global scope it is to; with no Reference (CheckGlobalAtSite
            // found the name bound), at the place where the name still is.
            Instruction::PutValueAtSite => {
                step!(PutValueAtSite);
                let Some(value) = vm.result else {
                    return Plain::No;
                };
                let (site, name) =
                    tables.keyed_binding_site(instr.get_second_index(), instr.get_first_index());
                let written = match &vm.reference {
                    Some(reference) => {
                        put_value_at_global_site(agent, site, name, reference, value, gc)
                    }
                    None => {
                        site.is_global() && set_binding_value_at_site(agent, site, name, value, gc)
                    }
                };
                if !written {
                    return Plain::No;
                }
                vm.result = None;
                vm.reference = None;
            }
            // The parameters of a call, from its arguments.
            Instruction::BindArguments => {
                step!(BindArguments);
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
            // `v instanceof F` where F is a function with the method every
            // function inherits and an object for its "prototype", both
            // found without running anything.
            Instruction::InstanceofOperator => {
                step!(InstanceofOperator);
                let (Some(&value), Some(target)) = (vm.stack.last(), vm.result) else {
                    return Plain::No;
                };
                let Some(found) = Self::ordinary_instance_of(agent, value, target, gc) else {
                    return Plain::No;
                };
                vm.stack.pop();
                vm.result = Some(found.into());
            }
            // A parameter with a default value or after one, bound from the
            // list of the call's arguments: the next argument, or undefined
            // where the call gave no more.
            Instruction::IteratorStepValueOrUndefined => {
                step!(IteratorStepValueOrUndefined);
                let Some(VmIteratorRecord::SliceIterator(arguments)) = vm.iterator_stack.last()
                else {
                    return Plain::No;
                };
                let value = arguments.unshift(agent, gc).unwrap_or(Value::Undefined);
                vm.result = Some(value.unbind());
            }
            // A rest parameter: the arguments that are left, as an Array.
            // Of a call with few arguments; a long list is asked for where
            // the Array can be refused.
            Instruction::IteratorRestIntoArray => {
                step!(IteratorRestIntoArray);
                let Some(VmIteratorRecord::SliceIterator(arguments)) = vm.iterator_stack.last()
                else {
                    return Plain::No;
                };
                if arguments.len(agent) > REST_IN_PLACE {
                    return Plain::No;
                }
                let mut rest = [Value::Undefined; REST_IN_PLACE];
                let mut count = 0;
                while let Some(value) = arguments.unshift(agent, gc) {
                    rest[count] = value.unbind();
                    count += 1;
                }
                let array = crate::ecmascript::create_array_from_list(agent, &rest[..count], gc);
                vm.result = Some(Value::from(array).unbind());
            }
            // The list of the call's arguments, when the parameters are
            // bound.
            Instruction::IteratorPop => {
                step!(IteratorPop);
                if !matches!(
                    vm.iterator_stack.last(),
                    Some(VmIteratorRecord::SliceIterator(_) | VmIteratorRecord::EmptySliceIterator)
                ) {
                    return Plain::No;
                }
                vm.iterator_stack.pop();
            }
            // A call of a built-in function that only computes a value from
            // arguments like these: computed here, with no call made.
            Instruction::EvaluateCall
            | Instruction::PopReferenceAndEvaluateCall
            | Instruction::EvaluateCallOnThis => {
                step!(
                    EvaluateCall,
                    PopReferenceAndEvaluateCall,
                    EvaluateCallOnThis
                );
                let count = instr.get_first_index();
                let Some(first) = vm.stack.len().checked_sub(count) else {
                    return Plain::No;
                };
                if count == IndexType::MAX as usize || first == 0 {
                    return Plain::No;
                }
                let function = match vm.stack[first - 1] {
                    Value::BuiltinFunction(function) => function,
                    Value::ECMAScriptFunction(_) | Value::BoundFunction(_) => return Plain::Call,
                    _ => return Plain::No,
                };
                let Some(direct) = function.get(agent).direct else {
                    return Plain::No;
                };
                // The this value of the call, for the functions that take
                // it: the base of the Reference the call is made through,
                // or the value that a method call keeps under the function.
                let on_this = instr.kind == Instruction::EvaluateCallOnThis;
                let this = if !direct.takes_this() {
                    None
                } else if on_this {
                    first.checked_sub(2).map(|at| vm.stack[at])
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
                vm.stack
                    .truncate((first - 1).saturating_sub(usize::from(on_this)));
                if instr.kind == Instruction::PopReferenceAndEvaluateCall {
                    vm.reference_stack.pop();
                }
                vm.reference = None;
                vm.result = Some(result);
            }
            // `o.p` where the property cache knows the shape of `o`, and the
            // length of an Array.
            Instruction::GetPropertyWithCache | Instruction::GetPropertyWithCacheKeepReference => {
                step!(GetPropertyWithCache, GetPropertyWithCacheKeepReference);
                let Some(base) = vm.result else {
                    return Plain::No;
                };
                if !Self::cached_property_into_result(agent, vm, base, tables, instr, gc) {
                    // As the instruction found it.
                    vm.result = Some(base);
                    if instr.kind == Instruction::GetPropertyWithCache
                        && Self::getter_for_call(agent, vm, base, tables, instr, gc)
                    {
                        return Plain::Accessor;
                    }
                    return Plain::No;
                }
                if instr.kind == Instruction::GetPropertyWithCacheKeepReference {
                    let found = vm.result;
                    vm.result = Some(base);
                    Self::property_reference_with_identifier_key(agent, vm, tables, instr, gc);
                    vm.result = found;
                } else {
                    vm.reference = None;
                }
            }
            // The function of a method call, pushed with its Reference
            // before the arguments.
            //
            // Or pushed after the value it is read from, which is the this
            // value of the call: no Reference is made for that, and the
            // call finds it under the function. (One arm for the two, and
            // one with the reads of a local's property below: an arm of
            // its own for each made the loop of instructions 440
            // instructions longer, by the search of the cache read into
            // each, and every instruction of a script five slower.)
            Instruction::LoadPropertyWithCachePushReference
            | Instruction::LoadPropertyWithCachePushThis => {
                step!(
                    LoadPropertyWithCachePushReference,
                    LoadPropertyWithCachePushThis
                );
                let Some(value) = Self::cached_property_value(agent, vm, tables, instr, gc) else {
                    return Plain::No;
                };
                if instr.kind == Instruction::LoadPropertyWithCachePushReference {
                    Self::property_reference_with_identifier_key(agent, vm, tables, instr, gc);
                    vm.execute_push_reference(agent);
                } else {
                    vm.reference = None;
                    vm.stack.extend(vm.result.take());
                }
                vm.stack.push(value);
            }
            // `p.x` of a local: the stack slot, and its property by the
            // cache.
            //
            // Or `o.x` of a name of an enclosing function that has one
            // value for as long as the call lasts: the stack slot is the
            // one that keeps the value once the call has read it, and
            // undefined there is nothing kept. The compiler emits this
            // instruction only after a read of the same name in the same
            // run of code that nothing jumps into, so the slot is filled
            // when it runs, unless the value of the name is undefined;
            // the arm does nothing for an empty slot, which has no
            // property that the cache places and goes the general way.
            // Every way of filling the slot from here cost what the loop
            // does for every instruction or for every call (see PERF.md,
            // area 4).
            Instruction::GetIndexPropertyWithCache
            | Instruction::GetIndexPropertyWithCacheKeepReference
            | Instruction::LoadIndexPropertyWithCachePushThis
            | Instruction::GetKeptPropertyWithCache
            | Instruction::LoadIndexPropertyWithCache => {
                step!(
                    GetIndexPropertyWithCache,
                    GetIndexPropertyWithCacheKeepReference,
                    LoadIndexPropertyWithCachePushThis,
                    GetKeptPropertyWithCache,
                    LoadIndexPropertyWithCache
                );
                let base = vm.stack[Instr::window_third_index(window)];
                if !Self::cached_property_into_result(agent, vm, base, tables, instr, gc) {
                    if matches!(
                        instr.kind,
                        Instruction::GetIndexPropertyWithCache
                            | Instruction::GetKeptPropertyWithCache
                    ) && Self::getter_for_call(agent, vm, base, tables, instr, gc)
                    {
                        return Plain::Accessor;
                    }
                    return Plain::No;
                }
                match instr.kind {
                    Instruction::GetIndexPropertyWithCache
                    | Instruction::GetKeptPropertyWithCache => {
                        vm.reference = None;
                    }
                    Instruction::GetIndexPropertyWithCacheKeepReference => {
                        let found = vm.result;
                        vm.result = Some(base);
                        Self::property_reference_with_identifier_key(agent, vm, tables, instr, gc);
                        vm.result = found;
                    }
                    // The value, pushed.
                    Instruction::LoadIndexPropertyWithCache => {
                        vm.reference = None;
                        vm.stack.extend(vm.result.take());
                    }
                    // The function of `p.m(x)`, pushed after `p`, the this
                    // value of the call.
                    _ => {
                        let found = vm.result.take();
                        vm.reference = None;
                        vm.stack.push(base);
                        vm.stack.extend(found);
                    }
                }
            }
            // `this.p`: the this value that the call keeps, and its
            // property by the cache.
            Instruction::GetThisPropertyWithCache
            | Instruction::GetThisPropertyWithCacheKeepReference => {
                step!(
                    GetThisPropertyWithCache,
                    GetThisPropertyWithCacheKeepReference
                );
                let Some(this) = agent.running_call_this_value() else {
                    return Plain::No;
                };
                if !Self::cached_property_into_result(agent, vm, this, tables, instr, gc) {
                    if instr.kind == Instruction::GetThisPropertyWithCache
                        && Self::getter_for_call(agent, vm, this, tables, instr, gc)
                    {
                        return Plain::Accessor;
                    }
                    return Plain::No;
                }
                if instr.kind == Instruction::GetThisPropertyWithCacheKeepReference {
                    let found = vm.result;
                    vm.result = Some(this);
                    Self::property_reference_with_identifier_key(agent, vm, tables, instr, gc);
                    vm.result = found;
                } else {
                    vm.reference = None;
                }
            }
            // `new F(…)` of an ECMAScript function, which the Vm may run
            // itself.
            Instruction::EvaluateNew => {
                step!(EvaluateNew);
                let count = instr.get_first_index();
                if count != IndexType::MAX as usize
                    && let Some(at) = vm.stack.len().checked_sub(count + 1)
                    && matches!(vm.stack[at], Value::ECMAScriptFunction(_))
                {
                    return Plain::Construct;
                }
                return Plain::No;
            }
            // `o.p = v` with `o` on the stack, where the property cache
            // knows its shape and `p` is a writable data property of its
            // own.
            Instruction::PutPropertyWithCache => {
                step!(PutPropertyWithCache);
                let (Some(value), Some(&Value::Object(object))) = (vm.result, vm.stack.last())
                else {
                    return Plain::No;
                };
                let cache = tables.cache(instr.get_second_index(), gc);
                match object.set_value_by_cache(agent, cache, value) {
                    CachedSet::Done => {}
                    CachedSet::Absent => {
                        // Neither the object nor any it inherits from has
                        // the property: it is added, as most properties of
                        // an object are.
                        let key = tables.property_key(instr.get_first_index(), gc);
                        if !ordinary_add_own_data_by_cache(agent, object, key, value, cache, gc) {
                            return Plain::No;
                        }
                    }
                    CachedSet::Unknown => {
                        if let Some(setter) = object.setter_by_cache(agent, cache, gc) {
                            // The setter and the value are the function and
                            // the argument of a call, and the object its
                            // this value.
                            vm.stack.pop();
                            vm.stack.push(Value::ECMAScriptFunction(setter.unbind()));
                            vm.stack.push(value);
                            vm.result = Some(Value::Object(object));
                            return Plain::Accessor;
                        }
                        return Plain::No;
                    }
                }
                vm.stack.pop();
                vm.result = None;
            }
            // `p.x = v` of a local `p`: as PutPropertyWithCache, with the
            // object in its stack slot and nothing to pop.
            Instruction::PutIndexPropertyWithCache => {
                step!(PutIndexPropertyWithCache);
                let (Some(value), Value::Object(object)) =
                    (vm.result, vm.stack[Instr::window_third_index(window)])
                else {
                    return Plain::No;
                };
                let cache = tables.cache(instr.get_second_index(), gc);
                match object.set_value_by_cache(agent, cache, value) {
                    CachedSet::Done => {}
                    CachedSet::Absent => {
                        let key = tables.property_key(instr.get_first_index(), gc);
                        if !ordinary_add_own_data_by_cache(agent, object, key, value, cache, gc) {
                            return Plain::No;
                        }
                    }
                    CachedSet::Unknown => {
                        if let Some(setter) = object.setter_by_cache(agent, cache, gc) {
                            // As there: the setter, the value and the
                            // object are a call.
                            vm.stack.push(Value::ECMAScriptFunction(setter.unbind()));
                            vm.stack.push(value);
                            vm.result = Some(Value::Object(object));
                            return Plain::Accessor;
                        }
                        return Plain::No;
                    }
                }
                vm.result = None;
            }
            // `o.p = v` where the property cache knows the shape of `o` and
            // `p` is a writable data property of its own.
            Instruction::PutValueWithCache => {
                step!(PutValueWithCache);
                let (Some(value), Some(reference)) = (vm.result, vm.reference.as_ref()) else {
                    return Plain::No;
                };
                let Some(Value::Object(object)) = reference.named_property_base() else {
                    return Plain::No;
                };
                let cache = tables.cache(instr.get_first_index(), gc);
                if object.set_value_by_cache(agent, cache, value) != CachedSet::Done {
                    return Plain::No;
                }
                vm.result = None;
                vm.reference = None;
            }
            // `--o.p` and `o.p++` of an integer that stays one, where the
            // property cache knows the shape of `o` and `p` is a writable
            // data property of its own: the value that was just read is
            // stepped and assigned, and the result register is left with
            // what the instructions this one stands for leave in it.
            Instruction::StepValueWithCache => {
                step!(StepValueWithCache);
                let (Some(Value::Integer(before)), Some(reference)) =
                    (vm.result, vm.reference.as_ref())
                else {
                    return Plain::No;
                };
                let Some(Value::Object(object)) = reference.named_property_base() else {
                    return Plain::No;
                };
                let which = instr.get_second_index();
                let step = if which & Instruction::STEP_DOWN != 0 {
                    -1
                } else {
                    1
                };
                let Ok(after) = SmallInteger::try_from(before.into_i64() + step) else {
                    return Plain::No;
                };
                let cache = tables.cache(instr.get_first_index(), gc);
                if object.set_value_by_cache(agent, cache, Value::Integer(after)) != CachedSet::Done
                {
                    return Plain::No;
                }
                vm.reference = None;
                vm.result = if which & Instruction::STEP_KEEPS_NEW != 0 {
                    Some(Value::Integer(after))
                } else if which & Instruction::STEP_KEEPS_OLD != 0 {
                    Some(Value::Integer(before))
                } else {
                    None
                };
            }
            // `a[i]` of an Array that has an element there, or of a
            // TypedArray within its length.
            Instruction::GetValue | Instruction::GetValueKeepReference => {
                step!(GetValue, GetValueKeepReference);
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
                step!(GetValueWithExpressionKey);
                let (Some(&base), Some(Value::Integer(index))) = (vm.stack.last(), vm.result)
                else {
                    // `o[name]`, a String for the key, or nothing for here.
                    return Self::named_value_with_expression_key(agent, vm, gc);
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
                    let Some(value) = Self::exotic_element(agent, base, index, gc) else {
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
                step!(GetValueWithIndexKey);
                let (Some(base), Value::Integer(index)) =
                    (vm.result, vm.stack[instr.get_first_index()])
                else {
                    // `o[name]`, the name in a local (the variable of a
                    // `for`-`in` loop over the object, for one), or nothing
                    // for here.
                    return Self::named_value_with_index_key(agent, vm, instr, gc);
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
                    let Some(value) = Self::exotic_element(agent, base, index, gc) else {
                        return Plain::No;
                    };
                    value
                };
                vm.result = Some(value);
            }
            // `a[i] = v` for an element the Array has, as a plain value, or
            // for a TypedArray and a Number. (The storage is looked at as
            // it was here, and once in the arm below: with both arms
            // changed the loop of instructions was laid out anew all
            // over, and two of the call cases were 3-4% slower.)
            Instruction::PutValue => {
                step!(PutValue);
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
                step!(PutValueWithExpressionKey);
                let (Some(value), [.., base, Value::Integer(index)]) =
                    (vm.result, vm.stack.as_slice())
                else {
                    // `o[name] = v`, a String for the key, or nothing for
                    // here.
                    return Self::set_named_value_with_expression_key(agent, vm, gc);
                };
                let (base, index) = (*base, index.into_i64());
                if let Value::Array(array) = base {
                    let Ok(index) = usize::try_from(index) else {
                        return Plain::No;
                    };
                    if !array.set_writable_element(agent, index, value) {
                        return Plain::No;
                    }
                } else if !Self::set_typed_array_element(agent, base, index, value) {
                    return Plain::No;
                }
                vm.stack.truncate(vm.stack.len() - 2);
                vm.result = None;
            }
            // A part of a concatenation that is a String, or a primitive
            // that ToString takes as it is: most are.
            Instruction::ToStringOperand | Instruction::ToStringSubstitution => {
                step!(ToStringOperand, ToStringSubstitution);
                match vm.result {
                    Some(Value::String(_) | Value::SmallString(_)) => {}
                    Some(value) => {
                        let Ok(primitive) = Primitive::try_from(value) else {
                            return Plain::No;
                        };
                        if matches!(primitive, Primitive::Symbol(_)) {
                            return Plain::No;
                        }
                        let Ok(string) = to_string_primitive(agent, primitive, gc) else {
                            return Plain::No;
                        };
                        vm.result = Some(string.unbind().into());
                    }
                    None => return Plain::No,
                }
            }
            // The parts of a concatenation as one String, where the text is
            // short: a longer one is asked for where it can be refused.
            Instruction::ConcatStrings => {
                step!(ConcatStrings);
                let count = instr.get_first_index();
                let Some(first) = vm.stack.len().checked_sub(count) else {
                    return Plain::No;
                };
                let Some(string) = concat_short_strings(agent, &vm.stack[first..], gc) else {
                    return Plain::No;
                };
                let string = string.unbind();
                vm.stack.truncate(first);
                vm.result = Some(string.into());
            }
            // A Number is its own numeric value: the counter of nearly every
            // loop.
            Instruction::ToNumeric => {
                step!(ToNumeric);
                if !matches!(
                    vm.result,
                    Some(Value::Integer(_) | Value::SmallF64(_) | Value::Number(_))
                ) {
                    return Plain::No;
                }
            }
            Instruction::Increment => {
                step!(Increment);
                return vm.step_integer(1);
            }
            Instruction::Decrement => {
                step!(Decrement);
                return vm.step_integer(-1);
            }
            // `-x` of a Number, which the general way was the only way
            // for: a call from here, so that the loop has no more of it
            // than the call.
            Instruction::UnaryMinus => {
                step!(UnaryMinus);
                return vm.negate_number(agent, gc);
            }
            Instruction::LessThan => operate_on_integers!(LessThan),
            Instruction::LessThanEquals => operate_on_integers!(LessThanEquals),
            Instruction::GreaterThan => operate_on_integers!(GreaterThan),
            Instruction::GreaterThanEquals => operate_on_integers!(GreaterThanEquals),
            Instruction::ApplyAdditionBinaryOperator => {
                operate_on_integers!(ApplyAdditionBinaryOperator)
            }
            Instruction::ApplySubtractionBinaryOperator => {
                operate_on_integers!(ApplySubtractionBinaryOperator)
            }
            Instruction::ApplyMultiplicationBinaryOperator => {
                operate_on_integers!(ApplyMultiplicationBinaryOperator)
            }
            Instruction::ApplyDivisionBinaryOperator => {
                operate_on_integers!(ApplyDivisionBinaryOperator)
            }
            Instruction::ApplyRemainderBinaryOperator => {
                operate_on_integers!(ApplyRemainderBinaryOperator)
            }
            Instruction::ApplyShiftLeftBinaryOperator => {
                operate_on_integers!(ApplyShiftLeftBinaryOperator)
            }
            Instruction::ApplyShiftRightBinaryOperator => {
                operate_on_integers!(ApplyShiftRightBinaryOperator)
            }
            Instruction::ApplyShiftRightZeroFillBinaryOperator => {
                operate_on_integers!(ApplyShiftRightZeroFillBinaryOperator)
            }
            Instruction::ApplyBitwiseORBinaryOperator => {
                operate_on_integers!(ApplyBitwiseORBinaryOperator)
            }
            Instruction::ApplyBitwiseXORBinaryOperator => {
                operate_on_integers!(ApplyBitwiseXORBinaryOperator)
            }
            Instruction::ApplyBitwiseAndBinaryOperator => {
                operate_on_integers!(ApplyBitwiseAndBinaryOperator)
            }
            _ => {
                // An instruction that is not plain.
                *ip += instr.kind.length();
                return Plain::No;
            }
        }
        Plain::Done
    }

    /// One of the instructions that compare the value on the stack with the
    /// result and jump: `kind`, which each of their arms gives as the
    /// constant it is, so that what the kind decides is decided when this
    /// is compiled into the arm.
    #[inline(always)]
    fn compare_and_jump(
        agent: &mut Agent,
        vm: &mut Vm,
        ip: &mut usize,
        instr: Instr,
        kind: Instruction,
    ) -> Plain {
        let (Some(&left), Some(right)) = (vm.stack.last(), vm.result) else {
            return Plain::No;
        };
        let jumps = if let (Value::Integer(left), Value::Integer(right)) = (left, right) {
            let (left, right) = (left.into_i64(), right.into_i64());
            match kind {
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
            let (comparison, negated) = match kind {
                Instruction::JumpIfNotLessThan => (Instruction::LessThan, true),
                Instruction::JumpIfNotLessThanEquals => (Instruction::LessThanEquals, true),
                Instruction::JumpIfNotGreaterThan => (Instruction::GreaterThan, true),
                Instruction::JumpIfNotGreaterThanEquals => (Instruction::GreaterThanEquals, true),
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
        Plain::Done
    }

    /// One of the instructions that compare a local with a constant or with
    /// another local and jump: `kind`, a constant of the arm as in
    /// [`Vm::compare_and_jump`].
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn compare_local_and_jump(
        agent: &mut Agent,
        vm: &mut Vm,
        ip: &mut usize,
        window: &[u8; Instr::WINDOW],
        tables: TablesView,
        instr: Instr,
        kind: Instruction,
        gc: NoGcScope,
    ) -> Plain {
        let Some((comparison, constant)) = kind.compared_local() else {
            return Plain::No;
        };
        let left = vm.stack[instr.get_first_index()];
        let right = if constant {
            tables.constant(instr.get_second_index(), gc).unbind()
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
                let Some(holds) = Self::numbers_compare(agent, comparison, left, right) else {
                    return Plain::No;
                };
                !holds
            }
        };
        vm.result = None;
        if jumps {
            let from = *ip;
            *ip = Instr::window_jump_target(window);
            if *ip < from {
                return Plain::Looped;
            }
        }
        Plain::Done
    }

    /// JumpIfResultComparesWithConstant: what the result register holds
    /// compared with a constant, and the jump where the instruction says;
    /// the register is emptied, as the comparison it stands for empties
    /// it. Two integers and two Numbers here; anything else is left to the
    /// general way with nothing changed, as the comparisons of a local
    /// leave it.
    // Part of its arm. As a function that the arm calls it made the
    // calls of a script no shorter than it does here (they are three to
    // four instructions longer with this instruction in the loop, however
    // it is written) and the comparison six longer.
    #[inline(always)]
    fn compare_result_with_constant_and_jump(
        agent: &mut Agent,
        vm: &mut Vm,
        ip: &mut usize,
        window: &[u8; Instr::WINDOW],
        tables: TablesView,
        instr: Instr,
        gc: NoGcScope,
    ) -> Plain {
        let Some(left) = vm.result else {
            return Plain::No;
        };
        let right = tables.constant(instr.get_second_index(), gc).unbind();
        let comparison = instr.get_first_index();
        let jumps = if comparison >= Instruction::RESULT_COMPARISON_STRICT {
            Self::strictly_equal(agent, left, right)
                == (comparison == Instruction::RESULT_COMPARISON_STRICT)
        } else {
            let holds = match (left, right) {
                (Value::Integer(left), Value::Integer(right)) => {
                    let (left, right) = (left.into_i64(), right.into_i64());
                    match comparison & 3 {
                        0 => left < right,
                        1 => left <= right,
                        2 => left > right,
                        _ => left >= right,
                    }
                }
                (left, right) => {
                    let order = match comparison & 3 {
                        0 => Instruction::LessThan,
                        1 => Instruction::LessThanEquals,
                        2 => Instruction::GreaterThan,
                        _ => Instruction::GreaterThanEquals,
                    };
                    let Some(holds) = Self::numbers_compare(agent, order, left, right) else {
                        return Plain::No;
                    };
                    holds
                }
            };
            holds != (comparison & Instruction::RESULT_COMPARISON_NOT != 0)
        };
        vm.result = None;
        if jumps {
            let from = *ip;
            *ip = Instr::window_jump_target(window);
            if *ip < from {
                return Plain::Looped;
            }
        }
        Plain::Done
    }

    /// One of the instructions that end a loop: a step of a local where
    /// `kind` has one, its comparison and the jump back. `kind` is a
    /// constant of the arm as in [`Vm::compare_and_jump`].
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn step_local_compare_and_jump(
        agent: &mut Agent,
        vm: &mut Vm,
        ip: &mut usize,
        window: &[u8; Instr::WINDOW],
        tables: TablesView,
        instr: Instr,
        kind: Instruction,
        gc: NoGcScope,
    ) -> Plain {
        let Some((step, comparison, constant)) = kind.loop_test() else {
            return Plain::No;
        };
        let slot = instr.get_first_index();
        let right = if constant {
            tables.constant(instr.get_second_index(), gc).unbind()
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
                        let Ok(stepped) = SmallInteger::try_from(left.into_i64() + step as i64)
                        else {
                            return Plain::No;
                        };
                        Some(Value::Integer(stepped))
                    }
                    _ => return Plain::No,
                };
                let Some(holds) =
                    Self::numbers_compare(agent, comparison, stepped.unwrap_or(left), right)
                else {
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
            *ip = Instr::window_jump_target(window);
            if *ip < from {
                return Plain::Looped;
            }
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
        tables: TablesView,
        instr: Instr,
        gc: NoGcScope,
    ) {
        let property_key = tables.property_key(instr.get_first_index(), gc);
        let base_value = vm.result.take().unwrap().bind(gc);
        let strict = agent.is_evaluating_strict_code();
        vm.reference =
            Some(Reference::new_property_reference(base_value, property_key, strict).unbind());
    }

    /// Where `base.name` is an accessor whose getter this `Vm` may run
    /// itself (the cache of the instruction knows the shape of `base`, and
    /// the getter is an ECMAScript function): leaves the getter on the
    /// stack and `base` as the result, for [`Plain::Accessor`], and returns
    /// true.
    #[inline(always)]
    fn getter_for_call(
        agent: &Agent,
        vm: &mut Vm,
        base: Value,
        tables: TablesView,
        instr: Instr,
        gc: NoGcScope,
    ) -> bool {
        let Value::Object(object) = base else {
            return false;
        };
        let cache = tables.cache(instr.get_second_index(), gc);
        let Some(getter) = object.getter_by_cache(agent, cache, gc) else {
            return false;
        };
        vm.stack.push(Value::ECMAScriptFunction(getter.unbind()));
        vm.result = Some(base.unbind());
        true
    }

    /// The value of `o.p` for the object in the result register, when it is
    /// read without a call: a property the cache places for the shape of an
    /// ordinary object (its own or of a prototype, or known to be absent),
    /// or the length of an Array.
    #[inline(always)]
    fn cached_property_value(
        agent: &mut Agent,
        vm: &Vm,
        tables: TablesView,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        Self::cached_property_value_of(agent, vm.result?, tables, instr, gc)
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
        let tables = executable.get_tables(agent);
        let tables = tables.view();
        let Some(value) = Self::cached_property_value(agent, vm, tables, instr, gc) else {
            return false;
        };
        if keep_reference {
            Self::property_reference_with_identifier_key(agent, vm, tables, instr, gc);
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
    // A function of its own: it is reached from every arm that reads a
    // property by its cache, and as part of each of them the calls read
    // 1-4% slower for where the loop's code then lay.
    #[inline(never)]
    //
    // It takes the cache, as the other functions that an arm calls for a
    // property take the cache and the name: given the tables to fetch them
    // from, which are six words that no register holds together, each arm
    // that can come here wrote the six to the stack where it starts, for
    // every property read, eight instructions.
    fn cached_prototype_property_value(
        agent: &mut Agent,
        prototype: OrdinaryObject<'static>,
        cache: PropertyLookupCache,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        // What a read of that name from the prototype object itself finds
        // by the cache: the one function that reads a property of an
        // ordinary object so, where this read the storage of the prototype
        // by three more general ones at every call of a method of a String
        // or a Number.
        prototype.value_by_cache(agent, cache, gc)
    }

    /// [`Vm::cached_property_value_of`] with what is found put in the
    /// result register, and whether anything was; nothing found leaves the
    /// register empty. Every way that finds a value writes it there as the
    /// eight bytes it has it in, and the register is asked whether it holds
    /// one. Handed back to an arm that looked at it and then wrote it, a
    /// value was written as four pieces (its kind, and its seven bytes as
    /// four, two and one), ten instructions for the one store, and what
    /// loads the register whole next had the four to wait for.
    #[inline(always)]
    fn cached_property_into_result(
        agent: &mut Agent,
        vm: &mut Vm,
        base: Value,
        tables: TablesView,
        instr: Instr,
        gc: NoGcScope,
    ) -> bool {
        match base {
            Value::Object(object) => {
                let cache = tables.cache(instr.get_second_index(), gc);
                vm.result = object.value_by_cache(agent, cache, gc);
            }
            Value::Arguments(arguments) => {
                let cache = tables.cache(instr.get_second_index(), gc);
                vm.result = arguments.ordinary_object().value_by_cache(agent, cache, gc);
            }
            _ => {
                vm.result = Self::cached_property_value_of(agent, base, tables, instr, gc);
            }
        }
        vm.result.is_some()
    }

    /// The same for `base`, wherever the instruction has it from.
    #[inline(always)]
    fn cached_property_value_of(
        agent: &mut Agent,
        base: Value,
        tables: TablesView,
        instr: Instr,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        match base {
            Value::Object(object) => {
                let cache = tables.cache(instr.get_second_index(), gc);
                object.value_by_cache(agent, cache, gc)
            }
            // `arguments.length`: an arguments object has its properties as
            // the ordinary object it is has them.
            Value::Arguments(arguments) => {
                let cache = tables.cache(instr.get_second_index(), gc);
                arguments.ordinary_object().value_by_cache(agent, cache, gc)
            }
            Value::Array(array) => {
                let key = tables.property_key(instr.get_first_index(), gc);
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
                let cache = tables.cache(instr.get_second_index(), gc);
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
                let key = tables.property_key(instr.get_first_index(), gc);
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
                let cache = tables.cache(instr.get_second_index(), gc);
                Self::cached_prototype_property_value(agent, prototype, cache, gc)
            }
            // A Number value has no properties of its own: what \[\[Get]]
            // of %Number.prototype% finds in the cache (`n.toFixed`).
            Value::Integer(_) | Value::SmallF64(_) | Value::Number(_) => {
                let prototype = agent
                    .current_realm_record()
                    .intrinsics()
                    .number_prototype_backing_object();
                let cache = tables.cache(instr.get_second_index(), gc);
                Self::cached_prototype_property_value(agent, prototype, cache, gc)
            }
            #[cfg(feature = "array-buffer")]
            value => {
                let key = tables.property_key(instr.get_first_index(), gc);
                let cache = tables.cache(instr.get_second_index(), gc);
                Self::exotic_property_value(agent, value, key, cache, gc)
            }
            #[cfg(not(feature = "array-buffer"))]
            _ => None,
        }
    }

    /// GetValueWithExpressionKey where the key is not an integer: `o[name]`
    /// for a String, read as [`Vm::own_named_value`] reads it, with the
    /// base taken off the stack as the instruction does. Nothing has
    /// changed where it is not done.
    // The three of these are not part of the loop of instructions: the
    // read and the write of an element by its index, which they share
    // their instructions with, are, and with these in their arms those
    // were 2-5% slower.
    #[inline(never)]
    fn named_value_with_expression_key(agent: &Agent, vm: &mut Vm, gc: NoGcScope) -> Plain {
        let (Some(&base), Some(name @ (Value::String(_) | Value::SmallString(_)))) =
            (vm.stack.last(), vm.result)
        else {
            return Plain::No;
        };
        let Some(value) = Self::own_named_value(agent, base, name, gc) else {
            return Plain::No;
        };
        vm.stack.pop();
        vm.result = Some(value);
        Plain::Done
    }

    /// GetValueWithIndexKey where the local does not hold an integer.
    #[inline(never)]
    fn named_value_with_index_key(
        agent: &Agent,
        vm: &mut Vm,
        instr: Instr,
        gc: NoGcScope,
    ) -> Plain {
        let (Some(base), name @ (Value::String(_) | Value::SmallString(_))) =
            (vm.result, vm.stack[instr.get_first_index()])
        else {
            return Plain::No;
        };
        let Some(value) = Self::own_named_value(agent, base, name, gc) else {
            return Plain::No;
        };
        vm.result = Some(value);
        Plain::Done
    }

    /// PutValueWithExpressionKey where the key is not an integer.
    #[inline(never)]
    fn set_named_value_with_expression_key(agent: &mut Agent, vm: &mut Vm, gc: NoGcScope) -> Plain {
        let (Some(value), [.., base, name @ (Value::String(_) | Value::SmallString(_))]) =
            (vm.result, vm.stack.as_slice())
        else {
            return Plain::No;
        };
        if !Self::set_own_named_value(agent, *base, *name, value, gc) {
            return Plain::No;
        }
        vm.stack.truncate(vm.stack.len() - 2);
        vm.result = None;
        Plain::Done
    }

    /// `base[name]` where `base` is an ordinary object and `name`, a String,
    /// is the key of one of its own data properties: the value of that.
    /// None for anything else, with nothing changed: another kind of
    /// object, a key the object does not have itself (the general way
    /// looks up the chain), an accessor.
    #[inline]
    fn own_named_value(
        agent: &Agent,
        base: Value,
        name: Value,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let Value::Object(object) = base else {
            return None;
        };
        // A String that is the text of an integer is that integer for a key.
        let key = to_property_key_simple(agent, name, gc)?;
        object.own_data_value(agent, key).map(Value::unbind)
    }

    /// `base[name] = value` where `base` is an ordinary object and `name`,
    /// a String, is the key of one of its own data properties that is
    /// writable: the property has the value, as OrdinarySet leaves it with
    /// the object for the receiver. False for anything else, with nothing
    /// changed.
    #[inline]
    fn set_own_named_value(
        agent: &mut Agent,
        base: Value,
        name: Value,
        value: Value,
        gc: NoGcScope,
    ) -> bool {
        let Value::Object(object) = base else {
            return false;
        };
        let Some(key) = to_property_key_simple(agent, name, gc) else {
            return false;
        };
        object.set_own_writable_value(agent, key.unbind(), value.unbind())
    }

    /// `base[index]` where `base` is an arguments object or a TypedArray:
    /// `arguments[i]`, the loop of a function that takes any number of
    /// arguments, reads the argument where the object has it
    /// ([`UnmappedArguments::argument`](crate::ecmascript::UnmappedArguments)),
    /// and a TypedArray its element.
    // A function of its own, which it was by the compiler's choice while
    // its work was calls: part of the loop of instructions, the element of
    // every kind of TypedArray read in place made every call there six
    // instructions longer.
    #[inline(never)]
    fn exotic_element(
        agent: &mut Agent,
        base: Value,
        index: i64,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        // Each of the two by a function called last, and the element of a
        // TypedArray whose view was last seen read here: with either as a
        // call in the middle, this function kept a frame of six registers
        // for it on every way through.
        if let Value::Arguments(arguments) = base {
            return Self::argument_of_arguments(agent, arguments, index);
        }
        Self::typed_array_element(agent, base, index, gc)
    }

    /// `arguments[index]` where the arguments object has the argument.
    #[inline(never)]
    fn argument_of_arguments(
        agent: &mut Agent,
        arguments: crate::ecmascript::UnmappedArguments,
        index: i64,
    ) -> Option<Value<'static>> {
        arguments
            .argument(agent, u32::try_from(index).ok()?)
            .map(Value::unbind)
    }

    /// `a[i]` of a TypedArray, for an index within its length: the element,
    /// read from the buffer. Outside the length the general way gives
    /// undefined.
    #[cfg(feature = "array-buffer")]
    #[inline(always)]
    fn typed_array_element(
        agent: &mut Agent,
        base: Value,
        index: i64,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let array = AnyTypedArray::try_from(base).ok()?;
        // Read where the view of the TypedArray was last seen, with no
        // call; everything else by a function of its own, called last, so
        // that this one keeps no frame for it.
        if let Some(value) = array.element_value_seen(agent, index) {
            return Some(value);
        }
        Self::typed_array_element_unseen(agent, array, index, gc)
    }

    /// [`Vm::typed_array_element`] where the view of the TypedArray is not
    /// the one last seen, or its element is not read from it as it is: the
    /// view is seen, and TypedArrayGetElement answers.
    #[cfg(feature = "array-buffer")]
    #[inline(never)]
    fn typed_array_element_unseen(
        agent: &mut Agent,
        array: AnyTypedArray,
        index: i64,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
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
        // Written where the view of the TypedArray was last seen, by a
        // function of its own that calls nothing; everything else as it
        // was, here.
        AnyTypedArray::try_from(base).is_ok_and(|array| {
            Self::set_typed_array_element_seen(agent, array, index, value)
                || array.set_element_unconverted(agent, index, value)
        })
    }

    /// [`AnyTypedArray::set_element_seen`], as a function of its own: part
    /// of the loop of instructions, the element of every kind of TypedArray
    /// written in place is that much more of it.
    #[cfg(feature = "array-buffer")]
    #[inline(never)]
    fn set_typed_array_element_seen(
        agent: &mut Agent,
        array: AnyTypedArray,
        index: i64,
        value: Value,
    ) -> bool {
        array.set_element_seen(agent, index, value)
    }

    #[cfg(not(feature = "array-buffer"))]
    #[inline]
    fn set_typed_array_element(_: &mut Agent, _: Value, _: i64, _: Value) -> bool {
        false
    }

    /// A property of a value that is none of those the arms of
    /// [`Vm::cached_property_value_of`] are for: of a function, or the
    /// length of a TypedArray.
    ///
    /// A function that has properties has them in an ordinary object, as
    /// its "length" and "name" are once it has one (a constructor with
    /// static methods, a function with a "prototype", a class), and a
    /// property of the function is what a read of the name from that
    /// object finds by the cache: `Array.isArray`, `Object.keys`,
    /// `String.fromCharCode`, `Date.now`, `K.prototype`. A function with no
    /// such object has its "length" and its "name" (which go the general
    /// way) and what it inherits, found by the cache under the shape of
    /// the functions that have nothing of their own: `f.call`, `f.apply`,
    /// `f.bind`. Every such read took the general way of an instruction
    /// before.
    #[cfg(feature = "array-buffer")]
    fn exotic_property_value(
        agent: &mut Agent,
        value: Value,
        key: PropertyKey,
        cache: PropertyLookupCache,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        if let Ok(function) = Function::try_from(value) {
            if let Some(backing) = function.get_backing_object(agent) {
                return backing.value_by_cache(agent, cache, gc);
            }
            if key == PropertyKey::from(BUILTIN_STRING_MEMORY.length)
                || key == PropertyKey::from(BUILTIN_STRING_MEMORY.name)
            {
                return None;
            }
            let shape = Object::from(function).object_shape(agent);
            let (offset, holder) = cache.find_cached_property_offset(agent, shape)?;
            if offset.is_unset() {
                return Some(Value::Undefined);
            }
            // The object that has the property (%Function.prototype%, or
            // one above it) is asked the general way: the one function
            // that reads a slot of an ordinary object is part of the read
            // of every property by its cache while that is all that calls
            // it, and with a second caller here a loop that reads the
            // properties of an object was 5% slower.
            return match holder?.get_own_property_at_offset(agent, offset, gc) {
                TryGetResult::Value(value) => Some(value.unbind()),
                _ => None,
            };
        }
        if let Some(length) = Self::typed_array_length(agent, value, key, cache, gc) {
            return Some(length);
        }
        Self::inherited_property_value(agent, value, key, cache, gc)
    }

    /// A named property of an object whose kind gives it nothing by a name
    /// of its own: a Map, a Set, a Date, a Promise, an ArrayBuffer, a
    /// DataView, an iterator, a generator; a RegExp but for its "lastIndex",
    /// and a TypedArray but for the names that are the strings of Numbers,
    /// which are its elements. \[\[Get]] of any of these is OrdinaryGet: of
    /// the ordinary object that has its properties once it has any, and
    /// until then of nothing but what it inherits, which the cache places
    /// under the shape of the objects that have its prototype and nothing
    /// else. `map.get`, `set.has`, `regexp.test`, `bytes.set`, `date.getTime`
    /// and `promise.then` are found so; each took the general way of an
    /// instruction before, to the same place of the same cache.
    #[cfg(feature = "array-buffer")]
    #[inline(never)]
    fn inherited_property_value(
        agent: &mut Agent,
        value: Value,
        key: PropertyKey,
        cache: PropertyLookupCache,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        if !matches!(key, PropertyKey::String(_) | PropertyKey::SmallString(_)) {
            return None;
        }
        // The object, and the ordinary one that has its properties if it
        // has any: asked of its kind, which is known here.
        macro_rules! bare {
            ($object:ident) => {
                (Object::from($object), $object.get_backing_object(agent))
            };
        }
        let (object, backing) = match value {
            Value::Map(o) => bare!(o),
            Value::Promise(o) => bare!(o),
            Value::ArrayBuffer(o) => bare!(o),
            Value::DataView(o) => bare!(o),
            Value::ArrayIterator(o) => bare!(o),
            Value::Generator(o) => bare!(o),
            #[cfg(feature = "set")]
            Value::Set(o) => bare!(o),
            #[cfg(feature = "set")]
            Value::SetIterator(o) => bare!(o),
            #[cfg(feature = "set")]
            Value::MapIterator(o) => bare!(o),
            #[cfg(feature = "date")]
            Value::Date(o) => bare!(o),
            #[cfg(feature = "weak-refs")]
            Value::WeakMap(o) => bare!(o),
            #[cfg(feature = "weak-refs")]
            Value::WeakSet(o) => bare!(o),
            #[cfg(feature = "weak-refs")]
            Value::WeakRef(o) => bare!(o),
            #[cfg(feature = "regexp")]
            Value::RegExp(o) => {
                if key == PropertyKey::from(BUILTIN_STRING_MEMORY.lastIndex) {
                    return None;
                }
                bare!(o)
            }
            _ => {
                let array = AnyTypedArray::try_from(value).ok()?;
                // The string of a Number starts with a digit, a minus sign,
                // "Infinity" or "NaN", and a name that starts so may be an
                // element, which is never looked for in a prototype.
                let name = String::try_from(key.into_value_unchecked()).ok()?;
                if matches!(
                    name.as_wtf8_(agent).as_bytes().first(),
                    None | Some(b'0'..=b'9' | b'-' | b'I' | b'N')
                ) {
                    return None;
                }
                bare!(array)
            }
        };
        if let Some(backing) = backing {
            return backing.value_by_cache(agent, cache, gc);
        }
        let kind = core::mem::discriminant(&value.unbind());
        let seen = agent.heap.bare_shape_seen.get();
        let shape = match seen.and_then(|seen| seen.shape(agent, kind)) {
            Some(shape) => shape,
            None => {
                let shape = object.object_shape(agent);
                let seen = crate::ecmascript::BareShapeSeen::now(agent, kind, shape);
                agent.heap.bare_shape_seen.set(Some(seen));
                shape
            }
        };
        let (offset, holder) = cache.find_cached_property_offset(agent, shape)?;
        if offset.is_unset() {
            return Some(Value::Undefined);
        }
        match holder?.get_own_property_at_offset(agent, offset, gc) {
            TryGetResult::Value(value) => Some(value.unbind()),
            _ => None,
        }
    }

    /// `a.length` of a TypedArray, where the property cache finds for its
    /// shape the accessor of %TypedArray%.prototype: what the accessor
    /// returns, without the call.
    #[cfg(feature = "array-buffer")]
    fn typed_array_length(
        agent: &mut Agent,
        value: Value,
        key: PropertyKey,
        cache: PropertyLookupCache,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let array = AnyTypedArray::try_from(value).ok()?;
        if key != PropertyKey::from(BUILTIN_STRING_MEMORY.length) {
            return None;
        }
        // A TypedArray with no properties of its own and the prototype of
        // its kind: what `length` is for one such is what it is for all of
        // that kind in this Realm, until something that the heap counts
        // changes (see `TypedArrayLengthSeen`). Finding it out anew at
        // every read was the shape of the array asked for by its
        // prototype, the cache searched, and the accessor read from the
        // prototype's storage through its descriptors: 28% of a loop over
        // a Uint8Array that tests `i < bytes.length`.
        let kind = array
            .get_backing_object(agent)
            .is_none()
            .then(|| core::mem::discriminant(&array.unbind()));
        if let Some(kind) = kind
            && agent
                .typed_array_length_seen
                .get()
                .is_some_and(|seen| seen.holds(agent, kind))
        {
            return Some(array.length_value(agent));
        }
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
        if !TypedArrayPrototype::is_length_getter(agent, getter) {
            return None;
        }
        if let Some(kind) = kind {
            agent
                .typed_array_length_seen
                .set(Some(crate::ecmascript::TypedArrayLengthSeen::now(
                    agent, kind,
                )));
        }
        Some(array.length_value(agent))
    }

    /// UnaryMinus of a Number in the result register: Number::unaryMinus,
    /// as the negation of the double the Number is, made the Number that
    /// double is (an integer that is not zero stays an integer, zero is
    /// -0 and -0 is zero). Anything else, a BigInt among it, goes the
    /// general way.
    #[inline(never)]
    fn negate_number(&mut self, agent: &mut Agent, gc: NoGcScope) -> Plain {
        let Some(double) = self.result.and_then(|value| Self::double_of(agent, value)) else {
            return Plain::No;
        };
        let negated: Value = Number::from_f64(agent, -double, gc).into();
        self.result = Some(negated.unbind());
        Plain::Done
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

    /// What a comparison or an arithmetic operator gives for two operands
    /// that no conversion that could run a script comes into: in place for
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
        if !(left.is_number() && right.is_number()) {
            // A comparison of two Strings is answered; `kind` is known
            // where this is compiled, so the operators have no such call.
            if matches!(
                kind,
                Instruction::LessThan
                    | Instruction::LessThanEquals
                    | Instruction::GreaterThan
                    | Instruction::GreaterThanEquals
            ) {
                return Self::values_compare(agent, kind, left, right).map(|holds| holds.into());
            }
            // The `+` of a String goes on to the call below, which every
            // operator has for its Numbers: one more call here, in every
            // arm that has an operator, moved the loops that add integers.
        }
        Self::float_operation(agent, kind, left, right, gc)
    }

    /// `left + right` where one operand is a String and the other a String,
    /// or a primitive that ToString takes without running anything (a
    /// Number, a Boolean, undefined, null, a BigInt; not a Symbol, which it
    /// throws for): the two texts joined, as the instruction of the general
    /// way joins them, after the same look at the length of the result.
    /// None for other operands, and where that look refuses the text: the
    /// general way then finds the same and throws.
    ///
    /// Every `s += "…"` and `"<td>" + cell` took the general way before: the
    /// loop left, the instruction was read again with what it is fused
    /// with, and the operands were asked whether they are Numbers first. A
    /// sixth of a loop appending short texts.
    #[inline(never)]
    fn values_added(
        agent: &mut Agent,
        left: Value,
        right: Value,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let (left, right) = match (String::try_from(left), String::try_from(right)) {
            (Ok(left), Ok(right)) => (left.bind(gc), right.bind(gc)),
            _ => {
                let (left, right) = string_and_primitive(left, right)?;
                let left = to_string_primitive(agent, left.bind(gc), gc).ok()?;
                let right = to_string_primitive(agent, right.bind(gc), gc).ok()?;
                (left, right)
            }
        };
        agent.check_concat_length(left, right, gc).ok()?;
        Some(String::concat(agent, [left, right], gc).unbind().into())
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
        // One call for whatever is not two integers, which the callers
        // have seen to: nothing of it is in the loop of instructions.
        Self::values_compare(agent, comparison, left, right)
    }

    /// Whether `comparison` holds for two Numbers of any kind or for two
    /// Strings; None for other operands, which are converted first.
    #[inline(never)]
    fn values_compare(
        agent: &Agent,
        comparison: Instruction,
        left: Value,
        right: Value,
    ) -> Option<bool> {
        if left.is_number() && right.is_number() {
            return Self::doubles_compare(agent, comparison, left, right);
        }
        Self::strings_compare(agent, comparison, left, right)
    }

    /// `value instanceof target`, where that is OrdinaryHasInstance of a
    /// function and comes to an answer with nothing run: the target is a
    /// function (not a bound one) whose @@hasInstance method is the one of
    /// the running realm's %Function.prototype%, as
    /// [`instanceof_operator`] looks for it; its "prototype" is a data
    /// property that holds an object; and no Proxy is met on the way up
    /// from the value. None for anything else, with nothing changed: the
    /// general way throws what is to be thrown.
    #[inline(never)]
    fn ordinary_instance_of(
        agent: &mut Agent,
        value: Value,
        target: Value,
        gc: NoGcScope,
    ) -> Option<bool> {
        let target = Function::try_from(target).ok()?;
        if matches!(target, Function::BoundFunction(_)) {
            return None;
        }
        // 2. Let instOfHandler be ? GetMethod(target, @@hasInstance).
        let core::ops::ControlFlow::Continue(Some(Function::BuiltinFunction(handler))) =
            try_get_object_method_by_cache(
                agent,
                target.into(),
                WellKnownSymbols::HasInstance.into(),
                gc,
            )
        else {
            return None;
        };
        if !matches!(
            handler.get(agent).behaviour,
            Behaviour::Regular(found)
                if core::ptr::fn_addr_eq(found, FunctionPrototype::has_instance as RegularFn)
        ) || handler.get(agent).realm != agent.current_realm(gc)
        {
            return None;
        }
        // OrdinaryHasInstance ( C, O )
        // 3. If O is not an Object, return false.
        let Ok(mut object) = Object::try_from(value) else {
            return Some(false);
        };
        // 4. Let P be ? Get(C, "prototype").
        let key = BUILTIN_STRING_MEMORY.prototype.to_property_key();
        let cache = PropertyLookupCache::new(agent, key);
        let core::ops::ControlFlow::Continue(TryGetResult::Value(prototype)) =
            crate::ecmascript::try_get(agent, target, key, Some(cache), gc)
        else {
            return None;
        };
        // 5. If P is not an Object, throw a TypeError exception.
        let prototype = Object::try_from(prototype).ok()?;
        // 6. Repeat: an ordinary chain ends, and a Proxy is not asked here.
        loop {
            let core::ops::ControlFlow::Continue(next) = object.try_get_prototype_of(agent, gc)
            else {
                return None;
            };
            let Some(next) = next else {
                return Some(false);
            };
            if next == prototype {
                return Some(true);
            }
            object = next;
        }
    }

    /// Whether `comparison` (LessThan or one like it) holds for two Strings,
    /// which are compared by their code units with nothing converted. None
    /// for another instruction or other operands.
    fn strings_compare(
        agent: &Agent,
        comparison: Instruction,
        left: Value,
        right: Value,
    ) -> Option<bool> {
        if !matches!(
            comparison,
            Instruction::LessThan
                | Instruction::LessThanEquals
                | Instruction::GreaterThan
                | Instruction::GreaterThanEquals
        ) {
            return None;
        }
        let (Ok(left), Ok(right)) = (String::try_from(left), String::try_from(right)) else {
            return None;
        };
        let order = left.as_wtf8_(agent).cmp_utf16(right.as_wtf8_(agent));
        Some(match comparison {
            Instruction::LessThan => order.is_lt(),
            Instruction::LessThanEquals => order.is_le(),
            Instruction::GreaterThan => order.is_gt(),
            _ => order.is_ge(),
        })
    }

    #[inline]
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

    /// The comparisons and the operators on two Numbers of any kind, and
    /// what is done here for operands that are not two Numbers. A
    /// comparison is of two doubles, where one that is NaN compares as
    /// false; an operator is what the Number type defines for it. A result
    /// that is a double too wide for a Value is a Number of the heap, and
    /// the `+` of two Strings is a String of it: what a plain instruction
    /// allocates, and no collection is due before the loop looks for one
    /// again.
    ///
    /// The arithmetic of two doubles is the way straight through, since
    /// nearly every call is for it: each operand is read as the double it
    /// is, once, the operator is found by one jump, and the Number of the
    /// result is made here. Everything else is a function of its own.
    /// (Written as one `match` over the operator around the operands taken
    /// apart as Numbers, this ran 137 instructions for a product of two
    /// doubles: it asked what the operator is three times, and called for
    /// the result.)
    #[inline(never)]
    fn float_operation(
        agent: &mut Agent,
        kind: Instruction,
        left: Value,
        right: Value,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let (Some(left_double), Some(right_double)) =
            (Self::double_of(agent, left), Self::double_of(agent, right))
        else {
            return Self::operation_on_others(agent, kind, left, right, gc);
        };
        // The four that are the arithmetic of doubles (Number::add and the
        // others say so), on the doubles the Numbers are.
        let result = match kind {
            Instruction::ApplyAdditionBinaryOperator => left_double + right_double,
            Instruction::ApplySubtractionBinaryOperator => left_double - right_double,
            Instruction::ApplyMultiplicationBinaryOperator => left_double * right_double,
            Instruction::ApplyDivisionBinaryOperator => left_double / right_double,
            Instruction::LessThan => return Some((left_double < right_double).into()),
            Instruction::LessThanEquals => return Some((left_double <= right_double).into()),
            Instruction::GreaterThan => return Some((left_double > right_double).into()),
            Instruction::GreaterThanEquals => return Some((left_double >= right_double).into()),
            _ => return Self::operation_on_numbers(agent, kind, left, right, gc),
        };
        // The Number of the result, made here where that calls nothing and
        // can fail nowhere, as [`Number::from_f64`] makes it and for its
        // reason: this function keeps no frame. Where the heap's vector of
        // Numbers has to grow for it, the Number is made by a function
        // called last.
        if Number::is_heap_f64(result) {
            if let Some(number) = Number::alloc_in_place(&mut agent.heap, result) {
                return Some(Value::Number(number.unbind()));
            }
        } else if Number::is_whole_and_safe(result) {
            if let Ok(integer) = SmallInteger::try_from(result as i64) {
                return Some(Value::Integer(integer));
            }
        } else if let Ok(small) = SmallF64::try_from(result) {
            return Some(Value::SmallF64(small));
        }
        Self::float_result_on_the_heap(agent, result, gc)
    }

    /// The result of [`Vm::float_operation`] where the heap's vector of
    /// Numbers grows for it.
    #[cold]
    #[inline(never)]
    fn float_result_on_the_heap(
        agent: &mut Agent,
        result: f64,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        let value: Value = Number::from_f64_on_the_heap(agent, result, gc).into();
        Some(value.unbind())
    }

    /// The double that a Number of any kind is; None for what is no Number.
    #[inline(always)]
    fn double_of(agent: &Agent, value: Value) -> Option<f64> {
        match value {
            // None for a handle that names no Number too: nothing here
            // has a message to give, and whoever takes the operands the
            // general way says so.
            Value::Number(number) => Number::heap_f64(agent, number),
            Value::Integer(number) => Some(number.into_i64() as f64),
            Value::SmallF64(number) => Some(number.into_f64()),
            _ => None,
        }
    }

    /// What [`Vm::float_operation`] gives for operands that are not two
    /// Numbers: a comparison of two Strings is answered, and the `+` of a
    /// String and a String or another primitive.
    #[inline(never)]
    fn operation_on_others(
        agent: &mut Agent,
        kind: Instruction,
        left: Value,
        right: Value,
        gc: NoGcScope,
    ) -> Option<Value<'static>> {
        match kind {
            Instruction::LessThan
            | Instruction::LessThanEquals
            | Instruction::GreaterThan
            | Instruction::GreaterThanEquals => {
                Self::values_compare(agent, kind, left, right).map(|holds| holds.into())
            }
            Instruction::ApplyAdditionBinaryOperator => Self::values_added(agent, left, right, gc),
            _ => None,
        }
    }

    /// What [`Vm::float_operation`] gives for two Numbers and an operator
    /// that is not the arithmetic of doubles or a comparison: the
    /// remainder, the shifts and the bitwise operators, as the Number type
    /// defines them.
    #[inline(never)]
    fn operation_on_numbers(
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
        match kind {
            Instruction::ApplyRemainderBinaryOperator
            | Instruction::ApplyShiftLeftBinaryOperator
            | Instruction::ApplyShiftRightBinaryOperator
            | Instruction::ApplyShiftRightZeroFillBinaryOperator
            | Instruction::ApplyBitwiseORBinaryOperator
            | Instruction::ApplyBitwiseXORBinaryOperator
            | Instruction::ApplyBitwiseAndBinaryOperator => {
                let value: Value = number_binary_operator(agent, kind, left, right, gc)
                    .ok()?
                    .into();
                Some(value.unbind())
            }
            _ => None,
        }
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
            // The second is the first where its stack slot keeps nothing
            // and the name is not where it is remembered to be: the slot is
            // filled by the next read that finds it there.
            Instruction::GetBindingAtSite | Instruction::GetBindingAtSiteOnce => {
                execute_get_binding_at_site(agent, vm, executable, instr, false, gc)?;
            }
            Instruction::PutBindingAtSite => {
                execute_put_binding_at_site(agent, vm, executable, instr, false, gc)?;
            }
            Instruction::PutBindingAtSiteAndGet => {
                execute_fused_instruction(agent, vm, executable, instr, gc)?;
            }
            Instruction::ResolveGlobalAtSite
            | Instruction::CheckGlobalAtSite
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
            | Instruction::JumpIfResultComparesWithConstant
            | Instruction::StepValueWithCache
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
            | Instruction::LoadBindingAtSiteOnce
            | Instruction::ApplyBinaryOperatorWithConstant
            | Instruction::ApplyBinaryOperatorToIndexWithConstant
            | Instruction::ApplyBinaryOperatorToIndexWithResult
            | Instruction::ApplyBinaryOperatorAndMove
            | Instruction::LoadPropertyWithCachePushReference
            | Instruction::LoadPropertyWithCachePushThis
            | Instruction::LoadIndexPropertyWithCachePushThis => {
                execute_fused_instruction(agent, vm, executable, instr, gc)?;
            }
            Instruction::PutPropertyWithCache
            | Instruction::PutIndexPropertyWithCache
            | Instruction::GetThisPropertyWithCache
            | Instruction::GetThisPropertyWithCacheKeepReference
            | Instruction::GetIndexPropertyWithCache
            | Instruction::GetIndexPropertyWithCacheKeepReference
            | Instruction::GetKeptPropertyWithCache
            | Instruction::LoadIndexPropertyWithCache => {
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
            | Instruction::GetBindingAtSiteOnce
            | Instruction::LoadBindingAtSiteOnce
            | Instruction::PutBindingAtSite
            | Instruction::PutBindingAtSiteAndGet
            | Instruction::ResolveGlobalAtSite
            | Instruction::CheckGlobalAtSite
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
            | Instruction::GetKeptPropertyWithCache
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
            | Instruction::JumpIfResultComparesWithConstant
            | Instruction::StepValueWithCache
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
            | Instruction::ApplyBinaryOperatorToIndexWithResult
            | Instruction::ApplyBinaryOperatorAndMove
            | Instruction::LoadPropertyWithCachePushReference
            | Instruction::LoadPropertyWithCachePushThis
            | Instruction::LoadIndexPropertyWithCachePushThis
            | Instruction::LoadIndexPropertyWithCache
            | Instruction::PutIndexPropertyWithCache
            | Instruction::BindArguments => {
                unreachable!("hot instruction not handled before execute_cold_instruction")
            }
            Instruction::Return => return Ok(ContinuationKind::Return),
            Instruction::Await => {
                execute_await_promise_resolve(agent, vm, gc)?;
                return Ok(ContinuationKind::Await);
            }
            Instruction::Yield => return Ok(ContinuationKind::Yield),
            Instruction::YieldValue => return Ok(ContinuationKind::YieldValue),
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
            Instruction::ArrayFromStack => {
                execute_array_from_stack(agent, vm, instr, gc.into_nogc())?
            }
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
            Instruction::EvaluateCallOnThis => execute_evaluate_call_on_this(agent, vm, instr, gc)?,
            Instruction::EvaluateNew => execute_evaluate_new(agent, vm, instr, gc)?,
            Instruction::EvaluateSuper => return Ok(ContinuationKind::Super),
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
            #[cfg(feature = "regexp")]
            Instruction::InstantiateRegExpLiteral => {
                let Some(Value::RegExp(literal)) = vm.result else {
                    unreachable!("a RegExp literal's instruction without its RegExp")
                };
                let gc = gc.into_nogc();
                let regexp = crate::ecmascript::reg_exp_instantiate_literal(agent, literal, gc);
                vm.result = Some(regexp.unbind().into());
            }
            #[cfg(not(feature = "regexp"))]
            Instruction::InstantiateRegExpLiteral => unreachable!(),
            Instruction::ConcatStrings => execute_concat_strings(agent, vm, instr, gc.into_nogc())?,
            Instruction::ToStringOperand => execute_to_string_part(agent, vm, false, gc)?,
            Instruction::ToStringSubstitution => execute_to_string_part(agent, vm, true, gc)?,
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
            // A byte that is no instruction: see [`Instruction::is_unused`].
            super::instructions::unused_kinds!() => {
                super::instructions::panic_invalid_instruction()
            }
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
    let inst_of_handler = if let Some(handler) =
        try_result_into_option_js(try_get_object_method_by_cache(
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
        // The method that every function inherits,
        // Function.prototype[%Symbol.hasInstance%], is OrdinaryHasInstance
        // of its this value, and gives a Boolean: taken here without the
        // call of a built-in function around it. Only for the method of
        // the running realm: the call of another realm's makes that realm
        // the running one, which what OrdinaryHasInstance reads of an
        // object without properties of its own depends on.
        if let Function::BuiltinFunction(handler) = inst_of_handler
            && matches!(
                handler.get(agent).behaviour,
                Behaviour::Regular(found)
                    if core::ptr::fn_addr_eq(found, FunctionPrototype::has_instance as RegularFn)
            )
            && handler.get(agent).realm == agent.current_realm(gc.nogc())
        {
            return ordinary_has_instance(agent, target.unbind(), value.unbind(), gc);
        }
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
            handler_base: _,
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
            handler_base: _,
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
