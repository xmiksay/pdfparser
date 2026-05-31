//! Line / paragraph grouping types plus pdfium text/image helpers.

use std::collections::HashMap;
use std::io::Cursor;

use pdfium_render::prelude::*;
use sha2::{Digest, Sha256};

use crate::PdfError as PreprocessorError;
use crate::PdfFigure as Figure;
use super::element::{PageCtx, TextElement};
use super::headings::{FontSignature, HeadingClassifier};

// ---- line / paragraph types -------------------------------------------------

pub(super) struct TextRun {
    pub(super) text: String,
    pub(super) sig: FontSignature,
    pub(super) x_left: f32,
    pub(super) x_right: f32,
}

pub(super) struct Line {
    pub(super) y_center: f32,
    pub(super) y_top: f32,
    pub(super) y_bottom: f32,
    pub(super) x_start: f32,
    /// Largest right-edge among this line's runs — used by the X-gap guard.
    pub(super) x_right: f32,
    pub(super) runs: Vec<TextRun>,
}

impl Line {
    pub(super) fn avg_line_height(&self) -> f32 {
        (self.y_top - self.y_bottom).abs().max(1.0)
    }

    pub(super) fn push_run(&mut self, run: TextRun) {
        if run.x_right > self.x_right {
            self.x_right = run.x_right;
        }
        self.runs.push(run);
    }

    pub(super) fn update_y(&mut self, te: &TextElement) {
        self.y_center = (self.y_center + te.y_center()) / 2.0;
        if te.top > self.y_top {
            self.y_top = te.top;
        }
        if te.bottom < self.y_bottom {
            self.y_bottom = te.bottom;
        }
        if te.left < self.x_start {
            self.x_start = te.left;
        }
    }

    fn dominant_sig(&self) -> FontSignature {
        let mut counts: HashMap<&FontSignature, usize> = HashMap::new();
        for run in &self.runs {
            *counts.entry(&run.sig).or_insert(0) += run.text.len();
        }
        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(sig, _)| sig.clone())
            .unwrap_or_else(|| FontSignature::new(0.0, false, false))
    }

    fn render_inline(&self, classifier: &HeadingClassifier, space_threshold: f32) -> String {
        let runs: Vec<RunRef> = self
            .runs
            .iter()
            .map(|r| RunRef {
                text: r.text.as_str(),
                sig: &r.sig,
                x_left: r.x_left,
                x_right: r.x_right,
            })
            .collect();
        render_runs(&runs, classifier, space_threshold, true)
    }
}

// ---- run merging + inline emphasis rendering --------------------------------

/// Markdown emphasis class for a run, used to coalesce adjacent runs into a
/// single span instead of one span per run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Emphasis {
    None,
    Bold,
    Italic,
    BoldItalic,
}

/// A lightweight borrowed view over a renderable run, so [`render_runs`] can
/// serve `Line` runs (body text), table cells, and the row-major fallback.
pub(super) struct RunRef<'a> {
    pub(super) text: &'a str,
    pub(super) sig: &'a FontSignature,
    pub(super) x_left: f32,
    pub(super) x_right: f32,
}

/// Inline emphasis for a run: heading-sized runs render plain; body runs are
/// bold/italic only when they aren't larger than body text.
fn emphasis_of(sig: &FontSignature, classifier: &HeadingClassifier) -> Emphasis {
    if classifier.classify(sig).is_some() {
        return Emphasis::None;
    }
    let bold = sig.is_bold && sig.size_bucket <= classifier.body.size_bucket;
    let italic = sig.is_italic && sig.size_bucket <= classifier.body.size_bucket;
    match (bold, italic) {
        (true, true) => Emphasis::BoldItalic,
        (true, false) => Emphasis::Bold,
        (false, true) => Emphasis::Italic,
        (false, false) => Emphasis::None,
    }
}

fn wrap_emphasis(text: &str, e: Emphasis) -> String {
    match e {
        Emphasis::None => text.to_string(),
        Emphasis::Bold => format!("**{text}**"),
        Emphasis::Italic => format!("*{text}*"),
        Emphasis::BoldItalic => format!("***{text}***"),
    }
}

/// Render horizontally-ordered runs into inline markdown.
///
/// Adjacent runs that abut with no real gap belong to the same word and are
/// concatenated WITHOUT a space (`fyzi`+`cké` → `fyzické`); a space is inserted
/// only when the runs carry explicit whitespace or are separated by a gap of at
/// least `space_threshold`. Consecutive runs of the same emphasis class are
/// wrapped in a SINGLE `*`/`**`/`***` span instead of one span per run
/// (`*fyzi* *cké*` → `*fyzické*`). With `with_emphasis = false` every run renders
/// plain (table cells) — only the spacing fix applies.
pub(super) fn render_runs(
    runs: &[RunRef],
    classifier: &HeadingClassifier,
    space_threshold: f32,
    with_emphasis: bool,
) -> String {
    let mut out = String::new();
    let mut buf = String::new();
    let mut buf_class = Emphasis::None;
    let mut buf_open = false;
    let mut prev_right: Option<f32> = None;
    let mut prev_trailing_ws = false;

    for run in runs {
        let leading_ws = run.text.starts_with(|c: char| c.is_whitespace());
        let trailing_ws = run.text.ends_with(|c: char| c.is_whitespace());
        let text = run.text.trim();
        if text.is_empty() {
            // Whitespace-only run: remember the space but keep `prev_right` so
            // the gap to the next run still spans the blank.
            if run.text.chars().any(|c| c.is_whitespace()) {
                prev_trailing_ws = true;
            }
            continue;
        }

        let class = if with_emphasis {
            emphasis_of(run.sig, classifier)
        } else {
            Emphasis::None
        };
        let gap_space = prev_right.is_some_and(|pr| run.x_left - pr >= space_threshold);
        let need_space = prev_trailing_ws || leading_ws || gap_space;

        if !buf_open {
            buf_class = class;
            buf.push_str(text);
            buf_open = true;
        } else if class == buf_class {
            if need_space {
                buf.push(' ');
            }
            buf.push_str(text);
        } else {
            out.push_str(&wrap_emphasis(&buf, buf_class));
            buf.clear();
            if need_space {
                out.push(' ');
            }
            buf_class = class;
            buf.push_str(text);
        }

        prev_right = Some(run.x_right);
        prev_trailing_ws = trailing_ws;
    }

    if buf_open {
        out.push_str(&wrap_emphasis(&buf, buf_class));
    }
    out
}

