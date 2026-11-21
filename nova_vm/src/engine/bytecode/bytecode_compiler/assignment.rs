// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use oxc_ast::ast::{self, AssignmentOperator, LogicalOperator};
use oxc_ast::{
    ast::{AssignmentTarget, AssignmentTargetMaybeDefault, ForStatementLeft},
    match_member_expression, match_simple_assignment_target,
};

use oxc_span::GetSpan;

use super::CompileExpressionVariants;

use crate::ecmascript::String;
use crate::engine::{
    Instruction,
    bytecode::bytecode_compiler::{
        CompileContext, CompileEvaluation, ExpressionError, NamedEvaluationParameter, Place,
        PlaceOrValue, ValueOutput, is_anonymous_function_definition,
    },
};

/// Whether the code that evaluates `expression` leaves the reference register
/// as it finds it, so that the Reference of an assignment's target can wait
/// in the register while the value is evaluated and need not be pushed
/// before and popped after. `this.n += by` with a parameter `by` is five
/// instructions and not seven.
///
/// That is: a literal; the name of a variable that the function keeps in a
/// stack slot, which is one instruction that reads the slot (or throws,
/// where the name is in its temporal dead zone); and the arithmetic, the
/// comparisons and the choices among such. An operator may run code of the
/// script (a `valueOf`), which another `Vm` runs, as it may where the
/// operator of a compound assignment itself runs with the Reference in the
/// register, after a literal as much as after anything.
///
/// Not a property, a call, a name found elsewhere, an assignment, `in` or
/// `instanceof`: those use the register, or enter a function in place.
fn leaves_reference_register<'s>(
    ctx: &CompileContext<'_, 's, '_, '_>,
    expression: &ast::Expression<'s>,
) -> bool {
    match expression {
        ast::Expression::ParenthesizedExpression(inner) => {
            leaves_reference_register(ctx, &inner.expression)
        }
        ast::Expression::BinaryExpression(binary) => {
            !matches!(
                binary.operator,
                ast::BinaryOperator::In | ast::BinaryOperator::Instanceof
            ) && leaves_reference_register(ctx, &binary.left)
                && leaves_reference_register(ctx, &binary.right)
        }
        ast::Expression::UnaryExpression(unary) => {
            matches!(
                unary.operator,
                ast::UnaryOperator::UnaryNegation
                    | ast::UnaryOperator::UnaryPlus
                    | ast::UnaryOperator::LogicalNot
                    | ast::UnaryOperator::BitwiseNot
            ) && leaves_reference_register(ctx, &unary.argument)
        }
        ast::Expression::LogicalExpression(logical) => {
            leaves_reference_register(ctx, &logical.left)
                && leaves_reference_register(ctx, &logical.right)
        }
        ast::Expression::ConditionalExpression(conditional) => {
            leaves_reference_register(ctx, &conditional.test)
                && leaves_reference_register(ctx, &conditional.consequent)
                && leaves_reference_register(ctx, &conditional.alternate)
        }
        _ => expression.is_literal() || is_stack_variable(ctx, expression),
    }
}

/// Whether `expression` is the name of a variable that the function keeps in
/// a stack slot: as the name is compiled (`IdentifierReference`), the
/// arguments object in its slot, or a variable with a stack slot.
fn is_stack_variable<'s>(
    ctx: &CompileContext<'_, 's, '_, '_>,
    expression: &ast::Expression<'s>,
) -> bool {
    let ast::Expression::Identifier(identifier) = expression else {
        return false;
    };
    let Some(reference) = identifier.reference_id.get() else {
        return false;
    };
    let symbol = ctx.scoping().get_reference(reference).symbol_id();
    (identifier.name == "arguments" && ctx.arguments_slot(symbol).is_some())
        || symbol.is_some_and(|symbol| ctx.get_variable_stack_index(symbol).is_some())
}

/// The stack slot of `object`, where it is the name of a variable that the
/// function keeps in one and nothing in `value` assigns to the variable: the
/// object of `object.name = value` is then what the slot holds when the
/// assignment is made, as it was before the value was evaluated. Nothing
/// else changes such a variable in between: a function that is called has
/// no way to it, and a direct `eval` or a `with` in the function leaves it
/// no slot.
pub(super) fn slot_of_object_kept_through<'s>(
    ctx: &CompileContext<'_, 's, '_, '_>,
    object: &ast::Expression<'s>,
    value: &ast::Expression<'s>,
) -> Option<u32> {
    let ast::Expression::Identifier(identifier) = object else {
        return None;
    };
    let symbol = ctx
        .scoping()
        .get_reference(identifier.reference_id.get()?)
        .symbol_id()?;
    let slot = ctx.get_variable_stack_index(symbol)?;
    let within = value.span();
    let nodes = ctx.nodes();
    let assigned = ctx
        .scoping()
        .get_resolved_references(symbol)
        .any(|reference| {
            let at = nodes.get_node(reference.node_id()).kind().span();
            reference.is_write() && within.start <= at.start && at.end <= within.end
        });
    (!assigned).then_some(slot)
}

/// The instruction of the operator of a compound assignment.
fn compound_operator_instruction(operator: ast::BinaryOperator) -> Instruction {
    match operator {
        ast::BinaryOperator::Addition => Instruction::ApplyAdditionBinaryOperator,
        ast::BinaryOperator::Subtraction => Instruction::ApplySubtractionBinaryOperator,
        ast::BinaryOperator::Multiplication => Instruction::ApplyMultiplicationBinaryOperator,
        ast::BinaryOperator::Division => Instruction::ApplyDivisionBinaryOperator,
        ast::BinaryOperator::Remainder => Instruction::ApplyRemainderBinaryOperator,
        ast::BinaryOperator::Exponential => Instruction::ApplyExponentialBinaryOperator,
        ast::BinaryOperator::ShiftLeft => Instruction::ApplyShiftLeftBinaryOperator,
        ast::BinaryOperator::ShiftRight => Instruction::ApplyShiftRightBinaryOperator,
        ast::BinaryOperator::ShiftRightZeroFill => {
            Instruction::ApplyShiftRightZeroFillBinaryOperator
        }
        ast::BinaryOperator::BitwiseOR => Instruction::ApplyBitwiseORBinaryOperator,
        ast::BinaryOperator::BitwiseXOR => Instruction::ApplyBitwiseXORBinaryOperator,
        ast::BinaryOperator::BitwiseAnd => Instruction::ApplyBitwiseAndBinaryOperator,
        _ => unreachable!(),
    }
}

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope> for ast::AssignmentExpression<'s> {
    type Output = Result<ValueOutput<'gc>, ExpressionError>;
    fn compile(&'s self, ctx: &mut CompileContext<'a, 's, 'gc, 'scope>) -> Self::Output {
        compile_assignment_expression(self, ctx, true)
    }
}

