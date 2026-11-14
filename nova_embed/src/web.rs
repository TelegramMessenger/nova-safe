// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The globals a QuickJS context has beyond ECMAScript that the widget
//! bundles read from the engine: `atob` and `btoa` (the HTML standard's
//! forgiving-base64, as quickjs-ng implements them). Scripts that run on
//! QuickJS find them there; the trusted validator decodes `data:` images with
//! `atob`.

use nova_vm::{
    ecmascript::{
        Agent, ArgumentsList, Behaviour, BuiltinFunctionArgs, ExceptionType, InternalMethods,
        JsError, JsResult, PropertyDescriptor, PropertyKey, RegularFn, String as JsString, Value,
        create_builtin_function,
    },
    engine::{Bindable, GcScope, NoGcScope},
};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Bytes of input between two interrupt polls.
const POLL_BYTES: usize = 1024;

const ATOB_MESSAGE: &str = "The string to be decoded is not correctly encoded";
const BTOA_MESSAGE: &str = "String contains an invalid character";

/// Defines `atob` and `btoa` on the realm's global object, as quickjs-ng
/// does: writable, configurable, not enumerable.
pub(crate) fn install(agent: &mut Agent, mut gc: GcScope) {
    let functions: [(&'static str, RegularFn); 2] = [("atob", atob), ("btoa", btoa)];
    for (name, behaviour) in functions {
        let function = create_builtin_function(
            agent,
            Behaviour::Regular(behaviour),
            BuiltinFunctionArgs::new(1, name),
            gc.nogc(),
        );
        let value: Value = function.unbind().into();
        let key = PropertyKey::from_static_str(agent, name, gc.nogc());
        let global = agent.current_realm(gc.nogc()).global_object(agent);
        let defined = global
            .unbind()
            .internal_define_own_property(
                agent,
                key.unbind(),
                PropertyDescriptor::non_enumerable_data_descriptor(value),
                gc.reborrow(),
            )
            .unwrap_or(false);
        assert!(defined, "the global {name} could not be defined");
    }
}

/// An `Error` named `InvalidCharacterError` with code 5, as the
/// `DOMException` quickjs-ng throws (there is no DOMException here).
fn invalid_character<'a>(
    agent: &mut Agent,
    message: &'static str,
    gc: NoGcScope<'a, '_>,
) -> JsError<'a> {
    let error = agent.throw_exception_with_static_message(ExceptionType::Error, message, gc);
    if let Ok(object) = nova_vm::ecmascript::Object::try_from(error.value()) {
        let name_key = PropertyKey::from_static_str(agent, "name", gc);
        let name = JsString::from_static_str(agent, "InvalidCharacterError", gc);
        let code_key = PropertyKey::from_static_str(agent, "code", gc);
        let _ = object.try_define_own_property(
            agent,
            name_key,
            PropertyDescriptor::non_enumerable_data_descriptor(name),
            None,
            gc,
        );
        let _ = object.try_define_own_property(
            agent,
            code_key,
            PropertyDescriptor::non_enumerable_data_descriptor(Value::from(5u8)),
            None,
            gc,
        );
    }
    error
}

