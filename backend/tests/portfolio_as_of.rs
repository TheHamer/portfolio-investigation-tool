//! The portfolio loaders "as of" a date: only trades on or before it count,
//! and a portfolio summary and its position details always agree.

mod common;

use chrono::NaiveDate;
use portfolio_investigation::AppState;
use portfolio_investigation::domain::{InstrumentId, PortfolioId};
use portfolio_investigation::import::{Outcome, import_trades};
use portfolio_investigation::persistence::reference as reference_db;
use portfolio_investigation::portfolio::{
    PortfolioError, PositionView, load_portfolio, load_position,
};
use rust_decimal::dec;
use tempfile::TempDir;

/// P001 / I002 (GBP): a buy and a sale by 2026-01-06, then a closing sale on
/// the final date. P001 / I001 (USD): a single buy on the final date.
const TRADES: &str = concat!(
    "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n",
    "T1,P001,I002,2026-01-05T10:00:00Z,BUY,100,10\n",
    "T2,P001,I002,2026-01-06T10:00:00Z,SELL,30,11\n",
    "T3,P001,I002,2026-03-27T10:00:00Z,SELL,70,12\n",
    "T4,P001,I001,2026-03-27T10:00:00Z,BUY,5,200\n",
);

fn date(s: &str) -> NaiveDate {
    s.parse().unwrap()
}

fn portfolio(id: &str) -> PortfolioId {
    PortfolioId::parse(id).unwrap()
}

fn instrument(id: &str) -> InstrumentId {
    InstrumentId::parse(id).unwrap()
}

async fn import(state: &AppState, csv: &[u8]) {
    let report = import_trades(&state.pool, state.manifest.valuation_dates(), csv)
        .await
        .unwrap();
    assert_eq!(report.outcome, Outcome::Success, "{:?}", report.errors);
}

async fn start_with_trades(dir: &TempDir) -> AppState {
    let state = common::start(dir).await;
    import(&state, TRADES.as_bytes()).await;
    state
}

/// The not-found error from `load_position` (`PositionView` has no `Debug`,
/// so `unwrap_err` is unavailable).
async fn position_error(
    state: &AppState,
    as_of: NaiveDate,
    portfolio_id: &str,
    instrument_id: &str,
) -> PortfolioError {
    match load_position(
        &state.pool,
        as_of,
        &portfolio(portfolio_id),
        &instrument(instrument_id),
        None,
    )
    .await
    {
        Ok(_) => panic!("{portfolio_id}/{instrument_id} unexpectedly exists as of {as_of}"),
        Err(error) => error,
    }
}

async fn position(
    state: &AppState,
    as_of: NaiveDate,
    portfolio_id: &str,
    instrument_id: &str,
) -> PositionView {
    match load_position(
        &state.pool,
        as_of,
        &portfolio(portfolio_id),
        &instrument(instrument_id),
        None,
    )
    .await
    {
        Ok(view) => view,
        Err(error) => panic!("{portfolio_id}/{instrument_id} as of {as_of}: {error}"),
    }
}

/// A USD position valued at an earlier date takes both its price and its FX
/// rate from that date, while its trade cost keeps the trade-day rate.
#[tokio::test]
async fn historical_valuation_uses_the_price_and_fx_of_the_requested_date() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    import(
        &state,
        concat!(
            "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n",
            "U1,P003,I001,2026-01-05T10:00:00Z,BUY,10,240\n",
        )
        .as_bytes(),
    )
    .await;

    let early = position(&state, date("2026-01-06"), "P003", "I001").await;
    let latest = position(&state, state.manifest.as_of(), "P003", "I001").await;

    // 10 × USD 239.5358 (2026-01-06 close) × 0.785223 (2026-01-06 rate).
    let early = early.position.valuation.unwrap();
    let price = early.valuation_price.unwrap();
    let fx = early.valuation_fx.unwrap();
    assert_eq!(
        (price.date, price.value.value()),
        (date("2026-01-06"), dec!(239.5358))
    );
    assert_eq!(
        (fx.date, fx.value.value()),
        (date("2026-01-06"), dec!(0.785223))
    );
    assert_eq!(early.market_value_gbp, dec!(1880.8901948340));
    assert_eq!(early.total_pnl_gbp, dec!(2.9285948340));

    // 10 × USD 240 at the 2026-01-05 rate of 0.782484, whatever the date.
    assert_eq!(early.contributions[0].fx.date, date("2026-01-05"));
    assert_eq!(early.net_trade_cost_gbp, dec!(1877.9616));

    let latest = latest.position.valuation.unwrap();
    assert_eq!(latest.valuation_price.unwrap().date, date("2026-03-27"));
    assert_eq!(latest.valuation_fx.unwrap().date, date("2026-03-27"));
    assert_eq!(latest.market_value_gbp, dec!(1883.6656356830));
    assert_eq!(latest.net_trade_cost_gbp, dec!(1877.9616));
}

