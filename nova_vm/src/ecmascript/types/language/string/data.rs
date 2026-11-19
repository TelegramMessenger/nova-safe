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
    /// for neither pieces nor joining.
    Concatenated(Wtf8Buf),
    /// A concatenated string with more text put after or before it: see
    /// [`BuiltText`].
    Built(Box<BuiltText>),
}

impl Hash for StringBuffer {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        match self {
            StringBuffer::Owned(wtf8_buf) | StringBuffer::Concatenated(wtf8_buf) => {
                wtf8_buf.hash(state)
            }
            StringBuffer::Static(wtf8) => wtf8.hash(state),
            StringBuffer::Built(text) => text.joined().hash(state),
        }
    }
}

/// The text of a string that was made by putting more text after or before
/// another one, kept as the pieces it was put together from. A loop that
/// builds a string by `s += x` or `s = x + s` makes a new string at every
/// step; were each one a copy of the last, the loop would copy the whole text
/// at every step. Here the new string shares the old one's pieces and adds
/// its own, and its bytes are joined into one buffer the first time
/// something reads them.
///
/// The text is `first`, the `front` pieces from the first one on, the `back`
/// pieces from the last one back, and `last`. What is appended goes to the
/// back pieces and what is put before to the front ones, so that either
/// shares all of the other side.
///
/// Towards the middle of the text each piece is at least twice as long as
/// the one before it: adding to a side takes the next piece into the new one
/// while it is shorter than that, so a side has no more pieces than its
/// length has bits (which also bounds how deep dropping them goes), and a
/// byte is copied a logarithmic number of times.
///
/// The two ends of the text are not in a piece until they are long enough
/// for one ([`EDGE_BYTES`]): most steps of such a loop add a few bytes,
/// which are then copied next to those of the steps before and allocate no
/// piece.
#[derive(Debug, Clone)]
pub(crate) struct BuiltText {
    /// The text after `first`, as far as the front pieces go: each piece is
    /// followed by the next one in the list.
    front: Option<Arc<Piece>>,
    /// The text from there to `last`: the last piece first, each preceded by
    /// the next one in the list.
    back: Arc<Piece>,
    /// The start of the text: what was put before it since the last front
    /// piece was made.
    first: [u8; EDGE_BYTES],
    first_len: u8,
    /// The end of the text: what was appended since the last back piece was
    /// made.
    last: [u8; EDGE_BYTES],
    last_len: u8,
    /// Bytes of `first` and of the front pieces.
    front_len: usize,
    /// Bytes of the whole text.
    len: usize,
    utf16_len: usize,
    /// Bytes of pieces that making this text allocated.
    fresh_bytes: usize,
    /// What the value of a text put before this one is multiplied by
    /// ([`TextHasher::power`] of `len`), where something was put before it
    /// or before the text it continues; otherwise 0, which no power is. A
    /// loop that builds a text from its end has it from the step before.
    power: u64,
    joined: OnceCell<Wtf8Buf>,
    budget: Arc<JoinBudget>,
}

/// What the heap's built strings may join. Joining a text allocates all of
/// it where no allocation counter is at hand and nothing can be refused, and
/// a script can hold many long built strings for the price of their
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

/// How many bytes of its start and of its end a built text keeps out of its
/// pieces ([`BuiltText::first`], [`BuiltText::last`]).
const EDGE_BYTES: usize = 31;

/// A piece of one side of a built text, and the next piece of that side
/// towards the middle of the text.
#[derive(Debug)]
struct Piece {
    bytes: Box<[u8]>,
    next: Option<Arc<Piece>>,
}

