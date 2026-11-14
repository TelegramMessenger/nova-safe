//! Identifier string type.

use std::{
    borrow::{Borrow, Cow},
    fmt, hash,
    ops::Deref,
    str,
};

use oxc_allocator::{
    Allocator, CloneIn, Dummy, FromIn, IdentBuildHasher, StringBuilder as ArenaStringBuilder,
};
#[cfg(feature = "serialize")]
use oxc_estree::{ESTree, JsonSafeString, Serializer as ESTreeSerializer};
#[cfg(feature = "serialize")]
use serde::{Serialize, Serializer as SerdeSerializer};

use crate::{CompactStr, Str};

/// An identifier string for oxc_allocator.
///
/// nova-safe stores only the `&str` (16 bytes, as upstream's pointer plus packed length and
/// hash). `Hash` hashes the `str`, so `Ident: Borrow<str>` holds under every hasher; maps keyed
/// by identifiers use [`IdentBuildHasher`], std's SipHash-1-3 under random keys.
///
/// Use [CompactStr] with [Ident::to_compact_str] or [Ident::into_compact_str] for
/// the lifetimeless form.
#[derive(Clone, Copy)]
pub struct Ident<'a> {
    text: &'a str,
}

impl<'a> Ident<'a> {
    #[cfg(test)]
    #[expect(clippy::cast_possible_truncation)]
    const fn ident_len(&self) -> u32 {
        self.text.len() as u32
    }


    /// Create a new [`Ident`] from a `&'a str`.
    #[inline(always)]
    #[expect(clippy::inline_always)]
    pub const fn new_const(s: &'a str) -> Self {
        Self { text: s }
    }

    /// An empty [`Ident`].
    #[inline]
    pub const fn empty() -> Self {
        Self { text: "" }
    }

    /// The identifier text.
    #[expect(clippy::inline_always)]
    #[inline(always)]
    pub fn as_str(&self) -> &'a str {
        self.text
    }

    #[expect(clippy::inline_always)]
    #[inline(always)] // Because this is a no-op
    pub fn as_arena_str(&self) -> Str<'a> {
        Str::from(self.as_str())
    }

    /// Convert this [`Ident`] into a [`String`].
    ///
    /// This is the explicit form of [`Into<String>`], which [`Ident`] also implements.
    #[inline]
    pub fn into_string(self) -> String {
        String::from(self.as_str())
    }

    /// Convert this [`Ident`] into a [`CompactStr`].
    ///
    /// This is the explicit form of [`Into<CompactStr>`], which [`Ident`] also implements.
    #[inline]
    pub fn into_compact_str(self) -> CompactStr {
        CompactStr::new(self.as_str())
    }

    /// Convert this [`Ident`] into a [`CompactStr`] without consuming `self`.
    #[inline]
    pub fn to_compact_str(self) -> CompactStr {
        CompactStr::new(self.as_str())
    }

    /// Create new [`Ident`] from a fixed-size array of `&str`s concatenated together,
    /// allocated in the given `allocator`.
    ///
    /// # Panics
    ///
    /// Panics if the sum of length of all strings exceeds `isize::MAX`.
    // `#[inline(always)]` because want compiler to be able to optimize where some of `strings`
    // are statically known. See `Allocator::alloc_concat_strs_array`.
    #[expect(clippy::inline_always)]
    #[inline(always)]
    pub fn from_strs_array_in<const N: usize>(
        strings: [&str; N],
        allocator: &'a Allocator,
    ) -> Ident<'a> {
        Self::from(allocator.alloc_concat_strs_array(strings))
    }

    /// Convert a [`Cow<'a, str>`] to an [`Ident<'a>`].
    ///
    /// If the `Cow` borrows a string from arena, returns an `Ident` which references that same string,
    /// without allocating a new one.
    ///
    /// If the `Cow` is owned, allocates the string into arena to generate a new `Ident`.
    #[inline]
    pub fn from_cow_in(value: &Cow<'a, str>, allocator: &'a Allocator) -> Ident<'a> {
        match value {
            Cow::Borrowed(s) => Ident::from(*s),
            Cow::Owned(s) => Ident::from_in(s, allocator),
        }
    }
}

