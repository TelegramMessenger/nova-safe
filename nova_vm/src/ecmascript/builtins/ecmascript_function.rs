// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::borrow::Cow;

use oxc_ast::ast::{FormalParameters, FunctionBody};
use oxc_semantic::ScopeId;
use oxc_span::Span;

use crate::{
    ecmascript::{
        Agent, ArgumentsList, BUILTIN_STRING_MEMORY, CallThisValue, ECMAScriptCodeEvaluationState,
        ECMAScriptFunctionHeapData, Environment, ExceptionType, ExecutionContext, Function,
        FunctionEnvironment, FunctionInternalProperties, InternalSlots, JsResult, Object,
        OrdinaryObject, PrivateEnvironment, PropertyDescriptor, PropertyKey, ProtoIntrinsics,
        Realm, ScriptOrModule, SourceCode, String, ThisBindingStatus, Value,
        evaluate_async_function_body, evaluate_async_generator_body, evaluate_function_body,
        evaluate_generator_body, function_handle, get_active_script_or_module,
        new_declarative_environment, new_function_environment, ordinary_create_from_constructor,
        ordinary_object_create_with_intrinsics, to_object,
    },
    engine::{Bindable, CallEnvironment, Executable, GcScope, NoGcScope, Scopable},
    heap::{
        ArenaAccess, ArenaAccessMut, BaseIndex, CompactionLists, CreateHeapData, Heap,
        HeapMarkAndSweep, HeapSweepWeakReference, ObjectEntry, ObjectEntryPropertyDescriptor,
        WorkQueues, arena_vec_access,
    },
    ndt,
};

/// ## [10.2 ECMAScript Function Objects](https://tc39.es/ecma262/#sec-ecmascript-function-objects)
///
/// ECMAScript function objects encapsulate parameterized ECMAScript code closed
/// over a lexical environment and support the dynamic evaluation of that code.
/// An ECMAScript function object is an ordinary object and has the same
/// internal slots and the same internal methods as other ordinary objects. The
/// code of an ECMAScript function object may be either strict mode code
/// (11.2.2) or non-strict code. An ECMAScript function object whose code is
/// strict mode code is called a _strict function_. One whose code is not strict
/// mode code is called a _non-strict function_.
///
/// In addition to \[\[Extensible]] and \[\[Prototype]], ECMAScript function
/// objects also have the internal slots listed in [Table 25].
///
/// [Table 25]: https://tc39.es/ecma262/#table-internal-slots-of-ecmascript-function-objects
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct ECMAScriptFunction<'a>(BaseIndex<'a, ECMAScriptFunctionHeapData<'static>>);
function_handle!(ECMAScriptFunction);
arena_vec_access!(
    ECMAScriptFunction,
    'a,
    ECMAScriptFunctionHeapData,
    ecmascript_functions
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConstructorStatus {
    NonConstructor,
    ConstructorFunction,
    BaseClass,
    DerivedClass,
}

impl ConstructorStatus {
    pub(crate) fn is_constructor(self) -> bool {
        self != ConstructorStatus::NonConstructor
    }
    pub(crate) fn is_class_constructor(self) -> bool {
        matches!(
            self,
            ConstructorStatus::BaseClass | ConstructorStatus::DerivedClass
        )
    }
    pub(crate) fn is_derived_class(self) -> bool {
        self == ConstructorStatus::DerivedClass
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ThisMode {
    Lexical,
    Strict,
    Global,
}

/// The kind of an ECMAScript function's code: what \[\[ECMAScriptCode]]
/// is and how calling the function evaluates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FunctionKind {
    Function,
    AsyncFunction,
    Generator,
    AsyncGenerator,
    ClassConstructor,
    Arrow,
    AsyncArrow,
}

impl FunctionKind {
    #[inline]
    pub(crate) fn is_async(self) -> bool {
        matches!(
            self,
            Self::AsyncFunction | Self::AsyncGenerator | Self::AsyncArrow
        )
    }

    #[inline]
    pub(crate) fn is_generator(self) -> bool {
        matches!(self, Self::Generator | Self::AsyncGenerator)
    }
}

/// What OrdinaryFunctionCreate needs from a function's source text, taken
/// from the AST when the function is compiled.
#[derive(Debug, Clone, Copy, Bindable)]
pub(crate) struct FunctionTemplate<'a> {
    pub(crate) kind: FunctionKind,
    /// The source text matched by the function's definition.
    pub(crate) span: Span,
    /// The function's own code makes it strict, whatever the code that
    /// creates it is: its body has a "use strict" directive, or it is an
    /// element of a class (a constructor, a method, a getter or a setter),
    /// and all of a class is strict mode code.
    pub(crate) has_use_strict_directive: bool,
    /// The ExpectedArgumentCount of the function's formal parameters.
    pub(crate) expected_argument_count: u32,
    /// The compiled \[\[ECMAScriptCode]]. Only a class constructor's is set
    /// after the template is made, when its class is compiled.
    pub(crate) executable: Option<Executable<'a>>,
}

impl<'a> FunctionTemplate<'a> {
    /// Takes a function's template from its AST, without compiling it.
    pub(crate) fn from_ast(ast: FunctionAstRef<'_>, executable: Option<Executable<'a>>) -> Self {
        Self {
            kind: ast.kind(),
            span: ast.span(),
            has_use_strict_directive: ast.ecmascript_code().has_use_strict_directive(),
            expected_argument_count: u32::try_from(expected_arguments_count(
                ast.formal_parameters(),
            ))
            .unwrap_or(u32::MAX),
            executable,
        }
    }
}

impl HeapMarkAndSweep for FunctionTemplate<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        self.executable.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        self.executable.sweep_values(compactions);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FunctionAstRef<'a> {
    Function(&'a oxc_ast::ast::Function<'a>),
    AsyncFunction(&'a oxc_ast::ast::Function<'a>),
    Generator(&'a oxc_ast::ast::Function<'a>),
    AsyncGenerator(&'a oxc_ast::ast::Function<'a>),
    ClassConstructor(&'a oxc_ast::ast::Function<'a>),
    Arrow(&'a oxc_ast::ast::ArrowFunctionExpression<'a>),
    AsyncArrow(&'a oxc_ast::ast::ArrowFunctionExpression<'a>),
}

impl<'a> From<&'a oxc_ast::ast::Function<'a>> for FunctionAstRef<'a> {
    fn from(f: &'a oxc_ast::ast::Function<'a>) -> Self {
        match (f.r#async, f.generator) {
            (true, true) => FunctionAstRef::AsyncGenerator(f),
            (true, false) => FunctionAstRef::AsyncFunction(f),
            (false, true) => FunctionAstRef::Generator(f),
            (false, false) => FunctionAstRef::Function(f),
        }
    }
}

impl<'a> From<&'a oxc_ast::ast::ArrowFunctionExpression<'a>> for FunctionAstRef<'a> {
    fn from(f: &'a oxc_ast::ast::ArrowFunctionExpression<'a>) -> Self {
        match f.r#async {
            true => FunctionAstRef::AsyncArrow(f),
            false => FunctionAstRef::Arrow(f),
        }
    }
}

