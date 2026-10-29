-- Schema for the Portfolio Investigation Tool.
--
-- Financial values (prices, FX rates) are stored as validated decimal TEXT and
-- parsed into rust_decimal::Decimal by the application, so no value ever passes
-- through binary floating point.
--
-- The decimal CHECKs use text patterns only (GLOB, instr, length), so SQLite's
-- permissive numeric casts play no part. A value must be one or more digits,
-- optionally followed by '.' and one or more digits; it must contain a
-- non-zero digit (so it is greater than zero); and it may have at most 4
-- fractional digits (prices) or 6 (FX rates). That is exactly the grammar the
-- Rust domain types (`Price`, `FxRate`) accept, so the database is an
-- independent safeguard rather than a weaker one.
--
-- STRICT tables make SQLite reject values of the wrong storage class instead
-- of silently coercing them. TEXT comparisons use the default BINARY collation,
-- so identifiers are case-sensitive as the data contract requires.

CREATE TABLE portfolios (
    portfolio_id  TEXT NOT NULL PRIMARY KEY CHECK (length(portfolio_id) > 0),
    name          TEXT NOT NULL,
    base_currency TEXT NOT NULL CHECK (base_currency = 'GBP')
) STRICT;

CREATE TABLE instruments (
    instrument_id TEXT NOT NULL PRIMARY KEY CHECK (length(instrument_id) > 0),
    ticker        TEXT NOT NULL,
    name          TEXT NOT NULL,
    currency      TEXT NOT NULL CHECK (currency IN ('GBP', 'USD', 'EUR')),
    sector        TEXT NOT NULL
) STRICT;

-- The composite primary key doubles as the index for "latest price on or
-- before date" lookups.
CREATE TABLE prices (
    instrument_id TEXT NOT NULL REFERENCES instruments (instrument_id),
    date          TEXT NOT NULL CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    close_price   TEXT NOT NULL CHECK (
        close_price GLOB '[0-9]*'
        AND close_price GLOB '*[0-9]'
        AND close_price NOT GLOB '*[^0-9.]*'
        AND close_price NOT GLOB '*.*.*'
        AND close_price GLOB '*[1-9]*'
        AND (instr(close_price, '.') = 0 OR length(close_price) - instr(close_price, '.') <= 4)
    ),
    PRIMARY KEY (instrument_id, date)
) STRICT, WITHOUT ROWID;

CREATE TABLE fx_rates (
    currency     TEXT NOT NULL CHECK (currency IN ('GBP', 'USD', 'EUR')),
    date         TEXT NOT NULL CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    gbp_per_unit TEXT NOT NULL CHECK (
        gbp_per_unit GLOB '[0-9]*'
        AND gbp_per_unit GLOB '*[0-9]'
        AND gbp_per_unit NOT GLOB '*[^0-9.]*'
        AND gbp_per_unit NOT GLOB '*.*.*'
        AND gbp_per_unit GLOB '*[1-9]*'
        AND (instr(gbp_per_unit, '.') = 0 OR length(gbp_per_unit) - instr(gbp_per_unit, '.') <= 6)
    ),
    PRIMARY KEY (currency, date)
) STRICT, WITHOUT ROWID;

-- timestamp_utc is canonical 'YYYY-MM-DDTHH:MM:SSZ', which sorts
-- lexicographically in chronological order.
CREATE TABLE trades (
    trade_id      TEXT    NOT NULL PRIMARY KEY CHECK (length(trade_id) > 0),
    portfolio_id  TEXT    NOT NULL REFERENCES portfolios (portfolio_id),
    instrument_id TEXT    NOT NULL REFERENCES instruments (instrument_id),
    timestamp_utc TEXT    NOT NULL CHECK (timestamp_utc GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9]Z'),
    side          TEXT    NOT NULL CHECK (side IN ('BUY', 'SELL')),
    quantity      INTEGER NOT NULL CHECK (quantity > 0),
    price         TEXT    NOT NULL CHECK (
        price GLOB '[0-9]*'
        AND price GLOB '*[0-9]'
        AND price NOT GLOB '*[^0-9.]*'
        AND price NOT GLOB '*.*.*'
        AND price GLOB '*[1-9]*'
        AND (instr(price, '.') = 0 OR length(price) - instr(price, '.') <= 4)
    ),
    imported_at   TEXT    NOT NULL
) STRICT;

-- Supports listing a portfolio's positions and fetching one position's trades
-- in execution order without a separate sort.
CREATE INDEX idx_trades_position
    ON trades (portfolio_id, instrument_id, timestamp_utc, trade_id);
