// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! [WTF-8] strings: UTF-8 extended with lone surrogate code points, so that
//! every JavaScript string (a sequence of UTF-16 code units, possibly
//! ill-formed) has an encoding.
//!
//! The owned form is [`Wtf8Buf`] (a `Vec<u8>`), the borrowed form is the value
//! type [`Wtf8Str`] (a `&[u8]`). Both keep the bytes well-formed WTF-8: they
//! are only built from `str`, from code points, from other WTF-8 strings or
//! by slicing at code point boundaries. [`Wtf8Str::from_wtf8_bytes`] trusts its
//! caller; malformed bytes there make the methods return wrong code points or
//! panic, never undefined behaviour, because nothing here is `unsafe`.
//!
//! [WTF-8]: https://wtf-8.codeberg.page/

#![forbid(unsafe_code)]

use core::{fmt, hash, iter::FromIterator, slice};
use std::borrow::Cow;

const UTF8_REPLACEMENT_CHARACTER: &[u8] = "\u{FFFD}".as_bytes();

/// A Unicode code point: from U+0000 to U+10FFFF, surrogates included.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Copy, Hash)]
pub struct CodePoint {
    value: u32,
}

impl fmt::Debug for CodePoint {
    #[inline]
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(formatter, "U+{:04X}", self.value)
    }
}

impl CodePoint {
    /// Create a code point from a `u32`, or `None` above U+10FFFF.
    #[inline]
    pub const fn from_u32(value: u32) -> Option<CodePoint> {
        match value {
            0..=0x10FFFF => Some(CodePoint { value }),
            _ => None,
        }
    }

    /// Create a code point from a UTF-16 code unit; every one is a code point.
    #[inline]
    pub const fn from_u16(value: u16) -> CodePoint {
        CodePoint {
            value: value as u32,
        }
    }

    /// Create a code point from a `char`.
    #[inline]
    pub const fn from_char(value: char) -> CodePoint {
        CodePoint {
            value: value as u32,
        }
    }

    /// The numeric value of the code point.
    #[inline]
    pub const fn to_u32(&self) -> u32 {
        self.value
    }

    /// The `char` of the code point, or `None` for a surrogate.
    #[inline]
    pub const fn to_char(&self) -> Option<char> {
        char::from_u32(self.value)
    }

    /// The `char` of the code point, with U+FFFD for a surrogate.
    #[inline]
    pub fn to_char_lossy(&self) -> char {
        self.to_char().unwrap_or('\u{FFFD}')
    }
}

/// An owned, growable WTF-8 string.
#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Default)]
pub struct Wtf8Buf {
    bytes: Vec<u8>,
}

impl fmt::Debug for Wtf8Buf {
    #[inline]
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt::Debug::fmt(&self.as_wtf8(), formatter)
    }
}

impl hash::Hash for Wtf8Buf {
    #[inline]
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.as_wtf8().hash(state)
    }
}

impl Wtf8Buf {
    /// An empty string.
    #[inline]
    pub const fn new() -> Wtf8Buf {
        Wtf8Buf { bytes: Vec::new() }
    }

    /// An empty string with room for `n` bytes.
    #[inline]
    pub fn with_capacity(n: usize) -> Wtf8Buf {
        Wtf8Buf {
            bytes: Vec::with_capacity(n),
        }
    }

    /// Take over a UTF-8 `String` without copying it.
    #[inline]
    pub fn from_string(string: String) -> Wtf8Buf {
        Wtf8Buf {
            bytes: string.into_bytes(),
        }
    }

