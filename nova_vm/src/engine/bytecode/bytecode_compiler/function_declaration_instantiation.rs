// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::cell::Cell;

use crate::hashing::{AHashMap, AHashSet};
use oxc_ast::ast::BindingIdentifier;
use oxc_ecmascript::BoundNames;
use oxc_span::Ident;

use crate::{
    ecmascript::{
        BUILTIN_STRING_MEMORY, ContainsExpression, FunctionAstRef, LexicallyDeclaredNames,
        LexicallyScopedDeclaration, LexicallyScopedDeclarations, Value, VarDeclaredNames,
        VarScopedDeclaration, VarScopedDeclarations,
    },
    engine::{
        CallEnvironment, Instruction,
        bytecode::bytecode_compiler::{
            CompileContext, ValueOutput, compile_context::StackVariable, variable_escapes_scope,
        },
    },
};

use super::{
    CompileEvaluation, function_environment_use::FunctionEnvironmentUse, simple_array_pattern,
};

/// ### [10.2.11 FunctionDeclarationInstantiation ( func, argumentsList )](https://tc39.es/ecma262/#sec-functiondeclarationinstantiation)
///
/// The abstract operation FunctionDeclarationInstantiation takes arguments
/// func (an ECMAScript function object) and argumentsList (a List of
/// ECMAScript language values) and returns either a normal completion
/// containing unused or a throw completion. func is the function object for
/// which the execution context is being established.
///
/// > NOTE 1: When an execution context is established for evaluating an
/// > ECMAScript function a new Function Environment Record is created and
/// > bindings for each formal parameter are instantiated in that Environment
/// > Record. Each declaration in the function body is also instantiated. If
/// > the function's formal parameters do not include any default value
/// > initializers then the body declarations are instantiated in the same
/// > Environment Record as the parameters. If default value parameter
/// > initializers exist, a second Environment Record is created for the body
/// > declarations. Formal parameters and functions are initialized as part of
/// > FunctionDeclarationInstantiation. All other bindings are initialized
/// > during evaluation of the function body.
///
/// > NOTE 2: B.3.2 provides an extension to the above algorithm that is
/// > necessary for backwards compatibility with web browser implementations of
/// > ECMAScript that predate ECMAScript 2015.
///
/// Returns the environment a \[\[Call]] of the function needs. Nothing can
/// observe the Function Environment Record of a function with no parameter
/// expressions, no arguments object and no direct eval, that does
/// not refer to `new.target` or `super`, and in which no nested arrow function
/// refers to `this`, `new.target` or `super`: such a function's call keeps
/// the this value in the execution context, and a Declarative Environment
/// Record holds its bindings, or no record at all when every parameter, var
/// and lexical declaration lives in a stack slot.
/// The first stack slot of the parameters when every one of them is a plain
/// name kept in a stack slot, in order and with no rest parameter: they are
/// then bound from the arguments by one instruction.
fn consecutive_parameter_slots<'s>(
    ctx: &CompileContext<'_, 's, '_, '_>,
    formals: &'s oxc_ast::ast::FormalParameters<'s>,
    has_duplicates: bool,
) -> Option<usize> {
    if has_duplicates || formals.rest.is_some() {
        return None;
    }
    let mut first_slot = None;
    for (index, parameter) in formals.items.iter().enumerate() {
        let oxc_ast::ast::BindingPattern::BindingIdentifier(identifier) = &parameter.pattern else {
            return None;
        };
        let slot = ctx.get_variable_stack_index(identifier.symbol_id())? as usize;
        if slot != *first_slot.get_or_insert(slot) + index {
            return None;
        }
    }
    first_slot
}

