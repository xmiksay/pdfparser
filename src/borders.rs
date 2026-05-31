//! PDF path-based table-border detection.
//!
//! pdfium emits drawn rectangles, horizontal lines, and vertical lines as
//! `PdfPageObject::Path` objects. Real tables typically have explicit
//! borders (cell rectangles, separator lines) drawn this way; 2-column
//! body prose does not. By extracting horizontal and vertical line
//! segments and clustering them into rectangular regions, we get a strong
//! "this is a table" signal that the heuristic text-geometry segmenter in
//! [`super::zone`] can use to override its column-count / asymmetry rules.

use std::cmp::Ordering;

use super::zone::BBox;

/// Per-cluster accumulator keyed by union-find root: merged bbox, horizontal
/// line count, vertical line count, horizontal-line Y positions, and
/// vertical-line X positions.
type ColumnAcc = std::collections::HashMap<usize, (BBox, usize, usize, Vec<f32>, Vec<f32>)>;

/// A horizontal line segment in PDF coordinates (y-up).
#[derive(Debug, Clone, Copy)]
pub struct HLine {
    pub left: f32,
    pub right: f32,
    pub y: f32,
}

/// A vertical line segment in PDF coordinates (y-up; `top > bottom`).
#[derive(Debug, Clone, Copy)]
pub struct VLine {
    pub x: f32,
    pub top: f32,
    pub bottom: f32,
}

/// Tolerance for treating a small Δy as a pure horizontal line, or a
/// small Δx as a pure vertical line. PDFs occasionally emit nominally-
/// straight rules with ≤1 pt of skew; anything tighter risks rejecting
/// legitimate borders.
const LINE_AXIS_TOLERANCE: f32 = 2.0;

/// Minimum length for a line segment to be considered (in pts). Filters
/// glyph-stem strokes and other sub-character paths. Also reused by the
/// caller to decide whether a non-axis `LineTo` is a meaningful diagonal
/// (i.e. real drawing geometry) versus negligible.
pub const LINE_MIN_LENGTH: f32 = 4.0;

/// Maximum gap between two line bboxes for them to belong to the same
/// table-region cluster. Cell borders often touch (gap = 0) but
/// occasionally pdfium reports them with tiny offsets; allow a small
/// fudge factor.
const REGION_CLUSTER_GAP: f32 = 4.0;

/// Minimum total line count (H + V combined) for a cluster to be
/// reported as a table region. Filters out incidental rules (footer
/// separators, single underlines) drawn outside tables.
const REGION_MIN_LINES: usize = 4;

/// Minimum number of horizontal AND vertical lines required for a
/// cluster to count as a table region. Catches false positives like
/// stacked underlines (no vertical borders) or a tall divider (no
/// horizontal borders).
const REGION_MIN_PER_AXIS: usize = 2;

/// Minimum number of contributing path bboxes for a cluster to count as a
/// vector figure. Kept at 1 because many real figures are a *single* complex
/// path (e.g. a circle drawn as one many-segment polyline, or a filled symbol),
/// and figure components are often spread too far apart to cluster. The size
/// gate ([`FIGURE_MIN_DIM`] + [`FIGURE_MIN_AREA`]) does the real filtering of
/// decorative strokes, bullet glyphs, and arrowheads.
const FIGURE_MIN_PATHS: usize = 1;

/// Minimum width AND height (pts) of a figure cluster. ~24 pt ≈ 1/3 inch;
/// rejects glyph-sized geometry and single-rule artifacts.
const FIGURE_MIN_DIM: f32 = 24.0;

/// Minimum area (pts²) of a figure cluster. Rejects thin tall/wide strips
/// that pass the per-dimension test (e.g. a 200×3 hairline box).
const FIGURE_MIN_AREA: f32 = 1500.0;

