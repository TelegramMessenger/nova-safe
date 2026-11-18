// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{
        Agent, BindingSite, CompileFunctionBodyData, FunctionAstRef, FunctionTemplate, ObjectShape,
        PropertyKey, PropertyLookupCache, SourceCode, String, Value, is_strictly_equal,
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
use core::{cell::Cell, marker::PhantomData};
use oxc_ast::ast;
use oxc_semantic::{AstNodes, Scoping};
use std::{rc::Rc, sync::Arc};

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
    /// The constants, the property caches and the binding sites: see
    /// [`CodeTables`]. A copy of this data shares them, and is only to be
    /// read: two Executables in the heap with one set of tables would have
    /// the collector rewrite them twice.
    pub(crate) tables: Rc<CodeTables>,
    /// Constants past the 16-bit indices, loaded by the Wide instructions:
    /// literals of a script with more distinct constants than those fit.
    pub(crate) wide_constants: Box<[Value<'static>]>,
    pub(crate) shapes: Box<[ObjectShape<'static>]>,
    pub(crate) function_expressions: Box<[FunctionExpression<'static>]>,
    pub(crate) arrow_function_expressions: Box<[ArrowFunctionExpression<'static>]>,
    pub(crate) class_initializer_bytecodes: Box<[(Option<Executable<'static>>, bool)]>,
    /// The instance field and private method initializers of a derived class
    /// constructor: SuperCall runs them on the new `this` value.
    pub(crate) instance_initializer: Option<Executable<'static>>,
    /// The environment a \[\[Call]] of the function this executable is the body
    /// of creates: see function_declaration_instantiation::instantiation.
    pub(crate) call_environment: CallEnvironment,
    /// The function this executable is the body of keeps its parameters in
    /// its first so many stack slots and reads its arguments nowhere else:
    /// whoever starts the code puts the arguments there
    /// (function_declaration_instantiation::instantiation, Vm::execute).
    pub(crate) stack_parameters: Option<u16>,
    /// The `case` selectors of each `switch` that tests them with one
    /// JumpToCase instruction; nothing here is known to the collector, the
    /// selectors are among the constants.
    pub(crate) switch_tables: Arc<[SwitchTable]>,
    pub(crate) _gc: PhantomData<Value<'a>>,
}

/// What a `Vm` reads of an Executable at every turn of its loop: the
/// instructions, and the tables they read by an index, which are the
/// constants (the names of variables and properties among them), the
/// property cache of each place that names a property, and where each
/// place that names a variable of an enclosing scope found it the last
/// time (nothing of which is known to the collector).
///
/// The Executable in the heap and every `Vm` that is running its code hold
/// this together, by one count: a call that the `Vm` runs itself takes one
/// reference for all of it, and the loop reads a constant with one look at
/// the table, where it went by the heap's table of executables to this
/// Executable and from there to its constants, for every one. The entries
/// of the tables are cells. The collector moves what the constants and the caches
/// refer to and rewrites them where they are, and a binding site is
/// remembered where it is: whoever holds the tables reads what is current.
#[derive(Debug)]
pub(crate) struct CodeTables {
    /// The bytecode itself, which the Executable holds too.
    instructions: Arc<[u8]>,
    constants: Box<[Cell<Value<'static>>]>,
    caches: Box<[Cell<PropertyLookupCache<'static>>]>,
    binding_sites: Box<[Cell<BindingSite>]>,
}

impl CodeTables {
    /// The tables of a new Executable, with `binding_sites` places that
    /// have found nothing yet.
    pub(crate) fn new(
        instructions: Arc<[u8]>,
        constants: Vec<Value<'static>>,
        caches: Vec<PropertyLookupCache<'static>>,
        binding_sites: usize,
    ) -> Rc<Self> {
        Rc::new(Self {
            instructions,
            constants: constants.into_iter().map(Cell::new).collect(),
            caches: caches.into_iter().map(Cell::new).collect(),
            binding_sites: (0..binding_sites)
                .map(|_| Cell::new(BindingSite::UNKNOWN))
                .collect(),
        })
    }

    #[inline(always)]
    pub(crate) fn instructions(&self) -> &[u8] {
        &self.instructions
    }

    /// The tables as the loop of a `Vm` keeps them at hand.
    #[inline(always)]
    pub(crate) fn view(&self) -> TablesView<'_> {
        TablesView {
            constants: &self.constants,
            caches: &self.caches,
            binding_sites: &self.binding_sites,
        }
    }

    pub(crate) fn constants(&self) -> impl ExactSizeIterator<Item = Value<'static>> + '_ {
        self.constants.iter().map(Cell::get)
    }

    pub(crate) fn caches(
        &self,
    ) -> impl ExactSizeIterator<Item = PropertyLookupCache<'static>> + '_ {
        self.caches.iter().map(Cell::get)
    }

    pub(crate) fn binding_sites(&self) -> usize {
        self.binding_sites.len()
    }

    #[inline]
    fn constant(&self, index: usize) -> Option<Value<'static>> {
        self.constants.get(index).map(Cell::get)
    }

    fn set_binding_site(&self, index: usize, site: BindingSite) {
        if let Some(slot) = self.binding_sites.get(index) {
            slot.set(site);
        }
    }

    /// Bytes of the heap that the constants and the caches take.
    pub(crate) fn heap_bytes(&self) -> usize {
        core::mem::size_of_val::<[_]>(&self.constants) + core::mem::size_of_val::<[_]>(&self.caches)
    }

    fn mark_values(&self, queues: &mut WorkQueues) {
        for constant in self.constants.iter() {
            constant.get().mark_values(queues);
        }
        for cache in self.caches.iter() {
            cache.get().mark_values(queues);
        }
    }

    fn sweep_values(&self, compactions: &CompactionLists) {
        for constant in self.constants.iter() {
            let mut value = constant.get();
            value.sweep_values(compactions);
            constant.set(value);
        }
        for cache in self.caches.iter() {
            let mut value = cache.get();
            value.sweep_values(compactions);
            cache.set(value);
        }
    }
}

/// The three tables of a [`CodeTables`], each a slice: what the loop of a
/// `Vm` reads constants, names, caches and binding sites from.
#[derive(Clone, Copy)]
pub(crate) struct TablesView<'a> {
    constants: &'a [Cell<Value<'static>>],
    caches: &'a [Cell<PropertyLookupCache<'static>>],
    binding_sites: &'a [Cell<BindingSite>],
}

impl TablesView<'_> {
    #[inline(always)]
    pub(super) fn constant<'gc>(self, index: usize, gc: NoGcScope<'gc, '_>) -> Value<'gc> {
        self.constants[index].get().bind(gc)
    }

    #[inline(always)]
    pub(super) fn identifier<'gc>(self, index: usize, gc: NoGcScope<'gc, '_>) -> String<'gc> {
        let Ok(value) = String::try_from(self.constants[index].get()) else {
            handle_identifier_failure()
        };
        value.bind(gc)
    }

    #[inline(always)]
    pub(super) fn property_key<'gc>(
        self,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> PropertyKey<'gc> {
        PropertyKey::from_value_unchecked(self.constants[index].get()).bind(gc)
    }

    #[inline(always)]
    pub(super) fn cache<'gc>(
        self,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> PropertyLookupCache<'gc> {
        self.caches[index].get().bind(gc)
    }

    #[inline(always)]
    pub(super) fn binding_site(self, index: usize) -> BindingSite {
        self.binding_sites
            .get(index)
            .map_or(BindingSite::ELSEWHERE, Cell::get)
    }
}

