//! HTTP error boundary: the only place errors become status codes and JSON.

use axum::Json;
use axum::extract::multipart::{MultipartError, MultipartRejection};
use axum::extract::rejection::PathRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use thiserror::Error;
use tokio::time::error::Elapsed;

use crate::error::AppError;
use crate::persistence::PersistenceError;

const INTERNAL_MESSAGE: &str = "An internal error occurred. Please try again.";

/// Errors a handler can return. The variants up to `MethodNotAllowed` are
/// problems with the request itself; the `Database*` variants are the
/// readiness probe's 503; `Application` wraps a failure from the application
/// layers. Messages are written for API clients.
///
/// Extractor rejections arrive through the `#[from]` conversions, which the
/// `api::extractors` aliases rely on. They keep Axum's typed rejection, and
/// its own status decides between a 4xx and an internal error.
///
/// A rejected trade upload is not an `ApiError`: it is a successful handler
/// result carrying an import report with status 422.
#[derive(Debug, Error)]
pub enum ApiError {
    /// A path parameter could not be extracted (`ApiPath`).
    #[error("Invalid request path: {0}")]
    InvalidPath(#[from] PathRejection),
    /// The upload request is not a multipart form (`ApiMultipart`).
    #[error("Expected a multipart form upload: {0}")]
    NotMultipart(#[from] MultipartRejection),
    #[error("No file was uploaded (expected form field 'file').")]
    MissingFile,
    /// A second `file` field, rejected before its contents are read.
    #[error("More than one file was uploaded (expected a single form field 'file').")]
    MultipleFiles,
    /// The multipart body could not be read, for a reason other than its
    /// size. Built by [`ApiError::from_multipart_body`].
    #[error("The upload could not be read: {}", .0.body_text())]
    UnreadableUpload(#[source] MultipartError),
    /// The upload request exceeded the configured `api.max_upload_bytes`.
    /// The limit applies to the whole multipart request body, not just the
    /// file inside it, and the message says so.
    #[error("The upload request exceeds the maximum size of {}.", byte_size(*max_bytes))]
    PayloadTooLarge { max_bytes: usize },
    #[error("No API endpoint at {path}.")]
    RouteNotFound { path: String },
    /// The path exists but not for this method. Axum adds the `Allow` header.
    #[error("The requested HTTP method is not supported.")]
    MethodNotAllowed,
    /// The readiness probe's database query failed. The cause is logged,
    /// never returned.
    #[error("The database is currently unavailable.")]
    DatabaseUnavailable(#[source] PersistenceError),
    /// The readiness probe's database query did not finish in time.
    #[error("The database is currently unavailable.")]
    DatabaseTimeout(#[source] Elapsed),
    #[error(transparent)]
    Application(#[from] AppError),
}

/// An exact, readable size: whole MiB or KiB where the value divides evenly
/// (1 MiB = 1024 KiB = 1,048,576 bytes), bytes otherwise. Never rounds, so the
/// limit shown is the limit applied.
fn byte_size(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * KIB;
    match bytes {
        1 => "1 byte".to_owned(),
        b if b >= MIB && b % MIB == 0 => format!("{} MiB", b / MIB),
        b if b >= KIB && b % KIB == 0 => format!("{} KiB", b / KIB),
        b => format!("{b} bytes"),
    }
}

/// Status and code for a failure that Axum has already classified (an
/// extractor rejection or a multipart read error). A client error keeps its
/// status. Anything else, such as a route whose parameters do not match its
/// `Path` type or a failure reading the body stream, is an internal error,
/// never a 400.
fn classified(status: StatusCode) -> (StatusCode, &'static str) {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        (status, "payload_too_large")
    } else if status.is_client_error() {
        (status, "bad_request")
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
    }
}

impl ApiError {
    /// Converts an error from reading a multipart body (`next_field()`,
    /// `bytes()`). This needs the configured upload limit, so it is a method
    /// rather than a `From` impl.
    ///
    /// Axum reports the body-size limit here, as a `MultipartError` with
    /// status 413, never as a `MultipartRejection`: in axum 0.8 that only
    /// means a missing or invalid boundary. The status comes from Axum's
    /// typed error, not from its message.
    pub(super) fn from_multipart_body(error: MultipartError, max_bytes: usize) -> Self {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Self::PayloadTooLarge { max_bytes }
        } else {
            Self::UnreadableUpload(error)
        }
    }

    /// Status and machine-readable code. Mapping is by variant, never by
    /// message text.
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            ApiError::InvalidPath(rejection) => classified(rejection.status()),
            ApiError::NotMultipart(rejection) => classified(rejection.status()),
            ApiError::UnreadableUpload(error) => classified(error.status()),
            ApiError::MissingFile | ApiError::MultipleFiles => {
                (StatusCode::BAD_REQUEST, "bad_request")
            }
            ApiError::PayloadTooLarge { .. } => {
                (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
            }
            ApiError::RouteNotFound { .. } => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed"),
            ApiError::DatabaseUnavailable(_) | ApiError::DatabaseTimeout(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable")
            }
            // Missing portfolio / instrument / position: an expected outcome.
            ApiError::Application(AppError::Portfolio(error)) if error.is_not_found() => {
                (StatusCode::NOT_FOUND, "not_found")
            }
            ApiError::Application(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();

        // Every server-side failure is logged with its underlying cause.
        // Internal errors then get a generic message, so details (SQL, file
        // paths) never reach the client; other statuses, including 503, carry
        // their own message, which contains no such details.
        if status.is_server_error() {
            tracing::error!(
                error = %self,
                source = std::error::Error::source(&self).map(tracing::field::display),
                "request failed"
            );
        }

        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            INTERNAL_MESSAGE.to_owned()
        } else {
            self.to_string()
        };

        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::PersistenceError;
    use crate::portfolio::PortfolioError;
    use axum::body::to_bytes;
    use serde_json::Value;

    async fn render(error: ApiError) -> (StatusCode, Value) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    #[tokio::test]
    async fn missing_resources_map_to_404_with_their_message() {
        use crate::domain::{InstrumentId, PortfolioId};

        let portfolio = |id| PortfolioId::parse(id).unwrap();
        let error = AppError::from(PortfolioError::PortfolioNotFound(portfolio("P999")));
        let (status, body) = render(ApiError::from(error)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            json!({ "error": { "code": "not_found", "message": "Portfolio 'P999' does not exist." } })
        );

        let error = AppError::from(PortfolioError::PositionNotFound {
            portfolio_id: portfolio("P001"),
            instrument_id: InstrumentId::parse("I002").unwrap(),
        });
        let (status, body) = render(ApiError::from(error)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body["error"]["message"],
            "Portfolio 'P001' has no trades in instrument 'I002'."
        );
    }

    #[tokio::test]
    async fn infrastructure_failures_map_to_a_generic_500() {
        // A database failure reached through the portfolio layer: details such
        // as the database path must not reach the client.
        let database = PersistenceError::Open {
            path: "/secret/location/portfolio.db".into(),
            source: sqlx::Error::PoolClosed,
        };

        for error in [
            AppError::from(PortfolioError::from(database)),
            AppError::from(PersistenceError::Database(sqlx::Error::PoolTimedOut)),
            AppError::from(PortfolioError::UnknownInstrument(
                crate::domain::InstrumentId::parse("I042").unwrap(),
            )),
        ] {
            let (status, body) = render(ApiError::from(error)).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(
                body,
                json!({ "error": { "code": "internal_error", "message": INTERNAL_MESSAGE } })
            );
        }
    }

    #[tokio::test]
    async fn request_errors_keep_their_status_and_code() {
        let (status, body) = render(ApiError::MissingFile).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            json!({ "error": { "code": "bad_request", "message": "No file was uploaded (expected form field 'file')." } })
        );

        let (status, body) = render(ApiError::PayloadTooLarge {
            max_bytes: 10 * 1024 * 1024,
        })
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            body,
            json!({ "error": { "code": "payload_too_large", "message": "The upload request exceeds the maximum size of 10 MiB." } })
        );

        let (status, body) = render(ApiError::RouteNotFound {
            path: "/api/nope".into(),
        })
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            json!({ "error": { "code": "not_found", "message": "No API endpoint at /api/nope." } })
        );

        let (status, body) = render(ApiError::MultipleFiles).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            json!({ "error": { "code": "bad_request", "message": "More than one file was uploaded (expected a single form field 'file')." } })
        );

        let (status, body) = render(ApiError::MethodNotAllowed).await;
        assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            body,
            json!({ "error": { "code": "method_not_allowed", "message": "The requested HTTP method is not supported." } })
        );
    }

