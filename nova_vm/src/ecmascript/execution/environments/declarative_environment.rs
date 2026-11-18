// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use core::cell::Cell;

use crate::hashing::AHashMap;

use crate::{
    ecmascript::{
        Agent, DeclarativeEnvironment, Environment, Environments, ExceptionType, JsResult,
        OuterEnv, String, Value,
    },
    engine::{Bindable, NoGcScope},
    heap::{
        ArenaAccess, ArenaAccessMut, CompactionLists, HeapIndexHandle, HeapMarkAndSweep, WorkQueues,
    },
};

/// ### [9.1.1.1 Declarative Environment Records](https://tc39.es/ecma262/#sec-declarative-environment-records)
///
/// A Declarative Environment Record is used to define the effect of ECMAScript
/// language syntactic elements such as FunctionDeclarations,
/// VariableDeclarations, and Catch clauses that directly associate identifier
/// bindings with ECMAScript language values.
#[derive(Debug, Clone)]
pub(crate) struct DeclarativeEnvironmentRecord {
    /// ### \[\[OuterEnv\]\]
    ///
    /// See [OuterEnv].
    outer_env: OuterEnv<'static>,

    /// The environment's bindings.
    bindings: BindingMap,
}

/// Bindings in insertion order, searched linearly while there are few of
/// them and through a hash index once there are more than
/// [`LINEAR_BINDINGS`].
#[derive(Debug, Clone, Default)]
struct BindingMap {
    entries: Vec<(String<'static>, Binding)>,
    index: Option<Box<BindingIndex>>,
    last_found: Cell<u32>,
}

/// The hash index of a large [`BindingMap`] and a 256-bit filter over its
/// names. A clear filter bit proves that a name is absent, so a lookup that
/// misses (a global object name such as `Math` tried against the global
/// lexical record first) usually skips hashing. Bits are only ever set until
/// the index is rebuilt, so the filter can claim a name that is gone but
/// never miss one that is present.
#[derive(Debug, Clone)]
struct BindingIndex {
    map: AHashMap<String<'static>, u32>,
    filter: [u64; 4],
}

impl BindingIndex {
    #[inline]
    fn filter_bit(name: String<'static>) -> usize {
        let key = match name {
            String::String(name) => u64::from(name.get_index_u32()),
            String::SmallString(name) => {
                let mut bytes = [0u8; 8];
                bytes[..7].copy_from_slice(name.data());
                u64::from_le_bytes(bytes) | (1 << 63)
            }
        };
        (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56) as usize
    }

    #[inline]
    fn may_contain(&self, name: String<'static>) -> bool {
        let bit = Self::filter_bit(name);
        self.filter[bit / 64] & (1 << (bit % 64)) != 0
    }

