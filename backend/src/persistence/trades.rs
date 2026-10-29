//! SQL for the `trades` table.
//!
//! Queries are fixed string constants with bound parameters; no SQL text is
//! ever built from input. Having them as constants also lets the tests run
//! `EXPLAIN QUERY PLAN` on exactly the statements the application executes.

use chrono::Utc;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, SqliteConnection, SqlitePool, Transaction};

use super::PersistenceError;
use crate::domain::{
    InstrumentId, PortfolioId, Price, Trade, TradeId, TradeQuantity, TradeTimestamp,
};

const FIND_TRADE_SQL: &str = "
    SELECT trade_id, portfolio_id, instrument_id, timestamp_utc, side, quantity, price
    FROM trades
    WHERE trade_id = ?";

const INSERT_TRADE_SQL: &str = "
    INSERT INTO trades
        (trade_id, portfolio_id, instrument_id, timestamp_utc, side, quantity, price, imported_at)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

/// Uses `idx_trades_position (portfolio_id, ...)` for the lookup and the
/// ordering: rows come back already sorted, with no separate sort step.
const PORTFOLIO_TRADES_SQL: &str = "
    SELECT trade_id, portfolio_id, instrument_id, timestamp_utc, side, quantity, price
    FROM trades
    WHERE portfolio_id = ?
    ORDER BY instrument_id, timestamp_utc, trade_id";

/// Both equality columns match the index's two leading columns, so SQLite
/// seeks straight to one position's rows, already in execution order.
const POSITION_TRADES_SQL: &str = "
    SELECT trade_id, portfolio_id, instrument_id, timestamp_utc, side, quantity, price
    FROM trades
    WHERE portfolio_id = ? AND instrument_id = ?
    ORDER BY timestamp_utc, trade_id";

fn trade_from_row(row: &SqliteRow) -> Result<Trade, PersistenceError> {
    let quantity: i64 = row.try_get("quantity")?;
    Ok(Trade {
        trade_id: TradeId::parse(row.try_get("trade_id")?)?,
        portfolio_id: PortfolioId::parse(row.try_get("portfolio_id")?)?,
        instrument_id: InstrumentId::parse(row.try_get("instrument_id")?)?,
        timestamp: TradeTimestamp::parse(row.try_get("timestamp_utc")?)?,
        side: row.try_get::<&str, _>("side")?.parse()?,
        quantity: TradeQuantity::new(quantity)?,
        price: Price::parse(row.try_get("price")?)?,
    })
}

/// Starts an import's write transaction.
///
/// `BEGIN IMMEDIATE` takes SQLite's write lock up front. A plain (deferred)
/// `BEGIN` would let two concurrent uploads both read "trade not stored" and
/// then race to insert; with IMMEDIATE the second upload waits (up to the
/// busy timeout), then sees the first upload's committed rows during its own
/// duplicate check. Dropping the transaction without committing rolls it back.
pub async fn begin_import(
    pool: &SqlitePool,
) -> Result<Transaction<'static, Sqlite>, PersistenceError> {
    Ok(pool.begin_with("BEGIN IMMEDIATE").await?)
}

/// Looks up one trade by ID. Takes a connection so it can run inside the
/// import's write transaction.
pub async fn find_trade(
    conn: &mut SqliteConnection,
    id: &TradeId,
) -> Result<Option<Trade>, PersistenceError> {
    let row = sqlx::query(FIND_TRADE_SQL)
        .bind(id.as_str())
        .fetch_optional(conn)
        .await?;

    row.as_ref().map(trade_from_row).transpose()
}

/// Inserts one trade, stamping `imported_at` with the current UTC time. Takes
/// a connection so it runs inside the import's write transaction.
pub async fn insert_trade(
    conn: &mut SqliteConnection,
    trade: &Trade,
) -> Result<(), PersistenceError> {
    // Same canonical UTC form as `timestamp_utc`.
    let imported_at = Utc::now().format(TradeTimestamp::FORMAT).to_string();

    sqlx::query(INSERT_TRADE_SQL)
        .bind(trade.trade_id.as_str())
        .bind(trade.portfolio_id.as_str())
        .bind(trade.instrument_id.as_str())
        .bind(trade.timestamp.to_canonical())
        .bind(trade.side.as_str())
        .bind(trade.quantity.get())
        .bind(trade.price.value().to_string())
        .bind(imported_at)
        .execute(conn)
        .await?;
    Ok(())
}

/// All of a portfolio's trades in `(instrument_id, timestamp, trade_id)` order.
pub async fn portfolio_trades(
    pool: &SqlitePool,
    portfolio_id: &PortfolioId,
) -> Result<Vec<Trade>, PersistenceError> {
    let rows = sqlx::query(PORTFOLIO_TRADES_SQL)
        .bind(portfolio_id.as_str())
        .fetch_all(pool)
        .await?;

    rows.iter().map(trade_from_row).collect()
}

/// One position's trades, `(portfolio, instrument)`, in `(timestamp, trade_id)`
/// order.
pub async fn position_trades(
    pool: &SqlitePool,
    portfolio_id: &PortfolioId,
    instrument_id: &InstrumentId,
) -> Result<Vec<Trade>, PersistenceError> {
    let rows = sqlx::query(POSITION_TRADES_SQL)
        .bind(portfolio_id.as_str())
        .bind(instrument_id.as_str())
        .fetch_all(pool)
        .await?;

    rows.iter().map(trade_from_row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;

    /// The `detail` column of `EXPLAIN QUERY PLAN`, one line per plan step.
    async fn query_plan(pool: &SqlitePool, sql: &str, binds: &[&str]) -> Vec<String> {
        let explain = format!("EXPLAIN QUERY PLAN {sql}");
        let mut query = sqlx::query(&explain);
        for bind in binds {
            query = query.bind(*bind);
        }

        query
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .map(|row| row.get::<String, _>("detail"))
            .collect()
    }

    async fn migrated_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = crate::persistence::connect(&crate::persistence::PersistenceConfig::for_tests(
            dir.path().join("plan.db"),
        ))
        .await
        .unwrap();
        (dir, pool)
    }

    #[tokio::test]
    async fn position_query_seeks_on_both_leading_index_columns() {
        let (_dir, pool) = migrated_pool().await;
        let plan = query_plan(&pool, POSITION_TRADES_SQL, &["P001", "I001"]).await;

        assert!(
            plan.iter().any(|step| step
                .contains("USING INDEX idx_trades_position (portfolio_id=? AND instrument_id=?)")),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|step| step.contains("TEMP B-TREE")),
            "{plan:?}"
        );
    }

    #[tokio::test]
    async fn portfolio_query_uses_index_for_lookup_and_ordering() {
        let (_dir, pool) = migrated_pool().await;
        let plan = query_plan(&pool, PORTFOLIO_TRADES_SQL, &["P001"]).await;

        assert!(
            plan.iter()
                .any(|step| step.contains("USING INDEX idx_trades_position (portfolio_id=?)")),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|step| step.contains("TEMP B-TREE")),
            "{plan:?}"
        );
    }

    #[tokio::test]
    async fn trade_lookup_uses_primary_key() {
        let (_dir, pool) = migrated_pool().await;
        let plan = query_plan(&pool, FIND_TRADE_SQL, &["T1"]).await;

        assert!(
            plan.iter().any(|step| step.contains("(trade_id=?)")),
            "{plan:?}"
        );
    }
}
