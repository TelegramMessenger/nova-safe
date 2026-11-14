// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! SourceCode is a Nova-engine specific concept to capture and keep any
//! `eval(source)` source strings alive after the eval call for the case where
//! that the eval call defines functions. Those functions will refer to the
//! SourceCode for their function source text.

use core::fmt::Debug;

use oxc_allocator::Allocator;
use oxc_ast::ast;
use oxc_diagnostics::OxcDiagnostic;
use oxc_parser::{Parser, ParserReturn};
use oxc_semantic::{AstNodes, Scoping, SemanticBuilder, SemanticBuilderReturn};
use oxc_span::SourceType;

use crate::{
    ecmascript::{HeapString, String, execution::Agent},
    engine::{Bindable, GcScope, NoGcScope},
    heap::{
        ArenaAccess, BaseIndex, CompactionLists, CreateHeapData, Heap, HeapIndexHandle,
        HeapMarkAndSweep, WorkQueues, arena_vec_access, index_handle,
    },
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub(crate) struct SourceCode<'a>(BaseIndex<'a, SourceCodeHeapData<'static>>);
index_handle!(SourceCode);
arena_vec_access!(SourceCode, 'a, SourceCodeHeapData, source_codes);

impl core::fmt::Debug for SourceCode<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "SourceCode({:?})", self.0.get_index_u32())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceCodeType {
    Eval { direct: bool, strict: bool },
    Script { strict: bool },
    Module,
}

impl SourceCodeType {
    fn is_strict(&self) -> bool {
        match self {
            SourceCodeType::Eval { strict, .. } | SourceCodeType::Script { strict } => *strict,
            SourceCodeType::Module => true,
        }
    }
}

/// A parsed program, borrowed from the parser's arena for the duration of a
/// [`SourceCode::parse_source`] callback.
pub(crate) struct ParseResult<'p, 'a> {
    pub(crate) source_code: SourceCode<'a>,
    pub(crate) body: &'p [ast::Statement<'p>],
    pub(crate) directives: &'p [ast::Directive<'p>],
    pub(crate) scoping: &'p Scoping,
    pub(crate) nodes: &'p AstNodes<'p>,
    pub(crate) is_strict: bool,
}

impl<'a> SourceCode<'a> {
    /// Parses the given source string as JavaScript code, creates a
    /// SourceCode heap reference for it and calls `f` with the parsed
    /// program.
    ///
    /// The program lives in an arena that is dropped when `f` returns: `f`
    /// compiles what it needs from it into the heap.
    pub(crate) fn parse_source<R>(
        agent: &mut Agent,
        source: String,
        source_type: SourceCodeType,
        #[cfg(feature = "typescript")] typescript: bool,
        gc: NoGcScope<'a, '_>,
        f: impl for<'p> FnOnce(&mut Agent, ParseResult<'p, 'a>) -> R,
    ) -> Result<R, Vec<OxcDiagnostic>> {
        #[cfg(not(feature = "typescript"))]
        let typescript = false;
        let (source, source_text) = heap_source(agent, source, gc);
        let allocator = Allocator::new();
        let parsed = parse_program(&allocator, &source_text, source_type, typescript)?;
        let source_code = agent.heap.create(SourceCodeHeapData {
            source: source.unbind(),
        });
        let (program, scoping, nodes) = (parsed.0, &parsed.1, &parsed.2);
        let is_strict = source_type.is_strict() || program.has_use_strict_directive();
        Ok(f(
            agent,
            ParseResult {
                source_code: source_code.bind(gc),
                body: program.body.as_slice(),
                directives: program.directives.as_slice(),
                scoping,
                nodes,
                is_strict,
            },
        ))
    }

