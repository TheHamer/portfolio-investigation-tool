import { useEffect, useState } from 'react';
import { PortfolioPositionsResponse, PositionSummary, fetchPositions } from '../api';
import { formatGbp, formatQuantity, signClass } from '../format';

interface Props {
  portfolioId: string;
  /** Changes whenever trades are imported, to trigger a reload. */
  refreshKey: number;
  selectedInstrumentId: string | null;
  onSelectPosition: (instrumentId: string) => void;
}

export function PortfolioOverview({ portfolioId, refreshKey, selectedInstrumentId, onSelectPosition }: Props) {
  const [data, setData] = useState<PortfolioPositionsResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);

    fetchPositions(portfolioId)
      .then((response) => !cancelled && setData(response))
      .catch((e: Error) => !cancelled && setError(e.message))
      .finally(() => !cancelled && setLoading(false));

    return () => {
      cancelled = true;
    };
  }, [portfolioId, refreshKey]);

  if (error) return <div className="alert error">Could not load the portfolio: {error}</div>;
  if (!data || (loading && data.portfolio.portfolio_id !== portfolioId)) {
    return <p className="muted">Loading portfolio…</p>;
  }

  const { totals } = data;

  return (
    <div className={loading ? 'refreshing' : undefined}>
      <div className="portfolio-heading">
        <h3>
          {data.portfolio.name} <span className="muted">({data.portfolio.portfolio_id})</span>
        </h3>
        <span className="muted">Valued as of {data.as_of} · amounts in GBP</span>
      </div>

      {data.status === 'unavailable' && <div className="alert warning">{data.unavailable_reason}</div>}

      <dl className="summary-cards">
        <div className="summary-card">
          <dt>Holdings market value</dt>
          <dd>{totals ? formatGbp(totals.market_value_gbp) : 'Unavailable'}</dd>
        </div>
        <div className="summary-card">
          <dt>Cumulative total P&amp;L</dt>
          <dd className={signClass(totals?.total_pnl_gbp)}>
            {totals ? formatGbp(totals.total_pnl_gbp) : 'Unavailable'}
          </dd>
        </div>
        <div className="summary-card">
          <dt>Net trade cost</dt>
          <dd>{totals ? formatGbp(totals.net_trade_cost_gbp) : 'Unavailable'}</dd>
        </div>
        <div className="summary-card">
          <dt>Positions</dt>
          <dd>
            {data.open_position_count} open
            <span className="muted small"> · {data.closed_position_count} closed</span>
          </dd>
        </div>
      </dl>

      {data.positions.length === 0 ? (
        <div className="empty">
          No trades have been imported for this portfolio yet. Upload a trade file above to see its positions.
        </div>
      ) : (
        <>
          <div className="table-wrap">
            <table className="positions">
              <thead>
                <tr>
                  <th>Instrument</th>
                  <th className="num">Quantity</th>
                  <th className="num">Valuation price</th>
                  <th className="num">FX (GBP per unit)</th>
                  <th className="num">Market value</th>
                  <th className="num">Net trade cost</th>
                  <th className="num">Total P&amp;L</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {data.positions.map((p) => (
                  <PositionRow
                    key={p.instrument_id}
                    position={p}
                    selected={p.instrument_id === selectedInstrumentId}
                    onSelect={() => onSelectPosition(p.instrument_id)}
                  />
                ))}
              </tbody>
            </table>
          </div>
          <p className="muted small">
            Market value = quantity × valuation price × valuation FX. Total P&amp;L = market value − net trade cost.
            Figures are rounded half-up to 2 decimal places for display; portfolio totals are summed from unrounded
            position values, so they can differ from the sum of the rounded rows by a few pennies.
          </p>
        </>
      )}
    </div>
  );
}

// Exported for its rendering tests.
export function PositionRow({
  position: p,
  selected,
  onSelect,
}: {
  position: PositionSummary;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <tr className={selected ? 'selected' : undefined}>
      <td>
        <div className="ticker">
          {p.ticker} {p.is_closed && <span className="badge">Closed</span>}
        </div>
        <div className="muted small">
          {p.name} · {p.currency}
        </div>
      </td>
      <td className="num">{formatQuantity(p.quantity)}</td>
      {p.status === 'ok' ? (
        <>
          <td className="num">
            {p.valuation_price ? (
              <>
                {p.valuation_price.price} {p.currency}
                <div className="muted small">
                  {p.valuation_price.date}
                  {p.valuation_price.carried_forward && <span className="badge warn">carried forward</span>}
                </div>
              </>
            ) : (
              <NotNeeded />
            )}
          </td>
          <td className="num">
            {p.valuation_fx ? (
              <>
                {p.valuation_fx.gbp_per_unit}
                <div className="muted small">
                  {p.valuation_fx.date}
                  {p.valuation_fx.carried_forward && <span className="badge warn">carried forward</span>}
                </div>
              </>
            ) : (
              <NotNeeded />
            )}
          </td>
          <td className="num">{formatGbp(p.market_value_gbp)}</td>
          <td className="num">{formatGbp(p.net_trade_cost_gbp)}</td>
          <td className={`num ${signClass(p.total_pnl_gbp)}`}>{formatGbp(p.total_pnl_gbp)}</td>
        </>
      ) : (
        <td colSpan={5} className="unavailable">
          Valuation unavailable: {p.unavailable_reason}
        </td>
      )}
      <td>
        <button className="button small" onClick={onSelect} aria-pressed={selected}>
          Investigate
        </button>
      </td>
    </tr>
  );
}

/** In place of a price or FX rate that a valued position lacks: only a closed
 * position, worth zero whatever the price, can be valued without them. */
function NotNeeded() {
  return (
    <>
      —<div className="muted small">not needed (closed)</div>
    </>
  );
}
