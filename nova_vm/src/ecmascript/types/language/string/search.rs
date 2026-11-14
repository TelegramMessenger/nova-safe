// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Substring search in O(n + m) time and O(1) extra space for any needle and
//! haystack: the Two-Way algorithm of Crochemore and Perrin, as Rust's core
//! used it for `str::find`. Both strings may come from a script, so the
//! worst case must stay linear.

trait Sequence<T> {
    fn len(&self) -> usize;
    fn at(&self, index: usize) -> T;
}

struct Forward<'a, T>(&'a [T]);

impl<T: Copy> Sequence<T> for Forward<'_, T> {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn at(&self, index: usize) -> T {
        self.0[index]
    }
}

struct Backward<'a, T>(&'a [T]);

impl<T: Copy> Sequence<T> for Backward<'_, T> {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn at(&self, index: usize) -> T {
        self.0[self.0.len() - 1 - index]
    }
}

/// The critical factorization of a needle: `crit_pos` splits it into a left
/// and a right part, `period` is the period of the needle or, when the
/// needle is not periodic (`long_period`), a shift that cannot skip a match.
struct Factorization {
    crit_pos: usize,
    period: usize,
    long_period: bool,
    /// Bit `x % 64` is set for every element `x` of the needle.
    element_set: u64,
}

#[inline]
fn element_bit<T: Into<u32>>(element: T) -> u64 {
    1 << (element.into() & 63)
}

fn maximal_suffix<T: Copy + Ord>(needle: &impl Sequence<T>, order_greater: bool) -> (usize, usize) {
    let mut left = 0;
    let mut right = 1;
    let mut offset = 0;
    let mut period = 1;
    while right + offset < needle.len() {
        let a = needle.at(right + offset);
        let b = needle.at(left + offset);
        if (a < b && !order_greater) || (a > b && order_greater) {
            right += offset + 1;
            offset = 0;
            period = right - left;
        } else if a == b {
            if offset + 1 == period {
                right += offset + 1;
                offset = 0;
            } else {
                offset += 1;
            }
        } else {
            left = right;
            right += 1;
            offset = 0;
            period = 1;
        }
    }
    (left, period)
}

fn factorize<T: Copy + Ord + Into<u32>>(needle: &impl Sequence<T>) -> Factorization {
    let element_set = (0..needle.len()).fold(0u64, |set, i| set | element_bit(needle.at(i)));
    let (crit_pos_false, period_false) = maximal_suffix(needle, false);
    let (crit_pos_true, period_true) = maximal_suffix(needle, true);
    let (crit_pos, period) = if crit_pos_false > crit_pos_true {
        (crit_pos_false, period_false)
    } else {
        (crit_pos_true, period_true)
    };
    let periodic = period + crit_pos <= needle.len()
        && (0..crit_pos).all(|i| needle.at(i) == needle.at(i + period));
    if periodic {
        Factorization {
            crit_pos,
            period,
            long_period: false,
            element_set,
        }
    } else {
        Factorization {
            crit_pos,
            period: crit_pos.max(needle.len() - crit_pos) + 1,
            long_period: true,
            element_set,
        }
    }
}

/// The first index at or after `from` where `needle` (not empty) occurs in
/// `haystack`.
fn two_way<T: Copy + Ord + Into<u32>>(
    needle: &impl Sequence<T>,
    factorization: &Factorization,
    haystack: &impl Sequence<T>,
    from: usize,
) -> Option<usize> {
    let Factorization {
        crit_pos,
        period,
        long_period,
        element_set,
    } = *factorization;
    let n = needle.len();
    debug_assert!(n > 0);
    let mut position = from;
    let mut memory = 0;
    'search: while position.checked_add(n)? <= haystack.len() {
        if element_set & element_bit(haystack.at(position + n - 1)) == 0 {
            position += n;
            memory = 0;
            continue 'search;
        }
        let start = if long_period {
            crit_pos
        } else {
            crit_pos.max(memory)
        };
        for i in start..n {
            if needle.at(i) != haystack.at(position + i) {
                position += i - crit_pos + 1;
                memory = 0;
                continue 'search;
            }
        }
        let start = if long_period { 0 } else { memory };
        for i in (start..crit_pos).rev() {
            if needle.at(i) != haystack.at(position + i) {
                position += period;
                if !long_period {
                    memory = n - period;
                }
                continue 'search;
            }
        }
        return Some(position);
    }
    None
}

/// Searches one needle in haystacks, from left to right.
pub(crate) struct Searcher<'n, T> {
    needle: &'n [T],
    factorization: Factorization,
}

impl<'n, T: Copy + Ord + Into<u32>> Searcher<'n, T> {
    pub(crate) fn new(needle: &'n [T]) -> Self {
        Self {
            needle,
            factorization: factorize(&Forward(needle)),
        }
    }

    /// The first index at or after `from` where the needle occurs.
    pub(crate) fn find_from(&self, haystack: &[T], from: usize) -> Option<usize> {
        if self.needle.is_empty() {
            return (from <= haystack.len()).then_some(from);
        }
        two_way(
            &Forward(self.needle),
            &self.factorization,
            &Forward(haystack),
            from,
        )
    }
}

/// The first index where `needle` occurs in `haystack`.
pub(crate) fn find<T: Copy + Ord + Into<u32>>(haystack: &[T], needle: &[T]) -> Option<usize> {
    Searcher::new(needle).find_from(haystack, 0)
}

/// The last index where `needle` occurs in `haystack`.
pub(crate) fn rfind<T: Copy + Ord + Into<u32>>(haystack: &[T], needle: &[T]) -> Option<usize> {
    if needle.is_empty() {
        return Some(haystack.len());
    }
    let reversed = Backward(needle);
    let factorization = factorize(&reversed);
    two_way(&reversed, &factorization, &Backward(haystack), 0)
        .map(|index| haystack.len() - index - needle.len())
}
