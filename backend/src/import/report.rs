//! The import report: what happened to an uploaded file, in the shape shown
//! to the user. Typed errors become text here, at the report boundary.

use serde::Serialize;

use super::error::FileError;
use super::validation::ParsedRow;

/// Whether the file was imported. A rejected file changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Success,
    Rejected,
}

/// One entry of the report's `errors`: every problem with one row, or a
/// file-level problem.
#[derive(Debug, Clone)]
pub struct RowError {
    /// `None` for a file-level error.
    pub row: Option<usize>,
    pub trade_id: Option<String>,
    pub messages: Vec<String>,
}

/// The import result returned to the client. Row counts are `None` only for
/// file-level failures, where rows were never examined.
///
/// The counts partition the file's data rows: on success
/// `inserted + skipped = total`; on rejection `inserted = skipped = 0` and
/// `invalid + aborted = total`.
#[derive(Debug, Clone)]
pub struct ImportReport {
    pub outcome: Outcome,
    pub message: String,
    pub total_rows: Option<usize>,
    pub inserted_rows: Option<usize>,
    pub skipped_rows: Option<usize>,
    pub invalid_rows: Option<usize>,
    pub aborted_rows: Option<usize>,
    pub errors: Vec<RowError>,
}

impl ImportReport {
    /// The file as a whole was unusable, so no rows were examined.
    pub(super) fn file_rejected(error: FileError) -> Self {
        let reason = error.to_string();

        Self {
            outcome: Outcome::Rejected,
            message: format!("File rejected: {reason} No trades were imported."),
            total_rows: None,
            inserted_rows: None,
            skipped_rows: None,
            invalid_rows: None,
            aborted_rows: None,
            errors: vec![RowError {
                row: None,
                trade_id: None,
                messages: vec![reason],
            }],
        }
    }

    /// At least one row is invalid. Every invalid row is listed once, in file
    /// order, with all of its problems; every other row counts as aborted.
    pub(super) fn rows_rejected(rows: &[ParsedRow]) -> Self {
        let errors: Vec<RowError> = rows
            .iter()
            .filter(|row| !row.is_valid())
            .map(|row| RowError {
                row: Some(row.row_number),
                trade_id: row.raw_trade_id().map(str::to_owned),
                messages: row.errors.iter().map(ToString::to_string).collect(),
            })
            .collect();

        let total = rows.len();
        let invalid = errors.len();

        Self {
            outcome: Outcome::Rejected,
            message: format!(
                "File rejected: {invalid} of {total} rows are invalid. No trades were imported \
                 and stored trades are unchanged."
            ),
            total_rows: Some(total),
            inserted_rows: Some(0),
            skipped_rows: Some(0),
            invalid_rows: Some(invalid),
            aborted_rows: Some(total - invalid),
            errors,
        }
    }

    /// Every row was valid: `inserted` new trades were stored and the rest
    /// of the `total` rows were exact duplicates, either of a stored trade or
    /// of an earlier row in the same file.
    pub(super) fn succeeded(total: usize, inserted: usize) -> Self {
        let skipped = total - inserted;

        Self {
            outcome: Outcome::Success,
            message: format!(
                "Import succeeded: {inserted} trades inserted, {skipped} duplicate rows skipped."
            ),
            total_rows: Some(total),
            inserted_rows: Some(inserted),
            skipped_rows: Some(skipped),
            invalid_rows: Some(0),
            aborted_rows: Some(0),
            errors: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::parse;
    use super::super::validation::mark_in_file_conflicts;
    use super::*;

    #[test]
    fn file_rejection_has_one_file_level_error_and_no_counts() {
        let report = ImportReport::file_rejected(FileError::NoRows);

        assert_eq!(report.outcome, Outcome::Rejected);
        assert_eq!(
            report.message,
            "File rejected: The file has a header but no trade rows. No trades were imported."
        );
        assert_eq!(report.total_rows, None);
        assert_eq!(report.invalid_rows, None);

        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].row, None);
        assert_eq!(report.errors[0].trade_id, None);
        assert_eq!(
            report.errors[0].messages,
            vec!["The file has a header but no trade rows."]
        );
    }

    #[test]
    fn row_rejection_lists_invalid_rows_in_file_order_and_counts_the_rest_as_aborted() {
        let mut rows = parse(concat!(
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T2,P999,I999,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T3,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            ",P001,I001,2026-01-05T10:00:00Z,BUY,0,10\n",
        ))
        .unwrap();
        mark_in_file_conflicts(&mut rows);

        let report = ImportReport::rows_rejected(&rows);

        assert_eq!(report.outcome, Outcome::Rejected);
        assert_eq!(
            report.message,
            "File rejected: 2 of 4 rows are invalid. No trades were imported \
             and stored trades are unchanged."
        );
        assert_eq!(report.total_rows, Some(4));
        assert_eq!(report.inserted_rows, Some(0));
        assert_eq!(report.skipped_rows, Some(0));
        assert_eq!(report.invalid_rows, Some(2));
        assert_eq!(report.aborted_rows, Some(2));

        // One entry per invalid row, each with every problem found on it.
        let rows_reported: Vec<Option<usize>> = report.errors.iter().map(|e| e.row).collect();
        assert_eq!(rows_reported, vec![Some(3), Some(5)]);
        assert_eq!(report.errors[0].trade_id.as_deref(), Some("T2"));
        assert_eq!(report.errors[0].messages.len(), 2);
        assert_eq!(report.errors[1].trade_id, None);
        assert_eq!(report.errors[1].messages.len(), 2);
    }

    #[test]
    fn success_counts_the_rows_not_inserted_as_skipped() {
        let report = ImportReport::succeeded(5, 3);

        assert_eq!(report.outcome, Outcome::Success);
        assert_eq!(
            report.message,
            "Import succeeded: 3 trades inserted, 2 duplicate rows skipped."
        );
        assert_eq!(report.total_rows, Some(5));
        assert_eq!(report.inserted_rows, Some(3));
        assert_eq!(report.skipped_rows, Some(2));
        assert_eq!(report.invalid_rows, Some(0));
        assert_eq!(report.aborted_rows, Some(0));
        assert!(report.errors.is_empty());
    }
}
