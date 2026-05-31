# pdf2md

[![crates.io](https://img.shields.io/crates/v/pdf2md.svg)](https://crates.io/crates/pdf2md)
[![docs.rs](https://img.shields.io/docsrs/pdf2md)](https://docs.rs/pdf2md)
[![CI](https://github.com/xmiksay/pdfparser/actions/workflows/ci.yml/badge.svg)](https://github.com/xmiksay/pdfparser/actions/workflows/ci.yml)
[![license](https://img.shields.io/crates/l/pdf2md.svg)](https://github.com/xmiksay/pdfparser/blob/main/LICENSE)

PDF → Markdown extractor for Rust. Wraps [pdfium-render](https://crates.io/crates/pdfium-render)
with a layout-analysis pipeline that recovers tables, columns, headings, and figures from
arbitrary PDF documents and emits clean Markdown.

## What it does

Given a PDF on disk, `pdf2md` returns:

- **Markdown** with ATX headings, paragraphs, and inline image references
- **Figures** — embedded raster images *and* rasterized vector regions, each
  deduplicated by SHA-256 hash
- **Heading depth** — the maximum ATX level produced (0..=6)

The pipeline includes:

- Recursive **XY-cut zone segmentation** with table promotion
- **Banner detection** — strips repeating page headers, footers, and stray page-number digits
- **Border/line detection** — recovers table grids and clusters path segments into figure regions
- **Heading classification** from font size, weight, and italic flags
- **Noise stripping** for junk headings and stray glyphs
- **Vector-figure rasterization** at ~180 DPI for diagrams without embedded bitmaps

## Requirements

`libpdfium` must be available at runtime. The crate uses dynamic binding via
`pdfium-render`'s `bind_to_system_library()`.

| OS | Install |
|---|---|
| Arch  | `yay -S pdfium-binaries` |
| Debian / Ubuntu | `sudo apt install libpdfium-dev` |
| macOS | `brew install pdfium` |
| Windows | grab `pdfium.dll` from [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) and place it next to your binary |

Minimum supported Rust version: **1.85** (edition 2024).

## Library use

The crate ships with default features (`cli`) on, which pulls in `clap` for the
binary. Library-only consumers should disable defaults:

```toml
[dependencies]
pdf2md = { version = "0.1", default-features = false }
```

Basic extraction:

```rust
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let doc = pdf2md::extract(Path::new("input.pdf")).await?;
    println!("{}", doc.markdown);
    eprintln!("extracted {} figures", doc.figures.len());
    Ok(())
}
```

Custom image-directive emission (e.g. a custom Markdown extension):

```rust
use std::sync::Arc;
use pdf2md::ExtractConfig;

let cfg = ExtractConfig {
    image_emitter: Arc::new(|hash, alt| {
        if alt.is_empty() {
            format!("<image hash=\"{hash}\">")
        } else {
            format!("<image hash=\"{hash}\" alt=\"{alt}\">")
        }
    }),
};
let doc = pdf2md::extract_with_config(Path::new("input.pdf"), cfg).await?;
```

## CLI

```sh
cargo install pdf2md
pdf2md input.pdf > out.md
pdf2md input.pdf --figures-dir ./figs > out.md
```

`--figures-dir` writes each extracted figure as `<sha256>.png` into the
target directory; without it, the CLI just reports the figure count to stderr.

## License

MIT — see [LICENSE](LICENSE).
