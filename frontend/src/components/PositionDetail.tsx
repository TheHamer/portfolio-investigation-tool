import { useEffect, useRef, useState } from 'react';
import { Breakdown, PositionDetailResponse, TradeLine, TradeSide, fetchPositionDetail } from '../api';
import { formatGbp, formatQuantity, formatTimestamp, groupDigits, signClass } from '../format';

interface Props {
  portfolioId: string;
  instrumentId: string;
  refreshKey: number;
  onClose: () => void;
}

export function PositionDetail({ portfolioId, instrumentId, refreshKey, onClose }: Props) {
  const [data, setData] = useState<PositionDetailResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [side, setSide] = useState<TradeSide | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);

  // Open as a modal so the page underneath stays where it was.
  useEffect(() => {
    const d = dialog.current;
    if (d && !d.open) d.showModal();
  }, []);

  // A different position starts from "Loading"; changing only the side
  // filter keeps the current view until the filtered trades arrive.
  useEffect(() => setData(null), [portfolioId, instrumentId, refreshKey]);

  useEffect(() => {
    let cancelled = false;
    setError(null);

    fetchPositionDetail(portfolioId, instrumentId, side)
      .then((response) => !cancelled && setData(response))
      .catch((e: Error) => {
        if (cancelled) return;
        setData(null);
        setError(e.message);
      });

    return () => {
      cancelled = true;
    };
  }, [portfolioId, instrumentId, refreshKey, side]);

  const p = data?.position;

  return (
    <dialog
      ref={dialog}
      className="modal"
      aria-labelledby="position-title"
      // Fired by Escape, the Close button and backdrop clicks alike.
      onClose={onClose}
      onMouseDown={(e) => e.target === e.currentTarget && dialog.current?.close()}
    >
      <div className="modal-header">
        <h2 id="position-title">
          Position investigation
          {data && p && (
            <span className="subtitle">
              {p.ticker} · {p.name} in {data.portfolio.name}
            </span>
          )}
        </h2>
        <button className="button secondary small" onClick={() => dialog.current?.close()}>
          Close
        </button>
      </div>

      <div className="modal-body">
        {error && <div className="alert error">Could not load the position: {error}</div>}
        {!data && !error && <p className="muted">Loading position…</p>}

        {data && p && (
          <>
            {p.status === 'unavailable' && (
              <div className="alert warning">This position cannot be valued: {p.unavailable_reason}</div>
            )}

            <h3>Position summary</h3>
            <dl className="summary-cards">
              <div className="summary-card">
                <dt>Quantity</dt>
                <dd>
                  {formatQuantity(p.quantity)} {p.is_closed && <span className="badge">Closed</span>}
                </dd>
              </div>
              <div className="summary-card">
                <dt>Market value</dt>
                <dd>{formatGbp(p.market_value_gbp)}</dd>
              </div>
              <div className="summary-card">
                <dt>Net trade cost</dt>
                <dd>{formatGbp(p.net_trade_cost_gbp)}</dd>
              </div>
              <div className="summary-card">
                <dt>Cumulative total P&amp;L</dt>
                <dd className={signClass(p.total_pnl_gbp)}>{formatGbp(p.total_pnl_gbp)}</dd>
              </div>
            </dl>

            {data.breakdown && (
              <ValuationBreakdown
                breakdown={data.breakdown}
                currency={p.currency}
                asOf={data.as_of}
                marketValue={p.market_value_gbp}
                netCost={p.net_trade_cost_gbp}
                pnl={p.total_pnl_gbp}
              />
            )}

            <TradesTable
              trades={data.trades}
              side={data.side}
              onSideChange={setSide}
              totalTrades={p.trade_count}
              currency={p.currency}
              breakdown={data.breakdown}
              netCost={p.net_trade_cost_gbp}
            />
          </>
        )}
      </div>
    </dialog>
  );
}