    /// Copy a `str`.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(str: &str) -> Wtf8Buf {
        Wtf8Buf {
            bytes: str.as_bytes().to_vec(),
        }
    }

    /// Decode potentially ill-formed UTF-16: pairs become supplementary code
    /// points, lone surrogates stay surrogate code points.
    pub fn from_ill_formed_utf16(v: &[u16]) -> Wtf8Buf {
        let mut string = Wtf8Buf::with_capacity(v.len());
        for item in char::decode_utf16(v.iter().copied()) {
            match item {
                Ok(c) => string.push_char(c),
                Err(e) => string.push_code_point_unpaired(e.unpaired_surrogate() as u32),
            }
        }
        string
    }

    /// Borrow the string.
    #[inline]
    pub fn as_wtf8(&self) -> Wtf8Str<'_> {
        Wtf8Str { bytes: &self.bytes }
    }

    /// The WTF-8 bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Reserve room for `additional` more bytes.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.bytes.reserve(additional)
    }

    /// The allocated capacity in bytes.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.bytes.capacity()
    }

    /// Append a `str`.
    #[inline]
    pub fn push_str(&mut self, other: &str) {
        self.bytes.extend_from_slice(other.as_bytes())
    }

    /// Append a WTF-8 string; a lead surrogate at the end of `self` and a
    /// trail surrogate at the start of `other` join into one code point.
    #[inline]
    pub fn push_wtf8<'b>(&mut self, other: impl Into<Wtf8Str<'b>>) {
        let other: Wtf8Str<'b> = other.into();
        match (
            self.as_wtf8().final_lead_surrogate(),
            other.initial_trail_surrogate(),
        ) {
            (Some(lead), Some(trail)) => {
                let len_without_lead_surrogate = self.bytes.len() - 3;
                self.bytes.truncate(len_without_lead_surrogate);
                let other_without_trail_surrogate = &other.bytes[3..];
                self.bytes.reserve(4 + other_without_trail_surrogate.len());
                self.push_char(decode_surrogate_pair(lead, trail));
                self.bytes.extend_from_slice(other_without_trail_surrogate);
            }
            _ => self.bytes.extend_from_slice(other.bytes),
        }
    }

    /// Append a `char`.
    #[inline]
    pub fn push_char(&mut self, c: char) {
        if c.is_ascii() {
            self.bytes.push(c as u8);
        } else {
            let mut buf = [0u8; 4];
            self.bytes
                .extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }

    /// Append a code point; a trail surrogate after a lead surrogate at the
    /// end of `self` joins it into one code point.
    #[inline]
    pub fn push(&mut self, code_point: CodePoint) {
        if let trail @ 0xDC00..=0xDFFF = code_point.to_u32()
            && let Some(lead) = self.as_wtf8().final_lead_surrogate()
        {
            let len_without_lead_surrogate = self.bytes.len() - 3;
            self.bytes.truncate(len_without_lead_surrogate);
            self.push_char(decode_surrogate_pair(lead, trail as u16));
            return;
        }
        self.push_code_point_unpaired(code_point.to_u32())
    }

    fn push_code_point_unpaired(&mut self, code: u32) {
        match char::from_u32(code) {
            Some(c) => self.push_char(c),
            None => {
                debug_assert!((0xD800..=0xDFFF).contains(&code));
                self.bytes.extend_from_slice(&[
                    (0xE0 | (code >> 12)) as u8,
                    (0x80 | ((code >> 6) & 0x3F)) as u8,
                    (0x80 | (code & 0x3F)) as u8,
                ]);
            }
        }
    }

    /// Shorten the string to `new_len` bytes.
    ///
    /// ## Panics
    ///
    /// If `new_len` is not a code point boundary.
    #[inline]
    pub fn truncate(&mut self, new_len: usize) {
        assert!(self.as_wtf8().is_code_point_boundary(new_len));
        self.bytes.truncate(new_len)
    }

    /// Convert to a `String`, or give the buffer back if it has surrogates.
    pub fn into_string(self) -> Result<String, Wtf8Buf> {
        String::from_utf8(self.bytes).map_err(|e| Wtf8Buf {
            bytes: e.into_bytes(),
        })
    }

    /// Convert to a `String`, replacing surrogates with U+FFFD.
    pub fn into_string_lossy(self) -> String {
        match String::from_utf8(self.bytes) {
            Ok(s) => s,
            Err(e) => {
                let mut bytes = e.into_bytes();
                let mut pos = 0;
                while let Some((surrogate_pos, _)) = (Wtf8Str { bytes: &bytes }).next_surrogate(pos)
                {
                    pos = surrogate_pos + 3;
                    bytes[surrogate_pos..pos].copy_from_slice(UTF8_REPLACEMENT_CHARACTER);
                }
                String::from_utf8(bytes).expect("WTF-8 without surrogates is UTF-8")
            }
        }
    }

    /// The length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns true if the string is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// See [`Wtf8Str::slice`].
    #[inline]
    pub fn slice(&self, begin: usize, end: usize) -> Wtf8Str<'_> {
        self.as_wtf8().slice(begin, end)
    }

    /// See [`Wtf8Str::slice_from`].
    #[inline]
    pub fn slice_from(&self, begin: usize) -> Wtf8Str<'_> {
        self.as_wtf8().slice_from(begin)
    }

    /// See [`Wtf8Str::slice_to`].
    #[inline]
    pub fn slice_to(&self, end: usize) -> Wtf8Str<'_> {
        self.as_wtf8().slice_to(end)
    }

    /// See [`Wtf8Str::ascii_byte_at`].
    #[inline]
    pub fn ascii_byte_at(&self, position: usize) -> u8 {
        self.as_wtf8().ascii_byte_at(position)
    }

    /// See [`Wtf8Str::code_points`].
    #[inline]
    pub fn code_points(&self) -> Wtf8CodePoints<'_> {
        self.as_wtf8().code_points()
    }

    /// See [`Wtf8Str::as_str`].
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        self.as_wtf8().as_str()
    }

    /// See [`Wtf8Str::to_string_lossy`].
    #[inline]
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        self.as_wtf8().to_string_lossy()
    }

    /// See [`Wtf8Str::to_ill_formed_utf16`].
    #[inline]
    pub fn to_ill_formed_utf16(&self) -> IllFormedUtf16CodeUnits<'_> {
        self.as_wtf8().to_ill_formed_utf16()
    }
}