    fn insert(&mut self, name: String<'static>, position: u32) {
        let bit = Self::filter_bit(name);
        self.filter[bit / 64] |= 1 << (bit % 64);
        self.map.insert(name, position);
    }
}

const LINEAR_BINDINGS: usize = 16;

/// What the garbage collector's allocation counter charges for a Declarative
/// Environment Record: the record's size when its bindings were a hash map,
/// so that collections, and so what a WeakRef sees, happen at the same points.
pub(crate) const DECLARATIVE_ENVIRONMENT_RECORD_ALLOC_SIZE: usize =
    core::mem::size_of::<Option<HashMapDeclarativeEnvironmentRecord>>();

#[expect(dead_code)]
struct HashMapDeclarativeEnvironmentRecord {
    outer_env: OuterEnv<'static>,
    bindings: AHashMap<String<'static>, Binding>,
}

impl BindingMap {
    /// Bytes held by the entries and the hash index, if any.
    fn bytes(&self) -> usize {
        let entries = self.entries.capacity() * core::mem::size_of::<(String<'static>, Binding)>();
        let index = self.index.as_ref().map_or(0, |index| {
            index.map.capacity() * (core::mem::size_of::<(String<'static>, u32)>() + 8)
        });
        entries + index
    }

    #[inline]
    fn position(&self, name: String) -> Option<usize> {
        let name = name.unbind();
        if let Some(index) = &self.index {
            let last_found = self.last_found.get() as usize;
            if self
                .entries
                .get(last_found)
                .is_some_and(|(key, _)| *key == name)
            {
                return Some(last_found);
            }
            if !index.may_contain(name) {
                return None;
            }
            let position = *index.map.get(&name)?;
            self.last_found.set(position);
            Some(position as usize)
        } else {
            self.entries.iter().position(|(key, _)| *key == name)
        }
    }

    #[inline]
    fn contains_key(&self, name: String) -> bool {
        self.position(name).is_some()
    }

    /// The binding at `slot` if it is the one for `name`: a binding keeps its
    /// place while none is deleted, so a place remembered for a name is
    /// checked with one comparison.
    #[inline]
    fn at(&self, slot: u32, name: String) -> Option<&Binding> {
        match self.entries.get(slot as usize) {
            Some((key, binding)) if *key == name.unbind() => Some(binding),
            _ => None,
        }
    }

    #[inline]
    fn at_mut(&mut self, slot: u32, name: String) -> Option<&mut Binding> {
        match self.entries.get_mut(slot as usize) {
            Some((key, binding)) if *key == name.unbind() => Some(binding),
            _ => None,
        }
    }

    #[inline]
    fn get(&self, name: String) -> Option<&Binding> {
        let position = self.position(name)?;
        self.entries.get(position).map(|(_, binding)| binding)
    }

    #[inline]
    fn get_mut(&mut self, name: String) -> Option<&mut Binding> {
        let position = self.position(name)?;
        self.entries.get_mut(position).map(|(_, binding)| binding)
    }

    fn insert(&mut self, name: String, binding: Binding) {
        let name = name.unbind();
        if let Some(position) = self.position(name) {
            self.entries[position].1 = binding;
            return;
        }
        let position = u32::try_from(self.entries.len()).expect("too many bindings");
        self.entries.push((name, binding));
        if let Some(index) = &mut self.index {
            index.insert(name, position);
        } else if self.entries.len() > LINEAR_BINDINGS {
            self.rebuild_index();
        }
    }

    fn remove(&mut self, name: String) {
        let Some(position) = self.position(name) else {
            return;
        };
        self.entries.swap_remove(position);
        if let Some(index) = &mut self.index {
            index.map.remove(&name.unbind());
            if let Some((moved, _)) = self.entries.get(position) {
                index.insert(*moved, u32::try_from(position).expect("too many bindings"));
            }
        }
    }

    fn rebuild_index(&mut self) {
        let mut map = AHashMap::default();
        map.reserve(self.entries.len());
        let mut index = BindingIndex {
            map,
            filter: [0; 4],
        };
        for (position, (key, _)) in self.entries.iter().enumerate() {
            index.insert(*key, u32::try_from(position).expect("too many bindings"));
        }
        self.index = Some(Box::new(index));
    }
}

impl HeapMarkAndSweep for BindingMap {
    fn mark_values(&self, queues: &mut WorkQueues) {
        for (key, binding) in &self.entries {
            key.mark_values(queues);
            binding.mark_values(queues);
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        for (key, binding) in &mut self.entries {
            key.sweep_values(compactions);
            binding.sweep_values(compactions);
        }
        if self.index.is_some() {
            self.rebuild_index();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Binding {
    pub(crate) value: Option<Value<'static>>,
    // TODO: Pack these into bitfields.
    pub(super) strict: bool,
    pub(super) mutable: bool,
    pub(super) deletable: bool,
}

impl DeclarativeEnvironmentRecord {
    /// Bytes held by the bindings table, for the heap size estimate.
    pub(crate) fn bindings_bytes(&self) -> usize {
        self.bindings.bytes()
    }

    pub(super) fn outer_env(&self) -> OuterEnv<'static> {
        self.outer_env
    }

    /// How many bindings the record holds.
    #[inline]
    pub(super) fn binding_count(&self) -> usize {
        self.bindings.entries.len()
    }

    /// The place of the binding for `name`, for [`Self::binding_at`].
    pub(super) fn binding_slot(&self, name: String) -> Option<u32> {
        self.bindings
            .position(name)
            .and_then(|slot| u32::try_from(slot).ok())
    }

    /// The binding at `slot` if it is the one for `name`.
    #[inline]
    pub(super) fn binding_at(&self, slot: u32, name: String) -> Option<&Binding> {
        self.bindings.at(slot, name)
    }

    #[inline]
    pub(super) fn binding_at_mut(&mut self, slot: u32, name: String) -> Option<&mut Binding> {
        self.bindings.at_mut(slot, name)
    }
}

impl DeclarativeEnvironmentRecord {
    /// ### [9.1.2.2 NewDeclarativeEnvironment ( E )](https://tc39.es/ecma262/#sec-newdeclarativeenvironment)
    ///
    /// The abstract operation NewDeclarativeEnvironment takes argument E (an
    /// Environment Record or null) and returns a Declarative Environment
    /// Record.
    pub(crate) fn new(outer_env: OuterEnv) -> DeclarativeEnvironmentRecord {
        // 1. Let env be a new Declarative Environment Record containing no bindings.
        // 2. Set env.[[OuterEnv]] to E.
        // 3. Return env.
        DeclarativeEnvironmentRecord {
            outer_env: outer_env.unbind(),
            bindings: BindingMap::default(),
        }
    }

    /// ### [9.1.1.1.1 HasBinding ( N )](https://tc39.es/ecma262/#sec-declarative-environment-records-hasbinding-n)
    pub(super) fn has_binding(&self, name: String) -> bool {
        // 1. If envRec has a binding for N, return true.
        // 2. Return false.
        self.bindings.contains_key(name)
    }
    /// ### [9.1.1.1.2 CreateMutableBinding ( N, D )](https://tc39.es/ecma262/#sec-declarative-environment-records-createmutablebinding-n-d)
    pub(super) fn create_mutable_binding(&mut self, name: String, is_deletable: bool) {
        // 1. Assert: envRec does not already have a binding for N.
        assert!(!self.has_binding(name));

        // 2. Create a mutable binding in envRec for N and record that it is
        // uninitialized. If D is true, record that the newly created binding
        // may be deleted by a subsequent DeleteBinding call.
        self.bindings.insert(
            name.unbind(),
            Binding {
                value: None,
                // Strictness only seems to matter for immutable bindings.
                strict: false,
                mutable: true,
                deletable: is_deletable,
            },
        );

        // 3. Return UNUSED.
    }
    /// ### [9.1.1.1.3 CreateImmutableBinding ( N, S )](https://tc39.es/ecma262/#sec-declarative-environment-records-createimmutablebinding-n-s)
    pub(super) fn create_immutable_binding(&mut self, name: String, is_strict: bool) {
        // 1. Assert: envRec does not already have a binding for N.
        debug_assert!(!self.has_binding(name));

        // 2. Create an immutable binding in envRec for N and record that it is
        // uninitialized. If S is true, record that the newly created binding is
        // a strict binding.
        self.bindings.insert(
            name.unbind(),
            Binding {
                value: None,
                strict: is_strict,
                mutable: false,
                deletable: false,
            },
        );

        // 3. Return UNUSED.
    }
    /// ### [9.1.1.1.4 InitializeBinding ( N, V )](https://tc39.es/ecma262/#sec-declarative-environment-records-initializebinding-n-v)
    pub(super) fn initialize_binding(&mut self, name: String, value: Value) {
        // 1. Assert: envRec must have an uninitialized binding for N.
        let binding = self.bindings.get_mut(name).unwrap();
        debug_assert!(binding.value.is_none());

        // 2. Set the bound value for N in envRec to V.
        // 3. Record that the binding for N in envRec has been initialized.
        // Note: Initialization status of N is determined by the Some/None.
        binding.value = Some(value.unbind());

        // 4. Return UNUSED.
    }

    /// ### [9.1.1.1.6 GetBindingValue ( N, S )](https://tc39.es/ecma262/#sec-declarative-environment-records-getbindingvalue-n-s)
    pub(super) fn get_binding_value(
        &self,
        name: String,
        _is_strict: bool,
    ) -> Option<Value<'static>> {
        // 1. Assert: envRec has a binding for N.
        let binding = self.bindings.get(name).unwrap();

        // 2. If the binding for N in envRec is an uninitialized binding, throw
        // a ReferenceError exception.
        let Some(value) = binding.value else {
            // Custom handling: Return None and let the caller handle throwing
            // an error.
            return None;
        };

        // 3. Return the value currently bound to N in envRec.
        Some(value)
    }

    pub(crate) fn get_binding(&self, name: String) -> Option<&Binding> {
        self.bindings.get(name)
    }

    fn get_binding_mut(&mut self, name: String) -> Option<&mut Binding> {
        self.bindings.get_mut(name)
    }

    /// ### [9.1.1.1.7 DeleteBinding ( N )](https://tc39.es/ecma262/#sec-declarative-environment-records-deletebinding-n)
    pub(super) fn delete_binding(&mut self, name: String) -> bool {
        // 1. Assert: envRec has a binding for N.
        let binding = self.bindings.get(name).unwrap();

        // 2. If the binding for N in envRec cannot be deleted, return false.
        if !binding.deletable {
            return false;
        }

        // 3. Remove the binding for N from envRec.
        self.bindings.remove(name);

        // 4. Return true.
        true
    }
}

impl HeapMarkAndSweep for DeclarativeEnvironmentRecord {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            outer_env,
            bindings,
        } = self;
        outer_env.mark_values(queues);
        bindings.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            outer_env,
            bindings,
        } = self;
        outer_env.sweep_values(compactions);
        bindings.sweep_values(compactions);
    }
}

impl HeapMarkAndSweep for Binding {
    fn mark_values(&self, queues: &mut WorkQueues) {
        let Self {
            value,
            strict: _,
            mutable: _,
            deletable: _,
        } = self;
        value.mark_values(queues);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        let Self {
            value,
            strict: _,
            mutable: _,
            deletable: _,
        } = self;
        value.sweep_values(compactions);
    }
}

impl<'e> DeclarativeEnvironment<'e> {
    pub(crate) fn get_outer_env(self, agent: &Agent) -> Option<Environment<'e>> {
        self.get(agent).outer_env
    }

    /// ### [9.1.1.1.1 HasBinding ( N )](https://tc39.es/ecma262/#sec-declarative-environment-records-hasbinding-n)
    ///
    /// The HasBinding concrete method of a Declarative Environment Record
    /// envRec takes argument N (a String) and returns a normal completion
    /// containing a Boolean. It determines if the argument identifier is one
    /// of the identifiers bound by the record.
    pub(crate) fn has_binding(self, agent: &impl AsRef<Environments>, name: String) -> bool {
        let env_rec = agent.as_ref().get_declarative_environment(self);
        // Delegate to heap data record method.
        env_rec.has_binding(name)
    }

    /// ### [9.1.1.1.2 CreateMutableBinding ( N, D )](https://tc39.es/ecma262/#sec-declarative-environment-records-createmutablebinding-n-d)
    ///
    /// The CreateMutableBinding concrete method of a Declarative Environment
    /// Record envRec takes arguments N (a String) and D (a Boolean) and
    /// returns a normal completion containing UNUSED. It creates a new mutable
    /// binding for the name N that is uninitialized. A binding must not
    /// already exist in this Environment Record for N. If D is true, the new
    /// binding is marked as being subject to deletion.
    pub(crate) fn create_mutable_binding(
        self,
        agent: &mut Agent,
        name: String,
        is_deletable: bool,
    ) {
        let env_rec = self.get_mut(agent);
        // Delegate to heap data record method.
        env_rec.create_mutable_binding(name, is_deletable);
    }

    /// ### [9.1.1.1.3 CreateImmutableBinding ( N, S )](https://tc39.es/ecma262/#sec-declarative-environment-records-createimmutablebinding-n-s)
    ///
    /// The CreateImmutableBinding concrete method of a Declarative Environment
    /// Record envRec takes arguments N (a String) and S (a Boolean) and
    /// returns a normal completion containing UNUSED. It creates a new
    /// immutable binding for the name N that is uninitialized. A binding must
    /// not already exist in this Environment Record for N. If S is true, the
    /// new binding is marked as a strict binding.
    pub(crate) fn create_immutable_binding(self, agent: &mut Agent, name: String, is_strict: bool) {
        let env_rec = self.get_mut(agent);
        // Delegate to heap data record method.
        env_rec.create_immutable_binding(name, is_strict);
    }

    /// ### [9.1.1.1.4 InitializeBinding ( N, V )](https://tc39.es/ecma262/#sec-declarative-environment-records-initializebinding-n-v)
    ///
    /// The InitializeBinding concrete method of a Declarative Environment
    /// Record envRec takes arguments N (a String) and V (an ECMAScript
    /// language value) and returns a normal completion containing UNUSED. It
    /// is used to set the bound value of the current binding of the identifier
    /// whose name is N to the value V. An uninitialized binding for N must
    /// already exist.
    pub(crate) fn initialize_binding(self, agent: &mut Agent, name: String, value: Value) {
        let env_rec = self.get_mut(agent);
        // Delegate to heap data record method.
        env_rec.initialize_binding(name, value)
    }

    /// ### [9.1.1.1.5 SetMutableBinding ( N, V, S )](https://tc39.es/ecma262/#sec-declarative-environment-records-setmutablebinding-n-v-s)
    ///
    /// The SetMutableBinding concrete method of a Declarative Environment
    /// Record envRec takes arguments N (a String), V (an ECMAScript language
    /// value), and S (a Boolean) and returns either a normal completion
    /// containing UNUSED or a throw completion. It attempts to change the
    /// bound value of the current binding of the identifier whose name is N to
    /// the value V. A binding for N normally already exists, but in rare cases
    /// it may not. If the binding is an immutable binding, a TypeError is
    /// thrown if S is true.
    pub(crate) fn set_mutable_binding<'a>(
        self,
        agent: &mut Agent,
        name: String,
        value: Value,
        mut is_strict: bool,
        gc: NoGcScope<'a, '_>,
    ) -> JsResult<'a, ()> {
        let env_rec = self.get_mut(agent);
        // 1. If envRec does not have a binding for N, then
        let Some(binding) = env_rec.bindings.get_mut(name) else {
            // a. If S is true, throw a ReferenceError exception.
            if is_strict {
                let error_message = format!(
                    "Cannot assign to nonexisting binding '{}'.",
                    name.to_string_lossy_(agent)
                );
                return Err(agent.throw_exception(
                    ExceptionType::ReferenceError,
                    error_message,
                    gc,
                ));
            }

            // b. Perform ! envRec.CreateMutableBinding(N, true).
            env_rec.create_mutable_binding(name, true);

            // c. Perform ! envRec.InitializeBinding(N, V).
            env_rec.initialize_binding(name, value);

            // d. Return UNUSED.
            return Ok(());
        };