function ValuationBreakdown({
  breakdown: b,
  currency,
  asOf,
  marketValue,
  netCost,
  pnl,
}: {
  breakdown: Breakdown;
  currency: string;
  asOf: string;
  marketValue: string | null;
  netCost: string | null;
  pnl: string | null;
}) {
  const price = b.valuation_price;
  const fx = b.valuation_fx;

  return (
    <>
      <h3>Valuation breakdown (as of {asOf})</h3>
      <div className="table-wrap">
        <table className="breakdown">
          <thead>
            <tr>
              <th>Figure</th>
              <th>How it is calculated</th>
              <th className="num">Exact value</th>
              <th className="num">Displayed</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td>Position quantity</td>
              <td>Sum of signed trade quantities (BUY +, SELL −)</td>
              <td className="num">{formatQuantity(b.quantity)}</td>
              <td className="num">{formatQuantity(b.quantity)}</td>
            </tr>
            <tr>
              <td>Valuation price ({currency})</td>
              {price ? (
                <>
                  <td>
                    Latest close price on or before {asOf}: dated {price.date}
                    {price.carried_forward && <span className="badge warn">carried forward — no price on {asOf}</span>}
                  </td>
                  <td className="num">{price.price}</td>
                  <td className="num">{price.price}</td>
                </>
              ) : (
                <>
                  <td>No price on or before {asOf}; not needed, as a closed position is worth zero</td>
                  <td className="num">—</td>
                  <td className="num">—</td>
                </>
              )}
            </tr>
            <tr>
              <td>Valuation FX (GBP per {currency})</td>
              {fx ? (
                <>
                  <td>
                    Latest rate on or before {asOf}: dated {fx.date}
                    {fx.carried_forward && <span className="badge warn">carried forward</span>}
                  </td>
                  <td className="num">{fx.gbp_per_unit}</td>
                  <td className="num">{fx.gbp_per_unit}</td>
                </>
              ) : (
                <>
                  <td>No rate on or before {asOf}; not needed, as a closed position is worth zero</td>
                  <td className="num">—</td>
                  <td className="num">—</td>
                </>
              )}
            </tr>
            <tr>
              <td>Market value ({currency})</td>
              <td>Quantity × valuation price</td>
              <td className="num">{groupDigits(b.market_value_local_exact)}</td>
              <td className="num">—</td>
            </tr>
            <tr>
              <td>Market value (GBP)</td>
              <td>Market value ({currency}) × valuation FX</td>
              <td className="num">{groupDigits(b.market_value_gbp_exact)}</td>
              <td className="num">{formatGbp(marketValue)}</td>
            </tr>
            <tr>
              <td>Net trade cost (GBP)</td>
              <td>Sum of the signed GBP trade costs below, each at its own trade-day FX rate</td>
              <td className="num">{groupDigits(b.net_trade_cost_gbp_exact)}</td>
              <td className="num">{formatGbp(netCost)}</td>
            </tr>
            <tr className="result">
              <td>Cumulative total P&amp;L (GBP)</td>
              <td>Market value (GBP) − net trade cost (GBP)</td>
              <td className="num">{groupDigits(b.total_pnl_gbp_exact)}</td>
              <td className={`num ${signClass(pnl)}`}>{formatGbp(pnl)}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </>
  );
}

const SIDE_FILTERS: { label: string; side: TradeSide | null }[] = [
  { label: 'All', side: null },
  { label: 'Buy', side: 'BUY' },
  { label: 'Sell', side: 'SELL' },
];

function TradesTable({
  trades,
  side,
  onSideChange,
  totalTrades,
  currency,
  breakdown,
  netCost,
}: {
  trades: TradeLine[];
  side: TradeSide | null;
  onSideChange: (side: TradeSide | null) => void;
  totalTrades: number;
  currency: string;
  breakdown: Breakdown | null;
  netCost: string | null;
}) {
  return (
    <>
      <h3>
        Underlying trades ({side ? `${trades.length} of ${totalTrades}` : trades.length})
      </h3>
      <div className="side-filter" role="group" aria-label="Filter trades by side">
        {SIDE_FILTERS.map((f) => (
          <button
            key={f.label}
            className="button secondary small"
            aria-pressed={side === f.side}
            onClick={() => onSideChange(f.side)}
          >
            {f.label}
          </button>
        ))}
      </div>
      <p className="muted small">
        In execution order. Signed cost (GBP) = signed quantity × execution price × FX rate on (or most recently before)
        the trade's UTC date. Historical trades are never revalued at today's FX rate.
      </p>
      <div className="table-wrap">
        <table className="trades">
          <thead>
            <tr>
              <th>Trade ID</th>
              <th>Executed (UTC)</th>
              <th>Side</th>
              <th className="num">Quantity</th>
              <th className="num">Price ({currency})</th>
              <th className="num">Signed amount ({currency})</th>
              <th className="num">Trade-day FX</th>
              <th className="num">Signed cost (GBP)</th>
              <th className="num">Running qty</th>
            </tr>
          </thead>
          <tbody>
            {trades.map((t) => (
              <tr key={t.trade_id}>
                <td>{t.trade_id}</td>
                <td>{formatTimestamp(t.timestamp)}</td>
                <td>
                  <span className={`side ${t.side.toLowerCase()}`}>{t.side}</span>
                </td>
                <td className="num">{formatQuantity(t.quantity)}</td>
                <td className="num">{t.price}</td>
                <td className="num">{t.signed_local_amount === null ? '—' : groupDigits(t.signed_local_amount)}</td>
                <td className="num">
                  {t.fx ? (
                    <>
                      {t.fx.gbp_per_unit}
                      <div className="muted small">
                        {t.fx.date}
                        {t.fx.carried_forward && <span className="badge warn">carried forward</span>}
                      </div>
                    </>
                  ) : (
                    '—'
                  )}
                </td>
                <td className="num" title={t.signed_cost_gbp_exact ?? undefined}>
                  {formatGbp(t.signed_cost_gbp)}
                </td>
                <td className="num">{formatQuantity(t.running_quantity)}</td>
              </tr>
            ))}
          </tbody>
          {breakdown && (
            <tfoot>
              <tr>
                <td colSpan={7}>
                  {side ? 'Position totals across all trades' : 'Totals'} (net trade cost summed from unrounded trade
                  costs)
                </td>
                <td className="num">{formatGbp(netCost)}</td>
                <td className="num">{formatQuantity(breakdown.quantity)}</td>
              </tr>
            </tfoot>
          )}
        </table>
      </div>
    </>
  );
}
