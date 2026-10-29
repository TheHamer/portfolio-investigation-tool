//! The portfolio list, positions and position-detail responses.

use chrono::NaiveDate;
use serde::Serialize;

use super::{capitalise, exact};
use crate::domain::{Currency, FxRate, InstrumentId, Portfolio, PortfolioId, Price, Trade};
use crate::portfolio::PositionResult;
use crate::valuation::{
    self, Observation, PortfolioTotals, PositionValuation, TradeContribution, format_money,
};

/// The `portfolio` object nested in the positions and position-detail
/// responses.
#[derive(Serialize)]
pub(in crate::api) struct PortfolioInfo {
    portfolio_id: PortfolioId,
    name: String,
    base_currency: Currency,
}

impl From<Portfolio> for PortfolioInfo {
    fn from(portfolio: Portfolio) -> Self {
        Self {
            portfolio_id: portfolio.portfolio_id,
            name: portfolio.name,
            base_currency: portfolio.base_currency,
        }
    }
}

/// One entry of the portfolio list: the portfolio's fields and its trade
/// count, side by side.
#[derive(Serialize)]
pub(in crate::api) struct PortfolioListItem {
    portfolio_id: PortfolioId,
    name: String,
    base_currency: Currency,
    trade_count: i64,
}

impl PortfolioListItem {
    pub(in crate::api) fn new(portfolio: Portfolio, trade_count: i64) -> Self {
        Self {
            portfolio_id: portfolio.portfolio_id,
            name: portfolio.name,
            base_currency: portfolio.base_currency,
            trade_count,
        }
    }
}

#[derive(Serialize)]
pub(in crate::api) struct PortfolioListResponse {
    pub(in crate::api) as_of: NaiveDate,
    pub(in crate::api) dataset_id: String,
    pub(in crate::api) portfolios: Vec<PortfolioListItem>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(in crate::api) enum Status {
    Ok,
    Unavailable,
}

#[derive(Serialize)]
struct PriceUsed {
    price: String,
    currency: Currency,
    date: NaiveDate,
    /// True when no price exists on the valuation date and an earlier one is used.
    carried_forward: bool,
}

impl PriceUsed {
    fn new(obs: Observation<Price>, currency: Currency, as_of: NaiveDate) -> Self {
        Self {
            price: obs.value.value().to_string(),
            currency,
            date: obs.date,
            carried_forward: obs.date < as_of,
        }
    }
}

#[derive(Serialize)]
struct FxUsed {
    /// GBP per one unit of `currency`.
    gbp_per_unit: String,
    currency: Currency,
    date: NaiveDate,
    carried_forward: bool,
}

impl FxUsed {
    fn new(obs: Observation<FxRate>, currency: Currency, wanted: NaiveDate) -> Self {
        Self {
            gbp_per_unit: obs.value.value().to_string(),
            currency,
            date: obs.date,
            carried_forward: obs.date < wanted,
        }
    }
}

/// One position: the instrument's fields followed by the position's figures,
/// all at the same level.
#[derive(Serialize)]
pub(in crate::api) struct PositionSummary {
    instrument_id: InstrumentId,
    ticker: String,
    name: String,
    currency: Currency,
    sector: String,
    trade_count: usize,
    /// `None` only if summing the signed trade quantities overflows `i64`.
    quantity: Option<i64>,
    is_closed: bool,
    status: Status,
    unavailable_reason: Option<String>,
    net_trade_cost_gbp: Option<String>,
    market_value_gbp: Option<String>,
    total_pnl_gbp: Option<String>,
    /// `None` when the position is unavailable, or closed with no price on or
    /// before the valuation date (a closed position does not need one).
    valuation_price: Option<PriceUsed>,
    /// `None` on the same terms as `valuation_price`.
    valuation_fx: Option<FxUsed>,
}

impl PositionSummary {
    pub(in crate::api) fn new(position: &PositionResult, as_of: NaiveDate) -> Self {
        let instrument = &position.instrument;
        let currency = instrument.currency;
        let ok = position.valuation.as_ref().ok();

        Self {
            instrument_id: instrument.instrument_id.clone(),
            ticker: instrument.ticker.clone(),
            name: instrument.name.clone(),
            currency,
            sector: instrument.sector.clone(),
            trade_count: position.trade_count,
            quantity: position.quantity,
            is_closed: position.is_closed(),
            status: if ok.is_some() {
                Status::Ok
            } else {
                Status::Unavailable
            },
            unavailable_reason: position
                .valuation
                .as_ref()
                .err()
                .map(|e| capitalise(&e.to_string())),
            net_trade_cost_gbp: ok.map(|v| format_money(v.net_trade_cost_gbp)),
            market_value_gbp: ok.map(|v| format_money(v.market_value_gbp)),
            total_pnl_gbp: ok.map(|v| format_money(v.total_pnl_gbp)),
            valuation_price: ok
                .and_then(|v| v.valuation_price)
                .map(|price| PriceUsed::new(price, currency, as_of)),
            valuation_fx: ok
                .and_then(|v| v.valuation_fx)
                .map(|fx| FxUsed::new(fx, currency, as_of)),
        }
    }
}

#[derive(Serialize)]
pub(in crate::api) struct Totals {
    market_value_gbp: String,
    net_trade_cost_gbp: String,
    total_pnl_gbp: String,
    market_value_gbp_exact: String,
    net_trade_cost_gbp_exact: String,
    total_pnl_gbp_exact: String,
}

impl Totals {
    pub(in crate::api) fn new(t: &PortfolioTotals) -> Self {
        Self {
            market_value_gbp: format_money(t.market_value_gbp),
            net_trade_cost_gbp: format_money(t.net_trade_cost_gbp),
            total_pnl_gbp: format_money(t.total_pnl_gbp),
            market_value_gbp_exact: exact(t.market_value_gbp),
            net_trade_cost_gbp_exact: exact(t.net_trade_cost_gbp),
            total_pnl_gbp_exact: exact(t.total_pnl_gbp),
        }
    }
}

#[derive(Serialize)]
pub(in crate::api) struct PortfolioPositionsResponse {
    pub(in crate::api) portfolio: PortfolioInfo,
    pub(in crate::api) as_of: NaiveDate,
    pub(in crate::api) status: Status,
    pub(in crate::api) unavailable_reason: Option<String>,
    pub(in crate::api) totals: Option<Totals>,
    pub(in crate::api) open_position_count: usize,
    pub(in crate::api) closed_position_count: usize,
    pub(in crate::api) positions: Vec<PositionSummary>,
}

/// One underlying trade. Calculated fields are `None` when the position
/// cannot be valued (for example, missing FX coverage); `signed_local_amount`
/// is also `None` if quantity × price itself exceeds the exact numeric range.
#[derive(Serialize)]
pub(in crate::api) struct TradeLine {
    trade_id: String,
    timestamp: String,
    trade_date: NaiveDate,
    side: &'static str,
    quantity: i64,
    signed_quantity: i64,
    price: String,
    currency: Currency,
    signed_local_amount: Option<String>,
    fx: Option<FxUsed>,
    signed_cost_gbp: Option<String>,
    signed_cost_gbp_exact: Option<String>,
    running_quantity: Option<i64>,
}

impl TradeLine {
    /// The stored trade fields, with no calculated values.
    fn record(trade: &Trade, currency: Currency) -> Self {
        Self {
            trade_id: trade.trade_id.to_string(),
            timestamp: trade.timestamp.to_canonical(),
            trade_date: trade.timestamp.utc_date(),
            side: trade.side.as_str(),
            quantity: trade.quantity.get(),
            signed_quantity: trade.signed_quantity(),
            price: trade.price.value().to_string(),
            currency,
            signed_local_amount: None,
            fx: None,
            signed_cost_gbp: None,
            signed_cost_gbp_exact: None,
            running_quantity: None,
        }
    }