/// `object[key] = value` with the object and the key kept on the stack:
/// the order of evaluation is the one of the Reference way (the object, the
/// key, the value, then the key made a property key as the assignment is
/// made).
fn compile_element_assignment<'s, 'gc>(
    target: &'s ast::ComputedMemberExpression<'s>,
    right: &'s ast::Expression<'s>,
    ctx: &mut CompileContext<'_, 's, 'gc, '_>,
) -> Result<ValueOutput<'gc>, ExpressionError> {
    // 1. Let baseReference be ? Evaluation of MemberExpression.
    // 2. Let baseValue be ? GetValue(baseReference).
    target.object.compile(ctx)?.get_value(ctx)?;
    ctx.add_instruction(Instruction::Load);
    let object_on_stack = ctx.mark_stack_value();
    // 3. Let propertyNameReference be ? Evaluation of Expression.
    // 4. Let propertyNameValue be ? GetValue(propertyNameReference).
    if let Err(err) = target
        .expression
        .compile(ctx)
        .and_then(|r| r.get_value(ctx))
    {
        object_on_stack.forget(ctx);
        return Err(err);
    }
    ctx.add_instruction(Instruction::Load);
    let key_on_stack = ctx.mark_stack_value();
    let rval = match right.compile(ctx).and_then(|r| r.get_value(ctx)) {
        Ok(rval) => rval,
        Err(err) => {
            key_on_stack.forget(ctx);
            object_on_stack.forget(ctx);
            return Err(err);
        }
    };
    key_on_stack.forget(ctx);
    object_on_stack.forget(ctx);
    ctx.add_instruction(Instruction::PutValueWithExpressionKey);
    Ok(rval)
}

/// `object.name = value` with the object kept on the stack: the order of
/// evaluation is the one of the Reference way (the object, the value, then
/// the object made an Object as the assignment is made).
fn compile_named_assignment<'s, 'gc>(
    target: &'s ast::StaticMemberExpression<'s>,
    name: String<'gc>,
    right: &'s ast::Expression<'s>,
    ctx: &mut CompileContext<'_, 's, 'gc, '_>,
) -> Result<ValueOutput<'gc>, ExpressionError> {
    // 1. Let baseReference be ? Evaluation of MemberExpression.
    // 2. Let baseValue be ? GetValue(baseReference).
    let object = target.object.compile(ctx)?;
    // A local that the value does not assign to stays in its slot, where
    // the assignment finds it: it is not pushed and popped.
    if let PlaceOrValue::Place(Place::Stack { stack_slot, .. }) = &object
        && slot_of_object_kept_through(ctx, &target.object, right) == Some(*stack_slot)
    {
        let slot = *stack_slot as usize;
        let rval = right.compile(ctx).and_then(|r| r.get_value(ctx))?;
        let cache = ctx.create_property_lookup_cache(name.to_property_key());
        ctx.add_instruction_with_identifier_cache_and_slot(
            Instruction::PutIndexPropertyWithCache,
            name,
            cache,
            slot,
        );
        return Ok(rval);
    }
    object.get_value(ctx)?;
    ctx.add_instruction(Instruction::Load);
    let object_on_stack = ctx.mark_stack_value();
    let rval = match right.compile(ctx).and_then(|r| r.get_value(ctx)) {
        Ok(rval) => rval,
        Err(err) => {
            object_on_stack.forget(ctx);
            return Err(err);
        }
    };
    object_on_stack.forget(ctx);
    let cache = ctx.create_property_lookup_cache(name.to_property_key());
    ctx.add_instruction_with_identifier_and_cache(Instruction::PutPropertyWithCache, name, cache);
    Ok(rval)
}

/// `p.name op= value` with the object kept on the stack or in its slot, for
/// a `p` that the function keeps in a stack slot, and a value whose code
/// does not leave the reference register alone (`p.x += p.vx`) or through
/// which `p` stays in its slot (`p.x += 1`): the Reference way reads the
/// property with its Reference kept, pushes the Reference for as long as
/// the value is evaluated, pops it, and assigns through it. Here the object
/// is pushed, its property read as any other, and the assignment made as
/// `p.name = …` is: one instruction fewer, and no Reference made, pushed and
/// popped.
///
/// The object that is assigned to is the one whose property was read: the
/// slot is read twice with nothing in between. What is read and what is
/// assigned are \[\[Get]] and \[\[Set]] of the same object and name, in the
/// order of the Reference way: the object, the property's value, the
/// value, the operator, the assignment.
fn compile_named_compound_assignment<'s, 'gc>(
    this: &'s ast::AssignmentExpression<'s>,
    target: &'s ast::StaticMemberExpression<'s>,
    name: String<'gc>,
    ctx: &mut CompileContext<'_, 's, 'gc, '_>,
) -> Result<ValueOutput<'gc>, ExpressionError> {
    // 1. Let lref be ? Evaluation of LeftHandSideExpression: its object.
    let object = target.object.compile(ctx)?;
    // A local that the value does not assign to stays in its slot, where
    // the assignment finds it a third time: it is not pushed and popped.
    let slot = match &object {
        PlaceOrValue::Place(Place::Stack { stack_slot, .. })
            if slot_of_object_kept_through(ctx, &target.object, &this.right)
                == Some(*stack_slot) =>
        {
            Some(*stack_slot as usize)
        }
        _ => None,
    };
    let object_on_stack = if slot.is_none() {
        object.get_value(ctx)?;
        ctx.add_instruction(Instruction::Load);
        Some(ctx.mark_stack_value())
    } else {
        None
    };
    // 2. Let lval be ? GetValue(lref).
    if let Err(err) = target.compile(ctx).and_then(|place| place.get_value(ctx)) {
        if let Some(object_on_stack) = object_on_stack {
            object_on_stack.forget(ctx);
        }
        return Err(err);
    }
    let lval_copy = ctx.load_to_stack();
    // 3. Let rref be ? Evaluation of AssignmentExpression.
    // 4. Let rval be ? GetValue(rref).
    if let Err(err) = this.right.compile(ctx).and_then(|r| r.get_value(ctx)) {
        lval_copy.forget(ctx);
        if let Some(object_on_stack) = object_on_stack {
            object_on_stack.forget(ctx);
        }
        return Err(err);
    }
    // 7. Let r be ? ApplyStringOrNumericBinaryOperator(lval, opText, rval).
    lval_copy.forget(ctx);
    ctx.add_instruction(compound_operator_instruction(
        this.operator.to_binary_operator().unwrap(),
    ));
    // 8. Perform ? PutValue(lref, r).
    let cache = ctx.create_property_lookup_cache(name.to_property_key());
    match slot {
        Some(slot) => ctx.add_instruction_with_identifier_cache_and_slot(
            Instruction::PutIndexPropertyWithCache,
            name,
            cache,
            slot,
        ),
        None => {
            if let Some(object_on_stack) = object_on_stack {
                object_on_stack.forget(ctx);
            }
            ctx.add_instruction_with_identifier_and_cache(
                Instruction::PutPropertyWithCache,
                name,
                cache,
            );
        }
    }
    Ok(ValueOutput::Value)
}

