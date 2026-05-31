//! The `extract_pdf` pipeline plus vector-figure region detection helpers.

use std::path::Path;
use std::time::Instant;

use pdfium_render::prelude::*;

use tracing::{debug, info, warn};

use super::banner::{
    BANNER_BAND_FRACTION, bbox_of_elements, detect_banners, strip_banners,
    strip_bottom_band_bare_digits,
};
use super::borders::{
    HLine, LINE_MIN_LENGTH, Line as BorderLine, TableRegion, VLine, classify_segment,
    detect_figure_regions, detect_table_regions,
};
use crate::PdfError as PreprocessorError;
use crate::PdfDocument as ExtractedDoc;
use crate::{ExtractConfig, PdfFigure as Figure, get_pdfium};
use super::element::{ImageElement, PageCtx, PageElement, TextElement};
use super::headings::{FontSignature, HeadingClassifier};
use super::noise::strip_noise_headings;
use super::render::emit_zone;
use super::text::{encode_figure_png, extract_image_figure, font_signature_from_text_object};
use super::zone::{BBox, SegmentParams};

// ---- vector-figure rasterization --------------------------------------------

/// Page scale factor used when rasterizing a vector-figure region (~180 DPI at
/// the default 72 DPI page space). High enough for crisp diagrams, low enough to
/// keep the per-page render cheap.
const FIGURE_RENDER_SCALE: f32 = 2.5;

/// Padding (PDF pts) added around a figure region before cropping, so strokes at
/// the very edge aren't clipped. Clamped to the page box.
const FIGURE_PAD_PTS: f32 = 4.0;

/// A figure region is dropped if it covers at least this fraction of the page —
/// it's almost certainly a page background or watermark, not a figure.
const FIGURE_MAX_PAGE_FRACTION: f32 = 0.85;

/// Render `page` once at [`FIGURE_RENDER_SCALE`], then crop each region (in PDF
/// points, y-up) out of the bitmap and PNG-encode it as a [`Figure`]. Returns
/// `(region, figure)` pairs in input order. Coordinate mapping uses
/// [`PdfPage::points_to_pixels`] with the *same* render config so the y-flip and
/// any page rotation are handled by pdfium.
fn rasterize_figures(
    page: &PdfPage,
    regions: &[BBox],
) -> Result<Vec<(BBox, Figure)>, PreprocessorError> {
    let config = PdfRenderConfig::new().scale_page_by_factor(FIGURE_RENDER_SCALE);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| PreprocessorError::PdfParse(format!("page render: {e}")))?;
    let full = bitmap
        .as_image()
        .map_err(|e| PreprocessorError::PdfParse(format!("bitmap to image: {e}")))?;
    let (bw, bh) = (full.width(), full.height());

    let page_w = page.width().value;
    let page_h = page.height().value;

    let mut out = Vec::with_capacity(regions.len());
    for region in regions {
        // Pad slightly, clamped to the page box.
        let left = (region.left - FIGURE_PAD_PTS).max(0.0);
        let right = (region.right + FIGURE_PAD_PTS).min(page_w);
        let top = (region.top + FIGURE_PAD_PTS).min(page_h);
        let bottom = (region.bottom - FIGURE_PAD_PTS).max(0.0);

        let (x0, y0) = page
            .points_to_pixels(PdfPoints::new(left), PdfPoints::new(top), &config)
            .map_err(|e| PreprocessorError::PdfParse(format!("points_to_pixels: {e}")))?;
        let (x1, y1) = page
            .points_to_pixels(PdfPoints::new(right), PdfPoints::new(bottom), &config)
            .map_err(|e| PreprocessorError::PdfParse(format!("points_to_pixels: {e}")))?;

        // Device-space corners; y is flipped relative to PDF points, so don't
        // assume which corner is larger — take min + abs.
        let px = x0.min(x1).max(0) as u32;
        let py = y0.min(y1).max(0) as u32;
        let pw = (x0 - x1).unsigned_abs().min(bw.saturating_sub(px));
        let ph = (y0 - y1).unsigned_abs().min(bh.saturating_sub(py));
        if pw == 0 || ph == 0 {
            continue;
        }

        let crop = full.crop_imm(px, py, pw, ph);
        let figure = encode_figure_png(&crop)?;
        out.push((*region, figure));
    }
    Ok(out)
}

// ---- main entry point --------------------------------------------------------

