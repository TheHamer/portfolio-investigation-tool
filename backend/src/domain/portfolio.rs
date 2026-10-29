//! The portfolio reference record.

use super::{Currency, PortfolioId};

/// A portfolio from the reference data.
#[derive(Debug, Clone)]
pub struct Portfolio {
    pub portfolio_id: PortfolioId,
    pub name: String,
    /// Always GBP: the reference loader and the database both reject any
    /// other base currency.
    pub base_currency: Currency,
}
