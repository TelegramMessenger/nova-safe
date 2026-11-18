// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A script compiled once and evaluated in many agents.
//!
//! [`CompiledScript::export`] turns a parsed, not yet evaluated [`Script`]
//! into data that belongs to no agent: the bytecode (shared), and every
//! constant, property key, object shape, function template and nested
//! executable written out as owned values. [`CompiledScript::instantiate`]
//! makes them again in another agent, the way compiling the same source
//! text there makes them, and returns a Script for [`script_evaluation`].
//!
//! Heap strings, property lookup caches and object shapes are kept in tables
//! of their own, each entry once however many executables use it, so that an
//! agent makes (and hashes) each of them once.
//!
//! Only constants whose making is known are exported (primitives, the
//! RegExp and template objects of literals, and the two intrinsics the
//! compiler refers to); anything else makes the export fail, and the caller
//! parses the source as before. The data is immutable, `Send` and `Sync`,
//! and never serialized: it cannot be built from bytes.
//!
//! [`script_evaluation`]: crate::ecmascript::script_evaluation

use std::{collections::BTreeMap, sync::Arc};

#[cfg(feature = "regexp")]
use oxc_ast::ast::RegExpFlags;
use oxc_span::Span;
use wtf8::Wtf8Buf;

#[cfg(feature = "regexp")]
use crate::ecmascript::{RegExp, reg_exp_create_literal};
use crate::{
    ecmascript::{
        Agent, Array, BUILTIN_STRING_MEMORY, BigInt, BigIntHeapData, FunctionKind,
        FunctionTemplate, HeapString, InternalMethods, InternalSlots, Number, ObjectShape,
        PropertyKey, PropertyLookupCache, Realm, SmallBigInt, SmallF64, SmallInteger, SourceCode,
        String, Value, unwrap_try,
    },
    engine::{
        ArrowFunctionExpression, Bindable, Executable, ExecutableHeapData, FunctionExpression,
        NamedEvaluationParameter, NoGcScope, create_template_object,
    },
    hashing::AHashMap,
    heap::{ArenaAccess, CreateHeapData, HeapIndexHandle},
};
use small_string::SmallString;

use super::{GlobalDeclarations, HostDefined, Script, ScriptRecord};
use crate::engine::{CallEnvironment, CodeTables, SwitchTable};

/// Why a script could not be exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompiledScriptError {
    /// The script was evaluated: its code is gone.
    AlreadyEvaluated,
    /// The script's code holds something this module cannot remake.
    Unsupported(&'static str),
}

impl core::fmt::Display for CompiledScriptError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AlreadyEvaluated => f.write_str("the script was already evaluated"),
            Self::Unsupported(what) => write!(f, "the compiled script holds {what}"),
        }
    }
}

type ExportResult<T> = Result<T, CompiledScriptError>;

/// A compiled script that belongs to no agent; see the module docs.
#[derive(PartialEq)]
pub struct CompiledScript {
    source_text: Box<str>,
    is_strict: bool,
    /// The distinct heap strings.
    strings: Box<[HeapText]>,
    /// The key of each distinct property lookup cache.
    caches: Box<[CompiledKey]>,
    /// The distinct object literal shapes.
    shapes: Box<[CompiledShape]>,
    /// The RegExp, template and intrinsic objects of the constants, each
    /// once however many constants refer to it.
    objects: Box<[CompiledObject]>,
    /// Every executable, each after the executables it refers to.
    executables: Box<[CompiledExecutable]>,
    declarations: CompiledDeclarations,
}

const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<CompiledScript>();
};

/// The text of a heap string; UTF-8 text is kept as such so that making the
/// string does not check it again.
#[derive(PartialEq)]
enum HeapText {
    Utf8(Box<str>),
    Wtf8(Wtf8Buf),
}

/// A string: small, or an index into [`CompiledScript::strings`].
#[derive(Clone, Copy, PartialEq)]
enum CompiledString {
    Small(SmallString),
    Heap(u32),
}

#[derive(Clone, Copy, PartialEq)]
enum CompiledKey {
    Integer(SmallInteger),
    String(CompiledString),
}

#[derive(Clone, PartialEq)]
enum CompiledValue {
    Undefined,
    Null,
    Boolean(bool),
    String(CompiledString),
    Integer(SmallInteger),
    SmallF64(SmallF64),
    Number(f64),
    SmallBigInt(SmallBigInt),
    BigInt(num_bigint::BigInt),
    Object(u32),
}

#[derive(PartialEq)]
enum CompiledObject {
    #[cfg(feature = "regexp")]
    RegExp {
        source: CompiledString,
        flags: RegExpFlags,
    },
    Template {
        cooked: Box<[CompiledValue]>,
        raw: Box<[CompiledValue]>,
    },
    ObjectPrototype,
    FunctionPrototype,
}

