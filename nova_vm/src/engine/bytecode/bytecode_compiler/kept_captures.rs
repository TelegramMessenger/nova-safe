// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The variables of enclosing functions that a function reads in its loops
//! and that keep one value for as long as a call of the function lasts:
//! such a value is read from its Environment Record once in the call, and
//! from a stack slot of the function after (GetBindingAtSiteOnce).
//!
//! A read of a variable of an enclosing function goes from the running
//! Environment Record out to the one that holds the variable, five loads
//! one through the other; a loop that names an object or a function of its
//! surroundings pays that at every turn.
//!
//! The value read in a call is the variable's for the rest of the call
//! where all of this holds:
//!
//! - The function is a plain one: not a generator and not async, whose
//!   calls go on after other code has run. (The code of a Script or a
//!   Module is no function.)
//! - The variable is declared outside the function. One declared in a block
//!   of the function itself, and kept in an Environment Record because a
//!   closure names it, is another variable in every turn of a loop around
//!   the block, and one place in the code reads several of them.
//! - Nothing assigns to the variable: no reference to it writes it, and no
//!   direct `eval` is anywhere in the scope that declares it, whose code
//!   could. (A `with`, or a direct `eval`, between a reference and the
//!   declaration makes that reference a lookup by name, which keeps
//!   nothing: see `scope_chain_is_fixed`.)
//! - The declaration gives the variable its value once, or only while no
//!   call of the function can be going on. A `let`, a `const` or a class is
//!   initialized once for its Environment Record, and cannot be read before
//!   (such a read throws, and keeps nothing). A `var`, a function
//!   declaration and a parameter are given values by the code of the
//!   function that declares them: at its start, at a declarator with an
//!   initializer, which a loop can run again, at a function declaration in
//!   a block. While that function is a plain one none of this happens
//!   during a call of the function that reads the variable: the declaring
//!   call is then either waiting for a call it made to return or has
//!   returned. A generator or an async function that declares the variable
//!   can be resumed from inside the reader, so its `var`s keep nothing.
//!   (A parameter is written by nothing but a reference only because the
//!   arguments objects of this engine are all unmapped ones: a mapped
//!   arguments object would write it, and parameters would have to be
//!   left out here.)
//!
//! What is kept is never `undefined`, which is what the slot holds while
//! nothing is kept: a `var` read before its initializer has run, and a
//! variable whose value is `undefined`, are read from the Environment
//! Record each time.

use oxc_ast::{AstKind, ast};
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::{ReferenceId, ScopeFlags, ScopeId, SymbolFlags, SymbolId};

use super::compile_context::CompileContext;
use crate::ecmascript::FunctionAstRef;

/// The most variables one function keeps: each is a stack slot that every
/// call of the function fills with undefined when it starts.
const MOST_KEPT: usize = 8;

/// The variables that `function` keeps the values of, in the order its
/// loops name them.
pub(super) fn kept_captures<'s>(
    ctx: &CompileContext<'_, 's, '_, '_>,
    function: FunctionAstRef<'s>,
) -> Vec<SymbolId> {
    let mut kept = Vec::new();
    if function.is_generator() || function.is_async() {
        return kept;
    }
    let mut finder = LoopReferences {
        loops: 0,
        found: Vec::new(),
    };
    finder.visit_function_body(function.ecmascript_code());
    let scoping = ctx.scoping();
    let function_scope = function.scope_id();
    for reference in finder.found {
        if kept.len() == MOST_KEPT {
            break;
        }
        let Some(symbol) = scoping.get_reference(reference).symbol_id() else {
            continue;
        };
        if !kept.contains(&symbol) && keeps_one_value_in_a_call(ctx, symbol, function_scope) {
            kept.push(symbol);
        }
    }
    kept
}

