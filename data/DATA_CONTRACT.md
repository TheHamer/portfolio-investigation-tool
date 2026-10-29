# Data contract: Portfolio Investigation Tool

Version: **1.0**. Audience: candidates and reviewers. These are deliberately simplified exercise conventions, not a specification for production accounting.

This contract is shared with every candidate. Assessors may use different data values, but must not introduce unpublished business rules. API routes and response shapes are your choice; expose the required information clearly and document it.

## 1. Files supplied

The standard dataset contains three GBP-base portfolios, 24 fictional instruments, 60 valuation dates, 600 initial trades and 60 additional trades. Exact dates, counts, file hashes and `dataset_id` are in `manifest.json`. The generated date range is fixed, not relative to today's date.

```text
reference/
  portfolios.csv
  instruments.csv
  prices.csv
  fx_rates.csv
uploads/
  trades_initial.csv
  trades_repeat.csv
  trades_incremental.csv
  trades_invalid.csv
  trades_corrected.csv
manifest.json
DATA_CONTRACT.md
```

Reference files are clean and should load automatically or through a documented initialization step invoked by Compose. Upload files must enter through the React upload flow. Reference records must not be duplicated on restart. Test upload order: initial, repeat, incremental, invalid, corrected. Repeat is byte-identical to initial. The invalid file contains a mixture of valid and invalid rows; corrected repairs those rows and uses a fresh ID for any conflicting record.

CSV encoding is UTF-8, comma-delimited, with a header. Quoted CSV fields may contain commas. Parse CSV properly rather than splitting text on commas. The supplied files use the exact headers below, no blank records and no embedded line breaks. Report row numbers with the header as row 1. Extra formats, arbitrary spreadsheets and unknown encodings are outside scope.

## 2. Schemas

| File | Header, in order | Meaning |
|---|---|---|
| portfolios.csv | portfolio_id,name,base_currency | Stable identifier; display name; always GBP. |
| instruments.csv | instrument_id,ticker,name,currency,sector | Currency is GBP, USD or EUR; ticker is a display label, not a primary key. |
| prices.csv | instrument_id,date,close_price | Positive local-currency price per share; one record per instrument/date. |
| fx_rates.csv | date,currency,gbp_per_unit | Positive GBP value of one unit of the currency; GBP is 1.000000. |
| Trade uploads | trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price | Globally unique trade ID; UTC execution timestamp; BUY or SELL; positive integer shares; positive execution price in the instrument's currency. |

Dates are `YYYY-MM-DD`. Trade timestamps are exactly `YYYY-MM-DDTHH:MM:SSZ`. Each trade date must appear in `manifest.json`'s ordered `valuation_dates` array. Do not infer a stock-exchange holiday calendar. Trade price and quantity fields are decimal text and integer text respectively; prices have at most four fractional digits and FX rates at most six. Reject non-finite numbers. Use exact decimal arithmetic or an equivalently accurate approach; do not round every intermediate cash flow to pennies.

IDs are nonempty strings and comparisons are case-sensitive. The supplied data does not contain leading or trailing whitespace. Store timestamps as UTC instants. Reference IDs, dates and numeric fields are unique/valid except for deliberately invalid upload rows.

## 3. Upload semantics

**Atomic per file.** Validate before committing trade changes. If any record is invalid, reject the entire upload: existing trades are unchanged and no new trade is inserted. Reporting a failed import attempt separately is permitted. No background queue is required.

**Validation.** Check required headers and fields, known portfolio/instrument IDs, supported UTC timestamp and valuation date, side, positive integer quantity and positive finite price. Enforce globally unique trade IDs. Uploads must not be interpreted as corrections to existing trades.

**Idempotency.** An incoming row whose `trade_id` and all other normalized field values match an existing row is an exact duplicate and is skipped. The same applies to an exact repeated row within one file: insert it once at most. Compare parsed values, so `10.0` and `10.0000` are equal. If one ID has conflicting values, reject the whole upload. For conflicting variants within a file, mark every row using that conflicting ID as invalid. For a conflict with a stored row, mark every incoming row of that ID as invalid. Existing data is never overwritten.

**Visible result.** On success, show how many rows were inserted or skipped. On rejection, show the offending row numbers and actionable reasons, and make clear that nothing was imported. Suggested machine-readable result fields are `outcome`, `total_rows`, `inserted_rows`, `skipped_rows`, `invalid_rows`, `aborted_rows` and `errors`; equivalent shapes are fine.