impl<'new_alloc> CloneIn<'new_alloc> for Ident<'_> {
    type Cloned = Ident<'new_alloc>;

    /// Clone the identifier into a new allocator, preserving the precomputed hash.
    #[inline]
    fn clone_in(&self, allocator: &'new_alloc Allocator) -> Self::Cloned {
        Ident::new_const(allocator.alloc_str(self.as_str()))
    }
}

impl<'a> Dummy<'a> for Ident<'a> {
    /// Create a dummy [`Ident`].
    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn dummy(_allocator: &'a Allocator) -> Self {
        Ident::empty()
    }
}

impl<'alloc> FromIn<'alloc, &Ident<'alloc>> for Ident<'alloc> {
    #[expect(clippy::inline_always)]
    #[inline(always)] // Because this is a no-op
    fn from_in(s: &Ident<'alloc>, _: &'alloc Allocator) -> Self {
        *s
    }
}

impl<'alloc> FromIn<'alloc, &str> for Ident<'alloc> {
    #[inline]
    fn from_in(s: &str, allocator: &'alloc Allocator) -> Self {
        Self::from(allocator.alloc_str(s))
    }
}

impl<'alloc> FromIn<'alloc, String> for Ident<'alloc> {
    #[inline]
    fn from_in(s: String, allocator: &'alloc Allocator) -> Self {
        Self::from_in(s.as_str(), allocator)
    }
}

impl<'alloc> FromIn<'alloc, &String> for Ident<'alloc> {
    #[inline]
    fn from_in(s: &String, allocator: &'alloc Allocator) -> Self {
        Self::from_in(s.as_str(), allocator)
    }
}

impl<'alloc> FromIn<'alloc, Cow<'_, str>> for Ident<'alloc> {
    #[inline]
    fn from_in(s: Cow<'_, str>, allocator: &'alloc Allocator) -> Self {
        Self::from_in(&*s, allocator)
    }
}

impl<'a> From<&'a str> for Ident<'a> {
    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn from(s: &'a str) -> Self {
        Self::new_const(s)
    }
}

impl<'alloc> From<ArenaStringBuilder<'alloc>> for Ident<'alloc> {
    #[inline]
    fn from(s: ArenaStringBuilder<'alloc>) -> Self {
        Self::from(s.into_str())
    }
}

impl<'a> From<Ident<'a>> for &'a str {
    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn from(s: Ident<'a>) -> Self {
        s.as_str()
    }
}

impl<'a> From<Ident<'a>> for Str<'a> {
    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn from(s: Ident<'a>) -> Self {
        s.as_arena_str()
    }
}

impl<'a> From<Str<'a>> for Ident<'a> {
    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn from(s: Str<'a>) -> Self {
        Self::from(s.as_str())
    }
}

impl From<Ident<'_>> for CompactStr {
    #[inline]
    fn from(val: Ident<'_>) -> Self {
        val.into_compact_str()
    }
}

impl From<Ident<'_>> for String {
    #[inline]
    fn from(val: Ident<'_>) -> Self {
        val.into_string()
    }
}

impl<'a> From<Ident<'a>> for Cow<'a, str> {
    #[inline]
    fn from(value: Ident<'a>) -> Self {
        Cow::Borrowed(value.as_str())
    }
}

impl Deref for Ident<'_> {
    type Target = str;

    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for Ident<'_> {
    #[expect(clippy::inline_always)]
    #[inline(always)]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Allows looking up an `Ident`-keyed map with a `&str` key. `Ident`'s `Eq` and `Hash` are its
/// `str`'s.
impl Borrow<str> for Ident<'_> {
    #[inline]
    fn borrow(&self) -> &str {
        self.text
    }
}

impl Eq for Ident<'_> {}

