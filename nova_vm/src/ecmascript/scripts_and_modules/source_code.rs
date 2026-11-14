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
use oxc_semantic::{AstNodes, Scoping, Semantic, SemanticBuilder, SemanticBuilderReturn};
use oxc_span::SourceType;

use crate::{
    ecmascript::{HeapString, String, TerminationReason, execution::Agent},
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
        let host_hooks = agent.host_hooks;
        let interrupt = move || host_hooks.check_interrupt();
        let allocator = Allocator::new();
        let program = parse_program(
            &allocator,
            &source_text,
            source_type,
            typescript,
            &interrupt,
        )
        .map_err(|errors| interrupted_errors(agent, errors))?;
        let (scoping, nodes) = check_program(&program, source_type, &interrupt)
            .map_err(|errors| interrupted_errors(agent, errors))?;
        let source_code = agent.heap.create(SourceCodeHeapData {
            source: source.unbind(),
        });
        let (scoping, nodes) = (&scoping, &nodes);
        let is_strict = source_type.is_strict() || program.has_use_strict_directive();
        agent.compile_limit_error = None;
        let result = f(
            agent,
            ParseResult {
                source_code: source_code.bind(gc),
                body: program.body.as_slice(),
                directives: program.directives.as_slice(),
                scoping,
                nodes,
                is_strict,
            },
        );
        if let Some(message) = agent.compile_limit_error.take() {
            // Code too large for the bytecode's operands, as QuickJS's "too
            // many ..." errors; what was compiled is left to the collector.
            return Err(vec![OxcDiagnostic::error(message)]);
        }
        Ok(result)
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
        let host_hooks = agent.host_hooks;
        let interrupt = move || host_hooks.check_interrupt();
        let allocator = Allocator::new();
        let program = parse_program(
            &allocator,
            &source_text,
            source_type,
            typescript,
            &interrupt,
        )
        .map_err(|errors| interrupted_errors(agent, errors))?;
        let (scoping, nodes) = check_program(&program, source_type, &interrupt)
            .map_err(|errors| interrupted_errors(agent, errors))?;
        let source_code = agent.heap.create(SourceCodeHeapData { source }).unbind();
        let (scoping, nodes) = (&scoping, &nodes);
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

    /// The SourceCode that [`SourceCode::parse_source`] makes of a UTF-8
    /// source text, for a script compiled from the same text in another
    /// agent: function source spans index into the same text.
    ///
    /// The source string is kept out of the string lookup table (it is
    /// never a JavaScript value; function source texts are new strings
    /// sliced from it), so that the text is not hashed, now or whenever the
    /// table grows.
    pub(crate) fn from_source_text(
        agent: &mut Agent,
        source_text: &str,
        gc: NoGcScope<'a, '_>,
    ) -> Self {
        let source = if String::try_from(source_text).is_ok() {
            agent
                .heap
                .alloc_unlisted_str(&format!("{source_text}          "))
        } else {
            agent.heap.alloc_unlisted_str(source_text)
        };
        agent.heap.create(SourceCodeHeapData { source }).bind(gc)
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

/// The diagnostic of a parse the host's interrupt stopped.
const PARSE_INTERRUPTED: &str = "Script terminated: interrupted";

fn interrupted() -> Vec<OxcDiagnostic> {
    vec![OxcDiagnostic::error(PARSE_INTERRUPTED)]
}

/// A parse the interrupt stopped terminates the script as an interrupted
/// instruction loop does; other errors pass through.
fn interrupted_errors(agent: &mut Agent, errors: Vec<OxcDiagnostic>) -> Vec<OxcDiagnostic> {
    if errors.len() == 1 && errors[0].message == PARSE_INTERRUPTED {
        agent.terminate(TerminationReason::Interrupted);
    }
    errors
}

/// Parses a program into `allocator`; [`check_program`] checks it. Both poll
/// `interrupt` while they work and stop when it says so.
fn parse_program<'p>(
    allocator: &'p Allocator,
    source_text: &'p str,
    source_type: SourceCodeType,
    typescript: bool,
    interrupt: &'p dyn Fn() -> bool,
) -> Result<ast::Program<'p>, Vec<OxcDiagnostic>> {
    if super::source_nesting::source_nesting_too_deep(source_text) {
        return Err(vec![OxcDiagnostic::error(
            "Source text nests too deeply to be parsed",
        )]);
    }
    let source_type_in = source_type;
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
                    interrupted: sloppy_interrupted,
                    ..
                } = Parser::new(&sloppy_allocator, source_text, source_type)
                    .with_interrupt(interrupt)
                    .parse();
                if sloppy_interrupted {
                    return Err(interrupted());
                }
                if panicked {
                    return Err(sloppy_errors);
                }
                let SemanticBuilderReturn {
                    errors: mut sloppy_errors,
                    semantic: sloppy_semantic,
                    interrupted: sloppy_interrupted,
                } = SemanticBuilder::new()
                    .with_check_syntax_error(true)
                    .with_interrupt(interrupt)
                    .build(&sloppy_program);
                if sloppy_interrupted {
                    return Err(interrupted());
                }
                if matches!(source_type_in, SourceCodeType::Eval { direct: true, .. }) {
                    drop_direct_eval_context_errors(&mut sloppy_errors, &sloppy_semantic);
                }
                drop(sloppy_semantic);

                if !sloppy_errors.is_empty() {
                    return Err(sloppy_errors);
                }
                drop(sloppy_program);
                drop(sloppy_allocator);

                let source_type = SourceType::mjs().with_typescript(typescript);
                let strict_result = Parser::new(allocator, source_text, source_type)
                    .with_interrupt(interrupt)
                    .parse();
                if strict_result.interrupted {
                    return Err(interrupted());
                }
                if strict_result.panicked {
                    let errors = strict_result.errors;
                    return Err(errors);
                }

                strict_result
            } else {
                Parser::new(allocator, source_text, source_type)
                    .with_interrupt(interrupt)
                    .parse()
            }
        }
        SourceCodeType::Module => {
            let source_type = SourceType::mjs().with_typescript(typescript);
            Parser::new(allocator, source_text, source_type)
                .with_interrupt(interrupt)
                .parse()
        }
    };

    let ParserReturn {
        errors,
        program,
        interrupted: parse_interrupted,
        ..
    } = parser_result;
    if parse_interrupted {
        return Err(interrupted());
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(program)
}

