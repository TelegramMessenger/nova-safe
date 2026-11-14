// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Hashing of keys a script chooses (property names, Map and Set keys,
//! strings), with std's SipHash-1-3 under random keys so that a script
//! cannot construct colliding keys.
//!
//! The names are those of the aHash types this replaces, so that their
//! users only changed an import.

use std::{
    hash::{BuildHasher, DefaultHasher, Hash, Hasher},
    sync::OnceLock,
};

pub(crate) type AHashMap<K, V> = std::collections::HashMap<K, V, std::hash::RandomState>;
pub(crate) type AHashSet<K> = std::collections::HashSet<K, std::hash::RandomState>;

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
