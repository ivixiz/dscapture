use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use lopdf::{Document, Object, decode_text_string};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionBackend {
    #[default]
    Auto,
    Poppler,
    Native,
    Ocr,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ParseOptions {
    pub backend: ExtractionBackend,
    /// Zero means all pages. Large documents are intentionally bounded by default.
    pub max_pages: usize,
    pub ocr_enabled: bool,
    pub ocr_max_pages: usize,
    pub ocr_language: String,
    pub ocr_dpi: u16,
    pub minimum_text_characters_per_page: usize,
    pub pretty_json: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            backend: ExtractionBackend::Auto,
            max_pages: 128,
            ocr_enabled: true,
            ocr_max_pages: 12,
            ocr_language: "eng".to_owned(),
            ocr_dpi: 300,
            minimum_text_characters_per_page: 80,
            pretty_json: true,
        }
    }
}

impl ParseOptions {
    pub(crate) fn validate(&self) -> Result<()> {
        if !(100..=600).contains(&self.ocr_dpi) {
            return Err(Error::InvalidOptions(
                "ocr_dpi must be between 100 and 600".to_owned(),
            ));
        }
        if self.ocr_language.trim().is_empty() {
            return Err(Error::InvalidOptions(
                "ocr_language must not be empty".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PdfMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug)]
pub(crate) struct ExtractedDocument {
    pub text: String,
    pub total_pages: usize,
    pub pages_processed: usize,
    pub method: String,
    pub ocr_pages: Vec<usize>,
    pub warnings: Vec<String>,
    pub pdf_metadata: PdfMetadata,
}

pub(crate) fn extract(path: &Path, options: &ParseOptions) -> Result<ExtractedDocument> {
    options.validate()?;
    validate_input(path)?;

    let (total_pages, pdf_metadata, inspection_warning) =
        if options.backend == ExtractionBackend::Native {
            inspect_pdf_native(path)
        } else {
            match inspect_pdf_poppler(path) {
                Ok((pages, metadata)) => (pages, metadata, None),
                Err(error) => {
                    let (pages, metadata, native_warning) = inspect_pdf_native(path);
                    let warning = native_warning.unwrap_or_else(|| {
                        format!("pdfinfo unavailable or failed ({error}); metadata read natively")
                    });
                    (pages, metadata, Some(warning))
                }
            }
        };
    let pages_processed = match (options.max_pages, total_pages) {
        (0, count) => count,
        (limit, 0) => limit,
        (limit, count) => limit.min(count),
    }
    .max(1);

    let mut warnings = Vec::new();
    if let Some(warning) = inspection_warning {
        warnings.push(warning);
    }
    if options.max_pages != 0 && total_pages > pages_processed {
        warnings.push(format!(
            "document has {total_pages} pages; only the first {pages_processed} were inspected"
        ));
    }

    let (mut pages, mut method) = match options.backend {
        ExtractionBackend::Auto => match extract_poppler(path, pages_processed) {
            Ok(text) => (
                split_pages(&text, pages_processed),
                "poppler-layout".to_owned(),
            ),
            Err(error) => {
                warnings.push(format!(
                    "Poppler unavailable or failed ({error}); using native PDF extraction"
                ));
                (
                    split_pages(&extract_native(path, pages_processed)?, pages_processed),
                    "lopdf-native".to_owned(),
                )
            }
        },
        ExtractionBackend::Poppler => (
            split_pages(&extract_poppler(path, pages_processed)?, pages_processed),
            "poppler-layout".to_owned(),
        ),
        ExtractionBackend::Native => (
            split_pages(&extract_native(path, pages_processed)?, pages_processed),
            "lopdf-native".to_owned(),
        ),
        ExtractionBackend::Ocr => (
            vec![String::new(); pages_processed],
            "tesseract-ocr".to_owned(),
        ),
    };

    let mut ocr_pages = Vec::new();
    let should_consider_ocr = options.backend == ExtractionBackend::Ocr
        || (options.backend == ExtractionBackend::Auto && options.ocr_enabled);
    if should_consider_ocr {
        let candidates: Vec<usize> = if options.backend == ExtractionBackend::Ocr {
            (1..=pages_processed).collect()
        } else {
            pages
                .iter()
                .enumerate()
                .filter(|(_, text)| {
                    useful_character_count(text) < options.minimum_text_characters_per_page
                })
                .map(|(index, _)| index + 1)
                .collect()
        };

        let allowed = candidates.len().min(options.ocr_max_pages);
        if candidates.len() > allowed {
            warnings.push(format!(
                "{} low-text page(s) were not OCRed because ocr_max_pages is {}",
                candidates.len() - allowed,
                options.ocr_max_pages
            ));
        }

        for page_number in candidates.into_iter().take(allowed) {
            match ocr_page(path, page_number, options) {
                Ok(text)
                    if useful_character_count(&text)
                        > useful_character_count(&pages[page_number - 1]) =>
                {
                    pages[page_number - 1] = text;
                    ocr_pages.push(page_number);
                }
                Ok(_) => warnings.push(format!(
                    "OCR produced no useful additional text for page {page_number}"
                )),
                Err(error) if options.backend == ExtractionBackend::Ocr => return Err(error),
                Err(error) => {
                    warnings.push(format!("OCR failed for page {page_number}: {error}"));
                    break;
                }
            }
        }
        if !ocr_pages.is_empty() {
            #[cfg(feature = "opencv-preprocessing")]
            if method == "tesseract-ocr" {
                method = "opencv-preprocessing+tesseract-ocr".to_owned();
            } else {
                method.push_str("+opencv-preprocessing+tesseract-ocr");
            }
            #[cfg(not(feature = "opencv-preprocessing"))]
            {
                if method != "tesseract-ocr" {
                    method.push_str("+tesseract-ocr");
                }
                warnings.push(
                    "OCR ran without OpenCV preprocessing; enable feature `opencv-preprocessing` for noisy scans"
                        .to_owned(),
                );
            }
        }
    }

    let text = normalize_text(&pages.join("\u{c}"));
    if useful_character_count(&text) < 20 {
        return Err(Error::NoText);
    }

    Ok(ExtractedDocument {
        text,
        total_pages: total_pages.max(pages_processed),
        pages_processed,
        method,
        ocr_pages,
        warnings,
        pdf_metadata,
    })
}

fn validate_input(path: &Path) -> Result<()> {
    let metadata = fs::metadata(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            Error::InputNotFound(path.to_owned())
        } else {
            Error::Io {
                path: path.to_owned(),
                source,
            }
        }
    })?;
    if !metadata.is_file() {
        return Err(Error::InvalidInput(path.to_owned()));
    }
    let mut file = File::open(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut magic = [0u8; 5];
    file.read_exact(&mut magic).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    if &magic != b"%PDF-" {
        return Err(Error::InvalidInput(path.to_owned()));
    }
    Ok(())
}

fn inspect_pdf_poppler(path: &Path) -> Result<(usize, PdfMetadata)> {
    let output = Command::new("pdfinfo")
        .arg(path)
        .output()
        .map_err(|error| external_io_error("pdfinfo", error))?;
    let text = command_stdout("pdfinfo", output)?;
    let mut pages = 0usize;
    let mut metadata = PdfMetadata::default();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("Pages:") {
            pages = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("Title:") {
            let value = value.trim();
            if !value.is_empty() {
                metadata.title = Some(value.to_owned());
            }
        } else if let Some(value) = line.strip_prefix("Author:") {
            let value = value.trim();
            if !value.is_empty() {
                metadata.author = Some(value.to_owned());
            }
        }
    }
    if pages == 0 {
        return Err(Error::ExternalProgram {
            program: "pdfinfo".to_owned(),
            message: "output did not contain a valid page count".to_owned(),
        });
    }
    Ok((pages, metadata))
}

fn inspect_pdf_native(path: &Path) -> (usize, PdfMetadata, Option<String>) {
    match Document::load(path) {
        Ok(document) => {
            let metadata = document
                .get_dict_in_dict(&document.trailer, b"Info")
                .ok()
                .map(|info| PdfMetadata {
                    title: info.get(b"Title").ok().and_then(object_text),
                    author: info.get(b"Author").ok().and_then(object_text),
                })
                .unwrap_or_default();
            (document.get_pages().len(), metadata, None)
        }
        Err(error) => (
            0,
            PdfMetadata::default(),
            Some(format!("native PDF inspection failed: {error}")),
        ),
    }
}

fn object_text(object: &Object) -> Option<String> {
    let value = decode_text_string(object).ok()?;
    let value = value.trim_matches(char::from(0)).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn extract_poppler(path: &Path, pages: usize) -> Result<String> {
    let output = Command::new("pdftotext")
        .args(["-f", "1", "-l"])
        .arg(pages.to_string())
        .args(["-layout", "-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .output()
        .map_err(|error| external_io_error("pdftotext", error))?;
    command_stdout("pdftotext", output)
}

fn extract_native(path: &Path, pages: usize) -> Result<String> {
    let document = Document::load(path)?;
    let mut output = String::new();
    for page in 1..=pages {
        if page > 1 {
            output.push('\u{c}');
        }
        match document.extract_text(&[page as u32]) {
            Ok(text) => output.push_str(&text),
            Err(error) => output.push_str(&format!("\n[page extraction failed: {error}]\n")),
        }
    }
    Ok(output)
}

fn ocr_page(path: &Path, page_number: usize, options: &ParseOptions) -> Result<String> {
    let directory = tempfile::tempdir().map_err(|source| Error::Io {
        path: std::env::temp_dir(),
        source,
    })?;
    let prefix = directory.path().join("page");
    let output = Command::new("pdftoppm")
        .args([
            "-f",
            &page_number.to_string(),
            "-l",
            &page_number.to_string(),
        ])
        .args([
            "-singlefile",
            "-r",
            &options.ocr_dpi.to_string(),
            "-gray",
            "-png",
        ])
        .arg(path)
        .arg(&prefix)
        .output()
        .map_err(|error| external_io_error("pdftoppm", error))?;
    ensure_success("pdftoppm", &output)?;

    let rendered = prefix.with_extension("png");
    let ocr_input = preprocess_for_ocr(&rendered, directory.path())?;
    let output = Command::new("tesseract")
        .arg(&ocr_input)
        .arg("stdout")
        .args(["-l", &options.ocr_language, "--psm", "3"])
        .args(["-c", "preserve_interword_spaces=1"])
        .output()
        .map_err(|error| external_io_error("tesseract", error))?;
    command_stdout("tesseract", output)
}

#[cfg(feature = "opencv-preprocessing")]
fn preprocess_for_ocr(input: &Path, directory: &Path) -> Result<PathBuf> {
    use opencv::{core, imgcodecs, imgproc};

    let input_name = input.to_string_lossy();
    let source = imgcodecs::imread(&input_name, imgcodecs::IMREAD_GRAYSCALE).map_err(|error| {
        Error::ExternalProgram {
            program: "opencv".to_owned(),
            message: error.to_string(),
        }
    })?;
    let mut denoised = core::Mat::default();
    imgproc::median_blur(&source, &mut denoised, 3).map_err(opencv_error)?;
    let mut binary = core::Mat::default();
    imgproc::adaptive_threshold(
        &denoised,
        &mut binary,
        255.0,
        imgproc::ADAPTIVE_THRESH_GAUSSIAN_C,
        imgproc::THRESH_BINARY,
        31,
        12.0,
    )
    .map_err(opencv_error)?;
    let output = directory.join("page-preprocessed.png");
    imgcodecs::imwrite(&output.to_string_lossy(), &binary, &core::Vector::new())
        .map_err(opencv_error)?;
    Ok(output)
}

#[cfg(feature = "opencv-preprocessing")]
fn opencv_error(error: opencv::Error) -> Error {
    Error::ExternalProgram {
        program: "opencv".to_owned(),
        message: error.to_string(),
    }
}

#[cfg(not(feature = "opencv-preprocessing"))]
fn preprocess_for_ocr(input: &Path, _directory: &Path) -> Result<PathBuf> {
    Ok(input.to_owned())
}

fn split_pages(text: &str, expected: usize) -> Vec<String> {
    let mut pages: Vec<String> = text.split('\u{c}').map(ToOwned::to_owned).collect();
    while pages.last().is_some_and(|page| page.trim().is_empty()) && pages.len() > expected {
        pages.pop();
    }
    pages.resize_with(expected, String::new);
    pages.truncate(expected);
    pages
}

fn useful_character_count(text: &str) -> usize {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .count()
}

fn normalize_text(text: &str) -> String {
    text.nfkc()
        .collect::<String>()
        .replace('\0', "")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn external_io_error(program: &str, error: std::io::Error) -> Error {
    Error::ExternalProgram {
        program: program.to_owned(),
        message: error.to_string(),
    }
}

fn ensure_success(program: &str, output: &std::process::Output) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    Err(Error::ExternalProgram {
        program: program.to_owned(),
        message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

fn command_stdout(program: &str, output: std::process::Output) -> Result<String> {
    ensure_success(program, &output)?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
