use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use crate::Allocator;

/// A box tied to the lifetime of an [`Allocator`]. The value is owned by a
/// `std` box; the allocator only names the lifetime of the tree it is part of.
#[repr(transparent)]
pub struct Box<'alloc, T: ?Sized>(std::boxed::Box<T>, PhantomData<&'alloc ()>);

impl<T> Box<'_, T> {
    #[inline]
    pub fn new_in(value: T, _allocator: &Allocator) -> Self {
        Self(std::boxed::Box::new(value), PhantomData)
    }

    #[inline]
    pub fn unbox(self) -> T {
        *self.0
    }
}

impl<T: ?Sized> Box<'_, T> {
    #[inline]
    pub fn from_std(boxed: std::boxed::Box<T>) -> Self {
        Self(boxed, PhantomData)
    }

    #[inline]
    pub fn into_std(self) -> std::boxed::Box<T> {
        self.0
    }
}

impl<T> Box<'static, [T]> {
    #[inline]
    pub fn new_empty_boxed_slice() -> Self {
        Self(std::boxed::Box::new([]), PhantomData)
    }
}

impl<T: ?Sized> Deref for Box<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: ?Sized> DerefMut for Box<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: ?Sized> AsRef<T> for Box<'_, T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsMut<T> for Box<'_, T> {
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized + Display> Display for Box<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T: ?Sized + Debug> Debug for Box<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T: Hash> Hash for Box<'_, T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.deref().hash(state);
    }
}
