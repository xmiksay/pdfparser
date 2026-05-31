//! Tests for table promotion: narrow-column / row-aligned probes, fill-rate
//! and content-size guards, and drawn-border (`TableRegion`) handling.

use crate::borders::TableRegion;
use crate::zone::test_support::{b, b_chars, params};
use crate::zone::{BBox, SplitDir, Zone, segment};

#[test]
fn narrow_three_columns_become_table() {
    // TOC-style layout: parent x=0..100. Three columns at x=0..10
    // (number, 10%), x=20..80 (title, 60%), x=90..100 (page#, 10%) —
    // two of three columns sit below the 20% NARROW_COL_FRACTION
    // threshold. Four rows separated by 10pt vertical gaps.
    let mut items = Vec::new();
    for row in 0..4 {
        let y_bottom = (3 - row) as f32 * 20.0 + 30.0;
        let y_top = y_bottom + 10.0;
        // number column
        items.push(b(0.0, y_bottom, 10.0, y_top));
        // title column
        items.push(b(20.0, y_bottom, 80.0, y_top));
        // page# column
        items.push(b(90.0, y_bottom, 100.0, y_top));
    }
    let z = segment(items, &params());
    match z {
        Zone::Table { rows, .. } => {
            assert_eq!(rows.len(), 4, "expected 4 rows");
            for (i, row) in rows.iter().enumerate() {
                assert_eq!(row.len(), 3, "row {i} should have 3 cells");
            }
        }
        other => panic!("expected Zone::Table, got {:?}", other),
    }
}

#[test]
fn two_narrow_columns_with_aligned_rows_become_table() {
    // 2-column layout with perfectly Y-aligned rows. The narrow-column
    // trigger requires 3+ columns, but the row-aligned 2-column probe
    // now promotes this layout to a `Zone::Table` directly.
    // (Originally `two_narrow_columns_still_split_vertical`, kept under
    // a new name with the inverted expectation: 2 columns whose rows
    // share Y centers are tabular data and emitted as such.)
    let mut items = Vec::new();
    for row in 0..3 {
        let y_bottom = (2 - row) as f32 * 20.0 + 30.0;
        let y_top = y_bottom + 10.0;
        items.push(b(0.0, y_bottom, 10.0, y_top));
        items.push(b(90.0, y_bottom, 100.0, y_top));
    }
    let z = segment(items, &params());
    match z {
        Zone::Table { rows, .. } => {
            assert_eq!(rows.len(), 3, "expected 3 rows");
            assert!(rows.iter().all(|r| r.len() == 2));
        }
        other => panic!(
            "expected Zone::Table for row-aligned 2-col layout, got {:?}",
            other
        ),
    }
}

#[test]
fn narrow_three_columns_with_sparse_rows_falls_back() {
    // 3-col-shaped layout where most rows have only one filled cell —
    // typical of a centered page banner above a 2-col table. The
    // fill-rate sanity check inside `build_table_zone` should reject
    // the Table promotion and fall back to a regular vertical split.
    let mut items = Vec::new();
    // Row 0: only the centre cell populated (e.g. a centred header).
    items.push(b(40.0, 90.0, 60.0, 100.0));
    // Rows 1..4: only outer cells populated.
    for row in 0..4 {
        let y_bottom = (3 - row) as f32 * 20.0 + 10.0;
        let y_top = y_bottom + 10.0;
        items.push(b(0.0, y_bottom, 10.0, y_top));
        items.push(b(90.0, y_bottom, 100.0, y_top));
    }
    let z = segment(items, &params());
    // Should NOT be a Zone::Table — fill rate is 0/5.
    if let Zone::Table { .. } = z {
        panic!("expected fall-back, got Zone::Table")
    }
}

