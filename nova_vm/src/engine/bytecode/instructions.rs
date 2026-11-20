// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{Agent, String},
    engine::{Executable, NoGcScope, Scoped, bytecode::bytecode_compiler::IndexType},
};

/// ## Notes
///
/// - This is inspired by and/or copied from Kiesel engine:
///   Copyright (c) 2023-2024 Linus Groh
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Instruction {
    // === HOT INSTRUCTIONS ===
    /// Load the result value and add it to the stack.
    Load,
    /// Load the result value to the given stack slot.
    PutValueToIndex,
    /// `++` of the variable in the stack slot given by the argument, where
    /// the value of the expression is not used: GetValueFromIndex, ToNumeric,
    /// Increment and PutValueToIndex as one instruction, which leaves no
    /// result.
    IncrementIndex,
    /// `--` of the variable in a stack slot, likewise.
    DecrementIndex,
    /// The tests of a `switch` whose `case` selectors are all literals, as
    /// one instruction: jumps to the first case that the value in the stack
    /// slot given by the first argument is strictly equal to, by the switch
    /// table the second argument names, and goes on to the next instruction
    /// where there is none.
    JumpToCase,
    /// LoadFromIndex followed by StoreConstant, as one instruction: a local
    /// and a constant, the operands of `i < 10` or `x * 2`. The arguments
    /// are the stack slot and the constant.
    LoadFromIndexStoreConstant,
    /// LoadFromIndex followed by GetValueFromIndex: two locals, the operands
    /// of `a + b`. The arguments are the two stack slots.
    LoadFromIndexGetFromIndex,
    /// LoadFromIndex twice: two locals as arguments of a call.
    LoadFromIndexLoadFromIndex,
    /// Load, StoreConstant and an arithmetic or bitwise
    /// Apply…BinaryOperator, as one instruction: `x & 255` or `x + 1`, with
    /// the left operand as the result value. The arguments are the constant
    /// and a description of the operation: the number of the operator (its
    /// place among the fused operators) in the four high bits, and below
    /// them a bit that says the result is pushed onto the stack, as a Load
    /// after the instruction would.
    ApplyBinaryOperatorWithConstant,
    /// LoadFromIndexStoreConstant and such an operator, as one instruction:
    /// `i - 1` or `n * 2` of a local. The first argument is the description
    /// of the operation, with the stack slot of the left operand in its low
    /// eleven bits; the second is the constant.
    ApplyBinaryOperatorToIndexWithConstant,
    /// An arithmetic or bitwise Apply…BinaryOperator and what is done with
    /// its value, as one instruction: PutValueToIndex of it (`sum = a + b`)
    /// or a Load (`a * b` as an operand). The argument is the description
    /// of the operation, with the stack slot that takes the value in its
    /// low eleven bits where it is not pushed.
    ApplyBinaryOperatorAndMove,
    /// Add the result value to the stack, without removing it as the result
    /// value.
    LoadCopy,
    /// Store the last value from the stack as the result value.
    Store,
    /// Store a copy of a value from the stack by index as the result value.
    GetValueFromIndex,
    /// Store a constant as the result value.
    StoreConstant,
    /// Store a constant from the wide constant table (past the 16-bit
    /// indices) as the result value.
    StoreConstantWide,
    /// Pop a value from the stack without storing it anywhere.
    PopStack,
    /// Jump to another instruction by setting the instruction pointer.
    Jump,
    /// Jump to another instruction by setting the instruction pointer
    /// if the current result is falsey.
    JumpIfNot,
    /// Store ResolveBinding() in the reference register.
    ResolveBinding,
    /// Store [GetValue()](https://tc39.es/ecma262/#sec-getvalue) as the result
    /// value.
    ///
    /// #### Note
    /// We only call `GetValue` on reference values. This can be statically
    /// analysed from the AST. Non-reference values are already in the result
    /// value so a `GetValue` call would be a no-op.
    GetValue,
    /// Store [GetValue()](https://tc39.es/ecma262/#sec-getvalue) as the result
    /// value. This variant caches the property lookup.
    ///
    /// #### Note
    /// We only call `GetValue` on reference values. This can be statically
    /// analysed from the AST. Non-reference values are already in the result
    /// value so a `GetValue` call would be a no-op.
    GetValueWithCache,
    /// Same as GetValue without taking the reference slot. Used for reference
    /// property updates and function calls (where `this` comes from the
    /// reference).
    GetValueKeepReference,
    GetValueWithCacheKeepReference,
    /// Call PutValue() with the last reference on the reference stack and the
    /// result value.
    PutValue,
    /// Same as PutValue but with a cache slot.
    PutValueWithCache,
    /// Store ToNumeric() as the result value.
    ToNumeric,
    /// Perform CreateImmutableBinding in the running execution context's
    /// LexicalEnvironment with an identifier parameter and `true`
    CreateImmutableBinding,
    /// Perform CreateMutableBinding in the running execution context's
    /// LexicalEnvironment with an identifier parameter and `false`
    CreateMutableBinding,
    /// Perform InitializeReferencedBinding with parameters reference (V) and
    /// result (W).
    InitializeReferencedBinding,
    /// Push the last evaluated reference, if any.
    PushReference,
    /// Pop the last stored reference.
    PopReference,
    /// Perform EvaluatePropertyAccessWithIdentifierKey with the `baseValue` in
    /// the result register and the `propertyNameString` given as the first
    /// immediate argument, and store the result in the reference register.
    EvaluatePropertyAccessWithIdentifierKey,
    /// Perform EvaluatePropertyAccessWithExpressionKey with the `baseValue` at
    /// the top of the stack and the `propertyNameValue` in result register,
    /// and store the result in the reference register.
    EvaluatePropertyAccessWithExpressionKey,
    /// EvaluatePropertyAccessWithExpressionKey followed by GetValue, as one
    /// instruction: `a[i]` read for its value, with no Reference when the
    /// base is an Array or a TypedArray and the key an integer.
    GetValueWithExpressionKey,
    /// Load, GetValueFromIndex and GetValueWithExpressionKey, as one
    /// instruction: `a[i]` read for its value where the key is a local, with
    /// the base as the result value. The argument is the stack slot.
    GetValueWithIndexKey,
    /// `a[i] = v` where the value of the assignment is not used: the object
    /// and the key are the two values on top of the stack, where they stayed
    /// while the value was evaluated into the result register. PutValue to
    /// the Reference that EvaluatePropertyAccessWithExpressionKey would have
    /// made of them, with no Reference when the object is an Array or a
    /// TypedArray and the key an integer.
    PutValueWithExpressionKey,
    /// GetValueFromIndex followed by Load: push a copy of the given stack
    /// slot, leaving the result register empty.
    LoadFromIndex,
    /// EvaluatePropertyAccessWithIdentifierKey followed by GetValueWithCache.
    /// The first immediate is the identifier, the second the cache.
    GetPropertyWithCache,
    /// EvaluatePropertyAccessWithIdentifierKey followed by
    /// GetValueWithCacheKeepReference.
    GetPropertyWithCacheKeepReference,
    /// GetPropertyWithCacheKeepReference, PushReference and Load: the
    /// function of `o.m(x)`, pushed with its Reference before the
    /// arguments. The immediates are the identifier and the cache.
    LoadPropertyWithCachePushReference,
    /// The function of `o.m(x)` as
    /// [`Instruction::LoadPropertyWithCachePushReference`] finds it, pushed
    /// after `o`, which is the this value of the call: the call
    /// ([`Instruction::EvaluateCallOnThis`]) finds both under its
    /// arguments, and no Reference is made, pushed and popped for the this
    /// value. The immediates are the identifier and the cache.
    LoadPropertyWithCachePushThis,
    /// ResolveThisBinding followed by GetPropertyWithCache: `this.x`. The
    /// immediates are the identifier and the cache.
    GetThisPropertyWithCache,
    /// ResolveThisBinding followed by GetPropertyWithCacheKeepReference:
    /// the function of `this.m()`.
    GetThisPropertyWithCacheKeepReference,
    /// GetValueFromIndex followed by GetPropertyWithCache: a property of a
    /// local, `p.x`. The arguments are the identifier and the cache, then
    /// the stack slot in four bytes more than other instructions have, of
    /// which the last two are not used.
    GetIndexPropertyWithCache,
    /// GetValueFromIndex followed by GetPropertyWithCacheKeepReference: the
    /// function of `p.m()`.
    GetIndexPropertyWithCacheKeepReference,
    /// GetValueFromIndex followed by
    /// [`Instruction::LoadPropertyWithCachePushThis`]: the function of
    /// `p.m(x)` where `p` is a local, pushed after `p`. The arguments are
    /// those of [`Instruction::GetIndexPropertyWithCache`].
    LoadIndexPropertyWithCachePushThis,
    /// `object.name = value` with the object on the stack and the value as
    /// the result, where the value of the assignment is not used: what
    /// EvaluatePropertyAccessWithIdentifierKey of the object and
    /// PutValueWithCache of the value do, with no Reference kept in
    /// between. The immediates are the identifier and the cache.
    PutPropertyWithCache,
    /// PushReference followed by Load.
    PushReferenceAndLoad,

    // === COLD INSTRUCTIONS ===

    // = INLINED COLD INSTRUCTIONS =
    /// Stop bytecode execution, indicating a return from the current function.
    Return,
    /// Performs Await() on the result value, and after resuming, stores the
    /// promise result as the result value.
    Await,
    /// Performs Yield() on the result value, and after resuming, stores the
    /// value passed to `next()` as the result value.
    Yield,
    /// Yield for `yield value` in a generator that is not async: the result
    /// value is the value itself, and the iterator result object for it is
    /// made by whoever resumed the generator, if it wants one. A loop that
    /// steps the generator takes the value as it is.
    YieldValue,
    /// Take the result value and the top stack value, compare them using
    /// IsStrictlyEqual() and store the result as the result value.
    IsStrictlyEqual,
    /// Load, StoreConstant and IsStrictlyEqual: compare the result value with
    /// a constant using IsStrictlyEqual() and store the result as the result
    /// value.
    IsStrictlyEqualConstant,
    /// Store true as the result value if the current result value is null or
    /// undefined, false otherwise.
    IsNullOrUndefined,
    /// Store true as the result value if the current result value is null,
    /// false otherwise.
    IsNull,
    /// Store true as the result value if the current result value is undefined,
    /// false otherwise.
    IsUndefined,
    /// Store true as the result value if the current result value is an
    /// object.
    IsObject,
    /// Call IsConstructor() on the current result value and store the result
    /// as the result value.
    IsConstructor,
    /// Jump to another instruction by setting the instruction pointer if the
    /// current result is truthy.
    JumpIfTrue,
    /// LessThan followed by JumpIfNot, as one instruction: the test of most
    /// loops. The comparison leaves no result.
    JumpIfNotLessThan,
    /// LessThanEquals followed by JumpIfNot.
    JumpIfNotLessThanEquals,
    /// GreaterThan followed by JumpIfNot.
    JumpIfNotGreaterThan,
    /// GreaterThanEquals followed by JumpIfNot.
    JumpIfNotGreaterThanEquals,
    /// LessThan followed by JumpIfTrue, as one instruction: the test at
    /// the end of a loop, and the jump back. The comparison leaves no
    /// result.
    JumpIfLessThan,
    /// LessThanEquals followed by JumpIfTrue.
    JumpIfLessThanEquals,
    /// GreaterThan followed by JumpIfTrue.
    JumpIfGreaterThan,
    /// GreaterThanEquals followed by JumpIfTrue.
    JumpIfGreaterThanEquals,
    /// IsStrictlyEqual followed by JumpIfNot, as one instruction: the test
    /// of `if (a === b)`.
    JumpIfNotStrictlyEqual,
    /// IsStrictlyEqual followed by JumpIfTrue: the test of `if (a !== b)`.
    JumpIfStrictlyEqual,
    /// IsLooselyEqual followed by JumpIfNot: the test of `if (a == b)`.
    JumpIfNotLooselyEqual,
    /// IsLooselyEqual followed by JumpIfTrue: the test of `if (a != b)`.
    JumpIfLooselyEqual,
    /// LoadFromIndexStoreConstant followed by JumpIfNotLessThan, as one
    /// instruction: the test of `i < 10` on a local, and the jump on it.
    /// This and the eleven after it take a stack slot, then a constant or a
    /// second stack slot, then the jump target in four bytes more than
    /// other instructions have: a local compared with a constant or with
    /// another local by the comparison the name says, and the jump that
    /// the instruction of that name without Index makes on it.
    JumpIfIndexNotLessThanConstant,
    JumpIfIndexNotLessThanIndex,
    JumpIfIndexNotLessThanEqualsConstant,
    JumpIfIndexNotLessThanEqualsIndex,
    JumpIfIndexNotGreaterThanConstant,
    JumpIfIndexNotGreaterThanIndex,
    JumpIfIndexNotGreaterThanEqualsConstant,
    JumpIfIndexNotGreaterThanEqualsIndex,
    JumpIfIndexNotStrictlyEqualConstant,
    JumpIfIndexNotStrictlyEqualIndex,
    JumpIfIndexStrictlyEqualConstant,
    JumpIfIndexStrictlyEqualIndex,
    /// The test at the end of a loop: a local compared with a constant or
    /// another local, and the jump back where the comparison holds. This
    /// and the fifteen after it take a stack slot, a constant or a second
    /// stack slot, and a jump target, like the twelve before; those named
    /// IncrementIndex and DecrementIndex first do `++` or `--` on the first
    /// stack slot, as IncrementIndex and DecrementIndex do.
    JumpIfIndexLessThanConstant,
    JumpIfIndexLessThanIndex,
    JumpIfIndexLessThanEqualsConstant,
    JumpIfIndexLessThanEqualsIndex,
    JumpIfIndexGreaterThanConstant,
    JumpIfIndexGreaterThanIndex,
    JumpIfIndexGreaterThanEqualsConstant,
    JumpIfIndexGreaterThanEqualsIndex,
    IncrementIndexJumpIfLessThanConstant,
    IncrementIndexJumpIfLessThanIndex,
    IncrementIndexJumpIfLessThanEqualsConstant,
    IncrementIndexJumpIfLessThanEqualsIndex,
    DecrementIndexJumpIfGreaterThanConstant,
    DecrementIndexJumpIfGreaterThanIndex,
    DecrementIndexJumpIfGreaterThanEqualsConstant,
    DecrementIndexJumpIfGreaterThanEqualsIndex,
    /// Load the result value, if present, to the top of the stack, replacing
    /// the previous top of the stack value.
    LoadReplace,
    /// Load a constant and add it to the stack.
    LoadConstant,
    /// Load a constant from the wide constant table (past the 16-bit indices)
    /// and add it to the stack.
    LoadConstantWide,
    /// Swaps the last value in the stack and the result value.
    LoadStoreSwap,
    /// Perform UpdateEmpty with the value coming from the top of the stack,
    /// and the Completion Record \[\[Value]] being the result value.
    UpdateEmpty,
    /// Swap the last two values on the stack.
    Swap,
    /// Empty the result register.
    Empty,
    /// Performs steps 2-4 from the [UnaryExpression ! Runtime Semantics](https://tc39.es/ecma262/#sec-logical-not-operator-runtime-semantics-evaluation).
    LogicalNot,

    // = OUTLINED COLD INSTRUCTIONS =
    /// Store ApplyStringOrNumericBinaryOperator() as the result value.
    ApplyAdditionBinaryOperator,
    ApplySubtractionBinaryOperator,
    ApplyMultiplicationBinaryOperator,
    ApplyDivisionBinaryOperator,
    ApplyRemainderBinaryOperator,
    ApplyExponentialBinaryOperator,
    ApplyShiftLeftBinaryOperator,
    ApplyShiftRightBinaryOperator,
    ApplyShiftRightZeroFillBinaryOperator,
    ApplyBitwiseORBinaryOperator,
    ApplyBitwiseXORBinaryOperator,
    ApplyBitwiseAndBinaryOperator,
    /// Store ArrayCreate(0) as the result value.
    ///
    /// This instruction has one immediate argument that is the minimum
    /// number of elements to expect.
    ArrayCreate,
    /// Store as the result value an Array of the values on top of the
    /// stack, the deepest of them its first element, and pop them: an
    /// array literal of values, with no spread element and no hole.
    ///
    /// This instruction has one immediate argument that is the number of
    /// the values.
    ArrayFromStack,
    /// Push a value into an array
    ArrayPush,
    /// Push a hole into an array
    ArrayElision,
    /// Performs steps 2-4 from the [UnaryExpression ~ Runtime Semantics](https://tc39.es/ecma262/#sec-bitwise-not-operator-runtime-semantics-evaluation).
    BitwiseNot,
    /// Performs CreateUnmappedArgumentsObject() on the arguments list present
    /// in the iterator stack, and stores the created arguments object as the
    /// result value.
    CreateUnmappedArgumentsObject,
    /// Performs CopyDataProperties() with the source being the result value and
    /// the target object being at the top of the stack. The excluded names list
    /// will be empty.
    CopyDataProperties,
    /// Performs CopyDataProperties() into a newly created object and returns it.
    /// The source object will be on the result value, and the excluded names
    /// will be read from the reference stack, with the number of names passed
    /// in an immediate.
    CopyDataPropertiesIntoObject,
    /// Apply the delete operation to the evaluated expression and set it as
    /// the result value.
    Delete,
    /// Call the `eval` function in a direct way.
    ///
    /// If the `eval` identifier points to the current realm's eval intrinsic
    /// function, then it performs a direct eval. Otherwise, it loads the value
    /// that identifier points to, and calls it.
    ///
    /// This instruction has the number of argument values that need to be
    /// popped from the stack (last to first) as an argument.
    DirectEvalCall,
    /// Store EvaluateCall() as the result value.
    ///
    /// This instruction has the number of argument values that need to be
    /// popped from the stack (last to first) as an argument, and finally the
    /// function to call.
    EvaluateCall,
    /// PopReference followed by EvaluateCall.
    PopReferenceAndEvaluateCall,
    /// EvaluateCall of a function whose this value is on the stack under
    /// it, where [`Instruction::LoadPropertyWithCachePushThis`] pushed it:
    /// the arguments, the function and the this value are popped.
    EvaluateCallOnThis,
    /// Store EvaluateNew() as the result value.
    ///
    /// This instruction has the number of argument values that need to be
    /// popped from the stack (last to first) as an argument, the value on the
    /// stack afterwards is the constructor to call.
    EvaluateNew,
    /// Store SuperCall() as the result value.
    ///
    /// This instruction has the number of argument values that need to be
    /// popped from the stack (last to first) as an argument.
    EvaluateSuper,
    /// Perform MakePrivateReference with the `baseValue` in the result
    /// register and the `privateIdentifier` given as the first immediate
    /// argument, and store the result in the reference register.
    MakePrivateReference,
    /// Perform MakeSuperPropertyReference with the `propertyKey` in the result
    /// register, and store the result in the reference register.
    MakeSuperPropertyReferenceWithExpressionKey,
    /// Perform MakeSuperPropertyReference with the `propertyKey` given as the
    /// first immediate argument, and store the result in the reference
    /// register.
    MakeSuperPropertyReferenceWithIdentifierKey,
    /// Compare the last two values on the stack using the '>' operator rules.
    GreaterThan,
    /// Compare the last two values on the stack using the '>=' operator rules.
    GreaterThanEquals,
    /// Store HasProperty() as the result value.
    HasProperty,
    /// Perform PrivateElementFind on the private property reference currently
    /// in the reference register, and convert the result into a boolean.
    HasPrivateElement,
    Increment,
    Decrement,
    /// Store InstanceofOperator() as the result value.
    InstanceofOperator,
    /// Store InstantiateArrowFunctionExpression() as the result value.
    InstantiateArrowFunctionExpression,
    /// Store InstantiateOrdinaryFunctionExpression() as the result value.
    InstantiateOrdinaryFunctionExpression,
    /// Create a class constructor and store it as the result value.
    ///
    /// The class name should be found at the top of the stack.
    /// If the class is a derived class, then the parent constructor should
    /// also be on the stack after the class name.
    ClassDefineConstructor,
    /// Store CreateBuiltinFunction(defaultConstructor, 0, className) as the
    /// result value.
    ClassDefineDefaultConstructor,
    /// Define a private method on class constructor or instances.
    ///
    /// The target object is at the top or second from the top of the stack,
    /// and the method's PrivateName's `[[Description]]` String is the
    /// current result value, the method's function expression is provided as
    /// the first immediate, and the method's getter/setter metadata is
    /// provided as the second immediate. The last bit in the metadata is the
    /// Get flag, the second last bit is the Set flag, and the third last bit
    /// is the Static flag.
    ClassDefinePrivateMethod,
    /// Define a private property field on class constructor or instances.
    ///
    /// The field's PrivateName's `[[Description]]` String is provided as an
    /// identifier, and the staticness of the field is provided as an
    /// immediate.
    ClassDefinePrivateProperty,
    /// Reserves enough room for all of a class instance's PrivateElements
    /// fields in the backing object, and copies all private methods to the
    /// backing object.
    ///
    /// The target object is at the top of the stack; it should be the `this`
    /// value. The target is not popped off the stack.
    ClassInitializePrivateElements,
    /// Put the current result value at the next PrivateName's slot in the
    /// target object. The PrivateName is calculated based on the offset
    /// provided as an immediate, and the current PrivateEnvironment.
    ///
    /// The target object is at the top of the stack. the target is not popped
    /// off the stack.
    ClassInitializePrivateValue,
    /// Store IsLooselyEqual() as the result value.
    IsLooselyEqual,
    /// Compare the last two values on the stack using the '<' operator rules.
    LessThan,
    /// Compare the last two values on the stack using the '<=' operator rules.
    LessThanEquals,
    /// Store OrdinaryObjectCreate(%Object.prototype%) on the stack.
    ObjectCreate,
    /// Store a new as the result Object created with the given shape, with its
    /// properties coming from the stack.
    ObjectCreateWithShape,
    /// Call CreateDataPropertyOrThrow(object, key, value) with value being the
    /// result value, key being the top stack value and object being the second
    /// stack value. The object is not popped from the stack.
    ObjectDefineProperty,
    /// Create and define a method on an object.
    ///
    /// The key is at the top stack value, the object is second on the stack.
    ObjectDefineMethod,
    ObjectDefineGetter,
    ObjectDefineSetter,
    /// Call `object[[SetPrototypeOf]](value)` on the object on the stack using
    /// the current result value as the parameter.
    ObjectSetPrototype,
    /// Push a jump target for uncaught exceptions
    PushExceptionJumpTarget,
    /// Pop a jump target for uncaught exceptions
    PopExceptionJumpTarget,
    /// Truncate the runtime stack to the given depth.
    TruncateStack,
    /// Store ResolveBinding() in the reference register using a property
    /// lookup cache.
    ResolveBindingWithCache,
    /// Store ResolveBinding() in the reference register for a name whose
    /// scope chain no `with` or direct `eval` can change, remembering where
    /// the name was found (a BindingSite of the executable) for the next
    /// time.
    ResolveBindingAtSite,
    /// Store in the result register the value of a name whose scope chain no
    /// `with` or direct `eval` can change: GetValue(ResolveBinding()) with no
    /// Reference when the name is where the BindingSite remembers it.
    GetBindingAtSite,
    /// GetBindingAtSite followed by Load: the value of such a name, pushed
    /// onto the stack.
    LoadBindingAtSite,
    /// GetBindingAtSite for a name declared outside the running function
    /// that nothing assigns to once it is initialized: the value, once
    /// read, is kept for the rest of the call in a stack slot of the
    /// function (the third argument, which is undefined while nothing is
    /// kept), and is read from there after.
    GetBindingAtSiteOnce,
    /// GetBindingAtSiteOnce followed by Load.
    LoadBindingAtSiteOnce,
    /// Set a name whose scope chain no `with` or direct `eval` can change to
    /// the value in the result register: PutValue(ResolveBinding(), value)
    /// with no Reference when the name is where the BindingSite remembers it.
    PutBindingAtSite,
    /// ResolveBindingAtSite for a name whose scope chain is fixed all the way
    /// out to the global scope of a Script: where it was found is remembered
    /// there too.
    ResolveGlobalAtSite,
    /// GetBindingAtSite for such a name.
    GetGlobalAtSite,
    /// PutBindingAtSite for such a name, where the name is known to be bound
    /// (a lexical declaration of the Script, or a name that GetGlobalAtSite
    /// has just read for the same assignment).
    PutGlobalAtSite,
    /// Set the Reference that ResolveGlobalAtSite left in the reference
    /// register to the value in the result register: PutValue(reference,
    /// value), written at the place the BindingSite remembers when that is
    /// what the Reference is to.
    PutValueAtSite,
    /// Store ResolveThisBinding() in the result register.
    ResolveThisBinding,

    /// Store a copy of the last value from the stack as the result value.
    StoreCopy,
    /// Replace the RegExp in the result register, which is the one a
    /// RegularExpressionLiteral was compiled to, with a new RegExp of its
    /// pattern and flags: what an evaluation of the literal returns.
    InstantiateRegExpLiteral,
    /// Take N Strings from the stack and string-concatenate them together:
    /// the operands of a chain of `+` that starts with a String, or the
    /// parts of a template literal, each already the String it is there.
    ConcatStrings,
    /// Make the result value the String it is as an operand of `+` whose
    /// other operand is a String: ToPrimitive, then ToString.
    ToStringOperand,
    /// Make the result value a String as ToString does: a substitution of a
    /// template literal.
    ToStringSubstitution,
    /// Throw the result value as an exception.
    Throw,
    /// Throw a new Error object as an exception with the result value as the
    /// message.
    ///
    /// The error subtype is determined by an immediate value.
    ThrowError,
    /// Store ToNumber() as the result value.
    ToNumber,
    /// Store ToObject() as the result value.
    ToObject,
    /// Apply the typeof operation to the evaluated expression and set it as
    /// the result value.
    Typeof,
    /// Performs steps 3 and 4 from the [UnaryExpression - Runtime Semantics](https://tc39.es/ecma262/#sec-unary-minus-operator-runtime-semantics-evaluation).
    UnaryMinus,

    /// Create a new VariableEnvironment and initialize it with variable names
    /// and values from the stack, where each name comes before the value.
    /// The first immediate argument is the number of variables to initialize.
    /// The second immediate is a boolean which is true if LexicalEnvironment
    /// should also be set to this new environment (true in strict mode), or
    /// false if it should be set to a new descendant declarative environment.
    InitializeVariableEnvironment,
    /// Perform NewDeclarativeEnvironment with the running execution context's
    /// LexicalEnvironment as the only parameter and set it as the running
    /// execution context's LexicalEnvironment.
    ///
    /// #### Note
    /// It is technically against the spec to immediately set the new
    /// environment as the running execution context's LexicalEnvironment. The
    /// spec requires that creation of bindings in the environment is done
    /// first. This is immaterial because creating the bindings cannot fail.
    EnterDeclarativeEnvironment,
    /// Enter a new FunctionEnvironment with the top of the stack as the this
    /// binding and `[[FunctionObject]]`. This is used for class static
    /// initializers.
    EnterClassStaticElementEnvironment,
    /// Perform NewPrivateEnvironment with the running execution context's
    /// PrivateEnvironment and enter it.
    ///
    /// The number of private names in the environment is given
    EnterPrivateEnvironment,
    /// Reset the running execution context's LexicalEnvironment to its current
    /// value's `[[OuterEnv]]`.
    ExitDeclarativeEnvironment,
    /// Reset the running execution context's VariableEnvironment to its
    /// current value's `[[OuterEnv]]`.
    ExitVariableEnvironment,
    /// Reset the running execution context's PrivateEnvironment to its current
    /// value's `[[OuterPrivateEnvironment]]`.
    ExitPrivateEnvironment,
    /// Begin binding values using destructuring
    BeginSimpleObjectBindingPattern,
    /// Begin binding values using a sync iterator for known repetitions
    BeginSimpleArrayBindingPattern,
    /// Bind the parameters of a function whose parameters are all plain
    /// names kept in consecutive stack slots: the slots from the first
    /// immediate on, as many as the second immediate says, take the next
    /// arguments of the call, or stay undefined where there are no more.
    BindArguments,
    /// In array binding patterns, bind the current result to the given
    /// identifier. In object binding patterns, bind the object's property with
    /// the identifier's name.
    ///
    /// ```js
    /// const { a } = x;
    /// const [a] = x;
    /// ```
    BindingPatternBind,
    /// Bind an object property to an identifier with a different name. The
    /// constant given as the second argument is the property key.
    ///
    /// ```js
    /// const { a: b } = x;
    /// ```
    BindingPatternBindNamed,
    /// Bind an object property to a stack variable. The constant given as the
    /// second argument is the stack slot.
    BindingPatternBindToIndex,
    /// Bind all remaining values to given identifier.
    ///
    /// ```js
    /// const { a, ...b } = x;
    /// const [a, ...b] = x;
    /// ```
    BindingPatternBindRest,
    /// Bind all remaining values to a given stack index.
    ///
    /// ```js
    /// const { a, ...b } = x;
    /// const [a, ...b] = x;
    /// ```
    BindingPatternBindRestToIndex,
    /// Skip the next value in an array binding pattern.
    ///
    /// ```js
    /// const [a,,b] = x;
    /// ```
    BindingPatternSkip,
    /// Load the next value in an array binding pattern onto the stack.
    ///
    /// This is used to implement nested binding patterns. The current binding
    /// pattern needs to take the "next step", but instead of binding to an
    /// identifier it is instead saved on the stack and the nested binding
    /// pattern starts its work.
    ///
    /// ```js
    /// const [{ a }] = x;
    /// ```
    BindingPatternGetValue,
    /// Load the value of the property with the given name in an object binding
    /// pattern onto the stack. The name is passed as a constant argument.
    ///
    /// This is used to implement nested binding patterns. The current binding
    /// pattern needs to take the "next step", but instead of binding to an
    /// identifier it is instead saved on the stack and the nested binding
    /// pattern starts its work.
    ///
    /// ```js
    /// const { a: [b] } = x;
    /// ```
    BindingPatternGetValueNamed,
    /// Load all remaining values onto the stack
    ///
    /// This is used to implement nested binding patterns in rest elements.
    ///
    /// ```js
    /// const [a, ...[b, c]] = x;
    /// ```
    BindingPatternGetRestValue,
    /// Finish binding values
    ///
    /// This stops
    FinishBindingPattern,
    /// Take the current result and begin iterating it according to
    /// EnumerateObjectProperties.
    EnumerateObjectProperties,
    /// Take the current result and call `GetIterator(result, SYNC)`
    GetIteratorSync,
    /// Take the current result and call `GetIterator(result, ASYNC)`
    GetIteratorAsync,
    /// Perform IteratorStepValue on the current iterator and jump to
    /// index if iterator completed.
    IteratorStepValue,
    /// Perform IteratorStepValue on the current iterator, putting the resulting
    /// value on the result value, or undefined if the iterator has completed.
    ///
    /// When the iterator has completed, rather than popping it off the stack,
    /// it sets it to `VmIterator::EmptyIterator` so further reads and closes
    /// aren't observable.
    IteratorStepValueOrUndefined,
    /// Call the current iterator's `[[NextMethod]]` with the current result
    /// register value as the only parameter.
    IteratorCallNextMethod,
    /// Verify that the current result register value contains an object, perform IteratorComplete on it, and if the
    /// result is `true` then perform IteratorValue on it and jump to the
    /// provided instruction.
    IteratorComplete,
    /// Perform IteratorValue on the current result register value.
    IteratorValue,
    /// Perform `? GetMethod(iterator, "return")` on the current iterator and
    /// call the result if it is not undefined. If the result is undefined,
    /// jump to the provided instruction.
    IteratorThrow,
    /// Perform `? GetMethod(iterator, "throw")` on the current iterator and
    /// call the result if it is not undefined. If the result is undefined,
    /// jump to the provided instruction.
    IteratorReturn,
    /// Consume the remainder of the iterator, and produce a new array with
    /// those elements. This replaces the iterator with a special exhausted
    /// iterator after use.
    IteratorRestIntoArray,
    /// Perform CloseIterator on the current iterator
    IteratorClose,
    /// Perform AsyncCloseIterator on the current iterator
    AsyncIteratorClose,
    /// Perform CloseIterator on the current iterator with the current result
    /// as a thrown value.
    ///
    /// This will call the `return` method of the current iterator, ignoring
    /// all errors, and then continues with the thrown value still as the
    /// current result.
    IteratorCloseWithError,
    /// Perform AsyncCloseIterator on the current iterator with the current
    /// result as a thrown value.
    ///
    /// This will call the `return` method of the current iterator. If the
    /// method is found and returns a value, then the current result is stored
    /// onto the stack, a special "ignore thrown error and next instruction"
    /// exception jump target handler is installed, and then an await is
    /// performed. If an error is thrown or no method exists, then the current
    /// result is rethrown immediately.
    ///
    /// This instruction should always be followed by the following bytecode
    /// snippet:
    /// ```rust,ignore
    /// // Pop the special exception jump target handler if await didn't throw.
    /// // Note: this is skipped by the special handler if Await did throw.
    /// Instruction::PopExceptionJumpTarget;
    /// // Return the current result into the result register.
    /// Instruction::Store;
    /// // Rethrow the current result.
    /// Instruction::Throw;
    /// ```
    AsyncIteratorCloseWithError,
    /// Pop the current iterator from the iterator stack.
    IteratorPop,
    /// Store GetNewTarget() as the result value.
    GetNewTarget,
    /// Perform EvaluateImportCall with specifier at the top of the stack, and
    /// options (optionally) in the result register. Pops the stack and places
    /// a dynamic import Promise into the result register.
    ImportCall,
    /// Store `import.meta` object as the result value.
    ImportMeta,
    /// Throw a TypeError if the result register does not contain an Object.
    ///
    /// The error message is provided as an identifier.
    VerifyIsObject,
    Debug,
    /// GetBindingAtSiteOnce followed by GetPropertyWithCache: a property
    /// of a name declared outside the running function that nothing
    /// assigns to, `o.x`, where the code has read the name before with
    /// nothing jumping in since. The arguments are those of
    /// [`Instruction::GetIndexPropertyWithCache`]: the identifier, the
    /// cache, and the stack slot, which is the one that keeps the value of
    /// the name once the call has read it; the two bytes after the slot
    /// are the BindingSite of the name. The name is no argument: the site
    /// has it.
    GetKeptPropertyWithCache,
    /// Load, StoreConstant and a comparison that jumps, as one instruction:
    /// what was just computed (the result register, which the instruction
    /// empties) is compared with a constant, as in `p.x < 0`; or
    /// IsStrictlyEqualConstant and the jump on it, `f() === 3`. The first
    /// argument says which comparison it is and which way it jumps
    /// ([`Instruction::result_comparison`]), the second is the constant,
    /// the last two are the jump target.
    JumpIfResultComparesWithConstant,
    /// `++` or `--` of a property whose value was just read with its
    /// Reference kept (`--p.life`, `o.count++`): ToNumeric, Increment or
    /// Decrement and PutValueWithCache as one instruction, with the
    /// LoadCopy and the Store that keep the value of the expression where
    /// it is used. The first argument is the cache of the assignment, the
    /// second says which it is ([`Instruction::STEP_DOWN`] for `--`) and
    /// what it leaves in the result register: what PutValueWithCache does
    /// for a statement, the new value ([`Instruction::STEP_KEEPS_NEW`]) or
    /// the value before ([`Instruction::STEP_KEEPS_OLD`]).
    StepValueWithCache,
    // The bytes that are no instruction, each with a kind of its own, so
    // that every byte is a kind: the loop of a `Vm` then reads the kind
    // of an instruction without asking whether its byte is one, a test
    // and a branch before every instruction. Code that has one of these
    // bytes is no code: [`Instruction::is_unused`] says so to whoever
    // reads instructions with care, and the loop leaves the byte to the
    // general way, which ends as it did. A new instruction takes the
    // first of them.
    Unused234,
    Unused235,
    Unused236,
    Unused237,
    Unused238,
    Unused239,
    Unused240,
    Unused241,
    Unused242,
    Unused243,
    Unused244,
    Unused245,
    Unused246,
    Unused247,
    Unused248,
    Unused249,
    Unused250,
    Unused251,
    Unused252,
    Unused253,
    Unused254,
    Unused255,
}

