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
    hash::{BuildHasher, DefaultHasher, Hasher},
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

/// Random SipHash keys for one table.
#[cfg(test)]
pub(crate) struct RandomState(std::hash::RandomState);

#[cfg(test)]
impl RandomState {
    pub(crate) fn new() -> Self {
        Self(std::hash::RandomState::new())
    }

    #[inline]
    pub(crate) fn hash_one(&self, value: impl std::hash::Hash) -> u64 {
        self.0.hash_one(value)
    }
}

/// The modulus of a string's hash: the prime 2^61 - 1.
const TEXT_PRIME: u64 = (1 << 61) - 1;

/// A product or sum of them under 2^127, brought under 2^62 + 64 with the
/// same remainder: 2^61 is 1 modulo the prime, so the three parts of 61
/// bits add up.
#[inline(always)]
fn reduce_text(wide: u128) -> u64 {
    let low = (wide as u64) & TEXT_PRIME;
    let middle = ((wide >> 61) as u64) & TEXT_PRIME;
    let high = (wide >> 122) as u64;
    low + middle + high
}

/// The remainder of a value under 2^63.
#[inline(always)]
fn canonical_text(value: u64) -> u64 {
    let folded = (value & TEXT_PRIME) + (value >> 61);
    if folded >= TEXT_PRIME {
        folded - TEXT_PRIME
    } else {
        folded
    }
}

/// The product of two values.
#[inline(always)]
pub(crate) fn multiply_text(a: u64, b: u64) -> u64 {
    canonical_text(reduce_text(u128::from(a) * u128::from(b)))
}

/// The hash of the heap's strings: of each text, a value that the value of
/// two texts put together follows from theirs, so that a string made by
/// concatenation is hashed without its text being read again.
///
/// The value is the polynomial whose coefficients are the bytes of the text
/// (each plus one, so that no byte is a zero coefficient and texts of
/// different lengths are different polynomials), at a random point `r` that
/// no script learns, modulo 2^61 - 1:
/// `(b0 + 1) * r^(n - 1) + (b1 + 1) * r^(n - 2) + ... + (b(n-1) + 1)`.
/// Two different texts of at most `n` bytes have the same value for at most
/// `n` of the 2^61 points, whatever the texts are.
///
/// The string table takes [`TextHasher::table_hash`] of the value: the value
/// with a second random key, mixed so that each of its bits changes each
/// bit of the hash about half the time. The mixing is one-to-one, so texts
/// have the same hash only where they have the same value, and values that
/// are near each other (texts that differ in their last byte have
/// consecutive ones) are not near each other in the table.
///
/// Neither step is a pseudorandom function as SipHash is: who could read
/// hashes could solve for the keys. A script cannot read them; it can only
/// time the table.
pub(crate) struct TextHasher {
    /// `(byte + 1) * r^(7 - place)` for a byte at each place of eight.
    places: Box<[[u64; 256]; 8]>,
    /// `r^n` for `n` up to 8.
    powers: [u64; 9],
    power_16: u64,
    power_24: u64,
    power_32: u64,
    /// `r^(2^n)`.
    squares: [u64; 64],
    mix_key: u64,
}

impl TextHasher {
    pub(crate) fn new() -> Self {
        let keys = std::hash::RandomState::new();
        let draw = |n: u8| keys.hash_one((0x7465_7874_u32, n));
        // Not 0, 1 or -1, at which texts of the same bytes in another order
        // would have the same value. Of 128 random bits, so that no point
        // is noticeably likelier than another.
        let wide = (u128::from(draw(0)) << 64) | u128::from(draw(1));
        let point = 2 + (wide % u128::from(TEXT_PRIME - 3)) as u64;
        Self::with_keys(point, draw(2))
    }

