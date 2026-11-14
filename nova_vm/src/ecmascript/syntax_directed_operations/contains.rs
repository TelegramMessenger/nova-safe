// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ## [8.5 Contains](https://tc39.es/ecma262/#sec-static-semantics-contains)

use oxc_ast::ast;
use oxc_span::Ident;

use crate::ecmascript::builtins::FunctionAstRef;

/// `Contains` of the `Expression` variants of `$ty`, an enum that inherits them, without a
/// cast between the enums.
macro_rules! expression_variants_contain {
    ($ty:ident, $node:expr, $symbol:expr) => {
        match $node {
            ast::$ty::BooleanLiteral(_)
            | ast::$ty::NullLiteral(_)
            | ast::$ty::NumericLiteral(_)
            | ast::$ty::BigIntLiteral(_)
            | ast::$ty::RegExpLiteral(_)
            | ast::$ty::StringLiteral(_) => false,
            ast::$ty::TemplateLiteral(e) => e.contains($symbol),
            ast::$ty::Identifier(i) => {
                $symbol == ContainsSymbol::Arguments && i.name == Ident::new_const("arguments")
            }
            ast::$ty::MetaProperty(e) => e.contains($symbol),
            ast::$ty::Super(e) => e.contains($symbol),
            ast::$ty::ArrayExpression(e) => e.contains($symbol),
            ast::$ty::ArrowFunctionExpression(f) if $symbol == ContainsSymbol::Arguments => {
                f.contains($symbol)
            }
            ast::$ty::ArrowFunctionExpression(e) => e.contains($symbol),
            ast::$ty::AssignmentExpression(e) => e.contains($symbol),
            ast::$ty::AwaitExpression(e) => e.contains($symbol),
            ast::$ty::BinaryExpression(e) => e.contains($symbol),
            ast::$ty::CallExpression(e) => e.contains($symbol),
            ast::$ty::ChainExpression(e) => e.contains($symbol),
            ast::$ty::ClassExpression(e) => e.contains($symbol),
            ast::$ty::ConditionalExpression(e) => e.contains($symbol),
            ast::$ty::FunctionExpression(e) => e.contains($symbol),
            ast::$ty::ImportExpression(e) => e.contains($symbol),
            ast::$ty::LogicalExpression(e) => e.contains($symbol),
            ast::$ty::NewExpression(e) => e.contains($symbol),
            ast::$ty::ObjectExpression(e) => e.contains($symbol),
            ast::$ty::ParenthesizedExpression(e) => e.contains($symbol),
            ast::$ty::SequenceExpression(e) => e.contains($symbol),
            ast::$ty::TaggedTemplateExpression(e) => e.contains($symbol),
            ast::$ty::ThisExpression(e) => e.contains($symbol),
            ast::$ty::UnaryExpression(e) => e.contains($symbol),
            ast::$ty::UpdateExpression(e) => e.contains($symbol),
            ast::$ty::YieldExpression(e) => e.contains($symbol),
            ast::$ty::PrivateInExpression(e) => e.contains($symbol),
            ast::$ty::JSXElement(_) | ast::$ty::JSXFragment(_) => unreachable!(),
            ast::$ty::TSAsExpression(e) => e.expression.contains($symbol),
            ast::$ty::TSSatisfiesExpression(e) => e.expression.contains($symbol),
            ast::$ty::TSTypeAssertion(e) => e.expression.contains($symbol),
            ast::$ty::TSNonNullExpression(e) => e.expression.contains($symbol),
            ast::$ty::TSInstantiationExpression(e) => e.expression.contains($symbol),
            ast::$ty::V8IntrinsicExpression(e) => {
                e.arguments.iter().any(|e| e.contains($symbol))
            }
            ast::$ty::ComputedMemberExpression(e) => e.contains($symbol),
            ast::$ty::StaticMemberExpression(e) => e.contains($symbol),
            ast::$ty::PrivateFieldExpression(e) => e.contains($symbol),
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContainsSymbol {
    Arguments,
    Await,
    NewTarget,
    SuperCall,
    SuperProperty,
    This,
    Yield,
}

/// ## [8.5 Contains](https://tc39.es/ecma262/#sec-static-semantics-contains)
///
/// The syntax-directed operation ComputedPropertyContains takes argument
/// symbol (a grammar symbol) and returns a Boolean.
pub(crate) trait Contains {
    /// ## [8.5 Contains](https://tc39.es/ecma262/#sec-static-semantics-contains)
    fn contains(&self, symbol: ContainsSymbol) -> bool;
}

impl Contains for [ast::Statement<'_>] {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.iter().any(|st| st.contains(symbol))
    }
}

impl Contains for ast::Program<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.body.iter().any(|st| st.contains(symbol))
    }
}

