# Design notes

Technical detail behind the [README](../README.md). The financial rules come from `DATA_CONTRACT.md` (in `data/`), which takes precedence over anything here.

## 1. Architecture

One Axum process serves the JSON API under `/api` and the built React app. SQLite holds the reference data and the imported trades. Nothing is cached: every portfolio and position figure is calculated on request from the trades table and the reference data.

```
backend/src/
  bin/main.rs    executable: logging, configuration, startup, graceful shutdown
  lib.rs         AppState, initialise() (migrate, load reference data), build_router()
  config/        TOML file + INTROPIC_* environment overrides, validation
  manifest/      manifest.json: dataset ID, as_of, valuation calendar, expected row counts
  domain/        validated types: IDs, Currency, TradeQuantity, Price, FxRate, TradeTimestamp, Trade
  reference/     loads and validates the reference CSVs at startup
  import/        trade CSV pipeline: parsing -> validation -> service (transaction) -> report
  portfolio/     decides which trades and market data a portfolio or position needs
  valuation/     pure arithmetic: as-of lookups, exact decimals, totals, rounding
  persistence/   all SQL; returns domain types, never database rows
  api/           routes, extractors, DTOs, HTTP errors, request-ID middleware, health probes
  utils/         Ctrl+C / SIGTERM handling
frontend/src/
  api.ts         typed client; amounts stay decimal strings
  format.ts      string-only display formatting (no floating-point maths)
  components/    UploadPanel, PortfolioOverview, PositionDetail (the drill-down modal)
```

Each request passes through three layers with one job each: `api` handles HTTP (extraction, status codes, JSON shapes) and does no financial arithmetic; `import` and `portfolio` apply the business rules; `persistence` runs SQL. `valuation` has no I/O, so the formulas are unit-tested directly. There are no repository traits or service objects: at this size they would add indirection without benefit.

**Domain types.** A `Price`, `FxRate`, `TradeQuantity` or ID can only be built through a checked constructor, so code that receives one need not re-validate it. IDs in request paths are deserialised through the same constructor, so they are typed from the HTTP boundary onwards. `TradeQuantity` wraps `i64` to match SQLite's INTEGER. `Trade` derives `PartialEq`; because `Decimal` equality is numeric, `a == b` is exactly the contract's "all normalised field values match" duplicate test (`10.0` equals `10.0000`).

## 2. Trade import

`import::import_trades` is independent of HTTP; tests call it directly.

1. **Parse** (`parsing`). The `csv` crate handles quoting. The file must be UTF-8 (a BOM is tolerated), have the exact header and at least one data row; otherwise it is rejected with a file-level message before the database is queried.
2. **Validate every row** (`validation`), collecting all errors rather than stopping at the first: field count; non-empty IDs, kept exactly as written; known portfolio and instrument; exact `YYYY-MM-DDTHH:MM:SSZ` timestamp whose UTC date is in the manifest's `valuation_dates`; `BUY`/`SELL`; quantity a positive integer up to `i64::MAX`; price positive with at most four fractional digits in the text as written. Rows sharing a trade ID are grouped: identical rows are one trade inserted once; otherwise every row of that ID is marked as conflicting. Row numbers count the header as row 1.
3. **Compare with stored trades** (`service`), inside a `BEGIN IMMEDIATE` transaction: a new ID is inserted, an identical stored trade is skipped, a different stored trade marks every incoming row with that ID invalid, naming the fields that differ. Stored trades are never updated or deleted.
4. **Decide.** Any invalid row rolls the transaction back: `inserted = skipped = 0`, `invalid` counts rows with at least one error, `aborted = total - invalid`. Otherwise the inserts commit and `inserted + skipped = total`. `report` turns the outcome into the response; a rejected file returns HTTP 422 with row numbers and reasons.

**Atomicity and concurrency.** One transaction per file makes the import all-or-nothing. `BEGIN IMMEDIATE` takes SQLite's write lock before the duplicate check, so two simultaneous uploads cannot both decide that a trade is new: the second waits (up to the configured busy timeout) and then sees the first one's rows. `INSERT ... ON CONFLICT DO NOTHING` was not used because it cannot tell an exact duplicate from a conflicting one.

**Where each rule is enforced.** The Rust types are the primary check; the SQLite constraints are an independent safeguard against anything that bypasses them.

| Rule | Rust | SQLite |
|---|---|---|
| Non-empty IDs, case-sensitive | ID newtypes | `CHECK (length(id) > 0)`, BINARY collation |
| BUY/SELL | `TradeSide` | `CHECK` |
| Positive integer quantity | `TradeQuantity` | `INTEGER CHECK (quantity > 0)` |
| Positive decimal, ≤ 4 dp (price) / ≤ 6 dp (FX) | `Price`, `FxRate` | text-pattern `CHECK` |
| Timestamp format | `TradeTimestamp` | `GLOB` shape check |
| Valuation date, known portfolio/instrument | `import::validation` | foreign keys |
| Duplicate and conflicting trade IDs | `import::validation`, `import::service` | primary key |

## 3. Valuation

