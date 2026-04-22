# Kronos POC

One-afternoon experiment: does Kronos's short-horizon BTC forecast give us
edge vs. Polymarket's `up_ask` as a signal for 5m Up/Down markets?

## Setup

```bash
cd kronos_poc
python3.10 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt

# Kronos isn't on PyPI — clone it next door:
git clone https://github.com/shiyu-coder/Kronos ../third_party/Kronos
export KRONOS_PATH=$(cd ../third_party/Kronos && pwd)
```

Needs Supabase creds in env (same as the bot):

```bash
export SUPABASE_URL=...
export SUPABASE_SERVICE_ROLE_KEY=...
```

## Pipeline

```bash
# 1. Pull our historical rollover trades (~355 at current count).
python fetch_markets.py --out markets.csv --limit 500

# 2. Run Kronos forecast for each market (downloads Kronos-small on first run).
#    Fetches matching 1m BTC candles from Binance public API.
python run_forecast.py --in markets.csv --out forecasts.csv --samples 30

# 3. Evaluate — log-loss vs market up_ask + PnL sim over disagreement thresholds.
python evaluate.py --markets markets.csv --forecasts forecasts.csv
```

## What to look at

`evaluate.py` prints:
- **Log-loss of `up_ask` vs outcomes** (baseline — the market's own probability).
- **Log-loss of Kronos `P(up)` vs outcomes** (our candidate signal).
- **PnL simulation**: enter when `|kronos_p - up_ask| > threshold`, sweep thresholds.

If Kronos log-loss ≥ market log-loss at every threshold, the signal has no edge
on this sample. If the PnL curve has a region where edge is consistent and the
sample is ≥100 trades, that's worth a second, larger pass.

## Limits of this POC

- 355 trades is small. Signal needs to be strong to show through the noise.
- Kronos-small is pre-trained, not fine-tuned on Polymarket 5m BTC. Fine-tuning
  is a separate project (see Kronos `finetune/` dir).
- Binance USDT price ≠ Polymarket strike reference, but the directional
  correlation is ~1.0 over a 5m window.