impl Contains for ast::Function<'_> {
    fn contains(&self, _symbol: ContainsSymbol) -> bool {
        false
    }
}

impl Contains for FunctionAstRef<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        let (params, body) = match self {
            FunctionAstRef::Function(f)
            | FunctionAstRef::AsyncFunction(f)
            | FunctionAstRef::Generator(f)
            | FunctionAstRef::AsyncGenerator(f)
            | FunctionAstRef::ClassConstructor(f) => (
                f.params.as_ref(),
                f.body
                    .as_deref()
                    .expect("FunctionAstRef cannot point to a TypeScript function declaration"),
            ),
            FunctionAstRef::Arrow(f) | FunctionAstRef::AsyncArrow(f) => {
                (f.params.as_ref(), &*f.body)
            }
        };
        params.items.iter().any(|p| p.contains(symbol))
            || body.statements.iter().any(|st| st.contains(symbol))
    }
}

impl Contains for ast::Class<'_> {
    /// ClassTail : ClassHeritageopt { ClassBody }
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        // 1. If symbol is ClassBody, return true.
        // 2. If symbol is ClassHeritage, then
        if let Some(heritage) = &self.super_class {
            // a. If ClassHeritage is present, return true; otherwise return
            //    false.
            // 3. If ClassHeritage is present, then
            // a. If ClassHeritage Contains symbol is true, return true.
            if heritage.contains(symbol) {
                return true;
            }
        }
        // 4. Return the result of ComputedPropertyContains of ClassBody with
        //    argument symbol.
        self.body.computed_property_contains(symbol)

        // Note 2

        // Static semantic rules that depend upon substructure generally do not
        // look into class bodies except for PropertyNames.
    }
}

impl Contains for ast::ArrowFunctionExpression<'_> {
    /// ArrowFunction : ArrowParameters => ConciseBody
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        // 1. If symbol is not one of NewTarget, SuperProperty, SuperCall, super, or this,
        if !matches!(
            symbol,
            ContainsSymbol::Arguments
                | ContainsSymbol::NewTarget
                | ContainsSymbol::SuperCall
                | ContainsSymbol::This
        ) {
            // return false.
            return false;
        }
        // 2. If ArrowParameters Contains symbol is true, return true.
        if self.params.items.iter().any(|p| p.contains(symbol)) {
            return true;
        }
        // 3. Return ConciseBody Contains symbol.
        self.body.statements.iter().any(|st| st.contains(symbol))
    }
}

impl Contains for ast::FormalParameter<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.pattern.contains(symbol)
            || self
                .initializer
                .as_ref()
                .is_some_and(|init| init.contains(symbol))
    }
}

impl Contains for ast::BindingPattern<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::BindingPattern::BindingIdentifier(_) => false,
            ast::BindingPattern::ObjectPattern(e) => {
                e.properties
                    .iter()
                    .any(|p| p.key.contains(symbol) || p.value.contains(symbol))
                    || e.rest.as_ref().is_some_and(|e| e.argument.contains(symbol))
            }
            ast::BindingPattern::ArrayPattern(e) => e
                .elements
                .iter()
                .any(|e| e.as_ref().is_some_and(|e| e.contains(symbol))),
            ast::BindingPattern::AssignmentPattern(e) => {
                e.left.contains(symbol) || e.right.contains(symbol)
            }
        }
    }
}

