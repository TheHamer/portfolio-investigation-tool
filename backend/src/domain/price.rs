//! Positive decimal values: execution/close prices and FX rates.

use rust_decimal::Decimal;

use super::FieldError;

/// A positive local-currency price with at most four fractional digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Price(Decimal);

impl Price {
    pub const MAX_FRACTIONAL_DIGITS: usize = 4;

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        parse_positive_decimal(raw, "price", Self::MAX_FRACTIONAL_DIGITS).map(Self)
    }

    pub fn value(self) -> Decimal {
        self.0
    }
}

/// A positive GBP-per-unit exchange rate with at most six fractional digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxRate(Decimal);

impl FxRate {
    pub const MAX_FRACTIONAL_DIGITS: usize = 6;

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        parse_positive_decimal(raw, "gbp_per_unit", Self::MAX_FRACTIONAL_DIGITS).map(Self)
    }

    pub fn value(self) -> Decimal {
        self.0
    }
}

/// Parses an unsigned decimal literal: one or more digits, optionally
/// followed by `.` and one or more digits (`12`, `12.5`, `12.5000`).
///
/// Two separate concerns:
/// - **Precision** is a rule about the *input text*: at most `max_dp` digits
///   after the point, counting trailing zeros. `10.00000` has five fractional
///   digits and is rejected as a price even though its value is 10.
/// - **Equality** is numeric: once parsed, `10.0` and `10.0000` are the same
///   `Decimal`, which is what duplicate detection relies on.
///
/// The shape is checked by hand because `Decimal::from_str` is more permissive
/// than the contract (signs, `_` separators). That check also rejects `NaN`,
/// `inf`, exponents, `.5`, `5.` and `1..2`. `from_str_exact` then refuses any
/// value it could not represent without rounding.
fn parse_positive_decimal(
    raw: &str,
    field: &'static str,
    max_dp: usize,
) -> Result<Decimal, FieldError> {
    if raw.is_empty() {
        return Err(FieldError::Missing { field });
    }

    let (int_part, frac_part) = match raw.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (raw, None),
    };
    let digits_only = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits_only(int_part) || !frac_part.is_none_or(digits_only) {
        return Err(FieldError::MalformedDecimal {
            field,
            value: raw.to_owned(),
        });
    }

    if frac_part.is_some_and(|f| f.len() > max_dp) {
        return Err(FieldError::TooManyDecimalPlaces {
            field,
            value: raw.to_owned(),
            max: max_dp,
        });
    }

    let value = Decimal::from_str_exact(raw).map_err(|source| FieldError::DecimalOutOfRange {
        field,
        value: raw.to_owned(),
        source,
    })?;
    if value <= Decimal::ZERO {
        return Err(FieldError::NotPositive {
            field,
            value: raw.to_owned(),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    /// Fractional digits are counted on the text as written, trailing zeros
    /// included, so the same input can suit an FX rate but not a price.
    #[test]
    fn precision_is_checked_on_the_input_text() {
        let cases = [
            // input, valid as price (4 dp), valid as FX rate (6 dp)
            ("10", true, true),
            ("10.0", true, true),
            ("10.0000", true, true),
            ("10.00000", false, true),
            ("10.000000", false, true),
            ("10.0000000", false, false),
            ("-10", false, false),
            ("0", false, false),
            ("1..2", false, false),
            ("NaN", false, false),
        ];

        for (input, price_ok, fx_ok) in cases {
            assert_eq!(Price::parse(input).is_ok(), price_ok, "price {input:?}");
            assert_eq!(FxRate::parse(input).is_ok(), fx_ok, "fx {input:?}");
        }

        let err = Price::parse("10.00000").unwrap_err().to_string();
        assert_eq!(err, "price '10.00000' has more than 4 decimal places");
    }

    #[test]
    fn price_shape_and_positivity() {
        assert_eq!(Price::parse("84.4835").unwrap().value(), dec!(84.4835));
        assert_eq!(Price::parse("007.5").unwrap().value(), dec!(7.5));

        for bad in [
            "", "0.0000", "-20.7558", "+1", "84.48351", "1e3", "1E3", "inf", "1_000", ".5", "5.",
            "1.2.3", " 1", "1 ", "abc",
        ] {
            assert!(Price::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn values_beyond_decimal_range_are_rejected_not_rounded() {
        // 30 significant digits cannot be held exactly by a 96-bit Decimal.
        assert!(Price::parse("123456789012345678901234567890").is_err());
        assert!(Price::parse("9234567890123456789012345.1234").is_err());
    }

    #[test]
    fn fx_rate_allows_six_decimal_places() {
        assert_eq!(FxRate::parse("0.782484").unwrap().value(), dec!(0.782484));
        assert!(FxRate::parse("0.7824841").is_err());
        assert!(FxRate::parse("0").is_err());
    }

    #[test]
    fn numerically_equal_prices_compare_equal() {
        // Precision rules are about text; equality of parsed values is numeric.
        assert_eq!(
            Price::parse("10.0").unwrap(),
            Price::parse("10.0000").unwrap()
        );
        assert_eq!(Price::parse("10").unwrap(), Price::parse("10.00").unwrap());
        assert_ne!(
            Price::parse("10.0001").unwrap(),
            Price::parse("10").unwrap()
        );
    }
}
