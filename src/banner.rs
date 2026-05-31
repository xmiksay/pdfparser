//! Header/footer (banner) detection and stripping.

use std::collections::{HashMap, HashSet};

use super::element::{PageElement, TextElement};
use super::zone::{BBox, Bounded};

pub(super) fn bbox_of_elements(elements: &[PageElement]) -> BBox {
    if elements.is_empty() {
        return BBox::empty();
    }
    let mut acc = elements[0].bbox();
    for el in &elements[1..] {
        let b = el.bbox();
        acc = BBox {
            left: acc.left.min(b.left),
            right: acc.right.max(b.right),
            top: acc.top.max(b.top),
            bottom: acc.bottom.min(b.bottom),
        };
    }
    acc
}

// ---- header/footer detection ------------------------------------------------

pub(super) const BANNER_BAND_FRACTION: f32 = 0.10;
const BANNER_MIN_PAGES: usize = 3;
/// Minimum token length (after digit-run normalization) considered for
/// per-token banner detection. 4 chars filters out filler words like "of",
/// "ze", "a" while keeping branding tokens such as "PŘEDPIS" or "HLAVA".
const BANNER_MIN_TOKEN_LEN: usize = 4;

/// Detect repeating page banners (headers and footers) by scanning the top and
/// bottom 10% bands of every page, line-grouping the text within each band,
/// normalizing (collapse whitespace, replace digit runs with `\d`), and
/// returning three banner sets:
///   * `lines` — normalized full-line strings on `>= 25 %` of pages
///   * `tokens` — normalized whitespace-separated tokens (≥ 4 chars after
///     digit normalization) appearing on `>= 25 %` of pages. A token is
///     counted once per page even if it recurs within that page's bands.
///   * `image_hashes` — `Figure.hash` of images whose bbox lies in the top
///     or bottom band on `>= 25 %` of pages. Catches repeating brand logos
///     placed in page headers/footers.
///
/// The token set catches mirrored running headers (e.g. L14's
/// "HLAVA n PŘEDPIS L14" / "PŘEDPIS L14 HLAVA n") that the line-level set
/// misses because the chapter number breaks whole-line equality.
pub(super) fn detect_banners(
    per_page_elements: &[Vec<PageElement>],
    per_page_bbox: &[BBox],
) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let n_pages = per_page_elements.len();
    if n_pages < BANNER_MIN_PAGES {
        return (HashSet::new(), HashSet::new(), HashSet::new());
    }
    // Threshold: line/token must appear on ≥ 25 % of pages (or
    // BANNER_MIN_PAGES, whichever is greater). The historical 50 % was too
    // strict for PDFs that alternate header layouts on even/odd pages.
    let threshold = std::cmp::max(BANNER_MIN_PAGES, n_pages.div_ceil(4));

    let mut line_counts: HashMap<String, usize> = HashMap::new();
    let mut token_counts: HashMap<String, usize> = HashMap::new();
    let mut image_counts: HashMap<String, usize> = HashMap::new();
    for (page_idx, elements) in per_page_elements.iter().enumerate() {
        let bbox = per_page_bbox[page_idx];
        // Token uniqueness *within* a page: only count each token once per
        // page so that a single chapter page printing "PŘEDPIS" twice in
        // its header doesn't inflate the across-page count.
        let mut page_tokens: HashSet<String> = HashSet::new();
        for line in band_lines(elements, &bbox) {
            let n = normalize_banner(&line);
            if !n.is_empty() {
                *line_counts.entry(n.clone()).or_insert(0) += 1;
                for tok in tokenize_banner(&n) {
                    page_tokens.insert(tok);
                }
            }
        }
        for tok in page_tokens {
            *token_counts.entry(tok).or_insert(0) += 1;
        }
        // Same dedup per page for image figures: a logo appearing twice
        // on the same page (e.g. header + footer) still only counts as
        // one occurrence for the across-page tally.
        let mut page_image_hashes: HashSet<String> = HashSet::new();
        let height = bbox.height();
        if height > 0.0 {
            let top_cutoff = bbox.top - height * BANNER_BAND_FRACTION;
            let bottom_cutoff = bbox.bottom + height * BANNER_BAND_FRACTION;
            for el in elements {
                if let PageElement::Image(im) = el {
                    let cy = (im.top + im.bottom) / 2.0;
                    if cy >= top_cutoff || cy <= bottom_cutoff {
                        page_image_hashes.insert(im.figure.hash.clone());
                    }
                }
            }
        }
        for h in page_image_hashes {
            *image_counts.entry(h).or_insert(0) += 1;
        }
    }
    let lines: HashSet<String> = line_counts
        .into_iter()
        .filter(|(_, c)| *c >= threshold)
        .map(|(k, _)| k)
        .collect();
    let tokens: HashSet<String> = token_counts
        .into_iter()
        .filter(|(_, c)| *c >= threshold)
        .map(|(k, _)| k)
        .collect();
    let image_hashes: HashSet<String> = image_counts
        .into_iter()
        .filter(|(_, c)| *c >= threshold)
        .map(|(k, _)| k)
        .collect();
    (lines, tokens, image_hashes)
}

