//! Internal element types and per-page layout context for the PDF pipeline.

use super::headings::FontSignature;
use super::zone::{BBox, Bounded};
use crate::ImageEmitter;
use crate::PdfFigure as Figure;

// ---- internal element types --------------------------------------------------

/// A collected text element from a PDF page.
pub(super) struct TextElement {
    pub(super) text: String,
    pub(super) sig: FontSignature,
    pub(super) left: f32,
    pub(super) right: f32,
    pub(super) top: f32,
    pub(super) bottom: f32,
}

impl TextElement {
    pub(super) fn y_center(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
    pub(super) fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }
}

/// A collected image element from a PDF page.
pub(super) struct ImageElement {
    pub(super) figure: Figure,
    pub(super) left: f32,
    pub(super) right: f32,
    pub(super) top: f32,
    pub(super) bottom: f32,
}

/// An element that can be either text or image, used for interleaving.
pub(super) enum PageElement {
    Text(TextElement),
    Image(ImageElement),
}

impl Bounded for PageElement {
    fn bbox(&self) -> BBox {
        match self {
            PageElement::Text(t) => BBox {
                left: t.left,
                right: t.right,
                top: t.top,
                bottom: t.bottom,
            },
            PageElement::Image(i) => BBox {
                left: i.left,
                right: i.right,
                top: i.top,
                bottom: i.bottom,
            },
        }
    }
    fn char_count(&self) -> usize {
        match self {
            PageElement::Text(t) => t.text.chars().count(),
            PageElement::Image(_) => 0,
        }
    }
}

// ---- per-page layout context (computed in pass 2) ---------------------------

/// Layout heuristics derived from a single page's elements. Drives gap
/// thresholds for zone segmentation and line/paragraph break decisions
/// inside leaf zones.
pub(super) struct PageCtx {
    /// Approximate width of one body-text character. Derived from
    /// `bbox.width / char_count` of body text elements.
    pub(super) median_char_width: f32,
    /// Height of a typical body line.
    pub(super) median_line_height: f32,
    /// Threshold beyond which an inter-run X gap forces a new line
    /// (defensive guard against zone segmentation missing a separator).
    pub(super) x_gap_line_break: f32,
    /// First-line-indent threshold (paragraph break).
    pub(super) indent_threshold: f32,
    /// Inter-run X gap at/above which a space is inserted between two runs.
    /// Below it, abutting runs are treated as one word (no space) — repairs
    /// pdfium splitting a single word into multiple runs (e.g. `fyzi`+`cké`).
    pub(super) space_threshold: f32,
    /// Caller-supplied markdown emitter for inline image references.
    pub(super) image_emitter: ImageEmitter,
}

impl PageCtx {
    pub(super) fn from_elements(elements: &[PageElement], image_emitter: ImageEmitter) -> Self {
        let mut char_widths: Vec<f32> = Vec::new();
        let mut line_heights: Vec<f32> = Vec::new();
        for el in elements {
            if let PageElement::Text(t) = el {
                let h = (t.top - t.bottom).abs();
                if h > 0.0 {
                    line_heights.push(h);
                }
                let n = t.text.chars().count();
                if n > 0 && t.width() > 0.0 {
                    char_widths.push(t.width() / n as f32);
                }
            }
        }

        let median_char_width = median(&mut char_widths).unwrap_or(5.0);
        let median_line_height = median(&mut line_heights).unwrap_or(10.0);

        Self {
            median_char_width,
            median_line_height,
            // 4 char widths' worth of gap between runs forces a line break.
            x_gap_line_break: median_char_width * 4.0,
            indent_threshold: median_char_width * 2.0,
            // A real inter-word space stretches well past a quarter char; tight
            // intra-word fragment gaps sit near zero, so this cleanly separates
            // "real space" from "pdfium split mid-word".
            space_threshold: median_char_width * 0.25,
            image_emitter,
        }
    }
}

pub(super) fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}