impl FromIterator<CodePoint> for Wtf8Buf {
    fn from_iter<T: IntoIterator<Item = CodePoint>>(iterable: T) -> Wtf8Buf {
        let mut string = Wtf8Buf::new();
        string.extend(iterable);
        string
    }
}

impl Extend<CodePoint> for Wtf8Buf {
    fn extend<T: IntoIterator<Item = CodePoint>>(&mut self, iterable: T) {
        let iterator = iterable.into_iter();
        let (low, _high) = iterator.size_hint();
        self.bytes.reserve(low);
        for code_point in iterator {
            self.push(code_point);
        }
    }
}

/// A borrowed WTF-8 string: a copyable view of well-formed WTF-8 bytes.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Default)]
pub struct Wtf8Str<'a> {
    bytes: &'a [u8],
}

impl hash::Hash for Wtf8Str<'_> {
    #[inline]
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        state.write(self.bytes);
        0xfeu8.hash(state)
    }
}

impl fmt::Debug for Wtf8Str<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        formatter.write_str("\"")?;
        let mut pos = 0;
        while let Some((surrogate_pos, surrogate)) = self.next_surrogate(pos) {
            formatter.write_str(&String::from_utf8_lossy(&self.bytes[pos..surrogate_pos]))?;
            write!(formatter, "\\u{{{surrogate:X}}}")?;
            pos = surrogate_pos + 3;
        }
        formatter.write_str(&String::from_utf8_lossy(&self.bytes[pos..]))?;
        formatter.write_str("\"")
    }
}

impl<'a> From<&'a Wtf8Buf> for Wtf8Str<'a> {
    #[inline]
    fn from(value: &'a Wtf8Buf) -> Self {
        value.as_wtf8()
    }
}

impl<'a> From<&'a str> for Wtf8Str<'a> {
    #[inline]
    fn from(value: &'a str) -> Self {
        Wtf8Str::from_str(value)
    }
}

impl PartialEq<Wtf8Str<'_>> for Wtf8Buf {
    #[inline]
    fn eq(&self, other: &Wtf8Str<'_>) -> bool {
        self.bytes == other.bytes
    }
}

impl PartialEq<Wtf8Buf> for Wtf8Str<'_> {
    #[inline]
    fn eq(&self, other: &Wtf8Buf) -> bool {
        self.bytes == other.bytes
    }
}