/// The `case` selectors of a `switch` statement that are all literals, for
/// the JumpToCase instruction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SwitchTable {
    /// The selectors in source order: the index of each among the constants,
    /// and where the statements of its case start.
    pub(crate) cases: Box<[(u16, u32)]>,
    /// For selectors that are all integers close to one another: the lowest
    /// of them, and where the case of each integer from it on starts, 0 for
    /// an integer that no case has. An integer finds its case here without
    /// a comparison.
    pub(crate) dense: Option<(i64, Box<[u32]>)>,
}

impl SwitchTable {
    /// How far apart the selectors of a table with a `dense` part may be.
    const DENSE_RANGE: i64 = 256;

    /// The table of `cases`, whose selectors are those `constants`.
    pub(crate) fn new(cases: Vec<(u16, u32)>, constants: &[Value]) -> Self {
        let integers = cases
            .iter()
            .map(|&(selector, target)| match constants[selector as usize] {
                Value::Integer(integer) => Some((integer.into_i64(), target)),
                _ => None,
            })
            .collect::<Option<Vec<_>>>();
        let dense = integers.and_then(|integers| {
            let lowest = integers.iter().map(|&(integer, _)| integer).min()?;
            let highest = integers.iter().map(|&(integer, _)| integer).max()?;
            let range = usize::try_from(highest - lowest)
                .ok()
                .filter(|range| *range < Self::DENSE_RANGE as usize)?;
            let mut targets = vec![0; range + 1];
            for (integer, target) in integers {
                // The first of two cases with one selector is the one taken.
                let slot = &mut targets[(integer - lowest) as usize];
                if *slot == 0 {
                    *slot = target;
                }
            }
            Some((lowest, targets.into_boxed_slice()))
        });
        Self {
            cases: cases.into_boxed_slice(),
            dense,
        }
    }
}