/// With `keep_value` false the value of a plain or compound assignment is not
/// left in the result register: the caller discards it.
pub(super) fn compile_assignment_expression<'s, 'gc>(
    this: &'s ast::AssignmentExpression<'s>,
    ctx: &mut CompileContext<'_, 's, 'gc, '_>,
    keep_value: bool,
) -> Result<ValueOutput<'gc>, ExpressionError> {
    {
        // `a[i] = v` whose value nothing uses: the object and the key stay
        // on the stack while the value is evaluated, and one instruction
        // assigns, with no Reference to make, keep and take back.
        if !keep_value
            && this.operator.is_assign()
            && let ast::AssignmentTarget::ComputedMemberExpression(target) = &this.left
            && !target.optional
            && !target.object.is_super()
            && !target.expression.is_literal()
            && ctx.optional_chains.is_none()
        {
            return compile_element_assignment(target, &this.right, ctx);
        }
        // `o.x = v` likewise, with only the object to keep.
        if !keep_value
            && this.operator.is_assign()
            && let ast::AssignmentTarget::StaticMemberExpression(target) = &this.left
            && !target.optional
            && !target.object.is_super()
            && ctx.optional_chains.is_none()
        {
            let name = ctx.create_string(target.property.name.as_str());
            if ctx.property_name_fits(name.to_property_key()) {
                return compile_named_assignment(target, name, &this.right, ctx);
            }
        }
        // `p.x += p.vx` likewise, for a local `p`: see
        // `compile_named_compound_assignment`. With a value that leaves the
        // reference register alone (`p.x += 1`, `p.x += i`) the Reference
        // way has no push and no pop, and as many instructions; but they
        // are the ones that make a Reference and assign through it, a
        // tenth to a seventh more than reading the property and assigning
        // it by the slot of `p`, which is taken where `p` stays in its
        // slot through the value.
        if !keep_value
            && !this.operator.is_assign()
            && this.operator.to_logical_operator().is_none()
            && let ast::AssignmentTarget::StaticMemberExpression(target) = &this.left
            && !target.optional
            && is_stack_variable(ctx, &target.object)
            && (!leaves_reference_register(ctx, &this.right)
                || slot_of_object_kept_through(ctx, &target.object, &this.right).is_some())
            && ctx.optional_chains.is_none()
        {
            let name = ctx.create_string(target.property.name.as_str());
            if ctx.property_name_fits(name.to_property_key()) {
                return compile_named_compound_assignment(this, target, name, ctx);
            }
        }
        let mut do_named_evaluation = false;
        // 1. Let lref be ? Evaluation of LeftHandSideExpression.
        let lref = match &this.left {
            ast::AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                let place = identifier.compile(ctx);
                // NOTE: If the left hand side does not constitute the start of
                // the assignment expression span, then it means that the left
                // side is inside parentheses and NamedEvaluation should not
                // happen.
                do_named_evaluation = this.span.start == identifier.span.start;
                place
            }
            ast::AssignmentTarget::ComputedMemberExpression(expression) => {
                // 1. If LeftHandSideExpression is neither an ObjectLiteral nor an ArrayLiteral, then
                expression.compile(ctx)?
            }
            ast::AssignmentTarget::ArrayAssignmentTarget(_)
            | ast::AssignmentTarget::ObjectAssignmentTarget(_) => {
                assert_eq!(
                    this.operator,
                    AssignmentOperator::Assign,
                    "SyntaxError: Invalid left-hand side in assignment expression"
                );
                // 2. Let assignmentPattern be the AssignmentPattern that is covered by LeftHandSideExpression.
                // 3. Let rRef be ? Evaluation of AssignmentExpression.
                // 4. Let rVal be ? GetValue(rRef).
                let rval = this.right.compile(ctx)?.get_value(ctx)?;
                // 5. Perform ? DestructuringAssignmentEvaluation of assignmentPattern with argument rVal.
                let rval_copy = ctx.load_copy_to_stack();
                let result = this.left.compile_pattern_target(ctx);
                // 6. Return rVal.
                rval_copy.store(ctx);
                return result.map(|_| rval);
            }
            ast::AssignmentTarget::PrivateFieldExpression(expression) => {
                // 1. If LeftHandSideExpression is neither an ObjectLiteral nor an ArrayLiteral, then
                expression.compile(ctx)?
            }
            ast::AssignmentTarget::StaticMemberExpression(expression) => {
                // 1. If LeftHandSideExpression is neither an ObjectLiteral nor an ArrayLiteral, then
                expression.compile(ctx)?
            }
            #[cfg(feature = "typescript")]
            ast::AssignmentTarget::TSNonNullExpression(x) => {
                let PlaceOrValue::Place(place) = x.expression.compile(ctx)? else {
                    unreachable!()
                };
                do_named_evaluation = this.span.start == x.span.start;
                place
            }
            #[cfg(feature = "typescript")]
            ast::AssignmentTarget::TSAsExpression(x) => {
                let PlaceOrValue::Place(place) = x.expression.compile(ctx)? else {
                    unreachable!()
                };
                do_named_evaluation = this.span.start == x.span.start;
                place
            }
            #[cfg(feature = "typescript")]
            ast::AssignmentTarget::TSSatisfiesExpression(x) => {
                let PlaceOrValue::Place(place) = x.expression.compile(ctx)? else {
                    unreachable!()
                };
                do_named_evaluation = this.span.start == x.span.start;
                place
            }
            #[cfg(not(feature = "typescript"))]
            ast::AssignmentTarget::TSAsExpression(_)
            | ast::AssignmentTarget::TSNonNullExpression(_)
            | ast::AssignmentTarget::TSSatisfiesExpression(_) => unreachable!(),
            ast::AssignmentTarget::TSTypeAssertion(_) => unreachable!(),
        };
        // A name of the global scope that may not be bound is found bound
        // at its place and no Reference made of it, where the value is
        // worked out without the reference register and so leaves it as
        // that leaves it.
        let register_is_left = this.operator.is_assign()
            && matches!(lref, Place::GlobalSite { bound: false, .. })
            && leaves_reference_register(ctx, &this.right)
            && !(do_named_evaluation && is_anonymous_function_definition(&this.right));
        let lref = lref.assignment_target_for(ctx, !this.operator.is_assign(), register_is_left);

        if this.operator.is_assign() {
            let push_reference =
                lref.has_reference() && !leaves_reference_register(ctx, &this.right);

            if push_reference {
                ctx.add_instruction(Instruction::PushReference);
            }

            if do_named_evaluation && is_anonymous_function_definition(&this.right) {
                ctx.add_instruction_with_constant(
                    Instruction::StoreConstant,
                    lref.identifier().unwrap(),
                );
                ctx.name_identifier = Some(NamedEvaluationParameter::Result);
            }

            let rval = this.right.compile(ctx)?.get_value(ctx)?;

            let rval_copy = keep_value.then(|| ctx.load_copy_to_stack());

            if push_reference {
                ctx.add_instruction(Instruction::PopReference);
            }

            let result = lref.put_value(ctx, rval);
            // ... Return rval.
            if let Some(rval_copy) = rval_copy {
                rval_copy.store(ctx);
            }
            result.map(|_| rval)
        } else if let Some(operator) = this.operator.to_logical_operator() {
            // 2. Let lval be ? GetValue(lref).
            lref.get_value_keep_reference(ctx)?;
            let push_reference =
                lref.has_reference() && !leaves_reference_register(ctx, &this.right);
            if push_reference {
                ctx.add_instruction(Instruction::PushReference);
            }
            // We store the lval on the stack because we to branch based on its
            // value to either return lval directly, or go into the
            // `PutValue(lRef, rVal)` branch and return rval. For the lval
            // return we need a copy of the value.
            let lval_copy = ctx.load_copy_to_stack();
            match operator {
                LogicalOperator::And => {
                    // 3. If ToBoolean(lVal) is false, return lVal.
                    // Note: We do not directly call ToBoolean: JumpIfNot does.
                }
                LogicalOperator::Or => {
                    // 3. 3. If ToBoolean(lVal) is true, return lVal.
                    // Note: We do not directly call ToBoolean: JumpIfNot does.
                    ctx.add_instruction(Instruction::LogicalNot);
                }
                LogicalOperator::Coalesce => {
                    // 3. If lval is neither undefined nor null, return lval.
                    ctx.add_instruction(Instruction::IsNullOrUndefined);
                }
            };
            // Phi-split: we have two alternative execution paths after this.
            let jump_to_return_lval = ctx.add_instruction_with_jump_slot(Instruction::JumpIfNot);
            let (value_on_stack, jump_to_return_rval) = 'rval: {
                // In this branch we're returning rval, so we discard lval from
                // the top of the stack.
                lval_copy.store(ctx);

                // 5. If IsAnonymousFunctionDefinition(AssignmentExpression)
                // is true and IsIdentifierRef of LeftHandSideExpression is true,
                // then
                if do_named_evaluation && is_anonymous_function_definition(&this.right) {
                    // a. Let lhs be the StringValue of LeftHandSideExpression.
                    ctx.add_instruction_with_constant(
                        Instruction::StoreConstant,
                        lref.identifier().unwrap(),
                    );
                    // b. Let rval be ? NamedEvaluation of AssignmentExpression
                    // with argument lhs.
                    ctx.name_identifier = Some(NamedEvaluationParameter::Result);
                }
                // a. Let rref be ? Evaluation of AssignmentExpression.
                // b. Let rval be ? GetValue(rref).
                // Note: no early exits because this path is not unconditional.
                let Ok(rval) = this.right.compile(ctx).and_then(|r| r.get_value(ctx)) else {
                    // If we're here then the code will never get to the end of
                    // the else branch. When we exit the 'rval block the only
                    // possible path forward is the one where we jumped over the
                    // 'rval block. In that case we have the lval still on the
                    // stack: mark it.
                    break 'rval (ctx.mark_stack_value(), None);
                };
                // We need to return the rval after PutValue, so we need to copy
                // it onto the stack.
                let rval_copy = ctx.load_copy_to_stack();

                if push_reference {
                    // Pop lref off the reference stack.
                    ctx.add_instruction(Instruction::PopReference);
                }
                // 6. Perform ? PutValue(lRef, rVal).
                let Ok(_) = lref.put_value(ctx, rval) else {
                    // 'rval branch ends at unreachable, we can use rval_copy as
                    // a stand-in for lval_copy.
                    break 'rval (rval_copy, None);
                };
                if push_reference {
                    // If lref was pushed onto the reference stack then the we
                    // have to jump over the if-branch popping it as we already
                    // popped it above. As we return from 'rval we're
                    // unreachable in the else-branch and thus we again must be
                    // generating code for the lval branch where we still have
                    // lval_copy alive: mark it.
                    rval_copy.store(ctx);
                    (
                        ctx.mark_stack_value(),
                        Some(ctx.add_instruction_with_jump_slot(Instruction::Jump)),
                    )
                } else {
                    // If lref wasn't pushed onto the reference stack then we
                    // can handle lval and rval stack store using the same
                    // instruction, so we return rval_copy out of this block.
                    (rval_copy, None)
                }
            };

            // If jump_to_return_rval is None then this is a phi-join, both
            // branches unify here. If that is not the case then this is where
            // we jump in to return lval_copy.
            ctx.set_jump_target_here(jump_to_return_lval);

            // 3. ..., return lval.
            value_on_stack.store(ctx);
            if push_reference {
                // Pop lref off the reference stack.
                ctx.add_instruction(Instruction::PopReference);
            }
            // Phi-join: after this both branches finally unify.
            if let Some(jump_over_else) = jump_to_return_rval {
                ctx.set_jump_target_here(jump_over_else);
            }
            // 7. Return rVal.
            Ok(ValueOutput::Value)
        } else {
            // 2. let lval be ? GetValue(lref).
            let _lval = lref.get_value_keep_reference(ctx)?;
            let lval_copy = ctx.load_to_stack();
            let do_push_reference =
                lref.has_reference() && !leaves_reference_register(ctx, &this.right);
            if do_push_reference {
                ctx.add_instruction(Instruction::PushReference);
            }
            // 3. Let rref be ? Evaluation of AssignmentExpression.
            // 4. Let rval be ? GetValue(rref).
            let _rval = match this.right.compile(ctx).and_then(|r| r.get_value(ctx)) {
                Ok(r) => r,
                Err(err) => {
                    lval_copy.forget(ctx);
                    return Err(err);
                }
            };

            // 5. Let assignmentOpText be the source text matched by AssignmentOperator.
            // 6. Let opText be the sequence of Unicode code points associated with assignmentOpText in the following table:
            let op_text = this.operator.to_binary_operator().unwrap();
            // 7. Let r be ? ApplyStringOrNumericBinaryOperator(lval, opText, rval).
            let op_text = compound_operator_instruction(op_text);
            // Consumed by instruction.
            lval_copy.forget(ctx);
            ctx.add_instruction(op_text);
            let r_copy = keep_value.then(|| ctx.load_copy_to_stack());
            let r = ValueOutput::Value;
            if do_push_reference {
                ctx.add_instruction(Instruction::PopReference);
            }
            // 8. Perform ? PutValue(lref, r).
            let result = lref.put_value(ctx, r);
            // 9. Return r.
            if let Some(r_copy) = r_copy {
                r_copy.store(ctx);
            }
            result.map(|_| r)
        }
    }
}

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope> for ast::AssignmentTarget<'s> {
    type Output = Result<(), ExpressionError>;
    /// ## Register states
    ///
    /// ### Entry condition
    /// ```text
    /// result: value
    /// stack: []
    /// reference: None
    /// reference stack: []
    /// ```
    ///
    /// ### Exit condition
    /// ```text
    /// result: None
    /// stack: []
    /// reference: None
    /// reference stack: []
    /// ```
    fn compile(&'s self, ctx: &mut CompileContext<'_, 's, 'gc, '_>) -> Self::Output {
        self.compile_assignment_target(ctx)
    }
}

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope>
    for ast::AssignmentTargetPattern<'s>
{
    type Output = Result<(), ExpressionError>;
    fn compile(&'s self, ctx: &mut CompileContext<'a, 's, 'gc, 'scope>) -> Self::Output {
        match self {
            ast::AssignmentTargetPattern::ArrayAssignmentTarget(t) => t.compile(ctx),
            ast::AssignmentTargetPattern::ObjectAssignmentTarget(t) => t.compile(ctx),
        }
    }
}

