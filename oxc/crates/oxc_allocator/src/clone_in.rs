use std::{
    cell::Cell,
    hash::{BuildHasher, Hash},
};

use crate::{Allocator, Box, HashMap, Vec};

/// A trait to explicitly clone an object into an arena allocator.
///
/// As a convention `Cloned` associated type should always be the same as `Self`,
/// It'd only differ in the lifetime, Here's an example:
///
/// ```
/// # use oxc_allocator::{Allocator, CloneIn, Vec};
/// # struct Struct<'a> {a: Vec<'a, u8>, b: u8}
///
/// impl<'old_alloc, 'new_alloc> CloneIn<'new_alloc> for Struct<'old_alloc> {
///     type Cloned = Struct<'new_alloc>;
///     fn clone_in(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
///         Struct { a: self.a.clone_in(allocator), b: self.b.clone_in(allocator) }
///     }
/// }
/// ```
///
/// Implementations of this trait on non-allocated items usually short-circuit to `Clone::clone`;
/// However, it **isn't** guaranteed.
///
pub trait CloneIn<'new_alloc>: Sized {
    /// The type of the cloned object.
    ///
    /// This should always be `Self` with a different lifetime.
    type Cloned;

    /// Clone `self` into the given `allocator`. `allocator` may be the same one
    /// that `self` is already in.
    fn clone_in(&self, allocator: &'new_alloc Allocator) -> Self::Cloned;

    /// Almost identical as `clone_in`, but for some special type, it will also clone the semantic ids.
    /// Please use this method only if you make sure semantic info is synced with the ast node.
    #[inline]
    fn clone_in_with_semantic_ids(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        self.clone_in(allocator)
    }
}

impl<'alloc, T, C> CloneIn<'alloc> for Option<T>
where
    T: CloneIn<'alloc, Cloned = C>,
{
    type Cloned = Option<C>;

    fn clone_in(&self, allocator: &'alloc Allocator) -> Self::Cloned {
        self.as_ref().map(|it| it.clone_in(allocator))
    }

    fn clone_in_with_semantic_ids(&self, allocator: &'alloc Allocator) -> Self::Cloned {
        self.as_ref().map(|it| it.clone_in_with_semantic_ids(allocator))
    }
}

impl<'new_alloc, T, C> CloneIn<'new_alloc> for Box<'_, T>
where
    T: CloneIn<'new_alloc, Cloned = C>,
{
    type Cloned = Box<'new_alloc, C>;

    fn clone_in(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        Box::new_in(self.as_ref().clone_in(allocator), allocator)
    }

    fn clone_in_with_semantic_ids(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        Box::new_in(self.as_ref().clone_in_with_semantic_ids(allocator), allocator)
    }
}

impl<'new_alloc, T, C> CloneIn<'new_alloc> for Box<'_, [T]>
where
    T: CloneIn<'new_alloc, Cloned = C>,
{
    type Cloned = Box<'new_alloc, [C]>;

    fn clone_in(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        Box::from_std(self.iter().map(|item| item.clone_in(allocator)).collect())
    }

    fn clone_in_with_semantic_ids(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        Box::from_std(self.iter().map(|item| item.clone_in_with_semantic_ids(allocator)).collect())
    }
}

impl<'new_alloc, T, C> CloneIn<'new_alloc> for Vec<'_, T>
where
    T: CloneIn<'new_alloc, Cloned = C>,
    C: 'new_alloc,
{
    type Cloned = Vec<'new_alloc, C>;

    fn clone_in(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        Vec::from_iter_in(self.iter().map(|item| item.clone_in(allocator)), allocator)
    }

    fn clone_in_with_semantic_ids(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        Vec::from_iter_in(self.iter().map(|item| item.clone_in_with_semantic_ids(allocator)), allocator)
    }
}

