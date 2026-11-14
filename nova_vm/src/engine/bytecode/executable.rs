// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{
        Agent, CompileFunctionBodyData, FunctionTemplate, ObjectShape, PropertyKey,
        PropertyLookupCache, SourceCode, String, Value,
    },
    engine::{
        Bindable, NoGcScope, Scoped,
        bytecode::{CompileContext, NamedEvaluationParameter, instructions::Instr},
    },
    heap::{
        ArenaAccess, CompactionLists, CreateHeapData, Heap, HeapMarkAndSweep, WorkQueues,
        arena_vec_access, {BaseIndex, HeapIndexHandle, index_handle},
    },
};
use core::marker::PhantomData;
use oxc_ast::ast;
use oxc_semantic::{AstNodes, Scoping};
use std::sync::Arc;

use super::bytecode_compiler::GeneratorKind;

/// A function defined by an Executable's code: what the instructions that
/// create its closures need.
#[derive(Debug, Clone, Bindable)]
pub(crate) struct FunctionExpression<'a> {
    pub(crate) template: FunctionTemplate<'a>,
    pub(crate) identifier: Option<NamedEvaluationParameter>,
    /// The StringValue of the function's BindingIdentifier.
    pub(crate) binding_name: Option<String<'a>>,
    /// The function is a FunctionExpression (not a declaration or a method).
    pub(crate) is_expression: bool,
    /// The function is a FunctionDeclaration.
    pub(crate) is_declaration: bool,
}

impl HeapMarkAndSweep for FunctionExpression<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            template,
            identifier: _,
            binding_name,
            is_expression: _,
            is_declaration: _,
        } = self;
        template.mark_values(queues);
        binding_name.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            template,
            identifier: _,
            binding_name,
            is_expression: _,
            is_declaration: _,
        } = self;
        template.sweep_values(compactions);
        binding_name.sweep_values(compactions);
    }
}

#[derive(Debug, Clone, Bindable)]
pub(crate) struct ArrowFunctionExpression<'a> {
    pub(crate) template: FunctionTemplate<'a>,
    pub(crate) identifier: Option<NamedEvaluationParameter>,
}

impl HeapMarkAndSweep for ArrowFunctionExpression<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            template,
            identifier: _,
        } = self;
        template.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            template,
            identifier: _,
        } = self;
        template.sweep_values(compactions);
    }
}

/// Reference to a heap-allocated executable VM bytecode.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub(crate) struct Executable<'a>(BaseIndex<'a, ExecutableHeapData<'static>>);
index_handle!(Executable);
arena_vec_access!(Executable, 'a, ExecutableHeapData, executables);

impl core::fmt::Debug for Executable<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Executable({:?})", self.get_index())
    }
}

#[expect(dead_code)]
const EXECUTABLE_OPTION_SIZE_IS_U32: () =
    assert!(size_of::<Executable<'_>>() == size_of::<Option<Executable<'_>>>());

