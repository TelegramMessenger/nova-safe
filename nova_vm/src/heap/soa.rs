// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A struct-of-arrays vector: one `Vec` per field of the element type, so a
//! pass over one field reads only that field's memory. The element type
//! derives [`SoAble`] (`nova_vm_derive::SoAble`), which generates the column
//! storage and the per-field reference structs; every column always has the
//! same length.

use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AllocError;

pub(crate) trait SoAble: Sized {
    type Columns: Default;
    type Ref<'a>: Copy
    where
        Self: 'a;
    type Mut<'a>
    where
        Self: 'a;
    type Slice<'a>: Copy
    where
        Self: 'a;
    type SliceMut<'a>
    where
        Self: 'a;

    fn with_capacity(capacity: usize) -> Self::Columns;
    fn push(columns: &mut Self::Columns, value: Self);
    fn get(columns: &Self::Columns, index: usize) -> Option<Self::Ref<'_>>;
    fn get_mut(columns: &mut Self::Columns, index: usize) -> Option<Self::Mut<'_>>;
    #[allow(dead_code)]
    fn as_slice(columns: &Self::Columns) -> Self::Slice<'_>;
    fn as_mut_slice(columns: &mut Self::Columns) -> Self::SliceMut<'_>;
    fn swap(columns: &mut Self::Columns, a: usize, b: usize);
    fn truncate(columns: &mut Self::Columns, len: usize);
}

pub(crate) struct SoAVec<T: SoAble> {
    columns: T::Columns,
    len: u32,
}

impl<T: SoAble> SoAVec<T> {
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            columns: T::Columns::default(),
            len: 0,
        }
    }

    pub(crate) fn with_capacity(capacity: u32) -> Result<Self, AllocError> {
        Ok(Self {
            columns: T::with_capacity(capacity as usize),
            len: 0,
        })
    }

    #[inline(always)]
    pub(crate) fn len(&self) -> u32 {
        self.len
    }

    #[inline(always)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(crate) fn push(&mut self, value: T) -> Result<(), AllocError> {
        let Some(len) = self.len.checked_add(1) else {
            return Err(AllocError);
        };
        T::push(&mut self.columns, value);
        self.len = len;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn get(&self, index: u32) -> Option<T::Ref<'_>> {
        T::get(&self.columns, index as usize)
    }

    #[inline(always)]
    pub(crate) fn get_mut(&mut self, index: u32) -> Option<T::Mut<'_>> {
        T::get_mut(&mut self.columns, index as usize)
    }

    #[inline]
    pub(crate) fn as_mut_slice(&mut self) -> T::SliceMut<'_> {
        T::as_mut_slice(&mut self.columns)
    }

    /// Keeps the elements for which `f` returns true, in order, calling `f`
    /// once for each element in order.
    pub(crate) fn retain_mut<F>(&mut self, mut f: F)
    where
        F: FnMut(T::Mut<'_>) -> bool,
    {
        let len = self.len as usize;
        let mut kept = 0;
        for index in 0..len {
            let item = T::get_mut(&mut self.columns, index).expect("SoAVec column too short");
            if f(item) {
                if kept != index {
                    T::swap(&mut self.columns, kept, index);
                }
                kept += 1;
            }
        }
        T::truncate(&mut self.columns, kept);
        self.len = kept as u32;
    }
}

impl<T: SoAble> Default for SoAVec<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: SoAble> fmt::Debug for SoAVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SoAVec").field("len", &self.len).finish()
    }
}

impl<T: SoAble> AsRef<SoAVec<T>> for SoAVec<T> {
    #[inline(always)]
    fn as_ref(&self) -> &SoAVec<T> {
        self
    }
}

impl<T: SoAble> AsMut<SoAVec<T>> for SoAVec<T> {
    #[inline(always)]
    fn as_mut(&mut self) -> &mut SoAVec<T> {
        self
    }
}
