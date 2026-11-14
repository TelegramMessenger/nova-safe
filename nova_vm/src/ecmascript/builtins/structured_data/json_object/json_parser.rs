// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A JSON text parser (ECMA-404) that builds ECMAScript values directly.
//!
//! It reads the WTF-8 bytes of the source string, so lone surrogates in the
//! text and in `\u` escapes stay in the resulting strings. Containers are
//! tracked on an explicit stack, never on the native one, and nesting deeper
//! than [`MAX_NESTING`] is a RangeError.

use wtf8::{CodePoint, Wtf8Buf, Wtf8Str};

use crate::{
    ecmascript::{
        Agent, Array, ExceptionType, Number, Object, OrdinaryObject, PropertyKey, ProtoIntrinsics,
        String, Value, ordinary_object_create_with_intrinsics, try_create_data_property,
        unwrap_try,
    },
    engine::NoGcScope,
};

/// The deepest nesting of arrays and objects a JSON text may have.
pub(crate) const MAX_NESTING: usize = 5000;

pub(crate) struct JsonParseError {
    pub(crate) kind: ExceptionType,
    pub(crate) message: std::string::String,
}

enum Frame<'gc> {
    Array(Vec<Value<'gc>>),
    Object(OrdinaryObject<'gc>, PropertyKey<'gc>),
}

enum JsonStr {
    Raw(usize, usize),
    Escaped(Wtf8Buf),
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

/// Parse `bytes`, the WTF-8 contents of a JSON text, into an ECMAScript value.
pub(crate) fn parse_json_text<'gc>(
    agent: &mut Agent,
    bytes: &[u8],
    gc: NoGcScope<'gc, '_>,
) -> Result<Value<'gc>, JsonParseError> {
    let mut parser = Parser { bytes, pos: 0 };
    let mut stack: Vec<Frame<'gc>> = Vec::new();
    'value: loop {
        parser.skip_whitespace();
        let mut value: Value<'gc> = match parser.peek() {
            Some(b'{') => {
                parser.pos += 1;
                let Object::Object(object) = ordinary_object_create_with_intrinsics(
                    agent,
                    ProtoIntrinsics::Object,
                    None,
                    gc,
                ) else {
                    unreachable!()
                };
                parser.skip_whitespace();
                if parser.peek() == Some(b'}') {
                    parser.pos += 1;
                    object.into()
                } else {
                    if stack.len() >= MAX_NESTING {
                        return Err(too_deep());
                    }
                    let key = parser.member_name(agent, gc)?;
                    stack.push(Frame::Object(object, key));
                    continue 'value;
                }
            }
            Some(b'[') => {
                parser.pos += 1;
                parser.skip_whitespace();
                if parser.peek() == Some(b']') {
                    parser.pos += 1;
                    Array::from_slice(agent, &[], gc).into()
                } else {
                    if stack.len() >= MAX_NESTING {
                        return Err(too_deep());
                    }
                    stack.push(Frame::Array(Vec::new()));
                    continue 'value;
                }
            }
            Some(b'"') => {
                let s = parser.string()?;
                parser.string_value(agent, s, gc).into()
            }
            Some(b'-' | b'0'..=b'9') => Number::from_f64(agent, parser.number()?, gc).into(),
            Some(b't') => {
                parser.literal(b"true")?;
                Value::Boolean(true)
            }
            Some(b'f') => {
                parser.literal(b"false")?;
                Value::Boolean(false)
            }
            Some(b'n') => {
                parser.literal(b"null")?;
                Value::Null
            }
            _ => return Err(parser.unexpected()),
        };
        loop {
            let Some(frame) = stack.last_mut() else {
                parser.skip_whitespace();
                if parser.pos < parser.bytes.len() {
                    return Err(parser.syntax_error(format!(
                        "Unexpected non-whitespace character after JSON at position {}",
                        parser.utf16_position()
                    )));
                }
                return Ok(value);
            };
            match frame {
                Frame::Array(elements) => {
                    elements.push(value);
                    parser.skip_whitespace();
                    match parser.peek() {
                        Some(b',') => {
                            parser.pos += 1;
                            continue 'value;
                        }
                        Some(b']') => {
                            parser.pos += 1;
                            let Some(Frame::Array(elements)) = stack.pop() else {
                                unreachable!()
                            };
                            value = Array::from_slice(agent, &elements, gc).into();
                        }
                        _ => return Err(parser.unexpected()),
                    }
                }
                Frame::Object(object, key) => {
                    unwrap_try(try_create_data_property(
                        agent, *object, *key, value, None, gc,
                    ));
                    parser.skip_whitespace();
                    match parser.peek() {
                        Some(b',') => {
                            parser.pos += 1;
                            parser.skip_whitespace();
                            *key = parser.member_name(agent, gc)?;
                            continue 'value;
                        }
                        Some(b'}') => {
                            parser.pos += 1;
                            let Some(Frame::Object(object, _)) = stack.pop() else {
                                unreachable!()
                            };
                            value = object.into();
                        }
                        _ => return Err(parser.unexpected()),
                    }
                }
            }
        }
    }
}

