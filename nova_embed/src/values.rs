// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Owned values in and out of the engine.

use nova_vm::{
    ecmascript::{
        Agent, ArgumentsList, ArrayBuffer, InternalMethods, JsResult, Number, Object, PropertyKey,
        String as JsString, TypedArray, Value,
    },
    engine::{Bindable, GcScope, NoGcScope, Scopable},
};

use crate::{ArgumentMode, Error, MAX_HOST_ARGUMENTS, Shared, hooks, limits};

/// The element type of a typed array argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TypedArrayKind {
    Int8 = 1,
    Uint8 = 2,
    Uint8Clamped = 3,
    Int16 = 4,
    Uint16 = 5,
    Int32 = 6,
    Uint32 = 7,
    BigInt64 = 8,
    BigUint64 = 9,
    Float32 = 10,
    Float64 = 11,
}

/// An argument of a host function call.
#[derive(Debug, Clone, PartialEq)]
pub enum HostArg {
    /// Anything that is not a string or a number (or a typed array, when the
    /// function takes bytes), including a detached or out-of-bounds view.
    Other,
    /// A string, as WTF-8: UTF-8 with each lone surrogate as its own 3-byte
    /// sequence (what QuickJS hands its hosts; a UTF-8 decoder replaces it).
    String(Vec<u8>),
    /// A number.
    Number(f64),
    /// A copy of the bytes a typed array views.
    Bytes {
        kind: TypedArrayKind,
        bytes: Vec<u8>,
    },
}

/// What a host function returns to the script.
#[derive(Debug, Clone, PartialEq)]
pub enum HostReturn {
    Undefined,
    Null,
    String(String),
    Number(f64),
    /// A new ArrayBuffer holding a copy of the bytes.
    ArrayBuffer(Vec<u8>),
}

/// The value a script evaluation or a call completed with: strings (as
/// WTF-8, see [`HostArg::String`]) and numbers; anything else is `Undefined`.
#[derive(Debug, Clone, PartialEq)]
pub enum Completion {
    Undefined,
    String(Vec<u8>),
    Number(f64),
}

fn number_of(agent: &Agent, value: Value) -> Option<f64> {
    Number::try_from(value)
        .ok()
        .map(|number| number.into_f64(agent))
}

fn typed_array_bytes(agent: &Agent, value: Value) -> Option<HostArg> {
    let array = TypedArray::try_from(value).ok()?;
    let (kind, element_size) = match array {
        TypedArray::Int8Array(_) => (TypedArrayKind::Int8, 1),
        TypedArray::Uint8Array(_) => (TypedArrayKind::Uint8, 1),
        TypedArray::Uint8ClampedArray(_) => (TypedArrayKind::Uint8Clamped, 1),
        TypedArray::Int16Array(_) => (TypedArrayKind::Int16, 2),
        TypedArray::Uint16Array(_) => (TypedArrayKind::Uint16, 2),
        TypedArray::Int32Array(_) => (TypedArrayKind::Int32, 4),
        TypedArray::Uint32Array(_) => (TypedArrayKind::Uint32, 4),
        TypedArray::BigInt64Array(_) => (TypedArrayKind::BigInt64, 8),
        TypedArray::BigUint64Array(_) => (TypedArrayKind::BigUint64, 8),
        TypedArray::Float32Array(_) => (TypedArrayKind::Float32, 4),
        TypedArray::Float64Array(_) => (TypedArrayKind::Float64, 8),
        #[allow(unreachable_patterns)]
        _ => return None,
    };
    let buffer = array.get_viewed_array_buffer(agent);
    if buffer.is_detached(agent) {
        return None;
    }
    let data = buffer.as_slice(agent);
    let offset = array.byte_offset(agent);
    let end = match array.array_length(agent) {
        Some(length) => offset.checked_add(length.checked_mul(element_size)?)?,
        None => {
            let available = data.len().checked_sub(offset)?;
            offset + available / element_size * element_size
        }
    };
    let bytes = data.get(offset..end)?;
    Some(HostArg::Bytes {
        kind,
        bytes: bytes.to_vec(),
    })
}

fn host_arg(agent: &Agent, value: Value, mode: ArgumentMode) -> HostArg {
    if let Ok(string) = JsString::try_from(value) {
        return HostArg::String(string.as_wtf8(agent).as_bytes().to_vec());
    }
    if let Some(number) = number_of(agent, value) {
        return HostArg::Number(number);
    }
    if mode == ArgumentMode::ValuesAndBytes
        && let Some(bytes) = typed_array_bytes(agent, value)
    {
        return bytes;
    }
    HostArg::Other
}

