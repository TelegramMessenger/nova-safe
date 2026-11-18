// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What a function's code needs from its Function Environment Record besides
//! its bindings. The walk uses oxc's generated visitor, which reaches every
//! child of every node, so that no position (a computed key in a pattern, a
//! rest element, a parameter default, ...) can be missed.

use oxc_ast::ast;
use oxc_ast_visit::{Visit, walk};
use oxc_semantic::ScopeFlags;

use crate::ecmascript::FunctionAstRef;

/// What the code of a function uses of its Function Environment Record, not
/// counting nested non-arrow functions and the values of class fields and
/// static blocks, which have environments of their own.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct FunctionEnvironmentUse {
    /// The function refers to new.target or super, or an arrow function in
    /// it (directly or through other arrows) refers to this, new.target or
    /// super: these find the Function Environment Record through
    /// GetThisEnvironment.
    pub(super) this_environment: bool,
    /// The function or an arrow function in it refers to `arguments`, or
    /// calls a function named eval.
    pub(super) arguments: bool,
    /// `arguments` is looked for by name from code that is not the
    /// function's own: an arrow function in it refers to it, or a function
    /// named eval is called. The arguments object must then be a binding of
    /// the function's Environment Record, where a name is found.
    pub(super) arguments_by_name: bool,
}

impl FunctionEnvironmentUse {
    pub(super) fn of<'s>(func: FunctionAstRef<'s>) -> Self {
        let mut finder = Finder {
            arrow_depth: 0,
            found: Self::default(),
        };
        finder.visit_formal_parameters(func.formal_parameters());
        finder.visit_function_body(func.ecmascript_code());
        finder.found
    }

    /// Whether an expression among `formals` makes a function: a function
    /// expression, an arrow function or a class, each of which closes over
    /// the Environment Record that the parameters are in.
    pub(super) fn makes_functions<'s>(formals: &'s ast::FormalParameters<'s>) -> bool {
        let mut finder = FunctionFinder { found: false };
        finder.visit_formal_parameters(formals);
        finder.found
    }
}

struct FunctionFinder {
    found: bool,
}

impl<'a> Visit<'a> for FunctionFinder {
    fn visit_function(&mut self, _: &'a ast::Function<'a>, _: ScopeFlags) {
        self.found = true;
    }

    fn visit_arrow_function_expression(&mut self, _: &'a ast::ArrowFunctionExpression<'a>) {
        self.found = true;
    }

    fn visit_class(&mut self, _: &'a ast::Class<'a>) {
        self.found = true;
    }
}

struct Finder {
    arrow_depth: u32,
    found: FunctionEnvironmentUse,
}

impl<'a> Visit<'a> for Finder {
    fn visit_function(&mut self, _: &'a ast::Function<'a>, _: ScopeFlags) {
        // A nested function has its own Function Environment Record.
    }

    fn visit_arrow_function_expression(&mut self, it: &'a ast::ArrowFunctionExpression<'a>) {
        self.arrow_depth += 1;
        walk::walk_arrow_function_expression(self, it);
        self.arrow_depth -= 1;
    }

    fn visit_property_definition(&mut self, it: &'a ast::PropertyDefinition<'a>) {
        // A field's initializer runs as a method of the class with its own
        // environment; its decorators and computed key run here.
        self.visit_decorators(&it.decorators);
        self.visit_property_key(&it.key);
    }

    fn visit_accessor_property(&mut self, it: &'a ast::AccessorProperty<'a>) {
        self.visit_decorators(&it.decorators);
        self.visit_property_key(&it.key);
    }

    fn visit_static_block(&mut self, _: &'a ast::StaticBlock<'a>) {
        // A static block runs in a Function Environment Record of its own.
    }

    fn visit_this_expression(&mut self, _: &'a ast::ThisExpression) {
        if self.arrow_depth > 0 {
            self.found.this_environment = true;
        }
    }

    fn visit_meta_property(&mut self, it: &'a ast::MetaProperty<'a>) {
        if it.meta.name == "new" && it.property.name == "target" {
            self.found.this_environment = true;
        }
    }

    fn visit_super(&mut self, _: &'a ast::Super) {
        self.found.this_environment = true;
    }

    fn visit_identifier_reference(&mut self, it: &'a ast::IdentifierReference<'a>) {
        if it.name == "arguments" {
            self.found.arguments = true;
            if self.arrow_depth > 0 {
                self.found.arguments_by_name = true;
            }
        }
    }

    fn visit_call_expression(&mut self, it: &'a ast::CallExpression<'a>) {
        if it
            .callee
            .get_identifier_reference()
            .is_some_and(|id| id.name == "eval")
        {
            // eval may always refer to arguments.
            self.found.arguments = true;
            self.found.arguments_by_name = true;
        }
        walk::walk_call_expression(self, it);
    }
}
