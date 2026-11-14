// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use wtf8::Wtf8Buf;

use crate::{
    ecmascript::{
        Agent, ArgumentsList, BUILTIN_STRING_MEMORY, Behaviour, Builtin,
        BuiltinIntrinsicConstructor, CompileFunctionBodyData, ECMAScriptFunction, Environment,
        ExceptionType, Function, FunctionAstRef, FunctionTemplate, JsResult, Object,
        OrdinaryFunctionCreateParams, ParseResult, Primitive, ProtoIntrinsics, Realm, SourceCode,
        SourceCodeType, String, Value, builders::BuiltinFunctionBuilder,
        get_prototype_from_constructor, make_constructor, ordinary_function_create,
        set_function_name, to_string, to_string_primitive,
    },
    engine::{Bindable, Executable, GcScope, Scopable},
    heap::IntrinsicConstructorIndexes,
};

pub(crate) struct FunctionConstructor;

impl Builtin for FunctionConstructor {
    const NAME: String<'static> = BUILTIN_STRING_MEMORY.Function;

    const LENGTH: u8 = 1;

    const BEHAVIOUR: Behaviour = Behaviour::Constructor(Self::constructor);
}
impl BuiltinIntrinsicConstructor for FunctionConstructor {
    const INDEX: IntrinsicConstructorIndexes = IntrinsicConstructorIndexes::Function;
}

