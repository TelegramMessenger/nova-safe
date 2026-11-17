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
    /// GetValueFromIndex followed by Load: push a copy of the given stack
    /// slot, leaving the result register empty.
    LoadFromIndex,
    /// EvaluatePropertyAccessWithIdentifierKey followed by GetValueWithCache.
    /// The first immediate is the identifier, the second the cache.
    GetPropertyWithCache,
    /// EvaluatePropertyAccessWithIdentifierKey followed by
    /// GetValueWithCacheKeepReference.
    GetPropertyWithCacheKeepReference,
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
    /// Take N items from the stack and string-concatenate them together.
    StringConcat,
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
}

impl Instruction {
    /// Returns true if this instruction is a terminal instruction where
    /// control flow cannot continue to the next instruction.
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Jump | Self::Return | Self::Throw | Self::ThrowError
        )
    }

    #[inline(always)]
    pub const fn argument_count(self) -> u8 {
        match self {
            // Number of repetitions and lexical status
            Self::BeginSimpleArrayBindingPattern
            | Self::BindArguments
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
            | Self::LoadConstantWide
            | Self::StoreConstantWide
            | Self::ObjectDefineGetter
            | Self::ObjectDefineMethod
            | Self::ObjectDefineSetter
            | Self::PushExceptionJumpTarget
            | Self::GetPropertyWithCache
            | Self::GetPropertyWithCacheKeepReference
            | Self::ResolveBindingWithCache
            | Self::ResolveBindingAtSite
            | Self::GetBindingAtSite
            | Self::PutBindingAtSite
            | Self::ResolveGlobalAtSite
            | Self::GetGlobalAtSite
            | Self::PutGlobalAtSite
            | Self::PutValueAtSite => 2,
            Self::PutValueToIndex
            | Self::GetValueFromIndex
            | Self::ArrayCreate
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
            | Self::StringConcat
            | Self::ThrowError
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
                | Self::PutValueWithCache
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
                | Self::MakePrivateReference
                | Self::MakeSuperPropertyReferenceWithIdentifierKey
                | Self::ResolveBinding
                | Self::ResolveBindingWithCache
                | Self::ResolveBindingAtSite
                | Self::GetBindingAtSite
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
                | Self::PushExceptionJumpTarget
                | Self::IteratorStepValue
        )
    }

    pub const fn as_u8(self) -> u8 {
        self as u8
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
        debug_assert_eq!(self.kind.argument_count(), 2);
        debug_assert!(!self.kind.has_double_arg());
        self.args.two_args()[1] as usize
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
            _ => unreachable!(),
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
            Instruction::ObjectDefineGetter => "get function() {}".to_string(),
            Instruction::ObjectDefineMethod => "function() {}".to_string(),
            Instruction::ObjectDefineSetter => "set function() {}".to_string(),
            Instruction::ResolveBindingWithCache
            | Instruction::ResolveBindingAtSite
            | Instruction::GetBindingAtSite
            | Instruction::PutBindingAtSite
            | Instruction::ResolveGlobalAtSite
            | Instruction::GetGlobalAtSite
            | Instruction::PutGlobalAtSite
            | Instruction::PutValueAtSite
            | Instruction::GetPropertyWithCache
            | Instruction::GetPropertyWithCacheKeepReference => {
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
        Self::from_u8(value).ok_or(())
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
        const ARRAYCREATE: u8 = Instruction::ArrayCreate.as_u8();
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
        const PUSHREFERENCEANDLOAD: u8 = Instruction::PushReferenceAndLoad.as_u8();
        const POPREFERENCEANDEVALUATECALL: u8 = Instruction::PopReferenceAndEvaluateCall.as_u8();
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
        const STRINGCONCAT: u8 = Instruction::StringConcat.as_u8();
        const THROW: u8 = Instruction::Throw.as_u8();
        const THROWERROR: u8 = Instruction::ThrowError.as_u8();
        const TONUMBER: u8 = Instruction::ToNumber.as_u8();
        const TONUMERIC: u8 = Instruction::ToNumeric.as_u8();
        const TOOBJECT: u8 = Instruction::ToObject.as_u8();
        const TRUNCATESTACK: u8 = Instruction::TruncateStack.as_u8();
        const TYPEOF: u8 = Instruction::Typeof.as_u8();
        const UNARYMINUS: u8 = Instruction::UnaryMinus.as_u8();
        const YIELD: u8 = Instruction::Yield.as_u8();
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
            PUSHREFERENCEANDLOAD => Some(Instruction::PushReferenceAndLoad),
            POPREFERENCEANDEVALUATECALL => Some(Instruction::PopReferenceAndEvaluateCall),
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
            STRINGCONCAT => Some(Instruction::StringConcat),
            THROW => Some(Instruction::Throw),
            THROWERROR => Some(Instruction::ThrowError),
            TONUMBER => Some(Instruction::ToNumber),
            TONUMERIC => Some(Instruction::ToNumeric),
            TOOBJECT => Some(Instruction::ToObject),
            TRUNCATESTACK => Some(Instruction::TruncateStack),
            TYPEOF => Some(Instruction::Typeof),
            UNARYMINUS => Some(Instruction::UnaryMinus),
            YIELD => Some(Instruction::Yield),
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
            _ => None,
        }
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
fn panic_invalid_instruction() -> ! {
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