/// Group elements within the top/bottom band of a page into single-line strings
/// (sorted left-to-right within each line, lines separated by Y center).
fn band_lines(elements: &[PageElement], bbox: &BBox) -> Vec<String> {
    let height = bbox.height();
    if height <= 0.0 {
        return Vec::new();
    }
    let top_cutoff = bbox.top - height * BANNER_BAND_FRACTION;
    let bottom_cutoff = bbox.bottom + height * BANNER_BAND_FRACTION;

    let mut texts: Vec<&TextElement> = elements
        .iter()
        .filter_map(|el| match el {
            PageElement::Text(t) => {
                let cy = t.y_center();
                if cy >= top_cutoff || cy <= bottom_cutoff {
                    Some(t)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();
    if texts.is_empty() {
        return Vec::new();
    }
    // Sort by Y descending (top-first), then X ascending.
    texts.sort_by(|a, b| {
        b.y_center()
            .partial_cmp(&a.y_center())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.left
                    .partial_cmp(&b.left)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let mut lines: Vec<Vec<&TextElement>> = Vec::new();
    let mut current: Vec<&TextElement> = Vec::new();
    let mut current_y: Option<f32> = None;
    for t in texts {
        let cy = t.y_center();
        let line_h = (t.top - t.bottom).abs().max(1.0);
        match current_y {
            Some(y) if (cy - y).abs() <= line_h * 0.5 => current.push(t),
            _ => {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                current_y = Some(cy);
                current.push(t);
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }

    lines
        .into_iter()
        .map(|mut line| {
            line.sort_by(|a, b| {
                a.left
                    .partial_cmp(&b.left)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            line.iter()
                .map(|t| t.text.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Collapse whitespace and replace runs of ASCII digits with the literal `\d`
/// so that "Page 1" and "Page 12" normalize to the same banner key.
pub(super) fn normalize_banner(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut prev_digit = false;
    for c in trimmed.chars() {
        if c.is_ascii_digit() {
            if !prev_digit {
                out.push_str("\\d");
            }
            prev_digit = true;
        } else {
            out.push(c);
            prev_digit = false;
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split an already-normalized banner line into tokens worth tracking
/// individually. Keeps whitespace-separated tokens whose character count is
/// at least `BANNER_MIN_TOKEN_LEN`. The input is assumed to have already
/// gone through `normalize_banner` (digit runs collapsed to `\d`).
fn tokenize_banner(normalized: &str) -> Vec<String> {
    normalized
        .split_whitespace()
        .filter(|t| t.chars().count() >= BANNER_MIN_TOKEN_LEN)
        .map(|t| t.to_string())
        .collect()
}

/// Re-run line-grouping on this page's banner bands and drop any text element
/// belonging to a line that matches either:
///   * the full-line banner set (exact normalized match), or
///   * the token-banner set — every ≥ 4-char token (after digit-run
///     normalization) is in `banner_tokens`. Pure-punctuation/digit tokens
///     are treated as auto-covered, so e.g. "PŘEDPIS L14 \d" matches if
///     "PŘEDPIS" and "L14" are both banner tokens.
pub(super) fn strip_banners(
    elements: Vec<PageElement>,
    bbox: BBox,
    banner_lines: &HashSet<String>,
    banner_tokens: &HashSet<String>,
    banner_image_hashes: &HashSet<String>,
) -> Vec<PageElement> {
    if banner_lines.is_empty() && banner_tokens.is_empty() && banner_image_hashes.is_empty() {
        return elements;
    }
    let height = bbox.height();
    if height <= 0.0 {
        return elements;
    }
    let top_cutoff = bbox.top - height * BANNER_BAND_FRACTION;
    let bottom_cutoff = bbox.bottom + height * BANNER_BAND_FRACTION;

    // First pass: gather image indices to drop when their figure hash is a
    // recurring banner image AND their bbox sits in the top/bottom band.
    let mut drop_indices: HashSet<usize> = HashSet::new();
    if !banner_image_hashes.is_empty() {
        for (idx, el) in elements.iter().enumerate() {
            if let PageElement::Image(im) = el {
                let cy = (im.top + im.bottom) / 2.0;
                if (cy >= top_cutoff || cy <= bottom_cutoff)
                    && banner_image_hashes.contains(&im.figure.hash)
                {
                    drop_indices.insert(idx);
                }
            }
        }
    }

    // Walk in band, group into lines (same algorithm as `band_lines`), but
    // remember the original-elements indices so we can drop them.
    let mut indexed: Vec<(usize, &TextElement)> = Vec::new();
    for (idx, el) in elements.iter().enumerate() {
        if let PageElement::Text(t) = el {
            let cy = t.y_center();
            if cy >= top_cutoff || cy <= bottom_cutoff {
                indexed.push((idx, t));
            }
        }
    }
    if indexed.is_empty() && drop_indices.is_empty() {
        return elements;
    }
    indexed.sort_by(|a, b| {
        b.1.y_center()
            .partial_cmp(&a.1.y_center())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.1.left
                    .partial_cmp(&b.1.left)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });

    let mut lines: Vec<Vec<(usize, &TextElement)>> = Vec::new();
    let mut current: Vec<(usize, &TextElement)> = Vec::new();
    let mut current_y: Option<f32> = None;
    for (idx, t) in indexed {
        let cy = t.y_center();
        let line_h = (t.top - t.bottom).abs().max(1.0);
        match current_y {
            Some(y) if (cy - y).abs() <= line_h * 0.5 => current.push((idx, t)),
            _ => {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                current_y = Some(cy);
                current.push((idx, t));
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }

    for line in lines {
        let mut sorted = line;
        sorted.sort_by(|a, b| {
            a.1.left
                .partial_cmp(&b.1.left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let joined = sorted
            .iter()
            .map(|(_, t)| t.text.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let normalized = normalize_banner(&joined);
        let line_match = banner_lines.contains(&normalized);
        // Token coverage: every ≥ 4-char token must be in banner_tokens.
        // Short / pure-punctuation tokens (anything that wouldn't be
        // emitted by `tokenize_banner`) are treated as auto-covered.
        let token_match = !normalized.is_empty() && !banner_tokens.is_empty() && {
            let toks = tokenize_banner(&normalized);
            // Require at least one substantive token so an all-digit /
            // all-punctuation line ("\d \d \d") doesn't vacuously match.
            !toks.is_empty() && toks.iter().all(|t| banner_tokens.contains(t))
        };
        if line_match || token_match {
            for (idx, _) in sorted {
                drop_indices.insert(idx);
            }
        }
    }

    elements
        .into_iter()
        .enumerate()
        .filter_map(|(idx, el)| {
            if drop_indices.contains(&idx) {
                None
            } else {
                Some(el)
            }
        })
        .collect()
}

/// Fraction of page height (from the bottom) inside which a bare numeric
/// text run is always treated as a page-number footer and dropped.
const FOOTER_DIGIT_BAND_FRACTION: f32 = 0.08;

/// Drop text elements whose stripped content is exactly 1–3 digits AND
/// whose bbox sits in the bottom `FOOTER_DIGIT_BAND_FRACTION` of the page.
/// Unlike `strip_banners` this is unconditional — a bare number alone in
/// the bottom band is essentially always a page-number footer that pdfium
/// has split off from the surrounding paragraph stream.
pub(super) fn strip_bottom_band_bare_digits(
    elements: Vec<PageElement>,
    bbox: BBox,
) -> Vec<PageElement> {
    let height = bbox.height();
    if height <= 0.0 {
        return elements;
    }
    let cutoff = bbox.bottom + height * FOOTER_DIGIT_BAND_FRACTION;
    elements
        .into_iter()
        .filter(|el| match el {
            PageElement::Text(t) => {
                let cy = t.y_center();
                if cy > cutoff {
                    return true;
                }
                let trimmed = t.text.trim();
                let is_bare_digits = !trimmed.is_empty()
                    && trimmed.len() <= 3
                    && trimmed.chars().all(|c| c.is_ascii_digit());
                !is_bare_digits
            }
            _ => true,
        })
        .collect()
}

#[cfg(test)]
#[path = "banner_tests.rs"]
mod tests;
