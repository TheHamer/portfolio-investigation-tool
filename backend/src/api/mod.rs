//! HTTP layer: router assembly, the endpoint handlers (`handlers`), response
//! shapes and the middleware wrapped around the whole application.
//!
//! Handlers stay thin: extract input, call `import` or `portfolio`, and map
//! the result to JSON via the types in `dto`. No financial arithmetic happens
//! here beyond formatting already-calculated values.

mod config;
mod dto;
mod error;
mod extractors;
mod handlers;
mod middleware;

pub(crate) use config::RawApiConfig;
pub use config::{ApiConfig, ApiConfigError};
pub use error::ApiError;
// Applied by `build_router` to the complete router, not just `/api`.
pub(crate) use middleware::apply as apply_http_middleware;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};

use crate::AppState;

/// The `/api` routes. `max_upload_bytes` (from `ApiConfig`) caps the whole
/// request body of the import route only; every other route keeps Axum's
/// default limit. The import handler reads the same value from `AppState`.
pub fn router(max_upload_bytes: usize) -> Router<AppState> {
    Router::new()
        .route("/live", get(handlers::health::live))
        .route("/ready", get(handlers::health::ready))
        .route("/portfolios", get(handlers::portfolios::list_portfolios))
        .route(
            "/portfolios/{portfolio_id}/positions",
            get(handlers::portfolios::portfolio_positions),
        )
        .route(
            "/portfolios/{portfolio_id}/positions/{instrument_id}",
            get(handlers::portfolios::position_detail),
        )
        .route(
            "/trades/import",
            post(handlers::imports::import_trades).layer(DefaultBodyLimit::max(max_upload_bytes)),
        )
        // A known path with an unsupported method gets a JSON 405. It applies
        // to the routes added above, so it must stay after them. Axum still
        // sets the `Allow` header on the response.
        .method_not_allowed_fallback(handlers::fallback::method_not_allowed)
        // Unknown /api paths get a JSON 404 rather than the SPA's index.html.
        .fallback(handlers::fallback::not_found)
}
