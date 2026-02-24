# Polymarket BTC 5m Assistant

A high-frequency trading bot for Polymarket **"Bitcoin Up or Down" 5-minute** contracts. Monitors BTC price movements, calculates trading signals using technical indicators (RSI, MACD, VWAP), and executes trades in both **paper trading mode** (simulated) and **live trading mode** (CLOB via Polymarket API).

## Features

### Dual-Mode Trading
- **Paper mode**: Simulated fills using Polymarket orderbook data, local JSON ledger
- **Live mode**: Real CLOB execution via Polymarket API with full order lifecycle management

### Market Data & Signals
- Auto-selects latest 5m Polymarket market (or pin via `POLYMARKET_SLUG`)
- BTC reference price from Chainlink (Polymarket live feed + on-chain Polygon fallback)
- Coinbase spot price for impulse/basis comparisons
- Kraken REST for candle seeding/backfill
- 1-minute candles built from ticks with warm-start backfill

### Technical Indicators
- **Heiken Ashi** (trend confirmation with consecutive candle count)
- **RSI** with slope detection (momentum + direction)
- **MACD** with histogram delta (momentum strength + acceleration)
- **VWAP** with slope and distance (mean reversion + trend)
- Market regime detection (Oversold / Ranging / Overbought)

### Trading Controls
- 24 entry blockers (risk, time, indicator, market quality)
- 8 exit conditions (rollover, probability flip, take profit, max loss, settlement, trailing TP)
- Dynamic bankroll-based position sizing with min/max bounds
- Circuit breaker (consecutive loss cooldown)
- Daily PnL kill-switch with midnight PT reset and manual override
- Weekend tightening (stricter thresholds when markets are thin)

### Analytics & Optimization
- **Period analytics**: Performance by day, week, and trading session
- **Segmented views**: Win rate / profit factor by entry phase, session, market regime
- **Advanced metrics**: Sharpe ratio, Sortino ratio, max drawdown (USD + %)
- **Backtest harness**: Replay historical trades with modified thresholds
- **Grid search optimizer**: Test parameter combinations, ranked by profit factor
- **Suggestion engine**: Analyzes blocker frequency, suggests threshold adjustments with projected impact

### Live Trading Hardening
- Full order lifecycle tracking (SUBMITTED -> PENDING -> FILLED -> MONITORING -> EXITED)
- Position reconciliation (local vs. CLOB state comparison every tick)
- Fee-aware position sizing
- Retry with exponential backoff (1s -> 2s -> 4s) + circuit breaker for CLOB failures

### Infrastructure & Reliability
- **SQLite persistence**: Structured trade store with 20+ fields per trade (JSON ledger fallback)
- **Webhook alerts**: Slack/Discord notifications for critical events (kill-switch, crash, circuit breaker)
- **Crash recovery**: PID lock detection, state persistence, automatic restoration on restart
- **Zero-downtime deployment**: Trading lock for instance coordination, graceful SIGTERM drain
- **Health endpoint**: `/health` for load balancer probes

### Dashboard
- Three-tab web UI (Dashboard, Analytics, Optimizer)
- Real-time status with 1.5s polling
- Compact status bar (Mode, Trading, Kill-switch, SQLite, Webhooks, Uptime)
- Trade history with filtering (limit, reason, side, losses only)
- Equity curve chart, drawdown chart
- Kill-switch progress bar with override button
- Order lifecycle badges, sync indicator dot
- Instance locking for multi-server production

## Requirements

- **Node.js 18+** (recommended: 20 LTS)
- **npm** (bundled with Node.js)
- **better-sqlite3** (optional, for structured trade persistence)

## Quick Start

```bash
# 1. Clone
git clone <repo-url>
cd polymarket-btc-5m-assistant

# 2. Install
npm install

# 3. Configure (optional)
cp .env.example .env
# Edit .env with your settings

# 4. Run pre-flight checks
npm run preflight

# 5. Start
npm start

# 6. Open dashboard
# http://localhost:3000
```

## Configuration