impl Piece {
    /// The pieces of a side that has `first` and then `second` beyond
    /// `inward`, at the end of the text or at its start, and how many bytes
    /// that allocated. The pieces that the new one takes in are found before
    /// anything is allocated, so that it is allocated once, whatever it
    /// takes in.
    fn grown(
        inward: Option<&Arc<Piece>>,
        first: &[u8],
        second: &[u8],
        at_end: bool,
    ) -> (Arc<Piece>, usize) {
        let mut len = first.len() + second.len();
        let mut taken = 0usize;
        let mut kept = inward;
        while let Some(next) = kept
            && next.bytes.len() < len.saturating_mul(2)
        {
            len += next.bytes.len();
            taken += 1;
            kept = next.next.as_ref();
        }
        let mut bytes = Vec::with_capacity(len);
        if at_end {
            // The pieces are linked from the last one back, and few are
            // taken in: each is walked to, the first of them first.
            for nth in (0..taken).rev() {
                let mut piece = inward;
                for _ in 0..nth {
                    piece = piece.and_then(|piece| piece.next.as_ref());
                }
                if let Some(piece) = piece {
                    bytes.extend_from_slice(&piece.bytes);
                }
            }
            bytes.extend_from_slice(first);
            bytes.extend_from_slice(second);
        } else {
            bytes.extend_from_slice(first);
            bytes.extend_from_slice(second);
            let mut piece = inward;
            for _ in 0..taken {
                if let Some(current) = piece {
                    bytes.extend_from_slice(&current.bytes);
                    piece = current.next.as_ref();
                }
            }
        }
        debug_assert_eq!(bytes.len(), len);
        let piece = Arc::new(Piece {
            bytes: bytes.into_boxed_slice(),
            next: kept.cloned(),
        });
        // With the two counts that the piece is shared by.
        let allocated = len + core::mem::size_of::<Piece>() + 2 * core::mem::size_of::<usize>();
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

/// A walk over one side of a built text from its edge towards the middle of
/// the text, in the chunks the side keeps its bytes in: what the text keeps
/// of that edge, then the side's pieces. On the back side a chunk's end is
/// taken first, on the front side its start.
struct Walk<'a> {
    /// What is left of the chunk being walked.
    chunk: &'a [u8],
    /// The piece after it in the walk.
    piece: Option<&'a Arc<Piece>>,
}

impl<'a> Walk<'a> {
    /// Goes on to the next chunk that has bytes when this one has none left;
    /// the chunk stays empty where the side ends.
    fn fill(&mut self) {
        while self.chunk.is_empty()
            && let Some(piece) = self.piece
        {
            self.chunk = &piece.bytes;
            self.piece = piece.next.as_ref();
        }
    }

    /// Whether the side is `bytes`.
    fn is(mut self, mut bytes: &'a [u8], from_end: bool) -> bool {
        loop {
            self.fill();
            if self.chunk.is_empty() {
                return bytes.is_empty();
            }
            let Some(rest) = bytes.len().checked_sub(self.chunk.len()) else {
                return false;
            };
            let (taken, left) = if from_end {
                let (left, taken) = bytes.split_at(rest);
                (taken, left)
            } else {
                bytes.split_at(self.chunk.len())
            };
            if taken != self.chunk {
                return false;
            }
            bytes = left;
            self.chunk = &[];
        }
    }

    /// Whether two sides of as many bytes are the same. Texts that continue
    /// one text share its pieces, and from where both walks come to the
    /// same piece the rest is the same without being looked at.
    fn same(mut self, mut other: Walk<'a>, from_end: bool) -> bool {
        loop {
            if self.chunk.is_empty() && other.chunk.is_empty() {
                match (self.piece, other.piece) {
                    (Some(ours), Some(theirs)) if Arc::ptr_eq(ours, theirs) => return true,
                    (None, None) => return true,
                    _ => {}
                }
            }
            self.fill();
            other.fill();
            let shared = self.chunk.len().min(other.chunk.len());
            if shared == 0 {
                return self.chunk.is_empty() && other.chunk.is_empty();
            }
            if from_end {
                let (ours_rest, ours) = self.chunk.split_at(self.chunk.len() - shared);
                let (theirs_rest, theirs) = other.chunk.split_at(other.chunk.len() - shared);
                if ours != theirs {
                    return false;
                }
                self.chunk = ours_rest;
                other.chunk = theirs_rest;
            } else {
                let (ours, ours_rest) = self.chunk.split_at(shared);
                let (theirs, theirs_rest) = other.chunk.split_at(shared);
                if ours != theirs {
                    return false;
                }
                self.chunk = ours_rest;
                other.chunk = theirs_rest;
            }
        }
    }
}

impl BuiltText {
    /// The walk over the front side.
    fn front_walk(&self) -> Walk<'_> {
        Walk {
            chunk: self.first(),
            piece: self.front.as_ref(),
        }
    }

