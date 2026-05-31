//! Zone-tree emission: row-major flatten, GFM table rendering, and dispatch.

use tracing::debug;

use super::element::{PageCtx, PageElement};
use super::headings::HeadingClassifier;
use super::render_leaf::{emit_leaf, escape_cell, render_simple_cell};
use super::text::{RunRef, render_runs};
use super::zone::{BBox, SplitDir, Zone};

// ---- zone-tree emission -----------------------------------------------------

pub(super) fn emit_zone(
    zone: &Zone<PageElement>,
    classifier: &HeadingClassifier,
    max_heading: &mut u8,
    ctx: &PageCtx,
) -> String {
    match zone {
        Zone::Leaf { items, .. } => emit_leaf(items, classifier, max_heading, ctx),
        Zone::Split {
            dir,
            bbox,
            children,
        } => {
            if *dir == SplitDir::Vertical {
                // First: try treating the columns as a column-major
                // GFM table (uniform-row-count case).
                if let Some(md) = try_emit_table(children, classifier, ctx.space_threshold) {
                    debug!(cols = children.len(), "table detected (column-major)");
                    return md;
                }
                // A tall vertical split is genuine side-by-side columns: read
                // each column fully (top→bottom), then the next, joined by a
                // blank line. Row-major would interleave independent columns
                // into one line ("Úvodní poznámka: 1.1 Definice").
                if looks_like_columns(*bbox, children, ctx) {
                    debug!(cols = children.len(), "columns detected (column-by-column)");
                    let mut parts: Vec<String> = Vec::new();
                    for child in children {
                        let s = emit_zone(child, classifier, max_heading, ctx);
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            parts.push(trimmed.to_string());
                        }
                    }
                    return parts.join("\n\n");
                }
                // Short vertical split: a single line cut by a wide inter-word
                // gap (false gutter). Row-major rejoins it in reading order.
                return emit_row_major(children, classifier, max_heading, ctx);
            }
            let mut parts: Vec<String> = Vec::new();
            for child in children {
                let s = emit_zone(child, classifier, max_heading, ctx);
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
            }
            parts.join("\n\n")
        }
        Zone::Table { rows, .. } => {
            debug!(
                rows = rows.len(),
                cols = rows.first().map(|r| r.len()).unwrap_or(0),
                "table detected (row-major, narrow-column promotion)"
            );
            emit_table(rows, classifier, max_heading, ctx)
        }
    }
}

/// Decide whether a `Vertical` split is genuine side-by-side columns (read
/// column-by-column) rather than a single line cut by a wide inter-word gap
/// (a false gutter, where row-major correctly rejoins the line).
///
/// Uses the split *region* height — not per-child — so asymmetric columns
/// (left 25 lines, right 2 lines) are still recognized. Requires ≥2 children
/// that actually carry content, so a tall region with one real column plus a
/// stray fragment falls through to row-major.
fn looks_like_columns(bbox: BBox, children: &[Zone<PageElement>], ctx: &PageCtx) -> bool {
    const MIN_COLUMN_HEIGHT_LINES: f32 = 3.0;
    children.len() >= 2
        && bbox.height() >= ctx.median_line_height.max(1.0) * MIN_COLUMN_HEIGHT_LINES
        && children.iter().filter(|c| c.item_count() >= 2).count() >= 2
}

