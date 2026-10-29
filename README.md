# Portfolio Investigation Tool

**Dataset ID: `ds-f77160d14d37a72b`** · valuation date (`as_of`) 2026-03-27

A web application for uploading trade CSVs, seeing each portfolio's holdings market value and cumulative P&L in GBP, and tracing every figure back to the trades, prices and FX rates behind it.

- **Backend:** Rust (Axum, SQLx, SQLite, `rust_decimal`).
- **Frontend:** React + TypeScript (Vite), served by the backend from one Docker image.

Technical detail is in [docs/DESIGN.md](docs/DESIGN.md); AI assistance is described in [AI_USAGE.md](AI_USAGE.md).

## Running the application

**Prerequisites:** Docker with Compose v2 (Docker Desktop on Windows or macOS). No accounts, keys or manual data preparation are needed. The smoke-test workflow below needs Docker Compose 2.24.4 or later, for the `!override` tag in `tools/compose.smoke.yml`.

```bash
docker compose up --build
```

Then open **http://localhost:8080**.

At startup the container applies the database migrations and loads the reference data (portfolios, instruments, prices and FX rates) from `data/`. Loading is idempotent, so restarts never duplicate it. Trades are never imported automatically. They are stored in the `portfolio-data` volume and survive `docker compose down` / `up`; `docker compose down -v` starts again from an empty database.

## Trying the upload journey

Upload the files in `data/uploads/` through the **Upload trades** panel, in the contract's order:

| # | File | Expected result |
|---|---|---|
| 1 | `trades_initial.csv` | Imported: 600 inserted |
| 2 | `trades_repeat.csv` | Imported: 0 inserted, 600 skipped as exact duplicates |
| 3 | `trades_incremental.csv` | Imported: 60 inserted |
| 4 | `trades_invalid.csv` | **Rejected**: rows 3, 5–11 invalid, 2 valid rows aborted, nothing stored |
| 5 | `trades_corrected.csv` | Imported: 10 inserted |

The invalid file's reasons are an unknown portfolio (row 3), a date outside the valuation calendar (5), a trade ID already stored with different values (6), side `HOLD` (7), quantity 0 (8), a malformed timestamp (9), an unknown instrument (10) and a negative price (11).

Then choose a portfolio and click **Investigate** on a position to see its valuation breakdown and underlying trades. Two useful examples in **P002**:

- **ZZCAR14 (I014)** is closed: quantity 0 and market value £0.00, but £24,686.16 of P&L that counts towards the total.
- **ZZBRA17 (I017)** has no price on 2026-03-27, so its 2026-03-26 price is used and flagged as carried forward.

After the five uploads the totals are:

| Portfolio | Market value | Net trade cost | Total P&L |
|---|---|---|---|
| P001 | £2,253,433.83 | £2,168,668.78 | £84,765.04 |
| P002 | £1,380,876.83 | £1,298,218.54 | £82,658.29 |
| P003 | £1,514,895.49 | £1,462,413.34 | £52,482.16 |

These figures are reproduced independently by `tools/recompute.py` (below).

## Running the tests

**Locally**, with Rust 1.88 or later (the minimum declared in `Cargo.toml`; the Docker image builds with 1.90) and Node.js 22 or later:

```bash
cd backend
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo test --doc

cd ../frontend
npm ci
npm run format:check   # Prettier
npm run lint           # ESLint
npm test               # Vitest unit tests
npm run build          # TypeScript type-check and production build
```

**With Docker only**, from the repository root (in PowerShell, write `${PWD}` instead of `$PWD`). Build output goes to `backend/target` and `frontend/node_modules`, both ignored by Git:

```bash
docker run --rm -v "$PWD:/src" -w /src/backend rust:1.90-bookworm cargo test --all-targets
docker run --rm -v "$PWD:/src" -w /src/frontend node:22.12.0-bookworm-slim sh -c "npm ci && npm test"
```

**Independent recomputation** with Python 3.10 or later and no third-party packages:

```bash
python tools/recompute.py --position P002/I014 --position P002/I017
```

