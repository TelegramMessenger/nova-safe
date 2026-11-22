// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use core::{cell::OnceCell, hash::Hash, num::NonZeroUsize};
use std::{
    borrow::Cow,
    collections::VecDeque,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use wtf8::{CodePoint, Wtf8Buf, Wtf8Str};

use crate::{
    hashing::{TextHasher, multiply_text},
    heap::{CompactionLists, HeapMarkAndSweep, WorkQueues},
};

#[derive(Debug, Clone)]
pub(crate) struct StringRecord {
    pub(crate) data: StringBuffer,
    pub(crate) mapping: OnceCell<IndexMapping>,
    /// The value of the text ([`TextHasher::text`]), kept so that the string
    /// table and a concatenation need not read the text for it; or
    /// [`UNHASHED`].
    hash: u64,
}

/// In place of the value of a text that is not in the string table
/// ([`StringRecord::unlisted`]). No text has it: values are under 2^61.
const UNHASHED: u64 = u64::MAX;

impl PartialEq for StringRecord {
    fn eq(&self, other: &Self) -> bool {
        // If both strings are static, we can compare their pointers directly.
        if let (&StringBuffer::Static(self_str), &StringBuffer::Static(other_str)) =
            (&self.data, &other.data)
            && core::ptr::eq(self_str.as_bytes(), other_str.as_bytes())
        {
            return true;
        }
        self.len() == other.len() && self.same_text(other)
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
    /// [`StringBuffer::Built`]; a concatenation that is used as it is pays
    /// for sharing growing storage.
    Concatenated(Wtf8Buf),
    /// A range of the buffer that holds successive versions of a growing string.
    Built(Box<BuiltText>),
}

impl Hash for StringBuffer {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        match self {
            StringBuffer::Owned(wtf8_buf) | StringBuffer::Concatenated(wtf8_buf) => {
                wtf8_buf.hash(state)
            }
            StringBuffer::Static(wtf8) => wtf8.hash(state),
            StringBuffer::Built(text) => text.as_wtf8().hash(state),
        }
    }
}

/// Versions of a growing string share one contiguous buffer. A view handed
/// to a reader freezes the buffer; further growth starts a new one.
#[derive(Debug, Clone)]
pub(crate) struct BuiltText {
    buffer: Arc<TextBuffer>,
    start: i64,
    len: usize,
    utf16_len: usize,
    power: u64,
}

#[derive(Default, Debug)]
pub(crate) struct TextBufferBytes {
    pub(crate) live: AtomicUsize,
    pub(crate) allocated: AtomicUsize,
    pub(crate) limit: AtomicUsize,
}

impl TextBufferBytes {
    fn capacity(&self, len: usize, needed: usize) -> usize {
        let cap = self.limit.load(Ordering::Relaxed);
        let cap = if cap == 0 {
            StringRecord::MAX_UTF8_LENGTH
        } else {
            cap.min(StringRecord::MAX_UTF8_LENGTH)
        };
        needed.max(len.saturating_add((len / 4).max(64)).min(cap))
    }
}

#[derive(Debug)]
struct TextBuffer {
    growing: Mutex<Option<TextBytes>>,
    frozen: OnceLock<TextBytes>,
    accounting: Arc<TextBufferBytes>,
}

#[derive(Default, Debug)]
struct TextBytes {
    first: i64,
    bytes: VecDeque<u8>,
}

impl TextBytes {
    fn range(&self, start: i64, len: usize) -> std::ops::Range<usize> {
        let index = usize::try_from(start - self.first).unwrap();
        index..index + len
    }

    fn add(&mut self, bytes: &[u8], after: bool, capacity: usize) {
        let len = self.bytes.len() + bytes.len();
        if len > self.bytes.capacity() {
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        if after {
            self.bytes.extend(bytes);
        } else {
            self.first -= bytes.len() as i64;
            for &byte in bytes.iter().rev() {
                self.bytes.push_front(byte);
            }
        }
    }
}

impl TextBuffer {
    const OVERHEAD: usize = core::mem::size_of::<Self>() + 2 * core::mem::size_of::<usize>();

    fn account(&self, bytes: usize) {
        self.accounting.live.fetch_add(bytes, Ordering::Relaxed);
        self.accounting
            .allocated
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |total| {
                Some(total.saturating_add(bytes))
            })
            .unwrap();
    }

    fn with_bytes<T>(&self, f: impl FnOnce(&[u8], i64) -> T) -> T {
        if let Some(text) = self.frozen.get() {
            return f(text.bytes.as_slices().0, text.first);
        }
        let mut growing = self.growing.lock().unwrap();
        if let Some(text) = growing.as_mut() {
            let first = text.first;
            return f(text.bytes.make_contiguous(), first);
        }
        let text = self.frozen.get().unwrap();
        f(text.bytes.as_slices().0, text.first)
    }

    fn frozen(&self) -> &TextBytes {
        if let Some(frozen) = self.frozen.get() {
            return frozen;
        }
        let mut growing = self.growing.lock().unwrap();
        if let Some(mut text) = growing.take() {
            text.bytes.make_contiguous();
            self.frozen.set(text).unwrap();
        }
        self.frozen.get().unwrap()
    }
}

impl Drop for TextBuffer {
    fn drop(&mut self) {
        let text = self
            .frozen
            .get()
            .or_else(|| self.growing.get_mut().unwrap().as_ref())
            .unwrap();
        self.accounting
            .live
            .fetch_sub(text.bytes.capacity() + Self::OVERHEAD, Ordering::Relaxed);
    }
}

impl BuiltText {
    fn as_wtf8(&self) -> Wtf8Str<'_> {
        let text = self.buffer.frozen();
        Wtf8Str::from_wtf8_bytes(&text.bytes.as_slices().0[text.range(self.start, self.len)])
    }

    fn with_bytes<T>(&self, f: impl FnOnce(&[u8]) -> T) -> T {
        self.buffer.with_bytes(|bytes, first| {
            let start = usize::try_from(self.start - first).unwrap();
            f(&bytes[start..start + self.len])
        })
    }

    fn is_text(&self, bytes: &[u8]) -> bool {
        self.len == bytes.len() && self.with_bytes(|ours| ours == bytes)
    }

    fn same_text(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        if Arc::ptr_eq(&self.buffer, &other.buffer) {
            return self.start == other.start
                || self.buffer.with_bytes(|all, first| {
                    let a = (self.start - first) as usize;
                    let b = (other.start - first) as usize;
                    all[a..a + self.len] == all[b..b + self.len]
                });
        }
        if Arc::as_ptr(&self.buffer) < Arc::as_ptr(&other.buffer) {
            self.with_bytes(|ours| other.is_text(ours))
        } else {
            other.with_bytes(|theirs| self.is_text(theirs))
        }
    }

    fn existing_extension(&self, added: &[u8], after: bool) -> Option<i64> {
        let start = if after {
            self.start
        } else {
            self.start - added.len() as i64
        };
        let edge = if after {
            self.start + self.len as i64
        } else {
            start
        };
        self.buffer.with_bytes(|all, first| {
            let offset = usize::try_from(edge - first).ok()?;
            let available = all.get(offset..)?.get(..added.len())?;
            (available == added).then_some(start)
        })
    }

    fn can_extend(&self, text: &TextBytes) -> bool {
        self.start == text.first && self.len == text.bytes.len()
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

    /// The value of the string's text ([`TextHasher::text`]).
    #[inline]
    pub(crate) fn text_hash(&self, hasher: &TextHasher) -> u64 {
        if self.hash == UNHASHED {
            return hasher.text(self.as_bytes());
        }
        self.hash
    }

    /// Feeds what stands for the string's text to the hasher of another
    /// table (the keys of a Map, the values of a Set): the value of the
    /// text, which every string of the string table has, in place of the
    /// text itself. Equal texts have equal values, and the hasher makes of
    /// the value what it would have made of the text; so a long key is not
    /// read again at every lookup, and one that is being built is not
    /// joined for it.
    #[inline]
    pub(crate) fn hash_text<H: core::hash::Hasher>(&self, state: &mut H) {
        if self.hash == UNHASHED {
            return self.data.hash(state);
        }
        state.write_u64(self.hash);
    }

    /// The string's hash in the heap's string table.
    #[inline]
    pub(crate) fn table_hash(&self, hasher: &TextHasher) -> u64 {
        hasher.table_hash(self.text_hash(hasher))
    }

    /// Compares the bytes for interning without freezing a growing buffer.
    pub(crate) fn text_is(&self, bytes: &[u8]) -> bool {
        match &self.data {
            StringBuffer::Built(text) => text.is_text(bytes),
            _ => self.as_bytes() == bytes,
        }
    }

    /// Compares two spans with the stored bytes without freezing the buffer.
    pub(crate) fn text_is_both(&self, left: &[u8], right: &[u8]) -> bool {
        if self.len() != left.len() + right.len() {
            return false;
        }
        let matches = |bytes: &[u8]| {
            let (start, end) = bytes.split_at(left.len());
            start == left && end == right
        };
        match &self.data {
            StringBuffer::Built(text) => text.with_bytes(matches),
            _ => matches(self.as_bytes()),
        }
    }

    /// Whether two strings have the same bytes.
    pub(crate) fn same_text(&self, other: &StringRecord) -> bool {
        match (&self.data, &other.data) {
            (StringBuffer::Built(ours), StringBuffer::Built(theirs)) => ours.same_text(theirs),
            (StringBuffer::Built(text), _) => text.is_text(other.as_bytes()),
            (_, StringBuffer::Built(text)) => text.is_text(self.as_bytes()),
            _ => self.as_bytes() == other.as_bytes(),
        }
    }

    /// Whether putting more text after or before the string continues a
    /// text that is being built ([`StringRecord::appended`],
    /// [`StringRecord::prepended`]): a concatenation made it.
    pub(crate) fn is_being_built(&self) -> bool {
        matches!(
            self.data,
            StringBuffer::Concatenated(_) | StringBuffer::Built(_)
        )
    }

    /// A long string that a concatenation made, and the value of its text.
    pub(crate) fn from_concatenation(text: Wtf8Buf, hash: u64) -> Self {
        debug_assert!(text.len() > 7);
        assert!(text.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Concatenated(text),
            mapping: OnceCell::new(),
            hash,
        }
    }

    /// The UTF-16 length, counted from the bytes where nothing has it: the
    /// index table of a long text is larger than the text, and only reading
    /// by index needs it.
    fn counted_utf16_len(&self) -> usize {
        match (&self.data, self.mapping.get()) {
            (StringBuffer::Built(text), _) => text.utf16_len,
            (_, Some(_)) => self.utf16_len(),
            _ => utf16_len_of(self.as_bytes()),
        }
    }

    pub(crate) fn growth_allocation(&self, added: &[u8], after: bool) -> usize {
        if let StringBuffer::Built(text) = &self.data
            && text.buffer.frozen.get().is_none()
            && let Some(bytes) = text.buffer.growing.lock().unwrap().as_ref()
            && text.can_extend(bytes)
        {
            let needed = bytes.bytes.len() + added.len();
            return if needed <= bytes.bytes.capacity() {
                0
            } else {
                text.buffer.accounting.capacity(bytes.bytes.len(), needed)
            };
        }
        if let StringBuffer::Built(text) = &self.data
            && text.existing_extension(added, after).is_some()
        {
            return 0;
        }
        self.new_buffer_allocation(added.len())
    }

    pub(crate) fn new_buffer_allocation(&self, added: usize) -> usize {
        self.len()
            .saturating_add(added)
            .saturating_add(TextBuffer::OVERHEAD)
    }

    pub(crate) fn appended(
        left: &StringRecord,
        right: &[u8],
        hasher: &TextHasher,
        accounting: &Arc<TextBufferBytes>,
    ) -> StringRecord {
        let power = match &left.data {
            StringBuffer::Built(text) if text.power != 0 => {
                multiply_text(text.power, hasher.power(right.len()))
            }
            _ => 0,
        };
        left.extended(
            right,
            true,
            hasher.appended(left.text_hash(hasher), right),
            power,
            accounting,
        )
    }

    pub(crate) fn prepended(
        left: &[u8],
        left_hash: u64,
        right: &StringRecord,
        hasher: &TextHasher,
        accounting: &Arc<TextBufferBytes>,
    ) -> StringRecord {
        let right_power = match &right.data {
            StringBuffer::Built(text) if text.power != 0 => text.power,
            _ => hasher.power(right.len()),
        };
        let hash = TextHasher::joined_by(left_hash, right.text_hash(hasher), right_power);
        let power = multiply_text(right_power, hasher.power(left.len()));
        right.extended(left, false, hash, power, accounting)
    }

    fn extended(
        &self,
        added: &[u8],
        after: bool,
        hash: u64,
        power: u64,
        accounting: &Arc<TextBufferBytes>,
    ) -> StringRecord {
        let len = self.len() + added.len();
        assert!(len <= Self::MAX_UTF8_LENGTH, "String is too long.");
        let utf16_len = self.counted_utf16_len() + utf16_len_of(added);
        let make = |buffer, start| StringRecord {
            data: StringBuffer::Built(Box::new(BuiltText {
                buffer,
                start,
                len,
                utf16_len,
                power,
            })),
            mapping: OnceCell::new(),
            hash,
        };
        if let StringBuffer::Built(text) = &self.data
            && text.buffer.frozen.get().is_none()
            && let Some(bytes) = text.buffer.growing.lock().unwrap().as_mut()
            && text.can_extend(bytes)
        {
            let before = bytes.bytes.capacity();
            let needed = bytes.bytes.len() + added.len();
            bytes.add(
                added,
                after,
                text.buffer.accounting.capacity(bytes.bytes.len(), needed),
            );
            if bytes.bytes.capacity() != before {
                text.buffer.account(bytes.bytes.capacity() - before);
            }
            let start = if after {
                text.start
            } else {
                text.start - added.len() as i64
            };
            return make(Arc::clone(&text.buffer), start);
        }
        if let StringBuffer::Built(text) = &self.data
            && let Some(start) = text.existing_extension(added, after)
        {
            return make(Arc::clone(&text.buffer), start);
        }
        let mut bytes = TextBytes {
            first: 0,
            bytes: VecDeque::with_capacity(len),
        };
        let mut copy = |current: &[u8]| {
            if after {
                bytes.bytes.extend(current);
                bytes.bytes.extend(added);
            } else {
                bytes.bytes.extend(added);
                bytes.bytes.extend(current);
            }
        };
        match &self.data {
            StringBuffer::Built(text) => text.with_bytes(copy),
            _ => copy(self.as_bytes()),
        }
        let capacity = bytes.bytes.capacity();
        let buffer = Arc::new(TextBuffer {
            growing: Mutex::new(Some(bytes)),
            frozen: OnceLock::new(),
            accounting: Arc::clone(accounting),
        });
        buffer.account(capacity + TextBuffer::OVERHEAD);
        make(buffer, 0)
    }

    pub(crate) fn allocated_bytes(&self) -> usize {
        match &self.data {
            StringBuffer::Built(_) => core::mem::size_of::<BuiltText>(),
            _ => self.len(),
        }
    }

    /// Get the byte length of the string.
    pub(crate) fn len(&self) -> usize {
        match &self.data {
            StringBuffer::Owned(buf) | StringBuffer::Concatenated(buf) => buf.len(),
            StringBuffer::Static(buf) => buf.len(),
            StringBuffer::Built(text) => text.len,
        }
    }

    /// The bytes up to which [`StringRecord::utf16_len`] counts the code
    /// units of a text in place of making the table of its indices: asked
    /// again and again for its length and never for an index, a text of
    /// this length is counted each time, in a few instructions.
    const COUNTED_LENGTH: usize = 64;

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
        if let StringBuffer::Built(text) = &self.data {
            return text.utf16_len;
        }
        // A text whose indices are known says its length by them.
        match self.mapping.get() {
            Some(IndexMapping::Ascii) => return self.len(),
            Some(IndexMapping::NonAscii { mapping }) => return mapping.len(),
            None => {}
        }
        // A short text that was not asked for its indices yet is counted.
        // The table of them is an allocation of eight bytes for every code
        // unit, made here to answer once what a look at the bytes answers:
        // the length of a substring, which is all that many are asked for.
        // A text of ASCII is as long as its bytes, and that is all there
        // is to know of its indices, with nothing allocated for it: it is
        // remembered, so that the text is not looked through at every
        // `s.length` (counted each time, a text of sixty characters was
        // four hundred instructions a read).
        if self.len() <= Self::COUNTED_LENGTH {
            let bytes = self.as_bytes();
            if bytes.is_ascii() {
                let _ = self.mapping.set(IndexMapping::Ascii);
                return bytes.len();
            }
            return utf16_len_of(bytes);
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
            StringBuffer::Built(text) => text.as_wtf8(),
        }
    }

    /// The record of a text and the value of that text
    /// ([`TextHasher::text`]), as for the constructors below.
    pub(crate) fn from_str(str: &str, hash: u64) -> Self {
        debug_assert!(str.len() > 7);
        assert!(str.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Owned(Wtf8Buf::from_str(str)),
            mapping: OnceCell::new(),
            hash,
        }
    }

    /// The record of a text that is not in the string table: nothing has
    /// hashed it.
    pub(crate) fn unlisted(str: &str) -> Self {
        Self::from_str(str, UNHASHED)
    }

    pub(crate) fn from_static_str(str: &'static str, hash: u64) -> Self {
        debug_assert!(str.len() > 7);
        assert!(str.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Static(Wtf8Str::from_str(str)),
            mapping: OnceCell::new(),
            hash,
        }
    }

    pub(crate) fn from_string(str: String, hash: u64) -> Self {
        debug_assert!(str.len() > 7);
        assert!(str.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Owned(Wtf8Buf::from_string(str)),
            mapping: OnceCell::new(),
            hash,
        }
    }

    pub(crate) fn from_wtf8_buf(str: Wtf8Buf, hash: u64) -> Self {
        debug_assert!(str.len() > 7);
        assert!(str.len() <= Self::MAX_UTF8_LENGTH, "String is too long.");
        StringRecord {
            data: StringBuffer::Owned(str),
            mapping: OnceCell::new(),
            hash,
        }
    }
}

