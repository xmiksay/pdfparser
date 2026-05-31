//! Leaf-zone emission (line + paragraph grouping) and GFM cell rendering.

use super::element::{PageCtx, PageElement, TextElement};
use super::headings::HeadingClassifier;
use super::text::{Line, Paragraph, RunRef, TextRun, group_into_paragraphs, render_runs};
use super::zone::Zone;

// ---- leaf-zone emission (line + paragraph grouping) -------------------------

pub(super) fn emit_leaf(
    elements: &[PageElement],
    classifier: &HeadingClassifier,
    max_heading: &mut u8,
    ctx: &PageCtx,
) -> String {
    // Sort items in reading order: top-down (Y desc), left-right.
    let mut sorted: Vec<&PageElement> = elements.iter().collect();
    sorted.sort_by(|a, b| {
        let ya = match a {
            PageElement::Text(t) => t.y_center(),
            PageElement::Image(i) => (i.top + i.bottom) / 2.0,
        };
        let yb = match b {
            PageElement::Text(t) => t.y_center(),
            PageElement::Image(i) => (i.top + i.bottom) / 2.0,
        };
        let xa = match a {
            PageElement::Text(t) => t.left,
            PageElement::Image(i) => i.left,
        };
        let xb = match b {
            PageElement::Text(t) => t.left,
            PageElement::Image(i) => i.left,
        };
        yb.partial_cmp(&ya)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(xa.partial_cmp(&xb).unwrap_or(std::cmp::Ordering::Equal))
    });

    let hyphen_re = regex::Regex::new(r"(\w+)-\n(\w+)").unwrap();
    let mut output = String::new();

    let mut i = 0;
    while i < sorted.len() {
        match sorted[i] {
            PageElement::Image(img) => {
                if !output.is_empty() && !output.ends_with("\n\n") {
                    output.push_str("\n\n");
                }
                output.push_str(&(ctx.image_emitter)(&img.figure.hash, ""));
                output.push_str("\n\n");
                i += 1;
            }
            PageElement::Text(_) => {
                let mut lines: Vec<Line> = Vec::new();
                while i < sorted.len() {
                    if let PageElement::Text(te) = sorted[i] {
                        if let Some(last_line) = lines.last_mut() {
                            let y_diff = (te.y_center() - last_line.y_center).abs();
                            let y_threshold = last_line.avg_line_height() * 0.5;
                            // X-gap guard: rightmost run on the line vs this element.
                            let x_gap = te.left - last_line.x_right;
                            if y_diff <= y_threshold && x_gap <= ctx.x_gap_line_break {
                                last_line.push_run(TextRun {
                                    text: te.text.clone(),
                                    sig: te.sig.clone(),
                                    x_left: te.left,
                                    x_right: te.right,
                                });
                                last_line.update_y(te);
                                i += 1;
                                continue;
                            }
                        }
                        let line = Line {
                            y_center: te.y_center(),
                            y_top: te.top,
                            y_bottom: te.bottom,
                            x_start: te.left,
                            x_right: te.right,
                            runs: vec![TextRun {
                                text: te.text.clone(),
                                sig: te.sig.clone(),
                                x_left: te.left,
                                x_right: te.right,
                            }],
                        };
                        lines.push(line);
                        i += 1;
                    } else {
                        break;
                    }
                }

                for line in &mut lines {
                    line.runs.sort_by(|a, b| {
                        a.x_left
                            .partial_cmp(&b.x_left)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                }

                let paragraphs = group_into_paragraphs(&lines, classifier, ctx);
                for para in &paragraphs {
                    if !output.is_empty() && !output.ends_with("\n\n") {
                        output.push_str("\n\n");
                    }
                    match para {
                        Paragraph::Heading { level, text } => {
                            if *level > *max_heading {
                                *max_heading = *level;
                            }
                            let prefix = "#".repeat(*level as usize);
                            output.push_str(&format!("{prefix} {text}"));
                        }
                        Paragraph::Body { text } => {
                            let joined = hyphen_re.replace_all(text, "$1$2");
                            output.push_str(&joined);
                        }
                    }
                }
            }
        }
    }

    output.trim().to_string()
}

/// Render a simple-text leaf as plain inline text: sort runs by Y desc / X asc,
/// concatenate with single spaces, strip leading/trailing whitespace.
pub(super) fn render_simple_cell(
    zone: &Zone<PageElement>,
    classifier: &HeadingClassifier,
    space_threshold: f32,
) -> String {
    let Zone::Leaf { items, .. } = zone else {
        return String::new();
    };
    let mut texts: Vec<&TextElement> = items
        .iter()
        .filter_map(|el| match el {
            PageElement::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    if texts.is_empty() {
        return String::new();
    }
    // Reading order inside a cell: group items into Y-bands first (so
    // wrapped lines stay together), then sort each band by X-asc, then
    // emit bands top-down. Sorting by (y_center desc, x_center asc) in
    // one pass mis-orders within-line items when pdfium reports the
    // same logical line at slightly different Y values (italic
    // descenders, mixed fonts).
    texts.sort_by(|a, b| {
        b.y_center()
            .partial_cmp(&a.y_center())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let median_height = {
        let mut hs: Vec<f32> = texts.iter().map(|t| (t.top - t.bottom).abs()).collect();
        hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        if hs.is_empty() {
            8.0
        } else {
            hs[hs.len() / 2].max(8.0)
        }
    };
    let y_tol = median_height * 0.5;
    let mut bands: Vec<Vec<&TextElement>> = Vec::new();
    let mut current_y: Option<f32> = None;
    for t in texts {
        let yc = t.y_center();
        let join = match current_y {
            Some(prev) => (prev - yc).abs() <= y_tol,
            None => false,
        };
        if join {
            bands.last_mut().expect("just pushed").push(t);
        } else {
            bands.push(vec![t]);
            current_y = Some(yc);
        }
    }
    for band in &mut bands {
        band.sort_by(|a, b| {
            a.left
                .partial_cmp(&b.left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    bands
        .into_iter()
        .map(|band| {
            // Merge runs with gap-aware spacing (no emphasis in table cells) so
            // pdfium-split words rejoin instead of becoming "fyzi cké".
            let runs: Vec<RunRef> = band
                .iter()
                .map(|t| RunRef {
                    text: t.text.as_str(),
                    sig: &t.sig,
                    x_left: t.left,
                    x_right: t.right,
                })
                .collect();
            render_runs(&runs, classifier, space_threshold, false)
        })
        .filter(|s: &String| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// GFM-table cell text needs `|` and newlines escaped/replaced.
pub(super) fn escape_cell(s: &str) -> String {
    s.replace('\n', " ").replace('|', "\\|")
}