impl<'a> Wtf8Str<'a> {
    /// Borrow a `str` as WTF-8.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub const fn from_str(value: &'a str) -> Wtf8Str<'a> {
        Wtf8Str {
            bytes: value.as_bytes(),
        }
    }

    /// View bytes that hold well-formed WTF-8.
    ///
    /// ## Correctness
    ///
    /// `bytes` must be well-formed WTF-8, as produced by [`Wtf8Buf`] or
    /// [`Wtf8Str::as_bytes`]. Malformed bytes make the methods return wrong
    /// code points or panic; they cannot cause undefined behaviour.
    #[inline]
    pub const fn from_wtf8_bytes(bytes: &'a [u8]) -> Wtf8Str<'a> {
        Wtf8Str { bytes }
    }

    /// The WTF-8 bytes.
    #[inline]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }

    /// The length in bytes.
    #[inline]
    pub const fn len(self) -> usize {
        self.bytes.len()
    }

    /// Returns true if the string is empty.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }

    #[inline]
    fn is_code_point_boundary(self, index: usize) -> bool {
        if index == self.bytes.len() {
            return true;
        }
        match self.bytes.get(index) {
            None => false,
            Some(&b) => !(128u8..192u8).contains(&b),
        }
    }

    #[inline(never)]
    #[cold]
    fn slice_error_fail(self, begin: usize, end: usize) -> ! {
        assert!(begin <= end);
        panic!("index {begin} and/or {end} in {self:?} do not lie on character boundary");
    }

    /// The bytes from `begin` to `end`.
    ///
    /// ## Panics
    ///
    /// If `begin > end` or either is not a code point boundary.
    #[inline]
    pub fn slice(self, begin: usize, end: usize) -> Wtf8Str<'a> {
        if begin <= end && self.is_code_point_boundary(begin) && self.is_code_point_boundary(end) {
            Wtf8Str {
                bytes: &self.bytes[begin..end],
            }
        } else {
            self.slice_error_fail(begin, end)
        }
    }

    /// The bytes from `begin` to the end.
    ///
    /// ## Panics
    ///
    /// If `begin` is not a code point boundary.
    #[inline]
    pub fn slice_from(self, begin: usize) -> Wtf8Str<'a> {
        if self.is_code_point_boundary(begin) {
            Wtf8Str {
                bytes: &self.bytes[begin..],
            }
        } else {
            self.slice_error_fail(begin, self.bytes.len())
        }
    }

    /// The bytes from the start to `end`.
    ///
    /// ## Panics
    ///
    /// If `end` is not a code point boundary.
    #[inline]
    pub fn slice_to(self, end: usize) -> Wtf8Str<'a> {
        if self.is_code_point_boundary(end) {
            Wtf8Str {
                bytes: &self.bytes[..end],
            }
        } else {
            self.slice_error_fail(0, end)
        }
    }

    /// The byte at `position` if it is ASCII, else 0xFF.
    ///
    /// ## Panics
    ///
    /// If `position` is out of bounds.
    #[inline]
    pub fn ascii_byte_at(self, position: usize) -> u8 {
        match self.bytes[position] {
            ascii_byte @ 0x00..=0x7F => ascii_byte,
            _ => 0xFF,
        }
    }

    /// Iterate over the code points.
    #[inline]
    pub fn code_points(self) -> Wtf8CodePoints<'a> {
        Wtf8CodePoints {
            bytes: self.bytes.iter(),
        }
    }

    /// The string as UTF-8, or `None` if it contains a surrogate.
    #[inline]
    pub fn as_str(self) -> Option<&'a str> {
        core::str::from_utf8(self.bytes).ok()
    }

    /// The string as UTF-8 with surrogates replaced by U+FFFD; copies only
    /// if there is a surrogate.
    pub fn to_string_lossy(self) -> Cow<'a, str> {
        let error = match core::str::from_utf8(self.bytes) {
            Ok(s) => return Cow::Borrowed(s),
            Err(error) => error,
        };
        let wtf8_bytes = self.bytes;
        let surrogate_pos = error.valid_up_to();
        let mut utf8_bytes = Vec::with_capacity(self.bytes.len());
        utf8_bytes.extend_from_slice(&wtf8_bytes[..surrogate_pos]);
        utf8_bytes.extend_from_slice(UTF8_REPLACEMENT_CHARACTER);
        let mut pos = surrogate_pos + 3;
        loop {
            match self.next_surrogate(pos) {
                Some((surrogate_pos, _)) => {
                    utf8_bytes.extend_from_slice(&wtf8_bytes[pos..surrogate_pos]);
                    utf8_bytes.extend_from_slice(UTF8_REPLACEMENT_CHARACTER);
                    pos = surrogate_pos + 3;
                }
                None => {
                    utf8_bytes.extend_from_slice(&wtf8_bytes[pos..]);
                    return Cow::Owned(
                        String::from_utf8(utf8_bytes)
                            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()),
                    );
                }
            }
        }
    }

    /// Iterate over the UTF-16 code units, lone surrogates included.
    #[inline]
    pub fn to_ill_formed_utf16(self) -> IllFormedUtf16CodeUnits<'a> {
        IllFormedUtf16CodeUnits {
            code_points: self.code_points(),
            extra: 0,
        }
    }

    #[inline]
    fn next_surrogate(self, mut pos: usize) -> Option<(usize, u16)> {
        let mut iter = self.bytes.get(pos..)?.iter();
        loop {
            let b = *iter.next()?;
            if b < 0x80 {
                pos += 1;
            } else if b < 0xE0 {
                iter.next();
                pos += 2;
            } else if b == 0xED {
                match (iter.next(), iter.next()) {
                    (Some(&b2), Some(&b3)) if b2 >= 0xA0 => {
                        return Some((pos, decode_surrogate(b2, b3)));
                    }
                    _ => pos += 3,
                }
            } else if b < 0xF0 {
                iter.next();
                iter.next();
                pos += 3;
            } else {
                iter.next();
                iter.next();
                iter.next();
                pos += 4;
            }
        }
    }

    /// The lead surrogate the string ends with, if any.
    #[inline]
    pub fn final_lead_surrogate(self) -> Option<u16> {
        match self.bytes {
            [.., 0xED, b2 @ 0xA0..=0xAF, b3] => Some(decode_surrogate(*b2, *b3)),
            _ => None,
        }
    }

    /// The trail surrogate the string starts with, if any.
    #[inline]
    pub fn initial_trail_surrogate(self) -> Option<u16> {
        match self.bytes {
            [0xED, b2 @ 0xB0..=0xBF, b3, ..] => Some(decode_surrogate(*b2, *b3)),
            _ => None,
        }
    }
}

