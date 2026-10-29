//! Loads the supplied reference CSVs into SQLite at startup.
//!
//! This module owns reading and validating the CSV files; the SQL lives in
//! `persistence::reference`. Loading is idempotent: every row is upserted on
//! its natural primary key inside one transaction, so restarting the
//! application never duplicates reference records, and a corrected reference
//! file replaces stale values. Rows removed from a file are not deleted
//! (portfolios and instruments may be referenced by stored trades); see the
//! README's limitations section.

mod error;

pub use error::ReferenceError;

use std::path::Path;

use rust_decimal::Decimal;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sqlx::SqlitePool;

use crate::domain::{
    Currency, FieldError, FxRate, Instrument, InstrumentId, Portfolio, PortfolioId, Price,
    parse_date,
};
use crate::manifest::Manifest;
use crate::persistence::PersistenceError;
use crate::persistence::reference as db;

/// One reference CSV. `file_name` is what is opened inside the configured
/// `reference_data_dir`; `key` is the logical name the manifest's
/// `file_counts` uses (relative to the dataset root) and that error messages
/// show. The two are kept apart so the directory can live anywhere.
#[derive(Clone, Copy)]
struct ReferenceFile {
    file_name: &'static str,
    key: &'static str,
}

const PORTFOLIOS_CSV: ReferenceFile = ReferenceFile {
    file_name: "portfolios.csv",
    key: "reference/portfolios.csv",
};
const INSTRUMENTS_CSV: ReferenceFile = ReferenceFile {
    file_name: "instruments.csv",
    key: "reference/instruments.csv",
};
const PRICES_CSV: ReferenceFile = ReferenceFile {
    file_name: "prices.csv",
    key: "reference/prices.csv",
};
const FX_RATES_CSV: ReferenceFile = ReferenceFile {
    file_name: "fx_rates.csv",
    key: "reference/fx_rates.csv",
};

/// A position in a reference file, used to attach context to errors.
#[derive(Clone, Copy)]
struct At {
    file: &'static str,
    row: usize,
}

impl At {
    /// `index` is the zero-based data-row index; the header is row 1.
    fn new(file: &'static str, index: usize) -> Self {
        Self {
            file,
            row: index + 2,
        }
    }

    fn invalid(self, source: FieldError) -> ReferenceError {
        ReferenceError::InvalidValue {
            file: self.file,
            row: self.row,
            source,
        }
    }

    fn store(self, source: PersistenceError) -> ReferenceError {
        ReferenceError::Store {
            file: self.file,
            row: self.row,
            source,
        }
    }
}

#[derive(Debug, Deserialize)]
struct PortfolioRow {
    portfolio_id: String,
    name: String,
    base_currency: String,
}

#[derive(Debug, Deserialize)]
struct InstrumentRow {
    instrument_id: String,
    ticker: String,
    name: String,
    currency: String,
    sector: String,
}

#[derive(Debug, Deserialize)]
struct PriceRow {
    instrument_id: String,
    date: String,
    close_price: String,
}

#[derive(Debug, Deserialize)]
struct FxRow {
    date: String,
    currency: String,
    gbp_per_unit: String,
}

/// Rows loaded from each reference file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceCounts {
    pub portfolios: usize,
    pub instruments: usize,
    pub prices: usize,
    pub fx_rates: usize,
}

