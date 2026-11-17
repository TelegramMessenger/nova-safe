// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ## [9.1 Environment Records](https://tc39.es/ecma262/#sec-environment-records)
//!
//! Environment Record is a specification type used to define the association of
//! Identifiers to specific variables and functions, based upon the lexical
//! nesting structure of ECMAScript code. Usually an Environment Record is
//! associated with some specific syntactic structure of ECMAScript code such as
//! a FunctionDeclaration, a BlockStatement, or a Catch clause of a
//! TryStatement. Each time such code is evaluated, a new Environment Record is
//! created to record the identifier bindings that are created by that code.
//!
//! Every Environment Record has an \[\[OuterEnv\]\] field, which is either null or
//! a reference to an outer Environment Record. This is used to model the
//! logical nesting of Environment Record values. The outer reference of an
//! (inner) Environment Record is a reference to the Environment Record that
//! logically surrounds the inner Environment Record. An outer Environment
//! Record may, of course, have its own outer Environment Record. An Environment
//! Record may serve as the outer environment for multiple inner Environment
//! Records. For example, if a FunctionDeclaration contains two nested
//! FunctionDeclarations then the Environment Records of each of the nested
//! functions will have as their outer Environment Record the Environment Record
//! of the current evaluation of the surrounding function.

mod declarative_environment;
mod function_environment;
mod global_environment;
mod module_environment;
mod object_environment;
mod private_environment;

pub(crate) use declarative_environment::*;
pub(crate) use function_environment::*;
pub(crate) use global_environment::*;
pub(crate) use module_environment::*;
pub(crate) use object_environment::*;
pub(crate) use private_environment::*;

use std::ops::ControlFlow;

use crate::{
    ecmascript::{
        Agent, InternalMethods, JsResult, Object, OrdinaryObject, PropertyLookupCache,
        PropertyOffset, Proxy, Reference, SetResult, String, TryError, TryHasResult, TryResult,
        Value, js_result_into_try, ordinary_set_own_data_at_offset,
    },
    engine::{Bindable, GcScope, HeapRootData, NoGcScope, Scopable},
    heap::{CompactionLists, HeapIndexHandle, HeapMarkAndSweep, WorkQueues},
};

/// ### [\[\[OuterEnv\]\]](https://tc39.es/ecma262/#sec-environment-records)
///
/// Every Environment Record has an \[\[OuterEnv\]\] field, which is either
/// null or a reference to an outer Environment Record. This is used to model
/// the logical nesting of Environment Record values. The outer reference of an
/// (inner) Environment Record is a reference to the Environment Record that
/// logically surrounds the inner Environment Record. An outer Environment
/// Record may, of course, have its own outer Environment Record. An
/// Environment Record may serve as the outer environment for multiple inner
/// Environment Records. For example, if a FunctionDeclaration contains two
/// nested FunctionDeclarations then the Environment Records of each of the
/// nested functions will have as their outer Environment Record the
/// Environment Record of the current evaluation of the surrounding function.
pub(crate) type OuterEnv<'a> = Option<Environment<'a>>;

macro_rules! create_environment_index {
    ($record: ident, $index: ident, $entry: ident) => {
        /// An index used to access an environment from [`Environments`].
        /// Internally, we store the index in a [`NonZeroU32`] with the index
        /// plus one. This allows us to not use an empty value in storage for
        /// the zero index while still saving room for a [`None`] value when
        /// stored in an [`Option`].
        ///
        /// [`NonZeroU32`]: core::num::NonZeroU32
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        #[repr(transparent)]
        pub(crate) struct $index<'a>(crate::heap::BaseIndex<'a, $record>);
        crate::heap::index_handle!($index);

        impl core::fmt::Debug for $index<'_> {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(
                    f,
                    "$index({:?})",
                    crate::heap::HeapIndexHandle::get_index_u32(*self)
                )
            }
        }

        impl<'a> crate::heap::DirectArenaAccess for $index<'a> {
            type Data = $record;
            type Output = $record;

            #[inline]
            fn get_direct(self, source: &Vec<Self::Data>) -> &Self::Output {
                source
                    .get(crate::heap::HeapIndexHandle::get_index(self))
                    .expect("Invalid environment handle")
            }
        }

        impl<'a> crate::heap::DirectArenaAccessMut for $index<'a> {
            #[inline]
            fn get_direct_mut(self, source: &mut Vec<Self::Data>) -> &mut Self::Data {
                source
                    .get_mut(crate::heap::HeapIndexHandle::get_index(self))
                    .expect("Invalid environment handle")
            }
        }

        #[doc(hidden)]
        impl AsRef<Vec<$record>> for crate::ecmascript::Agent {
            #[inline(always)]
            fn as_ref(&self) -> &Vec<$record> {
                &self.heap.environments.$entry
            }
        }

        #[doc(hidden)]
        impl AsMut<Vec<$record>> for crate::ecmascript::Agent {
            #[inline(always)]
            fn as_mut(&mut self) -> &mut Vec<$record> {
                &mut self.heap.environments.$entry
            }
        }
    };
}

create_environment_index!(
    DeclarativeEnvironmentRecord,
    DeclarativeEnvironment,
    declarative
);
create_environment_index!(FunctionEnvironmentRecord, FunctionEnvironment, function);
create_environment_index!(GlobalEnvironmentRecord, GlobalEnvironment, global);
create_environment_index!(ObjectEnvironmentRecord, ObjectEnvironment, object);
create_environment_index!(ModuleEnvironmentRecord, ModuleEnvironment, module);
create_environment_index!(PrivateEnvironmentRecord, PrivateEnvironment, private);

impl<'a> From<DeclarativeEnvironment<'a>> for Environment<'a> {
    fn from(value: DeclarativeEnvironment<'a>) -> Self {
        Environment::Declarative(value)
    }
}

impl<'a> From<GlobalEnvironment<'a>> for Environment<'a> {
    fn from(value: GlobalEnvironment<'a>) -> Self {
        Environment::Global(value)
    }
}

impl<'a> From<ModuleEnvironment<'a>> for Environment<'a> {
    fn from(value: ModuleEnvironment<'a>) -> Self {
        Environment::Module(value)
    }
}

impl<'a> From<ObjectEnvironment<'a>> for Environment<'a> {
    fn from(value: ObjectEnvironment<'a>) -> Self {
        Environment::Object(value)
    }
}

