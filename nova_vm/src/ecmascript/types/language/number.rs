// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod data;
mod small_f64;
mod small_integer;
mod text;

pub(crate) use data::*;
pub use small_f64::*;
pub use small_integer::*;
pub(crate) use text::*;

use super::{
    Numeric, Primitive, String, Value,
    value::{FLOAT_DISCRIMINANT, INTEGER_DISCRIMINANT, NUMBER_DISCRIMINANT},
};
use crate::{
    ecmascript::{Agent, numeric_handle, to_int32_number, to_uint32_number},
    engine::{Bindable, HeapRootData, HeapRootRef, NoGcScope, Rootable},
    heap::{
        ArenaAccess, BaseIndex, CompactionLists, CreateHeapData, Heap, HeapMarkAndSweep,
        NumberHeapAccess, WorkQueues, arena_vec_access,
    },
};

use num_traits::{PrimInt, Zero};

/// ### [6.1.6.1 The Number Type](https://tc39.es/ecma262/#sec-ecmascript-language-types-number-type)
///
/// The ECMAScript _Number type_ is officially a double-precision floating point
/// IEEE 754-2019 value, but with the added benefit of acting in a special way
/// under bitwise operations wherein the value gets narrowed down into a 32-bit
/// signed integer.
///
/// In Nova JavaScript engine, a [`Number`] is split into one of three options:
/// it is either a full double-precision floating point value allocated on the
/// heap, or it is a 54-bit signed integer stored on the stack, or it is a full
/// double-precision floating point value with 8 trailing zeroes stored in 56
/// bits on the stack with the 8 trailing zeroes chopped off.
///
/// [`Number`]: Number
#[derive(Clone, Copy, PartialEq, Bindable)]
#[repr(u8)]
pub enum Number<'a> {
    /// f64 on the heap. Accessing the data can only be done through the Agent.
    Number(HeapNumber<'a>) = NUMBER_DISCRIMINANT,
    /// 54-bit signed integer on the stack.
    Integer(SmallInteger) = INTEGER_DISCRIMINANT,
    /// f64 with 8 trailing zeroes on the stack. The trailing zeroes are chopped
    /// off to fit into 56 bits.
    SmallF64(SmallF64) = FLOAT_DISCRIMINANT,
}
impl<'a> Number<'a> {
    /// Allocate a 64-bit floating point number onto the Agent heap
    ///
    /// ## Correctness
    ///
    /// The number being allocated must not be representable
    /// as a SmallInteger or f32. All stack-allocated numbers must be
    /// inequal to any heap-allocated number.
    #[inline]
    fn alloc_number<'gc>(heap: &mut Heap, number: f64) -> HeapNumber<'gc> {
        debug_assert!(
            SmallInteger::try_from(number).is_err() && SmallF64::try_from(number).is_err()
        );
        heap.numbers.push(number.into());
        heap.alloc_counter += core::mem::size_of::<Option<NumberHeapData>>();
        HeapNumber(BaseIndex::last(&heap.numbers))
    }

    /// Create a new Number from an f64.
    #[inline]
    pub fn from_f64(agent: &mut Agent, value: f64, gc: NoGcScope<'a, '_>) -> Self {
        if Self::is_heap_f64(value) {
            let id = Self::alloc_number(&mut agent.heap, value);
            return Number::Number(id.unbind().bind(gc));
        }
        if let Ok(value) = Number::try_from(value) {
            value
        } else {
            let id = Self::alloc_number(&mut agent.heap, value);
            Number::Number(id.unbind().bind(gc))
        }
    }

    /// Whether `value` is surely a Number of the heap: what nearly every
    /// result of arithmetic on fractions is, told by two tests where the
    /// general way ([`Number::try_from`]) asks whether it is finite, whole,
    /// in the range of the safe integers and not -0 before it asks the
    /// one thing that says it here.
    ///
    /// A double fits a Value only with a low byte of zeros
    /// ([`SmallF64`]); one that has another low byte is an integer Number
    /// or a Number of the heap. It is an integer Number where it is whole
    /// and in the range of the safe integers: the conversion to an integer
    /// and back gives the double itself only for a whole one (it gives
    /// another for NaN and for the infinities, which have no such byte
    /// anyway, and -0 has a low byte of zeros).
    ///
    /// The general way written as these tests alone (whole and safe, else
    /// a low byte of zeros, else the heap) gives the same Numbers and was
    /// slower where an operator of the loop of instructions has it in
    /// line: trigonometry in a loop by 12%, arithmetic on doubles by 8%.
    #[inline(always)]
    fn is_heap_f64(value: f64) -> bool {
        if value.to_bits() & 0xFF == 0 {
            return false;
        }
        let whole = value as i64;
        whole as f64 != value || whole.unsigned_abs() > SmallInteger::MAX as u64
    }

    /// Create a new Number from an i64.
    pub fn from_i64(agent: &mut Agent, value: i64, gc: NoGcScope<'a, '_>) -> Self {
        if let Ok(value) = Number::try_from(value) {
            value
        } else {
            let value = value as f64;
            if let Ok(value) = SmallF64::try_from(value) {
                // Number did not fit the safe integer range but could be
                // represented as a SmallF64.
                Number::SmallF64(value)
            } else {
                let id = Self::alloc_number(&mut agent.heap, value);
                Number::Number(id.unbind().bind(gc))
            }
        }
    }

    /// Create a Number from a usize.
    pub fn from_usize(agent: &mut Agent, value: usize, gc: NoGcScope<'a, '_>) -> Self {
        if let Ok(value) = Number::try_from(value) {
            value
        } else {
            let value = value as f64;
            if let Ok(value) = SmallF64::try_from(value) {
                // Number did not fit the safe integer range but could be
                // represented as a SmallF64.
                Number::SmallF64(value)
            } else {
                let id = Self::alloc_number(&mut agent.heap, value);
                Number::Number(id.unbind().bind(gc))
            }
        }
    }

    /// Get the canonical NaN value.
    pub fn nan() -> Self {
        Self::from(f32::NAN)
    }

    /// Get the -0 value.
    pub fn neg_zero() -> Self {
        Self::from(-0.0f32)
    }

    /// Get the 0 value.
    pub fn pos_zero() -> Self {
        Self::from(0)
    }

    /// Get the Infinity value.
    pub fn pos_inf() -> Self {
        Self::from(f32::INFINITY)
    }

    /// Get the -Infinity value.
    pub fn neg_inf() -> Self {
        Self::from(f32::NEG_INFINITY)
    }

    /// Returns `true` if the number is any NaN.
    #[inline(always)]
    pub fn is_nan(self, agent: &Agent) -> bool {
        self.is_nan_(agent)
    }

