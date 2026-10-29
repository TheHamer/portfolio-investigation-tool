//! The import's write transaction against a real SQLite database: a database
//! failure after earlier inserts leaves nothing behind, and concurrent uploads
//! that disagree about a new trade commit exactly one of them.

mod common;

use common::{start, trade_snapshot};
use portfolio_investigation::AppState;
use portfolio_investigation::import::{ImportError, ImportReport, Outcome, import_trades};
use tempfile::TempDir;

const HEADER: &str = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n";

async fn import(state: &AppState, csv: &str) -> Result<ImportReport, ImportError> {
    import_trades(
        &state.pool,
        state.manifest.valuation_dates(),
        csv.as_bytes(),
    )
    .await
}

async fn execute(state: &AppState, sql: &str) {
    sqlx::query(sql).execute(&state.pool).await.unwrap();
}

/// The second insert of a valid two-trade file fails inside the database
/// (a test-only trigger, created in this temporary database only). The first
/// insert has already run in the same transaction; it must be rolled back,
/// and the database must be fully usable afterwards.
#[tokio::test]
async fn database_failure_after_an_insert_commits_none_of_the_file() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let existing = format!("{HEADER}E1,P001,I001,2026-01-05T10:00:00Z,BUY,5,100\n");
    import(&state, &existing).await.unwrap();
    let before = trade_snapshot(&state).await;

    execute(
        &state,
        "CREATE TRIGGER fail_second_insert BEFORE INSERT ON trades
         WHEN NEW.trade_id = 'N2'
         BEGIN SELECT RAISE(ABORT, 'injected failure on N2'); END",
    )
    .await;
    // New trades are inserted in file order, so N1 is written before N2 fails.
    let file = format!(
        "{HEADER}\
         N1,P001,I001,2026-01-06T10:00:00Z,BUY,10,100\n\
         N2,P001,I002,2026-01-06T11:00:00Z,BUY,20,50\n"
    );

    let result = import(&state, &file).await;

    let error = result.expect_err("a failed insert is an operational error, not a report");
    assert!(matches!(error, ImportError::Persistence(_)), "{error}");
    assert!(
        error.to_string().contains("injected failure on N2"),
        "{error}"
    );
    assert_eq!(trade_snapshot(&state).await, before);

    // The failed transaction released its write lock: once the fault is
    // removed, the same file imports in full.
    execute(&state, "DROP TRIGGER fail_second_insert").await;
    let retry = import(&state, &file).await.unwrap();
    assert_eq!(retry.outcome, Outcome::Success);
    assert_eq!(retry.inserted_rows, Some(2));
    assert_eq!(trade_snapshot(&state).await.len(), before.len() + 2);
}

/// Two uploads each bring a new trade K1, with different quantities, plus a
/// trade of their own. Whichever commits first wins; the other then sees K1
/// stored with different values and rejects its whole file, its own extra
/// trade included. Which upload wins is left to the scheduler.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_conflicting_uploads_commit_exactly_one_file() {
    let dir = TempDir::new().unwrap();
    let state = start(&dir).await;
    let file_a = format!(
        "{HEADER}\
         K1,P001,I001,2026-01-05T10:00:00Z,BUY,10,100\n\
         A2,P001,I002,2026-01-05T10:00:00Z,BUY,1,50\n"
    );
    let file_b = format!(
        "{HEADER}\
         K1,P001,I001,2026-01-05T10:00:00Z,BUY,20,100\n\
         B2,P002,I002,2026-01-05T10:00:00Z,BUY,2,50\n"
    );

    let spawn_upload = |file: String| {
        let state = state.clone();
        tokio::spawn(async move { import(&state, &file).await })
    };
    let (a, b) = (spawn_upload(file_a), spawn_upload(file_b));
    let a = a
        .await
        .unwrap()
        .expect("no SQLite lock or other database error");
    let b = b
        .await
        .unwrap()
        .expect("no SQLite lock or other database error");

    let (winner, loser, winner_trades) = match (a.outcome, b.outcome) {
        (Outcome::Success, Outcome::Rejected) => (a, b, ["A2", "K1"]),
        (Outcome::Rejected, Outcome::Success) => (b, a, ["B2", "K1"]),
        outcomes => panic!("expected exactly one success, got {outcomes:?}"),
    };
    assert_eq!(winner.inserted_rows, Some(2));
    assert_eq!((loser.invalid_rows, loser.aborted_rows), (Some(1), Some(1)));
    assert_eq!(loser.errors[0].trade_id.as_deref(), Some("K1"));
    assert!(
        loser.errors[0].messages[0].contains("already exists with different values"),
        "{:?}",
        loser.errors
    );

    let stored = trade_snapshot(&state).await;
    let stored_ids: Vec<&str> = stored.iter().map(|t| t.0.as_str()).collect();
    assert_eq!(stored_ids, winner_trades);

    let k1_quantity = stored.iter().find(|t| t.0 == "K1").unwrap().5;
    let winner_quantity = if winner_trades[0] == "A2" { 10 } else { 20 };
    assert_eq!(k1_quantity, winner_quantity);
}
