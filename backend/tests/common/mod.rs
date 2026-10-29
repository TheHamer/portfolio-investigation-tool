//! Shared helpers for integration tests. Each test gets its own temporary
//! SQLite file, so tests are isolated from each other and from a running app.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Request, Response, header};
use portfolio_investigation::config::Config;
use portfolio_investigation::{AppState, initialise};
use tempfile::TempDir;

pub const BOUNDARY: &str = "test-boundary-7d2f";

/// `POST /api/trades/import` with `bytes` as the multipart field `file`.
pub fn multipart_upload(file_name: &str, bytes: &[u8]) -> Request<Body> {
    multipart_fields(&[("file", file_name, bytes)])
}

/// `POST /api/trades/import` with one multipart part per
/// `(field name, file name, contents)`, in order.
pub fn multipart_fields(parts: &[(&str, &str, &[u8])]) -> Request<Body> {
    let mut body = Vec::new();
    for (name, file_name, bytes) in parts {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\n\
                 Content-Type: text/csv\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    Request::post("/api/trades/import")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap()
}

/// The response's `x-request-id` header as text. Panics when it is missing.
pub fn request_id<B>(response: &Response<B>) -> String {
    response
        .headers()
        .get("x-request-id")
        .expect("response has an x-request-id header")
        .to_str()
        .expect("x-request-id is visible ASCII")
        .to_owned()
}

/// Whether `id` is a hyphenated, lowercase UUID version 4 (RFC 9562), the
/// format `MakeRequestUuid` generates.
pub fn is_uuid_v4(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        })
        && bytes[14] == b'4'
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

/// The supplied dataset: manifest, reference CSVs and upload fixtures.
pub fn dataset_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../data")
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(dataset_dir().join("uploads").join(name)).expect("fixture file")
}

/// Builds a `Config` the only way the public API allows: by loading a TOML
/// file (written to `dir`). An empty override map keeps any INTROPIC_*
/// variables in the real environment out of the tests.
pub fn load_config(
    dir: &TempDir,
    database_path: &Path,
    manifest_path: &Path,
    reference_data_dir: &Path,
) -> Config {
    load_config_with(dir, database_path, manifest_path, reference_data_dir, "")
}

/// Like [`load_config`], with `extra` appended to the file (for example an
/// `[api]` section).
pub fn load_config_with(
    dir: &TempDir,
    database_path: &Path,
    manifest_path: &Path,
    reference_data_dir: &Path,
    extra: &str,
) -> Config {
    let file = dir.path().join("config.toml");
    // TOML literal strings ('...') take paths verbatim, with no escaping.
    let contents = format!(
        "manifest_path = '{}'\n\
         reference_data_dir = '{}'\n\
         bind_addr = '127.0.0.1:0'\n\
         \n\
         [persistence]\n\
         database_path = '{}'\n\
         {extra}",
        manifest_path.display(),
        reference_data_dir.display(),
        database_path.display(),
    );

    std::fs::write(&file, contents).expect("write test config");
    Config::load(Some(&file), Some(HashMap::new())).expect("test config is valid")
}

/// The supplied dataset with a fresh database and an upload limit of
/// `max_upload_bytes`, set through the `[api]` section like a real deployment.
pub async fn start_with_upload_limit(dir: &TempDir, max_upload_bytes: usize) -> AppState {
    let config = load_config_with(
        dir,
        &dir.path().join("test.db"),
        &dataset_dir().join("manifest.json"),
        &dataset_dir().join("reference"),
        &format!("\n[api]\nmax_upload_bytes = {max_upload_bytes}\n"),
    );

    assert_eq!(config.api().max_upload_bytes(), max_upload_bytes);
    initialise(&config).await.expect("initialise app")
}

/// Configuration for the supplied dataset with a fresh database in `dir`.
pub fn config(dir: &TempDir) -> Config {
    load_config(
        dir,
        &dir.path().join("test.db"),
        &dataset_dir().join("manifest.json"),
        &dataset_dir().join("reference"),
    )
}

pub async fn start(dir: &TempDir) -> AppState {
    initialise(&config(dir)).await.expect("initialise app")
}

/// Every stored trade as text, for before/after comparisons.
pub async fn trade_snapshot(
    state: &AppState,
) -> Vec<(String, String, String, String, String, i64, String)> {
    sqlx::query_as(
        "SELECT trade_id, portfolio_id, instrument_id, timestamp_utc, side, quantity, price
         FROM trades ORDER BY trade_id",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap()
}

pub async fn count(state: &AppState, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(&state.pool)
        .await
        .unwrap()
}