/// ### [9.1.1 The Environment Record Type Hierarchy](https://tc39.es/ecma262/#sec-the-environment-record-type-hierarchy)
///
/// Environment Records can be thought of as existing in a simple
/// object-oriented hierarchy where Environment Record is an abstract class
/// with three concrete subclasses: Declarative Environment Record, Object
/// Environment Record, and Global Environment Record. Function Environment
/// Records and Module Environment Records are subclasses of Declarative
/// Environment Record.
#[derive(Clone, Copy, PartialEq, Eq, Bindable)]
#[repr(u8)]
pub(crate) enum Environment<'a> {
    // Leave 0 for None option
    Declarative(DeclarativeEnvironment<'a>) = 1,
    Function(FunctionEnvironment<'a>),
    Global(GlobalEnvironment<'a>),
    Module(ModuleEnvironment<'a>),
    Object(ObjectEnvironment<'a>),
}
impl<'e> Environment<'e> {
    pub(crate) fn get_outer_env(self, agent: &Agent) -> OuterEnv<'e> {
        match self {
            Environment::Declarative(e) => e.get_outer_env(agent),
            Environment::Function(e) => e.get_outer_env(agent),
            Environment::Global(_) => None,
            Environment::Module(e) => e.get_outer_env(agent),
            Environment::Object(e) => e.get_outer_env(agent),
        }
    }

    /// ### Try [HasBinding(N)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Determine if an Environment Record has a binding for the String value
    /// N. Return true if it does and false if it does not.
    pub(crate) fn try_has_binding<'gc>(
        self,
        agent: &mut Agent,
        name: String,
        cache: Option<PropertyLookupCache>,
        gc: NoGcScope<'gc, '_>,
    ) -> ControlFlow<TryError<'gc>, TryHasBindingContinue<'gc>> {
        match self {
            Environment::Declarative(e) => {
                TryHasBindingContinue::Result(e.has_binding(agent, name)).into()
            }
            Environment::Function(e) => {
                TryHasBindingContinue::Result(e.has_binding(agent, name)).into()
            }
            Environment::Global(e) => e.try_has_binding(agent, name, cache, gc),
            Environment::Module(e) => {
                TryHasBindingContinue::Result(e.has_binding(agent, name)).into()
            }
            Environment::Object(e) => e.try_has_binding(agent, name, cache, gc),
        }
    }

    /// # [HasBinding(N)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Determine if an Environment Record has a binding for the String value
    /// N. Return true if it does and false if it does not.
    pub(crate) fn has_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, bool> {
        match self {
            Environment::Declarative(e) => Ok(e.has_binding(agent, name)),
            Environment::Function(e) => Ok(e.has_binding(agent, name)),
            Environment::Global(e) => e.has_binding(agent, name, gc),
            Environment::Module(e) => Ok(e.has_binding(agent, name)),
            Environment::Object(e) => e.has_binding(agent, name, gc),
        }
    }

    /// ### Try [CreateMutableBinding(N, D)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Create a new but uninitialized mutable binding in an Environment
    /// Record. The String value N is the text of the bound name. If the
    /// Boolean argument D is true the binding may be subsequently deleted.
    pub(crate) fn try_create_mutable_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        is_deletable: bool,
        cache: Option<PropertyLookupCache>,
        gc: NoGcScope<'a, '_>,
    ) -> TryResult<'a, ()> {
        match self {
            Environment::Declarative(e) => {
                e.create_mutable_binding(agent, name, is_deletable);
                TryResult::Continue(())
            }
            Environment::Function(e) => {
                e.create_mutable_binding(agent, name, is_deletable);
                TryResult::Continue(())
            }
            Environment::Global(e) => {
                js_result_into_try(e.create_mutable_binding(agent, name, is_deletable, gc))
            }
            Environment::Module(e) => {
                e.create_mutable_binding(agent, name, is_deletable);
                TryResult::Continue(())
            }
            Environment::Object(e) => {
                e.try_create_mutable_binding(agent, name, is_deletable, cache, gc)
            }
        }
    }

    /// # [CreateMutableBinding(N, D)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Create a new but uninitialized mutable binding in an Environment
    /// Record. The String value N is the text of the bound name. If the
    /// Boolean argument D is true the binding may be subsequently deleted.
    pub(crate) fn create_mutable_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        is_deletable: bool,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        match self {
            Environment::Declarative(e) => {
                e.create_mutable_binding(agent, name, is_deletable);
                Ok(())
            }
            Environment::Function(e) => {
                e.create_mutable_binding(agent, name, is_deletable);
                Ok(())
            }
            Environment::Global(e) => {
                e.create_mutable_binding(agent, name, is_deletable, gc.into_nogc())
            }
            Environment::Module(e) => {
                e.create_mutable_binding(agent, name, is_deletable);
                Ok(())
            }
            Environment::Object(e) => e.create_mutable_binding(agent, name, is_deletable, gc),
        }
    }

    /// # [CreateImmutableBinding(N, S)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Create a new but uninitialized immutable binding in an Environment
    /// Record. The String value N is the text of the bound name. If S is true
    /// then attempts to set it after it has been initialized will always throw
    /// an exception, regardless of the strict mode setting of operations that
    /// reference that binding.
    pub(crate) fn create_immutable_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        is_strict: bool,
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        match self {
            Environment::Declarative(e) => {
                e.create_immutable_binding(agent, name, is_strict);
                Ok(())
            }
            Environment::Function(e) => {
                e.create_immutable_binding(agent, name, is_strict);
                Ok(())
            }
            Environment::Global(e) => e.create_immutable_binding(agent, name, is_strict, gc),
            Environment::Module(e) => {
                debug_assert!(is_strict);
                e.create_immutable_binding(agent, name);
                Ok(())
            }
            Environment::Object(e) => {
                e.create_immutable_binding(agent, name, is_strict);
                Ok(())
            }
        }
    }

    /// ### Try [InitializeBinding(N, V)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Set the value of an already existing but uninitialized binding in an
    /// Environment Record. The String value N is the text of the bound name.
    /// V is the value for the binding and is a value of any ECMAScript
    /// language type.
    pub(crate) fn try_initialize_binding<'gc>(
        self,
        agent: &mut Agent,
        name: String,
        cache: Option<PropertyLookupCache>,
        value: Value,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, SetResult<'gc>> {
        match self {
            Environment::Declarative(e) => {
                e.initialize_binding(agent, name, value);
                SetResult::Done.into()
            }
            Environment::Function(e) => {
                e.initialize_binding(agent, name, value);
                SetResult::Done.into()
            }
            Environment::Global(e) => e.try_initialize_binding(agent, name, cache, value, gc),
            Environment::Module(e) => {
                e.initialize_binding(agent, name, value);
                SetResult::Done.into()
            }
            Environment::Object(e) => e.try_initialize_binding(agent, name, cache, value, gc),
        }
    }

    /// # [InitializeBinding(N, V)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Set the value of an already existing but uninitialized binding in an
    /// Environment Record. The String value N is the text of the bound name.
    /// V is the value for the binding and is a value of any ECMAScript
    /// language type.
    pub(crate) fn initialize_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        cache: Option<PropertyLookupCache>,
        value: Value,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        match self {
            Environment::Declarative(e) => {
                e.initialize_binding(agent, name, value);
                Ok(())
            }
            Environment::Function(e) => {
                e.initialize_binding(agent, name, value);
                Ok(())
            }
            Environment::Global(e) => e.initialize_binding(agent, name, cache, value, gc),
            Environment::Module(e) => {
                e.initialize_binding(agent, name, value);
                Ok(())
            }
            Environment::Object(e) => e.initialize_binding(agent, name, cache, value, gc),
        }
    }

    /// ### Try [SetMutableBinding(N, V, S)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Set the value of an already existing mutable binding in an Environment
    /// Record. The String value N is the text of the bound name. V is the
    /// value for the binding and may be a value of any ECMAScript language
    /// type. S is a Boolean flag. If S is true and the binding cannot be set
    /// throw a TypeError exception.
    pub(crate) fn try_set_mutable_binding<'gc>(
        self,
        agent: &mut Agent,
        name: String,
        cache: Option<PropertyLookupCache>,
        value: Value,
        is_strict: bool,
        gc: NoGcScope<'gc, '_>,
    ) -> TryResult<'gc, SetResult<'gc>> {
        match self {
            Environment::Declarative(e) => js_result_into_try(
                e.set_mutable_binding(agent, name, value, is_strict, gc)
                    .map(|_| SetResult::Done),
            ),
            Environment::Function(e) => js_result_into_try(
                e.set_mutable_binding(agent, name, value, is_strict, gc)
                    .map(|_| SetResult::Done),
            ),
            Environment::Global(e) => {
                e.try_set_mutable_binding(agent, name, cache, value, is_strict, gc)
            }
            Environment::Module(e) => {
                debug_assert!(is_strict);
                js_result_into_try(
                    e.set_mutable_binding(agent, name, value, gc)
                        .map(|_| SetResult::Done),
                )
            }
            Environment::Object(e) => {
                e.try_set_mutable_binding(agent, name, cache, value, is_strict, gc)
            }
        }
    }

    /// # [SetMutableBinding(N, V, S)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Set the value of an already existing mutable binding in an Environment
    /// Record. The String value N is the text of the bound name. V is the
    /// value for the binding and may be a value of any ECMAScript language
    /// type. S is a Boolean flag. If S is true and the binding cannot be set
    /// throw a TypeError exception.
    pub(crate) fn set_mutable_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        cache: Option<PropertyLookupCache>,
        value: Value,
        is_strict: bool,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        match self {
            Environment::Declarative(e) => {
                e.set_mutable_binding(agent, name, value, is_strict, gc.into_nogc())
            }
            Environment::Function(e) => {
                e.set_mutable_binding(agent, name, value, is_strict, gc.into_nogc())
            }
            Environment::Global(e) => {
                e.set_mutable_binding(agent, name, cache, value, is_strict, gc)
            }
            Environment::Module(e) => e.set_mutable_binding(agent, name, value, gc.into_nogc()),
            Environment::Object(e) => {
                e.set_mutable_binding(agent, name, cache, value, is_strict, gc)
            }
        }
    }

    /// ### Try [GetBindingValue(N, S)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Returns the value of an already existing binding from an Environment
    /// Record. The String value N is the text of the bound name. S is used to
    /// identify references originating in strict mode code or that otherwise
    /// require strict mode reference semantics. If S is true and the binding
    /// does not exist throw a ReferenceError exception. If the binding exists
    /// but is uninitialized a ReferenceError is thrown, regardless of the
    /// value of S.
    pub(crate) fn try_get_binding_value(
        self,
        agent: &mut Agent,
        name: String,
        cache: Option<PropertyLookupCache>,
        is_strict: bool,
        gc: NoGcScope<'e, '_>,
    ) -> TryResult<'e, Value<'e>> {
        match self {
            Environment::Declarative(e) => {
                js_result_into_try(e.get_binding_value(agent, name, is_strict, gc))
            }
            Environment::Function(e) => {
                js_result_into_try(e.get_binding_value(agent, name, is_strict, gc))
            }
            Environment::Global(e) => e.try_get_binding_value(agent, name, cache, is_strict, gc),
            Environment::Module(e) => e.try_get_binding_value(agent, name, is_strict, gc),
            Environment::Object(e) => e.try_get_binding_value(agent, name, cache, is_strict, gc),
        }
    }

    /// # [GetBindingValue(N, S)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Returns the value of an already existing binding from an Environment
    /// Record. The String value N is the text of the bound name. S is used to
    /// identify references originating in strict mode code or that otherwise
    /// require strict mode reference semantics. If S is true and the binding
    /// does not exist throw a ReferenceError exception. If the binding exists
    /// but is uninitialized a ReferenceError is thrown, regardless of the
    /// value of S.
    pub(crate) fn get_binding_value<'a>(
        self,
        agent: &mut Agent,
        name: String,
        is_strict: bool,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, Value<'a>> {
        match self {
            Environment::Declarative(e) => {
                let gc = gc.into_nogc();
                e.bind(gc).get_binding_value(agent, name, is_strict, gc)
            }
            Environment::Function(e) => {
                let gc = gc.into_nogc();
                e.bind(gc).get_binding_value(agent, name, is_strict, gc)
            }
            Environment::Global(e) => e.get_binding_value(agent, name, is_strict, gc),
            Environment::Module(e) => {
                let gc = gc.into_nogc();
                e.bind(gc)
                    .env_get_binding_value(agent, name, is_strict, gc.into_nogc())
            }
            Environment::Object(e) => e.get_binding_value(agent, name, is_strict, gc),
        }
    }

    /// ### Try [DeleteBinding(N)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Delete a binding from an Environment Record. The String value N is the
    /// text of the bound name. If a binding for N exists, remove the binding
    /// and return true. If the binding exists but cannot be removed return
    /// false.
    pub(crate) fn try_delete_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        gc: NoGcScope<'a, '_>,
    ) -> TryResult<'a, bool> {
        match self {
            Environment::Declarative(e) => TryResult::Continue(e.delete_binding(agent, name)),
            Environment::Function(e) => TryResult::Continue(e.delete_binding(agent, name)),
            Environment::Global(e) => e.try_delete_binding(agent, name, gc),
            // NOTE: Module Environment Records are only used within strict
            // code and an early error rule prevents the delete operator, in
            // strict code, from being applied to a Reference Record that would
            // resolve to a Module Environment Record binding. See 13.5.1.1.
            Environment::Module(_) => unreachable!(),
            Environment::Object(e) => TryResult::Continue(e.try_delete_binding(agent, name, gc)?),
        }
    }

    /// # [DeleteBinding(N)](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Delete a binding from an Environment Record. The String value N is the
    /// text of the bound name. If a binding for N exists, remove the binding
    /// and return true. If the binding exists but cannot be removed return
    /// false.
    pub(crate) fn delete_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        gc: GcScope<'a, '_>,
    ) -> JsResult<'a, bool> {
        match self {
            Environment::Declarative(e) => Ok(e.delete_binding(agent, name)),
            Environment::Function(e) => Ok(e.delete_binding(agent, name)),
            Environment::Global(e) => e.delete_binding(agent, name, gc),
            // NOTE: Module Environment Records are only used within strict
            // code and an early error rule prevents the delete operator, in
            // strict code, from being applied to a Reference Record that would
            // resolve to a Module Environment Record binding. See 13.5.1.1.
            Environment::Module(_) => unreachable!(),
            Environment::Object(e) => e.delete_binding(agent, name, gc),
        }
    }

    /// # [HasThisBinding()](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Determine if an Environment Record establishes a this binding. Return
    /// true if it does and false if it does not.
    pub(crate) fn has_this_binding(self, agent: &Agent) -> bool {
        match self {
            Environment::Declarative(_) => false,
            Environment::Function(e) => e.has_this_binding(agent),
            Environment::Global(_) => true,
            Environment::Module(_) => true,
            Environment::Object(_) => false,
        }
    }

    /// Get the `this` binding value of this environment.
    ///
    /// ## Panics
    ///
    /// Panics if the environment does not have a `this` binding.
    pub(crate) fn get_this_binding(
        self,
        agent: &mut Agent,
        gc: NoGcScope<'e, '_>,
    ) -> JsResult<'e, Value<'e>> {
        match self {
            Environment::Function(e) => e.get_this_binding(agent, gc),
            Environment::Global(e) => Ok(e.get_this_binding(agent).into()),
            Environment::Module(_) => Ok(Value::Undefined),
            _ => unreachable!(),
        }
    }

    /// # [HasSuperBinding()](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// Determine if an Environment Record establishes a super method binding.
    /// Return true if it does and false if it does not.
    pub(crate) fn has_super_binding(self, agent: &mut Agent) -> bool {
        match self {
            Environment::Function(e) => e.has_super_binding(agent),
            _ => false,
        }
    }

    /// # [WithBaseObject()](https://tc39.es/ecma262/#table-abstract-methods-of-environment-records)
    ///
    /// If this Environment Record is associated with a with statement, return
    /// the with object. Otherwise, return undefined.
    pub(crate) fn with_base_object(self, agent: &mut Agent) -> Option<Object<'e>> {
        match self {
            Environment::Object(e) => e.with_base_object(agent),
            _ => None,
        }
    }
}

