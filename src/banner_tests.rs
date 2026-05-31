//! Tests for banner (header/footer) detection and stripping.

use super::*;
use crate::PdfFigure as Figure;
use crate::element::{ImageElement, PageElement, TextElement};
use crate::headings::FontSignature;
use crate::zone::BBox;

fn fs() -> FontSignature {
    FontSignature::new(10.0, false, false)
}

/// Build a `TextElement`-wrapped page element with a given y_center and
/// height, x-spanning the full page width.
fn text_at(text: &str, left: f32, right: f32, y_center: f32, height: f32) -> PageElement {
    let top = y_center + height / 2.0;
    let bottom = y_center - height / 2.0;
    PageElement::Text(TextElement {
        text: text.to_string(),
        sig: fs(),
        left,
        right,
        top,
        bottom,
    })
}

/// Build a single-page-width bbox spanning a fixed y-range.
fn page_bbox(y_top: f32, y_bottom: f32) -> BBox {
    BBox {
        left: 0.0,
        right: 600.0,
        top: y_top,
        bottom: y_bottom,
    }
}

/// Helper: generate a page consisting solely of a header line in the
/// top 10% band of an 800x600 page (so y near 800).
fn header_only_page(header: &str) -> Vec<PageElement> {
    vec![text_at(header, 50.0, 550.0, 790.0, 12.0)]
}

#[test]
fn alternating_headers_caught_via_tokens() {
    // 12 pages — each carries the banner tokens "PŘEDPIS" and "HLAVA"
    // plus a unique trailing word so the full line keys never repeat
    // (line-level detection misses). The tokens recur on every page so
    // the token-level detector catches them.
    let unique_words = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
        "juliet", "kilo", "lima",
    ];
    let mut per_page_elements: Vec<Vec<PageElement>> = Vec::new();
    let mut per_page_bbox: Vec<BBox> = Vec::new();
    for (i, w) in unique_words.iter().enumerate() {
        let line = if i % 2 == 0 {
            format!("HLAVA PŘEDPIS L14 {}", w)
        } else {
            format!("PŘEDPIS L14 HLAVA {}", w)
        };
        per_page_elements.push(header_only_page(&line));
        per_page_bbox.push(page_bbox(800.0, 0.0));
    }

    let (banner_lines, banner_tokens, banner_image_hashes) =
        detect_banners(&per_page_elements, &per_page_bbox);

    // Each whole line is unique → no line-key banners.
    assert!(
        banner_lines.is_empty(),
        "expected no full-line banner matches, got {:?}",
        banner_lines
    );
    // But tokens recur on every page.
    assert!(
        banner_tokens.contains("HLAVA"),
        "expected HLAVA in banner_tokens, got {:?}",
        banner_tokens
    );
    assert!(
        banner_tokens.contains("PŘEDPIS"),
        "expected PŘEDPIS in banner_tokens, got {:?}",
        banner_tokens
    );

    // Verify strip_banners drops a mirrored variant. The trailing word
    // "lima" appears only once across the corpus so it shouldn't be
    // a banner token — use a NEW unique word so the corpus tokens
    // ("PŘEDPIS", "HLAVA") are the only ≥4-char banners; the trailing
    // unique word breaks token coverage. Pick a header line where all
    // tokens are banner tokens to assert stripping.
    let test_page = header_only_page("HLAVA PŘEDPIS");
    let cleaned = strip_banners(
        test_page,
        page_bbox(800.0, 0.0),
        &banner_lines,
        &banner_tokens,
        &banner_image_hashes,
    );
    assert!(
        cleaned.is_empty(),
        "alternating-header line should be stripped via token coverage; got {} elements",
        cleaned.len()
    );
}

#[test]
fn date_footer_token_stripped() {
    // 8 pages each carrying a date footer "25 . 12 . 2025" in the
    // bottom band. After normalization → "\\d . \\d . \\d", which the
    // line-key path already covers; assert it's stripped.
    let n_pages = 8;
    let mut per_page_elements: Vec<Vec<PageElement>> = Vec::new();
    let mut per_page_bbox: Vec<BBox> = Vec::new();
    for _ in 0..n_pages {
        // Place footer in the bottom 10% band (y_center near 10).
        let footer = text_at("25 . 12 . 2025", 250.0, 350.0, 10.0, 12.0);
        // Add a body item so the page isn't degenerate.
        let body = text_at("body content", 50.0, 550.0, 400.0, 12.0);
        per_page_elements.push(vec![footer, body]);
        per_page_bbox.push(page_bbox(800.0, 0.0));
    }

    let (banner_lines, banner_tokens, banner_image_hashes) =
        detect_banners(&per_page_elements, &per_page_bbox);
    // The normalized date line should be in banner_lines.
    assert!(
        banner_lines.iter().any(|s| s.contains("\\d")),
        "expected normalized date in banner_lines, got {:?}",
        banner_lines
    );

    let test_page = vec![
        text_at("25 . 12 . 2025", 250.0, 350.0, 10.0, 12.0),
        text_at("body content", 50.0, 550.0, 400.0, 12.0),
    ];
    let cleaned = strip_banners(
        test_page,
        page_bbox(800.0, 0.0),
        &banner_lines,
        &banner_tokens,
        &banner_image_hashes,
    );
    assert_eq!(
        cleaned.len(),
        1,
        "date footer should be stripped; expected only body element to remain"
    );
    if let PageElement::Text(t) = &cleaned[0] {
        assert_eq!(t.text, "body content");
    } else {
        panic!("expected text element after strip");
    }
}

