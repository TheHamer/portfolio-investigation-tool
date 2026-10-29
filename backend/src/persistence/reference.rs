//! SQL for the reference tables: portfolios, instruments, prices and FX rates.

use std::collections::HashSet;

use chrono::NaiveDate;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection, SqlitePool};

use super::PersistenceError;
use crate::domain::{
    Currency, FxRate, Instrument, InstrumentId, Portfolio, PortfolioId, Price, parse_date,
};

// --- Writes (used by the reference-data loader inside one transaction) -------

pub async fn upsert_portfolio(
    conn: &mut SqliteConnection,
    p: &Portfolio,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "INSERT INTO portfolios (portfolio_id, name, base_currency) VALUES (?, ?, ?)
         ON CONFLICT (portfolio_id) DO UPDATE
         SET name = excluded.name, base_currency = excluded.base_currency",
    )
    .bind(p.portfolio_id.as_str())
    .bind(&p.name)
    .bind(p.base_currency.as_str())
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn upsert_instrument(
    conn: &mut SqliteConnection,
    i: &Instrument,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "INSERT INTO instruments (instrument_id, ticker, name, currency, sector)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (instrument_id) DO UPDATE
         SET ticker = excluded.ticker, name = excluded.name,
             currency = excluded.currency, sector = excluded.sector",
    )
    .bind(i.instrument_id.as_str())
    .bind(&i.ticker)
    .bind(&i.name)
    .bind(i.currency.as_str())
    .bind(&i.sector)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn upsert_price(
    conn: &mut SqliteConnection,
    instrument_id: &InstrumentId,
    date: NaiveDate,
    price: Price,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "INSERT INTO prices (instrument_id, date, close_price) VALUES (?, ?, ?)
         ON CONFLICT (instrument_id, date) DO UPDATE SET close_price = excluded.close_price",
    )
    .bind(instrument_id.as_str())
    .bind(date.to_string())
    .bind(price.value().to_string())
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn upsert_fx_rate(
    conn: &mut SqliteConnection,
    currency: Currency,
    date: NaiveDate,
    rate: FxRate,
) -> Result<(), PersistenceError> {
    sqlx::query(
        "INSERT INTO fx_rates (currency, date, gbp_per_unit) VALUES (?, ?, ?)
         ON CONFLICT (currency, date) DO UPDATE SET gbp_per_unit = excluded.gbp_per_unit",
    )
    .bind(currency.as_str())
    .bind(date.to_string())
    .bind(rate.value().to_string())
    .execute(conn)
    .await?;
    Ok(())
}

// --- Reads --------------------------------------------------------------------

pub async fn portfolio_ids(pool: &SqlitePool) -> Result<HashSet<PortfolioId>, PersistenceError> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT portfolio_id FROM portfolios")
        .fetch_all(pool)
        .await?;

    Ok(ids
        .iter()
        .map(|id| PortfolioId::parse(id))
        .collect::<Result<_, _>>()?)
}

pub async fn instrument_ids(pool: &SqlitePool) -> Result<HashSet<InstrumentId>, PersistenceError> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT instrument_id FROM instruments")
        .fetch_all(pool)
        .await?;

    Ok(ids
        .iter()
        .map(|id| InstrumentId::parse(id))
        .collect::<Result<_, _>>()?)
}

fn portfolio_from_row(row: &SqliteRow) -> Result<Portfolio, PersistenceError> {
    Ok(Portfolio {
        portfolio_id: PortfolioId::parse(row.try_get("portfolio_id")?)?,
        name: row.try_get("name")?,
        base_currency: row.try_get::<&str, _>("base_currency")?.parse()?,
    })
}

fn instrument_from_row(row: &SqliteRow) -> Result<Instrument, PersistenceError> {
    Ok(Instrument {
        instrument_id: InstrumentId::parse(row.try_get("instrument_id")?)?,
        ticker: row.try_get("ticker")?,
        name: row.try_get("name")?,
        currency: row.try_get::<&str, _>("currency")?.parse()?,
        sector: row.try_get("sector")?,
    })
}

/// All portfolios with the number of stored trades in each.
pub async fn list_portfolios(pool: &SqlitePool) -> Result<Vec<(Portfolio, i64)>, PersistenceError> {
    let rows = sqlx::query(
        "SELECT p.portfolio_id, p.name, p.base_currency, COUNT(t.trade_id) AS trade_count
         FROM portfolios p LEFT JOIN trades t ON t.portfolio_id = p.portfolio_id
         GROUP BY p.portfolio_id ORDER BY p.portfolio_id",
    )
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| Ok((portfolio_from_row(row)?, row.try_get("trade_count")?)))
        .collect()
}

pub async fn get_portfolio(
    pool: &SqlitePool,
    id: &PortfolioId,
) -> Result<Option<Portfolio>, PersistenceError> {
    let row = sqlx::query(
        "SELECT portfolio_id, name, base_currency FROM portfolios WHERE portfolio_id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(portfolio_from_row).transpose()
}

pub async fn get_instrument(
    pool: &SqlitePool,
    id: &InstrumentId,
) -> Result<Option<Instrument>, PersistenceError> {
    let row = sqlx::query(
        "SELECT instrument_id, ticker, name, currency, sector FROM instruments WHERE instrument_id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;

    row.as_ref().map(instrument_from_row).transpose()
}

pub async fn instruments(pool: &SqlitePool) -> Result<Vec<Instrument>, PersistenceError> {
    let rows = sqlx::query("SELECT instrument_id, ticker, name, currency, sector FROM instruments")
        .fetch_all(pool)
        .await?;

    rows.iter().map(instrument_from_row).collect()
}

/// Close prices for the given instruments dated on or before `as_of`.
/// Nothing after `as_of` is returned, so later lookups cannot look forward.
pub async fn prices_up_to(
    pool: &SqlitePool,
    instrument_ids: &HashSet<InstrumentId>,
    as_of: NaiveDate,
) -> Result<Vec<(InstrumentId, NaiveDate, Price)>, PersistenceError> {
    // The instrument list is bound as one JSON array parameter and expanded by
    // json_each, which keeps the query parameterised for any number of IDs.
    let ids: Vec<&str> = instrument_ids.iter().map(InstrumentId::as_str).collect();
    let rows = sqlx::query(
        "SELECT instrument_id, date, close_price FROM prices
         WHERE instrument_id IN (SELECT value FROM json_each(?1)) AND date <= ?2",
    )
    .bind(serde_json::to_string(&ids)?)
    .bind(as_of.to_string())
    .fetch_all(pool)
    .await?;

    rows.iter()
        .map(|row| {
            Ok((
                InstrumentId::parse(row.try_get("instrument_id")?)?,
                parse_date(row.try_get("date")?, "date")?,
                Price::parse(row.try_get("close_price")?)?,
            ))
        })
        .collect()
}

/// All FX rates dated on or before `as_of`.
pub async fn fx_rates_up_to(
    pool: &SqlitePool,
    as_of: NaiveDate,
) -> Result<Vec<(Currency, NaiveDate, FxRate)>, PersistenceError> {
    let rows = sqlx::query("SELECT currency, date, gbp_per_unit FROM fx_rates WHERE date <= ?")
        .bind(as_of.to_string())
        .fetch_all(pool)
        .await?;

    rows.iter()
        .map(|row| {
            Ok((
                row.try_get::<&str, _>("currency")?.parse()?,
                parse_date(row.try_get("date")?, "date")?,
                FxRate::parse(row.try_get("gbp_per_unit")?)?,
            ))
        })
        .collect()
}