impl FunctionConstructor {
    fn constructor<'gc>(
        agent: &mut Agent,
        _this_value: Value,
        arguments: ArgumentsList,
        new_target: Option<Object>,
        mut gc: GcScope<'gc, '_>,
    ) -> JsResult<'gc, Value<'gc>> {
        // 2. If bodyArg is not present, set bodyArg to the empty String.
        let (parameter_args, body_arg) = if arguments.is_empty() {
            (&[] as &[Value], String::EMPTY_STRING.into())
        } else {
            let (last, others) = arguments.split_last().unwrap();
            (others, *last)
        };
        let constructor = if let Some(new_target) = new_target {
            Function::try_from(new_target).unwrap()
        } else {
            agent.running_execution_context().function.unwrap()
        };

        // 3. Return ? CreateDynamicFunction(C, NewTarget, normal, parameterArgs, bodyArg).
        let f = create_dynamic_function(
            agent,
            constructor,
            DynamicFunctionKind::Normal,
            parameter_args,
            body_arg,
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        // 20.2.1.1.1 CreateDynamicFunction ( constructor, newTarget, kind, parameterArgs, bodyArg )
        // 32. Else if kind is normal, then
        //   a. Perform MakeConstructor(F).
        make_constructor(agent, f.unbind(), None, None, gc.nogc());

        Ok(f.unbind().into())
    }

    pub(crate) fn create_intrinsic(agent: &mut Agent, realm: Realm<'static>) {
        let intrinsics = agent.get_realm_record_by_id(realm).intrinsics();
        let function_prototype = intrinsics.function_prototype().into();

        BuiltinFunctionBuilder::new_intrinsic_constructor::<FunctionConstructor>(agent, realm)
            .with_property_capacity(1)
            .with_prototype(function_prototype)
            .with_prototype_property(function_prototype)
            .build();
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DynamicFunctionKind {
    Normal,
    Generator,
    Async,
    AsyncGenerator,
}
impl DynamicFunctionKind {
    fn prefix(&self) -> &'static str {
        match self {
            DynamicFunctionKind::Normal => "function",
            DynamicFunctionKind::Generator => "function*",
            DynamicFunctionKind::Async => "async function",
            DynamicFunctionKind::AsyncGenerator => "async function*",
        }
    }
    fn function_matches_kind(&self, function: &oxc_ast::ast::Function) -> bool {
        let (is_async, is_generator) = match self {
            DynamicFunctionKind::Normal => (false, false),
            DynamicFunctionKind::Generator => (false, true),
            DynamicFunctionKind::Async => (true, false),
            DynamicFunctionKind::AsyncGenerator => (true, true),
        };
        function.r#async == is_async && function.generator == is_generator
    }
    fn intrinsic_prototype(&self) -> ProtoIntrinsics {
        match self {
            DynamicFunctionKind::Normal => ProtoIntrinsics::Function,
            DynamicFunctionKind::Generator => ProtoIntrinsics::GeneratorFunction,
            DynamicFunctionKind::Async => ProtoIntrinsics::AsyncFunction,
            DynamicFunctionKind::AsyncGenerator => ProtoIntrinsics::AsyncGeneratorFunction,
        }
    }
}

/// ### [20.2.1.1.1 CreateDynamicFunction ( constructor, newTarget, kind, parameterArgs, bodyArg )](https://tc39.es/ecma262/#sec-createdynamicfunction)
///
/// NOTE: This implementation doesn't cover steps 30-32, those should be handled by the caller.
pub(crate) fn create_dynamic_function<'a>(
    agent: &mut Agent,
    constructor: Function,
    kind: DynamicFunctionKind,
    parameter_args: &[Value],
    body_arg: Value,
    mut gc: GcScope<'a, '_>,
) -> JsResult<'a, ECMAScriptFunction<'a>> {
    let mut constructor = constructor.bind(gc.nogc());
    // 11. Perform ? HostEnsureCanCompileStrings(currentRealm, parameterStrings, bodyString, false).
    agent
        .host_hooks
        .ensure_can_compile_strings(agent.current_realm(gc.nogc()), gc.nogc())
        .unbind()?;

    let source_string = {
        let parameter_strings_vec;
        let parameter_strings_slice;
        let body_string;
        if body_arg.is_string() && parameter_args.iter().all(|arg| arg.is_string()) {
            body_string = String::try_from(body_arg).unwrap().bind(gc.nogc());
            parameter_strings_vec = parameter_args
                .iter()
                .map(|arg| {
                    String::try_from(*arg)
                        .expect("every parameter was checked to be a string")
                        .bind(gc.nogc())
                })
                .collect::<Vec<_>>();
            parameter_strings_slice = &parameter_strings_vec;
        } else if body_arg.is_primitive() && parameter_args.iter().all(|arg| arg.is_primitive()) {
            // We don't need to call JavaScript here. Nice.
            let gc = gc.nogc();
            let mut parameter_strings = Vec::with_capacity(parameter_args.len());
            for param in parameter_args {
                parameter_strings.push(
                    to_string_primitive(agent, Primitive::try_from(*param).unwrap(), gc)
                        .unbind()?
                        .bind(gc),
                );
            }
            parameter_strings_vec = parameter_strings;
            parameter_strings_slice = &parameter_strings_vec;
            body_string = to_string_primitive(agent, Primitive::try_from(body_arg).unwrap(), gc)
                .unbind()?
                .bind(gc);
        } else {
            // Some of the parameters are non-primitives. This means we'll be
            // calling into JavaScript during this work.
            let scoped_constructor = constructor.scope(agent, gc.nogc());
            // Every argument is rooted before the first conversion: any of
            // them may call into JavaScript and trigger garbage collection.
            let scoped_parameter_args = parameter_args
                .iter()
                .map(|param| param.scope(agent, gc.nogc()))
                .collect::<Vec<_>>();
            let scoped_body_arg = body_arg.scope(agent, gc.nogc());
            let mut parameter_string_roots = Vec::with_capacity(scoped_parameter_args.len());
            for param in scoped_parameter_args {
                let param_string = to_string(agent, param.get(agent), gc.reborrow())
                    .unbind()?
                    .bind(gc.nogc());
                parameter_string_roots.push(param.replace_self(agent, param_string.unbind()));
            }
            let body_string_unbound = scoped_body_arg
                .get(agent)
                .to_string(agent, gc.reborrow())
                .unbind()?;
            // We've done all our potential JavaScript calling: Now we rest.
            let gc = gc.nogc();
            body_string = body_string_unbound.bind(gc);
            let parameter_strings = parameter_string_roots
                .into_iter()
                .map(|param_root| param_root.get(agent).bind(gc))
                .collect::<Vec<_>>();

            constructor = scoped_constructor.get(agent).bind(gc);
            parameter_strings_vec = parameter_strings;
            parameter_strings_slice = &parameter_strings_vec;
        }

        // format!("{} anonymous({}\n) {{\n{}\n}}", kind.prefix(), parameters, body_arg)
        let str_len = kind.prefix().len()
            + 18
            + body_string.len_(agent)
            + if !parameter_strings_slice.is_empty() {
                // Separated by a single comma character
                parameter_strings_slice
                    .iter()
                    .map(|str| str.len_(agent) + 1)
                    .sum::<usize>()
                    - 1
            } else {
                0
            };
        let mut string = Wtf8Buf::with_capacity(str_len);
        string.push_str(kind.prefix());
        string.push_str(" anonymous(");
        for (i, parameter) in parameter_strings_slice.iter().enumerate() {
            if i != 0 {
                string.push_char(',');
            }
            string.push_wtf8(parameter.as_wtf8_(agent));
        }
        string.push_str("\n) {\n");
        string.push_wtf8(body_string.as_wtf8_(agent));
        string.push_str("\n}");

        debug_assert_eq!(string.len(), str_len);

        String::from_wtf8_buf(agent, string, gc.nogc())
    };

    // The spec says to parse the parameters and the function body separately to
    // avoid code injection, but oxc doesn't have a public API to do that.
    // Instead, we parse the source string as a script, and throw unless it has
    // exactly one statement which is a function declaration of the right kind.

    let nogc = gc.nogc();
    let parse_result = SourceCode::parse_source(
        agent,
        source_string,
        SourceCodeType::Script { strict: false },
        #[cfg(feature = "typescript")]
        false,
        nogc,
        |agent, parsed| {
            let ParseResult {
                source_code,
                body,
                directives,
                scoping,
                nodes,
                is_strict,
            } = parsed;
            let function = if let Some(oxc_ast::ast::Statement::FunctionDeclaration(func)) =
                body.first()
                // Note: we didn't add a directives so there should be none...
                && directives.is_empty()
                // ... and as a result, is_strict should be false!
                && !is_strict
                && body.len() == 1
                && kind.function_matches_kind(func)
            {
                func.as_ref()
            } else {
                return Err(source_code);
            };
            // The function's source code is only that of the function
            // definition, so its [[Strict]] comes from its own directive
            // alone.
            let ast = FunctionAstRef::from(function);
            let template = FunctionTemplate::from_ast(ast, None);
            let executable = Executable::compile_function_body(
                agent,
                CompileFunctionBodyData {
                    ast,
                    source_code,
                    scoping,
                    nodes,
                    is_strict: template.has_use_strict_directive,
                    is_lexical: false,
                },
                nogc,
            );
            Ok((
                source_code,
                FunctionTemplate {
                    executable: Some(executable),
                    ..template
                },
            ))
        },
    );

    let (source_code, template) = match parse_result {
        Ok(Ok(r)) => r,
        Ok(Err(source_code)) => {
            source_code.manually_drop(agent);
            return Err(agent.throw_exception_with_static_message(
                ExceptionType::SyntaxError,
                "Invalid function source text.",
                gc.into_nogc(),
            ));
        }
        Err(err) => {
            let gc = gc.into_nogc();
            if agent.termination().is_some() {
                return Err(agent.termination_error(gc));
            }
            let message = String::from_string(agent, err.first().unwrap().message.to_string(), gc);
            return Err(agent.throw_exception_with_message(
                ExceptionType::SyntaxError,
                message,
                gc,
            ));
        }
    };
    let executable = template
        .executable
        .expect("compiled above")
        .scope(agent, gc.nogc());
    let template = FunctionTemplate {
        executable: None,
        ..template.unbind()
    };
    let source_code = source_code.unbind();
    let source_code = source_code.bind(gc.nogc()).scope(agent, gc.nogc());
    let params = OrdinaryFunctionCreateParams {
        function_prototype: get_prototype_from_constructor(
            agent,
            constructor.unbind(),
            kind.intrinsic_prototype(),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc()),
        source_code: Some(source_code.take(agent)),
        template: FunctionTemplate {
            executable: Some(executable.take(agent)),
            ..template
        },
        lexical_this: false,
        env: Environment::Global(
            agent
                .current_realm_record()
                .global_env
                .unwrap()
                .unbind()
                .bind(gc.nogc()),
        ),
        private_env: None,
    };
    let f = ordinary_function_create(agent, params, gc.nogc()).unbind();
    let gc = gc.into_nogc();
    let f = f.bind(gc);

    set_function_name(
        agent,
        f,
        BUILTIN_STRING_MEMORY.anonymous.to_property_key(),
        None,
        gc,
    );
    // NOTE: Skipping steps 30-32.

    Ok(f)
}
