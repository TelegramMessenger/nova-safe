// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use core::{cell::OnceCell, hash::Hash, num::NonZeroUsize};
use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use wtf8::{CodePoint, Wtf8Buf, Wtf8Str};

use crate::{
    hashing::{RandomState, TEXT_BLOCK, TextHashState},
    heap::{CompactionLists, HeapMarkAndSweep, WorkQueues},
};

#[derive(Debug, Clone)]
pub(crate) struct StringRecord {
    pub(crate) data: StringBuffer,
    pub(crate) mapping: OnceCell<IndexMapping>,
}

impl PartialEq for StringRecord {
    fn eq(&self, other: &Self) -> bool {
        // If both strings are static, we can compare their pointers directly.
        if let (&StringBuffer::Static(self_str), &StringBuffer::Static(other_str)) =
            (&self.data, &other.data)
            && core::ptr::eq(self_str.as_bytes(), other_str.as_bytes())
        {
            return true;
        }
        self.len() == other.len() && self.as_wtf8() == other.as_wtf8()
    }
}
impl Eq for StringRecord {}

#[derive(Debug, Clone)]
pub(crate) enum IndexMapping {
    Ascii,
    NonAscii {
        /// Mapping from UTF-16 indices to indices in the UTF-8 representation.
        /// When the UTF-16 character would be the second character in a
        /// surrogate pair, it maps to None because there is no corresponding
        /// UTF-8 index.
        mapping: Box<[Option<NonZeroUsize>]>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum StringBuffer {
    Owned(Wtf8Buf),
    Static(Wtf8Str<'static>),
    /// A long string that a concatenation made, as one buffer. A script that
    /// appends to it again is building text, and what it gets then is
    /// [`StringBuffer::Appended`]; a concatenation that is used as it is pays
    /// for neither pieces nor joining.
    Concatenated(Wtf8Buf),
    /// A concatenated string with more text appended: see [`AppendedText`].
    Appended(Box<AppendedText>),
}

impl Hash for StringBuffer {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        match self {
            StringBuffer::Owned(wtf8_buf) | StringBuffer::Concatenated(wtf8_buf) => {
                wtf8_buf.hash(state)
            }
            StringBuffer::Static(wtf8) => wtf8.hash(state),
            StringBuffer::Appended(text) => text.joined().hash(state),
        }
    }
}

/// The text of a string that was made by appending to another one, kept as
/// the pieces it was appended from. A loop that builds a string by `s += x`
/// makes a new string at every step; were each one a copy of the last, the
/// loop would copy the whole text at every step. Here the new string shares
/// the old one's pieces and adds its own, and its bytes are joined into one
/// buffer the first time something reads them.
///
/// Each piece is at least as long as all the pieces after it together would
/// need to stay shorter: appending merges the new piece into the one before
/// it while that one is not longer, so sizes at least double going back, a
/// text has few pieces, and a byte is copied a logarithmic number of times.
#[derive(Debug, Clone)]
pub(crate) struct AppendedText {
    pieces: Arc<Piece>,
    /// Bytes of the whole text.
    len: usize,
    utf16_len: usize,
    /// The string table's hash of the text, after its whole blocks.
    hash_state: TextHashState,
    /// Bytes of pieces that making this text allocated.
    fresh_bytes: usize,
    joined: OnceCell<Wtf8Buf>,
    budget: Arc<JoinBudget>,
}

/// What the heap's appended strings may join. Joining a text allocates all of
/// it where no allocation counter is at hand and nothing can be refused, and
/// a script can hold many long appended strings for the price of their
/// shared pieces; so the bytes joined since the strings were last measured
/// are counted here, and past `cap` (the heap limit) a join does not
/// happen: it unwinds with [`TextJoinOverLimit`], which ends the engine as
/// out of memory. Up to the cap, the next poll or collection sees the count.
#[derive(Debug, Default)]
pub(crate) struct JoinBudget {
    joined: AtomicUsize,
    /// 0: no cap.
    cap: AtomicUsize,
}

impl JoinBudget {
    /// Bytes joined since [`JoinBudget::settled`].
    pub(crate) fn joined(&self) -> usize {
        self.joined.load(Ordering::Relaxed)
    }

    /// The strings were measured: joined texts are counted as what they are.
    pub(crate) fn settled(&self) {
        self.joined.store(0, Ordering::Relaxed);
    }

    pub(crate) fn set_cap(&self, cap: Option<usize>) {
        self.cap.store(cap.unwrap_or(0), Ordering::Relaxed);
    }
}

/// The payload a join past the heap limit unwinds with ([`JoinBudget`]): an
/// embedder that catches unwinds reports it as out of memory. The Agent must
/// not run again after it.
#[derive(Debug)]
pub struct TextJoinOverLimit;

#[derive(Debug)]
struct Piece {
    bytes: Box<[u8]>,
    before: Option<Arc<Piece>>,
}

impl Piece {
    /// The pieces of a text that ends with `bytes` after `before`, and how
    /// many bytes that allocated.
    fn appended(mut before: Option<Arc<Piece>>, mut bytes: Vec<u8>) -> (Arc<Piece>, usize) {
        let mut allocated = bytes.len() + core::mem::size_of::<Piece>();
        while let Some(previous) = &before
            && previous.bytes.len() <= bytes.len()
        {
            let mut merged = Vec::with_capacity(previous.bytes.len() + bytes.len());
            merged.extend_from_slice(&previous.bytes);
            merged.extend_from_slice(&bytes);
            allocated += merged.len();
            bytes = merged;
            before = previous.before.clone();
        }
        let piece = Arc::new(Piece {
            bytes: bytes.into_boxed_slice(),
            before,
        });
        (piece, allocated)
    }
}

/// The UTF-16 code units of well-formed WTF-8: one for every code point, and
/// one more for each that takes four bytes.
fn utf16_len_of(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .map(|&byte| usize::from(byte & 0xC0 != 0x80) + usize::from(byte >= 0xF0))
        .sum()
}

impl AppendedText {
    /// The text as one buffer, joined on the first call.
    fn joined(&self) -> Wtf8Str<'_> {
        self.joined
            .get_or_init(|| {
                let joined = self.budget.joined.fetch_add(self.len, Ordering::Relaxed);
                let cap = self.budget.cap.load(Ordering::Relaxed);
                if cap != 0 && joined.saturating_add(self.len) > cap {
                    // Without the panic hook: this is the heap limit at
                    // work, not a fault to report.
                    std::panic::resume_unwind(Box::new(TextJoinOverLimit));
                }
                let mut pieces = Vec::new();
                let mut piece = Some(&self.pieces);
                while let Some(current) = piece {
                    pieces.push(&current.bytes);
                    piece = current.before.as_ref();
                }
                let mut text = Wtf8Buf::with_capacity(self.len);
                for bytes in pieces.into_iter().rev() {
                    text.push_wtf8(Wtf8Str::from_wtf8_bytes(bytes));
                }
                text
            })
            .as_wtf8()
    }

    /// The bytes after the hashed blocks of the text: its last `count`,
    /// fewer than [`TEXT_BLOCK`].
    fn tail(&self) -> ([u8; TEXT_BLOCK], usize) {
        let count = (self.len - self.hash_state.hashed()).min(TEXT_BLOCK - 1);
        let mut tail = [0u8; TEXT_BLOCK];
        let mut missing = count;
        let mut piece = Some(&self.pieces);
        while let Some(current) = piece
            && missing > 0
        {
            let take = missing.min(current.bytes.len());
            tail[missing - take..missing]
                .copy_from_slice(&current.bytes[current.bytes.len() - take..]);
            missing -= take;
            piece = current.before.as_ref();
        }
        (tail, count)
    }
}

impl StringRecord {
    /// The maximum UTf-16 length of a JS string, according to the spec (2^53 - 1);
    /// on a 32-bit target, what a `usize` can hold.
    const MAX_UTF16_LENGTH: usize = {
        let spec: u64 = (1 << 53) - 1;
        if spec > usize::MAX as u64 {
            usize::MAX
        } else {
            spec as usize
        }
    };

