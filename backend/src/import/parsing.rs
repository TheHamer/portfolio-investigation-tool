//! CSV decoding and header verification.

use csv::StringRecord;

use super::error::FileError;

/// The exact header the data contract requires, in order.
pub(super) const TRADE_HEADER: [&str; 7] = [
    "trade_id",
    "portfolio_id",
    "instrument_id",
    "timestamp",
    "side",
    "quantity",
    "price",
];

/// One data record as read from the file, before any validation.
#[derive(Debug)]
pub(super) struct RawRow {
    /// 1-based line number the record starts on; the header is row 1.
    pub(super) row_number: usize,
    pub(super) record: StringRecord,
}

/// Decodes the upload and returns its data records. Fails only for problems
/// that make row-by-row validation meaningless: wrong encoding, empty file,
/// wrong header, unreadable CSV, or no data rows.
pub(super) fn read_trade_csv(bytes: &[u8]) -> Result<Vec<RawRow>, FileError> {
    // A UTF-8 byte-order mark (as written by Excel) is tolerated.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(bytes).map_err(FileError::InvalidEncoding)?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(FileError::Empty);
    }

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        // Rows with the wrong number of fields are reported per row instead of
        // aborting the parse.
        .flexible(true)
        .from_reader(bytes);

    let header = reader
        .headers()
        .map_err(|source| FileError::HeaderUnreadable { source })?;
    if !header.iter().eq(TRADE_HEADER) {
        return Err(FileError::InvalidHeader {
            actual: header.iter().collect::<Vec<_>>().join(","),
        });
    }

    // A row's number is the line it is on, as an editor or spreadsheet shows
    // it. The csv reader skips blank lines, so the record index alone would
    // undercount after one; it is only the fallback.
    let mut rows = Vec::new();
    for (index, record) in reader.records().enumerate() {
        let fallback = index + 2;
        let record = record.map_err(|source| FileError::RowUnreadable {
            row_number: line_number(bytes, source.position(), fallback),
            source,
        })?;
        let row_number = line_number(bytes, record.position(), fallback);
        rows.push(RawRow { row_number, record });
    }

    if rows.is_empty() {
        return Err(FileError::NoRows);
    }

    Ok(rows)
}

/// The 1-based line a record's content starts on (the header is line 1), or
/// `fallback` when the reader did not record a position.
///
/// The reader positions a record where it started reading it, which is before
/// any blank lines it then skipped, so those are counted on top.
fn line_number(bytes: &[u8], position: Option<&csv::Position>, fallback: usize) -> usize {
    let Some(position) = position else {
        return fallback;
    };

    let blank_lines_skipped = usize::try_from(position.byte())
        .ok()
        .and_then(|start| bytes.get(start..))
        .map_or(0, |rest| {
            rest.iter()
                .take_while(|&&b| b == b'\n' || b == b'\r')
                .filter(|&&b| b == b'\n')
                .count()
        });

    usize::try_from(position.line()).map_or(fallback, |line| line + blank_lines_skipped)
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    const HEADER: &str = "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n";

    #[test]
    fn quoted_fields_are_parsed_properly() {
        let file =
            format!("{HEADER}\"T,1\",\"P001\",I001,2026-01-05T10:00:00Z,BUY,\"100\",\"10.5\"\n");
        let rows = read_trade_csv(file.as_bytes()).unwrap();

        assert_eq!(&rows[0].record[0], "T,1");
        assert_eq!(&rows[0].record[5], "100");
        assert_eq!(rows[0].row_number, 2);
    }

    #[test]
    fn file_level_errors_are_distinct_variants() {
        assert!(matches!(read_trade_csv(b""), Err(FileError::Empty)));
        assert!(matches!(read_trade_csv(b"  \n "), Err(FileError::Empty)));
        assert!(matches!(
            read_trade_csv(HEADER.as_bytes()),
            Err(FileError::NoRows)
        ));
        assert!(matches!(
            read_trade_csv(b"\xff\xfe\x00"),
            Err(FileError::InvalidEncoding(_))
        ));

        let wrong = "trade_id,portfolio,instrument_id,timestamp,side,quantity,price\n\
                     T1,P001,I001,2026-01-05T10:00:00Z,BUY,1,1\n";
        match read_trade_csv(wrong.as_bytes()) {
            Err(FileError::InvalidHeader { actual }) => assert_eq!(
                actual,
                "trade_id,portfolio,instrument_id,timestamp,side,quantity,price"
            ),
            other => panic!("unexpected result: {other:?}"),
        }

        // A UTF-8 byte-order mark (as written by Excel) is tolerated.
        let bom = format!("\u{feff}{HEADER}T1,P001,I001,2026-01-05T10:00:00Z,BUY,1,1\n");
        assert!(read_trade_csv(bom.as_bytes()).is_ok());
    }

    /// Row numbers are the lines a user sees in an editor or spreadsheet. A
    /// blank line is not a record, but it still occupies a line, so the rows
    /// after it keep their physical numbers.
    #[test]
    fn row_numbers_are_physical_lines_even_after_blank_lines() {
        // Line 3 is blank; lines 5 and 6 are blank with Windows line endings.
        let file = format!(
            "{HEADER}T1,P001,I001,2026-01-05T10:00:00Z,BUY,1,1\n\
             \n\
             T2,P001,I001,2026-01-05T10:00:00Z,BUY,1,1\r\n\
             \r\n\
             \r\n\
             T3,P001,I001,2026-01-05T10:00:00Z,BUY,1,1"
        );

        let rows = read_trade_csv(file.as_bytes()).unwrap();

        let numbered: Vec<(usize, &str)> = rows
            .iter()
            .map(|row| (row.row_number, &row.record[0]))
            .collect();
        assert_eq!(numbered, vec![(2, "T1"), (4, "T2"), (7, "T3")]);
    }

    #[test]
    fn short_rows_are_returned_for_row_level_reporting() {
        let file = format!("{HEADER}T10,P001,I001\n");
        assert_eq!(read_trade_csv(file.as_bytes()).unwrap()[0].record.len(), 3);
    }

    #[test]
    fn file_error_messages_are_complete_sentences() {
        assert_eq!(FileError::Empty.to_string(), "The file is empty.");
        assert_eq!(
            FileError::NoRows.to_string(),
            "The file has a header but no trade rows."
        );

        let bytes = vec![0xff_u8];
        let invalid_utf8 = std::str::from_utf8(&bytes).unwrap_err();
        assert_eq!(
            FileError::InvalidEncoding(invalid_utf8).to_string(),
            "The file is not valid UTF-8 text."
        );
        assert_eq!(
            FileError::InvalidHeader {
                actual: "id,portfolio_id".into()
            }
            .to_string(),
            "The header must be exactly \
             'trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price' \
             but was 'id,portfolio_id'."
        );
    }

    #[test]
    fn unreadable_csv_keeps_row_number_and_underlying_error() {
        // The csv crate is lenient enough that byte input which passed the
        // UTF-8 check practically never fails to parse, so build the
        // underlying error directly and check what the variant preserves.
        let source = csv::Error::from(std::io::Error::other("stream interrupted"));
        let error = FileError::RowUnreadable {
            row_number: 7,
            source,
        };

        assert_eq!(
            error.to_string(),
            "Row 7 could not be parsed as CSV: stream interrupted."
        );
        assert!(error.source().is_some());
    }
}
