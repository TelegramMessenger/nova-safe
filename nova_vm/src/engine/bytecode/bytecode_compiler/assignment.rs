// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use oxc_ast::ast::{self, AssignmentOperator, LogicalOperator};
use oxc_ast::{
    ast::{AssignmentTarget, AssignmentTargetMaybeDefault, ForStatementLeft},
    match_member_expression, match_simple_assignment_target,
};

use super::CompileExpressionVariants;

#[cfg(feature = "typescript")]
use crate::engine::bytecode::bytecode_compiler::PlaceOrValue;
use crate::engine::{
    Instruction,
    bytecode::bytecode_compiler::{
        CompileContext, CompileEvaluation, ExpressionError, NamedEvaluationParameter, Place,
        ValueOutput, is_anonymous_function_definition,
    },
};

impl<'a, 's, 'gc, 'scope> CompileEvaluation<'a, 's, 'gc, 'scope> for ast::AssignmentExpression<'s> {
    type Output = Result<ValueOutput<'gc>, ExpressionError>;
    fn compile(&'s self, ctx: &mut CompileContext<'a, 's, 'gc, 'scope>) -> Self::Output {
        let mut do_named_evaluation = false;
        // 1. Let lref be ? Evaluation of LeftHandSideExpression.
        let lref = match &self.left {
            ast::AssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                let place = identifier.compile(ctx);
                // NOTE: If the left hand side does not constitute the start of
                // the assignment expression span, then it means that the left
                // side is inside parentheses and NamedEvaluation should not
                // happen.
                do_named_evaluation = self.span.start == identifier.span.start;
                place
            }
            ast::AssignmentTarget::ComputedMemberExpression(expression) => {
                // 1. If LeftHandSideExpression is neither an ObjectLiteral nor an ArrayLiteral, then
                expression.compile(ctx)?
            }
            ast::AssignmentTarget::ArrayAssignmentTarget(_)
            | ast::AssignmentTarget::ObjectAssignmentTarget(_) => {
                assert_eq!(
                    self.operator,
                    AssignmentOperator::Assign,
                    "SyntaxError: Invalid left-hand side in assignment expression"
                );
                // 2. Let assignmentPattern be the AssignmentPattern that is covered by LeftHandSideExpression.
                // 3. Let rRef be ? Evaluation of AssignmentExpression.
                // 4. Let rVal be ? GetValue(rRef).
                let rval = self.right.compile(ctx)?.get_value(ctx)?;
                // 5. Perform ? DestructuringAssignmentEvaluation of assignmentPattern with argument rVal.
                let rval_copy = ctx.load_copy_to_stack();
                let result = self.left.compile_pattern_target(ctx);
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
                do_named_evaluation = self.span.start == x.span.start;
                place
            }
            #[cfg(feature = "typescript")]
            ast::AssignmentTarget::TSAsExpression(x) => {
                let PlaceOrValue::Place(place) = x.expression.compile(ctx)? else {
                    unreachable!()
                };
                do_named_evaluation = self.span.start == x.span.start;
                place
            }
            #[cfg(feature = "typescript")]
            ast::AssignmentTarget::TSSatisfiesExpression(x) => {
                let PlaceOrValue::Place(place) = x.expression.compile(ctx)? else {
                    unreachable!()
                };
                do_named_evaluation = self.span.start == x.span.start;
                place
            }
            #[cfg(not(feature = "typescript"))]
            ast::AssignmentTarget::TSAsExpression(_)
            | ast::AssignmentTarget::TSNonNullExpression(_)
            | ast::AssignmentTarget::TSSatisfiesExpression(_) => unreachable!(),
            ast::AssignmentTarget::TSTypeAssertion(_) => unreachable!(),
        };

        if self.operator.is_assign() {
            let push_reference = lref.has_reference() && !self.right.is_literal();

            if push_reference {
                ctx.add_instruction(Instruction::PushReference);
            }

            if do_named_evaluation && is_anonymous_function_definition(&self.right) {
                ctx.add_instruction_with_constant(
                    Instruction::StoreConstant,
                    lref.identifier().unwrap(),
                );
                ctx.name_identifier = Some(NamedEvaluationParameter::Result);
            }

            let rval = self.right.compile(ctx)?.get_value(ctx)?;

            let rval_copy = ctx.load_copy_to_stack();

            if push_reference {
                ctx.add_instruction(Instruction::PopReference);
            }

            let result = lref.put_value(ctx, rval);
            // ... Return rval.
            rval_copy.store(ctx);
            result.map(|_| rval)
        } else if let Some(operator) = self.operator.to_logical_operator() {
            // 2. Let lval be ? GetValue(lref).
            lref.get_value_keep_reference(ctx)?;
            let push_reference = lref.has_reference() && !self.right.is_literal();
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
                if do_named_evaluation && is_anonymous_function_definition(&self.right) {
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
                let Ok(rval) = self.right.compile(ctx).and_then(|r| r.get_value(ctx)) else {
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
            let do_push_reference = lref.has_reference() && !self.right.is_literal();
            if do_push_reference {
                ctx.add_instruction(Instruction::PushReference);
            }
            // 3. Let rref be ? Evaluation of AssignmentExpression.
            // 4. Let rval be ? GetValue(rref).
            let _rval = match self.right.compile(ctx).and_then(|r| r.get_value(ctx)) {
                Ok(r) => r,
                Err(err) => {
                    lval_copy.forget(ctx);
                    return Err(err);
                }
            };

            // 5. Let assignmentOpText be the source text matched by AssignmentOperator.
            // 6. Let opText be the sequence of Unicode code points associated with assignmentOpText in the following table:
            let op_text = self.operator.to_binary_operator().unwrap();
            // 7. Let r be ? ApplyStringOrNumericBinaryOperator(lval, opText, rval).
            let op_text = match op_text {
                ast::BinaryOperator::Addition => Instruction::ApplyAdditionBinaryOperator,
                ast::BinaryOperator::Subtraction => Instruction::ApplySubtractionBinaryOperator,
                ast::BinaryOperator::Multiplication => {
                    Instruction::ApplyMultiplicationBinaryOperator
                }
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
            };
            // Consumed by instruction.
            lval_copy.forget(ctx);
            ctx.add_instruction(op_text);
            let r_copy = ctx.load_copy_to_stack();
            let r = ValueOutput::Value;
            if do_push_reference {
                ctx.add_instruction(Instruction::PopReference);
            }
            // 8. Perform ? PutValue(lref, r).
            let result = lref.put_value(ctx, r);
            // 9. Return r.
            r_copy.store(ctx);
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
                    compile_simple_target_variants!($ty, self, ctx)
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
        compile_simple_target_variants!(SimpleAssignmentTarget, self, ctx)
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
        let place = self.binding.compile(ctx);
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