Implemented in `valuation/mod.rs` exactly as DATA_CONTRACT.md section 4 (`s` = +1 for BUY, -1 for SELL):

```
quantity           = Σ s × quantity
net_trade_cost_gbp = Σ s × quantity × execution_price × trade_day_fx
market_value_gbp   = quantity × valuation_price × valuation_fx
total_pnl_gbp      = market_value_gbp - net_trade_cost_gbp
```

- **As of.** The valuation date is the manifest's `as_of`. Only trades whose UTC date is on or before it count, for the figures, the trade count, the trades shown and whether a position exists at all. This boundary is applied once, in `portfolio`, and the valuation relies on it. Trades are processed in `(timestamp, trade_id)` order, as the trade queries return them, which only affects the running quantity.
- **Lookups never look forward.** Market data is loaded only up to `as_of`, and each series is a `BTreeMap` queried for the latest observation on or before the wanted date. A gap carries the previous value forward; a missing value is never replaced by zero or a later value.
- **Trade-day FX.** Each trade converts at the latest rate on or before its own UTC date, never at today's rate.
- **Valuation price and FX** are looked up independently and their dates can differ. An observation dated before `as_of` is flagged `carried_forward` and badged in the UI (in this dataset, I017 uses its 2026-03-26 price with 2026-03-27 FX).
- **Missing market data.** An open position with no valuation price or FX rate, or any position missing a trade-day rate, is returned as `unavailable` with a reason. The portfolio totals are then also unavailable, with the reasons, rather than a misleading partial sum. Other positions are still shown.
- **Closed positions.** A position whose trades net to zero has market value zero whatever the current price, so it is valued without a valuation price or FX rate; `valuation_price` and `valuation_fx` are `null` only if none exists, and are still reported when they do. Its cost still uses every trade-day FX rate, so a missing historical rate makes it unavailable. Closed positions stay listed and their P&L counts towards the portfolio total.

**Decimal precision.** Prices and rates are stored as decimal text and parsed into `rust_decimal::Decimal`; no binary floating point is used for money anywhere, including the browser, which receives decimal strings. `rust_decimal`'s checked operations fail only on integer overflow and silently round results needing more than 28 significant digits, so the valuation multiplies and adds the `i128` mantissas itself and accepts a result only if it fits exactly. Otherwise the position is unavailable (`ValuationError::Overflow`), never rounded or wrapped. Quantity sums use `i64::checked_add`.

