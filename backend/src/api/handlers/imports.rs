//! `POST /api/trades/import`.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use crate::AppState;
use crate::api::ApiError;
use crate::api::dto::ImportResponse;
use crate::api::extractors::ApiMultipart;
use crate::error::AppError;
use crate::import::{self, Outcome};

/// Multipart form field `file`, exactly once. Returns 200 when the file was
/// imported and 422 when it was rejected; both carry the full import report.
pub(in crate::api) async fn import_trades(
    State(state): State<AppState>,
    multipart: ApiMultipart,
) -> Result<(StatusCode, Json<ImportResponse>), ApiError> {
    let mut multipart = multipart.into_inner();
    // The same value the router gave this route's `DefaultBodyLimit`.
    let max_bytes = state.api.max_upload_bytes();
    let read_error = |error| ApiError::from_multipart_body(error, max_bytes);

    // Exactly one `file` field; other fields are ignored. A second `file` is
    // rejected before its contents are read, and nothing is imported.
    let mut upload = None;

    while let Some(field) = multipart.next_field().await.map_err(read_error)? {
        if field.name() != Some("file") {
            continue;
        }

        if upload.is_some() {
            return Err(ApiError::MultipleFiles);
        }

        let file_name = field.file_name().map(str::to_owned);
        let bytes = field.bytes().await.map_err(read_error)?;

        upload = Some((file_name, bytes));
    }

    let (file_name, bytes) = upload.ok_or(ApiError::MissingFile)?;

    // Ok(report) covers both outcomes, imported and rejected; only an
    // unexpected failure (for example the database) is an error here.
    let report = import::import_trades(&state.pool, state.manifest.valuation_dates(), &bytes)
        .await
        .map_err(AppError::from)?;

    tracing::info!(
        file = file_name.as_deref().unwrap_or("<unnamed>"),
        outcome = ?report.outcome,
        total = ?report.total_rows,
        inserted = ?report.inserted_rows,
        skipped = ?report.skipped_rows,
        invalid = ?report.invalid_rows,
        "trade import finished"
    );

    let status = match report.outcome {
        Outcome::Success => StatusCode::OK,
        Outcome::Rejected => StatusCode::UNPROCESSABLE_ENTITY,
    };

    Ok((status, Json(ImportResponse::new(file_name, report))))
}
