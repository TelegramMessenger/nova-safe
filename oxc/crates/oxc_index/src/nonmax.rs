//! `NonMaxU32`: a `u32` that is never `u32::MAX`, so `Option<NonMaxU32>` is 4 bytes.
//!
//! Replaces the `nonmax` crate with the same representation (`value ^ u32::MAX` in a `NonZeroU32`),
//! so hashes and the niche are unchanged, without `unsafe`.

use core::{cmp::Ordering, fmt, hash::Hash, num::NonZeroU32};

/// An integer that is known not to equal `u32::MAX`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NonMaxU32(NonZeroU32);

impl NonMaxU32 {
    /// The largest value, `u32::MAX - 1`.
    pub const MAX: Self = Self::new_unchecked(u32::MAX - 1);

    /// The smallest value, `0`.
    pub const ZERO: Self = Self::new_unchecked(0);

    /// Creates a new `NonMaxU32` if `value` is not `u32::MAX`.
    #[inline]
    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value ^ u32::MAX) {
            Some(inner) => Some(Self(inner)),
            None => None,
        }
    }

    /// Creates a new `NonMaxU32`.
    ///
    /// # Panics
    /// Panics if `value` is `u32::MAX` (the `nonmax` crate's version was undefined behaviour).
    #[inline]
    pub const fn new_unchecked(value: u32) -> Self {
        match Self::new(value) {
            Some(nonmax) => nonmax,
            None => panic!("NonMaxU32 cannot hold u32::MAX"),
        }
    }

    /// Returns the value as a `u32`.
    #[inline]
    pub const fn get(&self) -> u32 {
        self.0.get() ^ u32::MAX
    }
}

impl Default for NonMaxU32 {
    fn default() -> Self {
        Self::ZERO
    }
}

impl Ord for NonMaxU32 {
    fn cmp(&self, other: &Self) -> Ordering {
        self.get().cmp(&other.get())
    }
}

impl PartialOrd for NonMaxU32 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Debug for NonMaxU32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.get(), f)
    }
}

impl fmt::Display for NonMaxU32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.get(), f)
    }
}

impl From<NonMaxU32> for u32 {
    fn from(value: NonMaxU32) -> Self {
        value.get()
    }
}

impl TryFrom<u32> for NonMaxU32 {
    type Error = TryFromIntError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(TryFromIntError(()))
    }
}

/// The error returned by `NonMaxU32::try_from(u32::MAX)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TryFromIntError(());

impl fmt::Display for TryFromIntError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("out of range integral type conversion attempted")
    }
}

#[cfg(test)]
mod test {
    use super::NonMaxU32;

    #[test]
    fn nonmax_u32() {
        assert_eq!(size_of::<Option<NonMaxU32>>(), 4);
        assert_eq!(NonMaxU32::new(u32::MAX), None);
        assert_eq!(NonMaxU32::new(7).map(|v| v.get()), Some(7));
        assert_eq!(NonMaxU32::MAX.get(), u32::MAX - 1);
        assert!(NonMaxU32::new(1) < NonMaxU32::new(2));
        assert!(NonMaxU32::new(u32::MAX - 1) > NonMaxU32::new(0));
    }

    #[test]
    #[should_panic(expected = "NonMaxU32 cannot hold u32::MAX")]
    fn new_unchecked_max() {
        let _ = NonMaxU32::new_unchecked(u32::MAX);
    }
}