        // 2. If the binding for N in envRec is a strict binding, set S to true.
        if binding.strict {
            is_strict = true;
        }

        // 3. If the binding for N in envRec has not yet been initialized, then
        if binding.value.is_none() {
            // a. Throw a ReferenceError exception.
            let error_message = format!(
                "Identifier '{}' has not been initialized.",
                name.to_string_lossy_(agent)
            );
            return Err(agent.throw_exception(ExceptionType::ReferenceError, error_message, gc));
        }

        // 4. Else if the binding for N in envRec is a mutable binding, then
        if binding.mutable {
            // a. Change its bound value to V.
            binding.value = Some(value.unbind());
        }
        // 5. Else,
        else {
            // a. Assert: This is an attempt to change the value of an immutable binding.
            debug_assert!(!binding.mutable);

            // b. If S is true, throw a TypeError exception.
            if is_strict {
                let error_message = format!(
                    "invalid assignment to const '{}'",
                    name.to_string_lossy_(agent)
                );
                return Err(agent.throw_exception(ExceptionType::TypeError, error_message, gc));
            }
        }

        // 6. Return UNUSED.
        Ok(())
    }

    /// ### [9.1.1.1.6 GetBindingValue ( N, S )](https://tc39.es/ecma262/#sec-declarative-environment-records-getbindingvalue-n-s)
    ///
    /// The GetBindingValue concrete method of a Declarative Environment Record
    /// envRec takes arguments N (a String) and S (a Boolean) and returns
    /// either a normal completion containing an ECMAScript language value or a
    /// throw completion. It returns the value of its bound identifier whose
    /// name is N. If the binding exists but is uninitialized a ReferenceError
    /// is thrown, regardless of the value of S.
    pub(crate) fn get_binding_value(
        self,
        agent: &mut Agent,
        name: String,
        is_strict: bool,
        gc: NoGcScope<'e, '_>,
    ) -> JsResult<'e, Value<'e>> {
        let env_rec = &self.get(agent);
        // Delegate to heap data record method.
        match env_rec.get_binding_value(name, is_strict) {
            Some(res) => Ok(res.bind(gc)),
            None => {
                // 2. If the binding for N in envRec is an uninitialized binding, throw
                // a ReferenceError exception.
                let error_message = format!(
                    "Could not get value of binding '{}': binding is uninitialized.",
                    name.to_string_lossy_(agent)
                );
                Err(agent.throw_exception(ExceptionType::ReferenceError, error_message, gc))
            }
        }
    }