/// Before its first trade a portfolio holds nothing: no positions and zero
/// totals, not an error, and none of its positions can be loaded yet.
#[tokio::test]
async fn portfolio_before_its_first_trade_has_no_positions_and_zero_totals() {
    let dir = TempDir::new().unwrap();
    let state = start_with_trades(&dir).await;
    let before_first_trade = date("2026-01-02");

    let view = load_portfolio(&state.pool, before_first_trade, &portfolio("P001"))
        .await
        .unwrap();
    let error = position_error(&state, before_first_trade, "P001", "I002").await;

    assert!(view.positions.is_empty());
    let totals = view.totals.unwrap();
    assert_eq!(totals.market_value_gbp, dec!(0));
    assert_eq!(totals.net_trade_cost_gbp, dec!(0));
    assert_eq!(totals.total_pnl_gbp, dec!(0));
    assert!(
        matches!(error, PortfolioError::PositionNotFound { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn portfolio_as_of_an_earlier_date_ignores_later_trades() {
    let dir = TempDir::new().unwrap();
    let state = start_with_trades(&dir).await;

    let view = load_portfolio(&state.pool, date("2026-01-06"), &portfolio("P001"))
        .await
        .unwrap();

    // I001's only trade is later, so it has no position yet.
    let instruments: Vec<&str> = view
        .positions
        .iter()
        .map(|p| p.instrument.instrument_id.as_str())
        .collect();
    assert_eq!(instruments, vec!["I002"]);

    // Only T1 and T2 count: 100 × 10 − 30 × 11 at GBP 1, and 70 shares at
    // the 2026-01-06 close of 81.3189.
    let i002 = &view.positions[0];
    let valuation = i002.valuation.as_ref().unwrap();
    assert_eq!(i002.trade_count, 2);
    assert_eq!(i002.quantity, Some(70));
    assert_eq!(valuation.contributions.len(), 2);
    assert_eq!(valuation.net_trade_cost_gbp, dec!(670));
    assert_eq!(valuation.market_value_gbp, dec!(5692.3230));
    assert_eq!(valuation.total_pnl_gbp, dec!(5022.3230));

    let totals = view.totals.unwrap();
    assert_eq!(totals.market_value_gbp, dec!(5692.3230));
    assert_eq!(totals.total_pnl_gbp, dec!(5022.3230));
}

#[tokio::test]
async fn position_as_of_an_earlier_date_returns_only_the_trades_it_counts() {
    let dir = TempDir::new().unwrap();
    let state = start_with_trades(&dir).await;
    let as_of = date("2026-01-06");

    let view = position(&state, as_of, "P001", "I002").await;

    let trade_ids: Vec<&str> = view.trades.iter().map(|t| t.trade_id.as_str()).collect();
    assert_eq!(trade_ids, vec!["T1", "T2"]);
    assert_eq!(view.position.trade_count, 2);
    assert_eq!(view.position.quantity, Some(70));
}

#[tokio::test]
async fn position_with_only_later_trades_does_not_exist_yet() {
    let dir = TempDir::new().unwrap();
    let state = start_with_trades(&dir).await;

    let error = position_error(&state, date("2026-01-06"), "P001", "I001").await;

    assert!(
        matches!(error, PortfolioError::PositionNotFound { .. }),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "Portfolio 'P001' has no trades in instrument 'I001'."
    );
}

#[tokio::test]
async fn final_date_includes_every_trade() {
    let dir = TempDir::new().unwrap();
    let state = start_with_trades(&dir).await;
    let as_of = state.manifest.as_of();

    let view = load_portfolio(&state.pool, as_of, &portfolio("P001"))
        .await
        .unwrap();

    let instruments: Vec<&str> = view
        .positions
        .iter()
        .map(|p| p.instrument.instrument_id.as_str())
        .collect();
    assert_eq!(instruments, vec!["I001", "I002"]);

    // I001: 5 × USD 200 at 0.783767, valued at USD 240.3349 and 0.783767.
    let i001 = view.positions[0].valuation.as_ref().unwrap();
    assert_eq!(view.positions[0].trade_count, 1);
    assert_eq!(i001.net_trade_cost_gbp, dec!(783.767));
    assert_eq!(i001.market_value_gbp, dec!(941.8328178415));

    // I002 is closed by T3: no market value, and a net trade cost of
    // 1000 − 330 − 840 = −170, so the P&L is 170.
    let i002 = view.positions[1].valuation.as_ref().unwrap();
    assert_eq!(view.positions[1].trade_count, 3);
    assert!(view.positions[1].is_closed());
    assert_eq!(i002.market_value_gbp, dec!(0));
    assert_eq!(i002.total_pnl_gbp, dec!(170));

    let detail = position(&state, as_of, "P001", "I002").await;
    assert_eq!(detail.trades.len(), 3);
}

/// For every instrument of every portfolio, at the final date and at an
/// earlier one: a position is listed in the summary exactly when its detail
/// can be loaded, and both report the same trades and figures.
#[tokio::test]
async fn summary_and_detail_agree_for_every_position() {
    let dir = TempDir::new().unwrap();
    let state = common::start(&dir).await;
    for file in [
        "trades_initial.csv",
        "trades_incremental.csv",
        "trades_corrected.csv",
    ] {
        import(&state, &common::fixture(file)).await;
    }

    let instruments = reference_db::instruments(&state.pool).await.unwrap();
    let mut positions_checked = 0;
    let mut absent_checked = 0;

    for as_of in [state.manifest.as_of(), date("2026-01-20")] {
        for portfolio_id in ["P001", "P002", "P003"] {
            let summary = load_portfolio(&state.pool, as_of, &portfolio(portfolio_id))
                .await
                .unwrap();

            for instrument in &instruments {
                let instrument_id = instrument.instrument_id.as_str();
                let listed = summary
                    .positions
                    .iter()
                    .find(|p| p.instrument.instrument_id.as_str() == instrument_id);

                let Some(listed) = listed else {
                    let error = position_error(&state, as_of, portfolio_id, instrument_id).await;
                    assert!(
                        matches!(error, PortfolioError::PositionNotFound { .. }),
                        "{error}"
                    );
                    absent_checked += 1;
                    continue;
                };

                let detail = position(&state, as_of, portfolio_id, instrument_id).await;
                let context = format!("{portfolio_id}/{instrument_id} as of {as_of}");

                assert_eq!(detail.position.trade_count, listed.trade_count, "{context}");
                assert_eq!(detail.trades.len(), listed.trade_count, "{context}");
                assert_eq!(detail.position.quantity, listed.quantity, "{context}");
                assert_eq!(detail.position.valuation, listed.valuation, "{context}");
                assert!(
                    detail
                        .trades
                        .iter()
                        .all(|t| t.timestamp.utc_date() <= as_of),
                    "{context}"
                );
                positions_checked += 1;
            }
        }
    }

    // Both branches were exercised: the earlier date has fewer positions.
    assert!(positions_checked > 72, "{positions_checked}");
    assert!(absent_checked > 0, "{absent_checked}");
}