/// Whether `symbol`, read from the function whose scope is `function_scope`,
/// has one value for as long as a call of that function lasts: see the
/// module.
fn keeps_one_value_in_a_call(
    ctx: &CompileContext,
    symbol: SymbolId,
    function_scope: ScopeId,
) -> bool {
    let scoping = ctx.scoping();
    let declared_in = scoping.symbol_scope_id(symbol);
    // Declared outside the function, with no `with` and no direct `eval`
    // on the way out to the declaration from the function's own scope.
    let mut scope = function_scope;
    loop {
        let flags = scoping.scope_flags(scope);
        if flags.contains_direct_eval() || flags.is_with() {
            return false;
        }
        let Some(parent) = scoping.scope_parent_id(scope) else {
            return false;
        };
        scope = parent;
        if scope == declared_in {
            break;
        }
    }
    let declared_flags = scoping.scope_flags(declared_in);
    // A direct `eval` anywhere in the declaring scope can assign to the
    // variable. The top scope of a Script holds no variable of this kind
    // (its names are the global scope's), and a Module's are resolved by
    // name.
    if declared_flags.contains_direct_eval() || declared_flags.contains(ScopeFlags::Top) {
        return false;
    }
    if scoping
        .get_resolved_references(symbol)
        .any(|reference| reference.is_write())
    {
        return false;
    }
    let flags = scoping.symbol_flags(symbol);
    if !flags.contains(SymbolFlags::Function)
        && (flags.intersects(SymbolFlags::BlockScopedVariable) || flags.is_class())
    {
        // A `let`, a `const`, a class: initialized once.
        return true;
    }
    // A `var`, a function declaration, a parameter (and whatever else there
    // is): the function that declares it has to be a plain one, and a
    // function declaration has to be one of that function's own scope, not
    // of a block in it.
    if flags.contains(SymbolFlags::Function) && !declared_flags.contains(ScopeFlags::Function) {
        return false;
    }
    let mut scope = declared_in;
    loop {
        let flags = scoping.scope_flags(scope);
        if flags.contains(ScopeFlags::Function) {
            return match ctx.nodes().kind(scoping.get_node_id(scope)) {
                AstKind::Function(function) => !function.r#async && !function.generator,
                AstKind::ArrowFunctionExpression(arrow) => !arrow.r#async,
                _ => false,
            };
        }
        let Some(parent) = scoping.scope_parent_id(scope) else {
            // The code of a Script or a Module.
            return false;
        };
        scope = parent;
    }
}

/// Finds the references to names that the code of one function makes
/// inside its loops, not counting the code of nested functions and classes.
struct LoopReferences {
    loops: u32,
    found: Vec<ReferenceId>,
}

impl<'a> Visit<'a> for LoopReferences {
    fn visit_function(&mut self, _: &'a ast::Function<'a>, _: ScopeFlags) {
        // Another function's code.
    }

    fn visit_arrow_function_expression(&mut self, _: &'a ast::ArrowFunctionExpression<'a>) {
        // Another function's code.
    }

    fn visit_class(&mut self, _: &'a ast::Class<'a>) {
        // Its methods, field values and static blocks are other functions'
        // code; the few computed keys are not worth the walk.
    }

    fn visit_for_statement(&mut self, it: &'a ast::ForStatement<'a>) {
        self.loops += 1;
        walk::walk_for_statement(self, it);
        self.loops -= 1;
    }

    fn visit_for_in_statement(&mut self, it: &'a ast::ForInStatement<'a>) {
        self.loops += 1;
        walk::walk_for_in_statement(self, it);
        self.loops -= 1;
    }

    fn visit_for_of_statement(&mut self, it: &'a ast::ForOfStatement<'a>) {
        self.loops += 1;
        walk::walk_for_of_statement(self, it);
        self.loops -= 1;
    }

    fn visit_while_statement(&mut self, it: &'a ast::WhileStatement<'a>) {
        self.loops += 1;
        walk::walk_while_statement(self, it);
        self.loops -= 1;
    }

    fn visit_do_while_statement(&mut self, it: &'a ast::DoWhileStatement<'a>) {
        self.loops += 1;
        walk::walk_do_while_statement(self, it);
        self.loops -= 1;
    }

    fn visit_identifier_reference(&mut self, it: &'a ast::IdentifierReference<'a>) {
        if self.loops != 0
            && let Some(reference) = it.reference_id.get()
        {
            self.found.push(reference);
        }
    }
}