const _EVERY_BYTE_IS_A_KIND: () = assert!(Instruction::Unused255.as_u8() == u8::MAX);

/// The kinds of the bytes that are no instruction, as one pattern: for a
/// `match` that has an arm for every kind and is to stay one.
macro_rules! unused_kinds {
    () => {
        Instruction::Unused234
            | Instruction::Unused235
            | Instruction::Unused236
            | Instruction::Unused237
            | Instruction::Unused238
            | Instruction::Unused239
            | Instruction::Unused240
            | Instruction::Unused241
            | Instruction::Unused242
            | Instruction::Unused243
            | Instruction::Unused244
            | Instruction::Unused245
            | Instruction::Unused246
            | Instruction::Unused247
            | Instruction::Unused248
            | Instruction::Unused249
            | Instruction::Unused250
            | Instruction::Unused251
            | Instruction::Unused252
            | Instruction::Unused253
            | Instruction::Unused254
            | Instruction::Unused255
    };
}
pub(super) use unused_kinds;

impl Instruction {
    /// Returns true if this instruction is a terminal instruction where
    /// control flow cannot continue to the next instruction.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Jump | Self::Return | Self::Throw | Self::ThrowError
        )
    }

    /// How many bytes the instruction takes in the code: itself and its
    /// arguments.
    pub(super) const fn length(self) -> usize {
        1 + self.argument_count() as usize * 2
    }

    #[inline(always)]
    pub const fn argument_count(self) -> u8 {
        match self {
            // A stack slot, a constant or stack slot, and a jump target.
            Self::JumpIfIndexNotLessThanConstant
            | Self::JumpIfIndexNotLessThanIndex
            | Self::JumpIfIndexNotLessThanEqualsConstant
            | Self::JumpIfIndexNotLessThanEqualsIndex
            | Self::JumpIfIndexNotGreaterThanConstant
            | Self::JumpIfIndexNotGreaterThanIndex
            | Self::JumpIfIndexNotGreaterThanEqualsConstant
            | Self::JumpIfIndexNotGreaterThanEqualsIndex
            | Self::JumpIfIndexNotStrictlyEqualConstant
            | Self::JumpIfIndexNotStrictlyEqualIndex
            | Self::JumpIfIndexStrictlyEqualConstant
            | Self::JumpIfIndexStrictlyEqualIndex
            | Self::GetIndexPropertyWithCache
            | Self::GetIndexPropertyWithCacheKeepReference
            | Self::LoadIndexPropertyWithCachePushThis
            | Self::GetKeptPropertyWithCache
            | Self::JumpIfResultComparesWithConstant
            | Self::GetBindingAtSiteOnce
            | Self::LoadBindingAtSiteOnce
            | Self::JumpIfIndexLessThanConstant
            | Self::JumpIfIndexLessThanIndex
            | Self::JumpIfIndexLessThanEqualsConstant
            | Self::JumpIfIndexLessThanEqualsIndex
            | Self::JumpIfIndexGreaterThanConstant
            | Self::JumpIfIndexGreaterThanIndex
            | Self::JumpIfIndexGreaterThanEqualsConstant
            | Self::JumpIfIndexGreaterThanEqualsIndex
            | Self::IncrementIndexJumpIfLessThanConstant
            | Self::IncrementIndexJumpIfLessThanIndex
            | Self::IncrementIndexJumpIfLessThanEqualsConstant
            | Self::IncrementIndexJumpIfLessThanEqualsIndex
            | Self::DecrementIndexJumpIfGreaterThanConstant
            | Self::DecrementIndexJumpIfGreaterThanIndex
            | Self::DecrementIndexJumpIfGreaterThanEqualsConstant
            | Self::DecrementIndexJumpIfGreaterThanEqualsIndex => 4,
            // Number of repetitions and lexical status
            Self::BeginSimpleArrayBindingPattern
            | Self::BindArguments
            | Self::JumpToCase
            | Self::BindingPatternBindNamed
            | Self::BindingPatternBindToIndex
            | Self::ClassDefineConstructor
            | Self::ClassDefinePrivateMethod
            | Self::ClassDefinePrivateProperty
            | Self::InitializeVariableEnvironment
            | Self::IteratorStepValue
            | Self::IteratorComplete
            | Self::IteratorThrow
            | Self::IteratorReturn
            | Self::Jump
            | Self::JumpIfNot
            | Self::JumpIfTrue
            | Self::LoadFromIndexStoreConstant
            | Self::ApplyBinaryOperatorWithConstant
            | Self::ApplyBinaryOperatorToIndexWithConstant
            | Self::LoadFromIndexGetFromIndex
            | Self::LoadFromIndexLoadFromIndex
            | Self::JumpIfNotLessThan
            | Self::JumpIfNotLessThanEquals
            | Self::JumpIfNotGreaterThan
            | Self::JumpIfNotGreaterThanEquals
            | Self::JumpIfLessThan
            | Self::JumpIfLessThanEquals
            | Self::JumpIfGreaterThan
            | Self::JumpIfGreaterThanEquals
            | Self::JumpIfNotStrictlyEqual
            | Self::JumpIfStrictlyEqual
            | Self::JumpIfNotLooselyEqual
            | Self::JumpIfLooselyEqual
            | Self::LoadConstantWide
            | Self::StoreConstantWide
            | Self::ObjectDefineGetter
            | Self::ObjectDefineMethod
            | Self::ObjectDefineSetter
            | Self::PushExceptionJumpTarget
            | Self::GetPropertyWithCache
            | Self::GetPropertyWithCacheKeepReference
            | Self::LoadPropertyWithCachePushReference
            | Self::LoadPropertyWithCachePushThis
            | Self::GetThisPropertyWithCache
            | Self::GetThisPropertyWithCacheKeepReference
            | Self::PutPropertyWithCache
            | Self::StepValueWithCache
            | Self::ResolveBindingWithCache
            | Self::ResolveBindingAtSite
            | Self::GetBindingAtSite
            | Self::LoadBindingAtSite
            | Self::PutBindingAtSite
            | Self::ResolveGlobalAtSite
            | Self::GetGlobalAtSite
            | Self::PutGlobalAtSite
            | Self::PutValueAtSite => 2,
            Self::PutValueToIndex
            | Self::IncrementIndex
            | Self::DecrementIndex
            | Self::GetValueFromIndex
            | Self::ArrayCreate
            | Self::ArrayFromStack
            | Self::BeginSimpleObjectBindingPattern
            | Self::BindingPatternBind
            | Self::BindingPatternBindRest
            | Self::BindingPatternBindRestToIndex
            | Self::BindingPatternGetValueNamed
            | Self::ClassDefineDefaultConstructor
            | Self::ClassInitializePrivateValue
            | Self::CopyDataPropertiesIntoObject
            | Self::CreateImmutableBinding
            | Self::CreateMutableBinding
            | Self::DirectEvalCall
            | Self::EnterPrivateEnvironment
            | Self::EvaluateCall
            | Self::PopReferenceAndEvaluateCall
            | Self::EvaluateCallOnThis
            | Self::EvaluateNew
            | Self::EvaluatePropertyAccessWithIdentifierKey
            | Self::EvaluateSuper
            | Self::GetValueWithCache
            | Self::GetValueWithCacheKeepReference
            | Self::IsStrictlyEqualConstant
            | Self::LoadFromIndex
            | Self::InstantiateArrowFunctionExpression
            | Self::InstantiateOrdinaryFunctionExpression
            | Self::LoadConstant
            | Self::MakePrivateReference
            | Self::MakeSuperPropertyReferenceWithIdentifierKey
            | Self::ObjectCreateWithShape
            | Self::TruncateStack
            | Self::PutValueWithCache
            | Self::ResolveBinding
            | Self::StoreConstant
            | Self::ConcatStrings
            | Self::ThrowError
            | Self::GetValueWithIndexKey
            | Self::ApplyBinaryOperatorAndMove
            | Self::VerifyIsObject => 1,
            _ => 0,
        }
    }

    pub(crate) fn has_double_arg(self) -> bool {
        debug_assert_eq!(self.argument_count(), 2);
        matches!(
            self,
            Self::IteratorStepValue
                | Self::Jump
                | Self::JumpIfNot
                | Self::JumpIfTrue
                | Self::JumpIfNotLessThan
                | Self::JumpIfNotLessThanEquals
                | Self::JumpIfNotGreaterThan
                | Self::JumpIfNotGreaterThanEquals
                | Self::JumpIfLessThan
                | Self::JumpIfLessThanEquals
                | Self::JumpIfGreaterThan
                | Self::JumpIfGreaterThanEquals
                | Self::JumpIfNotStrictlyEqual
                | Self::JumpIfStrictlyEqual
                | Self::JumpIfNotLooselyEqual
                | Self::JumpIfLooselyEqual
                | Self::LoadConstantWide
                | Self::PushExceptionJumpTarget
                | Self::StoreConstantWide
        )
    }

    pub const fn has_cache_index(self) -> bool {
        matches!(
            self,
            Self::GetValueWithCache
                | Self::GetValueWithCacheKeepReference
                | Self::GetPropertyWithCache
                | Self::GetPropertyWithCacheKeepReference
                | Self::LoadPropertyWithCachePushReference
                | Self::LoadPropertyWithCachePushThis
                | Self::GetThisPropertyWithCache
                | Self::GetThisPropertyWithCacheKeepReference
                | Self::GetIndexPropertyWithCache
                | Self::GetIndexPropertyWithCacheKeepReference
                | Self::LoadIndexPropertyWithCachePushThis
                | Self::GetKeptPropertyWithCache
                | Self::PutPropertyWithCache
                | Self::PutValueWithCache
                | Self::StepValueWithCache
                | Self::ResolveBindingWithCache
        )
    }

    pub const fn has_constant_index(self) -> bool {
        matches!(
            self,
            Self::BindingPatternBindNamed
                | Self::BindingPatternBindToIndex
                | Self::BindingPatternGetValueNamed
                | Self::IsStrictlyEqualConstant
                | Self::LoadConstant
                | Self::StoreConstant
        )
    }

    pub const fn has_shape_index(self) -> bool {
        matches!(self, Self::ObjectCreateWithShape)
    }

    pub const fn has_identifier_index(self) -> bool {
        let base_matches = matches!(
            self,
            Self::BindingPatternBind
                | Self::BindingPatternBindNamed
                | Self::BindingPatternBindRest
                | Self::ClassDefinePrivateMethod
                | Self::ClassDefinePrivateProperty
                | Self::CreateImmutableBinding
                | Self::CreateMutableBinding
                | Self::EvaluatePropertyAccessWithIdentifierKey
                | Self::GetPropertyWithCache
                | Self::GetPropertyWithCacheKeepReference
                | Self::LoadPropertyWithCachePushReference
                | Self::LoadPropertyWithCachePushThis
                | Self::GetThisPropertyWithCache
                | Self::GetThisPropertyWithCacheKeepReference
                | Self::GetIndexPropertyWithCache
                | Self::GetIndexPropertyWithCacheKeepReference
                | Self::LoadIndexPropertyWithCachePushThis
                | Self::GetKeptPropertyWithCache
                | Self::PutPropertyWithCache
                | Self::MakePrivateReference
                | Self::MakeSuperPropertyReferenceWithIdentifierKey
                | Self::ResolveBinding
                | Self::ResolveBindingWithCache
                | Self::ResolveBindingAtSite
                | Self::GetBindingAtSite
                | Self::LoadBindingAtSite
                | Self::GetBindingAtSiteOnce
                | Self::LoadBindingAtSiteOnce
                | Self::PutBindingAtSite
                | Self::ResolveGlobalAtSite
                | Self::GetGlobalAtSite
                | Self::PutGlobalAtSite
                | Self::PutValueAtSite
                | Self::VerifyIsObject
        );

        #[cfg(feature = "typescript")]
        let typescript_matches = false;
        #[cfg(not(feature = "typescript"))]
        let typescript_matches = false;

        base_matches || typescript_matches
    }

    pub const fn has_function_expression_index(self) -> bool {
        matches!(
            self,
            Self::ClassDefineConstructor
                | Self::ClassDefinePrivateMethod
                | Self::InstantiateArrowFunctionExpression
                | Self::InstantiateOrdinaryFunctionExpression
                | Self::ObjectDefineGetter
                | Self::ObjectDefineMethod
                | Self::ObjectDefineSetter
        )
    }

    pub const fn has_jump_slot(self) -> bool {
        matches!(
            self,
            Self::IteratorComplete
                | Self::IteratorThrow
                | Self::IteratorReturn
                | Self::Jump
                | Self::JumpIfNot
                | Self::JumpIfTrue
                | Self::JumpIfNotLessThan
                | Self::JumpIfNotLessThanEquals
                | Self::JumpIfNotGreaterThan
                | Self::JumpIfNotGreaterThanEquals
                | Self::JumpIfLessThan
                | Self::JumpIfLessThanEquals
                | Self::JumpIfGreaterThan
                | Self::JumpIfGreaterThanEquals
                | Self::JumpIfNotStrictlyEqual
                | Self::JumpIfStrictlyEqual
                | Self::JumpIfNotLooselyEqual
                | Self::JumpIfLooselyEqual
                | Self::PushExceptionJumpTarget
                | Self::IteratorStepValue
        )
    }

    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// For an instruction that compares a local with a constant or another
    /// local and jumps: the instruction that makes the same comparison and
    /// jump on the stack and the result, and whether the second operand is
    /// a constant.
    #[inline(always)]
    pub(crate) const fn compared_local(self) -> Option<(Self, bool)> {
        Some(match self {
            Self::JumpIfIndexNotLessThanConstant => (Self::JumpIfNotLessThan, true),
            Self::JumpIfIndexNotLessThanIndex => (Self::JumpIfNotLessThan, false),
            Self::JumpIfIndexNotLessThanEqualsConstant => (Self::JumpIfNotLessThanEquals, true),
            Self::JumpIfIndexNotLessThanEqualsIndex => (Self::JumpIfNotLessThanEquals, false),
            Self::JumpIfIndexNotGreaterThanConstant => (Self::JumpIfNotGreaterThan, true),
            Self::JumpIfIndexNotGreaterThanIndex => (Self::JumpIfNotGreaterThan, false),
            Self::JumpIfIndexNotGreaterThanEqualsConstant => {
                (Self::JumpIfNotGreaterThanEquals, true)
            }
            Self::JumpIfIndexNotGreaterThanEqualsIndex => (Self::JumpIfNotGreaterThanEquals, false),
            Self::JumpIfIndexNotStrictlyEqualConstant => (Self::JumpIfNotStrictlyEqual, true),
            Self::JumpIfIndexNotStrictlyEqualIndex => (Self::JumpIfNotStrictlyEqual, false),
            Self::JumpIfIndexStrictlyEqualConstant => (Self::JumpIfStrictlyEqual, true),
            Self::JumpIfIndexStrictlyEqualIndex => (Self::JumpIfStrictlyEqual, false),
            _ => return None,
        })
    }

    /// For an instruction that compares a local with a constant or another
    /// local and jumps where the comparison holds: the step it first makes
    /// on the local (1, -1, or 0 for none), the comparison, and whether
    /// the second operand is a constant.
    #[inline(always)]
    pub(crate) const fn loop_test(self) -> Option<(i8, Self, bool)> {
        Some(match self {
            Self::JumpIfIndexLessThanConstant => (0, Self::LessThan, true),
            Self::JumpIfIndexLessThanIndex => (0, Self::LessThan, false),
            Self::JumpIfIndexLessThanEqualsConstant => (0, Self::LessThanEquals, true),
            Self::JumpIfIndexLessThanEqualsIndex => (0, Self::LessThanEquals, false),
            Self::JumpIfIndexGreaterThanConstant => (0, Self::GreaterThan, true),
            Self::JumpIfIndexGreaterThanIndex => (0, Self::GreaterThan, false),
            Self::JumpIfIndexGreaterThanEqualsConstant => (0, Self::GreaterThanEquals, true),
            Self::JumpIfIndexGreaterThanEqualsIndex => (0, Self::GreaterThanEquals, false),
            Self::IncrementIndexJumpIfLessThanConstant => (1, Self::LessThan, true),
            Self::IncrementIndexJumpIfLessThanIndex => (1, Self::LessThan, false),
            Self::IncrementIndexJumpIfLessThanEqualsConstant => (1, Self::LessThanEquals, true),
            Self::IncrementIndexJumpIfLessThanEqualsIndex => (1, Self::LessThanEquals, false),
            Self::DecrementIndexJumpIfGreaterThanConstant => (-1, Self::GreaterThan, true),
            Self::DecrementIndexJumpIfGreaterThanIndex => (-1, Self::GreaterThan, false),
            Self::DecrementIndexJumpIfGreaterThanEqualsConstant => {
                (-1, Self::GreaterThanEquals, true)
            }
            Self::DecrementIndexJumpIfGreaterThanEqualsIndex => {
                (-1, Self::GreaterThanEquals, false)
            }
            _ => return None,
        })
    }

    /// The operator that a fused instruction has as an argument: the number
    /// of an arithmetic or bitwise operator among them.
    #[inline(always)]
    pub(crate) const fn fused_operator(number: usize) -> Option<Self> {
        Some(match number {
            0 => Self::ApplyAdditionBinaryOperator,
            1 => Self::ApplySubtractionBinaryOperator,
            2 => Self::ApplyMultiplicationBinaryOperator,
            3 => Self::ApplyDivisionBinaryOperator,
            4 => Self::ApplyRemainderBinaryOperator,
            5 => Self::ApplyShiftLeftBinaryOperator,
            6 => Self::ApplyShiftRightBinaryOperator,
            7 => Self::ApplyShiftRightZeroFillBinaryOperator,
            8 => Self::ApplyBitwiseORBinaryOperator,
            9 => Self::ApplyBitwiseXORBinaryOperator,
            10 => Self::ApplyBitwiseAndBinaryOperator,
            _ => return None,
        })
    }

    /// The number that the fused instructions have for this operator.
    pub(crate) const fn fused_operator_number(self) -> Option<usize> {
        Some(match self {
            Self::ApplyAdditionBinaryOperator => 0,
            Self::ApplySubtractionBinaryOperator => 1,
            Self::ApplyMultiplicationBinaryOperator => 2,
            Self::ApplyDivisionBinaryOperator => 3,
            Self::ApplyRemainderBinaryOperator => 4,
            Self::ApplyShiftLeftBinaryOperator => 5,
            Self::ApplyShiftRightBinaryOperator => 6,
            Self::ApplyShiftRightZeroFillBinaryOperator => 7,
            Self::ApplyBitwiseORBinaryOperator => 8,
            Self::ApplyBitwiseXORBinaryOperator => 9,
            Self::ApplyBitwiseAndBinaryOperator => 10,
            _ => return None,
        })
    }

    /// The stack slots that the description of a fused operation can name:
    /// the bit above them says the result is pushed, and the number of the
    /// operator is above that.
    pub(crate) const FUSED_SLOTS: usize = 1 << 11;

    /// In the description of a fused operation: its result is pushed onto
    /// the stack.
    pub(crate) const FUSED_PUSHES: usize = 1 << 11;

    /// The description of a fused operation for a stack slot and the number
    /// of an operator, with a result that is not pushed.
    pub(crate) const fn fused_slot_with_operator(slot: usize, operator: usize) -> usize {
        debug_assert!(slot < Self::FUSED_SLOTS && operator < 16);
        slot | operator << 12
    }

    /// The stack slot and the number of the operator in such a description.
    #[inline(always)]
    pub(crate) const fn fused_slot_and_operator(description: usize) -> (usize, usize) {
        (description & (Self::FUSED_SLOTS - 1), description >> 12)
    }

    /// Whether such a description says the result is pushed.
    #[inline(always)]
    pub(crate) const fn fused_pushes(description: usize) -> bool {
        description & Self::FUSED_PUSHES != 0
    }

    /// The instruction that makes the step `step` on a local, compares it by
    /// `self` (LessThan and its like) with a constant or another local, and
    /// jumps where the comparison holds.
    pub(crate) const fn as_loop_test(self, step: i8, constant: bool) -> Option<Self> {
        Some(match (step, self, constant) {
            (0, Self::LessThan, true) => Self::JumpIfIndexLessThanConstant,
            (0, Self::LessThan, false) => Self::JumpIfIndexLessThanIndex,
            (0, Self::LessThanEquals, true) => Self::JumpIfIndexLessThanEqualsConstant,
            (0, Self::LessThanEquals, false) => Self::JumpIfIndexLessThanEqualsIndex,
            (0, Self::GreaterThan, true) => Self::JumpIfIndexGreaterThanConstant,
            (0, Self::GreaterThan, false) => Self::JumpIfIndexGreaterThanIndex,
            (0, Self::GreaterThanEquals, true) => Self::JumpIfIndexGreaterThanEqualsConstant,
            (0, Self::GreaterThanEquals, false) => Self::JumpIfIndexGreaterThanEqualsIndex,
            (1, Self::LessThan, true) => Self::IncrementIndexJumpIfLessThanConstant,
            (1, Self::LessThan, false) => Self::IncrementIndexJumpIfLessThanIndex,
            (1, Self::LessThanEquals, true) => Self::IncrementIndexJumpIfLessThanEqualsConstant,
            (1, Self::LessThanEquals, false) => Self::IncrementIndexJumpIfLessThanEqualsIndex,
            (-1, Self::GreaterThan, true) => Self::DecrementIndexJumpIfGreaterThanConstant,
            (-1, Self::GreaterThan, false) => Self::DecrementIndexJumpIfGreaterThanIndex,
            (-1, Self::GreaterThanEquals, true) => {
                Self::DecrementIndexJumpIfGreaterThanEqualsConstant
            }
            (-1, Self::GreaterThanEquals, false) => {
                Self::DecrementIndexJumpIfGreaterThanEqualsIndex
            }
            _ => return None,
        })
    }

    /// Which comparison [`Instruction::JumpIfResultComparesWithConstant`]
    /// makes and which way it jumps, as its first argument has it: for the
    /// instruction `self` that makes the same comparison and jump on the
    /// stack and the result, or, where `self` is a comparison alone
    /// (LessThan and its like), for the jump where that holds. The lowest
    /// two bits are the order, [`Instruction::RESULT_COMPARISON_NOT`] says
    /// that it jumps where the comparison does not hold, and from
    /// [`Instruction::RESULT_COMPARISON_STRICT`] on it is strict equality:
    /// jump where equal, and one more where not.
    pub(crate) const fn result_comparison(self) -> Option<IndexType> {
        Some(match self {
            Self::LessThan | Self::JumpIfLessThan => 0,
            Self::LessThanEquals | Self::JumpIfLessThanEquals => 1,
            Self::GreaterThan | Self::JumpIfGreaterThan => 2,
            Self::GreaterThanEquals | Self::JumpIfGreaterThanEquals => 3,
            Self::JumpIfNotLessThan => 4,
            Self::JumpIfNotLessThanEquals => 5,
            Self::JumpIfNotGreaterThan => 6,
            Self::JumpIfNotGreaterThanEquals => 7,
            Self::JumpIfStrictlyEqual => 8,
            Self::JumpIfNotStrictlyEqual => 9,
            _ => return None,
        })
    }

    /// See [`Instruction::result_comparison`].
    pub(crate) const RESULT_COMPARISON_NOT: usize = 4;
    /// See [`Instruction::result_comparison`].
    pub(crate) const RESULT_COMPARISON_STRICT: usize = 8;

    /// In the second argument of [`Instruction::StepValueWithCache`]: the
    /// step is `--`.
    pub(crate) const STEP_DOWN: usize = 1;
    /// There: the new value is left in the result register (`--o.p` whose
    /// value is used).
    pub(crate) const STEP_KEEPS_NEW: usize = 2;
    /// There: the value before the step, as a Number, is left in the result
    /// register (`o.p--` whose value is used).
    pub(crate) const STEP_KEEPS_OLD: usize = 4;

    /// The instruction that makes the comparison and jump of `self` on a
    /// local and a constant, or on two locals.
    pub(crate) const fn with_compared_local(self, constant: bool) -> Option<Self> {
        Some(match (self, constant) {
            (Self::JumpIfNotLessThan, true) => Self::JumpIfIndexNotLessThanConstant,
            (Self::JumpIfNotLessThan, false) => Self::JumpIfIndexNotLessThanIndex,
            (Self::JumpIfNotLessThanEquals, true) => Self::JumpIfIndexNotLessThanEqualsConstant,
            (Self::JumpIfNotLessThanEquals, false) => Self::JumpIfIndexNotLessThanEqualsIndex,
            (Self::JumpIfNotGreaterThan, true) => Self::JumpIfIndexNotGreaterThanConstant,
            (Self::JumpIfNotGreaterThan, false) => Self::JumpIfIndexNotGreaterThanIndex,
            (Self::JumpIfNotGreaterThanEquals, true) => {
                Self::JumpIfIndexNotGreaterThanEqualsConstant
            }
            (Self::JumpIfNotGreaterThanEquals, false) => Self::JumpIfIndexNotGreaterThanEqualsIndex,
            (Self::JumpIfNotStrictlyEqual, true) => Self::JumpIfIndexNotStrictlyEqualConstant,
            (Self::JumpIfNotStrictlyEqual, false) => Self::JumpIfIndexNotStrictlyEqualIndex,
            (Self::JumpIfStrictlyEqual, true) => Self::JumpIfIndexStrictlyEqualConstant,
            (Self::JumpIfStrictlyEqual, false) => Self::JumpIfIndexStrictlyEqualIndex,
            _ => return None,
        })
    }
}

