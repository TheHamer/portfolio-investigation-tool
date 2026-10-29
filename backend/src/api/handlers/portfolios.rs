//! Portfolio listing, position summaries and position investigation.

use axum::Json;
use axum::extract::{Path, State};

use crate::AppState;
use crate::api::ApiError;
use crate::api::dto::{
    Breakdown, PortfolioListItem, PortfolioListResponse, PortfolioPositionsResponse,
    PositionDetailResponse, PositionSummary, Status, Totals, TradeLine,
};
use crate::api::extractors::ApiPath;
use crate::domain::{InstrumentId, PortfolioId};
use crate::error::AppError;
use crate::portfolio::{self, PositionView};

/// `GET /api/portfolios`: every portfolio with its stored trade count.
pub(in crate::api) async fn list_portfolios(
    State(state): State<AppState>,
) -> Result<Json<PortfolioListResponse>, ApiError> {
    let portfolios = portfolio::list_portfolios(&state.pool)
        .await
        .map_err(AppError::from)?
        .into_iter()
        .map(|(portfolio, trade_count)| PortfolioListItem::new(portfolio, trade_count))
        .collect();

    Ok(Json(PortfolioListResponse {
        as_of: state.manifest.as_of(),
        dataset_id: state.manifest.dataset_id().to_owned(),
        portfolios,
    }))
}

/// `GET /api/portfolios/{portfolio_id}/positions`: every position and, when
/// all of them can be valued, the portfolio totals.
pub(in crate::api) async fn portfolio_positions(
    State(state): State<AppState>,
    path: ApiPath<PortfolioId>,
) -> Result<Json<PortfolioPositionsResponse>, ApiError> {
    let Path(portfolio_id) = path.into_inner();
    let as_of = state.manifest.as_of();

    let view = portfolio::load_portfolio(&state.pool, as_of, &portfolio_id)
        .await
        .map_err(AppError::from)?;

    // Counted from the domain results, not the DTOs. Unavailable positions
    // are counted like any other, by quantity alone.
    let closed = view.positions.iter().filter(|p| p.is_closed()).count();
    let open = view.positions.len() - closed;

    let positions: Vec<PositionSummary> = view
        .positions
        .iter()
        .map(|p| PositionSummary::new(p, as_of))
        .collect();

    let (status, totals, unavailable_reason) = match view.totals {
        Ok(t) => (Status::Ok, Some(Totals::new(&t)), None),
        Err(reason) => (Status::Unavailable, None, Some(reason.to_string())),
    };

    Ok(Json(PortfolioPositionsResponse {
        portfolio: view.portfolio.into(),
        as_of,
        status,
        unavailable_reason,
        totals,
        open_position_count: open,
        closed_position_count: closed,
        positions,
    }))
}

/// `GET /api/portfolios/{portfolio_id}/positions/{instrument_id}`: one
/// position with its calculation breakdown and underlying trades.
pub(in crate::api) async fn position_detail(
    State(state): State<AppState>,
    path: ApiPath<(PortfolioId, InstrumentId)>,
) -> Result<Json<PositionDetailResponse>, ApiError> {
    let Path((portfolio_id, instrument_id)) = path.into_inner();
    let as_of = state.manifest.as_of();

    let PositionView {
        portfolio,
        position,
        trades,
    } = portfolio::load_position(&state.pool, as_of, &portfolio_id, &instrument_id)
        .await
        .map_err(AppError::from)?;

    let currency = position.instrument.currency;

    let breakdown = position
        .valuation
        .as_ref()
        .ok()
        .map(|v| Breakdown::new(v, currency, as_of));

    let trade_lines = match &position.valuation {
        Ok(v) => v
            .contributions
            .iter()
            .map(|c| TradeLine::calculated(c, currency))
            .collect(),
        Err(_) => trades
            .iter()
            .map(|t| TradeLine::unvalued(t, currency))
            .collect(),
    };

    Ok(Json(PositionDetailResponse {
        portfolio: portfolio.into(),
        as_of,
        position: PositionSummary::new(&position, as_of),
        breakdown,
        trades: trade_lines,
    }))
}
