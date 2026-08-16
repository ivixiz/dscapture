use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use dscapture::{ExtractionBackend, ParseOptions, parse_file, write_json_atomic};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BackendArg {
    Auto,
    Poppler,
    Native,
    Ocr,
}

impl From<BackendArg> for ExtractionBackend {
    fn from(value: BackendArg) -> Self {
        match value {
            BackendArg::Auto => Self::Auto,
            BackendArg::Poppler => Self::Poppler,
            BackendArg::Native => Self::Native,
            BackendArg::Ocr => Self::Ocr,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Parse an electronic-component PDF datasheet into JSON"
)]
struct Cli {
    /// Source PDF. Defaults to input.pdf.
    #[arg(default_value = "input.pdf")]
    input: PathBuf,

    /// Destination JSON. Defaults to output.json.
    #[arg(default_value = "output.json")]
    output: PathBuf,

    /// Text extraction strategy.
    #[arg(long, value_enum, default_value = "auto")]
    backend: BackendArg,

    /// Maximum number of pages to inspect. Use 0 for the complete document.
    #[arg(long, default_value_t = 128)]
    max_pages: usize,

    /// Maximum number of low-text pages sent to OCR in auto mode.
    #[arg(long, default_value_t = 12)]
    ocr_max_pages: usize,

    /// Tesseract language(s), for example eng or eng+deu.
    #[arg(long, default_value = "eng")]
    ocr_language: String,

    /// Rasterization resolution used by OCR.
    #[arg(long, default_value_t = 300)]
    ocr_dpi: u16,

    /// Disable automatic OCR of pages without a useful text layer.
    #[arg(long)]
    no_ocr: bool,

    /// Write compact rather than pretty-printed JSON.
    #[arg(long)]
    compact: bool,
}

fn run() -> dscapture::Result<()> {
    let cli = Cli::parse();
    let options = ParseOptions {
        backend: cli.backend.into(),
        max_pages: cli.max_pages,
        ocr_enabled: !cli.no_ocr,
        ocr_max_pages: cli.ocr_max_pages,
        ocr_language: cli.ocr_language,
        ocr_dpi: cli.ocr_dpi,
        pretty_json: !cli.compact,
        ..ParseOptions::default()
    };

    let datasheet = parse_file(&cli.input, &options)?;
    write_json_atomic(&cli.output, &datasheet, options.pretty_json)?;

    eprintln!(
        "parsed {} page(s): {} pin package(s), {} absolute and {} recommended rating row(s) -> {}",
        datasheet.metadata.pages_processed,
        datasheet.pin_configuration.len(),
        datasheet.absolute_maximum_ratings.len(),
        datasheet.recommended_operating_conditions.len(),
        cli.output.display()
    );
    for warning in &datasheet.metadata.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("dscapture: {error}");
        std::process::exit(1);
    }
}