impl core::fmt::Debug for Environment<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Environment::Declarative(d) => {
                write!(f, "DeclarativeEnvironment({:?})", d.get_index_u32())
            }
            Environment::Function(d) => write!(f, "FunctionEnvironment({:?})", d.get_index_u32()),
            Environment::Global(d) => write!(f, "GlobalEnvironment({:?})", d.get_index_u32()),
            Environment::Module(d) => write!(f, "ModuleEnvironment({:?})", d.get_index_u32()),
            Environment::Object(d) => write!(f, "ObjectEnvironment({:?})", d.get_index_u32()),
            // EnvironmentIndex::Module(d) => {}
        }
    }
}

impl From<Environment<'_>> for HeapRootData {
    fn from(value: Environment<'_>) -> Self {
        match value {
            Environment::Declarative(e) => Self::from(e),
            Environment::Function(e) => Self::from(e),
            Environment::Global(e) => Self::from(e),
            Environment::Module(e) => Self::from(e),
            Environment::Object(e) => Self::from(e),
        }
    }
}

impl TryFrom<HeapRootData> for Environment<'_> {
    type Error = ();

    fn try_from(value: HeapRootData) -> Result<Self, Self::Error> {
        match value {
            HeapRootData::DeclarativeEnvironment(e) => Ok(Self::Declarative(e)),
            HeapRootData::FunctionEnvironment(e) => Ok(Self::Function(e)),
            HeapRootData::GlobalEnvironment(e) => Ok(Self::Global(e)),
            HeapRootData::ModuleEnvironment(e) => Ok(Self::Module(e)),
            HeapRootData::ObjectEnvironment(e) => Ok(Self::Object(e)),
            _ => Err(()),
        }
    }
}

