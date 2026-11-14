use std::{cmp::Ordering, fmt};

use crate::{MAX_LEN, UniquePromise};

use super::search::SEARCH_BATCH_SIZE;

/// `Source` holds the source text for the lexer, and provides APIs to read it.
///
/// It is a cursor over the source text, stored as a byte offset into it.
/// Source text can be consumed either as `char`s or as bytes.
///
/// Every read is bounds-checked, and every `&str` it hands out is sliced from the source text with
/// `str` indexing, which checks that both ends are on UTF-8 character boundaries.
/// The cursor may temporarily sit inside a multi-byte character while stepping byte by byte,
/// but a `&str` can only be produced between character boundaries: a mistake panics instead of
/// splitting a code point.
pub(super) struct Source<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Source<'a> {
    /// Create `Source` from `&str`.
    ///
    /// If `source_text` is longer than `MAX_LEN`, the lexer reads `"\0"` instead.
    /// The parser checks the length before lexing and reports an error in that case.
    #[expect(unused_variables, clippy::needless_pass_by_value)]
    pub(super) fn new(mut source_text: &'a str, unique: UniquePromise) -> Self {
        if source_text.len() > MAX_LEN {
            source_text = "\0";
        }
        Self { text: source_text, pos: 0 }
    }

    /// Get entire source text as `&str`.
    #[inline]
    pub(super) fn whole(&self) -> &'a str {
        self.text
    }

    /// Get remaining source text as `&str`.
    #[inline]
    pub(super) fn remaining(&self) -> &'a str {
        &self.text[self.pos..]
    }

    #[inline]
    fn bytes(&self) -> &'a [u8] {
        self.text.as_bytes()
    }

    /// Get number of bytes of source text remaining.
    #[inline]
    fn remaining_bytes(&self) -> usize {
        self.text.len() - self.pos
    }

    /// Return position at end of source.
    #[inline]
    pub(super) fn end(&self) -> SourcePosition<'a> {
        SourcePosition { bytes: self.bytes(), offset: self.text.len() }
    }

    /// Get current position.
    #[inline]
    pub(super) fn position(&self) -> SourcePosition<'a> {
        SourcePosition { bytes: self.bytes(), offset: self.pos }
    }

    /// Move current position.
    ///
    /// Panics if `pos` is past the end of source.
    #[inline]
    pub(super) fn set_position(&mut self, pos: SourcePosition<'a>) {
        assert!(pos.offset <= self.text.len());
        self.pos = pos.offset;
    }

    /// Move current position to end of source.
    #[inline]
    pub(super) fn advance_to_end(&mut self) {
        self.pos = self.text.len();
    }

    /// Advance `Source`'s cursor by one byte if it is equal to the given ASCII value.
    #[inline]
    pub(super) fn advance_if_ascii_eq(&mut self, ascii_byte: u8) -> bool {
        debug_assert!(ascii_byte.is_ascii());
        let matched = self.peek_byte() == Some(ascii_byte);
        if matched {
            self.pos += 1;
        }
        matched
    }

    /// Get string slice from a `SourcePosition` up to the current position of `Source`.
    #[inline]
    pub(super) fn str_from_pos_to_current(&self, pos: SourcePosition<'a>) -> &'a str {
        &self.text[pos.offset..self.pos]
    }

    /// Get string slice from current position of `Source` up to a `SourcePosition`.
    #[inline]
    pub(super) fn str_from_current_to_pos(&self, pos: SourcePosition<'a>) -> &'a str {
        &self.text[self.pos..pos.offset]
    }

    /// Get string slice from a `SourcePosition` up to the end of `Source`.
    #[inline]
    pub(super) fn str_from_pos_to_end(&self, pos: SourcePosition<'a>) -> &'a str {
        &self.text[pos.offset..]
    }

    /// Get string slice of source between 2 `SourcePosition`s.
    #[inline]
    pub(super) fn str_between_positions(
        &self,
        start: SourcePosition<'a>,
        end: SourcePosition<'a>,
    ) -> &'a str {
        &self.text[start.offset..end.offset]
    }

    /// Get current position in source, relative to start of source.
    #[expect(clippy::cast_possible_truncation)]
    #[inline]
    pub(super) fn offset(&self) -> u32 {
        self.pos as u32
    }

    /// Get offset of `pos`.
    #[expect(clippy::cast_possible_truncation, clippy::unused_self)]
    #[inline]
    pub(super) fn offset_of(&self, pos: SourcePosition<'a>) -> u32 {
        pos.offset as u32
    }

    /// Move current position back by `n` bytes.
    ///
    /// # Panics
    /// Panics if:
    /// * `n` is 0.
    /// * `n` is greater than current offset in source.
    /// * Moving back `n` bytes would not place current position on a UTF-8 character boundary.
    #[inline]
    pub(super) fn back(&mut self, n: usize) {
        assert!(n > 0, "Cannot call `Source::back` with 0");

        let offset = self.pos;
        assert!(n <= offset, "Cannot go back {n} bytes - only {offset} bytes consumed");

        let new_pos = offset - n;
        assert!(self.text.is_char_boundary(new_pos), "Offset is not on a UTF-8 character boundary");

        self.pos = new_pos;
    }

    /// Get next char of source, and advance position to after it.
    #[inline]
    pub(super) fn next_char(&mut self) -> Option<char> {
        let byte = self.peek_byte()?;
        if byte.is_ascii() {
            self.pos += 1;
            Some(byte as char)
        } else {
            self.next_unicode_char()
        }
    }

    #[cold] // Unicode is rare
    fn next_unicode_char(&mut self) -> Option<char> {
        let c = self.remaining().chars().next()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Get next 2 chars of source, and advance position to after them.
    #[inline]
    pub(super) fn next_2_chars(&mut self) -> Option<[char; 2]> {
        let [byte1, byte2] = self.peek_2_bytes()?;
        if byte1.is_ascii() && byte2.is_ascii() {
            self.pos += 2;
            Some([byte1 as char, byte2 as char])
        } else {
            self.next_2_unicode_chars()
        }
    }

    #[cold] // Unicode is rare
    fn next_2_unicode_chars(&mut self) -> Option<[char; 2]> {
        let mut chars = self.remaining().chars();
        let c1 = chars.next()?;
        let c2 = chars.next()?;
        self.pos += c1.len_utf8() + c2.len_utf8();
        Some([c1, c2])
    }

    /// Get next byte of source, and advance position to after it.
    ///
    /// Panics if at EOF. Callers only use it where they know a byte is there.
    /// The cursor may land inside a multi-byte character; slicing a `&str` there panics.
    #[inline]
    pub(super) fn next_byte(&mut self) -> u8 {
        let byte = self.bytes()[self.pos];
        self.pos += 1;
        byte
    }

    /// Peek next char of source, without consuming it.
    #[inline]
    pub(super) fn peek_char(&self) -> Option<char> {
        let byte = self.peek_byte()?;
        if byte.is_ascii() { Some(byte as char) } else { self.peek_unicode_char() }
    }

    #[cold] // Unicode is rare
    fn peek_unicode_char(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    /// Peek next byte of source without consuming it.
    #[inline]
    pub(super) fn peek_byte(&self) -> Option<u8> {
        self.bytes().get(self.pos).copied()
    }

    /// Peek next two bytes of source without consuming them.
    #[inline]
    pub(super) fn peek_2_bytes(&self) -> Option<[u8; 2]> {
        if self.remaining_bytes() >= 2 { Some(self.position().read2()) } else { None }
    }
}

/// Wrapper around a byte offset into the source text.
///
/// It carries the source bytes, so it can read without a `Source`. Every read is bounds-checked.
/// Positions from different sources must not be compared.
#[derive(Clone, Copy)]
pub struct SourcePosition<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> SourcePosition<'a> {
    /// Create new `SourcePosition` which is `n` bytes after this one.
    #[inline]
    pub(super) fn add(self, n: usize) -> Self {
        Self { bytes: self.bytes, offset: self.offset + n }
    }

    /// Create new `SourcePosition` which is `n` bytes before this one.
    #[inline]
    pub(super) fn sub(self, n: usize) -> Self {
        Self { bytes: self.bytes, offset: self.offset - n }
    }

    /// Get the number of bytes between `origin` and this position.
    ///
    /// Panics if `origin` is after this position.
    #[inline]
    pub(super) fn offset_from(self, origin: Self) -> usize {
        self.offset - origin.offset
    }

    /// Check if this position is at the end of `source`.
    #[inline]
    pub(super) fn is_end_of(self, source: &Source<'a>) -> bool {
        self.offset == source.text.len()
    }

    /// Check if this position is not at the end of `source`.
    #[inline]
    pub(super) fn is_not_end_of(self, source: &Source<'a>) -> bool {
        !self.is_end_of(source)
    }

    /// Check if a batch of `SEARCH_BATCH_SIZE` bytes can be read from this position.
    #[inline]
    pub(super) fn can_read_batch_from(&self, _source: &Source<'a>) -> bool {
        self.offset + SEARCH_BATCH_SIZE <= self.bytes.len()
    }

    /// Read the byte at this position. Panics if at or past the end of source.
    #[inline]
    pub(super) fn read(self) -> u8 {
        self.bytes[self.offset]
    }

    /// Read the 2 bytes from this position. Panics if fewer than 2 bytes remain.
    #[inline]
    pub(super) fn read2(self) -> [u8; 2] {
        match self.bytes.get(self.offset..self.offset + 2) {
            Some(&[a, b]) => [a, b],
            _ => panic!("read2 past the end of source"),
        }
    }

    /// Get a slice of `len` bytes from this position. Panics if fewer than `len` bytes remain.
    #[inline]
    pub(super) fn slice(self, len: usize) -> &'a [u8] {
        &self.bytes[self.offset..self.offset + len]
    }

    /// Get the bytes from this position to the end of source.
    #[inline]
    pub(super) fn rest(self) -> &'a [u8] {
        &self.bytes[self.offset..]
    }
}

impl fmt::Debug for SourcePosition<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SourcePosition").field(&self.offset).finish()
    }
}

impl PartialEq for SourcePosition<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.offset == other.offset
    }
}

impl Eq for SourcePosition<'_> {}

impl Ord for SourcePosition<'_> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.offset.cmp(&other.offset)
    }
}

impl PartialOrd for SourcePosition<'_> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
