"""Pull closed Rust-origin rollover trades from Supabase → CSV.

Each row is a completed 5m market we traded, with entry_time (our entry into
the market), side, exit_reason (market_rolled_won/lost), and the JSON
entry_gate_snapshot containing the up_ask / down_ask the bot saw at entry.

From side + exit_reason we derive the settlement direction:
    outcome_up = (side == "UP") == (exit_reason == "market_rolled_won")
"""
import argparse
import csv
import json
import os
import sys

import requests


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="markets.csv")
    ap.add_argument("--limit", type=int, default=500)
    args = ap.parse_args()

    base = os.environ.get("SUPABASE_URL")
    key = os.environ.get("SUPABASE_SERVICE_ROLE_KEY")
    if not base or not key:
        sys.exit("SUPABASE_URL / SUPABASE_SERVICE_ROLE_KEY must be set")

    url = f"{base.rstrip('/')}/rest/v1/trades"
    params = {
        "status": "eq.CLOSED",
        "exitReason": "in.(market_rolled_won,market_rolled_lost)",
        "entryGateSnapshot": "like.*up_ask*",
        "order": "entryTime.desc",
        "limit": str(args.limit),
        "select": "id,entryTime,marketSlug,side,exitReason,entryPrice,entryGateSnapshot,pnl,contractSize,shares",
    }
    headers = {"apikey": key, "Authorization": f"Bearer {key}"}
    r = requests.get(url, params=params, headers=headers, timeout=30)
    r.raise_for_status()
    rows = r.json()
    print(f"fetched {len(rows)} trades")

    with open(args.out, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow([
            "id", "entry_time", "market_slug", "side", "exit_reason",
            "entry_price", "up_ask", "down_ask", "time_left_sec",
            "pnl", "contract_size", "shares", "outcome_up",
        ])
        skipped = 0
        for row in rows:
            snap_raw = row.get("entryGateSnapshot")
            if not snap_raw:
                skipped += 1
                continue
            try:
                snap = json.loads(snap_raw)
            except json.JSONDecodeError:
                skipped += 1
                continue
            side = (row.get("side") or "").upper()
            reason = row.get("exitReason") or ""
            outcome_up = int((side == "UP") == (reason == "market_rolled_won"))
            w.writerow([
                row.get("id"),
                row.get("entryTime"),
                row.get("marketSlug"),
                side,
                reason,
                row.get("entryPrice"),
                snap.get("up_ask"),
                snap.get("down_ask"),
                snap.get("time_left_sec"),
                row.get("pnl"),
                row.get("contractSize"),
                row.get("shares"),
                outcome_up,
            ])
    print(f"wrote {args.out} ({len(rows) - skipped} rows, {skipped} skipped)")


if __name__ == "__main__":
    main()
