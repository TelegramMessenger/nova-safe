// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::{
    ecmascript::{PropertyKey, Value},
    engine::{Bindable, GcToken, NoGcScope},
};
use core::fmt::Debug;
use core::hash::{Hash, Hasher};
use core::{marker::PhantomData, mem::size_of, num::NonZeroU32};
use std::any::type_name;

/// A struct containing a non-zero index into an array or
/// vector of `T`s. Due to the non-zero value, the offset
/// in the vector is offset by one.
///
/// This index implies a tracing reference count from this
/// struct to T at the given index.
#[repr(transparent)]
pub(crate) struct BaseIndex<'a, T: ?Sized>(
    NonZeroU32,
    PhantomData<fn() -> T>,
    PhantomData<&'a GcToken>,
);

impl<T: ?Sized> BaseIndex<'_, T> {
    pub(crate) const fn from_index_const(index: usize) -> Self {
        assert!(index < u32::MAX as usize);
        Self(
            NonZeroU32::new(index as u32 + 1).expect("index + 1 is not zero"),
            PhantomData,
            PhantomData,
        )
    }

    pub(crate) const fn from_index_u32_const(index: u32) -> Self {
        assert!(index != u32::MAX);
        Self(
            NonZeroU32::new(index + 1).expect("index + 1 is not zero"),
            PhantomData,
            PhantomData,
        )
    }

    #[inline(always)]
    pub(crate) const fn get_index_u32_const(self) -> u32 {
        self.0.get() - 1
    }

    /// The index of the place `index` of a vector, where there is one: None
    /// for a place beyond those an index can name. For whoever makes an
    /// index where a failed assertion, with its message, would be the only
    /// reason for the function to keep a frame.
    #[inline(always)]
    pub(crate) const fn try_from_index(index: usize) -> Option<Self> {
        if index >= u32::MAX as usize {
            return None;
        }
        match NonZeroU32::new(index as u32 + 1) {
            Some(index) => Some(Self(index, PhantomData, PhantomData)),
            None => None,
        }
    }
}

impl<T: ?Sized> Bindable for BaseIndex<'_, T> {
    type Of<'a> = BaseIndex<'a, T>;

    #[inline(always)]
    fn unbind(self) -> Self::Of<'static> {
        BaseIndex(self.0, PhantomData, PhantomData)
    }

    #[inline(always)]
    fn bind<'a>(self, _: NoGcScope<'a, '_>) -> Self::Of<'a> {
        BaseIndex(self.0, PhantomData, PhantomData)
    }
}

const _INDEX_SIZE_IS_U32: () = assert!(size_of::<BaseIndex<()>>() == size_of::<u32>());
const _OPTION_INDEX_SIZE_IS_U32: () =
    assert!(size_of::<Option<BaseIndex<()>>>() == size_of::<u32>());

impl<T: ?Sized> Debug for BaseIndex<'_, T> {
    #[inline(always)]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        assert!(self.0.get() != 0);
        (&self.0.get() - 1).fmt(f)
    }
}

impl<T: ?Sized> Clone for BaseIndex<'_, T> {
    #[inline(always)]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for BaseIndex<'_, T> {}

impl<T: ?Sized> PartialEq for BaseIndex<'_, T> {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        self.0.eq(&other.0)
    }
}

impl<T: ?Sized> Eq for BaseIndex<'_, T> {}

impl<T: ?Sized> PartialOrd for BaseIndex<'_, T> {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ?Sized> Ord for BaseIndex<'_, T> {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl<T: ?Sized> Hash for BaseIndex<'_, T> {
    #[inline(always)]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<T: ?Sized> BaseIndex<'_, T> {
    /// First valid BaseIndex.
    pub(crate) const ZERO: Self = Self(NonZeroU32::new(1).unwrap(), PhantomData, PhantomData);
    pub(crate) const MAX: Self = Self(NonZeroU32::new(u32::MAX).unwrap(), PhantomData, PhantomData);

