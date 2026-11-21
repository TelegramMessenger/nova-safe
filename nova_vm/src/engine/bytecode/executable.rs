// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::hashing::AHashMap;
use crate::{
    ecmascript::{
        Agent, BindingSite, CompileFunctionBodyData, FunctionAstRef, FunctionTemplate, NameKey,
        ObjectShape, PropertyKey, PropertyLookupCache, SourceCode, String, Value,
        is_strictly_equal,
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
/// The instructions of an Executable, shared by whoever runs them, and a few
/// bytes after them that are no instruction ([`Bytecode::PADDING`]).
///
/// The loop of a `Vm` reads an instruction with the eight bytes after it
/// in one look ([`Bytecode::padded`]): the longest instruction, whichever
/// of them the one it finds has. It went another, slower way for an
/// instruction that has fewer bytes of code after it, as the Return that
/// ends nearly every function has none. The padding gives every one its
/// eight bytes. It is not code: the instructions, where the code ends, and
/// what a jump or a listing may reach are [`Bytecode::code`], and nothing
/// but that look reads the padding.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Bytecode(Arc<[u8]>);

impl Bytecode {
    /// The bytes after the instructions: as many as the loop reads beyond
    /// the first byte of an instruction.
    const PADDING: usize = Instr::WINDOW - 1;

    pub(crate) fn new(mut instructions: Vec<u8>) -> Self {
        instructions.extend_from_slice(&[0; Self::PADDING]);
        Self(instructions.into())
    }

    /// The instructions.
    #[inline(always)]
    pub(crate) fn code(&self) -> &[u8] {
        match self.0.split_last_chunk::<{ Self::PADDING }>() {
            Some((code, _)) => code,
            None => &[],
        }
    }

    /// The instructions and the padding after them, for reading an
    /// instruction with the eight bytes that follow it.
    #[inline(always)]
    pub(crate) fn padded(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Bindable)]
pub(crate) struct ExecutableHeapData<'a> {
    pub(crate) instructions: Bytecode,
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
    instructions: Bytecode,
    constants: Box<[Cell<Value<'static>>]>,
    caches: Box<[Cell<PropertyLookupCache<'static>>]>,
    binding_sites: Box<[Cell<NamedSite>]>,
}

/// A [`BindingSite`] with the name whose place it remembers: one place in
/// the code names one name. The two are kept together, so that the
/// instruction that reads a variable where it was found the last time has
/// both by one look at one table. With the name among the constants it was
/// a second table asked, and the name made a String of the Value it is
/// kept as there, at every read: twelve instructions of the seventy-four.
///
/// The name is kept as the eight bytes a record keeps the names of its
/// bindings as ([`NameKey`]), so that it is compared with the name at the
/// place as it is: made such a key at every read and every assignment, it
/// was five instructions of each.
#[derive(Debug, Clone, Copy)]
struct NamedSite {
    site: BindingSite,
    /// The key of the empty String for a site that no instruction names.
    name: NameKey,
}

impl CodeTables {
    /// The tables of a new Executable, with a place that has found nothing
    /// yet for every entry of `binding_site_names`: where the name of the
    /// site is among the constants, as the compiler recorded it. The
    /// tables keep the names and not where they were: kept too, that list
    /// was a block of memory more for every function of a script in every
    /// Agent that runs it.
    pub(crate) fn new(
        instructions: Bytecode,
        constants: Vec<Value<'static>>,
        caches: Vec<PropertyLookupCache<'static>>,
        binding_site_names: &[u16],
    ) -> Rc<Self> {
        let binding_sites = binding_site_names
            .iter()
            .map(|&name| {
                Cell::new(NamedSite {
                    site: BindingSite::UNKNOWN,
                    name: NameKey::of(
                        constants
                            .get(usize::from(name))
                            .and_then(|&constant| String::try_from(constant).ok())
                            .unwrap_or(String::EMPTY_STRING),
                    ),
                })
            })
            .collect();
        Rc::new(Self {
            instructions,
            constants: constants.into_iter().map(Cell::new).collect(),
            caches: caches.into_iter().map(Cell::new).collect(),
            binding_sites,
        })
    }

    #[inline(always)]
    pub(crate) fn instructions(&self) -> &[u8] {
        self.instructions.code()
    }

    /// The instructions with the padding after them: see [`Bytecode`].
    #[inline(always)]
    pub(crate) fn padded(&self) -> &[u8] {
        self.instructions.padded()
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

    /// Where each binding site has its name among the constants: what
    /// [`CodeTables::new`] takes, for the compiled form of the code, from
    /// which every Agent makes tables of its own. Looked up again here,
    /// once for a script that is compiled to be kept: a name is the same
    /// String wherever it is among the constants, and a site that no
    /// instruction names has the empty String whatever this says of it.
    ///
    /// The constants are looked through once, for where each String among
    /// them stands first: looked through for every site, a function of
    /// many names took the product of the two to export.
    pub(crate) fn binding_site_names(&self) -> Box<[u16]> {
        let mut first_places: AHashMap<String<'static>, u16> = AHashMap::default();
        for (index, constant) in self.constants.iter().enumerate() {
            if let Ok(name) = String::try_from(constant.get()) {
                // A place past what a site can say is none.
                first_places
                    .entry(name)
                    .or_insert_with(|| u16::try_from(index).unwrap_or(u16::MAX));
            }
        }
        self.binding_sites
            .iter()
            .map(|named| {
                first_places
                    .get(&named.get().name.name())
                    .copied()
                    .unwrap_or(u16::MAX)
            })
            .collect()
    }

    /// [`CodeTables::binding_site_names`] as it was worked out, a search
    /// of the constants for every site: what the one above is held to.
    #[cfg(test)]
    pub(crate) fn binding_site_names_by_search(&self) -> Box<[u16]> {
        self.binding_sites
            .iter()
            .map(|named| {
                let name = Value::from(named.get().name.name());
                self.constants
                    .iter()
                    .position(|constant| constant.get() == name)
                    .and_then(|index| u16::try_from(index).ok())
                    .unwrap_or(u16::MAX)
            })
            .collect()
    }

    #[inline]
    fn constant(&self, index: usize) -> Option<Value<'static>> {
        self.constants.get(index).map(Cell::get)
    }

    fn set_binding_site(&self, index: usize, site: BindingSite) {
        if let Some(slot) = self.binding_sites.get(index) {
            slot.set(NamedSite {
                site,
                name: slot.get().name,
            });
        }
    }

    /// How many constants, caches and binding sites the tables have.
    #[cfg(test)]
    pub(crate) fn lengths(&self) -> (usize, usize, usize) {
        (
            self.constants.len(),
            self.caches.len(),
            self.binding_sites.len(),
        )
    }

    /// Bytes of the heap that the constants, the caches and the binding
    /// sites take: what the estimate of the live heap counts for the tables
    /// of an Executable, where no allocator counts for the Agent.
    pub(crate) fn heap_bytes(&self) -> usize {
        core::mem::size_of_val::<[_]>(&self.constants)
            + core::mem::size_of_val::<[_]>(&self.caches)
            + core::mem::size_of_val::<[_]>(&self.binding_sites)
    }

    fn mark_values(&self, queues: &mut WorkQueues) {
        for constant in self.constants.iter() {
            constant.get().mark_values(queues);
        }
        for cache in self.caches.iter() {
            cache.get().mark_values(queues);
        }
        // The names of the sites are constants too, and marked above.
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
        for named in self.binding_sites.iter() {
            // A name of the heap is at another index after a collection,
            // and its key is made again.
            let mut value = named.get();
            let mut name = value.name.name();
            name.sweep_values(compactions);
            value.name = NameKey::of(name);
            named.set(value);
        }
    }
}

/// The three tables of a [`CodeTables`], each a slice: what the loop of a
/// `Vm` reads constants, names, caches and binding sites from.
#[derive(Clone, Copy)]
pub(crate) struct TablesView<'a> {
    constants: &'a [Cell<Value<'static>>],
    caches: &'a [Cell<PropertyLookupCache<'static>>],
    binding_sites: &'a [Cell<NamedSite>],
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
            .map_or(BindingSite::ELSEWHERE, |named| named.get().site)
    }

    /// The BindingSite at `index` and the name whose place it remembers,
    /// as its key: see [`NamedSite`]. For the instructions that read or
    /// assign the variable at the place. A place that remembers nothing,
    /// with the key of a name that no variable has, where the executable
    /// has no such site.
    ///
    /// `identifier` is where the instruction that asks has the same name
    /// among the constants: the two are the same String for as long as
    /// the executable lives, through every collection, which a build with
    /// assertions checks at every use.
    #[inline(always)]
    pub(super) fn keyed_binding_site(
        self,
        index: usize,
        identifier: usize,
    ) -> (BindingSite, NameKey) {
        debug_assert!(self.binding_sites.get(index).is_none_or(|named| {
            self.constants.get(identifier).map(Cell::get) == Some(named.get().name.name().into())
        }));
        match self.binding_sites.get(index) {
            Some(named) => {
                let NamedSite { site, name } = named.get();
                (site, name)
            }
            None => (BindingSite::ELSEWHERE, NameKey::of(String::EMPTY_STRING)),
        }
    }

    /// The BindingSite at `index` and its name as a String, for an
    /// instruction that has the site and not the name
    /// (GetKeptPropertyWithCache): the name is the one the site was given
    /// when the code was compiled.
    #[inline(always)]
    pub(super) fn binding_site_with_name<'gc>(
        self,
        index: usize,
        gc: NoGcScope<'gc, '_>,
    ) -> (BindingSite, String<'gc>) {
        match self.binding_sites.get(index) {
            Some(named) => {
                let NamedSite { site, name } = named.get();
                (site, name.name().bind(gc))
            }
            None => (BindingSite::ELSEWHERE, String::EMPTY_STRING),
        }
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
    /// Bytes that the table holds besides itself: its cases, and the
    /// places of the integers of a dense one.
    pub(crate) fn heap_bytes(&self) -> usize {
        core::mem::size_of_val::<[_]>(&self.cases)
            + self
                .dense
                .as_ref()
                .map_or(0, |(_, places)| core::mem::size_of_val::<[_]>(places))
    }

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
    pub(super) fn get_instructions(self, agent: &Agent) -> Bytecode {
        self.get(agent).instructions.clone()
    }

    #[inline]
    fn peek_instruction_byte(self, agent: &Agent, ip: usize) -> Option<u8> {
        self.get(agent).instructions.code().get(ip).copied()
    }

    #[inline]
    fn get_instruction(self, agent: &Agent, ip: &mut usize) -> Option<Instr> {
        Instr::consume_instruction(self.get(agent).instructions.code(), ip)
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
    pub(super) fn get_instructions(&self, agent: &Agent) -> Bytecode {
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

/// The bytes that a constant, a cache and a binding site take in the tables
/// of an Executable ([`CodeTables`]).
#[cfg(test)]
pub(crate) fn table_entry_sizes() -> (usize, usize, usize) {
    (
        core::mem::size_of::<Cell<Value<'static>>>(),
        core::mem::size_of::<Cell<PropertyLookupCache<'static>>>(),
        core::mem::size_of::<Cell<NamedSite>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::{Bytecode, CodeTables, SwitchTable, table_entry_sizes};
    use crate::ecmascript::Value;
    use core::mem::size_of;

    /// The bytes of the tables are those of the constants, the caches and
    /// the binding sites: none of the three is left out.
    #[test]
    fn the_bytes_of_the_tables_count_their_binding_sites() {
        let bytes = |constants: usize, sites: &[u16]| {
            CodeTables::new(
                Bytecode::new(Vec::new()),
                vec![Value::Undefined; constants],
                Vec::new(),
                sites,
            )
            .heap_bytes()
        };
        let (value, _, site) = table_entry_sizes();
        assert_eq!(bytes(0, &[]), 0);
        assert_eq!(bytes(3, &[]), 3 * value);
        assert_eq!(bytes(3, &[0, 1, 2, 0, u16::MAX]), 3 * value + 5 * site);
        assert_eq!(bytes(0, &[u16::MAX]), site);
    }

    /// The bytes of a `switch` table are those of its cases, and of the
    /// places that one of integers close to one another has for them.
    #[test]
    fn the_bytes_of_a_switch_table_are_its_cases_and_its_places() {
        assert_eq!(SwitchTable::default().heap_bytes(), 0);
        let constants = [
            Value::from(5),
            Value::from(7),
            Value::from(10),
            Value::from(1000),
            Value::Undefined,
        ];
        // Integers from 5 to 10: six places.
        let close = SwitchTable::new(vec![(0, 11), (1, 22), (2, 33)], &constants);
        assert!(close.dense.is_some());
        assert_eq!(
            close.heap_bytes(),
            3 * size_of::<(u16, u32)>() + 6 * size_of::<u32>()
        );
        // Integers too far apart, and a selector that is no integer.
        for cases in [vec![(0, 11), (3, 22)], vec![(0, 11), (4, 22), (1, 33)]] {
            let count = cases.len();
            let table = SwitchTable::new(cases, &constants);
            assert!(table.dense.is_none());
            assert_eq!(table.heap_bytes(), count * size_of::<(u16, u32)>());
        }
    }
}
