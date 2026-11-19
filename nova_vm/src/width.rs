// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Conversions of the specification's 64-bit integers (lengths, indexes and
//! counts of up to 2^53 - 1) to the pointer-sized integers that index and
//! size memory.
//!
//! On a 64-bit target each of these is exactly the `as` cast it replaces. On a
//! 32-bit target `as` would drop the high bits, and a length of 2^32 + 5 would
//! become 5: a wrong answer where the 64-bit build refuses the request or
//! clamps it to the length of what it indexes. There a value that does not
//! fit becomes the nearest limit of the target type instead, which every
//! caller already treats as "more than there can be".

/// `value as usize`; on a target narrower than 64 bits, `usize::MAX` for a
/// value that does not fit. That includes a negative one, which the 64-bit
/// cast turns into a value above every possible length as well.
#[inline(always)]
pub(crate) const fn usize_from_i64(value: i64) -> usize {
    if usize::BITS >= 64 || (0 <= value && value <= usize::MAX as i64) {
        value as usize
    } else {
        usize::MAX
    }
}

/// `value as usize`; on a target narrower than 64 bits, `usize::MAX` for a
/// value that does not fit.
#[inline(always)]
pub(crate) const fn usize_from_u64(value: u64) -> usize {
    if value <= usize::MAX as u64 {
        value as usize
    } else {
        usize::MAX
    }
}

/// `value as isize`; on a target narrower than 64 bits, `isize::MIN` or
/// `isize::MAX` for a value that does not fit.
#[inline(always)]
pub(crate) const fn isize_from_i64(value: i64) -> isize {
    if value > isize::MAX as i64 {
        isize::MAX
    } else if value < isize::MIN as i64 {
        isize::MIN
    } else {
        value as isize
    }
}

/// `value as isize`; on a target narrower than 64 bits, `isize::MAX` for a
/// value that does not fit.
#[inline(always)]
pub(crate) const fn isize_from_u64(value: u64) -> isize {
    if usize::BITS >= 64 || value <= isize::MAX as u64 {
        value as isize
    } else {
        isize::MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_that_fit_convert_exactly() {
        assert_eq!(usize_from_i64(0), 0);
        assert_eq!(usize_from_i64(i32::MAX as i64), i32::MAX as usize);
        assert_eq!(usize_from_u64(u32::MAX as u64), u32::MAX as usize);
        assert_eq!(isize_from_i64(-7), -7);
        assert_eq!(isize_from_i64(i32::MIN as i64), i32::MIN as isize);
        assert_eq!(isize_from_u64(i32::MAX as u64), i32::MAX as isize);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_64_bit_target_keeps_the_plain_cast() {
        for value in [-1, i64::MIN, i64::MAX, (1 << 53) - 1] {
            assert_eq!(usize_from_i64(value), value as usize);
            assert_eq!(isize_from_i64(value), value as isize);
        }
        for value in [u64::MAX, 1 << 63, (1 << 53) - 1] {
            assert_eq!(usize_from_u64(value), value as usize);
            assert_eq!(isize_from_u64(value), value as isize);
        }
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn a_32_bit_target_saturates() {
        assert_eq!(usize_from_i64((1 << 32) + 5), usize::MAX);
        assert_eq!(usize_from_i64(-1), usize::MAX);
        assert_eq!(usize_from_u64((1 << 32) + 5), usize::MAX);
        assert_eq!(isize_from_i64((1 << 32) + 5), isize::MAX);
        assert_eq!(isize_from_i64(-(1 << 40)), isize::MIN);
        assert_eq!(isize_from_u64(1 << 31), isize::MAX);
    }
}
