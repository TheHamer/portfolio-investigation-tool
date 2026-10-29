//! Portfolio Investigation Tool backend.
//!
//! Module map:
//! - `config`: the TOML configuration file with `INTROPIC_*` overrides.
//! - `error`: `AppError`, the application-level error (no HTTP knowledge).
//! - `domain`: validated newtypes (IDs, prices, FX rates, quantities, timestamps).
//! - `import`: trade CSV pipeline: `parsing` -> `validation` -> `service`
//!   (duplicate classification and the atomic write transaction).
//! - `valuation`: pure position valuation and P&L arithmetic.
//! - `portfolio`: loads trades and market data and runs the valuation.
//! - `persistence`: SQLite pool, migrations and all SQL.
//! - `manifest` / `reference`: dataset calendar and idempotent reference loading.
//! - `api`: Axum router, handlers, JSON response shapes, `[api]` settings and
//!   the HTTP layers around the whole router (request IDs, tracing).
//! - `utils`: graceful-shutdown signal handling.

#![forbid(unsafe_code)]

pub mod api;
pub mod config;
pub mod domain;
pub mod error;
pub mod import;
pub mod manifest;
pub mod persistence;
pub mod portfolio;
pub mod reference;
pub mod utils;
pub mod valuation;

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use sqlx::SqlitePool;
use tower_http::services::{ServeDir, ServeFile};

use crate::api::ApiConfig;
use crate::config::Config;
use crate::error::AppError;
use crate::manifest::Manifest;

/// State shared by every request handler. Cloned per request; the pool and
/// manifest are shared, not copied.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub manifest: Arc<Manifest>,
    /// The only part of the configuration that handlers need at runtime.
    pub api: ApiConfig,
}

/// Opens the database, applies migrations and loads reference data. Safe to
/// run on every start.
pub async fn initialise(config: &Config) -> Result<AppState, AppError> {
    let manifest = Manifest::load(config.manifest_path())?;
    let pool = persistence::connect(config.persistence()).await?;
    let counts =
        reference::load_reference_data(&pool, config.reference_data_dir(), &manifest).await?;

    tracing::info!(
        dataset_id = %manifest.dataset_id(),
        as_of = %manifest.as_of(),
        ?counts,
        "reference data loaded"
    );

    Ok(AppState {
        pool,
        manifest: Arc::new(manifest),
        api: config.api().clone(),
    })
}

/// The `/api` routes plus, when a built frontend is available, the static
/// files with `index.html` as the fallback for client-side paths, all wrapped
/// in the HTTP middleware. The upload limit comes from `state.api`, the same
/// value the import handler reports.
pub fn build_router(state: AppState, static_dir: Option<&Path>) -> Router {
    let mut app = Router::new().nest("/api", api::router(state.api.max_upload_bytes()));
    if let Some(dir) = static_dir {
        let spa = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
        app = app.fallback_service(spa);
    }

    // Applied here, to `app`, so the layers wrap the static files and the SPA
    // fallback as well as every `/api` response.
    api::apply_http_middleware(app).with_state(state)
}