**Rounding.** Nothing is rounded until output. Money is shown to 2 dp, rounding half up (half away from zero for negatives, as Python's `ROUND_HALF_UP`). Portfolio totals sum unrounded position values and round once, so they can differ from the sum of rounded rows by a few pennies, as the contract allows. `*_exact` API fields carry the unrounded values.

## 4. Database

| Table | Primary key | Notes |
|---|---|---|
| `portfolios` | `portfolio_id` | `base_currency` must be `GBP` |
| `instruments` | `instrument_id` | currency GBP, USD or EUR |
| `prices` | `(instrument_id, date)` | FK to instruments; ≤ 4 dp; `WITHOUT ROWID` |
| `fx_rates` | `(currency, date)` | ≤ 6 dp; `WITHOUT ROWID` |
| `trades` | `trade_id` | FKs to portfolios and instruments; side, quantity, price and timestamp checks; `imported_at` audit column |

- **Migrations** run at startup. `0001_initial_schema.sql` creates the STRICT tables and the index. Its decimal `CHECK`s use only text patterns (`GLOB`, `instr`, `length`), never SQLite's lenient numeric casts: a value has digits and at most one `.`, starts and ends with a digit, contains a non-zero digit and respects the fractional-digit limit.
- **Connection policy** is fixed: foreign keys on, WAL journal, `synchronous = FULL`.
- **Index.** `idx_trades_position (portfolio_id, instrument_id, timestamp_utc, trade_id)` serves both trade queries (a whole portfolio, or one position) without a sort step; tests check the query plans. The composite primary keys cover the as-of price and FX lookups.
- **Reference data** is upserted in one transaction at each startup, so restarts never duplicate rows. Rows removed from a reference file are not deleted, since stored trades may reference them. The manifest's row counts are compared with the CSVs, and a mismatch is logged as a warning.

## 5. Configuration

A required TOML file, overlaid by `INTROPIC_*` environment variables, deserialised into typed fields and validated before startup continues (`config/`, `persistence/config.rs`, `api/config.rs`). `backend/config.toml` is for local development; `backend/config.docker.toml` becomes `/app/config.toml` in the image. Unknown keys and lenient conversions (`5.5` for an integer, `"yes"` for a boolean) are errors, and no required setting has a silent default.

| Setting | Environment override | Default |
|---|---|---|
| `manifest_path`, `reference_data_dir`, `bind_addr` | `INTROPIC_MANIFEST_PATH`, `INTROPIC_REFERENCE_DATA_DIR`, `INTROPIC_BIND_ADDR` | required |
| `static_dir` (built frontend; unset = API only) | `INTROPIC_STATIC_DIR` | unset; must contain `index.html` if set |
| `[persistence] database_path` | `INTROPIC_PERSISTENCE__DATABASE_PATH` | required |
| `[persistence] max_connections`, `busy_timeout_ms`, `acquire_timeout_ms`, `create_if_missing` | `INTROPIC_PERSISTENCE__*` | 5, 10000, 30000, true |
| `[api] max_upload_bytes` (whole multipart request) | `INTROPIC_API__MAX_UPLOAD_BYTES` | 10 MiB |

`RUST_LOG` sets the log filter (default `info,tower_http=info,sqlx=warn`, also in the Docker image). Log colours are used only when writing to a terminal.

## 6. HTTP API, errors and observability

All amounts are decimal strings. `*_gbp` fields are rounded for display; `*_exact` fields are unrounded.

| Endpoint | Purpose |
|---|---|
| `GET /api/live`, `GET /api/ready` | Liveness (no dependencies); readiness (`SELECT 1` with a 2 s timeout, 503 if the database is unavailable) |
| `GET /api/portfolios` | Portfolios with trade counts, `as_of` and `dataset_id` |
| `GET /api/portfolios/{id}/positions` | Status, totals, open/closed counts and one summary per position, with the price and FX used, their dates and `carried_forward` flags |
| `GET /api/portfolios/{id}/positions/{instrument}` | The summary plus a `breakdown` of every intermediate figure and the underlying trades with trade-day FX, signed GBP cost and running quantity |
| `POST /api/trades/import` | Multipart field `file` (exactly one). 200 on success, 422 on rejection, both with `outcome`, the row counts and `errors: [{row, trade_id, messages}]`; 400 for a malformed request, 413 above the upload limit |

**Errors.** Each subsystem has typed `thiserror` errors in its own `error.rs`; callers match on variants, never message text. Expected outcomes are results, not errors: a rejected file is an `ImportReport` (422), and an unvaluable position carries its `ValuationError` inside a 200 response. Infrastructure failures convert into `AppError` (no HTTP knowledge) and then `ApiError`, the only type that becomes an HTTP error response: `{"error": {"code", "message"}}` with 400, 404, 405 (JSON, with `Allow`), 413, 500 or 503. Internal errors are logged in full and returned as a generic message, so SQL and file paths never reach the client. Unknown `/api` paths return JSON 404 rather than the app's `index.html`.

**Observability.** Every response carries an `x-request-id` header (a new UUID v4, or the client's own value echoed back), and every log line for a request includes it, with the method and path, through a tracing span. On Ctrl+C or SIGTERM (`docker stop`) the server stops accepting connections, finishes in-flight requests and exits.

## 7. Testing and independent verification

- **Unit tests** sit beside the code: domain parsing, CSV parsing, row validation, import classification and report wording, the valuation formulas (including the contract's worked examples, carry-forward, closed positions and exact arithmetic), configuration and DTO JSON shapes.
- **Integration tests** (`backend/tests/`) use a fresh temporary SQLite file each: the fixture sequence with exact counts and an unchanged trades table after a rejected upload, restart persistence and concurrent uploads, database constraints by raw SQL, reference loading, as-of consistency between summaries and details, closed positions with missing market data, HTTP status codes and error bodies, request IDs, upload limits and shutdown.
- **Reconciliation** (`valuation_reconciliation.rs`) re-derives every position and total from the per-trade contributions and checks the contract's invariants.
- **Golden values** (`golden_values.rs`) compare the API's figures with values calculated independently by `tools/recompute.py`. The script uses only Python's standard library (`csv`, `decimal` with inexact operations trapped, `datetime`, `pathlib`) and shares nothing with the backend: it replays the five upload fixtures in the contract's order (initial, repeat, incremental, invalid, corrected) under the contract's upload rules, selects prices and rates with its own on-or-before lookup, and values each position. It reproduces the expected import counts (600/0, 0/600, 60/0, rejected with 8 invalid and 2 aborted rows, 10/0) and its totals and position figures match the API exactly. The golden constants are specific to this dataset; the application does not depend on them.
- **Frontend tests** (Vitest) cover the formatting helpers and the positions-table row: carried-forward price and FX badges, a closed position without observations, and an unavailable position.

## 8. Trade-offs

- **SQLite** fits a single-user tool with a few thousand rows: durable, zero setup, a file on a Docker volume, with real transactions, foreign keys and `CHECK` constraints. It serialises writers, which the import relies on.
- **Calculating on demand** keeps the trades table the single source of truth with nothing to invalidate; a portfolio request takes milliseconds on this dataset. A much larger dataset would need stored positions or incremental valuation.
- **Unavailable rather than partial totals.** A portfolio total that silently omitted a position would be wrong in a way the user could not see.
- **Strict input interpretation.** Precision is checked on the text as written (`10.00000` is rejected as a price, though duplicates compare numerically); quantities must be plain digits (`+5`, `1.0` and `1e3` are rejected); an empty or header-only file is rejected rather than treated as a successful no-op; IDs are compared exactly, including case and whitespace.
- **One container** serving both API and frontend keeps startup to one command and avoids CORS configuration; in development, Vite proxies `/api` to the backend instead.
