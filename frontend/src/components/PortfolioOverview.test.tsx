import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { PositionSummary } from '../api';
import { PositionRow } from './PortfolioOverview';

const OPEN: PositionSummary = {
  instrument_id: 'I017',
  ticker: 'ZZBRA17',
  name: 'Brannock',
  currency: 'EUR',
  sector: 'Energy',
  trade_count: 11,
  quantity: 1726,
  is_closed: false,
  status: 'ok',
  unavailable_reason: null,
  net_trade_cost_gbp: '161750.23',
  market_value_gbp: '170212.28',
  total_pnl_gbp: '8462.05',
  valuation_price: { price: '119.5408', currency: 'EUR', date: '2026-03-26', carried_forward: true },
  valuation_fx: { gbp_per_unit: '0.824962', currency: 'EUR', date: '2026-03-27', carried_forward: false },
};

function render(position: PositionSummary): string {
  return renderToStaticMarkup(
    <table>
      <tbody>
        <PositionRow position={position} selected={false} onSelect={() => {}} />
      </tbody>
    </table>,
  );
}

const badges = (html: string) => html.match(/carried forward/g)?.length ?? 0;

describe('PositionRow', () => {
  it('shows each observation with its date and flags a carried-forward price', () => {
    const html = render(OPEN);

    expect(html).toContain('119.5408 EUR');
    expect(html).toContain('2026-03-26');
    expect(html).toContain('0.824962');
    expect(badges(html)).toBe(1);
    expect(html).toContain('£170,212.28');
  });

  it('flags a carried-forward FX rate like a carried-forward price', () => {
    const html = render({
      ...OPEN,
      valuation_price: { ...OPEN.valuation_price!, date: '2026-03-27', carried_forward: false },
      valuation_fx: { ...OPEN.valuation_fx!, date: '2026-03-26', carried_forward: true },
    });

    expect(badges(html)).toBe(1);
    expect(html).toMatch(/0\.824962<div class="muted small">2026-03-26<span class="badge warn">carried forward/);
  });

  it('shows a closed position without a price as valued, with the observations not needed', () => {
    const html = render({
      ...OPEN,
      quantity: 0,
      is_closed: true,
      market_value_gbp: '0.00',
      net_trade_cost_gbp: '-24686.16',
      total_pnl_gbp: '24686.16',
      valuation_price: null,
      valuation_fx: null,
    });

    expect(html).toContain('Closed');
    expect(html.match(/not needed \(closed\)/g)?.length).toBe(2);
    expect(html).toContain('£0.00');
    expect(html).toContain('£24,686.16');
    expect(html).not.toContain('unavailable');
    // Closed positions still contribute P&L, so they must stay investigable.
    expect(html).toContain('Investigate');
  });

  it('explains an unavailable position instead of showing figures', () => {
    const html = render({
      ...OPEN,
      status: 'unavailable',
      unavailable_reason: 'No price for I017 is available on or before 2026-03-27',
      net_trade_cost_gbp: null,
      market_value_gbp: null,
      total_pnl_gbp: null,
      valuation_price: null,
      valuation_fx: null,
    });

    expect(html).toContain('Valuation unavailable: No price for I017 is available on or before 2026-03-27');
    expect(html).not.toContain('not needed');
  });
});