    pub(crate) fn is_nan_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => n.get(agent).is_nan(),
            Number::Integer(_) => false,
            Number::SmallF64(n) => n.into_f64().is_nan(),
        }
    }

    /// Returns `true` if the number is exactly +0 (and not -0).
    #[inline(always)]
    pub fn is_pos_zero(self, agent: &Agent) -> bool {
        self.is_pos_zero_(agent)
    }

    pub(crate) fn is_pos_zero_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => f64::to_bits(0.0) == f64::to_bits(*n.get(agent)),
            Number::Integer(n) => 0i64 == n.into_i64(),
            Number::SmallF64(n) => n.into_f64().to_bits() == 0.0f64.to_bits(),
        }
    }

    /// Returns `true` if the number is exactly -0 (and not +0).
    #[inline(always)]
    pub fn is_neg_zero(self, agent: &Agent) -> bool {
        self.is_neg_zero_(agent)
    }

    pub(crate) fn is_neg_zero_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => f64::to_bits(-0.0) == f64::to_bits(*n.get(agent)),
            Number::Integer(_) => false,
            Number::SmallF64(n) => n.into_f64().to_bits() == (-0.0f64).to_bits(),
        }
    }

    /// Returns `true` if the number is Infinity.
    #[inline(always)]
    pub fn is_pos_infinity(self, agent: &Agent) -> bool {
        self.is_pos_infinity_(agent)
    }

    pub(crate) fn is_pos_infinity_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => *n.get(agent) == f64::INFINITY,
            Number::Integer(_) => false,
            Number::SmallF64(n) => n.into_f64() == f64::INFINITY,
        }
    }

    /// Returns `true` if the number is -Infinity.
    #[inline(always)]
    pub fn is_neg_infinity(self, agent: &Agent) -> bool {
        self.is_neg_infinity_(agent)
    }

    pub(crate) fn is_neg_infinity_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => *n.get(agent) == f64::NEG_INFINITY,
            Number::Integer(_) => false,
            Number::SmallF64(n) => n.into_f64() == f64::NEG_INFINITY,
        }
    }

    /// Returns `true` if this number is neither infinite nor NaN.
    #[inline(always)]
    pub fn is_finite(self, agent: &Agent) -> bool {
        self.is_finite_(agent)
    }

    pub(crate) fn is_finite_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => n.get(agent).is_finite(),
            Number::Integer(_) => true,
            Number::SmallF64(n) => n.into_f64().is_finite(),
        }
    }

    /// Returns `true` if this number is an integer (has a fractional part of
    /// 0).
    #[inline(always)]
    pub fn is_integer(self, agent: &Agent) -> bool {
        self.is_integer_(agent)
    }

    pub(crate) fn is_integer_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => n.get(agent).fract() == 0.0,
            Number::Integer(_) => true,
            Number::SmallF64(n) => n.into_f64().fract() == 0.0,
        }
    }

    /// Returns `true` if this number is not equal to 0 or -0.
    #[inline(always)]
    pub fn is_nonzero(self, agent: &Agent) -> bool {
        self.is_nonzero_(agent)
    }

    pub(crate) fn is_nonzero_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => n.get(agent) != &0.0,
            Number::Integer(n) => n.into_i64() != 0,
            Number::SmallF64(n) => n.into_f64() != 0.0,
        }
    }

    /// Returns `true` if this number is equal to 1.
    #[inline(always)]
    pub fn is_pos_one(self, agent: &Agent) -> bool {
        self.is_pos_one_(agent)
    }

    pub(crate) fn is_pos_one_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        // NOTE: Only the integer variant should ever return true, if any other
        // variant returns true, that's a bug as it means that our variants are
        // no longer "sound".
        match self {
            Number::Integer(n) => 1i64 == n.into_i64(),
            Number::Number(n) => {
                debug_assert_ne!(*n.get(agent), 1.0);
                false
            }
            Number::SmallF64(n) => {
                debug_assert_ne!(n.into_f64(), 1.0);
                false
            }
        }
    }

    /// Returns `true` if this number is equal to -1.
    #[inline(always)]
    pub fn is_neg_one(self, agent: &Agent) -> bool {
        self.is_neg_one_(agent)
    }

    pub(crate) fn is_neg_one_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Integer(n) => -1i64 == n.into_i64(),
            Number::Number(n) => {
                debug_assert_ne!(*n.get(agent), -1.0);
                false
            }
            Number::SmallF64(n) => {
                debug_assert_ne!(n.into_f64(), -1.0);
                false
            }
        }
    }

    /// Returns `true` if this number has a positive sign, including `+0.0`,
    /// NaNs with positive sign bit and positive infinity.
    #[inline(always)]
    pub fn is_sign_positive(self, agent: &Agent) -> bool {
        self.is_sign_positive_(agent)
    }

    pub(crate) fn is_sign_positive_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => n.get(agent).is_sign_positive(),
            Number::Integer(n) => n.into_i64() >= 0,
            Number::SmallF64(n) => n.into_f64().is_sign_positive(),
        }
    }

    /// Returns `true` if this number has a negative sign, including `-0.0`,
    /// NaNs with negative sign bit and negative infinity.
    #[inline(always)]
    pub fn is_sign_negative(self, agent: &Agent) -> bool {
        self.is_sign_negative_(agent)
    }

    pub(crate) fn is_sign_negative_<T>(self, agent: &'a T) -> bool
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => n.get(agent).is_sign_negative(),
            Number::Integer(n) => n.into_i64().is_negative(),
            Number::SmallF64(n) => n.into_f64().is_sign_negative(),
        }
    }

    /// # [truncate](https://tc39.es/ecma262/#eqn-truncate)
    pub fn truncate(self, agent: &mut Agent, gc: NoGcScope<'a, '_>) -> Self {
        match self {
            Number::Number(n) => {
                let n = n.get(agent).trunc();
                Number::from_f64(agent, n, gc)
            }
            Number::Integer(_) => self,
            Number::SmallF64(n) => Number::from_f64(agent, n.into_f64().trunc(), gc),
        }
    }

    /// Get the number value as f64.
    #[inline(always)]
    pub fn into_f64(self, agent: &Agent) -> f64 {
        self.into_f64_(agent)
    }

    pub(crate) fn into_f64_<T>(self, agent: &'a T) -> f64
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => *n.get(agent),
            Number::Integer(n) => n.into_i64() as f64,
            Number::SmallF64(n) => n.into_f64(),
        }
    }

    /// Get the number value as f32.
    #[inline(always)]
    pub fn into_f32(self, agent: &Agent) -> f32 {
        self.into_f32_(agent)
    }

    pub(crate) fn into_f32_<T>(self, agent: &'a T) -> f32
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => *n.get(agent) as f32,
            Number::Integer(n) => Into::<i64>::into(n) as f32,
            Number::SmallF64(n) => n.into_f64() as f32,
        }
    }

    /// Get the number value as f16.
    #[cfg(feature = "proposal-float16array")]
    #[inline(always)]
    pub fn into_f16(self, agent: &Agent) -> f16 {
        self.into_f16_(agent)
    }

    #[cfg(feature = "proposal-float16array")]
    pub(crate) fn into_f16_<T>(self, agent: &'a T) -> f16
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => *n.get(agent) as f16,
            Number::Integer(n) => Into::<i64>::into(n) as f16,
            Number::SmallF64(n) => n.into_f64() as f16,
        }
    }

    /// Returns the number cast to an [`i64`].
    ///
    /// If the number isn't representable as an i64:
    /// - NaN becomes 0.
    /// - Numbers are clamped between [`i64::MIN`] and [`i64::MAX`].
    /// - All other numbers round towards zero.
    #[inline(always)]
    pub fn into_i64(self, agent: &Agent) -> i64 {
        self.into_i64_(agent)
    }

    pub(crate) fn into_i64_<T>(self, agent: &'a T) -> i64
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => *n.get(agent) as i64,
            Number::Integer(n) => Into::<i64>::into(n),
            Number::SmallF64(n) => n.into_f64() as i64,
        }
    }

    /// Returns the number cast to a [`usize`].
    ///
    /// If the number isn't representable as a usize:
    /// - NaN becomes 0.
    /// - Numbers are clamped between 0 and [`usize::MAX`].
    /// - All other numbers round towards zero.
    #[inline(always)]
    pub fn into_usize(self, agent: &Agent) -> usize {
        self.into_usize_(agent)
    }

    pub(crate) fn into_usize_<T>(self, agent: &'a T) -> usize
    where
        T: NumberHeapAccess,
    {
        match self {
            Number::Number(n) => *n.get(agent) as usize,
            Number::Integer(n) => {
                let i64 = Into::<i64>::into(n);
                if i64 < 0 {
                    0
                } else {
                    usize::try_from(i64).unwrap_or(usize::MAX)
                }
            }
            Number::SmallF64(n) => n.into_f64() as usize,
        }
    }

    /// Compare two Numbers with each other: This is used when the spec asks if
    /// `x is y` when talking of Numbers. Generally this is asked after various
    /// NaN and non-zero checks, depending on which spec algorithm is being
    /// used.
    #[inline(always)]
    fn is<T>(agent: &'a T, x: Self, y: Self) -> bool
    where
        T: NumberHeapAccess,
    {
        match (x, y) {
            // Optimisation: First compare by-reference; only read from heap if needed.
            (Number::Number(x), Number::Number(y)) => x == y || x.get(agent) == y.get(agent),
            (Number::Integer(x), Number::Integer(y)) => x == y,
            (Number::SmallF64(x), Number::SmallF64(y)) => x == y,
            (Number::Number(x), Number::Integer(y)) => {
                // Optimisation: Integers should never be allocated into the heap as f64s.
                debug_assert!(*x.get(agent) != y.into_i64() as f64);
                false
            }
            (Number::Number(x), Number::SmallF64(y)) => {
                // Optimisation: f32s should never be allocated into the heap
                debug_assert!(*x.get(agent) != y.into_f64());
                false
            }
            (Number::Integer(x), Number::Number(y)) => {
                // Optimisation: Integers should never be allocated into the heap as f64s.
                debug_assert!((x.into_i64() as f64) != *y.get(agent));
                false
            }
            (Number::Integer(x), Number::SmallF64(y)) => {
                debug_assert!(
                    y.into_f64().to_bits() == (-0.0f64).to_bits()
                        || (x.into_i64() as f64) != y.into_f64()
                );
                false
            }
            (Number::SmallF64(x), Number::Number(y)) => {
                // Optimisation: f32s should never be allocated into the heap
                debug_assert!(&x.into_f64() != y.get(agent));
                false
            }
            (Number::SmallF64(x), Number::Integer(y)) => {
                debug_assert!(
                    x.into_f64().to_bits() == (-0.0f64).to_bits()
                        || x.into_f64() != y.into_i64() as f64
                );
                false
            }
        }
    }

    /// Returns `true` if this number is an integer number and is odd.
    pub fn is_odd_integer(self, agent: &mut Agent) -> bool {
        match self {
            Number::Number(n) => n.get(agent).rem_euclid(2.0) == 1.0,
            Number::Integer(n) => i64::from(n).rem_euclid(2) == 1,
            Number::SmallF64(n) => n.into_f64().rem_euclid(2.0) == 1.0,
        }
    }

    /// Get the absolute value of this number.
    pub fn abs(self, agent: &mut Agent) -> Self {
        match self {
            Number::Number(n) => {
                let n = *n.get(agent);
                if n > 0.0 { self } else { agent.heap.create(-n) }
            }
            Number::Integer(n) => {
                let n = n.into_i64();
                Number::Integer(SmallInteger::try_from(n.abs()).unwrap())
            }
            Number::SmallF64(n) => {
                let n = n.into_f64();
                if n == 0.0 {
                    // Negative zero needs to be turned into a Number::Integer
                    debug_assert!(n.is_sign_negative());
                    Number::Integer(SmallInteger::zero())
                } else {
                    Number::SmallF64(SmallF64::try_from(n.abs()).unwrap())
                }
            }
        }
    }

    /// `x > y`
    pub fn greater_than(agent: &mut Agent, x: Self, y: Self) -> Option<bool> {
        Number::less_than(agent, x, y).map(|lt| !lt && !Number::is(agent, x, y))
    }

    /// ### [6.1.6.1.1 Number::unaryMinus ( x )](https://tc39.es/ecma262/#sec-numeric-types-number-unaryMinus)
    pub fn unary_minus(agent: &mut Agent, x: Self) -> Self {
        // 1. If x is NaN, return NaN.
        // NOTE: Computers do this automatically.

        // 2. Return the result of negating x; that is, compute a Number with the same magnitude but opposite sign.
        match x {
            Number::Number(n) => {
                let value = n.get(agent);
                agent.heap.create(-value)
            }
            Number::Integer(n) => {
                let n = n.into_i64();
                if n == 0 {
                    Number::SmallF64(SmallF64::try_from(-0.0).unwrap())
                } else {
                    SmallInteger::try_from(-n).unwrap().into()
                }
            }
            Number::SmallF64(n) => {
                let n = n.into_f64();
                if n == 0.0 {
                    debug_assert!(n.is_sign_negative());
                    SmallInteger::zero().into()
                } else {
                    (-n).try_into().unwrap()
                }
            }
        }
    }

    /// ### [6.1.6.1.2 Number::bitwiseNOT ( x )](https://tc39.es/ecma262/#sec-numeric-types-number-bitwiseNOT)
    pub fn bitwise_not(agent: &mut Agent, x: Self) -> Self {
        // 1. Let oldValue be ! ToInt32(x).
        let old_value = to_int32_number(agent, x);

        // 2. Return the result of applying bitwise complement to oldValue. The mathematical value of the result is exactly representable as a 32-bit two's complement bit string.
        Number::from(!old_value)
    }

    /// ### [6.1.6.1.3 Number::exponentiate ( base, exponent )](https://tc39.es/ecma262/#sec-numeric-types-number-exponentiate)
    pub fn exponentiate(agent: &mut Agent, base: Self, exponent: Self) -> Self {
        // 1. If exponent is NaN, return NaN.
        if exponent.is_nan_(agent) {
            return Number::nan();
        }

        // 2. If exponent is either +0𝔽 or -0𝔽, return 1𝔽.
        if exponent.is_pos_zero_(agent) || exponent.is_neg_zero_(agent) {
            return Number::from(1);
        }

        // 3. If base is NaN, return NaN.
        if base.is_nan_(agent) {
            return Number::nan();
        }

        // 4. If base is +∞𝔽, then
        if base.is_pos_infinity_(agent) {
            // a. If exponent > +0𝔽, return +∞𝔽. Otherwise, return +0𝔽.
            return if Number::greater_than(agent, exponent, Number::from(0)).unwrap_or(false) {
                Number::pos_inf()
            } else {
                Number::pos_zero()
            };
        }

        // 5. If base is -∞𝔽, then
        if base.is_neg_infinity_(agent) {
            // a. If exponent > +0𝔽, then
            return if Number::greater_than(agent, exponent, 0.into()).unwrap_or(false) {
                // i. If exponent is an odd integral Number, return -∞𝔽. Otherwise, return +∞𝔽.
                if exponent.is_odd_integer(agent) {
                    Number::neg_inf()
                } else {
                    Number::pos_inf()
                }
            }
            // b. Else,
            else {
                // i. If exponent is an odd integral Number, return -0𝔽. Otherwise, return +0𝔽.
                if exponent.is_odd_integer(agent) {
                    Number::neg_zero()
                } else {
                    Number::pos_zero()
                }
            };
        }

        // 6. If base is +0𝔽, then
        if base.is_pos_zero_(agent) {
            // a. If exponent > +0𝔽, return +0𝔽. Otherwise, return +∞𝔽.
            return if Number::greater_than(agent, exponent, Number::pos_zero()).unwrap_or(false) {
                Number::pos_zero()
            } else {
                Number::pos_inf()
            };
        }

        // 7. If base is -0𝔽, then
        if base.is_neg_zero_(agent) {
            // a. If exponent > +0𝔽, then
            return if Number::greater_than(agent, exponent, Number::pos_zero()).unwrap_or(false) {
                // i. If exponent is an odd integral Number, return -0𝔽. Otherwise, return +0𝔽.
                if exponent.is_odd_integer(agent) {
                    Number::neg_zero()
                } else {
                    Number::pos_zero()
                }
            }
            // b. Else,
            else {
                // i. If exponent is an odd integral Number, return -∞𝔽. Otherwise, return +∞𝔽.
                if exponent.is_odd_integer(agent) {
                    Number::neg_inf()
                } else {
                    Number::pos_inf()
                }
            };
        }

        // 8. Assert: base is finite and is neither +0𝔽 nor -0𝔽.
        debug_assert!(base.is_finite_(agent) && base.is_nonzero_(agent));

        // 9. If exponent is +∞𝔽, then
        if exponent.is_pos_infinity_(agent) {
            let base = base.abs(agent);

            // a. If abs(ℝ(base)) > 1, return +∞𝔽.
            return if Number::greater_than(agent, base, Number::from(1)).unwrap_or(false) {
                Number::pos_inf()
            }
            // b. If abs(ℝ(base)) = 1, return NaN.
            else if Number::is(agent, base, Number::from(1)) {
                Number::nan()
            }
            // c. If abs(ℝ(base)) < 1, return +0𝔽.
            else {
                Number::pos_zero()
            };
        }

        // 10. If exponent is -∞𝔽, then
        if exponent.is_neg_infinity_(agent) {
            let base = base.into_f64_(agent).abs();

            // a. If abs(ℝ(base)) > 1, return +0𝔽.
            return if base > 1.0 {
                Number::pos_zero()
            }
            // b. If abs(ℝ(base)) = 1, return NaN.
            else if base == 1.0 {
                Number::nan()
            }
            // c. If abs(ℝ(base)) < 1, return +∞𝔽.
            else {
                Number::pos_inf()
            };
        }

        // 11. Assert: exponent is finite and is neither +0𝔽 nor -0𝔽.
        debug_assert!(exponent.is_finite_(agent) && exponent.is_nonzero_(agent));

        // 12. If base < -0𝔽 and exponent is not an integral Number, return NaN.
        if Number::less_than(agent, base, Number::neg_zero()).unwrap_or(false)
            && !exponent.is_integer_(agent)
        {
            return Number::nan();
        }

        // 13. Return an implementation-approximated Number value representing the result of raising ℝ(base) to the ℝ(exponent) power.
        agent
            .heap
            .create(base.into_f64_(agent).powf(exponent.into_f64_(agent)))
    }

    /// ### [6.1.6.1.4 Number::multiply ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-multiply)
    ///
    /// The abstract operation Number::multiply takes arguments x (a Number)
    /// and y (a Number) and returns a Number. It performs multiplication
    /// according to the rules of IEEE 754-2019 binary double-precision
    /// arithmetic, producing the product of x and y.
    ///
    /// > NOTE: Finite-precision multiplication is commutative, but not always
    /// > associative.
    pub fn multiply(agent: &mut Agent, x: Self, y: Self, gc: NoGcScope<'a, '_>) -> Self {
        // Nonstandard fast path: If both numbers are integers, use integer
        // multiplication and try to return a safe integer as integer.
        if let (Self::Integer(x), Self::Integer(y)) = (x, y) {
            let x = x.into_i64();
            let y = y.into_i64();
            let result = x.checked_mul(y);
            // A zero product of a negative factor is -0, which no integer is.
            if let Some(result) = result
                && (result != 0 || (x >= 0 && y >= 0))
            {
                if let Ok(result) = SmallInteger::try_from(result) {
                    return result.into();
                }
                return Self::from_f64(agent, result as f64, gc);
            }
            return Self::from_f64(agent, x as f64 * y as f64, gc);
        }
        // Steps 1 to 6 are IEEE 754 multiplication: a NaN operand, or an
        // infinity times a zero, gives NaN; an infinity gives an infinity
        // and a zero a zero, each with the sign of the product.
        Self::from_f64(agent, x.into_f64_(agent) * y.into_f64_(agent), gc)
    }

    /// ### [6.1.6.1.5 Number::divide ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-divide)
    ///
    /// The abstract operation Number::divide takes arguments x (a Number) and
    /// y (a Number) and returns a Number. It performs division according to
    /// the rules of IEEE 754-2019 binary double-precision arithmetic,
    /// producing the quotient of x and y where x is the dividend and y is the
    /// divisor.
    pub fn divide(agent: &mut Agent, x: Self, y: Self, gc: NoGcScope<'a, '_>) -> Self {
        // Steps 1 to 8 are IEEE 754 division: a NaN operand, two infinities
        // or two zeros give NaN; an infinity divided or a division by a zero
        // gives an infinity, and a zero divided or a division by an infinity
        // a zero, each with the sign of the quotient.
        Number::from_f64(agent, x.into_f64_(agent) / y.into_f64_(agent), gc)
    }

    /// ### [6.1.6.1.6 Number::remainder ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-remainder)
    ///
    /// The abstract operation Number::remainder takes arguments n (a Number)
    /// and d (a Number) and returns a Number. It yields the remainder from an
    /// implied division of its operands where n is the dividend and d is the
    /// divisor.
    pub fn remainder(agent: &mut Agent, n: Self, d: Self, gc: NoGcScope<'a, '_>) -> Self {
        // Steps 1 to 11 are the remainder of two doubles as `%` takes it
        // (C's fmod): exact, with the sign of the dividend, also where it is
        // zero; NaN for a NaN operand, an infinite dividend or a zero
        // divisor; the dividend itself for an infinite divisor. The
        // quotient rounded to a double would not be exact (`1e21 % 3` was 0
        // by it).
        Self::from_f64(agent, n.into_f64_(agent) % d.into_f64_(agent), gc)
    }

    /// ### [6.1.6.1.7 Number::add ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-add)
    ///
    /// The abstract operation Number::add takes arguments x (a Number) and y
    /// (a Number) and returns a Number. It performs addition according to the
    /// rules of IEEE 754-2019 binary double-precision arithmetic, producing
    /// the sum of its arguments.
    pub(crate) fn add(agent: &mut Agent, x: Self, y: Self) -> Self {
        // Two integers: the sum of two safe integers cannot overflow an
        // i64, and it is the IEEE sum whenever it is itself a safe integer.
        if let (Self::Integer(x), Self::Integer(y)) = (x, y) {
            let (x, y) = (x.into_i64(), y.into_i64());
            if let Ok(sum) = SmallInteger::try_from(x + y) {
                return Self::Integer(sum);
            }
            return agent.heap.create(x as f64 + y as f64);
        }
        // Steps 1 to 8 are IEEE 754 addition: a NaN operand or infinities of
        // opposite sign give NaN, an infinity gives itself, -0 + -0 is -0.
        agent.heap.create(x.into_f64_(agent) + y.into_f64_(agent))
    }

    /// ### [6.1.6.1.8 Number::subtract ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-subtract)
    ///
    /// The abstract operation Number::subtract takes arguments x (a Number)
    /// and y (a Number) and returns a Number. It performs subtraction,
    /// producing the difference of its operands; x is the minuend and y is the
    /// subtrahend.
    pub(crate) fn subtract(agent: &mut Agent, x: Self, y: Self) -> Self {
        // Two integers, as in Number::add.
        if let (Self::Integer(x), Self::Integer(y)) = (x, y) {
            let (x, y) = (x.into_i64(), y.into_i64());
            if let Ok(difference) = SmallInteger::try_from(x - y) {
                return Self::Integer(difference);
            }
            return agent.heap.create(x as f64 - y as f64);
        }
        // 1. Return Number::add(x, Number::unaryMinus(y)): IEEE 754
        // subtraction.
        agent.heap.create(x.into_f64_(agent) - y.into_f64_(agent))
    }

    /// ### [6.1.6.1.9 Number::leftShift ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-leftShift)
    ///
    /// The abstract operation Number::signedRightShift takes arguments x
    /// (a Number) and y (a Number) and returns an integral Number.
    pub fn left_shift(agent: &mut Agent, x: Self, y: Self) -> Self {
        // 1. Let lnum be ! ToInt32(x).
        let lnum = to_int32_number(agent, x);
        // 2. Let rnum be ! ToUint32(y).
        let rnum = to_uint32_number(agent, y);
        // 3. Let shiftCount be ℝ(rnum) modulo 32.
        let shift_count = rnum % 32;
        // 4. Return the result of left shifting lnum by shiftCount bits. The mathematical value of the result is exactly representable as a 32-bit two's complement bit string.
        Number::from(lnum.signed_shl(shift_count))
    }

    /// ### [6.1.6.1.10 Number::signedRightShift ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-signedRightShift)
    ///
    /// The abstract operation Number::unsignedRightShift takes arguments x
    /// (a Number) and y (a Number) and returns an integral Number.
    pub fn signed_right_shift(agent: &mut Agent, x: Self, y: Self) -> Self {
        // 1. Let lnum be ! ToInt32(x).
        let lnum = to_int32_number(agent, x);
        // 2. Let rnum be ! ToUint32(y).
        let rnum = to_uint32_number(agent, y);
        // 3. Let shiftCount be ℝ(rnum) modulo 32.
        let shift_count = rnum % 32;
        // 4. Return the result of performing a sign-extending right shift of lnum by shiftCount bits. The most significant bit is propagated. The mathematical value of the result is exactly representable as a 32-bit two's complement bit string.
        Number::from(lnum.signed_shr(shift_count))
    }

    /// ### [6.1.6.1.11 Number::unsignedRightShift ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-unsignedRightShift)
    ///
    /// The abstract operation Number::lessThan takes arguments x (a Number)
    /// and y (a Number) and returns a Boolean or undefined.
    pub fn unsigned_right_shift(agent: &mut Agent, x: Self, y: Self) -> Self {
        // 1. Let lnum be ! ToUint32(x).
        let lnum = to_uint32_number(agent, x);
        // 2. Let rnum be ! ToUint32(y).
        let rnum = to_uint32_number(agent, y);
        // 3. Let shiftCount be ℝ(rnum) modulo 32.
        let shift_count = rnum % 32;
        // 4. Return the result of performing a zero-filling right shift of lnum by shiftCount bits. Vacated bits are filled with zero. The mathematical value of the result is exactly representable as a 32-bit unsigned bit string.
        Number::from(lnum.unsigned_shr(shift_count))
    }

    /// ### [6.1.6.1.12 Number::lessThan ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-lessThan)
    pub fn less_than(agent: &Agent, x: Self, y: Self) -> Option<bool> {
        if let (Self::Integer(x), Self::Integer(y)) = (x, y) {
            return Some(x.into_i64() < y.into_i64());
        }
        // Steps 1 to 11 are the IEEE 754 comparison, with undefined for a
        // NaN operand: equal values and zeros of either sign are not less,
        // nothing is above +∞ or below -∞.
        let (x, y) = (x.into_f64_(agent), y.into_f64_(agent));
        if x.is_nan() || y.is_nan() {
            None
        } else {
            Some(x < y)
        }
    }

    /// ### [6.1.6.1.13 Number::equal ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-equal)
    #[inline(always)]
    pub fn equal<T>(agent: &'a Agent, x: Self, y: Self) -> bool {
        Self::equal_(agent, x, y)
    }

    pub(crate) fn equal_<T>(agent: &'a T, x: Self, y: Self) -> bool
    where
        T: NumberHeapAccess,
    {
        // 1. If x is NaN, return false.
        if x.is_nan_(agent) {
            return false;
        }

        // 2. If y is NaN, return false.
        if y.is_nan_(agent) {
            return false;
        }

        // 3. If x is y, return true.
        if Number::is(agent, x, y) {
            return true;
        }

        // 4. If x is +0𝔽 and y is -0𝔽, return true.
        if x.is_pos_zero_(agent) && y.is_neg_zero_(agent) {
            return true;
        }

        // 5. If x is -0𝔽 and y is +0𝔽, return true.
        if x.is_neg_zero_(agent) && y.is_pos_zero_(agent) {
            return true;
        }

        // 6. Return false.
        false
    }

    /// ### [6.1.6.1.14 Number::sameValue ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-sameValue)
    #[inline(always)]
    pub fn same_value<T>(agent: &'a Agent, x: Self, y: Self) -> bool {
        Self::same_value_(agent, x, y)
    }

    pub(crate) fn same_value_<T>(agent: &'a T, x: Self, y: Self) -> bool
    where
        T: NumberHeapAccess,
    {
        // 1. If x is NaN and y is NaN, return true.
        if x.is_nan_(agent) && y.is_nan_(agent) {
            return true;
        }

        // 2. If x is +0𝔽 and y is -0𝔽, return false.
        if x.is_pos_zero_(agent) && y.is_neg_zero_(agent) {
            return false;
        }

        // 3. If x is -0𝔽 and y is +0𝔽, return false.
        if x.is_neg_zero_(agent) && y.is_pos_zero_(agent) {
            return false;
        }

        // 4. If x is y, return true.
        if Number::is(agent, x, y) {
            return true;
        }

        // 5. Return false.
        false
    }

    /// ### [6.1.6.1.15 Number::sameValueZero ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-sameValueZero)
    #[inline(always)]
    pub fn same_value_zero<T>(agent: &'a Agent, x: Self, y: Self) -> bool {
        Self::same_value_zero_(agent, x, y)
    }

    pub(crate) fn same_value_zero_<T>(agent: &'a T, x: Self, y: Self) -> bool
    where
        T: NumberHeapAccess,
    {
        // 1. If x is NaN and y is NaN, return true.
        if x.is_nan_(agent) && y.is_nan_(agent) {
            return true;
        }

        // 2. If x is +0𝔽 and y is -0𝔽, return true.
        if x.is_pos_zero_(agent) && y.is_neg_zero_(agent) {
            return true;
        }

        // 3. If x is -0𝔽 and y is +0𝔽, return true.
        if x.is_neg_zero_(agent) && y.is_pos_zero_(agent) {
            return true;
        }

        // 4. If x is y, return true.
        if Number::is(agent, x, y) {
            return true;
        }

        // 5. Return false.
        false
    }

    /// ### [6.1.6.1.16 NumberBitwiseOp ( op, x, y )](https://tc39.es/ecma262/#sec-numberbitwiseop)
    #[inline(always)]
    fn bitwise_op(agent: &mut Agent, op: BitwiseOp, x: Self, y: Self) -> i32 {
        // 1. Let lnum be ! ToInt32(x).
        let lnum = to_int32_number(agent, x);

        // 2. Let rnum be ! ToInt32(y).
        let rnum = to_int32_number(agent, y);

        // 3. Let lbits be the 32-bit two's complement bit string representing ℝ(lnum).
        let lbits = lnum;

        // 4. Let rbits be the 32-bit two's complement bit string representing ℝ(rnum).
        let rbits = rnum;

        // 8. Return the Number value for the integer represented by the 32-bit two's complement bit string result.
        match op {
            // 5. If op is &, then
            BitwiseOp::And => {
                // a. Let result be the result of applying the bitwise AND operation to lbits and rbits.
                lbits & rbits
            }
            // 6. Else if op is ^, then
            BitwiseOp::Xor => {
                // a. Let result be the result of applying the bitwise exclusive OR (XOR) operation to lbits and rbits.
                lbits ^ rbits
            }
            // 7. Else,
            // a. Assert: op is |.
            BitwiseOp::Or => {
                // b. Let result be the result of applying the bitwise inclusive OR operation to lbits and rbits.
                lbits | rbits
            }
        }
    }

    /// ### [6.1.6.1.17 Number::bitwiseAND ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-bitwiseAND)
    pub fn bitwise_and(agent: &mut Agent, x: Self, y: Self) -> i32 {
        // 1. Return NumberBitwiseOp(&, x, y).
        Number::bitwise_op(agent, BitwiseOp::And, x, y)
    }

    /// ### [6.1.6.1.18 Number::bitwiseXOR ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-bitwiseXOR)
    pub fn bitwise_xor(agent: &mut Agent, x: Self, y: Self) -> i32 {
        // 1. Return NumberBitwiseOp(^, x, y).
        Number::bitwise_op(agent, BitwiseOp::Xor, x, y)
    }

    /// ### [6.1.6.1.19 Number::bitwiseOR ( x, y )](https://tc39.es/ecma262/#sec-numeric-types-number-bitwiseOR)
    pub fn bitwise_or(agent: &mut Agent, x: Self, y: Self) -> i32 {
        // 1. Return NumberBitwiseOp(|, x, y).
        Number::bitwise_op(agent, BitwiseOp::Or, x, y)
    }

    // ### [6.1.6.1.20 Number::toString ( x, radix )](https://tc39.es/ecma262/#sec-numeric-types-number-tostring)
    pub(crate) fn to_string_radix_n<'gc>(
        agent: &mut Agent,
        x: Self,
        radix: u32,
        gc: NoGcScope<'gc, '_>,
    ) -> String<'gc> {
        let string = match x {
            Number::Integer(x) => i64_to_radix_string(x.into_i64(), radix),
            Number::Number(x) => f64_to_radix_string(*x.get(agent), radix),
            Number::SmallF64(x) => f64_to_radix_string(x.into_f64(), radix),
        };
        String::from_string(agent, string, gc)
    }

    // ### [6.1.6.1.20 Number::toString ( x, radix )](https://tc39.es/ecma262/#sec-numeric-types-number-tostring)
    pub(crate) fn to_string_radix_10<'gc>(
        agent: &mut Agent,
        x: Self,
        gc: NoGcScope<'gc, '_>,
    ) -> String<'gc> {
        // The text is written in the native stack and the String made of
        // it: a short one is the value itself, and nothing is allocated.
        match x {
            Number::Number(x) => f64_text(*x.get(agent)),
            Number::Integer(x) => i64_text(x.into_i64()),
            Number::SmallF64(x) => f64_text(x.into_f64()),
        }
        .to_js_string(agent, gc)
    }

    /// # [ℝ](https://tc39.es/ecma262/#%E2%84%9D)
    pub(crate) fn to_real<T>(self, agent: &'a T) -> f64
    where
        T: NumberHeapAccess,
    {
        match self {
            Self::Number(n) => *n.get(agent),
            Self::Integer(i) => i.into_i64() as f64,
            Self::SmallF64(f) => f.into_f64(),
        }
    }
}

impl<const N: usize> NumberText<N> {
    /// The String of the text. It is ASCII, so its bytes are the WTF-8 of a
    /// String as they are, and are not looked through to learn it.
    pub(crate) fn to_js_string<'gc>(
        &self,
        agent: &mut Agent,
        gc: NoGcScope<'gc, '_>,
    ) -> String<'gc> {
        let text = wtf8::Wtf8Str::from_wtf8_bytes(self.as_bytes());
        // What a SmallString holds: the seven bytes beside a Value's tag.
        if text.len() <= 7 {
            small_string::SmallString::from_wtf8(text).into()
        } else {
            agent.heap.alloc_wtf8(text).bind(gc)
        }
    }
}

