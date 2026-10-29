//! The request ID is recorded on the request's tracing span.
//!
//! Kept in its own test binary on purpose: it captures logs with a
//! thread-local subscriber, and `tracing` caches whether each log statement
//! is enabled process-wide. Requests from tests running in parallel in the
//! same binary, with no subscriber, could cache "disabled" for the statements
//! this test needs to see.

mod common;

use std::io;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::Request;
use portfolio_investigation::build_router;
use tempfile::TempDir;
use tower::ServiceExt;

/// Log output, shared between the test and the subscriber's writer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The `finished processing request` line for `path`.
fn response_line<'a>(logs: &'a str, path: &str) -> &'a str {
    logs.lines()
        .find(|line| {
            line.contains("finished processing request") && line.contains(&format!("path={path}}}"))
        })
        .unwrap_or_else(|| {
            panic!(
                "no response event for {path} in:
{logs}"
            )
        })
}

/// Covers an API route and, since the middleware wraps the whole router, a
/// client-side route answered by the frontend's `index.html` fallback.
#[tokio::test]
async fn request_span_records_id_method_and_path() {
    let dir = TempDir::new().unwrap();
    let static_dir = dir.path().join("dist");
    std::fs::create_dir_all(&static_dir).unwrap();
    std::fs::write(static_dir.join("index.html"), "<!doctype html>").unwrap();
    let app = build_router(common::start(&dir).await, Some(&static_dir));

    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();

    // Thread-local, and `#[tokio::test]` runs the request on this thread.
    let (api_id, spa_id) = {
        let _guard = tracing::subscriber::set_default(subscriber);
        let mut ids = Vec::new();
        for uri in ["/api/live?verbose=1", "/portfolios/P001"] {
            let request = Request::get(uri).body(Body::empty()).unwrap();
            let response = app.clone().oneshot(request).await.unwrap();
            ids.push(common::request_id(&response));
        }
        (ids[0].clone(), ids[1].clone())
    };
    assert_ne!(api_id, spa_id);

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();

    // The response-status event is emitted inside the request span, so the
    // span's fields prefix it.
    for (path, id) in [("/api/live", &api_id), ("/portfolios/P001", &spa_id)] {
        let line = response_line(&logs, path);
        assert!(line.contains(&format!("request_id={id}")), "{line}");
        assert!(line.contains("method=GET"), "{line}");
        assert!(line.contains("status=200"), "{line}");
    }

    // The query string is not recorded.
    assert!(!logs.contains("verbose"), "{logs}");
}
