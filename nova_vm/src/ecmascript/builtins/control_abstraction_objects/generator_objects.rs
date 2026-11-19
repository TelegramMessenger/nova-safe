// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{
        Agent, ExceptionType, ExecutionContext, InternalMethods, InternalSlots, JsError, JsResult,
        OrdinaryObject, ProtoIntrinsics, Value, create_iter_result_object,
        create_iter_result_object_in_realm, object_handle,
    },
    engine::{Bindable, Executable, ExecutionResult, GcScope, Scopable, SuspendedVm},
    heap::{
        ArenaAccess, ArenaAccessMut, BaseIndex, CompactionLists, CreateHeapData, Heap,
        HeapMarkAndSweep, HeapSweepWeakReference, WorkQueues, arena_vec_access,
    },
};

/// ## [27.5 Generator Objects](https://tc39.es/ecma262/#sec-generator-objects)
///
/// A Generator is created by calling a generator function and conforms to both
/// the iterator interface and the iterable interface.
///
/// Generator instances directly inherit properties from the initial value of
/// the **"prototype"** property of the generator function that created the
/// instance. Generator instances indirectly inherit properties from
/// %GeneratorPrototype%.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Generator<'a>(BaseIndex<'a, GeneratorHeapData<'static>>);
object_handle!(Generator);
arena_vec_access!(Generator, 'a, GeneratorHeapData, generators);

/// What a generator that was resumed came to, for a caller that steps it
/// ([`Generator::step`]).
#[derive(Debug, Bindable)]
pub(crate) enum GeneratorStep<'a> {
    /// `yield value`: the value, with no iterator result object made for it.
    Yielded(Value<'a>),
    /// An iterator result object that is passed on as it is: the one of the
    /// iterator that `yield*` delegates to.
    Result(Value<'a>),
    /// The generator is completed.
    Done,
}

impl Generator<'_> {
    /// ### [27.5.3.3 GeneratorResume ( generator, value, generatorBrand )](https://tc39.es/ecma262/#sec-generatorresume)
    ///
    /// What %GeneratorPrototype%.next returns: an iterator result object.
    pub(crate) fn resume<'a>(
        self,
        agent: &mut Agent,
        value: Value,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, Value<'a>> {
        match self.resume_as(agent, value, true, gc)? {
            GeneratorStep::Yielded(result) | GeneratorStep::Result(result) => Ok(result),
            // Not reached: a result object was asked for.
            GeneratorStep::Done => Ok(Value::Undefined),
        }
    }

    /// GeneratorResume with undefined for a caller that reads the result
    /// itself, as IteratorStepValue does where the generator's `next` is the
    /// original one: the value of `yield` comes as it is, and no iterator
    /// result object is made for the caller to take apart again.
    pub(crate) fn step<'a>(
        self,
        agent: &mut Agent,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, GeneratorStep<'a>> {
        self.resume_as(agent, Value::Undefined, false, gc)
    }

    /// GeneratorResume. With `wrapped`, what comes back is an iterator
    /// result object ([`GeneratorStep::Result`]) as the specification has
    /// it; without, the value of a `yield` and the completion come without
    /// one.
    fn resume_as<'a>(
        self,
        agent: &mut Agent,
        value: Value,
        wrapped: bool,
        mut gc: GcScope<'a, '_>,
    ) -> JsResult<'a, GeneratorStep<'a>> {
        let value = value.bind(gc.nogc());
        let generator = self.bind(gc.nogc());
        // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
        match generator.get(agent).generator_state.as_ref().unwrap() {
            GeneratorState::Executing => {
                return Err(agent.throw_exception_with_static_message(
                    ExceptionType::TypeError,
                    "The generator is currently running",
                    gc.into_nogc(),
                ));
            }
            GeneratorState::Completed => {
                // 2. If state is completed, return CreateIterResultObject(undefined, true).
                if !wrapped {
                    return Ok(GeneratorStep::Done);
                }
                return create_iter_result_object(agent, Value::Undefined, true, gc.into_nogc())
                    .map(|o| GeneratorStep::Result(o.into()));
            }
            GeneratorState::SuspendedStart(_) | GeneratorState::SuspendedYield(_) => {
                // 3. Assert: state is either suspended-start or suspended-yield.
            }
        };

        // 7. Set generator.[[GeneratorState]] to executing.
        let mut state = match generator
            .get_mut(agent)
            .generator_state
            .replace(GeneratorState::Executing)
        {
            Some(GeneratorState::SuspendedYield(state))
            | Some(GeneratorState::SuspendedStart(state)) => state,
            _ => unreachable!(),
        };
        // The record of the suspended state waits here, with its parts taken
        // out, for the next `yield` to fill again.
        let (vm, executable, execution_context) = state.take_parts();
        let executable = executable.scope(agent, gc.nogc());

        // 4. Let genContext be generator.[[GeneratorContext]].
        // 5. Let methodContext be the running execution context.
        // 6. Suspend methodContext.
        // 8. Push genContext onto the execution context stack; genContext is now the running
        // execution context.
        agent.push_execution_context(execution_context);

        let saved = generator.scope(agent, gc.nogc());

        // 9. Resume the suspended evaluation of genContext using NormalCompletion(value) as the
        // result of the operation that suspended it. Let result be the value returned by the
        // resumed computation.
        let execution_result = vm.resume(agent, executable.clone(), value.unbind(), gc.reborrow());

        let execution_result = execution_result.unbind();
        let gc = gc.into_nogc();
        let generator = saved.get(agent).bind(gc);
        let execution_result = execution_result.bind(gc);

        // GeneratorStart: 4.f. Remove acGenContext from the execution context stack and restore the
        // execution context that is at the top of the execution context stack as the running
        // execution context.
        // GeneratorYield 6 is the same.
        let execution_context = agent.pop_execution_context().unwrap();

        // 10. Assert: When we return here, genContext has already been removed
        // from the execution context stack and methodContext is the currently
        // running execution context.
        // 11. Return ? result.
        match execution_result {
            ExecutionResult::Return(result_value) => {
                // GeneratorStart step 4:
                // g. Set acGenerator.[[GeneratorState]] to completed.
                // h. NOTE: Once a generator enters the completed state it never leaves it and its
                // associated execution context is never resumed. Any execution state associated
                // with acGenerator can be discarded at this point.
                generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
                // i. If result is a normal completion, then
                //    i. Let resultValue be undefined.
                // j. Else if result is a return completion, then
                //    i. Let resultValue be result.[[Value]].
                // l. Return CreateIterResultObject(resultValue, true).
                if !wrapped {
                    return Ok(GeneratorStep::Done);
                }
                create_iter_result_object(agent, result_value, true, gc.into_nogc())
                    .map(|o| GeneratorStep::Result(o.into()))
            }
            ExecutionResult::Throw(err) => {
                // GeneratorStart step 4:
                // g. Set acGenerator.[[GeneratorState]] to completed.
                // h. NOTE: Once a generator enters the completed state it never leaves it and its
                // associated execution context is never resumed. Any execution state associated
                // with acGenerator can be discarded at this point.
                generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
                // k. i. Assert: result is a throw completion.
                //    ii. Return ? result.
                Err(err.unbind())
            }
            ExecutionResult::Yield {
                vm,
                yielded_value,
                plain,
            } => {
                // Yield:
                // 3. Otherwise, return ? GeneratorYield(CreateIterResultObject(value, false)).
                // GeneratorYield:
                // 3. Let generator be the value of the Generator component of genContext.
                // 5. Set generator.[[GeneratorState]] to suspended-yield.
                let realm = execution_context.realm;
                state.put_parts(vm, executable.get(agent), execution_context);
                generator.get_mut(agent).generator_state =
                    Some(GeneratorState::SuspendedYield(state));
                // 8. Resume callerContext passing NormalCompletion(iterNextObj). ...
                // NOTE: `callerContext` here is the `GeneratorResume` execution context.
                if !plain {
                    Ok(GeneratorStep::Result(yielded_value))
                } else if wrapped {
                    // The result object that `yield value` asks for, made
                    // here for the generator's realm as if its code had.
                    create_iter_result_object_in_realm(agent, realm, yielded_value, false, gc)
                        .map(|o| GeneratorStep::Result(o.into()))
                } else {
                    Ok(GeneratorStep::Yielded(yielded_value))
                }
            }
            ExecutionResult::Await { .. } => unreachable!(),
        }
    }

    /// ### [27.5.3.4 GeneratorResumeAbrupt ( generator, abruptCompletion, generatorBrand )](https://tc39.es/ecma262/#sec-generatorresumeabrupt)
    /// NOTE: This method only accepts throw completions.
    pub(crate) fn resume_throw<'a>(
        self,
        agent: &mut Agent,
        value: Value,
        mut gc: GcScope<'a, '_>,
    ) -> JsResult<'a, Value<'a>> {
        let value = value.bind(gc.nogc());
        let generator = self.bind(gc.nogc());
        // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
        match generator.get(agent).generator_state.as_ref().unwrap() {
            GeneratorState::SuspendedStart(_) => {
                // 2. If state is suspended-start, then
                // a. Set generator.[[GeneratorState]] to completed.
                // b. NOTE: Once a generator enters the completed state it never leaves it and its
                // associated execution context is never resumed. Any execution state associated
                // with generator can be discarded at this point.
                generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
                // c. Set state to completed.

                // 3. If state is completed, then
                // b. Return ? abruptCompletion.
                return Err(JsError::new(value.unbind()));
            }
            GeneratorState::SuspendedYield(_) => {
                // 4. Assert: state is suspended-yield.
            }
            GeneratorState::Executing => {
                return Err(agent.throw_exception_with_static_message(
                    ExceptionType::TypeError,
                    "The generator is currently running",
                    gc.into_nogc(),
                ));
            }
            GeneratorState::Completed => {
                // 3. If state is completed, then
                //    b. Return ? abruptCompletion.
                return Err(JsError::new(value.unbind()));
            }
        };

        // 8. Set generator.[[GeneratorState]] to executing.
        let Some(GeneratorState::SuspendedYield(mut state)) = generator
            .get_mut(agent)
            .generator_state
            .replace(GeneratorState::Executing)
        else {
            unreachable!()
        };
        let (vm, executable, execution_context) = state.take_parts();
        let generator = generator.scope(agent, gc.nogc());
        let executable = executable.scope(agent, gc.nogc());

        // 5. Let genContext be generator.[[GeneratorContext]].
        // 6. Let methodContext be the running execution context.
        // 7. Suspend methodContext.
        // 9. Push genContext onto the execution context stack; genContext is now the running
        // execution context.
        agent.push_execution_context(execution_context);

        // 10. Resume the suspended evaluation of genContext using NormalCompletion(value) as the
        // result of the operation that suspended it. Let result be the value returned by the
        // resumed computation.
        let execution_result = vm
            .resume_throw(agent, executable.clone(), value.unbind(), gc.reborrow())
            .unbind();
        let gc = gc.into_nogc();
        let execution_result = execution_result.bind(gc);
        let executable = executable.take(agent).bind(gc);
        let generator = generator.take(agent).bind(gc);

        // GeneratorStart: 4.f. Remove acGenContext from the execution context stack and restore the
        // execution context that is at the top of the execution context stack as the running
        // execution context.
        // GeneratorYield 6 is the same.
        let execution_context = agent.pop_execution_context().unwrap();

        // 11. Assert: When we return here, genContext has already been removed
        // from the execution context stack and methodContext is the currently
        // running execution context.
        // 12. Return ? result.
        match execution_result {
            ExecutionResult::Return(result) => {
                generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
                create_iter_result_object(agent, result.unbind(), true, gc.into_nogc())
                    .map(|o| o.into())
            }
            ExecutionResult::Throw(err) => {
                generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
                Err(err)
            }
            ExecutionResult::Yield {
                vm,
                yielded_value,
                plain,
            } => {
                let realm = execution_context.realm;
                state.put_parts(vm, executable.unbind(), execution_context);
                generator.get_mut(agent).generator_state =
                    Some(GeneratorState::SuspendedYield(state));
                if plain {
                    create_iter_result_object_in_realm(agent, realm, yielded_value, false, gc)
                        .map(|o| o.into())
                } else {
                    Ok(yielded_value.unbind())
                }
            }
            ExecutionResult::Await { .. } => unreachable!(),
        }
    }

    /// ### [27.5.3.4 GeneratorResumeAbrupt ( generator, abruptCompletion, generatorBrand )](https://tc39.es/ecma262/#sec-generatorresumeabrupt)
    /// NOTE: This method only accepts return completions.
    pub(crate) fn resume_return<'a>(
        self,
        agent: &mut Agent,
        abrupt_completion: Value,
        mut gc: GcScope<'a, '_>,
    ) -> JsResult<'a, Value<'a>> {
        let abrupt_completion = abrupt_completion.bind(gc.nogc());
        let generator = self.bind(gc.nogc());
        // 1. Let state be ? GeneratorValidate(generator, generatorBrand).
        match generator.get(agent).generator_state.as_ref().unwrap() {
            GeneratorState::SuspendedStart(_) => {
                // 2. If state is suspended-start, then
                // a. Set generator.[[GeneratorState]] to completed.
                // b. NOTE: Once a generator enters the completed state it never leaves it and its
                // associated execution context is never resumed. Any execution state associated
                // with generator can be discarded at this point.
                generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
                // c. Set state to completed.

                // 3. If abruptCompletion is a return completion, then
                // i. Return CreateIteratorResultObject(abruptCompletion.[[Value]], true).
                return create_iter_result_object(
                    agent,
                    abrupt_completion.unbind(),
                    true,
                    gc.into_nogc(),
                )
                .map(|o| o.into());
            }
            GeneratorState::SuspendedYield(_) => {
                // 4. Assert: state is suspended-yield.
            }
            GeneratorState::Executing => {
                return Err(agent.throw_exception_with_static_message(
                    ExceptionType::TypeError,
                    "The generator is currently running",
                    gc.into_nogc(),
                ));
            }
            GeneratorState::Completed => {
                // 3. If abruptCompletion is a return completion, then
                // i. Return CreateIteratorResultObject(abruptCompletion.[[Value]], true).
                return create_iter_result_object(
                    agent,
                    abrupt_completion.unbind(),
                    true,
                    gc.into_nogc(),
                )
                .map(|o| o.into());
            }
        };

        // 4. Assert: state is suspended-yield.
        // 8. Set generator.[[GeneratorState]] to executing.
        let Some(GeneratorState::SuspendedYield(mut state)) = generator
            .get_mut(agent)
            .generator_state
            .replace(GeneratorState::Executing)
        else {
            unreachable!()
        };
        let (vm, executable, execution_context) = state.take_parts();
        let generator = generator.scope(agent, gc.nogc());
        let executable = executable.scope(agent, gc.nogc());

        // 5. Let genContext be generator.[[GeneratorContext]].
        // 6. Let methodContext be the running execution context.
        // 7. Suspend methodContext.
        // 9. Push genContext onto the execution context stack; genContext is now the running
        // execution context.
        agent.push_execution_context(execution_context);

        // 10. Resume the suspended evaluation of genContext using
        //     abruptCompletion as the result of the operation that suspended
        //     it. Let result be the Completion Record returned by the resumed
        //     computation.
        let execution_result = vm
            .resume_return(
                agent,
                executable.clone(),
                abrupt_completion.unbind(),
                gc.reborrow(),
            )
            .unbind();
        let gc = gc.into_nogc();
        let execution_result = execution_result.bind(gc);
        let executable = executable.take(agent).bind(gc);
        let generator = generator.take(agent).bind(gc);

        // GeneratorStart: 4.f. Remove acGenContext from the execution context stack and restore the
        // execution context that is at the top of the execution context stack as the running
        // execution context.
        // GeneratorYield 6 is the same.
        let execution_context = agent.pop_execution_context().unwrap();

        // 11. Assert: When we return here, genContext has already been removed
        // from the execution context stack and methodContext is the currently
        // running execution context.
        // 12. Return ? result.
        match execution_result {
            ExecutionResult::Return(result) => {
                generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
                create_iter_result_object(agent, result, true, gc.into_nogc()).map(|o| o.into())
            }
            ExecutionResult::Throw(err) => {
                generator.get_mut(agent).generator_state = Some(GeneratorState::Completed);
                Err(err)
            }
            ExecutionResult::Yield {
                vm,
                yielded_value,
                plain,
            } => {
                let realm = execution_context.realm;
                state.put_parts(vm, executable.unbind(), execution_context);
                generator.get_mut(agent).generator_state =
                    Some(GeneratorState::SuspendedYield(state));
                if plain {
                    create_iter_result_object_in_realm(agent, realm, yielded_value, false, gc)
                        .map(|o| o.into())
                } else {
                    Ok(yielded_value)
                }
            }
            ExecutionResult::Await { .. } => unreachable!(),
        }
    }
}