impl HeapMarkAndSweep for Environment<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        match self {
            Environment::Declarative(e) => e.mark_values(queues),
            Environment::Function(e) => e.mark_values(queues),
            Environment::Global(e) => e.mark_values(queues),
            Environment::Module(e) => e.mark_values(queues),
            Environment::Object(e) => e.mark_values(queues),
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        match self {
            Environment::Declarative(e) => e.sweep_values(compactions),
            Environment::Function(e) => e.sweep_values(compactions),
            Environment::Global(e) => e.sweep_values(compactions),
            Environment::Module(e) => e.sweep_values(compactions),
            Environment::Object(e) => e.sweep_values(compactions),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Environments {
    pub(crate) declarative: Vec<DeclarativeEnvironmentRecord>,
    pub(crate) function: Vec<FunctionEnvironmentRecord>,
    pub(crate) global: Vec<GlobalEnvironmentRecord>,
    pub(crate) object: Vec<ObjectEnvironmentRecord>,
    pub(crate) module: Vec<ModuleEnvironmentRecord>,
    pub(crate) private: Vec<PrivateEnvironmentRecord>,
}

impl Default for Environments {
    fn default() -> Self {
        Self {
            declarative: Vec::new(),
            function: Vec::new(),
            global: Vec::with_capacity(1),
            object: Vec::new(),
            module: Vec::new(),
            private: Vec::new(),
        }
    }
}

/// Result of the HasBinding abstract operation's Try variant.
///
/// > Note: we could return eg. the exact object and offset that a property was
/// > found at, and cache that for later usage. Experiments showed that it did
/// > not have a meaningful impact on performance at that time.
#[derive(Bindable)]
pub(crate) enum TryHasBindingContinue<'a> {
    Result(bool),
    /// A Proxy trap call is needed.
    ///
    /// This means that the method ran to completion but could not call the
    /// Proxy trap itself.
    Proxy(Proxy<'a>),
}
impl<'a> TryFrom<TryHasBindingContinue<'a>> for bool {
    type Error = Proxy<'a>;

    fn try_from(value: TryHasBindingContinue<'a>) -> Result<Self, Self::Error> {
        match value {
            TryHasBindingContinue::Result(bool) => Ok(bool),
            TryHasBindingContinue::Proxy(proxy) => Err(proxy),
        }
    }
}

impl<'a> From<TryHasResult<'a>> for TryHasBindingContinue<'a> {
    fn from(value: TryHasResult<'a>) -> Self {
        match value {
            TryHasResult::Unset => Self::Result(false),
            TryHasResult::Offset(_, _) | TryHasResult::Custom(_, _) => Self::Result(true),
            TryHasResult::Proxy(proxy) => Self::Proxy(proxy),
        }
    }
}

impl<'a> From<TryHasResult<'a>> for TryResult<'a, TryHasBindingContinue<'a>> {
    fn from(value: TryHasResult<'a>) -> Self {
        Self::Continue(value.into())
    }
}

impl<'a> From<TryHasBindingContinue<'a>> for TryResult<'a, TryHasBindingContinue<'a>> {
    fn from(value: TryHasBindingContinue<'a>) -> Self {
        Self::Continue(value)
    }
}

/// Where a name was found from one place in the code: so many Environment
/// Records out from the running one, at such a place among the bindings
/// there.
///
/// This is remembered only for a name whose scope chain cannot change
/// between two runs of that place in the code (the compiler says so where
/// no `with` and no direct `eval` is in scope): the records on the way are
/// then declarative ones made by the same code in the same order every time,
/// so where the name was found once is where it is found again. The
/// remembered place is still checked when it is used, by the kinds of the
/// records on the way and by the name at the place; when the check fails the
/// name is resolved the long way.
///
/// A name found in the global scope (which the compiler allows where the
/// chain is fixed all the way out, in a Script) is remembered there without
/// the records on the way: among the lexical declarations of the global
/// scope, or among the properties of the global object. The second holds
/// only while no lexical declaration has been added, since one of the same
/// name would come first; and the name is checked at the place as elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BindingSite {
    outwards: u16,
    /// For a property of the global object: how many lexical declarations
    /// the global scope had when the name was not among them.
    lexical: u16,
    slot: u32,
}

