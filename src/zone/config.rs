//! Segmentation parameters and table-region (drawn-border) handling.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::super::borders::TableRegion;
use super::gaps::{bbox_of, find_x_gaps};
use super::rows::col_gaps_from_xs;
use super::segment::segment;
use super::table::build_table_zone;
use super::types::{BBox, Bounded, SplitDir, Zone};

#[derive(Debug, Clone)]
pub struct SegmentParams {
    /// Minimum width of a vertical (column) gutter, in input units (typically pts).
    pub min_v_gap: f32,
    /// Minimum height of a horizontal (row) gutter, in input units.
    pub min_h_gap: f32,
    /// Stop recursing if a candidate sub-zone has fewer items than this.
    pub min_zone_items: usize,
    /// Page regions known a-priori to enclose a table border (typically
    /// derived from PDF path operators — drawn rectangles or grids).
    /// Items entirely within one of these regions skip the heuristic
    /// gap-based segmentation and are emitted as a `Zone::Table`. Each
    /// region carries the drawn row-boundary Y positions so the table
    /// builder can use them directly as authoritative row anchors.
    pub table_regions: Vec<TableRegion>,
}

impl Default for SegmentParams {
    fn default() -> Self {
        Self {
            min_v_gap: 12.0,
            min_h_gap: 8.0,
            min_zone_items: 2,
            table_regions: Vec::new(),
        }
    }
}

/// Per-item region assignment result returned by
/// [`assign_items_to_regions`]. Drives how `segment` partitions the
/// input before recursing.
pub(super) enum ItemGrouping {
    /// No item's center sits inside any table region.
    None,
    /// Every item's center sits inside the same single region. The
    /// inner usize is that region's index in `SegmentParams::table_regions`.
    SingleRegion(usize),
    /// Items span multiple regions and/or include items outside every
    /// region. The inner vec is the per-item region index (or `None`
    /// for "outside every region"), aligned with the original `items`
    /// slice.
    Partitioned(Vec<Option<usize>>),
}

/// Classify each item by which table region (if any) contains its
/// `y_center`. Center-based containment is intentionally lenient: a
/// multi-line cell whose text wraps slightly outside the drawn border
/// still belongs to the cell.
pub(super) fn assign_items_to_regions<T: Bounded>(
    items: &[T],
    regions: &[TableRegion],
) -> ItemGrouping {
    let assignment: Vec<Option<usize>> = items
        .iter()
        .map(|it| {
            let c = it.bbox();
            let cx = c.x_center();
            let cy = c.y_center();
            regions.iter().position(|r| {
                cx >= r.bbox.left && cx <= r.bbox.right && cy >= r.bbox.bottom && cy <= r.bbox.top
            })
        })
        .collect();

    let mut first: Option<usize> = None;
    let mut multi = false;
    let mut has_outside = false;
    for a in &assignment {
        match a {
            None => has_outside = true,
            Some(idx) => match first {
                None => first = Some(*idx),
                Some(f) if f != *idx => {
                    multi = true;
                    break;
                }
                _ => {}
            },
        }
    }

    match (first, multi, has_outside) {
        (None, _, _) => ItemGrouping::None,
        (Some(idx), false, false) => ItemGrouping::SingleRegion(idx),
        _ => ItemGrouping::Partitioned(assignment),
    }
}

/// Build a `Zone::Split { Horizontal }` from a region-partitioned item
/// list. Each region's items become a `Zone::Table`; outside items are
/// fragmented into Y-slots relative to the regions (items above all
/// regions, items between adjacent regions, items below all regions)
/// so the rendered markdown preserves the page's reading order around
/// tables — instead of one giant outside-zone whose mid-page center
/// would jumble title (top) and notes (bottom) together.
pub(super) fn build_partitioned_zone<T: Bounded>(
    items: Vec<T>,
    assignment: Vec<Option<usize>>,
    p: &SegmentParams,
    parent_bbox: BBox,
) -> Zone<T> {
    // Order region indices by top desc so slot 0 is "above the topmost
    // region", slot k+1 is "between region k and the next one down",
    // and slot regions.len() is "below the bottommost region".
    let mut region_order: Vec<usize> = (0..p.table_regions.len()).collect();
    region_order.sort_by(|a, b| {
        p.table_regions[*b]
            .bbox
            .top
            .partial_cmp(&p.table_regions[*a].bbox.top)
            .unwrap_or(Ordering::Equal)
    });
    let region_count = region_order.len();

    // Distribute items by their assignment. Inside-region items go to
    // a per-region bucket (keyed by region index). Outside items go
    // into the Y-slot they belong to.
    let mut region_buckets: HashMap<usize, Vec<T>> = HashMap::new();
    let mut slot_buckets: Vec<Vec<T>> = (0..=region_count).map(|_| Vec::new()).collect();
    for (it, key) in items.into_iter().zip(assignment) {
        match key {
            Some(r) => region_buckets.entry(r).or_default().push(it),
            None => {
                let yc = it.bbox().y_center();
                // Slot index = number of regions whose `top` is strictly
                // greater than this item's center. Items above all
                // regions get slot 0; items below all regions get
                // slot `region_count`.
                let mut slot = 0usize;
                for &r_idx in &region_order {
                    if p.table_regions[r_idx].bbox.top > yc {
                        slot += 1;
                    } else {
                        break;
                    }
                }
                slot_buckets[slot].push(it);
            }
        }
    }

    // Interleave slot-zones with region-zones in reading order.
    let mut children: Vec<Zone<T>> = Vec::with_capacity(region_count * 2 + 1);
    let push_slot = |slot_items: Vec<T>, children: &mut Vec<Zone<T>>| {
        if slot_items.is_empty() {
            return;
        }
        children.push(segment(slot_items, p));
    };
    let push_region = |r_idx: usize, region_items: Option<Vec<T>>, children: &mut Vec<Zone<T>>| {
        if let Some(bucket_items) = region_items {
            if bucket_items.is_empty() {
                return;
            }
            let bucket_bbox = bbox_of(&bucket_items);
            let v_gaps = find_x_gaps(&bucket_items, p.min_v_gap);
            let region = &p.table_regions[r_idx];
            let drawn_gaps = col_gaps_from_xs(&region.col_xs);
            tracing::debug!(
                r_idx,
                col_xs_count = region.col_xs.len(),
                drawn_gap_count = drawn_gaps.len(),
                heuristic_gap_count = v_gaps.len(),
                "push_region gap source"
            );
            let effective_gaps = if !drawn_gaps.is_empty() {
                drawn_gaps
            } else {
                v_gaps
            };
            let zone = if effective_gaps.is_empty() {
                Zone::Leaf {
                    bbox: bucket_bbox,
                    items: bucket_items,
                }
            } else {
                build_table_zone(
                    bucket_items,
                    &effective_gaps,
                    bucket_bbox,
                    p,
                    &region.row_ys,
                )
            };
            children.push(zone);
        }
    };

    // Slot 0 (above the topmost region), then region 0, then slot 1, …
    push_slot(std::mem::take(&mut slot_buckets[0]), &mut children);
    for (i, &r_idx) in region_order.iter().enumerate() {
        push_region(r_idx, region_buckets.remove(&r_idx), &mut children);
        push_slot(std::mem::take(&mut slot_buckets[i + 1]), &mut children);
    }

    if children.len() == 1 {
        return children.into_iter().next().expect("just checked len");
    }

    Zone::Split {
        dir: SplitDir::Horizontal,
        bbox: parent_bbox,
        children,
    }
}
