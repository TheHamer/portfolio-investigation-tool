//! Row validation: field rules (via the domain types), contextual rules
//! (reference IDs, valuation calendar) and in-file duplicate/conflict
//! detection. Nothing here touches the database.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::NaiveDate;
use csv::StringRecord;

use super::error::RowValidationError;
use super::parsing::{RawRow, TRADE_HEADER};
use crate::domain::{
    FieldError, InstrumentId, PortfolioId, Price, Trade, TradeId, TradeQuantity, TradeSide,
    TradeTimestamp,
};

/// Reference data needed for contextual validation.
pub(super) struct ImportContext {
    pub(super) portfolios: HashSet<PortfolioId>,
    pub(super) instruments: HashSet<InstrumentId>,
    pub(super) valuation_dates: BTreeSet<NaiveDate>,
}

/// A data row and everything learned about it so far.
#[derive(Debug)]
pub(super) struct ParsedRow {
    /// 1-based file row number; the header is row 1.
    pub(super) row_number: usize,
    pub(super) raw: StringRecord,
    /// Present only when the row has no errors of its own (field or reference
    /// data). A later in-file or stored-trade conflict adds errors but keeps it.
    pub(super) trade: Option<Trade>,
    /// Every problem found with this row, in the order found.
    pub(super) errors: Vec<RowValidationError>,
}

impl ParsedRow {
    pub(super) fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// The trade ID as written, if non-empty. Available even when the row
    /// failed to parse, so invalid rows can still be grouped by ID.
    pub(super) fn raw_trade_id(&self) -> Option<&str> {
        self.raw.get(0).filter(|id| !id.is_empty())
    }
}

/// Validates every row in isolation (fields and reference data).
pub(super) fn validate_rows(rows: Vec<RawRow>, ctx: &ImportContext) -> Vec<ParsedRow> {
    rows.into_iter()
        .map(|RawRow { row_number, record }| {
            let (trade, errors) = match validate_record(&record, ctx) {
                Ok(trade) => (Some(trade), Vec::new()),
                Err(errors) => (None, errors),
            };
            ParsedRow {
                row_number,
                raw: record,
                trade,
                errors,
            }
        })
        .collect()
}

fn validate_record(
    raw: &StringRecord,
    ctx: &ImportContext,
) -> Result<Trade, Vec<RowValidationError>> {
    // Destructuring by pattern, rather than indexing, means a row with the
    // wrong number of fields is an error by construction and cannot panic.
    let fields: Vec<&str> = raw.iter().collect();
    let [
        trade_id,
        portfolio_id,
        instrument_id,
        timestamp,
        side,
        quantity,
        price,
    ] = fields[..]
    else {
        return Err(vec![RowValidationError::WrongFieldCount {
            expected: TRADE_HEADER.len(),
            actual: raw.len(),
        }]);
    };

    // Validate every field (rather than stopping at the first failure) so the
    // user sees all of a row's problems at once.
    fn check<T>(errors: &mut Vec<RowValidationError>, result: Result<T, FieldError>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                errors.push(error.into());
                None
            }
        }
    }

    let mut errors = Vec::new();
    let trade_id = check(&mut errors, TradeId::parse(trade_id));
    let portfolio_id = check(&mut errors, PortfolioId::parse(portfolio_id));
    let instrument_id = check(&mut errors, InstrumentId::parse(instrument_id));
    let timestamp = check(&mut errors, TradeTimestamp::parse(timestamp));
    let side = check(&mut errors, side.parse::<TradeSide>());
    let quantity = check(&mut errors, TradeQuantity::parse(quantity));
    let price = check(&mut errors, Price::parse(price));

    if let Some(id) = &portfolio_id
        && !ctx.portfolios.contains(id)
    {
        errors.push(RowValidationError::UnknownPortfolio {
            portfolio_id: id.clone(),
        });
    }

    if let Some(id) = &instrument_id
        && !ctx.instruments.contains(id)
    {
        errors.push(RowValidationError::UnknownInstrument {
            instrument_id: id.clone(),
        });
    }

    if let Some(ts) = &timestamp
        && !ctx.valuation_dates.contains(&ts.utc_date())
    {
        let calendar = ctx
            .valuation_dates
            .first()
            .zip(ctx.valuation_dates.last())
            .map(|(first, last)| (*first, *last));
        errors.push(RowValidationError::NotAValuationDate {
            date: ts.utc_date(),
            calendar,
        });
    }

    match (
        trade_id,
        portfolio_id,
        instrument_id,
        timestamp,
        side,
        quantity,
        price,
    ) {
        (
            Some(trade_id),
            Some(portfolio_id),
            Some(instrument_id),
            Some(timestamp),
            Some(side),
            Some(quantity),
            Some(price),
        ) if errors.is_empty() => Ok(Trade {
            trade_id,
            portfolio_id,
            instrument_id,
            timestamp,
            side,
            quantity,
            price,
        }),
        _ => Err(errors),
    }
}

