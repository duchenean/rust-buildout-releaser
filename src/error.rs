use thiserror::Error;

#[derive(Error, Debug)]
pub enum ReleaserError {
    #[error("Failed to fetch package info from PyPI: {0}")]
    PyPiError(String),

    #[error("Package not found on PyPI: {0}")]
    PackageNotFound(String),

    #[error("Failed to parse buildout file: {0}")]
    BuildoutParseError(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Git operation failed: {0}")]
    GitError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Version parse error: {0}")]
    VersionError(String),
}

pub type Result<T> = std::result::Result<T, ReleaserError>;
