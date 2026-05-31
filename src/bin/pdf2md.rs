//! CLI: `pdf2md <input.pdf> [--figures-dir DIR]`
//!
//! Extracts a PDF to markdown using the default config. Markdown goes to
//! stdout; figures (PNGs) are dumped to `--figures-dir` (one file per
//! figure, named `<sha256>.png`) when supplied, or just counted to stderr
//! otherwise.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "pdf2md",
    about = "Extract a PDF into markdown + figures using pdf2md.",
    version
)]
struct Cli {
    /// Input PDF file.
    input: PathBuf,

    /// If set, write each extracted figure as `<hash>.<ext>` into this directory.
    #[arg(long)]
    figures_dir: Option<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let doc = match pdf2md::extract(&cli.input).await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("pdf2md: {e}");
            return ExitCode::FAILURE;
        }
    };

    print!("{}", doc.markdown);
    if !doc.markdown.ends_with('\n') {
        println!();
    }

    if let Some(dir) = &cli.figures_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("pdf2md: create figures dir {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
        for fig in &doc.figures {
            let ext = match fig.mime_type.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                _ => "bin",
            };
            let path = dir.join(format!("{}.{ext}", fig.hash));
            if let Err(e) = std::fs::write(&path, &fig.bytes) {
                eprintln!("pdf2md: write {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
        eprintln!(
            "pdf2md: wrote {} figures to {}",
            doc.figures.len(),
            dir.display()
        );
    } else {
        eprintln!("pdf2md: extracted {} figures", doc.figures.len());
    }

    ExitCode::SUCCESS
}