fn is_ascii_whitespace(byte: u8) -> bool {
    matches!(byte, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

fn sextet(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The forgiving-base64 decode of `input` (WTF-8 bytes) into bytes, or `None`
/// where the HTML standard says failure.
pub(crate) fn forgiving_base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = Vec::new();
    data.try_reserve(input.len()).ok()?;
    data.extend(
        input
            .iter()
            .copied()
            .filter(|&byte| !is_ascii_whitespace(byte)),
    );
    if data.len().is_multiple_of(4) && data.last() == Some(&b'=') {
        data.pop();
        if data.last() == Some(&b'=') {
            data.pop();
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let mut output: Vec<u8> = Vec::new();
    output.try_reserve(data.len() / 4 * 3 + 2).ok()?;
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in &data {
        buffer = (buffer << 6) | u32::from(sextet(byte)?);
        bits += 6;
        if bits == 24 {
            output.extend_from_slice(&[(buffer >> 16) as u8, (buffer >> 8) as u8, buffer as u8]);
            buffer = 0;
            bits = 0;
        }
    }
    match bits {
        12 => output.push((buffer >> 4) as u8),
        18 => output.extend_from_slice(&[(buffer >> 10) as u8, (buffer >> 2) as u8]),
        _ => {}
    }
    Some(output)
}

/// The code units of `input` (WTF-8 bytes) if every one is at most 0xFF.
pub(crate) fn latin1_units(input: &[u8]) -> Option<Vec<u8>> {
    let mut units: Vec<u8> = Vec::new();
    units.try_reserve(input.len()).ok()?;
    let mut bytes = input.iter().copied();
    while let Some(byte) = bytes.next() {
        match byte {
            0x00..=0x7f => units.push(byte),
            0xc2 | 0xc3 => {
                let next = bytes.next()?;
                if next & 0xc0 != 0x80 {
                    return None;
                }
                units.push(((byte & 0x03) << 6) | (next & 0x3f));
            }
            _ => return None,
        }
    }
    Some(units)
}

/// Standard base64 with padding.
pub(crate) fn base64_encode(units: &[u8]) -> Option<String> {
    let mut output = String::new();
    output.try_reserve(units.len().div_ceil(3) * 4).ok()?;
    for group in units.chunks(3) {
        let a = u32::from(group[0]);
        let b = group.get(1).copied().map(u32::from);
        let c = group.get(2).copied().map(u32::from);
        let word = (a << 16) | (b.unwrap_or(0) << 8) | c.unwrap_or(0);
        output.push(char::from(ALPHABET[(word >> 18) as usize & 63]));
        output.push(char::from(ALPHABET[(word >> 12) as usize & 63]));
        output.push(if b.is_some() {
            char::from(ALPHABET[(word >> 6) as usize & 63])
        } else {
            '='
        });
        output.push(if c.is_some() {
            char::from(ALPHABET[word as usize & 63])
        } else {
            '='
        });
    }
    Some(output)
}

/// Polls the interrupt (and the heap limit) once for every `POLL_BYTES` of a
/// `length`-byte input, so a huge string cannot hold the thread past its
/// deadline. Results are checked like other built strings
/// (`check_string_length`: the string length and heap limits, a RangeError).
fn poll_for<'a>(agent: &mut Agent, length: usize, gc: NoGcScope<'a, '_>) -> JsResult<'a, ()> {
    let steps = (length / POLL_BYTES).min(u32::MAX as usize) as u32;
    agent.poll_interrupt_steps(steps.max(1), gc)
}

fn atob<'gc>(
    agent: &mut Agent,
    _this: Value,
    args: ArgumentsList,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    let data = args.get(0).bind(gc.nogc());
    let data = data.unbind().to_string(agent, gc.reborrow()).unbind()?;
    let gc = gc.into_nogc();
    let data = data.bind(gc);
    let length = data.as_wtf8(agent).as_bytes().len();
    poll_for(agent, length, gc)?;
    let Some(bytes) = forgiving_base64_decode(data.as_wtf8(agent).as_bytes()) else {
        return Err(invalid_character(agent, ATOB_MESSAGE, gc));
    };
    poll_for(agent, bytes.len(), gc)?;
    agent.check_string_length(bytes.len(), gc)?;
    let text: String = bytes.iter().map(|&byte| char::from(byte)).collect();
    Ok(JsString::from_string(agent, text, gc).into())
}

fn btoa<'gc>(
    agent: &mut Agent,
    _this: Value,
    args: ArgumentsList,
    mut gc: GcScope<'gc, '_>,
) -> JsResult<'gc, Value<'gc>> {
    let data = args.get(0).bind(gc.nogc());
    let data = data.unbind().to_string(agent, gc.reborrow()).unbind()?;
    let gc = gc.into_nogc();
    let data = data.bind(gc);
    let length = data.as_wtf8(agent).as_bytes().len();
    poll_for(agent, length, gc)?;
    let Some(units) = latin1_units(data.as_wtf8(agent).as_bytes()) else {
        return Err(invalid_character(agent, BTOA_MESSAGE, gc));
    };
    agent.check_string_length(units.len().div_ceil(3).saturating_mul(4), gc)?;
    let Some(text) = base64_encode(&units) else {
        return Err(agent.throw_exception_with_static_message(
            ExceptionType::RangeError,
            "Out of memory",
            gc,
        ));
    };
    poll_for(agent, text.len(), gc)?;
    Ok(JsString::from_string(agent, text, gc).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_as_the_html_standard_says() {
        let decode = |text: &str| forgiving_base64_decode(text.as_bytes());
        assert_eq!(decode(" R G 9 v\n"), Some(b"Doo".to_vec()));
        assert_eq!(decode("YQ"), Some(b"a".to_vec()));
        assert_eq!(decode("YQ=="), Some(b"a".to_vec()));
        assert_eq!(decode("YR=="), Some(b"a".to_vec()));
        assert_eq!(decode("Y Q = ="), Some(b"a".to_vec()));
        assert_eq!(decode(""), Some(Vec::new()));
        assert_eq!(decode("/+8="), Some(vec![0xff, 0xef]));
        assert_eq!(decode("AAAA"), Some(vec![0, 0, 0]));
        for bad in [
            "a",
            "Zm9v=",
            "YQ=",
            "====",
            "Y===",
            "YQ==YQ==",
            "\u{a0}YQ",
            "YQ\u{b}",
            "-_",
            "undefined",
        ] {
            assert_eq!(decode(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn encodes_latin1_only() {
        let encode =
            |text: &str| latin1_units(text.as_bytes()).and_then(|units| base64_encode(&units));
        assert_eq!(encode("undefined").as_deref(), Some("dW5kZWZpbmVk"));
        assert_eq!(encode("null").as_deref(), Some("bnVsbA=="));
        assert_eq!(encode("\u{ff}\u{fe}\u{0}").as_deref(), Some("//4A"));
        assert_eq!(encode("").as_deref(), Some(""));
        assert_eq!(encode("ab").as_deref(), Some("YWI="));
        assert_eq!(encode("\u{100}"), None);
        assert_eq!(encode("\u{1f600}"), None);
    }

    #[test]
    fn round_trips_every_byte_and_length() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        for length in 0..bytes.len() {
            let encoded = base64_encode(&bytes[..length]).unwrap();
            assert_eq!(
                forgiving_base64_decode(encoded.as_bytes()).as_deref(),
                Some(&bytes[..length])
            );
        }
    }
}