#[derive(PartialEq)]
struct CompiledShape {
    null_prototype: bool,
    keys: Box<[CompiledKey]>,
    extensible: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct CompiledTemplate {
    kind: FunctionKind,
    span: Span,
    has_use_strict_directive: bool,
    expected_argument_count: u32,
    executable: Option<u32>,
}

#[derive(Clone, Copy, PartialEq)]
enum CompiledNamedEvaluation {
    Result,
    Stack,
}

#[derive(PartialEq)]
struct CompiledFunctionExpression {
    template: CompiledTemplate,
    identifier: Option<CompiledNamedEvaluation>,
    binding_name: Option<CompiledString>,
    is_expression: bool,
    is_declaration: bool,
}

#[derive(PartialEq)]
struct CompiledArrowFunctionExpression {
    template: CompiledTemplate,
    identifier: Option<CompiledNamedEvaluation>,
}

#[derive(PartialEq)]
struct CompiledExecutable {
    instructions: Arc<[u8]>,
    /// Indices into [`CompiledScript::caches`].
    caches: Box<[u32]>,
    constants: Box<[CompiledValue]>,
    wide_constants: Box<[CompiledValue]>,
    /// Indices into [`CompiledScript::shapes`].
    shapes: Box<[u32]>,
    function_expressions: Box<[CompiledFunctionExpression]>,
    arrow_function_expressions: Box<[CompiledArrowFunctionExpression]>,
    class_initializer_bytecodes: Box<[(Option<u32>, bool)]>,
    instance_initializer: Option<u32>,
    call_environment: CallEnvironment,
    stack_parameters: Option<u16>,
    /// How many binding sites the instructions have; what each remembers
    /// belongs to the Agent that runs them.
    binding_sites: usize,
    /// The switch tables refer to constants by index and to instructions:
    /// they are the same in every Agent.
    switch_tables: Arc<[SwitchTable]>,
}

#[derive(PartialEq)]
struct CompiledDeclarations {
    lex_names: Box<[Box<str>]>,
    var_names: Box<[Box<str>]>,
    functions: Box<[(Box<str>, CompiledTemplate)]>,
    var_declaration_names: Box<[Box<str>]>,
    lex_declarations: Box<[(Option<Box<str>>, bool)]>,
}

fn table_index(len: usize, what: &'static str) -> ExportResult<u32> {
    u32::try_from(len).map_err(|_| CompiledScriptError::Unsupported(what))
}

impl CompiledScript {
    /// Exports a script that [`parse_script`] made and that was not
    /// evaluated yet.
    ///
    /// [`parse_script`]: crate::ecmascript::parse_script
    pub fn export(agent: &mut Agent, script: Script, gc: NoGcScope) -> ExportResult<Self> {
        let record = script.get(agent);
        let Some(executable) = record.executable else {
            return Err(CompiledScriptError::AlreadyEvaluated);
        };
        let executable = executable.unbind();
        let realm = record.realm.unbind();
        let is_strict = record.is_strict;
        let source_text: Box<str> = record.source_code.get_source_text(agent).into();
        let GlobalDeclarations {
            lex_names,
            var_names,
            functions,
            var_declaration_names,
            lex_declarations,
        } = &record.declarations;
        let lex_names = lex_names.clone();
        let var_names = var_names.clone();
        let functions = functions.clone();
        let var_declaration_names = var_declaration_names.clone();
        let lex_declarations = lex_declarations.clone();

        let intrinsics = agent.get_realm_record_by_id(realm).intrinsics();
        let object_prototype: Value = intrinsics.object_prototype().into();
        let function_prototype: Value = intrinsics.function_prototype().into();
        let cache_keys = agent
            .heap
            .caches
            .property_lookup_cache_keys()
            .map(|(cache, key)| (cache.unbind(), key.unbind()))
            .collect();
        let mut exporter = Exporter {
            agent,
            object_prototype: object_prototype.unbind(),
            function_prototype: function_prototype.unbind(),
            cache_keys,
            strings: Vec::new(),
            string_ids: AHashMap::default(),
            caches: Vec::new(),
            cache_ids: AHashMap::default(),
            shapes: Vec::new(),
            shape_ids: BTreeMap::new(),
            executables: Vec::new(),
            executable_ids: AHashMap::default(),
            in_progress: Vec::new(),
            objects: Vec::new(),
            object_ids: Vec::new(),
        };
        let mut compiled_functions = Vec::with_capacity(functions.len());
        for (name, template) in functions.iter() {
            compiled_functions.push((name.clone(), exporter.template(*template, gc)?));
        }
        let root = exporter.executable(executable, gc)?;
        if root as usize + 1 != exporter.executables.len() {
            return Err(CompiledScriptError::Unsupported(
                "a script executable referred to from its own functions",
            ));
        }
        Ok(Self {
            source_text,
            is_strict,
            strings: exporter.strings.into_boxed_slice(),
            caches: exporter.caches.into_boxed_slice(),
            shapes: exporter.shapes.into_boxed_slice(),
            objects: exporter.objects.into_boxed_slice(),
            executables: exporter.executables.into_boxed_slice(),
            declarations: CompiledDeclarations {
                lex_names,
                var_names,
                functions: compiled_functions.into_boxed_slice(),
                var_declaration_names,
                lex_declarations,
            },
        })
    }