impl core::fmt::Debug for Number<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self {
            Number::Number(idx) => write!(f, "Number({idx:?})"),
            Number::Integer(value) => write!(f, "{}", value.into_i64()),
            Number::SmallF64(value) => write!(f, "{}", value.into_f64()),
        }
    }
}

#[cfg(feature = "proposal-float16array")]
impl From<f16> for Number<'_> {
    fn from(value: f16) -> Self {
        if value.is_finite()
            && value.trunc() == value
            && !(value.is_sign_negative() && value == 0.0)
        {
            let int = value as i64;
            if let Ok(int) = SmallInteger::try_from(int) {
                Number::Integer(int)
            } else {
                Number::SmallF64(value.into())
            }
        } else {
            Number::SmallF64(SmallF64::from(value))
        }
    }
}

impl From<f32> for Number<'_> {
    fn from(value: f32) -> Self {
        if value.is_finite()
            && value.trunc() == value
            && !(value.is_sign_negative() && value == 0.0)
        {
            let int = value as i64;
            if let Ok(int) = SmallInteger::try_from(int) {
                Number::Integer(int)
            } else {
                Number::SmallF64(value.into())
            }
        } else {
            Number::SmallF64(SmallF64::from(value))
        }
    }
}

const MAX_NUMBER: f64 = ((1u64 << 53) - 1) as f64;
const MIN_NUMBER: f64 = -MAX_NUMBER;

