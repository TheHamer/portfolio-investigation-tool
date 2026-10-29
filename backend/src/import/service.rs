//! Import orchestration: parse and validate the upload, reconcile it with the
//! stored trades inside one write transaction, then commit or roll back.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::NaiveDate;
use sqlx::SqlitePool;

use super::error::{FieldDifference, ImportError, RowValidationError};
use super::parsing::read_trade_csv;
use super::report::ImportReport;
use super::validation::{
    ImportContext, ParsedRow, group_by_trade_id, mark_in_file_conflicts, validate_rows,
};
use crate::domain::{Trade, TradeId};
use crate::persistence::{PersistenceError, reference as reference_db, trades as trades_db};

/// Runs the whole import. Database errors propagate as `Err`; every
/// validation outcome, including rejection, is an `Ok(ImportReport)`.
///
/// A file that cannot be read as trade rows (encoding, header, CSV syntax,
/// no data rows) is rejected before the database is touched, so such a file
/// is reported as rejected even when the database is unavailable.
pub async fn import_trades(
    pool: &SqlitePool,
    valuation_dates: &BTreeSet<NaiveDate>,
    bytes: &[u8],
) -> Result<ImportReport, ImportError> {
    let raw_rows = match read_trade_csv(bytes) {
        Ok(rows) => rows,
        Err(error) => return Ok(ImportReport::file_rejected(error)),
    };

    // Reference data is fixed after startup, so reading it outside the write
    // transaction is safe; foreign keys remain the final guarantee.
    let context = ImportContext {
        portfolios: reference_db::portfolio_ids(pool).await?,
        instruments: reference_db::instrument_ids(pool).await?,
        valuation_dates: valuation_dates.clone(),
    };

    let mut rows = validate_rows(raw_rows, &context);
    mark_in_file_conflicts(&mut rows);

    // Holds SQLite's write lock from here, so the duplicate check below and
    // the inserts cannot interleave with another upload.
    let mut tx = trades_db::begin_import(pool).await?;

    let incoming_ids: HashSet<&TradeId> = rows
        .iter()
        .filter_map(|row| row.trade.as_ref())
        .map(|trade| &trade.trade_id)
        .collect();

    let mut stored = HashMap::new();
    for id in incoming_ids {
        if let Some(trade) = trades_db::find_trade(&mut tx, id).await? {
            stored.insert(id.clone(), trade);
        }
    }

    let new_trades = reconcile_with_stored(&mut rows, &stored);

    // Any invalid row rejects the whole file; nothing has been written yet.
    if rows.iter().any(|row| !row.is_valid()) {
        tx.rollback().await.map_err(PersistenceError::from)?;
        return Ok(ImportReport::rows_rejected(&rows));
    }

    for trade in &new_trades {
        trades_db::insert_trade(&mut tx, trade).await?;
    }

    // If any insert above fails, `tx` is dropped without commit and SQLite
    // rolls the whole transaction back.
    tx.commit().await.map_err(PersistenceError::from)?;

    Ok(ImportReport::succeeded(rows.len(), new_trades.len()))
}

/// Reconciles the file's valid trade IDs with the stored trades and returns
/// the trades to insert, one per trade ID not yet stored.
///
/// A stored trade with identical values (compared numerically, so `10.0`
/// equals `10.0000`) makes the incoming rows exact duplicates: they are
/// skipped. A stored trade with different values is never overwritten:
/// every incoming row with that ID gets a conflict error naming the
/// differing fields. IDs whose rows are already invalid are left alone.
fn reconcile_with_stored(rows: &mut [ParsedRow], stored: &HashMap<TradeId, Trade>) -> Vec<Trade> {
    let mut new_trades = Vec::new();

    for group in group_by_trade_id(rows) {
        if !group.iter().all(|&i| rows[i].is_valid()) {
            continue;
        }

        // The group is valid, so every row holds the same parsed trade.
        let Some(incoming) = rows[group[0]].trade.clone() else {
            continue;
        };

        match stored.get(&incoming.trade_id) {
            None => new_trades.push(incoming),
            Some(existing) if *existing == incoming => {}
            Some(existing) => {
                let conflict = RowValidationError::ConflictsWithStoredTrade {
                    trade_id: incoming.trade_id.clone(),
                    differences: differing_fields(existing, &incoming),
                };
                for &i in &group {
                    rows[i].errors.push(conflict.clone());
                }
            }
        }
    }

    new_trades
}