    /// Makes the script in `agent`, as [`parse_script`] of the same source
    /// text (not forced strict) with this realm and host data would.
    ///
    /// [`parse_script`]: crate::ecmascript::parse_script
    pub fn instantiate<'a>(
        &self,
        agent: &mut Agent,
        realm: Realm,
        host_defined: Option<HostDefined>,
        gc: NoGcScope<'a, '_>,
    ) -> Script<'a> {
        let realm = realm.bind(gc);
        let source_code = SourceCode::from_source_text(agent, &self.source_text, gc);
        let strings = import_strings(agent, &self.strings, gc);
        let mut importer = Importer {
            agent,
            realm,
            gc,
            strings,
            caches: Vec::with_capacity(self.caches.len()),
            shapes: Vec::with_capacity(self.shapes.len()),
            objects: Vec::with_capacity(self.objects.len()),
            executables: Vec::with_capacity(self.executables.len()),
        };
        for key in self.caches.iter() {
            let key = importer.key(*key);
            let cache = PropertyLookupCache::new(importer.agent, key);
            importer.caches.push(cache);
        }
        for shape in self.shapes.iter() {
            let shape = importer.shape(shape);
            importer.shapes.push(shape);
        }
        for object in self.objects.iter() {
            let object = importer.object(object);
            importer.objects.push(object);
        }
        for executable in self.executables.iter() {
            let executable = importer.executable(executable);
            importer.executables.push(executable);
        }
        let CompiledDeclarations {
            lex_names,
            var_names,
            functions,
            var_declaration_names,
            lex_declarations,
        } = &self.declarations;
        let declarations = GlobalDeclarations {
            lex_names: lex_names.clone(),
            var_names: var_names.clone(),
            functions: functions
                .iter()
                .map(|(name, template)| (name.clone(), importer.template(template).unbind()))
                .collect(),
            var_declaration_names: var_declaration_names.clone(),
            lex_declarations: lex_declarations.clone(),
        };
        let root = *importer
            .executables
            .last()
            .expect("an exported script has its own executable");
        let script_record = ScriptRecord {
            realm: realm.unbind(),
            executable: Some(root.unbind()),
            declarations,
            is_strict: self.is_strict,
            loaded_modules: Default::default(),
            host_defined,
            source_code: source_code.unbind(),
        };
        agent.heap.create(script_record).bind(gc)
    }
}

struct Exporter<'agent> {
    agent: &'agent mut Agent,
    object_prototype: Value<'static>,
    function_prototype: Value<'static>,
    cache_keys: AHashMap<PropertyLookupCache<'static>, PropertyKey<'static>>,
    strings: Vec<HeapText>,
    string_ids: AHashMap<HeapString<'static>, u32>,
    caches: Vec<CompiledKey>,
    cache_ids: AHashMap<PropertyLookupCache<'static>, u32>,
    shapes: Vec<CompiledShape>,
    shape_ids: BTreeMap<ObjectShape<'static>, u32>,
    executables: Vec<CompiledExecutable>,
    executable_ids: AHashMap<usize, u32>,
    in_progress: Vec<usize>,
    objects: Vec<CompiledObject>,
    object_ids: Vec<(Value<'static>, u32)>,
}