impl TryFrom<i64> for Number<'static> {
    type Error = ();

    fn try_from(value: i64) -> Result<Self, ()> {
        Ok(Number::Integer(SmallInteger::try_from(value)?))
    }
}

impl TryFrom<u64> for Number<'static> {
    type Error = ();

    fn try_from(value: u64) -> Result<Self, ()> {
        Ok(Number::Integer(SmallInteger::try_from(value)?))
    }
}

impl TryFrom<usize> for Number<'static> {
    type Error = ();

    fn try_from(value: usize) -> Result<Self, ()> {
        if let Ok(i64) = i64::try_from(value) {
            Self::try_from(i64)
        } else {
            Err(())
        }
    }
}

impl TryFrom<f64> for Number<'static> {
    type Error = ();

    #[inline]
    fn try_from(value: f64) -> Result<Self, ()> {
        if value.is_finite()
            && value.trunc() == value
            && (MIN_NUMBER..=MAX_NUMBER).contains(&value)
            && !(value.is_zero() && value.is_sign_negative())
        {
            debug_assert_eq!(value as i64 as f64, value);
            Ok(Number::try_from(value as i64).unwrap())
        } else if let Ok(value) = SmallF64::try_from(value) {
            Ok(Number::SmallF64(value))
        } else {
            Err(())
        }
    }
}