    /// The maximum UTF-8 length of a JS string.
    const MAX_UTF8_LENGTH: usize = Self::MAX_UTF16_LENGTH.saturating_mul(3);

    /// The string's hash in the heap's string table.
    pub(crate) fn table_hash(&self, hasher: &RandomState) -> u64 {
        match &self.data {
            StringBuffer::Appended(text) => {
                let (tail, count) = text.tail();
                RandomState::hash_text_from_state(&text.hash_state, &tail[..count])
            }
            _ => hasher.hash_text(self.as_bytes()),
        }
    }

    /// Whether appending to the string continues a text that is being built
    /// ([`StringRecord::appended`]): a concatenation made it.
    pub(crate) fn is_being_built(&self) -> bool {
        matches!(
            self.data,
            StringBuffer::Concatenated(_) | StringBuffer::Appended(_)
        )
    }

    /// A long string that a concatenation made.
    pub(crate) fn from_concatenation(text: Wtf8Buf) -> Self {
        debug_assert!(text.len() > 7);
        assert!(text.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Concatenated(text),
            mapping: OnceCell::new(),
        }
    }

    /// `left` with `right` appended (no surrogate pair joined across the
    /// two) and its hash in the string table. Nothing is joined: the new text
    /// shares the pieces of `left`, or starts them with a copy of it.
    pub(crate) fn appended(
        left: &StringRecord,
        right: Vec<u8>,
        hasher: &RandomState,
        budget: &Arc<JoinBudget>,
    ) -> (StringRecord, u64) {
        let len = left.len() + right.len();
        assert!(len <= Self::MAX_UTF8_LENGTH, "String is too long.");
        // Counted from the bytes: the index table of a long text is larger
        // than the text, and only reading by index needs it.
        let left_utf16_len = match (&left.data, left.mapping.get()) {
            (StringBuffer::Appended(text), _) => text.utf16_len,
            (_, Some(_)) => left.utf16_len(),
            _ => utf16_len_of(left.as_bytes()),
        };
        let utf16_len = left_utf16_len + utf16_len_of(&right);
        let (pieces, hash, hash_state, fresh_bytes) = match &left.data {
            StringBuffer::Appended(text) => {
                let (tail, count) = text.tail();
                let (hash, hash_state) =
                    RandomState::hash_text_appended(&text.hash_state, &tail[..count], &right);
                let (pieces, fresh_bytes) = Piece::appended(Some(text.pieces.clone()), right);
                (pieces, hash, hash_state, fresh_bytes)
            }
            _ => {
                let bytes = left.as_bytes();
                let (_, left_state) = hasher.hash_text_keeping_state(bytes);
                let (hash, hash_state) = RandomState::hash_text_appended(
                    &left_state,
                    &bytes[left_state.hashed()..],
                    &right,
                );
                let (first, first_bytes) = Piece::appended(None, bytes.to_vec());
                let (pieces, fresh_bytes) = Piece::appended(Some(first), right);
                (pieces, hash, hash_state, first_bytes + fresh_bytes)
            }
        };
        let data = StringBuffer::Appended(Box::new(AppendedText {
            pieces,
            len,
            utf16_len,
            hash_state,
            fresh_bytes,
            joined: OnceCell::new(),
            budget: budget.clone(),
        }));
        (
            StringRecord {
                data,
                mapping: OnceCell::new(),
            },
            hash,
        )
    }