/// The body of every host function: converts the arguments, calls the host
/// with the slot's id, converts what it returns. Runs no JavaScript.
pub(crate) fn dispatch<'gc>(
    slot: usize,
    agent: &mut Agent,
    args: ArgumentsList,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    let gc = gc.into_nogc();
    let args = args.bind(gc);
    let Some(shared) = hooks::current() else {
        return Ok(Value::Undefined);
    };
    let Some(slot) = shared.slots.borrow().get(slot).copied() else {
        return Ok(Value::Undefined);
    };
    let count = args.len().min(MAX_HOST_ARGUMENTS);
    let mut converted = Vec::with_capacity(count);
    for index in 0..count {
        converted.push(host_arg(agent, args.get(index), slot.mode));
    }
    let returned = shared.host.call(slot.id, converted);
    to_value(agent, returned, gc)
}

fn to_value<'gc>(
    agent: &mut Agent,
    value: HostReturn,
    gc: NoGcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    Ok(match value {
        HostReturn::Undefined => Value::Undefined,
        HostReturn::Null => Value::Null,
        HostReturn::String(text) => JsString::from_string(agent, text, gc).into(),
        HostReturn::Number(number) => Number::from_f64(agent, number, gc).into(),
        HostReturn::ArrayBuffer(bytes) => {
            let buffer = ArrayBuffer::new(agent, bytes.len(), gc)?;
            buffer.as_mut_slice(agent).copy_from_slice(&bytes);
            buffer.into()
        }
    })
}

/// `globalThis[name]`; getters run.
pub(crate) fn get_global<'gc>(
    agent: &mut Agent,
    name: &str,
    gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    let key = PropertyKey::from_str(agent, name, gc.nogc());
    let global = agent.current_realm(gc.nogc()).global_object(agent);
    global
        .unbind()
        .internal_get(agent, key.unbind(), global.unbind().into(), gc)
}

/// What a script completed with, or how it failed.
pub(crate) fn completion(
    agent: &mut Agent,
    shared: &Shared,
    result: JsResult<Value>,
    gc: GcScope,
) -> Result<Completion, Error> {
    match result {
        Ok(value) => {
            if let Ok(string) = JsString::try_from(value) {
                Ok(Completion::String(
                    string.as_wtf8(agent).as_bytes().to_vec(),
                ))
            } else if let Some(number) = number_of(agent, value) {
                Ok(Completion::Number(number))
            } else {
                Ok(Completion::Undefined)
            }
        }
        Err(error) => Err(self::error(agent, shared, error.value().unbind(), gc)),
    }
}

/// The error for a thrown value: the engine's termination if it ended the
/// script, else the exception's text.
pub(crate) fn error(agent: &mut Agent, _shared: &Shared, thrown: Value, gc: GcScope) -> Error {
    if let Some(error) = limits::take_termination(agent) {
        return error;
    }
    let text = describe(agent, thrown, gc);
    if let Some(error) = limits::take_termination(agent) {
        return error;
    }
    if limits::is_allocation_refused(agent, &text) {
        return Error::OutOfMemory;
    }
    Error::Exception(text)
}

fn string_property(
    agent: &mut Agent,
    object: Object,
    name: &'static str,
    gc: GcScope,
) -> Option<String> {
    let key = PropertyKey::from_static_str(agent, name, gc.nogc());
    let value = object
        .unbind()
        .internal_get(agent, key.unbind(), object.unbind().into(), gc)
        .ok()?;
    JsString::try_from(value)
        .ok()
        .map(|string| string.to_string_lossy(agent).into_owned())
}

/// `name: message` (and the stack, if the error has one as a string), as
/// the QuickJS hosts print exceptions; any other value as a string.
fn describe(agent: &mut Agent, thrown: Value, mut gc: GcScope) -> String {
    let thrown = thrown.bind(gc.nogc());
    if let Ok(object) = Object::try_from(thrown) {
        let object = object.scope(agent, gc.nogc());
        let name = string_property(agent, object.get(agent), "name", gc.reborrow());
        let message = string_property(agent, object.get(agent), "message", gc.reborrow());
        let stack = string_property(agent, object.get(agent), "stack", gc.reborrow());
        if name.is_some() || message.is_some() {
            let mut text = format!(
                "{}: {}",
                name.as_deref().unwrap_or("Error"),
                message.as_deref().unwrap_or("")
            );
            if let Some(stack) = stack {
                text.push('\n');
                text.push_str(&stack);
            }
            return text;
        }
        let value: Value = object.get(agent).into();
        return value
            .unbind()
            .string_repr(agent, gc.reborrow())
            .to_string_lossy(agent)
            .into_owned();
    }
    thrown
        .unbind()
        .string_repr(agent, gc.reborrow())
        .to_string_lossy(agent)
        .into_owned()
}
