//! Row-grouping and row-alignment heuristics used by table promotion and
//! the header-band probe.

use std::cmp::Ordering;
use std::collections::HashSet;

use super::config::SegmentParams;
use super::gaps::median_height_of;
use super::types::{BBox, Bounded};

/// Maximum fraction of the parent bbox height a candidate header band may
/// occupy. A taller "band" likely isn't a banner but a real content region.
const HEADER_BAND_MAX_HEIGHT_FRACTION: f32 = 0.25;

/// Fraction of left-side elements that must have a row-aligned right-side
/// partner before the layout is treated as a 2-column table.
const ROW_ALIGNED_MIN_FRACTION: f32 = 0.60;

/// Smallest column-side count for which row alignment is even attempted —
/// below this either column is too small to robustly call as a "row table".
const ROW_ALIGNED_MIN_ITEMS: usize = 3;

/// Median bbox height across `items`, or `None` for an empty slice.
pub(super) fn median_item_height<T: Bounded>(items: &[T]) -> Option<f32> {
    if items.is_empty() {
        return None;
    }
    let mut hs: Vec<f32> = items.iter().map(|it| it.bbox().height()).collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mid = hs.len() / 2;
    Some(if hs.len() % 2 == 0 {
        (hs[mid - 1] + hs[mid]) / 2.0
    } else {
        hs[mid]
    })
}

/// Median character count grouped by Y-band: items whose `y_center` falls
/// within `y_tol` of each other are summed into a single "row" before the
/// median is taken. This gives a per-row median that is robust to pdfium-
/// style word-level text-run fragmentation (where each item is a single
/// short word and a naive per-item median would massively under-count).
///
/// Returns `0` for an empty slice.
pub(super) fn median_band_char_count<T: Bounded>(items: &[&T], y_tol: f32) -> usize {
    if items.is_empty() {
        return 0;
    }
    let mut sorted: Vec<&&T> = items.iter().collect();
    sorted.sort_by(|a, b| {
        b.bbox()
            .y_center()
            .partial_cmp(&a.bbox().y_center())
            .unwrap_or(Ordering::Equal)
    });
    let mut bands: Vec<usize> = Vec::new();
    let mut cur_chars: usize = 0;
    let mut cur_y: Option<f32> = None;
    for it in sorted {
        let yc = it.bbox().y_center();
        let new_band = match cur_y {
            Some(prev) => (prev - yc).abs() > y_tol,
            None => false,
        };
        if new_band {
            bands.push(cur_chars);
            cur_chars = 0;
        }
        cur_y = Some(yc);
        cur_chars += it.char_count();
    }
    bands.push(cur_chars);
    bands.sort_unstable();
    let mid = bands.len() / 2;
    if bands.len() % 2 == 0 {
        (bands[mid - 1] + bands[mid]) / 2
    } else {
        bands[mid]
    }
}

/// Decide whether elements on the left and right side of a candidate column
/// gap actually share rows. For each left element we find the right element
/// whose Y-center is closest; the pair counts as aligned if the absolute
/// Y-delta is within `row_tolerance`. Returns `true` when at least 60 % of
/// left elements have an aligned partner.
///
/// Skips (returns `false`) when either side has fewer than `ROW_ALIGNED_MIN_ITEMS`
/// elements — too few to call.
pub(super) fn is_row_aligned<T: Bounded>(left: &[&T], right: &[&T], row_tolerance: f32) -> bool {
    if left.len() < ROW_ALIGNED_MIN_ITEMS || right.len() < ROW_ALIGNED_MIN_ITEMS {
        return false;
    }
    if row_tolerance <= 0.0 {
        return false;
    }
    let right_centers: Vec<f32> = right.iter().map(|r| r.bbox().y_center()).collect();
    let mut aligned = 0usize;
    for l in left {
        let ly = l.bbox().y_center();
        let mut best = f32::INFINITY;
        for &ry in &right_centers {
            let d = (ly - ry).abs();
            if d < best {
                best = d;
            }
        }
        if best <= row_tolerance {
            aligned += 1;
        }
    }
    (aligned as f32) >= (left.len() as f32) * ROW_ALIGNED_MIN_FRACTION
}

