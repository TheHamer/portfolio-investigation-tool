//! Errors from retrieving and assembling portfolio data.

use std::fmt;

use thiserror::Error;

use crate::domain::{InstrumentId, PortfolioId};
use crate::persistence::PersistenceError;
use crate::valuation::ValuationError;

/// Failure to retrieve or assemble portfolio data. A position whose
/// valuation is unavailable (missing market data, numeric limits) is not an
/// error here: it is reported inside `PositionResult::valuation`.
#[derive(Debug, Error)]
pub enum PortfolioError {
    // The messages of the three "not found" variants are shown to API clients.
    #[error("Portfolio '{0}' does not exist.")]
    PortfolioNotFound(PortfolioId),
    #[error("Instrument '{0}' does not exist.")]
    InstrumentNotFound(InstrumentId),
    /// No trades in the instrument on or before the valuation date.
    #[error("Portfolio '{portfolio_id}' has no trades in instrument '{instrument_id}'.")]
    PositionNotFound {
        portfolio_id: PortfolioId,
        instrument_id: InstrumentId,
    },
    /// Stored trades reference an instrument missing from the instruments
    /// table; foreign keys should make this impossible.
    #[error("trade data references unknown instrument {0}")]
    UnknownInstrument(InstrumentId),
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
}

impl PortfolioError {
    /// True for the variants that mean "the requested resource does not exist".
    pub fn is_not_found(&self) -> bool {
        matches!(
            self,
            Self::PortfolioNotFound(_)
                | Self::InstrumentNotFound(_)
                | Self::PositionNotFound { .. }
        )
    }
}

/// Why a portfolio's totals cannot be shown. The portfolio itself still loads:
/// its positions (including unavailable ones) are returned alongside this.
#[derive(Debug, Clone, Error)]
pub enum PortfolioTotalsError {
    /// Summing valued positions failed (the exact numeric range was exceeded).
    #[error(transparent)]
    Calculation(#[from] ValuationError),
    /// A partial total would be misleading, so none is given.
    #[error(
        "Portfolio totals are unavailable because {} position(s) cannot be valued ({}).",
        positions.len(),
        positions.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
    )]
    UnavailablePositions { positions: Vec<UnavailablePosition> },
}

/// A position that could not be valued, and why.
#[derive(Debug, Clone)]
pub struct UnavailablePosition {
    pub instrument_id: InstrumentId,
    pub ticker: String,
    pub reason: ValuationError,
}

impl fmt::Display for UnavailablePosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.ticker, self.reason)
    }
}
