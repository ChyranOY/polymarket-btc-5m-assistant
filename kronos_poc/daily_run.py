"""Daily Kronos evaluation. Runs via GitHub Actions cron before the PST
trading window opens. Emits a JSON summary of yesterday's trades for the
dashboard to render.

Output: ../static/kronos_daily.json (relative to this file).
"""
import argparse
import json
import os
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

import numpy as np
import pandas as pd
import requests

KRONOS_PATH = os.environ.get("KRONOS_PATH")
if not KRONOS_PATH:
    sys.exit("Set KRONOS_PATH to your cloned Kronos repo")
sys.path.insert(0, KRONOS_PATH)
from model import Kronos, KronosTokenizer, KronosPredictor  # noqa: E402

LOOKBACK_MIN = 400
HORIZON_MIN = 5
BINANCE_KLINES = "https://api.binance.com/api/v3/klines"


def pst_midnight_utc(days_ago: int = 1) -> datetime:
    """UTC instant for 00:00 PST `days_ago` days back. PST is UTC-8 year-round
    for this purpose — DST costs us an hour once a day, acceptable for a
    learning dashboard."""
    now = datetime.now(timezone.utc) - timedelta(days=days_ago)
    pst = now - timedelta(hours=8)
    pst_midnight = pst.replace(hour=0, minute=0, second=0, microsecond=0)
    return pst_midnight + timedelta(hours=8)


def fetch_trades(since: datetime, until: datetime) -> list[dict]:
    base = os.environ["SUPABASE_URL"].rstrip("/")
    key = os.environ["SUPABASE_SERVICE_ROLE_KEY"]
    # PostgREST supports repeated filters on a column; pass params as a list
    # of tuples so both `gte.` and `lte.` bounds survive encoding.
    r = requests.get(
        f"{base}/rest/v1/trades",
        params=[
            ("status", "eq.CLOSED"),
            ("exitReason", "in.(market_rolled_won,market_rolled_lost)"),
            ("entryGateSnapshot", "like.*up_ask*"),
            ("entryTime", f"gte.{since.isoformat()}"),
            ("entryTime", f"lt.{until.isoformat()}"),
            ("order", "entryTime.asc"),
            ("limit", "500"),
        ],
        headers={"apikey": key, "Authorization": f"Bearer {key}"},
        timeout=30,
    )
    r.raise_for_status()
    return r.json()


def fetch_klines(end_ms: int) -> pd.DataFrame:
    r = requests.get(BINANCE_KLINES, params={
        "symbol": "BTCUSDT", "interval": "1m",
        "startTime": end_ms - LOOKBACK_MIN * 60_000,
        "endTime": end_ms - 1,
        "limit": LOOKBACK_MIN,
    }, timeout=15)
    r.raise_for_status()
    raw = r.json()
    if len(raw) < LOOKBACK_MIN - 5:
        return pd.DataFrame()
    df = pd.DataFrame(raw, columns=[
        "open_time", "open", "high", "low", "close", "volume",
        "close_time", "quote_volume", "trades", "tb_b", "tb_q", "_",
    ])
    for c in ("open", "high", "low", "close", "volume", "quote_volume"):
        df[c] = df[c].astype(float)
    df["amount"] = df["quote_volume"]
    df["timestamps"] = pd.to_datetime(df["open_time"], unit="ms", utc=True)
    return df[["timestamps", "open", "high", "low", "close", "volume", "amount"]]


def kronos_p_up(predictor: KronosPredictor, candles: pd.DataFrame,
                samples: int, T: float, top_p: float) -> float:
    last_close = float(candles["close"].iloc[-1])
    last_ts = candles["timestamps"].iloc[-1]
    y_ts = pd.Series(pd.date_range(
        start=last_ts + pd.Timedelta(minutes=1),
        periods=HORIZON_MIN, freq="1min"
    ))
    ups = 0
    for _ in range(samples):
        pred = predictor.predict(
            df=candles[["open", "high", "low", "close", "volume", "amount"]],
            x_timestamp=candles["timestamps"],
            y_timestamp=y_ts,
            pred_len=HORIZON_MIN, T=T, top_p=top_p, sample_count=1,
        )
        if float(pred["close"].iloc[-1]) > last_close:
            ups += 1
    return ups / samples


def log_loss(p: np.ndarray, y: np.ndarray, eps: float = 1e-6) -> float:
    p = np.clip(p, eps, 1 - eps)
    return float(-np.mean(y * np.log(p) + (1 - y) * np.log(1 - p)))