fn too_deep() -> JsonParseError {
    JsonParseError {
        kind: ExceptionType::RangeError,
        message: format!("JSON text is nested deeper than {MAX_NESTING} levels"),
    }
}

fn hex_value(byte: u8) -> Option<u16> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u16),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u16),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u16),
        _ => None,
    }
}

impl Parser<'_> {
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn skip_whitespace(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.pos += 1;
        }
    }

    fn utf16_position(&self) -> usize {
        self.bytes[..self.pos.min(self.bytes.len())]
            .iter()
            .map(|&b| match b {
                0x80..=0xBF => 0,
                0xF0..=0xFF => 2,
                _ => 1,
            })
            .sum()
    }

    fn syntax_error(&self, message: std::string::String) -> JsonParseError {
        JsonParseError {
            kind: ExceptionType::SyntaxError,
            message,
        }
    }

    fn unexpected(&self) -> JsonParseError {
        let rest = &self.bytes[self.pos.min(self.bytes.len())..];
        if rest.is_empty() {
            return self.syntax_error("Unexpected end of JSON input".into());
        }
        let len = match rest[0] {
            0xF0..=0xFF => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        }
        .min(rest.len());
        let token = Wtf8Str::from_wtf8_bytes(&rest[..len]).to_string_lossy();
        self.syntax_error(format!(
            "Unexpected token '{}' in JSON at position {}",
            token,
            self.utf16_position()
        ))
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), JsonParseError> {
        for &expected in word {
            if self.peek() != Some(expected) {
                return Err(self.unexpected());
            }
            self.pos += 1;
        }
        Ok(())
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while let Some(b'0'..=b'9') = self.peek() {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Result<f64, JsonParseError> {
        let start = self.pos;
        let negative = self.peek() == Some(b'-');
        if negative {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.unexpected()),
        }
        let integer_end = self.pos;
        let mut simple = true;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return Err(self.unexpected());
            }
            simple = false;
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.pos += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return Err(self.unexpected());
            }
            simple = false;
        }
        let digits_start = if negative { start + 1 } else { start };
        if simple && integer_end - digits_start <= 15 {
            let magnitude = self.bytes[digits_start..integer_end]
                .iter()
                .fold(0u64, |acc, &b| acc * 10 + (b - b'0') as u64)
                as f64;
            return Ok(if negative { -magnitude } else { magnitude });
        }
        let text =
            core::str::from_utf8(&self.bytes[start..self.pos]).expect("a JSON number is ASCII");
        Ok(text
            .parse::<f64>()
            .expect("the JSON number grammar is a subset of Rust's float syntax"))
    }

    /// Scan a string starting at its opening quote.
    fn string(&mut self) -> Result<JsonStr, JsonParseError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.pos += 1;
        let start = self.pos;
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(JsonStr::Raw(start, self.pos - 1));
                }
                Some(b'\\') => break,
                Some(0x00..=0x1F) => return Err(self.control_character()),
                Some(_) => self.pos += 1,
                None => return Err(self.unexpected()),
            }
        }
        let mut buf = Wtf8Buf::with_capacity(self.pos - start + 16);
        let mut run_start = start;
        loop {
            match self.peek() {
                Some(b'"') => {
                    buf.push_wtf8(Wtf8Str::from_wtf8_bytes(&self.bytes[run_start..self.pos]));
                    self.pos += 1;
                    return Ok(JsonStr::Escaped(buf));
                }
                Some(b'\\') => {
                    buf.push_wtf8(Wtf8Str::from_wtf8_bytes(&self.bytes[run_start..self.pos]));
                    self.pos += 1;
                    let unit = match self.peek() {
                        Some(b'"') => b'"' as u16,
                        Some(b'\\') => b'\\' as u16,
                        Some(b'/') => b'/' as u16,
                        Some(b'b') => 0x08,
                        Some(b'f') => 0x0C,
                        Some(b'n') => b'\n' as u16,
                        Some(b'r') => b'\r' as u16,
                        Some(b't') => b'\t' as u16,
                        Some(b'u') => {
                            let mut unit = 0u16;
                            for _ in 0..4 {
                                self.pos += 1;
                                let Some(digit) = self.peek().and_then(hex_value) else {
                                    return Err(self.bad_escape());
                                };
                                unit = unit * 16 + digit;
                            }
                            unit
                        }
                        None => return Err(self.unexpected()),
                        Some(_) => return Err(self.bad_escape()),
                    };
                    self.pos += 1;
                    buf.push(CodePoint::from_u16(unit));
                    run_start = self.pos;
                }
                Some(0x00..=0x1F) => return Err(self.control_character()),
                Some(_) => self.pos += 1,
                None => return Err(self.unexpected()),
            }
        }
    }

    fn control_character(&self) -> JsonParseError {
        self.syntax_error(format!(
            "Bad control character in string literal in JSON at position {}",
            self.utf16_position()
        ))
    }

    fn bad_escape(&self) -> JsonParseError {
        if self.pos >= self.bytes.len() {
            return self.unexpected();
        }
        self.syntax_error(format!(
            "Bad escaped character in JSON at position {}",
            self.utf16_position()
        ))
    }

    fn string_value<'gc>(
        &self,
        agent: &mut Agent,
        s: JsonStr,
        gc: NoGcScope<'gc, '_>,
    ) -> String<'gc> {
        match s {
            JsonStr::Raw(start, end) => {
                let bytes = &self.bytes[start..end];
                match core::str::from_utf8(bytes) {
                    Ok(str) => String::from_str(agent, str, gc),
                    Err(_) => {
                        let mut buf = Wtf8Buf::with_capacity(bytes.len());
                        buf.push_wtf8(Wtf8Str::from_wtf8_bytes(bytes));
                        String::from_wtf8_buf(agent, buf, gc)
                    }
                }
            }
            JsonStr::Escaped(buf) => match buf.into_string() {
                Ok(string) => String::from_string(agent, string, gc),
                Err(buf) => String::from_wtf8_buf(agent, buf, gc),
            },
        }
    }

    /// Parse `"name" :` of an object member.
    fn member_name<'gc>(
        &mut self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> Result<PropertyKey<'gc>, JsonParseError> {
        if self.peek() != Some(b'"') {
            return Err(self.unexpected());
        }
        let name = self.string()?;
        let key = match name {
            JsonStr::Raw(start, end) => match core::str::from_utf8(&self.bytes[start..end]) {
                Ok(str) => PropertyKey::from_str(agent, str, gc),
                Err(_) => self.string_value(agent, name, gc).into(),
            },
            JsonStr::Escaped(buf) => match buf.into_string() {
                Ok(string) => PropertyKey::from_string(agent, string, gc),
                Err(buf) => String::from_wtf8_buf(agent, buf, gc).into(),
            },
        };
        self.skip_whitespace();
        if self.peek() != Some(b':') {
            return Err(self.unexpected());
        }
        self.pos += 1;
        Ok(key)
    }
}