#[test]
fn wide_three_columns_still_split_vertical() {
    // 3 columns of equal ~33% width — none below the 20% threshold,
    // so no Table promotion. Regression guard for the existing
    // `three_by_three_grid` shape.
    let mut items = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            let x_left = (col as f32) * 35.0;
            let y_bottom = (2 - row) as f32 * 30.0;
            items.push(b(x_left, y_bottom, x_left + 25.0, y_bottom + 20.0));
        }
    }
    let z = segment(items, &params());
    match z {
        Zone::Split {
            dir: SplitDir::Vertical,
            children,
            ..
        } => {
            assert_eq!(children.len(), 3);
        }
        other => panic!("expected Vertical split (3 cols), got {:?}", other),
    }
}

#[test]
fn toc_two_column_promoted_to_table() {
    // Synthetic TOC: 10 rows of [entry text, page number]. The entries
    // span x=0..200, page numbers at x=350..400 — a ~150 pt horizontal
    // gap between the two columns. Y centers of left and right elements
    // match per row. Neither column is < 20 % of parent width, so the
    // existing 3-column narrow-column trigger does NOT fire. The new
    // row-aligned 2-column probe must promote this to `Zone::Table`.
    let mut items = Vec::new();
    for row in 0..10 {
        let y_bottom = (9 - row) as f32 * 20.0 + 50.0;
        let y_top = y_bottom + 10.0;
        // Entry text column
        items.push(b(0.0, y_bottom, 200.0, y_top));
        // Page-number column — narrow but > 20 % of parent (400 pt total).
        items.push(b(350.0, y_bottom, 400.0, y_top));
    }
    let z = segment(items, &params());
    match z {
        Zone::Table { rows, .. } => {
            assert_eq!(rows.len(), 10, "expected 10 rows in TOC table");
            assert!(rows.iter().all(|r| r.len() == 2), "every row has 2 cells");
        }
        other => panic!("expected Zone::Table for row-aligned TOC, got {:?}", other),
    }
}

#[test]
fn newspaper_two_column_kept_as_columns() {
    // Two columns of independent body text with NO Y alignment across
    // the gap — typical newspaper layout where each column's lines are
    // staggered independently. The row-aligned probe must NOT fire and
    // the layout stays as a regular vertical split.
    let mut items = Vec::new();
    // Left column: lines at y_center = 100, 85, 70, 55, 40 (gap 15)
    for i in 0..5 {
        let y_center = 100.0 - (i as f32) * 15.0;
        let y_bottom = y_center - 5.0;
        let y_top = y_center + 5.0;
        items.push(b(0.0, y_bottom, 180.0, y_top));
    }
    // Right column: lines staggered by ~7 pt so Y centers don't align —
    // 93, 78, 63, 48, 33 (gap 15 again, but shifted by 7).
    for i in 0..5 {
        let y_center = 93.0 - (i as f32) * 15.0;
        let y_bottom = y_center - 5.0;
        let y_top = y_center + 5.0;
        items.push(b(220.0, y_bottom, 400.0, y_top));
    }
    let z = segment(items, &params());
    match z {
        Zone::Split {
            dir: SplitDir::Vertical,
            children,
            ..
        } => {
            assert_eq!(
                children.len(),
                2,
                "expected 2 columns kept as vertical split"
            );
        }
        Zone::Table { .. } => {
            panic!("newspaper-style 2 columns of body text should not be promoted to Table")
        }
        other => panic!("expected Vertical split, got {:?}", other),
    }
}

#[test]
fn two_column_glossary_kept_as_columns() {
    // 10 rows of glossary entries: term on the left, definition on the
    // right, both columns aligned per row (sharing the same baseline
    // grid). The row-alignment probe matches, but each row carries
    // ~150 chars per side — far above ROW_ALIGNED_MAX_MEDIAN_CHARS (60).
    // The content-size guard must reject the table promotion and the
    // layout stays as a regular vertical split (left column then right
    // column in reading order).
    let mut items = Vec::new();
    for row in 0..10 {
        let y_bottom = (9 - row) as f32 * 20.0 + 50.0;
        let y_top = y_bottom + 10.0;
        // Left "term" cell — ~150 chars on the row.
        items.push(b_chars(0.0, y_bottom, 200.0, y_top, 150));
        // Right "definition" cell — also ~150 chars.
        items.push(b_chars(220.0, y_bottom, 420.0, y_top, 150));
    }
    let z = segment(items, &params());
    match z {
        Zone::Split {
            dir: SplitDir::Vertical,
            children,
            ..
        } => {
            assert_eq!(
                children.len(),
                2,
                "expected 2 columns kept as vertical split"
            );
        }
        Zone::Table { .. } => {
            panic!("glossary-style 2 columns of long prose should not be promoted to Table",)
        }
        other => panic!("expected Vertical split, got {:?}", other),
    }
}

