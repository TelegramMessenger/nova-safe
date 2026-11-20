// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::{Agent, Environment, JsResult, PrivateEnvironment, Realm, get_this_environment};
use crate::{
    ecmascript::{Function, Object, ScriptOrModule, SourceCode, Value},
    engine::{Bindable, NoGcScope},
    heap::{CompactionLists, HeapMarkAndSweep, WorkQueues},
};

// TODO: Remove this.
pub(crate) type ECMAScriptCode = ECMAScriptCodeEvaluationState;

/// The this value of a call that has no Function Environment Record.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallThisValue {
    /// None for the global this value of the realm, which a function that
    /// is not strict has in place of undefined and null
    /// (OrdinaryCallBindThis, step 6.a), until something in the call asks
    /// for `this`: most functions called with no this value never do, and
    /// finding it through the realm and its Global Environment Record was
    /// a twentieth of a plain call. [`resolve_this_binding`] finds it then
    /// and keeps it here.
    pub(crate) value: Option<Value<'static>>,
    /// The VariableEnvironment of the call. A class static block evaluated in
    /// the call's execution context replaces it with an environment that has
    /// its own this binding; the this value only holds while it is current.
    pub(crate) variable_environment: Environment<'static>,
}

impl HeapMarkAndSweep for CallThisValue {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            value,
            variable_environment,
        } = self;
        value.mark_values(queues);
        variable_environment.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            value,
            variable_environment,
        } = self;
        value.sweep_values(compactions);
        variable_environment.sweep_values(compactions);
    }
}

/// # [code evaluation state](https://tc39.es/ecma262/#table-state-components-for-all-execution-contexts)
///
/// ECMAScript code execution contexts have the additional state components
/// listed in Table 26.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ECMAScriptCodeEvaluationState {
    /// ### LexicalEnvironment
    ///
    /// Identifies the Environment Record used to resolve identifier references
    /// made by code within this execution context.
    pub(crate) lexical_environment: Environment<'static>,

    /// ### VariableEnvironment
    ///
    /// Identifies the Environment Record that holds bindings created by
    /// VariableStatements within this execution context.
    pub(crate) variable_environment: Environment<'static>,

    /// ### PrivateEnvironment
    ///
    /// Identifies the PrivateEnvironment Record that holds Private Names
    /// created by ClassElements in the nearest containing class. null if there
    /// is no containing class.
    pub(crate) private_environment: Option<PrivateEnvironment<'static>>,

    /// Although the spec does not track this here, we also use
    /// [`ECMAScriptCodeEvaluationState`] to track whether some ECMAScript code
    /// is in strict mode.
    pub(crate) is_strict_mode: bool,

    /// Nova specific: the this value of a call to a strict function that runs
    /// without a Function Environment Record of its own. None in every other
    /// execution context.
    pub(crate) this_value: Option<CallThisValue>,

    /// Nova-specific piece of data that identifiers in which source code the
    /// currently evaluated code was defined in. Note that this is also
    /// defined for builtin functions: A builtin function's source code will
    /// point to the source code that called it.
    pub(crate) source_code: SourceCode<'static>,
}

/// ## [9.4 Execution Contexts](https://tc39.es/ecma262/#sec-execution-contexts)
///
/// An execution context is a specification device that is used to track the
/// runtime evaluation of code by an ECMAScript implementation. At any point in
/// time, there is at most one execution context per agent that is actually
/// executing code. This is known as the agent's running execution context. All
/// references to the running execution context in this specification denote
/// the running execution context of the surrounding agent.
#[derive(Debug, Clone)]
pub(crate) struct ExecutionContext {
    /// ### code evaluation state
    ///
    /// Any state needed to perform, suspend, and resume evaluation of the code
    /// associated with this execution context.
    pub(crate) ecmascript_code: Option<ECMAScriptCodeEvaluationState>,

    /// ### Function
    ///
    /// If this execution context is evaluating the code of a function object,
    /// then the value of this component is that function object. If the
    /// context is evaluating the code of a Script or Module, the value is
    /// null.
    pub(crate) function: Option<Function<'static>>,