const ARGUMENT_BYTES: [u8; 256] = {
    let mut table = [0; 256];
    let mut byte = 0;
    while byte < 256 {
        if let Some(kind) = Instruction::from_u8(byte as u8) {
            table[byte] = kind.argument_count() * 2;
        }
        byte += 1;
    }
    table
};

#[derive(Clone, Copy)]
struct InstructionArgs([u16; 2]);

impl InstructionArgs {
    const NONE: Self = Self([0, 0]);

    fn single_arg(self) -> u16 {
        self.0[0]
    }

    fn two_args(self) -> [u16; 2] {
        self.0
    }

    fn double_arg(self) -> u32 {
        let [a, b] = self.0[0].to_ne_bytes();
        let [c, d] = self.0[1].to_ne_bytes();
        u32::from_ne_bytes([a, b, c, d])
    }
}

impl core::fmt::Debug for InstructionArgs {
    fn fmt(&self, _: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Instr {
    pub kind: Instruction,
    args: InstructionArgs,
}

impl Instr {
    #[inline(always)]
    pub(super) fn consume_instruction(instructions: &[u8], ip: &mut usize) -> Option<Instr> {
        let cur_ip = *ip;
        let &byte = instructions.get(cur_ip)?;
        let Some(kind) = Instruction::from_u8(byte) else {
            panic_invalid_instruction()
        };
        if kind.is_unused() {
            panic_invalid_instruction()
        }
        let argument_bytes = ARGUMENT_BYTES[byte as usize] as usize;
        let args_ip = cur_ip + 1;
        *ip = args_ip + argument_bytes;
        // The four bytes after the instruction, whichever of them are its
        // arguments: an instruction reads only the arguments it has.
        let Some(&[a, b, c, d]) = instructions
            .get(args_ip..)
            .and_then(|bytes| bytes.first_chunk::<4>())
        else {
            // The Return that ends a function has nothing after it.
            if argument_bytes == 0 {
                return Some(Instr::new(kind));
            }
            return Some(Self::consume_short_arguments(kind, instructions, args_ip));
        };
        Some(Instr {
            kind,
            args: InstructionArgs([u16::from_ne_bytes([a, b]), u16::from_ne_bytes([c, d])]),
        })
    }

    /// How many bytes the loop of a `Vm` reads for an instruction with one
    /// look at how much code is left: the longest instruction, of four
    /// arguments.
    pub(crate) const WINDOW: usize = 9;

    /// The instruction that `window` starts with: its byte and the four
    /// bytes after it, whichever of those are its arguments. The same
    /// [`Instr`] as [`Instr::consume_instruction`] reads there, where the
    /// code has that many bytes left. The third and the fourth argument of
    /// an instruction with four are the rest of the window
    /// ([`Instr::window_third_index`], [`Instr::window_jump_target`]).
    #[inline(always)]
    pub(super) fn in_window(window: &[u8; Self::WINDOW]) -> Instr {
        let [byte, a, b, c, d, ..] = *window;
        let Some(kind) = Instruction::from_u8(byte) else {
            panic_invalid_instruction()
        };
        Instr {
            kind,
            args: InstructionArgs([u16::from_ne_bytes([a, b]), u16::from_ne_bytes([c, d])]),
        }
    }

    #[cold]
    #[inline(never)]
    fn consume_short_arguments(kind: Instruction, instructions: &[u8], args_ip: usize) -> Instr {
        let bytes = instructions.get(args_ip..).unwrap_or(&[]);
        match kind.argument_count() {
            0 => Instr::new(kind),
            1 => {
                let Some(arg_slice) = bytes.first_chunk::<2>() else {
                    panic_invalid_instruction()
                };
                Instr::new_with_arg(kind, IndexType::from_ne_bytes(*arg_slice))
            }
            _ => panic_invalid_instruction(),
        }
    }

    pub(crate) fn new(kind: Instruction) -> Self {
        Self {
            kind,
            args: InstructionArgs::NONE,
        }
    }

    pub(crate) fn new_with_arg(kind: Instruction, arg: u16) -> Self {
        Self {
            kind,
            args: InstructionArgs([arg, 0]),
        }
    }

    #[inline(always)]
    pub(crate) fn get_first_arg(&self) -> IndexType {
        debug_assert!(
            self.kind.argument_count() == 1
                || self.kind.argument_count() == 4
                || (self.kind.argument_count() == 2 && !self.kind.has_double_arg())
        );
        self.args.single_arg()
    }

    #[inline(always)]
    pub(crate) fn get_first_index(&self) -> usize {
        self.get_first_arg() as usize
    }

    #[inline(always)]
    pub(crate) fn get_second_index(&self) -> usize {
        debug_assert!(
            self.kind.argument_count() == 4
                || (self.kind.argument_count() == 2 && !self.kind.has_double_arg())
        );
        self.args.two_args()[1] as usize
    }

    /// The third argument of an instruction with four, where it is an
    /// index, from the window the instruction was read in: nothing is asked
    /// of the code again, where reading it from the code by the place of
    /// the next instruction asked twice how much code there is, eight
    /// machine instructions for the one load this is.
    #[inline(always)]
    pub(crate) fn window_third_index(window: &[u8; Self::WINDOW]) -> usize {
        u16::from_ne_bytes([window[5], window[6]]) as usize
    }

    /// The jump target of an instruction with four arguments, which is its
    /// last two, from its window.
    #[inline(always)]
    pub(crate) fn window_jump_target(window: &[u8; Self::WINDOW]) -> usize {
        u32::from_ne_bytes([window[5], window[6], window[7], window[8]]) as usize
    }

    /// The third argument of an instruction with four, where it is an index:
    /// the first two of the four bytes before `ip`, where the next
    /// instruction is.
    #[inline(always)]
    pub(crate) fn wide_third_index(instructions: &[u8], ip: usize) -> usize {
        let Some(bytes) = ip
            .checked_sub(4)
            .and_then(|start| instructions.get(start..ip))
            .and_then(|bytes| bytes.first_chunk::<2>())
        else {
            panic_invalid_instruction()
        };
        u16::from_ne_bytes(*bytes) as usize
    }

    /// The fourth argument of an instruction with four, where it is an
    /// index: the two bytes before `ip`, where the next instruction is.
    #[inline(always)]
    pub(crate) fn wide_fourth_index(instructions: &[u8], ip: usize) -> usize {
        let Some(bytes) = ip
            .checked_sub(2)
            .and_then(|start| instructions.get(start..ip))
            .and_then(|bytes| bytes.first_chunk::<2>())
        else {
            panic_invalid_instruction()
        };
        u16::from_ne_bytes(*bytes) as usize
    }

    /// The jump target of an instruction with four arguments, which is its
    /// last two: the four bytes before `ip`, where the next instruction is.
    #[inline(always)]
    pub(crate) fn wide_jump_target(instructions: &[u8], ip: usize) -> usize {
        let Some(bytes) = ip
            .checked_sub(4)
            .and_then(|start| instructions.get(start..ip))
            .and_then(|bytes| bytes.first_chunk::<4>())
        else {
            panic_invalid_instruction()
        };
        u32::from_ne_bytes(*bytes) as usize
    }

    pub(crate) fn get_first_bool(&self) -> bool {
        let first_arg = match self.kind.argument_count() {
            1 => self.args.single_arg(),
            2 => {
                debug_assert!(!self.kind.has_double_arg());
                self.args.two_args()[1]
            }
            _ => panic_not_two_arguments(),
        };
        if first_arg == 0 || first_arg == 1 {
            first_arg == 1
        } else {
            panic_first_argument_not_boolean()
        }
    }

    pub(crate) fn get_second_bool(&self) -> bool {
        match self.kind.argument_count() {
            2 => {
                debug_assert!(!self.kind.has_double_arg());
                let second_arg = self.args.two_args()[1];
                if second_arg == 0 || second_arg == 1 {
                    second_arg == 1
                } else {
                    panic_second_argument_not_boolean()
                }
            }
            _ => panic_no_argument(),
        }
    }

    /// The 32-bit index of a wide constant instruction.
    pub(crate) fn get_wide_index(&self) -> usize {
        debug_assert!(matches!(
            self.kind,
            Instruction::LoadConstantWide | Instruction::StoreConstantWide
        ));
        self.args.double_arg() as usize
    }

    pub(crate) fn get_jump_slot(&self) -> usize {
        debug_assert!(self.kind.has_jump_slot());
        debug_assert_eq!(self.kind.argument_count(), 2);
        let jump = self.args.double_arg();
        jump as usize
    }

    pub(crate) fn debug_print(
        &self,
        agent: &mut Agent,
        ip: usize,
        exe: Scoped<Executable>,
        gc: NoGcScope,
    ) {
        match self.kind.argument_count() {
            0 => {
                eprintln!("  {}: {:?}", ip, self.kind);
            }
            1 => {
                let arg0 = self.args.single_arg();
                eprintln!(
                    "  {}: {:?}({})",
                    ip,
                    self.kind,
                    Self::print_single_arg(agent, self.kind, arg0, exe, gc)
                );
            }
            2 => {
                if self.kind.has_jump_slot() {
                    let jump_slot = self.get_jump_slot();
                    eprintln!("  {}: {:?}({})", ip, self.kind, jump_slot);
                } else {
                    let [arg0, arg1] = self.args.two_args();
                    eprintln!(
                        "  {}: {:?}({})",
                        ip,
                        self.kind,
                        Self::print_two_args(agent, self.kind, arg0, arg1, exe, gc)
                    );
                }
            }
            4 if matches!(
                self.kind,
                Instruction::GetIndexPropertyWithCache
                    | Instruction::GetIndexPropertyWithCacheKeepReference
                    | Instruction::LoadIndexPropertyWithCachePushThis
            ) =>
            {
                let [arg0, arg1] = self.args.two_args();
                let instructions = exe.get_instructions(agent);
                let slot = Self::wide_third_index(instructions.code(), ip + 9);
                let key = debug_print_identifier(agent, exe, arg0 as usize, gc);
                eprintln!("  {}: {:?}({}, {}, {})", ip, self.kind, slot, key, arg1);
            }
            4 if self.kind == Instruction::GetKeptPropertyWithCache => {
                let [arg0, arg1] = self.args.two_args();
                let instructions = exe.get_instructions(agent);
                let slot = Self::wide_third_index(instructions.code(), ip + 9);
                let site = Self::wide_fourth_index(instructions.code(), ip + 9);
                let key = debug_print_identifier(agent, exe, arg0 as usize, gc);
                eprintln!(
                    "  {}: {:?}({}, {}, of site {} kept in {})",
                    ip, self.kind, key, arg1, site, slot
                );
            }
            4 if matches!(
                self.kind,
                Instruction::GetBindingAtSiteOnce | Instruction::LoadBindingAtSiteOnce
            ) =>
            {
                let [arg0, arg1] = self.args.two_args();
                let instructions = exe.get_instructions(agent);
                let slot = Self::wide_third_index(instructions.code(), ip + 9);
                let key = debug_print_identifier(agent, exe, arg0 as usize, gc);
                eprintln!(
                    "  {}: {:?}({}, {}, kept in {})",
                    ip, self.kind, key, arg1, slot
                );
            }
            4 => {
                let [arg0, arg1] = self.args.two_args();
                let instructions = exe.get_instructions(agent);
                let target = Self::wide_jump_target(instructions.code(), ip + 9);
                let constant = matches!(self.kind.compared_local(), Some((_, true)))
                    || matches!(self.kind.loop_test(), Some((_, _, true)));
                let operand = if constant {
                    debug_print_constant(agent, exe, arg1 as usize, gc)
                } else {
                    format!("{arg1}")
                };
                eprintln!(
                    "  {}: {:?}({}, {}, {})",
                    ip, self.kind, arg0, operand, target
                );
            }
            _ => unreachable!(),
        }
    }

    /// The operator in the description of a fused operation, and whether
    /// its result is pushed.
    fn print_operation(description: usize) -> std::string::String {
        let (_, operator) = Instruction::fused_slot_and_operator(description);
        let pushed = if Instruction::fused_pushes(description) {
            ", pushed"
        } else {
            ""
        };
        match Instruction::fused_operator(operator) {
            Some(kind) => format!("{kind:?}{pushed}"),
            None => format!("unknown operator {operator}{pushed}"),
        }
    }

    fn print_single_arg(
        agent: &mut Agent,
        kind: Instruction,
        arg: IndexType,
        exe: Scoped<Executable>,
        gc: NoGcScope,
    ) -> std::string::String {
        let index = arg as usize;
        debug_assert!(kind.argument_count() == 1);
        if kind.has_constant_index() {
            debug_print_constant(agent, exe, index, gc)
        } else if kind.has_jump_slot() {
            arg.to_string()
        } else if kind.has_identifier_index() {
            debug_print_identifier(agent, exe, index, gc)
        } else if kind.has_function_expression_index() {
            if kind == Instruction::InstantiateArrowFunctionExpression {
                let expr = exe.fetch_arrow_function_expression(agent, index, gc);
                format!("arrow function {:?}", expr.template.span)
            } else {
                let expr = exe.fetch_function_expression(agent, index, gc);
                let name = expr
                    .binding_name
                    .map(|name| name.to_string_lossy_(agent).into_owned());
                format!(
                    "function {} {:?}",
                    name.as_deref().unwrap_or("anonymous"),
                    expr.template.span
                )
            }
        } else if kind == Instruction::ApplyBinaryOperatorAndMove {
            let (slot, _) = Instruction::fused_slot_and_operator(index);
            if Instruction::fused_pushes(index) {
                Self::print_operation(index)
            } else {
                format!("{}, to {slot}", Self::print_operation(index))
            }
        } else if kind == Instruction::ClassDefineDefaultConstructor {
            if exe.fetch_class_initializer_bytecode(agent, index, gc).1 {
                "{ super() }".to_string()
            } else {
                "{}".to_string()
            }
        } else {
            // Immediate
            arg.to_string()
        }
    }

    fn print_two_args(
        agent: &mut Agent,
        kind: Instruction,
        arg0: IndexType,
        arg1: IndexType,
        exe: Scoped<Executable>,
        gc: NoGcScope,
    ) -> std::string::String {
        match kind {
            Instruction::BeginSimpleArrayBindingPattern => {
                let arg1_equals = arg1 == 1;
                format!("{{ length: {arg0}, env: {arg1_equals} }}")
            }
            Instruction::BindArguments => {
                format!("{{ first slot: {arg0}, count: {arg1} }}")
            }
            Instruction::JumpToCase => {
                format!("{{ slot: {arg0}, table: {arg1} }}")
            }
            Instruction::LoadFromIndexStoreConstant => {
                format!(
                    "{arg0}, {}",
                    debug_print_constant(agent, exe.clone(), arg1 as usize, gc)
                )
            }
            Instruction::LoadFromIndexGetFromIndex | Instruction::LoadFromIndexLoadFromIndex => {
                format!("{arg0}, {arg1}")
            }
            Instruction::ApplyBinaryOperatorWithConstant => {
                format!(
                    "{}, {}",
                    debug_print_constant(agent, exe.clone(), arg0 as usize, gc),
                    Self::print_operation(arg1 as usize)
                )
            }
            Instruction::ApplyBinaryOperatorToIndexWithConstant => {
                let (slot, _) = Instruction::fused_slot_and_operator(arg0 as usize);
                format!(
                    "{slot}, {}, {}",
                    debug_print_constant(agent, exe.clone(), arg1 as usize, gc),
                    Self::print_operation(arg0 as usize)
                )
            }
            Instruction::BindingPatternBindNamed => {
                format!(
                    "{{ {}: {} }}",
                    debug_print_constant(agent, exe.clone(), arg1 as usize, gc),
                    debug_print_identifier(agent, exe, arg0 as usize, gc)
                )
            }
            Instruction::BindingPatternBindToIndex => {
                format!(
                    "{{ {}: stack[{}] }}",
                    debug_print_constant(agent, exe, arg1 as usize, gc),
                    arg0,
                )
            }
            Instruction::ClassDefineConstructor => {
                if arg1 == 1 {
                    "constructor() { super() }".to_string()
                } else {
                    "constructor()".to_string()
                }
            }
            Instruction::ClassDefinePrivateMethod => {
                let is_static = arg1 & 0b100 == 0b100;
                let static_prefix = if is_static { "static " } else { "" };
                let is_get = arg1 & 0b1 == 0b1;
                let is_set = arg1 & 0b10 == 0b10;
                let accessor_prefix = if is_get {
                    "get "
                } else if is_set {
                    "set "
                } else {
                    ""
                };
                format!("{static_prefix}{accessor_prefix} #function() {{}}")
            }
            Instruction::ClassDefinePrivateProperty => {
                let key = debug_print_identifier(agent, exe, arg0 as usize, gc);
                let is_static = arg1 != 0;
                let static_prefix = if is_static { "static " } else { "" };
                format!("{static_prefix}#{key}")
            }
            Instruction::InitializeVariableEnvironment => {
                let arg1_equals = arg1 == 1;
                format!("{{ var count: {arg0}, strict: {arg1_equals} }}")
            }
            Instruction::StepValueWithCache => {
                let step = if arg1 as usize & Instruction::STEP_DOWN != 0 {
                    "--"
                } else {
                    "++"
                };
                let keeps = if arg1 as usize & Instruction::STEP_KEEPS_NEW != 0 {
                    ", keeps the new value"
                } else if arg1 as usize & Instruction::STEP_KEEPS_OLD != 0 {
                    ", keeps the value before"
                } else {
                    ""
                };
                format!("{arg0}, {step}{keeps}")
            }
            Instruction::ObjectDefineGetter => "get function() {}".to_string(),
            Instruction::ObjectDefineMethod => "function() {}".to_string(),
            Instruction::ObjectDefineSetter => "set function() {}".to_string(),
            Instruction::ResolveBindingWithCache
            | Instruction::ResolveBindingAtSite
            | Instruction::GetBindingAtSite
            | Instruction::LoadBindingAtSite
            | Instruction::PutBindingAtSite
            | Instruction::ResolveGlobalAtSite
            | Instruction::GetGlobalAtSite
            | Instruction::PutGlobalAtSite
            | Instruction::PutValueAtSite
            | Instruction::GetPropertyWithCache
            | Instruction::GetPropertyWithCacheKeepReference
            | Instruction::LoadPropertyWithCachePushReference
            | Instruction::LoadPropertyWithCachePushThis
            | Instruction::GetThisPropertyWithCache
            | Instruction::GetThisPropertyWithCacheKeepReference
            | Instruction::PutPropertyWithCache => {
                let key = debug_print_identifier(agent, exe, arg0 as usize, gc);
                format!("{key}, {arg1}")
            }
            _ => unreachable!("{kind:?}"),
        }
    }
}

fn debug_print_constant(
    agent: &mut Agent,
    exe: Scoped<Executable>,
    index: usize,
    gc: NoGcScope,
) -> std::string::String {
    let constant = exe.fetch_constant(agent, index, gc);
    if let Ok(string_constant) = String::try_from(constant) {
        format!("\"{}\"", string_constant.to_string_lossy_(agent))
    } else {
        constant
            .try_string_repr(agent, gc)
            .to_string_lossy_(agent)
            .to_string()
    }
}

fn debug_print_identifier(
    agent: &Agent,
    exe: Scoped<Executable>,
    index: usize,
    gc: NoGcScope,
) -> std::string::String {
    let identifier = exe.fetch_property_key(agent, index, gc);
    identifier.as_display(agent).to_string()
}

#[derive(Debug)]
pub(crate) struct InstructionIter<'a> {
    instructions: &'a [u8],
    pub(crate) index: usize,
}

impl<'a> InstructionIter<'a> {
    pub(crate) fn new(instructions: &'a [u8]) -> Self {
        Self {
            instructions,
            index: 0,
        }
    }
}

impl Iterator for InstructionIter<'_> {
    type Item = (usize, Instr);

