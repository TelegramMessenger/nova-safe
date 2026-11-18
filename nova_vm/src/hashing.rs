// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Hashing of keys a script chooses (property names, Map and Set keys,
//! strings), with std's SipHash-1-3 under random keys so that a script
//! cannot construct colliding keys.
//!
//! Maps keyed by engine handles (heap indices, small strings and small
//! integers held inline in a handle) use [`HandleState`] instead: a
//! multiply-fold hash under random per-map keys. A script can only pick a
//! handle's few bytes, not the keys, so it cannot precompute collisions; and
//! a flood it got by chance would only slow its own Agent, which the CPU
//! limit ends.
//!
//! The names are those of the aHash types this replaces, so that their
//! users only changed an import.

use std::{
    cell::Cell,
    hash::{BuildHasher, DefaultHasher, Hash, Hasher},
    sync::OnceLock,
};

pub(crate) type AHashMap<K, V> = std::collections::HashMap<K, V, HandleState>;
pub(crate) type AHashSet<K> = std::collections::HashSet<K, HandleState>;

#[inline]
pub(crate) fn map_with_capacity<K, V>(capacity: usize) -> AHashMap<K, V> {
    AHashMap::with_capacity_and_hasher(capacity, HandleState::default())
}

#[inline]
pub(crate) fn set_with_capacity<K>(capacity: usize) -> AHashSet<K> {
    AHashSet::with_capacity_and_hasher(capacity, HandleState::default())
}

#[inline(always)]
fn folded_multiply(a: u64, b: u64) -> u64 {
    let full = u128::from(a) * u128::from(b);
    (full as u64) ^ ((full >> 64) as u64)
}

fn next_handle_keys() -> (u64, u64) {
    thread_local! {
        static NEXT: Cell<u64> = Cell::new(std::hash::RandomState::new().hash_one(0x6e6f_7661_u32));
    }
    let k = NEXT.with(|next| {
        let k = next.get();
        next.set(k.wrapping_add(0x9e37_79b9_7f4a_7c15));
        k
    });
    let k0 = folded_multiply(k ^ 0x243f_6a88_85a3_08d3, 0x1319_8a2e_0370_7345);
    let k1 = folded_multiply(k ^ 0xa409_3822_299f_31d0, 0x082e_fa98_ec4e_6c89);
    (k0, k1 | 1)
}

/// Random keys for one map keyed by engine handles. It is as large as
/// std's `RandomState`, which heap accounting measures maps with.
#[derive(Clone, Copy, Debug)]
pub struct HandleState {
    k0: u64,
    k1: u64,
}

impl Default for HandleState {
    #[inline]
    fn default() -> Self {
        let (k0, k1) = next_handle_keys();
        Self { k0, k1 }
    }
}

impl HandleState {
    /// The hash of one 64-bit word, as `hash_one(value)` would give it.
    #[inline(always)]
    pub(crate) fn hash_u64(&self, value: u64) -> u64 {
        folded_multiply(self.k0 ^ value, self.k1)
    }
}

impl BuildHasher for HandleState {
    type Hasher = HandleHasher;

    #[inline(always)]
    fn build_hasher(&self) -> HandleHasher {
        HandleHasher {
            acc: self.k0,
            multiplier: self.k1,
        }
    }
}

pub struct HandleHasher {
    acc: u64,
    multiplier: u64,
}

impl Hasher for HandleHasher {
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.acc
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks {
            let mut word = [0u8; 8];
            word.copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.write_u64(u64::from_le_bytes(word) ^ ((rest.len() as u64) << 59));
        }
    }

    #[inline(always)]
    fn write_u8(&mut self, i: u8) {
        self.write_u64(u64::from(i))
    }

    #[inline(always)]
    fn write_u16(&mut self, i: u16) {
        self.write_u64(u64::from(i))
    }

    #[inline(always)]
    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i))
    }

    #[inline(always)]
    fn write_u64(&mut self, i: u64) {
        self.acc = folded_multiply(self.acc ^ i, self.multiplier);
    }

    #[inline(always)]
    fn write_u128(&mut self, i: u128) {
        self.write_u64(i as u64);
        self.write_u64((i >> 64) as u64);
    }

    #[inline(always)]
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64)
    }

    #[inline(always)]
    fn write_i8(&mut self, i: i8) {
        self.write_u64(i as u64)
    }

    #[inline(always)]
    fn write_i16(&mut self, i: i16) {
        self.write_u64(i as u64)
    }

    #[inline(always)]
    fn write_i32(&mut self, i: i32) {
        self.write_u64(i as u64)
    }

    #[inline(always)]
    fn write_i64(&mut self, i: i64) {
        self.write_u64(i as u64)
    }

    #[inline(always)]
    fn write_isize(&mut self, i: isize) {
        self.write_u64(i as u64)
    }
}

