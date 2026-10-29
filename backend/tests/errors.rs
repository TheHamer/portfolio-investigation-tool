//! Error propagation from the subsystems to `AppError`, and from `AppError`
//! through the HTTP boundary, using real failures rather than constructed
//! values.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use portfolio_investigation::build_router;
use portfolio_investigation::error::AppError;
use portfolio_investigation::import::{ImportError, Outcome, import_trades};
use portfolio_investigation::initialise;
use portfolio_investigation::manifest::ManifestError;
use portfolio_investigation::persistence::PersistenceError;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

#[tokio::test]
async fn missing_manifest_is_a_manifest_error() {
    let dir = TempDir::new().unwrap();
    let config = common::load_config(
        &dir,
        &dir.path().join("test.db"),
        &dir.path().join("no-such-data/manifest.json"),
        &common::dataset_dir().join("reference"),
    );

    let error = initialise(&config).await.err().unwrap();
    assert!(
        matches!(&error, AppError::Manifest(ManifestError::Read { path, .. })
            if path.ends_with("no-such-data/manifest.json")),
        "{error}"
    );
}

#[tokio::test]
async fn unusable_database_location_is_a_persistence_error() {
    let dir = TempDir::new().unwrap();
    // The database's parent "directory" is a regular file.
    let blocker = dir.path().join("not-a-directory");
    std::fs::write(&blocker, "x").unwrap();
    let config = common::load_config(
        &dir,
        &blocker.join("portfolio.db"),
        &common::dataset_dir().join("manifest.json"),
        &common::dataset_dir().join("reference"),
    );

    let error = initialise(&config).await.err().unwrap();
    assert!(
        matches!(
            &error,
            AppError::Persistence(PersistenceError::CreateDirectory { .. })
        ),
        "{error}"
    );
    assert!(error.to_string().contains("not-a-directory"), "{error}");
}

#[tokio::test]
async fn database_failure_during_import_is_an_import_error_not_a_rejection() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    state.pool.close().await;

    let result = import_trades(
        &state.pool,
        state.manifest.valuation_dates(),
        &common::fixture("trades_initial.csv"),
    )
    .await;
    let error = result.expect_err("a closed database is an error, not an ImportReport");
    assert!(matches!(error, ImportError::Persistence(_)), "{error}");
    assert!(matches!(AppError::from(error), AppError::Import(_)));
}

/// A file that cannot be read as trade rows is rejected before the database
/// is touched, so the user gets the file-level reason even while the database
/// is unavailable. (A readable file needs the database, and fails as above.)
#[tokio::test]
async fn unreadable_file_is_rejected_before_the_database_is_used() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    state.pool.close().await;

    let result = import_trades(
        &state.pool,
        state.manifest.valuation_dates(),
        b"id,portfolio_id\nT1,P001\n",
    )
    .await;

    let report = result.expect("a bad header is a rejected report, not an error");
    assert_eq!(report.outcome, Outcome::Rejected);
    assert_eq!(report.total_rows, None);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].row, None);
    assert!(
        report.errors[0].messages[0].starts_with("The header must be exactly"),
        "{:?}",
        report.errors
    );
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn database_failures_become_generic_500s_over_http() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    let pool = state.pool.clone();
    let app = build_router(state, None);

    // Expected outcome first: a missing portfolio is still a 404.
    let (status, body) = send(
        &app,
        Request::get("/api/portfolios/P999/positions")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["message"], "Portfolio 'P999' does not exist.");

    pool.close().await;

    let expected = serde_json::json!({
        "error": { "code": "internal_error", "message": "An internal error occurred. Please try again." }
    });
    for uri in [
        "/api/portfolios",
        "/api/portfolios/P001/positions",
        "/api/portfolios/P001/positions/I001",
    ] {
        let (status, body) = send(&app, Request::get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
        assert_eq!(body, expected, "{uri}");
    }

    // An upload that hits a database failure is a 500, not a 422 rejection.
    let boundary = "b";
    let csv = common::fixture("trades_initial.csv");
    let mut upload = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"t.csv\"\r\n\r\n"
    )
    .into_bytes();
    upload.extend_from_slice(&csv);
    upload.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let request = Request::post("/api/trades/import")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(upload))
        .unwrap();

    let (status, body) = send(&app, request).await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body, expected);
}