    fn with_keys(point: u64, mix_key: u64) -> Self {
        let mut powers = [1u64; 9];
        for n in 1..powers.len() {
            powers[n] = multiply_text(powers[n - 1], point);
        }
        let mut places = Box::new([[0u64; 256]; 8]);
        for (place, row) in places.iter_mut().enumerate() {
            let weight = powers[7 - place];
            for (byte, entry) in row.iter_mut().enumerate() {
                *entry = multiply_text(byte as u64 + 1, weight);
            }
        }
        let mut squares = [point; 64];
        for n in 1..squares.len() {
            squares[n] = multiply_text(squares[n - 1], squares[n - 1]);
        }
        let power_16 = multiply_text(powers[8], powers[8]);
        Self {
            places,
            powers,
            power_16,
            power_24: multiply_text(power_16, powers[8]),
            power_32: multiply_text(power_16, power_16),
            squares,
            mix_key,
        }
    }

    /// The sum of eight bytes' terms, each under the prime: it fits.
    #[inline(always)]
    fn word(&self, word: &[u8; 8]) -> u64 {
        let places = &*self.places;
        places[0][usize::from(word[0])]
            + places[1][usize::from(word[1])]
            + places[2][usize::from(word[2])]
            + places[3][usize::from(word[3])]
            + places[4][usize::from(word[4])]
            + places[5][usize::from(word[5])]
            + places[6][usize::from(word[6])]
            + places[7][usize::from(word[7])]
    }

    /// The value of a text.
    #[inline]
    pub(crate) fn text(&self, bytes: &[u8]) -> u64 {
        self.appended(0, bytes)
    }

    /// The value of a text of value `text` with `added` after it.
    pub(crate) fn appended(&self, text: u64, added: &[u8]) -> u64 {
        // Under 2^62 + 64 from one step to the next, and what a step adds
        // up is under 2^127: the value times a power under 2^61, three
        // words' sums (under 2^64) times such powers, and a fourth.
        let mut value = text;
        let (words, rest) = added.as_chunks::<8>();
        let mut fours = words.chunks_exact(4);
        for four in &mut fours {
            if let [first, second, third, fourth] = four {
                value = reduce_text(
                    u128::from(value) * u128::from(self.power_32)
                        + u128::from(self.word(first)) * u128::from(self.power_24)
                        + u128::from(self.word(second)) * u128::from(self.power_16)
                        + u128::from(self.word(third)) * u128::from(self.powers[8])
                        + u128::from(self.word(fourth)),
                );
            }
        }
        for word in fours.remainder() {
            value = reduce_text(
                u128::from(value) * u128::from(self.powers[8]) + u128::from(self.word(word)),
            );
        }
        if !rest.is_empty() {
            // The places of the last bytes of eight: the last byte's term
            // is the byte itself.
            let mut sum = 0u64;
            for (byte, place) in rest.iter().zip(&self.places[8 - rest.len()..]) {
                sum += place[usize::from(*byte)];
            }
            value = reduce_text(
                u128::from(value) * u128::from(self.powers[rest.len()]) + u128::from(sum),
            );
        }
        canonical_text(value)
    }

    /// `r^n`: what the value of a text is multiplied by when `n` bytes are
    /// put after it.
    pub(crate) fn power(&self, n: usize) -> u64 {
        if let Some(power) = self.powers.get(n) {
            return *power;
        }
        let mut power = 1u64;
        let mut left = n;
        for square in &self.squares {
            if left == 0 {
                break;
            }
            if left & 1 == 1 {
                power = multiply_text(power, *square);
            }
            left >>= 1;
        }
        power
    }

    /// The value of a text of value `left` followed by one of value `right`
    /// and `right_len` bytes.
    pub(crate) fn joined(&self, left: u64, right: u64, right_len: usize) -> u64 {
        Self::joined_by(left, right, self.power(right_len))
    }

    /// The value of a text of value `left` followed by one of value `right`,
    /// for whose length `right_power` is [`TextHasher::power`].
    #[inline]
    pub(crate) fn joined_by(left: u64, right: u64, right_power: u64) -> u64 {
        canonical_text(reduce_text(
            u128::from(left) * u128::from(right_power) + u128::from(right),
        ))
    }

    /// The hash in the string table of a text of this value.
    #[inline(always)]
    pub(crate) fn table_hash(&self, text: u64) -> u64 {
        // The finishing steps of SplitMix64.
        let mut mixed = text ^ self.mix_key;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }
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

#[cfg(test)]
mod text_tests {
    use super::*;

