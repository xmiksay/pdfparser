#[derive(thiserror::Error, Debug)]
pub enum PdfError {
    #[error("pdfium binding failed: {0}")]
    PdfiumBind(String),
    #[error("pdf parse error: {0}")]
    PdfParse(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
