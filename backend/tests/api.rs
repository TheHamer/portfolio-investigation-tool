//! HTTP-level tests through the real Axum router.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use common::{BOUNDARY, multipart_fields, multipart_upload};
use http_body_util::BodyExt;
use portfolio_investigation::build_router;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, json)
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    send(app, Request::get(uri).body(Body::empty()).unwrap()).await
}

async fn app_with_trades(dir: &TempDir) -> Router {
    let app = build_router(common::start(dir).await, None);

    for file in [
        "trades_initial.csv",
        "trades_incremental.csv",
        "trades_corrected.csv",
    ] {
        let (status, _) = send(&app, multipart_upload(file, &common::fixture(file))).await;
        assert_eq!(status, StatusCode::OK);
    }

    app
}

#[tokio::test]
async fn import_endpoint_reports_success_and_rejection() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    let (status, body) = send(
        &app,
        multipart_upload("trades_initial.csv", &common::fixture("trades_initial.csv")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outcome"], "success");
    assert_eq!(body["file_name"], "trades_initial.csv");
    assert_eq!(body["inserted_rows"], 600);

    let (status, body) = send(
        &app,
        multipart_upload("trades_invalid.csv", &common::fixture("trades_invalid.csv")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["outcome"], "rejected");
    assert_eq!(body["invalid_rows"], 8);
    assert_eq!(body["aborted_rows"], 2);
    assert_eq!(body["errors"][0]["row"], 3);
    assert_eq!(body["errors"][0]["trade_id"], "T572470");

    let bad_header = b"id,portfolio_id\nT1,P001\n";
    let (status, body) = send(&app, multipart_upload("bad.csv", bad_header)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["total_rows"], Value::Null);
    assert_eq!(body["errors"][0]["row"], Value::Null);
}

#[tokio::test]
async fn import_endpoint_rejects_malformed_requests() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    let not_multipart = Request::post("/api/trades/import")
        .body(Body::from("hello"))
        .unwrap();
    let (status, body) = send(&app, not_multipart).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");

    let no_file = Request::post("/api/trades/import")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(format!("--{BOUNDARY}--\r\n")))
        .unwrap();
    let (status, body) = send(&app, no_file).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("No file")
    );
    // Oversized uploads: see tests/upload_limit.rs (small configured limit)
    // and `rejection_and_request_error_bodies_are_exact` (default limit).
}