/// Flatten a list of vertical-split children into a single row-major
/// markdown stream. Collects every `PageElement` from the subtree,
/// sorts by Y-desc then X-asc, groups items whose `y_center` is within
/// 0.5 × median line height into a logical "row", and emits each row
/// as a single line of space-joined text (or, for image items, the
/// `<image hash="...">` directive).
///
/// Used as the fallback when a Vertical zone-split can't be promoted
/// to a `Zone::Table` — keeps reading order intact instead of dumping
/// one column at a time.
fn emit_row_major(
    children: &[Zone<PageElement>],
    classifier: &HeadingClassifier,
    _max_heading: &mut u8,
    ctx: &PageCtx,
) -> String {
    fn collect<'a>(zone: &'a Zone<PageElement>, out: &mut Vec<&'a PageElement>) {
        match zone {
            Zone::Leaf { items, .. } => {
                for it in items {
                    out.push(it);
                }
            }
            Zone::Split { children, .. } => {
                for c in children {
                    collect(c, out);
                }
            }
            Zone::Table { rows, .. } => {
                for row in rows {
                    for cell in row {
                        collect(cell, out);
                    }
                }
            }
        }
    }

    let mut elements: Vec<&PageElement> = Vec::new();
    for child in children {
        collect(child, &mut elements);
    }
    if elements.is_empty() {
        return String::new();
    }

    let y_of = |el: &&PageElement| match el {
        PageElement::Text(t) => t.y_center(),
        PageElement::Image(i) => (i.top + i.bottom) / 2.0,
    };
    let x_of = |el: &&PageElement| match el {
        PageElement::Text(t) => t.left,
        PageElement::Image(i) => i.left,
    };

    elements.sort_by(|a, b| {
        let ya = y_of(a);
        let yb = y_of(b);
        yb.partial_cmp(&ya)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                x_of(a)
                    .partial_cmp(&x_of(b))
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let y_tol = ctx.median_line_height.max(8.0) * 0.5;
    let mut lines: Vec<Vec<&PageElement>> = Vec::new();
    let mut current_y: Option<f32> = None;
    for el in &elements {
        let yc = y_of(el);
        let join = match current_y {
            Some(prev) => (prev - yc).abs() <= y_tol,
            None => false,
        };
        if join {
            lines
                .last_mut()
                .expect("join is false when lines is empty")
                .push(*el);
        } else {
            lines.push(vec![*el]);
            current_y = Some(yc);
        }
    }

    // Re-sort each line strictly by X-asc. The outer sort uses Y as
    // the primary key, so items in the same logical row whose
    // `y_center` differs by a fraction of a point (italics descenders,
    // subscripts, multi-font runs) would otherwise appear out of order
    // within the row.
    for line in &mut lines {
        line.sort_by(|a, b| {
            x_of(a)
                .partial_cmp(&x_of(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    let mut out = String::new();
    for line in lines {
        let has_image = line.iter().any(|el| matches!(el, PageElement::Image(_)));
        let rendered = if has_image {
            // Rare: an image shares a row-band with text — keep the simple
            // element join rather than splitting the run stream around it.
            line.iter()
                .filter_map(|el| match el {
                    PageElement::Text(t) => {
                        let s = t.text.trim();
                        (!s.is_empty()).then(|| s.to_string())
                    }
                    PageElement::Image(im) => Some((ctx.image_emitter)(&im.figure.hash, "")),
                })
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            // Merge runs with gap-aware spacing + emphasis coalescing so words
            // pdfium split into runs rejoin ("fyzi cké" → "fyzické") and
            // emphasis renders as one span instead of one per run.
            let runs: Vec<RunRef> = line
                .iter()
                .filter_map(|el| match el {
                    PageElement::Text(t) => Some(RunRef {
                        text: t.text.as_str(),
                        sig: &t.sig,
                        x_left: t.left,
                        x_right: t.right,
                    }),
                    PageElement::Image(_) => None,
                })
                .collect();
            render_runs(&runs, classifier, ctx.space_threshold, true)
        };
        if rendered.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&rendered);
    }
    out
}

/// Render a `Zone::Table` as a GFM markdown table. First row becomes the
/// header. Cells that are simple text leaves use the existing fast path
/// (`render_simple_cell`); anything more complex falls back to a recursive
/// `emit_zone` followed by `escape_cell` so a single oddly-shaped cell
/// doesn't abort the whole table.
fn emit_table(
    rows: &[Vec<Zone<PageElement>>],
    classifier: &HeadingClassifier,
    max_heading: &mut u8,
    ctx: &PageCtx,
) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let n_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if n_cols == 0 {
        return String::new();
    }

    let render_cell = |cell: &Zone<PageElement>, max_heading: &mut u8| -> String {
        if is_simple_text_leaf(cell) {
            escape_cell(&render_simple_cell(cell, classifier, ctx.space_threshold))
        } else {
            // Fallback for non-trivial cell content. Recurse, then strip
            // any newlines via escape_cell so the GFM row stays on one line.
            let inner = emit_zone(cell, classifier, max_heading, ctx);
            escape_cell(inner.trim())
        }
    };

    let push_row = |row: &[Zone<PageElement>], max_heading: &mut u8, out: &mut String| {
        out.push('|');
        for c in 0..n_cols {
            out.push(' ');
            if let Some(cell) = row.get(c) {
                out.push_str(&render_cell(cell, max_heading));
            }
            out.push_str(" |");
        }
        out.push('\n');
    };

    let mut md = String::new();
    push_row(&rows[0], max_heading, &mut md);
    md.push('|');
    for _ in 0..n_cols {
        md.push_str(" --- |");
    }
    md.push('\n');
    for row in &rows[1..] {
        push_row(row, max_heading, &mut md);
    }
    md
}

const TABLE_CELL_MAX_CHARS: usize = 200;
const TABLE_ROW_Y_TOLERANCE_SCALE: f32 = 0.6; // multiples of cell height

/// Attempt to render a `Vertical` split's children as a GFM markdown table.
/// Returns `None` if the structure doesn't match (caller falls back to
/// column-by-column emission).
fn try_emit_table(
    cols: &[Zone<PageElement>],
    classifier: &HeadingClassifier,
    space_threshold: f32,
) -> Option<String> {
    if cols.len() < 2 {
        return None;
    }
    // Each column must be a Horizontal split of leaves.
    let col_rows: Vec<&Vec<Zone<PageElement>>> = cols
        .iter()
        .map(|c| match c {
            Zone::Split {
                dir: SplitDir::Horizontal,
                children,
                ..
            } => Some(children),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;

    let n_rows = col_rows[0].len();
    if n_rows < 2 {
        return None;
    }
    if !col_rows.iter().all(|c| c.len() == n_rows) {
        return None;
    }

    // Every cell must be a leaf containing only text, no images, ≤ 200 chars.
    for col in &col_rows {
        for cell in col.iter() {
            if !is_simple_text_leaf(cell) {
                return None;
            }
        }
    }

    // Row Y centers must align across columns (within tolerance based on row
    // height). This rules out coincidental Vertical-of-Horizontals where the
    // column splits don't actually correspond to table rows.
    let row_centers: Vec<Vec<f32>> = (0..n_rows)
        .map(|r| {
            col_rows
                .iter()
                .map(|col| col[r].bbox().y_center())
                .collect()
        })
        .collect();
    let row_heights: Vec<f32> = (0..n_rows)
        .map(|r| {
            col_rows
                .iter()
                .map(|col| col[r].bbox().height())
                .fold(0.0_f32, f32::max)
        })
        .collect();
    for (r, centers) in row_centers.iter().enumerate() {
        let lo = centers.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = centers.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let tol = (row_heights[r].max(8.0)) * TABLE_ROW_Y_TOLERANCE_SCALE;
        if hi - lo > tol {
            return None;
        }
    }

    // Render: cell[r][c] = col_rows[c][r].
    let n_cols = col_rows.len();
    let mut md = String::new();
    let push_row = |row_idx: usize, md: &mut String| {
        md.push('|');
        for col in col_rows.iter() {
            let text = render_simple_cell(&col[row_idx], classifier, space_threshold);
            md.push(' ');
            md.push_str(&escape_cell(&text));
            md.push_str(" |");
        }
        md.push('\n');
    };
    push_row(0, &mut md);
    md.push('|');
    for _ in 0..n_cols {
        md.push_str(" --- |");
    }
    md.push('\n');
    for r in 1..n_rows {
        push_row(r, &mut md);
    }
    Some(md)
}

fn is_simple_text_leaf(zone: &Zone<PageElement>) -> bool {
    match zone {
        Zone::Leaf { items, .. } => {
            let mut total_chars = 0usize;
            for it in items {
                match it {
                    PageElement::Image(_) => return false,
                    PageElement::Text(t) => total_chars += t.text.chars().count(),
                }
            }
            total_chars > 0 && total_chars <= TABLE_CELL_MAX_CHARS
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::element::TextElement;
    use super::super::headings::FontSignature;
    use super::super::zone::Bounded;
    use super::*;

    fn body_sig() -> FontSignature {
        FontSignature::new(10.0, false, false)
    }

    fn classifier() -> HeadingClassifier {
        HeadingClassifier::build(std::iter::once((
            body_sig(),
            "the quick brown fox body text",
        )))
    }

    fn ctx() -> PageCtx {
        PageCtx {
            median_char_width: 5.0,
            median_line_height: 10.0,
            x_gap_line_break: 20.0,
            indent_threshold: 10.0,
            space_threshold: 1.25,
            image_emitter: crate::document::default_image_emitter(),
        }
    }

    fn txt(s: &str, left: f32, right: f32, top: f32, bottom: f32) -> PageElement {
        PageElement::Text(TextElement {
            text: s.to_string(),
            sig: body_sig(),
            left,
            right,
            top,
            bottom,
        })
    }

    fn bbox_of(get: impl Iterator<Item = BBox>) -> BBox {
        let mut acc = BBox {
            left: f32::MAX,
            right: f32::MIN,
            top: f32::MIN,
            bottom: f32::MAX,
        };
        for b in get {
            acc.left = acc.left.min(b.left);
            acc.right = acc.right.max(b.right);
            acc.top = acc.top.max(b.top);
            acc.bottom = acc.bottom.min(b.bottom);
        }
        acc
    }

    fn leaf(items: Vec<PageElement>) -> Zone<PageElement> {
        let bbox = bbox_of(items.iter().map(|i| i.bbox()));
        Zone::Leaf { bbox, items }
    }

    fn vsplit(children: Vec<Zone<PageElement>>) -> Zone<PageElement> {
        let bbox = bbox_of(children.iter().map(|c| c.bbox()));
        Zone::Split {
            dir: SplitDir::Vertical,
            bbox,
            children,
        }
    }

    #[test]
    fn tall_vertical_split_reads_column_by_column() {
        // A tall 2-column region (~5 lines each) must read left column fully,
        // then right column — not interleaved row-major.
        let left = leaf(vec![
            txt("LEFTA", 0.0, 40.0, 100.0, 90.0),
            txt("LEFTB", 0.0, 40.0, 88.0, 78.0),
            txt("LEFTC", 0.0, 40.0, 76.0, 66.0),
            txt("LEFTD", 0.0, 40.0, 64.0, 54.0),
        ]);
        let right = leaf(vec![
            txt("RIGHTA", 60.0, 100.0, 100.0, 90.0),
            txt("RIGHTB", 60.0, 100.0, 88.0, 78.0),
            txt("RIGHTC", 60.0, 100.0, 76.0, 66.0),
            txt("RIGHTD", 60.0, 100.0, 64.0, 54.0),
        ]);
        let zone = vsplit(vec![left, right]);
        let mut max_h = 0u8;
        let md = emit_zone(&zone, &classifier(), &mut max_h, &ctx());

        assert!(
            !md.lines()
                .any(|l| l.contains("LEFT") && l.contains("RIGHT")),
            "columns interleaved onto one line: {md:?}"
        );
        let last_left = md.rfind("LEFTD").expect("left column text present");
        let first_right = md.find("RIGHTA").expect("right column text present");
        assert!(
            last_left < first_right,
            "entire left column should precede the right column: {md:?}"
        );
    }

    #[test]
    fn short_vertical_split_reads_row_major() {
        // A single line cut by a wide inter-word gap (a false gutter) is too
        // short to be columns: row-major rejoins it onto one line.
        let left = leaf(vec![
            txt("LE", 0.0, 20.0, 100.0, 90.0),
            txt("FT", 20.0, 40.0, 100.0, 90.0),
        ]);
        let right = leaf(vec![
            txt("RI", 60.0, 80.0, 100.0, 90.0),
            txt("GHT", 80.0, 100.0, 100.0, 90.0),
        ]);
        let zone = vsplit(vec![left, right]);
        let mut max_h = 0u8;
        let md = emit_zone(&zone, &classifier(), &mut max_h, &ctx());

        assert!(
            md.lines().any(|l| l.contains("LE") && l.contains("RI")),
            "short false-gutter split should rejoin row-major onto one line: {md:?}"
        );
    }
}
