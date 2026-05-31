//! Sweep-line gap detection and item bucketing/partitioning helpers.

use std::cmp::Ordering;

use super::types::{BBox, Bounded};

pub(super) fn bbox_of<T: Bounded>(items: &[T]) -> BBox {
    if items.is_empty() {
        return BBox::empty();
    }
    let mut acc = items[0].bbox();
    for it in &items[1..] {
        acc = acc.merge(it.bbox());
    }
    acc
}

/// Sweep-line gap detection on the X axis. Returns gaps `(start, end)` with
/// `end - start > min_gap`, in increasing-X order.
pub(super) fn find_x_gaps<T: Bounded>(items: &[T], min_gap: f32) -> Vec<(f32, f32)> {
    if items.len() < 2 {
        return Vec::new();
    }
    let mut intervals: Vec<(f32, f32)> = items
        .iter()
        .map(|i| {
            let b = i.bbox();
            (b.left, b.right)
        })
        .collect();
    intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

    let mut gaps = Vec::new();
    let mut cur_right = intervals[0].1;
    for &(l, r) in &intervals[1..] {
        if l > cur_right + min_gap {
            gaps.push((cur_right, l));
        }
        if r > cur_right {
            cur_right = r;
        }
    }
    gaps
}

/// Sweep-line gap detection on the Y axis (PDF y-up). Returns gaps as
/// `(low_y, high_y)` with `high_y - low_y > min_gap`, in increasing-Y order.
pub(super) fn find_y_gaps<T: Bounded>(items: &[T], min_gap: f32) -> Vec<(f32, f32)> {
    if items.len() < 2 {
        return Vec::new();
    }
    let mut intervals: Vec<(f32, f32)> = items
        .iter()
        .map(|i| {
            let b = i.bbox();
            (b.bottom, b.top)
        })
        .collect();
    // Sort by bottom ascending. (Each interval is [bottom, top] with top>=bottom.)
    intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

    let mut gaps = Vec::new();
    let mut cur_top = intervals[0].1;
    for &(b, t) in &intervals[1..] {
        if b > cur_top + min_gap {
            gaps.push((cur_top, b));
        }
        if t > cur_top {
            cur_top = t;
        }
    }
    gaps
}

pub(super) fn max_gap_size(gaps: &[(f32, f32)]) -> f32 {
    gaps.iter().map(|(a, b)| b - a).fold(0.0_f32, f32::max)
}

/// Widths of the N+1 candidate columns implied by N vertical gaps within
/// `parent`. Used to decide whether the layout should be promoted to a
/// `Zone::Table` (TOC heuristic).
pub(super) fn candidate_col_widths(parent: &BBox, v_gaps: &[(f32, f32)]) -> Vec<f32> {
    let mut widths = Vec::with_capacity(v_gaps.len() + 1);
    let mut left = parent.left;
    for &(g_low, g_high) in v_gaps {
        widths.push((g_low - left).max(0.0));
        left = g_high;
    }
    widths.push((parent.right - left).max(0.0));
    widths
}

/// Bucket item indices by column. Mirrors `partition_by_x_center` but works
/// on bboxes (no item ownership needed) and returns indices.
pub(super) fn bucket_by_column(bboxes: &[BBox], v_gaps: &[(f32, f32)]) -> Vec<Vec<usize>> {
    let n_cols = v_gaps.len() + 1;
    let mut buckets: Vec<Vec<usize>> = (0..n_cols).map(|_| Vec::new()).collect();
    for (i, b) in bboxes.iter().enumerate() {
        let cx = b.x_center();
        let mut idx = v_gaps.len();
        for (j, &(g_low, _)) in v_gaps.iter().enumerate() {
            if cx <= g_low {
                idx = j;
                break;
            }
        }
        buckets[idx].push(i);
    }
    buckets
}

/// Median bbox height over `indices`. Returns `0.0` for an empty slice.
pub(super) fn median_height_of(bboxes: &[BBox], indices: &[usize]) -> f32 {
    if indices.is_empty() {
        return 0.0;
    }
    let mut hs: Vec<f32> = indices.iter().map(|&i| bboxes[i].height()).collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mid = hs.len() / 2;
    if hs.len() % 2 == 0 {
        (hs[mid - 1] + hs[mid]) / 2.0
    } else {
        hs[mid]
    }
}

/// Partition by x-center but return references — used by the row-aligned
/// table probe so we don't need to consume `items` before deciding which
/// split to emit.
pub(super) fn partition_by_x_center_refs<'a, T: Bounded>(
    items: &'a [T],
    gaps: &[(f32, f32)],
) -> Vec<Vec<&'a T>> {
    let mut groups: Vec<Vec<&'a T>> = (0..=gaps.len()).map(|_| Vec::new()).collect();
    for item in items {
        let cx = item.bbox().x_center();
        let mut idx = gaps.len();
        for (i, &(g_low, _)) in gaps.iter().enumerate() {
            if cx <= g_low {
                idx = i;
                break;
            }
        }
        groups[idx].push(item);
    }
    groups
}

/// Split `items` into N+1 groups separated by N X-gaps, indexed by `x_center`.
/// Groups are returned in left-to-right order.
pub(super) fn partition_by_x_center<T: Bounded>(items: Vec<T>, gaps: &[(f32, f32)]) -> Vec<Vec<T>> {
    let mut groups: Vec<Vec<T>> = (0..=gaps.len()).map(|_| Vec::new()).collect();
    for item in items {
        let cx = item.bbox().x_center();
        let mut idx = gaps.len();
        for (i, &(g_low, _)) in gaps.iter().enumerate() {
            if cx <= g_low {
                idx = i;
                break;
            }
        }
        groups[idx].push(item);
    }
    groups
}

/// Split `items` into N+1 groups separated by N Y-gaps, indexed by `y_center`.
/// Groups are returned in **top-to-bottom** order (highest Y first).
pub(super) fn partition_by_y_center<T: Bounded>(items: Vec<T>, gaps: &[(f32, f32)]) -> Vec<Vec<T>> {
    let mut groups: Vec<Vec<T>> = (0..=gaps.len()).map(|_| Vec::new()).collect();
    for item in items {
        let cy = item.bbox().y_center();
        let mut idx = gaps.len();
        for (i, &(g_low, _)) in gaps.iter().enumerate() {
            if cy <= g_low {
                idx = i;
                break;
            }
        }
        groups[idx].push(item);
    }
    groups.reverse(); // ascending-Y → descending-Y (top first)
    groups
}