For row-validating files, counts partition all data rows: on success, inserted + skipped = total; on rejection, inserted = skipped = 0 and invalid + aborted = total. Invalid means a row with an error; aborted means a row not itself invalid that was not imported because the file failed. Count each invalid row once, even if it has several errors. A file-level error such as a missing header may instead have a file-level diagnostic without row counts.

Trades may be out of timestamp order in a file. Calculate from execution timestamps, not upload order. For any optional running-position view, order by `(timestamp, trade_id)`. Valid fixtures never create a short position, including after incremental imports. Detecting economic oversells is outside the core exercise; detecting malformed quantities is not.

## 4. Financial conventions

Only long-only equities, BUY/SELL transactions, and GBP/USD/EUR are included. Opening holdings and historical cash flows are zero. There are no fees, dividends, corporate actions, financing, settlement delays, taxes, deposits or withdrawals to model. Do not implement a cash ledger or a percentage return calculation.

All summaries use the final valuation date `as_of` in the manifest. Include trades on or before that UTC date. The optional time-series view uses each supplied valuation date in turn.

For instrument `i` and valuation date `t`, let `s` be +1 for a BUY and -1 for a SELL:

```text
quantity_i(t) = sum(s * trade.quantity for trades through t)

net_trade_cost_gbp_i(t) = sum(
  s * trade.quantity * trade.price * trade_day_fx
  for trades through t
)

market_value_gbp_i(t) = quantity_i(t) * valuation_price * valuation_fx

total_pnl_gbp_i(t) = market_value_gbp_i(t) - net_trade_cost_gbp_i(t)
```

`trade_day_fx` is the latest rate dated on or before the trade's UTC date. `valuation_price` and `valuation_fx` are independently the latest price and FX rate dated on or before `t`. Never look forward. A missing same-day price must not cause use of a future price or a zero price. Fixtures always supply an initial price and rate before the first relevant trade. Missing older coverage in other data should produce a clear error or unavailable result, not a fabricated value.

Include the actual `price_date` and `fx_date` used. A carried-forward price should be visible as such in the UI. FX remains the current valuation-date rate even when the price is older. Historical purchases and sales are converted at their own trade-day FX rate, never retrospectively at today's rate.

Sum unrounded instrument values for portfolio totals, then round for display/comparison. Monetary output is in GBP, two decimal places with round-half-up; quantity is an integer. A numeric API representation or a decimal string is acceptable if it preserves this meaning. A rounded position table may differ from a rounded portfolio total by a few pennies because totals are calculated before rounding.

A closed position has quantity and market value zero, but may have nonzero P&L. Include it in portfolio P&L and make it accessible in the UI. These values are **holdings market value** and **cumulative trading P&L**, not portfolio cash balance, NAV, an investment recommendation or time-weighted return.

## 5. Small worked examples

**GBP trades.** Buy 100 at GBP 10; later sell 40 at GBP 15; later buy 20 at GBP 12. At an ending price of GBP 16:

```text
quantity = 100 - 40 + 20 = 80
net trade cost = 1000 - 600 + 240 = GBP 640
market value = 80 * 16 = GBP 1,280
cumulative total P&L = 1280 - 640 = GBP 640
```

**FX matters.** Buy 100 shares at USD 10 when one USD is GBP 0.80. At a later price of USD 12 and FX of GBP 0.75 per USD, cost is GBP 800, market value is GBP 900 and cumulative P&L is GBP 100. Revaluing the original purchase at 0.75 would give the wrong result.

**Closed position.** Buy 10 at GBP 10, then sell all 10 at GBP 12. Market value is zero and cumulative P&L is GBP 20.

## 6. Optional daily P&L

This is not required and must not replace the core journey. If implemented:

```text
net_trade_cost_today = signed GBP cost of today's buys and sells

daily_pnl(t) = market_value(t) - market_value(previous_valuation_date)
               - net_trade_cost_today
            = cumulative_pnl(t) - cumulative_pnl(previous_valuation_date)
```

Use zero prior market value and zero prior P&L on the first valuation date. Per-position contributions use the same formula and sum to portfolio daily P&L before rounding. A change in holdings market value alone is not P&L: a purchase can increase holdings value without generating a profit. There is no requirement to split daily P&L into price versus FX components, or total P&L into realised versus unrealised amounts.
