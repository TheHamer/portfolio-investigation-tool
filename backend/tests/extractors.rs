//! Extractor failures through the real router: they produce the structured
//! JSON error (`{"error": {"code", "message"}}`), not Axum's plain-text
//! default, and keep the request-ID header.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::{BOUNDARY, multipart_upload};
use http_body_util::BodyExt;
use portfolio_investigation::build_router;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

/// Status, `x-request-id` and body as JSON. Panics if the body is not JSON,
/// which would mean Axum's default rejection response leaked through.
async fn send(app: &Router, request: Request<Body>) -> (StatusCode, String, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let id = common::request_id(&response);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&bytes)));
    (status, id, body)
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn message(body: &Value) -> &str {
    body["error"]["message"].as_str().unwrap()
}

async fn app_with_initial_trades(dir: &TempDir) -> Router {
    let app = build_router(common::start(dir).await, None);
    let csv = common::fixture("trades_initial.csv");
    let (status, _, _) = send(&app, multipart_upload("trades_initial.csv", &csv)).await;
    assert_eq!(status, StatusCode::OK);

    app
}

#[tokio::test]
async fn path_parameters_are_still_extracted() {
    let dir = TempDir::new().unwrap();
    let app = app_with_initial_trades(&dir).await;

    let (status, _, body) = send(&app, get("/api/portfolios/P001/positions")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["portfolio"]["portfolio_id"], "P001");
    let instrument = body["positions"][0]["instrument_id"].as_str().unwrap();

    let uri = format!("/api/portfolios/P001/positions/{instrument}");
    let (status, _, body) = send(&app, get(&uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["position"]["instrument_id"], instrument);
}

#[tokio::test]
async fn path_rejection_is_a_structured_400_with_a_request_id() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    // `%FF` matches the route but does not decode to UTF-8, so `Path<String>`
    // rejects it before the handler runs.
    for uri in [
        "/api/portfolios/%FF/positions",
        "/api/portfolios/P001/positions/%FF",
    ] {
        let (status, id, body) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(body["error"]["code"], "bad_request", "{uri}");
        assert!(
            message(&body).starts_with("Invalid request path: "),
            "{uri}: {body}"
        );
        assert!(common::is_uuid_v4(&id), "{uri}: {id}");
    }
}

#[tokio::test]
async fn unknown_resources_keep_their_application_404s() {
    let dir = TempDir::new().unwrap();
    let app = app_with_initial_trades(&dir).await;

    let (status, _, body) = send(&app, get("/api/portfolios/NOPE/positions")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({ "error": { "code": "not_found", "message": "Portfolio 'NOPE' does not exist." } })
    );

    let (status, _, body) = send(&app, get("/api/portfolios/P001/positions/I999")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({ "error": { "code": "not_found", "message": "Instrument 'I999' does not exist." } })
    );
}

#[tokio::test]
async fn multipart_upload_still_imports() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);
    let csv = common::fixture("trades_initial.csv");

    let (status, id, body) = send(&app, multipart_upload("trades_initial.csv", &csv)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outcome"], "success");
    assert_eq!(body["inserted_rows"], 600);
    assert!(common::is_uuid_v4(&id), "{id}");
}

/// `MultipartRejection`: the request cannot become a `Multipart` at all.
#[tokio::test]
async fn missing_or_invalid_boundary_is_a_structured_400() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    for content_type in [None, Some("text/csv"), Some("multipart/form-data")] {
        let mut request = Request::post("/api/trades/import");
        if let Some(content_type) = content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        let request = request.body(Body::from("a,b\n1,2\n")).unwrap();

        let (status, id, body) = send(&app, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{content_type:?}");
        assert_eq!(body["error"]["code"], "bad_request", "{content_type:?}");
        assert!(
            message(&body).starts_with("Expected a multipart form upload: "),
            "{content_type:?}: {body}"
        );
        assert!(common::is_uuid_v4(&id), "{content_type:?}: {id}");
    }
}

/// `MultipartError`: extraction succeeded, reading the body failed.
#[tokio::test]
async fn malformed_multipart_body_is_a_structured_400() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    let app = build_router(state.clone(), None);

    let request = Request::post("/api/trades/import")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        // A valid boundary parameter, but the body never contains it.
        .body(Body::from("this is not a multipart body"))
        .unwrap();

    let (status, id, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "bad_request");
    assert!(
        message(&body).starts_with("The upload could not be read: "),
        "{body}"
    );
    assert!(common::is_uuid_v4(&id), "{id}");
    assert_eq!(common::count(&state, "trades").await, 0);
}

/// Also a `MultipartError`, mapped with the configured limit.
#[tokio::test]
async fn oversized_upload_reports_the_configured_limit() {
    let dir = TempDir::new().unwrap();
    let state = common::start_with_upload_limit(&dir, 2048).await;
    let app = build_router(state.clone(), None);

    let (status, id, body) = send(&app, multipart_upload("big.csv", &[b'a'; 4096])).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        body,
        json!({ "error": {
            "code": "payload_too_large",
            "message": "The upload request exceeds the maximum size of 2 KiB."
        } })
    );
    assert!(common::is_uuid_v4(&id), "{id}");
    assert_eq!(common::count(&state, "trades").await, 0);
}

#[tokio::test]
async fn upload_without_the_file_field_keeps_its_error() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"other\"\r\n\r\nx\r\n--{BOUNDARY}--\r\n"
    );
    let request = Request::post("/api/trades/import")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();

    let (status, id, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body,
        json!({ "error": {
            "code": "bad_request",
            "message": "No file was uploaded (expected form field 'file')."
        } })
    );
    assert!(common::is_uuid_v4(&id), "{id}");
}

#[tokio::test]
async fn rejected_import_is_a_report_and_database_failure_a_generic_500() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    let pool = state.pool.clone();
    let app = build_router(state, None);

    let (status, _, body) = send(
        &app,
        multipart_upload("bad.csv", b"id,portfolio_id\nT1,P001\n"),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["outcome"], "rejected");

    pool.close().await;

    let (status, id, body) = send(&app, get("/api/portfolios/P001/positions")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        body,
        json!({ "error": {
            "code": "internal_error",
            "message": "An internal error occurred. Please try again."
        } })
    );
    assert!(common::is_uuid_v4(&id), "{id}");
}
