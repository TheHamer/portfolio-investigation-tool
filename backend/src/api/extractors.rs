//! Extractor wrappers that report failures as [`ApiError`].
//!
//! Axum's fallible extractors, such as `Path` and `Multipart`, reject a bad
//! request with their own rejection type, and by default that rejection is
//! sent as Axum's plain-text response. That would bypass the API's error
//! contract: every error is `{"error": {"code", "message"}}` with a status
//! chosen in `error.rs`.
//!
//! `WithRejection<E, ApiError>` still lets Axum's extractor `E` do the
//! extraction, but converts its rejection into `ApiError` (through the
//! `From` impls on `ApiError`). Handlers name these aliases and get the
//! standard error response for free, without matching on rejections
//! themselves.
//!
//! Only the extraction step is covered. A multipart body is read after
//! extraction, and errors from `next_field()` or `bytes()` are converted
//! separately with `ApiError::from_multipart_body`, which also needs the
//! configured upload limit.
//!
//! `State` and `OriginalUri` cannot fail, so they are used unwrapped.

use axum::extract::{Multipart, Path, Query};
use axum_extra::extract::WithRejection;

use super::error::ApiError;

/// `Path<T>` whose rejection becomes `ApiError::InvalidPath`.
pub type ApiPath<T> = WithRejection<Path<T>, ApiError>;

/// `Query<T>` whose rejection becomes `ApiError::InvalidQuery`.
pub type ApiQuery<T> = WithRejection<Query<T>, ApiError>;

/// `Multipart` whose rejection (no valid `multipart/form-data` boundary)
/// becomes `ApiError::NotMultipart`.
pub type ApiMultipart = WithRejection<Multipart, ApiError>;

#[cfg(test)]
mod tests {
    //! Path rejections the real routes cannot produce: a parse failure for a
    //! typed parameter, and the server-side failures caused by a route that
    //! does not match its extractor.

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::*;

    async fn number(path: ApiPath<u32>) -> String {
        let Path(n) = path.into_inner();
        n.to_string()
    }

    async fn get_status_and_body(uri: &str) -> (StatusCode, Vec<u8>) {
        let app = Router::new()
            .route("/numbers/{n}", get(number))
            // Misconfigured on purpose: one `u32` from two parameters, or
            // from none.
            .route("/numbers/{n}/{m}", get(number))
            .route("/no-parameters", get(number));

        let response = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, body.to_vec())
    }

    #[tokio::test]
    async fn valid_parameter_is_extracted() {
        let (status, body) = get_status_and_body("/numbers/42").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"42");
    }

    #[tokio::test]
    async fn unparsable_parameter_is_a_structured_400() {
        let (status, body) = get_status_and_body("/numbers/abc").await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "bad_request");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.starts_with("Invalid request path: "), "{message}");
        assert!(message.contains("abc"), "{message}");
    }

    #[tokio::test]
    async fn route_and_extractor_mismatch_is_a_generic_500_not_a_400() {
        for uri in ["/numbers/1/2", "/no-parameters"] {
            let (status, body) = get_status_and_body(uri).await;

            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}");
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                body,
                json!({ "error": {
                    "code": "internal_error",
                    "message": "An internal error occurred. Please try again."
                } }),
                "{uri}"
            );
        }
    }
}
