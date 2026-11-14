// Auto-generated code, DO NOT EDIT DIRECTLY!
// To edit this generated file you have to edit `tasks/ast_tools/src/generators/visit.rs`.

//! Visitor Pattern
//!
//! See:
//! * [visitor pattern](https://rust-unofficial.github.io/patterns/patterns/behavioural/visitor.html)
//! * [rustc visitor](https://github.com/rust-lang/rust/blob/1.82.0/compiler/rustc_ast/src/visit.rs)

#![expect(unused_variables, clippy::match_same_arms, clippy::semicolon_if_nothing_returned)]
#![allow(clippy::needless_pass_by_ref_mut, clippy::trivially_copy_pass_by_ref)]

use std::cell::Cell;

use oxc_allocator::Vec;
use oxc_syntax::scope::{ScopeFlags, ScopeId};

use oxc_ast::ast::*;
use oxc_ast::ast_kind::AstKind;

use walk::*;

/// Syntax tree traversal
pub trait Visit<'a>: Sized {
    #[inline]
    fn enter_node(&mut self, kind: AstKind<'a>) {}
    #[inline]
    fn leave_node(&mut self, kind: AstKind<'a>) {}

    #[inline]
    fn enter_scope(&mut self, flags: ScopeFlags, scope_id: &Cell<Option<ScopeId>>) {}
    #[inline]
    fn leave_scope(&mut self) {}

    #[inline]
    fn visit_program(&mut self, it: &'a Program<'a>) {
        walk_program(self, it);
    }

    #[inline]
    fn visit_expression(&mut self, it: &'a Expression<'a>) {
        walk_expression(self, it);
    }

    #[inline]
    fn visit_identifier_name(&mut self, it: &'a IdentifierName<'a>) {
        walk_identifier_name(self, it);
    }

    #[inline]
    fn visit_identifier_reference(&mut self, it: &'a IdentifierReference<'a>) {
        walk_identifier_reference(self, it);
    }

    #[inline]
    fn visit_binding_identifier(&mut self, it: &'a BindingIdentifier<'a>) {
        walk_binding_identifier(self, it);
    }

    #[inline]
    fn visit_label_identifier(&mut self, it: &'a LabelIdentifier<'a>) {
        walk_label_identifier(self, it);
    }

    #[inline]
    fn visit_this_expression(&mut self, it: &'a ThisExpression) {
        walk_this_expression(self, it);
    }

    #[inline]
    fn visit_array_expression(&mut self, it: &'a ArrayExpression<'a>) {
        walk_array_expression(self, it);
    }

    #[inline]
    fn visit_array_expression_element(&mut self, it: &'a ArrayExpressionElement<'a>) {
        walk_array_expression_element(self, it);
    }

    #[inline]
    fn visit_elision(&mut self, it: &'a Elision) {
        walk_elision(self, it);
    }

    #[inline]
    fn visit_object_expression(&mut self, it: &'a ObjectExpression<'a>) {
        walk_object_expression(self, it);
    }

    #[inline]
    fn visit_object_property_kind(&mut self, it: &'a ObjectPropertyKind<'a>) {
        walk_object_property_kind(self, it);
    }

    #[inline]
    fn visit_object_property(&mut self, it: &'a ObjectProperty<'a>) {
        walk_object_property(self, it);
    }

    #[inline]
    fn visit_property_key(&mut self, it: &'a PropertyKey<'a>) {
        walk_property_key(self, it);
    }

    #[inline]
    fn visit_template_literal(&mut self, it: &'a TemplateLiteral<'a>) {
        walk_template_literal(self, it);
    }

    #[inline]
    fn visit_tagged_template_expression(&mut self, it: &'a TaggedTemplateExpression<'a>) {
        walk_tagged_template_expression(self, it);
    }

    #[inline]
    fn visit_template_element(&mut self, it: &'a TemplateElement<'a>) {
        walk_template_element(self, it);
    }


    #[inline]
    fn visit_computed_member_expression(&mut self, it: &'a ComputedMemberExpression<'a>) {
        walk_computed_member_expression(self, it);
    }

    #[inline]
    fn visit_static_member_expression(&mut self, it: &'a StaticMemberExpression<'a>) {
        walk_static_member_expression(self, it);
    }

    #[inline]
    fn visit_private_field_expression(&mut self, it: &'a PrivateFieldExpression<'a>) {
        walk_private_field_expression(self, it);
    }

    #[inline]
    fn visit_call_expression(&mut self, it: &'a CallExpression<'a>) {
        walk_call_expression(self, it);
    }

    #[inline]
    fn visit_new_expression(&mut self, it: &'a NewExpression<'a>) {
        walk_new_expression(self, it);
    }

    #[inline]
    fn visit_meta_property(&mut self, it: &'a MetaProperty<'a>) {
        walk_meta_property(self, it);
    }

    #[inline]
    fn visit_spread_element(&mut self, it: &'a SpreadElement<'a>) {
        walk_spread_element(self, it);
    }

    #[inline]
    fn visit_argument(&mut self, it: &'a Argument<'a>) {
        walk_argument(self, it);
    }

    #[inline]
    fn visit_update_expression(&mut self, it: &'a UpdateExpression<'a>) {
        walk_update_expression(self, it);
    }

    #[inline]
    fn visit_unary_expression(&mut self, it: &'a UnaryExpression<'a>) {
        walk_unary_expression(self, it);
    }

    #[inline]
    fn visit_binary_expression(&mut self, it: &'a BinaryExpression<'a>) {
        walk_binary_expression(self, it);
    }

    #[inline]
    fn visit_private_in_expression(&mut self, it: &'a PrivateInExpression<'a>) {
        walk_private_in_expression(self, it);
    }

    #[inline]
    fn visit_logical_expression(&mut self, it: &'a LogicalExpression<'a>) {
        walk_logical_expression(self, it);
    }

    #[inline]
    fn visit_conditional_expression(&mut self, it: &'a ConditionalExpression<'a>) {
        walk_conditional_expression(self, it);
    }

    #[inline]
    fn visit_assignment_expression(&mut self, it: &'a AssignmentExpression<'a>) {
        walk_assignment_expression(self, it);
    }

    #[inline]
    fn visit_assignment_target(&mut self, it: &'a AssignmentTarget<'a>) {
        walk_assignment_target(self, it);
    }

    #[inline]
    fn visit_simple_assignment_target(&mut self, it: &'a SimpleAssignmentTarget<'a>) {
        walk_simple_assignment_target(self, it);
    }


    #[inline]
    fn visit_array_assignment_target(&mut self, it: &'a ArrayAssignmentTarget<'a>) {
        walk_array_assignment_target(self, it);
    }

    #[inline]
    fn visit_object_assignment_target(&mut self, it: &'a ObjectAssignmentTarget<'a>) {
        walk_object_assignment_target(self, it);
    }

    #[inline]
    fn visit_assignment_target_rest(&mut self, it: &'a AssignmentTargetRest<'a>) {
        walk_assignment_target_rest(self, it);
    }

    #[inline]
    fn visit_assignment_target_maybe_default(&mut self, it: &'a AssignmentTargetMaybeDefault<'a>) {
        walk_assignment_target_maybe_default(self, it);
    }

    #[inline]
    fn visit_assignment_target_with_default(&mut self, it: &'a AssignmentTargetWithDefault<'a>) {
        walk_assignment_target_with_default(self, it);
    }

    #[inline]
    fn visit_assignment_target_property(&mut self, it: &'a AssignmentTargetProperty<'a>) {
        walk_assignment_target_property(self, it);
    }

    #[inline]
    fn visit_assignment_target_property_identifier(
        &mut self,
        it: &'a AssignmentTargetPropertyIdentifier<'a>,
    ) {
        walk_assignment_target_property_identifier(self, it);
    }

    #[inline]
    fn visit_assignment_target_property_property(
        &mut self,
        it: &'a AssignmentTargetPropertyProperty<'a>,
    ) {
        walk_assignment_target_property_property(self, it);
    }

    #[inline]
    fn visit_sequence_expression(&mut self, it: &'a SequenceExpression<'a>) {
        walk_sequence_expression(self, it);
    }

    #[inline]
    fn visit_super(&mut self, it: &'a Super) {
        walk_super(self, it);
    }

    #[inline]
    fn visit_await_expression(&mut self, it: &'a AwaitExpression<'a>) {
        walk_await_expression(self, it);
    }

    #[inline]
    fn visit_chain_expression(&mut self, it: &'a ChainExpression<'a>) {
        walk_chain_expression(self, it);
    }

    #[inline]
    fn visit_chain_element(&mut self, it: &'a ChainElement<'a>) {
        walk_chain_element(self, it);
    }

    #[inline]
    fn visit_parenthesized_expression(&mut self, it: &'a ParenthesizedExpression<'a>) {
        walk_parenthesized_expression(self, it);
    }

    #[inline]
    fn visit_statement(&mut self, it: &'a Statement<'a>) {
        walk_statement(self, it);
    }

    #[inline]
    fn visit_directive(&mut self, it: &'a Directive<'a>) {
        walk_directive(self, it);
    }

    #[inline]
    fn visit_hashbang(&mut self, it: &'a Hashbang<'a>) {
        walk_hashbang(self, it);
    }

    #[inline]
    fn visit_block_statement(&mut self, it: &'a BlockStatement<'a>) {
        walk_block_statement(self, it);
    }

    #[inline]
    fn visit_declaration(&mut self, it: &'a Declaration<'a>) {
        walk_declaration(self, it);
    }

    #[inline]
    fn visit_variable_declaration(&mut self, it: &'a VariableDeclaration<'a>) {
        walk_variable_declaration(self, it);
    }

    #[inline]
    fn visit_variable_declarator(&mut self, it: &'a VariableDeclarator<'a>) {
        walk_variable_declarator(self, it);
    }

    #[inline]
    fn visit_empty_statement(&mut self, it: &'a EmptyStatement) {
        walk_empty_statement(self, it);
    }

    #[inline]
    fn visit_expression_statement(&mut self, it: &'a ExpressionStatement<'a>) {
        walk_expression_statement(self, it);
    }

    #[inline]
    fn visit_if_statement(&mut self, it: &'a IfStatement<'a>) {
        walk_if_statement(self, it);
    }

    #[inline]
    fn visit_do_while_statement(&mut self, it: &'a DoWhileStatement<'a>) {
        walk_do_while_statement(self, it);
    }

    #[inline]
    fn visit_while_statement(&mut self, it: &'a WhileStatement<'a>) {
        walk_while_statement(self, it);
    }

    #[inline]
    fn visit_for_statement(&mut self, it: &'a ForStatement<'a>) {
        walk_for_statement(self, it);
    }

    #[inline]
    fn visit_for_statement_init(&mut self, it: &'a ForStatementInit<'a>) {
        walk_for_statement_init(self, it);
    }

    #[inline]
    fn visit_for_in_statement(&mut self, it: &'a ForInStatement<'a>) {
        walk_for_in_statement(self, it);
    }

    #[inline]
    fn visit_for_statement_left(&mut self, it: &'a ForStatementLeft<'a>) {
        walk_for_statement_left(self, it);
    }

    #[inline]
    fn visit_for_of_statement(&mut self, it: &'a ForOfStatement<'a>) {
        walk_for_of_statement(self, it);
    }

    #[inline]
    fn visit_continue_statement(&mut self, it: &'a ContinueStatement<'a>) {
        walk_continue_statement(self, it);
    }

    #[inline]
    fn visit_break_statement(&mut self, it: &'a BreakStatement<'a>) {
        walk_break_statement(self, it);
    }

    #[inline]
    fn visit_return_statement(&mut self, it: &'a ReturnStatement<'a>) {
        walk_return_statement(self, it);
    }

    #[inline]
    fn visit_with_statement(&mut self, it: &'a WithStatement<'a>) {
        walk_with_statement(self, it);
    }

    #[inline]
    fn visit_switch_statement(&mut self, it: &'a SwitchStatement<'a>) {
        walk_switch_statement(self, it);
    }

    #[inline]
    fn visit_switch_case(&mut self, it: &'a SwitchCase<'a>) {
        walk_switch_case(self, it);
    }

    #[inline]
    fn visit_labeled_statement(&mut self, it: &'a LabeledStatement<'a>) {
        walk_labeled_statement(self, it);
    }

    #[inline]
    fn visit_throw_statement(&mut self, it: &'a ThrowStatement<'a>) {
        walk_throw_statement(self, it);
    }

    #[inline]
    fn visit_try_statement(&mut self, it: &'a TryStatement<'a>) {
        walk_try_statement(self, it);
    }

    #[inline]
    fn visit_catch_clause(&mut self, it: &'a CatchClause<'a>) {
        walk_catch_clause(self, it);
    }

    #[inline]
    fn visit_catch_parameter(&mut self, it: &'a CatchParameter<'a>) {
        walk_catch_parameter(self, it);
    }

    #[inline]
    fn visit_debugger_statement(&mut self, it: &'a DebuggerStatement) {
        walk_debugger_statement(self, it);
    }

    #[inline]
    fn visit_binding_pattern(&mut self, it: &'a BindingPattern<'a>) {
        walk_binding_pattern(self, it);
    }

    #[inline]
    fn visit_assignment_pattern(&mut self, it: &'a AssignmentPattern<'a>) {
        walk_assignment_pattern(self, it);
    }

    #[inline]
    fn visit_object_pattern(&mut self, it: &'a ObjectPattern<'a>) {
        walk_object_pattern(self, it);
    }

    #[inline]
    fn visit_binding_property(&mut self, it: &'a BindingProperty<'a>) {
        walk_binding_property(self, it);
    }

    #[inline]
    fn visit_array_pattern(&mut self, it: &'a ArrayPattern<'a>) {
        walk_array_pattern(self, it);
    }

    #[inline]
    fn visit_binding_rest_element(&mut self, it: &'a BindingRestElement<'a>) {
        walk_binding_rest_element(self, it);
    }

    #[inline]
    fn visit_function(&mut self, it: &'a Function<'a>, flags: ScopeFlags) {
        walk_function(self, it, flags);
    }

    #[inline]
    fn visit_formal_parameters(&mut self, it: &'a FormalParameters<'a>) {
        walk_formal_parameters(self, it);
    }

    #[inline]
    fn visit_formal_parameter(&mut self, it: &'a FormalParameter<'a>) {
        walk_formal_parameter(self, it);
    }

    #[inline]
    fn visit_formal_parameter_rest(&mut self, it: &'a FormalParameterRest<'a>) {
        walk_formal_parameter_rest(self, it);
    }

    #[inline]
    fn visit_function_body(&mut self, it: &'a FunctionBody<'a>) {
        walk_function_body(self, it);
    }

    #[inline]
    fn visit_arrow_function_expression(&mut self, it: &'a ArrowFunctionExpression<'a>) {
        walk_arrow_function_expression(self, it);
    }

    #[inline]
    fn visit_yield_expression(&mut self, it: &'a YieldExpression<'a>) {
        walk_yield_expression(self, it);
    }

    #[inline]
    fn visit_class(&mut self, it: &'a Class<'a>) {
        walk_class(self, it);
    }

    #[inline]
    fn visit_class_body(&mut self, it: &'a ClassBody<'a>) {
        walk_class_body(self, it);
    }

    #[inline]
    fn visit_class_element(&mut self, it: &'a ClassElement<'a>) {
        walk_class_element(self, it);
    }

    #[inline]
    fn visit_method_definition(&mut self, it: &'a MethodDefinition<'a>) {
        walk_method_definition(self, it);
    }

    #[inline]
    fn visit_property_definition(&mut self, it: &'a PropertyDefinition<'a>) {
        walk_property_definition(self, it);
    }

    #[inline]
    fn visit_private_identifier(&mut self, it: &'a PrivateIdentifier<'a>) {
        walk_private_identifier(self, it);
    }

    #[inline]
    fn visit_static_block(&mut self, it: &'a StaticBlock<'a>) {
        walk_static_block(self, it);
    }


    #[inline]
    fn visit_accessor_property(&mut self, it: &'a AccessorProperty<'a>) {
        walk_accessor_property(self, it);
    }

    #[inline]
    fn visit_import_expression(&mut self, it: &'a ImportExpression<'a>) {
        walk_import_expression(self, it);
    }

    #[inline]
    fn visit_import_declaration(&mut self, it: &'a ImportDeclaration<'a>) {
        walk_import_declaration(self, it);
    }

    #[inline]
    fn visit_import_declaration_specifier(&mut self, it: &'a ImportDeclarationSpecifier<'a>) {
        walk_import_declaration_specifier(self, it);
    }

    #[inline]
    fn visit_import_specifier(&mut self, it: &'a ImportSpecifier<'a>) {
        walk_import_specifier(self, it);
    }

    #[inline]
    fn visit_import_default_specifier(&mut self, it: &'a ImportDefaultSpecifier<'a>) {
        walk_import_default_specifier(self, it);
    }

    #[inline]
    fn visit_import_namespace_specifier(&mut self, it: &'a ImportNamespaceSpecifier<'a>) {
        walk_import_namespace_specifier(self, it);
    }

    #[inline]
    fn visit_with_clause(&mut self, it: &'a WithClause<'a>) {
        walk_with_clause(self, it);
    }

    #[inline]
    fn visit_import_attribute(&mut self, it: &'a ImportAttribute<'a>) {
        walk_import_attribute(self, it);
    }

    #[inline]
    fn visit_import_attribute_key(&mut self, it: &'a ImportAttributeKey<'a>) {
        walk_import_attribute_key(self, it);
    }

    #[inline]
    fn visit_export_named_declaration(&mut self, it: &'a ExportNamedDeclaration<'a>) {
        walk_export_named_declaration(self, it);
    }

    #[inline]
    fn visit_export_default_declaration(&mut self, it: &'a ExportDefaultDeclaration<'a>) {
        walk_export_default_declaration(self, it);
    }

    #[inline]
    fn visit_export_all_declaration(&mut self, it: &'a ExportAllDeclaration<'a>) {
        walk_export_all_declaration(self, it);
    }

    #[inline]
    fn visit_export_specifier(&mut self, it: &'a ExportSpecifier<'a>) {
        walk_export_specifier(self, it);
    }

    #[inline]
    fn visit_export_default_declaration_kind(&mut self, it: &'a ExportDefaultDeclarationKind<'a>) {
        walk_export_default_declaration_kind(self, it);
    }

    #[inline]
    fn visit_module_export_name(&mut self, it: &'a ModuleExportName<'a>) {
        walk_module_export_name(self, it);
    }

    #[inline]
    fn visit_v8_intrinsic_expression(&mut self, it: &'a V8IntrinsicExpression<'a>) {
        walk_v8_intrinsic_expression(self, it);
    }

    #[inline]
    fn visit_boolean_literal(&mut self, it: &'a BooleanLiteral) {
        walk_boolean_literal(self, it);
    }

    #[inline]
    fn visit_null_literal(&mut self, it: &'a NullLiteral) {
        walk_null_literal(self, it);
    }

    #[inline]
    fn visit_numeric_literal(&mut self, it: &'a NumericLiteral<'a>) {
        walk_numeric_literal(self, it);
    }

    #[inline]
    fn visit_string_literal(&mut self, it: &'a StringLiteral<'a>) {
        walk_string_literal(self, it);
    }

    #[inline]
    fn visit_big_int_literal(&mut self, it: &'a BigIntLiteral<'a>) {
        walk_big_int_literal(self, it);
    }

    #[inline]
    fn visit_reg_exp_literal(&mut self, it: &'a RegExpLiteral<'a>) {
        walk_reg_exp_literal(self, it);
    }

    #[inline]
    fn visit_jsx_element(&mut self, it: &'a JSXElement<'a>) {
        walk_jsx_element(self, it);
    }

    #[inline]
    fn visit_jsx_opening_element(&mut self, it: &'a JSXOpeningElement<'a>) {
        walk_jsx_opening_element(self, it);
    }

    #[inline]
    fn visit_jsx_closing_element(&mut self, it: &'a JSXClosingElement<'a>) {
        walk_jsx_closing_element(self, it);
    }

    #[inline]
    fn visit_jsx_fragment(&mut self, it: &'a JSXFragment<'a>) {
        walk_jsx_fragment(self, it);
    }

    #[inline]
    fn visit_jsx_opening_fragment(&mut self, it: &'a JSXOpeningFragment) {
        walk_jsx_opening_fragment(self, it);
    }

    #[inline]
    fn visit_jsx_closing_fragment(&mut self, it: &'a JSXClosingFragment) {
        walk_jsx_closing_fragment(self, it);
    }

    #[inline]
    fn visit_jsx_element_name(&mut self, it: &'a JSXElementName<'a>) {
        walk_jsx_element_name(self, it);
    }

    #[inline]
    fn visit_jsx_namespaced_name(&mut self, it: &'a JSXNamespacedName<'a>) {
        walk_jsx_namespaced_name(self, it);
    }

    #[inline]
    fn visit_jsx_member_expression(&mut self, it: &'a JSXMemberExpression<'a>) {
        walk_jsx_member_expression(self, it);
    }

    #[inline]
    fn visit_jsx_member_expression_object(&mut self, it: &'a JSXMemberExpressionObject<'a>) {
        walk_jsx_member_expression_object(self, it);
    }

    #[inline]
    fn visit_jsx_expression_container(&mut self, it: &'a JSXExpressionContainer<'a>) {
        walk_jsx_expression_container(self, it);
    }

    #[inline]
    fn visit_jsx_expression(&mut self, it: &'a JSXExpression<'a>) {
        walk_jsx_expression(self, it);
    }

    #[inline]
    fn visit_jsx_empty_expression(&mut self, it: &'a JSXEmptyExpression) {
        walk_jsx_empty_expression(self, it);
    }

    #[inline]
    fn visit_jsx_attribute_item(&mut self, it: &'a JSXAttributeItem<'a>) {
        walk_jsx_attribute_item(self, it);
    }

    #[inline]
    fn visit_jsx_attribute(&mut self, it: &'a JSXAttribute<'a>) {
        walk_jsx_attribute(self, it);
    }

    #[inline]
    fn visit_jsx_spread_attribute(&mut self, it: &'a JSXSpreadAttribute<'a>) {
        walk_jsx_spread_attribute(self, it);
    }

    #[inline]
    fn visit_jsx_attribute_name(&mut self, it: &'a JSXAttributeName<'a>) {
        walk_jsx_attribute_name(self, it);
    }

    #[inline]
    fn visit_jsx_attribute_value(&mut self, it: &'a JSXAttributeValue<'a>) {
        walk_jsx_attribute_value(self, it);
    }

    #[inline]
    fn visit_jsx_identifier(&mut self, it: &'a JSXIdentifier<'a>) {
        walk_jsx_identifier(self, it);
    }

    #[inline]
    fn visit_jsx_child(&mut self, it: &'a JSXChild<'a>) {
        walk_jsx_child(self, it);
    }

    #[inline]
    fn visit_jsx_spread_child(&mut self, it: &'a JSXSpreadChild<'a>) {
        walk_jsx_spread_child(self, it);
    }

    #[inline]
    fn visit_jsx_text(&mut self, it: &'a JSXText<'a>) {
        walk_jsx_text(self, it);
    }

    #[inline]
    fn visit_ts_this_parameter(&mut self, it: &'a TSThisParameter<'a>) {
        walk_ts_this_parameter(self, it);
    }

    #[inline]
    fn visit_ts_enum_declaration(&mut self, it: &'a TSEnumDeclaration<'a>) {
        walk_ts_enum_declaration(self, it);
    }

    #[inline]
    fn visit_ts_enum_body(&mut self, it: &'a TSEnumBody<'a>) {
        walk_ts_enum_body(self, it);
    }

    #[inline]
    fn visit_ts_enum_member(&mut self, it: &'a TSEnumMember<'a>) {
        walk_ts_enum_member(self, it);
    }

    #[inline]
    fn visit_ts_enum_member_name(&mut self, it: &'a TSEnumMemberName<'a>) {
        walk_ts_enum_member_name(self, it);
    }

    #[inline]
    fn visit_ts_type_annotation(&mut self, it: &'a TSTypeAnnotation<'a>) {
        walk_ts_type_annotation(self, it);
    }

    #[inline]
    fn visit_ts_literal_type(&mut self, it: &'a TSLiteralType<'a>) {
        walk_ts_literal_type(self, it);
    }

    #[inline]
    fn visit_ts_literal(&mut self, it: &'a TSLiteral<'a>) {
        walk_ts_literal(self, it);
    }

    #[inline]
    fn visit_ts_type(&mut self, it: &'a TSType<'a>) {
        walk_ts_type(self, it);
    }

    #[inline]
    fn visit_ts_conditional_type(&mut self, it: &'a TSConditionalType<'a>) {
        walk_ts_conditional_type(self, it);
    }

    #[inline]
    fn visit_ts_union_type(&mut self, it: &'a TSUnionType<'a>) {
        walk_ts_union_type(self, it);
    }

    #[inline]
    fn visit_ts_intersection_type(&mut self, it: &'a TSIntersectionType<'a>) {
        walk_ts_intersection_type(self, it);
    }

    #[inline]
    fn visit_ts_parenthesized_type(&mut self, it: &'a TSParenthesizedType<'a>) {
        walk_ts_parenthesized_type(self, it);
    }

    #[inline]
    fn visit_ts_type_operator(&mut self, it: &'a TSTypeOperator<'a>) {
        walk_ts_type_operator(self, it);
    }

    #[inline]
    fn visit_ts_array_type(&mut self, it: &'a TSArrayType<'a>) {
        walk_ts_array_type(self, it);
    }

    #[inline]
    fn visit_ts_indexed_access_type(&mut self, it: &'a TSIndexedAccessType<'a>) {
        walk_ts_indexed_access_type(self, it);
    }

    #[inline]
    fn visit_ts_tuple_type(&mut self, it: &'a TSTupleType<'a>) {
        walk_ts_tuple_type(self, it);
    }

    #[inline]
    fn visit_ts_named_tuple_member(&mut self, it: &'a TSNamedTupleMember<'a>) {
        walk_ts_named_tuple_member(self, it);
    }

    #[inline]
    fn visit_ts_optional_type(&mut self, it: &'a TSOptionalType<'a>) {
        walk_ts_optional_type(self, it);
    }

    #[inline]
    fn visit_ts_rest_type(&mut self, it: &'a TSRestType<'a>) {
        walk_ts_rest_type(self, it);
    }

    #[inline]
    fn visit_ts_tuple_element(&mut self, it: &'a TSTupleElement<'a>) {
        walk_ts_tuple_element(self, it);
    }

    #[inline]
    fn visit_ts_any_keyword(&mut self, it: &'a TSAnyKeyword) {
        walk_ts_any_keyword(self, it);
    }

    #[inline]
    fn visit_ts_string_keyword(&mut self, it: &'a TSStringKeyword) {
        walk_ts_string_keyword(self, it);
    }

    #[inline]
    fn visit_ts_boolean_keyword(&mut self, it: &'a TSBooleanKeyword) {
        walk_ts_boolean_keyword(self, it);
    }

    #[inline]
    fn visit_ts_number_keyword(&mut self, it: &'a TSNumberKeyword) {
        walk_ts_number_keyword(self, it);
    }

    #[inline]
    fn visit_ts_never_keyword(&mut self, it: &'a TSNeverKeyword) {
        walk_ts_never_keyword(self, it);
    }

    #[inline]
    fn visit_ts_intrinsic_keyword(&mut self, it: &'a TSIntrinsicKeyword) {
        walk_ts_intrinsic_keyword(self, it);
    }

    #[inline]
    fn visit_ts_unknown_keyword(&mut self, it: &'a TSUnknownKeyword) {
        walk_ts_unknown_keyword(self, it);
    }

    #[inline]
    fn visit_ts_null_keyword(&mut self, it: &'a TSNullKeyword) {
        walk_ts_null_keyword(self, it);
    }

    #[inline]
    fn visit_ts_undefined_keyword(&mut self, it: &'a TSUndefinedKeyword) {
        walk_ts_undefined_keyword(self, it);
    }

    #[inline]
    fn visit_ts_void_keyword(&mut self, it: &'a TSVoidKeyword) {
        walk_ts_void_keyword(self, it);
    }

    #[inline]
    fn visit_ts_symbol_keyword(&mut self, it: &'a TSSymbolKeyword) {
        walk_ts_symbol_keyword(self, it);
    }

    #[inline]
    fn visit_ts_this_type(&mut self, it: &'a TSThisType) {
        walk_ts_this_type(self, it);
    }

    #[inline]
    fn visit_ts_object_keyword(&mut self, it: &'a TSObjectKeyword) {
        walk_ts_object_keyword(self, it);
    }

    #[inline]
    fn visit_ts_big_int_keyword(&mut self, it: &'a TSBigIntKeyword) {
        walk_ts_big_int_keyword(self, it);
    }

    #[inline]
    fn visit_ts_type_reference(&mut self, it: &'a TSTypeReference<'a>) {
        walk_ts_type_reference(self, it);
    }

    #[inline]
    fn visit_ts_type_name(&mut self, it: &'a TSTypeName<'a>) {
        walk_ts_type_name(self, it);
    }

    #[inline]
    fn visit_ts_qualified_name(&mut self, it: &'a TSQualifiedName<'a>) {
        walk_ts_qualified_name(self, it);
    }

    #[inline]
    fn visit_ts_type_parameter_instantiation(&mut self, it: &'a TSTypeParameterInstantiation<'a>) {
        walk_ts_type_parameter_instantiation(self, it);
    }

    #[inline]
    fn visit_ts_type_parameter(&mut self, it: &'a TSTypeParameter<'a>) {
        walk_ts_type_parameter(self, it);
    }

    #[inline]
    fn visit_ts_type_parameter_declaration(&mut self, it: &'a TSTypeParameterDeclaration<'a>) {
        walk_ts_type_parameter_declaration(self, it);
    }

    #[inline]
    fn visit_ts_type_alias_declaration(&mut self, it: &'a TSTypeAliasDeclaration<'a>) {
        walk_ts_type_alias_declaration(self, it);
    }

    #[inline]
    fn visit_ts_class_implements(&mut self, it: &'a TSClassImplements<'a>) {
        walk_ts_class_implements(self, it);
    }

    #[inline]
    fn visit_ts_interface_declaration(&mut self, it: &'a TSInterfaceDeclaration<'a>) {
        walk_ts_interface_declaration(self, it);
    }

    #[inline]
    fn visit_ts_interface_body(&mut self, it: &'a TSInterfaceBody<'a>) {
        walk_ts_interface_body(self, it);
    }

    #[inline]
    fn visit_ts_property_signature(&mut self, it: &'a TSPropertySignature<'a>) {
        walk_ts_property_signature(self, it);
    }

    #[inline]
    fn visit_ts_signature(&mut self, it: &'a TSSignature<'a>) {
        walk_ts_signature(self, it);
    }

    #[inline]
    fn visit_ts_index_signature(&mut self, it: &'a TSIndexSignature<'a>) {
        walk_ts_index_signature(self, it);
    }

    #[inline]
    fn visit_ts_call_signature_declaration(&mut self, it: &'a TSCallSignatureDeclaration<'a>) {
        walk_ts_call_signature_declaration(self, it);
    }

    #[inline]
    fn visit_ts_method_signature(&mut self, it: &'a TSMethodSignature<'a>) {
        walk_ts_method_signature(self, it);
    }

    #[inline]
    fn visit_ts_construct_signature_declaration(
        &mut self,
        it: &'a TSConstructSignatureDeclaration<'a>,
    ) {
        walk_ts_construct_signature_declaration(self, it);
    }

    #[inline]
    fn visit_ts_index_signature_name(&mut self, it: &'a TSIndexSignatureName<'a>) {
        walk_ts_index_signature_name(self, it);
    }

    #[inline]
    fn visit_ts_interface_heritage(&mut self, it: &'a TSInterfaceHeritage<'a>) {
        walk_ts_interface_heritage(self, it);
    }

    #[inline]
    fn visit_ts_type_predicate(&mut self, it: &'a TSTypePredicate<'a>) {
        walk_ts_type_predicate(self, it);
    }

    #[inline]
    fn visit_ts_type_predicate_name(&mut self, it: &'a TSTypePredicateName<'a>) {
        walk_ts_type_predicate_name(self, it);
    }

    #[inline]
    fn visit_ts_module_declaration(&mut self, it: &'a TSModuleDeclaration<'a>) {
        walk_ts_module_declaration(self, it);
    }

    #[inline]
    fn visit_ts_module_declaration_name(&mut self, it: &'a TSModuleDeclarationName<'a>) {
        walk_ts_module_declaration_name(self, it);
    }

    #[inline]
    fn visit_ts_module_declaration_body(&mut self, it: &'a TSModuleDeclarationBody<'a>) {
        walk_ts_module_declaration_body(self, it);
    }

    #[inline]
    fn visit_ts_global_declaration(&mut self, it: &'a TSGlobalDeclaration<'a>) {
        walk_ts_global_declaration(self, it);
    }

    #[inline]
    fn visit_ts_module_block(&mut self, it: &'a TSModuleBlock<'a>) {
        walk_ts_module_block(self, it);
    }

    #[inline]
    fn visit_ts_type_literal(&mut self, it: &'a TSTypeLiteral<'a>) {
        walk_ts_type_literal(self, it);
    }

    #[inline]
    fn visit_ts_infer_type(&mut self, it: &'a TSInferType<'a>) {
        walk_ts_infer_type(self, it);
    }

    #[inline]
    fn visit_ts_type_query(&mut self, it: &'a TSTypeQuery<'a>) {
        walk_ts_type_query(self, it);
    }

    #[inline]
    fn visit_ts_type_query_expr_name(&mut self, it: &'a TSTypeQueryExprName<'a>) {
        walk_ts_type_query_expr_name(self, it);
    }

    #[inline]
    fn visit_ts_import_type(&mut self, it: &'a TSImportType<'a>) {
        walk_ts_import_type(self, it);
    }

    #[inline]
    fn visit_ts_import_type_qualifier(&mut self, it: &'a TSImportTypeQualifier<'a>) {
        walk_ts_import_type_qualifier(self, it);
    }

    #[inline]
    fn visit_ts_import_type_qualified_name(&mut self, it: &'a TSImportTypeQualifiedName<'a>) {
        walk_ts_import_type_qualified_name(self, it);
    }

    #[inline]
    fn visit_ts_function_type(&mut self, it: &'a TSFunctionType<'a>) {
        walk_ts_function_type(self, it);
    }

    #[inline]
    fn visit_ts_constructor_type(&mut self, it: &'a TSConstructorType<'a>) {
        walk_ts_constructor_type(self, it);
    }

    #[inline]
    fn visit_ts_mapped_type(&mut self, it: &'a TSMappedType<'a>) {
        walk_ts_mapped_type(self, it);
    }

    #[inline]
    fn visit_ts_template_literal_type(&mut self, it: &'a TSTemplateLiteralType<'a>) {
        walk_ts_template_literal_type(self, it);
    }

    #[inline]
    fn visit_ts_as_expression(&mut self, it: &'a TSAsExpression<'a>) {
        walk_ts_as_expression(self, it);
    }

    #[inline]
    fn visit_ts_satisfies_expression(&mut self, it: &'a TSSatisfiesExpression<'a>) {
        walk_ts_satisfies_expression(self, it);
    }

    #[inline]
    fn visit_ts_type_assertion(&mut self, it: &'a TSTypeAssertion<'a>) {
        walk_ts_type_assertion(self, it);
    }

    #[inline]
    fn visit_ts_import_equals_declaration(&mut self, it: &'a TSImportEqualsDeclaration<'a>) {
        walk_ts_import_equals_declaration(self, it);
    }

    #[inline]
    fn visit_ts_module_reference(&mut self, it: &'a TSModuleReference<'a>) {
        walk_ts_module_reference(self, it);
    }

    #[inline]
    fn visit_ts_external_module_reference(&mut self, it: &'a TSExternalModuleReference<'a>) {
        walk_ts_external_module_reference(self, it);
    }

    #[inline]
    fn visit_ts_non_null_expression(&mut self, it: &'a TSNonNullExpression<'a>) {
        walk_ts_non_null_expression(self, it);
    }

    #[inline]
    fn visit_decorator(&mut self, it: &'a Decorator<'a>) {
        walk_decorator(self, it);
    }

    #[inline]
    fn visit_ts_export_assignment(&mut self, it: &'a TSExportAssignment<'a>) {
        walk_ts_export_assignment(self, it);
    }

    #[inline]
    fn visit_ts_namespace_export_declaration(&mut self, it: &'a TSNamespaceExportDeclaration<'a>) {
        walk_ts_namespace_export_declaration(self, it);
    }

    #[inline]
    fn visit_ts_instantiation_expression(&mut self, it: &'a TSInstantiationExpression<'a>) {
        walk_ts_instantiation_expression(self, it);
    }

    #[inline]
    fn visit_js_doc_nullable_type(&mut self, it: &'a JSDocNullableType<'a>) {
        walk_js_doc_nullable_type(self, it);
    }

    #[inline]
    fn visit_js_doc_non_nullable_type(&mut self, it: &'a JSDocNonNullableType<'a>) {
        walk_js_doc_non_nullable_type(self, it);
    }

    #[inline]
    fn visit_js_doc_unknown_type(&mut self, it: &'a JSDocUnknownType) {
        walk_js_doc_unknown_type(self, it);
    }

    #[inline]
    fn visit_span(&mut self, it: &'a Span) {
        walk_span(self, it);
    }

    #[inline]
    fn visit_directives(&mut self, it: &'a Vec<'a, Directive<'a>>) {
        walk_directives(self, it);
    }

    #[inline]
    fn visit_statements(&mut self, it: &'a Vec<'a, Statement<'a>>) {
        walk_statements(self, it);
    }

    #[inline]
    fn visit_array_expression_elements(&mut self, it: &'a Vec<'a, ArrayExpressionElement<'a>>) {
        walk_array_expression_elements(self, it);
    }

    #[inline]
    fn visit_object_property_kinds(&mut self, it: &'a Vec<'a, ObjectPropertyKind<'a>>) {
        walk_object_property_kinds(self, it);
    }

    #[inline]
    fn visit_template_elements(&mut self, it: &'a Vec<'a, TemplateElement<'a>>) {
        walk_template_elements(self, it);
    }

    #[inline]
    fn visit_expressions(&mut self, it: &'a Vec<'a, Expression<'a>>) {
        walk_expressions(self, it);
    }

    #[inline]
    fn visit_arguments(&mut self, it: &'a Vec<'a, Argument<'a>>) {
        walk_arguments(self, it);
    }

    #[inline]
    fn visit_assignment_target_properties(&mut self, it: &'a Vec<'a, AssignmentTargetProperty<'a>>) {
        walk_assignment_target_properties(self, it);
    }

    #[inline]
    fn visit_variable_declarators(&mut self, it: &'a Vec<'a, VariableDeclarator<'a>>) {
        walk_variable_declarators(self, it);
    }

    #[inline]
    fn visit_switch_cases(&mut self, it: &'a Vec<'a, SwitchCase<'a>>) {
        walk_switch_cases(self, it);
    }

    #[inline]
    fn visit_binding_properties(&mut self, it: &'a Vec<'a, BindingProperty<'a>>) {
        walk_binding_properties(self, it);
    }

    #[inline]
    fn visit_formal_parameter_list(&mut self, it: &'a Vec<'a, FormalParameter<'a>>) {
        walk_formal_parameter_list(self, it);
    }

    #[inline]
    fn visit_decorators(&mut self, it: &'a Vec<'a, Decorator<'a>>) {
        walk_decorators(self, it);
    }

    #[inline]
    fn visit_ts_class_implements_list(&mut self, it: &'a Vec<'a, TSClassImplements<'a>>) {
        walk_ts_class_implements_list(self, it);
    }

    #[inline]
    fn visit_class_elements(&mut self, it: &'a Vec<'a, ClassElement<'a>>) {
        walk_class_elements(self, it);
    }

    #[inline]
    fn visit_import_declaration_specifiers(
        &mut self,
        it: &'a Vec<'a, ImportDeclarationSpecifier<'a>>,
    ) {
        walk_import_declaration_specifiers(self, it);
    }

    #[inline]
    fn visit_import_attributes(&mut self, it: &'a Vec<'a, ImportAttribute<'a>>) {
        walk_import_attributes(self, it);
    }

    #[inline]
    fn visit_export_specifiers(&mut self, it: &'a Vec<'a, ExportSpecifier<'a>>) {
        walk_export_specifiers(self, it);
    }

    #[inline]
    fn visit_jsx_children(&mut self, it: &'a Vec<'a, JSXChild<'a>>) {
        walk_jsx_children(self, it);
    }

    #[inline]
    fn visit_jsx_attribute_items(&mut self, it: &'a Vec<'a, JSXAttributeItem<'a>>) {
        walk_jsx_attribute_items(self, it);
    }

    #[inline]
    fn visit_ts_enum_members(&mut self, it: &'a Vec<'a, TSEnumMember<'a>>) {
        walk_ts_enum_members(self, it);
    }

    #[inline]
    fn visit_ts_types(&mut self, it: &'a Vec<'a, TSType<'a>>) {
        walk_ts_types(self, it);
    }

    #[inline]
    fn visit_ts_tuple_elements(&mut self, it: &'a Vec<'a, TSTupleElement<'a>>) {
        walk_ts_tuple_elements(self, it);
    }

    #[inline]
    fn visit_ts_type_parameters(&mut self, it: &'a Vec<'a, TSTypeParameter<'a>>) {
        walk_ts_type_parameters(self, it);
    }

    #[inline]
    fn visit_ts_interface_heritages(&mut self, it: &'a Vec<'a, TSInterfaceHeritage<'a>>) {
        walk_ts_interface_heritages(self, it);
    }

    #[inline]
    fn visit_ts_signatures(&mut self, it: &'a Vec<'a, TSSignature<'a>>) {
        walk_ts_signatures(self, it);
    }

    #[inline]
    fn visit_ts_index_signature_names(&mut self, it: &'a Vec<'a, TSIndexSignatureName<'a>>) {
        walk_ts_index_signature_names(self, it);
    }

    #[inline]
    fn visit_spans(&mut self, it: &'a Vec<'a, Span>) {
        walk_spans(self, it);
    }
}

pub mod walk {
    use super::*;

    macro_rules! walk_expression_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                $ty::BooleanLiteral(it) => $visitor.visit_boolean_literal(it),
                $ty::NullLiteral(it) => $visitor.visit_null_literal(it),
                $ty::NumericLiteral(it) => $visitor.visit_numeric_literal(it),
                $ty::BigIntLiteral(it) => $visitor.visit_big_int_literal(it),
                $ty::RegExpLiteral(it) => $visitor.visit_reg_exp_literal(it),
                $ty::StringLiteral(it) => $visitor.visit_string_literal(it),
                $ty::TemplateLiteral(it) => $visitor.visit_template_literal(it),
                $ty::Identifier(it) => $visitor.visit_identifier_reference(it),
                $ty::MetaProperty(it) => $visitor.visit_meta_property(it),
                $ty::Super(it) => $visitor.visit_super(it),
                $ty::ArrayExpression(it) => $visitor.visit_array_expression(it),
                $ty::ArrowFunctionExpression(it) => $visitor.visit_arrow_function_expression(it),
                $ty::AssignmentExpression(it) => $visitor.visit_assignment_expression(it),
                $ty::AwaitExpression(it) => $visitor.visit_await_expression(it),
                $ty::BinaryExpression(it) => $visitor.visit_binary_expression(it),
                $ty::CallExpression(it) => $visitor.visit_call_expression(it),
                $ty::ChainExpression(it) => $visitor.visit_chain_expression(it),
                $ty::ClassExpression(it) => $visitor.visit_class(it),
                $ty::ConditionalExpression(it) => $visitor.visit_conditional_expression(it),
                $ty::FunctionExpression(it) => {
                    let flags = ScopeFlags::Function;
                    $visitor.visit_function(it, flags)
                }
                $ty::ImportExpression(it) => $visitor.visit_import_expression(it),
                $ty::LogicalExpression(it) => $visitor.visit_logical_expression(it),
                $ty::NewExpression(it) => $visitor.visit_new_expression(it),
                $ty::ObjectExpression(it) => $visitor.visit_object_expression(it),
                $ty::ParenthesizedExpression(it) => $visitor.visit_parenthesized_expression(it),
                $ty::SequenceExpression(it) => $visitor.visit_sequence_expression(it),
                $ty::TaggedTemplateExpression(it) => {
                    $visitor.visit_tagged_template_expression(it)
                }
                $ty::ThisExpression(it) => $visitor.visit_this_expression(it),
                $ty::UnaryExpression(it) => $visitor.visit_unary_expression(it),
                $ty::UpdateExpression(it) => $visitor.visit_update_expression(it),
                $ty::YieldExpression(it) => $visitor.visit_yield_expression(it),
                $ty::PrivateInExpression(it) => $visitor.visit_private_in_expression(it),
                $ty::JSXElement(it) => $visitor.visit_jsx_element(it),
                $ty::JSXFragment(it) => $visitor.visit_jsx_fragment(it),
                $ty::TSAsExpression(it) => $visitor.visit_ts_as_expression(it),
                $ty::TSSatisfiesExpression(it) => $visitor.visit_ts_satisfies_expression(it),
                $ty::TSTypeAssertion(it) => $visitor.visit_ts_type_assertion(it),
                $ty::TSNonNullExpression(it) => $visitor.visit_ts_non_null_expression(it),
                $ty::TSInstantiationExpression(it) => {
                    $visitor.visit_ts_instantiation_expression(it)
                }
                $ty::V8IntrinsicExpression(it) => $visitor.visit_v8_intrinsic_expression(it),
                match_member_expression!($ty) => {
                    walk_member_expression_variants!($visitor, $ty, $it)
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    macro_rules! walk_member_expression_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                $ty::ComputedMemberExpression(it) => {
                    $visitor.visit_computed_member_expression(it)
                }
                $ty::StaticMemberExpression(it) => {
                    $visitor.visit_static_member_expression(it)
                }
                $ty::PrivateFieldExpression(it) => {
                    $visitor.visit_private_field_expression(it)
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    macro_rules! walk_simple_assignment_target_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                $ty::AssignmentTargetIdentifier(it) => {
                    $visitor.visit_identifier_reference(it)
                }
                $ty::TSAsExpression(it) => $visitor.visit_ts_as_expression(it),
                $ty::TSSatisfiesExpression(it) => {
                    $visitor.visit_ts_satisfies_expression(it)
                }
                $ty::TSNonNullExpression(it) => {
                    $visitor.visit_ts_non_null_expression(it)
                }
                $ty::TSTypeAssertion(it) => $visitor.visit_ts_type_assertion(it),
                match_member_expression!($ty) => {
                    walk_member_expression_variants!($visitor, $ty, $it)
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    macro_rules! walk_assignment_target_pattern_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                $ty::ArrayAssignmentTarget(it) => {
                    $visitor.visit_array_assignment_target(it)
                }
                $ty::ObjectAssignmentTarget(it) => {
                    $visitor.visit_object_assignment_target(it)
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    macro_rules! walk_assignment_target_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                match_simple_assignment_target!($ty) => {
                    walk_simple_assignment_target_variants!($visitor, $ty, $it)
                }
                match_assignment_target_pattern!($ty) => {
                    walk_assignment_target_pattern_variants!($visitor, $ty, $it)
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    macro_rules! walk_declaration_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                $ty::VariableDeclaration(it) => $visitor.visit_variable_declaration(it),
                $ty::FunctionDeclaration(it) => {
                    let flags = ScopeFlags::Function;
                    $visitor.visit_function(it, flags)
                }
                $ty::ClassDeclaration(it) => $visitor.visit_class(it),
                $ty::TSTypeAliasDeclaration(it) => $visitor.visit_ts_type_alias_declaration(it),
                $ty::TSInterfaceDeclaration(it) => $visitor.visit_ts_interface_declaration(it),
                $ty::TSEnumDeclaration(it) => $visitor.visit_ts_enum_declaration(it),
                $ty::TSModuleDeclaration(it) => $visitor.visit_ts_module_declaration(it),
                $ty::TSGlobalDeclaration(it) => $visitor.visit_ts_global_declaration(it),
                $ty::TSImportEqualsDeclaration(it) => {
                    $visitor.visit_ts_import_equals_declaration(it)
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    macro_rules! walk_module_declaration_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                $ty::ImportDeclaration(it) => $visitor.visit_import_declaration(it),
                $ty::ExportAllDeclaration(it) => $visitor.visit_export_all_declaration(it),
                $ty::ExportDefaultDeclaration(it) => {
                    $visitor.visit_export_default_declaration(it)
                }
                $ty::ExportNamedDeclaration(it) => {
                    $visitor.visit_export_named_declaration(it)
                }
                $ty::TSExportAssignment(it) => $visitor.visit_ts_export_assignment(it),
                $ty::TSNamespaceExportDeclaration(it) => {
                    $visitor.visit_ts_namespace_export_declaration(it)
                }
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    macro_rules! walk_ts_type_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                $ty::TSAnyKeyword(it) => $visitor.visit_ts_any_keyword(it),
                $ty::TSBigIntKeyword(it) => $visitor.visit_ts_big_int_keyword(it),
                $ty::TSBooleanKeyword(it) => $visitor.visit_ts_boolean_keyword(it),
                $ty::TSIntrinsicKeyword(it) => $visitor.visit_ts_intrinsic_keyword(it),
                $ty::TSNeverKeyword(it) => $visitor.visit_ts_never_keyword(it),
                $ty::TSNullKeyword(it) => $visitor.visit_ts_null_keyword(it),
                $ty::TSNumberKeyword(it) => $visitor.visit_ts_number_keyword(it),
                $ty::TSObjectKeyword(it) => $visitor.visit_ts_object_keyword(it),
                $ty::TSStringKeyword(it) => $visitor.visit_ts_string_keyword(it),
                $ty::TSSymbolKeyword(it) => $visitor.visit_ts_symbol_keyword(it),
                $ty::TSUndefinedKeyword(it) => $visitor.visit_ts_undefined_keyword(it),
                $ty::TSUnknownKeyword(it) => $visitor.visit_ts_unknown_keyword(it),
                $ty::TSVoidKeyword(it) => $visitor.visit_ts_void_keyword(it),
                $ty::TSArrayType(it) => $visitor.visit_ts_array_type(it),
                $ty::TSConditionalType(it) => $visitor.visit_ts_conditional_type(it),
                $ty::TSConstructorType(it) => $visitor.visit_ts_constructor_type(it),
                $ty::TSFunctionType(it) => $visitor.visit_ts_function_type(it),
                $ty::TSImportType(it) => $visitor.visit_ts_import_type(it),
                $ty::TSIndexedAccessType(it) => $visitor.visit_ts_indexed_access_type(it),
                $ty::TSInferType(it) => $visitor.visit_ts_infer_type(it),
                $ty::TSIntersectionType(it) => $visitor.visit_ts_intersection_type(it),
                $ty::TSLiteralType(it) => $visitor.visit_ts_literal_type(it),
                $ty::TSMappedType(it) => $visitor.visit_ts_mapped_type(it),
                $ty::TSNamedTupleMember(it) => $visitor.visit_ts_named_tuple_member(it),
                $ty::TSTemplateLiteralType(it) => $visitor.visit_ts_template_literal_type(it),
                $ty::TSThisType(it) => $visitor.visit_ts_this_type(it),
                $ty::TSTupleType(it) => $visitor.visit_ts_tuple_type(it),
                $ty::TSTypeLiteral(it) => $visitor.visit_ts_type_literal(it),
                $ty::TSTypeOperatorType(it) => $visitor.visit_ts_type_operator(it),
                $ty::TSTypePredicate(it) => $visitor.visit_ts_type_predicate(it),
                $ty::TSTypeQuery(it) => $visitor.visit_ts_type_query(it),
                $ty::TSTypeReference(it) => $visitor.visit_ts_type_reference(it),
                $ty::TSUnionType(it) => $visitor.visit_ts_union_type(it),
                $ty::TSParenthesizedType(it) => $visitor.visit_ts_parenthesized_type(it),
                $ty::JSDocNullableType(it) => $visitor.visit_js_doc_nullable_type(it),
                $ty::JSDocNonNullableType(it) => $visitor.visit_js_doc_non_nullable_type(it),
                $ty::JSDocUnknownType(it) => $visitor.visit_js_doc_unknown_type(it),
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    macro_rules! walk_ts_type_name_variants {
        ($visitor:ident, $ty:ident, $it:ident) => {
            match $it {
                $ty::IdentifierReference(it) => $visitor.visit_identifier_reference(it),
                $ty::QualifiedName(it) => $visitor.visit_ts_qualified_name(it),
                $ty::ThisExpression(it) => $visitor.visit_this_expression(it),
                #[allow(unreachable_patterns)]
                _ => unreachable!(),
            }
        };
    }

    #[inline]
    pub fn walk_program<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Program<'a>) {
        let kind = AstKind::Program(it);
        visitor.enter_node(kind);
        visitor.enter_scope(
            {
                let mut flags = ScopeFlags::Top;
                if it.source_type.is_strict() || it.has_use_strict_directive() {
                    flags |= ScopeFlags::StrictMode;
                }
                flags
            },
            &it.scope_id,
        );
        visitor.visit_span(&it.span);
        if let Some(hashbang) = &it.hashbang {
            visitor.visit_hashbang(hashbang);
        }
        visitor.visit_directives(&it.directives);
        visitor.visit_statements(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    pub fn walk_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Expression<'a>) {
        // No `AstKind` for this type
        walk_expression_variants!(visitor, Expression, it);
    }

    #[inline]
    pub fn walk_identifier_name<'a, V: Visit<'a>>(visitor: &mut V, it: &'a IdentifierName<'a>) {
        let kind = AstKind::IdentifierName(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_identifier_reference<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a IdentifierReference<'a>,
    ) {
        let kind = AstKind::IdentifierReference(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_binding_identifier<'a, V: Visit<'a>>(visitor: &mut V, it: &'a BindingIdentifier<'a>) {
        let kind = AstKind::BindingIdentifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_label_identifier<'a, V: Visit<'a>>(visitor: &mut V, it: &'a LabelIdentifier<'a>) {
        let kind = AstKind::LabelIdentifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_this_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ThisExpression) {
        let kind = AstKind::ThisExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_array_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ArrayExpression<'a>) {
        let kind = AstKind::ArrayExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_array_expression_elements(&it.elements);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_array_expression_element<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ArrayExpressionElement<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            ArrayExpressionElement::SpreadElement(it) => visitor.visit_spread_element(it),
            ArrayExpressionElement::Elision(it) => visitor.visit_elision(it),
            match_expression!(ArrayExpressionElement) => {
                walk_expression_variants!(visitor, ArrayExpressionElement, it)
            }
        }
    }

    #[inline]
    pub fn walk_elision<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Elision) {
        let kind = AstKind::Elision(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_object_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ObjectExpression<'a>) {
        let kind = AstKind::ObjectExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_object_property_kinds(&it.properties);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_object_property_kind<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ObjectPropertyKind<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            ObjectPropertyKind::ObjectProperty(it) => visitor.visit_object_property(it),
            ObjectPropertyKind::SpreadProperty(it) => visitor.visit_spread_element(it),
        }
    }

    #[inline]
    pub fn walk_object_property<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ObjectProperty<'a>) {
        let kind = AstKind::ObjectProperty(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_property_key(&it.key);
        visitor.visit_expression(&it.value);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_property_key<'a, V: Visit<'a>>(visitor: &mut V, it: &'a PropertyKey<'a>) {
        // No `AstKind` for this type
        match it {
            PropertyKey::StaticIdentifier(it) => visitor.visit_identifier_name(it),
            PropertyKey::PrivateIdentifier(it) => visitor.visit_private_identifier(it),
            match_expression!(PropertyKey) => walk_expression_variants!(visitor, PropertyKey, it),
        }
    }

    #[inline]
    pub fn walk_template_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TemplateLiteral<'a>) {
        let kind = AstKind::TemplateLiteral(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_template_elements(&it.quasis);
        visitor.visit_expressions(&it.expressions);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_tagged_template_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TaggedTemplateExpression<'a>,
    ) {
        let kind = AstKind::TaggedTemplateExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.tag);
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.visit_template_literal(&it.quasi);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_template_element<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TemplateElement<'a>) {
        let kind = AstKind::TemplateElement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_member_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a MemberExpression<'a>) {
        // No `AstKind` for this type
        walk_member_expression_variants!(visitor, MemberExpression, it);
    }

    #[inline]
    pub fn walk_computed_member_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ComputedMemberExpression<'a>,
    ) {
        let kind = AstKind::ComputedMemberExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.object);
        visitor.visit_expression(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_static_member_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a StaticMemberExpression<'a>,
    ) {
        let kind = AstKind::StaticMemberExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.object);
        visitor.visit_identifier_name(&it.property);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_private_field_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a PrivateFieldExpression<'a>,
    ) {
        let kind = AstKind::PrivateFieldExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.object);
        visitor.visit_private_identifier(&it.field);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_call_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a CallExpression<'a>) {
        let kind = AstKind::CallExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.callee);
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.visit_arguments(&it.arguments);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_new_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a NewExpression<'a>) {
        let kind = AstKind::NewExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.callee);
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.visit_arguments(&it.arguments);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_meta_property<'a, V: Visit<'a>>(visitor: &mut V, it: &'a MetaProperty<'a>) {
        let kind = AstKind::MetaProperty(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_identifier_name(&it.meta);
        visitor.visit_identifier_name(&it.property);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_spread_element<'a, V: Visit<'a>>(visitor: &mut V, it: &'a SpreadElement<'a>) {
        let kind = AstKind::SpreadElement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.argument);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_argument<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Argument<'a>) {
        // No `AstKind` for this type
        match it {
            Argument::SpreadElement(it) => visitor.visit_spread_element(it),
            match_expression!(Argument) => walk_expression_variants!(visitor, Argument, it),
        }
    }

    #[inline]
    pub fn walk_update_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a UpdateExpression<'a>) {
        let kind = AstKind::UpdateExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_simple_assignment_target(&it.argument);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_unary_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a UnaryExpression<'a>) {
        let kind = AstKind::UnaryExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.argument);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_binary_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a BinaryExpression<'a>) {
        let kind = AstKind::BinaryExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.left);
        visitor.visit_expression(&it.right);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_private_in_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a PrivateInExpression<'a>,
    ) {
        let kind = AstKind::PrivateInExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_private_identifier(&it.left);
        visitor.visit_expression(&it.right);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_logical_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a LogicalExpression<'a>) {
        let kind = AstKind::LogicalExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.left);
        visitor.visit_expression(&it.right);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_conditional_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ConditionalExpression<'a>,
    ) {
        let kind = AstKind::ConditionalExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.test);
        visitor.visit_expression(&it.consequent);
        visitor.visit_expression(&it.alternate);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_assignment_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a AssignmentExpression<'a>,
    ) {
        let kind = AstKind::AssignmentExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_assignment_target(&it.left);
        visitor.visit_expression(&it.right);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_assignment_target<'a, V: Visit<'a>>(visitor: &mut V, it: &'a AssignmentTarget<'a>) {
        // No `AstKind` for this type
        walk_assignment_target_variants!(visitor, AssignmentTarget, it);
    }

    pub fn walk_simple_assignment_target<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a SimpleAssignmentTarget<'a>,
    ) {
        // No `AstKind` for this type
        walk_simple_assignment_target_variants!(visitor, SimpleAssignmentTarget, it);
    }

    #[inline]
    pub fn walk_assignment_target_pattern<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a AssignmentTargetPattern<'a>,
    ) {
        // No `AstKind` for this type
        walk_assignment_target_pattern_variants!(visitor, AssignmentTargetPattern, it);
    }

    #[inline]
    pub fn walk_array_assignment_target<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ArrayAssignmentTarget<'a>,
    ) {
        let kind = AstKind::ArrayAssignmentTarget(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        for el in it.elements.iter().flatten() {
            visitor.visit_assignment_target_maybe_default(el);
        }
        if let Some(rest) = &it.rest {
            visitor.visit_assignment_target_rest(rest);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_object_assignment_target<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ObjectAssignmentTarget<'a>,
    ) {
        let kind = AstKind::ObjectAssignmentTarget(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_assignment_target_properties(&it.properties);
        if let Some(rest) = &it.rest {
            visitor.visit_assignment_target_rest(rest);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_assignment_target_rest<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a AssignmentTargetRest<'a>,
    ) {
        let kind = AstKind::AssignmentTargetRest(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_assignment_target(&it.target);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_assignment_target_maybe_default<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a AssignmentTargetMaybeDefault<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(it) => {
                visitor.visit_assignment_target_with_default(it)
            }
            match_assignment_target!(AssignmentTargetMaybeDefault) => {
                walk_assignment_target_variants!(visitor, AssignmentTargetMaybeDefault, it)
            }
        }
    }

    #[inline]
    pub fn walk_assignment_target_with_default<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a AssignmentTargetWithDefault<'a>,
    ) {
        let kind = AstKind::AssignmentTargetWithDefault(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_assignment_target(&it.binding);
        visitor.visit_expression(&it.init);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_assignment_target_property<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a AssignmentTargetProperty<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(it) => {
                visitor.visit_assignment_target_property_identifier(it)
            }
            AssignmentTargetProperty::AssignmentTargetPropertyProperty(it) => {
                visitor.visit_assignment_target_property_property(it)
            }
        }
    }

    #[inline]
    pub fn walk_assignment_target_property_identifier<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a AssignmentTargetPropertyIdentifier<'a>,
    ) {
        let kind = AstKind::AssignmentTargetPropertyIdentifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_identifier_reference(&it.binding);
        if let Some(init) = &it.init {
            visitor.visit_expression(init);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_assignment_target_property_property<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a AssignmentTargetPropertyProperty<'a>,
    ) {
        let kind = AstKind::AssignmentTargetPropertyProperty(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_property_key(&it.name);
        visitor.visit_assignment_target_maybe_default(&it.binding);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_sequence_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a SequenceExpression<'a>,
    ) {
        let kind = AstKind::SequenceExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expressions(&it.expressions);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_super<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Super) {
        let kind = AstKind::Super(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_await_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a AwaitExpression<'a>) {
        let kind = AstKind::AwaitExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.argument);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_chain_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ChainExpression<'a>) {
        let kind = AstKind::ChainExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_chain_element(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_chain_element<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ChainElement<'a>) {
        // No `AstKind` for this type
        match it {
            ChainElement::CallExpression(it) => visitor.visit_call_expression(it),
            ChainElement::TSNonNullExpression(it) => visitor.visit_ts_non_null_expression(it),
            match_member_expression!(ChainElement) => {
                walk_member_expression_variants!(visitor, ChainElement, it)
            }
        }
    }

    #[inline]
    pub fn walk_parenthesized_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ParenthesizedExpression<'a>,
    ) {
        let kind = AstKind::ParenthesizedExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.leave_node(kind);
    }

    pub fn walk_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Statement<'a>) {
        // No `AstKind` for this type
        match it {
            Statement::BlockStatement(it) => visitor.visit_block_statement(it),
            Statement::BreakStatement(it) => visitor.visit_break_statement(it),
            Statement::ContinueStatement(it) => visitor.visit_continue_statement(it),
            Statement::DebuggerStatement(it) => visitor.visit_debugger_statement(it),
            Statement::DoWhileStatement(it) => visitor.visit_do_while_statement(it),
            Statement::EmptyStatement(it) => visitor.visit_empty_statement(it),
            Statement::ExpressionStatement(it) => visitor.visit_expression_statement(it),
            Statement::ForInStatement(it) => visitor.visit_for_in_statement(it),
            Statement::ForOfStatement(it) => visitor.visit_for_of_statement(it),
            Statement::ForStatement(it) => visitor.visit_for_statement(it),
            Statement::IfStatement(it) => visitor.visit_if_statement(it),
            Statement::LabeledStatement(it) => visitor.visit_labeled_statement(it),
            Statement::ReturnStatement(it) => visitor.visit_return_statement(it),
            Statement::SwitchStatement(it) => visitor.visit_switch_statement(it),
            Statement::ThrowStatement(it) => visitor.visit_throw_statement(it),
            Statement::TryStatement(it) => visitor.visit_try_statement(it),
            Statement::WhileStatement(it) => visitor.visit_while_statement(it),
            Statement::WithStatement(it) => visitor.visit_with_statement(it),
            match_declaration!(Statement) => walk_declaration_variants!(visitor, Statement, it),
            match_module_declaration!(Statement) => {
                walk_module_declaration_variants!(visitor, Statement, it)
            }
        }
    }

    #[inline]
    pub fn walk_directive<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Directive<'a>) {
        let kind = AstKind::Directive(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_string_literal(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_hashbang<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Hashbang<'a>) {
        let kind = AstKind::Hashbang(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_block_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a BlockStatement<'a>) {
        let kind = AstKind::BlockStatement(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        visitor.visit_statements(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    pub fn walk_declaration<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Declaration<'a>) {
        // No `AstKind` for this type
        walk_declaration_variants!(visitor, Declaration, it);
    }

    #[inline]
    pub fn walk_variable_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a VariableDeclaration<'a>,
    ) {
        let kind = AstKind::VariableDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_variable_declarators(&it.declarations);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_variable_declarator<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a VariableDeclarator<'a>,
    ) {
        let kind = AstKind::VariableDeclarator(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_pattern(&it.id);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        if let Some(init) = &it.init {
            visitor.visit_expression(init);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_empty_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a EmptyStatement) {
        let kind = AstKind::EmptyStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_expression_statement<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ExpressionStatement<'a>,
    ) {
        let kind = AstKind::ExpressionStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_if_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a IfStatement<'a>) {
        let kind = AstKind::IfStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.test);
        visitor.visit_statement(&it.consequent);
        if let Some(alternate) = &it.alternate {
            visitor.visit_statement(alternate);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_do_while_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a DoWhileStatement<'a>) {
        let kind = AstKind::DoWhileStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_statement(&it.body);
        visitor.visit_expression(&it.test);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_while_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a WhileStatement<'a>) {
        let kind = AstKind::WhileStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.test);
        visitor.visit_statement(&it.body);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_for_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ForStatement<'a>) {
        let kind = AstKind::ForStatement(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        if let Some(init) = &it.init {
            visitor.visit_for_statement_init(init);
        }
        if let Some(test) = &it.test {
            visitor.visit_expression(test);
        }
        if let Some(update) = &it.update {
            visitor.visit_expression(update);
        }
        visitor.visit_statement(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_for_statement_init<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ForStatementInit<'a>) {
        // No `AstKind` for this type
        match it {
            ForStatementInit::VariableDeclaration(it) => visitor.visit_variable_declaration(it),
            match_expression!(ForStatementInit) => walk_expression_variants!(visitor, ForStatementInit, it),
        }
    }

    #[inline]
    pub fn walk_for_in_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ForInStatement<'a>) {
        let kind = AstKind::ForInStatement(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        visitor.visit_for_statement_left(&it.left);
        visitor.visit_expression(&it.right);
        visitor.visit_statement(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_for_statement_left<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ForStatementLeft<'a>) {
        // No `AstKind` for this type
        match it {
            ForStatementLeft::VariableDeclaration(it) => visitor.visit_variable_declaration(it),
            match_assignment_target!(ForStatementLeft) => {
                walk_assignment_target_variants!(visitor, ForStatementLeft, it)
            }
        }
    }

    #[inline]
    pub fn walk_for_of_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ForOfStatement<'a>) {
        let kind = AstKind::ForOfStatement(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        visitor.visit_for_statement_left(&it.left);
        visitor.visit_expression(&it.right);
        visitor.visit_statement(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_continue_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ContinueStatement<'a>) {
        let kind = AstKind::ContinueStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        if let Some(label) = &it.label {
            visitor.visit_label_identifier(label);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_break_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a BreakStatement<'a>) {
        let kind = AstKind::BreakStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        if let Some(label) = &it.label {
            visitor.visit_label_identifier(label);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_return_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ReturnStatement<'a>) {
        let kind = AstKind::ReturnStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        if let Some(argument) = &it.argument {
            visitor.visit_expression(argument);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_with_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a WithStatement<'a>) {
        let kind = AstKind::WithStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.object);
        visitor.enter_scope(ScopeFlags::With, &it.scope_id);
        visitor.visit_statement(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_switch_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a SwitchStatement<'a>) {
        let kind = AstKind::SwitchStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.discriminant);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_switch_cases(&it.cases);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_switch_case<'a, V: Visit<'a>>(visitor: &mut V, it: &'a SwitchCase<'a>) {
        let kind = AstKind::SwitchCase(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        if let Some(test) = &it.test {
            visitor.visit_expression(test);
        }
        visitor.visit_statements(&it.consequent);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_labeled_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a LabeledStatement<'a>) {
        let kind = AstKind::LabeledStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_label_identifier(&it.label);
        visitor.visit_statement(&it.body);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_throw_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ThrowStatement<'a>) {
        let kind = AstKind::ThrowStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.argument);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_try_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TryStatement<'a>) {
        let kind = AstKind::TryStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_block_statement(&it.block);
        if let Some(handler) = &it.handler {
            visitor.visit_catch_clause(handler);
        }
        if let Some(finalizer) = &it.finalizer {
            visitor.visit_block_statement(finalizer);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_catch_clause<'a, V: Visit<'a>>(visitor: &mut V, it: &'a CatchClause<'a>) {
        let kind = AstKind::CatchClause(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::CatchClause, &it.scope_id);
        visitor.visit_span(&it.span);
        if let Some(param) = &it.param {
            visitor.visit_catch_parameter(param);
        }
        visitor.visit_block_statement(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_catch_parameter<'a, V: Visit<'a>>(visitor: &mut V, it: &'a CatchParameter<'a>) {
        let kind = AstKind::CatchParameter(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_pattern(&it.pattern);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_debugger_statement<'a, V: Visit<'a>>(visitor: &mut V, it: &'a DebuggerStatement) {
        let kind = AstKind::DebuggerStatement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_binding_pattern<'a, V: Visit<'a>>(visitor: &mut V, it: &'a BindingPattern<'a>) {
        // No `AstKind` for this type
        match it {
            BindingPattern::BindingIdentifier(it) => visitor.visit_binding_identifier(it),
            BindingPattern::ObjectPattern(it) => visitor.visit_object_pattern(it),
            BindingPattern::ArrayPattern(it) => visitor.visit_array_pattern(it),
            BindingPattern::AssignmentPattern(it) => visitor.visit_assignment_pattern(it),
        }
    }

    #[inline]
    pub fn walk_assignment_pattern<'a, V: Visit<'a>>(visitor: &mut V, it: &'a AssignmentPattern<'a>) {
        let kind = AstKind::AssignmentPattern(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_pattern(&it.left);
        visitor.visit_expression(&it.right);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_object_pattern<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ObjectPattern<'a>) {
        let kind = AstKind::ObjectPattern(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_properties(&it.properties);
        if let Some(rest) = &it.rest {
            visitor.visit_binding_rest_element(rest);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_binding_property<'a, V: Visit<'a>>(visitor: &mut V, it: &'a BindingProperty<'a>) {
        let kind = AstKind::BindingProperty(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_property_key(&it.key);
        visitor.visit_binding_pattern(&it.value);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_array_pattern<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ArrayPattern<'a>) {
        let kind = AstKind::ArrayPattern(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        for el in it.elements.iter().flatten() {
            visitor.visit_binding_pattern(el);
        }
        if let Some(rest) = &it.rest {
            visitor.visit_binding_rest_element(rest);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_binding_rest_element<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a BindingRestElement<'a>,
    ) {
        let kind = AstKind::BindingRestElement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_pattern(&it.argument);
        visitor.leave_node(kind);
    }

    pub fn walk_function<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Function<'a>, flags: ScopeFlags) {
        let kind = AstKind::Function(it);
        visitor.enter_node(kind);
        visitor.enter_scope(
            {
                let mut flags = flags;
                if it.has_use_strict_directive() {
                    flags |= ScopeFlags::StrictMode;
                }
                flags
            },
            &it.scope_id,
        );
        visitor.visit_span(&it.span);
        if let Some(id) = &it.id {
            visitor.visit_binding_identifier(id);
        }
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(this_param) = &it.this_param {
            visitor.visit_ts_this_parameter(this_param);
        }
        visitor.visit_formal_parameters(&it.params);
        if let Some(return_type) = &it.return_type {
            visitor.visit_ts_type_annotation(return_type);
        }
        if let Some(body) = &it.body {
            visitor.visit_function_body(body);
        }
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_formal_parameters<'a, V: Visit<'a>>(visitor: &mut V, it: &'a FormalParameters<'a>) {
        let kind = AstKind::FormalParameters(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_formal_parameter_list(&it.items);
        if let Some(rest) = &it.rest {
            visitor.visit_formal_parameter_rest(rest);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_formal_parameter<'a, V: Visit<'a>>(visitor: &mut V, it: &'a FormalParameter<'a>) {
        let kind = AstKind::FormalParameter(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_decorators(&it.decorators);
        visitor.visit_binding_pattern(&it.pattern);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        if let Some(initializer) = &it.initializer {
            visitor.visit_expression(initializer);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_formal_parameter_rest<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a FormalParameterRest<'a>,
    ) {
        let kind = AstKind::FormalParameterRest(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_decorators(&it.decorators);
        visitor.visit_binding_rest_element(&it.rest);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_function_body<'a, V: Visit<'a>>(visitor: &mut V, it: &'a FunctionBody<'a>) {
        let kind = AstKind::FunctionBody(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_directives(&it.directives);
        visitor.visit_statements(&it.statements);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_arrow_function_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ArrowFunctionExpression<'a>,
    ) {
        let kind = AstKind::ArrowFunctionExpression(it);
        visitor.enter_node(kind);
        visitor.enter_scope(
            {
                let mut flags = ScopeFlags::Function | ScopeFlags::Arrow;
                if it.has_use_strict_directive() {
                    flags |= ScopeFlags::StrictMode;
                }
                flags
            },
            &it.scope_id,
        );
        visitor.visit_span(&it.span);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        visitor.visit_formal_parameters(&it.params);
        if let Some(return_type) = &it.return_type {
            visitor.visit_ts_type_annotation(return_type);
        }
        visitor.visit_function_body(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_yield_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a YieldExpression<'a>) {
        let kind = AstKind::YieldExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        if let Some(argument) = &it.argument {
            visitor.visit_expression(argument);
        }
        visitor.leave_node(kind);
    }

    pub fn walk_class<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Class<'a>) {
        let kind = AstKind::Class(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_decorators(&it.decorators);
        if let Some(id) = &it.id {
            visitor.visit_binding_identifier(id);
        }
        visitor.enter_scope(ScopeFlags::StrictMode, &it.scope_id);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(super_class) = &it.super_class {
            visitor.visit_expression(super_class);
        }
        if let Some(super_type_arguments) = &it.super_type_arguments {
            visitor.visit_ts_type_parameter_instantiation(super_type_arguments);
        }
        visitor.visit_ts_class_implements_list(&it.implements);
        visitor.visit_class_body(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_class_body<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ClassBody<'a>) {
        let kind = AstKind::ClassBody(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_class_elements(&it.body);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_class_element<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ClassElement<'a>) {
        // No `AstKind` for this type
        match it {
            ClassElement::StaticBlock(it) => visitor.visit_static_block(it),
            ClassElement::MethodDefinition(it) => visitor.visit_method_definition(it),
            ClassElement::PropertyDefinition(it) => visitor.visit_property_definition(it),
            ClassElement::AccessorProperty(it) => visitor.visit_accessor_property(it),
            ClassElement::TSIndexSignature(it) => visitor.visit_ts_index_signature(it),
        }
    }

    #[inline]
    pub fn walk_method_definition<'a, V: Visit<'a>>(visitor: &mut V, it: &'a MethodDefinition<'a>) {
        let kind = AstKind::MethodDefinition(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_decorators(&it.decorators);
        visitor.visit_property_key(&it.key);
        {
            let flags = match it.kind {
                MethodDefinitionKind::Get => ScopeFlags::Function | ScopeFlags::GetAccessor,
                MethodDefinitionKind::Set => ScopeFlags::Function | ScopeFlags::SetAccessor,
                MethodDefinitionKind::Constructor => ScopeFlags::Function | ScopeFlags::Constructor,
                MethodDefinitionKind::Method => ScopeFlags::Function,
            };
            visitor.visit_function(&it.value, flags);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_property_definition<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a PropertyDefinition<'a>,
    ) {
        let kind = AstKind::PropertyDefinition(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_decorators(&it.decorators);
        visitor.visit_property_key(&it.key);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        if let Some(value) = &it.value {
            visitor.visit_expression(value);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_private_identifier<'a, V: Visit<'a>>(visitor: &mut V, it: &'a PrivateIdentifier<'a>) {
        let kind = AstKind::PrivateIdentifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_static_block<'a, V: Visit<'a>>(visitor: &mut V, it: &'a StaticBlock<'a>) {
        let kind = AstKind::StaticBlock(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::ClassStaticBlock, &it.scope_id);
        visitor.visit_span(&it.span);
        visitor.visit_statements(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    pub fn walk_module_declaration<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ModuleDeclaration<'a>) {
        // No `AstKind` for this type
        walk_module_declaration_variants!(visitor, ModuleDeclaration, it);
    }

    #[inline]
    pub fn walk_accessor_property<'a, V: Visit<'a>>(visitor: &mut V, it: &'a AccessorProperty<'a>) {
        let kind = AstKind::AccessorProperty(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_decorators(&it.decorators);
        visitor.visit_property_key(&it.key);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        if let Some(value) = &it.value {
            visitor.visit_expression(value);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_import_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ImportExpression<'a>) {
        let kind = AstKind::ImportExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.source);
        if let Some(options) = &it.options {
            visitor.visit_expression(options);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_import_declaration<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ImportDeclaration<'a>) {
        let kind = AstKind::ImportDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        if let Some(specifiers) = &it.specifiers {
            visitor.visit_import_declaration_specifiers(specifiers);
        }
        visitor.visit_string_literal(&it.source);
        if let Some(with_clause) = &it.with_clause {
            visitor.visit_with_clause(with_clause);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_import_declaration_specifier<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ImportDeclarationSpecifier<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            ImportDeclarationSpecifier::ImportSpecifier(it) => visitor.visit_import_specifier(it),
            ImportDeclarationSpecifier::ImportDefaultSpecifier(it) => {
                visitor.visit_import_default_specifier(it)
            }
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(it) => {
                visitor.visit_import_namespace_specifier(it)
            }
        }
    }

    #[inline]
    pub fn walk_import_specifier<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ImportSpecifier<'a>) {
        let kind = AstKind::ImportSpecifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_module_export_name(&it.imported);
        visitor.visit_binding_identifier(&it.local);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_import_default_specifier<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ImportDefaultSpecifier<'a>,
    ) {
        let kind = AstKind::ImportDefaultSpecifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_identifier(&it.local);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_import_namespace_specifier<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ImportNamespaceSpecifier<'a>,
    ) {
        let kind = AstKind::ImportNamespaceSpecifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_identifier(&it.local);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_with_clause<'a, V: Visit<'a>>(visitor: &mut V, it: &'a WithClause<'a>) {
        let kind = AstKind::WithClause(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_import_attributes(&it.with_entries);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_import_attribute<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ImportAttribute<'a>) {
        let kind = AstKind::ImportAttribute(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_import_attribute_key(&it.key);
        visitor.visit_string_literal(&it.value);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_import_attribute_key<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ImportAttributeKey<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            ImportAttributeKey::Identifier(it) => visitor.visit_identifier_name(it),
            ImportAttributeKey::StringLiteral(it) => visitor.visit_string_literal(it),
        }
    }

    #[inline]
    pub fn walk_export_named_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ExportNamedDeclaration<'a>,
    ) {
        let kind = AstKind::ExportNamedDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        if let Some(declaration) = &it.declaration {
            visitor.visit_declaration(declaration);
        }
        visitor.visit_export_specifiers(&it.specifiers);
        if let Some(source) = &it.source {
            visitor.visit_string_literal(source);
        }
        if let Some(with_clause) = &it.with_clause {
            visitor.visit_with_clause(with_clause);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_export_default_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ExportDefaultDeclaration<'a>,
    ) {
        let kind = AstKind::ExportDefaultDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_export_default_declaration_kind(&it.declaration);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_export_all_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ExportAllDeclaration<'a>,
    ) {
        let kind = AstKind::ExportAllDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        if let Some(exported) = &it.exported {
            visitor.visit_module_export_name(exported);
        }
        visitor.visit_string_literal(&it.source);
        if let Some(with_clause) = &it.with_clause {
            visitor.visit_with_clause(with_clause);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_export_specifier<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ExportSpecifier<'a>) {
        let kind = AstKind::ExportSpecifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_module_export_name(&it.local);
        visitor.visit_module_export_name(&it.exported);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_export_default_declaration_kind<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a ExportDefaultDeclarationKind<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            ExportDefaultDeclarationKind::FunctionDeclaration(it) => {
                let flags = ScopeFlags::Function;
                visitor.visit_function(it, flags)
            }
            ExportDefaultDeclarationKind::ClassDeclaration(it) => visitor.visit_class(it),
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(it) => {
                visitor.visit_ts_interface_declaration(it)
            }
            match_expression!(ExportDefaultDeclarationKind) => {
                walk_expression_variants!(visitor, ExportDefaultDeclarationKind, it)
            }
        }
    }

    #[inline]
    pub fn walk_module_export_name<'a, V: Visit<'a>>(visitor: &mut V, it: &'a ModuleExportName<'a>) {
        // No `AstKind` for this type
        match it {
            ModuleExportName::IdentifierName(it) => visitor.visit_identifier_name(it),
            ModuleExportName::IdentifierReference(it) => visitor.visit_identifier_reference(it),
            ModuleExportName::StringLiteral(it) => visitor.visit_string_literal(it),
        }
    }

    #[inline]
    pub fn walk_v8_intrinsic_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a V8IntrinsicExpression<'a>,
    ) {
        let kind = AstKind::V8IntrinsicExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_identifier_name(&it.name);
        visitor.visit_arguments(&it.arguments);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_boolean_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a BooleanLiteral) {
        let kind = AstKind::BooleanLiteral(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_null_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a NullLiteral) {
        let kind = AstKind::NullLiteral(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_numeric_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a NumericLiteral<'a>) {
        let kind = AstKind::NumericLiteral(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_string_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a StringLiteral<'a>) {
        let kind = AstKind::StringLiteral(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_big_int_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a BigIntLiteral<'a>) {
        let kind = AstKind::BigIntLiteral(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_reg_exp_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a RegExpLiteral<'a>) {
        let kind = AstKind::RegExpLiteral(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_element<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXElement<'a>) {
        let kind = AstKind::JSXElement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_jsx_opening_element(&it.opening_element);
        visitor.visit_jsx_children(&it.children);
        if let Some(closing_element) = &it.closing_element {
            visitor.visit_jsx_closing_element(closing_element);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_opening_element<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXOpeningElement<'a>) {
        let kind = AstKind::JSXOpeningElement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_jsx_element_name(&it.name);
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.visit_jsx_attribute_items(&it.attributes);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_closing_element<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXClosingElement<'a>) {
        let kind = AstKind::JSXClosingElement(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_jsx_element_name(&it.name);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_fragment<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXFragment<'a>) {
        let kind = AstKind::JSXFragment(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_jsx_opening_fragment(&it.opening_fragment);
        visitor.visit_jsx_children(&it.children);
        visitor.visit_jsx_closing_fragment(&it.closing_fragment);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_opening_fragment<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXOpeningFragment) {
        let kind = AstKind::JSXOpeningFragment(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_closing_fragment<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXClosingFragment) {
        let kind = AstKind::JSXClosingFragment(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_element_name<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXElementName<'a>) {
        // No `AstKind` for this type
        match it {
            JSXElementName::Identifier(it) => visitor.visit_jsx_identifier(it),
            JSXElementName::IdentifierReference(it) => visitor.visit_identifier_reference(it),
            JSXElementName::NamespacedName(it) => visitor.visit_jsx_namespaced_name(it),
            JSXElementName::MemberExpression(it) => visitor.visit_jsx_member_expression(it),
            JSXElementName::ThisExpression(it) => visitor.visit_this_expression(it),
        }
    }

    #[inline]
    pub fn walk_jsx_namespaced_name<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXNamespacedName<'a>) {
        let kind = AstKind::JSXNamespacedName(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_jsx_identifier(&it.namespace);
        visitor.visit_jsx_identifier(&it.name);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_member_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a JSXMemberExpression<'a>,
    ) {
        let kind = AstKind::JSXMemberExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_jsx_member_expression_object(&it.object);
        visitor.visit_jsx_identifier(&it.property);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_member_expression_object<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a JSXMemberExpressionObject<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            JSXMemberExpressionObject::IdentifierReference(it) => {
                visitor.visit_identifier_reference(it)
            }
            JSXMemberExpressionObject::MemberExpression(it) => {
                visitor.visit_jsx_member_expression(it)
            }
            JSXMemberExpressionObject::ThisExpression(it) => visitor.visit_this_expression(it),
        }
    }

    #[inline]
    pub fn walk_jsx_expression_container<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a JSXExpressionContainer<'a>,
    ) {
        let kind = AstKind::JSXExpressionContainer(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_jsx_expression(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXExpression<'a>) {
        // No `AstKind` for this type
        match it {
            JSXExpression::EmptyExpression(it) => visitor.visit_jsx_empty_expression(it),
            match_expression!(JSXExpression) => walk_expression_variants!(visitor, JSXExpression, it),
        }
    }

    #[inline]
    pub fn walk_jsx_empty_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXEmptyExpression) {
        let kind = AstKind::JSXEmptyExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_attribute_item<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXAttributeItem<'a>) {
        // No `AstKind` for this type
        match it {
            JSXAttributeItem::Attribute(it) => visitor.visit_jsx_attribute(it),
            JSXAttributeItem::SpreadAttribute(it) => visitor.visit_jsx_spread_attribute(it),
        }
    }

    #[inline]
    pub fn walk_jsx_attribute<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXAttribute<'a>) {
        let kind = AstKind::JSXAttribute(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_jsx_attribute_name(&it.name);
        if let Some(value) = &it.value {
            visitor.visit_jsx_attribute_value(value);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_spread_attribute<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a JSXSpreadAttribute<'a>,
    ) {
        let kind = AstKind::JSXSpreadAttribute(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.argument);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_attribute_name<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXAttributeName<'a>) {
        // No `AstKind` for this type
        match it {
            JSXAttributeName::Identifier(it) => visitor.visit_jsx_identifier(it),
            JSXAttributeName::NamespacedName(it) => visitor.visit_jsx_namespaced_name(it),
        }
    }

    #[inline]
    pub fn walk_jsx_attribute_value<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXAttributeValue<'a>) {
        // No `AstKind` for this type
        match it {
            JSXAttributeValue::StringLiteral(it) => visitor.visit_string_literal(it),
            JSXAttributeValue::ExpressionContainer(it) => {
                visitor.visit_jsx_expression_container(it)
            }
            JSXAttributeValue::Element(it) => visitor.visit_jsx_element(it),
            JSXAttributeValue::Fragment(it) => visitor.visit_jsx_fragment(it),
        }
    }

    #[inline]
    pub fn walk_jsx_identifier<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXIdentifier<'a>) {
        let kind = AstKind::JSXIdentifier(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_child<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXChild<'a>) {
        // No `AstKind` for this type
        match it {
            JSXChild::Text(it) => visitor.visit_jsx_text(it),
            JSXChild::Element(it) => visitor.visit_jsx_element(it),
            JSXChild::Fragment(it) => visitor.visit_jsx_fragment(it),
            JSXChild::ExpressionContainer(it) => visitor.visit_jsx_expression_container(it),
            JSXChild::Spread(it) => visitor.visit_jsx_spread_child(it),
        }
    }

    #[inline]
    pub fn walk_jsx_spread_child<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXSpreadChild<'a>) {
        let kind = AstKind::JSXSpreadChild(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_jsx_text<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSXText<'a>) {
        let kind = AstKind::JSXText(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_this_parameter<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSThisParameter<'a>) {
        let kind = AstKind::TSThisParameter(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_span(&it.this_span);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_enum_declaration<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSEnumDeclaration<'a>) {
        let kind = AstKind::TSEnumDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_identifier(&it.id);
        visitor.visit_ts_enum_body(&it.body);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_enum_body<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSEnumBody<'a>) {
        let kind = AstKind::TSEnumBody(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        visitor.visit_ts_enum_members(&it.members);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_enum_member<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSEnumMember<'a>) {
        let kind = AstKind::TSEnumMember(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_enum_member_name(&it.id);
        if let Some(initializer) = &it.initializer {
            visitor.visit_expression(initializer);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_enum_member_name<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSEnumMemberName<'a>) {
        // No `AstKind` for this type
        match it {
            TSEnumMemberName::Identifier(it) => visitor.visit_identifier_name(it),
            TSEnumMemberName::String(it) => visitor.visit_string_literal(it),
            TSEnumMemberName::ComputedString(it) => visitor.visit_string_literal(it),
            TSEnumMemberName::ComputedTemplateString(it) => visitor.visit_template_literal(it),
        }
    }

    #[inline]
    pub fn walk_ts_type_annotation<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypeAnnotation<'a>) {
        let kind = AstKind::TSTypeAnnotation(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_literal_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSLiteralType<'a>) {
        let kind = AstKind::TSLiteralType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_literal(&it.literal);
        visitor.leave_node(kind);
    }

    pub fn walk_ts_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSLiteral<'a>) {
        // No `AstKind` for this type
        match it {
            TSLiteral::BooleanLiteral(it) => visitor.visit_boolean_literal(it),
            TSLiteral::NumericLiteral(it) => visitor.visit_numeric_literal(it),
            TSLiteral::BigIntLiteral(it) => visitor.visit_big_int_literal(it),
            TSLiteral::StringLiteral(it) => visitor.visit_string_literal(it),
            TSLiteral::TemplateLiteral(it) => visitor.visit_template_literal(it),
            TSLiteral::UnaryExpression(it) => visitor.visit_unary_expression(it),
        }
    }

    pub fn walk_ts_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSType<'a>) {
        // No `AstKind` for this type
        walk_ts_type_variants!(visitor, TSType, it);
    }

    #[inline]
    pub fn walk_ts_conditional_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSConditionalType<'a>) {
        let kind = AstKind::TSConditionalType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.check_type);
        visitor.enter_scope(ScopeFlags::TsConditional, &it.scope_id);
        visitor.visit_ts_type(&it.extends_type);
        visitor.visit_ts_type(&it.true_type);
        visitor.leave_scope();
        visitor.visit_ts_type(&it.false_type);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_union_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSUnionType<'a>) {
        let kind = AstKind::TSUnionType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_types(&it.types);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_intersection_type<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSIntersectionType<'a>,
    ) {
        let kind = AstKind::TSIntersectionType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_types(&it.types);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_parenthesized_type<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSParenthesizedType<'a>,
    ) {
        let kind = AstKind::TSParenthesizedType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_operator<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypeOperator<'a>) {
        let kind = AstKind::TSTypeOperator(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_array_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSArrayType<'a>) {
        let kind = AstKind::TSArrayType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.element_type);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_indexed_access_type<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSIndexedAccessType<'a>,
    ) {
        let kind = AstKind::TSIndexedAccessType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.object_type);
        visitor.visit_ts_type(&it.index_type);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_tuple_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTupleType<'a>) {
        let kind = AstKind::TSTupleType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_tuple_elements(&it.element_types);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_named_tuple_member<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSNamedTupleMember<'a>,
    ) {
        let kind = AstKind::TSNamedTupleMember(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_identifier_name(&it.label);
        visitor.visit_ts_tuple_element(&it.element_type);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_optional_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSOptionalType<'a>) {
        let kind = AstKind::TSOptionalType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_rest_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSRestType<'a>) {
        let kind = AstKind::TSRestType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_tuple_element<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTupleElement<'a>) {
        // No `AstKind` for this type
        match it {
            TSTupleElement::TSOptionalType(it) => visitor.visit_ts_optional_type(it),
            TSTupleElement::TSRestType(it) => visitor.visit_ts_rest_type(it),
            match_ts_type!(TSTupleElement) => walk_ts_type_variants!(visitor, TSTupleElement, it),
        }
    }

    #[inline]
    pub fn walk_ts_any_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSAnyKeyword) {
        let kind = AstKind::TSAnyKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_string_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSStringKeyword) {
        let kind = AstKind::TSStringKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_boolean_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSBooleanKeyword) {
        let kind = AstKind::TSBooleanKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_number_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSNumberKeyword) {
        let kind = AstKind::TSNumberKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_never_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSNeverKeyword) {
        let kind = AstKind::TSNeverKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_intrinsic_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSIntrinsicKeyword) {
        let kind = AstKind::TSIntrinsicKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_unknown_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSUnknownKeyword) {
        let kind = AstKind::TSUnknownKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_null_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSNullKeyword) {
        let kind = AstKind::TSNullKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_undefined_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSUndefinedKeyword) {
        let kind = AstKind::TSUndefinedKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_void_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSVoidKeyword) {
        let kind = AstKind::TSVoidKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_symbol_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSSymbolKeyword) {
        let kind = AstKind::TSSymbolKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_this_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSThisType) {
        let kind = AstKind::TSThisType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_object_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSObjectKeyword) {
        let kind = AstKind::TSObjectKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_big_int_keyword<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSBigIntKeyword) {
        let kind = AstKind::TSBigIntKeyword(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_reference<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypeReference<'a>) {
        let kind = AstKind::TSTypeReference(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type_name(&it.type_name);
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_name<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypeName<'a>) {
        // No `AstKind` for this type
        walk_ts_type_name_variants!(visitor, TSTypeName, it);
    }

    #[inline]
    pub fn walk_ts_qualified_name<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSQualifiedName<'a>) {
        let kind = AstKind::TSQualifiedName(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type_name(&it.left);
        visitor.visit_identifier_name(&it.right);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_parameter_instantiation<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSTypeParameterInstantiation<'a>,
    ) {
        let kind = AstKind::TSTypeParameterInstantiation(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_types(&it.params);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_parameter<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypeParameter<'a>) {
        let kind = AstKind::TSTypeParameter(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_identifier(&it.name);
        if let Some(constraint) = &it.constraint {
            visitor.visit_ts_type(constraint);
        }
        if let Some(default) = &it.default {
            visitor.visit_ts_type(default);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_parameter_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSTypeParameterDeclaration<'a>,
    ) {
        let kind = AstKind::TSTypeParameterDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type_parameters(&it.params);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_alias_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSTypeAliasDeclaration<'a>,
    ) {
        let kind = AstKind::TSTypeAliasDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_identifier(&it.id);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_class_implements<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSClassImplements<'a>) {
        let kind = AstKind::TSClassImplements(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type_name(&it.expression);
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_interface_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSInterfaceDeclaration<'a>,
    ) {
        let kind = AstKind::TSInterfaceDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_identifier(&it.id);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        visitor.visit_ts_interface_heritages(&it.extends);
        visitor.visit_ts_interface_body(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_interface_body<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSInterfaceBody<'a>) {
        let kind = AstKind::TSInterfaceBody(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_signatures(&it.body);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_property_signature<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSPropertySignature<'a>,
    ) {
        let kind = AstKind::TSPropertySignature(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_property_key(&it.key);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_signature<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSSignature<'a>) {
        // No `AstKind` for this type
        match it {
            TSSignature::TSIndexSignature(it) => visitor.visit_ts_index_signature(it),
            TSSignature::TSPropertySignature(it) => visitor.visit_ts_property_signature(it),
            TSSignature::TSCallSignatureDeclaration(it) => {
                visitor.visit_ts_call_signature_declaration(it)
            }
            TSSignature::TSConstructSignatureDeclaration(it) => {
                visitor.visit_ts_construct_signature_declaration(it)
            }
            TSSignature::TSMethodSignature(it) => visitor.visit_ts_method_signature(it),
        }
    }

    #[inline]
    pub fn walk_ts_index_signature<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSIndexSignature<'a>) {
        let kind = AstKind::TSIndexSignature(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_index_signature_names(&it.parameters);
        visitor.visit_ts_type_annotation(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_call_signature_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSCallSignatureDeclaration<'a>,
    ) {
        let kind = AstKind::TSCallSignatureDeclaration(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(this_param) = &it.this_param {
            visitor.visit_ts_this_parameter(this_param);
        }
        visitor.visit_formal_parameters(&it.params);
        if let Some(return_type) = &it.return_type {
            visitor.visit_ts_type_annotation(return_type);
        }
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    pub fn walk_ts_method_signature<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSMethodSignature<'a>) {
        let kind = AstKind::TSMethodSignature(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        visitor.visit_property_key(&it.key);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(this_param) = &it.this_param {
            visitor.visit_ts_this_parameter(this_param);
        }
        visitor.visit_formal_parameters(&it.params);
        if let Some(return_type) = &it.return_type {
            visitor.visit_ts_type_annotation(return_type);
        }
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_construct_signature_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSConstructSignatureDeclaration<'a>,
    ) {
        let kind = AstKind::TSConstructSignatureDeclaration(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        visitor.visit_formal_parameters(&it.params);
        if let Some(return_type) = &it.return_type {
            visitor.visit_ts_type_annotation(return_type);
        }
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_index_signature_name<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSIndexSignatureName<'a>,
    ) {
        let kind = AstKind::TSIndexSignatureName(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type_annotation(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_interface_heritage<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSInterfaceHeritage<'a>,
    ) {
        let kind = AstKind::TSInterfaceHeritage(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_predicate<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypePredicate<'a>) {
        let kind = AstKind::TSTypePredicate(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type_predicate_name(&it.parameter_name);
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type_annotation(type_annotation);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_predicate_name<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSTypePredicateName<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            TSTypePredicateName::Identifier(it) => visitor.visit_identifier_name(it),
            TSTypePredicateName::This(it) => visitor.visit_ts_this_type(it),
        }
    }

    #[inline]
    pub fn walk_ts_module_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSModuleDeclaration<'a>,
    ) {
        let kind = AstKind::TSModuleDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_module_declaration_name(&it.id);
        visitor.enter_scope(
            {
                let mut flags = ScopeFlags::TsModuleBlock;
                if it.body.as_ref().is_some_and(TSModuleDeclarationBody::has_use_strict_directive) {
                    flags |= ScopeFlags::StrictMode;
                }
                flags
            },
            &it.scope_id,
        );
        if let Some(body) = &it.body {
            visitor.visit_ts_module_declaration_body(body);
        }
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_module_declaration_name<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSModuleDeclarationName<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            TSModuleDeclarationName::Identifier(it) => visitor.visit_binding_identifier(it),
            TSModuleDeclarationName::StringLiteral(it) => visitor.visit_string_literal(it),
        }
    }

    #[inline]
    pub fn walk_ts_module_declaration_body<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSModuleDeclarationBody<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            TSModuleDeclarationBody::TSModuleDeclaration(it) => {
                visitor.visit_ts_module_declaration(it)
            }
            TSModuleDeclarationBody::TSModuleBlock(it) => visitor.visit_ts_module_block(it),
        }
    }

    #[inline]
    pub fn walk_ts_global_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSGlobalDeclaration<'a>,
    ) {
        let kind = AstKind::TSGlobalDeclaration(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::TsModuleBlock, &it.scope_id);
        visitor.visit_span(&it.span);
        visitor.visit_span(&it.global_span);
        visitor.visit_ts_module_block(&it.body);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_module_block<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSModuleBlock<'a>) {
        let kind = AstKind::TSModuleBlock(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_directives(&it.directives);
        visitor.visit_statements(&it.body);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_literal<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypeLiteral<'a>) {
        let kind = AstKind::TSTypeLiteral(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_signatures(&it.members);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_infer_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSInferType<'a>) {
        let kind = AstKind::TSInferType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type_parameter(&it.type_parameter);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_query<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypeQuery<'a>) {
        let kind = AstKind::TSTypeQuery(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type_query_expr_name(&it.expr_name);
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_query_expr_name<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSTypeQueryExprName<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            TSTypeQueryExprName::TSImportType(it) => visitor.visit_ts_import_type(it),
            match_ts_type_name!(TSTypeQueryExprName) => {
                walk_ts_type_name_variants!(visitor, TSTypeQueryExprName, it)
            }
        }
    }

    #[inline]
    pub fn walk_ts_import_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSImportType<'a>) {
        let kind = AstKind::TSImportType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_string_literal(&it.source);
        if let Some(options) = &it.options {
            visitor.visit_object_expression(options);
        }
        if let Some(qualifier) = &it.qualifier {
            visitor.visit_ts_import_type_qualifier(qualifier);
        }
        if let Some(type_arguments) = &it.type_arguments {
            visitor.visit_ts_type_parameter_instantiation(type_arguments);
        }
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_import_type_qualifier<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSImportTypeQualifier<'a>,
    ) {
        // No `AstKind` for this type
        match it {
            TSImportTypeQualifier::Identifier(it) => visitor.visit_identifier_name(it),
            TSImportTypeQualifier::QualifiedName(it) => {
                visitor.visit_ts_import_type_qualified_name(it)
            }
        }
    }

    #[inline]
    pub fn walk_ts_import_type_qualified_name<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSImportTypeQualifiedName<'a>,
    ) {
        let kind = AstKind::TSImportTypeQualifiedName(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_import_type_qualifier(&it.left);
        visitor.visit_identifier_name(&it.right);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_function_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSFunctionType<'a>) {
        let kind = AstKind::TSFunctionType(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(this_param) = &it.this_param {
            visitor.visit_ts_this_parameter(this_param);
        }
        visitor.visit_formal_parameters(&it.params);
        visitor.visit_ts_type_annotation(&it.return_type);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_constructor_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSConstructorType<'a>) {
        let kind = AstKind::TSConstructorType(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        if let Some(type_parameters) = &it.type_parameters {
            visitor.visit_ts_type_parameter_declaration(type_parameters);
        }
        visitor.visit_formal_parameters(&it.params);
        visitor.visit_ts_type_annotation(&it.return_type);
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_mapped_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSMappedType<'a>) {
        let kind = AstKind::TSMappedType(it);
        visitor.enter_node(kind);
        visitor.enter_scope(ScopeFlags::empty(), &it.scope_id);
        visitor.visit_span(&it.span);
        visitor.visit_binding_identifier(&it.key);
        visitor.visit_ts_type(&it.constraint);
        if let Some(name_type) = &it.name_type {
            visitor.visit_ts_type(name_type);
        }
        if let Some(type_annotation) = &it.type_annotation {
            visitor.visit_ts_type(type_annotation);
        }
        visitor.leave_scope();
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_template_literal_type<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSTemplateLiteralType<'a>,
    ) {
        let kind = AstKind::TSTemplateLiteralType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_template_elements(&it.quasis);
        visitor.visit_ts_types(&it.types);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_as_expression<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSAsExpression<'a>) {
        let kind = AstKind::TSAsExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_satisfies_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSSatisfiesExpression<'a>,
    ) {
        let kind = AstKind::TSSatisfiesExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_type_assertion<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSTypeAssertion<'a>) {
        let kind = AstKind::TSTypeAssertion(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.visit_expression(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_import_equals_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSImportEqualsDeclaration<'a>,
    ) {
        let kind = AstKind::TSImportEqualsDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_binding_identifier(&it.id);
        visitor.visit_ts_module_reference(&it.module_reference);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_module_reference<'a, V: Visit<'a>>(visitor: &mut V, it: &'a TSModuleReference<'a>) {
        // No `AstKind` for this type
        match it {
            TSModuleReference::ExternalModuleReference(it) => {
                visitor.visit_ts_external_module_reference(it)
            }
            TSModuleReference::IdentifierReference(it) => visitor.visit_identifier_reference(it),
            TSModuleReference::QualifiedName(it) => visitor.visit_ts_qualified_name(it),
        }
    }

    #[inline]
    pub fn walk_ts_external_module_reference<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSExternalModuleReference<'a>,
    ) {
        let kind = AstKind::TSExternalModuleReference(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_string_literal(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_non_null_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSNonNullExpression<'a>,
    ) {
        let kind = AstKind::TSNonNullExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_decorator<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Decorator<'a>) {
        let kind = AstKind::Decorator(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_export_assignment<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSExportAssignment<'a>,
    ) {
        let kind = AstKind::TSExportAssignment(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_namespace_export_declaration<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSNamespaceExportDeclaration<'a>,
    ) {
        let kind = AstKind::TSNamespaceExportDeclaration(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_identifier_name(&it.id);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_ts_instantiation_expression<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a TSInstantiationExpression<'a>,
    ) {
        let kind = AstKind::TSInstantiationExpression(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_expression(&it.expression);
        visitor.visit_ts_type_parameter_instantiation(&it.type_arguments);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_js_doc_nullable_type<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a JSDocNullableType<'a>,
    ) {
        let kind = AstKind::JSDocNullableType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_js_doc_non_nullable_type<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a JSDocNonNullableType<'a>,
    ) {
        let kind = AstKind::JSDocNonNullableType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.visit_ts_type(&it.type_annotation);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_js_doc_unknown_type<'a, V: Visit<'a>>(visitor: &mut V, it: &'a JSDocUnknownType) {
        let kind = AstKind::JSDocUnknownType(it);
        visitor.enter_node(kind);
        visitor.visit_span(&it.span);
        visitor.leave_node(kind);
    }

    #[inline]
    pub fn walk_span<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Span) {
        // No `AstKind` for this type
    }

    #[inline]
    pub fn walk_directives<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, Directive<'a>>) {
        for el in it {
            visitor.visit_directive(el);
        }
    }

    #[inline]
    pub fn walk_statements<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, Statement<'a>>) {
        for el in it {
            visitor.visit_statement(el);
        }
    }

    #[inline]
    pub fn walk_array_expression_elements<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, ArrayExpressionElement<'a>>,
    ) {
        for el in it {
            visitor.visit_array_expression_element(el);
        }
    }

    #[inline]
    pub fn walk_object_property_kinds<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, ObjectPropertyKind<'a>>,
    ) {
        for el in it {
            visitor.visit_object_property_kind(el);
        }
    }

    #[inline]
    pub fn walk_template_elements<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, TemplateElement<'a>>,
    ) {
        for el in it {
            visitor.visit_template_element(el);
        }
    }

    #[inline]
    pub fn walk_expressions<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, Expression<'a>>) {
        for el in it {
            visitor.visit_expression(el);
        }
    }

    #[inline]
    pub fn walk_arguments<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, Argument<'a>>) {
        for el in it {
            match el {
                oxc_ast::ast::Argument::SpreadElement(spread) => {
                    visitor.visit_spread_element(spread);
                }
                _ => {
                    walk_argument(visitor, el);
                }
            }
        }
    }

    #[inline]
    pub fn walk_assignment_target_properties<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, AssignmentTargetProperty<'a>>,
    ) {
        for el in it {
            visitor.visit_assignment_target_property(el);
        }
    }

    #[inline]
    pub fn walk_variable_declarators<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, VariableDeclarator<'a>>,
    ) {
        for el in it {
            visitor.visit_variable_declarator(el);
        }
    }

    #[inline]
    pub fn walk_switch_cases<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, SwitchCase<'a>>) {
        for el in it {
            visitor.visit_switch_case(el);
        }
    }

    #[inline]
    pub fn walk_binding_properties<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, BindingProperty<'a>>,
    ) {
        for el in it {
            visitor.visit_binding_property(el);
        }
    }

    #[inline]
    pub fn walk_formal_parameter_list<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, FormalParameter<'a>>,
    ) {
        for el in it {
            visitor.visit_formal_parameter(el);
        }
    }

    #[inline]
    pub fn walk_decorators<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, Decorator<'a>>) {
        for el in it {
            visitor.visit_decorator(el);
        }
    }

    #[inline]
    pub fn walk_ts_class_implements_list<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, TSClassImplements<'a>>,
    ) {
        for el in it {
            visitor.visit_ts_class_implements(el);
        }
    }

    #[inline]
    pub fn walk_class_elements<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, ClassElement<'a>>) {
        for el in it {
            visitor.visit_class_element(el);
        }
    }

    #[inline]
    pub fn walk_import_declaration_specifiers<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, ImportDeclarationSpecifier<'a>>,
    ) {
        for el in it {
            visitor.visit_import_declaration_specifier(el);
        }
    }

    #[inline]
    pub fn walk_import_attributes<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, ImportAttribute<'a>>,
    ) {
        for el in it {
            visitor.visit_import_attribute(el);
        }
    }

    #[inline]
    pub fn walk_export_specifiers<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, ExportSpecifier<'a>>,
    ) {
        for el in it {
            visitor.visit_export_specifier(el);
        }
    }

    #[inline]
    pub fn walk_jsx_children<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, JSXChild<'a>>) {
        for el in it {
            visitor.visit_jsx_child(el);
        }
    }

    #[inline]
    pub fn walk_jsx_attribute_items<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, JSXAttributeItem<'a>>,
    ) {
        for el in it {
            visitor.visit_jsx_attribute_item(el);
        }
    }

    #[inline]
    pub fn walk_ts_enum_members<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, TSEnumMember<'a>>) {
        for el in it {
            visitor.visit_ts_enum_member(el);
        }
    }

    #[inline]
    pub fn walk_ts_types<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, TSType<'a>>) {
        for el in it {
            visitor.visit_ts_type(el);
        }
    }

    #[inline]
    pub fn walk_ts_tuple_elements<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, TSTupleElement<'a>>,
    ) {
        for el in it {
            visitor.visit_ts_tuple_element(el);
        }
    }

    #[inline]
    pub fn walk_ts_type_parameters<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, TSTypeParameter<'a>>,
    ) {
        for el in it {
            visitor.visit_ts_type_parameter(el);
        }
    }

    #[inline]
    pub fn walk_ts_interface_heritages<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, TSInterfaceHeritage<'a>>,
    ) {
        for el in it {
            visitor.visit_ts_interface_heritage(el);
        }
    }

    #[inline]
    pub fn walk_ts_signatures<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, TSSignature<'a>>) {
        for el in it {
            visitor.visit_ts_signature(el);
        }
    }

    #[inline]
    pub fn walk_ts_index_signature_names<'a, V: Visit<'a>>(
        visitor: &mut V,
        it: &'a Vec<'a, TSIndexSignatureName<'a>>,
    ) {
        for el in it {
            visitor.visit_ts_index_signature_name(el);
        }
    }

    #[inline]
    pub fn walk_spans<'a, V: Visit<'a>>(visitor: &mut V, it: &'a Vec<'a, Span>) {
        for el in it {
            visitor.visit_span(el);
        }
    }
}