    /// What making the record allocated: for an appended string its own
    /// pieces, not the text it shares, so that a loop that appends is not
    /// charged the whole text at every step.
    pub(crate) fn allocated_bytes(&self) -> usize {
        match &self.data {
            StringBuffer::Appended(text) => text.fresh_bytes + core::mem::size_of::<AppendedText>(),
            _ => self.len(),
        }
    }

    /// What a string that lives on counts for against the heap limit: all of
    /// its text, also where an appended string shares it with others. The
    /// pieces make appending fast, not free: every appended string can be
    /// read, and reading joins its whole text, so the strings a script may
    /// keep are those whose joined texts fit the limit together.
    pub(crate) fn counted_bytes(&self) -> usize {
        match &self.data {
            StringBuffer::Appended(text) => {
                let joined = if text.joined.get().is_some() {
                    text.len
                } else {
                    0
                };
                text.len + joined + core::mem::size_of::<AppendedText>()
            }
            _ => self.len(),
        }
    }

    /// Called by the collector for a string that lives on: an appended text
    /// that was joined becomes that one buffer and lets go of its pieces.
    fn settle(&mut self) {
        if let StringBuffer::Appended(text) = &mut self.data
            && let Some(joined) = text.joined.take()
        {
            self.data = StringBuffer::Concatenated(joined);
        }
    }