impl<'ast> FunctionAstRef<'ast> {
    pub(crate) fn scope_id(&self) -> ScopeId {
        match self {
            FunctionAstRef::Function(f)
            | FunctionAstRef::AsyncFunction(f)
            | FunctionAstRef::Generator(f)
            | FunctionAstRef::AsyncGenerator(f)
            | FunctionAstRef::ClassConstructor(f) => f.scope_id(),
            FunctionAstRef::Arrow(f) | FunctionAstRef::AsyncArrow(f) => f.scope_id(),
        }
    }

    /// \[\[FormalParameters]].
    #[inline]
    pub(crate) fn formal_parameters(&self) -> &'ast FormalParameters<'ast> {
        match self {
            Self::Function(f)
            | Self::AsyncFunction(f)
            | Self::Generator(f)
            | Self::AsyncGenerator(f)
            | Self::ClassConstructor(f) => f.params.as_ref(),
            Self::Arrow(f) | Self::AsyncArrow(f) => f.params.as_ref(),
        }
    }

    /// \[\[ECMAScriptCode]]
    #[inline]
    pub(crate) fn ecmascript_code(&self) -> &'ast FunctionBody<'ast> {
        match self {
            Self::Function(f)
            | Self::AsyncFunction(f)
            | Self::Generator(f)
            | Self::AsyncGenerator(f)
            | Self::ClassConstructor(f) => f
                .body
                .as_ref()
                .expect("ECMAScriptFunction cannot refer to a TypeScript function declaration"),
            FunctionAstRef::Arrow(f) | FunctionAstRef::AsyncArrow(f) => f.body.as_ref(),
        }
    }

    #[inline]
    pub(crate) fn is_async(&self) -> bool {
        matches!(
            self,
            Self::AsyncFunction(_) | Self::AsyncGenerator(_) | Self::AsyncArrow(_)
        )
    }

    #[inline]
    pub(crate) fn is_concise_body(&self) -> bool {
        match self {
            FunctionAstRef::Arrow(f) | FunctionAstRef::AsyncArrow(f) => f.expression,
            _ => false,
        }
    }

    #[inline]
    pub(crate) fn is_generator(&self) -> bool {
        matches!(self, Self::Generator(_) | Self::AsyncGenerator(_))
    }

    pub(crate) fn kind(&self) -> FunctionKind {
        match self {
            Self::Function(_) => FunctionKind::Function,
            Self::AsyncFunction(_) => FunctionKind::AsyncFunction,
            Self::Generator(_) => FunctionKind::Generator,
            Self::AsyncGenerator(_) => FunctionKind::AsyncGenerator,
            Self::ClassConstructor(_) => FunctionKind::ClassConstructor,
            Self::Arrow(_) => FunctionKind::Arrow,
            Self::AsyncArrow(_) => FunctionKind::AsyncArrow,
        }
    }

    pub(crate) fn span(&self) -> Span {
        match self {
            Self::Function(f)
            | Self::AsyncFunction(f)
            | Self::Generator(f)
            | Self::AsyncGenerator(f)
            | Self::ClassConstructor(f) => f.span,
            Self::Arrow(f) | Self::AsyncArrow(f) => f.span,
        }
    }
}

/// ## [10.2 ECMAScript Function Objects](https://tc39.es/ecma262/#sec-ecmascript-function-objects)
#[derive(Debug, Bindable)]
pub(crate) struct ECMAScriptFunctionObjectHeapData<'a> {
    /// \[\[Environment]]
    pub(crate) environment: Environment<'a>,

    /// \[\[PrivateEnvironment]]
    pub(crate) private_environment: Option<PrivateEnvironment<'a>>,

    /// The kind of \[\[ECMAScriptCode]]; the code itself is compiled into
    /// the function's Executable.
    pub(crate) kind: FunctionKind,

    /// \[\[ConstructorKind]]
    /// \[\[IsClassConstructor]]
    pub(crate) constructor_status: ConstructorStatus,

    /// \[\[Realm]]
    pub(crate) realm: Realm<'a>,

    /// \[\[ScriptOrModule]]
    pub(crate) script_or_module: ScriptOrModule<'a>,

    /// \[\[ThisMode]]
    pub(crate) this_mode: ThisMode,

    /// \[\[Strict]]
    pub(crate) strict: bool,

    /// \[\[HomeObject]]
    pub(crate) home_object: Option<Object<'a>>,

    ///  \[\[SourceText]]
    pub(crate) source_text: Span,

    /// \[\[SourceCode]]
    ///
    /// Nova specific addition: This SourceCode is where \[\[SourceText]]
    /// refers to.
    pub(crate) source_code: SourceCode<'a>,
    // TODO: [[Fields]],  [[PrivateMethods]], [[ClassFieldInitializerName]]
}

pub(crate) struct OrdinaryFunctionCreateParams<'gc> {
    pub(crate) function_prototype: Option<Object<'gc>>,
    pub(crate) source_code: Option<SourceCode<'gc>>,
    pub(crate) template: FunctionTemplate<'gc>,
    pub(crate) lexical_this: bool,
    pub(crate) env: Environment<'gc>,
    pub(crate) private_env: Option<PrivateEnvironment<'gc>>,
}

impl<'a> ECMAScriptFunction<'a> {
    /// Returns true if this function executes in strict mode.
    pub fn is_strict(self, agent: &Agent) -> bool {
        self.get(agent).ecmascript_function.strict
    }

    #[inline]
    pub(crate) fn get_executable(self, agent: &Agent) -> Executable<'a> {
        self.get(agent)
            .compiled_bytecode
            .expect("ECMAScript functions are created with their compiled code")
            .unbind()
    }

    /// ### \[\[Realm]]
    pub fn realm(self, agent: &Agent) -> Realm<'a> {
        self.get(agent).ecmascript_function.realm
    }

    /// Returns the kind of this function's code.
    #[inline]
    pub(crate) fn get_kind(self, agent: &Agent) -> FunctionKind {
        self.get(agent).ecmascript_function.kind
    }

    /// Returns `true` if the ECMAScript function is a constructor.
    ///
    /// ## Examples
    ///
    /// ```javascript
    /// const f1 = () => {}; // returns false
    /// const f2 = function() {}; // returns true
    /// const f3 = class { constructor() {} }; // returns true
    /// ```
    pub fn is_constructor(self, agent: &Agent) -> bool {
        // An ECMAScript function has the [[Construct]] slot if its constructor
        // status is something other than non-constructor.
        self.get(agent).ecmascript_function.constructor_status != ConstructorStatus::NonConstructor
    }
}

