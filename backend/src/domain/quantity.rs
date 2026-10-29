//! Trade quantities.

use super::FieldError;

const FIELD: &str = "quantity";

/// A strictly positive whole number of shares.
///
/// Stored as `i64` to match SQLite's INTEGER, so the only upper bound is
/// `i64::MAX`. A trade quantity is always positive; signed and aggregated
/// position quantities (which may be zero or negative) are plain `i64`s
/// computed with checked arithmetic in `valuation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TradeQuantity(i64);

impl TradeQuantity {
    pub fn new(value: i64) -> Result<Self, FieldError> {
        if value <= 0 {
            return Err(FieldError::NotPositive {
                field: FIELD,
                value: value.to_string(),
            });
        }
        Ok(Self(value))
    }

    /// Accepts plain digit strings only: no sign, decimal point, exponent or
    /// thousands separator.
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        if raw.is_empty() {
            return Err(FieldError::Missing { field: FIELD });
        }
        if !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(FieldError::MalformedInteger {
                field: FIELD,
                value: raw.to_owned(),
            });
        }

        // Digits only, so overflow is the only way this parse can fail.
        let value: i64 = raw.parse().map_err(|source| FieldError::IntegerTooLarge {
            field: FIELD,
            value: raw.to_owned(),
            max: i64::MAX,
            source,
        })?;

        // Report the input as written ("000"), not the parsed number.
        Self::new(value).map_err(|_| FieldError::NotPositive {
            field: FIELD,
            value: raw.to_owned(),
        })
    }

    pub fn get(self) -> i64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantity_must_be_positive_integer_text() {
        assert_eq!(TradeQuantity::parse("203").unwrap().get(), 203);
        assert_eq!(TradeQuantity::parse("007").unwrap().get(), 7);

        for bad in ["", "0", "000", "-5", "+5", "1.0", "1e3", "1,000", " 5"] {
            assert!(TradeQuantity::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn quantity_range_is_i64() {
        // Quantities are i64, so values above u32::MAX are accepted.
        let above_u32 = u64::from(u32::MAX) + 1;
        assert_eq!(
            TradeQuantity::parse(&above_u32.to_string()).unwrap().get(),
            4_294_967_296
        );
        assert_eq!(
            TradeQuantity::parse("9223372036854775807").unwrap().get(),
            i64::MAX
        );

        let err = TradeQuantity::parse("9223372036854775808").unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
        assert!(TradeQuantity::parse("99999999999999999999999").is_err());
    }

    #[test]
    fn constructor_enforces_positivity() {
        assert!(TradeQuantity::new(0).is_err());
        assert!(TradeQuantity::new(-1).is_err());
        assert!(TradeQuantity::new(i64::MIN).is_err());
        assert_eq!(TradeQuantity::new(i64::MAX).unwrap().get(), i64::MAX);
    }
}
