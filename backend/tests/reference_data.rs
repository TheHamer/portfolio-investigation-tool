//! Reference CSVs go through the same domain validation as uploads.

mod common;

use std::path::{Path, PathBuf};

use portfolio_investigation::error::AppError;
use portfolio_investigation::initialise;
use portfolio_investigation::reference::{ReferenceError, load_reference_data};
use sqlx::SqlitePool;
use tempfile::TempDir;

/// Copies the four supplied reference CSVs into `dir/<dir_name>`, applying
/// `edit` to one of them, and returns that directory.
fn reference_dir_with(
    dir: &TempDir,
    dir_name: &str,
    file: &str,
    edit: impl Fn(&str) -> String,
) -> PathBuf {
    let reference = dir.path().join(dir_name);
    std::fs::create_dir_all(&reference).unwrap();
    let source = common::dataset_dir().join("reference");

    for name in [
        "portfolios.csv",
        "instruments.csv",
        "prices.csv",
        "fx_rates.csv",
    ] {
        let text = std::fs::read_to_string(source.join(name)).unwrap();
        let text = if name == file { edit(&text) } else { text };
        std::fs::write(reference.join(name), text).unwrap();
    }

    reference
}

/// Starts with the supplied manifest and the given reference directory.
async fn start_with(dir: &TempDir, reference_data_dir: &Path) -> Result<(), AppError> {
    let config = common::load_config(
        dir,
        &dir.path().join("ref.db"),
        &common::dataset_dir().join("manifest.json"),
        reference_data_dir,
    );

    initialise(&config).await.map(|_| ())
}

#[tokio::test]
async fn supplied_reference_data_loads() {
    let dir = TempDir::new().unwrap();
    start_with(&dir, &common::dataset_dir().join("reference"))
        .await
        .unwrap();
}

#[tokio::test]
async fn reference_directory_can_live_anywhere() {
    // Not named "reference" and not next to the manifest: the directory is
    // used as configured, and the manifest is configured separately.
    let dir = TempDir::new().unwrap();
    let elsewhere = reference_dir_with(&dir, "csv-files", "", str::to_owned);
    start_with(&dir, &elsewhere).await.unwrap();
}

#[tokio::test]
async fn price_with_too_many_decimal_places_stops_startup() {
    let dir = TempDir::new().unwrap();
    // First data row of prices.csv is "I001,2026-01-05,239.4400".
    let data = reference_dir_with(&dir, "reference", "prices.csv", |t| {
        t.replacen("I001,2026-01-05,239.4400", "I001,2026-01-05,239.44000", 1)
    });

    let error = start_with(&dir, &data).await.unwrap_err();
    assert!(
        matches!(
            &error,
            AppError::Reference(ReferenceError::InvalidValue {
                file: "reference/prices.csv",
                row: 2,
                ..
            })
        ),
        "{error}"
    );
    assert!(
        error.to_string().contains("reference/prices.csv row 2"),
        "{error}"
    );
    assert!(
        error.to_string().contains("more than 4 decimal places"),
        "{error}"
    );
}

#[tokio::test]
async fn fx_rate_with_too_many_decimal_places_stops_startup() {
    let dir = TempDir::new().unwrap();
    let data = reference_dir_with(&dir, "reference", "fx_rates.csv", |t| {
        t.replacen("2026-01-05,EUR,0.830630", "2026-01-05,EUR,0.8306300", 1)
    });

    let error = start_with(&dir, &data).await.unwrap_err();
    assert!(
        matches!(
            &error,
            AppError::Reference(ReferenceError::InvalidValue {
                file: "reference/fx_rates.csv",
                row: 2,
                ..
            })
        ),
        "{error}"
    );
    assert!(
        error.to_string().contains("more than 6 decimal places"),
        "{error}"
    );
}

/// Replaces `from` with `to` in one CSV of a copied reference directory,
/// insisting that `from` is present so the test cannot silently do nothing.
fn replace_in(reference_dir: &Path, file: &str, from: &str, to: &str) {
    let path = reference_dir.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{file} does not contain {from:?}");
    std::fs::write(&path, text.replacen(from, to, 1)).unwrap();
}

/// Every stored reference row, in a stable order: comparing two snapshots
/// compares values, not just row counts.
#[derive(Debug, PartialEq)]
struct ReferenceSnapshot {
    portfolios: Vec<(String, String, String)>,
    instruments: Vec<(String, String, String, String, String)>,
    prices: Vec<(String, String, String)>,
    fx_rates: Vec<(String, String, String)>,
}

async fn snapshot(pool: &SqlitePool) -> ReferenceSnapshot {
    ReferenceSnapshot {
        portfolios: sqlx::query_as(
            "SELECT portfolio_id, name, base_currency FROM portfolios ORDER BY portfolio_id",
        )
        .fetch_all(pool)
        .await
        .unwrap(),
        instruments: sqlx::query_as(
            "SELECT instrument_id, ticker, name, currency, sector FROM instruments
             ORDER BY instrument_id",
        )
        .fetch_all(pool)
        .await
        .unwrap(),
        prices: sqlx::query_as(
            "SELECT instrument_id, date, close_price FROM prices ORDER BY instrument_id, date",
        )
        .fetch_all(pool)
        .await
        .unwrap(),
        fx_rates: sqlx::query_as(
            "SELECT currency, date, gbp_per_unit FROM fx_rates ORDER BY currency, date",
        )
        .fetch_all(pool)
        .await
        .unwrap(),
    }
}

