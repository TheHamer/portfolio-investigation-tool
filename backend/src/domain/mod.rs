//! Validated domain types.
//!
//! Each value type has a private field and a checked constructor, so a value
//! that exists has already passed the data contract's field-level rules.
//! Contextual rules (does this portfolio exist? is this date in the valuation
//! calendar?) cannot be expressed by syntax alone and are checked by the
//! importer. The reference records (`Portfolio`, `Instrument`, `Trade`) are
//! plain structs composed of those validated types.

mod currency;
mod error;
mod identifiers;
mod instrument;
mod portfolio;
mod price;
mod quantity;
mod timestamp;
mod trade;

pub use currency::Currency;
pub use error::FieldError;
pub use identifiers::{InstrumentId, PortfolioId, TradeId};
pub use instrument::Instrument;
pub use portfolio::Portfolio;
pub use price::{FxRate, Price};
pub use quantity::TradeQuantity;
pub use timestamp::{TradeTimestamp, parse_date};
pub use trade::{Trade, TradeSide};
