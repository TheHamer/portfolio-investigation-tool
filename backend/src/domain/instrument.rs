//! The instrument reference record.

use super::{Currency, InstrumentId};

/// An instrument from the reference data.
#[derive(Debug, Clone)]
pub struct Instrument {
    pub instrument_id: InstrumentId,
    pub ticker: String,
    pub name: String,
    pub currency: Currency,
    pub sector: String,
}
