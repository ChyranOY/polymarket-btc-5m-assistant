"""For each market in markets.csv, fetch 1m BTC candles from Binance ending at
entry_time, run Kronos-small forecast for the next 5 minutes, compute P(up).

Writes forecasts.csv with one row per market. Skips rows where the candle
window overlaps the forecast horizon (data contamination guard).

Kronos import assumes the repo is cloned and KRONOS_PATH env var points to it.
"""
import argparse
import csv
import os
import sys
import time
from datetime import datetime, timezone

import numpy as np
import pandas as pd
import requests
from tqdm import tqdm

KRONOS_PATH = os.environ.get("KRONOS_PATH")
if not KRONOS_PATH:
    sys.exit("Set KRONOS_PATH to your cloned Kronos repo (see README)")
sys.path.insert(0, KRONOS_PATH)

from model import Kronos, KronosTokenizer, KronosPredictor  # noqa: E402

LOOKBACK_MIN = 400
HORIZON_MIN = 5
BINANCE_KLINES = "https://api.binance.com/api/v3/klines"


def fetch_klines(end_ms: int, lookback_min: int) -> pd.DataFrame:
    """Last `lookback_min` 1m candles ending at (not including) end_ms."""
    start_ms = end_ms - lookback_min * 60_000
    params = {
        "symbol": "BTCUSDT",
        "interval": "1m",
        "startTime": start_ms,
        "endTime": end_ms - 1,
        "limit": lookback_min,
    }
    r = requests.get(BINANCE_KLINES, params=params, timeout=15)
    r.raise_for_status()
    raw = r.json()
    if len(raw) < lookback_min - 5:
        return pd.DataFrame()
    df = pd.DataFrame(raw, columns=[
        "open_time", "open", "high", "low", "close", "volume",
        "close_time", "quote_volume", "trades", "tb_base", "tb_quote", "ignore",
    ])
    for c in ("open", "high", "low", "close", "volume", "quote_volume"):
        df[c] = df[c].astype(float)
    df["amount"] = df["quote_volume"]
    df["timestamps"] = pd.to_datetime(df["open_time"], unit="ms", utc=True)
    return df[["timestamps", "open", "high", "low", "close", "volume", "amount"]]


def parse_iso(s: str) -> datetime:
    # Supabase returns e.g. "2026-04-16T19:05:03.123456+00:00"
    return datetime.fromisoformat(s.replace("Z", "+00:00"))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="inp", default="markets.csv")
    ap.add_argument("--out", default="forecasts.csv")
    ap.add_argument("--samples", type=int, default=30,
                    help="Monte Carlo sample paths per market")
    ap.add_argument("--model", default="NeoQuasar/Kronos-small")
    ap.add_argument("--tokenizer", default="NeoQuasar/Kronos-Tokenizer-base")
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--T", type=float, default=1.0)
    ap.add_argument("--top_p", type=float, default=0.9)
    args = ap.parse_args()

    print(f"loading {args.model} on {args.device} ...")
    tok = KronosTokenizer.from_pretrained(args.tokenizer)
    mdl = Kronos.from_pretrained(args.model)
    predictor = KronosPredictor(mdl, tok, device=args.device, max_context=512)

    markets = pd.read_csv(args.inp)
    out_rows = []

    for _, m in tqdm(markets.iterrows(), total=len(markets)):
        try:
            t0 = parse_iso(m["entry_time"])
        except Exception as e:
            print(f"skip {m['id']}: bad entry_time ({e})")
            continue
        end_ms = int(t0.astimezone(timezone.utc).timestamp() * 1000)

        candles = fetch_klines(end_ms, LOOKBACK_MIN)
        if candles.empty:
            continue
        # Rate-limit: Binance allows 1200 req/min — sleep a touch to be safe.
        time.sleep(0.05)

        last_close = float(candles["close"].iloc[-1])
        last_ts = candles["timestamps"].iloc[-1]
        y_ts = pd.Series(pd.date_range(
            start=last_ts + pd.Timedelta(minutes=1),
            periods=HORIZON_MIN, freq="1min"
        ))

        ups = 0
        closes_end = []
        for _ in range(args.samples):
            pred = predictor.predict(
                df=candles[["open", "high", "low", "close", "volume", "amount"]],
                x_timestamp=candles["timestamps"],
                y_timestamp=y_ts,
                pred_len=HORIZON_MIN,
                T=args.T, top_p=args.top_p, sample_count=1,
            )
            close_end = float(pred["close"].iloc[-1])
            closes_end.append(close_end)
            if close_end > last_close:
                ups += 1

        p_up = ups / args.samples
        out_rows.append({
            "id": m["id"],
            "entry_time": m["entry_time"],
            "last_close": last_close,
            "pred_close_median": float(np.median(closes_end)),
            "pred_close_std": float(np.std(closes_end)),
            "kronos_p_up": p_up,
        })

    pd.DataFrame(out_rows).to_csv(args.out, index=False)
    print(f"wrote {args.out} ({len(out_rows)} forecasts)")


if __name__ == "__main__":
    main()