    pub(crate) fn get_binding<'a>(
        self,
        agent: &'a impl AsRef<Environments>,
        name: String,
    ) -> Option<&'a Binding> {
        agent
            .as_ref()
            .get_declarative_environment(self)
            .get_binding(name)
    }

    pub(crate) fn get_binding_mut<'a>(
        self,
        agent: &'a mut impl AsMut<Environments>,
        name: String,
    ) -> Option<&'a mut Binding> {
        agent
            .as_mut()
            .get_declarative_environment_mut(self)
            .get_binding_mut(name)
    }

    /// ### [9.1.1.1.7 DeleteBinding ( N )](https://tc39.es/ecma262/#sec-declarative-environment-records-deletebinding-n)
    ///
    /// The DeleteBinding concrete method of a Declarative Environment Record
    /// envRec takes argument N (a String) and returns a normal completion
    /// containing a Boolean. It can only delete bindings that have been
    /// explicitly designated as being subject to deletion.
    pub(crate) fn delete_binding(self, agent: &mut Agent, name: String) -> bool {
        let env_rec = self.get_mut(agent);
        // Delegate to heap data record method.
        env_rec.delete_binding(name)
    }
}

impl HeapMarkAndSweep for DeclarativeEnvironment<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.declarative_environments.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions
            .declarative_environments
            .shift_index(&mut self.0);
    }
}

/// ### [9.1.2.2 NewDeclarativeEnvironment ( E )](https://tc39.es/ecma262/#sec-newdeclarativeenvironment)
///
/// The abstract operation NewDeclarativeEnvironment takes argument E (an
/// Environment Record or null) and returns a Declarative Environment
/// Record.
pub(crate) fn new_declarative_environment<'a>(
    agent: &mut Agent,
    outer_env: OuterEnv,
    gc: NoGcScope<'a, '_>,
) -> DeclarativeEnvironment<'a> {
    // 1. Let env be a new Declarative Environment Record containing no bindings.
    // 2. Set env.[[OuterEnv]] to E.
    agent.heap.alloc_counter += DECLARATIVE_ENVIRONMENT_RECORD_ALLOC_SIZE;
    // 3. Return env.
    agent
        .heap
        .environments
        .push_declarative_environment(DeclarativeEnvironmentRecord::new(outer_env), gc)
}
