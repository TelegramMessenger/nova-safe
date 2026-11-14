//! The hasher of identifier-keyed maps.
//!
//! nova-safe: a script chooses every identifier it parses, so the maps keyed by them hash the
//! whole name with std's SipHash-1-3 under random keys chosen once per process. Upstream's
//! hash was unkeyed and sampled at most 16 bytes of a name, so a script could give thousands of
//! names one hash and make parsing quadratic.

use std::{
    hash::{BuildHasher, DefaultHasher, Hasher, RandomState},
    sync::OnceLock,
};

fn process_state() -> &'static RandomState {
    static STATE: OnceLock<RandomState> = OnceLock::new();
    STATE.get_or_init(RandomState::new)
}

/// A [`BuildHasher`] for maps keyed by identifier names (`Ident`, `str` and names that hash
/// like their `str`): SipHash-1-3 under the process's random keys.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentBuildHasher;

impl BuildHasher for IdentBuildHasher {
    type Hasher = IdentHasher;

    #[inline]
    fn build_hasher(&self) -> Self::Hasher {
        IdentHasher(process_state().build_hasher())
    }
}

/// The [`Hasher`] of [`IdentBuildHasher`].
#[derive(Debug, Clone)]
pub struct IdentHasher(DefaultHasher);

impl Hasher for IdentHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.0.write_u8(i);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0.write_u64(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.0.write_usize(i);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0.finish()
    }
}

#[cfg(test)]
mod test {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};

    use super::IdentBuildHasher;

    #[test]
    fn equal_names_hash_equally() {
        let a = IdentBuildHasher;
        let b = IdentBuildHasher;
        assert_eq!(a.hash_one("fooBar"), b.hash_one(String::from("fooBar").as_str()));
    }

    #[test]
    fn the_whole_name_is_hashed() {
        let h = IdentBuildHasher;
        let base = "ABCDaaaaEFGHIJJJJJJJJJJJJJJJJJJJJJJJJJJJ";
        let mut seen = std::collections::HashSet::new();
        for i in 0..base.len() {
            let mut bytes = base.as_bytes().to_vec();
            bytes[i] = b'z';
            let name = String::from_utf8(bytes).unwrap();
            assert!(seen.insert(h.hash_one(name.as_str())), "byte {i} does not reach the hash");
        }
    }

    #[test]
    fn the_keys_are_not_fixed() {
        let ours = IdentBuildHasher.hash_one("fooBar");
        let unkeyed = BuildHasherDefault::<DefaultHasher>::default();
        assert_ne!(ours, unkeyed.hash_one("fooBar"));
    }
}
