# Changelog

All notable changes to `pdf2md` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] — 2026-05-31

Initial release. Lifted from the in-tree PDF preprocessor in
`nexial/Infrastructure` so multiple downstream projects can share one
implementation.

### Added

- `pdf2md::extract(path).await -> Result<PdfDocument, PdfError>` — async
  entry point. Runs the blocking pdfium work on `tokio::task::spawn_blocking`.
- `pdf2md::extract_with_config(path, ExtractConfig)` for caller-supplied
  configuration.
- `ExtractConfig::image_emitter` — the (currently sole) configuration knob:
  an `Arc<dyn Fn(&str, &str) -> String + Send + Sync>` that converts a
  `(figure_hash, alt_text)` pair into the markdown fragment for inline
  image references. Defaults to standard markdown — `![alt](hash)`.
- `PdfDocument { markdown, figures, heading_levels }` and
  `PdfFigure { hash, mime_type, bytes, name }` as the public output types.
- `PdfError` with `PdfiumBind`, `PdfParse`, and `Io` variants.
- `pdf2md` CLI binary (`cargo install pdf2md`, gated behind the `cli`
  feature which is on by default). Writes markdown to stdout and optionally
  dumps each extracted figure as `<sha256>.png` into `--figures-dir`.

### Pipeline features (all carried over from the original implementation)

- Recursive XY-cut **zone segmentation** with table promotion.
- **Banner detection** — strips repeating page headers, footers, and
  bottom-band bare-digit page numbers.
- **Border / line detection** — recovers table grids and clusters path
  segments into vector-figure regions.
- **Heading classification** from font-size frequencies, weight, and
  italic flags.
- **Noise stripping** for junk headings and stray glyphs.
- **Embedded image extraction** — bitmap image objects re-encoded as PNG
  and SHA-256 hashed.
- **Vector-figure rasterization** — pages with line art / diagrams are
  rendered at ~180 DPI and the detected figure region cropped out.

[Unreleased]: https://github.com/xmiksay/pdfparser/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/xmiksay/pdfparser/releases/tag/v0.1.0