    #[inline(always)]
    pub(crate) fn last(vec: &[T]) -> Self
    where
        T: Sized,
    {
        assert!(!vec.is_empty());
        Self::from_index(vec.len() - 1)
    }
}

impl<T> Default for BaseIndex<'_, T> {
    #[inline(always)]
    fn default() -> Self {
        Self::ZERO
    }
}

pub(crate) type ElementIndex<'a> = BaseIndex<'a, [Option<Value<'static>>]>;
pub(crate) type PropertyKeyIndex<'a> = BaseIndex<'a, [PropertyKey<'static>]>;

// Implement Default for ElementIndex: This is done to support Default
// constructor of ElementsVector.
impl Default for ElementIndex<'static> {
    #[inline(always)]
    fn default() -> Self {
        Self(
            const { NonZeroU32::new(1).unwrap() },
            PhantomData,
            PhantomData,
        )
    }
}

impl Default for PropertyKeyIndex<'static> {
    fn default() -> Self {
        Self(
            const { NonZeroU32::new(1).unwrap() },
            PhantomData,
            PhantomData,
        )
    }
}

/// Trait for working with index-based heap handles. The handles are internally
/// limited to 32 bit unsigned values.
pub(crate) trait HeapIndexHandle: Copy + Sized + core::fmt::Debug {
    /// Constant-time value used for discriminant checking only.
    const _DEF: Self;

    /// Convert an index into a heap handle.
    fn from_index(index: usize) -> Self {
        Self::from_index_u32(
            u32::try_from(index)
                .unwrap_or_else(|_| panic!("{} index out of bounds", type_name::<Self>())),
        )
    }
    /// Convert a 32-bit index into a heap handle.
    fn from_index_u32(index: u32) -> Self;

    /// Get the handle's stored index.
    fn get_index(self) -> usize {
        self.get_index_u32() as usize
    }

    /// Get the handle's stored 32-bit index.
    fn get_index_u32(self) -> u32;
}

impl<T: ?Sized> HeapIndexHandle for BaseIndex<'_, T> {
    const _DEF: Self = Self(NonZeroU32::new(u32::MAX).unwrap(), PhantomData, PhantomData);

    #[inline(always)]
    fn from_index_u32(index: u32) -> Self {
        Self::from_index_u32_const(index)
    }

    #[inline(always)]
    fn get_index_u32(self) -> u32 {
        self.get_index_u32_const()
    }
}

macro_rules! index_handle {
    ($name: tt) => {
        crate::heap::index_handle!($name, $name);
    };
    ($name: ident, $variant: ident) => {
        impl crate::engine::Bindable for $name<'_> {
            type Of<'a> = $name<'a>;

            #[inline(always)]
            fn unbind(self) -> Self::Of<'static> {
                $name(crate::engine::Bindable::unbind(self.0))
            }

            #[inline(always)]
            fn bind<'a>(self, gc: crate::engine::NoGcScope<'a, '_>) -> Self::Of<'a> {
                $name(crate::engine::Bindable::bind(self.0, gc))
            }
        }

        impl crate::heap::HeapIndexHandle for $name<'_> {
            const _DEF: Self = Self(crate::heap::BaseIndex::MAX);

            #[inline]
            fn from_index_u32(index: u32) -> Self {
                Self(crate::heap::BaseIndex::from_index_u32(index))
            }

            #[inline]
            fn get_index_u32(self) -> u32 {
                self.0.get_index_u32()
            }
        }

        impl<'a> From<$name<'a>> for crate::engine::HeapRootData {
            #[inline(always)]
            fn from(value: $name<'a>) -> Self {
                Self::$variant(crate::engine::Bindable::unbind(value))
            }
        }

        impl TryFrom<crate::engine::HeapRootData> for $name<'_> {
            type Error = ();

            #[inline]
            fn try_from(value: crate::engine::HeapRootData) -> Result<Self, Self::Error> {
                match value {
                    crate::engine::HeapRootData::$variant(data) => Ok(data),
                    _ => Err(()),
                }
            }
        }
    };
}
pub(crate) use index_handle;