impl Contains for ast::ParenthesizedExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.expression.contains(symbol)
    }
}

impl Contains for ast::SequenceExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.expressions.iter().any(|e| e.contains(symbol))
    }
}

impl Contains for ast::MethodDefinition<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.computed_property_contains(symbol)
    }
}

impl Contains for ast::StaticMemberExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        if symbol == ContainsSymbol::SuperProperty && self.object.is_super() {
            true
        } else {
            self.object.contains(symbol)
        }
    }
}

impl Contains for ast::Super {
    fn contains(&self, _symbol: ContainsSymbol) -> bool {
        // `super` alone is the SuperCall or SuperProperty around it.
        false
    }
}

impl Contains for ast::CallExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        symbol == ContainsSymbol::SuperCall && self.callee.is_super()
            || self.callee.contains(symbol)
            || self.arguments.iter().any(|arg| arg.contains(symbol))
            || symbol == ContainsSymbol::Arguments
                && self
                    .callee
                    .get_identifier_reference()
                    // eval may always cause an "arguments" reference :(
                    .is_some_and(|id| id.name.as_str() == "eval")
    }
}

impl Contains for ast::ChainExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.expression.contains(symbol)
    }
}

impl Contains for ast::ChainElement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::ChainElement::CallExpression(e) => e.contains(symbol),
            ast::ChainElement::TSNonNullExpression(e) => e.expression.contains(symbol),
            ast::ChainElement::ComputedMemberExpression(e) => e.contains(symbol),
            ast::ChainElement::StaticMemberExpression(e) => e.contains(symbol),
            ast::ChainElement::PrivateFieldExpression(e) => e.contains(symbol),
        }
    }
}

impl Contains for ast::TemplateLiteral<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.expressions.iter().any(|e| e.contains(symbol))
    }
}

impl Contains for ast::MetaProperty<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        symbol == ContainsSymbol::NewTarget
            && self.meta.name == "new"
            && self.property.name == "target"
    }
}

impl Contains for ast::ArrayExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.elements.iter().any(|e| e.contains(symbol))
    }
}

impl Contains for ast::ArrayExpressionElement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::ArrayExpressionElement::SpreadElement(e) => e.argument.contains(symbol),
            ast::ArrayExpressionElement::Elision(_) => false,
            _ => expression_variants_contain!(ArrayExpressionElement, self, symbol),
        }
    }
}

impl Contains for ast::AssignmentExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.left.contains(symbol) || self.right.contains(symbol)
    }
}

impl Contains for ast::AssignmentTarget<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::AssignmentTarget::AssignmentTargetIdentifier(_) => false,
            ast::AssignmentTarget::TSAsExpression(e) => e.expression.contains(symbol),
            ast::AssignmentTarget::TSSatisfiesExpression(e) => e.expression.contains(symbol),
            ast::AssignmentTarget::TSNonNullExpression(e) => e.expression.contains(symbol),
            ast::AssignmentTarget::TSTypeAssertion(e) => e.expression.contains(symbol),
            ast::AssignmentTarget::ComputedMemberExpression(e) => e.contains(symbol),
            ast::AssignmentTarget::StaticMemberExpression(e) => e.contains(symbol),
            ast::AssignmentTarget::PrivateFieldExpression(e) => e.contains(symbol),
            ast::AssignmentTarget::ArrayAssignmentTarget(e) => e.contains(symbol),
            ast::AssignmentTarget::ObjectAssignmentTarget(e) => e.contains(symbol),
        }
    }
}

impl Contains for ast::SimpleAssignmentTarget<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(_) => false,
            ast::SimpleAssignmentTarget::TSAsExpression(e) => e.expression.contains(symbol),
            ast::SimpleAssignmentTarget::TSSatisfiesExpression(e) => e.expression.contains(symbol),
            ast::SimpleAssignmentTarget::TSNonNullExpression(e) => e.expression.contains(symbol),
            ast::SimpleAssignmentTarget::TSTypeAssertion(e) => e.expression.contains(symbol),
            ast::SimpleAssignmentTarget::ComputedMemberExpression(e) => e.contains(symbol),
            ast::SimpleAssignmentTarget::StaticMemberExpression(e) => e.contains(symbol),
            ast::SimpleAssignmentTarget::PrivateFieldExpression(e) => e.contains(symbol),
        }
    }
}