impl PartialEq for Ident<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl PartialEq<str> for Ident<'_> {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Ident<'_> {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<Ident<'_>> for &str {
    #[inline]
    fn eq(&self, other: &Ident<'_>) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<Ident<'_>> for Cow<'_, str> {
    #[inline]
    fn eq(&self, other: &Ident<'_>) -> bool {
        self.as_ref() == other.as_str()
    }
}

impl PartialEq<&Ident<'_>> for Ident<'_> {
    #[inline]
    fn eq(&self, other: &&Ident<'_>) -> bool {
        self == *other
    }
}

impl PartialEq<Str<'_>> for Ident<'_> {
    #[inline]
    fn eq(&self, other: &Str<'_>) -> bool {
        self.as_str() == other.as_str()
    }
}

impl hash::Hash for Ident<'_> {
    /// Hash the `str`, as `Borrow<str>` requires.
    #[inline]
    fn hash<H: hash::Hasher>(&self, hasher: &mut H) {
        hash::Hash::hash(self.text, hasher);
    }
}

impl fmt::Debug for Ident<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for Ident<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

#[cfg(feature = "serialize")]
impl Serialize for Ident<'_> {
    #[inline] // Because it just delegates
    fn serialize<S: SerdeSerializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Serialize::serialize(self.as_str(), serializer)
    }
}

#[cfg(feature = "serialize")]
impl ESTree for Ident<'_> {
    #[inline] // Because it just delegates
    fn serialize<S: ESTreeSerializer>(&self, serializer: S) {
        // `Ident`s are always JSON-safe
        ESTree::serialize(&JsonSafeString(self.as_str()), serializer);
    }
}

/// Hash map keyed by [`Ident`], under the keyed [`IdentBuildHasher`].
pub type IdentHashMap<'a, V> = std::collections::HashMap<Ident<'a>, V, IdentBuildHasher>;

/// Arena-allocated hash map keyed by [`Ident`], under the keyed [`IdentBuildHasher`].
pub type ArenaIdentHashMap<'alloc, V> =
    oxc_allocator::HashMap<'alloc, Ident<'alloc>, V, IdentBuildHasher>;

/// Hash set of [`Ident`], under the keyed [`IdentBuildHasher`].
pub type IdentHashSet<'a> = std::collections::HashSet<Ident<'a>, IdentBuildHasher>;

pub const AGGREGATE_ERROR: Ident<'static> = Ident::new_const("AggregateError");
pub const ARGUMENTS: Ident<'static> = Ident::new_const("arguments");
pub const ARRAY: Ident<'static> = Ident::new_const("Array");
pub const ERROR: Ident<'static> = Ident::new_const("Error");
pub const EXPORTS: Ident<'static> = Ident::new_const("exports");
pub const FUNCTION: Ident<'static> = Ident::new_const("Function");
pub const GLOBAL_THIS: Ident<'static> = Ident::new_const("globalThis");
pub const MATH: Ident<'static> = Ident::new_const("Math");
pub const MODULE: Ident<'static> = Ident::new_const("module");
pub const OBJECT: Ident<'static> = Ident::new_const("Object");
pub const PROCESS: Ident<'static> = Ident::new_const("process");
pub const REG_EXP: Ident<'static> = Ident::new_const("RegExp");
pub const REQUIRE: Ident<'static> = Ident::new_const("require");
pub const TYPE_ERROR: Ident<'static> = Ident::new_const("TypeError");

/// Creates an [`Ident`] using interpolation of runtime expressions.
///
/// Identical to [`std`'s `format!` macro](std::format), except:
///
/// * First argument is the allocator.
/// * Produces an [`Ident`] instead of a [`String`].
///
/// The string is built in the arena, without allocating an intermediate `String`.
///
/// # Panics
///
/// Panics if a formatting trait implementation returns an error.
///
/// # Example
///
/// ```
/// use oxc_allocator::Allocator;
/// use oxc_str::format_ident;
/// let allocator = Allocator::new();
///
/// let s1 = "foo";
/// let s2 = "bar";
/// let formatted = format_ident!(&allocator, "{s1}.{s2}");
/// assert_eq!(formatted, "foo.bar");
/// ```
#[macro_export]
macro_rules! format_ident {
    ($alloc:expr, $($arg:tt)*) => {{
        use ::std::{write, fmt::Write};
        use $crate::{Ident, __internal::ArenaStringBuilder};

        let mut s = ArenaStringBuilder::new_in($alloc);
        write!(s, $($arg)*).unwrap();
        Ident::from(s)
    }}
}