impl BindingSite {
    /// Nothing is remembered yet.
    pub(crate) const UNKNOWN: Self = Self {
        outwards: u16::MAX,
        lexical: 0,
        slot: 0,
    };

    /// The name was found where no place can be remembered (a module
    /// record, or past one): it is resolved the long way every time.
    pub(crate) const ELSEWHERE: Self = Self {
        outwards: u16::MAX - 1,
        lexical: 0,
        slot: 0,
    };

    /// `slot` is among the lexical declarations of the global scope.
    const GLOBAL_LEXICAL: u16 = u16::MAX - 2;

    /// `slot` is among the properties of the global object.
    const GLOBAL_OBJECT: u16 = u16::MAX - 3;

    /// The name is in the global scope, at no place that can be remembered
    /// (an accessor of the global object, a property it inherits).
    const GLOBAL_SOMEWHERE: u16 = u16::MAX - 4;

    const FARTHEST: u16 = u16::MAX - 5;

    /// The Environment Record this many records out from the running one,
    /// if all on the way are declarative.
    #[inline]
    fn record<'a>(self, agent: &Agent, gc: NoGcScope<'a, '_>) -> Option<Environment<'a>> {
        if self.outwards > Self::FARTHEST {
            return None;
        }
        let environments = &agent.heap.environments;
        let mut env = agent.current_lexical_environment(gc);
        for _ in 0..self.outwards {
            env = environments.declarative_record(env)?.outer_env()?.bind(gc);
        }
        Some(env)
    }

    /// Whether the name was found in the global scope.
    #[inline]
    pub(crate) fn is_global(self) -> bool {
        self.outwards > Self::FARTHEST && self.outwards < Self::ELSEWHERE.outwards
    }
}

impl Environments {
    /// The bindings of a Declarative or a Function Environment Record.
    #[inline]
    fn declarative_record(&self, env: Environment) -> Option<&DeclarativeEnvironmentRecord> {
        match env {
            Environment::Declarative(e) => Some(self.get_declarative_environment(e)),
            Environment::Function(e) => Some(self.get_declarative_environment(
                self.get_function_environment(e).declarative_environment(),
            )),
            _ => None,
        }
    }

    #[inline]
    fn declarative_record_mut(
        &mut self,
        env: Environment,
    ) -> Option<&mut DeclarativeEnvironmentRecord> {
        match env {
            Environment::Declarative(e) => Some(self.get_declarative_environment_mut(e)),
            Environment::Function(e) => {
                let e = self.get_function_environment(e).declarative_environment();
                Some(self.get_declarative_environment_mut(e))
            }
            _ => None,
        }
    }
}

/// The lexical declarations of the running realm's global scope.
#[inline]
fn global_lexical_record<'a>(agent: &Agent, gc: NoGcScope<'a, '_>) -> DeclarativeEnvironment<'a> {
    agent
        .heap
        .environments
        .get_global_environment(agent.current_global_env(gc))
        .declarative_record()
        .bind(gc)
}

/// The global object, if `site` remembers `name` among its properties and
/// the name is still there, with no lexical declaration added to the global
/// scope since: the property is then what the name means.
#[inline]
fn global_object_at_site<'a>(
    agent: &Agent,
    site: BindingSite,
    name: String,
    gc: NoGcScope<'a, '_>,
) -> Option<OrdinaryObject<'a>> {
    let environments = &agent.heap.environments;
    let global = environments.get_global_environment(agent.current_global_env(gc));
    let lexical = environments.get_declarative_environment(global.declarative_record());
    if lexical.binding_count() != usize::from(site.lexical) {
        return None;
    }
    let Object::Object(object) = global.object_record().get_binding_object(agent) else {
        return None;
    };
    let keys = object.get_property_keys(agent);
    (keys.get(site.slot as usize) == Some(&name.to_property_key())).then_some(object.bind(gc))
}

/// The value of `name` where `site` remembers it in the global scope, if it
/// is still found there as a value (a lexical declaration that is
/// initialized, a data property of the global object).
fn global_value_at_site<'a>(
    agent: &Agent,
    site: BindingSite,
    name: String,
    gc: NoGcScope<'a, '_>,
) -> Option<Value<'a>> {
    match site.outwards {
        BindingSite::GLOBAL_LEXICAL => agent
            .heap
            .environments
            .get_declarative_environment(global_lexical_record(agent, gc))
            .binding_at(site.slot, name)?
            .value
            .bind(gc),
        BindingSite::GLOBAL_OBJECT => {
            let object = global_object_at_site(agent, site, name, gc)?;
            // An empty slot is an accessor.
            object
                .get_elements_values(agent)
                .get(site.slot as usize)
                .copied()
                .flatten()
        }
        _ => None,
    }
}

/// Sets `name` where `site` remembers it in the global scope, if it is
/// still found there as something an assignment simply writes: a lexical
/// declaration that is mutable and initialized, a writable data property of
/// the global object.
fn set_global_value_at_site(
    agent: &mut Agent,
    site: BindingSite,
    name: String,
    value: Value,
    gc: NoGcScope,
) -> bool {
    match site.outwards {
        BindingSite::GLOBAL_LEXICAL => {
            let record = global_lexical_record(agent, gc).unbind();
            let Some(binding) = agent
                .heap
                .environments
                .get_declarative_environment_mut(record)
                .binding_at_mut(site.slot, name)
            else {
                return false;
            };
            if !binding.mutable || binding.value.is_none() {
                return false;
            }
            binding.value = Some(value.unbind());
            true
        }
        BindingSite::GLOBAL_OBJECT => {
            let Some(object) = global_object_at_site(agent, site, name, gc) else {
                return false;
            };
            let Some(offset) = PropertyOffset::new(site.slot) else {
                return false;
            };
            ordinary_set_own_data_at_offset(agent, object.unbind(), offset, value)
        }
        _ => false,
    }
}

