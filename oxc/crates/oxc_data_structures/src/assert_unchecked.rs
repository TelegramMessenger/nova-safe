//! `assert_unchecked!` macro.

/// Asserts a condition in debug builds; a no-op in release builds.
///
/// It used to wrap [`std::hint::assert_unchecked`], which is undefined behaviour when the condition is
/// false. It is now [`debug_assert!`]: a false condition panics in a debug build and is ignored in a
/// release build, so callers must not rely on it for memory safety. Calls inside an `unsafe {}` block
/// still compile.
///
/// This macro can be used in const context.
///
/// # Examples
/// ```
/// // Requires `assert_unchecked` feature
/// use oxc_data_structures::assert_unchecked;
///
/// fn first(slice: &[u8]) -> u8 {
///     assert_unchecked!(!slice.is_empty(), "Oh no. Slice is empty.");
///     slice[0]
/// }
/// ```
#[macro_export]
macro_rules! assert_unchecked {
    ($cond:expr) => (assert_unchecked!($cond,));
    ($cond:expr, $($arg:tt)*) => ({
        debug_assert!($cond, $($arg)*);
    })
}

#[cfg(test)]
mod test {
    mod pass {
        #[test]
        fn plain() {
            assert_unchecked!(0 == 0);
        }

        #[test]
        fn string_literal() {
            assert_unchecked!(0 == 0, "String literal");
        }

        #[test]
        fn fmt_string() {
            assert_unchecked!(0 == 0, "Format str {} {:?}", 123, 456);
        }
    }

    #[cfg(debug_assertions)]
    mod fail {
        #[test]
        #[should_panic(expected = "assertion failed: 0 == 1")]
        fn plain() {
            assert_unchecked!(0 == 1);
        }

        #[test]
        #[should_panic(expected = "String literal")]
        fn string_literal() {
            assert_unchecked!(0 == 1, "String literal");
        }

        #[test]
        #[should_panic(expected = "Format str: 123 456")]
        fn fmt_string() {
            assert_unchecked!(0 == 1, "Format str: {} {:?}", 123, 456);
        }
    }
}