impl<'a> FunctionInternalProperties<'a> for ECMAScriptFunction<'a> {
    fn get_name(self, agent: &Agent) -> &String<'a> {
        self.get(agent)
            .name
            .as_ref()
            .unwrap_or(&String::EMPTY_STRING)
    }

    fn get_length(self, agent: &Agent) -> u8 {
        self.get(agent).length
    }

    #[inline(always)]
    fn get_function_backing_object(self, agent: &Agent) -> Option<OrdinaryObject<'static>> {
        self.get(agent).object_index.unbind()
    }

    fn set_function_backing_object(
        self,
        agent: &mut Agent,
        backing_object: OrdinaryObject<'static>,
    ) {
        assert!(
            self.get_mut(agent)
                .object_index
                .replace(backing_object.unbind())
                .is_none()
        );
    }

    fn function_prototype(self, agent: &Agent) -> Option<Object<'static>> {
        if let Some(object_index) = self.get_backing_object(agent) {
            object_index.internal_prototype(agent)
        } else {
            let realm = self.get(agent).ecmascript_function.realm;
            let intrinsics = realm.get(agent).intrinsics();
            let kind = self.get(agent).ecmascript_function.kind;
            let proto = match (kind.is_async(), kind.is_generator()) {
                (false, false) => intrinsics.function_prototype().into(),
                (false, true) => intrinsics.generator_function_prototype().into(),
                (true, false) => intrinsics.async_function_prototype().into(),
                (true, true) => intrinsics.async_generator_function_prototype().into(),
            };
            Some(proto)
        }
    }

    /// ### [10.2.1 \[\[Call\]\] ( thisArgument, argumentsList )](https://tc39.es/ecma262/#sec-call)
    ///
    /// The \[\[Call]] internal method of an ECMAScript function object `F`
    /// takes arguments `thisArgument` (an ECMAScript language value) and
    /// `argumentsList` (a List of ECMAScript language values) and returns
    /// either a normal completion containing an ECMAScript language value or a
    /// throw completion.
    fn function_call<'gc>(
        self,
        agent: &mut Agent,
        this_argument: Value,
        arguments_list: ArgumentsList,
        gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        agent.check_call_depth(gc.nogc()).unbind()?;
        let f = self.bind(gc.nogc());
        let mut id = 0;
        ndt::javascript_call_start!(|| {
            let args = create_name_and_id(agent, f);
            id = args.1;
            args
        });

        let arguments_list = arguments_list.bind(gc.nogc());

        // 1. Let callerContext be the running execution context.
        let _ = agent.running_execution_context();
        let call_environment = call_environment(agent, f);
        if call_environment != CallEnvironment::Function {
            // OPTIMISATION: Only the function's own code asks for the this
            // binding of its Function Environment Record, so the record is
            // never observed: keep the this value of OrdinaryCallBindThis in
            // the execution context, and the function's bindings, if any, in
            // a Declarative Environment Record.
            prepare_for_call_without_function_environment(
                agent,
                f,
                this_argument,
                call_environment == CallEnvironment::Declarative,
                gc.nogc(),
            );
            let result =
                ordinary_call_evaluate_body(agent, f.unbind(), arguments_list.unbind(), gc);
            let _callee_context = agent.pop_execution_context();
            ndt::javascript_call_done!(|| id);
            return result;
        }
        // 2. Let calleeContext be PrepareForOrdinaryCall(F, undefined).
        let callee_context = prepare_for_ordinary_call(agent, f, None, gc.nogc());
        // This is step 4. or OrdinaryCallBindThis:
        // "Let localEnv be the LexicalEnvironment of calleeContext."
        let local_env = callee_context
            .ecmascript_code
            .as_ref()
            .unwrap()
            .lexical_environment
            .bind(gc.nogc());
        // 3. Assert: calleeContext is now the running execution context.
        // assert!(core::ptr::eq(agent.running_execution_context(), callee_context));
        // 4. If F.[[IsClassConstructor]] is true, then
        if f.get(agent)
            .ecmascript_function
            .constructor_status
            .is_class_constructor()
        {
            // a. Let error be a newly created TypeError object.
            // b. NOTE: error is created in calleeContext with F's associated Realm Record.
            let error = agent.throw_exception_with_static_message(
                ExceptionType::TypeError,
                "class constructors must be invoked with 'new'",
                gc.nogc(),
            );
            // c. Remove calleeContext from the execution context stack and restore callerContext as the running execution context.
            agent.pop_execution_context();
            // d. Return ThrowCompletion(error).
            return Err(error.unbind());
        }
        let Environment::Function(local_env) = local_env else {
            panic!("localEnv is not a Function Environment Record");
        };
        // 5. Perform OrdinaryCallBindThis(F, calleeContext, thisArgument).
        ordinary_call_bind_this(agent, f, local_env, this_argument, gc.nogc());
        // 6. Let result be Completion(OrdinaryCallEvaluateBody(F, argumentsList)).
        let result = ordinary_call_evaluate_body(agent, f.unbind(), arguments_list.unbind(), gc);
        // 7. Remove calleeContext from the execution context stack and restore callerContext as the running execution context.
        // NOTE: calleeContext must not be destroyed if it is suspended and retained for later resumption by an accessible Generator.
        let _callee_context = agent.pop_execution_context();
        ndt::javascript_call_done!(|| id);
        // 8. If result is a return completion, return result.[[Value]].
        // 9. ReturnIfAbrupt(result).
        // 10. Return undefined.
        result
    }

    fn function_construct<'gc>(
        self,
        agent: &mut Agent,
        arguments: ArgumentsList,
        new_target: Function,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Object<'gc>> {
        let mut f = self.bind(gc.nogc());
        let mut id = 0;
        ndt::javascript_constructor_start!(|| {
            let args = create_name_and_id(agent, f);
            id = args.1;
            args
        });

        let mut new_target = new_target.bind(gc.nogc());
        let mut arguments_list = arguments.bind(gc.nogc());
        // 2. Let kind be F.[[ConstructorKind]].
        let is_base = !f
            .get(agent)
            .ecmascript_function
            .constructor_status
            .is_derived_class();
        // 3. If kind is BASE, then
        let this_argument = if is_base {
            let scoped_self_fn = f.scope(agent, gc.nogc());
            let scoped_new_target = new_target.scope(agent, gc.nogc());
            // a. Let thisArgument be ? OrdinaryCreateFromConstructor(newTarget, "%Object.prototype%").
            let unbound_new_target = new_target.unbind();
            let mut args = arguments_list.unbind();
            let this_argument = args
                .with_scoped(
                    agent,
                    |agent, _, gc| {
                        ordinary_create_from_constructor(
                            agent,
                            unbound_new_target,
                            ProtoIntrinsics::Object,
                            gc,
                        )
                    },
                    gc.reborrow(),
                )
                .unbind()?
                .bind(gc.nogc());
            f = scoped_self_fn.get(agent).bind(gc.nogc());
            new_target = scoped_new_target.get(agent).bind(gc.nogc());
            arguments_list = args.bind(gc.nogc());
            Some(this_argument)
        } else {
            None
        };

        // 4. Let calleeContext be PrepareForOrdinaryCall(F, newTarget).
        let callee_context =
            prepare_for_ordinary_call(agent, f, Some(new_target.into()), gc.nogc());
        // 7. Let constructorEnv be the LexicalEnvironment of calleeContext.
        let constructor_env = callee_context
            .ecmascript_code
            .as_ref()
            .unwrap()
            .lexical_environment
            .bind(gc.nogc());
        let Environment::Function(constructor_env) = constructor_env else {
            panic!("constructorEnv is not a Function Environment Record");
        };
        // 5. Assert: calleeContext is now the running execution context.
        // assert!(core::ptr::eq(agent.running_execution_context(), callee_context));

        // 6. If kind is base, then
        if is_base {
            // a. Perform OrdinaryCallBindThis(F, calleeContext, thisArgument).
            ordinary_call_bind_this(
                agent,
                f,
                constructor_env,
                this_argument.unwrap().into(),
                gc.nogc(),
            );
            // b. Let initializeResult be Completion(InitializeInstanceElements(thisArgument, F)).
            // c. If initializeResult is an abrupt completion, then
            //    i. Remove calleeContext from the execution context stack and
            //       restore callerContext as the running execution context.
            //    ii. Return ? initializeResult.
            // TODO: Classes.
        }

        let scoped_constructor_env = constructor_env.scope(agent, gc.nogc());
        let scoped_this_argument = this_argument.map(|f| f.scope(agent, gc.nogc()));

        // 8. Let result be Completion(OrdinaryCallEvaluateBody(F, argumentsList)).
        let result =
            ordinary_call_evaluate_body(agent, f.unbind(), arguments_list.unbind(), gc.reborrow());
        // 9. Remove calleeContext from the execution context stack and restore
        //    callerContext as the running execution context.
        let _callee_context = agent.pop_execution_context();
        // 10. If result is a return completion, then
        // 11. Else,
        //   a. ReturnIfAbrupt(result).
        let value = result.unbind()?.bind(gc.nogc());
        // 10. If result is a return completion, then
        //   a. If result.[[Value]] is an Object, return result.[[Value]].
        let result = if let Ok(value) = Object::try_from(value) {
            Ok(value.unbind())
        } else
        //   b. If kind is base, return thisArgument.
        if is_base {
            Ok(scoped_this_argument.unwrap().get(agent))
        } else
        //   c. If result.[[Value]] is not undefined, throw a TypeError exception.
        if !value.is_undefined() {
            let message = format!(
                "derived class constructor returned invalid value {}",
                value
                    .unbind()
                    .string_repr(agent, gc.reborrow())
                    .to_string_lossy_(agent)
            );
            let message = String::from_string(agent, message, gc.nogc());
            Err(agent.throw_exception_with_message(
                ExceptionType::TypeError,
                message.unbind(),
                gc.into_nogc(),
            ))
        } else {
            // 12. Let thisBinding be ? constructorEnv.GetThisBinding().
            // 13. Assert: thisBinding is an Object.
            let Ok(this_binding) = Object::try_from(
                scoped_constructor_env
                    .get(agent)
                    .get_this_binding(agent, gc.into_nogc())?,
            ) else {
                unreachable!();
            };

            // 14. Return thisBinding.
            Ok(this_binding)
        };
        ndt::javascript_constructor_done!(|| id);
        result
    }
}

