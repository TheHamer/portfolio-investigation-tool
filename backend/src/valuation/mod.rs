//! Position valuation and P&L, following DATA_CONTRACT.md section 4.
//!
//! Everything here is pure: callers supply trades and market data, and these
//! functions do the arithmetic. Every operation is exact: it either produces
//! the mathematically exact `Decimal` or returns `ValuationError::Overflow`.
//! Nothing is rounded until a value is formatted for output.

mod error;

pub use error::ValuationError;

use std::collections::{BTreeMap, HashMap};

use chrono::NaiveDate;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::domain::{Currency, FxRate, InstrumentId, Price, Trade};

/// A dated observation from a price or FX series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation<T> {
    pub date: NaiveDate,
    pub value: T,
}

/// One instrument's daily prices or one currency's daily FX rates.
#[derive(Debug, Clone)]
pub struct Series<T> {
    points: BTreeMap<NaiveDate, T>,
}

// Not derived: the derive would needlessly require `T: Default`.
impl<T> Default for Series<T> {
    fn default() -> Self {
        Self {
            points: BTreeMap::new(),
        }
    }
}

impl<T: Copy> Series<T> {
    pub fn new(points: BTreeMap<NaiveDate, T>) -> Self {
        Self { points }
    }

    /// The latest observation dated on or before `date`. This is the only
    /// lookup the contract permits: it never looks forward, and a gap carries
    /// the previous value forward instead of substituting zero.
    pub fn latest_on_or_before(&self, date: NaiveDate) -> Option<Observation<T>> {
        self.points
            .range(..=date)
            .next_back()
            .map(|(&date, &value)| Observation { date, value })
    }
}

/// Price series per instrument and FX series per currency.
#[derive(Debug, Clone, Default)]
pub struct MarketData {
    pub prices: HashMap<InstrumentId, Series<Price>>,
    pub fx_rates: HashMap<Currency, Series<FxRate>>,
}

impl MarketData {
    /// Builds the per-instrument and per-currency series from flat dated
    /// observations, as returned by the persistence layer.
    pub fn from_observations(
        prices: impl IntoIterator<Item = (InstrumentId, NaiveDate, Price)>,
        fx_rates: impl IntoIterator<Item = (Currency, NaiveDate, FxRate)>,
    ) -> Self {
        let mut market = Self::default();

        for (instrument, date, price) in prices {
            market
                .prices
                .entry(instrument)
                .or_default()
                .points
                .insert(date, price);
        }

        for (currency, date, rate) in fx_rates {
            market
                .fx_rates
                .entry(currency)
                .or_default()
                .points
                .insert(date, rate);
        }

        market
    }

    pub fn price_on_or_before(
        &self,
        instrument: &InstrumentId,
        date: NaiveDate,
    ) -> Option<Observation<Price>> {
        self.prices.get(instrument)?.latest_on_or_before(date)
    }

    pub fn fx_on_or_before(
        &self,
        currency: Currency,
        date: NaiveDate,
    ) -> Option<Observation<FxRate>> {
        self.fx_rates.get(&currency)?.latest_on_or_before(date)
    }
}

/// One trade's contribution to a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeContribution {
    pub trade: Trade,
    /// +quantity for BUY, -quantity for SELL.
    pub signed_quantity: i64,
    /// Trade-day FX: latest rate on or before the trade's UTC date.
    pub fx: Observation<FxRate>,
    /// signed_quantity × price, in the instrument's currency.
    pub signed_local_amount: Decimal,
    /// signed_local_amount × trade-day FX.
    pub signed_cost_gbp: Decimal,
    /// Position quantity after this trade, in (timestamp, trade_id) order.
    pub running_quantity: i64,
}