    /// Like [`SourceCode::parse_source`], but `f` also gets the garbage
    /// collector scope, so that it can run code while the program lives.
    pub(crate) fn parse_source_with_gc<'gc, R>(
        agent: &mut Agent,
        source: String,
        source_type: SourceCodeType,
        #[cfg(feature = "typescript")] typescript: bool,
        gc: GcScope<'gc, '_>,
        f: impl for<'p> FnOnce(&mut Agent, ParseResult<'p, 'static>, GcScope<'gc, '_>) -> R,
    ) -> Result<R, Vec<OxcDiagnostic>> {
        #[cfg(not(feature = "typescript"))]
        let typescript = false;
        let (source, source_text) = heap_source(agent, source, gc.nogc());
        let source = source.unbind();
        let allocator = Allocator::new();
        let parsed = parse_program(&allocator, &source_text, source_type, typescript)?;
        let source_code = agent.heap.create(SourceCodeHeapData { source }).unbind();
        let (program, scoping, nodes) = (parsed.0, &parsed.1, &parsed.2);
        let is_strict = source_type.is_strict() || program.has_use_strict_directive();
        Ok(f(
            agent,
            ParseResult {
                source_code,
                body: program.body.as_slice(),
                directives: program.directives.as_slice(),
                scoping,
                nodes,
                is_strict,
            },
            gc,
        ))
    }

    /// Manually drop a SourceCode.
    ///
    /// ## Correctness
    ///
    /// The caller must guarantee that the SourceCode has not and will not be
    /// executed.
    ///
    /// ## Panics
    ///
    /// If the SourceCode was not the last SourceCode to be allocated.
    pub(crate) fn manually_drop(self, agent: &mut Agent) {
        agent.heap.alloc_counter = agent
            .heap
            .alloc_counter
            .saturating_sub(core::mem::size_of::<Option<SourceCodeHeapData<'static>>>());
        assert_eq!(self, SourceCode(BaseIndex::last(&agent.heap.source_codes)));
        agent.heap.source_codes.pop();
    }

    pub(crate) fn get_source_text(self, agent: &Agent) -> &str {
        self.get(agent)
            .source
            .get(agent)
            .as_str()
            .expect("parse_source copies non-UTF-8 source texts into UTF-8")
    }
}

#[derive(Bindable)]
pub(crate) struct SourceCodeHeapData<'a> {
    /// The source JavaScript string data the eval was called with. The string
    /// is known and required to be a HeapString because functions created
    /// in the eval call keep its span to give their source text.
    source: HeapString<'a>,
}

impl Debug for SourceCodeHeapData<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SourceCodeHeapData")
            .field("source", &self.source)
            .finish()
    }
}

impl<'a> CreateHeapData<SourceCodeHeapData<'a>, SourceCode<'a>> for Heap {
    fn create(&mut self, data: SourceCodeHeapData<'a>) -> SourceCode<'a> {
        self.source_codes.push(data.unbind());
        self.alloc_counter += core::mem::size_of::<SourceCodeHeapData<'static>>();
        SourceCode(BaseIndex::last(&self.source_codes))
    }
}

impl HeapMarkAndSweep for SourceCodeHeapData<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self { source } = self;
        source.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self { source } = self;
        source.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for SourceCode<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.source_codes.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.source_codes.shift_index(&mut self.0);
    }
}

/// Makes sure the source is a heap string, which functions created from
/// it refer to for their source text, and copies its text out for the
/// parser.
fn heap_source<'g>(
    agent: &mut Agent,
    source: String,
    gc: NoGcScope<'g, '_>,
) -> (HeapString<'g>, std::string::String) {
    match source {
        String::String(source) => match source.to_string_lossy(agent) {
            std::borrow::Cow::Borrowed(source_text) => {
                // Source text is a valid heap-allocated UTF-8 string.
                let source_text = source_text.to_owned();
                (source.bind(gc), source_text)
            }
            std::borrow::Cow::Owned(string) => {
                // Source text is invalid UTF-8 and needed to be copied.
                let source_text = string.clone();
                let String::String(source) = String::from_string(agent, string, gc) else {
                    unreachable!()
                };
                (source, source_text)
            }
        },
        String::SmallString(source) => {
            // Add 10 whitespace bytes to the end of the eval string. This
            // should guarantee that the string gets heap-allocated.
            let source_text = source.to_string_lossy().into_owned();
            let data = format!("{source_text}          ");
            let String::String(source) = String::from_string(agent, data, gc) else {
                unreachable!()
            };
            // The parser gets the original text without the added
            // whitespace.
            (source, source_text)
        }
    }
}

type ParsedProgram<'p> = (&'p ast::Program<'p>, Scoping, AstNodes<'p>);