#[test]
fn table_region_force_promotes_symmetric_two_column() {
    // 2-column body-prose layout that the heuristic guards would
    // reject (symmetric, no narrow column). But if the PDF border
    // detection reports a table region covering both columns, the
    // force-promote path must kick in and emit a Zone::Table.
    let mut items = Vec::new();
    for row in 0..3 {
        let y_bottom = (2 - row) as f32 * 20.0 + 50.0;
        let y_top = y_bottom + 10.0;
        items.push(b(0.0, y_bottom, 100.0, y_top));
        items.push(b(120.0, y_bottom, 220.0, y_top));
    }
    // Border region containing the items (y-up: top > bottom; the
    // items span y=50..100 so top must be ≥ 100).
    let region = BBox {
        left: -5.0,
        right: 225.0,
        bottom: 45.0,
        top: 105.0,
    };
    let mut p = params();
    p.table_regions = vec![TableRegion {
        bbox: region,
        row_ys: Vec::new(),
        col_xs: Vec::new(),
    }];
    let z = segment(items, &p);
    match z {
        Zone::Table { rows, .. } => {
            assert_eq!(rows.len(), 3);
            assert!(rows.iter().all(|r| r.len() == 2));
        }
        other => panic!(
            "expected Zone::Table inside a force-promote region, got {:?}",
            other
        ),
    }
}

#[test]
fn table_region_does_not_force_outside_items() {
    // Items outside any known table region must NOT be force-promoted.
    // This is a 2-col body layout that should stay as a vertical split.
    let mut items = Vec::new();
    for row in 0..3 {
        let y_bottom = (2 - row) as f32 * 20.0 + 50.0;
        let y_top = y_bottom + 10.0;
        items.push(b(0.0, y_bottom, 100.0, y_top));
        items.push(b(120.0, y_bottom, 220.0, y_top));
    }
    // Region elsewhere on the page — does NOT contain the items.
    let region = BBox {
        left: 500.0,
        right: 700.0,
        bottom: 500.0,
        top: 700.0,
    };
    let mut p = params();
    p.table_regions = vec![TableRegion {
        bbox: region,
        row_ys: Vec::new(),
        col_xs: Vec::new(),
    }];
    let z = segment(items, &p);
    match z {
        Zone::Split {
            dir: SplitDir::Vertical,
            ..
        } => {}
        other => panic!(
            "expected Vertical split when items are outside region, got {:?}",
            other
        ),
    }
}

#[test]
fn table_region_with_drawn_row_anchors_uses_them() {
    // Force-promote a 2-column layout with row_ys from drawn
    // borders. There are 5 H-line positions (4 rows). Items have
    // 4 cells per column but with one wrap text in col 2 row 2 —
    // the row_ys-based anchor must produce 4 rows, not 5, by
    // absorbing the wrap line into row 2's cell.
    let mut items = Vec::new();
    for row in 0..4 {
        let y_bottom = (3 - row) as f32 * 20.0 + 30.0;
        let y_top = y_bottom + 10.0;
        items.push(b(10.0, y_bottom, 90.0, y_top));
        items.push(b(110.0, y_bottom, 190.0, y_top));
        if row == 1 {
            // Wrap line in col 2, between row 1 and row 2 anchors.
            items.push(b(110.0, y_bottom - 8.0, 190.0, y_top - 8.0));
        }
    }
    let region_bbox = BBox {
        left: 0.0,
        right: 200.0,
        bottom: 25.0,
        top: 105.0,
    };
    let row_ys = vec![100.0, 80.0, 60.0, 40.0, 20.0];
    let mut p = params();
    p.table_regions = vec![TableRegion {
        bbox: region_bbox,
        row_ys,
        col_xs: Vec::new(),
    }];
    let z = segment(items, &p);
    match z {
        Zone::Table { rows, .. } => {
            assert_eq!(rows.len(), 4, "row_ys define 4 rows");
            assert!(rows.iter().all(|r| r.len() == 2));
        }
        other => panic!("expected Zone::Table, got {:?}", other),
    }
}

