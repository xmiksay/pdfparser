//! PDF → Markdown extractor.
//!
//! See crate-level [`extract`] / [`extract_with_config`] for the entry points.

use std::path::Path;
use std::sync::OnceLock;

use pdfium_render::prelude::*;

mod banner;
mod borders;
mod document;
mod element;
mod error;
mod extract;
mod headings;
mod noise;
mod render;
mod render_leaf;
mod text;
mod zone;

pub use document::{ExtractConfig, ImageEmitter, PdfDocument, PdfFigure};
pub use error::PdfError;

/// Lazy-initialized pdfium binding. Fails gracefully if `libpdfium` is absent.
static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

pub(crate) fn get_pdfium() -> Result<&'static Pdfium, PdfError> {
    let result = PDFIUM.get_or_init(|| {
        Pdfium::bind_to_system_library()
            .map(Pdfium::new)
            .map_err(|e| format!("{e}"))
    });
    match result {
        Ok(p) => Ok(p),
        Err(e) => Err(PdfError::PdfiumBind(e.clone())),
    }
}

/// Extract a PDF file into markdown + figures using the default config.
///
/// Pdfium-render is not `Send`, so the blocking work runs on
/// [`tokio::task::spawn_blocking`].
pub async fn extract(input: &Path) -> Result<PdfDocument, PdfError> {
    extract_with_config(input, ExtractConfig::default()).await
}

/// Like [`extract`], with a caller-supplied [`ExtractConfig`].
pub async fn extract_with_config(
    input: &Path,
    config: ExtractConfig,
) -> Result<PdfDocument, PdfError> {
    let input = input.to_path_buf();
    tokio::task::spawn_blocking(move || extract::extract_pdf(&input, &config))
        .await
        .map_err(|e| PdfError::PdfParse(format!("task join error: {e}")))?
}