    /// The walk over the back side.
    fn back_walk(&self) -> Walk<'_> {
        Walk {
            chunk: self.last(),
            piece: Some(&self.back),
        }
    }

    /// Whether the text is `bytes`, learned from the pieces: nothing is
    /// joined for it.
    fn is_text(&self, bytes: &[u8]) -> bool {
        if let Some(joined) = self.joined.get() {
            return joined.as_bytes() == bytes;
        }
        if self.len != bytes.len() {
            return false;
        }
        let (front, back) = bytes.split_at(self.front_len);
        self.back_walk().is(back, true) && self.front_walk().is(front, false)
    }

    /// Whether two texts are the same, learned from the pieces of both:
    /// nothing is joined for it. The text a loop builds again, step by
    /// step, is found to be the one it built before by what each step added
    /// ([`Walk::same`]).
    fn same_text(&self, other: &BuiltText) -> bool {
        if self.len != other.len {
            return false;
        }
        if self.front_len == other.front_len {
            return self.back_walk().same(other.back_walk(), true)
                && self.front_walk().same(other.front_walk(), false);
        }
        // Put together in other ways: chunk by chunk from the start.
        let (ours, theirs) = (self.chunks(), other.chunks());
        let (mut ours, mut theirs) = (ours.into_iter(), theirs.into_iter());
        let (mut one, mut two): (&[u8], &[u8]) = (&[], &[]);
        loop {
            while one.is_empty()
                && let Some(next) = ours.next()
            {
                one = next;
            }
            while two.is_empty()
                && let Some(next) = theirs.next()
            {
                two = next;
            }
            let shared = one.len().min(two.len());
            if shared == 0 {
                return one.is_empty() && two.is_empty();
            }
            if one[..shared] != two[..shared] {
                return false;
            }
            one = &one[shared..];
            two = &two[shared..];
        }
    }

    /// The chunks of the text from its start.
    fn chunks(&self) -> Vec<&[u8]> {
        let mut chunks = vec![self.first()];
        let mut piece = self.front.as_ref();
        while let Some(current) = piece {
            chunks.push(&current.bytes);
            piece = current.next.as_ref();
        }
        let back = chunks.len();
        let mut piece = Some(&self.back);
        while let Some(current) = piece {
            chunks.push(&current.bytes);
            piece = current.next.as_ref();
        }
        chunks[back..].reverse();
        chunks.push(self.last());
        chunks
    }

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
                let mut text = Wtf8Buf::with_capacity(self.len);
                for bytes in self.chunks() {
                    text.push_wtf8(Wtf8Str::from_wtf8_bytes(bytes));
                }
                text
            })
            .as_wtf8()
    }

    /// The start of the text that is in no piece.
    fn first(&self) -> &[u8] {
        &self.first[..usize::from(self.first_len)]
    }

    /// The end of the text that is in no piece.
    fn last(&self) -> &[u8] {
        &self.last[..usize::from(self.last_len)]
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

    /// Whether the string's text is `bytes`. A built text is not joined to
    /// learn it: the string table asks this of every string whose hash is
    /// that of a new one, and a join is the whole text copied.
    pub(crate) fn text_is(&self, bytes: &[u8]) -> bool {
        match &self.data {
            StringBuffer::Built(text) => text.is_text(bytes),
            _ => self.as_bytes() == bytes,
        }
    }

    /// Whether the string's text is `left` and then `right`, with nothing
    /// joined or put together to learn it.
    pub(crate) fn text_is_both(&self, left: &[u8], right: &[u8]) -> bool {
        if self.len() != left.len() + right.len() {
            return false;
        }
        match &self.data {
            StringBuffer::Built(text) if text.joined.get().is_none() => {
                // Chunk by chunk: the text has no one buffer to split.
                let mut expected = [left, right].into_iter();
                let mut wanted: &[u8] = &[];
                for mut chunk in text.chunks() {
                    while !chunk.is_empty() {
                        while wanted.is_empty() {
                            match expected.next() {
                                Some(next) => wanted = next,
                                None => return false,
                            }
                        }
                        let shared = chunk.len().min(wanted.len());
                        if chunk[..shared] != wanted[..shared] {
                            return false;
                        }
                        chunk = &chunk[shared..];
                        wanted = &wanted[shared..];
                    }
                }
                true
            }
            _ => {
                let (start, end) = self.as_bytes().split_at(left.len());
                start == left && end == right
            }
        }
    }

    /// Whether two strings have the same text, with neither joined to learn
    /// it ([`StringRecord::text_is`]).
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

    /// `left` with `right` appended (no surrogate pair joined across the
    /// two). Nothing is joined, and the text of `left` is not read for the
    /// hash: the new text shares the pieces of `left`, or starts them with a
    /// copy of it.
    pub(crate) fn appended(
        left: &StringRecord,
        right: &[u8],
        hasher: &TextHasher,
        budget: &Arc<JoinBudget>,
    ) -> StringRecord {
        let len = left.len() + right.len();
        assert!(len <= Self::MAX_UTF8_LENGTH, "String is too long.");
        let utf16_len = left.counted_utf16_len() + utf16_len_of(right);
        let hash = hasher.appended(left.text_hash(hasher), right);
        let mut last = [0u8; EDGE_BYTES];
        let mut last_len = 0;
        let mut power = 0;
        let (front, first, first_len, front_len, back, fresh_bytes) = match &left.data {
            StringBuffer::Built(text) => {
                if text.power != 0 {
                    power = multiply_text(text.power, hasher.power(right.len()));
                }
                let held = text.last();
                let (back, fresh_bytes) = if held.len() + right.len() <= EDGE_BYTES {
                    last_len = held.len() + right.len();
                    last[..held.len()].copy_from_slice(held);
                    last[held.len()..last_len].copy_from_slice(right);
                    (text.back.clone(), 0)
                } else {
                    Piece::grown(Some(&text.back), held, right, true)
                };
                (
                    text.front.clone(),
                    text.first,
                    text.first_len,
                    text.front_len,
                    back,
                    fresh_bytes,
                )
            }
            _ => {
                let (whole, whole_bytes) = Piece::grown(None, left.as_bytes(), &[], true);
                let (back, fresh_bytes) = if right.len() <= EDGE_BYTES {
                    last_len = right.len();
                    last[..last_len].copy_from_slice(right);
                    (whole, whole_bytes)
                } else {
                    let (back, fresh_bytes) = Piece::grown(Some(&whole), &[], right, true);
                    (back, whole_bytes + fresh_bytes)
                };
                (None, [0u8; EDGE_BYTES], 0, 0, back, fresh_bytes)
            }
        };
        let data = StringBuffer::Built(Box::new(BuiltText {
            front,
            back,
            first,
            first_len,
            last,
            // At most EDGE_BYTES.
            last_len: last_len as u8,
            front_len,
            len,
            utf16_len,
            fresh_bytes,
            power,
            joined: OnceCell::new(),
            budget: budget.clone(),
        }));
        StringRecord {
            data,
            mapping: OnceCell::new(),
            hash,
        }
    }

    /// `right` with `left`, of value `left_hash`, put before it (no
    /// surrogate pair joined across the two). Nothing is joined, and the
    /// text of `right` is not read for the hash: the new text shares the
    /// pieces of `right`, or starts them with a copy of it.
    pub(crate) fn prepended(
        left: &[u8],
        left_hash: u64,
        right: &StringRecord,
        hasher: &TextHasher,
        budget: &Arc<JoinBudget>,
    ) -> StringRecord {
        let len = left.len() + right.len();
        assert!(len <= Self::MAX_UTF8_LENGTH, "String is too long.");
        let utf16_len = utf16_len_of(left) + right.counted_utf16_len();
        let right_power = match &right.data {
            StringBuffer::Built(text) if text.power != 0 => text.power,
            _ => hasher.power(right.len()),
        };
        let hash = TextHasher::joined_by(left_hash, right.text_hash(hasher), right_power);
        let power = multiply_text(right_power, hasher.power(left.len()));
        let mut first = [0u8; EDGE_BYTES];
        let mut first_len = 0;
        let (front, back, last, last_len, front_len, fresh_bytes) = match &right.data {
            StringBuffer::Built(text) => {
                let held = text.first();
                let (front, fresh_bytes) = if left.len() + held.len() <= EDGE_BYTES {
                    first_len = left.len() + held.len();
                    first[..left.len()].copy_from_slice(left);
                    first[left.len()..first_len].copy_from_slice(held);
                    (text.front.clone(), 0)
                } else {
                    let (front, fresh_bytes) = Piece::grown(text.front.as_ref(), left, held, false);
                    (Some(front), fresh_bytes)
                };
                (
                    front,
                    text.back.clone(),
                    text.last,
                    text.last_len,
                    text.front_len + left.len(),
                    fresh_bytes,
                )
            }
            _ => {
                let (whole, whole_bytes) = Piece::grown(None, right.as_bytes(), &[], true);
                let (front, fresh_bytes) = if left.len() <= EDGE_BYTES {
                    first_len = left.len();
                    first[..first_len].copy_from_slice(left);
                    (None, whole_bytes)
                } else {
                    let (front, fresh_bytes) = Piece::grown(None, left, &[], false);
                    (Some(front), whole_bytes + fresh_bytes)
                };
                (front, whole, [0u8; EDGE_BYTES], 0, left.len(), fresh_bytes)
            }
        };
        let data = StringBuffer::Built(Box::new(BuiltText {
            front,
            back,
            first,
            // At most EDGE_BYTES.
            first_len: first_len as u8,
            last,
            last_len,
            front_len,
            len,
            utf16_len,
            fresh_bytes,
            power,
            joined: OnceCell::new(),
            budget: budget.clone(),
        }));
        StringRecord {
            data,
            mapping: OnceCell::new(),
            hash,
        }
    }

    /// What making the record allocated: for a built string its own pieces,
    /// not the text it shares, so that a loop that builds a text is not
    /// charged the whole text at every step.
    pub(crate) fn allocated_bytes(&self) -> usize {
        match &self.data {
            StringBuffer::Built(text) => text.fresh_bytes + core::mem::size_of::<BuiltText>(),
            _ => self.len(),
        }
    }

    /// What a string that lives on counts for against the heap limit: all of
    /// its text, also where a built string shares it with others. The
    /// pieces make building fast, not free: every built string can be
    /// read, and reading joins its whole text, so the strings a script may
    /// keep are those whose joined texts fit the limit together.
    pub(crate) fn counted_bytes(&self) -> usize {
        match &self.data {
            StringBuffer::Built(text) => {
                let joined = if text.joined.get().is_some() {
                    text.len
                } else {
                    0
                };
                text.len + joined + core::mem::size_of::<BuiltText>()
            }
            _ => self.len(),
        }
    }

    /// Called by the collector for a string that lives on: a built text
    /// that was joined becomes that one buffer and lets go of its pieces.
    fn settle(&mut self) {
        if let StringBuffer::Built(text) = &mut self.data
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
        // A short text that was not asked for its indices yet is counted.
        // The table of them is an allocation of eight bytes for every code
        // unit, made here to answer once what a look at the bytes answers:
        // the length of a substring, which is all that many are asked for.
        if self.mapping.get().is_none() && self.len() <= Self::COUNTED_LENGTH {
            return utf16_len_of(self.as_bytes());
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
            StringBuffer::Built(text) => text.joined(),
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
        self.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lengths of the pieces of one side of a built text, from its edge.
    fn piece_lengths(record: &StringRecord, back: bool) -> Vec<usize> {
        let StringBuffer::Built(text) = &record.data else {
            panic!("not a built text");
        };
        let mut lengths = Vec::new();
        let mut piece = if back {
            Some(&text.back)
        } else {
            text.front.as_ref()
        };
        while let Some(current) = piece {
            lengths.push(current.bytes.len());
            piece = current.next.as_ref();
        }
        lengths
    }

    fn concatenation(text: &str, hasher: &TextHasher) -> StringRecord {
        StringRecord::from_concatenation(Wtf8Buf::from_str(text), hasher.text(text.as_bytes()))
    }

    /// A text built in steps, each of which appends so many characters or
    /// puts them before (`false`): at every step it has the hash, the length
    /// and the few pieces it should, and the text it was built from is what
    /// it was.
    fn built_in_steps(steps: impl Iterator<Item = (bool, usize)>) {
        const CHARS: [char; 9] = ['a', 'ж', 'z', '中', 'q', '😀', 'k', 'é', 'x'];
        let hasher = TextHasher::new();
        let budget = Arc::new(JoinBudget::default());
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
            for back in [true, false] {
                let lengths = piece_lengths(&next, back);
                assert!(lengths.len() <= usize::BITS as usize, "step {step}");
                for pair in lengths.windows(2) {
                    assert!(pair[1] >= 2 * pair[0], "step {step}: {lengths:?}");
                }
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
        budget: &Arc<JoinBudget>,
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

    /// The string table asks of a new string whether it is one it has, and
    /// a loop that builds a text again makes, at every step, the string it
    /// made before. Texts are compared by their pieces: none is joined for
    /// it, however they were put together.
    #[test]
    fn built_texts_are_compared_without_being_joined() {
        let hasher = TextHasher::new();
        let budget = Arc::new(JoinBudget::default());
        let text: std::string::String = (0..5000usize)
            .map(|at| char::from(b'a' + ((at * 7 + at / 13) % 26) as u8))
            .collect();
        type Recipe<'a> = ((usize, usize), &'a [(bool, usize)]);
        let recipes: [Recipe; 12] = [
            ((0, 21), &[(true, 1)]),
            ((0, 21), &[(true, 5)]),
            (
                (0, 21),
                &[(true, EDGE_BYTES), (true, 1), (true, EDGE_BYTES + 1)],
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
                &[(false, EDGE_BYTES), (false, 1), (false, EDGE_BYTES + 1)],
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
        assert_eq!(budget.joined(), 0, "a comparison joined a text");
        // A text that was joined is compared as it is.
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
    fn a_built_text_has_few_pieces_whatever_is_added() {
        for after in [true, false] {
            // One character at a time, and sizes around what a text keeps
            // of its ends.
            built_in_steps(core::iter::repeat_n((after, 1), 3000));
            built_in_steps((0..600).map(|step| (after, EDGE_BYTES - 2 + step % 5)));
            built_in_steps((0..400).map(|step| (after, [1, 70, 3, 31, 32, 200, 5][step % 7])));
            // Each shorter than the one before: no piece takes in the next
            // one by being merely longer than it.
            built_in_steps((1..=300).rev().map(|size| (after, size)));
            // And each longer.
            built_in_steps((1..=300).map(|size| (after, size)));
        }
        // At both ends in turn, and in runs.
        built_in_steps((0..2000).map(|step| (step % 2 == 0, 1 + step % 3)));
        built_in_steps((0..900).map(|step| (step / 40 % 2 == 0, [2, 45, 1, 9][step % 4])));
    }
}
