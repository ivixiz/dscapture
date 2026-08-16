//! Datasheet-to-JSON parser.
//!
//! The crate exposes a native Rust API and a small C ABI. Text-based PDFs use
//! Poppler when available, with a pure-Rust fallback. Image-only pages can be
//! rasterized and recognized by Tesseract.

mod error;
mod extract;
mod ffi;
mod model;
mod netlist;
mod parser;

use std::fs;
use std::io::Write;
use std::path::Path;

pub use error::{Error, Result};
pub use extract::{ExtractionBackend, ParseOptions};
pub use model::{Datasheet, General, ParseMetadata, Pin, Rating};
pub use netlist::{NetlistOptions, SmokeOptions, to_ngspice_netlist, to_smoke_profile};

/// Parse a PDF file into a typed datasheet model.
pub fn parse_file(path: impl AsRef<Path>, options: &ParseOptions) -> Result<Datasheet> {
    let path = path.as_ref();
    let extracted = extract::extract(path, options)?;
    Ok(parser::parse_document(Some(path), extracted))
}

/// Parse an in-memory PDF. `filename_hint` improves part/package recognition.
pub fn parse_bytes(
    bytes: &[u8],
    filename_hint: Option<&str>,
    options: &ParseOptions,
) -> Result<Datasheet> {
    let mut temporary = tempfile::Builder::new()
        .prefix("dscapture-")
        .suffix(".pdf")
        .tempfile()
        .map_err(|source| Error::Io {
            path: std::env::temp_dir(),
            source,
        })?;
    temporary.write_all(bytes).map_err(|source| Error::Io {
        path: temporary.path().to_owned(),
        source,
    })?;
    temporary.flush().map_err(|source| Error::Io {
        path: temporary.path().to_owned(),
        source,
    })?;
    let extracted = extract::extract(temporary.path(), options)?;
    Ok(parser::parse_document(
        filename_hint.map(Path::new),
        extracted,
    ))
}

/// Serialize a parsed datasheet.
pub fn to_json(datasheet: &Datasheet, pretty: bool) -> Result<String> {
    if pretty {
        Ok(serde_json::to_string_pretty(datasheet)?)
    } else {
        Ok(serde_json::to_string(datasheet)?)
    }
}

/// Atomically replace the destination JSON after successful serialization.
pub fn write_json_atomic(
    path: impl AsRef<Path>,
    datasheet: &Datasheet,
    pretty: bool,
) -> Result<()> {
    let json = to_json(datasheet, pretty)?;
    write_text_atomic(path.as_ref(), &format!("{json}\n"))
}

/// Generate and atomically write a custom `.smoke` constraint profile.
pub fn write_smoke_atomic(
    path: impl AsRef<Path>,
    datasheet: &Datasheet,
    options: &SmokeOptions,
) -> Result<()> {
    let profile = to_smoke_profile(datasheet, options)?;
    write_text_atomic(path.as_ref(), &profile)
}

/// Backward-compatible writer name. The written content is now a custom
/// `.smoke` profile rather than a standalone NGSpice testbench.
pub fn write_netlist_atomic(
    path: impl AsRef<Path>,
    datasheet: &Datasheet,
    options: &NetlistOptions,
) -> Result<()> {
    write_smoke_atomic(path, datasheet, options)
}

fn write_text_atomic(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| Error::Io {
        path: parent.to_owned(),
        source,
    })?;
    temporary
        .write_all(contents.as_bytes())
        .map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })?;
    temporary.as_file().sync_all().map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    temporary.persist(path).map_err(|error| Error::Io {
        path: path.to_owned(),
        source: error.error,
    })?;
    if let Ok(directory) = fs::File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}