impl HeapMarkAndSweep for StringRecord {
    fn mark_values(&self, _queues: &mut WorkQueues) {
        let Self {
            data: _,
            mapping: _,
            hash: _,
        } = self;
    }

    fn sweep_values(&mut self, _compactions: &CompactionLists) {
        let Self {
            data: _,
            mapping: _,
            hash: _,
        } = self;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_WIDTH: usize = 31;

    fn concatenation(text: &str, hasher: &TextHasher) -> StringRecord {
        StringRecord::from_concatenation(Wtf8Buf::from_str(text), hasher.text(text.as_bytes()))
    }

    /// A text built in steps, each of which appends so many characters or
    /// puts them before (`false`), checking lengths, hashes and saved versions.
    fn built_in_steps(steps: impl Iterator<Item = (bool, usize)>) {
        const CHARS: [char; 9] = ['a', 'ж', 'z', '中', 'q', '😀', 'k', 'é', 'x'];
        let hasher = TextHasher::new();
        let budget = Arc::new(TextBufferBytes::default());
        let mut expected = std::string::String::from("the start of the text");
        let mut text = concatenation(&expected, &hasher);
        for (step, (after, size)) in steps.enumerate() {
            let added: std::string::String = (0..size)
                .map(|i| CHARS[(step * 5 + i * 7 + i / 3) % CHARS.len()])
                .collect();
            let before = expected.clone();
            let next = if after {
                expected.push_str(&added);
                StringRecord::appended(&text, added.as_bytes(), &hasher, &budget)
            } else {
                expected.insert_str(0, &added);
                StringRecord::prepended(
                    added.as_bytes(),
                    hasher.text(added.as_bytes()),
                    &text,
                    &hasher,
                    &budget,
                )
            };
            assert_eq!(
                next.text_hash(&hasher),
                hasher.text(expected.as_bytes()),
                "step {step}"
            );
            assert_eq!(next.len(), expected.len());
            assert_eq!(next.utf16_len(), expected.encode_utf16().count());
            if let StringBuffer::Built(built) = &next.data
                && built.power != 0
            {
                assert_eq!(built.power, hasher.power(expected.len()), "step {step}");
            }
            assert!(next.text_is(expected.as_bytes()), "step {step}");
            // The text before is what it was, whatever was added to it.
            if step % 97 == 0 {
                assert_eq!(text.as_bytes(), before.as_bytes());
                assert_eq!(next.as_bytes(), expected.as_bytes());
            }
            text = next;
        }
        assert_eq!(
            text.table_hash(&hasher),
            hasher.table_hash(hasher.text(expected.as_bytes()))
        );
        assert_eq!(text.as_bytes(), expected.as_bytes());
    }

    /// `text` built from its bytes `from..to` by the `steps`, over and over:
    /// each appends so many of the bytes after what there is, or puts so
    /// many of those before it in front (`false`).
    fn built(
        text: &str,
        (from, to): (usize, usize),
        steps: &[(bool, usize)],
        hasher: &TextHasher,
        budget: &Arc<TextBufferBytes>,
    ) -> StringRecord {
        let bytes = text.as_bytes();
        let (mut start, mut end) = (from, to);
        let mut record = concatenation(&text[start..end], hasher);
        for &(after, size) in steps.iter().cycle() {
            if start == 0 && end == bytes.len() {
                break;
            }
            if after && end < bytes.len() {
                let next = (end + size).min(bytes.len());
                record = StringRecord::appended(&record, &bytes[end..next], hasher, budget);
                end = next;
            } else if !after && start > 0 {
                let next = start.saturating_sub(size);
                let added = &bytes[next..start];
                record =
                    StringRecord::prepended(added, hasher.text(added), &record, hasher, budget);
                start = next;
            }
        }
        record
    }

    #[test]
    fn growing_buffers_charge_once_and_keep_borrowed_versions() {
        let accounting = Arc::new(TextBufferBytes::default());
        let hasher = TextHasher::new();
        let base = concatenation("original text", &hasher);
        let first = StringRecord::appended(&base, b"!", &hasher, &accounting);
        let longer = StringRecord::appended(&first, b"++", &hasher, &accounting);
        let before =
            StringRecord::prepended(b"-", hasher.text(b"-"), &longer, &hasher, &accounting);
        let held = accounting.live.load(Ordering::Relaxed);
        assert_eq!(accounting.allocated.load(Ordering::Relaxed), held);
        assert!(held >= before.len() + TextBuffer::OVERHEAD);
        assert!(held < before.len() + TextBuffer::OVERHEAD + 128);
        assert_eq!(first.growth_allocation(b"++", true), 0);
        let borrowed = first.as_bytes();
        assert_eq!(before.as_bytes(), b"-original text!++");
        assert_eq!(accounting.live.load(Ordering::Relaxed), held);
        let fork = StringRecord::appended(&first, b"?", &hasher, &accounting);
        assert_eq!(borrowed, b"original text!");
        assert_eq!(fork.as_bytes(), b"original text!?");
        assert_eq!(
            accounting.live.load(Ordering::Relaxed),
            held + first.new_buffer_allocation(1)
        );
        drop((first, longer, before));
        assert_eq!(
            accounting.live.load(Ordering::Relaxed),
            fork.len() + TextBuffer::OVERHEAD
        );
        drop(fork);
        assert_eq!(accounting.live.load(Ordering::Relaxed), 0);
        accounting
            .allocated
            .store(usize::MAX - 1, Ordering::Relaxed);
        let final_text = StringRecord::appended(&base, b"!", &hasher, &accounting);
        assert_eq!(final_text.as_bytes(), b"original text!");
        assert_eq!(accounting.allocated.load(Ordering::Relaxed), usize::MAX);
    }

    /// The string table asks of a new string whether it is one it has, and
    /// a loop that builds a text again makes, at every step, the string it
    /// made before. Comparing their bytes leaves both buffers growable.
    #[test]
    fn built_texts_are_compared_without_freezing_their_buffers() {
        let hasher = TextHasher::new();
        let budget = Arc::new(TextBufferBytes::default());
        let text: std::string::String = (0..5000usize)
            .map(|at| char::from(b'a' + ((at * 7 + at / 13) % 26) as u8))
            .collect();
        type Recipe<'a> = ((usize, usize), &'a [(bool, usize)]);
        let recipes: [Recipe; 12] = [
            ((0, 21), &[(true, 1)]),
            ((0, 21), &[(true, 5)]),
            (
                (0, 21),
                &[(true, SAMPLE_WIDTH), (true, 1), (true, SAMPLE_WIDTH + 1)],
            ),
            (
                (0, 21),
                &[
                    (true, 1),
                    (true, 70),
                    (true, 3),
                    (true, 31),
                    (true, 32),
                    (true, 200),
                ],
            ),
            ((0, 21), &[(true, 400), (true, 3)]),
            ((0, 21), &[(true, 4979)]),
            ((4979, 5000), &[(false, 1)]),
            ((4979, 5000), &[(false, 2)]),
            (
                (4979, 5000),
                &[(false, SAMPLE_WIDTH), (false, 1), (false, SAMPLE_WIDTH + 1)],
            ),
            ((4700, 5000), &[(false, 300), (false, 7)]),
            ((2400, 2700), &[(true, 1), (false, 2)]),
            (
                (2400, 2700),
                &[(false, 40), (true, 3), (true, 90), (false, 1)],
            ),
        ];
        let plain =
            StringRecord::from_wtf8_buf(Wtf8Buf::from_str(&text), hasher.text(text.as_bytes()));
        for (range, ours) in recipes {
            let first = built(&text, range, ours, &hasher, &budget);
            assert!(first.text_is(text.as_bytes()), "{range:?} {ours:?}");
            assert!(first.same_text(&plain) && plain.same_text(&first));
            assert!(first == plain);
            for at in [0, 1, 21, 2500, 4999, 5000] {
                let (left, right) = text.as_bytes().split_at(at);
                assert!(first.text_is_both(left, right), "{range:?} {ours:?} {at}");
                assert!(plain.text_is_both(left, right));
                if at > 0 {
                    let mut other = left.to_vec();
                    other[at / 2] ^= 1;
                    assert!(
                        !first.text_is_both(&other, right),
                        "{range:?} {ours:?} {at}"
                    );
                }
                if at > 0 && at < text.len() {
                    assert!(!first.text_is_both(right, left), "{range:?} {ours:?} {at}");
                }
            }
            assert!(!first.text_is_both(&text.as_bytes()[..2500], &text.as_bytes()[2501..]));
            for (their_range, theirs) in recipes {
                let second = built(&text, their_range, theirs, &hasher, &budget);
                assert!(first.same_text(&second), "{ours:?} {theirs:?}");
                assert_eq!(
                    first.table_hash(&hasher),
                    second.table_hash(&hasher),
                    "{ours:?} {theirs:?}"
                );
            }
            // One byte of difference, wherever it is, and another length.
            for at in [
                0, 1, 20, 21, 22, 2399, 2400, 2500, 2700, 4968, 4978, 4979, 4998, 4999,
            ] {
                let mut changed = text.clone().into_bytes();
                changed[at] ^= 1;
                assert!(!first.text_is(&changed), "{ours:?} {at}");
                let changed = std::string::String::from_utf8(changed).unwrap();
                for (their_range, theirs) in recipes {
                    let other = built(&changed, their_range, theirs, &hasher, &budget);
                    assert!(!first.same_text(&other), "{ours:?} {theirs:?} {at}");
                    assert!(!other.same_text(&first), "{ours:?} {theirs:?} {at}");
                }
            }
            assert!(!first.text_is(&text.as_bytes()[1..]));
            assert!(!first.text_is(&text.as_bytes()[..4999]));
            let longer = StringRecord::appended(&first, b"!", &hasher, &budget);
            assert!(!first.same_text(&longer) && !longer.same_text(&first));
            let longer = StringRecord::prepended(b"!", hasher.text(b"!"), &first, &hasher, &budget);
            assert!(!first.same_text(&longer) && !longer.same_text(&first));
            // What continues one text the same way is the same text, and
            // not what continues it another way; at either end.
            for added in [&b"A"[..], &[b'A'; 40][..]] {
                let mut other = added.to_vec();
                other[0] = b'B';
                let one = StringRecord::appended(&first, added, &hasher, &budget);
                let two = StringRecord::appended(&first, added, &hasher, &budget);
                assert!(one.same_text(&two));
                assert_eq!(one.text_hash(&hasher), two.text_hash(&hasher));
                let three = StringRecord::appended(&first, &other, &hasher, &budget);
                assert!(!one.same_text(&three));
                let before = |added: &[u8]| {
                    StringRecord::prepended(added, hasher.text(added), &first, &hasher, &budget)
                };
                let (four, five, six) = (before(added), before(added), before(&other));
                assert!(four.same_text(&five));
                assert_eq!(four.text_hash(&hasher), five.text_hash(&hasher));
                assert!(!four.same_text(&six));
                // The same bytes at the other end are another text.
                assert!(!one.same_text(&four) && !four.same_text(&one));
            }
        }

        // A frozen buffer is compared by the same operations.
        let joined = built(&text, (0, 21), &[(true, 7)], &hasher, &budget);
        assert_eq!(joined.as_bytes(), text.as_bytes());
        assert!(joined.text_is(text.as_bytes()));
        assert!(joined.text_is_both(&text.as_bytes()[..77], &text.as_bytes()[77..]));
        assert!(joined.same_text(&built(&text, (0, 21), &[(true, 9)], &hasher, &budget)));
        assert!(joined.same_text(&built(
            &text,
            (4000, 4300),
            &[(false, 9), (true, 2)],
            &hasher,
            &budget
        )));
    }

    #[test]
    fn a_built_text_keeps_its_versions_whatever_is_added() {
        for after in [true, false] {
            // One character at a time, then varying widths.
            built_in_steps(core::iter::repeat_n((after, 1), 3000));
            built_in_steps((0..600).map(|step| (after, SAMPLE_WIDTH - 2 + step % 5)));
            built_in_steps((0..400).map(|step| (after, [1, 70, 3, 31, 32, 200, 5][step % 7])));
            // Decreasing and increasing widths.
            built_in_steps((1..=300).rev().map(|size| (after, size)));
            // And each longer.
            built_in_steps((1..=300).map(|size| (after, size)));
        }
        // At both ends in turn, and in runs.
        built_in_steps((0..2000).map(|step| (step % 2 == 0, 1 + step % 3)));
        built_in_steps((0..900).map(|step| (step / 40 % 2 == 0, [2, 45, 1, 9][step % 4])));
    }
}
