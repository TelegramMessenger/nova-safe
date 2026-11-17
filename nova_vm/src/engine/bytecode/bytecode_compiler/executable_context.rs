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
        Agent, BigInt, BindingSite, ExceptionType, Number, ObjectShape, PropertyKey,
        PropertyLookupCache, String, Value,
    },
    engine::{
        Bindable, Executable, ExecutableHeapData, FunctionExpression, Instruction, NoGcScope,
        bytecode::executable::ArrowFunctionExpression,
    },
    heap::{CreateHeapData, HeapIndexHandle},
};

use super::{IndexType, JumpIndex};
use crate::engine::CallEnvironment;

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
    /// How many BindingSites the instructions use: one for each place in the
    /// code that names a variable whose scope chain is fixed.
    binding_sites: usize,
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
            binding_sites: 0,
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
        self.recent_len = 0;
        self.caches.clear();
        self.cache_indexes.clear();
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
        self.agent.heap.create(ExecutableHeapData {
            instructions: self.instructions.into(),
            caches: self.caches.unbind().into_boxed_slice(),
            constants: self.constants.unbind().into_boxed_slice(),
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
            binding_sites: vec![BindingSite::UNKNOWN; self.binding_sites].into_boxed_slice(),
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
                if let Some(start) = self.recent_instruction(0, Instruction::PushReference) {
                    self.instructions[start] = Instruction::PushReferenceAndLoad.as_u8();
                    return;
                }
            }
            Instruction::IsStrictlyEqual => {
                if let Some(constant) = self.recent_instruction(0, Instruction::StoreConstant) {
                    let operand = self.operand_bytes(constant);
                    if let Some(load) = self.recent_instruction(1, Instruction::Load) {
                        self.drop_recent_instructions(load, 2);
                        self.push_instruction(Instruction::IsStrictlyEqualConstant);
                        self.instructions.extend_from_slice(&operand);
                        return;
                    }
                    if let Some(load) = self.recent_instruction(1, Instruction::LoadFromIndex) {
                        self.instructions[load] = Instruction::GetValueFromIndex.as_u8();
                        self.drop_recent_instructions(constant, 1);
                        self.push_instruction(Instruction::IsStrictlyEqualConstant);
                        self.instructions.extend_from_slice(&operand);
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

    /// Removes the `count` most recent instructions, which start at `start`.
    fn drop_recent_instructions(&mut self, start: usize, count: usize) {
        debug_assert!(count <= self.recent_len && self.recent[count - 1] == start);
        self.instructions.truncate(start);
        self.recent.copy_within(count.., 0);
        self.recent_len -= count;
    }

    fn forget_recent_instructions(&mut self) {
        self.recent_len = 0;
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
        self.push_instruction(instruction);
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
        if let Some(fused) = fused
            && let Some(access) =
                self.recent_instruction(0, Instruction::EvaluatePropertyAccessWithIdentifierKey)
        {
            let identifier = self.operand_bytes(access);
            self.drop_recent_instructions(access, 1);
            self.push_instruction(fused);
            self.instructions.extend_from_slice(&identifier);
        } else {
            self.push_instruction(instruction);
        }
        let cache = self.add_cache(cache);
        self.add_index_of(cache, TOO_MANY_PROPERTY_NAMES);
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
        let site = IndexType::try_from(self.binding_sites).ok()?;
        self.binding_sites += 1;
        Some(site)
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
        ));
        self.push_instruction(instruction);
        let identifier = self.add_identifier(identifier);
        self.add_index_of(identifier, TOO_MANY_NAMES);
        self.instructions.extend_from_slice(&site.to_ne_bytes());
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
