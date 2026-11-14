//! Line terminator utilities.
//!
//! ## References
//! - [12.3 Line Terminators](https://tc39.es/ecma262/#sec-line-terminators)
use std::iter::FusedIterator;

/// U+000A LINE FEED, abbreviated in the spec as `<LF>`.
pub const LF: char = '\u{a}';

/// U+000D CARRIAGE RETURN, abbreviated in the spec as `<CR>`.
pub const CR: char = '\u{d}';

/// U+2028 LINE SEPARATOR, abbreviated `<LS>`.
pub const LS: char = '\u{2028}';

/// U+2029 PARAGRAPH SEPARATOR, abbreviated `<PS>`.
pub const PS: char = '\u{2029}';

/// Checks if the character is a regular line terminator (`LF` or `CR`).
pub fn is_regular_line_terminator(c: char) -> bool {
    matches!(c, LF | CR)
}

/// Checks if the character is an irregular line terminator (`LS` or `PS`).
pub fn is_irregular_line_terminator(c: char) -> bool {
    matches!(c, LS | PS)
}

/// Checks if the character is any line terminator (`LF`, `CR`, `LS`, or `PS`).
pub fn is_line_terminator(c: char) -> bool {
    is_regular_line_terminator(c) || is_irregular_line_terminator(c)
}

/// Convert `char` to UTF-8 bytes array.
const fn to_bytes<const N: usize>(ch: char) -> [u8; N] {
    assert!(ch.len_utf8() == N);
    let mut bytes = [0u8; N];
    ch.encode_utf8(&mut bytes);
    bytes
}

/// `LS` character as UTF-8 bytes.
const LS_BYTES: [u8; 3] = to_bytes(LS);
/// `PS` character as UTF-8 bytes.
const PS_BYTES: [u8; 3] = to_bytes(PS);

/// First byte of either `LS` or `PS`
pub const LS_OR_PS_FIRST_BYTE: u8 = 0xE2;

/// Last 2 bytes of `LS` character.
pub const LS_LAST_2_BYTES: [u8; 2] = [LS_BYTES[1], LS_BYTES[2]];
/// Last 2 bytes of `PS` character.
pub const PS_LAST_2_BYTES: [u8; 2] = [PS_BYTES[1], PS_BYTES[2]];

/// Custom iterator that splits text on line terminators while handling CRLF as a single unit.
/// This avoids creating empty strings between CR and LF characters.
///
/// Also splits on irregular line breaks (LS and PS).
///
/// # Example
/// Standard split would turn `"line1\r\nline2"` into `["line1", "", "line2"]` because
/// it treats `\r` and `\n` as separate terminators. This iterator correctly produces
/// `["line1", "line2"]` by treating `\r\n` as a single terminator.
pub struct LineTerminatorSplitter<'a> {
    text: &'a str,
}

impl<'a> LineTerminatorSplitter<'a> {
    /// Creates a new `LineTerminatorSplitter` for the given string.
    pub fn new(text: &'a str) -> Self {
        Self { text }
    }
}

impl<'a> Iterator for LineTerminatorSplitter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.text.is_empty() {
            return None;
        }

        for (index, &byte) in self.text.as_bytes().iter().enumerate() {
            match byte {
                b'\n' => {
                    let line = &self.text[..index];
                    self.text = &self.text[index + 1..];
                    return Some(line);
                }
                b'\r' => {
                    let line = &self.text[..index];
                    // If the next byte is `\n`, consume it as well
                    let skip_bytes =
                        if self.text.as_bytes().get(index + 1) == Some(&b'\n') { 2 } else { 1 };
                    self.text = &self.text[index + skip_bytes..];
                    return Some(line);
                }
                LS_OR_PS_FIRST_BYTE => {
                    let next2: [u8; 2] = {
                        let next2 = &self.text.as_bytes()[index + 1..index + 3];
                        next2.try_into().unwrap()
                    };
                    // If this is LS or PS, treat it as a line terminator
                    if matches!(next2, LS_LAST_2_BYTES | PS_LAST_2_BYTES) {
                        let line = &self.text[..index];
                        self.text = &self.text[index + 3..];
                        return Some(line);
                    }
                }
                _ => {}
            }
        }

        // No line break found - return the remaining text. Next call will return `None`.
        let line = self.text;
        self.text = "";
        Some(line)
    }
}

impl FusedIterator for LineTerminatorSplitter<'_> {}