impl<'a> From<Number<'a>> for Numeric<'a> {
    fn from(value: Number<'a>) -> Self {
        match value {
            Number::Number(data) => Self::Number(data),
            Number::Integer(data) => Self::Integer(data),
            Number::SmallF64(data) => Self::SmallF64(data),
        }
    }
}

impl<'a> From<Number<'a>> for Primitive<'a> {
    #[inline]
    fn from(value: Number<'a>) -> Self {
        match value {
            Number::Number(d) => Self::Number(d),
            Number::Integer(d) => Self::Integer(d),
            Number::SmallF64(d) => Self::SmallF64(d),
        }
    }
}

impl<'a> From<Number<'a>> for Value<'a> {
    fn from(value: Number<'a>) -> Self {
        match value {
            Number::Number(data) => Self::Number(data),
            Number::Integer(data) => Self::Integer(data),
            Number::SmallF64(data) => Self::SmallF64(data),
        }
    }
}

impl<'a> TryFrom<Numeric<'a>> for Number<'a> {
    type Error = ();
    fn try_from(value: Numeric<'a>) -> Result<Self, Self::Error> {
        match value {
            Numeric::Number(data) => Ok(Number::Number(data)),
            Numeric::Integer(data) => Ok(Number::Integer(data)),
            Numeric::SmallF64(data) => Ok(Number::SmallF64(data)),
            _ => Err(()),
        }
    }
}

