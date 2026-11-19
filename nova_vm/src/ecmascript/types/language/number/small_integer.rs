// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::ecmascript::{SmallBigInt, number_value};
use crate::width::usize_from_i64;

/// ### [6.1.6.1 The Number Type](https://tc39.es/ecma262/#sec-ecmascript-language-types-number-type)
///
/// Stack-allocated integer [`Number`] data. Internally, the value is a 54-bit
/// signed integer stored in 56 bits, filling in the entire safe integer range.
///
/// [`Number`]: crate::ecmascript::Number
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SmallInteger {
    pub(crate) data: [u8; 7],
}
number_value!(SmallInteger, Integer);

/// The u64 whose bytes in memory are a zero and then `data`: what
/// `u64::from_ne_bytes([0, data[0], …, data[6]])` gives.
///
/// Put together on a little-endian target from the three pieces that seven
/// bytes are read from memory as (four, two and one), with shifts: written
/// as the array of eight bytes, every read of a small Number in the
/// interpreter became a vector shuffle of a dozen instructions.
#[inline(always)]
pub(super) const fn seven_bytes_after_zero(data: [u8; 7]) -> u64 {
    if cfg!(target_endian = "little") {
        let [a, b, c, d, e, f, g] = data;
        let low = u32::from_le_bytes([a, b, c, d]) as u64;
        let middle = u16::from_le_bytes([e, f]) as u64;
        ((g as u64) << 56) | (middle << 40) | (low << 8)
    } else {
        u64::from_ne_bytes([
            0, data[0], data[1], data[2], data[3], data[4], data[5], data[6],
        ])
    }
}

impl core::fmt::Debug for SmallInteger {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", Into::<i64>::into(*self))
    }
}

impl core::hash::Hash for SmallInteger {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.into_i64().hash(state);
    }
}

impl SmallInteger {
    /// Minimum value that can be represented as a SmallInteger.
    ///
    /// This corresponds with `Number.MIN_SAFE_INTEGER`.
    pub const MIN: i64 = -(2i64.pow(53)) + 1;
    /// Maximum value that can be represented as a SmallInteger.
    ///
    /// This corresponds with `Number.MAX_SAFE_INTEGER`.
    pub const MAX: i64 = 2i64.pow(53) - 1;

    /// Returns the contained value as an i64.
    #[inline]
    pub const fn into_i64(self) -> i64 {
        let SmallInteger { data } = self;

        let number = seven_bytes_after_zero(data) as i64;

        if cfg!(target_endian = "little") {
            number >> 8
        } else {
            number << 8 >> 8
        }
    }

    /// Returns the `0` value.
    pub const fn zero() -> Self {
        Self {
            data: [0, 0, 0, 0, 0, 0, 0],
        }
    }

    /// Encode an i64 as a SmallInteger without a range check.
    ///
    /// ## Correctness
    ///
    /// If the value is outside the SmallInteger range, the method panics
    /// in debug mode. In release mode, data may be lost and an out of range
    /// value may be created.
    pub(crate) fn from_i64_unchecked(value: i64) -> Self {
        debug_assert!((Self::MIN..=Self::MAX).contains(&value));
        let bytes = i64::to_ne_bytes(value);

        let data = if cfg!(target_endian = "little") {
            [
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
            ]
        } else {
            [
                bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]
        };

        Self { data }
    }

    /// Convert a SmallBigInt as SmallInteger without a range check.
    ///
    /// ## Correctness
    ///
    /// If the value is outside the SmallInteger range, the method panics
    /// in debug mode. In release mode, an out of range value is created.
    pub(crate) fn from_small_bigint_unchecked(value: SmallBigInt) -> Self {
        debug_assert!((Self::MIN..=Self::MAX).contains(&value.into_i64()));
        Self { data: value.data }
    }
}

impl core::ops::Neg for SmallInteger {
    type Output = Self;

    /// ## Panics
    /// - If the negation overflows.
    fn neg(self) -> Self::Output {
        Self::from_i64_unchecked(-self.into_i64())
    }
}

impl core::ops::Not for SmallInteger {
    type Output = Self;
    fn not(self) -> Self::Output {
        Self::from_i64_unchecked(!self.into_i64())
    }
}

impl TryFrom<i64> for SmallInteger {
    type Error = ();
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self::from_i64_unchecked(value))
        } else {
            Err(())
        }
    }
}

impl TryFrom<isize> for SmallInteger {
    type Error = ();
    fn try_from(value: isize) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&(value as i64)) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<f64> for SmallInteger {
    type Error = ();
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if value.fract() == 0.0 && (Self::MIN..=Self::MAX).contains(&(value as i64)) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<f32> for SmallInteger {
    type Error = ();
    fn try_from(value: f32) -> Result<Self, Self::Error> {
        if value.fract() == 0.0 && (Self::MIN..=Self::MAX).contains(&(value as i64)) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<i128> for SmallInteger {
    type Error = ();
    fn try_from(value: i128) -> Result<Self, Self::Error> {
        if (Self::MIN as i128..=Self::MAX as i128).contains(&value) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<u64> for SmallInteger {
    type Error = ();
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value <= (Self::MAX as u64) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<usize> for SmallInteger {
    type Error = ();
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value <= usize_from_i64(Self::MAX) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

macro_rules! from_numeric_type {
    ($numtype:ty) => {
        // Checking at compile-time that $numtype fully fits within the range.
        const _: () = {
            assert!(
                <$numtype>::MIN as i64 >= SmallInteger::MIN,
                concat!(
                    stringify!($numtype),
                    " is outside of the SmallInteger range (min)"
                )
            );
            assert!(
                <$numtype>::MAX as i64 <= SmallInteger::MAX,
                concat!(
                    stringify!($numtype),
                    " is outside of the SmallInteger range (max)"
                )
            );
        };
        impl From<$numtype> for SmallInteger {
            fn from(value: $numtype) -> Self {
                Self::from_i64_unchecked(i64::from(value))
            }
        }
    };
}
from_numeric_type!(u8);
from_numeric_type!(i8);
from_numeric_type!(u16);
from_numeric_type!(i16);
from_numeric_type!(u32);
from_numeric_type!(i32);

impl From<SmallInteger> for i64 {
    fn from(value: SmallInteger) -> Self {
        value.into_i64()
    }
}

#[test]
fn valid_small_integers() {
    assert_eq!(0i64, SmallInteger::from(0).into_i64());
    assert_eq!(5i64, SmallInteger::from(5).into_i64());
    assert_eq!(23i64, SmallInteger::from(23).into_i64());
    assert_eq!(
        SmallInteger::MAX,
        SmallInteger::try_from(SmallInteger::MAX)
            .unwrap()
            .into_i64()
    );

    assert_eq!(-5i64, SmallInteger::from(-5).into_i64());
    assert_eq!(-59i64, SmallInteger::from(-59).into_i64());
    assert_eq!(
        SmallInteger::MIN,
        SmallInteger::try_from(SmallInteger::MIN)
            .unwrap()
            .into_i64()
    );
}

#[test]
fn invalid_small_integers() {
    assert_eq!(SmallInteger::try_from(SmallInteger::MAX + 1), Err(()));
    assert_eq!(SmallInteger::try_from(i64::MAX), Err(()));
    assert_eq!(SmallInteger::try_from(SmallInteger::MIN - 1), Err(()));
    assert_eq!(SmallInteger::try_from(i64::MIN), Err(()));
}