#[inline]
fn decode_surrogate(second_byte: u8, third_byte: u8) -> u16 {
    0xD800 | ((second_byte as u16 & 0x3F) << 6) | (third_byte as u16 & 0x3F)
}

#[inline]
fn decode_surrogate_pair(lead: u16, trail: u16) -> char {
    let code_point = 0x10000 + (((lead as u32 - 0xD800) << 10) | (trail as u32 - 0xDC00));
    char::from_u32(code_point).expect("a surrogate pair decodes to a supplementary char")
}

const CONT_MASK: u8 = 0b0011_1111;

#[inline]
fn utf8_first_byte(byte: u8, width: u32) -> u32 {
    (byte & (0x7F >> width)) as u32
}

#[inline]
fn utf8_acc_cont_byte(ch: u32, byte: u8) -> u32 {
    (ch << 6) | (byte & CONT_MASK) as u32
}

#[inline]
fn next_code_point(bytes: &mut slice::Iter<u8>) -> Option<u32> {
    let x = match bytes.next() {
        None => return None,
        Some(&next_byte) if next_byte < 128 => return Some(next_byte as u32),
        Some(&next_byte) => next_byte,
    };
    let init = utf8_first_byte(x, 2);
    let y = bytes.next().copied().unwrap_or(0);
    let mut ch = utf8_acc_cont_byte(init, y);
    if x >= 0xE0 {
        let z = bytes.next().copied().unwrap_or(0);
        let y_z = utf8_acc_cont_byte((y & CONT_MASK) as u32, z);
        ch = (init << 12) | y_z;
        if x >= 0xF0 {
            let w = bytes.next().copied().unwrap_or(0);
            ch = ((init & 7) << 18) | utf8_acc_cont_byte(y_z, w);
        }
    }
    Some(ch)
}

/// Iterator over the code points of a WTF-8 string.
#[derive(Clone)]
pub struct Wtf8CodePoints<'a> {
    bytes: slice::Iter<'a, u8>,
}