/// A valued position. Every amount is exact and unrounded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionValuation {
    pub quantity: i64,
    pub net_trade_cost_gbp: Decimal,
    /// The latest price on or before the valuation date. Always `Some` for an
    /// open position. A closed position (quantity zero) is worth nothing
    /// whatever the price, so it does not need one: `None` means no price
    /// exists, and the position is still valued.
    pub valuation_price: Option<Observation<Price>>,
    /// The latest FX rate on or before the valuation date; optional on the
    /// same terms as `valuation_price`.
    pub valuation_fx: Option<Observation<FxRate>>,
    /// quantity × valuation price, in the instrument's currency.
    pub market_value_local: Decimal,
    pub market_value_gbp: Decimal,
    pub total_pnl_gbp: Decimal,
    pub contributions: Vec<TradeContribution>,
}

// --- Exact decimal arithmetic -------------------------------------------------
//
// `Decimal::checked_mul` / `checked_add` only fail when the integer part
// overflows. When a result needs more than 28 significant digits they keep it
// representable by silently rounding off fractional digits. For money that is
// a quiet approximation, so these helpers do the arithmetic on the i128
// mantissas and accept a result only if it fits exactly.

/// Largest mantissa a `Decimal` can hold: 2^96 - 1.
const MAX_MANTISSA: u128 = (1 << 96) - 1;
const MAX_SCALE: u32 = 28;

/// Builds `mantissa × 10^-scale` exactly, dropping trailing fractional zeros
/// (which never changes the value) only if needed to fit.
fn exact(mut mantissa: i128, mut scale: u32) -> Result<Decimal, ValuationError> {
    let fits = |m: i128, s: u32| m.unsigned_abs() <= MAX_MANTISSA && s <= MAX_SCALE;
    while !fits(mantissa, scale) && scale > 0 && mantissa % 10 == 0 {
        mantissa /= 10;
        scale -= 1;
    }

    if !fits(mantissa, scale) {
        return Err(ValuationError::Overflow);
    }
    Decimal::try_from_i128_with_scale(mantissa, scale).map_err(|_| ValuationError::Overflow)
}

fn mul(a: Decimal, b: Decimal) -> Result<Decimal, ValuationError> {
    let mantissa = a
        .mantissa()
        .checked_mul(b.mantissa())
        .ok_or(ValuationError::Overflow)?;
    exact(mantissa, a.scale() + b.scale())
}

fn add(a: Decimal, b: Decimal) -> Result<Decimal, ValuationError> {
    let scale = a.scale().max(b.scale());
    let aligned = |d: Decimal| {
        10i128
            .checked_pow(scale - d.scale())
            .and_then(|factor| d.mantissa().checked_mul(factor))
    };

    let mantissa = aligned(a)
        .zip(aligned(b))
        .and_then(|(x, y)| x.checked_add(y))
        .ok_or(ValuationError::Overflow)?;
    exact(mantissa, scale)
}

fn sub(a: Decimal, b: Decimal) -> Result<Decimal, ValuationError> {
    add(a, -b)
}

/// signed quantity × execution price, in the instrument's currency. Exposed
/// so callers explaining an unavailable valuation can still show it without
/// risking an unchecked multiplication.
pub fn signed_local_amount(trade: &Trade) -> Result<Decimal, ValuationError> {
    mul(Decimal::from(trade.signed_quantity()), trade.price.value())
}

/// Net position quantity: the sum of signed trade quantities. Needs no
/// market data, so it is known even when a valuation is not. The sum is
/// checked: overflowing `i64` is an error, never a wrapped value.
pub fn net_quantity(trades: &[Trade]) -> Result<i64, ValuationError> {
    trades.iter().try_fold(0i64, |total, t| {
        total
            .checked_add(t.signed_quantity())
            .ok_or(ValuationError::Overflow)
    })
}