    #[tokio::test]
    async fn unavailable_database_is_a_503_without_the_cause() {
        let database = PersistenceError::Open {
            path: "/secret/location/portfolio.db".into(),
            source: sqlx::Error::PoolClosed,
        };

        let (status, body) = render(ApiError::DatabaseUnavailable(database)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body,
            json!({ "error": { "code": "service_unavailable", "message": "The database is currently unavailable." } })
        );
    }

    #[test]
    fn classified_failures_keep_client_statuses_and_hide_server_ones() {
        assert_eq!(
            classified(StatusCode::BAD_REQUEST),
            (StatusCode::BAD_REQUEST, "bad_request")
        );
        assert_eq!(
            classified(StatusCode::PAYLOAD_TOO_LARGE),
            (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large")
        );
        // A server-side read or routing failure is never reported as a 400.
        for status in [
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            assert_eq!(
                classified(status),
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
            );
        }
    }

    #[test]
    fn payload_too_large_message_states_the_exact_limit() {
        let message = |max_bytes| ApiError::PayloadTooLarge { max_bytes }.to_string();

        // The default.
        assert_eq!(
            message(10 * 1024 * 1024),
            "The upload request exceeds the maximum size of 10 MiB."
        );
        assert_eq!(
            message(1024 * 1024),
            "The upload request exceeds the maximum size of 1 MiB."
        );
        // Not a whole number of MiB: integer division would have said "0 MiB"
        // or "1 MiB" for these.
        assert_eq!(
            message(512 * 1024),
            "The upload request exceeds the maximum size of 512 KiB."
        );
        assert_eq!(
            message(1024 * 1024 + 1024),
            "The upload request exceeds the maximum size of 1025 KiB."
        );
        // Not a whole number of KiB either: exact bytes, never rounded.
        assert_eq!(
            message(1_500_000),
            "The upload request exceeds the maximum size of 1500000 bytes."
        );
        assert_eq!(
            message(10 * 1024 * 1024 + 1),
            "The upload request exceeds the maximum size of 10485761 bytes."
        );
        assert_eq!(
            message(1500),
            "The upload request exceeds the maximum size of 1500 bytes."
        );
        assert_eq!(
            message(1),
            "The upload request exceeds the maximum size of 1 byte."
        );
    }
}
