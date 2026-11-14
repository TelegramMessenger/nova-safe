/// A macro that generates a struct-of-arrays (SoA) container.
///
/// The generated struct holds one `Vec` per field and a single `len` (stored as `u32`), with
/// per-field typed accessors (bounds-checked) and a `push` that appends to every field.
/// Upstream oxc kept all fields in one raw allocation; this version is the same API on safe `Vec`s.
///
/// # Usage
///
/// ```ignore
/// multi_index_vec! {
///     pub struct ScopeTable<ScopeId> {
///         pub parent_ids => parent_ids_mut: Option<ScopeId>,
///         pub node_ids => node_ids_mut: NodeId,
///         pub flags => flags_mut: ScopeFlags,
///     }
/// }
/// ```
///
/// The `=> name_mut` part specifies the mutable accessor name (needed because
/// declarative macros cannot concatenate identifiers).
macro_rules! multi_index_vec {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident<$idx:ty> {
            $(
                $fvis:vis $fname:ident => $fname_mut:ident : $fty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $name {
            $( $fname: ::std::vec::Vec<$fty>, )*
            /// Number of elements currently stored (the length of every field's `Vec`).
            len: u32,
        }

        #[expect(clippy::allow_attributes, reason = "macro-generated methods may not all be used")]
        impl $name {
            /// Maximum number of elements this table can hold while still being
            /// representable by the index type.
            #[inline(always)]
            fn max_len() -> usize {
                <$idx as ::oxc_index::Idx>::MAX.saturating_add(1)
            }

            /// Create a new empty container.
            #[inline]
            $vis fn new() -> Self {
                Self {
                    $( $fname: ::std::vec::Vec::new(), )*
                    len: 0,
                }
            }

            /// Returns the number of elements.
            #[inline]
            $vis fn len(&self) -> usize {
                self.len as usize
            }

            /// Returns `true` if there are no elements.
            #[inline]
            $vis fn is_empty(&self) -> bool {
                self.len == 0
            }

            /// Reserve capacity for at least `additional` more elements.
            $vis fn reserve(&mut self, additional: usize) {
                let required = (self.len as usize).checked_add(additional).expect("capacity overflow");
                assert!(
                    required <= Self::max_len(),
                    "capacity exceeds index type range"
                );
                $( self.$fname.reserve(additional); )*
            }

            /// Push a new element to all field arrays. Returns the index of the new element.
            $vis fn push(&mut self, $( $fname: $fty ),*) -> $idx {
                let idx = self.len as usize;
                assert!(idx < Self::max_len(), "capacity exceeds index type range");
                let new_len = u32::try_from(idx + 1).expect("capacity exceeds u32");
                $( self.$fname.push($fname); )*
                self.len = new_len;
                <$idx as ::oxc_index::Idx>::from_usize(idx)
            }

            /// Iterate over all valid indices.
            $vis fn iter_ids(&self) -> impl Iterator<Item = $idx> {
                let len = self.len as usize;
                (0..len).map(<$idx as ::oxc_index::Idx>::from_usize)
            }

            #[inline(always)]
            fn checked_idx(&self, id: $idx) -> usize {
                let idx = ::oxc_index::Idx::index(id);
                assert!(idx < self.len as usize, "index out of bounds");
                idx
            }

            // Per-field accessors.
            $(
                #[inline]
                $fvis fn $fname(&self, id: $idx) -> &$fty {
                    let idx = self.checked_idx(id);
                    &self.$fname[idx]
                }

                #[inline]
                #[allow(dead_code)]
                $fvis fn $fname_mut(&mut self, id: $idx) -> &mut $fty {
                    let idx = self.checked_idx(id);
                    &mut self.$fname[idx]
                }
            )*
        }

        impl Clone for $name
        where
            $( $fty: Clone, )*
        {
            fn clone(&self) -> Self {
                Self {
                    $( $fname: self.$fname.clone(), )*
                    len: self.len,
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("len", &self.len)
                    .finish()
            }
        }
    };
}

pub(crate) use multi_index_vec;

#[cfg(test)]
mod tests {
    oxc_index::define_index_type! {
        struct TestId = usize;
    }

    multi_index_vec! {
        struct TestTable<TestId> {
            values => values_mut: u32,
        }
    }

    oxc_index::define_index_type! {
        struct SmallId = u8;
        MAX_INDEX = 3;
    }

    multi_index_vec! {
        struct SmallTable<SmallId> {
            values => values_mut: u32,
        }
    }

    oxc_index::define_index_type! {
        struct StringId = usize;
    }

    multi_index_vec! {
        struct StringTable<StringId> {
            values => values_mut: String,
        }
    }

    #[test]
    fn iter_ids_are_valid() {
        let mut table = TestTable::new();
        assert!(table.is_empty());
        table.reserve(3);
        table.push(1);
        table.push(2);
        table.push(3);
        assert_eq!(table.len(), 3);

        let ids = table.iter_ids().collect::<Vec<_>>();
        assert_eq!(ids.len(), 3);
        assert_eq!(usize::from(ids[0]), 0);
        assert_eq!(usize::from(ids[1]), 1);
        assert_eq!(usize::from(ids[2]), 2);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn accessor_panics_on_invalid_index() {
        let mut table = TestTable::new();
        table.push(1);
        let _ = table.values(TestId::from(1usize));
    }

    #[test]
    #[should_panic(expected = "capacity exceeds index type range")]
    fn reserve_panics_when_exceeding_index_range() {
        let mut table = SmallTable::new();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
        let _ = table.iter_ids().count();
        table.reserve(5);
    }

    #[test]
    #[should_panic(expected = "capacity exceeds index type range")]
    fn push_panics_when_exceeding_index_range() {
        let mut table = SmallTable::new();
        let id = table.push(1);
        assert_eq!(*table.values(id), 1);
        table.push(2);
        table.push(3);
        table.push(4);
        table.push(5);
    }

    #[test]
    fn clone_clones_non_copy_fields() {
        let mut table = StringTable::new();
        assert!(table.is_empty());
        table.reserve(1);
        let id = table.push(String::from("a"));
        assert_eq!(table.len(), 1);
        let _ = table.iter_ids().count();
        let mut clone = table.clone();

        table.values_mut(id).push('1');
        clone.values_mut(id).push('2');

        assert_eq!(table.values(id), "a1");
        assert_eq!(clone.values(id), "a2");
    }
}
