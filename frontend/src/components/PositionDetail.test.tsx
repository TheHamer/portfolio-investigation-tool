// @vitest-environment jsdom
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { PositionDetailResponse } from '../api';
import { mockFetch, render } from '../test-utils';
import { PositionDetail } from './PositionDetail';

// jsdom has <dialog> but not its modal API.
beforeAll(() => {
  HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
    this.setAttribute('open', '');
  };
  HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
    this.removeAttribute('open');
  };
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = '';
});

/** P002 / ZZCAR14: bought and sold in full, with no price on the valuation
 * date. Valued all the same, since a closed position is worth zero. */
const CLOSED_WITHOUT_PRICE: PositionDetailResponse = {
  portfolio: { portfolio_id: 'P002', name: 'Fictional Income Portfolio', base_currency: 'GBP' },
  as_of: '2026-03-27',
  position: {
    instrument_id: 'I014',
    ticker: 'ZZCAR14',
    name: 'Carvel',
    currency: 'GBP',
    sector: 'Industrials',
    trade_count: 2,
    quantity: 0,
    is_closed: true,
    status: 'ok',
    unavailable_reason: null,
    net_trade_cost_gbp: '-24686.16',
    market_value_gbp: '0.00',
    total_pnl_gbp: '24686.16',
    valuation_price: null,
    valuation_fx: null,
  },
  breakdown: {
    quantity: 0,
    valuation_price: null,
    valuation_fx: null,
    market_value_local_exact: '0',
    market_value_gbp_exact: '0',
    net_trade_cost_gbp_exact: '-24686.1645',
    total_pnl_gbp_exact: '24686.1645',
  },
  side: null,
  trades: [
    {
      trade_id: 'T770892',
      timestamp: '2026-01-06T09:56:23Z',
      trade_date: '2026-01-06',
      side: 'BUY',
      quantity: 515,
      signed_quantity: 515,
      price: '236.5910',
      currency: 'GBP',
      signed_local_amount: '121844.365',
      fx: { gbp_per_unit: '1.000000', currency: 'GBP', date: '2026-01-06', carried_forward: false },
      signed_cost_gbp: '121844.37',
      signed_cost_gbp_exact: '121844.365',
      running_quantity: 515,
    },
    {
      trade_id: 'T719329',
      timestamp: '2026-03-02T09:09:40Z',
      trade_date: '2026-03-02',
      side: 'SELL',
      quantity: 515,
      signed_quantity: -515,
      price: '284.5253',
      currency: 'GBP',
      signed_local_amount: '-146530.5295',
      fx: { gbp_per_unit: '1.000000', currency: 'GBP', date: '2026-03-02', carried_forward: false },
      signed_cost_gbp: '-146530.53',
      signed_cost_gbp_exact: '-146530.5295',
      running_quantity: 0,
    },
  ],
};

/** An open position whose trade-day FX is missing: no figures exist. */
const UNAVAILABLE: PositionDetailResponse = {
  ...CLOSED_WITHOUT_PRICE,
  position: {
    ...CLOSED_WITHOUT_PRICE.position,
    quantity: 515,
    is_closed: false,
    status: 'unavailable',
    unavailable_reason: 'No GBP FX rate is available on or before 2026-01-06 for trade T770892',
    net_trade_cost_gbp: null,
    market_value_gbp: null,
    total_pnl_gbp: null,
  },
  breakdown: null,
  trades: [
    {
      ...CLOSED_WITHOUT_PRICE.trades[0],
      fx: null,
      signed_cost_gbp: null,
      signed_cost_gbp_exact: null,
      running_quantity: null,
    },
  ],
};

async function showDetail(response: PositionDetailResponse) {
  mockFetch({ status: 200, body: response });
  const { container } = await render(
    <PositionDetail portfolioId="P002" instrumentId="I014" refreshKey={0} onClose={() => {}} />,
  );
  return container;
}

const cells = (row: Element) => [...row.querySelectorAll('td')].map((td) => td.textContent);

describe('PositionDetail', () => {
  it('values a closed position without a current price, showing its market value and P&L', async () => {
    const container = await showDetail(CLOSED_WITHOUT_PRICE);

    const summary = container.querySelector('.summary-cards')!.textContent;
    expect(summary).toContain('Market value£0.00');
    expect(summary).toContain('Net trade cost-£24,686.16');
    expect(summary).toContain('Cumulative total P&L£24,686.16');
    expect(container.textContent).toContain(
      'No price on or before 2026-03-27; not needed, as a closed position is worth zero',
    );
    expect(container.textContent).not.toContain('cannot be valued');
  });

  it('shows each trade with its signed amount, trade-day FX, GBP cost and running quantity', async () => {
    const container = await showDetail(CLOSED_WITHOUT_PRICE);

    const rows = [...container.querySelectorAll('table.trades tbody tr')].map(cells);
    expect(rows).toEqual([
      [
        'T770892',
        '2026-01-06 09:56:23',
        'BUY',
        '515',
        '236.5910',
        '121,844.365',
        '1.0000002026-01-06',
        '£121,844.37',
        '515',
      ],
      [
        'T719329',
        '2026-03-02 09:09:40',
        'SELL',
        '515',
        '284.5253',
        '-146,530.5295',
        '1.0000002026-03-02',
        '-£146,530.53',
        '0',
      ],
    ]);
    const footer = cells(container.querySelector('table.trades tfoot tr')!);
    expect(footer.slice(1)).toEqual(['-£24,686.16', '0']);
  });

  it('explains an unavailable position and shows dashes, never zeros, for missing figures', async () => {
    const container = await showDetail(UNAVAILABLE);

    expect(container.textContent).toContain(
      'This position cannot be valued: No GBP FX rate is available on or before 2026-01-06 for trade T770892',
    );
    const summary = container.querySelector('.summary-cards')!.textContent;
    expect(summary).toContain('Market value—');
    expect(summary).toContain('Cumulative total P&L—');
    expect(container.querySelector('table.breakdown')).toBeNull();

    const [trade] = [...container.querySelectorAll('table.trades tbody tr')].map(cells);
    expect(trade.slice(6)).toEqual(['—', '—', '—']);
  });

  it('shows the server error when the position cannot be loaded', async () => {
    mockFetch({
      status: 404,
      body: { error: { code: 'not_found', message: "Portfolio 'P002' has no trades in instrument 'I014'." } },
    });

    const { container } = await render(
      <PositionDetail portfolioId="P002" instrumentId="I014" refreshKey={0} onClose={() => {}} />,
    );

    expect(container.textContent).toContain(
      "Could not load the position: Portfolio 'P002' has no trades in instrument 'I014'.",
    );
  });
});