/// Greedy top-down accumulation of items into a "header band". Returns the
/// indices that form the band iff lifting them exposes a wider X-gap in the
/// remainder than the parent's current `v_max`. Returns `None` if no useful
/// band is found.
pub(super) fn try_header_band<T: Bounded>(
    items: &[T],
    page_bbox: &BBox,
    current_v_max: f32,
    p: &SegmentParams,
) -> Option<HashSet<usize>> {
    if items.len() < 3 {
        return None;
    }

    let mut indexed: Vec<(usize, BBox)> = items
        .iter()
        .enumerate()
        .map(|(i, it)| (i, it.bbox()))
        .collect();
    indexed.sort_by(|a, b| b.1.top.partial_cmp(&a.1.top).unwrap_or(Ordering::Equal));

    let first = &indexed[0].1;
    let mut band_bottom = first.bottom;
    let mut band_top = first.top;
    let mut header_size = 1usize;
    for (_, bb) in &indexed[1..] {
        if band_bottom - bb.top > p.min_h_gap {
            break;
        }
        if bb.bottom < band_bottom {
            band_bottom = bb.bottom;
        }
        if bb.top > band_top {
            band_top = bb.top;
        }
        header_size += 1;
    }

    if header_size == 0 || header_size >= items.len() {
        return None;
    }
    let header_height = (band_top - band_bottom).max(0.0);
    if header_height >= page_bbox.height() * HEADER_BAND_MAX_HEIGHT_FRACTION {
        return None;
    }

    let header_indices: HashSet<usize> = indexed[..header_size].iter().map(|(i, _)| *i).collect();

    // Inline X-gap detection on the rest, without consuming `items`.
    let mut intervals: Vec<(f32, f32)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            if header_indices.contains(&i) {
                None
            } else {
                let b = it.bbox();
                Some((b.left, b.right))
            }
        })
        .collect();
    if intervals.len() < 2 {
        return None;
    }
    intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
    let mut rest_v_max: f32 = 0.0;
    let mut cur_right = intervals[0].1;
    for &(l, r) in &intervals[1..] {
        if l > cur_right + p.min_v_gap {
            let gap = l - cur_right;
            if gap > rest_v_max {
                rest_v_max = gap;
            }
        }
        if r > cur_right {
            cur_right = r;
        }
    }

    if rest_v_max > current_v_max && rest_v_max > 0.0 {
        Some(header_indices)
    } else {
        None
    }
}

/// Y-gap-based row grouping: sort all items top-down and split on
/// `min_h_gap`. Used when column item counts are uniform — the simple,
/// fast case that does not need anchor reasoning.
pub(super) fn row_groups_by_y_gap(bboxes: &[BBox], p: &SegmentParams) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..bboxes.len()).collect();
    order.sort_by(|a, b| {
        bboxes[*b]
            .y_center()
            .partial_cmp(&bboxes[*a].y_center())
            .unwrap_or(Ordering::Equal)
    });
    let mut row_groups: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut current_bottom = f32::INFINITY;
    for &idx in &order {
        let b = bboxes[idx];
        let row_break = !current.is_empty() && current_bottom - b.top > p.min_h_gap;
        if row_break {
            row_groups.push(std::mem::take(&mut current));
            current_bottom = b.bottom;
        } else if current.is_empty() || b.bottom < current_bottom {
            current_bottom = b.bottom;
        }
        current.push(idx);
    }
    if !current.is_empty() {
        row_groups.push(current);
    }
    row_groups
}

/// Build a `partition_by_x_center`-compatible gap list from a sorted
/// list of drawn V-line X positions. The leftmost and rightmost V-lines
/// are the outer edges of the table; the inner V-lines are the inter-
/// column boundaries (`N` V-lines → `N-1` columns → `N-2` inner gaps).
/// Each returned tuple is `(boundary_x, boundary_x)` because
/// `partition_by_x_center` only consults `.0` (the upper bound of the
/// current column).
pub(super) fn col_gaps_from_xs(col_xs: &[f32]) -> Vec<(f32, f32)> {
    if col_xs.len() < 3 {
        return Vec::new();
    }
    col_xs[1..col_xs.len() - 1]
        .iter()
        .map(|&x| (x, x))
        .collect()
}

