//! Exercises the supplied upload fixtures in the order given by
//! DATA_CONTRACT.md, plus atomicity, idempotency, concurrency and restart.

mod common;

use std::collections::BTreeSet;

use common::{count, fixture, start, trade_snapshot};
use portfolio_investigation::AppState;
use portfolio_investigation::import::{ImportReport, Outcome, import_trades};
use tempfile::TempDir;

async fn upload(state: &AppState, bytes: &[u8]) -> ImportReport {
    import_trades(&state.pool, state.manifest.valuation_dates(), bytes)
        .await
        .unwrap()
}

fn manifest_count(state: &AppState, file: &str) -> usize {
    state
        .manifest
        .expected_file_count(&format!("uploads/{file}"))
        .expect("manifest lists the fixture")
}

fn assert_success(report: &ImportReport, total: usize, inserted: usize, skipped: usize) {
    assert_eq!(report.outcome, Outcome::Success, "{report:#?}");
    assert_eq!(report.total_rows, Some(total));
    assert_eq!(report.inserted_rows, Some(inserted));
    assert_eq!(report.skipped_rows, Some(skipped));
    assert_eq!(report.invalid_rows, Some(0));
    assert_eq!(report.aborted_rows, Some(0));
    assert!(report.errors.is_empty());
}

#[tokio::test]
async fn supplied_fixture_sequence() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;

    // 1. Initial: every row is new.
    let n_initial = manifest_count(&state, "trades_initial.csv");
    let report = upload(&state, &fixture("trades_initial.csv")).await;
    assert_success(&report, n_initial, n_initial, 0);
    assert_eq!(count(&state, "trades").await, n_initial as i64);

    // 2. Repeat (byte-identical): every row is an exact duplicate.
    let report = upload(&state, &fixture("trades_repeat.csv")).await;
    assert_success(&report, n_initial, 0, n_initial);
    assert_eq!(count(&state, "trades").await, n_initial as i64);

    // 3. Incremental: new trades only.
    let n_incremental = manifest_count(&state, "trades_incremental.csv");
    let report = upload(&state, &fixture("trades_incremental.csv")).await;
    assert_success(&report, n_incremental, n_incremental, 0);
    let after_incremental = (n_initial + n_incremental) as i64;
    assert_eq!(count(&state, "trades").await, after_incremental);

    // 4. Invalid: rejected as a whole, stored trades unchanged.
    let before = trade_snapshot(&state).await;
    let report = upload(&state, &fixture("trades_invalid.csv")).await;
    assert_eq!(report.outcome, Outcome::Rejected);
    assert_eq!(report.total_rows, Some(10));
    assert_eq!(report.inserted_rows, Some(0));
    assert_eq!(report.skipped_rows, Some(0));
    assert_eq!(report.invalid_rows, Some(8));
    assert_eq!(report.aborted_rows, Some(2));

    let invalid_rows: Vec<usize> = report.errors.iter().map(|e| e.row.unwrap()).collect();
    assert_eq!(invalid_rows, vec![3, 5, 6, 7, 8, 9, 10, 11]);
    let message_for = |row: usize| {
        report
            .errors
            .iter()
            .find(|e| e.row == Some(row))
            .unwrap()
            .messages
            .join(" | ")
    };
    assert!(message_for(3).contains("portfolio_id 'P999' does not exist"));
    assert!(message_for(5).contains("2026-03-28 is not a valuation date"));
    assert!(message_for(6).contains("trade_id 'T376158' already exists with different values"));
    assert!(message_for(7).contains("side 'HOLD'"));
    assert!(message_for(8).contains("greater than zero"));
    assert!(message_for(9).contains("YYYY-MM-DDTHH:MM:SSZ"));
    assert!(message_for(10).contains("instrument_id 'I999' does not exist"));
    assert!(message_for(11).contains("price '-20.7558'"));

    assert_eq!(trade_snapshot(&state).await, before);

    // 5. Corrected: all ten rows are new (the invalid upload inserted nothing,
    //    and the conflicting trade now has a fresh ID).
    let n_corrected = manifest_count(&state, "trades_corrected.csv");
    let report = upload(&state, &fixture("trades_corrected.csv")).await;
    assert_success(&report, n_corrected, n_corrected, 0);
    let final_count = after_incremental + n_corrected as i64;
    assert_eq!(count(&state, "trades").await, final_count);

    // Uploading every file again changes nothing (the invalid one is still rejected).
    for file in [
        "trades_initial.csv",
        "trades_incremental.csv",
        "trades_corrected.csv",
    ] {
        let report = upload(&state, &fixture(file)).await;
        assert_eq!(report.inserted_rows, Some(0), "{file}");
    }

    assert_eq!(
        upload(&state, &fixture("trades_invalid.csv")).await.outcome,
        Outcome::Rejected
    );
    assert_eq!(count(&state, "trades").await, final_count);

    // Restart: reopen the same database file and reload reference data.
    let snapshot = trade_snapshot(&state).await;
    state.pool.close().await;
    let restarted = start(&dir).await;

    assert_eq!(trade_snapshot(&restarted).await, snapshot);
    assert_eq!(count(&restarted, "portfolios").await, 3);
    assert_eq!(count(&restarted, "instruments").await, 24);
    assert_eq!(count(&restarted, "prices").await, 1436);
    assert_eq!(count(&restarted, "fx_rates").await, 178);
}