/// The value of `name` where `site` remembers it, if the name is still found
/// there and is initialized.
#[inline]
pub(crate) fn binding_value_at_site<'a>(
    agent: &Agent,
    site: BindingSite,
    name: String,
    gc: NoGcScope<'a, '_>,
) -> Option<Value<'a>> {
    let Some(env) = site.record(agent, gc) else {
        return global_value_at_site(agent, site, name, gc);
    };
    agent
        .heap
        .environments
        .declarative_record(env)?
        .binding_at(site.slot, name)?
        .value
        .bind(gc)
}

/// Sets `name` where `site` remembers it to `value`, if the name is still
/// found there, mutable and initialized; false leaves it to PutValue.
#[inline]
pub(crate) fn set_binding_value_at_site(
    agent: &mut Agent,
    site: BindingSite,
    name: String,
    value: Value,
    gc: NoGcScope,
) -> bool {
    let Some(env) = site.record(agent, gc) else {
        return set_global_value_at_site(agent, site, name, value, gc);
    };
    let Some(binding) = agent
        .heap
        .environments
        .declarative_record_mut(env.unbind())
        .and_then(|record| record.binding_at_mut(site.slot, name))
    else {
        return false;
    };
    if !binding.mutable || binding.value.is_none() {
        return false;
    }
    binding.value = Some(value.unbind());
    true
}

/// Whether `reference` is to a name of the running realm's global scope and
/// `site` remembers a place there: what the reference would read or write is
/// then what is at the place, as long as the place still holds the name.
#[inline]
pub(crate) fn reference_is_at_global_site(
    agent: &Agent,
    site: BindingSite,
    reference: &Reference,
    gc: NoGcScope,
) -> bool {
    site.is_global()
        && matches!(
            reference,
            Reference::Variable(_) | Reference::VariableStrict(_)
        )
        && reference.base_env() == Environment::Global(agent.current_global_env(gc))
}

/// The Reference to `name` where `site` remembers it, if the name is still
/// found there.
#[inline]
pub(crate) fn try_resolve_binding_at_site<'a>(
    agent: &Agent,
    site: BindingSite,
    name: String<'a>,
    strict: bool,
    gc: NoGcScope<'a, '_>,
) -> Option<Reference<'a>> {
    let Some(env) = site.record(agent, gc) else {
        return try_resolve_global_at_site(agent, site, name, strict, gc);
    };
    agent
        .heap
        .environments
        .declarative_record(env)?
        .binding_at(site.slot, name)?;
    Some(Reference::new_variable_reference(env, name, None, strict))
}

/// The Reference to `name` in the running realm's global scope, where the
/// name is known to have been resolved to.
pub(crate) fn global_scope_reference<'a>(
    agent: &Agent,
    name: String<'a>,
    strict: bool,
    gc: NoGcScope<'a, '_>,
) -> Reference<'a> {
    let env = Environment::Global(agent.current_global_env(gc));
    Reference::new_variable_reference(env, name, None, strict)
}

/// The Reference to `name` in the global scope, if `site` remembers it
/// there and it is still there.
fn try_resolve_global_at_site<'a>(
    agent: &Agent,
    site: BindingSite,
    name: String<'a>,
    strict: bool,
    gc: NoGcScope<'a, '_>,
) -> Option<Reference<'a>> {
    match site.outwards {
        BindingSite::GLOBAL_LEXICAL => {
            agent
                .heap
                .environments
                .get_declarative_environment(global_lexical_record(agent, gc))
                .binding_at(site.slot, name)?;
        }
        BindingSite::GLOBAL_OBJECT => {
            global_object_at_site(agent, site, name, gc)?;
        }
        _ => return None,
    }
    Some(global_scope_reference(agent, name, strict, gc))
}

/// What to remember of where `reference` found its name
/// ([`BindingSite`]). `to_root` says that the scope chain is fixed all the
/// way out to the global scope of a Script, so that a place there may be
/// remembered too.
pub(crate) fn binding_site_of(
    agent: &Agent,
    reference: &Reference,
    to_root: bool,
    gc: NoGcScope,
) -> BindingSite {
    let (Reference::Variable(_) | Reference::VariableStrict(_)) = reference else {
        // A name that is nowhere may yet come to be in the global scope.
        return if to_root {
            BindingSite::UNKNOWN
        } else {
            BindingSite::ELSEWHERE
        };
    };
    let base = reference.base_env();
    let name = reference.referenced_name_string();
    if let Environment::Global(global) = base {
        return if to_root {
            global_binding_site(agent, global, name, gc)
        } else {
            BindingSite::ELSEWHERE
        };
    }
    let environments = &agent.heap.environments;
    let mut env = agent.current_lexical_environment(gc);
    let mut outwards = 0u16;
    loop {
        let Some(record) = environments.declarative_record(env) else {
            return BindingSite::ELSEWHERE;
        };
        if env == base.bind(gc) {
            return match record.binding_slot(name) {
                Some(slot) => BindingSite {
                    outwards,
                    lexical: 0,
                    slot,
                },
                None => BindingSite::ELSEWHERE,
            };
        }
        let Some(outer) = record.outer_env() else {
            return BindingSite::ELSEWHERE;
        };
        if outwards == BindingSite::FARTHEST {
            return BindingSite::ELSEWHERE;
        }
        outwards += 1;
        env = outer.bind(gc);
    }
}

/// Where `name` is in the global scope `global`, as a place to remember:
/// among its lexical declarations, or as a data property of the global
/// object when that is an ordinary one.
fn global_binding_site(
    agent: &Agent,
    global: GlobalEnvironment,
    name: String,
    gc: NoGcScope,
) -> BindingSite {
    if global != agent.current_global_env(gc) {
        return BindingSite::ELSEWHERE;
    }
    let environments = &agent.heap.environments;
    let global = environments.get_global_environment(global);
    let lexical = environments.get_declarative_environment(global.declarative_record());
    if let Some(slot) = lexical.binding_slot(name) {
        return BindingSite {
            outwards: BindingSite::GLOBAL_LEXICAL,
            lexical: 0,
            slot,
        };
    }
    // In the global scope, where no place holds it as a value: a property
    // that the global object inherits, an accessor, a global object that is
    // not an ordinary one.
    let somewhere = BindingSite {
        outwards: BindingSite::GLOBAL_SOMEWHERE,
        lexical: 0,
        slot: 0,
    };
    let Ok(lexical) = u16::try_from(lexical.binding_count()) else {
        return somewhere;
    };
    let Object::Object(object) = global.object_record().get_binding_object(agent) else {
        return somewhere;
    };
    let key = name.to_property_key();
    let keys = object.get_property_keys(agent);
    let Some(slot) = keys.iter().position(|k| *k == key) else {
        return somewhere;
    };
    let is_value = matches!(object.get_elements_values(agent).get(slot), Some(Some(_)));
    match u32::try_from(slot) {
        Ok(slot) if is_value && PropertyOffset::new(slot).is_some() => BindingSite {
            outwards: BindingSite::GLOBAL_OBJECT,
            lexical,
            slot,
        },
        _ => somewhere,
    }
}

