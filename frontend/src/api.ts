// Typed client for the backend JSON API.
//
// Monetary and price values arrive as decimal strings and are only ever
// formatted for display, never converted to JavaScript numbers, so no
// financial arithmetic happens in the browser.

export type Status = 'ok' | 'unavailable';

export interface Portfolio {
  portfolio_id: string;
  name: string;
  base_currency: string;
}

export interface PortfolioListResponse {
  as_of: string;
  dataset_id: string;
  portfolios: (Portfolio & { trade_count: number })[];
}

export interface PriceUsed {
  price: string;
  currency: string;
  date: string;
  carried_forward: boolean;
}

export interface FxUsed {
  gbp_per_unit: string;
  currency: string;
  date: string;
  carried_forward: boolean;
}

export interface PositionSummary {
  instrument_id: string;
  ticker: string;
  name: string;
  currency: string;
  sector: string;
  trade_count: number;
  /** null only if summing the signed trade quantities overflows on the server. */
  quantity: number | null;
  is_closed: boolean;
  status: Status;
  unavailable_reason: string | null;
  net_trade_cost_gbp: string | null;
  market_value_gbp: string | null;
  total_pnl_gbp: string | null;
  /** null when the position is unavailable, or closed with no price available
   * (a closed position is worth zero, so it does not need one). */
  valuation_price: PriceUsed | null;
  /** null on the same terms as valuation_price. */
  valuation_fx: FxUsed | null;
}

export interface Totals {
  market_value_gbp: string;
  net_trade_cost_gbp: string;
  total_pnl_gbp: string;
  market_value_gbp_exact: string;
  net_trade_cost_gbp_exact: string;
  total_pnl_gbp_exact: string;
}

export interface PortfolioPositionsResponse {
  portfolio: Portfolio;
  as_of: string;
  status: Status;
  unavailable_reason: string | null;
  totals: Totals | null;
  open_position_count: number;
  closed_position_count: number;
  positions: PositionSummary[];
}

export interface TradeLine {
  trade_id: string;
  timestamp: string;
  trade_date: string;
  side: 'BUY' | 'SELL';
  quantity: number;
  signed_quantity: number;
  price: string;
  currency: string;
  /** null if quantity × price exceeds the server's exact numeric range. */
  signed_local_amount: string | null;
  fx: FxUsed | null;
  signed_cost_gbp: string | null;
  signed_cost_gbp_exact: string | null;
  running_quantity: number | null;
}

export interface Breakdown {
  quantity: number;
  /** null only for a closed position with no price or rate available. */
  valuation_price: PriceUsed | null;
  valuation_fx: FxUsed | null;
  market_value_local_exact: string;
  market_value_gbp_exact: string;
  net_trade_cost_gbp_exact: string;
  total_pnl_gbp_exact: string;
}

export interface PositionDetailResponse {
  portfolio: Portfolio;
  as_of: string;
  position: PositionSummary;
  breakdown: Breakdown | null;
  trades: TradeLine[];
}

export interface RowError {
  row: number | null;
  trade_id: string | null;
  messages: string[];
}

export interface ImportResult {
  file_name: string | null;
  outcome: 'success' | 'rejected';
  message: string;
  total_rows: number | null;
  inserted_rows: number | null;
  skipped_rows: number | null;
  invalid_rows: number | null;
  aborted_rows: number | null;
  errors: RowError[];
}

export class ApiError extends Error {
  /** True when the request may have succeeded on the server even though the
   * client did not receive a usable response. */
  constructor(
    message: string,
    public readonly status: number,
    public readonly outcomeUnknown = false,
  ) {
    super(message);
  }
}

async function errorFrom(response: Response): Promise<ApiError> {
  let message = `Request failed with status ${response.status}.`;

  try {
    const body = await response.json();
    if (body?.error?.message) message = body.error.message;
  } catch {
    // Not JSON (for example, a proxy error page); keep the generic message.
  }

  return new ApiError(message, response.status);
}

/** The body of a response that should be JSON. A body that is not (for
 * example a proxy's HTML page) becomes an ApiError rather than a raw
 * SyntaxError that would reach the user. */
async function readJson<T>(response: Response): Promise<T> {
  try {
    return (await response.json()) as T;
  } catch {
    throw new ApiError(`The server returned an unreadable response (status ${response.status}).`, response.status);
  }
}

async function getJson<T>(url: string): Promise<T> {
  let response: Response;
  try {
    response = await fetch(url);
  } catch {
    throw new ApiError('Could not reach the server. Is the backend running?', 0);
  }

  if (!response.ok) throw await errorFrom(response);
  return readJson<T>(response);
}

export const fetchPortfolios = () => getJson<PortfolioListResponse>('/api/portfolios');

export const fetchPositions = (portfolioId: string) =>
  getJson<PortfolioPositionsResponse>(`/api/portfolios/${encodeURIComponent(portfolioId)}/positions`);

export const fetchPositionDetail = (portfolioId: string, instrumentId: string) =>
  getJson<PositionDetailResponse>(
    `/api/portfolios/${encodeURIComponent(portfolioId)}/positions/${encodeURIComponent(instrumentId)}`,
  );

/** Uploads a CSV. Both a successful import (200) and a rejected file (422)
 * return an ImportResult; anything else is thrown as an ApiError. A failure
 * with no usable response on a 200 is marked `outcomeUnknown`, because the
 * server may already have committed the import. */
export async function uploadTrades(file: File): Promise<ImportResult> {
  const form = new FormData();
  form.append('file', file);

  let response: Response;
  try {
    response = await fetch('/api/trades/import', { method: 'POST', body: form });
  } catch {
    throw new ApiError('Could not reach the server. Is the backend running?', 0, true);
  }

  if (response.ok || response.status === 422) {
    try {
      return (await response.json()) as ImportResult;
    } catch {
      throw new ApiError(
        `The server returned an unreadable response (status ${response.status}).`,
        response.status,
        response.ok,
      );
    }
  }

  throw await errorFrom(response);
}
