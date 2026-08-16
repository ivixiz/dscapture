use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use dscapture::{
    ExtractionBackend, ParseOptions, SmokeOptions, parse_file, to_smoke_profile, write_json_atomic,
    write_smoke_atomic,
};

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
    about = "Parse an electronic-component PDF datasheet into JSON and a custom smoke profile"
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

    /// Custom smoke-profile path. Defaults to the JSON path with a .cir extension.
    #[arg(long, visible_alias = "netlist-output")]
    smoke_output: Option<PathBuf>,

    /// Do not generate the custom smoke profile.
    #[arg(long, visible_alias = "no-netlist")]
    no_smoke: bool,

    /// Override the device name following the .smoke directive.
    #[arg(long)]
    smoke_device: Option<String>,

    /// Project derating factor written to the profile.
    #[arg(long, default_value_t = 0.8)]
    derate: f64,

    /// Default TC_REF for current and power ratings when the table omits it.
    #[arg(long, default_value_t = 25.0)]
    reference_temperature: f64,
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
    let smoke_options = SmokeOptions {
        device_name: cli.smoke_device,
        derate: cli.derate,
        reference_temperature_c: cli.reference_temperature,
    };
    let smoke_path = cli
        .smoke_output
        .unwrap_or_else(|| cli.output.with_extension("cir"));
    if !cli.no_smoke {
        if smoke_path == cli.output {
            return Err(dscapture::Error::InvalidOptions(
                "JSON and smoke-profile output paths must be different".to_owned(),
            ));
        }
        // Validate smoke-profile settings before writing either output.
        let _ = to_smoke_profile(&datasheet, &smoke_options)?;
    }
    write_json_atomic(&cli.output, &datasheet, options.pretty_json)?;
    if !cli.no_smoke {
        write_smoke_atomic(&smoke_path, &datasheet, &smoke_options)?;
    }

    eprintln!(
        "parsed {} page(s): {} pin package(s), {} absolute, {} recommended, {} electrical, and {} thermal row(s) -> {}",
        datasheet.metadata.pages_processed,
        datasheet.pin_configuration.len(),
        datasheet.absolute_maximum_ratings.len(),
        datasheet.recommended_operating_conditions.len(),
        datasheet.electrical_characteristics.len(),
        datasheet.thermal_characteristics.len(),
        cli.output.display()
    );
    if !cli.no_smoke {
        eprintln!("custom smoke profile -> {}", smoke_path.display());
    }
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