/// Values one position, `(portfolio, instrument)`, as of `as_of`.
///
/// `trades` must all belong to the same position, be dated on or before
/// `as_of`, and be in `(timestamp, trade_id)` order. The portfolio loaders
/// guarantee all three (the as-of filter and the trade queries' `ORDER BY`),
/// so they are checked here only in debug builds. The order affects only the
/// running quantity; the totals are order-independent.
pub fn value_position(
    instrument_id: &InstrumentId,
    currency: Currency,
    trades: &[Trade],
    as_of: NaiveDate,
    market: &MarketData,
) -> Result<PositionValuation, ValuationError> {
    debug_assert!(
        trades.iter().all(|t| t.timestamp.utc_date() <= as_of),
        "trades must be filtered to on or before {as_of}"
    );
    debug_assert!(
        trades.is_sorted_by_key(|t| (t.timestamp, &t.trade_id)),
        "trades must be in (timestamp, trade_id) order"
    );

    let mut quantity: i64 = 0;
    let mut net_trade_cost_gbp = Decimal::ZERO;
    let mut contributions = Vec::with_capacity(trades.len());

    for trade in trades {
        let trade_date = trade.timestamp.utc_date();
        // Each trade is converted at its own trade-day rate, never today's.
        let fx = market
            .fx_on_or_before(currency, trade_date)
            .ok_or_else(|| ValuationError::MissingTradeFx {
                trade_id: trade.trade_id.clone(),
                currency,
                date: trade_date,
            })?;

        let signed_quantity = trade.signed_quantity();
        let signed_local_amount = signed_local_amount(trade)?;
        let signed_cost_gbp = mul(signed_local_amount, fx.value.value())?;

        quantity = quantity
            .checked_add(signed_quantity)
            .ok_or(ValuationError::Overflow)?;
        net_trade_cost_gbp = add(net_trade_cost_gbp, signed_cost_gbp)?;

        contributions.push(TradeContribution {
            trade: trade.clone(),
            signed_quantity,
            fx,
            signed_local_amount,
            signed_cost_gbp,
            running_quantity: quantity,
        });
    }

    // Price and FX are looked up independently: either may be carried forward
    // from an earlier date, and their dates may differ.
    let valuation_price = market.price_on_or_before(instrument_id, as_of);
    let valuation_fx = market.fx_on_or_before(currency, as_of);

    // A closed position's market value is zero without a price or rate, so
    // only an open position requires them. (Its trade costs above still
    // needed every trade-day FX rate.)
    let (market_value_local, market_value_gbp) = if quantity == 0 {
        (Decimal::ZERO, Decimal::ZERO)
    } else {
        let price = valuation_price.ok_or_else(|| ValuationError::MissingValuationPrice {
            instrument_id: instrument_id.clone(),
            as_of,
        })?;
        let fx = valuation_fx.ok_or(ValuationError::MissingValuationFx { currency, as_of })?;

        let local = mul(Decimal::from(quantity), price.value.value())?;
        (local, mul(local, fx.value.value())?)
    };

    let total_pnl_gbp = sub(market_value_gbp, net_trade_cost_gbp)?;

    Ok(PositionValuation {
        quantity,
        net_trade_cost_gbp,
        valuation_price,
        valuation_fx,
        market_value_local,
        market_value_gbp,
        total_pnl_gbp,
        contributions,
    })
}

/// Exact, unrounded sums over a portfolio's valued positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PortfolioTotals {
    pub market_value_gbp: Decimal,
    pub net_trade_cost_gbp: Decimal,
    pub total_pnl_gbp: Decimal,
}

/// Sums unrounded position values. Rounding happens only when the totals are
/// formatted, so the rounded total can differ from the sum of rounded
/// position figures by a few pennies, as the contract allows.
pub fn portfolio_totals<'a>(
    positions: impl IntoIterator<Item = &'a PositionValuation>,
) -> Result<PortfolioTotals, ValuationError> {
    positions
        .into_iter()
        .try_fold(PortfolioTotals::default(), |acc, p| {
            Ok(PortfolioTotals {
                market_value_gbp: add(acc.market_value_gbp, p.market_value_gbp)?,
                net_trade_cost_gbp: add(acc.net_trade_cost_gbp, p.net_trade_cost_gbp)?,
                total_pnl_gbp: add(acc.total_pnl_gbp, p.total_pnl_gbp)?,
            })
        })
}

