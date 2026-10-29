//! Exercises the SQLite constraints directly, bypassing all Rust validation,
//! to check what the database itself guarantees.

mod common;

use common::start;
use sqlx::SqlitePool;
use tempfile::TempDir;

/// Text that is not a positive decimal literal, as raw SQL values.
const MALFORMED: &[&str] = &[
    "", "0", "0.0", "000.000", "-1", "-1.5", "+1", "1..2", "1.2.3", ".5", "5.", "1e5", "1E5",
    "abc", "NaN", "inf", " 1", "1 ", "1,000", "0x10", "1_000",
];

async fn insert_price(pool: &SqlitePool, date: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO prices (instrument_id, date, close_price) VALUES ('I001', ?, ?)")
        .bind(date)
        .bind(value)
        .execute(pool)
        .await
        .map(|_| ())
}

async fn insert_fx(pool: &SqlitePool, date: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO fx_rates (currency, date, gbp_per_unit) VALUES ('USD', ?, ?)")
        .bind(date)
        .bind(value)
        .execute(pool)
        .await
        .map(|_| ())
}

async fn insert_trade(
    pool: &SqlitePool,
    id: &str,
    portfolio: &str,
    quantity: i64,
    price: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO trades
            (trade_id, portfolio_id, instrument_id, timestamp_utc, side, quantity, price, imported_at)
         VALUES (?, ?, 'I001', '2026-01-05T10:00:00Z', 'BUY', ?, ?, '2026-01-05T10:00:00Z')",
    )
    .bind(id)
    .bind(portfolio)
    .bind(quantity)
    .bind(price)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn price_column_accepts_only_positive_decimals_with_four_places() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;

    for (i, bad) in MALFORMED.iter().chain(&["1.00000", "1.12345"]).enumerate() {
        let date = format!("2031-01-{:02}", i + 1);
        assert!(
            insert_price(&state.pool, &date, bad).await.is_err(),
            "prices accepted {bad:?}"
        );
    }

    for (i, good) in ["1", "10", "0.0001", "12.5", "007.5000", "123456789.1234"]
        .iter()
        .enumerate()
    {
        let date = format!("2032-01-{:02}", i + 1);
        insert_price(&state.pool, &date, good)
            .await
            .unwrap_or_else(|e| panic!("prices rejected {good:?}: {e}"));
    }
}

#[tokio::test]
async fn fx_column_accepts_only_positive_decimals_with_six_places() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;

    for (i, bad) in MALFORMED.iter().chain(&["1.0000000"]).enumerate() {
        let date = format!("2031-01-{:02}", i + 1);
        assert!(
            insert_fx(&state.pool, &date, bad).await.is_err(),
            "fx_rates accepted {bad:?}"
        );
    }

    for (i, good) in ["1", "0.000001", "0.782484", "1.000000"].iter().enumerate() {
        let date = format!("2032-01-{:02}", i + 1);
        insert_fx(&state.pool, &date, good)
            .await
            .unwrap_or_else(|e| panic!("fx_rates rejected {good:?}: {e}"));
    }
}

#[tokio::test]
async fn trades_table_enforces_its_own_invariants() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let pool = &state.pool;

    for (i, bad) in MALFORMED.iter().chain(&["1.00000"]).enumerate() {
        assert!(
            insert_trade(pool, &format!("BADP{i}"), "P001", 1, bad)
                .await
                .is_err(),
            "trades accepted price {bad:?}"
        );
    }

    assert!(insert_trade(pool, "Q0", "P001", 0, "1").await.is_err());
    assert!(insert_trade(pool, "QNEG", "P001", -5, "1").await.is_err());
    assert!(insert_trade(pool, "", "P001", 1, "1").await.is_err());
    // Foreign key: unknown portfolio.
    assert!(insert_trade(pool, "FK1", "P999", 1, "1").await.is_err());

    insert_trade(pool, "OK1", "P001", 1, "10").await.unwrap();
    insert_trade(pool, "OK2", "P001", i64::MAX, "84.4835")
        .await
        .unwrap();

    // Primary key: a second row with the same trade_id.
    assert!(insert_trade(pool, "OK1", "P001", 1, "10").await.is_err());
    // IDs are exact, so these are distinct keys.
    insert_trade(pool, "ok1", "P001", 1, "10").await.unwrap();
    insert_trade(pool, " OK1", "P001", 1, "10").await.unwrap();
}
