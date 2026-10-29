//! Case-sensitive string identifiers.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use super::FieldError;

// Each ID type wraps the string exactly as supplied; see `parse_identifier`.
// Deserialising goes through `parse`, so an ID taken from a request path is
// validated at the HTTP boundary like one read from a CSV.
macro_rules! string_id {
    ($name:ident, $field:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(raw: &str) -> Result<Self, FieldError> {
                parse_identifier(raw, $field).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                Self::parse(&raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

string_id!(TradeId, "trade_id");
string_id!(PortfolioId, "portfolio_id");
string_id!(InstrumentId, "instrument_id");

/// The contract's only rules for IDs: non-empty, compared case-sensitively.
/// The string is kept exactly as supplied, never trimmed, so `"T1"` and
/// `" T1"` are different IDs everywhere (duplicate detection, foreign keys,
/// storage), just as `"T1"` and `"t1"` are.
fn parse_identifier(raw: &str, field: &'static str) -> Result<String, FieldError> {
    if raw.is_empty() {
        return Err(FieldError::Missing { field });
    }
    Ok(raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_non_empty_and_exact() {
        assert!(TradeId::parse("").is_err());
        assert_eq!(TradeId::parse(" T1").unwrap().as_str(), " T1");
        assert_eq!(TradeId::parse("T1 ").unwrap().as_str(), "T1 ");
        assert_ne!(TradeId::parse("t1").unwrap(), TradeId::parse("T1").unwrap());
        assert_ne!(
            TradeId::parse(" T1").unwrap(),
            TradeId::parse("T1").unwrap()
        );
    }

    #[test]
    fn deserialising_applies_the_same_rules() {
        let id: PortfolioId = serde_json::from_str("\" P1\"").unwrap();
        assert_eq!(id.as_str(), " P1");

        let error = serde_json::from_str::<InstrumentId>("\"\"").unwrap_err();
        assert!(
            error.to_string().contains("instrument_id is required"),
            "{error}"
        );
    }
}