pub(crate) fn instantiation<'s>(
    ctx: &mut CompileContext<'_, 's, '_, '_>,
    stack_variables: &mut Vec<StackVariable>,
    func: FunctionAstRef<'s>,
    strict: bool,
    is_lexical: bool,
) -> CallEnvironment {
    let environment_use = FunctionEnvironmentUse::of(func);
    // Strict or not: what a function that is not strict makes of its this
    // argument (OrdinaryCallBindThis) is made when it is called.
    let mut this_free =
        !ctx.scope_contains_direct_eval(func.scope_id()) && !environment_use.this_environment;
    // 1. Let calleeContext be the running execution context.
    // 2. Let code be func.[[ECMAScriptCode]].
    let body = func.ecmascript_code();
    // 3. Let strict be func.[[Strict]].
    // 4. Let formals be func.[[FormalParameters]].
    let formals = func.formal_parameters();

    // 8. Let hasParameterExpressions be ContainsExpression of formals.
    let has_parameter_expressions = formals.contains_expression();
    // The Environment Records that steps 20, 28 and 30 make for a function
    // with parameter expressions keep three things apart from the
    // parameters: what a direct eval declares, what a function made in the
    // parameter list can see, and the vars of the body, which such a
    // function must not see. Where there is no direct eval, no function is
    // made in the parameter list and the body declares no var and no
    // function, nothing can tell those records from the one the parameters
    // are in: none is made, and the function is called as one without
    // parameter expressions is.
    let has_direct_eval = ctx.scope_contains_direct_eval(func.scope_id());
    let separate_environments = has_parameter_expressions && {
        let mut has_vars = false;
        body.var_declared_names(&mut |_| has_vars = true);
        has_direct_eval || has_vars || FunctionEnvironmentUse::makes_functions(formals)
    };
    if separate_environments {
        this_free = false;
    }
    let mut bindings_in_environment = false;

    // 12. Let functionNames be a new empty List.
    // 13. Let functionsToInitialize be a new empty List.
    // NOTE: the keys of `functions` will be `functionNames`, its values will be
    // `functionsToInitialize`.
    let mut functions = AHashMap::default();
    body.var_scoped_declarations(&mut |d| {
        // a. If d is neither a VariableDeclaration nor a ForBinding nor a
        //    BindingIdentifier, then
        // i. Assert: d is either a FunctionDeclaration, a
        //    GeneratorDeclaration, an AsyncFunctionDeclaration, or an
        //    AsyncGeneratorDeclaration.
        let VarScopedDeclaration::Function(d) = d else {
            return;
        };
        // Skip function declarations with declare modifier - they are
        // TypeScript ambient declarations
        #[cfg(feature = "typescript")]
        if d.declare {
            return;
        }

        // ii. Let fn be the sole element of the BoundNames of d.
        let f_name = d.id.as_ref().unwrap().name;
        // iii. If functionNames does not contain fn, then
        //   1. Insert fn as the first element of functionNames.
        //   2. NOTE: If there are multiple function declarations for the
        //      same name, the last declaration is used.
        //   3. Insert d as the first element of functionsToInitialize.
        functions.insert(f_name, d);
    });

    // 15. Let argumentsObjectNeeded be true.
    let mut arguments_object_needed = true;
    // 16. If func.[[ThisMode]] is lexical, then
    if arguments_object_needed && is_lexical {
        // a. NOTE: Arrow functions never have an arguments object.
        // b. Set argumentsObjectNeeded to false.
        arguments_object_needed = false;
    } else if arguments_object_needed {
        // 17. Else if parameterNames contains "arguments", then
        formals.bound_names(&mut |name| {
            // a. Set argumentsObjectNeeded to false.
            if arguments_object_needed && name.name == Ident::new_const("arguments") {
                arguments_object_needed = false;
            }
        });
    }
    // 18. Else if hasParameterExpressions is false, then
    if arguments_object_needed && !has_parameter_expressions {
        // a. If functionNames contains "arguments" or
        if functions.contains_key(&Ident::new_const("arguments")) {
            // i. Set argumentsObjectNeeded to false.
            arguments_object_needed = false;
        } else {
            // lexicalNames contains "arguments", then
            body.lexically_declared_names(&mut |binding| {
                if binding.name == Ident::new_const("arguments") {
                    // i. Set argumentsObjectNeeded to false.
                    arguments_object_needed = false;
                }
            })
        }
    }

    // Note: after the easy checks we check a for optimisation. If the scope is not
    // poisoned and the function contains no arguments references, then it's
    // impossible for users to detect the arguments object being missing.
    if arguments_object_needed {
        arguments_object_needed =
            ctx.scope_contains_direct_eval(func.scope_id()) || environment_use.arguments;
    }

    // The arguments object is a binding named "arguments" of the function's
    // Environment Record, for whatever looks the name up there. Where
    // nothing can (no arrow function in the function refers to it, there is
    // no direct eval, and the function declares nothing of that name that a
    // reference would resolve to instead), the object is kept in a stack
    // slot, and a reference to `arguments` in the function's own code reads
    // the slot. The arguments object is never a mapped one here, so the
    // parameters are not tied to it either way.
    let arguments_in_slot = arguments_object_needed
        && !environment_use.arguments_by_name
        && !ctx.scope_contains_direct_eval(func.scope_id())
        && !ctx
            .scoping()
            .scope_has_binding(func.scope_id(), Ident::new_const("arguments"));

    // 5. Let parameterNames be the BoundNames of formals.
    // 6. If parameterNames has any duplicate entries, let hasDuplicates be
    //    true. Otherwise, let hasDuplicates be false.
    let mut parameter_names = crate::hashing::set_with_capacity(formals.parameters_count());
    let mut env_parameters = Vec::with_capacity(formals.parameters_count());
    let mut stack_parameters = Vec::with_capacity(if arguments_object_needed {
        0
    } else {
        formals.parameters_count()
    });
    let mut has_duplicates = false;
    if arguments_object_needed {
        this_free = false;
    }
    formals.bound_names(&mut |identifier| {
        let added = parameter_names.insert(identifier.name);
        if added {
            if arguments_object_needed || variable_escapes_scope(ctx, identifier) {
                env_parameters.push(identifier.name);
            } else {
                stack_parameters.push(identifier.symbol_id());
            }
        } else {
            has_duplicates = true;
        }
    });

    // 19. If strict is true or hasParameterExpressions is false, then
    //   a. NOTE: Only a single Environment Record is needed for the parameters,
    //      since calls to eval in strict mode code cannot create new bindings
    //      which are visible outside of the eval.
    //   b. Let env be the LexicalEnvironment of calleeContext.
    // 20. Else,
    //   a. NOTE: A separate Environment Record is needed to ensure that
    //      bindings created by direct eval calls in the formal parameter list
    //      are outside the environment where parameters are declared.
    //   b. Let calleeEnv be the LexicalEnvironment of calleeContext.
    //   c. Let env be NewDeclarativeEnvironment(calleeEnv).
    //   d. Assert: The VariableEnvironment of calleeContext and calleeEnv are
    //      the same Environment Record.
    //   e. Set the LexicalEnvironment of calleeContext to env.
    // NOTE: the record is there for a direct eval alone, as the one of step
    // 30 is.
    if !strict && has_parameter_expressions && has_direct_eval {
        // Note: these are not lexical scopes per-se, just something we "start
        // with". Thus, we do not use ctx.enter_lexical_scope().
        ctx.add_instruction(Instruction::EnterDeclarativeEnvironment);
    }

    // The call puts the arguments in the parameters' stack slots itself
    // (Executable::stack_parameters, Vm::execute) when that is all there is
    // to binding them: every parameter a plain name kept in a stack slot,
    // those the function's first slots, and the arguments read nowhere else
    // (no arguments object, no rest parameter, no parameter expressions).
    // Nothing is compiled for the parameters then, and no arguments iterator
    // is there to pop.
    let direct_parameters = !arguments_object_needed
        && !has_parameter_expressions
        && !has_duplicates
        && formals.rest.is_none()
        && env_parameters.is_empty()
        && ctx.stack_depth() == 0
        && stack_parameters.len() == formals.items.len()
        && formals.items.iter().all(|parameter| {
            matches!(
                parameter.pattern,
                oxc_ast::ast::BindingPattern::BindingIdentifier(_)
            )
        });
    let direct_parameters = if direct_parameters {
        u16::try_from(stack_parameters.len()).ok()
    } else {
        None
    };

    // 21. For each String paramName of parameterNames, do
    for param_name in &stack_parameters {
        stack_variables.push(if direct_parameters.is_some() {
            ctx.push_filled_stack_variable(*param_name)
        } else {
            ctx.push_stack_variable(*param_name, false)
        });
    }
    if !env_parameters.is_empty() {
        bindings_in_environment = true;
    }
    for param_name in &env_parameters {
        // a. Let alreadyDeclared be ! env.HasBinding(paramName).
        // b. NOTE: Early errors ensure that duplicate parameter names can only
        //    occur in non-strict functions that do not have parameter default
        //    values or rest parameters.
        // c. If alreadyDeclared is false, then
        // NOTE: Addition into `stack_parameters` or `env_parameters` is
        // guarded by AHashSet::insert returning `true`, so `alreadyDeclared` is
        // always false.

        // i. Perform ! env.CreateMutableBinding(paramName, false).
        let param_name = ctx.create_string(param_name);
        ctx.add_instruction_with_identifier(
            Instruction::CreateMutableBinding,
            param_name.to_property_key(),
        );
        // ii. If hasDuplicates is true, then
        if has_duplicates {
            // 1. Perform ! env.InitializeBinding(paramName, undefined).
            ctx.add_instruction_with_identifier(
                Instruction::ResolveBinding,
                param_name.to_property_key(),
            );
            ctx.add_instruction_with_constant(Instruction::StoreConstant, Value::Undefined);
            ctx.add_instruction(Instruction::InitializeReferencedBinding);
        }
    }

    // 22. If argumentsObjectNeeded is true, then
    if arguments_object_needed {
        // a. If strict is true or simpleParameterList is false, then
        //     i. Let ao be CreateUnmappedArgumentsObject(argumentsList).
        // b. Else,
        //     i. NOTE: A mapped argument object is only provided for
        //        non-strict functions that don't have a rest parameter, any
        //        parameter default value initializers, or any destructured
        //        parameters.
        //     ii. Let ao be CreateMappedArgumentsObject(func, formals,
        //         argumentsList, env).
        // TODO: Currently, we always create an unmapped arguments object, even
        // in non-strict mode.
        ctx.add_instruction(Instruction::CreateUnmappedArgumentsObject);

        if arguments_in_slot {
            stack_variables.push(ctx.push_arguments_stack_variable(func.scope_id()));
        } else if strict {
            // c. If strict is true, then
            // i. Perform ! env.CreateImmutableBinding("arguments", false).
            // ii. NOTE: In strict mode code early errors prevent attempting to
            //     assign to this binding, so its mutability is not observable.
            ctx.add_instruction_with_identifier(
                Instruction::CreateImmutableBinding,
                BUILTIN_STRING_MEMORY.arguments.to_property_key(),
            );
        } else {
            // d. Else,
            //   i. Perform ! env.CreateMutableBinding("arguments", false).
            ctx.add_instruction_with_identifier(
                Instruction::CreateMutableBinding,
                BUILTIN_STRING_MEMORY.arguments.to_property_key(),
            );
        }

        if !arguments_in_slot {
            // e. Perform ! env.InitializeBinding("arguments", ao).
            ctx.add_instruction_with_identifier(
                Instruction::ResolveBinding,
                BUILTIN_STRING_MEMORY.arguments.to_property_key(),
            );
            ctx.add_instruction(Instruction::InitializeReferencedBinding);
        }

        // f. Let parameterBindings be the list-concatenation of parameterNames
        //    and « "arguments" ».
        // NOTE: reusing `parameter_names` for `parameterBindings`.
        parameter_names.insert("arguments".into());
    }

    // 24. Let iteratorRecord be CreateListIteratorRecord(argumentsList).
    // 25. If hasDuplicates is true, then
    //   a. Perform ? IteratorBindingInitialization of formals with arguments
    //      iteratorRecord and undefined.
    // 26. Else,
    //   a. Perform ? IteratorBindingInitialization of formals with arguments
    //      iteratorRecord and env.
    if let Some(count) = direct_parameters {
        ctx.set_stack_parameters(count);
    } else if formals.has_parameter() {
        if has_parameter_expressions {
            // Note: ignore errors here for safety.
            let lexical_binding_state = ctx.lexical_binding_state;
            ctx.lexical_binding_state = !has_duplicates;
            let _ = formals.compile(ctx);
            ctx.lexical_binding_state = lexical_binding_state;
        } else if let Some(first_slot) = consecutive_parameter_slots(ctx, formals, has_duplicates) {
            ctx.add_instruction_with_immediate_and_immediate(
                Instruction::BindArguments,
                first_slot,
                formals.items.len(),
            );
        } else {
            simple_array_pattern(
                ctx,
                formals.items.iter().map(|param| Some(&param.pattern)),
                formals.rest.as_ref().map(|r| &r.rest),
                formals.items.len(),
                !has_duplicates,
            );
        }
    }
    if direct_parameters.is_none() {
        // Remove the arguments iterator from the iterator stack.
        ctx.add_instruction(Instruction::IteratorPop);
    }

    // 27. If hasParameterExpressions is false, then
    if !separate_environments {
        // a. NOTE: Only a single Environment Record is needed for the
        //    parameters and top-level vars.
        // b. Let instantiatedVarNames be a copy of the List parameterBindings.
        let mut instantiated_var_names = parameter_names.clone();
        // c. For each element n of varNames, do
        body.var_declared_names(&mut |d| {
            let n = d.name;
            // i. If instantiatedVarNames does not contain n, then
            if !instantiated_var_names.insert(n) {
                return;
            }
            // 1. Append n to instantiatedVarNames.
            let n_string = ctx.create_string(&n);
            if variable_escapes_scope(ctx, d) {
                bindings_in_environment = true;
                // 2. Perform ! env.CreateMutableBinding(n, false).
                ctx.add_instruction_with_identifier(
                    Instruction::CreateMutableBinding,
                    n_string.to_property_key(),
                );
                // 3. Perform ! env.InitializeBinding(n, undefined).
                ctx.add_instruction_with_identifier(
                    Instruction::ResolveBinding,
                    n_string.to_property_key(),
                );
                ctx.add_instruction_with_constant(Instruction::StoreConstant, Value::Undefined);
                ctx.add_instruction(Instruction::InitializeReferencedBinding);
            } else {
                stack_variables.push(ctx.push_stack_variable(d.symbol_id(), false));
            }
        });

        // d. Let varEnv be env.
        // 30. If strict is false, then
        //   a. Let lexEnv be NewDeclarativeEnvironment(varEnv).
        // 31. Else,
        //   a. Let lexEnv be varEnv.
        // 32. Set the LexicalEnvironment of calleeContext to lexEnv.
        // NOTE: the separate record is there for a direct eval, which looks
        // in it for the lexical declarations its own vars would conflict
        // with. Where no direct eval can run, nothing tells the two records
        // apart and the lexical declarations go where a strict function's
        // go.
        if !strict && ctx.scope_contains_direct_eval(func.scope_id()) {
            // Note: these are not lexical scopes per-se, just something we
            // "start with". Thus, we do not use ctx.enter_lexical_scope().
            ctx.add_instruction(Instruction::EnterDeclarativeEnvironment);
        }
    } else {
        // 28. Else,
        // a. NOTE: A separate Environment Record is needed to ensure that
        //    closures created by expressions in the formal parameter list do
        //    not have visibility of declarations in the function body.
        // b. Let varEnv be NewDeclarativeEnvironment(env).
        // c. Set the VariableEnvironment of calleeContext to varEnv.
        // NOTE: Since this step operates on a variable environment, rather than
        // the usual lexical environments, we implement this by pushing all
        // variable names and values into the stack, and then having a single
        // instruction that initializes all of them and sets the right
        // environment in one go.

        // d. Let instantiatedVarNames be a new empty List.
        let mut instantiated_var_names = AHashSet::default();
        // e. For each element n of varNames, do
        body.var_declared_names(&mut |d| {
            let n = d.name;
            // i. If instantiatedVarNames does not contain n, then
            if !instantiated_var_names.insert(n) {
                return;
            }
            // 1. Append n to instantiatedVarNames.
            // 3. If parameterBindings does not contain n, or if functionNames
            //    contains n, then
            let n_string = ctx.create_string(&n);
            if !parameter_names.contains(&n) || functions.contains_key(&n) {
                // a. Let initialValue be undefined.
                ctx.add_instruction_with_constant(Instruction::LoadConstant, Value::Undefined);
            } else {
                // 4. Else,
                // a. Let initialValue be ! env.GetBindingValue(n, false).
                ctx.add_instruction_with_identifier(
                    Instruction::ResolveBinding,
                    n_string.to_property_key(),
                );
                ctx.add_instruction(Instruction::GetValue);
                ctx.add_instruction(Instruction::Load);
            }

            // 2. Perform ! varEnv.CreateMutableBinding(n, false).
            // 5. Perform ! varEnv.InitializeBinding(n, initialValue).
            // 6. NOTE: A var with the same name as a formal parameter
            //    initially has the same value as the corresponding initialized
            //    parameter.
            ctx.add_instruction_with_constant(Instruction::LoadConstant, n_string);
        });

        // 30. If strict is false, then
        //   a. Let lexEnv be NewDeclarativeEnvironment(varEnv).
        //   b. NOTE: Non-strict functions use a separate Environment Record
        //      for top-level lexical declarations so that a direct eval can
        //      determine whether any var scoped declarations introduced by the
        //      eval code conflict with pre-existing top-level lexically scoped
        //      declarations. This is not needed for strict functions because a
        //      strict direct eval always places all declarations into a new
        //      Environment Record.
        // 31. Else,
        //   a. Let lexEnv be varEnv.
        // 32. Set the LexicalEnvironment of calleeContext to lexEnv.
        // NOTE: as in step 27, the separate record of step 30 is there for
        // a direct eval; without one the lexical declarations go where a
        // strict function's go.
        ctx.add_instruction_with_immediate_and_immediate(
            Instruction::InitializeVariableEnvironment,
            instantiated_var_names.len(),
            (strict || !has_direct_eval).into(),
        );
    }

    // 33. Let lexDeclarations be the LexicallyScopedDeclarations of code.
    // 34. For each element d of lexDeclarations, do
    {
        let is_constant_declaration = Cell::new(false);
        let lexical_in_environment = Cell::new(false);
        let cb = &mut |identifier: &BindingIdentifier<'s>| {
            if variable_escapes_scope(ctx, identifier) {
                lexical_in_environment.set(true);
                let dn = ctx.create_string(&identifier.name);
                ctx.add_instruction_with_identifier(
                    // i. If IsConstantDeclaration of d is true, then
                    if is_constant_declaration.get() {
                        // 1. Perform ! lexEnv.CreateImmutableBinding(dn, true).
                        Instruction::CreateImmutableBinding
                    } else {
                        // ii. Else,
                        // 1. Perform ! lexEnv.CreateMutableBinding(dn, false).
                        Instruction::CreateMutableBinding
                    },
                    dn.to_property_key(),
                );
            } else {
                stack_variables.push(ctx.push_stack_variable(identifier.symbol_id(), false));
            }
        };
        let mut create_default_export = false;
        body.lexically_scoped_declarations(&mut |d| {
            // a. NOTE: A lexically declared name cannot be the same as a
            //    function/generator declaration, formal parameter, or a var name.
            //    Lexically declared names are only instantiated here but not
            //    initialized.
            // b. For each element dn of the BoundNames of d, do
            match d {
                LexicallyScopedDeclaration::Variable(decl) => {
                    is_constant_declaration.set(decl.kind.is_const());
                    decl.id.bound_names(cb);
                    is_constant_declaration.set(false);
                }
                LexicallyScopedDeclaration::Function(decl) => {
                    // Skip function declarations with declare modifier - they are TypeScript ambient declarations
                    #[cfg(feature = "typescript")]
                    if decl.declare {
                        return;
                    }

                    decl.bound_names(cb);
                }
                LexicallyScopedDeclaration::Class(decl) => {
                    decl.bound_names(cb);
                }
                LexicallyScopedDeclaration::DefaultExport => {
                    create_default_export = true;
                }
                #[cfg(feature = "typescript")]
                LexicallyScopedDeclaration::TSEnum(decl) => {
                    decl.id.bound_names(cb);
                }
            }
        });
        if lexical_in_environment.get() || create_default_export {
            bindings_in_environment = true;
        }
        if create_default_export {
            let dn = BUILTIN_STRING_MEMORY._default_;
            ctx.add_instruction_with_identifier(
                Instruction::CreateMutableBinding,
                dn.to_property_key(),
            );
        }
    }

    // 36. For each Parse Node f of functionsToInitialize, do
    for f in functions.values() {
        // b. Let fo be InstantiateFunctionObject of f with arguments lexEnv
        //    and privateEnv.
        f.compile(ctx);
        // a. Let fn be the sole element of the BoundNames of f.
        // c. Perform ! varEnv.SetMutableBinding(fn, fo, false).
        // TODO: This compilation is incorrect if !strict, when varEnv != lexEnv.
        let f = f.id.as_ref().unwrap().compile(ctx);
        f.put_value(ctx, ValueOutput::Value).unwrap();
    }
    if !this_free {
        CallEnvironment::Function
    } else if bindings_in_environment {
        CallEnvironment::Declarative
    } else {
        CallEnvironment::None
    }
}