/// Reads and validates all four reference files, then upserts every row in a
/// single transaction. Any invalid row or failed write rolls back the whole
/// load, leaving the stored reference data unchanged.
pub async fn load_reference_data(
    pool: &SqlitePool,
    reference_data_dir: &Path,
    manifest: &Manifest,
) -> Result<ReferenceCounts, ReferenceError> {
    let portfolios: Vec<PortfolioRow> = read_reference_csv(
        reference_data_dir,
        PORTFOLIOS_CSV,
        &["portfolio_id", "name", "base_currency"],
        manifest,
    )?;
    let instruments: Vec<InstrumentRow> = read_reference_csv(
        reference_data_dir,
        INSTRUMENTS_CSV,
        &["instrument_id", "ticker", "name", "currency", "sector"],
        manifest,
    )?;
    let prices: Vec<PriceRow> = read_reference_csv(
        reference_data_dir,
        PRICES_CSV,
        &["instrument_id", "date", "close_price"],
        manifest,
    )?;
    let fx_rates: Vec<FxRow> = read_reference_csv(
        reference_data_dir,
        FX_RATES_CSV,
        &["date", "currency", "gbp_per_unit"],
        manifest,
    )?;

    let transaction_failed = |e: sqlx::Error| ReferenceError::Transaction(e.into());
    let mut tx = pool.begin().await.map_err(transaction_failed)?;

    for (i, row) in portfolios.iter().enumerate() {
        let at = At::new(PORTFOLIOS_CSV.key, i);
        let portfolio_id = PortfolioId::parse(&row.portfolio_id).map_err(|e| at.invalid(e))?;

        // GBP is the only supported reporting currency. Any other value,
        // including one that is not a currency at all, gets the same error.
        let base_currency = match row.base_currency.parse() {
            Ok(Currency::Gbp) => Currency::Gbp,
            _ => {
                return Err(ReferenceError::BaseCurrencyNotGbp {
                    file: at.file,
                    row: at.row,
                });
            }
        };

        let portfolio = Portfolio {
            portfolio_id,
            name: row.name.clone(),
            base_currency,
        };

        db::upsert_portfolio(&mut tx, &portfolio)
            .await
            .map_err(|e| at.store(e))?;
    }

    for (i, row) in instruments.iter().enumerate() {
        let at = At::new(INSTRUMENTS_CSV.key, i);
        let instrument = Instrument {
            instrument_id: InstrumentId::parse(&row.instrument_id).map_err(|e| at.invalid(e))?,
            ticker: row.ticker.clone(),
            name: row.name.clone(),
            currency: row.currency.parse().map_err(|e| at.invalid(e))?,
            sector: row.sector.clone(),
        };

        db::upsert_instrument(&mut tx, &instrument)
            .await
            .map_err(|e| at.store(e))?;
    }

    for (i, row) in prices.iter().enumerate() {
        let at = At::new(PRICES_CSV.key, i);
        let id = InstrumentId::parse(&row.instrument_id).map_err(|e| at.invalid(e))?;
        let date = parse_date(&row.date, "date").map_err(|e| at.invalid(e))?;
        let price = Price::parse(&row.close_price).map_err(|e| at.invalid(e))?;

        db::upsert_price(&mut tx, &id, date, price)
            .await
            .map_err(|e| at.store(e))?;
    }

    for (i, row) in fx_rates.iter().enumerate() {
        let at = At::new(FX_RATES_CSV.key, i);
        let date = parse_date(&row.date, "date").map_err(|e| at.invalid(e))?;
        let currency: Currency = row.currency.parse().map_err(|e| at.invalid(e))?;
        let rate = FxRate::parse(&row.gbp_per_unit).map_err(|e| at.invalid(e))?;

        if currency == Currency::Gbp && rate.value() != Decimal::ONE {
            return Err(ReferenceError::GbpRateNotOne {
                file: at.file,
                row: at.row,
            });
        }

        db::upsert_fx_rate(&mut tx, currency, date, rate)
            .await
            .map_err(|e| at.store(e))?;
    }

    // Returning early above drops `tx`, which rolls back every upsert.
    tx.commit().await.map_err(transaction_failed)?;

    Ok(ReferenceCounts {
        portfolios: portfolios.len(),
        instruments: instruments.len(),
        prices: prices.len(),
        fx_rates: fx_rates.len(),
    })
}

/// The manifest's expected row count for `file` when it differs from
/// `actual`. Looked up by the logical key (`reference/prices.csv`), which is
/// independent of where `reference_data_dir` points.
fn count_mismatch(manifest: &Manifest, file: ReferenceFile, actual: usize) -> Option<usize> {
    manifest
        .expected_file_count(file.key)
        .filter(|&expected| expected != actual)
}

/// Reads a reference CSV, insisting on the exact header from the data contract.
/// A row count that disagrees with the manifest is logged rather than fatal:
/// the file contents, not the manifest, are what gets validated and loaded.
fn read_reference_csv<T: DeserializeOwned>(
    reference_data_dir: &Path,
    reference_file: ReferenceFile,
    expected_header: &[&str],
    manifest: &Manifest,
) -> Result<Vec<T>, ReferenceError> {
    let path = reference_data_dir.join(reference_file.file_name);
    let file = reference_file.key;
    let mut reader =
        csv::Reader::from_path(&path).map_err(|source| ReferenceError::Open { path, source })?;

    let header = reader
        .headers()
        .map_err(|source| ReferenceError::Csv {
            file,
            row: 1,
            source,
        })?
        .clone();
    if !header.iter().eq(expected_header.iter().copied()) {
        return Err(ReferenceError::Header {
            file,
            expected: expected_header.join(","),
            found: header.iter().collect::<Vec<_>>().join(","),
        });
    }

    let rows = reader
        .deserialize()
        .enumerate()
        .map(|(i, row)| {
            row.map_err(|source| ReferenceError::Csv {
                file,
                row: i + 2,
                source,
            })
        })
        .collect::<Result<Vec<T>, _>>()?;

    if let Some(expected) = count_mismatch(manifest, reference_file, rows.len()) {
        tracing::warn!(
            file,
            expected,
            actual = rows.len(),
            "reference row count differs from manifest"
        );
    }

    Ok(rows)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The supplied manifest, whose `file_counts` keys are dataset-relative.
    fn supplied_manifest() -> Manifest {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/manifest.json");
        Manifest::load(&path).unwrap()
    }

    #[test]
    fn manifest_counts_are_matched_by_logical_key() {
        let manifest = supplied_manifest();
        let files = [
            (PORTFOLIOS_CSV, 3),
            (INSTRUMENTS_CSV, 24),
            (PRICES_CSV, 1436),
            (FX_RATES_CSV, 178),
        ];

        for (file, rows) in files {
            assert!(
                manifest.expected_file_count(file.key).is_some(),
                "manifest has no count for {}",
                file.key
            );
            assert_eq!(count_mismatch(&manifest, file, rows), None, "{}", file.key);
            assert_eq!(
                count_mismatch(&manifest, file, rows + 1),
                Some(rows),
                "{}",
                file.key
            );
        }

        // The bare file name is not a manifest key: matching must use `key`.
        assert_eq!(manifest.expected_file_count(PRICES_CSV.file_name), None);
    }
}
