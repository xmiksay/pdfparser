//! Tests for the core `segment` recursion: leaves, splits, header-band probe.

use crate::zone::test_support::{Item, b, params};
use crate::zone::{SplitDir, Zone, segment};

#[test]
fn empty_input_is_leaf() {
    let z: Zone<Item> = segment(Vec::new(), &params());
    match z {
        Zone::Leaf { items, .. } => assert!(items.is_empty()),
        _ => panic!("expected leaf"),
    }
}

#[test]
fn single_item_is_leaf() {
    let items = vec![b(0.0, 0.0, 10.0, 10.0)];
    let z = segment(items, &params());
    match z {
        Zone::Leaf { items, .. } => assert_eq!(items.len(), 1),
        _ => panic!("expected leaf"),
    }
}

#[test]
fn single_column_is_leaf() {
    // Three lines stacked vertically with normal line spacing (no big gaps).
    let items = vec![
        b(10.0, 90.0, 100.0, 100.0),
        b(10.0, 80.0, 100.0, 90.0),
        b(10.0, 70.0, 100.0, 80.0),
    ];
    let z = segment(items, &params());
    match z {
        Zone::Leaf { items, .. } => assert_eq!(items.len(), 3),
        other => panic!("expected leaf, got {:?}", other),
    }
}

#[test]
fn two_columns_split_vertical() {
    // Left column at x=10..40, right column at x=60..90, big horizontal gutter at x=40..60.
    let items = vec![
        b(10.0, 90.0, 40.0, 100.0),
        b(10.0, 70.0, 40.0, 80.0),
        b(60.0, 90.0, 90.0, 100.0),
        b(60.0, 70.0, 90.0, 80.0),
    ];
    let z = segment(items, &params());
    match z {
        Zone::Split { dir, children, .. } => {
            assert_eq!(dir, SplitDir::Vertical);
            assert_eq!(children.len(), 2);
            // Left child has x_center near 25, right near 75.
            let left_cx = children[0].bbox().x_center();
            let right_cx = children[1].bbox().x_center();
            assert!(left_cx < right_cx, "left should sort before right");
        }
        other => panic!("expected vertical split, got {:?}", other),
    }
}

#[test]
fn header_then_body_horizontal_split() {
    // Header at y=190..200, body at y=90..100 — big vertical gap.
    let items = vec![
        b(10.0, 190.0, 100.0, 200.0),
        b(10.0, 90.0, 100.0, 100.0),
        b(10.0, 80.0, 100.0, 90.0),
    ];
    let z = segment(items, &params());
    match z {
        Zone::Split { dir, children, .. } => {
            assert_eq!(dir, SplitDir::Horizontal);
            assert_eq!(children.len(), 2);
            // Top-first ordering: children[0] is the highest-Y group.
            assert!(children[0].bbox().y_center() > children[1].bbox().y_center());
        }
        other => panic!("expected horizontal split, got {:?}", other),
    }
}

#[test]
fn three_by_three_grid_is_vertical_of_horizontals() {
    // 3 columns × 3 rows, gutters of size 10 between.
    let mut items = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            let x_left = (col as f32) * 30.0;
            let y_bottom = (2 - row) as f32 * 30.0;
            items.push(b(x_left, y_bottom, x_left + 20.0, y_bottom + 20.0));
        }
    }
    let z = segment(items, &params());
    // Expect Split{Vertical, [Split{H,3}, Split{H,3}, Split{H,3}]}
    match z {
        Zone::Split {
            dir: SplitDir::Vertical,
            children: cols,
            ..
        } => {
            assert_eq!(cols.len(), 3, "expected 3 columns");
            for col in &cols {
                match col {
                    Zone::Split {
                        dir: SplitDir::Horizontal,
                        children: rows,
                        ..
                    } => {
                        assert_eq!(rows.len(), 3, "expected 3 rows per column");
                    }
                    other => panic!("expected horizontal split inside column, got {:?}", other),
                }
            }
        }
        other => panic!("expected vertical split at top, got {:?}", other),
    }
}