/// The environment a \[\[Call]] of an ECMAScript function creates for its code.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallEnvironment {
    /// A Function Environment Record: NewFunctionEnvironment(F, newTarget).
    #[default]
    Function,
    /// A Declarative Environment Record for the function's bindings: nothing
    /// asks for the this binding of a Function Environment Record.
    Declarative,
    /// None: the function's code runs in F.\[\[Environment]].
    None,
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
        // A class constructor too: \[\[Call]] of it throws whatever its body
        // needs (ecmascript_function::call_environment), and `new` of one
        // that is not derived may be run without a Function Environment
        // Record where the body needs none.
        let plain_function = matches!(
            data.ast,
            FunctionAstRef::Function(_)
                | FunctionAstRef::Arrow(_)
                | FunctionAstRef::ClassConstructor(_)
        );

        let call_environment = ctx.compile_function_body(data);
        if plain_function {
            ctx.set_call_environment(call_environment);
        }

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
    pub(super) fn get_instructions(self, agent: &Agent) -> Arc<[u8]> {
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

    /// The tables of this Executable, shared: see [`CodeTables`].
    #[inline]
    pub(super) fn get_tables(self, agent: &Agent) -> Rc<CodeTables> {
        self.get(agent).tables.clone()
    }

    fn get_constants(self, agent: &Agent, gc: NoGcScope<'gc, '_>) -> Vec<Value<'gc>> {
        self.get(agent)
            .tables
            .constants()
            .map(|value| value.bind(gc))
            .collect()
    }

    #[inline(always)]
    pub(super) fn fetch_cache(
        self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> PropertyLookupCache<'gc> {
        self.get(agent).tables.view().cache(index, gc)
    }

    #[inline(always)]
    pub(super) fn fetch_constant(
        self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> Value<'gc> {
        self.get(agent).tables.view().constant(index, gc)
    }

    #[inline]
    fn stack_parameters(self, agent: &Agent) -> Option<u16> {
        self.get(agent).stack_parameters
    }

    #[inline(always)]
    pub(super) fn fetch_binding_site(self, agent: &Agent, index: usize) -> BindingSite {
        self.get(agent).tables.view().binding_site(index)
    }

    /// Where the `switch` whose table is at `index` goes for `value`: to
    /// the first case whose selector IsStrictlyEqual(value, selector) holds
    /// for, if there is one.
    #[inline]
    pub(super) fn switch_target(self, agent: &Agent, index: usize, value: Value) -> Option<usize> {
        let data = self.get(agent);
        let table = data.switch_tables.get(index)?;
        if let (Value::Integer(value), Some((lowest, targets))) = (value, &table.dense) {
            // Every selector is an integer: no other case could match.
            let target = usize::try_from(value.into_i64() - lowest)
                .ok()
                .and_then(|distance| targets.get(distance))
                .copied()
                .unwrap_or(0);
            return (target != 0).then_some(target as usize);
        }
        table
            .cases
            .iter()
            .find(|&&(selector, _)| {
                data.tables
                    .constant(selector as usize)
                    .is_some_and(|selector| is_strictly_equal(agent, value, selector))
            })
            .map(|&(_, target)| target as usize)
    }

    fn set_binding_site(self, agent: &Agent, index: usize, site: BindingSite) {
        self.get(agent).tables.set_binding_site(index, site);
    }

    #[inline]
    fn fetch_wide_constant(
        self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> Value<'gc> {
        self.get(agent).wide_constants[index].bind(gc)
    }

    #[inline(always)]
    pub(super) fn fetch_identifier(
        self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> String<'gc> {
        self.get(agent).tables.view().identifier(index, gc)
    }

    #[inline(always)]
    pub(super) fn fetch_property_key(
        self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> PropertyKey<'gc> {
        self.get(agent).tables.view().property_key(index, gc)
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

    /// See [`CodeTables`].
    #[inline]
    pub(super) fn get_tables(&self, agent: &Agent) -> Rc<CodeTables> {
        self.get(agent).get_tables(agent)
    }

    pub(super) fn get_constants<'gc>(
        &self,
        agent: &Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> Vec<Value<'gc>> {
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

    /// See [`ExecutableHeapData::stack_parameters`].
    #[inline]
    pub(super) fn stack_parameters(&self, agent: &Agent) -> Option<u16> {
        self.get(agent).stack_parameters(agent)
    }

    #[inline]
    pub(super) fn fetch_binding_site(&self, agent: &Agent, index: usize) -> BindingSite {
        self.get(agent).fetch_binding_site(agent, index)
    }

    pub(super) fn set_binding_site(&self, agent: &mut Agent, index: usize, site: BindingSite) {
        self.get(agent).set_binding_site(agent, index, site)
    }

    #[inline]
    pub(super) fn fetch_wide_constant<'gc>(
        &self,
        agent: &Agent,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> Value<'gc> {
        self.get(agent).fetch_wide_constant(agent, index, gc)
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
            tables,
            wide_constants,
            shapes,
            function_expressions,
            arrow_function_expressions,
            class_initializer_bytecodes,
            instance_initializer,
            call_environment: _,
            stack_parameters: _,
            switch_tables: _,
            _gc: _,
        } = self;
        instance_initializer.mark_values(queues);
        tables.mark_values(queues);
        wide_constants.mark_values(queues);
        arrow_function_expressions.mark_values(queues);
        shapes.mark_values(queues);
        function_expressions.mark_values(queues);
        class_initializer_bytecodes.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            instructions: _,
            tables,
            wide_constants,
            shapes,
            function_expressions,
            arrow_function_expressions,
            class_initializer_bytecodes,
            instance_initializer,
            call_environment: _,
            stack_parameters: _,
            switch_tables: _,
            _gc: _,
        } = self;
        instance_initializer.sweep_values(compactions);
        tables.sweep_values(compactions);
        wide_constants.sweep_values(compactions);
        arrow_function_expressions.sweep_values(compactions);
        shapes.sweep_values(compactions);
        function_expressions.sweep_values(compactions);
        class_initializer_bytecodes.sweep_values(compactions);
    }
}

#[cold]
fn handle_identifier_failure() -> ! {
    panic!("Invalid identifier index: Value was not a String")
}