impl Contains for ast::MemberExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::MemberExpression::ComputedMemberExpression(e) => e.contains(symbol),
            ast::MemberExpression::StaticMemberExpression(e) => e.contains(symbol),
            ast::MemberExpression::PrivateFieldExpression(e) => e.contains(symbol),
        }
    }
}

impl Contains for ast::AssignmentTargetPattern<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::AssignmentTargetPattern::ArrayAssignmentTarget(e) => e.contains(symbol),
            ast::AssignmentTargetPattern::ObjectAssignmentTarget(e) => e.contains(symbol),
        }
    }
}

impl Contains for ast::ArrayAssignmentTarget<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.elements.iter().any(|e| {
            let Some(e) = e else {
                return false;
            };
            e.contains(symbol)
        }) || self
            .rest
            .as_ref()
            .is_some_and(|e| e.target.contains(symbol))
    }
}

impl Contains for ast::AssignmentTargetMaybeDefault<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(e) => e.contains(symbol),
            ast::AssignmentTargetMaybeDefault::AssignmentTargetIdentifier(_) => false,
            ast::AssignmentTargetMaybeDefault::TSAsExpression(e) => e.expression.contains(symbol),
            ast::AssignmentTargetMaybeDefault::TSSatisfiesExpression(e) => {
                e.expression.contains(symbol)
            }
            ast::AssignmentTargetMaybeDefault::TSNonNullExpression(e) => {
                e.expression.contains(symbol)
            }
            ast::AssignmentTargetMaybeDefault::TSTypeAssertion(e) => e.expression.contains(symbol),
            ast::AssignmentTargetMaybeDefault::ComputedMemberExpression(e) => e.contains(symbol),
            ast::AssignmentTargetMaybeDefault::StaticMemberExpression(e) => e.contains(symbol),
            ast::AssignmentTargetMaybeDefault::PrivateFieldExpression(e) => e.contains(symbol),
            ast::AssignmentTargetMaybeDefault::ArrayAssignmentTarget(e) => e.contains(symbol),
            ast::AssignmentTargetMaybeDefault::ObjectAssignmentTarget(e) => e.contains(symbol),
        }
    }
}

impl Contains for ast::AssignmentTargetWithDefault<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.binding.contains(symbol) || self.init.contains(symbol)
    }
}

impl Contains for ast::ObjectAssignmentTarget<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.properties.iter().any(|e| match e {
            ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(e) => {
                e.init.as_ref().is_some_and(|e| e.contains(symbol))
            }
            ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(e) => {
                e.contains(symbol)
            }
        })
    }
}

impl Contains for ast::AssignmentTargetPropertyProperty<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.binding.contains(symbol)
    }
}

impl Contains for ast::AwaitExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        if symbol == ContainsSymbol::Await {
            return true;
        }
        self.argument.contains(symbol)
    }
}

impl Contains for ast::BinaryExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.left.contains(symbol) || self.right.contains(symbol)
    }
}

impl Contains for ast::ConditionalExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.alternate.contains(symbol)
            || self.consequent.contains(symbol)
            || self.test.contains(symbol)
    }
}

impl Contains for ast::ImportExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.options.as_ref().is_some_and(|e| e.contains(symbol)) || self.source.contains(symbol)
    }
}

impl Contains for ast::LogicalExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.left.contains(symbol) || self.right.contains(symbol)
    }
}

impl Contains for ast::NewExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.callee.contains(symbol) || self.arguments.iter().any(|arg| arg.contains(symbol))
    }
}

impl Contains for ast::Argument<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::Argument::SpreadElement(e) => e.argument.contains(symbol),
            _ => expression_variants_contain!(Argument, self, symbol),
        }
    }
}