#[inline(never)]
fn create_name_and_id<'a>(agent: &'a Agent, f: ECMAScriptFunction<'a>) -> (Cow<'a, str>, u64) {
    let id = f
        .get(agent)
        .compiled_bytecode
        .map_or(0, |b| b.get(agent).instructions.as_ptr() as u64);
    let name = f.get_name(agent).to_string_lossy_(agent);
    (name, id)
}

/// ### [10.2.1.1 PrepareForOrdinaryCall ( F, newTarget )](https://tc39.es/ecma262/#sec-prepareforordinarycall)
///
/// The abstract operation PrepareForOrdinaryCall takes arguments `F` (an
/// ECMAScript function object) and newTarget (an Object or undefined) and
/// returns an execution context.
pub(crate) fn prepare_for_ordinary_call<'a>(
    agent: &'a mut Agent,
    f: ECMAScriptFunction,
    new_target: Option<Object>,
    gc: NoGcScope,
) -> &'a ExecutionContext {
    let f = f.bind(gc);
    let new_target = new_target.bind(gc);
    let ecmascript_function_object = &f.get(agent).ecmascript_function;
    let private_environment = ecmascript_function_object.private_environment.bind(gc);
    let is_strict_mode = ecmascript_function_object.strict;
    let script_or_module = ecmascript_function_object.script_or_module;
    let source_code = ecmascript_function_object.source_code;
    // 1. Let callerContext be the running execution context.
    let _caller_context = agent.running_execution_context();
    // 4. Let calleeRealm be F.[[Realm]].
    let callee_realm = ecmascript_function_object.realm;
    // 7. Let localEnv be NewFunctionEnvironment(F, newTarget).
    let local_env = new_function_environment(agent, f, new_target, gc);
    // 2. Let calleeContext be a new ECMAScript code execution context.
    let callee_context = ExecutionContext {
        // 8. Set the LexicalEnvironment of calleeContext to localEnv.
        // 9. Set the VariableEnvironment of calleeContext to localEnv.
        // 10. Set the PrivateEnvironment of calleeContext to F.[[PrivateEnvironment]].
        ecmascript_code: Some(ECMAScriptCodeEvaluationState {
            lexical_environment: Environment::Function(local_env.unbind()),
            variable_environment: Environment::Function(local_env.unbind()),
            private_environment: private_environment.unbind(),
            is_strict_mode,
            this_value: None,
            source_code: source_code.unbind(),
        }),
        // 3. Set the Function of calleeContext to F.
        function: Some(f.unbind().into()),
        // 5. Set the Realm of calleeContext to calleeRealm.
        realm: callee_realm.unbind(),
        // 6. Set the ScriptOrModule of calleeContext to F.[[ScriptOrModule]].
        script_or_module: Some(script_or_module.unbind()),
    };
    // 11. If callerContext is not already suspended, suspend callerContext.
    // 12. Push calleeContext onto the execution context stack; calleeContext is now the running execution context.
    agent.push_execution_context(callee_context);
    // 13. NOTE: Any exception objects produced after this point are associated with calleeRealm.
    // 14. Return calleeContext.
    agent.running_execution_context()
}

/// The environment a \[\[Call]] of F creates: a class constructor's call
/// throws in a Function Environment Record, every other function's comes from
/// its compiled body.
#[inline]
pub(crate) fn call_environment(agent: &Agent, f: ECMAScriptFunction) -> CallEnvironment {
    let data = f.get(agent);
    if data
        .ecmascript_function
        .constructor_status
        .is_class_constructor()
    {
        return CallEnvironment::Function;
    }
    data.compiled_bytecode
        .map_or(CallEnvironment::Function, |executable| {
            executable.get(agent).call_environment
        })
}