impl<'a> TryFrom<Primitive<'a>> for Number<'a> {
    type Error = ();
    fn try_from(value: Primitive<'a>) -> Result<Self, Self::Error> {
        match value {
            Primitive::Number(data) => Ok(Number::Number(data)),
            Primitive::Integer(data) => Ok(Number::Integer(data)),
            Primitive::SmallF64(data) => Ok(Number::SmallF64(data)),
            _ => Err(()),
        }
    }
}

impl<'a> TryFrom<Value<'a>> for Number<'a> {
    type Error = ();
    fn try_from(value: Value<'a>) -> Result<Self, Self::Error> {
        match value {
            Value::Number(data) => Ok(Number::Number(data)),
            Value::Integer(data) => Ok(Number::Integer(data)),
            Value::SmallF64(data) => Ok(Number::SmallF64(data)),
            _ => Err(()),
        }
    }
}

macro_rules! impl_value_from_n {
    ($size: ty) => {
        impl From<$size> for Number<'_> {
            fn from(value: $size) -> Self {
                Number::Integer(SmallInteger::from(value))
            }
        }
    };
}

impl_value_from_n!(u8);
impl_value_from_n!(i8);
impl_value_from_n!(u16);
impl_value_from_n!(i16);
impl_value_from_n!(u32);
impl_value_from_n!(i32);