All configuration is via environment variables (or `.env` file). See `src/config.js` for the full list.

### Essential Settings

| Variable | Description | Default |
|----------|-------------|---------|
| `POLYMARKET_AUTO_SELECT_LATEST` | Auto-pick latest 5m market | `true` |
| `POLYGON_RPC_URL` | Chainlink BTC price feed | `https://polygon-rpc.com` |
| `STARTING_BALANCE` | Paper trading bankroll | `1000` |
| `STAKE_PCT` | Position size as % of balance | `0.08` (8%) |
| `DAILY_LOSS_LIMIT` | Kill-switch threshold (USD) | `50` |
| `UI_PORT` | Dashboard port | `8080` |

### Live Trading

| Variable | Description | Default |
|----------|-------------|---------|
| `LIVE_TRADING_ENABLED` | Enable live CLOB trading | `false` |
| `FUNDER_ADDRESS` | CLOB funder wallet | (required) |
| `LIVE_MAX_PER_TRADE_USD` | Max trade size | `7` |
| `LIVE_MAX_DAILY_LOSS_USD` | Live daily loss limit | `30` |

### Monitoring

| Variable | Description | Default |
|----------|-------------|---------|
| `WEBHOOK_URL` | Slack/Discord webhook URL | (optional) |
| `WEBHOOK_TYPE` | `slack` or `discord` | (optional) |
| `DATA_DIR` | Data directory for SQLite, state files | `./data` |

For complete configuration reference, proxy setup, and deployment guide, see [DEPLOYMENT.md](./DEPLOYMENT.md).

## Testing

```bash
# Run all tests (unit + integration)
npm test

# Pre-flight check (tests + env validation)
npm run preflight
```

## Architecture

```
src/
  domain/           Pure functions (no side effects)
    entryGate.js      24 entry blockers
    exitEvaluator.js  8 exit conditions
    sizing.js         Dynamic trade size
    killSwitch.js     Kill-switch logic
    orderLifecycle.js Order state machine
    retryPolicy.js    Error classification + retry
    reconciliation.js Position reconciliation
    backtester.js     Trade replay engine
    optimizer.js      Grid search

  application/      Orchestration + state
    TradingEngine.js  Main loop orchestrator
    TradingState.js   Mutable session state
    ModeManager.js    Paper/Live toggle

  infrastructure/   External I/O
    executors/        PaperExecutor, LiveExecutor
    persistence/      SQLite trade store
    webhooks/         Slack/Discord alerting
    recovery/         Crash detection + state persistence
    deployment/       Trading lock, graceful shutdown

  services/         Application services
    analyticsService.js   Trade analytics
    backtestService.js    Backtest orchestration
    suggestionService.js  Threshold suggestions

  ui/               Dashboard
    index.html, script.js, style.css, analytics.js
    server.js         Express API routes
```

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check for load balancers |
| `/api/status` | GET | Engine state + signals + entry debug |
| `/api/trades` | GET | Paper trade history |
| `/api/analytics` | GET | Period analytics + advanced metrics |
| `/api/trading/start` | POST | Enable trading |
| `/api/trading/stop` | POST | Disable trading |
| `/api/mode` | POST | Switch paper/live mode |
| `/api/backtest` | POST | Run backtest with custom params |
| `/api/optimizer` | POST | Run grid search optimizer |
| `/api/kill-switch/status` | GET | Daily PnL vs. limit |
| `/api/kill-switch/override` | POST | Override kill-switch |
| `/api/metrics` | GET | Operational metrics |
| `/api/diagnostics` | GET | Entry blocker diagnostics |

## Documentation

- **[DEPLOYMENT.md](./DEPLOYMENT.md)** - Production deployment guide, DigitalOcean setup, troubleshooting
- **[CHANGELOG.md](./CHANGELOG.md)** - Release history
- **[CLAUDE.md](./CLAUDE.md)** - Codebase reference for Claude Code

## Safety

This is not financial advice. Use at your own risk. Always start in paper mode and validate with the backtest harness before enabling live trading.

---

created by @krajekis
