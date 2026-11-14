use core::cmp::Eq;
use core::fmt;
use core::marker::PhantomData;
use core::ops;

/// A trait which references a position in an input string.
/// The intent is that this may be satisfied via indexes or pointers.
/// Positions must be subtractable, producing usize; they also obey other "pointer arithmetic" ideas.
pub trait PositionType:
    core::fmt::Debug + Copy + Clone + PartialEq + Eq + PartialOrd + Ord
where
    Self: ops::Add<usize, Output = Self>,
    Self: ops::Sub<usize, Output = Self>,
    Self: ops::Sub<Self, Output = usize>,
    Self: ops::AddAssign<usize>,
    Self: ops::SubAssign<usize>,
{
}

/// Choose the preferred position type with this alias.
pub type DefPosition<'a> = IndexPosition<'a>;

/// A simple index-based position.
/// It remembers the lifetime of the slice it is tied to.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IndexPosition<'a>(usize, PhantomData<&'a ()>);

impl<'a> fmt::Debug for IndexPosition<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IndexPosition").field(&self.0).finish()
    }
}

#[allow(dead_code)]
impl IndexPosition<'_> {
    #[inline(always)]
    pub fn new(pos: usize) -> Self {
        Self(pos, PhantomData)
    }
}

impl ops::Add<usize> for IndexPosition<'_> {
    type Output = Self;

    #[inline(always)]
    fn add(self, rhs: usize) -> Self::Output {
        debug_assert!(self.0 + rhs >= self.0, "Overflow");
        IndexPosition(self.0 + rhs, PhantomData)
    }
}

impl ops::AddAssign<usize> for IndexPosition<'_> {
    #[inline(always)]
    fn add_assign(&mut self, rhs: usize) {
        *self = *self + rhs;
    }
}

impl ops::SubAssign<usize> for IndexPosition<'_> {
    #[inline(always)]
    fn sub_assign(&mut self, rhs: usize) {
        *self = *self - rhs;
    }
}

impl<'a> ops::Sub<IndexPosition<'a>> for IndexPosition<'a> {
    type Output = usize;

    #[inline(always)]
    fn sub(self, rhs: Self) -> Self::Output {
        debug_assert!(self.0 >= rhs.0, "Underflow");
        self.0 - rhs.0
    }
}

impl ops::Sub<usize> for IndexPosition<'_> {
    type Output = Self;

    #[inline(always)]
    fn sub(self, rhs: usize) -> Self::Output {
        debug_assert!(self.0 >= rhs, "Underflow");
        IndexPosition(self.0 - rhs, PhantomData)
    }
}

impl PositionType for IndexPosition<'_> {}
