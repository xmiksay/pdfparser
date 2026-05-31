//! The main recursive XY-cut segmentation algorithm.

use super::config::{ItemGrouping, SegmentParams, assign_items_to_regions, build_partitioned_zone};
use super::gaps::{
    bbox_of, candidate_col_widths, find_x_gaps, find_y_gaps, max_gap_size, partition_by_x_center,
    partition_by_x_center_refs, partition_by_y_center,
};
use super::rows::{
    col_gaps_from_xs, is_row_aligned, median_band_char_count, median_item_height, try_header_band,
};
use super::table::build_table_zone;
use super::types::{Bounded, SplitDir, Zone};

/// Threshold below which a candidate column is considered "narrow" — at
/// least one column under this fraction of the parent width AND ≥ 3 columns
/// total triggers `Zone::Table` emission instead of plain vertical split.
/// Tuned for TOC-style layouts (number / title / page) where two of three
/// columns are typically very narrow.
const NARROW_COL_FRACTION: f32 = 0.20;

/// Multiplier on the median item height used as the Y-alignment tolerance
/// for the row-aligned 2-column table probe. Mirrors `pdf.rs`'s
/// `TABLE_ROW_Y_TOLERANCE_SCALE`.
const ROW_ALIGNED_TABLE_TOLERANCE_SCALE: f32 = 0.6;

/// Largest per-side median character count for which the row-aligned 2-column
/// probe still promotes the layout to a `Zone::Table`. Above this, the
/// columns almost certainly carry continuous prose (e.g. a glossary or a
/// newspaper-style body text split across two columns) rather than tabular
/// data, so we fall back to a regular vertical split.
const ROW_ALIGNED_MAX_MEDIAN_CHARS: usize = 60;

/// Minimum ratio of the wider column's median character count to the
/// narrower column's, for the row-aligned 2-column probe to still promote
/// the layout. Captures the structural asymmetry of real tabular layouts
/// (TOC title + page#, lookup-table key + value, etc.) and rejects 2-column
/// body prose where both sides carry similar per-row content even though
/// individual text-runs are short.
const ROW_ALIGNED_ASYMMETRY_RATIO: usize = 3;

/// Per-side median character count below which the row-aligned 2-column
/// probe always promotes — very short content on a side (1–2 chars, single
/// letters, single digits) is essentially never body prose, so the
/// asymmetry requirement is waived.
const ROW_ALIGNED_NARROW_MEDIAN_CHARS: usize = 10;