impl Contains for ast::ObjectExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.properties.iter().any(|p| match p {
            ast::ObjectPropertyKind::ObjectProperty(p) => {
                p.key.contains(symbol) || p.value.contains(symbol)
            }
            ast::ObjectPropertyKind::SpreadProperty(p) => p.argument.contains(symbol),
        })
    }
}

impl Contains for ast::PropertyKey<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::PropertyKey::StaticIdentifier(_) | ast::PropertyKey::PrivateIdentifier(_) => false,
            _ => expression_variants_contain!(PropertyKey, self, symbol),
        }
    }
}

impl Contains for ast::TaggedTemplateExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.tag.contains(symbol) || self.quasi.contains(symbol)
    }
}

impl Contains for ast::ThisExpression {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        symbol == ContainsSymbol::This
    }
}

impl Contains for ast::UnaryExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.argument.contains(symbol)
    }
}

impl Contains for ast::UpdateExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.argument.contains(symbol)
    }
}

impl Contains for ast::YieldExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        if symbol == ContainsSymbol::Yield {
            return true;
        }
        self.argument.as_ref().is_some_and(|e| e.contains(symbol))
    }
}

impl Contains for ast::PrivateInExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.right.contains(symbol)
    }
}

impl Contains for ast::V8IntrinsicExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.arguments.iter().any(|e| e.contains(symbol))
    }
}

impl Contains for ast::ComputedMemberExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        if symbol == ContainsSymbol::SuperProperty && self.object.is_super() {
            true
        } else {
            self.object.contains(symbol) || self.expression.contains(symbol)
        }
    }
}

impl Contains for ast::PrivateFieldExpression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        if symbol == ContainsSymbol::SuperProperty && self.object.is_super() {
            true
        } else {
            self.object.contains(symbol)
        }
    }
}

impl Contains for ast::Expression<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        expression_variants_contain!(Expression, self, symbol)
    }
}

impl Contains for ast::BlockStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.body.iter().any(|st| st.contains(symbol))
    }
}

impl Contains for ast::DoWhileStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.test.contains(symbol) || self.body.contains(symbol)
    }
}

impl Contains for ast::ForInStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.right.contains(symbol) || self.left.contains(symbol) || self.body.contains(symbol)
    }
}

impl Contains for ast::ForStatementLeft<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::ForStatementLeft::VariableDeclaration(e) => e.contains(symbol),
            ast::ForStatementLeft::AssignmentTargetIdentifier(_) => false,
            ast::ForStatementLeft::TSAsExpression(e) => e.expression.contains(symbol),
            ast::ForStatementLeft::TSSatisfiesExpression(e) => e.expression.contains(symbol),
            ast::ForStatementLeft::TSNonNullExpression(e) => e.expression.contains(symbol),
            ast::ForStatementLeft::TSTypeAssertion(e) => e.expression.contains(symbol),
            ast::ForStatementLeft::ComputedMemberExpression(e) => e.contains(symbol),
            ast::ForStatementLeft::StaticMemberExpression(e) => e.contains(symbol),
            ast::ForStatementLeft::PrivateFieldExpression(e) => e.contains(symbol),
            ast::ForStatementLeft::ArrayAssignmentTarget(e) => e.contains(symbol),
            ast::ForStatementLeft::ObjectAssignmentTarget(e) => e.contains(symbol),
        }
    }
}

impl Contains for ast::ForOfStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.right.contains(symbol) || self.left.contains(symbol) || self.body.contains(symbol)
    }
}

impl Contains for ast::ForStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.init.as_ref().is_some_and(|e| e.contains(symbol))
            || self.test.as_ref().is_some_and(|e| e.contains(symbol))
            || self.update.as_ref().is_some_and(|e| e.contains(symbol))
            || self.body.contains(symbol)
    }
}

impl Contains for ast::ForStatementInit<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::ForStatementInit::VariableDeclaration(e) => e.contains(symbol),
            _ => expression_variants_contain!(ForStatementInit, self, symbol),
        }
    }
}

