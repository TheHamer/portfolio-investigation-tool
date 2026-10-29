//! HTTP middleware applied to every response: API routes, the API's JSON 404
//! and 405 fallbacks, the static frontend and its `index.html` fallback.
//!
//! It lives in `api` but is not part of the `/api` router: `lib.rs` applies it
//! (as `api::apply_http_middleware`) to the complete application router.
//!
//! A request passes through the layers in this order:
//!
//! 1. `SetRequestIdLayer`: if the request has no `x-request-id` header, adds
//!    one holding a new UUID v4. A header the client already sent is kept
//!    unchanged (not validated or replaced).
//! 2. `TraceLayer`: opens the `request` span, recording that ID with the
//!    method and path, and logs the response's status and latency.
//! 3. `PropagateRequestIdLayer`: copies the request's `x-request-id` onto the
//!    response, whatever its status.
//! 4. The router: handlers, fallbacks and static files.
//!
//! Every log event emitted while handling the request, including handler
//! logs and error reports, is inside the span and so carries the request ID.

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use tower_http::request_id::{
    MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};
use tower_http::trace::TraceLayer;
use tracing::Span;

/// Wraps `router` in the request-ID and tracing layers.
///
/// `Router::layer` wraps everything added before it, so the layer added last
/// runs first. They are therefore added innermost first, the reverse of the
/// execution order in the module documentation. Add future layers here,
/// placed by where they must run relative to these.
pub(crate) fn apply<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        // 3. Innermost: runs just before the router.
        .layer(PropagateRequestIdLayer::x_request_id())
        // 2. Inside step 1, so the ID already exists when the span is made.
        .layer(TraceLayer::new_for_http().make_span_with(request_span))
        // 1. Outermost: runs first.
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
}

/// The span for one request. Records only the request ID, the method and the
/// path: no query string, no other headers, and never the body (uploaded
/// CSVs). It is created at INFO level so that the ID appears on the
/// INFO-level events logged while handling the request.
fn request_span(request: &Request<Body>) -> Span {
    // Set by `SetRequestIdLayer`. A client-supplied value that is not visible
    // ASCII cannot be shown as text; it is still echoed on the response.
    let request_id = request.extensions().get::<RequestId>().map_or("-", |id| {
        id.header_value().to_str().unwrap_or("<non-ascii>")
    });

    tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %request.method(),
        path = %request.uri().path(),
    )
}