/// Classify a 2-point segment as an H line, V line, or neither.
pub fn classify_segment(x0: f32, y0: f32, x1: f32, y1: f32) -> Option<Line> {
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    if dy <= LINE_AXIS_TOLERANCE && dx >= LINE_MIN_LENGTH {
        Some(Line::H(HLine {
            left: x0.min(x1),
            right: x0.max(x1),
            y: (y0 + y1) / 2.0,
        }))
    } else if dx <= LINE_AXIS_TOLERANCE && dy >= LINE_MIN_LENGTH {
        Some(Line::V(VLine {
            x: (x0 + x1) / 2.0,
            top: y0.max(y1),
            bottom: y0.min(y1),
        }))
    } else {
        None
    }
}

/// One classified line segment. Convenience enum returned by
/// [`classify_segment`].
#[derive(Debug, Clone, Copy)]
pub enum Line {
    H(HLine),
    V(VLine),
}

/// Bounding box wrapper for a line — used by the clustering pass which
/// works generically on bboxes regardless of axis.
fn h_bbox(h: &HLine) -> BBox {
    BBox {
        left: h.left,
        right: h.right,
        top: h.y + LINE_AXIS_TOLERANCE,
        bottom: h.y - LINE_AXIS_TOLERANCE,
    }
}
fn v_bbox(v: &VLine) -> BBox {
    BBox {
        left: v.x - LINE_AXIS_TOLERANCE,
        right: v.x + LINE_AXIS_TOLERANCE,
        top: v.top,
        bottom: v.bottom,
    }
}

fn bboxes_touch(a: &BBox, b: &BBox) -> bool {
    let x_overlap =
        a.left <= b.right + REGION_CLUSTER_GAP && b.left <= a.right + REGION_CLUSTER_GAP;
    let y_overlap =
        a.bottom <= b.top + REGION_CLUSTER_GAP && b.bottom <= a.top + REGION_CLUSTER_GAP;
    x_overlap && y_overlap
}

fn merge_bbox(acc: BBox, other: BBox) -> BBox {
    BBox {
        left: acc.left.min(other.left),
        right: acc.right.max(other.right),
        top: acc.top.max(other.top),
        bottom: acc.bottom.min(other.bottom),
    }
}

/// A path-detected table region: the bounding box of all line segments
/// in the cluster, the unique Y positions of horizontal lines in the
/// cluster (sorted descending, top-first), and the unique X positions
/// of vertical lines (sorted ascending, left-first). Together these
/// are the row and column boundaries pdfium drew, which the downstream
/// `build_table_zone` uses as authoritative anchors when heuristic
/// text-geometry disagrees with the drawn structure (e.g. a narrow
/// annotation column whose gap is too small for `min_v_gap`).
#[derive(Debug, Clone)]
pub struct TableRegion {
    pub bbox: BBox,
    pub row_ys: Vec<f32>,
    pub col_xs: Vec<f32>,
}