impl<'a> InternalSlots<'a> for Generator<'a> {
    const DEFAULT_PROTOTYPE: ProtoIntrinsics = ProtoIntrinsics::Generator;

    #[inline(always)]
    fn get_backing_object(self, agent: &Agent) -> Option<OrdinaryObject<'static>> {
        self.get(agent).object_index.unbind()
    }

    fn set_backing_object(self, agent: &mut Agent, backing_object: OrdinaryObject<'static>) {
        assert!(
            self.get_mut(agent)
                .object_index
                .replace(backing_object)
                .is_none()
        );
    }
}

impl<'a> InternalMethods<'a> for Generator<'a> {}

impl<'a> CreateHeapData<GeneratorHeapData<'a>, Generator<'a>> for Heap {
    fn create(&mut self, data: GeneratorHeapData<'a>) -> Generator<'a> {
        self.generators.push(data.unbind());
        // With the record of its suspended state, which every new generator
        // has.
        self.alloc_counter += core::mem::size_of::<GeneratorHeapData<'static>>()
            + core::mem::size_of::<SuspendedGeneratorState>();
        Generator(BaseIndex::last(&self.generators))
    }
}

impl HeapMarkAndSweep for Generator<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.generators.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.generators.shift_index(&mut self.0)
    }
}

impl HeapSweepWeakReference for Generator<'static> {
    fn sweep_weak_reference(self, compactions: &CompactionLists) -> Option<Self> {
        compactions.generators.shift_weak_index(self.0).map(Self)
    }
}

