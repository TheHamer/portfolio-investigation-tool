//! The configurable upload limit (`[api] max_upload_bytes`), through the real
//! router. A deliberately small limit keeps the request bodies tiny.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::multipart_upload;
use http_body_util::BodyExt;
use portfolio_investigation::build_router;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

/// Not a whole number of KiB, so the message must state it in bytes.
const LIMIT: usize = 1000;

const HEADER: &str = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n";

/// A valid trades file with `rows` rows (I002 is a GBP instrument and
/// 2026-01-05 is a valuation date).
fn trades_csv(rows: usize) -> Vec<u8> {
    let mut csv = HEADER.to_owned();
    for i in 0..rows {
        csv.push_str(&format!(
            "TLIMIT{i},P001,I002,2026-01-05T10:00:00Z,BUY,10,100\n"
        ));
    }
    csv.into_bytes()
}

/// Status, `x-request-id` and JSON body.
async fn send(app: &Router, request: Request<Body>) -> (StatusCode, String, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let id = common::request_id(&response);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, id, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn upload_over_the_configured_limit_is_rejected_and_commits_nothing() {
    let dir = TempDir::new().unwrap();
    let state = common::start_with_upload_limit(&dir, LIMIT).await;
    let app = build_router(state.clone(), None);

    // A valid file that would import if it were small enough.
    let csv = trades_csv(30);
    assert!(csv.len() > LIMIT);
    let (status, id, body) = send(&app, multipart_upload("big.csv", &csv)).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body,
        serde_json::json!({ "error": {
            "code": "payload_too_large",
            "message": "The upload request exceeds the maximum size of 1000 bytes."
        } })
    );
    assert!(common::is_uuid_v4(&id), "{id}");
    assert_eq!(common::count(&state, "trades").await, 0);
}

#[tokio::test]
async fn the_limit_covers_the_whole_multipart_request() {
    let dir = TempDir::new().unwrap();
    let state = common::start_with_upload_limit(&dir, LIMIT).await;
    let app = build_router(state.clone(), None);

    // The file alone is under the limit, but boundaries and part headers push
    // the request body over it.
    let mut csv = trades_csv(1);
    csv.resize(LIMIT - 10, b'\n');
    let (status, _, body) = send(&app, multipart_upload("almost.csv", &csv)).await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"]["code"], "payload_too_large");
    assert_eq!(common::count(&state, "trades").await, 0);
}

#[tokio::test]
async fn upload_under_the_configured_limit_succeeds() {
    let dir = TempDir::new().unwrap();
    let state = common::start_with_upload_limit(&dir, LIMIT).await;
    let app = build_router(state.clone(), None);

    let csv = trades_csv(1);
    let (status, id, body) = send(&app, multipart_upload("small.csv", &csv)).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "success");
    assert_eq!(body["inserted_rows"], 1);
    assert!(common::is_uuid_v4(&id), "{id}");
    assert_eq!(common::count(&state, "trades").await, 1);
}

#[tokio::test]
async fn supplied_fixture_sequence_imports_at_the_default_limit() {
    // No [api] section: the 10 MiB default, which comfortably fits every
    // supplied file (the largest is about 33 KB).
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    assert_eq!(state.api.max_upload_bytes(), 10 * 1024 * 1024);
    let app = build_router(state.clone(), None);

    for file in [
        "trades_initial.csv",
        "trades_incremental.csv",
        "trades_corrected.csv",
    ] {
        let (status, _, body) = send(&app, multipart_upload(file, &common::fixture(file))).await;
        assert_eq!(status, StatusCode::OK, "{file}: {body}");
    }

    assert!(common::count(&state, "trades").await > 0);
}
