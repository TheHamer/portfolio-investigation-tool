//! Closed positions and missing market data over HTTP. A closed position
//! (net quantity zero) is worth nothing whatever its current price, so a
//! missing valuation price does not make it unavailable; its historical
//! trade-day FX rates are still required. An open position still needs both
//! a valuation price and a valuation FX rate.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::multipart_upload;
use http_body_util::BodyExt;
use portfolio_investigation::build_router;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

/// P001: I014 (GBP) bought and sold in full, so closed with a GBP 10 profit;
/// I002 (GBP) open, 5 shares bought at 80. P002: I001 (USD) bought and sold
/// in full.
const TRADES: &str = concat!(
    "trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price\n",
    "C1,P001,I014,2026-01-05T10:00:00Z,BUY,10,100\n",
    "C2,P001,I014,2026-01-06T10:00:00Z,SELL,10,101\n",
    "O1,P001,I002,2026-01-05T10:00:00Z,BUY,5,80\n",
    "U1,P002,I001,2026-01-05T10:00:00Z,BUY,4,100\n",
    "U2,P002,I001,2026-01-08T10:00:00Z,SELL,4,110\n",
);

async fn get(app: &Router, uri: &str) -> Value {
    let response = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");

    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn start_with_trades(dir: &TempDir) -> (Router, SqlitePool) {
    let state = common::start(dir).await;
    let pool = state.pool.clone();
    let app = build_router(state, None);

    let response = app
        .clone()
        .oneshot(multipart_upload("trades.csv", TRADES.as_bytes()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    (app, pool)
}

async fn execute(pool: &SqlitePool, sql: &str) {
    sqlx::query(sql).execute(pool).await.unwrap();
}

fn position<'a>(body: &'a Value, instrument_id: &str) -> &'a Value {
    body["positions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["instrument_id"] == instrument_id)
        .unwrap_or_else(|| panic!("{instrument_id} is listed"))
}

#[tokio::test]
async fn closed_position_without_a_current_price_is_valued_and_totalled() {
    let dir = TempDir::new().unwrap();
    let (app, pool) = start_with_trades(&dir).await;
    execute(&pool, "DELETE FROM prices WHERE instrument_id = 'I014'").await;

    let body = get(&app, "/api/portfolios/P001/positions").await;

    let closed = position(&body, "I014");
    assert_eq!(closed["is_closed"], true);
    assert_eq!(closed["status"], "ok");
    assert_eq!(closed["unavailable_reason"], Value::Null);
    assert_eq!(closed["market_value_gbp"], "0.00");
    assert_eq!(closed["net_trade_cost_gbp"], "-10.00");
    assert_eq!(closed["total_pnl_gbp"], "10.00");
    assert_eq!(closed["valuation_price"], Value::Null);
    // The valuation FX rate still exists, so it is still reported.
    assert_eq!(closed["valuation_fx"]["date"], "2026-03-27");

    // I002: 5 × 83.8414 (its 2026-03-27 price) = 419.207, cost 400.
    // Totals: market value 419.207 + 0, cost 400 − 10, P&L 19.207 + 10.
    assert_eq!(body["status"], "ok");
    assert_eq!(
        body["totals"],
        json!({
            "market_value_gbp": "419.21",
            "net_trade_cost_gbp": "390.00",
            "total_pnl_gbp": "29.21",
            "market_value_gbp_exact": "419.207",
            "net_trade_cost_gbp_exact": "390",
            "total_pnl_gbp_exact": "29.207",
        })
    );
    assert_eq!(body["open_position_count"], 1);
    assert_eq!(body["closed_position_count"], 1);

    let detail = get(&app, "/api/portfolios/P001/positions/I014").await;
    let breakdown = &detail["breakdown"];
    assert_eq!(breakdown["quantity"], 0);
    assert_eq!(breakdown["valuation_price"], Value::Null);
    assert_eq!(breakdown["market_value_gbp_exact"], "0");
    assert_eq!(breakdown["total_pnl_gbp_exact"], "10");
    assert_eq!(detail["trades"][1]["signed_cost_gbp_exact"], "-1010");
}

#[tokio::test]
async fn closed_position_uses_each_trades_historical_fx() {
    let dir = TempDir::new().unwrap();
    let (app, pool) = start_with_trades(&dir).await;
    execute(&pool, "DELETE FROM prices WHERE instrument_id = 'I001'").await;

    let detail = get(&app, "/api/portfolios/P002/positions/I001").await;

    assert_eq!(detail["position"]["status"], "ok");
    assert_eq!(detail["position"]["is_closed"], true);

    // Each trade is converted at its own trade-day USD rate, never today's.
    let rates: Vec<(&str, &str)> = detail["trades"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["fx"]["date"].as_str().unwrap(),
                t["fx"]["gbp_per_unit"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        rates,
        vec![("2026-01-05", "0.782484"), ("2026-01-08", "0.781848")]
    );

    // 4 × 100 × 0.782484 − 4 × 110 × 0.781848 = 312.9936 − 344.01312.
    let breakdown = &detail["breakdown"];
    assert_eq!(breakdown["net_trade_cost_gbp_exact"], "-31.01952");
    assert_eq!(breakdown["total_pnl_gbp_exact"], "31.01952");
    assert_eq!(breakdown["valuation_price"], Value::Null);
}

#[tokio::test]
async fn closed_position_missing_a_historical_fx_rate_is_unavailable() {
    let dir = TempDir::new().unwrap();
    let (app, pool) = start_with_trades(&dir).await;
    // The current USD rate remains; only rates up to the purchase are removed.
    execute(
        &pool,
        "DELETE FROM fx_rates WHERE currency = 'USD' AND date <= '2026-01-05'",
    )
    .await;

    let body = get(&app, "/api/portfolios/P002/positions").await;

    let closed = position(&body, "I001");
    assert_eq!(closed["is_closed"], true);
    assert_eq!(closed["status"], "unavailable");
    assert_eq!(
        closed["unavailable_reason"],
        "No USD FX rate is available on or before 2026-01-05 for trade U1"
    );
    assert_eq!(body["status"], "unavailable");
    assert_eq!(body["totals"], Value::Null);
}

#[tokio::test]
async fn open_position_without_a_current_price_is_unavailable() {
    let dir = TempDir::new().unwrap();
    let (app, pool) = start_with_trades(&dir).await;
    execute(
        &pool,
        "DELETE FROM prices WHERE instrument_id IN ('I002', 'I014')",
    )
    .await;

    let body = get(&app, "/api/portfolios/P001/positions").await;

    let open = position(&body, "I002");
    assert_eq!(open["is_closed"], false);
    assert_eq!(open["status"], "unavailable");
    assert_eq!(
        open["unavailable_reason"],
        "No price for I002 is available on or before 2026-03-27"
    );
    assert_eq!(open["valuation_price"], Value::Null);
    assert_eq!(open["market_value_gbp"], Value::Null);

    // The closed position is unaffected, but a partial total would mislead.
    assert_eq!(position(&body, "I014")["status"], "ok");
    assert_eq!(body["status"], "unavailable");
    assert_eq!(body["totals"], Value::Null);
}
