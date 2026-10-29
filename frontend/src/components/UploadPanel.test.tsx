// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ImportResult } from '../api';
import { interact, mockFetch, render } from '../test-utils';
import { UploadPanel } from './UploadPanel';

const SUCCESS: ImportResult = {
  file_name: 'trades_repeat.csv',
  outcome: 'success',
  message: 'Import succeeded: 0 trades inserted, 600 duplicate rows skipped.',
  total_rows: 600,
  inserted_rows: 0,
  skipped_rows: 600,
  invalid_rows: 0,
  aborted_rows: 0,
  errors: [],
};

const REJECTED: ImportResult = {
  file_name: 'trades_invalid.csv',
  outcome: 'rejected',
  message: 'File rejected: 2 of 3 rows are invalid. No trades were imported and stored trades are unchanged.',
  total_rows: 3,
  inserted_rows: 0,
  skipped_rows: 0,
  invalid_rows: 2,
  aborted_rows: 1,
  errors: [
    { row: 3, trade_id: 'T572470', messages: ["portfolio_id 'P999' does not exist"] },
    {
      row: 4,
      trade_id: 'T978178',
      messages: ["side 'HOLD' is invalid (expected BUY or SELL)", "quantity '0' must be greater than zero"],
    },
  ],
};

/** Renders the panel, chooses a file and clicks Upload. */
async function upload(onImported: () => void) {
  const { container, unmount } = await render(<UploadPanel onImported={onImported} />);
  const input = container.querySelector<HTMLInputElement>('input[type="file"]')!;
  const file = new File(['trade_id\n'], 'trades.csv', { type: 'text/csv' });

  await interact(() => {
    Object.defineProperty(input, 'files', { value: [file], configurable: true });
    input.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await interact(() => container.querySelector('button')!.click());

  return { text: container.textContent ?? '', container, unmount };
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = '';
});

describe('UploadPanel', () => {
  it('shows inserted and skipped counts and refreshes the figures after a successful import', async () => {
    mockFetch({ status: 200, body: SUCCESS });
    const onImported = vi.fn();

    const { text } = await upload(onImported);

    expect(text).toContain('Import succeeded (trades_repeat.csv)');
    expect(text).toContain('Inserted0');
    expect(text).toContain('Skipped (duplicates)600');
    expect(onImported).toHaveBeenCalledTimes(1);
  });

  it('lists each invalid row with all of its reasons and does not refresh after a rejection', async () => {
    mockFetch({ status: 422, body: REJECTED });
    const onImported = vi.fn();

    const { text, container } = await upload(onImported);

    expect(text).toContain('File rejected — nothing was imported');
    expect(text).toContain('Invalid rows2');
    expect(text).toContain('Valid rows not imported1');
    const rows = [...container.querySelectorAll('tbody tr')].map((tr) => tr.textContent);
    expect(rows).toEqual([
      "3T572470portfolio_id 'P999' does not exist",
      "4T978178side 'HOLD' is invalid (expected BUY or SELL)quantity '0' must be greater than zero",
    ]);
    expect(onImported).not.toHaveBeenCalled();
  });

  it("shows the server's message for an operational failure and does not refresh", async () => {
    mockFetch({
      status: 500,
      body: { error: { code: 'internal_error', message: 'An internal error occurred. Please try again.' } },
    });
    const onImported = vi.fn();

    const { text } = await upload(onImported);

    expect(text).toContain('Upload failed. An internal error occurred. Please try again. Nothing was imported.');
    expect(onImported).not.toHaveBeenCalled();
  });

  it('tells the user when the server cannot be reached, without claiming nothing was imported', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new TypeError('fetch failed')));
    const onImported = vi.fn();

    const { text } = await upload(onImported);

    expect(text).toContain('Upload could not be confirmed. Could not reach the server. Is the backend running?');
    expect(text).toContain('Previously imported trades will be skipped.');
    expect(text).not.toContain('Nothing was imported');
    expect(onImported).not.toHaveBeenCalled();
  });

  it('does not claim nothing was imported when a successful response cannot be read', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('<html>proxy</html>', { status: 200 })));
    const onImported = vi.fn();

    const { text } = await upload(onImported);

    expect(text).toContain('Upload could not be confirmed.');
    expect(text).toContain('The server returned an unreadable response (status 200).');
    expect(text).not.toContain('Nothing was imported');
    expect(onImported).not.toHaveBeenCalled();
  });

  it('still reports a rejection whose body cannot be read as nothing imported', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('<html>proxy</html>', { status: 422 })));

    const { text } = await upload(vi.fn());

    expect(text).toContain(
      'Upload failed. The server returned an unreadable response (status 422). Nothing was imported.',
    );
    expect(text).not.toContain('could not be confirmed');
  });
});
