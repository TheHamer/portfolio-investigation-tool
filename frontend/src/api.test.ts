import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, ImportResult, fetchPositions, uploadTrades } from './api';

const FILE = new File(['trade_id,portfolio_id\n'], 'trades.csv', { type: 'text/csv' });

const SUCCESS: ImportResult = {
  file_name: 'trades.csv',
  outcome: 'success',
  message: 'Import succeeded: 600 trades inserted, 0 duplicate rows skipped.',
  total_rows: 600,
  inserted_rows: 600,
  skipped_rows: 0,
  invalid_rows: 0,
  aborted_rows: 0,
  errors: [],
};

const REJECTED: ImportResult = {
  file_name: 'trades_invalid.csv',
  outcome: 'rejected',
  message: 'File rejected: 1 of 2 rows are invalid. No trades were imported and stored trades are unchanged.',
  total_rows: 2,
  inserted_rows: 0,
  skipped_rows: 0,
  invalid_rows: 1,
  aborted_rows: 1,
  errors: [{ row: 3, trade_id: 'T2', messages: ["portfolio_id 'P999' does not exist"] }],
};

function respondWith(body: string, status: number, contentType = 'application/json') {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(new Response(body, { status, headers: { 'Content-Type': contentType } })),
  );
}

async function rejectionOf(promise: Promise<unknown>): Promise<ApiError> {
  const error = await promise.then(
    () => {
      throw new Error('expected the request to fail');
    },
    (e: unknown) => e,
  );
  expect(error).toBeInstanceOf(ApiError);
  return error as ApiError;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('uploadTrades', () => {
  it('posts the file as the multipart field "file"', async () => {
    respondWith(JSON.stringify(SUCCESS), 200);

    await uploadTrades(FILE);

    const [url, init] = vi.mocked(fetch).mock.calls[0];
    expect(url).toBe('/api/trades/import');
    expect(init?.method).toBe('POST');
    expect((init?.body as FormData).get('file')).toBeInstanceOf(File);
  });

  it('returns the import report of a successful upload', async () => {
    respondWith(JSON.stringify(SUCCESS), 200);

    const result = await uploadTrades(FILE);

    expect(result).toEqual(SUCCESS);
  });

  it('returns a rejected file (422) as a report, not as an error', async () => {
    respondWith(JSON.stringify(REJECTED), 422);

    const result = await uploadTrades(FILE);

    expect(result.outcome).toBe('rejected');
    expect(result.errors[0].row).toBe(3);
  });

  it("throws other failures as an ApiError carrying the server's message and status", async () => {
    respondWith(
      JSON.stringify({
        error: { code: 'payload_too_large', message: 'The upload request exceeds the maximum size of 10 MiB.' },
      }),
      413,
    );

    const error = await rejectionOf(uploadTrades(FILE));

    expect(error.status).toBe(413);
    expect(error.message).toBe('The upload request exceeds the maximum size of 10 MiB.');
  });

  it('falls back to a generic message when an error response is not JSON', async () => {
    respondWith('<html>Bad gateway</html>', 502, 'text/html');

    const error = await rejectionOf(uploadTrades(FILE));

    expect(error.status).toBe(502);
    expect(error.message).toBe('Request failed with status 502.');
  });

  it('reports an unreadable success body as an ApiError, not a SyntaxError', async () => {
    respondWith('<html>not json</html>', 200, 'text/html');

    const error = await rejectionOf(uploadTrades(FILE));

    expect(error.status).toBe(200);
    expect(error.message).toBe('The server returned an unreadable response (status 200).');
    expect(error.outcomeUnknown).toBe(true);
  });

  it('reports an unreachable server with status 0 and an unknown outcome', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('fetch failed')));

    const error = await rejectionOf(uploadTrades(FILE));

    expect(error.status).toBe(0);
    expect(error.message).toBe('Could not reach the server. Is the backend running?');
    expect(error.outcomeUnknown).toBe(true);
  });

  it('marks a definite server error as a known outcome', async () => {
    respondWith(JSON.stringify({ error: { code: 'internal_error', message: 'An internal error occurred.' } }), 500);

    const error = await rejectionOf(uploadTrades(FILE));

    expect(error.outcomeUnknown).toBe(false);
  });
});

describe('fetchPositions', () => {
  it('encodes the portfolio ID into the path', async () => {
    respondWith('{}', 200);

    await fetchPositions('P 1/2');

    expect(vi.mocked(fetch).mock.calls[0][0]).toBe('/api/portfolios/P%201%2F2/positions');
  });

  it("throws a not-found response as an ApiError with the server's message", async () => {
    respondWith(JSON.stringify({ error: { code: 'not_found', message: "Portfolio 'P999' does not exist." } }), 404);

    const error = await rejectionOf(fetchPositions('P999'));

    expect(error.status).toBe(404);
    expect(error.message).toBe("Portfolio 'P999' does not exist.");
  });

  it('reports an unreadable body as an ApiError', async () => {
    respondWith('not json', 200, 'text/plain');

    const error = await rejectionOf(fetchPositions('P001'));

    expect(error.message).toBe('The server returned an unreadable response (status 200).');
  });
});