    /// Get the byte length of the string.
    pub(crate) fn len(&self) -> usize {
        match &self.data {
            StringBuffer::Owned(buf) | StringBuffer::Concatenated(buf) => buf.len(),
            StringBuffer::Static(buf) => buf.len(),
            StringBuffer::Appended(text) => text.len,
        }
    }

    fn index_mapping(&self) -> &IndexMapping {
        self.mapping.get_or_init(|| {
            fn is_surrogate_pair(cp: CodePoint) -> bool {
                let code = cp.to_u32();
                (code & !0xFFFF) > 0
            }

            let Some((mut idx, _)) = self
                .as_bytes()
                .iter()
                .enumerate()
                .find(|(_, ch)| !ch.is_ascii())
            else {
                return IndexMapping::Ascii;
            };

            // All indices less than `idx` map to ASCII bytes, so all UTF-16
            // indices less *or equal* than `idx` map to that same UTF-8 index
            let mut mapping: Vec<Option<NonZeroUsize>> = (0..=idx).map(NonZeroUsize::new).collect();

            let mut buf = [0u8; 4];
            let mut iter = self.as_wtf8().slice_from(idx).code_points();

            let ch = iter.next().expect("non-ASCII byte starts a code point");

            if is_surrogate_pair(ch) {
                mapping.push(None);
            }

            if let Some(ch) = ch.to_char() {
                idx += ch.encode_utf8(&mut buf).len();
            } else {
                // Lone surrogate; these always take 3 bytes in WTF-8.
                idx += 3;
            }

            assert_ne!(idx, 0);
            for ch in iter {
                mapping.push(NonZeroUsize::new(idx));
                if is_surrogate_pair(ch) {
                    mapping.push(None);
                }
                if let Some(ch) = ch.to_char() {
                    idx += ch.encode_utf8(&mut buf).len();
                } else {
                    // Lone surrogate; these always take 3 bytes in WTF-8.
                    idx += 3;
                }
            }

            assert!(
                mapping.len() <= Self::MAX_UTF16_LENGTH,
                "String is too long."
            );

            IndexMapping::NonAscii {
                mapping: mapping.into_boxed_slice(),
            }
        })
    }

    /// Get the WTF-16 length of the string.
    pub(crate) fn utf16_len(&self) -> usize {
        // Known without joining the text, so that a loop can ask a string it
        // is building for its length.
        if let StringBuffer::Appended(text) = &self.data {
            return text.utf16_len;
        }
        match self.index_mapping() {
            IndexMapping::Ascii => self.len(),
            IndexMapping::NonAscii { mapping } => mapping.len(),
        }
    }

