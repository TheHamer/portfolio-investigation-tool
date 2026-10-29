//! JSON response shapes and their construction from application results.
//!
//! Decimal values are serialised as strings so no precision is lost in
//! transit. Display amounts (`*_gbp`) are rounded half up to 2 dp; `*_exact`
//! fields carry the unrounded values used in calculations.
//!
//! Every response declares the fields it exposes. Domain and application
//! structs (`Portfolio`, `Instrument`, `ImportReport`) have no `Serialize`
//! implementation and are converted here, so changing them cannot change the
//! HTTP contract.
//!
//! - `portfolios`: the portfolio list, positions and position-detail responses.
//! - `imports`: the trade import response.
//! - This module: the probe response and the formatting shared by both.

mod imports;
mod portfolios;

pub(super) use imports::ImportResponse;
pub(super) use portfolios::{
    Breakdown, PortfolioListItem, PortfolioListResponse, PortfolioPositionsResponse,
    PositionDetailResponse, PositionSummary, Status, Totals, TradeLine,
};

use rust_decimal::Decimal;
use serde::Serialize;

fn exact(value: Decimal) -> String {
    if value.is_zero() {
        "0".into()
    } else {
        value.normalize().to_string()
    }
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Body of the liveness and readiness probes.
#[derive(Serialize)]
pub(super) struct ProbeResponse {
    pub(super) status: &'static str,
}

/// The exact JSON text of a response, for the tests in this module and its
/// submodules.
#[cfg(test)]
fn json(value: &impl Serialize) -> String {
    serde_json::to_string(value).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_response_is_just_a_status() {
        assert_eq!(json(&ProbeResponse { status: "ok" }), r#"{"status":"ok"}"#);
    }
}