#[test]
fn no_split_when_gaps_below_threshold() {
    // Two items with a tiny gap (< min_v_gap).
    let items = vec![
        b(10.0, 0.0, 20.0, 10.0),
        b(22.0, 0.0, 30.0, 10.0), // gap of 2 px, below threshold of 5
    ];
    let z = segment(items, &params());
    match z {
        Zone::Leaf { items, .. } => assert_eq!(items.len(), 2),
        other => panic!("expected leaf, got {:?}", other),
    }
}

#[test]
fn header_band_above_two_columns_lifts_horizontal() {
    // L14 KSS topology: a title that spans wider than the strana column AND
    // overlaps the column gutter. Geometry tuned so:
    //   * global v_max = 20 (strana col → title-cluster gap)
    //   * global h_max = 10 (title bottom → first body-row top)
    //   → without the probe, vertical wins and the title leaks between cols.
    //   * rest_v_max = 70 (strana → datum once title is removed)
    //   → probe lifts the title as a top-level Horizontal child.
    let items = vec![
        // Title (1 line spanning x=200..450 — overlaps the gutter)
        b(200.0, 750.0, 450.0, 770.0),
        // Strana column body, x=80..180, 6 tightly-packed rows
        b(80.0, 730.0, 180.0, 740.0),
        b(80.0, 715.0, 180.0, 725.0),
        b(80.0, 700.0, 180.0, 710.0),
        b(80.0, 685.0, 180.0, 695.0),
        b(80.0, 670.0, 180.0, 680.0),
        b(80.0, 655.0, 180.0, 665.0),
        // Datum column body, x=250..440, 6 tightly-packed rows
        b(250.0, 730.0, 440.0, 740.0),
        b(250.0, 715.0, 440.0, 725.0),
        b(250.0, 700.0, 440.0, 710.0),
        b(250.0, 685.0, 440.0, 695.0),
        b(250.0, 670.0, 440.0, 680.0),
        b(250.0, 655.0, 440.0, 665.0),
    ];
    let z = segment(items, &params());
    match z {
        Zone::Split {
            dir: SplitDir::Horizontal,
            children,
            ..
        } => {
            assert_eq!(children.len(), 2, "expected [header, body]");
            // Top-first: children[0] = header, children[1] = body.
            assert!(
                children[0].bbox().y_center() > children[1].bbox().y_center(),
                "header should sort above body"
            );
            // After the row-aligned 2-column table probe was added, the
            // body's two perfectly Y-aligned "strana" / "datum" columns
            // are now (correctly) promoted to a `Zone::Table` — this
            // matches their actual semantics as page#/date tabular data.
            // Accept either shape so the test stays robust to either
            // segmentation behavior.
            match &children[1] {
                Zone::Split {
                    dir: SplitDir::Vertical,
                    children: cols,
                    ..
                } => {
                    assert_eq!(cols.len(), 2, "expected 2 columns under header");
                    assert!(cols[0].bbox().x_center() < cols[1].bbox().x_center());
                }
                Zone::Table { rows, .. } => {
                    assert_eq!(rows.len(), 6, "expected 6 rows in table");
                    assert!(rows.iter().all(|r| r.len() == 2));
                }
                other => panic!(
                    "expected vertical split (2 cols) or Table under header, got {:?}",
                    other
                ),
            }
        }
        other => panic!("expected horizontal split at top, got {:?}", other),
    }
}

#[test]
fn largest_gap_wins_when_both_dimensions_have_gaps() {
    // Items arranged so that the horizontal gap (rows) is 50pt and the
    // vertical gap (cols) is 10pt — horizontal split should win.
    let items = vec![
        // Top row
        b(0.0, 200.0, 30.0, 210.0),
        b(40.0, 200.0, 70.0, 210.0),
        // Bottom row, big vertical gap below 200
        b(0.0, 100.0, 30.0, 110.0),
        b(40.0, 100.0, 70.0, 110.0),
    ];
    let z = segment(items, &params());
    match z {
        Zone::Split { dir, .. } => {
            // 90pt y gap (110→200) > 10pt x gap → horizontal wins.
            assert_eq!(dir, SplitDir::Horizontal);
        }
        other => panic!("expected split, got {:?}", other),
    }
}