#[tokio::test]
async fn rejected_upload_inserts_none_of_its_valid_rows() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let file = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n\
                NEW1,P001,I001,2026-01-05T10:00:00Z,BUY,10,100\n\
                NEW2,P001,I001,2026-01-06T10:00:00Z,BUY,10,100\n\
                BAD1,P001,I001,2026-01-06T10:00:00Z,BUY,10,0\n";

    let report = upload(&state, file.as_bytes()).await;
    assert_eq!(report.outcome, Outcome::Rejected);
    assert_eq!(
        (report.invalid_rows, report.aborted_rows),
        (Some(1), Some(2))
    );
    assert_eq!(count(&state, "trades").await, 0);
}

#[tokio::test]
async fn numeric_normalisation_when_comparing_with_stored_trades() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let header = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n";
    let first = format!("{header}X1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.0\n");
    let same = format!("{header}X1,P001,I001,2026-01-05T10:00:00Z,BUY,010,10.0000\n");
    let different = format!("{header}X1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.0001\n");

    assert_success(&upload(&state, first.as_bytes()).await, 1, 1, 0);
    assert_success(&upload(&state, same.as_bytes()).await, 1, 0, 1);

    let report = upload(&state, different.as_bytes()).await;
    assert_eq!(report.outcome, Outcome::Rejected);
    assert!(report.errors[0].messages[0].contains("price stored 10 vs file 10.0001"));
}

#[tokio::test]
async fn ids_are_case_sensitive() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let file = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n\
                abc,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n\
                ABC,P001,I001,2026-01-05T10:00:00Z,BUY,20,10\n\
                X2,p001,I001,2026-01-05T10:00:00Z,BUY,20,10\n";

    let report = upload(&state, file.as_bytes()).await;
    // abc/ABC are different trades; lowercase p001 is an unknown portfolio.
    let invalid: BTreeSet<usize> = report.errors.iter().map(|e| e.row.unwrap()).collect();
    assert_eq!(invalid, BTreeSet::from([4]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_uploads_of_the_same_file_insert_each_trade_once() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let bytes = fixture("trades_initial.csv");
    let n = manifest_count(&state, "trades_initial.csv");

    let spawn_upload = || {
        let (state, bytes) = (state.clone(), bytes.clone());
        tokio::spawn(async move { upload(&state, &bytes).await })
    };
    let (a, b) = (spawn_upload(), spawn_upload());
    let (a, b) = (a.await.unwrap(), b.await.unwrap());

    assert_eq!(a.outcome, Outcome::Success);
    assert_eq!(b.outcome, Outcome::Success);
    assert_eq!(a.inserted_rows.unwrap() + b.inserted_rows.unwrap(), n);
    assert_eq!(a.skipped_rows.unwrap() + b.skipped_rows.unwrap(), n);
    assert_eq!(count(&state, "trades").await, n as i64);
}

const HEADER: &str = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n";

#[tokio::test]
async fn identifiers_are_stored_exactly_including_surrounding_whitespace() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    // The contract only requires non-empty, case-sensitive IDs, so " W1" and
    // "W1" are two different trades. (Reference IDs must still match
    // exactly: " P001" is not the portfolio "P001".)
    let file = format!(
        "{HEADER}W1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n\
         \" W1\",P001,I001,2026-01-05T10:00:00Z,BUY,20,10\n\
         W2,\" P001\",I001,2026-01-05T10:00:00Z,BUY,20,10\n"
    );

    let report = upload(&state, file.as_bytes()).await;
    assert_eq!(report.outcome, Outcome::Rejected);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].row, Some(4));
    assert!(report.errors[0].messages[0].contains("portfolio_id ' P001' does not exist"));

    let valid = format!(
        "{HEADER}W1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n\
         \" W1\",P001,I001,2026-01-05T10:00:00Z,BUY,20,10\n"
    );
    assert_success(&upload(&state, valid.as_bytes()).await, 2, 2, 0);

    let ids: Vec<String> = trade_snapshot(&state)
        .await
        .into_iter()
        .map(|t| t.0)
        .collect();
    assert_eq!(ids, vec![" W1".to_string(), "W1".to_string()]);

    // Re-uploading is an exact duplicate of each, not a conflict between them.
    assert_success(&upload(&state, valid.as_bytes()).await, 2, 0, 2);
}

#[tokio::test]
async fn precision_is_enforced_on_input_text_but_duplicates_compare_numerically() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let five_dp = format!("{HEADER}D1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.00000\n");

    let report = upload(&state, five_dp.as_bytes()).await;
    assert_eq!(report.outcome, Outcome::Rejected);
    assert_eq!(
        report.errors[0].messages,
        vec!["price '10.00000' has more than 4 decimal places".to_string()]
    );
    assert_eq!(count(&state, "trades").await, 0);

    let first = format!("{HEADER}D1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.0\n");
    let same_value = format!("{HEADER}D1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.0000\n");

    assert_success(&upload(&state, first.as_bytes()).await, 1, 1, 0);
    assert_success(&upload(&state, same_value.as_bytes()).await, 1, 0, 1);
}

#[tokio::test]
async fn quantities_above_u32_are_imported_and_read_back() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let file = format!(
        "{HEADER}Q1,P001,I002,2026-01-05T10:00:00Z,BUY,5000000000,1\n\
         Q2,P001,I002,2026-01-05T11:00:00Z,BUY,9223372036854775807,1\n"
    );

    assert_success(&upload(&state, file.as_bytes()).await, 2, 2, 0);

    let stored: Vec<i64> = trade_snapshot(&state)
        .await
        .into_iter()
        .map(|t| t.5)
        .collect();
    assert_eq!(stored, vec![5_000_000_000, i64::MAX]);

    let too_large =
        format!("{HEADER}Q3,P001,I002,2026-01-05T10:00:00Z,BUY,9223372036854775808,1\n");

    let report = upload(&state, too_large.as_bytes()).await;
    assert_eq!(report.outcome, Outcome::Rejected);
    assert!(report.errors[0].messages[0].contains("too large"));
}
