//! Loads stored trades and reference data and runs the valuation for a
//! portfolio or a single position. Calculations are done on demand from the
//! trades table, which is the single source of truth; nothing is cached.
//!
//! Both loaders value "as of" a date, and only trades executed on or before
//! that date (by UTC date) are considered: they decide the valuation, the
//! quantity, the trade count, the trades returned and whether a position
//! exists at all. A later trade is invisible, as if not yet imported.
//!
//! This layer owns "which data is needed"; SQL lives in `persistence` and the
//! arithmetic in `valuation`.

mod error;

pub use error::{PortfolioError, PortfolioTotalsError, UnavailablePosition};

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;
use sqlx::SqlitePool;

use crate::domain::{Instrument, InstrumentId, Portfolio, PortfolioId, Trade, TradeSide};
use crate::persistence::PersistenceError;
use crate::persistence::reference as reference_db;
use crate::persistence::trades as trades_db;
use crate::valuation::{self, MarketData, PortfolioTotals, PositionValuation, ValuationError};

/// One position as of the valuation date: its valuation, or why it cannot
/// be valued.
pub struct PositionResult {
    pub instrument: Instrument,
    /// Trades on or before the valuation date.
    pub trade_count: usize,
    /// Known even when valuation inputs are missing; `None` only if the
    /// signed quantities overflow `i64` (the valuation then reports why).
    pub quantity: Option<i64>,
    pub valuation: Result<PositionValuation, ValuationError>,
}

impl PositionResult {
    /// The trades net to exactly zero. An overflowing quantity (`None`) is not
    /// closed. Independent of whether the position can be valued.
    pub fn is_closed(&self) -> bool {
        self.quantity == Some(0)
    }
}

/// A portfolio with every position it holds, in instrument ID order.
pub struct PortfolioView {
    pub portfolio: Portfolio,
    pub positions: Vec<PositionResult>,
    /// `Err` holds a user-facing reason when any position cannot be valued:
    /// a partial total would be misleading.
    pub totals: Result<PortfolioTotals, PortfolioTotalsError>,
}

/// One position with the trades behind it.
pub struct PositionView {
    pub portfolio: Portfolio,
    pub position: PositionResult,
    /// The trades to display: those on or before the valuation date, in
    /// `(timestamp, trade_id)` order, narrowed to one side when requested.
    /// `position` is always valued from all trades regardless.
    pub trades: Vec<Trade>,
}

/// The single as-of boundary for both loaders: keeps only trades executed on
/// or before `as_of`, so everything derived from them agrees. The valuation
/// relies on it and does not filter again.
fn trades_up_to(mut trades: Vec<Trade>, as_of: NaiveDate) -> Vec<Trade> {
    trades.retain(|trade| trade.timestamp.utc_date() <= as_of);
    trades
}

/// Values one position from its trades, which are already as-of filtered and
/// in `(timestamp, trade_id)` order as the trade queries return them. Shared
/// by both loaders so summaries and details follow the same rules.
fn build_position(
    instrument: Instrument,
    trades: &[Trade],
    as_of: NaiveDate,
    market: &MarketData,
) -> PositionResult {
    let valuation = valuation::value_position(
        &instrument.instrument_id,
        instrument.currency,
        trades,
        as_of,
        market,
    );

    // The valuation already has the quantity; it is summed separately only
    // when the position could not be valued.
    let quantity = match &valuation {
        Ok(v) => Some(v.quantity),
        Err(_) => valuation::net_quantity(trades).ok(),
    };

    PositionResult {
        trade_count: trades.len(),
        quantity,
        instrument,
        valuation,
    }
}

async fn load_market_data(
    pool: &SqlitePool,
    instruments: &HashSet<InstrumentId>,
    as_of: NaiveDate,
) -> Result<MarketData, PersistenceError> {
    let prices = reference_db::prices_up_to(pool, instruments, as_of).await?;
    let fx_rates = reference_db::fx_rates_up_to(pool, as_of).await?;

    Ok(MarketData::from_observations(prices, fx_rates))
}

async fn find_portfolio(
    pool: &SqlitePool,
    portfolio_id: &PortfolioId,
) -> Result<Portfolio, PortfolioError> {
    reference_db::get_portfolio(pool, portfolio_id)
        .await?
        .ok_or_else(|| PortfolioError::PortfolioNotFound(portfolio_id.clone()))
}