/// PrepareForOrdinaryCall for a function whose call needs no Function
/// Environment Record: the callee context's LexicalEnvironment and
/// VariableEnvironment are F.[[Environment]], or a new Declarative Environment
/// Record in it for the function's bindings. A function that does not take
/// this lexically gets the this value that OrdinaryCallBindThis would bind:
/// thisArgument for a strict function; for one that is not, the global this
/// value of its realm in place of undefined and null, and an object made of
/// any other primitive.
// Part of every such call, whichever way it is made: inlined into both.
#[inline(always)]
pub(crate) fn prepare_for_call_without_function_environment(
    agent: &mut Agent,
    f: ECMAScriptFunction,
    this_argument: Value,
    declarative_environment: bool,
    gc: NoGcScope,
) {
    let f = f.bind(gc);
    let outer_env = f.get(agent).ecmascript_function.environment.bind(gc);
    let environment = if declarative_environment {
        Environment::Declarative(new_declarative_environment(agent, Some(outer_env), gc))
    } else {
        outer_env
    };
    let ecmascript_function_object = &f.get(agent).ecmascript_function;
    let this_mode = ecmascript_function_object.this_mode;
    let this_value = match this_mode {
        // 2. If thisMode is LEXICAL, return UNUSED.
        ThisMode::Lexical => None,
        // 6. a. If thisArgument is either undefined or null, then
        //       iii. Let thisValue be globalEnv.[[GlobalThisValue]].
        ThisMode::Global if this_argument.is_undefined() || this_argument.is_null() => {
            let global_env = agent
                .get_realm_record_by_id(ecmascript_function_object.realm)
                .global_env
                .unwrap();
            Some(global_env.get_this_binding(agent).into())
        }
        // 5. If thisMode is STRICT, then let thisValue be thisArgument; and
        // an object is itself in 6.b. A primitive is made an object below.
        ThisMode::Strict | ThisMode::Global => Some(this_argument),
    }
    .map(|value: Value| CallThisValue {
        value: value.unbind(),
        variable_environment: environment.unbind(),
    });
    let callee_context = ExecutionContext {
        ecmascript_code: Some(ECMAScriptCodeEvaluationState {
            lexical_environment: environment.unbind(),
            variable_environment: environment.unbind(),
            private_environment: ecmascript_function_object.private_environment.unbind(),
            is_strict_mode: ecmascript_function_object.strict,
            this_value,
            source_code: ecmascript_function_object.source_code.unbind(),
        }),
        function: Some(f.unbind().into()),
        realm: ecmascript_function_object.realm.unbind(),
        script_or_module: Some(ecmascript_function_object.script_or_module.unbind()),
    };
    agent.push_execution_context(callee_context);
    if this_mode == ThisMode::Global
        && !this_argument.is_undefined()
        && !this_argument.is_null()
        && !this_argument.is_object()
    {
        // 6. b. i. Let thisValue be ! ToObject(thisArgument).
        //       ii. NOTE: ToObject produces wrapper objects using calleeRealm,
        //           which is the running one now.
        let wrapper = to_object(agent, this_argument, gc).unwrap();
        agent.set_running_call_this_value(wrapper.unbind().into());
    }
}

/// ### [10.2.1.2 OrdinaryCallBindThis ( F, calleeContext, thisArgument )](https://tc39.es/ecma262/#sec-ordinarycallbindthis)
///
/// The abstract operation OrdinaryCallBindThis takes arguments `F` (an
/// ECMAScript function object), calleeContext (an execution context), and
/// `thisArgument` (an ECMAScript language value) and returns UNUSED.
///
/// Note: calleeContext is replaced by localEnv which is the only thing it is
/// truly used for.
pub(crate) fn ordinary_call_bind_this(
    agent: &mut Agent,
    f: ECMAScriptFunction,
    local_env: FunctionEnvironment,
    this_argument: Value,
    gc: NoGcScope,
) {
    let function_heap_data = &f.get(agent).ecmascript_function;
    // 1. Let thisMode be F.[[ThisMode]].
    let this_mode = function_heap_data.this_mode;
    // 2. If thisMode is LEXICAL, return UNUSED.
    if this_mode == ThisMode::Lexical {
        return;
    }
    // 3. Let calleeRealm be F.[[Realm]].
    let callee_realm = function_heap_data.realm.bind(gc);
    // 4. Let localEnv be the LexicalEnvironment of calleeContext.
    // 5. If thisMode is STRICT, then
    let this_value = if this_mode == ThisMode::Strict {
        // a. Let thisValue be thisArgument.
        this_argument
    } else {
        // 6. Else,
        // a. If thisArgument is either undefined or null, then
        if this_argument == Value::Undefined || this_argument == Value::Null {
            // i. Let globalEnv be calleeRealm.[[GlobalEnv]].
            let global_env = agent.get_realm_record_by_id(callee_realm).global_env;
            // ii. Assert: globalEnv is a Global Environment Record.
            let global_env = global_env.unwrap();
            // iii. Let thisValue be globalEnv.[[GlobalThisValue]].
            global_env.get_this_binding(agent).into()
        } else {
            // b. Else,
            // i. Let thisValue be ! ToObject(thisArgument).
            to_object(agent, this_argument, gc).unwrap().into()
            // ii. NOTE: ToObject produces wrapper objects using calleeRealm.
        }
    };
    // 7. Assert: localEnv is a Function Environment Record.
    // 8. Assert: The next step never returns an abrupt completion because localEnv.[[ThisBindingStatus]] is not INITIALIZED.
    assert_ne!(
        local_env.get_this_binding_status(agent),
        ThisBindingStatus::Initialized
    );
    // 9. Perform ! localEnv.BindThisValue(thisValue).
    local_env.bind_this_value(agent, this_value, gc).unwrap();
    // 10. Return UNUSED.
}