impl<'a> CreateHeapData<f64, Number<'a>> for Heap {
    fn create(&mut self, data: f64) -> Number<'a> {
        // NOTE: This function cannot currently be implemented
        // directly using `Number::from_f64` as it takes an Agent
        // parameter that we do not have access to here.
        if let Ok(value) = Number::try_from(data) {
            value
        } else {
            let heap_number = Number::alloc_number(self, data);
            Number::Number(heap_number)
        }
    }
}

impl HeapMarkAndSweep for Number<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        if let Self::Number(idx) = self {
            idx.mark_values(queues);
        }
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        if let Self::Number(idx) = self {
            idx.sweep_values(compactions);
        }
    }
}

impl HeapMarkAndSweep for HeapNumber<'static> {
    fn mark_values(&self, queues: &mut WorkQueues) {
        queues.numbers.push(*self);
    }

    fn sweep_values(&mut self, compactions: &CompactionLists) {
        compactions.numbers.shift_index(&mut self.0);
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub(crate) enum NumberRootRepr {
    Integer(SmallInteger) = INTEGER_DISCRIMINANT,
    SmallF64(SmallF64) = FLOAT_DISCRIMINANT,
    HeapRef(HeapRootRef) = 0x80,
}

impl Rootable for Number<'_> {
    type RootRepr = NumberRootRepr;

    #[inline]
    fn to_root_repr(value: Self) -> Result<Self::RootRepr, HeapRootData> {
        match value {
            Self::Number(d) => Err(HeapRootData::from(d)),
            Self::Integer(d) => Ok(Self::RootRepr::Integer(d)),
            Self::SmallF64(d) => Ok(Self::RootRepr::SmallF64(d)),
        }
    }

    #[inline]
    fn from_root_repr(value: &Self::RootRepr) -> Result<Self, HeapRootRef> {
        match *value {
            Self::RootRepr::Integer(d) => Ok(Self::Integer(d)),
            Self::RootRepr::SmallF64(d) => Ok(Self::SmallF64(d)),
            Self::RootRepr::HeapRef(d) => Err(d),
        }
    }

    #[inline]
    fn from_heap_ref(heap_ref: HeapRootRef) -> Self::RootRepr {
        Self::RootRepr::HeapRef(heap_ref)
    }

    #[inline]
    fn from_heap_data(heap_data: HeapRootData) -> Option<Self> {
        match heap_data {
            HeapRootData::Number(d) => Some(Self::Number(d)),
            _ => None,
        }
    }
}

/// ### [6.1.6.1 The Number Type](https://tc39.es/ecma262/#sec-ecmascript-language-types-number-type)
///
/// Heap-allocated [`Number`] data. Accessing the data can only be done through
/// the [`Agent`].
///
/// [`Agent`]: Agent
/// [`Number`]: Number
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct HeapNumber<'a>(BaseIndex<'a, NumberHeapData>);
numeric_handle!(HeapNumber, Number);
arena_vec_access!(HeapNumber, NumberHeapData, numbers, f64);

impl<'a> From<HeapNumber<'a>> for Number<'a> {
    fn from(value: HeapNumber<'a>) -> Self {
        Number::Number(value)
    }
}

impl<'a> TryFrom<Number<'a>> for HeapNumber<'a> {
    type Error = ();

    fn try_from(value: Number<'a>) -> Result<Self, Self::Error> {
        match value {
            Number::Number(s) => Ok(s),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum BitwiseOp {
    And,
    Xor,
    Or,
}

macro_rules! number_value {
    ($name: tt) => {
        crate::ecmascript::number_value!($name, $name);
    };
    ($name: ident, $variant: ident) => {
        crate::ecmascript::numeric_value!($name, $variant);

        impl From<$name> for crate::ecmascript::Number<'static> {
            #[inline(always)]
            fn from(value: $name) -> Self {
                Self::$variant(value)
            }
        }

        impl TryFrom<crate::ecmascript::Number<'_>> for $name {
            type Error = ();

            #[inline]
            fn try_from(value: crate::ecmascript::Number) -> Result<Self, Self::Error> {
                match value {
                    crate::ecmascript::Number::$variant(data) => Ok(data),
                    _ => Err(()),
                }
            }
        }
    };
}
pub(crate) use number_value;