/// ### Try [9.1.2.1 GetIdentifierReference ( env, name, strict )](https://tc39.es/ecma262/#sec-getidentifierreference)
///
/// The abstract operation GetIdentifierReference takes arguments env (an
/// Environment Record or null), name (a String), and strict (a Boolean) and
/// returns either a normal completion containing a Reference Record or a throw
/// completion.
pub(crate) fn try_get_identifier_reference<'a>(
    agent: &mut Agent,
    env: Environment,
    name: String,
    cache: Option<PropertyLookupCache>,
    strict: bool,
    gc: NoGcScope<'a, '_>,
) -> TryResult<'a, Reference<'a>> {
    let mut env = env.bind(gc);
    let name = name.bind(gc);
    let cache = cache.bind(gc);
    loop {
        // 1. If env is null, then
        // 2. Let exists be ? env.HasBinding(name).
        let outer = match env {
            Environment::Declarative(e) => {
                let env_rec = agent.heap.environments.get_declarative_environment(e);
                if env_rec.has_binding(name) {
                    return TryResult::Continue(Reference::new_variable_reference(
                        env, name, cache, strict,
                    ));
                }
                env_rec.outer_env().bind(gc)
            }
            Environment::Function(e) => {
                let environments = &agent.heap.environments;
                let dcl_env = environments
                    .get_function_environment(e)
                    .declarative_environment();
                let env_rec = environments.get_declarative_environment(dcl_env);
                if env_rec.has_binding(name) {
                    return TryResult::Continue(Reference::new_variable_reference(
                        env, name, cache, strict,
                    ));
                }
                env_rec.outer_env().bind(gc)
            }
            _ => {
                let exists = if let ControlFlow::Continue(TryHasBindingContinue::Result(exists)) =
                    env.try_has_binding(agent, name, cache, gc)
                {
                    exists
                } else {
                    return TryError::GcError.into();
                };

                // 3. If exists is true, then
                if exists {
                    // a. Return the Reference Record {
                    // [[ReferencedName]]: name,
                    // [[Base]]: env,
                    // [[Strict]]: strict,
                    return TryResult::Continue(Reference::new_variable_reference(
                        env, name, cache, strict,
                    ));
                    // [[ThisValue]]: EMPTY
                    // }.
                }
                // 4. Else,
                // a. Let outer be env.[[OuterEnv]].
                env.get_outer_env(agent)
            }
        };

        let Some(outer) = outer else {
            // a. Return the Reference Record {
            // [[Base]]: UNRESOLVABLE,
            // [[ReferencedName]]: name,
            // [[Strict]]: strict,
            return TryResult::Continue(Reference::new_unresolvable_reference(name, strict));
            // [[ThisValue]]: EMPTY
            // }.
        };

        // b. Return ? GetIdentifierReference(outer, name, strict).
        env = outer;
    }
}

/// ### [9.1.2.1 GetIdentifierReference ( env, name, strict )](https://tc39.es/ecma262/#sec-getidentifierreference)
///
/// The abstract operation GetIdentifierReference takes arguments env (an
/// Environment Record or null), name (a String), and strict (a Boolean) and
/// returns either a normal completion containing a Reference Record or a throw
/// completion.
pub(crate) fn get_identifier_reference<'a, 'b>(
    agent: &mut Agent,
    env: Option<Environment>,
    name: String,
    cache: Option<PropertyLookupCache>,
    strict: bool,
    mut gc: GcScope<'a, 'b>,
) -> JsResult<'a, Reference<'a>> {
    let env = env.bind(gc.nogc());
    let mut name = name.bind(gc.nogc());
    let mut cache = cache.bind(gc.nogc());

    // 1. If env is null, then
    let Some(mut env) = env else {
        let name = name.unbind().bind(gc.into_nogc());
        // a. Return the Reference Record {
        // [[Base]]: UNRESOLVABLE,
        // [[ReferencedName]]: name,
        // [[Strict]]: strict,
        return Ok(Reference::new_unresolvable_reference(name, strict));
        // [[ThisValue]]: EMPTY
        // }.
    };

    // 2. Let exists be ? env.HasBinding(name).
    let exists = env.try_has_binding(agent, name, cache, gc.nogc());
    let exists = if let ControlFlow::Continue(TryHasBindingContinue::Result(exists)) = exists {
        exists
    } else {
        let env_scoped = env.scope(agent, gc.nogc());
        let name_scoped = name.scope(agent, gc.nogc());
        let cache_scoped = cache.map(|c| c.scope(agent, gc.nogc()));
        let exists = handle_try_has_binding_result_cold(
            agent,
            env.unbind(),
            name.unbind(),
            exists.unbind(),
            gc.reborrow(),
        )
        .unbind()?
        .bind(gc.nogc());
        {
            cache = cache_scoped.map(|c| c.take(agent));
            name = name_scoped.take(agent);
            env = env_scoped.take(agent);
        }
        exists
    };

    // 3. If exists is true, then
    if exists {
        // a. Return the Reference Record {
        // [[Base]]: env,
        // [[ReferencedName]]: name,
        // [[Strict]]: strict,
        Ok(Reference::new_variable_reference(env, name, cache, strict).unbind())
        // [[ThisValue]]: EMPTY
        // }.
    }
    // 4. Else,
    else {
        // a. Let outer be env.[[OuterEnv]].
        let outer = env.get_outer_env(agent);

        // b. Return ? GetIdentifierReference(outer, name, strict).
        get_identifier_reference(
            agent,
            outer.unbind(),
            name.unbind(),
            cache.unbind(),
            strict,
            gc,
        )
    }
}

#[cold]
#[inline(never)]
fn handle_try_has_binding_result_cold<'a>(
    agent: &mut Agent,
    env: Environment,
    name: String,
    exists: ControlFlow<TryError, TryHasBindingContinue>,
    gc: GcScope<'a, '_>,
) -> JsResult<'a, bool> {
    match exists {
        ControlFlow::Continue(c) => match c {
            TryHasBindingContinue::Result(exists) => Ok(exists),
            TryHasBindingContinue::Proxy(proxy) => {
                proxy
                    .unbind()
                    .internal_has_property(agent, name.to_property_key(), gc)
            }
        },
        ControlFlow::Break(b) => match b {
            TryError::Err(err) => Err(err.unbind().bind(gc.into_nogc())),
            _ => env.unbind().has_binding(agent, name.unbind(), gc),
        },
    }
}