    /// The definition, a byte at a time with the remainder taken by division.
    fn slowly(hasher: &TextHasher, bytes: &[u8]) -> u64 {
        let point = u128::from(hasher.powers[1]);
        let prime = u128::from(TEXT_PRIME);
        let mut value = 0u128;
        for &byte in bytes {
            value = (value * point + u128::from(byte) + 1) % prime;
        }
        value as u64
    }

    fn sample(len: usize, seed: usize) -> Vec<u8> {
        (0..len)
            .map(|at| match (at + seed) % 11 {
                0 => 0,
                1 => 255,
                _ => ((at * 131 + seed * 31 + at / 7) % 256) as u8,
            })
            .collect()
    }

    #[test]
    fn the_value_of_a_text_is_the_polynomial_of_its_bytes() {
        // Points at the ends of the range too, where sums come nearest to
        // what the steps have room for.
        let hashers = [
            TextHasher::new(),
            TextHasher::new(),
            TextHasher::with_keys(2, 0),
            TextHasher::with_keys(TEXT_PRIME - 2, u64::MAX),
        ];
        for hasher in &hashers {
            assert!((2..TEXT_PRIME - 1).contains(&hasher.powers[1]));
            for len in 0..200 {
                let bytes = sample(len, len);
                assert_eq!(hasher.text(&bytes), slowly(hasher, &bytes), "{len}");
            }
            for bytes in [vec![255u8; 4099], vec![0u8; 4099], sample(70001, 5)] {
                let value = hasher.text(&bytes);
                assert_eq!(value, slowly(hasher, &bytes));
                assert!(value < TEXT_PRIME);
            }
        }
    }

    #[test]
    fn the_value_of_two_texts_together_follows_from_theirs() {
        let hasher = TextHasher::new();
        for len in [0usize, 1, 7, 8, 9, 31, 32, 33, 63, 64, 65, 200, 1000, 4097] {
            let bytes = sample(len, 3);
            let whole = hasher.text(&bytes);
            for at in (0..=len).filter(|at| *at < 70 || at % 61 == 0 || len - at < 70) {
                let (left, right) = bytes.split_at(at);
                let (left_value, right_value) = (hasher.text(left), hasher.text(right));
                assert_eq!(hasher.appended(left_value, right), whole, "{len} {at}");
                assert_eq!(
                    hasher.joined(left_value, right_value, right.len()),
                    whole,
                    "{len} {at}"
                );
            }
        }
        // A power by its bits is the power by its steps.
        let mut power = 1u64;
        for n in 0..300 {
            assert_eq!(hasher.power(n), power);
            power = multiply_text(power, hasher.powers[1]);
        }
        let big = (1usize << 31) + 12345;
        assert_eq!(
            hasher.power(big),
            multiply_text(hasher.power(1 << 31), hasher.power(12345))
        );
    }

    #[test]
    fn texts_that_differ_have_other_values_and_other_places() {
        let hasher = TextHasher::new();
        // What a hash without a secret point, or one that took a byte for
        // its coefficient, would not tell apart.
        let pairs: [(&[u8], &[u8]); 6] = [
            (b"", b"\0"),
            (b"\0", b"\0\0"),
            (b"abc", b"\0abc"),
            (b"abc", b"acb"),
            (b"abcdefgh12345678", b"12345678abcdefgh"),
            (b"aaaaaaaab", b"aaaaaaaba"),
        ];
        for (one, other) in pairs {
            assert_ne!(hasher.text(one), hasher.text(other), "{one:?} {other:?}");
        }
        // Texts that differ in the last byte have consecutive values; the
        // table's hash of them is spread over its places and its tags.
        let mut places = std::collections::HashSet::new();
        let mut tags = std::collections::HashSet::new();
        let mut text = *b"the same start, then ?";
        for last in 0..=255u8 {
            *text.last_mut().unwrap() = last;
            let hash = hasher.table_hash(hasher.text(&text));
            places.insert(hash & 0xffff);
            tags.insert(hash >> 57);
        }
        assert!(places.len() > 240, "{}", places.len());
        assert!(tags.len() > 80, "{}", tags.len());
    }
}
