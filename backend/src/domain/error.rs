//! Field-level validation failures of the domain types.

use std::num::ParseIntError;

use thiserror::Error;

/// Why a raw value could not become a domain value. Each variant keeps the
/// field name and the offending input; the messages are shown to users (in
/// import reports) and name both.
#[derive(Debug, Clone, Error)]
pub enum FieldError {
    #[error("{field} is required")]
    Missing { field: &'static str },

    // --- Decimal values (prices, FX rates) -----------------------------------
    /// Not of the form `digits` or `digits.digits` (covers signs, exponents,
    /// `NaN`, `inf`, `.5`, `5.`, `1..2`, separators).
    #[error("{field} '{value}' is not a valid positive decimal number")]
    MalformedDecimal { field: &'static str, value: String },
    #[error("{field} '{value}' has more than {max} decimal places")]
    TooManyDecimalPlaces {
        field: &'static str,
        value: String,
        max: usize,
    },
    /// Well-formed but not exactly representable as a `Decimal`.
    #[error("{field} '{value}' is out of range")]
    DecimalOutOfRange {
        field: &'static str,
        value: String,
        #[source]
        source: rust_decimal::Error,
    },

    // --- Whole numbers (quantities) -------------------------------------------
    #[error("{field} '{value}' must be a positive whole number")]
    MalformedInteger { field: &'static str, value: String },
    #[error("{field} '{value}' is too large (maximum {max})")]
    IntegerTooLarge {
        field: &'static str,
        value: String,
        max: i64,
        #[source]
        source: ParseIntError,
    },

    /// A numeric value that is zero or negative where it must be positive.
    #[error("{field} '{value}' must be greater than zero")]
    NotPositive { field: &'static str, value: String },

    // --- Enumerations ----------------------------------------------------------
    #[error("currency '{value}' is not supported (expected GBP, USD or EUR)")]
    UnsupportedCurrency { value: String },
    #[error("side '{value}' is invalid (expected BUY or SELL)")]
    InvalidSide { value: String },

    // --- Dates and times ---------------------------------------------------------
    /// Wrong shape, or the right shape but not a real instant (`source` is
    /// then chrono's reason, e.g. 30 February).
    #[error("timestamp '{value}' must be a UTC time in the format YYYY-MM-DDTHH:MM:SSZ")]
    InvalidTimestamp {
        value: String,
        #[source]
        source: Option<chrono::ParseError>,
    },
    #[error("{field} '{value}' must be a date in the format YYYY-MM-DD")]
    InvalidDate {
        field: &'static str,
        value: String,
        #[source]
        source: Option<chrono::ParseError>,
    },
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::FieldError as F;
    use crate::domain::{
        Currency, FxRate, Price, TradeId, TradeQuantity, TradeSide, TradeTimestamp, parse_date,
    };

    #[test]
    fn each_invariant_has_its_own_variant() {
        assert!(matches!(
            TradeId::parse(""),
            Err(F::Missing { field: "trade_id" })
        ));
        assert!(matches!(
            Price::parse(""),
            Err(F::Missing { field: "price" })
        ));

        assert!(matches!(
            Price::parse("1e3"),
            Err(F::MalformedDecimal { field: "price", value }) if value == "1e3"
        ));
        assert!(matches!(
            Price::parse("1.12345"),
            Err(F::TooManyDecimalPlaces {
                field: "price",
                max: 4,
                ..
            })
        ));
        assert!(matches!(
            FxRate::parse("0.1234567"),
            Err(F::TooManyDecimalPlaces {
                field: "gbp_per_unit",
                max: 6,
                ..
            })
        ));
        assert!(matches!(
            Price::parse("0.0"),
            Err(F::NotPositive { field: "price", .. })
        ));
        let out_of_range = Price::parse("123456789012345678901234567890").unwrap_err();
        assert!(matches!(out_of_range, F::DecimalOutOfRange { .. }));
        assert!(
            out_of_range.source().is_some(),
            "keeps rust_decimal's error"
        );

        assert!(matches!(
            TradeQuantity::parse("1.0"),
            Err(F::MalformedInteger {
                field: "quantity",
                ..
            })
        ));
        let too_large = TradeQuantity::parse("9223372036854775808").unwrap_err();
        assert!(matches!(
            too_large,
            F::IntegerTooLarge { max: i64::MAX, .. }
        ));
        assert!(too_large.source().is_some(), "keeps the ParseIntError");
        assert!(matches!(
            TradeQuantity::new(-3),
            Err(F::NotPositive { field: "quantity", value }) if value == "-3"
        ));

        assert!(matches!(
            "JPY".parse::<Currency>(),
            Err(F::UnsupportedCurrency { value }) if value == "JPY"
        ));
        assert!(matches!(
            "HOLD".parse::<TradeSide>(),
            Err(F::InvalidSide { value }) if value == "HOLD"
        ));
    }

    #[test]
    fn date_errors_distinguish_shape_from_calendar() {
        // Wrong shape: no chrono error to keep.
        assert!(matches!(
            TradeTimestamp::parse("2026-03-13 15:10:35"),
            Err(F::InvalidTimestamp { source: None, .. })
        ));
        // Right shape, impossible date: chrono's reason is kept.
        assert!(matches!(
            TradeTimestamp::parse("2026-02-30T10:00:00Z"),
            Err(F::InvalidTimestamp {
                source: Some(_),
                ..
            })
        ));
        assert!(matches!(
            parse_date("27/03/2026", "as_of"),
            Err(F::InvalidDate {
                field: "as_of",
                source: None,
                ..
            })
        ));
        assert!(matches!(
            parse_date("2026-02-30", "date"),
            Err(F::InvalidDate {
                source: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn user_facing_messages_name_the_field_and_the_value() {
        let message = |e: F| e.to_string();
        assert_eq!(
            message(TradeQuantity::parse("0").unwrap_err()),
            "quantity '0' must be greater than zero"
        );
        assert_eq!(
            message(Price::parse("-20.7558").unwrap_err()),
            "price '-20.7558' is not a valid positive decimal number"
        );
        assert_eq!(
            message(Price::parse("10.00000").unwrap_err()),
            "price '10.00000' has more than 4 decimal places"
        );
        assert_eq!(
            message("HOLD".parse::<TradeSide>().unwrap_err()),
            "side 'HOLD' is invalid (expected BUY or SELL)"
        );
        assert_eq!(
            message(TradeTimestamp::parse("2026-03-13 15:10:35").unwrap_err()),
            "timestamp '2026-03-13 15:10:35' must be a UTC time in the format YYYY-MM-DDTHH:MM:SSZ"
        );
        assert_eq!(
            message(TradeQuantity::parse("9223372036854775808").unwrap_err()),
            "quantity '9223372036854775808' is too large (maximum 9223372036854775807)"
        );
    }
}