/// ## Notes
///
/// - This is inspired by and/or copied from Kiesel engine:
///   Copyright (c) 2023-2024 Linus Groh
#[derive(Debug, Clone, Bindable)]
pub(crate) struct ExecutableHeapData<'a> {
    pub(crate) instructions: Arc<[u8]>,
    pub(crate) caches: Box<[PropertyLookupCache<'static>]>,
    pub(crate) constants: Box<[Value<'static>]>,
    pub(crate) shapes: Box<[ObjectShape<'static>]>,
    pub(crate) function_expressions: Box<[FunctionExpression<'static>]>,
    pub(crate) arrow_function_expressions: Box<[ArrowFunctionExpression<'static>]>,
    pub(crate) class_initializer_bytecodes: Box<[(Option<Executable<'static>>, bool)]>,
    /// The instance field and private method initializers of a derived class
    /// constructor: SuperCall runs them on the new `this` value.
    pub(crate) instance_initializer: Option<Executable<'static>>,
    pub(crate) _gc: PhantomData<Value<'a>>,
}

impl<'gc> Executable<'gc> {
    pub(crate) fn compile_script<'s>(
        agent: &mut Agent,
        body: &'s [ast::Statement<'s>],
        directives: &'s [ast::Directive<'s>],
        source_code: SourceCode<'gc>,
        scoping: &'s Scoping,
        nodes: &'s AstNodes<'s>,
        is_strict: bool,
        gc: NoGcScope<'gc, '_>,
    ) -> Self {
        if agent.options.print_internals {
            eprintln!();
            eprintln!("=== Compiling Script ===");
            eprintln!();
        }
        let mut ctx = CompileContext::new(agent, source_code, scoping, nodes, gc);
        ctx.set_runtime_strict(is_strict);

        ctx.compile_directive_prologue(directives);
        ctx.compile_statements(body);
        ctx.do_implicit_return();
        ctx.finish()
    }

    pub(crate) fn compile_module<'s>(
        agent: &mut Agent,
        body: &'s [ast::Statement<'s>],
        source_code: SourceCode<'gc>,
        scoping: &'s Scoping,
        nodes: &'s AstNodes<'s>,
        gc: NoGcScope<'gc, '_>,
    ) -> Self {
        if agent.options.print_internals {
            eprintln!();
            eprintln!("=== Compiling Module ===");
            eprintln!();
        }
        let mut ctx = CompileContext::new(agent, source_code, scoping, nodes, gc);
        ctx.set_runtime_strict(true);

        ctx.compile_statements(body);
        ctx.do_implicit_return();
        ctx.finish()
    }

    pub(crate) fn compile_function_body(
        agent: &mut Agent,
        data: CompileFunctionBodyData<'_, 'gc>,
        gc: NoGcScope<'gc, '_>,
    ) -> Self {
        let mut ctx = CompileContext::new(agent, data.source_code, data.scoping, data.nodes, gc);
        ctx.set_runtime_strict(data.is_strict);
        if data.ast.is_generator() {
            ctx.set_generator_kind(if data.ast.is_async() {
                GeneratorKind::Async
            } else {
                GeneratorKind::Sync
            });
        }

        let is_concise = data.ast.is_concise_body();

        ctx.compile_function_body(data);

        if is_concise {
            ctx.do_implicit_return();
        }

        ctx.finish()
    }

    pub(crate) fn compile_eval_body<'s>(
        agent: &mut Agent,
        body: &'s [ast::Statement<'s>],
        directives: &'s [ast::Directive<'s>],
        source_code: SourceCode<'gc>,
        scoping: &'s Scoping,
        nodes: &'s AstNodes<'s>,
        is_strict: bool,
        gc: NoGcScope<'gc, '_>,
    ) -> Self {
        if agent.options.print_internals {
            eprintln!();
            eprintln!("=== Compiling Eval Body ===");
            eprintln!();
        }
        let mut ctx = CompileContext::new(agent, source_code, scoping, nodes, gc);
        ctx.set_runtime_strict(is_strict);

        ctx.compile_directive_prologue(directives);
        ctx.compile_statements(body);
        ctx.do_implicit_return();
        ctx.finish()
    }

    /// Drops the Executable's heap-allocated data if possible.
    ///
    /// ## Correctness
    ///
    /// Any attempt to use the Executable after this call will lead to a crash
    /// if the drop was performed.
    pub(crate) fn try_drop(self, agent: &mut Agent) {
        debug_assert!(!agent.heap.executables.is_empty());
        let index = self.get_index();
        let last_index = agent.heap.executables.len() - 1;
        if last_index == index {
            // This bytecode was the last-allocated bytecode, and we can drop
            // it from the Heap without affecting any other indexes. The caller
            // guarantees that the Executable will not be used anymore.
            agent.heap.alloc_counter = agent
                .heap
                .alloc_counter
                .saturating_sub(core::mem::size_of::<ExecutableHeapData>());
            let _ = agent.heap.executables.pop().unwrap();
        }
    }

    /// The bytecode, shared: a running VM holds its own reference, so the
    /// bytecode outlives the Executable if that is dropped while it runs.
    #[inline]
    fn get_instructions(self, agent: &Agent) -> Arc<[u8]> {
        self.get(agent).instructions.clone()
    }

    #[inline]
    fn peek_instruction_byte(self, agent: &Agent, ip: usize) -> Option<u8> {
        self.get(agent).instructions.get(ip).copied()
    }

    #[inline]
    fn get_instruction(self, agent: &Agent, ip: &mut usize) -> Option<Instr> {
        Instr::consume_instruction(&self.get(agent).instructions, ip)
    }

    #[inline]
    fn get_constants<'a>(self, agent: &'a Agent, _: NoGcScope<'gc, '_>) -> &'a [Value<'gc>] {
        &self.get(agent).constants[..]
    }

    #[inline]
    fn fetch_cache(
        self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> PropertyLookupCache<'gc> {
        self.get(agent).caches[index].bind(gc)
    }

    #[inline]
    fn fetch_constant(self, agent: &Agent, index: usize, gc: NoGcScope<'gc, '_>) -> Value<'gc> {
        self.get(agent).constants[index].bind(gc)
    }

    #[inline]
    fn fetch_identifier(self, agent: &Agent, index: usize, gc: NoGcScope<'gc, '_>) -> String<'gc> {
        let value = self.get(agent).constants[index];
        let Ok(value) = String::try_from(value) else {
            handle_identifier_failure()
        };
        value.bind(gc)
    }

    #[inline]
    fn fetch_property_key(
        self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> PropertyKey<'gc> {
        let value = self.get(agent).constants[index];
        PropertyKey::from_value_unchecked(value).bind(gc)
    }

    fn fetch_function_expression<'a>(
        self,
        agent: &'a Agent,
        index: usize,
        _: NoGcScope<'gc, '_>,
    ) -> &'a FunctionExpression<'gc> {
        &self.get(agent).function_expressions[index]
    }

    fn fetch_arrow_function_expression<'a>(
        self,
        agent: &'a Agent,
        index: usize,
        _: NoGcScope<'gc, '_>,
    ) -> &'a ArrowFunctionExpression<'gc> {
        &self.get(agent).arrow_function_expressions[index]
    }

    fn fetch_class_initializer_bytecode(
        self,
        agent: &Agent,
        index: usize,
        _: NoGcScope<'gc, '_>,
    ) -> (Option<Executable<'gc>>, bool) {
        self.get(agent).class_initializer_bytecodes[index]
    }

    fn fetch_object_shape(
        self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> ObjectShape<'gc> {
        self.get(agent).shapes[index].bind(gc)
    }
}

