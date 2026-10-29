//! Supported currencies.

use serde::Serialize;
use strum::{Display, EnumString, IntoStaticStr};

use super::FieldError;

/// A currency the data contract allows, written as its ISO code. Parsing is
/// exact and case-sensitive: `"GBP"` is a currency, `"gbp"` is not, and any
/// other input is a `FieldError::UnsupportedCurrency`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Display,
    EnumString,
    IntoStaticStr,
)]
#[serde(rename_all = "UPPERCASE")]
#[strum(
    serialize_all = "UPPERCASE",
    parse_err_ty = FieldError,
    parse_err_fn = unsupported_currency
)]
pub enum Currency {
    Gbp,
    Usd,
    Eur,
}

impl Currency {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// The parse error `EnumString` returns, keeping the input exactly as given.
fn unsupported_currency(value: &str) -> FieldError {
    FieldError::UnsupportedCurrency {
        value: value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [(Currency, &str); 3] = [
        (Currency::Gbp, "GBP"),
        (Currency::Usd, "USD"),
        (Currency::Eur, "EUR"),
    ];

    #[test]
    fn currency_is_strict() {
        assert_eq!("USD".parse::<Currency>().unwrap(), Currency::Usd);
        assert!("usd".parse::<Currency>().is_err());
        assert!("JPY".parse::<Currency>().is_err());
    }

    #[test]
    fn every_currency_is_written_and_read_as_its_uppercase_code() {
        for (currency, code) in ALL {
            assert_eq!(code.parse::<Currency>().unwrap(), currency);
            assert_eq!(currency.as_str(), code);
            assert_eq!(currency.to_string(), code);
            assert_eq!(
                serde_json::to_string(&currency).unwrap(),
                format!("\"{code}\"")
            );
        }
    }

    #[test]
    fn anything_else_is_unsupported_and_keeps_the_input_as_given() {
        for raw in ["gbp", "Gbp", "uSD", "eur", "JPY", "", " GBP", "GBP "] {
            match raw.parse::<Currency>() {
                Err(FieldError::UnsupportedCurrency { value }) => assert_eq!(value, raw),
                other => panic!("{raw:?} gave {other:?}"),
            }
        }

        assert_eq!(
            "gbp".parse::<Currency>().unwrap_err().to_string(),
            "currency 'gbp' is not supported (expected GBP, USD or EUR)"
        );
    }
}