/// ### [10.2.1.3 Runtime Semantics: EvaluateBody](https://tc39.es/ecma262/#sec-runtime-semantics-evaluatebody)
///
/// The syntax-directed operation EvaluateBody takes arguments `functionObject`
/// (an ECMAScript function object) and `argumentsList` (a List of ECMAScript
/// language values) and returns either a normal completion containing an
/// ECMAScript language value or an abrupt completion.
pub(crate) fn evaluate_body<'gc>(
    agent: &mut Agent,
    function_object: ECMAScriptFunction,
    arguments_list: ArgumentsList,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    let function_object = function_object.bind(gc.nogc());
    let kind = function_object.get_kind(agent);
    match (kind.is_generator(), kind.is_async()) {
        (false, true) => {
            // AsyncFunctionBody : FunctionBody
            // 1. Return ? EvaluateAsyncFunctionBody of AsyncFunctionBody with arguments functionObject and argumentsList.
            // AsyncConciseBody : ExpressionBody
            // 1. Return ? EvaluateAsyncConciseBody of AsyncConciseBody with arguments functionObject and argumentsList.
            Ok(
                evaluate_async_function_body(agent, function_object.unbind(), arguments_list, gc)
                    .into(),
            )
        }
        (false, false) => {
            // FunctionBody : FunctionStatementList
            // 1. Return ? EvaluateFunctionBody of FunctionBody with arguments functionObject and argumentsList.
            // ConciseBody : ExpressionBody
            // 1. Return ? EvaluateConciseBody of ConciseBody with arguments functionObject and argumentsList.
            evaluate_function_body(agent, function_object.unbind(), arguments_list, gc)
        }
        (true, false) => {
            // GeneratorBody : FunctionBody
            // 1. Return ? EvaluateGeneratorBody of GeneratorBody with arguments functionObject and argumentsList.
            evaluate_generator_body(agent, function_object.unbind(), arguments_list, gc)
        }
        // AsyncGeneratorBody : FunctionBody
        // 1. Return ? EvaluateAsyncGeneratorBody of AsyncGeneratorBody with arguments functionObject and argumentsList.
        _ => evaluate_async_generator_body(agent, function_object.unbind(), arguments_list, gc),
    }

    // Initializer :
    // = AssignmentExpression
    // 1. Assert: argumentsList is empty.
    // 2. Assert: functionObject.[[ClassFieldInitializerName]] is not EMPTY.
    // 3. If IsAnonymousFunctionDefinition(AssignmentExpression) is true, then
    // a. Let value be ? NamedEvaluation of Initializer with argument functionObject.[[ClassFieldInitializerName]].
    // 4. Else,
    // a. Let rhs be ? Evaluation of AssignmentExpression.
    // b. Let value be ? GetValue(rhs).
    // 5. Return Completion Record { [[Type]]: RETURN, [[Value]]: value, [[Target]]: EMPTY }.
    // NOTE
    // Even though field initializers constitute a function boundary, calling FunctionDeclarationInstantiation does not have any observable effect and so is omitted.
    // ClassStaticBlockBody : ClassStaticBlockStatementList
    // 1. Assert: argumentsList is empty.
    // 2. Return ? EvaluateClassStaticBlockBody of ClassStaticBlockBody with argument functionObject.
}

/// ### [10.2.1.4 OrdinaryCallEvaluateBody ( F, argumentsList )](https://tc39.es/ecma262/#sec-ordinarycallevaluatebody)
///
/// The abstract operation OrdinaryCallEvaluateBody takes arguments `F` (an
/// ECMAScript function object) and `argumentsList` (a List of ECMAScript
/// language values) and returns either a normal completion containing an
/// ECMAScript language value or an abrupt completion.
pub(crate) fn ordinary_call_evaluate_body<'gc>(
    agent: &mut Agent,
    f: ECMAScriptFunction,
    arguments_list: ArgumentsList,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    // 1. Return ? EvaluateBody of F.[[ECMAScriptCode]] with arguments F and argumentsList.
    evaluate_body(agent, f, arguments_list, gc)
}

/// ### [10.2.3 OrdinaryFunctionCreate ( functionPrototype, sourceText, ParameterList, Body, thisMode, env, privateEnv )](https://tc39.es/ecma262/#sec-ordinaryfunctioncreate)
///
/// The abstract operation OrdinaryFunctionCreate takes arguments
/// functionPrototype (an Object), sourceText (a sequence of Unicode code
/// points), ParameterList (a Parse Node), Body (a Parse Node), thisMode
/// (LEXICAL-THIS or NON-LEXICAL-THIS), env (an Environment Record), and
/// privateEnv (a PrivateEnvironment Record or null) and returns an ECMAScript
/// function object. It is used to specify the runtime creation of a new
/// function with a default \[\[Call\]\] internal method and no
/// \[\[Construct\]\] internal method (although one may be subsequently added
/// by an operation such as MakeConstructor). sourceText is the source text of
/// the syntactic definition of the function to be created.
pub(crate) fn ordinary_function_create<'gc>(
    agent: &mut Agent,
    params: OrdinaryFunctionCreateParams<'gc>,
    gc: NoGcScope<'gc, '_>,
) -> ECMAScriptFunction<'gc> {
    let (source_code, outer_env_is_strict) = if let Some(source_code) = params.source_code {
        (source_code, false)
    } else {
        (
            agent.current_source_code(gc),
            agent.is_evaluating_strict_code(),
        )
    };
    // 7. If the source text matched by Body is strict mode code, let Strict be
    //    true; else let Strict be false.
    let strict = outer_env_is_strict || params.template.has_use_strict_directive;

    // 1. Let internalSlotsList be the internal slots listed in Table 30.
    // 2. Let F be OrdinaryObjectCreate(functionPrototype, internalSlotsList).
    // 3. Set F.[[Call]] to the definition specified in 10.2.1.
    let ecmascript_function = ECMAScriptFunctionObjectHeapData {
        // 13. Set F.[[Environment]] to env.
        environment: params.env.unbind(),
        // 14. Set F.[[PrivateEnvironment]] to privateEnv.
        private_environment: params.private_env.unbind(),
        // 5. Set F.[[FormalParameters]] to ParameterList.
        // 6. Set F.[[ECMAScriptCode]] to Body.
        kind: params.template.kind,
        // 12. Set F.[[IsClassConstructor]] to false.
        constructor_status: ConstructorStatus::NonConstructor,
        // 16. Set F.[[Realm]] to the current Realm Record.
        realm: agent.current_realm(gc),
        // 15. Set F.[[ScriptOrModule]] to GetActiveScriptOrModule().
        script_or_module: get_active_script_or_module(agent, gc).unwrap().unbind(),
        // 9. If thisMode is LEXICAL-THIS, set F.[[ThisMode]] to LEXICAL.
        // 10. Else if Strict is true, set F.[[ThisMode]] to STRICT.
        // 11. Else, set F.[[ThisMode]] to GLOBAL.
        this_mode: if params.lexical_this {
            ThisMode::Lexical
        } else if strict {
            ThisMode::Strict
        } else {
            ThisMode::Global
        },
        // 8. Set F.[[Strict]] to Strict.
        strict,
        // 17. Set F.[[HomeObject]] to undefined.
        home_object: None,
        // 4. Set F.[[SourceText]] to sourceText.
        source_text: params.template.span,
        source_code: source_code.unbind(),
    };

    let mut function = ECMAScriptFunctionHeapData {
        object_index: None,
        length: 0,
        ecmascript_function,
        compiled_bytecode: params.template.executable.unbind(),
        name: None,
    };
    // 18. Set F.[[Fields]] to a new empty List.
    // 19. Set F.[[PrivateMethods]] to a new empty List.
    // 20. Set F.[[ClassFieldInitializerName]] to EMPTY.
    // 21. Let len be the ExpectedArgumentCount of ParameterList.
    let len = params.template.expected_argument_count as usize;
    // 22. Perform SetFunctionLength(F, len).
    function.length = u8::try_from(len).unwrap_or(u8::MAX);
    // A function whose prototype is not %Function.prototype%, or whose length
    // does not fit the heap data, keeps its properties in a backing object
    // from the start. SetFunctionName adds "name" to it.
    let function_prototype = params.function_prototype.filter(|function_prototype| {
        *function_prototype
            != agent
                .current_realm_record()
                .intrinsics()
                .function_prototype()
                .into()
    });
    if function_prototype.is_some() || len > usize::from(u8::MAX) {
        let prototype = function_prototype.unwrap_or_else(|| {
            agent
                .current_realm_record()
                .intrinsics()
                .function_prototype()
                .into()
        });
        let length_entry = ObjectEntry {
            key: BUILTIN_STRING_MEMORY.length.into(),
            value: ObjectEntryPropertyDescriptor::Data {
                value: Value::try_from(len as f64).expect("a parameter count is a safe integer"),
                writable: false,
                enumerable: false,
                configurable: true,
            },
        };
        function.object_index = Some(
            OrdinaryObject::create_object(agent, Some(prototype), &[length_entry])
                .expect("Should perform GC here"),
        );
    }
    // 23. Return F.
    agent.heap.create(function)
}