impl Contains for ast::IfStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.test.contains(symbol)
            || self.consequent.contains(symbol)
            || self
                .alternate
                .as_ref()
                .is_some_and(|st| st.contains(symbol))
    }
}

impl Contains for ast::LabeledStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.body.contains(symbol)
    }
}

impl Contains for ast::ReturnStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.argument.as_ref().is_some_and(|e| e.contains(symbol))
    }
}

impl Contains for ast::SwitchStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.discriminant.contains(symbol) || self.cases.iter().any(|e| e.contains(symbol))
    }
}

impl Contains for ast::SwitchCase<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.test.as_ref().is_some_and(|e| e.contains(symbol))
            || self.consequent.iter().any(|st| st.contains(symbol))
    }
}

impl Contains for ast::ThrowStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.argument.contains(symbol)
    }
}

impl Contains for ast::TryStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.block.contains(symbol)
            || self.handler.as_ref().is_some_and(|e| e.contains(symbol))
            || self
                .finalizer
                .as_ref()
                .is_some_and(|st| st.contains(symbol))
    }
}

impl Contains for ast::CatchClause<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.param
            .as_ref()
            .is_some_and(|e| e.pattern.contains(symbol))
            || self.body.contains(symbol)
    }
}

impl Contains for ast::WhileStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.test.contains(symbol) || self.body.contains(symbol)
    }
}

impl Contains for ast::WithStatement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.object.contains(symbol) || self.body.contains(symbol)
    }
}

impl Contains for ast::VariableDeclaration<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.declarations.iter().any(|e| e.contains(symbol))
    }
}

impl Contains for ast::VariableDeclarator<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.init.as_ref().is_some_and(|e| e.contains(symbol)) || self.id.contains(symbol)
    }
}

impl Contains for ast::TSEnumDeclaration<'_> {
    fn contains(&self, _symbol: ContainsSymbol) -> bool {
        unreachable!()
    }
}

impl Contains for ast::TSModuleDeclaration<'_> {
    fn contains(&self, _symbol: ContainsSymbol) -> bool {
        unreachable!()
    }
}

impl Contains for ast::TSImportEqualsDeclaration<'_> {
    fn contains(&self, _symbol: ContainsSymbol) -> bool {
        unreachable!()
    }
}

impl Contains for ast::ImportDeclaration<'_> {
    fn contains(&self, _symbol: ContainsSymbol) -> bool {
        false
    }
}

impl Contains for ast::ExportAllDeclaration<'_> {
    fn contains(&self, _symbol: ContainsSymbol) -> bool {
        false
    }
}

impl Contains for ast::ExportDefaultDeclaration<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match &self.declaration {
            ast::ExportDefaultDeclarationKind::FunctionDeclaration(f) => f.contains(symbol),
            ast::ExportDefaultDeclarationKind::ClassDeclaration(c) => c.contains(symbol),
            ast::ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => false,
            _ => expression_variants_contain!(ExportDefaultDeclarationKind, &self.declaration, symbol),
        }
    }
}

impl Contains for ast::ExportNamedDeclaration<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.declaration.as_ref().is_some_and(|e| match e {
            ast::Declaration::VariableDeclaration(e) => e.contains(symbol),
            ast::Declaration::FunctionDeclaration(f) => f.contains(symbol),
            ast::Declaration::ClassDeclaration(c) => c.contains(symbol),
            ast::Declaration::TSTypeAliasDeclaration(_)
            | ast::Declaration::TSInterfaceDeclaration(_)
            | ast::Declaration::TSGlobalDeclaration(_) => false,
            ast::Declaration::TSEnumDeclaration(e) => e.contains(symbol),
            ast::Declaration::TSModuleDeclaration(e) => e.contains(symbol),
            ast::Declaration::TSImportEqualsDeclaration(e) => e.contains(symbol),
        })
    }
}

impl Contains for ast::TSExportAssignment<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        self.expression.contains(symbol)
    }
}

