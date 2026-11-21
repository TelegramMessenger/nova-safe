// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use core::{
    hash::{Hash, Hasher},
    marker::PhantomData,
};

use crate::hashing::AHashMap;

use num_traits::Num;
#[cfg(feature = "regexp")]
use oxc_ast::ast::RegExpFlags;
use wtf8::Wtf8Buf;

#[cfg(feature = "regexp")]
use crate::ecmascript::{RegExp, reg_exp_create_literal};
use crate::{
    ecmascript::{
        Agent, BigInt, ExceptionType, Number, ObjectShape, PropertyKey, PropertyLookupCache,
        String, Value,
    },
    engine::{
        Bindable, Executable, ExecutableHeapData, FunctionExpression, Instruction, NoGcScope,
        bytecode::executable::ArrowFunctionExpression,
    },
    heap::{CreateHeapData, HeapIndexHandle},
};

use super::{IndexType, JumpIndex};
use crate::engine::{Bytecode, CallEnvironment, CodeTables, SwitchTable};

/// The ThrowError immediate of an executable that stands in for code using
/// syntax the compiler does not support. Running it panics, as compiling that
/// code did before functions were compiled ahead of their first call.
pub(crate) const UNSUPPORTED_SYNTAX_IMMEDIATE: IndexType = IndexType::MAX;

/// From this many constants on, new literal constants go to the wide table.
const WIDE_CONSTANTS_FROM: usize = 60_000;
/// From this many constants on, new property names in member expressions are
/// wide constants too, leaving the last indices for binding names.
const WIDE_PROPERTY_NAMES_FROM: usize = 64_000;

/// How many of the last instructions peephole fusion looks back at.
const RECENT_INSTRUCTIONS: usize = 8;

const TOO_MANY_NAMES: &str = "too many identifiers and constants in one function or script";
const TOO_MANY_PROPERTY_NAMES: &str = "too many property names in one function or script";
const TOO_MANY_FUNCTIONS: &str = "too many functions in one function or script";
const TOO_MANY_CLASSES: &str = "too many classes in one function or script";
const TOO_MANY_OBJECT_LITERALS: &str = "too many object literals in one function or script";
const TOO_MANY_VALUES: &str = "too many variables, arguments or values in one function or script";

/// Context for executable creation only. This struct contains the things
/// needed for adding instructions and nothing else, no scope tracking and
/// other metadata is included.
pub(super) struct ExecutableContext<'agent, 'gc, 'scope> {
    pub(crate) agent: &'agent mut Agent,
    pub(crate) gc: NoGcScope<'gc, 'scope>,
    /// Current depth of the Value stack.
    current_value_stack_depth: u32,
    /// true if the current last instruction is a terminal instruction and no
    /// jumps point past it.
    current_instruction_pointer_is_unreachable: bool,
    /// Instructions being built
    instructions: Vec<u8>,
    /// Start offsets of the last instructions added since the last jump
    /// target, most recent first: the window that peephole fusion may
    /// combine.
    recent: [usize; RECENT_INSTRUCTIONS],
    recent_len: usize,
    /// Caches being built
    caches: Vec<PropertyLookupCache<'gc>>,
    cache_indexes: AHashMap<PropertyLookupCache<'gc>, usize>,
    /// The stack slots of kept names (GetBindingAtSiteOnce) that the code
    /// emitted since the last jump target has read, each with where the
    /// instruction that read it starts: a later read in the same run of
    /// code finds the slot filled, or the name without a value to keep.
    kept_slots_read: Vec<(IndexType, usize)>,
    /// Where the most recent GetBindingAtSiteOnce starts, if it reads a
    /// slot that an earlier one of the same run of code has read.
    kept_read_again: Option<usize>,
    /// The BindingSites the instructions use, one for each place in the
    /// code that names a variable whose scope chain is fixed: where the
    /// name of each is among the constants, once an instruction names it.
    binding_site_names: Vec<IndexType>,
    /// Switch tables being built: for each, the constant of every selector
    /// and where the statements of its case start, once that is known.
    switch_tables: Vec<Vec<(IndexType, u32)>>,
    /// Constants being built
    constants: Vec<Value<'gc>>,
    constant_indexes: AHashMap<ConstantKey<'gc>, usize>,
    /// Literal constants added once `constants` is near the 16-bit index
    /// limit, so that identifiers and property keys keep fitting there.
    wide_constants: Vec<Value<'gc>>,
    wide_constant_indexes: AHashMap<ConstantKey<'gc>, usize>,
    /// Object Shapes being built
    shapes: Vec<ObjectShape<'gc>>,
    /// Function expressions being built
    function_expressions: Vec<FunctionExpression<'gc>>,
    /// Arrow function expressions being built
    arrow_function_expressions: Vec<ArrowFunctionExpression<'gc>>,
    class_initializer_bytecodes: Vec<(Option<Executable<'gc>>, bool)>,
    /// The field initializers of a derived class with a constructor, run
    /// after each super call of the constructor.
    instance_initializer: Option<Executable<'gc>>,
    call_environment: CallEnvironment,
    stack_parameters: Option<u16>,
    /// Set when an index did not fit its bytecode operand: the executable
    /// is finished as one that throws this SyntaxError.
    too_many: Option<&'static str>,
}

impl<'agent, 'gc, 'scope> ExecutableContext<'agent, 'gc, 'scope> {
    pub(super) fn new(agent: &'agent mut Agent, gc: NoGcScope<'gc, 'scope>) -> Self {
        Self {
            agent,
            gc,
            current_value_stack_depth: 0,
            current_instruction_pointer_is_unreachable: false,
            instructions: Vec::new(),
            recent: [0; RECENT_INSTRUCTIONS],
            recent_len: 0,
            caches: Vec::new(),
            cache_indexes: AHashMap::default(),
            kept_slots_read: Vec::new(),
            kept_read_again: None,
            binding_site_names: Vec::new(),
            switch_tables: Vec::new(),
            constants: Vec::new(),
            constant_indexes: AHashMap::default(),
            wide_constants: Vec::new(),
            wide_constant_indexes: AHashMap::default(),
            shapes: Vec::new(),
            function_expressions: Vec::new(),
            arrow_function_expressions: Vec::new(),
            class_initializer_bytecodes: Vec::new(),
            instance_initializer: None,
            call_environment: CallEnvironment::Function,
            stack_parameters: None,
            too_many: None,
        }
    }

    pub(super) fn get_agent_and_gc(&mut self) -> (&mut Agent, NoGcScope<'gc, 'scope>) {
        (&mut self.agent, self.gc)
    }

    pub(crate) fn get_agent(&self) -> &Agent {
        self.agent
    }

    pub(crate) fn get_agent_mut(&mut self) -> &mut Agent {
        self.agent
    }