    /// Get the WTF-16 code unit at a given WTF-16 code unit index.
    ///
    /// This is equivalent to the JavaScript `string.charCodeAt(idx)` method.
    pub(crate) fn char_code_at(&self, idx: usize) -> CodePoint {
        let (utf8_idx, take_latter_half): (usize, bool) = if idx != 0 {
            match self.index_mapping() {
                IndexMapping::Ascii => {
                    return CodePoint::from_char(char::from(self.as_wtf8().ascii_byte_at(idx)));
                }
                IndexMapping::NonAscii { mapping } => {
                    match mapping[idx] {
                        Some(idx) => (idx.into(), false),
                        None => {
                            // We got a None; that means we're looking at a latter
                            // surrogate here.
                            let idx = idx - 1;
                            let idx = if idx == 0 {
                                0
                            } else {
                                mapping[idx].unwrap().into()
                            };
                            (idx, true)
                        }
                    }
                }
            }
        } else {
            (0, false)
        };
        let char = self
            .as_wtf8()
            .slice_from(utf8_idx)
            .code_points()
            .next()
            .unwrap();
        let code = char.to_u32();
        if (code & 0xFFFF) == code {
            // Single-char character.
            return char;
        }
        let char = char
            .to_char()
            .expect("Surrogate pair did not map to a char");
        let encoded = &mut [0; 2];
        let enc = char.encode_utf16(encoded);
        // Note: since this is a surrogate pair, it should always
        // encode into two u16s.
        debug_assert_eq!(enc.len(), 2);
        let surrogate = encoded[if take_latter_half { 1 } else { 0 }];
        CodePoint::from_u32(u32::from(surrogate)).expect("a u16 is a code point")
    }

    pub(crate) fn utf8_index(&self, utf16_idx: usize) -> Option<usize> {
        if utf16_idx == 0 {
            Some(0)
        } else {
            match self.index_mapping() {
                IndexMapping::Ascii => {
                    assert!(utf16_idx <= self.len());
                    Some(utf16_idx)
                }
                IndexMapping::NonAscii { mapping } => {
                    if utf16_idx == mapping.len() {
                        Some(self.len())
                    } else {
                        mapping[utf16_idx].map(NonZeroUsize::get)
                    }
                }
            }
        }
    }

    pub(crate) fn utf16_index(&self, utf8_idx: usize) -> usize {
        if utf8_idx == 0 {
            0
        } else {
            assert!(utf8_idx <= self.len());
            match self.index_mapping() {
                IndexMapping::Ascii => utf8_idx,
                IndexMapping::NonAscii { mapping } => {
                    if utf8_idx == self.len() {
                        return mapping.len();
                    }

                    // Binary search `mapping`. We start at `utf8_idx` though,
                    // if it's in range.
                    let mut range = if utf8_idx >= mapping.len() {
                        0..mapping.len()
                    } else {
                        let mut pivot = utf8_idx;
                        if mapping[pivot].is_none() {
                            pivot -= 1;
                        }
                        match mapping[pivot].unwrap().get().cmp(&utf8_idx) {
                            core::cmp::Ordering::Less => pivot..mapping.len(),
                            core::cmp::Ordering::Equal => return pivot,
                            core::cmp::Ordering::Greater => 0..pivot,
                        }
                    };

                    loop {
                        let mut pivot = (range.start + range.end) / 2;
                        if mapping[pivot].is_none() {
                            pivot -= 1;
                        }
                        debug_assert!(range.contains(&pivot));

                        // Since we're adjusting the pivot due to None elements
                        // (i.e. surrogates), we might get stuck in an infinite
                        // loop if pivot is the start of the range. In this
                        // case, we walk through the range we know is valid.
                        if pivot == range.start {
                            for i in range {
                                if mapping[i].is_some() && mapping[i].unwrap().get() == utf8_idx {
                                    return i;
                                }
                            }
                            unreachable!();
                        }

                        let new_range = match mapping[pivot].unwrap().get().cmp(&utf8_idx) {
                            core::cmp::Ordering::Less => pivot..range.end,
                            core::cmp::Ordering::Equal => return pivot,
                            core::cmp::Ordering::Greater => range.start..pivot,
                        };
                        assert_ne!(range, new_range);

                        range = new_range;
                    }
                }
            }
        }
    }

