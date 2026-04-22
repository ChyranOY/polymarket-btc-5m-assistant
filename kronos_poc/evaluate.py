"""Join markets.csv with forecasts.csv and measure whether Kronos's P(up) has
edge vs. the market's own up_ask probability.

Outputs:
  1. Log-loss of each probability vs. actual outcomes.
  2. Brier score (same idea, squared-error instead of log).
  3. PnL simulation: enter UP when (kronos_p - up_ask) > T, DOWN when
     (up_ask - kronos_p) > T. PnL per trade mirrors the bot's payoff:
       win  → (1 - entry_ask) / entry_ask * contract_size
       lose → -contract_size
     Contract size normalized to $1 (use returns, not dollars).
"""
import argparse
import math

import numpy as np
import pandas as pd


def log_loss(p, y, eps=1e-6):
    p = np.clip(p, eps, 1 - eps)
    return -np.mean(y * np.log(p) + (1 - y) * np.log(1 - p))


def brier(p, y):
    return np.mean((p - y) ** 2)


def sim_pnl(df: pd.DataFrame, threshold: float) -> dict:
    """Take one trade per market: direction = sign of (kronos_p - up_ask).
    Only trade when |edge| > threshold. Normalize per $1 contract."""
    edge = df["kronos_p_up"] - df["up_ask"]
    taken = df[edge.abs() > threshold].copy()
    if taken.empty:
        return {"n": 0, "pnl": 0.0, "win_rate": float("nan")}
    pnls, wins = [], 0
    for _, r in taken.iterrows():
        go_up = r["kronos_p_up"] > r["up_ask"]
        won = bool(r["outcome_up"]) == go_up
        entry_ask = r["up_ask"] if go_up else r["down_ask"]
        if entry_ask is None or pd.isna(entry_ask) or entry_ask <= 0:
            continue
        pnl = (1 - entry_ask) / entry_ask if won else -1.0
        pnls.append(pnl)
        wins += int(won)
    if not pnls:
        return {"n": 0, "pnl": 0.0, "win_rate": float("nan")}
    return {
        "n": len(pnls),
        "pnl": float(np.sum(pnls)),
        "win_rate": wins / len(pnls),
        "avg_pnl": float(np.mean(pnls)),
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--markets", default="markets.csv")
    ap.add_argument("--forecasts", default="forecasts.csv")
    args = ap.parse_args()

    m = pd.read_csv(args.markets)
    f = pd.read_csv(args.forecasts)
    df = m.merge(f[["id", "kronos_p_up"]], on="id", how="inner")

    df["up_ask"] = pd.to_numeric(df["up_ask"], errors="coerce")
    df["down_ask"] = pd.to_numeric(df["down_ask"], errors="coerce")
    df = df.dropna(subset=["up_ask", "kronos_p_up", "outcome_up"])
    df = df[(df["up_ask"] > 0) & (df["up_ask"] < 1)]

    if df.empty:
        print("no usable rows after join")
        return

    y = df["outcome_up"].astype(int).to_numpy()
    p_mkt = df["up_ask"].to_numpy()
    p_krn = df["kronos_p_up"].to_numpy()

    print(f"\nN = {len(df)} markets\n")
    print(f"{'':20s}{'log-loss':>12s}{'brier':>10s}")
    print(f"{'market up_ask':20s}{log_loss(p_mkt, y):>12.4f}{brier(p_mkt, y):>10.4f}")
    print(f"{'kronos P(up)':20s}{log_loss(p_krn, y):>12.4f}{brier(p_krn, y):>10.4f}")

    # Correlation of signal disagreement with outcome direction.
    edge = p_krn - p_mkt
    outcome_signed = 2 * y - 1  # +1 up, -1 down
    corr = np.corrcoef(edge, outcome_signed)[0, 1] if len(edge) > 1 else float("nan")
    print(f"\ncorr(kronos − market, outcome) = {corr:+.4f}")

    print("\nPnL sim — enter when |kronos − market| > threshold, $1/trade:")
    print(f"{'thresh':>8s}{'n':>6s}{'win%':>8s}{'total':>10s}{'per trade':>12s}")
    for t in (0.0, 0.02, 0.05, 0.10, 0.15, 0.20):
        r = sim_pnl(df, t)
        wr = f"{r['win_rate']*100:.1f}" if not math.isnan(r['win_rate']) else "—"
        pt = f"{r.get('avg_pnl', 0):+.4f}" if r["n"] else "—"
        print(f"{t:>8.2f}{r['n']:>6d}{wr:>8s}{r['pnl']:>+10.3f}{pt:>12s}")


if __name__ == "__main__":
    main()