/// Parses and checks a program into `allocator`.
fn parse_program<'p>(
    allocator: &'p Allocator,
    source_text: &'p str,
    source_type: SourceCodeType,
    typescript: bool,
) -> Result<ParsedProgram<'p>, Vec<OxcDiagnostic>> {
    if super::source_nesting::source_nesting_too_deep(source_text) {
        return Err(vec![OxcDiagnostic::error(
            "Source text nests too deeply to be parsed",
        )]);
    }
    let parser_result = match source_type {
        SourceCodeType::Script { strict } | SourceCodeType::Eval { strict, .. } => {
            // Potentially strict script! We first parse and syntax check
            // this as a normal script, which checks that the code contains
            // no module declarations or TLA. If that passes and we're
            // strict, then we parse the script as a module which sets
            // strict mode on.
            let source_type = SourceType::script().with_typescript(typescript);
            if strict {
                // The sloppy program is only checked, in an arena of its own
                // that is dropped before the strict parse.
                let sloppy_allocator = Allocator::new();
                let ParserReturn {
                    errors: sloppy_errors,
                    program: sloppy_program,
                    panicked,
                    ..
                } = Parser::new(&sloppy_allocator, source_text, source_type).parse();
                if panicked {
                    return Err(sloppy_errors);
                }
                let SemanticBuilderReturn {
                    errors: sloppy_errors,
                    ..
                } = SemanticBuilder::new()
                    .with_check_syntax_error(true)
                    .build(&sloppy_program);

                if !sloppy_errors.is_empty() {
                    return Err(sloppy_errors);
                }
                drop(sloppy_program);
                drop(sloppy_allocator);

                let source_type = SourceType::mjs().with_typescript(typescript);
                let strict_result = Parser::new(allocator, source_text, source_type).parse();
                if strict_result.panicked {
                    let errors = strict_result.errors;
                    return Err(errors);
                }

                strict_result
            } else {
                Parser::new(allocator, source_text, source_type).parse()
            }
        }
        SourceCodeType::Module => {
            let source_type = SourceType::mjs().with_typescript(typescript);
            Parser::new(allocator, source_text, source_type).parse()
        }
    };

    let ParserReturn {
        errors, program, ..
    } = parser_result;

    if !errors.is_empty() {
        return Err(errors);
    }
    // The semantic analysis refers to the program node, so it is moved into
    // the arena with the rest of the program.
    let program: &'p ast::Program<'p> = allocator.alloc(program);

    let SemanticBuilderReturn { errors, semantic } = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(program);

    if !errors.is_empty() {
        return Err(errors);
    }
    let (scoping, nodes) = semantic.into_scoping_and_nodes();
    Ok((program, scoping, nodes))
}

#[cfg(test)]
mod test {
    use crate::{
        ecmascript::{
            Agent, AgentOptions, DefaultHostHooks, SourceCode, SourceCodeType, String,
            initialize_default_realm,
        },
        engine::GcScope,
    };

    #[test]
    fn script_with_imports() {
        let (mut gc, mut scope) = GcScope::create_root();
        let mut gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        initialize_default_realm(&mut agent, gc.reborrow());

        let source_text = String::from_static_str(&mut agent, "import 'foo';", gc.nogc());
        let errors = SourceCode::parse_source(
            &mut agent,
            source_text,
            SourceCodeType::Script { strict: false },
            #[cfg(feature = "typescript")]
            false,
            gc.nogc(),
            |_, _| (),
        )
        .unwrap_err();

        assert!(!errors.is_empty());
    }

    #[test]
    fn strict_script_with_imports() {
        let (mut gc, mut scope) = GcScope::create_root();
        let mut gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        initialize_default_realm(&mut agent, gc.reborrow());

        let source_text = String::from_static_str(&mut agent, "import 'foo';", gc.nogc());
        let errors = SourceCode::parse_source(
            &mut agent,
            source_text,
            SourceCodeType::Script { strict: true },
            #[cfg(feature = "typescript")]
            false,
            gc.nogc(),
            |_, _| (),
        )
        .unwrap_err();

        assert!(!errors.is_empty());
    }

    #[test]
    fn parse_and_realloc_source_codes_with_program() {
        let (mut gc, mut scope) = GcScope::create_root();
        let mut gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        initialize_default_realm(&mut agent, gc.reborrow());

        let source_text = String::from_static_str(&mut agent, "const foo = 3;", gc.nogc());
        let is_declaration = SourceCode::parse_source(
            &mut agent,
            source_text,
            SourceCodeType::Script { strict: true },
            #[cfg(feature = "typescript")]
            false,
            gc.nogc(),
            |agent, parsed| {
                let cap = agent.heap.source_codes.capacity();
                // Force the vector capacity to double, reallocating the
                // vector: the program does not live in it.
                agent
                    .heap
                    .source_codes
                    .reserve(cap - agent.heap.source_codes.len() + 1);
                assert_ne!(agent.heap.source_codes.capacity(), cap);
                parsed.body[0].is_declaration()
            },
        )
        .unwrap();
        assert!(is_declaration);
    }
}