/// Compiles the `SimpleAssignmentTarget` variants of `$ty` to a place, without a cast between
/// the enums.
macro_rules! compile_simple_target_variants {
    ($ty:ident, $node:expr, $ctx:expr) => {
        match $node {
            ast::$ty::AssignmentTargetIdentifier(t) => Ok(t.compile($ctx)),
            ast::$ty::ComputedMemberExpression(t) => t.compile($ctx),
            ast::$ty::StaticMemberExpression(t) => t.compile($ctx),
            ast::$ty::PrivateFieldExpression(t) => t.compile($ctx),
            #[cfg(feature = "typescript")]
            ast::$ty::TSNonNullExpression(t) => {
                t.expression.compile($ctx).map(|r| match r {
                    PlaceOrValue::Place(p) => p,
                    _ => unreachable!(),
                })
            }
            #[cfg(feature = "typescript")]
            ast::$ty::TSAsExpression(t) => {
                t.expression.compile($ctx).map(|r| match r {
                    PlaceOrValue::Place(p) => p,
                    _ => unreachable!(),
                })
            }
            #[cfg(feature = "typescript")]
            ast::$ty::TSSatisfiesExpression(t) => {
                t.expression.compile($ctx).map(|r| match r {
                    PlaceOrValue::Place(p) => p,
                    _ => unreachable!(),
                })
            }
            #[cfg(not(feature = "typescript"))]
            ast::$ty::TSAsExpression(_)
            | ast::$ty::TSNonNullExpression(_)
            | ast::$ty::TSSatisfiesExpression(_) => unreachable!(),
            ast::$ty::TSTypeAssertion(_) => unreachable!(),
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    };
}

/// The `SimpleAssignmentTarget` and `AssignmentTargetPattern` variants that
/// `AssignmentTarget`, `AssignmentTargetMaybeDefault` and `ForStatementLeft` inherit.
pub(crate) trait AssignmentTargetVariants<'s> {
    /// Evaluation of a simple assignment target variant. Panics on other variants.
    fn compile_simple_target<'a, 'gc, 'scope>(
        &'s self,
        ctx: &mut CompileContext<'a, 's, 'gc, 'scope>,
    ) -> Result<Place<'s, 'gc>, ExpressionError>;

    /// DestructuringAssignmentEvaluation of an assignment pattern variant. Panics on other
    /// variants.
    fn compile_pattern_target<'a, 'gc, 'scope>(
        &'s self,
        ctx: &mut CompileContext<'a, 's, 'gc, 'scope>,
    ) -> Result<(), ExpressionError>;

    /// Evaluation of an assignment target variant followed by PutValue of the result
    /// register, as `AssignmentTarget`'s compile does.
    fn compile_assignment_target<'a, 'gc, 'scope>(
        &'s self,
        ctx: &mut CompileContext<'a, 's, 'gc, 'scope>,
    ) -> Result<(), ExpressionError>;
}