/// ### [15.1.5 Static Semantics: ExpectedArgumentCount](https://tc39.es/ecma262/#sec-static-semantics-expectedargumentcount)
fn expected_arguments_count(params: &FormalParameters) -> usize {
    // FormalParameterList : FormalParameterList , FormalParameter

    // 1. Let count be the ExpectedArgumentCount of FormalParameterList.
    let mut count = 0;
    // 2. If HasInitializer of FormalParameterList is true or HasInitializer of
    //    FormalParameter is true, return count.
    // 3. Return count + 1.
    for param in params.items.iter() {
        if param.initializer.is_some() {
            // FormalParameterList : FormalParameter
            // 1. If HasInitializer of FormalParameter is true, return 0.
            break;
        }
        count += 1;
    }
    count
}

/// ### [10.2.5 MakeConstructor ( F \[ , writablePrototype \[ , prototype \] \] )](https://tc39.es/ecma262/#sec-makeconstructor)
/// The abstract operation MakeConstructor takes argument F (an ECMAScript
/// function object or a built-in function object) and optional arguments
/// writablePrototype (a Boolean) and prototype (an Object) and returns
/// UNUSED. It converts F into a constructor.
pub(crate) fn make_constructor<'a>(
    agent: &mut Agent,
    function: impl FunctionInternalProperties<'a>,
    writable_prototype: Option<bool>,
    prototype: Option<OrdinaryObject>,
    gc: NoGcScope,
) {
    // 4. If writablePrototype is not present, set writablePrototype to true.
    let writable_prototype = writable_prototype.unwrap_or(true);
    match function.into() {
        Function::BoundFunction(_) => unreachable!(),
        // 1. If F is an ECMAScript function object, then
        Function::ECMAScriptFunction(idx) => {
            let data = idx.get_mut(agent);
            // a. Assert: IsConstructor(F) is false.
            debug_assert!(!data.ecmascript_function.constructor_status.is_constructor());
            // b. Assert: F is an extensible object that does not have a "prototype" own property.
            // c. Set F.[[Construct]] to the definition specified in 10.2.2.
            // 3. Set F.[[ConstructorKind]] to BASE.
            data.ecmascript_function.constructor_status = ConstructorStatus::ConstructorFunction;
        }
        Function::BuiltinFunction(_) => {
            // 2. Else,
            // a. Set F.[[Construct]] to the definition specified in 10.3.2.
        }
        Function::BuiltinConstructorFunction(_)
        | Function::BuiltinPromiseResolvingFunction(_)
        | Function::BuiltinPromiseFinallyFunction(_)
        | Function::BuiltinProxyRevokerFunction => unreachable!(),
    }
    // 5. If prototype is not present, then
    let prototype = prototype.unwrap_or_else(|| {
        // a. Set prototype to OrdinaryObjectCreate(%Object.prototype%).
        let prototype = OrdinaryObject::try_from(ordinary_object_create_with_intrinsics(
            agent,
            ProtoIntrinsics::Object,
            None,
            gc,
        ))
        .unwrap();
        let f: Value = function.into();
        // b. Perform ! DefinePropertyOrThrow(
        prototype
            .property_storage()
            .set(
                agent,
                // prototype,
                prototype.into(),
                // "constructor",
                BUILTIN_STRING_MEMORY.constructor.into(),
                // PropertyDescriptor {
                PropertyDescriptor {
                    // [[Value]]: F,
                    value: Some(f.unbind()),
                    // [[Writable]]: writablePrototype,
                    writable: Some(writable_prototype),
                    // [[Enumerable]]: false,
                    enumerable: Some(false),
                    // [[Configurable]]: true
                    configurable: Some(true),
                    ..Default::default()
                },
                gc,
            )
            .expect("Failed to allocate memory for constructor");
        // }).
        prototype
    });
    let backing_object = function
        .get_backing_object(agent)
        .unwrap_or_else(|| function.create_backing_object(agent));
    // 6. Perform ! DefinePropertyOrThrow(
    backing_object
        .property_storage()
        .set(
            agent,
            // F,
            function.into(),
            // "prototype",
            BUILTIN_STRING_MEMORY.prototype.into(),
            // PropertyDescriptor {
            PropertyDescriptor {
                // [[Value]]: prototype,
                value: Some(prototype.unbind().into()),
                // [[Writable]]: writablePrototype,
                writable: Some(writable_prototype),
                // [[Enumerable]]: false,
                enumerable: Some(false),
                // [[Configurable]]: false
                configurable: Some(false),
                ..Default::default()
            },
            gc,
        )
        .expect("Failed to allocate memory for constructor");
    // }).
    // 7. Return UNUSED.
}

/// ### [10.2.7 MakeMethod ( F, homeObject )](https://tc39.es/ecma262/#sec-makemethod)
///
/// The abstract operation MakeMethod takes arguments F (an ECMAScript function
/// object) and homeObject (an Object) and returns unused. It configures F as a
/// method.
#[inline]
pub(crate) fn make_method(agent: &mut Agent, f: ECMAScriptFunction, home_object: Object) {
    // 1. Assert: homeObject is an ordinary object.
    // 2. Set F.[[HomeObject]] to homeObject.
    f.get_mut(agent).ecmascript_function.home_object = Some(home_object.unbind());
    // 3. Return unused.
}

pub(crate) enum SetFunctionNamePrefix {
    Get,
    Set,
    Bound,
}

impl SetFunctionNamePrefix {
    fn into_str(self) -> &'static str {
        match self {
            SetFunctionNamePrefix::Get => "get ",
            SetFunctionNamePrefix::Set => "set ",
            SetFunctionNamePrefix::Bound => "bound ",
        }
    }
}

fn prefix_into_str(prefix: Option<SetFunctionNamePrefix>) -> &'static str {
    match prefix {
        Some(p) => p.into_str(),
        None => "",
    }
}

