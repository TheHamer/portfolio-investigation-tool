#!/usr/bin/env python3
"""End-to-end smoke test of a running instance, over HTTP.

Drives the documented journey against a fresh database: checks that the
frontend is served, uploads the five fixtures in the contract's order, checks
each import report and that the rejected file changed nothing, then compares
every portfolio total and every position (figures, valuation price and FX with
their dates, and each trade's trade-day FX) with the independent calculation
in `tools/recompute.py`. With `--after-restart` it instead checks that a
restarted instance still holds the same trades and figures.

This covers the API and that the UI is served; it does not drive a browser.
See the README for the manual UI checks and how to run this against an
isolated Docker Compose project.

Usage:
    python tools/smoke_test.py --base-url http://localhost:18080
    python tools/smoke_test.py --base-url http://localhost:18080 --after-restart
"""

import argparse
import importlib.util
import json
import re
import sys
import time
import urllib.error
import urllib.request
import uuid
from decimal import Decimal
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("recompute", TOOLS / "recompute.py")
recompute = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(recompute)

failures: list[str] = []


def check(condition: bool, description: str) -> bool:
    print(f"  {'ok  ' if condition else 'FAIL'} {description}")
    if not condition:
        failures.append(description)
    return condition


def request(base: str, path: str, data: bytes | None = None, headers: dict | None = None):
    """(status, body bytes, content type); HTTP errors are returned, not raised."""
    req = urllib.request.Request(base + path, data=data, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            return response.status, response.read(), response.headers.get("Content-Type", "")
    except urllib.error.HTTPError as error:
        return error.code, error.read(), error.headers.get("Content-Type", "")


def get_json(base: str, path: str):
    status, body, _ = request(base, path)
    if status != 200:
        raise AssertionError(f"GET {path} returned {status}: {body[:200]!r}")
    return json.loads(body)


def upload(base: str, file: Path):
    boundary = uuid.uuid4().hex
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{file.name}"\r\n'
        "Content-Type: text/csv\r\n\r\n"
    ).encode() + file.read_bytes() + f"\r\n--{boundary}--\r\n".encode()
    status, response, _ = request(
        base, "/api/trades/import", body, {"Content-Type": f"multipart/form-data; boundary={boundary}"}
    )
    return status, json.loads(response)


def wait_until_ready(base: str, timeout: float = 120) -> None:
    deadline = time.monotonic() + timeout
    while True:
        try:
            if request(base, "/api/ready")[0] == 200:
                return
        except (urllib.error.URLError, ConnectionError, OSError):
            pass
        if time.monotonic() > deadline:
            raise SystemExit(f"{base} did not become ready within {timeout:.0f}s")
        time.sleep(1)


def stored_trade_count(base: str) -> int:
    return sum(p["trade_count"] for p in get_json(base, "/api/portfolios")["portfolios"])


def positions_snapshot(base: str, expected) -> list:
    return [get_json(base, f"/api/portfolios/{p}/positions") for p in expected.totals]


# --- Steps ---------------------------------------------------------------------------


def check_frontend(base: str) -> None:
    print("Frontend")
    status, html, content_type = request(base, "/")
    check(status == 200 and "text/html" in content_type, "GET / serves the HTML page")
    check(b'<div id="root">' in html, "the page has the React root element")

    script = re.search(rb'src="(/assets/[^"]+\.js)"', html)
    if check(script is not None, "the page references a built JavaScript bundle"):
        status, _, content_type = request(base, script.group(1).decode())
        check(status == 200 and "javascript" in content_type, "the bundle is served")


def check_portfolio_list(base: str, expected, trades: int) -> None:
    print("Portfolios")
    listing = get_json(base, "/api/portfolios")
    check(listing["dataset_id"] == expected.dataset_id, f"dataset {expected.dataset_id}")
    check(listing["as_of"] == str(expected.as_of), f"valued as of {expected.as_of}")
    ids = [p["portfolio_id"] for p in listing["portfolios"]]
    check(ids == sorted(expected.totals), f"portfolios {', '.join(ids)}")
    check(stored_trade_count(base) == trades, f"{trades} trades stored")