macro_rules! impl_assignment_target_variants {
    ($($ty:ident),*) => {
        $(
            impl<'s> AssignmentTargetVariants<'s> for ast::$ty<'s> {
                fn compile_simple_target<'a, 'gc, 'scope>(
                    &'s self,
                    ctx: &mut CompileContext<'a, 's, 'gc, 'scope>,
                ) -> Result<Place<'s, 'gc>, ExpressionError> {
                    let place = compile_simple_target_variants!($ty, self, ctx)?;
                    Ok(place.assignment_target(ctx, false))
                }

                fn compile_pattern_target<'a, 'gc, 'scope>(
                    &'s self,
                    ctx: &mut CompileContext<'a, 's, 'gc, 'scope>,
                ) -> Result<(), ExpressionError> {
                    match self {
                        ast::$ty::ArrayAssignmentTarget(t) => t.compile(ctx),
                        ast::$ty::ObjectAssignmentTarget(t) => t.compile(ctx),
                        _ => unreachable!(),
                    }
                }

                fn compile_assignment_target<'a, 'gc, 'scope>(
                    &'s self,
                    ctx: &mut CompileContext<'a, 's, 'gc, 'scope>,
                ) -> Result<(), ExpressionError> {
                    // result: value
                    // stack: []
                    if matches!(self, match_simple_assignment_target!($ty)) {
                        let needs_load_store = matches!(self, match_member_expression!($ty));
                        let place = if needs_load_store {
                            let value_on_stack = ctx.load_to_stack();
                            // result: None
                            // stack: [value]
                            match self.compile_simple_target(ctx) {
                                Ok(p) => {
                                    // result: None
                                    // stack: [value]
                                    // reference: &target
                                    value_on_stack.store(ctx);
                                    p
                                }
                                Err(err) => {
                                    value_on_stack.forget(ctx);
                                    return Err(err);
                                }
                            }
                        } else {
                            self.compile_simple_target(ctx)?
                        };
                        // result: value
                        // stack: []
                        // reference: &target
                        place.put_value(ctx, ValueOutput::Value)?;
                        // result: None
                        // stack: []
                        // reference: None
                    } else {
                        self.compile_pattern_target(ctx)?;
                    }
                    Ok(())
                }
            }
        )*
    };
}