/// Runs the semantic checks of a parsed program. The returned nodes borrow
/// the program, which the caller keeps alive while it uses them.
fn check_program<'p>(
    program: &'p ast::Program<'p>,
    source_type: SourceCodeType,
    interrupt: &'p dyn Fn() -> bool,
) -> Result<(Scoping, AstNodes<'p>), Vec<OxcDiagnostic>> {
    let source_type_in = source_type;
    let SemanticBuilderReturn {
        mut errors,
        semantic,
        interrupted: check_interrupted,
    } = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .with_interrupt(interrupt)
        .build(program);
    if check_interrupted {
        return Err(interrupted());
    }
    if matches!(source_type_in, SourceCodeType::Eval { direct: true, .. }) {
        drop_direct_eval_context_errors(&mut errors, &semantic);
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(semantic.into_scoping_and_nodes())
}

/// Drops the errors for `super` and `new.target` outside any function of a
/// direct eval's code: whether the code that calls eval allows them is
/// checked by PerformEval (steps 11.c to 11.f), as the parser cannot know.
fn drop_direct_eval_context_errors(errors: &mut Vec<OxcDiagnostic>, semantic: &Semantic) {
    use oxc_ast::AstKind;
    use oxc_span::GetSpan;
    let nodes = semantic.nodes();
    errors.retain(|error| {
        let message = error.message.as_ref();
        let is_super_reference = message
            == "'super' can only be referenced in members of derived classes or object literal expressions.";
        let is_super_call = message
            == "Super calls are not permitted outside constructors or in nested functions inside constructors.";
        let is_new_target = message == "Unexpected new.target expression";
        if !is_super_reference && !is_super_call && !is_new_target {
            return true;
        }
        let Some(offset) = error
            .labels
            .as_ref()
            .and_then(|labels| labels.first())
            .map(|label| label.offset())
        else {
            return true;
        };
        let node = nodes.iter().find(|node| {
            node.span().start as usize == offset
                && match node.kind() {
                    AstKind::Super(_) => is_super_reference,
                    AstKind::CallExpression(_) | AstKind::NewExpression(_) => is_super_call,
                    AstKind::MetaProperty(_) => is_new_target,
                    _ => false,
                }
        });
        let Some(node) = node else {
            return true;
        };
        // Inside a function of the eval code itself (not an arrow function,
        // which sees the caller's), the error stands.
        nodes
            .ancestor_kinds(node.id())
            .any(|kind| matches!(kind, AstKind::Function(_) | AstKind::StaticBlock(_)))
    });
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