#[cfg(test)]
mod test {
    use std::hash::BuildHasher;

    use oxc_allocator::Allocator;

    use super::*;

    #[test]
    fn ident_size() {
        #[cfg(target_pointer_width = "64")]
        assert_eq!(std::mem::size_of::<Ident<'_>>(), 16);
        #[cfg(target_pointer_width = "32")]
        assert_eq!(std::mem::size_of::<Ident<'_>>(), 12);
    }

    #[test]
    fn ident_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<Ident<'_>>();
        assert_sync::<Ident<'_>>();
    }

    #[test]
    fn ident_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<Ident<'_>>();
    }

    #[test]
    fn ident_from_str() {
        let s = "hello";
        let ident = Ident::from(s);
        assert_eq!(ident.as_str(), "hello");
    }

    #[test]
    fn ident_empty() {
        let ident = Ident::empty();
        assert_eq!(ident.as_str(), "");
        assert_eq!(ident.ident_len(), 0);
    }

    #[test]
    fn ident_new_const() {
        let ident = Ident::new_const("world");
        assert_eq!(ident.as_str(), "world");
    }

    #[test]
    fn ident_eq() {
        let a = Ident::from("foo");
        let b = Ident::from("foo");
        let c = Ident::from("bar");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn ident_eq_str() {
        let ident = Ident::from("hello");
        assert_eq!(ident, "hello");
        assert_ne!(ident, "world");
    }

    #[test]
    fn ident_hash_consistency() {
        let build_hasher = IdentBuildHasher;

        // Hash an Ident via BuildHasher::hash_one
        let ident = Ident::from("fooBar");
        let ident_hash_val = build_hasher.hash_one(ident);

        // Hash a &str through the same hasher
        let str_hash = build_hasher.hash_one("fooBar");

        assert_eq!(ident_hash_val, str_hash);
    }

    #[test]
    fn ident_hashes_like_its_str() {
        let ident = Ident::from("fooBarBazQuxQuuxCorge");
        let state = std::hash::RandomState::new();
        assert_eq!(state.hash_one(ident), state.hash_one("fooBarBazQuxQuuxCorge"));
    }

    #[test]
    fn ident_hashmap_str_lookup() {
        let mut map = IdentHashMap::default();
        map.insert(Ident::from("key1"), 1);
        map.insert(Ident::from("key2"), 2);

        // Lookup with &str via Borrow
        assert_eq!(map.get("key1"), Some(&1));
        assert_eq!(map.get("key2"), Some(&2));
        assert_eq!(map.get("key3"), None);
    }

    #[test]
    fn ident_hashmap_ident_lookup() {
        let mut map = IdentHashMap::default();
        map.insert(Ident::from("key1"), 1);

        let key = Ident::from("key1");
        assert_eq!(map.get(&key), Some(&1));
    }

    #[test]
    fn ident_clone_in() {
        let allocator = Allocator::new();
        let original = Ident::from("test");
        let cloned = original.clone_in(&allocator);
        assert_eq!(original.as_str(), cloned.as_str());
        assert_eq!(original, cloned);
        assert_eq!(IdentBuildHasher.hash_one(original), IdentBuildHasher.hash_one(cloned));
    }

    #[test]
    fn ident_deref() {
        let ident = Ident::from("hello");
        // Should be able to call str methods
        assert!(ident.starts_with("hel")); // spellchecker:disable-line
        assert_eq!(ident.len(), 5);
    }

    #[test]
    fn ident_display() {
        let ident = Ident::from("test");
        assert_eq!(format!("{ident}"), "test");
    }

    #[test]
    fn ident_debug() {
        let ident = Ident::from("test");
        assert_eq!(format!("{ident:?}"), "\"test\"");
    }

    #[test]
    fn arena_ident_hashmap() {
        let allocator = Allocator::new();
        let mut map = ArenaIdentHashMap::new_in(&allocator);
        let key = Ident::from_in("hello", &allocator);
        map.insert(key, 42);
        assert_eq!(map.get("hello"), Some(&42));
        assert_eq!(map.get(&Ident::from("hello")), Some(&42));
    }
}