    fn next(&mut self) -> Option<Self::Item> {
        let index = self.index;
        let instr = Instr::consume_instruction(self.instructions, &mut self.index)?;

        Some((index, instr))
    }
}

impl TryFrom<u8> for Instruction {
    type Error = ();

    #[inline]
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match Self::from_u8(value) {
            Some(kind) if !kind.is_unused() => Ok(kind),
            _ => Err(()),
        }
    }
}

impl Instruction {
    #[inline(always)]
    const fn from_u8(value: u8) -> Option<Self> {
        const APPLYADDITIONBINARYOPERATOR: u8 = Instruction::ApplyAdditionBinaryOperator.as_u8();
        const APPLYSUBTRACTIONBINARYOPERATOR: u8 =
            Instruction::ApplySubtractionBinaryOperator.as_u8();
        const APPLYMULTIPLICATIONBINARYOPERATOR: u8 =
            Instruction::ApplyMultiplicationBinaryOperator.as_u8();
        const APPLYDIVISIONBINARYOPERATOR: u8 = Instruction::ApplyDivisionBinaryOperator.as_u8();
        const APPLYREMAINDERBINARYOPERATOR: u8 = Instruction::ApplyRemainderBinaryOperator.as_u8();
        const APPLYEXPONENTIALBINARYOPERATOR: u8 =
            Instruction::ApplyExponentialBinaryOperator.as_u8();
        const APPLYSHIFTLEFTBINARYOPERATOR: u8 = Instruction::ApplyShiftLeftBinaryOperator.as_u8();
        const APPLYSHIFTRIGHTBINARYOPERATOR: u8 =
            Instruction::ApplyShiftRightBinaryOperator.as_u8();
        const APPLYSHIFTRIGHTZEROFILLBINARYOPERATOR: u8 =
            Instruction::ApplyShiftRightZeroFillBinaryOperator.as_u8();
        const APPLYBITWISEORBINARYOPERATOR: u8 = Instruction::ApplyBitwiseORBinaryOperator.as_u8();
        const APPLYBITWISEXORBINARYOPERATOR: u8 =
            Instruction::ApplyBitwiseXORBinaryOperator.as_u8();
        const APPLYBITWISEANDBINARYOPERATOR: u8 =
            Instruction::ApplyBitwiseAndBinaryOperator.as_u8();
        const DEBUG: u8 = Instruction::Debug.as_u8();
        const GETKEPTPROPERTYWITHCACHE: u8 = Instruction::GetKeptPropertyWithCache.as_u8();
        const JUMPIFRESULTCOMPARESWITHCONSTANT: u8 =
            Instruction::JumpIfResultComparesWithConstant.as_u8();
        const STEPVALUEWITHCACHE: u8 = Instruction::StepValueWithCache.as_u8();
        const UNUSED234: u8 = Instruction::Unused234.as_u8();
        const UNUSED235: u8 = Instruction::Unused235.as_u8();
        const UNUSED236: u8 = Instruction::Unused236.as_u8();
        const UNUSED237: u8 = Instruction::Unused237.as_u8();
        const UNUSED238: u8 = Instruction::Unused238.as_u8();
        const UNUSED239: u8 = Instruction::Unused239.as_u8();
        const UNUSED240: u8 = Instruction::Unused240.as_u8();
        const UNUSED241: u8 = Instruction::Unused241.as_u8();
        const UNUSED242: u8 = Instruction::Unused242.as_u8();
        const UNUSED243: u8 = Instruction::Unused243.as_u8();
        const UNUSED244: u8 = Instruction::Unused244.as_u8();
        const UNUSED245: u8 = Instruction::Unused245.as_u8();
        const UNUSED246: u8 = Instruction::Unused246.as_u8();
        const UNUSED247: u8 = Instruction::Unused247.as_u8();
        const UNUSED248: u8 = Instruction::Unused248.as_u8();
        const UNUSED249: u8 = Instruction::Unused249.as_u8();
        const UNUSED250: u8 = Instruction::Unused250.as_u8();
        const UNUSED251: u8 = Instruction::Unused251.as_u8();
        const UNUSED252: u8 = Instruction::Unused252.as_u8();
        const UNUSED253: u8 = Instruction::Unused253.as_u8();
        const UNUSED254: u8 = Instruction::Unused254.as_u8();
        const UNUSED255: u8 = Instruction::Unused255.as_u8();
        const ARRAYCREATE: u8 = Instruction::ArrayCreate.as_u8();
        const ARRAYFROMSTACK: u8 = Instruction::ArrayFromStack.as_u8();
        const ARRAYPUSH: u8 = Instruction::ArrayPush.as_u8();
        const ARRAYELISION: u8 = Instruction::ArrayElision.as_u8();
        const AWAIT: u8 = Instruction::Await.as_u8();
        const BITWISENOT: u8 = Instruction::BitwiseNot.as_u8();
        const CREATEUNMAPPEDARGUMENTSOBJECT: u8 =
            Instruction::CreateUnmappedArgumentsObject.as_u8();
        const COPYDATAPROPERTIES: u8 = Instruction::CopyDataProperties.as_u8();
        const COPYDATAPROPERTIESINTOOBJECT: u8 = Instruction::CopyDataPropertiesIntoObject.as_u8();
        const DELETE: u8 = Instruction::Delete.as_u8();
        const DIRECTEVALCALL: u8 = Instruction::DirectEvalCall.as_u8();
        const EVALUATECALL: u8 = Instruction::EvaluateCall.as_u8();
        const LOADFROMINDEX: u8 = Instruction::LoadFromIndex.as_u8();
        const GETPROPERTYWITHCACHE: u8 = Instruction::GetPropertyWithCache.as_u8();
        const GETPROPERTYWITHCACHEKEEPREFERENCE: u8 =
            Instruction::GetPropertyWithCacheKeepReference.as_u8();
        const GETTHISPROPERTYWITHCACHE: u8 = Instruction::GetThisPropertyWithCache.as_u8();
        const GETTHISPROPERTYWITHCACHEKEEPREFERENCE: u8 =
            Instruction::GetThisPropertyWithCacheKeepReference.as_u8();
        const GETINDEXPROPERTYWITHCACHE: u8 = Instruction::GetIndexPropertyWithCache.as_u8();
        const GETINDEXPROPERTYWITHCACHEKEEPREFERENCE: u8 =
            Instruction::GetIndexPropertyWithCacheKeepReference.as_u8();
        const LOADINDEXPROPERTYWITHCACHEPUSHTHIS: u8 =
            Instruction::LoadIndexPropertyWithCachePushThis.as_u8();
        const PUTPROPERTYWITHCACHE: u8 = Instruction::PutPropertyWithCache.as_u8();
        const PUSHREFERENCEANDLOAD: u8 = Instruction::PushReferenceAndLoad.as_u8();
        const POPREFERENCEANDEVALUATECALL: u8 = Instruction::PopReferenceAndEvaluateCall.as_u8();
        const EVALUATECALLONTHIS: u8 = Instruction::EvaluateCallOnThis.as_u8();
        const ISSTRICTLYEQUALCONSTANT: u8 = Instruction::IsStrictlyEqualConstant.as_u8();
        const EVALUATENEW: u8 = Instruction::EvaluateNew.as_u8();
        const EVALUATESUPER: u8 = Instruction::EvaluateSuper.as_u8();
        const EVALUATEPROPERTYACCESSWITHEXPRESSIONKEY: u8 =
            Instruction::EvaluatePropertyAccessWithExpressionKey.as_u8();
        const EVALUATEPROPERTYACCESSWITHIDENTIFIERKEY: u8 =
            Instruction::EvaluatePropertyAccessWithIdentifierKey.as_u8();
        const MAKEPRIVATEREFERENCE: u8 = Instruction::MakePrivateReference.as_u8();
        const MAKESUPERPROPERTYREFERENCEWITHEXPRESSIONKEY: u8 =
            Instruction::MakeSuperPropertyReferenceWithExpressionKey.as_u8();
        const MAKESUPERPROPERTYREFERENCEWITHIDENTIFIERKEY: u8 =
            Instruction::MakeSuperPropertyReferenceWithIdentifierKey.as_u8();
        const GETVALUE: u8 = Instruction::GetValue.as_u8();
        const GETVALUEWITHCACHE: u8 = Instruction::GetValueWithCache.as_u8();
        const GETVALUEKEEPREFERENCE: u8 = Instruction::GetValueKeepReference.as_u8();
        const GETVALUEWITHCACHEKEEPREFERENCE: u8 =
            Instruction::GetValueWithCacheKeepReference.as_u8();
        const GREATERTHAN: u8 = Instruction::GreaterThan.as_u8();
        const GREATERTHANEQUALS: u8 = Instruction::GreaterThanEquals.as_u8();
        const HASPROPERTY: u8 = Instruction::HasProperty.as_u8();
        const HASPRIVATEELEMENT: u8 = Instruction::HasPrivateElement.as_u8();
        const INCREMENT: u8 = Instruction::Increment.as_u8();
        const DECREMENT: u8 = Instruction::Decrement.as_u8();
        const INSTANCEOFOPERATOR: u8 = Instruction::InstanceofOperator.as_u8();
        const INSTANTIATEARROWFUNCTIONEXPRESSION: u8 =
            Instruction::InstantiateArrowFunctionExpression.as_u8();
        const INSTANTIATEORDINARYFUNCTIONEXPRESSION: u8 =
            Instruction::InstantiateOrdinaryFunctionExpression.as_u8();
        const CLASSDEFINECONSTRUCTOR: u8 = Instruction::ClassDefineConstructor.as_u8();
        const CLASSDEFINEDEFAULTCONSTRUCTOR: u8 =
            Instruction::ClassDefineDefaultConstructor.as_u8();
        const CLASSDEFINEPRIVATEMETHOD: u8 = Instruction::ClassDefinePrivateMethod.as_u8();
        const CLASSDEFINEPRIVATEPROPERTY: u8 = Instruction::ClassDefinePrivateProperty.as_u8();
        const CLASSINITIALIZEPRIVATEELEMENTS: u8 =
            Instruction::ClassInitializePrivateElements.as_u8();
        const PUTPRIVATEVALUE: u8 = Instruction::ClassInitializePrivateValue.as_u8();
        const ISLOOSELYEQUAL: u8 = Instruction::IsLooselyEqual.as_u8();
        const ISSTRICTLYEQUAL: u8 = Instruction::IsStrictlyEqual.as_u8();
        const ISNULLORUNDEFINED: u8 = Instruction::IsNullOrUndefined.as_u8();
        const ISNULL: u8 = Instruction::IsNull.as_u8();
        const ISUNDEFINED: u8 = Instruction::IsUndefined.as_u8();
        const ISOBJECT: u8 = Instruction::IsObject.as_u8();
        const ISCONSTRUCTOR: u8 = Instruction::IsConstructor.as_u8();
        const JUMP: u8 = Instruction::Jump.as_u8();
        const JUMPIFNOT: u8 = Instruction::JumpIfNot.as_u8();
        const JUMPIFTRUE: u8 = Instruction::JumpIfTrue.as_u8();
        const GETVALUEWITHEXPRESSIONKEY: u8 = Instruction::GetValueWithExpressionKey.as_u8();
        const PUTVALUEWITHEXPRESSIONKEY: u8 = Instruction::PutValueWithExpressionKey.as_u8();
        const LOADFROMINDEXSTORECONSTANT: u8 = Instruction::LoadFromIndexStoreConstant.as_u8();
        const LOADFROMINDEXGETFROMINDEX: u8 = Instruction::LoadFromIndexGetFromIndex.as_u8();
        const LOADFROMINDEXLOADFROMINDEX: u8 = Instruction::LoadFromIndexLoadFromIndex.as_u8();
        const JUMPIFNOTLESSTHAN: u8 = Instruction::JumpIfNotLessThan.as_u8();
        const JUMPIFNOTLESSTHANEQUALS: u8 = Instruction::JumpIfNotLessThanEquals.as_u8();
        const JUMPIFNOTGREATERTHAN: u8 = Instruction::JumpIfNotGreaterThan.as_u8();
        const JUMPIFNOTGREATERTHANEQUALS: u8 = Instruction::JumpIfNotGreaterThanEquals.as_u8();
        const JUMPIFLESSTHAN: u8 = Instruction::JumpIfLessThan.as_u8();
        const JUMPIFLESSTHANEQUALS: u8 = Instruction::JumpIfLessThanEquals.as_u8();
        const JUMPIFGREATERTHAN: u8 = Instruction::JumpIfGreaterThan.as_u8();
        const JUMPIFGREATERTHANEQUALS: u8 = Instruction::JumpIfGreaterThanEquals.as_u8();
        const JUMPIFNOTSTRICTLYEQUAL: u8 = Instruction::JumpIfNotStrictlyEqual.as_u8();
        const JUMPIFSTRICTLYEQUAL: u8 = Instruction::JumpIfStrictlyEqual.as_u8();
        const JUMPIFNOTLOOSELYEQUAL: u8 = Instruction::JumpIfNotLooselyEqual.as_u8();
        const JUMPIFLOOSELYEQUAL: u8 = Instruction::JumpIfLooselyEqual.as_u8();
        const JUMPIFINDEXNOTLESSTHANCONSTANT: u8 =
            Instruction::JumpIfIndexNotLessThanConstant.as_u8();
        const JUMPIFINDEXNOTLESSTHANINDEX: u8 = Instruction::JumpIfIndexNotLessThanIndex.as_u8();
        const JUMPIFINDEXNOTLESSTHANEQUALSCONSTANT: u8 =
            Instruction::JumpIfIndexNotLessThanEqualsConstant.as_u8();
        const JUMPIFINDEXNOTLESSTHANEQUALSINDEX: u8 =
            Instruction::JumpIfIndexNotLessThanEqualsIndex.as_u8();
        const JUMPIFINDEXNOTGREATERTHANCONSTANT: u8 =
            Instruction::JumpIfIndexNotGreaterThanConstant.as_u8();
        const JUMPIFINDEXNOTGREATERTHANINDEX: u8 =
            Instruction::JumpIfIndexNotGreaterThanIndex.as_u8();
        const JUMPIFINDEXNOTGREATERTHANEQUALSCONSTANT: u8 =
            Instruction::JumpIfIndexNotGreaterThanEqualsConstant.as_u8();
        const JUMPIFINDEXNOTGREATERTHANEQUALSINDEX: u8 =
            Instruction::JumpIfIndexNotGreaterThanEqualsIndex.as_u8();
        const JUMPIFINDEXNOTSTRICTLYEQUALCONSTANT: u8 =
            Instruction::JumpIfIndexNotStrictlyEqualConstant.as_u8();
        const JUMPIFINDEXNOTSTRICTLYEQUALINDEX: u8 =
            Instruction::JumpIfIndexNotStrictlyEqualIndex.as_u8();
        const JUMPIFINDEXSTRICTLYEQUALCONSTANT: u8 =
            Instruction::JumpIfIndexStrictlyEqualConstant.as_u8();
        const JUMPIFINDEXSTRICTLYEQUALINDEX: u8 =
            Instruction::JumpIfIndexStrictlyEqualIndex.as_u8();
        const JUMPIFINDEXLESSTHANCONSTANT: u8 = Instruction::JumpIfIndexLessThanConstant.as_u8();
        const JUMPIFINDEXLESSTHANINDEX: u8 = Instruction::JumpIfIndexLessThanIndex.as_u8();
        const JUMPIFINDEXLESSTHANEQUALSCONSTANT: u8 =
            Instruction::JumpIfIndexLessThanEqualsConstant.as_u8();
        const JUMPIFINDEXLESSTHANEQUALSINDEX: u8 =
            Instruction::JumpIfIndexLessThanEqualsIndex.as_u8();
        const JUMPIFINDEXGREATERTHANCONSTANT: u8 =
            Instruction::JumpIfIndexGreaterThanConstant.as_u8();
        const JUMPIFINDEXGREATERTHANINDEX: u8 = Instruction::JumpIfIndexGreaterThanIndex.as_u8();
        const JUMPIFINDEXGREATERTHANEQUALSCONSTANT: u8 =
            Instruction::JumpIfIndexGreaterThanEqualsConstant.as_u8();
        const JUMPIFINDEXGREATERTHANEQUALSINDEX: u8 =
            Instruction::JumpIfIndexGreaterThanEqualsIndex.as_u8();
        const INCREMENTINDEXJUMPIFLESSTHANCONSTANT: u8 =
            Instruction::IncrementIndexJumpIfLessThanConstant.as_u8();
        const INCREMENTINDEXJUMPIFLESSTHANINDEX: u8 =
            Instruction::IncrementIndexJumpIfLessThanIndex.as_u8();
        const INCREMENTINDEXJUMPIFLESSTHANEQUALSCONSTANT: u8 =
            Instruction::IncrementIndexJumpIfLessThanEqualsConstant.as_u8();
        const INCREMENTINDEXJUMPIFLESSTHANEQUALSINDEX: u8 =
            Instruction::IncrementIndexJumpIfLessThanEqualsIndex.as_u8();
        const DECREMENTINDEXJUMPIFGREATERTHANCONSTANT: u8 =
            Instruction::DecrementIndexJumpIfGreaterThanConstant.as_u8();
        const DECREMENTINDEXJUMPIFGREATERTHANINDEX: u8 =
            Instruction::DecrementIndexJumpIfGreaterThanIndex.as_u8();
        const DECREMENTINDEXJUMPIFGREATERTHANEQUALSCONSTANT: u8 =
            Instruction::DecrementIndexJumpIfGreaterThanEqualsConstant.as_u8();
        const DECREMENTINDEXJUMPIFGREATERTHANEQUALSINDEX: u8 =
            Instruction::DecrementIndexJumpIfGreaterThanEqualsIndex.as_u8();
        const INCREMENTINDEX: u8 = Instruction::IncrementIndex.as_u8();
        const DECREMENTINDEX: u8 = Instruction::DecrementIndex.as_u8();
        const JUMPTOCASE: u8 = Instruction::JumpToCase.as_u8();
        const LESSTHAN: u8 = Instruction::LessThan.as_u8();
        const LESSTHANEQUALS: u8 = Instruction::LessThanEquals.as_u8();
        const LOAD: u8 = Instruction::Load.as_u8();
        const LOADTOINDEX: u8 = Instruction::PutValueToIndex.as_u8();
        const LOADCOPY: u8 = Instruction::LoadCopy.as_u8();
        const LOADCONSTANT: u8 = Instruction::LoadConstant.as_u8();
        const LOADCONSTANTWIDE: u8 = Instruction::LoadConstantWide.as_u8();
        const LOADSTORESWAP: u8 = Instruction::LoadStoreSwap.as_u8();
        const LOADREPLACE: u8 = Instruction::LoadReplace.as_u8();
        const UPDATEEMPTY: u8 = Instruction::UpdateEmpty.as_u8();
        const SWAP: u8 = Instruction::Swap.as_u8();
        const EMPTY: u8 = Instruction::Empty.as_u8();
        const POPSTACK: u8 = Instruction::PopStack.as_u8();
        const LOGICALNOT: u8 = Instruction::LogicalNot.as_u8();
        const OBJECTCREATE: u8 = Instruction::ObjectCreate.as_u8();
        const OBJECTCREATEWITHSHAPE: u8 = Instruction::ObjectCreateWithShape.as_u8();
        const OBJECTDEFINEPROPERTY: u8 = Instruction::ObjectDefineProperty.as_u8();
        const OBJECTDEFINEMETHOD: u8 = Instruction::ObjectDefineMethod.as_u8();
        const OBJECTDEFINEGETTER: u8 = Instruction::ObjectDefineGetter.as_u8();
        const OBJECTDEFINESETTER: u8 = Instruction::ObjectDefineSetter.as_u8();
        const OBJECTSETPROTOTYPE: u8 = Instruction::ObjectSetPrototype.as_u8();
        const POPEXCEPTIONJUMPTARGET: u8 = Instruction::PopExceptionJumpTarget.as_u8();
        const POPREFERENCE: u8 = Instruction::PopReference.as_u8();
        const PUSHEXCEPTIONJUMPTARGET: u8 = Instruction::PushExceptionJumpTarget.as_u8();
        const PUSHREFERENCE: u8 = Instruction::PushReference.as_u8();
        const PUTVALUE: u8 = Instruction::PutValue.as_u8();
        const PUTVALUEWITHCACHE: u8 = Instruction::PutValueWithCache.as_u8();
        const RESOLVEBINDING: u8 = Instruction::ResolveBinding.as_u8();
        const RESOLVEBINDINGWITHCACHE: u8 = Instruction::ResolveBindingWithCache.as_u8();
        const RESOLVEBINDINGATSITE: u8 = Instruction::ResolveBindingAtSite.as_u8();
        const GETBINDINGATSITE: u8 = Instruction::GetBindingAtSite.as_u8();
        const INSTANTIATEREGEXPLITERAL: u8 = Instruction::InstantiateRegExpLiteral.as_u8();
        const LOADBINDINGATSITE: u8 = Instruction::LoadBindingAtSite.as_u8();
        const GETBINDINGATSITEONCE: u8 = Instruction::GetBindingAtSiteOnce.as_u8();
        const LOADBINDINGATSITEONCE: u8 = Instruction::LoadBindingAtSiteOnce.as_u8();
        const GETVALUEWITHINDEXKEY: u8 = Instruction::GetValueWithIndexKey.as_u8();
        const APPLYBINARYOPERATORWITHCONSTANT: u8 =
            Instruction::ApplyBinaryOperatorWithConstant.as_u8();
        const APPLYBINARYOPERATORTOINDEXWITHCONSTANT: u8 =
            Instruction::ApplyBinaryOperatorToIndexWithConstant.as_u8();
        const APPLYBINARYOPERATORANDMOVE: u8 = Instruction::ApplyBinaryOperatorAndMove.as_u8();
        const LOADPROPERTYWITHCACHEPUSHREFERENCE: u8 =
            Instruction::LoadPropertyWithCachePushReference.as_u8();
        const LOADPROPERTYWITHCACHEPUSHTHIS: u8 =
            Instruction::LoadPropertyWithCachePushThis.as_u8();
        const PUTBINDINGATSITE: u8 = Instruction::PutBindingAtSite.as_u8();
        const RESOLVEGLOBALATSITE: u8 = Instruction::ResolveGlobalAtSite.as_u8();
        const GETGLOBALATSITE: u8 = Instruction::GetGlobalAtSite.as_u8();
        const PUTGLOBALATSITE: u8 = Instruction::PutGlobalAtSite.as_u8();
        const PUTVALUEATSITE: u8 = Instruction::PutValueAtSite.as_u8();
        const RESOLVETHISBINDING: u8 = Instruction::ResolveThisBinding.as_u8();
        const RETURN: u8 = Instruction::Return.as_u8();
        const STORE: u8 = Instruction::Store.as_u8();
        const STORECOPY: u8 = Instruction::StoreCopy.as_u8();
        const STORECONSTANT: u8 = Instruction::StoreConstant.as_u8();
        const STORECONSTANTWIDE: u8 = Instruction::StoreConstantWide.as_u8();
        const STOREFROMINDEX: u8 = Instruction::GetValueFromIndex.as_u8();
        const CONCATSTRINGS: u8 = Instruction::ConcatStrings.as_u8();
        const TOSTRINGOPERAND: u8 = Instruction::ToStringOperand.as_u8();
        const TOSTRINGSUBSTITUTION: u8 = Instruction::ToStringSubstitution.as_u8();
        const THROW: u8 = Instruction::Throw.as_u8();
        const THROWERROR: u8 = Instruction::ThrowError.as_u8();
        const TONUMBER: u8 = Instruction::ToNumber.as_u8();
        const TONUMERIC: u8 = Instruction::ToNumeric.as_u8();
        const TOOBJECT: u8 = Instruction::ToObject.as_u8();
        const TRUNCATESTACK: u8 = Instruction::TruncateStack.as_u8();
        const TYPEOF: u8 = Instruction::Typeof.as_u8();
        const UNARYMINUS: u8 = Instruction::UnaryMinus.as_u8();
        const YIELD: u8 = Instruction::Yield.as_u8();
        const YIELDVALUE: u8 = Instruction::YieldValue.as_u8();
        const CREATEIMMUTABLEBINDING: u8 = Instruction::CreateImmutableBinding.as_u8();
        const CREATEMUTABLEBINDING: u8 = Instruction::CreateMutableBinding.as_u8();
        const INITIALIZEREFERENCEDBINDING: u8 = Instruction::InitializeReferencedBinding.as_u8();
        const INITIALIZEVARIABLEENVIRONMENT: u8 =
            Instruction::InitializeVariableEnvironment.as_u8();
        const ENTERDECLARATIVEENVIRONMENT: u8 = Instruction::EnterDeclarativeEnvironment.as_u8();
        const ENTERCLASSSTATICELEMENTENVIRONMENT: u8 =
            Instruction::EnterClassStaticElementEnvironment.as_u8();
        const ENTERPRIVATEENVIRONMENT: u8 = Instruction::EnterPrivateEnvironment.as_u8();
        const EXITDECLARATIVEENVIRONMENT: u8 = Instruction::ExitDeclarativeEnvironment.as_u8();
        const EXITVARIABLEENVIRONMENT: u8 = Instruction::ExitVariableEnvironment.as_u8();
        const EXITPRIVATEENVIRONMENT: u8 = Instruction::ExitPrivateEnvironment.as_u8();
        const BEGINSIMPLEOBJECTBINDINGPATTERN: u8 =
            Instruction::BeginSimpleObjectBindingPattern.as_u8();
        const BEGINSIMPLEARRAYBINDINGPATTERN: u8 =
            Instruction::BeginSimpleArrayBindingPattern.as_u8();
        const BINDARGUMENTS: u8 = Instruction::BindArguments.as_u8();
        const BINDINGPATTERNBIND: u8 = Instruction::BindingPatternBind.as_u8();
        const BINDINGPATTERNBINDNAMED: u8 = Instruction::BindingPatternBindNamed.as_u8();
        const BINDINGPATTERNBINDTOINDEX: u8 = Instruction::BindingPatternBindToIndex.as_u8();
        const BINDINGPATTERNBINDREST: u8 = Instruction::BindingPatternBindRest.as_u8();
        const BINDINGPATTERNBINDRESTTOINDEX: u8 =
            Instruction::BindingPatternBindRestToIndex.as_u8();
        const BINDINGPATTERNSKIP: u8 = Instruction::BindingPatternSkip.as_u8();
        const BINDINGPATTERNGETVALUE: u8 = Instruction::BindingPatternGetValue.as_u8();
        const BINDINGPATTERNGETVALUENAMED: u8 = Instruction::BindingPatternGetValueNamed.as_u8();
        const BINDINGPATTERNGETRESTVALUE: u8 = Instruction::BindingPatternGetRestValue.as_u8();
        const FINISHBINDINGPATTERN: u8 = Instruction::FinishBindingPattern.as_u8();
        const ENUMERATEOBJECTPROPERTIES: u8 = Instruction::EnumerateObjectProperties.as_u8();
        const GETITERATORSYNC: u8 = Instruction::GetIteratorSync.as_u8();
        const GETITERATORASYNC: u8 = Instruction::GetIteratorAsync.as_u8();
        const ITERATORSTEPVALUE: u8 = Instruction::IteratorStepValue.as_u8();
        const ITERATORSTEPVALUEORUNDEFINED: u8 = Instruction::IteratorStepValueOrUndefined.as_u8();
        const ITERATORNEXT: u8 = Instruction::IteratorCallNextMethod.as_u8();
        const ITERATORCOMPLETE: u8 = Instruction::IteratorComplete.as_u8();
        const ITERATORVALUE: u8 = Instruction::IteratorValue.as_u8();
        const ITERATORTHROW: u8 = Instruction::IteratorThrow.as_u8();
        const ITERATORRETURN: u8 = Instruction::IteratorReturn.as_u8();
        const ITERATORRESTINTOARRAY: u8 = Instruction::IteratorRestIntoArray.as_u8();
        const ITERATORCLOSE: u8 = Instruction::IteratorClose.as_u8();
        const ASYNCITERATORCLOSE: u8 = Instruction::AsyncIteratorClose.as_u8();
        const ITERATORCLOSEWITHERROR: u8 = Instruction::IteratorCloseWithError.as_u8();
        const ASYNCITERATORCLOSEWITHERROR: u8 = Instruction::AsyncIteratorCloseWithError.as_u8();
        const ITERATORPOP: u8 = Instruction::IteratorPop.as_u8();
        const GETNEWTARGET: u8 = Instruction::GetNewTarget.as_u8();
        const IMPORTCALL: u8 = Instruction::ImportCall.as_u8();
        const IMPORTMETA: u8 = Instruction::ImportMeta.as_u8();
        const VERIFYISOBJECT: u8 = Instruction::VerifyIsObject.as_u8();
        match value {
            APPLYADDITIONBINARYOPERATOR => Some(Instruction::ApplyAdditionBinaryOperator),
            APPLYSUBTRACTIONBINARYOPERATOR => Some(Instruction::ApplySubtractionBinaryOperator),
            APPLYMULTIPLICATIONBINARYOPERATOR => {
                Some(Instruction::ApplyMultiplicationBinaryOperator)
            }
            APPLYDIVISIONBINARYOPERATOR => Some(Instruction::ApplyDivisionBinaryOperator),
            APPLYREMAINDERBINARYOPERATOR => Some(Instruction::ApplyRemainderBinaryOperator),
            APPLYEXPONENTIALBINARYOPERATOR => Some(Instruction::ApplyExponentialBinaryOperator),
            APPLYSHIFTLEFTBINARYOPERATOR => Some(Instruction::ApplyShiftLeftBinaryOperator),
            APPLYSHIFTRIGHTBINARYOPERATOR => Some(Instruction::ApplyShiftRightBinaryOperator),
            APPLYSHIFTRIGHTZEROFILLBINARYOPERATOR => {
                Some(Instruction::ApplyShiftRightZeroFillBinaryOperator)
            }
            APPLYBITWISEORBINARYOPERATOR => Some(Instruction::ApplyBitwiseORBinaryOperator),
            APPLYBITWISEXORBINARYOPERATOR => Some(Instruction::ApplyBitwiseXORBinaryOperator),
            APPLYBITWISEANDBINARYOPERATOR => Some(Instruction::ApplyBitwiseAndBinaryOperator),
            DEBUG => Some(Instruction::Debug),
            ARRAYCREATE => Some(Instruction::ArrayCreate),
            ARRAYFROMSTACK => Some(Instruction::ArrayFromStack),
            ARRAYPUSH => Some(Instruction::ArrayPush),
            ARRAYELISION => Some(Instruction::ArrayElision),
            AWAIT => Some(Instruction::Await),
            BITWISENOT => Some(Instruction::BitwiseNot),
            CREATEUNMAPPEDARGUMENTSOBJECT => Some(Instruction::CreateUnmappedArgumentsObject),
            COPYDATAPROPERTIES => Some(Instruction::CopyDataProperties),
            COPYDATAPROPERTIESINTOOBJECT => Some(Instruction::CopyDataPropertiesIntoObject),
            DELETE => Some(Instruction::Delete),
            DIRECTEVALCALL => Some(Instruction::DirectEvalCall),
            EVALUATECALL => Some(Instruction::EvaluateCall),
            LOADFROMINDEX => Some(Instruction::LoadFromIndex),
            GETPROPERTYWITHCACHE => Some(Instruction::GetPropertyWithCache),
            GETPROPERTYWITHCACHEKEEPREFERENCE => {
                Some(Instruction::GetPropertyWithCacheKeepReference)
            }
            GETTHISPROPERTYWITHCACHE => Some(Instruction::GetThisPropertyWithCache),
            GETTHISPROPERTYWITHCACHEKEEPREFERENCE => {
                Some(Instruction::GetThisPropertyWithCacheKeepReference)
            }
            GETINDEXPROPERTYWITHCACHE => Some(Instruction::GetIndexPropertyWithCache),
            GETINDEXPROPERTYWITHCACHEKEEPREFERENCE => {
                Some(Instruction::GetIndexPropertyWithCacheKeepReference)
            }
            LOADINDEXPROPERTYWITHCACHEPUSHTHIS => {
                Some(Instruction::LoadIndexPropertyWithCachePushThis)
            }
            PUTPROPERTYWITHCACHE => Some(Instruction::PutPropertyWithCache),
            PUSHREFERENCEANDLOAD => Some(Instruction::PushReferenceAndLoad),
            POPREFERENCEANDEVALUATECALL => Some(Instruction::PopReferenceAndEvaluateCall),
            EVALUATECALLONTHIS => Some(Instruction::EvaluateCallOnThis),
            ISSTRICTLYEQUALCONSTANT => Some(Instruction::IsStrictlyEqualConstant),
            EVALUATENEW => Some(Instruction::EvaluateNew),
            EVALUATESUPER => Some(Instruction::EvaluateSuper),
            EVALUATEPROPERTYACCESSWITHEXPRESSIONKEY => {
                Some(Instruction::EvaluatePropertyAccessWithExpressionKey)
            }
            EVALUATEPROPERTYACCESSWITHIDENTIFIERKEY => {
                Some(Instruction::EvaluatePropertyAccessWithIdentifierKey)
            }
            MAKEPRIVATEREFERENCE => Some(Instruction::MakePrivateReference),
            MAKESUPERPROPERTYREFERENCEWITHEXPRESSIONKEY => {
                Some(Instruction::MakeSuperPropertyReferenceWithExpressionKey)
            }
            MAKESUPERPROPERTYREFERENCEWITHIDENTIFIERKEY => {
                Some(Instruction::MakeSuperPropertyReferenceWithIdentifierKey)
            }
            GETVALUE => Some(Instruction::GetValue),
            GETVALUEWITHCACHE => Some(Instruction::GetValueWithCache),
            GETVALUEKEEPREFERENCE => Some(Instruction::GetValueKeepReference),
            GETVALUEWITHCACHEKEEPREFERENCE => Some(Instruction::GetValueWithCacheKeepReference),
            GREATERTHAN => Some(Instruction::GreaterThan),
            GREATERTHANEQUALS => Some(Instruction::GreaterThanEquals),
            HASPROPERTY => Some(Instruction::HasProperty),
            HASPRIVATEELEMENT => Some(Instruction::HasPrivateElement),
            INCREMENT => Some(Instruction::Increment),
            DECREMENT => Some(Instruction::Decrement),
            INSTANCEOFOPERATOR => Some(Instruction::InstanceofOperator),
            INSTANTIATEARROWFUNCTIONEXPRESSION => {
                Some(Instruction::InstantiateArrowFunctionExpression)
            }
            INSTANTIATEORDINARYFUNCTIONEXPRESSION => {
                Some(Instruction::InstantiateOrdinaryFunctionExpression)
            }
            CLASSDEFINECONSTRUCTOR => Some(Instruction::ClassDefineConstructor),
            CLASSDEFINEDEFAULTCONSTRUCTOR => Some(Instruction::ClassDefineDefaultConstructor),
            CLASSDEFINEPRIVATEMETHOD => Some(Instruction::ClassDefinePrivateMethod),
            CLASSDEFINEPRIVATEPROPERTY => Some(Instruction::ClassDefinePrivateProperty),
            CLASSINITIALIZEPRIVATEELEMENTS => Some(Instruction::ClassInitializePrivateElements),
            PUTPRIVATEVALUE => Some(Instruction::ClassInitializePrivateValue),
            ISLOOSELYEQUAL => Some(Instruction::IsLooselyEqual),
            ISSTRICTLYEQUAL => Some(Instruction::IsStrictlyEqual),
            ISNULLORUNDEFINED => Some(Instruction::IsNullOrUndefined),
            ISNULL => Some(Instruction::IsNull),
            ISUNDEFINED => Some(Instruction::IsUndefined),
            ISOBJECT => Some(Instruction::IsObject),
            ISCONSTRUCTOR => Some(Instruction::IsConstructor),
            JUMP => Some(Instruction::Jump),
            JUMPIFNOT => Some(Instruction::JumpIfNot),
            JUMPIFTRUE => Some(Instruction::JumpIfTrue),
            GETVALUEWITHEXPRESSIONKEY => Some(Instruction::GetValueWithExpressionKey),
            PUTVALUEWITHEXPRESSIONKEY => Some(Instruction::PutValueWithExpressionKey),
            LOADFROMINDEXSTORECONSTANT => Some(Instruction::LoadFromIndexStoreConstant),
            LOADFROMINDEXGETFROMINDEX => Some(Instruction::LoadFromIndexGetFromIndex),
            LOADFROMINDEXLOADFROMINDEX => Some(Instruction::LoadFromIndexLoadFromIndex),
            JUMPIFNOTLESSTHAN => Some(Instruction::JumpIfNotLessThan),
            JUMPIFNOTLESSTHANEQUALS => Some(Instruction::JumpIfNotLessThanEquals),
            JUMPIFNOTGREATERTHAN => Some(Instruction::JumpIfNotGreaterThan),
            JUMPIFNOTGREATERTHANEQUALS => Some(Instruction::JumpIfNotGreaterThanEquals),
            JUMPIFLESSTHAN => Some(Instruction::JumpIfLessThan),
            JUMPIFLESSTHANEQUALS => Some(Instruction::JumpIfLessThanEquals),
            JUMPIFGREATERTHAN => Some(Instruction::JumpIfGreaterThan),
            JUMPIFGREATERTHANEQUALS => Some(Instruction::JumpIfGreaterThanEquals),
            JUMPIFNOTSTRICTLYEQUAL => Some(Instruction::JumpIfNotStrictlyEqual),
            JUMPIFSTRICTLYEQUAL => Some(Instruction::JumpIfStrictlyEqual),
            JUMPIFNOTLOOSELYEQUAL => Some(Instruction::JumpIfNotLooselyEqual),
            JUMPIFLOOSELYEQUAL => Some(Instruction::JumpIfLooselyEqual),
            JUMPIFINDEXNOTLESSTHANCONSTANT => Some(Instruction::JumpIfIndexNotLessThanConstant),
            JUMPIFINDEXNOTLESSTHANINDEX => Some(Instruction::JumpIfIndexNotLessThanIndex),
            JUMPIFINDEXNOTLESSTHANEQUALSCONSTANT => {
                Some(Instruction::JumpIfIndexNotLessThanEqualsConstant)
            }
            JUMPIFINDEXNOTLESSTHANEQUALSINDEX => {
                Some(Instruction::JumpIfIndexNotLessThanEqualsIndex)
            }
            JUMPIFINDEXNOTGREATERTHANCONSTANT => {
                Some(Instruction::JumpIfIndexNotGreaterThanConstant)
            }
            JUMPIFINDEXNOTGREATERTHANINDEX => Some(Instruction::JumpIfIndexNotGreaterThanIndex),
            JUMPIFINDEXNOTGREATERTHANEQUALSCONSTANT => {
                Some(Instruction::JumpIfIndexNotGreaterThanEqualsConstant)
            }
            JUMPIFINDEXNOTGREATERTHANEQUALSINDEX => {
                Some(Instruction::JumpIfIndexNotGreaterThanEqualsIndex)
            }
            JUMPIFINDEXNOTSTRICTLYEQUALCONSTANT => {
                Some(Instruction::JumpIfIndexNotStrictlyEqualConstant)
            }
            JUMPIFINDEXNOTSTRICTLYEQUALINDEX => Some(Instruction::JumpIfIndexNotStrictlyEqualIndex),
            JUMPIFINDEXSTRICTLYEQUALCONSTANT => Some(Instruction::JumpIfIndexStrictlyEqualConstant),
            JUMPIFINDEXSTRICTLYEQUALINDEX => Some(Instruction::JumpIfIndexStrictlyEqualIndex),
            JUMPIFINDEXLESSTHANCONSTANT => Some(Instruction::JumpIfIndexLessThanConstant),
            JUMPIFINDEXLESSTHANINDEX => Some(Instruction::JumpIfIndexLessThanIndex),
            JUMPIFINDEXLESSTHANEQUALSCONSTANT => {
                Some(Instruction::JumpIfIndexLessThanEqualsConstant)
            }
            JUMPIFINDEXLESSTHANEQUALSINDEX => Some(Instruction::JumpIfIndexLessThanEqualsIndex),
            JUMPIFINDEXGREATERTHANCONSTANT => Some(Instruction::JumpIfIndexGreaterThanConstant),
            JUMPIFINDEXGREATERTHANINDEX => Some(Instruction::JumpIfIndexGreaterThanIndex),
            JUMPIFINDEXGREATERTHANEQUALSCONSTANT => {
                Some(Instruction::JumpIfIndexGreaterThanEqualsConstant)
            }
            JUMPIFINDEXGREATERTHANEQUALSINDEX => {
                Some(Instruction::JumpIfIndexGreaterThanEqualsIndex)
            }
            INCREMENTINDEXJUMPIFLESSTHANCONSTANT => {
                Some(Instruction::IncrementIndexJumpIfLessThanConstant)
            }
            INCREMENTINDEXJUMPIFLESSTHANINDEX => {
                Some(Instruction::IncrementIndexJumpIfLessThanIndex)
            }
            INCREMENTINDEXJUMPIFLESSTHANEQUALSCONSTANT => {
                Some(Instruction::IncrementIndexJumpIfLessThanEqualsConstant)
            }
            INCREMENTINDEXJUMPIFLESSTHANEQUALSINDEX => {
                Some(Instruction::IncrementIndexJumpIfLessThanEqualsIndex)
            }
            DECREMENTINDEXJUMPIFGREATERTHANCONSTANT => {
                Some(Instruction::DecrementIndexJumpIfGreaterThanConstant)
            }
            DECREMENTINDEXJUMPIFGREATERTHANINDEX => {
                Some(Instruction::DecrementIndexJumpIfGreaterThanIndex)
            }
            DECREMENTINDEXJUMPIFGREATERTHANEQUALSCONSTANT => {
                Some(Instruction::DecrementIndexJumpIfGreaterThanEqualsConstant)
            }
            DECREMENTINDEXJUMPIFGREATERTHANEQUALSINDEX => {
                Some(Instruction::DecrementIndexJumpIfGreaterThanEqualsIndex)
            }
            INCREMENTINDEX => Some(Instruction::IncrementIndex),
            DECREMENTINDEX => Some(Instruction::DecrementIndex),
            JUMPTOCASE => Some(Instruction::JumpToCase),
            LESSTHAN => Some(Instruction::LessThan),
            LESSTHANEQUALS => Some(Instruction::LessThanEquals),
            LOAD => Some(Instruction::Load),
            LOADCOPY => Some(Instruction::LoadCopy),
            LOADCONSTANT => Some(Instruction::LoadConstant),
            LOADCONSTANTWIDE => Some(Instruction::LoadConstantWide),
            LOADSTORESWAP => Some(Instruction::LoadStoreSwap),
            LOADREPLACE => Some(Instruction::LoadReplace),
            LOADTOINDEX => Some(Instruction::PutValueToIndex),
            UPDATEEMPTY => Some(Instruction::UpdateEmpty),
            SWAP => Some(Instruction::Swap),
            EMPTY => Some(Instruction::Empty),
            POPSTACK => Some(Instruction::PopStack),
            LOGICALNOT => Some(Instruction::LogicalNot),
            OBJECTCREATE => Some(Instruction::ObjectCreate),
            OBJECTCREATEWITHSHAPE => Some(Instruction::ObjectCreateWithShape),
            OBJECTDEFINEPROPERTY => Some(Instruction::ObjectDefineProperty),
            OBJECTDEFINEMETHOD => Some(Instruction::ObjectDefineMethod),
            OBJECTDEFINEGETTER => Some(Instruction::ObjectDefineGetter),
            OBJECTDEFINESETTER => Some(Instruction::ObjectDefineSetter),
            OBJECTSETPROTOTYPE => Some(Instruction::ObjectSetPrototype),
            POPEXCEPTIONJUMPTARGET => Some(Instruction::PopExceptionJumpTarget),
            POPREFERENCE => Some(Instruction::PopReference),
            PUSHEXCEPTIONJUMPTARGET => Some(Instruction::PushExceptionJumpTarget),
            PUSHREFERENCE => Some(Instruction::PushReference),
            PUTVALUE => Some(Instruction::PutValue),
            PUTVALUEWITHCACHE => Some(Instruction::PutValueWithCache),
            RESOLVEBINDING => Some(Instruction::ResolveBinding),
            RESOLVEBINDINGWITHCACHE => Some(Instruction::ResolveBindingWithCache),
            RESOLVEBINDINGATSITE => Some(Instruction::ResolveBindingAtSite),
            GETBINDINGATSITE => Some(Instruction::GetBindingAtSite),
            INSTANTIATEREGEXPLITERAL => Some(Instruction::InstantiateRegExpLiteral),
            LOADBINDINGATSITE => Some(Instruction::LoadBindingAtSite),
            GETBINDINGATSITEONCE => Some(Instruction::GetBindingAtSiteOnce),
            LOADBINDINGATSITEONCE => Some(Instruction::LoadBindingAtSiteOnce),
            GETVALUEWITHINDEXKEY => Some(Instruction::GetValueWithIndexKey),
            APPLYBINARYOPERATORWITHCONSTANT => Some(Instruction::ApplyBinaryOperatorWithConstant),
            APPLYBINARYOPERATORTOINDEXWITHCONSTANT => {
                Some(Instruction::ApplyBinaryOperatorToIndexWithConstant)
            }
            APPLYBINARYOPERATORANDMOVE => Some(Instruction::ApplyBinaryOperatorAndMove),
            LOADPROPERTYWITHCACHEPUSHREFERENCE => {
                Some(Instruction::LoadPropertyWithCachePushReference)
            }
            LOADPROPERTYWITHCACHEPUSHTHIS => Some(Instruction::LoadPropertyWithCachePushThis),
            PUTBINDINGATSITE => Some(Instruction::PutBindingAtSite),
            RESOLVEGLOBALATSITE => Some(Instruction::ResolveGlobalAtSite),
            GETGLOBALATSITE => Some(Instruction::GetGlobalAtSite),
            PUTGLOBALATSITE => Some(Instruction::PutGlobalAtSite),
            PUTVALUEATSITE => Some(Instruction::PutValueAtSite),
            RESOLVETHISBINDING => Some(Instruction::ResolveThisBinding),
            RETURN => Some(Instruction::Return),
            STORE => Some(Instruction::Store),
            STORECOPY => Some(Instruction::StoreCopy),
            STORECONSTANT => Some(Instruction::StoreConstant),
            STORECONSTANTWIDE => Some(Instruction::StoreConstantWide),
            STOREFROMINDEX => Some(Instruction::GetValueFromIndex),
            CONCATSTRINGS => Some(Instruction::ConcatStrings),
            TOSTRINGOPERAND => Some(Instruction::ToStringOperand),
            TOSTRINGSUBSTITUTION => Some(Instruction::ToStringSubstitution),
            THROW => Some(Instruction::Throw),
            THROWERROR => Some(Instruction::ThrowError),
            TONUMBER => Some(Instruction::ToNumber),
            TONUMERIC => Some(Instruction::ToNumeric),
            TOOBJECT => Some(Instruction::ToObject),
            TRUNCATESTACK => Some(Instruction::TruncateStack),
            TYPEOF => Some(Instruction::Typeof),
            UNARYMINUS => Some(Instruction::UnaryMinus),
            YIELD => Some(Instruction::Yield),
            YIELDVALUE => Some(Instruction::YieldValue),
            CREATEIMMUTABLEBINDING => Some(Instruction::CreateImmutableBinding),
            CREATEMUTABLEBINDING => Some(Instruction::CreateMutableBinding),
            INITIALIZEREFERENCEDBINDING => Some(Instruction::InitializeReferencedBinding),
            INITIALIZEVARIABLEENVIRONMENT => Some(Instruction::InitializeVariableEnvironment),
            ENTERDECLARATIVEENVIRONMENT => Some(Instruction::EnterDeclarativeEnvironment),
            ENTERCLASSSTATICELEMENTENVIRONMENT => {
                Some(Instruction::EnterClassStaticElementEnvironment)
            }
            ENTERPRIVATEENVIRONMENT => Some(Instruction::EnterPrivateEnvironment),
            EXITDECLARATIVEENVIRONMENT => Some(Instruction::ExitDeclarativeEnvironment),
            EXITVARIABLEENVIRONMENT => Some(Instruction::ExitVariableEnvironment),
            EXITPRIVATEENVIRONMENT => Some(Instruction::ExitPrivateEnvironment),
            BEGINSIMPLEOBJECTBINDINGPATTERN => Some(Instruction::BeginSimpleObjectBindingPattern),
            BEGINSIMPLEARRAYBINDINGPATTERN => Some(Instruction::BeginSimpleArrayBindingPattern),
            BINDARGUMENTS => Some(Instruction::BindArguments),
            BINDINGPATTERNBIND => Some(Instruction::BindingPatternBind),
            BINDINGPATTERNBINDNAMED => Some(Instruction::BindingPatternBindNamed),
            BINDINGPATTERNBINDTOINDEX => Some(Instruction::BindingPatternBindToIndex),
            BINDINGPATTERNBINDREST => Some(Instruction::BindingPatternBindRest),
            BINDINGPATTERNBINDRESTTOINDEX => Some(Instruction::BindingPatternBindRestToIndex),
            BINDINGPATTERNSKIP => Some(Instruction::BindingPatternSkip),
            BINDINGPATTERNGETVALUE => Some(Instruction::BindingPatternGetValue),
            BINDINGPATTERNGETVALUENAMED => Some(Instruction::BindingPatternGetValueNamed),
            BINDINGPATTERNGETRESTVALUE => Some(Instruction::BindingPatternGetRestValue),
            FINISHBINDINGPATTERN => Some(Instruction::FinishBindingPattern),
            ENUMERATEOBJECTPROPERTIES => Some(Instruction::EnumerateObjectProperties),
            GETITERATORSYNC => Some(Instruction::GetIteratorSync),
            GETITERATORASYNC => Some(Instruction::GetIteratorAsync),
            ITERATORSTEPVALUE => Some(Instruction::IteratorStepValue),
            ITERATORSTEPVALUEORUNDEFINED => Some(Instruction::IteratorStepValueOrUndefined),
            ITERATORNEXT => Some(Instruction::IteratorCallNextMethod),
            ITERATORCOMPLETE => Some(Instruction::IteratorComplete),
            ITERATORVALUE => Some(Instruction::IteratorValue),
            ITERATORTHROW => Some(Instruction::IteratorThrow),
            ITERATORRETURN => Some(Instruction::IteratorReturn),
            ITERATORRESTINTOARRAY => Some(Instruction::IteratorRestIntoArray),
            ITERATORCLOSE => Some(Instruction::IteratorClose),
            ASYNCITERATORCLOSE => Some(Instruction::AsyncIteratorClose),
            ITERATORCLOSEWITHERROR => Some(Instruction::IteratorCloseWithError),
            ASYNCITERATORCLOSEWITHERROR => Some(Instruction::AsyncIteratorCloseWithError),
            ITERATORPOP => Some(Instruction::IteratorPop),
            GETNEWTARGET => Some(Instruction::GetNewTarget),
            IMPORTCALL => Some(Instruction::ImportCall),
            IMPORTMETA => Some(Instruction::ImportMeta),
            VERIFYISOBJECT => Some(Instruction::VerifyIsObject),
            GETKEPTPROPERTYWITHCACHE => Some(Instruction::GetKeptPropertyWithCache),
            JUMPIFRESULTCOMPARESWITHCONSTANT => Some(Instruction::JumpIfResultComparesWithConstant),
            STEPVALUEWITHCACHE => Some(Instruction::StepValueWithCache),
            UNUSED234 => Some(Instruction::Unused234),
            UNUSED235 => Some(Instruction::Unused235),
            UNUSED236 => Some(Instruction::Unused236),
            UNUSED237 => Some(Instruction::Unused237),
            UNUSED238 => Some(Instruction::Unused238),
            UNUSED239 => Some(Instruction::Unused239),
            UNUSED240 => Some(Instruction::Unused240),
            UNUSED241 => Some(Instruction::Unused241),
            UNUSED242 => Some(Instruction::Unused242),
            UNUSED243 => Some(Instruction::Unused243),
            UNUSED244 => Some(Instruction::Unused244),
            UNUSED245 => Some(Instruction::Unused245),
            UNUSED246 => Some(Instruction::Unused246),
            UNUSED247 => Some(Instruction::Unused247),
            UNUSED248 => Some(Instruction::Unused248),
            UNUSED249 => Some(Instruction::Unused249),
            UNUSED250 => Some(Instruction::Unused250),
            UNUSED251 => Some(Instruction::Unused251),
            UNUSED252 => Some(Instruction::Unused252),
            UNUSED253 => Some(Instruction::Unused253),
            UNUSED254 => Some(Instruction::Unused254),
            UNUSED255 => Some(Instruction::Unused255),
        }
    }

