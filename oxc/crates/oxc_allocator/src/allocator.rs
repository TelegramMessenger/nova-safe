use std::cell::{Cell, OnceCell};

const FIRST_CHUNK_BITS: u32 = 6;
const CHUNKS: usize = 40;

/// Owner of the strings the parser creates (unescaped identifiers, cooked
/// template strings, concatenations).
///
/// Every string lives in its own `OnceCell` slot. A slot is written once and
/// never moved: chunk `k` holds `2^(k + FIRST_CHUNK_BITS)` slots and is
/// created once, so a `&str` from [`Allocator::alloc_str`] stays valid while
/// the allocator is borrowed. Nodes of the syntax tree are owned by their
/// parents ([`crate::Box`], [`crate::Vec`]) and do not live here.
pub struct Allocator {
    chunks: [OnceCell<std::boxed::Box<[OnceCell<std::boxed::Box<str>>]>>; CHUNKS],
    next: Cell<usize>,
    bytes: Cell<usize>,
}

impl Default for Allocator {
    fn default() -> Self {
        Self { chunks: std::array::from_fn(|_| OnceCell::new()), next: Cell::new(0), bytes: Cell::new(0) }
    }
}

impl std::fmt::Debug for Allocator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Allocator").field("strings", &self.next.get()).finish()
    }
}

fn slot_position(index: usize) -> (usize, usize) {
    let biased = index + (1 << FIRST_CHUNK_BITS);
    let chunk = (usize::BITS - 1 - biased.leading_zeros() - FIRST_CHUNK_BITS) as usize;
    let offset = biased - (1 << (chunk as u32 + FIRST_CHUNK_BITS));
    (chunk, offset)
}

impl Allocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(_capacity: usize) -> Self {
        Self::default()
    }

    fn store(&self, value: std::boxed::Box<str>) -> &str {
        let index = self.next.get();
        let (chunk, offset) = slot_position(index);
        let slots = self
            .chunks
            .get(chunk)
            .expect("string arena exhausted")
            .get_or_init(|| (0..1usize << (chunk as u32 + FIRST_CHUNK_BITS)).map(|_| OnceCell::new()).collect());
        let slot = &slots[offset];
        self.next.set(index + 1);
        self.bytes.set(self.bytes.get() + value.len());
        if slot.set(value).is_err() {
            unreachable!("string arena slot written twice");
        }
        slot.get().expect("slot was just set")
    }

    /// Copies `src` into the arena.
    pub fn alloc_str<'alloc>(&'alloc self, src: &str) -> &'alloc str {
        if src.is_empty() {
            return "";
        }
        self.store(src.into())
    }

    /// Moves an owned string into the arena.
    pub fn alloc_string<'alloc>(&'alloc self, src: String) -> &'alloc str {
        if src.is_empty() {
            return "";
        }
        self.store(src.into_boxed_str())
    }

    pub fn alloc_concat_strs_array<'a, const N: usize>(&'a self, strings: [&str; N]) -> &'a str {
        self.alloc_string(strings.concat())
    }

    /// Drops every string. Needs `&mut self`, so no `&str` handed out before
    /// can still be alive.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn capacity(&self) -> usize {
        self.bytes.get()
    }

    pub fn used_bytes(&self) -> usize {
        self.bytes.get()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn slots_are_contiguous() {
        let mut expected = (0, 0);
        for index in 0..10_000 {
            assert_eq!(slot_position(index), expected, "{index}");
            expected.1 += 1;
            if expected.1 == 1 << (expected.0 as u32 + FIRST_CHUNK_BITS) {
                expected = (expected.0 + 1, 0);
            }
        }
    }

    #[test]
    fn strings_stay_valid() {
        let allocator = Allocator::new();
        let all: std::vec::Vec<&str> = (0..5000).map(|i| allocator.alloc_string(format!("s{i}"))).collect();
        for (i, s) in all.iter().enumerate() {
            assert_eq!(*s, format!("s{i}"));
        }
        assert_eq!(allocator.alloc_concat_strs_array(["a", "", "bc"]), "abc");
    }
}
