# Session Summary — Feb 25, 2026

## What Was Wrong

The bot was losing money: 3 wins / 7 losses, -$75.97 total. Profit factor 0.31.

**Root causes:**
1. Losses averaged -$16, wins averaged +$11 — asymmetric risk/reward
2. Every trade entered with RSI 89 (overbought) buying UP — buying at the top
3. MFE/MAE tracking never persisted to trade records (bug)
4. Trailing take-profit thresholds too loose ($20 start) — rarely triggered
5. After deploy, balance/win rate/daily PnL reset to zero because they read from empty local file instead of Supabase

## What Was Fixed (3 commits, pushed to main)

### Commit 1: `2aaa14b` — Profitability fixes
- **MFE/MAE bug**: Now writes tracked max/min PnL to trade record before close
- **Trailing TP tightened**: Start $20→$3, drawdown $10→$1.50 (capture small wins)
- **RSI filter added**: Blocks UP entries when RSI > 78, DOWN when RSI < 22
- **Max loss lowered**: 20%→12% of contract size (~$9.60 vs ~$16 per trade), ceiling $40→$20
- **Stale candle blocker**: Blocks entry if last candle > 2 min old

### Commit 2: `56ea3d1` — Test fixes
- Fixed all 10 pre-existing test failures (420 pass, 0 fail)

### Commit 3: `cb7f3b9` — Deploy state persistence
- Balance/summary now reads from Supabase instead of empty local JSON ledger
- Daily PnL seeded from Supabase on boot (kill switch works after deploy)
- Added `getTradesByDateRange()` to Supabase store

## What You Need To Do

1. **Wait for DO auto-deploy** (or trigger manually)
2. **Click "Start Trading"** in the dashboard — trading is disabled by default on boot
3. **Monitor** the next 20+ trades to see if the new settings improve profitability

## Expected Impact

| Metric | Before | Expected |
|--------|--------|----------|
| Avg loss | -$16 | -$9.60 |
| Trailing TP activation | Rare ($20 threshold) | Frequent ($3 threshold) |
| Overbought entries | Allowed (RSI 89) | Blocked (RSI > 78) |
| Balance after deploy | Shows $1000 (wrong) | Shows actual balance |
| Kill switch after deploy | Starts at $0 (wrong) | Seeded from Supabase |

## Config Overrides (env vars on DO if you want to tune)

| Var | Default | Purpose |
|-----|---------|---------|
| `TRAILING_TAKE_PROFIT_START_USD` | 3 | Trailing TP activates after this much unrealized profit |
| `TRAILING_TAKE_PROFIT_DRAWDOWN_USD` | 1.50 | Exit if profit drops this much from peak |
| `DYNAMIC_STOP_LOSS_PCT` | 0.12 | Max loss as % of position size |
| `MAX_MAX_LOSS_USD` | 20 | Absolute max loss ceiling |
| `NO_TRADE_RSI_OVERBOUGHT` | 78 | Block UP entries above this RSI |
| `NO_TRADE_RSI_OVERSOLD` | 22 | Block DOWN entries below this RSI |