/// The fields that differ, each with both values in canonical form.
fn differing_fields(stored: &Trade, incoming: &Trade) -> Vec<FieldDifference> {
    let mut fields = Vec::new();
    let mut compare = |field: &'static str, stored: String, incoming: String| {
        if stored != incoming {
            fields.push(FieldDifference {
                field,
                stored,
                incoming,
            });
        }
    };

    compare(
        "portfolio_id",
        stored.portfolio_id.to_string(),
        incoming.portfolio_id.to_string(),
    );
    compare(
        "instrument_id",
        stored.instrument_id.to_string(),
        incoming.instrument_id.to_string(),
    );
    compare(
        "timestamp",
        stored.timestamp.to_canonical(),
        incoming.timestamp.to_canonical(),
    );
    compare(
        "side",
        stored.side.as_str().into(),
        incoming.side.as_str().into(),
    );
    compare(
        "quantity",
        stored.quantity.get().to_string(),
        incoming.quantity.get().to_string(),
    );
    compare(
        "price",
        stored.price.value().normalize().to_string(),
        incoming.price.value().normalize().to_string(),
    );

    fields
}

#[cfg(test)]
mod tests {
    use super::super::test_support::parse;
    use super::*;

    #[test]
    fn repeats_within_a_file_are_inserted_once() {
        let mut rows = parse(concat!(
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.0\n",
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.0000\n",
        ))
        .unwrap();
        mark_in_file_conflicts(&mut rows);

        // Inserted once; the second row counts as skipped.
        let planned = reconcile_with_stored(&mut rows, &HashMap::new());
        assert_eq!(planned, vec![rows[0].trade.clone().unwrap()]);
    }

    #[test]
    fn stored_trades_decide_skip_or_conflict() {
        let mut rows = parse(concat!(
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.00\n",
            "T2,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T3,P001,I001,2026-01-05T10:00:00Z,SELL,5,12\n",
            "T2,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
        ))
        .unwrap();
        mark_in_file_conflicts(&mut rows);

        let stored_t1 = parse("T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n").unwrap()[0]
            .trade
            .clone()
            .unwrap();
        let stored_t2 = parse("T2,P002,I001,2026-01-05T10:00:00Z,BUY,10,10.5\n").unwrap()[0]
            .trade
            .clone()
            .unwrap();
        let stored = HashMap::from([
            (stored_t1.trade_id.clone(), stored_t1),
            (stored_t2.trade_id.clone(), stored_t2),
        ]);

        let to_insert = reconcile_with_stored(&mut rows, &stored);

        // T1 is an exact duplicate of a stored trade (10.00 == 10) and is skipped.
        assert!(rows[0].is_valid());
        // Both rows of T2 conflict with the stored T2.
        assert!(!rows[1].is_valid() && !rows[3].is_valid());
        match &rows[1].errors[..] {
            [
                RowValidationError::ConflictsWithStoredTrade {
                    trade_id,
                    differences,
                },
            ] => {
                assert_eq!(trade_id.as_str(), "T2");
                let fields: Vec<(&str, &str, &str)> = differences
                    .iter()
                    .map(|d| (d.field, d.stored.as_str(), d.incoming.as_str()))
                    .collect();
                assert_eq!(
                    fields,
                    vec![("portfolio_id", "P002", "P001"), ("price", "10.5", "10")]
                );
            }
            other => panic!("unexpected errors: {other:?}"),
        }
        assert_eq!(
            rows[1].errors[0].to_string(),
            concat!(
                "trade_id 'T2' already exists with different values: ",
                "portfolio_id stored P002 vs file P001; price stored 10.5 vs file 10. ",
                "Stored trades are never overwritten; use a new trade_id"
            )
        );

        // Only T3 would be inserted (if the file were otherwise valid).
        let ids: Vec<&str> = to_insert.iter().map(|t| t.trade_id.as_str()).collect();
        assert_eq!(ids, vec!["T3"]);
    }
}
