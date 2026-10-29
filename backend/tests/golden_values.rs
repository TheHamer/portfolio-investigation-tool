//! Golden values for the supplied reference dataset (`ds-f77160d14d37a72b`).
//!
//! The five upload fixtures are sent through the HTTP API in the contract's
//! order (initial, repeat, incremental, invalid, corrected), and the figures
//! the API returns are compared with values calculated independently by
//! `tools/recompute.py`. That script shares no code with the backend: it
//! replays the same fixtures from the CSVs with Python's `decimal` module and
//! follows DATA_CONTRACT.md section 4. Regenerate the expected values with:
//!
//! ```text
//! python tools/recompute.py --position P002/I014 --position P002/I017 --position P002/I020
//! ```
//!
//! These constants describe this dataset only; nothing in the application
//! depends on them, and another dataset would produce different figures.

mod common;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::multipart_upload;
use http_body_util::BodyExt;
use portfolio_investigation::build_router;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn get(app: &Router, uri: &str) -> Value {
    let (status, body) = send(app, Request::get(uri).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

/// Uploads the five fixtures in the contract's order, checking each outcome,
/// and returns the router over the resulting database.
async fn app_after_fixture_sequence(dir: &TempDir) -> Router {
    let app = build_router(common::start(dir).await, None);

    // (file, HTTP status, outcome, inserted, skipped, invalid, aborted)
    let sequence = [
        (
            "trades_initial.csv",
            200,
            "success",
            json!(600),
            json!(0),
            json!(0),
            json!(0),
        ),
        (
            "trades_repeat.csv",
            200,
            "success",
            json!(0),
            json!(600),
            json!(0),
            json!(0),
        ),
        (
            "trades_incremental.csv",
            200,
            "success",
            json!(60),
            json!(0),
            json!(0),
            json!(0),
        ),
        (
            "trades_invalid.csv",
            422,
            "rejected",
            json!(0),
            json!(0),
            json!(8),
            json!(2),
        ),
        (
            "trades_corrected.csv",
            200,
            "success",
            json!(10),
            json!(0),
            json!(0),
            json!(0),
        ),
    ];

    for (file, status, outcome, inserted, skipped, invalid, aborted) in sequence {
        let (actual_status, body) =
            send(&app, multipart_upload(file, &common::fixture(file))).await;

        assert_eq!(actual_status.as_u16(), status, "{file}: {body}");
        assert_eq!(body["outcome"], outcome, "{file}");
        assert_eq!(body["inserted_rows"], inserted, "{file}");
        assert_eq!(body["skipped_rows"], skipped, "{file}");
        assert_eq!(body["invalid_rows"], invalid, "{file}");
        assert_eq!(body["aborted_rows"], aborted, "{file}");
    }

    app
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
async fn portfolio_totals_match_the_independent_recomputation() {
    let dir = TempDir::new().unwrap();
    let app = app_after_fixture_sequence(&dir).await;

    let portfolios = get(&app, "/api/portfolios").await;
    assert_eq!(portfolios["dataset_id"], "ds-f77160d14d37a72b");
    assert_eq!(portfolios["as_of"], "2026-03-27");

    // (portfolio, open, closed, totals). Each displayed figure is the exact
    // total rounded on its own: for P001 the displayed P&L is 84765.04, one
    // penny below 2253433.83 − 2168668.78, as the contract allows.
    let expected = [
        (
            "P001",
            24,
            0,
            json!({
                "market_value_gbp": "2253433.83",
                "net_trade_cost_gbp": "2168668.78",
                "total_pnl_gbp": "84765.04",
                "market_value_gbp_exact": "2253433.8268696607",
                "net_trade_cost_gbp_exact": "2168668.7836127341",
                "total_pnl_gbp_exact": "84765.0432569266",
            }),
        ),
        (
            "P002",
            23,
            1,
            json!({
                "market_value_gbp": "1380876.83",
                "net_trade_cost_gbp": "1298218.54",
                "total_pnl_gbp": "82658.29",
                "market_value_gbp_exact": "1380876.8303846794",
                "net_trade_cost_gbp_exact": "1298218.5399060502",
                "total_pnl_gbp_exact": "82658.2904786292",
            }),
        ),
        (
            "P003",
            24,
            0,
            json!({
                "market_value_gbp": "1514895.49",
                "net_trade_cost_gbp": "1462413.34",
                "total_pnl_gbp": "52482.16",
                "market_value_gbp_exact": "1514895.4923108588",
                "net_trade_cost_gbp_exact": "1462413.3360049331",
                "total_pnl_gbp_exact": "52482.1563059257",
            }),
        ),
    ];

    for (portfolio_id, open, closed, totals) in expected {
        let body = get(&app, &format!("/api/portfolios/{portfolio_id}/positions")).await;

        assert_eq!(body["status"], "ok", "{portfolio_id}");
        assert_eq!(body["totals"], totals, "{portfolio_id}");
        assert_eq!(body["open_position_count"], open, "{portfolio_id}");
        assert_eq!(body["closed_position_count"], closed, "{portfolio_id}");
    }
}

/// P002 / ZZCAR14 (I014, GBP): bought on 2026-01-06 and sold in full on
/// 2026-03-02. Closed, so its market value is zero, but its P&L counts.
#[tokio::test]
async fn closed_position_p002_i014_matches_the_independent_recomputation() {
    let dir = TempDir::new().unwrap();
    let app = app_after_fixture_sequence(&dir).await;

    let body = get(&app, "/api/portfolios/P002/positions/I014").await;
    let summary = &body["position"];

    assert_eq!(summary["ticker"], "ZZCAR14");
    assert_eq!(summary["quantity"], 0);
    assert_eq!(summary["is_closed"], true);
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["market_value_gbp"], "0.00");
    assert_eq!(summary["net_trade_cost_gbp"], "-24686.16");
    assert_eq!(summary["total_pnl_gbp"], "24686.16");

    // Not needed for a closed position, but reported because they exist.
    assert_eq!(summary["valuation_price"]["price"], "310.5635");
    assert_eq!(summary["valuation_price"]["date"], "2026-03-27");
    assert_eq!(summary["valuation_fx"]["gbp_per_unit"], "1.000000");
    assert_eq!(summary["valuation_fx"]["date"], "2026-03-27");

    let breakdown = &body["breakdown"];
    assert_eq!(breakdown["market_value_gbp_exact"], "0");
    assert_eq!(breakdown["net_trade_cost_gbp_exact"], "-24686.1645");
    assert_eq!(breakdown["total_pnl_gbp_exact"], "24686.1645");

    // (trade, signed quantity, trade-day FX date, signed GBP cost)
    let trades: Vec<(&str, i64, &str, &str)> = body["trades"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["trade_id"].as_str().unwrap(),
                t["signed_quantity"].as_i64().unwrap(),
                t["fx"]["date"].as_str().unwrap(),
                t["signed_cost_gbp_exact"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        trades,
        vec![
            ("T770892", 515, "2026-01-06", "121844.365"),
            ("T719329", -515, "2026-03-02", "-146530.5295"),
        ]
    );
}

/// P002 / ZZBRA17 (I017, EUR): no price on the valuation date, so the
/// 2026-03-26 price is carried forward, while FX is the 2026-03-27 rate.
#[tokio::test]
async fn carried_forward_position_p002_i017_matches_the_independent_recomputation() {
    let dir = TempDir::new().unwrap();
    let app = app_after_fixture_sequence(&dir).await;

    let positions = get(&app, "/api/portfolios/P002/positions").await;
    let summary = position(&positions, "I017");

    assert_eq!(summary["ticker"], "ZZBRA17");
    assert_eq!(summary["quantity"], 1726);
    assert_eq!(summary["market_value_gbp"], "170212.28");
    assert_eq!(summary["net_trade_cost_gbp"], "161750.23");
    assert_eq!(summary["total_pnl_gbp"], "8462.05");
    assert_eq!(
        summary["valuation_price"],
        json!({"price": "119.5408", "currency": "EUR", "date": "2026-03-26", "carried_forward": true})
    );
    assert_eq!(
        summary["valuation_fx"],
        json!({"gbp_per_unit": "0.824962", "currency": "EUR", "date": "2026-03-27", "carried_forward": false})
    );

    let detail = get(&app, "/api/portfolios/P002/positions/I017").await;
    let breakdown = &detail["breakdown"];
    assert_eq!(breakdown["market_value_local_exact"], "206327.4208");
    assert_eq!(breakdown["market_value_gbp_exact"], "170212.2817180096");
    assert_eq!(breakdown["net_trade_cost_gbp_exact"], "161750.2329428466");
    assert_eq!(breakdown["total_pnl_gbp_exact"], "8462.048775163");

    // Every trade is converted at the EUR rate of its own trade date.
    assert_eq!(
        trade_fx(&detail),
        vec![
            ("T481251", "2026-01-05", "0.830630"),
            ("T450305", "2026-01-08", "0.827723"),
            ("T158667", "2026-01-14", "0.819391"),
            ("T803873", "2026-01-19", "0.822258"),
            ("T170177", "2026-02-10", "0.820832"),
            ("T749680", "2026-02-11", "0.821735"),
            ("T747222", "2026-02-18", "0.820984"),
            ("T988168", "2026-02-26", "0.825421"),
            ("T501696", "2026-03-03", "0.823028"),
            ("T834514", "2026-03-04", "0.823193"),
            ("T331862", "2026-03-09", "0.825321"),
        ]
    );
}

/// P002 / ZZHAR20 (I020, EUR): T229708 trades on 2026-01-09, a day with no
/// EUR rate, so its cost uses the 2026-01-08 rate, flagged as carried forward.
#[tokio::test]
async fn trade_day_fx_carried_over_a_missing_rate_matches_the_independent_recomputation() {
    let dir = TempDir::new().unwrap();
    let app = app_after_fixture_sequence(&dir).await;

    let detail = get(&app, "/api/portfolios/P002/positions/I020").await;

    let carried = detail["trades"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["trade_id"] == "T229708")
        .expect("T229708 is listed");
    assert_eq!(carried["trade_date"], "2026-01-09");
    assert_eq!(
        carried["fx"],
        json!({"gbp_per_unit": "0.827723", "currency": "EUR", "date": "2026-01-08", "carried_forward": true})
    );
    assert_eq!(carried["signed_cost_gbp_exact"], "-11570.1809384304");

    assert_eq!(
        trade_fx(&detail),
        vec![
            ("T271651", "2026-01-08", "0.827723"),
            ("T229708", "2026-01-08", "0.827723"),
            ("T793130", "2026-01-14", "0.819391"),
            ("T505575", "2026-02-11", "0.821735"),
            ("T696107", "2026-02-12", "0.823789"),
        ]
    );

    let summary = &detail["position"];
    assert_eq!(summary["quantity"], 16);
    assert_eq!(summary["market_value_gbp"], "731.88");
    assert_eq!(summary["net_trade_cost_gbp"], "603.65");
    assert_eq!(summary["total_pnl_gbp"], "128.24");
    assert_eq!(
        detail["breakdown"]["market_value_gbp_exact"],
        "731.8812075552"
    );
    assert_eq!(
        detail["breakdown"]["net_trade_cost_gbp_exact"],
        "603.6450308754"
    );
    assert_eq!(detail["breakdown"]["total_pnl_gbp_exact"], "128.2361766798");
}

/// Each trade's `(trade_id, trade-day FX date, rate)`, in execution order.
fn trade_fx(detail: &Value) -> Vec<(&str, &str, &str)> {
    detail["trades"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["trade_id"].as_str().unwrap(),
                t["fx"]["date"].as_str().unwrap(),
                t["fx"]["gbp_per_unit"].as_str().unwrap(),
            )
        })
        .collect()
}