#[test]
fn symmetric_two_column_body_kept_as_columns() {
    // 2-column body text where pdfium-style fragmentation gives each
    // text-run ~40 chars on both sides (under the 60-char median
    // guard) but the per-row content is symmetric across the columns.
    // This should NOT be promoted to a Table — it's parallel prose.
    let mut items = Vec::new();
    for row in 0..10 {
        let y_bottom = (9 - row) as f32 * 20.0 + 50.0;
        let y_top = y_bottom + 10.0;
        // Both columns ~200pt wide; both items ~40 chars (b()'s
        // bbox-width-derived heuristic gives 40).
        items.push(b(0.0, y_bottom, 200.0, y_top));
        items.push(b(220.0, y_bottom, 420.0, y_top));
    }
    let z = segment(items, &params());
    match z {
        Zone::Split {
            dir: SplitDir::Vertical,
            children,
            ..
        } => {
            assert_eq!(children.len(), 2, "expected 2-col vertical split");
        }
        Zone::Table { .. } => panic!("symmetric 2-col body must not be promoted to Table"),
        other => panic!("expected Vertical split, got {:?}", other),
    }
}

#[test]
fn fragmented_two_column_body_kept_as_columns() {
    // Models pdfium-style word-level fragmentation of 2-column body
    // prose: each row has 6 short text-runs per side (~7 chars each,
    // summing to ~42 chars of content per row per side). A naive
    // per-item median (≤10 chars) would otherwise trip the
    // narrow-side waiver and promote this to a Table — exactly the
    // L14 regression. Y-band aggregation must catch it.
    let mut items = Vec::new();
    for row in 0..6 {
        let y_bottom = (5 - row) as f32 * 20.0 + 50.0;
        let y_top = y_bottom + 10.0;
        // 6 narrow word-like text-runs across the left column.
        for w in 0..6 {
            let x_left = (w as f32) * 35.0;
            items.push(b(x_left, y_bottom, x_left + 30.0, y_top));
        }
        // Same on the right column.
        for w in 0..6 {
            let x_left = 230.0 + (w as f32) * 35.0;
            items.push(b(x_left, y_bottom, x_left + 30.0, y_top));
        }
    }
    let z = segment(items, &params());
    match z {
        Zone::Split {
            dir: SplitDir::Vertical,
            children,
            ..
        } => {
            assert_eq!(children.len(), 2, "expected 2-col vertical split");
        }
        Zone::Table { .. } => panic!("word-fragmented 2-col body must not be promoted to Table"),
        other => panic!("expected Vertical split, got {:?}", other),
    }
}

