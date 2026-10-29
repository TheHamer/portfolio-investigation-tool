//! Health probes: `/live` (the process is serving) and `/ready` (the database
//! answers). Neither touches the business modules.

use std::time::Duration;

use axum::Json;
use axum::extract::State;

use crate::AppState;
use crate::api::ApiError;
use crate::api::dto::ProbeResponse;
use crate::persistence;

/// How long the readiness probe waits for the database before reporting 503.
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);

/// Liveness: the process is serving requests. It touches no dependency, so a
/// database outage never makes the process look dead.
pub(in crate::api) async fn live() -> Json<ProbeResponse> {
    Json(ProbeResponse { status: "ok" })
}

/// Readiness: the database answers a trivial read through the shared pool
/// within `READINESS_TIMEOUT`.
pub(in crate::api) async fn ready(
    State(state): State<AppState>,
) -> Result<Json<ProbeResponse>, ApiError> {
    tokio::time::timeout(READINESS_TIMEOUT, persistence::ping(&state.pool))
        .await
        .map_err(ApiError::DatabaseTimeout)?
        .map_err(ApiError::DatabaseUnavailable)?;

    Ok(Json(ProbeResponse { status: "ready" }))
}
