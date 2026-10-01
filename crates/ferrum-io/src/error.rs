//! Error type of the data layer.

use std::path::{Path, PathBuf};

use ferrum_domain::{RepositoryError, VolumeError};
use thiserror::Error;

/// Errors raised while reading medical image files.
#[derive(Debug, Error)]
pub enum IoError {
    /// Operating-system error.
    #[error("{path}: {source}")]
    Os {
        /// Offending file.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The file could not be parsed.
    #[error("{path}: cannot parse: {message}")]
    Parse {
        /// Offending file.
        path: PathBuf,
        /// Parser message.
        message: String,
    },
    /// A mandatory attribute is missing.
    #[error("{path}: missing attribute {attribute}")]
    MissingAttribute {
        /// Offending file.
        path: PathBuf,
        /// Attribute name.
        attribute: &'static str,
    },
    /// The data violates an invariant.
    #[error("{path}: {message}")]
    Invalid {
        /// Offending file.
        path: PathBuf,
        /// Explanation.
        message: String,
    },
    /// Pixel data could not be decoded (e.g. unsupported compression).
    #[error("{path}: cannot decode pixel data: {message}")]
    Decode {
        /// Offending file.
        path: PathBuf,
        /// Decoder message.
        message: String,
    },
    /// Assembled volume is invalid.
    #[error(transparent)]
    Volume(#[from] VolumeError),
    /// Series slices are inconsistent.
    #[error("inconsistent series: {0}")]
    Inconsistent(String),
    /// The operation was cancelled.
    #[error("cancelled")]
    Cancelled,
}

impl IoError {
    pub(crate) fn os(path: &Path, source: std::io::Error) -> Self {
        Self::Os { path: path.to_path_buf(), source }
    }

    pub(crate) fn parse(path: &Path, e: impl std::fmt::Display) -> Self {
        Self::Parse { path: path.to_path_buf(), message: e.to_string() }
    }

    pub(crate) fn missing(path: &Path, attribute: &'static str) -> Self {
        Self::MissingAttribute { path: path.to_path_buf(), attribute }
    }

    pub(crate) fn invalid(path: &Path, message: impl Into<String>) -> Self {
        Self::Invalid { path: path.to_path_buf(), message: message.into() }
    }

    pub(crate) fn decode(path: &Path, e: impl std::fmt::Display) -> Self {
        Self::Decode { path: path.to_path_buf(), message: e.to_string() }
    }
}

impl From<IoError> for RepositoryError {
    fn from(e: IoError) -> Self {
        match e {
            IoError::Os { .. } => RepositoryError::Io(e.to_string()),
            IoError::Decode { .. } => RepositoryError::Unsupported(e.to_string()),
            IoError::Cancelled => RepositoryError::Cancelled,
            _ => RepositoryError::Corrupt(e.to_string()),
        }
    }
}
