//! Errors from loading the reference CSVs.

use std::path::PathBuf;

use thiserror::Error;

use crate::domain::FieldError;
use crate::persistence::PersistenceError;

/// Every variant names the file, and the row where one applies (the header
/// is row 1), so a bad reference file can be fixed from the message alone.
#[derive(Debug, Error)]
pub enum ReferenceError {
    #[error("could not open {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: csv::Error,
    },
    #[error("{file}: expected header {expected:?}, found {found:?}")]
    Header {
        file: &'static str,
        expected: String,
        found: String,
    },
    #[error("{file} row {row}: {source}")]
    Csv {
        file: &'static str,
        row: usize,
        #[source]
        source: csv::Error,
    },
    #[error("{file} row {row}: {source}")]
    InvalidValue {
        file: &'static str,
        row: usize,
        #[source]
        source: FieldError,
    },
    #[error("{file} row {row}: base_currency must be GBP")]
    BaseCurrencyNotGbp { file: &'static str, row: usize },
    #[error("{file} row {row}: the GBP rate must be 1")]
    GbpRateNotOne { file: &'static str, row: usize },
    #[error("{file} row {row}: could not store the row: {source}")]
    Store {
        file: &'static str,
        row: usize,
        #[source]
        source: PersistenceError,
    },
    #[error("reference data transaction failed: {0}")]
    Transaction(#[source] PersistenceError),
}