/// ### [10.2.9 SetFunctionName ( F, name \[ , prefix \] )](https://tc39.es/ecma262/#sec-setfunctionname)
/// The abstract operation SetFunctionName takes arguments F (a function
/// object) and name (a property key or Private Name) and optional argument
/// prefix (a String) and returns UNUSED. It adds a "name" property to F.
pub(crate) fn set_function_name<'a>(
    agent: &mut Agent,
    function: impl Into<Function<'a>>,
    name: PropertyKey,
    prefix: Option<SetFunctionNamePrefix>,
    gc: NoGcScope,
) {
    // 2. If name is a Symbol, then
    // 5. If prefix is present, then
    // a. Set name to the string-concatenation of prefix, the code unit 0x0020 (SPACE), and name.
    let name: String = match name {
        PropertyKey::Symbol(s) => {
            // a. Let description be name's [[Description]] value.
            // b. If description is undefined, set name to the empty String.
            // c. Else, set name to the string-concatenation of "[", description, and "]".
            if let Some(descriptor) = s.description(agent) {
                let descriptor = descriptor.to_string_lossy_(agent);
                String::from_string(
                    agent,
                    format!("{}[{descriptor}]", prefix_into_str(prefix)),
                    gc,
                )
            } else if let Some(prefix) = prefix {
                // Step 5 applies even when description is None
                String::from_static_str(agent, prefix.into_str(), gc)
            } else {
                String::EMPTY_STRING
            }
        }

        PropertyKey::Integer(integer) => String::from_string(
            agent,
            format!("{}{}", prefix_into_str(prefix), integer.into_i64()),
            gc,
        ),
        PropertyKey::SmallString(str) => {
            if let Some(prefix) = prefix {
                String::from_string(
                    agent,
                    format!("{}{}", prefix.into_str(), str.to_string_lossy()),
                    gc,
                )
            } else {
                str.into()
            }
        }
        PropertyKey::String(str) => {
            if let Some(prefix) = prefix {
                String::from_string(
                    agent,
                    format!("{}{}", prefix.into_str(), str.to_string_lossy(agent)),
                    gc,
                )
            } else {
                str.into()
            }
        }
        // 3. Else if name is a Private Name, then
        // a. Set name to name.[[Description]].
        PropertyKey::PrivateName(p) => p
            .get_description(agent, gc)
            .expect("Should always find PrivateName in scope when calling SetFunctionName"),
    };

    match function.into() {
        Function::BoundFunction(idx) => {
            let function = idx.get_mut(agent);
            // Note: It's possible that the bound function targeted a function
            // with a non-default prototype. In that case, object_index is
            // already set, and "name" is a property of it.
            assert!(function.name.is_none());
            function.name = Some(name.unbind());
            if let Some(backing_object) = function.object_index {
                backing_object
                    .property_storage()
                    .set(
                        agent,
                        idx.into(),
                        BUILTIN_STRING_MEMORY.name.into(),
                        PropertyDescriptor {
                            value: Some(name.unbind().into()),
                            writable: Some(false),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..Default::default()
                        },
                        gc,
                    )
                    .expect("Failed to allocate memory for the function name");
            }
        }
        Function::BuiltinFunction(_idx) => unreachable!(),
        Function::ECMAScriptFunction(idx) => {
            let function = idx.get_mut(agent);
            // 1. Assert: F is an extensible object that does not have a "name" own property.
            assert!(function.name.is_none());
            // 6. Perform ! DefinePropertyOrThrow(F, "name", PropertyDescriptor { [[Value]]: name, [[Writable]]: false, [[Enumerable]]: false, [[Configurable]]: true }).
            function.name = Some(name.unbind());
            if let Some(backing_object) = function.object_index {
                backing_object
                    .property_storage()
                    .set(
                        agent,
                        idx.into(),
                        BUILTIN_STRING_MEMORY.name.into(),
                        PropertyDescriptor {
                            value: Some(name.unbind().into()),
                            writable: Some(false),
                            enumerable: Some(false),
                            configurable: Some(true),
                            ..Default::default()
                        },
                        gc,
                    )
                    .expect("Failed to allocate memory for the function name");
            }
            // 7. Return UNUSED.
        }
        Function::BuiltinConstructorFunction(_)
        | Function::BuiltinPromiseResolvingFunction(_)
        | Function::BuiltinPromiseFinallyFunction(_)
        | Function::BuiltinProxyRevokerFunction => unreachable!(),
    }
}

impl HeapMarkAndSweep for ECMAScriptFunction<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.ecmascript_functions.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.ecmascript_functions.shift_index(&mut self.0);
    }
}

impl HeapSweepWeakReference for ECMAScriptFunction<'static> {
    fn sweep_weak_reference(self, compactions: &CompactionLists) -> Option<Self> {
        compactions
            .ecmascript_functions
            .shift_weak_index(self.0)
            .map(Self)
    }
}

impl<'a> CreateHeapData<ECMAScriptFunctionHeapData<'a>, ECMAScriptFunction<'a>> for Heap {
    fn create(&mut self, data: ECMAScriptFunctionHeapData<'a>) -> ECMAScriptFunction<'a> {
        self.ecmascript_functions.push(data.unbind());
        self.alloc_counter += core::mem::size_of::<ECMAScriptFunctionHeapData<'static>>();

        ECMAScriptFunction(BaseIndex::last(&self.ecmascript_functions))
    }
}

impl HeapMarkAndSweep for ECMAScriptFunctionHeapData<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            object_index,
            length: _,
            ecmascript_function,
            compiled_bytecode,
            name,
        } = self;
        let ECMAScriptFunctionObjectHeapData {
            environment,
            private_environment,
            kind: _,
            constructor_status: _,
            realm,
            script_or_module,
            this_mode: _,
            strict: _,
            home_object,
            source_text: _,
            source_code,
        } = ecmascript_function;
        object_index.mark_values(queues);
        compiled_bytecode.mark_values(queues);
        name.mark_values(queues);
        environment.mark_values(queues);
        private_environment.mark_values(queues);
        realm.mark_values(queues);
        script_or_module.mark_values(queues);
        home_object.mark_values(queues);
        source_code.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            object_index,
            length: _,
            ecmascript_function,
            compiled_bytecode,
            name,
        } = self;
        let ECMAScriptFunctionObjectHeapData {
            environment,
            private_environment,
            kind: _,
            constructor_status: _,
            realm,
            script_or_module,
            this_mode: _,
            strict: _,
            home_object,
            source_text: _,
            source_code,
        } = ecmascript_function;
        object_index.sweep_values(compactions);
        compiled_bytecode.sweep_values(compactions);
        name.sweep_values(compactions);
        environment.sweep_values(compactions);
        private_environment.sweep_values(compactions);
        realm.sweep_values(compactions);
        script_or_module.sweep_values(compactions);
        home_object.sweep_values(compactions);
        source_code.sweep_values(compactions);
    }
}