impl Iterator for Wtf8CodePoints<'_> {
    type Item = CodePoint;

    #[inline]
    fn next(&mut self) -> Option<CodePoint> {
        next_code_point(&mut self.bytes).map(|value| CodePoint {
            value: value.min(0x10FFFF),
        })
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (len, _) = self.bytes.size_hint();
        (len.saturating_add(3) / 4, Some(len))
    }
}

/// Iterator over the UTF-16 code units of a WTF-8 string.
#[derive(Clone)]
pub struct IllFormedUtf16CodeUnits<'a> {
    code_points: Wtf8CodePoints<'a>,
    extra: u16,
}

impl Iterator for IllFormedUtf16CodeUnits<'_> {
    type Item = u16;

    #[inline]
    fn next(&mut self) -> Option<u16> {
        if self.extra != 0 {
            let tmp = self.extra;
            self.extra = 0;
            return Some(tmp);
        }
        self.code_points.next().map(|code_point| {
            let code = code_point.to_u32();
            if code < 0x10000 {
                code as u16
            } else {
                let c = code - 0x1_0000;
                self.extra = 0xDC00 | ((c as u16) & 0x3FF);
                0xD800 | ((c >> 10) as u16)
            }
        })
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (low, high) = self.code_points.size_hint();
        (low, high.and_then(|n| n.checked_mul(2)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surrogates_pair_on_push() {
        let mut s = Wtf8Buf::new();
        s.push(CodePoint::from_u16(0xD83D));
        assert_eq!(s.as_bytes(), &[0xED, 0xA0, 0xBD]);
        assert_eq!(s.as_str(), None);
        assert_eq!(s.to_string_lossy(), "\u{FFFD}");
        s.push(CodePoint::from_u16(0xDCA9));
        assert_eq!(s.as_str(), Some("💩"));
    }

    #[test]
    fn surrogates_pair_on_push_wtf8() {
        let lead = Wtf8Buf::from_ill_formed_utf16(&[0x61, 0xD83D]);
        let trail = Wtf8Buf::from_ill_formed_utf16(&[0xDCA9, 0x62]);
        let mut s = lead.clone();
        s.push_wtf8(&trail);
        assert_eq!(s.as_str(), Some("a💩b"));
        let mut s = trail.clone();
        s.push_wtf8(&lead);
        assert_eq!(s.to_string_lossy(), "\u{FFFD}ba\u{FFFD}");
        assert_eq!(
            s.to_ill_formed_utf16().collect::<Vec<_>>(),
            [0xDCA9, 0x62, 0x61, 0xD83D]
        );
        assert_eq!(s.clone().into_string_lossy(), "\u{FFFD}ba\u{FFFD}");
        assert!(s.into_string().is_err());
    }

    #[test]
    fn code_points_and_utf16() {
        let units = [0x41, 0xD800, 0xD83D, 0xDCA9, 0xDFFF, 0xE9, 0xFFFF];
        let s = Wtf8Buf::from_ill_formed_utf16(&units);
        assert_eq!(s.to_ill_formed_utf16().collect::<Vec<_>>(), units);
        let cps: Vec<u32> = s.code_points().map(|c| c.to_u32()).collect();
        assert_eq!(cps, [0x41, 0xD800, 0x1F4A9, 0xDFFF, 0xE9, 0xFFFF]);
        let rebuilt: Wtf8Buf = s.code_points().collect();
        assert_eq!(rebuilt, s);
        assert_eq!(format!("{s:?}"), "\"A\\u{D800}💩\\u{DFFF}é\u{FFFF}\"");
    }

    #[test]
    #[should_panic]
    fn slice_inside_code_point_panics() {
        Wtf8Str::from_str("é").slice_from(1);
    }

    #[test]
    fn slicing() {
        let s = Wtf8Str::from_str("aé💩b");
        assert_eq!(s.slice(1, 3).as_str(), Some("é"));
        assert_eq!(s.slice_from(3).as_str(), Some("💩b"));
        assert_eq!(s.slice_to(1).as_str(), Some("a"));
        assert_eq!(s.ascii_byte_at(0), b'a');
        assert_eq!(s.ascii_byte_at(1), 0xFF);
    }
}
