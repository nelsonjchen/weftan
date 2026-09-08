// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// Third-party portions retain their terms; see THIRD_PARTY_NOTICES.md.

//! Go 1.24 `sort.Slice` ordering for recovery-sensitive equal elements.
//!
//! TALA uses the unstable PDQsort implementation behind `sort.Slice` when it
//! orders disconnected subgraphs. Equal-area components can therefore receive
//! a deterministic permutation that Rust's stable slice sort does not produce.
//!
//! Portions are translated from Go 1.24.6's `sort` package. Go source copyright
//! The Go Authors, used under the BSD-3-Clause license. See THIRD_PARTY_NOTICES.md.

#[derive(Clone, Copy, Eq, PartialEq)]
enum Hint {
    Unknown,
    Increasing,
    Decreasing,
}

pub(super) fn sort_by<T, F>(values: &mut [T], less: F)
where
    F: Fn(&T, &T) -> bool + Copy,
{
    if values.len() <= 1 {
        return;
    }
    let limit = usize::BITS as usize - values.len().leading_zeros() as usize;
    pdqsort(values, 0, values.len(), limit, less);
}

fn insertion_sort<T, F>(values: &mut [T], a: usize, b: usize, less: F)
where
    F: Fn(&T, &T) -> bool + Copy,
{
    for i in a + 1..b {
        let mut j = i;
        while j > a && less(&values[j], &values[j - 1]) {
            values.swap(j, j - 1);
            j -= 1;
        }
    }
}

fn sift_down<T, F>(values: &mut [T], mut root: usize, hi: usize, first: usize, less: F)
where
    F: Fn(&T, &T) -> bool + Copy,
{
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            return;
        }
        if child + 1 < hi && less(&values[first + child], &values[first + child + 1]) {
            child += 1;
        }
        if !less(&values[first + root], &values[first + child]) {
            return;
        }
        values.swap(first + root, first + child);
        root = child;
    }
}

fn heap_sort<T, F>(values: &mut [T], a: usize, b: usize, less: F)
where
    F: Fn(&T, &T) -> bool + Copy,
{
    let length = b - a;
    if length == 0 {
        return;
    }
    for i in (0..=(length - 1) / 2).rev() {
        sift_down(values, i, length, a, less);
    }
    for i in (0..length).rev() {
        values.swap(a, a + i);
        sift_down(values, 0, i, a, less);
    }
}

fn pdqsort<T, F>(values: &mut [T], mut a: usize, mut b: usize, mut limit: usize, less: F)
where
    F: Fn(&T, &T) -> bool + Copy,
{
    let mut was_balanced = true;
    let mut was_partitioned = true;
    loop {
        let length = b - a;
        if length <= 12 {
            insertion_sort(values, a, b, less);
            return;
        }
        if limit == 0 {
            heap_sort(values, a, b, less);
            return;
        }
        if !was_balanced {
            break_patterns(values, a, b);
            limit -= 1;
        }
        let (mut pivot, mut hint) = choose_pivot(values, a, b, less);
        if hint == Hint::Decreasing {
            values[a..b].reverse();
            pivot = (b - 1) - (pivot - a);
            hint = Hint::Increasing;
        }
        if was_balanced
            && was_partitioned
            && hint == Hint::Increasing
            && partial_insertion_sort(values, a, b, less)
        {
            return;
        }
        if a > 0 && !less(&values[a - 1], &values[pivot]) {
            a = partition_equal(values, a, b, pivot, less);
            continue;
        }
        let (middle, already_partitioned) = partition(values, a, b, pivot, less);
        was_partitioned = already_partitioned;
        let left_length = middle - a;
        let right_length = b - middle;
        let threshold = length / 8;
        if left_length < right_length {
            was_balanced = left_length >= threshold;
            pdqsort(values, a, middle, limit, less);
            a = middle + 1;
        } else {
            was_balanced = right_length >= threshold;
            pdqsort(values, middle + 1, b, limit, less);
            b = middle;
        }
    }
}

fn partition<T, F>(values: &mut [T], a: usize, b: usize, pivot: usize, less: F) -> (usize, bool)
where
    F: Fn(&T, &T) -> bool + Copy,
{
    values.swap(a, pivot);
    let mut i = a + 1;
    let mut j = b - 1;
    while i <= j && less(&values[i], &values[a]) {
        i += 1;
    }
    while i <= j && !less(&values[j], &values[a]) {
        if j == 0 {
            break;
        }
        j -= 1;
    }
    if i > j {
        values.swap(j, a);
        return (j, true);
    }
    values.swap(i, j);
    i += 1;
    j -= 1;
    loop {
        while i <= j && less(&values[i], &values[a]) {
            i += 1;
        }
        while i <= j && !less(&values[j], &values[a]) {
            if j == 0 {
                break;
            }
            j -= 1;
        }
        if i > j {
            break;
        }
        values.swap(i, j);
        i += 1;
        j -= 1;
    }
    values.swap(j, a);
    (j, false)
}