impl Exporter<'_> {
    fn executable(&mut self, executable: Executable, gc: NoGcScope) -> ExportResult<u32> {
        let index = executable.get_index();
        if let Some(&id) = self.executable_ids.get(&index) {
            return Ok(id);
        }
        if self.in_progress.contains(&index) {
            return Err(CompiledScriptError::Unsupported(
                "an executable that refers to itself",
            ));
        }
        self.in_progress.push(index);
        let ExecutableHeapData {
            instructions,
            tables,
            wide_constants,
            shapes,
            function_expressions,
            arrow_function_expressions,
            class_initializer_bytecodes,
            instance_initializer,
            call_environment,
            stack_parameters,
            switch_tables,
            _gc: _,
        } = executable.get(self.agent).clone();
        let caches: Vec<_> = tables.caches().collect();
        let constants: Vec<_> = tables.constants().collect();
        let binding_sites = tables.binding_sites();

        let mut compiled_function_expressions = Vec::with_capacity(function_expressions.len());
        for function in function_expressions.iter() {
            let FunctionExpression {
                template,
                identifier,
                binding_name,
                is_expression,
                is_declaration,
            } = function;
            compiled_function_expressions.push(CompiledFunctionExpression {
                template: self.template(*template, gc)?,
                identifier: identifier.map(named_evaluation),
                binding_name: match binding_name {
                    Some(name) => Some(self.string(*name)?),
                    None => None,
                },
                is_expression: *is_expression,
                is_declaration: *is_declaration,
            });
        }
        let mut compiled_arrows = Vec::with_capacity(arrow_function_expressions.len());
        for arrow in arrow_function_expressions.iter() {
            let ArrowFunctionExpression {
                template,
                identifier,
            } = arrow;
            compiled_arrows.push(CompiledArrowFunctionExpression {
                template: self.template(*template, gc)?,
                identifier: identifier.map(named_evaluation),
            });
        }
        let mut compiled_class_initializers = Vec::with_capacity(class_initializer_bytecodes.len());
        for (initializer, has_constructor_parent) in class_initializer_bytecodes.iter() {
            let initializer = match initializer {
                Some(initializer) => Some(self.executable(*initializer, gc)?),
                None => None,
            };
            compiled_class_initializers.push((initializer, *has_constructor_parent));
        }
        let instance_initializer = match instance_initializer {
            Some(initializer) => Some(self.executable(initializer, gc)?),
            None => None,
        };
        let mut compiled_caches = Vec::with_capacity(caches.len());
        for cache in caches.iter() {
            compiled_caches.push(self.cache(*cache)?);
        }
        let mut compiled_constants = Vec::with_capacity(constants.len());
        for constant in constants.iter() {
            compiled_constants.push(self.value(*constant, gc)?);
        }
        let mut compiled_wide_constants = Vec::with_capacity(wide_constants.len());
        for constant in wide_constants.iter() {
            compiled_wide_constants.push(self.value(*constant, gc)?);
        }
        let mut compiled_shapes = Vec::with_capacity(shapes.len());
        for shape in shapes.iter() {
            compiled_shapes.push(self.shape(*shape)?);
        }

        let id = table_index(self.executables.len(), "too many executables")?;
        self.executables.push(CompiledExecutable {
            instructions,
            caches: compiled_caches.into_boxed_slice(),
            constants: compiled_constants.into_boxed_slice(),
            wide_constants: compiled_wide_constants.into_boxed_slice(),
            shapes: compiled_shapes.into_boxed_slice(),
            function_expressions: compiled_function_expressions.into_boxed_slice(),
            arrow_function_expressions: compiled_arrows.into_boxed_slice(),
            class_initializer_bytecodes: compiled_class_initializers.into_boxed_slice(),
            instance_initializer,
            call_environment,
            stack_parameters,
            binding_sites,
            switch_tables,
        });
        self.executable_ids.insert(index, id);
        self.in_progress.pop();
        Ok(id)
    }

    fn template(
        &mut self,
        template: FunctionTemplate,
        gc: NoGcScope,
    ) -> ExportResult<CompiledTemplate> {
        let FunctionTemplate {
            kind,
            span,
            has_use_strict_directive,
            expected_argument_count,
            executable,
        } = template;
        let executable = match executable {
            Some(executable) => Some(self.executable(executable, gc)?),
            None => None,
        };
        Ok(CompiledTemplate {
            kind,
            span,
            has_use_strict_directive,
            expected_argument_count,
            executable,
        })
    }

    fn string(&mut self, string: String) -> ExportResult<CompiledString> {
        let heap = match string {
            String::SmallString(string) => return Ok(CompiledString::Small(string)),
            String::String(heap) => heap.unbind(),
        };
        if let Some(&id) = self.string_ids.get(&heap) {
            return Ok(CompiledString::Heap(id));
        }
        let string = String::String(heap);
        let text = string.as_wtf8_(self.agent);
        if String::try_from(text).is_ok() {
            return Err(CompiledScriptError::Unsupported(
                "a heap string short enough to be a small string",
            ));
        }
        let text = match text.as_str() {
            Some(text) => HeapText::Utf8(text.into()),
            None => {
                let mut owned = Wtf8Buf::with_capacity(text.len());
                owned.push_wtf8(text);
                HeapText::Wtf8(owned)
            }
        };
        let id = table_index(self.strings.len(), "too many strings")?;
        self.strings.push(text);
        self.string_ids.insert(heap, id);
        Ok(CompiledString::Heap(id))
    }

    fn key(&mut self, key: PropertyKey) -> ExportResult<CompiledKey> {
        match key {
            PropertyKey::Integer(integer) => Ok(CompiledKey::Integer(integer)),
            PropertyKey::SmallString(string) => {
                Ok(CompiledKey::String(CompiledString::Small(string)))
            }
            PropertyKey::String(string) => Ok(CompiledKey::String(self.string(string.into())?)),
            PropertyKey::Symbol(_) | PropertyKey::PrivateName(_) => Err(
                CompiledScriptError::Unsupported("a symbol or private name property key"),
            ),
        }
    }

    fn cache(&mut self, cache: PropertyLookupCache) -> ExportResult<u32> {
        let cache = cache.unbind();
        if let Some(&id) = self.cache_ids.get(&cache) {
            return Ok(id);
        }
        let Some(key) = self.cache_keys.get(&cache).copied() else {
            return Err(CompiledScriptError::Unsupported(
                "a property lookup cache without a key",
            ));
        };
        let key = self.key(key)?;
        let id = table_index(self.caches.len(), "too many property lookup caches")?;
        self.caches.push(key);
        self.cache_ids.insert(cache, id);
        Ok(id)
    }

    fn shape(&mut self, shape: ObjectShape) -> ExportResult<u32> {
        let shape = shape.unbind();
        if let Some(&id) = self.shape_ids.get(&shape) {
            return Ok(id);
        }
        let null_prototype = match shape.get_prototype(&self.agent.heap.object_shapes) {
            None => true,
            Some(prototype) if Value::from(prototype) == self.object_prototype => false,
            Some(_) => {
                return Err(CompiledScriptError::Unsupported(
                    "an object literal shape with another prototype",
                ));
            }
        };
        let keys = shape
            .keys(&self.agent.heap.object_shapes, &self.agent.heap.elements)
            .to_vec();
        let mut compiled_keys = Vec::with_capacity(keys.len());
        for key in keys {
            compiled_keys.push(self.key(key)?);
        }
        let id = table_index(self.shapes.len(), "too many object shapes")?;
        self.shapes.push(CompiledShape {
            null_prototype,
            keys: compiled_keys.into_boxed_slice(),
            extensible: shape.extensible(),
        });
        self.shape_ids.insert(shape, id);
        Ok(id)
    }

    fn value(&mut self, value: Value, gc: NoGcScope) -> ExportResult<CompiledValue> {
        Ok(match value {
            Value::Undefined => CompiledValue::Undefined,
            Value::Null => CompiledValue::Null,
            Value::Boolean(value) => CompiledValue::Boolean(value),
            Value::SmallString(string) => CompiledValue::String(CompiledString::Small(string)),
            Value::String(string) => CompiledValue::String(self.string(string.into())?),
            Value::Integer(integer) => CompiledValue::Integer(integer),
            Value::SmallF64(number) => CompiledValue::SmallF64(number),
            Value::Number(number) => {
                CompiledValue::Number(Number::Number(number).into_f64(self.agent))
            }
            Value::SmallBigInt(bigint) => CompiledValue::SmallBigInt(bigint),
            Value::BigInt(bigint) => CompiledValue::BigInt(bigint.get(self.agent).data.clone()),
            _ => CompiledValue::Object(self.object(value, gc)?),
        })
    }

    fn object(&mut self, value: Value, gc: NoGcScope) -> ExportResult<u32> {
        if let Some((_, id)) = self.object_ids.iter().find(|(known, _)| *known == value) {
            return Ok(*id);
        }
        let object = if value == self.object_prototype {
            CompiledObject::ObjectPrototype
        } else if value == self.function_prototype {
            CompiledObject::FunctionPrototype
        } else {
            match value {
                #[cfg(feature = "regexp")]
                Value::RegExp(regexp) => self.regexp(regexp)?,
                Value::Array(array) => self.template_object(array, gc)?,
                _ => {
                    return Err(CompiledScriptError::Unsupported(
                        "an object constant that is not a RegExp or template literal's",
                    ));
                }
            }
        };
        let id = table_index(self.objects.len(), "too many object constants")?;
        self.objects.push(object);
        self.object_ids.push((value.unbind(), id));
        Ok(id)
    }

    #[cfg(feature = "regexp")]
    fn regexp(&mut self, regexp: RegExp) -> ExportResult<CompiledObject> {
        if regexp.get_backing_object(self.agent).is_some()
            || regexp.try_get_last_index(self.agent) != Some(0)
        {
            return Err(CompiledScriptError::Unsupported(
                "a RegExp literal that was used",
            ));
        }
        let source = regexp.original_source(self.agent);
        let flags = regexp.original_flags(self.agent);
        Ok(CompiledObject::RegExp {
            source: self.string(source)?,
            flags,
        })
    }

    fn template_object(&mut self, template: Array, gc: NoGcScope) -> ExportResult<CompiledObject> {
        const NOT_A_TEMPLATE: CompiledScriptError =
            CompiledScriptError::Unsupported("an array constant that is not a template object");
        let raw = unwrap_try(template.try_get_own_property(
            self.agent,
            BUILTIN_STRING_MEMORY.raw.to_property_key(),
            None,
            gc,
        ));
        let Some(Value::Array(raw)) = raw.and_then(|descriptor| descriptor.value) else {
            return Err(NOT_A_TEMPLATE);
        };
        let cooked: Vec<Option<Value>> = template.as_slice(self.agent).to_vec();
        let raw: Vec<Option<Value>> = raw.as_slice(self.agent).to_vec();
        if cooked.len() != raw.len() {
            return Err(NOT_A_TEMPLATE);
        }
        let mut compiled_cooked = Vec::with_capacity(cooked.len());
        for value in cooked {
            compiled_cooked.push(match value {
                Some(Value::Undefined) => CompiledValue::Undefined,
                Some(value @ (Value::String(_) | Value::SmallString(_))) => {
                    self.value(value, gc)?
                }
                _ => return Err(NOT_A_TEMPLATE),
            });
        }
        let mut compiled_raw = Vec::with_capacity(raw.len());
        for value in raw {
            compiled_raw.push(match value {
                Some(value @ (Value::String(_) | Value::SmallString(_))) => {
                    self.value(value, gc)?
                }
                _ => return Err(NOT_A_TEMPLATE),
            });
        }
        Ok(CompiledObject::Template {
            cooked: compiled_cooked.into_boxed_slice(),
            raw: compiled_raw.into_boxed_slice(),
        })
    }
}

