//! `Zone::Table` construction from a flat item list and X-gap structure.

use super::config::SegmentParams;
use super::gaps::{bbox_of, bucket_by_column, partition_by_x_center};
use super::rows::{group_rows_by_anchor, row_groups_by_drawn_anchors, row_groups_by_y_gap};
use super::segment::segment;
use super::types::{BBox, Bounded, SplitDir, Zone};

/// Build a `Zone::Table` from a flat item list and the X-gap structure.
/// Items are grouped into rows by Y proximity, then each row is partitioned
/// into cells by `partition_by_x_center` with `v_gaps`. Empty cells are
/// padded so column count is uniform; rows with no content are dropped.
///
/// Falls back gracefully if the layout can't yield ≥ 2 rows: returns a
/// regular `Zone::Split { Vertical }` (or `Zone::Leaf` if the split is
/// degenerate).
pub(super) fn build_table_zone<T: Bounded>(
    items: Vec<T>,
    v_gaps: &[(f32, f32)],
    parent_bbox: BBox,
    p: &SegmentParams,
    forced_row_ys: &[f32],
) -> Zone<T> {
    if items.is_empty() {
        return Zone::Leaf {
            bbox: parent_bbox,
            items,
        };
    }

    // Recursive `segment()` calls in our fallback paths below operate on
    // subsets of `items` that — by construction — still sit inside the
    // same drawn `TableRegion`. Re-running region-aware segmentation on
    // those subsets re-applies the region's drawn column gaps, treats
    // most cells as empty, fails the fill-rate guard, and recurses
    // forever (stack overflow on real documents). Strip regions for the
    // fallback so column subsets segment heuristically and terminate.
    let p_no_regions = SegmentParams {
        table_regions: Vec::new(),
        ..p.clone()
    };

    // 1. Determine row groups. Drawn H-line positions (from PDF path
    //    detection) are authoritative when available — they tell us
    //    exactly where the cell boundaries are, regardless of how the
    //    text items wrap. Otherwise fall back to heuristic row
    //    grouping: column-bucket anchor when col counts disagree,
    //    plain Y-gap when counts are uniform.
    let bboxes: Vec<BBox> = items.iter().map(|it| it.bbox()).collect();
    let row_groups: Vec<Vec<usize>> = if forced_row_ys.len() >= 2 {
        row_groups_by_drawn_anchors(&bboxes, forced_row_ys)
    } else {
        let col_buckets = bucket_by_column(&bboxes, v_gaps);
        let nonzero: Vec<usize> = col_buckets
            .iter()
            .map(|b| b.len())
            .filter(|&c| c > 0)
            .collect();
        let uniform_counts = nonzero.len() < 2 || nonzero.iter().all(|&c| c == nonzero[0]);
        if uniform_counts {
            row_groups_by_y_gap(&bboxes, p)
        } else {
            group_rows_by_anchor(&bboxes, &col_buckets)
                .unwrap_or_else(|| row_groups_by_y_gap(&bboxes, p))
        }
    };

    // 3. Need ≥ 2 rows for a real table — otherwise fall back to a regular
    //    vertical split (or leaf if degenerate).
    if row_groups.len() < 2 {
        let groups = partition_by_x_center(items, v_gaps);
        let children: Vec<Zone<T>> = groups
            .into_iter()
            .filter(|g| !g.is_empty())
            .map(|g| segment(g, &p_no_regions))
            .collect();
        if children.len() < 2 {
            let merged: Vec<T> = children
                .into_iter()
                .flat_map(|c| match c {
                    Zone::Leaf { items, .. } => items,
                    _ => Vec::new(),
                })
                .collect();
            return Zone::Leaf {
                bbox: parent_bbox,
                items: merged,
            };
        }
        return Zone::Split {
            dir: SplitDir::Vertical,
            bbox: parent_bbox,
            children,
        };
    }

    // 4. Distribute items into per-row vectors.
    let mut item_row: Vec<Option<usize>> = vec![None; items.len()];
    for (row_i, row) in row_groups.iter().enumerate() {
        for &idx in row {
            item_row[idx] = Some(row_i);
        }
    }
    let mut rows_items: Vec<Vec<T>> = (0..row_groups.len()).map(|_| Vec::new()).collect();
    for (i, item) in items.into_iter().enumerate() {
        if let Some(r) = item_row[i] {
            rows_items[r].push(item);
        }
    }

    // 5. Per row, partition into cells; pad empty cells; drop fully-empty rows.
    let mut rows: Vec<Vec<Zone<T>>> = Vec::with_capacity(rows_items.len());
    for row_items in rows_items {
        let cells = partition_by_x_center(row_items, v_gaps);
        let mut row: Vec<Zone<T>> = Vec::with_capacity(cells.len());
        let mut row_has_content = false;
        for cell_items in cells {
            if cell_items.is_empty() {
                row.push(Zone::Leaf {
                    bbox: BBox::empty(),
                    items: Vec::new(),
                });
            } else {
                row_has_content = true;
                let bbox = bbox_of(&cell_items);
                row.push(Zone::Leaf {
                    bbox,
                    items: cell_items,
                });
            }
        }
        if row_has_content {
            rows.push(row);
        }
    }

    if rows.len() < 2 {
        // Collapsed below the row threshold after dropping empties — return a
        // single leaf with all items merged.
        let merged: Vec<T> = rows
            .into_iter()
            .flat_map(|r| {
                r.into_iter().flat_map(|c| match c {
                    Zone::Leaf { items, .. } => items,
                    _ => Vec::new(),
                })
            })
            .collect();
        return Zone::Leaf {
            bbox: parent_bbox,
            items: merged,
        };
    }

    // Sanity: real tabular data has most rows fully populated. If fewer than
    // 60% of rows have every cell filled, this is likely a false positive
    // (e.g. centered banner above a 2-col layout fakes a 3-col grid).
    // Fall back to a regular vertical split.
    let n_cols = rows[0].len();
    tracing::debug!(
        v_gaps_len = v_gaps.len(),
        forced_row_ys_len = forced_row_ys.len(),
        rows_len = rows.len(),
        n_cols,
        "build_table_zone shape"
    );
    let is_filled = |c: &Zone<T>| -> bool {
        match c {
            Zone::Leaf { items, .. } => !items.is_empty(),
            _ => true,
        }
    };
    let fully_filled_rows = rows
        .iter()
        .filter(|r| r.len() == n_cols && r.iter().all(is_filled))
        .count();
    if fully_filled_rows * 5 < rows.len() * 3 {
        let merged: Vec<T> = rows
            .into_iter()
            .flat_map(|r| {
                r.into_iter().flat_map(|c| match c {
                    Zone::Leaf { items, .. } => items,
                    _ => Vec::new(),
                })
            })
            .collect();
        let groups = partition_by_x_center(merged, v_gaps);
        let children: Vec<Zone<T>> = groups
            .into_iter()
            .filter(|g| !g.is_empty())
            .map(|g| segment(g, &p_no_regions))
            .collect();
        if children.len() < 2 {
            let merged: Vec<T> = children
                .into_iter()
                .flat_map(|c| match c {
                    Zone::Leaf { items, .. } => items,
                    _ => Vec::new(),
                })
                .collect();
            return Zone::Leaf {
                bbox: parent_bbox,
                items: merged,
            };
        }
        return Zone::Split {
            dir: SplitDir::Vertical,
            bbox: parent_bbox,
            children,
        };
    }

    Zone::Table {
        bbox: parent_bbox,
        rows,
    }
}

#[cfg(test)]
#[path = "table_tests.rs"]
mod tests;
