//! Fallbacks for `/api` requests that no route matches: JSON 405 for a known
//! path with an unsupported method, JSON 404 for an unknown path.

use axum::extract::OriginalUri;

use crate::api::ApiError;

/// A known `/api` path called with an unsupported method.
pub(in crate::api) async fn method_not_allowed() -> ApiError {
    ApiError::MethodNotAllowed
}

/// An unknown `/api` path: a JSON 404 rather than the SPA's `index.html`.
pub(in crate::api) async fn not_found(OriginalUri(uri): OriginalUri) -> ApiError {
    ApiError::RouteNotFound {
        path: uri.path().to_owned(),
    }
}