fn named_evaluation(parameter: NamedEvaluationParameter) -> CompiledNamedEvaluation {
    match parameter {
        NamedEvaluationParameter::Result => CompiledNamedEvaluation::Result,
        NamedEvaluationParameter::Stack => CompiledNamedEvaluation::Stack,
    }
}

fn import_named_evaluation(parameter: CompiledNamedEvaluation) -> NamedEvaluationParameter {
    match parameter {
        CompiledNamedEvaluation::Result => NamedEvaluationParameter::Result,
        CompiledNamedEvaluation::Stack => NamedEvaluationParameter::Stack,
    }
}

/// Makes the distinct heap strings, each with one hash: room for all of them
/// in the string table is made first, so that adding them never rehashes the
/// strings already there.
fn import_strings<'a>(
    agent: &mut Agent,
    texts: &[HeapText],
    gc: NoGcScope<'a, '_>,
) -> Vec<String<'a>> {
    {
        let heap = &mut agent.heap;
        heap.strings.reserve(texts.len());
        let strings = &heap.strings;
        let hasher = &heap.string_hasher;
        heap.string_lookup_table.reserve(texts.len(), |string| {
            strings[string.get_index()].table_hash(hasher)
        });
    }
    texts
        .iter()
        .map(|text| match text {
            HeapText::Utf8(text) => String::from_str(agent, text, gc),
            HeapText::Wtf8(text) => String::from_wtf8_buf(agent, text.clone(), gc),
        })
        .collect()
}