#[tokio::test]
async fn portfolio_and_position_endpoints() {
    let dir = TempDir::new().unwrap();
    let app = app_with_trades(&dir).await;

    let (status, body) = get(&app, "/api/portfolios").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["as_of"], "2026-03-27");
    assert_eq!(body["dataset_id"], "ds-f77160d14d37a72b");
    assert_eq!(body["portfolios"].as_array().unwrap().len(), 3);
    assert_eq!(
        body["portfolios"][1]["name"],
        "Fictional Income, Value & Quality Portfolio"
    );

    let (status, body) = get(&app, "/api/portfolios/P001/positions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    let mv = body["totals"]["market_value_gbp"].as_str().unwrap();
    assert_eq!(
        mv.split('.').nth(1).unwrap().len(),
        2,
        "money is formatted to 2 dp: {mv}"
    );

    let positions = body["positions"].as_array().unwrap();
    assert!(!positions.is_empty());
    let first = &positions[0];
    for field in [
        "instrument_id",
        "ticker",
        "quantity",
        "market_value_gbp",
        "total_pnl_gbp",
        "valuation_price",
        "valuation_fx",
    ] {
        assert!(!first[field].is_null(), "missing {field}");
    }

    let instrument = first["instrument_id"].as_str().unwrap();
    let (status, detail) = get(
        &app,
        &format!("/api/portfolios/P001/positions/{instrument}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["position"]["total_pnl_gbp"], first["total_pnl_gbp"]);
    let trades = detail["trades"].as_array().unwrap();
    assert_eq!(trades.len() as u64, first["trade_count"].as_u64().unwrap());
    assert_eq!(
        trades.last().unwrap()["running_quantity"],
        first["quantity"]
    );
    assert!(!trades[0]["fx"]["gbp_per_unit"].is_null());
    assert!(!detail["breakdown"]["market_value_gbp_exact"].is_null());

    let (status, body) = get(&app, "/api/portfolios/NOPE/positions").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "not_found");

    let (status, _) = get(&app, "/api/portfolios/P001/positions/I999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // IDs are case-sensitive.
    let (status, _) = get(&app, "/api/portfolios/p001/positions").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn carried_forward_price_is_flagged() {
    let dir = TempDir::new().unwrap();
    let app = app_with_trades(&dir).await;

    // I017 has no price on the as_of date (2026-03-27) in the supplied data.
    let mut found = false;
    for portfolio in ["P001", "P002", "P003"] {
        let (_, body) = get(&app, &format!("/api/portfolios/{portfolio}/positions")).await;
        for p in body["positions"].as_array().unwrap() {
            if p["instrument_id"] == "I017" {
                found = true;
                assert_eq!(p["valuation_price"]["carried_forward"], true);
                assert_eq!(p["valuation_price"]["date"], "2026-03-26");
                assert_eq!(p["valuation_fx"]["date"], "2026-03-27");
            }
        }
    }

    assert!(found, "I017 is held by at least one portfolio");
}

#[tokio::test]
async fn unknown_api_routes_return_json_404_even_with_frontend_served() {
    let dir = TempDir::new().unwrap();
    let static_dir = dir.path().join("dist");
    std::fs::create_dir_all(&static_dir).unwrap();
    std::fs::write(
        static_dir.join("index.html"),
        "<!doctype html><title>app</title>",
    )
    .unwrap();
    let app = build_router(common::start(&dir).await, Some(&static_dir));

    let (status, body) = get(&app, "/api/does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body["error"]["message"],
        "No API endpoint at /api/does-not-exist."
    );

    let (status, body) = get(&app, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_str().unwrap().contains("<title>app</title>"));

    let (status, body) = get(&app, "/api/live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn error_paths_report_unavailable_values_instead_of_panicking() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    let pool = state.pool.clone();
    let app = build_router(state, None);
    let header = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n";

    // Valid inputs whose products exceed the exact Decimal range (I002 is GBP),
    // a USD position whose FX coverage is removed below (I001 is USD), and two
    // trades whose quantities overflow i64 when summed.
    let file = format!(
        "{header}BIG1,P001,I002,2026-01-05T10:00:00Z,BUY,9223372036854775807,9999999999999999.9999\n\
         U1,P002,I001,2026-01-05T10:00:00Z,BUY,10,100\n\
         OV1,P003,I002,2026-01-05T10:00:00Z,BUY,9223372036854775807,1\n\
         OV2,P003,I002,2026-01-05T11:00:00Z,BUY,1,1\n"
    );
    let (status, _) = send(&app, multipart_upload("edge.csv", file.as_bytes())).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("DELETE FROM fx_rates WHERE currency = 'USD'")
        .execute(&pool)
        .await
        .unwrap();

    // Oversized amounts.
    let (status, body) = get(&app, "/api/portfolios/P001/positions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "unavailable");
    assert!(body["totals"].is_null());
    let position = &body["positions"][0];
    assert_eq!(position["status"], "unavailable");
    assert_eq!(position["quantity"], i64::MAX);
    assert!(
        position["unavailable_reason"]
            .as_str()
            .unwrap()
            .contains("exact numeric range")
    );

    let (status, detail) = get(&app, "/api/portfolios/P001/positions/I002").await;
    assert_eq!(status, StatusCode::OK);
    assert!(detail["breakdown"].is_null());
    assert!(detail["trades"][0]["signed_local_amount"].is_null());
    assert_eq!(detail["trades"][0]["quantity"], i64::MAX);

    // Missing FX coverage: the trade lines still show what can be computed.
    let (_, body) = get(&app, "/api/portfolios/P002/positions").await;
    assert_eq!(body["status"], "unavailable");
    assert!(
        body["positions"][0]["unavailable_reason"]
            .as_str()
            .unwrap()
            .contains("No USD FX rate")
    );

    let (status, detail) = get(&app, "/api/portfolios/P002/positions/I001").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["trades"][0]["signed_local_amount"], "1000");
    assert!(detail["trades"][0]["fx"].is_null());

    // Quantity aggregation overflow: quantity is null rather than wrapped.
    let (status, body) = get(&app, "/api/portfolios/P003/positions").await;
    assert_eq!(status, StatusCode::OK);
    let position = &body["positions"][0];
    assert!(position["quantity"].is_null());
    assert_eq!(position["is_closed"], false);
    assert_eq!(position["status"], "unavailable");
}

/// Closed means a net quantity of exactly zero, whether or not the position
/// can be valued; an overflowed (null) quantity is open. The counts agree
/// with the positions' `is_closed` flags.
#[tokio::test]
async fn open_and_closed_counts_follow_quantity_alone() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    let pool = state.pool.clone();
    let app = build_router(state, None);
    let header = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n";

    // P001: I002 (GBP) closed and valued; I001 (USD) closed but unvalued once
    // USD FX is removed; I003 (EUR) open; I004 (EUR) overflows i64.
    let file = format!(
        "{header}C1,P001,I002,2026-01-05T10:00:00Z,BUY,10,100\n\
         C2,P001,I002,2026-01-06T10:00:00Z,SELL,10,101\n\
         U1,P001,I001,2026-01-05T10:00:00Z,BUY,5,100\n\
         U2,P001,I001,2026-01-06T10:00:00Z,SELL,5,100\n\
         O1,P001,I003,2026-01-05T10:00:00Z,BUY,7,100\n\
         OV1,P001,I004,2026-01-05T10:00:00Z,BUY,9223372036854775807,1\n\
         OV2,P001,I004,2026-01-05T11:00:00Z,BUY,1,1\n"
    );
    let (status, _) = send(&app, multipart_upload("counts.csv", file.as_bytes())).await;
    assert_eq!(status, StatusCode::OK);

    sqlx::query("DELETE FROM fx_rates WHERE currency = 'USD'")
        .execute(&pool)
        .await
        .unwrap();

    let (status, body) = get(&app, "/api/portfolios/P001/positions").await;
    assert_eq!(status, StatusCode::OK);
    let summary: Vec<(&str, bool, &str)> = body["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["instrument_id"].as_str().unwrap(),
                p["is_closed"].as_bool().unwrap(),
                p["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("I001", true, "unavailable"),
            ("I002", true, "ok"),
            ("I003", false, "ok"),
            ("I004", false, "unavailable"),
        ]
    );
    assert_eq!(body["closed_position_count"], 2);
    assert_eq!(body["open_position_count"], 2);
}

/// A rejected import and the request errors, with their bodies compared in
/// full against the documented JSON.
#[tokio::test]
async fn rejection_and_request_error_bodies_are_exact() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    // No [api] section in the test configuration: the 10 MiB default.
    let max_upload_bytes = state.api.max_upload_bytes();
    assert_eq!(max_upload_bytes, 10 * 1024 * 1024);

    let app = build_router(state, None);
    let (status, _) = send(
        &app,
        multipart_upload("trades_initial.csv", &common::fixture("trades_initial.csv")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send(
        &app,
        multipart_upload("trades_invalid.csv", &common::fixture("trades_invalid.csv")),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let row = |row: u64, trade_id: &str, message: &str| serde_json::json!({ "row": row, "trade_id": trade_id, "messages": [message] });
    assert_eq!(
        body,
        serde_json::json!({
            "file_name": "trades_invalid.csv",
            "outcome": "rejected",
            "message": "File rejected: 8 of 10 rows are invalid. No trades were imported and stored trades are unchanged.",
            "total_rows": 10,
            "inserted_rows": 0,
            "skipped_rows": 0,
            "invalid_rows": 8,
            "aborted_rows": 2,
            "errors": [
                row(3, "T572470", "portfolio_id 'P999' does not exist"),
                row(5, "T978178", "trade date 2026-03-28 is not a valuation date in the dataset calendar (2026-01-05 to 2026-03-27)"),
                row(6, "T376158", "trade_id 'T376158' already exists with different values: portfolio_id stored P003 vs file P002; instrument_id stored I019 vs file I008; timestamp stored 2026-01-22T15:12:35Z vs file 2026-03-12T09:54:20Z; quantity stored 432 vs file 168; price stored 92.2513 vs file 61.2895. Stored trades are never overwritten; use a new trade_id"),
                row(7, "T742462", "side 'HOLD' is invalid (expected BUY or SELL)"),
                row(8, "T474813", "quantity '0' must be greater than zero"),
                row(9, "T250143", "timestamp '2026-03-13 15:10:35' must be a UTC time in the format YYYY-MM-DDTHH:MM:SSZ"),
                row(10, "T109815", "instrument_id 'I999' does not exist"),
                row(11, "T841347", "price '-20.7558' is not a valid positive decimal number"),
            ]
        })
    );

    // File-level rejection: no row counts, one message without a row.
    let (status, body) = send(
        &app,
        multipart_upload("bad.csv", b"id,portfolio_id\nT1,P001\n"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body,
        serde_json::json!({
            "file_name": "bad.csv",
            "outcome": "rejected",
            "message": "File rejected: The header must be exactly 'trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price' but was 'id,portfolio_id'. No trades were imported.",
            "total_rows": null,
            "inserted_rows": null,
            "skipped_rows": null,
            "invalid_rows": null,
            "aborted_rows": null,
            "errors": [{
                "row": null,
                "trade_id": null,
                "messages": ["The header must be exactly 'trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price' but was 'id,portfolio_id'."]
            }]
        })
    );

    // Request errors.
    let no_file = Request::post("/api/trades/import")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(format!("--{BOUNDARY}--\r\n")))
        .unwrap();
    let (status, body) = send(&app, no_file).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body,
        serde_json::json!({ "error": { "code": "bad_request", "message": "No file was uploaded (expected form field 'file')." } })
    );

    let not_multipart = Request::post("/api/trades/import")
        .body(Body::from("hello"))
        .unwrap();
    let (status, body) = send(&app, not_multipart).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Expected a multipart form upload: ")
    );

    // The one test that needs a body over the default limit, to pin the
    // default's exact message.
    let huge = vec![b'a'; max_upload_bytes + 1];
    let (status, body) = send(&app, multipart_upload("huge.csv", &huge)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body,
        serde_json::json!({ "error": { "code": "payload_too_large", "message": "The upload request exceeds the maximum size of 10 MiB." } })
    );
}

/// A known API path with the wrong method: JSON 405, Axum's `Allow` header
/// and a request ID. Unknown paths stay 404 whatever the method.
#[tokio::test]
async fn unsupported_methods_return_structured_405() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    for (method, uri, allowed) in [
        (Method::POST, "/api/portfolios", "GET"),
        (Method::DELETE, "/api/portfolios/P001/positions", "GET"),
        (Method::PUT, "/api/portfolios/P001/positions/I001", "GET"),
        (Method::GET, "/api/trades/import", "POST"),
        (Method::POST, "/api/live", "GET"),
        (Method::POST, "/api/ready", "GET"),
    ] {
        let request = Request::builder()
            .method(method.clone())
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri}"
        );

        let allow = response.headers()[header::ALLOW].to_str().unwrap();
        assert!(allow.contains(allowed), "{method} {uri}: Allow: {allow}");
        assert!(common::is_uuid_v4(&common::request_id(&response)));

        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body,
            serde_json::json!({ "error": {
                "code": "method_not_allowed",
                "message": "The requested HTTP method is not supported."
            } }),
            "{method} {uri}"
        );
    }

    for method in [Method::GET, Method::POST] {
        let request = Request::builder()
            .method(method.clone())
            .uri("/api/does-not-exist")
            .body(Body::empty())
            .unwrap();

        let (status, body) = send(&app, request).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method}");
        assert_eq!(body["error"]["code"], "not_found", "{method}");
    }
}