    /// Used when the position could not be valued. This path explains an
    /// earlier error, so it must not fail itself: the local amount uses the
    /// same exact, checked multiplication as the valuation and is omitted if
    /// it does not fit.
    pub(in crate::api) fn unvalued(trade: &Trade, currency: Currency) -> Self {
        Self {
            signed_local_amount: valuation::signed_local_amount(trade).ok().map(exact),
            ..Self::record(trade, currency)
        }
    }

    /// Reuses the figures the valuation already calculated.
    pub(in crate::api) fn calculated(c: &TradeContribution, currency: Currency) -> Self {
        Self {
            signed_local_amount: Some(exact(c.signed_local_amount)),
            fx: Some(FxUsed::new(c.fx, currency, c.trade.timestamp.utc_date())),
            signed_cost_gbp: Some(format_money(c.signed_cost_gbp)),
            signed_cost_gbp_exact: Some(exact(c.signed_cost_gbp)),
            running_quantity: Some(c.running_quantity),
            ..Self::record(&c.trade, currency)
        }
    }
}

/// Every intermediate figure needed to reproduce the position's numbers.
#[derive(Serialize)]
pub(in crate::api) struct Breakdown {
    quantity: i64,
    /// `None` only for a closed position with no price or rate available:
    /// its market value is zero without them.
    valuation_price: Option<PriceUsed>,
    valuation_fx: Option<FxUsed>,
    market_value_local_exact: String,
    market_value_gbp_exact: String,
    net_trade_cost_gbp_exact: String,
    total_pnl_gbp_exact: String,
}

impl Breakdown {
    pub(in crate::api) fn new(v: &PositionValuation, currency: Currency, as_of: NaiveDate) -> Self {
        Self {
            quantity: v.quantity,
            valuation_price: v
                .valuation_price
                .map(|price| PriceUsed::new(price, currency, as_of)),
            valuation_fx: v.valuation_fx.map(|fx| FxUsed::new(fx, currency, as_of)),
            market_value_local_exact: exact(v.market_value_local),
            market_value_gbp_exact: exact(v.market_value_gbp),
            net_trade_cost_gbp_exact: exact(v.net_trade_cost_gbp),
            total_pnl_gbp_exact: exact(v.total_pnl_gbp),
        }
    }
}

#[derive(Serialize)]
pub(in crate::api) struct PositionDetailResponse {
    pub(in crate::api) portfolio: PortfolioInfo,
    pub(in crate::api) as_of: NaiveDate,
    pub(in crate::api) position: PositionSummary,
    pub(in crate::api) breakdown: Option<Breakdown>,
    /// In `(timestamp, trade_id)` order.
    pub(in crate::api) trades: Vec<TradeLine>,
}

#[cfg(test)]
mod tests {
    //! The exact JSON text of the responses built from domain types: their
    //! fields at one level, in a fixed order, with `null` for absent values.

