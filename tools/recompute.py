#!/usr/bin/env python3
"""Independent recomputation of the portfolio figures from the supplied data.

This script is a verification aid, not part of the application. It shares no
code or data with the Rust backend: it reads the reference CSVs, the manifest
and the trade upload fixtures directly, replays the fixtures in the order the
data contract specifies (initial, repeat, incremental, invalid, corrected)
using the contract's upload rules, and values every position as of the
manifest's `as_of` date following DATA_CONTRACT.md section 4.

All arithmetic uses `decimal.Decimal` in a context that raises on any inexact
operation, so every figure is exact until it is rounded half-up to pennies
for display. The golden values in `backend/tests/golden_values.rs` were taken
from this script's output.

Usage:
    python tools/recompute.py [--data-dir PATH] [--position PORTFOLIO/INSTRUMENT ...]
"""

import argparse
import csv
import decimal
import json
import re
import sys
from dataclasses import dataclass
from datetime import date
from decimal import ROUND_HALF_UP, Decimal
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_DATA_DIR = REPO_ROOT / "data"

UPLOAD_SEQUENCE = [
    "trades_initial.csv",
    "trades_repeat.csv",
    "trades_incremental.csv",
    "trades_invalid.csv",
    "trades_corrected.csv",
]

TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")
QUANTITY = re.compile(r"\d+")
PRICE = re.compile(r"\d+(\.\d{1,4})?")  # at most four fractional digits

TRADE_HEADER = ["trade_id", "portfolio_id", "instrument_id", "timestamp", "side", "quantity", "price"]

# Exact arithmetic: any operation that would need rounding raises instead.
EXACT = decimal.Context(prec=100, traps=[decimal.Inexact, decimal.InvalidOperation, decimal.Overflow])
decimal.setcontext(EXACT)

PENNY = Decimal("0.01")


def round_money(value: Decimal) -> Decimal:
    """Two decimal places, half up (away from zero for negative amounts)."""
    rounded = value.quantize(PENNY, rounding=ROUND_HALF_UP, context=decimal.Context(prec=100))
    return rounded if rounded else Decimal("0.00")  # no "-0.00"


# --- Reference data -------------------------------------------------------------


def read_csv(path: Path) -> list[dict[str, str]]:
    with path.open(newline="", encoding="utf-8-sig") as f:
        return list(csv.DictReader(f))


def load_series(rows, key_field, value_field):
    """{key: sorted [(date, Decimal)]} from dated reference rows."""
    series: dict[str, list[tuple[date, Decimal]]] = {}
    for row in rows:
        series.setdefault(row[key_field], []).append(
            (date.fromisoformat(row["date"]), Decimal(row[value_field]))
        )
    for points in series.values():
        points.sort()
    return series


def latest_on_or_before(points, wanted: date):
    """The latest (date, value) dated on or before `wanted`, or None. Never looks forward."""
    found = None
    for point_date, value in points:
        if point_date > wanted:
            break
        found = (point_date, value)
    return found


# --- Trade upload replay ----------------------------------------------------------


@dataclass(frozen=True)
class Trade:
    trade_id: str
    portfolio_id: str
    instrument_id: str
    timestamp: str
    side: str
    quantity: int
    price: Decimal

    @property
    def trade_date(self) -> date:
        return date.fromisoformat(self.timestamp[:10])

    @property
    def signed_quantity(self) -> int:
        return self.quantity if self.side == "BUY" else -self.quantity


def parse_row(row, portfolios, instruments, calendar):
    """A Trade, or the list of reasons the row is invalid."""
    errors = []
    if len(row) != len(TRADE_HEADER):
        return None, ["wrong field count"]
    trade_id, portfolio_id, instrument_id, timestamp, side, quantity, price = row

    if not trade_id:
        errors.append("empty trade_id")
    if portfolio_id not in portfolios:
        errors.append(f"unknown portfolio {portfolio_id!r}")
    if instrument_id not in instruments:
        errors.append(f"unknown instrument {instrument_id!r}")

    if not TIMESTAMP.fullmatch(timestamp):
        errors.append(f"timestamp {timestamp!r} not YYYY-MM-DDTHH:MM:SSZ")
    elif timestamp[:10] not in calendar:
        errors.append(f"date {timestamp[:10]} not a valuation date")

    if side not in ("BUY", "SELL"):
        errors.append(f"side {side!r}")
    if not QUANTITY.fullmatch(quantity) or int(quantity) <= 0:
        errors.append(f"quantity {quantity!r}")
    if not PRICE.fullmatch(price) or Decimal(price) <= 0:
        errors.append(f"price {price!r}")

    if errors:
        return None, errors
    return Trade(trade_id, portfolio_id, instrument_id, timestamp, side, int(quantity), Decimal(price)), []