#[cfg(test)]
mod tests {
    use super::Number;
    use crate::{
        ecmascript::{Agent, AgentOptions, HostHooks, Job},
        engine::GcScope,
    };

    #[derive(Default, Debug)]
    struct TestAgentHooks;

    impl HostHooks for TestAgentHooks {
        fn enqueue_generic_job(&self, _job: Job) {
            // No-op
        }

        fn enqueue_promise_job(&self, _job: Job) {
            // no-op
        }

        fn enqueue_timeout_job(&self, _timeout_job: Job, _milliseconds: u64) {
            // no-op
        }
    }

    #[test]
    fn test_greater_than() {
        let hooks = Box::leak(Box::new(TestAgentHooks));
        let mut agent = Agent::new(AgentOptions::default(), hooks);
        let (mut token, mut scope) = GcScope::create_root();
        let gc = GcScope::new(&mut token, &mut scope);

        let x = Number::from_f64(&mut agent, 1.0, gc.nogc());
        let y = Number::from_f64(&mut agent, 2.0, gc.nogc());

        assert_eq!(Number::greater_than(&mut agent, x, y), Some(false));
        assert_eq!(Number::greater_than(&mut agent, y, x), Some(true));

        assert_eq!(Number::greater_than(&mut agent, x, x), Some(false));
        agent.gc(gc);
    }
}

#[cfg(test)]
mod ieee_tests {
    //! Number::multiply, Number::divide and Number::remainder are the
    //! arithmetic of doubles. The specification writes them as steps, one
    //! for each special value; these are those steps, and what `*`, `/` and
    //! `%` give is the same for every pair of the values the steps name and
    //! of ordinary ones.

    fn is_zero(x: f64) -> bool {
        x == 0.0
    }

    fn multiply_by_steps(x: f64, y: f64) -> f64 {
        if x.is_nan() || y.is_nan() {
            return f64::NAN;
        }
        if x.is_infinite() {
            if is_zero(y) {
                return f64::NAN;
            }
            return if y.is_sign_positive() { x } else { -x };
        }
        if y.is_infinite() {
            if is_zero(x) {
                return f64::NAN;
            }
            return if x.is_sign_positive() { y } else { -y };
        }
        if is_zero(x) && x.is_sign_negative() {
            return if y.is_sign_negative() { 0.0 } else { -0.0 };
        }
        if is_zero(y) && y.is_sign_negative() {
            return if x.is_sign_negative() { 0.0 } else { -0.0 };
        }
        x * y
    }

    fn divide_by_steps(x: f64, y: f64) -> f64 {
        if x.is_nan() || y.is_nan() {
            return f64::NAN;
        }
        if x.is_infinite() {
            if y.is_infinite() {
                return f64::NAN;
            }
            return if y.is_sign_positive() { x } else { -x };
        }
        if y.is_infinite() {
            let positive = x.is_sign_positive() == y.is_sign_positive();
            return if positive { 0.0 } else { -0.0 };
        }
        if is_zero(x) {
            if is_zero(y) {
                return f64::NAN;
            }
            return if y.is_sign_positive() { x } else { -x };
        }
        if is_zero(y) {
            let positive = x.is_sign_positive() == y.is_sign_positive();
            return if positive {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            };
        }
        x / y
    }

    fn remainder_by_steps(n: f64, d: f64) -> f64 {
        if n.is_nan() || d.is_nan() {
            return f64::NAN;
        }
        if n.is_infinite() {
            return f64::NAN;
        }
        if d.is_infinite() {
            return n;
        }
        if is_zero(d) {
            return f64::NAN;
        }
        if is_zero(n) {
            return n;
        }
        let r = n % d;
        if r == 0.0 && n.is_sign_negative() {
            return -0.0;
        }
        r
    }

    fn same(found: f64, expected: f64) -> bool {
        (found.is_nan() && expected.is_nan()) || found.to_bits() == expected.to_bits()
    }

    #[test]
    fn the_operators_of_doubles_are_the_steps_of_the_specification() {
        let mut values = vec![
            f64::NAN,
            f64::INFINITY,
            0.0,
            1.0,
            0.5,
            3.0,
            7.25,
            1e21,
            1e-7,
            f64::MAX,
            f64::MIN_POSITIVE,
            5e-324,
            9007199254740992.0,
            9007199254740991.0,
            4294967296.0,
            0.1,
            1.0000001,
            123456.789,
        ];
        let negated: Vec<f64> = values.iter().map(|value| -value).collect();
        values.extend(negated);
        for &x in &values {
            for &y in &values {
                assert!(same(x * y, multiply_by_steps(x, y)), "{x:?} * {y:?}");
                assert!(same(x / y, divide_by_steps(x, y)), "{x:?} / {y:?}");
                assert!(same(x % y, remainder_by_steps(x, y)), "{x:?} % {y:?}");
            }
        }
    }
}

#[cfg(test)]
mod heap_f64_tests {
    use super::*;

    /// A double that [`Number::is_heap_f64`] says is a Number of the heap
    /// is one that the general way makes neither an integer Number nor a
    /// SmallF64; and of those that the general way makes a Number of the
    /// heap, only the ones with a low byte of zeros are left to it.
    #[test]
    fn a_double_told_to_be_of_the_heap_is() {
        fn check(value: f64) {
            let general = Number::try_from(value).is_err();
            if Number::is_heap_f64(value) {
                assert!(general, "{value:?} ({:#x}) fits a Value", value.to_bits());
            } else if general {
                assert_eq!(
                    value.to_bits() & 0xFF,
                    0,
                    "{value:?} was left to the general way"
                );
            }
        }
        let safe = SmallInteger::MAX as f64;
        for value in [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            0.1,
            -0.1,
            1.5,
            1e300,
            -1e300,
            5e-324,
            -5e-324,
            f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
            f64::EPSILON,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            -f64::NAN,
            f64::from_bits(0x7ff8_0000_0000_0001),
            f64::from_bits(0x7ff0_0000_0000_0100),
            f64::from_bits(0xfff0_0000_0000_00ff),
            f64::from_bits(0xfff8_0000_0000_0000),
            f64::from_bits(0x7ff0_0000_0000_0001),
            safe,
            -safe,
            safe + 1.0,
            -safe - 1.0,
            safe + 2.0,
            safe - 1.0,
            9.223_372_036_854_775_8e18,
            -9.223_372_036_854_775_8e18,
            1.844_674_407_370_955_2e19,
            35_184_372_088_832.0,
            35_184_372_088_833.0,
            -35_184_372_088_833.0,
            4_503_599_627_370_497.0,
            4_503_599_627_370_496.5,
            2_251_799_813_685_248.5,
        ] {
            check(value);
        }
        // Whole numbers around every power of two, and what is beside them.
        for power in -10..80 {
            let base = 2f64.powi(power);
            for delta in [
                -3.0, -2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 3.0, 255.0, 256.0, 257.0,
            ] {
                check(base + delta);
                check(-(base + delta));
            }
        }
        // And patterns of bits from a generator.
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        for round in 0..4_000_000u32 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let bits = match round % 4 {
                0 => state,
                // A low byte of zeros.
                1 => state & !0xFF,
                // Whole numbers of every size.
                2 => (((state >> 11) as i64 >> (state % 60)) as f64).to_bits(),
                // Halves and quarters of them.
                _ => (((state >> 11) as i64 >> (state % 60)) as f64 / 4.0).to_bits(),
            };
            check(f64::from_bits(bits));
        }
    }
}
