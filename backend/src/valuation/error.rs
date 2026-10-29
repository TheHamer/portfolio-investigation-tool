//! Reasons a position cannot be valued.

use chrono::NaiveDate;
use thiserror::Error;

use crate::domain::{Currency, InstrumentId, TradeId};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValuationError {
    #[error("no {currency} FX rate is available on or before {date} for trade {trade_id}")]
    MissingTradeFx {
        trade_id: TradeId,
        currency: Currency,
        date: NaiveDate,
    },
    #[error("no price for {instrument_id} is available on or before {as_of}")]
    MissingValuationPrice {
        instrument_id: InstrumentId,
        as_of: NaiveDate,
    },
    #[error("no {currency} FX rate is available on or before {as_of}")]
    MissingValuationFx {
        currency: Currency,
        as_of: NaiveDate,
    },
    #[error("the calculation exceeds the supported exact numeric range")]
    Overflow,
}