#[derive(Debug, Default, Bindable)]
pub(crate) struct GeneratorHeapData<'a> {
    pub(crate) object_index: Option<OrdinaryObject<'a>>,
    pub(crate) generator_state: Option<GeneratorState>,
}

#[derive(Debug)]
pub(crate) struct SuspendedGeneratorState {
    pub(crate) vm: SuspendedVm,
    pub(crate) executable: Executable<'static>,
    /// The generator's execution context. None only in a record that what
    /// runs the generator holds aside meanwhile, to fill it again at the
    /// next `yield`: never in the state of a generator.
    pub(crate) execution_context: Option<ExecutionContext>,
}

impl SuspendedGeneratorState {
    /// What a generator that is resumed goes on with, taken out of the
    /// record, which is left to be filled again by
    /// [`SuspendedGeneratorState::put_parts`].
    pub(crate) fn take_parts(&mut self) -> (SuspendedVm, Executable<'static>, ExecutionContext) {
        let execution_context = self
            .execution_context
            .take()
            .expect("a suspended generator has its execution context");
        (self.vm.take(), self.executable, execution_context)
    }

    /// The record as it is in the state of a generator that yielded.
    pub(crate) fn put_parts(
        &mut self,
        vm: SuspendedVm,
        executable: Executable<'static>,
        execution_context: ExecutionContext,
    ) {
        self.vm = vm;
        self.executable = executable;
        self.execution_context = Some(execution_context);
    }
}