impl<'new_alloc, K, V, CK, CV, S> CloneIn<'new_alloc> for HashMap<'_, K, V, S>
where
    K: CloneIn<'new_alloc, Cloned = CK>,
    V: CloneIn<'new_alloc, Cloned = CV>,
    CK: Hash + Eq,
    S: Default + BuildHasher,
{
    type Cloned = HashMap<'new_alloc, CK, CV, S>;

    fn clone_in(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        // Keys in original hash map are guaranteed to be unique.
        // Unfortunately, we have no static guarantee that `CloneIn` maintains that uniqueness
        // - original keys (`K`) are guaranteed unique, but cloned keys (`CK`) might not be.
        // If we did have that guarantee, we could use the faster `insert_unique_unchecked` here.
        // `hashbrown::HashMap` also has a faster cloning method in its `Clone` implementation,
        // but those APIs are not exposed, and `Clone` doesn't support custom allocators.
        // So sadly this is a lot slower than it could be, especially for `Copy` types.
        let mut cloned = HashMap::with_capacity_in(self.len(), allocator);
        for (key, value) in self {
            cloned.insert(key.clone_in(allocator), value.clone_in(allocator));
        }
        cloned
    }

    fn clone_in_with_semantic_ids(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        let mut cloned = HashMap::with_capacity_in(self.len(), allocator);
        for (key, value) in self {
            cloned.insert(
                key.clone_in_with_semantic_ids(allocator),
                value.clone_in_with_semantic_ids(allocator),
            );
        }
        cloned
    }
}

impl<'alloc, T: Copy> CloneIn<'alloc> for Cell<T> {
    type Cloned = Cell<T>;

    fn clone_in(&self, _: &'alloc Allocator) -> Self::Cloned {
        Cell::new(self.get())
    }
}

impl<'new_alloc> CloneIn<'new_alloc> for &str {
    type Cloned = &'new_alloc str;

    fn clone_in(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        allocator.alloc_str(self)
    }
}

macro_rules! impl_clone_in {
    ($($t:ty)*) => {
        $(
            impl<'alloc> CloneIn<'alloc> for $t {
                type Cloned = Self;
                #[inline(always)]
                fn clone_in(&self, _: &'alloc Allocator) -> Self {
                    *self
                }
            }
        )*
    }
}

impl_clone_in! {
    usize u8 u16 u32 u64 u128
    isize i8 i16 i32 i64 i128
    f32 f64
    bool char
}

#[cfg(test)]
mod test {
    use super::{Allocator, CloneIn, HashMap, Vec};

    #[test]
    fn clone_in_boxed_slice() {
        let allocator = Allocator::default();

        let mut original = Vec::from_iter_in([1, 2, 3], &allocator).into_boxed_slice();

        let cloned = original.clone_in(&allocator);
        let cloned2 = original.clone_in_with_semantic_ids(&allocator);
        original[1] = 4;

        assert_eq!(original.as_ref(), &[1, 4, 3]);
        assert_eq!(cloned.as_ref(), &[1, 2, 3]);
        assert_eq!(cloned2.as_ref(), &[1, 2, 3]);
    }

    #[test]
    fn clone_in_vec() {
        let allocator = Allocator::default();

        let mut original = Vec::with_capacity_in(8, &allocator);
        original.extend_from_slice(&[1, 2, 3]);

        let cloned = original.clone_in(&allocator);
        let cloned2 = original.clone_in_with_semantic_ids(&allocator);
        original[1] = 4;

        assert_eq!(original.as_slice(), &[1, 4, 3]);
        assert_eq!(cloned.as_slice(), &[1, 2, 3]);
        assert_eq!(cloned.capacity(), 3);
        assert_eq!(cloned2.as_slice(), &[1, 2, 3]);
        assert_eq!(cloned2.capacity(), 3);
    }

    #[test]
    fn clone_in_hash_map() {
        let allocator = Allocator::default();

        let mut original: HashMap<'_, &str, &str> = HashMap::with_capacity_in(8, &allocator);
        original.extend(&[("x", "xx"), ("y", "yy"), ("z", "zz")]);

        let cloned = original.clone_in(&allocator);
        let cloned2 = original.clone_in_with_semantic_ids(&allocator);
        *original.get_mut("y").unwrap() = "changed";

        let mut original_as_vec = original.iter().collect::<std::vec::Vec<_>>();
        original_as_vec.sort_unstable();
        assert_eq!(original_as_vec, &[(&"x", &"xx"), (&"y", &"changed"), (&"z", &"zz")]);

        assert_eq!(cloned.capacity(), 3);
        let mut cloned_as_vec = cloned.iter().collect::<std::vec::Vec<_>>();
        cloned_as_vec.sort_unstable();
        assert_eq!(cloned_as_vec, &[(&"x", &"xx"), (&"y", &"yy"), (&"z", &"zz")]);

        assert_eq!(cloned2.capacity(), 3);
        let mut cloned2_as_vec = cloned2.iter().collect::<std::vec::Vec<_>>();
        cloned2_as_vec.sort_unstable();
        assert_eq!(cloned2_as_vec, &[(&"x", &"xx"), (&"y", &"yy"), (&"z", &"zz")]);
    }
}