struct Importer<'agent, 'a, 'scope> {
    agent: &'agent mut Agent,
    realm: Realm<'a>,
    gc: NoGcScope<'a, 'scope>,
    strings: Vec<String<'a>>,
    caches: Vec<PropertyLookupCache<'a>>,
    shapes: Vec<ObjectShape<'a>>,
    objects: Vec<Value<'a>>,
    executables: Vec<Executable<'a>>,
}

impl<'a> Importer<'_, 'a, '_> {
    fn string(&self, string: CompiledString) -> String<'a> {
        match string {
            CompiledString::Small(string) => String::SmallString(string),
            CompiledString::Heap(id) => self.strings[id as usize],
        }
    }

    fn key(&self, key: CompiledKey) -> PropertyKey<'a> {
        match key {
            CompiledKey::Integer(integer) => PropertyKey::Integer(integer),
            CompiledKey::String(string) => match self.string(string) {
                String::SmallString(string) => PropertyKey::SmallString(string),
                String::String(string) => PropertyKey::String(string),
            },
        }
    }

    fn value(&mut self, value: &CompiledValue) -> Value<'a> {
        match value {
            CompiledValue::Undefined => Value::Undefined,
            CompiledValue::Null => Value::Null,
            CompiledValue::Boolean(value) => Value::Boolean(*value),
            CompiledValue::String(string) => self.string(*string).into(),
            CompiledValue::Integer(integer) => Value::Integer(*integer),
            CompiledValue::SmallF64(number) => Value::SmallF64(*number),
            CompiledValue::Number(number) => Number::from_f64(self.agent, *number, self.gc).into(),
            CompiledValue::SmallBigInt(bigint) => Value::SmallBigInt(*bigint),
            CompiledValue::BigInt(bigint) => {
                let bigint: BigInt = self.agent.heap.create(BigIntHeapData {
                    data: bigint.clone(),
                });
                bigint.bind(self.gc).into()
            }
            CompiledValue::Object(id) => self.objects[*id as usize],
        }
    }

    fn shape(&mut self, shape: &CompiledShape) -> ObjectShape<'a> {
        let prototype = if shape.null_prototype {
            None
        } else {
            Some(
                self.agent
                    .get_realm_record_by_id(self.realm)
                    .intrinsics()
                    .object_prototype()
                    .into(),
            )
        };
        let mut object_shape = ObjectShape::get_shape_for_prototype(self.agent, prototype);
        for key in shape.keys.iter() {
            let key = self.key(*key);
            object_shape = object_shape
                .get_child_shape(self.agent, key)
                .expect("Should perform GC here");
        }
        object_shape.set_extensible(shape.extensible);
        object_shape
    }

    fn object(&mut self, object: &CompiledObject) -> Value<'a> {
        match object {
            #[cfg(feature = "regexp")]
            CompiledObject::RegExp { source, flags } => {
                let source = self.string(*source);
                reg_exp_create_literal(self.agent, source, Some(*flags), self.gc).into()
            }
            CompiledObject::Template { cooked, raw } => {
                let cooked = cooked
                    .iter()
                    .map(|value| self.value(value).unbind())
                    .collect();
                let raw = raw.iter().map(|value| self.value(value).unbind()).collect();
                create_template_object(self.agent, cooked, raw, self.gc).into()
            }
            CompiledObject::ObjectPrototype => self
                .agent
                .get_realm_record_by_id(self.realm)
                .intrinsics()
                .object_prototype()
                .bind(self.gc)
                .into(),
            CompiledObject::FunctionPrototype => self
                .agent
                .get_realm_record_by_id(self.realm)
                .intrinsics()
                .function_prototype()
                .bind(self.gc)
                .into(),
        }
    }

    fn template(&self, template: &CompiledTemplate) -> FunctionTemplate<'a> {
        FunctionTemplate {
            kind: template.kind,
            span: template.span,
            has_use_strict_directive: template.has_use_strict_directive,
            expected_argument_count: template.expected_argument_count,
            executable: template.executable.map(|id| self.executables[id as usize]),
        }
    }

    fn executable(&mut self, compiled: &CompiledExecutable) -> Executable<'a> {
        let caches: Vec<_> = compiled
            .caches
            .iter()
            .map(|id| self.caches[*id as usize].unbind())
            .collect();
        let constants: Vec<_> = compiled
            .constants
            .iter()
            .map(|value| self.value(value).unbind())
            .collect();
        let wide_constants = compiled
            .wide_constants
            .iter()
            .map(|value| self.value(value).unbind())
            .collect();
        let shapes = compiled
            .shapes
            .iter()
            .map(|id| self.shapes[*id as usize].unbind())
            .collect();
        let function_expressions = compiled
            .function_expressions
            .iter()
            .map(|function| FunctionExpression {
                template: self.template(&function.template).unbind(),
                identifier: function.identifier.map(import_named_evaluation),
                binding_name: function.binding_name.map(|name| self.string(name).unbind()),
                is_expression: function.is_expression,
                is_declaration: function.is_declaration,
            })
            .collect();
        let arrow_function_expressions = compiled
            .arrow_function_expressions
            .iter()
            .map(|arrow| ArrowFunctionExpression {
                template: self.template(&arrow.template).unbind(),
                identifier: arrow.identifier.map(import_named_evaluation),
            })
            .collect();
        let class_initializer_bytecodes = compiled
            .class_initializer_bytecodes
            .iter()
            .map(|(initializer, has_constructor_parent)| {
                (
                    initializer.map(|id| self.executables[id as usize].unbind()),
                    *has_constructor_parent,
                )
            })
            .collect();
        let instance_initializer = compiled
            .instance_initializer
            .map(|id| self.executables[id as usize].unbind());
        self.agent.heap.create(ExecutableHeapData {
            instructions: compiled.instructions.clone(),
            tables: CodeTables::new(
                compiled.instructions.clone(),
                constants,
                caches,
                compiled.binding_sites,
            ),
            wide_constants,
            shapes,
            function_expressions,
            arrow_function_expressions,
            class_initializer_bytecodes,
            instance_initializer,
            call_environment: compiled.call_environment,
            stack_parameters: compiled.stack_parameters,
            switch_tables: compiled.switch_tables.clone(),
            _gc: core::marker::PhantomData,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CompiledScript;
    use crate::{
        ecmascript::{
            Agent, AgentOptions, DefaultHostHooks, String, initialize_default_realm, parse_script,
            script_evaluation, to_string,
        },
        engine::{Bindable, GcScope},
    };

    const TRICKY: &str = r#"
var out = [];
function f(a, b = 1, ...c) { return a + b; }
out.push(f.toString(), f.name, f.length);
function dup() { return 1 } function dup() { return 2 }
out.push(dup());
const g = function named() {}; out.push(g.name, g.toString());
const anon = function () {}; out.push(anon.name);
const arrow = (x) => x * 2; out.push(arrow.toString(), arrow.length, arrow.name);
class A {
  #p = 1; static s = 'st'; x = 5;
  get p() { return this.#p; } #m() { return 2 } m() { return this.#m(); }
  static { out.push('static block ' + this.name) }
}
class B extends A { constructor() { super(); this.y = 3; } z = 7; }
const b = new B(); out.push(b.p, b.m(), b.x, b.y, b.z, A.s, B.toString(), A.toString().length);
class D extends A { w = 9 } out.push(new D().w, new D().x);
class N extends null {} out.push(typeof N, Object.getPrototypeOf(N) === Function.prototype);
function tag(s, ...v) { return s; }
function site() { return tag`a${1}b\u{41}${2}\x`; }
out.push(site() === site(), site().raw.join('|'), String(site()[2]), Object.isFrozen(site()), site().length);
const other = tag`a${1}b\u{41}${2}\x`; out.push(other === site());
const re = /ab+c/gi; out.push(re.source, re.flags, re.test('xABBC'), re.lastIndex);
function reFn() { return /x(?<n>y)\u{1F600}/gu; } out.push(reFn() === reFn(), reFn().source);
out.push(123n, 2n ** 64n, 12345678901234567890n, -98765432109876543210n, 1.5, 1e300, -0, 0.1 + 0.2, 4294967296.5);
out.push("long string literal over seven bytes", "abc\ud800defghij".length, `tpl ${1 + 1} more text`);
const o = { a: 1, b: 2, __proto__: null }; out.push(Object.getPrototypeOf(o), Object.keys(o).join());
const o2 = { x: 1, y: { z: 2 }, 0: 'zero', "long key name": 3 }; out.push(JSON.stringify(o2));
function* gen() { yield 1; yield* [2, 3]; } out.push([...gen()].join());
async function af() { await 1; return 2 } out.push(typeof af().then);
out.push((function () { return typeof this })(), (function () { 'use strict'; return typeof this })());
label: for (let i = 0; i < 3; i++) { for (const k in { a: 1 }) { if (i) continue label; out.push(k) } }
let { aa = 1, ...rest } = { bb: 2 }; out.push(aa, rest.bb);
try { null.x } catch (e) { out.push(e.constructor.name) }
var declared = 1; let lexical = 2;
out.push(typeof declared, typeof globalThis.declared, typeof globalThis.lexical);
const objs = [1, 2].map((v) => ({ v, d: v * 2 })); out.push(JSON.stringify(objs));
out.push(String(Symbol.iterator in []), [..."ab"].join("-"));
out.map(String).join('\n')
"#;

    fn run(source: &str, compiled: Option<&CompiledScript>) -> std::string::String {
        let (mut gc, mut scope) = GcScope::create_root();
        let mut gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        initialize_default_realm(&mut agent, gc.reborrow());
        let realm = agent.current_realm(gc.nogc());
        let script = match compiled {
            Some(compiled) => compiled.instantiate(&mut agent, realm, None, gc.nogc()),
            None => {
                let source = String::from_str(&mut agent, source, gc.nogc());
                parse_script(&mut agent, source, realm, false, None, gc.nogc()).unwrap()
            }
        };
        let result = script_evaluation(&mut agent, script.unbind(), gc.reborrow())
            .unbind()
            .unwrap();
        let text = to_string(&mut agent, result, gc.reborrow())
            .unbind()
            .unwrap();
        text.to_string_lossy_(&agent).into_owned()
    }

    fn export(source: &str, compiled: Option<&CompiledScript>) -> CompiledScript {
        let (mut gc, mut scope) = GcScope::create_root();
        let mut gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        initialize_default_realm(&mut agent, gc.reborrow());
        let realm = agent.current_realm(gc.nogc());
        let script = match compiled {
            Some(compiled) => compiled.instantiate(&mut agent, realm, None, gc.nogc()),
            None => {
                let source = String::from_str(&mut agent, source, gc.nogc());
                parse_script(&mut agent, source, realm, false, None, gc.nogc()).unwrap()
            }
        };
        CompiledScript::export(&mut agent, script, gc.nogc()).unwrap()
    }

    #[test]
    fn instantiated_script_is_the_parsed_one() {
        for source in [TRICKY, "'use strict'; var x = 1; x", "", "1", "`${1}`"] {
            let compiled = export(source, None);
            assert!(export(source, Some(&compiled)) == compiled, "{source}");
            assert_eq!(run(source, Some(&compiled)), run(source, None), "{source}");
        }
    }

    #[test]
    fn evaluated_script_is_not_exported() {
        let (mut gc, mut scope) = GcScope::create_root();
        let mut gc = GcScope::new(&mut gc, &mut scope);
        let mut agent = Agent::new(AgentOptions::default(), &DefaultHostHooks);
        initialize_default_realm(&mut agent, gc.reborrow());
        let realm = agent.current_realm(gc.nogc());
        let source = String::from_str(&mut agent, "1", gc.nogc());
        let script = parse_script(&mut agent, source, realm, false, None, gc.nogc())
            .unwrap()
            .unbind();
        let _ = script_evaluation(&mut agent, script, gc.reborrow());
        assert!(matches!(
            CompiledScript::export(&mut agent, script, gc.nogc()),
            Err(super::CompiledScriptError::AlreadyEvaluated)
        ));
    }
}
