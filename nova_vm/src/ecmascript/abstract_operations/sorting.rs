// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A stable sort for comparisons that run JavaScript.
//!
//! The standard library's sorts may panic when the comparison is not a total
//! order, which a JavaScript comparator (`() => Math.random() - 0.5`) need
//! not be. This merge sort only ever reads and writes in bounds, whatever
//! the comparison returns, and stops at the first error.

use core::cmp::Ordering;

const RUN: usize = 16;

/// Sort `items` stably by `compare`, stopping at the first error.
///
/// `compare` is called at most O(n log n) times and the result is a
/// permutation of the input even when `compare` is inconsistent.
pub(crate) fn try_merge_sort_by<T: Clone, E>(
    items: &mut [T],
    mut compare: impl FnMut(&T, &T) -> Result<Ordering, E>,
) -> Result<(), E> {
    let len = items.len();
    if len < 2 {
        return Ok(());
    }
    for start in (0..len).step_by(RUN) {
        let end = (start + RUN).min(len);
        for i in start + 1..end {
            let mut j = i;
            while j > start && compare(&items[j - 1], &items[j])? == Ordering::Greater {
                items.swap(j - 1, j);
                j -= 1;
            }
        }
    }
    if len <= RUN {
        return Ok(());
    }
    let mut buffer = items.to_vec();
    let mut width = RUN;
    let mut sorted_in_items = true;
    while width < len {
        if sorted_in_items {
            merge_runs(items, &mut buffer, width, &mut compare)?;
        } else {
            merge_runs(&buffer, items, width, &mut compare)?;
        }
        sorted_in_items = !sorted_in_items;
        width = width.saturating_mul(2);
    }
    if !sorted_in_items {
        items.clone_from_slice(&buffer);
    }
    Ok(())
}

fn merge_runs<T: Clone, E>(
    source: &[T],
    target: &mut [T],
    width: usize,
    compare: &mut impl FnMut(&T, &T) -> Result<Ordering, E>,
) -> Result<(), E> {
    let len = source.len();
    let mut start = 0;
    while start < len {
        let middle = start.saturating_add(width).min(len);
        let end = middle.saturating_add(width).min(len);
        let (mut left, mut right, mut out) = (start, middle, start);
        while left < middle && right < end {
            if compare(&source[left], &source[right])? == Ordering::Greater {
                target[out] = source[right].clone();
                right += 1;
            } else {
                target[out] = source[left].clone();
                left += 1;
            }
            out += 1;
        }
        let rest = if left < middle {
            left..middle
        } else {
            right..end
        };
        for index in rest {
            target[out] = source[index].clone();
            out += 1;
        }
        start = end;
    }
    Ok(())
}