/// Random SipHash keys for one table, such as the heap's string table.
pub(crate) struct RandomState(std::hash::RandomState);

impl RandomState {
    pub(crate) fn new() -> Self {
        Self(std::hash::RandomState::new())
    }

    #[inline]
    pub(crate) fn hash_one(&self, value: impl Hash) -> u64 {
        self.0.hash_one(value)
    }

    /// The hash of a string's bytes in the heap's string table. The bytes go
    /// to the hasher in blocks of [`TEXT_BLOCK`], whatever made the string,
    /// so that the state at a block boundary ([`TextHashState`]) continues
    /// into the hash of any longer string that starts with those bytes.
    #[inline]
    pub(crate) fn hash_text(&self, bytes: &[u8]) -> u64 {
        if bytes.len() < TEXT_BLOCK {
            return finish_text(self.0.build_hasher(), bytes);
        }
        self.hash_text_keeping_state(bytes).0
    }

    /// [`RandomState::hash_text`] of `bytes`, and the state a text that
    /// starts with them continues from.
    pub(crate) fn hash_text_keeping_state(&self, bytes: &[u8]) -> (u64, TextHashState) {
        let mut hasher = self.0.build_hasher();
        let mut blocks = bytes.chunks_exact(TEXT_BLOCK);
        for block in &mut blocks {
            hasher.write(block);
        }
        let state = TextHashState {
            blocks: hasher.clone(),
            hashed: bytes.len() - blocks.remainder().len(),
        };
        (finish_text(hasher, blocks.remainder()), state)
    }

    /// [`RandomState::hash_text`] of a text with `added` after it, from the
    /// text's `state` and its `tail`, the bytes after its hashed blocks
    /// (fewer than [`TEXT_BLOCK`]); and the state of the longer text.
    pub(crate) fn hash_text_appended(
        state: &TextHashState,
        tail: &[u8],
        added: &[u8],
    ) -> (u64, TextHashState) {
        debug_assert!(tail.len() < TEXT_BLOCK);
        let mut hasher = state.blocks.clone();
        let mut hashed = state.hashed;
        let mut block = [0u8; TEXT_BLOCK];
        let mut filled = tail.len().min(TEXT_BLOCK - 1);
        block[..filled].copy_from_slice(&tail[..filled]);
        let take = (TEXT_BLOCK - filled).min(added.len());
        block[filled..filled + take].copy_from_slice(&added[..take]);
        filled += take;
        let mut rest = &added[take..];
        if filled == TEXT_BLOCK {
            hasher.write(&block);
            hashed += TEXT_BLOCK;
            let mut blocks = rest.chunks_exact(TEXT_BLOCK);
            for whole in &mut blocks {
                hasher.write(whole);
                hashed += TEXT_BLOCK;
            }
            rest = blocks.remainder();
            filled = rest.len();
            block[..filled].copy_from_slice(rest);
        }
        let state = TextHashState {
            blocks: hasher.clone(),
            hashed,
        };
        (finish_text(hasher, &block[..filled]), state)
    }

    /// [`RandomState::hash_text`] of a text from its `state` and its `tail`.
    pub(crate) fn hash_text_from_state(state: &TextHashState, tail: &[u8]) -> u64 {
        finish_text(state.blocks.clone(), tail)
    }
}

/// Bytes a string hash takes at a time ([`RandomState::hash_text`]).
pub(crate) const TEXT_BLOCK: usize = 64;

/// A string hash after the whole blocks of a text's first `hashed` bytes:
/// what a text that starts with those bytes continues from, so that
/// appending to a long string hashes only what was appended.
#[derive(Debug, Clone)]
pub(crate) struct TextHashState {
    blocks: DefaultHasher,
    hashed: usize,
}

impl TextHashState {
    /// How many of the text's first bytes are in the state.
    pub(crate) fn hashed(&self) -> usize {
        self.hashed
    }
}

#[inline]
fn finish_text(mut hasher: DefaultHasher, tail: &[u8]) -> u64 {
    hasher.write(tail);
    // As a string's Hash ends: no string's bytes are a prefix of another's.
    hasher.write_u8(0xfe);
    hasher.finish()
}

fn process_random_state() -> &'static std::hash::RandomState {
    static STATE: OnceLock<std::hash::RandomState> = OnceLock::new();
    STATE.get_or_init(std::hash::RandomState::new)
}

/// A hasher with the process's random SipHash keys: equal keys hash equally
/// in every table of the process, and the keys are not known to a script.
pub(crate) struct AHasher(DefaultHasher);

impl Default for AHasher {
    #[inline]
    fn default() -> Self {
        Self(process_random_state().build_hasher())
    }
}

impl Hasher for AHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0.finish()
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes)
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.0.write_u8(i)
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.0.write_u32(i)
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0.write_u64(i)
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.0.write_usize(i)
    }
}