#[tokio::test]
async fn loading_the_same_reference_data_twice_changes_nothing() {
    let dir = TempDir::new().unwrap();
    // First load: the normal startup path.
    let state = common::start(&dir).await;
    let before = snapshot(&state.pool).await;
    assert_eq!(
        (
            before.portfolios.len(),
            before.instruments.len(),
            before.prices.len(),
            before.fx_rates.len()
        ),
        (3, 24, 1436, 178)
    );

    // Second load of the identical files into the same database.
    let counts = load_reference_data(
        &state.pool,
        &common::dataset_dir().join("reference"),
        &state.manifest,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            counts.portfolios,
            counts.instruments,
            counts.prices,
            counts.fx_rates
        ),
        (3, 24, 1436, 178)
    );

    let after = snapshot(&state.pool).await;
    assert_eq!(after, before, "every stored value is unchanged");
    // A few representative values, including a quoted name with a comma.
    assert!(after.portfolios.contains(&(
        "P002".into(),
        "Fictional Income, Value & Quality Portfolio".into(),
        "GBP".into()
    )));
    assert!(
        after
            .prices
            .contains(&("I001".into(), "2026-01-05".into(), "239.4400".into()))
    );
    assert!(
        after
            .fx_rates
            .contains(&("EUR".into(), "2026-01-05".into(), "0.830630".into()))
    );
}

#[tokio::test]
async fn failure_after_earlier_writes_rolls_back_the_whole_load() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    let before = snapshot(&state.pool).await;

    // Valid edits to files processed early in the transaction (portfolios,
    // then prices): one value change without a row-count change, one value
    // change, and one new row.
    let modified = reference_dir_with(&dir, "modified", "", str::to_owned);
    replace_in(
        &modified,
        "portfolios.csv",
        "P001,Fictional Growth Portfolio,GBP",
        "P001,Renamed Growth Portfolio,GBP",
    );
    replace_in(
        &modified,
        "prices.csv",
        "I001,2026-01-05,239.4400",
        "I001,2026-01-05,999.0000
I001,2030-01-02,1.0000",
    );
    // The failure comes last: fx_rates.csv is processed after the other three
    // files, and the GBP-must-be-1 rule is checked inside the transaction,
    // so the edits above have already been written when it fails.
    replace_in(
        &modified,
        "fx_rates.csv",
        "2026-01-05,GBP,1.000000",
        "2026-01-05,GBP,1.100000",
    );

    let error = load_reference_data(&state.pool, &modified, &state.manifest)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            ReferenceError::GbpRateNotOne {
                file: "reference/fx_rates.csv",
                row: 3
            }
        ),
        "{error}"
    );

    // Nothing from the failed load is visible: same rows, same values.
    let after = snapshot(&state.pool).await;
    assert_eq!(after, before);
    assert!(after.portfolios.contains(&(
        "P001".into(),
        "Fictional Growth Portfolio".into(),
        "GBP".into()
    )));
    assert!(
        after
            .prices
            .contains(&("I001".into(), "2026-01-05".into(), "239.4400".into()))
    );
    assert!(!after.prices.iter().any(|(_, date, _)| date == "2030-01-02"));

    // Control: the same edits without the bad FX row do take effect, so the
    // values above were preserved by the rollback, not because the edits
    // were never applied.
    replace_in(
        &modified,
        "fx_rates.csv",
        "2026-01-05,GBP,1.100000",
        "2026-01-05,GBP,1.000000",
    );
    load_reference_data(&state.pool, &modified, &state.manifest)
        .await
        .unwrap();

    let applied = snapshot(&state.pool).await;
    assert!(applied.portfolios.contains(&(
        "P001".into(),
        "Renamed Growth Portfolio".into(),
        "GBP".into()
    )));
    assert!(
        applied
            .prices
            .contains(&("I001".into(), "2026-01-05".into(), "999.0000".into()))
    );
    assert_eq!(applied.prices.len(), before.prices.len() + 1);
}

#[tokio::test]
async fn non_gbp_portfolio_base_currency_is_rejected() {
    let dir = TempDir::new().unwrap();
    let reference = reference_dir_with(&dir, "reference", "portfolios.csv", |t| {
        t.replacen(
            "P001,Fictional Growth Portfolio,GBP",
            "P001,Fictional Growth Portfolio,USD",
            1,
        )
    });

    let error = start_with(&dir, &reference).await.unwrap_err();
    assert!(
        matches!(
            &error,
            AppError::Reference(ReferenceError::BaseCurrencyNotGbp {
                file: "reference/portfolios.csv",
                row: 2
            })
        ),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "reference/portfolios.csv row 2: base_currency must be GBP"
    );
}

/// A base currency that is not a supported currency at all (wrong case,
/// unknown code) gets the same error as a supported non-GBP one.
#[tokio::test]
async fn unrecognised_portfolio_base_currency_is_rejected_as_non_gbp() {
    for base_currency in ["gbp", "JPY"] {
        let dir = TempDir::new().unwrap();
        let reference = reference_dir_with(&dir, "reference", "portfolios.csv", |t| {
            t.replacen(
                "P001,Fictional Growth Portfolio,GBP",
                &format!("P001,Fictional Growth Portfolio,{base_currency}"),
                1,
            )
        });

        let error = start_with(&dir, &reference).await.unwrap_err();

        assert!(
            matches!(
                &error,
                AppError::Reference(ReferenceError::BaseCurrencyNotGbp {
                    file: "reference/portfolios.csv",
                    row: 2
                })
            ),
            "{base_currency}: {error}"
        );
    }
}