def check_uploads(base: str, expected, data_dir: Path) -> None:
    print("Uploads, in the contract's order")
    for name, outcome, counts in expected.uploads:
        if outcome == "rejected":
            before = positions_snapshot(base, expected)

        status, report = upload(base, data_dir / "uploads" / name)

        if outcome == "success":
            check(
                status == 200
                and report["outcome"] == "success"
                and report["total_rows"] == counts["total"]
                and report["inserted_rows"] == counts["inserted"]
                and report["skipped_rows"] == counts["skipped"],
                f"{name}: success, {report.get('inserted_rows')} inserted, {report.get('skipped_rows')} skipped",
            )
        else:
            rows = [e["row"] for e in report["errors"]]
            check(
                status == 422
                and report["outcome"] == "rejected"
                and report["inserted_rows"] == 0
                and report["invalid_rows"] == counts["invalid"]
                and report["aborted_rows"] == counts["aborted"]
                and rows == counts["invalid_rows"],
                f"{name}: rejected, invalid rows {rows}, {report.get('aborted_rows')} valid rows not imported",
            )
            check(positions_snapshot(base, expected) == before, f"{name}: every portfolio unchanged")


def same(api_value: str | None, expected: Decimal) -> bool:
    return api_value is not None and Decimal(api_value) == expected


def check_valuations(base: str, expected) -> None:
    print("Portfolio totals and every position, against tools/recompute.py")
    money = recompute.round_money

    for portfolio_id, t in expected.totals.items():
        body = get_json(base, f"/api/portfolios/{portfolio_id}/positions")
        totals = body["totals"] or {}
        check(
            body["status"] == "ok"
            and body["open_position_count"] == t["open"]
            and body["closed_position_count"] == t["closed"]
            and same(totals.get("market_value_gbp_exact"), t["market_value"])
            and same(totals.get("net_trade_cost_gbp_exact"), t["net_cost"])
            and same(totals.get("total_pnl_gbp_exact"), t["pnl"])
            and totals.get("market_value_gbp") == str(money(t["market_value"]))
            and totals.get("net_trade_cost_gbp") == str(money(t["net_cost"]))
            and totals.get("total_pnl_gbp") == str(money(t["pnl"])),
            f"{portfolio_id}: market value {totals.get('market_value_gbp')}, "
            f"net cost {totals.get('net_trade_cost_gbp')}, P&L {totals.get('total_pnl_gbp')}, "
            f"{t['open']} open, {t['closed']} closed",
        )

        listed = {p["instrument_id"] for p in body["positions"]}
        wanted = {i for p, i in expected.results if p == portfolio_id}
        check(listed == wanted, f"{portfolio_id}: lists exactly its {len(wanted)} positions")

        mismatched = [i for i in sorted(wanted) if not position_matches(base, expected, portfolio_id, i)]
        check(not mismatched, f"{portfolio_id}: every position's figures, observations and trade FX match"
              + (f" (mismatched: {', '.join(mismatched)})" if mismatched else ""))