impl_assignment_target_variants!(AssignmentTarget, AssignmentTargetMaybeDefault, ForStatementLeft);

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope>
    for ast::SimpleAssignmentTarget<'s>
{
    type Output = Result<Place<'s, 'gc>, ExpressionError>;

    fn compile(&'s self, ctx: &mut CompileContext<'a, 's, 'gc, 'scope>) -> Self::Output {
        let place = compile_simple_target_variants!(SimpleAssignmentTarget, self, ctx)?;
        Ok(place.assignment_target(ctx, false))
    }
}

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope>
    for ast::ArrayAssignmentTarget<'s>
{
    type Output = Result<(), ExpressionError>;
    fn compile(&'s self, ctx: &mut CompileContext<'a, 's, 'gc, 'scope>) -> Self::Output {
        let sync_iterator = ctx.push_sync_iterator();
        let array_destructuring = ctx.enter_array_destructuring();
        let mut result = Ok(());
        'args: {
            for element in &self.elements {
                if let Some(element) = element {
                    // AssignmentElement : DestructuringAssignmentTarget Initializer (opt)

                    // 1. If DestructuringAssignmentTarget is neither an
                    //    ObjectLiteral nor an ArrayLiteral, then
                    if let ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(element) =
                        element
                    {
                        // a. Let lRef be ? Evaluation of DestructuringAssignmentTarget.
                        if element.binding.is_simple_assignment_target() {
                            let lref = match element.binding.compile_simple_target(ctx) {
                                Ok(l) => l,
                                Err(e) => {
                                    result = Err(e);
                                    break 'args;
                                }
                            };
                            let needs_push_reference =
                                lref.has_reference() && !element.init.is_literal();
                            if needs_push_reference {
                                ctx.add_instruction(Instruction::PushReference);
                            }
                            // 2. Let value be undefined.
                            // 3. If iteratorRecord.[[Done]] is false, then
                            // a. Let next be ? IteratorStepValue(iteratorRecord).
                            ctx.add_instruction(Instruction::IteratorStepValueOrUndefined);
                            // b. If next is not done, then
                            // i. Set value to next.
                            // 4. If Initializer is present and value is undefined, then
                            //    ...
                            compile_initializer(element, ctx);
                            if needs_push_reference {
                                ctx.add_instruction(Instruction::PopReference);
                            }
                            // 7. Return ? PutValue(lRef, v).
                            if let Err(e) = lref.put_value(ctx, ValueOutput::Value) {
                                result = Err(e);
                                break 'args;
                            }
                        } else {
                            // 2. Let value be undefined.
                            // 3. If iteratorRecord.[[Done]] is false, then
                            // a. Let next be ? IteratorStepValue(iteratorRecord).
                            ctx.add_instruction(Instruction::IteratorStepValueOrUndefined);
                            // b. If next is not done, then
                            // i. Set value to next.
                            // 4. If Initializer is present and value is undefined, then
                            //    ...
                            compile_initializer(element, ctx);
                            // 5. Else,
                            // a. Let v be value.
                            // 6. If DestructuringAssignmentTarget is either an
                            //    ObjectLiteral or an ArrayLiteral, then
                            // a. Let nestedAssignmentPattern be the
                            //    AssignmentPattern that is covered by
                            //    DestructuringAssignmentTarget.
                            let nested_assignment_pattern = &element.binding;
                            // b. Return ? DestructuringAssignmentEvaluation of
                            //    nestedAssignmentPattern with argument v.
                            if let Err(e) = nested_assignment_pattern.compile_pattern_target(ctx) {
                                result = Err(e);
                                break 'args;
                            }
                        }
                    } else if element.is_simple_assignment_target() {
                        // a. Let lRef be ? Evaluation of DestructuringAssignmentTarget.
                        let lref = match element.compile_simple_target(ctx) {
                            Ok(l) => l,
                            Err(e) => {
                                result = Err(e);
                                break 'args;
                            }
                        };
                        // 2. Let value be undefined.
                        // 3. If iteratorRecord.[[Done]] is false, then
                        // a. Let next be ? IteratorStepValue(iteratorRecord).
                        ctx.add_instruction(Instruction::IteratorStepValueOrUndefined);
                        // b. If next is not done, then
                        // i. Set value to next.
                        // 4. If Initializer is present and value is undefined, then
                        //    ...
                        // 5. Else,
                        // a. Let v be value.
                        // 7. Return ? PutValue(lRef, v).
                        if let Err(e) = lref.put_value(ctx, ValueOutput::Value) {
                            result = Err(e);
                            break 'args;
                        }
                    } else {
                        // 2. Let value be undefined.
                        // 3. If iteratorRecord.[[Done]] is false, then
                        // a. Let next be ? IteratorStepValue(iteratorRecord).
                        ctx.add_instruction(Instruction::IteratorStepValueOrUndefined);
                        // b. If next is not done, then
                        // i. Set value to next.
                        // 4. If Initializer is present and value is undefined, then
                        //    ...
                        // 5. Else,
                        // a. Let v be value.
                        // 6. If DestructuringAssignmentTarget is either an ObjectLiteral or an ArrayLiteral, then
                        // a. Let nestedAssignmentPattern be the AssignmentPattern that is covered by DestructuringAssignmentTarget.
                        // b. Return ? DestructuringAssignmentEvaluation of nestedAssignmentPattern with argument v.
                        let nested_assignment_pattern = element;
                        if let Err(e) = nested_assignment_pattern.compile_pattern_target(ctx) {
                            result = Err(e);
                            break 'args;
                        }
                    }
                } else {
                    // Elision : ,
                    // 1. If iteratorRecord.[[Done]] is false, then
                    // a. Perform ? IteratorStep(iteratorRecord).
                    ctx.add_instruction(Instruction::IteratorStepValueOrUndefined);
                    // 2. Return unused.
                }
            }
            if let Some(rest) = &self.rest {
                if rest.target.is_simple_assignment_target() {
                    let target = &rest.target;
                    // 1. If DestructuringAssignmentTarget is neither an
                    //    ObjectLiteral nor an ArrayLiteral, then
                    // a. Let lRef be ? Evaluation of
                    //    DestructuringAssignmentTarget.
                    let lref = match target.compile_simple_target(ctx) {
                        Ok(l) => l,
                        Err(e) => {
                            result = Err(e);
                            break 'args;
                        }
                    };
                    ctx.add_instruction(Instruction::IteratorRestIntoArray);
                    // a. Return ? PutValue(lRef, A).
                    if let Err(e) = lref.put_value(ctx, ValueOutput::Value) {
                        result = Err(e);
                        break 'args;
                    }
                } else {
                    // 5. If DestructuringAssignmentTarget is neither an
                    //    ObjectLiteral nor an ArrayLiteral, then
                    ctx.add_instruction(Instruction::IteratorRestIntoArray);
                    // 6. Let nestedAssignmentPattern be the AssignmentPattern that
                    //    is covered by DestructuringAssignmentTarget.
                    let nested_assignment_pattern = &rest.target;
                    // 7. Return ? DestructuringAssignmentEvaluation of
                    //    nestedAssignmentPattern with argument A.
                    if let Err(e) = nested_assignment_pattern.compile_pattern_target(ctx) {
                        result = Err(e);
                        break 'args;
                    }
                }
            }
        }
        // Note: An error during IteratorClose should not jump into
        // IteratorCloseWithError, hence we pop exception jump target here.
        let jump_to_iterator_close_handler = array_destructuring.exit(ctx);
        let jump_to_iterator_pop = sync_iterator.exit(ctx);
        let jump_over_catch = ctx.add_instruction_with_jump_slot(Instruction::Jump);
        // 3. If status is an abrupt completion, then
        {
            ctx.set_jump_target_here(jump_to_iterator_close_handler);
            // a. If iteratorRecord.[[Done]] is false, return
            //    ? IteratorClose(iteratorRecord, status).
            // Note: removing jump_to_iterator_pop catch handler.
            ctx.add_instruction(Instruction::PopExceptionJumpTarget);
            ctx.add_instruction(Instruction::IteratorCloseWithError);
            ctx.set_jump_target_here(jump_to_iterator_pop);
            ctx.add_instruction(Instruction::IteratorPop);
            ctx.add_instruction(Instruction::Throw);
        }
        ctx.set_jump_target_here(jump_over_catch);
        result
    }
}

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope>
    for ast::ObjectAssignmentTarget<'s>
{
    type Output = Result<(), ExpressionError>;
    fn compile(&'s self, ctx: &mut CompileContext<'a, 's, 'gc, 'scope>) -> Self::Output {
        // result: source
        // stack: []

        ctx.add_instruction(Instruction::ToObject);
        // result: source (converted to object)
        // stack: []

        // Each property and the rest binding require access to the source as
        // object: thus, we effectively need to create a copy of it on the
        // stack before each property call _except_ for the very last one
        // (including the rest binding; before rest we never need a copy as it
        // is always the last one).
        let has_rest = self.rest.is_some();
        let store_copy_cutoff = if has_rest {
            if self.properties.is_empty() {
                // Only rest: we don't need to bother with properties or anything.
                return compile_assignment_target_rest(self.rest.as_ref().unwrap(), ctx, 0);
            } else {
                // Properties and rest: we need to create a copy of source on
                // the stack, and our cutoff happens on the last property:
                // before it we need to StoreCopy, at cutoff we use Store.
                ctx.add_instruction(Instruction::LoadCopy);
                self.properties.len() - 1
            }
        } else {
            if self.properties.is_empty() {
                // No rest and no properties: we've done all we need to do.
                return Ok(());
            }
            if self.properties.len() == 1 {
                // Only one property: we can just compile the property directly.
                return compile_assignment_target_property(
                    self.properties.first().unwrap(),
                    ctx,
                    false,
                );
            } else {
                // At least two properties: we need to create a copy of source
                // on the stack, and our cutoff happens on the second to last
                // property: before it we need to StoreCopy, at cutoff we use
                // Store, and the last property does nothing.
                ctx.add_instruction(Instruction::LoadCopy);
                self.properties.len() - 2
            }
        };
        // result: source
        // stack: [source?]
        for (index, property) in self.properties.iter().enumerate() {
            // result: source
            // stack: [source?]
            compile_assignment_target_property(property, ctx, has_rest)?;
            // result: None
            // stack: [source?]
            match index.cmp(&store_copy_cutoff) {
                std::cmp::Ordering::Less => {
                    // If index is less than the cutoff, there are still more
                    // properties coming after this that need the source. Thus
                    // we must perform a StoreCopy to get source back into
                    // result without removing it from stack.
                    ctx.add_instruction(Instruction::StoreCopy);
                    // result: source
                    // stack: [source]
                }
                std::cmp::Ordering::Equal => {
                    // If index is equal to cutoff, it means that the next
                    // property is the last one that needs the source. Thus we
                    // perform a Store to get the source back into result while
                    // removing it from stack.
                    ctx.add_instruction(Instruction::Store);
                    // result: source
                    // stack: []
                }
                std::cmp::Ordering::Greater => {
                    // If index is greater than cutoff, it means that this is
                    // the last property, no rest property exists after this,
                    // and the stack is empty. We need do nothing here.
                    // result: None
                    // stack: []
                }
            }
        }
        if let Some(rest) = &self.rest {
            // result: source
            // stack: []
            compile_assignment_target_rest(rest, ctx, self.properties.len())?;
        }
        // result: None
        // stack: []
        // reference: None
        // reference stack: []
        Ok(())
    }
}