    use rust_decimal::Decimal;

    use super::*;
    use crate::api::dto::json;
    use crate::domain::Instrument;
    use crate::valuation::ValuationError;

    fn portfolio() -> Portfolio {
        Portfolio {
            portfolio_id: PortfolioId::parse("P001").unwrap(),
            name: "Growth".into(),
            base_currency: Currency::Gbp,
        }
    }

    #[test]
    fn portfolio_list_item_keeps_portfolio_fields_at_the_top_level() {
        assert_eq!(
            json(&PortfolioListItem::new(portfolio(), 229)),
            r#"{"portfolio_id":"P001","name":"Growth","base_currency":"GBP","trade_count":229}"#
        );
    }

    #[test]
    fn nested_portfolio_object_has_only_the_portfolio_fields() {
        assert_eq!(
            json(&PortfolioInfo::from(portfolio())),
            r#"{"portfolio_id":"P001","name":"Growth","base_currency":"GBP"}"#
        );
    }

    #[test]
    fn position_summary_keeps_instrument_fields_at_the_top_level() {
        let instrument_id = InstrumentId::parse("I001").unwrap();
        let as_of = NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();
        let position = PositionResult {
            instrument: Instrument {
                instrument_id: instrument_id.clone(),
                ticker: "OLV".into(),
                name: "Olvanta".into(),
                currency: Currency::Usd,
                sector: "Tech".into(),
            },
            trade_count: 2,
            quantity: Some(10),
            valuation: Err(ValuationError::MissingValuationPrice {
                instrument_id,
                as_of,
            }),
        };

        assert_eq!(
            json(&PositionSummary::new(&position, as_of)),
            concat!(
                r#"{"instrument_id":"I001","ticker":"OLV","name":"Olvanta","currency":"USD","sector":"Tech","#,
                r#""trade_count":2,"quantity":10,"is_closed":false,"status":"unavailable","#,
                r#""unavailable_reason":"No price for I001 is available on or before 2026-03-27","#,
                r#""net_trade_cost_gbp":null,"market_value_gbp":null,"total_pnl_gbp":null,"#,
                r#""valuation_price":null,"valuation_fx":null}"#
            )
        );
    }

    #[test]
    fn closed_position_without_a_price_is_valued_with_null_observations() {
        let as_of = NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();
        let position = PositionResult {
            instrument: Instrument {
                instrument_id: InstrumentId::parse("I014").unwrap(),
                ticker: "CAR".into(),
                name: "Carvel".into(),
                currency: Currency::Gbp,
                sector: "Industrials".into(),
            },
            trade_count: 2,
            quantity: Some(0),
            valuation: Ok(PositionValuation {
                quantity: 0,
                net_trade_cost_gbp: Decimal::from(-20),
                valuation_price: None,
                valuation_fx: None,
                market_value_local: Decimal::ZERO,
                market_value_gbp: Decimal::ZERO,
                total_pnl_gbp: Decimal::from(20),
                contributions: vec![],
            }),
        };

        assert_eq!(
            json(&PositionSummary::new(&position, as_of)),
            concat!(
                r#"{"instrument_id":"I014","ticker":"CAR","name":"Carvel","currency":"GBP","sector":"Industrials","#,
                r#""trade_count":2,"quantity":0,"is_closed":true,"status":"ok","unavailable_reason":null,"#,
                r#""net_trade_cost_gbp":"-20.00","market_value_gbp":"0.00","total_pnl_gbp":"20.00","#,
                r#""valuation_price":null,"valuation_fx":null}"#
            )
        );
    }
}
