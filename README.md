# pdf2md

PDF → Markdown extractor for Rust. Wraps [pdfium-render](https://crates.io/crates/pdfium-render)
with a layout-analysis pipeline that recovers tables, columns, headings, and figures from
arbitrary PDF documents and emits clean Markdown.

## What it does

Given a PDF on disk, `pdf2md` returns:

- **Markdown** with ATX headings, paragraphs, and inline image references
- **Figures** — raster images and rasterized vector regions, deduplicated by SHA-256
- **Heading depth** — the maximum ATX level produced (0..=6)

The pipeline includes:

- Recursive **XY-cut zone segmentation** with table detection
- **Banner detection** — strips repeating page headers and footers
- **Border/line detection** — recovers table grids and figure regions
- **Heading classification** from font size, weight, and italic flags
- **Noise stripping** for junk headings and stray glyphs
- **Vector-figure rasterization** at 2.5× scale for diagrams without embedded bitmaps

## Requirements

`libpdfium` must be available at runtime. On Arch:

```sh
yay -S pdfium-binaries     # or build from source
```

On Debian/Ubuntu:

```sh
sudo apt install libpdfium-dev
```

## Library use

```rust
use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let doc = pdf2md::extract(Path::new("input.pdf")).await?;
    println!("{}", doc.markdown);
    eprintln!("extracted {} figures", doc.figures.len());
    Ok(())
}
```

For custom image-directive emission (e.g. a custom Markdown extension):

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
cargo install --path .
pdf2md input.pdf > out.md
pdf2md input.pdf --figures-dir ./figs > out.md
```
