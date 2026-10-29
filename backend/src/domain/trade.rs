//! Trades and their side.

use strum::{EnumString, IntoStaticStr};

use super::{FieldError, InstrumentId, PortfolioId, Price, TradeId, TradeQuantity, TradeTimestamp};

/// Trade direction, written `BUY` or `SELL`. Parsing is exact and
/// case-sensitive; any other input is a `FieldError::InvalidSide`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString, IntoStaticStr)]
#[strum(
    serialize_all = "UPPERCASE",
    parse_err_ty = FieldError,
    parse_err_fn = invalid_side
)]
pub enum TradeSide {
    Buy,
    Sell,
}

impl TradeSide {
    pub fn as_str(self) -> &'static str {
        self.into()
    }

    /// +1 for a BUY, -1 for a SELL: the `s` in the contract's formulas.
    pub fn sign(self) -> i64 {
        match self {
            TradeSide::Buy => 1,
            TradeSide::Sell => -1,
        }
    }
}

/// The parse error `EnumString` returns, keeping the input exactly as given.
fn invalid_side(value: &str) -> FieldError {
    FieldError::InvalidSide {
        value: value.to_owned(),
    }
}

/// One trade. Equality compares parsed values, which is exactly the
/// "all normalized field values match" test used to identify exact duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trade {
    pub trade_id: TradeId,
    pub portfolio_id: PortfolioId,
    pub instrument_id: InstrumentId,
    pub timestamp: TradeTimestamp,
    pub side: TradeSide,
    pub quantity: TradeQuantity,
    pub price: Price,
}

impl Trade {
    /// +quantity for a BUY, -quantity for a SELL. Cannot overflow: the
    /// quantity is in `1..=i64::MAX`, and `-i64::MAX` is representable.
    pub fn signed_quantity(&self) -> i64 {
        self.side.sign() * self.quantity.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_is_strict() {
        assert_eq!("SELL".parse::<TradeSide>().unwrap().sign(), -1);
        assert!("HOLD".parse::<TradeSide>().is_err());
        assert!("buy".parse::<TradeSide>().is_err());
    }

    #[test]
    fn each_side_is_read_and_written_as_its_uppercase_name() {
        for (side, text, sign) in [(TradeSide::Buy, "BUY", 1), (TradeSide::Sell, "SELL", -1)] {
            assert_eq!(text.parse::<TradeSide>().unwrap(), side);
            assert_eq!(side.as_str(), text);
            assert_eq!(side.sign(), sign);
        }
    }

    #[test]
    fn anything_else_is_an_invalid_side_and_keeps_the_input_as_given() {
        for raw in ["buy", "Sell", "HOLD", "", " BUY", "BUY "] {
            match raw.parse::<TradeSide>() {
                Err(FieldError::InvalidSide { value }) => assert_eq!(value, raw),
                other => panic!("{raw:?} gave {other:?}"),
            }
        }

        assert_eq!(
            "HOLD".parse::<TradeSide>().unwrap_err().to_string(),
            "side 'HOLD' is invalid (expected BUY or SELL)"
        );
    }
}