    /// Get the CodePoint at a given WTF-16 code unit index.
    ///
    /// This is equivalent to the JavaScript `string.codePointAt(idx)` method.
    pub(crate) fn code_point_at(&self, utf16_idx: usize) -> CodePoint {
        assert!(utf16_idx <= self.utf16_len());
        let mapping = match self.index_mapping() {
            IndexMapping::Ascii => {
                return CodePoint::from_char(char::from(self.as_wtf8().ascii_byte_at(utf16_idx)));
            }
            IndexMapping::NonAscii { mapping } => mapping,
        };
        if utf16_idx == 0 {
            let char = self.as_wtf8().code_points().next().unwrap();
            return char;
        }
        match mapping[utf16_idx] {
            Some(wtf8_idx) => {
                let wtf8_index: usize = wtf8_idx.into();
                self.as_wtf8()
                    .slice_from(wtf8_index)
                    .code_points()
                    .next()
                    .unwrap()
            }
            None => {
                // Matched None; this is the second character in a surrogate pair.
                // Index 0 is stored as None too: the pair starts the string.
                let wtf8_index: usize = mapping[utf16_idx - 1].map_or(0, NonZeroUsize::get);
                let char = self
                    .as_wtf8()
                    .slice_from(wtf8_index)
                    .code_points()
                    .next()
                    .unwrap();
                let char = char
                    .to_char()
                    .expect("Surrogate pair did not map to a char");
                let encoded = &mut [0; 2];
                let encoded = char.encode_utf16(encoded);
                // Note: since this is a surrogate pair, it should always
                // encode into two u16s.
                debug_assert_eq!(encoded.len(), 2);
                let surrogate = encoded[1];
                CodePoint::from_u32(u32::from(surrogate)).expect("a u16 is a code point")
            }
        }
    }

    /// Lossily convert the string to UTF-8.
    /// Return an UTF-8 `&str` slice if the contents are well-formed in UTF-8.
    ///
    /// Surrogates are replaced with `"\u{FFFD}"` (the replacement character “�”).
    ///
    /// This only copies the data if necessary (if it contains any surrogate).
    pub(crate) fn to_string_lossy(&self) -> Cow<'_, str> {
        self.as_wtf8().to_string_lossy()
    }

    /// Try to convert the string to UTF-8 and return a `&str` slice.
    ///
    /// Return `None` if the string contains surrogates.
    ///
    /// This does not copy the data.
    #[inline]
    pub(crate) fn as_str(&self) -> Option<&str> {
        self.as_wtf8().as_str()
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.as_wtf8().as_bytes()
    }

    pub(crate) fn as_wtf8(&self) -> Wtf8Str<'_> {
        match &self.data {
            StringBuffer::Owned(buf) | StringBuffer::Concatenated(buf) => buf.as_wtf8(),
            StringBuffer::Static(buf) => *buf,
            StringBuffer::Appended(text) => text.joined(),
        }
    }

    pub(crate) fn from_str(str: &str) -> Self {
        debug_assert!(str.len() > 7);
        assert!(str.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Owned(Wtf8Buf::from_str(str)),
            mapping: OnceCell::new(),
        }
    }

    pub(crate) fn from_static_str(str: &'static str) -> Self {
        debug_assert!(str.len() > 7);
        assert!(str.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Static(Wtf8Str::from_str(str)),
            mapping: OnceCell::new(),
        }
    }

    pub(crate) fn from_string(str: String) -> Self {
        debug_assert!(str.len() > 7);
        assert!(str.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Owned(Wtf8Buf::from_string(str)),
            mapping: OnceCell::new(),
        }
    }

    pub(crate) fn from_wtf8_buf(str: Wtf8Buf) -> Self {
        debug_assert!(str.len() > 7);
        assert!(str.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Owned(str),
            mapping: OnceCell::new(),
        }
    }
}

impl HeapMarkAndSweep for StringRecord {
    fn mark_values(&self, _queues: &mut WorkQueues) {
        let Self {
            data: _,
            mapping: _,
        } = self;
    }

    fn sweep_values(&mut self, _compactions: &CompactionLists) {
        let Self {
            data: _,
            mapping: _,
        } = self;
        self.settle();
    }
}
