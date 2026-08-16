use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("input does not exist: {0}")]
    InputNotFound(PathBuf),
    #[error("input is not a regular PDF file: {0}")]
    InvalidInput(PathBuf),
    #[error("cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("PDF error: {0}")]
    Pdf(#[from] lopdf::Error),
    #[error("external program `{program}` failed: {message}")]
    ExternalProgram { program: String, message: String },
    #[error("no usable text could be extracted from the PDF")]
    NoText,
    #[error("invalid parser options: {0}")]
    InvalidOptions(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