/// Row grouping driven by drawn H-line Y positions (from PDF path
/// detection). `anchors` is sorted top-down (largest Y first); row `r`
/// owns items whose `y_center` is strictly less than `anchors[r]` and
/// strictly greater than `anchors[r+1]`. Items above the topmost
/// anchor and below the bottommost anchor are clamped to the first
/// and last row respectively.
pub(super) fn row_groups_by_drawn_anchors(bboxes: &[BBox], anchors: &[f32]) -> Vec<Vec<usize>> {
    let n_rows = anchors.len() - 1;
    let mut row_groups: Vec<Vec<usize>> = (0..n_rows).map(|_| Vec::new()).collect();
    for (i, b) in bboxes.iter().enumerate() {
        let yc = b.y_center();
        // Smallest r such that yc > anchors[r+1]. anchors is sorted
        // desc, so anchors[r+1] decreases as r increases.
        let mut row = n_rows - 1;
        for r in 0..n_rows {
            if yc > anchors[r + 1] {
                row = r;
                break;
            }
        }
        row_groups[row].push(i);
    }
    row_groups
}

/// Row grouping that uses the column whose item count matches the modal
/// (most-common) count across non-empty columns as the row anchor. Other
/// columns' items (potentially with extra wrapped lines, or missing some
/// rows) are assigned to whichever anchor row their `y_center` falls
/// within.
///
/// Returns `None` when the anchor would not yield a usable row template
/// (< 2 non-empty columns; anchor column too short; anchors poorly
/// ordered). Caller falls back to [`row_groups_by_y_gap`].
pub(super) fn group_rows_by_anchor(
    bboxes: &[BBox],
    col_buckets: &[Vec<usize>],
) -> Option<Vec<Vec<usize>>> {
    let nonempty: Vec<usize> = (0..col_buckets.len())
        .filter(|&i| !col_buckets[i].is_empty())
        .collect();
    if nonempty.len() < 2 {
        return None;
    }

    // Pick anchor by modal column count: the count shared by the most
    // columns is the "true" row count. Within that count, tiebreak by
    // largest median item height (header-like cells tend to be slightly
    // taller than wrapped body lines), then lowest column index.
    let mut counts: Vec<usize> = nonempty.iter().map(|&i| col_buckets[i].len()).collect();
    counts.sort();
    let mut target_count = counts[0];
    let mut target_freq = 1usize;
    let mut cur_count = counts[0];
    let mut cur_freq = 1usize;
    for &c in &counts[1..] {
        if c == cur_count {
            cur_freq += 1;
        } else {
            if cur_freq > target_freq || (cur_freq == target_freq && cur_count > target_count) {
                target_freq = cur_freq;
                target_count = cur_count;
            }
            cur_count = c;
            cur_freq = 1;
        }
    }
    if cur_freq > target_freq || (cur_freq == target_freq && cur_count > target_count) {
        target_count = cur_count;
    }
    if target_count < 2 {
        return None;
    }
    let mut tied: Vec<usize> = nonempty
        .iter()
        .copied()
        .filter(|&i| col_buckets[i].len() == target_count)
        .collect();
    tied.sort_by(|&a, &b| {
        let ma = median_height_of(bboxes, &col_buckets[a]);
        let mb = median_height_of(bboxes, &col_buckets[b]);
        mb.partial_cmp(&ma)
            .unwrap_or(Ordering::Equal)
            .then(a.cmp(&b))
    });
    let anchor_col = tied[0];

    // Sort the anchor column top-down by bbox.top.
    let mut anchor: Vec<usize> = col_buckets[anchor_col].clone();
    anchor.sort_by(|&a, &b| {
        bboxes[b]
            .top
            .partial_cmp(&bboxes[a].top)
            .unwrap_or(Ordering::Equal)
    });

    // Well-ordered guard: every anchor's y_center must sit above the next
    // anchor's top edge. Failure means anchor cells overlap or are out of
    // order — bail and let the y-gap fallback handle it.
    for r in 0..anchor.len() - 1 {
        if bboxes[anchor[r]].y_center() <= bboxes[anchor[r + 1]].top {
            return None;
        }
    }

    // Row boundaries: row r owns y_center >= anchor[r+1].top (and below
    // the previous boundary). Last row catches everything below the final
    // boundary.
    let n_rows = anchor.len();
    let boundaries: Vec<f32> = (0..n_rows - 1).map(|r| bboxes[anchor[r + 1]].top).collect();

    let mut row_groups: Vec<Vec<usize>> = (0..n_rows).map(|_| Vec::new()).collect();
    for (i, bbox) in bboxes.iter().enumerate() {
        let yc = bbox.y_center();
        let mut row = n_rows - 1;
        for (r, &boundary) in boundaries.iter().enumerate() {
            if yc >= boundary {
                row = r;
                break;
            }
        }
        row_groups[row].push(i);
    }

    Some(row_groups)
}
