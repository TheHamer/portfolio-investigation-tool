//! Trade CSV import.
//!
//! The pipeline has three steps, one per module:
//!
//! 1. `parsing`: decode the bytes, check the exact header and split the file
//!    into raw records. No business rules and no database access, so an
//!    unreadable file is rejected first.
//! 2. `validation`: validate every row's fields and reference IDs, then find
//!    trade IDs repeated within the file with different values. Pure; needs no
//!    database lock.
//! 3. `service`: inside one `BEGIN IMMEDIATE` transaction, compare the file's
//!    trade IDs with stored trades (identical stored trades are skipped,
//!    different ones are conflicts), then either roll back and report, or
//!    insert and commit.
//!
//! `report` turns the outcome into the [`ImportReport`] shown to the user.
//!
//! Every row is validated even after the first failure, so the user sees all
//! problems at once. Any invalid row rejects the whole file. Nothing here
//! depends on HTTP; the importer is called directly by tests.

mod error;
mod parsing;
mod report;
mod service;
mod validation;

pub use error::{FileError, ImportError};
pub use report::{ImportReport, Outcome, RowError};
pub use service::import_trades;

#[cfg(test)]
mod test_support {
    use super::FileError;
    use super::parsing::{TRADE_HEADER, read_trade_csv};
    use super::validation::{ImportContext, ParsedRow, validate_rows};
    use crate::domain::{InstrumentId, PortfolioId};

    pub fn ctx() -> ImportContext {
        ImportContext {
            portfolios: ["P001", "P002"]
                .iter()
                .map(|p| PortfolioId::parse(p).unwrap())
                .collect(),
            instruments: ["I001", "I002"]
                .iter()
                .map(|i| InstrumentId::parse(i).unwrap())
                .collect(),
            valuation_dates: ["2026-01-05", "2026-01-06"]
                .iter()
                .map(|d| d.parse().unwrap())
                .collect(),
        }
    }

    /// Parses and validates CSV body rows (the header is prepended).
    pub fn parse(body: &str) -> Result<Vec<ParsedRow>, FileError> {
        let file = format!("{}\n{body}", TRADE_HEADER.join(","));
        Ok(validate_rows(read_trade_csv(file.as_bytes())?, &ctx()))
    }
}