def position_matches(base: str, expected, portfolio_id: str, instrument_id: str) -> bool:
    v = expected.results[(portfolio_id, instrument_id)]
    detail = get_json(base, f"/api/portfolios/{portfolio_id}/positions/{instrument_id}")
    position, breakdown = detail["position"], detail["breakdown"] or {}
    money = recompute.round_money

    def observation(api, field, wanted):
        if wanted is None:
            return api is None
        return api is not None and api["date"] == str(wanted[0]) and Decimal(api[field]) == wanted[1]

    currency = expected.instruments[instrument_id]["currency"]
    trades = sorted(expected.positions[(portfolio_id, instrument_id)], key=lambda t: (t.timestamp, t.trade_id))
    trade_fx = [recompute.latest_on_or_before(expected.fx_rates[currency], t.trade_date) for t in trades]

    return (
        position["status"] == "ok"
        and position["quantity"] == v["quantity"]
        and position["is_closed"] == (v["quantity"] == 0)
        and position["market_value_gbp"] == str(money(v["market_value"]))
        and position["net_trade_cost_gbp"] == str(money(v["net_cost"]))
        and position["total_pnl_gbp"] == str(money(v["pnl"]))
        and same(breakdown.get("market_value_gbp_exact"), v["market_value"])
        and same(breakdown.get("net_trade_cost_gbp_exact"), v["net_cost"])
        and same(breakdown.get("total_pnl_gbp_exact"), v["pnl"])
        and observation(position["valuation_price"], "price", v["price"])
        and observation(position["valuation_fx"], "gbp_per_unit", v["fx"])
        and [t["trade_id"] for t in detail["trades"]] == [t.trade_id for t in trades]
        and all(observation(api["fx"], "gbp_per_unit", fx) for api, fx in zip(detail["trades"], trade_fx))
    )


def check_investigations(base: str) -> None:
    print("Investigating positions")
    open_position = get_json(base, "/api/portfolios/P002/positions/I017")["position"]
    check(
        open_position["valuation_price"]["date"] == "2026-03-26"
        and open_position["valuation_price"]["carried_forward"] is True
        and open_position["valuation_fx"]["date"] == "2026-03-27"
        and open_position["valuation_fx"]["carried_forward"] is False,
        "P002/I017 (open): 2026-03-26 price carried forward, 2026-03-27 FX",
    )

    closed = get_json(base, "/api/portfolios/P002/positions/I014")["position"]
    check(
        closed["is_closed"] is True and closed["market_value_gbp"] == "0.00" and closed["total_pnl_gbp"] != "0.00",
        f"P002/I014 (closed): market value 0.00, P&L {closed['total_pnl_gbp']}",
    )

    trades = get_json(base, "/api/portfolios/P002/positions/I020")["trades"]
    carried = next(t for t in trades if t["trade_id"] == "T229708")
    check(
        carried["fx"]["date"] == "2026-01-08" and carried["fx"]["carried_forward"] is True,
        "P002/I020 trade T229708 (2026-01-09): trade-day FX carried forward from 2026-01-08",
    )


def check_after_restart(base: str, expected, data_dir: Path) -> None:
    print("After restart")
    stored = sum(len(t) for t in expected.positions.values())
    check(stored_trade_count(base) == stored, f"all {stored} trades are still stored")

    status, report = upload(base, data_dir / "uploads" / "trades_initial.csv")
    check(
        status == 200 and report["inserted_rows"] == 0 and report["skipped_rows"] == report["total_rows"],
        f"re-uploading trades_initial.csv inserts nothing ({report.get('skipped_rows')} skipped)",
    )
    check(stored_trade_count(base) == stored, f"still {stored} trades stored")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base-url", default="http://localhost:8080")
    parser.add_argument("--data-dir", type=Path, default=recompute.DEFAULT_DATA_DIR)
    parser.add_argument("--after-restart", action="store_true", help="check a restarted instance instead")
    args = parser.parse_args()
    base = args.base_url.rstrip("/")

    expected = recompute.recompute(args.data_dir)
    wait_until_ready(base)
    print(f"Smoke test of {base}\n")

    check_frontend(base)
    if args.after_restart:
        check_after_restart(base, expected, args.data_dir)
    else:
        check_portfolio_list(base, expected, trades=0)
        check_uploads(base, expected, args.data_dir)
    check_valuations(base, expected)
    check_investigations(base)

    print(f"\n{'FAILED: ' + str(len(failures)) + ' check(s)' if failures else 'All checks passed.'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
