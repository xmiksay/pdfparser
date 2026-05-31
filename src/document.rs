use std::sync::Arc;

/// The result of extracting a PDF document into markdown.
#[derive(Debug, Clone)]
pub struct PdfDocument {
    /// Rendered markdown. Image references use the format produced by
    /// [`ExtractConfig::image_emitter`].
    pub markdown: String,
    /// Extracted figures (embedded raster images and rasterized vector regions),
    /// deduplicated by SHA-256 hash and referenced from the markdown.
    pub figures: Vec<PdfFigure>,
    /// Maximum ATX heading depth produced (0 = none, 1..=6).
    pub heading_levels: u8,
}

/// A single extracted image asset.
#[derive(Debug, Clone)]
pub struct PdfFigure {
    /// Lowercase 64-hex SHA-256 of `bytes`. Doubles as the figure's stable id.
    pub hash: String,
    /// MIME type of `bytes` (currently always `"image/png"`).
    pub mime_type: String,
    /// Raw image bytes.
    pub bytes: Vec<u8>,
    /// Optional stable filename hint (without directory). `None` lets the
    /// consumer pick a name.
    pub name: Option<String>,
}

/// Image emitter signature: `fn(hash, alt) -> markdown_fragment`.
pub type ImageEmitter = Arc<dyn Fn(&str, &str) -> String + Send + Sync>;

/// Caller-supplied knobs for [`crate::extract_with_config`].
#[derive(Clone)]
pub struct ExtractConfig {
    /// Called for each image inserted into the markdown stream. Receives
    /// `(figure_hash, alt_text)` and returns the markdown fragment to emit.
    ///
    /// Default: standard markdown — `![alt](hash)` (or `![](hash)` when `alt`
    /// is empty).
    pub image_emitter: ImageEmitter,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            image_emitter: default_image_emitter(),
        }
    }
}

pub(crate) fn default_image_emitter() -> ImageEmitter {
    Arc::new(|hash, alt| {
        if alt.is_empty() {
            format!("![]({hash})")
        } else {
            format!("![{alt}]({hash})")
        }
    })
}