/// Groups row indices by raw trade ID. Groups are ordered by each ID's first
/// appearance and hold their rows in file order. Rows without a trade ID
/// belong to no group.
pub(super) fn group_by_trade_id(rows: &[ParsedRow]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut group_of_id: HashMap<&str, usize> = HashMap::new();

    for (index, row) in rows.iter().enumerate() {
        let Some(id) = row.raw_trade_id() else {
            continue;
        };

        match group_of_id.get(id) {
            Some(&group) => groups[group].push(index),
            None => {
                group_of_id.insert(id, groups.len());
                groups.push(vec![index]);
            }
        }
    }

    groups
}

/// A trade ID repeated within the file is fine if every occurrence has the
/// same normalized values (it will be inserted once). Otherwise every row
/// using that ID is invalid.
pub(super) fn mark_in_file_conflicts(rows: &mut [ParsedRow]) {
    for group in group_by_trade_id(rows) {
        if group.len() < 2 {
            continue;
        }

        let first = &rows[group[0]];
        let all_parsed = group.iter().all(|&i| rows[i].trade.is_some());
        // Parsed trades compare by value (10.0 == 10.0000); rows that failed
        // to parse can only be compared by their raw text.
        let conflicting = if all_parsed {
            group.iter().any(|&i| rows[i].trade != first.trade)
        } else {
            group.iter().any(|&i| rows[i].raw != first.raw)
        };

        if conflicting {
            let conflict = RowValidationError::ConflictingTradeIdInFile {
                trade_id: first.raw_trade_id().unwrap_or_default().to_owned(),
                rows: group.iter().map(|&i| rows[i].row_number).collect(),
            };
            for &i in &group {
                rows[i].errors.push(conflict.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::parse;
    use super::*;

    #[test]
    fn valid_row_parses_into_trade() {
        let rows = parse("T1,P001,I001,2026-01-05T10:00:00Z,BUY,100,10.5\n").unwrap();
        let trade = rows[0].trade.as_ref().unwrap();

        assert_eq!(trade.signed_quantity(), 100);
        assert_eq!(rows[0].row_number, 2);
    }

    #[test]
    fn quoted_fields_validate() {
        let rows =
            parse("\"T1\",\"P001\",I001,2026-01-05T10:00:00Z,BUY,\"100\",\"10.5\"\n").unwrap();
        assert!(rows[0].trade.is_some());

        let rows = parse("\"T,1\",P001,I001,2026-01-05T10:00:00Z,BUY,100,10.5\n").unwrap();
        assert_eq!(rows[0].trade.as_ref().unwrap().trade_id.as_str(), "T,1");
    }

    #[test]
    fn every_invalid_field_is_reported_as_a_typed_error() {
        use FieldError as F;
        use RowValidationError as R;

        let rows = parse(concat!(
            "T1,P999,I001,2026-01-05T10:00:00Z,BUY,10,1\n",
            "T2,P001,I999,2026-01-05T10:00:00Z,BUY,10,1\n",
            "T3,P001,I001,2026-01-07T10:00:00Z,BUY,10,1\n",
            "T4,P001,I001,2026-01-05 10:00:00,BUY,10,1\n",
            "T5,P001,I001,2026-01-05T10:00:00Z,HOLD,10,1\n",
            "T6,P001,I001,2026-01-05T10:00:00Z,BUY,0,1\n",
            "T7,P001,I001,2026-01-05T10:00:00Z,BUY,10,-1\n",
            "T8,P001,I001,2026-01-05T10:00:00Z,BUY,10,1.12345\n",
            ",P001,I001,2026-01-05T10:00:00Z,BUY,10,1\n",
            "T10,P001,I001\n",
            "T11,P999,I999,bad,SIDE,x,y\n",
        ))
        .unwrap();

        let rows_with_errors: BTreeSet<usize> = rows
            .iter()
            .filter(|r| !r.is_valid())
            .map(|r| r.row_number)
            .collect();
        assert_eq!(rows_with_errors, (2..=12).collect());

        let only = |i: usize| {
            assert_eq!(rows[i].errors.len(), 1, "row {}", rows[i].row_number);
            &rows[i].errors[0]
        };
        assert!(
            matches!(only(0), R::UnknownPortfolio { portfolio_id } if portfolio_id.as_str() == "P999")
        );
        assert!(
            matches!(only(1), R::UnknownInstrument { instrument_id } if instrument_id.as_str() == "I999")
        );
        assert!(matches!(
            only(2),
            R::NotAValuationDate { date, calendar: Some(_) } if date.to_string() == "2026-01-07"
        ));
        assert!(matches!(
            only(3),
            R::Field(F::InvalidTimestamp { source: None, .. })
        ));
        assert!(matches!(only(4), R::Field(F::InvalidSide { value }) if value == "HOLD"));
        assert!(
            matches!(only(5), R::Field(F::NotPositive { field: "quantity", value }) if value == "0")
        );
        assert!(
            matches!(only(6), R::Field(F::MalformedDecimal { field: "price", value }) if value == "-1")
        );
        assert!(matches!(
            only(7),
            R::Field(F::TooManyDecimalPlaces {
                field: "price",
                max: 4,
                ..
            })
        ));
        assert!(matches!(
            only(8),
            R::Field(F::Missing { field: "trade_id" })
        ));
        assert!(matches!(
            only(9),
            R::WrongFieldCount {
                expected: 7,
                actual: 3
            }
        ));
    }

    #[test]
    fn extra_fields_are_a_field_count_error() {
        let rows = parse("T12,P001,I001,2026-01-05T10:00:00Z,BUY,10,100,extra\n").unwrap();

        assert!(rows[0].trade.is_none());
        assert!(matches!(
            rows[0].errors[..],
            [RowValidationError::WrongFieldCount {
                expected: 7,
                actual: 8
            }]
        ));
    }

    #[test]
    fn several_problems_on_one_row_are_all_kept() {
        use FieldError as F;
        use RowValidationError as R;

        let rows = parse("T11,P999,I999,bad,SIDE,x,y\n").unwrap();
        let errors = &rows[0].errors;

        assert_eq!(errors.len(), 6);
        assert!(matches!(errors[0], R::Field(F::InvalidTimestamp { .. })));
        assert!(matches!(errors[1], R::Field(F::InvalidSide { .. })));
        assert!(matches!(
            errors[2],
            R::Field(F::MalformedInteger {
                field: "quantity",
                ..
            })
        ));
        assert!(matches!(
            errors[3],
            R::Field(F::MalformedDecimal { field: "price", .. })
        ));
        assert!(matches!(errors[4], R::UnknownPortfolio { .. }));
        assert!(matches!(errors[5], R::UnknownInstrument { .. }));
    }

    #[test]
    fn row_error_messages_name_the_problem_value() {
        let rows = parse(concat!(
            "T1,P999,I001,2026-01-05T10:00:00Z,BUY,10,1\n",
            "T3,P001,I001,2026-01-07T10:00:00Z,BUY,10,1\n",
            "T10,P001,I001\n",
        ))
        .unwrap();

        assert_eq!(
            rows[0].errors[0].to_string(),
            "portfolio_id 'P999' does not exist"
        );
        assert_eq!(
            rows[1].errors[0].to_string(),
            "trade date 2026-01-07 is not a valuation date in the dataset calendar \
             (2026-01-05 to 2026-01-06)"
        );
        assert_eq!(
            rows[2].errors[0].to_string(),
            "expected 7 fields but found 3"
        );
    }

    #[test]
    fn identical_repeats_in_one_file_are_not_conflicts() {
        let mut rows = parse(concat!(
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.0\n",
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10.0000\n",
        ))
        .unwrap();

        mark_in_file_conflicts(&mut rows);
        assert!(rows.iter().all(ParsedRow::is_valid));
    }

    #[test]
    fn conflicting_variants_in_one_file_mark_every_row() {
        let mut rows = parse(concat!(
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T2,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,11,10\n",
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
        ))
        .unwrap();

        mark_in_file_conflicts(&mut rows);
        let invalid: Vec<usize> = rows
            .iter()
            .filter(|r| !r.is_valid())
            .map(|r| r.row_number)
            .collect();
        assert_eq!(invalid, vec![2, 4, 5]);

        for i in [0, 2, 3] {
            match &rows[i].errors[..] {
                [RowValidationError::ConflictingTradeIdInFile { trade_id, rows }] => {
                    assert_eq!(trade_id, "T1");
                    assert_eq!(rows, &vec![2, 4, 5]);
                }
                other => panic!("unexpected errors: {other:?}"),
            }
        }

        assert_eq!(
            rows[0].errors[0].to_string(),
            "trade_id 'T1' appears in rows 2, 4, 5 with conflicting values"
        );
    }

    #[test]
    fn invalid_variant_of_an_id_conflicts_with_valid_variant() {
        let mut rows = parse(concat!(
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,0,10\n",
        ))
        .unwrap();

        mark_in_file_conflicts(&mut rows);
        assert!(rows.iter().all(|r| !r.is_valid()));
        // The invalid row keeps its own error as well as the conflict.
        assert_eq!(rows[1].errors.len(), 2);
    }

    #[test]
    fn groups_follow_first_appearance_and_keep_file_order() {
        let rows = parse(concat!(
            "T2,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T1,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            ",P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T2,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T3,P999,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
            "T1,P001,I001\n",
            "T2,P001,I001,2026-01-05T10:00:00Z,BUY,10,10\n",
        ))
        .unwrap();

        let groups = group_by_trade_id(&rows);

        // T2 first (rows 0, 3, 6), then T1 (rows 1, 5), then T3. The row
        // with no trade ID is in no group; invalid rows are grouped by the ID
        // as written.
        assert_eq!(groups, vec![vec![0, 3, 6], vec![1, 5], vec![4]]);
    }
}
