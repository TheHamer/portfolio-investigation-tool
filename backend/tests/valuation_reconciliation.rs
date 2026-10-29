//! After loading all valid fixtures, checks that every displayed figure
//! reconciles to the underlying records and reference data.

mod common;

use common::{fixture, start};
use portfolio_investigation::domain::PortfolioId;
use portfolio_investigation::import::import_trades;
use portfolio_investigation::portfolio::load_portfolio;
use portfolio_investigation::valuation::format_money;
use rust_decimal::{Decimal, dec};
use tempfile::TempDir;

/// Imports the three valid fixtures, as the contract's upload order leaves
/// them stored.
async fn start_with_valid_fixtures(dir: &TempDir) -> portfolio_investigation::AppState {
    let state = start(dir).await;
    for file in [
        "trades_initial.csv",
        "trades_incremental.csv",
        "trades_corrected.csv",
    ] {
        import_trades(
            &state.pool,
            state.manifest.valuation_dates(),
            &fixture(file),
        )
        .await
        .unwrap();
    }
    state
}

/// The exact, unrounded totals as of the manifest's final valuation date.
/// They match an independent recomputation with Python's `decimal` module
/// straight from the CSVs, following DATA_CONTRACT.md section 4.
#[tokio::test]
async fn final_date_portfolio_totals_match_the_independent_recomputation() {
    let dir = TempDir::new().unwrap();
    let state = start_with_valid_fixtures(&dir).await;
    let as_of = state.manifest.as_of();

    // (portfolio, market value, net trade cost, total P&L, open, closed)
    let expected = [
        (
            "P001",
            dec!(2253433.8268696607),
            dec!(2168668.7836127341),
            dec!(84765.0432569266),
            24,
            0,
        ),
        (
            "P002",
            dec!(1380876.8303846794),
            dec!(1298218.5399060502),
            dec!(82658.2904786292),
            23,
            1,
        ),
        (
            "P003",
            dec!(1514895.4923108588),
            dec!(1462413.3360049331),
            dec!(52482.1563059257),
            24,
            0,
        ),
    ];

    for (portfolio_id, market_value, net_cost, pnl, open, closed) in expected {
        let view = load_portfolio(
            &state.pool,
            as_of,
            &PortfolioId::parse(portfolio_id).unwrap(),
        )
        .await
        .unwrap();

        let totals = view.totals.unwrap();
        assert_eq!(totals.market_value_gbp, market_value, "{portfolio_id}");
        assert_eq!(totals.net_trade_cost_gbp, net_cost, "{portfolio_id}");
        assert_eq!(totals.total_pnl_gbp, pnl, "{portfolio_id}");

        let closed_seen = view.positions.iter().filter(|p| p.is_closed()).count();
        assert_eq!(closed_seen, closed, "{portfolio_id}");
        assert_eq!(view.positions.len() - closed_seen, open, "{portfolio_id}");
    }
}

#[tokio::test]
async fn portfolio_figures_reconcile_to_trades_and_reference_data() {
    let dir = TempDir::new().unwrap();
    let state = start_with_valid_fixtures(&dir).await;

    let as_of = state.manifest.as_of();

    let mut positions_seen = 0;
    let mut carried_forward_seen = false;
    for portfolio_id in ["P001", "P002", "P003"] {
        let view = load_portfolio(
            &state.pool,
            as_of,
            &PortfolioId::parse(portfolio_id).unwrap(),
        )
        .await
        .unwrap();
        let totals = view
            .totals
            .expect("fixtures supply full price and FX coverage");

        let mut sum_mv = Decimal::ZERO;
        let mut sum_pnl = Decimal::ZERO;
        for position in &view.positions {
            positions_seen += 1;
            let v = position.valuation.as_ref().unwrap();

            // Quantity and net cost are the sums of the per-trade figures.
            let qty: i64 = v.contributions.iter().map(|c| c.signed_quantity).sum();
            let cost: Decimal = v.contributions.iter().map(|c| c.signed_cost_gbp).sum();
            assert_eq!(v.quantity, qty);
            assert_eq!(position.quantity, Some(qty));
            assert_eq!(v.net_trade_cost_gbp, cost);
            assert!(v.quantity >= 0, "valid fixtures never go short");

            for c in &v.contributions {
                let trade_date = c.trade.timestamp.utc_date();
                assert!(trade_date <= as_of);
                // Trade-day FX is never from a later date.
                assert!(c.fx.date <= trade_date);
                assert_eq!(
                    c.signed_cost_gbp,
                    Decimal::from(c.signed_quantity) * c.trade.price.value() * c.fx.value.value()
                );
            }

            // Market value and P&L follow the contract's formulas exactly.
            // The fixtures price every instrument, closed ones included.
            let price = v.valuation_price.expect("fixture price coverage");
            let fx = v.valuation_fx.expect("fixture FX coverage");
            assert!(price.date <= as_of && fx.date <= as_of);
            assert_eq!(
                v.market_value_gbp,
                Decimal::from(v.quantity) * price.value.value() * fx.value.value()
            );
            assert_eq!(v.total_pnl_gbp, v.market_value_gbp - v.net_trade_cost_gbp);
            if v.quantity == 0 {
                assert_eq!(v.market_value_gbp, Decimal::ZERO);
            }
            if price.date < as_of {
                carried_forward_seen = true;
            }

            sum_mv += v.market_value_gbp;
            sum_pnl += v.total_pnl_gbp;
        }

        // Portfolio totals are sums of unrounded position values.
        assert_eq!(totals.market_value_gbp, sum_mv);
        assert_eq!(totals.total_pnl_gbp, sum_pnl);
        let displayed = format_money(totals.total_pnl_gbp);
        assert_eq!(
            displayed.split('.').nth(1).map(str::len),
            Some(2),
            "{displayed}"
        );
    }

    assert!(positions_seen > 0);
    // I017 has no price on the as_of date in the supplied data.
    assert!(
        carried_forward_seen,
        "expected at least one carried-forward valuation price"
    );
}
