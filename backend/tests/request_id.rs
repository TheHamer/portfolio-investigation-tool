//! Request IDs (`api::middleware`, applied to the whole router): every
//! response carries `x-request-id`. That the same ID is recorded on the
//! tracing span is tested in `request_id_span.rs`.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{multipart_fields, multipart_upload};
use portfolio_investigation::build_router;
use tempfile::TempDir;
use tower::ServiceExt;

/// Status and `x-request-id` of the response.
async fn send(app: &Router, request: Request<Body>) -> (StatusCode, String) {
    let response = app.clone().oneshot(request).await.unwrap();
    (response.status(), common::request_id(&response))
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn probe_responses_carry_a_generated_uuid() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    for uri in ["/api/live", "/api/ready"] {
        let (status, id) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(common::is_uuid_v4(&id), "{uri}: {id}");
    }
}

#[tokio::test]
async fn independent_requests_get_different_ids() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    let (_, first) = send(&app, get("/api/live")).await;
    let (_, second) = send(&app, get("/api/live")).await;
    assert_ne!(first, second);
}

#[tokio::test]
async fn upload_outcomes_and_request_errors_carry_an_id() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    let initial = common::fixture("trades_initial.csv");
    let (status, id) = send(&app, multipart_upload("trades_initial.csv", &initial)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(common::is_uuid_v4(&id), "{id}");

    let bad_header = b"id,portfolio_id\nT1,P001\n";
    let (status, id) = send(&app, multipart_upload("bad.csv", bad_header)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(common::is_uuid_v4(&id), "{id}");

    let not_multipart = Request::post("/api/trades/import")
        .body(Body::from("hello"))
        .unwrap();
    let (status, id) = send(&app, not_multipart).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(common::is_uuid_v4(&id), "{id}");

    let two_files = multipart_fields(&[("file", "a.csv", b"x"), ("file", "b.csv", b"y")]);
    let (status, id) = send(&app, two_files).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(common::is_uuid_v4(&id), "{id}");
}

#[tokio::test]
async fn method_not_allowed_response_carries_an_id() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    let request = Request::post("/api/portfolios")
        .body(Body::empty())
        .unwrap();
    let (status, id) = send(&app, request).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(common::is_uuid_v4(&id), "{id}");
}

#[tokio::test]
async fn unavailable_readiness_response_carries_an_id() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    state.pool.close().await;
    let app = build_router(state, None);

    let (status, id) = send(&app, get("/api/ready")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(common::is_uuid_v4(&id), "{id}");
}

#[tokio::test]
async fn not_found_responses_carry_an_id() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    for uri in [
        // The API's JSON fallback.
        "/api/does-not-exist",
        // A handler's not-found result.
        "/api/portfolios/NOPE/positions",
        // Outside /api with no frontend served: the router's default 404.
        "/index.html",
    ] {
        let (status, id) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
        assert!(common::is_uuid_v4(&id), "{uri}: {id}");
    }
}

#[tokio::test]
async fn payload_too_large_response_carries_an_id() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start_with_upload_limit(&dir, 512).await, None);

    let (status, id) = send(&app, multipart_upload("big.csv", &[b'a'; 1024])).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(common::is_uuid_v4(&id), "{id}");
}

#[tokio::test]
async fn internal_error_response_carries_an_id() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    state.pool.close().await;
    let app = build_router(state, None);

    let (status, id) = send(&app, get("/api/portfolios")).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(common::is_uuid_v4(&id), "{id}");
}

#[tokio::test]
async fn frontend_responses_carry_an_id() {
    let dir = TempDir::new().unwrap();
    let static_dir = dir.path().join("dist");
    std::fs::create_dir_all(&static_dir).unwrap();
    std::fs::write(static_dir.join("index.html"), "<!doctype html>").unwrap();
    std::fs::write(static_dir.join("app.js"), "console.log(1)").unwrap();
    let app = build_router(common::start(&dir).await, Some(&static_dir));

    // A file, the root, and a client-side route served by the index fallback.
    for uri in ["/app.js", "/", "/portfolios/P001"] {
        let (status, id) = send(&app, get(uri)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert!(common::is_uuid_v4(&id), "{uri}: {id}");
    }
}

/// With a frontend served, the API's own fallbacks and the static fallback
/// are all inside the middleware: the layers wrap the complete router, not
/// only `/api`.
#[tokio::test]
async fn api_fallbacks_carry_an_id_alongside_the_frontend() {
    let dir = TempDir::new().unwrap();
    let static_dir = dir.path().join("dist");
    std::fs::create_dir_all(&static_dir).unwrap();
    std::fs::write(static_dir.join("index.html"), "<!doctype html>").unwrap();
    let app = build_router(common::start(&dir).await, Some(&static_dir));

    let (status, id) = send(&app, get("/api/does-not-exist")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(common::is_uuid_v4(&id), "{id}");

    let request = Request::post("/api/portfolios")
        .body(Body::empty())
        .unwrap();
    let (status, id) = send(&app, request).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(common::is_uuid_v4(&id), "{id}");

    // A client-chosen ID passes through the static fallback too.
    let request = Request::get("/portfolios/P001")
        .header("x-request-id", "client-chosen-id-43")
        .body(Body::empty())
        .unwrap();
    let (status, id) = send(&app, request).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(id, "client-chosen-id-43");
}

/// Tower HTTP's request-ID layers keep an ID the client sent, unvalidated,
/// and return it unchanged: a caller can correlate its own logs with ours.
#[tokio::test]
async fn incoming_request_id_is_kept_and_returned() {
    let dir = TempDir::new().unwrap();
    let app = build_router(common::start(&dir).await, None);

    for uri in ["/api/live", "/api/does-not-exist"] {
        let request = Request::get(uri)
            .header("x-request-id", "client-chosen-id-42")
            .body(Body::empty())
            .unwrap();
        let (_, id) = send(&app, request).await;
        assert_eq!(id, "client-chosen-id-42", "{uri}");
    }
}