def replay_upload(path, stored, portfolios, instruments, calendar):
    """Applies one upload with the contract's atomic, idempotent rules.
    Returns (outcome, counts) and updates `stored` only on success."""
    with path.open(newline="", encoding="utf-8-sig") as f:
        reader = csv.reader(f)
        if next(reader) != TRADE_HEADER:
            return "rejected", {"file_error": "bad header"}
        rows = list(reader)

    parsed = {}  # row number -> Trade
    invalid = {}  # row number -> reasons
    for number, row in enumerate(rows, start=2):  # the header is row 1
        trade, errors = parse_row(row, portfolios, instruments, calendar)
        if errors:
            invalid[number] = errors
        else:
            parsed[number] = trade

    # Rows sharing a trade ID: identical repeats are fine, anything else conflicts.
    by_id: dict[str, list[int]] = {}
    for number, row in enumerate(rows, start=2):
        if row and row[0]:
            by_id.setdefault(row[0], []).append(number)
    for trade_id, numbers in by_id.items():
        variants = {parsed.get(n) for n in numbers}
        if len(variants) > 1:
            for n in numbers:
                invalid.setdefault(n, []).append(f"conflicting rows for {trade_id}")

    # Conflicts with trades already stored.
    new, skipped = {}, 0
    for number, trade in parsed.items():
        if number in invalid:
            continue
        existing = stored.get(trade.trade_id)
        if existing is None:
            if trade.trade_id in new:
                skipped += 1  # exact repeat within the file
            new[trade.trade_id] = trade
        elif existing == trade:
            skipped += 1
        else:
            invalid[number] = [f"{trade.trade_id} already stored with different values"]

    total = len(rows)
    if invalid:
        return "rejected", {
            "total": total,
            "invalid": len(invalid),
            "aborted": total - len(invalid),
            "invalid_rows": sorted(invalid),
        }

    stored.update(new)
    return "success", {"total": total, "inserted": len(new), "skipped": skipped}


# --- Valuation ----------------------------------------------------------------------


def value_position(trades, currency, price_points, fx_points, as_of):
    """DATA_CONTRACT.md section 4 for one (portfolio, instrument) position."""
    trades = sorted(
        (t for t in trades if t.trade_date <= as_of),
        key=lambda t: (t.timestamp, t.trade_id),
    )

    quantity = 0
    net_cost = Decimal(0)
    for trade in trades:
        trade_fx = latest_on_or_before(fx_points, trade.trade_date)
        if trade_fx is None:
            raise ValueError(f"no {currency} FX on or before {trade.trade_date} for {trade.trade_id}")
        quantity += trade.signed_quantity
        net_cost += trade.signed_quantity * trade.price * trade_fx[1]

    price = latest_on_or_before(price_points, as_of)
    fx = latest_on_or_before(fx_points, as_of)

    if quantity == 0:
        # A closed position is worth nothing whatever today's price; the
        # observations are reported only for information.
        market_value_local = Decimal(0)
        market_value = Decimal(0)
    else:
        if price is None or fx is None:
            raise ValueError("missing valuation price or FX for an open position")
        market_value_local = quantity * price[1]
        market_value = market_value_local * fx[1]

    return {
        "trade_count": len(trades),
        "quantity": quantity,
        "price": price,
        "fx": fx,
        "market_value_local": market_value_local,
        "market_value": market_value,
        "net_cost": net_cost,
        "pnl": market_value - net_cost,
    }


@dataclass
class Recomputation:
    """Everything the replay and valuation produced, for printing or for the
    end-to-end smoke test (`tools/smoke_test.py`) to compare against."""

    dataset_id: str
    as_of: date
    portfolio_names: dict[str, str]
    instruments: dict[str, dict[str, str]]
    fx_rates: dict[str, list[tuple[date, Decimal]]]
    uploads: list[tuple[str, str, dict]]  # (file, outcome, counts)
    positions: dict[tuple[str, str], list[Trade]]
    results: dict[tuple[str, str], dict]  # value_position() per position
    totals: dict[str, dict]  # per portfolio: market_value, net_cost, pnl, open, closed