impl Scoped<'_, Executable<'static>> {
    #[inline]
    pub(super) fn get_instructions(&self, agent: &Agent) -> Arc<[u8]> {
        self.get(agent).get_instructions(agent)
    }

    #[inline]
    pub(super) fn peek_instruction_byte(&self, agent: &Agent, ip: usize) -> Option<u8> {
        self.get(agent).peek_instruction_byte(agent, ip)
    }

    #[inline]
    pub(super) fn get_instruction(&self, agent: &Agent, ip: &mut usize) -> Option<Instr> {
        self.get(agent).get_instruction(agent, ip)
    }

    #[inline]
    pub(super) fn get_constants<'a, 'gc>(
        &self,
        agent: &'a Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> &'a [Value<'gc>] {
        self.get(agent).get_constants(agent, gc)
    }

    #[inline]
    pub(super) fn fetch_cache<'gc>(
        &self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> PropertyLookupCache<'gc> {
        self.get(agent).fetch_cache(agent, index, gc)
    }

    #[inline]
    pub(super) fn fetch_identifier<'gc>(
        &self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> String<'gc> {
        self.get(agent).fetch_identifier(agent, index, gc)
    }

    #[inline]
    pub(super) fn fetch_property_key<'gc>(
        &self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> PropertyKey<'gc> {
        self.get(agent).fetch_property_key(agent, index, gc)
    }

    #[inline]
    pub(super) fn fetch_constant<'gc>(
        &self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> Value<'gc> {
        self.get(agent).fetch_constant(agent, index, gc)
    }

    #[inline]
    pub(super) fn fetch_function_expression<'a, 'gc>(
        &self,
        agent: &'a Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> &'a FunctionExpression<'gc> {
        self.get(agent).fetch_function_expression(agent, index, gc)
    }

    #[inline]
    pub(super) fn fetch_arrow_function_expression<'a, 'gc>(
        &self,
        agent: &'a Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> &'a ArrowFunctionExpression<'gc> {
        self.get(agent)
            .fetch_arrow_function_expression(agent, index, gc)
    }

    #[inline]
    pub(super) fn fetch_class_initializer_bytecode<'gc>(
        &self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> (Option<Executable<'gc>>, bool) {
        self.get(agent)
            .fetch_class_initializer_bytecode(agent, index, gc)
    }

    #[inline]
    pub(super) fn fetch_object_shape<'gc>(
        &self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> ObjectShape<'gc> {
        self.get(agent).fetch_object_shape(agent, index, gc)
    }
}

impl<'a> CreateHeapData<ExecutableHeapData<'a>, Executable<'a>> for Heap {
    fn create(&mut self, data: ExecutableHeapData<'a>) -> Executable<'a> {
        let index = u32::try_from(self.executables.len()).expect("Executables overflowed");
        self.executables.push(data.unbind());
        self.alloc_counter += core::mem::size_of::<ExecutableHeapData<'static>>();
        // SAFETY: After pushing to executables, the vector cannot be empty.
        Executable(BaseIndex::from_index_u32(index))
    }
}

impl HeapMarkAndSweep for Executable<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.executables.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.executables.shift_index(&mut self.0);
    }
}

impl HeapMarkAndSweep for ExecutableHeapData<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            instructions: _,
            caches,
            constants,
            shapes,
            function_expressions,
            arrow_function_expressions,
            class_initializer_bytecodes,
            instance_initializer,
            _gc: _,
        } = self;
        instance_initializer.mark_values(queues);
        constants.mark_values(queues);
        arrow_function_expressions.mark_values(queues);
        caches.mark_values(queues);
        shapes.mark_values(queues);
        function_expressions.mark_values(queues);
        class_initializer_bytecodes.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            instructions: _,
            caches,
            constants,
            shapes,
            function_expressions,
            arrow_function_expressions,
            class_initializer_bytecodes,
            instance_initializer,
            _gc: _,
        } = self;
        instance_initializer.sweep_values(compactions);
        constants.sweep_values(compactions);
        arrow_function_expressions.sweep_values(compactions);
        caches.sweep_values(compactions);
        shapes.sweep_values(compactions);
        function_expressions.sweep_values(compactions);
        class_initializer_bytecodes.sweep_values(compactions);
    }
}

#[cold]
fn handle_identifier_failure() -> ! {
    panic!("Invalid identifier index: Value was not a String")
}