#[tokio::test]
async fn health_endpoint_is_no_longer_registered() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    let (status, body) = get(&app, "/api/health").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        serde_json::json!({ "error": { "code": "not_found", "message": "No API endpoint at /api/health." } })
    );
}

#[tokio::test]
async fn liveness_and_readiness_with_an_accessible_database() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    let (status, body) = get(&app, "/api/live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "status": "ok" }));

    let (status, body) = get(&app, "/api/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "status": "ready" }));
}

/// With the pool closed, readiness reports 503 without the cause, and
/// liveness still succeeds because it never touches SQLite.
#[tokio::test]
async fn readiness_fails_and_liveness_succeeds_when_sqlite_is_unavailable() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    state.pool.close().await;
    let app = build_router(state, None);

    let (status, body) = get(&app, "/api/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        serde_json::json!({ "error": {
            "code": "service_unavailable",
            "message": "The database is currently unavailable."
        } })
    );

    let (status, body) = get(&app, "/api/live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({ "status": "ok" }));
}

/// A second `file` field rejects the whole request before anything is
/// imported, whether or not the database already holds trades. Other fields
/// are still ignored, and a single file still imports.
#[tokio::test]
async fn duplicate_file_fields_are_rejected_without_changing_the_database() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    let app = build_router(state.clone(), None);
    let initial = common::fixture("trades_initial.csv");
    let incremental = common::fixture("trades_incremental.csv");
    let duplicate = || {
        multipart_fields(&[
            ("file", "trades_initial.csv", &initial),
            ("file", "trades_incremental.csv", &incremental),
        ])
    };

    let expected = serde_json::json!({ "error": {
        "code": "bad_request",
        "message": "More than one file was uploaded (expected a single form field 'file')."
    } });

    // Empty database.
    let (status, body) = send(&app, duplicate()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, expected);
    assert_eq!(common::count(&state, "trades").await, 0);

    // Single-file upload, with an unrelated field before it, still works.
    let (status, body) = send(
        &app,
        multipart_fields(&[
            ("note", "note.txt", b"ignored"),
            ("file", "trades_initial.csv", &initial),
        ]),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["inserted_rows"], 600);

    // Database with trades: unchanged by a duplicate-file request.
    let before = common::trade_snapshot(&state).await;
    let (status, body) = send(&app, duplicate()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, expected);
    assert_eq!(common::trade_snapshot(&state).await, before);
}