def recompute(data: Path) -> Recomputation:
    """Replays the upload sequence and values every position as of `as_of`."""
    manifest = json.loads((data / "manifest.json").read_text(encoding="utf-8"))
    as_of = date.fromisoformat(manifest["as_of"])
    calendar = set(manifest["valuation_dates"])

    portfolios = {r["portfolio_id"]: r for r in read_csv(data / "reference" / "portfolios.csv")}
    instruments = {r["instrument_id"]: r for r in read_csv(data / "reference" / "instruments.csv")}
    prices = load_series(read_csv(data / "reference" / "prices.csv"), "instrument_id", "close_price")
    fx_rates = load_series(read_csv(data / "reference" / "fx_rates.csv"), "currency", "gbp_per_unit")

    stored: dict[str, Trade] = {}
    uploads = []
    for name in UPLOAD_SEQUENCE:
        outcome, counts = replay_upload(data / "uploads" / name, stored, portfolios, instruments, calendar)
        uploads.append((name, outcome, counts))

    positions: dict[tuple[str, str], list[Trade]] = {}
    for trade in stored.values():
        positions.setdefault((trade.portfolio_id, trade.instrument_id), []).append(trade)

    results = {}
    for (portfolio_id, instrument_id), trades in sorted(positions.items()):
        currency = instruments[instrument_id]["currency"]
        results[(portfolio_id, instrument_id)] = value_position(
            trades, currency, prices.get(instrument_id, []), fx_rates.get(currency, []), as_of
        )

    totals = {}
    for portfolio_id in sorted(portfolios):
        rows = [v for k, v in results.items() if k[0] == portfolio_id]
        closed = sum(1 for v in rows if v["quantity"] == 0)
        # Totals are summed from unrounded position values, then rounded.
        totals[portfolio_id] = {
            "market_value": sum((v["market_value"] for v in rows), Decimal(0)),
            "net_cost": sum((v["net_cost"] for v in rows), Decimal(0)),
            "pnl": sum((v["pnl"] for v in rows), Decimal(0)),
            "open": len(rows) - closed,
            "closed": closed,
        }

    return Recomputation(
        dataset_id=manifest["dataset_id"],
        as_of=as_of,
        portfolio_names={k: v["name"] for k, v in portfolios.items()},
        instruments=instruments,
        fx_rates=fx_rates,
        uploads=uploads,
        positions=positions,
        results=results,
        totals=totals,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--data-dir", type=Path, default=DEFAULT_DATA_DIR)
    parser.add_argument(
        "--position",
        action="append",
        default=[],
        metavar="PORTFOLIO/INSTRUMENT",
        help="also print the full calculation for this position (repeatable)",
    )
    args = parser.parse_args()

    r = recompute(args.data_dir)
    as_of, instruments, fx_rates, results = r.as_of, r.instruments, r.fx_rates, r.results

    print(f"Dataset {r.dataset_id}, valued as of {as_of}\n")
    print("Upload replay")
    for name, outcome, counts in r.uploads:
        print(f"  {name:<24} {outcome:<9} {counts}")
    print(f"  stored trades: {sum(len(t) for t in r.positions.values())}\n")

    for portfolio_id, t in r.totals.items():
        rows = {k: v for k, v in results.items() if k[0] == portfolio_id}
        market_value, net_cost, pnl = t["market_value"], t["net_cost"], t["pnl"]

        print(f"{portfolio_id}  {r.portfolio_names[portfolio_id]}")
        print(f"  positions: {t['open']} open, {t['closed']} closed")
        print(f"  market value   {round_money(market_value):>14}   exact {market_value}")
        print(f"  net trade cost {round_money(net_cost):>14}   exact {net_cost}")
        print(f"  total P&L      {round_money(pnl):>14}   exact {pnl}")
        print(f"  {'instrument':<10} {'qty':>7} {'price date':>11} {'fx date':>11} "
              f"{'market value':>14} {'net cost':>14} {'P&L':>12}")
        for (_, instrument_id), v in rows.items():
            price_date = v["price"][0] if v["price"] else "-"
            fx_date = v["fx"][0] if v["fx"] else "-"
            flag = " (carried forward)" if v["price"] and v["price"][0] < as_of else ""
            print(f"  {instrument_id:<10} {v['quantity']:>7} {str(price_date):>11} {str(fx_date):>11} "
                  f"{round_money(v['market_value']):>14} {round_money(v['net_cost']):>14} "
                  f"{round_money(v['pnl']):>12}{flag}")
        print()

    for key in args.position:
        portfolio_id, instrument_id = key.split("/")
        v = results[(portfolio_id, instrument_id)]
        currency = instruments[instrument_id]["currency"]
        print(f"{portfolio_id}/{instrument_id} ({instruments[instrument_id]['ticker']}, {currency})")
        for trade in sorted(r.positions[(portfolio_id, instrument_id)], key=lambda t: (t.timestamp, t.trade_id)):
            fx_date, rate = latest_on_or_before(fx_rates[currency], trade.trade_date)
            cost = trade.signed_quantity * trade.price * rate
            print(f"  {trade.trade_id} {trade.timestamp} {trade.side:<4} {trade.quantity:>5} @ {trade.price}"
                  f"  fx {rate} ({fx_date})  cost {cost}")
        print(f"  quantity         {v['quantity']}")
        print(f"  valuation price  {v['price'][1] if v['price'] else '-'} ({v['price'][0] if v['price'] else '-'})")
        print(f"  valuation FX     {v['fx'][1] if v['fx'] else '-'} ({v['fx'][0] if v['fx'] else '-'})")
        print(f"  market value ({currency}) {v['market_value_local']}")
        print(f"  market value     {v['market_value']}  -> {round_money(v['market_value'])}")
        print(f"  net trade cost   {v['net_cost']}  -> {round_money(v['net_cost'])}")
        print(f"  total P&L        {v['pnl']}  -> {round_money(v['pnl'])}\n")

    return 0


if __name__ == "__main__":
    sys.exit(main())
