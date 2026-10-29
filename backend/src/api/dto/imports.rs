//! The trade import response.

use serde::Serialize;

use crate::import::{ImportReport, Outcome, RowError};

/// The import result: the uploaded file's name followed by the report's
/// fields, all at the same level.
#[derive(Serialize)]
pub(in crate::api) struct ImportResponse {
    file_name: Option<String>,
    outcome: Outcome,
    message: String,
    total_rows: Option<usize>,
    inserted_rows: Option<usize>,
    skipped_rows: Option<usize>,
    invalid_rows: Option<usize>,
    aborted_rows: Option<usize>,
    errors: Vec<ImportRowError>,
}

impl ImportResponse {
    pub(in crate::api) fn new(file_name: Option<String>, report: ImportReport) -> Self {
        Self {
            file_name,
            outcome: report.outcome,
            message: report.message,
            total_rows: report.total_rows,
            inserted_rows: report.inserted_rows,
            skipped_rows: report.skipped_rows,
            invalid_rows: report.invalid_rows,
            aborted_rows: report.aborted_rows,
            errors: report
                .errors
                .into_iter()
                .map(ImportRowError::from)
                .collect(),
        }
    }
}

/// One entry of `errors`. `row` is `null` for a file-level error.
#[derive(Serialize)]
struct ImportRowError {
    row: Option<usize>,
    trade_id: Option<String>,
    messages: Vec<String>,
}

impl From<RowError> for ImportRowError {
    fn from(error: RowError) -> Self {
        Self {
            row: error.row,
            trade_id: error.trade_id,
            messages: error.messages,
        }
    }
}

#[cfg(test)]
mod tests {
    //! The exact JSON text of the import response: the report's fields at the
    //! top level, in a fixed order, with `null` for absent values.

    use super::*;
    use crate::api::dto::json;

    #[test]
    fn import_response_keeps_report_fields_at_the_top_level() {
        let imported = ImportReport {
            outcome: Outcome::Success,
            message: "Imported 2 trades.".into(),
            total_rows: Some(3),
            inserted_rows: Some(2),
            skipped_rows: Some(1),
            invalid_rows: Some(0),
            aborted_rows: Some(0),
            errors: vec![],
        };

        assert_eq!(
            json(&ImportResponse::new(Some("trades.csv".into()), imported)),
            concat!(
                r#"{"file_name":"trades.csv","outcome":"success","message":"Imported 2 trades.","#,
                r#""total_rows":3,"inserted_rows":2,"skipped_rows":1,"invalid_rows":0,"aborted_rows":0,"#,
                r#""errors":[]}"#
            )
        );

        let rejected = ImportReport {
            outcome: Outcome::Rejected,
            message: "File rejected.".into(),
            total_rows: None,
            inserted_rows: None,
            skipped_rows: None,
            invalid_rows: None,
            aborted_rows: None,
            errors: vec![
                RowError {
                    row: None,
                    trade_id: None,
                    messages: vec!["Missing header.".into()],
                },
                RowError {
                    row: Some(3),
                    trade_id: Some("T1".into()),
                    messages: vec!["a".into(), "b".into()],
                },
            ],
        };

        assert_eq!(
            json(&ImportResponse::new(None, rejected)),
            concat!(
                r#"{"file_name":null,"outcome":"rejected","message":"File rejected.","#,
                r#""total_rows":null,"inserted_rows":null,"skipped_rows":null,"invalid_rows":null,"aborted_rows":null,"#,
                r#""errors":[{"row":null,"trade_id":null,"messages":["Missing header."]},"#,
                r#"{"row":3,"trade_id":"T1","messages":["a","b"]}]}"#
            )
        );
    }
}