#[test]
fn wrapping_fourth_column_becomes_table() {
    // 4-column × 4-row layout. Cols at x=0..10, 20..50, 60..90,
    // 100..200; col gaps 10 pt each (>= min_v_gap=5). Rows at
    // y=30..40, 50..60, 70..80, 90..100; row gaps 10 pt. Col 0 is
    // narrow (10 of 200 = 5 %) so the narrow-column probe fires.
    // Col 3 row 2 has an extra wrapped line at y=42..50 (8 pt
    // below the main line in the same column). With the old Y-gap
    // grouping the wrap forms its own sparse row and the fill-rate
    // guard rejects the table; the anchor path absorbs it instead.
    let mut items = Vec::new();
    for row in 0..4 {
        let y_bottom = (3 - row) as f32 * 20.0 + 30.0;
        let y_top = y_bottom + 10.0;
        items.push(b(0.0, y_bottom, 10.0, y_top));
        items.push(b(20.0, y_bottom, 50.0, y_top));
        items.push(b(60.0, y_bottom, 90.0, y_top));
        items.push(b(100.0, y_bottom, 200.0, y_top));
        if row == 2 {
            // Wrap continuation in col 3, immediately below row 2.
            items.push(b(100.0, y_bottom - 8.0, 200.0, y_top - 8.0));
        }
    }
    let z = segment(items, &params());
    match z {
        Zone::Table { rows, .. } => {
            assert_eq!(rows.len(), 4, "expected 4 rows, got {}", rows.len());
            assert!(rows.iter().all(|r| r.len() == 4), "every row has 4 cells");
            // Loop iteration row=2 sits at top-down index 2 (third
            // row from top in the emitted table).
            let wrap_cell = &rows[2][3];
            match wrap_cell {
                Zone::Leaf { items, .. } => {
                    assert_eq!(items.len(), 2, "wrapping cell should hold 2 items",)
                }
                other => panic!("expected Leaf in wrap cell, got {:?}", other),
            }
        }
        other => panic!("expected Zone::Table, got {:?}", other),
    }
}

#[test]
fn wrapping_with_tied_min_columns() {
    // 4 cols tied for fewest items (4 each in cols 0..2, 5 in col 3).
    // Anchor tiebreak: lowest column index → col 0. Result must
    // still be a 4-row table.
    let mut items = Vec::new();
    for row in 0..4 {
        let y_bottom = (3 - row) as f32 * 20.0 + 30.0;
        let y_top = y_bottom + 10.0;
        items.push(b(0.0, y_bottom, 10.0, y_top));
        items.push(b(20.0, y_bottom, 50.0, y_top));
        items.push(b(60.0, y_bottom, 90.0, y_top));
        items.push(b(100.0, y_bottom, 200.0, y_top));
        if row == 1 || row == 3 {
            items.push(b(100.0, y_bottom - 8.0, 200.0, y_top - 8.0));
        }
    }
    let z = segment(items, &params());
    match z {
        Zone::Table { rows, .. } => {
            assert_eq!(rows.len(), 4);
            assert!(rows.iter().all(|r| r.len() == 4));
        }
        other => panic!("expected Zone::Table, got {:?}", other),
    }
}

#[test]
fn row_anchor_empty_column_padded() {
    // 4-col × 4-row table where col 2 row 1 (top-down index 2) is
    // missing one item, plus one wrap in col 3 to force the anchor
    // path. The remaining items still cluster into a 4-row table;
    // the missing cell becomes an empty Leaf and the fill-rate
    // guard still accepts the table (3-of-4 rows fully filled).
    let mut items = Vec::new();
    for row in 0..4 {
        let y_bottom = (3 - row) as f32 * 20.0 + 30.0;
        let y_top = y_bottom + 10.0;
        items.push(b(0.0, y_bottom, 10.0, y_top));
        items.push(b(20.0, y_bottom, 50.0, y_top));
        if row != 1 {
            items.push(b(60.0, y_bottom, 90.0, y_top));
        }
        items.push(b(100.0, y_bottom, 200.0, y_top));
    }
    // Force non-uniform col counts → anchor path.
    items.push(b(100.0, 30.0 - 8.0, 200.0, 40.0 - 8.0));
    let z = segment(items, &params());
    match z {
        Zone::Table { rows, .. } => {
            assert_eq!(rows.len(), 4);
            assert!(rows.iter().all(|r| r.len() == 4));
            // Loop iteration row=1 sits at top-down index 1 (second
            // row from top in the emitted table).
            let empty_cell = &rows[1][2];
            match empty_cell {
                Zone::Leaf { items, .. } => {
                    assert!(items.is_empty(), "expected empty cell")
                }
                other => panic!("expected empty Leaf, got {:?}", other),
            }
        }
        other => panic!("expected Zone::Table, got {:?}", other),
    }
}