pub(super) fn extract_pdf(
    path: &Path,
    config: &ExtractConfig,
) -> Result<ExtractedDoc, PreprocessorError> {
    let total_start = Instant::now();
    info!(path = %path.display(), "PDF preprocess: starting");

    let bind_start = Instant::now();
    let pdfium = get_pdfium()?;
    debug!(
        elapsed_ms = bind_start.elapsed().as_millis() as u64,
        "PDF preprocess: pdfium binding ready"
    );

    let load_start = Instant::now();
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| PreprocessorError::PdfParse(format!("{e}")))?;
    let pages = doc.pages();
    let page_count = pages.len();
    info!(
        pages = page_count,
        elapsed_ms = load_start.elapsed().as_millis() as u64,
        "PDF preprocess: document loaded"
    );

    // ----- PASS 1: per-page extraction (text + image elements only) -----
    // Font samples are intentionally NOT collected here — banner text would
    // contaminate the histogram (e.g. a "PŘEDPIS L14" header repeated 296×
    // pushes real heading sizes below the rare-size threshold). Banners are
    // stripped first; samples are gathered afterwards from clean elements.
    let pass1_start = Instant::now();
    let mut per_page_elements: Vec<Vec<PageElement>> = Vec::with_capacity(page_count as usize);
    let mut per_page_bbox: Vec<BBox> = Vec::with_capacity(page_count as usize);
    let mut per_page_table_regions: Vec<Vec<TableRegion>> = Vec::with_capacity(page_count as usize);
    let mut images_failed: usize = 0;
    let mut total_table_regions: usize = 0;

    for page_idx in 0..page_count {
        let page = pages
            .get(page_idx)
            .map_err(|e| PreprocessorError::PdfParse(format!("page {page_idx}: {e}")))?;

        let mut elements: Vec<PageElement> = Vec::new();
        let mut h_lines: Vec<HLine> = Vec::new();
        let mut v_lines: Vec<VLine> = Vec::new();
        let mut path_bboxes: Vec<BBox> = Vec::new();
        for obj in page.objects().iter() {
            match &obj {
                PdfPageObject::Text(text_obj) => {
                    let text = text_obj.text();
                    if text.is_empty() {
                        continue;
                    }
                    let sig = font_signature_from_text_object(text_obj);
                    if let Ok(bounds) = obj.bounds() {
                        elements.push(PageElement::Text(TextElement {
                            text,
                            sig,
                            left: bounds.left().value,
                            right: bounds.right().value,
                            top: bounds.top().value,
                            bottom: bounds.bottom().value,
                        }));
                    }
                }
                PdfPageObject::Image(img_obj) => match extract_image_figure(img_obj) {
                    Ok(figure) => {
                        if let Ok(bounds) = obj.bounds() {
                            elements.push(PageElement::Image(ImageElement {
                                figure,
                                left: bounds.left().value,
                                right: bounds.right().value,
                                top: bounds.top().value,
                                bottom: bounds.bottom().value,
                            }));
                        }
                    }
                    Err(e) => {
                        images_failed += 1;
                        warn!("failed to extract image on page {page_idx}: {e}");
                    }
                },
                PdfPageObject::Path(path_obj) => {
                    // Walk segments: pdfium's path segment iterator gives
                    // each segment's destination point and type. Track the
                    // current pen position to materialize each LineTo as a
                    // 2-point segment, then classify as H / V / other.
                    //
                    // While walking, also decide whether this path is a *pure
                    // rule* — made only of axis-aligned H/V line segments (a
                    // table border, separator, or hairline rectangle) — versus
                    // real drawing geometry (a meaningful diagonal or a
                    // curve). Non-rule paths feed `detect_figure_regions`.
                    let mut current: Option<(f32, f32)> = None;
                    let mut sub_start: Option<(f32, f32)> = None;
                    let mut is_pure_rule = true;
                    for seg in path_obj.segments().iter() {
                        let (px, py) = (seg.x().value, seg.y().value);
                        match seg.segment_type() {
                            PdfPathSegmentType::MoveTo => {
                                current = Some((px, py));
                                sub_start = Some((px, py));
                            }
                            PdfPathSegmentType::LineTo => {
                                if let Some((cx, cy)) = current {
                                    match classify_segment(cx, cy, px, py) {
                                        Some(BorderLine::H(h)) => h_lines.push(h),
                                        Some(BorderLine::V(v)) => v_lines.push(v),
                                        None => {
                                            // A non-axis line of meaningful
                                            // length is real drawing geometry.
                                            let dx = px - cx;
                                            let dy = py - cy;
                                            if (dx * dx + dy * dy).sqrt() >= LINE_MIN_LENGTH {
                                                is_pure_rule = false;
                                            }
                                        }
                                    }
                                }
                                current = Some((px, py));
                                // A "close" line back to the sub-path start
                                // is implicit in some PDFs; explicit close
                                // is reported by `is_close()` on the segment.
                                if seg.is_close()
                                    && let (Some((cx, cy)), Some((sx, sy))) = (current, sub_start)
                                    && let Some(line) = classify_segment(cx, cy, sx, sy)
                                {
                                    match line {
                                        BorderLine::H(h) => h_lines.push(h),
                                        BorderLine::V(v) => v_lines.push(v),
                                    }
                                }
                            }
                            // Bezier / other curve segments are real drawing.
                            _ => {
                                is_pure_rule = false;
                                current = Some((px, py));
                            }
                        }
                    }

                    // Collect the bbox of real drawing paths so vector
                    // figures (diagrams, charts, vector logos) can be
                    // clustered and rasterized later. Pure H/V rules are left
                    // to table-border detection only.
                    if !is_pure_rule && let Ok(bounds) = obj.bounds() {
                        path_bboxes.push(BBox {
                            left: bounds.left().value,
                            right: bounds.right().value,
                            top: bounds.top().value,
                            bottom: bounds.bottom().value,
                        });
                    }
                }
                _ => {}
            }
        }

        let page_bbox = bbox_of_elements(&elements);
        let table_regions = detect_table_regions(&h_lines, &v_lines);
        total_table_regions += table_regions.len();

        // ----- Vector-figure capture -----
        // Cluster non-rule path geometry into figure regions, discard the ones
        // that are header/footer logos or page backgrounds, then rasterize each
        // survivor into a PageElement::Image so it rides the same figure
        // pipeline (harvest → <image hash> → asset upload) as embedded raster
        // images.
        //
        // We deliberately do NOT exclude regions that overlap detected table
        // regions: figures are built only from *non-rule* paths, which real
        // data tables don't contain (a table is text + H/V rules, and even
        // colored cell fills are axis rectangles = pure rules). Worse, a
        // figure's own axis-aligned edges (e.g. a bordered diagram, square
        // symbols) feed table detection and would form a fake table coextensive
        // with the figure — suppressing the very figure we want to keep.
        let figure_regions: Vec<BBox> = {
            let page_w = page.width().value;
            let page_h = page.height().value;
            let page_area = (page_w * page_h).max(1.0);
            // Banner bands are measured against the physical page (y-up,
            // 0..=page_h), not the text-content bbox: a page whose only text is
            // a short caption would otherwise yield a tiny band and wrongly
            // drop a large body diagram.
            let band = page_h * BANNER_BAND_FRACTION;
            let top_cutoff = page_h - band;
            let bottom_cutoff = band;
            detect_figure_regions(&path_bboxes)
                .into_iter()
                .filter(|r| {
                    let area = ((r.right - r.left) * (r.top - r.bottom)).max(0.0);
                    // Skip page-background / watermark vectors.
                    if area >= page_area * FIGURE_MAX_PAGE_FRACTION {
                        return false;
                    }
                    // Skip header/footer logos sitting in the banner bands.
                    let y_center = (r.top + r.bottom) / 2.0;
                    y_center < top_cutoff && y_center > bottom_cutoff
                })
                .collect()
        };

        if !figure_regions.is_empty() {
            match rasterize_figures(&page, &figure_regions) {
                Ok(figs) => {
                    for (region, figure) in figs {
                        elements.push(PageElement::Image(ImageElement {
                            figure,
                            left: region.left,
                            right: region.right,
                            top: region.top,
                            bottom: region.bottom,
                        }));
                    }
                }
                Err(e) => {
                    images_failed += 1;
                    warn!("failed to rasterize vector figures on page {page_idx}: {e}");
                }
            }
        }

        per_page_elements.push(elements);
        per_page_bbox.push(page_bbox);
        per_page_table_regions.push(table_regions);
    }
    debug!(
        table_regions = total_table_regions,
        elapsed_ms = pass1_start.elapsed().as_millis() as u64,
        "PDF preprocess: pass 1 (extraction) complete"
    );

    // ----- Header/footer detection + strip (BEFORE classifier build) -----
    let banner_start = Instant::now();
    let (banner_lines, banner_tokens, banner_image_hashes) =
        detect_banners(&per_page_elements, &per_page_bbox);
    let mut total_stripped: usize = 0;
    let needs_strip =
        !banner_lines.is_empty() || !banner_tokens.is_empty() || !banner_image_hashes.is_empty();
    if needs_strip {
        for (page_idx, elements) in per_page_elements.iter_mut().enumerate() {
            let bbox = per_page_bbox[page_idx];
            let before = elements.len();
            let cleaned = strip_banners(
                std::mem::take(elements),
                bbox,
                &banner_lines,
                &banner_tokens,
                &banner_image_hashes,
            );
            total_stripped += before - cleaned.len();
            *elements = cleaned;
        }
    }
    // Unconditional bottom-band bare-digit footer strip. Independent of the
    // ≥3-page threshold because a stand-alone short numeric run in the
    // bottom 8 % of a page is essentially always a page-number footer.
    let mut footer_digits_stripped: usize = 0;
    for (page_idx, elements) in per_page_elements.iter_mut().enumerate() {
        let bbox = per_page_bbox[page_idx];
        let before = elements.len();
        let cleaned = strip_bottom_band_bare_digits(std::mem::take(elements), bbox);
        footer_digits_stripped += before - cleaned.len();
        *elements = cleaned;
    }
    debug!(
        banner_lines = banner_lines.len(),
        banner_tokens = banner_tokens.len(),
        banner_images = banner_image_hashes.len(),
        stripped = total_stripped,
        footer_digits_stripped,
        elapsed_ms = banner_start.elapsed().as_millis() as u64,
        "PDF preprocess: banner detection + strip complete"
    );

    // ----- Build font histogram from CLEAN per-page elements -----
    let mut font_samples: Vec<(FontSignature, &str)> = Vec::new();
    for elements in &per_page_elements {
        for el in elements {
            if let PageElement::Text(t) = el {
                font_samples.push((t.sig.clone(), t.text.as_str()));
            }
        }
    }

    if font_samples.is_empty() {
        info!("PDF preprocess: no text objects after banner strip; returning empty markdown");
        return Ok(ExtractedDoc {
            markdown: String::new(),
            figures: Vec::new(),
            heading_levels: 0,
        });
    }

    let classifier = HeadingClassifier::build(font_samples.into_iter());

    // ----- PASS 2: per page, segment into zones, emit markdown -----
    let pass2_start = Instant::now();
    let mut all_figures: Vec<Figure> = Vec::new();
    let mut page_markdowns: Vec<String> = Vec::new();
    let mut max_heading_level: u8 = 0;

    for (page_idx, elements) in per_page_elements.into_iter().enumerate() {
        let page_start = Instant::now();
        // Banners already stripped — no per-page strip needed.

        // Collect figures from this page.
        for el in &elements {
            if let PageElement::Image(im) = el {
                all_figures.push(im.figure.clone());
            }
        }

        let ctx = PageCtx::from_elements(&elements, config.image_emitter.clone());
        let page_table_regions: Vec<TableRegion> = per_page_table_regions
            .get(page_idx)
            .cloned()
            .unwrap_or_default();
        debug!(
            page = page_idx + 1,
            table_regions = page_table_regions.len(),
            "page table-border regions detected"
        );
        let params = SegmentParams {
            min_v_gap: (ctx.median_char_width * 3.0).max(6.0),
            min_h_gap: (ctx.median_line_height * 1.2).max(6.0),
            min_zone_items: 2,
            table_regions: page_table_regions,
        };
        let zone = super::zone::segment(elements, &params);
        let leaf_count = zone.leaf_count();
        let item_count = zone.item_count();

        let page_md = emit_zone(&zone, &classifier, &mut max_heading_level, &ctx);
        debug!(
            page = page_idx + 1,
            of = page_count,
            items = item_count,
            zones = leaf_count,
            md_bytes = page_md.len(),
            elapsed_ms = page_start.elapsed().as_millis() as u64,
            "PDF preprocess: page processed"
        );
        page_markdowns.push(page_md);
    }
    debug!(
        figures = all_figures.len(),
        figures_failed = images_failed,
        elapsed_ms = pass2_start.elapsed().as_millis() as u64,
        "PDF preprocess: pass 2 (segmentation + emit) complete"
    );

    let markdown = page_markdowns.join("\n\n");
    let markdown = strip_noise_headings(&markdown);
    info!(
        pages = page_count,
        figures = all_figures.len(),
        markdown_bytes = markdown.len(),
        heading_levels = max_heading_level,
        elapsed_ms = total_start.elapsed().as_millis() as u64,
        "PDF preprocess: done"
    );

    Ok(ExtractedDoc {
        markdown,
        figures: all_figures,
        heading_levels: max_heading_level,
    })
}