impl Environments {
    pub(crate) fn push_declarative_environment<'a>(
        &mut self,
        env: DeclarativeEnvironmentRecord,
        _: NoGcScope<'a, '_>,
    ) -> DeclarativeEnvironment<'a> {
        self.declarative.push(env);
        DeclarativeEnvironment::from_index_u32(self.declarative.len() as u32 - 1)
    }

    pub(crate) fn push_function_environment<'a>(
        &mut self,
        env: FunctionEnvironmentRecord,
        _: NoGcScope<'a, '_>,
    ) -> FunctionEnvironment<'a> {
        self.function.push(env);
        FunctionEnvironment::from_index_u32(self.function.len() as u32 - 1)
    }

    pub(crate) fn push_global_environment<'a>(
        &mut self,
        env: GlobalEnvironmentRecord,
        _: NoGcScope<'a, '_>,
    ) -> GlobalEnvironment<'a> {
        self.global.push(env);
        GlobalEnvironment::from_index_u32(self.global.len() as u32 - 1)
    }

    pub(crate) fn push_module_environment<'a>(
        &mut self,
        env: ModuleEnvironmentRecord,
        _: NoGcScope<'a, '_>,
    ) -> ModuleEnvironment<'a> {
        self.module.push(env);
        ModuleEnvironment::from_index_u32(self.module.len() as u32 - 1)
    }

    pub(crate) fn push_object_environment<'a>(
        &mut self,
        env: ObjectEnvironmentRecord,
        decl_env: DeclarativeEnvironmentRecord,
        _: NoGcScope<'a, '_>,
    ) -> (ObjectEnvironment<'a>, DeclarativeEnvironment<'a>) {
        self.object.push(env);
        self.declarative.push(decl_env);
        (
            ObjectEnvironment::from_index_u32(self.object.len() as u32 - 1),
            DeclarativeEnvironment::from_index_u32(self.declarative.len() as u32 - 1),
        )
    }

    pub(crate) fn push_private_environment<'a>(
        &mut self,
        env: PrivateEnvironmentRecord,
        _: NoGcScope<'a, '_>,
    ) -> PrivateEnvironment<'a> {
        self.private.push(env);
        PrivateEnvironment::from_index_u32(self.private.len() as u32 - 1)
    }

    pub(crate) fn get_declarative_environment(
        &self,
        index: DeclarativeEnvironment,
    ) -> &DeclarativeEnvironmentRecord {
        self.declarative
            .get(index.get_index())
            .expect("DeclarativeEnvironment did not match to any vector index")
    }

    pub(crate) fn get_declarative_environment_mut(
        &mut self,
        index: DeclarativeEnvironment,
    ) -> &mut DeclarativeEnvironmentRecord {
        self.declarative
            .get_mut(index.get_index())
            .expect("DeclarativeEnvironment did not match to any vector index")
    }

    pub(crate) fn get_function_environment(
        &self,
        index: FunctionEnvironment,
    ) -> &FunctionEnvironmentRecord {
        self.function
            .get(index.get_index())
            .expect("FunctionEnvironment did not match to any vector index")
    }

    #[expect(dead_code)]
    pub(crate) fn get_function_environment_mut(
        &mut self,
        index: FunctionEnvironment,
    ) -> &mut FunctionEnvironmentRecord {
        self.function
            .get_mut(index.get_index())
            .expect("FunctionEnvironment did not match to any vector index")
    }

    pub(crate) fn get_module_environment(
        &self,
        index: ModuleEnvironment,
    ) -> &ModuleEnvironmentRecord {
        self.module
            .get(index.get_index())
            .expect("ModuleEnvironment did not match to any vector index")
    }

    pub(crate) fn get_module_environment_mut(
        &mut self,
        index: ModuleEnvironment,
    ) -> &mut ModuleEnvironmentRecord {
        self.module
            .get_mut(index.get_index())
            .expect("ModuleEnvironment did not match to any vector index")
    }

    #[expect(dead_code)]
    pub(crate) fn get_global_environment(
        &self,
        index: GlobalEnvironment,
    ) -> &GlobalEnvironmentRecord {
        self.global
            .get(index.get_index())
            .expect("GlobalEnvironment did not match to any vector index")
    }

    #[expect(dead_code)]
    pub(crate) fn get_global_environment_mut(
        &mut self,
        index: GlobalEnvironment,
    ) -> &mut GlobalEnvironmentRecord {
        self.global
            .get_mut(index.get_index())
            .expect("GlobalEnvironment did not match to any vector index")
    }

    #[expect(dead_code)]
    pub(crate) fn get_object_environment(
        &self,
        index: ObjectEnvironment,
    ) -> &ObjectEnvironmentRecord {
        self.object
            .get(index.get_index())
            .expect("ObjectEnvironment did not match to any vector index")
    }

    #[expect(dead_code)]
    pub(crate) fn get_object_environment_mut(
        &mut self,
        index: ObjectEnvironment,
    ) -> &mut ObjectEnvironmentRecord {
        self.object
            .get_mut(index.get_index())
            .expect("ObjectEnvironment did not match to any vector index")
    }

    pub(crate) fn get_private_environment(
        &self,
        index: PrivateEnvironment,
    ) -> &PrivateEnvironmentRecord {
        self.private
            .get(index.get_index())
            .expect("PrivateEnvironment did not match to any vector index")
    }

    pub(crate) fn get_private_environment_mut(
        &mut self,
        index: PrivateEnvironment,
    ) -> &mut PrivateEnvironmentRecord {
        self.private
            .get_mut(index.get_index())
            .expect("PrivateEnvironment did not match to any vector index")
    }
}

/// ### [9.4.3 GetThisEnvironment ( )](https://tc39.es/ecma262/#sec-getthisenvironment)
/// The abstract operation GetThisEnvironment takes no arguments and returns an
/// Environment Record. It finds the Environment Record that currently supplies
/// the binding of the keyword this.
pub(crate) fn get_this_environment<'a>(agent: &Agent, gc: NoGcScope<'a, '_>) -> Environment<'a> {
    // 1. Let env be the running execution context's LexicalEnvironment.
    let mut env = agent.current_lexical_environment(gc);
    // 2. Repeat,
    loop {
        // a. Let exists be env.HasThisBinding().
        // b. If exists is true, return env.
        if env.has_this_binding(agent) {
            return env;
        }
        // c. Let outer be env.[[OuterEnv]].
        // d. Assert: outer is not null.
        // e. Set env to outer.
        env = env.get_outer_env(agent).unwrap();
    }
}

impl AsRef<Environments> for Environments {
    fn as_ref(&self) -> &Environments {
        self
    }
}

impl AsMut<Environments> for Environments {
    fn as_mut(&mut self) -> &mut Environments {
        self
    }
}

#[doc(hidden)]
impl AsRef<Environments> for Agent {
    fn as_ref(&self) -> &Environments {
        &self.heap.environments
    }
}

#[doc(hidden)]
impl AsMut<Environments> for Agent {
    fn as_mut(&mut self) -> &mut Environments {
        &mut self.heap.environments
    }
}