    /// Whether this is the kind of a byte that is no instruction.
    #[inline(always)]
    pub(crate) const fn is_unused(self) -> bool {
        self.as_u8() >= Instruction::Unused234.as_u8()
    }
}

// These panics are marked as cold and moved into their own non-inlineable
// functions, they should be called very infrequently and by doing this
// eliminates the setup and teardown of stack frames keeping the hot-paths
// as hot as possible.
//
// For reference, on my ARM64 machine it reduced the assembly instructions
// needed for `get_first_arg` from 34 all the way down to 14 and showing
// a minor but noticeable performance increase when running the "richards"
// benchmark.

#[cold]
#[inline(never)]
pub(super) fn panic_invalid_instruction() -> ! {
    panic!("Invalid bytecode instruction")
}

#[cold]
#[inline(never)]
fn panic_no_argument() -> ! {
    panic!("Instruction does not have an argument")
}

#[cold]
#[inline(never)]
fn panic_not_two_arguments() -> ! {
    panic!("Instruction does not have two argument")
}

#[cold]
#[inline(never)]
fn panic_first_argument_not_boolean() -> ! {
    panic!("First argument was not a boolean")
}

#[cold]
#[inline(never)]
fn panic_second_argument_not_boolean() -> ! {
    panic!("Second argument was not a boolean")
}