fn compile_assignment_target_property<'s>(
    property: &'s ast::AssignmentTargetProperty<'s>,
    ctx: &mut CompileContext<'_, 's, '_, '_>,
    has_rest: bool,
) -> Result<(), ExpressionError> {
    match property {
        ast::AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(identifier) => {
            // result: source
            // stack: [source?]
            let key = ctx.create_string(identifier.binding.name.as_str());
            ctx.add_instruction_with_identifier(
                Instruction::EvaluatePropertyAccessWithIdentifierKey,
                key.to_property_key(),
            );
            let place: Place = key.to_property_key().into();
            // result: None
            // stack: [source?]
            // reference: &source.identifier
            place.get_value_maybe_keep_reference(ctx, has_rest)?;
            if has_rest {
                debug_assert!(place.has_reference());
                ctx.add_instruction(Instruction::PushReference);
            }
            // result: source.identifier
            // stack: [source?]
            // reference: None
            // reference stack: [&source.identifier?]
            identifier.compile(ctx)?;
            // result: None
            // stack: [source?]
            // reference: None
            // reference stack: [&source.identifier?]
        }
        ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(property)
            if !has_rest && !property.computed =>
        {
            let (is_simple_target, default) = match &property.binding {
                ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(target) => {
                    (target.binding.is_simple_assignment_target(), Some(&**target))
                }
                target => (target.is_simple_assignment_target(), None),
            };
            if !is_simple_target {
                return compile_assignment_target_property_value(property, ctx, has_rest);
            }
            // KeyedDestructuringAssignmentEvaluation: the target is evaluated
            // before the property is read.
            // result: source
            // stack: [source?]
            let source = ctx.load_to_stack();
            // stack: [source?, source]
            // 1. Let lRef be ? Evaluation of DestructuringAssignmentTarget.
            let lref = match &property.binding {
                ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(target) => {
                    target.binding.compile_simple_target(ctx)
                }
                target => target.compile_simple_target(ctx),
            };
            let lref = match lref {
                Ok(lref) => lref,
                Err(err) => {
                    source.forget(ctx);
                    return Err(err);
                }
            };
            let push_reference = lref.has_reference();
            if push_reference {
                ctx.add_instruction(Instruction::PushReference);
            }
            source.store(ctx);
            // result: source
            // stack: [source?]
            // 2. Let v be ? GetV(value, propertyName).
            let place = property.name.compile(ctx)?;
            place.get_value_maybe_keep_reference(ctx, false)?;
            // result: source.property
            // 3. If Initializer is present and v is undefined, then
            if let Some(default) = default {
                compile_initializer(default, ctx);
            }
            if push_reference {
                ctx.add_instruction(Instruction::PopReference);
            }
            // 4. Return ? PutValue(lRef, v).
            lref.put_value(ctx, ValueOutput::Value)?;
        }
        ast::AssignmentTargetProperty::AssignmentTargetPropertyProperty(property) => {
            compile_assignment_target_property_value(property, ctx, has_rest)?;
        }
    }
    // result: None
    // stack: [source?]
    // reference: None
    // reference stack: [&source.property?]
    Ok(())
}

