// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::ecmascript::{SmallInteger, numeric_value};

/// ### [6.1.6.2 The BigInt Type](https://tc39.es/ecma262/#sec-ecmascript-language-types-bigint-type)
///
/// Primitive BigInt value allocated on the stack.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SmallBigInt {
    pub(crate) data: [u8; 7],
}
numeric_value!(SmallBigInt);

impl core::fmt::Debug for SmallBigInt {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", Into::<i64>::into(*self))
    }
}

impl core::hash::Hash for SmallBigInt {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.into_i64().hash(state);
    }
}

impl SmallBigInt {
    /// Minimum value that can be represented as a SmallBigInt.
    pub const MIN: i64 = -(2i64.pow(55));
    /// Maximum value that can be represented as a SmallBigInt
    pub const MAX: i64 = 2i64.pow(55) - 1;

    /// Returns true if SmallBigInt equals zero.
    pub const fn is_zero(self) -> bool {
        let Self {
            data: [a, b, c, d, e, f, g],
        } = self;
        a == 0 && b == 0 && c == 0 && d == 0 && e == 0 && f == 0 && g == 0
    }

    /// Returns the contained value as an i64.
    #[inline]
    pub const fn into_i64(self) -> i64 {
        let SmallBigInt { data } = self;

        let number = i64::from_ne_bytes([
            0, data[0], data[1], data[2], data[3], data[4], data[5], data[6],
        ]);

        if cfg!(target_endian = "little") {
            number >> 8
        } else {
            number << 8 >> 8
        }
    }

    /// Returns the `0n` value.
    pub const fn zero() -> SmallBigInt {
        Self {
            data: [0, 0, 0, 0, 0, 0, 0],
        }
    }

    /// Encode an i64 as a SmallBigInt without a range check.
    ///
    /// ## Correctness
    ///
    /// If the value is outside the SmallBigInt range, the method panics in
    /// debug mode. In release mode, data is lost.
    pub(crate) fn from_i64_unchecked(value: i64) -> SmallBigInt {
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
}

impl core::ops::Neg for SmallBigInt {
    type Output = Self;

    /// ## Correctness
    /// - If the negation overflows, data is lost.
    fn neg(self) -> Self::Output {
        Self::from_i64_unchecked(-self.into_i64())
    }
}

impl core::ops::Not for SmallBigInt {
    type Output = Self;
    fn not(self) -> Self::Output {
        Self::from_i64_unchecked(!self.into_i64())
    }
}

impl From<SmallInteger> for SmallBigInt {
    fn from(value: SmallInteger) -> Self {
        Self { data: value.data }
    }
}

impl TryFrom<SmallBigInt> for SmallInteger {
    type Error = ();

    fn try_from(value: SmallBigInt) -> Result<Self, Self::Error> {
        Self::try_from(value.into_i64())
    }
}

impl TryFrom<i64> for SmallBigInt {
    type Error = ();
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self::from_i64_unchecked(value))
        } else {
            Err(())
        }
    }
}

impl TryFrom<isize> for SmallBigInt {
    type Error = ();
    fn try_from(value: isize) -> Result<Self, Self::Error> {
        if (Self::MIN..=Self::MAX).contains(&(value as i64)) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<f64> for SmallBigInt {
    type Error = ();
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if value.fract() == 0.0 && (Self::MIN..=Self::MAX).contains(&(value as i64)) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<f32> for SmallBigInt {
    type Error = ();
    fn try_from(value: f32) -> Result<Self, Self::Error> {
        if value.fract() == 0.0 && (Self::MIN..=Self::MAX).contains(&(value as i64)) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<i128> for SmallBigInt {
    type Error = ();
    fn try_from(value: i128) -> Result<Self, Self::Error> {
        if (Self::MIN as i128..=Self::MAX as i128).contains(&value) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<u64> for SmallBigInt {
    type Error = ();
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value <= (Self::MAX as u64) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<u128> for SmallBigInt {
    type Error = ();
    fn try_from(value: u128) -> Result<Self, Self::Error> {
        if (Self::MIN as u128..=Self::MAX as u128).contains(&value) {
            Ok(Self::from_i64_unchecked(value as i64))
        } else {
            Err(())
        }
    }
}

impl TryFrom<usize> for SmallBigInt {
    type Error = ();
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        if value <= (Self::MAX as usize) {
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
                <$numtype>::MIN as i64 >= SmallBigInt::MIN,
                concat!(
                    stringify!($numtype),
                    " is outside of the SmallBigInt range (min)"
                )
            );
            assert!(
                <$numtype>::MAX as i64 <= SmallBigInt::MAX,
                concat!(
                    stringify!($numtype),
                    " is outside of the SmallBigInt range (max)"
                )
            );
        };
        impl From<$numtype> for SmallBigInt {
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

impl From<SmallBigInt> for i64 {
    fn from(value: SmallBigInt) -> Self {
        value.into_i64()
    }
}

#[test]
fn valid_small_integers() {
    assert_eq!(0i64, SmallBigInt::from(0).into_i64());
    assert_eq!(5i64, SmallBigInt::from(5).into_i64());
    assert_eq!(23i64, SmallBigInt::from(23).into_i64());
    assert_eq!(
        SmallBigInt::MAX,
        SmallBigInt::try_from(SmallBigInt::MAX).unwrap().into_i64()
    );

    assert_eq!(-5i64, SmallBigInt::from(-5).into_i64());
    assert_eq!(-59i64, SmallBigInt::from(-59).into_i64());
    assert_eq!(
        SmallBigInt::MIN,
        SmallBigInt::try_from(SmallBigInt::MIN).unwrap().into_i64()
    );
}

#[test]
fn invalid_small_integers() {
    assert_eq!(SmallBigInt::try_from(SmallBigInt::MAX + 1), Err(()));
    assert_eq!(SmallBigInt::try_from(i64::MAX), Err(()));
    assert_eq!(SmallBigInt::try_from(SmallBigInt::MIN - 1), Err(()));
    assert_eq!(SmallBigInt::try_from(i64::MIN), Err(()));
}