def brier(p: np.ndarray, y: np.ndarray) -> float:
    return float(np.mean((p - y) ** 2))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", type=int, default=1, help="PST days back")
    ap.add_argument("--samples", type=int, default=20)
    ap.add_argument("--model", default="NeoQuasar/Kronos-small")
    ap.add_argument("--tokenizer", default="NeoQuasar/Kronos-Tokenizer-base")
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--T", type=float, default=1.0)
    ap.add_argument("--top_p", type=float, default=0.9)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    out_path = Path(args.out) if args.out else (
        Path(__file__).resolve().parent.parent / "static" / "kronos_daily.json"
    )

    since = pst_midnight_utc(days_ago=args.days)
    until = pst_midnight_utc(days_ago=args.days - 1)
    print(f"window: {since.isoformat()} → {until.isoformat()}")

    trades = fetch_trades(since, until)
    print(f"fetched {len(trades)} trades")

    if not trades:
        out_path.write_text(json.dumps({
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "window": {"from": since.isoformat(), "to": until.isoformat()},
            "model": args.model, "samples": args.samples,
            "metrics": None, "trades": [],
            "note": "no trades in window",
        }, indent=2))
        print(f"wrote {out_path} (empty)")
        return

    print(f"loading {args.model} on {args.device} ...")
    tok = KronosTokenizer.from_pretrained(args.tokenizer)
    mdl = Kronos.from_pretrained(args.model)
    predictor = KronosPredictor(mdl, tok, device=args.device, max_context=512)

    scored = []
    for t in trades:
        try:
            snap = json.loads(t["entryGateSnapshot"])
            up_ask = float(snap["up_ask"]) if snap.get("up_ask") else None
            down_ask = float(snap["down_ask"]) if snap.get("down_ask") else None
            if up_ask is None or down_ask is None or not (0 < up_ask < 1):
                continue
            entry_dt = datetime.fromisoformat(t["entryTime"].replace("Z", "+00:00"))
            end_ms = int(entry_dt.timestamp() * 1000)
            candles = fetch_klines(end_ms)
            if candles.empty:
                continue
            time.sleep(0.05)
            p_up = kronos_p_up(predictor, candles, args.samples, args.T, args.top_p)

            side = (t.get("side") or "").upper()
            reason = t.get("exitReason") or ""
            outcome_up = int((side == "UP") == (reason == "market_rolled_won"))
            # "Agreement" = Kronos's side matches what we actually traded.
            kronos_go_up = p_up > 0.5
            bot_go_up = side == "UP"
            agreement = kronos_go_up == bot_go_up
            scored.append({
                "id": t["id"],
                "entry_time": t["entryTime"],
                "slug": t.get("marketSlug"),
                "side": side,
                "market_p_up": up_ask,
                "kronos_p_up": p_up,
                "outcome_up": outcome_up,
                "pnl": float(t["pnl"]) if t.get("pnl") is not None else None,
                "agreement": agreement,
            })
        except Exception as e:
            print(f"skip {t.get('id')}: {e}")

    if not scored:
        print("no scored trades — aborting")
        return

    sdf = pd.DataFrame(scored)
    y = sdf["outcome_up"].to_numpy()
    p_m = sdf["market_p_up"].to_numpy()
    p_k = sdf["kronos_p_up"].to_numpy()

    # Counterfactual: had we traded Kronos's side with the market's ask on that
    # side at entry, what's the PnL per $1 contract?
    def cf_pnl(row) -> float:
        go_up = row["kronos_p_up"] > 0.5
        entry_ask = row["market_p_up"] if go_up else (1 - row["market_p_up"])
        if entry_ask <= 0:
            return 0.0
        won = bool(row["outcome_up"]) == go_up
        return (1 - entry_ask) / entry_ask if won else -1.0

    sdf["cf_pnl_per_dollar"] = sdf.apply(cf_pnl, axis=1)
    actual_pnl = sdf["pnl"].fillna(0).sum()
    # Normalize per-$1: assume contract size ≈ |pnl|/return. Use actual pnl as-is
    # since the dashboard already reads our balance; this is just a total number.

    metrics = {
        "n": int(len(sdf)),
        "market_log_loss": log_loss(p_m, y),
        "kronos_log_loss": log_loss(p_k, y),
        "market_brier": brier(p_m, y),
        "kronos_brier": brier(p_k, y),
        "agreement_rate": float(sdf["agreement"].mean()),
        "counterfactual_total_per_dollar": float(sdf["cf_pnl_per_dollar"].sum()),
        "counterfactual_avg_per_dollar": float(sdf["cf_pnl_per_dollar"].mean()),
        "actual_pnl_usd": float(actual_pnl),
    }

    payload = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "window": {"from": since.isoformat(), "to": until.isoformat()},
        "model": args.model,
        "samples": args.samples,
        "metrics": metrics,
        "trades": sdf.to_dict(orient="records"),
    }

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(payload, indent=2, default=str))
    print(f"wrote {out_path} ({len(sdf)} trades scored)")
    print(json.dumps(metrics, indent=2))


if __name__ == "__main__":
    main()
