# pdf2md

Standalone PDF → Markdown extractor. Lifted from Nexial's `Infrastructure/src/storage/preprocess/pdf/`
into its own crate so Nexial and `f13/knowledge-base` can share a single implementation.

## Build / test

```sh
cargo build                                                  # lib + bin
cargo test --lib                                             # 64 unit tests, all synthetic (no PDF needed)
cargo run --bin pdf2md -- tests/fixtures/L14_cely.pdf        # smoke test (fixture is gitignored)
cargo check --no-default-features --lib                      # verify library doesn't pull clap
```

`libpdfium.so` must be on the runtime library path or `Pdfium::bind_to_system_library()` fails with
`PdfError::PdfiumBind`. Arch: `yay -S pdfium-binaries`.

## Public API surface (`src/lib.rs`)

- `extract(path).await -> Result<PdfDocument, PdfError>` — default config
- `extract_with_config(path, cfg).await` — pass a custom `ExtractConfig`
- `PdfDocument { markdown, figures, heading_levels }` — `figures` are `PdfFigure` with SHA-256 `hash`
- `ExtractConfig::image_emitter: Arc<dyn Fn(&str, &str) -> String + Send + Sync>` — the **only** config
  knob. Default emits `![alt](hash)`. Downstream callers (Nexial, F13) pass a closure that emits
  `<image hash="…">` directives instead.

The crate deliberately does NOT expose a `Preprocessor` trait — that abstraction belongs to the
host projects. Each project writes a ~30-line adapter implementing its own trait on top of
`extract_with_config`.

## Pipeline (read in this order)

1. `extract.rs::extract_pdf` — entry point. Loads PDF, runs two passes per page.
2. **Pass 1** — pdfium walks objects: text → `TextElement`, image → `extract_image_figure` →
   `ImageElement`, path → border/figure-region detection (`borders.rs`).
3. After pass 1, `banner.rs` strips repeating headers/footers across pages (must happen *before*
   font-histogram building, or banner text contaminates heading classification).
4. `headings.rs::HeadingClassifier::build` derives H1..H4 from font-size frequencies.
5. **Pass 2** — per page: `zone::segment` does recursive XY-cut → `Zone<PageElement>` tree;
   `render::emit_zone` walks the tree, dispatches to `render_leaf::emit_leaf` for atomic regions,
   and emits GFM tables for `Zone::Table` nodes.
6. `noise::strip_noise_headings` post-processes the final markdown.

## Critical invariants

- **pdfium-render is `!Send`**. All pdfium work runs on `tokio::task::spawn_blocking` (`lib.rs`).
  Don't try to thread `Page`/`Pdfium` across `.await`.
- **`Pdfium` is a `OnceLock` singleton** in `lib.rs`. First `extract*` call binds it; subsequent
  calls reuse. Don't construct `Pdfium` elsewhere.
- **Naming collision**: `pdfium_render::prelude::*` exports its own `PdfDocument`. `extract.rs`
  uses `use crate::PdfDocument as ExtractedDoc;` to avoid shadowing. If you add new files that
  import the pdfium prelude AND build `crate::PdfDocument` literals, alias it the same way.
- **`PageCtx::image_emitter` must be passed when constructing `PageCtx`** — it threads the
  caller's image-directive closure down to the two emission call sites (`render.rs`,
  `render_leaf.rs`). The only call site is in `extract.rs::extract_pdf` per page.

## Visibility convention

Internal-only items use `pub(super)` (= visible to the parent module). For files directly under
`src/`, `pub(super)` = `pub(crate)`. Zone submodule items use `pub(super)` to mean "visible to
`zone/mod.rs`". When in doubt, copy the visibility from the original Nexial source.

## Test wiring

Tests live in separate files included via `#[cfg(test)] #[path = "*_tests.rs"] mod tests;` at the
bottom of the unit they test:

- `banner.rs` ← `banner_tests.rs`
- `zone/segment.rs` ← `zone/segment_tests.rs`
- `zone/table.rs` ← `zone/table_tests.rs`

Do NOT add `mod *_tests;` declarations in `lib.rs` or `zone/mod.rs` — the `#[path]` attribute
already wires them.

## Syncing improvements from Nexial

While Nexial keeps its in-tree copy, improvements that land there should be lifted back here.
Workflow:

1. `diff -r /home/miksa/projects/nexial/Infrastructure/src/storage/preprocess/pdf/ src/`
   (ignoring the import-rewrite lines in `extract.rs`, `element.rs`, `text.rs`, `render.rs`,
   `render_leaf.rs`, `banner_tests.rs`, `zone/segment_tests.rs`, `zone/table_tests.rs`).
2. Cherry-pick the algorithmic changes; keep the retargeted imports.
3. Re-run `cargo test` and the smoke test.

Once both downstream projects migrate to this crate, this manual sync goes away.

## Out of scope (do not add)

- A `Preprocessor` trait, `ExtractedDoc` re-export, or any preprocessor-orchestration types —
  those belong to the host project, not here.
- OCR, page parallelism, or any new extraction features without explicit go-ahead — this crate
  is currently a faithful lift of Nexial's logic.
- A second binary or feature flags beyond `cli` — keep the surface minimal.