#[cfg(test)]
mod tests {
    use super::{Instr, Instruction};

    #[test]
    fn every_byte_is_a_kind_and_the_unused_ones_are_no_instruction() {
        for byte in 0..=u8::MAX {
            let kind = Instruction::from_u8(byte).expect("every byte is a kind");
            assert_eq!(kind.as_u8(), byte);
            assert_eq!(
                kind.is_unused(),
                byte > Instruction::StepValueWithCache.as_u8()
            );
            assert_eq!(Instruction::try_from(byte).is_ok(), !kind.is_unused());
            if kind.is_unused() {
                assert_eq!(kind.argument_count(), 0);
            }
        }
        assert!(!Instruction::Debug.is_unused());
        assert!(!Instruction::GetKeptPropertyWithCache.is_unused());
        assert!(!Instruction::JumpIfResultComparesWithConstant.is_unused());
        assert!(!Instruction::StepValueWithCache.is_unused());
        assert!(Instruction::Unused234.is_unused());
        assert!(Instruction::Unused255.is_unused());
    }

    #[test]
    #[should_panic(expected = "Invalid bytecode instruction")]
    fn code_with_a_byte_that_is_no_instruction_is_not_read() {
        let code = [Instruction::Unused234.as_u8()];
        let mut ip = 0;
        let _ = Instr::consume_instruction(&code, &mut ip);
    }

    #[test]
    #[should_panic(expected = "Invalid bytecode instruction")]
    fn the_last_byte_is_no_instruction_either() {
        let code = [Instruction::Debug.as_u8(), u8::MAX];
        let mut ip = 1;
        let _ = Instr::consume_instruction(&code, &mut ip);
    }
}
