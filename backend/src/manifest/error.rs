//! Errors from reading and validating `manifest.json`.

use std::path::PathBuf;

use chrono::NaiveDate;
use thiserror::Error;

use crate::domain::FieldError;

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("could not read manifest {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("manifest is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("manifest has an invalid date: {0}")]
    InvalidDate(#[from] FieldError),
    #[error("manifest base currency must be GBP, found {0}")]
    UnsupportedBaseCurrency(String),
    #[error("manifest as_of {0} must be the final valuation date")]
    AsOfNotFinal(NaiveDate),
}