The script shares no code with the application. It replays the five fixtures under the contract's upload rules, values every position with Python's `decimal`, and prints the import outcomes, the portfolio totals and each position. `backend/tests/golden_values.rs` checks the API's figures against its output.

**End-to-end smoke test** of the Docker image, as its own Compose project on port 18080 with its own volume, so an existing database is never touched:

```bash
SMOKE="docker compose -p pit-smoke -f docker-compose.yml -f tools/compose.smoke.yml"
$SMOKE up -d --build
python tools/smoke_test.py --base-url http://localhost:18080
$SMOKE restart
python tools/smoke_test.py --base-url http://localhost:18080 --after-restart
$SMOKE down -v    # removes the smoke project's container and volume only
```

`tools/smoke_test.py` drives the journey over HTTP: the page and its bundle are served, the five uploads give the expected reports, the rejected file changes nothing, and every portfolio total and every position (figures, valuation price and FX with their dates, each trade's FX) matches `tools/recompute.py`. After the restart it checks that all trades and figures persisted and that a re-upload inserts nothing. It does not drive a browser; in the browser, check that the upload panel shows the counts and the rejected rows with their reasons, that **Investigate** opens the breakdown for an open position (P002 ZZBRA17, with its carried-forward price badge) and a closed one (P002 ZZCAR14), and that the trade table's FX dates match.

Backend integration tests use a temporary SQLite file each, so they never touch a running application's data.

**Development mode:** `cargo run` in `backend/` starts the API on port 8080 (using `backend/config.toml`), and `npm run dev` in `frontend/` serves the UI on http://localhost:5173, proxying `/api` to it.

## Main design choices

- **Calculated on demand.** Positions and totals are computed from the stored trades and reference data on each request; nothing is cached or precomputed.
- **Exact decimals.** Prices, rates and amounts are never binary floating point, in the database, the backend or the browser. Arithmetic is exact (a result that cannot be represented exactly is reported, not rounded), and values are rounded half up to pennies only for display, with totals summed before rounding.
- **Atomic, idempotent imports.** Each file is validated in full and imported in one SQLite `BEGIN IMMEDIATE` transaction: any invalid row rejects the whole file and reports every problem with its row number; exact duplicates are skipped; conflicting trade IDs are rejected; concurrent uploads cannot insert a trade twice.
- **Contract-faithful valuation.** Each trade is converted at its own trade-day FX rate; the valuation price and FX rate are independently the latest on or before `as_of` and never look forward. Both dates are shown, and carried-forward values are badged.
- **Honest failure.** A position missing market data is shown as unavailable with the reason, and the portfolio totals are withheld rather than shown as a partial sum. A closed position needs no current price, since it is worth zero regardless, but still needs its historical FX rates.
- **Layered backend** with validated domain types, SQL confined to one module, typed errors, structured JSON errors, request IDs in logs, health probes, an upload size limit and graceful shutdown.

## Assumptions and limitations

- Fractional-digit limits apply to the text as written (`10.0000` is a valid price, `10.00000` is not), while duplicates compare values numerically. Quantities must be plain digits (`+5` and `1.0` are rejected) and sides upper-case.
- An empty or header-only file is rejected rather than treated as a successful import of nothing.
- IDs only need to be non-empty and are compared exactly, including case and whitespace.
- Economic oversells (selling more than is held) are not detected; the contract puts this out of scope.
- Reference rows removed from a CSV are not deleted from an existing database. Pointing the application at a different dataset should start from a fresh volume.
- Quantities above 2^53 are exact in the backend and API but would display approximately in the browser.
- Browser behaviour was checked manually; there are no end-to-end browser tests. Docker startup was verified on Windows 11 with Docker Desktop, not on macOS or Linux hosts.
- Out of scope, per the brief: authentication, daily P&L, realised/unrealised split and pagination.

## Time spent

Approximately four hours of hands-on work across two sittings, focused on planning, reviewing, testing and refining the AI-assisted implementation (see `AI_USAGE.md`).

## Next steps

1. A CI workflow running formatting, linting, both test suites and the Docker build.
2. Browser-level end-to-end tests of the upload and investigation journey.
3. The optional per-position daily P&L view.