/// Portfolio totals, or why they cannot be given. Any unavailable position
/// makes the totals unavailable rather than silently partial.
fn totals_for(positions: &[PositionResult]) -> Result<PortfolioTotals, PortfolioTotalsError> {
    let unavailable: Vec<UnavailablePosition> = positions
        .iter()
        .filter_map(|p| {
            p.valuation
                .as_ref()
                .err()
                .map(|reason| UnavailablePosition {
                    instrument_id: p.instrument.instrument_id.clone(),
                    ticker: p.instrument.ticker.clone(),
                    reason: reason.clone(),
                })
        })
        .collect();

    if !unavailable.is_empty() {
        return Err(PortfolioTotalsError::UnavailablePositions {
            positions: unavailable,
        });
    }

    Ok(valuation::portfolio_totals(
        positions.iter().filter_map(|p| p.valuation.as_ref().ok()),
    )?)
}

/// Every portfolio, in portfolio ID order, with its number of stored trades.
pub async fn list_portfolios(pool: &SqlitePool) -> Result<Vec<(Portfolio, i64)>, PortfolioError> {
    Ok(reference_db::list_portfolios(pool).await?)
}

/// Values every position in the portfolio as of `as_of`: one per instrument
/// with at least one trade on or before that date. Fails if the portfolio
/// does not exist or the stored data cannot be read or is inconsistent; a
/// position that cannot be valued is reported in its `PositionResult`, not as
/// an error.
pub async fn load_portfolio(
    pool: &SqlitePool,
    as_of: NaiveDate,
    portfolio_id: &PortfolioId,
) -> Result<PortfolioView, PortfolioError> {
    let portfolio = find_portfolio(pool, portfolio_id).await?;

    // Ordered by instrument, then (timestamp, trade_id), so each instrument's
    // trades stay in execution order when grouped.
    let trades = trades_db::portfolio_trades(pool, &portfolio.portfolio_id).await?;
    let mut trades_by_instrument: BTreeMap<InstrumentId, Vec<Trade>> = BTreeMap::new();
    for trade in trades_up_to(trades, as_of) {
        trades_by_instrument
            .entry(trade.instrument_id.clone())
            .or_default()
            .push(trade);
    }

    let traded_instruments: HashSet<InstrumentId> = trades_by_instrument.keys().cloned().collect();
    let market = load_market_data(pool, &traded_instruments, as_of).await?;
    let mut instruments: HashMap<InstrumentId, Instrument> = reference_db::instruments(pool)
        .await?
        .into_iter()
        .map(|i| (i.instrument_id.clone(), i))
        .collect();

    let mut positions = Vec::with_capacity(trades_by_instrument.len());
    for (instrument_id, trades) in trades_by_instrument {
        let instrument = instruments
            .remove(&instrument_id)
            .ok_or(PortfolioError::UnknownInstrument(instrument_id))?;

        positions.push(build_position(instrument, &trades, as_of, &market));
    }

    let totals = totals_for(&positions);

    Ok(PortfolioView {
        portfolio,
        positions,
        totals,
    })
}