pub(super) enum Paragraph {
    Heading { level: u8, text: String },
    Body { text: String },
}

pub(super) fn group_into_paragraphs(
    lines: &[Line],
    classifier: &HeadingClassifier,
    ctx: &PageCtx,
) -> Vec<Paragraph> {
    let mut paragraphs: Vec<Paragraph> = Vec::new();
    let mut body_lines: Vec<String> = Vec::new();

    let flush_body = |body_lines: &mut Vec<String>, paragraphs: &mut Vec<Paragraph>| {
        if !body_lines.is_empty() {
            let text = body_lines.join("\n");
            paragraphs.push(Paragraph::Body { text });
            body_lines.clear();
        }
    };

    // Right edge of the leaf zone (max x_right across all lines). Used to
    // detect "previous line ended significantly short of the column edge"
    // — a strong end-of-paragraph signal when first-line indent is absent.
    let zone_right = lines
        .iter()
        .map(|l| l.x_right)
        .fold(f32::NEG_INFINITY, f32::max);
    let prev_short_threshold = ctx.median_char_width * 6.0;

    for (idx, line) in lines.iter().enumerate() {
        let dom_sig = line.dominant_sig();
        if let Some(level) = classifier.classify(&dom_sig) {
            flush_body(&mut body_lines, &mut paragraphs);
            let text = line.render_inline(classifier, ctx.space_threshold);
            paragraphs.push(Paragraph::Heading { level, text });
        } else {
            // Body text. Decide whether to start a new paragraph.
            if let Some(prev_idx) = idx.checked_sub(1) {
                let prev = &lines[prev_idx];
                let y_gap = (prev.y_bottom - line.y_top).abs();
                // Was 1.2 — too strict; many PDFs separate paragraphs by
                // ~0.5–1.0 line heights. 0.5 catches typical paragraph
                // spacing without merging adjacent body lines.
                let y_threshold = line.avg_line_height() * 0.5;
                let indent_break = (line.x_start - prev.x_start) > ctx.indent_threshold;
                let prev_short =
                    zone_right.is_finite() && (zone_right - prev.x_right) > prev_short_threshold;
                if (y_gap > y_threshold || indent_break || prev_short) && !body_lines.is_empty() {
                    flush_body(&mut body_lines, &mut paragraphs);
                }
            }
            let rendered = line.render_inline(classifier, ctx.space_threshold);
            if !rendered.is_empty() {
                body_lines.push(rendered);
            }
        }
    }
    flush_body(&mut body_lines, &mut paragraphs);
    paragraphs
}

// ---- pdfium helpers (unchanged) ---------------------------------------------

pub(super) fn font_signature_from_text_object(text_obj: &PdfPageTextObject) -> FontSignature {
    let font = text_obj.font();
    let size_pts = text_obj.scaled_font_size().value;

    let is_bold = match font.weight() {
        Ok(w) => matches!(
            w,
            PdfFontWeight::Weight700Bold | PdfFontWeight::Weight800 | PdfFontWeight::Weight900
        ),
        Err(_) => false,
    } || font.is_bold_reenforced()
        || font.name().to_lowercase().contains("bold");

    let is_italic = font.is_italic()
        || font.name().to_lowercase().contains("italic")
        || font.name().to_lowercase().contains("oblique");

    FontSignature::new(size_pts, is_bold, is_italic)
}

pub(super) fn extract_image_figure(
    img_obj: &PdfPageImageObject,
) -> Result<Figure, PreprocessorError> {
    let dyn_image = img_obj
        .get_raw_image()
        .map_err(|e| PreprocessorError::PdfParse(format!("image extraction: {e}")))?;
    encode_figure_png(&dyn_image)
}

/// PNG-encode a [`DynamicImage`] and wrap it as a [`Figure`] (deduped downstream
/// by the SHA-256 `hash`). Shared by raster-image extraction
/// ([`extract_image_figure`]) and vector-figure rasterization, so both produce
/// byte-identical hashing/dedup behavior.
pub(super) fn encode_figure_png(
    dyn_image: &image::DynamicImage,
) -> Result<Figure, PreprocessorError> {
    let mut buf = Cursor::new(Vec::new());
    dyn_image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| PreprocessorError::PdfParse(format!("image encode: {e}")))?;

    let bytes = buf.into_inner();

    let mime_type = match image::guess_format(&bytes) {
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::Jpeg) => "image/jpeg",
        Ok(image::ImageFormat::Gif) => "image/gif",
        Ok(image::ImageFormat::WebP) => "image/webp",
        Ok(image::ImageFormat::Tiff) => "image/tiff",
        Ok(image::ImageFormat::Bmp) => "image/bmp",
        _ => "image/png",
    };

    let hash = hex::encode(Sha256::digest(&bytes));

    Ok(Figure {
        hash,
        mime_type: mime_type.to_string(),
        bytes,
        name: None,
    })
}