/// GBP to two decimal places, rounding half up. For negative amounts, "half
/// up" is taken as half away from zero (the convention of Python's
/// ROUND_HALF_UP), so rounding is symmetric: -0.005 becomes -0.01.
pub fn round_money(value: Decimal) -> Decimal {
    let mut rounded = value.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    if rounded.is_zero() {
        rounded.set_sign_positive(true);
    }
    rounded
}

/// `round_money` formatted with exactly two decimal places ("1280.00").
/// Padding is done by the formatter rather than `Decimal::rescale(2)`, which
/// cannot add decimal places to values near the top of `Decimal`'s range.
pub fn format_money(value: Decimal) -> String {
    format!("{:.2}", round_money(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{PortfolioId, TradeId, TradeQuantity, TradeSide, TradeTimestamp};
    use rust_decimal::dec;

    fn date(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    fn trade(id: &str, ts: &str, side: TradeSide, qty: i64, price: &str) -> Trade {
        Trade {
            trade_id: TradeId::parse(id).unwrap(),
            portfolio_id: PortfolioId::parse("P1").unwrap(),
            instrument_id: InstrumentId::parse("I1").unwrap(),
            timestamp: TradeTimestamp::parse(ts).unwrap(),
            side,
            quantity: TradeQuantity::new(qty).unwrap(),
            price: Price::parse(price).unwrap(),
        }
    }

    fn series<T: Copy>(points: &[(&str, T)]) -> Series<T> {
        Series::new(points.iter().map(|(d, v)| (date(d), *v)).collect())
    }

    fn market(currency: Currency, prices: &[(&str, &str)], fx: &[(&str, &str)]) -> MarketData {
        let prices: Vec<(&str, Price)> = prices
            .iter()
            .map(|(d, p)| (*d, Price::parse(p).unwrap()))
            .collect();
        let fx: Vec<(&str, FxRate)> = fx
            .iter()
            .map(|(d, r)| (*d, FxRate::parse(r).unwrap()))
            .collect();
        MarketData {
            prices: HashMap::from([(InstrumentId::parse("I1").unwrap(), series(&prices))]),
            fx_rates: HashMap::from([(currency, series(&fx))]),
        }
    }

    fn i1() -> InstrumentId {
        InstrumentId::parse("I1").unwrap()
    }

    #[test]
    fn worked_example_gbp_trades() {
        let trades = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, 100, "10"),
            trade("T2", "2026-01-06T10:00:00Z", TradeSide::Sell, 40, "15"),
            trade("T3", "2026-01-07T10:00:00Z", TradeSide::Buy, 20, "12"),
        ];
        let market = market(
            Currency::Gbp,
            &[("2026-01-08", "16")],
            &[("2026-01-05", "1.000000")],
        );

        let v = value_position(&i1(), Currency::Gbp, &trades, date("2026-01-08"), &market).unwrap();

        assert_eq!(v.quantity, 80);
        assert_eq!(v.net_trade_cost_gbp, dec!(640));
        assert_eq!(v.market_value_gbp, dec!(1280));
        assert_eq!(v.total_pnl_gbp, dec!(640));
        let costs: Vec<Decimal> = v.contributions.iter().map(|c| c.signed_cost_gbp).collect();
        assert_eq!(costs, vec![dec!(1000), dec!(-600), dec!(240)]);
        let running: Vec<i64> = v.contributions.iter().map(|c| c.running_quantity).collect();
        assert_eq!(running, vec![100, 60, 80]);
    }

    #[test]
    fn worked_example_fx_uses_trade_day_rate_for_cost() {
        let trades = [trade(
            "T1",
            "2026-01-05T10:00:00Z",
            TradeSide::Buy,
            100,
            "10",
        )];
        let market = market(
            Currency::Usd,
            &[("2026-01-05", "10"), ("2026-01-09", "12")],
            &[("2026-01-05", "0.80"), ("2026-01-09", "0.75")],
        );

        let v = value_position(&i1(), Currency::Usd, &trades, date("2026-01-09"), &market).unwrap();

        assert_eq!(v.net_trade_cost_gbp, dec!(800));
        assert_eq!(v.market_value_gbp, dec!(900));
        assert_eq!(v.total_pnl_gbp, dec!(100));
        assert_eq!(v.contributions[0].fx.value.value(), dec!(0.80));
    }

    #[test]
    fn worked_example_closed_position() {
        let trades = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, 10, "10"),
            trade("T2", "2026-01-06T10:00:00Z", TradeSide::Sell, 10, "12"),
        ];
        let market = market(
            Currency::Gbp,
            &[("2026-01-06", "13")],
            &[("2026-01-05", "1")],
        );

        let v = value_position(&i1(), Currency::Gbp, &trades, date("2026-01-06"), &market).unwrap();

        assert_eq!(v.quantity, 0);
        assert_eq!(v.market_value_gbp, Decimal::ZERO);
        assert_eq!(v.total_pnl_gbp, dec!(20));
        // The available price is reported even though it does not matter.
        assert_eq!(v.valuation_price.unwrap().date, date("2026-01-06"));
    }

    #[test]
    fn as_of_lookup_carries_forward_and_never_looks_forward() {
        let s = series(&[("2026-01-05", 1), ("2026-01-07", 2), ("2026-01-09", 3)]);

        assert_eq!(s.latest_on_or_before(date("2026-01-04")), None);
        assert_eq!(s.latest_on_or_before(date("2026-01-05")).unwrap().value, 1);
        let gap = s.latest_on_or_before(date("2026-01-08")).unwrap();
        assert_eq!((gap.date, gap.value), (date("2026-01-07"), 2));
        assert_eq!(s.latest_on_or_before(date("2026-12-31")).unwrap().value, 3);
    }

    #[test]
    fn valuation_price_and_fx_dates_are_independent() {
        let trades = [trade(
            "T1",
            "2026-01-05T10:00:00Z",
            TradeSide::Buy,
            10,
            "100",
        )];
        // The price is stale (last on the 7th); FX has an observation on the 9th.
        let market = market(
            Currency::Eur,
            &[("2026-01-05", "100"), ("2026-01-07", "110")],
            &[
                ("2026-01-05", "0.8"),
                ("2026-01-09", "0.9"),
                ("2026-01-12", "0.95"),
            ],
        );

        let v = value_position(&i1(), Currency::Eur, &trades, date("2026-01-09"), &market).unwrap();

        assert_eq!(v.valuation_price.unwrap().date, date("2026-01-07"));
        assert_eq!(v.valuation_fx.unwrap().date, date("2026-01-09"));
        // 10 × 110 × 0.9, using no data after the valuation date.
        assert_eq!(v.market_value_gbp, dec!(990));
    }

    #[test]
    fn trade_fx_carries_forward_over_a_missing_day() {
        let trades = [trade(
            "T1",
            "2026-01-09T10:00:00Z",
            TradeSide::Buy,
            10,
            "10",
        )];
        let market = market(
            Currency::Eur,
            &[("2026-01-09", "10")],
            &[("2026-01-08", "0.8"), ("2026-01-12", "0.9")],
        );

        let v = value_position(&i1(), Currency::Eur, &trades, date("2026-01-09"), &market).unwrap();

        assert_eq!(v.contributions[0].fx.date, date("2026-01-08"));
        assert_eq!(v.net_trade_cost_gbp, dec!(80));
    }

    #[test]
    fn missing_coverage_is_an_error_not_zero() {
        let trades = [trade(
            "T1",
            "2026-01-05T10:00:00Z",
            TradeSide::Buy,
            10,
            "10",
        )];

        let no_price = market(
            Currency::Gbp,
            &[("2026-01-06", "10")],
            &[("2026-01-05", "1")],
        );
        assert!(matches!(
            value_position(&i1(), Currency::Gbp, &trades, date("2026-01-05"), &no_price),
            Err(ValuationError::MissingValuationPrice { .. })
        ));

        let no_trade_fx = market(
            Currency::Usd,
            &[("2026-01-05", "10")],
            &[("2026-01-06", "0.8")],
        );
        assert!(matches!(
            value_position(
                &i1(),
                Currency::Usd,
                &trades,
                date("2026-01-06"),
                &no_trade_fx
            ),
            Err(ValuationError::MissingTradeFx { .. })
        ));
    }

    #[test]
    fn open_position_without_a_valuation_price_is_unavailable() {
        let trades = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, 10, "10"),
            trade("T2", "2026-01-06T10:00:00Z", TradeSide::Sell, 4, "12"),
        ];
        // FX is complete; the instrument has no price at all.
        let no_price = market(Currency::Usd, &[], &[("2026-01-05", "0.8")]);

        assert_eq!(
            value_position(&i1(), Currency::Usd, &trades, date("2026-01-06"), &no_price),
            Err(ValuationError::MissingValuationPrice {
                instrument_id: i1(),
                as_of: date("2026-01-06"),
            })
        );
    }

    #[test]
    fn closed_position_needs_no_valuation_price() {
        let trades = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, 10, "10"),
            trade("T2", "2026-01-07T10:00:00Z", TradeSide::Sell, 10, "12"),
        ];
        // No price for the instrument at all. USD trade-day rates differ, and
        // the 2026-01-07 sale uses the rate carried forward from the 6th.
        let market = market(
            Currency::Usd,
            &[],
            &[("2026-01-05", "0.8"), ("2026-01-06", "0.75")],
        );

        let v = value_position(&i1(), Currency::Usd, &trades, date("2026-01-08"), &market).unwrap();

        assert_eq!(v.quantity, 0);
        assert_eq!(v.valuation_price, None);
        assert_eq!(v.market_value_local, Decimal::ZERO);
        assert_eq!(v.market_value_gbp, Decimal::ZERO);
        // Cost: 10 × 10 × 0.8 − 10 × 12 × 0.75 = 80 − 90.
        assert_eq!(v.net_trade_cost_gbp, dec!(-10));
        assert_eq!(v.total_pnl_gbp, dec!(10));
        let trade_fx: Vec<NaiveDate> = v.contributions.iter().map(|c| c.fx.date).collect();
        assert_eq!(trade_fx, vec![date("2026-01-05"), date("2026-01-06")]);
        // An observation that exists is still reported, though not needed.
        assert_eq!(v.valuation_fx.unwrap().date, date("2026-01-06"));
    }

    #[test]
    fn closed_position_still_needs_every_trade_day_fx_rate() {
        let trades = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, 10, "10"),
            trade("T2", "2026-01-07T10:00:00Z", TradeSide::Sell, 10, "12"),
        ];
        // Prices are complete, but the first USD rate is after the purchase.
        let market = market(
            Currency::Usd,
            &[("2026-01-05", "10"), ("2026-01-07", "12")],
            &[("2026-01-06", "0.75")],
        );

        assert_eq!(
            value_position(&i1(), Currency::Usd, &trades, date("2026-01-07"), &market),
            Err(ValuationError::MissingTradeFx {
                trade_id: TradeId::parse("T1").unwrap(),
                currency: Currency::Usd,
                date: date("2026-01-05"),
            })
        );
    }

    #[test]
    fn large_quantities_value_correctly() {
        // Above u32::MAX: quantities are i64.
        let qty = 5_000_000_000;
        let trades = [trade(
            "T1",
            "2026-01-05T10:00:00Z",
            TradeSide::Buy,
            qty,
            "2.5",
        )];
        let market = market(
            Currency::Gbp,
            &[("2026-01-05", "3")],
            &[("2026-01-05", "1")],
        );

        let v = value_position(&i1(), Currency::Gbp, &trades, date("2026-01-05"), &market).unwrap();

        assert_eq!(v.quantity, qty);
        assert_eq!(v.net_trade_cost_gbp, dec!(12_500_000_000));
        assert_eq!(v.total_pnl_gbp, dec!(2_500_000_000));
    }

    #[test]
    fn quantity_aggregation_overflow_is_an_error() {
        let trades = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, i64::MAX, "1"),
            trade("T2", "2026-01-05T11:00:00Z", TradeSide::Buy, 1, "1"),
        ];
        let as_of = date("2026-01-05");
        assert_eq!(net_quantity(&trades), Err(ValuationError::Overflow));

        let market = market(
            Currency::Gbp,
            &[("2026-01-05", "1")],
            &[("2026-01-05", "1")],
        );
        assert_eq!(
            value_position(&i1(), Currency::Gbp, &trades, as_of, &market),
            Err(ValuationError::Overflow)
        );

        // Large but balanced: i64::MAX bought then sold nets to zero safely.
        let balanced = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, i64::MAX, "1"),
            trade("T2", "2026-01-05T11:00:00Z", TradeSide::Sell, i64::MAX, "1"),
        ];
        assert_eq!(net_quantity(&balanced), Ok(0));
    }

    #[test]
    fn exact_arithmetic_refuses_to_round() {
        // A 29-digit mantissa times 1.1 needs 30 significant digits.
        let big = Decimal::from_i128_with_scale(79_228_162_514_264_337_593_543_950_335, 4);
        // rust_decimal's checked_mul "succeeds" by silently rounding...
        assert!(big.checked_mul(dec!(1.1)).is_some());
        // ...whereas the valuation arithmetic reports it.
        assert_eq!(mul(big, dec!(1.1)), Err(ValuationError::Overflow));
        assert_eq!(add(big, dec!(0.00001)), Err(ValuationError::Overflow));
        assert_eq!(mul(Decimal::MAX, dec!(2)), Err(ValuationError::Overflow));
        assert_eq!(sub(Decimal::MIN, dec!(1)), Err(ValuationError::Overflow));
    }

    #[test]
    fn exact_arithmetic_matches_true_results() {
        assert_eq!(mul(dec!(3), dec!(0.1)), Ok(dec!(0.3)));
        assert_eq!(add(dec!(0.1), dec!(0.2)), Ok(dec!(0.3)));
        assert_eq!(sub(dec!(1), dec!(1.0001)), Ok(dec!(-0.0001)));

        // i64::MAX shares at a 4 dp price and a 6 dp rate is still exact.
        let local = mul(Decimal::from(i64::MAX), dec!(1.2345)).unwrap();
        assert_eq!(local, dec!(11386252779497220733.7415));
        let gbp = mul(local, dec!(0.000001)).unwrap();
        assert_eq!(gbp, dec!(11386252779497.2207337415));

        // Trailing zeros are dropped only when needed, without changing value.
        // 1.000000000000000 squared has scale 30 (> 28): two zeros are dropped.
        let one_15dp = Decimal::from_i128_with_scale(1_000_000_000_000_000, 15);
        let squared = mul(one_15dp, one_15dp).unwrap();
        assert_eq!(squared, Decimal::ONE);
        assert_eq!(squared.scale(), 28);
    }

    #[test]
    fn oversized_values_make_a_position_unavailable_not_panic() {
        let trades = [trade(
            "T1",
            "2026-01-05T10:00:00Z",
            TradeSide::Buy,
            i64::MAX,
            "9999999999999999.9999",
        )];
        let market = market(
            Currency::Usd,
            &[("2026-01-05", "1")],
            &[("2026-01-05", "1")],
        );

        assert_eq!(
            value_position(&i1(), Currency::Usd, &trades, date("2026-01-05"), &market),
            Err(ValuationError::Overflow)
        );
        assert_eq!(
            signed_local_amount(&trades[0]),
            Err(ValuationError::Overflow)
        );
    }

    #[test]
    fn running_quantity_follows_the_trades_in_execution_order() {
        let trades = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, 10, "10"),
            trade("T2", "2026-01-05T10:00:00Z", TradeSide::Buy, 10, "10"),
            trade("T9", "2026-01-06T10:00:00Z", TradeSide::Sell, 5, "10"),
        ];
        let market = market(
            Currency::Gbp,
            &[("2026-01-05", "10")],
            &[("2026-01-05", "1")],
        );

        let v = value_position(&i1(), Currency::Gbp, &trades, date("2026-01-06"), &market).unwrap();

        let running: Vec<(&str, i64)> = v
            .contributions
            .iter()
            .map(|c| (c.trade.trade_id.as_str(), c.running_quantity))
            .collect();
        assert_eq!(running, vec![("T1", 10), ("T2", 20), ("T9", 15)]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "trades must be filtered")]
    fn trades_after_as_of_break_the_precondition() {
        let trades = [trade(
            "T1",
            "2026-01-06T10:00:00Z",
            TradeSide::Buy,
            10,
            "10",
        )];
        let market = market(Currency::Gbp, &[], &[]);
        let _ = value_position(&i1(), Currency::Gbp, &trades, date("2026-01-05"), &market);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "(timestamp, trade_id) order")]
    fn unordered_trades_break_the_precondition() {
        let trades = [
            trade("T2", "2026-01-05T10:00:00Z", TradeSide::Buy, 10, "10"),
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, 10, "10"),
        ];
        let market = market(Currency::Gbp, &[], &[]);
        let _ = value_position(&i1(), Currency::Gbp, &trades, date("2026-01-05"), &market);
    }

    #[test]
    fn totals_sum_unrounded_values_before_rounding() {
        let position = |mv: Decimal, pnl: Decimal| PositionValuation {
            quantity: 1,
            net_trade_cost_gbp: mv - pnl,
            valuation_price: Some(Observation {
                date: date("2026-01-05"),
                value: Price::parse("1").unwrap(),
            }),
            valuation_fx: Some(Observation {
                date: date("2026-01-05"),
                value: FxRate::parse("1").unwrap(),
            }),
            market_value_local: mv,
            market_value_gbp: mv,
            total_pnl_gbp: pnl,
            contributions: vec![],
        };

        // Each position rounds to 0.00, but together they are 0.012 -> 0.01.
        let positions = [
            position(dec!(0.004), dec!(0.004)),
            position(dec!(0.004), dec!(0.004)),
            position(dec!(0.004), dec!(0.004)),
        ];

        let totals = portfolio_totals(&positions).unwrap();
        assert_eq!(totals.market_value_gbp, dec!(0.012));
        assert_eq!(format_money(totals.market_value_gbp), "0.01");
        let sum_of_rounded: Decimal = positions
            .iter()
            .map(|p| round_money(p.market_value_gbp))
            .sum();
        assert_eq!(sum_of_rounded, Decimal::ZERO);
    }

    #[test]
    fn money_rounds_half_up_to_two_places() {
        assert_eq!(format_money(dec!(1280)), "1280.00");
        assert_eq!(format_money(dec!(2.345)), "2.35");
        assert_eq!(format_money(dec!(2.3449999)), "2.34");
        assert_eq!(format_money(dec!(-2.345)), "-2.35");
        assert_eq!(format_money(dec!(-0.001)), "0.00");
        assert_eq!(format_money(dec!(0.1)), "0.10");
        assert_eq!(round_money(dec!(2.345)), dec!(2.35));
        // rescale(2) cannot represent this with two decimals; formatting can.
        assert_eq!(
            format_money(Decimal::MAX),
            "79228162514264337593543950335.00"
        );
    }

    #[test]
    fn exact_decimal_arithmetic_has_no_float_error() {
        // 0.1 + 0.2 style cases are exact.
        let trades = [
            trade("T1", "2026-01-05T10:00:00Z", TradeSide::Buy, 3, "0.1"),
            trade("T2", "2026-01-05T11:00:00Z", TradeSide::Buy, 3, "0.2"),
        ];
        let market = market(
            Currency::Usd,
            &[("2026-01-05", "0.3")],
            &[("2026-01-05", "0.333333")],
        );

        let v = value_position(&i1(), Currency::Usd, &trades, date("2026-01-05"), &market).unwrap();

        assert_eq!(v.net_trade_cost_gbp, dec!(0.299999700));
        assert_eq!(v.total_pnl_gbp, dec!(0.299999700));
    }
}