/// Recursively segment `items` into a [`Zone`] tree using largest-gap-first XY-cut.
pub fn segment<T: Bounded>(items: Vec<T>, p: &SegmentParams) -> Zone<T> {
    let bbox = bbox_of(&items);

    if items.len() < p.min_zone_items {
        return Zone::Leaf { bbox, items };
    }

    // Path-based table-region handling. Each item is assigned to the
    // table-border region (if any) whose interior contains its
    // y_center (or `None` for items outside every region). Items that
    // land in a region are force-promoted to a `Zone::Table`; the
    // remainder fall through to ordinary heuristic segmentation.
    if !p.table_regions.is_empty() {
        match assign_items_to_regions(&items, &p.table_regions) {
            ItemGrouping::SingleRegion(r_idx) => {
                let region = &p.table_regions[r_idx];
                let drawn_gaps = col_gaps_from_xs(&region.col_xs);
                let v_gaps = if !drawn_gaps.is_empty() {
                    drawn_gaps
                } else {
                    find_x_gaps(&items, p.min_v_gap)
                };
                tracing::debug!(
                    r_idx,
                    col_xs_count = region.col_xs.len(),
                    row_ys_count = region.row_ys.len(),
                    v_gaps_count = v_gaps.len(),
                    "SingleRegion force-promote"
                );
                if !v_gaps.is_empty() {
                    return build_table_zone(items, &v_gaps, bbox, p, &region.row_ys);
                }
                // Single column inside the region — no table to build,
                // fall through.
            }
            ItemGrouping::Partitioned(assignment) => {
                return build_partitioned_zone(items, assignment, p, bbox);
            }
            ItemGrouping::None => {}
        }
    }

    let v_gaps = find_x_gaps(&items, p.min_v_gap);
    let h_gaps = find_y_gaps(&items, p.min_h_gap);

    let v_max = max_gap_size(&v_gaps);
    let h_max = max_gap_size(&h_gaps);

    // Header-band probe: a page-spanning title/subtitle row can horizontally
    // overlap a column gutter that would otherwise cleanly split the body
    // into two columns. If lifting the topmost contiguous Y-band of items
    // exposes a wider X-gap below than the global one, do a horizontal split
    // first so the body's column structure is preserved.
    if let Some(header_indices) = try_header_band(&items, &bbox, v_max, p) {
        let mut header_items = Vec::new();
        let mut rest_items = Vec::new();
        for (i, item) in items.into_iter().enumerate() {
            if header_indices.contains(&i) {
                header_items.push(item);
            } else {
                rest_items.push(item);
            }
        }
        return Zone::Split {
            dir: SplitDir::Horizontal,
            bbox,
            children: vec![segment(header_items, p), segment(rest_items, p)],
        };
    }

    // Pick the larger gap dimension. Vertical wins ties (preserves column structure).
    if v_max <= 0.0 && h_max <= 0.0 {
        return Zone::Leaf { bbox, items };
    }

    if v_max >= h_max {
        // Narrow-column probe: TOC-style layouts have 3+ columns where at
        // least one is narrower than 20% of the parent — emit as a Table
        // zone instead of column-major content.
        let parent_w = bbox.width();
        let col_widths = candidate_col_widths(&bbox, &v_gaps);
        let want_table = col_widths.len() >= 3
            && parent_w > 0.0
            && col_widths
                .iter()
                .any(|&w| w < parent_w * NARROW_COL_FRACTION);

        if want_table {
            return build_table_zone(items, &v_gaps, bbox, p, &[]);
        }

        // Row-alignment probe: 2-column layouts whose elements share rows
        // (TOC: "title ........ page#"). The narrow-column trigger fails
        // because TOC entries are wide, but Y centers across the gap match.
        // Replicates `TABLE_ROW_Y_TOLERANCE_SCALE = 0.6` from `pdf.rs` —
        // tolerance is 0.6 × median item height.
        if col_widths.len() == 2 {
            let groups_probe = partition_by_x_center_refs(&items, &v_gaps);
            if groups_probe.len() == 2 && groups_probe[0].len() >= 3 && groups_probe[1].len() >= 3 {
                let row_tol = median_item_height(&items)
                    .map(|h| h * ROW_ALIGNED_TABLE_TOLERANCE_SCALE)
                    .unwrap_or(0.0);
                if row_tol > 0.0 && is_row_aligned(&groups_probe[0], &groups_probe[1], row_tol) {
                    // Content guards. Promote only when:
                    //   * neither side's per-row content is too long
                    //     (above MAX_MEDIAN it's almost certainly prose), AND
                    //   * at least one side is very short (≤ NARROW: single
                    //     letters, digits, page numbers) OR the columns are
                    //     asymmetric (max ≥ ASYMMETRY × min).
                    // 2-column body prose has symmetric, moderately-short
                    // per-run content (~30–50 chars from pdfium
                    // fragmentation) and fails both arms — falls through
                    // to a vertical split.
                    // Use per-Y-band totals (not per-item) so pdfium's
                    // word-level fragmentation doesn't mask a row that
                    // actually carries 60+ chars of content.
                    let left_median = median_band_char_count(&groups_probe[0], row_tol);
                    let right_median = median_band_char_count(&groups_probe[1], row_tol);
                    let min_m = left_median.min(right_median);
                    let max_m = left_median.max(right_median);
                    let narrow_side = min_m <= ROW_ALIGNED_NARROW_MEDIAN_CHARS;
                    let asymmetric =
                        min_m == 0 || max_m >= min_m.saturating_mul(ROW_ALIGNED_ASYMMETRY_RATIO);
                    if max_m <= ROW_ALIGNED_MAX_MEDIAN_CHARS && (narrow_side || asymmetric) {
                        return build_table_zone(items, &v_gaps, bbox, p, &[]);
                    }
                    // Otherwise fall through to a normal vertical split.
                }
            }
        }

        let groups = partition_by_x_center(items, &v_gaps);
        let children: Vec<Zone<T>> = groups
            .into_iter()
            .filter(|g| !g.is_empty())
            .map(|g| segment(g, p))
            .collect();
        if children.len() < 2 {
            // Degenerate split (everything ended up in one group): fall back to leaf.
            // This shouldn't happen with correct gap detection, but keep safe.
            let items: Vec<T> = children
                .into_iter()
                .flat_map(|c| match c {
                    Zone::Leaf { items, .. } => items,
                    _ => Vec::new(),
                })
                .collect();
            return Zone::Leaf { bbox, items };
        }
        Zone::Split {
            dir: SplitDir::Vertical,
            bbox,
            children,
        }
    } else {
        let groups = partition_by_y_center(items, &h_gaps);
        let children: Vec<Zone<T>> = groups
            .into_iter()
            .filter(|g| !g.is_empty())
            .map(|g| segment(g, p))
            .collect();
        if children.len() < 2 {
            let items: Vec<T> = children
                .into_iter()
                .flat_map(|c| match c {
                    Zone::Leaf { items, .. } => items,
                    _ => Vec::new(),
                })
                .collect();
            return Zone::Leaf { bbox, items };
        }
        Zone::Split {
            dir: SplitDir::Horizontal,
            bbox,
            children,
        }
    }
}

#[cfg(test)]
#[path = "segment_tests.rs"]
mod tests;