/// Cluster horizontal and vertical lines into rectangular table regions.
/// Returns one [`TableRegion`] per cluster meeting the minimum line-
/// count thresholds.
///
/// Lines are grouped by spatial adjacency — two lines whose bboxes touch
/// (within `REGION_CLUSTER_GAP`) belong to the same cluster. A cluster
/// becomes a reported region only when it carries `>= REGION_MIN_LINES`
/// total lines AND `>= REGION_MIN_PER_AXIS` of each axis.
pub fn detect_table_regions(h_lines: &[HLine], v_lines: &[VLine]) -> Vec<TableRegion> {
    let mut entries: Vec<(BBox, bool)> = Vec::with_capacity(h_lines.len() + v_lines.len());
    for h in h_lines {
        entries.push((h_bbox(h), true));
    }
    for v in v_lines {
        entries.push((v_bbox(v), false));
    }
    if entries.is_empty() {
        return Vec::new();
    }

    // Union-find with naive bbox-merge clustering. O(n²) but n is small
    // (a typical PDF page has ≤ a few hundred line segments).
    let n = entries.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] == i {
            i
        } else {
            let r = find(parent, parent[i]);
            parent[i] = r;
            r
        }
    }
    for i in 0..n {
        for j in (i + 1)..n {
            if bboxes_touch(&entries[i].0, &entries[j].0) {
                let ri = find(&mut parent, i);
                let rj = find(&mut parent, j);
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // Aggregate per cluster: bbox merge, H/V line counts, plus the
    // unique Y positions of horizontal lines (row boundaries) and X
    // positions of vertical lines (column boundaries) pdfium drew.
    let mut acc: ColumnAcc = std::collections::HashMap::new();
    let h_count = h_lines.len();
    for (i, (bbox, is_h)) in entries.iter().enumerate() {
        let root = find(&mut parent, i);
        let entry = acc
            .entry(root)
            .or_insert((*bbox, 0, 0, Vec::new(), Vec::new()));
        entry.0 = merge_bbox(entry.0, *bbox);
        if *is_h {
            entry.1 += 1;
            // First `h_count` entries are h_lines in input order; their
            // y is preserved in the original `h_lines` slice.
            entry.3.push(h_lines[i].y);
        } else {
            entry.2 += 1;
            // V-line entries start after the h_lines in `entries`.
            entry.4.push(v_lines[i - h_count].x);
        }
    }

    acc.into_values()
        .filter(|(_, n_h, n_v, _, _)| {
            n_h + n_v >= REGION_MIN_LINES
                && *n_h >= REGION_MIN_PER_AXIS
                && *n_v >= REGION_MIN_PER_AXIS
        })
        .map(|(bbox, _, _, mut ys, mut xs)| {
            // Sort Y descending (top first) and dedupe near-equal
            // values — pdfium emits each H rectangle as TWO horizontal
            // lines (top and bottom of a 0.48-pt thick rect) at almost
            // the same Y; we want one anchor per drawn row.
            ys.sort_by(|a, b| b.partial_cmp(a).unwrap_or(Ordering::Equal));
            let mut dedup_ys: Vec<f32> = Vec::with_capacity(ys.len());
            for y in ys {
                if dedup_ys
                    .last()
                    .is_none_or(|&prev: &f32| (prev - y).abs() > LINE_AXIS_TOLERANCE)
                {
                    dedup_ys.push(y);
                }
            }
            // X ascending (left first), same dedup rule.
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
            let mut dedup_xs: Vec<f32> = Vec::with_capacity(xs.len());
            for x in xs {
                if dedup_xs
                    .last()
                    .is_none_or(|&prev: &f32| (prev - x).abs() > LINE_AXIS_TOLERANCE)
                {
                    dedup_xs.push(x);
                }
            }
            TableRegion {
                bbox,
                row_ys: dedup_ys,
                col_xs: dedup_xs,
            }
        })
        .collect()
}

/// Cluster the bounding boxes of vector-drawing paths into figure regions.
///
/// Callers pass the bboxes of `PdfPageObject::Path` objects that are NOT pure
/// horizontal/vertical rules (those are consumed by [`detect_table_regions`]).
/// Touching bboxes (within `REGION_CLUSTER_GAP`) are union-find clustered, and a
/// cluster is reported only when it carries `>= FIGURE_MIN_PATHS` paths and its
/// merged box clears `FIGURE_MIN_DIM` on each axis and `FIGURE_MIN_AREA` overall
/// — filtering out incidental glyph strokes, bullets, and hairline rectangles.
/// Returns the merged [`BBox`] of each surviving cluster.
pub fn detect_figure_regions(path_bboxes: &[BBox]) -> Vec<BBox> {
    if path_bboxes.is_empty() {
        return Vec::new();
    }

    // Union-find with naive bbox-merge clustering, mirroring
    // `detect_table_regions`. O(n²) but n (paths per page) is small.
    let n = path_bboxes.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] == i {
            i
        } else {
            let r = find(parent, parent[i]);
            parent[i] = r;
            r
        }
    }
    for i in 0..n {
        for j in (i + 1)..n {
            if bboxes_touch(&path_bboxes[i], &path_bboxes[j]) {
                let ri = find(&mut parent, i);
                let rj = find(&mut parent, j);
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // Aggregate per cluster: merged bbox + member count.
    let mut acc: std::collections::HashMap<usize, (BBox, usize)> = std::collections::HashMap::new();
    for (i, bbox) in path_bboxes.iter().enumerate() {
        let root = find(&mut parent, i);
        let entry = acc.entry(root).or_insert((*bbox, 0));
        entry.0 = merge_bbox(entry.0, *bbox);
        entry.1 += 1;
    }

    acc.into_values()
        .filter(|(bbox, count)| {
            let width = bbox.right - bbox.left;
            let height = bbox.top - bbox.bottom;
            *count >= FIGURE_MIN_PATHS
                && width >= FIGURE_MIN_DIM
                && height >= FIGURE_MIN_DIM
                && width * height >= FIGURE_MIN_AREA
        })
        .map(|(bbox, _)| bbox)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(left: f32, right: f32, y: f32) -> HLine {
        HLine { left, right, y }
    }
    fn v(x: f32, bottom: f32, top: f32) -> VLine {
        VLine { x, top, bottom }
    }

    #[test]
    fn classify_horizontal_line() {
        match classify_segment(10.0, 100.0, 200.0, 100.5) {
            Some(Line::H(line)) => {
                assert_eq!(line.left, 10.0);
                assert_eq!(line.right, 200.0);
            }
            other => panic!("expected H line, got {:?}", other),
        }
    }

    #[test]
    fn classify_vertical_line() {
        match classify_segment(50.0, 100.0, 50.5, 200.0) {
            Some(Line::V(line)) => {
                assert_eq!(line.top, 200.0);
                assert_eq!(line.bottom, 100.0);
            }
            other => panic!("expected V line, got {:?}", other),
        }
    }

    #[test]
    fn classify_diagonal_rejected() {
        assert!(classify_segment(0.0, 0.0, 100.0, 100.0).is_none());
    }

    #[test]
    fn classify_too_short_rejected() {
        assert!(classify_segment(0.0, 0.0, 2.0, 0.0).is_none());
    }

    #[test]
    fn detect_simple_rectangle_table() {
        // Four borders forming one cell.
        let h_lines = vec![
            h(0.0, 100.0, 100.0), // top
            h(0.0, 100.0, 50.0),  // bottom
        ];
        let v_lines = vec![
            v(0.0, 50.0, 100.0),   // left
            v(100.0, 50.0, 100.0), // right
        ];
        let regions = detect_table_regions(&h_lines, &v_lines);
        assert_eq!(regions.len(), 1);
        let r = &regions[0];
        assert!((r.bbox.left - 0.0).abs() < 3.0);
        assert!((r.bbox.right - 100.0).abs() < 3.0);
        assert_eq!(r.row_ys.len(), 2, "expected 2 row Y positions");
    }

    #[test]
    fn detect_grid_table() {
        // 3-column × 2-row grid: 3 H lines + 4 V lines.
        let h_lines = vec![
            h(0.0, 300.0, 100.0),
            h(0.0, 300.0, 60.0),
            h(0.0, 300.0, 20.0),
        ];
        let v_lines = vec![
            v(0.0, 20.0, 100.0),
            v(100.0, 20.0, 100.0),
            v(200.0, 20.0, 100.0),
            v(300.0, 20.0, 100.0),
        ];
        let regions = detect_table_regions(&h_lines, &v_lines);
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn no_region_for_isolated_underline() {
        // Single H line — no V lines. Should NOT produce a region.
        let h_lines = vec![h(0.0, 200.0, 50.0)];
        let v_lines: Vec<VLine> = Vec::new();
        let regions = detect_table_regions(&h_lines, &v_lines);
        assert!(regions.is_empty());
    }

    #[test]
    fn no_region_when_below_min_lines() {
        // 1 H + 1 V — 2 lines total, below REGION_MIN_LINES (4).
        let h_lines = vec![h(0.0, 100.0, 50.0)];
        let v_lines = vec![v(50.0, 0.0, 100.0)];
        let regions = detect_table_regions(&h_lines, &v_lines);
        assert!(regions.is_empty());
    }

    #[test]
    fn two_distinct_tables_two_regions() {
        // Two cell-rectangles separated by 50pt of whitespace.
        let h_lines = vec![
            h(0.0, 100.0, 100.0),
            h(0.0, 100.0, 50.0),
            h(200.0, 300.0, 100.0),
            h(200.0, 300.0, 50.0),
        ];
        let v_lines = vec![
            v(0.0, 50.0, 100.0),
            v(100.0, 50.0, 100.0),
            v(200.0, 50.0, 100.0),
            v(300.0, 50.0, 100.0),
        ];
        let regions = detect_table_regions(&h_lines, &v_lines);
        assert_eq!(regions.len(), 2);
    }

    fn bb(left: f32, right: f32, bottom: f32, top: f32) -> BBox {
        BBox {
            left,
            right,
            top,
            bottom,
        }
    }

    #[test]
    fn figure_cluster_merges_touching_boxes() {
        // Four overlapping 2-D path boxes forming one drawing.
        let boxes = vec![
            bb(100.0, 160.0, 100.0, 160.0),
            bb(150.0, 210.0, 120.0, 180.0),
            bb(120.0, 200.0, 150.0, 200.0),
            bb(110.0, 170.0, 90.0, 140.0),
        ];
        let regions = detect_figure_regions(&boxes);
        assert_eq!(regions.len(), 1);
        let r = &regions[0];
        assert!((r.left - 100.0).abs() < 1.0);
        assert!((r.right - 210.0).abs() < 1.0);
        assert!((r.bottom - 90.0).abs() < 1.0);
        assert!((r.top - 200.0).abs() < 1.0);
    }

    #[test]
    fn two_separated_diagrams_two_regions() {
        // Two clusters of 3 boxes each, separated by >REGION_CLUSTER_GAP.
        let boxes = vec![
            bb(0.0, 40.0, 0.0, 40.0),
            bb(20.0, 60.0, 10.0, 50.0),
            bb(10.0, 50.0, 20.0, 60.0),
            bb(300.0, 340.0, 0.0, 40.0),
            bb(320.0, 360.0, 10.0, 50.0),
            bb(310.0, 350.0, 20.0, 60.0),
        ];
        let regions = detect_figure_regions(&boxes);
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn single_large_path_is_a_figure() {
        // A lone but large complex path (e.g. a circle as one polyline) is a
        // figure — the size gate, not a path count, is what qualifies it.
        let boxes = vec![bb(0.0, 100.0, 0.0, 100.0)];
        let regions = detect_figure_regions(&boxes);
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn no_figure_for_single_small_box() {
        // One small box — rejected by the size gate (below FIGURE_MIN_DIM).
        let boxes = vec![bb(0.0, 10.0, 0.0, 10.0)];
        let regions = detect_figure_regions(&boxes);
        assert!(regions.is_empty());
    }

    #[test]
    fn no_figure_for_thin_strip() {
        // Three touching boxes, but the merged cluster is a 200×3 hairline
        // strip — clears FIGURE_MIN_PATHS but fails the height/area gates.
        let boxes = vec![
            bb(0.0, 80.0, 0.0, 3.0),
            bb(60.0, 140.0, 0.0, 3.0),
            bb(120.0, 200.0, 0.0, 3.0),
        ];
        let regions = detect_figure_regions(&boxes);
        assert!(regions.is_empty());
    }
}