fn compile_assignment_target_property_value<'s>(
    property: &'s ast::AssignmentTargetPropertyProperty<'s>,
    ctx: &mut CompileContext<'_, 's, '_, '_>,
    has_rest: bool,
) -> Result<(), ExpressionError> {
    // result: source
    // stack: [source?]
    let place = property.name.compile(ctx)?;
    // result: None
    // stack: [source?]
    // reference: &source.property
    place.get_value_maybe_keep_reference(ctx, has_rest)?;
    if has_rest {
        debug_assert!(place.has_reference());
        ctx.add_instruction(Instruction::PushReference);
    }
    // result: source.property
    // stack: [source?]
    // reference: None
    // reference stack: [&source.property?]
    property.binding.compile(ctx)
    // result: None
    // stack: [source?]
    // reference: None
    // reference stack: [&source.property?]
}

fn compile_assignment_target_rest<'s>(
    rest: &'s ast::AssignmentTargetRest<'s>,
    ctx: &mut CompileContext<'_, 's, '_, '_>,
    property_count: usize,
) -> Result<(), ExpressionError> {
    // result: source
    // stack: []
    // reference: None
    // reference stack: [...source.properties]
    ctx.add_instruction_with_immediate(Instruction::CopyDataPropertiesIntoObject, property_count);
    // result: object copy
    // stack: []
    // reference: None
    // reference stack: []
    rest.target.compile(ctx)
    // result: None
    // stack: []
    // reference: None
    // reference stack: []
}

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope>
    for ast::AssignmentTargetPropertyIdentifier<'s>
{
    type Output = Result<(), ExpressionError>;
    fn compile(&'s self, ctx: &mut CompileContext<'a, 's, 'gc, 'scope>) -> Self::Output {
        // result: binding
        // stack: []

        // Note: the caller is expected to handle the self.binding side of
        // this! When we enter here, self.binding property access result should
        // be in the result register.
        if let Some(init) = &self.init {
            let binding_copy = ctx.load_copy_to_stack();
            // result: binding
            // stack: [binding]
            ctx.add_instruction(Instruction::IsUndefined);
            // result: binding === undefined
            // stack: [binding]
            let jump_slot = ctx.add_instruction_with_jump_slot(Instruction::JumpIfNot);
            binding_copy.store(ctx);
            // result: binding
            // stack: []
            if is_anonymous_function_definition(init) {
                let identifier_string = ctx.create_string(self.binding.name.as_str());
                ctx.add_instruction_with_constant(Instruction::StoreConstant, identifier_string);
                ctx.name_identifier = Some(NamedEvaluationParameter::Result);
            }
            // Ignore errors: this is not an unconditional path.
            let _ = init.compile(ctx).and_then(|r| r.get_value(ctx));
            ctx.name_identifier = None;
            // result: init
            // stack: []
            let init_on_stack = ctx.load_to_stack();
            // result: None
            // stack: [init]
            ctx.set_jump_target_here(jump_slot);
            // result: None
            // stack: [binding / init]
            init_on_stack.store(ctx);
            // result: binding / init
            // stack: []
        }
        let place = self.binding.compile(ctx).assignment_target(ctx, false);
        // result: binding / init
        // stack: []
        // reference: &binding
        place.put_value(ctx, ValueOutput::Value)?;
        // result: None
        // stack: []
        // reference: None
        Ok(())
    }
}

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope> for ast::PropertyKey<'s> {
    type Output = Result<Place<'s, 'gc>, ExpressionError>;
    /// ## Register states
    ///
    /// ### Entry condition
    /// ```text
    /// result: source
    /// stack: []
    /// reference: None
    /// reference stack: []
    /// ```
    ///
    /// ### Exit condition
    /// ```text
    /// result: None
    /// stack: []
    /// reference: &source.property
    /// reference stack: []
    /// ```
    fn compile(&'s self, ctx: &mut CompileContext<'a, 's, 'gc, 'scope>) -> Self::Output {
        // result: source
        // stack: []
        match self {
            ast::PropertyKey::StaticIdentifier(identifier) => {
                Ok(identifier.compile(ctx))
                // result: None
                // stack: []
                // reference: &source.identifier
            }
            // Note: Private names are not allowed in this position.
            ast::PropertyKey::PrivateIdentifier(_) => unreachable!(),
            _ => {
                let source_on_stack = ctx.load_to_stack();
                // result: None
                // stack: [source]
                let expr_result = self.compile_expression(ctx).and_then(|r| r.get_value(ctx));

                // Source on stack is either forget on the stack and cleaned up
                // by try-catch if expr is Err, or is consumed by below
                // instruction.
                source_on_stack.forget(ctx);

                let expr_result = expr_result?;

                // result: expr
                // stack: [source]
                ctx.add_instruction(Instruction::EvaluatePropertyAccessWithExpressionKey);
                // result: None
                // stack: []
                // reference: &source[expr]
                Ok(expr_result.to_expression_key())
            }
        }
    }
}

fn compile_initializer<'s>(
    target: &'s ast::AssignmentTargetWithDefault<'s>,
    ctx: &mut CompileContext<'_, 's, '_, '_>,
) {
    // result: value
    // stack: []
    let value_copy = ctx.load_copy_to_stack();
    ctx.add_instruction(Instruction::IsUndefined);
    // result: value === undefined
    // stack: [value]
    let jump_slot = ctx.add_instruction_with_jump_slot(Instruction::JumpIfNot);
    // result: None
    // stack: [value]
    value_copy.store(ctx);
    // result: value
    // stack: []
    if is_anonymous_function_definition(&target.init)
        && let ast::AssignmentTarget::AssignmentTargetIdentifier(identifier) = &target.binding
    {
        let identifier_string = ctx.create_string(identifier.name.as_str());
        ctx.add_instruction_with_constant(Instruction::StoreConstant, identifier_string);
        ctx.name_identifier = Some(NamedEvaluationParameter::Result);
    }
    // Note: ignore errors; this is not an unconditional path.
    let _ = target
        .init
        .compile(ctx)
        .and_then(|init| init.get_value(ctx));
    ctx.name_identifier = None;
    // result: init
    // stack: []
    let init_on_stack = ctx.load_to_stack();
    // result: None
    // stack: [init]
    ctx.set_jump_target_here(jump_slot);
    // result: None
    // stack: [value / init]
    init_on_stack.store(ctx);
    // result: value / init
    // stack: []
}

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope>
    for ast::AssignmentTargetMaybeDefault<'s>
{
    type Output = Result<(), ExpressionError>;
    /// ## Register states
    ///
    /// ### Entry condition
    /// ```text
    /// result: value
    /// stack: []
    /// reference: None
    /// reference stack: []
    /// ```
    ///
    /// ### Exit condition
    /// ```text
    /// result: None
    /// stack: []
    /// reference: &source.property
    /// reference stack: []
    /// ```
    fn compile(&'s self, ctx: &mut CompileContext<'_, 's, '_, '_>) -> Self::Output {
        match self {
            ast::AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(target) => {
                // result: value
                // stack: []
                compile_initializer(target, ctx);
                // result: value / init
                // stack: []
                target.binding.compile(ctx)?;
                // result: None
                // stack: []
            }
            _ => {
                // result: value
                // stack: []
                self.compile_assignment_target(ctx)?;
                // result: None
                // stack: []
            }
        }
        Ok(())
    }
}