    pub(super) fn create_property_lookup_cache(
        &mut self,
        identifier: PropertyKey<'gc>,
    ) -> PropertyLookupCache<'gc> {
        PropertyLookupCache::new(self.agent, identifier)
    }

    /// Push a Value onto the bytecode VM Stack.
    pub(super) fn push_stack(&mut self) -> u32 {
        let curr = self.current_value_stack_depth;
        self.current_value_stack_depth += 1;
        curr
    }

    /// Pop a Value from the bytecode VM Stack.
    pub(super) fn pop_stack(&mut self) {
        self.current_value_stack_depth -= 1;
    }

    #[inline(always)]
    pub(super) fn stack_depth(&self) -> usize {
        self.current_value_stack_depth as usize
    }

    pub(super) fn create_bigint(&mut self, literal: &str, radix: u32) -> BigInt<'gc> {
        if let Ok(result) = i64::from_str_radix(literal, radix) {
            BigInt::from_i64(self.agent, result).bind(self.gc)
        } else {
            BigInt::from_num_bigint(
                self.agent,
                num_bigint::BigInt::from_str_radix(literal, radix).unwrap(),
            )
            .bind(self.gc)
        }
    }

    pub(super) fn create_number(&mut self, value: f64) -> Number<'gc> {
        Number::from_f64(self.agent, value, self.gc)
    }

    pub(crate) fn create_property_key(&mut self, literal: &str) -> PropertyKey<'gc> {
        PropertyKey::from_str(self.agent, literal, self.gc)
    }

    #[cfg(feature = "regexp")]
    pub(crate) fn create_regexp(&mut self, literal: &str, flags: RegExpFlags) -> RegExp<'gc> {
        let pattern = self.create_string(literal);
        reg_exp_create_literal(self.agent, pattern, Some(flags), self.gc)
    }

    pub(super) fn create_string(&mut self, literal: &str) -> String<'gc> {
        String::from_str(self.agent, literal, self.gc)
    }

    pub(super) fn create_string_from_owned(&mut self, owned: std::string::String) -> String<'gc> {
        String::from_string(self.agent, owned, self.gc)
    }

    pub(super) fn create_string_from_wtf8_buf(&mut self, buf: Wtf8Buf) -> String<'gc> {
        String::from_wtf8_buf(self.agent, buf, self.gc)
    }

    /// Returns true if the current instruction pointer is a unreachable.
    pub(super) fn is_unreachable(&self) -> bool {
        self.current_instruction_pointer_is_unreachable
    }

    pub(super) fn finish_as_unsupported_stub(self, message: &'static str) -> Executable<'gc> {
        self.finish_as_stub(message, UNSUPPORTED_SYNTAX_IMMEDIATE as usize)
    }

    fn finish_as_stub(mut self, message: &'static str, immediate: usize) -> Executable<'gc> {
        self.current_value_stack_depth = 0;
        self.current_instruction_pointer_is_unreachable = false;
        self.instructions.clear();
        self.forget_recent_instructions();
        self.caches.clear();
        self.cache_indexes.clear();
        self.switch_tables.clear();
        self.constants.clear();
        self.constant_indexes.clear();
        self.wide_constants.clear();
        self.wide_constant_indexes.clear();
        self.shapes.clear();
        self.function_expressions.clear();
        self.arrow_function_expressions.clear();
        self.class_initializer_bytecodes.clear();
        self.instance_initializer = None;
        self.call_environment = CallEnvironment::Function;
        self.stack_parameters = None;
        self.too_many = None;
        let message = self.create_string(message);
        self.add_instruction_with_constant(Instruction::StoreConstant, message);
        self.add_instruction_with_immediate(Instruction::ThrowError, immediate);
        self.build()
    }

    pub(super) fn finish(mut self) -> Executable<'gc> {
        if let Some(message) = self.too_many.take() {
            // The bytecode refers to entries by truncated indexes and must
            // never run. The code that parsed this reports the SyntaxError;
            // the stub throws it should anything still call this code.
            self.agent.compile_limit_error.get_or_insert(message);
            return self.finish_as_unsupported_stub(message);
        }
        self.build()
    }

    fn build(mut self) -> Executable<'gc> {
        for index in 0..self.function_expressions.len() {
            if self.function_expressions[index]
                .template
                .executable
                .is_none()
            {
                // A class constructor whose class body always throws before
                // the constructor is compiled: its closure is never reachable.
                let stub = ExecutableContext::new(&mut *self.agent, self.gc).finish_as_stub(
                    "Class constructor was not compiled",
                    ExceptionType::TypeError as usize,
                );
                self.function_expressions[index].template.executable = Some(stub);
            }
        }
        let switch_tables = core::mem::take(&mut self.switch_tables)
            .into_iter()
            .map(|cases| SwitchTable::new(cases, &self.constants))
            .collect();
        let instructions = Bytecode::new(self.instructions);
        self.agent.heap.create(ExecutableHeapData {
            tables: CodeTables::new(
                instructions.clone(),
                self.constants.unbind(),
                self.caches.unbind(),
                &self.binding_site_names,
            ),
            instructions,
            wide_constants: self.wide_constants.unbind().into_boxed_slice(),
            shapes: self.shapes.unbind().into_boxed_slice(),
            function_expressions: self.function_expressions.unbind().into_boxed_slice(),
            arrow_function_expressions: self.arrow_function_expressions.unbind().into_boxed_slice(),
            class_initializer_bytecodes: self
                .class_initializer_bytecodes
                .into_iter()
                .map(|(exe, b)| (exe.unbind(), b))
                .collect(),
            instance_initializer: self.instance_initializer.unbind(),
            call_environment: self.call_environment,
            stack_parameters: self.stack_parameters,
            switch_tables,
            _gc: PhantomData,
        })
    }

    pub(super) fn add_instruction(&mut self, instruction: Instruction) {
        debug_assert_eq!(instruction.argument_count(), 0);
        debug_assert!(
            !instruction.has_constant_index()
                && !instruction.has_function_expression_index()
                && !instruction.has_identifier_index()
        );
        match instruction {
            Instruction::Load => {
                if let Some(start) = self.recent_instruction(0, Instruction::GetValueFromIndex) {
                    self.instructions[start] = Instruction::LoadFromIndex.as_u8();
                    return;
                }
                if let Some(start) =
                    self.recent_instruction(0, Instruction::LoadFromIndexGetFromIndex)
                {
                    self.instructions[start] = Instruction::LoadFromIndexLoadFromIndex.as_u8();
                    return;
                }
                if let Some(start) = self.recent_instruction(0, Instruction::PushReference) {
                    // The function of a method call with its Reference:
                    // found, kept and pushed by one instruction.
                    if let Some(property) =
                        self.recent_instruction(1, Instruction::GetPropertyWithCacheKeepReference)
                    {
                        self.instructions[property] =
                            Instruction::LoadPropertyWithCachePushReference.as_u8();
                        self.drop_recent_instructions(start, 1);
                        return;
                    }
                    self.instructions[start] = Instruction::PushReferenceAndLoad.as_u8();
                    return;
                }
                // The value of an operator, pushed by the operator itself.
                if let Some(description) = self.recent_fused_operation() {
                    let operation = self.operand(description);
                    self.set_operand(description, operation | Instruction::FUSED_PUSHES);
                    return;
                }
                if let Some((start, operator)) = self.recent_operator() {
                    self.drop_recent_instructions(start, 1);
                    self.push_instruction(Instruction::ApplyBinaryOperatorAndMove);
                    self.add_index(
                        Instruction::fused_slot_with_operator(0, operator)
                            | Instruction::FUSED_PUSHES,
                    );
                    return;
                }
                // A variable of an enclosing function, pushed.
                if let Some(start) = self.recent_instruction(0, Instruction::GetBindingAtSite) {
                    self.instructions[start] = Instruction::LoadBindingAtSite.as_u8();
                    return;
                }
                if let Some(start) = self.recent_instruction(0, Instruction::GetBindingAtSiteOnce) {
                    self.instructions[start] = Instruction::LoadBindingAtSiteOnce.as_u8();
                    return;
                }
                // The property of a local, pushed.
                if let Some(start) =
                    self.recent_instruction(0, Instruction::GetIndexPropertyWithCache)
                {
                    self.instructions[start] = Instruction::LoadIndexPropertyWithCache.as_u8();
                    return;
                }
                // A constant pushed right after another push: there is no
                // result before the two instructions and none after them,
                // as with the one that pushes the constant itself.
                if let Some(start) = self.recent_instruction(0, Instruction::StoreConstant)
                    && self.recent_leaves_no_result(1)
                {
                    self.instructions[start] = Instruction::LoadConstant.as_u8();
                    return;
                }
            }
            Instruction::IsStrictlyEqual => {
                if let Some(constant) = self.recent_instruction(0, Instruction::StoreConstant) {
                    let operand = self.operand_bytes(constant);
                    if self.unpush_recent(1) {
                        self.push_instruction(Instruction::IsStrictlyEqualConstant);
                        self.instructions.extend_from_slice(&operand);
                        return;
                    }
                }
                // The same local and constant, already made one instruction.
                if let Some(fused) =
                    self.recent_instruction(0, Instruction::LoadFromIndexStoreConstant)
                {
                    let slot = self.operand_bytes(fused);
                    let constant = [self.instructions[fused + 3], self.instructions[fused + 4]];
                    self.drop_recent_instructions(fused, 1);
                    self.push_instruction(Instruction::GetValueFromIndex);
                    self.instructions.extend_from_slice(&slot);
                    self.push_instruction(Instruction::IsStrictlyEqualConstant);
                    self.instructions.extend_from_slice(&constant);
                    return;
                }
            }
            // `a[i]` read for its value, as one instruction.
            Instruction::GetValue => {
                if let Some(start) =
                    self.recent_instruction(0, Instruction::EvaluatePropertyAccessWithExpressionKey)
                {
                    // The key a local: the base stays the result, and the
                    // instruction takes the key from its slot.
                    if let Some(key) = self.recent_instruction(1, Instruction::GetValueFromIndex) {
                        let slot = self.operand_bytes(key);
                        if self.unpush_recent(2) {
                            self.push_instruction(Instruction::GetValueWithIndexKey);
                            self.instructions.extend_from_slice(&slot);
                            return;
                        }
                    }
                    self.instructions[start] = Instruction::GetValueWithExpressionKey.as_u8();
                    return;
                }
            }
            // An operator whose right operand is a constant: with its left
            // operand as the result, or taken from a local.
            operator if operator.fused_operator_number().is_some() => {
                let number = operator.fused_operator_number().unwrap_or_default();
                if let Some(constant) = self.recent_instruction(0, Instruction::StoreConstant) {
                    let operand = self.operand_bytes(constant);
                    if self.unpush_recent(1) {
                        self.push_instruction(Instruction::ApplyBinaryOperatorWithConstant);
                        self.instructions.extend_from_slice(&operand);
                        self.add_index(Instruction::fused_slot_with_operator(0, number));
                        return;
                    }
                    // Two locals pushed before the constant: the second of
                    // them is the left operand.
                    if let Some(locals) =
                        self.recent_instruction(1, Instruction::LoadFromIndexLoadFromIndex)
                    {
                        let first = self.operand_bytes(locals);
                        let second = [self.instructions[locals + 3], self.instructions[locals + 4]];
                        if usize::from(u16::from_ne_bytes(second)) < Instruction::FUSED_SLOTS {
                            self.drop_recent_instructions(locals, 2);
                            self.push_instruction(Instruction::LoadFromIndex);
                            self.instructions.extend_from_slice(&first);
                            self.push_operator_on_local(number, second, operand);
                            return;
                        }
                    }
                }
                if let Some(operands) =
                    self.recent_instruction(0, Instruction::LoadFromIndexStoreConstant)
                {
                    let slot = self.operand_bytes(operands);
                    let constant = [
                        self.instructions[operands + 3],
                        self.instructions[operands + 4],
                    ];
                    if usize::from(u16::from_ne_bytes(slot)) < Instruction::FUSED_SLOTS {
                        self.drop_recent_instructions(operands, 1);
                        self.push_operator_on_local(number, slot, constant);
                        return;
                    }
                }
            }
            Instruction::Return => {
                while let Some(pop) = self.recent_instruction(0, Instruction::PopStack) {
                    self.drop_recent_instructions(pop, 1);
                }
            }
            _ => {}
        }
        self.push_instruction(instruction);
    }

    /// The start of the `nth` most recent instruction if it is `kind`, is
    /// not a jump target, and is followed only by the more recent ones.
    fn recent_instruction(&self, nth: usize, kind: Instruction) -> Option<usize> {
        if nth >= self.recent_len {
            return None;
        }
        let start = self.recent[nth];
        let end = if nth == 0 {
            self.instructions.len()
        } else {
            self.recent[nth - 1]
        };
        if self.instructions.get(start) != Some(&kind.as_u8())
            || start + 1 + 2 * kind.argument_count() as usize != end
        {
            return None;
        }
        Some(start)
    }

    fn operand_bytes(&self, start: usize) -> [u8; 2] {
        [self.instructions[start + 1], self.instructions[start + 2]]
    }

    /// The 16-bit operand at `at`.
    fn operand(&self, at: usize) -> usize {
        usize::from(u16::from_ne_bytes([
            self.instructions[at],
            self.instructions[at + 1],
        ]))
    }

    /// Replaces the 16-bit operand at `at`, with a value that fits one.
    fn set_operand(&mut self, at: usize, value: usize) {
        let bytes = (value as u16).to_ne_bytes();
        self.instructions[at] = bytes[0];
        self.instructions[at + 1] = bytes[1];
    }

    /// Whether the `nth` most recent instruction is known to leave no
    /// result: one that moves the result away (a push of it, a store in a
    /// local), or the push of a constant after such a one, which does not
    /// touch it.
    fn recent_leaves_no_result(&self, nth: usize) -> bool {
        let mut nth = nth;
        while self
            .recent_instruction(nth, Instruction::LoadConstant)
            .is_some()
        {
            nth += 1;
        }
        if [
            Instruction::Load,
            Instruction::LoadFromIndex,
            Instruction::LoadBindingAtSite,
            Instruction::LoadBindingAtSiteOnce,
            Instruction::PushReferenceAndLoad,
            Instruction::LoadPropertyWithCachePushReference,
            Instruction::LoadIndexPropertyWithCache,
            Instruction::PutValueToIndex,
            Instruction::ApplyBinaryOperatorAndMove,
        ]
        .into_iter()
        .any(|moving| self.recent_instruction(nth, moving).is_some())
        {
            return true;
        }
        [
            (Instruction::ApplyBinaryOperatorToIndexWithConstant, 1),
            (Instruction::ApplyBinaryOperatorToIndexWithResult, 1),
            (Instruction::ApplyBinaryOperatorWithConstant, 3),
        ]
        .into_iter()
        .any(|(fused, description)| {
            self.recent_instruction(nth, fused)
                .is_some_and(|start| Instruction::fused_pushes(self.operand(start + description)))
        })
    }

    /// Where the most recent instruction, an operator fused with a constant
    /// that leaves its value as the result, keeps the description of its
    /// operation.
    fn recent_fused_operation(&self) -> Option<usize> {
        let description = if let Some(start) = self
            .recent_instruction(0, Instruction::ApplyBinaryOperatorToIndexWithConstant)
            .or_else(|| {
                self.recent_instruction(0, Instruction::ApplyBinaryOperatorToIndexWithResult)
            }) {
            start + 1
        } else {
            self.recent_instruction(0, Instruction::ApplyBinaryOperatorWithConstant)? + 3
        };
        (!Instruction::fused_pushes(self.operand(description))).then_some(description)
    }

    /// The start of the most recent instruction and the number the fused
    /// instructions have for it, if it is an arithmetic or bitwise operator
    /// on its own.
    fn recent_operator(&self) -> Option<(usize, usize)> {
        if self.recent_len == 0 {
            return None;
        }
        let start = self.recent[0];
        let kind = Instruction::try_from(*self.instructions.get(start)?).ok()?;
        let operator = kind.fused_operator_number()?;
        self.recent_instruction(0, kind)
            .map(|start| (start, operator))
    }

    /// Where the `nth` most recent instruction pushes the value that is or
    /// was the result, has it leave the value as the result instead, and
    /// removes the instructions after it: a Load is removed with them, an
    /// instruction that ends in one becomes the one that does not. False,
    /// with nothing changed, where it is no such instruction.
    fn unpush_recent(&mut self, nth: usize) -> bool {
        debug_assert!(nth >= 1);
        if let Some(load) = self.recent_instruction(nth, Instruction::Load) {
            self.drop_recent_instructions(load, nth + 1);
            return true;
        }
        for (pushing, keeping) in [
            (Instruction::LoadFromIndex, Instruction::GetValueFromIndex),
            (
                Instruction::LoadBindingAtSite,
                Instruction::GetBindingAtSite,
            ),
            (
                Instruction::LoadBindingAtSiteOnce,
                Instruction::GetBindingAtSiteOnce,
            ),
            (
                Instruction::PushReferenceAndLoad,
                Instruction::PushReference,
            ),
            (
                Instruction::LoadIndexPropertyWithCache,
                Instruction::GetIndexPropertyWithCache,
            ),
        ] {
            if let Some(start) = self.recent_instruction(nth, pushing) {
                self.instructions[start] = keeping.as_u8();
                self.drop_recent_instructions(self.recent[nth - 1], nth);
                return true;
            }
        }
        // An operator that pushes its value: with a constant, it is told
        // not to; on its own, it is the operator again.
        for (fused, description) in [
            (Instruction::ApplyBinaryOperatorToIndexWithConstant, 1),
            (Instruction::ApplyBinaryOperatorToIndexWithResult, 1),
            (Instruction::ApplyBinaryOperatorWithConstant, 3),
        ] {
            if let Some(start) = self.recent_instruction(nth, fused) {
                let operation = self.operand(start + description);
                if !Instruction::fused_pushes(operation) {
                    return false;
                }
                self.set_operand(start + description, operation & !Instruction::FUSED_PUSHES);
                self.drop_recent_instructions(self.recent[nth - 1], nth);
                return true;
            }
        }
        if let Some(start) = self.recent_instruction(nth, Instruction::ApplyBinaryOperatorAndMove) {
            let operation = self.operand(start + 1);
            let (_, operator) = Instruction::fused_slot_and_operator(operation);
            let Some(operator) = Instruction::fused_operator(operator) else {
                return false;
            };
            if !Instruction::fused_pushes(operation) {
                return false;
            }
            self.drop_recent_instructions(start, nth + 1);
            self.push_instruction(operator);
            return true;
        }
        // The function of a method call, pushed with its Reference: found
        // and kept by one instruction, its Reference pushed by another.
        if let Some(start) =
            self.recent_instruction(nth, Instruction::LoadPropertyWithCachePushReference)
        {
            self.instructions[start] = Instruction::GetPropertyWithCacheKeepReference.as_u8();
            self.drop_recent_instructions(self.recent[nth - 1], nth);
            self.push_instruction(Instruction::PushReference);
            return true;
        }
        false
    }

    /// ApplyBinaryOperatorToIndexWithConstant: the stack slot with the number
    /// of the operator above its bits, then the constant.
    fn push_operator_on_local(&mut self, operator: usize, slot: [u8; 2], constant: [u8; 2]) {
        let slot = usize::from(u16::from_ne_bytes(slot));
        self.push_instruction(Instruction::ApplyBinaryOperatorToIndexWithConstant);
        self.add_index(Instruction::fused_slot_with_operator(slot, operator));
        self.instructions.extend_from_slice(&constant);
    }

    /// An arithmetic or bitwise operator whose left operand is the local
    /// in `slot` and whose right operand was just worked out, by code that
    /// does not assign to the local: the local is not pushed before that
    /// code, the operator reads it from its slot. A right operand that is
    /// a constant or another local is what the instructions for those
    /// take; any other is the result that
    /// ApplyBinaryOperatorToIndexWithResult takes.
    pub(super) fn add_operator_on_local(&mut self, slot: usize, operator: Instruction) {
        debug_assert!(slot < Instruction::FUSED_SLOTS);
        let Some(number) = operator.fused_operator_number() else {
            unreachable!("an operator that no instruction is fused with")
        };
        let slot_bytes = (slot as IndexType).to_ne_bytes();
        if let Some(constant) = self.recent_instruction(0, Instruction::StoreConstant) {
            let constant_bytes = self.operand_bytes(constant);
            self.drop_recent_instructions(constant, 1);
            self.push_operator_on_local(number, slot_bytes, constant_bytes);
            return;
        }
        if let Some(pushed) = self.recent_instruction(0, Instruction::LoadFromIndexStoreConstant) {
            // The constant, with a local pushed before it that is no
            // operand of this operator: the push by itself again.
            let pushed_slot = self.operand_bytes(pushed);
            let constant_bytes = [self.instructions[pushed + 3], self.instructions[pushed + 4]];
            self.drop_recent_instructions(pushed, 1);
            self.push_instruction(Instruction::LoadFromIndex);
            self.instructions.extend_from_slice(&pushed_slot);
            self.push_operator_on_local(number, slot_bytes, constant_bytes);
            return;
        }
        if let Some(local) = self.recent_instruction(0, Instruction::GetValueFromIndex) {
            // Two locals: both by one instruction, the operator by its own,
            // which goes on to take what is done with its value.
            let right = self.operand_bytes(local);
            self.drop_recent_instructions(local, 1);
            self.push_instruction(Instruction::LoadFromIndexGetFromIndex);
            self.instructions.extend_from_slice(&slot_bytes);
            self.instructions.extend_from_slice(&right);
            self.add_instruction(operator);
            return;
        }
        self.push_instruction(Instruction::ApplyBinaryOperatorToIndexWithResult);
        self.add_index(Instruction::fused_slot_with_operator(slot, number));
        self.add_index(0);
    }

    /// Removes the `count` most recent instructions, which start at `start`.
    fn drop_recent_instructions(&mut self, start: usize, count: usize) {
        debug_assert!(count <= self.recent_len && self.recent[count - 1] == start);
        self.instructions.truncate(start);
        self.kept_slots_read.retain(|&(_, at)| at < start);
        if self.kept_read_again.is_some_and(|at| at >= start) {
            self.kept_read_again = None;
        }
        self.recent.copy_within(count.., 0);
        self.recent_len -= count;
    }

    fn forget_recent_instructions(&mut self) {
        self.recent_len = 0;
        // Code that is jumped to may be reached with no kept name read.
        self.kept_slots_read.clear();
        self.kept_read_again = None;
    }

    /// Makes the function of a method call, which was just read by its
    /// cache with its Reference kept and loaded for the call, be pushed
    /// after the value it is read from instead: that value is the this
    /// value of the call, and the call finds it under the function
    /// ([`Instruction::EvaluateCallOnThis`]) where it took the base of a
    /// Reference that was made, pushed before the arguments and popped
    /// after them for nothing else. Returns whether it did; nothing is
    /// changed where it did not.
    ///
    /// `o.m(x)`, `p.m(x)` of a local and `this.m(x)`, with arguments or
    /// without: a property by a name that is known when the code is
    /// compiled, of a call that is not optional.
    pub(super) fn keep_this_under_function(&mut self) -> bool {
        // With arguments, of a value in the result register: one
        // instruction found the function, kept and pushed its Reference
        // and loaded it.
        if let Some(start) =
            self.recent_instruction(0, Instruction::LoadPropertyWithCachePushReference)
        {
            self.instructions[start] = Instruction::LoadPropertyWithCachePushThis.as_u8();
            return true;
        }
        // Otherwise the load is an instruction of its own, with the push
        // of the Reference where there are arguments.
        let Some(load) = self
            .recent_instruction(0, Instruction::PushReferenceAndLoad)
            .or_else(|| self.recent_instruction(0, Instruction::Load))
        else {
            return false;
        };
        if let Some(property) =
            self.recent_instruction(1, Instruction::GetIndexPropertyWithCacheKeepReference)
        {
            self.instructions[property] = Instruction::LoadIndexPropertyWithCachePushThis.as_u8();
            self.drop_recent_instructions(load, 1);
            return true;
        }
        if let Some(property) =
            self.recent_instruction(1, Instruction::GetPropertyWithCacheKeepReference)
        {
            self.instructions[property] = Instruction::LoadPropertyWithCachePushThis.as_u8();
            self.drop_recent_instructions(load, 1);
            return true;
        }
        if let Some(property) =
            self.recent_instruction(1, Instruction::GetThisPropertyWithCacheKeepReference)
        {
            // `this.m(x)`: the this value by its own instruction again,
            // then the function pushed after it.
            let identifier = self.operand_bytes(property);
            let cache = [
                self.instructions[property + 3],
                self.instructions[property + 4],
            ];
            self.drop_recent_instructions(property, 2);
            self.push_instruction(Instruction::ResolveThisBinding);
            self.push_instruction(Instruction::LoadPropertyWithCachePushThis);
            self.instructions.extend_from_slice(&identifier);
            self.instructions.extend_from_slice(&cache);
            return true;
        }
        false
    }

    pub(super) fn add_instruction_with_jump_slot(&mut self, instruction: Instruction) -> JumpIndex {
        debug_assert_eq!(instruction.argument_count(), 2);
        debug_assert!(instruction.has_jump_slot());
        let instruction = match instruction {
            Instruction::JumpIfNot | Instruction::JumpIfTrue => {
                if let Some(not) = self.recent_instruction(0, Instruction::LogicalNot) {
                    self.drop_recent_instructions(not, 1);
                    if instruction == Instruction::JumpIfNot {
                        Instruction::JumpIfTrue
                    } else {
                        Instruction::JumpIfNot
                    }
                } else {
                    instruction
                }
            }
            _ => instruction,
        };
        // An equality and the jump on it, as one instruction.
        let instruction = match instruction {
            Instruction::JumpIfNot | Instruction::JumpIfTrue => {
                let (strict, loose) = if instruction == Instruction::JumpIfNot {
                    (
                        Instruction::JumpIfNotStrictlyEqual,
                        Instruction::JumpIfNotLooselyEqual,
                    )
                } else {
                    (
                        Instruction::JumpIfStrictlyEqual,
                        Instruction::JumpIfLooselyEqual,
                    )
                };
                if let Some(equal) = self.recent_instruction(0, Instruction::IsStrictlyEqual) {
                    self.drop_recent_instructions(equal, 1);
                    strict
                } else if let Some(equal) = self.recent_instruction(0, Instruction::IsLooselyEqual)
                {
                    // `typeof v == "number"`: two Strings are loosely equal
                    // where they are strictly equal, so this is the test
                    // of the value's type too.
                    if let Some(comparison) = strict.result_comparison()
                        && let Some(constant) =
                            self.recent_instruction(1, Instruction::StoreConstant)
                        && let Some(type_named) =
                            self.type_named_by_constant(self.operand_bytes(constant))
                        && self.recent_instruction(2, Instruction::Load).is_some()
                        && let Some(type_of) = self.recent_instruction(3, Instruction::Typeof)
                    {
                        self.drop_recent_instructions(type_of, 4);
                        return self.add_type_comparison(comparison, type_named);
                    }
                    self.drop_recent_instructions(equal, 1);
                    loose
                } else if let Some(equal) =
                    self.recent_instruction(0, Instruction::IsStrictlyEqualConstant)
                    && let Some(local) = self.recent_instruction(1, Instruction::GetValueFromIndex)
                {
                    // A local and a constant: both operands by one
                    // instruction, the test by another.
                    let slot = self.operand_bytes(local);
                    let constant = self.operand_bytes(equal);
                    self.drop_recent_instructions(local, 2);
                    self.push_instruction(Instruction::LoadFromIndexStoreConstant);
                    self.instructions.extend_from_slice(&slot);
                    self.instructions.extend_from_slice(&constant);
                    strict
                } else if let Some(equal) =
                    self.recent_instruction(0, Instruction::IsStrictlyEqualConstant)
                    && let Some(comparison) = strict.result_comparison()
                {
                    // What was just computed and a constant: the test and
                    // the jump on it by one instruction.
                    let constant = self.operand_bytes(equal);
                    self.drop_recent_instructions(equal, 1);
                    // `typeof v === "number"`: the Typeof too, where the
                    // constant names a type; the test is then of the
                    // value's type.
                    if let Some(type_named) = self.type_named_by_constant(constant)
                        && let Some(type_of) = self.recent_instruction(0, Instruction::Typeof)
                    {
                        self.drop_recent_instructions(type_of, 1);
                        return self.add_type_comparison(comparison, type_named);
                    }
                    return self.add_result_comparison(comparison, constant);
                } else {
                    instruction
                }
            }
            _ => instruction,
        };
        // The test of a loop or an `if`, a comparison and the jump on it,
        // as one instruction.
        let instruction = if instruction == Instruction::JumpIfNot {
            let fused = [
                (Instruction::LessThan, Instruction::JumpIfNotLessThan),
                (
                    Instruction::LessThanEquals,
                    Instruction::JumpIfNotLessThanEquals,
                ),
                (Instruction::GreaterThan, Instruction::JumpIfNotGreaterThan),
                (
                    Instruction::GreaterThanEquals,
                    Instruction::JumpIfNotGreaterThanEquals,
                ),
            ]
            .into_iter()
            .find_map(|(comparison, fused)| {
                self.recent_instruction(0, comparison)
                    .map(|start| (start, fused))
            });
            match fused {
                Some((start, fused)) => {
                    self.drop_recent_instructions(start, 1);
                    fused
                }
                None => instruction,
            }
        } else {
            instruction
        };
        // The test at the end of a loop: a local compared with a constant or
        // another local, the jump back where that holds, and the `i++` or
        // `i--` just before them if it is of the same local.
        if instruction == Instruction::JumpIfTrue {
            let test = [
                Instruction::LessThan,
                Instruction::LessThanEquals,
                Instruction::GreaterThan,
                Instruction::GreaterThanEquals,
            ]
            .into_iter()
            .find_map(|comparison| {
                self.recent_instruction(0, comparison)?;
                [
                    (Instruction::LoadFromIndexStoreConstant, true),
                    (Instruction::LoadFromIndexGetFromIndex, false),
                ]
                .into_iter()
                .find_map(|(operands, constant)| {
                    let start = self.recent_instruction(1, operands)?;
                    Some((comparison, constant, start))
                })
            });
            if let Some((comparison, constant, start)) = test {
                let first = self.operand_bytes(start);
                let second = [self.instructions[start + 3], self.instructions[start + 4]];
                self.drop_recent_instructions(start, 2);
                let step = [
                    (Instruction::IncrementIndex, 1),
                    (Instruction::DecrementIndex, -1),
                ]
                .into_iter()
                .find_map(|(update, step)| {
                    let start = self.recent_instruction(0, update)?;
                    let fused = comparison.as_loop_test(step, constant)?;
                    (self.operand_bytes(start) == first).then_some((start, fused))
                });
                let fused = match step {
                    Some((update, fused)) => {
                        self.drop_recent_instructions(update, 1);
                        Some(fused)
                    }
                    None => comparison.as_loop_test(0, constant),
                };
                if let Some(fused) = fused {
                    self.push_instruction(fused);
                    self.instructions.extend_from_slice(&first);
                    self.instructions.extend_from_slice(&second);
                    return self.add_jump_index();
                }
            }
            // What was just computed compared with a constant: the
            // Load, the StoreConstant, the comparison and the jump as one
            // instruction.
            let of_result = [
                Instruction::LessThan,
                Instruction::LessThanEquals,
                Instruction::GreaterThan,
                Instruction::GreaterThanEquals,
            ]
            .into_iter()
            .find_map(|comparison| {
                self.recent_instruction(0, comparison)?;
                let constant = self.recent_instruction(1, Instruction::StoreConstant)?;
                Some((comparison.result_comparison()?, constant))
            });
            if let Some((comparison, constant)) = of_result {
                // Whatever pushed the value leaves it as the result: a
                // Load is dropped, an instruction that ends in one is the
                // one that does not.
                let constant = self.operand_bytes(constant);
                if self.unpush_recent(2) {
                    return self.add_result_comparison(comparison, constant);
                }
            }
            // Any other operands: the comparison and the jump on it.
            let fused = [
                (Instruction::LessThan, Instruction::JumpIfLessThan),
                (
                    Instruction::LessThanEquals,
                    Instruction::JumpIfLessThanEquals,
                ),
                (Instruction::GreaterThan, Instruction::JumpIfGreaterThan),
                (
                    Instruction::GreaterThanEquals,
                    Instruction::JumpIfGreaterThanEquals,
                ),
            ]
            .into_iter()
            .find_map(|(comparison, fused)| {
                self.recent_instruction(0, comparison)
                    .map(|start| (start, fused))
            });
            if let Some((start, fused)) = fused {
                self.drop_recent_instructions(start, 1);
                self.push_instruction(fused);
                return self.add_jump_index();
            }
        }
        // A local compared with a constant or with another local, and the
        // jump: both operands by the instruction that jumps.
        let operands = [
            (Instruction::LoadFromIndexStoreConstant, true),
            (Instruction::LoadFromIndexGetFromIndex, false),
        ]
        .into_iter()
        .find_map(|(operands, constant)| {
            let fused = instruction.with_compared_local(constant)?;
            let start = self.recent_instruction(0, operands)?;
            Some((start, fused))
        });
        if let Some((start, fused)) = operands {
            let first = self.operand_bytes(start);
            let second = [self.instructions[start + 3], self.instructions[start + 4]];
            self.drop_recent_instructions(start, 1);
            self.push_instruction(fused);
            self.instructions.extend_from_slice(&first);
            self.instructions.extend_from_slice(&second);
            return self.add_jump_index();
        }
        // What was just computed compared with a constant, and the jump
        // where the comparison does not hold: as above.
        if let Some(comparison) = instruction.result_comparison()
            && let Some(constant) = self.recent_instruction(0, Instruction::StoreConstant)
        {
            let constant = self.operand_bytes(constant);
            if self.unpush_recent(1) {
                return self.add_result_comparison(comparison, constant);
            }
        }
        self.push_instruction(instruction);
        self.add_jump_index()
    }

    /// JumpIfResultComparesWithConstant for `typeof` of what was just
    /// computed and the name of a type: `comparison` is the strict equality
    /// and jump it stands for ([`Instruction::result_comparison`]),
    /// `type_named` the place of the name in [`Instruction::TYPE_NAMES`].
    fn add_type_comparison(&mut self, comparison: IndexType, type_named: IndexType) -> JumpIndex {
        debug_assert!(usize::from(comparison) >= Instruction::RESULT_COMPARISON_STRICT);
        let comparison = comparison - Instruction::RESULT_COMPARISON_STRICT as IndexType
            + Instruction::RESULT_COMPARISON_TYPE as IndexType;
        self.add_result_comparison(comparison, type_named.to_ne_bytes())
    }

    /// The place in [`Instruction::TYPE_NAMES`] of the type that the
    /// constant of a StoreConstant names, if it is a String that names
    /// one: `operand` is the instruction's argument.
    fn type_named_by_constant(&self, operand: [u8; 2]) -> Option<IndexType> {
        let constant = *self
            .constants
            .get(usize::from(u16::from_ne_bytes(operand)))?;
        let name = String::try_from(constant).ok()?;
        let name = name.to_string_lossy_(self.agent);
        let place = Instruction::TYPE_NAMES
            .iter()
            .position(|type_name| *type_name == name)?;
        IndexType::try_from(place).ok()
    }

    /// JumpIfResultComparesWithConstant: which comparison and jump it makes
    /// ([`Instruction::result_comparison`]), the constant as another
    /// instruction had it, and the jump target to be set.
    fn add_result_comparison(&mut self, comparison: IndexType, constant: [u8; 2]) -> JumpIndex {
        self.push_instruction(Instruction::JumpIfResultComparesWithConstant);
        self.instructions
            .extend_from_slice(&comparison.to_ne_bytes());
        self.instructions.extend_from_slice(&constant);
        self.add_jump_index()
    }

    pub(super) fn add_jump_instruction_to_index(
        &mut self,
        instruction: Instruction,
        jump_index: JumpIndex,
    ) {
        debug_assert_eq!(instruction.argument_count(), 2);
        debug_assert!(instruction.has_jump_slot());
        self.push_instruction(instruction);
        self.add_double_index(jump_index.index);
    }

    pub(super) fn get_jump_index_to_here(&mut self) -> JumpIndex {
        self.current_instruction_pointer_is_unreachable = false;
        self.forget_recent_instructions();
        JumpIndex {
            index: self.instructions.len(),
        }
    }

    /// A switch table for the `selectors` of a `switch` statement's cases,
    /// in source order, and the JumpToCase instruction that tests the value
    /// in the stack slot `slot` by it. Returns the table: each case sets
    /// where it starts with [`Self::set_switch_target_here`]. None, and no
    /// instruction, if the executable has no room for the table.
    pub(super) fn add_switch_table(
        &mut self,
        slot: usize,
        selectors: &[Value<'gc>],
    ) -> Option<usize> {
        let table = self.switch_tables.len();
        if IndexType::try_from(table).is_err()
            || IndexType::try_from(slot).is_err()
            || self.constants.len() + selectors.len() >= WIDE_CONSTANTS_FROM
        {
            return None;
        }
        let cases = selectors
            .iter()
            .map(|selector| (self.add_constant(*selector) as IndexType, 0))
            .collect();
        self.switch_tables.push(cases);
        self.add_instruction_with_immediate_and_immediate(Instruction::JumpToCase, slot, table);
        Some(table)
    }

    /// The case `case` of the switch table `table` starts here.
    pub(super) fn set_switch_target_here(&mut self, table: usize, case: usize) {
        // A jump target: nothing before it is fused with what follows, and
        // a Jump just before it stays.
        self.forget_recent_instructions();
        self.current_instruction_pointer_is_unreachable = false;
        let Ok(target) = u32::try_from(self.instructions.len()) else {
            self.too_many.get_or_insert(TOO_MANY_VALUES);
            return;
        };
        self.switch_tables[table][case].1 = target;
    }

    pub(super) fn add_cache(&mut self, cache: PropertyLookupCache<'gc>) -> usize {
        let next_index = self.caches.len();
        let index = *self.cache_indexes.entry(cache).or_insert(next_index);
        if index == next_index {
            self.caches.push(cache);
        }
        index
    }

    pub(super) fn add_constant(&mut self, constant: Value<'gc>) -> usize {
        let next_index = self.constants.len();
        let index = *self
            .constant_indexes
            .entry(ConstantKey(constant))
            .or_insert(next_index);
        if index == next_index {
            self.constants.push(constant);
        }
        index
    }

    /// Whether a member expression names the property `identifier` by a
    /// 16-bit operand, rather than by a wide constant key.
    pub(super) fn property_name_fits(&self, identifier: PropertyKey<'gc>) -> bool {
        self.constants.len() < WIDE_PROPERTY_NAMES_FROM
            || self
                .constant_indexes
                .get(&ConstantKey(identifier.into_value_unchecked()))
                .is_some_and(|&index| IndexType::try_from(index).is_ok())
    }

    pub(super) fn add_identifier(&mut self, identifier: PropertyKey<'gc>) -> usize {
        self.add_constant(identifier.into_value_unchecked())
    }

    pub(super) fn add_shape(&mut self, shape: ObjectShape<'gc>) -> usize {
        let duplicate = self
            .shapes
            .iter()
            .enumerate()
            .find(|item| item.1.eq(&shape))
            .map(|(idx, _)| idx);

        duplicate.unwrap_or_else(|| {
            let index = self.shapes.len();
            self.shapes.push(shape);
            index
        })
    }

    pub(super) fn add_instruction_with_immediate(
        &mut self,
        instruction: Instruction,
        immediate: usize,
    ) {
        debug_assert_eq!(instruction.argument_count(), 1);
        if instruction == Instruction::EvaluateCall
            && let Some(pop) = self.recent_instruction(0, Instruction::PopReference)
        {
            self.drop_recent_instructions(pop, 1);
            self.push_instruction(Instruction::PopReferenceAndEvaluateCall);
            self.add_index(immediate);
            return;
        }
        // An operator whose value goes to a local, as one instruction.
        if instruction == Instruction::PutValueToIndex
            && immediate < Instruction::FUSED_SLOTS
            && let Some((start, operator)) = self.recent_operator()
        {
            self.drop_recent_instructions(start, 1);
            self.push_instruction(Instruction::ApplyBinaryOperatorAndMove);
            self.add_index(Instruction::fused_slot_with_operator(immediate, operator));
            return;
        }
        // Two locals in a row, as one instruction.
        if instruction == Instruction::GetValueFromIndex
            && let Some(load) = self.recent_instruction(0, Instruction::LoadFromIndex)
        {
            let slot = self.operand_bytes(load);
            self.drop_recent_instructions(load, 1);
            self.push_instruction(Instruction::LoadFromIndexGetFromIndex);
            self.instructions.extend_from_slice(&slot);
            self.add_index(immediate);
            return;
        }
        self.push_instruction(instruction);
        self.add_index(immediate);
    }

    pub(super) fn add_instruction_with_constant(
        &mut self,
        instruction: Instruction,
        constant: impl Into<Value<'gc>>,
    ) {
        debug_assert_eq!(instruction.argument_count(), 1);
        debug_assert!(instruction.has_constant_index());
        let constant = constant.into();
        let wide_instruction = match instruction {
            Instruction::LoadConstant => Some(Instruction::LoadConstantWide),
            Instruction::StoreConstant => Some(Instruction::StoreConstantWide),
            _ => None,
        };
        if let Some(wide_instruction) = wide_instruction
            && self.constants.len() >= WIDE_CONSTANTS_FROM
            && !self.constant_indexes.contains_key(&ConstantKey(constant))
        {
            // A script with this many distinct constants (a large data
            // table) keeps the 16-bit table's last indices for identifiers
            // and property keys: its further literals go to the wide table.
            let next_index = self.wide_constants.len();
            let index = *self
                .wide_constant_indexes
                .entry(ConstantKey(constant))
                .or_insert(next_index);
            if index == next_index {
                self.wide_constants.push(constant);
            }
            self.push_instruction(wide_instruction);
            self.add_double_index(index);
            return;
        }
        // A local and then a constant, as one instruction.
        if instruction == Instruction::StoreConstant
            && let Some(load) = self.recent_instruction(0, Instruction::LoadFromIndex)
        {
            let slot = self.operand_bytes(load);
            self.drop_recent_instructions(load, 1);
            self.push_instruction(Instruction::LoadFromIndexStoreConstant);
            self.instructions.extend_from_slice(&slot);
            let constant = self.add_constant(constant);
            self.add_index_of(constant, TOO_MANY_NAMES);
            return;
        }
        self.push_instruction(instruction);
        let constant = self.add_constant(constant);
        self.add_index_of(constant, TOO_MANY_NAMES);
    }

    pub(super) fn add_instruction_with_identifier(
        &mut self,
        instruction: Instruction,
        identifier: PropertyKey<'gc>,
    ) {
        debug_assert_eq!(instruction.argument_count(), 1);
        debug_assert!(instruction.has_identifier_index());
        self.push_instruction(instruction);
        let identifier = self.add_identifier(identifier);
        self.add_index_of(identifier, TOO_MANY_NAMES);
    }

    pub(super) fn add_instruction_with_cache(
        &mut self,
        instruction: Instruction,
        cache: PropertyLookupCache<'gc>,
    ) {
        debug_assert_eq!(instruction.argument_count(), 1);
        debug_assert!(instruction.has_cache_index());
        let fused = match instruction {
            Instruction::GetValueWithCache => Some(Instruction::GetPropertyWithCache),
            Instruction::GetValueWithCacheKeepReference => {
                Some(Instruction::GetPropertyWithCacheKeepReference)
            }
            _ => None,
        };
        // The stack slot of a local whose property is read, for the
        // instruction that reads both: it goes after the cache. And the
        // BindingSite after that, where the slot is the one that keeps a
        // name of an enclosing function.
        let mut slot = None;
        let mut site = [0, 0];
        if let Some(fused) = fused
            && let Some(access) =
                self.recent_instruction(0, Instruction::EvaluatePropertyAccessWithIdentifierKey)
        {
            let identifier = self.operand_bytes(access);
            let of_local = if fused == Instruction::GetPropertyWithCache {
                Instruction::GetIndexPropertyWithCache
            } else {
                Instruction::GetIndexPropertyWithCacheKeepReference
            };
            if let Some(this) = self.recent_instruction(1, Instruction::ResolveThisBinding) {
                // `this.x`: the this value is fetched by the same
                // instruction.
                self.drop_recent_instructions(this, 2);
                self.push_instruction(if fused == Instruction::GetPropertyWithCache {
                    Instruction::GetThisPropertyWithCache
                } else {
                    Instruction::GetThisPropertyWithCacheKeepReference
                });
            } else if let Some(local) = self.recent_instruction(1, Instruction::GetValueFromIndex) {
                // `p.x`: the local is read by the same instruction.
                slot = Some(self.operand_bytes(local));
                self.drop_recent_instructions(local, 2);
                self.push_instruction(of_local);
            } else if fused == Instruction::GetPropertyWithCache
                && let Some(kept) = self.recent_instruction(1, Instruction::GetBindingAtSiteOnce)
                && self.kept_read_again == Some(kept)
            {
                // `o.x` of a name of an enclosing function that a stack
                // slot keeps for the call once it was read, where the code
                // has read the name before with nothing jumping in since:
                // the slot is filled then (or the name has no value to
                // keep), and one instruction reads the property from it,
                // with the slot and the site where GetBindingAtSiteOnce
                // had them. The first read of such a run stays the two
                // instructions it was, of which the first fills the slot
                // on the short way; an instruction that had to fill it
                // would do so on the general way, at several times the
                // price of both.
                site = [self.instructions[kept + 3], self.instructions[kept + 4]];
                slot = Some([self.instructions[kept + 5], self.instructions[kept + 6]]);
                self.drop_recent_instructions(kept, 2);
                self.push_instruction(Instruction::GetKeptPropertyWithCache);
            } else if let Some(locals) =
                self.recent_instruction(1, Instruction::LoadFromIndexGetFromIndex)
            {
                // The same after another local was pushed, which stays a
                // push.
                let pushed = self.operand_bytes(locals);
                slot = Some([self.instructions[locals + 3], self.instructions[locals + 4]]);
                self.drop_recent_instructions(locals, 2);
                self.push_instruction(Instruction::LoadFromIndex);
                self.instructions.extend_from_slice(&pushed);
                self.push_instruction(of_local);
            } else {
                self.drop_recent_instructions(access, 1);
                self.push_instruction(fused);
            }
            self.instructions.extend_from_slice(&identifier);
        } else {
            self.push_instruction(instruction);
        }
        let cache = self.add_cache(cache);
        self.add_index_of(cache, TOO_MANY_PROPERTY_NAMES);
        if let Some(slot) = slot {
            self.instructions.extend_from_slice(&slot);
            self.instructions.extend_from_slice(&site);
        }
    }

    /// PutIndexPropertyWithCache: the name, the cache and the stack slot of
    /// the object, laid out as GetIndexPropertyWithCache has its own.
    pub(super) fn add_instruction_with_identifier_cache_and_slot(
        &mut self,
        instruction: Instruction,
        identifier: PropertyKey<'gc>,
        cache: PropertyLookupCache<'gc>,
        slot: usize,
    ) {
        debug_assert_eq!(instruction.argument_count(), 4);
        debug_assert!(instruction.has_identifier_index() && instruction.has_cache_index());
        self.push_instruction(instruction);
        let identifier = self.add_identifier(identifier);
        self.add_index_of(identifier, TOO_MANY_NAMES);
        let cache = self.add_cache(cache);
        self.add_index_of(cache, TOO_MANY_PROPERTY_NAMES);
        self.add_index(slot);
        self.add_index(0);
    }

    /// StepValueWithCache: the cache of the assignment, and which step it
    /// is and what it leaves.
    pub(super) fn add_instruction_with_cache_and_immediate(
        &mut self,
        instruction: Instruction,
        cache: PropertyLookupCache<'gc>,
        immediate: usize,
    ) {
        debug_assert_eq!(instruction.argument_count(), 2);
        debug_assert!(instruction.has_cache_index());
        self.push_instruction(instruction);
        let cache = self.add_cache(cache);
        self.add_index_of(cache, TOO_MANY_PROPERTY_NAMES);
        self.add_index(immediate);
    }

    pub(super) fn add_instruction_with_identifier_and_cache(
        &mut self,
        instruction: Instruction,
        identifier: PropertyKey<'gc>,
        cache: PropertyLookupCache<'gc>,
    ) {
        debug_assert_eq!(instruction.argument_count(), 2);
        debug_assert!(instruction.has_identifier_index() && instruction.has_cache_index());
        self.push_instruction(instruction);
        let identifier = self.add_identifier(identifier);
        self.add_index_of(identifier, TOO_MANY_NAMES);
        let cache = self.add_cache(cache);
        self.add_index_of(cache, TOO_MANY_PROPERTY_NAMES);
    }

    /// A new BindingSite of the executable, while an instruction can still
    /// number one.
    pub(super) fn new_binding_site(&mut self) -> Option<IndexType> {
        let site = IndexType::try_from(self.binding_site_names.len()).ok()?;
        self.binding_site_names.push(IndexType::MAX);
        Some(site)
    }

    /// Records that `site` is the place of the name at `identifier` among
    /// the constants: the tables of the executable keep the name beside
    /// the site, for the instructions that read a variable there.
    fn name_binding_site(&mut self, site: IndexType, identifier: usize) {
        if let Some(name) = self.binding_site_names.get_mut(usize::from(site)) {
            *name = IndexType::try_from(identifier).unwrap_or(IndexType::MAX);
        }
    }

    /// One of the instructions that take a name and the BindingSite that
    /// remembers where it was found.
    pub(super) fn add_instruction_with_identifier_and_site(
        &mut self,
        instruction: Instruction,
        identifier: PropertyKey<'gc>,
        site: IndexType,
    ) {
        debug_assert!(matches!(
            instruction,
            Instruction::ResolveBindingAtSite
                | Instruction::GetBindingAtSite
                | Instruction::PutBindingAtSite
                | Instruction::ResolveGlobalAtSite
                | Instruction::CheckGlobalAtSite
                | Instruction::GetGlobalAtSite
                | Instruction::PutGlobalAtSite
                | Instruction::PutValueAtSite
        ));
        // A variable of an enclosing function that was just assigned to:
        // the assignment leaves what it assigned as the result, which is
        // what this read would find there. The two are next to each other
        // with nothing that jumps in between, so they name one variable;
        // the site of the read is left with no name and is not used.
        if instruction == Instruction::GetBindingAtSite
            && let Some(put) = self.recent_instruction(0, Instruction::PutBindingAtSite)
            && self.operand(put + 1) == self.add_identifier(identifier)
        {
            self.instructions[put] = Instruction::PutBindingAtSiteAndGet.as_u8();
            return;
        }
        self.push_instruction(instruction);
        let identifier = self.add_identifier(identifier);
        self.name_binding_site(site, identifier);
        self.add_index_of(identifier, TOO_MANY_NAMES);
        self.instructions.extend_from_slice(&site.to_ne_bytes());
    }

    /// GetBindingAtSiteOnce: the name, its BindingSite, and the stack slot
    /// that keeps its value once it was read.
    pub(super) fn add_instruction_with_identifier_site_and_slot(
        &mut self,
        instruction: Instruction,
        identifier: PropertyKey<'gc>,
        site: IndexType,
        slot: IndexType,
    ) {
        debug_assert_eq!(instruction, Instruction::GetBindingAtSiteOnce);
        let start = self.instructions.len();
        self.kept_read_again = if self.kept_slots_read.iter().any(|&(read, _)| read == slot) {
            Some(start)
        } else {
            self.kept_slots_read.push((slot, start));
            None
        };
        self.push_instruction(instruction);
        let identifier = self.add_identifier(identifier);
        self.name_binding_site(site, identifier);
        self.add_index_of(identifier, TOO_MANY_NAMES);
        self.instructions.extend_from_slice(&site.to_ne_bytes());
        self.instructions.extend_from_slice(&slot.to_ne_bytes());
        // The fourth argument, of which nothing is made.
        self.instructions.extend_from_slice(&[0, 0]);
    }

    pub(super) fn add_instruction_with_identifier_and_constant(
        &mut self,
        instruction: Instruction,
        identifier: PropertyKey<'gc>,
        constant: impl Into<Value<'gc>>,
    ) {
        debug_assert_eq!(instruction.argument_count(), 2);
        debug_assert!(instruction.has_identifier_index() && instruction.has_constant_index());
        self.push_instruction(instruction);
        let identifier = self.add_identifier(identifier);
        self.add_index_of(identifier, TOO_MANY_NAMES);
        let constant = self.add_constant(constant.into());
        self.add_index_of(constant, TOO_MANY_NAMES);
    }

    pub(super) fn add_instruction_with_identifier_and_immediate(
        &mut self,
        instruction: Instruction,
        identifier: PropertyKey<'gc>,
        immediate: usize,
    ) {
        debug_assert_eq!(instruction.argument_count(), 2);
        debug_assert!(instruction.has_identifier_index());
        self.push_instruction(instruction);
        let identifier = self.add_identifier(identifier);
        self.add_index_of(identifier, TOO_MANY_NAMES);
        self.add_index(immediate);
    }

    pub(super) fn add_instruction_with_immediate_and_immediate(
        &mut self,
        instruction: Instruction,
        immediate1: usize,
        immediate2: usize,
    ) {
        debug_assert_eq!(instruction.argument_count(), 2);
        self.push_instruction(instruction);
        self.add_index(immediate1);
        self.add_index(immediate2)
    }

    pub(super) fn add_instruction_with_immediate_and_constant(
        &mut self,
        instruction: Instruction,
        immediate: usize,
        constant: Value<'gc>,
    ) {
        debug_assert_eq!(instruction.argument_count(), 2);
        debug_assert!(instruction.has_constant_index());
        self.push_instruction(instruction);
        self.add_index(immediate);
        let constant = self.add_constant(constant);
        self.add_index_of(constant, TOO_MANY_NAMES)
    }

    pub(super) fn add_instruction_with_function_expression(
        &mut self,
        instruction: Instruction,
        function_expression: FunctionExpression<'gc>,
    ) {
        debug_assert_eq!(instruction.argument_count(), 1);
        debug_assert!(instruction.has_function_expression_index());
        self.push_instruction(instruction);
        self.function_expressions.push(function_expression);
        let index = self.function_expressions.len() - 1;
        self.add_index_of(index, TOO_MANY_FUNCTIONS);
    }

    /// Add an Instruction that takes a function expression and an immediate
    /// as its bytecode parameters.
    ///
    /// Returns the function expression's index.
    pub(super) fn add_instruction_with_function_expression_and_immediate(
        &mut self,
        instruction: Instruction,
        function_expression: FunctionExpression<'gc>,
        immediate: usize,
    ) -> usize {
        debug_assert_eq!(instruction.argument_count(), 2);
        debug_assert!(instruction.has_function_expression_index());
        self.push_instruction(instruction);
        let index = self.function_expressions.len();
        self.function_expressions.push(function_expression);
        self.add_index_of(index, TOO_MANY_FUNCTIONS);
        self.add_index(immediate);
        index
    }

    pub(super) fn add_instruction_with_shape(
        &mut self,
        instruction: Instruction,
        shape: ObjectShape<'gc>,
    ) {
        debug_assert_eq!(instruction.argument_count(), 1);
        debug_assert!(instruction.has_shape_index());
        self.push_instruction(instruction);
        let shape = self.add_shape(shape);
        self.add_index_of(shape, TOO_MANY_OBJECT_LITERALS);
    }

    pub(super) fn add_arrow_function_expression(
        &mut self,
        arrow_function_expression: ArrowFunctionExpression<'gc>,
    ) {
        let instruction = Instruction::InstantiateArrowFunctionExpression;
        debug_assert_eq!(instruction.argument_count(), 1);
        debug_assert!(instruction.has_function_expression_index());
        self.push_instruction(instruction);
        self.arrow_function_expressions
            .push(arrow_function_expression);
        let index = self.arrow_function_expressions.len() - 1;
        self.add_index_of(index, TOO_MANY_FUNCTIONS);
    }

    pub(super) fn set_jump_target(&mut self, source: JumpIndex, target: JumpIndex) {
        assert!(target.index < u32::MAX as usize);
        let bytes: [u8; 4] = (target.index as u32).to_ne_bytes();
        self.instructions[source.index..source.index + 4].copy_from_slice(&bytes);
    }

    pub(super) fn set_jump_target_here(&mut self, jump: JumpIndex) {
        self.forget_recent_instructions();
        if self.current_instruction_pointer_is_unreachable
            && jump.index == self.instructions.len().saturating_sub(4)
        {
            // OPTIMISATION: An unconditional jump to next instruction
            // can be popped from the bytecode stream.
            self.instructions
                .truncate(self.instructions.len().saturating_sub(5));
            // After popping the Jump off, we're no longer on unreachable
            // ground.
            self.current_instruction_pointer_is_unreachable = false;
            return;
        }
        self.set_jump_target(
            jump,
            JumpIndex {
                index: self.instructions.len(),
            },
        );
        self.current_instruction_pointer_is_unreachable = false;
    }

    /// Adds an instruction whose immediate is the index of the next class
    /// initializer: ClassDefineDefaultConstructor.
    pub(super) fn add_instruction_with_next_class_initializer_index(
        &mut self,
        instruction: Instruction,
    ) {
        debug_assert_eq!(instruction.argument_count(), 1);
        self.push_instruction(instruction);
        let index = self.class_initializer_bytecodes.len();
        self.add_index_of(index, TOO_MANY_CLASSES);
    }

    pub(super) fn set_function_expression_bytecode(
        &mut self,
        index: usize,
        executable: Executable<'gc>,
    ) {
        self.function_expressions[index].template.executable = Some(executable);
    }

    pub(super) fn add_class_initializer_bytecode(
        &mut self,
        executable: Executable<'gc>,
        has_constructor_parent: bool,
    ) {
        self.class_initializer_bytecodes
            .push((Some(executable), has_constructor_parent));
    }

    pub(super) fn set_call_environment(&mut self, call_environment: CallEnvironment) {
        self.call_environment = call_environment;
    }

    pub(super) fn set_stack_parameters(&mut self, count: u16) {
        self.stack_parameters = Some(count);
    }

    pub(super) fn set_instance_initializer(&mut self, executable: Executable<'gc>) {
        self.instance_initializer = Some(executable);
    }

    pub(super) fn add_class_initializer(&mut self, has_constructor_parent: bool) {
        self.class_initializer_bytecodes
            .push((None, has_constructor_parent));
    }

    fn push_instruction(&mut self, instruction: Instruction) {
        self.recent.copy_within(..RECENT_INSTRUCTIONS - 1, 1);
        self.recent[0] = self.instructions.len();
        self.recent_len = (self.recent_len + 1).min(RECENT_INSTRUCTIONS);
        self.instructions.push(instruction.as_u8());
        self.current_instruction_pointer_is_unreachable = instruction.is_terminal();
    }

    fn add_index(&mut self, index: usize) {
        self.add_index_of(index, TOO_MANY_VALUES);
    }

    /// Adds a 16-bit operand. An index that does not fit marks the
    /// executable as too large, with `too_many` as its SyntaxError.
    fn add_index_of(&mut self, index: usize, too_many: &'static str) {
        let index = IndexType::try_from(index).unwrap_or_else(|_| {
            self.too_many.get_or_insert(too_many);
            0
        });
        let bytes: [u8; 2] = index.to_ne_bytes();
        self.instructions.extend_from_slice(&bytes);
    }

    fn add_double_index(&mut self, index: usize) {
        let index = u32::try_from(index).unwrap_or_else(|_| {
            self.too_many.get_or_insert(TOO_MANY_VALUES);
            0
        });
        let bytes: [u8; 4] = index.to_ne_bytes();
        self.instructions.extend_from_slice(&bytes);
    }

    fn add_jump_index(&mut self) -> JumpIndex {
        self.add_double_index(0);
        JumpIndex {
            index: self.instructions.len() - core::mem::size_of::<u32>(),
        }
    }
}

/// A constant compared the way `Value`'s `PartialEq` compares it, hashed so
/// that equal constants hash alike.
#[derive(Clone, Copy)]
struct ConstantKey<'gc>(Value<'gc>);

impl PartialEq for ConstantKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for ConstantKey<'_> {}

impl Hash for ConstantKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        core::mem::discriminant(&self.0).hash(state);
        match self.0 {
            Value::Boolean(value) => value.hash(state),
            Value::String(value) => value.hash(state),
            Value::SmallString(value) => value.hash(state),
            Value::Integer(value) => value.into_i64().hash(state),
            Value::SmallF64(value) => value.into_f64().to_bits().hash(state),
            Value::Number(value) => value.get_index_u32().hash(state),
            Value::Symbol(value) => value.hash(state),
            Value::Object(value) => value.hash(state),
            _ => {}
        }
    }
}