/// Build an image PageElement at a given y-band, all pages sharing a
/// stable hash so the across-page recurrence check fires.
fn image_at(hash: &str, left: f32, right: f32, y_center: f32, height: f32) -> PageElement {
    let top = y_center + height / 2.0;
    let bottom = y_center - height / 2.0;
    PageElement::Image(ImageElement {
        figure: Figure {
            hash: hash.to_string(),
            mime_type: "image/png".to_string(),
            bytes: Vec::new(),
            name: None,
        },
        left,
        right,
        top,
        bottom,
    })
}

#[test]
fn image_banner_stripped_across_pages() {
    // 4 pages, each with the same logo image in the top band plus a
    // unique body text element. The detector must classify the logo
    // hash as a banner image and strip it on every page.
    let mut per_page_elements: Vec<Vec<PageElement>> = Vec::new();
    let mut per_page_bbox: Vec<BBox> = Vec::new();
    for i in 0..4 {
        let logo = image_at("logo-hash-0xDEADBEEF", 50.0, 150.0, 790.0, 20.0);
        let body = text_at(&format!("body content {}", i), 50.0, 550.0, 400.0, 12.0);
        per_page_elements.push(vec![logo, body]);
        per_page_bbox.push(page_bbox(800.0, 0.0));
    }

    let (banner_lines, banner_tokens, banner_image_hashes) =
        detect_banners(&per_page_elements, &per_page_bbox);
    assert!(
        banner_image_hashes.contains("logo-hash-0xDEADBEEF"),
        "expected logo hash in banner_image_hashes, got {:?}",
        banner_image_hashes
    );

    // Apply strip to page 0 and confirm the body survives, the logo
    // is gone.
    let page = std::mem::take(&mut per_page_elements[0]);
    let cleaned = strip_banners(
        page,
        page_bbox(800.0, 0.0),
        &banner_lines,
        &banner_tokens,
        &banner_image_hashes,
    );
    assert_eq!(cleaned.len(), 1, "logo image should be stripped");
    match &cleaned[0] {
        PageElement::Text(t) => assert_eq!(t.text, "body content 0"),
        other => panic!("expected text-only body, got {:?}", element_kind(other)),
    }
}

/// Test helper — describe an element kind for assertion messages.
fn element_kind(el: &PageElement) -> &'static str {
    match el {
        PageElement::Text(_) => "text",
        PageElement::Image(_) => "image",
    }
}

#[test]
fn body_image_not_stripped() {
    // Same hash but bbox sits in the middle of the page (not in band).
    // Across-page recurrence count for the band-restricted detector
    // is 0, so the image is never classified as a banner.
    let mut per_page_elements: Vec<Vec<PageElement>> = Vec::new();
    let mut per_page_bbox: Vec<BBox> = Vec::new();
    for i in 0..4 {
        let body_image = image_at("body-image-hash", 200.0, 400.0, 400.0, 100.0);
        let body = text_at(&format!("body content {}", i), 50.0, 550.0, 500.0, 12.0);
        per_page_elements.push(vec![body_image, body]);
        per_page_bbox.push(page_bbox(800.0, 0.0));
    }
    let (_lines, _tokens, banner_image_hashes) = detect_banners(&per_page_elements, &per_page_bbox);
    assert!(
        !banner_image_hashes.contains("body-image-hash"),
        "body-band image must NOT be classified as banner"
    );
}

#[test]
fn bottom_band_bare_digit_dropped() {
    let page = vec![
        text_at("body content", 50.0, 550.0, 400.0, 12.0),
        // Bare "7" near the bottom (y=10 of 800pt page → bottom 1.25 %).
        text_at("7", 295.0, 305.0, 10.0, 12.0),
    ];
    let cleaned = strip_bottom_band_bare_digits(page, page_bbox(800.0, 0.0));
    assert_eq!(cleaned.len(), 1, "bare-digit footer should be stripped");
    match &cleaned[0] {
        PageElement::Text(t) => assert_eq!(t.text, "body content"),
        other => panic!("expected body text, got {:?}", element_kind(other)),
    }
}

#[test]
fn bottom_band_non_digit_kept() {
    // Non-digit content in the bottom band stays put — the strip is
    // surgical, not blanket.
    let page = vec![
        text_at("body content", 50.0, 550.0, 400.0, 12.0),
        text_at("Confidential", 100.0, 500.0, 10.0, 12.0),
    ];
    let cleaned = strip_bottom_band_bare_digits(page, page_bbox(800.0, 0.0));
    assert_eq!(cleaned.len(), 2, "non-digit content must be preserved");
}

#[test]
fn body_band_bare_digit_kept() {
    // A bare digit in the body (not the footer band) is legitimate
    // content (e.g. an inline reference number) — must stay.
    let page = vec![
        text_at("Section", 50.0, 100.0, 400.0, 12.0),
        text_at("7", 110.0, 120.0, 400.0, 12.0),
    ];
    let cleaned = strip_bottom_band_bare_digits(page, page_bbox(800.0, 0.0));
    assert_eq!(cleaned.len(), 2, "body-band digit must be preserved");
}