fn partition_equal<T, F>(values: &mut [T], a: usize, b: usize, pivot: usize, less: F) -> usize
where
    F: Fn(&T, &T) -> bool + Copy,
{
    values.swap(a, pivot);
    let mut i = a + 1;
    let mut j = b - 1;
    loop {
        while i <= j && !less(&values[a], &values[i]) {
            i += 1;
        }
        while i <= j && less(&values[a], &values[j]) {
            if j == 0 {
                break;
            }
            j -= 1;
        }
        if i > j {
            return i;
        }
        values.swap(i, j);
        i += 1;
        j -= 1;
    }
}

fn partial_insertion_sort<T, F>(values: &mut [T], a: usize, b: usize, less: F) -> bool
where
    F: Fn(&T, &T) -> bool + Copy,
{
    let mut i = a + 1;
    for _ in 0..5 {
        while i < b && !less(&values[i], &values[i - 1]) {
            i += 1;
        }
        if i == b {
            return true;
        }
        if b - a < 50 {
            return false;
        }
        values.swap(i, i - 1);
        if i - a >= 2 {
            let mut j = i - 1;
            while j >= 1 && less(&values[j], &values[j - 1]) {
                values.swap(j, j - 1);
                j -= 1;
            }
        }
        if b - i >= 2 {
            let mut j = i + 1;
            while j < b && less(&values[j], &values[j - 1]) {
                values.swap(j, j - 1);
                j += 1;
            }
        }
    }
    false
}

fn break_patterns<T>(values: &mut [T], a: usize, b: usize) {
    let length = b - a;
    if length < 8 {
        return;
    }
    let mut random = length as u64;
    let modulus = 1usize << (usize::BITS - length.leading_zeros());
    for index in a + (length / 4) * 2 - 1..=a + (length / 4) * 2 + 1 {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        let mut other = random as usize & (modulus - 1);
        if other >= length {
            other -= length;
        }
        values.swap(index, a + other);
    }
}

fn choose_pivot<T, F>(values: &mut [T], a: usize, b: usize, less: F) -> (usize, Hint)
where
    F: Fn(&T, &T) -> bool + Copy,
{
    let length = b - a;
    let mut swaps = 0;
    let mut i = a + length / 4;
    let mut j = a + length / 4 * 2;
    let mut k = a + length / 4 * 3;
    if length >= 8 {
        if length >= 50 {
            i = median_adjacent(values, i, &mut swaps, less);
            j = median_adjacent(values, j, &mut swaps, less);
            k = median_adjacent(values, k, &mut swaps, less);
        }
        j = median(values, i, j, k, &mut swaps, less);
    }
    let hint = match swaps {
        0 => Hint::Increasing,
        12 => Hint::Decreasing,
        _ => Hint::Unknown,
    };
    (j, hint)
}

fn order_two<T, F>(values: &[T], a: usize, b: usize, swaps: &mut usize, less: F) -> (usize, usize)
where
    F: Fn(&T, &T) -> bool + Copy,
{
    if less(&values[b], &values[a]) {
        *swaps += 1;
        (b, a)
    } else {
        (a, b)
    }
}

fn median<T, F>(values: &[T], a: usize, b: usize, c: usize, swaps: &mut usize, less: F) -> usize
where
    F: Fn(&T, &T) -> bool + Copy,
{
    let (a, b) = order_two(values, a, b, swaps, less);
    let (b, _c) = order_two(values, b, c, swaps, less);
    let (_a, b) = order_two(values, a, b, swaps, less);
    b
}

fn median_adjacent<T, F>(values: &[T], a: usize, swaps: &mut usize, less: F) -> usize
where
    F: Fn(&T, &T) -> bool + Copy,
{
    median(values, a - 1, a, a + 1, swaps, less)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_equal_runs_remain_in_insertion_order() {
        let mut values = vec![(2, 'a'), (1, 'b'), (1, 'c'), (2, 'd')];
        sort_by(&mut values, |left, right| left.0 < right.0);
        assert_eq!(values, vec![(1, 'b'), (1, 'c'), (2, 'a'), (2, 'd')]);
    }

    #[test]
    fn long_equal_runs_match_go_pdqsort_permutation() {
        let areas = [
            12_000, 12_000, 12_000, 12_000, 12_000, 24_000, 8_000, 16_000, 16_000, 48_800, 69_600,
            6_006, 7_722, 5_808, 6_270, 9_174,
        ];
        let mut values: Vec<_> = areas.into_iter().enumerate().collect();
        sort_by(&mut values, |left, right| left.1 > right.1);
        assert_eq!(
            values.iter().map(|value| value.0).collect::<Vec<_>>(),
            vec![10, 9, 5, 8, 7, 4, 0, 3, 2, 1, 15, 6, 12, 14, 11, 13]
        );
    }

    #[test]
    fn unbalanced_equal_partition_matches_go_1_24_pattern_breaking() {
        let areas = [
            10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000,
            6_864, 10_000, 10_000,
        ];
        let mut values: Vec<_> = areas.into_iter().enumerate().collect();
        sort_by(&mut values, |left, right| left.1 > right.1);
        assert_eq!(
            values.iter().map(|value| value.0).collect::<Vec<_>>(),
            vec![6, 12, 2, 3, 4, 5, 8, 0, 1, 9, 10, 13, 7, 11]
        );
    }
}
