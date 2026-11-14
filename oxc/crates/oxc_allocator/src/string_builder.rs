use std::{
    fmt::{self, Debug, Display, Write},
    ops::{Deref, DerefMut},
};

use crate::Allocator;

/// A string being built; [`StringBuilder::into_str`] moves it into the
/// allocator's string arena.
pub struct StringBuilder<'a> {
    string: String,
    allocator: &'a Allocator,
}

impl<'a> StringBuilder<'a> {
    #[inline]
    pub fn new_in(allocator: &'a Allocator) -> Self {
        Self { string: String::new(), allocator }
    }

    #[inline]
    pub fn with_capacity_in(capacity: usize, allocator: &'a Allocator) -> Self {
        Self { string: String::with_capacity(capacity), allocator }
    }

    #[inline]
    pub fn from_str_in(s: &str, allocator: &'a Allocator) -> Self {
        Self { string: String::from(s), allocator }
    }

    #[inline]
    pub fn from_strs_array_in<const N: usize>(strings: [&str; N], allocator: &'a Allocator) -> Self {
        Self { string: strings.concat(), allocator }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.string.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.string.is_empty()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.string.capacity()
    }

    #[inline]
    pub fn into_str(self) -> &'a str {
        self.allocator.alloc_string(self.string)
    }

    #[inline]
    pub fn push_ascii_byte(&mut self, byte: u8) {
        assert!(byte.is_ascii(), "byte is not ASCII");
        self.string.push(byte as char);
    }

    #[inline]
    pub fn push_ascii_byte_start(&mut self, byte: u8) {
        assert!(byte.is_ascii(), "byte is not ASCII");
        self.string.insert(0, byte as char);
    }

    #[inline]
    pub fn push(&mut self, c: char) {
        self.string.push(c);
    }

    #[inline]
    pub fn push_str(&mut self, s: &str) {
        self.string.push_str(s);
    }

    pub fn push_ascii_byte_repeat(&mut self, byte: u8, n: usize) {
        assert!(byte.is_ascii(), "byte is not ASCII");
        self.string.extend(std::iter::repeat_n(byte as char, n));
    }

    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.string.reserve(additional);
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.string
    }

    #[inline]
    pub fn as_mut_str(&mut self) -> &mut str {
        &mut self.string
    }
}

impl Deref for StringBuilder<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        &self.string
    }
}

impl DerefMut for StringBuilder<'_> {
    fn deref_mut(&mut self) -> &mut str {
        &mut self.string
    }
}

impl PartialEq for StringBuilder<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.string == other.string
    }
}

impl Eq for StringBuilder<'_> {}

impl PartialEq<str> for StringBuilder<'_> {
    fn eq(&self, other: &str) -> bool {
        self.string == other
    }
}

impl PartialEq<StringBuilder<'_>> for str {
    fn eq(&self, other: &StringBuilder<'_>) -> bool {
        self == other.string
    }
}

impl PartialEq<&str> for StringBuilder<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.string == *other
    }
}

impl PartialEq<StringBuilder<'_>> for &str {
    fn eq(&self, other: &StringBuilder<'_>) -> bool {
        *self == other.string
    }
}

impl Write for StringBuilder<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_str(s);
        Ok(())
    }

    fn write_char(&mut self, c: char) -> fmt::Result {
        self.push(c);
        Ok(())
    }
}

impl Display for StringBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_str(), f)
    }
}

impl Debug for StringBuilder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.as_str(), f)
    }
}