    /// ### Realm
    ///
    /// The Realm Record from which associated code accesses ECMAScript
    /// resources.
    pub(crate) realm: Realm<'static>,

    /// ### ScriptOrModule
    ///
    /// The Module Record or Script Record from which associated code
    /// originates. If there is no originating script or module, as is the case
    /// for the original execution context created in
    /// InitializeHostDefinedRealm, the value is null.
    pub(crate) script_or_module: Option<ScriptOrModule<'static>>,
}

impl ExecutionContext {
    pub(crate) fn suspend(&self) {
        // TODO: What does this actually mean in the end?
    }
}

impl HeapMarkAndSweep for ECMAScriptCodeEvaluationState {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            lexical_environment,
            variable_environment,
            private_environment,
            is_strict_mode: _,
            this_value,
            source_code,
        } = self;
        this_value.mark_values(queues);
        lexical_environment.mark_values(queues);
        variable_environment.mark_values(queues);
        private_environment.mark_values(queues);
        source_code.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            lexical_environment,
            variable_environment,
            private_environment,
            is_strict_mode: _,
            this_value,
            source_code,
        } = self;
        this_value.sweep_values(compactions);
        lexical_environment.sweep_values(compactions);
        variable_environment.sweep_values(compactions);
        private_environment.sweep_values(compactions);
        source_code.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for ExecutionContext {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            ecmascript_code,
            function,
            realm,
            script_or_module,
        } = self;
        ecmascript_code.mark_values(queues);
        function.mark_values(queues);
        realm.mark_values(queues);
        script_or_module.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            ecmascript_code,
            function,
            realm,
            script_or_module,
        } = self;
        ecmascript_code.sweep_values(compactions);
        function.sweep_values(compactions);
        realm.sweep_values(compactions);
        script_or_module.sweep_values(compactions);
    }
}

/// ### [9.4.4 ResolveThisBinding ( )](https://tc39.es/ecma262/#sec-resolvethisbinding)
pub(crate) fn resolve_this_binding<'a>(
    agent: &mut Agent,
    gc: NoGcScope<'a, '_>,
) -> JsResult<'a, Value<'a>> {
    if let Some(code) = agent.running_execution_context().ecmascript_code.as_ref()
        && let Some(this_value) = code.this_value
        && this_value.variable_environment == code.variable_environment
    {
        if let Some(value) = this_value.value {
            return Ok(value.bind(gc));
        }
        // OrdinaryCallBindThis 6.a, left until now:
        // i. Let globalEnv be calleeRealm.[[GlobalEnv]].
        // ii. Assert: globalEnv is a Global Environment Record.
        // iii. Let thisValue be globalEnv.[[GlobalThisValue]].
        // The realm of the running execution context is calleeRealm.
        let value: Value = agent
            .current_global_env(gc)
            .get_this_binding(agent)
            .bind(gc)
            .into();
        agent.set_running_call_this_value(value.unbind());
        return Ok(value);
    }
    // 1. Let envRec be GetThisEnvironment().
    let env_rec = get_this_environment(agent, gc);
    // 2. Return ? envRec.GetThisBinding().
    match env_rec {
        Environment::Function(e) => e.unbind().get_this_binding(agent, gc),
        Environment::Global(e) => Ok(e.unbind().get_this_binding(agent).into()),
        Environment::Module(_) => Ok(Value::Undefined),
        Environment::Declarative(_) | Environment::Object(_) => unreachable!(),
    }
}

/// ### [9.4.6 GetGlobalObject ( )](https://tc39.es/ecma262/#sec-getglobalobject)
///
/// The abstract operation GetGlobalObject takes no arguments and returns an
/// Object. It returns the global object used by the currently running
/// execution context.
pub(crate) fn get_global_object<'a>(agent: &Agent, gc: NoGcScope<'a, '_>) -> Object<'a> {
    // 1. Let currentRealm be the current Realm Record.
    let current_realm = agent.current_realm_record();
    // 2. Return currentRealm.[[GlobalObject]].
    current_realm.global_object.bind(gc)
}