/// Responses built from domain types expose their fields at one level, with
/// no nested domain objects. (The tests in `src/api/dto/` pin the exact text;
/// `rejection_and_request_error_bodies_are_exact` pins full import bodies.)
#[tokio::test]
async fn responses_expose_their_fields_at_one_level() {
    fn keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    const PORTFOLIO: [&str; 3] = ["base_currency", "name", "portfolio_id"];
    const POSITION: [&str; 15] = [
        "currency",
        "instrument_id",
        "is_closed",
        "market_value_gbp",
        "name",
        "net_trade_cost_gbp",
        "quantity",
        "sector",
        "status",
        "ticker",
        "total_pnl_gbp",
        "trade_count",
        "unavailable_reason",
        "valuation_fx",
        "valuation_price",
    ];

    let dir = TempDir::new().unwrap();
    let app = app_with_trades(&dir).await;

    let (_, list) = get(&app, "/api/portfolios").await;
    assert_eq!(keys(&list), ["as_of", "dataset_id", "portfolios"]);
    for item in list["portfolios"].as_array().unwrap() {
        assert_eq!(
            keys(item),
            ["base_currency", "name", "portfolio_id", "trade_count"]
        );
    }
    assert_eq!(
        list["portfolios"][0],
        serde_json::json!({
            "portfolio_id": "P001",
            "name": "Fictional Growth Portfolio",
            "base_currency": "GBP",
            "trade_count": 229
        })
    );

    let (_, positions) = get(&app, "/api/portfolios/P001/positions").await;
    assert_eq!(
        keys(&positions),
        [
            "as_of",
            "closed_position_count",
            "open_position_count",
            "portfolio",
            "positions",
            "status",
            "totals",
            "unavailable_reason"
        ]
    );
    assert_eq!(keys(&positions["portfolio"]), PORTFOLIO);
    for position in positions["positions"].as_array().unwrap() {
        assert_eq!(keys(position), POSITION);
    }

    let instrument = positions["positions"][0]["instrument_id"].as_str().unwrap();
    let (_, detail) = get(
        &app,
        &format!("/api/portfolios/P001/positions/{instrument}"),
    )
    .await;
    assert_eq!(
        keys(&detail),
        ["as_of", "breakdown", "portfolio", "position", "trades"]
    );
    assert_eq!(keys(&detail["portfolio"]), PORTFOLIO);
    assert_eq!(keys(&detail["position"]), POSITION);
    assert_eq!(detail["position"], positions["positions"][0]);
}