/// The suspended states are records of their own: resuming a generator and
/// suspending it again exchanges the one record, and what is in it stays
/// where it is.
#[derive(Debug)]
pub(crate) enum GeneratorState {
    SuspendedStart(Box<SuspendedGeneratorState>),
    SuspendedYield(Box<SuspendedGeneratorState>),
    Executing,
    Completed,
}

impl HeapMarkAndSweep for SuspendedGeneratorState {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            vm,
            executable,
            execution_context,
        } = self;
        vm.mark_values(queues);
        executable.mark_values(queues);
        execution_context.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            vm,
            executable,
            execution_context,
        } = self;
        vm.sweep_values(compactions);
        executable.sweep_values(compactions);
        execution_context.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for GeneratorHeapData<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            object_index,
            generator_state,
        } = self;
        object_index.mark_values(queues);
        generator_state.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            object_index,
            generator_state,
        } = self;
        object_index.sweep_values(compactions);
        generator_state.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for GeneratorState {
    fn mark_values(&self, queues: &mut WorkQueues) {
        match self {
            GeneratorState::SuspendedStart(s) | GeneratorState::SuspendedYield(s) => {
                s.mark_values(queues)
            }
            GeneratorState::Executing | GeneratorState::Completed => {}
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        match self {
            GeneratorState::SuspendedStart(s) | GeneratorState::SuspendedYield(s) => {
                s.sweep_values(compactions)
            }
            GeneratorState::Executing | GeneratorState::Completed => {}
        }
    }
}