impl Contains for ast::TSNamespaceExportDeclaration<'_> {
    fn contains(&self, _symbol: ContainsSymbol) -> bool {
        false
    }
}

impl Contains for ast::Statement<'_> {
    fn contains(&self, symbol: ContainsSymbol) -> bool {
        match self {
            ast::Statement::BlockStatement(st) => st.contains(symbol),
            ast::Statement::BreakStatement(_)
            | ast::Statement::ContinueStatement(_)
            | ast::Statement::DebuggerStatement(_) => false,
            ast::Statement::DoWhileStatement(st) => st.contains(symbol),
            ast::Statement::EmptyStatement(_) => false,
            ast::Statement::ExpressionStatement(st) => st.expression.contains(symbol),
            ast::Statement::ForInStatement(st) => st.contains(symbol),
            ast::Statement::ForOfStatement(st) => st.contains(symbol),
            ast::Statement::ForStatement(st) => st.contains(symbol),
            ast::Statement::IfStatement(st) => st.contains(symbol),
            ast::Statement::LabeledStatement(st) => st.contains(symbol),
            ast::Statement::ReturnStatement(st) => st.contains(symbol),
            ast::Statement::SwitchStatement(st) => st.contains(symbol),
            ast::Statement::ThrowStatement(st) => st.contains(symbol),
            ast::Statement::TryStatement(st) => st.contains(symbol),
            ast::Statement::WhileStatement(st) => st.contains(symbol),
            ast::Statement::WithStatement(st) => st.contains(symbol),
            ast::Statement::VariableDeclaration(st) => st.contains(symbol),
            ast::Statement::FunctionDeclaration(st) => st.contains(symbol),
            ast::Statement::ClassDeclaration(st) => st.contains(symbol),
            ast::Statement::TSTypeAliasDeclaration(_)
            | ast::Statement::TSInterfaceDeclaration(_)
            | ast::Statement::TSGlobalDeclaration(_) => false,
            ast::Statement::TSEnumDeclaration(st) => st.contains(symbol),
            ast::Statement::TSModuleDeclaration(st) => st.contains(symbol),
            ast::Statement::TSImportEqualsDeclaration(st) => st.contains(symbol),
            ast::Statement::ImportDeclaration(st) => st.contains(symbol),
            ast::Statement::ExportAllDeclaration(st) => st.contains(symbol),
            ast::Statement::ExportDefaultDeclaration(st) => st.contains(symbol),
            ast::Statement::ExportNamedDeclaration(st) => st.contains(symbol),
            ast::Statement::TSExportAssignment(st) => st.contains(symbol),
            ast::Statement::TSNamespaceExportDeclaration(st) => st.contains(symbol),
        }
    }
}

/// ### [8.5.2 Static Semantics: ComputedPropertyContains](https://tc39.es/ecma262/#sec-static-semantics-computedpropertycontains)
///
/// The syntax-directed operation ComputedPropertyContains takes argument
/// symbol (a grammar symbol) and returns a Boolean.
pub(crate) trait ComputedPropertyContains {
    /// ### [8.5.2 Static Semantics: ComputedPropertyContains](https://tc39.es/ecma262/#sec-static-semantics-computedpropertycontains)
    fn computed_property_contains(&self, symbol: ContainsSymbol) -> bool;
}

impl ComputedPropertyContains for ast::ClassBody<'_> {
    fn computed_property_contains(&self, symbol: ContainsSymbol) -> bool {
        self.body.iter().any(|e| match e {
            ast::ClassElement::StaticBlock(_) => false,
            ast::ClassElement::MethodDefinition(e) => e.computed_property_contains(symbol),
            ast::ClassElement::PropertyDefinition(p) => p.key.contains(symbol),
            ast::ClassElement::AccessorProperty(p) => p.key.contains(symbol),
            ast::ClassElement::TSIndexSignature(_) => false,
        })
    }
}

impl ComputedPropertyContains for ast::MethodDefinition<'_> {
    fn computed_property_contains(&self, symbol: ContainsSymbol) -> bool {
        self.key.contains(symbol)
    }
}