/// Values one position as of `as_of` and returns it with its trades. A
/// missing portfolio or instrument, or a portfolio with no trades in the
/// instrument on or before `as_of`, is a not-found error, so a position found
/// here is exactly one that `load_portfolio` lists for the same date.
///
/// `side` narrows only the returned `trades` to BUY or SELL (`None` returns
/// all). The position itself is always valued from every trade: a running
/// average cost and quantity are meaningless on one side alone. A side with
/// no trades yields an empty list, not a not-found error.
pub async fn load_position(
    pool: &SqlitePool,
    as_of: NaiveDate,
    portfolio_id: &PortfolioId,
    instrument_id: &InstrumentId,
    side: Option<TradeSide>,
) -> Result<PositionView, PortfolioError> {
    let portfolio = find_portfolio(pool, portfolio_id).await?;
    let instrument = reference_db::get_instrument(pool, instrument_id)
        .await?
        .ok_or_else(|| PortfolioError::InstrumentNotFound(instrument_id.clone()))?;

    let trades =
        trades_db::position_trades(pool, &portfolio.portfolio_id, &instrument.instrument_id)
            .await?;
    let mut trades = trades_up_to(trades, as_of);
    if trades.is_empty() {
        return Err(PortfolioError::PositionNotFound {
            portfolio_id: portfolio_id.clone(),
            instrument_id: instrument_id.clone(),
        });
    }

    let traded_instruments = HashSet::from([instrument.instrument_id.clone()]);
    let market = load_market_data(pool, &traded_instruments, as_of).await?;
    let position = build_position(instrument, &trades, as_of, &market);

    // Filtered in memory after valuation: the trades are already loaded, and
    // a single read keeps the listed trades a subset of those valued.
    if let Some(side) = side {
        trades.retain(|trade| trade.side == side);
    }

    Ok(PositionView {
        portfolio,
        position,
        trades,
    })
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::*;
    use crate::domain::{Currency, FxRate, Price};
    use crate::valuation::Observation;

    fn date(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn instrument(id: &str, ticker: &str) -> Instrument {
        Instrument {
            instrument_id: InstrumentId::parse(id).unwrap(),
            ticker: ticker.into(),
            name: format!("{ticker} name"),
            currency: Currency::Usd,
            sector: "Tech".into(),
        }
    }

    fn valued(id: &str, market_value: Decimal) -> PositionResult {
        PositionResult {
            instrument: instrument(id, id),
            trade_count: 1,
            quantity: Some(1),
            valuation: Ok(PositionValuation {
                quantity: 1,
                net_trade_cost_gbp: Decimal::ZERO,
                valuation_price: Some(Observation {
                    date: date("2026-01-05"),
                    value: Price::parse("1").unwrap(),
                }),
                valuation_fx: Some(Observation {
                    date: date("2026-01-05"),
                    value: FxRate::parse("1").unwrap(),
                }),
                market_value_local: market_value,
                market_value_gbp: market_value,
                total_pnl_gbp: market_value,
                contributions: vec![],
            }),
        }
    }

    fn unvalued(id: &str, ticker: &str, reason: ValuationError) -> PositionResult {
        PositionResult {
            instrument: instrument(id, ticker),
            trade_count: 1,
            quantity: Some(10),
            valuation: Err(reason),
        }
    }

    #[test]
    fn totals_sum_valued_positions() {
        let totals = totals_for(&[valued("I1", Decimal::ONE), valued("I2", Decimal::TWO)]).unwrap();
        assert_eq!(totals.market_value_gbp, Decimal::from(3));
    }

    #[test]
    fn unavailable_positions_are_kept_with_their_reasons() {
        let missing_price = ValuationError::MissingValuationPrice {
            instrument_id: InstrumentId::parse("I2").unwrap(),
            as_of: date("2026-03-27"),
        };
        let missing_fx = ValuationError::MissingValuationFx {
            currency: Currency::Usd,
            as_of: date("2026-03-27"),
        };
        let positions = [
            valued("I1", Decimal::ONE),
            unvalued("I2", "ZZB", missing_price.clone()),
            unvalued("I3", "ZZC", missing_fx.clone()),
        ];

        match totals_for(&positions) {
            Err(PortfolioTotalsError::UnavailablePositions { positions }) => {
                let summary: Vec<(&str, &str, &ValuationError)> = positions
                    .iter()
                    .map(|p| (p.instrument_id.as_str(), p.ticker.as_str(), &p.reason))
                    .collect();
                assert_eq!(
                    summary,
                    vec![("I2", "ZZB", &missing_price), ("I3", "ZZC", &missing_fx)]
                );
            }
            other => panic!("unexpected result: {other:?}"),
        }

        // The explanation shown to users names each position and its reason.
        let error = totals_for(&positions).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Portfolio totals are unavailable because 2 position(s) cannot be valued \
             (ZZB: no price for I2 is available on or before 2026-03-27; \
             ZZC: no USD FX rate is available on or before 2026-03-27)."
        );
    }

    #[test]
    fn only_a_quantity_of_exactly_zero_is_closed() {
        let with_quantity = |quantity| PositionResult {
            quantity,
            ..valued("I1", Decimal::ONE)
        };

        assert!(with_quantity(Some(0)).is_closed());
        assert!(!with_quantity(Some(1)).is_closed());
        assert!(!with_quantity(Some(-1)).is_closed());
        // Overflowed: unknown, not closed.
        assert!(!with_quantity(None).is_closed());

        // Valuation status does not affect it.
        let missing_fx = ValuationError::MissingValuationFx {
            currency: Currency::Usd,
            as_of: date("2026-03-27"),
        };
        let unavailable = PositionResult {
            quantity: Some(0),
            ..unvalued("I2", "ZZB", missing_fx)
        };
        assert!(unavailable.is_closed());
    }

    #[test]
    fn overflowing_sum_is_a_calculation_error() {
        let positions = [valued("I1", Decimal::MAX), valued("I2", Decimal::MAX)];
        assert!(matches!(
            totals_for(&positions),
            Err(PortfolioTotalsError::Calculation(ValuationError::Overflow))
        ));
    }
}
