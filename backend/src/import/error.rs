//! Import errors at three levels:
//! - `FileError`: the file as a whole is unusable; no row counts apply.
//! - `RowValidationError`: one problem with one row. A row can have several.
//! - `ImportError`: an unexpected failure (database), unlike the two above,
//!   which are normal outcomes reported to the user as a rejected import.

use std::fmt;

use chrono::NaiveDate;
use thiserror::Error;

use super::parsing::TRADE_HEADER;
use crate::domain::{FieldError, InstrumentId, PortfolioId, TradeId};
use crate::persistence::PersistenceError;

/// A problem with the uploaded file as a whole. The messages are shown to
/// users as-is.
#[derive(Debug, Error)]
pub enum FileError {
    #[error("The file is not valid UTF-8 text.")]
    InvalidEncoding(#[source] std::str::Utf8Error),
    #[error("The file is empty.")]
    Empty,
    #[error("The header row could not be read: {source}.")]
    HeaderUnreadable {
        #[source]
        source: csv::Error,
    },
    #[error("The header must be exactly '{}' but was '{actual}'.", TRADE_HEADER.join(","))]
    InvalidHeader { actual: String },
    #[error("Row {row_number} could not be parsed as CSV: {source}.")]
    RowUnreadable {
        row_number: usize,
        #[source]
        source: csv::Error,
    },
    #[error("The file has a header but no trade rows.")]
    NoRows,
}

/// One validation problem with one data row. Rows accumulate a list of these,
/// so every problem is reported, and each is turned into a message only when
/// the import report is assembled.
#[derive(Debug, Clone, Error)]
pub enum RowValidationError {
    #[error("expected {expected} fields but found {actual}")]
    WrongFieldCount { expected: usize, actual: usize },
    /// A field that fails its domain type's rules.
    #[error(transparent)]
    Field(#[from] FieldError),
    #[error("portfolio_id '{portfolio_id}' does not exist")]
    UnknownPortfolio { portfolio_id: PortfolioId },
    #[error("instrument_id '{instrument_id}' does not exist")]
    UnknownInstrument { instrument_id: InstrumentId },
    /// `calendar` is the first and last valuation date, for the message.
    #[error(
        "trade date {date} is not a valuation date in the dataset calendar{}",
        calendar_suffix(.calendar)
    )]
    NotAValuationDate {
        date: NaiveDate,
        calendar: Option<(NaiveDate, NaiveDate)>,
    },
    /// The same trade ID appears in several rows of the file with different
    /// values. Every one of those rows carries this error. The ID is the raw
    /// text, because some of the rows may not have parsed.
    #[error(
        "trade_id '{trade_id}' appears in rows {} with conflicting values",
        join(.rows, ", ")
    )]
    ConflictingTradeIdInFile { trade_id: String, rows: Vec<usize> },
    /// The trade ID is already stored with different values.
    #[error(
        "trade_id '{trade_id}' already exists with different values: {}. \
         Stored trades are never overwritten; use a new trade_id",
        join(.differences, "; ")
    )]
    ConflictsWithStoredTrade {
        trade_id: TradeId,
        differences: Vec<FieldDifference>,
    },
}

/// One field that differs between a stored trade and an incoming row, with
/// both values in canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDifference {
    pub field: &'static str,
    pub stored: String,
    pub incoming: String,
}

impl fmt::Display for FieldDifference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} stored {} vs file {}",
            self.field, self.stored, self.incoming
        )
    }
}

fn join<T: fmt::Display>(items: &[T], separator: &str) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

fn calendar_suffix(calendar: &Option<(NaiveDate, NaiveDate)>) -> String {
    match calendar {
        Some((first, last)) => format!(" ({first} to {last})"),
        None => String::new(),
    }
}

/// An unexpected failure while importing. A file that breaks the data
/// contract is *not* an error: it is an `Ok(ImportReport)` with
/// `Outcome::Rejected`, which the API returns as HTTP 422.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("trade import failed: {0}")]
    Persistence(#[from] PersistenceError),
}
